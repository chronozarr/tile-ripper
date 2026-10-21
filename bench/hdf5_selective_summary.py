"""Derive the selective-index decision tables from retained machine receipts."""

import itertools
import json
import re
import statistics
from pathlib import Path

import h5py

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs/hdf5-selective"
WORK = ROOT / "prototype-output/hdf5-selective"


def load(name):
    return json.loads((DOCS / f"{name}.json").read_text())


def med(cells, key):
    assert len(cells) == 3, (len(cells), key)
    return statistics.median(c[key] for c in cells)


def group(cells, selection, latency, reader, index=None, page=None):
    return [
        c
        for c in cells
        if c.get("selection") == selection
        and c["latency"] == latency
        and c["reader"] == reader
        and c.get("index") == index
        and c.get("metadata", c.get("page_bytes")) == page
    ]


def allocations(path, dataset):
    with h5py.File(path, "r") as f:
        ds = f[dataset]
        return sorted(
            (c.byte_offset, c.byte_offset + c.size)
            for i in range(ds.id.get_num_chunks())
            if (c := ds.id.get_chunk_info(i)).byte_offset is not None
        )


def partition(cells, ranges):
    result = []
    for cell in cells:
        if cell["reader"] != "h5py/libhdf5":
            continue
        counts = {
            "metadata_only_gets": 0,
            "payload_only_gets": 0,
            "mixed_gets": 0,
            "scientific_payload_bytes": 0,
            "metadata_or_other_bytes": 0,
        }
        for a, b in cell["events"]:
            payload = sum(max(0, min(b, z) - max(a, x)) for x, z in ranges)
            other = b - a - payload
            counts["scientific_payload_bytes"] += payload
            counts["metadata_or_other_bytes"] += other
            counts[
                "mixed_gets"
                if payload and other
                else "payload_only_gets"
                if payload
                else "metadata_only_gets"
            ] += 1
        result.append(
            {
                k: cell[k]
                for k in ["selection", "latency", "repeat", "page_bytes"]
                if k in cell
            }
            | counts
        )
    return result


def index_structure(path, ranges):
    """Independent test-only walk of the captured primary rank-two index."""
    raw = path.read_bytes()
    width = raw[13] if raw[8] < 2 else raw[9]
    assert width in (4, 8)
    header, stride = 8 + 2 * width, 32 + width
    nodes = {}
    for match in re.finditer(b"TREE\x01", raw):
        a = match.start()
        n = int.from_bytes(raw[a + 6 : a + 8], "little")
        if not n or n > 1024 or a + header + n * stride + 32 > len(raw):
            continue
        keys = [
            tuple(
                int.from_bytes(
                    raw[
                        a + header + i * stride + 8 + j * 8 : a
                        + header
                        + i * stride
                        + 16
                        + j * 8
                    ],
                    "little",
                )
                for j in range(3)
            )
            for i in range(n + 1)
        ]
        if any(x >= y for x, y in itertools.pairwise(keys)):
            continue
        children = [
            int.from_bytes(
                raw[
                    a + header + i * stride + 32 : a + header + i * stride + 32 + width
                ],
                "little",
            )
            for i in range(n)
        ]
        nodes[a] = {"level": raw[a + 5], "children": children, "entries": n}
    allocated = {a for a, _ in ranges}

    def walk(address, visited):
        if address in visited or address not in nodes:
            raise ValueError("not a primary index tree")
        visited.add(address)
        node = nodes[address]
        if node["level"] == 0:
            return set(node["children"])
        result = set()
        for child in node["children"]:
            result |= walk(child, visited)
        return result

    for a in nodes:
        visited = set()
        try:
            found = walk(a, visited)
        except ValueError:
            continue
        if found == allocated:
            levels = {}
            for n in visited:
                level = str(nodes[n]["level"])
                levels[level] = levels.get(level, 0) + 1
            return {
                "root_address": a,
                "nodes": len(visited),
                "levels": levels,
                "allocated_records": len(found),
                "leaf_record_counts": sorted(
                    nodes[n]["entries"] for n in visited if nodes[n]["level"] == 0
                ),
            }
    raise ValueError("captured primary tree not identified")


