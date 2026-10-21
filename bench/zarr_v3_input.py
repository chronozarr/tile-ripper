"""Independent Zarr-Python fixtures, controlled range transport and v3 input gates.
Run with the reference environment recorded in docs/zarr-v3-input/reference.txt.
All format writing/reading and shard codecs use Zarr-Python, not Tile-Ripper.
"""

from __future__ import annotations
import argparse
from collections import Counter
import copy
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import itertools
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import threading
import time
import zlib

import numpy as np
import xarray as xr
import zarr
from zarr.codecs import (
    BytesCodec,
    GzipCodec,
    BloscCodec,
    ZstdCodec,
    ShardingCodec,
    Crc32cCodec,
)

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/zarr-v3-input"
DOCS = ROOT / "docs/zarr-v3-input"
BINARY = ROOT / "target/zarr-v3-input/release/examples/zarr_subset_oracle"
CLI = ROOT / "target/zarr-v3-input/release/tile-ripper"


def scheduler_summary(value):
    """Keep v3 reservations; omit irrelevant per-task COG diagnostic expansions.

    Complete execution receipts remain in ignored per-cell raw logs.
    """
    return {
        k: v
        for k, v in value.items()
        if k not in {"native_strategies", "native_metadata", "native_buffers"}
    }


def codec(name, typesize):
    return (
        []
        if name == "raw"
        else [GzipCodec(level=1)]
        if name == "gzip"
        else [ZstdCodec(level=1, checksum=True)]
        if name == "zstd"
        else [
            BloscCodec(
                cname=name.removeprefix("blosc-")
                .removesuffix("-bitshuffle")
                .removesuffix("-noshuffle"),
                clevel=3,
                shuffle="bitshuffle"
                if name.endswith("-bitshuffle")
                else "noshuffle"
                if name.endswith("-noshuffle")
                else "shuffle",
                typesize=typesize,
                blocksize=0,
            )
        ]
    )


