#!/usr/bin/env python3
"""Bounded real-source inspections. Exact auth is reused; no secrets are saved."""
import argparse, ctypes, json, math, shutil, time
from pathlib import Path
import h5py, numpy as np, requests
from hdf5_selective_mirror import Mirror
from wave3_arrays import View, save
from wave3_capture import ExactSession

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / 'prototype-output/wave4'
DOC = ROOT / 'docs/products/wave4'

def clean(v):
 if isinstance(v, (bytes, np.bytes_)): return bytes(v).decode()
 if isinstance(v, np.ndarray): return clean(v.tolist())
 if isinstance(v, np.generic): return clean(v.item())
 if isinstance(v, float) and not math.isfinite(v): return str(v)
 if isinstance(v, (list, tuple)): return [clean(x) for x in v]
 if isinstance(v, dict): return {k:clean(x) for k,x in v.items()}
 return v

def record(d, values=False):
 p=d.id.get_create_plist()
 r=dict(shape=list(d.shape),dtype=str(d.dtype),chunks=d.chunks,filters=[p.get_filter(i) for i in range(p.get_nfilters())],storage_initialization=clean(d.fillvalue),fill_value_defined=p.fill_value_defined(),fill_time=p.get_fill_time(),attributes={k:clean(v) for k,v in d.attrs.items() if k not in ['DIMENSION_LIST','REFERENCE_LIST']})
 if d.chunks:
  index=ctypes.c_int(-1);fn=ctypes.CDLL(h5py.h5d.__file__).H5Dget_chunk_index_type;fn.argtypes=[ctypes.c_int64,ctypes.POINTER(ctypes.c_int)];fn.restype=ctypes.c_int
  assert fn(d.id.id,ctypes.byref(index))==0;r['chunk_index_enum']=index.value;r['allocated_chunks']=d.id.get_num_chunks()
 if d.size <= 16: r['value']=clean(d[()])
 elif values:
  v=d[:];r['first_last']=clean(v[[0,-1]]);r['uniform_step']=float(v[1]-v[0]);r['uniform']=bool(np.all(np.diff(v)==v[1]-v[0]))
 return clean(r)

def seeded(source,destination):
 """Copy only known captured extents into a fresh sparse mirror."""
 old=Mirror(source);old.verify();destination.mkdir(parents=True,exist_ok=False)
 with old.path.open('rb') as src,(destination/'source.nc').open('wb') as dst:
  dst.truncate(old.info['size'])
  for a,b in old.extents:src.seek(a);dst.seek(a);dst.write(src.read(b-a))
 shutil.copyfile(old.manifest,destination/'manifest.json')
 return old.info['url']

def smap():
 directory=WORK/'smap-polar'
 if not directory.exists():url=seeded(ROOT/'prototype-output/wave3/smap/global',directory)
 else:url=json.loads((directory/'manifest.json').read_text())['url']
 session=ExactSession(url,Path.home()/'.netrc');m=Mirror(directory,url,session=session,cap=8*1024*1024)
 with h5py.File(View(m)) as f:
  report=dict(source=url,etag=m.info['etag'],size=m.info['size'],root_groups=list(f),datasets={},metadata={})
  for group in ['Soil_Moisture_Retrieval_Data_Polar_AM','Soil_Moisture_Retrieval_Data_Polar_PM']:
   report[group]=list(f[group]);suffix='_pm' if group.endswith('PM') else ''
   for name in ['soil_moisture','retrieval_qual_flag','surface_flag','freeze_thaw_fraction','latitude','longitude','EASE_row_index','EASE_column_index']:
    key=group+'/'+name+suffix
    if key in f:report['datasets'][key]=record(f[key])
  for key in ['Metadata/GridSpatialRepresentation','Metadata/GridSpatialRepresentation/GridDefinition']:
   report['metadata'][key]=clean(dict(f[key].attrs))
 m.seal();m.verify();report['capture']=m.info;save(DOC/'smap-polar-inspection.json',report)
 print(json.dumps({'smap':{k:(v['shape'],v['dtype'],v['chunks']) for k,v in report['datasets'].items()},'provider_bytes':m.info['provider_bytes']}),flush=True)

