"""Bounded real-product acquisition, frozen replay and independent science gate.

Uses the existing version-bound sparse benchmark mirror. Raw mirrors/references
stay under ignored prototype-output; reports retain canonical unsigned identities.
Run with prototype-output/zarr-env/bin/python bench/wave1_products.py landsat-live|offline.
"""
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import threading
import time
import urllib.parse

import numpy as np
import requests
import rasterio
from rasterio.features import geometry_mask
from rasterio.warp import transform_geom
from rasterio.windows import Window
import xarray as xr
import zarr

import real_multiscene as m
from scientific_output import invoke

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / 'prototype-output/wave1'
DOC = ROOT / 'docs/products'
BIN = Path(os.environ.get('TILE_RIPPER_PRODUCTS_BINARY', ROOT / 'target/debug/tile-ripper'))


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, allow_nan=False)+'\n')


class SignedMirror(m.Mirror):
    """Canonical identity on disk, Azure delegation token only in live memory."""
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.runtime_lock = threading.RLock()
        self.runtime = {}
    def read(self, key, start, end):
        with self.runtime_lock:
            if self.live and not self.sources[key].get('_runtime'):
                source = self.sources[key]
                if '.blob.core.windows.net/' in source['href']:
                    response = requests.get('https://planetarycomputer.microsoft.com/api/sas/v1/sign', params={'href':source['href']}, timeout=30)
                    response.raise_for_status()
                    self.runtime[key] = response.json()['href']
                else:
                    self.runtime[key] = source['href']
                source['_runtime'] = True
            source = self.sources[key]
            canonical = source['href']
            if self.live:
                source['href'] = self.runtime[key]
            try:
                return super().read(key, start, end)
            finally:
                source['href'] = canonical
    def save(self):
        public = copy.deepcopy(list(self.sources.values()))
        for source in public:
            source.pop('_runtime', None)
            source['href'] = source.get('canonical_href', source['href'])
        self.index.write_text(json.dumps(dict(sources=public,pages={k:sorted(v) for k,v in self.pages.items()},events=self.events),indent=2))


def server(mirror):
    s = m.start(mirror)
    return s, f'http://127.0.0.1:{s.server_port}'


def landsat_manifest(day, label):
    url='https://planetarycomputer.microsoft.com/api/stac/v1/search'
    params={'collections':'landsat-c2-l2','bbox':'-78.95,35.95,-78.94,35.96','datetime':f'{day}T00:00:00Z/{day}T23:59:59Z','limit':2}
    response=requests.get(url,params=params,timeout=30);response.raise_for_status()
    item=response.json()['features'][0]
    save(WORK/f'{label}-item.json',item)
    bands={'SR_B4':'red','SR_B5':'nir08','QA_PIXEL':'qa_pixel','QA_RADSAT':'qa_radsat','SR_QA_AEROSOL':'qa_aerosol'}
    sources=[]
    for band,key in bands.items():
        href=item['assets'][key]['href']
        sign=requests.get('https://planetarycomputer.microsoft.com/api/sas/v1/sign',params={'href':href},timeout=30);sign.raise_for_status()
        head=requests.head(sign.json()['href'],timeout=30);head.raise_for_status()
        sources.append(dict(id=band,href=href,canonical_href=href,bytes=int(head.headers['Content-Length']),etag=head.headers['ETag'],datetime=item['properties']['datetime'],metadata=item['assets'][key]))
    save(WORK/f'{label}-sources.json',sources)
    return item,sources


