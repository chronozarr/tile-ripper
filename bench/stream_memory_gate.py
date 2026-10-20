"""Retain default RSS misses and test a fixed two-worker native-consumer recipe.
No scheduler changes or adaptive policy. All original matrix failures stay intact.
"""

import hashlib
import json
import os

import acquisition_engine as engine
import real_multiscene as m


def main():
    base = m.ROOT / "prototype-output/production-streaming/real"
    sources = json.loads((base / "sources.json").read_text())["sources"]
    mirror = m.Mirror(base / "mirror", sources)
    digest = mirror.verify_pages()
    folder = base / "memory-followup"
    folder.mkdir(exist_ok=False)
    refs = {
        a["id"]: {**a, "format": "cog", "local": mirror.path(f"/{i}.tif")}
        for i, a in enumerate(sources)
    }
    windows = {
        "many-small": (7300, 2100, 96, 96),
        "several-large": (6144, 1024, 2048, 2048),
        "overlap-a": (6868, 1748, 768, 768),
        "overlap-b": (7124, 2004, 768, 768),
    }
    rows = []
    try:
        for workers, latency, bandwidth in [(4, 8, 100), (2, 0, 0), (2, 8, 100)]:
            os.environ["TILE_RIPPER_WORKERS"] = str(workers)
            server = m.start(mirror, latency, bandwidth)
            try:
                for name, win in windows.items():
                    if workers == 4 and name != "overlap-a":
                        continue
                    req = m.request(
                        sources if name == "many-small" else sources[:6], server, win
                    )
                    for rep in range(3):
                        row = m.measure(
                            "rust-stream",
                            req,
                            folder / f"{workers}workers-{latency}ms-{name}-{rep}",
                            server,
                            refs,
                            workers,
                        )
                        row.update(
                            workload=name,
                            rep=rep,
                            workers=workers,
                            latency_ms=latency,
                            bandwidth_mbps=bandwidth,
                            diagnostic=workers == 4,
                        )
                        rows.append(row)
                        with (folder / "runs.jsonl").open("a") as out:
                            out.write(json.dumps(row) + "\n")
                        print(
                            workers,
                            latency,
                            name,
                            rep,
                            "passed" if row["passed"] else "RSS FAIL",
                            round(row["wall_seconds"], 3),
                            round(row["peak_rss_bytes"] / 2**20, 1),
                            flush=True,
                        )
            finally:
                server.shutdown()
                server.server_close()
    finally:
        os.environ.pop("TILE_RIPPER_WORKERS", None)
    report = dict(
        rows=rows,
        source_sha256=engine.source_hash(),
        mirror_index_sha256=digest,
        rss_limit_bytes=192 * 2**20,
        configuration="fixed TILE_RIPPER_WORKERS=2, GDAL_CACHEMAX=32, queue=0, buffer=16 MiB",
        binaries={
            str(p.relative_to(m.ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in [m.CLI, m.CONSUMER]
        },
    )
    (folder / "report.json").write_text(json.dumps(report, indent=2))
    assert all(r["passed"] for r in rows if r["workers"] == 2), (
        "two-worker memory recipe failed; retained rows"
    )


if __name__ == "__main__":
    main()
