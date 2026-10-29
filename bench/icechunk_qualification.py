#!/usr/bin/env python3
"""Bounded, independently metered Dynamical Icechunk source comparison.

Install reference dependencies in a dedicated environment, not Tile-Ripper's build:
    uv run --python 3.13 --with icechunk==2.3.0 --with zarr==3.4.1 \
      --with gribberish==1.8.4 --with numpy --with requests \
      bench/icechunk_qualification.py --output docs/icechunk/reference-receipt.json

All upstream reads pass through a loopback forwarding server with a hard total
response-body cap. Counters include metadata, source chunks, retries and duplicate
reads, not HTTP headers/TLS overhead. This is a Python reference, not a Rust speed
claim. Snapshots are pinned once per product and recorded. No credentials are read.
"""
from __future__ import annotations

import argparse
import contextlib
from datetime import datetime, timezone
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.metadata
import json
from pathlib import Path
import threading
import time
import urllib.parse

import icechunk
import numpy as np
import requests
import zarr
from gribberish.zarr import GribberishCodec  # noqa: F401; codec registration

PRODUCTS = {
    "materialized": "noaa-gfs-analysis",
    "virtual": "noaa-gfs-analysis-virtual",
}
BASE = "https://dynamical-noaa-gfs.s3.us-west-2.amazonaws.com"
VIRTUAL_PREFIX = "s3://noaa-gfs-bdp-pds/"
DEFAULT_SNAPSHOTS = {
    "materialized": "6K6H23PYR6QXA9VM0C6G",
    "virtual": "SJSV5KG1GSWR2XQD2SW0",
}


class Meter:
    def __init__(self, cap: int):
        self.cap = cap
        self.bytes = 0
        self.events = []
        self.lock = threading.Lock()
        self.stage = "initializing"

    def mark(self, stage):
        with self.lock:
            self.stage = stage
            return self.bytes, len(self.events)

    def since(self, marker):
        with self.lock:
            events = self.events[marker[1]:]
            return {
                "response_body_bytes": self.bytes - marker[0],
                "http_operations": len(events),
                "events": events,
            }

    def consume(self, count):
        with self.lock:
            if self.bytes + count > self.cap:
                raise RuntimeError(f"response body cap {self.cap} bytes exceeded")
            self.bytes += count


