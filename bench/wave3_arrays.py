#!/usr/bin/env python3
"""Bounded real HDF5 captures, native CLI, independent h5py/NetCDF/PROJ/Zarr gates.

No capture action downloads whole objects. Sparse mirrors reject holes on replay.
Raw backing files are incomplete and are never admitted as complete sources.
"""
import argparse,base64,copy,hashlib,io,json,os,subprocess,sys,tempfile,time
from pathlib import Path
import h5py,netCDF4,numpy as np,requests,xarray as xr,zarr
from pyproj import CRS,Transformer
from hdf5_selective_mirror import Mirror,server
from hdf5_reference import Remote
ROOT=Path(__file__).resolve().parents[1];WORK=ROOT/'prototype-output/wave3';DOC=ROOT/'docs/products/wave3';BIN=ROOT/'target/release/tile-ripper'
class View(io.RawIOBase):
 def __init__(self,m):self.m=m;self.p=0
 def readable(self):return True
 def seekable(self):return True
 def tell(self):return self.p
 def seek(self,n,w=0):self.p=n if w==0 else self.p+n if w==1 else self.m.info['size']+n;return self.p
 def readinto(self,b):
  if not len(b):return 0
  b[:]=self.m.read(self.p,len(b));self.p+=len(b);return len(b)
def save(path,v):path.parent.mkdir(parents=True,exist_ok=True);path.write_text(json.dumps(v,indent=2,allow_nan=False)+'\n')
def invoke(args):
 with tempfile.TemporaryFile() as out,tempfile.TemporaryFile() as err:
  start=time.monotonic();p=subprocess.Popen([str(BIN),*map(str,args)],stdout=out,stderr=err,env={**os.environ,'TILE_RIPPER_HDF5_METADATA_POLICY':os.environ.get('WAVE3_METADATA_POLICY','page16k'),'TILE_RIPPER_WORKERS':'1'});_,status,usage=os.wait4(p.pid,0);p.returncode=os.waitstatus_to_exitcode(status);out.seek(0);err.seek(0)
  result=dict(code=p.returncode,stdout=out.read().decode(),stderr=err.read().decode(),wall_seconds=time.monotonic()-start,cpu_seconds=usage.ru_utime+usage.ru_stime,peak_rss_bytes=usage.ru_maxrss*(1 if sys.platform=='darwin' else 1024))
 if result['code']:raise RuntimeError(json.dumps(result))
 return result
def compact(r):return {k:v for k,v in r.items() if k not in ['stdout','stderr']}
def request(band,info,href,selection):
 filename=Path(info['url']).name;identity=filename[:-3];root=info['root_attrs'];asset=dict(href=href,etag=info['etag'],metadata={})
 return dict(product='goes19-abi-cmipc-'+band.lower(),provider='pinned',aoi=selection,start='2026-10-24T16:00:00Z',end='2026-10-24T16:05:00Z',bands=['CMI','DQF'],observations=[dict(id=identity,datetime=root['time_coverage_start'],collection='ABI-L2-CMIPC',platform='G19',processing='L2 M6',assets={n:asset for n in ['CMI','DQF']},properties={})],quality_policies=['goes_dqf_good'],buffer_bytes=33554432,chunk_edge=32,max_observations=1)
def selection(start,count,step=(1,1)):return dict(kind='array_slice',start=start,count=count,step=step,axes=['/y','/x'])
def raw_metadata(m):
 with h5py.File(View(m),'r') as f:
  def visit(n,d):
   _=dict(d.attrs)
   if isinstance(d,h5py.Dataset) and d.ndim==0:_=d[()]
  f.visititems(visit)
