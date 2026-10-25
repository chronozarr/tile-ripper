#!/usr/bin/env python3
"""Wave4 offline lifecycle controls and one bounded native live check per path."""
import argparse,copy,json,os,time
from pathlib import Path
import wave3_arrays as w
import wave3_acceptance as old
import wave4_geographic as geo
from hdf5_selective_mirror import Mirror,server
ROOT=w.ROOT;WORK=geo.WORK;DOC=geo.DOC
CASES=[('nisar','nisar-geographic','nisar-center-box-request.json'),('nisar20','nisar-additional','nisar-additional-request.json'),('goes','goes-m1-c02','goes-geographic-request.json'),('smap','smap-polar','smap-am-ordinary-request.json')]

def faults():
 old.WORK=WORK;old.DOC=DOC;old.CASES=CASES;old.faults()
 results=[]
 for family,directory,request in CASES:
  os.environ['WAVE3_METADATA_POLICY']='page4k' if family=='smap' else 'page16k';m=Mirror(WORK/directory)
  with server(m) as srv:
   r=json.loads((DOC/request).read_text());r['provider']='pinned'
   for asset in r['observations'][0]['assets'].values():asset['href']=srv.url
   negatives=[('non-WGS84',dict(kind='bounds',crs='EPSG:3857',bbox=[0,0,1,1])),('empty-AOI',dict(kind='bounds',crs='EPSG:4326',bbox=[-84.,40.,-83.99,40.01]))] if family.startswith('nisar') else [('non-WGS84',dict(kind='bounds',crs='EPSG:6931',bbox=[0,0,1,1]))]
   if family=='smap':negatives += [('southern-AOI',dict(kind='bounds',crs='EPSG:4326',bbox=[0,-20,1,-19])),('cross-grid-native-axes',dict(kind='array_slice',start=[0,0],count=[1,1],step=[1,1],axes=['/@smap_ease2_m09/y','/@smap_ease2_m09/x']))]
   for label,aoi in negatives:
    bad=copy.deepcopy(r);bad['aoi']=aoi;req=WORK/f'{family}-{label}.json';w.save(req,bad);out=WORK/f'{family}-{label}-{time.time_ns()}.zarr';run=old.call([w.BIN,'fetch',req,'--output',out]);assert run['code']!=0 and not out.exists();results.append(dict(family=family,case=label,passed=True,error=run['stderr'].strip()))
   # A changed identity after metadata acquisition must reject preparation.
   saved=m.info['etag'];original=old.Handler.do_HEAD
   def head(self):
    if self.server.mirror is m and any(e['method']=='GET' for e in srv.events):m.info['etag']='"changed-during-preparation"'
    original(self)
   old.Handler.do_HEAD=head
   try:
    srv.events.clear();req=WORK/f'{family}-prepare-mutation.json';w.save(req,r);run=old.call([w.BIN,'plan',req]);assert run['code']!=0
   finally:old.Handler.do_HEAD=original;m.info['etag']=saved
   results.append(dict(family=family,case='source-mutation-during-preparation',passed=True))
 report=json.loads((DOC/'faults.json').read_text());report['cases']+=results;w.save(DOC/'faults.json',report);print(json.dumps(dict(additional_faults=len(results))))

