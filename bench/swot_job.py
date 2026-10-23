"""Explicit SWOT job acceptance, fault gates and matched durable-write controls.

offline uses pinned native captures over exact-policy local HTTPS and separate
guarded scientific reference captures. live is a bounded explicit NASA operation.
All raw outputs/temporary credentials stay in ignored prototype-output/swot-job.
"""

import argparse
import base64
import copy
import hashlib
import json
import os
from pathlib import Path
import resource
import shutil
import signal
import subprocess
import sys
import tempfile
import time

import numpy as np
import psutil
import xarray as xr
import zarr

import earthdata_auth as auth
import swot_held_out as held
import swot_quality as quality
import transfer_resume as resume
from hdf5_selective_mirror import Mirror

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/swot-job"
DOCS = ROOT / "docs/swot-job"
BINARY = ROOT / "target/release/tile-ripper"
ORIGINAL = ROOT / "prototype-output/swot-quality/mirror"
HELD_NATIVE = (
    ROOT
    / "prototype-output/held-out-qualification/original/prototype-output/transfer-resume/held-out-mirror"
)
HELD_REFERENCE = ROOT / "prototype-output/held-out-qualification/reference-mirror"
WORK.mkdir(parents=True, exist_ok=True)
DOCS.mkdir(parents=True, exist_ok=True)
held.WORK = WORK / "guards"
held.WORK.mkdir(exist_ok=True)
SECRETS = auth.SECRET_MARKERS + ["FIXTURE_PASSWORD_A", "FIXTURE_PASSWORD_B"]


def write(path, value):
    text = json.dumps(value, indent=2, allow_nan=False) + "\n"
    assert all(secret not in text for secret in SECRETS)
    path.write_text(text)


def save(name, value):
    write(WORK / f"{name}-full.json", value)
    write(DOCS / f"{name}.json", compact(value))


def compact(value):
    """Keep measurements and science receipts; raw metadata remains in output."""
    if isinstance(value, list):
        return [compact(v) for v in value]
    if not isinstance(value, dict):
        return value
    result = {k: compact(v) for k, v in value.items() if k not in ["datasets", "plan"]}
    native_plan = isinstance(value.get("plan"), dict) and "nodes" in value["plan"]
    if "plan" in value and not native_plan:
        result["plan"] = compact(value["plan"])
    if "datasets" in value:
        result["datasets"] = []
        for dataset in value["datasets"]:
            encoded = json.dumps(
                dataset, sort_keys=True, separators=(",", ":")
            ).encode()
            result["datasets"].append(
                {
                    "path": dataset["path"],
                    "shape": dataset["shape"],
                    "stored_dtype": dataset["stored_dtype"],
                    "chunk_shape": dataset["chunk_shape"],
                    "attributes_and_schema_sha256": hashlib.sha256(encoded).hexdigest(),
                    "units": dataset["attributes"].get("units", {}).get("value"),
                    "grid_mapping_path": (dataset.get("grid_mapping") or {}).get(
                        "path"
                    ),
                }
            )
        if value["datasets"]:
            first = value["datasets"][0]
            result["source_root_attributes"] = {
                k: v.get("value", v.get("nonfinite", "uninterpreted; see output"))
                for k, v in first["root_attributes"].items()
            }
            result["source_grid_mapping"] = first["grid_mapping"]
            result["source_time"] = first["time"]
    if native_plan:
        plan = value["plan"]
        result["plan_summary"] = {
            "selected": plan["selected"],
            "estimates": plan["estimates"],
            "nodes": len(plan["nodes"]),
            "limitations": plan["limitations"],
        }
    return result


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def manifest():
    return json.loads((ROOT / "examples/swot-job.json").read_text())


class Archive(resume.Archive):
    def __init__(self, mirror, delay=0):
        self.callback = None
        self.expected_auth = None
        super().__init__(mirror, delay)

    def hook(self, handler, role, path, respond):
        if role == "login" and self.expected_auth:
            assert handler.headers.get("Authorization") == self.expected_auth, (
                "source credential isolation failure"
            )
        if self.callback:
            if self.callback(handler, role, path, respond):
                return True
        return super().hook(handler, role, path, respond)


