"""Historical Landsat C2 native COG qualification against independent rasterio.

Commands: capture (bounded original-provider ranges + physical inspection),
fetch-live (capture + normal native CLI + independent comparison), offline
(fail-closed captured-range replay), direct-live (normal catalog-discovery CLI).
No imagery, signed SAS URLs, or source bytes are saved in versioned reports.
"""
from __future__ import annotations
import copy
import base64
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import signal
import tempfile
import sys
import subprocess
import time
import urllib.parse

import numpy as np
import rasterio
from rasterio.features import geometry_mask
from rasterio.warp import transform_geom
from rasterio.windows import Window
import requests
import xarray as xr
import zarr

from wave1_products import SignedMirror, server, save, work_metrics
import real_multiscene as mirror_fixture
from scientific_output import invoke

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / 'prototype-output/wave2/landsat'
DOC = ROOT / 'docs/products/wave2'
BIN = Path(os.environ.get('TILE_RIPPER_PRODUCTS_BINARY', ROOT / 'target/debug/tile-ripper'))
STAC = 'https://planetarycomputer.microsoft.com/api/stac/v1'
BANDS = {'SR_B1':'blue','SR_B2':'green','SR_B3':'red','SR_B4':'nir08','SR_B5':'swir16','SR_B7':'swir22','QA_PIXEL':'qa_pixel','QA_RADSAT':'qa_radsat','SR_CLOUD_QA':'cloud_qa','SR_ATMOS_OPACITY':'atmos_opacity'}
CASES = {
 'lt04': ('landsat-tm-c2-l2-sr','LT04_L2SP_016035_19881109_02_T1',[-78.95,35.95,-78.91,35.99]),
 'lt05': ('landsat-tm-c2-l2-sr','LT05_L2SP_016035_20100810_02_T1',[-78.95,35.95,-78.91,35.99]),
 'le07': ('landsat-etm-c2-l2-sr','LE07_L2SP_016035_20040716_02_T1',[-78.65,35.95,-78.61,35.99]),
}
RUNTIME_TOKEN = None

def signed_href(href):
    global RUNTIME_TOKEN
    if RUNTIME_TOKEN is None:
        for attempt in range(5):
            r=requests.get('https://planetarycomputer.microsoft.com/api/sas/v1/sign',params={'href':href},timeout=30)
            if r.status_code!=429:break
            time.sleep(min(10,int(r.headers.get('Retry-After','5'))))
        r.raise_for_status();RUNTIME_TOKEN=urllib.parse.urlsplit(r.json()['href']).query
    return href+'?'+RUNTIME_TOKEN

class HistoricalMirror(SignedMirror):
    def read(self,key,start,end):
        if self.live and not self.sources[key].get('_runtime'):
            self.runtime[key]=signed_href(self.sources[key]['href'])
            self.sources[key]['_runtime']=True
        return super().read(key,start,end)

NODATA = {**{b:0 for b in BANDS if b.startswith('SR_B')},'QA_PIXEL':1,'QA_RADSAT':None,'SR_CLOUD_QA':0,'SR_ATMOS_OPACITY':-9999}

def calibration(band):
    return (0.0000275,-0.2) if band.startswith('SR_B') else ((0.001,0.0) if band=='SR_ATMOS_OPACITY' else (1.0,0.0))

def policies(label):
    return ['landsat_tm_etm_pixel_clear',f'landsat_{"etm" if label=="le07" else "tm"}_radiometrically_unsaturated','landsat_tm_etm_cloud_clear']

