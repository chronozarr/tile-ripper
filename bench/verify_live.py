"""Independent rasterio/GDAL reference for one already acquired small live subset."""

import argparse
import hashlib
import json
from pathlib import Path
import time
import numpy as np
import rasterio
from rasterio.features import geometry_mask
from rasterio.warp import transform_geom
from rasterio.windows import Window
from stac_workflow import dense_wgs


def verify(request_file, output):
    req = json.loads(Path(request_file).read_text())
    root = Path(output)
    manifest = json.loads((root / "manifest.json").read_text())
    refs = {s["asset_id"]: s for s in manifest["sources"]}
    checks = []
    with rasterio.Env(
        GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
        GDAL_NUM_THREADS="1",
        GDAL_CACHEMAX=32 * 1024**2,
        GDAL_INGESTED_BYTES_AT_OPEN="32768",
        GDAL_HTTP_MAX_RETRY="2",
        GDAL_HTTP_TIMEOUT="45",
    ):
        for entry in manifest["outputs"]:
            t = time.perf_counter()
            with (
                rasterio.open(refs[entry["asset_id"]]["href"]) as src,
                rasterio.open(root / entry["file"]) as out,
            ):
                x, y = (~src.transform) * (out.transform.c, out.transform.f)
                x, y = round(x), round(y)
                window = Window(x, y, out.width, out.height)
                expected = src.read(1, window=window)
                geom = transform_geom("EPSG:4326", src.crs, dense_wgs(req))
                mask = geometry_mask(
                    [geom], out_shape=out.shape, transform=out.transform, invert=True
                )
                expected[~mask] = src.nodata
                actual = out.read(1)
                assert np.array_equal(actual, expected), entry["file"]
                assert (
                    out.crs == src.crs
                    and out.transform == src.window_transform(window)
                    and out.nodata == src.nodata
                    and out.scales == src.scales
                    and out.offsets == src.offsets
                )
                checks.append(
                    dict(
                        file=entry["file"],
                        item_id=refs[entry["asset_id"]]["item_id"],
                        band=refs[entry["asset_id"]]["requested_band"],
                        pixels=int(actual.size),
                        inside_aoi=int(mask.sum()),
                        valid_pixels=int(np.count_nonzero(actual != src.nodata)),
                        sha256=hashlib.sha256(actual.tobytes()).hexdigest(),
                        bit_exact=True,
                        reference_seconds=time.perf_counter() - t,
                    )
                )
    result = dict(
        gdal=rasterio.__gdal_version__,
        rasterio=rasterio.__version__,
        checks=checks,
        scope="independent remote GDAL reads and rasterio/PROJ geometry masking; source files not downloaded whole",
    )
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("request")
    parser.add_argument("output")
    parser.add_argument("--report")
    args = parser.parse_args()
    result = verify(args.request, args.output)
    text = json.dumps(result, indent=2)
    if args.report:
        Path(args.report).write_text(text + "\n")
    print(text)
