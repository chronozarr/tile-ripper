"""Four-reader HTTP efficiency replay of frozen immutable Sentinel-2 bytes.

No provider capture, materialized output, production cache, or source mutation.
The preserved 1473c4c binary supplies the native baseline even after source edits.
All consumer timings exclude the independent local scientific oracle.
"""

from __future__ import annotations

import argparse
from collections import defaultdict
import copy
import hashlib
import json
from pathlib import Path
import random
import statistics
import subprocess

import cog_io_trace as trace
import native_request_profile as profile
import real_multiscene as m
import snapshot_comparison as snapshot

ROOT = m.ROOT
WORK = ROOT / "prototype-output/native-http"
BASELINE = ROOT / "docs/native-http/baseline.json"
MODES = ["native-baseline", "native-optimized", "tile-ripper-gdal", "gdal-tuned"]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read_chunks(dest, native):
    value = (dest / "stdout.json").read_text()
    records = (
        [json.loads(s) for s in value.splitlines()]
        if native
        else json.loads(value)["rows"]
    )
    return records


def trial(mode, binary, req, dest, server, refs, layouts, workload, workers, per_host):
    """Retain admission failures as measurements, with the requested budget intact."""
    original_run = m.run
    captured = {}

    def run(command, output, env=None, ndjson=False):
        result = original_run(
            command,
            output,
            env={
                **(env or {}),
                "TILE_RIPPER_WORKERS": str(workers),
                "TILE_RIPPER_PER_HOST": str(per_host),
                "TILE_RIPPER_COG_BACKEND": "native"
                if mode.startswith("native")
                else "gdal",
            },
            ndjson=ndjson,
        )
        captured.update(result)
        return result

    m.run = run
    digest = sha(binary) if binary else None
    native = binary is not None
    try:
        if native:
            m.CONSUMER = binary
            row = m.measure("rust-stream", req, dest, server, refs, workers)
        else:
            row = snapshot.measure(
                "gdal-snapshot-stream", req, dest, server, refs, workers
            )
    except (AssertionError, ValueError, KeyError) as error:
        if not captured:
            raise
        row = {k: v for k, v in captured.items() if k != "result"}
        records = captured.get("result")
        aborted = (
            next(
                (
                    r.get("error")
                    for r in reversed(records)
                    if r.get("type") == "aborted"
                ),
                None,
            )
            if isinstance(records, list)
            else None
        )
        reason = aborted or row.get("error") or str(error)
        row.update(
            passed=False,
            scientific_correctness_passed=False,
            failure=reason[:3000],
            http=copy.deepcopy(server.counts),
            workers=None,
        )
        row["admission_failed"] = bool(
            row["exit_code"]
            and any(
                word in reason
                for word in [
                    "working buffer",
                    "buffer_bytes",
                    "reservation",
                    "working set",
                ]
            )
        )
    finally:
        m.run = original_run
    if binary:
        assert sha(binary) == digest, "binary changed during measured trial"
    records = copy.deepcopy(server.profile_records)
    assert len(records) == row["http"]["head"] + row["http"]["get"], (
        "incomplete server trace"
    )
    if row["passed"] and any(r["method"] == "GET" for r in records):
        summary, physical = profile.summarize_requests(records, layouts, workload)
        assert summary["physical_bytes"]["body_bytes"] == row["http"]["body_bytes"]
    else:
        # A rejected admission fetches metadata but deliberately no full imagery.
        # Keep exact response-body totals and raw ranges without invoking the
        # complete-selected-block assertion required for successful acquisitions.
        body_bytes = sum(
            r["end"] - r["start"] + 1 for r in records if r["method"] == "GET"
        )
        assert body_bytes == row["http"]["body_bytes"]
        physical = dict(totals=dict(body_bytes=body_bytes), partial_trace=True)
        summary = dict(physical_bytes=physical["totals"], partial_trace=True)
    per_connection = defaultdict(list)
    for record in records:
        per_connection[record["connection"]].append(record)
    idle = []
    for sequence in per_connection.values():
        sequence.sort(key=lambda r: r["started"])
        idle.extend(
            max(0, b["started"] - a["started"] - a["seconds"])
            for a, b in zip(sequence, sequence[1:])
        )
    summary["connection_idle_seconds_sum"] = sum(idle)
    summary["connection_idle_seconds_median"] = (
        statistics.median(idle) if idle else None
    )
    summary["connection_idle_seconds_max"] = max(idle) if idle else None
    summary["socket_mode"] = "inherited original TCP_NODELAY"
    (dest / "requests.json").write_text(json.dumps(records, indent=2) + "\n")
    (dest / "physical.json").write_text(json.dumps(physical, indent=2) + "\n")
    row.pop("request_records", None)
    row.pop("stages", None)
    obs = row.get("observed") or {}
    result = captured.get("result")
    internal = (
        result[-1].get("seconds")
        if isinstance(result, list) and result
        else result.get("seconds")
        if isinstance(result, dict)
        else None
    )
    row["consumer_seconds"] = internal if row["passed"] else None
    row["outside_consumer_seconds"] = (
        row["wall_seconds"] - internal
        if row["passed"] and internal is not None
        else None
    )
    row.update(
        backend=mode,
        requested_workers=workers,
        per_host=per_host,
        buffer_bytes=req["budget"]["buffer_bytes"],
        profile=summary,
        binary_sha256=digest,
        admission_failed=row.get("admission_failed", False),
    )
    row["read_decode_seconds"] = obs.get(
        "read_decode_seconds", row.get("read_worker_seconds")
    )
    row["decode_assembly_seconds"] = (
        obs["read_decode_seconds"] - obs["source_retrieval_seconds"]
        if obs.get("source_retrieval_seconds") is not None
        else None
    )
    if native and row["passed"]:
        scheduler = obs["scheduler"]
        assert scheduler["reserved_bytes_peak"] <= req["budget"]["buffer_bytes"]
        row["memory_reservation_gate_passed"] = True
    else:
        # Standalone GDAL has its documented cache/RSS scope, no Rust reservation.
        row["memory_reservation_gate_passed"] = None
    return row


