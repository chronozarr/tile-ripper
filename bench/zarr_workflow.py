"""Independent xarray/Zarr oracle, deterministic demo and fresh-process benchmark.

Run with the documented isolated Python environment. No remote data or downloads.
"""

import argparse
import hashlib
import platform
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

import numpy as np
import xarray as xr
import zarr
from numcodecs import GZip

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/zarr"
BIN = ROOT / "target/release/tile-ripper"


def fixture():
    WORK.mkdir(parents=True, exist_ok=True)
    source = WORK / "source.zarr"
    if source.exists():
        shutil.rmtree(source)
    t = np.array([1735689600, 1735776000, 1735948800, 1736035200], dtype="int64")
    x = 500000 + (np.arange(160) + 0.5) * 10
    y = 4000000 - (np.arange(128) + 0.5) * 10
    raw = (np.arange(4 * 128 * 160).reshape(4, 128, 160) % 60000 + 1).astype("uint16")
    raw[:, :7, :11] = 0
    ds = xr.Dataset(
        {"red": (("time", "y", "x"), raw), "nir": (("time", "y", "x"), raw // 2)},
        coords={"time": t, "y": y, "x": x},
        attrs={
            "title": "deterministic georeferenced validation cube",
            "tile_ripper_grid": {
                "crs": "EPSG:32617",
                "transform": [500000, 10, 0, 4000000, 0, -10],
            },
        },
    )
    ds.time.attrs = {
        "units": "seconds since 1970-01-01 00:00:00",
        "calendar": "proleptic_gregorian",
    }
    for v in ["red", "nir"]:
        ds[v].attrs = {
            "units": "1",
            "long_name": v + " raw DN",
            "scale_factor": 0.0001,
            "add_offset": -0.1,
        }
    encoding = {
        v: {"chunks": (2, 32, 40), "compressor": GZip(1), "_FillValue": np.uint16(0)}
        for v in ["red", "nir"]
    }
    encoding.update(
        {v: {"compressor": None, "_FillValue": None} for v in ["x", "y", "time"]}
    )
    # Build via Zarr directly: xarray's encoder applies scale_factor to stored data.
    group = zarr.open_group(str(source), mode="w")
    group.attrs.update(ds.attrs)
    for v in ds.variables:
        values = ds[v].values
        enc = encoding[v]
        ar = group.create_dataset(
            v,
            data=values,
            chunks=(1, 40, 32) if v == "nir" else enc.get("chunks", values.shape),
            dimension_separator="/" if v == "nir" else ".",
            compressor=enc["compressor"],
            fill_value=enc["_FillValue"],
        )
        ar.attrs.update({**ds[v].attrs, "_ARRAY_DIMENSIONS": list(ds[v].dims)})
        if v in ["red", "nir"]:
            ar.attrs["_FillValue"] = 0
    # An absent chunk is a valid sparse/fill region, not an I/O error.
    (source / "nir/1/1/1").unlink()
    request = {
        "source": {
            "kind": "assets",
            "assets": [
                {
                    "id": "cube",
                    "locator": str(source),
                    "format": "zarr_v2",
                    "datetime": None,
                }
            ],
        },
        "spatial": {
            "kind": "bounds",
            "crs": "EPSG:32617",
            "bbox": [500155, 3999095, 501035, 3999925],
        },
        "time": {
            "kind": "interval",
            "start": "2025-01-02T00:00:00Z",
            "end": "2025-01-04T00:00:00Z",
        },
        "variables": [{"kind": "dataset", "path": v} for v in ["red", "nir"]],
        "grid": {"kind": "native"},
        "sink": {"kind": "zarr_directory", "path": str(WORK / "extract")},
        "budget": {"buffer_bytes": 1048576, "max_nodes": 100000, "chunk_edge": 32},
    }
    file = WORK / "request.json"
    file.write_text(json.dumps(request, indent=2) + "\n")
    return file


class CountStore(zarr.DirectoryStore):
    def __init__(self, path):
        super().__init__(path)
        self.calls = self.bytes = 0

    def __getitem__(self, key):
        is_data = not Path(key).name.startswith(".")
        if is_data:
            self.calls += 1
        data = super().__getitem__(key)
        if is_data:
            self.bytes += len(data)
        return data


def expected(request, source):
    # Independently compute outer half-open raster selection from coordinates.
    xmin, ymin, xmax, ymax = request["spatial"]["bbox"]
    x = np.where((source.x.values + 5 > xmin) & (source.x.values - 5 < xmax))[0]
    y = np.where((source.y.values + 5 > ymin) & (source.y.values - 5 < ymax))[0]
    seconds = source.time.values
    selection = request.get("time")
    if selection is None:
        ti = np.arange(len(seconds))
    else:

        def epoch(v):
            return np.datetime64(v.rstrip("Z"), "s").astype("int64")

        if selection["kind"] == "interval":
            ti = np.where(
                (seconds >= epoch(selection["start"]))
                & (seconds <= epoch(selection["end"]))
            )[0]
        else:
            ti = np.where(np.isin(seconds, [epoch(v) for v in selection["values"]]))[0]
    return source[[v["path"].lstrip("/") for v in request["variables"]]].isel(
        time=ti, y=y, x=x
    )


def baseline(request, output):
    source_path = request["source"]["assets"][0]["locator"]
    store = CountStore(source_path)
    t = time.perf_counter()
    source = xr.open_zarr(store, consolidated=False, decode_cf=False, chunks=None)
    metadata = time.perf_counter() - t
    t = time.perf_counter()
    subset = expected(request, source)
    planning = time.perf_counter() - t
    # Exclude coordinate traffic from data counts, matching the Rust report.
    store.calls = store.bytes = 0
    t = time.perf_counter()
    subset.load()
    reading = time.perf_counter() - t
    calls, size = store.calls, store.bytes
    t = time.perf_counter()
    subset.to_zarr(output, mode="w", consolidated=False)
    return {
        "metadata_seconds": metadata,
        "planning_seconds": planning,
        "read_decode_seconds": reading,
        "writing_seconds": time.perf_counter() - t,
        "data_requests": calls,
        "data_bytes": size,
    }


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
            raise RuntimeError(err.read().decode()[:2000])
        return {
            "report": (lambda value: value.get("observed", value))(json.load(out)),
            "wall_seconds": time.perf_counter() - start,
            "peak_rss_bytes": usage.ru_maxrss
            * (1 if sys.platform == "darwin" else 1024),
        }


def check(request, output):
    source = xr.open_zarr(
        request["source"]["assets"][0]["locator"],
        consolidated=False,
        decode_cf=False,
        chunks=None,
    )
    reference = expected(request, source).load()
    actual = xr.open_zarr(
        output / "cube.zarr", consolidated=False, decode_cf=False, chunks=None
    ).load()
    for name in reference.variables:
        xr.testing.assert_identical(reference[name], actual[name])
        assert reference[name].dtype == actual[name].dtype
    attrs = dict(reference.attrs)
    transform = list(attrs["tile_ripper_grid"]["transform"])
    transform[0] = float(reference.x.values[0] - 5)
    transform[3] = float(reference.y.values[0] + 5)
    attrs["tile_ripper_grid"] = {**attrs["tile_ripper_grid"], "transform": transform}
    assert attrs == actual.attrs
    assert zarr.open_group(str(output / "cube.zarr"))["nir"].fill_value == 0
    # Decode CF independently as well: scale/offset, nodata and dates must agree.
    xr.testing.assert_identical(
        xr.decode_cf(reference).drop_attrs(), xr.decode_cf(actual).drop_attrs()
    )
    return int(sum(reference[v].size for v in reference.data_vars))


def run():
    file = fixture()
    request = json.loads(file.read_text())
    rows = []
    for rep in range(5):
        for kind in ["rust", "xarray"] if rep % 2 == 0 else ["xarray", "rust"]:
            output = WORK / f"{kind}-{rep}"
            shutil.rmtree(output, ignore_errors=True)
            cmd = (
                [str(BIN), "fetch", str(file), "--output", str(output)]
                if kind == "rust"
                else [sys.executable, __file__, "--baseline", str(file), str(output)]
            )
            row = process(cmd)
            row.update(reader=kind, repetition=rep)
            if kind == "rust":
                row["validated_values"] = check(request, output)
            rows.append(row)
    gates = []
    cases = [
        ([499900, 3999900, 500100, 4000100], None),
        (
            [500399, 3999601, 500801, 3999681],
            {
                "kind": "instants",
                "values": ["2025-01-01T00:00:00Z", "2025-01-05T00:00:00Z"],
            },
        ),
        ([501590, 3998710, 501700, 3998730], None),
    ]
    for i, (bbox, dates) in enumerate(cases):
        r = json.loads(file.read_text())
        r["spatial"]["bbox"] = bbox
        r["time"] = dates
        f = WORK / f"case-{i}.json"
        f.write_text(json.dumps(r))
        out = WORK / f"case-{i}"
        shutil.rmtree(out, ignore_errors=True)
        process([str(BIN), "fetch", str(f), "--output", str(out)])
        gates.append({"case": i, "validated_values": check(r, out)})
    # Empty time/space, refusal to overwrite and clear unsupported selections.
    for label, change in [
        (
            "empty-time",
            lambda r: r.update(
                time={"kind": "instants", "values": ["2020-01-01T00:00:00Z"]}
            ),
        ),
        ("empty-space", lambda r: r["spatial"].update(bbox=[0, 0, 1, 1])),
    ]:
        r = json.loads(file.read_text())
        change(r)
        f = WORK / (label + ".json")
        f.write_text(json.dumps(r))
        out = WORK / label
        shutil.rmtree(out, ignore_errors=True)
        result = process([str(BIN), "fetch", str(f), "--output", str(out)])
        assert result["report"]["chunks"] == 0
        gates.append({"case": label, "passed": True})
    for label, change in [
        ("wrong-crs", lambda r: r["spatial"].update(crs="EPSG:4326")),
        ("budget", lambda r: r["budget"].update(buffer_bytes=2048)),
        (
            "unknown-variable",
            lambda r: r.update(variables=[{"kind": "dataset", "path": "absent"}]),
        ),
        (
            "traversal",
            lambda r: r.update(variables=[{"kind": "dataset", "path": "../red"}]),
        ),
        ("node-cap", lambda r: r["budget"].update(max_nodes=1)),
    ]:
        r = json.loads(file.read_text())
        change(r)
        f = WORK / (label + ".json")
        f.write_text(json.dumps(r))
        out = WORK / label
        shutil.rmtree(out, ignore_errors=True)
        p = subprocess.run(
            [str(BIN), "fetch", str(f), "--output", str(out)], capture_output=True
        )
        assert p.returncode and not out.exists(), (label, p.stderr)
        gates.append({"case": label, "passed": True})
    p = subprocess.run(
        [str(BIN), "fetch", str(file), "--output", str(WORK / "rust-0")],
        capture_output=True,
    )
    assert p.returncode
    gates.append({"case": "overwrite-refused", "passed": True})
    # Existing but corrupt data is an error, while absent chunks above use fill.
    chunk = WORK / "source.zarr/red/0.0.0"
    original = chunk.read_bytes()
    chunk.write_bytes(b"corrupt")
    process([str(BIN), "plan", str(file)])
    out = WORK / "corrupt"
    shutil.rmtree(out, ignore_errors=True)
    p = subprocess.run(
        [str(BIN), "fetch", str(file), "--output", str(out)], capture_output=True
    )
    chunk.write_bytes(original)
    assert p.returncode and not out.exists()
    gates.append({"case": "corrupt-cleanup", "passed": True})
    result = {
        "environment": {
            "platform": platform.platform(),
            "python": sys.version,
            "xarray": xr.__version__,
            "zarr": zarr.__version__,
            "numpy": np.__version__,
            "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
            "gdal": subprocess.check_output(
                ["gdal-config", "--version"], text=True
            ).strip(),
            "cargo_lock_sha256": hashlib.sha256(
                (ROOT / "Cargo.lock").read_bytes()
            ).hexdigest(),
            "source_sha256": hashlib.sha256(
                b"".join(
                    p.name.encode() + p.read_bytes()
                    for p in sorted((ROOT / "src").glob("*.rs"))
                )
            ).hexdigest(),
        },
        "layout": "local Zarr v2 UInt16 time/y/x, gzip, native affine",
        "runs": rows,
        "gates": gates,
        "notes": [
            "warm local filesystem cache; interleaved fresh processes",
            "wall and RSS include language startup; phase timings do not",
            "data bytes/requests exclude metadata and coordinates; local store calls, not network",
            "no discovery phase for explicit assets; transfer and decode fused in both libraries",
            "no speedup claim; output encodings may differ",
        ],
    }
    (ROOT / "docs/zarr-benchmark-results.json").write_text(
        json.dumps(result, indent=2) + "\n"
    )
    print(
        json.dumps(
            {
                "runs": len(rows),
                "gates": len(gates),
                "validated_values": sum(r.get("validated_values", 0) for r in rows),
            }
        )
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixture", action="store_true")
    parser.add_argument("--baseline", nargs=2)
    args = parser.parse_args()
    if args.fixture:
        print(fixture())
    elif args.baseline:
        print(
            json.dumps(
                baseline(
                    json.loads(Path(args.baseline[0]).read_text()), args.baseline[1]
                )
            )
        )
    else:
        run()
