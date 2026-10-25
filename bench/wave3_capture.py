#!/usr/bin/env python3
"""Reproduce bounded protected-source evidence with exact-object confined auth.

No signed URL, cookie, authorization value or credential is persisted. Every
payload read is a conditional range and Mirror enforces the total provider cap.
The underlying sparse file is incomplete; replay refuses every uncaptured hole.
"""
import argparse,copy,json,netrc,os,re,stat,sys,time
from pathlib import Path
from urllib.parse import urlsplit,urljoin
import h5py,numpy as np,requests
import wave3_arrays as w
from hdf5_selective_mirror import Mirror,server
def require(condition,message='unqualified exact-object reference policy'):
 if not condition:raise RuntimeError(message)
class SafeResponse:
 def __init__(self,response):self.response=response;self.headers=response.headers;self.status_code=response.status_code;self.raw=response.raw
 def raise_for_status(self):
  if not 200<=self.status_code<300:raise RuntimeError(f'bounded original-source HTTP status {self.status_code}')
 def close(self):self.response.close()
 def __enter__(self):return self
 def __exit__(self,*args):self.close()
class ExactSession:
 def __init__(self,url,path):
  p=urlsplit(url);require(p.scheme=='https' and not p.query and not p.fragment and not p.username and not p.password and not p.port)
  self.url=url;self.archive=p.scheme+'://'+p.netloc;self.login='https://urs.earthdata.nasa.gov';self.routes={};self.sessions={};self.trace=[]
  if p.hostname=='data.nsidc.earthdatacloud.nasa.gov':
   match=re.fullmatch(r'/nsidc-cumulus-prod-protected/SMAP/SPL3SMP_E/006/(\d{4})/(\d{2})/(\d{2})/SMAP_L3_SM_P_E_(\d{8})_R19240_001.h5',p.path);require(match and ''.join(match.groups()[:3])==match.group(4))
   self.resource='https://d3h5e2j7riftk6.cloudfront.net';suffix='/nsidc-cumulus-prod-protected.s3.us-west-2.amazonaws.com/'+p.path.split('/nsidc-cumulus-prod-protected/',1)[1];gate='6fa4010df9e3d374e86134879ef72c9b'
  elif p.hostname=='nisar.asf.earthdatacloud.nasa.gov':
   match=re.fullmatch(r'/NISAR/NISAR_L2_GCOV_PROVISIONAL_V1/(NISAR_L2_PR_GCOV_[A-Z0-9_]+_P05023_N_F_J_001)/\1.h5',p.path);require(match)
   self.resource='https://d1mv8zhcvry6x4.cloudfront.net';suffix='/sds-n-cumulus-prod-nisar-products.s3.us-west-2.amazonaws.com/'+p.path[len('/NISAR/'):];gate='7fdfbcb0ce308dc58d08f97acb1f0590'
  else:raise RuntimeError('no qualified protected reference archive')
  self.paths={'HEAD':'/head-'+gate+suffix,'GET':'/s3-'+gate+suffix}
  f=os.lstat(path);require(stat.S_ISREG(f.st_mode) and not f.st_mode&0o077 and f.st_nlink==1,'reference netrc must be a private ordinary file')
  try:a=netrc.netrc(path).authenticators('urs.earthdata.nasa.gov')
  except Exception:raise RuntimeError('explicit reference credentials unavailable or invalid') from None
  require(a is not None,'explicit URS credentials missing');self.credentials=(a[0],a[2])
  for origin in [self.archive,self.login,self.resource]:s=requests.Session();s.trust_env=False;self.sessions[origin]=s
 def request(self,method,url,headers=None,**kwargs):
  require(url==self.url,'reference is confined to one canonical object');current=self.routes.get(method,self.url);headers=dict(headers or {});headers['Accept-Encoding']='identity'
  for _ in range(8):
   p=urlsplit(current);origin=p.scheme+'://'+p.netloc;require(origin in self.sessions)
   if origin==self.resource:require(p.path==self.paths[method])
   session=self.sessions[origin];auth=self.credentials if origin==self.login else None
   if origin==self.resource:session.cookies.clear()
   try:r=session.request(method,current,headers=headers,auth=auth,allow_redirects=False,stream=True,timeout=(10,30))
   except requests.RequestException:raise RuntimeError('bounded original-source transport failed') from None
   self.trace.append(dict(method=method,origin=origin,path=p.path,status=r.status_code))
   if not r.is_redirect:
    if origin==self.resource and r.status_code in [200,206,412]:self.routes[method]=current
    return SafeResponse(r)
   nxt=urljoin(current,r.headers['Location']);q=urlsplit(nxt);target=q.scheme+'://'+q.netloc;edge=(origin,target,q.path)
   allowed=[(self.archive,self.login,'/oauth/authorize'),(self.login,self.archive,'/login'),(self.archive,self.archive,urlsplit(self.url).path),(self.archive,self.resource,self.paths[method])]
   require(edge in allowed and not q.username and not q.password)
   # Do not retain resource cookies or attach any login cookies/credentials.
   if origin==self.resource:session.cookies.clear()
   r.close();current=nxt
  raise RuntimeError('bounded reference redirect limit')
 def head(self,url,**kwargs):return self.request('HEAD',url,**kwargs)
 def get(self,url,**kwargs):return self.request('GET',url,**kwargs)