def verify_goes(m,path,r,manifest):
 group='observations/'+r['observations'][0]['id'];g=zarr.open_group(str(path),mode='r')[group]
 native=manifest['observations'][0]['source']['logical_request']['spatial'];start=native['start'];count=native['count'];step=native['step'];sl=tuple(slice(a,a+n*s,s) for a,n,s in zip(start,count,step))
 rows=[]
 with h5py.File(View(m),'r') as f:
  for name in ['CMI','DQF']:
   expected=f[name][sl];actual=g[name][:];assert actual.dtype==expected.dtype
   assert actual.tobytes()==expected.tobytes(),name
   fill=f[name].attrs['_FillValue'].item();valid=(expected!=fill)
   np.testing.assert_array_equal(g['source_valid_'+name][:],valid.astype('u1'))
   if name=='DQF':np.testing.assert_array_equal(g['quality_goes_dqf_good'][:],(expected==0).astype('u1'))
   rows.append(dict(name=name,dtype=str(actual.dtype),samples=actual.size,sha256=hashlib.sha256(actual.tobytes()).hexdigest(),valid_centers=int(valid.sum()),source_fill=int(fill)))
  x=f['x'][sl[1]];y=f['y'][sl[0]];np.testing.assert_array_equal(g['x'][:],x);np.testing.assert_array_equal(g['y'][:],y);assert g['x'].dtype==x.dtype and g['y'].dtype==y.dtype
  xx=x.astype('f8')*f['x'].attrs['scale_factor'].item()+f['x'].attrs['add_offset'].item();yy=y.astype('f8')*f['y'].attrs['scale_factor'].item()+f['y'].attrs['add_offset'].item()
  a={k:v.item() if isinstance(v,np.ndarray) and v.size==1 else v.decode() if isinstance(v,bytes) else v for k,v in f['goes_imager_projection'].attrs.items()}
  crs=CRS.from_cf(a);transform=Transformer.from_crs(crs,4326,always_xy=True);X,Y=np.meshgrid(xx*a['perspective_point_height'],yy*a['perspective_point_height']);lon,lat=transform.transform(X,Y);earth=np.isfinite(lon)&np.isfinite(lat)
  np.testing.assert_array_equal(g['earth_visible'][:],earth.astype('u1'))
  if earth.any():
   error=max(float(np.max(np.abs(g['longitude'][:][earth]-lon[earth]))),float(np.max(np.abs(g['latitude'][:][earth]-lat[earth]))));assert error<1e-8,error
  else:error=0.
  aoi=np.ones(earth.shape,dtype=bool)
  if r['aoi']['kind']=='bounds':
   w,s,e,n=r['aoi']['bbox'];aoi&=earth&(lon>=w)&(lon<=e)&(lat>=s)&(lat<=n)
  np.testing.assert_array_equal(g['aoi_mask'][:],aoi.astype('u1'))
  assert g['spatial_ref'].attrs['perspective_point_height']==a['perspective_point_height']
  assert float(g['t'][()])==f['t'][()]
 # Mature NetCDF-C sees original storage codes with automatic unsigned/packing disabled.
 with netCDF4.Dataset(m.path) as f:
  f.set_auto_maskandscale(False)
  for name in ['CMI','DQF']:assert f[name][sl].tobytes()==g[name][:].tobytes()
 with xr.open_zarr(str(path),group=group,consolidated=False,decode_cf=False,chunks=None) as ds:
  ds.load();assert ds.CMI.dtype==np.dtype('int16') and ds.DQF.dtype==np.dtype('int8')
 files=[p for p in path.rglob('*') if p.is_file()]
 return dict(native_selection=native,variables=rows,coordinate_max_error_degrees=error,visible_centers=int(earth.sum()),included_centers=int(aoi.sum()),netcdf4_raw_equal=True,xarray_reopen=True,output_objects=len(files),output_bytes=sum(p.stat().st_size for p in files))
