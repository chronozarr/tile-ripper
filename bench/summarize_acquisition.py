"""Summarize accepted shaped-network runs; no network requests or benchmarks.

Usage: python bench/summarize_acquisition.py prototype-output/acquisition-engine/accepted-final
"""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import random
import statistics

ROOT = Path(__file__).resolve().parents[1]


def median(values):
    return statistics.median(values) if values else None


def interval(reference, current):
    rng = random.Random(20261019)
    estimates = sorted(
        statistics.median(rng.choices(reference, k=len(reference)))
        / statistics.median(rng.choices(current, k=len(current)))
        for _ in range(10000)
    )
    return [estimates[249], estimates[9749]]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("runs", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "docs/acquisition-engine")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    accepted = []
    manifest = []
    summaries = []
    comparisons = []
    for result_path in sorted(args.runs.glob("*/results.json")):
        result = json.loads(result_path.read_text())
        regime = result_path.parent.name
        rows = result["runs"]
        current_hash = result["environment"]["current_sha256"]
        for row in rows:
            if row["reader"].startswith("current") and row["binary_sha256"] != current_hash:
                raise ValueError(f"mixed current binaries in {result_path}")
            row["regime"] = regime
            accepted.append(row)
        manifest.append({
            "regime": regime,
            "environment": result["environment"],
            "invocation": result["invocation"],
            "request_sha256": result["request_sha256"],
            "scope": result["scope"],
            "results_sha256": hashlib.sha256(result_path.read_bytes()).hexdigest(),
            "runs": len(rows),
        })
        for summary in result["summary"]:
            summary = {"regime": regime, **summary}
            good = [r for r in rows if r.get("success") and r["workload"] == summary["workload"] and r["reader"] == summary["reader"]]
            value_floors, traffic_floors, request_floors = [], [], []
            for row in good:
                rate = row["scenario"].get("shared_bandwidth_bytes_per_second")
                physical = row.get("result", {}).get("plan", {}).get("estimates", {}).get("selected_compressed_payload_bytes")
                if physical is None:
                    physical = row.get("result", {}).get("inspection", {}).get("plan", {}).get("estimates", {}).get("selected_compressed_payload_bytes")
                if rate:
                    traffic_floors.append(row["http"]["body_bytes"] / rate)
                    if physical is not None:
                        value_floors.append(physical / rate)
                peak = row["http"].get("peak_active_requests", 0)
                if peak:
                    request_floors.append(row["http"]["attempts"] * row["scenario"]["latency_ms"] / 1000 / peak)
            summary["selected_encoded_data_floor_seconds"] = median(value_floors)
            summary["observed_body_traffic_floor_seconds"] = median(traffic_floors)
            summary["observed_peak_request_floor_seconds"] = median(request_floors)
            summary["floor_caveat"] = "Weak lower bounds for this shaped server; source payload excludes metadata; peak-request bound uses observed peak, not an independently optimized reader. Output, startup and ordered dependencies excluded."
            summaries.append(summary)
        for workload in sorted({r["workload"] for r in rows}):
            current = [r["wall_seconds"] for r in rows if r.get("success") and r["workload"] == workload and r["reader"] == "current-default"]
            if not current:
                continue
            candidates = ["baseline", "current-workers-1"]
            tuned = [s for s in result["summary"] if s["workload"] == workload and s["reader"].startswith("gdal-workers") and s["failures"] == 0]
            if tuned:
                candidates.append(min(tuned, key=lambda s:s["wall_median_seconds"])["reader"])
            else:
                candidates.append("xarray-http-4")
            for reader in candidates:
                reference = [r["wall_seconds"] for r in rows if r.get("success") and r["workload"] == workload and r["reader"] == reader]
                if not reference:
                    continue
                ratio = statistics.median(reference) / statistics.median(current)
                ci = interval(reference, current)
                comparisons.append({"regime": regime, "workload": workload,
                                    "reference": reader, "current": "current-default",
                                    "current_n": len(current), "reference_n": len(reference),
                                    "reference_seconds": statistics.median(reference),
                                    "current_seconds": statistics.median(current),
                                    "reference_over_current": ratio,
                                    "bootstrap_95_percentile_interval": ci,
                                    "observation": "lower current median" if ratio > 1.1 and ci[0] > 1 else "higher current median" if ratio < 1/1.1 and ci[1] < 1 else "no clear difference under 10% rule",
                                    "caveat": "One workstation/session per simulated regime. Tuned baseline chosen from 1/4/8 sweep; reader-only controls omit Tile-Ripper source pinning. No public-data or universal speed claim."})
    if not accepted:
        raise ValueError("No completed accepted results found")
    with gzip.GzipFile(filename=str(args.output / "accepted-runs.jsonl.gz"), mode="wb", mtime=0) as stream:
        for row in accepted:
            stream.write((json.dumps(row, separators=(",", ":")) + "\n").encode())
    result = {"runs": len(accepted), "successes": sum(bool(r.get("success")) for r in accepted),
              "failures": sum(not r.get("success") for r in accepted),
              "manifest": manifest, "summary": summaries, "comparisons": comparisons}
    (args.output / "benchmarks.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key:result[key] for key in ["runs", "successes", "failures"]}))


if __name__ == "__main__":
    main()
