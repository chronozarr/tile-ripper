"""Selective HDF5 traversal, capped public mirrors, science and timing gates.

Run with the pinned docs/native-hdf5/reference.txt Python environment.
Old milestone receipts stay untouched; all new results use hdf5-selective.
"""

import argparse
import contextlib
import hashlib
import itertools
import json
import os
import resource
import struct
import subprocess
import sys
import time

import h5py
import hdf5_input as h
import numpy as np
from hdf5_reference import Remote
from hdf5_selective_mirror import Mirror, OfficialRanges, server

ROOT = h.ROOT
WORK = ROOT / "prototype-output/hdf5-selective"
DOCS = ROOT / "docs/hdf5-selective"
h.WORK, h.DOCS = WORK, DOCS
WORK.mkdir(parents=True, exist_ok=True)
DOCS.mkdir(parents=True, exist_ok=True)
LARGE_URL = json.loads((DOCS / "goes19-request.json").read_text())["source"]["assets"][
    0
]["locator"]
SELECTIONS = {
    "sparse": ((2500, 2500), (64, 64), (1, 1)),
    "dense": ((2000, 2000), (904, 904), (1, 1)),
    "noncontiguous": ((226, 452), (5, 6), (901, 677)),
}


@contextlib.contextmanager
def policies(index="selective", metadata="exact"):
    keys = {
        "TILE_RIPPER_HDF5_INDEX_POLICY": index,
        "TILE_RIPPER_HDF5_METADATA_POLICY": metadata,
    }
    prior = {k: os.environ.get(k) for k in keys}
    os.environ.update(keys)
    try:
        yield
    finally:
        for k, v in prior.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v


def native(req, name, index="selective", metadata="exact", mode="dump"):
    with policies(index, metadata):
        r = h.run_native(req, name, mode, policy="adjacent")
    assert r["returncode"] == 0, r["error"]
    return r


def reference(req, name, page=0, local=None, full=False):
    r = {
        "url": req["source"]["assets"][0]["locator"],
        "dataset": req["variables"][0]["path"],
        "spatial": req["spatial"],
        "page": page,
        "full": full,
    }
    if local:
        r["local"] = str(local)
    path = WORK / f"reference-{name}.json"
    path.write_text(json.dumps(r))
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    process_started = time.perf_counter()
    p = subprocess.run(
        [sys.executable, str(ROOT / "bench/hdf5_selective_reference.py"), str(path)],
        text=True,
        check=False,
        capture_output=True,
        timeout=120,
    )
    assert p.returncode == 0, p.stderr
    result = json.loads(p.stdout)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    result["api_cpu_seconds"] = result["cpu_seconds"]
    result["cpu_seconds"] = (
        after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime
    )
    result["process_elapsed_seconds"] = time.perf_counter() - process_started
    result["cpu_scope"] = (
        "whole child process, including interpreter startup and imports"
    )
    return result


def metrics(r):
    out = r["result"]
    obs = out["runs"][0]["observed"]
    transport = obs["transport"]["science"]
    phases = transport.get("phases", {})
    identity = out["inspection"]["sources"][0]["object_identities"]
    index_phase = phases.get("hdf5_chunk_index_discovery", {})
    index = {
        "gets": sum(
            v["attempts"] for k, v in index_phase.items() if k.startswith("GET ")
        ),
        "bytes": sum(v["bytes"] for v in index_phase.values()),
    }
    return {
        "prepare_seconds": out["prepare_seconds"],
        "first_chunk_seconds": out["runs"][0]["first_chunk_seconds"],
        "completion_seconds": out["seconds"],
        "cpu_seconds": r["child_cpu_seconds"],
        "peak_rss_bytes": r["peak_rss_bytes"],
        "process_elapsed_seconds": r["elapsed_seconds"],
        "binary_sha256": r["binary_sha256"],
        "index_nodes_visited": identity["index_nodes_visited"],
        "index_phase": index,
        "metadata_gets": sum(
            v["attempts"]
            for phase, calls in phases.items()
            if phase != "execution"
            for k, v in calls.items()
            if k.startswith("GET ")
        ),
        "metadata_bytes": sum(
            v["bytes"]
            for phase, calls in phases.items()
            if phase != "execution"
            for k, v in calls.items()
            if k.startswith("GET ")
        ),
        "payload_gets": obs["source_data_requests"],
        "payload_bytes": obs["source_data_bytes"],
        "scheduler": obs["scheduler"],
        "transport": transport,
        "stored_value_sum": out["runs"][0]["sum"],
        "valid_pixels": out["runs"][0]["valid_pixels"],
    }