def quality(policy, values):
    if policy=='landsat_tm_etm_pixel_clear':
        if np.any(values&0xc004) or np.any(((values>>10)&3)==2) or np.any(((values>>12)&3)==2):
            raise ValueError('undefined historical QA_PIXEL field')
        return (values&0x003b)==0
    if policy.endswith('radiometrically_unsaturated'):
        allowed=0x037f if policy.startswith('landsat_etm_') else 0x027f
        if np.any(values.astype('uint32')&(~allowed&0xffff)):raise ValueError('undefined sensor QA_RADSAT field')
        return values==0
    if policy=='landsat_tm_etm_cloud_clear':
        documented={0,1,2,4,8,9,12,16,20,24,32,34,36,40,48,52,56}
        if not set(map(int,np.unique(values)))<=documented:raise ValueError('undefined LEDAPS cloud QA code')
        return (values!=0)&((values&0x1e)==0)
    raise ValueError(policy)

def discover(label):
    _, item_id, _ = CASES[label]
    r=requests.get(f'{STAC}/collections/landsat-c2-l2/items/{item_id}',timeout=45);r.raise_for_status();item=r.json()
    save(WORK/f'{label}-item.json',item)
    sources=[]
    for band,key in BANDS.items():
        a=item['assets'][key];href=a['href']
        h=requests.head(signed_href(href),timeout=30);h.raise_for_status()
        sources.append(dict(id=band,href=href,canonical_href=href,bytes=int(h.headers['Content-Length']),etag=h.headers['ETag'],datetime=item['properties']['datetime'],metadata=a))
    save(WORK/f'{label}-sources.json',sources)
    return item,sources

def request_for(label,item,sources,base=None):
    product,_,bbox=CASES[label];date=item['properties']['datetime'][:10]
    return dict(product=product,provider='pinned' if base else 'planetary_computer',aoi=dict(kind='bounds',crs='EPSG:4326',bbox=bbox),start=f'{date}T00:00:00Z',end=f'{date}T23:59:59Z',bands=list(BANDS),quality_policies=policies(label),max_observations=8,buffer_bytes=33554432,chunk_edge=256,**(dict(observations=[dict(id=item['id'],datetime=item['properties']['datetime'],collection=item['collection'],platform=item['properties']['platform'],processing=item['properties']['landsat:correction'],properties=item['properties'],assets={s['id']:dict(href=f'{base}/{i}.tif',etag=s['etag'],metadata=s['metadata']) for i,s in enumerate(sources)})]) if base else {}))

def open_env(source):
    return rasterio.Env(GDAL_DISABLE_READDIR_ON_OPEN='EMPTY_DIR',GDAL_HTTP_MULTIRANGE='SERIAL',GDAL_NUM_THREADS='1',GDAL_CACHEMAX=32*2**20,GDAL_HTTP_HEADERS=f'If-Match: {source["etag"]}\r\n')

def reference_window(src,bbox):
    west,south,east,north=bbox
    ring=[[west,south],[east,south],[east,north],[west,north],[west,south]]
    dense=[]
    for a,b in zip(ring,ring[1:]):
        count=max(1,math.ceil(max(abs(b[0]-a[0]),abs(b[1]-a[1]))/0.001))
        dense.extend([[a[0]+(b[0]-a[0])*n/count,a[1]+(b[1]-a[1])*n/count] for n in range(count)])
    dense.append(ring[-1])
    geo=transform_geom('EPSG:4326',src.crs,{'type':'Polygon','coordinates':[dense]})
    coords=np.asarray(geo['coordinates'][0]);cols,rows=(~src.transform)*(coords[:,0],coords[:,1]);x,y=math.floor(min(cols)),math.floor(min(rows));right,bottom=math.ceil(max(cols)),math.ceil(max(rows))
    return Window(x,y,right-x,bottom-y),geo