def verify(output, request, sources, base):
    manifest=json.loads((output/'manifest.json').read_text())
    assert manifest['status']=='complete'
    root=zarr.open_group(str(output),mode='r')
    assert root.attrs['status']=='complete'
    by_id={f'p000_{s["id"]}':(i,s) for i,s in enumerate(sources)}
    references=[]
    for record in manifest['outputs']:
        group=record['group'];zg=root[group]
        with xr.open_zarr(str(output),group=group,consolidated=False,decode_cf=False,chunks=None) as ds:
            ds.load()
        i,source=by_id[record['asset_id']]
        with rasterio.Env(GDAL_DISABLE_READDIR_ON_OPEN='EMPTY_DIR',GDAL_HTTP_MULTIRANGE='SERIAL',GDAL_NUM_THREADS='1',GDAL_HTTP_HEADERS=f'If-Match: {source["etag"]}\r\n'):
            with rasterio.open(f'{base}/{i}.tif') as src:
                w=record['window'];win=Window(w['x'],w['y'],w['width'],w['height'])
                values=src.read(1,window=win)
                west,south,east,north=request['aoi']['bbox']
                geo={'type':'Polygon','coordinates':[[[west,south],[east,south],[east,north],[west,north],[west,south]]]}
                geo=transform_geom('EPSG:4326',src.crs,geo)
                aoi=geometry_mask([geo],out_shape=values.shape,transform=src.window_transform(win),invert=True)
                if record['aoi_mask_applied'] and src.nodata is not None and np.isfinite(src.nodata):
                    values[~aoi]=src.nodata
                elif not record['aoi_mask_applied']:
                    aoi[:]=True
                actual=ds['values'].values[0]
                assert actual.dtype==values.dtype
                assert actual.tobytes()==values.tobytes(),source['id']
                valid=aoi & np.isfinite(values)
                if src.nodata is not None and not np.isnan(src.nodata):valid &= values!=src.nodata
                if request['product']=='landsat-c2-l2-sr' and source['id'] in ('QA_PIXEL','SR_QA_AEROSOL'):valid &= (values & 1)==0
                np.testing.assert_array_equal(ds['valid'].values[0],valid.astype('uint8'))
                np.testing.assert_array_equal(ds['aoi_mask'].values[0],aoi.astype('uint8'))
                if request['product']=='opera-dswx-hls' and src.crs.to_epsg() is None:
                    tags=src.tags()
                    assert tags['PRODUCT_TYPE']=='DSWx-HLS' and tags['PROJECT']=='OPERA' and tags['PRODUCT_VERSION']=='1.0'
                    params=src.crs.to_dict();zone=params['zone']
                    assert params['proj']=='utm' and params['ellps']=='WGS84' and params['units']=='m'
                    assert zg.attrs['crs']==f'EPSG:{(32700 if params.get("south") else 32600)+zone}'
                    from pyproj import CRS
                    actual=CRS(src.crs);expected=CRS(zg.attrs['crs'])
                    assert [(p.name,p.value,p.unit_conversion_factor) for p in actual.coordinate_operation.params]==[(p.name,p.value,p.unit_conversion_factor) for p in expected.coordinate_operation.params]
                    assert actual.ellipsoid.semi_major_metre==expected.ellipsoid.semi_major_metre
                    assert actual.ellipsoid.inverse_flattening==expected.ellipsoid.inverse_flattening
                else:
                    assert zg.attrs['crs']==src.crs.to_string()
                transform=src.window_transform(win)
                np.testing.assert_allclose(ds.x.values,transform.c+(np.arange(w['width'])+.5)*transform.a,rtol=0,atol=1e-8)
                np.testing.assert_allclose(ds.y.values,transform.f+(np.arange(w['height'])+.5)*transform.e,rtol=0,atol=1e-8)
                source_record=next(s for s in manifest['sources'] if s['asset_id']==record['asset_id'])
                assert source_record['item_id']==request['observations'][0]['id']
                attrs=ds['values'].attrs
                expected_scale=source.get('metadata',{}).get('raster:bands',[{}])[0].get('scale')
                expected_offset=source.get('metadata',{}).get('raster:bands',[{}])[0].get('offset')
                if expected_scale is not None:assert attrs['scale_factor']==expected_scale
                if expected_offset is not None:assert attrs['add_offset']==expected_offset
                references.append(dict(band=source['id'],dtype=str(values.dtype),shape=list(values.shape),source_window=w,crs=src.crs.to_string(),normalized_crs=zg.attrs['crs'],compression=src.compression.name,scale=attrs.get('scale_factor'),offset=attrs.get('add_offset'),source_nodata=('NaN' if src.nodata is not None and np.isnan(src.nodata) else src.nodata),sha256=hashlib.sha256(values.tobytes()).hexdigest(),values_equal_bits=True,masks_equal=True,coordinates_equal=True))
    return references


