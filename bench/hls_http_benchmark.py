"""Fresh-process HTTP-concurrency controls around the unchanged trial sampler.

Use the original runner for the ordinary-default comparison. This separately
named control caps GDAL HTTP/1.1 connections at one per worker, or selects a
preserved native binary for contemporaneous old/new controls. Scientific inputs,
science selection, encoder, validation and durable output are unchanged. The
`sixteen` mode changes native worker/per-host limits to sixteen and retains
four Python asset workers with four connections each. Trace actual concurrency
separately before using its timings.
`parallel16` gives Python sixteen cross-scene acquisition workers and one GDAL
connection each, while retaining the same native sixteen-worker setting.
"""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path
import subprocess

import run_hls_river_30day as runner
from hls_river_30day import ROOT, save


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument(
        "--mode",
        choices=["default", "serial", "capped", "sixteen", "parallel16"],
        default="capped",
    )
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--trials", required=True)
    parser.add_argument("--receipt", default="controlled-measurements.json")
    args = parser.parse_args()
    args.work = args.work.resolve()
    original_popen = subprocess.Popen

    def controlled_popen(command, *pargs, **kwargs):
        # Dispatch the same recipe through a small control wrapper. No input,
        # source checks, QA, analysis or encoder setting is altered.
        if len(command) > 3 and command[2] == str(ROOT / "bench/hls_river_30day.py"):
            command = [
                *command[:2],
                str(ROOT / "bench/hls_http_worker.py"),
                *command[4:],
                "--mode",
                args.mode,
            ]
            if args.binary:
                command.extend(["--binary", str(args.binary.resolve())])
        return original_popen(command, *pargs, **kwargs)

    subprocess.Popen = controlled_popen
    rows = []
    try:
        for spec in args.trials.split(","):
            route, name = spec.split(":")
            row = runner.trial(route, name, args.work)
            row["http_control"] = {
                "mode": args.mode,
                "gdal_http_multirange": "SERIAL"
                if args.mode == "serial"
                else "YES/default",
                "gdal_http_version": "1.1"
                if args.mode in {"capped", "sixteen", "parallel16"}
                else "default",
                "gdal_http_max_total_connections_per_worker": (
                    4 if args.mode == "sixteen" else 1
                )
                if args.mode in {"capped", "sixteen", "parallel16"}
                else "default",
                "gdal_http_multiplex": "NO"
                if args.mode in {"capped", "sixteen", "parallel16"}
                else "default",
                "acquisition_workers": 16
                if args.mode == "parallel16"
                or (args.mode == "sixteen" and route == "tile")
                else 4,
                "native_per_host": 16 if args.mode in {"sixteen", "parallel16"} else 4,
                "source_http_limit": 16
                if args.mode in {"sixteen", "parallel16"}
                else (4 if args.mode == "capped" else None),
                "native_binary": str(args.binary.resolve())
                if args.binary
                else str(ROOT / "target/release/tile-ripper"),
            }
            for path in [
                ROOT / "bench/hls_http_worker.py",
                ROOT / "bench/hls_scene_pool.py",
                Path(__file__).resolve(),
            ]:
                row["input_and_code_sha256"][str(path.relative_to(ROOT))] = (
                    hashlib.sha256(path.read_bytes()).hexdigest()
                )
            if args.binary:
                row["selected_binary_sha256"] = hashlib.sha256(
                    args.binary.read_bytes()
                ).hexdigest()
            save(args.work / f"{name}-measurement.json", row)
            rows.append(row)
            if row["returncode"] != 0:
                raise RuntimeError(f"control {name} failed; see sanitized log")
    finally:
        subprocess.Popen = original_popen
        save(args.work / args.receipt, rows)


if __name__ == "__main__":
    main()
