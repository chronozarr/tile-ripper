"""HTTP diagnostics/control around the unchanged frozen HLS benchmark.

Trace only safe source basenames, range intervals, status, bytes and times.
GDAL's benchmark-only libcurl interposer records individual actual transfers;
requests and native record dispatch-through-body transaction intervals. Trace
runs are excluded from timing results. The `capped` control keeps GDAL's range
planning and allows one HTTP/1.1 connection per worker. The exploratory `serial`
control also changes the multi-range read strategy and is labeled separately.
The `sixteen` control allows four GDAL connections per each of four asset
workers and uses sixteen native workers with a sixteen-request per-host limit.
The `parallel16` control gives Python sixteen workers across scenes with one
connection each; its original source-read, QA, provenance and encoder code runs
unchanged. At most thirty-two pending asset results are retained, so the frozen
32-asset workload can keep all sixteen workers occupied despite ordered output.
"""

from __future__ import annotations

import argparse
import itertools
import json
import os
from pathlib import Path
import threading
import time
from urllib.parse import urlsplit

import hls_river_30day as recipe


def configure(mode, trace, binary, protocol, route):
    if mode == "serial" and route == "python":
        import rasterio

        original_env = rasterio.Env

        def serial_env(*args, **kwargs):
            kwargs["GDAL_HTTP_MULTIRANGE"] = "SERIAL"
            return original_env(*args, **kwargs)

        rasterio.Env = serial_env
    elif mode in {"capped", "sixteen", "parallel16"} and route == "python":
        import rasterio

        original_env = rasterio.Env

        def capped_env(*args, **kwargs):
            kwargs.update(
                GDAL_HTTP_MULTIRANGE="YES",
                GDAL_HTTP_VERSION="1.1",
                GDAL_HTTP_MAX_TOTAL_CONNECTIONS="4" if mode == "sixteen" else "1",
                GDAL_HTTP_MULTIPLEX="NO",
            )
            return original_env(*args, **kwargs)

        rasterio.Env = capped_env

    if mode == "parallel16" and route == "python":
        from hls_scene_pool import ScenePool

        pools = []

        def scene_pool(**kwargs):
            pool = ScenePool(protocol, **kwargs)
            pools.append(pool)
            return pool

        recipe.ThreadPoolExecutor = scene_pool
        original_pipeline = recipe.python_pipeline

        def parallel_pipeline(*args):
            report = original_pipeline(*args)
            assert len(pools) == 1
            report["cross_scene_acquisition"] = pools[0].stats()
            return report

        recipe.python_pipeline = parallel_pipeline

    if binary or mode in {"sixteen", "parallel16"}:
        original_run = recipe.subprocess.run

        def native_run(command, *args, **kwargs):
            if command[0] == str(recipe.ROOT / "target/release/tile-ripper"):
                command = [str(binary) if binary else command[0], *command[1:]]
                if mode in {"sixteen", "parallel16"}:
                    for option in ["--workers", "--per-host"]:
                        index = command.index(option) + 1
                        assert command[index] == "4", "recipe concurrency changed"
                        command[index] = "16"
            return original_run(command, *args, **kwargs)

        recipe.subprocess.run = native_run

    if not trace:
        return
    import requests

    original_send = requests.Session.send
    sources = {urlsplit(p["href"]).path.rsplit("/", 1)[-1] for p in protocol["pins"]}
    mutex, ids = threading.Lock(), itertools.count(1)
    trace.parent.mkdir(parents=True, exist_ok=True)

    def event(value):
        value.update(time_unix_s=time.time(), pid=os.getpid())
        with mutex, trace.open("a") as file:
            file.write(json.dumps(value, separators=(",", ":")) + "\n")

    def traced_send(self, request, **kwargs):
        source = urlsplit(request.url).path.rsplit("/", 1)[-1]
        if source not in sources:
            return original_send(self, request, **kwargs)
        ident = next(ids)
        event(
            dict(
                event="start",
                id=ident,
                client="requests",
                source=source,
                method=request.method,
                range=request.headers.get("Range"),
                conditional="If-Match" in request.headers,
            )
        )
        response = None
        try:
            response = original_send(self, request, **kwargs)
            return response
        finally:
            event(
                dict(
                    event="end",
                    id=ident,
                    client="requests",
                    status=response.status_code if response is not None else None,
                    bytes=len(response.content) if response is not None else 0,
                )
            )

    requests.Session.send = traced_send


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--route", choices=["python", "tile"], required=True)
    parser.add_argument(
        "--mode",
        choices=["default", "serial", "capped", "sixteen", "parallel16"],
        default="default",
    )
    parser.add_argument("--trace", type=Path)
    parser.add_argument("--binary", type=Path)
    args = parser.parse_args()
    protocol = json.loads((args.work / "protocol.json").read_text())
    configure(args.mode, args.trace, args.binary, protocol, args.route)
    recipe.worker(args.route, args.work, args.out)


if __name__ == "__main__":
    main()
