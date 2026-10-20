"""Frozen-source, fresh-process memory ablations; macOS probe is diagnostic only.

Uses the independent native consumer and verifies every scientific result. The
small optional preload observer samples process memory, libc live allocations
and GDAL's public block-cache counters. Its diagnostic timings are not used for
performance ranking. This is a benchmark, not an RSS enforcement mechanism.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import threading
import time

import real_multiscene as m


PROBE = r"""
#include <dlfcn.h>
#include <mach/mach.h>
#include <malloc/malloc.h>
#include <pthread.h>
#include <stdio.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>
static atomic_int done;
static pthread_t thread;
static int started;
static FILE *out;
static double origin;
static double now(void) {
  struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t);
  return t.tv_sec + t.tv_nsec / 1e9;
}
static void *observe(void *unused) {
  long long (*used)(void) = dlsym(RTLD_DEFAULT, "GDALGetCacheUsed64");
  long long (*cap)(void) = dlsym(RTLD_DEFAULT, "GDALGetCacheMax64");
  while (!atomic_load(&done)) {
    task_vm_info_data_t vm = {0};
    mach_msg_type_number_t n = TASK_VM_INFO_COUNT;
    kern_return_t ok = task_info(mach_task_self(), TASK_VM_INFO, (task_info_t)&vm, &n);
    malloc_statistics_t heap = {0};
    malloc_zone_statistics(NULL, &heap);
    fprintf(out, "{\"seconds\":%.6f,\"rss_bytes\":%llu,\"physical_footprint_bytes\":%llu,\"malloc_live_bytes\":%llu,\"malloc_reserved_bytes\":%llu,\"gdal_cache_used_bytes\":%lld,\"gdal_cache_max_bytes\":%lld,\"task_info_ok\":%d}\n",
      now()-origin, vm.resident_size, vm.phys_footprint,
      (unsigned long long)heap.size_in_use,
      (unsigned long long)heap.size_allocated,
      used ? used() : -1, cap ? cap() : -1, ok == KERN_SUCCESS);
    fflush(out);
    usleep(10000);
  }
  return NULL;
}
__attribute__((constructor)) static void begin(void) {
  const char *path = getenv("TILE_RIPPER_MEMORY_PROBE");
  if (!path || !(out=fopen(path, "w"))) return;
  origin=now();
  started = pthread_create(&thread, NULL, observe, NULL) == 0;
}
__attribute__((destructor)) static void end(void) {
  if (!started) return;
  atomic_store(&done,1); pthread_join(thread,NULL); fclose(out);
}
"""


def build_probe(folder):
    assert sys.platform == "darwin", "memory counter probe currently targets macOS"
    source = folder / "memory_probe.c"
    dylib = folder / "memory_probe.dylib"
    source.write_text(PROBE)
    subprocess.run(
        ["clang", "-O2", "-dynamiclib", "-pthread", str(source), "-o", str(dylib)],
        check=True,
    )
    return dylib


def snapshot(req, folder, server, refs, workers, after):
    """Pause consumer delivery; attribute live heap by allocation call stacks.

    Full MallocStackLogging and stopping for inspectors substantially perturb
    memory/timing. These snapshots establish allocation ownership, not gates.
    """
    from consumer_validation import verify

    path = folder / "request.json"
    path.write_text(json.dumps(req))
    server.reset()
    env = {**os.environ, **m.GDAL_ENV, "MallocStackLogging": "1"}
    rows = []
    inspections = []
    with tempfile.TemporaryFile() as errors:
        started = time.perf_counter()
        proc = subprocess.Popen(
            [str(m.CONSUMER), str(path), "--pause-after", str(after)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=errors,
            text=True,
            env=env,
        )
        timer = threading.Timer(180, proc.kill)
        timer.start()
        chunks = 0
        for line in proc.stdout:
            row = json.loads(line)
            rows.append(row)
            if row["type"] == "provisional_chunk":
                chunks += 1
                if chunks == after:
                    for name, cmd in [
                        ("vmmap", ["vmmap", "-summary", str(proc.pid)]),
                        ("heap", ["heap", "-s", "--noContent", str(proc.pid)]),
                        (
                            "malloc_history",
                            [
                                "malloc_history",
                                str(proc.pid),
                                "-callTree",
                                "-invert",
                                "-ignoreThreads",
                                "-consolidateAllBySymbol",
                                "-noContent",
                            ],
                        ),
                    ]:
                        try:
                            inspected = subprocess.run(
                                cmd,
                                text=True,
                                stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT,
                                timeout=30,
                            )
                            (folder / f"{name}.txt").write_text(inspected.stdout)
                            inspections.append((name, inspected.returncode))
                        except (OSError, subprocess.TimeoutExpired) as error:
                            (folder / f"{name}.txt").write_text(str(error))
                            inspections.append((name, -1))
                    proc.stdin.write("continue\n")
                    proc.stdin.flush()
        _, status, usage = os.wait4(proc.pid, 0)
        timer.cancel()
        proc.returncode = os.waitstatus_to_exitcode(status)
        errors.seek(0)
        err = errors.read().decode(errors="replace")
    (folder / "stdout.json").write_text("".join(json.dumps(r) + "\n" for r in rows))
    (folder / "stderr.txt").write_text(err)
    assert proc.returncode == 0, err
    assert len(inspections) == 3 and all(code == 0 for _, code in inspections), (
        inspections
    )
    checked = verify(rows, refs)
    return dict(
        workers=workers,
        exit_code=proc.returncode,
        scientific_correctness_passed=True,
        correctness={k: v for k, v in checked.items() if k != "coverage"},
        peak_rss_bytes=usage.ru_maxrss,
        wall_seconds=time.perf_counter() - started,
        snapshot_only=True,
        resource_gate_applied=False,
        http=server.counts.copy(),
        observed=rows[-1]["observed"],
    )


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--label", required=True)
    ap.add_argument(
        "--consumer",
        type=Path,
        default=m.ROOT
        / "prototype-output/cog-efficiency/baseline-binaries/tile-ripper-native-stats",
    )
    ap.add_argument("--workers", type=int, nargs="+", default=[2, 4])
    ap.add_argument("--cache-mib", type=int, nargs="+", default=[1, 32, 64])
    ap.add_argument("--buffer-mib", type=int, default=16)
    ap.add_argument("--latency-ms", type=float, default=0)
    ap.add_argument("--bandwidth-mbps", type=float, default=0)
    ap.add_argument("--reps", type=int, default=2)
    observation = ap.add_mutually_exclusive_group()
    observation.add_argument("--probe", action="store_true")
    observation.add_argument("--snapshot-after-chunk", type=int)
    ap.add_argument("--workloads", nargs="+", default=["overlap-a", "several-large"])
    args = ap.parse_args()
    if args.snapshot_after_chunk and sys.platform != "darwin":
        ap.error("allocation-stack snapshots require macOS")
    folder = m.ROOT / "prototype-output/cog-efficiency" / args.label
    folder.mkdir(parents=True, exist_ok=False)
    m.CONSUMER = args.consumer.resolve()
    sources = json.loads((m.WORK / "sources.json").read_text())["sources"]
    mirror = m.Mirror(m.WORK / "mirror", sources)
    digest = mirror.verify_pages()
    refs = {
        a["id"]: {**a, "format": "cog", "local": mirror.path(f"/{i}.tif")}
        for i, a in enumerate(sources)
    }
    windows = {
        "many-small": (7300, 2100, 96, 96),
        "overlap-a": (6868, 1748, 768, 768),
        "overlap-b": (7124, 2004, 768, 768),
        "several-large": (6144, 1024, 2048, 2048),
    }
    dylib = build_probe(folder) if args.probe else None
    original_run = m.run
    if dylib:

        def observed_run(cmd, out, env=None, ndjson=False):
            return original_run(
                cmd,
                out,
                env={
                    **(env or {}),
                    "DYLD_INSERT_LIBRARIES": str(dylib),
                    "TILE_RIPPER_MEMORY_PROBE": str(out.parent / "memory.jsonl"),
                },
                ndjson=ndjson,
            )

        m.run = observed_run
    rows = []
    server = m.start(mirror, args.latency_ms, args.bandwidth_mbps)
    try:
        for rep in range(args.reps):
            for workload in args.workloads:
                for cache in args.cache_mib:
                    for workers in args.workers:
                        runfolder = folder / f"{workload}-w{workers}-cache{cache}-{rep}"
                        runfolder.mkdir()
                        os.environ["TILE_RIPPER_WORKERS"] = str(workers)
                        m.GDAL_ENV["GDAL_CACHEMAX"] = str(cache)
                        req = m.request(
                            sources if workload == "many-small" else sources[:6],
                            server,
                            windows[workload],
                        )
                        req["budget"]["buffer_bytes"] = args.buffer_mib * 2**20
                        row = (
                            snapshot(
                                req,
                                runfolder,
                                server,
                                refs,
                                workers,
                                args.snapshot_after_chunk,
                            )
                            if args.snapshot_after_chunk
                            else m.measure(
                                "rust-stream", req, runfolder, server, refs, workers
                            )
                        )
                        row.update(
                            workload=workload,
                            rep=rep,
                            gdal_cache_mib=cache,
                            buffer_mib=args.buffer_mib,
                            instrumented=bool(dylib),
                            latency_ms=args.latency_ms,
                            bandwidth_mbps=args.bandwidth_mbps,
                        )
                        if dylib:
                            samples = [
                                json.loads(line)
                                for line in (runfolder / "memory.jsonl")
                                .read_text()
                                .splitlines()
                            ]
                            assert samples and all(s["task_info_ok"] for s in samples)
                            assert all(s["gdal_cache_used_bytes"] >= 0 for s in samples)
                            row["sampled_peaks"] = {
                                key: max(s[key] for s in samples)
                                for key in samples[0]
                                if key not in ("seconds", "task_info_ok")
                            }
                            row["peak_rss_sample"] = max(
                                samples, key=lambda s: s["rss_bytes"]
                            )
                            first_cached = next(
                                (
                                    i
                                    for i, s in enumerate(samples)
                                    if s["gdal_cache_used_bytes"] > 0
                                ),
                                len(samples),
                            )
                            before_cache = samples[:first_cached]
                            row["before_first_cached_block_rss_bytes"] = max(
                                (s["rss_bytes"] for s in before_cache), default=None
                            )
                            row["memory_samples"] = len(samples)
                        rows.append(row)
                        with (folder / "runs.jsonl").open("a") as out:
                            out.write(json.dumps(row) + "\n")
                        print(
                            workload,
                            workers,
                            cache,
                            rep,
                            "snapshot"
                            if row.get("snapshot_only")
                            else "pass"
                            if row["passed"]
                            else "RSS FAIL",
                            round(row["peak_rss_bytes"] / 2**20, 2),
                            flush=True,
                        )
    finally:
        server.shutdown()
        server.server_close()
        os.environ.pop("TILE_RIPPER_WORKERS", None)
        m.run = original_run
    report = dict(
        rows=rows,
        binary=str(m.CONSUMER.relative_to(m.ROOT)),
        binary_sha256=hashlib.sha256(m.CONSUMER.read_bytes()).hexdigest(),
        mirror_index_sha256=digest,
        platform=platform.platform(),
        rss_limit_bytes=192 * 2**20,
        observer="10-ms macOS task_info, malloc_zone_statistics, GDAL public cache counters"
        if dylib
        else None,
        timing_scope="memory diagnostic; instrumented runs excluded from timing rankings",
        source_bytes="frozen original Sentinel-2 COG ranges; no upstream access",
    )
    (folder / "report.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