def main():
    benchmark, tuning, small = load("benchmark"), load("tuning"), load("small")
    cells = benchmark["cells"] + tuning["cells"]
    final = load("final-binary")
    hashes = {
        c["binary_sha256"] for c in cells + small["cells"] if c["reader"] == "native"
    }
    assert hashes == {final["sha256"]}, hashes
    final["benchmark_sha256"] = final["sha256"]
    final["byte_identical_to_timing_binary"] = True
    (DOCS / "final-binary.json").write_text(json.dumps(final, indent=2) + "\n")
    assert (
        len(benchmark["cells"]) == 216
        and len(tuning["cells"]) == 54
        and len(small["cells"]) == 54
    )
    assert len(load("science")["cells"]) == 64 and len(load("faults")["cases"]) == 33
    ranges = allocations(WORK / "large-mirror/source.nc", "/TEMP")
    structure = index_structure(WORK / "large-mirror/source.nc", ranges)
    mature_bytes = partition(cells, ranges)
    (DOCS / "mature-io.json").write_text(json.dumps(mature_bytes, indent=2) + "\n")
    configurations = [
        ("A full/exact", "native", "full", "exact"),
        ("B selective/exact", "native", "selective", "exact"),
        ("C full/16 KiB", "native", "full", "page16k"),
        ("C selective/16 KiB", "native", "selective", "page16k"),
        ("D h5py/64 KiB", "h5py/libhdf5", None, 65536),
        ("D h5py/256 KiB", "h5py/libhdf5", None, 262144),
    ]
    performance = [
        "# Matched selection performance",
        "",
        "Medians of three separate processes per cell; seconds. Added delay is per HTTP response, including HEAD. Local-file results are separate in `local.json`.",
        "",
        "| Delay | Selection | Strategy | Prepare | First values | Complete | CPU | Peak RSS MiB |",
        "|---:|---|---|---:|---:|---:|---:|---:|",
    ]
    comparisons = []
    for latency in [0, 0.02, 0.08]:
        for selection in ["sparse", "dense", "noncontiguous"]:
            for label, reader, index, page in configurations:
                selected = group(cells, selection, latency, reader, index, page)
                row = {
                    "delay": latency,
                    "selection": selection,
                    "strategy": label,
                    **{
                        k: med(selected, k)
                        for k in [
                            "prepare_seconds",
                            "first_chunk_seconds",
                            "completion_seconds",
                            "cpu_seconds",
                            "peak_rss_bytes",
                        ]
                    },
                }
                comparisons.append(row)
                performance.append(
                    f"| {latency * 1000:.0f} ms | {selection} | {label} | {row['prepare_seconds']:.4f} | {row['first_chunk_seconds']:.4f} | {row['completion_seconds']:.4f} | {row['cpu_seconds']:.4f} | {row['peak_rss_bytes'] / 2**20:.2f} |"
                )
    performance += [
        "",
        "`First values` is first provisional native chunk versus completion of the one-shot h5py slice. The latter materializes the whole selection. CPU is whole child-process CPU, including Python startup/imports; h5py API-only CPU and whole-process elapsed time are also retained. Timed acquisition wall clocks exclude interpreter imports and request-file parsing.",
        "",
        "Native transport uses five HEADs; the h5py range control uses two. Both pin exact ranges and revalidate ETag/size at completion. Native reservations include its prepared metadata and bounded scheduler; h5py uses a 2 MiB chunk cache and a bounded 2 MiB range-page cache, plus the selected output array. RSS includes the respective runtimes. Native provisional finish, cancellation and whole-index/full versus selected validation scope are not equivalent to returning one h5py array.",
        "",
        "The experiment adds delay on loopback without a bandwidth throttle, TLS or WAN variation. It supports neither a general performance-leadership claim nor an optimal-page-size claim outside the tested choices. `mature-io.json` partitions h5py ranges into primary-science payload and metadata/other bytes. Mixed GETs are explicit. Native metadata byte counts are phase-based and include page overfetch, which can contain nearby payload bytes.",
    ]
    (DOCS / "performance.md").write_text("\n".join(performance) + "\n")
    io_rows = []
    for selection in ["sparse", "dense", "noncontiguous"]:
        for policy in ["full", "selective"]:
            c = group(cells, selection, 0, "native", policy, "exact")[0]
            io_rows.append(
                {
                    "selection": selection,
                    "policy": policy,
                    **{
                        k: c[k]
                        for k in [
                            "index_nodes_visited",
                            "index_phase",
                            "metadata_gets",
                            "metadata_bytes",
                            "payload_gets",
                            "payload_bytes",
                        ]
                    },
                }
            )
    summary = {
        "base_commit": "fe58a2c",
        "branch": "codex/acquisition-planner",
        "default_index_policy": "full",
        "selective_default_adoption": "not recommended under inherited whole-index failure contract",
        "source": load("large-file")["dataset"],
        "index_structure": structure,
        "native_io": io_rows,
        "comparisons": comparisons,
        "science_cases": 64,
        "inherited_fault_cases": 33,
        "added_science_cells": len(load("extended")["science"])
        + len(load("extra")["science"]),
        "added_fault_policy_cells": len(load("extended")["faults"])
        + len(load("extra")["faults"]),
        "large_independent_cases": len(load("large-file")["science"]),
        "benchmark_cells": len(cells) + len(small["cells"]),
        "native_binary_sha256": final["sha256"],
        "primary_mirror_bytes": load("large-file")["mirror"]["provider_bytes"],
        "tuned_mirror_bytes": tuning["mirror"]["provider_bytes"],
        "inherited_gates": load("inherited-v2") + load("inherited-v3"),
    }
    (DOCS / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    native_table = [
        "| Selection | Policy | Nodes | Index GETs / bytes | All prep GETs / bytes | Payload GETs / bytes |",
        "|---|---|---:|---:|---:|---:|",
    ]
    for r in io_rows:
        native_table.append(
            f"| {r['selection']} | {r['policy']} | {r['index_nodes_visited']} | {r['index_phase']['gets']} / {r['index_phase']['bytes']:,} | {r['metadata_gets']} / {r['metadata_bytes']:,} | {r['payload_gets']} / {r['payload_bytes']:,} |"
        )
    latency_table = [
        "| Selection | Full / exact | Selective / exact | Full / 16 KiB | Selective / 16 KiB | h5py / 64 KiB | h5py / 256 KiB |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    for selection in ["sparse", "dense", "noncontiguous"]:
        times = [
            next(
                r["completion_seconds"]
                for r in comparisons
                if r["selection"] == selection
                and r["delay"] == 0.08
                and r["strategy"] == label
            )
            for label, *_ in configurations
        ]
        latency_table.append(
            "| " + selection + " | " + " | ".join(f"{t:.3f}" for t in times) + " |"
        )
    memory = load("selective-resources")
    reserved_max = max(
        c["scheduler"]["reserved_bytes_peak"] for c in cells if c["reader"] == "native"
    )
    rss_max = max(c["peak_rss_bytes"] for c in cells if c["reader"] == "native")
    small_full = [
        c
        for c in small["cells"]
        if c["latency"] == 0.08
        and c["reader"] == "h5py/libhdf5"
        and c["full_file_control"]
    ]
    small_selected = [
        c
        for c in small["cells"]
        if c["latency"] == 0.08
        and c["reader"] == "native"
        and c["index"] == "selective"
        and c["metadata"] == "page16k"
    ]
    report = f"""# Selective HDF5 v1 traversal: decision gate

**Pruning is possible for valid ordered v1 chunk indexes and materially reduces
remote metadata I/O. Keep it as an explicit experiment. Full traversal remains
the default because pruning cannot retain failure detection inside unread
subtrees.** The additive parser extension, tests and unpublished upstream patch
are committed locally on `codex/acquisition-planner`.

## Real-file result

The [NOAA GOES-19 request](goes19-request.json) selects native UInt16 cloud-top
temperature codes from a **28,545,639-byte** NetCDF-4 object, 22.5 times the
original rainfall control. `/TEMP` is 5424×5424 with 226×226 chunks,
shuffle/DEFLATE and a legacy v1 index. There are 548 allocated chunks in a
24×24 logical grid. Independent inspection identifies {structure["nodes"]} index
nodes: one level-one root and 15 leaves. See [large-file.json](large-file.json)
and [summary.json](summary.json).

For the one-chunk selection, exact index traversal changes from **32 GETs /
23,800 bytes to 4 GETs / 3,040 bytes**, visiting 2 rather than 16 nodes. Payload
retrieval is the identical one GET / 87,452 bytes. All preparation, including
attributes and selected coordinates, falls from 76 GETs / 34,908 bytes to
48 GETs / 14,148 bytes. Only 101,600 response bytes are needed for the native
selective acquisition, about 0.36% of the object. Direct conditional NOAA reads
and h5py/NetCDF-C comparisons confirm the result; this is not a synthetic file.

{chr(10).join(native_table)}

Dense means the 904×904 window beginning at (2000,2000); noncontiguous means
5×6 samples at steps (901,677), beginning at (226,452). The latter includes
unallocated chunks. All values, validity bits, stored coordinates and selected
chunk sets match full traversal and independent references. The original
`fe58a2c` executable has the same full-index counts. Whole-grid geometry tests
visit the same complete index under both policies; pruning is not always an
I/O saving.

The primary mirror downloaded **{summary["primary_mirror_bytes"]:,} bytes** and
the separate broader-page tuning mirror totals **{summary["tuned_mirror_bytes"]:,}
bytes**, including the reused primary extents. The latter required only
{tuning["additional_provider_bytes"]:,} additional provider bytes. Both remain
under the explicit 16 MiB cap. Extent hashes and version identities are retained;
HTTP replay refuses uncaptured holes. No massive object was staged, and native
acquisition has no whole-file fallback.

## Performance and resources

The completed matrix has **324 cells**, three processes per combination,
including 0, 20 and 80 ms added response delay. It compares full and selective
exact reads, bounded 4/16 KiB native metadata pages, and h5py exact/16/64/256 KiB
range strategies. Local-file timings and the small-object whole-file control
are separate. At 80 ms, median completion seconds are:

{chr(10).join(latency_table)}

Metadata batching can outperform pruning alone. Mature h5py remains faster for
the sparse selection; the measured selected/batched native path is competitive
for denser work. This is a delay-only loopback experiment, not a universal
performance ranking. First-native-chunk timing is compared with whole-slice
return for h5py, and native pre/post checks use more HEADs. See the complete
[phase, CPU and RSS tables](performance.md), [local results](local.json) and
[mature metadata/payload byte partition](mature-io.json) for these differences.
libhdf5 internal node visits are not exposed by its file-object API; its exact
requested byte ranges are retained instead of inventing visit counts.

For the 1.27 MB rainfall control at 80 ms, whole-file h5py takes
{med(small_full, "completion_seconds"):.3f} s versus
{med(small_selected, "completion_seconds"):.3f} s for selected native acquisition
with 16 KiB metadata pages. Whole retrieval really is cheaper in this small,
delay-dominated control; it is explicitly labelled reference tooling.

All native benchmark reservations fit the declared 32 MiB budget. The largest
observed benchmark reservation is {reserved_max / 2**20:.2f} MiB and peak native
RSS is {rss_max / 2**20:.2f} MiB. Separate resource gates acquire 0.5 and 32 MiB of
output under 4 and 8 MiB working budgets; the selected-mode RSS ranges from
{min(r["peak_rss_bytes"] for r in memory) / 2**20:.2f} to
{max(r["peak_rss_bytes"] for r in memory) / 2**20:.2f} MiB. Working reservations
are not an RSS cap; runtime, HTTP, receipt/inspection structures and caller
retention have their documented scope. [Resource receipts](selective-resources.json)
also retain declared reservations. Timing, scientific and fault receipts name
the exact final native binary SHA256 in [final-binary.json](final-binary.json).
The original mirror-capture records retain their earlier capture-stage hash;
final large-file comparisons use the current benchmark receipts.

## Scientific, fault and structural decision

Passed: the original **64 scientific and 33 fault cases**, **56 added scientific
comparison cells**, **33 added fault/policy cells**, and three independently
checked large-file selections. Added coverage includes boundary/first/last and
intermediate keys, several chunk shapes, full and strided selections, holes and
fully unallocated storage, reverse payload allocation, bad pointers/counts,
truncation, parent/key/level inconsistencies, mutation, page-cache framing,
cancellation and small budgets. Some scope tests deliberately expect selected
acceptance of an unrelated damaged subtree; they document the limit, not a
claim that every corrupt file is rejected. A further [123-case NetCDF-C axis
audit](netcdf-coordinates.json) independently confirms the stored coordinates,
with automatic scientific packing disabled.

All **269 upstream library unit tests** and all-target Clippy pass. The root
default/no-default/native-cog/native-hdf5/all-features test and Clippy matrix
passes, with final HDF5/all-features checks refreshed. Inherited COG, Zarr v2,
CF, Zarr v3, streaming, durable-output and fault gates pass. Receipts are under
[inherited/](inherited/) and [verification.json](verification.json).

The [audit](audit.md) explains lexicographic Cartesian pruning, the byte
dimension, missing records and validation. Visible-node validation is stronger
in several respects, but an unread descendant cannot be checked. Full traversal
is retained by default. Recommend **against default adoption** under the
inherited whole-index failure contract. `TILE_RIPPER_HDF5_INDEX_POLICY=selective`
is an explicitly selected experiment; metadata pages are independently opt-in.

## Upstream and next milestone

The additive, opt-in [upstream proposal](upstream-proposal.md) and its
[apply-checked patch](upstream.patch) are ready for review. Existing collector/read-slice signatures
and defaults remain compatible, and selected entry caches cannot bypass later
full traversal. Only two upstream source files change. No issue or PR was
published, and no default behavior was silently weakened.

Actual bounded inspection found **v1 indexes in both SWOT Raster and the NISAR
simulated GCOV sample**. SWOT's compressed rank-one coordinates exceed the
current axis-storage profile. NISAR's CF NaN marker accompanies implicit zero
storage initialization, a deliberately rejected ambiguity in the current
reader. Official method-specific redirects are also outside native transport;
the SWOT inspection used existing Earthdata authentication. These are observed
restrictions, not guesses that those products require modern index families.
Stored signed Int16 excludes the larger inspected GOES CMI/radiance files.
See [actual layouts and access evidence](products.md).

Recommend the next narrow milestone: bounded chunked-coordinate acquisition,
followed by actual SWOT Raster qualification through a deliberate archive-access
contract. Resolve upstream implicit-fill semantics before admitting the NISAR
candidate. Keep modern index families and additional pixel types separate until
their own physical/scientific cases are demonstrated. No product adapter,
general HDF5 implementation, merge, tag, push, release, deployment or ChronoZarr
change is included. [Reproduction commands](workflow.md) retain the complete
method and source-validation limits.
"""
    (DOCS / "report.md").write_text(report)
    print("SUMMARY PASS", summary["benchmark_cells"], structure)


if __name__ == "__main__":
    main()
