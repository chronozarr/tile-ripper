"""Restricted SWOT WSE qualification. Network capture is explicit and capped.

Run in docs/native-hdf5/reference.txt's independent reference environment.
The native executable has no Python/libhdf5 dependency. Sparse mirror holes are
never served; netCDF4 is a separate local-file reference, not a remote control.
"""

import argparse
import contextlib
import hashlib
import json
import os
import resource
import subprocess
import sys
import time
from urllib.parse import urlsplit

import h5py
import hdf5_input as h
import numpy as np
import requests
from hdf5_reference import Remote
from hdf5_selective_mirror import Mirror, OfficialRanges, server

ROOT = h.ROOT
WORK = ROOT / "prototype-output/swot-wse"
DOCS = ROOT / "docs/swot-wse"
URL = json.loads((ROOT / "docs/hdf5-selective/product-access.json").read_text())[1][
    "url"
]
SELECTIONS = {
    "window": ((600, 400), (128, 128), (1, 1)),
    "boundary": ((780, 780), (30, 30), (1, 1)),
    "strided": ((77, 113), (7, 6), (247, 283)),
}
h.WORK, h.DOCS = WORK, DOCS
WORK.mkdir(parents=True, exist_ok=True)
DOCS.mkdir(parents=True, exist_ok=True)


def save(name, value):
    (DOCS / f"{name}.json").write_text(
        json.dumps(value, indent=2, allow_nan=False, default=str) + "\n"
    )


@contextlib.contextmanager
def policies(index="full", metadata="exact"):
    keys = {
        "TILE_RIPPER_HDF5_INDEX_POLICY": index,
        "TILE_RIPPER_HDF5_METADATA_POLICY": metadata,
    }
    previous = {k: os.environ.get(k) for k in keys}
    os.environ.update(keys)
    try:
        yield
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


def request(locator, name):
    return h.request(locator, "/wse", *SELECTIONS[name])


def metrics(native):
    result = native["result"]
    run = result["runs"][0]
    observed = run["observed"]
    identity = result["inspection"]["sources"][0]["object_identities"]
    binding = identity["datasets"][0]
    transport = observed["transport"]["science"]
    phases = transport.get("phases", {}) if isinstance(transport, dict) else {}
    coordinate = phases.get("hdf5_coordinate_chunks", {})
    index = [v for k, v in phases.items() if k.endswith("index_discovery")]
    return {
        **h.compact(native),
        "prepare_seconds": result["prepare_seconds"],
        "first_result_seconds": run["first_chunk_seconds"],
        "completion_seconds": result["seconds"],
        "valid_pixels": run["valid_pixels"],
        "stored_value_sum": run["sum"],
        "output_bytes": observed["decoded_bytes"],
        "wse_payload_gets": observed["source_data_requests"],
        "wse_payload_bytes": observed["source_data_bytes"],
        "coordinate_chunk_gets": sum(v["attempts"] for v in coordinate.values()),
        "coordinate_chunk_bytes": sum(v["bytes"] for v in coordinate.values()),
        "selected_coordinate_encoded_bytes": sum(
            c["encoded_bytes"] for a in binding["axes"] for c in a["storage"]["chunks"]
        ),
        "coordinate_index_gets": sum(
            v["attempts"]
            for v in phases.get("hdf5_coordinate_index_discovery", {}).values()
        ),
        "coordinate_index_bytes": sum(
            v["bytes"]
            for v in phases.get("hdf5_coordinate_index_discovery", {}).values()
        ),
        "index_nodes_visited": identity["index_nodes_visited"],
        "index_and_metadata_bytes": sum(
            v["bytes"] for phase in index for v in phase.values()
        ),
        "metadata_cache_bytes": identity["metadata_cache_bytes"],
        "scheduler": observed["scheduler"],
        "coordinate_prepare_reserved_bytes_peak": max(
            a["storage"]["prepare_reserved_bytes_peak"] for a in binding["axes"]
        ),
        "transport": transport,
        "source_identity": identity["identity"],
        "chunk_checksums": [
            {k: c[k] for k in ["window", "adler32", "mask_adler32"]}
            for c in run["chunks"]
        ],
        "chunk_coordinate_sha256s": [
            [
                hashlib.sha256(np.asarray(a, dtype="<f8").tobytes()).hexdigest()
                for a in c["coordinates"]
            ]
            for c in run["chunks"]
        ],
    }


