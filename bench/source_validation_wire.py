"""Pinned anonymous HTTPS wire fixtures; Python stdlib only at execution time.

Generate fixtures with source_validation.py. On Linux SSL_CERT_FILE supplies the
private fixture trust anchor through the existing system TLS verifier. No TLS
verification bypass or production CA setting is added to Tile-Ripper.
"""

import copy
import hashlib
import json
import os
import random
import ssl
import subprocess
import sys
import threading
import time
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/source-validation"
DOCS = ROOT / "docs/source-validation"


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def measure(binary, value, label, env=None):
    directory = WORK / "runs" / label
    directory.mkdir(parents=True, exist_ok=True)
    request = directory / "input.json"
    write(request, value)
    stdout, stderr = directory / "stdout.json", directory / "stderr.txt"
    started = time.perf_counter()
    with stdout.open("wb") as out, stderr.open("wb") as err:
        p = subprocess.Popen(
            [str(binary), str(request)],
            stdout=out,
            stderr=err,
            env={
                **os.environ,
                "TILE_RIPPER_HDF5_METADATA_POLICY": "page16k",
                **(env or {}),
            },
        )
        _, status, usage = os.wait4(p.pid, 0)
        p.returncode = os.waitstatus_to_exitcode(status)
    row = {
        "label": label,
        "returncode": p.returncode,
        "wall_seconds": time.perf_counter() - started,
        "cpu_seconds": usage.ru_utime + usage.ru_stime,
        "peak_rss_bytes": usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024),
        "binary_sha256": digest(binary),
        "stdout_sha256": digest(stdout),
        "stderr_sha256": digest(stderr),
        "raw_directory": str(directory),
        "load_average": os.getloadavg(),
    }
    if p.returncode == 0:
        value = json.loads(stdout.read_text())
        row.update(
            {
                k: value[k]
                for k in ["prepare_seconds", "first_chunk_seconds", "seconds"]
                if k in value
            }
        )
    else:
        value = None
        row["error"] = stderr.read_text()[-1500:]
    return value, row


def accounting(events):
    counts = Counter(f"{e['role']} {e['method']} {e['status']}" for e in events)
    source = [e for e in events if e["role"] in ["resource", "anonymous"]]
    return {
        "requests": len(events),
        "canonical_head": sum(
            e["method"] == "HEAD" and e["role"] in ["archive", "anonymous"]
            for e in events
        ),
        "source_head": sum(e["method"] == "HEAD" for e in source),
        "conditional_get": sum(
            e["method"] == "GET" and bool(e.get("if_match")) for e in source
        ),
        "signed_redirects": sum(
            e["role"] == "archive" and e["status"] == 303 for e in events
        ),
        "redirect_hops": sum(e["status"] in [301, 302, 303, 307, 308] for e in events),
        "authentication_requests": sum(e["role"] == "login" for e in events),
        "source_body_bytes": sum(e["body_bytes"] for e in source),
        "metadata_body_bytes": sum(
            e["body_bytes"] for e in source if e.get("body_class") == "metadata"
        )
        if all("body_class" in e for e in source)
        else None,
        "payload_body_bytes": sum(
            e["body_bytes"] for e in source if e.get("body_class") == "payload"
        )
        if all("body_class" in e for e in source)
        else None,
        "status_counts": dict(counts),
    }


