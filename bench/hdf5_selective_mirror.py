"""Capped, identity-pinned range mirror. Replay refuses every uncaptured byte.

The sparse backing file has the provider's logical size. Its holes are never
served by this HTTP interface; the manifest records downloaded extents.
"""

import contextlib
import hashlib
import io
import json
import socket
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import requests


class OfficialRanges(io.RawIOBase):
    """Bounded research inspection through an archive's official GET redirects.

    Requests may use an existing Earthdata configuration. No credentials or
    signed redirect URLs are persisted. This is reference tooling, not a native
    transport adapter or a change to Tile-Ripper's redirect policy.
    """

    def __init__(self, url, cap=512 * 1024):
        self.url, self.cap, self.pos, self.events = url, cap, 0, []
        self.session = requests.Session()
        response = self.session.head(url, allow_redirects=True, timeout=30)
        response.raise_for_status()
        self.size = int(response.headers["Content-Length"])
        self.etag = response.headers["ETag"]
        assert self.etag.startswith('"') and self.etag.endswith('"')

    def readable(self):
        return True

    def seekable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, offset, whence=0):
        self.pos = (
            offset
            if whence == 0
            else self.pos + offset
            if whence == 1
            else self.size + offset
        )
        return self.pos

    def readinto(self, buffer):
        size, start = len(buffer), self.pos
        if not size:
            return 0
        if sum(e["bytes"] for e in self.events) + size > self.cap:
            raise ValueError("explicit product metadata inspection cap exceeded")
        assert 0 <= start < start + size <= self.size
        with self.session.get(
            self.url,
            headers={
                "Range": f"bytes={start}-{start + size - 1}",
                "If-Match": self.etag,
                "Accept-Encoding": "identity",
            },
            allow_redirects=True,
            stream=True,
            timeout=30,
        ) as response:
            if (
                response.status_code != 206
                or response.headers.get("ETag") != self.etag
                or response.headers.get("Content-Range")
                != f"bytes {start}-{start + size - 1}/{self.size}"
            ):
                raise ValueError(
                    f"conditional product range rejected: HTTP {response.status_code}"
                )
            data = response.raw.read(size + 1)
            assert len(data) == size
            self.events.append(
                {
                    "start": start,
                    "bytes": size,
                    "redirect_statuses": [r.status_code for r in response.history],
                }
            )
        buffer[:] = data
        self.pos += size
        return size

    def validate(self):
        response = self.session.head(
            self.url, headers={"If-Match": self.etag}, allow_redirects=True, timeout=30
        )
        response.raise_for_status()
        assert (
            response.headers["ETag"] == self.etag
            and int(response.headers["Content-Length"]) == self.size
        )

    def close(self):
        self.session.close()
        super().close()


