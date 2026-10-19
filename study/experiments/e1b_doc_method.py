"""E1b: the exact method of docs/query-performance.md (sentinel-2-l2a, year=2026/z21-31.parquet, tile 31UFU)
on the documented https door, plus ST_Intersects_Extent variants of the bbox query.

The doc measured this file on 2026-09-19 (V3-2026: 109 row groups, tile3mo 40 GET / 1.4 MiB, assets 37 / 1.9,
bbox 39 / 3.8, year 46 / 2.3, bbox-no-month 111 GET / 20.4 MiB). The part has since been consolidated
(live tail folded on the 3rd), so absolute counts may drift; the mechanism is what is reproduced.
"""

from __future__ import annotations

import random
import sys

from common import TGE_BASE, save, summarize
from ddbprobe import connect, run_cold

F = f"{TGE_BASE}/sentinel-2-l2a/year=2026/z21-31.parquet"
T = "31UFU"
APP = 'id, datetime, "eo:cloud_cover", thumbnail_url, "s2:mgrs_tile", "s2:nodata_pixel_percentage"'
ENV = "ST_MakeEnvelope(5.226, 52.476, 5.726, 52.976)"


def main() -> None:
    reps = int(sys.argv[1]) if len(sys.argv) > 1 else 3
    con = connect(log=False)
    item = con.execute(
        f"SELECT id FROM read_parquet('{F}') WHERE _month = 5 AND \"s2:mgrs_tile\" = '{T}' ORDER BY datetime LIMIT 1"
    ).fetchone()[0]
    rgs = con.execute(
        f"SELECT count(DISTINCT row_group_id) FROM parquet_metadata('{F}')"
    ).fetchone()[0]
    print(f"part has {rgs} row groups; first May scene of {T}: {item}")
    queries = {
        "tile3mo": f"""SELECT {APP} FROM read_parquet('{F}') WHERE "s2:mgrs_tile"='{T}' AND _month BETWEEN 4 AND 6
                       AND "eo:cloud_cover"<=20 ORDER BY "eo:cloud_cover", id LIMIT 30""",
        "assets_id": f"SELECT assets FROM read_parquet('{F}') WHERE _month=5 AND id='{item}' LIMIT 1",
        "bbox_month5": f"SELECT {APP} FROM read_parquet('{F}') WHERE _month=5 AND ST_Intersects(geometry, {ENV})",
        "bbox_month5_extent": f"""SELECT {APP} FROM read_parquet('{F}') WHERE _month=5 AND ST_Intersects_Extent(geometry, {ENV})
                                  AND ST_Intersects(geometry, {ENV})""",
        "bbox_nomonth": f"SELECT {APP} FROM read_parquet('{F}') WHERE ST_Intersects(geometry, {ENV})",
        "bbox_nomonth_extent": f"""SELECT {APP} FROM read_parquet('{F}') WHERE ST_Intersects_Extent(geometry, {ENV})
                                   AND ST_Intersects(geometry, {ENV})""",
        "year_tile": f"SELECT {APP} FROM read_parquet('{F}') WHERE \"s2:mgrs_tile\"='{T}' ORDER BY datetime",
    }
    rng = random.Random(11)
    runs: dict[str, list[dict]] = {k: [] for k in queries}
    for rep in range(reps):
        order = list(queries)
        rng.shuffle(order)
        for name in order:
            r = run_cold(queries[name], warm_repeat=True)
            r.pop("requests")
            runs[name].append(r)
            print(
                f"rep{rep} {name:22s} wall {r['wall_s']:5.1f}s rows {r['rows']:5d} GET {r['get']:4d} in {r['bytes_in'] / 2**20:6.2f} MiB waves {r['waves']} warm {r['warm_wall_s']:.2f}s",
                flush=True,
            )
    summary = {
        k: {
            "wall_s": summarize([r["wall_s"] for r in v]),
            "get": sorted({r["get"] for r in v}),
            "MiB": sorted({round(r["bytes_in"] / 2**20, 2) for r in v}),
            "rows": sorted({r["rows"] for r in v}),
            "waves": sorted({r["waves"] for r in v}),
        }
        for k, v in runs.items()
    }
    save(
        "e1b_doc_method",
        {
            "part": F,
            "row_groups_now": rgs,
            "item": item,
            "reps": reps,
            "summary": summary,
            "runs": runs,
        },
    )
    print("\nSUMMARY")
    for k, s in summary.items():
        print(
            f"{k:22s} wall median {s['wall_s']['median']:5.1f}s  GET {s['get']}  MiB {s['MiB']}  rows {s['rows']}  waves {s['waves']}"
        )


if __name__ == "__main__":
    main()