def verify_stream(events,output,r,native):
 assert events[-1]['event']=='complete'
 group=zarr.open_group(str(output),mode='r')['observations/'+r['observations'][0]['id']];counts={};coverage={}
 for e in events:
  if e['event']!='chunk':continue
  name=e['variable']['path'].rsplit('/',1)[-1];win=e['window'];row=(win['y']-native['start'][0])//native['step'][0];col=(win['x']-native['start'][1])//native['step'][1];sl=(slice(row,row+win['height']),slice(col,col+win['width']));expected=group[name][sl]
  actual=np.frombuffer(base64.b64decode(e['values_le_base64']),dtype=np.dtype(e['dtype']).newbyteorder('<')).reshape(expected.shape);assert actual.dtype==expected.dtype and actual.tobytes()==expected.tobytes()
  for field,key in [('source_valid_mask_base64','source_valid_'+name),('aoi_mask_base64','aoi_mask')]:assert base64.b64decode(e[field])==group[key][sl].tobytes()
  if 'quality' in e:assert base64.b64decode(e['quality']['mask_base64'])==group['quality_'+e['quality']['policy']][sl].tobytes()
  if name not in coverage:coverage[name]=np.zeros(group[name].shape,dtype='u1')
  assert not coverage[name][sl].any();coverage[name][sl]=1;counts[name]=counts.get(name,0)+actual.size
 assert all(mask.all() for mask in coverage.values()) and len(counts)==len(r['bands'])
 return dict(native_samples=counts,all_raw_bits_and_masks_equal_to_independently_verified_output=True)
def goes(capture=False,live=False):
 results=[]
 for band in ['C13','C01']:
  info=json.loads((WORK/(band+'-inspection.json')).read_text());m=Mirror(WORK/('goes-'+band),info['url'] if capture else None,cap=8*1024*1024)
  # Source metadata needed by independently executed NetCDF-C is captured too.
  if capture:raw_metadata(m)
  cases=[('center',selection([900,1200],[32,32])),('space',selection([0,0],[8,8])),('strided',selection([900,1200],[9,11],[3,2])),('geographic',dict(kind='bounds',crs='EPSG:4326',bbox=[-80.,34.,-79.9,34.1]))]
  with server(m) as s:
   for label,aoi in cases:
    r=request(band,info,s.url,aoi);stamp=str(time.time_ns());req=WORK/f'{band}-{label}-request.json';save(req,r);output=WORK/f'{band}-{label}-{stamp}.zarr'
    plan=invoke(['plan',req]);fetch=invoke(['fetch',req,'--output',output]);manifest=json.loads(fetch['stdout']);verified=verify_goes(m,output,r,manifest)
    stream=invoke(['stream',req,'--values']);events=[json.loads(line) for line in stream['stdout'].splitlines()];assert events[-1]['event']=='complete'
    verified['stream']=verify_stream(events,output,r,manifest['plan']['observations'][0]['native_selection'])
    results.append(dict(band=band,case=label,plan=compact(plan),fetch=compact(fetch),stream=compact(stream),verification=verified,execution=manifest['observations'][0]['execution'],preparation_timings=manifest['plan']['observations'][0].get('preparation_timings'),output_io=manifest['output_io'],mirror_requests=len(s.events)))
    canonical=request(band,info,info['url'],aoi);canonical['provider']='auto';save(DOC/f'goes-{band.lower()}-{label}-request.json',canonical)
   if s.errors:raise RuntimeError(str(s.errors))
  if capture:m.seal()
  m.verify()
  save(DOC/f'goes-{band.lower()}-capture.json',m.info)
  # Physical inspection arrays are retained in output receipts; avoid thousands
  # of coordinate codes in the compact report.
  inspect=copy.deepcopy(info)
  for n in ['x','y']:inspect['datasets'][n]['values']=[inspect['datasets'][n]['values'][0],inspect['datasets'][n]['values'][-1]]
  save(DOC/f'goes-{band.lower()}-inspection.json',inspect)
  if live:
   r=request(band,info,info['url'],cases[0][1]);r['provider']='auto';req=WORK/f'{band}-live-request.json';save(req,r);output=WORK/f'{band}-live-{time.time_ns()}.zarr';run=invoke(['fetch',req,'--output',output]);manifest=json.loads(run['stdout']);results.append(dict(band=band,case='live-noaa',fetch=compact(run),verification=verify_goes(m,output,r,manifest),execution=manifest['observations'][0]['execution'],preparation_timings=manifest['plan']['observations'][0].get('preparation_timings')))
 save(DOC/('goes-live-validation.json' if live else 'goes-validation.json'),dict(status='passed',reference_versions=dict(h5py=h5py.__version__,netCDF4=netCDF4.__version__,zarr=zarr.__version__,xarray=xr.__version__),scope='two independent GOES-19 CMIPC band/grid specimens; bounded original source captures; raw codes plus CF unsigned/packing annotations',cases=results))
 print(json.dumps(dict(status='passed',cases=len(results))),flush=True)
