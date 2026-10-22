"""Version D quality qualification. Only inspect/capture/access contact NASA.

All replay, science, fault and performance commands are offline conditional
range mirrors. Sparse source holes are never served. libhdf5 is reference-only.
"""

import argparse
import hashlib
import json
import os
import re
import resource
import subprocess
import sys
import time
from pathlib import Path

import h5py
import hdf5_input as h
import numpy as np
import swot_wse as old
from hdf5_reference import Remote
from hdf5_selective_mirror import Mirror, server

ROOT = h.ROOT
WORK = ROOT / "prototype-output/swot-quality"
DOCS = ROOT / "docs/swot-quality"
BINARY = ROOT / "target/release/examples/swot_quality"
PATHS = ["wse", "wse_qual", "wse_qual_bitwise", "wse_uncert", "n_wse_pix"]
DTYPES = ["<f4", "u1", "<u4", "<f4", "<u4"]
POLICIES = {
    "raw": {"kind": "raw"},
    "cookbook": {"kind": "summary", "accepted": ["good", "suspect"]},
    "good": {"kind": "summary", "accepted": ["good"]},
    "bitwise": {
        "kind": "bitwise",
        "require": [],
        "exclude": [
            "value_bad",
            "outside_data_window",
            "no_pixels",
            "outside_scene_bounds",
            "inner_swath",
            "missing_karin_data",
            "specular_ringing_prior_land_degraded",
        ],
    },
    "ringing": {
        "kind": "bitwise",
        "require": ["specular_ringing_prior_water_suspect"],
        "exclude": ["value_bad", "missing_karin_data"],
    },
}
SELECTIONS = {
    **old.SELECTIONS,
    "missing": ((0, 0), (20, 20), (1, 1)),
    "dense": ((480, 300), (300, 300), (1, 1)),
}
WORK.mkdir(parents=True, exist_ok=True)
DOCS.mkdir(parents=True, exist_ok=True)


def save(name, value):
    (DOCS / f"{name}.json").write_text(
        json.dumps(value, indent=2, allow_nan=False, default=str) + "\n"
    )


def request(locator, name="window", budget=32 * 1024 * 1024):
    r = h.request(str(locator), "/wse", *SELECTIONS[name])
    r["variables"] = [{"kind": "dataset", "path": "/" + n} for n in PATHS]
    r["budget"]["buffer_bytes"] = budget
    return r


def run(r, policy, name, mode="dump", control=None):
    out = WORK / name
    out.mkdir(parents=True, exist_ok=True)
    (out / "request.json").write_text(json.dumps(r))
    (out / "policy.json").write_text(json.dumps(policy))
    env = os.environ.copy()
    env["TILE_RIPPER_WORKERS"] = "1"
    if control:
        env["TILE_RIPPER_TEST_CONTROL"] = control
    start = time.perf_counter()
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    with (
        (out / "stdout.json").open("wb") as stdout,
        (out / "stderr.txt").open("wb") as stderr,
    ):
        p = subprocess.run(
            [
                "/usr/bin/time",
                "-l" if sys.platform == "darwin" else "-v",
                str(BINARY),
                str(out / "request.json"),
                str(out / "policy.json"),
                mode,
                str(out),
            ],
            check=False,
            stdout=stdout,
            stderr=stderr,
            env=env,
            timeout=180,
        )
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    error = (out / "stderr.txt").read_text()
    rss = (
        re.search(r"(\d+)\s+maximum resident set size", error)
        if sys.platform == "darwin"
        else re.search(r"Maximum resident set size \(kbytes\): (\d+)", error)
    )
    result = {
        "returncode": p.returncode,
        "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
        "elapsed_seconds": time.perf_counter() - start,
        "child_cpu_seconds": after.ru_utime
        + after.ru_stime
        - before.ru_utime
        - before.ru_stime,
        "peak_rss_bytes": int(rss.group(1)) * (1 if sys.platform == "darwin" else 1024)
        if rss
        else None,
        "output_dir": str(out),
    }
    if p.returncode:
        result["error"] = error[:1200]
    else:
        result["result"] = json.loads((out / "stdout.json").read_text())
    return result


