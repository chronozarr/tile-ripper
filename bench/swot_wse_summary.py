"""Derive acquisition, performance and verification tables from SWOT receipts."""

import collections
import hashlib
import json
import statistics
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs/swot-wse"
WORK = ROOT / "prototype-output/swot-wse"


def read(name):
    return json.loads((DOCS / f"{name}.json").read_text())


def save(name, value):
    (DOCS / f"{name}.json").write_text(
        json.dumps(value, indent=2, allow_nan=False) + "\n"
    )


def union(ranges):
    out = []
    for a, b in sorted(ranges):
        if out and a <= out[-1][1]:
            out[-1] = (out[-1][0], max(b, out[-1][1]))
        else:
            out.append((a, b))
    return out


def length(ranges):
    return sum(b - a for a, b in union(ranges))


def io_counts(events, reference=False, baseline=None):
    ranges = (
        events
        if reference
        else [
            (e["start"], e["end"])
            for e in events
            if e["method"] == "GET" and e["status"] == 206
        ]
    )
    total = sum(b - a for a, b in ranges)
    out = {
        "gets": len(ranges),
        "heads": 2 if reference else sum(e["method"] == "HEAD" for e in events),
        "response_bytes": total,
        "unique_response_bytes": length(ranges),
        "duplicate_response_bytes": total - length(ranges),
        "repeated_exact_gets": sum(
            n - 1 for n in collections.Counter(map(tuple, ranges)).values()
        ),
        "largest_range_bytes": max((b - a for a, b in ranges), default=0),
    }
    if baseline is not None:
        current = union(ranges)
        expected = union(baseline)
        overlap = sum(
            max(0, min(b, d) - max(a, c)) for a, b in current for c, d in expected
        )
        out["unique_extra_bytes_vs_same_reader_exact"] = length(current) - overlap
    return out


