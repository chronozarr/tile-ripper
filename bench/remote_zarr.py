"""Controlled anonymous HTTP Zarr oracle, faults and equivalent benchmarks.
Requires only the existing frozen xarray/Zarr environment and Python stdlib HTTP.
"""

import argparse
import copy
from collections import Counter
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import itertools
import http.client
from urllib.parse import urlsplit
import json
import os
from pathlib import Path
import random
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import MutableMapping

import numpy as np
import xarray as xr
import zarr
from zarr_workflow import fixture, expected, check
from unified_workflow import minimum_keys, medium_fixture

ROOT = Path(__file__).resolve().parents[1]
DOCS = Path(os.environ.get("TILE_RIPPER_VALIDATION_DOCS", ROOT / "docs"))
WORK = ROOT / "prototype-output/remote-zarr"
CURRENT = ROOT / "target/release/tile-ripper"
EXAMPLE = ROOT / "target/release/examples/remote_zarr"


class Server(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, root, latency=0, bandwidth=0, connections=1, port=0):
        super().__init__(("127.0.0.1", port), Handler)
        self.root = root
        self.latency = latency
        self.bandwidth = bandwidth
        self.slots = threading.BoundedSemaphore(connections)
        self.records = []
        self.lock = threading.Lock()
        self.fault = {}

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server_port}/store/"


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self):
        super().setup()
        self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def log_message(self, *_):
        pass

    def do_HEAD(self):
        self.serve(True)

    def do_GET(self):
        self.serve(False)

    def serve(self, head):
        server = self.server
        if self.path.startswith("/control/"):
            mode = self.path.rsplit("/", 1)[1]
            if mode == "mutate":
                path = server.root / "red/0.0.0"
                data = bytearray(path.read_bytes())
                data[-1] ^= 1
                path.write_bytes(data)
            elif mode == "appear":
                target = server.root / "nir/1/1/1"
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes((server.root / "nir/1/1/0").read_bytes())
            elif mode == "admission":
                (server.root / "zarr.json").write_text('{"zarr_format":3}')
            elif mode == "coordinate":
                path = server.root / "x/0"
                data = bytearray(path.read_bytes())
                data[0] ^= 1
                path.write_bytes(data)
            elif mode == "metadata":
                path = server.root / ".zattrs"
                data = json.loads(path.read_text())
                data["title"] = "changed"
                path.write_text(json.dumps(data))
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        key = self.path.removeprefix("/store/")
        if self.path.split("?", 1)[0] != "/store/" + key or ".." in key.split("/"):
            self.send_error(400)
            return
        with server.slots:
            time.sleep(server.latency)
            path = server.root / key
            data = path.read_bytes() if path.is_file() else None
            etag = (
                '"' + hashlib.sha256(data).hexdigest() + '"'
                if data is not None
                else None
            )
            status = 200 if data is not None else 404
            if self.headers.get("If-Match") and self.headers["If-Match"] != etag:
                status = 412
            if self.headers.get("If-None-Match") == "*" and data is not None:
                status = 412
            fault = server.fault.get((self.command, key), [])
            action = fault.pop(0) if fault else None
            if isinstance(action, int):
                status = action
            self.send_response(status)
            if etag:
                self.send_header("ETag", ("W/" if action == "weak" else "") + etag)
            self.send_header(
                "Content-Length",
                str(
                    2 * 1024 * 1024
                    if action == "oversize"
                    else len(data)
                    if status == 200
                    else 0
                ),
            )
            if status == 302:
                self.send_header(
                    "Location", "https://example.invalid/?token=server-secret"
                )
            if status == 429:
                self.send_header("Retry-After", "0")
            self.end_headers()
            written = 0
            if not head and status == 200:
                limit = len(data) // 2 if action == "interrupt" else len(data)
                try:
                    for start in range(0, limit, 4096):
                        part = data[start : min(start + 4096, limit)]
                        self.wfile.write(part)
                        self.wfile.flush()
                        written += len(part)
                        if server.bandwidth:
                            time.sleep(len(part) / server.bandwidth)
                except (BrokenPipeError, ConnectionResetError):
                    pass
                if action == "interrupt":
                    self.close_connection = True
                    try:
                        self.connection.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
            with server.lock:
                server.records.append(
                    dict(
                        method=self.command,
                        key=key,
                        status=status,
                        bytes=written,
                        if_match=self.headers.get("If-Match"),
                        if_none_match=self.headers.get("If-None-Match"),
                        time=time.perf_counter(),
                    )
                )


