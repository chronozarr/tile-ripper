"""Worker (main env, GDAL 3.12.2 via rasterio 1.5.2): windowed reads through /vsicurl/ with a thread pool.

GDAL configuration comes from the *process environment* (set by the orchestrator), so it applies to every
thread: this is how a user would tune it. Request accounting comes from GDAL's own CPL_DEBUG lines routed
through Python logging (one atomic line per request): GetFileSize = HEAD, Downloading a-b = ranged GET,
GetFileList = directory-listing GET.
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import hashlib
import json
import logging
import re
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from planwin import WINDOWS  # noqa: E402

import rasterio  # noqa: E402
from rasterio.windows import Window  # noqa: E402


class Capture(logging.Handler):
    def __init__(self) -> None:
        super().__init__()
        self.msgs: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.msgs.append(record.getMessage())


def parse_requests(msgs: list[str]) -> dict:
    head = get = listing = 0
    nbytes = 0
    status: dict[str, int] = {}
    for m in msgs:
        if "GetFileSize(" in m:
            head += 1
            code = re.search(r"response_code=(\d+)", m)
            status[code.group(1) if code else "?"] = (
                status.get(code.group(1) if code else "?", 0) + 1
            )
        elif "VSICURL: Downloading " in m:
            r = re.search(r"Downloading (\d+)-(\d+) ", m)
            if r:
                get += 1
                nbytes += int(r.group(2)) - int(r.group(1)) + 1
        elif "GetFileList(" in m:
            listing += 1
    return {
        "head": head,
        "get_range": get,
        "get_listing": listing,
        "requests": head + get + listing,
        "bytes_requested": nbytes,
        "head_status": status,
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--workload", default="W_A_chip400")
    ap.add_argument("--scenes", type=int, default=24)
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--frozen", required=True)
    a = ap.parse_args()

    cap = Capture()
    lg = logging.getLogger("rasterio")
    lg.addHandler(cap)
    lg.setLevel(logging.DEBUG)
    lg.propagate = False

    scenes = json.loads(Path(a.frozen).read_text())["scenes"][: a.scenes]
    assets = [(s["id"], b, s[b]) for s in scenes for b in ("red", "nir")]
    w = WINDOWS[a.workload]
    win = Window(w["c0"], w["r0"], w["w"], w["h"])

    def task(item: tuple[str, str, str]) -> tuple[str, str]:
        sid, band, href = item
        with rasterio.open("/vsicurl/" + href) as src:
            arr = src.read(1, window=win)
        return f"{sid}|{band}", hashlib.sha256(arr.tobytes()).hexdigest()

    t0 = time.perf_counter()
    with cf.ThreadPoolExecutor(a.threads) as ex:
        hashes = dict(ex.map(task, assets))
    t_work = time.perf_counter() - t0
    out = {
        "tool": "gdal",
        "gdal": rasterio.__gdal_version__,
        "workload": a.workload,
        "threads": a.threads,
        "assets": len(assets),
        "t_work_s": t_work,
        "hashes": hashes,
        **parse_requests(cap.msgs),
    }
    print("RESULT_JSON " + json.dumps(out))


if __name__ == "__main__":
    main()
