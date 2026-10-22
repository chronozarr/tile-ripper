"""Isolated h5py or NetCDF-C reference; never imports Tile-Ripper tooling.

Input contains only raw source, selection and documented policy. Every value,
source mask, derived mask and coordinate is written for byte-wise comparison.
"""

import hashlib
import json
import resource
import sys
import time
from pathlib import Path

import numpy as np

PATHS = ["wse", "wse_qual", "wse_qual_bitwise", "wse_uncert", "n_wse_pix"]
DTYPES = ["<f4", "u1", "<u4", "<f4", "<u4"]


def selection(spatial):
    return tuple(
        slice(a, a + (n - 1) * s + 1, s)
        for a, n, s in zip(spatial["start"], spatial["count"], spatial["step"])
    )


def calculate(arrays, masks, attrs, policy):
    valid = masks[0].copy()
    if policy["kind"] != "raw":
        valid &= np.isfinite(arrays[0])
    if policy["kind"] == "summary":
        categories = {"good": 0, "suspect": 1, "degraded": 2, "bad": 3}
        valid &= masks[1] & np.isin(
            arrays[1], [categories[n] for n in policy["accepted"]]
        )
    if policy["kind"] == "bitwise":
        names = attrs[2]["flag_meanings"]
        if isinstance(names, bytes):
            names = names.decode()
        definitions = dict(zip(names.split(), np.asarray(attrs[2]["flag_masks"]).flat))
        required = sum(int(definitions[n]) for n in policy["require"])
        excluded = sum(int(definitions[n]) for n in policy["exclude"])
        known = sum(int(n) for n in definitions.values())
        valid &= (
            masks[2]
            & (arrays[2] & np.uint32(~known & 0xFFFFFFFF) == 0)
            & (arrays[2] & required == required)
            & (arrays[2] & excluded == 0)
        )
    um = valid & masks[3] & np.isfinite(arrays[3])
    nm = valid & masks[4]
    wse = arrays[0][valid].astype("f8")
    u = arrays[3][um].astype("f8")
    return valid.astype("u1"), {
        "selected_count": int(valid.sum()),
        "wse_sum_m": float(wse.sum()),
        "wse_mean_m": float(wse.mean()) if len(wse) else None,
        "uncertainty_count": int(um.sum()),
        "uncertainty_sum_m": float(u.sum()),
        "mean_pixel_uncertainty_m": float(u.mean()) if len(u) else None,
        "contributing_count": int(nm.sum()),
        "contributing_pixel_sum": int(arrays[4][nm].astype("u8").sum()),
    }


def run(req):
    if req["reader"] == "netcdf4":
        import netCDF4

        versions = {
            "reader": netCDF4.__version__,
            "netcdf_c": netCDF4.getlibversion(),
            "hdf5": netCDF4.__hdf5libversion__,
        }
    else:
        import h5py

        versions = {"reader": h5py.__version__, "hdf5": h5py.version.hdf5_version}
    started, cpu = time.perf_counter(), time.process_time()
    spatial = req["spatial"]
    sl = selection(spatial)
    remote = None
    if req["reader"] == "netcdf4":
        f = netCDF4.Dataset(req["local"])
        f.set_auto_maskandscale(False)
        attrs = [{k: f[n].getncattr(k) for k in f[n].ncattrs()} for n in PATHS]
    else:
        if "url" in req:
            from hdf5_reference import Remote

            remote = Remote(req["url"], req.get("page", 0))
        f = h5py.File(req.get("local", remote), "r", rdcc_nbytes=2 * 1024 * 1024)
        attrs = [dict(f[n].attrs) for n in PATHS]
    prep = time.perf_counter() - started
    arrays = [np.asarray(f[n][sl], dtype=d) for n, d in zip(PATHS, DTYPES)]
    masks = []
    for a, at in zip(arrays, attrs):
        m = ~np.isnan(a)
        for key in ["_FillValue", "missing_value"]:
            if key in at:
                m &= a != np.asarray(at[key]).item()
        masks.append(m)
    axes = [np.asarray(f[n][s], dtype="<f8") for n, s in zip(["y", "x"], sl)]
    f.close()
    if remote:
        response = remote.session.head(
            req["url"], headers={"If-Match": remote.etag}, timeout=30
        )
        assert (
            response.status_code == 200
            and response.headers["ETag"] == remote.etag
            and int(response.headers["Content-Length"]) == remote.size
        )
        remote.close()
    valid, summary = calculate(arrays, masks, attrs, req["policy"])
    if "dump" in req:
        out = Path(req["dump"])
        out.mkdir(parents=True, exist_ok=True)
        for i, (a, m) in enumerate(zip(arrays, masks)):
            (out / f"{i}.bin").write_bytes(a.tobytes())
            (out / f"{i}.mask").write_bytes(m.astype("u1").tobytes())
        (out / "validity.bin").write_bytes(valid.tobytes())
    out = {
        "reader": req["reader"],
        "versions": versions,
        "access": "conditional_loopback" if remote else "local_sparse_reference",
        "prepare_seconds": prep,
        "completion_seconds": time.perf_counter() - started,
        "api_cpu_seconds": time.process_time() - cpu,
        "peak_rss_bytes": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        * (1 if sys.platform == "darwin" else 1024),
        "value_sha256": [hashlib.sha256(a.tobytes()).hexdigest() for a in arrays],
        "axis_sha256": [hashlib.sha256(a.tobytes()).hexdigest() for a in axes],
        "summary": summary,
    }
    if remote:
        out.update(
            events=remote.events,
            gets=len(remote.events),
            heads=2,
            response_bytes=sum(b - a for a, b in remote.events),
        )
    print(json.dumps(out, allow_nan=False))


if __name__ == "__main__":
    run(json.loads(Path(sys.argv[1]).read_text()))