def fixture(
    path,
    sharded=True,
    compression="gzip",
    swap=False,
    reverse=False,
    shape=(5, 12, 16),
    inner=(2, 3, 4),
    factor=(2, 2, 2),
    index_location="end",
    consolidate=False,
    key_encoding=None,
    index_crc=True,
):
    shutil.rmtree(path, ignore_errors=True)
    g = zarr.open_group(str(path), mode="w", zarr_format=3)
    g.attrs.update(
        {
            "title": "Independent v3 native scientific fixture",
            "source": "Zarr-Python",
            "institution": "validation",
        }
    )
    nt, ny, nx = shape
    x = np.arange(nx, dtype="<f8") * 10 + 5
    y = np.arange(ny, dtype="<f8") * -10 + ny * 10 - 5
    if reverse:
        x = x[::-1]
        y = y[::-1]
    t = np.arange(nt, dtype="<i8") * 3600_000_000_000 + 123456789
    for name, data in [("x", x), ("y", y), ("time", t)]:
        a = g.create_array(
            name,
            data=data,
            chunks=(len(data),),
            compressors=codec(compression, 8),
            dimension_names=(name,),
            fill_value=0,
        )
        if name == "time":
            a.attrs.update(
                {
                    "units": "nanoseconds since 2025-01-01 00:00:00",
                    "calendar": "proleptic_gregorian",
                }
            )
        else:
            a.attrs.update({"units": "m", "axis": name.upper()})
    mapping = g.create_array(
        "spatial_ref",
        shape=(),
        dtype="<i8",
        compressors=[],
        dimension_names=(),
        fill_value=0,
    )
    mapping.attrs.update(
        {"epsg_code": "EPSG:32617", "grid_mapping_name": "transverse_mercator"}
    )
    base = np.arange(nt * ny * nx).reshape(shape)
    arrays = {
        "signal": (base % 65535).astype("<u2"),
        "temperature": (base.astype("<f4") / 7 - 90).astype("<f4"),
        "mask": (base % 5 != 0).astype("<u2"),
    }
    # Different NaN payloads, signed zero, and CF sentinel test native bit preservation.
    bits = arrays["temperature"].view("<u4")
    bits.ravel()[::41] = 0x7FC01234
    bits.ravel()[23::73] = 0xFFC05678
    arrays["temperature"].ravel()[15] = -0.0
    arrays["signal"][: inner[0], : inner[1], : inner[2]] = (
        0  # omitted inner chunk: zero remains valid
    )
    arrays["temperature"][: inner[0], : inner[1], : inner[2]] = np.nan
    dims = ("time", "x", "y") if swap else ("time", "y", "x")
    chunks = (inner[0], inner[2], inner[1]) if swap else inner
    shards = tuple(
        c * f
        for c, f in zip(chunks, (factor[0], factor[2], factor[1]) if swap else factor)
    )
    for name, data in arrays.items():
        if swap:
            data = data.transpose(0, 2, 1)
        compressors = codec(compression, data.dtype.itemsize)
        if sharded:
            serializer = ShardingCodec(
                chunk_shape=chunks,
                codecs=[BytesCodec(endian="little"), *compressors],
                index_codecs=[
                    BytesCodec(endian="little"),
                    *([Crc32cCodec()] if index_crc else []),
                ],
                index_location=index_location,
            )
            a = g.create_array(
                name,
                data=data,
                chunks=shards,
                serializer=serializer,
                compressors=[],
                dimension_names=dims,
                fill_value="NaN" if name == "temperature" else 0,
                **({"chunk_key_encoding": key_encoding} if key_encoding else {}),
            )
        else:
            a = g.create_array(
                name,
                data=data,
                chunks=chunks,
                compressors=compressors,
                dimension_names=dims,
                fill_value="NaN" if name == "temperature" else 0,
                **({"chunk_key_encoding": key_encoding} if key_encoding else {}),
            )
        a.attrs.update(
            {
                "grid_mapping": "spatial_ref",
                "coordinates": "time x y spatial_ref",
                "long_name": name,
                "units": "K" if name == "temperature" else "1",
            }
        )
        if name == "temperature":
            a.attrs["_FillValue"] = "AAAAAAAA+H8="
        if name == "signal":
            a.attrs.update({"scale_factor": 0.01, "add_offset": -1.0})
    if consolidate:
        zarr.consolidate_metadata(str(path))
    return path


def request(source, output, shape=(5, 12, 16), dense=False):
    nt, ny, nx = shape
    return {
        "source": {
            "kind": "assets",
            "assets": [
                {
                    "id": "science",
                    "locator": str(source),
                    "format": "zarr_v3",
                    "datetime": None,
                }
            ],
        },
        "spatial": {
            "kind": "bounds",
            "crs": "EPSG:32617",
            "bbox": [0, 0, nx * 10, ny * 10]
            if dense
            else [10, 10, min(nx * 10 - 10, 65), min(ny * 10 - 10, 75)],
        },
        "time": {
            "kind": "instants",
            "values": [
                str(
                    np.datetime64("2025-01-01T00:00:00.123456789", "ns")
                    + np.timedelta64(int(i * 3600), "s")
                )
                + "Z"
                for i in sorted(set([0, nt - 2, nt - 1]))
            ],
        },
        "variables": [
            {"kind": "dataset", "path": v} for v in ["signal", "temperature", "mask"]
        ],
        "quality": [],
        "grid": {"kind": "native"},
        "operations": [],
        "sink": {"kind": "directory", "path": str(output)},
        "budget": {
            "buffer_bytes": 64 * 1024 * 1024,
            "max_nodes": 100000,
            "chunk_edge": 4,
        },
    }


