#!/usr/bin/env python3
"""Controlled CRS fixtures; never a replacement for real product qualification."""
from pathlib import Path
import hashlib
import json
import numpy as np
import rasterio
from rasterio.transform import Affine
from pyproj import Transformer

root = Path(__file__).resolve().parents[1] / 'tests/data/dist_ann_southern'
root.mkdir(parents=True, exist_ok=True)
values = (np.arange(32 * 32).reshape(32, 32) % 251).astype('u1')
values[0, 0] = 255
rows = []
for name, epsg, northing in [('extended_north', 32620, -1099980.), ('conventional_south', 32720, 8900020.)]:
    path = root / (name + '.tif')
    affine = Affine(30., 0., 799980., 0., -30., northing)
    with rasterio.open(path, 'w', driver='GTiff', width=32, height=32, count=1,
                       dtype='uint8', crs=f'EPSG:{epsg}', transform=affine, nodata=255,
                       tiled=True, blockxsize=16, blockysize=16, interleave='pixel',
                       compress='deflate', predictor=2) as ds:
        ds.write(values, 1)
    with rasterio.open(path) as ds:
        assert ds.read(1).tobytes() == values.tobytes()
        assert ds.transform == affine and ds.crs.to_epsg() == epsg
    rows.append({'file': path.name, 'epsg': epsg, 'affine_gdal': list(affine.to_gdal()),
                 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
xs = [800130., 800430., 800130., 800430.]
ys = [-1100130., -1100130., -1100430., -1100430.]
lon, lat = Transformer.from_crs(32620, 4326, always_xy=True).transform(xs, ys)
box = [min(lon), min(lat), max(lon), max(lat)]
px, py = Transformer.from_crs(4326, 32620, always_xy=True).transform(lon, lat)
sx, sy = Transformer.from_crs(4326, 32720, always_xy=True).transform(lon, lat)
assert np.allclose(px, sx, rtol=0, atol=1e-7)
assert np.allclose(np.asarray(py) + 10000000., sy, rtol=0, atol=1e-7)
receipt = {'generator': 'rasterio ' + rasterio.__version__, 'gdal': rasterio.__gdal_version__,
           'scope': 'synthetic generic-admission/coordinate regression fixtures, not annual product qualification',
           'shape': [32,32], 'fixtures': rows, 'geographic_bbox': box,
           'reference_lonlat': list(map(list, zip(lon,lat))),
           'reference_extended_xy': list(map(list, zip(px,py))),
           'reference_conventional_xy': list(map(list, zip(sx,sy))),
           'raw_sha256': hashlib.sha256(values.tobytes()).hexdigest()}
(root / 'reference.json').write_text(json.dumps(receipt, indent=2) + '\n')
