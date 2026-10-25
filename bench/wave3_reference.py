#!/usr/bin/env python3
"""Isolated mature HDF5→PROJ→Zarr process with conditional source validation.

This contains no native acquisition/decoder calls. Only the bounded HTTP control
is shared. It preserves raw bits, native axes and distinct science/quality/AOI
masks. Timing includes Python imports and process startup when invoked externally.
"""
import json,os,sys,time,math
from pathlib import Path
import h5py,numpy as np,requests,zarr
from pyproj import CRS,Transformer
from hdf5_reference import Remote
r=json.loads(Path(sys.argv[1]).read_text());dest=Path(sys.argv[2]);assert not dest.exists();temp=dest.with_name('.'+dest.name+'.partial');assert not temp.exists();start=time.monotonic();ob=r['observations'][0];source=ob['assets'][r['bands'][0]];url=source['href'];remote=Remote(url,page=4096 if r['product'].startswith('smap') else 16384);etag=remote.etag
if source.get('etag'):assert etag==source['etag']
def clean(v):
 if isinstance(v,bytes):return v.decode(errors='replace')
 if isinstance(v,np.ndarray):return [clean(x) for x in v.flat] if v.size>1 else clean(v.item())
 if isinstance(v,np.generic):return clean(v.item())
 if isinstance(v,float) and not math.isfinite(v):return str(v)
 return v
root=zarr.open_group(str(temp),mode='w');g=root.require_group('observations/'+ob['id']);g.attrs.update(source=url,source_etag=etag,observation=ob,Conventions='CF-1.8');edge=r['chunk_edge'];policy=r.get('quality_policies',[]);datasetmeta={}
with h5py.File(remote,'r') as f:
 product=r['product'];kind='goes' if product.startswith('goes') else 'smap' if product.startswith('smap') else 'nisar';suffix='_pm' if product.endswith('-pm') else ''
 if kind=='goes':
  paths=r['bands'];xall=f['x'][:];yall=f['y'][:];a={k:clean(v) for k,v in f['goes_imager_projection'].attrs.items()};crs=CRS.from_cf(a);height=a['perspective_point_height'];px=f['x'].attrs;py=f['y'].attrs;xp=[float(px['scale_factor'].item()),float(px['add_offset'].item())];yp=[float(py['scale_factor'].item()),float(py['add_offset'].item())]
 elif kind=='smap':
  sourcegroup='Soil_Moisture_Retrieval_Data_'+('PM' if suffix else 'AM');paths=[sourcegroup+'/'+b+suffix for b in r['bands']];xall=-17367530.4451615+(np.arange(3856,dtype='f8')+.5)*9008.055210146;yall=7314540.8306386-(np.arange(1624,dtype='f8')+.5)*9008.055210146;crs=CRS.from_epsg(6933);height=1.;xp=[1.,0.];yp=[1.,0.];a=crs.to_cf()
  g.attrs['source_metadata']={p:{k:clean(v) for k,v in f[p].attrs.items()} for p in ['Metadata/DatasetIdentification','Metadata/Extent','Metadata/ProcessStep','Metadata/GridSpatialRepresentation']}
 else:
  base='/science/LSAR/GCOV/grids/frequencyA';paths=[base+'/HHHH'];xall=f[base+'/xCoordinates'][:];yall=f[base+'/yCoordinates'][:];a={k:clean(v) for k,v in f[base+'/projection'].attrs.items()};crs=CRS.from_epsg(int(f[base+'/projection'][()]));height=1.;xp=[1.,0.];yp=[1.,0.]
 transform=Transformer.from_crs(crs,4326,always_xy=True)
 def nav(ys,xs):
  X,Y=np.meshgrid((xs.astype('f8')*xp[0]+xp[1])*height,(ys.astype('f8')*yp[0]+yp[1])*height);lon,lat=transform.transform(X,Y);earth=np.isfinite(lon)&np.isfinite(lat);lon[~earth]=np.nan;lat[~earth]=np.nan;return lon,lat,earth
 if r['aoi']['kind']=='array_slice':selection=r['aoi']
 else:
  # Independent geographic envelope determination, scanning bounded row strips.
  w,s,e,n=r['aoi']['bbox'];lo=[len(yall),len(xall)];hi=[-1,-1]
  for row in range(0,len(yall),128):
   lon,lat,earth=nav(yall[row:row+128],xall);mask=earth&(lon>=w)&(lon<=e)&(lat>=s)&(lat<=n);iy,ix=np.where(mask)
   if iy.size:lo=[min(lo[0],row+int(iy.min())),min(lo[1],int(ix.min()))];hi=[max(hi[0],row+int(iy.max())),max(hi[1],int(ix.max()))]
  assert hi[0]>=lo[0];selection=dict(start=lo,count=[hi[i]-lo[i]+1 for i in range(2)],step=[1,1])
 at=selection['start'];count=selection['count'];step=selection['step'];slices=tuple(slice(a,a+n*s,s) for a,n,s in zip(at,count,step));x=xall[slices[1]];y=yall[slices[0]]
 def array(name,values,attrs=None):
  values=np.asarray(values);chunks=tuple(min(edge,n) for n in values.shape);ds=g.create_dataset(name,shape=values.shape,chunks=chunks,dtype=values.dtype,compressor=None,fill_value=None,overwrite=False);ds[...]=values;ds.attrs['_ARRAY_DIMENSIONS']=['y','x'] if values.ndim==2 else ['y'] if name in ['y','native_row'] else ['x'] if values.ndim==1 else []
  if attrs:ds.attrs.update(attrs)
  return ds
 for path in paths:
  d=f[path];name=path.rsplit('/',1)[-1];values=d[slices];attr={k:clean(v) for k,v in d.attrs.items() if k not in ['DIMENSION_LIST','REFERENCE_LIST']};array(name,values,attr);fill=d.attrs.get('_FillValue');valid=~np.isnan(values) if values.dtype.kind=='f' else np.ones(values.shape,dtype=bool)
  if fill is not None:valid&=values!=fill
  array('source_valid_'+name,valid.astype('u1'))
  if policy and (name=='DQF' or name in ['retrieval_qual_flag','retrieval_qual_flag_pm']):array('quality_'+policy[0],(values==0).astype('u1') if kind=='goes' else np.isin(values,[0,8]).astype('u1'))
  datasetmeta[name]=dict(shape=values.shape,dtype=str(values.dtype),source_attributes=attr)
 array('x',x,{k:clean(v) for k,v in f['x'].attrs.items() if k not in ['REFERENCE_LIST']} if kind=='goes' else None);array('y',y,{k:clean(v) for k,v in f['y'].attrs.items() if k not in ['REFERENCE_LIST']} if kind=='goes' else None);array('native_row',(at[0]+np.arange(count[0])*step[0]).astype('i8'));array('native_column',(at[1]+np.arange(count[1])*step[1]).astype('i8'))
 longitude,latitude,earth=nav(y,x);mask=np.ones(earth.shape,dtype=bool)
 if r['aoi']['kind']=='bounds':w,s,e,n=r['aoi']['bbox'];mask&=earth&(longitude>=w)&(longitude<=e)&(latitude>=s)&(latitude<=n)
 array('aoi_mask',mask.astype('u1'));array('earth_visible',earth.astype('u1'));array('longitude',longitude);array('latitude',latitude);array('spatial_ref',np.asarray(32616,dtype='u4') if kind=='nisar' else np.asarray(0,dtype='i4'),a)
 if kind=='goes':
  for name in ['t','band_id','band_wavelength']:array(name,np.asarray(f[name][()]).reshape(()),{k:clean(v) for k,v in f[name].attrs.items() if k not in ['DIMENSION_LIST','REFERENCE_LIST']})
 g.attrs.update(native_selection=selection,source_root_attributes={k:clean(v) for k,v in f.attrs.items()},datasets=datasetmeta)