def main():
    science = read("science")
    profile = read("product-profile")
    benchmark = read("benchmark")
    baseline = {
        r["selection"]: [
            (e["start"], e["end"]) for e in r["http_events"] if e["method"] == "GET"
        ]
        for r in science["cells"]
        if r["index"] == "full" and r["metadata"] == "exact"
    }
    physical = []
    for row in science["cells"]:
        m = row["native"]
        counts = io_counts(row["http_events"], baseline=baseline[row["selection"]])
        assert counts["response_bytes"] == m["transport"]["bytes"]
        assert counts["gets"] + counts["heads"] == m["transport"]["requests"]
        assert m["coordinate_chunk_bytes"] == 1728
        physical.append(
            {
                "selection": row["selection"],
                "index": row["index"],
                "metadata": row["metadata"],
                **counts,
                "original_file_bytes": profile["size"],
                "fraction_of_file": counts["response_bytes"] / profile["size"],
                "requested_wse_samples": row["scientific_summary"]["requested_pixels"],
                "wse_output_bytes": m["output_bytes"],
                "wse_payload_bytes": m["wse_payload_bytes"],
                "coordinate_chunk_bytes": m["coordinate_chunk_bytes"],
                "metadata_index_and_page_bytes": counts["response_bytes"]
                - m["wse_payload_bytes"]
                - m["coordinate_chunk_bytes"],
                "coordinate_index_bytes": m["coordinate_index_bytes"],
                "index_nodes_visited": m["index_nodes_visited"],
                "coordinate_reserved_bytes_peak": m[
                    "coordinate_prepare_reserved_bytes_peak"
                ],
                "stream_reserved_bytes_peak": m["scheduler"]["reserved_bytes_peak"],
            }
        )
    save("acquisition", physical)
    groups = collections.defaultdict(list)
    ref_baseline = {
        r["selection"]: r["events"]
        for r in benchmark["cells"]
        if r["reader"] == "h5py" and r.get("page_bytes") == 0 and "events" in r
    }
    for row in benchmark["cells"]:
        access = row.get("access", "loopback_conditional_ranges")
        native = row["reader"] == "native"
        policy = (
            (row.get("index", "full") + "/" + row.get("metadata", "exact"))
            if native
            else str(row.get("page_bytes", 0))
        )
        key = (
            access,
            row.get("latency_seconds", 0),
            row["selection"],
            row["reader"],
            policy,
        )
        m = row["native"] if native else row
        values = {
            k: m[k]
            for k in [
                "prepare_seconds",
                "first_result_seconds",
                "completion_seconds",
                "peak_rss_bytes",
            ]
        }
        values["cpu_seconds"] = (
            m["child_cpu_seconds"] if native else m["process_cpu_seconds"]
        )
        if not native:
            values["api_cpu_seconds"] = m["api_cpu_seconds"]
        if native:
            values["reserved_bytes_peak"] = max(
                m["scheduler"]["reserved_bytes_peak"],
                m["coordinate_prepare_reserved_bytes_peak"],
            )
        if access != "local_sparse_file":
            values.update(
                io_counts(
                    row["http_events"] if native else row["events"],
                    not native,
                    baseline[row["selection"]]
                    if native
                    else ref_baseline[row["selection"]],
                )
            )
        groups[key].append(values)
    summaries = []
    for key, rows in groups.items():
        out = dict(
            zip(["access", "latency_seconds", "selection", "reader", "policy"], key)
        )
        out.update(
            repeats=len(rows),
            **{k: statistics.median(r[k] for r in rows) for k in rows[0]},
        )
        out["max_peak_rss_bytes"] = max(r["peak_rss_bytes"] for r in rows)
        summaries.append(out)
    save("performance-summary", summaries)
    text = [
        "# SWOT subset performance and physical reads\n",
        "Three fresh reader processes per cell. Delays apply to each loopback HEAD/GET; there is no bandwidth throttle or provider traffic. CPU includes child process startup; mature API CPU is retained separately. Native first-result time is first provisional chunk; mature first-result time is a complete slice. Native pre/post identity checks use more HEADs. Native emits chunk coordinates/metadata; controls return array hashes and summaries. Working reservations exclude runtime and caller retention and are not RSS caps.\n",
    ]
    for access in ["loopback_conditional_ranges", "local_sparse_file"]:
        for latency in [0, 0.02] if access.startswith("loopback") else [0]:
            text.append(f"## {access}; added delay {latency * 1000:g} ms\n")
            text += [
                "| Selection | Reader / policy | Prep ms | First ms | Total ms | CPU ms | Peak RSS MiB | GET/HEAD | Bytes | Reservation MiB |",
                "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|",
            ]
            for r in summaries:
                if r["access"] != access or r["latency_seconds"] != latency:
                    continue
                calls = f"{int(r['gets'])}/{int(r['heads'])}" if "gets" in r else "—"
                nbytes = (
                    str(int(r["response_bytes"]))
                    if "response_bytes" in r
                    else "not instrumented"
                )
                reserve = (
                    f"{r['reserved_bytes_peak'] / 1048576:.2f}"
                    if "reserved_bytes_peak" in r
                    else "—"
                )
                text.append(
                    f"| {r['selection']} | {r['reader']} {r['policy']} | {r['prepare_seconds'] * 1000:.2f} | {r['first_result_seconds'] * 1000:.2f} | {r['completion_seconds'] * 1000:.2f} | {r['cpu_seconds'] * 1000:.2f} | {r['max_peak_rss_bytes'] / 1048576:.2f} | {calls} | {nbytes} | {reserve} |"
                )
            text.append("")
    text += [
        "## Byte accounting\n",
        "Native phase bytes separate explicit coordinate chunk reads, WSE payload reads and metadata/index/page reads. Metadata pages can overlap payloads. Duplicate-byte counts include all physical overlap, even nonidentical ranges. Extra unique bytes compare a paged policy with the same reader's exact union; they do not equate native and mature validation scopes. All source coordinate chunks in this file hold 1580 samples, so 25,280 bytes of axes must be decoded even for a few requested coordinates. Each WSE chunk decodes 2,496,400 bytes. See acquisition.json and performance-summary.json for per-cell overfetch and range counts.\n",
        "netCDF4/NetCDF-C is a local sparse-file reference. Its kernel read ranges are not instrumented, so no remote byte or latency equivalence is claimed. h5py exact/16 KiB controls use bounded conditional mirror ranges with pre/post ETag/length checks and a bounded 2 MiB reference page cache. Native uses full index traversal by default; experimental selective traversal cannot validate unread subtrees. The three relevant indexes here are flat, so both strategies visit all three nodes.\n",
    ]
    (DOCS / "performance.md").write_text("\n".join(text))
    binary = ROOT / "target/release/examples/hdf5_subset_oracle"
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    assert science["binary_sha256"] == digest
    assert all(
        r["native"]["binary_sha256"] == digest
        for r in benchmark["cells"]
        if r["reader"] == "native"
    )
    for name in ["axes-science", "axes-faults", "axes-resources"]:
        assert all(r["binary_sha256"] == digest for r in read(name)["cells"])
    linked = (WORK / "native-linked-libraries.txt").read_text()
    assert "hdf5" not in linked.split(":\n", 1)[-1].lower()
    rust = read("inherited/rust")
    assert all(r["returncode"] == 0 for r in rust)
    checks = {
        "product_scientific_cells": len(science["cells"]),
        "independent_netcdf4_selections": 3,
        "axis_scientific_cells": len(read("axes-science")["cells"]),
        "axis_fault_cells": len(read("axes-faults")["cells"]),
        "axis_resource_cells": len(read("axes-resources")["cells"]),
        "benchmark_cells": len(benchmark["cells"]),
        "rust_checks": len(rust),
        "rust_configurations": len(rust) // 2,
        "inherited_netcdf_coordinate_cases": len(
            read("inherited/hdf5-selective/netcdf-coordinates")["cases"]
        ),
        "real_goes_regression_cells": len(
            read("inherited/hdf5-selective/goes-regression")["cases"]
        ),
    }
    inherited = []
    for path in sorted((DOCS / "inherited").rglob("*.json")):
        value = json.loads(path.read_text())
        inherited.append(
            {
                "path": str(path.relative_to(DOCS)),
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                "records": len(value),
                "passed": value.get("passed") if isinstance(value, dict) else None,
            }
        )
    save(
        "verification",
        {
            "base_commit": "92a6f6b2b61c21a81253b6214d92eb4c1c64b42b",
            "branch": "codex/acquisition-planner",
            "binary_sha256": digest,
            "checks": checks,
            "rust": rust,
            "inherited": inherited,
            "native_linked_libraries": linked.splitlines()[1:],
            "source_sha256": {
                str(p): hashlib.sha256((ROOT / p).read_bytes()).hexdigest()
                for p in ["src/hdf5.rs", "src/storage.rs", "tests/hdf5.rs"]
            },
            "native_live_remote_qualified": False,
        },
    )
    print(json.dumps(checks))


if __name__ == "__main__":
    main()