def inspect(label,sources,base):
    rows=[];raw={}
    for i,source in enumerate(sources):
        band=source['id']
        with open_env(source),rasterio.open(f'{base}/{i}.tif') as src:
            win,_=reference_window(src,CASES[label][2]);values=src.read(1,window=win)
            expected_dtype='int16' if band=='SR_ATMOS_OPACITY' else ('uint8' if band=='SR_CLOUD_QA' else 'uint16')
            raw[band]=values.copy()
            assert values.dtype==expected_dtype and src.nodata==(None if band in ('QA_PIXEL','SR_CLOUD_QA') else NODATA[band]),(label,band,src.nodata)
            scale,offset=calibration(band)
            meta=source['metadata']['raster:bands'][0]
            assert meta.get('scale',1)==scale and meta.get('offset',0)==offset
            assert src.crs.to_string()=='EPSG:32617' and src.res==(30,30)
            rows.append(dict(band=band,asset_key=BANDS[band],dtype=str(values.dtype),source_nodata=src.nodata,product_fill=NODATA[band],scale=scale,offset=offset,crs=src.crs.to_string(),transform=list(src.transform)[:6],shape=list(src.shape),source_window=dict(x=int(win.col_off),y=int(win.row_off),width=int(win.width),height=int(win.height)),block_shapes=[list(v) for v in src.block_shapes],compression=src.compression.name,window_sha256=hashlib.sha256(values.tobytes()).hexdigest(),minimum=int(values.min()),maximum=int(values.max()),fill_samples=int(np.count_nonzero(values==NODATA[band])) if NODATA[band] is not None else None,qa_histogram={str(int(v)):int(n) for v,n in zip(*np.unique(values,return_counts=True))} if 'QA' in band else None))
    if label=='le07':
        missing=raw['SR_B3']==0
        np.testing.assert_array_equal(missing,(raw['QA_PIXEL']&1)!=0)
        next(r for r in rows if r['band']=='SR_B3')['gap_evidence']=dict(source_fill_samples=int(missing.sum()),qa_pixel_fill_coincides=True,qa_radsat_codes_in_gaps=[int(v) for v in np.unique(raw['QA_RADSAT'][missing])],interpolation=False)
    return rows


def complete_manifest(manifest,bands):
    expected={f'p000_{band}' for band in bands}
    outputs=manifest['outputs'];sources=manifest['sources']
    assert manifest['status']=='complete'
    assert len(outputs)==len(expected) and {r['asset_id'] for r in outputs}==expected,'missing or duplicate output band'
    assert len({r['group'] for r in outputs})==len(expected),'duplicate observation group'
    assert len(sources)==len(expected) and {r['asset_id'] for r in sources}==expected,'missing or duplicate source provenance'
    return dict(expected_bands=sorted(bands),output_groups=len(outputs),sources=len(sources),all_requested_bands_once=True)