def smap_request(passname,info,href,aoi):
 id='SMAP_L3_SM_P_E_20261023_R19240_001';asset=dict(href=href,etag=info['etag'],metadata={});bands=['soil_moisture','retrieval_qual_flag','surface_flag','freeze_thaw_fraction']
 return dict(product='smap-spl3smp-e-v6-'+passname,provider='pinned',aoi=aoi,start='2026-10-23T00:00:00Z',end='2026-10-23T23:59:59Z',bands=bands,quality_policies=['smap_retrieval_recommended'],observations=[dict(id=id,datetime='2026-10-23T00:00:00.000Z',collection='SPL3SMP_E',platform='SMAP',processing='006/R19240',assets={b:asset for b in bands},properties={})],buffer_bytes=67108864,chunk_edge=32,max_observations=1)
def smap_slice(start,count,step=(1,1)):
 return dict(kind='array_slice',start=start,count=count,step=step,axes=['/@smap_ease2_m09/y','/@smap_ease2_m09/x'])
def verify_smap(m,path,r,manifest):
 group='observations/'+r['observations'][0]['id'];g=zarr.open_group(str(path),mode='r')[group];native=manifest['observations'][0]['source']['logical_request']['spatial'];a=native['start'];n=native['count'];step=native['step'];sl=tuple(slice(a,a+n*s,s) for a,n,s in zip(a,n,step));passname='PM' if r['product'].endswith('-pm') else 'AM';suffix='_pm' if passname=='PM' else '';sourcegroup='Soil_Moisture_Retrieval_Data_'+passname;refs=[]
 with h5py.File(View(m),'r') as f:
  for b in r['bands']:
   name=b+suffix;d=f[sourcegroup+'/'+name];expected=d[sl];actual=g[name][:];assert actual.dtype==expected.dtype;assert actual.tobytes()==expected.tobytes(),name
   valid=expected!=d.attrs['_FillValue'];np.testing.assert_array_equal(g['source_valid_'+name][:],valid.astype('u1'))
   refs.append(dict(name=name,dtype=str(actual.dtype),samples=actual.size,valid_centers=int(valid.sum()),sha256=hashlib.sha256(actual.tobytes()).hexdigest()))
  q=f[sourcegroup+'/retrieval_qual_flag'+suffix][sl];expectedq=np.isin(q,[0,8]);np.testing.assert_array_equal(g['quality_smap_retrieval_recommended'][:],expectedq.astype('u1'))
  expectedx=-17367530.4451615+(a[1]+np.arange(n[1])*step[1]+.5)*9008.055210146;expectedy=7314540.8306386-(a[0]+np.arange(n[0])*step[0]+.5)*9008.055210146
  np.testing.assert_array_equal(g['x'][:],expectedx);np.testing.assert_array_equal(g['y'][:],expectedy)
  X,Y=np.meshgrid(expectedx,expectedy);lon,lat=Transformer.from_crs(6933,4326,always_xy=True).transform(X,Y);error=max(float(np.abs(g['longitude'][:]-lon).max()),float(np.abs(g['latitude'][:]-lat).max()));assert error<2e-8,error
  srclat=f[sourcegroup+'/latitude'+suffix][sl];srclon=f[sourcegroup+'/longitude'+suffix][sl];ok=(srclat!=-9999)&(srclon!=-9999);sourceerr=max(float(np.abs(srclat[ok]-lat[ok]).max()),float(np.abs(srclon[ok]-lon[ok]).max())) if ok.any() else 0.;assert sourceerr<5e-6,sourceerr
  mask=np.ones(X.shape,dtype=bool)
  if r['aoi']['kind']=='bounds':
   w,s,e,nn=r['aoi']['bbox'];mask&=(lon>=w)&(lon<=e)&(lat>=s)&(lat<=nn)
  np.testing.assert_array_equal(g['aoi_mask'][:],mask.astype('u1'));np.testing.assert_array_equal(g['earth_visible'][:],np.ones(X.shape,dtype='u1'))
  assert CRS.from_cf(dict(g['spatial_ref'].attrs)).to_epsg()==6933
  meta=g.attrs['product_metadata'];assert meta['/Metadata/DatasetIdentification']['ECSVersionID']['value']=='006';assert meta['/Metadata/DatasetIdentification']['CompositeReleaseID']['value']=='R19240'
 with xr.open_zarr(str(path),group=group,decode_cf=False,consolidated=False,chunks=None) as ds:ds.load();assert ds['soil_moisture'+suffix].dtype==np.dtype('f4')
 files=[p for p in path.rglob('*') if p.is_file()]
 return dict(native_selection=native,variables=refs,quality_accepted=int(expectedq.sum()),coordinate_max_error_degrees=error,source_coordinate_rounding_error_degrees=sourceerr,source_coordinate_nonfill_centers=int(ok.sum()),included_centers=int(mask.sum()),xarray_reopen=True,output_objects=len(files),output_bytes=sum(p.stat().st_size for p in files))
