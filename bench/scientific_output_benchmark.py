"""Matched acquisition/output on versioned real Sentinel COG sparse mirrors.

Requires the existing production-streaming mirror (no new public downloads).
Reference decoding and output verification are outside timed child execution.
"""
import copy
import hashlib
import json
import os
from pathlib import Path
import random
import statistics
import subprocess
import sys
import numpy as np
import rasterio
from rasterio.windows import Window
import zarr
import real_multiscene as real
from scientific_output import invoke, save

ROOT = Path(__file__).resolve().parents[1]
WORK = Path(os.environ.get("TILE_RIPPER_OUTPUT_BENCH_WORK",ROOT/"prototype-output/scientific-output/benchmark"))
NATIVE = ROOT/"target/scientific-output/release/tile-ripper"
GDAL = ROOT/"target/scientific-output-gdal/release/tile-ripper"
MODES = ["native_stream","native_zarr","gdal_zarr","gdal_geotiff"]
WINDOWS = {"small":(7300,2100,96,96),"large":(6144,1024,2048,2048)}


def main():
    WORK.mkdir(parents=True,exist_ok=True)
    all_sources = json.loads((real.WORK/"sources.json").read_text())["sources"]
    sources = all_sources[:4]
    mirror = real.Mirror(real.WORK/"mirror",all_sources)
    captured_digest = mirror.verify_pages()
    interposer = None
    if sys.platform=="darwin":
        interposer = WORK/"io.dylib"
        subprocess.run(["cc","-O2","-dynamiclib",str(ROOT/"bench/io_profile.c"),"-o",str(interposer)],check=True)
    runs = []
    for workload,window in WINDOWS.items():
        x,y,width,height = window
        references = {}
        for i,a in enumerate(sources):
            with rasterio.open(mirror.path(f"/{i}.tif")) as src:
                references[a["id"]] = src.read(1,window=Window(x,y,width,height))
        for latency,mbps in [(0,0),(8,100)]:
            server = real.start(mirror,latency=latency,mbps=mbps)
            try:
                cells = [(rep,mode) for rep in range(3) for mode in MODES]
                random.Random(20261020).shuffle(cells)
                for rep,mode in cells:
                    cell = WORK/f"{workload}-{latency}-{rep}-{mode}"
                    cell.mkdir()
                    dest = cell/"data"
                    request = real.request(sources,server,window)
                    request["sink"] = dict(kind="callback") if mode.endswith("stream") else dict(kind="zarr_directory" if mode.endswith("zarr") else "directory",path=str(dest))
                    path = WORK/"request.json"; save(path,request)
                    args = ["stream" if mode.endswith("stream") else "fetch",path]
                    binary = NATIVE if mode.startswith("native") else GDAL
                    args += ["--backend","native" if mode.startswith("native") else "gdal","--workers","2","--per-host","2"]
                    environment = {"GDAL_CACHEMAX":"32","GDAL_NUM_THREADS":"1"}
                    if interposer:
                        environment.update(DYLD_INSERT_LIBRARIES=str(interposer),TR_IO_OUTPUT=str(cell),TR_IO_REPORT=str(cell/"io"))
                    server.reset()
                    result = invoke(args,binary=binary,env=environment)
                    http = copy.deepcopy(server.counts)
                    assert result["code"]==0,(mode,result)
                    stdout = result.pop("stdout")
                    value = json.loads(stdout.splitlines()[-1]) if mode.endswith("stream") else json.loads(stdout)
                    observed = value["observed"]
                    assert observed["decoded_bytes"] == len(sources)*width*height*2
                    assert observed["non_nodata_pixels"] == sum(int(np.count_nonzero(a)) for a in references.values())
                    if mode.endswith("zarr"):
                        store = zarr.open_group(str(dest),mode="r")
                        for record in value["outputs"]:
                            g = store[record["group"]]
                            data = references[record["asset_id"]]
                            np.testing.assert_array_equal(g["values"][0],data)
                            np.testing.assert_array_equal(g["valid"][0],(data!=0).astype("u1"))
                            assert np.all(g["aoi_mask"][0]==1)
                    elif mode=="gdal_geotiff":
                        for record in value["outputs"]:
                            w = record["window"]
                            with rasterio.open(dest/record["file"]) as ds:
                                np.testing.assert_array_equal(ds.read(1),references[record["asset_id"]][w["y"]-y:w["y"]-y+w["height"],w["x"]-x:w["x"]-x+w["width"]])
                    io_files = list(cell.glob("io.*"))
                    io = json.loads(io_files[0].read_text()) if io_files else None
                    if io is not None:
                        io["coverage"] = "Zarr output and manifest calls observed; GDAL buffered raster writes bypass this interposer, so their syscall totals are unavailable"
                    output_files = [p for p in dest.rglob("*") if p.is_file()] if dest.exists() else []
                    runs.append(dict(workload=workload,link=dict(latency_ms=latency,mbps=mbps),rep=rep,mode=mode,
                        process=result,http=http,observed=observed,filesystem=io,
                        output_files=len(output_files),output_bytes=sum(p.stat().st_size for p in output_files),
                        passed=True))
                    print(workload,latency,rep,mode,round(result["wall_seconds"],3),flush=True)
                    save(WORK/"runs.json",runs)
            finally:
                server.shutdown(); server.server_close()
    summary = []
    for workload in WINDOWS:
        for latency in [0,8]:
            for mode in MODES:
                rows = [r for r in runs if r["workload"]==workload and r["link"]["latency_ms"]==latency and r["mode"]==mode]
                summary.append(dict(workload=workload,latency_ms=latency,mode=mode,repetitions=len(rows),
                    wall_seconds_median=statistics.median(r["process"]["wall_seconds"] for r in rows),
                    cpu_seconds_median=statistics.median(r["process"]["cpu_seconds"] for r in rows),
                    rss_bytes_max=max(r["process"]["peak_rss_bytes"] for r in rows),
                    http_requests=[r["http"]["get"]+r["http"]["head"] for r in rows],
                    transferred_body_bytes=[r["http"]["body_bytes"] for r in rows],
                    output_bytes=[r["output_bytes"] for r in rows],output_files=[r["output_files"] for r in rows],
                    output_write_calls=[sum(r["filesystem"][kind]["writes"] for kind in ["output","manifest"]) if r["filesystem"] and mode != "gdal_geotiff" else None for r in rows],
                    gdal_raster_api_write_calls=[r["observed"]["chunks"] if mode == "gdal_geotiff" else None for r in rows],
                    output_store_writes=[(r["observed"].get("output_io") or {}).get("writes") for r in rows],
                    reservations_bytes=[r["observed"]["scheduler"]["reserved_bytes_peak"] for r in rows]))
    record = dict(passed=True,runs=runs,summary=summary,
        source_versions=sources,mirror_index_sha256=captured_digest,
        binaries={p.name+label:hashlib.sha256(p.read_bytes()).hexdigest() for p,label in [(NATIVE,"_native"),(GDAL,"_gdal")]},
        scope="4 original Sentinel-2 assets; two dates and B04/B08; one grid; frozen warm loopback and 8ms/100Mbit/s simulation; fresh child per run; three shuffled repetitions",
        caveats=["Native stream emits descriptors and writes no scientific dataset; timings do not establish an output speedup over doing no output.",
            "Both Zarr modes use the same per-observation sink, raw encoding, masks, coordinates and object fsync policy.",
            "Existing GeoTIFF workflow has fewer auxiliary products and uses GDAL flush behavior; filesystem durability work differs.",
            "Source capture, reference decoding and verification excluded; no public-provider throughput claim.",
            "RSS is measured process memory; scheduler reservations and writer buffers are separate accounting.",
            "File write counts are successful user-space/kernel-facing calls via the existing macOS interposer; not device write IOPS. GDAL buffered raster writes bypass it; GeoTIFF syscall totals are null. Distinct files and one GDAL RasterIO write per delivered chunk are reported separately."])
    save(ROOT/"docs/scientific-output/benchmark.json",record)


if __name__=="__main__":main()
