"""Reproducible local acquisition scheduler benchmark.

All HTTP traffic is loopback. See docs/acquisition-benchmark-protocol.md.
"""

from __future__ import annotations

import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import platform
import random
import socket
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
from concurrent.futures import ThreadPoolExecutor

import numpy as np
import rasterio
from rasterio.shutil import copy as raster_copy
from rasterio.windows import Window

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/acquisition-engine"
BASELINE = WORK / "baseline-tile-ripper"
CURRENT = ROOT / "target/release/tile-ripper"
GDAL_ENV = {
    "GDAL_DISABLE_READDIR_ON_OPEN": "EMPTY_DIR",
    "GDAL_INGESTED_BYTES_AT_OPEN": "32768",
    "GDAL_HTTP_MAX_RETRY": "3",
    "GDAL_HTTP_TIMEOUT": "45",
}


class SharedLink:
    """Token-free leaky bucket: all response writers share one byte budget."""

    def __init__(self, bytes_per_second: float | None):
        self.rate = bytes_per_second
        self.lock = threading.Lock()
        self.next_free = time.monotonic()

    def transfer(self, size: int) -> None:
        if not self.rate:
            return
        with self.lock:
            now = time.monotonic()
            start = max(now, self.next_free)
            self.next_free = start + size / self.rate
            wait = self.next_free - now
        if wait > 0:
            time.sleep(wait)


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def configure(self, objects: dict[str, Path], latency_ms: float,
                  bandwidth: float | None, fail_first: bool = False):
        self.objects = objects
        self.etags = {path: '"' + hashlib.sha256(source.read_bytes()).hexdigest() + '"'
                      for path, source in objects.items()}
        self.latency = latency_ms / 1000
        self.link = SharedLink(bandwidth)
        self.fail_first = fail_first
        self.lock = threading.Lock()
        self.reset()

    def reset(self):
        with getattr(self, "lock", threading.Lock()):
            self.counts = {"head": 0, "get": 0, "range_get": 0,
                           "unique_requests": 0, "conditional": 0,
                           "attempts": 0, "retries": 0, "body_bytes": 0,
                           "status_503": 0, "status_404": 0, "active_requests": 0,
                           "peak_active_requests": 0, "paths": {}}
            self.pending_retry = set()

    def handle_error(self, request, client_address):
        # Canceled probes are expected when readers abandon HEAD/metadata checks.
        pass


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self):
        super().setup()
        self.request.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def log_message(self, *_):
        pass

    def do_HEAD(self):
        self.serve(False)

    def do_GET(self):
        self.serve(True)

    def serve(self, body: bool):
        path = urllib.parse.urlsplit(self.path).path
        source = self.server.objects.get(path)
        if source is None:
            self.record_missing(path, body)
            self.send_error(404)
            return
        blob_size = source.stat().st_size
        etag = self.server.etags[path]
        rng = self.headers.get("Range")
        start, end = 0, blob_size - 1
        if rng:
            try:
                unit, values = rng.split("=", 1)
                a, b = values.split("-", 1)
                if unit != "bytes" or not a:
                    raise ValueError
                start, end = int(a), min(blob_size - 1, int(b) if b else blob_size - 1)
                if start > end or start >= blob_size:
                    raise ValueError
            except ValueError:
                self.send_error(416)
                return
        key = ("HEAD" if not body else "GET") + " " + path
        signature = key + " " + (rng or "whole") + " " + str(self.headers.get("If-Match", ""))
        with self.server.lock:
            counts = self.server.counts
            counts["attempts"] += 1
            row = counts["paths"].setdefault(key, {"attempts": 0, "bytes": 0, "ranges": 0,
                                                  "request_signatures": {}})
            row["attempts"] += 1
            if rng:
                row["ranges"] += 1
            prior = row["request_signatures"].get(signature, 0)
            if signature in self.server.pending_retry:
                counts["retries"] += 1
                self.server.pending_retry.remove(signature)
            else:
                if prior == 0:
                    counts["unique_requests"] += 1
            row["request_signatures"][signature] = prior + 1
            counts["active_requests"] += 1
            counts["peak_active_requests"] = max(counts["peak_active_requests"],
                                                   counts["active_requests"])
            if body:
                counts["get"] += 1
                if rng:
                    counts["range_get"] += 1
            else:
                counts["head"] += 1
            if self.headers.get("If-Match") or self.headers.get("If-None-Match"):
                counts["conditional"] += 1
            conditional_failure = (
                (self.headers.get("If-Match") and self.headers["If-Match"] != etag)
                or self.headers.get("If-None-Match") == "*"
                or self.headers.get("If-None-Match") == etag
            )
            if conditional_failure:
                counts["active_requests"] -= 1
            fail = self.server.fail_first and prior == 0
            if fail:
                counts["status_503"] += 1
                self.server.pending_retry.add(signature)
        if self.server.latency:
            time.sleep(self.server.latency)
        if conditional_failure:
            self.send_response(412)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if fail:
            self.send_response(503)
            self.send_header("Content-Length", "0")
            self.send_header("Retry-After", "0")
            self.end_headers()
            with self.server.lock:
                self.server.counts["active_requests"] -= 1
            return
        self.send_response(206 if rng else 200)
        self.send_header("Content-Length", str(end - start + 1))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("ETag", etag)
        if rng:
            self.send_header("Content-Range", f"bytes {start}-{end}/{blob_size}")
        self.end_headers()
        if body:
            with source.open("rb") as stream:
                stream.seek(start)
                remain = end - start + 1
                while remain:
                    chunk = stream.read(min(remain, 64 * 1024))
                    if not chunk:
                        break
                    self.server.link.transfer(len(chunk))
                    self.wfile.write(chunk)
                    self.wfile.flush()
                    remain -= len(chunk)
                    with self.server.lock:
                        self.server.counts["body_bytes"] += len(chunk)
                        self.server.counts["paths"][key]["bytes"] += len(chunk)
        with self.server.lock:
            self.server.counts["active_requests"] -= 1

    def record_missing(self, path: str, body: bool):
        key = ("GET" if body else "HEAD") + " " + path
        with self.server.lock:
            counts = self.server.counts
            counts["attempts"] += 1
            counts["unique_requests"] += 1
            counts["get" if body else "head"] += 1
            counts["status_404"] += 1
            counts["active_requests"] += 1
            counts["peak_active_requests"] = max(counts["peak_active_requests"],
                                                   counts["active_requests"])
            counts["paths"][key] = {"attempts": 1, "bytes": 0, "ranges": 0,
                                     "request_signatures": {key + " whole ": 1}}
        if self.server.latency:
            time.sleep(self.server.latency)
        with self.server.lock:
            self.server.counts["active_requests"] -= 1


