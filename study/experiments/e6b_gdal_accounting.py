"""E6b: libcurl-level request accounting for GDAL (separate, untimed runs).

The timed runs count requests from GDAL's CPL_DEBUG lines through Python logging. Those lines are dropped when
GDAL emits them from its internal decode threads (GDAL_NUM_THREADS=ALL_CPUS on multi-block windows), so the
timed-run counts are only trusted where this accounting agrees. Here every request is counted from libcurl's
own verbose lines (one '> GET|HEAD' and one '> Range:' line per request; counting lines is thread-safe).
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path

from common import save
from e6_pixels import FROZEN, PY, TUNED, nic_in_bytes

HERE = Path(__file__).resolve().parent


def parse_curl(stderr: str) -> dict:
    head = get = 0
    ranged = 0
    rbytes = 0
    status: dict[str, int] = {}
    for line in stderr.splitlines():
        if line.startswith("> HEAD "):
            head += 1
        elif line.startswith("> GET "):
            get += 1
        elif line.startswith("> Range: bytes="):
            for a, b in re.findall(r"(\d+)-(\d+)", line):
                rbytes += int(b) - int(a) + 1
            ranged += 1
        elif line.startswith("< HTTP/"):
            code = line.split()[1]
            status[code] = status.get(code, 0) + 1
    return {
        "curl_head": head,
        "curl_get": get,
        "curl_range_headers": ranged,
        "curl_requests": head + get,
        "curl_range_bytes": rbytes,
        "curl_status": status,
    }


def run(
    workload: str, scenes: int, threads: int, env: dict, label: str, mode: str = "curl"
) -> dict:
    """mode 'curl': libcurl verbose to stderr (CPL_DEBUG off). mode 'logged': CPL_DEBUG lines via Python logging."""
    cmd = [
        PY,
        "-u",
        str(HERE / "w_gdal.py"),
        "--workload",
        workload,
        "--scenes",
        str(scenes),
        "--threads",
        str(threads),
        "--frozen",
        FROZEN,
    ]
    e = {
        **os.environ,
        **env,
        **({"CPL_CURL_VERBOSE": "YES"} if mode == "curl" else {"CPL_DEBUG": "ON"}),
    }
    n0 = nic_in_bytes()
    p = subprocess.run(
        cmd, env=e, capture_output=True, text=True, timeout=900, cwd=HERE
    )
    n1 = nic_in_bytes()
    line = next(ln for ln in p.stdout.splitlines() if ln.startswith("RESULT_JSON "))
    r = json.loads(line[12:])
    r.pop("hashes", None)
    out = {
        "label": f"{label} [{mode}]",
        "workload": workload,
        "scenes": scenes,
        "threads": threads,
        "t_work_s_NOT_for_timing": r["t_work_s"],
        "logged": {
            k: r[k]
            for k in ("head", "get_range", "get_listing", "requests", "bytes_requested")
        },
        **parse_curl(p.stderr),
        "nic_bytes_in": n1 - n0,
    }
    print(
        f"{out['label']:44s} {workload} logged req {out['logged']['requests']:4d} ({out['logged']['bytes_requested'] / 1e6:7.2f} MB) | "
        f"curl req {out['curl_requests']:4d} [HEAD {out['curl_head']} GET {out['curl_get']}] range bytes {out['curl_range_bytes'] / 1e6:7.2f} MB | NIC {out['nic_bytes_in'] / 1e6:7.2f} MB",
        flush=True,
    )
    return out


def main() -> None:
    res = []
    res.append(run("W_A_chip400", 24, 16, TUNED, "gdal-tuned-T16"))
    res.append(run("W_A_chip400", 24, 4, {}, "gdal-default-T4"))
    res.append(run("W_B_corner600", 12, 16, TUNED, "gdal-tuned-T16"))
    res.append(
        run(
            "W_B_corner600",
            12,
            16,
            {**TUNED, "GDAL_NUM_THREADS": "1"},
            "gdal-tuned-T16 GDAL_NUM_THREADS=1",
        )
    )
    res.append(
        run(
            "W_B_corner600",
            12,
            16,
            {**TUNED, "GDAL_NUM_THREADS": "1"},
            "gdal-tuned-T16 GDAL_NUM_THREADS=1",
            mode="logged",
        )
    )
    res.append(run("W_B_corner600", 12, 4, {}, "gdal-default-T4"))
    save("e6b_gdal_accounting", {"runs": res})


if __name__ == "__main__":
    sys.exit(main())
