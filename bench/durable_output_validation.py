"""Scientific acceptance, phase-specific faults and memory checks for output.

Use a fresh TILE_RIPPER_DURABLE_WORK. Fault injection is confined to child
processes via the macOS interposer; no fault controls exist in production code.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import threading
import time

import numpy as np
import rasterio
import zarr
import xarray as xr
from rasterio.transform import from_origin
from rasterio.windows import Window

import scientific_output as gate
import stac_workflow as fixture
from native_sentinel_workflow import unified

ROOT = Path(__file__).resolve().parents[1]
WORK = Path(os.environ.get("TILE_RIPPER_DURABLE_WORK",ROOT/"prototype-output/durable-output/validation")).resolve()
DOCS = ROOT/"docs/durable-output"
BIN = ROOT/"target/scientific-output/release/tile-ripper"
CANDIDATE = BIN.parent/"examples/durable_output_candidate"


def source_server():
    s = fixture.Server(("127.0.0.1",0),gate.FaultHandler)
    s.root = WORK/"fixtures"
    s.files,s.features = fixture.create(s.root,s.server_port)
    s.features[0]["properties"]["datetime"] = "2025-07-29T15:00:00.123456Z"
    s.features[1]["properties"]["datetime"] = "2025-07-28T15:00:00.987654Z"
    s.queries,s.seen_conditions = [],[]
    s.reset()
    threading.Thread(target=s.serve_forever,daemon=True).start()
    return s


def request(s, name):
    r = unified(fixture.request(s.server_port,"unused",large=True))
    r["time"]["end"] = "2025-07-30T00:00:00Z"
    r["sink"] = dict(kind="zarr_directory",path=str(WORK/name))
    r["budget"]["buffer_bytes"] = 16*2**20
    return r


def candidate_args(path, mode, edge=256):
    return [str(BIN),"fetch",str(path),"--gdal-free"] if mode=="default" else [str(CANDIDATE),str(path),mode,str(edge)]


def invoke_request(req, mode, edge=256, env=None):
    path = Path(req["sink"]["path"]).with_suffix(".json")
    gate.save(path,req)
    args = candidate_args(path,mode,edge)
    return gate.invoke(args[1:],binary=Path(args[0]),env=env)


def checks(modes=None):
    # Replay the inherited mixed-grid, late-mutation and actual mid-write signal
    # oracle with the new default binary, retaining the old receipt unchanged.
    gate.WORK,gate.DOCS,gate.BIN = WORK/"inherited",DOCS/"inherited",BIN
    gate.controlled()
    s = source_server()
    cases = []
    if modes is None:
        modes = [("raw",128),("raw",512),("gzip",256),("blosc_zstd",256)]
        if int(zarr.__version__.split(".")[0]) >= 3: modes.append(("sharded",256))
    try:
        for mode,edge in modes:
            req = request(s,f"{mode}-{edge}.zarr")
            result = invoke_request(req,mode,edge)
            assert result["code"]==0,result
            manifest = json.loads(result.pop("stdout"))
            local = {source["asset_id"]:str(s.root/f'scene{source["item_id"][-1]}-{source["asset_key"]}.tif') for source in manifest["sources"]}
            expected_format = "zarr_v3" if mode=="sharded" or mode.startswith("v3_") else "zarr_v2"
            verified = gate.verify(Path(req["sink"]["path"]),req,local,expected_format=expected_format)
            # Metadata and observation order must be repeatable, including
            # coordinates and exact subsecond timestamps on distinct grids.
            cases.append(dict(mode=mode,edge=edge,process=result,**verified))
            for fault in ["late_mutation","fail_data"]:
                failed = request(s,f"{mode}-{edge}-{fault}.zarr")
                if fault=="late_mutation":
                    failed["source"] = dict(kind="assets",assets=[dict(id="late",format="cog",locator=f"http://127.0.0.1:{s.server_port}/scene0-red.tif",datetime="2025-07-29T15:00:00.123456Z")])
                    failed["variables"] = [dict(kind="band",index=1)]
                s.output = Path(failed["sink"]["path"])
                s.mutated_after_writes = False
                setattr(s,fault,True)
                r = invoke_request(failed,mode,edge)
                setattr(s,fault,False);s.current_etag='"fixed-fixture"'
                assert r["code"]!=0 and not s.output.exists(),r
                assert not list(WORK.glob(f".{s.output.name}.partial-*"))
                if fault=="late_mutation": assert s.mutated_after_writes
                cases.append(dict(mode=mode,edge=edge,fault=fault,passed=True,error=r["stderr"].strip()))
            empty = request(s,f"{mode}-{edge}-empty.zarr")
            empty["time"] = dict(kind="interval",start="2020-01-01T00:00:00Z",end="2020-01-02T00:00:00Z")
            r = invoke_request(empty,mode,edge)
            assert r["code"]==0 and json.loads(r["stdout"])["outputs"]==[],r
            assert zarr.open_group(empty["sink"]["path"],mode="r").attrs["status"]=="complete"
            tiny = request(s,f"{mode}-{edge}-tiny.zarr");tiny["budget"]["buffer_bytes"]=16
            r = invoke_request(tiny,mode,edge)
            assert r["code"]!=0 and not Path(tiny["sink"]["path"]).exists(),r
            cases.append(dict(mode=mode,edge=edge,empty_and_small_buffer=True,passed=True))
    finally:
        s.shutdown();s.server_close()
    gate.save(DOCS/"candidates.json",dict(passed=True,cases=cases,
        readers=dict(zarr=zarr.__version__,xarray=xr.__version__,rasterio=rasterio.__version__),
        binary_sha256=hashlib.sha256(CANDIDATE.read_bytes()).hexdigest()))
    print(json.dumps(dict(passed=True,candidate_checks=len(cases))))


def live():
    gate.WORK,gate.DOCS,gate.BIN = WORK/"default",DOCS/"live-default",BIN
    gate.live()
    req = json.loads((ROOT/"docs/native-sentinel/request.json").read_text())
    req["time"] = dict(kind="interval",start="2025-01-02T00:00:00Z",end="2025-01-13T23:59:59Z")
    req["variables"] = [dict(kind="named",name=n) for n in ["B04","B08","B11"]]
    req["source"]["limits"]["max_items"] = 32
    req["source"]["limits"]["max_assets"] = 64
    req["budget"] = dict(buffer_bytes=16*2**20,max_nodes=100000,chunk_edge=256)
    rows = []
    modes = ["gzip","blosc_zstd"]
    if int(zarr.__version__.split(".")[0])>=3:modes.append("sharded")
    for mode in modes:
        req["sink"] = dict(kind="zarr_directory",path=str(WORK/f"{mode}.zarr"))
        result = invoke_request(req,mode)
        assert result["code"]==0,result
        result.pop("stdout")
        verified = gate.verify(Path(req["sink"]["path"]),req,expected_format="zarr_v3" if mode=="sharded" else "zarr_v2")
        assert verified["observations"]==6 and verified["samples"]==46376
        rows.append(dict(mode=mode,process=result,**verified))
    gate.save(DOCS/"live-candidates.json",dict(passed=True,candidates=rows,
        binary_sha256=hashlib.sha256(CANDIDATE.read_bytes()).hexdigest(),readers=dict(zarr=zarr.__version__,xarray=xr.__version__)))
    print(json.dumps(dict(passed=True,candidates=len(rows),observations_per_candidate=6,samples_per_candidate=46376)))


def faults(mode="default"):
    s = source_server()
    interposer = WORK/"io.dylib"
    subprocess.run(["cc","-O2","-dynamiclib",str(ROOT/"bench/durable_output_io.c"),"-o",str(interposer)],check=True)
    rows = []
    # All points before/after the atomic commit. Publication faults after the
    # rename must return failure while preserving a complete, readable dataset.
    points = ["write","file_sync","pixel_write","pixel_sync","directory_sync","ancestor_sync","final_sync","rename","parent_sync","post_sync"]
    try:
        for point in points:
            for action in ["fail","int","kill","race"]:
                if action=="race" and point!="rename":continue
                cell=WORK/f"{point}-{action}";cell.mkdir()
                out=cell/"data"
                req=request(s,str(out));path=cell/"request.json";gate.save(path,req)
                marker=cell/"marker"
                env={**os.environ,"TILE_RIPPER_WORKERS":"2","DYLD_INSERT_LIBRARIES":str(interposer),"TR_IO_OUTPUT":str(cell),
                    "TR_IO_REPORT":str(cell/"io.json"),"TR_FAULT_POINT":point,"TR_FAULT_ACTION":"fail" if action=="fail" else "pause","TR_FAULT_MARKER":str(marker)}
                with tempfile.TemporaryFile() as log:
                    p=subprocess.Popen(candidate_args(path,mode),env=env,stdout=log,stderr=log)
                    deadline=time.monotonic()+30
                    while not marker.exists() and p.poll() is None and time.monotonic()<deadline:time.sleep(0.002)
                    assert marker.exists(),(point,action,p.poll())
                    committed=point in ["parent_sync","post_sync"]
                    if action in ["int","kill"]:
                        p.send_signal(signal.SIGINT if action=="int" else signal.SIGKILL)
                        time.sleep(0.05) # allow the cooperative SIGINT handler to run
                    # The rename pause is inside the atomic publication call,
                    # after the last application cancellation poll. A racing
                    # cancellation may commit; it must still finish syncing.
                    if point=="rename" and action=="int":committed=True
                    if action=="race":out.mkdir();(out/"winner").write_text("keep me")
                    marker.with_suffix(".release").touch()
                    p.wait(timeout=45);log.seek(0);text=log.read().decode()
                stages=list(cell.glob(".data.partial-*"))
                if committed:
                    assert out.exists()
                    manifest=json.loads((out/"manifest.json").read_text())
                    local={v["asset_id"]:str(s.root/f'scene{v["item_id"][-1]}-{v["asset_key"]}.tif') for v in manifest["sources"]}
                    verified=gate.verify(out,req,local,expected_format="zarr_v3" if mode=="sharded" or mode.startswith("v3_") else "zarr_v2")
                    assert verified["passed"]
                    if action=="fail":assert p.returncode!=0 and "durability is unconfirmed" in text,text
                    if action=="int":assert p.returncode==0,text # commit wins; complete sync
                elif action=="race":
                    assert p.returncode!=0 and (out/"winner").read_text()=="keep me"
                    assert len(list(out.iterdir()))==1 and not stages
                else:
                    assert p.returncode!=0 and not out.exists(),text
                    assert bool(stages)==(action=="kill")
                if action=="kill" and not committed:
                    # Retry must use a new owned sibling and leave the orphan.
                    retry=invoke_request(req,mode)
                    assert retry["code"]==0 and out.exists() and all(p.exists() for p in stages),retry
                    shutil.rmtree(out)
                rows.append(dict(point=point,action=action,exit_code=p.returncode,commit_point_passed=committed,
                    staging_retained=bool(stages),passed=True))
    finally:
        s.shutdown();s.server_close()
    gate.save(DOCS/"faults.json",dict(passed=True,cases=rows,
        binary_sha256=hashlib.sha256((BIN if mode=="default" else CANDIDATE).read_bytes()).hexdigest(),
        scope="Application-visible faults/signals at actual syscalls; not power-cut emulation. Pre-commit failure leaves no destination; post-commit failure preserves complete output and refuses to report durable success."))
    print(json.dumps(dict(passed=True,fault_cases=len(rows))))


def memory(modes=None):
    """Logical output grows 64x; construction/verification stay tile-bounded."""
    binary_sha256=hashlib.sha256(CANDIDATE.read_bytes()).hexdigest()
    rows = []
    for n in [1024,8192]:
        source=WORK/f"source-{n}.tif"
        with rasterio.open(source,"w",driver="GTiff",width=n,height=n,count=1,dtype="uint16",
                crs="EPSG:32617",transform=from_origin(600000,4000020,10,10),nodata=0,
                tiled=True,blockxsize=256,blockysize=256,compress="deflate",predictor=2) as ds:
            for y in range(0,n,256):
                for x in range(0,n,256):
                    yy,xx=np.mgrid[y:y+256,x:x+256]
                    a=((xx*17+yy*31)%65535+1).astype("u2")
                    a[(xx+yy)%37==0]=0
                    ds.write(a,1,window=Window(x,y,256,256))
        if modes is None:
            modes=["raw","gzip","blosc_zstd"]
            if int(zarr.__version__.split(".")[0])>=3:modes.append("sharded")
        for mode in modes:
            out=WORK/f"{n}-{mode}.zarr"
            req=dict(source=dict(kind="assets",assets=[dict(id=f"observation-{i}",format="cog",locator=str(source),datetime=f"2025-01-0{2-i}T12:00:00.123456Z") for i in range(2)]),
                spatial=dict(kind="bounds",crs="EPSG:32617",bbox=[600000,4000020-n*10,600000+n*10,4000020]),
                variables=[dict(kind="band",index=1)],grid=dict(kind="native"),sink=dict(kind="zarr_directory",path=str(out)),
                budget=dict(buffer_bytes=16*2**20,max_nodes=100000,chunk_edge=256))
            result=invoke_request(req,mode)
            assert result["code"]==0,result
            manifest=json.loads(result.pop("stdout"))
            root=zarr.open_group(str(out),mode="r")
            samples=0
            with rasterio.open(source) as src:
                for record in manifest["outputs"]:
                    g=root[record["group"]]
                    for y in range(0,n,256):
                        for x in range(0,n,256):
                            expected=src.read(1,window=Window(x,y,256,256))
                            np.testing.assert_array_equal(g["values"][0,y:y+256,x:x+256],expected)
                            np.testing.assert_array_equal(g["valid"][0,y:y+256,x:x+256],(expected!=0).astype("u1"))
                            assert np.all(g["aoi_mask"][0,y:y+256,x:x+256]==1)
                            samples+=expected.size
                    with xr.open_zarr(str(out),group=record["group"],decode_cf=False,consolidated=False,chunks=None) as ds:
                        assert ds["values"].shape==(1,n,n)
                        np.testing.assert_array_equal(ds["values"].isel(time=0,y=slice(0,16),x=slice(0,16)).values,src.read(1,window=Window(0,0,16,16)))
                        expected_time=np.datetime64(record["datetime"].replace("Z",""),"ns").astype("i8")
                        assert ds.time.values[0]==expected_time
            assert result["peak_rss_bytes"]<192*2**20,(mode,n,result["peak_rss_bytes"])
            rows.append(dict(mode=mode,edge=n,logical_array_bytes=2*n*n*4,samples_checked=samples,
                process=result,observed=manifest["observed"],passed=True))
            gate.save(WORK/"memory-progress.json",rows)
            shutil.rmtree(out)
            print("memory",n,mode,round(result["peak_rss_bytes"]/2**20,1),"MiB",flush=True)
    for mode in modes:
        a,b=[r for r in rows if r["mode"]==mode]
        assert a["observed"]["output_io"]["buffered_bytes_peak"]==b["observed"]["output_io"]["buffered_bytes_peak"]
        assert b["process"]["peak_rss_bytes"]<a["process"]["peak_rss_bytes"]+64*2**20
    assert binary_sha256==hashlib.sha256(CANDIDATE.read_bytes()).hexdigest(),"binary changed during memory gate"
    gate.save(DOCS/"memory.json",dict(passed=True,runs=rows,binary_sha256=binary_sha256,
        scope="Two separately timestamped observations of a deterministic tiled UInt16 source; 1024 and 8192 square native grids; exact independent GDAL comparison of every value and both masks in bounded 256-square tiles. Logical array bytes include values and both masks; not serialized size.",
        limits="192 MiB RSS gate; growth under 64 MiB when logical arrays grow 64x. Output scratch reservation and source scheduler reservations are separate. Metadata and file-key bookkeeping grow with max_nodes; OS page cache and reference-reader memory are not process reservations."))


def regressions():
    # These inherited reference harnesses use Zarr-Python 2's DirectoryStore.
    import native_cog_checks as native_faults
    import native_cog_exact as exact
    import consumer_validation as consumer
    binary=BIN.parent/"tile-ripper-native-stats"
    digest=hashlib.sha256(binary.read_bytes()).hexdigest()
    native_faults.BIN,native_faults.WORK=binary,WORK/"cog-faults"
    consumer.BIN=binary
    native_faults.main()
    os.environ["TILE_RIPPER_ORACLE_BINARY"]=str(BIN.parent/"examples/native_cog_oracle")
    exact.main()
    consumer.WORK=WORK/"consumer"
    consumer.main()
    assert hashlib.sha256(binary.read_bytes()).hexdigest()==digest
    gate.save(DOCS/"regressions.json",dict(passed=True,binary_sha256=digest,
        native_cog_faults=json.loads((native_faults.WORK/"report.json").read_text()),
        native_cog_exact=json.loads((ROOT/"prototype-output/native-cog/exact/report.json").read_text()),
        streaming_consumer=json.loads((consumer.WORK/"receipt.json").read_text())))


def reopen_v2():
    assert int(zarr.__version__.split(".")[0])==2
    rows=[]
    for mode,edge in [("raw",128),("raw",512),("gzip",256),("blosc_zstd",256)]:
        out=WORK/f"{mode}-{edge}.zarr"
        req=json.loads(out.with_suffix(".json").read_text())
        manifest=json.loads((out/"manifest.json").read_text())
        local={s["asset_id"]:str(WORK/"fixtures"/f'scene{s["item_id"][-1]}-{s["asset_key"]}.tif') for s in manifest["sources"]}
        checked=gate.verify(out,req,local)
        rows.append(dict(mode=mode,edge=edge,passed=True,observations=checked["observations"],samples=checked["samples"]))
    gate.save(DOCS/"interoperability-v2.json",dict(passed=True,candidates=rows,
        readers=dict(zarr=zarr.__version__,xarray=xr.__version__,rasterio=rasterio.__version__)))


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action",choices=["checks","live","faults","memory","regressions","reopen-v2"])
    args=parser.parse_args()
    WORK.mkdir(parents=True,exist_ok=True)
    {"checks":checks,"live":live,"faults":faults,"memory":memory,"regressions":regressions,"reopen-v2":reopen_v2}[args.action]()
