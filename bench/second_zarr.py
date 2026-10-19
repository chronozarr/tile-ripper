"""POWER IMERG oracle/benchmark, reusing the existing stdlib HTTP baseline.
No new downloader, packages or codecs. Run in the frozen Zarr oracle environment.
"""

import argparse
import hashlib
import itertools
import json
from pathlib import Path
import shutil
import statistics
import sys

import numpy as np
import xarray as xr
import zarr
import cf_zarr as core
import remote_zarr as h

ROOT = h.ROOT
WORK = ROOT / "prototype-output/second-zarr"
core.WORK = WORK


def verify(ref, output):
    ds = xr.open_zarr(
        str(output / "imerg.zarr"), consolidated=False, decode_cf=False, chunks=None
    ).load()
    xr.testing.assert_identical(ref, ds)
    for n in ref.data_vars:
        np.testing.assert_array_equal(ref[n].values, ds[n].values)
        assert ds[n].dtype == np.dtype("float32")
        assert zarr.open_group(str(output / "imerg.zarr"))[n].filters is None
    decoded = xr.open_zarr(str(output / "imerg.zarr"), consolidated=False, chunks=None)
    np.testing.assert_array_equal(
        decoded.time.values, np.array(["2020-06-01"], dtype="datetime64[ns]")
    )
    assert json.loads((output / "manifest.json").read_text())["status"] == "complete"
    return sum(v.size for v in ds.data_vars.values())