class Fixtures:
    def __init__(self, delay=0):
        for path in [ORIGINAL, HELD_NATIVE, HELD_REFERENCE]:
            Mirror(path).verify()
        # Same order as example: held-out first, then the earlier original.
        self.archives = [
            Archive(Mirror(HELD_NATIVE), delay),
            Archive(Mirror(ORIGINAL), delay),
        ]
        directory = Path(tempfile.mkdtemp(prefix="access-", dir=WORK))
        self.policy = directory / "policies.json"
        self.ca = directory / "ca.pem"
        self.netrc = directory / "private.netrc"
        policies = []
        for i, archive in enumerate(self.archives):
            fixture = archive.fixture
            config = json.loads(fixture.config_path.read_text())
            if i == 1:
                old = fixture.login
                fixture.login = old.replace("127.0.0.1", "localhost")
                config["login_origin"] = fixture.login
                for rule in config["redirects"]:
                    for key in ["from_origin", "to_origin"]:
                        if rule[key] == old:
                            rule[key] = fixture.login
            user = "source_a" if i == 0 else "source_b"
            password = "FIXTURE_PASSWORD_A" if i == 0 else "FIXTURE_PASSWORD_B"
            archive.expected_auth = (
                "Basic " + base64.b64encode(f"{user}:{password}".encode()).decode()
            )
            policies.append(config)
        write(self.policy, {"sources": policies})
        self.ca.write_bytes(
            b"\n".join(a.fixture.cert.read_bytes() for a in self.archives)
        )
        self.netrc.write_text(
            "machine 127.0.0.1 login source_a password FIXTURE_PASSWORD_A\nmachine localhost login source_b password FIXTURE_PASSWORD_B\n"
        )
        self.netrc.chmod(0o600)
        self.reset()

    def reset(self):
        for archive, mirror in zip(self.archives, [HELD_NATIVE, ORIGINAL]):
            archive.reset()
            archive.callback = None
            archive.fixture.etag = Mirror(mirror).info["etag"]

    def job(self):
        job = manifest()
        for entry, archive in zip(job["observations"], self.archives):
            entry["request"]["source"]["assets"][0].update(
                locator=archive.fixture.canonical, etag=archive.fixture.etag
            )
        return job

    def arguments(self, job):
        # Single-observation controls get the same one exact policy.
        policies = json.loads(self.policy.read_text())["sources"]
        wanted = {
            entry["request"]["source"]["assets"][0]["locator"]
            for entry in job["observations"]
        }
        selected = self.policy.with_name(
            f"selection-{len(wanted)}-{'-'.join(entry['id'] for entry in job['observations'])}.json"
        )
        write(
            selected, {"sources": [p for p in policies if p["canonical_url"] in wanted]}
        )
        return [
            "--auth",
            "configured",
            "--netrc",
            str(self.netrc),
            "--auth-policy",
            str(selected),
            "--auth-ca",
            str(self.ca),
        ]

    def traffic(self):
        rows = []
        for archive in self.archives:
            events = archive.fixture.events
            assert all(
                not e.get("authorization_present") and not e.get("cookie_present")
                for e in events
                if e["role"] == "resource"
            )
            assert all(
                not e.get("authorization_present")
                for e in events
                if e["role"] == "archive"
            )
            rows.append(
                {
                    "events": dict(archive.fixture.counts),
                    "requests": len(events),
                    "resource_get_bytes": sum(
                        e["body_bytes"]
                        for e in events
                        if e["role"] == "resource" and e["method"] == "GET"
                    ),
                    "redirect_body_bytes": sum(
                        e["body_bytes"] for e in events if e["role"] != "resource"
                    ),
                    "login_requests": sum(e["role"] == "login" for e in events),
                    "credential_cookie_boundaries_checked": True,
                }
            )
            rows[-1]["events"] = {
                f"{k[0]} {k[1]}": v for k, v in rows[-1]["events"].items()
            }
        return rows

    def close(self):
        for archive in self.archives:
            archive.close()


def tree(path):
    files = [p for p in path.rglob("*") if p.is_file()] if path.exists() else []
    return {
        "files": len(files),
        "logical_bytes": sum(p.stat().st_size for p in files),
        "allocated_bytes": sum(p.stat().st_blocks * 512 for p in files),
        "range_entries": sum(p.name.startswith("range-") for p in files),
    }