def start(root, **kwargs):
    server = Server(root, **kwargs)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server


def process(cmd):
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        t = time.perf_counter()
        p = subprocess.Popen(
            cmd,
            cwd=ROOT,
            stdout=out,
            stderr=err,
            env={**os.environ, "RAYON_NUM_THREADS": "1"},
        )
        _, status, usage = os.wait4(p.pid, 0)
        p.returncode = os.waitstatus_to_exitcode(status)
        out.seek(0)
        err.seek(0)
        stdout = out.read().decode()
        stderr = err.read().decode()
        return dict(
            code=p.returncode,
            wall_seconds=time.perf_counter() - t,
            peak_rss_bytes=usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024),
            result=json.loads(stdout) if not p.returncode else None,
            error=stderr[:3000],
        )


class HttpMapping(MutableMapping):
    """Stdlib HTTP transport for independent Zarr-Python decoding/selection.

    Enforces the same snapshot/size/retry contract as Rust; no chunk cache.
    zarr itself chooses chunk keys, codecs, fill and index assembly.
    """

    def __init__(self, url, cap, nodes):
        parsed = urlsplit(url)
        self.path = parsed.path
        self.connection = (
            http.client.HTTPSConnection
            if parsed.scheme == "https"
            else http.client.HTTPConnection
        )(parsed.hostname, parsed.port, timeout=30)
        self.url = url
        self.cap = cap
        self.nodes = nodes
        self.versions = {}
        self.events = []
        self.cached = {}
        self.sealed = False
        self.names = []
        self.metadata = None
        raw = self.fetch(".zmetadata")
        if raw is not None:
            self.metadata = json.loads(raw)["metadata"]
            self.cached.pop(".zmetadata", None)

    def fetch(self, key, head=False, data_head=False):
        known = self.versions.get(key, "unknown")
        headers = {}
        if known != "unknown":
            headers = {"If-Match": known[0]} if known else {"If-None-Match": "*"}
        for attempt in range(3):
            try:
                self.connection.request(
                    "HEAD" if head else "GET", self.path + key, headers=headers
                )
                response = self.connection.getresponse()
                with response:
                    status = response.status
                    event = dict(key=key, head=head, status=status, bytes=0)
                    self.events.append(event)
                    if status in [404, 408, 412, 429, 500, 502, 503, 504]:
                        response.read()
                    if status in [408, 429, 500, 502, 503, 504]:
                        raise ConnectionError("retry")
                    if status == 404:
                        if known not in ["unknown", None]:
                            raise RuntimeError("source disappeared")
                        self.versions[key] = None
                        return None
                    if status != 200 or known is None:
                        raise RuntimeError("version/status conflict")
                    tag = response.headers["ETag"]
                    size = int(response.headers["Content-Length"])
                    if size > self.cap:
                        raise RuntimeError("object budget")
                    body = b"" if head else response.read(self.cap + 1)
                    event["bytes"] = len(body)
                    if not head and len(body) != size:
                        raise ConnectionError("truncated body")
                    version = (
                        tag,
                        size,
                        known[2]
                        if known != "unknown"
                        else not data_head and not self.sealed,
                    )
                    if known != "unknown" and known != version:
                        raise RuntimeError("version conflict")
                    self.versions[key] = version
                    return size if head else body
            except (ConnectionError, OSError):
                if attempt == 2:
                    raise
                time.sleep(0.025 * 2**attempt)

    def seal(self):
        self.sealed = True
        self.validate()

    def validate(self):
        for key, v in list(self.versions.items()):
            if v is None or v[2]:
                self.fetch(key, head=True)

    def __getitem__(self, key):
        if self.metadata is not None and key in self.metadata:
            return json.dumps(self.metadata[key]).encode()
        if key in self.cached:
            body = self.cached[key]
        else:
            body = self.fetch(key)
            if not self.sealed:
                if (
                    sum(len(v) for v in self.cached.values() if v is not None)
                    + (len(body) if body else 0)
                    > self.cap
                ):
                    raise RuntimeError("metadata cache budget")
                self.cached[key] = body
        if body is None:
            raise KeyError(key)
        return body

    def __iter__(self):
        # HTTP object storage has no portable listing API. Both readers receive
        # requested variable names; enumerate their v2 metadata keys only.
        return iter(
            [".zgroup", ".zattrs"]
            + [
                name + "/" + key
                for name in self.names
                for key in [".zarray", ".zattrs"]
            ]
        )

    def __len__(self):
        return 2 + len(self.names) * 2

    def __setitem__(self, *_):
        raise TypeError("read only")

    def __delitem__(self, *_):
        raise TypeError("read only")

    def __contains__(self, key):
        if key in self.versions:
            return self.versions[key] is not None
        try:
            self[key]
            return True
        except KeyError:
            return False


