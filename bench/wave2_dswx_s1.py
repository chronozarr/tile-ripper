#!/usr/bin/env python3
"""Pinned DSWx-S1 production inspection and independent rasterio/xarray gate.

Capture uses explicit private URS netrc, exact-object redirect admission, and
bounded version-conditional Range GETs. No temporary provider locators persist.
Raw captures and CLI outputs belong under ignored prototype-output only.
"""
from __future__ import annotations
import argparse
import base64
import signal
import tempfile
import xml.etree.ElementTree as ET
import hashlib
import json
import time
import netrc
import re
import stat
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urljoin, urlparse
import numpy as np
import rasterio
import requests
import xarray as xr
import zarr
from rasterio.warp import transform as warp_coordinates
from rasterio.windows import Window

ROOT = Path(__file__).resolve().parents[1]
DOC = ROOT / 'docs/products/wave2'
WORK = ROOT / 'prototype-output/wave2-dswx-s1'
COLLECTION = 'OPERA_L3_DSWX-S1_V1'
CONCEPT = 'G4385346748-POCLOUD'
GRANULE = 'OPERA_L3_DSWx-S1_T17SQV_20261021T230515Z_20261023T070839Z_S1D_30_v1.0'
ARCHIVE = 'https://archive.podaac.earthdata.nasa.gov'
URS = 'https://urs.earthdata.nasa.gov'
RESOURCE = 'https://deotb6e7tfubr.cloudfront.net'
PREFIX = '/podaac-ops-cumulus-protected/'
GATE = '/s3-edaf5da92e0ce48fb61175c28b67e95d/podaac-ops-cumulus-protected.s3.us-west-2.amazonaws.com/'
BANDS = {'WTR':'B01_WTR','BWTR':'B02_BWTR','CONF':'B03_CONF','DIAG':'B04_DIAG'}
REFERENCES = {
 'collection':'https://cmr.earthdata.nasa.gov/search/collections.umm_json?concept_id=C2949811996-POCLOUD',
 'granule':f'https://cmr.earthdata.nasa.gov/search/granules.umm_json?concept_id={CONCEPT}',
 'specification':'https://archive.podaac.earthdata.nasa.gov/podaac-ops-cumulus-docs/web-misc/opera/D-108761_OPERA_DSWx_S1AB_ProductSpec_Rev_A.pdf',
 'software':'https://github.com/opera-adt/DSWX-SAR/tree/v1.2',
}

class ValidationError(Exception): pass

def require(condition, message):
 if not condition: raise ValidationError(message)

def save(path, data):
 path.parent.mkdir(parents=True, exist_ok=True)
 path.write_text(json.dumps(data,indent=2,allow_nan=False)+'\n')

def sha(array): return hashlib.sha256(np.ascontiguousarray(array).tobytes()).hexdigest()
def origin(url):
 p=urlparse(url);return f'{p.scheme}://{p.netloc}'
def href(band): return f'{ARCHIVE}{PREFIX}{COLLECTION}/{GRANULE}_{BANDS[band]}.tif'

def credentials(path):
 s=path.stat();require(stat.S_ISREG(s.st_mode) and s.st_mode & 0o077 == 0,'netrc must be a private regular file')
 a=netrc.netrc(path).authenticators('urs.earthdata.nasa.gov')
 require(a and a[0] and a[2],'explicit URS credentials missing');return a[0],a[2]

