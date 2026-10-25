#!/usr/bin/env python3
"""Wave 4 independent PROJ/GEOS, HDF5-C, raw Zarr/xarray qualification."""
import argparse, copy, hashlib, json, os, sys, time
from pathlib import Path
import h5py, numpy as np, shapely, zarr, xarray as xr
from pyproj import Transformer
import wave3_arrays as w
from hdf5_selective_mirror import Mirror, server

ROOT=w.ROOT;WORK=ROOT/'prototype-output/wave4';DOC=ROOT/'docs/products/wave4'
TRANSFORM=Transformer.from_crs(32616,4326,always_xy=True)
TOL=1e-10

def geom(aoi):
 if aoi['kind']=='bounds':return shapely.box(*aoi['bbox'])
 return shapely.Polygon(aoi['coordinates'][0],aoi['coordinates'][1:])

def membership(aoi,lon,lat,tolerance=0.):
 g=geom(aoi);p=shapely.points(lon,lat)
 mask=shapely.covers(g,p)
 if tolerance:mask|=shapely.distance(shapely.boundary(g),p)<=tolerance
 return mask&np.isfinite(lon)&np.isfinite(lat)

def fixture():
 cases=[];ys=4_052_515.-np.arange(33)*10.;xs=499_845.+np.arange(41)*10.
 X,Y=np.meshgrid(xs,ys);lon,lat=TRANSFORM.transform(X,Y)
 def add(name,aoi):
  mask=membership(aoi,lon,lat,TOL);iy,ix=np.where(mask);r=dict(name=name,ys=ys.tolist(),xs=xs.tolist(),aoi=aoi,included=int(mask.sum()))
  if iy.size:
   start=[int(iy.min()),int(ix.min())];count=[int(iy.max()-iy.min()+1),int(ix.max()-ix.min()+1)];r.update(start=start,count=count,mask=mask[start[0]:start[0]+count[0],start[1]:start[1]+count[1]].astype('u1').ravel().tolist())
  cases.append(r)
 box=lambda b:dict(kind='bounds',crs='EPSG:4326',bbox=b)
 add('both-sides-central-meridian',box([float(lon[5,4]),float(lat[25,0]),float(lon[5,35]),float(lat[4,16])]))
 add('closed-pixel-boundary-ties',box([float(lon[16,16]),float(lat[16,16]),float(lon[8,30]),float(lat[8,30])]))
 add('partial-overlap',box([float(lon.min())-.1,float(lat.min())-.1,float(lon[15,15]),float(lat[15,15])]))
 add('empty-overlap',box([-86.,36.,-85.99,36.01]))
 add('subpixel-zero-centers',box([float(lon[15,15])+1e-6,float(lat[15,15])+1e-6,float(lon[15,15])+2e-6,float(lat[15,15])+2e-6]))
 outer=[[float(lon[0,0])-.001,float(lat[-1,0])-.001],[float(lon[0,-1])+.001,float(lat[-1,0])-.001],[float(lon[0,-1])+.001,float(lat[0,20])+.001],[float(lon[0,0])-.001,float(lat[0,20])+.001]];outer.append(outer[0])
 hole=[[float(lon[22,12]),float(lat[22,12])],[float(lon[22,26]),float(lat[22,26])],[float(lon[9,26]),float(lat[9,26])],[float(lon[9,12]),float(lat[9,12])]];hole.append(hole[0])
 add('hole-and-closed-hole-boundaries',dict(kind='polygon',crs='EPSG:4326',coordinates=[outer,hole]))
 # Different source extent, entirely east of the central meridian.
 ys=4_052_515.-np.arange(30)*10.;xs=551_885.+np.arange(36)*10.;X,Y=np.meshgrid(xs,ys);lon,lat=TRANSFORM.transform(X,Y)
 add('inspected-source-location',box([-86.4197,36.615,-86.417,36.6166]))
 w.save(ROOT/'tests/data/wave4/utm-centers.json',dict(reference='pyproj/PROJ + GEOS covers and explicit 1e-10 degree boundary distance; no native oracle',versions=dict(shapely=shapely.__version__),cases=cases))
 polar=[];transform=Transformer.from_crs(6931,4326,always_xy=True)
 for x,y in [(0,0),(4500,4500),(-4500,4500),(4500,-4500),(-4500,-4500),(0,8995500),(8995500,0),(-8995500,8995500),(0,15000000),(float('nan'),0)]:
  lon,lat=transform.transform(x,y);polar.append(dict(x=x if np.isfinite(x) else None,y=y,expected=[lon,lat] if np.isfinite(lon) and np.isfinite(lat) else None))
 w.save(ROOT/'tests/data/wave4/polar-navigation.json',dict(reference='independent pyproj EPSG:6931 WGS84 inverse',cases=polar))
 print(json.dumps(dict(fixture_cases=len(cases))))