def compare(label,output,sources,base):
    manifest=json.loads((output/'manifest.json').read_text());complete_manifest(manifest,BANDS)
    root=zarr.open_group(str(output),mode='r');rows=[]
    assert set(root['observations'].group_keys())=={r['group'].split('/')[-1] for r in manifest['outputs']},'unexpected or missing Zarr group'
    by_id={f'p000_{s["id"]}':(i,s) for i,s in enumerate(sources)}
    for record in manifest['outputs']:
        i,source=by_id[record['asset_id']];band=source['id'];group=root[record['group']]
        expected_arrays={'values','valid','aoi_mask','x','y','time','spatial_ref'}
        for policy in policies(label):
            if band==('QA_PIXEL' if policy.endswith('pixel_clear') else ('SR_CLOUD_QA' if policy.endswith('cloud_clear') else 'QA_RADSAT')):expected_arrays.add(f'quality_{policy}')
        assert set(group.array_keys())==expected_arrays,(band,'array completeness')
        with xr.open_zarr(str(output),group=record['group'],consolidated=False,decode_cf=False,chunks=None) as ds:
            ds.load()
        with open_env(source),rasterio.open(f'{base}/{i}.tif') as src:
            expected_win,geo=reference_window(src,CASES[label][2]);w=record['window'];win=Window(w['x'],w['y'],w['width'],w['height'])
            assert win==expected_win,(win,expected_win)
            values=src.read(1,window=win);aoi=geometry_mask([geo],out_shape=values.shape,transform=src.window_transform(win),invert=True)
            if record['aoi_mask_applied'] and src.nodata is not None:
                values[~aoi]=src.nodata
            elif not record['aoi_mask_applied']:aoi[:]=True
            actual=ds['values'].values[0];assert actual.dtype==values.dtype and actual.tobytes()==values.tobytes(),band
            valid=np.ones(values.shape,dtype=bool)
            if NODATA[band] is not None:valid &= values!=NODATA[band]
            if band=='QA_PIXEL':valid &= (values&1)==0
            valid &= aoi
            np.testing.assert_array_equal(ds['valid'].values[0],valid.astype('uint8'))
            np.testing.assert_array_equal(ds['aoi_mask'].values[0],aoi.astype('uint8'))
            trans=src.window_transform(win)
            np.testing.assert_allclose(ds.x.values,trans.c+(np.arange(win.width)+.5)*trans.a,atol=1e-8,rtol=0)
            np.testing.assert_allclose(ds.y.values,trans.f+(np.arange(win.height)+.5)*trans.e,atol=1e-8,rtol=0)
            assert group.attrs['crs']==src.crs.to_string()
            scale,offset=calibration(band);assert ds['values'].attrs['scale_factor']==scale and ds['values'].attrs['add_offset']==offset
            checked=[]
            for policy in policies(label):
                applicable=band==('QA_PIXEL' if policy.endswith('pixel_clear') else ('SR_CLOUD_QA' if policy.endswith('cloud_clear') else 'QA_RADSAT'))
                if applicable:
                    expected=(quality(policy,values)&valid).astype('uint8');np.testing.assert_array_equal(ds[f'quality_{policy}'].values[0],expected);checked.append(policy)
            rows.append(dict(band=band,values_equal_bits=True,source_validity_equal=True,aoi_equal=True,coordinates_equal=True,calibration_equal=True,quality_policies_equal=checked,sha256=hashlib.sha256(values.tobytes()).hexdigest(),valid_samples=int(valid.sum()),samples=int(values.size)))
    return rows


def reference_build(request_path,native_path,destination):
    # The established independent writer imports no Tile-Ripper implementation.
    # Only the independently transcribed historical science table differs.
    import wave1_reference as ref
    product=json.loads(request_path.read_text())['product']
    def band_profile(product,name):
        scale,offset=calibration(name)
        dtype='int16' if name=='SR_ATMOS_OPACITY' else ('uint8' if name=='SR_CLOUD_QA' else 'uint16')
        return ref.Band(dtype,NODATA[name],scale,offset,'1')
    def source_validity(product,name,profile,values):
        valid=np.ones(values.shape,dtype=bool)
        if NODATA[name] is not None:valid &= values!=NODATA[name]
        if name=='QA_PIXEL':valid &= (values&1)==0
        return valid
    def geometry(request,crs):
        grid=type('Grid',(),{'crs':crs,'transform':rasterio.Affine.identity()})()
        return reference_window(grid,request['aoi']['bbox'])[1]
    ref.product_band=band_profile;ref.product_source_validity=source_validity
    ref.quality=quality;ref.request_geometry=geometry
    ref.BAND_ALIASES[product]={band:() for band in BANDS}
    for label in CASES:
        for policy in policies(label):
            ref.POLICY_BANDS[policy]={'QA_PIXEL' if policy.endswith('pixel_clear') else ('SR_CLOUD_QA' if policy.endswith('cloud_clear') else 'QA_RADSAT')}
    return ref.build(request_path,native_path,destination)

