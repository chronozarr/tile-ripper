"""Deterministic COG and Zarr metadata fixtures; uses the existing locked study environment."""

import json
from pathlib import Path
import numpy as np
import rasterio
from rasterio.transform import from_origin
from rasterio.shutil import copy


def create(root):
    root = Path(root)
    root.mkdir(parents=True, exist_ok=True)
    source = root / "source.tif"
    if not source.exists():
        rng = np.random.default_rng(20261018)
        data = rng.integers(0, 30000, (2048, 2048), dtype=np.uint16)
        data[100:120, 100:120] = 0
        with rasterio.open(
            root / "staging.tif",
            "w",
            driver="GTiff",
            width=2048,
            height=2048,
            count=1,
            dtype="uint16",
            crs="EPSG:32617",
            transform=from_origin(600000, 4000020, 10, 10),
            nodata=0,
            tiled=True,
            blockxsize=256,
            blockysize=256,
        ) as ds:
            ds.write(data, 1)
            ds.scales = (0.0001,)
            ds.offsets = (-0.1,)
        copy(
            root / "staging.tif",
            source,
            driver="COG",
            BLOCKSIZE=256,
            COMPRESS="DEFLATE",
            PREDICTOR=2,
        )
        (root / "staging.tif").unlink()
    requests = {}
    for name, win in {
        "small": (100, 100, 160, 160),
        "large": (240, 240, 1400, 1400),
    }.items():
        x, y, w, h = win
        req = dict(
            source=dict(
                kind="assets",
                assets=[
                    dict(
                        id="fixture",
                        locator=str(source.resolve()),
                        format="cog",
                        datetime="2025-06-01T00:00:00Z",
                    )
                ],
            ),
            spatial=dict(
                kind="bounds",
                crs="EPSG:32617",
                bbox=[
                    600000 + x * 10,
                    4000020 - (y + h) * 10,
                    600000 + (x + w) * 10,
                    4000020 - y * 10,
                ],
            ),
            time=dict(
                kind="interval",
                start="2025-01-01T00:00:00Z",
                end="2025-12-31T23:59:59Z",
            ),
            variables=[dict(kind="band", index=1)],
            grid=dict(kind="native"),
            sink=dict(
                kind="geotiff_directory", path=str((root / f"{name}-output").resolve())
            ),
            budget=dict(buffer_bytes=524288, chunk_edge=512, max_nodes=10000),
        )
        (root / f"{name}.json").write_text(json.dumps(req, indent=2))
        requests[name] = (req, win)
    zarr = root / "array.zarr"
    zarr.mkdir(exist_ok=True)
    (zarr / ".zarray").write_text(
        json.dumps(
            dict(
                zarr_format=2,
                shape=[2048, 2048],
                chunks=[256, 256],
                dtype="<u2",
                compressor=None,
                filters=None,
                fill_value=0,
                order="C",
            )
        )
    )
    (zarr / ".zattrs").write_text(
        json.dumps(
            dict(
                tile_ripper_grid=dict(
                    crs="EPSG:32617", transform=[600000, 10, 0, 4000020, 0, -10]
                )
            )
        )
    )
    req = json.loads(json.dumps(requests["small"][0]))
    req["source"]["assets"][0].update(locator=str(zarr.resolve()), format="zarr_v2")
    req["variables"] = [dict(kind="dataset", path="/")]
    (root / "zarr.json").write_text(json.dumps(req, indent=2))
    return requests


if __name__ == "__main__":
    import sys

    create(sys.argv[1] if len(sys.argv) > 1 else "prototype-output/fixtures")