def verify_nisar(m,path,r,manifest):
 native=manifest['plan']['observations'][0]['native_selection'];sl=tuple(slice(a,a+n*s,s) for a,n,s in zip(native['start'],native['count'],native['step']));group='observations/'+r['observations'][0]['id'];g=zarr.open_group(str(path),mode='r')[group];base='/science/LSAR/GCOV/grids/frequencyA'
 with h5py.File(w.View(m)) as f:
  expected=f[base+'/HHHH'][sl];actual=g['HHHH'][:];assert actual.dtype==expected.dtype and actual.tobytes()==expected.tobytes()
  valid=~np.isnan(expected);np.testing.assert_array_equal(g['source_valid_HHHH'][:],valid.astype('u1'))
  x=f[base+'/xCoordinates'][sl[1]];y=f[base+'/yCoordinates'][sl[0]];np.testing.assert_array_equal(g['x'][:],x);np.testing.assert_array_equal(g['y'][:],y)
  X,Y=np.meshgrid(x,y);lon,lat=TRANSFORM.transform(X,Y);mask=membership(r['aoi'],lon,lat,TOL)
  np.testing.assert_array_equal(g['aoi_mask'][:],mask.astype('u1'));assert int(mask.sum())==manifest['plan']['observations'][0]['selection']['included_centers']
  error=max(float(np.abs(g['longitude'][:]-lon).max()),float(np.abs(g['latitude'][:]-lat).max()));assert error<1e-9
  assert g['HHHH'].dtype==np.dtype('f4') and g['x'].dtype==np.dtype('f8');assert int(g['spatial_ref'][()])==32616
  meta=g.attrs['product_metadata'];p='/science/LSAR/GCOV/metadata/processingInformation/parameters/rtc/';assert meta[p+'outputBackscatterNormalizationConvention']['value']=='gamma0';assert meta[p+'outputBackscatterExpressionConvention']['value']=='Linear power'
 with xr.open_zarr(str(path),group=group,consolidated=False,decode_cf=False,chunks=None) as ds:ds.load();assert ds.HHHH.dtype==np.dtype('f4')
 return dict(native_selection=native,samples=actual.size,included_centers=int(mask.sum()),source_valid_centers=int(valid.sum()),usable_centers=int((mask&valid).sum()),raw_bits_sha256=hashlib.sha256(actual.tobytes()).hexdigest(),coordinate_max_error_degrees=error,xarray_reopened=True)

