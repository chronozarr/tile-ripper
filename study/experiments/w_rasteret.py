"""Worker (runs in the isolated rasteret env): build or read with Rasteret v0.4.1, counting every range request.

phase=build : rasteret.build_from_stac for the AOI/date range (timed; index persisted to --workspace)
phase=read  : load the persisted index, read_window for the frozen record ids (timed)
Prints one JSON object. Fresh process per call (cold tool caches); the persisted Parquet index is the only warm state.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import time
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from planwin import WINDOWS, bounds  # noqa: E402  (shared window definitions, no heavy deps)

import rasteret  # noqa: E402

ES = "https://earth-search.aws.element84.com/v1"
AOI = (-79.10, 35.88, -79.02, 35.95)


class CountingBackend:
    """Delegating StorageBackend that records every get_range / get_ranges call."""

    def __init__(self, inner):
        self.inner = inner
        self.calls: list[tuple[str, int, int]] = []

    async def get_range(self, url, start, length):
        buf = await self.inner.get_range(url, start, length)
        self.calls.append((url, start, length))
        return buf

    async def get_ranges(self, url, ranges):
        bufs = await self.inner.get_ranges(url, ranges)
        self.calls.extend((url, s, n) for s, n in ranges)
        return bufs

    def __getattr__(
        self, name
    ):  # pass through anything else the collection asks of the backend
        return getattr(self.inner, name)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--phase", choices=["build", "read"], required=True)
    ap.add_argument("--workload", default="W_A_chip400")
    ap.add_argument("--scenes", type=int, default=24)
    ap.add_argument("--workspace", required=True)
    ap.add_argument("--frozen", required=True)
    ap.add_argument("--name", default="s2c1_17SPV_2025")
    ap.add_argument("--max-concurrent", type=int, default=50)
    a = ap.parse_args()

    frozen = json.loads(Path(a.frozen).read_text())["scenes"][: a.scenes]
    ids = [s["id"] for s in frozen]
    backend = CountingBackend(rasteret.create_backend(region="us-west-2"))
    out: dict = {
        "phase": a.phase,
        "workload": a.workload,
        "rasteret": getattr(rasteret, "__version__", "?"),
    }
    t0 = time.perf_counter()
    if a.phase == "build":
        col = rasteret.build_from_stac(
            name=a.name,
            stac_api=ES,
            collection="sentinel-2-c1-l2a",
            data_source="earthsearch/sentinel-2-l2a",
            band_map={"B04": "red", "B08": "nir"},
            bbox=AOI,
            date_range=("2025-01-01", "2025-09-05"),
            query={"eo:cloud_cover": {"lte": 20}},
            workspace_dir=a.workspace,
            force=True,
            backend=backend,
            max_concurrent=a.max_concurrent,
        )
        out["t_work_s"] = time.perf_counter() - t0
        have = (
            set(col.dataset.to_table(columns=["id"])["id"].to_pylist())
            if hasattr(col, "dataset")
            else set()
        )
        out["records_in_index"] = len(have)
        out["frozen_missing_from_index"] = sorted(set(ids) - have)
        out["index_extra_ids"] = sorted(have - set(ids))
    else:
        indexes = sorted(Path(a.workspace).glob("*_stac"))
        assert len(indexes) == 1, (
            f"expected exactly one persisted index in {a.workspace}, found {indexes}"
        )
        col = rasteret.load(str(indexes[0]))
        t_load = time.perf_counter() - t0
        win = WINDOWS[a.workload]
        xmin, ymin, xmax, ymax = bounds(win)
        t1 = time.perf_counter()
        arr = col.read_window(
            record_ids=ids,
            bounds=(xmin, ymin, xmax, ymax),
            res=(10.0, 10.0),
            bands=["B04", "B08"],
            target_crs=32617,
            max_concurrent=a.max_concurrent,
            backend=backend,
            group_by="id",
        )
        out["t_load_index_s"] = t_load
        out["t_read_s"] = time.perf_counter() - t1
        out["t_work_s"] = time.perf_counter() - t0
        out["array_shape"] = list(arr.shape)
        out["dtype"] = str(arr.dtype)
        out["hashes"] = (
            {
                f"{ids[t]}|{b}": hashlib.sha256(
                    np.ascontiguousarray(arr[t, c]).tobytes()
                ).hexdigest()
                for t in range(arr.shape[0])
                for c, b in enumerate(["red", "nir"])
            }
            if arr.shape[0] == len(ids)
            else {}
        )
    calls = backend.calls
    out["requests"] = len(calls)
    out["bytes_requested"] = int(sum(n for _, _, n in calls))
    out["small_calls_le_64KiB"] = int(sum(1 for _, _, n in calls if n <= 65536))
    out["large_calls_gt_64KiB"] = int(sum(1 for _, _, n in calls if n > 65536))
    print("RESULT_JSON " + json.dumps(out))


if __name__ == "__main__":
    main()
