"""E5: a DuckDB-SQL acquisition planner over the layout index. Pure SQL, no network.

  window (native pixel grid) -> tile indices -> (offset, length) from TileOffsets/TileByteCounts
  -> per-object coalescing with a gap threshold -> request manifest (one row per ranged GET)

Measures planning time and manifest quality (requests, wire bytes, payload bytes, over-read) for two
windows, sweeps the coalescing gap, and scales the layout table up synthetically to see how planning time grows.
"""

from __future__ import annotations

import statistics
import time

import duckdb

from common import RESULTS, save

LAYOUT = RESULTS / "layout_17SPV_2025.parquet"
from planwin import WINDOWS, bounds  # noqa: E402  (single source for window definitions)


PLAN_SQL = """
WITH a AS (
  SELECT l.scene_id, l.band, l.href, l.etag, l.file_size, l.level, l.width, l.height, l.tile_w, l.tile_h, l.offsets, l.counts,
         l.tiepoint[4] AS x0, l.tiepoint[5] AS y0, l.pixel_scale[1] AS sx, l.pixel_scale[2] AS sy,
         p.aoi, p.xmin, p.ymin, p.xmax, p.ymax
  FROM layout l, aoi p
  WHERE l.level = 0 AND l.band IN (SELECT unnest($bands))
), w AS (
  SELECT *,
    greatest(0, floor((xmin - x0) / sx))::INT AS c0, least(width,  ceil((xmax - x0) / sx))::INT AS c1,
    greatest(0, floor((y0 - ymax) / sy))::INT AS r0, least(height, ceil((y0 - ymin) / sy))::INT AS r1,
    (width + tile_w - 1) // tile_w AS tiles_across
  FROM a
), t AS (
  SELECT w.*, tx, ty, ty * tiles_across + tx AS idx
  FROM w,
       LATERAL (SELECT unnest(range(c0 // tile_w, (c1 - 1) // tile_w + 1)) AS tx),
       LATERAL (SELECT unnest(range(r0 // tile_h, (r1 - 1) // tile_h + 1)) AS ty)
), r AS (
  SELECT aoi, scene_id, band, href, etag, file_size, tx, ty,
         offsets[idx + 1]::BIGINT AS off, counts[idx + 1]::BIGINT AS len
  FROM t
), g AS (
  SELECT *, off - lag(off + len) OVER (PARTITION BY aoi, href ORDER BY off) AS gap FROM r
), m AS (
  SELECT *, sum(CASE WHEN gap IS NULL OR gap > $gap THEN 1 ELSE 0 END) OVER (PARTITION BY aoi, href ORDER BY off) AS grp FROM g
)
SELECT aoi, scene_id, band, href, etag, file_size, grp,
       min(off) AS range_start, max(off + len) - 1 AS range_end,
       count(*)::INT AS n_tiles, sum(len)::BIGINT AS payload_bytes,
       list(struct_pack(tx := tx, ty := ty, off := off, len := len) ORDER BY off) AS tiles
FROM m GROUP BY aoi, scene_id, band, href, etag, file_size, grp
"""


def new_con(layout_sql: str = f"read_parquet('{LAYOUT}')") -> duckdb.DuckDBPyConnection:
    con = duckdb.connect()
    con.execute(f"CREATE TABLE layout AS SELECT * FROM {layout_sql}")
    con.execute(
        "CREATE TABLE aoi(aoi VARCHAR, xmin DOUBLE, ymin DOUBLE, xmax DOUBLE, ymax DOUBLE)"
    )
    for name, win in WINDOWS.items():
        con.execute("INSERT INTO aoi VALUES (?, ?, ?, ?, ?)", [name, *bounds(win)])
    return con


def plan(con, bands: list[str], gap: int, only: str | None = None):
    sql = PLAN_SQL + (f" HAVING aoi = '{only}'" if only else "")
    return con.execute(sql, {"bands": bands, "gap": gap}).to_arrow_table()