def run(
    name,
    job,
    fixtures=None,
    checkpoint=None,
    command="fetch",
    extra=(),
    stop=None,
    live=False,
    checkpoint_mib=32,
    checkpoint_entries=2048,
):
    directory = Path(tempfile.mkdtemp(prefix=name + "-", dir=WORK))
    input_path = directory / "job.json"
    write(input_path, job)
    output = directory / "observations.zarr"
    args = [str(BINARY), command, str(input_path)]
    if command == "fetch":
        args += ["--output", str(output)]
    if fixtures:
        args += fixtures.arguments(job)
    if live:
        args += ["--auth", "earthdata", "--netrc", str(Path.home() / ".netrc")]
    if checkpoint:
        args += [
            "--checkpoint",
            str(checkpoint),
            "--checkpoint-mib",
            str(checkpoint_mib),
            "--checkpoint-entries",
            str(checkpoint_entries),
        ]
    args += extra
    env = os.environ.copy()
    env.update(
        TILE_RIPPER_HDF5_METADATA_POLICY="page16k",
        TILE_RIPPER_WORKERS="1",
        TILE_RIPPER_PER_HOST="1",
        TILE_RIPPER_STAGE_TRACE=str(directory / "stage-trace.jsonl"),
    )
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started, peak, stopped = time.perf_counter(), 0, False
    with (
        (directory / "stdout.json").open("wb") as out,
        (directory / "stderr.txt").open("wb") as err,
    ):
        process = subprocess.Popen(args, stdout=out, stderr=err, env=env)
        while process.poll() is None:
            try:
                peak = max(peak, psutil.Process(process.pid).memory_info().rss)
            except psutil.NoSuchProcess:
                pass
            if stop and not stopped and stop(output, directory):
                process.send_signal(signal.SIGINT)
                stopped = True
            if time.perf_counter() - started > 90:
                process.kill()
                raise TimeoutError(name)
            time.sleep(0.002)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    stdout, stderr = (
        (directory / "stdout.json").read_text(),
        (directory / "stderr.txt").read_text(),
    )
    assert all(s not in stdout + stderr for s in SECRETS), "secret leaked to CLI output"
    receipt = json.loads(stdout) if stdout else None
    row = {
        "name": name,
        "command": args[:3]
        + ["[runtime access/output flags omitted from compact receipt]"],
        "returncode": process.returncode,
        "wall_seconds": time.perf_counter() - started,
        "cpu_seconds": after.ru_utime
        + after.ru_stime
        - before.ru_utime
        - before.ru_stime,
        "sampled_process_peak_rss_bytes": peak,
        "interruption_sent": stopped,
        "receipt": receipt,
        "error": stderr[-2500:] if process.returncode else None,
        "output": str(output),
        "output_files": tree(output),
        "checkpoint_files": tree(checkpoint) if checkpoint else None,
        "binary_sha256": digest(BINARY),
        "stdout_sha256": digest(directory / "stdout.json"),
        "stderr_sha256": digest(directory / "stderr.txt"),
    }
    if fixtures:
        row["wire"] = fixtures.traffic()
    trace = directory / "stage-trace.jsonl"
    row["publication_stages"] = (
        [json.loads(line) for line in trace.read_text().splitlines()]
        if trace.exists()
        else []
    )
    if process.returncode:
        assert not output.exists(), "failed job published a destination"
        assert not list(directory.glob("*.partial-*")) and not list(
            directory.glob(".*.partial-*")
        ), "failed job left ordinary staging"
    return row


def attr_equal(stored, expected, label):
    compared = 0
    for name, description in stored.items():
        if name not in expected:
            # NetCDF-C intentionally hides HDF5/NetCDF storage implementation attrs.
            assert name.startswith("_Netcdf") or name in [
                "DIMENSION_LIST",
                "REFERENCE_LIST",
                "CLASS",
                "NAME",
                "_NCProperties",
                "_ARRAY_DIMENSIONS",
            ], (label, name, "missing reference attribute")
            continue
        value = expected[name]
        if "value" in description:
            a, b = description["value"], value
            if (
                np.prod(description["shape"], dtype="u8") == 1
                and isinstance(b, list)
                and len(b) == 1
            ):
                b = b[0]
            assert a == b, (label, name, a, b)
            compared += 1
        elif "nonfinite" in description:
            assert isinstance(value, dict) and "nonfinite" in value, (label, name)
            compared += 1
        else:
            # Exact source bytes remain preserved even for uninterpreted storage
            # attributes; do not make them into invented CF values.
            assert "stored_bytes_base64" in description, (label, name)
    return compared