def selection(r, group):
    lo, la, hi, ha = r["spatial"]["bbox"]
    axes = {}
    for name, lower, upper in [("x", lo, hi), ("y", la, ha)]:
        v = group[name][:]
        e = np.r_[
            v[0] - (v[1] - v[0]) / 2, (v[1:] + v[:-1]) / 2, v[-1] + (v[-1] - v[-2]) / 2
        ]
        axes[name] = np.flatnonzero(
            (np.maximum(e[1:], e[:-1]) > lower) & (np.minimum(e[1:], e[:-1]) < upper)
        )
    times = group["time"][:] + np.datetime64("2025-01-01", "ns").astype("int64")
    chosen = np.array(
        [
            np.datetime64(v.removesuffix("Z"), "ns").astype("int64")
            for v in r["time"]["values"]
        ]
    )
    axes["time"] = np.flatnonzero(np.isin(times, chosen))
    return axes


class Server(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, root, latency=0, bandwidth=0):
        super().__init__(("127.0.0.1", 0), Handler)
        self.root = Path(root)
        self.latency = latency
        self.bandwidth = bandwidth
        self.records = []
        self.lock = threading.Lock()
        self.fault = {}
        self.control_key = None
        self.next_byte = 0.0
        self.cache = {
            p.relative_to(root).as_posix(): p.read_bytes()
            for p in Path(root).rglob("*")
            if p.is_file()
        }

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server_port}/store/"

    def start(self):
        threading.Thread(target=self.serve_forever, daemon=True).start()
        return self


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self):
        super().setup()
        self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def log_message(self, *_):
        pass

    def do_HEAD(self):
        self.serve(True)

    def do_GET(self):
        self.serve(False)

    def serve(self, head):
        s = self.server
        if self.path.startswith("/control/"):
            key = s.control_key
            if key:
                b = bytearray(s.cache[key])
                b[0] ^= 1
                s.cache[key] = bytes(b)
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        key = self.path.removeprefix("/store/")
        data = s.cache.get(key)
        status = 404 if data is None else 200
        etag = None if data is None else '"' + hashlib.sha256(data).hexdigest() + '"'
        if self.headers.get("If-Match") and self.headers["If-Match"] != etag:
            status = 412
        if self.headers.get("If-None-Match") == "*" and data is not None:
            status = 412
        span = None
        rang = self.headers.get("Range")
        if status == 200 and rang:
            a, b = rang.removeprefix("bytes=").split("-")
            start = int(a) if a else len(data) - int(b)
            end = int(b) + 1 if a and b else len(data)
            span = (start, end)
            status = 206
        with s.lock:
            actions = s.fault.get((self.command, key), [])
            action = actions.pop(0) if actions else None
        if isinstance(action, int):
            status = action
        if action == "ignore-range":
            status = 200
            span = None
        body = (
            b""
            if status not in (200, 206)
            else data
            if span is None
            else data[slice(*span)]
        )
        if action == "corrupt" and body:
            damaged = bytearray(body)
            damaged[: min(32, len(body))] = bytes(min(32, len(body)))
            body = bytes(damaged)
        time.sleep(s.latency)
        self.send_response(status)
        if etag:
            self.send_header("ETag", ("W/" if action == "weak" else "") + etag)
        self.send_header("Content-Length", str(len(body)))
        if span:
            self.send_header(
                "Content-Range",
                f"bytes {span[0]}-{span[1] - 1}/{len(data) + (1 if action == 'bad-range' else 0)}",
            )
        if action == "encoded":
            self.send_header("Content-Encoding", "gzip")
        if status == 429:
            self.send_header("Retry-After", "0")
        self.end_headers()
        written = 0
        if not head:
            limit = len(body) // 2 if action == "interrupt" else len(body)
            try:
                for start in range(0, limit, 65536):
                    part = body[start : min(start + 65536, limit)]
                    if s.bandwidth:
                        with s.lock:
                            now = time.perf_counter()
                            s.next_byte = (
                                max(s.next_byte, now) + len(part) / s.bandwidth
                            )
                            delay = s.next_byte - now
                        time.sleep(delay)
                    self.wfile.write(part)
                    self.wfile.flush()
                    written += len(part)
            except (BrokenPipeError, ConnectionResetError):
                pass
            if action == "interrupt":
                self.close_connection = True
                self.connection.shutdown(socket.SHUT_RDWR)
        with s.lock:
            s.records.append(
                {
                    "method": self.command,
                    "key": key,
                    "range": rang,
                    "span": span,
                    "status": status,
                    "bytes": written,
                    "if_match": self.headers.get("If-Match"),
                    "time": time.perf_counter(),
                }
            )