def baseline(r, mode):
    # Independent xarray selects native coordinates/variables. Freeze exactly the
    # selected data identities, matching Rust's required HEAD/version work.
    t = time.perf_counter()
    store = HttpMapping(
        r["source"]["assets"][0]["locator"],
        r["budget"]["buffer_bytes"] // 2,
        r["budget"]["max_nodes"],
    )
    store.names = ["x", "y", "time"] + [v["path"].lstrip("/") for v in r["variables"]]
    source = xr.open_zarr(store, consolidated=False, decode_cf=False, chunks=None)
    subset = expected(r, source)
    group = zarr.open_group(store=store, mode="r")
    keys = []
    for var in subset.data_vars:
        chunks = group[var].chunks
        sep = group[var]._dimension_separator
        indices = []
        for axis, c in zip(["time", "y", "x"], chunks):
            selected = np.flatnonzero(np.isin(source[axis].values, subset[axis].values))
            indices.append(sorted(set((selected // c).tolist())))
        for index in itertools.product(*indices):
            keys.append(var + "/" + sep.join(map(str, index)))
    for key in sorted(keys):
        store.fetch(key, head=True, data_head=True)
    store.seal()
    planning = time.perf_counter() - t
    rows = []
    # Same scientific/codec/chunk/synchronization semantics as the local matched
    # baseline. Write the loaded subset through xarray, not Tile-Ripper code.
    for rep in range(2 if mode == "warm" else 1):
        output = Path(r["sink"]["path"] + f"-{rep}")
        output.mkdir()
        t = time.perf_counter()
        store.validate()
        read = time.perf_counter()
        sub = subset.copy(deep=False).load()
        read = time.perf_counter() - read
        sub.attrs = copy.deepcopy(sub.attrs)
        tr = sub.attrs["tile_ripper_grid"]["transform"]
        tr[0] = float(sub.x.values[0] - 5)
        tr[3] = float(sub.y.values[0] + 5)
        edge = r["budget"]["chunk_edge"]

        class Durable(zarr.DirectoryStore):
            first_output_seconds = None

            def __setitem__(self, key, value):
                super().__setitem__(key, value)
                with open(Path(self.path) / key, "rb") as file:
                    if sys.platform == "darwin":
                        __import__("fcntl").fcntl(file.fileno(), 51)
                    else:
                        os.fsync(file.fileno())
                if (
                    key.split("/")[0] in sub.data_vars
                    and not Path(key).name.startswith(".")
                    and self.first_output_seconds is None
                ):
                    self.first_output_seconds = time.perf_counter() - t

        encoding = {
            n: {
                "chunks": (sub[n].size,),
                "compressor": group[n].compressor,
                "_FillValue": None,
            }
            for n in sub.coords
        }
        encoding.update(
            {
                n: {
                    "chunks": (1, min(edge, sub.sizes["y"]), min(edge, sub.sizes["x"])),
                    "compressor": group[n].compressor,
                }
                for n in sub.data_vars
            }
        )
        write = time.perf_counter()
        destination = Durable(str(output / "cube.zarr"))
        sub.to_zarr(
            destination,
            mode="w",
            consolidated=False,
            encoding=encoding,
            write_empty_chunks=False,
        )
        write = time.perf_counter() - write
        store.validate()
        manifest = json.loads((WORK / "manifest-template.json").read_text())
        manifest["observed"] = {
            "reader": "independent xarray/Zarr",
            "source_versions": store.versions,
            "read_decode_seconds": read,
            "writing_seconds": write,
        }
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2))
        rows.append(
            dict(
                output=str(output),
                fetch_seconds=time.perf_counter() - t,
                read_decode_seconds=read,
                writing_seconds=write,
                first_output_seconds=destination.first_output_seconds,
            )
        )
    return dict(prepare_seconds=planning, runs=rows)


def fixture_setup(consolidated=False):
    r = json.loads(fixture().read_text())
    source = WORK / "source.zarr"
    shutil.rmtree(source, ignore_errors=True)
    shutil.copytree(r["source"]["assets"][0]["locator"], source)
    if consolidated:
        zarr.consolidate_metadata(str(source))
    return r, source


def checks():
    WORK.mkdir(parents=True, exist_ok=True)
    r, source = fixture_setup()
    server = start(source)
    original = r["source"]["assets"][0]["locator"]
    r["source"]["assets"][0]["locator"] = server.url
    r["sink"] = {"kind": "directory", "path": str(WORK / "checked")}
    req = WORK / "request.json"
    req.write_text(json.dumps(r))
    local = copy.deepcopy(r)
    local["source"]["assets"][0]["locator"] = original
    results = {}
    try:
        for mode in [
            "normal",
            "no-trailing-slash",
            "uncompressed",
            "corrupt",
            "decoded-size-mismatch",
            "gzip-bomb",
            "gzip-truncated",
            "consolidated",
            "503",
            "429",
            "interrupt",
            "permanent",
            "mutate",
            "appear",
            "metadata",
            "isolation",
            "admission",
            "coordinate",
            "postmetadata",
            "cancel",
            "malformed",
            "empty",
            "oversize",
            "empty-space",
            "weak-etag",
            "redirect",
            "hybrid",
        ]:
            shutil.rmtree(source)
            shutil.copytree(original, source)
            if mode == "consolidated":
                zarr.consolidate_metadata(str(source))
                for path in source.rglob("*"):
                    if path.name in [".zarray", ".zattrs", ".zgroup"]:
                        path.unlink()
            if mode == "hybrid":
                (source / "zarr.json").write_text('{"zarr_format":3}')
            if mode == "malformed":
                (source / ".zmetadata").write_text("{bad")
            if mode == "uncompressed":
                group = zarr.open_group(str(source), mode="a")
                for name in ["red", "nir"]:
                    ar = group[name]
                    values = ar[:]
                    attrs = dict(ar.attrs)
                    chunks = ar.chunks
                    sep = ar._dimension_separator
                    fill = ar.fill_value
                    del group[name]
                    target = group.create_dataset(
                        name,
                        data=values,
                        chunks=chunks,
                        compressor=None,
                        fill_value=fill,
                        dimension_separator=sep,
                        write_empty_chunks=False,
                    )
                    target.attrs.update(attrs)
            if mode in ["decoded-size-mismatch", "gzip-bomb"]:
                # Reject after expected bytes + one probe, independent of the
                # stream's advertised/actual inflation (1 MiB and 16 MiB).
                from numcodecs import GZip

                (source / "red/0.0.0").write_bytes(
                    GZip(1).encode(
                        bytes((16 if mode == "gzip-bomb" else 1) * 1024 * 1024)
                    )
                )
            if mode == "gzip-truncated":
                path = source / "red/0.0.0"
                path.write_bytes(path.read_bytes()[:-4])
            if mode == "corrupt":
                path = source / "red/0.0.0"
                encoded = bytearray(path.read_bytes())
                encoded[-1] ^= 1
                path.write_bytes(encoded)
            server.records.clear()
            server.fault = {}
            if mode in ["503", "429", "permanent", "interrupt"]:
                server.fault[("GET", "red/0.0.0")] = (
                    [503] * 3
                    if mode == "permanent"
                    else [int(mode)]
                    if mode.isdigit()
                    else ["interrupt"]
                )
            if mode == "weak-etag":
                server.fault[("GET", ".zgroup")] = ["weak"]
            if mode == "redirect":
                server.fault[("GET", ".zmetadata")] = [302]
            if mode == "oversize":
                server.fault[("HEAD", "red/0.0.0")] = ["oversize"]
            request = copy.deepcopy(r)
            if mode == "no-trailing-slash":
                request["source"]["assets"][0]["locator"] = server.url.rstrip("/")
            if mode == "empty":
                request["time"] = {
                    "kind": "instants",
                    "values": ["2024-01-01T00:00:00Z"],
                }
            if mode == "empty-space":
                request["spatial"]["bbox"] = [600000, 3990000, 600100, 3990100]
            req.write_text(json.dumps(request))
            out = Path(r["sink"]["path"])
            shutil.rmtree(out, ignore_errors=True)
            shutil.rmtree(Path(str(out) + "-0"), ignore_errors=True)
            if mode in [
                "mutate",
                "appear",
                "metadata",
                "isolation",
                "cancel",
                "admission",
                "coordinate",
                "postmetadata",
            ]:
                test = subprocess.run(
                    [
                        "cargo",
                        "test",
                        "--locked",
                        "--test",
                        "remote_zarr",
                        "prepared_http_changes_and_cancellation",
                        "--",
                        "--ignored",
                        "--exact",
                    ],
                    cwd=ROOT,
                    env={
                        **os.environ,
                        "TILE_RIPPER_HTTP_REQUEST": str(req),
                        "TILE_RIPPER_HTTP_MODE": mode,
                    },
                    capture_output=True,
                    text=True,
                )
                assert test.returncode == 0, (mode, test.stdout, test.stderr)
                results[mode] = dict(passed=True, requests=list(server.records))
                continue
            command = [str(EXAMPLE), str(req), "fetch"]
            row = process(command)
            failure = mode in [
                "permanent",
                "corrupt",
                "decoded-size-mismatch",
                "gzip-bomb",
                "gzip-truncated",
                "mutate",
                "appear",
                "metadata",
                "malformed",
                "oversize",
                "weak-etag",
                "redirect",
                "hybrid",
            ]
            assert bool(row["code"]) == failure, (mode, row)
            if failure:
                assert not out.exists(), (mode, "partial output exists")
            else:
                manifest = row["result"]["runs"][0]
                report = manifest["observed"]
                expected_keys = (
                    minimum_keys(local) if mode not in ["empty", "empty-space"] else []
                )
                assert sorted(v["key"] for v in report["source_reads"]) == expected_keys
                assert all(v["requests"] == 1 for v in report["source_reads"])
                assert report["decoded_source_chunks"] + report[
                    "filled_source_chunks"
                ] == len(expected_keys)
                if mode not in ["empty", "empty-space"]:
                    check(local, Path(manifest["output"]))
                existing = [key for key in expected_keys if (source / key).is_file()]
                actual = Counter(
                    v["key"]
                    for v in server.records
                    if v["method"] == "GET" and v["key"] in expected_keys
                )
                assert sorted(actual) == existing
                extra = 1 if mode in ["503", "429", "interrupt"] else 0
                assert sum(actual.values()) == len(existing) + extra
                if mode in ["503", "429", "interrupt"]:
                    assert report["transport"]["cube"]["retries"] == 1
                if mode == "normal":
                    before_plan = len(server.records)
                    plan = process([str(CURRENT), "plan", str(req)])
                    assert plan["code"] == 0
                    assert (
                        sorted(
                            n["operation"]["read"]["key"]
                            for n in plan["result"]["plan"]["nodes"]
                            if n["operation"]["kind"] == "read"
                        )
                        == expected_keys
                    )
                    assert not any(
                        v["method"] == "GET" and v["key"] in expected_keys
                        for v in server.records[before_plan:]
                    )
            results[mode] = dict(
                passed=True,
                error=row["error"] if failure else None,
                peak_rss_bytes=row["peak_rss_bytes"],
                requests=list(server.records),
            )
        # Malformed/signed URLs fail before network traffic and cannot leak secrets.
        secret = copy.deepcopy(r)
        secret["source"]["assets"][0]["locator"] = server.url + "?token=secret"
        req.write_text(json.dumps(secret))
        row = process([str(CURRENT), "plan", str(req)])
        assert row["code"] and "secret" not in row["error"]
        results["credential_redaction"] = True
    finally:
        server.shutdown()
        server.server_close()
    (DOCS / "remote-zarr-checks.json").write_text(json.dumps(results, indent=2))


def resource_check():
    local = medium_fixture(json.loads(fixture().read_text()))
    server = start(Path(local["source"]["assets"][0]["locator"]))
    remote = copy.deepcopy(local)
    remote["source"]["assets"][0]["locator"] = server.url
    remote["sink"] = {"kind": "directory", "path": str(WORK / "medium-resource")}
    req = WORK / "resource-request.json"
    req.write_text(json.dumps(remote))
    shutil.rmtree(Path(remote["sink"]["path"]), ignore_errors=True)
    shutil.rmtree(Path(remote["sink"]["path"] + "-0"), ignore_errors=True)
    try:
        row = process([str(EXAMPLE), str(req), "fetch"])
        assert row["code"] == 0, row
        result = row.pop("result")
        observed = result["runs"][0]["observed"]
        keys = minimum_keys(local)
        assert sorted(v["key"] for v in observed["source_reads"]) == keys
        assert all(v["requests"] == 1 for v in observed["source_reads"])
        assert (
            observed["output_io"]["buffered_bytes_peak"]
            <= remote["budget"]["buffer_bytes"] // 4
        )
        assert (
            observed["decoded_source_chunks"] == 111
            and observed["filled_source_chunks"] == 1
        )
        row.update(
            minimum_keys=keys,
            observed=observed,
            validated_values=check(local, Path(result["runs"][0]["output"])),
            requests=list(server.records),
            buffer_bytes=remote["budget"]["buffer_bytes"],
        )
    finally:
        server.shutdown()
        server.server_close()
    (DOCS / "remote-zarr-resource.json").write_text(
        json.dumps(row, indent=2) + "\n"
    )


def benchmark(reps):
    WORK.mkdir(parents=True, exist_ok=True)
    rows = []
    rng = random.Random(19)
    for label, latency, bandwidth, connections in [
        ("near", 0, 0, 4),
        ("latency", 0.01, 0, 1),
        ("bandwidth", 0.002, 256 * 1024, 2),
    ]:
        for consolidated in [False, True]:
            r, source = fixture_setup(consolidated)
            local = copy.deepcopy(r)
            keys = minimum_keys(local)
            server = start(
                source, latency=latency, bandwidth=bandwidth, connections=connections
            )
            r["source"]["assets"][0]["locator"] = server.url
            try:
                template = WORK / "template"
                shutil.rmtree(template, ignore_errors=True)
                r["sink"] = {"kind": "directory", "path": str(template)}
                req = WORK / "template-request.json"
                req.write_text(json.dumps(r))
                sample = process(
                    [str(CURRENT), "fetch", str(req), "--output", str(template)]
                )
                assert sample["code"] == 0, sample
                shutil.copy(template / "manifest.json", WORK / "manifest-template.json")
                for rep in range(reps):
                    readers = ["rust", "xarray"]
                    rng.shuffle(readers)
                    for reader in readers:
                        r["sink"] = {
                            "kind": "directory",
                            "path": str(
                                WORK / f"{label}-{consolidated}-{rep}-{reader}"
                            ),
                        }
                        for i in range(2):
                            shutil.rmtree(
                                Path(r["sink"]["path"] + f"-{i}"), ignore_errors=True
                            )
                        req = WORK / "bench-request.json"
                        req.write_text(json.dumps(r))
                        server.records.clear()
                        cmd = (
                            [str(EXAMPLE), str(req), "warm"]
                            if reader == "rust"
                            else [
                                sys.executable,
                                __file__,
                                "worker",
                                "--request",
                                str(req),
                                "--mode",
                                "warm",
                            ]
                        )
                        result = process(cmd)
                        assert result["code"] == 0, result
                        data = result.pop("result")
                        records = list(server.records)
                        for index, run in enumerate(data["runs"]):
                            check(local, Path(run["output"]))
                        gets = Counter(
                            v["key"]
                            for v in records
                            if v["method"] == "GET" and v["key"] in keys
                        )
                        existing = [k for k in keys if (source / k).is_file()]
                        assert sorted(gets) == existing and all(
                            v == 2 for v in gets.values()
                        ), (reader, gets)
                        for run in data["runs"]:
                            files = [
                                p for p in Path(run["output"]).rglob("*") if p.is_file()
                            ]
                            run["output_bytes"] = sum(p.stat().st_size for p in files)
                            run["output_files"] = len(files)
                        result.update(
                            reader=reader,
                            regime=label,
                            consolidated=consolidated,
                            repetition=rep,
                            planning_transport=data.get("inspection", {}).get(
                                "transport"
                            ),
                            planned_keys=sorted(
                                n["operation"]["read"]["key"]
                                for n in data["inspection"]["plan"]["nodes"]
                                if n["operation"]["kind"] == "read"
                            )
                            if reader == "rust"
                            else None,
                            prepare_seconds=data["prepare_seconds"],
                            runs=data["runs"],
                            requests=records,
                            minimum_keys=keys,
                        )
                        rows.append(result)
                        print(
                            label,
                            consolidated,
                            reader,
                            rep,
                            round(result["wall_seconds"], 3),
                            file=sys.stderr,
                            flush=True,
                        )
            finally:
                server.shutdown()
                server.server_close()
    summary = []
    for regime, cons, reader in itertools.product(
        ["near", "latency", "bandwidth"], [False, True], ["rust", "xarray"]
    ):
        selected = [
            v
            for v in rows
            if (v["regime"], v["consolidated"], v["reader"]) == (regime, cons, reader)
        ]
        summary.append(
            dict(
                regime=regime,
                consolidated=cons,
                reader=reader,
                wall_median=statistics.median(v["wall_seconds"] for v in selected),
                wall_min=min(v["wall_seconds"] for v in selected),
                wall_max=max(v["wall_seconds"] for v in selected),
                peak_rss_bytes=max(v["peak_rss_bytes"] for v in selected),
                prepare_median=statistics.median(
                    v["prepare_seconds"] for v in selected
                ),
                cold_acquisition_median=statistics.median(
                    v["prepare_seconds"] + v["runs"][0]["fetch_seconds"]
                    for v in selected
                ),
                warm_fetch_median=statistics.median(
                    v["runs"][1]["fetch_seconds"] for v in selected
                ),
            )
        )
    (DOCS / "remote-zarr-benchmark.json").write_text(
        json.dumps(
            dict(
                repetitions=reps,
                runs=rows,
                summary=summary,
                binary_sha256=hashlib.sha256(EXAMPLE.read_bytes()).hexdigest(),
                cli_sha256=hashlib.sha256(CURRENT.read_bytes()).hexdigest(),
                source_sha256=hashlib.sha256(
                    b"".join(
                        p.name.encode() + p.read_bytes()
                        for p in sorted((ROOT / "src").glob("*.rs"))
                    )
                ).hexdigest(),
                harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                cargo_lock_sha256=hashlib.sha256(
                    (ROOT / "Cargo.lock").read_bytes()
                ).hexdigest(),
            ),
            indent=2,
        )
    )


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "action", choices=["checks", "benchmark", "worker", "serve", "resource"]
    )
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--request", type=Path)
    ap.add_argument("--mode", default="warm")
    args = ap.parse_args()
    if args.action == "checks":
        checks()
    elif args.action == "resource":
        resource_check()
    elif args.action == "benchmark":
        benchmark(args.reps)
    elif args.action == "worker":
        print(json.dumps(baseline(json.loads(args.request.read_text()), args.mode)))
    else:
        WORK.mkdir(parents=True, exist_ok=True)
        r, source = fixture_setup()
        s = start(source, port=args.port)
        r["source"]["assets"][0]["locator"] = s.url
        r["sink"] = {"kind": "directory", "path": str(WORK / "my-extract")}
        (WORK / "demo-request.json").write_text(json.dumps(r))
        print(
            f"Anonymous HTTP store: {s.url}\nRequest: {WORK / 'demo-request.json'}",
            flush=True,
        )
        try:
            threading.Event().wait()
        except KeyboardInterrupt:
            s.shutdown()
            s.server_close()