def capture():
    mirror = Mirror(WORK / "large-mirror", LARGE_URL)
    cells = []
    # Page tuning may fetch extra neighboring bytes, always within the transfer cap.
    with server(mirror) as s:
        for label, selection in SELECTIONS.items():
            req = h.request(s.url, "/TEMP", *selection)
            for idx, page in [
                ("full", "exact"),
                ("full", "page16k"),
                ("selective", "exact"),
            ]:
                result = native(req, f"capture-{label}-{idx}-{page}", idx, page)
                cells.append(
                    {
                        "selection": label,
                        "index": idx,
                        "metadata": page,
                        **metrics(result),
                    }
                )
            reference(req, "capture-" + label, page=65536)
        # Capture every root dataset header, attribute and scalar for NetCDF-C's
        # independent local opening of the sparse backing file. No science arrays
        # outside the declared selections are requested.
        r = Remote(s.url, page=65536)
        with h5py.File(r, "r") as f:

            def visit(name, obj):
                _ = dict(obj.attrs)
                if isinstance(obj, h5py.Dataset) and obj.ndim == 0:
                    _ = obj[()]

            f.visititems(visit)
        r.close()
        assert not s.errors, s.errors
    mirror.seal()
    mirror.verify()
    meta = {}
    with h5py.File(mirror.path, "r") as f:
        ds = f["/TEMP"]
        meta = {
            "shape": ds.shape,
            "dtype": str(ds.dtype),
            "chunks": ds.chunks,
            "allocated_chunks": ds.id.get_num_chunks(),
            "filters": [
                str(ds.id.get_create_plist().get_filter(i))
                for i in range(ds.id.get_create_plist().get_nfilters())
            ],
        }
    # Independent exact comparisons, including NetCDF-C, use only captured data.
    validation = []
    with server(mirror) as s:
        for label, selection in SELECTIONS.items():
            req = h.request(s.url, "/TEMP", *selection)
            result = native(req, "large-science-" + label)
            validation.append(
                {"selection": label, **h.validate(result, mirror.path, req)}
            )
        assert not s.errors, s.errors
    h.save(
        "large-file",
        {
            "mirror": mirror.info,
            "dataset": meta,
            "capture": cells,
            "science": validation,
        },
    )
    print(
        "CAPTURE PASS",
        mirror.info["provider_bytes"],
        "of",
        mirror.info["size"],
        flush=True,
    )


def live():
    mirror = Mirror(WORK / "large-mirror")
    rows = []
    for policy in ["full", "selective"]:
        req = h.request(LARGE_URL, "/TEMP", *SELECTIONS["sparse"])
        result = native(req, "live-" + policy, policy)
        rows.append(
            {
                "policy": policy,
                **metrics(result),
                **h.validate(result, mirror.path, req),
            }
        )
    h.save("live", rows)
    print("LIVE PASS", flush=True)


