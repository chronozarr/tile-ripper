"""Source-validation milestone: frozen HTTPS fixtures, matched runs and faults.

No live provider access. Use the existing pinned scientific Python environment.
The anonymous Linux runner uses stdlib only and the platform's fixture CA file.
"""

import copy
import json
import os
import random
import shutil
import sys
import tempfile
from pathlib import Path

import source_validation_wire as wire

ROOT, WORK, DOCS = wire.ROOT, wire.WORK, wire.DOCS


def source_bytes(result):
    counts = {"metadata": 0, "payload": 0, "authorization_proof": 0}
    for source in result["observed"]["transport"].values():
        for phase, operations in source["phases"].items():
            kind = (
                "authorization_proof"
                if phase == "checkpoint_online_authorization"
                else "payload"
                if phase.endswith("_payload")
                else "metadata"
            )
            counts[kind] += sum(v["bytes"] for v in operations.values())
    return counts


def fixtures():
    import earthdata_auth as auth
    import native_cog_checks as cog
    import swot_quality as quality
    import zarr_workflow as v2

    tls = WORK / "tls"
    tls.mkdir(parents=True, exist_ok=True)
    fixture = auth.Fixture(directory=tls)
    fixture.close()
    objects = WORK / "fixtures/objects"
    objects.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ROOT / "tests/data/swot/quality-le.h5", objects / "science.h5")
    hdf5 = quality.request("placeholder")
    hdf5["spatial"].update(start=[3, 4], count=[17, 19])
    cog.fixture(objects / "science.tif")
    cog_request = cog.request("placeholder")
    v2.WORK = WORK / "fixtures/v2-source"
    request = json.loads(v2.fixture().read_text())
    shutil.copytree(
        request["source"]["assets"][0]["locator"], objects / "v2", dirs_exist_ok=True
    )
    request["sink"] = {"kind": "callback"}
    request["spatial"]["bbox"] = [500015, 3999695, 500305, 3999985]
    configs = {
        "hdf5": {
            "input": {"request": hdf5},
            "path": "/science.h5",
            "metadata": "/science.h5",
            "payload": "/science.h5",
        },
        "cog": {
            "input": {"request": cog_request},
            "path": "/science.tif",
            "metadata": "/science.tif",
            "payload": "/science.tif",
        },
        "zarr_v2": {
            "input": {"request": request},
            "path": "/v2/",
            "metadata": "/v2/.zattrs",
            "payload": "/v2/red/1.0.0",
        },
    }
    wire.write(WORK / "fixtures/requests.json", configs)
    print("Pinned HDF5/COG/v2 fixtures generated; run fixtures-v3 next", flush=True)


def fixtures_v3():
    import zarr_v3_input as v3

    objects = WORK / "fixtures/objects"
    configs = json.loads((WORK / "fixtures/requests.json").read_text())
    for name, sharded in [("zarr_v3", False), ("zarr_v3_sharded", True)]:
        source = v3.fixture(objects / name, sharded=sharded, shape=(5, 6, 8))
        r = v3.request("placeholder", "unused", shape=(5, 6, 8), dense=True)
        r["sink"] = {"kind": "callback"}
        payload = next(
            p for p in sorted((source / "signal/c").rglob("*")) if p.is_file()
        )
        configs[name] = {
            "input": {"request": r},
            "path": f"/{name}/",
            "metadata": f"/{name}/zarr.json",
            "payload": f"/{name}/{payload.relative_to(source).as_posix()}",
        }
    wire.write(WORK / "fixtures/requests.json", configs)
    files = {
        str(p.relative_to(objects)): wire.digest(p)
        for p in sorted(objects.rglob("*"))
        if p.is_file()
    }
    wire.write(
        DOCS / "fixtures.json",
        {
            "files": files,
            "requests": configs,
            "source_bytes": sum(
                p.stat().st_size for p in objects.rglob("*") if p.is_file()
            ),
        },
    )
    print("Pinned anonymous fixtures generated", flush=True)


