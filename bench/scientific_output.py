"""Independent Zarr/xarray/rasterio output gate; generated imagery stays local.

Run with prototype-output/zarr-env/bin/python bench/scientific_output.py checks|live.
No Tile-Ripper raster/geometry code is used by the reference calculation.
"""
import argparse
import copy
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import threading
import time

import numpy as np
import rasterio
from rasterio.features import geometry_mask
from rasterio.warp import transform_geom
from rasterio.windows import Window
import xarray as xr
import zarr
from pyproj import CRS
from shapely.geometry import box, mapping, shape

import stac_workflow as fixture
from native_sentinel_live import dense
from native_sentinel_workflow import unified

ROOT = Path(__file__).resolve().parents[1]
WORK = Path(os.environ.get("TILE_RIPPER_OUTPUT_WORK", ROOT / "prototype-output/scientific-output"))
DOCS = ROOT / "docs/scientific-output"
BIN = Path(os.environ.get("TILE_RIPPER_OUTPUT_BINARY", ROOT / "target/scientific-output/release/tile-ripper"))


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def invoke(args, binary=BIN, env=None):
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        start = time.monotonic()
        p = subprocess.Popen([str(binary), *map(str, args)], stdout=out, stderr=err,
            env={**os.environ, "TILE_RIPPER_WORKERS":"2", **(env or {})})
        _, status, usage = os.wait4(p.pid, 0)
        p.returncode = os.waitstatus_to_exitcode(status)
        out.seek(0); err.seek(0)
        text, error = out.read().decode(), err.read().decode()
    return dict(code=p.returncode, stdout=text, stderr=error, wall_seconds=time.monotonic()-start,
        cpu_seconds=usage.ru_utime+usage.ru_stime,
        peak_rss_bytes=usage.ru_maxrss*(1 if sys.platform=="darwin" else 1024))