def run(r, path, mode="stream", dump=None, env=None, check=True):
    path.write_text(json.dumps(r))
    cmd = [str(BINARY), str(path), mode] + ([str(dump)] if dump else [])
    p = subprocess.run(
        cmd,
        env={
            **os.environ,
            "TILE_RIPPER_WORKERS": "2",
            "TILE_RIPPER_PER_HOST": "2",
            **(env or {}),
        },
        capture_output=True,
        text=True,
    )
    if check and p.returncode:
        raise RuntimeError(p.stderr[-5000:])
    return (
        json.loads(p.stdout)
        if p.returncode == 0
        else {"error": p.stderr, "code": p.returncode}
    )


def verify(r, source, result, dump=None):
    g = zarr.open_group(str(source), mode="r", use_consolidated=False)
    idx = selection(r, g)
    seen = set()
    valid = 0
    for i, c in enumerate(result["runs"][0]["chunks"]):
        name = c["variable"]["path"]
        a = g[name]
        dims = a.metadata.dimension_names
        w = c["window"]
        ti = c["indices"][0]
        pos = {
            "time": [ti],
            "x": np.arange(w["x"], w["x"] + w["width"]),
            "y": np.arange(w["y"], w["y"] + w["height"]),
        }
        v = a.oindex[tuple(pos[d] for d in dims)]
        v = np.ascontiguousarray(
            v.transpose(tuple(dims.index(d) for d in ["time", "y", "x"]))[0]
        )
        raw = v.astype(v.dtype.newbyteorder("<"), copy=False).tobytes()
        mask = (~np.isnan(v)).astype("u1")
        marker = a.attrs.get("_FillValue")
        if isinstance(marker, (int, float)):
            mask &= (v != np.array(marker, dtype=v.dtype)).astype("u1")
        assert zlib.adler32(raw) == c["adler32"], (name, i, "native bits")
        assert zlib.adler32(mask.tobytes()) == c["mask_adler32"], (name, i, "mask")
        np.testing.assert_array_equal(
            c["coordinates"][0], g["x"][w["x"] : w["x"] + w["width"]]
        )
        np.testing.assert_array_equal(
            c["coordinates"][1], g["y"][w["y"] : w["y"] + w["height"]]
        )
        assert c["crs"] == "EPSG:32617"
        assert np.datetime64(
            c["datetime"].replace("+00:00", ""), "ns"
        ) == np.datetime64("2025-01-01", "ns") + np.timedelta64(
            int(g["time"][ti]), "ns"
        )
        if dump:
            assert (dump / f"{i}.bin").read_bytes() == raw and (
                dump / f"{i}.mask"
            ).read_bytes() == mask.tobytes()
        for y in pos["y"]:
            for x in pos["x"]:
                key = (name, ti, int(y), int(x))
                assert key not in seen
                seen.add(key)
        valid += int(mask.sum())
    expected = {
        (v["path"], int(t), int(y), int(x))
        for v in r["variables"]
        for t, y, x in itertools.product(idx["time"], idx["y"], idx["x"])
    }
    assert seen == expected, (len(seen), len(expected))
    assert result["runs"][0]["valid_pixels"] == valid
    return {
        "samples": len(seen),
        "valid": valid,
        "chunks": len(result["runs"][0]["chunks"]),
    }