def authenticated():
    import earthdata_auth as auth
    import grid_inspection as grid
    import swot_job as job
    import swot_quality as quality

    fixtures = grid.Fixtures()
    binaries = {
        label: WORK / label / "source_validation_bench"
        for label in ["baseline", "candidate"]
    }
    examples = {label: WORK / label / "grid_inspection_bench" for label in binaries}
    cli = {label: WORK / label / "tile-ripper" for label in binaries}
    rows, faults = [], []
    expected_chunks, expected_manifest, output_digest = None, None, None
    candidate_fetch = None
    try:
        manifest = fixtures.manifest()
        first = manifest["observations"][0]
        policies = json.loads(fixtures.policy.read_text())["sources"]
        single = {
            "swot": True,
            "request": copy.deepcopy(first["request"]),
            "policy": policies[0],
            "ca": str(fixtures.ca),
            "netrc": str(fixtures.netrc),
        }
        aois = [
            e.pop("geographic_selection")["aoi"]
            for e in copy.deepcopy(manifest)["observations"]
        ]
        draft = copy.deepcopy(manifest)
        for entry in draft["observations"]:
            entry.pop("geographic_selection")
            entry["request"]["spatial"].update(start=[0, 0], count=[1, 1], step=[1, 1])
        generation = {
            "manifest": draft,
            "aois": aois,
            "policies": policies,
            "ca": str(fixtures.ca),
            "netrc": str(fixtures.netrc),
        }
        smoke = bool(os.environ.get("TILE_RIPPER_VALIDATION_SMOKE"))
        for delay in [0] if smoke else [0, 0.025, 0.1]:
            for repeat in range(1 if smoke else 3):
                order = list(binaries)
                random.Random(20261023 + repeat).shuffle(order)
                for label in order:
                    fixtures.reset(delay)
                    for archive in fixtures.archives:
                        archive.fixture.binding_heads = 2 if label == "baseline" else 1
                    result, row = wire.measure(
                        binaries[label], single, f"auth-single-{delay}-{repeat}-{label}"
                    )
                    assert row["returncode"] == 0, row
                    expected_chunks = (
                        result["chunks"] if expected_chunks is None else expected_chunks
                    )
                    assert result["chunks"] == expected_chunks
                    row.update(
                        source="authenticated_swot",
                        delay_seconds=delay,
                        repeat=repeat,
                        implementation=label,
                        wire=wire.accounting(fixtures.archives[0].fixture.events),
                        transport=result["observed"]["transport"],
                        working_memory=result["inspection"]["aligned_execution"][
                            "working_memory"
                        ],
                    )
                    row["source_bytes"] = source_bytes(result)
                    assert (
                        sum(row["source_bytes"].values())
                        == row["wire"]["source_body_bytes"]
                    )
                    rows.append(row)
                    if repeat == 0 and delay == 0:
                        wire.write(
                            DOCS / f"trace-authenticated-swot-{label}.json",
                            fixtures.archives[0].fixture.events,
                        )
                    checkpoint = Path(
                        tempfile.mkdtemp(prefix=f"checkpoint-{label}-", dir=WORK)
                    )
                    for phase in ["warm", "replay"]:
                        fixtures.reset(delay)
                        result, row = wire.measure(
                            binaries[label],
                            {**single, "checkpoint": str(checkpoint)},
                            f"auth-checkpoint-{phase}-{delay}-{repeat}-{label}",
                        )
                        assert (
                            row["returncode"] == 0
                            and result["chunks"] == expected_chunks
                        ), row
                        row.update(
                            source="checkpoint_" + phase,
                            delay_seconds=delay,
                            repeat=repeat,
                            implementation=label,
                            wire=wire.accounting(fixtures.archives[0].fixture.events),
                            transport=result["observed"]["transport"],
                            working_memory=result["inspection"]["aligned_execution"][
                                "working_memory"
                            ],
                        )
                        row["source_bytes"] = source_bytes(result)
                        assert (
                            sum(row["source_bytes"].values())
                            == row["wire"]["source_body_bytes"]
                        )
                        if repeat == 0 and delay == 0:
                            wire.write(
                                DOCS / f"trace-checkpoint-{phase}-{label}.json",
                                fixtures.archives[0].fixture.events,
                            )
                        rows.append(row)
                        if phase == "replay":
                            assert (
                                row["wire"]["source_body_bytes"] == 8
                                and row["wire"]["authentication_requests"] > 0
                            )
                    fixtures.reset(delay)
                    result, row = wire.measure(
                        examples[label],
                        generation,
                        f"auth-geographic-{delay}-{repeat}-{label}",
                    )
                    assert row["returncode"] == 0, row
                    expected_manifest = (
                        result["manifest"]
                        if expected_manifest is None
                        else expected_manifest
                    )
                    assert result["manifest"] == expected_manifest
                    row.update(
                        source="three_source_generation",
                        delay_seconds=delay,
                        repeat=repeat,
                        implementation=label,
                        wire=wire.accounting(
                            [e for a in fixtures.archives for e in a.fixture.events]
                        ),
                        generation_seconds=result["generation_seconds"],
                        allocations=result["allocations_after"],
                    )
                    rows.append(row)
                    job.BINARY = cli[label]
                    fixtures.reset(delay)
                    plan = job.run(
                        f"validation-plan-{delay}-{repeat}-{label}",
                        expected_manifest,
                        fixtures,
                        command="plan",
                    )
                    assert plan["returncode"] == 0, plan["error"]
                    rows.append(
                        {
                            "source": "three_source_plan",
                            "implementation": label,
                            "delay_seconds": delay,
                            "repeat": repeat,
                            "wall_seconds": plan["wall_seconds"],
                            "wire": wire.accounting(
                                [e for a in fixtures.archives for e in a.fixture.events]
                            ),
                            "raw": plan,
                        }
                    )
                    fixtures.reset(delay)
                    fetched = job.run(
                        f"validation-fetch-{delay}-{repeat}-{label}",
                        expected_manifest,
                        fixtures,
                    )
                    assert fetched["returncode"] == 0, fetched["error"]
                    checksum = grid.science_digest(fetched["output"])
                    output_digest = checksum if output_digest is None else output_digest
                    assert checksum == output_digest
                    if label == "candidate":
                        candidate_fetch = fetched
                    rows.append(
                        {
                            "source": "three_source_fetch",
                            "implementation": label,
                            "delay_seconds": delay,
                            "repeat": repeat,
                            "wall_seconds": fetched["wall_seconds"],
                            "wire": wire.accounting(
                                [e for a in fixtures.archives for e in a.fixture.events]
                            ),
                            "science_sha256": checksum,
                            "raw": fetched,
                        }
                    )
                    print("authenticated", delay, repeat, label, "PASS", flush=True)
                wire.write(
                    WORK / "authenticated-benchmarks-full.json",
                    {"runs": rows, "scientific_output_sha256": output_digest},
                )
        fixtures.reset()
        job.BINARY = cli["candidate"]
        verification = job.verify(
            candidate_fetch, expected_manifest, sources=grid.REFERENCES
        )
        wire.write(DOCS / "three-source-science.json", verification)
        # Compare semantic fault positions with each binary's binding offset.
        fault_tls = WORK / "fault-tls"
        fault_tls.mkdir(exist_ok=True)
        fixture = auth.Fixture(
            lambda *args: fixture_hook[0](*args), directory=fault_tls
        )
        try:
            for case in [
                "success",
                "expired-once",
                "expired-after-cache",
                "short-expiry",
                "wrong-method",
                "login-401",
                "weak-etag",
                "weak-get-etag",
                "missing-get-etag",
                "length-head",
                "changed-length",
                "wrong-range",
                "truncated",
                "mutate-final",
                "mutate-bound",
                "mutate-inspection",
                "mutate-prepared",
                "mutate-delivered",
                "auth-after-success",
                "cancel",
            ]:
                decisions = {}
                for label, binary in binaries.items():
                    fixture.binding_heads = 2 if label == "baseline" else 1
                    fixture.case = case
                    fixture.events.clear()
                    fixture.counts.clear()
                    changed = False

                    def hook(handler, role, path, respond, case=case):
                        nonlocal changed
                        if role == "control":
                            if path.endswith(
                                "/"
                                + {
                                    "mutate-inspection": "inspected",
                                    "mutate-prepared": "prepared",
                                    "mutate-delivered": "delivered",
                                }.get(case, "never")
                            ):
                                changed = True
                            respond(200)
                            return True
                        if (
                            case == "mutate-bound"
                            and role == "resource"
                            and handler.command == "HEAD"
                            and fixture.counts[(role, "HEAD")] == 1
                        ):
                            respond(
                                200,
                                {
                                    "ETag": fixture.etag,
                                    "Content-Length": str(len(fixture.bytes)),
                                },
                            )
                            changed = True
                            return True
                        if (
                            case == "auth-after-success"
                            and fixture.counts[("resource", "HEAD")]
                            >= fixture.binding_heads + 2
                            and role in ["resource", "login"]
                        ):
                            respond(401 if role == "login" else 403)
                            return True
                        if changed and role == "resource":
                            respond(412)
                            return True
                        return False

                    # Fixture captures hook at construction; replace through a
                    # forwarding callback, without recreating signing/login servers.
                    fixture_hook[0] = hook
                    request = quality.request(fixture.canonical)
                    request["spatial"].update(start=[3, 4], count=[17, 19])
                    value = {
                        "swot": True,
                        "request": request,
                        "policy": json.loads(fixture.config_path.read_text()),
                        "ca": str(fixture.cert),
                        "netrc": str(fixture.netrc),
                        "control": fixture.control,
                        "cancel": case == "cancel",
                        "inspect_grid": case == "mutate-inspection",
                    }
                    if case == "mutate-inspection":
                        fixture.bytes = (
                            ROOT / "tests/data/swot/grid-inspection.h5"
                        ).read_bytes()
                    else:
                        fixture.bytes = (
                            ROOT / "tests/data/swot/quality-le.h5"
                        ).read_bytes()
                    result, row = wire.measure(
                        binary, value, f"auth-fault-{case}-{label}"
                    )
                    success = case in [
                        "success",
                        "expired-once",
                        "expired-after-cache",
                        "short-expiry",
                    ]
                    assert (row["returncode"] == 0) == success, (case, label, row)
                    decisions[label] = {
                        "returncode": row["returncode"],
                        "error": row.get("error"),
                        "wire": wire.accounting(fixture.events),
                    }
                faults.append(
                    {"case": case, "expected_success": success, "outcomes": decisions}
                )
                print(case, "both PASS", flush=True)
        finally:
            fixture.close()
        wire.write(
            DOCS / "authenticated-faults.json",
            {"cases": faults, "all_passed": True, "new_successful_fault_paths": 0},
        )
        wire.write(
            DOCS / "authenticated-benchmarks.json",
            {"runs": job.compact(rows), "scientific_output_sha256": output_digest},
        )
    finally:
        fixtures.close()


# One mutable callback is fixture machinery only, never acquisition state.
fixture_hook = [lambda *_: False]


if __name__ == "__main__":
    if sys.argv[1] == "fixtures":
        fixtures()
    elif sys.argv[1] == "fixtures-v3":
        fixtures_v3()
    elif sys.argv[1] == "authenticated":
        authenticated()
    else:
        raise ValueError(sys.argv[1])