class Files(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, root, certificate):
        super().__init__(("127.0.0.1", 0), Handler)
        self.original = {
            "/" + p.relative_to(root).as_posix(): p.read_bytes()
            for p in Path(root).rglob("*")
            if p.is_file()
        }
        self.events = []
        self.delay = 0
        self.case = "success"
        self.target = None
        self.triggered = False
        self.prepared = False
        self.lock = threading.Lock()
        self.reset()
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(
            certificate, Path(certificate).with_name("fixture-key.pem")
        )
        self.socket = context.wrap_socket(self.socket, server_side=True)
        threading.Thread(target=self.serve_forever, daemon=True).start()

    @property
    def url(self):
        return f"https://127.0.0.1:{self.server_port}"

    def reset(self, delay=0, case="success", target=None):
        self.cache = self.original.copy()
        self.events.clear()
        self.delay, self.case, self.target = delay, case, target
        self.triggered = False
        self.prepared = False

    def mutate(self):
        if self.target in self.cache:
            data = self.cache[self.target]
            self.cache[self.target] = data + b"x"
        else:
            self.cache[self.target] = b"new object"
        self.triggered = True


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def do_HEAD(self):
        self.handle_object()

    def do_GET(self):
        self.handle_object()

    def handle_object(self):
        s = self.server
        if self.path.startswith("/control/"):
            stage = self.path.removeprefix("/control/")
            if stage == "restore":
                s.cache = s.original.copy()
            elif stage == s.case:
                s.mutate()
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        assert not self.headers.get("Authorization") and not self.headers.get("Cookie")
        data = s.cache.get(self.path)
        etag = None if data is None else '"' + hashlib.sha256(data).hexdigest() + '"'
        if s.case == "same-etag-size" and s.triggered and self.path in s.original:
            etag = '"' + hashlib.sha256(s.original[self.path]).hexdigest() + '"'
        condition, span = self.headers.get("If-Match"), self.headers.get("Range")
        status = 404 if data is None else 200
        if (
            condition
            and condition != etag
            or self.headers.get("If-None-Match") == "*"
            and data is not None
        ):
            status = 412
        start, end = 0, len(data or b"")
        if span and status == 200:
            start, last = map(int, span.removeprefix("bytes=").split("-"))
            end, status = last + 1, 206
        body = data[start:end] if status in [200, 206] else b""
        headers = {"Content-Length": str(end - start if status in [200, 206] else 0)}
        if etag:
            headers["ETag"] = ("W/" if s.case == "weak" else "") + etag
        if s.case == "missing-etag":
            headers.pop("ETag", None)
        if status == 206:
            headers["Content-Range"] = f"bytes {start}-{end - 1}/{len(data)}"
        if s.case == "retry-exhausted":
            status, body, headers = (
                503,
                b"",
                {"Content-Length": "0", "Retry-After": "0"},
            )
        time.sleep(s.delay)
        self.send_response(status)
        for key, value in headers.items():
            self.send_header(key, value)
        self.end_headers()
        sent = 0
        if self.command == "GET" and body:
            try:
                self.wfile.write(body)
                self.wfile.flush()
                sent = len(body)
            except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
                pass
        with s.lock:
            s.events.append(
                {
                    "role": "anonymous",
                    "path": self.path,
                    "method": self.command,
                    "status": status,
                    "range": span,
                    "if_match": condition,
                    "content_range": headers.get("Content-Range"),
                    "content_length": headers["Content-Length"],
                    "body_bytes": sent,
                    "body_class": "payload" if s.prepared else "metadata",
                }
            )
            if (
                self.command == "HEAD"
                and not s.triggered
                and s.case in ["bound", "same-etag-size"]
            ):
                if s.case == "bound":
                    s.target = self.path
                s.mutate()
            if (
                s.case == "payload-then-restore"
                and not s.triggered
                and self.command == "GET"
                and sum(e["method"] == "HEAD" for e in s.events) >= 4
            ):
                s.target = self.path
                s.mutate()


def classify(events, name, binding_heads):
    """Label fixture bodies by lifecycle/purpose, then reconcile with the reader.

    The pinned sharded fixture uses the existing 132-byte end index. Metadata page
    overfetch stays charged to metadata requests rather than selected science.
    """
    heads = 0
    for event in events:
        if event["method"] == "HEAD":
            heads += 1
        path = event["path"]
        if name == "hdf5":
            payload = heads >= binding_heads + 2
        elif name == "cog":
            payload = heads >= 4
        elif name == "zarr_v2":
            payload = (
                any(f"/{v}/" in path for v in ["red", "nir"]) and "/.z" not in path
            )
        else:
            payload = any(f"/{v}/c/" in path for v in ["signal", "temperature", "mask"])
            if payload and name.endswith("sharded") and event.get("range"):
                start, end = map(int, event["range"].removeprefix("bytes=").split("-"))
                total = (
                    int(event["content_range"].rsplit("/", 1)[1])
                    if event.get("content_range")
                    else 0
                )
                if end + 1 == total and total - start == 132:
                    payload = False
        event["body_class"] = "payload" if payload else "metadata"


