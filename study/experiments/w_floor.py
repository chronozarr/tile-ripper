"""Worker (main env): execute a planner manifest with the most minimal fetcher possible.

This is the spec's T_io_floor path: one HTTP/1.1 ranged GET per manifest row, N threads, bytes discarded
(or, with --decode, inflated + predictor-undone with numpy and hashed so the manifest can be proven
correct against the GDAL oracle). It is a measurement tool, not an engine: no retries, no scheduling.
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import hashlib
import json
import sys
import time
import zlib
from pathlib import Path

import httpx
import numpy as np
import pyarrow.parquet as pq

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from common import USER_AGENT  # noqa: E402
from planwin import WINDOWS  # noqa: E402


def decode_window(buf_by_row: dict, rows: list[dict], w: dict, tile: int) -> np.ndarray:
    """Assemble the pixel window of one asset from its manifest rows (deflate + horizontal predictor)."""
    out = np.zeros((w["h"], w["w"]), dtype=np.uint16)
    for row in rows:
        buf = buf_by_row[(row["href"], row["grp"])]
        for t in row["tiles"]:
            raw = zlib.decompress(
                buf[
                    t["off"] - row["range_start"] : t["off"]
                    - row["range_start"]
                    + t["len"]
                ]
            )
            a = np.cumsum(
                np.frombuffer(raw, dtype="<u2").reshape(tile, tile),
                axis=1,
                dtype=np.uint16,
            )
            ty0, tx0 = t["ty"] * tile, t["tx"] * tile
            r0, r1 = max(ty0, w["r0"]), min(ty0 + tile, w["r0"] + w["h"])
            c0, c1 = max(tx0, w["c0"]), min(tx0 + tile, w["c0"] + w["w"])
            out[r0 - w["r0"] : r1 - w["r0"], c0 - w["c0"] : c1 - w["c0"]] = a[
                r0 - ty0 : r1 - ty0, c0 - tx0 : c1 - tx0
            ]
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", required=True)
    ap.add_argument("--workload", default="W_A_chip400")
    ap.add_argument("--frozen", required=True)
    ap.add_argument("--scenes", type=int, default=24)
    ap.add_argument("--conc", type=int, default=32)
    ap.add_argument("--decode", action="store_true")
    a = ap.parse_args()

    keep = {
        s["id"] for s in json.loads(Path(a.frozen).read_text())["scenes"][: a.scenes]
    }
    rows = [
        r
        for r in pq.read_table(a.manifest).to_pylist()
        if r["scene_id"] in keep and r["band"] in ("red", "nir")
    ]
    limits = httpx.Limits(max_connections=a.conc, max_keepalive_connections=a.conc)
    store: dict = {}
    sizes: list[int] = []

    def fetch(r: dict) -> None:
        resp = c.get(
            r["href"], headers={"Range": f"bytes={r['range_start']}-{r['range_end']}"}
        )
        resp.raise_for_status()
        want = r["range_end"] - r["range_start"] + 1
        assert len(resp.content) == want, (
            f"short read {len(resp.content)} != {want} for {r['href']}"
        )
        assert resp.headers.get("ETag", "").strip('"') == r["etag"], (
            f"ETag changed for {r['href']}"
        )
        sizes.append(len(resp.content))
        if a.decode:
            store[(r["href"], r["grp"])] = resp.content

    t0 = time.perf_counter()
    with httpx.Client(
        http2=False, timeout=60, limits=limits, headers={"User-Agent": USER_AGENT}
    ) as c:
        with cf.ThreadPoolExecutor(a.conc) as ex:
            list(ex.map(fetch, rows))
    t_fetch = time.perf_counter() - t0
    out = {
        "tool": "manifest-floor",
        "workload": a.workload,
        "conc": a.conc,
        "requests": len(rows),
        "bytes_requested": int(sum(sizes)),
        "t_work_s": t_fetch,
        "t_fetch_s": t_fetch,
    }
    if a.decode:
        w = WINDOWS[a.workload]
        by_asset: dict = {}
        for r in rows:
            by_asset.setdefault((r["scene_id"], r["band"]), []).append(r)
        t1 = time.perf_counter()
        out["hashes"] = {
            f"{sid}|{band}": hashlib.sha256(
                decode_window(store, rs, w, 1024).tobytes()
            ).hexdigest()
            for (sid, band), rs in by_asset.items()
        }
        out["t_decode_serial_s"] = time.perf_counter() - t1
    print("RESULT_JSON " + json.dumps(out))


if __name__ == "__main__":
    main()