def checks():
    records = []
    for sharded, compression, swap, reverse in itertools.product(
        [False, True],
        ["raw", "gzip", "zstd", "blosc-lz4", "blosc-zstd"],
        [False, True],
        [False, True],
    ):
        label = f"{'sharded' if sharded else 'ordinary'}-{compression}-{swap}-{reverse}"
        cell = WORK / "checks" / label
        cell.mkdir(parents=True, exist_ok=True)
        source = fixture(
            cell / "source",
            sharded,
            compression,
            swap,
            reverse,
            index_location="start" if swap else "end",
            consolidate=reverse,
        )
        s = Server(source).start()
        try:
            r = request(s.url, cell / "out")
            result = run(r, cell / "request.json", mode="dump", dump=cell / "chunks")
            values = verify(r, source, result, cell / "chunks")
            (cell / "result.json").write_text(json.dumps(result))
            (cell / "http.json").write_text(json.dumps(s.records))
            reads = [
                v
                for v in s.records
                if v["method"] == "GET"
                and "/c/" in v["key"]
                and v["key"].split("/")[0] in ["signal", "temperature", "mask"]
            ]
            assert all(v["range"] and v["status"] == 206 for v in reads)
            # Distinct ranges (index + selected payload) are each transferred once.
            duplicates = sum(
                n - 1 for n in Counter((v["key"], v["range"]) for v in reads).values()
            )
            assert duplicates == 0, (label, duplicates)
            records.append(
                {
                    "profile": label,
                    **values,
                    "data_ranges": len(reads),
                    "data_bytes": sum(v["bytes"] for v in reads),
                    "duplicate_ranges": duplicates,
                }
            )
            print(label, values, flush=True)
        finally:
            s.shutdown()
            s.server_close()
    DOCS.mkdir(exist_ok=True, parents=True)
    (DOCS / "correctness.json").write_text(json.dumps(records, indent=2))


def faults():
    import google_crc32c
    import gzip

    records = []
    base = WORK / "faults"
    base.mkdir(parents=True, exist_ok=True)
    source = fixture(base / "source", True, "gzip")
    r = request("", base / "out")
    probe = Server(source).start()
    try:
        r["source"]["assets"][0]["locator"] = probe.url
        run(r, base / "baseline.json")
        data_keys = sorted(
            {
                v["key"]
                for v in probe.records
                if v["method"] == "GET" and v["key"].startswith("signal/c/")
            }
        )
        key = data_keys[0]
    finally:
        probe.shutdown()
        probe.server_close()

    def cell(label, edit=None, action=None, success=False, mode="stream", control=None):
        server = Server(source).start()
        if edit:
            edit(server.cache, key)
        if action:
            server.fault[("GET", key)] = list(action)
        if control:
            server.control_key = control if control != "data" else key
        rr = copy.deepcopy(r)
        rr["source"]["assets"][0]["locator"] = server.url
        result = run(
            rr,
            base / (label + ".json"),
            mode=mode,
            check=False,
            env={
                "TILE_RIPPER_TEST_CONTROL": server.url.replace(
                    "/store/", "/control/mutate"
                )
            },
        )
        server.shutdown()
        server.server_close()
        assert ("error" not in result) == success, (
            label,
            result.get("error", "unexpected success"),
        )
        if success:
            verify(rr, source, result)
        records.append(
            {
                "case": label,
                "passed": True,
                "expected_success": success,
                "error": result.get("error"),
                "get_attempts": sum(
                    v["method"] == "GET" and v["key"] == key for v in server.records
                ),
                "body_bytes": sum(v["bytes"] for v in server.records),
            }
        )
        (base / (label + "-http.json")).write_text(json.dumps(server.records))
        print(label, "PASS", flush=True)

    for label, actions, ok in [
        ("retry-503", [503, None], True),
        ("retry-429", [429, None], True),
        ("interrupted-retry", ["interrupt", None], True),
        ("retry-exhausted", [503, 503, 503], False),
        ("range-ignored", ["ignore-range"], False),
        ("bad-content-range", ["bad-range"], False),
        ("weak-etag", ["weak"], False),
        ("encoded-range", ["encoded"], False),
        ("payload-range-ignored", [None, "ignore-range"], False),
        ("payload-interrupted-retry", [None, "interrupt", None], True),
        ("payload-retry-exhausted", [None] + [503] * 64, False),
    ]:
        cell(label, action=actions, success=ok)
    cell("cancel-after-delivery", mode="cancel")
    for target in ["data", "zarr.json", "x/c/0"]:
        cell("mutate-before-" + target.replace("/", "_"), mode="before", control=target)
        cell("mutate-during-" + target.replace("/", "_"), mode="during", control=target)
    doc = json.loads((source / "signal/zarr.json").read_text())
    config = doc["codecs"][0]["configuration"]
    num = int(
        np.prod(
            np.array(doc["chunk_grid"]["configuration"]["chunk_shape"])
            // config["chunk_shape"]
        )
    )
    index_bytes = 16 * num + 4

    def index_edit(change, crc=True):
        def edit(cache, key):
            data = cache[key]
            raw = bytearray(data[-index_bytes:])
            table = np.frombuffer(raw[:-4], dtype="<u8").copy().reshape(-1, 2)
            change(table)
            encoded = table.tobytes()
            cache[key] = (
                data[:-index_bytes]
                + encoded
                + int(google_crc32c.value(encoded) if crc else 0).to_bytes(4, "little")
            )

        return edit

    for label, fn in [
        ("offset-overflow", lambda t: t.__setitem__(0, [2**64 - 2, 32])),
        ("offset-outside", lambda t: t.__setitem__(0, [2**63, 32])),
        (
            "overlap-index",
            lambda t: t.__setitem__(
                0, [len((source / key).read_bytes()) - index_bytes, 1]
            ),
        ),
        ("mixed-missing", lambda t: t.__setitem__(0, [2**64 - 1, 0])),
        ("zero-length", lambda t: t.__setitem__(0, [0, 0])),
        ("overlap-payload", lambda t: t.__setitem__(slice(0, 2), [[0, 10], [1, 10]])),
    ]:
        cell(label, edit=index_edit(fn))
    cell("bad-index-crc", edit=index_edit(lambda t: None, False))
    cell(
        "truncated-index",
        edit=lambda cache, key: cache.__setitem__(key, cache[key][: index_bytes - 1]),
    )

    def bomb(cache, key):
        data = cache[key]
        table = np.frombuffer(data[-index_bytes:-4], dtype="<u8").copy().reshape(-1, 2)
        payload = bytearray(data[:-index_bytes])
        b = gzip.compress(bytes(16 * 1024 * 1024), compresslevel=1)
        for row in table:
            if row[0] != 2**64 - 1:
                row[:] = [len(payload), len(b)]
                payload.extend(b)
        raw = table.tobytes()
        cache[key] = (
            bytes(payload) + raw + int(google_crc32c.value(raw)).to_bytes(4, "little")
        )

    cell("gzip-expansion-limit", edit=bomb)
    (DOCS / "faults.json").write_text(json.dumps(records, indent=2))