class Forwarder(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def do_HEAD(self):
        self.forward()

    def do_GET(self):
        self.forward()

    def forward(self):
        meter = self.server.meter
        raw = self.path
        if raw.startswith("/repo/"):
            remote = BASE + raw[len("/repo"):]
            kind = "repository"
        elif raw.startswith("/legacy/"):
            remote = "https://s3.us-west-2.amazonaws.com/us-west-2.opendata.source.coop/dynamical/noaa-gfs-analysis/v0.1.0.zarr/" + raw[len("/legacy/"):]
            kind = "legacy_zarr"
        elif raw.startswith("/noaa-gfs-bdp-pds/"):
            remote = "https://noaa-gfs-bdp-pds.s3.amazonaws.com/" + raw[len("/noaa-gfs-bdp-pds/"):]
            kind = "virtual_source"
        else:
            self.send_error(403, "unadmitted origin")
            return
        request_headers = {k: self.headers[k] for k in ["Range", "If-Match", "If-Unmodified-Since"] if k in self.headers}
        started = time.perf_counter()
        try:
            session = requests.Session()
            session.trust_env = False
            with session.request(self.command, remote, headers=request_headers, stream=True, timeout=(10, 40), allow_redirects=False) as response:
                declared = int(response.headers.get("Content-Length", "0"))
                with meter.lock:
                    if self.command != "HEAD" and declared + meter.bytes > meter.cap:
                        raise RuntimeError(f"next response {declared} exceeds remaining response-body budget")
                chunks = []
                for block in response.iter_content(65536):
                    meter.consume(len(block))
                    chunks.append(block)
                body = b"".join(chunks)
                event = {
                    "stage": meter.stage, "method": self.command, "url": remote,
                    "kind": kind, "request_headers": request_headers,
                    "status": response.status_code, "response_body_bytes": len(body),
                    "etag": response.headers.get("ETag"),
                    "content_range": response.headers.get("Content-Range"),
                    "sha256": hashlib.sha256(body).hexdigest(),
                    "wall_seconds": time.perf_counter() - started,
                }
                with meter.lock:
                    meter.events.append(event)
                self.send_response(response.status_code)
                for key in ["Content-Type", "ETag", "Last-Modified", "Content-Range", "Accept-Ranges"]:
                    if key in response.headers:
                        self.send_header(key, response.headers[key])
                self.send_header("Content-Length", str(len(body) if self.command != "HEAD" else declared))
                self.end_headers()
                if self.command != "HEAD":
                    self.wfile.write(body)
        except Exception as exc:
            with meter.lock:
                meter.events.append({"stage": meter.stage, "url": remote, "error": str(exc)})
            self.send_error(502, str(exc))


@contextlib.contextmanager
def proxy(cap):
    server = ThreadingHTTPServer(("127.0.0.1", 0), Forwarder)
    server.daemon_threads = True
    server.meter = Meter(cap)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}", server.meter
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def open_product(base, kind, snapshot):
    config = icechunk.RepositoryConfig.default()
    if kind == "virtual":
        config.set_virtual_chunk_container(icechunk.VirtualChunkContainer(
            VIRTUAL_PREFIX, icechunk.s3_store(
                region="us-east-1", endpoint_url=base, allow_http=True,
                anonymous=True, force_path_style=True,
            ),
        ))
    config.max_concurrent_requests = 4
    credentials = {VIRTUAL_PREFIX: icechunk.Credentials.S3(icechunk.s3_anonymous_credentials())} if kind == "virtual" else {}
    root = f"{base}/repo/{PRODUCTS[kind]}/v0.1.0.icechunk"
    repository = icechunk.Repository.open(icechunk.http_storage(root), config=config, authorize_virtual_chunk_access=credentials)
    session = repository.readonly_session(snapshot_id=snapshot) if snapshot else repository.readonly_session("main")
    group = zarr.open_group(session.store, mode="r")
    return session, group


def numeric_receipt(values):
    a = np.asarray(values)
    finite = a[np.isfinite(a)]
    return {
        "shape": list(a.shape), "dtype": str(a.dtype),
        "logical_bytes": int(a.nbytes), "sha256": hashlib.sha256(a.tobytes()).hexdigest(),
        "finite_values": int(finite.size), "nan_values": int(np.isnan(a).sum()),
        "min": float(finite.min()) if finite.size else None,
        "max": float(finite.max()) if finite.size else None,
        "first_values": [float(x) if np.isfinite(x) else None for x in a.ravel()[:24]],
    }