def exact_get(url, auth, start, end, etag=None):
 """One bounded request: admit only this object's observed provider route."""
 p=urlparse(url);require(origin(url)==ARCHIVE and p.path.startswith(PREFIX+COLLECTION+'/') and not p.query and not p.fragment,'canonical identity changed')
 require(p.path.endswith('.tif'),'canonical object must be TIFF')
 resource_path=GATE+p.path[len(PREFIX):]
 sessions={o:requests.Session() for o in (ARCHIVE,URS,RESOURCE)}
 for session in sessions.values():
  session.trust_env=False;session.headers.update({'Accept-Encoding':'identity','User-Agent':'tile-ripper-wave2-dswx-s1/1'})
 current=url;trace=[]
 try:
  for _ in range(8):
   o=origin(current);cp=urlparse(current)
   allowed=(o==ARCHIVE and cp.path in (p.path,'/login')) or (o==URS and cp.path=='/oauth/authorize') or (o==RESOURCE and cp.path==resource_path)
   require(allowed,'redirect escaped exact-object policy')
   headers={'Range':f'bytes={start}-{end}'} if (o==RESOURCE or (o==ARCHIVE and cp.path==p.path)) else {}
   if headers and etag:headers['If-Match']=etag
   r=sessions[o].get(current,headers=headers,auth=auth if o==URS else None,allow_redirects=False,stream=True,timeout=(10,60))
   if r.status_code in (301,302,303,307,308):
    target=urljoin(current,r.headers.get('Location',''));tp=urlparse(target)
    valid=(origin(target)==ARCHIVE and tp.path in (p.path,'/login')) or (origin(target)==URS and tp.path=='/oauth/authorize') or (origin(target)==RESOURCE and tp.path==resource_path)
    require(valid,'redirect destination changed')
    trace.append({'from_origin':o,'to_origin':origin(target),'status':r.status_code,'temporary_query_present':bool(tp.query)})
    current=target;r.close();continue
   require(o==RESOURCE and r.status_code==206,'range did not resolve to provider resource')
   framing=re.fullmatch(r'bytes (\d+)-(\d+)/(\d+)',r.headers.get('Content-Range',''))
   require(framing and int(framing[1])==start and int(framing[2])==end,'range framing changed')
   actual_etag=r.headers.get('ETag');require(actual_etag and (etag is None or actual_etag==etag),'object version changed')
   body=r.raw.read(end-start+2);r.close();require(len(body)==end-start+1,'bounded response length changed')
   return body,actual_etag,int(framing[3]),trace
  raise ValidationError('redirect budget exhausted')
 finally:
  for session in sessions.values():session.close()

def inspect(path):
 with rasterio.open(path) as src:
  values=src.read(1);v,c=np.unique(values,return_counts=True)
  tags=src.tags();require(src.count==1 and src.dtypes==('uint8',),'actual source dtype changed')
  require(src.shape==(3660,3660),'actual shape changed')
  return {'dtype':src.dtypes[0],'nodata':src.nodata,'shape':list(src.shape),'crs':str(src.crs),'transform':list(tuple(src.transform)[:6]),'bounds':list(src.bounds),'block_shapes':[list(x) for x in src.block_shapes],'overviews':src.overviews(1),'compression':src.compression.name,'scales':list(src.scales),'offsets':list(src.offsets),'descriptions':list(src.descriptions),'metadata':tags,'unique_counts':{str(int(x)):int(y) for x,y in zip(v,c)},'raw_sha256':sha(values),'valid_sha256':sha(src.read_masks(1)!=0),'valid_pixels':int(np.count_nonzero(src.read_masks(1)))},values

def capture(netrc_path):
 auth=credentials(netrc_path);WORK.mkdir(parents=True,exist_ok=True)
 prior_path=DOC/'dswx-s1-source-inspection.json';prior=json.loads(prior_path.read_text()) if prior_path.exists() else None
 cmr=requests.get(REFERENCES['granule'],timeout=30);require(cmr.status_code==200,'CMR metadata unavailable');meta=cmr.json()['items'][0]
 require(meta['umm']['GranuleUR']==GRANULE,'CMR granule identity changed')
 save(WORK/'cmr.json',{'items':[meta]})
 receipt={'status':'source_inspected','captured_utc':datetime.now(timezone.utc).isoformat(),'primary_references':REFERENCES,'collection':COLLECTION,'collection_version':'1.0','product_version':'1.0','granule':GRANULE,'cmr_concept':CONCEPT,'cmr_revision':meta['meta']['revision-id'],'cmr_temporal_extent':meta['umm']['TemporalExtent'],'input_granules':meta['umm']['InputGranules'],'sources':{}}
 arrays={}
 for band in BANDS:
  _,etag,size,probe_trace=exact_get(href(band),auth,0,0)
  require(size<=8*1024*1024,'source exceeds 8 MiB per-object capture bound')
  if prior:
   require(prior['sources'][band]['etag']==etag and prior['sources'][band]['size_bytes']==size,'pinned source version changed')
  body,_,_,trace=exact_get(href(band),auth,0,size-1,etag)
  if prior:require(hashlib.sha256(body).hexdigest()==prior['sources'][band]['sha256'],'pinned source bytes changed')
  path=WORK/f'{band}.tif';path.write_bytes(body);info,arrays[band]=inspect(path)
  receipt['sources'][band]={'canonical_href':href(band),'etag':etag,'size_bytes':size,'sha256':hashlib.sha256(body).hexdigest(),'range_probe_trace':probe_trace,'conditional_capture_trace':trace,'inspection':info}
 expected_union=np.where(arrays['WTR']==3,1,arrays['WTR']);mismatch=expected_union!=arrays['BWTR']
 receipt['wtr_bwtr_union_comparison']={'equal':not bool(np.any(mismatch)),'mismatch_count':int(np.count_nonzero(mismatch)),'observed_wtr_bwtr_pairs':np.unique(np.stack([arrays['WTR'][mismatch],arrays['BWTR'][mismatch]],axis=1),axis=0).tolist(),'interpretation':'Source layers retain independent mask precedence; no raw class values are rewritten.'}
 save(DOC/'dswx-s1-source-inspection.json',receipt)
 print(json.dumps({'status':receipt['status'],'granule':GRANULE,'layers':{b:{'size_bytes':s['size_bytes'],'nodata':s['inspection']['nodata'],'unique_counts':s['inspection']['unique_counts']} for b,s in receipt['sources'].items()}},indent=2))
 return receipt

