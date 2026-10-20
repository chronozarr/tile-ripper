"""Geospatial-library-free runtime: real STAC and frozen HTTP byte equality."""

import ctypes.util
import hashlib
import http.server
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import threading

ROOT = Path("/src")
WORK = ROOT / "prototype-output/native-http"


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self):
        super().setup()
        self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def log_message(self, *args):
        pass

    def do_HEAD(self):
        self.serve(False)

    def do_GET(self):
        self.serve(True)

    def serve(self, body):
        key = self.path
        if key not in self.server.sources:
            self.send_error(404)
            return
        source = self.server.sources[key]
        size, etag = source["bytes"], source["etag"]
        if self.headers.get("If-Match") not in [None, etag]:
            self.send_error(412)
            return
        lo, hi = 0, size - 1
        if body:
            match = re.fullmatch(r"bytes=(\d+)-(\d+)", self.headers.get("Range", ""))
            if not match:
                self.send_error(416)
                return
            lo, hi = map(int, match.groups())
            assert 0 <= lo <= hi < size
            assert set(range(lo // self.server.page, hi // self.server.page + 1)) <= set(self.server.pages[key])
            assert self.headers.get("If-Match") == etag
        self.send_response(206 if body else 200)
        self.send_header("ETag", etag)
        self.send_header("Content-Length", str(hi - lo + 1))
        if body:
            self.send_header("Content-Range", f"bytes {lo}-{hi}/{size}")
        self.end_headers()
        if body:
            with (self.server.mirror / key[1:]).open("rb") as stream:
                stream.seek(lo)
                self.wfile.write(stream.read(hi - lo + 1))
            with self.server.lock:
                self.server.gets += 1
                self.server.bytes += hi - lo + 1


def invoke(args):
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        p = subprocess.Popen([str(a) for a in args], stdout=out, stderr=err,
                             env={**os.environ, "TILE_RIPPER_WORKERS": "2"})
        _, status, usage = os.wait4(p.pid, 0)
        p.returncode = os.waitstatus_to_exitcode(status)
        err.seek(0)
        assert p.returncode == 0, err.read().decode()
        out.seek(0)
        return out.read().decode(), usage.ru_maxrss * 1024


def main():
    assert all(ctypes.util.find_library(n) is None for n in ["gdal", "proj", "tiff"])
    libs = (WORK / "linux-ldd.txt").read_text()
    assert not any(n in libs.lower() for n in ["gdal", "libproj", "libtiff"])
    expected = json.loads((ROOT / "docs/native-http/live.json").read_text())
    request_path = ROOT / "docs/native-sentinel/request.json"
    plan_text, _ = invoke([WORK / "linux-tile-ripper", "plan", request_path, "--gdal-free", "--backend", "native", "--json"])
    assert json.loads(plan_text)["query"]["coordinate_order"] == "longitude, latitude"
    text, live_rss = invoke([WORK / "linux-native-stats", request_path])
    rows = [json.loads(s) for s in text.splitlines()]
    assert rows[-1]["type"] == "complete"
    source_fields = ["item_id", "asset_key", "requested_band", "datetime", "href", "object_identity"]
    assert [{k:s[k] for k in source_fields} for s in rows[0]["provenance"]["sources"]] == expected["selected_sources"]
    fields = list(expected["independent_chunk_references"][0])
    assert [{k:r[k] for k in fields} for r in rows[1:-1]] == expected["independent_chunk_references"]
    assert live_rss < 192 * 2**20

    mirror = ROOT / "prototype-output/production-streaming/real/mirror"
    index = json.loads((mirror / "index.json").read_text())
    for event in index["events"]:
        with (mirror / event["source"].lstrip("/")).open("rb") as stream:
            stream.seek(event["start"])
            captured = stream.read(event["bytes"])
        assert hashlib.sha256(captured).hexdigest() == event["sha256"]
    sources = json.loads((mirror.parent / "sources.json").read_text())["sources"]
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    server.sources = {f"/{i}.tif": s for i,s in enumerate(sources)}
    server.mirror, server.pages, server.page = mirror, index["pages"], 256 * 1024
    server.gets, server.bytes, server.lock = 0, 0, threading.Lock()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    frozen = []
    try:
        for budget in [16, 32]:
            request = json.loads((ROOT / "prototype-output/native-cog/exact/overlap-b.json").read_text())
            for i,a in enumerate(request["source"]["assets"]):
                a["locator"] = f"http://127.0.0.1:{server.server_port}/{i}.tif"
                a["etag"] = sources[i]["etag"]
            request["budget"]["buffer_bytes"] = budget * 2**20
            path = WORK / f"linux-overlap-{budget}.json"
            path.write_text(json.dumps(request))
            before_gets, before_bytes = server.gets, server.bytes
            text, rss = invoke([WORK / "linux-exact-oracle", path, ROOT / "prototype-output/native-cog/exact/overlap-b.bin"])
            result = json.loads(text)
            assert result["complete"] and result["samples_compared_bit_for_bit"] == 3538944
            assert rss < 192 * 2**20
            frozen.append(dict(budget_mib=budget, **result, peak_rss_bytes=rss,
                               gets=server.gets-before_gets, body_bytes=server.bytes-before_bytes))
    finally:
        server.shutdown()
        server.server_close()
    record = dict(passed=True, gdal_proj_libtiff_absent=True, live_independent_gdal_chunks=len(rows)-2,
        mirror_index_sha256=hashlib.sha256((mirror/"index.json").read_bytes()).hexdigest(),
        live_peak_rss_bytes=live_rss, frozen_http_exact=frozen, linkage=libs,
        binaries={p.name:dict(bytes=p.stat().st_size,sha256=hashlib.sha256(p.read_bytes()).hexdigest())
                  for p in [WORK/"linux-tile-ripper", WORK/"linux-native-stats", WORK/"linux-exact-oracle"]})
    (ROOT / "docs/native-http/deployment.json").write_text(json.dumps(record, indent=2)+"\n")
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()
