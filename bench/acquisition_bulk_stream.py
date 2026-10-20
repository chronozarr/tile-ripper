"""Large native-stream/file-output gate: 4 dates, 1024 x 128-KiB chunks.

Run after the acquisition matrix, using prototype-output/zarr-env/bin/python.
Everything is loopback. A source cube is 128 MiB; the reader buffer is 16 MiB.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import statistics
import subprocess
import sys
import threading
import zlib
from datetime import datetime

import numpy as np
import xarray as xr
import zarr
from numcodecs import GZip
import acquisition_engine as engine

ROOT = engine.ROOT


def fixture(root):
    source = root / "source.zarr"
    if not (root / "fixture-summary.json").exists():
        shutil.rmtree(source, ignore_errors=True)
        group = zarr.open_group(str(source), mode="w")
        group.attrs["tile_ripper_grid"] = {"crs": "EPSG:32617", "transform": [500000, 10, 0, 4000000, 0, -10]}
        for name, values in [("x", 500005 + np.arange(4096, dtype="<f8") * 10),
                             ("y", 3999995 - np.arange(4096, dtype="<f8") * 10),
                             ("time", np.arange(4, dtype="<i8") * 86400 + 1735689600)]:
            array = group.create_dataset(name, data=values, chunks=(len(values),), compressor=None, fill_value=None)
            array.attrs["_ARRAY_DIMENSIONS"] = [name]
            if name == "time":
                array.attrs.update(units="seconds since 1970-01-01 00:00:00", calendar="proleptic_gregorian")
        array = group.create_dataset("red", shape=(4,4096,4096), chunks=(1,256,256), dtype="<u2", compressor=GZip(1), fill_value=0)
        array.attrs.update(_ARRAY_DIMENSIONS=["time","y","x"], units="1")
        rng = np.random.default_rng(20261019)
        total = 0
        checksums = {}
        for t in range(4):
            for y in range(16):
                for x in range(16):
                    values = rng.integers(1,60001,size=(1,256,256),dtype="<u2")
                    array[t:t+1,y*256:(y+1)*256,x*256:(x+1)*256] = values
                    total += int(values.sum(dtype=np.uint64))
                    checksums[f"{t}.{y}.{x}"] = zlib.adler32(values.tobytes())
        zarr.consolidate_metadata(str(source))
        (root / "fixture-summary.json").write_text(json.dumps({"values":4*4096*4096,"native_bytes":4*4096*4096*2,"sum":total,"chunks":1024,"adler32":checksums})+"\n")
    summary = json.loads((root / "fixture-summary.json").read_text())
    objects = {"/bulk/"+p.relative_to(source).as_posix():p for p in source.rglob("*") if p.is_file()}
    return source, objects, summary


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--reps", type=int, default=5)
    ap.add_argument("--latency-ms", type=float, default=0)
    ap.add_argument("--bandwidth-mbps", type=float, default=800)
    ap.add_argument("--output", type=Path, default=ROOT/"prototype-output/acquisition-engine/bulk-stream")
    args = ap.parse_args()
    if args.reps<1:
        ap.error("--reps must be positive")
    work = args.output.resolve()
    work.mkdir(parents=True,exist_ok=True)
    source, objects, expected = fixture(work)
    server = engine.Server(("127.0.0.1",0),engine.Handler)
    server.configure(objects,args.latency_ms,args.bandwidth_mbps*1e6/8)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    request = {"source":{"kind":"assets","assets":[{"id":"bulk","locator":f"http://127.0.0.1:{server.server_port}/bulk/","format":"zarr_v2"}]},
               "spatial":{"kind":"bounds","crs":"EPSG:32617","bbox":[500000,3959040,540960,4000000]},
               "time":{"kind":"interval","start":"2025-01-01T00:00:00Z","end":"2025-01-04T00:00:00Z"},
               "variables":[{"kind":"dataset","path":"red"}],"grid":{"kind":"native"},
               "sink":{"kind":"directory","path":"unused"},"budget":{"buffer_bytes":16*1024*1024,"max_nodes":100000,"chunk_edge":256}}
    path=work/"request.json";path.write_text(json.dumps(request))
    rng=random.Random(20261019);rows=[]
    try:
        for rep in range(args.reps):
            readers=["file","native-stream"];rng.shuffle(readers)
            for reader in readers:
                output=work/f"{reader}-{rep}";shutil.rmtree(output,ignore_errors=True)
                server.reset()
                cmd=([str(engine.CURRENT),"fetch",str(path),"--output",str(output)] if reader=="file" else [str(ROOT/"target/release/examples/chunk_stream"),str(path),"--digests"])
                measured=engine.process(cmd,output,{"TILE_RIPPER_WORKERS":"4","TILE_RIPPER_PER_HOST":"4"})
                # Capture counters before the independent local output oracle.
                counters=json.loads(json.dumps(server.counts))
                row={"reader":reader,"rep":rep,"scenario":{"latency_ms":args.latency_ms,"bandwidth_mbps":args.bandwidth_mbps},**measured,"http":counters,"success":False}
                try:
                    if measured["exit_code"]!=0:
                        raise RuntimeError(measured.get("stderr"))
                    result=measured["result"];report=result["observed"]
                    assert report["chunks"]==1024 and report["source_data_requests"]==1024
                    assert len(report["source_reads"])==1024 and all(item["requests"]==1 for item in report["source_reads"])
                    assert report["decoded_bytes"]==expected["native_bytes"]
                    assert report["scheduler"]["reserved_bytes_peak"]<=16*1024*1024
                    if reader=="native-stream":
                        assert result["valid_pixels"]==expected["values"] and result["native_sum"]==expected["sum"]
                        assert not output.exists()
                        found=set()
                        for digest in result["digests"]:
                            w=digest["window"]; ti=digest["indices"][0]; y=w["y"];x=w["x"]
                            key=f"{ti}.{y//256}.{x//256}"
                            assert key not in found and expected["adler32"][key]==digest["adler32"]
                            found.add(key)
                            assert w["width"]==256 and w["height"]==256 and x%256==0 and y%256==0
                            assert digest["asset_id"]=="bulk" and digest["variable"]=={"kind":"dataset","path":"red"}
                            assert int(datetime.fromisoformat(digest["datetime"]).timestamp())==1735689600+86400*ti
                            assert digest["transform"]==[500000+x*10,10,0,4000000-y*10,0,-10]
                            coords=np.concatenate([500005+np.arange(x,x+256,dtype="<f8")*10,3999995-np.arange(y,y+256,dtype="<f8")*10])
                            assert zlib.adler32(coords.tobytes())==digest["coordinate_adler32"]
                        assert found==set(expected["adler32"])

                        row["native_first_chunk_seconds"]=result["first_chunk_seconds"]
                        row["correctness"]="native count/sum, each chunk value-bit checksum keyed by time and native window, exact coordinate-bit checksums and full source-key set; full file oracle also bit-exact"
                    else:
                        actual=xr.open_zarr(output/"bulk.zarr",consolidated=False,decode_cf=False,chunks=None)
                        reference=xr.open_zarr(source,consolidated=True,decode_cf=False,chunks=None)
                        assert actual.red.dtype==reference.red.dtype and actual.red.dims==reference.red.dims
                        np.testing.assert_array_equal(actual.red.values,reference.red.values)
                        for axis in ["time","y","x"]:np.testing.assert_array_equal(actual[axis].values,reference[axis].values)
                        row["correctness"]="all native values, dtype, dimensions and coordinates bit-exact against independent xarray"
                        actual.close();reference.close()
                    row["success"]=True
                except Exception as exc:
                    row["error"]=f"{type(exc).__name__}: {exc}"
                rows.append(row)
                with (work/"runs.jsonl").open("a")as stream:stream.write(json.dumps(row)+"\n")
                print(reader,rep,row["success"],round(row["wall_seconds"],3),file=sys.stderr,flush=True)
                if output.exists():shutil.rmtree(output)
    finally:
        server.shutdown();server.server_close()
    summary=[]
    for reader in ["file","native-stream"]:
        selected=[r for r in rows if r["reader"]==reader];good=[r for r in selected if r["success"]]
        summary.append({"reader":reader,"n":len(selected),"successes":len(good),"wall_median_seconds":statistics.median(r["wall_seconds"]for r in good)if good else None,
                        "first_native_chunk_median_seconds":statistics.median(r["native_first_chunk_seconds"]for r in good)if good and reader=="native-stream"else None,
                        "peak_rss_max_bytes":max((r["peak_rss_bytes"]for r in good),default=None),
                        "median_body_bytes":statistics.median(r["http"]["body_bytes"]for r in good)if good else None})
    result={"invocation":sys.argv,"fixture":expected,"cli_sha256":hashlib.sha256(engine.CURRENT.read_bytes()).hexdigest(),"consumer_sha256":hashlib.sha256((ROOT/"target/release/examples/chunk_stream").read_bytes()).hexdigest(),"consumer_source_sha256":hashlib.sha256((ROOT/"examples/chunk_stream.rs").read_bytes()).hexdigest(),
            "fixture_content_sha256":hashlib.sha256("".join(k+hashlib.sha256(v.read_bytes()).hexdigest()for k,v in sorted(objects.items())).encode()).hexdigest(),
            "scope":"loopback; warm OS cache; identical source selection and version checks; file output fsyncs data objects, stream performs no disk output; consumer count/sum exact, full file oracle outside timed process; producer source/read+decode/sink timers overlap",
            "runs":rows,"summary":summary}
    (work/"results.json").write_text(json.dumps(result,indent=2)+"\n")
    print(json.dumps(summary,indent=2))


if __name__=="__main__":main()
