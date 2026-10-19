"""E6: pixel retrieval (P), identical frozen scenes and windows, every tool in a fresh process.

Tools: GDAL 3.12.2 via rasterio 1.5.2 (default and tuned env, thread sweep), Rasteret 0.4.1 (index persisted),
and the planner manifest executed by a bare range fetcher (the spec's T_io_floor path).
Runs are interleaved in a fresh random order every repetition so link drift is not confounded with a tool.
Per run: worker-reported time and request counts, NIC (en0) byte delta as an independent check, and
bit-exact comparison against the GDAL-tuned oracle hashes.
"""

from __future__ import annotations

import json
import os
import random
import subprocess
import sys
import time
from pathlib import Path

from common import RESULTS, save, summarize

HERE = Path(__file__).resolve().parent
PY = str(HERE / ".venv/bin/python")
PY_RASTERET = str(HERE / "rasteret-env/.venv/bin/python")
FROZEN = str(RESULTS / "scenes_17SPV_2025.json")
RASTERET_WS = "/private/tmp/claude-501/-Users-jakegearon-projects-tile-ripper/5763ccdd-34a5-4e6c-ac18-f1f32b3e3ecd/scratchpad/rasteret_ws"

TUNED = {
    "CPL_VSIL_CURL_ALLOWED_EXTENSIONS": ".tif",
    "GDAL_DISABLE_READDIR_ON_OPEN": "EMPTY_DIR",
    "GDAL_INGESTED_BYTES_AT_OPEN": "32768",
    "CPL_VSIL_CURL_CACHE_SIZE": "200000000",
    "GDAL_CACHEMAX": "1024",
    "GDAL_BAND_BLOCK_CACHE": "HASHSET",
    "GDAL_MAX_RAW_BLOCK_CACHE_SIZE": "268435456",
    "GDAL_NUM_THREADS": "ALL_CPUS",
    "GDAL_HTTP_MAX_RETRY": "3",
    "GDAL_HTTP_TIMEOUT": "45",
}


def nic_in_bytes() -> int:
    out = subprocess.run(["netstat", "-ib"], capture_output=True, text=True).stdout
    for line in out.splitlines():
        t = line.split()
        if t and t[0] == "en0" and t[2].startswith("<Link"):
            return int(t[6])
    raise RuntimeError("en0 counters not found")


def configs(workload: str, scenes: int) -> dict[str, dict]:
    manifest = str(RESULTS / f"manifest_{workload}_gap8.parquet")
    base = ["--workload", workload, "--scenes", str(scenes), "--frozen", FROZEN]
    cfg = {
        "gdal-default-T4": dict(
            py=PY, cmd=["w_gdal.py", *base, "--threads", "4"], env={}
        ),
        "gdal-tuned-T4": dict(
            py=PY, cmd=["w_gdal.py", *base, "--threads", "4"], env=TUNED
        ),
        "gdal-tuned-T16": dict(
            py=PY, cmd=["w_gdal.py", *base, "--threads", "16"], env=TUNED
        ),
        "gdal-tuned-T32": dict(
            py=PY, cmd=["w_gdal.py", *base, "--threads", "32"], env=TUNED
        ),
        "floor-C16": dict(
            py=PY,
            cmd=["w_floor.py", "--manifest", manifest, *base, "--conc", "16"],
            env={},
        ),
        "floor-C32": dict(
            py=PY,
            cmd=["w_floor.py", "--manifest", manifest, *base, "--conc", "32"],
            env={},
        ),
        "floor-C64": dict(
            py=PY,
            cmd=["w_floor.py", "--manifest", manifest, *base, "--conc", "64"],
            env={},
        ),
        "rasteret-read": dict(
            py=PY_RASTERET,
            cmd=[
                "w_rasteret.py",
                "--phase",
                "read",
                "--workload",
                workload,
                "--scenes",
                str(scenes),
                "--workspace",
                RASTERET_WS,
                "--frozen",
                FROZEN,
            ],
            env={},
        ),
    }
    for k in ("gdal-tuned-T4", "gdal-tuned-T16", "gdal-tuned-T32", "gdal-default-T4"):
        cfg[k]["env"] = {**cfg[k]["env"], "CPL_DEBUG": "ON"}
    return cfg