def established(output,path,srv):
    reference=output.with_name(output.name+'.rasterio-reference');shutil.rmtree(reference,ignore_errors=True);srv.reset()
    process=invoke([__file__,'reference-runtime',path,output/'manifest.json',reference],binary=Path(sys.executable));assert process['code']==0,process['stderr']
    native=zarr.open_group(str(output),mode='r');other=zarr.open_group(str(reference),mode='r');compared=[]
    manifest=json.loads((output/'manifest.json').read_text())
    for record in manifest['outputs']:
        name=record['group'];a=native[name];b=other[name];assert set(a.array_keys())==set(b.array_keys())
        for key in a.array_keys():
            x=a[key][:] if a[key].ndim else a[key][...];y=b[key][:] if b[key].ndim else b[key][...]
            assert x.dtype==y.dtype and x.shape==y.shape and x.tobytes()==y.tobytes(),(name,key)
        for key in ['scale_factor','add_offset','units']:assert a['values'].attrs.get(key)==b['values'].attrs.get(key)
        compared.append(dict(group=name,arrays=sorted(a.array_keys()),equal_bits=True))
    return dict(process={k:v for k,v in process.items() if k not in ('stdout','stderr')},metrics=json.loads(process['stdout']),wire=copy.deepcopy(srv.counts),compared=compared,scope='same approved source windows and groups; discovery excluded from both; independent rasterio/GDAL reader and Zarr-Python writer')

def normal_commands(label,path,output,sources,base):
    plan=invoke(['plan',path,'--gdal-free'],binary=BIN);assert plan['code']==0,plan['stderr']
    stream=invoke(['stream',path,'--gdal-free','--values'],binary=BIN);assert stream['code']==0,stream['stderr']
    rows=[json.loads(line) for line in stream['stdout'].splitlines() if line]
    assert rows[-1]['event']=='complete' and rows[-1]['source_versions_validated']
    chunks=[r for r in rows if 'values_le_base64' in r]
    assert chunks,(label,[list(r) for r in rows[:2]])
    manifest=json.loads((output/'manifest.json').read_text());complete_manifest(manifest,BANDS);native=zarr.open_group(str(output),mode='r')
    assert len(chunks)==len(BANDS) and {r['asset_id'] for r in chunks}=={r['asset_id'] for r in manifest['outputs']},'missing or duplicated stream band'
    matched=[]
    for row in chunks:
        band=row['band'];record=next(r for r in manifest['outputs'] if r['asset_id']==row['asset_id']);group=native[record['group']]
        assert row['window']==record['window'],'stream source window incomplete, overlapping or duplicated'
        assert row['provisional'],'stream delivery is not provisional'
        # These bounded windows fit one delivery chunk per source.
        assert base64.b64decode(row['aoi_mask_base64'])==group['aoi_mask'][:].tobytes()
        # Stream uses the same finite physical-nodata AOI convention as fetch.
        i=list(BANDS).index(band);source=sources[i]
        w=row['window'];window=Window(w['x'],w['y'],w['width'],w['height'])
        with open_env(source),rasterio.open(f'{base}/{i}.tif') as src:
            original=src.read(1,window=window)
            if src.nodata is not None:original[group['aoi_mask'][:][0]==0]=src.nodata
        assert base64.b64decode(row['values_le_base64'])==original.tobytes()
        valid=np.ones(original.shape,dtype=bool)
        if NODATA[band] is not None:valid &= original!=NODATA[band]
        if band=='QA_PIXEL':valid &= (original&1)==0
        assert base64.b64decode(row['source_valid_mask_base64'])==valid.astype('uint8').tobytes()
        for policy,entry in row['quality'].items():assert base64.b64decode(entry['mask_base64'])==group[f'quality_{policy}'][:].tobytes()
        matched.append(dict(band=band,original_values_equal_bits=True,source_validity_equal=True,aoi_equal=True,quality_equal=True))
    return dict(plan_passed=True,stream_passed=True,stream_coverage=dict(expected_bands=len(BANDS),chunks=len(chunks),all_requested_bands_once=True,windows_exactly_cover_output=True,overlapping_chunks=False),stream_comparison=matched,stream_process={k:v for k,v in stream.items() if k not in ('stdout','stderr')})

