"""Normal-CLI authenticated transfer recovery, local HTTPS faults and pinned science.

fixtures and matrix never contact a public provider. live is one explicit
interruption/resume acceptance against the already qualified NASA product.
"""

import argparse
import base64
import hashlib
import json
import os
import resource
import signal
import subprocess
import tempfile
import threading
import time
from collections import Counter
from pathlib import Path

import psutil

import earthdata_auth as auth
import swot_quality as q
from hdf5_selective_mirror import Mirror

ROOT = q.ROOT
WORK = ROOT / "prototype-output/transfer-resume"
DOCS = ROOT / "docs/transfer-resume"
BINARY = ROOT / "target/release/tile-ripper"
WORK.mkdir(parents=True, exist_ok=True)
DOCS.mkdir(parents=True, exist_ok=True)
q.WORK = WORK


def save(name, value):
    text = json.dumps(value, indent=2, allow_nan=False) + "\n"
    assert all(s not in text for s in auth.SECRET_MARKERS)
    (DOCS / f"{name}.json").write_text(text)


class SparseBytes:
    def __init__(self, mirror):
        self.mirror = mirror

    def __len__(self):
        return self.mirror.info["size"]

    def __getitem__(self, selection):
        assert selection.step is None
        return self.mirror.read(selection.start, selection.stop - selection.start)