def anonymous():
    configs = json.loads((WORK / "fixtures/requests.json").read_text())
    if selected := os.environ.get("TILE_RIPPER_VALIDATION_SOURCES"):
        configs = {name: configs[name] for name in selected.split(",")}
    certificate = WORK / "tls/fixture-cert.pem"
    s = Files(WORK / "fixtures/objects", certificate)
    binaries = {
        label: WORK / f"linux/{label}/source_validation_bench"
        for label in ["baseline", "candidate"]
    }
    rows, faults, expected = [], [], {}
    try:
        for name, config in configs.items():
            value = copy.deepcopy(config["input"])
            value["request"]["source"]["assets"][0]["locator"] = s.url + config["path"]
            smoke = bool(os.environ.get("TILE_RIPPER_VALIDATION_SMOKE"))
            for delay in [0] if smoke else [0, 0.025, 0.1]:
                for repeat in range(1 if smoke else 3):
                    order = list(binaries)
                    random.Random(20261023 + repeat).shuffle(order)
                    for label in order:
                        s.reset(delay)
                        result, row = measure(
                            binaries[label],
                            value,
                            f"linux-{name}-{delay}-{repeat}-{label}",
                            {"SSL_CERT_FILE": str(certificate)},
                        )
                        assert row["returncode"] == 0, row
                        fingerprint = result["chunks"]
                        assert expected.setdefault(name, fingerprint) == fingerprint
                        classify(s.events, name, 2 if label == "baseline" else 1)
                        traffic = accounting(s.events)
                        assert (
                            traffic["payload_body_bytes"]
                            == result["observed"]["source_data_bytes"]
                        ), (name, traffic)
                        row.update(
                            source=name,
                            delay_seconds=delay,
                            repeat=repeat,
                            implementation=label,
                            wire=traffic,
                            working_memory={
                                key: result["observed"]["scheduler"][key]
                                for key in [
                                    "buffer_bytes",
                                    "reserved_bytes_peak",
                                    "reader_state_reserved_bytes",
                                    "prepared_native_metadata_bytes",
                                    "prepared_v3_metadata_index_bytes",
                                    "prepared_hdf5_bytes",
                                    "memory_scope",
                                ]
                            },
                        )
                        rows.append(row)
                        if delay == 0 and repeat == 0:
                            write(DOCS / f"trace-{name}-{label}.json", s.events)
                    print(name, delay, repeat, "PASS", flush=True)
            cases = [
                ("bound", config["payload"]),
                ("prepared", config["payload"]),
                ("prepared", config["metadata"]),
                ("delivered", config["metadata"]),
                ("weak", config["metadata"]),
                ("missing-etag", config["metadata"]),
                ("retry-exhausted", config["metadata"]),
                ("same-etag-size", config["metadata"]),
            ]
            if name.startswith("zarr"):
                cases.append(("delivered", config["payload"]))
            for number, (case, target) in enumerate(cases):
                outcomes = {}
                for label, binary in binaries.items():
                    s.reset(case=case, target=target)
                    test = {
                        **value,
                        "control": s.url.replace("https:", "http:") + "/control",
                    }
                    # Control is a separate plaintext loopback server; object traffic stays HTTPS.
                    with Control(s) as control:
                        test["control"] = control.url
                        result, row = measure(
                            binary,
                            test,
                            f"fault-{name}-{number}-{label}",
                            {"SSL_CERT_FILE": str(certificate)},
                        )
                    outcomes[label] = {
                        "returncode": row["returncode"],
                        "error": row.get("error"),
                        "wire": accounting(s.events),
                    }
                # Provider lies about ETag on an already-read COG may escape its
                # legacy final ETag-only sweep. Compare explicitly, never count it as a new rejection.
                success = (
                    name == "cog"
                    and case == "same-etag-size"
                    or name == "zarr_v2"
                    and case == "delivered"
                    and target == config["payload"]
                )
                assert all(
                    (outcome["returncode"] == 0) == success
                    for outcome in outcomes.values()
                ), (name, case, success, outcomes)
                faults.append(
                    {
                        "source": name,
                        "case": case,
                        "target": target,
                        "expected_success": success,
                        "outcomes": outcomes,
                    }
                )
            for label, binary in binaries.items():
                s.reset(case="prepared", target=config["metadata"])
                with Control(s) as control:
                    result, row = measure(
                        binary,
                        {**value, "control": control.url, "sticky": True},
                        f"sticky-{name}-{label}",
                        {"SSL_CERT_FILE": str(certificate)},
                    )
                assert row["returncode"] == 0 and result["first_failed"]
                assert result["retry_failed"] == (label == "candidate"), (
                    name,
                    label,
                    result,
                )
                faults.append(
                    {
                        "source": name,
                        "case": "detected-conflict-then-restore",
                        "implementation": label,
                        "result": result,
                    }
                )
            if name == "cog":
                for label, binary in binaries.items():
                    s.reset(case="payload-then-restore", target=config["payload"])
                    with Control(s) as control:
                        result, row = measure(
                            binary,
                            {**value, "control": control.url, "sticky": True},
                            f"sticky-payload-{name}-{label}",
                            {"SSL_CERT_FILE": str(certificate)},
                        )
                    assert row["returncode"] == 0 and result["first_failed"]
                    assert result["retry_failed"] == (label == "candidate"), result
                    faults.append(
                        {
                            "source": name,
                            "case": "payload-conflict-then-restore",
                            "implementation": label,
                            "result": result,
                        }
                    )
        write(
            DOCS / "anonymous-benchmarks.json",
            {
                "runs": rows,
                "platform": sys.platform,
                "fixture_sha256": digest(WORK / "fixtures/requests.json"),
            },
        )
        write(
            DOCS / "anonymous-faults.json",
            {"cases": faults, "all_passed": True, "new_successful_fault_paths": 0},
        )
    finally:
        s.shutdown()
        s.server_close()


class Control:
    def __init__(self, objects):
        class Controller(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                stage = self.path.rsplit("/", 1)[-1]
                if stage == "prepared":
                    objects.prepared = True
                if stage == "restore":
                    objects.cache = objects.original.copy()
                elif stage == objects.case:
                    objects.mutate()
                self.send_response(200)
                self.send_header("Content-Length", "0")
                self.end_headers()

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Controller)
        self.url = f"http://127.0.0.1:{self.server.server_port}/control"

    def __enter__(self):
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        return self

    def __exit__(self, *_):
        self.server.shutdown()
        self.server.server_close()


if __name__ == "__main__":
    anonymous()
