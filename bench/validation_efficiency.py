"""Repeat the identical public WeatherBench workload with two local binaries.
The baseline is the same independent xarray/Numcodecs writer in both batches.
"""

import argparse
import os
from pathlib import Path
import shutil
import cf_zarr as core


def controlled(before_library):
    import json
    import zarr
    import xarray as xr
    import remote_zarr as h

    work = core.ROOT / "prototype-output/validation-controlled"
    work.mkdir(parents=True, exist_ok=True)
    source = work / "source.zarr"
    core.fixture(source)
    zarr.consolidate_metadata(str(source))
    r = core.request(source, work / "unused")
    dataset = xr.open_zarr(
        str(source), consolidated=False, decode_cf=False, chunks=None
    )
    reference, _ = core.selection(r, dataset)
    reference = reference.transpose("time", "latitude", "longitude").load()
    server = h.start(source, latency=0.01, connections=1)
    r["source"]["assets"][0]["locator"] = server.url
    rows = []
    try:
        for rep in range(3):
            for label in ["before", "after"] if rep % 2 == 0 else ["after", "before"]:
                r["sink"]["path"] = str(work / f"{label}-{rep}")
                for i in range(2):
                    shutil.rmtree(Path(r["sink"]["path"] + f"-{i}"), ignore_errors=True)
                req = work / "request.json"
                req.write_text(json.dumps(r))
                server.records.clear()
                binary = before_library.resolve() if label == "before" else h.EXAMPLE
                result = h.process([str(binary), str(req), "warm"])
                assert result["code"] == 0, result
                data = result.pop("result")
                for run in data["runs"]:
                    actual = xr.open_zarr(
                        str(Path(run["output"]) / "era5.zarr"),
                        consolidated=False,
                        decode_cf=False,
                        chunks=None,
                    ).load()
                    xr.testing.assert_identical(reference, actual)
                result.update(
                    data, label=label, repetition=rep, requests=list(server.records)
                )
                rows.append(result)
        (core.ROOT / "docs/validation-efficiency-controlled.json").write_text(
            json.dumps(
                dict(
                    latency_seconds=0.01,
                    server_connections=1,
                    repetitions=3,
                    runs=rows,
                    scope="same synthetic source, exact xarray values and native output semantics; interleaved binaries; no public-network inference",
                ),
                indent=2,
            )
            + "\n"
        )
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--before", type=Path, required=True)
    ap.add_argument("--before-library", type=Path, required=True)
    ap.add_argument("--controlled-only", action="store_true")
    args = ap.parse_args()
    if args.controlled_only:
        controlled(args.before_library)
        raise SystemExit(0)
    historical = core.ROOT / "docs/cf-zarr-benchmark.json"
    original = historical.read_bytes()
    current, example = core.h.CURRENT, core.h.EXAMPLE
    try:
        for label, cli, library in [
            ("before", args.before, args.before_library),
            ("weatherbench", current, example),
        ]:
            work = core.ROOT / "prototype-output" / ("validation-" + label)
            work.mkdir(parents=True, exist_ok=True)
            os.environ["TILE_RIPPER_BENCH_WORK"] = str(work)
            core.WORK = work
            core.h.CURRENT = cli.resolve()
            core.h.EXAMPLE = library.resolve()
            core.benchmark()
            shutil.copy(
                historical,
                core.ROOT / "docs" / ("validation-efficiency-" + label + ".json"),
            )
    finally:
        historical.write_bytes(original)
        os.environ.pop("TILE_RIPPER_BENCH_WORK", None)
