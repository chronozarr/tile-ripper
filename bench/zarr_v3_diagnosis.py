"""Reproduce the upstream gzip/shard bound defect without changing production.

Stdlib-only by default. --frozen also uses the retained Sentinel mirror and the
existing rasterio benchmark environment. Writes receipts to prototype-output.
"""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
REPRO = ROOT / "bench/zarr-v3-repro"
WORK = ROOT / "prototype-output/zarr-v3-diagnosis"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def preserved():
    docs = ROOT / "docs/durable-output"
    verification = json.loads((docs / "verification.json").read_text())
    files = sorted([ROOT / "Cargo.toml", ROOT / "Cargo.lock",
                    *ROOT.glob("src/**/*.rs"),
                    *ROOT.glob("vendor/zarrs_filesystem/src/*.rs")])
    digest = hashlib.sha256()
    current_digest = hashlib.sha256()
    for path in files:
        current = path.read_bytes()
        baseline = current
        if path == ROOT / "src/scientific_output.rs":
            baseline = subprocess.check_output(["git", "show", "0d0cc18:src/scientific_output.rs"], cwd=ROOT)
            # This audit corrects a comment about Rust's macOS implementation.
            # Accept exactly that correction, with no runtime source change.
            before = b"// Unlike Rust's sync_all, do not silently fall back to fsync if the\n        // device refuses the stronger barrier that this strategy requires."
            after = b"// Require the explicit stronger macOS request and propagate failures;\n        // POSIX fsync alone does not satisfy this transaction barrier."
            assert before in baseline and current == baseline.replace(before, after)
        digest.update(str(path.relative_to(ROOT)).encode())
        digest.update(baseline)
        current_digest.update(str(path.relative_to(ROOT)).encode())
        current_digest.update(current)
    assert digest.hexdigest() == verification["production_source_sha256"]
    for path, expected in verification["binaries"].items():
        assert sha(Path(path)) == expected
    for receipt in verification["independent_receipts"]:
        assert json.loads((docs / receipt).read_text())["passed"]
    benchmark = json.loads((docs / "benchmark.json").read_text())
    assert benchmark["completed"] and benchmark["stable_modes_passed"]
    assert len(benchmark["runs"]) == 144
    failures = [r for r in benchmark["runs"] if not r["passed"]]
    assert len(failures) == 12
    assert all(not r["published"] and r["staging_cleaned"] for r in failures)
    assert all('Sharding did not allocate a large enough buffer' in r["process"]["stderr"] for r in failures)
    medians = []
    for latency in [0, 8]:
        rows = [r for r in benchmark["runs"] if r["workload"] == "large" and r["link"]["latency_ms"] == latency]
        times = {mode: statistics.median(r["process"]["wall_seconds"] for r in rows if r["mode"] == mode) for mode in ["old", "raw"]}
        medians.append(dict(latency_ms=latency, **times, speedup=times["old"]/times["raw"]))
    calls = []
    for mode in ["old", "raw"]:
        rows = [r for r in benchmark["runs"] if r["workload"] == "large" and r["link"]["latency_ms"] == 0 and r["mode"] == mode]
        for r in rows:
            groups = [r["filesystem"][k] for k in ["output", "manifest", "directories"]]
            assert sum(g["errors"] for g in groups) == 0
            calls.append(dict(mode=mode, rep=r["rep"], fsync=sum(g["fsyncs"] for g in groups),
                              f_fullfsync=sum(g["fullsyncs"] for g in groups)))
    return dict(commit="0d0cc1802dd14c781c30e2551e8429a0ed7a92ec",
                baseline_production_source_sha256=digest.hexdigest(),
                audited_production_source_sha256=current_digest.hexdigest(),
                source_change="Only the checked two-line comment correction in device_barrier; runtime source, root manifests and lockfile otherwise identical",
                binaries=verification["binaries"],
                independent_receipts_checked=verification["independent_receipts"],
                benchmark_rows=144, experimental_failures=12, medians=medians,
                synchronization_calls=calls,
                native_flate2_features=subprocess.check_output(
                    ["cargo", "tree", "--locked", "--no-default-features", "--features", "native-cog,experimental-output", "-e", "features", "-i", "flate2"],
                    cwd=ROOT, text=True).replace(str(ROOT), "$REPO"),
                hardware_observation="Interposed syscall requests and return values only; no device telemetry or power-cut experiment")