def nisar(capture=False):
 os.environ['WAVE3_METADATA_POLICY']='page16k';info=json.loads((ROOT/'prototype-output/wave3/nisar/inspection.json').read_text())
 if capture:
  from wave4_inspect import seeded
  from wave3_capture import ExactSession
  directory=WORK/'nisar-geographic'
  if not directory.exists():seeded(ROOT/'prototype-output/wave3/nisar',directory)
  m=Mirror(directory,info['canonical_url'],session=ExactSession(info['canonical_url'],Path.home()/'.netrc'))
 else:m=Mirror(WORK/'nisar-geographic')
 m.verify();base='/science/LSAR/GCOV/grids/frequencyA'
 with h5py.File(w.View(m)) as f:x=f[base+'/xCoordinates'][:];y=f[base+'/yCoordinates'][:]
 def box_at(row,col,span):
  X,Y=np.meshgrid(x[col:col+span],y[row:row+span]);lon,lat=TRANSFORM.transform(X,Y);return dict(kind='bounds',crs='EPSG:4326',bbox=[float(lon.min())-1e-9,float(lat.min())-1e-9,float(lon.max())+1e-9,float(lat.max())+1e-9])
 aoi=box_at(17388,17676,8);b=aoi['bbox'];poly=dict(kind='polygon',crs='EPSG:4326',coordinates=[[[b[0],b[1]],[b[2],b[1]],[b[2],b[3]],[b[0],b[1]]]])
 cases=[('center-box',aoi),('triangle-mask',poly),('chunk-crossing',box_at(17404,17916,8)),('nan-corner',box_at(0,0,8))]
 results=[]
 with server(m) as srv:
  for label,aoi in cases:
   r=w.nisar_request(info,srv.url,[0,0],[1,1]);r['aoi']=aoi;req=WORK/f'nisar-{label}.json';w.save(req,r);output=WORK/f'nisar-{label}-{time.time_ns()}.zarr'
   before=len(srv.events);plan=w.invoke(['plan',req]);human=w.invoke(['plan',req,'--human']);run=w.invoke(['fetch',req,'--output',output]);manifest=json.loads(run['stdout']);verified=verify_nisar(m,output,r,manifest)
   events=[json.loads(e) for e in w.invoke(['stream',req,'--values'])['stdout'].splitlines()];verified['stream']=w.verify_stream(events,output,r,manifest['plan']['observations'][0]['native_selection'])
   physical=manifest['plan']['observations'][0]['physical'];datasets=physical['sources'][0]['object_identities']['datasets'];decoded=sum(int(np.prod(d['chunk_shape']))*np.dtype(d['stored_dtype']).itemsize*sum(c.get('byte_offset') is not None for c in d['chunks']) for d in datasets)
   results.append(dict(case=label,verification=verified,process=w.compact(run),plan_process=w.compact(plan),preparation_timings=manifest['plan']['observations'][0]['preparation_timings'],execution=manifest['observations'][0]['execution'],output_io=manifest['output_io'],physical_decoded_bytes=decoded,decoded_to_selected_amplification=decoded/(verified['samples']*4),working_memory=manifest['plan']['working_memory'],mirror_events=srv.events[before:],human_plan=human['stdout']))
   canonical=copy.deepcopy(r);canonical['provider']='earthdata';canonical['observations'][0]['assets']['HHHH']['href']=info['canonical_url'];w.save(DOC/f'nisar-{label}-request.json',canonical)
   if label=='center-box':w.save(ROOT/'examples/product-nisar-geographic.json',canonical)
  assert not srv.errors,srv.errors
 if capture:m.seal()
 m.verify();w.save(DOC/'nisar-geographic-capture.json',m.info);w.save(DOC/'nisar-geographic-validation.json',dict(status='passed',scope='original Wave3 specimen, bounded new extents and frozen replay; independent HDF5-C/PROJ/GEOS/Zarr/xarray; raw Float32 gamma0 linear power',cases=results))
 print(json.dumps(dict(status='passed',nisar_cases=len(results))),flush=True)

