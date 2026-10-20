"""Linux output and independent reopen against the separate GDAL live oracle.
Run inside python:3.13-slim, with numpy/xarray/zarr/numcodecs only.
"""
import ctypes.util
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import time

import numpy as np
import xarray as xr
import zarr

ROOT=Path("/src")
WORK=ROOT/"prototype-output/durable-output"
assert all(ctypes.util.find_library(n) is None for n in ["gdal","proj","tiff"])
reference=json.loads((ROOT/"docs/durable-output/live-default/live.json").read_text())
request=json.loads((ROOT/"docs/native-sentinel/request.json").read_text())
request["time"]=dict(kind="interval",start="2025-01-02T00:00:00Z",end="2025-01-13T23:59:59Z")
request["variables"]=[dict(kind="named",name=b) for b in ["B04","B08","B11"]]
request["source"]["limits"].update(max_items=32,max_assets=64)
request["budget"]=dict(buffer_bytes=16*2**20,max_nodes=100000,chunk_edge=256)
rows=[]
label=os.environ.get("TILE_RIPPER_LINUX_LABEL","linux")
for mode in ["raw","gzip","blosc_zstd","sharded"]:
    out=WORK/f"{label}-{mode}.zarr"
    assert not out.exists(),"use fresh output directories"
    request["sink"]=dict(kind="zarr_directory",path=str(out))
    path=WORK/f"{label}-{mode}.json";path.write_text(json.dumps(request))
    args=[str(WORK/"linux-tile-ripper"),"fetch",str(path),"--gdal-free"] if mode=="raw" else [str(WORK/"linux-candidate"),str(path),mode,"256"]
    start=time.monotonic()
    p=subprocess.run(args,env={**os.environ,"TILE_RIPPER_WORKERS":"2"},capture_output=True,text=True,check=True)
    manifest=json.loads(p.stdout)
    assert manifest["status"]=="complete"
    fields=["item_id","asset_key","requested_band","datetime","href","object_identity"]
    assert [{k:s[k] for k in fields} for s in manifest["sources"]]==[{k:s[k] for k in fields} for s in reference["sources"]]
    root=zarr.open_group(str(out),mode="r")
    for expected in reference["references"]:
        g=root[expected["group"]]
        with xr.open_zarr(str(out),group=expected["group"],consolidated=False,decode_cf=False,chunks=None) as ds:
            assert ds["values"].dtype==np.dtype("uint16")
            for name,key,dtype in [("values","values_sha256","<u2"),("valid","valid_sha256","u1"),("aoi_mask","aoi_sha256","u1")]:
                assert hashlib.sha256(ds[name].values[0].astype(dtype).tobytes()).hexdigest()==expected[key]
                np.testing.assert_array_equal(g[name][0],ds[name].values[0])
            assert g.attrs["crs"]==expected["crs"] and g.attrs["transform"]==expected["transform"]
            attrs=dict(ds.spatial_ref.attrs)
            # Xarray consumes the v2 _ARRAY_DIMENSIONS convention but leaves
            # that legacy attribute visible beside v3 dimension_names. Check
            # the scalar rank separately, then compare all scientific fields.
            assert attrs.pop("_ARRAY_DIMENSIONS",[])==[] and ds.spatial_ref.dims==()
            assert attrs==expected["crs_attributes"]
            assert ds["values"].dims==("time","y","x")
            actual=xr.decode_cf(ds,mask_and_scale=False).time.values[0]
            assert actual==np.datetime64(expected["datetime"].replace("+00:00","").rstrip("Z"),"ns")
    rows.append(dict(mode=mode,observations=reference["observations"],samples=reference["samples"],
        independent_GDAL_hashes_match=True,wall_seconds=time.monotonic()-start,passed=True))
record=dict(passed=True,runs=rows,platform=platform.platform(),architecture=platform.machine(),
    readers=dict(zarr=zarr.__version__,xarray=xr.__version__),gdal_proj_libtiff_absent=True,
    binary_sha256={name:hashlib.sha256((WORK/name).read_bytes()).hexdigest() for name in ["linux-tile-ripper","linux-candidate"]},
    limitations="Container mounted filesystem; application-level Linux publication checked. No power-loss durability certification of Docker/host storage.")
(ROOT/"docs/durable-output/linux.json").write_text(json.dumps(record,indent=2)+"\n")
print(json.dumps(record,indent=2))