def goes():
 prefix='ABI-L2-CMIPM/2026/297/16/OR_ABI-L2-CMIPM1-M6C02'
 t=time.monotonic();r=requests.get('https://noaa-goes19.s3.amazonaws.com/',params={'list-type':'2','prefix':prefix,'max-keys':'1000'},timeout=30);r.raise_for_status();assert len(r.content)<1024*1024
 import xml.etree.ElementTree as ET
 tree=ET.fromstring(r.content);assert tree.find('{*}IsTruncated').text=='false'
 key=next(n.text for n in tree.findall('.//{*}Key') if 'CMIPM1-M6C02_' in n.text)
 catalog_seconds=time.monotonic()-t;url='https://noaa-goes19.s3.amazonaws.com/'+key;m=Mirror(WORK/'goes-m1-c02',url,cap=8*1024*1024)
 with h5py.File(View(m)) as f:
  report=dict(url=url,etag=m.info['etag'],size=m.info['size'],root_attrs=clean(dict(f.attrs)),datasets={},catalog_seconds=catalog_seconds)
  for name in ['CMI','DQF','x','y','t','goes_imager_projection','band_id','band_wavelength']:
   report['datasets'][name]=record(f[name],name in ['x','y'])
   if name in ['x','y']:report['datasets'][name]['values']=f[name][:].tolist()
  # Capture metadata needed by NetCDF-C without reading science globally.
  def visit(n,d):
   _=dict(d.attrs)
   if isinstance(d,h5py.Dataset) and d.ndim==0:_=d[()]
  f.visititems(visit)
 m.seal();m.verify();report['capture']=m.info;save(WORK/'goes-inspection.json',report)
 compact=json.loads(json.dumps(report))
 for name in ['x','y']:compact['datasets'][name].pop('values')
 save(DOC/'goes-inspection.json',compact)
 print(json.dumps({'goes':url,'shape':report['datasets']['CMI']['shape'],'chunks':report['datasets']['CMI']['chunks'],'provider_bytes':m.info['provider_bytes']}),flush=True)

def nisar():
 entries=json.loads((WORK/'nisar-catalog.json').read_text())['feed']['entry'];e=entries[0];granule=e['producer_granule_id'].removesuffix('.h5');url=next(l['href'] for l in e['links'] if l['href'].endswith('/'+granule+'.h5'))
 m=Mirror(WORK/'nisar-additional',url,session=ExactSession(url,Path.home()/'.netrc'),cap=8*1024*1024)
 report=dict(source=url,granule=granule,cmr_id=e['id'],etag=m.info['etag'],size=m.info['size'],datasets={})
 with h5py.File(View(m)) as f:
  base='/science/LSAR/GCOV/grids/frequencyA'
  for name in ['HHHH','HVHV','projection','xCoordinates','yCoordinates','xCoordinateSpacing','yCoordinateSpacing','listOfCovarianceTerms','listOfPolarizations']:
   report['datasets'][base+'/'+name]=record(f[base+'/'+name],name.endswith('Coordinates'))
  for group in ['/science/LSAR/identification','/science/LSAR/GCOV/metadata/processingInformation','/science/LSAR/GCOV/metadata/calibrationInformation/frequencyA']:
   def visit(name,d):
    if isinstance(d,h5py.Dataset) and d.size<=16:report['datasets'][group+'/'+name]=record(d)
   f[group].visititems(visit)
 m.seal();m.verify();report['capture']=m.info;save(DOC/'nisar-additional-inspection.json',report)
 print(json.dumps({'nisar':granule,'shape':report['datasets'][base+'/HHHH']['shape'],'projection':report['datasets'][base+'/projection'].get('value'),'provider_bytes':m.info['provider_bytes']}),flush=True)

if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('family',choices=['smap','goes','nisar']);a=p.parse_args();WORK.mkdir(parents=True,exist_ok=True);globals()[a.family]()
