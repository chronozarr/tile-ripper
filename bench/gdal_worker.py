"""Independent tuned rasterio/GDAL materialization baseline, same chunk shape and output."""

import json
import sys
import time
from pathlib import Path
import rasterio
from rasterio.windows import Window

req = json.loads(Path(sys.argv[1]).read_text())
root = Path(sys.argv[2])
root.mkdir()
asset = req["source"]["assets"][0]
bbox = req["spatial"]["bbox"]
t = time.perf_counter()
read_s = write_s = 0
with rasterio.Env(
    GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
    GDAL_INGESTED_BYTES_AT_OPEN="32768",
    GDAL_NUM_THREADS="1",
    GDAL_CACHEMAX=32 * 1024**2,
    GDAL_HTTP_MAX_RETRY="3",
    GDAL_HTTP_TIMEOUT="45",
):
    with rasterio.open(asset["locator"]) as ds:
        import math

        t0 = ds.transform
        x0 = max(0, math.floor((bbox[0] - t0.c) / t0.a))
        x1 = min(ds.width, math.ceil((bbox[2] - t0.c) / t0.a))
        y0 = max(0, math.floor((bbox[3] - t0.f) / t0.e))
        y1 = min(ds.height, math.ceil((bbox[1] - t0.f) / t0.e))
        edge = req["budget"]["chunk_edge"]
        for y in range(y0, y1, edge):
            for x in range(x0, x1, edge):
                w = Window(x, y, min(edge, x1 - x), min(edge, y1 - y))
                q = time.perf_counter()
                pixels = ds.read(1, window=w)
                read_s += time.perf_counter() - q
                q = time.perf_counter()
                with rasterio.open(
                    root / f"{x}-{y}.tif",
                    "w",
                    driver="GTiff",
                    width=w.width,
                    height=w.height,
                    count=1,
                    dtype="uint16",
                    crs=ds.crs,
                    transform=ds.window_transform(w),
                    nodata=ds.nodata,
                ) as out:
                    out.write(pixels, 1)
                    out.scales = ds.scales
                    out.offsets = ds.offsets
                write_s += time.perf_counter() - q
print(
    json.dumps(
        dict(
            read_decode_seconds=read_s,
            sink_seconds=write_s,
            total_worker_seconds=time.perf_counter() - t,
            gdal=rasterio.__gdal_version__,
        )
    )
)