def edges():
    import google_crc32c

    work = WORK / "edges"
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir(parents=True, exist_ok=True)
    rows = []
    for n, (inner, factor, encoding) in enumerate(
        [
            (
                (1, 2, 5),
                (1, 3, 2),
                {"name": "default", "configuration": {"separator": "."}},
            ),
            ((3, 5, 7), (2, 1, 3), {"name": "v2", "configuration": {"separator": "."}}),
            ((2, 3, 4), (2, 2, 2), None),
        ]
    ):
        cell = work / str(n)
        cell.mkdir(exist_ok=True)
        source = fixture(
            cell / "source",
            True,
            "zstd",
            shape=(7, 13, 19),
            inner=inner,
            factor=factor,
            key_encoding=encoding,
        )
        g = zarr.open_group(str(source), mode="a")
        g["signal"][2, 8, 4] = 65535
        g["signal"].attrs["_FillValue"] = 65535
        g["temperature"][2, 8, 4] = -9999
        g["temperature"].attrs["_FillValue"] = -9999
        if n == 2:
            # Whole absent shard and a retained shard containing only missing-inner
            # entries: the independent reader defines the expected native fill.
            target = source / "mask/c/0/0/0"
            target.unlink()
            target = source / "signal/c/0/0/0"
            m = json.loads((source / "signal/zarr.json").read_text())
            c = m["codecs"][0]["configuration"]
            count = int(
                np.prod(
                    np.array(m["chunk_grid"]["configuration"]["chunk_shape"])
                    // c["chunk_shape"]
                )
            )
            raw = np.full((count, 2), 2**64 - 1, dtype="<u8").tobytes()
            data = target.read_bytes()
            target.write_bytes(
                data[: -len(raw) - 4]
                + raw
                + int(google_crc32c.value(raw)).to_bytes(4, "little")
            )
        server = Server(source).start()
        try:
            r = request(server.url, cell / "out", shape=(7, 13, 19), dense=True)
            r["time"]["values"] = [
                "2025-01-01T02:00:00.123456789Z",
                "2025-01-01T06:00:00.123456789Z",
            ]
            result = run(r, cell / "request.json")
            values = verify(r, source, result)
            run(r, cell / "request.json", "fetch")
            got = zarr.open_group(str(cell / "out" / "science.zarr"), mode="r")
            idx = selection(r, g)
            for name in ["signal", "temperature", "mask"]:
                expected = g[name].oindex[
                    tuple(idx[d] for d in g[name].metadata.dimension_names)
                ]
                actual = got[name][:]
                assert actual.tobytes() == expected.tobytes(), (
                    name,
                    "materialized raw bits",
                )
            np.testing.assert_array_equal(got["time"][:], g["time"][idx["time"]])
            assert got["signal"].metadata.fill_value == 65535
            assert got["temperature"].metadata.fill_value == -9999
            decoded = xr.open_zarr(
                str(cell / "out" / "science.zarr"), consolidated=False, chunks=None
            )
            assert decoded.time.size == 2
            assert "spatial_ref" in decoded
            assert decoded["signal"].isnull().sum() == 1 if n != 2 else True
            rows.append(
                {
                    "inner": inner,
                    "factor": factor,
                    "key_encoding": encoding,
                    "fill_0_valid_CF_markers_distinct": True,
                    "native_stream_and_materialization": True,
                    **values,
                }
            )
        finally:
            server.shutdown()
            server.server_close()
    (DOCS / "edges.json").write_text(json.dumps(rows, indent=2))