def work_metrics(manifest, srv):
    plan=manifest['inspection']['plan']
    layouts={selection['asset_id']:selection['layout'] for selection in plan['selected']}
    widths={'Byte':1,'UInt8':1,'UInt16':2,'Int16':2,'Float32':4}
    unique={};visits=0
    for node in plan['nodes']:
        operation=node['operation']
        if operation['kind']!='read' or operation['read']['kind']!='library_window':continue
        for tile in operation['read']['tiles']:
            visits+=1;unique[(node['asset_id'],operation['read']['band'],tile['x'],tile['y'])]=tile
    selected_bytes=sum(record['window']['width']*record['window']['height']*widths[layouts[record['asset_id']]['dtype']] for record in manifest['outputs'])
    block_bytes=sum(layouts[key[0]]['block'][0]*layouts[key[0]]['block'][1]*widths[layouts[key[0]]['dtype']] for key in unique)
    encoded=sum(tile['bytes'] for tile in unique.values())
    intervals={};wire=0;requests=[]
    for record in srv.records:
        if record.get('method')!='GET':continue
        wire+=record['end']-record['start']+1
        intervals.setdefault(record['source'],[]).append((record['start'],record['end']))
        requests.append((record['source'],record['start'],record['end']))
    distinct=0
    for values in intervals.values():
        cursor=-1
        for start,end in sorted(values):
            distinct+=max(0,end-max(cursor,start-1));cursor=max(cursor,end)
    return dict(selected_native_bytes=selected_bytes,unique_source_blocks=len(unique),planned_block_visits=visits,planned_repeated_block_visits=visits-len(unique),source_block_decoded_bytes=block_bytes,decoded_block_to_selected_byte_ratio=(block_bytes/selected_bytes if selected_bytes else None),unique_selected_tile_encoded_bytes=encoded,http_body_bytes=wire,http_unique_range_bytes=distinct,http_duplicate_range_bytes=wire-distinct,http_repeated_identical_ranges=len(requests)-len(set(requests)),scope='blocks from physical plan; HTTP range union includes metadata and data; freezing-page overhead is separate')


def established_reference(output, request_path, srv):
    reference = output.with_name(output.name + '.rasterio-reference')
    shutil.rmtree(reference, ignore_errors=True)
    srv.reset()
    result = invoke([ROOT/'bench/wave1_reference.py', request_path, output/'manifest.json', reference], binary=Path(sys.executable))
    assert result['code']==0,result['stderr']
    metrics=json.loads(result['stdout'])
    native=zarr.open_group(str(output),mode='r');independent=zarr.open_group(str(reference),mode='r')
    manifest=json.loads((output/'manifest.json').read_text())
    compared=[]
    for record in manifest['outputs']:
        group=record['group'];a=native[group];b=independent[group]
        assert set(a.array_keys())==set(b.array_keys()),(group,list(a.array_keys()),list(b.array_keys()))
        for name in a.array_keys():
            left=a[name][:] if a[name].ndim else a[name][...]
            right=b[name][:] if b[name].ndim else b[name][...]
            assert left.dtype==right.dtype and left.shape==right.shape,(group,name)
            assert left.tobytes()==right.tobytes(),(group,name)
        for key in ['scale_factor','add_offset','units']:
            assert a['values'].attrs.get(key)==b['values'].attrs.get(key),(group,key)
        compared.append(dict(group=group,arrays=sorted(a.array_keys()),equal_bits=True))
    return dict(process={k:v for k,v in result.items() if k not in ('stdout','stderr')},metrics=metrics,wire=copy.deepcopy(srv.counts),compared=compared,selection='identical approved source windows; discovery excluded from both replay execution comparisons')


def independent_verify(output, request_path, source_path, base):
    return json.loads(subprocess.check_output([sys.executable, str(Path(__file__).resolve()), 'verify', str(output), str(request_path), str(source_path), base],text=True,timeout=120))

