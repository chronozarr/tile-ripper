"""Interleaved local/HTTP benchmarks. No public network data tests or paid infrastructure."""

import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import platform
import random
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import numpy as np
import rasterio
from rasterio.windows import Window
from make_fixture import create

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
TUNED = dict(
    GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
    GDAL_INGESTED_BYTES_AT_OPEN="32768",
    GDAL_NUM_THREADS="1",
    GDAL_CACHEMAX="32",
    GDAL_HTTP_MAX_RETRY="3",
    GDAL_HTTP_TIMEOUT="45",
)


def process(cmd):
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        start = time.perf_counter()
        p = subprocess.Popen(cmd, stdout=out, stderr=err, env={**os.environ, **TUNED})
        _, status, usage = os.wait4(p.pid, 0)
        p.returncode = os.waitstatus_to_exitcode(status)
        wall = time.perf_counter() - start
        out.seek(0)
        err.seek(0)
        text = out.read().decode()
        error = err.read().decode()
        if p.returncode:
            raise RuntimeError(f"worker failed: {error[:1000]}")
    return (
        json.loads(text),
        wall,
        usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024),
    )


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def reset(self):
        self.counts = dict(head=0, get=0, bytes=0)


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def do_HEAD(self):
        self.serve(False)

    def do_GET(self):
        self.serve(True)

    def serve(self, body):
        if self.path.split("?")[0] != "/source.tif":
            self.send_error(404)
            return
        path = self.server.fixture
        size = path.stat().st_size
        start, end = 0, size - 1
        if "Range" in self.headers:
            match = re.fullmatch(r"bytes=(\d+)-(\d*)", self.headers["Range"])
            if not match:
                self.send_error(416)
                return
            start = int(match[1])
            end = min(size - 1, int(match[2]) if match[2] else size - 1)
            if start > end:
                self.send_error(416)
                return
        self.send_response(206 if "Range" in self.headers else 200)
        self.send_header("Content-Length", str(end - start + 1))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("ETag", self.server.etag)
        if "Range" in self.headers:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        if body:
            self.server.counts["get"] += 1
            self.server.counts["bytes"] += end - start + 1
        else:
            self.server.counts["head"] += 1
        self.end_headers()
        if body:
            with path.open("rb") as f:
                f.seek(start)
                self.wfile.write(f.read(end - start + 1))


def verify(root, source, win):
    x, y, w, h = win
    seen = np.zeros((h, w), dtype=np.uint8)
    with rasterio.open(source) as src:
        for path in root.glob("*.tif"):
            with rasterio.open(path) as out:
                col, row = (~src.transform) * (out.transform.c, out.transform.f)
                col, row = round(col), round(row)
                actual = out.read(1)
                expected = src.read(1, window=Window(col, row, out.width, out.height))
                assert np.array_equal(actual, expected), path
                assert out.transform == src.window_transform(
                    Window(col, row, out.width, out.height)
                )
                assert (
                    out.crs == src.crs
                    and out.nodata == src.nodata
                    and out.scales == src.scales
                    and out.offsets == src.offsets
                )
                assert (
                    col >= x
                    and row >= y
                    and col + out.width <= x + w
                    and row + out.height <= y + h
                )
                seen[row - y : row - y + out.height, col - x : col - x + out.width] += 1
    assert np.all(seen == 1), "missing or duplicate output pixels"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--reps", type=int, default=5)
    ap.add_argument("--output", default="prototype-output/benchmark")
    args = ap.parse_args()
    if args.reps < 1:
        ap.error("reps must be positive")
    root = Path(args.output).resolve()
    root.mkdir(parents=True, exist_ok=True)
    (root / "runs.jsonl").write_text("")
    fixtures = root / "fixtures"
    requests = create(fixtures)
    binary = REPO / "target/release/tile-ripper"
    server = Server(("127.0.0.1", 0), Handler)
    server.fixture = fixtures / "source.tif"
    server.etag = '"' + hashlib.sha256(server.fixture.read_bytes()).hexdigest() + '"'
    server.reset()
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    runs = []
    rng = random.Random(20261018)
    try:
        for name, (base, win) in requests.items():
            for transport in ("local", "http"):
                req = json.loads(json.dumps(base))
                if transport == "http":
                    req["source"]["assets"][0]["locator"] = (
                        f"http://127.0.0.1:{server.server_port}/source.tif"
                    )
                request = root / "request.json"
                request.write_text(json.dumps(req))
                cache = root / f"{name}-{transport}-layout.json"
                # Warm local metadata is seeded outside timed runs. HTTP is index-free,
                # and intentionally has no warm label until ETag validation exists.
                configs = ["tile-ripper-cold", "gdal-tuned"]
                if transport == "local":
                    subprocess.run(
                        [str(binary), "plan", str(request), "--cache", str(cache)],
                        stdout=subprocess.DEVNULL,
                        check=True,
                    )
                    configs.append("tile-ripper-warm")
                for rep in range(args.reps):
                    order = configs.copy()
                    rng.shuffle(order)
                    for config in order:
                        out = root / "out"
                        if out.exists():
                            shutil.rmtree(out)
                        server.reset()
                        if config == "gdal-tuned":
                            cmd = [
                                sys.executable,
                                str(HERE / "gdal_worker.py"),
                                str(request),
                                str(out),
                            ]
                        else:
                            cmd = [
                                str(binary),
                                "fetch",
                                str(request),
                                "--output",
                                str(out),
                            ]
                            if config.endswith("warm"):
                                cmd.extend(["--cache", str(cache)])
                        result, wall, rss = process(cmd)
                        counts = dict(server.counts)
                        verify(out, server.fixture, win)
                        row = dict(
                            workload=name,
                            transport=transport,
                            config=config,
                            rep=rep,
                            wall_seconds=wall,
                            peak_rss_bytes=rss,
                            observed_http=counts if transport == "http" else None,
                            correct=True,
                            worker=result,
                        )
                        runs.append(row)
                        with (root / "runs.jsonl").open("a") as f:
                            f.write(json.dumps(row) + "\n")
    finally:
        server.shutdown()
        server.server_close()
    summary = []
    for key in sorted({(r["workload"], r["transport"], r["config"]) for r in runs}):
        rows = [r for r in runs if (r["workload"], r["transport"], r["config"]) == key]
        values = [r["wall_seconds"] for r in rows]
        summary.append(
            dict(
                workload=key[0],
                transport=key[1],
                config=key[2],
                n=len(rows),
                median_seconds=statistics.median(values),
                min_seconds=min(values),
                max_seconds=max(values),
                stdev_seconds=statistics.stdev(values) if len(values) > 1 else 0,
                peak_rss_bytes=max(r["peak_rss_bytes"] for r in rows),
                requests=rows[0]["observed_http"],
            )
        )
    result = dict(
        environment=dict(
            platform=platform.platform(),
            python=sys.version,
            rasterio=rasterio.__version__,
            gdal_oracle=rasterio.__gdal_version__,
            gdal_rust=subprocess.check_output(
                ["gdal-config", "--version"], text=True
            ).strip(),
            rustc=subprocess.check_output(["rustc", "--version"], text=True).strip(),
            fixture_sha256=hashlib.sha256(server.fixture.read_bytes()).hexdigest(),
            cargo_lock_sha256=hashlib.sha256(
                (REPO / "Cargo.lock").read_bytes()
            ).hexdigest(),
            tuned=TUNED,
        ),
        scope="local HTTP loopback, serial execution, warm OS cache, cold/warm local layout metadata only; imports included in process timing; no network performance claim",
        summary=summary,
        runs=runs,
    )
    (root / "results.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