def smap(live=False):
 os.environ["WAVE3_METADATA_POLICY"]="page4k"
 info=json.loads((WORK/'smap/global/inspection.json').read_text());m=Mirror(WORK/'smap/global');m.verify();results=[]
 # A geographic envelope strictly between native centers, independently derived by PROJ.
 xx=-17367530.4451615+(np.array([901.75,905.25])+.5)*9008.055210146;yy=7314540.8306386-(np.array([401.75,405.25])+.5)*9008.055210146
 lon,lat=Transformer.from_crs(6933,4326,always_xy=True).transform(xx,yy);bbox=[float(lon.min()),float(lat.min()),float(lon.max()),float(lat.max())]
 for passname in ['am','pm']:
  cases=[('texas',smap_slice([400,900],[8,8])),('gulf',smap_slice([400,1000],[8,8])),('strided',smap_slice([400,900],[3,4],[3,2])),('geographic',dict(kind='bounds',crs='EPSG:4326',bbox=bbox))]
  with server(m) as srv:
   for label,aoi in cases:
    r=smap_request(passname,info,srv.url,aoi);req=WORK/f'smap-{passname}-{label}.json';save(req,r);out=WORK/f'smap-{passname}-{label}-{time.time_ns()}.zarr';plan=invoke(['plan',req]);run=invoke(['fetch',req,'--output',out]);manifest=json.loads(run['stdout']);verify=verify_smap(m,out,r,manifest);stream=invoke(['stream',req,'--values']);events=[json.loads(v) for v in stream['stdout'].splitlines()];verify['stream']=verify_stream(events,out,r,manifest['plan']['observations'][0]['native_selection']);results.append(dict(passname=passname,case=label,plan=compact(plan),fetch=compact(run),stream=compact(stream),verification=verify,execution=manifest['observations'][0]['execution'],preparation_timings=manifest['plan']['observations'][0].get('preparation_timings'),output_io=manifest['output_io']))
    canonical=smap_request(passname,info,info['canonical_url'],aoi);canonical['provider']='earthdata';save(DOC/f'smap-{passname}-{label}-request.json',canonical)
   assert not srv.errors,srv.errors
  if live:
   req=WORK/f'smap-{passname}-live.json';r=smap_request(passname,info,info['canonical_url'],cases[0][1]);r['provider']='earthdata';save(req,r);out=WORK/f'smap-{passname}-live-{time.time_ns()}.zarr';run=invoke(['fetch',req,'--auth','earthdata','--netrc',Path.home()/'.netrc','--output',out]);manifest=json.loads(run['stdout']);results.append(dict(passname=passname,case='live-nsidc',fetch=compact(run),verification=verify_smap(m,out,r,manifest),execution=manifest['observations'][0]['execution'],preparation_timings=manifest['plan']['observations'][0].get('preparation_timings')))
 save(DOC/'smap-capture.json',m.info)
 # Full attributes retained; compact windows become hashes/value counts in acceptance.
 inspection=copy.deepcopy(info)
 for d in inspection['datasets'].values():
  for k in list(d):
   if k.startswith('window_'):del d[k]
 save(DOC/'smap-inspection.json',inspection)
 save(DOC/('smap-live-validation.json' if live else 'smap-validation.json'),dict(status='passed',cases=results,scope='global AM and PM v006/R19240 from one daily granule; EPSG6931 polar grid unqualified',raw_reference='range-backed h5py; original HDF5 C library; source coordinate fields independently compared with official grid plus PROJ'))
 print(json.dumps(dict(status='passed',smap_cases=len(results))),flush=True)
