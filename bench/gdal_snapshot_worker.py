"""Tuned independent native GDAL workflow with conditional reads and commit checks.
Fresh-process worker for real_multiscene.py; no Tile-Ripper imports or internals.
"""

from concurrent.futures import ThreadPoolExecutor
import json
import math
from pathlib import Path
import shutil
import sys
import threading
import time
import zlib

import numpy as np
import rasterio
from rasterio.windows import Window
import requests


def acquire(request, workers, output=None):
    started = time.perf_counter()
    first = []
    lock = threading.Lock()
    head_slots = threading.BoundedSemaphore(4)
    local = threading.local()
    assets = request["source"]["assets"]
    bbox = request["spatial"]["bbox"]
    if output is not None:
        output.mkdir(parents=True, exist_ok=False)

    def validate(asset):
        if not hasattr(local, "session"):
            local.session = requests.Session()
        with head_slots:
            response = local.session.head(
                asset["locator"],
                headers={"If-Match": asset["etag"]},
                timeout=45,
                allow_redirects=False,
            )
        response.raise_for_status()
        if response.status_code != 200 or response.headers.get("ETag") != asset["etag"]:
            raise ValueError("GDAL source version conflict")
        return int(response.headers["Content-Length"])

    def read_asset(asset):
        chunks = []
        read_seconds = write_seconds = plan_seconds = 0.0
        options = dict(
            GDAL_CACHEMAX=32 * 2**20,
            GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
            GDAL_INGESTED_BYTES_AT_OPEN="32768",
            GDAL_NUM_THREADS="1",
            GDAL_HTTP_MULTIRANGE="SERIAL",
            GDAL_HTTP_MAX_RETRY="3",
            GDAL_HTTP_TIMEOUT="45",
            GDAL_HTTP_HEADERS=f"If-Match: {asset['etag']}\r\n",
        )
        opened = time.perf_counter()
        with rasterio.Env(**options), rasterio.open(asset["locator"]) as source:
            metadata_seconds = time.perf_counter() - opened
            t = time.perf_counter()
            c0, r0 = (~source.transform) * (bbox[0], bbox[3])
            c1, r1 = (~source.transform) * (bbox[2], bbox[1])
            left, top = max(0, math.floor(c0)), max(0, math.floor(r0))
            right, bottom = (
                min(source.width, math.ceil(c1)),
                min(source.height, math.ceil(r1)),
            )
            plan_seconds += time.perf_counter() - t
            validate(asset)  # after layout inspection, before the first sample
            edge = request["budget"]["chunk_edge"]
            for iy, y in enumerate(range(top, bottom, edge)):
                for ix, x in enumerate(range(left, right, edge)):
                    window = Window(x, y, min(edge, right - x), min(edge, bottom - y))
                    t = time.perf_counter()
                    values = source.read(1, window=window)
                    read_seconds += time.perf_counter() - t
                    transform = source.window_transform(window)
                    if output is None:
                        with lock:
                            if not first:
                                first.append(time.perf_counter() - started)
                        valid = (
                            values != source.nodata
                            if source.nodata is not None
                            else np.ones(values.shape, dtype=bool)
                        )
                        chunks.append(
                            dict(
                                type="provisional_chunk",
                                asset_id=asset["id"],
                                variable=dict(kind="band", index=1),
                                datetime=asset["datetime"],
                                indices=[],
                                window=dict(
                                    x=x,
                                    y=y,
                                    width=int(window.width),
                                    height=int(window.height),
                                ),
                                transform=list(transform.to_gdal()),
                                crs=str(source.crs),
                                coordinates=None,
                                dtype="uint16",
                                nodata=source.nodata,
                                scale=source.scales[0],
                                offset=source.offsets[0],
                                valid_pixels=int(valid.sum()),
                                native_sum=float(values[valid].sum(dtype=np.float64)),
                                adler32_le=zlib.adler32(
                                    values.astype("<u2", copy=False).tobytes()
                                ),
                            )
                        )
                    else:
                        t = time.perf_counter()
                        name = f"{asset['id']}-chunk-{iy}-{ix}.tif"
                        with rasterio.open(
                            output / name,
                            "w",
                            driver="GTiff",
                            count=1,
                            width=values.shape[1],
                            height=values.shape[0],
                            dtype="uint16",
                            crs=source.crs,
                            transform=transform,
                            nodata=source.nodata,
                        ) as dest:
                            dest.write(values, 1)
                            dest.scales, dest.offsets = source.scales, source.offsets
                        write_seconds += time.perf_counter() - t
                        with lock:
                            if not first:
                                first.append(time.perf_counter() - started)
                        chunks.append(
                            dict(
                                asset_id=asset["id"],
                                timestamps=[asset["datetime"]],
                                file=name,
                            )
                        )
        return chunks, metadata_seconds, plan_seconds, read_seconds, write_seconds

    try:
        with ThreadPoolExecutor(max_workers=workers) as pool:
            t = time.perf_counter()
            pinned_sizes = list(pool.map(validate, assets))
            initial_validation_seconds = time.perf_counter() - t
            batches = list(pool.map(read_asset, assets))
            t = time.perf_counter()
            final_sizes = list(pool.map(validate, assets))
            final_validation_seconds = time.perf_counter() - t
            if final_sizes != pinned_sizes:
                raise ValueError("GDAL source length changed")
        chunks = [row for batch, *_ in batches for row in batch]
        result = dict(
            seconds=time.perf_counter() - started,
            initial_validation_seconds=initial_validation_seconds,
            final_validation_seconds=final_validation_seconds,
            metadata_worker_seconds=sum(batch[1] for batch in batches),
            planning_worker_seconds=sum(batch[2] for batch in batches),
            read_worker_seconds=sum(batch[3] for batch in batches),
            write_seconds=sum(batch[4] for batch in batches),
            source_validation="pinned strong ETag/size; conditional GDAL reads; initial, pre-read and final version checks",
        )
        if output is None:
            result["first_chunk_seconds"] = first[0] if first else None
            result["rows"] = (
                [dict(type="provisional_start", provenance=dict(sources=assets))]
                + chunks
                + [
                    dict(
                        type="complete",
                        chunks=len(chunks),
                        valid_pixels=sum(c["valid_pixels"] for c in chunks),
                        native_sum=sum(c["native_sum"] for c in chunks),
                    )
                ]
            )
        else:
            result["usable_ttfc_seconds"] = first[0] if first else None
            result["outputs"] = chunks
            # Only the final manifest authorizes committing provisional TIFFs.
            (output / "manifest.json").write_text(
                json.dumps(dict(status="complete", outputs=chunks))
            )
        return result
    except BaseException:
        if output is not None:
            shutil.rmtree(output, ignore_errors=True)
        raise


if __name__ == "__main__":
    req = json.loads(Path(sys.argv[1]).read_text())
    out = Path(sys.argv[3]) if len(sys.argv) > 3 else None
    print(json.dumps(acquire(req, int(sys.argv[2]), out)))