def verify(output, request, local_sources=None, expected_format="zarr_v2"):
    manifest = json.loads((output / "manifest.json").read_text())
    assert manifest["status"] == "complete" and manifest["format"] == expected_format
    root = zarr.open_group(str(output), mode="r")
    assert root.attrs["status"] == "complete"
    sources = {s["asset_id"]:s for s in manifest["sources"]}
    references = []
    stamps = []
    for record in manifest["outputs"]:
        group = record["group"]
        zg = root[group]
        with xr.open_zarr(str(output), group=group, consolidated=False, decode_cf=False, chunks=None) as ds:
            ds.load()
        source = sources[record["asset_id"]]
        uri = (local_sources or {}).get(record["asset_id"], source.get("href",source.get("reference")))
        with rasterio.Env(GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", GDAL_CACHEMAX=32*2**20,
                GDAL_HTTP_HEADERS=(f'If-Match: {source["object_identity"]["etag"]}\r\n' if source.get("object_identity") else "")):
            with rasterio.open(uri) as src:
                spatial = request["spatial"]
                if spatial["kind"] == "polygon":
                    geo = dense(dict(coordinates=spatial["coordinates"]))
                    geo = transform_geom(spatial["crs"],src.crs,geo)
                else:
                    x0,y0,x1,y1 = spatial["bbox"]
                    geo = fixture.polygon(x0,y0,x1,y1)
                    geo = transform_geom(spatial["crs"],src.crs,geo)
                # Bounds of the polygon, independently transformed and clipped
                # to the source's pixel rectangle. Native grids are north-up.
                clipped = shape(geo).intersection(box(*src.bounds))
                bx0,by0,bx1,by1 = clipped.bounds
                points = np.asarray([[bx0,by0],[bx1,by1]])
                cols, rows = (~src.transform) * (points[:,0],points[:,1])
                x0,y0 = max(0,math.floor(cols.min())),max(0,math.floor(rows.min()))
                x1,y1 = min(src.width,math.ceil(cols.max())),min(src.height,math.ceil(rows.max()))
                # Large geographic AOIs may be densified by Tile-Ripper before
                # clipping. The fixture/reference edges use the same declared
                # <=0.001 degree geometric interpolation, not its rasterizer.
                w = record["window"]
                assert w == dict(x=x0,y=y0,width=x1-x0,height=y1-y0),(w,(x0,y0,x1,y1))
                win = Window(x0,y0,x1-x0,y1-y0)
                data = src.read(1,window=win)
                full_mask = geometry_mask([mapping(clipped)],out_shape=(src.height,src.width),transform=src.transform,invert=True)
                aoi = full_mask[y0:y1,x0:x1]
                if record["aoi_mask_applied"]:
                    data[~aoi] = src.nodata
                else:
                    aoi = np.ones(data.shape,dtype=bool)
                valid = aoi & ((data!=src.nodata) if src.nodata is not None else True)
                assert ds["values"].dtype == np.dtype("uint16")
                np.testing.assert_array_equal(ds["values"].values[0],data)
                np.testing.assert_array_equal(ds["valid"].values[0],valid.astype("uint8"))
                np.testing.assert_array_equal(ds["aoi_mask"].values[0],aoi.astype("uint8"))
                assert record["native_grid"]["crs"] == src.crs.to_string()
                t = src.window_transform(win)
                assert zg.attrs["transform"] == list(t.to_gdal())
                np.testing.assert_array_equal(ds.x.values,t.c+(np.arange(data.shape[1])+0.5)*t.a)
                np.testing.assert_array_equal(ds.y.values,t.f+(np.arange(data.shape[0])+0.5)*t.e)
                attrs = ds.spatial_ref.attrs
                assert attrs["crs"] == src.crs.to_string()
                if attrs.get("grid_mapping_name") == "transverse_mercator":
                    assert CRS.from_cf(attrs).to_epsg() == src.crs.to_epsg()
                scale = zg.attrs["source_scale"]
                offset = zg.attrs["source_offset"]
                assert (1 if scale is None else scale) == src.scales[0]
                assert (0 if offset is None else offset) == src.offsets[0]
                if scale is None: scale = (source.get("raster_bands") or [{}])[0].get("scale")
                if offset is None: offset = (source.get("raster_bands") or [{}])[0].get("offset")
                assert ds["values"].attrs.get("scale_factor") == scale
                assert ds["values"].attrs.get("add_offset") == offset
                stamp = record["datetime"]
                if stamp:
                    expected_time = np.datetime64(stamp.replace("+00:00","").rstrip("Z"),"ns")
                    decoded = xr.decode_cf(ds,mask_and_scale=False)
                    assert decoded.time.values[0] == expected_time
                    stamps.append(int(expected_time.astype("i8")))
                assert zg.attrs["source"] == source
                assert source.get("object_identity") or source.get("local_identity")
                references.append(dict(group=group,asset_id=record["asset_id"],datetime=stamp,
                    requested_band=source.get("requested_band"),window=w,crs=src.crs.to_string(),
                    transform=list(t.to_gdal()),shape=list(data.shape),dtype=str(data.dtype),
                    crs_attributes=dict(attrs),
                    values_sha256=hashlib.sha256(data.astype("<u2").tobytes()).hexdigest(),
                    aoi_sha256=hashlib.sha256(aoi.astype("uint8").tobytes()).hexdigest(),
                    valid_sha256=hashlib.sha256(valid.astype("uint8").tobytes()).hexdigest(),
                    valid_pixels=int(valid.sum()),samples=int(data.size)))
    assert stamps == sorted(stamps)
    return dict(passed=True,observations=len(references),samples=sum(r["samples"] for r in references),
        references=references,sources=manifest["sources"],observed=manifest["observed"])


class FaultHandler(fixture.Handler):
    def do_HEAD(self):
        s = self.server
        if getattr(s,"late_mutation",False):
            stages = list(s.output.parent.glob(f".{s.output.name}.partial-*"))
            if any(list(p.glob("observations/*/values/[0-9]*")) for p in stages):
                s.mutated_after_writes = True
                s.current_etag = '"late-mutated"'
        super().do_HEAD()

    def do_GET(self):
        if getattr(self.server,"slow_data",False) and self.headers.get("Range"):
            time.sleep(0.025)
        super().do_GET()