def extras():
    work = WORK / "extras"
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir(parents=True)
    rows = []
    profiles = [
        (sharded, codec, True)
        for sharded in [False, True]
        for codec in [
            "blosc-lz4-bitshuffle",
            "blosc-zstd-bitshuffle",
            "blosc-zstd-noshuffle",
        ]
    ] + [(True, "raw", False)]
    for n, (sharded, compression, crc) in enumerate(profiles):
        cell = work / str(n)
        cell.mkdir()
        source = fixture(cell / "source", sharded, compression, index_crc=crc)
        server = Server(source).start()
        try:
            r = request(server.url, cell / "unused", dense=True)
            result = run(r, cell / "request.json")
            rows.append(
                {
                    "sharded": sharded,
                    "codec": compression,
                    "index_crc": crc,
                    **verify(r, source, result),
                }
            )
        finally:
            server.shutdown()
            server.server_close()
    # A layout requiring full-shard decoding must be rejected at preparation.
    source = fixture(work / "unsupported", True, "raw")
    meta = source / "signal/zarr.json"
    original = json.loads(meta.read_text())
    for label, edit in [
        (
            "outer-gzip",
            lambda m: m["codecs"].append(
                {"name": "gzip", "configuration": {"level": 1}}
            ),
        ),
        (
            "non-time-first",
            lambda m: m.__setitem__("dimension_names", ["x", "time", "y"]),
        ),
    ]:
        m = copy.deepcopy(original)
        edit(m)
        meta.write_text(json.dumps(m))
        r = request(source, work / "unused")
        result = run(r, work / (label + ".json"), check=False)
        assert "prepare:" in result.get("error", "")
        rows.append(
            {"rejected": label, "before_delivery": True, "error": result["error"]}
        )
    (DOCS / "extras.json").write_text(json.dumps(rows, indent=2))


