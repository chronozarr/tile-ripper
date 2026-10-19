"""E1: reproduce the queries documented in taylor-geospatial/s2-stac-geoparquet (README, AGENTS.md,
docs/query-performance.md) from this machine, with the documented method:
a fresh DuckDB connection per cold run, request counts and bytes from the HTTP log.

Reproduction criterion (fixed before running): same rows as the doc describes where it states them,
and request/byte counts in the same order of magnitude with the same chain structure. Wall time is
network dependent (the doc measured from Copenhagen at 0.3-0.9 s TTFB) so it is reported, not compared.
"""

from __future__ import annotations

import random
import sys

from common import TGE_BASE, save, summarize
from ddbprobe import connect, run_cold

C1 = f"{TGE_BASE}/sentinel-2-c1-l2a"
L2A = f"{TGE_BASE}/sentinel-2-l2a"
S3_SETUP = ["SET s3_region='us-west-2'", "SET s3_url_style='path'"]

QUERIES: dict[str, dict] = {
    # --- README.md "Query it": tile + window + cloud (C1, one year file)
    "readme_q1_tile_window_cloud": {
        "sql": f"""SELECT id, datetime::VARCHAR AS datetime, "eo:cloud_cover" AS cloud, thumbnail_url
            FROM read_parquet('{C1}/year=2021/items.parquet')
            WHERE _tile = '31UFU' AND datetime BETWEEN '2021-08-01' AND '2021-10-15 23:59:59'
              AND "eo:cloud_cover" <= 10
            ORDER BY "eo:cloud_cover", id LIMIT 30""",
    },
    # --- README.md: asset hrefs for the chosen scene
    "readme_q2_assets_one_scene": {
        "sql": f"""SELECT json_extract_string(assets, '$.visual.href') AS visual,
                   json_extract_string(assets, '$.red.href') AS red,
                   json_extract_string(assets, '$.scl.href') AS scl
            FROM read_parquet('{C1}/year=2021/items.parquet')
            WHERE _tile = '31UFU' AND datetime BETWEEN '2021-10-04' AND '2021-10-04 23:59:59' LIMIT 1""",
    },
    # --- README.md: whole archive through the s3:// door with hive partition pruning
    "readme_q3_glob_year_prune": {
        "setup": S3_SETUP,
        "sql": """SELECT year, count(*) AS scenes, min(datetime)::VARCHAR AS first, max(datetime)::VARCHAR AS last
            FROM read_parquet('s3://us-west-2.opendata.source.coop/tge-labs/s2-stac-geoparquet/sentinel-2-c1-l2a/year=*/*.parquet',
                              hive_partitioning = true)
            WHERE year IN (2016, 2017) GROUP BY year ORDER BY year""",
    },
    # --- README.md: stats collection (answer 'which month is clear' without scanning items)
    "readme_q4_stats_monthly": {
        "sql": f"""SELECT year, month, scene_count, min_cloud_cover, best_item_id, best_item_date::VARCHAR
            FROM read_parquet('{TGE_BASE}/stats-c1/mgrs-monthly.parquet')
            WHERE mgrs_tile = '31UFU' AND year = 2021 ORDER BY year, month""",
    },
    "readme_q5_stats_timeline": {
        "sql": f"""SELECT year, month, tile_count, scene_count
            FROM read_parquet('{TGE_BASE}/stats-c1/timeline.parquet') ORDER BY year DESC, month DESC LIMIT 1""",
    },
}


def main() -> None:
    reps = int(sys.argv[1]) if len(sys.argv) > 1 else 5
    rng = random.Random(7)
    runs: dict[str, list[dict]] = {k: [] for k in QUERIES}
    for rep in range(reps):
        order = list(QUERIES)
        rng.shuffle(
            order
        )  # interleave so network drift is not confounded with one query
        for name in order:
            q = QUERIES[name]
            r = run_cold(q["sql"], q.get("setup"))
            r.pop(
                "requests"
            ) if rep > 0 else None  # keep the full request trace for the first rep only
            runs[name].append(r)
            print(
                f"rep{rep} {name:32s} wall {r['wall_s']:5.1f}s rows {r['rows']:3d} HEAD {r['head']} GET {r['get']:3d} "
                f"in {r['bytes_in'] / 2**20:6.2f} MiB waves {r['waves']} warm {r.get('warm_wall_s', 0):.2f}s/{r.get('warm_get')}GET"
            )
    summary = {}
    for name, rs in runs.items():
        summary[name] = {
            "wall_s": summarize([r["wall_s"] for r in rs]),
            "get": sorted({r["get"] for r in rs}),
            "bytes_in_MiB": sorted({round(r["bytes_in"] / 2**20, 2) for r in rs}),
            "waves": sorted({r["waves"] for r in rs}),
            "rows": sorted({r["rows"] for r in rs}),
            "warm_wall_s": summarize([r["warm_wall_s"] for r in rs]),
        }
    save(
        "e1_documented_queries",
        {
            "reps": reps,
            "duckdb": connect(log=False).execute("select version()").fetchone()[0],
            "summary": summary,
            "runs": runs,
        },
    )
    print("\nSUMMARY (cold, fresh connection each)")
    for n, s in summary.items():
        print(
            f"{n:32s} wall median {s['wall_s']['median']:.1f}s (min {s['wall_s']['min']:.1f} max {s['wall_s']['max']:.1f}) "
            f"GET {s['get']} MiB {s['bytes_in_MiB']} waves {s['waves']} rows {s['rows']} warm {s['warm_wall_s']['median']:.2f}s"
        )


if __name__ == "__main__":
    main()
