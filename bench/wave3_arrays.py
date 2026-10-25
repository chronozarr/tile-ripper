#!/usr/bin/env python3
"""Bounded real HDF5 captures, native CLI, independent h5py/NetCDF/PROJ/Zarr gates.

No capture action downloads whole objects. Sparse mirrors reject holes on replay.
Raw backing files are incomplete and are never admitted as complete sources.
"""
import argparse,copy,hashlib,io,json,os,subprocess,sys,tempfile,time
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
  start=time.monotonic();p=subprocess.Popen([str(BIN),*map(str,args)],stdout=out,stderr=err,env={**os.environ,'TILE_RIPPER_HDF5_METADATA_POLICY':'page16k','TILE_RIPPER_WORKERS':'1'});_,status,usage=os.wait4(p.pid,0);p.returncode=os.waitstatus_to_exitcode(status);out.seek(0);err.seek(0)
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
    results.append(dict(band=band,case=label,plan=compact(plan),fetch=compact(fetch),stream=compact(stream),verification=verified,execution=manifest['observations'][0]['execution'],output_io=manifest['output_io'],mirror_requests=len(s.events)))
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
   r=request(band,info,info['url'],cases[0][1]);r['provider']='auto';req=WORK/f'{band}-live-request.json';save(req,r);output=WORK/f'{band}-live-{time.time_ns()}.zarr';run=invoke(['fetch',req,'--output',output]);manifest=json.loads(run['stdout']);results.append(dict(band=band,case='live-noaa',fetch=compact(run),verification=verify_goes(m,output,r,manifest),execution=manifest['observations'][0]['execution']))
 save(DOC/'goes-validation.json',dict(status='passed',reference_versions=dict(h5py=h5py.__version__,netCDF4=netCDF4.__version__,zarr=zarr.__version__,xarray=xr.__version__),scope='two independent GOES-19 CMIPC band/grid specimens; bounded original source captures; raw codes plus CF unsigned/packing annotations',cases=results))
 print(json.dumps(dict(status='passed',cases=len(results))),flush=True)
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('family',choices=['goes']);p.add_argument('--capture',action='store_true');p.add_argument('--live',action='store_true');a=p.parse_args();goes(a.capture,a.live)
