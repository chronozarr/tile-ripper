"""Serial fresh-process trials with sampled process-tree memory and disk usage."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

import psutil

from hls_river_30day import DEFAULT, ROOT, save


def disk(root):
    logical = allocated = files = 0
    for base, _, names in os.walk(root):
        for name in names:
            try:
                stat = (Path(base) / name).stat()
            except FileNotFoundError:
                continue
            logical += stat.st_size
            allocated += stat.st_blocks * 512
            files += 1
    return logical, allocated, files


def trial(route, name, work):
    out = work / name
    log_path = work / f"{name}.log"
    command = [
        sys.executable,
        "-u",
        str(ROOT / "bench/hls_river_30day.py"),
        "worker",
        "--work",
        str(work),
        "--route",
        route,
        "--out",
        str(out),
    ]
    started = time.perf_counter()
    peak_rss = peak_disk = peak_allocated = 0
    last_disk = 0
    cpu = {}
    samples = 0
    with log_path.open("w") as log:
        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT)
        parent = psutil.Process(process.pid)
        while process.poll() is None:
            now = time.perf_counter()
            try:
                processes = [parent, *parent.children(recursive=True)]
            except psutil.NoSuchProcess:
                processes = []
            rss = 0
            for p in processes:
                try:
                    rss += p.memory_info().rss
                    c = p.cpu_times()
                    cpu[p.pid] = max(cpu.get(p.pid, 0), c.user + c.system)
                except (psutil.NoSuchProcess, psutil.AccessDenied):
                    pass
            peak_rss = max(peak_rss, rss)
            samples += 1
            if now - last_disk >= 1:
                size, allocated, _ = disk(out)
                peak_disk = max(peak_disk, size)
                peak_allocated = max(peak_allocated, allocated)
                last_disk = now
            time.sleep(0.05)
        returncode = process.wait()
    elapsed = time.perf_counter() - started
    # Error messages from dependency libraries sometimes include signed URLs.
    log_path.write_text(
        re.sub(r"\?[^\s\]\)\"<>]+", "?[redacted]", log_path.read_text())
    )
    size, allocated, files = disk(out)
    peak_disk, peak_allocated = max(peak_disk, size), max(peak_allocated, allocated)
    result = {
        "name": name,
        "route": route,
        "returncode": returncode,
        "wall_s": elapsed,
        "sampled_peak_tree_rss_bytes": peak_rss,
        "sampled_cpu_s": sum(cpu.values()),
        "sampled_peak_work_logical_bytes": peak_disk,
        "sampled_peak_work_allocated_bytes": peak_allocated,
        "retained_work_bytes": size,
        "retained_work_allocated_bytes": allocated,
        "retained_files": files,
        "samples": samples,
        "memory_sampling_s": 0.05,
        "disk_sampling_s": 1.0,
        "output": str(out),
    }
    result["input_and_code_sha256"] = {
        str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in [
            ROOT / "bench/hls_river_30day.py",
            ROOT / "bench/run_hls_river_30day.py",
            work / "protocol.json",
            work / "request.json",
            ROOT / "target/release/tile-ripper",
        ]
    }
    conventions = work / "run-conventions.json"
    if conventions.exists():
        result["input_and_code_sha256"][str(conventions.relative_to(ROOT))] = (
            hashlib.sha256(conventions.read_bytes()).hexdigest()
        )
    result["playback_bytes"] = disk(out / "playback")[0]
    result["raw_observations_bytes"] = disk(out / "observations.zarr")[0]
    if (out / "stages.json").exists():
        result["stages"] = json.loads((out / "stages.json").read_text())
    save(work / f"{name}-measurement.json", result)
    print(json.dumps(result), flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path, default=DEFAULT)
    parser.add_argument(
        "--trials",
        default="python:final-python-1,tile:final-tile-1,tile:final-tile-2,python:final-python-2,python:final-python-3,tile:final-tile-3",
    )
    args = parser.parse_args()
    results = []
    for spec in args.trials.split(","):
        route, name = spec.split(":")
        results.append(trial(route, name, args.work))
    save(args.work / "measurements.json", results)


if __name__ == "__main__":
    main()