def live():
 results=[]
 for family,directory,request in CASES:
  os.environ['WAVE3_METADATA_POLICY']='page4k' if family=='smap' else 'page16k';r=json.loads((DOC/request).read_text());req=WORK/f'{family}-live-request.json';w.save(req,r);out=WORK/f'{family}-live-{time.time_ns()}.zarr';args=['fetch',req,'--output',out]
  if family!='goes':args+=['--auth','earthdata','--netrc',Path.home()/'.netrc']
  run=w.invoke(args);manifest=json.loads(run['stdout']);m=Mirror(WORK/directory)
  if family.startswith('nisar'):verification=geo.verify_nisar(m,out,r,manifest)
  elif family=='goes':verification=w.verify_goes(m,out,r,manifest)
  else:
   # Compare live bytes to the independently verified frozen polar output.
   import numpy as np,zarr
   reference=next(p for p in sorted(WORK.glob('smap-am-ordinary-*.zarr'),reverse=True) if p.is_dir());a=zarr.open_group(str(out),mode='r')['observations/'+r['observations'][0]['id']];b=zarr.open_group(str(reference),mode='r')['observations/'+r['observations'][0]['id']]
   for name in b.array_keys():
    x=a[name][...];y=b[name][...];assert x.dtype==y.dtype and x.tobytes()==y.tobytes(),name
   verification=dict(all_arrays_bit_equal_to_independently_checked_frozen_polar_output=True)
  results.append(dict(family=family,process=w.compact(run),verification=verification,preparation_timings=manifest['plan']['observations'][0]['preparation_timings'],execution=manifest['observations'][0]['execution'],output_io=manifest['output_io']))
 w.save(DOC/'live-native-validation.json',dict(status='passed',scope='one native canonical live fetch per new geographic path; inherited exact source/version/authentication contracts, final validation before publication',cases=results));print(json.dumps(dict(status='passed',live_cases=len(results))))

def workflow():
 results=[]
 for family,directory,request in CASES:
  os.environ['WAVE3_METADATA_POLICY']='page4k' if family=='smap' else 'page16k';m=Mirror(WORK/directory)
  with server(m) as srv:
   r=json.loads((DOC/request).read_text());r['provider']='pinned'
   for asset in r['observations'][0]['assets'].values():asset['href']=srv.url
   req=WORK/f'{family}-rust.json';w.save(req,r);out=WORK/f'{family}-rust-{time.time_ns()}.zarr';run=old.call([ROOT/'target/release/examples/array_product',req,out]);assert run['code']==0,run
   human=w.invoke(['plan',req,'--human']);assert 'Included native centers:' in human['stdout'] and 'Requested AOI:' in human['stdout'];results.append(dict(family=family,process=w.compact(run),human_plan=human['stdout']))
 w.save(DOC/'rust-workflows.json',dict(status='passed',cases=results));print(json.dumps(dict(status='passed',rust_cases=len(results))))

def catalog():
 results=[]
 for family,directory,request in CASES:
  r=json.loads((DOC/request).read_text());o=r['observations'][0];start=r['start'];end=r['end']
  if family=='goes':start=end=o['datetime']
  run=w.invoke(['discover','--product',r['product'],'--bbox',','.join(map(str,r['aoi']['bbox'])),'--start',start,'--end',end,'--bands',','.join(r['bands']),'--limit','8']);report=json.loads(run['stdout']);assert any(v['id']==o['id'] for v in report['observations']);results.append(dict(family=family,process=w.compact(run),catalog=report))
  if family=='nisar':
   r.pop('observations');r.pop('output',None);req=WORK/'nisar-automatic-discovery.json';w.save(req,r);plan=w.invoke(['plan',req,'--auth','earthdata','--netrc',Path.home()/'.netrc']);p=json.loads(plan['stdout']);assert len(p['observations'])==1 and p['observations'][0]['id']==o['id'];assert p['observations'][0]['selection']['included_centers']==64;results[-1]['automatic_discovery_and_native_geographic_preparation']=dict(process=w.compact(plan),plan=p)
   polygon=json.loads((DOC/'nisar-triangle-mask-request.json').read_text());polygon.pop('observations');polygon.pop('output',None);req=WORK/'nisar-automatic-polygon.json';w.save(req,polygon);run=w.invoke(['plan',req,'--auth','earthdata','--netrc',Path.home()/'.netrc']);p=json.loads(run['stdout']);assert len(p['observations'])==1 and p['observations'][0]['selection']['included_centers']==32;results[-1]['automatic_polygon_preparation']=dict(process=w.compact(run),plan=p)
 w.save(DOC/'catalog-workflows.json',dict(status='passed',scope='live bounded named-product discovery; original NISAR request also prepared with no observations or manual indices',cases=results));print(json.dumps(dict(status='passed',catalog_cases=len(results))))

if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('action',choices=['faults','live','workflow','catalog']);a=p.parse_args();globals()[a.action]()