def nan_bits():
    work = WORK / "nan-bits"
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir(parents=True)
    rows = []
    for compression in ["raw", "gzip"]:
        cell = work / compression
        cell.mkdir()
        source = fixture(cell / "source", False, compression)
        a = zarr.open_array(str(source / "temperature"), mode="r+").with_config(
            {"write_empty_chunks": True}
        )
        values = np.full((2, 3, 4), np.nan, dtype="<f4")
        values.view("<u4")[:] = 0x7FC0BEEF
        a[:2, :3, :4] = values
        assert int(a[:2, :3, :4].view("<u4")[0, 0, 0]) == 0x7FC0BEEF
        r = request(source, cell / "out", dense=True)
        result = run(r, cell / "request.json")
        checked = verify(r, source, result)
        run(r, cell / "request.json", "fetch")
        got = zarr.open_group(str(cell / "out" / "science.zarr"), mode="r")
        expected = a.oindex[
            tuple(
                selection(r, zarr.open_group(str(source)))[d]
                for d in a.metadata.dimension_names
            )
        ]
        assert expected.tobytes() == got["temperature"][:].tobytes()
        rows.append(
            {
                "codec": compression,
                "all_NaN_chunk_payload_bits": "0x7fc0beef",
                "stream_and_materialization_bits_exact": True,
                **checked,
            }
        )
    (DOCS / "nan-bits.json").write_text(json.dumps(rows, indent=2))


def memory():
    import re

    rows = []
    work = WORK / "memory"
    work.mkdir(parents=True, exist_ok=True)
    for sharded, compression, nt in itertools.product(
        [False, True], ["raw", "gzip", "zstd", "blosc-zstd"], [4, 256]
    ):
        label = f"{sharded}-{compression}-{nt}"
        cell = work / label
        cell.mkdir(exist_ok=True)
        shape = (nt, 128, 128)
        source = fixture(
            cell / "source",
            sharded,
            compression,
            shape=shape,
            inner=(4, 128, 128),
            factor=(2, 2, 2),
        )
        r = request(source, cell / "unused", shape=shape, dense=True)
        r["budget"]["chunk_edge"] = 64
        r["time"] = {
            "kind": "interval",
            "start": "2025-01-01T00:00:00Z",
            "end": str(
                np.datetime64("2025-01-01", "s")
                + np.timedelta64((nt - 1) * 3600 + 1, "s")
            )
            + "Z",
        }
        file = cell / "request.json"
        file.write_text(json.dumps(r))
        command = [
            "/usr/bin/time",
            "-l",
            str(BINARY.parent / "chunk_stream"),
            str(file),
        ]
        p = subprocess.run(
            command,
            env={**os.environ, "TILE_RIPPER_WORKERS": "2", "TILE_RIPPER_PER_HOST": "2"},
            capture_output=True,
            text=True,
        )
        assert p.returncode == 0, p.stderr[-3000:]
        result = json.loads(p.stdout)
        rss = int(re.search(r"(\d+)\s+maximum resident set size", p.stderr).group(1))
        observed = result["observed"]
        assert observed["decoded_bytes"] == nt * 128 * 128 * 8
        assert (
            observed["scheduler"]["reserved_bytes_peak"] <= r["budget"]["buffer_bytes"]
        )
        assert rss < 192 * 1024 * 1024
        rows.append(
            {
                "sharded": sharded,
                "codec": compression,
                "time_count": nt,
                "logical_output_bytes": observed["decoded_bytes"],
                "peak_rss_bytes": rss,
                "scheduler": scheduler_summary(observed["scheduler"]),
            }
        )
        print(label, rss, flush=True)
        # Only the compact receipts are retained; all source bytes are reproducible.
        shutil.rmtree(source)
    for small, big in zip(rows[::2], rows[1::2]):
        assert big["peak_rss_bytes"] - small["peak_rss_bytes"] < 64 * 1024 * 1024
    (DOCS / "memory.json").write_text(json.dumps(rows, indent=2))


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument(
        "action", choices=["checks", "faults", "edges", "memory", "nan_bits", "extras"]
    )
    a = p.parse_args()
    WORK.mkdir(parents=True, exist_ok=True)
    globals()[a.action]()
