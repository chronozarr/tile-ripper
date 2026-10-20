"""Three readers, same frozen bytes/native science/source versions. No capture."""

import argparse
import hashlib
import json
from pathlib import Path
import random
import statistics

import real_multiscene as m
import snapshot_comparison as snapshot
import cog_io_trace as trace

WORK = m.ROOT / "prototype-output/native-cog"
WINDOWS = trace.WINDOWS


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--label", default="decision")
    ap.add_argument("--local", action="store_true")
    ap.add_argument("--workloads", nargs="+", default=list(WINDOWS))
    args = ap.parse_args()
    folder = WORK / args.label
    folder.mkdir(parents=True, exist_ok=False)
    sources = json.loads((m.WORK / "sources.json").read_text())["sources"]
    mirror = m.Mirror(m.WORK / "mirror", sources)
    digest = mirror.verify_pages()
    refs = {
        a["id"]: {**a, "format": "cog", "local": mirror.path(f"/{i}.tif")}
        for i, a in enumerate(sources)
    }
    layouts = {
        f"/{i}.tif": trace.tiff_layout(mirror.path(f"/{i}.tif"))
        for i in range(len(sources))
    }
    modes = {
        "native": WORK / "native/tile-ripper-native-stats",
        "tile-ripper-gdal": WORK / "baseline/tile-ripper-native-stats",
        "gdal-control": None,
    }
    # Prior final binary hash is independently recorded in the milestone report.
    assert (
        hashlib.sha256(modes["tile-ripper-gdal"].read_bytes()).hexdigest()
        == "06407f2ded4ece14b2d6b63be0bada0071bdd6db0c69a2b961448a03876715fd"
    )
    source_digest = m.engine.source_hash()
    rows = []
    pilots = []
    rng = random.Random(20261019)
    for latency, mbps in [(0, 0)] if args.local else [(0, 0), (8, 100)]:
        server = m.start(mirror, latency, mbps)
        try:
            for name in args.workloads:
                req = m.request(
                    sources if name == "many-small" else sources[:6],
                    server,
                    WINDOWS[name],
                )
                if args.local:
                    for a in req["source"]["assets"]:
                        a["locator"] = str(refs[a["id"]]["local"])

                def measure(mode, rep, workers=2):
                    dest = folder / f"{latency}-{name}-{rep}-{mode}"
                    assert m.engine.source_hash() == source_digest, (
                        "source changed during matrix"
                    )
                    digest_before = (
                        hashlib.sha256(modes[mode].read_bytes()).hexdigest()
                        if modes[mode]
                        else None
                    )
                    if modes[mode]:
                        m.CONSUMER = modes[mode]
                        row = m.measure("rust-stream", req, dest, server, refs)
                        records = [
                            json.loads(s)
                            for s in (dest / "stdout.json").read_text().splitlines()
                        ]
                        chunks = records[1:-1]
                        for a, p in zip(
                            req["source"]["assets"], records[0]["provenance"]["sources"]
                        ):
                            assert p["object_version"] == a["etag"]
                    else:
                        row = snapshot.measure(
                            "gdal-snapshot-stream", req, dest, server, refs, workers
                        )
                        records = json.loads((dest / "stdout.json").read_text())
                        chunks = records["rows"][1:-1]
                    if modes[mode]:
                        assert (
                            hashlib.sha256(modes[mode].read_bytes()).hexdigest()
                            == digest_before
                        ), "binary changed during trial"
                    row.update(
                        backend=mode,
                        workload=name,
                        rep=rep,
                        latency_ms=latency,
                        bandwidth_mbps=mbps,
                        local=args.local,
                        binary_sha256=digest_before,
                    )
                    if not args.local:
                        account = trace.analyze(row["request_records"], layouts, name)[
                            "totals"
                        ]
                        assert account["body_bytes"] == row["http"]["body_bytes"]
                        row["blocks"] = account
                    row.pop("request_records", None)
                    row.pop("stages", None)
                    return row, chunks

                trials = []
                if not args.local:
                    for n in [1, 2, 4, 8]:
                        row, _ = measure("gdal-control", f"pilot{n}", n)
                        trials.append(row)
                        pilots.append(row)
                    tuned = min(trials, key=lambda r: r["wall_seconds"])["workers"]
                else:
                    tuned = 2
                canonical = None
                for rep in range(args.reps):
                    order = list(modes)
                    rng.shuffle(order)
                    for mode in order:
                        row, chunks = measure(mode, rep, tuned)
                        # Baseline/native contract comparison includes deterministic order.
                        if mode != "gdal-control":
                            normalized = [
                                {k: v for k, v in c.items() if k not in ["node_id"]}
                                for c in chunks
                            ]
                            if canonical is None:
                                canonical = normalized
                            assert normalized == canonical, (
                                "native/baseline public chunk contract changed"
                            )
                        obs = row.get("observed") or {}
                        row["read_decode_seconds"] = obs.get(
                            "read_decode_seconds", row.get("read_worker_seconds")
                        )
                        if obs.get("source_retrieval_seconds") is not None:
                            row["decode_assembly_seconds"] = (
                                obs["read_decode_seconds"]
                                - obs["source_retrieval_seconds"]
                            )
                        rows.append(row)
                        with (folder / "runs.jsonl").open("a") as f:
                            f.write(json.dumps(row) + "\n")
                        print(
                            latency,
                            name,
                            rep,
                            mode,
                            row["passed"],
                            round(row["wall_seconds"], 3),
                            round(row["peak_rss_bytes"] / 2**20, 1),
                            flush=True,
                        )
        finally:
            server.shutdown()
            server.server_close()
    summary = []
    for latency in sorted({r["latency_ms"] for r in rows}):
        for name in args.workloads:
            for mode in modes:
                group = [
                    r
                    for r in rows
                    if (r["latency_ms"], r["workload"], r["backend"])
                    == (latency, name, mode)
                ]
                result = dict(
                    backend=mode,
                    workload=name,
                    latency_ms=latency,
                    runs=len(group),
                    failures=sum(not r["passed"] for r in group),
                )
                for key in [
                    "wall_seconds",
                    "first_chunk_seconds",
                    "prepare_seconds",
                    "peak_rss_bytes",
                    "cpu_seconds",
                    "cpu_percent",
                    "read_decode_seconds",
                    "decode_assembly_seconds",
                ]:
                    values = [r[key] for r in group if r.get(key) is not None]
                    result[key] = (
                        dict(
                            median=statistics.median(values),
                            min=min(values),
                            max=max(values),
                        )
                        if values
                        else None
                    )
                result["http"] = {
                    k: statistics.median(r["http"][k] for r in group)
                    for k in ["head", "get", "body_bytes"]
                }
                if not args.local:
                    result["blocks"] = {
                        k: statistics.median(r["blocks"].get(k, 0) for r in group)
                        for k in group[0]["blocks"]
                    }
                summary.append(result)
    report = dict(
        summary=summary,
        rows=rows,
        pilots=pilots,
        mirror_index_sha256=digest,
        source_commit="911a268",
        native_source_sha256=source_digest,
        binary_sha256={
            k: hashlib.sha256(p.read_bytes()).hexdigest() for k, p in modes.items() if p
        },
        harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        identical_native_baseline_chunks=True,
    )
    (folder / "report.json").write_text(json.dumps(report, indent=2))
    assert all(r["passed"] for r in rows), "resource failures retained"


if __name__ == "__main__":
    main()