def benchmark(repeats):
    mirror = Mirror(WORK / "large-mirror")
    mirror.verify()
    cells = []
    for latency in [0, 0.02, 0.08]:
        with server(mirror, latency=latency) as s:
            for label, selection in SELECTIONS.items():
                req = h.request(s.url, "/TEMP", *selection)
                expected = reference(req, f"expected-{label}", local=mirror.path)
                for repeat in range(repeats):
                    for idx, page in [
                        ("full", "exact"),
                        ("selective", "exact"),
                        ("full", "page4k"),
                        ("selective", "page4k"),
                        ("selective", "page16k"),
                    ]:
                        name = f"{latency}-{label}-{repeat}-{idx}-{page}"
                        r = native(req, name, idx, page)
                        m = metrics(r)
                        assert m["stored_value_sum"] == expected["stored_value_sum"]
                        assert m["valid_pixels"] == expected["valid_pixels"]
                        h.validate(r, mirror.path, req)
                        cells.append(
                            {
                                "selection": label,
                                "latency": latency,
                                "repeat": repeat,
                                "reader": "native",
                                "index": idx,
                                "metadata": page,
                                **m,
                            }
                        )
                    for page in [0, 16384, 65536]:
                        r = reference(req, f"{latency}-{label}-{repeat}-{page}", page)
                        assert r["value_sha256"] == expected["value_sha256"]
                        cells.append(
                            {
                                "selection": label,
                                "latency": latency,
                                "repeat": repeat,
                                **r,
                            }
                        )
                    h.save(
                        "benchmark",
                        {
                            "latency_model": "added response delay per HEAD/GET on loopback; no bandwidth throttle",
                            "cells": cells,
                        },
                    )
                    print("BENCH PASS", latency, label, repeat, flush=True)
            assert not s.errors, s.errors
    # Local file I/O, no HTTP, with the same selected data.
    local = []
    for label, selection in SELECTIONS.items():
        req = h.request(mirror.path, "/TEMP", *selection)
        for idx in ["full", "selective"]:
            r = native(req, "local-" + label + idx, idx)
            h.validate(r, mirror.path, req)
            local.append(
                {"selection": label, "reader": "native", "index": idx, **metrics(r)}
            )
        local.append(
            {"selection": label, **reference(req, "local-" + label, local=mirror.path)}
        )
    h.save("local", local)


def small(repeats):
    path = ROOT / "prototype-output/native-hdf5/goes19-reference-control.nc"
    assert path.stat().st_size == 1268526
    assert (
        hashlib.md5(path.read_bytes()).hexdigest() == "343acd4029c38ee0adb8bd41394359a4"
    )
    cells = []
    for latency in [0, 0.02, 0.08]:
        with h.server(path, "/RRQPE", latency=latency) as s:
            req = h.request(s.url, "/RRQPE", (2500, 2500), (64, 64))
            expected = reference(req, "small-local", local=path)
            for repeat in range(repeats):
                for idx, page in [
                    ("full", "exact"),
                    ("selective", "exact"),
                    ("selective", "page16k"),
                ]:
                    r = native(req, f"small-{latency}-{repeat}-{idx}-{page}", idx, page)
                    h.validate(r, path, req)
                    cells.append(
                        {
                            "latency": latency,
                            "repeat": repeat,
                            "reader": "native",
                            "index": idx,
                            "metadata": page,
                            **metrics(r),
                        }
                    )
                for page, full in [(0, False), (65536, False), (0, True)]:
                    r = reference(
                        req, f"small-{latency}-{repeat}-{page}-{full}", page, full=full
                    )
                    assert r["value_sha256"] == expected["value_sha256"]
                    cells.append({"latency": latency, "repeat": repeat, **r})
                h.save("small", {"source_bytes": path.stat().st_size, "cells": cells})
                print("SMALL PASS", latency, repeat, flush=True)


def baseline():
    mirror = Mirror(WORK / "large-mirror")
    current = h.BINARY
    h.BINARY = WORK / "baseline/hdf5_subset_oracle"
    rows = []
    try:
        with server(mirror) as s:
            for label, selection in SELECTIONS.items():
                req = h.request(s.url, "/TEMP", *selection)
                r = native(req, "baseline-fe58a2c-" + label, "full")
                check = h.validate(r, mirror.path, req)
                rows.append(
                    {
                        "selection": label,
                        "reader": "unmodified fe58a2c",
                        **h.compact(r),
                        **check,
                        "receipt": r["result"],
                    }
                )
    finally:
        h.BINARY = current
    h.save("baseline", rows)