def stats(tbl) -> dict:
    n_req = tbl.num_rows
    wire = sum(
        s - r + 1
        for r, s in zip(tbl["range_start"].to_pylist(), tbl["range_end"].to_pylist())
    )
    payload = sum(tbl["payload_bytes"].to_pylist())
    return {
        "requests": n_req,
        "wire_bytes": wire,
        "payload_bytes": payload,
        "tiles": sum(tbl["n_tiles"].to_pylist()),
        "gap_overhead_bytes": wire - payload,
    }


def main() -> None:
    con = new_con()
    out: dict = {
        "windows": WINDOWS,
        "layout_rows": con.execute("SELECT count(*) FROM layout").fetchone()[0],
    }

    print("== manifests, gap sweep (bands B04+B08; per window; 24 scenes)")
    sweep = {}
    for gap in (0, 8, 1024, 65536, 1 << 20, 8 << 20):
        row = {}
        for name in WINDOWS:
            tbl = plan(con, ["red", "nir"], gap, name)
            row[name] = stats(tbl)
        sweep[gap] = row
        print(
            f"  gap {gap:>8}: "
            + " | ".join(
                f"{n}: {v['requests']:4d} req, wire {v['wire_bytes'] / 1e6:7.2f} MB, payload {v['payload_bytes'] / 1e6:7.2f} MB, tiles {v['tiles']}"
                for n, v in row.items()
            )
        )
    out["gap_sweep"] = sweep

    print(
        "== SCL (20 m, 512 tiles) in the same SQL, window scaled automatically by pixel_scale"
    )
    scl = {n: stats(plan(con, ["scl"], 8, n)) for n in WINDOWS}
    out["scl_gap8"] = scl
    print(
        "  ",
        {n: (v["requests"], round(v["wire_bytes"] / 1e6, 2)) for n, v in scl.items()},
    )

    print("== planning time (24 scenes x 2 bands x 2 windows), warm DuckDB, 30 runs")
    times = []
    for _ in range(30):
        t = time.perf_counter()
        plan(con, ["red", "nir"], 8)
        times.append(time.perf_counter() - t)
    out["plan_ms_72assets"] = {
        "median": statistics.median(times) * 1e3,
        "min": min(times) * 1e3,
        "max": max(times) * 1e3,
    }
    print(f"   median {out['plan_ms_72assets']['median']:.1f} ms")

    print(
        "== scale-up: replicate the real layout rows with synthetic hrefs (x100, x1000)"
    )
    scale = {}
    for k in (100, 1000):
        con2 = duckdb.connect()
        con2.execute(f"""CREATE TABLE layout AS SELECT l.* REPLACE (href || '#' || i::VARCHAR AS href, scene_id || '#' || i::VARCHAR AS scene_id)
                         FROM read_parquet('{LAYOUT}') l, range({k}) t(i)""")
        con2.execute(
            "CREATE TABLE aoi AS SELECT * FROM "
            + "(VALUES "
            + ",".join(
                f"('{n}',{bounds(w)[0]},{bounds(w)[1]},{bounds(w)[2]},{bounds(w)[3]})"
                for n, w in WINDOWS.items()
            )
            + ") v(aoi,xmin,ymin,xmax,ymax)"
        )
        t = time.perf_counter()
        tbl = con2.execute(
            PLAN_SQL, {"bands": ["red", "nir"], "gap": 8}
        ).to_arrow_table()
        dt = time.perf_counter() - t
        assets = con2.execute(
            "SELECT count(DISTINCT href) FROM layout WHERE level=0 AND band IN ('red','nir')"
        ).fetchone()[0]
        scale[k] = {"assets": assets, "manifest_rows": tbl.num_rows, "plan_s": dt}
        print(
            f"   x{k}: {assets} assets x 2 windows -> {tbl.num_rows} manifest rows in {dt * 1e3:.0f} ms"
        )
    out["scale"] = scale

    # keep the gap-8 manifest of both windows for the pixel experiments
    for name in WINDOWS:
        tbl = plan(con, ["red", "nir"], 8, name)
        import pyarrow.parquet as pq

        pq.write_table(
            tbl, RESULTS / f"manifest_{name}_gap8.parquet", compression="zstd"
        )
    save("e5_planner", out)


if __name__ == "__main__":
    main()
