"""Equivalent native Zarr materialization and syscall profiling (macOS).
Run with prototype-output/zarr-env/bin/python; see output-write-decision.md.
"""

import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import statistics
import subprocess
import sys
import time
import tempfile

import unified_workflow as unified
from zarr_workflow import check, expected, fixture

ROOT = unified.ROOT
WORK = ROOT / "prototype-output/write-profile"


def worker(request, output, mode):
    import xarray as xr
    import zarr
    import numpy as np

    start = time.perf_counter()
    source = xr.open_zarr(
        request["source"]["assets"][0]["locator"],
        consolidated=False,
        decode_cf=False,
        chunks=None,
    )
    metadata = time.perf_counter() - start
    start = time.perf_counter()
    subset = expected(request, source)
    planning = time.perf_counter() - start
    start = time.perf_counter()
    subset.load()
    reading = time.perf_counter() - start
    subset.attrs = copy.deepcopy(subset.attrs)
    affine = subset.attrs["tile_ripper_grid"]["transform"]
    affine[0] = float(subset.x.values[0] - 5)
    affine[3] = float(subset.y.values[0] + 5)
    edge = request["budget"]["chunk_edge"]
    output.mkdir()

    class DurableStore(zarr.DirectoryStore):
        # Match zarrs_filesystem: each successful object write fsyncs its file.
        def __setitem__(self, key, value):
            super().__setitem__(key, value)
            with open(Path(self.path) / key, "rb") as handle:
                __import__("fcntl").fcntl(handle.fileno(), 51)  # Darwin F_FULLFSYNC

    store = DurableStore(str(output / "cube.zarr"))
    start = time.perf_counter()
    if mode == "xarray":
        source_group = zarr.open_group(
            request["source"]["assets"][0]["locator"], mode="r"
        )
        encoding = {
            n: {
                "_FillValue": None,
                "compressor": source_group[n].compressor,
                "chunks": (subset[n].size,),
            }
            for n in subset.coords
        }
        encoding.update(
            {
                n: {
                    "compressor": source_group[n].compressor,
                    "chunks": (
                        1,
                        min(edge, subset.sizes["y"]),
                        min(edge, subset.sizes["x"]),
                    ),
                }
                for n in subset.data_vars
            }
        )
        subset.to_zarr(
            store,
            mode="w",
            consolidated=False,
            encoding=encoding,
            write_empty_chunks=False,
        )
    else:
        group = zarr.group(store=store, overwrite=True)
        group.attrs.update(subset.attrs)
        source_group = zarr.open_group(
            request["source"]["assets"][0]["locator"], mode="r"
        )
        for n in subset.variables:
            original = source_group[n]
            values = np.asarray(subset[n].values)
            chunks = (
                (1, min(edge, subset.sizes["y"]), min(edge, subset.sizes["x"]))
                if n in subset.data_vars
                else (values.size,)
            )
            ar = group.create_dataset(
                n,
                shape=values.shape,
                dtype=values.dtype,
                chunks=chunks,
                compressor=original.compressor,
                fill_value=original.fill_value,
                dimension_separator=original._dimension_separator,
                write_empty_chunks=False,
            )
            ar.attrs.update(dict(original.attrs))
            ar[:] = values
    writing = time.perf_counter() - start
    # Equal manifest payload/serialization work; source references are diagnostic.
    template = WORK / "manifest-template.json"
    if template.exists():
        start = time.perf_counter()
        (output / "manifest.json").write_text(
            json.dumps(json.loads(template.read_text()), indent=2)
        )
        manifest = time.perf_counter() - start
    else:
        manifest = 0
    return dict(
        metadata_seconds=metadata,
        planning_seconds=planning,
        read_decode_seconds=reading,
        sink_seconds=writing,
        manifest_seconds=manifest,
    )


def process(cmd, request, output, label, cold):
    trace = WORK / "syscalls.json"
    stages = WORK / "stages.jsonl"
    stages.unlink(missing_ok=True)
    env = {
        **os.environ,
        "RAYON_NUM_THREADS": "1",
        "DYLD_INSERT_LIBRARIES": str(WORK / "io.dylib"),
        "TR_IO_OUTPUT": str(output),
        "TR_IO_SOURCE": str(Path(request["source"]["assets"][0]["locator"]).resolve()),
        "TR_IO_REPORT": str(trace),
        "TILE_RIPPER_STAGE_TRACE": str(stages),
        "TR_IO_NOCACHE": "1" if cold else "0",
    }
    start = time.perf_counter()
    with (
        tempfile.TemporaryFile() as stdout_file,
        tempfile.TemporaryFile() as stderr_file,
    ):
        p = subprocess.Popen(
            cmd, cwd=ROOT, stdout=stdout_file, stderr=stderr_file, env=env
        )
        _, status, usage = os.wait4(p.pid, 0)
        p.returncode = os.waitstatus_to_exitcode(status)
        stdout_file.seek(0)
        stderr_file.seek(0)
        stdout, stderr = stdout_file.read(), stderr_file.read()
    if p.returncode:
        raise RuntimeError(stderr.decode()[:4000])
    result = json.loads(stdout)
    files = [f for f in output.rglob("*") if f.is_file()]
    return dict(
        reader=label,
        cache="bypass-source-cache" if cold else "warm",
        wall_seconds=time.perf_counter() - start,
        peak_rss_bytes=usage.ru_maxrss,
        report=result.get("observed", result),
        stages=[json.loads(v) for v in stages.read_text().splitlines()]
        if stages.exists()
        else [],
        syscalls=json.loads(Path(str(trace) + "." + str(p.pid)).read_text()),
        file_count=len(files),
        output_bytes=sum(f.stat().st_size for f in files),
    )


