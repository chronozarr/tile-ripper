"""Fresh-process, frozen-source reader/cache/order diagnostics, not production.

Profiles perform identical native checksum/count/sum work. Decoded GDAL block
cache size, dataset reuse and 256-pixel window traversal are varied explicitly;
the ordinary /vsicurl/ range cache and HTTP configuration stay unchanged.
"""

from concurrent.futures import ThreadPoolExecutor
import argparse
import copy
import hashlib
import json
import math
from pathlib import Path
import sys
import time
import zlib

import numpy as np
import rasterio
from rasterio.windows import Window

import cog_io_trace as trace
import real_multiscene as m


def worker(path, profile):
    req = json.loads(path.read_text())
    asset = req["source"]["assets"][0]
    bbox = req["spatial"]["bbox"]
    options = dict(
        m.GDAL_ENV,
        GDAL_CACHEMAX=0 if profile == "cache0" else 32 * 2**20,
        GDAL_HTTP_HEADERS=f"If-Match: {asset['etag']}\r\n",
    )
    started = time.perf_counter()
    with rasterio.Env(**options), rasterio.open(asset["locator"]) as source:
        c0, r0 = (~source.transform) * (bbox[0], bbox[3])
        c1, r1 = (~source.transform) * (bbox[2], bbox[1])
        windows = [
            Window(x, y, min(256, math.ceil(c1) - x), min(256, math.ceil(r1) - y))
            for y in range(math.floor(r0), math.ceil(r1), 256)
            for x in range(math.floor(c0), math.ceil(c1), 256)
        ]
        bx, by = source.block_shapes[0][::-1]
        metadata_seconds = time.perf_counter() - started

        def read(ds, win):
            values = ds.read(1, window=win)
            keep = (
                values != ds.nodata
                if ds.nodata is not None
                else np.ones(values.shape, dtype=bool)
            )
            return dict(
                window=[
                    int(win.col_off),
                    int(win.row_off),
                    int(win.width),
                    int(win.height),
                ],
                adler32=zlib.adler32(values.astype("<u2", copy=False).tobytes()),
                valid_pixels=int(keep.sum()),
                native_sum=float(values[keep].sum(dtype=np.float64)),
            )

        if profile == "block-order":
            windows.sort(
                key=lambda w: (
                    int(w.row_off) // by,
                    int(w.col_off) // bx,
                    w.row_off,
                    w.col_off,
                )
            )
        if profile in ("row-order", "block-order", "cache0"):
            rows = [read(source, win) for win in windows]
        elif profile == "reopen":
            rows = []
            for win in windows:
                with rasterio.open(asset["locator"]) as ds:
                    rows.append(read(ds, win))
        elif profile == "four-handles-serial":
            from contextlib import ExitStack

            with ExitStack() as stack:
                readers = [
                    stack.enter_context(rasterio.open(asset["locator"]))
                    for _ in range(4)
                ]
                rows = [read(readers[i % 4], win) for i, win in enumerate(windows)]
        elif profile == "four-handles-concurrent":

            def acquire(index):
                with rasterio.Env(**options), rasterio.open(asset["locator"]) as ds:
                    return [read(ds, win) for win in windows[index::4]]

            with ThreadPoolExecutor(max_workers=4) as pool:
                rows = [row for batch in pool.map(acquire, range(4)) for row in batch]
        else:
            raise ValueError(profile)
    return dict(
        rows=sorted(rows, key=lambda row: row["window"]),
        seconds=time.perf_counter() - started,
        metadata_seconds=metadata_seconds,
        profile=profile,
        gdal=rasterio.__gdal_version__,
    )


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "worker":
        print(json.dumps(worker(Path(sys.argv[2]), sys.argv[3])))
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reps", type=int, default=3)
    parser.add_argument(
        "--profiles",
        nargs="+",
        default=[
            "row-order",
            "cache0",
            "block-order",
            "reopen",
            "four-handles-serial",
            "four-handles-concurrent",
        ],
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=m.ROOT / "prototype-output/cog-efficiency/ablations",
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    manifest = json.loads((m.WORK / "sources.json").read_text())
    mirror = m.Mirror(m.WORK / "mirror", manifest["sources"])
    index_hash = mirror.verify_pages()
    server = m.start(mirror)
    source = manifest["sources"][0]
    request = m.request([source], server, trace.WINDOWS["several-large"])
    path = args.output / "request.json"
    path.write_text(json.dumps(request))
    layout = {"/0.tif": trace.tiff_layout(mirror.path("/0.tif"))}
    rows = []
    expected = None
    # Build the independent local pixel oracle outside all timed processes.
    local_request = copy.deepcopy(request)
    local_request["source"]["assets"][0]["locator"] = str(mirror.path("/0.tif"))
    local_path = args.output / "oracle-request.json"
    local_path.write_text(json.dumps(local_request))
    expected = worker(local_path, "row-order")["rows"]
    try:
        for rep in range(args.reps):
            for profile in args.profiles:
                server.reset()
                result = m.run(
                    [sys.executable, __file__, "worker", str(path), profile],
                    args.output / f"{rep}-{profile}.json",
                )
                events = copy.deepcopy(server.records)
                counts = copy.deepcopy(server.counts)
                assert result["exit_code"] == 0, result
                actual = result.pop("result")
                assert actual.pop("rows") == expected
                accounting = trace.analyze(events, layout, "several-large")
                rows.append(
                    dict(
                        rep=rep, **result, **actual, http=counts, accounting=accounting
                    )
                )
                print(
                    rep,
                    profile,
                    counts["body_bytes"],
                    accounting["totals"]["duplicate_response_bytes"],
                    round(result["wall_seconds"], 3),
                    flush=True,
                )
    finally:
        server.shutdown()
        server.server_close()
    full = dict(
        scope="one frozen real Sentinel asset, identical 64 window value checksums/counts/sums; diagnostic reader profiles, not matched full acquisitions",
        mirror_index_sha256=index_hash,
        harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        rows=rows,
    )
    (args.output / "report.json").write_text(json.dumps(full, indent=2) + "\n")
    compact = {
        **full,
        "rows": [{**row, "accounting": row["accounting"]["totals"]} for row in rows],
    }
    destination = m.ROOT / "docs/cog-efficiency/trace-ablations.json"
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(compact, indent=2) + "\n")


if __name__ == "__main__":
    main()