def inspect():
    """Fresh capped physical inspection, independently through libhdf5."""

    def clean(value):
        if isinstance(value, bytes):
            return value.decode()
        if isinstance(value, np.ndarray):
            return [clean(v) for v in value.flat]
        if isinstance(value, np.generic):
            return clean(value.item())
        return value if isinstance(value, (str, int, float)) else str(value)

    with OfficialRanges(URL) as source:
        out = {"url": URL, "size": source.size, "etag": source.etag, "datasets": {}}
        with h5py.File(source, "r") as f:
            out["root_attrs"] = {k: clean(v) for k, v in f.attrs.items()}
            for name in [
                "wse",
                "x",
                "y",
                "crs",
                "wse_qual",
                "wse_qual_bitwise",
                "wse_uncert",
                "n_wse_pix",
                "illumination_time",
            ]:
                ds = f[name]
                properties = ds.id.get_create_plist()
                item = {
                    "shape": ds.shape,
                    "dtype": str(ds.dtype),
                    "chunks": ds.chunks,
                    "filters": [
                        str(properties.get_filter(i))
                        for i in range(properties.get_nfilters())
                    ],
                    "attrs": {
                        k: clean(v)
                        for k, v in ds.attrs.items()
                        if k not in ["DIMENSION_LIST", "REFERENCE_LIST"]
                    },
                    "fill": clean(ds.fillvalue),
                    "object_address": h5py.h5o.get_info(ds.id).addr,
                }
                if ds.chunks:
                    item["allocated_chunks"] = [
                        {
                            "offset": c.chunk_offset,
                            "address": c.byte_offset,
                            "size": c.size,
                        }
                        for i in range(ds.id.get_num_chunks())
                        if (c := ds.id.get_chunk_info(i)).byte_offset is not None
                    ]
                out["datasets"][name] = item
        source.validate()
        out.update(events=source.events, bytes=sum(e["bytes"] for e in source.events))
        save("product-profile", out)
        print(
            json.dumps({"inspection_bytes": out["bytes"], "gets": len(source.events)})
        )


class ArchiveSession(requests.Session):
    """Reference tooling only; default Requests/netrc supplies caller credentials.

    Store only response status/host/version facts. Never save redirect URLs,
    Authorization, cookies, netrc contents or exception strings containing URLs.
    HEAD and GET resolve independently from the official canonical URL.
    """

    def __init__(self):
        super().__init__()
        self.events = []

    def head(self, url, **kwargs):
        kwargs["allow_redirects"] = True
        return super().head(url, **kwargs)

    def request(self, method, url, **kwargs):
        response = super().request(method, url, **kwargs)
        self.events.append(
            {
                "method": method,
                "status": response.status_code,
                "redirect_statuses": [r.status_code for r in response.history],
                "redirect_hosts": [urlsplit(r.url).hostname for r in response.history],
                "final_host": urlsplit(response.url).hostname,
                "etag": response.headers.get("ETag"),
                "content_length": response.headers.get("Content-Length"),
                "content_range": response.headers.get("Content-Range"),
                "content_encoding": response.headers.get("Content-Encoding"),
            }
        )
        return response


