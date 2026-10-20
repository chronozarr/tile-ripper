"""Matched current Zarr-Python/xarray benchmark using library range coalescing.
PinnedStore adds only conditional HTTP transport; Zarr owns metadata, selections,
index decoding, coalescing, decompression and numeric interpretation.
"""

from __future__ import annotations
import hashlib
import asyncio
import itertools
import json
import os
import resource
import subprocess
import sys
import time
import zlib
from pathlib import Path
import aiohttp
import numpy as np
import xarray as xr
import zarr
from zarr.abc.store import Store, RangeByteRequest, OffsetByteRequest, SuffixByteRequest
from zarr.storage import WrapperStore, FsspecStore
from zarr.core.sync import sync
import zarr_v3_input as h


class PinnedStore(WrapperStore):
    def __init__(self, url, variables):
        super().__init__(FsspecStore.from_url(url, read_only=True))
        self.url = url
        self.variables = set(variables)
        self.versions = {}
        self.cached = {}
        self.session = None
        self.limit = None
        self.locks = {}

    async def client(self):
        if self.session is None:
            self.session = aiohttp.ClientSession(
                connector=aiohttp.TCPConnector(limit=4), auto_decompress=False
            )
            self.limit = asyncio.Semaphore(4)
        return self.session

    async def request(self, key, head=False, byte_range=None):
        client = await self.client()
        headers = {}
        old = self.versions.get(key, "unknown")
        if old != "unknown":
            headers["If-Match" if old is not None else "If-None-Match"] = (
                old[0] if old is not None else "*"
            )
        if byte_range is not None:
            if isinstance(byte_range, RangeByteRequest):
                headers["Range"] = f"bytes={byte_range.start}-{byte_range.end - 1}"
            elif isinstance(byte_range, SuffixByteRequest):
                headers["Range"] = f"bytes=-{byte_range.suffix}"
            elif isinstance(byte_range, OffsetByteRequest):
                headers["Range"] = f"bytes={byte_range.offset}-"
            else:
                raise ValueError(byte_range)
        async with self.limit:
            async with client.request(
                "HEAD" if head else "GET",
                self.url + key,
                headers=headers,
                allow_redirects=False,
            ) as r:
                if r.status == 404:
                    assert old in ("unknown", None)
                    self.versions[key] = None
                    return None
                assert r.status == (206 if byte_range is not None else 200), (
                    key,
                    r.status,
                )
                etag = r.headers["ETag"]
                assert etag.startswith('"') and not etag.startswith("W/")
                n = (
                    int(r.headers["Content-Range"].rsplit("/", 1)[1])
                    if byte_range
                    else int(r.headers["Content-Length"])
                )
                assert old == "unknown" or old == (etag, n), (key, "version conflict")
                self.versions[key] = (etag, n)
                return b"" if head else await r.read()

    async def pin(self, keys):
        await asyncio.gather(*(self.request(k, True) for k in keys))

    async def validate(self):
        await self.pin(list(self.versions))

    async def get(self, key, prototype, byte_range=None):
        cacheable = key.split("/")[0] not in self.variables
        signature = (key, repr(byte_range))
        if signature in self.cached:
            return (
                None
                if self.cached[signature] is None
                else prototype.buffer.from_bytes(self.cached[signature])
            )
        # Pinned shards/chunks bind index and payload requests to one version.
        if byte_range is not None and key not in self.versions:
            await self.request(key, True)
        data = await self.request(key, False, byte_range)
        if cacheable:
            self.cached[signature] = data
        return None if data is None else prototype.buffer.from_bytes(data)

    async def get_partial_values(self, prototype, key_ranges):
        return await asyncio.gather(*(self.get(k, prototype, r) for k, r in key_ranges))

    async def get_ranges(self, key, byte_ranges, *, prototype, **kwargs):
        async for batch in Store.get_ranges(
            self,
            key,
            byte_ranges,
            prototype=prototype,
            **{k: v for k, v in kwargs.items() if v is not None},
        ):
            yield batch

    async def shutdown(self):
        if self.session:
            await self.session.close()


