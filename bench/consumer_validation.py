"""External public Rust consumer vs independent GDAL/xarray, including late faults.
All remote traffic is loopback. No output imagery or generated rows are committed.
"""
from __future__ import annotations
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import zlib

import numpy as np
import rasterio
from rasterio.transform import from_origin
from rasterio.windows import Window
import xarray as xr

import acquisition_engine as engine
import cf_zarr
import remote_zarr
import zarr_workflow
import stac_workflow

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get("TILE_RIPPER_STATS_BINARY", str(ROOT / "target/release/tile-ripper-native-stats")))
WORK = ROOT / "prototype-output/production-streaming/consumer"


def invoke(request, path, args=(), mutate=None):
    path.write_text(json.dumps(request))
    rows = []
    env = {**os.environ, "GDAL_CACHEMAX": "32", "GDAL_DISABLE_READDIR_ON_OPEN": "EMPTY_DIR",
           "GDAL_INGESTED_BYTES_AT_OPEN": "32768"}
    with tempfile.TemporaryFile() as errors:
        start = time.monotonic()
        proc = subprocess.Popen([str(BIN), str(path), *args], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=errors, text=True, env=env)
        timer = threading.Timer(60, proc.kill)
        timer.start()
        try:
            for line in proc.stdout:
                row = json.loads(line)
                rows.append(row)
                if row['type'] == 'provisional_chunk' and mutate is not None:
                    mutate(row)
                    mutate = None
                    proc.stdin.write('continue\n');proc.stdin.flush()
            _, status, usage = os.wait4(proc.pid, 0)
            proc.returncode = os.waitstatus_to_exitcode(status)
        finally:
            timer.cancel()
        errors.seek(0)
        stderr = errors.read().decode(errors='replace')
    return dict(code=proc.returncode, rows=rows, stderr=stderr, seconds=time.monotonic()-start,
                peak_rss_bytes=usage.ru_maxrss*(1 if sys.platform=='darwin' else 1024))


def verify(rows, sources):
    assert rows[0]['type']=='provisional_start' and rows[-1]['type']=='complete'
    assert all(r['type']=='provisional_chunk' for r in rows[1:-1])
    provenance=rows[0]['provenance'];assert provenance['sources']
    chunks=rows[1:-1];n=0;total=0.;seen=set();coverage={}
    for c in chunks:
        asset=sources[c['asset_id']];w=c['window'];win=Window(w['x'],w['y'],w['width'],w['height'])
        identity=(c['asset_id'],json.dumps(c['variable'],sort_keys=True),tuple(c['indices']),*w.values())
        assert identity not in seen;seen.add(identity)
        source=asset['local']
        if asset['format']=='cog':
            with rasterio.open(source) as ds:
                a=ds.read(1,window=win);assert c['dtype']=='uint16'
                assert c['transform']==list(ds.window_transform(win).to_gdal())
                assert rasterio.crs.CRS.from_string(c['crs'])==ds.crs
                assert c['nodata']==ds.nodata
                assert c['scale']==ds.scales[0] and c['offset']==ds.offsets[0]
                assert c['datetime']==asset['datetime']
        else:
            with xr.open_zarr(source,consolidated=False,decode_cf=False,mask_and_scale=False) as ds:
                v=c['variable']['path'];dims=ds[v].dims
                y=next(d for d in dims if d in ('y','lat','latitude'));x=next(d for d in dims if d in ('x','lon','longitude'))
                a=ds[v].isel(time=c['indices'][0],**{y:slice(w['y'],w['y']+w['height']),x:slice(w['x'],w['x']+w['width'])}).transpose(y,x).values
                assert c['dtype']==str(a.dtype)
                np.testing.assert_array_equal(c['coordinates'][0],ds[x].values[w['x']:w['x']+w['width']])
                np.testing.assert_array_equal(c['coordinates'][1],ds[y].values[w['y']:w['y']+w['height']])
                if c['dtype']=='float32':assert c['nodata_is_nan']
        key=(c['asset_id'],json.dumps(c['variable'],sort_keys=True),tuple(c['indices']))
        coverage.setdefault(key,[]).append((w['x'],w['y'],w['width'],w['height']))
        expected=zlib.adler32(a.astype(a.dtype.newbyteorder('<'),copy=False).tobytes())
        assert c['adler32_le']==expected,(identity,c['adler32_le'],expected)
        mask=~np.isnan(a)
        if c['nodata'] is not None:mask &= a!=c['nodata']
        count=int(mask.sum());s=float(a[mask].sum(dtype=np.float64))
        assert c['valid_pixels']==count and c['native_sum']==s
        n+=count;total+=s
    assert rows[-1]['chunks']==len(chunks) and rows[-1]['valid_pixels']==n and rows[-1]['native_sum']==total
    # Every observation is a complete rectangle with no overlapping deliveries.
    points=0
    for rectangles in coverage.values():
        x0=min(r[0] for r in rectangles);y0=min(r[1] for r in rectangles)
        x1=max(r[0]+r[2] for r in rectangles);y1=max(r[1]+r[3] for r in rectangles)
        area=sum(w*h for _,_,w,h in rectangles)
        assert area==(x1-x0)*(y1-y0)
        for i,(x,y,w,h) in enumerate(rectangles):
            for xx,yy,ww,hh in rectangles[:i]:
                assert min(x+w,xx+ww)<=max(x,xx) or min(y+h,yy+hh)<=max(y,yy)
        points+=area
    return dict(chunks=len(chunks),valid_pixels=n,native_sum=total,values=points,observations=len(coverage),coverage=coverage)


