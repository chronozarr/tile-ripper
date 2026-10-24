"""Budgeted original-source AlphaEarth oracle using Zarr-Python and obstore.

Run in prototype-output/alphaearth-env, isolated from the Zarr-v2 product oracle.
No shard/large coordinate whole GETs. The proxy admits exact ranges only and
pins strong ETags before reading. Native and Python see original encoded bytes.
Default upstream budget:32MiB/128requests, single range<=8MiB, metadata<=64KiB.
"""
from __future__ import annotations

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import resource
import subprocess
import sys
import threading
import time

import numpy as np
import obstore
from obstore.store import HTTPStore
import requests
import zarr

ROOT=Path(__file__).resolve().parents[1]
BASE="https://s3.us-west-2.amazonaws.com/us-west-2.opendata.source.coop/tge-labs/aef-mosaic/"
WORK=ROOT/"prototype-output/alphaearth"
DOC=ROOT/"docs/alphaearth"
BYTE_BUDGET=32*2**20
REQUEST_BUDGET=128


def save(path,value):
    path.parent.mkdir(parents=True,exist_ok=True)
    path.write_text(json.dumps(value,indent=2,allow_nan=False)+"\n")


class Source:
    def __init__(self):
        self.session=requests.Session();self.session.trust_env=False
        self.session.headers["Accept-Encoding"]="identity"
        self.objects={};self.cache={};self.events=[];self.bytes=0
        self.lock=threading.RLock()

    def head(self,key):
        with self.lock:
            if key in self.objects:return self.objects[key]
            if not re.fullmatch(r"[a-z0-9_/.-]+",key) or ".." in key:
                raise ValueError("invalid source key")
            headers={}
            self.admit(0)
            t=time.monotonic();r=self.session.head(BASE+key,headers=headers,timeout=(10,30),allow_redirects=False)
            row=dict(method="HEAD",key=key,status=r.status_code,bytes=0,seconds=time.monotonic()-t)
            self.events.append(row)
            if r.status_code==404:
                self.objects[key]=None;return None
            if r.status_code!=200:raise ValueError("source HEAD failed")
            tag=r.headers.get("ETag","");size=int(r.headers.get("Content-Length",0))
            if not tag.startswith('"') or not tag.endswith('"') or size<=0:raise ValueError("source identity absent")
            identity=dict(etag=tag,size=size,version_id=r.headers.get("x-amz-version-id"))
            self.objects[key]=identity
            return identity

    def admit(self,n):
        if self.bytes+n>BYTE_BUDGET or len(self.events)>=REQUEST_BUDGET:
            raise ValueError("source experiment budget exceeded")

    def read(self,key,start,end):
        with self.lock:
            identity=self.head(key)
            if identity is None:return None
            if not (0<=start<=end<identity["size"]) or end-start+1>8*2**20:raise ValueError("invalid/excessive range")
            cache_key=(key,start,end)
            if cache_key in self.cache:return self.cache[cache_key]
            n=end-start+1;self.admit(n)
            t=time.monotonic();r=self.session.get(BASE+key,headers={"If-Match":identity["etag"],"Range":f"bytes={start}-{end}"},timeout=(10,45),allow_redirects=False,stream=True)
            try:
                if r.status_code!=206 or r.headers.get("ETag")!=identity["etag"] or r.headers.get("Content-Range")!=f"bytes {start}-{end}/{identity['size']}":raise ValueError("source rejected conditional range")
                data=r.raw.read(n+1)
                if len(data)!=n:raise ValueError("source range truncation/overrun")
            finally:r.close()
            self.bytes+=len(data)
            self.events.append(dict(method="GET",key=key,status=206,start=start,end=end,bytes=len(data),seconds=time.monotonic()-t,sha256=hashlib.sha256(data).hexdigest()))
            self.cache[cache_key]=data
            # Individual response files never masquerade as complete objects.
            path=WORK/"ranges"/(key.replace("/","_")+f"-{start}-{end}.bin")
            path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(data)
            return data

    def validate(self):
        for key,identity in self.objects.items():
            self.admit(0);t=time.monotonic()
            r=self.session.head(BASE+key,headers={"If-Match":identity["etag"]} if identity else {},timeout=(10,30),allow_redirects=False)
            self.events.append(dict(method="HEAD-final",key=key,status=r.status_code,bytes=0,seconds=time.monotonic()-t))
            assert (r.status_code==404 if identity is None else r.status_code==200 and r.headers.get("ETag")==identity["etag"] and int(r.headers.get("Content-Length",0))==identity["size"])


