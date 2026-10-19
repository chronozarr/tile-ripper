"""Independent CF geographic oracle and bounded public HTTP comparison.
Uses the existing frozen xarray environment; no new downloader or codec.
"""

import argparse
import copy
import hashlib
import itertools
import json
import os
from pathlib import Path
import shutil
import statistics
import sys
import time

import numpy as np
import xarray as xr
import zarr
from numcodecs import Blosc, GZip
import remote_zarr as h

ROOT = h.ROOT
WORK = ROOT / "prototype-output/cf-zarr"


def request(source, output):
    r = json.loads((ROOT / "examples/weatherbench2.json").read_text())
    r["source"]["assets"][0]["locator"] = str(source)
    r["sink"]["path"] = str(output)
    return r


def selection(r, ds):
    lo, la, hi, ha = r["spatial"]["bbox"]
    # Match native cell-intersection semantics independently from affine math.
    idx = {}
    for axis, lower, upper in [("longitude", lo, hi), ("latitude", la, ha)]:
        v = ds[axis].values
        half = abs(v[1] - v[0]) / 2
        idx[axis] = np.flatnonzero((v + half > lower) & (v - half < upper))
    t = ds.time.values
    attrs = ds.time.attrs
    if attrs["units"] == "hours since 1959-01-01":
        seconds = t * 3600 - 347155200
    else:
        assert attrs["units"] == "seconds since 1970-01-01 00:00:00"
        seconds = t
    start = np.datetime64(r["time"]["start"].replace("Z", ""), "s").astype("int64")
    end = np.datetime64(r["time"]["end"].replace("Z", ""), "s").astype("int64")
    idx["time"] = np.flatnonzero((seconds >= start) & (seconds <= end))
    return ds[[v["path"] for v in r["variables"]]].isel(idx), idx


