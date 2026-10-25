#!/usr/bin/env python3
"""Offline cross-product lifecycle faults, resource admission and equivalent outputs."""
import argparse,copy,hashlib,json,os,signal,subprocess,sys,tempfile,time
from pathlib import Path
import numpy as np,zarr
import wave3_arrays as w
from hdf5_selective_mirror import Mirror,server,Handler
ROOT=w.ROOT;WORK=w.WORK;DOC=w.DOC;BIN=w.BIN
CASES=[('goes-c13','goes-C13','goes-c13-center-request.json'),('smap-am','smap/global','smap-am-texas-request.json'),('nisar','nisar','nisar-center-request.json')]
def call(command):
 with tempfile.TemporaryFile() as out,tempfile.TemporaryFile() as err:
  start=time.monotonic();p=subprocess.Popen(list(map(str,command)),stdout=out,stderr=err,env={**os.environ,'TILE_RIPPER_HDF5_METADATA_POLICY':os.environ.get('WAVE3_METADATA_POLICY','page16k'),'TILE_RIPPER_WORKERS':'1'});_,status,u=os.wait4(p.pid,0);p.returncode=os.waitstatus_to_exitcode(status);out.seek(0);err.seek(0);return dict(code=p.returncode,stdout=out.read().decode(),stderr=err.read().decode(),wall_seconds=time.monotonic()-start,cpu_seconds=u.ru_utime+u.ru_stime,peak_rss_bytes=u.ru_maxrss*(1 if sys.platform=='darwin' else 1024))
def req_for(label,srv,canonical):
 r=json.loads((DOC/canonical).read_text());r['provider']='pinned'
 for a in r['observations'][0]['assets'].values():a['href']=srv.url
 return r
class FaultMirror(Mirror):
 def read(self,start,size):
  data=super().read(start,size)
  if (start,size)==getattr(self,'target',None):
   self.science_seen=True
   if getattr(self,'mode',None)=='corrupt':data=bytes([data[0]^255])+data[1:]
  return data
def faults():
 results=[]
 for label,directory,canonical in CASES:
  os.environ['WAVE3_METADATA_POLICY']='page4k' if label.startswith('smap') else 'page16k'
  m=FaultMirror(WORK/directory);m.verify()
  with server(m) as srv:
   r=req_for(label,srv,canonical);req=WORK/f'{label}-fault-request.json';w.save(req,r);plan=json.loads(w.invoke(['plan',req])['stdout']);physical=plan['observations'][0]['physical'];group=physical['sources'][0]['object_identities']['datasets'][0]['physical_groups'][0];m.target=(group['start'],group['len']);assert group['len']>0
   for mode in ['cancel-output','cancel-stream','consumer-error']:
    dest=WORK/f'{label}-{mode}-{time.time_ns()}.zarr';run=call([ROOT/'target/release/examples/array_faults',req,mode,dest]);assert run['code']==0,run['stderr'];assert not dest.exists();results.append(dict(family=label,case=mode,passed=True,process=w.compact(run)))
   # Small working budgets must fail during admission, before source requests.
   tiny=copy.deepcopy(r);tiny['buffer_bytes']=1024;small=WORK/f'{label}-tiny.json';w.save(small,tiny);before=len(srv.events);run=call([BIN,'plan',small]);assert run['code']!=0 and len(srv.events)==before;results.append(dict(family=label,case='too-small-budget-before-network',passed=True))
   # The writer refuses an existing destination and leaves the sentinel intact.
   dest=WORK/f'{label}-existing-{time.time_ns()}.zarr';dest.mkdir();(dest/'sentinel').write_text('preserve');run=call([BIN,'fetch',req,'--output',dest]);assert run['code']!=0 and (dest/'sentinel').read_text()=='preserve';results.append(dict(family=label,case='existing-output-preserved',passed=True))
   for mode in ['corrupt','late-mutation']:
    m.mode=mode;m.science_seen=False;original=m.info['etag'];dest=WORK/f'{label}-{mode}-{time.time_ns()}.zarr';m.raw_written=False
    original_head=Handler.do_HEAD
    def head(self):
     if self.server.mirror is m and m.mode=='late-mutation' and m.science_seen:
      stages=list(dest.parent.glob('.'+dest.name+'.partial-*'));names=['CMI','soil_moisture','HHHH'];m.raw_written=any(any((p/'observations'/r['observations'][0]['id']/name).glob('[0-9]*')) for p in stages for name in names);m.info['etag']='"changed-after-native-write"'
     original_head(self)
    Handler.do_HEAD=head
    try:run=call([BIN,'fetch',req,'--output',dest])
    finally:Handler.do_HEAD=original_head;m.info['etag']=original;m.mode=None
    assert m.science_seen and run['code']!=0 and not dest.exists() and not list(dest.parent.glob('.'+dest.name+'.partial-*')),(mode,run)
    if mode=='late-mutation':assert m.raw_written
    results.append(dict(family=label,case=mode,passed=True,native_values_written_before_mutation=m.raw_written,process=w.compact(run)))
   assert not srv.errors,srv.errors
 w.save(DOC/'faults.json',dict(status='passed',cases=results,scope='guarded original-byte loopback controls only; no provider mutation or signing requests'))
 print(json.dumps(dict(status='passed',fault_cases=len(results))))
