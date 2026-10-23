"""Matched authenticated frozen-source inspection benchmarks; no provider access.

Production catalog provenance remains strict. The generation adapter uses the
same native inspection/resolution/scientific calls with catalog assertions
omitted only for these local exact-object HTTPS fixtures. Production CLI
compatibility is separately checked against the actual archive.
"""

import copy
import json
from pathlib import Path
import resource
import subprocess
import tempfile
import time

import psutil

import swot_aoi as aoi
import swot_job as job
from hdf5_selective_mirror import Mirror

ROOT = job.ROOT
WORK = ROOT / "prototype-output/grid-inspection"
DOCS = ROOT / "docs/grid-inspection"
BASELINE = WORK / "baseline-grid-inspection-bench"
OPTIMIZED = ROOT / "target/release/examples/grid_inspection_bench"
MIRRORS = [job.HELD_NATIVE, job.ORIGINAL, aoi.WORK / "additional/native"]
REFERENCES = [job.HELD_REFERENCE, job.ORIGINAL, aoi.WORK / "additional/reference"]


class Fixtures(job.Fixtures):
    def __init__(self):
        super().__init__()
        third = job.Archive(Mirror(MIRRORS[2]))
        self.archives.append(third)
        policies = json.loads(self.policy.read_text())
        policies["sources"].append(json.loads(third.fixture.config_path.read_text()))
        job.write(self.policy, policies)
        self.ca.write_bytes(
            self.ca.read_bytes() + b"\n" + third.fixture.cert.read_bytes()
        )
        self.reset()

    def reset(self, delay=0):
        for archive, mirror in zip(self.archives, MIRRORS):
            archive.reset()
            archive.fixture.etag = Mirror(mirror).info["etag"]

            def wait(handler, role, path, respond, delay=delay):
                time.sleep(delay)
                return False

            archive.callback = wait

    def manifest(self):
        m = json.loads((ROOT / "docs/swot-aoi/generated-job.json").read_text())
        for entry, archive in zip(m["observations"], self.archives):
            entry.pop("catalog", None)
            entry["request"]["source"]["assets"][0]["locator"] = (
                archive.fixture.canonical
            )
        return m


def measure(binary, request, directory, name, fixtures):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started, peak = time.perf_counter(), 0
    import os

    env = os.environ.copy()
    env.update(TILE_RIPPER_HDF5_METADATA_POLICY="page16k")
    out, err = directory / f"{name}.json", directory / f"{name}.stderr"
    with out.open("wb") as stdout, err.open("wb") as stderr:
        p = subprocess.Popen(
            [str(binary), str(request)], env=env, stdout=stdout, stderr=stderr
        )
        while p.poll() is None:
            try:
                peak = max(peak, psutil.Process(p.pid).memory_info().rss)
            except psutil.NoSuchProcess:
                pass
            if time.perf_counter() - started > 180:
                p.kill()
                raise TimeoutError(name)
            time.sleep(0.003)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    assert p.returncode == 0, (name, err.read_text()[-3000:])
    result = json.loads(out.read_text())
    text = out.read_text() + err.read_text()
    assert all(secret not in text for secret in job.SECRETS)
    row = dict(
        name=name,
        wall_seconds=time.perf_counter() - started,
        cpu_seconds=after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime,
        peak_rss_bytes=peak,
        binary_sha256=job.digest(binary),
        output_sha256=job.digest(out),
        wire=fixtures.traffic(),
        generation_seconds=result["generation_seconds"],
        allocations=result["allocations_after"],
        raw_output=str(out),
    )
    job.write(directory / f"{name}-measurements.json", row)
    return result, row


def summarize_generation(result, row, baseline=False):
    sources = []
    for selected, resolution in zip(
        result["review"]["observations"], result["review"]["resolutions"]
    ):
        inspection = selected["inspection"]
        identity = inspection["sources"][0]["object_identities"]
        if baseline:
            phases = resolution["aoi_resolution"]["inspection_transport"]
            transports = [
                phases["shape_probe"],
                phases["full_axes"],
                inspection["transport"],
            ]
            parser_opens = 3
        else:
            transports = [inspection["transport"]]
            parser_opens = identity["inspection_reuse"]["metadata_parser_opens"]
        sources.append(
            dict(
                id=selected["id"],
                metadata_parser_opens=parser_opens,
                geographic_seconds=resolution["aoi_resolution"]["resolution_seconds"],
                transports=transports,
                inspection_reuse=identity.get("inspection_reuse"),
                selected_scientific_plan_seconds=inspection["planning_seconds"],
                working_memory=inspection["aligned_execution"]["working_memory"],
            )
        )
    row["sources"] = sources
    row["source_body_bytes"] = sum(w["resource_get_bytes"] for w in row["wire"])
    row["wire_requests"] = sum(w["requests"] for w in row["wire"])
    return row


