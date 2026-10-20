"""Derive HTTP critical-path and resource receipts from saved benchmark trials.

This analysis never starts a consumer, server, build, provider request, or timing
trial. It binds the benchmark inputs, raw records it reads, and its own source.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import statistics

ROOT = Path(__file__).resolve().parents[1]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def merged(intervals):
    result = []
    for left, right in sorted(intervals):
        if result and left <= result[-1][1]:
            result[-1][1] = max(result[-1][1], right)
        else:
            result.append([left, right])
    return result


def duration(intervals):
    return sum(right - left for left, right in merged(intervals))


def destination(report, row):
    raw = ROOT / report["raw_directory"]
    assert raw.resolve().is_relative_to(ROOT)
    budget = row["buffer_bytes"] // 2**20
    return raw / (
        f"{row['latency_ms']}-{row['workload']}"
        f"-w{row['comparison_workers']}-b{budget}"
        f"-r{row['rep']}-{row['backend']}"
    )


def load_bound(path, inputs):
    inputs[str(path.relative_to(ROOT))] = sha(path)
    return json.loads(path.read_text())


def critical_path(report, benchmark_path):
    rows, inputs = [], {}
    for row in report["rows"]:
        if row["latency_ms"] != 8 or row["workload"] != "overlap-b":
            continue
        folder = destination(report, row)
        records = load_bound(folder / "requests.json", inputs)
        physical = load_bound(folder / "physical.json", inputs)
        # Bind the raw process row as well as the public report used below.
        load_bound(folder / "measurement.json", inputs)
        started = min(r["started"] for r in records)
        finished = max(r["started"] + r["seconds"] for r in records)
        lookup = {(r["source"], r["start"], r["end"]): r for r in physical["requests"]}
        gets = [r for r in records if r["method"] == "GET"]
        payload = [
            r
            for r in gets
            if lookup[(r["source"], r["start"], r["end"])].get(
                "selected_payload_bytes", 0
            )
        ]
        metadata = [r for r in gets if r not in payload]

        def full(record):
            return record["started"], record["started"] + record["seconds"]

        def body(record):
            return (
                record["started"] + record["header_seconds"],
                record["started"] + record["seconds"],
            )

        events = []
        for record in payload:
            events.extend(
                [(record["started"], 1), (record["started"] + record["seconds"], -1)]
            )
        active, last, by_count = 0, started, {}
        for instant, change in sorted(events):
            by_count[active] = by_count.get(active, 0) + instant - last
            active += change
            last = instant
        by_count[0] = by_count.get(0, 0) + finished - last
        sources = []
        for key in sorted({r["source"] for r in payload}):
            selected = [r for r in payload if r["source"] == key]
            intervals = merged([full(r) for r in selected])
            sources.append(
                dict(
                    source=key,
                    payload_start=min(r["started"] for r in selected) - started,
                    payload_end=max(r["started"] + r["seconds"] for r in selected)
                    - started,
                    payload_requests=len(selected),
                    payload_service_seconds_sum=sum(r["seconds"] for r in selected),
                    within_source_between_payload_gaps_seconds=[
                        max(0, right[0] - left[1])
                        for left, right in zip(intervals, intervals[1:])
                    ],
                )
            )
        rows.append(
            dict(
                backend=row["backend"],
                rep=row["rep"],
                wall_seconds=row["wall_seconds"],
                consumer_seconds=row["consumer_seconds"],
                outside_consumer_seconds=row["outside_consumer_seconds"],
                request_span_seconds=finished - started,
                source_byte_lower_bound_seconds=row["http"]["body_bytes"]
                / (row["bandwidth_mbps"] * 1e6 / 8),
                process_cpu_seconds=row["cpu_seconds"],
                prepare_seconds=row.get("prepare_seconds"),
                first_chunk_seconds=row["first_chunk_seconds"],
                get_active_union_seconds=duration([full(r) for r in gets]),
                payload_active_union_seconds=duration([full(r) for r in payload]),
                payload_body_interval_union_seconds=duration(
                    [body(r) for r in payload]
                ),
                all_get_body_interval_union_seconds=duration([body(r) for r in gets]),
                no_http_request_active_in_request_span_seconds=(finished - started)
                - duration([full(r) for r in records]),
                payload_concurrency_seconds=by_count,
                metadata_gets=len(metadata),
                payload_gets=len(payload),
                metadata_service_seconds_sum=sum(r["seconds"] for r in metadata),
                payload_service_seconds_sum=sum(r["seconds"] for r in payload),
                decode_assembly_worker_seconds_sum=row.get("decode_assembly_seconds"),
                read_decode_worker_seconds_sum=row.get("read_decode_seconds"),
                sources=sources,
            )
        )
    summary = []
    for mode in sorted({r["backend"] for r in rows}):
        group = [r for r in rows if r["backend"] == mode]
        result = dict(backend=mode, runs=len(group))
        for key in group[0]:
            values = [
                r[key]
                for r in group
                if isinstance(r.get(key), (int, float)) and key != "rep"
            ]
            if values:
                result[key] = dict(
                    median=statistics.median(values), min=min(values), max=max(values)
                )
        keys = {k for r in group for k in r["payload_concurrency_seconds"]}
        result["payload_concurrency_seconds_medians"] = {
            str(k): statistics.median(
                r["payload_concurrency_seconds"].get(k, 0) for r in group
            )
            for k in sorted(keys)
        }
        summary.append(result)
    handoffs = []
    for row in rows:
        if row["backend"] != "native-optimized":
            continue
        sources = {s["source"]: s for s in row["sources"]}
        gaps = [
            sources[f"/{i + 2}.tif"]["payload_start"]
            - sources[f"/{i}.tif"]["payload_end"]
            for i in range(4)
        ]
        row["source_i_end_to_source_i_plus_two_first_payload_seconds"] = gaps
        handoffs.extend(gaps)
    by_backend = {s["backend"]: s for s in summary}
    saved_metadata_gets = (
        by_backend["native-baseline"]["metadata_gets"]["median"]
        - by_backend["native-optimized"]["metadata_gets"]["median"]
    )
    return dict(
        scope="Untimed interval analysis of every saved final shaped overlap-B trial. "
        "Run counts are retained per reader. Full wall remains primary. Transfer-body intervals are "
        "server observations, not a directly instrumented link-utilization counter.",
        benchmark_json_sha256=sha(benchmark_path),
        analysis_source_sha256=sha(Path(__file__)),
        raw_inputs_sha256=inputs,
        raw_directory=report["raw_directory"],
        queue_wait_seconds=None,
        queue_wait_limit="Worker-side rendezvous wait is not separately instrumented. "
        "Consumer sink_seconds measures callback work and does not measure that wait. "
        "No exact CPU-versus-queue attribution is claimed.",
        worker_time_limit="Read/decode and retrieval are sums of overlapping worker "
        "elapsed times, not process CPU time or additive wall phases.",
        metadata_gets_saved=saved_metadata_gets,
        injected_metadata_service_seconds_saved=saved_metadata_gets * 0.008,
        causal_fraction_limit="Fewer metadata GETs remove the recorded injected "
        "per-request service time in aggregate; request overlap prevents treating "
        "that sum as an additive wall saving or causal fraction of the observed gain.",
        optimized_source_handoff_gap_proxy_seconds=dict(
            median=statistics.median(handoffs),
            min=min(handoffs),
            max=max(handoffs),
            scope="Observed timestamp difference from source i final payload response "
            "end to source i+2 first payload request start. Includes decode/delivery, "
            "validation/open and scheduling; not an isolated queue-wait measurement.",
        ),
        summary=summary,
        rows=rows,
    )


def resource_summary(main, resource, benchmark_path, resource_path):
    result = {}
    for label, report in [("main", main), ("resource", resource)]:
        rows = [
            r
            for r in report["rows"]
            if r["backend"] == "native-optimized" and r["passed"]
        ]
        buffers = [
            b for r in rows for b in r["observed"]["scheduler"]["native_buffers"]
        ]
        reservations = [r["observed"]["scheduler"]["reserved_bytes_peak"] for r in rows]
        result[label] = dict(
            admitted_optimized_trials=len(rows),
            reservation_peak_range_bytes=[min(reservations), max(reservations)],
            transport_encoded_capacity_peak_range_per_reader_bytes=[
                min(b["transport_encoded_capacity_peak_bytes"] for b in buffers),
                max(b["transport_encoded_capacity_peak_bytes"] for b in buffers),
            ],
            adapter_delivery_capacity_peak_range_per_reader_bytes=[
                min(b["adapter_delivery_capacity_peak_bytes"] for b in buffers),
                max(b["adapter_delivery_capacity_peak_bytes"] for b in buffers),
            ],
            all_reservations_within_declared_budget=all(
                r["observed"]["scheduler"]["reserved_bytes_peak"] <= r["buffer_bytes"]
                for r in rows
            ),
            row_memory_scope=rows[0]["observed"]["scheduler"]["memory_scope"],
        )
    failed = [r for r in resource["rows"] if not r["passed"]]
    complete_checks, inputs = [], {}
    for row in failed:
        path = destination(resource, row) / "stdout.json"
        inputs[str(path.relative_to(ROOT))] = sha(path)
        records = [json.loads(line) for line in path.read_text().splitlines()]
        complete_checks.append(
            records[-1]["type"] == "aborted"
            and not any(record["type"] == "complete" for record in records)
        )
    result["resource_rejections"] = dict(
        total=len(failed),
        all_expected_admission=all(r["admission_failed"] for r in failed),
        by_backend={
            mode: sum(r["backend"] == mode for r in failed)
            for mode in ["native-baseline", "native-optimized"]
        },
        all_budget_8mib=all(r["buffer_bytes"] == 8 * 2**20 for r in failed),
        all_metadata_gets_12=all(r["http"]["get"] == 12 for r in failed),
        all_metadata_body_49152_bytes=all(
            r["http"]["body_bytes"] == 49152 for r in failed
        ),
        no_successful_complete_record=all(complete_checks),
        scopes=[
            {
                key: r[key]
                for key in [
                    "backend",
                    "workload",
                    "latency_ms",
                    "buffer_bytes",
                    "failure",
                ]
            }
            | dict(workers=r["comparison_workers"])
            for r in failed
        ],
    )
    result["resource_selected_shape_rows"] = [
        dict(
            backend=r["backend"],
            workload=r["workload"],
            workers=r["comparison_workers"],
            buffer_bytes=r["buffer_bytes"],
            wall_seconds=r["wall_seconds"],
            active_workers_peak=r.get("observed", {})
            .get("scheduler", {})
            .get("active_workers_peak"),
            peak_concurrent_payload_get=r["profile"].get("peak_concurrent_payload_get"),
            reservation_peak_bytes=r.get("observed", {})
            .get("scheduler", {})
            .get("reserved_bytes_peak"),
            get=r["http"]["get"],
            passed=r["passed"],
        )
        for r in resource["rows"]
        if r["latency_ms"] == 8
    ]
    result["source_reports_sha256"] = {
        str(path.relative_to(ROOT)): sha(path)
        for path in [benchmark_path, resource_path]
    }
    result["analysis_source_sha256"] = sha(Path(__file__))
    result["raw_inputs_sha256"] = inputs
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--benchmark", type=Path, default=ROOT / "docs/native-http/benchmark.json"
    )
    parser.add_argument(
        "--resource-benchmark",
        type=Path,
        default=ROOT / "docs/native-http/resource-benchmark.json",
    )
    parser.add_argument("--output-dir", type=Path, default=ROOT / "docs/native-http")
    args = parser.parse_args()
    benchmark_path = args.benchmark.resolve()
    resource_path = args.resource_benchmark.resolve()
    benchmark = json.loads(benchmark_path.read_text())
    resource = json.loads(resource_path.read_text())
    critical = critical_path(benchmark, benchmark_path)
    resources = resource_summary(benchmark, resource, benchmark_path, resource_path)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    for name, receipt in [
        ("critical-path.json", critical),
        ("resource-summary.json", resources),
    ]:
        (args.output_dir / name).write_text(json.dumps(receipt, indent=2) + "\n")
    assert all(
        resources[k]["all_reservations_within_declared_budget"]
        for k in ["main", "resource"]
    )
    assert resources["resource_rejections"]["all_expected_admission"]
    assert resources["resource_rejections"]["no_successful_complete_record"]
    print(
        "Derived critical-path and resource-summary receipts; no timed runs performed."
    )


if __name__ == "__main__":
    main()
