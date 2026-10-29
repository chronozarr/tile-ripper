"""Independent readback and summary for the matched river experiment."""

import argparse
import hashlib
import json
from pathlib import Path
import statistics

import numpy as np
import zarr
from chronozarr import open_store, validate

from hls_river_30day import DEFAULT, save


def sha(values):
    return hashlib.sha256(np.asarray(values).tobytes()).hexdigest()


def verify(protocol, tile, python):
    left, right = open_store(tile / "playback"), open_store(python / "playback")
    assert not validate(tile / "playback")
    assert not validate(python / "playback")
    assert left.times.tolist() == right.times.tolist()
    assert left.bands == right.bands
    assert left.attrs.bands == right.attrs.bands
    assert left.attrs.crs == right.attrs.crs == protocol["native"]["crs"]
    assert [x.transform for x in left.levels] == [x.transform for x in right.levels]
    values_checked = masks_checked = 0
    pixels = []
    for lod in range(len(left.levels)):
        assert left.levels[lod].shape == right.levels[lod].shape
        for t, item in enumerate(protocol["items"]):
            a, b = left.read(t, lod), right.read(t, lod)
            assert a.dtype == b.dtype == np.dtype("int16")
            assert a.tobytes() == b.tobytes(), (
                lod,
                item["id"],
                "stored samples differ",
            )
            am, bm = left.read_mask(t, lod), right.read_mask(t, lod)
            assert np.array_equal(am, bm), (lod, item["id"], "QA/AOI masks differ")
            values_checked += a.size
            masks_checked += am.size
            if lod == 0:
                good = np.argwhere(am != 0)
                if len(good):
                    # Middle accepted pixel usually stays well within the viewport.
                    y, x = map(int, good[len(good) // 2])
                    pixels.append(
                        {
                            "t": t,
                            "x": x,
                            "y": y,
                            "stored": a[:, y, x].tolist(),
                            "physical": (a[:, y, x].astype(float) * 0.0001).tolist(),
                        }
                    )
    expected_pins = {(p["item"], p["band"]): p for p in protocol["pins"]}
    python_stages = json.loads((python / "stages.json").read_text())
    independent = {x["item"]: x for x in python_stages["source_checks"]}
    root = tile / "observations.zarr"
    manifest_bytes = (root / "manifest.json").read_bytes()
    manifest = json.loads(manifest_bytes)
    assert manifest["status"] == "complete"
    observed = set()
    raw_checks = []
    for record in manifest["outputs"]:
        group = zarr.open_group(root / record["group"], mode="r")
        source = group.attrs["source"]
        key = source["item_id"], source["requested_band"]
        pin = expected_pins[key]
        assert key not in observed
        observed.add(key)
        assert source["href"] == pin["href"] and source["object_version"] == pin["etag"]
        assert source["product_version"] == "2.0"
        raw = np.asarray(group["values"][0])
        assert sha(raw) == independent[key[0]]["asset_sha256"][key[1]], (
            key,
            "independent COG read differs",
        )
        raw_checks.append({"item": key[0], "band": key[1], "sha256": sha(raw)})
        if key[1] == "Fmask":
            aoi = np.asarray(group["aoi_mask"][0])
            valid = np.asarray(group["valid"][0])
            expected_qa = (aoi & valid & (raw != 255) & ((raw & 0x1E) == 0)).astype(
                "uint8"
            )
            assert np.array_equal(group["quality_hls_fmask_clear"][0], expected_qa)
    assert observed == set(expected_pins), "source selection omitted or added an asset"
    notes = json.loads(left.attrs.provenance["notes"])
    assert notes["manifest_sha256"] == hashlib.sha256(manifest_bytes).hexdigest()
    retained = {
        (o["source"]["item_id"], o["source"]["requested_band"]): o["source"]
        for o in notes["observations"]
    }
    assert set(retained) == set(expected_pins)
    for key, pin in expected_pins.items():
        assert (
            retained[key]["href"] == pin["href"]
            and retained[key]["object_version"] == pin["etag"]
        )
    python_notes = json.loads(right.attrs.provenance["notes"])
    assert python_notes["pins"] == protocol["pins"]
    assert json.loads((tile / "analysis.json").read_text()) == json.loads(
        (python / "analysis.json").read_text()
    )
    for run in [tile, python]:
        save(
            run.parent / f"{run.name}-browser-input.json",
            {
                "output": str(run / "playback"),
                "browser_pixel_checks": pixels,
                "screenshot_t": 3,
                "browser_invalid_checks": [
                    {
                        "t": t,
                        "x": protocol["window"][2] // 2,
                        "y": protocol["window"][3] // 2,
                    }
                    for t in range(len(protocol["items"]))
                    if not left.read_mask(t).any()
                ],
            },
        )
    return {
        "status": "passed",
        "stored_samples_all_levels": values_checked,
        "mask_samples_all_levels": masks_checked,
        "independent_raw_asset_reads": len(raw_checks),
        "retained_object_pins": len(retained),
        "raw_checks": raw_checks,
        "browser_clear_timesteps": len(pixels),
        "empty_timesteps": [
            i for i in range(len(protocol["items"])) if not left.read_mask(i).any()
        ],
    }


def summarize(work, names):
    protocol = json.loads((work / "protocol.json").read_text())
    measurements = [
        json.loads((work / f"{name}-measurement.json").read_text()) for name in names
    ]
    assert all(x["returncode"] == 0 for x in measurements)
    routes = {
        route: [x for x in measurements if x["route"] == route]
        for route in ["tile", "python"]
    }
    assert all(len(x) == 3 for x in routes.values())
    tile, python = (Path(routes[x][0]["output"]) for x in ["tile", "python"])
    verification = verify(protocol, tile, python)
    payload = json.loads((tile / "analysis.json").read_text())
    for x in measurements:
        assert json.loads((Path(x["output"]) / "analysis.json").read_text()) == payload
    summary = {}
    for route, runs in routes.items():
        summary[route] = {
            "runs": [x["name"] for x in runs],
            "wall_s": [x["wall_s"] for x in runs],
            "median_wall_s": statistics.median(x["wall_s"] for x in runs),
            "median_peak_tree_rss_mib": statistics.median(
                x["sampled_peak_tree_rss_bytes"] / 2**20 for x in runs
            ),
            "median_peak_work_mib": statistics.median(
                x["sampled_peak_work_logical_bytes"] / 2**20 for x in runs
            ),
            "median_peak_work_allocated_mib": statistics.median(
                x["sampled_peak_work_allocated_bytes"] / 2**20 for x in runs
            ),
            "median_retained_work_mib": statistics.median(
                x["retained_work_bytes"] / 2**20 for x in runs
            ),
            "median_cpu_s": statistics.median(x["sampled_cpu_s"] for x in runs),
            "median_acquisition_s": statistics.median(
                x["stages"]["acquisition_s"] for x in runs
            ),
            "retained_files": [x["retained_files"] for x in runs],
        }
    summary["tile_wall_ratio_to_python"] = (
        summary["tile"]["median_wall_s"] / summary["python"]["median_wall_s"]
    )
    result = {
        "status": "passed",
        "protocol": protocol,
        "verification": verification,
        "measurements": summary,
        "analysis": payload,
    }
    save(work / "comparison.json", result)
    print(
        json.dumps(
            {
                "verification": {
                    k: v for k, v in verification.items() if k != "raw_checks"
                },
                "measurements": summary,
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path, default=DEFAULT)
    parser.add_argument(
        "--names",
        default="final-python-1,final-tile-1,final-tile-2,final-python-2,final-python-3,final-tile-3",
    )
    args = parser.parse_args()
    summarize(args.work, args.names.split(","))
