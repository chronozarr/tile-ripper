"""Isolated h5py/libhdf5 reference with bounded exact or paged remote reads."""

import hashlib
import io
import json
import resource
import sys
import time
from pathlib import Path

import h5py
import numpy as np
import requests
from hdf5_reference import Remote


def main(request):
    started, cpu = time.perf_counter(), time.process_time()
    spatial = request["spatial"]
    selected = tuple(
        slice(a, a + n * s, s)
        for a, n, s in zip(spatial["start"], spatial["count"], spatial["step"])
    )
    remote, session, events, heads = None, None, [], 0
    if "local" in request:
        source = request["local"]
    elif request.get("full"):
        session = requests.Session()
        r = session.head(request["url"], timeout=30)
        r.raise_for_status()
        size, etag = int(r.headers["Content-Length"]), r.headers["ETag"]
        assert size <= 2 * 1024 * 1024, "full-file control is limited to small files"
        r = session.get(
            request["url"],
            headers={"Range": f"bytes=0-{size - 1}", "If-Match": etag},
            timeout=30,
        )
        assert r.status_code == 206 and r.headers["ETag"] == etag
        assert r.headers["Content-Range"] == f"bytes 0-{size - 1}/{size}"
        assert len(r.content) == size
        source = io.BytesIO(r.content)
        events, heads = [(0, size)], 2
    else:
        remote = Remote(request["url"], request.get("page", 0))
        source, heads = remote, 2
    with h5py.File(source, "r", rdcc_nbytes=2 * 1024 * 1024, rdcc_nslots=521) as f:
        ds = f[request["dataset"]]
        attrs = dict(ds.attrs)
        axes = []
        for path, sl in zip(spatial["axes"], selected):
            axis = f[path]
            _ = dict(axis.attrs)
            axes.append(axis[sl].astype("<f8"))
        if "grid_mapping" in attrs:
            name = np.asarray(attrs["grid_mapping"]).item()
            if isinstance(name, bytes):
                name = name.decode()
            parent = request["dataset"].rsplit("/", 1)[0]
            _ = dict(f[parent + "/" + name].attrs)
        if "t" in f:
            _ = f["/t"][()]
            _ = dict(f["/t"].attrs)
        prepared = time.perf_counter() - started
        data = ds[selected].astype("<u2" if ds.dtype.kind == "u" else "<f4")
        first = time.perf_counter() - started
        mask = ~np.isnan(data)
        for key in ["_FillValue", "missing_value"]:
            if key in attrs:
                mask &= data != np.asarray(attrs[key]).item()
        digest = hashlib.sha256(data.tobytes()).hexdigest()
        value_sum = float(data[mask].astype("f8").sum())
    if remote:
        etag, size = remote.etag, remote.size
        session = remote.session
        events = remote.events
    if session:
        r = session.head(request["url"], headers={"If-Match": etag}, timeout=30)
        r.raise_for_status()
        assert r.headers["ETag"] == etag and int(r.headers["Content-Length"]) == size
    if remote:
        remote.close()
    elif session:
        session.close()
    return {
        "reader": "h5py/libhdf5",
        "page_bytes": request.get("page", 0),
        "full_file_control": request.get("full", False),
        "prepare_seconds": prepared,
        "first_chunk_seconds": first,
        "completion_seconds": time.perf_counter() - started,
        "cpu_seconds": time.process_time() - cpu,
        "peak_rss_bytes": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        * (1 if sys.platform == "darwin" else 1024),
        "gets": len(events),
        "heads": heads,
        "bytes": sum(b - a for a, b in events),
        "events": events,
        "value_sha256": digest,
        "valid_pixels": int(mask.sum()),
        "stored_value_sum": value_sum,
        "values": int(data.size),
        "h5py": h5py.__version__,
        "hdf5": h5py.version.hdf5_version,
    }


if __name__ == "__main__":
    print(json.dumps(main(json.loads(Path(sys.argv[1]).read_text()))))