def goes(capture=False):
 info=json.loads((WORK/'goes-inspection.json').read_text());m=Mirror(WORK/'goes-m1-c02',info['url'] if capture else None,cap=8*1024*1024)
 with h5py.File(w.View(m)) as f:
  from pyproj import CRS
  attrs={k:v.item() if isinstance(v,np.ndarray) and v.size==1 else v.decode() if isinstance(v,bytes) else v for k,v in f['goes_imager_projection'].attrs.items()};crs=CRS.from_cf(attrs);tr=Transformer.from_crs(crs,4326,always_xy=True)
  x=f['x'][995:1005].astype('f8')*f['x'].attrs['scale_factor'].item()+f['x'].attrs['add_offset'].item();y=f['y'][995:1005].astype('f8')*f['y'].attrs['scale_factor'].item()+f['y'].attrs['add_offset'].item();X,Y=np.meshgrid(x*attrs['perspective_point_height'],y*attrs['perspective_point_height']);lon,lat=tr.transform(X,Y);bbox=[float(lon.min())-1e-8,float(lat.min())-1e-8,float(lon.max())+1e-8,float(lat.max())+1e-8]
 cases=[('geographic',dict(kind='bounds',crs='EPSG:4326',bbox=bbox)),('chunk-crossing',w.selection([1038,995],[8,10])),('native-strided',w.selection([995,995],[4,4],[3,2]))];results=[]
 with server(m) as srv:
  for label,aoi in cases:
   r=w.request('C02',info,srv.url,aoi);r['product']='goes19-abi-cmipm1-c02';r['observations'][0]['collection']='ABI-L2-CMIPM';req=WORK/f'goes-{label}.json';w.save(req,r);out=WORK/f'goes-{label}-{time.time_ns()}.zarr'
   run=w.invoke(['fetch',req,'--output',out]);manifest=json.loads(run['stdout']);verified=w.verify_goes(m,out,r,manifest);events=[json.loads(v) for v in w.invoke(['stream',req,'--values'])['stdout'].splitlines()];verified['stream']=w.verify_stream(events,out,r,manifest['plan']['observations'][0]['native_selection']);human=w.invoke(['plan',req,'--human'])
   results.append(dict(case=label,process=w.compact(run),verification=verified,preparation_timings=manifest['plan']['observations'][0]['preparation_timings'],execution=manifest['observations'][0]['execution'],output_io=manifest['output_io'],human_plan=human['stdout']))
   canonical=copy.deepcopy(r);canonical['provider']='auto'
   for a in canonical['observations'][0]['assets'].values():a['href']=info['url']
   w.save(DOC/f'goes-{label}-request.json',canonical)
  assert not srv.errors,srv.errors
 if capture:m.seal()
 m.verify();w.save(DOC/'goes-capture.json',m.info);w.save(DOC/'goes-validation.json',dict(status='passed',scope='GOES19 mesoscale1 C02 reflectance factor, original packed Int16 CMI/Int8 DQF; 0.5km scan-angle grid; PROJ + HDF5-C and separately opened NetCDF-C; raw Zarr/xarray',cases=results))
 print(json.dumps(dict(status='passed',goes_cases=len(results))),flush=True)