class Mirror:
    def __init__(self, directory, url=None, cap=16 * 1024 * 1024, session=None):
        self.directory = Path(directory)
        self.directory.mkdir(parents=True, exist_ok=True)
        self.path = self.directory / "source.nc"
        self.manifest = self.directory / "manifest.json"
        self.session = session if session is not None else requests.Session()
        self.lock = threading.Lock()
        self.capture = url is not None
        if self.manifest.exists():
            self.info = json.loads(self.manifest.read_text())
            if url is not None:
                assert url == self.info["url"]
                r = self.session.head(
                    url, headers={"If-Match": self.info["etag"]}, timeout=30
                )
                r.raise_for_status()
                assert r.headers["ETag"] == self.info["etag"]
                assert int(r.headers["Content-Length"]) == self.info["size"]
        else:
            assert url is not None
            r = self.session.head(url, timeout=30)
            r.raise_for_status()
            etag = r.headers["ETag"]
            assert etag.startswith('"') and etag.endswith('"')
            self.info = {
                "url": url,
                "etag": etag,
                "size": int(r.headers["Content-Length"]),
                "cap": cap,
                "extents": [],
                "provider_gets": 0,
                "provider_bytes": 0,
            }
            with self.path.open("wb") as f:
                f.truncate(self.info["size"])
        self.extents = [tuple(x) for x in self.info["extents"]]

    def missing(self, start, end):
        gaps = []
        for a, b in self.extents:
            if b <= start:
                continue
            if a >= end:
                break
            if a > start:
                gaps.append((start, a))
            start = max(start, b)
        if start < end:
            gaps.append((start, end))
        return gaps

    def read(self, start, size):
        end = start + size
        assert 0 <= start < end <= self.info["size"]
        with self.lock:
            for a, b in self.missing(start, end):
                if not self.capture:
                    raise ValueError(f"uncaptured mirror range {a}:{b}")
                if self.info["provider_bytes"] + b - a > self.info["cap"]:
                    raise ValueError("explicit mirror transfer cap exceeded")
                r = self.session.get(
                    self.info["url"],
                    headers={
                        "Range": f"bytes={a}-{b - 1}",
                        "If-Match": self.info["etag"],
                        "Accept-Encoding": "identity",
                    },
                    timeout=30,
                    stream=True,
                )
                assert r.status_code == 206, r.status_code
                assert r.headers["ETag"] == self.info["etag"]
                assert (
                    r.headers["Content-Range"]
                    == f"bytes {a}-{b - 1}/{self.info['size']}"
                )
                assert r.headers.get("Content-Encoding", "identity") == "identity"
                content = r.raw.read(b - a + 1)
                assert len(content) == b - a
                r.close()
                with self.path.open("r+b") as f:
                    f.seek(a)
                    f.write(content)
                self.info["provider_gets"] += 1
                self.info["provider_bytes"] += b - a
                merged = []
                for x, y in sorted(self.extents + [(a, b)]):
                    if merged and x <= merged[-1][1]:
                        merged[-1] = (merged[-1][0], max(y, merged[-1][1]))
                    else:
                        merged.append((x, y))
                self.extents = merged
                self.save()
            with self.path.open("rb") as f:
                f.seek(start)
                return f.read(size)

    def save(self):
        self.info["extents"] = self.extents
        self.manifest.write_text(json.dumps(self.info, indent=2) + "\n")

    def seal(self):
        # Revalidate against the provider before sealing a new capture.
        if self.capture:
            r = self.session.head(
                self.info["url"], headers={"If-Match": self.info["etag"]}, timeout=30
            )
            r.raise_for_status()
            assert r.headers["ETag"] == self.info["etag"]
            assert int(r.headers["Content-Length"]) == self.info["size"]
        self.info["extent_sha256"] = []
        for a, b in self.extents:
            with self.path.open("rb") as f:
                f.seek(a)
                self.info["extent_sha256"].append(
                    [a, b, hashlib.sha256(f.read(b - a)).hexdigest()]
                )
        self.save()
        self.capture = False

    def verify(self):
        for a, b, digest in self.info["extent_sha256"]:
            assert hashlib.sha256(self.read(a, b - a)).hexdigest() == digest


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def do_HEAD(self):
        self.serve(True)

    def do_GET(self):
        self.serve(False)

    def serve(self, head):
        s = self.server
        if s.latency:
            time.sleep(s.latency)
        a, b = 0, s.mirror.info["size"]
        status = 200 if head else 206
        data = b""
        if self.headers.get("If-Match") not in [None, s.mirror.info["etag"]]:
            status = 412
        if not head and status == 206:
            try:
                r = self.headers["Range"]
                a, z = map(int, r.removeprefix("bytes=").split("-"))
                b = z + 1
                # Full retrieval is only a separately labelled small-file control.
                data = s.mirror.read(a, b - a)
            except (AssertionError, KeyError, ValueError, OSError) as error:
                s.errors.append(str(error))
                status = 416
        self.send_response(status)
        self.send_header("ETag", s.mirror.info["etag"])
        self.send_header("Accept-Ranges", "bytes")
        if not head and status == 206:
            self.send_header(
                "Content-Range", f"bytes {a}-{b - 1}/{s.mirror.info['size']}"
            )
        self.send_header(
            "Content-Length", str(s.mirror.info["size"] if head else len(data))
        )
        self.end_headers()
        if not head and data:
            self.wfile.write(data)
        s.events.append(
            {
                "method": "HEAD" if head else "GET",
                "status": status,
                "start": a,
                "end": b,
                "bytes": len(data),
                "conditional": self.headers.get("If-Match") == s.mirror.info["etag"],
            }
        )


@contextlib.contextmanager
def server(mirror, latency=0):
    class Server(ThreadingHTTPServer):
        daemon_threads = True

        def get_request(self):
            c, a = super().get_request()
            c.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            return c, a

    s = Server(("127.0.0.1", 0), Handler)
    s.mirror, s.latency, s.events, s.errors = mirror, latency, [], []
    s.url = f"http://127.0.0.1:{s.server_port}/source.nc"
    thread = threading.Thread(target=s.serve_forever, daemon=True)
    thread.start()
    try:
        yield s
    finally:
        s.shutdown()
        s.server_close()
        thread.join()
