"""E2: partitioning and row-group pruning of the TGE stac-geoparquet files.

Part A  structure from parquet_metadata (footer only): row groups, sort order, bytes by column, bloom filters.
Part B  pruning matrix: for each predicate, which (row group, column) chunks DuckDB actually fetched
        (HTTP log byte ranges mapped onto column-chunk intervals) vs which row groups the footer
        statistics could have admitted.
"""

from __future__ import annotations

import json
import sys
import threading
import time
from collections import defaultdict
from pathlib import Path


from common import RESULTS, TGE_BASE, save
from ddbprobe import connect, fetch_requests, summarize_requests

C1 = f"{TGE_BASE}/sentinel-2-c1-l2a"
# Chapel Hill area AOI, wholly inside MGRS tile 17SPV (lon/lat)
AOI = (-79.10, 35.88, -79.02, 35.95)
TILE = "17SPV"


def load_meta(url: str, tag: str, setup: list[str]) -> tuple[Path, int, int]:
    """Read the footer once (remote), keep a local copy of parquet_metadata for offline analysis."""
    path = RESULTS / f"meta_{tag}.parquet"
    side = RESULTS / f"meta_{tag}.json"
    if path.exists() and side.exists():
        c = json.loads(side.read_text())
        return path, c["file_size"], c["footer"]
    con = connect(setup)
    con.execute(
        f"COPY (SELECT * FROM parquet_metadata('{url}')) TO '{path}' (FORMAT parquet)"
    )
    reqs = fetch_requests(con)
    file_size = max(r.rng[1] for r in reqs if r.rng) + 1
    footer = max((r.nbytes for r in reqs if r.type == "GET"), default=0)
    side.write_text(json.dumps({"file_size": file_size, "footer": footer}))
    return path, file_size, footer


def structure(meta: Path, file_size: int, footer: int) -> dict:
    con = connect(log=False)
    m = f"read_parquet('{meta}')"
    rg = con.execute(
        f"""SELECT count(DISTINCT row_group_id), sum(DISTINCT row_group_num_rows) FROM {m}"""
    ).fetchone()
    nrg, _ = rg
    rows = con.execute(
        f"SELECT sum(rows) FROM (SELECT DISTINCT row_group_id, row_group_num_rows AS rows FROM {m})"
    ).fetchone()[0]
    per_rg = con.execute(
        f"SELECT min(r), median(r), max(r) FROM (SELECT DISTINCT row_group_id, row_group_num_rows r FROM {m})"
    ).fetchone()
    cols = con.execute(f"""SELECT path_in_schema, sum(total_compressed_size)/1e6 AS mb, sum(CASE WHEN bloom_filter_length>0 THEN 1 ELSE 0 END) AS bloom_rgs
                           FROM {m} GROUP BY 1 ORDER BY 2 DESC""").fetchall()
    # tile statistics: how many row groups per tile, is the file sorted by _tile?
    tile = con.execute(f"""SELECT row_group_id, stats_min_value AS mn, stats_max_value AS mx FROM {m}
                           WHERE path_in_schema='_tile' ORDER BY row_group_id""").fetchall()
    sorted_ok = all(
        tile[i][2] <= tile[i + 1][1] for i in range(len(tile) - 1)
    )  # max(rg i) <= min(rg i+1)
    single_tile_rgs = sum(1 for _, a, b in tile if a == b)
    span: dict[str, int] = defaultdict(
        int
    )  # tile -> row groups whose [min,max] cover it (using min/max endpoints only)
    for _, a, b in tile:
        span[a] += 1
        if b != a:
            span[b] += 1
    hist = defaultdict(int)
    for v in span.values():
        hist[v] += 1
    return {
        "file_size_GB": file_size / 1e9,
        "footer_MiB_read": footer / 2**20,
        "row_groups": nrg,
        "rows": rows,
        "rows_per_rg_min_med_max": per_rg,
        "bytes_by_column_MB_top12": [(c, round(mb, 1), b) for c, mb, b in cols[:12]],
        "bloom_filter_columns": [c for c, _, b in cols if b],
        "sorted_by_tile": sorted_ok,
        "single_tile_row_groups": single_tile_rgs,
        "tile_endpoints_row_group_count_hist": dict(sorted(hist.items())),
        "row_groups_with_lon_extent_gt_180deg": con.execute(
            f"SELECT count(*) FROM {m} WHERE path_in_schema='geometry' AND geo_bbox.xmax-geo_bbox.xmin > 180"
        ).fetchone()[0],
        "geo_stats_present": bool(
            con.execute(
                f"SELECT count(*) FROM {m} WHERE path_in_schema='geometry' AND geo_bbox IS NOT NULL"
            ).fetchone()[0]
        ),
    }