def verify(output, request_path=None):
 receipt=json.loads((DOC/'dswx-s1-source-inspection.json').read_text());request=json.loads((request_path or DOC/'dswx-s1-request.json').read_text())
 manifest=json.loads((output/'manifest.json').read_text());require(manifest['status']=='complete','CLI output incomplete')
 expected_assets={f'p000_{band}' for band in BANDS}
 require(set(request['bands'])==set(BANDS),'pinned qualification must request all four layers')
 for field in ('outputs','sources'):
  ids=[entry['asset_id'] for entry in manifest[field]]
  require(len(ids)==len(expected_assets) and set(ids)==expected_assets,f'{field} must contain each requested layer exactly once')
 require(len({entry['group'] for entry in manifest['outputs']})==len(expected_assets),'output groups must be unique per layer')
 root=zarr.open_group(str(output),mode='r');require(root.attrs['status']=='complete','Zarr root incomplete')
 results=[]
 for entry in manifest['outputs']:
  band=entry['asset_id'].split('_',1)[1];pin=receipt['sources'][band];source=next(x for x in manifest['sources'] if x['asset_id']==entry['asset_id'])
  require(source['item_id']==GRANULE and source['product_version']=='1.0','stored product identity changed')
  require(source['object_version']==pin['etag'],'stored object version changed')
  asset=request['observations'][0]['assets'][band]
  require(source['reference']==asset['href'],'source reference changed')
  require(asset.get('metadata',{}).get('canonical_href',asset['href'])==pin['canonical_href'],'canonical source identity changed')
  group=entry['group'];zg=root[group];w=entry['window'];win=Window(w['x'],w['y'],w['width'],w['height'])
  with xr.open_zarr(str(output),group=group,consolidated=False,decode_cf=False,chunks=None) as ds:
   ds.load()
   with rasterio.open(WORK/f'{band}.tif') as src:
    require(hashlib.sha256((WORK/f'{band}.tif').read_bytes()).hexdigest()==pin['sha256'],'captured reference bytes changed')
    raw=src.read(1,window=win);transform=src.window_transform(win)
    west,south,east,north=request['aoi']['bbox']
    xx,yy=np.meshgrid(transform.c+(np.arange(win.width)+.5)*transform.a,transform.f+(np.arange(win.height)+.5)*transform.e)
    longitude,latitude=warp_coordinates(src.crs,'EPSG:4326',xx.ravel(),yy.ravel())
    longitude=np.asarray(longitude).reshape(raw.shape);latitude=np.asarray(latitude).reshape(raw.shape)
    aoi=(longitude>=west)&(longitude<=east)&(latitude>=south)&(latitude<=north)
    if not entry['aoi_mask_applied']:aoi[:]=True
    expected=raw.copy()
    if entry['aoi_mask_applied']:expected[~aoi]=src.nodata
    actual=ds['values'].values[0];require(actual.dtype==raw.dtype and actual.tobytes()==expected.tobytes(),f'{band} raw values differ')
    valid=aoi & (expected!=src.nodata)
    np.testing.assert_array_equal(ds['valid'].values[0],valid.astype('uint8'))
    np.testing.assert_array_equal(ds['aoi_mask'].values[0],aoi.astype('uint8'))
    np.testing.assert_allclose(ds.x.values,transform.c+(np.arange(win.width)+.5)*transform.a,rtol=0,atol=1e-8)
    np.testing.assert_allclose(ds.y.values,transform.f+(np.arange(win.height)+.5)*transform.e,rtol=0,atol=1e-8)
    require(zg.attrs['crs']==str(src.crs),f'{band} CRS differs')
    attributes=ds['values'].attrs
    require(attributes['units']=='1' and attributes['scale_factor']==1 and attributes['add_offset']==0,'stored calibration/units differ')
    xml=source['object_identities']['native_cog']['gdal_metadata_xml']
    tags={item.attrib['name']:item.text or '' for item in ET.fromstring(xml)}
    for key,value in pin['inspection']['metadata'].items():
     if key!='AREA_OR_POINT':require(tags.get(key)==value,f'{band} metadata {key} differs')
    require(zg.attrs['source']==source,'Zarr provenance differs from manifest')
    policies=[]
    for key in ds.data_vars:
     if key=='quality_dswx_s1_bwtr_water':
      np.testing.assert_array_equal(ds[key].values[0],(valid & (expected==1)).astype('uint8'));policies.append(key)
     if key=='quality_dswx_s1_wtr_water':
      np.testing.assert_array_equal(ds[key].values[0],(valid & np.isin(expected,[1,3])).astype('uint8'));policies.append(key)
    if band=='WTR':require('quality_dswx_s1_wtr_water' in policies,'explicit S1 WTR water mask missing')
    if band=='BWTR' and 'dswx_s1_bwtr_water' in request['quality_policies']:require('quality_dswx_s1_bwtr_water' in policies,'explicit S1 BWTR water mask missing')
    expected_time=np.datetime64(request['observations'][0]['datetime'].rstrip('Z'),'ns').astype('i8')
    np.testing.assert_array_equal(ds.time.values,[expected_time])
    results.append({'band':band,'window':w,'dtype':str(raw.dtype),'source_nodata':src.nodata,'values_sha256':sha(expected),'values_equal_bits':True,'valid_equal':True,'aoi_equal':True,'coordinates_equal':True,'crs_equal':True,'quality_policies':policies,'class_counts':{str(int(v)):int(c) for v,c in zip(*np.unique(raw,return_counts=True))}})
 return {'status':'passed','granule':GRANULE,'product_version':'1.0','output':str(output),'independent_reader':'rasterio '+rasterio.__version__,'reopen_reader':'xarray '+xr.__version__,'layers':results,'manifest_sources_retained':True}