def resources():
    import shutil

    import hdf5_resources

    # Re-run inherited working-memory, bomb and cancellation gates with the
    # original complete-index control. Preserve its full-scan equality check.
    shutil.copyfile(
        ROOT / "prototype-output/native-hdf5/goes19-reference-control.nc",
        WORK / "goes19-reference-control.nc",
    )
    with policies("full"):
        hdf5_resources.main()
    rows = []
    for edge in [512, 4096]:
        path, ds = h.fixture(
            "selective-memory-" + str(edge),
            shape=(edge, edge),
            chunks=(256, 256),
            holes=False,
        )
        for budget in [4 * 1024 * 1024, 8 * 1024 * 1024]:
            req = h.request(path, ds, (0, 0), (edge, edge), budget=budget)
            with policies():
                r = h.run_native(
                    req, f"selective-memory-{edge}-{budget}", "discard", "adjacent"
                )
            assert r["returncode"] == 0, r["error"]
            obs = r["result"]["runs"][0]["observed"]
            assert obs["decoded_bytes"] == edge * edge * 2
            assert obs["scheduler"]["reserved_bytes_peak"] <= budget
            rows.append(
                {
                    "shape": [edge, edge],
                    "budget": budget,
                    **h.compact(r),
                    "scheduler": obs["scheduler"],
                }
            )
    for budget in [4 * 1024 * 1024, 8 * 1024 * 1024]:
        a, b = [x for x in rows if x["budget"] == budget]
        assert b["peak_rss_bytes"] - a["peak_rss_bytes"] < 32 * 1024 * 1024
    h.save("selective-resources", rows)


def extra():
    path, ds = h.fixture("page-faults", fletcher=True)
    faults = []
    for page in ["page4k", "page16k"]:
        for fault in ["no-range", "bad-range", "encoded", "changed-etag", "exhaust"]:
            with h.server(path, ds, fault=fault, fault_scope="all") as s:
                req = h.request(s.url, ds)
                with policies(metadata=page):
                    r = h.run_native(req, "page-fault-" + page + fault, "stream")
                assert r["returncode"] != 0
                faults.append({"metadata": page, "fault": fault, **h.compact(r)})
        for mode in ["before", "during", "cancel-inflight"]:
            with h.server(path, ds, latency=0.002) as s:
                req = h.request(s.url, ds, (0, 0), (128, 192))
                with policies(metadata=page):
                    r = h.run_native(
                        req,
                        "page-lifecycle-" + page + mode,
                        mode,
                        control=s.url.replace(
                            "/data.nc",
                            "/control/wait-payload"
                            if mode == "cancel-inflight"
                            else "/control",
                        ),
                    )
                assert r["returncode"] != 0
                faults.append({"metadata": page, "fault": mode, **h.compact(r)})
    path = WORK / "all-unallocated.nc"
    with h5py.File(path, "w", libver="earliest") as f:
        f.create_dataset("y", data=np.arange(128, dtype="f8"))
        f.create_dataset("x", data=np.arange(256, dtype="f8"))
        d = f.create_dataset(
            "signal", shape=(128, 256), dtype="u2", chunks=(16, 16), fillvalue=7
        )
        d.attrs["_FillValue"] = np.uint16(65535)
    fills = []
    with h.server(path, "/signal") as s:
        req = h.request(s.url, "/signal", (1, 2), (20, 30), (3, 7))
        for idx in ["full", "selective"]:
            for page in ["exact", "page4k", "page16k"]:
                r = native(req, "all-unallocated-" + idx + page, idx, page)
                check = h.validate(r, path, req)
                assert metrics(r)["index_nodes_visited"] == 0
                assert metrics(r)["payload_gets"] == 0
                assert r["result"]["runs"][0]["valid_pixels"] == 600
                fills.append({"index": idx, "metadata": page, **check})
        for idx, page in [("unknown", "exact"), ("selective", "unknown")]:
            with policies(idx, page):
                r = h.run_native(req, "invalid-policy-" + idx + page, "stream")
            assert r["returncode"] != 0
            faults.append(
                {
                    "fault": "invalid-policy",
                    "index": idx,
                    "metadata": page,
                    **h.compact(r),
                }
            )
        for page in ["page4k", "page16k"]:
            req["budget"]["buffer_bytes"] = 65536
            with policies(metadata=page):
                r = h.run_native(req, "small-metadata-budget-" + page, "stream")
            assert r["returncode"] != 0
            faults.append(
                {"fault": "small-metadata-budget", "metadata": page, **h.compact(r)}
            )
    h.save("extra", {"science": fills, "faults": faults})
    print("EXTRA PASS", len(fills), len(faults), flush=True)