def run(reps, phase):
    WORK.mkdir(parents=True, exist_ok=True)
    request = json.loads(fixture().read_text())
    rows = []
    probes = []
    readers = ["b3ceb70", "46dd503", "xarray", "minimal"]
    if phase == "after":
        readers.insert(0, "fixed")
    binaries = {
        "fixed": ROOT / "target/release/tile-ripper",
        "b3ceb70": WORK / "b3ceb70",
        "46dd503": ROOT / "prototype-output/unified/previous-46dd503",
    }
    rng = random.Random(18)
    for size, r in [("small", request), ("medium", unified.medium_fixture(request))]:
        file = WORK / f"request-{size}.json"
        file.write_text(json.dumps(r))
        keys = unified.minimum_keys(r)
        probes.append(dict(workload=size, **unified.zarr_probes(r, keys)))
        # Produce the common manifest once, outside timings, for equal baseline payload.
        template_out = WORK / "template"
        shutil.rmtree(template_out, ignore_errors=True)
        subprocess.run(
            [
                str(binaries["b3ceb70"]),
                "fetch",
                str(file),
                "--output",
                str(template_out),
            ],
            cwd=ROOT,
            check=True,
            stdout=subprocess.DEVNULL,
        )
        shutil.copy(template_out / "manifest.json", WORK / "manifest-template.json")
        for cold in [True, False]:
            for rep in range(reps):
                order = readers.copy()
                rng.shuffle(order)
                for reader in order:
                    out = WORK / f"{phase}-{size}-{reader}-{int(cold)}-{rep}"
                    shutil.rmtree(out, ignore_errors=True)
                    cmd = (
                        [
                            str(binaries[reader]),
                            "fetch",
                            str(file),
                            "--output",
                            str(out),
                        ]
                        if reader in binaries
                        else [
                            sys.executable,
                            __file__,
                            "--worker",
                            reader,
                            "--request",
                            str(file),
                            "--output",
                            str(out),
                        ]
                    )
                    row = process(cmd, r, out, reader, cold)
                    row.update(
                        workload=size, repetition=rep, validated_values=check(r, out)
                    )
                    if reader in ["b3ceb70", "fixed"]:
                        reads = row["report"]["source_reads"]
                        assert sorted(v["key"] for v in reads) == keys
                        assert all(v["requests"] == 1 for v in reads)
                    # Encoding parity is part of the benchmark contract.
                    import zarr

                    actual = zarr.open_group(str(out / "cube.zarr"), mode="r")
                    ref = zarr.open_group(str(template_out / "cube.zarr"), mode="r")
                    for n in ref:
                        assert actual[n].shape == ref[n].shape
                        assert actual[n].chunks == ref[n].chunks
                        assert actual[n].compressor == ref[n].compressor
                        assert actual[n].fill_value == ref[n].fill_value
                    rows.append(row)
                    print(
                        size,
                        reader,
                        cold,
                        round(row["wall_seconds"], 3),
                        file=sys.stderr,
                        flush=True,
                    )
    summary = []
    for size in ["small", "medium"]:
        for cache in ["bypass-source-cache", "warm"]:
            for reader in readers:
                rr = [
                    r
                    for r in rows
                    if (r["workload"], r["cache"], r["reader"]) == (size, cache, reader)
                ]
                times = [r["wall_seconds"] for r in rr]
                summary.append(
                    dict(
                        workload=size,
                        cache=cache,
                        reader=reader,
                        median=statistics.median(times),
                        min=min(times),
                        max=max(times),
                        peak_rss_bytes=max(r["peak_rss_bytes"] for r in rr),
                    )
                )
    evidence = dict(
        phase=phase,
        repetitions=reps,
        rows=rows,
        summary=summary,
        probes=probes,
        source_sha256=hashlib.sha256(
            b"".join(p.read_bytes() for p in sorted((ROOT / "src").glob("*.rs")))
        ).hexdigest(),
        harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        profiler_sha256=hashlib.sha256(
            (ROOT / "bench/io_profile.c").read_bytes()
        ).hexdigest(),
        binaries={
            n: hashlib.sha256(p.read_bytes()).hexdigest() for n, p in binaries.items()
        },
    )
    (ROOT / "docs" / f"output-write-{phase}.json").write_text(
        json.dumps(evidence, indent=2)
    )


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--worker", choices=["xarray", "minimal"])
    ap.add_argument("--request", type=Path)
    ap.add_argument("--output", type=Path)
    ap.add_argument("--phase", choices=["before", "after"], default="before")
    ap.add_argument("--reps", type=int, default=3)
    args = ap.parse_args()
    if args.worker:
        print(
            json.dumps(
                worker(json.loads(args.request.read_text()), args.output, args.worker)
            )
        )
    else:
        run(args.reps, args.phase)