def compare(native,reference):
 a=zarr.open_group(str(native),mode='r');b=zarr.open_group(str(reference),mode='r');name=next(iter(a['observations'].group_keys()));a=a['observations/'+name];b=b['observations/'+name];arrays=[]
 for key in b.array_keys():
  if key=='spatial_ref':continue
  x=a[key][...];y=b[key][...];assert x.dtype==y.dtype,(key,x.dtype,y.dtype)
  if key in ['longitude','latitude']:np.testing.assert_allclose(x,y,rtol=0,atol=2e-8,equal_nan=True)
  else:assert x.tobytes()==y.tobytes(),key
  arrays.append(key)
 return dict(exact_native_arrays=arrays,derived_navigation_tolerance_degrees=2e-8)
def benchmark():
 results=[]
 for label,directory,canonical in CASES:
  os.environ['WAVE3_METADATA_POLICY']='page4k' if label.startswith('smap') else 'page16k';m=Mirror(WORK/directory);m.verify()
  with server(m) as srv:
   r=req_for(label,srv,canonical);req=WORK/f'{label}-benchmark.json';w.save(req,r)
   native=WORK/f'{label}-native-benchmark-{time.time_ns()}.zarr';ref=WORK/f'{label}-reference-benchmark-{time.time_ns()}.zarr';nr=w.invoke(['fetch',req,'--output',native]);rr=call([sys.executable,ROOT/'bench/wave3_reference.py',req,ref]);assert rr['code']==0,rr['stderr'];equal=compare(native,ref)
   ns=json.loads(nr['stdout']);rs=json.loads(rr['stdout']);sr=w.invoke(['stream',req,'--values']);events=[json.loads(x) for x in sr['stdout'].splitlines()];first=next(e for e in events if e['event']=='chunk');outputs=[]
   for p in [native,ref]:
    files=[x for x in p.rglob('*') if x.is_file()];outputs.append(dict(objects=len(files),bytes=sum(x.stat().st_size for x in files)))
   exec=ns['observations'][0]['execution'];preparation=ns['plan']['observations'][0]['preparation_timings'];physical=ns['plan']['observations'][0]['physical'];datasets=physical['sources'][0]['object_identities']['datasets'];native_decoded=sum(int(np.prod(d['chunk_shape']))*np.dtype(d['stored_dtype']).itemsize*sum(c.get('byte_offset') is not None for c in d['chunks']) for d in datasets);results.append(dict(family=label,native=w.compact(nr),mature=w.compact(rr),first_native_chunk_from_preparation_seconds=first.get('seconds'),scientific_comparison=equal,native_execution=exec,mature_execution=rs,preparation_timings=preparation,native_decoded_chunk_bytes=native_decoded,decoded_chunk_to_selected_bytes=native_decoded/exec['decoded_bytes'],working_memory=ns['plan']['working_memory'],native_output=outputs[0],mature_output=outputs[1],source_capture=dict(bytes=m.info['provider_bytes'],gets=m.info['provider_gets'],logical_object_bytes=m.info['size'])))
   assert not srv.errors,srv.errors
 w.save(DOC/'performance.json',dict(status='passed',native_binary_sha256=hashlib.sha256(BIN.read_bytes()).hexdigest(),scope='fresh native and mature-reader processes against identical guarded original bytes; serialized runs; Python imports included; no live-provider throughput or performance-leadership claim',cases=results,comparison_limits='Both validate strong source identity before/after and preserve native numeric/mask/coordinate work. Native validates the full legacy chunk index; h5py may traverse less index metadata. Both flush files/directories and include two macOS device-flush barriers before/after fresh-destination publication; reference uses ordinary rename in its isolated fresh benchmark namespace. Index-traversal work differs.'))
 print(json.dumps(dict(status='passed',benchmark_cases=len(results))))
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('action',choices=['faults','benchmark']);args=p.parse_args();faults() if args.action=='faults' else benchmark()