def reference(r):
    zarr.config.set(
        {
            "async.concurrency": 4,
            "threading.max_workers": 4,
            "array.sharding_coalesce_max_gap_bytes": 1048576,
            "array.sharding_coalesce_max_bytes": 16777216,
        }
    )
    start = time.perf_counter()
    variables = [v["path"] for v in r["variables"]]
    store = PinnedStore(r["source"]["assets"][0]["locator"], variables)
    g = zarr.open_group(store, mode="r", use_consolidated=True)
    # xarray supplies dimension metadata without dask or automatic calibration.
    ds = xr.open_zarr(store, consolidated=True, decode_cf=False, chunks=None)
    idx = h.selection(r, g)
    objects = []
    for name in variables:
        a = g[name]
        shape = a.shards or a.chunks
        dims = a.metadata.dimension_names
        for key in itertools.product(
            *(sorted(set((idx[d] // c).tolist())) for d, c in zip(dims, shape))
        ):
            objects.append(name + "/" + a.metadata.encode_chunk_key(key))
    sync(store.pin(objects))
    sync(store.validate())
    sync(store.validate())
    prepare = time.perf_counter() - start
    first = None
    valid = 0
    total = 0.0
    work = 0
    arrays = {}
    # One oindex selection per variable enables the library's coalesced shard reads.
    for name in variables:
        a = g[name]
        dims = ds[name].dims
        array = ds[name].isel({d: idx[d] for d in dims}).values
        arrays[name] = array
        if first is None:
            first = time.perf_counter() - start
        mask = ~np.isnan(array)
        valid += int(mask.sum())
        total += float(np.sum(array[mask], dtype=np.float64))
        work += array.nbytes
        # Same native-bit and mask checksum work as the Rust validation consumer.
        zlib.adler32(np.ascontiguousarray(array).tobytes())
        zlib.adler32(mask.astype("u1").tobytes())
    # Native CRS and exact selected coordinate/time metadata are consumed too.
    coordinates = {n: g[n][idx[n]].tolist() for n in ["x", "y", "time"]}
    sync(store.validate())
    seconds = time.perf_counter() - start
    sync(store.shutdown())
    return {
        "status": "complete",
        "seconds": seconds,
        "prepare_seconds": prepare,
        "first_variable_seconds": first,
        "valid_pixels": valid,
        "sum": total,
        "decoded_output_bytes": work,
        "coordinate_counts": {k: len(v) for k, v in coordinates.items()},
        "peak_rss_bytes": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
        "versions": len(store.versions),
        "tuning": {
            "concurrency": 4,
            "threads": 4,
            "coalescing_gap": 1048576,
            "coalescing_max": 16777216,
        },
        "consistency": "strong ETag pin, conditional GET, pre/post HEAD all selected metadata/coordinates/data; no atomic dataset snapshot",
        "materialization": "native arrays retained in memory; no file output",
    }


def measure(cmd, env):
    import re

    start = time.perf_counter()
    p = subprocess.run(
        ["/usr/bin/time", "-l", *map(str, cmd)], env=env, capture_output=True, text=True
    )
    if p.returncode:
        raise RuntimeError(p.stderr[-4000:])
    v = json.loads(p.stdout)
    v["process_wall_seconds"] = time.perf_counter() - start
    m = re.search(r"(\d+)\s+maximum resident set size", p.stderr)
    v["process_peak_rss_bytes"] = int(m.group(1)) if m else None
    return v


def benchmark(repetitions=3):
    work = h.WORK / "benchmark"
    work.mkdir(parents=True, exist_ok=True)
    rows = []
    env = {
        **os.environ,
        "TILE_RIPPER_WORKERS": "4",
        "TILE_RIPPER_PER_HOST": "4",
        "OMP_NUM_THREADS": "1",
        "OPENBLAS_NUM_THREADS": "1",
    }
    shape = (16, 256, 256)
    for sharded in [False, True]:
        source = h.fixture(
            work / ("sharded" if sharded else "ordinary"),
            sharded,
            "blosc-zstd",
            shape=shape,
            inner=(2, 32, 32),
            factor=(2, 4, 4),
            consolidate=True,
        )
        for dense in [False, True]:
            for link in ["local-http", "8ms-100Mbps"]:
                server = h.Server(
                    source,
                    latency=0.008 if link.startswith("8") else 0,
                    bandwidth=12_500_000 if link.startswith("8") else 0,
                ).start()
                r = h.request(server.url, work / "unused", shape=shape, dense=dense)
                r["budget"]["chunk_edge"] = 32
                if not dense:
                    r["spatial"]["bbox"] = [10, 10, 95, 95]
                requestfile = work / "request.json"
                requestfile.write_text(json.dumps(r))
                try:
                    for rep in range(repetitions):
                        for reader in (
                            ["tile-ripper", "xarray-zarr"]
                            if rep % 2 == 0
                            else ["xarray-zarr", "tile-ripper"]
                        ):
                            server.records.clear()
                            server.next_byte = 0.0
                            cmd = (
                                [h.BINARY, requestfile, "stream"]
                                if reader == "tile-ripper"
                                else [sys.executable, Path(__file__), requestfile]
                            )
                            result = measure(cmd, env)
                            traffic = list(server.records)
                            gets = [v for v in traffic if v["method"] == "GET"]
                            data = [
                                v
                                for v in gets
                                if v["key"].split("/")[0]
                                in ["signal", "temperature", "mask"]
                            ]
                            indexes = [
                                v
                                for v in data
                                if v["range"] and v["range"].startswith("bytes=-")
                            ]
                            # Tile-Ripper emits an absolute range for the fixed suffix.
                            if reader == "tile-ripper" and sharded:
                                index_size = (
                                    16
                                    * int(
                                        np.prod(np.array([4, 128, 128]) // [2, 32, 32])
                                    )
                                    + 4
                                )
                                indexes = [v for v in data if v["bytes"] == index_size]
                            payload = [v for v in data if v not in indexes]
                            record = {
                                "layout": "sharded" if sharded else "ordinary",
                                "selection": "dense" if dense else "sparse",
                                "link": link,
                                "rep": rep,
                                "reader": reader,
                                "seconds": result["seconds"],
                                "process_wall_seconds": result["process_wall_seconds"],
                                "peak_rss_bytes": result["process_peak_rss_bytes"],
                                "requests": len(traffic),
                                "heads": len(traffic) - len(gets),
                                "gets": len(gets),
                                "body_bytes": sum(v["bytes"] for v in gets),
                                "index_requests": len(indexes),
                                "index_bytes": sum(v["bytes"] for v in indexes),
                                "payload_requests": len(payload),
                                "payload_bytes": sum(v["bytes"] for v in payload),
                                "full_shard_gets": sum(v["range"] is None for v in data)
                                if sharded
                                else 0,
                                "duplicate_ranges": sum(
                                    n - 1
                                    for n in h.Counter(
                                        (v["key"], v["range"]) for v in data
                                    ).values()
                                ),
                            }
                            if reader == "tile-ripper":
                                rr = result["runs"][0]
                                record.update(
                                    valid=rr["valid_pixels"],
                                    sum=rr["sum"],
                                    first_chunk_seconds=rr["first_chunk_seconds"],
                                    scheduler=h.scheduler_summary(
                                        rr["observed"]["scheduler"]
                                    ),
                                    read_decode_seconds=rr["observed"][
                                        "read_decode_seconds"
                                    ],
                                    retrieval_seconds=rr["observed"][
                                        "source_retrieval_seconds"
                                    ],
                                    output_bytes=rr["observed"]["decoded_bytes"],
                                )
                            else:
                                record.update(
                                    valid=result["valid_pixels"],
                                    sum=result["sum"],
                                    first_variable_seconds=result[
                                        "first_variable_seconds"
                                    ],
                                    output_bytes=result["decoded_output_bytes"],
                                )
                            label = f"{record['layout']}-{record['selection']}-{link}-{rep}-{reader}"
                            (work / (label + ".json")).write_text(json.dumps(result))
                            (work / (label + "-http.json")).write_text(
                                json.dumps(traffic)
                            )
                            rows.append(record)
                            print(
                                label,
                                round(record["seconds"], 3),
                                record["body_bytes"],
                                flush=True,
                            )
                    group = rows[-repetitions * 2 :]
                    ref = group[0]
                    assert all(
                        v["valid"] == ref["valid"]
                        and np.isclose(v["sum"], ref["sum"], rtol=1e-12)
                        and v["output_bytes"] == ref["output_bytes"]
                        for v in group
                    )
                finally:
                    server.shutdown()
                    server.server_close()
    (h.DOCS / "benchmark.json").write_text(
        json.dumps(
            {
                "repetitions": repetitions,
                "tile_ripper_binary_sha256": hashlib.sha256(
                    h.BINARY.read_bytes()
                ).hexdigest(),
                "fixture_shape": [16, 256, 256],
                "inner_chunks": [2, 32, 32],
                "shards": [4, 128, 128],
                "zarr": zarr.__version__,
                "xarray": xr.__version__,
                "numpy": np.__version__,
                "rows": rows,
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    if sys.argv[1] == "benchmark":
        benchmark(int(sys.argv[2]) if len(sys.argv) > 2 else 3)
    else:
        print(json.dumps(reference(json.loads(Path(sys.argv[1]).read_text()))))
