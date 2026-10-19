"""Shared helpers for the study probes. Kept deliberately small."""

from __future__ import annotations

import json
import platform
import statistics
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
RESULTS = HERE / "results"
RESULTS.mkdir(exist_ok=True)

USER_AGENT = (
    "tile-ripper-study/0.1 (jake gearon, UNC Global Hydrology Lab; benchmark probe)"
)

# Hosts used by the study. Etiquette: identifying UA, bounded volume, stop on sustained 429/503.
S3_HOST = "e84-earth-search-sentinel-data.s3.us-west-2.amazonaws.com"
TGE_BASE = "https://data.source.coop/tge-labs/s2-stac-geoparquet"
ES_API = "https://earth-search.aws.element84.com/v1"


def run(cmd: list[str]) -> str:
    return subprocess.run(
        cmd, capture_output=True, text=True, check=False
    ).stdout.strip()


def env_manifest() -> dict:
    """Machine + tool versions, recorded with every result file (spec 04 section 3)."""
    return {
        "utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "uname": platform.platform(),
        "cpu": run(["sysctl", "-n", "machdep.cpu.brand_string"]),
        "cores": run(["sysctl", "-n", "hw.ncpu"]),
        "mem_gb": round(int(run(["sysctl", "-n", "hw.memsize"]) or 0) / 2**30, 1),
        "python": sys.version.split()[0],
    }


def summarize(xs: list[float]) -> dict:
    xs = sorted(xs)
    n = len(xs)
    if n == 0:
        return {"n": 0}
    q = lambda p: xs[min(n - 1, max(0, round(p * (n - 1))))]  # noqa: E731
    return {
        "n": n,
        "min": xs[0],
        "p25": q(0.25),
        "median": statistics.median(xs),
        "p75": q(0.75),
        "max": xs[-1],
    }


def save(name: str, payload: dict) -> Path:
    path = RESULTS / f"{name}.json"
    payload = {"env": env_manifest(), **payload}
    path.write_text(json.dumps(payload, indent=2, default=str))
    print(f"[saved] {path}")
    return path
