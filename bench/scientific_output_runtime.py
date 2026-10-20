"""Create and reopen real output in python:3.13-slim with no geospatial libraries.

pip install numpy==2.2.6 xarray==2025.6.1 zarr==3.1.3 numcodecs==0.16.3
The ground truth in live.json was separately validated against rasterio/GDAL.
"""
import ctypes.util
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile
import time
import numpy as np
import xarray as xr
import zarr

ROOT = Path("/src")
WORK = ROOT/"prototype-output/scientific-output"
assert all(ctypes.util.find_library(n) is None for n in ["gdal","proj","tiff"])
reference = json.loads((ROOT/"docs/scientific-output/live.json").read_text())
output = Path(os.environ.get("TILE_RIPPER_LINUX_OUTPUT",WORK/"linux-sentinel.zarr"))
args = [str(a) for a in reference["invocation"]]
# The saved command may contain the host absolute worktree path.
args[args.index("--aoi")+1] = str(WORK/"polygon.geojson")
args[args.index("--output")+1] = str(output)
binary = WORK/"linux-tile-ripper"
start = time.monotonic()
with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
    p = subprocess.Popen([str(binary),*args],stdout=stdout,stderr=stderr,env={**os.environ,"TILE_RIPPER_WORKERS":"2"})
    _,status,usage = os.wait4(p.pid,0)
    p.returncode = os.waitstatus_to_exitcode(status)
    stderr.seek(0); assert p.returncode==0,stderr.read().decode()
    stdout.seek(0); manifest = json.load(stdout)
assert manifest["status"] == "complete"
fields = ["item_id","asset_key","requested_band","datetime","href","object_identity"]
assert [{k:s[k] for k in fields} for s in manifest["sources"]] == [{k:s[k] for k in fields} for s in reference["sources"]]
root = zarr.open_group(str(output),mode="r")
for expected in reference["references"]:
    g = root[expected["group"]]
    with xr.open_zarr(str(output),group=expected["group"],consolidated=False,decode_cf=False,chunks=None) as ds:
        assert ds["values"].dtype == np.dtype("uint16")
        for name,key,dtype in [("values","values_sha256","<u2"),("valid","valid_sha256","uint8"),("aoi_mask","aoi_sha256","uint8")]:
            assert hashlib.sha256(ds[name].values[0].astype(dtype).tobytes()).hexdigest() == expected[key]
        assert g.attrs["crs"] == expected["crs"] and g.attrs["transform"] == expected["transform"]
        assert dict(ds.spatial_ref.attrs) == expected["crs_attributes"]
        actual = xr.decode_cf(ds,mask_and_scale=False).time.values[0]
        assert actual == np.datetime64(expected["datetime"].replace("+00:00","").rstrip("Z"),"ns")
libraries = subprocess.check_output(["ldd",str(binary)],text=True)
assert not any(s in libraries.lower() for s in ["libgdal","libproj","libtiff"])
record = dict(passed=True,platform=platform.platform(),architecture=platform.machine(),
    gdal_proj_libtiff_absent=True,clean_source_install=True,ordinary_cli_created_real_dataset=True,
    independently_reopened_by=["zarr "+zarr.__version__,"xarray "+xr.__version__],
    observations=len(reference["references"]),samples=reference["samples"],
    matches_independent_GDAL_reference=True,wall_seconds=time.monotonic()-start,
    peak_rss_bytes=usage.ru_maxrss*1024,cpu_seconds=usage.ru_utime+usage.ru_stime,
    output_bytes=sum(p.stat().st_size for p in output.rglob("*") if p.is_file()),
    binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),dynamic_libraries=libraries)
(ROOT/"docs/scientific-output/deployment.json").write_text(json.dumps(record,indent=2)+"\n")
print(json.dumps(record,indent=2))