def chunk_table(meta: Path) -> list[tuple[int, str, int, int, str]]:
    """(row_group, column, start, end, kind) for every column chunk and bloom filter."""
    con = connect(log=False)
    rows = con.execute(f"""SELECT row_group_id, path_in_schema,
            CASE WHEN dictionary_page_offset IS NOT NULL AND dictionary_page_offset > 0 THEN dictionary_page_offset ELSE data_page_offset END AS s,
            total_compressed_size, bloom_filter_offset, bloom_filter_length FROM read_parquet('{meta}')""").fetchall()
    out = []
    for rg, col, s, sz, bo, bl in rows:
        out.append((rg, col, s, s + sz, "chunk"))
        if bl and bl > 0:
            out.append((rg, col, bo, bo + bl, "bloom"))
    return out


def attribute(reqs, chunks, file_size: int, footer_start: int) -> dict:
    """Map each GET to the row groups / columns / bloom filters it overlaps."""
    touched: dict[int, set[str]] = defaultdict(set)
    bloom_rgs: set[int] = set()
    data_bytes = footer_bytes = 0
    for r in reqs:
        if r.type != "GET" or not r.rng:
            continue
        a, b = r.rng
        if b + 1 > footer_start:  # footer-region read
            footer_bytes += r.nbytes
            continue
        data_bytes += r.nbytes
        for rg, col, s, e, kind in chunks:
            if a < e and b + 1 > s:
                if kind == "bloom":
                    bloom_rgs.add(rg)
                else:
                    touched[rg].add(col)
    return {
        "row_groups_with_chunks_read": len(touched),
        "row_groups_bloom_probed": len(bloom_rgs),
        "columns_read": sorted({c for cs in touched.values() for c in cs}),
        "data_bytes": data_bytes,
        "footer_bytes": footer_bytes,
    }


def admitted(meta: Path, where: str) -> list[int]:
    """Row groups whose statistics cannot exclude the predicate. `where` is SQL over (tmin,tmax,...) pivoted stats."""
    con = connect(log=False)
    return [
        r[0]
        for r in con.execute(f"""
        WITH s AS (
          SELECT row_group_id,
                 max(CASE WHEN path_in_schema='_tile' THEN stats_min_value END) AS tmin,
                 max(CASE WHEN path_in_schema='_tile' THEN stats_max_value END) AS tmax,
                 max(CASE WHEN path_in_schema='datetime' THEN stats_min_value END) AS dmin,
                 max(CASE WHEN path_in_schema='datetime' THEN stats_max_value END) AS dmax,
                 max(CASE WHEN path_in_schema='geometry' THEN geo_bbox.xmin END) AS gxmin,
                 max(CASE WHEN path_in_schema='geometry' THEN geo_bbox.xmax END) AS gxmax,
                 max(CASE WHEN path_in_schema='geometry' THEN geo_bbox.ymin END) AS gymin,
                 max(CASE WHEN path_in_schema='geometry' THEN geo_bbox.ymax END) AS gymax,
                 max(CASE WHEN path_in_schema='proj:centroid, lat' THEN TRY_CAST(stats_min_value AS DOUBLE) END) AS latmin,
                 max(CASE WHEN path_in_schema='proj:centroid, lat' THEN TRY_CAST(stats_max_value AS DOUBLE) END) AS latmax,
                 max(CASE WHEN path_in_schema='proj:centroid, lon' THEN TRY_CAST(stats_min_value AS DOUBLE) END) AS lonmin,
                 max(CASE WHEN path_in_schema='proj:centroid, lon' THEN TRY_CAST(stats_max_value AS DOUBLE) END) AS lonmax
          FROM read_parquet('{meta}') GROUP BY 1)
        SELECT row_group_id FROM s WHERE {where} ORDER BY 1""").fetchall()
    ]


def attribute_groups(reqs, chunks, footer_start: int) -> dict[int, set[str]]:
    touched: dict[int, set[str]] = defaultdict(set)
    for r in reqs:
        if r.type == "GET" and r.rng and r.rng[1] + 1 <= footer_start:
            a, b = r.rng
            for rg, col, s, e, kind in chunks:
                if kind == "chunk" and a < e and b + 1 > s:
                    touched[rg].add(col)
    return touched


def timed_query(con, sql: str, budget_s: float) -> tuple[int | None, float]:
    """Run sql; if it exceeds budget_s, interrupt. Returns (rows or None if interrupted, wall)."""
    box: dict = {}

    def work() -> None:
        try:
            box["rows"] = con.execute(sql).to_arrow_table().num_rows
        except Exception as e:  # interrupted
            box["err"] = type(e).__name__

    t0 = time.perf_counter()
    th = threading.Thread(target=work)
    th.start()
    th.join(budget_s)
    if th.is_alive():
        con.interrupt()
        th.join(15)
    return box.get("rows"), time.perf_counter() - t0


