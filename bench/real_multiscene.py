"""Bounded real Sentinel COG range capture and frozen local replay (no paid services).
See docs/production-streaming/protocol.md. Source bytes stay in ignored prototype-output.
"""

from __future__ import annotations
import argparse
from concurrent.futures import ThreadPoolExecutor
import copy
import hashlib
import http.server
import json
import math
import os
from pathlib import Path
import platform
import random
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
import zlib

import numpy as np
import rasterio
from rasterio.windows import Window
import requests

import acquisition_engine as engine

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/production-streaming/real"
CONSUMER = ROOT / "target/release/tile-ripper-native-stats"
CLI = ROOT / "target/release/tile-ripper"
PAGE = 256 * 1024
GDAL_ENV = {
    **engine.GDAL_ENV,
    "GDAL_CACHEMAX": "32",
    "GDAL_HTTP_MULTIRANGE": "SERIAL",
    "GDAL_NUM_THREADS": "1",
}


class Mirror:
    """Version-pinned sparse *benchmark fixture*, never a production data cache.
    Frozen replay fails closed on any uncaptured range. Original offsets/layout
    and bytes are preserved; no reencoding or synthetic imagery is substituted.
    """

    def __init__(self, root, sources, live=False, max_bytes=192 * 2**20):
        self.root = root
        root.mkdir(parents=True, exist_ok=True)
        self.sources = {f"/{i}.tif": a for i, a in enumerate(sources)}
        self.live = live
        self.max_bytes = max_bytes
        self.lock = threading.Lock()
        self.events = []
        self.provider_bytes = 0
        self.pages = {k: set() for k in self.sources}
        self.index = root / "index.json"
        if self.index.exists():
            saved = json.loads(self.index.read_text())
            assert saved["sources"] == sources or (
                live and sources[: len(saved["sources"])] == saved["sources"]
            ), "source manifest differs from captured mirror"
            self.pages.update({k: set(v) for k, v in saved["pages"].items()})
            self.events = saved["events"]
            self.provider_bytes = sum(e["bytes"] for e in self.events)
        for key, a in self.sources.items():
            p = self.path(key)
            if not p.exists():
                with p.open("wb") as f:
                    f.truncate(a["bytes"])

    def path(self, key):
        return self.root / key.lstrip("/")

    def read(self, key, start, end):
        a = self.sources[key]
        needed = set(range(start // PAGE, end // PAGE + 1))
        with self.lock:
            missing = needed - self.pages[key]
            if missing and not self.live:
                raise ValueError("uncaptured range in frozen mirror")
            while missing:
                first = min(missing)
                last = first
                while last + 1 in missing and last - first < 15:
                    last += 1
                lo = first * PAGE
                hi = min(a["bytes"] - 1, (last + 1) * PAGE - 1)
                if self.provider_bytes + hi - lo + 1 > self.max_bytes:
                    raise ValueError("live byte budget exceeded")
                if len(self.events) >= 256:
                    raise ValueError("live range-request cap exceeded")
                t = time.monotonic()
                # One upstream request at a time; no unbounded retries or prefetch.
                r = requests.get(
                    a["href"],
                    headers={"Range": f"bytes={lo}-{hi}", "If-Match": a["etag"]},
                    timeout=45,
                )
                r.raise_for_status()
                assert r.status_code == 206 and r.headers.get("ETag") == a["etag"]
                assert r.headers["Content-Range"] == f"bytes {lo}-{hi}/{a['bytes']}"
                assert len(r.content) == hi - lo + 1
                with self.path(key).open("r+b") as f:
                    f.seek(lo)
                    f.write(r.content)
                self.provider_bytes += len(r.content)
                self.events.append(
                    dict(
                        source=key,
                        start=lo,
                        end=hi,
                        bytes=len(r.content),
                        seconds=time.monotonic() - t,
                        sha256=hashlib.sha256(r.content).hexdigest(),
                    )
                )
                self.pages[key].update(range(first, last + 1))
                missing = needed - self.pages[key]
                self.save()
        with self.path(key).open("rb") as f:
            f.seek(start)
            data = f.read(end - start + 1)
        assert len(data) == end - start + 1
        return data

    def save(self):
        self.index.write_text(
            json.dumps(
                dict(
                    sources=list(self.sources.values()),
                    pages={k: sorted(v) for k, v in self.pages.items()},
                    events=self.events,
                ),
                indent=2,
            )
        )

    def verify_pages(self):
        # Verify each immutable captured response against its receipt, then a
        # portable manifest digest over captured page bytes and logical offsets.
        for e in self.events:
            with self.path(e["source"]).open("rb") as f:
                f.seek(e["start"])
                b = f.read(e["bytes"])
            assert hashlib.sha256(b).hexdigest() == e["sha256"]
        return hashlib.sha256(self.index.read_bytes()).hexdigest()


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def reset(self):
        with self.lock:
            self.counts = dict(
                head=0,
                get=0,
                range_get=0,
                body_bytes=0,
                conditional=0,
                errors=0,
                head_active=0,
                head_peak=0,
            )
            self.records = []

    def handle_error(self, *_):
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

    def serve(self, body):
        server = self.server
        key = urllib.parse.urlsplit(self.path).path
        a = server.mirror.sources.get(key)
        if a is None:
            self.send_error(404)
            return
        start, end = 0, a["bytes"] - 1
        rng = self.headers.get("Range")
        if rng:
            try:
                left, right = rng.removeprefix("bytes=").split("-")
                start = int(left)
                end = min(int(right) if right else end, end)
                assert 0 <= start <= end
            except (ValueError, AssertionError):
                self.send_error(416)
                return
        t = time.perf_counter()
        with server.lock:
            server.counts["get" if body else "head"] += 1
            server.counts["conditional"] += int(bool(self.headers.get("If-Match")))
            if rng and body:
                server.counts["range_get"] += 1
            if not body:
                server.counts["head_active"] += 1
                server.counts["head_peak"] = max(
                    server.counts["head_peak"], server.counts["head_active"]
                )
        if server.latency:
            time.sleep(server.latency)
        if self.headers.get("If-Match") not in (None, a["etag"]):
            self.send_error(412)
            return
        try:
            data = server.mirror.read(key, start, end) if body else None
        except Exception as error:
            with server.lock:
                server.counts["errors"] += 1
                server.records.append(
                    dict(error=str(error), source=key, start=start, end=end)
                )
            self.send_error(500)
            return
        self.send_response(206 if rng else 200)
        self.send_header("Content-Length", str(end - start + 1))
        self.send_header("ETag", a["etag"])
        self.send_header("Accept-Ranges", "bytes")
        if rng:
            self.send_header("Content-Range", f"bytes {start}-{end}/{a['bytes']}")
        self.end_headers()
        if body:
            for off in range(0, len(data), 65536):
                block = data[off : off + 65536]
                server.link.transfer(len(block))
                self.wfile.write(block)
                with server.lock:
                    server.counts["body_bytes"] += len(block)
            self.wfile.flush()
        with server.lock:
            if not body:
                server.counts["head_active"] -= 1
            server.records.append(
                dict(
                    method="GET" if body else "HEAD",
                    source=key,
                    start=start,
                    end=end,
                    seconds=time.perf_counter() - t,
                    started=t,
                )
            )


def start(mirror, latency=0, mbps=0):
    server = Server(("127.0.0.1", 0), Handler)
    server.mirror = mirror
    server.latency = latency / 1000
    server.link = engine.SharedLink(mbps * 1e6 / 8 if mbps else None)
    server.lock = threading.Lock()
    server.reset()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def request(sources, server, window):
    x, y, w, h = window
    return dict(
        source=dict(
            kind="assets",
            assets=[
                dict(
                    id=a["id"],
                    locator=f"http://127.0.0.1:{server.server_port}/{i}.tif",
                    format="cog",
                    datetime=a["datetime"],
                    etag=a["etag"],
                )
                for i, a in enumerate(sources)
            ],
        ),
        spatial=dict(
            kind="bounds",
            crs="EPSG:32617",
            bbox=[
                600000 + x * 10,
                4000020 - (y + h) * 10,
                600000 + (x + w) * 10,
                4000020 - y * 10,
            ],
        ),
        variables=[dict(kind="band", index=1)],
        grid=dict(kind="native"),
        sink=dict(kind="callback"),
        budget=dict(buffer_bytes=16 * 2**20, max_nodes=100000, chunk_edge=256),
    )


def gdal_stats(path, workers):
    req = json.loads(path.read_text())
    bbox = req["spatial"]["bbox"]
    started = time.perf_counter()
    first = []
    lock = threading.Lock()

    def acquire(asset):
        rows = []
        opened = time.perf_counter()
        read_s = 0.0
        with rasterio.Env(**{**GDAL_ENV, "GDAL_CACHEMAX": 32 * 2**20}):
            with rasterio.open(asset["locator"]) as ds:
                metadata = time.perf_counter() - opened
                c0, r0 = (~ds.transform) * (bbox[0], bbox[3])
                c1, r1 = (~ds.transform) * (bbox[2], bbox[1])
                for y in range(
                    max(0, math.floor(r0)), min(ds.height, math.ceil(r1)), 256
                ):
                    for x in range(
                        max(0, math.floor(c0)), min(ds.width, math.ceil(c1)), 256
                    ):
                        w = Window(
                            x,
                            y,
                            min(256, math.ceil(c1) - x),
                            min(256, math.ceil(r1) - y),
                        )
                        t = time.perf_counter()
                        a = ds.read(1, window=w)
                        read_s += time.perf_counter() - t
                        with lock:
                            if not first:
                                first.append(time.perf_counter() - started)
                        keep = (
                            a != ds.nodata
                            if ds.nodata is not None
                            else np.ones(a.shape, dtype=bool)
                        )
                        rows.append(
                            dict(
                                type="provisional_chunk",
                                asset_id=asset["id"],
                                variable=dict(kind="band", index=1),
                                datetime=asset["datetime"],
                                indices=[],
                                window=dict(
                                    x=x, y=y, width=int(w.width), height=int(w.height)
                                ),
                                transform=list(ds.window_transform(w).to_gdal()),
                                crs=str(ds.crs),
                                dtype="uint16",
                                nodata=ds.nodata,
                                scale=ds.scales[0],
                                offset=ds.offsets[0],
                                coordinates=None,
                                adler32_le=zlib.adler32(
                                    a.astype("<u2", copy=False).tobytes()
                                ),
                                valid_pixels=int(keep.sum()),
                                native_sum=float(a[keep].sum(dtype=np.float64)),
                            )
                        )
        return rows, metadata, read_s

    with ThreadPoolExecutor(max_workers=workers) as pool:
        batches = list(pool.map(acquire, req["source"]["assets"]))
    chunks = [r for batch, _, _ in batches for r in batch]
    return dict(
        rows=[
            dict(
                type="provisional_start",
                provenance={"sources": req["source"]["assets"]},
            )
        ]
        + chunks
        + [
            dict(
                type="complete",
                chunks=len(chunks),
                valid_pixels=sum(c["valid_pixels"] for c in chunks),
                native_sum=sum(c["native_sum"] for c in chunks),
            )
        ],
        seconds=time.perf_counter() - started,
        first_chunk_seconds=first[0] if first else None,
        metadata_worker_seconds=sum(b[1] for b in batches),
        read_worker_seconds=sum(b[2] for b in batches),
        source_validation="reader only; frozen mirror supplies immutable bytes",
    )


def run(cmd, out, env=None, ndjson=False):
    out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        t = time.perf_counter()
        p = subprocess.Popen(
            cmd,
            stdout=stdout,
            stderr=stderr,
            env={**os.environ, **GDAL_ENV, **(env or {})},
        )
        timer = threading.Timer(180, p.kill)
        timer.start()
        try:
            _, status, usage = os.wait4(p.pid, 0)
        finally:
            timer.cancel()
        p.returncode = os.waitstatus_to_exitcode(status)
        wall = time.perf_counter() - t
        stdout.seek(0)
        text = stdout.read().decode()
        stderr.seek(0)
        err = stderr.read().decode()
    out.write_text(text)
    try:
        result = (
            [json.loads(line) for line in text.splitlines()]
            if ndjson
            else json.loads(text)
        )
    except ValueError:
        result = None
    return dict(
        exit_code=p.returncode,
        wall_seconds=wall,
        peak_rss_bytes=usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024),
        result=result,
        error=err[-2000:],
    )


def check_file(out, req, refs):
    manifest = json.loads((out / "manifest.json").read_text())
    records = manifest["outputs"]
    coverage = {a["id"]: set() for a in req["source"]["assets"]}
    values = 0
    bbox = req["spatial"]["bbox"]
    for rec in records:
        aid = rec["asset_id"]
        path = out / rec["file"]
        with rasterio.open(refs[aid]["local"]) as src, rasterio.open(path) as got:
            x, y = (~src.transform) * (got.transform.c, got.transform.f)
            x, y = round(x), round(y)
            w = Window(x, y, got.width, got.height)
            np.testing.assert_array_equal(got.read(1), src.read(1, window=w))
            assert (
                got.transform == src.window_transform(w)
                and got.crs == src.crs
                and got.nodata == src.nodata
            )
            assert got.scales == src.scales and got.offsets == src.offsets
            assert rec["timestamps"] == [refs[aid]["datetime"]]
            assert (x, y) not in coverage[aid]
            coverage[aid].add((x, y))
            values += got.width * got.height
    assert len(list(out.glob("*.tif"))) == len(records)
    for aid, actual in coverage.items():
        with rasterio.open(refs[aid]["local"]) as src:
            c0, r0 = (~src.transform) * (bbox[0], bbox[3])
            c1, r1 = (~src.transform) * (bbox[2], bbox[1])
            expected = {
                (x, y)
                for y in range(math.floor(r0), math.ceil(r1), 256)
                for x in range(math.floor(c0), math.ceil(c1), 256)
            }
            assert actual == expected
    return dict(files=len(records), values=values)


def measure(mode, req, folder, server, refs, workers=4):
    from consumer_validation import verify

    folder.mkdir(parents=True, exist_ok=True)
    path = folder / "request.json"
    path.write_text(json.dumps(req))
    out = folder / "out"
    server.reset()
    if mode == "rust-stream":
        cmd = [str(CONSUMER), str(path)]
        nd = True
    elif mode == "gdal-stream":
        cmd = [sys.executable, __file__, "worker", str(path), str(workers)]
        nd = False
    elif mode == "rust-file":
        cmd = [str(CLI), "fetch", str(path), "--output", str(out)]
        nd = False
    else:
        cmd = [
            sys.executable,
            str(ROOT / "bench/acquisition_engine.py"),
            "--gdal-worker",
            str(path),
            str(out),
            str(workers),
        ]
        nd = False
    trace = folder / "stages.jsonl"
    row = run(
        cmd,
        folder / "stdout.json",
        env={"TILE_RIPPER_STAGE_TRACE": str(trace)},
        ndjson=nd,
    )
    row["stages"] = (
        [json.loads(line) for line in trace.read_text().splitlines()]
        if trace.exists()
        else []
    )
    stage_totals = {}
    for stage in row["stages"]:
        stage_totals[stage["stage"]] = (
            stage_totals.get(stage["stage"], 0) + stage["seconds"]
        )
    row["cog_identity_validation_seconds"] = stage_totals.get("cog_identity_batch")
    row["layout_inspection_seconds"] = stage_totals.get("cog_layout_inspection")
    row["planning_compute_seconds"] = (
        max(
            0,
            stage_totals.get("native_planning_with_layout", 0)
            - stage_totals.get("cog_layout_inspection", 0),
        )
        if "native_planning_with_layout" in stage_totals
        else None
    )
    row.update(
        mode=mode,
        workers=workers,
        http=copy.deepcopy(server.counts),
        request_records=copy.deepcopy(server.records),
    )
    assert row["exit_code"] == 0, row
    assert not row["http"]["errors"], row
    result = row["result"]
    t = time.perf_counter()
    if mode.endswith("stream"):
        rows = result if nd else result["rows"]
        checked = verify(rows, refs)
        final = rows[-1]
        # Independent complete coverage for each requested asset and each native
        # grid window; hash checks alone cannot detect omitted or duplicate chunks.
        for a in req["source"]["assets"]:
            aid = a["id"]
            rects = checked["coverage"][
                (aid, json.dumps({"kind": "band", "index": 1}, sort_keys=True), ())
            ]
            with rasterio.open(refs[aid]["local"]) as src:
                c0, r0 = (~src.transform) * (
                    req["spatial"]["bbox"][0],
                    req["spatial"]["bbox"][3],
                )
                c1, r1 = (~src.transform) * (
                    req["spatial"]["bbox"][2],
                    req["spatial"]["bbox"][1],
                )
                expected = {
                    (x, y, min(256, math.ceil(c1) - x), min(256, math.ceil(r1) - y))
                    for y in range(math.floor(r0), math.ceil(r1), 256)
                    for x in range(math.floor(c0), math.ceil(c1), 256)
                }
                assert set(rects) == expected
        row["first_chunk_seconds"] = (
            final.get("first_chunk_seconds") if nd else result["first_chunk_seconds"]
        )
        row["prepare_seconds"] = final.get("prepare_seconds") if nd else None
        row["metadata_worker_seconds"] = (
            result.get("metadata_worker_seconds") if not nd else None
        )
        row["observed"] = final.get("observed")
        row["correctness"] = {k: v for k, v in checked.items() if k != "coverage"}
        row["materialization_seconds"] = 0.0
    else:
        row["correctness"] = check_file(out, req, refs)
        obs = result.get("observed", {})
        row["observed"] = obs
        row["prepare_seconds"] = obs.get("planning_seconds")
        row["first_chunk_seconds"] = None
        row["first_materialized_seconds"] = obs.get(
            "first_output_seconds", result.get("usable_ttfc_seconds")
        )
        row["materialization_seconds"] = obs.get(
            "sink_seconds", result.get("write_seconds", 0)
        ) + obs.get("output_finalize_seconds", 0)
        shutil.rmtree(out)
    row["oracle_seconds_excluded"] = time.perf_counter() - t
    row.pop("result")
    row["scientific_correctness_passed"] = True
    row["rss_gate_passed"] = (
        mode != "rust-stream" or row["peak_rss_bytes"] <= 192 * 2**20
    )
    row["head_gate_passed"] = mode != "rust-stream" or row["http"]["head_peak"] <= 4
    row["passed"] = row["rss_gate_passed"] and row["head_gate_passed"]
    if not row["passed"]:
        row["error"] = "native consumer RSS or HEAD concurrency gate exceeded"
    return row


def discover(out):
    frozen = json.loads(
        (ROOT / "study/experiments/results/scenes_17SPV_2025.json").read_text()
    )["scenes"][:8]
    sources = []
    discovery = []
    session = requests.Session()
    params = {
        "collections": "sentinel-2-c1-l2a",
        "ids": ",".join(s["id"] for s in frozen),
        "limit": len(frozen) + 1,
    }
    started = time.perf_counter()
    response = session.get(
        "https://earth-search.aws.element84.com/v1/search", params=params, timeout=30
    )
    response.raise_for_status()
    collection = response.json()
    assert {v["id"] for v in collection["features"]} == {s["id"] for s in frozen}
    count = 1
    size = len(response.content)
    (out / "stac-search.json").write_bytes(response.content)
    # Earth Search may emit a next link even for a complete frozen-ID result.
    links = [link for link in collection.get("links", []) if link.get("rel") == "next"]
    if links:
        assert links[0]["href"].startswith(
            "https://earth-search.aws.element84.com/v1/search?"
        )
        terminal = session.get(links[0]["href"], timeout=30)
        terminal.raise_for_status()
        assert not terminal.json()["features"]
        (out / "stac-search-terminal.json").write_bytes(terminal.content)
        count += 1
        size += len(terminal.content)
    search = dict(
        params=params,
        seconds=time.perf_counter() - started,
        requests=count,
        bytes=size,
        items=len(frozen),
    )
    for scene in frozen:
        url = f"https://earth-search.aws.element84.com/v1/collections/sentinel-2-c1-l2a/items/{scene['id']}"
        t = time.perf_counter()
        r = session.get(url, timeout=30)
        r.raise_for_status()
        item = r.json()
        elapsed = time.perf_counter() - t
        discovery.append(
            dict(
                item=scene["id"],
                seconds=elapsed,
                bytes=len(r.content),
                sha256=hashlib.sha256(r.content).hexdigest(),
            )
        )
        (out / f"{scene['id']}.json").write_bytes(r.content)
        for band, key in [("B04", "red"), ("B08", "nir")]:
            href = item["assets"][key]["href"]
            t = time.perf_counter()
            h = session.head(href, timeout=30)
            h.raise_for_status()
            assert not h.headers["ETag"].startswith("W/")
            sources.append(
                dict(
                    id=f"{scene['id']}-{band}",
                    item_id=scene["id"],
                    datetime=item["properties"]["datetime"],
                    band=band,
                    href=href,
                    etag=h.headers["ETag"],
                    bytes=int(h.headers["Content-Length"]),
                    head_seconds=time.perf_counter() - t,
                )
            )
    manifest = dict(
        sources=sources,
        discovery=discovery,
        frozen_id_search=search,
        collection="sentinel-2-c1-l2a",
        license="Copernicus Sentinel data; source collection metadata retained separately",
        selection="first eight items of the existing frozen 17SPV/2025 study list; two native 10 m bands, no cloud filtering added",
        captured_utc=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    )
    (out / "sources.json").write_text(json.dumps(manifest, indent=2))
    return manifest


def main():
    global CLI, CONSUMER
    if len(sys.argv) > 1 and sys.argv[1] == "worker":
        print(json.dumps(gdal_stats(Path(sys.argv[2]), int(sys.argv[3]))))
        return
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("action", choices=["capture", "benchmark"])
    ap.add_argument("--output", type=Path, default=WORK)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--latency-ms", type=float, default=0)
    ap.add_argument("--bandwidth-mbps", type=float, default=0)
    ap.add_argument("--label", default="serial")
    ap.add_argument("--binary-dir", type=Path)
    ap.add_argument("--max-provider-mib", type=int, default=192)
    ap.add_argument("--workloads", nargs="+")
    args = ap.parse_args()
    work = args.output.resolve()
    work.mkdir(parents=True, exist_ok=True)
    if args.binary_dir:
        CLI = args.binary_dir.resolve() / "tile-ripper"
        CONSUMER = args.binary_dir.resolve() / "tile-ripper-native-stats"
    source_digest = (
        json.loads((args.binary_dir / "receipt.json").read_text())["source_sha256"]
        if args.binary_dir
        else engine.source_hash()
    )
    manifest = (
        json.loads((work / "sources.json").read_text())
        if (work / "sources.json").exists()
        else discover(work)
    )
    sources = manifest["sources"]
    mirror = Mirror(
        work / "mirror",
        sources,
        live=args.action == "capture",
        max_bytes=args.max_provider_mib * 2**20,
    )
    server = start(mirror, args.latency_ms, args.bandwidth_mbps)
    # Same native tile and full-resolution grid on three genuine acquisition dates.
    windows = {
        "many-small": (7300, 2100, 96, 96),
        "several-large": (6144, 1024, 2048, 2048),
        "overlap-a": (6868, 1748, 768, 768),
        "overlap-b": (7124, 2004, 768, 768),
    }
    refs = {
        a["id"]: {**a, "format": "cog", "local": mirror.path(f"/{i}.tif")}
        for i, a in enumerate(sources)
    }
    try:
        if args.action == "capture":
            # Prime both access patterns, including each overlap's rounded tile
            # ranges. Only selected native ranges are fetched from the provider.
            for name, win in windows.items():
                req = request(
                    sources if name == "many-small" else sources[:6], server, win
                )
                for mode in ["rust-stream", "gdal-stream"]:
                    row = measure(
                        mode, req, work / "capture" / f"{name}-{mode}", server, refs
                    )
                    print(
                        name,
                        mode,
                        round(row["wall_seconds"], 3),
                        "provider MiB",
                        round(mirror.provider_bytes / 2**20, 2),
                        flush=True,
                    )
            mirror.live = False
            digest = mirror.verify_pages()
            mirror.save()
            (work / "capture-receipt.json").write_text(
                json.dumps(
                    dict(
                        mirror_index_sha256=digest,
                        provider_requests=len(mirror.events),
                        provider_bytes=mirror.provider_bytes,
                    ),
                    indent=2,
                )
            )
            return
        mirror_hash = mirror.verify_pages()
        dest = work / f"{args.label}-{args.latency_ms:g}ms-{args.bandwidth_mbps:g}mbit"
        dest.mkdir(exist_ok=False)
        rows = []
        rng = random.Random(20261019)
        pilots = []
        for name, win in windows.items():
            if args.workloads and name not in args.workloads:
                continue
            req = request(sources if name == "many-small" else sources[:6], server, win)
            # Actual concurrency pilot uses the same workload and scientific oracle.
            tuned = {}
            for mode in ["gdal-stream", "gdal-file"]:
                trials = []
                for n in [1, 2, 4, 8]:
                    row = measure(
                        mode, req, dest / f"pilot-{name}-{mode}-{n}", server, refs, n
                    )
                    row.update(workload=name, pilot=True)
                    pilots.append(row)
                    trials.append(row)
                tuned[mode] = min(trials, key=lambda r: r["wall_seconds"])["workers"]
            for rep in range(args.reps):
                modes = ["rust-stream", "gdal-stream", "rust-file", "gdal-file"]
                rng.shuffle(modes)
                for mode in modes:
                    folder = dest / f"{name}-{rep}-{mode}"
                    try:
                        row = measure(
                            mode, req, folder, server, refs, tuned.get(mode, 4)
                        )
                    except Exception as error:
                        row = dict(
                            mode=mode,
                            passed=False,
                            error=str(error),
                            http=copy.deepcopy(server.counts),
                        )
                    row.update(workload=name, rep=rep, pilot=False)
                    rows.append(row)
                    with (dest / "runs.jsonl").open("a") as f:
                        f.write(json.dumps(row) + "\n")
                    print(
                        name,
                        rep,
                        mode,
                        "passed" if row["passed"] else "FAILED",
                        round(row.get("wall_seconds", 0), 3),
                        flush=True,
                    )
        summary = []
        for name in windows:
            for mode in ["rust-stream", "gdal-stream", "rust-file", "gdal-file"]:
                group = [r for r in rows if r["workload"] == name and r["mode"] == mode]
                ok = [r for r in group if r["passed"]]
                if not group:
                    continue
                result = dict(
                    workload=name,
                    mode=mode,
                    runs=len(group),
                    failures=len(group) - len(ok),
                )
                for key in [
                    "wall_seconds",
                    "first_chunk_seconds",
                    "prepare_seconds",
                    "materialization_seconds",
                    "peak_rss_bytes",
                    "cog_identity_validation_seconds",
                    "layout_inspection_seconds",
                    "planning_compute_seconds",
                ]:
                    vals = [r[key] for r in ok if r.get(key) is not None]
                    result[key] = (
                        dict(
                            median=statistics.median(vals), min=min(vals), max=max(vals)
                        )
                        if vals
                        else None
                    )
                result["http_medians"] = (
                    {
                        key: statistics.median(r["http"][key] for r in ok)
                        for key in ["head", "get", "body_bytes"]
                    }
                    if ok
                    else None
                )
                summary.append(result)
        report = dict(
            summary=summary,
            pilots=pilots,
            mirror_index_sha256=mirror_hash,
            source_manifest=manifest,
            link=dict(latency_ms=args.latency_ms, bandwidth_mbps=args.bandwidth_mbps),
            binaries={
                str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in [CLI, CONSUMER]
            },
            source_sha256=source_digest,
            artifact_sha256={
                str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in [
                    ROOT / "Cargo.lock",
                    ROOT / "consumers/native-stats/Cargo.lock",
                    Path(__file__),
                    ROOT / "bench/consumer_validation.py",
                    ROOT / "bench/acquisition_engine.py",
                ]
            },
            rss_limit_bytes=192 * 2**20,
            environment=dict(
                platform=platform.platform(),
                python=sys.version,
                rasterio=rasterio.__version__,
                python_gdal=rasterio.__gdal_version__,
                rust_gdal=subprocess.check_output(
                    ["gdal-config", "--version"], text=True
                ).strip(),
                overrides=GDAL_ENV,
            ),
            scopes=[
                "real original COG range bytes, local warm-page-cache replay; shaped link is not public-provider throughput",
                "discovery is bounded frozen-ID STAC search plus item metadata lookups, shared by both readers and excluded from replay timings",
                "Rust pins and validates versions before/after execution; GDAL reader control uses immutable fixture without those validation costs",
                "first chunk is process-internal elapsed: Rust includes prepare, GDAL begins after Python imports; wall includes fresh-process startup",
                "materialization is sum of overlapping worker sink durations, not additive wall time; native stream has no file sink",
                "raw rows, sparse imagery and logs remain under ignored prototype-output; no production persistent cache",
            ],
        )
        (dest / "report.json").write_text(json.dumps(report, indent=2))
        print("report", dest / "report.json", flush=True)
        assert all(r["passed"] for r in rows), "retain failures and investigate"
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
