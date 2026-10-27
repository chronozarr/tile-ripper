#!/usr/bin/env python3
"""Requalify a small northern window without replacing earlier receipts."""
from pathlib import Path
import argparse
import hashlib
import importlib.util
import json
from datetime import datetime, timezone
import numpy as np
import rasterio
import zarr
from rasterio.transform import Affine
from rasterio.warp import transform

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--netrc', type=Path, required=True)
parser.add_argument('--baseline', type=Path, required=True, help='retained completed northern Zarr from prior qualification')
parser.add_argument('--replay-output', type=Path, help='reuse an already verified northern Zarr and stream the current binary against it')
args = parser.parse_args()
spec = importlib.util.spec_from_file_location('annual_reference', ROOT / 'bench/opera_dist_ann_qualification.py')
h = importlib.util.module_from_spec(spec); spec.loader.exec_module(h)
h.WORK = ROOT / 'prototype-output/dist-ann-southern/north-regression'
request = json.loads((ROOT / 'examples/opera-dist-ann-hls.json').read_text())
headers = json.loads(h.SOURCE_RECEIPT.read_text())['sources']
baseline = zarr.open_group(str(args.baseline), mode='r')
manifest = json.loads((args.baseline / 'manifest.json').read_text())
status = next(row for row in manifest['outputs'] if row['asset_id'] == 'p000_VEG-DIST-STATUS')
group = baseline[status['group']]
hits = np.argwhere(np.asarray(group['quality_dist_ann_veg_confirmed'][0]) != 0)
if not len(hits): raise RuntimeError('baseline lacks a confirmed status window')
y, x = map(int, hits[len(hits)//2]); x += status['window']['x']; y += status['window']['y']
affine = Affine(*headers['VEG-DIST-STATUS']['transform'])
xs = [affine.c + affine.a*(x-8), affine.c + affine.a*(x+9)]
ys = [affine.f + affine.e*(y-8), affine.f + affine.e*(y+9)]
lon, lat = transform('EPSG:32617', 'EPSG:4326', [xs[0],xs[1],xs[0],xs[1]], [ys[0],ys[0],ys[1],ys[1]])
request['aoi']['bbox'] = [min(lon),min(lat),max(lon),max(lat)]
request['buffer_bytes'] = 128*1024*1024; request['chunk_edge'] = 64
stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')
output = args.replay_output or h.WORK / f'north-{stamp}.zarr'
result = h.inspect_case(args.binary.resolve(), args.netrc, request, output, 'north-'+stamp, h.private_credentials(args.netrc), headers, replay=args.replay_output is not None)
current = json.loads((output / 'manifest.json').read_text())
previous = {row['asset_id']:row for row in manifest['sources']}
for source in current['sources']:
    assert source['object_version'] == previous[source['asset_id']]['object_version']
    assert source['object_identities']['native_cog'] == previous[source['asset_id']]['object_identities']['native_cog']
for row in current['outputs']:
    attrs = json.loads((output / row['group'] / 'spatial_ref/.zattrs').read_text())
    assert attrs['epsg_code'] == 'EPSG:32617' and attrs['false_northing'] == 0
# Verify new prepared provenance as well as old/new pixel equality.
streams = sorted((h.WORK / 'runs').glob('*stream.stdout'), key=lambda p: p.stat().st_mtime)
prepared = None
for line in streams[-1].open():
    row = json.loads(line)
    if row.get('event') == 'prepared': prepared = row['provenance']; break
assert prepared is not None
for source in prepared['sources']:
    assert 'native_coordinate_convention' not in source
    assert source['object_identities']['native_cog'] == previous[source['asset_id']]['object_identities']['native_cog']
    band = source['requested_band']
    if band.endswith('LAST-DATE'):
        temporal = source['annual_last_date_temporal_provenance']
        assert temporal['tiff_update_timestamp'] == headers[band]['tags']['Update_Date']
        assert temporal['catalog_coverage_interval'] == request['observations'][0]['properties']['temporal_extent']
receipt = {'status':'passed','scope':'small northern all21 live regression against independent source windows and retained prior metadata; prior receipts untouched',
           'binary_sha256':hashlib.sha256(args.binary.read_bytes()).hexdigest(),
           'baseline_manifest_sha256':hashlib.sha256((args.baseline/'manifest.json').read_bytes()).hexdigest(),
           'original_native_tiff_metadata_identical':True,'northern_sources_have_no_extended_selection_marker':True,'separate_catalog_and_tiff_update_times_preserved':True,'delivery_mode':'stream replay against retained, independently verified Zarr' if args.replay_output else 'fresh fetch and stream','case':result}
h.dump(ROOT / 'docs/products/dist-ann-southern/north-regression.json',receipt)
print(json.dumps({'status':'passed','layers':len(result['layers']),'chunks':result['stream']['chunks']}))