def regressions(action):
    import hdf5_regressions as g

    g.WORK = WORK / "inherited"
    g.DOCS = DOCS / "inherited"
    g.WORK.mkdir(parents=True, exist_ok=True)
    g.DOCS.mkdir(parents=True, exist_ok=True)
    g.main(action)


def products():
    import ctypes

    get_index = ctypes.CDLL(h5py.h5d.__file__).H5Dget_chunk_index_type
    get_index.argtypes = [ctypes.c_int64, ctypes.POINTER(ctypes.c_int)]
    get_index.restype = ctypes.c_int
    rows = json.loads((DOCS / "product-access.json").read_text())
    for row in rows:
        source = OfficialRanges(row["url"])
        try:
            with h5py.File(source, "r") as f:
                for path in row["datasets"]:
                    ds = f[path]
                    item = row["datasets"][path]
                    item.update(
                        shape=ds.shape,
                        dtype=str(ds.dtype),
                        chunks=ds.chunks,
                        layout_enum=ds.id.get_create_plist().get_layout(),
                        filters=[
                            str(ds.id.get_create_plist().get_filter(i))
                            for i in range(ds.id.get_create_plist().get_nfilters())
                        ],
                        fill_value_defined_enum=ds.id.get_create_plist().fill_value_defined(),
                        fill_value=str(ds.fillvalue),
                        attribute_names=list(ds.attrs),
                    )
                    if ds.chunks:
                        index = ctypes.c_int(-1)
                        assert get_index(ds.id.id, ctypes.byref(index)) == 0
                        item["chunk_index_enum"] = index.value
                        item["allocated_chunks"] = ds.id.get_num_chunks()
                    for name in [
                        "_FillValue",
                        "missing_value",
                        "grid_mapping",
                        "units",
                    ]:
                        if name in ds.attrs:
                            item[name] = str(ds.attrs[name])
            source.validate()
            row.update(
                inspected_storage=True,
                inspection_gets=len(source.events),
                inspection_bytes=sum(e["bytes"] for e in source.events),
                inspection_events=source.events,
                inspection_cap=source.cap,
                source_revalidated=True,
            )
            row.pop("inspection_error", None)
            row.pop("earthdata_netrc_available", None)
            row["authentication"] = (
                "existing Earthdata identity"
                if row.get("resolved_get_identity_present")
                else "public sample GET route"
            )
        finally:
            source.close()
        print(
            "PRODUCT PASS",
            row["inspection_bytes"],
            row["url"].split("/")[2],
            flush=True,
        )
    h.save("product-access", rows)


