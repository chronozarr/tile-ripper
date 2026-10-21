"""Isolated mature-reader performance worker. No native acquisition code."""

import hashlib
import io
import json
import resource
import sys
import time
from pathlib import Path

import h5py
import requests


class Remote(io.RawIOBase):
    """An exact-range h5py reference; optional bounded pages tune metadata I/O."""

    def __init__(self, url, page=0):
        self.url = url
        self.pos = 0
        self.session = requests.Session()
        self.page = page
        self.cache = {}
        self.events = []
        r = self.session.head(url, timeout=30)
        r.raise_for_status()
        self.size = int(r.headers["Content-Length"])
        self.etag = r.headers["ETag"]
        self.heads = 1

    def readable(self):
        return True

    def seekable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, n, w=0):
        self.pos = n if w == 0 else self.pos + n if w == 1 else self.size + n
        return self.pos

    def readinto(self, b):
        n = len(b)
        if not n:
            return 0
        if n > 4 * 1024 * 1024:
            raise ValueError("reference read exceeds explicit 4 MiB cap")
        if self.page and n <= self.page:
            start = self.pos // self.page * self.page
            end = min(
                self.size, ((self.pos + n + self.page - 1) // self.page) * self.page
            )
        else:
            start = self.pos
            end = self.pos + n
        key = (start, end)
        if key not in self.cache:
            r = self.session.get(
                self.url,
                headers={"Range": f"bytes={start}-{end - 1}", "If-Match": self.etag},
                timeout=30,
            )
            assert (
                r.status_code == 206
                and r.headers["ETag"] == self.etag
                and r.headers["Content-Range"] == f"bytes {start}-{end - 1}/{self.size}"
            )
            data = r.content
            assert len(data) == end - start
            self.events.append((start, end))
            if self.page:
                if (
                    sum(len(v) for v in self.cache.values()) + len(data)
                    > 2 * 1024 * 1024
                ):
                    self.cache.clear()
                self.cache[key] = data
        else:
            data = self.cache[key]
        b[:] = data[self.pos - start : self.pos - start + n]
        self.pos += n
        return n

    def close(self):
        self.session.close()
        super().close()


def main():
    request = json.loads(Path(sys.argv[1]).read_text())
    url = request["url"]
    path = request["dataset"]
    selection = request["slice"]
    page = request.get("page", 0)
    selected = tuple(
        slice(a, a + n * s, s)
        for a, n, s in zip(selection["start"], selection["count"], selection["step"])
    )
    start = time.perf_counter()
    cpu = time.process_time()
    remote = None
    if request.get("full"):
        session = requests.Session()
        identity = session.head(url, timeout=30)
        identity.raise_for_status()
        size = int(identity.headers["Content-Length"])
        etag = identity.headers["ETag"]
        response = session.get(
            url, headers={"Range": f"bytes=0-{size - 1}", "If-Match": etag}, timeout=30
        )
        response.raise_for_status()
        assert len(response.content) == size
        source = io.BytesIO(response.content)
    else:
        remote = Remote(url, page=page)
        source = remote
    with h5py.File(source, "r", rdcc_nbytes=2 * 1024 * 1024, rdcc_nslots=521) as f:
        ds = f[path]
        _ = dict(ds.attrs)
        parent = path.rsplit("/", 1)[0]
        coords = []
        for axis, axis_slice in zip(["y", "x"], selected):
            coord = f[parent + "/" + axis]
            _ = dict(coord.attrs)
            coords.append(coord[axis_slice].astype("<f8"))
        prepared = time.perf_counter() - start
        a = ds[selected].astype("<u2")
        first = time.perf_counter() - start
        digest = hashlib.sha256(a.tobytes()).hexdigest()
    if remote:
        validation = remote.session.head(
            url, headers={"If-Match": remote.etag}, timeout=30
        )
        validation.raise_for_status()
        assert validation.headers["ETag"] == remote.etag
        gets = len(remote.events)
        heads = 2
        count = sum(b - a for a, b in remote.events)
        largest = max((b - a for a, b in remote.events), default=0)
        remote.close()
    else:
        validation = session.head(url, headers={"If-Match": etag}, timeout=30)
        validation.raise_for_status()
        assert validation.headers["ETag"] == etag
        session.close()
        gets = 1
        heads = 2
        count = size
        largest = size
    out = {
        "reader": "explicit-full-file-h5py-control"
        if request.get("full")
        else "h5py/libhdf5 range fileobj",
        "page_bytes": page,
        "completion_seconds": time.perf_counter() - start,
        "initial_metadata_coordinate_seconds": prepared,
        "first_scientific_values_seconds": first,
        "cpu_seconds": time.process_time() - cpu,
        "values": a.size,
        "value_sha256": digest,
        "coordinate_sha256": [hashlib.sha256(v.tobytes()).hexdigest() for v in coords],
        "gets": gets,
        "heads": heads,
        "bytes": count,
        "max_request_bytes": largest,
        "peak_rss_bytes": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        * (1 if sys.platform == "darwin" else 1024),
    }
    print(json.dumps(out))


if __name__ == "__main__":
    main()