def benchmark(capture=False):
 from wave3_acceptance import call,compare
 results=[]
 for family,directory,request in [('nisar','nisar-geographic','nisar-center-box-request.json'),('goes','goes-m1-c02','goes-geographic-request.json'),('smap','smap-polar','smap-am-ordinary-request.json'),('nisar20','nisar-additional','nisar-additional-request.json'),('smap-pm','smap-polar','smap-pm-ordinary-request.json')]:
  os.environ['WAVE3_METADATA_POLICY']='page4k' if family.startswith('smap') else 'page16k'
  if capture:
   from wave3_capture import ExactSession
   url=json.loads((WORK/directory/'manifest.json').read_text())['url'];m=Mirror(WORK/directory,url,session=ExactSession(url,Path.home()/'.netrc') if family!='goes' else None)
  else:m=Mirror(WORK/directory)
  m.verify()
  with server(m) as srv:
   r=json.loads((DOC/request).read_text());r['provider']='pinned'
   for a in r['observations'][0]['assets'].values():a['href']=srv.url
   req=WORK/f'{family}-benchmark.json';w.save(req,r);out=WORK/f'{family}-benchmark-native-{time.time_ns()}.zarr';ref=WORK/f'{family}-benchmark-reference-{time.time_ns()}.zarr'
   before=len(srv.events);native=w.invoke(['fetch',req,'--output',out]);native_events=srv.events[before:];manifest=json.loads(native['stdout']);before=len(srv.events)
   run=call([sys.executable,ROOT/('bench/wave4_reference.py' if family!='goes' else 'bench/wave3_reference.py'),req,ref]);assert run['code']==0,(run,srv.errors);reference_events=srv.events[before:];equality=compare(out,ref)
   datasets=manifest['plan']['observations'][0]['physical']['sources'][0]['object_identities']['datasets'];decoded=sum(int(np.prod(d['chunk_shape']))*np.dtype(d['stored_dtype']).itemsize*sum(c.get('byte_offset') is not None for c in d['chunks']) for d in datasets);selected=manifest['observations'][0]['execution']['decoded_bytes']
   stream=w.invoke(['stream',req,'--values']);events=[json.loads(e) for e in stream['stdout'].splitlines()];first=next(e for e in events if e['event']=='chunk')
   results.append(dict(family=family,native_process=w.compact(native),reference_process=w.compact(run),reference_report=json.loads(run['stdout']),scientific_comparison=equality,native_source_events=native_events,reference_source_events=reference_events,preparation_timings=manifest['plan']['observations'][0]['preparation_timings'],execution=manifest['observations'][0]['execution'],output_io=manifest['output_io'],included_centers=manifest['plan']['observations'][0]['selection']['included_centers'],physical_decoded_bytes=decoded,selected_bytes=selected,decoded_to_selected_amplification=decoded/selected,working_memory=manifest['plan']['working_memory'],first_chunk_from_preparation_seconds=first['seconds']))
   assert not srv.errors,srv.errors
  if capture:m.seal()
  m.verify()
 w.save(DOC/'performance.json',dict(status='passed',binary_sha256=hashlib.sha256(w.BIN.read_bytes()).hexdigest(),scope='fresh serialized processes against frozen guarded source bytes; startup/import costs included in process metrics; independent selection algorithms and index traversal differ; no live throughput claim',cases=results));print(json.dumps(dict(status='passed',benchmark_cases=len(results))))