def science_digest(path):
    import hashlib
    import zarr

    result = hashlib.sha256()
    root = zarr.open_group(path, mode="r")
    for name in sorted(root["observations"].group_keys()):
        group = root["observations"][name]
        for key in sorted(group.array_keys()):
            array = group[key]
            result.update(
                json.dumps([name, key, array.dtype.str, array.shape]).encode()
            )
            result.update(array[()].tobytes())
    return result.hexdigest()


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    DOCS.mkdir(parents=True, exist_ok=True)
    fixtures = Fixtures()
    directory = Path(tempfile.mkdtemp(prefix="matched-", dir=WORK))
    original_binary = job.BINARY
    try:
        template = fixtures.manifest()
        aois = [e["geographic_selection"]["aoi"] for e in template["observations"]]
        draft = copy.deepcopy(template)
        for entry in draft["observations"]:
            entry.pop("geographic_selection")
            entry["request"]["spatial"].update(start=[0, 0], count=[1, 1], step=[1, 1])
        request = directory / "input.json"
        job.write(
            request,
            dict(
                manifest=draft,
                aois=aois,
                policies=json.loads(fixtures.policy.read_text())["sources"],
                ca=str(fixtures.ca),
                netrc=str(fixtures.netrc),
            ),
        )
        results, expected = [], None
        for delay in [0, 0.025]:
            for repeat in range(2):
                for label, binary in [("baseline", BASELINE), ("optimized", OPTIMIZED)]:
                    fixtures.reset(delay)
                    result, row = measure(
                        binary,
                        request,
                        directory,
                        f"{label}-{delay}-{repeat}",
                        fixtures,
                    )
                    assert expected is None or result["manifest"] == expected, (
                        "native geometry/fingerprints changed"
                    )
                    expected = result["manifest"]
                    row.update(
                        delay_per_wire_request_seconds=delay,
                        repeat=repeat,
                        implementation=label,
                    )
                    results.append(
                        summarize_generation(result, row, label == "baseline")
                    )
                    print(
                        label,
                        delay,
                        repeat,
                        round(row["wall_seconds"], 3),
                        row["wire_requests"],
                        row["source_body_bytes"],
                        flush=True,
                    )
        runs = []
        for label, binary in [
            ("baseline", WORK / "baseline-tile-ripper"),
            ("optimized", original_binary),
        ]:
            job.BINARY = binary
            fixtures.reset()
            plan = job.run(label + "-separate-plan", expected, fixtures, command="plan")
            assert plan["returncode"] == 0, plan["error"]
            fixtures.reset()
            fetched = job.run(label + "-separate-fetch", expected, fixtures)
            assert fetched["returncode"] == 0, fetched["error"]
            runs.append(dict(implementation=label, plan=plan, fetch=fetched))
        job.BINARY = original_binary
        checkpoint = directory / "checkpoint"
        fixtures.reset()
        cpplan = job.run(
            "checkpoint-plan", expected, fixtures, checkpoint, command="plan"
        )
        assert cpplan["returncode"] == 0, cpplan["error"]
        fixtures.reset()
        cpfetch = job.run("checkpoint-fetch", expected, fixtures, checkpoint)
        assert cpfetch["returncode"] == 0, cpfetch["error"]
        fixtures.reset()
        replay = job.run("checkpoint-rebuild", expected, fixtures, checkpoint)
        assert replay["returncode"] == 0, replay["error"]
        coords = [
            aoi.axes(path, "grid-" + str(i))[0] for i, path in enumerate(REFERENCES)
        ]
        for row in [runs[0]["fetch"], runs[1]["fetch"], cpfetch, replay]:
            row["geographic_science"] = aoi.verify_aoi(
                row, expected, coords, sources=REFERENCES
            )
        expected_science = science_digest(runs[0]["fetch"]["output"])
        latency_workflow = []
        for label, binary in [
            ("baseline", WORK / "baseline-tile-ripper"),
            ("optimized", original_binary),
        ]:
            job.BINARY = binary
            fixtures.reset(0.025)
            plan = job.run(label + "-latency-plan", expected, fixtures, command="plan")
            assert plan["returncode"] == 0, plan["error"]
            fixtures.reset(0.025)
            fetched = job.run(label + "-latency-fetch", expected, fixtures)
            assert fetched["returncode"] == 0, fetched["error"]
            assert science_digest(fetched["output"]) == expected_science
            latency_workflow.append(
                dict(
                    implementation=label,
                    delay_per_wire_request_seconds=0.025,
                    plan=job.compact(plan),
                    fetch=job.compact(fetched),
                    independently_verified_science_digest=expected_science,
                )
            )
        job.BINARY = original_binary
        job.write(
            DOCS / "benchmarks.json",
            dict(
                baseline_commit="afe41c2",
                timing_environment="final matched run after other verification processes completed",
                latency_workflow=latency_workflow,
                generation=results,
                separate_process_runs=[
                    dict(
                        implementation=r["implementation"],
                        plan=job.compact(r["plan"]),
                        fetch=job.compact(r["fetch"]),
                        geographic_science=r["fetch"]["geographic_science"],
                    )
                    for r in runs
                ],
                checkpoint=dict(
                    plan=job.compact(cpplan),
                    fetch=job.compact(cpfetch),
                    reconstruction=job.compact(replay),
                    geographic_science=replay["geographic_science"],
                ),
                raw_directory=str(directory),
                public_provider_requests=0,
                all_passed=True,
            ),
        )
        print(
            "matched three-source HTTPS generation, independent science and checkpoint reconstruction passed",
            flush=True,
        )
    finally:
        job.BINARY = original_binary
        fixtures.close()


