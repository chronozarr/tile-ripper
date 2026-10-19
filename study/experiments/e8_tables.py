"""E8: turn the saved result JSON into the tables used in the report (results/tables.md). Offline."""

from __future__ import annotations

import json
import random
import statistics

from common import RESULTS


def J(name: str) -> dict:
    return json.loads((RESULTS / f"{name}.json").read_text())


def boot_ratio(
    a: list[float], b: list[float], n: int = 10000
) -> tuple[float, float, float]:
    """median(a)/median(b) with a percentile bootstrap 95% interval (small n: indicative only)."""
    rng = random.Random(1)
    est = statistics.median(a) / statistics.median(b)
    rs = sorted(
        statistics.median(rng.choices(a, k=len(a)))
        / statistics.median(rng.choices(b, k=len(b)))
        for _ in range(n)
    )
    return est, rs[int(0.025 * n)], rs[int(0.975 * n)]


def fmt(x: float, d: int = 2) -> str:
    return f"{x:.{d}f}"


def main() -> None:
    L: list[str] = []
    p = L.append

    # ---- D1: documented queries
    e1, e1b = J("e1_documented_queries"), J("e1b_doc_method")
    p(
        "## D1. Documented queries, reproduced (DuckDB "
        + str(e1["duckdb"])
        + ", fresh connection per cold run)\n"
    )
    p(
        "| query | cold wall s (median, min-max) | HTTP GET | MiB in | waves | rows | warm s |\n|---|---|---|---|---|---|---|"
    )
    for k, s in {**e1["summary"], **e1b["summary"]}.items():
        w = s["wall_s"]
        warm = s.get("warm_wall_s", {}).get("median")
        p(
            f"| {k} | {fmt(w['median'], 1)} ({fmt(w['min'], 1)}-{fmt(w['max'], 1)}) | {s['get']} | {s.get('bytes_in_MiB', s.get('MiB'))} | {s['waves']} | {s['rows']} | {fmt(warm) if warm else '-'} |"
        )

    # ---- D2: structure + pruning
    e2 = J("e2_rowgroups")["2025"]
    st = e2["structure"]
    p("\n## D2. C1 `year=2025/items.parquet` structure\n")
    p(
        f"- {st['file_size_GB']:.2f} GB, {st['row_groups']} row groups, {st['rows']:,} rows, rows/group min-median-max {st['rows_per_rg_min_med_max']}, footer {st['footer_MiB_read']:.2f} MiB"
    )
    p(
        f"- sorted by `_tile`: {st['sorted_by_tile']}; row groups whose geometry lon-extent exceeds 180 degrees: {st['row_groups_with_lon_extent_gt_180deg']} of {st['row_groups']}"
    )
    p(
        "- bytes by column (MB): "
        + ", ".join(f"{c} {mb}" for c, mb, _ in st["bytes_by_column_MB_top12"][:6])
    )
    p(
        "\n| predicate | outcome | stats-admitted rg | rg fetched | fetched outside admit set | GET | data MiB | footer MiB | rows |\n|---|---|---|---|---|---|---|---|---|"
    )
    for r in e2["pruning"]:
        verdict = (
            "pruned"
            if r["completed"] and r["fetched_outside_admit"] == 0
            else (
                "NOT pruned" if r["fetched_outside_admit"] else "interrupted at budget"
            )
        )
        p(
            f"| {r['predicate']} | {verdict} | {r['stats_admit_row_groups']} | {r['row_groups_with_chunks_read']} | {r['fetched_outside_admit']} | {r['get']} | {r['data_bytes'] / 2**20:.2f} | {r['footer_bytes'] / 2**20:.2f} | {r['rows'] if r['rows'] is not None else '-'} |"
        )

    # ---- D3: discovery comparison
    e3, e3b = J("e3_discovery"), J("e3b_multiyear")
    p("\n## D3. Discovery for one tile + AOI footprint, 2025, cloud<=20 (36 scenes)\n")
    p("| path | wall s median (cold) | requests | MiB | notes |\n|---|---|---|---|---|")
    for k, v in e3["api"].items():
        p(
            f"| STAC API ({k}) | {fmt(v['wall_s']['median'])} | {v['pages']} pages | {v['bytes'] / 2**20:.2f} | |"
        )
    for k, v in e3["geoparquet"].items():
        p(
            f"| GeoParquet {k} | {fmt(v['wall_s']['median'])} | {v['get'] + 1} | {v['MiB']:.2f} | warm repeat {fmt(v['warm_wall_s']['median'])} s; waves {v['waves']} |"
        )
    p(
        f"\nTile-only selection returns {e3['tile_only_rows']} rows; tile + footprint returns {e3['equality']['gp_n']} = API's {e3['equality']['api_n']} (set-equal: only_in_api={len(e3['equality']['only_in_api'])}, only_in_gp={len(e3['equality']['only_in_gp'])})."
    )
    p(
        f"Asset hrefs derived from id match the stored hrefs for {e3['href_derivation']['checked'] - e3['href_derivation']['mismatches']}/{e3['href_derivation']['checked']} rows; distinct B04 grids across scenes: {len(e3['red_grid_variants'])}."
    )
    cp = e3b["cold_parallel"]
    p(
        f"\nAll years 2015-2026, GeoParquet cold, 12 files in parallel: wall {[fmt(c['wall_s'], 1) for c in cp]} s, {cp[0]['get']} GET, {cp[0]['MiB']:.1f} MiB, {cp[0]['rows']} rows. Persisted slice: {e3b['local_slice']['bytes'] / 1024:.0f} KiB, query {fmt(e3b['local_slice']['query_ms_median'], 1)} ms. STAC API all years: {[fmt(x) for x in e3b['api']['wall_s']]} s, {e3b['api']['pages']} pages, {e3b['api']['bytes'] / 2**20:.2f} MiB, {e3b['api']['items']} items. Set difference: only_api={e3b['equality']['only_api_n']} ({e3b['equality']['only_api']}), only_gp={e3b['equality']['only_gp_n']}."
    )

    # ---- L and planner
    e4, e5 = J("e4_layout"), J("e5_planner")
    p("\n## L. Layout index (one 32 KiB range GET per COG)\n")
    p(
        f"- {e4['assets']} assets: {e4['gets']} GETs, {e4['bytes'] / 1e6:.2f} MB, wall {fmt(e4['wall_s'])} s at concurrency {e4['concurrency']}; extra fetches needed: {e4['extra_fetches_needed']}"
    )
    p(
        "- header bytes actually required (min/median/max): "
        + ", ".join(
            f"{b} {v['min']}/{int(v['median'])}/{v['max']}"
            for b, v in e4["header_bytes_needed"].items()
        )
    )
    p(
        "\n## Planner (SQL), W_A = 400x400 px in one block, W_B = 600x600 px over a 4-block corner\n"
    )
    p(
        "| gap (bytes) | W_A requests | W_A wire MB | W_B requests (24 scenes) | W_B wire MB | W_B payload MB |\n|---|---|---|---|---|---|"
    )
    for g, row in e5["gap_sweep"].items():
        a, b = row["W_A_chip400"], row["W_B_corner600"]
        p(
            f"| {g} | {a['requests']} | {a['wire_bytes'] / 1e6:.2f} | {b['requests']} | {b['wire_bytes'] / 1e6:.2f} | {b['payload_bytes'] / 1e6:.2f} |"
        )
    p(
        f"\nPlanning time, 72 assets x 2 windows: median {fmt(e5['plan_ms_72assets']['median'], 1)} ms. Scale-up: "
        + "; ".join(
            f"{v['assets']} assets -> {v['manifest_rows']} manifest rows in {v['plan_s'] * 1e3:.0f} ms"
            for v in e5["scale"].values()
        )
    )

    # ---- P
    for wl, scenes, note in (("W_A_chip400", 24, ""), ("W_B_corner600", 12, "")):
        e6 = J(f"e6_pixels_{wl}_{scenes}")
        runs = e6["runs"]
        p(
            f"\n## P. {wl}, {scenes} scenes x B04+B08 ({e6['reps']} interleaved reps, fresh process each)\n"
        )
        p(
            "| config | t_work median s [min-max] | requests | bytes requested MB | NIC min MB (all runs) | pixels vs GDAL oracle | ratio to best floor (95% boot CI) |\n|---|---|---|---|---|---|---|"
        )
        floor_name = min(
            (
                k
                for k in e6["summary"]
                if k.startswith("floor") and e6["summary"][k]["n"]
            ),
            key=lambda k: e6["summary"][k]["t_work_s"]["median"],
        )
        floor_t = [
            r["t_work_s"]
            for r in runs
            if r["config"] == floor_name and not r.get("failed")
        ]
        for name, s in e6["summary"].items():
            if s["n"] == 0:
                continue
            ts = [
                r["t_work_s"]
                for r in runs
                if r["config"] == name and not r.get("failed")
            ]
            est, lo, hi = boot_ratio(ts, floor_t)
            nic = min(
                r["nic_bytes_in"]
                for r in runs
                if r["config"] == name and not r.get("failed")
            )
            req, rb = s["requests"], s["bytes_requested"]
            if wl == "W_B_corner600" and name == "gdal-tuned-T16":
                req, rb = (
                    ["96*"],
                    [151.57e6],
                )  # see e6b: log bridge drops lines from GDAL decode threads; value from the GDAL_NUM_THREADS=1 control
            t = s["t_work_s"]
            p(
                f"| {name} | {fmt(t['median'], 1)} [{fmt(t['min'], 1)}-{fmt(t['max'], 1)}] | {req} | {[round(b / 1e6, 2) if b else b for b in rb]} | {nic / 1e6:.1f} | {s['bit_exact'] or 'n/a (separate decode check)'} | {fmt(est, 2)}x ({fmt(lo, 2)}-{fmt(hi, 2)}) |"
            )
        pairs = [
            ("gdal-tuned-T16", "floor-C16"),
            ("gdal-tuned-T32", "floor-C32"),
            ("rasteret-read", "floor-C32"),
            ("gdal-tuned-T16", "gdal-tuned-T4"),
        ]
        p(
            "\nLike-for-like (same concurrency) ratios of median t_work, bootstrap 95% interval (n is small: indicative):"
        )
        for a_, b_ in pairs:
            ta = [
                r["t_work_s"] for r in runs if r["config"] == a_ and not r.get("failed")
            ]
            tb = [
                r["t_work_s"] for r in runs if r["config"] == b_ and not r.get("failed")
            ]
            if ta and tb:
                est, lo, hi = boot_ratio(ta, tb)
                p(f"- {a_} / {b_}: {fmt(est, 2)}x ({fmt(lo, 2)}-{fmt(hi, 2)})")
        p(
            f"\nBest floor config = {floor_name}. NIC counter includes TLS/HTTP framing and other machine traffic; the minimum across runs is the cleanest bound. `*` see e6b."
        )

    # ---- duckdb-cog
    try:
        e7 = J("e7_cog_tasks")
        p("\n## duckdb-cog 0.3.0 (DuckDB 1.5.5), same assets and window\n")
        p(
            "| task | t_work s median [min-max] | requests | MB | check |\n|---|---|---|---|---|"
        )
        for k, v in e7["runs"].items():
            ok = [r for r in v["runs"] if not r.get("failed")]
            if not ok:
                p(f"| {k} | FAILED | | | |")
                continue
            t = v["t_work_s"]
            req = ok[0].get("requests_read_cog") or ok[0]["requests"]
            chk = ok[0].get("bit_exact") or (
                f"max err {ok[0]['max_abs_err_vs_numpy']}"
                if "max_abs_err_vs_numpy" in ok[0]
                else (
                    f"rows {ok[0].get('rows')} items {ok[0].get('items')}"
                    if "rows" in ok[0]
                    else f"tile rows {ok[0].get('tile_rows_all_levels')}"
                )
            )
            p(
                f"| {k} | {fmt(t['median'], 1)} [{fmt(t['min'], 1)}-{fmt(t['max'], 1)}] | {req['requests']} (HEAD {req['head']}) | {req['bytes'] / 1e6:.2f} | {chk} |"
            )
    except FileNotFoundError:
        p("\n(duckdb-cog task results not yet saved)")

    (RESULTS / "tables.md").write_text("\n".join(L) + "\n")
    print("\n".join(L))


if __name__ == "__main__":
    main()