def verify(row, job, sources=None):
    assert row["returncode"] == 0, row["error"]
    destination = Path(row["output"])
    root = zarr.open_group(str(destination), mode="r")
    receipt = json.loads((destination / "manifest.json").read_text())
    assert root.attrs["status"] == receipt["status"] == "complete"
    assert receipt["observation_ids"] == [e["id"] for e in job["observations"]]
    results = []
    for i, entry in enumerate(job["observations"]):
        name = entry["id"]
        group = root[f"observations/{name}"]
        source = (
            sources[i]
            if sources
            else (HELD_REFERENCE if name.startswith("held") else ORIGINAL)
        )
        guarded = (source / "manifest.json").exists()
        references = []
        for reader in ["h5py", "netcdf4"]:
            out = Path(tempfile.mkdtemp(prefix=f"ref-{name}-{reader}-", dir=WORK))
            request = {
                "reader": reader,
                "local": str(source / "source.nc" if guarded else source),
                "spatial": entry["request"]["spatial"],
                "policy": entry["quality_policy"],
                "dump": str(out),
            }
            write(out / "request.json", request)
            env, trace = (
                held.guard_environment(source, out.name)
                if guarded
                else (os.environ.copy(), None)
            )
            process = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "bench/swot_job_reference.py"),
                    str(out / "request.json"),
                ],
                env=env,
                capture_output=True,
                timeout=60,
            )
            (out / "stdout.json").write_bytes(process.stdout)
            (out / "stderr.txt").write_bytes(process.stderr)
            assert process.returncode == 0, process.stderr.decode()[-3000:]
            events = trace.read_text().splitlines() if trace else []
            assert (
                not guarded or events and not any("CAPTURE_MISS" in e for e in events)
            ), events[-10:]
            metadata = json.loads((out / "attributes.json").read_text())
            storage_checks = 0
            if reader == "h5py":
                storage = json.loads((out / "attribute-storage.json").read_text())
                for variable, attributes in storage.items():
                    native = (
                        group.attrs["source_attributes"]
                        if variable == "root"
                        else group[
                            "spatial_ref" if variable == "crs" else variable
                        ].attrs["source_attributes"]
                    )
                    for key, stored in attributes.items():
                        assert native[key]["shape"] == stored["shape"], (
                            variable,
                            key,
                            "attribute shape",
                        )
                        assert native[key]["stored_bytes_base64"] == stored["bytes"], (
                            variable,
                            key,
                            "attribute raw bytes",
                        )
                        storage_checks += 1
            attribute_checks = attr_equal(
                group.attrs["source_attributes"], metadata["root"], "root"
            )
            for index, (variable, dtype) in enumerate(
                zip(quality.PATHS, quality.DTYPES)
            ):
                array = np.asarray(group[variable][:])
                assert array.dtype == np.dtype(dtype), (variable, array.dtype, dtype)
                assert array.tobytes() == (out / f"{index}.bin").read_bytes(), (
                    name,
                    reader,
                    variable,
                    "value bits",
                )
                assert (
                    group[f"source_valid_{variable}"][:].tobytes()
                    == (out / f"{index}.mask").read_bytes()
                ), (name, variable, "source mask")
                assert array.shape == tuple(entry["request"]["spatial"]["count"])
                attribute_checks += attr_equal(
                    group[variable].attrs["source_attributes"],
                    metadata[variable],
                    variable,
                )
            assert (
                group["quality_mask"][:].tobytes()
                == (out / "validity.bin").read_bytes()
            )
            for axis in ["x", "y"]:
                assert group[axis][:].tobytes() == (out / f"{axis}.bin").read_bytes(), (
                    name,
                    reader,
                    axis,
                )
                attribute_checks += attr_equal(
                    group[axis].attrs["source_attributes"], metadata[axis], axis
                )
            attribute_checks += attr_equal(
                group["spatial_ref"].attrs["source_attributes"], metadata["crs"], "CRS"
            )
            if (out / "t.bin").exists():
                assert (
                    np.asarray(group["t"][...]).tobytes()
                    == (out / "t.bin").read_bytes()
                )
                attribute_checks += attr_equal(
                    group["t"].attrs["source_attributes"], metadata["t"], "time"
                )
            else:
                assert "t" not in group
            asset = entry["request"]["source"]["assets"][0]
            provenance = group.attrs["source"]
            assert provenance["reference"] == asset["locator"]
            if asset.get("etag"):
                assert (
                    provenance["object_identities"]["identity"]["version"]["etag"]
                    == asset["etag"]
                )
            assert group.attrs["selection"] == entry["request"]["spatial"]
            assert group.attrs["quality_policy"]["policy"] == entry["quality_policy"]
            assert "aoi_mask" not in group
            references.append(
                {
                    **json.loads(process.stdout),
                    "interpreted_attribute_checks": attribute_checks,
                    "fixed_attribute_shape_and_byte_checks": storage_checks,
                    "guarded_file_reads": len(events),
                    "capture_misses": 0,
                    "trace_sha256": digest(trace) if trace else None,
                }
            )
        dataset = xr.open_zarr(
            str(destination),
            group=f"observations/{name}",
            consolidated=False,
            mask_and_scale=False,
            decode_times=False,
        )
        for variable in (
            quality.PATHS
            + [f"source_valid_{v}" for v in quality.PATHS]
            + ["quality_mask", "x", "y"]
        ):
            assert dataset[variable].values.tobytes() == group[variable][:].tobytes(), (
                name,
                variable,
                "xarray",
            )
        assert dataset["wse_qual_bitwise"].dtype == np.dtype("u4")
        dataset.close()
        results.append(
            {
                "id": name,
                "shape": list(group["wse"].shape),
                "source_identity": group.attrs["source"]["object_identities"][
                    "identity"
                ],
                "all_value_bits_masks_coordinates_time_source_identity_equal": True,
                "xarray_raw_equal": True,
                "references": references,
            }
        )
    return {
        "observations": results,
        "zarr_python": zarr.__version__,
        "xarray": xr.__version__,
        "numpy": np.__version__,
        "all_passed": True,
    }


