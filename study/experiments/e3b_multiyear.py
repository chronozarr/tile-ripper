"""E3b: discovery across the full record (2015-2026) for tile 17SPV + the Chapel Hill footprint.

Cold: one fresh DuckDB connection per year file, all years in parallel (s3 door). Then the same result is persisted as
a small local Parquet slice and re-queried, which is what a planner would do after the first run. The STAC API
equivalent (all years, paged) is timed for comparison.
"""

from __future__ import annotations

import concurrent.futures as cf
import statistics
import sys
import time

import httpx

from common import ES_API, RESULTS, USER_AGENT, save
from e3_discovery import AOI, CC, ENV, S3_SETUP, TILE, api_search  # noqa: F401
from ddbprobe import connect, fetch_requests, summarize_requests

YEARS = list(range(2015, 2027))
BASE = (
    "s3://us-west-2.opendata.source.coop/tge-labs/s2-stac-geoparquet/sentinel-2-c1-l2a"
)
SQL = """SELECT id, datetime::VARCHAR AS datetime, "eo:cloud_cover" AS cc, "s2:nodata_pixel_percentage" AS nodata, '{y}' AS year
         FROM read_parquet('{u}') WHERE _tile = '{t}' AND "eo:cloud_cover" <= {cc} AND ST_Intersects(geometry, {env})"""


def one_year(y: int) -> dict:
    con = connect(S3_SETUP)
    t0 = time.perf_counter()
    try:
        tbl = con.execute(
            SQL.format(y=y, u=f"{BASE}/year={y}/items.parquet", t=TILE, cc=CC, env=ENV)
        ).to_arrow_table()
    except Exception as e:  # a year file may be absent or empty
        return {"year": y, "error": str(e)[:100]}
    wall = time.perf_counter() - t0
    s = summarize_requests(fetch_requests(con))
    return {
        "year": y,
        "rows": tbl.num_rows,
        "wall_s": wall,
        **{k: s[k] for k in ("head", "get", "bytes_in", "waves")},
        "_tbl": tbl,
    }


def api_all_years() -> dict:
    items, pages, nbytes = [], 0, 0
    t0 = time.perf_counter()
    with httpx.Client(timeout=60, headers={"User-Agent": USER_AGENT}) as c:
        body: dict | None = {
            "collections": ["sentinel-2-c1-l2a"],
            "bbox": list(AOI),
            "limit": 200,
            "datetime": "2015-01-01T00:00:00Z/2026-12-31T23:59:59Z",
            "query": {"eo:cloud_cover": {"lte": CC}},
        }
        url = f"{ES_API}/search"
        while url:
            r = c.post(url, json=body)
            r.raise_for_status()
            nbytes += len(r.content)
            pages += 1
            j = r.json()
            items += j["features"]
            nxt = next(
                (link for link in j.get("links", []) if link.get("rel") == "next"), None
            )
            url, body = (nxt["href"], nxt.get("body", body)) if nxt else (None, None)
    return {
        "wall_s": time.perf_counter() - t0,
        "pages": pages,
        "bytes": nbytes,
        "items": len(items),
        "ids": {f["id"] for f in items},
    }


def main() -> None:
    reps = int(sys.argv[1]) if len(sys.argv) > 1 else 3
    out: dict = {"years": YEARS, "tile": TILE, "aoi": AOI}
    cold = []
    for rep in range(reps):
        t0 = time.perf_counter()
        with cf.ThreadPoolExecutor(len(YEARS)) as ex:
            res = list(ex.map(one_year, YEARS))
        wall = time.perf_counter() - t0
        ok = [r for r in res if "error" not in r]
        cold.append(
            {
                "wall_s": wall,
                "rows": sum(r["rows"] for r in ok),
                "get": sum(r["get"] for r in ok),
                "MiB": sum(r["bytes_in"] for r in ok) / 2**20,
                "per_year": [{k: v for k, v in r.items() if k != "_tbl"} for r in res],
            }
        )
        print(
            f"cold rep{rep}: {len(YEARS)} years in parallel: wall {wall:5.1f}s  rows {cold[-1]['rows']}  GET {cold[-1]['get']}  {cold[-1]['MiB']:.1f} MiB  (slowest year {max(r.get('wall_s', 0) for r in res):.1f}s)",
            flush=True,
        )
        last = ok
    out["cold_parallel"] = cold

    import pyarrow as pa
    import pyarrow.parquet as pq

    tbl = pa.concat_tables([r["_tbl"] for r in last])
    path = RESULTS / "slice_17SPV_allyears.parquet"
    pq.write_table(tbl, path, compression="zstd")
    local = []
    for _ in range(20):
        con = connect(log=False)
        t0 = time.perf_counter()
        con.execute(
            f"SELECT count(*) FROM read_parquet('{path}') WHERE cc <= {CC} AND nodata <= 5"
        ).fetchone()
        local.append(time.perf_counter() - t0)
    out["local_slice"] = {
        "bytes": path.stat().st_size,
        "rows": tbl.num_rows,
        "query_ms_median": statistics.median(local) * 1e3,
    }
    print(
        f"persisted slice: {tbl.num_rows} rows, {path.stat().st_size / 1024:.0f} KiB; local query median {out['local_slice']['query_ms_median']:.1f} ms"
    )

    api = [api_all_years() for _ in range(reps)]
    out["api"] = {
        "wall_s": [a["wall_s"] for a in api],
        "pages": api[0]["pages"],
        "bytes": api[0]["bytes"],
        "items": api[0]["items"],
    }
    print(
        f"STAC API all years: wall median {statistics.median(a['wall_s'] for a in api):.2f}s  pages {api[0]['pages']}  {api[0]['bytes'] / 2**20:.2f} MiB  items {api[0]['items']}"
    )
    gp_ids = set(tbl["id"].to_pylist())
    out["equality"] = {
        "gp": len(gp_ids),
        "api": len(api[0]["ids"]),
        "only_api": sorted(api[0]["ids"] - gp_ids)[:5],
        "only_gp": sorted(gp_ids - api[0]["ids"])[:5],
        "only_api_n": len(api[0]["ids"] - gp_ids),
        "only_gp_n": len(gp_ids - api[0]["ids"]),
    }
    print("set equality:", out["equality"])
    save("e3b_multiyear", out)


if __name__ == "__main__":
    main()