def smap(capture=False):
 from wave3_capture import ExactSession
 from pyproj import CRS
 os.environ['WAVE3_METADATA_POLICY']='page4k';info=json.loads((ROOT/'prototype-output/wave3/smap/global/inspection.json').read_text());url=info['canonical_url'];m=Mirror(WORK/'smap-polar',url if capture else None,session=ExactSession(url,Path.home()/'.netrc') if capture else None);results=[]
 transform=Transformer.from_crs(6931,4326,always_xy=True)
 # Four distinct locations exercise north-pole convergence, the longitude seam,
 # lower-hemisphere footprint edge and ordinary soil-moisture retrievals.
 cases=[('ordinary',[-86.6,36.5,-86.3,36.8]),('near-pole',[-45.,89.5,45.,90.]),('seam-east',[179.,75.,180.,75.5]),('equatorial-edge',[-.2,0.,.2,.3]),('native-corner',dict(kind='array_slice',start=[0,0],count=[4,4],step=[1,1],axes=['/@smap_ease2_n09/y','/@smap_ease2_n09/x']))]
 with server(m) as srv:
  for passname in ['am','pm']:
   for label,bbox in cases:
    r=w.smap_request(passname,info,srv.url,bbox if isinstance(bbox,dict) else dict(kind='bounds',crs='EPSG:4326',bbox=bbox));r['product']='smap-spl3smp-e-v6-north-'+passname;req=WORK/f'smap-{passname}-{label}.json';w.save(req,r);out=WORK/f'smap-{passname}-{label}-{time.time_ns()}.zarr';run=w.invoke(['fetch',req,'--output',out]);manifest=json.loads(run['stdout']);native=manifest['plan']['observations'][0]['native_selection'];a=native['start'];n=native['count'];sl=tuple(slice(a,a+n) for a,n in zip(a,n));suffix='_pm' if passname=='pm' else '';group='Soil_Moisture_Retrieval_Data_Polar_'+passname.upper();g=zarr.open_group(str(out),mode='r')['observations/'+r['observations'][0]['id']];refs=[]
    with h5py.File(w.View(m)) as f:
     for band in r['bands']:
      d=f[group+'/'+band+suffix];raw=d[sl];actual=g[band+suffix][:];assert raw.dtype==actual.dtype and raw.tobytes()==actual.tobytes();np.testing.assert_array_equal(g['source_valid_'+band+suffix][:],(raw!=d.attrs['_FillValue']).astype('u1'));refs.append(dict(variable=band+suffix,dtype=str(raw.dtype),samples=raw.size,raw_sha256=hashlib.sha256(raw.tobytes()).hexdigest(),source_valid_centers=int((raw!=d.attrs['_FillValue']).sum())))
     q=f[group+'/retrieval_qual_flag'+suffix][sl];np.testing.assert_array_equal(g['quality_smap_retrieval_recommended'][:],np.isin(q,[0,8]).astype('u1'))
     x=-9000000.+(np.arange(a[1],a[1]+n[1])+.5)*9000.;y=9000000.-(np.arange(a[0],a[0]+n[0])+.5)*9000.;np.testing.assert_array_equal(g['x'][:],x);np.testing.assert_array_equal(g['y'][:],y);X,Y=np.meshgrid(x,y);lon,lat=transform.transform(X,Y);error=max(float(np.abs(g['longitude'][:]-lon).max()),float(np.abs(g['latitude'][:]-lat).max()));assert error<2e-8,error
     source_lat=f[group+'/latitude'+suffix][sl];source_lon=f[group+'/longitude'+suffix][sl];ok=(source_lat!=-9999)&(source_lon!=-9999);source_error=max(float(np.abs(source_lat[ok]-lat[ok]).max()),float(np.abs(source_lon[ok]-lon[ok]).max())) if ok.any() else 0.;assert source_error<1e-5,source_error
     rows=f[group+'/EASE_row_index'+suffix][sl];cols=f[group+'/EASE_column_index'+suffix][sl];validindex=(rows!=65534)&(cols!=65534);R,C=np.meshgrid(np.arange(a[0],a[0]+n[0]),np.arange(a[1],a[1]+n[1]),indexing='ij');np.testing.assert_array_equal(rows[validindex],R[validindex]);np.testing.assert_array_equal(cols[validindex],C[validindex])
     mask=np.ones(lon.shape,dtype=bool) if r['aoi']['kind']=='array_slice' else membership(r['aoi'],lon,lat,TOL);np.testing.assert_array_equal(g['aoi_mask'][:],mask.astype('u1'));assert CRS.from_cf(dict(g['spatial_ref'].attrs)).to_epsg()==6931
    with xr.open_zarr(str(out),group='observations/'+r['observations'][0]['id'],decode_cf=False,consolidated=False,chunks=None) as ds:ds.load();assert ds['soil_moisture'+suffix].dtype==np.dtype('f4')
    events=[json.loads(v) for v in w.invoke(['stream',req,'--values'])['stdout'].splitlines()];stream=w.verify_stream(events,out,r,native);human=w.invoke(['plan',req,'--human'])
    results.append(dict(passname=passname,case=label,verification=dict(native_selection=native,variables=refs,coordinate_max_error_degrees=error,source_coordinate_max_rounding_error_degrees=source_error,source_coordinate_nonfill_centers=int(ok.sum()),included_centers=int(mask.sum()),stream=stream,xarray_reopened=True),process=w.compact(run),preparation_timings=manifest['plan']['observations'][0]['preparation_timings'],execution=manifest['observations'][0]['execution'],output_io=manifest['output_io'],human_plan=human['stdout']))
    canonical=copy.deepcopy(r);canonical['provider']='earthdata'
    for asset in canonical['observations'][0]['assets'].values():asset['href']=url
    w.save(DOC/f'smap-{passname}-{label}-request.json',canonical)
  assert not srv.errors,srv.errors
 if capture:m.seal()
 m.verify();w.save(DOC/'smap-polar-capture.json',m.info);w.save(DOC/'smap-polar-validation.json',dict(status='passed',authority='https://github.com/nsidc/mapxmaps/blob/main/EASE2_N09km.gpd',scope='actual R19240 2000x2000 polar AM/PM groups; separate observation passes and raw quality; independent HDF5-C, PROJ, original source geolocation/index fields, Zarr/xarray',cases=results));print(json.dumps(dict(status='passed',smap_cases=len(results))))