def offline():
    fixtures = Fixtures(delay=0.001)
    rows = []
    try:
        save("capture-guard", guard_failure())
        job = fixtures.job()
        plan = run("plan", job, fixtures, command="plan")
        assert plan["returncode"] == 0, plan["error"]
        save("plan", plan)
        fixtures.reset()
        full = run("qualified-two-source", job, fixtures)
        full["science"] = verify(full, job)
        save("qualification", full)
        print("qualified two-source Zarr: exact independent science passed", flush=True)
        # Same observation contracts and durable Zarr writer on both sides.
        measurements = []
        for repeat in range(3):
            fixtures.reset()
            batch = run(f"matched-batch-{repeat}", job, fixtures)
            assert batch["returncode"] == 0, batch["error"]
            singles = []
            for entry in job["observations"]:
                fixtures.reset()
                individual = copy.deepcopy(job)
                individual["observations"] = [entry]
                control = run(
                    f"matched-individual-{entry['id']}-{repeat}", individual, fixtures
                )
                assert control["returncode"] == 0, control["error"]
                batch_group = zarr.open_group(batch["output"], mode="r")[
                    f"observations/{entry['id']}"
                ]
                single_group = zarr.open_group(control["output"], mode="r")[
                    f"observations/{entry['id']}"
                ]
                for name in batch_group.array_keys():
                    assert (
                        np.asarray(batch_group[name][...]).tobytes()
                        == np.asarray(single_group[name][...]).tobytes()
                    ), (entry["id"], name, "matched durable outputs")
                singles.append(control)
            assert sum(s["resource_get_bytes"] for s in batch["wire"]) == sum(
                s["resource_get_bytes"] for row in singles for s in row["wire"]
            )
            measurements.append({"batch": batch, "individuals": singles})
        save(
            "measurements",
            {
                "comparison": "same selections, authentication, final validation and durable Zarr writer; three local fixed-delay repetitions; no no-write baseline",
                "runs": measurements,
            },
        )
        # Successful source checkpoints survive a failed later observation.
        fixtures.reset()
        fixtures.archives[1].mode = "retry-exhausted"
        checkpoint = WORK / f"checkpoint-{time.time_ns()}"
        failed = run("later-source-fails", job, fixtures, checkpoint)
        assert (
            failed["returncode"] != 0
            and len(failed["receipt"]["provisional_source_receipts"]) == 1
        )
        rows.append(failed)
        fixtures.reset()
        recovered = run("restart-retains-source-bytes", job, fixtures, checkpoint)
        recovered["science"] = verify(recovered, job)
        assert recovered["returncode"] == 0
        assert (
            recovered["receipt"]["observations"][0]["observed"]["transport"]["science"][
                "checkpoint"
            ]["counts"]["retained_bytes_reused"]
            > 0
        )
        rows.append(recovered)
        fixtures.reset()
        replay = run("complete-checkpoint-reconstruction", job, fixtures, checkpoint)
        replay["science"] = verify(replay, job)
        assert replay["returncode"] == 0
        assert sum(s["resource_get_bytes"] for s in replay["wire"]) == 32, (
            "two prepare proofs plus two final-validation proofs"
        )
        assert replay["checkpoint_files"]["logical_bytes"] <= 32 * 1024 * 1024
        assert replay["checkpoint_files"]["range_entries"] <= 2048
        rows.append(replay)
        # An earlier source changes while the second source is being acquired.
        fixtures.reset()

        def mutate_first(handler, role, _path, _respond):
            if role == "resource" and handler.command == "GET":
                fixtures.archives[0].fixture.etag = '"changed-before-job-completion"'

        fixtures.archives[1].callback = mutate_first
        mutation = run("previous-source-mutation", job, fixtures)
        assert (
            mutation["returncode"] != 0
            and mutation["receipt"]["failed_observation"]
            == job["observations"][0]["id"]
        )
        assert len(mutation["receipt"]["provisional_source_receipts"]) == 2
        rows.append(mutation)
        fixtures.reset()

        def cross_source_login(handler, role, _path, respond):
            if role == "archive" and handler.command == "GET":
                respond(
                    302,
                    {
                        "Location": fixtures.archives[0].fixture.login
                        + "/oauth/authorize?state=FIXTURE_CODE"
                    },
                )
                return True
            return False

        fixtures.archives[1].callback = cross_source_login
        crossed = run("cross-source-login-refused", job, fixtures)
        assert crossed["returncode"] != 0
        assert (
            crossed["wire"][0]["login_requests"] == 1
            and crossed["wire"][1]["login_requests"] == 0
        )
        rows.append(crossed)
        fixtures.reset()
        fixtures.archives[1].mode = "science-gate"
        interrupted_checkpoint = WORK / f"checkpoint-interrupted-{time.time_ns()}"
        interrupted = run(
            "cancel-acquisition",
            job,
            fixtures,
            interrupted_checkpoint,
            stop=lambda *_: fixtures.archives[1].block.is_set(),
        )
        assert interrupted["returncode"] != 0 and interrupted["interruption_sent"]
        rows.append(interrupted)
        fixtures.archives[1].release.set()
        fixtures.reset()
        restarted = run("cancel-restart", job, fixtures, interrupted_checkpoint)
        restarted["science"] = verify(restarted, job)
        rows.append(restarted)
        fixtures.reset()
        small = copy.deepcopy(job)
        small["limits"]["memory_bytes"] = 64 * 1024 * 1024
        row = run("small-aggregate-memory", small, fixtures)
        assert row["returncode"] != 0 and "aggregate memory" in row["error"]
        assert sum(s["requests"] for s in row["wire"]) == 0
        rows.append(row)
        fixtures.reset()
        disk = run(
            "small-aggregate-disk",
            job,
            fixtures,
            WORK / f"small-disk-{time.time_ns()}",
            checkpoint_mib=1,
        )
        assert disk["returncode"] != 0 and "checkpoint" in disk["error"]
        assert disk["checkpoint_files"]["logical_bytes"] <= 1024 * 1024
        rows.append(disk)
        fixtures.reset()
        entries = run(
            "small-aggregate-entries",
            job,
            fixtures,
            WORK / f"small-entries-{time.time_ns()}",
            checkpoint_entries=2,
        )
        assert entries["returncode"] != 0 and "checkpoint" in entries["error"]
        assert entries["checkpoint_files"]["range_entries"] <= 2
        rows.append(entries)
        fixtures.reset()
        lowered = run(
            "lowered-disk-retained-inventory",
            job,
            fixtures,
            checkpoint,
            checkpoint_mib=1,
        )
        assert lowered["returncode"] != 0 and "retained" in lowered["error"]
        assert sum(s["requests"] for s in lowered["wire"]) == 0
        rows.append(lowered)
        fixtures.reset()
        changed = copy.deepcopy(job)
        changed["observations"][0]["quality_policy"] = {"kind": "raw"}
        mismatch = run("checkpoint-policy-mismatch", changed, fixtures, checkpoint)
        assert mismatch["returncode"] != 0 and "checkpoint job" in mismatch["error"]
        rows.append(mismatch)
        # Different selected dimensions and strides on the same native sources.
        fixtures.reset()
        different = copy.deepcopy(job)
        different["observations"][0]["request"]["spatial"]["count"] = [63, 45]
        different["observations"][0]["request"]["spatial"]["step"] = [2, 2]
        different["observations"][1]["request"]["spatial"]["count"] = [96, 80]
        heterogeneous = run("different-dimensions-strides", different, fixtures)
        heterogeneous["science"] = verify(heterogeneous, different)
        rows.append(heterogeneous)
        # All-invalid selection is still a completed, raw-value-preserving group.
        fixtures.reset()
        invalid = copy.deepcopy(job)
        invalid["observations"] = [invalid["observations"][0]]
        invalid["observations"][0]["request"]["spatial"]["start"] = [600, 400]
        empty_quality = run("all-invalid-nonempty", invalid, fixtures)
        empty_quality["science"] = verify(empty_quality, invalid)
        assert (
            empty_quality["science"]["observations"][0]["references"][0]["summary"][
                "selected_count"
            ]
            == 0
        )
        rows.append(empty_quality)
        rows.extend(generated_cases())
        save("faults-restart", {"cases": rows, "all_passed": True})
        save(
            "boundaries",
            {
                "memory": "source cap plus explicit job/writer reservations; serial synchronous producer and sink; RSS sampled separately",
                "checkpoint": "exclusive root and fixed equal per-source disk/entry quotas; all retained source namespaces inventoried before new writes",
                "authentication": "two independent exact-object sessions, distinct login hosts and credentials; login-only Authorization and origin-only cookies asserted by HTTPS fixtures; new session for final sweep",
                "secret_redaction": "all subprocess stdout/stderr and compact receipts checked for fixture secret markers",
                "all_passed": True,
            },
        )
        print(
            "offline restart, mutation, cancellation and heterogeneous selections passed",
            flush=True,
        )
    finally:
        fixtures.close()