def landsat(live=False, day="2024-06-29", label="landsat"):
    WORK.mkdir(parents=True,exist_ok=True)
    if live:item,sources=landsat_manifest(day,label)
    else:item=json.loads((WORK/f'{label}-item.json').read_text());sources=json.loads((WORK/f'{label}-sources.json').read_text())
    mirror=SignedMirror(WORK/f'{label}-mirror',sources,live=live,max_bytes=48*2**20);mirror.runtime={}
    srv,base=server(mirror)
    try:
        request=dict(product='landsat-c2-l2-sr',aoi=dict(kind='bounds',crs='EPSG:4326',bbox=[-78.95,35.95,-78.94,35.96]),start=f'{day}T00:00:00Z',end=f'{day}T23:59:59Z',bands=[s['id'] for s in sources],quality_policies=['landsat_pixel_clear'],observations=[dict(id=item['id'],datetime=item['properties']['datetime'],collection=item['collection'],platform=item['properties']['platform'],processing=item['properties']['landsat:correction'],properties=item['properties'],assets={s['id']:dict(href=f'{base}/{i}.tif',etag=s['etag'],metadata=s['metadata']) for i,s in enumerate(sources)})])
        request_path=WORK/f'{label}-request-runtime.json';save(request_path,request)
        output=WORK/f'{label}.zarr';shutil.rmtree(output,ignore_errors=True)
        result=invoke(['fetch',request_path,'--output',output,'--gdal-free'],binary=BIN)
        assert result['code']==0,result['stderr']
        native=json.loads(result['stdout']);native_wire=copy.deepcopy(srv.counts);work=work_metrics(native,srv)
        reference_start=time.monotonic();refs=independent_verify(output,request_path,WORK/f'{label}-sources.json',base);reference_seconds=time.monotonic()-reference_start
        established=established_reference(output,request_path,srv)
        footprint=sum(p.stat().st_size for p in output.rglob('*') if p.is_file())
        report=dict(passed=True,product=request['product'],scope=f"one {item['properties']['platform']} observation, two SR and three QA bands; 30m geographic subset; frozen mirror plus independent rasterio/Zarr-Python/xarray reads",sources=sources,reference=refs,native={k:v for k,v in result.items() if k not in ('stdout','stderr')},observed=native['observed'],wire=native_wire,source_work=work,reference_seconds=reference_seconds,output_objects=sum(p.is_file() for p in output.rglob('*')),output_bytes=footprint,upstream_encoded_bytes=mirror.provider_bytes,upstream_requests=len(mirror.events),mirror_digest=mirror.verify_pages(),established_workflow=established)
        save(DOC/f'{label}-validation.json',report)
        print(json.dumps(dict(product=request['product'],passed=True,wall=result['wall_seconds'],encoded_upstream=mirror.provider_bytes,output_bytes=footprint)))
    finally:srv.shutdown();srv.server_close()


def nasa(live=False):
    from pyproj import Transformer
    manifest=json.loads((WORK/'nasa/manifest.json').read_text())
    groups=[('hls-l30',['hls_l30_b04','hls_l30_fmask'],['B04','Fmask'],['hls_fmask_clear']),('hls-s30',['hls_s30_b04','hls_s30_fmask'],['B04','Fmask'],['hls_fmask_clear']),('opera-dswx-hls',['dswx_wtr','dswx_diag'],['WTR','DIAG'],['dswx_wtr_water']),('opera-rtc-s1',['rtc_vv','rtc_vh','rtc_mask'],['VV','VH','mask'],['opera_rtc_finite','opera_rtc_mask_unaffected'])]
    assets={a['name']:a for a in manifest['assets']}
    rows=[]
    for product,names,bands,policies in groups:
        # The audit's names may carry an explicit mission prefix.
        chosen=[]
        for name in names:
            candidate=assets.get(name)
            if candidate is None:
                candidate=next(a for n,a in assets.items() if n.endswith(name.split('_',1)[-1]) and ('rtc' in n)==('rtc' in name) and ('dswx' in n)==('dswx' in name))
            chosen.append(candidate)
        sources=[dict(id=band,href=a['identity']['url'],canonical_href=a['identity']['url'],bytes=a['identity']['size'],etag=a['identity']['etag'],datetime=manifest['observations'][len(rows)]['datetime'],metadata={'raster:bands':[dict(scale=.0001,offset=0)]} if product.startswith('hls') and band=='B04' else {}) for band,a in zip(bands,chosen)]
        save(WORK/f'{product}-sources.json',sources)
        mirror=SignedMirror(WORK/f'{product}-mirror',sources,live=live,max_bytes=32*2**20);mirror.runtime={}
        srv,base=server(mirror)
        try:
            ref=chosen[0]['reference'];x,y,w,h=ref['window'];a,b,c,d,e,f=ref['transform']
            trans=Transformer.from_crs(ref['crs'],'EPSG:4326',always_xy=True)
            xx=[c+a*(x+.1),c+a*(x+w-.1)];yy=[f+e*(y+.1),f+e*(y+h-.1)]
            points=[trans.transform(px,py) for px in xx for py in yy]
            bbox=[min(p[0] for p in points),min(p[1] for p in points),max(p[0] for p in points),max(p[1] for p in points)]
            observation=copy.deepcopy(manifest['observations'][len(rows)])
            observation['assets']={s['id']:dict(href=f'{base}/{i}.tif',etag=s['etag'],metadata=s['metadata']) for i,s in enumerate(sources)}
            request=dict(product=product,aoi=dict(kind='bounds',crs='EPSG:4326',bbox=bbox),start=observation['datetime'],end=observation['datetime'],bands=bands,quality_policies=policies,observations=[observation])
            path=WORK/f'{product}-request-runtime.json';save(path,request)
            output=WORK/f'{product}.zarr';shutil.rmtree(output,ignore_errors=True)
            result=invoke(['fetch',path,'--output',output,'--gdal-free'],binary=BIN)
            assert result['code']==0,(product,result['stderr'])
            native=json.loads(result['stdout']);wire=copy.deepcopy(srv.counts);work=work_metrics(native,srv)
            reference_start=time.monotonic();refs=independent_verify(output,path,WORK/f'{product}-sources.json',base);reference_seconds=time.monotonic()-reference_start
            established=established_reference(output,path,srv)
            report=dict(passed=True,product=product,scope='public official reference products, independent native-grid observation groups; production archive access is separately constrained',sources=sources,reference=refs,native={k:v for k,v in result.items() if k not in ('stdout','stderr')},observed=native['observed'],wire=wire,source_work=work,reference_seconds=reference_seconds,output_objects=sum(p.is_file() for p in output.rglob('*')),output_bytes=sum(p.stat().st_size for p in output.rglob('*') if p.is_file()),upstream_encoded_bytes=mirror.provider_bytes,upstream_requests=len(mirror.events),mirror_digest=mirror.verify_pages(),established_workflow=established)
            save(DOC/f'{product}-validation.json',report);rows.append(report)
            print(json.dumps(dict(product=product,passed=True,wall=result['wall_seconds'],encoded_upstream=mirror.provider_bytes)),flush=True)
        finally:srv.shutdown();srv.server_close()
    return rows