def nisar_request(info,href,start,count,step=(1,1)):
 asset=dict(href=href,etag=info['etag'],metadata={})
 return dict(product='nisar-gcov-p1-fa-hhhh',provider='pinned',aoi=dict(kind='array_slice',start=start,count=count,step=step,axes=['/science/LSAR/GCOV/grids/frequencyA/yCoordinates','/science/LSAR/GCOV/grids/frequencyA/xCoordinates']),start='2026-10-22T10:55:00Z',end='2026-10-22T10:57:00Z',bands=['HHHH'],observations=[dict(id=info['granule'],datetime='2026-10-22T10:55:46.000000000Z',collection=info['collection'],platform='NISAR',processing='P05023',assets={'HHHH':asset},properties={})],quality_policies=[],buffer_bytes=67108864,chunk_edge=32,max_observations=1)
def verify_nisar(m,path,r,manifest):
 group='observations/'+r['observations'][0]['id'];g=zarr.open_group(str(path),mode='r')[group];sl=tuple(slice(a,a+n*s,s) for a,n,s in zip(r['aoi']['start'],r['aoi']['count'],r['aoi']['step']));root='/science/LSAR/GCOV/grids/frequencyA'
 with h5py.File(View(m)) as f:
  expected=f[root+'/HHHH'][sl];actual=g['HHHH'][:];assert actual.dtype==np.dtype('f4') and actual.tobytes()==expected.tobytes()
  valid=~np.isnan(expected);np.testing.assert_array_equal(g['source_valid_HHHH'][:],valid.astype('u1'));np.testing.assert_array_equal(g['aoi_mask'][:],np.ones(expected.shape,dtype='u1'))
  x=f[root+'/xCoordinates'][sl[1]];y=f[root+'/yCoordinates'][sl[0]];np.testing.assert_array_equal(g['x'][:],x);np.testing.assert_array_equal(g['y'][:],y);assert g['x'].dtype==np.dtype('f8') and g['y'].dtype==np.dtype('f8')
  X,Y=np.meshgrid(x,y);lon,lat=Transformer.from_crs(32616,4326,always_xy=True).transform(X,Y);error=max(float(np.abs(g['longitude'][:]-lon).max()),float(np.abs(g['latitude'][:]-lat).max()));assert error<1e-8,error
  assert int(g['spatial_ref'][()])==32616 and g['spatial_ref'].dtype==np.dtype('u4')
  meta=g.attrs['product_metadata'];p='/science/LSAR/GCOV/metadata/processingInformation/parameters/rtc/';assert meta[p+'outputBackscatterNormalizationConvention']['value']=='gamma0';assert meta[p+'outputBackscatterExpressionConvention']['value']=='Linear power';assert meta['/science/LSAR/GCOV/metadata/calibrationInformation/frequencyA/HH/scaleFactor']['value']==27030.051422175486
 with xr.open_zarr(str(path),group=group,decode_cf=False,consolidated=False,chunks=None) as ds:ds.load();assert ds.HHHH.dtype==np.dtype('f4')
 files=[p for p in path.rglob('*') if p.is_file()]
 return dict(dtype=str(actual.dtype),samples=actual.size,raw_bits_sha256=hashlib.sha256(actual.tobytes()).hexdigest(),valid_centers=int(valid.sum()),coordinate_max_error_degrees=error,xarray_reopen=True,storage_fill=0,cf_fill='NaN',output_objects=len(files),output_bytes=sum(p.stat().st_size for p in files))
