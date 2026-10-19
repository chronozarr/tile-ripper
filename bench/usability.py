"""Unfamiliar public CLI requests; independent xarray oracle via existing harness.
Run with prototype-output/zarr-env/bin/python bench/usability.py.
No dependencies, downloader or codecs added. At most ~15 MB expected scientific bodies.
"""

import copy
import itertools
import json
from pathlib import Path
import time
import numpy as np
import xarray as xr
import zarr
import cf_zarr as core
import remote_zarr as h

ROOT = h.ROOT
WORK = ROOT / "prototype-output/external-usability"


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    records = []
    for label, template, bbox, start, end, variables in [
        (
            "era5-new",
            "weatherbench2",
            [100, -10, 110, 0],
            "1960-02-02T12:00:00Z",
            "1960-02-02T18:00:00Z",
            ["2m_temperature"],
        ),
        (
            "imerg-new",
            "imerg",
            [8.02, 45.02, 8.18, 45.18],
            "2021-01-03T00:00:00Z",
            "2021-01-03T00:00:00Z",
            ["IMERG_PRECTOT"],
        ),
    ]:
        r = json.loads((ROOT / f"examples/{template}.json").read_text())
        asset = r["source"]["assets"][0]
        asset["id"] = "subset"
        r["spatial"]["bbox"] = bbox
        r["time"].update(start=start, end=end)
        r["variables"] = [{"kind": "dataset", "path": v} for v in variables]
        r["budget"].update(buffer_bytes=33554432, chunk_edge=512)
        output = WORK / label
        r["sink"]["path"] = str(output)
        args = [
            "--source",
            asset["locator"],
            "--format",
            "zarr_v2",
            "--bbox",
            ",".join(map(str, bbox)),
            "--start",
            start,
            "--end",
            end,
            "--variables",
            ",".join(variables),
        ]
        dry = h.process(
            [str(h.CURRENT), "fetch", *args, "--dry-run", "--output", str(output)]
        )
        assert dry["code"] == 0 and not output.exists(), dry
        assert dry["result"]["document_type"] == "inspection_only"
        row = h.process([str(h.CURRENT), "fetch", *args, "--output", str(output)])
        assert row["code"] == 0, row
        manifest = row.pop("result")
        # Independent xarray/Zarr metadata interpretation and value decoding.
        store = core.Mapping(asset["locator"], 16777216, 100000)
        yn = "lat" if template == "imerg" else "latitude"
        xn = "lon" if template == "imerg" else "longitude"
        store.names = ["time", yn, xn] + variables
        excluded = [
            k.split("/")[0]
            for k in store.metadata
            if k.endswith("/.zarray") and k.split("/")[0] not in store.names
        ]
        ds = xr.open_zarr(
            store,
            consolidated=True,
            decode_cf=False,
            chunks=None,
            drop_variables=excluded,
        )
        ref, indices = core.selection(r, ds)
        ref = ref.transpose("time", yn, xn).load()
        got = xr.open_zarr(
            str(output / "subset.zarr"),
            consolidated=False,
            decode_cf=False,
            chunks=None,
        ).load()
        xr.testing.assert_identical(ref, got)
        for variable in variables:
            np.testing.assert_array_equal(ref[variable], got[variable])
            np.testing.assert_array_equal(
                np.isnan(ref[variable]), np.isnan(got[variable])
            )
            assert got[variable].dtype == np.dtype("float32")
        decoded = xr.open_zarr(
            str(output / "subset.zarr"), consolidated=False, chunks=None
        )
        assert str(decoded.time.values[0])[:10] == start[:10]
        group = zarr.open_group(store=store, mode="r")
        keys = []
        for v in variables:
            ar = group[v]
            ranges = [
                sorted(set((indices[d] // c).tolist()))
                for d, c in zip(ds[v].dims, ar.chunks)
            ]
            keys.extend(
                v + "/" + ar._dimension_separator.join(map(str, i))
                for i in itertools.product(*ranges)
            )
        reads = manifest["observed"]["source_reads"]
        assert sorted(v["key"] for v in reads) == sorted(keys)
        assert all(v["requests"] == 1 for v in reads)
        assert manifest["status"] == "complete"
        # Existing matched durable xarray baseline; first extraction compared only.
        core.WORK = WORK
        (WORK / "manifest-template.json").write_text(json.dumps(manifest))
        baseline_request = copy.deepcopy(r)
        baseline_request["sink"]["path"] = str(WORK / (label + "-xarray"))
        before = time.perf_counter()
        baseline = core.baseline(baseline_request)
        baseline["two_fetch_wall_seconds"] = time.perf_counter() - before
        baseline_output = Path(baseline["runs"][0]["output"]) / "subset.zarr"
        xr.testing.assert_identical(
            got,
            xr.open_zarr(
                str(baseline_output), consolidated=False, decode_cf=False, chunks=None
            ).load(),
        )
        records.append(
            dict(
                label=label,
                request=r,
                dry_run=dry,
                cli=row,
                observed=manifest["observed"],
                minimum_keys=keys,
                validation="xarray assert_identical including native Float32, NaNs, coordinates and attrs; decoded time opens",
                shape=dict(got.sizes),
                cells=sum(got[v].size for v in variables),
                baseline=baseline,
                first_success_seconds=dry["wall_seconds"] + row["wall_seconds"]
                if "wall_seconds" in row
                else None,
            )
        )
    result = dict(
        trial_kind="primary-agent scripted validation, not independent newcomer/human trial",
        runs=records,
        notes=[
            "First-success includes one dry-run and acquisition; excludes build and source choice.",
            "One new request per source; no speedup inference from single public-network observations.",
            "Baseline uses existing durable native-output writer and dependency checks; diagnostic template supplied outside timing.",
        ],
    )
    (ROOT / "docs/external-usability-public.json").write_text(
        json.dumps(result, indent=2) + "\n"
    )
    print(
        json.dumps(
            [
                {k: v for k, v in r.items() if k in ["label", "cells", "shape"]}
                for r in records
            ]
        )
    )


if __name__ == "__main__":
    main()
