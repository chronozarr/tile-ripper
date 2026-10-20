"""Isolate bounded layout inspection using preserved binaries and identical work.
Run after baseline/layout-only binaries are saved; never captures public imagery.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics

import real_multiscene as m


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--label", required=True)
    ap.add_argument("--reps", type=int, default=3)
    args = ap.parse_args()
    folder = m.ROOT / "prototype-output/cog-efficiency" / args.label
    folder.mkdir(parents=True, exist_ok=False)
    sources = json.loads((m.WORK / "sources.json").read_text())["sources"]
    mirror = m.Mirror(m.WORK / "mirror", sources)
    mirror_hash = mirror.verify_pages()
    refs = {
        a["id"]: {**a, "format": "cog", "local": mirror.path(f"/{i}.tif")}
        for i, a in enumerate(sources)
    }
    server = m.start(mirror, 8, 100)
    previous = os.environ.get("TILE_RIPPER_WORKERS")
    rows = []
    canonical = None
    binaries = {}
    try:
        req = m.request(sources, server, (7300, 2100, 96, 96))
        # Equal identity/data worker widths. Only the layout-only binary has
        # parallel inspection; it retains the baseline data scheduler/cache clear.
        for workers in [2, 4]:
            os.environ["TILE_RIPPER_WORKERS"] = str(workers)
            for rep in range(args.reps):
                modes = ["baseline-binaries", "layout-binaries"]
                if rep % 2:
                    modes.reverse()
                for mode in modes:
                    binary = (
                        m.ROOT
                        / "prototype-output/cog-efficiency"
                        / mode
                        / "tile-ripper-native-stats"
                    )
                    binaries[mode] = hashlib.sha256(binary.read_bytes()).hexdigest()
                    m.CONSUMER = binary
                    dest = folder / f"{workers}-{rep}-{mode}"
                    row = m.measure("rust-stream", req, dest, server, refs)
                    output = [
                        json.loads(line)
                        for line in (dest / "stdout.json").read_text().splitlines()
                    ]
                    science = (output[0]["provenance"]["plan"], output[1:-1])
                    if canonical is None:
                        canonical = science
                    assert science == canonical, (
                        "plan or provisional science/order changed"
                    )
                    row.update(variant=mode, configured_workers=workers, rep=rep)
                    rows.append(row)
                    with (folder / "runs.jsonl").open("a") as out:
                        out.write(json.dumps(row) + "\n")
                    print(
                        workers,
                        rep,
                        mode,
                        row["passed"],
                        round(row["wall_seconds"], 3),
                        round(row["layout_inspection_seconds"], 3),
                        flush=True,
                    )
    finally:
        if previous is None:
            os.environ.pop("TILE_RIPPER_WORKERS", None)
        else:
            os.environ["TILE_RIPPER_WORKERS"] = previous
        server.shutdown()
        server.server_close()
    metrics = [
        "wall_seconds",
        "prepare_seconds",
        "layout_inspection_seconds",
        "first_chunk_seconds",
        "peak_rss_bytes",
    ]
    summary = []
    for workers in [2, 4]:
        for mode in ["baseline-binaries", "layout-binaries"]:
            group = [
                r
                for r in rows
                if r["variant"] == mode and r["configured_workers"] == workers
            ]
            summary.append(
                dict(
                    variant=mode,
                    workers=workers,
                    failures=sum(not r["passed"] for r in group),
                    **{
                        key: dict(
                            median=statistics.median(r[key] for r in group),
                            min=min(r[key] for r in group),
                            max=max(r[key] for r in group),
                        )
                        for key in metrics
                    },
                    http=[r["http"] for r in group],
                )
            )
    report = dict(
        summary=summary,
        rows=rows,
        binary_sha256=binaries,
        mirror_index_sha256=mirror_hash,
        harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        identical_plans_and_chunks=True,
        scope="layout-only ablation; data scheduling and worker widths held constant; 8ms/100Mbit/s frozen replay",
    )
    (folder / "report.json").write_text(json.dumps(report, indent=2))
    assert all(r["passed"] for r in rows), "failures retained"


if __name__ == "__main__":
    main()
