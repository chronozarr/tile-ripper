"""Derive compact matched medians and range coverage from retained receipts."""

import json
import statistics
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs/native-hdf5"
WORK = ROOT / "prototype-output/native-hdf5"


def coverage(spans):
    spans = sorted(spans)
    union = 0
    end = 0
    total = 0
    for start, stop, count in spans:
        total += (stop - start) * count
        union += max(0, stop - max(start, end))
        end = max(end, stop)
    return {
        "response_range_bytes": total,
        "unique_range_bytes": union,
        "repeated_range_bytes": total - union,
    }


def native_coverage(receipt):
    transport = receipt["runs"][0]["observed"]["transport"]["science"]
    datasets = receipt["inspection"]["sources"][0]["object_identities"]["datasets"]
    payload = {
        (g["start"], g["start"] + g["len"])
        for d in datasets
        for g in d["physical_groups"]
        if g["len"]
    }
    all_spans = []
    payload_spans = []
    for key, value in transport["ranges"].items():
        if not value["statuses"].get("206"):
            continue
        start, end = map(int, key.rsplit(" ", 1)[1].split(":"))
        entry = (start, end, value["statuses"]["206"])
        all_spans.append(entry)
        if (start, end) in payload:
            payload_spans.append(entry)
    return {"all": coverage(all_spans), "payload": coverage(payload_spans)}


