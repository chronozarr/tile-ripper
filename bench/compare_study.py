"""Offline parity with the original SQL manifests. Does not rewrite study outputs."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import pyarrow.parquet as pq

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
STUDY = REPO / "study/experiments/results"
sys.path.insert(0, str(REPO / "study/experiments"))
from planwin import WINDOWS, bounds  # noqa: E402
from e5_planner import new_con, plan as sql_plan  # noqa: E402


def main():
    layouts = [
        r
        for r in pq.read_table(STUDY / "layout_17SPV_2025.parquet").to_pylist()
        if r["level"] == 0 and r["band"] in ("red", "nir")
    ]
    cases = []
    expected = {}
    con = new_con()
    sql_seconds = 0
    for workload in WINDOWS:
        saved = pq.read_table(STUDY / f"manifest_{workload}_gap8.parquet").to_pylist()
        start = time.perf_counter()
        manifest = sql_plan(con, ["red", "nir"], 8, workload).to_pylist()
        sql_seconds += time.perf_counter() - start

        def signatures(rows):
            return sorted(
                (
                    r["href"],
                    r["range_start"],
                    r["range_end"],
                    sorted((t["tx"], t["ty"], t["off"], t["len"]) for t in r["tiles"]),
                )
                for r in rows
            )

        assert signatures(saved) == signatures(manifest), (
            "E5 SQL rerun differs from frozen manifest"
        )
        for row in manifest:
            key = f"{workload}:{row['scene_id']}:{row['band']}"
            expected.setdefault(key, []).extend(
                (t["tx"], t["ty"], t["off"], t["len"]) for t in row["tiles"]
            )
        for r in layouts:
            across = (r["width"] + r["tile_w"] - 1) // r["tile_w"]
            layout = dict(
                width=r["width"],
                height=r["height"],
                transform=[
                    r["tiepoint"][3],
                    r["pixel_scale"][0],
                    0,
                    r["tiepoint"][4],
                    0,
                    -r["pixel_scale"][1],
                ],
                crs="EPSG:32617",
                band=1,
                block=[r["tile_w"], r["tile_h"]],
                dtype="UInt16",
                nodata=0,
                scale=None,
                offset=None,
                tiles=[
                    dict(x=i % across, y=i // across, offset=off, bytes=n)
                    for i, (off, n) in enumerate(zip(r["offsets"], r["counts"]))
                ],
            )
            cases.append(
                dict(
                    id=f"{workload}:{r['scene_id']}:{r['band']}",
                    layout=layout,
                    spatial=dict(
                        kind="bounds", crs="EPSG:32617", bbox=bounds(WINDOWS[workload])
                    ),
                )
            )
    with tempfile.TemporaryDirectory() as d:
        path = Path(d) / "cases.json"
        path.write_text(json.dumps(cases))
        result = json.loads(
            subprocess.check_output(
                [str(REPO / "target/release/examples/layout_oracle"), str(path)],
                text=True,
            )
        )
    for row in result["rows"]:
        actual = sorted((t["x"], t["y"], t["offset"], t["bytes"]) for t in row["tiles"])
        assert actual == sorted(expected[row["id"]]), row["id"]
        w = WINDOWS[row["id"].split(":")[0]]
        assert row["window"] == dict(x=w["c0"], y=w["r0"], width=w["w"], height=w["h"])
    out = dict(
        cases=len(cases),
        sql_rerun_seconds=sql_seconds,
        all_tile_offsets_counts_and_windows_equal=True,
        planning_seconds=result["planning_seconds"],
        scope="offline comparison against saved E5 SQL manifests; no pixel execution or remote metadata validation",
    )
    (REPO / "docs/study-parity.json").write_text(json.dumps(out, indent=2) + "\n")
    print(json.dumps(out))


if __name__ == "__main__":
    main()
