#!/usr/bin/env python3
"""Independent geographic mature-reader workflow; no native navigation calls.

PROJ maps a finely subdivided geographic AOI to a conservative search window,
then GEOS evaluates inverse-transformed actual native centers. This independent
candidate method is cross-checked by the full-source oracle in the harness.
"""
import json, os, sys, time
from pathlib import Path
import h5py, numpy as np, shapely, zarr
from pyproj import Transformer
from hdf5_reference import Remote
from wave4_geographic import membership, geom

started=time.monotonic();r=json.loads(Path(sys.argv[1]).read_text());dest=Path(sys.argv[2]);assert not dest.exists();stage=dest.with_name('.'+dest.name+'.partial');assert not stage.exists();ob=r['observations'][0];asset=ob['assets'][r['bands'][0]];remote=Remote(asset['href'],page=16384);assert remote.etag==asset['etag'];base='/science/LSAR/GCOV/grids/frequencyA';polar=r['product'].startswith('smap');suffix='_pm' if r['product'].endswith('-pm') else '';sourcegroup='Soil_Moisture_Retrieval_Data_Polar_'+('PM' if suffix else 'AM')
with h5py.File(remote) as f:
 t=time.monotonic();x=-9000000.+(np.arange(2000)+.5)*9000. if polar else f[base+'/xCoordinates'][:];y=9000000.-(np.arange(2000)+.5)*9000. if polar else f[base+'/yCoordinates'][:];inspection=time.monotonic()-t
 t=time.monotonic();poly=shapely.segmentize(geom(r['aoi']),.0001);ring=np.asarray(poly.exterior.coords);forward=Transformer.from_crs(4326,6931 if polar else 32616,always_xy=True);xx,yy=forward.transform(ring[:,0],ring[:,1]);ix=np.flatnonzero((x>=min(xx)-(9000 if polar else 20))&(x<=max(xx)+(9000 if polar else 20)));iy=np.flatnonzero((y>=min(yy)-(9000 if polar else 20))&(y<=max(yy)+(9000 if polar else 20)));assert ix.size and iy.size
 inverse=Transformer.from_crs(6931 if polar else 32616,4326,always_xy=True);X,Y=np.meshgrid(x[ix],y[iy]);lon,lat=inverse.transform(X,Y);mask=membership(r['aoi'],lon,lat,1e-10);rows,cols=np.where(mask);assert rows.size;at=[int(iy[rows.min()]),int(ix[cols.min()])];count=[int(rows.max()-rows.min()+1),int(cols.max()-cols.min()+1)];sl=tuple(slice(a,a+n) for a,n in zip(at,count));selection=time.monotonic()-t
 t=time.monotonic();paths=[sourcegroup+'/'+b+suffix for b in r['bands']] if polar else [base+'/HHHH'];raws={p.rsplit('/',1)[-1]:f[p][sl] for p in paths};science=time.monotonic()-t;x=x[sl[1]];y=y[sl[0]];X,Y=np.meshgrid(x,y);lon,lat=inverse.transform(X,Y);mask=membership(r['aoi'],lon,lat,1e-10)
 root=zarr.open_group(str(stage),mode='w');g=root.require_group('observations/'+ob['id']);edge=r['chunk_edge']
 def write(name,v,dims):
  a=g.create_dataset(name,shape=v.shape,chunks=tuple(min(edge,n) for n in v.shape),dtype=v.dtype,compressor=None,fill_value=None);a[...]=v;a.attrs['_ARRAY_DIMENSIONS']=dims
 t=time.monotonic()
 for path in paths:
  name=path.rsplit('/',1)[-1];raw=raws[name];fill=f[path].attrs['_FillValue'];valid=(~np.isnan(raw))&(raw!=fill);write(name,raw,['y','x']);write('source_valid_'+name,valid.astype('u1'),['y','x'])
  if name.startswith('retrieval_qual_flag'):write('quality_smap_retrieval_recommended',np.isin(raw,[0,8]).astype('u1'),['y','x'])
 for name,v in [('aoi_mask',mask.astype('u1')),('earth_visible',np.ones(mask.shape,dtype='u1')),('longitude',lon),('latitude',lat)]:write(name,v,['y','x'])
 write('x',x,['x']);write('y',y,['y']);write('native_row',(at[0]+np.arange(count[0])).astype('i8'),['y']);write('native_column',(at[1]+np.arange(count[1])).astype('i8'),['x']);write('spatial_ref',np.asarray(0,dtype='i4') if polar else np.asarray(32616,dtype='u4'),[])
 g.attrs.update(observation=ob,source_etag=remote.etag,source_attributes={k:str(v) for k,v in f[paths[0]].attrs.items()});root.attrs['status']='complete'
 output_encode=time.monotonic()-t
q=remote.session.head(asset['href'],headers={'If-Match':remote.etag},timeout=30);assert q.status_code==200 and q.headers['ETag']==remote.etag and int(q.headers['Content-Length'])==remote.size
report=dict(status='complete',inspection_seconds=inspection,geographic_selection_seconds=selection,science_read_decode_seconds=science,output_encode_seconds=output_encode,source_gets=len(remote.events),source_heads=2,source_bytes=sum(b-a for a,b in remote.events),native_selection=dict(start=at,count=count,step=[1,1]),included_centers=int(mask.sum()),source_version_validated=True)
(stage/'manifest.json').write_text(json.dumps(report,indent=2)+'\n')
for p in stage.rglob('*'):
 if p.is_file():
  with p.open('rb') as handle:os.fsync(handle.fileno())
for p in sorted([p for p in stage.rglob('*') if p.is_dir()],key=lambda p:len(p.parts),reverse=True)+[stage]:
 fd=os.open(p,os.O_RDONLY);os.fsync(fd);os.close(fd)
if sys.platform=='darwin':
 import fcntl
 with (stage/'manifest.json').open('rb') as handle:fcntl.fcntl(handle.fileno(),51)
os.rename(stage,dest);fd=os.open(dest.parent,os.O_RDONLY);os.fsync(fd);os.close(fd)
if sys.platform=='darwin':
 with (dest/'manifest.json').open('rb') as handle:fcntl.fcntl(handle.fileno(),51)
report['end_to_end_seconds_after_imports']=time.monotonic()-started;print(json.dumps(report))