def capture(request,directory,netrcpath):
 r=copy.deepcopy(request);assert len(r['observations'])==1;source=r['observations'][0]['assets'][r['bands'][0]];canonical=source['href'];session=ExactSession(canonical,netrcpath);m=Mirror(directory,canonical,cap=8*1024*1024,session=session);m.save();os.environ['WAVE3_METADATA_POLICY']='page4k' if r['product'].startswith('smap') else 'page16k'
 with server(m) as srv:
  r['provider']='pinned'
  for a in r['observations'][0]['assets'].values():assert a['href']==canonical;a['href']=srv.url;a['etag']=m.info['etag']
  req=directory/'native-request.json';w.save(req,r);plan=w.invoke(['plan',req]);physical=json.loads(plan['stdout']);selection=physical['observations'][0]['native_selection'];sl=tuple(slice(a,a+n*s,s) for a,n,s in zip(selection['start'],selection['count'],selection['step']));reference=[]
  with h5py.File(w.View(m),'r') as f:
   if r['product'].startswith('smap'):
    suffix='_pm' if r['product'].endswith('-pm') else '';group='Soil_Moisture_Retrieval_Data_'+('PM' if suffix else 'AM');paths=[group+'/'+b+suffix for b in r['bands']]+[group+'/latitude'+suffix,group+'/longitude'+suffix]
   else:paths=['/science/LSAR/GCOV/grids/frequencyA/HHHH']
   for name in paths:
    values=f[name][sl];np.save(directory/(name.replace('/','_')+'.npy'),values);reference.append(dict(path=name,dtype=str(values.dtype),shape=list(values.shape),raw_bits_sha256=__import__('hashlib').sha256(values.tobytes()).hexdigest()))
  out=directory/f'native-output-{time.time_ns()}.zarr';run=w.invoke(['fetch',req,'--output',out]);manifest=json.loads(run['stdout']);verify=w.verify_smap(m,out,r,manifest) if r['product'].startswith('smap') else w.verify_nisar(m,out,r,manifest)
  assert not srv.errors,srv.errors
 m.seal();m.verify()
 wrong=session.head(canonical,headers={'If-Match':'"deliberately-incorrect-version"'});assert wrong.status_code==412;wrong.close()
 report=dict(status='passed',canonical_source=canonical,provider_capture=m.info,reference=reference,verification=verify,native_process=w.compact(run),source_revalidated=True,wrong_if_match_rejected=True,access_trace=session.trace,scope='bounded h5py reference and native loopback replay; not a live-native transport qualification')
 w.save(directory/'qualification.json',report);return report
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('request',type=Path);p.add_argument('--netrc',type=Path,required=True);p.add_argument('--directory',type=Path,required=True);a=p.parse_args();request=json.loads(a.request.read_text());report=capture(request,a.directory,a.netrc);print(json.dumps(dict(status=report['status'],provider_bytes=report['provider_capture']['provider_bytes'],reference_samples=sum(np.prod(v['shape']).item() for v in report['reference']))))