def process(cmd: list[str], output: Path, env: dict[str, str]):
    """Run a fresh process; observe first output file and wait4 peak RSS."""
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        start = time.perf_counter()
        child_env = {**os.environ, **GDAL_ENV}
        child_env.pop("TILE_RIPPER_WORKERS", None)
        child_env.pop("TILE_RIPPER_PER_HOST", None)
        child_env.update(env)
        proc = subprocess.Popen(cmd, cwd=ROOT, stdout=stdout, stderr=stderr,
                                env=child_env)
        first_file_seconds = None
        waited = None
        while waited is None:
            if first_file_seconds is None and any(p.is_file() for p in output.rglob("*")):
                first_file_seconds = time.perf_counter() - start
            waited = os.wait4(proc.pid, os.WNOHANG)
            if waited[0] == 0:
                waited = None
                time.sleep(.01)
        wall = time.perf_counter() - start
        _, status, usage = waited
        code = os.waitstatus_to_exitcode(status)
        stdout.seek(0)
        stderr.seek(0)
        out_text, err_text = stdout.read().decode(errors="replace"), stderr.read().decode(errors="replace")
    result = None
    if code == 0:
        try:
            result = json.loads(out_text)
        except json.JSONDecodeError:
            err_text += "\nbenchmark: fetch stdout was not JSON"
            code = 65
    proc.returncode = code
    return {"exit_code": code, "wall_seconds": wall,
            "first_file_seconds": first_file_seconds,
            "peak_rss_bytes": usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024),
            "result": result, "stderr": err_text[:3000]}


def attach_stage_metrics(row: dict):
    result = row.get("result") or {}
    observed = result.get("observed", {}) if isinstance(result, dict) else {}
    row["source_retrieval_seconds"] = observed.get("source_retrieval_seconds")
    row["read_decode_seconds"] = observed.get("read_decode_seconds", result.get("read_decode_seconds"))
    row["output_seconds"] = (
        observed.get("sink_seconds", 0.0) + observed.get("output_finalize_seconds", 0.0)
        if observed else result.get("write_seconds")
    )
    row["reported_first_output_seconds"] = observed.get("first_output_seconds")
    if observed.get("first_output_seconds") is not None:
        row["reader_reported_ttfc_seconds"] = (
            observed["planning_seconds"] + observed["first_output_seconds"]
        )
        row["ttfc_scope"] = "planning plus execution; excludes process startup"
    else:
        row["reader_reported_ttfc_seconds"] = result.get("usable_ttfc_seconds")
        row["ttfc_scope"] = ("worker start through first complete output" if
                              row["reader_reported_ttfc_seconds"] is not None else None)
    row["output_io"] = observed.get("output_io")
    row["scheduler"] = observed.get("scheduler")


def source_hash():
    digest = hashlib.sha256()
    for path in sorted((ROOT / "src").rglob("*.rs")):
        digest.update(path.relative_to(ROOT).as_posix().encode())
        digest.update(path.read_bytes())
    return digest.hexdigest()