def faults():
    """Online version changes include actual coordinate bytes at one locator."""
    import h5py
    import shutil
    import os
    import earthdata_auth as auth

    directory = Path(tempfile.mkdtemp(prefix="faults-", dir=WORK))
    new = directory / "updated.h5"
    shutil.copyfile(ROOT / "tests/data/swot/grid-inspection.h5", new)
    with h5py.File(new, "r+") as f:
        f["x"][:] = f["x"][:] + 100.0

    def hook(handler, role, path, respond):
        if role == "control" and path.endswith("/coordinates-version"):
            fixture.bytes = new.read_bytes()
            fixture.etag = '"coordinate-version-two"'
            respond(200)
            return True
        return False

    fixture = auth.Fixture(hook, directory=directory)
    try:
        fixture.bytes = (ROOT / "tests/data/swot/grid-inspection.h5").read_bytes()
        r = copy.deepcopy(job.manifest()["observations"][0]["request"])
        r["source"]["assets"][0].update(
            id="fixture", locator=fixture.canonical, etag=fixture.etag
        )
        r["spatial"].update(start=[1, 2], count=[5, 7], step=[2, 3])
        policy = json.loads(fixture.config_path.read_text())
        request = directory / "input.json"
        job.write(
            request,
            dict(
                request=r,
                policy=policy,
                ca=str(fixture.cert),
                netrc=str(fixture.netrc),
                control=fixture.control,
            ),
        )
        env = {
            **os.environ,
            "TILE_RIPPER_GRID_FIXTURE": str(request),
            "TILE_RIPPER_HDF5_METADATA_POLICY": "page16k",
        }
        command = [
            str(Path.home() / ".cargo/bin/cargo"),
            "test",
            "--locked",
            "--no-default-features",
            "--features",
            "native-hdf5,authenticated-http",
            "--test",
            "grid_inspection",
            "authenticated_changed_coordinates_etag_and_concurrent_preparations",
            "--",
            "--ignored",
            "--nocapture",
        ]
        p = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, timeout=180)
        (directory / "rust.log").write_bytes(p.stdout + p.stderr)
        assert p.returncode == 0, (p.stdout + p.stderr).decode()[-5000:]
        for e in fixture.events:
            if e["role"] == "resource":
                assert not e["authorization_present"] and not e["cookie_present"]
        job.write(
            DOCS / "inspection-faults.json",
            dict(
                all_passed=True,
                canonical_locators=1,
                distinct_versions=2,
                actual_coordinate_change=True,
                concurrent_preparations=4,
                old_conflict_sticky=True,
                requests=len(fixture.events),
                public_provider_requests=0,
                log_sha256=job.digest(directory / "rust.log"),
            ),
        )
        print(
            "authenticated inspection version, coordinate and concurrent-context gates passed",
            flush=True,
        )
    finally:
        fixture.close()