def generated_cases():
    """Independent local fixture checks for bit patterns and unsupported schema."""
    import h5py

    directory = Path(tempfile.mkdtemp(prefix="numeric-fixture-", dir=WORK))
    path = directory / "source.h5"
    shutil.copy2(ROOT / "tests/data/swot/quality-be.h5", path)
    with h5py.File(path, "r+") as source:
        for point, bits in zip(
            [(1, 2), (4, 7), (7, 12)], [0x7FC00001, 0xFFC01234, 0x80000000]
        ):
            source["wse"][point] = np.array(bits, dtype="u4").view("f4")
        source["n_wse_pix"][1, 2] = np.uint32(4294967294)
        source["n_wse_pix"][4, 7] = np.uint32(2147483648)
        source["wse"].attrs["scale_factor"] = np.float32(0.5)
        source["wse"].attrs["add_offset"] = np.float32(100.0)
        source["wse"].attrs["_ARRAY_DIMENSIONS"] = np.bytes_("raw source annotation")
        source["wse"].attrs["source_attributes"] = np.bytes_(
            "raw source provenance annotation"
        )
        source.create_dataset(
            "t", data=np.array(0x8000000000000000, dtype="u8").view("f8")
        )
        source["t"].attrs["units"] = np.bytes_("seconds since 2000-01-01 00:00:00")
        source["t"].attrs["calendar"] = np.bytes_("gregorian")
    job = manifest()
    job["observations"] = [job["observations"][0]]
    entry = job["observations"][0]
    entry["id"] = "numeric_bit_patterns"
    entry["request"]["source"]["assets"][0].update(locator=str(path))
    entry["request"]["source"]["assets"][0].pop("etag", None)
    entry["request"]["spatial"].update(start=[1, 2], count=[12, 10], step=[3, 5])
    entry["request"]["budget"]["buffer_bytes"] = 4 * 1024 * 1024
    job["limits"]["memory_bytes"] = 64 * 1024 * 1024
    job["limits"]["output_chunk_edge"] = 8
    result = run("raw-nan-payloads-signed-zero-high-uint32-packing-time", job)
    result["science"] = verify(result, job, [path])
    values = zarr.open_group(result["output"], mode="r")[
        "observations/numeric_bit_patterns/wse"
    ][:].view("u4")
    assert [int(values[i, i]) for i in range(3)] == [0x7FC00001, 0xFFC01234, 0x80000000]
    rows = [result]
    for case in [
        "processing-release",
        "summary-definitions",
        "bitwise-definitions",
        "missing-crs",
    ]:
        other = directory / f"{case}.h5"
        shutil.copy2(path, other)
        with h5py.File(other, "r+") as source:
            if case == "processing-release":
                source.attrs.modify("crid", np.bytes_("PID0"))
            elif case == "summary-definitions":
                source["wse_qual"].attrs.modify(
                    "flag_meanings", np.bytes_("uninspected categories")
                )
            elif case == "bitwise-definitions":
                flags = source["wse_qual_bitwise"].attrs["flag_masks"].copy()
                flags[0] = 1
                source["wse_qual_bitwise"].attrs.modify("flag_masks", flags)
            else:
                for name in quality.PATHS:
                    del source[name].attrs["grid_mapping"]
        changed = copy.deepcopy(job)
        changed["observations"][0]["request"]["source"]["assets"][0]["locator"] = str(
            other
        )
        failed = run("unsupported-" + case, changed)
        assert failed["returncode"] != 0
        rows.append(failed)
    return rows


