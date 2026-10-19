"""E7b: duckdb-cog tasks, saved. Each run is a fresh process in the cog env (DuckDB 1.5.5 + cog 0.3.0).

meta   read_cog tile grid of one B04 (what does it return / cost?)
window RS_BandAsArray over the 48 frozen assets x W_A window, checked bit-exact against the GDAL oracle
zonal  RS_ZonalStats mean, ONE call per scene (best case), checked against a numpy mean from rasterio
search read_stac_search for the AOI, 2025
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import rasterio
from rasterio.windows import Window

from common import RESULTS, save, summarize
from planwin import WINDOWS

HERE = Path(__file__).resolve().parent
PY = str(HERE / "cog-env/.venv/bin/python")
FROZEN = str(RESULTS / "scenes_17SPV_2025.json")
KNOBS = {"COG_IO_THREADS": "16", "COG_FETCH_CONCURRENCY": "24"}


def run(task: str, env: dict | None = None) -> dict:
    p = subprocess.run(
        [PY, "-u", str(HERE / "w_cog.py"), "--task", task, "--frozen", FROZEN],
        env={**os.environ, **(env or {})},
        capture_output=True,
        text=True,
        timeout=900,
        cwd=HERE,
    )
    line = next(
        (ln for ln in p.stdout.splitlines() if ln.startswith("RESULT_JSON ")), None
    )
    if line is None:
        return {"task": task, "failed": True, "stderr_tail": p.stderr[-300:]}
    return json.loads(line[12:])


def main() -> None:
    oracle = json.loads((RESULTS / "oracle_W_A_chip400.json").read_text())
    scenes = json.loads(Path(FROZEN).read_text())["scenes"]
    w = WINDOWS["W_A_chip400"]
    ref = {}
    for s in scenes:  # independent GDAL/numpy reference for the zonal mean (untimed)
        with rasterio.open("/vsicurl/" + s["red"]) as src:
            ref[s["id"]] = float(
                src.read(1, window=Window(w["c0"], w["r0"], w["w"], w["h"])).mean()
            )
    out: dict = {"duckdb": "1.5.5", "cog": "0.3.0", "runs": {}}
    plan = [
        ("meta", None, "default", 3),
        ("window", None, "default", 3),
        ("window", KNOBS, "knobs", 1),
        ("zonal", None, "default", 3),
        ("zonal", KNOBS, "knobs", 1),
        ("search", None, "default", 3),
    ]
    for task, env, label, n in plan:
        rs = []
        for _ in range(n):
            r = run(task, env)
            if r.get("failed"):
                print(task, label, "FAILED", r["stderr_tail"])
                rs.append(r)
                continue
            if task == "window":
                h = r.pop("hashes")
                r["bit_exact"] = (
                    f"{sum(1 for k, v in h.items() if oracle.get(k) == v)}/{len(oracle)}"
                )
            if task == "zonal":
                z = r.pop("zonal")
                r["max_abs_err_vs_numpy"] = max(abs(z[k][0] - ref[k]) for k in ref)
            rs.append(r)
            req = r["requests"]
            print(
                f"{task:7s} {label:8s} t_work {r['t_work_s']:6.2f}s  req {req['requests']:3d} (HEAD {req['head']}) {req['bytes'] / 1e6:7.2f} MB  "
                + (
                    f"bit_exact {r.get('bit_exact')}"
                    if task == "window"
                    else f"max err {r.get('max_abs_err_vs_numpy')}"
                    if task == "zonal"
                    else f"rows {r.get('rows')} items {r.get('items')}"
                    if task == "search"
                    else f"tile rows {r.get('tile_rows_all_levels')}"
                ),
                flush=True,
            )
            time.sleep(1.5)
        out["runs"][f"{task}/{label}"] = {
            "runs": rs,
            "t_work_s": summarize([r["t_work_s"] for r in rs if not r.get("failed")]),
        }
    save("e7_cog_tasks", out)


if __name__ == "__main__":
    sys.exit(main())
