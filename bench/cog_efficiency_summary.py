"""Compact exact source-I/O and retained resource accounting for matched matrices."""

import argparse
import hashlib
import json
from pathlib import Path

import real_multiscene as m
from cog_io_trace import analyze, tiff_layout


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--runs", type=Path, nargs="+", required=True)
    ap.add_argument("--output", type=Path, required=True)
    args = ap.parse_args()
    args.runs = [path.resolve() for path in args.runs]
    sources = json.loads((m.WORK / "sources.json").read_text())["sources"]
    by_id = {a["id"]: a for a in sources}
    mirror = m.Mirror(m.WORK / "mirror", sources)
    layouts = {
        f"/{i}.tif": tiff_layout(mirror.path(f"/{i}.tif")) for i in range(len(sources))
    }
    rows = []
    matrices = []
    for path in args.runs:
        matrix = json.loads((path.parent / "report.json").read_text())
        matrices.append(
            dict(
                path=str(path.relative_to(m.ROOT)),
                sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                link=matrix["link"],
                summary=matrix["summary"],
                pilots=[
                    {
                        k: v
                        for k, v in row.items()
                        if k not in ("request_records", "stages", "observed")
                    }
                    for row in matrix["pilots"]
                ],
                source_sha256=matrix["source_sha256"],
                artifact_sha256=matrix["artifact_sha256"],
                binaries=matrix["binaries"],
                environment=matrix["environment"],
                mirror_index_sha256=matrix["mirror_index_sha256"],
            )
        )
        for row in map(json.loads, path.read_text().splitlines()):
            io = analyze(row["request_records"], layouts, row["workload"])["totals"]
            assert io["body_bytes"] == row["http"]["body_bytes"]
            if row["mode"] == "rust-stream":
                output = [
                    json.loads(line)
                    for line in (
                        path.parent
                        / f"{row['workload']}-{row['rep']}-{row['mode']}"
                        / "stdout.json"
                    )
                    .read_text()
                    .splitlines()
                ]
                assert output[-1]["type"] == "complete"
                node_ids = [chunk["node_id"] for chunk in output[1:-1]]
                assert all(a < b for a, b in zip(node_ids, node_ids[1:])), (
                    "native chunk order changed"
                )
                expected_ids = {
                    a["id"]
                    for a in (
                        sources if row["workload"] == "many-small" else sources[:6]
                    )
                }
                assert {
                    s["asset_id"] for s in output[0]["provenance"]["sources"]
                } == expected_ids
                for source in output[0]["provenance"]["sources"]:
                    assert source["object_version"] == by_id[source["asset_id"]]["etag"]
                assert row["observed"]["scheduler"]["workers"] == 2, (
                    "default was overridden"
                )
                row["source_versions_and_completion_verified"] = True
            elif row["mode"] == "rust-file":
                output = json.loads(
                    (
                        path.parent
                        / f"{row['workload']}-{row['rep']}-{row['mode']}"
                        / "stdout.json"
                    ).read_text()
                )
                assert output["status"] == "complete"
                expected_ids = {
                    a["id"]
                    for a in (
                        sources if row["workload"] == "many-small" else sources[:6]
                    )
                }
                assert {
                    source["asset_id"] for source in output["sources"]
                } == expected_ids
                for source in output["sources"]:
                    assert source["object_version"] == by_id[source["asset_id"]]["etag"]
                assert row["observed"]["scheduler"]["workers"] == 2
                row["source_versions_and_completion_verified"] = True
            rows.append(
                {
                    **{
                        k: v
                        for k, v in row.items()
                        if k not in ("request_records", "stages")
                    },
                    "link": matrix["link"],
                    "source_block_io": io,
                }
            )
    report = dict(
        scope="3 fresh-process repetitions, both links, matched pinned-source GDAL native and TIFF controls; all failures retained",
        runs=len(rows),
        failures=sum(not r["passed"] for r in rows),
        native_rss_max_bytes=max(
            r["peak_rss_bytes"] for r in rows if r["mode"] == "rust-stream"
        ),
        mirror_index_sha256=mirror.verify_pages(),
        matrices=matrices,
        rows=rows,
        inherited_default_gate=dict(
            path="docs/production-streaming/summary.json",
            rss_failures=3,
            missing_exact_original_failure_rss=True,
            retained=True,
        ),
        accounting_harness_sha256={
            str(p.relative_to(m.ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in [Path(__file__).resolve(), m.ROOT / "bench/cog_io_trace.py"]
        },
        duplicate_definition="repeated compressed block byte coverage; split nonoverlapping requests touching one block are not duplicate reads",
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2))
    print(
        json.dumps({k: report[k] for k in ("runs", "failures", "native_rss_max_bytes")})
    )


if __name__ == "__main__":
    main()
