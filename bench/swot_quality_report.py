"""Derive the Version D decision gate and compact reproducible receipts."""

import hashlib
import json
import statistics
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs/swot-quality"
WORK = ROOT / "prototype-output/swot-quality"


def read(name):
    return json.loads((DOCS / f"{name}.json").read_text())


def save(name, value):
    (DOCS / f"{name}.json").write_text(
        json.dumps(value, indent=2, allow_nan=False) + "\n"
    )


def phase_counts(row):
    source = row["observed"]["transport"]["science"]
    out = {}
    for name, operations in source.get("phases", {}).items():
        gets = [v for k, v in operations.items() if k.startswith("GET ")]
        out[name] = {
            "gets": sum(v["attempts"] for v in gets),
            "bytes": sum(v["bytes"] for v in gets),
        }
    assert sum(v["bytes"] for v in out.values()) == row["response_bytes"]
    assert sum(v["gets"] for v in out.values()) == row["gets"]
    return out


def main():
    science, bench, profile, capture = [
        read(n) for n in ["science", "benchmark", "product-profile", "capture"]
    ]
    digest = hashlib.sha256(
        (ROOT / "target/release/examples/swot_quality").read_bytes()
    ).hexdigest()
    assert digest == science["binary_sha256"] == bench["binary_sha256"]
    assert all(c["native"]["binary_sha256"] == digest for c in science["cells"])
    assert all(
        c["binary_sha256"] == digest
        for c in bench["cells"]
        if c["reader"] == "native" and c["mode"] == "multi-aligned"
    )
    assert read("faults")["all_rejected"] and len(read("faults")["cells"]) == 31
    rust = read("inherited/rust")
    assert all(c["returncode"] == 0 for c in rust)
    assert all(c["returncode"] == 0 for c in read("format-checks")["cells"])
    assert all(
        c["all_five_native_values_and_masks_equal"]
        for c in read("raw-science")["cells"]
    )
    example = next(
        c["native"]
        for c in science["cells"]
        if c.get("selection") == "window"
        and c["policy"] == "cookbook"
        and "metadata" not in c
    )
    source = json.loads((WORK / "science-window-cookbook/stdout.json").read_text())
    native_profile = source["inspection"]["sources"][0]["object_identities"]
    save(
        "native-profile",
        {
            "binary_sha256": digest,
            "provider_identity": {k: profile[k] for k in ["url", "etag", "size"]},
            "note": "Native preparation ran against the sealed conditional mirror; the mirror identity is bound to this provider object.",
            "native": native_profile,
        },
    )
    grouped = defaultdict(list)
    for c in bench["cells"]:
        key = (
            c["reader"],
            c["mode"],
            c["selection"],
            c.get("access", "conditional_loopback"),
            c.get("latency_seconds", 0),
            c.get("metadata", str(c.get("page_bytes", 0))),
        )
        grouped[key].append(c)
    summaries = []
    for key, rows in grouped.items():
        assert len(rows) == 3
        v = dict(
            zip(
                ["reader", "mode", "selection", "access", "latency_seconds", "policy"],
                key,
            )
        )
        v["repeats"] = len(rows)
        fields = [
            "prepare_seconds",
            "first_result_seconds",
            "completion_seconds",
            "child_cpu_seconds",
            "api_cpu_seconds",
            "process_cpu_seconds",
            "process_wall_seconds",
            "peak_rss_bytes",
            "gets",
            "heads",
            "response_bytes",
            "duplicate_bytes",
        ]
        for f in fields:
            if f in rows[0]:
                v[f] = statistics.median(r[f] for r in rows)
        v["max_peak_rss_bytes"] = max(r["peak_rss_bytes"] for r in rows)
        if rows[0]["reader"] == "native":
            v["reserved_bytes_peak"] = max(
                r["observed"]["scheduler"]["reserved_bytes_peak"] for r in rows
            )
            if v["access"] == "conditional_loopback":
                v["traffic_by_phase"] = phase_counts(rows[0])
        summaries.append(v)
    save(
        "performance-summary",
        {"cells": summaries, "repeats": 3, "binary_sha256": digest},
    )
    text = [
        "# Performance and physical acquisition",
        "",
        "Medians of three fresh processes per cell; RSS columns use the maximum peak. Zero/20 ms delays apply per loopback HEAD/GET, with no bandwidth throttle. No provider traffic occurs. CPU includes native child startup; mature API and process CPU are retained separately. Native validates more metadata and emits provisional tiles; mature first results are complete slices. NetCDF-C local kernel I/O is not instrumented. All file/cache reuse is stated in workflow.md.",
        "",
    ]
    for access in ["conditional_loopback", "local_sparse_reference"]:
        for delay in [0, 0.02] if access == "conditional_loopback" else [0]:
            text += [
                f"## {access}, {delay * 1000:g} ms added response delay",
                "",
                "| Selection | Reader / operation / policy | Prep ms | First ms | Total API ms | Process CPU ms | API CPU ms | RSS MiB | GET/HEAD | Bytes | Reservation MiB |",
                "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
            ]
            for c in summaries:
                if c["access"] != access or c["latency_seconds"] != delay:
                    continue
                first = (
                    f"{c['first_result_seconds'] * 1000:.2f}"
                    if "first_result_seconds" in c
                    else "complete slice"
                )
                process_cpu = (
                    c.get("child_cpu_seconds", c.get("process_cpu_seconds", 0)) * 1000
                )
                api_cpu = (
                    f"{c['api_cpu_seconds'] * 1000:.2f}"
                    if "api_cpu_seconds" in c
                    else "-"
                )
                calls = f"{int(c['gets'])}/{int(c['heads'])}" if "gets" in c else "-"
                b = (
                    str(int(c["response_bytes"]))
                    if "response_bytes" in c and access != "local_sparse_reference"
                    else "not instrumented"
                )
                reserve = (
                    f"{c['reserved_bytes_peak'] / 1048576:.2f}"
                    if "reserved_bytes_peak" in c
                    else "-"
                )
                text.append(
                    f"| {c['selection']} | {c['reader']} / {c['mode']} / {c['policy']} | {c['prepare_seconds'] * 1000:.2f} | {first} | {c['completion_seconds'] * 1000:.2f} | {process_cpu:.2f} | {api_cpu} | {c['max_peak_rss_bytes'] / 1048576:.2f} | {calls} | {b} | {reserve} |"
                )
            text.append("")
    text += [
        "## Default exact traffic for the 128 x 128 window",
        "",
        "| Operation | GET/HEAD | Total bytes | Duplicate bytes | Coordinates bytes / GET | Metadata and indexes bytes / GET |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    physical = []
    for c in summaries:
        if (
            c["reader"] != "native"
            or c["access"] != "conditional_loopback"
            or c["latency_seconds"] != 0
            or c["selection"] != "window"
            or c["policy"] != "exact"
        ):
            continue
        phases = c["traffic_by_phase"]
        coords = phases["hdf5_coordinate_chunks"]
        meta = [v for k, v in phases.items() if k.endswith("index_discovery")]
        entry = {
            "mode": c["mode"],
            "gets": c["gets"],
            "heads": c["heads"],
            "response_bytes": c["response_bytes"],
            "duplicate_bytes": c["duplicate_bytes"],
            "fraction_of_source": c["response_bytes"] / profile["size"],
            "traffic_by_phase": phases,
        }
        physical.append(entry)
        text.append(
            f"| {c['mode']} | {int(c['gets'])}/{int(c['heads'])} | {int(c['response_bytes'])} | {int(c['duplicate_bytes'])} | {coords['bytes']} / {coords['gets']} | {sum(v['bytes'] for v in meta)} / {sum(v['gets'] for v in meta)} |"
        )
    text += [
        "",
        "Metadata/index phases include their actual physical bytes. Paged metadata can overlap payload ranges, so phase totals, unique bytes and duplicate bytes have different meanings. Coordinate reads occur once per axis across five variables. Cache eviction may require repeat payload reads; encoded cache hits and repeated decompression are reported separately. Working reservations are not RSS caps. Mature reader validation scopes differ; these results do not establish a universal performance lead.",
        "",
    ]
    (DOCS / "performance.md").write_text("\n".join(text))
    save("acquisition", physical)
    best = next(
        c
        for c in summaries
        if c["reader"] == "native"
        and c["mode"] == "multi-aligned"
        and c["selection"] == "window"
        and c["access"] == "conditional_loopback"
        and c["latency_seconds"] == 0
        and c["policy"] == "exact"
    )
    page = next(
        c
        for c in summaries
        if c["reader"] == "native"
        and c["mode"] == "multi-aligned"
        and c["selection"] == "window"
        and c["access"] == "conditional_loopback"
        and c["latency_seconds"] == 0.02
        and c["policy"] == "page16k"
    )
    exact_latency = next(
        c
        for c in summaries
        if c["reader"] == "native"
        and c["mode"] == "multi-aligned"
        and c["selection"] == "window"
        and c["access"] == "conditional_loopback"
        and c["latency_seconds"] == 0.02
        and c["policy"] == "exact"
    )

    def cell(reader, mode, selection, access, delay, policy):
        return next(
            c
            for c in summaries
            if (
                c["reader"],
                c["mode"],
                c["selection"],
                c["access"],
                c["latency_seconds"],
                c["policy"],
            )
            == (reader, mode, selection, access, delay, policy)
        )

    h5_exact = cell(
        "h5py", "multi-quality", "window", "conditional_loopback", 0.02, "0"
    )
    h5_paged = cell(
        "h5py", "multi-quality", "window", "conditional_loopback", 0.02, "16384"
    )
    dense_h5 = cell("h5py", "multi-quality", "dense", "local_sparse_reference", 0, "0")
    dense_native = cell(
        "native", "multi-aligned", "dense", "local_sparse_reference", 0, "0"
    )
    baseline = next(c for c in physical if c["mode"] == "wse-only")
    s = example["summary"]
    phases = phase_counts(example)
    rows = ["| Variable / traffic | Bytes | GET |", "|---|---:|---:|"]
    for phase in [
        "hdf5_wse_payload",
        "hdf5_summary_quality_payload",
        "hdf5_bitwise_quality_payload",
        "hdf5_uncertainty_payload",
        "hdf5_pixel_count_payload",
        "hdf5_coordinate_chunks",
        "hdf5_coordinate_index_discovery",
        "hdf5_chunk_index_discovery",
        "metadata_index_discovery",
    ]:
        v = phases[phase]
        rows.append(f"| {phase} | {v['bytes']} | {v['gets']} |")
    resources = read("resources")["cells"]
    resource_rows = [
        "| Raw output | Working budget | Peak reservation | Process peak RSS | Completion |",
        "|---:|---:|---:|---:|---:|",
    ]
    for r in resources:
        if r["status"] == "complete":
            n = r["native"]
            resource_rows.append(
                f"| {r['edge'] ** 2 * 17 / 1048576:g} MiB | {r['budget_mib']} MiB | {n['observed']['scheduler']['reserved_bytes_peak'] / 1048576:.2f} MiB | {n['peak_rss_bytes'] / 1048576:.2f} MiB | {n['completion_seconds'] * 1000:.1f} ms |"
            )
    report = f"""# Version D quality-aware SWOT WSE: decision gate

**PASS for scientifically correct, bounded five-layer Version D acquisition and
explicit quality-aware analysis on a provider-identity-pinned mirror. The
authenticated live-native archive gate remains CLOSED.** Validated locally on
2026-10-22, continuing from `dbafecc` on `codex/acquisition-planner`.

## 1. Scientific correctness

All 38 quality-aware science cells agree bit-for-bit with separately executed
h5py and NetCDF-C for every selected raw value, source mask, coordinate and
derived mask. Three further raw multi-variable cells validate the ordinary
stream API. Cases include real D sparse/dense/missing windows, boundary crossings,
noncontiguous strides, both byte orders, independent physical layouts, high
UInt32 bits and unassigned conditions. See [science](science.json),
[raw stream science](raw-science.json) and [policies](policies.md).

The demonstrative `[600,400]`, 128 x 128 window uses the explicit Cookbook good
or suspect policy. It selects **{s["selected_count"]:,} pixels**, with stored WSE
mean **{s["wse_mean_m"]:.9f} m**, mean available per-pixel one-sigma uncertainty
**{s["mean_pixel_uncertainty_m"]:.9f} m** ({s["uncertainty_count"]:,} samples), and
**{s["contributing_pixel_sum"]:,}** contributing PIXC samples across selected
cells. Both independent readers exactly reproduce these counts, sums and means.
The uncertainty mean is descriptive, not uncertainty of the spatial mean.
This is not a river-specific SWOT processing workflow.

## 2. Exact stored types and variables

The native HDF5 adapter now admits UInt8, UInt16, UInt32 and Float32 within its
existing rank-two/v1-index/filter profile. Source byte order is interpreted
exactly; public values retain native integer types and original attributes.
The D workflow streams `/wse` Float32, `/wse_qual` UInt8,
`/wse_qual_bitwise` UInt32, `/wse_uncert` Float32 and `/n_wse_pix` UInt32.
WSE/uncertainty CF fill is 9.969209968386869e36; summary fill is 255;
UInt32 fill is 4294967295. The actual storage and CF fills agree. Implicit
initialization remains separate from CF missingness; ambiguous states fail.

Stored Float64 `/x` and `/y` remain exact rank-one coordinates. Source units,
packing, WKT/CF CRS, processing identifiers, original attribute bytes and
source identities are preserved. The source count-units string `l` is retained
literally. Rank-two Float64 `/illumination_time` is inspected and explicitly
unsupported. Granule/coverage times suffice for this single-observation summary;
per-pixel timing is needed before time-sensitive inter-observation analysis.

## 3. Validated quality semantics

Raw source missingness; explicitly accepted good/suspect/degraded/bad categories;
and named bitwise required/excluded conditions are separate policies. The
Cookbook example accepts good and suspect. Bitwise requires every requested
condition, excludes every named veto and rejects unassigned observed bits.
Source definitions must match before applying interpreted policies; unknown,
duplicate or contradictory requested names fail. Raw auxiliary arrays and
independent masks remain available under every policy. No uncertainty/count
threshold, vector-product bit meaning or universal good-pixel definition is
inferred. The exact 19 D masks, definitions and ambiguities are in
[policies.md](policies.md) and recorded in each stream's provenance.

## 4. Exact logical alignment and bounded delivery

All five arrays share the requested logical source indices, positive strides
and stored coordinate centers. The actual D summary array has one 1580 x 1580
physical chunk; the four other science arrays use 790 x 790 chunks. Each uses
its own index and encoded ranges. Synthetic files also vary all five layouts.
Output order is deterministic by WSE chunk row/column and admitted logical
subtile, with PATHS order within a tile. Values and calculations are provisional
until `finish()` validates the final source identity. Cancellation/drop joins
the worker. An execution-local bounded encoded-range LRU and shared coordinate
preparation avoid redundant reads; decoded chunks may be recomputed across
logical intersections. Full index traversal remains the default. Experimental
selective traversal does not validate skipped subtrees.

## 5. Physical bytes and requests

The real object is **{profile["size"]:,} bytes**, strong opaque ETag
`{profile["etag"]}`. Its exact filename and all inspected layouts, filters,
allocated extents, fills and attributes are in [product-profile.json](product-profile.json)
and [native-profile.json](native-profile.json). All selected arrays are aligned
1580 x 1580 `(y,x)`, shuffle + DEFLATE 4; axes have one 1580-sample compressed
chunk each. The source projection is WGS 84 / UTM 19N (EPSG:32619).

The default exact five-layer 128 x 128 acquisition retrieves
**{example["response_bytes"]:,} bytes ({100 * example["response_bytes"] / profile["size"]:.3f}% of the product)**
through **{example["gets"]} GET + {example["heads"]} HEAD** requests, with
**{example["duplicate_bytes"]} duplicate response bytes**. Per-phase payloads:

{chr(10).join(rows)}

The two coordinate payloads total 1,728 bytes across all five variables.
WSE alone retrieves {int(baseline["response_bytes"]):,} bytes through {int(baseline["gets"])} GET + {int(baseline["heads"])} HEAD; adding all four
auxiliary layers increases physical traffic by {int(example["response_bytes"] - baseline["response_bytes"]):,} bytes and {int(example["gets"] - baseline["gets"])} GETs.
Shared metadata/coordinates prevent five independent preparation passes.
WSE-only versus raw multi-variable versus aligned quality-aware totals,
metadata/page overlap and per-phase requests are in [acquisition.json](acquisition.json)
and [performance.md](performance.md). There is no full-product acquisition.
The separately labelled reference inspection fetched {profile["bytes"]:,} bytes;
reference capture fetched {capture["provider_bytes"]:,} bytes in {capture["provider_gets"]} GETs
under a 24 MiB cap. Captured-extent hashes are not a whole-file checksum.

## 6. Working reservations, CPU, RSS and completion

For the default exact 128 x 128 aligned operation, the three-run median is
**{best["completion_seconds"] * 1000:.2f} ms** completion and
**{best["child_cpu_seconds"] * 1000:.2f} ms** native process CPU, maximum peak RSS
**{best["max_peak_rss_bytes"] / 1048576:.2f} MiB**, reservation
**{best["reserved_bytes_peak"] / 1048576:.2f} MiB** within a 32 MiB working budget.
At 20 ms added per-response delay, exact metadata takes
{exact_latency["completion_seconds"]:.3f} s versus {page["completion_seconds"]:.3f} s
with the verified optional 16 KiB metadata policy. All 114 benchmark cells and
mature h5py/NetCDF-C controls are retained in [benchmark.json](benchmark.json)
and [performance-summary.json](performance-summary.json). These controlled
loopback results are not provider-throughput or universal performance claims.

The tradeoffs are visible in the controls: at 20 ms response delay, h5py exact
ranges complete the window in {h5_exact["completion_seconds"]:.3f} s versus native exact {exact_latency["completion_seconds"]:.3f} s, reflecting
fewer metadata requests. The 16 KiB policies are close (native {page["completion_seconds"]:.3f} s, h5py
{h5_paged["completion_seconds"]:.3f} s). On the local dense slice, native aligned completion is {dense_native["completion_seconds"] * 1000:.1f} ms versus
h5py {dense_h5["completion_seconds"] * 1000:.1f} ms; native repeats decoder intersections to bound assembled output.
Decode counters include first visits as well as repeats. These are observed
profile-specific results, with the differing validation scopes described above.

{chr(10).join(resource_rows)}

[Resources](resources.json) show bounded tiled assembly for up to 68 MiB of raw
output. [Real D admission](real-resources.json) rejects insufficient 4/8/16/24 MiB
budgets and succeeds at 32 MiB. Reservations are working admission, not RSS caps:
runtime/allocator overhead, inspection clones, plan representations and caller
retention differ. RSS grows with selected plan/chunk count; it is not presented
as independent of total selection size.

## 7. Authenticated live-native archive access

**CLOSED.** Fresh Version D official authenticated eight-byte conditional GET
and wrong-ETag rejection succeed in independent reference tooling. HEAD and GET
use method-specific signed routes; cross-method reuse returns 403. Native
canonical inspection still rejects redirects. No native authentication or HTTP
validation was weakened. The caller-configured origin, credential/cookie,
redirect and final-object invariants remain a separate gate in
[access-design.md](access-design.md), with fresh evidence in [access.json](access.json).
No secret, cookie value or signed URL is committed.

## 8. Version C versus actually inspected Version D

The target is the actual **PGD0 Version D**, PGE 5.4.0, created 2025-12-22,
for the 2024-02-16 cycle-11/pass-35/scene-115 observation. The older PIC0 Version C
object is 92,416,806 bytes and PGE 5.1.1. Both inspected grids and chunk shapes
match, but raw science values differ across reprocessing and must not be mixed.
D declares bit 9 (512) and bit 22 (4194304), absent from the inspected PIC0 flag
definitions. The newer PDD is document Revision C, which is not the data release.
The original C WSE/coordinate suite still passes all 18 cells. The interpreted
quality-aware wrapper requires D identity/schema; C auxiliary quality acquisition
and other D scenes/PID0 processing are not blanket-qualified by this result.

## 9. Remaining workflow barriers and regression gate

Live native archive access is the main usability barrier. Geographic AOI mapping,
per-pixel Float64 timing, time-series alignment, broad CF interpretation,
river masks/corrections and wider HDF5 types/index families remain outside scope.
Large physical chunks impose a decode/reservation floor. Aligned intersections
can repeat decompression despite encoded-range cache hits; metadata batching
helps latency but does not remove this CPU tradeoff.

The [verification receipt](verification.json) records 31 new rejected fault cases,
six synthetic and five real budget cases, the Rust test/Clippy feature matrix,
270 vendored-reader tests, original C science, 146 compressed-axis
science cases, HDF5 full/selective science/fault/resources and coordinate checks,
and COG/Zarr v2/v3/streaming/cancellation/transactional-publication gates.
The native build requires no GDAL or libhdf5. Existing default GDAL behavior is
retained. A small local vendored-parser guard rejects nonzero integer bit offsets,
partial precision and unrepresentable widths instead of misdecoding them.
Three independently written noncanonical fixtures are rejected; the
[admission receipt](integer-admission.json) includes the confirmed pre-guard
16-versus-1 counterexample and final explicit rejection. The existing
selective-index patch and dependency lockfiles are unchanged; no ChronoZarr or
upstream repository was modified.

## 10. Best next general-purpose milestone

Implement and adversarially qualify explicit caller-configured authentication
and method-specific redirects in the shared conditional-range transport. This
would make the verified D science workflow usable against the official archive
and help other protected scientific products. Preserve version validation and
secret confinement as entry gates. Reducing repeated decompression is a
subsequent measured optimization, not a reason to weaken the access contract.

Implementation and validated reports are committed locally. No merge, push,
tag, publication, deployment or upstream HDF5 submission is part of this work.
"""
    (DOCS / "report.md").write_text(report)
    verification = {
        "date": "2026-10-22",
        "base_commit": "dbafecc227a052490644f39fd1aba097b6ecbac3",
        "target": "actual Version D PGD0 object",
        "quality_binary_sha256": digest,
        "reader_datatype_guard_sha256": hashlib.sha256(
            (ROOT / "vendor/hdf5-reader/src/messages/datatype.rs").read_bytes()
        ).hexdigest(),
        "ordinary_hdf5_binary_sha256": hashlib.sha256(
            (ROOT / "target/release/examples/hdf5_subset_oracle").read_bytes()
        ).hexdigest(),
        "science_cells": len(science["cells"]),
        "raw_multivariable_cells": len(read("raw-science")["cells"]),
        "independent_quality_reference_processes": len(science["cells"]) * 2,
        "fault_rejections": 31,
        "noncanonical_integer_rejections": 3,
        "synthetic_resource_cells": len(resources),
        "real_budget_cells": len(read("real-resources")["cells"]),
        "benchmark_cells": len(bench["cells"]),
        "rust_feature_matrix": rust,
        "format_checks": read("format-checks")["cells"],
        "native_core": {
            "mandatory_gdal": False,
            "mandatory_libhdf5": False,
            "dependency_tree": "dependency-tree.txt",
            "linkage_command": "otool -L target/release/examples/swot_quality",
        },
        "checks": [
            "cargo fmt --all -- --check",
            "uvx ruff check bench/swot_quality.py bench/swot_quality_reference.py bench/swot_quality_report.py bench/hdf5_input.py",
            "git diff --check",
        ],
        "final_gates": {
            "quality_aware_version_d_mirror": "pass",
            "authenticated_live_native_archive": "closed",
            "original_version_c_wse_regression": "pass",
            "cross_format_regressions": "pass",
            "general_version_d_family": "not blanket qualified",
        },
    }
    save("verification", verification)
    print(
        json.dumps(
            {
                "science_cells": len(science["cells"]),
                "benchmark_cells": len(bench["cells"]),
                "binary_sha256": digest,
            }
        )
    )


if __name__ == "__main__":
    main()