def main():
    WORK.mkdir(parents=True,exist_ok=True)
    cogdir=WORK/'cog';cogdir.mkdir(exist_ok=True)
    objects={};assets=[];local={}
    for i in range(2):
        p=cogdir/f'{i}.tif';values=(np.arange(64*64).reshape(64,64)+i*100).astype('uint16');values[:4,:4]=0
        with rasterio.open(p,'w',driver='GTiff',width=64,height=64,count=1,dtype='uint16',
                           crs='EPSG:32617',transform=from_origin(600000,4000000,10,10),nodata=0,
                           tiled=True,blockxsize=32,blockysize=32) as ds:
            ds.write(values,1);ds.scales=(0.0001,);ds.offsets=(-0.1,)
        objects[f'/{i}.tif']=p
        a=dict(id=f'cog-{i}',locator='',format='cog',datetime=f'2025-01-0{i+1}T00:00:00Z')
        assets.append(a);local[a['id']]={**a,'local':p}
    server=engine.Server(('127.0.0.1',0),engine.Handler);server.configure(objects,0,None)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    request=dict(source=dict(kind='assets',assets=assets),spatial=dict(kind='bounds',crs='EPSG:32617',bbox=[600000,3999360,600640,4000000]),
                 variables=[dict(kind='band',index=1)],grid=dict(kind='native'),sink=dict(kind='callback'),
                 budget=dict(buffer_bytes=2**20,max_nodes=1000,chunk_edge=64))
    for i,a in enumerate(assets):a['locator']=f'http://127.0.0.1:{server.server_port}/{i}.tif'
    results=[]
    try:
        good=invoke(request,WORK/'cog.json');assert good['code']==0,good
        checked=verify(good['rows'],local);assert checked['chunks']==2 and checked['valid_pixels']==2*(4096-16)
        results.append(dict(case='two-remote-cogs',passed=True,chunks=checked['chunks'],peak_rss_bytes=good['peak_rss_bytes']))
        cancel=invoke(request,WORK/'cancel.json',['--cancel-after','1'])
        assert cancel['code'] and any(r['type']=='provisional_chunk' for r in cancel['rows'])
        assert cancel['rows'][-1]['type']=='aborted' and not any(r['type']=='complete' for r in cancel['rows'])
        results.append(dict(case='cancel-after-delivery',passed=True))
        # Only asset 0's read has already completed; its version changes while
        # asset 1 is blocked at delivery. Final checks, not a later asset-0 GET,
        # must reject the acquisition.
        def mutate(_):
            p=objects['/0.tif'];blob=bytearray(p.read_bytes());blob[-1]^=1;p.write_bytes(blob)
            server.etags['/0.tif']='"changed-after-delivery"'
        failed=invoke(request,WORK/'mutation.json',['--pause-after','1'],mutate)
        assert failed['code'] and sum(r['type']=='provisional_chunk' for r in failed['rows'])==2,failed
        assert failed['rows'][-1]['type']=='aborted' and 'version conflict' in failed['rows'][-1]['error']
        results.append(dict(case='already-read-cog-mutated-after-delivery',passed=True,provisional_chunks=2))
        (WORK/'cog-mutation-events.json').write_text(json.dumps(failed,indent=2))
    finally:server.shutdown();server.server_close()
    catalog=stac_workflow.Server(('127.0.0.1',0),stac_workflow.Handler)
    catalog.root=WORK/'stac-fixtures'
    catalog.files,catalog.features=stac_workflow.create(catalog.root,catalog.server_port)
    catalog.queries=[];catalog.seen_conditions=[];catalog.reset()
    threading.Thread(target=catalog.serve_forever,daemon=True).start()
    try:
        legacy=stac_workflow.request(catalog.server_port,WORK/'unused')
        r=dict(source=dict(kind='stac',endpoint=legacy['endpoint'],collection=legacy['collection'],limits=dict(max_items=8,page_size=1,max_assets=8,max_output_pixels=1000000)),
               spatial=legacy['aoi'],time=dict(kind='interval',start=legacy['start'],end=legacy['end']),
               variables=[dict(kind='named',name=b) for b in legacy['bands']],grid=dict(kind='native'),sink=dict(kind='callback'),
               budget=dict(buffer_bytes=1048576,max_nodes=100000,chunk_edge=64))
        good=invoke(r,WORK/'stac.json');assert good['code']==0,good
        assert good['rows'][-1]['type']=='complete'
        assert 'NEVER_SAVE_THIS' not in json.dumps(good['rows'])
        catalog.current_etag='"fixed-fixture"'
        def mutate_catalog(_):catalog.current_etag='"catalog-changed-after-delivery"'
        bad=invoke(r,WORK/'stac-mutation.json',['--pause-after','1'],mutate_catalog)
        assert bad['code'] and bad['rows'][-1]['type']=='aborted'
        assert any(c['type']=='provisional_chunk' for c in bad['rows'])
        results.append(dict(case='catalog-consumer-and-post-delivery-mutation',passed=True,peak_rss_bytes=good['peak_rss_bytes']))
    finally:catalog.shutdown();catalog.server_close()
    zarr_workflow.WORK=WORK/'uint16-fixture';r=json.loads(zarr_workflow.fixture().read_text());source=Path(r['source']['assets'][0]['locator'])
    servers=[remote_zarr.start(source),remote_zarr.start(source)]
    try:
        r['sink']={'kind':'callback'};r['source']['assets']=[dict(id=f'cube-{i}',locator=s.url,format='zarr_v2') for i,s in enumerate(servers)]
        good=invoke(r,WORK/'uint16.json');assert good['code']==0,good
        refs={a['id']:{**a,'local':source} for a in r['source']['assets']};checked=verify(good['rows'],refs)
        # Independent xarray rectangle/time selection determines expected counts.
        with xr.open_zarr(source,consolidated=False,decode_cf=False) as src:
            ds=zarr_workflow.expected(r,src)
            expected=sum(int(np.count_nonzero(ds[v].values)) for v in ['red','nir'])
        assert checked['valid_pixels']==2*expected,(checked['valid_pixels'],expected)
        assert checked['values']==2*sum(ds[v].size for v in ['red','nir'])
        assert checked['observations']==2*len(r['variables'])*ds.sizes['time']
        results.append(dict(case='two-remote-uint16-zarr',passed=True,chunks=checked['chunks'],peak_rss_bytes=good['peak_rss_bytes']))
        def mutate_zarr(_):
            p=source/'.zattrs';attrs=json.loads(p.read_text());attrs['title']='changed after delivery';p.write_text(json.dumps(attrs))
        bad=invoke(r,WORK/'uint16-mutation.json',['--pause-after','1'],mutate_zarr)
        assert bad['code'] and bad['rows'][-1]['type']=='aborted' and any(c['type']=='provisional_chunk' for c in bad['rows']),bad
        results.append(dict(case='zarr-mutated-after-delivery',passed=True))
    finally:
        for s in servers:s.shutdown();s.server_close()
    float_source=WORK/'float.zarr';g=cf_zarr.fixture(float_source,reverse_x=True,reverse_y=True,swap=True)
    # Preserve a noncanonical NaN payload and negative zero, not merely numerical equality.
    v=g['2m_temperature'][:];bits=v.view(np.uint32);bits[:,3,3]=0x7fc01234;bits[:,4,2]=0x80000000
    g['2m_temperature'][:]=v
    s=remote_zarr.start(float_source)
    try:
        r=cf_zarr.request(float_source,WORK/'unused');r['source']['assets'][0]['locator']=s.url;r['sink']={'kind':'callback'}
        good=invoke(r,WORK/'float.json');assert good['code']==0,good
        checked=verify(good['rows'],{r['source']['assets'][0]['id']:{'local':float_source,'format':'zarr_v2'}})
        with xr.open_zarr(float_source,consolidated=False,decode_cf=False) as ds:
            selected,_=cf_zarr.selection(r,ds);assert checked['valid_pixels']==int(np.isfinite(selected['2m_temperature'].values).sum())
            assert checked['values']==selected['2m_temperature'].size
            assert checked['observations']==selected.sizes['time']
        results.append(dict(case='remote-float32-nan-reversed-swapped-axes',passed=True,chunks=checked['chunks'],peak_rss_bytes=good['peak_rss_bytes']))
    finally:s.shutdown();s.server_close()
    receipt=dict(results=results,consumer_sha256=hashlib.sha256(BIN.read_bytes()).hexdigest(),
                 rust_source_sha256=engine.source_hash(),rss_limit_bytes=192*2**20,
                 memory_scope='consumer process RSS including runtime/GDAL; payload limit 2 MiB, queue 0, working budget per request')
    assert all(v.get('peak_rss_bytes',0)<=receipt['rss_limit_bytes'] for v in results)
    (WORK/'receipt.json').write_text(json.dumps(receipt,indent=2));print(json.dumps(receipt,indent=2))

if __name__=='__main__':main()
