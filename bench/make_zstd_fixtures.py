#!/usr/bin/env python3
"""Recompress existing controlled samples; independently reopen and check bytes."""
from pathlib import Path
import hashlib
import json
import rasterio

root = Path(__file__).resolve().parents[1] / 'tests/data/native_cog'
rows = []
for name in ('hls_int16.tif', 'classification_uint8.tif', 'radar_float32.tif', 'radar_float32_nan_nodata.tif'):
    with rasterio.open(root / name) as source:
        values = source.read(1)
        options = source.profile
        options.update(interleave="pixel", tiled=True, blockxsize=source.block_shapes[0][1], blockysize=source.block_shapes[0][0], compress='zstd', zstd_level=3,
                       predictor=3 if values.dtype.kind == 'f' else 2)
        output = root / (Path(name).stem + '_zstd.tif')
        with rasterio.open(output, 'w', **options) as destination:
            destination.write(values, 1)
            destination.scales = source.scales
            destination.offsets = source.offsets
            destination.update_tags(**source.tags())
    with rasterio.open(output) as source:
        assert source.read(1).tobytes() == values.tobytes()
    rows.append({'source': name, 'fixture': output.name, 'sha256': hashlib.sha256(output.read_bytes()).hexdigest(),
                 'raw_sha256': hashlib.sha256(values.tobytes()).hexdigest(), 'predictor': options['predictor']})
(root / 'zstd-fixtures.json').write_text(json.dumps({
    'generator': 'rasterio ' + rasterio.__version__, 'gdal': rasterio.__gdal_version__,
    'operation': 'lossless ZSTD recompression of adjacent original fixtures; raw bytes independently reopened and compared before receipt',
    'fixtures': rows}, indent=2) + '\n')