def additional(capture=False):
 from wave3_capture import ExactSession
 j=json.loads((DOC/'nisar-additional-inspection.json').read_text());url=j['source'];m=Mirror(WORK/'nisar-additional',url if capture else None,session=ExactSession(url,Path.home()/'.netrc') if capture else None);base='/science/LSAR/GCOV/grids/frequencyA'
 with h5py.File(w.View(m)) as f:
  x=f[base+'/xCoordinates'][8838:8846];y=f[base+'/yCoordinates'][8712:8720];X,Y=np.meshgrid(x,y);lon,lat=TRANSFORM.transform(X,Y);bbox=[float(lon.min())-1e-9,float(lat.min())-1e-9,float(lon.max())+1e-9,float(lat.max())+1e-9]
 info=dict(etag=j['etag'],granule=j['granule'],collection='NISAR_L2_GCOV_PROVISIONAL_V1');r=w.nisar_request(info,url,[0,0],[1,1]);r['observations'][0]['datetime']='2026-10-22T10:56:19.000000000Z';r['aoi']=dict(kind='bounds',crs='EPSG:4326',bbox=bbox);canonical=copy.deepcopy(r);canonical['provider']='earthdata';w.save(DOC/'nisar-additional-request.json',canonical)
 with server(m) as srv:
  r['observations'][0]['assets']['HHHH']['href']=srv.url;req=WORK/'nisar-additional-request.json';w.save(req,r);out=WORK/f'nisar-additional-{time.time_ns()}.zarr';run=w.invoke(['fetch',req,'--output',out]);manifest=json.loads(run['stdout']);verified=verify_nisar(m,out,r,manifest);events=[json.loads(v) for v in w.invoke(['stream',req,'--values'])['stdout'].splitlines()];verified['stream']=w.verify_stream(events,out,r,manifest['plan']['observations'][0]['native_selection']);assert not srv.errors,srv.errors
 if capture:m.seal()
 m.verify();w.save(DOC/'nisar-additional-capture.json',m.info);w.save(DOC/'nisar-additional-validation.json',dict(status='passed',verification=verified,process=w.compact(run),preparation_timings=manifest['plan']['observations'][0]['preparation_timings'],execution=manifest['observations'][0]['execution'],output_io=manifest['output_io'],scope='independently inspected second actual GCOV: 20m grid; HHHH only; no frequency/channel inference'));print(json.dumps(dict(status='passed',additional_nisar=True)))