def capture():
    profile = json.loads((DOCS / "product-profile.json").read_text())
    session = ArchiveSession()
    mirror = Mirror(WORK / "mirror", URL, cap=8 * 1024 * 1024, session=session)
    assert mirror.info["etag"] == profile["etag"]
    assert mirror.info["size"] == profile["size"]
    # Batch previously inspected metadata into bounded 16 KiB pages. Capture
    # all four WSE chunks for the three declared selections, never other arrays.
    extents = [(e["start"], e["start"] + e["bytes"]) for e in profile["events"]]
    for name in ["wse", "x", "y"]:
        extents += [
            (c["address"], c["address"] + c["size"])
            for c in profile["datasets"][name]["allocated_chunks"]
        ]
    ranges = []
    for a, b in sorted(
        (a // 16384 * 16384, min(profile["size"], (b + 16383) // 16384 * 16384))
        for a, b in extents
    ):
        if ranges and a <= ranges[-1][1]:
            ranges[-1] = (ranges[-1][0], max(b, ranges[-1][1]))
        else:
            ranges.append((a, b))
    for a, b in ranges:
        assert b - a <= 4 * 1024 * 1024
        mirror.read(a, b - a)
    with server(mirror) as srv:
        # Read metadata for all variables for the independent NetCDF-C open.
        source = Remote(srv.url, page=16384)
        with h5py.File(source, "r") as f:
            _ = dict(f.attrs)
            for ds in f.values():
                _ = dict(ds.attrs)
            for name in SELECTIONS:
                r = request(srv.url, name)
                selected = slices(r["spatial"])
                _ = f["wse"][selected]
                _ = f["y"][selected[0]]
                _ = f["x"][selected[1]]
        source.close()
        for name in SELECTIONS:
            for metadata in ["exact", "page4k", "page16k"]:
                with policies(metadata=metadata):
                    result = h.run_native(
                        request(srv.url, name), f"capture-{name}-{metadata}"
                    )
                assert result["returncode"] == 0, result["error"]
        assert not srv.errors, srv.errors
    mirror.seal()
    mirror.verify()
    save(
        "capture",
        {
            **mirror.info,
            "archive_events": session.events,
            "note": "Authenticated reference capture; native acquisition is loopback only. Sparse backing file is incomplete.",
        },
    )
    print(
        json.dumps(
            {
                "provider_bytes": mirror.info["provider_bytes"],
                "provider_gets": mirror.info["provider_gets"],
                "size": mirror.info["size"],
            }
        )
    )


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


def science():
    mirror = Mirror(WORK / "mirror")
    mirror.verify()
    rows = []
    with server(mirror) as srv:
        for name in SELECTIONS:
            r = request(srv.url, name)
            for index in ["full", "selective"]:
                for metadata in ["exact", "page4k", "page16k"]:
                    srv.events.clear()
                    with policies(index, metadata):
                        result = h.run_native(r, f"science-{name}-{index}-{metadata}")
                    agreement = h.validate(result, mirror.path, r)
                    binding = result["result"]["inspection"]["sources"][0][
                        "object_identities"
                    ]["datasets"][0]
                    with h5py.File(mirror.path, "r") as f:
                        h.verify_attributes(binding["root_attributes"], f)
                        data = f["wse"][slices(r["spatial"])].astype("<f4")
                        expected = summary(data, f["wse"].fillvalue)
                        expected_hashes = {
                            "value_sha256": hashlib.sha256(data.tobytes()).hexdigest(),
                            "axis_sha256": [
                                hashlib.sha256(
                                    f[path][sl].astype("<f8").tobytes()
                                ).hexdigest()
                                for path, sl in zip(
                                    r["spatial"]["axes"], slices(r["spatial"])
                                )
                            ],
                        }
                        assert (
                            result["result"]["runs"][0]["valid_pixels"]
                            == expected["finite_nonfill_pixels"]
                        )
                        # The real selected values are finite except the CF marker.
                        assert result["result"]["runs"][0]["sum"] == expected["sum_m"]
                        for axis in binding["axes"]:
                            assert len(axis["storage"]["chunks"]) == 1
                            assert (
                                axis["storage"]["prepare_reserved_bytes_peak"]
                                <= r["budget"]["buffer_bytes"]
                            )
                    if not rows:
                        save("native-profile", binding)
                    rows.append(
                        {
                            "selection": name,
                            "index": index,
                            "metadata": metadata,
                            "scientific_summary": expected,
                            "reference_hashes": expected_hashes,
                            "native": metrics(result),
                            "agreement": agreement,
                            "http_events": list(srv.events),
                        }
                    )
            # Independently executed NetCDF-C with masking/scaling disabled.
            worker = {
                "reader": "netcdf4",
                "local": str(mirror.path),
                "spatial": r["spatial"],
            }
            control = reference_worker(worker, f"netcdf-{name}")
            assert control["summary"] == expected
            assert all(control[k] == v for k, v in expected_hashes.items())
            rows[-1]["independent_netcdf4"] = control
        assert not srv.errors, srv.errors
    save(
        "science",
        {
            "cells": rows,
            "binary_sha256": hashlib.sha256(h.BINARY.read_bytes()).hexdigest(),
            "original_source": {k: mirror.info[k] for k in ["url", "etag", "size"]},
            "capture_manifest_sha256": hashlib.sha256(
                mirror.manifest.read_bytes()
            ).hexdigest(),
        },
    )
    print(
        json.dumps(
            {"scientific_cells": len(rows), "independent_netcdf4": len(SELECTIONS)}
        )
    )


def reference_worker(req, name):
    path = WORK / f"reference-{name}.json"
    path.write_text(json.dumps(req))
    start = time.perf_counter()
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    p = subprocess.run(
        [sys.executable, str(ROOT / "bench/swot_wse_reference.py"), str(path)],
        check=True,
        capture_output=True,
        text=True,
        timeout=120,
    )
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    out = json.loads(p.stdout)
    out["process_elapsed_seconds"] = time.perf_counter() - start
    out["process_cpu_seconds"] = (
        after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime
    )
    return out


def access():
    session = ArchiveSession()
    response = session.head(URL, timeout=30)
    response.raise_for_status()
    etag, size = response.headers["ETag"], int(response.headers["Content-Length"])
    head_resolved = response.url
    get_resolved = None
    for match in [etag, '"tile-ripper-deliberately-wrong-version"']:
        with session.get(
            URL,
            headers={
                "If-Match": match,
                "Range": "bytes=0-7",
                "Accept-Encoding": "identity",
            },
            stream=True,
            timeout=30,
        ) as r:
            if match == etag:
                assert r.status_code == 206 and r.headers["ETag"] == etag
                assert r.headers["Content-Range"] == f"bytes 0-7/{size}"
                assert r.raw.read(9) == bytes.fromhex("894844460d0a1a0a")
                get_resolved = r.url
            else:
                assert r.status_code == 412
    with session.get(
        head_resolved,
        headers={"If-Match": etag, "Range": "bytes=0-7"},
        allow_redirects=False,
        stream=True,
        timeout=30,
    ) as response:
        head_route_get_status = response.status_code
    response = session.head(get_resolved, timeout=30)
    get_route_head_status = response.status_code
    # HEAD's resolved resource and GET's resolved resource are intentionally not
    # persisted, and neither is treated as the other's directly usable route.
    response = session.head(URL, headers={"If-Match": etag}, timeout=30)
    assert (
        response.headers["ETag"] == etag
        and int(response.headers["Content-Length"]) == size
    )
    native = h.run_native(
        request(URL, "window"), "official-native-rejection", mode="inspect"
    )
    assert native["returncode"] != 0 and "redirect" in native["error"]
    save(
        "access",
        {
            "url": URL,
            "size": size,
            "etag": etag,
            "events": session.events,
            "reference_authentication": "Caller-owned existing Requests/netrc Earthdata identity; no secrets persisted",
            "native": h.compact(native),
            "native_live_remote_qualified": False,
            "head_resolved_get_status": head_route_get_status,
            "get_resolved_head_status": get_route_head_status,
        },
    )
    print(
        json.dumps(
            {"official_conditional_ranges": True, "native_live_remote_qualified": False}
        )
    )


def benchmark(repeats):
    mirror = Mirror(WORK / "mirror")
    mirror.verify()
    rows = []
    science = json.loads((DOCS / "science.json").read_text())["cells"]
    qualified = {(r["selection"], r["index"], r["metadata"]): r for r in science}
    for latency in [0, 0.02]:
        with server(mirror, latency=latency) as srv:
            for name in SELECTIONS:
                r = request(srv.url, name)
                for repeat in range(repeats):
                    for index in ["full", "selective"]:
                        for metadata in ["exact", "page4k", "page16k"]:
                            srv.events.clear()
                            with policies(index, metadata):
                                native = h.run_native(
                                    r,
                                    f"bench-{latency}-{name}-{repeat}-{index}-{metadata}",
                                    mode="stream",
                                )
                            assert native["returncode"] == 0, native["error"]
                            measured = metrics(native)
                            expected = qualified[(name, index, metadata)]["native"]
                            for key in [
                                "chunk_checksums",
                                "chunk_coordinate_sha256s",
                                "valid_pixels",
                                "stored_value_sum",
                            ]:
                                assert measured[key] == expected[key], key
                            rows.append(
                                {
                                    "latency_seconds": latency,
                                    "selection": name,
                                    "repeat": repeat,
                                    "index": index,
                                    "metadata": metadata,
                                    "reader": "native",
                                    "native": measured,
                                    "http_events": list(srv.events),
                                }
                            )
                    for page in [0, 16384]:
                        control = reference_worker(
                            {
                                "reader": "h5py",
                                "url": srv.url,
                                "spatial": r["spatial"],
                                "page": page,
                            },
                            f"bench-{latency}-{name}-{repeat}-{page}",
                        )
                        expected = qualified[(name, "full", "exact")]
                        assert control["summary"] == expected["scientific_summary"]
                        assert all(
                            control[k] == v
                            for k, v in expected["reference_hashes"].items()
                        )
                        rows.append(
                            {
                                "latency_seconds": latency,
                                "selection": name,
                                "repeat": repeat,
                                **control,
                            }
                        )
            assert not srv.errors, srv.errors
    for name in SELECTIONS:
        for repeat in range(repeats):
            r = request(str(mirror.path), name)
            native = h.run_native(r, f"local-{name}-{repeat}", mode="stream")
            assert native["returncode"] == 0, native["error"]
            rows.append(
                {
                    "selection": name,
                    "repeat": repeat,
                    "reader": "native",
                    "access": "local_sparse_file",
                    "native": metrics(native),
                }
            )
            for reader in ["h5py", "netcdf4"]:
                rows.append(
                    {
                        "selection": name,
                        "repeat": repeat,
                        **reference_worker(
                            {
                                "reader": reader,
                                "local": str(mirror.path),
                                "spatial": r["spatial"],
                            },
                            f"local-{reader}-{name}-{repeat}",
                        ),
                    }
                )
    save(
        "benchmark",
        {
            "repeats": repeats,
            "cells": rows,
            "original_source": {k: mirror.info[k] for k in ["url", "etag", "size"]},
            "capture_manifest_sha256": hashlib.sha256(
                mirror.manifest.read_bytes()
            ).hexdigest(),
            "latency_model": "Added loopback response delay per HEAD/GET, without bandwidth throttling. No provider traffic.",
        },
    )
    print(json.dumps({"benchmark_cells": len(rows)}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "action",
        choices=["inspect", "capture", "science", "access", "benchmark"],
    )
    parser.add_argument("--repeats", type=int, default=3)
    args = parser.parse_args()
    if args.action == "benchmark":
        benchmark(args.repeats)
    else:
        if args.action in ["inspect", "capture", "access"]:
            try:
                globals()[args.action]()
            except Exception as error:  # noqa: BLE001 - redact authenticated URLs at the network CLI boundary
                # Exception URLs can contain ephemeral authentication queries.
                status = getattr(getattr(error, "response", None), "status_code", None)
                raise SystemExit(
                    f"archive operation failed: {type(error).__name__}; HTTP {status}; URLs redacted"
                ) from None
        else:
            globals()[args.action]()
