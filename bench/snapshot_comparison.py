"""Matched source-validation GDAL controls on the frozen real-scene mirror.
Keep reader-only controls separately; use this matrix for the final decision.
"""

import argparse
import copy
import hashlib
import json
from pathlib import Path
import random
import shutil
import statistics
import sys
import time

import acquisition_engine as engine
import real_multiscene as m
from consumer_validation import verify

ROOT = m.ROOT
WORK = m.WORK


def measure(mode, req, folder, server, refs, workers=4):
    if mode.startswith("rust"):
        return m.measure(mode, req, folder, server, refs)
    folder.mkdir(parents=True, exist_ok=False)
    path = folder / "request.json"
    path.write_text(json.dumps(req))
    output = folder / "out"
    command = [
        sys.executable,
        str(ROOT / "bench/gdal_snapshot_worker.py"),
        str(path),
        str(workers),
    ]
    is_file = mode.endswith("file")
    if is_file:
        command.append(str(output))
    server.reset()
    row = m.run(command, folder / "stdout.json")
    row.update(
        mode=mode,
        workers=workers,
        http=copy.deepcopy(server.counts),
        request_records=copy.deepcopy(server.records),
    )
    assert row["exit_code"] == 0, row
    assert not row["http"]["errors"], row
    result = row.pop("result")
    began = time.perf_counter()
    if is_file:
        row["correctness"] = m.check_file(output, req, refs)
        shutil.rmtree(output)
        row["first_chunk_seconds"] = None
        row["first_materialized_seconds"] = result["usable_ttfc_seconds"]
        row["materialization_seconds"] = result["write_seconds"]
    else:
        checked = verify(result["rows"], refs)
        # Same exact rectangle/window coverage requirement as the Rust path.
        x0, y0 = req["spatial"]["bbox"][0], req["spatial"]["bbox"][3]
        col, top = round((x0 - 600000) / 10), round((4000020 - y0) / 10)
        width = round((req["spatial"]["bbox"][2] - x0) / 10)
        height = round((y0 - req["spatial"]["bbox"][1]) / 10)
        expected = {
            (x, y, min(256, col + width - x), min(256, top + height - y))
            for y in range(top, top + height, 256)
            for x in range(col, col + width, 256)
        }
        for asset in req["source"]["assets"]:
            key = (
                asset["id"],
                json.dumps(dict(kind="band", index=1), sort_keys=True),
                (),
            )
            assert set(checked["coverage"][key]) == expected
        row["correctness"] = {k: v for k, v in checked.items() if k != "coverage"}
        row["first_chunk_seconds"] = result["first_chunk_seconds"]
        row["materialization_seconds"] = 0.0
    row["oracle_seconds_excluded"] = time.perf_counter() - began
    for key in [
        "initial_validation_seconds",
        "final_validation_seconds",
        "metadata_worker_seconds",
        "planning_worker_seconds",
        "read_worker_seconds",
        "source_validation",
    ]:
        row[key] = result[key]
    row["passed"] = True
    return row


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--latency-ms", type=float, default=0)
    ap.add_argument("--bandwidth-mbps", type=float, default=0)
    ap.add_argument("--label", required=True)
    ap.add_argument("--workloads", nargs="+")
    args = ap.parse_args()
    if args.reps < 1 or args.latency_ms < 0 or args.bandwidth_mbps < 0:
        ap.error("positive repetitions and nonnegative link settings are required")
    manifest = json.loads((WORK / "sources.json").read_text())
    sources = manifest["sources"]
    mirror = m.Mirror(WORK / "mirror", sources)
    mirror_hash = mirror.verify_pages()
    paths = [
        ROOT / "Cargo.lock",
        ROOT / "consumers/native-stats/Cargo.lock",
        Path(__file__),
        ROOT / "bench/real_multiscene.py",
        ROOT / "bench/gdal_snapshot_worker.py",
        ROOT / "bench/consumer_validation.py",
        ROOT / "bench/acquisition_engine.py",
    ]
    artifact_hashes = {
        str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in paths
    }
    windows = {
        "many-small": (7300, 2100, 96, 96),
        "several-large": (6144, 1024, 2048, 2048),
        "overlap-a": (6868, 1748, 768, 768),
        "overlap-b": (7124, 2004, 768, 768),
    }
    refs = {
        a["id"]: {**a, "format": "cog", "local": mirror.path(f"/{i}.tif")}
        for i, a in enumerate(sources)
    }
    folder = WORK / f"{args.label}-{args.latency_ms:g}ms-{args.bandwidth_mbps:g}mbit"
    folder.mkdir(exist_ok=False)
    server = m.start(mirror, args.latency_ms, args.bandwidth_mbps)
    rows = []
    pilots = []
    rng = random.Random(20261019)
    try:
        for name, win in windows.items():
            if args.workloads and name not in args.workloads:
                continue
            req = m.request(
                sources if name == "many-small" else sources[:6], server, win
            )
            tuning = {}
            for mode in ["gdal-snapshot-stream", "gdal-snapshot-file"]:
                group = []
                for n in [1, 2, 4, 8]:
                    trial = measure(
                        mode, req, folder / f"pilot-{name}-{mode}-{n}", server, refs, n
                    )
                    trial.update(workload=name, pilot=True)
                    pilots.append(trial)
                    group.append(trial)
                tuning[mode] = min(group, key=lambda r: r["wall_seconds"])["workers"]
            for rep in range(args.reps):
                modes = [
                    "rust-stream",
                    "gdal-snapshot-stream",
                    "rust-file",
                    "gdal-snapshot-file",
                ]
                rng.shuffle(modes)
                for mode in modes:
                    try:
                        row = measure(
                            mode,
                            req,
                            folder / f"{name}-{rep}-{mode}",
                            server,
                            refs,
                            tuning.get(mode, 4),
                        )
                    except Exception as error:
                        row = dict(
                            mode=mode,
                            passed=False,
                            error=str(error),
                            http=copy.deepcopy(server.counts),
                        )
                    row.update(workload=name, rep=rep, pilot=False)
                    rows.append(row)
                    with (folder / "runs.jsonl").open("a") as f:
                        f.write(json.dumps(row) + "\n")
                    print(
                        name,
                        rep,
                        mode,
                        "passed" if row["passed"] else "FAILED",
                        round(row.get("wall_seconds", 0), 3),
                        flush=True,
                    )
        summary = []
        metrics = [
            "wall_seconds",
            "first_chunk_seconds",
            "first_materialized_seconds",
            "prepare_seconds",
            "materialization_seconds",
            "peak_rss_bytes",
            "cog_identity_validation_seconds",
            "layout_inspection_seconds",
            "planning_compute_seconds",
            "initial_validation_seconds",
            "final_validation_seconds",
            "metadata_worker_seconds",
            "planning_worker_seconds",
        ]
        for name in windows:
            for mode in [
                "rust-stream",
                "gdal-snapshot-stream",
                "rust-file",
                "gdal-snapshot-file",
            ]:
                group = [r for r in rows if r["workload"] == name and r["mode"] == mode]
                if not group:
                    continue
                ok = [r for r in group if r["passed"]]
                result = dict(
                    workload=name,
                    mode=mode,
                    runs=len(group),
                    failures=len(group) - len(ok),
                )
                for key in metrics:
                    values = [r[key] for r in ok if r.get(key) is not None]
                    result[key] = (
                        dict(
                            median=statistics.median(values),
                            min=min(values),
                            max=max(values),
                        )
                        if values
                        else None
                    )
                result["http_medians"] = (
                    {
                        k: statistics.median(r["http"][k] for r in ok)
                        for k in [
                            "head",
                            "get",
                            "range_get",
                            "conditional",
                            "body_bytes",
                            "head_peak",
                        ]
                    }
                    if ok
                    else None
                )
                summary.append(result)
        report = dict(
            summary=summary,
            pilots=pilots,
            source_manifest=manifest,
            mirror_index_sha256=mirror_hash,
            source_sha256=engine.source_hash(),
            artifact_sha256=artifact_hashes,
            binaries={
                str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in [m.CLI, m.CONSUMER]
            },
            link=dict(latency_ms=args.latency_ms, bandwidth_mbps=args.bandwidth_mbps),
            environment=dict(
                python=sys.version,
                python_gdal=m.rasterio.__gdal_version__,
                rust_gdal=m.subprocess.check_output(
                    ["gdal-config", "--version"], text=True
                ).strip(),
                overrides=m.GDAL_ENV,
            ),
            rss_limit_bytes=192 * 2**20,
            scope="Matched immutable original COG sources, native statistics or native TIFF chunks, pinned conditional reads and initial/pre-read/final source validation; warm local replay, no public throughput claim",
        )
        (folder / "report.json").write_text(json.dumps(report, indent=2))
        print("report", folder / "report.json", flush=True)
        assert all(row["passed"] for row in rows), "failed rows retained"
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