def coordinates():
    """Check stored axes independently through NetCDF-C, including GOES packing."""
    import netCDF4

    cases = []
    for c in json.loads((DOCS / "science.json").read_text())["cells"]:
        stem = c["name"].split("-(", 1)[0]
        cases.append((c["name"], WORK / (stem + ".nc")))
    for c in json.loads((DOCS / "extended.json").read_text())["science"]:
        if c["selection"] == "backwards-payload":
            # These two receipts are stored in full/selected order.
            idx = (
                "full"
                if not any(n == "backwards-full" for n, _ in cases)
                else "selective"
            )
            cases.append(("backwards-" + idx, WORK / "backwards-payload.nc"))
        else:
            chunks = tuple(c["chunk_shape"])
            cases.append(
                (
                    f"extended-{chunks}-{c['selection']}-{c['index']}",
                    WORK / f"geometry-{chunks}.nc",
                )
            )
    for c in json.loads((DOCS / "extra.json").read_text())["science"]:
        cases.append(
            (
                "all-unallocated-" + c["index"] + c["metadata"],
                WORK / "all-unallocated.nc",
            )
        )
    for label in SELECTIONS:
        cases.append((f"0-{label}-0-selective-exact", WORK / "large-mirror/source.nc"))
    rows = []
    for name, source in cases:
        directory = WORK / "runs" / name
        req = json.loads((directory / "request.json").read_text())
        output = json.loads((directory / "stdout.json").read_text())
        count = 0
        with netCDF4.Dataset(source) as f:
            for c in output["runs"][0]["chunks"]:
                w = c["window"]
                for dim, (start, length, actual) in enumerate(
                    [
                        (w["y"], w["height"], c["coordinates"][1]),
                        (w["x"], w["width"], c["coordinates"][0]),
                    ]
                ):
                    axis = f[req["spatial"]["axes"][dim]]
                    axis.set_auto_maskandscale(False)
                    step = req["spatial"]["step"][dim]
                    expected = axis[slice(start, start + length * step, step)].astype(
                        "f8"
                    )
                    assert np.array_equal(actual, expected), name
                    count += len(expected)
        rows.append(
            {
                "case": name,
                "stored_axis_samples_equal": count,
                "netcdf4_equal": True,
                "automatic_packing_disabled": True,
            }
        )
    assert len(rows) == 123, len(rows)
    h.save(
        "netcdf-coordinates",
        {"cases": rows, "all_equal": True, "netCDF4": netCDF4.__version__},
    )
    print("NETCDF AXES PASS", len(rows), flush=True)


def tuning(repeats):
    import shutil

    base = Mirror(WORK / "large-mirror")
    folder = WORK / "tuning-mirror"
    folder.mkdir(exist_ok=True)
    for name in ["source.nc", "manifest.json"]:
        if not (folder / name).exists():
            shutil.copyfile(base.directory / name, folder / name)
    expanded = Mirror(folder, LARGE_URL)
    initial_bytes = expanded.info["provider_bytes"]
    failures = []
    with server(expanded) as s:
        for label, selection in SELECTIONS.items():
            req = h.request(s.url, "/TEMP", *selection)
            try:
                reference(req, "capture-256k-" + label, page=262144)
            except AssertionError as error:
                failures.append(
                    {
                        "selection": label,
                        "error": str(error)[-1000:],
                        "server_errors": s.errors[:],
                    }
                )
    expanded.seal()
    expanded.verify()
    rows = []
    for latency in [0, 0.02, 0.08]:
        with (
            server(base, latency=latency) as standard,
            server(expanded, latency=latency) as broad,
        ):
            for label, selection in SELECTIONS.items():
                req = h.request(standard.url, "/TEMP", *selection)
                expected = reference(req, "tuning-local-" + label, local=base.path)
                for repeat in range(repeats):
                    r = native(
                        req, f"full16k-{latency}-{label}-{repeat}", "full", "page16k"
                    )
                    h.validate(r, base.path, req)
                    rows.append(
                        {
                            "selection": label,
                            "latency": latency,
                            "repeat": repeat,
                            "reader": "native",
                            "index": "full",
                            "metadata": "page16k",
                            **metrics(r),
                        }
                    )
                    if not any(f["selection"] == label for f in failures):
                        req2 = h.request(broad.url, "/TEMP", *selection)
                        r = reference(
                            req2, f"256k-{latency}-{label}-{repeat}", page=262144
                        )
                        assert r["value_sha256"] == expected["value_sha256"]
                        rows.append(
                            {
                                "selection": label,
                                "latency": latency,
                                "repeat": repeat,
                                **r,
                            }
                        )
                    h.save(
                        "tuning",
                        {
                            "initial_mirror_bytes": initial_bytes,
                            "additional_provider_bytes": expanded.info["provider_bytes"]
                            - initial_bytes,
                            "mirror": expanded.info,
                            "failures": failures,
                            "cells": rows,
                        },
                    )
                    print("TUNING PASS", latency, label, repeat, flush=True)


def inherited_hdf5(action):
    with policies():
        getattr(h, action)()