def run(args):
    receipt = {
        "created_utc": datetime.now(timezone.utc).isoformat(),
        "purpose": "Independent maintained-Python-reader source correctness and response-body measurement; not Rust implementation speed evidence",
        "dependencies": {p: importlib.metadata.version(p) for p in ["icechunk", "zarr", "gribberish", "numpy", "requests"]},
        "per_product_response_body_budget_bytes": args.max_body_mib * 1024 * 1024,
        "credentials": "explicit anonymous only; no environment credential discovery",
        "products": {}, "comparisons": [],
        "measurement_limits": [
            "Response bodies only: excludes request/response headers, TCP/TLS and DNS.",
            "Loopback forwarding adds latency; compare bytes and selected values, not absolute production latency.",
            "Cold means a fresh Python repository/session and its in-process caches, not a cold provider/CDN cache.",
            "Warm repeats keep that session; Zarr may still re-fetch payloads because no decoded-value cache is installed.",
            "Only selected samples are compared; shared coordinates/names do not prove archive-wide equivalence.",
            "Materialized Float32 and virtual Float64 are different stored representations; tolerances are reported, never silently treated as bit equal.",
        ],
    }
    observed = {}
    # time index 48072 = 2026-10-25T00:00Z. Keep one 50x50 spatial chunk.
    # near 34N,101W uses rows224:226 and cols316:318.
    workloads = [("regional_one_time", 48072, 1, 224, 2, 316, 2)]
    if args.time_series:
        workloads.append(("regional_24_hours", 48072, 24, 224, 2, 316, 2))
    if args.map_window:
        workloads.append(("regional_map_101", 48072, 1, 200, 101, 300, 101))
    for kind in args.products:
        with proxy(args.max_body_mib * 1024 * 1024) as (base, meter):
            marker = meter.mark(f"{kind}:open_metadata")
            started = time.perf_counter()
            session, group = open_product(base, kind, None if args.latest else DEFAULT_SNAPSHOTS[kind])
            temp = group["temperature_2m"]
            coords = {key: group[key][:] for key in ["time", "latitude", "longitude"]}
            pr = {
                "catalog_url": f"https://stac.dynamical.org/{PRODUCTS[kind]}/collection.json",
                "repository_url": f"{BASE}/{PRODUCTS[kind]}/v0.1.0.icechunk",
                "snapshot_id": session.snapshot_id,
                "metadata": {key: group[key].metadata.to_dict() for key in ["temperature_2m", "time", "latitude", "longitude", "spatial_ref"]},
                "open_metadata_wall_seconds": time.perf_counter() - started,
                "open_metadata_transport": meter.since(marker), "workloads": [],
            }
            for name, t0, nt, y0, ny, x0, nx in workloads:
                repeats = []
                for trial in range(args.repeats):
                    marker = meter.mark(f"{kind}:{name}:{trial}")
                    started = time.perf_counter()
                    values = temp[t0:t0+nt, y0:y0+ny, x0:x0+nx]
                    repeats.append({"trial": trial, "session_cache": "retained; first payload read" if trial == 0 else "retained; repeated payload read", "wall_seconds": time.perf_counter()-started, "transport": meter.since(marker), "values": numeric_receipt(values)})
                    observed[kind, name] = values
                pr["workloads"].append({
                    "name": name, "index_start": [t0,y0,x0], "index_count": [nt,ny,nx],
                    "selected_coordinates": {"time": coords["time"][t0:t0+nt].tolist(), "latitude": coords["latitude"][y0:y0+ny].tolist(), "longitude": coords["longitude"][x0:x0+nx].tolist()},
                    "repeats": repeats,
                })
            pr["total_response_body_bytes"] = meter.bytes
            receipt["products"][kind] = pr
    for name, *_ in workloads:
        if ("materialized",name) in observed and ("virtual",name) in observed:
            m,v = observed["materialized",name],observed["virtual",name]
            diff = np.abs(m.astype("f8")-v)
            receipt["comparisons"].append({"name":name,"same_nan_mask":bool(np.array_equal(np.isnan(m),np.isnan(v))),"max_absolute_difference_celsius":float(np.nanmax(diff)),"equal_after_virtual_cast_float32":bool(np.array_equal(m,v.astype("f4"),equal_nan=True)),"allclose_atol_0.0001_rtol_0":bool(np.allclose(m,v,rtol=0,atol=0.0001,equal_nan=True))})
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output",type=Path,required=True)
    parser.add_argument("--products",nargs="+",choices=list(PRODUCTS),default=list(PRODUCTS))
    parser.add_argument("--max-body-mib",type=int,default=128)
    parser.add_argument("--repeats",type=int,default=2)
    parser.add_argument("--time-series",action="store_true")
    parser.add_argument("--map-window",action="store_true")
    parser.add_argument("--latest",action="store_true",help="Resolve live main instead of the documented snapshots")
    args=parser.parse_args()
    if not 1<=args.repeats<=3 or not 1<=args.max_body_mib<=256: parser.error("repeats must be 1..3, max-body-mib 1..256")
    receipt=run(args)
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(receipt,indent=2,allow_nan=False)+"\n")
    print(json.dumps({"output":str(args.output),"snapshots":{k:v["snapshot_id"] for k,v in receipt["products"].items()},"comparisons":receipt["comparisons"]}))

if __name__ == "__main__": main()