def compact(row):
    return {k: v for k, v in row.items() if k not in ["error", "failure"] or v}


def summarize(rows):
    groups = defaultdict(list)
    for row in rows:
        groups[
            (
                row["latency_ms"],
                row["bandwidth_mbps"],
                row["workload"],
                row["backend"],
                row["comparison_workers"],
                row["buffer_bytes"],
            )
        ].append(row)
    results = []
    for key, group in groups.items():
        latency, bandwidth, workload, mode, workers, budget = key
        result = dict(
            latency_ms=latency,
            bandwidth_mbps=bandwidth,
            workload=workload,
            backend=mode,
            comparison_workers=workers,
            buffer_bytes=budget,
            requested_workers=sorted({r["requested_workers"] for r in group}),
            runs=len(group),
            failures=sum(not r["passed"] for r in group),
            admission_failures=sum(r["admission_failed"] for r in group),
            actual_workers=sorted(
                {r["workers"] for r in group if r.get("workers") is not None}
            ),
        )
        for field in [
            "wall_seconds",
            "consumer_seconds",
            "outside_consumer_seconds",
            "first_chunk_seconds",
            "prepare_seconds",
            "cpu_seconds",
            "peak_rss_bytes",
            "read_decode_seconds",
            "decode_assembly_seconds",
            "layout_inspection_seconds",
            "planning_compute_seconds",
        ]:
            values = [
                r[field] for r in group if r["passed"] and r.get(field) is not None
            ]
            result[field] = (
                dict(median=statistics.median(values), min=min(values), max=max(values))
                if values
                else None
            )
        for field, source in [
            ("http", lambda r: r["http"]),
            ("physical_bytes", lambda r: r["profile"].get("physical_bytes", {})),
            (
                "request_categories",
                lambda r: r["profile"].get("request_categories", {}),
            ),
            ("scheduler", lambda r: (r.get("observed") or {}).get("scheduler", {})),
        ]:
            fields = {
                k
                for row in group
                for k, v in source(row).items()
                if isinstance(v, (float, int)) and not isinstance(v, bool)
            }
            result[field] = {
                k: statistics.median(source(r).get(k, 0) for r in group)
                for k in sorted(fields)
            }
        for field in [
            "tcp_connections",
            "requests_on_reused_connections",
            "peak_concurrent_get",
            "peak_concurrent_payload_get",
        ]:
            values = [r["profile"][field] for r in group if field in r["profile"]]
            result[field] = statistics.median(values) if values else None
        results.append(result)
    return results


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--label", required=True)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--baseline-only", action="store_true")
    ap.add_argument(
        "--native-binary",
        type=Path,
        help="Frozen optimized consumer path; required unless that mode is omitted",
    )
    ap.add_argument(
        "--workloads",
        nargs="+",
        choices=list(trace.WINDOWS),
        default=list(trace.WINDOWS),
    )
    ap.add_argument("--workers", nargs="+", type=int, default=[2])
    ap.add_argument("--budget-mib", nargs="+", type=int, default=[16])
    ap.add_argument("--per-host", type=int, default=4)
    ap.add_argument(
        "--conditions", choices=["both", "unshaped", "shaped"], default="both"
    )
    ap.add_argument("--modes", nargs="+", choices=MODES)
    ap.add_argument("--tuning-workers", nargs="+", type=int, default=[1, 2, 4, 8])
    ap.add_argument("--tuning-from", type=Path)
    ap.add_argument("--output", type=Path)
    args = ap.parse_args()
    assert (
        args.reps > 0 and min(args.workers + args.budget_mib + args.tuning_workers) > 0
    )
    folder = WORK / args.label
    baseline = json.loads(BASELINE.read_text())
    modes = {
        "native-baseline": ROOT
        / baseline["binaries"]["tile-ripper-native-stats"]["path"],
        "tile-ripper-gdal": ROOT
        / baseline["binaries"]["tile-ripper-gdal-stats"]["path"],
        "gdal-tuned": None,
    }
    include_optimized = not args.baseline_only and (
        args.modes is None or "native-optimized" in args.modes
    )
    if args.baseline_only and args.modes and "native-optimized" in args.modes:
        ap.error("native-optimized is unavailable with --baseline-only")
    if include_optimized:
        if args.native_binary is None:
            ap.error("--native-binary must name the frozen optimized consumer")
        modes["native-optimized"] = args.native_binary.resolve()
    if args.modes:
        modes = {k: modes[k] for k in args.modes}
    for mode, name in [
        ("native-baseline", "tile-ripper-native-stats"),
        ("tile-ripper-gdal", "tile-ripper-gdal-stats"),
    ]:
        if mode in modes:
            assert sha(modes[mode]) == baseline["binaries"][name]["sha256"]
    binaries = {k: sha(p) for k, p in modes.items() if p}
    if (
        include_optimized
        and "native-baseline" in binaries
        and binaries["native-optimized"] == binaries["native-baseline"]
    ):
        ap.error("--native-binary matches the frozen baseline; use --baseline-only")
    folder.mkdir(parents=True, exist_ok=False)
    (folder / "native_http_benchmark.py").write_bytes(Path(__file__).read_bytes())
    harness_files = [
        Path(__file__),
        ROOT / "bench/real_multiscene.py",
        ROOT / "bench/snapshot_comparison.py",
        ROOT / "bench/gdal_snapshot_worker.py",
        ROOT / "bench/native_request_profile.py",
        ROOT / "bench/cog_io_trace.py",
        ROOT / "bench/consumer_validation.py",
    ]
    harness_hashes = {str(p.relative_to(ROOT)): sha(p) for p in harness_files}
    sources = json.loads((m.WORK / "sources.json").read_text())["sources"]
    mirror = m.Mirror(m.WORK / "mirror", sources)
    digest = mirror.verify_pages()
    assert digest == baseline["mirror_index_sha256"]
    refs = {
        a["id"]: {**a, "format": "cog", "local": mirror.path(f"/{i}.tif")}
        for i, a in enumerate(sources)
    }
    layouts = {
        f"/{i}.tif": trace.tiff_layout(mirror.path(f"/{i}.tif"))
        for i in range(len(sources))
    }
    frozen_tuning = (
        json.loads(args.tuning_from.read_text())["tuning"] if args.tuning_from else []
    )
    rows, pilots, tuning = [], [], []
    rng = random.Random(20261020)
    conditions = [(0, 0), (8, 100)]
    if args.conditions != "both":
        conditions = [conditions[args.conditions == "shaped"]]
    for latency, mbps in conditions:
        server = profile.start(mirror, latency, mbps)
        try:
            for workload in args.workloads:
                req = m.request(
                    sources if workload == "many-small" else sources[:6],
                    server,
                    trace.WINDOWS[workload],
                )
                tuned = next(
                    (
                        t["workers"]
                        for t in frozen_tuning
                        if t["workload"] == workload and t["latency_ms"] == latency
                    ),
                    None,
                )
                if "gdal-tuned" in modes and tuned is None:
                    trials = []
                    for workers in rng.sample(
                        args.tuning_workers, len(args.tuning_workers)
                    ):
                        dest = folder / f"{latency}-{workload}-pilot{workers}"
                        row = trial(
                            "gdal-tuned",
                            None,
                            req,
                            dest,
                            server,
                            refs,
                            layouts,
                            workload,
                            workers,
                            args.per_host,
                        )
                        row.update(
                            latency_ms=latency,
                            bandwidth_mbps=mbps,
                            workload=workload,
                            rep="pilot",
                        )
                        trials.append(row)
                        pilots.append(row)
                    tuned = min(
                        (r for r in trials if r["passed"]),
                        key=lambda r: r["wall_seconds"],
                    )["workers"]
                tuning.append(
                    dict(
                        workload=workload,
                        latency_ms=latency,
                        bandwidth_mbps=mbps,
                        workers=tuned,
                    )
                )
                canonical = None
                for workers in args.workers:
                    for budget in args.budget_mib:
                        req["budget"]["buffer_bytes"] = budget * 2**20
                        for rep in range(args.reps):
                            order = list(modes)
                            rng.shuffle(order)
                            for mode in order:
                                actual_workers = (
                                    tuned if mode == "gdal-tuned" else workers
                                )
                                dest = (
                                    folder
                                    / f"{latency}-{workload}-w{workers}-b{budget}-r{rep}-{mode}"
                                )
                                row = trial(
                                    mode,
                                    modes[mode],
                                    req,
                                    dest,
                                    server,
                                    refs,
                                    layouts,
                                    workload,
                                    actual_workers,
                                    args.per_host,
                                )
                                row.update(
                                    latency_ms=latency,
                                    bandwidth_mbps=mbps,
                                    workload=workload,
                                    rep=rep,
                                    comparison_workers=workers,
                                )
                                if row["passed"] and modes[mode]:
                                    records = read_chunks(dest, True)
                                    for a, p in zip(
                                        req["source"]["assets"],
                                        records[0]["provenance"]["sources"],
                                    ):
                                        assert p["object_version"] == a["etag"]
                                    normalized = [
                                        {k: v for k, v in c.items() if k != "node_id"}
                                        for c in records[1:-1]
                                    ]
                                    if canonical is None:
                                        canonical = normalized
                                    assert normalized == canonical, (
                                        "public ordered native chunk contract changed"
                                    )
                                rows.append(compact(row))
                                (dest / "measurement.json").write_text(
                                    json.dumps(row, indent=2) + "\n"
                                )
                                with (folder / "runs.jsonl").open("a") as out:
                                    out.write(json.dumps(row) + "\n")
                                print(
                                    latency,
                                    workload,
                                    mode,
                                    f"w{workers}/b{budget}",
                                    rep,
                                    "PASS"
                                    if row["passed"]
                                    else "ADMISSION"
                                    if row["admission_failed"]
                                    else "FAIL",
                                    round(row["wall_seconds"], 3),
                                    row["http"]["get"],
                                    row["http"]["body_bytes"],
                                    flush=True,
                                )
        finally:
            server.shutdown()
            server.server_close()
    assert all(sha(ROOT / path) == digest for path, digest in harness_hashes.items()), (
        "harness changed during matrix"
    )
    assert all(sha(modes[mode]) == digest for mode, digest in binaries.items()), (
        "binary changed during matrix"
    )
    report = dict(
        environment=dict(
            platform=m.platform.platform(),
            python=m.sys.version,
            rasterio=m.rasterio.__version__,
            python_gdal=m.rasterio.__gdal_version__,
            gdal_environment=m.GDAL_ENV,
        ),
        production_sources_sha256_at_finish={
            str(p.relative_to(ROOT)): sha(p)
            for p in sorted((ROOT / "src").rglob("*.rs"))
        },
        manifest_sha256_at_finish={
            str(p.relative_to(ROOT)): sha(p)
            for p in [
                ROOT / "Cargo.toml",
                ROOT / "Cargo.lock",
                ROOT / "consumers/native-stats/Cargo.toml",
                ROOT / "consumers/native-stats/Cargo.lock",
                ROOT / "consumers/native-stats/src/main.rs",
            ]
        },
        working_tree_dirty_at_finish=bool(
            subprocess.check_output(
                ["git", "status", "--porcelain"], cwd=ROOT, text=True
            ).strip()
        ),
        baseline_commit=baseline["commit"],
        source_commit_at_finish=subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        mirror_index_sha256=digest,
        binary_sha256=binaries,
        harness_sha256=harness_hashes,
        raw_directory=str(folder.relative_to(ROOT)),
        repetitions=args.reps,
        randomized_seed=20261020,
        requested_workers=args.workers,
        budget_mib=args.budget_mib,
        per_host=args.per_host,
        timing_scope="Fresh process wall/CPU/RSS. First chunk is process-internal; Python imports excluded from GDAL first chunk. Independent local oracle excluded from all timings. HTTP/1.1 frozen loopback, no TLS; shaped network has 8ms per request and shared 100Mbit/s link.",
        gdal_memory_scope="Standalone GDAL uses a fixed 32MiB raster cache. It does not implement the Rust working reservation; budget is retained in matched request metadata.",
        physical_accounting="Every response range counted, including duplicate metadata, selected/unselected compressed tiles, exact TIFF metadata, padding and other bytes. Range unions are never substituted for transferred byte totals.",
        all_scientific_checks_passed=all(
            r["passed"] or r["admission_failed"] for r in rows
        ),
        all_trials_admitted=not any(r["admission_failed"] for r in rows),
        ordered_chunks_identical=True,
        rows=rows,
        summary=summarize(rows),
        pilots=pilots,
        tuning=tuning,
    )
    output = args.output or folder / "report.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n")
    if output != folder / "report.json":
        (folder / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    assert report["all_scientific_checks_passed"], (
        "scientific or runtime failures retained in report"
    )


if __name__ == "__main__":
    main()