def tree_nodes(path):
    """Test-only inspection of h5py's legacy nodes for targeted corruptions."""
    import re

    raw = path.read_bytes()
    width = raw[13]
    assert width == 8
    nodes = {}
    for match in re.finditer(b"TREE\x01", raw):
        a = match.start()
        level, count = raw[a + 5], struct.unpack_from("<H", raw, a + 6)[0]
        if count > 1024 or a + 24 + (count + 1) * 32 + count * 8 > len(raw):
            continue
        keys = [
            struct.unpack_from("<QQQ", raw, a + 24 + i * 40 + 8)
            for i in range(count + 1)
        ]
        children = [
            struct.unpack_from("<Q", raw, a + 24 + i * 40 + 32)[0] for i in range(count)
        ]
        nodes[a] = {"level": level, "keys": keys, "children": children}
    root = max(nodes, key=lambda a: nodes[a]["level"])
    return raw, nodes, root


def extended():
    science = []
    for chunks in [(17, 29), (128, 16), (7, 97), (31, 47)]:
        path, ds = h.fixture(f"geometry-{chunks}", shape=(385, 513), chunks=chunks)
        selections = [
            ("first", (0, 0), (1, 1), (1, 1)),
            ("last", (384, 512), (1, 1), (1, 1)),
            ("intermediate", (191, 255), (9, 11), (1, 1)),
            (
                "boundary",
                (chunks[0] - 1, chunks[1] - 1),
                (chunks[0] + 2, chunks[1] + 2),
                (1, 1),
            ),
            ("sparse-strided", (1, 2), (5, 6), (80, 100)),
            ("dense-full", (0, 0), (385, 513), (1, 1)),
        ]
        with h.server(path, ds) as s:
            for name, start, count, step in selections:
                req = h.request(s.url, ds, start, count, step)
                results = []
                for idx in ["full", "selective"]:
                    r = native(req, f"extended-{chunks}-{name}-{idx}", idx)
                    check = h.validate(r, path, req)
                    results.append(r)
                    science.append(
                        {
                            "chunk_shape": chunks,
                            "selection": name,
                            "index": idx,
                            **check,
                            **metrics(r),
                        }
                    )
                # Identical logical and physical chunk sets and exact checksums.
                assert (
                    results[0]["result"]["runs"][0]["chunks"]
                    == results[1]["result"]["runs"][0]["chunks"]
                )
                if name == "dense-full":
                    assert (
                        metrics(results[0])["index_nodes_visited"]
                        == metrics(results[1])["index_nodes_visited"]
                    )
                print("GEOMETRY PASS", chunks, name, flush=True)
    # A different physical allocation order must not alter logical key ordering.
    path = WORK / "backwards-payload.nc"
    rng = np.random.default_rng(52)
    values = rng.integers(0, 50000, (513, 769), dtype="u2")
    with h5py.File(path, "w", libver="earliest") as f:
        f.create_dataset("x", data=np.arange(769, dtype="f8"))
        f.create_dataset("y", data=np.arange(513, dtype="f8"))
        ds = f.create_dataset(
            "signal",
            shape=values.shape,
            dtype="u2",
            chunks=(31, 47),
            compression="gzip",
            fillvalue=65535,
        )
        ds.attrs["_FillValue"] = np.uint16(65535)
        for row in reversed(range(0, 513, 31)):
            for col in reversed(range(0, 769, 47)):
                ds[row : row + 31, col : col + 47] = values[
                    row : row + 31, col : col + 47
                ]
        addresses = [
            ds.id.get_chunk_info(i).byte_offset for i in range(ds.id.get_num_chunks())
        ]
        assert any(b < a for a, b in itertools.pairwise(addresses))
    with h.server(path, "/signal") as s:
        req = h.request(s.url, "/signal", (1, 2), (8, 9), (64, 80))
        results = [
            native(req, "backwards-" + idx, idx) for idx in ["full", "selective"]
        ]
        for r in results:
            science.append(
                {
                    "selection": "backwards-payload",
                    **h.validate(r, path, req),
                    **metrics(r),
                }
            )
        assert (
            results[0]["result"]["runs"][0]["chunks"]
            == results[1]["result"]["runs"][0]["chunks"]
        )
    # Mutations are produced independently from the Rust parser. No parser lives
    # in Tile-Ripper's production adapter.
    path, ds = h.fixture("index-faults", shape=(1025, 1537), chunks=(31, 47))
    raw, nodes, root = tree_nodes(path)
    assert nodes[root]["level"] > 0
    first_child = nodes[root]["children"][0]
    last_child = nodes[root]["children"][-1]
    faults = []
    mutations = {
        "undefined-child": (root + 56, struct.pack("<Q", 2**64 - 1)),
        "cycle-child": (root + 56, struct.pack("<Q", root)),
        "outside-child": (root + 56, struct.pack("<Q", len(raw) + 16)),
        "truncated-child-header": (root + 56, struct.pack("<Q", len(raw) - 8)),
        "invalid-record-count": (root + 6, b"\xff\xff"),
        "unordered-keys": (root + 24 + 40 + 8, b"\0" * 24),
        "unaligned-key": (root + 24 + 8, struct.pack("<Q", 1)),
        "wrong-child-level": (first_child + 5, bytes([nodes[root]["level"]])),
        "wrong-child-boundary": (first_child + 24 + 8, struct.pack("<Q", 31)),
        "visited-bad-signature": (first_child, b"XXXX"),
    }
    for name, (a, value) in mutations.items():
        damaged = bytearray(raw)
        damaged[a : a + len(value)] = value
        with h.server(path, ds) as s:
            s.raw = bytes(damaged)
            s.etag = '"' + hashlib.sha256(s.raw).hexdigest() + '"'
            req = h.request(s.url, ds, (0, 0), (1, 1))
            with policies():
                r = h.run_native(req, "index-fault-" + name, "stream")
            assert r["returncode"] != 0, name
            faults.append({"name": name, **h.compact(r), "error": r["error"]})
    # Reading selected branches cannot validate the bytes of skipped branches.
    damaged = bytearray(raw)
    damaged[last_child : last_child + 4] = b"XXXX"
    with h.server(path, ds) as s:
        s.raw = bytes(damaged)
        s.etag = '"' + hashlib.sha256(s.raw).hexdigest() + '"'
        for idx, expected in [("full", False), ("selective", True)]:
            req = h.request(s.url, ds, (0, 0), (1, 1))
            with policies(idx):
                r = h.run_native(req, "unread-corruption-" + idx, "stream")
            assert (r["returncode"] == 0) == expected
            faults.append(
                {
                    "name": "unread-corruption-" + idx,
                    "expected_success": expected,
                    **h.compact(r),
                }
            )
        # Selecting the damaged subtree must fail.
        req = h.request(s.url, ds, (1024, 1536), (1, 1))
        with policies():
            r = h.run_native(req, "select-damaged-subtree", "stream")
        assert r["returncode"] != 0
        faults.append({"name": "select-damaged-subtree", **h.compact(r)})
    h.save(
        "extended",
        {
            "science": science,
            "faults": faults,
            "scope": "unread-subtree corruption is detectable only when traversed",
        },
    )
    print("EXTENDED PASS", len(science), len(faults), flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "action",
        choices=[
            "capture",
            "live",
            "benchmark",
            "science",
            "faults",
            "extended",
            "small",
            "baseline",
            "resources",
            "regressions",
            "extra",
            "tuning",
            "products",
            "coordinates",
        ],
    )
    parser.add_argument(
        "--gate",
        choices=[
            "remote",
            "cf",
            "cog-stream",
            "checks",
            "faults",
            "v3-checks",
            "v3-faults",
        ],
    )
    parser.add_argument("--repeats", type=int, default=3)
    args = parser.parse_args()
    if args.action in ["science", "faults"]:
        inherited_hdf5(args.action)
    elif args.action in ["benchmark", "small", "tuning"]:
        globals()[args.action](args.repeats)
    elif args.action == "regressions":
        assert args.gate
        regressions(args.gate)
    else:
        globals()[args.action]()


if __name__ == "__main__":
    main()