def prepare_baseline():
    """Build afe41c2, then expose only its existing two-pass resolver to the adapter."""
    WORK.mkdir(parents=True, exist_ok=True)
    source = WORK / "baseline-source"
    source.mkdir(exist_ok=True)
    archive = WORK / "afe41c2.tar"
    subprocess.run(
        ["git", "archive", "afe41c2", "--output", str(archive)], cwd=ROOT, check=True
    )
    subprocess.run(["tar", "-xf", str(archive), "-C", str(source)], check=True)
    import os
    import shutil

    env = {**os.environ, "CARGO_TARGET_DIR": str(ROOT / "target")}
    base = [
        str(Path.home() / ".cargo/bin/cargo"),
        "build",
        "--locked",
        "--release",
        "--manifest-path",
        str(source / "Cargo.toml"),
        "--no-default-features",
        "--features",
        "native-hdf5,authenticated-http",
    ]
    subprocess.run([*base, "--bin", "tile-ripper"], env=env, cwd=ROOT, check=True)
    shutil.copyfile(ROOT / "target/release/tile-ripper", WORK / "baseline-tile-ripper")
    (WORK / "baseline-tile-ripper").chmod(0o755)
    p = source / "src/swot_aoi.rs"
    p.write_text(
        p.read_text().replace("pub(crate) fn inspect_with(", "pub fn inspect_with(")
    )
    s = (ROOT / "examples/grid_inspection_bench.rs").read_text()
    a = s.index("    fn generation(")
    b = s.index("    pub fn run()", a)
    body = """    fn generation(manifest:&mut Manifest,aois:&[Spatial],input:&Value)->Result<Value> {
        let mut resolutions=Vec::new();
        for (entry,aoi) in manifest.observations.iter_mut().zip(aois) {
            let mut cache=tile_ripper::cog::LayoutCache::default();
            let report=swot_aoi::inspect_with(entry,aoi,|r| {
                let Source::Assets{assets}=&r.source else { unreachable!() };
                tile_ripper::acquisition::prepare_authenticated(r,&mut cache,session(&assets[0],input)?)
            })?;
            resolutions.push(json!({"id":entry.id,"aoi_resolution":report}));
        }
        let mut factory=|a:&Asset|session(a,input);
        let plan=tile_ripper::swot_job::Job::authenticated(manifest,&mut factory,None)?.plan()?;
        Ok(json!({"observations":plan["observations"],"resolutions":resolutions}))
    }
"""
    (source / "examples/grid_inspection_bench.rs").write_text(s[:a] + body + s[b:])
    subprocess.run(
        [*base, "--example", "grid_inspection_bench"], env=env, cwd=ROOT, check=True
    )
    shutil.copyfile(ROOT / "target/release/examples/grid_inspection_bench", BASELINE)
    BASELINE.chmod(0o755)
    job.write(
        DOCS / "baseline-build.json",
        dict(
            commit="afe41c2",
            resolver_patch="pub(crate) to pub visibility only; original resolver body unchanged",
            adapter_sha256=job.digest(source / "examples/grid_inspection_bench.rs"),
            binary_sha256=job.digest(BASELINE),
            cli_sha256=job.digest(WORK / "baseline-tile-ripper"),
        ),
    )
    print(
        "baseline built; rebuild current release targets before benchmarking",
        flush=True,
    )


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser()
    parser.add_argument(
        "action",
        nargs="?",
        default="benchmark",
        choices=["benchmark", "faults", "prepare-baseline"],
    )
    action = parser.parse_args().action
    {"benchmark": main, "faults": faults, "prepare-baseline": prepare_baseline}[
        action
    ]()
