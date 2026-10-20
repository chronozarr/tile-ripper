"""Matched durable-output benchmark; original source ranges, independent readers.

Use a fresh TILE_RIPPER_DURABLE_WORK. Generated datasets are removed only after
every pixel and both masks have been checked; compact measurements are retained.
"""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import statistics
import subprocess
import time

import numpy as np
import rasterio
from rasterio.windows import Window
import xarray as xr
import zarr

import real_multiscene as real
from scientific_output import invoke, save

ROOT = Path(__file__).resolve().parents[1]
WORK = Path(os.environ.get("TILE_RIPPER_DURABLE_WORK", ROOT/"prototype-output/durable-output/benchmark")).resolve()
DOCS = ROOT/"docs/durable-output"
OLD = ROOT/"prototype-output/durable-output/baseline-82e21b1"
NEW = ROOT/"target/scientific-output/release/tile-ripper"
CANDIDATE = ROOT/"target/scientific-output/release/examples/durable_output_candidate"
WINDOWS = {"small":(7300,2100,96,96), "medium":(6144,1024,512,512), "large":(6144,1024,2048,2048)}


def validate(dest, manifest, refs):
    """Both independent libraries load every value and mask, outside timing."""
    root = zarr.open_group(str(dest), mode="r")
    assert root.attrs["status"] == "complete"
    hashes = []
    for record in manifest["outputs"]:
        g = root[record["group"]]
        data = refs[record["asset_id"]]
        valid = (data != 0).astype("u1")
        np.testing.assert_array_equal(g["values"][0], data)
        np.testing.assert_array_equal(g["valid"][0], valid)
        assert np.all(g["aoi_mask"][0] == 1)
        with xr.open_zarr(str(dest),group=record["group"],consolidated=False,decode_cf=False,chunks=None) as ds:
            assert ds["values"].dtype == np.dtype("u2")
            np.testing.assert_array_equal(ds["values"].values[0], data)
            np.testing.assert_array_equal(ds["valid"].values[0], valid)
            assert np.all(ds["aoi_mask"].values == 1)
        hashes.append(hashlib.sha256(data.astype("<u2").tobytes()).hexdigest())
    return hashes


def read_probe(dest, manifest):
    """Warm independent Zarr-Python full/64x64 reads with CPU and wall cost."""
    root = zarr.open_group(str(dest),mode="r")
    start, cpu = time.perf_counter(), time.process_time()
    count = 0
    for record in manifest["outputs"]:
        g = root[record["group"]]
        for name in ["values","valid","aoi_mask"]:
            count += g[name][:].nbytes
    full = dict(wall_seconds=time.perf_counter()-start,cpu_seconds=time.process_time()-cpu,bytes=count)
    start, cpu = time.perf_counter(), time.process_time()
    for record in manifest["outputs"]:
        for name in ["values","valid","aoi_mask"]:
            root[record["group"]][name][0,:64,:64]
    return dict(full=full,subset=dict(wall_seconds=time.perf_counter()-start,cpu_seconds=time.process_time()-cpu))