def reference(request_path, manifest_path, output):
 """Independent GDAL HTTP windows → reopenable Zarr; only metadata copied."""
 require(not output.exists(),'reference output already exists')
 request=json.loads(request_path.read_text());manifest=json.loads(manifest_path.read_text())
 native=zarr.open_group(str(manifest_path.parent),mode='r');root=zarr.open_group(str(output),mode='w');root.attrs.update(dict(native.attrs));reads=0
 for entry in manifest['outputs']:
  band=entry['asset_id'].split('_',1)[1];asset=request['observations'][0]['assets'][band];w=entry['window'];win=Window(w['x'],w['y'],w['width'],w['height'])
  with rasterio.Env(GDAL_DISABLE_READDIR_ON_OPEN='EMPTY_DIR',GDAL_HTTP_MULTIRANGE='SERIAL',GDAL_NUM_THREADS='1',GDAL_HTTP_HEADERS=f'If-Match: {asset["etag"]}\r\n'):
   with rasterio.open(asset['href']) as src:
    raw=src.read(1,window=win);reads+=1;t=src.window_transform(win);x=t.c+(np.arange(win.width)+.5)*t.a;y=t.f+(np.arange(win.height)+.5)*t.e;xx,yy=np.meshgrid(x,y)
    lon,lat=warp_coordinates(src.crs,'EPSG:4326',xx.ravel(),yy.ravel());lon=np.array(lon).reshape(raw.shape);lat=np.array(lat).reshape(raw.shape);west,south,east,north=request['aoi']['bbox'];aoi=(lon>=west)&(lon<=east)&(lat>=south)&(lat<=north)
    if not entry['aoi_mask_applied']:aoi[:]=True
    raw[~aoi]=src.nodata;valid=aoi&(raw!=src.nodata)
  approved=native[entry['group']];group=root.require_group(entry['group']);group.attrs.update(dict(approved.attrs))
  values={'values':raw[None,:,:],'valid':valid.astype('u1')[None,:,:],'aoi_mask':aoi.astype('u1')[None,:,:],'x':x,'y':y,'time':np.array([np.datetime64(entry['datetime'].rstrip('Z'),'ns').astype('i8')]),'spatial_ref':np.array(0,dtype='i4')}
  if band=='WTR' and 'dswx_s1_wtr_water' in request['quality_policies']:values['quality_dswx_s1_wtr_water']=(valid&np.isin(raw,[1,3])).astype('u1')[None,:,:]
  if band=='BWTR' and 'dswx_s1_bwtr_water' in request['quality_policies']:values['quality_dswx_s1_bwtr_water']=(valid&(raw==1)).astype('u1')[None,:,:]
  for name,array in values.items():
   template=approved[name];created=group.create_dataset(name,data=array,chunks=template.chunks,dtype=template.dtype,compressor=None,fill_value=template.fill_value);created.attrs.update(dict(template.attrs))
 save(output/'manifest.json',manifest)
 print(json.dumps({'status':'complete','source_read_calls':reads,'output_objects':sum(p.is_file() for p in output.rglob('*')),'output_bytes':sum(p.stat().st_size for p in output.rglob('*') if p.is_file())}))