def direct_live():
    rows=[]
    for label, example in [('landsat', 'product-landsat.json'),('landsat9','product-landsat.json'),('opera-dswx-hls','product-opera-dswx-hls.json'),('opera-rtc-s1','product-opera-rtc-s1.json')]:
        request=json.loads((ROOT/'examples'/example).read_text())
        if label=='landsat9':request['start']='2024-06-21T00:00:00Z';request['end']='2024-06-21T23:59:59Z'
        request_path=WORK/f'{label}-direct-request.json';save(request_path,request)
        output=WORK/f'{label}-direct.zarr';shutil.rmtree(output,ignore_errors=True)
        result=invoke(['fetch',request_path,'--output',output,'--gdal-free'],binary=BIN)
        assert result['code']==0,(label,result['stderr'])
        manifest=json.loads(result['stdout'])
        direct=zarr.open_group(str(output),mode='r');frozen=zarr.open_group(str(WORK/f'{label}.zarr'),mode='r')
        for record in manifest['outputs']:
            group=record['group']
            for name in direct[group].array_keys():
                a=direct[group][name];b=frozen[group][name]
                x=a[:] if a.ndim else a[...];y=b[:] if b.ndim else b[...]
                assert x.dtype==y.dtype and x.shape==y.shape and x.tobytes()==y.tobytes(),(label,group,name)
        row=dict(label=label,product=request['product'],passed=True,scope='native CLI direct original-provider acquisition; exact array agreement with independently checked frozen selections',process={k:v for k,v in result.items() if k not in ('stdout','stderr')},observed=manifest['observed'],sources=[{k:v for k,v in source.items() if k in ('asset_id','href','object_version','item_id','collection','datetime','product_version','requested_band')}for source in manifest['sources']])
        rows.append(row);print(json.dumps(dict(label=label,passed=True,wall=result['wall_seconds'])),flush=True)
    save(DOC/'direct-source-validation.json',dict(passed=True,rows=rows))


if __name__=='__main__':
    if len(sys.argv)==6 and sys.argv[1]=='verify':print(json.dumps(verify(Path(sys.argv[2]),json.loads(Path(sys.argv[3]).read_text()),json.loads(Path(sys.argv[4]).read_text()),sys.argv[5])))
    elif sys.argv[1:] == ['landsat-live']:landsat(True)
    elif sys.argv[1:] == ['landsat9-live']:landsat(True,'2024-06-21','landsat9')
    elif sys.argv[1:] == ['nasa-live']:nasa(True)
    elif sys.argv[1:] == ['direct-live']:direct_live()
    elif sys.argv[1:] == ['offline']:landsat(False);landsat(False,'2024-06-21','landsat9');nasa(False)
    else:raise SystemExit('landsat-live | offline')
