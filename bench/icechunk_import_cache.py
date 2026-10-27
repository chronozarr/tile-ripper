#!/usr/bin/env python3
"""Meter maintained Icechunk encoded-range caching through Chronozarr's source reader.

Run with the Chronozarr checkout's geo+icechunk environment:
    ../chronozarr/.venv/bin/python bench/icechunk_import_cache.py \
      --chronozarr-source ../chronozarr/src \
      --output docs/icechunk/import-cache-receipt.json

Uses a temporary source-opening injection solely to route the maintained reader
through the bounded loopback meter and choose cache budgets. No source code or
upstream data is modified. Snapshot and selected coordinates stay identical.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import importlib.metadata
import json
from pathlib import Path
import sys
import time

import icechunk_qualification as q


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--chronozarr-source',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    sys.path.insert(0,str(args.chronozarr_source.resolve()))
    from chronozarr import IcechunkInput
    from chronozarr.convert import plan_conversion
    import chronozarr.icechunk as adapter
    original_open=adapter._open_repository
    root=f'{q.BASE}/noaa-gfs-analysis/v0.1.0.icechunk'
    output={
        'created_utc':datetime.now(timezone.utc).isoformat(),
        'repository':root, 'snapshot_id':q.DEFAULT_SNAPSHOTS['materialized'],
        'selection':{'time':[48072,48074],'latitude':[224,226],'longitude':[316,318]},
        'read_order':[0,1,0],
        'dependencies':{name:importlib.metadata.version(name) for name in ['icechunk','zarr','xarray','numpy']},
        'limits':['Loopback forwarding adds latency; response-body bytes exclude HTTP/TLS overhead.',
                  'The source-opening injection is test instrumentation, not a production endpoint rewrite.',
                  'Cache budget covers maintained Icechunk encoded chunk bytes, not metadata, decoded chunks or RSS.',
                  'One source window only; no archive-wide or device-wide performance promise.'],
        'cases':[],
    }
    results={}
    try:
        for cache_bytes in [0,32*1024**2,64*1024**2]:
            with q.proxy(32*1024**2) as (base,meter):
                config=q.icechunk.RepositoryConfig(caching=q.icechunk.CachingConfig(num_bytes_chunks=cache_bytes))
                repo=q.icechunk.Repository.open(q.icechunk.http_storage(base+'/repo/noaa-gfs-analysis/v0.1.0.icechunk'),config=config,authorize_virtual_chunk_access={})
                adapter._open_repository=lambda source:repo
                plan=plan_conversion(IcechunkInput(root,{'time':slice(48072,48074),'latitude':slice(224,226),'longitude':slice(316,318)},snapshot=q.DEFAULT_SNAPSHOTS['materialized']),variable='temperature_2m',sample=False)
                case={'encoded_chunk_cache_bytes':cache_bytes,'reads':[]}
                for trial,step in enumerate(output['read_order']):
                    mark=meter.mark(f'cache_{cache_bytes}:trial_{trial}:time_{step}')
                    start=time.perf_counter()
                    values=plan.source.read(step).data
                    case['reads'].append({'trial':trial,'time_index_in_selection':step,'wall_seconds':time.perf_counter()-start,'values':q.numeric_receipt(values),'transport':meter.since(mark)})
                    results[cache_bytes,trial]=values.copy()
                output['cases'].append(case)
    finally:
        adapter._open_repository=original_open
    output['exact_values_unchanged']=all(q.np.array_equal(results[0,i],results[c,i],equal_nan=True) for c in [32*1024**2,64*1024**2] for i in range(3))
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(output,indent=2,allow_nan=False)+'\n')
    print(json.dumps({'output':str(args.output),'exact_values_unchanged':output['exact_values_unchanged'],
                      'body_bytes':{str(c['encoded_chunk_cache_bytes']):[r['transport']['response_body_bytes'] for r in c['reads']] for c in output['cases']}}))


if __name__=='__main__':main()
