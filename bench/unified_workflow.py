"""Unified lifecycle, independent read selection and before/after reader benchmarks.

Use the existing GDAL Python environment for stac; the isolated Zarr environment
for zarr. Only loopback HTTP and deterministic local data are used.
"""

import argparse
import copy
import hashlib
import itertools
import json
import os
from pathlib import Path
import platform
import random
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/unified-benchmark"
CURRENT = ROOT / "target/release/tile-ripper"
PREVIOUS = ROOT / "prototype-output/unified/previous-46dd503"


def process(cmd):
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        start = time.perf_counter()
        p = subprocess.Popen(
            cmd,
            cwd=ROOT,
            stdout=out,
            stderr=err,
            env={**os.environ, "RAYON_NUM_THREADS": "1"},
        )
        _, status, usage = os.wait4(p.pid, 0)
        p.returncode = os.waitstatus_to_exitcode(status)
        out.seek(0)
        err.seek(0)
        if p.returncode:
            raise RuntimeError(err.read().decode()[:3000])
        return {
            "result": json.load(out),
            "wall_seconds": time.perf_counter() - start,
            "peak_rss_bytes": usage.ru_maxrss
            * (1 if sys.platform == "darwin" else 1024),
        }


def environment():
    return {
        "platform": platform.platform(),
        "python": sys.version,
        "source_sha256": hashlib.sha256(
            b"".join(
                p.name.encode() + p.read_bytes()
                for p in sorted((ROOT / "src").glob("*.rs"))
            )
        ).hexdigest(),
        "current_binary_sha256": hashlib.sha256(CURRENT.read_bytes()).hexdigest(),
        "previous_binary_sha256": hashlib.sha256(PREVIOUS.read_bytes()).hexdigest(),
        "previous_commit": "46dd503",
        "cargo_lock_sha256": hashlib.sha256(
            (ROOT / "Cargo.lock").read_bytes()
        ).hexdigest(),
    }


def summarize(rows):
    summaries = []
    for workload, reader in sorted({(r["workload"], r["reader"]) for r in rows}):
        a = [r for r in rows if (r["workload"], r["reader"]) == (workload, reader)]
        vals = [r["wall_seconds"] for r in a]
        summaries.append(
            {
                "workload": workload,
                "reader": reader,
                "n": len(a),
                "wall_median": statistics.median(vals),
                "wall_min": min(vals),
                "wall_max": max(vals),
                "wall_stdev": statistics.stdev(vals) if len(vals) > 1 else 0,
                "rss_max": max(r["peak_rss_bytes"] for r in a),
            }
        )
    return summaries


