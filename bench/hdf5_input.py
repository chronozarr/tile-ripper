"""Independent h5py/netCDF4 fixtures and controlled HTTP range/science gates.

uv run --no-project --python 3.12 --with h5py --with netCDF4 --with requests \
  --with psutil python bench/hdf5_input.py science
Public access is opt-in and bounded to one 1.27 MB NOAA object.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import re
import resource
import socket
import struct
import subprocess
import sys
import threading
import time
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import h5py
import netCDF4
import numpy as np
import psutil
import requests

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/native-hdf5"
DOCS = ROOT / "docs/native-hdf5"
BINARY = Path(
    os.environ.get(
        "TILE_RIPPER_HDF5_BINARY", ROOT / "target/release/examples/hdf5_subset_oracle"
    )
)
URL = "https://noaa-goes19.s3.amazonaws.com/ABI-L2-RRQPEF/2025/100/12/OR_ABI-L2-RRQPEF-M6_G19_s20251001200211_e20251001209519_c20251001210009.nc"


def save(name, value):
    DOCS.mkdir(parents=True, exist_ok=True)
    (DOCS / f"{name}.json").write_text(
        json.dumps(value, indent=2, allow_nan=False, default=str) + "\n"
    )


def fixture(
    name,
    dtype="<u2",
    compression="gzip",
    shuffle=True,
    fletcher=False,
    latest=False,
    shape=(257, 385),
    chunks=(31, 47),
    group="science",
    holes=True,
):
    path = WORK / f"{name}.nc"
    path.parent.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(462)
    values = (
        rng.integers(0, 50000, size=shape, dtype=np.uint16)
        if dtype.endswith("u2")
        else rng.normal(8, 3, size=shape).astype(np.float32)
    ).astype(dtype)
    fill = 65535 if dtype.endswith("u2") else -9999.0
    values[1, 1] = fill
    if dtype.endswith("f4"):
        special = np.array(
            [0x7FC01234, 0x80000000, 0x7F800000, 0xFF800000], dtype="<u4"
        ).view("<f4")
        values[2, 2:6] = special
    with h5py.File(path, "w", libver="latest" if latest else "earliest") as f:
        g = f.require_group(group) if group else f
        for axis, n in [("y", shape[0]), ("x", shape[1])]:
            coords = (
                np.arange(n, dtype="f8") * 0.25 + np.sin(np.arange(n)) * 0.001
            ).astype(">f8")
            d = g.create_dataset(axis, data=coords)
            d.make_scale(axis)
            d.attrs["units"] = np.bytes_("degree")
            d.attrs["long_name"] = np.bytes_(f"irregular {axis} centers")
        ds = g.create_dataset(
            "signal",
            shape=shape,
            dtype=dtype,
            chunks=chunks,
            compression=compression,
            shuffle=shuffle if chunks else False,
            fletcher32=fletcher if chunks else False,
            fillvalue=fill,
        )
        ds.attrs["_FillValue"] = np.array([fill], dtype=dtype)
        ds.attrs["scale_factor"] = np.float64(0.01)
        ds.attrs["add_offset"] = np.float64(-2)
        ds.attrs["units"] = np.bytes_("m")
        ds.attrs["long_name"] = np.bytes_("independent scientific signal")
        ds.dims[0].attach_scale(g["y"])
        ds.dims[1].attach_scale(g["x"])
        if holes and chunks:
            for row in range(0, shape[0], chunks[0]):
                for col in range(0, shape[1], chunks[1]):
                    if row == chunks[0] and col == chunks[1]:
                        continue
                    ds[row : row + chunks[0], col : col + chunks[1]] = values[
                        row : row + chunks[0], col : col + chunks[1]
                    ]
        else:
            ds[:] = values
    return path, "/" + group + "/signal" if group else "/signal"


def payload_ranges(path, dataset):
    with h5py.File(path, "r") as f:
        ds = f[dataset]
        return (
            [
                (int(c.byte_offset), int(c.size))
                for i in range(ds.id.get_num_chunks())
                if (c := ds.id.get_chunk_info(i)).byte_offset is not None
            ]
            if ds.chunks
            else []
        )


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_HEAD(self):
        self.serve(True)

    def do_GET(self):
        self.serve(False)

    def serve(self, head):
        srv = self.server
        if self.path.startswith("/control"):
            if self.path == "/control/wait-payload":
                srv.payload_started.wait(10)
            else:
                srv.changed = True
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        raw = srv.raw
        etag = '"changed"' if srv.changed else srv.etag
        start, end = 0, len(raw)
        requested = self.headers.get("Range")
        if requested:
            start, last = map(int, requested.removeprefix("bytes=").split("-"))
            end = last + 1
        payload = bool(
            requested
            and any(
                start <= offset and offset + size <= end for offset, size in srv.payload
            )
        )
        if payload:
            srv.payload_started.set()
        fault = srv.fault if (payload or srv.fault_scope == "all") else None
        status = 200 if head else 206
        if requested and (start < 0 or start >= end or end > len(raw)):
            status = 416
        if self.headers.get("If-Match") not in [None, etag]:
            status = 412
        if fault in ["retry", "exhaust", "interrupt"]:
            with srv.lock:
                srv.fault_count += 1
                attempt = srv.fault_count
            if fault == "exhaust" or (fault == "retry" and attempt <= 2):
                status = 503
        if fault == "no-range" and not head:
            status = 200
        if fault == "changed-etag":
            etag = '"other"'
        if srv.latency:
            time.sleep(srv.latency)
        body = raw[start:end] if not head and status == 206 else b""
        if fault == "raw-bitflip" and body:
            body = bytes([body[0] ^ 1]) + body[1:]
        self.send_response(status)
        if fault != "no-etag":
            self.send_header("ETag", "W/" + etag if fault == "weak-etag" else etag)
        self.send_header("Content-Length", str(len(raw) if head else len(body)))
        if requested and status == 206:
            self.send_header(
                "Content-Range",
                f"bytes {start + (1 if fault == 'bad-range' else 0)}-{end - 1}/{len(raw)}",
            )
        if fault == "encoded":
            self.send_header("Content-Encoding", "gzip")
        self.end_headers()
        if fault == "interrupt" and body:
            body = body[: len(body) // 2]
            self.close_connection = True
        try:
            if body:
                self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass
        with srv.lock:
            srv.events.append(
                {
                    "method": self.command,
                    "start": start if requested else None,
                    "end": end if requested else None,
                    "bytes": len(body),
                    "status": status,
                    "payload": payload,
                    "conditional": self.headers.get("If-Match") is not None,
                }
            )


class Server(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, path, dataset, latency=0, fault=None, fault_scope="payload"):
        super().__init__(("127.0.0.1", 0), Handler)
        self.raw = path.read_bytes()
        self.etag = '"' + hashlib.sha256(self.raw).hexdigest() + '"'
        self.payload = (
            payload_ranges(path, dataset) if path.stat().st_size > 256 else []
        )
        self.events = []
        self.lock = threading.Lock()
        self.fault = fault
        self.fault_count = 0
        self.fault_scope = fault_scope
        self.changed = False
        self.latency = latency
        self.payload_started = threading.Event()

    def get_request(self):
        connection, address = super().get_request()
        connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        return connection, address

    def handle_error(self, request, client_address):
        # Rejected/aborted streams deliberately close keep-alive connections.
        if not isinstance(sys.exception(), (BrokenPipeError, ConnectionResetError)):
            super().handle_error(request, client_address)

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server_port}/data.nc"

    def reset(self):
        self.events.clear()
        self.fault_count = 0
        self.changed = False
        self.payload_started.clear()

    def stats(self):
        events = list(self.events)
        get = [v for v in events if v["method"] == "GET"]
        keys = [(v["start"], v["end"]) for v in get if v["status"] == 206]
        return {
            "gets": len(get),
            "heads": len(events) - len(get),
            "response_bytes": sum(v["bytes"] for v in events),
            "payload_gets": sum(v["payload"] for v in get),
            "all_ranges_conditional": all(v["conditional"] for v in get),
            "repeated_exact_ranges": sum(n - 1 for n in Counter(keys).values()),
            "events": events,
        }


@contextlib.contextmanager
def server(*args, **kw):
    s = Server(*args, **kw)
    t = threading.Thread(target=s.serve_forever, daemon=True)
    t.start()
    try:
        yield s
    finally:
        s.shutdown()
        s.server_close()
        t.join()


def request(
    locator, dataset, start=(0, 0), count=(64, 96), step=(1, 1), budget=32 * 1024 * 1024
):
    parent = dataset.rsplit("/", 1)[0]
    return {
        "source": {
            "kind": "assets",
            "assets": [
                {
                    "id": "science",
                    "locator": str(locator),
                    "format": "hdf5",
                    "datetime": None,
                }
            ],
        },
        "spatial": {
            "kind": "array_slice",
            "start": list(start),
            "count": list(count),
            "step": list(step),
            "axes": [parent + "/y", parent + "/x"],
        },
        "time": None,
        "variables": [{"kind": "dataset", "path": dataset}],
        "grid": {"kind": "native"},
        "sink": {"kind": "callback"},
        "budget": {"buffer_bytes": budget, "max_nodes": 4096, "chunk_edge": 32},
    }


def run_native(req, name, mode="dump", policy="individual", control=None, workers=2):
    out = WORK / "runs" / name
    out.mkdir(parents=True, exist_ok=True)
    path = out / "request.json"
    path.write_text(json.dumps(req))
    env = {
        **os.environ,
        "TILE_RIPPER_WORKERS": str(workers),
        "TILE_RIPPER_PER_HOST": str(workers),
        "TILE_RIPPER_HDF5_RANGE_POLICY": policy,
    }
    if control:
        env["TILE_RIPPER_TEST_CONTROL"] = control
    before_cpu = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.perf_counter()
    stdout_file = (out / "stdout.json").open("wb")
    stderr_file = (out / "stderr.txt").open("wb")
    p = subprocess.Popen(
        [
            "/usr/bin/time",
            "-l" if sys.platform == "darwin" else "-v",
            str(BINARY),
            str(path),
            mode,
            str(out),
        ],
        stdout=stdout_file,
        stderr=stderr_file,
        env=env,
    )
    process = psutil.Process(p.pid)
    peak = 0
    cpu = 0
    while p.poll() is None:
        if time.perf_counter() - start > 120:
            p.kill()
            raise TimeoutError(name)
        try:
            peak = max(peak, process.memory_info().rss)
            c = process.cpu_times()
            cpu = max(cpu, c.user + c.system)
        except psutil.NoSuchProcess:
            pass
        time.sleep(0.005)
    p.wait()
    stdout_file.close()
    stderr_file.close()
    stdout = (out / "stdout.json").read_bytes()
    stderr = (out / "stderr.txt").read_bytes()
    seconds = time.perf_counter() - start
    rss_match = (
        re.search(rb"(\d+)\s+maximum resident set size", stderr)
        if sys.platform == "darwin"
        else re.search(rb"Maximum resident set size \(kbytes\): (\d+)", stderr)
    )
    measured_rss = (
        int(rss_match.group(1)) * (1 if sys.platform == "darwin" else 1024)
        if rss_match
        else peak
    )
    after_cpu = resource.getrusage(resource.RUSAGE_CHILDREN)
    cpu = (
        after_cpu.ru_utime
        + after_cpu.ru_stime
        - before_cpu.ru_utime
        - before_cpu.ru_stime
    )
    result = {
        "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
        "returncode": p.returncode,
        "elapsed_seconds": seconds,
        "peak_rss_bytes": measured_rss,
        "sampled_peak_rss_bytes": peak,
        "child_cpu_seconds": cpu,
        "error": stderr.decode()[-2000:] if p.returncode else None,
        "output_dir": str(out),
    }
    if p.returncode == 0:
        result["result"] = json.loads(stdout)
    return result


def verify_attributes(actual, dataset):
    assert set(actual) == set(dataset.attrs), (
        dataset.name,
        "attribute identity differs",
    )
    for name, record in actual.items():
        if "value" not in record:
            continue
        expected = np.asarray(dataset.attrs[name])
        value = expected.item() if expected.size == 1 else expected.tolist()
        if isinstance(value, bytes):
            value = value.decode().rstrip("\0")
        assert record["value"] == value, (dataset.name, name, record["value"], value)


def validate(result, path, req, netcdf=True):
    assert result["returncode"] == 0, result["error"]
    out = Path(result["output_dir"])
    run = result["result"]["runs"][0]
    total = 0
    seen = set()
    with h5py.File(path, "r") as f:
        ds = f[req["variables"][0]["path"]]
        binding = result["result"]["inspection"]["sources"][0]["object_identities"][
            "datasets"
        ][0]
        assert binding["shape"] == list(ds.shape)
        verify_attributes(binding["attributes"], ds)
        for i, axis in enumerate(binding["axes"]):
            assert axis["path"] == req["spatial"]["axes"][i]
            verify_attributes(axis["attributes"], f[axis["path"]])
            if len(ds.dims[i]):
                assert ds.dims[i][0].name == axis["path"]
        if binding["grid_mapping"] is not None:
            mapping = binding["grid_mapping"]
            verify_attributes(mapping["attributes"], f[mapping["path"]])
        if binding["time"] is not None:
            t = binding["time"]
            assert t["stored_value"] == f[t["path"]][()].item()
            verify_attributes(t["attributes"], f[t["path"]])
        dtype = ds.dtype.newbyteorder("<").str
        for i, c in enumerate(run["chunks"]):
            w = c["window"]
            ys = slice(
                w["y"],
                w["y"] + w["height"] * req["spatial"]["step"][0],
                req["spatial"]["step"][0],
            )
            xs = slice(
                w["x"],
                w["x"] + w["width"] * req["spatial"]["step"][1],
                req["spatial"]["step"][1],
            )
            expected = ds[ys, xs].astype(dtype)
            actual = (out / f"{i}.bin").read_bytes()
            assert actual == expected.tobytes(), (
                i,
                c["window"],
                "native stored value bits differ",
            )
            mask = ~np.isnan(expected)
            for marker in ["_FillValue", "missing_value"]:
                if marker in ds.attrs:
                    mask &= expected != np.asarray(ds.attrs[marker]).item()
            assert (out / f"{i}.mask").read_bytes() == mask.astype("u1").tobytes()
            axes = req["spatial"]["axes"]
            y = f[axes[0]][ys].astype("f8")
            x = f[axes[1]][xs].astype("f8")
            assert np.array_equal(c["coordinates"][0], x) and np.array_equal(
                c["coordinates"][1], y
            )
            assert c["variable"] == req["variables"][0]
            for row in range(
                w["y"],
                min(w["y"] + w["height"] * req["spatial"]["step"][0], ds.shape[0]),
                req["spatial"]["step"][0],
            ):
                for col in range(
                    w["x"],
                    min(w["x"] + w["width"] * req["spatial"]["step"][1], ds.shape[1]),
                    req["spatial"]["step"][1],
                ):
                    assert (row, col) not in seen
                    seen.add((row, col))
            total += expected.size
        assert total == int(np.prod(req["spatial"]["count"]))
        assert run["observed"]["decoded_bytes"] == total * np.dtype(dtype).itemsize
    if netcdf:
        with netCDF4.Dataset(path) as f:
            ds = f[req["variables"][0]["path"]]
            ds.set_auto_maskandscale(False)
            start = req["spatial"]["start"]
            count = req["spatial"]["count"]
            step = req["spatial"]["step"]
            sel = tuple(slice(a, a + n * s, s) for a, n, s in zip(start, count, step))
            reference = ds[sel]
            assert reference.size == total
            # Compare every streamed piece through independently executed NetCDF-C too.
            for i, c in enumerate(run["chunks"]):
                w = c["window"]
                ys = slice(w["y"], w["y"] + w["height"] * step[0], step[0])
                xs = slice(w["x"], w["x"] + w["width"] * step[1], step[1])
                assert (out / f"{i}.bin").read_bytes() == ds[ys, xs].astype(
                    dtype
                ).tobytes()
    report = run["observed"]
    assert report["scheduler"]["reserved_bytes_peak"] <= req["budget"]["buffer_bytes"]
    return {
        "all_stored_value_bits_equal": True,
        "all_interpreted_metadata_equal": True,
        "all_coordinates_equal": True,
        "all_validity_bits_equal": True,
        "netcdf4_equal": netcdf,
        "values": total,
        "chunks": len(run["chunks"]),
        "output_bytes": report["decoded_bytes"],
        "scheduler": report["scheduler"],
        "report_payload_gets": report["source_data_requests"],
        "report_payload_bytes": report["source_data_bytes"],
    }


def compact(result):
    return {k: v for k, v in result.items() if k not in ["result", "output_dir"]}


def science():
    cells = []
    for dtype in ["<u2", ">u2", "<f4", ">f4", "<i2", ">i2"]:
        for filters in [
            ("raw", None, False, False),
            ("gzip", "gzip", False, False),
            ("shuffle-gzip", "gzip", True, False),
            ("checksum", "gzip", True, True),
        ]:
            name, compression, shuffle, fletcher = filters
            path, ds = fixture(
                dtype.replace("<", "le").replace(">", "be") + "-" + name,
                dtype,
                compression,
                shuffle,
                fletcher,
            )
            for selection in [
                ((29, 43), (67, 103), (1, 1)),
                ((0, 0), (86, 77), (3, 5)),
                ((31, 47), (31, 47), (1, 1)),
                ((250, 378), (7, 7), (1, 1)),
            ]:
                label = f"{path.stem}-{selection[0]}-{selection[2]}"
                with server(path, ds) as s:
                    req = request(s.url, ds, *selection)
                    result = run_native(req, label, policy="bounded")
                    checks = validate(result, path, req)
                    stats = s.stats()
                    assert stats["all_ranges_conditional"]
                    assert stats["repeated_exact_ranges"] == 0
                    cells.append(
                        {
                            "name": label,
                            **compact(result),
                            **checks,
                            "http": {k: v for k, v in stats.items() if k != "events"},
                        }
                    )
                    print("PASS", label, flush=True)
    save(
        "science",
        {
            "versions": {
                "h5py": h5py.__version__,
                "hdf5": h5py.version.hdf5_version,
                "netCDF4": netCDF4.__version__,
                "numpy": np.__version__,
            },
            "cells": cells,
        },
    )
    return cells


def public(reference=None):
    WORK.mkdir(parents=True, exist_ok=True)
    req = request(URL, "/RRQPE", (2400, 2400), (384, 384))
    req["source"]["assets"][0]["datetime"] = "2025-04-10T12:05:06.527315Z"
    result = run_native(req, "public-real", "dump", "adjacent")
    assert result["returncode"] == 0, result["error"]
    # The independent full-file control is explicit and version matched. An
    # explicitly supplied frozen control avoids another provider transfer.
    version = result["result"]["inspection"]["sources"][0]["object_identities"][
        "identity"
    ]["version"]
    path = (
        Path(reference)
        if reference is not None
        else WORK / "goes19-reference-control.nc"
    )
    if reference is None:
        response = requests.get(URL, headers={"If-Match": version["etag"]}, timeout=30)
        response.raise_for_status()
        reference_bytes = response.content
        reference_etag = response.headers.get("ETag")
        path.write_bytes(reference_bytes)
    else:
        reference_bytes = path.read_bytes()
        reference_etag = version["etag"]
    assert reference_etag == version["etag"]
    assert len(reference_bytes) == version["size"] == 1268526
    assert hashlib.md5(reference_bytes).hexdigest() == reference_etag.strip('"')
    checks = validate(result, path, req)
    with h5py.File(path, "r") as f:
        ds = f["RRQPE"]
        index = f.id.get_create_plist().get_version()
        meta = {
            "dtype": str(ds.dtype),
            "chunks": ds.chunks,
            "allocated_chunks": ds.id.get_num_chunks(),
            "filters": [
                str(ds.id.get_create_plist().get_filter(i))
                for i in range(ds.id.get_create_plist().get_nfilters())
            ],
            "hdf5_versions": index,
        }
        start = req["spatial"]["start"]
        count = req["spatial"]["count"]
        sel = tuple(slice(a, a + n) for a, n in zip(start, count))
        values = ds[sel]
        valid = values != ds.fillvalue
        physical = (
            values[valid].astype("f8") * ds.attrs["scale_factor"].item()
            + ds.attrs["add_offset"].item()
        )
        analysis = {
            "valid_pixels": int(valid.sum()),
            "missing_pixels": int((~valid).sum()),
            "rain_pixels": int(np.count_nonzero(physical > 0)),
            "mean_rain_rate_mm_per_hour": float(physical.mean()),
            "max_rain_rate_mm_per_hour": float(physical.max()),
        }
    # Analyze the native chunks themselves; no reference object is needed here.
    unpacked = []
    for i, c in enumerate(result["result"]["runs"][0]["chunks"]):
        raw = np.fromfile(Path(result["output_dir"]) / f"{i}.bin", dtype="<u2")
        raw = raw[raw != c["nodata"]]
        unpacked.append(raw.astype("f8") * c["scale"] + c["offset"])
    native_physical = np.concatenate(unpacked)
    assert np.array_equal(np.sort(native_physical), np.sort(physical))
    analysis["analyzed_from_native_chunks"] = True
    analysis["whole_file_required_for_analysis"] = False
    analysis["mean_rain_rate_mm_per_hour"] = float(native_physical.mean())
    transport = result["result"]["runs"][0]["observed"]["transport"]["science"]
    source = result["result"]["inspection"]["sources"][0]
    save(
        "public",
        {
            "url": URL,
            "source_bytes": len(reference_bytes),
            "requested_slice": req["spatial"],
            "delivered_value_bytes": checks["output_bytes"],
            "native_response_bytes": transport["bytes"],
            "native_requests": transport["requests"],
            "native_fraction": transport["bytes"] / len(reference_bytes),
            "checks": checks,
            "source_metadata": meta,
            "analysis": analysis,
            "source": source,
            "native": compact(result),
            "timings": {
                "prepare_seconds": result["result"]["prepare_seconds"],
                "completion_seconds": result["result"]["seconds"],
                "first_scientific_values_seconds": result["result"]["runs"][0][
                    "first_chunk_seconds"
                ],
            },
            "transport": transport,
            "reference_control": {
                "whole_object_download": True,
                "frozen_control_explicitly_supplied": reference is not None,
                "network_bytes_this_run": 0
                if reference is not None
                else len(reference_bytes),
                "bytes": len(reference_bytes),
                "purpose": "independent h5py and netCDF4 comparison, never used by native acquisition",
                "etag": reference_etag,
                "md5": hashlib.md5(reference_bytes).hexdigest(),
                "etag_equals_md5": reference_etag.strip('"')
                == hashlib.md5(reference_bytes).hexdigest(),
            },
        },
    )
    (DOCS / "goes19-request.json").write_text(json.dumps(req, indent=2) + "\n")
    print(
        "PUBLIC PASS",
        checks["values"],
        "values;",
        transport["bytes"],
        "of",
        len(reference_bytes),
        "bytes",
        flush=True,
    )


def faults():
    cells = []
    path, ds = fixture("fault-source", fletcher=True)
    for fault, scope in [
        ("no-etag", "all"),
        ("weak-etag", "all"),
        ("no-range", "payload"),
        ("bad-range", "payload"),
        ("encoded", "payload"),
        ("changed-etag", "payload"),
        ("retry", "payload"),
        ("exhaust", "payload"),
        ("interrupt", "payload"),
    ]:
        with server(path, ds, fault=fault, fault_scope=scope) as s:
            req = request(s.url, ds, (0, 0), (63, 95))
            result = run_native(req, "fault-" + fault, "stream")
            if fault == "retry":
                assert result["returncode"] == 0, result["error"]
            else:
                assert result["returncode"] != 0, fault
            cells.append(
                {
                    "name": fault,
                    "expected_success": fault == "retry",
                    **compact(result),
                    "http": s.stats(),
                }
            )
            print("PASS fault", fault, flush=True)
    for mode in ["before", "during", "cancel", "cancel-inflight"]:
        with server(path, ds, latency=0.01) as s:
            req = request(s.url, ds, (0, 0), (128, 192))
            result = run_native(
                req,
                "lifecycle-" + mode,
                mode,
                control=s.url.replace(
                    "/data.nc",
                    "/control/wait-payload"
                    if mode == "cancel-inflight"
                    else "/control",
                ),
            )
            assert result["returncode"] != 0, (mode, "false successful finish")
            cells.append(
                {
                    "name": mode,
                    "expected_success": False,
                    **compact(result),
                    "http": s.stats(),
                }
            )
            print("PASS lifecycle", mode, flush=True)
    for name, kw in [
        ("modern-index", {"latest": True}),
        ("unsupported-filter", {"compression": "lzf"}),
        ("contiguous", {"chunks": None, "compression": None}),
    ]:
        p, v = fixture(name, **kw)
        result = run_native(request(p, v), "unsupported-" + name, "stream")
        assert result["returncode"] != 0, (name, result)
        cells.append({"name": name, **compact(result)})
    # Stored checksums are checked by the established filter implementation.
    p, v = fixture("corrupt-checksum", fletcher=True)
    offset, size = payload_ranges(p, v)[0]
    raw = bytearray(p.read_bytes())
    raw[offset + size - 1] ^= 1
    p.write_bytes(raw)
    result = run_native(request(p, v), "corrupt-checksum", "stream")
    assert result["returncode"] != 0
    cells.append({"name": "corrupt-Fletcher32", **compact(result)})
    p, v = fixture("corrupt-deflate", fletcher=False)
    offset, size = payload_ranges(p, v)[0]
    raw = bytearray(p.read_bytes())
    raw[offset + size // 2] ^= 128
    p.write_bytes(raw)
    result = run_native(request(p, v), "corrupt-deflate", "stream")
    assert result["returncode"] != 0
    cells.append({"name": "corrupt-deflate", **compact(result)})
    p, v = fixture("bad-offset")
    offset, size = payload_ranges(p, v)[0]
    raw = bytearray(p.read_bytes())
    old = struct.pack("<Q", offset)
    new = struct.pack("<Q", len(raw) + 8192)
    assert raw.count(old) >= 1
    raw = raw.replace(old, new)
    p.write_bytes(raw)
    result = run_native(request(p, v), "bad-offset", "stream")
    assert result["returncode"] != 0
    cells.append({"name": "chunk-offset-outside-file", **compact(result)})
    p = WORK / "truncated.nc"
    p.write_bytes(path.read_bytes()[:80])
    result = run_native(request(p, ds), "truncated-metadata", "stream")
    assert result["returncode"] != 0
    cells.append({"name": "truncated-metadata", **compact(result)})
    p = WORK / "classic.nc"
    with netCDF4.Dataset(p, "w", format="NETCDF3_CLASSIC") as f:
        f.createDimension("x", 4)
        f.createVariable("x", "f4", ("x",))[:] = range(4)
    result = run_native(request(p, "/x"), "classic", "stream")
    assert result["returncode"] != 0
    cells.append({"name": "classic-netcdf-rejected", **compact(result)})
    p, v = fixture("layout-errors")
    with h5py.File(p, "a") as f:
        f.create_dataset("rank3", shape=(3, 4, 5), dtype="<f4", chunks=(1, 2, 3))
        f["external"] = h5py.ExternalLink("not-allowed.nc", "/signal")
        f.create_virtual_dataset(
            "virtual", h5py.VirtualLayout(shape=(257, 385), dtype="<f4")
        )
    for target in ["/rank3", "/external", "/virtual"]:
        req = request(p, target)
        req["spatial"]["axes"] = ["/science/y", "/science/x"]
        result = run_native(req, "unsupported-" + target.strip("/"), "stream")
        assert result["returncode"] != 0
        cells.append({"name": target, **compact(result)})
    for label, change in [
        ("small-budget", lambda r: r["budget"].update(buffer_bytes=16384)),
        ("out-of-bounds", lambda r: r["spatial"].update(start=[1000, 0])),
        ("zero-stride", lambda r: r["spatial"].update(step=[0, 1])),
        ("huge-count", lambda r: r["spatial"].update(count=[2**64 - 1, 2**64 - 1])),
        ("max-nodes", lambda r: r["budget"].update(max_nodes=4)),
        (
            "file-output",
            lambda r: r.update(
                sink={"kind": "directory", "path": str(WORK / "rejected-output")}
            ),
        ),
        (
            "time-filter",
            lambda r: r.update(
                time={"kind": "instants", "values": ["2025-01-01T00:00:00Z"]}
            ),
        ),
    ]:
        req = request(path, ds)
        change(req)
        result = run_native(req, label, "stream")
        assert result["returncode"] != 0, label
        cells.append({"name": label, **compact(result)})
    p = WORK / "implicit-fill.nc"
    with h5py.File(p, "w", libver="earliest") as f:
        f.create_dataset("y", data=np.arange(8, dtype="f8"))
        f.create_dataset("x", data=np.arange(11, dtype="f8"))
        d = f.create_dataset("signal", shape=(8, 11), chunks=(3, 4), dtype="u2")
        d.attrs["_FillValue"] = np.uint16(65535)
        assert np.array_equal(d[:], np.zeros((8, 11), dtype="u2"))
    req=request(p,"/signal",count=(8,11))
    result=run_native(req,"implicit-fill","dump")
    validate(result,p,req)
    assert result["result"]["runs"][0]["valid_pixels"]==88
    assert result["result"]["runs"][0]["sum"]==0
    cells.append(
        {
            "name": "independent-default-storage-zero-and-CF-marker",
            "h5py_stored_initialization": 0,
            **compact(result),
        }
    )
    # Demonstrate the inherited integrity limit with a raw chunk and unchanged
    # object identity: accepted transport cannot authenticate arbitrary raw bytes.
    p, v = fixture("unchecksummed", compression=None, shuffle=False)
    with server(p, v, fault="raw-bitflip") as s:
        req = request(s.url, v)
        result = run_native(req, "raw-integrity", "dump")
        assert result["returncode"] == 0, result["error"]
        mismatch = False
        try:
            validate(result, p, req)
        except AssertionError:
            mismatch = True
        assert mismatch
        qualification = {
            "same_etag_raw_corruption_undetected": True,
            "transport_completed": True,
            "independent_oracle_detected_mismatch": True,
            "guarantee": "strong ETags pin identity; raw unchecksummed bytes are not authenticated",
            "http": s.stats(),
        }
    save("faults", {"cases": cells, "integrity_qualification": qualification})
    print("FAULTS PASS", len(cells), flush=True)


def reference_worker(url, dataset, selection, page=0, full=False):
    path = WORK / "reference-request.json"
    path.write_text(
        json.dumps(
            {
                "url": url,
                "dataset": dataset,
                "slice": selection,
                "page": page,
                "full": full,
            }
        )
    )
    proc = subprocess.run(
        [sys.executable, str(ROOT / "bench/hdf5_reference.py"), str(path)],
        capture_output=True,
        check=False,
        text=True,
        timeout=120,
    )
    assert proc.returncode == 0, proc.stderr
    return json.loads(proc.stdout)


def benchmark():
    p, v = fixture("benchmark", shape=(768, 1024), chunks=(64, 64), holes=False)
    cells = []
    for latency in [0, 0.04]:
        for name, start, count, step in [
            ("sparse", (65, 70), (4, 4), (191, 251)),
            ("dense", (128, 128), (384, 512), (1, 1)),
        ]:
            selection = tuple(
                slice(a, a + n * s, s) for a, n, s in zip(start, count, step)
            )
            with h5py.File(p, "r") as f:
                expected = f[v][selection].astype("<u2")
                digest = hashlib.sha256(expected.tobytes()).hexdigest()
            for repeat in range(3):
                # Alternate orders to reduce systematic connection/thermal ordering bias.
                policies = (
                    ["individual", "adjacent", "bounded"]
                    if repeat % 2 == 0
                    else ["bounded", "adjacent", "individual"]
                )
                for policy in policies:
                    with server(p, v, latency=latency) as s:
                        req = request(s.url, v, start, count, step)
                        result = run_native(
                            req,
                            f"bench-{name}-{latency}-{repeat}-{policy}",
                            "dump",
                            policy,
                            workers=1,
                        )
                        checks = validate(result, p, req)
                        stats = s.stats()
                        native = result["result"]
                        report = native["runs"][0]["observed"]
                        transport = report["transport"]["science"]
                        cells.append(
                            {
                                "reader": "tile-ripper",
                                "latency_seconds": latency,
                                "selection": name,
                                "repeat": repeat,
                                "policy": policy,
                                **compact(result),
                                "scientific_equivalence": True,
                                "values": checks["values"],
                                "initial_metadata_index_seconds": native[
                                    "prepare_seconds"
                                ],
                                "first_scientific_values_seconds": native["runs"][0][
                                    "first_chunk_seconds"
                                ],
                                "completion_seconds": native["seconds"],
                                "read_decode_seconds": report["read_decode_seconds"],
                                "source_open_payload_seconds": report[
                                    "source_open_seconds"
                                ],
                                "scheduler": report["scheduler"],
                                "transport": transport,
                                "server": {
                                    k: z for k, z in stats.items() if k != "events"
                                },
                                "selected_compressed_bytes": native["inspection"][
                                    "plan"
                                ]["estimates"]["selected_compressed_payload_bytes"],
                                "payload_bytes": report["source_data_bytes"],
                                "payload_gets": report["source_data_requests"],
                            }
                        )
                for page in [0, 4096, 65536]:
                    with server(p, v, latency=latency) as s:
                        ref = reference_worker(s.url, v, req["spatial"], page)
                        assert ref["value_sha256"] == digest
                        cells.append(
                            {
                                "latency_seconds": latency,
                                "selection": name,
                                "repeat": repeat,
                                **ref,
                                "scientific_equivalence": True,
                            }
                        )
                # Full-file retrieval stays an explicit independently executed control.
                with server(p, v, latency=latency) as s:
                    ref = reference_worker(s.url, v, req["spatial"], full=True)
                    assert ref["value_sha256"] == digest
                    cells.append(
                        {
                            "latency_seconds": latency,
                            "selection": name,
                            "repeat": repeat,
                            **ref,
                            "scientific_equivalence": True,
                        }
                    )
            print("BENCH PASS", name, latency, flush=True)
    save(
        "benchmark",
        {
            "fixture_bytes": p.stat().st_size,
            "shape": [768, 1024],
            "chunks": [64, 64],
            "workers": 1,
            "network_model": "controlled per-request latency; loopback bandwidth; not a WAN throughput claim",
            "cells": cells,
        },
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("phase", choices=["science", "public", "faults", "benchmark"])
    parser.add_argument(
        "--reference",
        type=Path,
        help="Explicit version-matched frozen full-file oracle for public validation",
    )
    args = parser.parse_args()
    if args.reference is not None and args.phase != "public":
        parser.error("--reference applies to public validation only")
    public(args.reference) if args.phase == "public" else globals()[args.phase]()


if __name__ == "__main__":
    main()