def controlled():
    WORK.mkdir(parents=True,exist_ok=True)
    s = fixture.Server(("127.0.0.1",0),FaultHandler)
    s.root = WORK / "fixtures"
    s.files,s.features = fixture.create(s.root,s.server_port)
    s.features[0]["properties"]["datetime"] = "2025-07-29T15:00:00.123456Z"
    s.features[1]["properties"]["datetime"] = "2025-07-28T15:00:00.987654Z"
    s.queries,s.seen_conditions = [],[]
    s.reset()
    threading.Thread(target=s.serve_forever,daemon=True).start()
    checks = []
    try:
        req = unified(fixture.request(s.server_port,"unused",large=True))
        req["time"]["end"] = "2025-07-30T00:00:00Z"
        out = WORK / "mixed.zarr"
        assert not out.exists(),"choose a fresh WORK directory; validation never overwrites datasets"
        req["sink"] = dict(kind="zarr_directory",path=str(out))
        path = WORK / "mixed.json"; save(path,req)
        r = invoke(["fetch",path,"--gdal-free"])
        assert r["code"] == 0,r
        manifest = json.loads(r["stdout"])
        assert "NEVER_SAVE_THIS" not in json.dumps(manifest)
        local = {source["asset_id"]:str(s.root/f'scene{source["item_id"][-1]}-{source["asset_key"]}.tif') for source in manifest["sources"]}
        checked = verify(out,req,local)
        assert checked["observations"] == 4
        assert len({r["crs"] for r in checked["references"]}) == 2
        assert len({tuple(r["shape"]) for r in checked["references"]}) >= 2
        checks.append(dict(name="mixed_CRS_resolution_dates_bands_partial_source_coverage",**checked))
        assert invoke(["fetch",path,"--gdal-free"])["code"] != 0
        checks.append(dict(name="existing_destination_refused",passed=True))
        empty = copy.deepcopy(req); empty["time"]=dict(kind="interval",start="2020-01-01T00:00:00Z",end="2020-01-02T00:00:00Z")
        empty["sink"]["path"] = str(WORK/"empty.zarr")
        save(WORK/"empty.json",empty)
        e = invoke(["fetch",WORK/"empty.json","--gdal-free"])
        assert e["code"] == 0,e
        assert json.loads(e["stdout"])["outputs"] == []
        assert zarr.open_group(empty["sink"]["path"],mode="r").attrs["status"] == "complete"
        checks.append(dict(name="empty_selection_valid_empty_group",passed=True))
        # Corrupt/rejected source and final ETag change must remove all staging.
        for fault in ["fail_data","late_mutation"]:
            s.output = WORK/f"{fault}.zarr"
            fault_request = copy.deepcopy(req)
            fault_request["sink"]["path"] = str(s.output)
            if fault == "late_mutation":
                # A single source guarantees the next HEAD after pixel delivery
                # is final validation, rather than opening another source.
                fault_request["source"] = dict(kind="assets",assets=[dict(id="late",format="cog",
                    locator=f"http://127.0.0.1:{s.server_port}/scene0-red.tif",datetime="2025-07-29T15:00:00.123456Z")])
                fault_request["variables"] = [dict(kind="band",index=1)]
            save(path,fault_request)
            setattr(s,fault,True)
            r = invoke(["fetch",path,"--gdal-free"])
            setattr(s,fault,False)
            s.current_etag = '"fixed-fixture"'
            assert r["code"] != 0,r
            assert not s.output.exists()
            assert not list(WORK.glob(f".{s.output.name}.partial-*"))
            if fault == "late_mutation": assert s.mutated_after_writes
            checks.append(dict(name=fault,passed=True,error=r["stderr"].strip()))
        # Signal after data reaches the staged store. SIGINT cleans; SIGKILL
        # leaves only a hidden uncommitted sibling that a retry ignores.
        s.slow_data = True
        for sig in [signal.SIGINT,signal.SIGKILL]:
            out = WORK/f"signal-{sig}.zarr"
            req["sink"]["path"] = str(out); save(path,req)
            with tempfile.TemporaryFile() as log:
                p = subprocess.Popen([str(BIN),"fetch",str(path),"--gdal-free"],stdout=log,stderr=log)
                deadline = time.monotonic()+30
                stage = None
                while time.monotonic()<deadline and p.poll() is None:
                    candidates = list(WORK.glob(f".{out.name}.partial-*"))
                    if candidates and list(candidates[0].glob("observations/*/values/[0-9]*")):
                        stage = candidates[0]; break
                    time.sleep(0.002)
                assert stage is not None,"signal test must interrupt actual partial pixel output"
                p.send_signal(sig); p.wait(timeout=60)
                assert p.returncode != 0 and not out.exists()
                if sig == signal.SIGINT: assert not stage.exists()
                else:
                    assert stage.exists()
                    assert not (stage/"manifest.json").exists()
                    assert zarr.open_group(str(stage),mode="r").attrs["status"] == "incomplete"
            checks.append(dict(name=f"signal_{sig}_after_pixel_write",passed=True,staging_retained=sig==signal.SIGKILL))
            if sig == signal.SIGKILL:
                r = invoke(["fetch",path,"--gdal-free"])
                assert r["code"]==0,r
                assert stage.exists() and out.exists()
                checks.append(dict(name="retry_ignores_interrupted_sibling",passed=True))
        record = dict(passed=True,checks=checks,binary_sha256=hashlib.sha256(BIN.read_bytes()).hexdigest(),
            python_versions=dict(zarr=zarr.__version__,xarray=xr.__version__,rasterio=rasterio.__version__))
        save(DOCS/"validation.json",record)
        print(json.dumps(dict(passed=True,checks=len(checks),observations=checked["observations"],samples=checked["samples"])))
    finally:
        s.shutdown(); s.server_close()