def create_cog(root: Path, n: int = 8):
    """Make one deterministic, Sentinel-like synthetic COG and mirror routes."""
    root.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(20261019)
    staging, cog = root / "staging.tif", root / "source.tif"
    if not cog.exists():
        data = rng.integers(1, 60000, (2048, 2048), dtype=np.uint16)
        data[:24, :24] = 0
        with rasterio.open(staging, "w", driver="GTiff", width=2048, height=2048,
                           count=1, dtype="uint16", crs="EPSG:32617",
                           transform=rasterio.transform.from_origin(600000, 4000020, 10, 10),
                           nodata=0, tiled=True, blockxsize=256, blockysize=256) as ds:
            ds.write(data, 1)
            ds.scales = (0.0001,)
            ds.offsets = (-0.1,)
        raster_copy(staging, cog, driver="COG", BLOCKSIZE=256,
                    COMPRESS="DEFLATE", PREDICTOR=2)
        staging.unlink()
    objects = {}
    assets = []
    for i in range(n):
        route = f"/asset-{i}.tif"
        objects[route] = cog
        assets.append({"id": f"synthetic-{i:04d}", "locator": route,
                       "format": "cog", "datetime": "2025-06-01T00:00:00Z"})
    return cog, objects, assets


def verify_cog(outdir: Path, source: Path, request: dict):
    """Validate every output pixel and complete tiled coverage independently."""
    manifest = json.loads((outdir / "manifest.json").read_text())
    outputs = [p for p in outdir.glob("*.tif") if p.is_file()]
    records = manifest.get("outputs", [])
    if not outputs or not records:
        raise AssertionError("completion directory has no chunk TIFFs")
    with rasterio.open(source) as src:
        bbox = request["spatial"]["bbox"]
        inv = ~src.transform
        c0, r0 = inv * (bbox[0], bbox[3])
        c1, r1 = inv * (bbox[2], bbox[1])
        left, top = max(0, int(np.floor(c0))), max(0, int(np.floor(r0)))
        right, bottom = min(src.width, int(np.ceil(c1))), min(src.height, int(np.ceil(r1)))
        width, height = right-left, bottom-top
        edge = request["budget"]["chunk_edge"]
        per_asset = ((width + edge - 1) // edge) * ((height + edge - 1) // edge)
        expected_files = per_asset * len(request["source"]["assets"])
        if len(outputs) != expected_files or len(records) != expected_files:
            raise AssertionError(f"expected {expected_files} TIFF chunks and records, found {len(outputs)} files/{len(records)} records")
        expected_ids = {asset["id"]: asset.get("datetime") for asset in request["source"]["assets"]}
        counts = {asset_id: 0 for asset_id in expected_ids}
        coverage = {asset_id: np.zeros((height, width), dtype=np.uint16)
                    for asset_id in expected_ids}
        files_by_name = {p.name: p for p in outputs}
        if len(files_by_name) != len(outputs):
            raise AssertionError("duplicate output filenames")
        if {r.get("file") for r in records} != set(files_by_name):
            raise AssertionError("manifest output files do not match the completed TIFF set")
        total_values = 0
        for record in records:
            asset_id = record.get("asset_id")
            if asset_id not in expected_ids:
                raise AssertionError(f"unexpected output asset id: {asset_id}")
            if record.get("timestamps") != [expected_ids[asset_id]]:
                raise AssertionError(f"output datetime differs from request for {asset_id}")
            counts[asset_id] += 1
            path = files_by_name[record["file"]]
            with rasterio.open(path) as got:
                actual = got.read(1)
                col, row = (~src.transform) * (got.transform.c, got.transform.f)
                col, row = round(col), round(row)
                if col < left or row < top or col + got.width > right or row + got.height > bottom:
                    raise AssertionError(f"output chunk outside selected extent: {path}")
                ref = src.read(1, window=Window(col, row, got.width, got.height))
                np.testing.assert_array_equal(actual, ref)
                assert got.crs == src.crs and got.nodata == src.nodata
                assert got.scales == src.scales and got.offsets == src.offsets
                assert got.transform == src.window_transform(Window(col, row, got.width, got.height))
                assert actual.size > 0
                coverage[asset_id][row-top:row-top+got.height, col-left:col-left+got.width] += 1
                total_values += int(actual.size)
        if any(count != per_asset for count in counts.values()):
            raise AssertionError(f"incorrect per-asset output counts: {counts}")
        for asset_id, values in coverage.items():
            np.testing.assert_array_equal(values, np.ones_like(values))
    return {"validated_output_files": len(outputs), "validated_values": total_values,
            "complete_asset_pixel_coverage": True}


def gdal_worker(request_path: Path, outdir: Path, workers: int):
    """Independent rasterio/GDAL threaded materializer used by the pilot."""
    worker_start = time.perf_counter()
    first_output = []
    first_lock = threading.Lock()
    request = json.loads(request_path.read_text())
    outdir.mkdir(parents=True, exist_ok=False)
    bbox = request["spatial"]["bbox"]
    records = []

    def acquire(asset):
        read_seconds = write_seconds = 0.0
        with rasterio.open(asset["locator"]) as src:
            inv = ~src.transform
            c0, r0 = inv * (bbox[0], bbox[3])
            c1, r1 = inv * (bbox[2], bbox[1])
            col, row = max(0, int(np.floor(c0))), max(0, int(np.floor(r0)))
            right, bottom = min(src.width, int(np.ceil(c1))), min(src.height, int(np.ceil(r1)))
            edge = request["budget"]["chunk_edge"]
            outputs = []
            for iy, y in enumerate(range(row, bottom, edge)):
                for ix, x in enumerate(range(col, right, edge)):
                    win = Window(x, y, min(edge, right-x), min(edge, bottom-y))
                    started = time.perf_counter()
                    values = src.read(1, window=win)
                    read_seconds += time.perf_counter() - started
                    name = f"{asset['id']}-chunk-{iy}-{ix}.tif"
                    started = time.perf_counter()
                    with rasterio.open(outdir / name, "w", driver="GTiff",
                                       width=values.shape[1], height=values.shape[0],
                                       count=1, dtype=src.dtypes[0], crs=src.crs,
                                       transform=src.window_transform(win),
                                       nodata=src.nodata) as dst:
                        dst.write(values, 1)
                        dst.scales, dst.offsets = src.scales, src.offsets
                    write_seconds += time.perf_counter() - started
                    with first_lock:
                        if not first_output:
                            first_output.append(time.perf_counter() - worker_start)
                    outputs.append({"asset_id": asset["id"], "datetime": asset.get("datetime"),
                                    "timestamps": [asset.get("datetime")], "file": name})
            return outputs, read_seconds, write_seconds

    with rasterio.Env(GDAL_NUM_THREADS="1", GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
                      GDAL_INGESTED_BYTES_AT_OPEN="32768", GDAL_HTTP_MAX_RETRY="3",
                      GDAL_HTTP_TIMEOUT="45"):
        with ThreadPoolExecutor(max_workers=workers) as pool:
            batches = list(pool.map(acquire, request["source"]["assets"]))
            records = [item for batch, _, _ in batches for item in batch]
    (outdir / "manifest.json").write_text(json.dumps({"outputs": records}))
    return {"outputs": records, "gdal": rasterio.__gdal_version__,
            "read_decode_seconds": sum(read for _, read, _ in batches),
            "write_seconds": sum(write for _, _, write in batches),
            "usable_ttfc_seconds": first_output[0] if first_output else None}


def create_zarr(root: Path):
    """Create deterministic one-variable cubes with 256 and 1024 data chunks."""
    import zarr
    from numcodecs import GZip

    objects: dict[str, Path] = {}
    requests = {}
    for name, side in (("zarr-256", 256), ("zarr-1024", 512)):
        path = root / f"{name}.zarr"
        shutil.rmtree(path, ignore_errors=True)
        group = zarr.open_group(str(path), mode="w")
        group.attrs.update({"tile_ripper_grid": {"crs": "EPSG:32617",
                            "transform": [500000, 10, 0, 4000000, 0, -10]}})
        times = np.array([1735689600], dtype="<i8")
        x = 500000 + (np.arange(side) + .5) * 10
        y = 4000000 - (np.arange(side) + .5) * 10
        data = (np.arange(side * side).reshape(1, side, side) % 60000 + 1).astype("<u2")
        for key, value, chunks, dims in [
            ("time", times, (1,), ["time"]),
            ("x", x, (side,), ["x"]),
            ("y", y, (side,), ["y"]),
            ("red", data, (1, 16, 16), ["time", "y", "x"]),
        ]:
            arr = group.create_dataset(key, data=value, chunks=chunks,
                                       compressor=GZip(1) if key == "red" else None,
                                       fill_value=0 if key == "red" else None,
                                       dimension_separator=".")
            arr.attrs["_ARRAY_DIMENSIONS"] = dims
            if key == "time":
                arr.attrs.update({"units": "seconds since 1970-01-01 00:00:00",
                                  "calendar": "proleptic_gregorian"})
            if key == "red":
                arr.attrs.update({"_FillValue": 0, "units": "1"})
        zarr.consolidate_metadata(str(path))
        for item in path.rglob("*"):
            if item.is_file():
                route = f"/{name}.zarr/" + item.relative_to(path).as_posix()
                objects[route] = item
        bbox = ([500000, 3997440, 502560, 4000000] if side == 256 else
                [500000, 3994880, 505120, 4000000])
        request = {"source": {"kind": "assets", "assets": [{
            "id": name, "locator": str(path), "format": "zarr_v2"}]},
            "spatial": {"kind": "bounds", "crs": "EPSG:32617", "bbox": bbox},
            "time": {"kind": "interval", "start": "2025-01-01T00:00:00Z",
                     "end": "2025-01-02T00:00:00Z"},
            "variables": [{"kind": "dataset", "path": "/red"}],
            "grid": {"kind": "native"},
            "sink": {"kind": "zarr_directory", "path": str(root / f"{name}-out")},
            "budget": {"buffer_bytes": 4194304, "max_nodes": 100000,
                       "chunk_edge": 16}}
        requests[name] = request
    return objects, requests


def xarray_worker(request_path: Path, outdir: Path):
    """HTTP-backed xarray/Zarr reader control and exact value oracle.

    This control intentionally does not claim Tile-Ripper's per-object ETag
    snapshot validation. Its HTTP byte ranges are bounded to the same four
    concurrent requests, and its output objects are individually synced.
    """
    worker_start = time.perf_counter()
    import fsspec
    import xarray as xr
    import aiohttp

    request = json.loads(request_path.read_text())
    asset = request["source"]["assets"][0]
    remote = asset["locator"]
    local = os.environ["ACQ_BENCH_LOCAL_SOURCE"]
    batch_size = int(os.environ.get("ACQ_BENCH_XARRAY_BATCH_SIZE", "4"))

    async def limited_client(loop=None, **kwargs):
        connector = aiohttp.TCPConnector(limit=batch_size, limit_per_host=batch_size)
        return aiohttp.ClientSession(connector=connector, **kwargs)

    read_start = time.perf_counter()
    remote_store = fsspec.get_mapper(remote, get_client=limited_client)
    remote_ds = xr.open_zarr(remote_store, consolidated=True,
                             decode_cf=False, chunks=None)
    local_ds = xr.open_zarr(local, consolidated=True, decode_cf=False, chunks=None)
    xmin, ymin, xmax, ymax = request["spatial"]["bbox"]
    exp = local_ds[["red"]].where(
        (local_ds.x >= xmin) & (local_ds.x <= xmax), drop=True
    ).where((local_ds.y >= ymin) & (local_ds.y <= ymax), drop=True)
    actual = remote_ds[["red"]].where(
        (remote_ds.x >= xmin) & (remote_ds.x <= xmax), drop=True
    ).where((remote_ds.y >= ymin) & (remote_ds.y <= ymax), drop=True)
    actual.load()
    read_seconds = time.perf_counter() - read_start
    np.testing.assert_array_equal(actual.red.values, exp.red.values)
    np.testing.assert_array_equal(actual.x.values, exp.x.values)
    np.testing.assert_array_equal(actual.y.values, exp.y.values)
    np.testing.assert_array_equal(actual.time.values, exp.time.values)
    np.testing.assert_equal(actual.red.dtype, exp.red.dtype)
    outdir.mkdir(parents=True, exist_ok=False)
    t = time.perf_counter()
    from numcodecs import GZip
    import zarr

    class DurableStore(zarr.DirectoryStore):
        """Mirror zarrs_filesystem's per-object full synchronization."""
        def __init__(self, path):
            super().__init__(path)
            self.first_data_object_seconds = None

        def __setitem__(self, key, value):
            super().__setitem__(key, value)
            with open(Path(self.path) / key, "rb") as handle:
                # Darwin F_FULLFSYNC, matching the repository's durable writer.
                __import__("fcntl").fcntl(handle.fileno(), 51)
            if (key.startswith("red/") and not Path(key).name.startswith(".")
                    and self.first_data_object_seconds is None):
                self.first_data_object_seconds = time.perf_counter() - worker_start

    store = DurableStore(str(outdir / "extract.zarr"))
    actual.to_zarr(store, mode="w", consolidated=False,
                   encoding={"red": {"chunks": (1, 16, 16), "compressor": GZip(1)}})
    write_seconds = time.perf_counter() - t
    (outdir / "manifest.json").write_text(json.dumps({"outputs": [{"path": "extract.zarr"}]}))
    return {"validated_values": int(actual.red.size), "read_decode_seconds": read_seconds,
            "write_seconds": write_seconds,
            "usable_ttfc_seconds": store.first_data_object_seconds,
            "read_decode_scope": "remote metadata open, selection, and source read/decode combined",
            "output_durability": "Darwin F_FULLFSYNC after each Zarr object write",
            "source_snapshot_validation": "reader-only control; no ETag pin/If-Match"}


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "--gdal-worker":
        answer = gdal_worker(Path(sys.argv[2]), Path(sys.argv[3]), int(sys.argv[4]))
        print(json.dumps(answer))
        return
    if len(sys.argv) > 1 and sys.argv[1] == "--xarray-worker":
        answer = xarray_worker(Path(sys.argv[2]), Path(sys.argv[3]))
        print(json.dumps(answer))
        return
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--reps", type=int, default=5)
    ap.add_argument("--assets", type=int, default=16)
    ap.add_argument("--latency-ms", type=float, default=20)
    ap.add_argument("--bandwidth-mbps", type=float, default=100,
                    help="shared aggregate link rate; 0 disables throttling")
    ap.add_argument("--inject-503", action="store_true",
                    help="inject one transient 503 for each distinct method/path/range")
    ap.add_argument("--output", type=Path, default=WORK)
    ap.add_argument("--smoke", action="store_true",
                    help="run one small COG and one 256-chunk Zarr cell")
    ap.add_argument("--workloads", nargs="+",
                    help="optional exact workload names to run (for example many-small zarr-256)")
    ap.add_argument("--readers", nargs="+",
                    help="optional exact reader/config names to run")
    args = ap.parse_args()
    if args.reps < 1 or args.assets < 1:
        ap.error("--reps and --assets must be positive")
    work = args.output.resolve()
    work.mkdir(parents=True, exist_ok=True)
    (work / "runs.jsonl").write_text("")
    source_digest = source_hash()
    binary_hashes = {
        "baseline": hashlib.sha256(BASELINE.read_bytes()).hexdigest(),
        "current": hashlib.sha256(CURRENT.read_bytes()).hexdigest(),
    }
    fixture, objects, assets = create_cog(work / "fixtures", args.assets)
    server = Server(("127.0.0.1", 0), Handler)
    bandwidth = args.bandwidth_mbps * 1e6 / 8 if args.bandwidth_mbps else None
    server.configure(objects, args.latency_ms, bandwidth, args.inject_503)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    bbox_cases = {"many-small": [600100, 3999800, 600700, 4000020],
                  "few-large": [600000, 3986020, 614000, 4000020],
                  "overlap-a": [600100, 3999000, 601100, 4000020],
                  "overlap-b": [600500, 3999000, 601500, 4000020]}
    if args.smoke:
        bbox_cases = {"many-small": bbox_cases["many-small"]}
    runs = []
    request_hashes = {}
    req_path = work / "request.json"
    rng = random.Random(20261019)
    try:
        for workload, bbox in bbox_cases.items():
            if args.workloads and workload not in args.workloads:
                continue
            if workload == "few-large":
                selected = assets[:min(4, len(assets))]
            elif workload.startswith("overlap"):
                selected = assets[:min(8, len(assets))]
            else:
                selected = assets
            request = {"source": {"kind": "assets", "assets": [
                {**a, "locator": f"http://127.0.0.1:{server.server_port}{a['locator']}"}
                for a in selected]},
                "spatial": {"kind": "bounds", "crs": "EPSG:32617", "bbox": bbox},
                "time": {"kind": "interval", "start": "2025-01-01T00:00:00Z", "end": "2025-12-31T23:59:59Z"},
                "variables": [{"kind": "band", "index": 1}], "grid": {"kind": "native"},
                "sink": {"kind": "geotiff_directory", "path": str(work / "unused")},
                "budget": {"buffer_bytes": 1048576, "max_nodes": 100000, "chunk_edge": 256}}
            req_path.write_text(json.dumps(request, indent=2) + "\n")
            request_hashes[workload] = hashlib.sha256(req_path.read_bytes()).hexdigest()
            configs = (["baseline", "current-workers-1", "current-default", "gdal-workers-1"]
                       if args.smoke else
                       ["baseline", "current-workers-1", "current-default",
                        "gdal-workers-1", "gdal-workers-4", "gdal-workers-8"])
            if args.readers:
                configs = [config for config in configs if config in args.readers]
            for rep in range(args.reps):
                order = configs.copy()
                rng.shuffle(order)
                for config in order:
                    binary = BASELINE if config == "baseline" else CURRENT
                    outdir = work / f"{workload}-{config}-{rep}"
                    shutil.rmtree(outdir, ignore_errors=True)
                    server.reset()
                    workers = "1" if config == "current-workers-1" else "4"
                    overrides = ({"TILE_RIPPER_WORKERS": workers, "TILE_RIPPER_PER_HOST": "4"}
                                 if config != "current-default" else {})
                    if config.startswith("gdal-workers-"):
                        gdal_workers = config.rsplit("-", 1)[1]
                        command = [sys.executable, str(Path(__file__).resolve()),
                                   "--gdal-worker", str(req_path), str(outdir), gdal_workers]
                    else:
                        command = [str(binary), "fetch", str(req_path), "--output", str(outdir)]
                    row = {"workload": workload, "reader": config, "rep": rep,
                           "source_sha256": source_digest,
                           "binary_sha256": binary_hashes["baseline" if config == "baseline" else "current"]
                           if not config.startswith("gdal-workers-") else None,
                           "scenario": {"latency_ms": args.latency_ms,
                                        "shared_bandwidth_bytes_per_second": bandwidth,
                                        "synthetic_sentinel_like": True},
                           "concurrency_config": {
                               "tile_ripper_workers": (None if config in ("baseline", "current-default") else int(workers)),
                               "tile_ripper_per_host": (None if config in ("baseline", "current-default") else 4),
                               "gdal_workers": int(config.rsplit("-", 1)[1]) if config.startswith("gdal-workers-") else None}}
                    try:
                        measured = process(command, outdir, overrides)
                        row.update(measured)
                        attach_stage_metrics(row)
                        if measured["exit_code"] == 0:
                            row["correctness"] = verify_cog(outdir, fixture, request)
                            row["output_bytes"] = sum(p.stat().st_size for p in outdir.rglob("*") if p.is_file())
                            row["success"] = True
                        else:
                            row["success"] = False
                    except Exception as exc:
                        row.update(success=False, error=f"{type(exc).__name__}: {exc}")
                    row["http"] = json.loads(json.dumps(server.counts))
                    runs.append(row)
                    with (work / "runs.jsonl").open("a") as stream:
                        stream.write(json.dumps(row) + "\n")
                    print(f"{workload} {config} rep={rep} success={row['success']} wall={row.get('wall_seconds')}", file=sys.stderr)
        # Zarr workloads use the same shared loopback server and limiter.
        zarr_objects, zarr_requests = create_zarr(work / "fixtures")
        server.configure(zarr_objects, args.latency_ms, bandwidth, args.inject_503)
        zarr_workloads = {"zarr-256": zarr_requests["zarr-256"]} if args.smoke else zarr_requests
        for workload, base in zarr_workloads.items():
            if args.workloads and workload not in args.workloads:
                continue
            request = json.loads(json.dumps(base))
            local_source = request["source"]["assets"][0]["locator"]
            request["source"]["assets"][0]["locator"] = (
                f"http://127.0.0.1:{server.server_port}/{workload}.zarr")
            req_path.write_text(json.dumps(request, indent=2) + "\n")
            request_hashes[workload] = hashlib.sha256(req_path.read_bytes()).hexdigest()
            for rep in range(args.reps):
                order = (["baseline", "current-default", "xarray-http-4"] if args.smoke else
                         ["baseline", "current-workers-1", "current-default",
                          "xarray-http-4"])
                if args.readers:
                    order = [reader for reader in order if reader in args.readers]
                rng.shuffle(order)
                for reader in order:
                    outdir = work / f"{workload}-{reader}-{rep}"
                    shutil.rmtree(outdir, ignore_errors=True)
                    server.reset()
                    workers = "1" if reader == "current-workers-1" else "4"
                    command = ([sys.executable, str(Path(__file__).resolve()), "--xarray-worker",
                                str(req_path), str(outdir)] if reader == "xarray-http-4" else
                               [str(BASELINE if reader == "baseline" else CURRENT), "fetch",
                                str(req_path), "--output", str(outdir)])
                    row = {"workload": workload, "reader": reader, "rep": rep,
                           "source_sha256": source_digest,
                           "binary_sha256": binary_hashes["baseline" if reader == "baseline" else "current"]
                           if reader != "xarray-http-4" else None,
                           "scenario": {"latency_ms": args.latency_ms,
                                        "shared_bandwidth_bytes_per_second": bandwidth,
                                        "synthetic_zarr_chunks": True},
                           "concurrency_config": {
                               "tile_ripper_workers": (None if reader in ("baseline", "current-default", "xarray-http-4") else int(workers)),
                               "tile_ripper_per_host": (None if reader in ("baseline", "current-default", "xarray-http-4") else 4),
                               "xarray_batch_size": 4 if reader == "xarray-http-4" else None}}
                    try:
                        overrides = {"ACQ_BENCH_LOCAL_SOURCE": local_source}
                        if reader == "xarray-http-4":
                            overrides["ACQ_BENCH_XARRAY_BATCH_SIZE"] = "4"
                        if reader != "current-default":
                            overrides.update({"TILE_RIPPER_WORKERS": workers,
                                              "TILE_RIPPER_PER_HOST": "4"})
                        measured = process(command, outdir, overrides)
                        row.update(measured)
                        attach_stage_metrics(row)
                        if measured["exit_code"] == 0:
                            if reader == "xarray-http-4":
                                output_path = outdir / "extract.zarr"
                                import zarr
                                ar = zarr.open_group(str(output_path), mode="r")["red"]
                                assert ar.chunks == (1, 16, 16)
                                assert ar.compressor.codec_id == "gzip" and ar.compressor.level == 1
                                row["correctness"] = {"exact_values": True,
                                                       "validated_values": measured["result"]["validated_values"]}
                            else:
                                import xarray as xr
                                expected = xr.open_zarr(local_source, consolidated=False,
                                                        decode_cf=False, chunks=None)
                                xmin, ymin, xmax, ymax = request["spatial"]["bbox"]
                                expected = expected[["red"]].where(
                                    (expected.x >= xmin) & (expected.x <= xmax), drop=True
                                ).where((expected.y >= ymin) & (expected.y <= ymax), drop=True)
                                output_path = outdir / "extract.zarr"
                                if not output_path.exists():
                                    candidates = list(outdir.glob("*.zarr"))
                                    if len(candidates) != 1:
                                        raise AssertionError("could not identify Rust Zarr output")
                                    output_path = candidates[0]
                                actual = xr.open_zarr(output_path, consolidated=False,
                                                      decode_cf=False, chunks=None)
                                import zarr
                                ar = zarr.open_group(str(output_path), mode="r")["red"]
                                assert ar.chunks == (1, 16, 16)
                                assert ar.compressor.codec_id == "gzip" and ar.compressor.level == 1
                                np.testing.assert_array_equal(actual.red.values, expected.red.values)
                                np.testing.assert_array_equal(actual.x.values, expected.x.values)
                                np.testing.assert_array_equal(actual.y.values, expected.y.values)
                                np.testing.assert_array_equal(actual.time.values,
                                                              expected.time.values[:1])
                                row["correctness"] = {"exact_values": True,
                                                       "validated_values": int(actual.red.size)}
                            row["output_bytes"] = sum(p.stat().st_size for p in outdir.rglob("*") if p.is_file())
                            row["success"] = True
                        else:
                            row["success"] = False
                    except Exception as exc:
                        row.update(success=False, error=f"{type(exc).__name__}: {exc}")
                    row["http"] = json.loads(json.dumps(server.counts))
                    runs.append(row)
                    with (work / "runs.jsonl").open("a") as stream:
                        stream.write(json.dumps(row) + "\n")
                    print(f"{workload} {reader} rep={rep} success={row['success']} wall={row.get('wall_seconds')}", file=sys.stderr)
    finally:
        server.shutdown()
        server.server_close()
    summary = []
    for workload, reader in sorted({(r["workload"], r["reader"]) for r in runs}):
        rows = [r for r in runs if r["workload"] == workload and r["reader"] == reader]
        good = [r for r in rows if r.get("success")]
        vals = [r["wall_seconds"] for r in good]
        first = [r["reader_reported_ttfc_seconds"] for r in good
                 if r.get("reader_reported_ttfc_seconds") is not None]
        writes = [r["output_seconds"] for r in good if r.get("output_seconds") is not None]
        reads = [r["read_decode_seconds"] for r in good if r.get("read_decode_seconds") is not None]
        retrievals = [r["source_retrieval_seconds"] for r in good
                      if r.get("source_retrieval_seconds") is not None]
        http = [r["http"] for r in good]
        summary.append({"workload": workload, "reader": reader, "n": len(rows),
                        "successes": len(good), "failures": len(rows)-len(good),
                        "wall_median_seconds": statistics.median(vals) if vals else None,
                        "wall_min_seconds": min(vals) if vals else None,
                        "wall_max_seconds": max(vals) if vals else None,
                        "wall_stdev_seconds": statistics.stdev(vals) if len(vals)>1 else (0 if vals else None),
                        "reader_reported_first_output_median_seconds": statistics.median(first) if first else None,
                        "read_decode_median_seconds": statistics.median(reads) if reads else None,
                        "source_retrieval_median_seconds": statistics.median(retrievals) if retrievals else None,
                        "output_median_seconds": statistics.median(writes) if writes else None,
                        "median_http_attempts": statistics.median([v["attempts"] for v in http]) if http else None,
                        "median_http_unique_requests": statistics.median([v["unique_requests"] for v in http]) if http else None,
                        "median_http_retries": statistics.median([v["retries"] for v in http]) if http else None,
                        "median_http_body_bytes": statistics.median([v["body_bytes"] for v in http]) if http else None,
                        "median_http_peak_concurrency": statistics.median([v["peak_active_requests"] for v in http]) if http else None,
                        "peak_rss_max_bytes": max([r["peak_rss_bytes"] for r in good], default=None)})
    import xarray as xr
    import zarr
    import fsspec
    import numcodecs
    try:
        system_gdal = subprocess.check_output(["gdal-config", "--version"], text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        system_gdal = None
    try:
        memory_bytes = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES")
    except (AttributeError, ValueError, OSError):
        memory_bytes = None
    fixture_hashes = {
        "cog": {"source.tif": hashlib.sha256(fixture.read_bytes()).hexdigest()},
        "zarr": {route: hashlib.sha256(path.read_bytes()).hexdigest()
                 for route, path in sorted(zarr_objects.items())},
    }
    result = {"environment": {"platform": platform.platform(), "python": sys.version,
                "rasterio": rasterio.__version__, "rasterio_gdal": rasterio.__gdal_version__,
                "system_gdal_config": system_gdal,
                "xarray": xr.__version__, "zarr": zarr.__version__, "numcodecs": numcodecs.__version__,
                "fsspec": fsspec.__version__,
                "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
                "cpu_count": os.cpu_count(), "memory_bytes": memory_bytes,
                "source_sha256": source_hash(),
                "fixture_file_hashes": fixture_hashes,
                "baseline_sha256": hashlib.sha256(BASELINE.read_bytes()).hexdigest(),
                "current_sha256": hashlib.sha256(CURRENT.read_bytes()).hexdigest(),
                "cargo_lock_sha256": hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest(),
                "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()},
              "invocation": sys.argv,
              "request_sha256": request_hashes,
              "scope": "loopback only; synthetic Sentinel-like COG and synthetic Zarr; source files are served from the OS page cache after fixture creation; shared bandwidth and injected latency; no public-provider speed claim",
              "runs": runs, "summary": summary}
    (work / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