def nisar(live=False):
 os.environ['WAVE3_METADATA_POLICY']='page16k';info=json.loads((WORK/'nisar/inspection.json').read_text());m=Mirror(WORK/'nisar');m.verify();results=[]
 with server(m) as srv:
  for name,start,count,step in [('center',[17388,17676],[8,8],[1,1]),('corner',[0,0],[8,8],[1,1]),('strided',[17388,17676],[3,3],[3,3])]:
   r=nisar_request(info,srv.url,start,count,step);req=WORK/f'nisar-{name}.json';save(req,r);out=WORK/f'nisar-{name}-{time.time_ns()}.zarr';plan=invoke(['plan',req]);run=invoke(['fetch',req,'--output',out]);manifest=json.loads(run['stdout']);verified=verify_nisar(m,out,r,manifest);stream=invoke(['stream',req,'--values']);events=[json.loads(v) for v in stream['stdout'].splitlines()];verified['stream']=verify_stream(events,out,r,manifest['plan']['observations'][0]['native_selection']);results.append(dict(case=name,plan=compact(plan),fetch=compact(run),stream=compact(stream),verification=verified,execution=manifest['observations'][0]['execution'],preparation_timings=manifest['plan']['observations'][0].get('preparation_timings'),output_io=manifest['output_io']))
   canonical=nisar_request(info,info['canonical_url'],start,count,step);canonical['provider']='earthdata';save(DOC/f'nisar-{name}-request.json',canonical)
  assert not srv.errors,srv.errors
 if live:
  r=nisar_request(info,info['canonical_url'],[17388,17676],[8,8]);r['provider']='earthdata';req=WORK/'nisar-live.json';save(req,r);out=WORK/f'nisar-live-{time.time_ns()}.zarr';run=invoke(['fetch',req,'--auth','earthdata','--netrc',Path.home()/'.netrc','--output',out]);manifest=json.loads(run['stdout']);results.append(dict(case='live-asf',fetch=compact(run),verification=verify_nisar(m,out,r,manifest),execution=manifest['observations'][0]['execution'],preparation_timings=manifest['plan']['observations'][0].get('preparation_timings')))
 save(DOC/'nisar-capture.json',m.info);save(DOC/'nisar-inspection.json',info);save(DOC/'nisar-metadata-inspection.json',json.loads((WORK/'nisar/metadata-extension.json').read_text()));save(DOC/'nisar-fill-oracle.json',json.loads((WORK/'nisar/fill-oracle.json').read_text()))
 save(DOC/('nisar-live-validation.json' if live else 'nisar-validation.json'),dict(status='passed',scope='one real provisional GCOV P05023/product1.0.11/spec1.5.0; frequencyA HHHH only, original UTM16N native-index selection',cases=results,netcdf4_real_product='sparse-source NetCDF-C open unsupported; no real NetCDF-C equality claim; independent HDF5 C/h5py reference and separate NetCDF-C fill fixtures pass'))
 print(json.dumps(dict(status='passed',nisar_cases=len(results))),flush=True)
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('family',choices=['goes','smap','nisar']);p.add_argument('--capture',action='store_true');p.add_argument('--live',action='store_true');a=p.parse_args()
 if a.family=='goes':goes(a.capture,a.live)
 elif a.family=='smap':smap(a.live)
 else:nisar(a.live)