def run_one(name: str, c: dict) -> dict:
    env = {**os.environ, **c["env"]}
    n0, t0 = nic_in_bytes(), time.perf_counter()
    p = subprocess.run(
        [c["py"], "-u", str(HERE / c["cmd"][0]), *c["cmd"][1:]],
        env=env,
        capture_output=True,
        text=True,
        timeout=600,
        cwd=HERE,
    )
    t_proc, n1 = time.perf_counter() - t0, nic_in_bytes()
    line = next(
        (ln for ln in p.stdout.splitlines() if ln.startswith("RESULT_JSON ")), None
    )
    if p.returncode != 0 or line is None:
        return {
            "config": name,
            "failed": True,
            "returncode": p.returncode,
            "stderr_tail": p.stderr[-400:],
        }
    r = json.loads(line[len("RESULT_JSON ") :])
    r.update(config=name, t_process_s=t_proc, nic_bytes_in=n1 - n0)
    return r


def main() -> None:
    workload = sys.argv[1]
    scenes = int(sys.argv[2])
    reps = int(sys.argv[3])
    only = sys.argv[4].split(",") if len(sys.argv) > 4 else None
    cfg = configs(workload, scenes)
    if only:
        cfg = {k: v for k, v in cfg.items() if k in only}
    rng = random.Random(2026)
    runs: list[dict] = []
    log = RESULTS / f"e6_runs_{workload}_{scenes}.jsonl"
    log.write_text("")
    for rep in range(reps):
        order = list(cfg)
        rng.shuffle(order)
        for name in order:
            r = run_one(name, cfg[name])
            r["rep"] = rep
            runs.append(r)
            with log.open("a") as f:
                f.write(
                    json.dumps({k: v for k, v in r.items() if k != "hashes"}) + "\n"
                )
            if r.get("failed"):
                print(
                    f"rep{rep} {name:16s} FAILED rc={r['returncode']} {r['stderr_tail'][-160:]!r}",
                    flush=True,
                )
            else:
                print(
                    f"rep{rep} {name:16s} t_work {r['t_work_s']:6.2f}s  req {r['requests']:4d}  req_bytes {r.get('bytes_requested', 0) / 1e6:7.2f} MB  "
                    f"NIC {r['nic_bytes_in'] / 1e6:7.2f} MB",
                    flush=True,
                )
            time.sleep(1.5)

    # oracle = first GDAL-tuned run's hashes; every hashed run must match it
    oracle_run = next(
        (r for r in runs if r["config"].startswith("gdal-tuned") and "hashes" in r),
        None,
    )
    oracle = oracle_run["hashes"] if oracle_run else {}
    if oracle:
        (RESULTS / f"oracle_{workload}_{scenes}.json").write_text(json.dumps(oracle))
    for r in runs:
        if "hashes" in r and oracle:
            r["bit_exact"] = sum(
                1 for k, v in r["hashes"].items() if oracle.get(k) == v
            )
            r["bit_total"] = len(oracle)
    summary: dict = {}
    for name in cfg:
        rs = [r for r in runs if r["config"] == name and not r.get("failed")]
        if not rs:
            summary[name] = {
                "n": 0,
                "failed": sum(1 for r in runs if r["config"] == name),
            }
            continue
        summary[name] = {
            "n": len(rs),
            "failed": sum(1 for r in runs if r["config"] == name and r.get("failed")),
            "t_work_s": summarize([r["t_work_s"] for r in rs]),
            "t_process_s": summarize([r["t_process_s"] for r in rs]),
            "requests": sorted({r["requests"] for r in rs}),
            "bytes_requested": sorted({r.get("bytes_requested") for r in rs}),
            "nic_bytes_in_median": sorted(r["nic_bytes_in"] for r in rs)[len(rs) // 2],
            "bit_exact": sorted(
                {f"{r['bit_exact']}/{r['bit_total']}" for r in rs if "bit_exact" in r}
            ),
            "head": sorted({r.get("head") for r in rs}),
            "listing": sorted({r.get("get_listing") for r in rs}),
        }
    save(
        f"e6_pixels_{workload}_{scenes}",
        {
            "workload": workload,
            "scenes": scenes,
            "reps": reps,
            "summary": summary,
            "runs": [{k: v for k, v in r.items() if k != "hashes"} for r in runs],
        },
    )
    print(
        "\nSUMMARY (medians over reps; t_work = in-process time, excludes interpreter+import)"
    )
    for name, s in summary.items():
        if s["n"] == 0:
            print(f"{name:16s} no successful runs ({s['failed']} failed)")
            continue
        print(
            f"{name:16s} n={s['n']} t_work {s['t_work_s']['median']:6.2f}s [{s['t_work_s']['min']:.1f}-{s['t_work_s']['max']:.1f}]  req {s['requests']}  "
            f"req_bytes {[round(b / 1e6, 2) if b else b for b in s['bytes_requested']]} MB  NIC {s['nic_bytes_in_median'] / 1e6:.2f} MB  exact {s['bit_exact']}"
        )


if __name__ == "__main__":
    main()
