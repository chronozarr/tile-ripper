"""Compact diagnostic evidence and summarize matched source-validation runs.

The retained full receipts are audit artifacts, never read by acquisition.
"""

import hashlib
import json
import shutil
import statistics
from collections import defaultdict
from pathlib import Path

from grid_inspection_receipts import compact
from source_validation_wire import DOCS, WORK, digest, write


def phases(transport):
    counts = {"metadata": 0, "payload": 0, "authorization_proof": 0}
    for source in transport.values():
        for phase, operations in source["phases"].items():
            key = "payload" if phase.endswith("_payload") else "metadata"
            counts[key] += sum(v["bytes"] for v in operations.values())
    return counts


def diagnostic_compact(value, key=""):
    if isinstance(value, list):
        return [diagnostic_compact(v) for v in value]
    if not isinstance(value, dict):
        return value
    if key == "raw" and "receipt" in value:
        receipt = value["receipt"]
        return {
            "full_json_sha256": hashlib.sha256(
                json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest(),
            "full_diagnostics": "see receipt-index.json",
            **{
                k: value[k]
                for k in [
                    "binary_sha256",
                    "stdout_sha256",
                    "stderr_sha256",
                    "output",
                    "publication_stages",
                ]
            },
            "status": receipt.get("status", receipt.get("document_type")),
            "admission": receipt.get("admission"),
            "observations": [
                {
                    k: o[k]
                    for k in ["id", "status", "working_memory", "job_final_validation"]
                    if k in o
                }
                | {
                    "producer_timing": {
                        k: o.get("observed", {}).get(k)
                        for k in [
                            "planning_seconds",
                            "first_output_seconds",
                            "read_decode_seconds",
                        ]
                    }
                }
                for o in receipt.get("observations", [])
            ],
        }
    if all(k in value for k in ["phases", "requests", "bytes"]):
        return {
            k: diagnostic_compact(v, k)
            for k, v in value.items()
            if k not in ["objects", "ranges"]
        }
    return {k: diagnostic_compact(v, k) for k, v in value.items()}


def annotate_traces():
    for label, binding_heads in [("baseline", 2), ("candidate", 1)]:
        events = json.loads(
            (DOCS / f"trace-authenticated-swot-{label}.json").read_text()
        )
        heads = 0
        annotated = []
        boundaries = {
            1: "ObjectFile::open: bind strong ETag and size",
            binding_heads + 1: "prepare_on / Sources::seal: completed preparation",
            binding_heads + 2: "Prepared::execute_swot_cancellable: before execution",
            binding_heads
            + 3: "Prepared::execute_swot_cancellable: after provisional delivery",
        }
        if binding_heads == 2:
            boundaries[2] = (
                "ObjectFile::open / Http::seal: adjacent binding recheck (removed)"
            )
        for sequence, event in enumerate(events, 1):
            if event["method"] == "HEAD":
                if event["role"] == "resource":
                    heads += 1
                    number = heads
                else:
                    number = heads + 1
                operation = boundaries[number]
                guarantee = (
                    "resolve exact HEAD route"
                    if event["role"] == "archive"
                    else "live strong ETag and full-size identity comparison"
                )
            elif event["role"] != "resource":
                operation = (
                    "AuthenticatedHttp::send: exact-method login / GET route resolution"
                )
                guarantee = "configured origins and paths; login-only credentials; bounded method-specific route expiry"
            else:
                operation = (
                    "ObjectFile::read: selected scientific payload"
                    if heads >= binding_heads + 2
                    else "ObjectFile::read: native metadata, index or coordinate inspection"
                )
                guarantee = "206, exact pinned ETag, range and total size, bounded identity-encoded body and exact length"
            annotated.append(
                {
                    "sequence": sequence,
                    **event,
                    "operation": operation,
                    "guarantee": guarantee,
                    "reusable_for_immediate_seal": event["method"] == "HEAD"
                    and event["role"] == "resource"
                    and heads == 1,
                }
            )
        write(DOCS / f"lifecycle-authenticated-swot-{label}.json", annotated)

        events = json.loads((DOCS / f"trace-cog-{label}.json").read_text())
        heads, annotated = 0, []
        boundaries = {
            1: "stac_workflow::resolve_identities: pin ETag",
            2: "RangeSource::open: bind parser size and ETag",
            3: "stac_workflow::validate_identities: before execution",
            4: "RangeSource::open: compare prepared parser snapshot identity and size",
            5: "stac_workflow::validate_identities: after provisional delivery",
        }
        for sequence, event in enumerate(events, 1):
            if event["method"] == "HEAD":
                heads += 1
                operation = boundaries[heads]
                guarantee = (
                    "live ETag identity; size is bound/compared by native opens"
                    if heads in [1, 3, 5]
                    else "native ETag and full-size binding / snapshot check"
                )
            else:
                operation = (
                    "RangeSource::read_exact_at: TIFF parser page"
                    if heads == 2
                    else "RangeSource::exact: selected tile decoding"
                )
                guarantee = "conditional exact ETag and Content-Range total; exact bounded body length"
            annotated.append(
                {
                    "sequence": sequence,
                    **event,
                    "operation": operation,
                    "guarantee": guarantee,
                }
            )
        write(DOCS / f"lifecycle-anonymous-cog-{label}.json", annotated)


def run():
    auth = json.loads((WORK / "authenticated-benchmarks-full.json").read_text())
    anonymous = json.loads((DOCS / "anonymous-benchmarks.json").read_text())
    rows = auth["runs"] + anonymous["runs"]
    for row in rows:
        if "raw" in row:
            for key in [
                "cpu_seconds",
                "sampled_process_peak_rss_bytes",
                "binary_sha256",
            ]:
                row[key] = row["raw"][key]
            row["peak_rss_bytes"] = row["raw"]["sampled_process_peak_rss_bytes"]
        if row["source"] in ["three_source_generation", "three_source_plan"]:
            row["source_bytes"] = {
                "metadata": row["wire"]["source_body_bytes"],
                "payload": 0,
                "authorization_proof": 0,
            }
        elif row["source"] == "three_source_fetch":
            row.update(
                {
                    k: row["raw"]["receipt"]["observed"][k]
                    for k in [
                        "first_observation_seconds",
                        "final_source_sweep_seconds",
                        "working_reservation_peak_bytes",
                    ]
                }
            )
            row["source_planning_seconds"] = [
                o["observed"]["planning_seconds"]
                for o in row["raw"]["receipt"]["observations"]
            ]
            row["source_first_output_seconds"] = [
                o["observed"]["first_output_seconds"]
                for o in row["raw"]["receipt"]["observations"]
            ]
            counts = {"metadata": 0, "payload": 0, "authorization_proof": 0}
            for observation in row["raw"]["receipt"]["observations"]:
                current = phases(observation["observed"]["transport"])
                for key in counts:
                    counts[key] += current[key]
                validation = observation["job_final_validation"]["transport"]
                counts["authorization_proof"] += validation["bytes"]
            assert sum(counts.values()) == row["wire"]["source_body_bytes"]
            row["source_bytes"] = counts
        elif "source_bytes" not in row:
            row["source_bytes"] = {
                "metadata": row["wire"]["metadata_body_bytes"],
                "payload": row["wire"]["payload_body_bytes"],
                "authorization_proof": 0,
            }
            assert sum(row["source_bytes"].values()) == row["wire"]["source_body_bytes"]

    # Sum the ordinary independent generation/plan/fetch operations per repeat.
    workflow = defaultdict(list)
    for row in rows:
        if row["source"] in [
            "three_source_generation",
            "three_source_plan",
            "three_source_fetch",
        ]:
            workflow[
                (row["implementation"], row["delay_seconds"], row["repeat"])
            ].append(row)
    for (implementation, delay, repeat), group in workflow.items():
        assert len(group) == 3
        rows.append(
            {
                "source": "three_source_complete_workflow",
                "implementation": implementation,
                "delay_seconds": delay,
                "repeat": repeat,
                "wall_seconds": sum(r["wall_seconds"] for r in group),
                "cpu_seconds": sum(r["cpu_seconds"] for r in group),
                "peak_rss_bytes": max(r["peak_rss_bytes"] for r in group),
                "wire": {
                    key: sum(r["wire"][key] for r in group)
                    for key in group[0]["wire"]
                    if isinstance(group[0]["wire"][key], int)
                },
                "source_bytes": {
                    key: sum(r["source_bytes"][key] for r in group)
                    for key in group[0]["source_bytes"]
                },
            }
        )
    grouped = defaultdict(list)
    for row in rows:
        grouped[(row["source"], row["delay_seconds"], row["implementation"])].append(
            row
        )
    summaries = []
    for (source, delay, implementation), group in sorted(grouped.items()):
        assert len(group) == 3, (source, delay, implementation, len(group))
        wire_counts = [
            {k: v for k, v in r["wire"].items() if isinstance(v, int)} for r in group
        ]
        assert all(w == wire_counts[0] for w in wire_counts)
        assert all(r["source_bytes"] == group[0]["source_bytes"] for r in group)
        timings = {}
        for key in [
            "wall_seconds",
            "prepare_seconds",
            "first_chunk_seconds",
            "seconds",
            "generation_seconds",
            "cpu_seconds",
            "peak_rss_bytes",
            "first_observation_seconds",
            "final_source_sweep_seconds",
            "working_reservation_peak_bytes",
        ]:
            values = [r[key] for r in group if r.get(key) is not None]
            if values:
                timings[key] = {
                    "median": statistics.median(values),
                    "min": min(values),
                    "max": max(values),
                }
        summaries.append(
            {
                "source": source,
                "delay_seconds": delay,
                "implementation": implementation,
                "repetitions": len(group),
                "wire": wire_counts[0],
                "source_bytes": group[0]["source_bytes"],
                "measurements": timings,
                "working_memory": group[0].get("working_memory"),
            }
        )
    write(
        DOCS / "summary.json",
        {
            "baseline": "157facc",
            "repetitions": 3,
            "added_delay_per_wire_request_seconds": [0, 0.025, 0.1],
            "platforms": {
                "authenticated": "macOS ARM64",
                "anonymous": "Linux ARM64, native GDAL-free binaries",
            },
            "source_body_bytes_exclude": "HTTP/TLS framing and redirect bodies; fixture-observed bytes, reconciled with application phases",
            "all_scientific_fingerprints_equal": True,
            "results": summaries,
        },
    )
    write(
        DOCS / "authenticated-benchmarks.json",
        {
            "runs": auth["runs"],
            "scientific_output_sha256": auth["scientific_output_sha256"],
        },
    )

    annotate_traces()
    raw = WORK / "full-receipts"
    previous_path = DOCS / "receipt-index.json"
    previous = (
        {r["receipt"]: r for r in json.loads(previous_path.read_text())["receipts"]}
        if previous_path.exists()
        else {}
    )
    index = []
    for path in sorted(DOCS.rglob("*.json")):
        if path.name in [
            "summary.json",
            "verification.json",
            "receipt-index.json",
            "feature-matrix.json",
            "fixtures.json",
        ]:
            continue
        relative = path.relative_to(DOCS)
        retained = previous.get(str(relative))
        if retained and digest(path) == retained["compact_sha256"]:
            original = Path(retained["full_receipt"])
            assert digest(original) == retained["full_sha256"]
        else:
            original = raw / digest(path) / relative
            original.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, original)
        result = diagnostic_compact(compact(json.loads(original.read_text())))
        encoded = json.dumps(result, indent=2, allow_nan=False)
        if len(encoded) > 32768:
            encoded = json.dumps(result, separators=(",", ":"), allow_nan=False)
        path.write_text(encoded + "\n")
        index.append(
            {
                "receipt": str(relative),
                "full_receipt": str(original),
                "full_sha256": digest(original),
                "compact_sha256": digest(path),
            }
        )
    write(
        DOCS / "receipt-index.json",
        {
            "scope": "diagnostic receipts only; not source byte or identity cache",
            "receipts": index,
        },
    )
    print(f"Summarized {len(rows)} runs; retained {len(index)} full receipts")


if __name__ == "__main__":
    run()