def guard_failure():
    """Both scientific libraries must reject an intentionally omitted extent."""
    rows = []
    entry = manifest()["observations"][0]
    for reader in ["h5py", "netcdf4"]:
        out = Path(tempfile.mkdtemp(prefix="guard-hole-" + reader + "-", dir=WORK))
        env, trace = held.guard_environment(HELD_REFERENCE, out.name)
        # Hide the captured signature without modifying the source or original
        # capture. The interposer must refuse the file read before parsing.
        original = json.loads((HELD_REFERENCE / "manifest.json").read_text())["extents"]
        extents = out / "incomplete.extents"
        extents.write_text("".join(f"{max(a, 8)} {b}\n" for a, b in original if b > 8))
        env["TR_CAPTURE_EXTENTS"] = str(extents)
        request = {
            "reader": reader,
            "local": str(HELD_REFERENCE / "source.nc"),
            "spatial": entry["request"]["spatial"],
            "policy": entry["quality_policy"],
            "dump": str(out),
        }
        write(out / "request.json", request)
        process = subprocess.run(
            [
                sys.executable,
                str(ROOT / "bench/swot_job_reference.py"),
                str(out / "request.json"),
            ],
            env=env,
            capture_output=True,
            timeout=30,
        )
        events = trace.read_text().splitlines()
        assert process.returncode != 0 and any(
            "CAPTURE_MISS" in line for line in events
        )
        rows.append(
            {
                "reader": reader,
                "returncode": process.returncode,
                "expected_capture_misses": [
                    line for line in events if "CAPTURE_MISS" in line
                ],
                "trace_sha256": digest(trace),
                "missing_bytes_did_not_become_source_zeros": True,
            }
        )
    return {"cases": rows, "original_captures_modified": False, "all_passed": True}