def main():
    benchmark = json.loads((DOCS / "benchmark.json").read_text())
    rows = []
    for latency in [0, 0.04]:
        for selection in ["sparse", "dense"]:
            cells = [
                c
                for c in benchmark["cells"]
                if c["latency_seconds"] == latency and c["selection"] == selection
            ]
            for label, predicate in [
                *[
                    (p, lambda c, p=p: c.get("policy") == p)
                    for p in ["individual", "adjacent", "bounded"]
                ],
                *[
                    (
                        f"h5py-{p}",
                        lambda c, p=p: (
                            c["reader"] == "h5py/libhdf5 range fileobj"
                            and c["page_bytes"] == p
                        ),
                    )
                    for p in [0, 4096, 65536]
                ],
                (
                    "full-file",
                    lambda c: c["reader"] == "explicit-full-file-h5py-control",
                ),
            ]:
                group = [c for c in cells if predicate(c)]
                assert len(group) == 3, (latency, selection, label)
                first = group[0]
                row = {
                    "latency_ms": latency * 1000,
                    "selection": selection,
                    "reader": label,
                    "completion_seconds_median": statistics.median(
                        c["completion_seconds"] for c in group
                    ),
                    "first_values_seconds_median": statistics.median(
                        c["first_scientific_values_seconds"] for c in group
                    ),
                    "metadata_seconds_median": statistics.median(
                        c.get(
                            "initial_metadata_index_seconds",
                            c.get("initial_metadata_coordinate_seconds"),
                        )
                        for c in group
                    ),
                    "cpu_seconds_median": statistics.median(
                        c.get("child_cpu_seconds", c.get("cpu_seconds")) for c in group
                    ),
                    "peak_rss_bytes_max": max(c["peak_rss_bytes"] for c in group),
                    "gets": first.get("server", {}).get("gets", first.get("gets")),
                    "heads": first.get("server", {}).get("heads", first.get("heads")),
                    "response_bytes": first.get("server", {}).get(
                        "response_bytes", first.get("bytes")
                    ),
                }
                if "policy" in first:
                    receipt = json.loads(
                        (
                            WORK
                            / "runs"
                            / f"bench-{selection}-{latency}-{first['repeat']}-{label}"
                            / "stdout.json"
                        ).read_text()
                    )
                    row.update(
                        {
                            "selected_compressed_bytes": first[
                                "selected_compressed_bytes"
                            ],
                            "payload_response_bytes": first["payload_bytes"],
                            "payload_gets": first["payload_gets"],
                            "range_coverage": native_coverage(receipt),
                            "working_bytes_peak": max(
                                c["scheduler"]["reserved_bytes_peak"] for c in group
                            ),
                        }
                    )
                rows.append(row)
    public = json.loads((WORK / "runs/public-real/stdout.json").read_text())
    summary = {
        "fixture_bytes": benchmark["fixture_bytes"],
        "repetitions": 3,
        "native_workers": 1,
        "cells": rows,
        "public_coverage": native_coverage(public),
    }
    (DOCS / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    text = [
        "# Matched HDF5 acquisition measurements",
        "",
        f"Frozen synthetic source: {benchmark['fixture_bytes']:,} bytes, UInt16, shape 768 × 1024, 64 × 64 gzip chunks. Sparse selection: 16 samples across 16 chunks. Dense selection: 196,608 samples across 48 chunks. Every native value, validity bit and coordinate matches h5py and netCDF4. Independent h5py workers read the same selected axes and preserve scientific attributes.",
        "",
        "Medians of three alternating native-policy repetitions. One native worker and one independent h5py process; 40 ms is controlled per-request latency, with unconstrained loopback bandwidth. Times include initial metadata/coordinates, payloads and source validation. Native metadata timing includes index discovery; h5py defers some index work until the value read. Process startup/imports are outside the internal acquisition timer. The native timed verification consumer dumps per-chunk bytes/masks; Python returns an array and hashes it. Low-latency and CPU comparisons include this consumer-work difference; native policy comparisons keep that work fixed. Retrieval/decode stages are separately retained. CPU/RSS scopes differ explicitly in raw receipts.",
        "",
    ]
    for latency in [0, 40]:
        text += [
            f"## {latency} ms added request latency",
            "",
            "| Selection | Reader | Completion (s) | GET / HEAD | Response bytes | Max RSS (MiB) |",
            "| --- | --- | ---: | ---: | ---: | ---: |",
        ]
        for r in rows:
            if r["latency_ms"] == latency:
                text.append(
                    f"| {r['selection']} | {r['reader']} | {r['completion_seconds_median']:.4f} | {r['gets']} / {r['heads']} | {r['response_bytes']:,} | {r['peak_rss_bytes_max'] / 2**20:.1f} |"
                )
        text += [""]
    dense = {
        r["reader"]: r
        for r in rows
        if r["latency_ms"] == 40 and r["selection"] == "dense"
    }
    speed = (
        dense["individual"]["completion_seconds_median"]
        / dense["adjacent"]["completion_seconds_median"]
    )
    text += [
        f"Adjacent coalescing improves the dense 40 ms case by {speed:.2f}× over native individual reads, with zero payload gap overfetch. Exact native payload/index range repeats are absent; small overlapping header-prefix reads remain visible in byte-union coverage. The bounded-gap control in resources.json reduces eight payload GETs to one with 7,680 overfetch bytes (11.71% of selected compressed bytes).",
        "",
        "Tuned h5py remains faster in the high-latency cases. Its larger windows trade extra bytes for fewer requests; the 64 KiB strategy can cover most or more than one source-file length after overlapping reads. The explicit full-file control is fastest on this small source and fast link. That control is a useful alternative when transfer cost, memory and file size permit; it is never a native fallback.",
        "",
        "Native index discovery scans all v1 B-tree leaves. This metadata cost, small signature/header reads, and repeated parsing of cached index bytes limit latency and dense decode efficiency. The priority is established-reader index pruning and metadata locality. These measurements support a useful selective payload profile and do not establish general throughput or memory leadership.",
        "",
        "summary.json retains metadata/first-value/CPU/RSS/reservation medians and byte-union accounting; benchmark.json retains all cells and transport phases. resources.json retains bounded-gap, memory, corruption and index-discovery controls.",
    ]
    (DOCS / "performance.md").write_text("\n".join(text) + "\n")


if __name__ == "__main__":
    main()
