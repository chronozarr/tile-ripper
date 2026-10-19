"""One unfamiliar real STAC request, independently checked through rasterio/GDAL.
Run in the established study/experiments/.venv Python environment.
"""

import json
from pathlib import Path
import time
import subprocess
import numpy as np
import rasterio
from rasterio.features import geometry_mask
from rasterio.warp import transform_geom
from rasterio.windows import Window

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/external-usability"


def main():
    output = WORK / "sentinel-timed"
    args = [
        str(ROOT / "target/release/tile-ripper"),
        "fetch",
        "--source",
        "https://earth-search.aws.element84.com/v1",
        "--format",
        "stac",
        "--collection",
        "sentinel-2-c1-l2a",
        "--bbox",
        "-79.04,35.92,-79.038,35.922",
        "--start",
        "2025-07-28T00:00:00Z",
        "--end",
        "2025-07-28T23:59:59Z",
        "--variables",
        "B03",
        "--output",
        str(output),
    ]
    t = time.perf_counter()
    result = subprocess.run(args, capture_output=True, text=True, check=True)
    elapsed = time.perf_counter() - t
    manifest = json.loads(result.stdout)
    # Densify rectangle in geographic coordinates independently; no planner geometry reuse.
    points = [
        (-79.04, 35.92),
        (-79.038, 35.92),
        (-79.038, 35.922),
        (-79.04, 35.922),
        (-79.04, 35.92),
    ]
    ring = []
    for p, q in zip(points, points[1:]):
        for frac in np.linspace(0, 1, 101)[:-1]:
            ring.append([p[0] + frac * (q[0] - p[0]), p[1] + frac * (q[1] - p[1])])
    ring.append(ring[0])
    checks = []
    t = time.perf_counter()
    with rasterio.Env(
        GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
        GDAL_NUM_THREADS="1",
        GDAL_CACHEMAX=32 * 1024 * 1024,
        GDAL_INGESTED_BYTES_AT_OPEN="32768",
    ):
        for record in manifest["outputs"]:
            source = next(
                v for v in manifest["sources"] if v["asset_id"] == record["asset_id"]
            )
            with (
                rasterio.open(source["href"]) as src,
                rasterio.open(output / record["file"]) as dst,
            ):
                col, row = (~src.transform) * (dst.transform.c, dst.transform.f)
                win = Window(round(col), round(row), dst.width, dst.height)
                expected = src.read(1, window=win)
                geometry = transform_geom(
                    "EPSG:4326", src.crs, {"type": "Polygon", "coordinates": [ring]}
                )
                inside = geometry_mask(
                    [geometry],
                    out_shape=dst.shape,
                    transform=dst.transform,
                    invert=True,
                )
                expected[~inside] = src.nodata
                np.testing.assert_array_equal(expected, dst.read(1))
                assert dst.transform == src.window_transform(win)
                assert (dst.crs, dst.nodata, dst.scales, dst.offsets) == (
                    src.crs,
                    src.nodata,
                    src.scales,
                    src.offsets,
                )
                # Ordinary alternative output: same masked native values and encoding.
                profile = dst.profile.copy()
                with rasterio.open(
                    WORK / "sentinel-reference.tif", "w", **profile
                ) as ref:
                    ref.write(expected, 1)
                    ref.scales = src.scales
                    ref.offsets = src.offsets
                checks.append(
                    dict(
                        shape=list(dst.shape),
                        cells=expected.size,
                        valid=int(np.count_nonzero(expected)),
                        crs=str(dst.crs),
                        dtype=str(expected.dtype),
                    )
                )
    baseline = time.perf_counter() - t
    result = dict(
        command=args,
        wall_seconds=elapsed,
        validation=checks,
        observed=manifest["observed"],
        sources=manifest["sources"],
        reference_read_mask_write_seconds=baseline,
        notes=[
            "One bounded live request; primary agent script, not independent newcomer trial.",
            "Reference given selected source/window from output; independently reads original COG and constructs AOI mask.",
            "Reference timing excludes STAC discovery, dependency validation and provenance; not equivalent end-to-end performance.",
            "GDAL HTTP request/byte totals unavailable; no fabricated observed transfer counts.",
        ],
    )
    (ROOT / "docs/external-usability-stac.json").write_text(
        json.dumps(result, indent=2) + "\n"
    )
    print(json.dumps(checks))


if __name__ == "__main__":
    main()