def replay(binary, output):
 """Replay original bytes with no upstream requests via existing frozen mirror."""
 import real_multiscene as mirrorlib
 import wave1_products
 from scientific_output import invoke
 require(not output.exists(),'replay output already exists')
 receipt=json.loads((DOC/'dswx-s1-source-inspection.json').read_text())
 request=json.loads((DOC/'dswx-s1-request.json').read_text())
 sources=[];mirror_root=WORK/'frozen';mirror_root.mkdir(exist_ok=True)
 pages={};events=[]
 for i,band in enumerate(BANDS):
  pin=receipt['sources'][band];body=(WORK/f'{band}.tif').read_bytes()
  require(hashlib.sha256(body).hexdigest()==pin['sha256'],'frozen capture digest changed')
  sources.append({'id':band,'href':pin['canonical_href'],'canonical_href':pin['canonical_href'],'etag':pin['etag'],'bytes':pin['size_bytes'],'datetime':request['start']})
  (mirror_root/f'{i}.tif').write_bytes(body)
  pages[f'/{i}.tif']=list(range((len(body)+mirrorlib.PAGE-1)//mirrorlib.PAGE))
  events.append({'source':f'/{i}.tif','start':0,'end':len(body)-1,'bytes':len(body),'seconds':0,'sha256':pin['sha256']})
 save(mirror_root/'index.json',{'sources':sources,'pages':pages,'events':events})
 mirror=mirrorlib.Mirror(mirror_root,sources,live=False)
 digest=mirror.verify_pages();server=mirrorlib.start(mirror)
 try:
  for i,band in enumerate(BANDS):
   asset=request['observations'][0]['assets'][band]
   asset['metadata']['canonical_href']=asset['href'];asset['href']=f'http://127.0.0.1:{server.server_port}/{i}.tif'
  request_path=WORK/'frozen-request.json';save(request_path,request)
  t=time.monotonic()
  command=[str(binary.resolve()),'fetch',str(request_path),'--output',str(output),'--output-format','zarr_v2','--backend','native','--json']
  process=invoke(command[1:],binary=binary.resolve())
  require(process['code']==0,'frozen native CLI fetch failed')
  seconds=time.monotonic()-t
  result=verify(output,request_path);result['mode']='frozen_original_bytes';result['wall_seconds']=seconds;result['wire']=server.counts;result['upstream_requests_during_replay']=0;result['mirror_digest']=digest;result['binary_sha256']=hashlib.sha256(binary.read_bytes()).hexdigest()
  manifest=json.loads((output/'manifest.json').read_text());result['native_writer']=manifest['observed'];result['source_work']=wave1_products.work_metrics(manifest,server)
  result['native_process']={k:v for k,v in process.items() if k not in ('stdout','stderr')};result['native_output_objects']=sum(p.is_file() for p in output.rglob('*'));result['native_output_bytes']=sum(p.stat().st_size for p in output.rglob('*') if p.is_file())
  result['native_wire']=dict(server.counts)
  server.reset();reference_output=output.with_name(output.name+'.rasterio-reference')
  baseline=invoke([str(Path(__file__).resolve()),'--reference',str(request_path),str(output/'manifest.json'),'--output',str(reference_output)],binary=Path(sys.executable))
  require(baseline['code']==0,'independent rasterio→Zarr reference failed')
  result['reference_process']={k:v for k,v in baseline.items() if k not in ('stdout','stderr')};result['reference_metrics']=json.loads(baseline['stdout']);result['reference_wire']=dict(server.counts)
  reference_root=zarr.open_group(str(reference_output),mode='r');native_root=zarr.open_group(str(output),mode='r')
  for entry in manifest['outputs']:
   for name in native_root[entry['group']].array_keys():np.testing.assert_array_equal(native_root[entry['group']][name][...],reference_root[entry['group']][name][...])
  result['all_reference_arrays_equal']=True
  server.reset()
  streamed=invoke(['stream',str(request_path),'--backend','native','--values','--json'],binary=binary.resolve())
  require(streamed['code']==0,'frozen value stream failed')
  rows=[json.loads(line) for line in streamed['stdout'].splitlines()];require(rows[-1].get('event')=='complete' and rows[-1].get('source_versions_validated'),'stream completion not version validated')
  require(sum(row.get('event')=='complete' for row in rows)==1,'stream completion must occur exactly once')
  approved={entry['asset_id']:entry for entry in manifest['outputs']}
  coverage={asset:np.zeros((entry['window']['height'],entry['window']['width']),dtype=bool) for asset,entry in approved.items()}
  chunks=0
  for row in rows:
   if row.get('event')!='chunk':continue
   require(row['asset_id'] in approved,'stream emitted an unrequested asset')
   chunks+=1;entry=approved[row['asset_id']];w=row['window'];whole=entry['window'];y=w['y']-whole['y'];x=w['x']-whole['x']
   require(w['height']>0 and w['width']>0 and x>=0 and y>=0 and x+w['width']<=whole['width'] and y+w['height']<=whole['height'],'stream chunk escapes approved window')
   covered=coverage[row['asset_id']][y:y+w['height'],x:x+w['width']]
   require(not np.any(covered),'stream emitted duplicate or overlapping cells')
   covered[:]=True
   selection=(0,slice(y,y+w['height']),slice(x,x+w['width']));group=native_root[entry['group']]
   actual=np.frombuffer(base64.b64decode(row['values_le_base64']),dtype='u1').reshape(w['height'],w['width']);np.testing.assert_array_equal(actual,group['values'][selection])
   np.testing.assert_array_equal(np.frombuffer(base64.b64decode(row['aoi_mask_base64']),dtype='u1').reshape(actual.shape),group['aoi_mask'][selection])
   source_valid=(actual!=255).astype('u1')
   np.testing.assert_array_equal(np.frombuffer(base64.b64decode(row['source_valid_mask_base64']),dtype='u1').reshape(actual.shape),source_valid)
   require(row['valid_pixels']==int(np.count_nonzero(group['valid'][selection])),'stream validity count differs from approved output')
   expected_policies={name.removeprefix('quality_') for name in group.array_keys() if name.startswith('quality_')}
   require(set(row['quality'])==expected_policies,'stream quality policy coverage differs from approved output')
   for name,quality in row['quality'].items():np.testing.assert_array_equal(np.frombuffer(base64.b64decode(quality['mask_base64']),dtype='u1').reshape(actual.shape),group['quality_'+name][selection])
   require(row['provisional'],'stream chunk lacks provisional marker')
  require(all(np.all(mask) for mask in coverage.values()),'stream omitted requested cells or layers')
  result['stream_values']={'status':'passed','chunks':chunks,'values_aoi_quality_equal':True,'source_valid_masks_equal':True,'complete_nonoverlapping_coverage':{asset:int(np.count_nonzero(mask)) for asset,mask in coverage.items()},'completion_versions_validated':True,'wire':dict(server.counts)}
  faults=[]
  for mode in ('source_mutation','cancellation'):
   server.reset();server.latency=.02;fault_output=output.with_name(output.name+'.'+mode);require(not fault_output.exists(),'fault destination already exists')
   command=[str(binary.resolve()),'fetch',str(request_path),'--output',str(fault_output),'--output-format','zarr_v2','--backend','native','--json']
   with tempfile.TemporaryFile() as stdout,tempfile.TemporaryFile() as stderr:
    process=subprocess.Popen(command,cwd=ROOT,stdin=subprocess.DEVNULL,stdout=stdout,stderr=stderr)
    deadline=time.monotonic()+20
    while process.poll() is None and server.counts['get']<5 and time.monotonic()<deadline:time.sleep(.01)
    require(process.poll() is None,'fault completed before injection')
    if mode=='source_mutation':
     for item in mirror.sources.values():item['etag']='"changed-after-planning"'
    else:process.send_signal(signal.SIGINT)
    code=process.wait(timeout=20)
   for i,item in enumerate(mirror.sources.values()):item['etag']=receipt['sources'][list(BANDS)[i]]['etag']
   server.latency=0
   completed=False
   if (fault_output/'manifest.json').exists():completed=json.loads((fault_output/'manifest.json').read_text()).get('status')=='complete'
   require(code!=0 and not completed,'fault published successful completion')
   faults.append({'mode':mode,'exit_code':code,'complete_manifest_published':completed,'injected_after_gets':5,'wire':dict(server.counts)})
  for mode in ('stream_source_mutation_after_chunk','stream_consumer_cancellation_after_chunk'):
   server.reset();server.latency=.01
   command=[str(binary.resolve()),'stream',str(request_path),'--backend','native','--values','--json']
   process=subprocess.Popen(command,cwd=ROOT,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True)
   received_chunk=False;completion=False
   for line in process.stdout:
    row=json.loads(line)
    if row.get('event')=='chunk':received_chunk=True;break
   require(received_chunk and process.poll() is None,'stream fault lacked active provisional chunk')
   if mode=='stream_source_mutation_after_chunk':
    for item in mirror.sources.values():item['etag']='"changed-after-first-chunk"'
   else:process.send_signal(signal.SIGINT)
   for line in process.stdout:
    if json.loads(line).get('event')=='complete':completion=True
   code=process.wait(timeout=20)
   for i,item in enumerate(mirror.sources.values()):item['etag']=receipt['sources'][list(BANDS)[i]]['etag']
   server.latency=0
   require(code!=0 and not completion,'stream fault published complete')
   faults.append({'mode':mode,'exit_code':code,'provisional_chunk_received':received_chunk,'complete_event_published':completion,'wire':dict(server.counts)})
  result['faults']=faults
  save(DOC/'dswx-s1-frozen-validation.json',result)
  print(json.dumps(result,indent=2))
 finally:server.shutdown();server.server_close()

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--capture',action='store_true');p.add_argument('--replay',action='store_true');p.add_argument('--reference',nargs=2,type=Path);p.add_argument('--netrc',type=Path);p.add_argument('--binary',type=Path);p.add_argument('--output',type=Path);a=p.parse_args()
 if a.reference:
  require(a.output,'reference requires --output');reference(a.reference[0],a.reference[1],a.output);return
 if a.capture:
  require(a.netrc is not None,'capture requires explicit --netrc');capture(a.netrc)
 if a.replay:
  require(a.binary and a.output,'replay requires --binary and --output');replay(a.binary,a.output);return
 if a.binary:
  require(a.output and a.netrc,'CLI run requires explicit --output and --netrc');require(not a.output.exists(),'output already exists')
  command=[str(a.binary.resolve()),'fetch',str(DOC/'dswx-s1-request.json'),'--auth','earthdata','--netrc',str(a.netrc),'--output',str(a.output),'--output-format','zarr_v2','--backend','native','--json']
  r=subprocess.run(command,cwd=ROOT,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,timeout=240)
  require(r.returncode==0,'native CLI fetch failed (provider output suppressed)')
 if a.output:
  result=verify(a.output);save(DOC/'dswx-s1-native-validation.json',result);print(json.dumps(result,indent=2))

if __name__=='__main__':
 try:main()
 except Exception as error:
  # Network/library exceptions can contain temporary URLs: never print them.
  if isinstance(error,ValidationError):print(f'validation failed: {error}')
  else:print(f'validation failed: {type(error).__name__}; details suppressed to protect temporary locators')
  raise SystemExit(1)