def main(action, reps, candidate_modes=None):
    WORK.mkdir(parents=True,exist_ok=True)
    sources_all = json.loads((real.WORK/"sources.json").read_text())["sources"]
    sources = sources_all[:4]
    mirror = real.Mirror(real.WORK/"mirror",sources_all)
    digest = mirror.verify_pages()
    interposer = WORK/"io.dylib"
    subprocess.run(["cc","-O2","-dynamiclib",str(ROOT/"bench/durable_output_io.c"),"-o",str(interposer)],check=True)
    modes = ["old","stream"] if action == "baseline" else ["old","raw","gzip","blosc_zstd","raw128","raw512","stream"]
    if action != "baseline" and int(zarr.__version__.split(".")[0]) >= 3:
        modes.append("sharded")
    if candidate_modes is not None:
        modes = candidate_modes
    binaries = [OLD,NEW,CANDIDATE] if action!="baseline" else [OLD,NEW]
    hashes = {str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in binaries}
    runs_path = WORK/"runs.json"
    runs = json.loads(runs_path.read_text()) if runs_path.exists() else []
    for workload, window in WINDOWS.items():
        refs = {}
        for i,s in enumerate(sources):
            with rasterio.open(mirror.path(f"/{i}.tif")) as src:
                refs[s["id"]] = src.read(1,window=Window(*window))
        for latency, mbps in [(0,0),(8,100)]:
            server = real.start(mirror,latency=latency,mbps=mbps)
            try:
                cells = [(r,m) for r in range(reps) for m in modes]
                random.Random(20261020).shuffle(cells)
                for rep, mode in cells:
                    if any(r["workload"]==workload and r["link"]["latency_ms"]==latency and r["rep"]==rep and r["mode"]==mode for r in runs):
                        continue
                    cell = WORK/f"{workload}-{latency}-{rep}-{mode}"
                    cell.mkdir(exist_ok=True)
                    dest = cell/"data"
                    assert not dest.exists(),"never overwrite a previous dataset"
                    request = real.request(sources,server,window)
                    request["sink"] = dict(kind="callback") if mode=="stream" else dict(kind="zarr_directory",path=str(dest))
                    path = cell/"request.json"; save(path,request)
                    binary = OLD if mode=="old" else NEW
                    args = ["stream" if mode=="stream" else "fetch",path,"--gdal-free","--workers","2","--per-host","2"]
                    if mode not in ["old","raw","stream"]:
                        binary = CANDIDATE
                        codec = mode if mode in ["gzip","blosc_zstd","sharded"] or mode.startswith("v3_") else "raw"
                        edge = 128 if mode=="raw128" else 512 if mode=="raw512" else 256
                        args = [path,codec,str(edge)]
                    server.reset()
                    result = invoke(args,binary=binary,env=dict(DYLD_INSERT_LIBRARIES=str(interposer),TR_IO_OUTPUT=str(cell),
                        TR_IO_REPORT=str(cell/"io.json"),TILE_RIPPER_STAGE_TRACE=str(cell/"stages.jsonl"),TILE_RIPPER_PER_HOST="2"))
                    http = copy.deepcopy(server.counts)
                    if result["code"]!=0:
                        # Experimental means failures are evidence, never a
                        # successful duration or a reason to skip stable modes.
                        assert mode=="sharded",(mode,result)
                        assert "Sharding did not allocate a large enough buffer" in result["stderr"],result
                        assert not dest.exists() and not list(cell.glob(".data.partial-*"))
                        result.pop("stdout")
                        runs.append(dict(workload=workload,link=dict(latency_ms=latency,mbps=mbps),rep=rep,mode=mode,
                            process=result,http=http,filesystem=json.loads((cell/"io.json").read_text()),
                            published=False,staging_cleaned=True,passed=False))
                        save(runs_path,runs)
                        print(workload,latency,rep,mode,"FAILED: zarrs shard bound; no publication",flush=True)
                        continue
                    stdout = result.pop("stdout")
                    manifest = json.loads(stdout.splitlines()[-1]) if mode=="stream" else json.loads(stdout)
                    if mode != "stream":
                        assert manifest["format"] == ("zarr_v3" if mode=="sharded" or mode.startswith("v3_") else "zarr_v2")
                    observed = manifest["observed"]
                    assert observed["decoded_bytes"] == sum(a.size*2 for a in refs.values())
                    files = [p for p in dest.rglob("*") if p.is_file()] if dest.exists() else []
                    output_bytes = sum(p.stat().st_size for p in files)
                    values_hashes = validate(dest,manifest,refs) if dest.exists() else []
                    reading = read_probe(dest,manifest) if dest.exists() else None
                    trace = cell/"stages.jsonl"
                    row = dict(workload=workload,link=dict(latency_ms=latency,mbps=mbps),rep=rep,mode=mode,process=result,
                        http=http,observed=observed,filesystem=json.loads((cell/"io.json").read_text()),
                        stages=[json.loads(line) for line in trace.read_text().splitlines()] if trace.exists() else [],
                        output_files=len(files),output_bytes=output_bytes,reading=reading,values_sha256=values_hashes,passed=True)
                    runs.append(row)
                    save(WORK/"runs.json",runs)
                    if dest.exists(): shutil.rmtree(dest)
                    print(workload,latency,rep,mode,round(result["wall_seconds"],4),flush=True)
            finally:
                server.shutdown(); server.server_close()
    summary = []
    for workload in WINDOWS:
        for latency in [0,8]:
            for mode in modes:
                rows = [r for r in runs if r["workload"]==workload and r["link"]["latency_ms"]==latency and r["mode"]==mode]
                failed = [r for r in rows if not r["passed"]]
                rows = [r for r in rows if r["passed"]]
                if not rows:
                    summary.append(dict(workload=workload,latency_ms=latency,mode=mode,repetitions=len(failed),
                        successful_runs=0,failed_runs=len(failed),passed=False,
                        error="zarrs sharding encode bound error; no destination published"))
                    continue
                def med(fn): return statistics.median(fn(r) for r in rows)
                summary.append(dict(workload=workload,latency_ms=latency,mode=mode,repetitions=len(rows)+len(failed),
                    successful_runs=len(rows),failed_runs=len(failed),passed=not failed,
                    wall_seconds=med(lambda r:r["process"]["wall_seconds"]),cpu_seconds=med(lambda r:r["process"]["cpu_seconds"]),
                    rss_bytes_max=max(r["process"]["peak_rss_bytes"] for r in rows),
                    output_files=med(lambda r:r["output_files"]),output_bytes=med(lambda r:r["output_bytes"]),
                    writes=med(lambda r:sum(r["filesystem"][k]["writes"] for k in ["output","manifest"])),
                    fsyncs=med(lambda r:sum(r["filesystem"][k]["fsyncs"] for k in ["output","manifest","directories"])),
                    fullsyncs=med(lambda r:sum(r["filesystem"][k]["fullsyncs"] for k in ["output","manifest","directories"])),
                    sync_seconds=med(lambda r:sum(r["filesystem"][k][f] for k in ["output","manifest","directories"] for f in ["fsync_seconds","fullsync_seconds"]))))
    assert hashes == {str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in binaries},"binary changed during timing matrix"
    record = dict(passed=all(r["passed"] for r in runs),completed=True,
        stable_modes_passed=all(r["passed"] for r in runs if r["mode"]!="sharded"),
        experimental_sharding_ready=all(r["passed"] for r in runs if r["mode"]=="sharded"),
        action=action,runs=runs,summary=summary,source_versions=sources,mirror_index_sha256=digest,
        binaries=hashes,
        readers=dict(zarr=zarr.__version__,xarray=xr.__version__,rasterio=rasterio.__version__),
        scope="Four unchanged original Sentinel-2 B04/B08 assets, two dates, 96/512/2048 square windows; frozen warm local HTTP and simulated 8ms/100Mbit/s link. Fresh child; shuffled cells. Source acquisition/decoding and durable output included. Independent verification and read probes excluded from write timing.",
        limitations=["Warm host caches, one macOS workstation; not public-provider throughput or physical device IOPS.",
            "Source read/decode worker sums overlap sink time and backpressure; do not add these durations as disjoint wall stages.",
            "CPU and RSS measured by wait4. Reservations exclude runtime, metadata containers, allocator and filesystem page cache.",
            "Stream writes descriptors only and is a lower-work baseline. Output modes retain the same values, masks, coordinates, timestamps and provenance.",
            "POSIX host fsync and macOS full device flushes counted separately; no unsynchronized mode is timed.",
            "Final publication timing is external diagnostic data; the staged manifest cannot record operations that occur after its own synchronization."])
    save(DOCS/f"{action}.json",record)
    print(json.dumps(summary,indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action",choices=["baseline","benchmark"])
    parser.add_argument("--reps",type=int,default=3)
    args = parser.parse_args()
    main(args.action,args.reps)