def capture():
    profile = json.loads((DOCS / "product-profile.json").read_text())
    session = old.ArchiveSession()
    mirror = Mirror(
        WORK / "mirror", profile["url"], cap=24 * 1024 * 1024, session=session
    )
    assert (
        mirror.info["etag"] == profile["etag"]
        and mirror.info["size"] == profile["size"]
    )
    # Actual chunks of only the five declared layers and projected axes. No
    # acquisition or reference path may fetch uncaptured science variables.
    ranges = [(e["start"], e["start"] + e["bytes"]) for e in profile["events"]]
    for name in PATHS + ["x", "y"]:
        ranges += [
            (c["address"], c["address"] + c["size"])
            for c in profile["datasets"][name]["allocated_chunks"]
        ]
    merged = []
    for a, b in sorted(
        (a // 16384 * 16384, min(profile["size"], (b + 16383) // 16384 * 16384))
        for a, b in ranges
    ):
        if merged and a <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(b, merged[-1][1]))
        else:
            merged.append((a, b))
    for a, b in merged:
        # Bound a single transfer even when adjacent chunk extents merge.
        for x in range(a, b, 1024 * 1024):
            mirror.read(x, min(b - x, 1024 * 1024))
    with server(mirror) as srv:
        source = Remote(srv.url, page=16384)
        with h5py.File(source, "r") as f:
            _ = dict(f.attrs)
            for ds in f.values():
                _ = dict(ds.attrs)
            for name in SELECTIONS:
                sl = old.slices(request(srv.url, name)["spatial"])
                for path in PATHS:
                    _ = f[path][sl]
        source.close()
        for metadata in ["exact", "page4k", "page16k"]:
            with old.policies(metadata=metadata):
                result = run(
                    request(srv.url),
                    POLICIES["cookbook"],
                    f"capture-{metadata}",
                    mode="stream",
                )
            assert result["returncode"] == 0, result.get("error")
        assert not srv.errors, srv.errors
    mirror.seal()
    mirror.verify()
    # Inspect numeric definitions from the sealed captured headers, without
    # another provider read or trusting size alone for integer interpretation.
    with h5py.File(mirror.path, "r") as f:
        for name in ["wse_qual", "wse_qual_bitwise", "n_wse_pix"]:
            datatype = f[name].id.get_type()
            profile["datasets"][name]["integer_precision_bits"] = (
                datatype.get_precision()
            )
            profile["datasets"][name]["integer_bit_offset"] = datatype.get_offset()
            profile["datasets"][name]["dtype_str"] = f[name].dtype.str
            assert (
                datatype.get_offset() == 0
                and datatype.get_precision() == f[name].dtype.itemsize * 8
            )
    save("product-profile", profile)
    save(
        "capture",
        {
            **mirror.info,
            "archive_events": session.events,
            "note": "Explicit, capped reference capture of incomplete Version D object; mirror is not live-native access.",
        },
    )
    print(
        json.dumps(
            {
                "provider_bytes": mirror.info["provider_bytes"],
                "provider_gets": mirror.info["provider_gets"],
                "source_bytes": mirror.info["size"],
            }
        )
    )


def fixtures():
    profile = json.loads((DOCS / "product-profile.json").read_text())
    dest = ROOT / "tests/data/swot"
    dest.mkdir(parents=True, exist_ok=True)
    for order in ["le", "be"]:
        path = dest / f"quality-{order}.h5"
        shape = (37, 53)
        y, x = np.indices(shape)
        flags = np.array(
            [0, 2, 4, 32, 128, 512, 4096, 4194304, 16777216, 2147483648, 1], dtype="u4"
        )[(y * shape[1] + x) % 11]
        values = [
            (10 + y * 0.25 + x * 0.125).astype("f4"),
            np.select(
                [flags == 0, flags < 32768, flags < 8388608], [0, 1, 2], default=3
            ).astype("u1"),
            flags,
            np.full(shape, 0.25, dtype="f4"),
            (y * 53 + x + 1).astype("u4"),
        ]
        chunks = [(11, 13), (37, 53), (7, 9), (13, 8), (6, 17)]
        with h5py.File(path, "w", libver="earliest") as f:
            f.attrs["crid"] = np.bytes_("PGD0")
            f.attrs["title"] = np.bytes_("independent Version D schema fixture")
            for name, n in [("y", shape[0]), ("x", shape[1])]:
                axis = f.create_dataset(
                    name,
                    data=np.arange(n, dtype="f8") * 100
                    + (4000000 if name == "y" else 200000),
                    chunks=(5 if name == "y" else 7,),
                    compression="gzip",
                    shuffle=True,
                    fillvalue=-9999.0,
                )
                axis.attrs["_FillValue"] = np.float64(-9999.0)
                axis.attrs["units"] = np.bytes_("m")
                axis.make_scale(name)
            crs = f.create_dataset("crs", data=np.bytes_(" "))
            crs.attrs["spatial_ref"] = np.bytes_("test projected CRS")
            crs.attrs["grid_mapping_name"] = np.bytes_("transverse_mercator")
            for i, (name, v, block, dt) in enumerate(
                zip(PATHS, values, chunks, DTYPES)
            ):
                dt = np.dtype(dt).newbyteorder(">" if order == "be" else "<")
                fill = np.finfo("f4").max if dt.kind == "f" else np.iinfo(dt).max
                ds = f.create_dataset(
                    name,
                    shape=shape,
                    dtype=dt,
                    chunks=block,
                    compression="gzip",
                    shuffle=True,
                    fletcher32=True,
                    fillvalue=fill,
                )
                for a in range(0, shape[0], block[0]):
                    for b in range(0, shape[1], block[1]):
                        if i != 1 and a == block[0] and b == block[1]:
                            continue
                        ds[a : a + block[0], b : b + block[1]] = v[
                            a : a + block[0], b : b + block[1]
                        ]
                ds.attrs["_FillValue"] = np.array([fill], dtype=dt)
                ds.attrs["grid_mapping"] = np.bytes_("crs")
                ds.attrs["coordinates"] = np.bytes_("x y")
                ds.attrs["units"] = np.bytes_("m" if i in [0, 3] else "1")
                for j, axis in enumerate(["y", "x"]):
                    ds.dims[j].attach_scale(f[axis])
                if i in [1, 2]:
                    for key in ["flag_values", "flag_masks", "flag_meanings"]:
                        if key in profile["datasets"][name]["attrs"]:
                            val = profile["datasets"][name]["attrs"][key]
                            ds.attrs[key] = (
                                np.bytes_(val)
                                if isinstance(val, str)
                                else np.asarray(val, dtype=dt)
                            )
    for dt in ["u1", ">u4"]:
        for fill in [None, 17]:
            path = dest / f"initial-{np.dtype(dt).itemsize}-{fill}.h5"
            with h5py.File(path, "w", libver="earliest") as f:
                for axis in ["x", "y"]:
                    f.create_dataset(axis, data=np.arange(4, dtype="f8"))
                kwargs = {} if fill is None else {"fillvalue": fill}
                f.create_dataset(
                    "signal", shape=(4, 4), dtype=dt, chunks=(2, 2), **kwargs
                )
    print("independent golden SWOT fixtures written")


