"""Bounded held-out reference diagnostics and qualification receipts.

diagnose is offline. complete-reference explicitly fetches only the exact holes
reported by the guarded NetCDF-C open, into a separate reference-only capture.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

import requests

from hdf5_selective_mirror import Mirror

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/held-out-qualification"
DOCS = ROOT / "docs/held-out-qualification"
ORIGINAL = WORK / "original"
ORIGINAL_MIRROR = ORIGINAL / "prototype-output/transfer-resume/held-out-mirror"
REFERENCE = WORK / "reference-mirror"


def preserve():
    """Snapshot the staged 987e760 failure without modifying the original files."""
    sources = [
        "docs/transfer-resume/report.md",
        "docs/transfer-resume/held-out.json",
        "docs/transfer-resume/held-out-candidates.json",
        "docs/transfer-resume/decision.md",
        "docs/earthdata-auth/access-contract.json",
        "bench/swot_quality_reference.py",
        "bench/hdf5_reference.py",
        "prototype-output/transfer-resume/held-out-stage.json",
        "prototype-output/transfer-resume/held-out-netcdf-request.json",
        "prototype-output/transfer-resume/held-out-stream-result.json",
        "prototype-output/transfer-resume/held-out-stream-128mib-result.json",
        "prototype-output/transfer-resume/held-out-mirror/manifest.json",
        "prototype-output/transfer-resume/held-out-mirror/source.nc",
    ]
    if ORIGINAL.exists():
        receipt = json.loads((ORIGINAL / "preservation.json").read_text())
        for row in receipt["preserved"]:
            assert (
                hashlib.sha256((ORIGINAL / row["path"]).read_bytes()).hexdigest()
                == row["sha256"]
            )
        print("original preserved snapshot verified", flush=True)
        return
    import importlib.metadata
    import platform

    ORIGINAL.mkdir(parents=True)
    rows = []
    for source in sources:
        p = ROOT / source
        target = ORIGINAL / source
        target.parent.mkdir(parents=True, exist_ok=True)
        if source.startswith("prototype-output/"):
            shutil.copy2(p, target)
        else:
            target.write_bytes(
                subprocess.check_output(
                    [
                        "git",
                        "show",
                        "987e7600586fab04f5a588ff6a68fbd93d7f5a33:" + source,
                    ],
                    cwd=ROOT,
                )
            )
        rows.append(
            {
                "path": source,
                "bytes": target.stat().st_size,
                "sha256": hashlib.sha256(target.read_bytes()).hexdigest(),
            }
        )
    stage = json.loads((ROOT / sources[7]).read_text())
    shutil.copytree(stage["checkpoint_directory"], ORIGINAL / "checkpoint")
    write(
        ORIGINAL / "preservation.json",
        {
            "python": sys.version,
            "executable": sys.executable,
            "platform": platform.platform(),
            "base_commit": "987e7600586fab04f5a588ff6a68fbd93d7f5a33",
            "packages": {
                n: importlib.metadata.version(n)
                for n in ["numpy", "h5py", "netCDF4", "requests", "psutil"]
            },
            "preserved": rows,
        },
    )
    args = [
        sys.executable,
        str(ROOT / "bench/swot_quality_reference.py"),
        str(ROOT / "prototype-output/transfer-resume/held-out-netcdf-request.json"),
    ]
    p = subprocess.run(args, capture_output=True, timeout=30)
    (ORIGINAL / "failure.stdout").write_bytes(p.stdout)
    (ORIGINAL / "failure.stderr").write_bytes(p.stderr)
    write(
        ORIGINAL / "failure.json",
        {
            "command": args,
            "returncode": p.returncode,
            "signal": -p.returncode if p.returncode < 0 else None,
        },
    )
    print("preserved original reference returncode", p.returncode, flush=True)


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def guard_environment(mirror, name):
    manifest = json.loads((mirror / "manifest.json").read_text())
    extents = WORK / (name + ".extents")
    extents.write_text("".join(f"{a} {b}\n" for a, b in manifest["extents"]))
    library = WORK / (
        "capture_guard.dylib" if sys.platform == "darwin" else "capture_guard.so"
    )
    if (
        not library.exists()
        or library.stat().st_mtime < (ROOT / "bench/capture_guard.c").stat().st_mtime
    ):
        args = ["cc", "-Wall", "-Wextra", "-Werror"]
        args += ["-dynamiclib"] if sys.platform == "darwin" else ["-shared", "-fPIC"]
        args += ["-o", str(library), str(ROOT / "bench/capture_guard.c")]
        if sys.platform != "darwin":
            args += ["-ldl"]
        subprocess.run(args, check=True)
    trace = WORK / (name + ".trace")
    trace.unlink(missing_ok=True)
    env = os.environ.copy()
    env.update(
        TR_CAPTURE_FILE=str(mirror / "source.nc"),
        TR_CAPTURE_EXTENTS=str(extents),
        TR_CAPTURE_TRACE=str(trace),
    )
    env["DYLD_INSERT_LIBRARIES" if sys.platform == "darwin" else "LD_PRELOAD"] = str(
        library
    )
    return env, trace


def probe(mirror, reader, operation, name, guarded=True):
    env, trace = (
        guard_environment(mirror, name) if guarded else (os.environ.copy(), None)
    )
    args = [
        sys.executable,
        str(ROOT / "bench/swot_reference_probe.py"),
        reader,
        str(mirror / "source.nc"),
        operation,
    ]
    p = subprocess.run(args, env=env, capture_output=True, timeout=30)
    (WORK / (name + ".stdout")).write_bytes(p.stdout)
    (WORK / (name + ".stderr")).write_bytes(p.stderr)
    events = trace.read_text().splitlines() if trace and trace.exists() else []
    return {
        "name": name,
        "command": args,
        "returncode": p.returncode,
        "stdout": p.stdout.decode(errors="replace"),
        "stderr": p.stderr.decode(errors="replace"),
        "events": events,
        "misses": [
            list(map(int, v.split()[1:]))
            for v in events
            if v.startswith("CAPTURE_MISS")
        ],
    }


def compact_probe(row):
    """Full transcripts remain in WORK; version only phase/range/error receipts."""
    return {
        "name": row["name"],
        "command": row["command"],
        "returncode": row["returncode"],
        "signal": -row["returncode"] if row["returncode"] < 0 else None,
        "phases": [json.loads(line)["phase"] for line in row["stdout"].splitlines()],
        "misses": row["misses"],
        "reads": len(row["events"]),
        "trace_sha256": hashlib.sha256(
            ("\n".join(row["events"]) + "\n").encode()
        ).hexdigest(),
        "stdout_sha256": hashlib.sha256(row["stdout"].encode()).hexdigest(),
        "stderr_sha256": hashlib.sha256(row["stderr"].encode()).hexdigest(),
        "stderr_excerpt": row["stderr"][-512:],
        "raw_transcript_prefix": str(WORK / row["name"]),
    }


def diagnose():
    rows = []
    for reader in ["netcdf4", "h5py"]:
        for operation in [
            "open",
            "enumerate",
            "read:x",
            "read:y",
            "attributes:wse",
            "read:wse",
            "read:wse_qual",
            "read:wse_qual_bitwise",
            "read:wse_uncert",
            "read:n_wse_pix",
            "close",
        ]:
            for strict in [False, True]:
                name = f"{reader}-{operation.replace(':', '-')}-{'strict' if strict else 'original'}"
                rows.append(probe(ORIGINAL_MIRROR, reader, operation, name, strict))
    write(DOCS / "diagnostic.json", rows)
    for row in rows:
        print(row["name"], row["returncode"], row["misses"], flush=True)


def reference_regression():
    original = probe(ORIGINAL_MIRROR, "netcdf4", "open", "regression-original-open")
    assert original["returncode"] != 0 and original["misses"] == [[4985152, 4985664]]
    assert '"phase": "open.end"' not in original["stdout"]
    # A direct POSIX read must fail at the same hole with EIO. This tests the
    # interposer independently of either HDF5 library's error handling.
    env, trace = guard_environment(ORIGINAL_MIRROR, "guard-control")
    program = """import os,sys,errno