def benchmark():
    r = json.loads((ROOT / "examples/imerg.json").read_text())
    url = r["source"]["assets"][0]["locator"]
    store = core.Mapping(
        url, r["budget"]["buffer_bytes"] // 2, r["budget"]["max_nodes"]
    )
    store.names = ["time", "lat", "lon"] + [v["path"] for v in r["variables"]]
    excluded = [
        k.split("/")[0]
        for k in store.metadata
        if k.endswith("/.zarray") and k.split("/")[0] not in store.names
    ]
    ds = xr.open_zarr(
        store, consolidated=True, decode_cf=False, chunks=None, drop_variables=excluded
    )
    subset, idx = core.selection(r, ds)
    ref = subset.transpose("time", "lat", "lon").load()
    group = zarr.open_group(store=store, mode="r")
    keys = []
    for n in ref.data_vars:
        ar = group[n]
        ranges = [
            sorted(set((idx[d] // c).tolist())) for d, c in zip(ds[n].dims, ar.chunks)
        ]
        keys += [
            n + "/" + ar._dimension_separator.join(map(str, v))
            for v in itertools.product(*ranges)
        ]
    assert len(keys) == 2
    # Matching diagnostic serialization template, outside the timed comparison.
    r["sink"]["path"] = str(WORK / "template")
    shutil.rmtree(Path(r["sink"]["path"]), ignore_errors=True)
    req = WORK / "request.json"
    req.write_text(json.dumps(r))
    result = h.process([str(h.CURRENT), "fetch", str(req)])
    assert result["code"] == 0, result
    shutil.copy(
        Path(r["sink"]["path"]) / "manifest.json", WORK / "manifest-template.json"
    )
    verify(ref, Path(r["sink"]["path"]))
    runs = []
    for rep in range(3):
        for reader in ["rust", "xarray"] if rep % 2 == 0 else ["xarray", "rust"]:
            r["sink"]["path"] = str(WORK / f"{reader}-{rep}")
            for i in range(2):
                shutil.rmtree(Path(r["sink"]["path"] + f"-{i}"), ignore_errors=True)
            req.write_text(json.dumps(r))
            cmd = (
                [str(h.EXAMPLE), str(req), "warm"]
                if reader == "rust"
                else [sys.executable, __file__, "worker", "--request", str(req)]
            )
            result = h.process(cmd)
            assert result["code"] == 0, result
            data = result.pop("result")
            for run in data["runs"]:
                out = Path(run["output"])
                assert verify(ref, out) == 30
                files = [p for p in out.rglob("*") if p.is_file()]
                run["output_files"] = len(files)
                run["output_bytes"] = sum(p.stat().st_size for p in files)
                if reader == "rust":
                    obs = run["observed"]
                    assert sorted(v["key"] for v in obs["source_reads"]) == sorted(keys)
                    assert (
                        all(v["requests"] == 1 for v in obs["source_reads"])
                        and obs["decoded_source_chunks"] == 2
                    )
            if reader == "rust":
                planned = sorted(
                    v["operation"]["read"]["key"]
                    for v in data["inspection"]["plan"]["nodes"]
                    if v["operation"]["kind"] == "read"
                )
                assert planned == sorted(keys)
                objects = data["runs"][-1]["observed"]["transport"]["imerg"]["objects"]
                assert all(objects["GET " + k]["attempts"] == 2 for k in keys)
            else:
                assert sorted(data["minimum_keys"]) == sorted(keys)
                assert all(
                    sum(
                        e["key"] == k and not e["head"]
                        for e in data["transport_events"]
                    )
                    == 2
                    for k in keys
                )
            fingerprints = [
                {
                    str(p.relative_to(Path(v["output"]))): hashlib.sha256(
                        p.read_bytes()
                    ).hexdigest()
                    for p in Path(v["output"]).rglob("*")
                    if p.is_file() and p.name != "manifest.json"
                }
                for v in data["runs"]
            ]
            assert fingerprints[0] == fingerprints[1]
            result.update(data, reader=reader, repetition=rep)
            runs.append(result)
            print(
                reader,
                rep,
                round(result["wall_seconds"], 3),
                file=sys.stderr,
                flush=True,
            )
    summary = []
    for reader in ["rust", "xarray"]:
        rows = [v for v in runs if v["reader"] == reader]
        summary.append(
            dict(
                reader=reader,
                cold_median=statistics.median(
                    v["prepare_seconds"] + v["runs"][0]["fetch_seconds"] for v in rows
                ),
                warm_median=statistics.median(
                    v["runs"][1]["fetch_seconds"] for v in rows
                ),
                process_wall_median=statistics.median(v["wall_seconds"] for v in rows),
                peak_rss_bytes=max(v["peak_rss_bytes"] for v in rows),
            )
        )
    (ROOT / "docs/second-zarr-benchmark.json").write_text(
        json.dumps(
            dict(
                request=json.loads((ROOT / "examples/imerg.json").read_text()),
                reference_requests=store.events,
                minimum_keys=keys,
                max_absolute_difference=0.0,
                validated_values_per_extraction=30,
                runs=runs,
                summary=summary,
                binary_sha256=hashlib.sha256(h.CURRENT.read_bytes()).hexdigest(),
                harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                oracle_core_sha256=hashlib.sha256(
                    Path(core.__file__).read_bytes()
                ).hexdigest(),
                transport_harness_sha256=hashlib.sha256(
                    Path(h.__file__).read_bytes()
                ).hexdigest(),
                cargo_lock_sha256=hashlib.sha256(
                    (ROOT / "Cargo.lock").read_bytes()
                ).hexdigest(),
                limitations=[
                    "3 repetitions, one workstation and variable real network/provider caches; no statistical speedup claim",
                    "output stores decoded logical Float32 without reapplying lossy source filter; matched xarray output does the same",
                    "single reference and template acquisition outside timing; bounded historical source selection",
                ],
            ),
            indent=2,
        )
        + "\n"
    )


def checks():
    from numcodecs import Blosc, FixedScaleOffset

    records = []
    for reverse in [False, True]:
        for mode in [
            "normal",
            "missing",
            "empty",
            "irregular",
            "bad-filter",
            "bad-units",
            "503",
        ]:
            path = WORK / "fixture.zarr"
            shutil.rmtree(path, ignore_errors=True)
            g = zarr.open_group(str(path), mode="w")
            x = np.arange(8) * 0.1 - 80.05
            x = x.astype("float32").astype("float64")
            y = np.arange(6) * 0.1 + 35.05
            if reverse:
                x = x[::-1]
                y = y[::-1]
            g.attrs.update(
                dict(
                    title="independent filtered precipitation fixture",
                    geospatial_lon_resolution=0.1,
                    geospatial_lat_resolution=0.1,
                    geospatial_lon_min=-180.0,
                    geospatial_lon_max=180.0,
                    geospatial_lat_min=-90.0,
                    geospatial_lat_max=90.0,
                    geospatial_bounds="source-global-extent",
                    time_coverage_start="1998-01-01",
                    time_coverage_end="2029-12-31",
                    time_coverage_duration="P1D",
                )
            )
            for n, v in [
                ("lon", x),
                ("lat", y),
                ("time", np.array([8186, 8187, 8188], dtype="int64")),
            ]:
                ar = g.create_dataset(
                    n,
                    data=v,
                    chunks=v.shape,
                    compressor=Blosc(cname="lz4", shuffle=1),
                    fill_value=None if n == "time" else np.nan,
                )
                ar.attrs["_ARRAY_DIMENSIONS"] = [n]
                if n == "time":
                    ar.attrs.update(
                        units="days since 1998-01-01 00:00:00",
                        calendar="proleptic_gregorian",
                    )
                else:
                    ar.attrs.update(
                        units="degrees_east" if n == "lon" else "degrees_north",
                        standard_name="Longitude" if n == "lon" else "Latitude",
                        axis="X" if n == "lon" else "Y",
                    )
            for name, scale in [("IMERG_PRECTOT", 100), ("IMERG_PRECLIQUID_PROB", 1)]:
                raw = np.arange(3 * 6 * 8, dtype="float32").reshape(3, 6, 8) / 7
                raw[1, :2, :2] = np.nan
                ar = g.create_dataset(
                    name,
                    data=raw,
                    chunks=(1, 4, 4),
                    fill_value=np.nan,
                    compressor=Blosc(cname="zstd", clevel=3, shuffle=2),
                    filters=[
                        FixedScaleOffset(
                            offset=1000, scale=scale, dtype="<f4", astype="<f4"
                        )
                    ],
                )
                ar.attrs.update(
                    _ARRAY_DIMENSIONS=["time", "lat", "lon"],
                    units="mm/day" if scale == 100 else "%",
                )
            r = json.loads((ROOT / "examples/imerg.json").read_text())
            r["sink"]["path"] = str(WORK / "checked")
            if mode == "missing":
                (path / "IMERG_PRECTOT/1.0.0").unlink()
            if mode == "irregular":
                g["lon"][3] += 0.02
            if mode == "bad-units":
                g["lat"].attrs["units"] = "radians"
            if mode == "bad-filter":
                p = path / "IMERG_PRECTOT/.zarray"
                meta = json.loads(p.read_text())
                meta["filters"][0]["astype"] = "<i2"
                p.write_text(json.dumps(meta))
            if mode == "empty":
                r["spatial"]["bbox"] = [50, 30, 51, 31]
            zarr.consolidate_metadata(str(path))
            server = h.start(path)
            try:
                r["source"]["assets"][0]["locator"] = server.url
                if mode not in ["irregular", "bad-filter", "bad-units"]:
                    ds = xr.open_zarr(
                        str(path), consolidated=False, decode_cf=False, chunks=None
                    )
                    subset, idx = core.selection(r, ds)
                    ref = subset.transpose("time", "lat", "lon").load()
                if mode == "503":
                    server.fault[("GET", "IMERG_PRECTOT/1.0.0")] = [503]
                req = WORK / "check-request.json"
                req.write_text(json.dumps(r))
                out = Path(r["sink"]["path"])
                shutil.rmtree(out, ignore_errors=True)
                result = h.process([str(h.CURRENT), "fetch", str(req)])
                if mode in ["irregular", "bad-filter", "bad-units"]:
                    assert result["code"] != 0 and not out.exists(), result
                elif mode == "empty":
                    assert result["code"] == 0 and not result["result"]["outputs"], (
                        result
                    )
                else:
                    assert result["code"] == 0, result
                    verify(ref, out)
                    keys = core.minimum(r, path)
                    actual = result["result"]["observed"]["source_reads"]
                    assert sorted(v["key"] for v in actual) == keys
                    assert all(v["requests"] == 1 for v in actual)
                    if mode == "503":
                        assert any(v["status"] == 503 for v in server.records)
                    if mode == "missing":
                        assert result["result"]["observed"]["filled_source_chunks"] > 0
                records.append(
                    dict(
                        reverse=reverse,
                        mode=mode,
                        passed=True,
                        code=result["code"],
                        error=result["error"],
                        peak_rss_bytes=result["peak_rss_bytes"],
                    )
                )
            finally:
                server.shutdown()
                server.server_close()
    (ROOT / "docs/second-zarr-checks.json").write_text(
        json.dumps(records, indent=2) + "\n"
    )


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("action", choices=["benchmark", "worker", "checks"])
    ap.add_argument("--request", type=Path)
    args = ap.parse_args()
    WORK.mkdir(parents=True, exist_ok=True)
    if args.action == "worker":
        print(json.dumps(core.baseline(json.loads(args.request.read_text()))))
    elif args.action == "checks":
        checks()
    else:
        benchmark()
