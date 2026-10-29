#!/usr/bin/env python3
"""Offline, independent source/output check for the direct Chronozarr bridge.

Run with the sibling Chronozarr checkout's Python environment. The input is a
retained, completed scientific COG fetch; this script performs no acquisition.
The output directory must be new. Timings are a local probe, not a benchmark.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import time
from pathlib import Path

import numpy as np
import zarr

from chronozarr import convert_tile_ripper, open_store, validate
from chronozarr.convert import convert


def fingerprint(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(p for p in root.rglob("*") if p.is_file()):
        digest.update(path.relative_to(root).as_posix().encode())
        with path.open("rb") as handle:
            for block in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(block)
    return digest.hexdigest()


def check(
    source: Path, output: Path, bands: list[str], policy: str | None, stack: Path | None
) -> dict:
    original = fingerprint(source)
    manifest = json.loads((source / "manifest.json").read_text())
    records = []
    for record in manifest["outputs"]:
        group = zarr.open_group(str(source / record["group"]), mode="r")
        records.append((record, group, dict(group.attrs["source"])))
    started = time.perf_counter()
    if stack is None:
        report = convert_tile_ripper(
            source,
            output,
            bands=bands,
            quality_policy=policy,
            chunk_size=64,
            n_lods=2,
            workers=1,
        )
    else:
        assert policy is None, "cross-observation QA uses the direct bridge"
        group = zarr.open_group(str(stack), mode="r")
        assert list(group.attrs["bands"]) == bands
        observations = list(group.attrs["observations"])
        assert all(o["object_version"] for o in observations)
        notes = {
            "importer": "stack_then_generic_with_explicit_provenance",
            "manifest_sha256": hashlib.sha256(
                (source / "manifest.json").read_bytes()
            ).hexdigest(),
            "observations": [
                {"group": o["source_group"], "source": o["source"]}
                for o in observations
            ],
        }
        provenance = {
            "sources": list(dict.fromkeys(o["href"] for o in observations)),
            "composite": "none: one source item per timestep",
            "gap_fill": "none",
            "notes": json.dumps(notes, sort_keys=True),
        }
        converted = convert(
            stack,
            output,
            variable="values",
            mask_var="valid_all",
            chunk_size=64,
            n_lods=2,
            workers=1,
            provenance=provenance,
        )
        report = converted.encode
        assert report is not None
    elapsed = time.perf_counter() - started
    problems = validate(output)
    assert not problems, problems
    store = open_store(output)
    notes = json.loads(store.attrs.provenance["notes"])
    assert (
        notes["manifest_sha256"]
        == hashlib.sha256((source / "manifest.json").read_bytes()).hexdigest()
    )
    retained = {o["group"]: o["source"] for o in notes["observations"]}
    selected = [r for r in records if r[2]["requested_band"] in bands]
    native = selected[0][0]["native_grid"]
    transform = native["transform"]
    data = zarr.open_group(str(output), mode="r")["0/data"]
    assert list(data.attrs["spatial:transform"]) == [
        transform[i] for i in [1, 2, 0, 4, 5, 3]
    ]
    assert (
        zarr.open_group(str(output), mode="r").attrs["chronozarr"]["crs"]
        == native["crs"]
    )
    dates = sorted({r[0]["datetime"] for r in selected})
    assert len(store.times) == len(dates)
    checks, counts = [], []
    for t, date in enumerate(dates):
        slot = [
            next(
                r
                for r in selected
                if r[0]["datetime"] == date and r[2]["requested_band"] == band
            )
            for band in bands
        ]
        truth = np.stack([np.asarray(g["values"][0]) for _, g, _ in slot])
        valid = np.ones(truth.shape[1:], dtype=np.uint8)
        for b, (record, group, descriptor) in enumerate(slot):
            assert retained[record["group"]] == descriptor
            valid &= np.asarray(group["valid"][0])
            valid &= np.asarray(group["aoi_mask"][0])
            assert store.attrs.bands[b].scale == group["values"].attrs.get(
                "scale_factor", 1
            )
            assert store.attrs.bands[b].offset == group["values"].attrs.get(
                "add_offset", 0
            )
            assert store.attrs.bands[b].units == group["values"].attrs.get("units")
        physical_valid = int(np.count_nonzero(valid))
        if policy:
            qa = [
                r
                for r in records
                if r[2]["item_id"] == slot[0][2]["item_id"]
                and r[0]["datetime"] == date
                and any(p["policy"] == policy for p in r[0].get("derived_masks", []))
            ]
            assert len(qa) == 1
            record, group, descriptor = qa[0]
            assert retained[record["group"]] == descriptor
            valid &= np.asarray(group[f"quality_{policy}"][0])
            valid &= np.asarray(group["valid"][0])
            valid &= np.asarray(group["aoi_mask"][0])
        assert store.read(t).tobytes() == truth.tobytes(), (
            f"raw values differ at {date}"
        )
        assert np.array_equal(store.read_mask(t), valid), f"mask differs at {date}"
        good = np.argwhere(valid != 0)
        if len(good):
            y, x = map(int, good[0])
            checks.append(
                {
                    "t": t,
                    "x": x,
                    "y": y,
                    "stored": truth[:, y, x].tolist(),
                    "physical": [
                        float(truth[b, y, x]) * store.attrs.bands[b].scale
                        + store.attrs.bands[b].offset
                        for b in range(len(bands))
                    ],
                }
            )
        counts.append(
            {
                "time": date,
                "source_valid_pixels": physical_valid,
                "final_valid_pixels": int(np.count_nonzero(valid)),
                "quality_excluded_pixels": physical_valid
                - int(np.count_nonzero(valid)),
            }
        )
    assert fingerprint(source) == original, "input files changed"
    return {
        "status": "passed",
        "input": source.name,
        "output": str(output.resolve()),
        "input_payload_sha256": original,
        "manifest_sha256": notes["manifest_sha256"],
        "shape": [len(dates), len(bands), *store.read(0).shape[1:]],
        "bands": bands,
        "quality_policy": policy,
        "conversion_route": "direct"
        if stack is None
        else "stack_then_generic_with_explicit_provenance",
        "exact_level0_bytes": True,
        "exact_masks": True,
        "source_descriptors_preserved": len(retained),
        "input_unchanged": True,
        "validation_problems": problems,
        "local_probe_seconds": elapsed,
        "output_bytes": report.total_bytes,
        "counts": counts,
        "browser_pixel_checks": checks,
        "scope": "retained local captures; no acquisition, speed comparison or human trial",
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--bands", nargs="+", required=True)
    parser.add_argument("--quality-policy")
    parser.add_argument(
        "--stack",
        type=Path,
        help="Verify an already written stack subgroup through generic conversion",
    )
    parser.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args()
    result = check(
        args.source, args.output, args.bands, args.quality_policy, args.stack
    )
    args.receipt.parent.mkdir(parents=True, exist_ok=True)
    args.receipt.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