def run(label,live=False,fetch=False):
    if live and not (WORK/f'{label}-sources.json').exists():item,sources=discover(label)
    else:item=json.loads((WORK/f'{label}-item.json').read_text());sources=json.loads((WORK/f'{label}-sources.json').read_text())
    assert len(sources)==len(BANDS) and [s['id'] for s in sources]==list(BANDS),'missing or duplicate captured source'
    mirror=HistoricalMirror(WORK/f'{label}-mirror',sources,live=live,max_bytes=64*2**20)
    srv,base=server(mirror)
    try:
        physical=json.loads(subprocess.check_output([sys.executable,__file__,'inspect-runtime',label,base],text=True,timeout=300))
        if label=='le07':
            red=next(r for r in physical if r['band']=='SR_B3');assert 0<red['fill_samples']<red['source_window']['width']*red['source_window']['height'],'L7 qualification window must cross an actual data gap'
        request=request_for(label,item,sources,base);path=WORK/f'{label}-request-runtime.json';save(path,request)
        save(DOC/f'landsat-{label}-request.json',request_for(label,item,sources))
        report=dict(passed=True,binary=dict(path=str(BIN.resolve()),sha256=hashlib.sha256(BIN.read_bytes()).hexdigest()),product=CASES[label][0],item_id=item['id'],datetime=item['properties']['datetime'],platform=item['properties']['platform'],stac_item=f'{STAC}/collections/landsat-c2-l2/items/{item["id"]}',scope='bounded original COG range capture and independent GDAL/rasterio physical inspection',aoi=request['aoi'],sources=[{k:v for k,v in source.items() if k!='_runtime'} for source in sources],physical=physical,provider_bytes=mirror.provider_bytes,upstream_requests=len(mirror.events),mirror_digest=mirror.verify_pages(),independent_reader=dict(rasterio=rasterio.__version__,gdal=rasterio.__gdal_version__))
        if fetch:
            output=WORK/f'{label}.zarr';shutil.rmtree(output,ignore_errors=True);srv.reset()
            result=invoke(['fetch',path,'--output',output,'--gdal-free'],binary=BIN);assert result['code']==0,result['stderr']
            native=json.loads(result['stdout']);report['completeness']=complete_manifest(native,BANDS);report.update(scope='normal native CLI frozen replay plus independent rasterio/xarray/Zarr values, validity, QA, coordinates and calibration comparison',native={k:v for k,v in result.items() if k not in ('stdout','stderr')},wire=copy.deepcopy(srv.counts),source_work=work_metrics(native,srv),comparison=json.loads(subprocess.check_output([sys.executable,__file__,'compare-runtime',label,base,str(output)],text=True,timeout=300)),observed=native['observed'])
            report.update(provider_bytes=mirror.provider_bytes,upstream_requests=len(mirror.events),mirror_digest=mirror.verify_pages(),output_objects=sum(p.is_file() for p in output.rglob('*')),output_bytes=sum(p.stat().st_size for p in output.rglob('*') if p.is_file()),established_workflow=established(output,path,srv))
            report['normal_commands']=json.loads(subprocess.check_output([sys.executable,__file__,'commands-runtime',label,base,str(output),str(path)],text=True,timeout=300))
        save(DOC/f'landsat-{label}-{"validation" if fetch else "inspection"}.json',report)
        print(json.dumps(dict(label=label,passed=True,fetch=fetch,provider_bytes=mirror.provider_bytes)),flush=True)
    finally:srv.shutdown();srv.server_close()


class FaultHandler(mirror_fixture.Handler):
    def serve(self,body):
        if getattr(self.server,'force_503',False):
            with self.server.lock:self.server.counts['errors']+=1
            self.send_error(503);return
        return super().serve(body)

