"""Opt-in native Earthdata acceptance and offline adversarial HTTPS fixtures.

`fixtures` never contacts NASA. `live` performs one native 128x128 acquisition;
its two scientific reference readers use only the previously sealed mirror.
Credentials, cookies and signed URLs are never written by this harness.
"""

import argparse
import contextlib
import json
import os
import re
import ssl
import subprocess
import threading
import time
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

import swot_quality as q
from hdf5_selective_mirror import Mirror

ROOT = q.ROOT
WORK = ROOT / "prototype-output/earthdata-auth"
DOCS = ROOT / "docs/earthdata-auth"
WORK.mkdir(parents=True, exist_ok=True)
DOCS.mkdir(parents=True, exist_ok=True)
q.WORK = WORK
SECRET_MARKERS = [
    "FIXTURE_PASSWORD",
    "FIXTURE_COOKIE",
    "FIXTURE_SIGNATURE",
    "FIXTURE_CODE",
]


def save(name, value):
    (DOCS / f"{name}.json").write_text(json.dumps(value, indent=2) + "\n")


@contextlib.contextmanager
def environment(values):
    previous = {k: os.environ.get(k) for k in values}
    for k, v in values.items():
        if v is None:
            os.environ.pop(k, None)
        else:
            os.environ[k] = str(v)
    try:
        yield
    finally:
        for k, v in previous.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v