def pruning_matrix(
    url: str,
    meta: Path,
    file_size: int,
    footer: int,
    year: int,
    setup: list[str],
    budget_s: float,
) -> list[dict]:
    chunks = chunk_table(meta)
    footer_start = file_size - footer
    env = f"ST_MakeEnvelope({AOI[0]}, {AOI[1]}, {AOI[2]}, {AOI[3]})"
    ocean = "ST_MakeEnvelope(-150.0, -20.0, -149.9, -19.9)"  # negative control: no S2 footprint intersects
    d0, d1 = f"{year}-06-01", f"{year}-08-31 23:59:59"

    def geo_adm(b: tuple[float, float, float, float]) -> str:
        """Stats predicate: the row group's geometry extent overlaps box b (xmin, ymin, xmax, ymax)."""
        return f"gxmin <= {b[2]} AND gxmax >= {b[0]} AND gymin <= {b[3]} AND gymax >= {b[1]}"

    tests = {
        "tile_eq": (f"_tile = '{TILE}'", f"tmin <= '{TILE}' AND tmax >= '{TILE}'"),
        "tile_eq_and_3mo": (
            f"_tile = '{TILE}' AND datetime BETWEEN '{d0}' AND '{d1}'",
            f"tmin <= '{TILE}' AND tmax >= '{TILE}'",
        ),
        "tile_range_between": (
            "_tile BETWEEN '17SPU' AND '17SPV'",
            "tmin <= '17SPV' AND tmax >= '17SPU'",
        ),
        "datetime_3mo_only": (
            f"datetime BETWEEN '{d0}' AND '{d1}'",
            f"dmin <= '{d1}' AND dmax >= '{d0}'",
        ),
        "st_intersects_geom": (f"ST_Intersects(geometry, {env})", geo_adm(AOI)),
        "st_intersects_extent": (
            f"ST_Intersects_Extent(geometry, {env})",
            geo_adm(AOI),
        ),
        "st_intersects_ocean_control": (
            f"ST_Intersects(geometry, {ocean})",
            geo_adm((-150.0, -20.0, -149.9, -19.9)),
        ),
        "st_intersects_extent_ocean_control": (
            f"ST_Intersects_Extent(geometry, {ocean})",
            geo_adm((-150.0, -20.0, -149.9, -19.9)),
        ),
        "extent_and_exact": (
            f"ST_Intersects_Extent(geometry, {env}) AND ST_Intersects(geometry, {env})",
            geo_adm(AOI),
        ),
        "centroid_within_1deg": (
            '"proj:centroid".lat BETWEEN 34.9 AND 36.9 AND "proj:centroid".lon BETWEEN -80.0 AND -78.0',
            "latmin <= 36.9 AND latmax >= 34.9 AND lonmin <= -78.0 AND lonmax >= -80.0",
        ),
    }
    out = []
    for name, (pred, stats_where) in tests.items():
        adm = set(admitted(meta, stats_where))
        con = connect(setup)
        rows, wall = timed_query(
            con,
            f"SELECT id, \"eo:cloud_cover\" FROM read_parquet('{url}') WHERE {pred}",
            budget_s,
        )
        reqs = fetch_requests(con)
        att = attribute(reqs, chunks, file_size, footer_start)
        touched = attribute_groups(reqs, chunks, footer_start)
        outside = sorted(
            set(touched) - adm
        )  # row groups fetched although footer statistics exclude them
        rec = {
            "predicate": name,
            "completed": rows is not None,
            "rows": rows,
            "wall_s": wall,
            "stats_admit_row_groups": len(adm),
            "fetched_outside_admit": len(outside),
            **att,
            **summarize_requests(reqs),
        }
        out.append(rec)
        verdict = (
            "PRUNED"
            if rec["completed"] and not outside
            else ("NOT PRUNED" if outside else "?")
        )
        print(
            f"  {name:36s} {verdict:10s} rows {rows if rows is not None else '-':>6}  stats-admit {len(adm):4d} rg | fetched rg {att['row_groups_with_chunks_read']:4d} "
            f"({len(outside)} outside admit) GET {rec['get']:4d} data {att['data_bytes'] / 2**20:7.2f} MiB footer {att['footer_bytes'] / 2**20:5.2f} MiB "
            f"wall {wall:5.1f}s{'' if rec['completed'] else ' (interrupted at budget)'}",
            flush=True,
        )
        con.close()
    return out


def main() -> None:
    years = [int(y) for y in sys.argv[1:]] or [2025]
    result: dict = {"aoi": AOI, "tile": TILE}
    # the s3:// door measured 1.6-1.9x faster than https for identical requests (see results/e2_doors.json)
    setup = ["SET s3_region='us-west-2'", "SET s3_url_style='path'"]
    for year in years:
        url = f"s3://us-west-2.opendata.source.coop/tge-labs/s2-stac-geoparquet/sentinel-2-c1-l2a/year={year}/items.parquet"
        print(f"== C1 {year}: footer (cached after first run)")
        meta, size, footer = load_meta(url, f"c1_{year}", setup)
        st = structure(meta, size, footer)
        print({k: v for k, v in st.items() if k != "bytes_by_column_MB_top12"})
        print("  top columns (MB, bloom rgs):", st["bytes_by_column_MB_top12"][:6])
        print("== pruning matrix")
        pm = pruning_matrix(url, meta, size, footer, year, setup, budget_s=30.0)
        result[str(year)] = {"structure": st, "pruning": pm}
    save("e2_rowgroups", result)


if __name__ == "__main__":
    main()