def minimum_keys(request):
    import numpy as np
    import xarray as xr
    import zarr
    from zarr_workflow import expected

    path = request["source"]["assets"][0]["locator"]
    source = xr.open_zarr(path, consolidated=False, decode_cf=False, chunks=None)
    subset = expected(request, source)
    group = zarr.open_group(path, mode="r")
    keys = set()
    for v in subset.data_vars:
        chunks = group[v].chunks
        bins = []
        for axis, chunk in zip(("time", "y", "x"), chunks):
            selected = np.flatnonzero(np.isin(source[axis].values, subset[axis].values))
            bins.append(sorted(set(int(i // chunk) for i in selected)))
        metadata = json.loads(Path(path, v, ".zarray").read_text())
        sep = metadata.get("dimension_separator", ".")
        keys.update(
            v + "/" + sep.join(str(i) for i in indices)
            for indices in itertools.product(*bins)
        )
    return sorted(keys)


def zarr_baseline(request, out):
    import xarray as xr
    import zarr
    from zarr_workflow import expected

    class Store(zarr.DirectoryStore):
        def __init__(self, path):
            super().__init__(path)
            self.keys = {}
            self.bytes = 0
            self.read_seconds = 0
            self.probes = set()

        def __getitem__(self, key):
            is_data = key.startswith(("red/", "nir/")) and not Path(
                key
            ).name.startswith(".")
            if is_data:
                self.keys[key] = self.keys.get(key, 0) + 1
            t = time.perf_counter()
            try:
                data = super().__getitem__(key)
            finally:
                if is_data:
                    self.read_seconds += time.perf_counter() - t
            if is_data:
                self.bytes += len(data)
            return data

        def __contains__(self, key):
            if key.startswith(("red/", "nir/")) and not Path(key).name.startswith("."):
                self.probes.add(key)
            return super().__contains__(key)

    store = Store(request["source"]["assets"][0]["locator"])
    t = time.perf_counter()
    source = xr.open_zarr(store, consolidated=False, decode_cf=False, chunks=None)
    metadata = time.perf_counter() - t
    t = time.perf_counter()
    subset = expected(request, source)
    planning = time.perf_counter() - t
    store.keys.clear()
    store.probes.clear()
    store.bytes = store.read_seconds = 0
    t = time.perf_counter()
    subset.load()
    read_decode = time.perf_counter() - t
    observed = {
        "data_keys": store.keys,
        "probed_data_keys": sorted(store.probes),
        "data_bytes": store.bytes,
        "data_requests": sum(store.keys.values()),
        "store_read_seconds": store.read_seconds,
    }
    subset.attrs = copy.deepcopy(subset.attrs)
    affine = subset.attrs["tile_ripper_grid"]["transform"]
    affine[0] = float(subset.x.values[0] - 5)
    affine[3] = float(subset.y.values[0] + 5)
    t = time.perf_counter()
    subset.to_zarr(
        Path(out) / "cube.zarr",
        mode="w",
        consolidated=False,
        encoding={n: {"_FillValue": None} for n in subset.coords},
    )
    return {
        **observed,
        "metadata_seconds": metadata,
        "planning_seconds": planning,
        "read_decode_seconds": read_decode,
        "writing_seconds": time.perf_counter() - t,
    }


def medium_fixture(request):
    import numpy as np
    import zarr
    from numcodecs import GZip

    path = WORK / "zarr-medium-source.zarr"
    shutil.rmtree(path, ignore_errors=True)
    group = zarr.open_group(str(path), mode="w")
    group.attrs.update(
        {
            "tile_ripper_grid": {
                "crs": "EPSG:32617",
                "transform": [500000, 10, 0, 4000000, 0, -10],
            },
            "title": "medium deterministic regular cube",
        }
    )
    times = np.array([1735689600, 1735776000, 1735948800, 1736035200], dtype="<i8")
    raw = (np.arange(4 * 256 * 320).reshape(4, 256, 320) % 60000 + 1).astype("<u2")
    raw[:, :17, :19] = 0
    for name, values in [
        ("x", 500000 + (np.arange(320) + 0.5) * 10),
        ("y", 4000000 - (np.arange(256) + 0.5) * 10),
        ("time", times),
        ("red", raw),
        ("nir", raw // 2),
    ]:
        data = name in ("red", "nir")
        ar = group.create_dataset(
            name,
            data=values,
            chunks=((2, 64, 80) if name == "red" else (1, 80, 64))
            if data
            else values.shape,
            fill_value=0 if data else None,
            compressor=GZip(1) if data else None,
            dimension_separator="/" if name == "nir" else ".",
        )
        ar.attrs["_ARRAY_DIMENSIONS"] = ["time", "y", "x"] if data else [name]
        if name == "time":
            ar.attrs.update(
                {
                    "units": "seconds since 1970-01-01 00:00:00",
                    "calendar": "proleptic_gregorian",
                }
            )
        if data:
            ar.attrs.update(
                {
                    "_FillValue": 0,
                    "scale_factor": 0.0001,
                    "add_offset": -0.1,
                    "units": "1",
                }
            )
    (path / "nir/1/1/1").unlink()
    r = copy.deepcopy(request)
    r["source"]["assets"][0]["locator"] = str(path)
    r["time"] = None
    r["spatial"]["bbox"] = [500015, 3997445, 503195, 3999985]
    return r


def zarr_probes(request, keys):
    from numcodecs import GZip
    import zarr

    store = zarr.DirectoryStore(request["source"]["assets"][0]["locator"])
    encoded = []
    t = time.perf_counter()
    for key in keys:
        try:
            encoded.append(store[key])
        except KeyError:
            pass
    reading = time.perf_counter() - t
    t = time.perf_counter()
    for data in encoded:
        GZip().decode(data)
    return {
        "read_only_seconds": reading,
        "decode_only_seconds": time.perf_counter() - t,
        "encoded_bytes": sum(map(len, encoded)),
        "note": "independent warm DirectoryStore/gzip probes; not additive pipeline stages",
    }


def run_zarr(reps):
    from zarr_workflow import fixture, check

    WORK.mkdir(parents=True, exist_ok=True)
    request = json.loads(fixture().read_text())
    rows, selections, probes = [], [], []
    rng = random.Random(20261020)
    for workload, r in [("small", request), ("medium", medium_fixture(request))]:
        keys = minimum_keys(r)
        file = WORK / f"zarr-{workload}.json"
        file.write_text(json.dumps(r))
        planned = process([str(CURRENT), "plan", str(file)])["result"]
        planned_keys = sorted(
            n["operation"]["read"]["key"]
            for n in planned["plan"]["nodes"]
            if n["operation"]["kind"] == "read"
        )
        assert keys == planned_keys
        previous_plan = process([str(PREVIOUS), "plan", str(file)])["result"]
        previous_visits = {}
        source_path = Path(r["source"]["assets"][0]["locator"])
        for node in previous_plan["nodes"]:
            op = node["operation"]
            if op["kind"] != "read":
                continue
            read = op["read"]
            if read["kind"] != "dataset_slice":
                continue
            v = read["path"]
            meta = json.loads((source_path / v / ".zarray").read_text())
            sep = meta.get("dimension_separator", ".")
            bins = [
                range(start // chunk, (start + count - 1) // chunk + 1)
                for start, count, chunk in zip(
                    read["start"], read["count"], meta["chunks"]
                )
            ]
            for indices in itertools.product(*bins):
                key = v + "/" + sep.join(str(i) for i in indices)
                previous_visits[key] = previous_visits.get(key, 0) + 1
        assert sorted(previous_visits) == keys
        selections.append(
            {
                "workload": workload,
                "minimum_keys": keys,
                "planned_keys": planned_keys,
                "previous_planned_key_visits": previous_visits,
                "previous_distinct_keys": sorted(previous_visits),
            }
        )
        for rep in range(reps):
            order = ["previous-46dd503", "unified", "xarray"]
            rng.shuffle(order)
            for reader in order:
                out = WORK / f"zarr-{workload}-{reader}-{rep}"
                shutil.rmtree(out, ignore_errors=True)
                cmd = (
                    [
                        sys.executable,
                        __file__,
                        "zarr-worker",
                        "--request",
                        str(file),
                        "--output",
                        str(out),
                    ]
                    if reader == "xarray"
                    else [
                        str(CURRENT if reader == "unified" else PREVIOUS),
                        "fetch",
                        str(file),
                        "--output",
                        str(out),
                    ]
                )
                row = process(cmd)
                row.update(reader=reader, workload=workload, repetition=rep)
                result = row.pop("result")
                report = result.get("observed", result)
                if reader == "unified":
                    actual = {v["key"]: v["requests"] for v in report["source_reads"]}
                    assert sorted(actual) == keys and set(actual.values()) == {1}
                    assert report["source_data_requests"] == len(keys)
                    row["observed_keys"] = actual
                elif reader == "previous-46dd503":
                    assert report["source_data_requests"] == sum(
                        previous_visits.values()
                    )
                elif reader == "xarray":
                    observed = sorted(
                        set(report["data_keys"]) | set(report["probed_data_keys"])
                    )
                    assert observed == keys, (observed, keys)
                row["report"] = report
                row["validated_values"] = check(r, out)
                rows.append(row)
            probes.append(
                {"workload": workload, "repetition": rep, **zarr_probes(r, keys)}
            )
    edge_checks = []
    for label, bbox, dates in [
        ("upper-left", [499990, 3999975, 500015, 4000010], None),
        ("lower-right", [501590, 3998710, 501700, 3998730], None),
        (
            "chunk-boundary",
            [500400, 3999360, 500800, 3999680],
            {
                "kind": "instants",
                "values": ["2025-01-01T00:00:00Z", "2025-01-02T00:00:00Z"],
            },
        ),
        (
            "inclusive-time",
            request["spatial"]["bbox"],
            {
                "kind": "interval",
                "start": "2025-01-02T00:00:00Z",
                "end": "2025-01-02T00:00:00Z",
            },
        ),
        (
            "irregular-instants",
            request["spatial"]["bbox"],
            {
                "kind": "instants",
                "values": ["2025-01-04T00:00:00Z", "2025-01-01T00:00:00Z"],
            },
        ),
        (
            "empty-time",
            request["spatial"]["bbox"],
            {"kind": "instants", "values": ["2000-01-01T00:00:00Z"]},
        ),
        ("empty-space", [0, 0, 1, 1], None),
    ]:
        r = copy.deepcopy(request)
        r["spatial"]["bbox"] = bbox
        r["time"] = dates
        file = WORK / (label + ".json")
        file.write_text(json.dumps(r))
        out = WORK / (label + "-out")
        shutil.rmtree(out, ignore_errors=True)
        keys = minimum_keys(r)
        p = process([str(CURRENT), "plan", str(file)])["result"]
        planned = sorted(
            n["operation"]["read"]["key"]
            for n in p["plan"]["nodes"]
            if n["operation"]["kind"] == "read"
        )
        m = process([str(CURRENT), "fetch", str(file), "--output", str(out)])["result"]
        actual = {v["key"]: v["requests"] for v in m["observed"]["source_reads"]}
        assert keys == planned == sorted(actual) and all(
            v == 1 for v in actual.values()
        )
        if keys:
            check(r, out)
        else:
            assert m["outputs"] == [] and m["observed"]["chunks"] == 0
        edge_checks.append(
            {
                "case": label,
                "minimum_keys": keys,
                "planned_keys": planned,
                "observed_keys": actual,
                "exact_values": True,
            }
        )
    # A terminated writer must not leave a schema-3 completion marker.
    file = WORK / "zarr-medium.json"
    out = WORK / "interrupted-out"
    shutil.rmtree(out, ignore_errors=True)
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        child = subprocess.Popen(
            [str(CURRENT), "fetch", str(file), "--output", str(out)],
            stdout=stdout,
            stderr=stderr,
            cwd=ROOT,
        )
        deadline = time.monotonic() + 10
        while not out.exists() and child.poll() is None and time.monotonic() < deadline:
            time.sleep(0.001)
        assert out.exists() and child.poll() is None
        child.terminate()
        child.wait(timeout=10)
        assert child.returncode != 0
        manifest = out / "manifest.json"
        if manifest.exists():
            assert json.loads(manifest.read_text()).get("status") != "complete"
    shutil.rmtree(out)
    result = {
        "environment": environment(),
        "runs": rows,
        "summary": summarize(rows),
        "physical_selection": selections,
        "edge_checks": edge_checks,
        "hard_interruption_without_completion_marker": True,
        "independent_probes": probes,
        "notes": [
            "five interleaved fresh processes per reader/workload; warm local cache",
            "current data chunk set equals independent coordinate/chunk minimum, including absent keys",
            "xarray may test absence without a get; probed keys are reported separately",
            "metadata/coordinate reads excluded from data key counters; discovery not applicable",
            "read/decode production timer fused; separate probes are not additive",
            "before/after wall includes startup, metadata checks, different output writing and provenance",
        ],
    }
    (ROOT / "docs/unified-zarr-benchmark.json").write_text(
        json.dumps(result, indent=2) + "\n"
    )
    print(json.dumps({"runs": len(rows), "summary": result["summary"]}))


def common_stac(r):
    fields = {
        k: v
        for k, v in r.get("limits", {}).items()
        if k not in ("buffer_bytes", "chunk_edge", "max_nodes")
    }
    return {
        "source": {
            "kind": "stac",
            "endpoint": r["endpoint"],
            "collection": r["collection"],
            "limits": fields,
        },
        "spatial": r["aoi"],
        "time": {"kind": "interval", "start": r["start"], "end": r["end"]},
        "variables": [{"kind": "named", "name": b} for b in r["bands"]],
        "grid": {"kind": "native"},
        "sink": {"kind": "directory", "path": r["output"]},
        "budget": {
            "buffer_bytes": r.get("limits", {}).get("buffer_bytes", 4194304),
            "chunk_edge": r.get("limits", {}).get("chunk_edge", 512),
            "max_nodes": r.get("limits", {}).get("max_nodes", 100000),
        },
    }


def run_stac(reps, result_path=None):
    import stac_workflow as oracle

    WORK.mkdir(parents=True, exist_ok=True)
    server = oracle.Server(("127.0.0.1", 0), oracle.Handler)
    server.root = WORK / "stac-source"
    server.files, server.features = oracle.create(server.root, server.server_port)
    server.queries, server.seen_conditions = [], []
    server.reset()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    rows, probes = [], []
    rng = random.Random(20261020)
    try:
        for large in (False, True):
            workload = "medium" if large else "small"
            for rep in range(reps):
                order = ["previous-46dd503", "unified", "gdal-stac-tuned"]
                rng.shuffle(order)
                for reader in order:
                    out = WORK / f"stac-{workload}-{reader}-{rep}"
                    shutil.rmtree(out, ignore_errors=True)
                    r = oracle.request(server.server_port, out, large)
                    legacy_file = WORK / "stac-legacy.json"
                    legacy_file.write_text(json.dumps(r))
                    common_file = WORK / "stac-common.json"
                    common_file.write_text(json.dumps(common_stac(r)))
                    server.reset()
                    cmd = (
                        [str(CURRENT), "fetch", str(common_file)]
                        if reader == "unified"
                        else [str(PREVIOUS), "stac-fetch", str(legacy_file)]
                        if reader == "previous-46dd503"
                        else [
                            sys.executable,
                            str(Path(oracle.__file__)),
                            "--worker",
                            str(legacy_file),
                        ]
                    )
                    row = process(cmd)
                    manifest = row.pop("result")
                    stages = dict(
                        manifest.get(
                            "observed",
                            {
                                k: v
                                for k, v in manifest.items()
                                if k.endswith("_seconds")
                            },
                        )
                    )
                    stages["discovery_seconds"] = manifest.get(
                        "discovery_seconds",
                        manifest.get("discovery", {}).get("seconds"),
                    )
                    stages["final_flush_seconds"] = manifest.get("final_flush_seconds")
                    row.update(
                        reader=reader,
                        workload=workload,
                        repetition=rep,
                        observed=dict(server.counts),
                        validation=oracle.verify(r, out, server.root, manifest),
                        stages=stages,
                    )
                    if reader == "unified":
                        assert (
                            manifest["schema_version"] == 3
                            and manifest["status"] == "complete"
                        )
                    rows.append(row)
                planned = process([str(CURRENT), "plan", str(common_file)])["result"]
                probes.append(
                    {
                        "workload": workload,
                        "repetition": rep,
                        **oracle.probes(planned, server.root),
                    }
                )
    finally:
        server.shutdown()
        server.server_close()
    result = {
        "environment": environment(),
        "runs": rows,
        "summary": summarize(rows),
        "independent_probes": probes,
        "notes": [
            "loopback HTTP; no public provider traffic",
            "native grids/nodata independently verified via rasterio/GDAL for every output",
            "same bounded fixture and conditional version checks before and after",
            "fused production read/decode; independent transfer/decode probes nonadditive",
            "baseline freezes independently known selected item IDs while doing real pagination",
        ],
    }
    (result_path or ROOT / "docs/unified-stac-benchmark.json").write_text(
        json.dumps(result, indent=2) + "\n"
    )
    print(json.dumps({"runs": len(rows), "summary": result["summary"]}))


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["zarr", "stac", "zarr-worker"])
    ap.add_argument("--reps", type=int, default=5)
    ap.add_argument("--request")
    ap.add_argument("--output")
    args = ap.parse_args()
    if args.mode == "zarr-worker":
        print(
            json.dumps(
                zarr_baseline(json.loads(Path(args.request).read_text()), args.output)
            )
        )
    elif args.mode == "zarr":
        run_zarr(args.reps)
    else:
        run_stac(args.reps)