fd=os.open(sys.argv[1],os.O_RDONLY)
assert os.pread(fd,8,0)==b'\\x89HDF\\r\\n\\x1a\\n'
try: os.pread(fd,512,4985152)
except OSError as error: assert error.errno==errno.EIO
else: raise AssertionError('capture hole supplied bytes')
os.close(fd)
"""
    subprocess.run(
        [sys.executable, "-c", program, str(ORIGINAL_MIRROR / "source.nc")],
        env=env,
        check=True,
        timeout=10,
    )
    assert trace.read_text().splitlines() == ["HIT 0 8", "CAPTURE_MISS 4985152 4985664"]
    rows = []
    for operation in [
        "open",
        "enumerate",
        "read:x",
        "read:y",
        "attributes:wse",
        "read:wse",
        "read:wse_qual",
        "read:wse_qual_bitwise",
        "read:wse_uncert",
        "read:n_wse_pix",
        "close",
    ]:
        for reader in ["netcdf4", "h5py"]:
            row = probe(
                REFERENCE,
                reader,
                operation,
                "regression-" + reader + "-" + operation.replace(":", "-"),
            )
            assert row["returncode"] == 0 and row["events"] and not row["misses"]
            rows.append(row)
    write(
        DOCS / "reference-regression.json",
        {
            "passed": True,
            "original_capture_rejected": original,
            "posix_hole_is_explicit_eio": True,
            "corrected_operations": [compact_probe(row) for row in rows],
        },
    )
    print(
        "reference regression PASS: original miss, explicit EIO, 22 corrected operations",
        flush=True,
    )


class OfficialSession(requests.Session):
    def head(self, url, **kwargs):
        kwargs["allow_redirects"] = True
        return super().head(url, **kwargs)


def complete_reference():
    # This receipt is installed before any network operation. Do not silently
    # widen it on exhaustion; stop and report the measured requirement.
    bounds = {
        "additional_body_bytes": 512 * 1024,
        "additional_range_gets": 128,
        "scope": "reference-only root/group/variable metadata required by NetCDF-C eager open; exact guard misses only",
        "native_accounting": "excluded; reference capture never supplied to native acquisition",
    }
    budget_path = DOCS / "reference-budget.json"
    if budget_path.exists():
        assert json.loads(budget_path.read_text()) == bounds
    else:
        write(budget_path, bounds)
    if not REFERENCE.exists():
        shutil.copytree(ORIGINAL_MIRROR, REFERENCE)
    mirror = Mirror(REFERENCE)
    mirror.verify()
    mirror.info["cap"] = bounds["additional_body_bytes"]
    mirror.save()
    mirror = Mirror(REFERENCE, url=mirror.info["url"], session=OfficialSession())
    receipt_path = DOCS / "reference-ranges.json"
    receipt = (
        json.loads(receipt_path.read_text())
        if receipt_path.exists()
        else {"bounds": bounds, "attempts": [], "added": []}
    )
    while True:
        row = probe(
            REFERENCE,
            "netcdf4",
            "enumerate",
            f"reference-fill-{len(receipt['attempts'])}",
        )
        receipt["attempts"].append(compact_probe(row))
        write(receipt_path, receipt)
        if row["returncode"] == 0:
            assert row["events"] and not row["misses"]
            break
        if not row["misses"]:
            raise RuntimeError("reference failed without an explicit capture miss")
        for start, end in row["misses"]:
            for a, b in mirror.missing(start, end):
                if mirror.info["provider_gets"] >= bounds["additional_range_gets"]:
                    raise RuntimeError("explicit reference request cap exhausted")
                # Mirror enforces byte cap, framing, exact ETag/length and identity encoding.
                data = mirror.read(a, b - a)
                receipt["added"].append(
                    {
                        "start": a,
                        "end": b,
                        "bytes": len(data),
                        "sha256": hashlib.sha256(data).hexdigest(),
                        "reason": "exact NetCDF-C open/enumeration capture miss",
                    }
                )
                mirror.seal()
                mirror.capture = True
                write(receipt_path, receipt)
                print(
                    "reference-only",
                    a,
                    b,
                    "total",
                    mirror.info["provider_bytes"],
                    flush=True,
                )
    mirror.seal()
    receipt.update(
        source={k: mirror.info[k] for k in ["url", "etag", "size"]},
        additional_bytes=mirror.info["provider_bytes"],
        additional_range_gets=mirror.info["provider_gets"],
        manifest=mirror.info,
    )
    write(receipt_path, receipt)
    print(
        "reference enumeration PASS",
        receipt["additional_bytes"],
        receipt["additional_range_gets"],
        flush=True,
    )


def select():
    """Inspect a fixed sparse quality lattice from native-captured bytes only."""
    import h5py
    import numpy as np
    from hdf5_reference import Remote
    from hdf5_selective_mirror import server

    bounds = {
        "quality_sample_step": 16,
        "max_quality_cells": 16384,
        "additional_provider_bytes": 0,
        "max_final_windows": 1,
        "window_shape": [128, 128],
        "rule": "sample /wse_qual every 16 cells from (0,0), row-major; center a clipped 128x128 window on the first stored good/suspect sample; preserve original empty window",
    }
    write(DOCS / "selection-budget.json", bounds)
    mirror = Mirror(ORIGINAL_MIRROR)
    mirror.verify()
    with server(mirror) as srv:
        remote = Remote(srv.url)
        with h5py.File(remote, "r") as f:
            shape = f["wse_qual"].shape
            sampled = f["wse_qual"][::16, ::16]
            assert sampled.size <= bounds["max_quality_cells"]
            accepted = np.argwhere(np.isin(sampled, [0, 1]))
            if not len(accepted):
                raise RuntimeError(
                    "no good/suspect quality sample inside the fixed inspection budget"
                )
            center = [int(v) * 16 for v in accepted[0]]
            start = [max(0, min(c - 64, d - 128)) for c, d in zip(center, shape)]
            point = [float(f[n][i]) for n, i in zip(["y", "x"], center)]
        remote.close()
        assert not srv.errors
        events = srv.events
    stage = json.loads(
        (ORIGINAL / "prototype-output/transfer-resume/held-out-stage.json").read_text()
    )
    request = stage["request"]
    request["spatial"].update(start=start, count=[128, 128], step=[1, 1])
    request["budget"]["buffer_bytes"] = 64 * 1024 * 1024
    write(DOCS / "request.json", request)
    policy = json.loads((ROOT / "docs/swot-quality/cookbook-policy.json").read_text())
    write(DOCS / "policy.json", policy)
    write(
        DOCS / "selection.json",
        {
            "bounds": bounds,
            "shape": list(shape),
            "inspected": [
                {
                    "kind": "original_edge_case",
                    "start": [600, 400],
                    "count": [128, 128],
                    "step": [1, 1],
                    "quality_accepted_cells": 0,
                },
                {
                    "kind": "quality_only_lattice",
                    "start": [0, 0],
                    "count": list(sampled.shape),
                    "step": [16, 16],
                    "category_counts": {
                        str(v): int((sampled == v).sum()) for v in np.unique(sampled)
                    },
                    "sha256": hashlib.sha256(sampled.tobytes()).hexdigest(),
                },
            ],
            "chosen_sample": center,
            "chosen_sample_yx_m": point,
            "final_spatial": request["spatial"],
            "events": events,
            "additional_provider_bytes": 0,
            "policy": policy,
            "source": {k: mirror.info[k] for k in ["url", "etag", "size"]},
        },
    )
    print(
        "selected",
        start,
        "quality sample",
        center,
        "accepted samples",
        len(accepted),
        flush=True,
    )


def verify_result():
    """Runs in a guarded process, with separate one-reader reference children."""
    import transfer_resume as resume

    value = json.loads(Path(sys.argv[2]).read_text())
    resume.q.WORK = WORK / "references"
    result = resume.verify(
        value["native"],
        value["request"],
        REFERENCE / "source.nc",
        value["policy"],
        value["name"],
    )
    print(json.dumps(result["science"], allow_nan=False))


def guarded_verify(native, request, policy, name):
    input_path = WORK / (name + "-verify-input.json")
    write(
        input_path,
        {"native": native, "request": request, "policy": policy, "name": name},
    )
    env, trace = guard_environment(REFERENCE, name + "-science")
    p = subprocess.run(
        [
            sys.executable,
            str(Path(__file__).resolve()),
            "verify-result",
            str(input_path),
        ],
        env=env,
        capture_output=True,
        timeout=120,
    )
    (WORK / (name + "-verify.stdout")).write_bytes(p.stdout)
    (WORK / (name + "-verify.stderr")).write_bytes(p.stderr)
    assert p.returncode == 0, p.stderr.decode(errors="replace")[-3000:]
    events = trace.read_text().splitlines()
    assert events and not any("CAPTURE_MISS" in e for e in events)
    result = json.loads(p.stdout)
    result.update(
        guarded_file_reads=len(events),
        capture_misses=0,
        read_trace_sha256=hashlib.sha256(trace.read_bytes()).hexdigest(),
    )
    return result


def qualify():
    import transfer_resume as resume

    for p in [ORIGINAL_MIRROR, REFERENCE]:
        Mirror(p).verify()
    policy = json.loads((DOCS / "policy.json").read_text())
    # Qualify the preserved edge case with both readers too.
    old = json.loads(
        (
            ORIGINAL
            / "prototype-output/transfer-resume/held-out-stream-128mib-result.json"
        ).read_text()
    )
    old_request = json.loads(
        (ORIGINAL / "prototype-output/transfer-resume/held-out-stage.json").read_text()
    )["request"]
    old_request["budget"]["buffer_bytes"] = 128 * 1024 * 1024
    old["output_dir"] = str(
        ROOT / "prototype-output/transfer-resume/held-out-final-replay"
    )
    edge = guarded_verify(old, old_request, policy, "original-empty")
    resume.WORK = WORK / "native"
    resume.WORK.mkdir(exist_ok=True)
    archive = resume.Archive(Mirror(ORIGINAL_MIRROR))
    request = json.loads((DOCS / "request.json").read_text())
    request["source"]["assets"][0].update(
        locator=archive.fixture.canonical, etag=archive.fixture.etag
    )
    try:
        native = resume.run(
            "offline-nonempty", request, archive, policy=policy, metadata="page16k"
        )
        write(WORK / "offline-nonempty-result.json", native)
        assert native["returncode"] == 0, native["error"]
        science = guarded_verify(native, request, policy, "offline-nonempty")
        assert native["result"]["summary"]["selected_count"] > 0
        write(
            DOCS / "qualification.json",
            {
                "passed": True,
                "empty_edge_case": edge,
                "native": resume.compact(native),
                "science": science,
                "native_capture": "original native-only capture; never includes reference-added ranges",
                "production_scientific_code_changed": False,
            },
        )
        print("both references PASS", native["result"]["summary"], flush=True)
    finally:
        archive.close()


def resume():
    import transfer_resume as recovery

    assert json.loads((DOCS / "qualification.json").read_text())["passed"]
    recovery.WORK = WORK / "native"
    recovery.WORK.mkdir(exist_ok=True)
    policy = json.loads((DOCS / "policy.json").read_text())

    class HeldOutArchive(recovery.Archive):
        def hook(self, handler, role, path, respond):
            if (
                self.mode == "provisional-gate"
                and role == "resource"
                and handler.command == "HEAD"
                and self.fixture.counts[(role, "HEAD")] >= 5
            ):
                time.sleep(0.1)  # keep final validation pending until SIGTERM
            return super().hook(handler, role, path, respond)

    archive = HeldOutArchive(Mirror(ORIGINAL_MIRROR), delay=0.003)
    request = json.loads((DOCS / "request.json").read_text())
    request["source"]["assets"][0].update(
        locator=archive.fixture.canonical, etag=archive.fixture.etag
    )
    rows = []

    def run(
        name,
        checkpoint=None,
        mode="success",
        stop=None,
        req=None,
        pol=None,
        success=True,
    ):
        archive.reset(mode)
        result = recovery.run(
            name,
            req or request,
            archive,
            checkpoint,
            policy=pol or policy,
            metadata="page16k",
            stop=stop,
        )
        write(WORK / (name + "-result.json"), result)
        assert (result["returncode"] == 0) == success, (name, result.get("error"))
        assert result["completion_emitted"] == success
        if success:
            result["science"] = guarded_verify(
                result, req or request, pol or policy, name
            )
        else:
            consumer = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "examples/consume_swot_ndjson.py"),
                    str(Path(result["output_dir"]) / "stream.ndjson"),
                ],
                capture_output=True,
            )
            assert consumer.returncode != 0 and not consumer.stdout
        rows.append(
            {
                "case": name,
                **recovery.compact(result),
                **({"science": result["science"]} if success else {}),
            }
        )
        print(name, "PASS", flush=True)
        return result

    try:
        normal = run("held-uninterrupted")
        checkpoint = recovery.private_dir("held-source-ranges")
        first = run(
            "held-interrupted", checkpoint, "science-gate", "gate", success=False
        )
        assert first["interruption_sent"]
        retained_after_stop = sorted(p.name for p in checkpoint.rglob("range-*"))
        assert retained_after_stop
        resumed = run("held-resumed-fresh-session", checkpoint)
        replayed = run("held-complete-checkpoint", checkpoint)
        assert replayed["result"]["observed"]["transport"]["science"]["bytes"] == 8
        # Fresh caller sessions log in again, including complete reconstruction.
        for r in [resumed, replayed]:
            assert (
                r["result"]["observed"]["transport"]["science"]["authenticated_http"][
                    "login_requests"
                ]
                == 1
            )
        archive.fixture.etag = '"changed-generation"'
        run("held-mutated-before-replay", checkpoint, success=False)
        archive.fixture.etag = Mirror(ORIGINAL_MIRROR).info["etag"]
        mutated = run(
            "held-mutated-final-replay", checkpoint, "mutate-final", success=False
        )
        assert mutated["tiles_received"] > 0
        changed = json.loads(json.dumps(request))
        changed["spatial"]["start"][0] += 1
        run("held-incompatible-selection", checkpoint, req=changed, success=False)
        run(
            "held-incompatible-policy",
            checkpoint,
            pol=recovery.q.POLICIES["good"],
            success=False,
        )
        repeated_cp = recovery.private_dir("held-provisional")
        stopped = run(
            "held-provisional-interruption",
            repeated_cp,
            "provisional-gate",
            "tile",
            success=False,
        )
        assert stopped["tiles_received"] > 0
        repeated = run("held-provisional-redelivery", repeated_cp)
        old_tile = next(
            json.loads(s)
            for s in (Path(stopped["output_dir"]) / "stream.ndjson")
            .read_text()
            .splitlines()
            if json.loads(s)["type"] == "tile"
        )
        new_tile = repeated["result"]["tiles"][0]
        assert {k: v for k, v in old_tile.items() if k != "seconds"} == {
            k: v for k, v in new_tile.items() if k != "seconds"
        }
        checkpointed = run(
            "held-uninterrupted-checkpoint",
            recovery.private_dir("held-checkpoint-control"),
        )
        stats = resumed["result"]["observed"]["transport"]["science"]
        write(
            DOCS / "resume.json",
            {
                "passed": True,
                "cases": rows,
                "retained_ranges_after_interruption": retained_after_stop,
                "normal_new_body_bytes": normal["result"]["observed"]["transport"][
                    "science"
                ]["bytes"],
                "resume_new_body_bytes": stats["bytes"],
                "resume_checkpoint": stats["checkpoint"],
                "uninterrupted_checkpoint": checkpointed["result"]["observed"][
                    "transport"
                ]["science"]["checkpoint"],
                "redelivered_first_tile_equal": True,
                "semantics": "scientific and consumer calculations restart at zero; source ranges only; finish and process completion required",
            },
        )
    finally:
        archive.close()


def live():
    import transfer_resume as recovery

    assert json.loads((DOCS / "qualification.json").read_text())["passed"]
    assert json.loads((DOCS / "resume.json").read_text())["passed"]
    recovery.WORK = WORK / "native"
    request = json.loads((DOCS / "request.json").read_text())
    policy = json.loads((DOCS / "policy.json").read_text())
    bounds = {
        "invocations": 3,
        "checkpoint_logical_bytes_per_invocation": 1024 * 1024,
        "expected_combined_new_source_bytes_cap": 2 * 1024 * 1024,
        "basis": "already verified native capture and page16k reads; range persistence fails at 1 MiB; no provider reference capture",
    }
    write(DOCS / "live-budget.json", bounds)
    checkpoint = recovery.private_dir("held-live")
    first = recovery.run(
        "held-live-interrupted",
        request,
        checkpoint=checkpoint,
        policy=policy,
        metadata="page16k",
        stop="prepared",
        live=True,
        extra=["--checkpoint-mib", "1"],
    )
    write(WORK / "held-live-interrupted-result.json", first)
    assert (
        first["interruption_sent"]
        and not first["completion_emitted"]
        and first["returncode"] != 0
    )
    retained = [p.name for p in checkpoint.rglob("range-*")]
    assert retained
    rows = [{"case": "interrupted", **recovery.compact(first)}]
    for name in ["resumed", "complete-replay"]:
        native = recovery.run(
            "held-live-" + name,
            request,
            checkpoint=checkpoint,
            policy=policy,
            metadata="page16k",
            live=True,
            extra=["--checkpoint-mib", "1"],
        )
        write(WORK / ("held-live-" + name + "-result.json"), native)
        assert native["returncode"] == 0, native.get("error")
        science = guarded_verify(native, request, policy, "held-live-" + name)
        if name == "complete-replay":
            assert native["result"]["observed"]["transport"]["science"]["bytes"] == 8
        rows.append({"case": name, **recovery.compact(native), "science": science})
        print("live", name, "PASS", flush=True)
    write(
        DOCS / "live.json",
        {
            "passed": True,
            "bounds": bounds,
            "cases": rows,
            "retained_ranges_after_interruption": retained,
            "source": {
                k: Mirror(ORIGINAL_MIRROR).info[k] for k in ["url", "etag", "size"]
            },
        },
    )


def resources():
    import transfer_resume as recovery

    recovery.WORK = WORK / "native"
    archive = recovery.Archive(Mirror(ORIGINAL_MIRROR))
    request = json.loads(
        (ORIGINAL / "prototype-output/transfer-resume/held-out-stage.json").read_text()
    )["request"]
    request["source"]["assets"][0].update(
        locator=archive.fixture.canonical, etag=archive.fixture.etag
    )
    policy = json.loads((DOCS / "policy.json").read_text())
    checkpoint = recovery.private_dir("held-memory")
    rows = []
    try:
        for size in [32 * 1024 * 1024, None, 64 * 1024 * 1024, 128 * 1024 * 1024]:
            if size is None:
                size = rows[0]["working_memory"]["estimated_sufficient_buffer_bytes"]
            request["budget"]["buffer_bytes"] = size
            archive.reset()
            result = recovery.run(
                "memory-" + str(size),
                request,
                archive,
                checkpoint,
                policy=policy,
                metadata="page16k",
            )
            lines = [
                json.loads(v)
                for v in (Path(result["output_dir"]) / "stream.ndjson")
                .read_text()
                .splitlines()
            ]
            info = next(v["inspection"] for v in lines if v["type"] == "prepared")
            memory = info["aligned_execution"]["working_memory"]
            write(WORK / ("memory-" + str(size) + "-result.json"), result)
            assert (result["returncode"] == 0) == (size != 32 * 1024 * 1024), (
                result.get("error")
            )
            if result["returncode"] == 0:
                result["science"] = guarded_verify(
                    result, request, policy, "memory-" + str(size)
                )
            else:
                assert "estimated sufficient buffer_bytes=" in result["error"]
                assert not result["tiles_received"] and not result["completion_emitted"]
            rows.append(
                {
                    "working_memory": memory,
                    **recovery.compact(result),
                    **({"science": result["science"]} if "science" in result else {}),
                }
            )
            print(
                "memory",
                size,
                result["returncode"],
                "estimate",
                memory["estimated_sufficient_buffer_bytes"],
                flush=True,
            )
        write(
            DOCS / "resources.json",
            {
                "passed": True,
                "cases": rows,
                "working_budget_is_rss_ceiling": False,
                "requested_budget_automatically_increased": False,
                "authentication_reservation_bytes": 1048576,
                "checkpoint_reservation_bytes": rows[0]["working_memory"][
                    "authentication_checkpoint_reservation_bytes"
                ]
                - 1048576,
            },
        )
    finally:
        archive.close()


def faults():
    """Reuse the existing adversarial gate without replacing its old receipts."""
    import transfer_resume as recovery

    recovery.WORK = WORK / "fault-regression"
    recovery.WORK.mkdir(exist_ok=True)
    recovery.q.WORK = recovery.WORK
    recovery.DOCS = recovery.WORK / "receipts"
    recovery.DOCS.mkdir(exist_ok=True)
    recovery.fixtures()
    value = json.loads((recovery.DOCS / "fixtures.json").read_text())
    write(DOCS / "fault-regression.json", value)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command",
        choices=[
            "preserve",
            "diagnose",
            "reference-regression",
            "complete-reference",
            "select",
            "qualify",
            "verify-result",
            "resume",
            "live",
            "resources",
            "faults",
        ],
    )
    parser.add_argument("paths", nargs="*")
    globals()[parser.parse_args().command.replace("-", "_")]()
