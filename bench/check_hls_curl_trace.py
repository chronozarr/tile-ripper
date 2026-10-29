"""Stress the diagnostic curl interposer with concurrent local TIFF handle reuse."""

import argparse
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time

import numpy as np
import rasterio
from rasterio.io import MemoryFile
from rasterio.transform import from_origin

from hls_river_30day import save
from summarize_hls_http import summarize, concurrency


def worker(url, repeats):
    source = np.arange(1536 * 1536, dtype="int32").reshape(1536, 1536).astype("int16")

    def read(index):
        with rasterio.Env(
            GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
            GDAL_HTTP_NETRC="NO",
            GDAL_HTTP_VERSION="1.1",
            GDAL_HTTP_MULTIRANGE="YES",
            GDAL_HTTP_MAX_TOTAL_CONNECTIONS="4",
            GDAL_HTTP_MULTIPLEX="NO",
            GDAL_NUM_THREADS="1",
            GDAL_HTTP_HEADERS='If-Match: "fixture"',
            CPL_LOG=os.devnull,
        ):
            with rasterio.open(
                f"{url}?iteration={index}&sig=fixture-secret"
            ) as dataset:
                data = dataset.read(
                    1, window=rasterio.windows.Window(250, 250, 333, 332)
                )
        assert data.tobytes() == source[250:582, 250:583].tobytes()

    with (
        rasterio.Env(GDAL_CACHEMAX=8 * 2**20),
        ThreadPoolExecutor(max_workers=4) as pool,
    ):
        list(pool.map(read, range(repeats)))
    print(json.dumps({"pixel_reads_passed": repeats}), flush=True)


def main(work, dylib, repeats):
    work.mkdir(parents=True, exist_ok=False)
    source = np.arange(1536 * 1536, dtype="int32").reshape(1536, 1536).astype("int16")
    with MemoryFile() as memory:
        with memory.open(
            driver="GTiff",
            width=1536,
            height=1536,
            count=1,
            dtype="int16",
            crs="EPSG:32618",
            transform=from_origin(0, 1536, 1, 1),
            tiled=True,
            blockxsize=128,
            blockysize=128,
            compress="deflate",
        ) as dataset:
            dataset.write(source, 1)
        data = memory.read()
    records, mutex = [], threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):
            pass

        def do_HEAD(self):
            self.serve(False)

        def do_GET(self):
            self.serve(True)

        def serve(self, body):
            began = time.time()
            bounds = self.headers.get("Range")
            # CPL's header-option parser may strip token quotes. This local
            # tracer fixture accepts either spelling of its synthetic tag.
            assert self.headers.get("If-Match", "").strip('"') == "fixture"
            lo, hi = 0, len(data) - 1
            if bounds:
                assert bounds.startswith("bytes=") and "," not in bounds
                left, right = bounds[6:].split("-")
                lo, hi = int(left), min(int(right) if right else hi, hi)
            time.sleep(0.002)
            response_start = time.time()
            self.send_response(206 if body and bounds else 200)
            self.send_header("Content-Length", str(hi - lo + 1))
            self.send_header("ETag", '"fixture"')
            self.send_header("Accept-Ranges", "bytes")
            if body and bounds:
                self.send_header("Content-Range", f"bytes {lo}-{hi}/{len(data)}")
            self.end_headers()
            if body:
                self.wfile.write(data[lo : hi + 1])
            self.wfile.flush()
            with mutex:
                records.append(
                    {
                        "method": self.command,
                        "range": bounds,
                        "bytes": hi - lo + 1 if body else 0,
                        "start_unix_s": began,
                        "end_unix_s": response_start,
                    }
                )

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    trace = work / "curl.jsonl"
    env = dict(os.environ, DYLD_INSERT_LIBRARIES=str(dylib), TR_CURL_TRACE=str(trace))
    try:
        result = subprocess.run(
            [
                sys.executable,
                __file__,
                "--worker",
                f"http://127.0.0.1:{server.server_port}/fixture.tif",
                "--repeats",
                str(repeats),
            ],
            env=env,
            capture_output=True,
            text=True,
        )
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
    (work / "stdout.log").write_text(result.stdout)
    (work / "stderr.log").write_text(result.stderr)
    assert result.returncode == 0, "fixture worker failed"
    assert "fixture-secret" not in trace.read_text() + result.stdout + result.stderr
    report = summarize([trace])
    assert report["transactions"] == len(records)
    assert report["body_bytes"] == sum(r["bytes"] for r in records)
    assert report["concurrency"]["peak"] <= 16
    server_peak = concurrency(records)["peak"]
    assert server_peak <= report["concurrency"]["peak"]
    summary = {
        "status": "passed",
        "reads": repeats,
        "transactions": len(records),
        "body_bytes": report["body_bytes"],
        "traced_peak": report["concurrency"]["peak"],
        "independent_server_overlap_peak": server_peak,
        "curl_summaries": report["curl_summaries"],
        "credentials_redacted": True,
    }
    save(work / "receipt.json", summary)
    print(json.dumps(summary), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path)
    parser.add_argument("--dylib", type=Path)
    parser.add_argument("--repeats", type=int, default=96)
    parser.add_argument("--worker")
    args = parser.parse_args()
    if args.worker:
        worker(args.worker, args.repeats)
    else:
        main(args.work.resolve(), args.dylib.resolve(), args.repeats)
