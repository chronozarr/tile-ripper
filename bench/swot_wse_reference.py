"""Isolated mature SWOT reference; imports only the requested reader.

No native acquisition modules are imported. NetCDF-C uses a local sparse file;
h5py uses either the same local file or explicit version-checked mirror ranges.
"""

import hashlib
import json
import resource
import sys
import time
from pathlib import Path

import numpy as np


def slices(spatial):
    return tuple(
        slice(a, a + (n - 1) * s + 1, s)
        for a, n, s in zip(spatial["start"], spatial["count"], spatial["step"])
    )


def summary(values, fill):
    valid = np.isfinite(values) & (values != fill)
    v = values[valid].astype("f8")
    return {
        "requested_pixels": int(values.size),
        "finite_nonfill_pixels": int(valid.sum()),
        "sum_m": float(v.sum()),
        "mean_m": float(v.mean()) if len(v) else None,
        "min_m": float(v.min()) if len(v) else None,
        "max_m": float(v.max()) if len(v) else None,
    }


def reference(req):
    if req["reader"] == "netcdf4":
        import netCDF4

        versions = {
            "reader_version": netCDF4.__version__,
            "netcdf_c": netCDF4.getlibversion(),
            "hdf5": netCDF4.__hdf5libversion__,
        }
    else:
        import h5py
        from hdf5_reference import Remote

        versions = {
            "reader_version": h5py.__version__,
            "hdf5": h5py.version.hdf5_version,
        }
    start, cpu = time.perf_counter(), time.process_time()
    selected = slices(req["spatial"])
    remote = None
    if req["reader"] == "netcdf4":
        with netCDF4.Dataset(req["local"]) as f:
            f.set_auto_maskandscale(False)
            _ = {k: f.getncattr(k) for k in f.ncattrs()}
            ds = f["wse"]
            _ = {k: ds.getncattr(k) for k in ds.ncattrs()}
            _ = {k: f["crs"].getncattr(k) for k in f["crs"].ncattrs()}
            y, x = f["y"][selected[0]], f["x"][selected[1]]
            prepared = time.perf_counter() - start
            data = np.asarray(ds[selected], dtype="<f4")
            fill = ds.getncattr("_FillValue")
    else:
        remote = None if "local" in req else Remote(req["url"], req.get("page", 0))
        with h5py.File(req.get("local", remote), "r", rdcc_nbytes=2 * 1024 * 1024) as f:
            _ = dict(f.attrs)
            _ = dict(f["crs"].attrs)
            ds = f["wse"]
            _ = dict(ds.attrs)
            for name in ["x", "y"]:
                _ = dict(f[name].attrs)
            y, x = f["y"][selected[0]], f["x"][selected[1]]
            prepared = time.perf_counter() - start
            data = ds[selected].astype("<f4")
            fill = ds.fillvalue
    first = time.perf_counter() - start
    out = {
        "reader": req["reader"],
        "access": "local_sparse_file"
        if "local" in req
        else "loopback_conditional_ranges",
        "prepare_seconds": prepared,
        "first_result_seconds": first,
        "value_sha256": hashlib.sha256(data.tobytes()).hexdigest(),
        "axis_sha256": [
            hashlib.sha256(a.astype("<f8").tobytes()).hexdigest() for a in [y, x]
        ],
        "summary": summary(data, fill),
        "page_bytes": req.get("page", 0),
    }
    if remote:
        check = remote.session.head(
            req["url"], headers={"If-Match": remote.etag}, timeout=30
        )
        assert check.status_code == 200 and check.headers["ETag"] == remote.etag
        assert int(check.headers["Content-Length"]) == remote.size
        out.update(
            events=remote.events,
            heads=2,
            gets=len(remote.events),
            response_bytes=sum(b - a for a, b in remote.events),
        )
        remote.close()
    out.update(
        completion_seconds=time.perf_counter() - start,
        api_cpu_seconds=time.process_time() - cpu,
        peak_rss_bytes=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        * (1 if sys.platform == "darwin" else 1024),
        **versions,
    )
    print(json.dumps(out))


if __name__ == "__main__":
    reference(json.loads(Path(sys.argv[1]).read_text()))
