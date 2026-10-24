"""Bounded matched-provider oracle; no Tile-Ripper decoder is a reference.

Run in prototype-output/zarr-env. Explicit --netrc is required for NASA.
Science traffic: <=32 MiB, <=240 range GETs across four assets, no whole GETs.
Runtime signatures and credentials are never written. Sparse captured TIFFs
retain original bytes/offsets and support repeatable independent local replay.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import threading
import time
from urllib.parse import urlparse

import numpy as np
import rasterio
from rasterio.windows import Window
import requests

import real_multiscene as mirror_base
from scientific_output import invoke
from wave1_hls_auth import resolve_signed_get

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/providers"
DOC = ROOT / "docs/providers"
PC = "https://planetarycomputer.microsoft.com/api/stac/v1"
SIGN = "https://planetarycomputer.microsoft.com/api/sas/v1/sign"
ARCHIVE = "https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected"
LIMIT_BYTES = 32 * 2**20
LIMIT_REQUESTS = 240


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def session():
    value = requests.Session()
    value.trust_env = False
    value.headers["Accept-Encoding"] = "identity"
    return value


class Capture(mirror_base.Mirror):
    """Exact conditional ranges, runtime URLs excluded from the stored index."""

    def __init__(self, root, sources, runtime, live):
        self.runtime = runtime
        self.http = session()
        super().__init__(root, sources, live=live, max_bytes=LIMIT_BYTES)

    def read(self, key, start, end):
        source = self.sources[key]
        if not (0 <= start <= end < source["bytes"]) or end - start + 1 > 2**20:
            raise ValueError("invalid or excessive oracle range")
        needed = set(range(start // mirror_base.PAGE, end // mirror_base.PAGE + 1))
        with self.lock:
            missing = needed - self.pages[key]
            if missing and not self.live:
                raise ValueError("uncaptured range")
            while missing:
                first = last = min(missing)
                while last + 1 in missing and last - first < 15:
                    last += 1
                lo = first * mirror_base.PAGE
                hi = min(source["bytes"] - 1, (last + 1) * mirror_base.PAGE - 1)
                if self.provider_bytes + hi - lo + 1 > LIMIT_BYTES or len(self.events) >= LIMIT_REQUESTS:
                    raise ValueError("qualification science budget exhausted")
                began = time.monotonic()
                response = self.http.get(
                    self.runtime[key],
                    headers={"Range": f"bytes={lo}-{hi}", "If-Match": source["etag"]},
                    timeout=(10, 45), allow_redirects=False, stream=True,
                )
                try:
                    if response.status_code != 206 or response.headers.get("ETag") != source["etag"]:
                        raise ValueError("provider rejected version-bound range")
                    if response.headers.get("Content-Range") != f"bytes {lo}-{hi}/{source['bytes']}":
                        raise ValueError("provider range identity mismatch")
                    data = response.raw.read(hi - lo + 2)
                    if len(data) != hi - lo + 1:
                        raise ValueError("provider truncated/exceeded range")
                finally:
                    response.close()
                with self.path(key).open("r+b") as handle:
                    handle.seek(lo)
                    handle.write(data)
                self.provider_bytes += len(data)
                self.events.append(dict(source=key, start=lo, end=hi, bytes=len(data), seconds=time.monotonic()-began, sha256=hashlib.sha256(data).hexdigest()))
                self.pages[key].update(range(first, last + 1))
                missing = needed - self.pages[key]
            with self.path(key).open("rb") as handle:
                handle.seek(start)
                return handle.read(end - start + 1)


def identity(http, url):
    response = http.head(url, timeout=(10, 30), allow_redirects=False)
    if response.status_code == 303 and url.startswith(ARCHIVE + "/"):
        target = response.headers.get("Location", "")
        parsed = urlparse(target)
        expected = "/head-2d2df3a34830d5223d1e9547cd713408/lp-prod-protected.s3.us-west-2.amazonaws.com/" + url[len(ARCHIVE)+1:]
        if parsed.scheme != "https" or parsed.netloc != "d1nklfio7vscoe.cloudfront.net" or parsed.path != expected:
            raise ValueError("NASA HEAD redirect escaped exact object")
        response = http.head(target, timeout=(10,30), allow_redirects=False)
    if response.status_code != 200:
        raise ValueError("provider HEAD rejected")
    etag = response.headers.get("ETag", "")
    size = int(response.headers.get("Content-Length", 0))
    if not etag.startswith('"') or not etag.endswith('"') or size <= 0:
        raise ValueError("missing strong object identity")
    return etag, size


def oracle(url, window):
    # All reads go through a capped conditional-range proxy, never directly
    # through GDAL's authentication, ambient netrc, or unrestricted HTTP path.
    with rasterio.Env(GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", GDAL_HTTP_NETRC="NO", GDAL_CACHEMAX=8*2**20, CPL_LOG=os.devnull):
        with rasterio.open(url) as src:
            values = src.read(1, window=Window(*window))
            metadata = dict(shape=[src.height, src.width], dtype=src.dtypes[0], crs=src.crs.to_string(), transform=list(src.transform)[:6], nodata=src.nodata, scale=src.scales[0], offset=src.offsets[0], tags=src.tags(), band_tags=src.tags(1), description=src.descriptions[0], block_shape=list(src.block_shapes[0]), compression=src.compression.name)
    return values, metadata


def compare(netrc_path, product_filter=None):
    http = session()
    cases = [
        ("hls-l30", "hls2-l30", "HLS.L30.T15VUE.2022231T172609.v2.0", "HLSL30", [1792,1792,32,32]),
        ("hls-s30", "hls2-s30", "HLS.S30.T59UNU.2026295T235129.v2.0", "HLSS30", [1124,1659,32,32]),
    ]
    report = dict(schema="tile-ripper-provider-qualification-v1", captured_at_utc=time.strftime("%Y-%m-%dT%H:%M:%SZ",time.gmtime()), science_budget=dict(max_bytes=LIMIT_BYTES,max_requests=LIMIT_REQUESTS,maximum_single_range_bytes=2**20), reference=dict(rasterio=rasterio.__version__, gdal=rasterio.__gdal_version__, numpy=np.__version__, requests=requests.__version__), cases=[])
    if product_filter and (DOC/"equivalence.json").exists():
        report = json.loads((DOC/"equivalence.json").read_text())
        report["cases"] = [c for c in report["cases"] if c["product"] != product_filter]
    for product, collection, granule, short, window in cases:
        if product_filter and product != product_filter:
            continue
        began=time.monotonic()
        response=http.get(f"{PC}/collections/{collection}/items/{granule}",timeout=(10,30),allow_redirects=False)
        response.raise_for_status()
        item=response.json()
        catalog_seconds=time.monotonic()-began
        sources=[];runtime={};overheads=[]
        for provider in ("planetary_computer","earthdata"):
            for band in ("B04","Fmask"):
                t=time.monotonic()
                if provider=="planetary_computer":
                    canonical=item["assets"][band]["href"]
                    unsigned=http.head(canonical,timeout=(10,30),allow_redirects=False).status_code
                    signed=http.get(SIGN,params={"href":canonical},timeout=(10,30),allow_redirects=False).json()["href"]
                    parsed=urlparse(canonical);actual=urlparse(signed)
                    assert (parsed.scheme,parsed.netloc,parsed.path)==(actual.scheme,actual.netloc,actual.path)
                    signing_seconds=time.monotonic()-t
                    etag,size=identity(http,signed)
                    transitions=[]
                else:
                    canonical=f"{ARCHIVE}/{short}.020/{granule}/{granule}.{band}.tif"
                    # NASA's unsigned HEAD returns the object ETag without a
                    # payload. GET authorization is separately exact-object.
                    etag,size=identity(http,canonical)
                    signed,transitions,actual_size=resolve_signed_get(canonical,etag,netrc_path)
                    assert actual_size==size
                    unsigned=None;signing_seconds=time.monotonic()-t
                runtime[f"/{len(sources)}.tif"]=signed
                sources.append(dict(href=canonical,bytes=size,etag=etag,provider=provider,band=band))
                overheads.append(dict(provider=provider,band=band,unsigned_head_status=unsigned,signing_auth_seconds=signing_seconds,identity_and_access_seconds=time.monotonic()-t,redirects=[dict(from_origin=a,to_origin=b,status=c) for a,b,c in transitions]))
        capture=Capture(WORK/(product+"-capture"),sources,runtime,True)
        server=mirror_base.start(capture)
        base=f"http://127.0.0.1:{server.server_port}"
        arrays=[];metadata=[]
        try:
            for i in range(4):
                t=time.monotonic();values,meta=oracle(f"{base}/{i}.tif",window)
                arrays.append(values);metadata.append(meta)
                np.save(WORK/(product+f"-{i}-values.npy"),values)
                overheads[i]["reference_window_seconds"]=time.monotonic()-t
            capture.save()
        finally:
            server.shutdown();server.server_close()
        checks=[]
        for a,b in ((0,2),(1,3)):
            compared=["shape","dtype","crs","transform","nodata","scale","offset","tags","band_tags","description"]
            equal=np.array_equal(arrays[a],arrays[b])
            fields={key:metadata[a][key]==metadata[b][key] for key in compared}
            checks.append(dict(band=sources[a]["band"],complete_selected_native_values_equal=bool(equal),selected_shape=list(arrays[a].shape),metadata_equal=fields,classification="scientifically_equivalent_selected_sample" if equal and all(fields.values()) else "unknown_or_different",exact_byte_replica_proven=False,stored_values=arrays[a].tolist(),sha256=hashlib.sha256(arrays[a].tobytes()).hexdigest()))
        result=dict(product=product,granule=granule,collection=collection,catalog_seconds=catalog_seconds,window=window,sources=sources,access=overheads,science_comparisons=checks,metadata_by_source=metadata,capture=dict(requests=len(capture.events),bytes=capture.provider_bytes,ranges=capture.events),scope="complete selected 32x32 native samples and retained physical science metadata; no whole-object byte identity or archive-wide equivalence claim")
        report["cases"].append(result)
        save(DOC/"equivalence.json",report)
        print(product,[(c["band"],c["classification"],c["metadata_equal"]) for c in checks],flush=True)
    return report


def benchmark(binary, netrc_path, live, rounds):
    """Same scientific request/output per provider; no discovery speed probe."""
    import zarr
    from rasterio.warp import transform_bounds
    evidence=json.loads((DOC/"equivalence.json").read_text())
    results=[]
    stamp=time.strftime("%Y%m%dT%H%M%S",time.gmtime())
    for case in evidence["cases"]:
        captured=Capture(WORK/(case["product"]+"-capture"),case["sources"],{},False)
        captured.verify_pages()
        server=mirror_base.start(captured,latency=5,mbps=100)
        base=f"http://127.0.0.1:{server.server_port}"
        meta=case["metadata_by_source"][0]
        affine=rasterio.Affine(*meta["transform"])
        x,y,w,h=case["window"]
        left,top=affine*(x+2,y+2);right,bottom=affine*(x+w-2,y+h-2)
        bbox=list(transform_bounds(meta["crs"],"EPSG:4326",left,bottom,right,top))
        sensing=meta["tags"]["SENSING_TIME"].split(";")[0].strip()
        try:
            for turn in range(rounds):
                providers=("planetary_computer","earthdata") if turn%2==0 else ("earthdata","planetary_computer")
                for provider in providers:
                    indexes=[i for i,s in enumerate(case["sources"]) if s["provider"]==provider]
                    assets={}
                    for i in indexes:
                        source=case["sources"][i]
                        assets[source["band"]]=dict(href=source["href"] if live else f"{base}/{i}.tif",etag=source["etag"])
                    request=dict(product=case["product"],provider=provider if live else "pinned",aoi=dict(kind="bounds",crs="EPSG:4326",bbox=bbox),start=sensing,end=sensing,bands=["B04","Fmask"],max_observations=1,buffer_bytes=32*2**20,chunk_edge=64,quality_policies=["hls_fmask_clear"],observations=[dict(id=case["granule"],datetime=sensing,collection=case["collection"] if provider=="planetary_computer" else ("HLSL30" if case["product"]=="hls-l30" else "HLSS30"),platform=None,processing=None,assets=assets)])
                    label=f"{case['product']}-{provider}-{'live' if live else 'mirror'}-{stamp}-{turn}"
                    request_path=WORK/(label+".json");save(request_path,request)
                    output=WORK/(label+".zarr")
                    args=["fetch",str(request_path),"--output",str(output),"--gdal-free","--workers","1","--per-host","1"]
                    if live and provider=="earthdata":args += ["--auth","earthdata","--netrc",str(netrc_path)]
                    server.reset()
                    run=invoke(args,binary=binary)
                    if run["code"]:
                        # CLI must already confine errors. Do not record request
                        # exception strings or runtime signer responses here.
                        save(WORK/(label+"-failure.json"),dict(code=run["code"],stderr=run["stderr"]))
                        results.append(dict(product=case["product"],provider=provider,mode="live" if live else "controlled_mirror",round=turn,status="failed",wall_seconds=run["wall_seconds"],cpu_seconds=run["cpu_seconds"],peak_rss_bytes=run["peak_rss_bytes"],error=run["stderr"],no_provider_fallback=True))
                        save(DOC/("live-performance.json" if live else "mirror-performance.json"),dict(schema="tile-ripper-provider-performance-v1",binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),cases=results))
                        print(label,"failed without fallback",flush=True)
                        continue
                    manifest=json.loads((output/"manifest.json").read_text())
                    assert manifest["status"]=="complete"
                    checks=[]
                    for row in manifest["outputs"]:
                        band=row["asset_id"].split("_",1)[1]
                        idx=next(i for i in indexes if case["sources"][i]["band"]==band)
                        with rasterio.open(captured.path(f"/{idx}.tif")) as src:
                            win=row["window"];window=Window(win["x"],win["y"],win["width"],win["height"])
                            raw=src.read(1,window=window)
                            tx=src.window_transform(window)
                        group=zarr.open_group(str(output),mode="r")[row["group"]]
                        values=np.asarray(group["values"])[0]
                        aoi=np.asarray(group["aoi_mask"])[0].astype(bool)
                        valid=np.asarray(group["valid"])[0].astype(bool)
                        expected=raw.copy();expected[~aoi]=meta["nodata"] if band=="B04" else 255
                        assert np.array_equal(values,expected)
                        assert np.array_equal(valid,aoi & (raw != (-9999 if band=="B04" else 255)))
                        assert np.allclose(np.asarray(group["x"]),tx.c+(np.arange(raw.shape[1])+.5)*tx.a,rtol=0,atol=1e-8)
                        assert np.allclose(np.asarray(group["y"]),tx.f+(np.arange(raw.shape[0])+.5)*tx.e,rtol=0,atol=1e-8)
                        if band=="Fmask":
                            expected_quality=(aoi & (raw != 255) & ((raw & 0x1e)==0)).astype("uint8")
                            assert np.array_equal(np.asarray(group["quality_hls_fmask_clear"])[0],expected_quality)
                        checks.append(dict(band=band,shape=list(values.shape),complete_values_equal=True,masks_equal=True,quality_equal=True if band=="Fmask" else None,coordinates_equal=True,raw_sha256=hashlib.sha256(values.tobytes()).hexdigest()))
                    measured=dict(product=case["product"],provider=provider,mode="live" if live else "controlled_mirror",round=turn,wall_seconds=run["wall_seconds"],cpu_seconds=run["cpu_seconds"],peak_rss_bytes=run["peak_rss_bytes"],observed=manifest["observed"],loopback=dict(server.counts) if not live else None,checks=checks,output_bytes=sum(p.stat().st_size for p in output.rglob("*") if p.is_file()),output_objects=sum(1 for p in output.rglob("*") if p.is_file()),routing=manifest["inspection"]["product"]["routing"])
                    results.append(measured)
                    save(DOC/("live-performance.json" if live else "mirror-performance.json"),dict(schema="tile-ripper-provider-performance-v1",binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),conditions=dict(workers=1,per_host=1,loopback_latency_ms=5,loopback_mbps=100,rounds=rounds,live_source_bytes="native metadata ranges plus selected payload; original-source science capture reported separately",timing="wall includes process startup, discovery of supplied selection, signing/auth, metadata inspection, execution and durable output"),cases=results))
                    print(label,round(run["wall_seconds"],3),"passed",flush=True)
        finally:
            server.shutdown();server.server_close()


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--netrc",type=Path,required=True)
    parser.add_argument("--product",choices=["hls-l30","hls-s30"])
    parser.add_argument("--benchmark",choices=["mirror","live"])
    parser.add_argument("--binary",type=Path,default=ROOT/"target/release/tile-ripper")
    parser.add_argument("--rounds",type=int,default=2)
    args=parser.parse_args()
    try:
        if args.benchmark:
            benchmark(args.binary,args.netrc,args.benchmark=="live",args.rounds)
        else:
            compare(args.netrc,args.product)
    except Exception as exc:
        # Requests exceptions may contain runtime URLs. Keep them off console.
        raise SystemExit("Provider qualification failed ("+type(exc).__name__+"); no temporary URLs logged") from None