def reference(r, policy, path, name, reader="h5py", url=None, page=0, dump=True):
    out = WORK / f"reference-{name}-{reader}"
    out.mkdir(parents=True, exist_ok=True)
    req = {"reader": reader, "spatial": r["spatial"], "policy": policy}
    req.update({"url": url, "page": page} if url else {"local": str(path)})
    if dump:
        req["dump"] = str(out)
    q = out / "request.json"
    q.write_text(json.dumps(req))
    start = time.perf_counter()
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    p = subprocess.run(
        [sys.executable, str(ROOT / "bench/swot_quality_reference.py"), str(q)],
        check=False,
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert p.returncode == 0, p.stderr[-2500:]
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    v = json.loads(p.stdout)
    v.update(
        process_wall_seconds=time.perf_counter() - start,
        process_cpu_seconds=after.ru_utime
        + after.ru_stime
        - before.ru_utime
        - before.ru_stime,
    )
    return v, out


def verify(native, r, path, policy, name):
    assert native["returncode"] == 0, native.get("error")
    result = native["result"]
    out = Path(native["output_dir"])
    count = r["spatial"]["count"]
    steps = r["spatial"]["step"]
    starts = r["spatial"]["start"]
    arrays = [np.empty(count, dtype=d) for d in DTYPES]
    masks = [np.zeros(count, dtype="u1") for _ in PATHS]
    valid = np.zeros(count, dtype="u1")
    seen = np.zeros(count, dtype="u1")
    for i, t in enumerate(result["tiles"]):
        w = t["layers"][0]["window"]
        a = [(w["y"] - starts[0]) // steps[0], (w["x"] - starts[1]) // steps[1]]
        sl = (slice(a[0], a[0] + w["height"]), slice(a[1], a[1] + w["width"]))
        shape = (w["height"], w["width"])
        seen[sl] += 1
        valid[sl] = np.frombuffer(
            (out / f"{i}.validity").read_bytes(), dtype="u1"
        ).reshape(shape)
        for j in range(5):
            assert t["layers"][j]["window"] == w
            arrays[j][sl] = np.frombuffer(
                (out / f"{i}-{j}.bin").read_bytes(), dtype=DTYPES[j]
            ).reshape(shape)
            masks[j][sl] = np.frombuffer(
                (out / f"{i}-{j}.mask").read_bytes(), dtype="u1"
            ).reshape(shape)
    assert np.all(seen == 1), "missing or repeated logical output pixels"
    with h5py.File(path, "r") as f:
        ds = result["inspection"]["sources"][0]["object_identities"]["datasets"]
        lookup = {d["path"]: d for d in ds}
        for path_name in PATHS:
            d = lookup["/" + path_name]
            source = f[path_name]
            assert d["shape"] == list(source.shape) and d["chunk_shape"] == list(
                source.chunks
            )
            for key, v in d["attributes"].items():
                if "value" not in v:
                    continue
                expected = np.asarray(source.attrs[key])
                value = expected.item() if expected.size == 1 else expected.tolist()
                if isinstance(value, bytes):
                    value = value.decode().rstrip("\0")
                assert v["value"] == value, (path_name, key)
            assert d["root_attributes"]["crid"]["value"] == "PGD0"
        for t in result["tiles"]:
            c = t["layers"][0]
            w = c["window"]
            sl = (
                slice(w["y"], w["y"] + w["height"] * steps[0], steps[0]),
                slice(w["x"], w["x"] + w["width"] * steps[1], steps[1]),
            )
            expected = [f["x"][sl[1]].tolist(), f["y"][sl[0]].tolist()]
            for layer in t["layers"]:
                assert layer["coordinates"] == expected
    controls = []
    for reader in ["h5py", "netcdf4"]:
        ref, ref_out = reference(r, policy, path, name, reader)
        for j in range(5):
            assert arrays[j].tobytes() == (ref_out / f"{j}.bin").read_bytes(), (
                reader,
                j,
                "native stored bits",
            )
            assert masks[j].tobytes() == (ref_out / f"{j}.mask").read_bytes(), (
                reader,
                j,
                "source missingness",
            )
        assert valid.tobytes() == (ref_out / "validity.bin").read_bytes(), (
            reader,
            "quality mask",
        )
        assert result["summary"] == ref["summary"], (result["summary"], ref["summary"])
        controls.append(ref)
    assert (
        result["observed"]["scheduler"]["reserved_bytes_peak"]
        <= r["budget"]["buffer_bytes"]
    )
    return {
        "all_native_value_bits_equal": True,
        "all_five_source_masks_equal": True,
        "exact_logical_coverage": True,
        "all_coordinates_equal": True,
        "derived_mask_equal": True,
        "independent_summaries_equal": True,
        "pixels_per_variable": int(np.prod(count)),
        "tiles": len(result["tiles"]),
        "references": controls,
    }


def metrics(native, events):
    value = native["result"]
    report = value["observed"]
    gets = [e for e in events if e["method"] == "GET" and e["status"] == 206]
    from swot_wse_summary import length

    total = sum(e["bytes"] for e in gets)
    unique = length([(e["start"], e["end"]) for e in gets])
    return (
        {
            k: native[k]
            for k in [
                "binary_sha256",
                "child_cpu_seconds",
                "peak_rss_bytes",
                "elapsed_seconds",
            ]
        }
        | {
            k: value[k]
            for k in [
                "prepare_seconds",
                "first_result_seconds",
                "completion_seconds",
                "summary",
            ]
        }
        | {
            "observed": report,
            "gets": len(gets),
            "heads": sum(e["method"] == "HEAD" for e in events),
            "response_bytes": total,
            "duplicate_bytes": total - unique,
            "largest_range_bytes": max((e["bytes"] for e in gets), default=0),
        }
    )


def science():
    mirror = Mirror(WORK / "mirror")
    mirror.verify()
    rows = []
    with server(mirror) as srv:
        for name in SELECTIONS:
            r = request(srv.url, name)
            for pname, policy in POLICIES.items():
                srv.events.clear()
                label = f"science-{name}-{pname}"
                native = run(r, policy, label)
                agreement = verify(native, r, mirror.path, policy, label)
                rows.append(
                    {
                        "selection": name,
                        "policy": pname,
                        "agreement": agreement,
                        "native": metrics(native, list(srv.events)),
                    }
                )
                print(
                    label,
                    "PASS",
                    native["result"]["summary"]["selected_count"],
                    flush=True,
                )
            # Independent default/experimental traversal and bounded metadata batching.
            if name == "window":
                for index, metadata in [
                    ("full", "page4k"),
                    ("full", "page16k"),
                    ("selective", "exact"),
                ]:
                    srv.events.clear()
                    with old.policies(index, metadata):
                        native = run(
                            r, POLICIES["cookbook"], f"{name}-{index}-{metadata}"
                        )
                    rows.append(
                        {
                            "selection": name,
                            "policy": "cookbook",
                            "index": index,
                            "metadata": metadata,
                            "agreement": verify(
                                native,
                                r,
                                mirror.path,
                                POLICIES["cookbook"],
                                f"{name}-{index}-{metadata}",
                            ),
                            "native": metrics(native, list(srv.events)),
                        }
                    )
        assert not srv.errors, srv.errors
    for order in ["le", "be"]:
        path = ROOT / f"tests/data/swot/quality-{order}.h5"
        r = request(path)
        r["spatial"].update(start=[1, 2], count=[12, 10], step=[3, 5])
        for pname, policy in POLICIES.items():
            label = f"synthetic-{order}-{pname}"
            native = run(r, policy, label)
            agreement = verify(native, r, path, policy, label)
            rows.append(
                {
                    "fixture": order,
                    "policy": pname,
                    "agreement": agreement,
                    "native": metrics(native, []),
                }
            )
    save(
        "science",
        {
            "cells": rows,
            "source_identity": {k: mirror.info[k] for k in ["url", "etag", "size"]},
            "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
            "new_provider_bytes": 0,
        },
    )
    print(json.dumps({"scientific_cells": len(rows)}))


def faults():
    import shutil

    rows = []
    original = ROOT / "tests/data/swot/quality-le.h5"

    def reject(r, policy, label, mode="stream", control=None):
        native = run(r, policy, "fault-" + label, mode, control)
        assert native["returncode"] != 0, (label, "unexpected successful completion")
        rows.append(
            {"case": label, **{k: v for k, v in native.items() if k != "output_dir"}}
        )

    for label in (
        [
            "ambiguous-summary",
            "ambiguous-bitwise",
            "missing-crs",
            "bad-fill",
            "implicit-initialization",
            "distinct-missing",
            "signed-flags",
        ]
        + [f"corrupt-{n}" for n in PATHS]
        + [f"truncated-{n}" for n in PATHS]
    ):
        path = WORK / f"{label}.h5"
        shutil.copyfile(original, path)
        with h5py.File(path, "a") as f:
            if label == "ambiguous-summary":
                f["wse_qual"].attrs["flag_meanings"] = np.bytes_(
                    "good suspect unknown bad"
                )
            elif label == "ambiguous-bitwise":
                f["wse_qual_bitwise"].attrs["flag_masks"] = np.arange(19, dtype="u4")
            elif label == "missing-crs":
                del f["n_wse_pix"].attrs["grid_mapping"]
            elif label == "bad-fill":
                f["n_wse_pix"].attrs["_FillValue"] = np.float64(4294967296)
            elif label == "distinct-missing":
                f["n_wse_pix"].attrs["missing_value"] = np.uint32(0)
            elif label in ["implicit-initialization", "signed-flags"]:
                name = "n_wse_pix"
                del f[name]
                d = f.create_dataset(
                    name,
                    shape=(37, 53),
                    dtype="u4" if label == "implicit-initialization" else "i4",
                    chunks=(6, 17),
                )
                d.attrs["_FillValue"] = np.uint32(4294967295)
            else:
                name = label.split("-", 1)[1]
                ds = f[name]
                mask, raw = ds.id.read_direct_chunk((0, 0))
                data = (
                    raw[: len(raw) // 2]
                    if label.startswith("truncated")
                    else raw[:-1] + bytes([raw[-1] ^ 128])
                )
                ds.id.write_direct_chunk((0, 0), data, filter_mask=mask)
        r = request(path)
        r["spatial"].update(start=[0, 0], count=[5, 5], step=[1, 1])
        reject(
            r,
            POLICIES["bitwise"]
            if label == "ambiguous-bitwise"
            else POLICIES["cookbook"],
            label,
        )
    r = request(original)
    r["spatial"].update(start=[0, 0], count=[20, 20], step=[1, 1])
    for label, policy in [
        (
            "unknown-condition",
            {"kind": "bitwise", "require": ["vector_only_flag"], "exclude": []},
        ),
        (
            "contradiction",
            {"kind": "bitwise", "require": ["bright_land"], "exclude": ["bright_land"]},
        ),
    ]:
        reject(r, policy, label)
    tiny = json.loads(json.dumps(r))
    tiny["budget"]["buffer_bytes"] = 4096
    reject(tiny, POLICIES["cookbook"], "small-budget")
    reject(r, POLICIES["cookbook"], "cancel", mode="cancel")
    # Controlled conditional HTTP faults, and mutation before/after provisional output.
    for fault in [
        "changed-etag",
        "bad-range",
        "interrupt",
        "raw-bitflip",
        "no-range",
        "encoded",
        "exhaust",
    ]:
        with h.server(original, "/wse", fault=fault) as srv:
            rr = request(srv.url)
            rr["spatial"] = r["spatial"]
            reject(rr, POLICIES["cookbook"], "http-" + fault)
    for mode in ["before", "during", "cancel-inflight"]:
        with h.server(
            original, "/wse", latency=0.1 if mode == "cancel-inflight" else 0
        ) as srv:
            rr = request(srv.url)
            rr["spatial"] = r["spatial"]
            control = srv.url.replace(
                "/data.nc",
                "/control/wait-payload"
                if mode == "cancel-inflight"
                else "/control/mutate",
            )
            reject(rr, POLICIES["cookbook"], "http-" + mode, mode, control)
    save("faults", {"cells": rows, "all_rejected": True})
    print(json.dumps({"fault_cells": len(rows), "all_rejected": True}))


def resources():
    rows = []
    profile = json.loads((DOCS / "product-profile.json").read_text())
    for edge in [128, 1024, 2048]:
        path = WORK / f"resource-{edge}.h5"
        shape = (edge, edge)
        with h5py.File(path, "w", libver="earliest") as f:
            f.attrs["crid"] = np.bytes_("PGD0")
            for n in ["y", "x"]:
                axis = f.create_dataset(
                    n,
                    data=np.arange(edge, dtype="f8"),
                    chunks=(min(edge, 256),),
                    compression="gzip",
                )
                axis.make_scale(n)
            for i, (n, dt, block) in enumerate(
                zip(
                    PATHS,
                    DTYPES,
                    [(128, 128), (256, 256), (192, 96), (64, 192), (256, 64)],
                )
            ):
                block = tuple(min(edge, v) for v in block)
                fill = 255 if i == 1 else 4294967295 if i in [2, 4] else -9999.0
                d = f.create_dataset(
                    n,
                    shape=shape,
                    dtype=dt,
                    chunks=block,
                    compression="gzip",
                    shuffle=True,
                    fillvalue=fill,
                )
                d.attrs["_FillValue"] = np.array([fill], dtype=dt)
                for a in range(0, edge, block[0]):
                    for b in range(0, edge, block[1]):
                        d[a : a + block[0], b : b + block[1]] = (
                            0 if i in [1, 2] else 0.25 if i == 3 else 10
                        )
                for j, axis in enumerate(["y", "x"]):
                    d.dims[j].attach_scale(f[axis])
                for key in ["flag_masks", "flag_values", "flag_meanings"]:
                    at = profile["datasets"][n]["attrs"]
                    if key in at:
                        d.attrs[key] = (
                            np.bytes_(at[key])
                            if isinstance(at[key], str)
                            else np.asarray(at[key], dtype=dt)
                        )
        for budget in [4, 8]:
            r = request(path, budget=budget * 1048576)
            r["spatial"].update(start=[0, 0], count=list(shape), step=[1, 1])
            native = run(
                r, POLICIES["cookbook"], f"resource-{edge}-{budget}", mode="discard"
            )
            if native["returncode"] == 0:
                m = native["result"]
                assert (
                    m["summary"]["selected_count"] == edge * edge
                    and m["observed"]["decoded_bytes"] == edge * edge * 17
                )
                assert (
                    m["observed"]["scheduler"]["reserved_bytes_peak"]
                    <= r["budget"]["buffer_bytes"]
                )
                rows.append(
                    {
                        "edge": edge,
                        "budget_mib": budget,
                        "status": "complete",
                        "native": metrics(native, []),
                    }
                )
            else:
                assert "budget" in native["error"] or "reservation" in native["error"]
                rows.append(
                    {
                        "edge": edge,
                        "budget_mib": budget,
                        "status": "admission_rejected",
                        "error": native["error"],
                    }
                )
    save("resources", {"cells": rows})
    print(json.dumps({"resource_cells": len(rows)}))


def benchmark():
    mirror = Mirror(WORK / "mirror")
    mirror.verify()
    rows = []
    h.WORK = WORK / "raw"
    h.WORK.mkdir(parents=True, exist_ok=True)
    for latency in [0, 0.02]:
        with server(mirror, latency=latency) as srv:
            for name in ["window", "dense"]:
                r = request(srv.url, name)
                for repeat in range(3):
                    for metadata in ["exact", "page16k"]:
                        with old.policies(metadata=metadata):
                            for mode in ["wse-only", "multi-raw", "multi-aligned"]:
                                srv.events.clear()
                                label = (
                                    f"bench-{latency}-{name}-{repeat}-{metadata}-{mode}"
                                )
                                if mode == "multi-aligned":
                                    native = run(
                                        r, POLICIES["cookbook"], label, mode="stream"
                                    )
                                    assert native["returncode"] == 0, native.get(
                                        "error"
                                    )
                                    m = metrics(native, list(srv.events))
                                else:
                                    rr = json.loads(json.dumps(r))
                                    if mode == "wse-only":
                                        rr["variables"] = rr["variables"][:1]
                                    native = h.run_native(
                                        rr, label, mode="stream", workers=1
                                    )
                                    assert native["returncode"] == 0, native.get(
                                        "error"
                                    )
                                    result = native["result"]
                                    run_value = result["runs"][0]
                                    m = metrics(
                                        {
                                            **native,
                                            "result": {
                                                "prepare_seconds": result[
                                                    "prepare_seconds"
                                                ],
                                                "first_result_seconds": run_value[
                                                    "first_chunk_seconds"
                                                ],
                                                "completion_seconds": result["seconds"],
                                                "summary": None,
                                                "observed": run_value["observed"],
                                            },
                                        },
                                        list(srv.events),
                                    )
                                rows.append(
                                    {
                                        "reader": "native",
                                        "mode": mode,
                                        "selection": name,
                                        "latency_seconds": latency,
                                        "metadata": metadata,
                                        "repeat": repeat,
                                        **m,
                                    }
                                )
                    for page in [0, 16384]:
                        srv.events.clear()
                        ref, _ = reference(
                            r,
                            POLICIES["cookbook"],
                            mirror.path,
                            f"bench-{latency}-{name}-{repeat}-{page}",
                            url=srv.url,
                            page=page,
                            dump=False,
                        )
                        rows.append(
                            {
                                "reader": "h5py",
                                "mode": "multi-quality",
                                "selection": name,
                                "latency_seconds": latency,
                                "page_bytes": page,
                                "repeat": repeat,
                                **ref,
                            }
                        )
            assert not srv.errors, srv.errors
    for name in ["window", "dense"]:
        for repeat in range(3):
            for reader in ["h5py", "netcdf4"]:
                ref, _ = reference(
                    request(mirror.path, name),
                    POLICIES["cookbook"],
                    mirror.path,
                    f"local-{name}-{repeat}",
                    reader=reader,
                    dump=False,
                )
                rows.append(
                    {
                        "reader": reader,
                        "mode": "multi-quality",
                        "selection": name,
                        "repeat": repeat,
                        **ref,
                    }
                )
            native = run(
                request(mirror.path, name),
                POLICIES["cookbook"],
                f"local-native-{name}-{repeat}",
                mode="stream",
            )
            assert native["returncode"] == 0, native.get("error")
            rows.append(
                {
                    "reader": "native",
                    "mode": "multi-aligned",
                    "access": "local_sparse_reference",
                    "selection": name,
                    "repeat": repeat,
                    **metrics(native, []),
                }
            )
    save(
        "benchmark",
        {
            "cells": rows,
            "repeats": 3,
            "new_provider_bytes": 0,
            "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
        },
    )
    print(json.dumps({"benchmark_cells": len(rows)}))


def access():
    profile = json.loads((DOCS / "product-profile.json").read_text())
    old.URL = profile["url"]
    old.DOCS = DOCS
    h.WORK = WORK / "access"
    h.WORK.mkdir(exist_ok=True)
    old.access()


def compact_receipts():
    # Compact request-level transport ranges to phase counters in versioned
    # receipts; unabridged native stdout and dumped arrays stay in ignored WORK.
    for name in ["science", "benchmark", "resources"]:
        path = DOCS / f"{name}.json"
        if not path.exists():
            continue
        v = json.loads(path.read_text())
        for c in v["cells"]:
            m = c.get("native", c)
            if "observed" in m:
                for source in m["observed"]["transport"].values():
                    if isinstance(source, dict):
                        source.pop("ranges", None)
                        source.pop("objects", None)
        save(name, v)
    # Inherited receipts are machine evidence; retain every field without
    # repeating tens of thousands of indentation-only lines in the change.
    for path in (DOCS / "inherited").rglob("*.json"):
        if path.name == "rust.json":
            continue
        value = json.loads(path.read_text())
        path.write_text(
            json.dumps(value, separators=(",", ":"), allow_nan=False) + "\n"
        )


def integer_admission():
    rows = []
    h.WORK = WORK / "admission"
    h.WORK.mkdir(parents=True, exist_ok=True)
    for dtype, precision, offset in [("u1", 4, 0), ("<u4", 24, 0), (">u4", 28, 4)]:
        path = WORK / f"noncanonical-{np.dtype(dtype).itemsize}-{precision}-{offset}.h5"
        with h5py.File(path, "w", libver="earliest") as f:
            for axis in ["x", "y"]:
                f.create_dataset(axis, data=np.arange(4, dtype="f8"))
            space = h5py.h5s.create_simple((4, 4))
            datatype = h5py.h5t.py_create(np.dtype(dtype))
            datatype.set_precision(precision)
            datatype.set_offset(offset)
            properties = h5py.h5p.create(h5py.h5p.DATASET_CREATE)
            properties.set_chunk((2, 2))
            dataset = h5py.h5d.create(f.id, b"signal", datatype, space, dcpl=properties)
            h5py.Dataset(dataset)[:] = np.arange(16, dtype="u4").reshape(4, 4)
        native = h.run_native(
            h.request(path.resolve(), "/signal", (0, 0), (4, 4)),
            path.stem,
            mode="inspect",
        )
        assert native["returncode"] != 0 and "precision" in native["error"]
        rows.append(
            {
                "dtype": dtype,
                "precision": precision,
                "bit_offset": offset,
                "explicitly_rejected": True,
                "binary_sha256": native["binary_sha256"],
            }
        )
    save("integer-admission", {"cells": rows, "new_provider_bytes": 0})
    print(json.dumps({"noncanonical_integer_rejections": len(rows)}))


def real_resources():
    mirror = Mirror(WORK / "mirror")
    mirror.verify()
    rows = []
    with server(mirror) as srv:
        for budget in [4, 8, 16, 24, 32]:
            r = request(srv.url, budget=budget * 1048576)
            native = run(
                r, POLICIES["cookbook"], f"real-budget-{budget}", mode="stream"
            )
            if budget < 32:
                assert native["returncode"] != 0 and (
                    "budget" in native["error"] or "reservation" in native["error"]
                )
            else:
                assert native["returncode"] == 0, native.get("error")
            rows.append(
                {
                    "budget_mib": budget,
                    "status": "complete"
                    if not native["returncode"]
                    else "admission_rejected",
                    "peak_rss_bytes": native["peak_rss_bytes"],
                    "error": native.get("error"),
                    "scheduler": native.get("result", {})
                    .get("observed", {})
                    .get("scheduler"),
                }
            )
        h.WORK = WORK / "raw"
        r = request(srv.url)
        r["variables"] = [{"kind": "dataset", "path": "/illumination_time"}]
        native = h.run_native(r, "float64-illumination-rejection", mode="inspect")
        assert native["returncode"] != 0 and "supports only" in native["error"]
    save(
        "real-resources",
        {
            "cells": rows,
            "illumination_time_explicitly_rejected": True,
            "new_provider_bytes": 0,
        },
    )
    print(json.dumps({"real_budget_cells": len(rows)}))


def inspect():
    p = json.loads((DOCS / "version-d-candidates.json").read_text())[0]
    old.URL = p["links"][0]
    old.DOCS = DOCS
    old.inspect()


def inherited():
    import swot_wse_axes as axes
    import swot_wse_regressions as gates

    gates.WORK = WORK / "inherited"
    gates.DOCS = DOCS / "inherited"
    gates.WORK.mkdir(parents=True, exist_ok=True)
    gates.DOCS.mkdir(parents=True, exist_ok=True)
    gates.matrix()
    for action in [
        "science",
        "faults",
        "selective-science",
        "selective-faults",
        "selective-extended",
        "selective-extra",
        "resources",
    ]:
        gates.hdf5(action)
    gates.coordinates()
    old.DOCS = DOCS / "inherited/version-c"
    old.DOCS.mkdir(parents=True, exist_ok=True)
    old.science()
    for action in ["science", "faults", "resources"]:
        getattr(axes, action)()


def formats():
    build = [
        [
            "cargo",
            "build",
            "--release",
            "--locked",
            "--no-default-features",
            "--features",
            "native-cog,native-hdf5,experimental-output",
            "--bin",
            "tile-ripper",
            "--example",
            "zarr_subset_oracle",
            "--example",
            "native_cog_oracle",
            "--example",
            "remote_zarr",
            "--example",
            "durable_output_candidate",
            "--target-dir",
            "target/hdf5-regression",
        ],
        [
            "cargo",
            "build",
            "--release",
            "--locked",
            "--manifest-path",
            "consumers/native-stats/Cargo.toml",
            "--no-default-features",
            "--features",
            "native-cog",
            "--target-dir",
            "target/hdf5-regression",
        ],
    ]
    for cmd in build:
        subprocess.run(cmd, cwd=ROOT, check=True)
    rows = []
    replay = (
        (WORK / "inherited" / f"format-replay-{time.time_ns()}")
        .relative_to(ROOT)
        .as_posix()
    )
    for env, actions in [
        (
            "prototype-output/zarr-env/bin/python",
            ["remote", "cf", "cog-stream", "checks", "faults"],
        ),
        (
            "prototype-output/zarr-v3-input/reference-env/bin/python",
            ["v3-checks", "v3-faults"],
        ),
    ]:
        for action in actions:
            code = f"import sys;sys.path.insert(0,'bench');import swot_wse_regressions as r;r.WORK=r.ROOT/{replay!r};r.DOCS=r.ROOT/'docs/swot-quality/inherited';r.inherited({action!r})"
            log = WORK / f"inherited-{action}.log"
            cmd = [str(ROOT / env), "-u", "-c", code]
            with log.open("w") as f:
                p = subprocess.run(
                    cmd, check=False, cwd=ROOT, stdout=f, stderr=subprocess.STDOUT
                )
            rows.append(
                {
                    "action": action,
                    "environment": env,
                    "returncode": p.returncode,
                    "log_sha256": hashlib.sha256(log.read_bytes()).hexdigest(),
                }
            )
            save("format-checks", {"cells": rows})
            assert p.returncode == 0, str(log)
            print("format", action, "PASS", flush=True)


def raw_science():
    mirror = Mirror(WORK / "mirror")
    mirror.verify()
    h.WORK = WORK / "raw"
    h.WORK.mkdir(exist_ok=True)
    rows = []
    with server(mirror) as srv:
        for name in ["window", "boundary", "strided"]:
            r = request(srv.url, name)
            out = h.run_native(r, "raw-science-" + name)
            assert out["returncode"] == 0, out.get("error")
            result = out["result"]
            folder = Path(out["output_dir"])
            count = r["spatial"]["count"]
            steps = r["spatial"]["step"]
            starts = r["spatial"]["start"]
            arrays = [np.empty(count, dtype=d) for d in DTYPES]
            masks = [np.zeros(count, dtype="u1") for _ in PATHS]
            seen = [np.zeros(count, dtype="u1") for _ in PATHS]
            for i, c in enumerate(result["runs"][0]["chunks"]):
                j = PATHS.index(c["variable"]["path"].strip("/"))
                w = c["window"]
                a = [(w["y"] - starts[0]) // steps[0], (w["x"] - starts[1]) // steps[1]]
                sl = (slice(a[0], a[0] + w["height"]), slice(a[1], a[1] + w["width"]))
                shape = (w["height"], w["width"])
                seen[j][sl] += 1
                arrays[j][sl] = np.frombuffer(
                    (folder / f"{i}.bin").read_bytes(), dtype=DTYPES[j]
                ).reshape(shape)
                masks[j][sl] = np.frombuffer(
                    (folder / f"{i}.mask").read_bytes(), dtype="u1"
                ).reshape(shape)
            assert all(np.all(v == 1) for v in seen)
            for reader in ["h5py", "netcdf4"]:
                _, ref = reference(
                    r, POLICIES["raw"], mirror.path, "raw-science-" + name, reader
                )
                for j in range(5):
                    assert (
                        arrays[j].tobytes() == (ref / f"{j}.bin").read_bytes()
                        and masks[j].tobytes() == (ref / f"{j}.mask").read_bytes()
                    )
            assert (
                result["inspection"]["output_estimates"]["native_value_bytes"]
                == int(np.prod(count)) * 17
            )
            rows.append(
                {
                    "selection": name,
                    "all_five_native_values_and_masks_equal": True,
                    "exact_logical_coverage": True,
                    "binary_sha256": out["binary_sha256"],
                }
            )
    save("raw-science", {"cells": rows, "new_provider_bytes": 0})
    print(json.dumps({"raw_multivariable_cells": len(rows)}))


if __name__ == "__main__":
    a = argparse.ArgumentParser()
    a.add_argument("action")
    args = a.parse_args()
    import requests

    try:
        globals()[args.action.replace("-", "_")]()
    except requests.RequestException:
        raise SystemExit(
            "Reference network request failed; credentials and URLs redacted"
        ) from None