def minimum(r, source):
    ds = xr.open_zarr(str(source), consolidated=False, decode_cf=False, chunks=None)
    subset, idx = selection(r, ds)
    group = zarr.open_group(str(source), mode="r")
    keys = []
    for name in subset.data_vars:
        ar = group[name]
        ranges = [
            sorted(set((idx[d] // c).tolist()))
            for d, c in zip(ds[name].dims, ar.chunks)
        ]
        keys += [
            name + "/" + ar._dimension_separator.join(map(str, index))
            for index in itertools.product(*ranges)
        ]
    return sorted(keys)


def check(r, source, output):
    ds = xr.open_zarr(source, consolidated=False, decode_cf=False, chunks=None)
    ref, _ = selection(r, ds)
    ref = ref.transpose("time", "latitude", "longitude").load()
    got = xr.open_zarr(
        str(output / "era5.zarr"), consolidated=False, decode_cf=False, chunks=None
    ).load()
    xr.testing.assert_identical(ref, got)
    for var in ref.data_vars:
        assert got[var].dtype == np.dtype("float32")
        np.testing.assert_array_equal(ref[var].values, got[var].values)
        assert np.array_equal(np.isnan(ref[var]), np.isnan(got[var]))
    decoded = xr.open_zarr(str(output / "era5.zarr"), consolidated=False, chunks=None)
    assert decoded.time.size == ref.time.size
    manifest = json.loads((output / "manifest.json").read_text())
    assert manifest["status"] == "complete"
    return sum(ref[v].size for v in ref.data_vars)


def fixture(path, reverse_x=False, reverse_y=False, swap=True, codec="blosc"):
    shutil.rmtree(path, ignore_errors=True)
    x = np.array([0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0])
    y = np.array([20.0, 30.0, 40.0, 50.0, 60.0, 70.0])
    if reverse_x:
        x = x[::-1]
    if reverse_y:
        y = y[::-1]
    time_values = np.array([0, 6, 12, 24], dtype="int64")
    values = (
        np.arange(4 * len(y) * len(x), dtype="float32").reshape(4, len(y), len(x)) / 7
    )
    values[:, 1:3, 2:4] = np.nan
    dims = (
        ["time", "longitude", "latitude"] if swap else ["time", "latitude", "longitude"]
    )
    if swap:
        values = values.transpose(0, 2, 1)
    compressor = (
        Blosc(cname="lz4", clevel=5, shuffle=1)
        if codec == "blosc"
        else GZip(1)
        if codec == "gzip"
        else None
    )
    g = zarr.open_group(str(path), mode="w")
    g.attrs.update({"title": "independent geographic fixture", "institution": "test"})
    for n, v in [
        ("longitude", x),
        ("latitude", y),
        ("time", time_values),
        ("2m_temperature", values),
    ]:
        ar = g.create_dataset(
            n,
            data=v,
            chunks=(2, 3, 4) if v.ndim == 3 else v.shape,
            compressor=compressor,
            fill_value=np.nan if v.dtype.kind == "f" else None,
        )
        ar.attrs["_ARRAY_DIMENSIONS"] = dims if v.ndim == 3 else [n]
        if n == "time":
            ar.attrs.update(
                {"units": "hours since 1959-01-01", "calendar": "proleptic_gregorian"}
            )
        elif n == "2m_temperature":
            ar.attrs.update({"units": "K", "long_name": "test temperature"})
    return g


class Mapping(h.HttpMapping):
    def __init__(self, url, cap, nodes):
        # Parent uses stdlib HTTP; choose its TLS counterpart without changing reads.
        self.counts = {"requests": 0, "bytes": 0, "objects": []}
        super().__init__(url, cap, nodes)

    def __getitem__(self, key):
        if key == ".zmetadata" and self.metadata is not None:
            return json.dumps(
                {"zarr_consolidated_format": 1, "metadata": self.metadata}
            ).encode()
        return super().__getitem__(key)

    def fetch(self, key, head=False, data_head=False):
        t = time.perf_counter()
        value = super().fetch(key, head, data_head)
        self.counts["requests"] += 1
        size = len(value) if isinstance(value, bytes) else 0
        self.counts["bytes"] += size
        self.counts["objects"].append(
            dict(key=key, head=head, bytes=size, seconds=time.perf_counter() - t)
        )
        return value


def baseline(r):
    started = time.perf_counter()
    store = Mapping(
        r["source"]["assets"][0]["locator"],
        r["budget"]["buffer_bytes"] // 2,
        r["budget"]["max_nodes"],
    )
    store.names = ["time", "latitude", "longitude"] + [
        v["path"] for v in r["variables"]
    ]
    excluded = (
        [
            k.split("/")[0]
            for k in store.metadata
            if k.endswith("/.zarray") and k.split("/")[0] not in store.names
        ]
        if store.metadata is not None
        else None
    )
    ds = xr.open_zarr(
        store,
        consolidated=store.metadata is not None,
        decode_cf=False,
        chunks=None,
        drop_variables=excluded,
    )
    sub, idx = selection(r, ds)
    group = zarr.open_group(store=store, mode="r")
    keys = []
    for v in sub.data_vars:
        ar = group[v]
        indices = [
            sorted(set((idx[d] // c).tolist())) for d, c in zip(ds[v].dims, ar.chunks)
        ]
        keys += [
            v + "/" + ar._dimension_separator.join(map(str, i))
            for i in itertools.product(*indices)
        ]
    for key in keys:
        store.fetch(key, head=True, data_head=True)
    store.seal()
    planning = time.perf_counter() - started
    rows = []
    for rep in range(2):
        output = Path(r["sink"]["path"] + f"-{rep}")
        output.mkdir()
        t = time.perf_counter()
        store.validate()
        read = time.perf_counter()
        # Reload from the lazy selection for each extraction: no data-body cache.
        data = sub.copy(deep=False).transpose("time", "latitude", "longitude").load()
        read = time.perf_counter() - read
        edge = r["budget"]["chunk_edge"]

        class Durable(zarr.DirectoryStore):
            def __setitem__(self, key, value):
                super().__setitem__(key, value)
                with open(Path(self.path) / key, "rb") as f:
                    if sys.platform == "darwin":
                        __import__("fcntl").fcntl(f.fileno(), 51)
                    else:
                        os.fsync(f.fileno())

        enc = {
            n: {
                "chunks": (data[n].size,),
                "compressor": group[n].compressor,
            }
            for n in data.coords
        }
        enc.update(
            {
                n: {
                    "chunks": (
                        1,
                        min(edge, data.sizes["latitude"]),
                        min(edge, data.sizes["longitude"]),
                    ),
                    "compressor": group[n].compressor,
                }
                for n in data.data_vars
            }
        )
        write = time.perf_counter()
        data.to_zarr(
            Durable(str(output / "era5.zarr")),
            mode="w",
            consolidated=False,
            encoding=enc,
            write_empty_chunks=False,
        )
        write = time.perf_counter() - write
        store.validate()
        manifest = json.loads((WORK / "manifest-template.json").read_text())
        manifest["observed"] = {
            "reader": "xarray",
            "transport": copy.deepcopy(store.counts),
            "read_decode_seconds": read,
            "writing_seconds": write,
        }
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2))
        rows.append(
            dict(
                output=str(output),
                fetch_seconds=time.perf_counter() - t,
                read_decode_seconds=read,
                writing_seconds=write,
                transport=copy.deepcopy(store.counts),
            )
        )
    return dict(
        prepare_seconds=planning,
        minimum_keys=keys,
        runs=rows,
        transport_events=store.events,
    )


def checks():
    results = []
    for rx, ry, swap, codec in itertools.product(
        [False, True], [False, True], [False, True], ["blosc", "gzip", None]
    ):
        source = WORK / "fixture.zarr"
        fixture(source, rx, ry, swap, codec)
        r = request(source, WORK / "checked")
        req = WORK / "request.json"
        req.write_text(json.dumps(r))
        shutil.rmtree(Path(r["sink"]["path"]), ignore_errors=True)
        result = h.process([str(h.CURRENT), "fetch", str(req)])
        assert result["code"] == 0, result
        count = check(r, str(source), Path(r["sink"]["path"]))
        obs = result["result"]["observed"]
        keys = minimum(r, source)
        planned = sorted(
            n["operation"]["read"]["key"]
            for n in result["result"]["plan"]["nodes"]
            if n["operation"]["kind"] == "read"
        )
        assert keys == planned == sorted(v["key"] for v in obs["source_reads"])
        assert obs["decoded_source_chunks"] == len(keys)
        assert all(v["requests"] == 1 for v in obs["source_reads"])
        results.append(
            dict(
                reverse_x=rx,
                reverse_y=ry,
                swap=swap,
                codec=codec,
                values=count,
                passed=True,
            )
        )
    # Independent error cases reject before a success marker; gzip transport regressions elsewhere.
    errors = {}
    for mode in [
        "dtype",
        "dimensions",
        "units",
        "irregular",
        "calendar",
        "shape-overflow",
        "chunk-bomb",
        "truncated",
        "blosc-bomb",
        "empty-space",
        "empty-time",
        "http-503",
        "missing",
        "missing-coordinate",
        "codec",
        "codec-header",
        "time-overflow",
    ]:
        source = WORK / "fixture.zarr"
        fixture(source)
        r = request(source, WORK / "invalid")
        if mode in [
            "dtype",
            "dimensions",
            "units",
            "calendar",
            "shape-overflow",
            "chunk-bomb",
        ]:
            key = {
                "dtype": "2m_temperature/.zarray",
                "dimensions": "2m_temperature/.zattrs",
                "units": "latitude/.zattrs",
                "calendar": "time/.zattrs",
                "shape-overflow": "2m_temperature/.zarray",
                "chunk-bomb": "2m_temperature/.zarray",
            }[mode]
            p = source / key
            m = json.loads(p.read_text())
            if mode == "dtype":
                m["dtype"] = ">f4"
            elif mode == "dimensions":
                m["_ARRAY_DIMENSIONS"] = ["time", "level", "latitude"]
            elif mode == "units":
                m["units"] = "radians"
            elif mode == "calendar":
                m["calendar"] = "360_day"
            elif mode == "shape-overflow":
                m["shape"] = [2**63, 2**63, 2**63]
            else:
                m["chunks"] = [2**30, 2**30, 2**30]
            p.write_text(json.dumps(m))
        if mode == "missing":
            (source / "2m_temperature/0.0.0").unlink()
        if mode == "missing-coordinate":
            (source / "latitude/0").unlink()
        if mode == "codec":
            p = source / "2m_temperature/.zarray"
            m = json.loads(p.read_text())
            m["compressor"]["cname"] = "zstd"
            p.write_text(json.dumps(m))
        if mode == "codec-header":
            # Use a block large enough for genuine compression rather than a
            # codec-independent Blosc memcpy block.
            p = source / "2m_temperature/.zarray"
            m = json.loads(p.read_text())
            m["chunks"] = [2, 8, 6]
            p.write_text(json.dumps(m))
            encoded = Blosc(cname="zstd", clevel=5, shuffle=1).encode(
                np.zeros((2, 8, 6), dtype="float32")
            )
            assert not encoded[2] & 2 and encoded[2] >> 5 == 4
            (source / "2m_temperature/0.0.0").write_bytes(encoded)
        if mode == "time-overflow":
            zarr.open_group(str(source), mode="a")["time"][:] = [0, 6, 12, 2**62]
        if mode == "irregular":
            zarr.open_group(str(source), mode="a")["latitude"][:] = [
                20,
                30,
                41,
                50,
                60,
                70,
            ]
        if mode in ["truncated", "blosc-bomb"]:
            p = source / "2m_temperature/0.0.0"
            data = bytearray(p.read_bytes())
            if mode == "truncated":
                data = data[:10]
            else:
                data[4:8] = (2**30).to_bytes(4, "little")
            p.write_bytes(data)
        if mode == "empty-space":
            r["spatial"]["bbox"] = [80, 20, 90, 40]
        if mode == "empty-time":
            r["time"]["start"] = "2025-01-01T00:00:00Z"
            r["time"]["end"] = "2025-01-01T06:00:00Z"
        server = h.start(source)
        try:
            if mode == "http-503":
                server.fault[("GET", "2m_temperature/0.0.0")] = [503]
            local = copy.deepcopy(r)
            r["source"]["assets"][0]["locator"] = server.url
            req = WORK / "request.json"
            req.write_text(json.dumps(r))
            out = Path(r["sink"]["path"])
            shutil.rmtree(out, ignore_errors=True)
            result = h.process([str(h.CURRENT), "fetch", str(req)])
            if mode.startswith("empty"):
                assert (
                    result["code"] == 0 and not result["result"]["plan"]["selected"]
                ), result
            elif mode in ["http-503", "missing"]:
                assert result["code"] == 0, result
                check(local, str(source), out)
                if mode == "http-503":
                    assert any(v["status"] == 503 for v in server.records)
                else:
                    assert result["result"]["observed"]["filled_source_chunks"] > 0
            else:
                assert result["code"] != 0 and not out.exists(), result
            errors[mode] = {
                "passed": True,
                "error": result["error"],
                "peak_rss_bytes": result["peak_rss_bytes"],
            }
        finally:
            server.shutdown()
            server.server_close()
    (ROOT / "docs/cf-zarr-checks.json").write_text(
        json.dumps(dict(orientations=results, errors=errors), indent=2) + "\n"
    )


def benchmark():
    r = json.loads((ROOT / "examples/weatherbench2.json").read_text())
    # One independent reference fetch, outside timed runs, with bounded body cap.
    store = Mapping(
        r["source"]["assets"][0]["locator"],
        r["budget"]["buffer_bytes"] // 2,
        r["budget"]["max_nodes"],
    )
    store.names = ["time", "latitude", "longitude", "2m_temperature"]
    source = xr.open_zarr(store, consolidated=False, decode_cf=False, chunks=None)
    ref, idx = selection(r, source)
    ref = ref.transpose("time", "latitude", "longitude").load()
    refpath = WORK / "reference.zarr"
    shutil.rmtree(refpath, ignore_errors=True)
    ref.to_zarr(str(refpath), consolidated=False)
    reference_events = list(store.events)
    rows = []
    max_difference = 0.0
    # Diagnostic template outside timed runs, using the same sink schema.
    r["sink"]["path"] = str(WORK / "template")
    shutil.rmtree(Path(r["sink"]["path"]), ignore_errors=True)
    req = WORK / "request.json"
    req.write_text(json.dumps(r))
    result = h.process([str(h.CURRENT), "fetch", str(req)])
    assert result["code"] == 0, result
    shutil.copy(
        Path(r["sink"]["path"]) / "manifest.json", WORK / "manifest-template.json"
    )
    for rep in range(3):
        for reader in ["rust", "xarray"] if rep % 2 == 0 else ["xarray", "rust"]:
            r["sink"]["path"] = str(WORK / f"public-{reader}-{rep}")
            for i in range(2):
                shutil.rmtree(Path(r["sink"]["path"] + f"-{i}"), ignore_errors=True)
            req = WORK / "request.json"
            req.write_text(json.dumps(r))
            cmd = (
                [str(h.EXAMPLE), str(req), "warm"]
                if reader == "rust"
                else [sys.executable, __file__, "baseline", "--request", str(req)]
            )
            result = h.process(cmd)
            assert result["code"] == 0, result
            data = result.pop("result")
            for run in data["runs"]:
                out = Path(run["output"])
                got = xr.open_zarr(
                    str(out / "era5.zarr"),
                    consolidated=False,
                    decode_cf=False,
                    chunks=None,
                ).load()
                xr.testing.assert_identical(ref, got)
                np.testing.assert_array_equal(
                    ref["2m_temperature"].values, got["2m_temperature"].values
                )
                difference = np.abs(
                    ref["2m_temperature"].values - got["2m_temperature"].values
                )
                max_difference = max(
                    max_difference, float(np.nanmax(difference, initial=0))
                )
                decoded = xr.open_zarr(
                    str(out / "era5.zarr"), consolidated=False, chunks=None
                )
                np.testing.assert_array_equal(
                    decoded.time.values,
                    np.array(
                        ["1959-01-01T00:00:00", "1959-01-01T06:00:00"],
                        dtype="datetime64[ns]",
                    ),
                )
                assert got["2m_temperature"].attrs["units"] == "K"
                assert (
                    zarr.open_group(str(out / "era5.zarr"))["2m_temperature"].fill_value
                    is not None
                )
                manifest = json.loads((out / "manifest.json").read_text())
                assert manifest["status"] == "complete"
                files = [p for p in out.rglob("*") if p.is_file()]
                run["output_files"] = len(files)
                run["output_bytes"] = sum(p.stat().st_size for p in files)
                if reader == "rust":
                    obs = run["observed"]
                    assert (
                        obs["decoded_source_chunks"] == 1
                        and obs["source_data_requests"] == 1
                    )
                    assert (
                        obs["source_reads"][0]["key"] == "2m_temperature/0.0.0"
                        and obs["source_reads"][0]["requests"] == 1
                    )
                    assert obs["native_materialized_bytes"] == 819200
            native_fingerprints = [
                {
                    str(p.relative_to(Path(run["output"]))): hashlib.sha256(
                        p.read_bytes()
                    ).hexdigest()
                    for p in Path(run["output"]).rglob("*")
                    if p.is_file() and p.name != "manifest.json"
                }
                for run in data["runs"]
            ]
            assert native_fingerprints[0] == native_fingerprints[1]
            if reader == "rust":
                assert [
                    n["operation"]["read"]["key"]
                    for n in data["inspection"]["plan"]["nodes"]
                    if n["operation"]["kind"] == "read"
                ] == ["2m_temperature/0.0.0"]
            if reader == "xarray":
                assert data["minimum_keys"] == ["2m_temperature/0.0.0"]
                assert (
                    sum(
                        e["key"] == "2m_temperature/0.0.0" and not e["head"]
                        for e in data["transport_events"]
                    )
                    == 2
                )
            else:
                objects = data["runs"][1]["observed"]["transport"]["era5"]["objects"]
                assert objects["GET 2m_temperature/0.0.0"]["attempts"] == 2
            result.update(data, reader=reader, repetition=rep)
            rows.append(result)
            print(
                reader,
                rep,
                round(result["wall_seconds"], 3),
                file=sys.stderr,
                flush=True,
            )
    summaries = []
    for reader in ["rust", "xarray"]:
        group = [v for v in rows if v["reader"] == reader]
        summaries.append(
            dict(
                reader=reader,
                cold_median=statistics.median(
                    v["prepare_seconds"] + v["runs"][0]["fetch_seconds"] for v in group
                ),
                warm_median=statistics.median(
                    v["runs"][1]["fetch_seconds"] for v in group
                ),
                process_wall_median=statistics.median(v["wall_seconds"] for v in group),
                peak_rss_bytes=max(v["peak_rss_bytes"] for v in group),
            )
        )
    evidence = dict(
        dataset=r["source"],
        request=json.loads((ROOT / "examples/weatherbench2.json").read_text()),
        reference_requests=reference_events,
        validated_values_per_extraction=int(ref["2m_temperature"].size),
        max_abs_difference=max_difference,
        minimum_keys=["2m_temperature/0.0.0"],
        repetitions=3,
        runs=rows,
        summary=summaries,
        binary_sha256=hashlib.sha256(h.CURRENT.read_bytes()).hexdigest(),
        library_example_sha256=hashlib.sha256(h.EXAMPLE.read_bytes()).hexdigest(),
        harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        limitations=[
            "real HTTPS variable provider/network/OS caches; warm means prepared metadata, not cached data",
            "three repetitions one workstation; no significance/speedup claim",
            "xarray materializes selected data; Rust streams bounded native buffers",
            "baseline uses same diagnostic manifest template; scientific selection/codecs are independent",
        ],
    )
    (ROOT / "docs/cf-zarr-benchmark.json").write_text(
        json.dumps(evidence, indent=2) + "\n"
    )


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("action", choices=["checks", "baseline", "benchmark"])
    ap.add_argument("--request", type=Path)
    args = ap.parse_args()
    WORK.mkdir(parents=True, exist_ok=True)
    if args.action == "checks":
        checks()
    elif args.action == "baseline":
        print(json.dumps(baseline(json.loads(args.request.read_text()))))
    else:
        benchmark()