def faults(label='lt05'):
    sources=json.loads((WORK/f'{label}-sources.json').read_text());item=json.loads((WORK/f'{label}-item.json').read_text())
    mirror=HistoricalMirror(WORK/f'{label}-mirror',sources,live=False);srv,base=server(mirror);srv.RequestHandlerClass=FaultHandler
    request=request_for(label,item,sources,base);path=WORK/f'{label}-fault-request-runtime.json';save(path,request);rows=[]
    original_etags=[s['etag'] for s in mirror.sources.values()]
    try:
        for mode in ('fetch_source_mutation','fetch_cancellation','fetch_503'):
            srv.reset();srv.latency=.02;srv.force_503=False
            output=WORK/f'{label}.{mode}.zarr';shutil.rmtree(output,ignore_errors=True)
            with tempfile.TemporaryFile() as stdout,tempfile.TemporaryFile() as stderr:
                process=subprocess.Popen([str(BIN),'fetch',str(path),'--output',str(output),'--gdal-free'],stdout=stdout,stderr=stderr)
                deadline=time.monotonic()+20
                while process.poll() is None and srv.counts['get']<5 and time.monotonic()<deadline:time.sleep(.01)
                assert process.poll() is None,'fault injection missed active operation'
                if mode=='fetch_source_mutation':
                    for source in mirror.sources.values():source['etag']='"changed-during-historical-acquisition"'
                elif mode=='fetch_503':srv.force_503=True
                else:process.send_signal(signal.SIGINT)
                code=process.wait(timeout=30)
            completed=(output/'manifest.json').exists() and json.loads((output/'manifest.json').read_text()).get('status')=='complete'
            assert code!=0 and not completed,(mode,code,completed)
            rows.append(dict(mode=mode,exit_code=code,injected_after_gets=5,complete_manifest_published=False,wire=copy.deepcopy(srv.counts)))
            for source,etag in zip(mirror.sources.values(),original_etags):source['etag']=etag
            srv.force_503=False
        for mode in ('stream_source_mutation_after_chunk','stream_cancellation_after_chunk','stream_503_after_chunk'):
            srv.reset();srv.latency=.02
            process=subprocess.Popen([str(BIN),'stream',str(path),'--gdal-free','--values'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True)
            chunk=False;complete=False
            for line in process.stdout:
                if json.loads(line).get('event')=='chunk':chunk=True;break
            assert chunk and process.poll() is None,'fault must follow active provisional delivery'
            if mode=='stream_source_mutation_after_chunk':
                for source in mirror.sources.values():source['etag']='"changed-after-historical-chunk"'
            elif mode=='stream_503_after_chunk':srv.force_503=True
            else:process.send_signal(signal.SIGINT)
            for line in process.stdout:
                if json.loads(line).get('event')=='complete':complete=True
            code=process.wait(timeout=30);assert code!=0 and not complete,(mode,code,complete)
            rows.append(dict(mode=mode,exit_code=code,provisional_chunk_received=True,complete_event_published=False,wire=copy.deepcopy(srv.counts)))
            for source,etag in zip(mirror.sources.values(),original_etags):source['etag']=etag
            srv.force_503=False
        save(DOC/f'landsat-{label}-faults.json',dict(passed=True,binary=dict(path=str(BIN.resolve()),sha256=hashlib.sha256(BIN.read_bytes()).hexdigest()),product=CASES[label][0],item_id=item['id'],scope='captured real historical COG bytes; failures deliberately injected on local replay; no live fault traffic',faults=rows))
        print(json.dumps(dict(label=label,faults_passed=True,count=len(rows))),flush=True)
    finally:srv.shutdown();srv.server_close()

def discovery(label):
    request=json.loads((DOC/f'landsat-{label}-request.json').read_text())
    process=invoke(['discover','--product',request['product'],'--bbox',','.join(map(str,request['aoi']['bbox'])),'--start',request['start'],'--end',request['end'],'--bands',','.join(request['bands']),'--limit','8'],binary=BIN);assert process['code']==0,process['stderr']
    catalog=json.loads(process['stdout']);assert catalog['complete'] and [o['id'] for o in catalog['observations']]==[CASES[label][1]]
    save(DOC/f'landsat-{label}-discovery.json',dict(passed=True,product=request['product'],expected_item_id=CASES[label][1],discovery=catalog))

def direct(label):
    request=json.loads((DOC/f'landsat-{label}-request.json').read_text());request['bands']=['SR_B3','SR_B4','QA_PIXEL','QA_RADSAT','SR_CLOUD_QA'];request_path=WORK/f'{label}-direct-request.json';save(request_path,request);output=WORK/f'{label}-direct.zarr';shutil.rmtree(output,ignore_errors=True)
    discovery=invoke(['discover','--product',request['product'],'--bbox',','.join(map(str,request['aoi']['bbox'])),'--start',request['start'],'--end',request['end'],'--bands',','.join(request['bands']),'--limit','8'],binary=BIN);assert discovery['code']==0,discovery['stderr']
    result=invoke(['fetch',request_path,'--output',output,'--gdal-free'],binary=BIN);assert result['code']==0,result['stderr']
    manifest=json.loads(result['stdout']);native=zarr.open_group(str(output),mode='r');frozen=zarr.open_group(str(WORK/f'{label}.zarr'),mode='r')
    compared=[]
    frozen_manifest=json.loads((WORK/f'{label}.zarr'/'manifest.json').read_text())
    for record in manifest['outputs']:
        group=record['group'];other_group=next(r['group'] for r in frozen_manifest['outputs'] if r['asset_id']==record['asset_id']);assert set(native[group].array_keys())==set(frozen[other_group].array_keys())
        for name in native[group].array_keys():
            a=native[group][name];b=frozen[other_group][name];x=a[:] if a.ndim else a[...];y=b[:] if b.ndim else b[...]
            assert x.dtype==y.dtype and x.shape==y.shape and x.tobytes()==y.tobytes(),(label,group,name)
        compared.append(dict(group=group,arrays=sorted(native[group].array_keys()),equal_bits=True))
    save(DOC/f'landsat-{label}-direct.json',dict(passed=True,product=CASES[label][0],scope='normal native CLI discover plus five-band catalog discovery and original provider acquisition; every output array exactly matches independently validated replay',bands=request['bands'],discovery=json.loads(discovery['stdout']),process={k:v for k,v in result.items() if k not in ('stdout','stderr')},observed=manifest['observed'],compared=compared,sources=manifest['sources']))
    print(json.dumps(dict(label=label,direct=True,passed=True)),flush=True)

if __name__=='__main__':
    mode=sys.argv[1] if len(sys.argv)>1 else ''
    if mode=='reference-runtime':
        print(json.dumps(reference_build(*map(Path,sys.argv[2:5]))));raise SystemExit(0)
    if mode=='commands-runtime':
        label,base,output,path=sys.argv[2:6];sources=json.loads((WORK/f'{label}-sources.json').read_text())
        print(json.dumps(normal_commands(label,Path(path),Path(output),sources,base)));raise SystemExit(0)
    if mode in ('inspect-runtime','compare-runtime'):
        label,base=sys.argv[2:4];sources=json.loads((WORK/f'{label}-sources.json').read_text())
        print(json.dumps(inspect(label,sources,base) if mode=='inspect-runtime' else compare(label,Path(sys.argv[4]),sources,base)));raise SystemExit(0)
    labels=sys.argv[2:] or list(CASES)
    if mode not in ('capture','fetch-live','offline','direct-live','faults','discover'):raise SystemExit('capture | fetch-live | offline | direct-live | faults | discover [lt04 lt05 le07]')
    for index,label in enumerate(labels):
        if mode=='direct-live':
            if index:time.sleep(6)  # Respect the observed PC signing-service cooldown.
            direct(label)
        elif mode=='faults':faults(label)
        elif mode=='discover':discovery(label)
        else:run(label,live=mode!='offline',fetch=mode!='capture')