class Handler(BaseHTTPRequestHandler):
    protocol_version="HTTP/1.1"
    def log_message(self,*args):pass
    def do_HEAD(self):self.serve(False)
    def do_GET(self):self.serve(True)
    def serve(self,body):
        try:
            key=self.path.lstrip("/");source=self.server.source
            identity=source.head(key)
            if identity is None:self.send_error(404);return
            expected=self.headers.get("If-Match")
            if expected and expected!=identity["etag"]:self.send_error(412);return
            start,end=0,identity["size"]-1
            span=self.headers.get("Range")
            if span:
                match=re.fullmatch(r"bytes=(\d*)-(\d*)",span)
                if not match:raise ValueError("range")
                a,b=match.groups()
                if a:start,end=int(a),int(b) if b else end
                elif b:start=max(0,identity["size"]-int(b))
                else:raise ValueError("range")
            if body and not span and identity["size"]>65536:
                raise ValueError("whole large object GET forbidden")
            data=source.read(key,start,end) if body else None
            self.send_response(206 if span else 200)
            self.send_header("Content-Length",str(end-start+1));self.send_header("ETag",identity["etag"])
            self.send_header("Accept-Ranges","bytes")
            if span:self.send_header("Content-Range",f"bytes {start}-{end}/{identity['size']}")
            self.end_headers()
            if body:self.wfile.write(data)
        except Exception as e:
            self.server.errors.append(type(e).__name__)
            self.send_error(500)