def live():
    # One write and one complete checkpoint reconstruction: at most 16 MiB
    # source-response bodies expected from these two already qualified slices.
    job = manifest()
    checkpoint = WORK / f"live-checkpoint-{time.time_ns()}"
    first = run("live-two-source", job, checkpoint=checkpoint, live=True)
    first["science"] = verify(first, job)
    replay = run("live-reconstruction", job, checkpoint=checkpoint, live=True)
    replay["science"] = verify(replay, job)
    save("live", live_receipt(first, replay))
    print(
        "bounded live two-source acquisition and checkpoint reconstruction passed",
        flush=True,
    )


def live_receipt(first, replay, final_binary=None):
    source_bytes, redirect_bytes = 0, 0
    for row in [first, replay] + ([final_binary] if final_binary else []):
        for source in row["receipt"]["observations"]:
            for transport in [
                source["observed"]["transport"]["science"],
                source["job_final_validation"]["transport"],
            ]:
                source_bytes += transport["bytes"]
                redirect_bytes += transport["authenticated_http"]["redirect_body_bytes"]
    assert source_bytes <= 16 * 1024 * 1024
    receipt = {
        "expected_source_body_cap_bytes": 16 * 1024 * 1024,
        "observed_source_body_bytes": source_bytes,
        "observed_redirect_body_bytes": redirect_bytes,
        "first": first,
        "reconstruction": replay,
        "all_passed": True,
    }
    if final_binary:
        receipt["final_binary_reconstruction"] = final_binary
    return receipt


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["offline", "live"])
    args = parser.parse_args()
    {"offline": offline, "live": live}[args.command]()