def live():
    out = WORK/"sentinel.zarr"
    assert not out.exists(),"refusing to overwrite prior live evidence"
    spatial = json.loads((ROOT/"docs/native-sentinel/request.json").read_text())["spatial"]
    save(WORK/"polygon.geojson",dict(type="Polygon",coordinates=spatial["coordinates"]))
    args = ["fetch","--source","https://earth-search.aws.element84.com/v1","--format","stac",
        "--collection","sentinel-2-c1-l2a","--aoi",WORK/"polygon.geojson",
        "--start","2025-01-02T00:00:00Z","--end","2025-01-13T23:59:59Z",
        "--variables","B04,B08,B11","--gdal-free","--buffer-mib","16","--output",out]
    result = invoke(args)
    assert result["code"]==0,result
    manifest = json.loads(result.pop("stdout"))
    req = dict(spatial=spatial)
    checked = verify(out,req)
    assert len({r["datetime"] for r in checked["references"]}) >= 2
    assert {r["requested_band"] for r in checked["references"]} == {"B04","B08","B11"}
    assert len({r["transform"][1] for r in checked["references"]}) == 2
    result["output_bytes"] = sum(p.stat().st_size for p in out.rglob("*") if p.is_file())
    result["output_files"] = sum(p.is_file() for p in out.rglob("*"))
    record = dict(**checked,invocation=[str(a) for a in args],process=result,
        binary_sha256=hashlib.sha256(BIN.read_bytes()).hexdigest(),
        provenance=manifest["inspection"]["query"])
    save(DOCS/"live.json",record)
    print(json.dumps(dict(passed=True,observations=checked["observations"],samples=checked["samples"],process=result)))


if __name__=="__main__":
    action = argparse.ArgumentParser(description=__doc__)
    action.add_argument("action",choices=["checks","live"])
    {"checks":controlled,"live":live}[action.parse_args().action]()