def full_oracle():
 """Brute force every original source center with PROJ; no candidate pruning."""
 import resource
 m=Mirror(WORK/'nisar-geographic');r=json.loads((DOC/'nisar-center-box-request.json').read_text());base='/science/LSAR/GCOV/grids/frequencyA'
 with h5py.File(w.View(m)) as f:x=f[base+'/xCoordinates'][:];y=f[base+'/yCoordinates'][:]
 west,south,east,north=r['aoi']['bbox'];count=0;first=[len(y),len(x)];last=[-1,-1];start=time.monotonic();cpu=time.process_time()
 for row in range(0,len(y),16):
  X,Y=np.meshgrid(x,y[row:row+16]);lon,lat=TRANSFORM.transform(X,Y);mask=(lon>=west-TOL)&(lon<=east+TOL)&(lat>=south-TOL)&(lat<=north+TOL);iy,ix=np.where(mask)
  if iy.size:count+=int(iy.size);first=[min(first[0],row+int(iy.min())),min(first[1],int(ix.min()))];last=[max(last[0],row+int(iy.max())),max(last[1],int(ix.max()))]
 expected=json.loads((DOC/'nisar-geographic-validation.json').read_text())['cases'][0]['verification'];assert count==expected['included_centers'] and first==expected['native_selection']['start'];assert [last[i]-first[i]+1 for i in range(2)]==expected['native_selection']['count']
 w.save(DOC/'nisar-full-source-coordinate-oracle.json',dict(status='passed',reference='independent PROJ inverse of EVERY source center; bounded 16-row strips; no native row/binary-search or forward-envelope pruning',evaluated_centers=len(x)*len(y),included_centers=count,start=first,count=[last[i]-first[i]+1 for i in range(2)],wall_seconds=time.monotonic()-start,cpu_seconds=time.process_time()-cpu,peak_rss_bytes=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss*(1 if sys.platform=='darwin' else 1024)));print(json.dumps(dict(status='passed',full_source_centers=len(x)*len(y))))

def polar_oracle():
 cases=json.loads((DOC/'smap-polar-validation.json').read_text())['cases'][:4];boxes=[json.loads((DOC/f"smap-am-{c['case']}-request.json").read_text())['aoi']['bbox'] for c in cases];results=[dict(case=c['case'],included=0,first=[2000,2000],last=[-1,-1]) for c in cases];x=-9000000+(np.arange(2000)+.5)*9000;y=9000000-(np.arange(2000)+.5)*9000;transform=Transformer.from_crs(6931,4326,always_xy=True);started=time.monotonic()
 for row in range(0,2000,32):
  X,Y=np.meshgrid(x,y[row:row+32]);lon,lat=transform.transform(X,Y)
  for bbox,result in zip(boxes,results):
   mask=(lon>=bbox[0]-TOL)&(lat>=bbox[1]-TOL)&(lon<=bbox[2]+TOL)&(lat<=bbox[3]+TOL);iy,ix=np.where(mask)
   if iy.size:result['included']+=int(iy.size);result['first']=[min(result['first'][0],row+int(iy.min())),min(result['first'][1],int(ix.min()))];result['last']=[max(result['last'][0],row+int(iy.max())),max(result['last'][1],int(ix.max()))]
 for result,case in zip(results,cases):
  native=case['verification']['native_selection'];assert result['included']==case['verification']['included_centers'] and result['first']==native['start'] and [result['last'][i]-result['first'][i]+1 for i in range(2)]==native['count']
 w.save(DOC/'smap-full-source-coordinate-oracle.json',dict(status='passed',reference='PROJ inverse at all 4 million actual N09 centers in bounded strips, without native navigation or candidate pruning',source_centers=4000000,wall_seconds=time.monotonic()-started,cases=results));print(json.dumps(dict(status='passed',polar_full_source_windows=len(results))))

if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('action',choices=['fixture','nisar','goes','smap','additional','benchmark','full_oracle','polar_oracle']);p.add_argument('--capture',action='store_true');a=p.parse_args();WORK.mkdir(parents=True,exist_ok=True)
 if a.action in ['nisar','goes','smap','additional','benchmark']:globals()[a.action](a.capture)
 else:globals()[a.action]()
