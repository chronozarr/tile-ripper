"""Full retained readback for every timed HLS HTTP experiment run."""

import argparse
import json
from pathlib import Path

import numpy as np
import zarr
from chronozarr import open_store, validate

from compare_hls_river_30day import verify
from hls_river_30day import save


def sources(path):
    root = path / "observations.zarr"
    manifest = json.loads((root / "manifest.json").read_text())
    result = {}
    for record in manifest["outputs"]:
        source = zarr.open_group(root / record["group"], mode="r").attrs["source"]
        result[source["item_id"], source["requested_band"]] = source
    return result


def main(work, original, measurements=None):
    protocol = json.loads((work / "protocol.json").read_text())
    rows = sum(
        [
            json.loads((work / name).read_text())
            for name in (
                measurements
                or [
                    "measurements.json",
                    "controlled-measurements.json",
                    "paired-measurements.json",
                ]
            )
        ],
        [],
    )
    expected = json.loads((original / "analysis.json").read_text())
    expected_sources = sources(original)
    reference = open_store(original / "playback")
    value_count = mask_count = 0
    full, raw = [], []
    for row in rows:
        out = Path(row["output"])
        assert row["returncode"] == 0
        assert json.loads((out / "analysis.json").read_text()) == expected, row["name"]
        assert not validate(out / "playback"), row["name"]
        store = open_store(out / "playback")
        assert np.array_equal(reference.times, store.times)
        assert (
            reference.bands == store.bands
            and reference.attrs.bands == store.attrs.bands
        )
        assert reference.attrs.crs == store.attrs.crs
        assert [x.transform for x in reference.levels] == [
            x.transform for x in store.levels
        ]
        nvalue = nmask = 0
        for lod in range(len(reference.levels)):
            for t in range(len(protocol["items"])):
                a, b = reference.read(t, lod), store.read(t, lod)
                am, bm = reference.read_mask(t, lod), store.read_mask(t, lod)
                assert a.dtype == b.dtype and a.tobytes() == b.tobytes(), (
                    row["name"],
                    t,
                    lod,
                )
                assert am.tobytes() == bm.tobytes(), (row["name"], t, lod)
                nvalue += a.size
                nmask += am.size
        value_count += nvalue
        mask_count += nmask
        full.append(
            {
                "run": row["name"],
                "stored_samples_all_levels": nvalue,
                "mask_samples_all_levels": nmask,
                "analysis_matches_original": True,
            }
        )
        if row["route"] == "tile":
            assert sources(out) == expected_sources, row["name"]
            result = verify(protocol, out, work / "matched-python-1")
            raw.append(
                {
                    "run": row["name"],
                    "full_source_descriptors_match_original": len(expected_sources),
                    **result,
                }
            )
        print("PASS", row["name"], flush=True)
    receipt = {
        "status": "passed",
        "timed_runs": len(rows),
        "all_level_value_samples_checked": value_count,
        "all_level_mask_samples_checked": mask_count,
        "native_runs_raw_checked": len(raw),
        "raw_asset_checks": sum(x["independent_raw_asset_reads"] for x in raw),
        "full_source_descriptors_checked": len(expected_sources) * len(raw),
        "runs": full,
        "raw_native_checks": raw,
    }
    save(work / "all-run-verification.json", receipt)
    print(
        json.dumps(
            {k: v for k, v in receipt.items() if k not in ["runs", "raw_native_checks"]}
        )
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--original", type=Path, required=True)
    parser.add_argument("--measurements", nargs="+")
    args = parser.parse_args()
    main(args.work.resolve(), args.original.resolve(), args.measurements)