def run(binary):
    started=time.monotonic();cpu=time.process_time()
    source=Source();server=ThreadingHTTPServer(("127.0.0.1",0),Handler);server.source=source;server.errors=[]
    threading.Thread(target=server.serve_forever,daemon=True).start()
    url=f"http://127.0.0.1:{server.server_port}/"
    store=zarr.storage.ObjectStore(HTTPStore(url,client_options={"allow_http":True}),read_only=True)
    original={}
    for key in ["zarr.json","embeddings/zarr.json","time/zarr.json","x/zarr.json","y/zarr.json","band/zarr.json"]:
        identity=source.head(key)
        if identity:original[key]=json.loads(source.read(key,0,identity["size"]-1))
    array=zarr.open_array(store,path="embeddings",mode="r",zarr_format=3)
    times=zarr.open_array(store,path="time",mode="r",zarr_format=3)[:]
    assert list(times)==list(range(2017,2026))
    root=original["zarr.json"]["attributes"]
    a,b,c,d,e,f=root["spatial:transform"]
    assert b==d==0 and a>0 and e<0 and root["spatial:registration"]=="pixel"
    samples=[]
    for axis,n,origin,step,selected in [("x",array.shape[3],c,a,1124304),("y",array.shape[2],f,e,542672)]:
        for index in sorted({0,1,n-2,n-1,n//2,255,256,4095,4096,selected-1,selected,selected+1}):
            actual=float(np.frombuffer(source.read(axis+"/c/0",index*8,index*8+7),dtype="<f8")[0]);expected=origin+(index+.5)*step
            assert abs(actual-expected)<1e-10
            samples.append(dict(axis=axis,index=index,stored_value=actual,affine_value=expected))
    configs=[
        ("pixel",[542672,1124304],[1,1],None),
        ("window",[542670,1124302],[3,4],None),
        ("logical_boundary",[542719,1124351],[2,2],None),
        ("shard_boundary",[544767,1126399],[2,2],None),
        ("channel_positions",[542672,1124304],[1,1],[63,0,17]),
    ]
    results=[]
    try:
        for label,start,count,channels in configs:
            request=dict(locator=url,time_index=0,start=start,count=count,network_bytes=BYTE_BUDGET,network_requests=REQUEST_BUDGET,working_bytes=64*2**20)
            if channels is not None:request["channels"]=channels
            path=WORK/(label+".json");save(path,request)
            t=time.monotonic()
            process=subprocess.run([str(binary),str(path)],capture_output=True,text=True)
            native_seconds=time.monotonic()-t
            if process.returncode:
                save(WORK/(label+"-failed.json"),dict(code=process.returncode,stderr=process.stderr))
                raise ValueError("native prototype failed "+label)
            rows=[json.loads(line) for line in process.stdout.splitlines()]
            save(WORK/(label+"-native.json"),rows)
            metadata=next(row["metadata"] for row in rows if row["kind"]=="metadata")
            chunks=[row["chunk"] for row in rows if row["kind"]=="chunk"]
            completion=next(row["report"] for row in rows if row["kind"]=="completion")
            indexes=list(range(64)) if channels is None else channels
            native=np.empty((1,len(indexes),*count),dtype="int8")
            for chunk in chunks:
                values=np.array(chunk["values"],dtype="int8").reshape(chunk["shape"])
                yy,xx=chunk["start"];hh,ww=values.shape[-2:]
                native[:,:,yy-start[0]:yy-start[0]+hh,xx-start[1]:xx-start[1]+ww]=values
            t=time.monotonic()
            # Orthogonal indexing preserves the requested original channel order.
            reference=array.oindex[(slice(0,1),indexes,slice(start[0],start[0]+count[0]),slice(start[1],start[1]+count[1]))]
            reference_seconds=time.monotonic()-t
            assert native.dtype==reference.dtype==np.dtype("int8") and np.array_equal(native,reference)
            np.save(WORK/(label+"-reference.npy"),reference)
            results.append(dict(label=label,start=start,count=count,channels=indexes,shape=list(native.shape),complete_native_values_equal=True,reference_sha256=hashlib.sha256(reference.tobytes()).hexdigest(),reference_values=reference.tolist() if reference.size<=512 else None,native_seconds=native_seconds,reference_seconds=reference_seconds,native_metadata=metadata,native_report=completion,physical_chunks=[{k:ch[k] for k in ["logical_indices","shard_key","shard_size","encoded_range"] if k in ch} for ch in chunks]))
            print(label,"exact Int8 equality",native.shape,flush=True)
        source.validate()
        assert not server.errors
        report=dict(schema="tile-ripper-alphaearth-qualification-v1",status="passed",source=BASE,budget=dict(bytes=BYTE_BUDGET,requests=REQUEST_BUDGET,single_range=8*2**20,metadata=65536),dependencies=dict(python=sys.version.split()[0],zarr=zarr.__version__,obstore=obstore.__version__,numpy=np.__version__,requests=requests.__version__),original_metadata=original,years=times.tolist(),coordinate_samples=samples,cases=results,objects=source.objects,source_events=source.events,source_bytes=source.bytes,source_requests=len(source.events),wall_seconds=time.monotonic()-started,cpu_seconds=time.process_time()-cpu,python_peak_rss_bytes=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss*(1 if sys.platform=="darwin" else 1024),scope="stream-only dedicated Int8 rank-four profile; original band dimension positions preserved; no channel labels present; one channel/pixel still decodes all64x256x256; no full shards/coordinate axes; reference/native share guarded original-byte capture, independent library decoders")
        save(DOC/"qualification.json",report)
    finally:
        save(WORK/"source-progress.json",dict(objects=source.objects,events=source.events,bytes=source.bytes,errors=server.errors))
        server.shutdown();server.server_close()


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--binary",type=Path,default=ROOT/"target/debug/examples/alphaearth_stream")
    args=parser.parse_args()
    try:run(args.binary)
    except Exception as e:raise SystemExit("Bounded AlphaEarth probe failed: "+type(e).__name__+"; see local progress receipt") from None