class Archive:
    def __init__(self, mirror=None, delay=0):
        self.mode = "success"
        self.delay = delay
        self.payload_requests = 0
        self.ranges = []
        self.block = threading.Event()
        self.release = threading.Event()
        self.fixture = auth.Fixture(
            self.hook, directory=tempfile.mkdtemp(prefix="https-", dir=WORK)
        )
        if mirror:
            self.fixture.bytes = SparseBytes(mirror)
            self.fixture.etag = mirror.info["etag"]

    def reset(self, mode="success"):
        self.mode = mode
        self.fixture.case = "success"
        self.fixture.events.clear()
        self.fixture.counts.clear()
        self.ranges.clear()
        self.payload_requests = 0
        self.block.clear()
        self.release.clear()

    def hook(self, handler, role, path, respond):
        f = self.fixture
        if self.mode == "no-auth" and role == "login":
            respond(401)
            return True
        if role != "resource":
            return False
        if (
            self.mode == "mutate-final"
            and handler.command == "HEAD"
            and f.counts[(role, "HEAD")] >= f.binding_heads + 3
        ):
            respond(
                200, {"ETag": '"new-generation"', "Content-Length": str(len(f.bytes))}
            )
            return True
        if handler.command != "GET":
            return False
        if self.mode == "retry-exhausted":
            respond(503, {"Retry-After": "0"})
            return True
        a, b = map(int, handler.headers["Range"].removeprefix("bytes=").split("-"))
        science = f.counts[(role, "HEAD")] >= f.binding_heads + 2
        if science:
            self.payload_requests += 1
        self.ranges.append({"start": a, "length": b - a + 1, "science": science})
        time.sleep(self.delay)
        gated = science and (
            self.mode == "partial"
            and self.payload_requests == 2
            or self.mode == "science-gate"
            and self.payload_requests == 3
        )
        if gated:
            data = f.bytes[a : b + 1]
            prefix = data[: max(1, len(data) // 2)] if self.mode == "partial" else b""
            handler.send_response(206)
            for k, v in {
                "ETag": f.etag,
                "Content-Length": str(len(data)),
                "Content-Range": f"bytes {a}-{b}/{len(f.bytes)}",
            }.items():
                handler.send_header(k, v)
            handler.end_headers()
            handler.wfile.write(prefix)
            handler.wfile.flush()
            f.events.append(
                {
                    "role": role,
                    "method": "GET",
                    "status": 206,
                    "body_bytes": len(prefix),
                    "partial_attempt": True,
                }
            )
            self.block.set()
            self.release.wait(15)
            return True
        return False

    def close(self):
        self.release.set()
        self.fixture.close()


def run(
    name,
    request,
    archive=None,
    checkpoint=None,
    command="stream",
    policy=None,
    metadata="exact",
    stop=None,
    extra=(),
    live=False,
    authenticate=True,
):
    out = WORK / name
    out.mkdir(parents=True, exist_ok=True)
    policy = policy or q.POLICIES["cookbook"]
    (out / "request.json").write_text(json.dumps(request))
    (out / "policy.json").write_text(json.dumps(policy))
    args = [
        str(BINARY),
        command,
        str(out / "request.json"),
        "--quality-policy",
        str(out / "policy.json"),
    ]
    if command == "stream":
        args.append("--values")
    if live:
        args += ["--auth", "earthdata", "--netrc", str(Path.home() / ".netrc")]
    elif archive and authenticate:
        f = archive.fixture
        args += [
            "--auth",
            "configured",
            "--netrc",
            str(f.netrc),
            "--auth-policy",
            str(f.config_path),
            "--auth-ca",
            str(f.cert),
        ]
    if checkpoint:
        args += ["--checkpoint", str(checkpoint)]
    args += list(extra)
    env = os.environ.copy()
    env.update(
        TILE_RIPPER_HDF5_METADATA_POLICY=metadata,
        TILE_RIPPER_HDF5_INDEX_POLICY="full",
        TILE_RIPPER_WORKERS="1",
        TILE_RIPPER_PER_HOST="1",
    )
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    binary_sha256 = hashlib.sha256(BINARY.read_bytes()).hexdigest()
    clock = time.perf_counter()
    lines, first, prepared = [], [], []
    peak = 0
    sent = False
    with (out / "stderr.txt").open("wb") as error:
        p = subprocess.Popen(
            args, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=error, text=True
        )

        def read():
            nonlocal sent
            if command == "plan":
                text = p.stdout.read()
                if text.strip():
                    lines.append(json.loads(text))
                return
            for line in p.stdout:
                value = json.loads(line)
                lines.append(value)
                if value.get("type") == "tile" and not first:
                    first.append(time.perf_counter() - clock)
                    if stop == "tile":
                        sent = True
                        p.send_signal(signal.SIGTERM)
                if value.get("type") == "prepared":
                    prepared.append(time.perf_counter() - clock)
                    if stop == "prepared":
                        sent = True
                        p.send_signal(signal.SIGTERM)

        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        while p.poll() is None:
            try:
                peak = max(peak, psutil.Process(p.pid).memory_info().rss)
            except psutil.Error:
                pass
            if stop == "gate" and archive.block.is_set() and not sent:
                sent = True
                p.send_signal(signal.SIGTERM)
            if time.perf_counter() - clock > 150:
                p.kill()
                raise TimeoutError("bounded native acquisition timed out")
            time.sleep(0.002)
        reader.join(5)
    if archive:
        archive.release.set()
    elapsed = time.perf_counter() - clock
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    error = (out / "stderr.txt").read_text()
    assert all(s not in error for s in auth.SECRET_MARKERS)
    (out / "stream.ndjson").write_text("".join(json.dumps(v) + "\n" for v in lines))
    complete = [v for v in lines if v.get("type") == "complete"]
    if command == "plan":
        inspection = lines[0] if lines else None
    else:
        inspection = next(
            (v["inspection"] for v in lines if v.get("type") == "prepared"), None
        )
    result = {
        "binary_sha256": binary_sha256,
        "returncode": p.returncode,
        "elapsed_seconds": elapsed,
        "cpu_seconds": after.ru_utime
        + after.ru_stime
        - before.ru_utime
        - before.ru_stime,
        "sampled_peak_rss_bytes": peak,
        "first_chunk_seconds": first[0] if first else None,
        "prepared_seconds": prepared[0] if prepared else None,
        "tiles_received": sum(v.get("type") == "tile" for v in lines),
        "completion_emitted": bool(complete),
        "interruption_sent": sent,
        "output_dir": str(out),
        "error": error[:1800] if p.returncode else None,
    }
    if complete:
        result["result"] = {
            **complete[0],
            "inspection": inspection,
            "tiles": [v for v in lines if v.get("type") == "tile"],
        }
        for i, tile in enumerate(result["result"]["tiles"]):
            (out / f"{i}.validity").write_bytes(bytes(tile["validity"]))
            for j, layer in enumerate(tile["layers"]):
                assert layer["variable"] == {
                    "kind": "dataset",
                    "path": "/" + q.PATHS[j],
                }
                assert layer["encoding"] == "base64-little-endian"
                (out / f"{i}-{j}.bin").write_bytes(
                    base64.b64decode(layer["values"], validate=True)
                )
                (out / f"{i}-{j}.mask").write_bytes(
                    base64.b64decode(layer["source_mask"], validate=True)
                )
    if archive:
        result["server"] = {
            "requests": dict(
                Counter(
                    f"{e['role']} {e['method']} {e['status']}"
                    for e in archive.fixture.events
                )
            ),
            "response_body_bytes": sum(e["body_bytes"] for e in archive.fixture.events),
            "ranges": list(archive.ranges),
            "route_generation": archive.fixture.route_generation,
        }
    return result


def compact(result):
    out = {k: v for k, v in result.items() if k not in ["result", "output_dir"]}
    if "server" in out:
        server = dict(out["server"])
        ranges = server.pop("ranges", [])
        server["range_requests"] = len(ranges)
        server["range_trace_sha256"] = hashlib.sha256(
            json.dumps(ranges, sort_keys=True).encode()
        ).hexdigest()
        out["server"] = server
    if "result" in result:
        r = result["result"]
        t = dict(r["observed"]["transport"]["science"])
        ranges = t.pop("ranges", {})
        t["range_trace_sha256"] = hashlib.sha256(
            json.dumps(ranges, sort_keys=True).encode()
        ).hexdigest()
        t["range_identities"] = len(ranges)
        out.update(
            summary=r["summary"], transport=t, scheduler=r["observed"]["scheduler"]
        )
    return out


def verify(result, request, source, policy, name):
    result["science"] = q.verify(result, request, source, policy, name)
    consumer = subprocess.run(
        [
            os.sys.executable,
            str(ROOT / "examples/consume_swot_ndjson.py"),
            str(Path(result["output_dir"]) / "stream.ndjson"),
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    assert json.loads(consumer.stdout) == result["result"]["summary"]
    result["science"]["fresh_consumer_calculation_equal"] = True
    return result


def private_dir(name):
    return Path(tempfile.mkdtemp(prefix=name + "-", dir=WORK))


def fixtures():
    archive = Archive(delay=0.002)
    rows = []
    req = q.request(archive.fixture.canonical)
    req["spatial"].update(start=[3, 4], count=[17, 19])
    source = ROOT / "tests/data/swot/quality-le.h5"
    policy = q.POLICIES["cookbook"]

    def record(name, result, success=False):
        assert (result["returncode"] == 0) == success, (name, result.get("error"))
        assert result["completion_emitted"] == success, name
        if not success and result["tiles_received"]:
            consumer = subprocess.run(
                [
                    os.sys.executable,
                    str(ROOT / "examples/consume_swot_ndjson.py"),
                    str(Path(result["output_dir"]) / "stream.ndjson"),
                ],
                capture_output=True,
                text=True,
            )
            assert consumer.returncode != 0 and not consumer.stdout
        if success:
            verify(result, req, source, policy, name)
        rows.append({"case": name, **compact(result)})
        print(name, "PASS", flush=True)

    try:
        archive.reset()
        with auth.environment(
            {
                "TILE_RIPPER_AUTH_NETRC": archive.fixture.netrc,
                "NETRC": archive.fixture.netrc,
            }
        ):
            anonymous = run(
                "anonymous-ignores-credential-environment",
                req,
                archive,
                authenticate=False,
            )
        record("anonymous-ignores-credential-environment", anonymous)
        assert not any(e["role"] == "login" for e in archive.fixture.events)
        checkpoint = private_dir("fixture-checkpoint")
        archive.reset()
        record("uninterrupted", run("uninterrupted", req, archive), True)
        archive.reset()
        plan = run("plan-with-checkpoint", req, archive, checkpoint, command="plan")
        assert plan["returncode"] == 0 and not plan["completion_emitted"]
        rows.append({"case": "after-metadata-planning", **compact(plan)})
        archive.reset()
        result = run("resume-after-metadata", req, archive, checkpoint)
        record("resume-after-metadata", result, True)
        assert (
            result["result"]["observed"]["transport"]["science"]["checkpoint"][
                "counts"
            ]["reused_bytes"]
            > 0
        )
        # A fully populated checkpoint must still authenticate a fresh GET.
        for mode, success in [("no-auth", False), ("success", True)]:
            archive.reset(mode)
            record(
                "complete-checkpoint-" + mode,
                run("complete-checkpoint-" + mode, req, archive, checkpoint),
                success,
            )
        archive.reset()
        archive.fixture.case = "expired-once"
        record(
            "fresh-signed-route",
            run("fresh-signed-route", req, archive, checkpoint),
            True,
        )
        archive.reset("mutate-final")
        mutation = run("mutation-after-replay", req, archive, checkpoint)
        record("mutation-after-replay", mutation)
        assert mutation["tiles_received"] > 0
        for mode in ["partial", "science-gate", "success"]:
            cp = private_dir("interrupted-" + mode)
            archive.reset(mode)
            stopped = run(
                "interrupted-" + mode,
                req,
                archive,
                cp,
                stop="gate" if mode != "success" else "tile",
            )
            record("interrupted-" + mode, stopped)
            assert stopped["interruption_sent"]
            archive.reset()
            resumed = run("resumed-" + mode, req, archive, cp)
            record("resumed-" + mode, resumed, True)
            if mode == "success":
                assert stopped["tiles_received"] > 0
                # Consumer starts from zero again; the first tile is redelivered.
                old = json.loads(
                    (Path(stopped["output_dir"]) / "stream.ndjson")
                    .read_text()
                    .splitlines()[1]
                )
                new = resumed["result"]["tiles"][0]
                assert (
                    old["layers"] == new["layers"]
                    and old["validity"] == new["validity"]
                )
        # No useful ranges exist when initial login fails.
        cp = private_dir("initial-auth-failed")
        archive.reset("no-auth")
        record(
            "initial-auth-no-ranges", run("initial-auth-no-ranges", req, archive, cp)
        )
        assert not list(cp.rglob("range-*"))
        archive.reset()
        record(
            "resume-after-auth-failure",
            run("resume-after-auth-failure", req, archive, cp),
            True,
        )
        for change in [
            "selection",
            "stride",
            "coordinates",
            "quality",
            "etag",
            "length",
            "access",
        ]:
            changed = json.loads(json.dumps(req))
            changed_policy = policy
            original_etag = archive.fixture.etag
            original_bytes = archive.fixture.bytes
            config = json.loads(archive.fixture.config_path.read_text())
            if change == "selection":
                changed["spatial"]["count"][0] -= 1
            if change == "stride":
                changed["spatial"]["step"][0] = 2
            if change == "coordinates":
                changed["spatial"]["axes"] = ["/x", "/y"]
            if change == "quality":
                changed_policy = q.POLICIES["good"]
            if change == "etag":
                archive.fixture.etag = '"different-version"'
            if change == "length":
                archive.fixture.bytes += b"x"
            if change == "access":
                altered = dict(config)
                # Change a policy edge that is not used by fresh cached replay.
                altered["redirects"] = config["redirects"] + [
                    {
                        "from_origin": archive.fixture.login,
                        "to_origin": archive.fixture.archive,
                        "destination_path": "/unused",
                    }
                ]
                archive.fixture.config_path.write_text(json.dumps(altered))
            archive.reset()
            result = run(
                "changed-" + change, changed, archive, checkpoint, policy=changed_policy
            )
            record("changed-" + change, result)
            assert "mismatch" in result["error"], result["error"]
            archive.fixture.etag, archive.fixture.bytes = original_etag, original_bytes
            archive.fixture.config_path.write_text(json.dumps(config))
        for case in ["truncated", "resource-429", "refresh-exhausted"]:
            cp = private_dir("transport-" + case)
            archive.reset()
            archive.fixture.case = case
            if case == "resource-429":
                # Bounded retry recovery must still yield independent equality.
                record(case, run(case, req, archive, cp), True)
            else:
                record(case, run(case, req, archive, cp))
        for limit, extra in [("entries", ["--checkpoint-entries", "1"])]:
            cp = private_dir("limit-" + limit)
            archive.reset()
            record(
                "limit-" + limit, run("limit-" + limit, req, archive, cp, extra=extra)
            )
        archive.reset("retry-exhausted")
        retry = run("retry-exhausted", req, archive, private_dir("retry-exhausted"))
        record("retry-exhausted", retry)
        assert retry["server"]["requests"]["resource GET 503"] == 3
        small = json.loads(json.dumps(req))
        small["budget"]["buffer_bytes"] = 1024 * 1024
        archive.reset()
        record(
            "small-memory",
            run("small-memory", small, archive, private_dir("small-memory")),
        )
        assert not archive.fixture.events, (
            "small working budget must fail before source access"
        )
        for label in ["cookbook", "bitwise"]:
            strided = json.loads(json.dumps(req))
            strided["spatial"].update(start=[1, 2], count=[12, 10], step=[3, 5])
            cp = private_dir("strided-" + label)
            archive.reset("partial")
            interrupted = run(
                "strided-" + label + "-first",
                strided,
                archive,
                cp,
                policy=q.POLICIES[label],
                stop="gate",
            )
            assert (
                interrupted["interruption_sent"]
                and not interrupted["completion_emitted"]
            )
            archive.reset()
            resumed = run(
                "strided-" + label + "-resume",
                strided,
                archive,
                cp,
                policy=q.POLICIES[label],
            )
            verify(
                resumed,
                strided,
                source,
                q.POLICIES[label],
                "strided-" + label + "-resume",
            )
            rows.append(
                {
                    "case": "strided-" + label,
                    "interrupted": compact(interrupted),
                    "resumed": compact(resumed),
                }
            )
        # A worker/page/tile tuning change preserves identity, with exact misses.
        tuned = json.loads(json.dumps(req))
        tuned["budget"]["chunk_edge"] = 7
        archive.reset()
        tuned_result = run(
            "execution-tuning",
            tuned,
            archive,
            checkpoint,
            metadata="page4k",
            extra=["--workers", "2"],
        )
        verify(tuned_result, tuned, source, policy, "execution-tuning")
        assert tuned_result["returncode"] == 0
        rows.append({"case": "execution-tuning", **compact(tuned_result)})
        for path in checkpoint.rglob("*"):
            if path.is_file():
                data = path.read_bytes()
                assert all(s.encode() not in data for s in auth.SECRET_MARKERS)
                assert b"Authorization" not in data and b"Signature=" not in data
        save(
            "fixtures",
            {
                "passed": len(rows),
                "cases": rows,
                "public_provider_requests": 0,
                "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
            },
        )
    finally:
        archive.close()


def matrix():
    mirror = Mirror(ROOT / "prototype-output/swot-quality/mirror")
    mirror.verify()
    rows = []
    archive = Archive(mirror, delay=0.003)
    try:
        for metadata in ["exact", "page4k", "page16k"]:
            for repeat in range(3):
                stem = f"matrix-{metadata}-{repeat}"
                req = q.request(archive.fixture.canonical)
                cp = private_dir(stem + "-checkpoint")
                archive.reset()
                normal = run(stem + "-normal", req, archive, metadata=metadata)
                verify(
                    normal, req, mirror.path, q.POLICIES["cookbook"], stem + "-normal"
                )
                archive.reset()
                checkpointed = run(
                    stem + "-checkpointed",
                    req,
                    archive,
                    private_dir(stem + "-fresh"),
                    metadata=metadata,
                )
                verify(
                    checkpointed,
                    req,
                    mirror.path,
                    q.POLICIES["cookbook"],
                    stem + "-checkpointed",
                )
                archive.reset("partial")
                first = run(
                    stem + "-first", req, archive, cp, metadata=metadata, stop="gate"
                )
                assert first["interruption_sent"] and not first["completion_emitted"]
                archive.reset()
                resume = run(stem + "-resume", req, archive, cp, metadata=metadata)
                verify(
                    resume, req, mirror.path, q.POLICIES["cookbook"], stem + "-resume"
                )
                archive.reset()
                restart = run(stem + "-restart", req, archive, metadata=metadata)
                verify(
                    restart, req, mirror.path, q.POLICIES["cookbook"], stem + "-restart"
                )
                rows.append(
                    {
                        "metadata": metadata,
                        "repeat": repeat,
                        "uninterrupted": compact(normal),
                        "uninterrupted_checkpointed": compact(checkpointed),
                        "interrupted": compact(first),
                        "resume": compact(resume),
                        "restart": compact(restart),
                        "combined_resume_seconds": first["elapsed_seconds"]
                        + resume["elapsed_seconds"],
                        "combined_restart_seconds": first["elapsed_seconds"]
                        + restart["elapsed_seconds"],
                    }
                )
                print(stem, "PASS", flush=True)
            cp = private_dir("matrix-auth-control-" + metadata)
            archive.reset("no-auth")
            failed_auth = run(
                "matrix-auth-failed-" + metadata, req, archive, cp, metadata=metadata
            )
            assert failed_auth["returncode"] != 0 and not list(cp.rglob("range-*"))
            archive.reset()
            fresh = run(
                "matrix-auth-resume-" + metadata, req, archive, cp, metadata=metadata
            )
            verify(
                fresh,
                req,
                mirror.path,
                q.POLICIES["cookbook"],
                "matrix-auth-resume-" + metadata,
            )
            assert (
                fresh["result"]["observed"]["transport"]["science"]["checkpoint"][
                    "counts"
                ]["retained_bytes_reused"]
                == 0
            )
            rows.append(
                {
                    "metadata": metadata,
                    "control": "initial authentication failed; no completed ranges",
                    "failed_auth": compact(failed_auth),
                    "fresh_resume": compact(fresh),
                }
            )
        save(
            "performance",
            {
                "cases": rows,
                "delay_per_source_get_seconds": 0.003,
                "source": mirror.info["url"],
                "etag": mirror.info["etag"],
                "source_size": mirror.info["size"],
                "public_provider_requests": 0,
                "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
                "scope": "local HTTPS, three repeats per metadata strategy; sampled RSS excludes fixture/reference processes",
            },
        )
    finally:
        archive.close()


def live():
    mirror = Mirror(ROOT / "prototype-output/swot-quality/mirror")
    mirror.verify()
    req = q.request(mirror.info["url"])
    req["source"]["assets"][0]["etag"] = mirror.info["etag"]
    cp = private_dir("live-checkpoint")
    first = run(
        "live-first", req, checkpoint=cp, metadata="page16k", stop="prepared", live=True
    )
    assert first["interruption_sent"] and not first["completion_emitted"]
    resumed = run("live-resumed", req, checkpoint=cp, metadata="page16k", live=True)
    verify(resumed, req, mirror.path, q.POLICIES["cookbook"], "live-resumed")
    save(
        "live",
        {
            "interrupted": compact(first),
            "resumed": compact(resumed),
            "science": resumed["science"],
            "source": {k: mirror.info[k] for k in ["url", "etag", "size"]},
            "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
        },
    )


def held_out():
    """Verify the separately selected granule's bounded native capture.

    Stage files retain the explicit live plan/32 MiB failure/128 MiB stream.
    This command performs no provider requests and has no recapture fallback.
    """
    from hdf5_selective_mirror import server

    stage = json.loads((WORK / "held-out-stage.json").read_text())
    failed = json.loads((WORK / "held-out-stream-result.json").read_text())
    native = json.loads((WORK / "held-out-stream-128mib-result.json").read_text())
    assert failed["returncode"] != 0 and "working budget" in failed["error"]
    assert native["returncode"] == 0
    request = stage["request"]
    request["budget"]["buffer_bytes"] = 128 * 1024 * 1024
    checkpoint = Path(stage["checkpoint_directory"]) / "tile-ripper-checkpoint-v1"
    identity = json.loads((checkpoint / "identity.json").read_text())
    target = WORK / "held-out-mirror"
    target.mkdir(exist_ok=True)
    extents = []
    with (target / "source.nc").open("w+b") as destination:
        destination.truncate(identity["object_length"])
        for entry in sorted(checkpoint.glob("range-*")):
            data = entry.read_bytes()
            start = int.from_bytes(data[8:16], "little")
            length = int.from_bytes(data[16:24], "little")
            body = data[88:]
            assert len(body) == length and data[56:88] == hashlib.sha256(body).digest()
            assert (
                data[24:56]
                == hashlib.sha256((checkpoint / "identity.json").read_bytes()).digest()
            )
            destination.seek(start)
            destination.write(body)
            extents.append((start, start + length))
    merged = []
    for a, b in sorted(extents):
        if merged and a <= merged[-1][1]:
            merged[-1][1] = max(b, merged[-1][1])
        else:
            merged.append([a, b])
    info = {
        "url": identity["canonical"],
        "etag": identity["etag"],
        "size": identity["object_length"],
        "extents": merged,
        "provider_gets": 0,
        "provider_bytes": 0,
        "cap": 8 * 1024 * 1024,
        "extent_sha256": [],
    }
    with (target / "source.nc").open("rb") as source:
        for a, b in merged:
            source.seek(a)
            info["extent_sha256"].append(
                [a, b, hashlib.sha256(source.read(b - a)).hexdigest()]
            )
    (target / "manifest.json").write_text(json.dumps(info, indent=2) + "\n")
    mirror = Mirror(target)
    mirror.verify()
    with server(mirror) as srv:
        remote, _ = q.reference(
            request,
            q.POLICIES["cookbook"],
            None,
            "held-out-coverage",
            url=srv.url,
            page=0,
        )
        assert not srv.errors
    assert len(native["result"]["tiles"]) == 1
    tile = native["result"]["tiles"][0]
    reference_dir = WORK / "reference-held-out-coverage-h5py"
    import struct

    for j, layer in enumerate(tile["layers"]):
        assert (
            base64.b64decode(layer["values"])
            == (reference_dir / f"{j}.bin").read_bytes()
        )
        assert (
            base64.b64decode(layer["source_mask"])
            == (reference_dir / f"{j}.mask").read_bytes()
        )
        axes = layer["coordinates"]
        assert [
            hashlib.sha256(struct.pack("<" + str(len(a)) + "d", *a)).hexdigest()
            for a in axes[::-1]
        ] == remote["axis_sha256"]
    assert bytes(tile["validity"]) == (reference_dir / "validity.bin").read_bytes()
    assert native["result"]["summary"] == remote["summary"]
    netcdf_request = {
        "reader": "netcdf4",
        "spatial": request["spatial"],
        "policy": q.POLICIES["cookbook"],
        "local": str(mirror.path),
    }
    request_path = WORK / "held-out-netcdf-request.json"
    request_path.write_text(json.dumps(netcdf_request))
    netcdf = subprocess.run(
        [
            os.sys.executable,
            str(ROOT / "bench/swot_quality_reference.py"),
            str(request_path),
        ],
        capture_output=True,
        text=True,
        timeout=30,
    )
    save(
        "held-out",
        {
            "candidate": stage["candidate"],
            "passed": netcdf.returncode == 0,
            "initial_32mib_attempt": compact(failed),
            "native_128mib": compact(native),
            "h5py_equality": {
                "all_stored_bits": True,
                "all_source_masks": True,
                "all_coordinates": True,
                "derived_mask": True,
                "summary": True,
            },
            "netcdf_c_reference": {
                "returncode": netcdf.returncode,
                "stderr_present": bool(netcdf.stderr),
                "scope": "local incomplete sparse capture; abort cause not established; no full download or format-profile change to force a pass",
            },
            "h5py_capture_coverage": remote,
            "source": {k: info[k] for k in ["url", "etag", "size"]},
            "captured_unique_bytes": sum(b - a for a, b in merged),
            "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
            "configuration": "same public archive policy and 128x128 indices; larger caller memory budget; no production code changes for this product",
            "limit": "selected window has zero quality-accepted cells; all stored bits, masks and axes still compared; this does not qualify nonmissing values on this held-out granule",
        },
    )
    print(
        "held-out native completion and h5py equality PASS; NetCDF-C reference gate",
        netcdf.returncode,
        flush=True,
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["fixtures", "matrix", "live", "held-out"])
    globals()[parser.parse_args().command.replace("-", "_")]()
