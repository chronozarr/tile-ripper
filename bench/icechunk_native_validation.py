#!/usr/bin/env python3
"""Exercise native plan/stream/fetch against independently recorded Icechunk values.

Requires numpy and zarr. All commands use fixed snapshots, a 2x2 or 101x101 window,
and one or 24 times. A new output directory is required. Each CLI process opens
a fresh session; provider/OS caches are uncontrolled. Timings are descriptive,
and materialized/virtual products are never considered interchangeable.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import subprocess
import time

import numpy as np
import zarr

ROOT = Path(__file__).resolve().parents[1]


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def invoke(binary: Path, command: str, request: Path, destination: Path) -> tuple[object, float]:
    argv = [str(binary), command, str(request)]
    if command == "fetch":
        argv += ["--output", str(destination / "output")]
    started = time.perf_counter()
    result = subprocess.run(argv, capture_output=True, text=True, timeout=240, cwd=ROOT)
    elapsed = time.perf_counter() - started
    (destination / f"{command}.stdout").write_text(result.stdout)
    (destination / f"{command}.stderr").write_text(result.stderr)
    if result.returncode:
        raise RuntimeError(f"{command} failed ({result.returncode}): {result.stderr[-3000:]}")
    if command == "stream":
        rows = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
        if not rows or rows[-1].get("type") != "complete":
            raise AssertionError("stream did not return validated completion")
        return rows, elapsed
    return json.loads(result.stdout), elapsed


def run(args: argparse.Namespace) -> dict:
    binary = args.binary.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    reference_path = ROOT / "docs/icechunk/reference-receipt.json"
    reference = json.loads(reference_path.read_text())
    receipt = {
        "status": "running",
        "created_utc": datetime.now(timezone.utc).isoformat(),
        "binary": str(binary),
        "binary_profile": binary.parent.name if binary.parent.name in ("debug", "release") else "unspecified",
        "binary_sha256": digest(binary),
        "harness_sha256": digest(Path(__file__).resolve()),
        "reference_receipt_sha256": digest(reference_path),
        "scope": "fixed GFS analysis snapshots; exact native values, coordinates and CLI completion",
        "measurement_limits": [
            "Each command starts a fresh process/session; provider and OS cache states are unknown.",
            "Reported wall times include preparation and process startup; fetch includes durable output.",
            "A debug executable is not evidence of release-build throughput.",
            "Native transport counters cover physical payload bodies, not Icechunk library metadata HTTP.",
            "Reference values come from a separate maintained Python reader and recorded source bodies.",
            "Materialized and virtual sources have different numerical representations; no automatic routing.",
        ],
        "cases": [],
    }
    try:
        for kind in args.products:
            product = reference["products"][kind]
            template = json.loads((ROOT / f"examples/icechunk-gfs-{kind}.json").read_text())
            for workload in product["workloads"]:
                if workload["name"] not in args.workloads:
                    continue
                for trial in range(args.repeats):
                    directory = output / f"{kind}-{workload['name']}-{trial}"
                    directory.mkdir()
                    request = json.loads(json.dumps(template))
                    request["time"] = {
                        "kind": "instants",
                        "values": [
                            datetime.fromtimestamp(t, timezone.utc).isoformat().replace("+00:00", "Z")
                            for t in workload["selected_coordinates"]["time"]
                        ],
                    }
                    coordinates = workload["selected_coordinates"]
                    request["spatial"]["bbox"] = [
                        min(coordinates["longitude"]) - 0.125,
                        min(coordinates["latitude"]) - 0.125,
                        max(coordinates["longitude"]) + 0.125,
                        max(coordinates["latitude"]) + 0.125,
                    ]
                    request_path = directory / "request.json"
                    request_path.write_text(json.dumps(request, indent=2) + "\n")
                    plan, plan_seconds = invoke(binary, "plan", request_path, directory)
                    source = plan["sources"][0]["object_identities"]
                    assert source["snapshot_id"] == product["snapshot_id"]
                    assert plan["expected_outputs"][0]["dimensions"]["shape"] == workload["repeats"][0]["values"]["shape"]
                    stream, stream_seconds = invoke(binary, "stream", request_path, directory)
                    fetched, fetch_seconds = invoke(binary, "fetch", request_path, directory)
                    assert fetched["status"] == "complete"
                    assert fetched["outputs"][0]["native_grid"]["crs"] == request["spatial"]["crs"]
                    group = zarr.open_group(str(directory / "output/gfs.zarr"), mode="r")
                    values = np.asarray(group["temperature_2m"][:])
                    expected = workload["repeats"][0]["values"]
                    value_hash = hashlib.sha256(values.tobytes(order="C")).hexdigest()
                    assert list(values.shape) == expected["shape"]
                    assert str(values.dtype) == expected["dtype"]
                    assert value_hash == expected["sha256"], (kind, workload["name"], value_hash, expected["sha256"])
                    assert int(np.isnan(values).sum()) == expected["nan_values"]
                    for axis, expected_coordinates in workload["selected_coordinates"].items():
                        np.testing.assert_array_equal(np.asarray(group[axis][:]), expected_coordinates)
                    attrs = dict(group["temperature_2m"].attrs)
                    assert attrs["units"] == "degree_Celsius"
                    chunks = [row for row in stream if row["type"] == "chunk"]
                    assert sum(row["bytes"] for row in chunks) == values.nbytes
                    assert sum(row["valid_pixels"] for row in chunks) == int(np.isfinite(values).sum())
                    assert all(row["dtype"] == expected["dtype"] for row in chunks)
                    receipt["cases"].append({
                        "product": kind, "workload": workload["name"], "trial": trial,
                        "snapshot_id": source["snapshot_id"],
                        "shape": list(values.shape), "dtype": str(values.dtype),
                        "native_sha256": value_hash, "reference_equal_bits": True,
                        "coordinates_equal": True, "units": attrs["units"],
                        "stream_validated": True, "stream_chunks": len(chunks),
                        "wall_seconds": {"plan": plan_seconds, "stream": stream_seconds, "fetch": fetch_seconds},
                        "stream_observed": stream[-1]["observed"],
                        "logs_directory": str(directory.relative_to(output)),
                    })
                    print(f"{kind} {workload['name']} trial {trial}: exact values and coordinates", flush=True)
        receipt["status"] = "passed"
    except Exception as exc:
        receipt["status"] = "failed"
        receipt["error"] = f"{type(exc).__name__}: {exc}"
        raise
    finally:
        (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    return receipt


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/tile-ripper")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--products", nargs="+", choices=["materialized", "virtual"], default=["materialized", "virtual"])
    parser.add_argument("--workloads", nargs="+", choices=["regional_one_time", "regional_24_hours", "regional_map_101"], default=["regional_one_time", "regional_24_hours", "regional_map_101"])
    parser.add_argument("--repeats", type=int, default=1)
    arguments = parser.parse_args()
    if not 1 <= arguments.repeats <= 3:
        parser.error("--repeats must be between 1 and 3")
    run(arguments)