# Source validation is equivalent conditional identity checking, not a whole-file hash.
q=remote.session.head(url,headers={'If-Match':etag},timeout=30);assert q.status_code==200 and q.headers['ETag']==etag and int(q.headers['Content-Length'])==remote.size
report=dict(status='complete',seconds_before_publication=time.monotonic()-start,source_gets=len(remote.events),source_bytes=sum(b-a for a,b in remote.events),selection=selection,source_etag=etag,native_values='raw; no scale/offset or quality conversion',workflow='h5py/HDF5-C → PROJ → Zarr-Python, with conditional pre/post source identity')
root.attrs.update(status='complete');(temp/'manifest.json').write_text(json.dumps(report,indent=2)+'\n')
# Flush ordinary files and directories before atomic fresh-destination publication.
for path in temp.rglob('*'):
 if path.is_file():
  with path.open('rb') as f:os.fsync(f.fileno())
for path in sorted([p for p in temp.rglob('*') if p.is_dir()],key=lambda p:len(p.parts),reverse=True)+[temp]:
 fd=os.open(path,os.O_RDONLY);os.fsync(fd);os.close(fd)
if sys.platform=='darwin':
 import fcntl
 with (temp/'manifest.json').open('rb') as f:fcntl.fcntl(f.fileno(),51)
os.rename(temp,dest);fd=os.open(dest.parent,os.O_RDONLY);os.fsync(fd);os.close(fd)
if sys.platform=='darwin':
 with (dest/'manifest.json').open('rb') as f:fcntl.fcntl(f.fileno(),51)
print(json.dumps(report))