class Fixture:
    def __init__(self, hook=None, directory=None):
        fixture_work = Path(directory) if directory else WORK
        self.events = []
        self.case = ""
        self.counts = Counter()
        self.route_generation = 0
        self.pending = threading.Event()
        self.release = threading.Event()
        self.bytes = (ROOT / "tests/data/swot/quality-le.h5").read_bytes()
        self.etag = '"fixture-pinned-version"'
        cert, key = fixture_work / "fixture-cert.pem", fixture_work / "fixture-key.pem"
        subprocess.run(
            [
                "openssl",
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-days",
                "2",
                "-subj",
                "/CN=localhost",
                "-addext",
                "subjectAltName=DNS:localhost,IP:127.0.0.1",
                "-addext",
                "basicConstraints=critical,CA:FALSE",
                "-addext",
                "extendedKeyUsage=serverAuth",
                "-addext",
                "keyUsage=critical,digitalSignature,keyEncipherment",
                "-keyout",
                str(key),
                "-out",
                str(cert),
            ],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self.cert = cert
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def do_HEAD(self):
                self.handle_request()

            def do_GET(self):
                self.handle_request()

            def handle_request(self):
                role = self.server.role
                path = urlsplit(self.path).path
                fixture.counts[(role, self.command)] += 1
                auth = self.headers.get("Authorization")
                cookies = self.headers.get("Cookie", "")
                if role != "login":
                    assert not auth, "Authorization escaped login origin"
                if role in ["resource", "trap", "control"]:
                    assert not cookies, "cookie escaped its exact origin"

                def respond(status, headers=None, body=b""):
                    fixture.events.append(
                        {
                            "role": role,
                            "method": self.command,
                            "status": status,
                            "authorization_present": bool(auth),
                            "cookie_present": bool(cookies),
                            "body_bytes": len(body) if self.command != "HEAD" else 0,
                        }
                    )
                    self.send_response(status)
                    for k, v in (headers or {}).items():
                        self.send_header(k, v)
                    if not headers or "Content-Length" not in headers:
                        self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    if self.command != "HEAD" and body:
                        try:
                            self.wfile.write(body)
                        except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
                            pass

                def redirect(target, status=302, cookie=None):
                    h = {"Location": target}
                    if cookie:
                        h["Set-Cookie"] = cookie
                    respond(status, h)

                case = fixture.case
                if hook and hook(self, role, path, respond):
                    return
                if role == "trap":
                    respond(500)
                elif role == "control":
                    if path.endswith("/mutate"):
                        fixture.case = "changed-etag"
                    elif path.endswith("/restore"):
                        fixture.case = "success"
                    else:
                        assert fixture.pending.wait(10), (
                            "pending operation was not reached"
                        )
                    respond(200)
                elif role == "login":
                    assert auth, "no login credentials"
                    if case == "login-401":
                        respond(401)
                    elif (
                        case == "pending-auth" and fixture.counts[("login", "GET")] > 1
                    ):
                        fixture.pending.set()
                        fixture.release.wait(10)
                        respond(403)
                    else:
                        redirect(
                            fixture.archive + "/login?code=FIXTURE_CODE",
                            cookie="auth_session=FIXTURE_COOKIE; Path=/; Secure",
                        )
                elif role == "archive":
                    if path == "/login":
                        redirect(
                            fixture.canonical,
                            301,
                            "archive_session=FIXTURE_COOKIE; Path=/; Secure",
                        )
                    elif case == "downgrade":
                        redirect("http://127.0.0.1:1/leak?signature=FIXTURE_SIGNATURE")
                    elif case == "untrusted":
                        redirect(fixture.trap + "/leak?signature=FIXTURE_SIGNATURE")
                    elif case == "unexpected-path":
                        redirect(fixture.login + "/wrong?signature=FIXTURE_SIGNATURE")
                    elif case == "hop-budget":
                        redirect(
                            fixture.canonical,
                            cookie="step=FIXTURE_COOKIE; Path=/; Secure",
                        )
                    elif case == "loop":
                        redirect(fixture.canonical)
                    elif (
                        case == "canonical-429"
                        and fixture.counts[("archive", "HEAD")] == 1
                    ):
                        respond(429, {"Retry-After": "0"})
                    elif self.command == "GET" and (
                        not cookies
                        or (
                            case == "pending-auth"
                            and fixture.counts[("resource", "HEAD")] >= 4
                        )
                    ):
                        redirect(fixture.login + "/oauth/authorize?state=FIXTURE_CODE")
                    else:
                        expiry = int(time.time()) + (
                            29 if case == "short-expiry" else 3600
                        )
                        method = "head" if self.command == "HEAD" else "get"
                        if case == "wrong-method":
                            method = "get" if method == "head" else "head"
                        fixture.route_generation += 1
                        redirect(
                            fixture.resource
                            + f"/{method}/object?Expires={expiry}&Signature=FIXTURE_SIGNATURE&generation={fixture.route_generation}",
                            303,
                        )
                else:
                    if case == "resource-redirect":
                        redirect(fixture.login + "/oauth/authorize?state=FIXTURE_CODE")
                        return
                    expected = (
                        "/head/object" if self.command == "HEAD" else "/get/object"
                    )
                    if path != expected:
                        respond(403)
                        return
                    if (
                        self.command == "GET"
                        and case
                        in [
                            "expired-once",
                            "expired-after-cache",
                            "concurrent-expiration",
                            "pending-auth",
                        ]
                        and fixture.counts[("resource", "GET")]
                        == (
                            1
                            if case == "expired-once"
                            else 2
                            if case in ["expired-after-cache", "concurrent-expiration"]
                            else 0
                        )
                    ):
                        respond(403)
                        return
                    if self.command == "GET" and case == "refresh-exhausted":
                        respond(403)
                        return
                    if self.command == "GET" and case in [
                        "resource-401",
                        "resource-412",
                        "resource-429",
                    ]:
                        status = int(case.split("-")[1])
                        if status != 429 or fixture.counts[("resource", "GET")] == 1:
                            respond(status, {"Retry-After": "0"})
                            return
                    if (
                        self.command == "GET"
                        and case in ["pending-range", "pending-auth"]
                        and fixture.counts[("resource", "HEAD")] >= 4
                    ):
                        if case == "pending-auth":
                            respond(403)
                            return
                        fixture.pending.set()
                        fixture.release.wait(10)
                    etag = fixture.etag
                    size = len(fixture.bytes)
                    if (
                        case == "missing-etag"
                        or case == "missing-get-etag"
                        and self.command == "GET"
                    ):
                        etag = None
                    elif (
                        case == "weak-etag"
                        or case == "weak-get-etag"
                        and self.command == "GET"
                    ):
                        etag = "W/" + etag
                    elif case == "changed-etag" and self.command == "GET":
                        etag = '"different-version"'
                    elif (
                        case == "mutate-final"
                        and fixture.counts[("resource", "HEAD")] >= 5
                    ):
                        etag = '"different-version"'
                    elif (
                        case == "length-head"
                        and fixture.counts[("resource", "HEAD")] > 1
                    ):
                        size += 1
                    h = {"Content-Length": str(size)}
                    if etag:
                        h["ETag"] = etag
                    if self.command == "HEAD":
                        respond(200, h)
                        return
                    assert self.headers.get("If-Match") == fixture.etag, (
                        "conditional was lost"
                    )
                    assert self.headers.get("Accept-Encoding") == "identity", (
                        "encoding changed"
                    )
                    match = re.fullmatch(
                        r"bytes=(\d+)-(\d+)", self.headers.get("Range", "")
                    )
                    assert match, "range was lost"
                    a, b = map(int, match.groups())
                    body = fixture.bytes[a : b + 1]
                    h.update(
                        {
                            "Content-Length": str(len(body)),
                            "Content-Range": f"bytes {a}-{b}/{size}",
                        }
                    )
                    if case == "changed-length":
                        h["Content-Range"] = f"bytes {a}-{b}/{size + 1}"
                    elif case == "wrong-range":
                        h["Content-Range"] = f"bytes {a + 1}-{b + 1}/{size}"
                    elif case == "oversized":
                        body += b"x"
                        h["Content-Length"] = str(len(body))
                    elif case == "encoded":
                        h["Content-Encoding"] = "gzip"
                    elif case == "corrupt":
                        body = bytes([body[0] ^ 255]) + body[1:]
                    elif case == "truncated":
                        body = body[:-1]
                        h["Connection"] = "close"
                        self.close_connection = True
                    if (
                        case == "pending-body"
                        and fixture.counts[("resource", "HEAD")] >= 4
                    ):
                        self.send_response(206)
                        for k, v in h.items():
                            self.send_header(k, v)
                        self.end_headers()
                        self.wfile.flush()
                        fixture.pending.set()
                        fixture.release.wait(10)
                        try:
                            self.wfile.write(body)
                        except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
                            pass
                        return
                    respond(200 if case == "ignored-range" else 206, h, body)

        self.servers = []
        self.threads = []
        for role, host in [
            ("archive", "localhost"),
            ("login", "127.0.0.1"),
            ("resource", "localhost"),
            ("trap", "127.0.0.1"),
            ("control", "127.0.0.1"),
        ]:
            srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            srv.daemon_threads = True
            srv.role = role
            if role != "control":
                context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
                context.load_cert_chain(cert, key)
                srv.socket = context.wrap_socket(srv.socket, server_side=True)
            setattr(
                self,
                role,
                f"{'http' if role == 'control' else 'https'}://{host}:{srv.server_port}",
            )
            thread = threading.Thread(target=srv.serve_forever, daemon=True)
            thread.start()
            self.servers.append(srv)
            self.threads.append(thread)
        self.canonical = self.archive + "/source.nc"

        def rule(src, dest, path):
            return {"from_origin": src, "to_origin": dest, "destination_path": path}

        config = {
            "canonical_url": self.canonical,
            "login_origin": self.login,
            "resource_origin": self.resource,
            "head_resource_path": "/head/object",
            "get_resource_path": "/get/object",
            "redirects": [
                rule(self.archive, self.login, "/oauth/authorize"),
                rule(self.login, self.archive, "/login"),
                rule(self.archive, self.archive, "/source.nc"),
                rule(self.archive, self.resource, "/head/object"),
                rule(self.archive, self.resource, "/get/object"),
            ],
        }
        self.config_path = fixture_work / "fixture-policy.json"
        self.config_path.write_text(json.dumps(config))
        self.netrc = fixture_work / "fixture.netrc"
        self.netrc.write_text(
            "machine 127.0.0.1 login fixture password FIXTURE_PASSWORD\n"
        )
        self.netrc.chmod(0o600)

    def close(self):
        self.release.set()
        for srv in self.servers:
            srv.shutdown()
            srv.server_close()


def fixtures():
    fixture = Fixture()
    rows = []
    cases = [
        "success",
        "expired-once",
        "expired-after-cache",
        "short-expiry",
        "canonical-429",
        "resource-429",
        "resource-401",
        "login-401",
        "resource-412",
        "refresh-exhausted",
        "downgrade",
        "untrusted",
        "unexpected-path",
        "loop",
        "hop-budget",
        "wrong-method",
        "resource-redirect",
        "missing-etag",
        "weak-etag",
        "missing-get-etag",
        "weak-get-etag",
        "changed-etag",
        "length-head",
        "changed-length",
        "wrong-range",
        "oversized",
        "encoded",
        "corrupt",
        "truncated",
        "ignored-range",
        "mutate-final",
        "pending-range",
        "pending-auth",
        "pending-body",
        "anonymous",
        "unrelated",
        "public-netrc",
    ]
    successes = {
        "success",
        "expired-once",
        "expired-after-cache",
        "expired-after-cache",
        "short-expiry",
        "canonical-429",
        "resource-429",
    }
    try:
        for case in cases:
            fixture.case = case
            fixture.events.clear()
            fixture.counts.clear()
            fixture.pending.clear()
            fixture.release.clear()
            req = q.request(fixture.canonical)
            req["spatial"].update(start=[3, 4], count=[17, 19])
            if case == "unrelated":
                req["source"]["assets"][0]["locator"] = fixture.trap + "/unrelated.nc"
            fixture.netrc.chmod(0o644 if case == "public-netrc" else 0o600)
            with environment(
                {
                    "TILE_RIPPER_AUTH_NETRC": None
                    if case == "anonymous"
                    else fixture.netrc,
                    "TILE_RIPPER_AUTH_POLICY": fixture.config_path,
                    "TILE_RIPPER_AUTH_CA": fixture.cert,
                    "TILE_RIPPER_TEST_CONTROL": fixture.control + "/pending",
                    "TILE_RIPPER_HDF5_METADATA_POLICY": "page16k",
                }
            ):
                native = q.run(
                    req,
                    q.POLICIES["cookbook"],
                    "fixture-" + case,
                    mode="cancel-inflight" if case.startswith("pending-") else "dump",
                )
            assert (native["returncode"] == 0) == (case in successes), (
                case,
                native.get("error", "unexpected completion"),
            )
            row = {
                "case": case,
                "expected_success": case in successes,
                "returncode": native["returncode"],
                "elapsed_seconds": native["elapsed_seconds"],
                "wire_counts": dict(
                    Counter(
                        f"{e['role']} {e['method']} {e['status']}"
                        for e in fixture.events
                    )
                ),
            }
            if case in successes:
                row["science"] = q.verify(
                    native,
                    req,
                    ROOT / "tests/data/swot/quality-le.h5",
                    q.POLICIES["cookbook"],
                    "fixture-" + case,
                )
                row["transport"] = native["result"]["observed"]["transport"]["science"][
                    "authenticated_http"
                ]
                if case in ["expired-once", "expired-after-cache"]:
                    assert row["transport"]["refreshes"] == 1
                if case == "short-expiry":
                    assert row["transport"]["route_cache_hits"] == 0
            else:
                lines = (
                    (Path(native["output_dir"]) / "stderr.txt").read_text().splitlines()
                )
                row["error"] = next(
                    (line for line in lines if line.startswith("Error:")),
                    native["error"].splitlines()[0],
                )
                for line in lines:
                    if line.startswith('{"authenticated_http":'):
                        row["transport"] = json.loads(line)["authenticated_http"]
                        break
                expected = {
                    "downgrade": "unsafe",
                    "untrusted": "unsafe",
                    "unexpected-path": "unsafe",
                    "wrong-method": "unsafe",
                    "resource-redirect": "unsafe",
                    "loop": "redirect loop",
                    "hop-budget": "hop budget",
                    "missing-etag": "strong ETags",
                    "weak-etag": "strong ETags",
                    "missing-get-etag": "strong ETags",
                    "weak-get-etag": "strong ETags",
                    "changed-etag": "version conflict",
                    "length-head": "version conflict",
                    "changed-length": "range framing",
                    "wrong-range": "range framing",
                }
                if case in expected:
                    assert expected[case] in row["error"], (
                        "wrong transport rejection reason"
                    )
            if case.startswith("pending-"):
                assert native["elapsed_seconds"] < 5 and "cancel" in row["error"]
                fixture.release.set()
                time.sleep(0.05)
            assert (
                not fixture.counts[("trap", "GET")]
                and not fixture.counts[("trap", "HEAD")]
            )
            if case in ["anonymous", "unrelated", "public-netrc"]:
                assert not fixture.counts[("login", "GET")]
            raw = json.dumps(native)
            for marker in SECRET_MARKERS:
                assert marker not in raw, "secret appeared in diagnostics/provenance"
            rows.append(row)
            save(
                "fixtures",
                {"cases": rows, "passed": len(rows), "public_provider_requests": 0},
            )
            print(case, "PASS", flush=True)
        fixture.netrc.chmod(0o600)
        fixture.case = "concurrent-expiration"
        fixture.events.clear()
        fixture.counts.clear()
        with environment(
            {
                "TILE_RIPPER_AUTH_NETRC": fixture.netrc,
                "TILE_RIPPER_AUTH_POLICY": fixture.config_path,
                "TILE_RIPPER_AUTH_CA": fixture.cert,
                "TILE_RIPPER_TEST_CONTROL": fixture.control,
            }
        ):
            result = subprocess.run(
                [
                    "cargo",
                    "test",
                    "--locked",
                    "--all-features",
                    "--lib",
                    "storage::tests::authenticated_concurrent_routes_and_sticky_conflicts",
                    "--",
                    "--ignored",
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
        (WORK / "concurrent-test.log").write_text(result.stdout + result.stderr)
        assert result.returncode == 0, (
            "concurrent/sticky-conflict test failed; inspect local log"
        )
        rows.append(
            {
                "case": "concurrent-expiration-and-sticky-conflict",
                "passed": True,
                "wire_counts": dict(
                    Counter(
                        f"{e['role']} {e['method']} {e['status']}"
                        for e in fixture.events
                    )
                ),
            }
        )
        save(
            "fixtures",
            {"cases": rows, "passed": len(rows), "public_provider_requests": 0},
        )
        print("concurrent-expiration-and-sticky-conflict PASS", flush=True)
    finally:
        fixture.close()


def live(metadata):
    profile = json.loads((ROOT / "docs/swot-quality/product-profile.json").read_text())
    mirror = Mirror(ROOT / "prototype-output/swot-quality/mirror")
    mirror.verify()
    assert all(mirror.info[k] == profile[k] for k in ["url", "etag", "size"])
    req = q.request(profile["url"])
    req["source"]["assets"][0]["etag"] = profile["etag"]
    with environment(
        {
            "TILE_RIPPER_AUTH_NETRC": Path.home() / ".netrc",
            "TILE_RIPPER_AUTH_POLICY": None,
            "TILE_RIPPER_AUTH_CA": None,
            "TILE_RIPPER_HDF5_METADATA_POLICY": metadata,
        }
    ):
        native = q.run(req, q.POLICIES["cookbook"], "live-" + metadata, mode="dump")
    if native["returncode"]:
        save("live-" + metadata, {"live_native_pass": False, **native})
        raise RuntimeError(native["error"].splitlines()[0])
    science = q.verify(
        native, req, mirror.path, q.POLICIES["cookbook"], "live-" + metadata
    )
    value = native["result"]
    transport = value["observed"]["transport"]["science"]
    save(
        "live-" + metadata,
        {
            "live_native_pass": True,
            "metadata": metadata,
            **{
                k: native[k]
                for k in [
                    "binary_sha256",
                    "elapsed_seconds",
                    "peak_rss_bytes",
                    "child_cpu_seconds",
                ]
            },
            **{
                k: value[k]
                for k in [
                    "prepare_seconds",
                    "first_result_seconds",
                    "completion_seconds",
                    "summary",
                ]
            },
            "product": {k: profile[k] for k in ["url", "etag", "size"]},
            "science": science,
            "transport": transport,
            "observed": value["observed"],
            "identity": value["inspection"]["sources"][0]["object_identities"][
                "identity"
            ],
            "references": "independent h5py/NetCDF-C against verified previously sealed incomplete mirror; native acquisition is directly from NASA",
        },
    )
    print(
        json.dumps(
            {
                "live_native_pass": True,
                "metadata": metadata,
                "source_body_bytes": transport["bytes"],
                "authenticated_http": transport["authenticated_http"],
            }
        ),
        flush=True,
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["fixtures", "live"])
    parser.add_argument(
        "--metadata", choices=["exact", "page4k", "page16k"], default="page16k"
    )
    args = parser.parse_args()
    fixtures() if args.action == "fixtures" else live(args.metadata)