def frozen_inputs():
    # Import only for the optional local-data replay.
    import rasterio
    from rasterio.windows import Window
    import real_multiscene as real
    sources = json.loads((real.WORK / "sources.json").read_text())["sources"]
    mirror = real.Mirror(real.WORK / "mirror", sources)
    mirror_digest = mirror.verify_pages()
    inputs = []
    for i, source in enumerate(sources[:4]):
        with rasterio.open(mirror.path(f"/{i}.tif")) as src:
            data = src.read(1, window=Window(6144, 1024, 512, 512))
        path = WORK / f"sentinel-{i}.u16"
        path.write_bytes(data.astype("<u2").tobytes())
        inputs.append(dict(file=path.name, asset_id=source["id"], source_etag=source["etag"],
                           window=[6144, 1024, 512, 512], sha256=sha(path)))
    rows = []
    server = real.start(mirror)
    try:
        for workload, edge in [("medium", 512), ("large", 2048)]:
            for mib in [8, 16, 64]:
                name = f"{workload}-budget-{mib}"
                destination = WORK / name
                assert not destination.exists()
                request = real.request(sources[:4], server, (6144, 1024, edge, edge))
                request["sink"] = dict(kind="zarr_directory", path=str(destination))
                request["budget"]["buffer_bytes"] = mib * 2**20
                path = WORK / (name + ".json")
                save(path, request)
                result = subprocess.run([ROOT / "target/scientific-output/release/examples/durable_output_candidate", path, "sharded", "256"],
                                        capture_output=True, text=True, timeout=45, cwd=ROOT)
                assert result.returncode == 1 and not destination.exists()
                assert not list(WORK.glob("." + name + ".partial-*"))
                assert ("budget" if mib == 8 else "Sharding did not allocate a large enough buffer") in result.stderr
                rows.append(dict(workload=workload, budget_bytes=mib*2**20, returncode=result.returncode,
                                 stderr=result.stderr, published=False, staging_cleaned=True))
    finally:
        server.shutdown()
        server.server_close()
    return dict(mirror_sha256=mirror_digest, inputs=inputs, budget_replay=rows)


def standalone(frozen):
    results = []
    for fixed in [False, True]:
        manifest = REPRO / ("fixed/Cargo.toml" if fixed else "Cargo.toml")
        target = ROOT / ("target/zarr-v3-repro-fixed" if fixed else "target/zarr-v3-repro")
        lock = tomllib.loads(manifest.with_name("Cargo.lock").read_text())
        names = {"zarrs", "flate2", "zlib-rs", "miniz_oxide", "rayon", "zarrs_storage",
                 "zarrs_metadata", "zarrs_metadata_ext", "zarrs_data_type", "zarrs_codec", "serde_json"}
        deps = {p["name"]: dict(version=p["version"], checksum=p["checksum"]) for p in lock["package"] if p["name"] in names}
        for native in [False, True]:
            name = ("fixed" if fixed else "pinned") + ("-zlib" if native else "-miniz")
            features = (["upstream-fixed"] if fixed else []) + (["native-backend"] if native else [])
            cmd = ["cargo", "run", "--locked", "--manifest-path", str(manifest), "--target-dir", str(target), "--no-default-features"]
            if features:
                cmd += ["--features", ",".join(features)]
            minimal = subprocess.run(cmd + ["--bin", "zarr-v3-repro"], capture_output=True, text=True, cwd=ROOT, timeout=120)
            assert minimal.returncode == (0 if fixed else 1), minimal.stderr
            probes = [json.loads(line) for line in minimal.stdout.splitlines()]
            assert len(probes) == 2
            matrix = subprocess.run(cmd + ["--bin", "matrix", "--"] + ([str(WORK)] if frozen else []), capture_output=True, text=True, cwd=ROOT, timeout=120)
            (WORK / (name + ".log")).write_text(minimal.stderr + matrix.stderr)
            assert matrix.returncode == 0, matrix.stderr
            full = json.loads(matrix.stdout)
            save(WORK / ("matrix-" + name + ".json"), full)
            rows = full["rows"]
            assert len(rows) == (98 if frozen else 74)
            assert all(r["result"] == "roundtrip_ok" for r in rows if fixed or "gzip" not in str(r["metadata"]["codecs"]))
            compact = []
            for row in rows:
                meta = row.pop("metadata")
                compact.append(dict(shape=meta["shape"], dtype=meta["data_type"],
                                    outer_chunk=meta["chunk_grid"]["configuration"]["chunk_shape"],
                                    codecs=meta["codecs"], **row))
            results.append(dict(profile=name, backend=full["backend"], dependencies=deps,
                                lock_sha256=sha(manifest.with_name("Cargo.lock")),
                                minimal=dict(returncode=minimal.returncode, probes=probes),
                                outcomes=dict(Counter(r["result"] for r in compact)), rows=compact))
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--frozen", action="store_true", help="also replay the local captured Sentinel ranges and application budgets")
    args = parser.parse_args()
    WORK.mkdir(parents=True, exist_ok=True)
    preserved_v2 = preserved()
    frozen = frozen_inputs() if args.frozen else None
    results = standalone(args.frozen)
    artifact_files = sorted([*REPRO.glob("src/**/*.rs"), REPRO / "Cargo.toml", REPRO / "Cargo.lock",
                             REPRO / "fixed/Cargo.toml", REPRO / "fixed/Cargo.lock", Path(__file__).resolve()])
    receipt = dict(passed=True, meaning="The expected pinned failures and fixed-release round trips were reproduced; this is not a production v3 readiness gate",
                   rustc=subprocess.check_output(["rustc", "--version"], text=True).strip(),
                   preserved_v2=preserved_v2, artifacts={str(p.relative_to(ROOT)): sha(p) for p in artifact_files},
                   standalone=results)
    if frozen is not None:
        receipt["frozen"] = frozen
    save(WORK / "results.json", receipt)
    print(json.dumps({"passed": True, "profiles": [{"profile":r["profile"], "outcomes":r["outcomes"]} for r in results]}))


if __name__ == "__main__":
    main()
