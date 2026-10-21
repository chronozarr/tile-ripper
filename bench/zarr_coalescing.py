"""Independent science, transport, resource and matched range-policy experiments.
All Zarr interpretation/repacking uses Zarr-Python (including its index codec).
Raw cells/failed runs stay in ignored prototype-output; compact receipts in docs.
"""

from __future__ import annotations
import hashlib
import itertools
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import threading
import numpy as np
import zarr
from zarr.core.buffer.cpu import Buffer
from zarr.codecs.sharding import _ShardIndex
from zarr.core.sync import sync
import zarr_v3_input as h
import zarr_v3_compare as ref

ROOT = h.ROOT
WORK = ROOT / "prototype-output/zarr-coalescing"
DOCS = ROOT / "docs/zarr-coalescing"
BASE = WORK / "baseline-fe6fcd1"
BINARY = h.BINARY
ENV = {
    **os.environ,
    "TILE_RIPPER_WORKERS": "4",
    "TILE_RIPPER_PER_HOST": "4",
    "OPENBLAS_NUM_THREADS": "1",
    "OMP_NUM_THREADS": "1",
}
SHAPE = (16, 256, 256)
VARS = {"signal", "temperature", "mask"}


class Handler(h.Handler):
    def serve(self, head):
        start = time.perf_counter()
        if self.path == "/control/wait-payload":
            ok = self.server.payload_started.wait(15)
            self.send_response(200 if ok else 500)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if not head and self.headers.get("Range"):
            key = self.path.removeprefix("/store/")
            if key.split("/")[0] in VARS:
                text = self.headers["Range"].removeprefix("bytes=")
                if not text.startswith("-"):
                    a, b = map(int, text.split("-"))
                    # Benchmark and extended fault sources use 516-byte end indices.
                    is_payload = b + 1 != len(self.server.cache.get(key, b""))
                    if is_payload:
                        self.server.payload_started.set()
                        if (
                            self.server.payload_fault
                            and not self.server.fault_triggered
                        ):
                            with self.server.lock:
                                if not self.server.fault_triggered:
                                    self.server.fault_triggered = True
                                    action = self.server.payload_fault
                                    if action == "mutate":
                                        data = bytearray(self.server.cache[key])
                                        data[0] ^= 1
                                        self.server.cache[key] = bytes(data)
                                    else:
                                        self.server.fault[("GET", key)] = list(action)
                        if self.server.payload_delay:
                            time.sleep(self.server.payload_delay)
        try:
            super().serve(head)
        finally:
            with self.server.lock:
                self.server.activity.append(
                    {
                        "method": self.command,
                        "key": self.path.removeprefix("/store/"),
                        "range": self.headers.get("Range"),
                        "start": start,
                        "end": time.perf_counter(),
                    }
                )


class Server(h.Server):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.RequestHandlerClass = Handler
        self.activity = []
        self.payload_started = threading.Event()
        self.payload_fault = None
        self.fault_triggered = False
        self.payload_delay = 0
        self.corrupt_key = None


def repack(source, gap=0, reverse=False):
    """Reorder encoded members with the library index decoder/encoder; no codec rewrite."""
    group = zarr.open_group(str(source), mode="r", use_consolidated=False)
    for name in VARS:
        a = group[name]
        codec = a.metadata.codecs[0]
        cps = tuple(o // i for o, i in zip(a.shards, a.chunks))
        index_size = int(np.prod(cps)) * 16 + (4 if len(codec.index_codecs) == 2 else 0)
        for p in sorted((source / name / "c").rglob("*")):
            if not p.is_file():
                continue
            data = p.read_bytes()
            at_start = codec.index_location == "start"
            encoded = data[:index_size] if at_start else data[-index_size:]
            index = sync(codec._decode_shard_index(Buffer.from_bytes(encoded), cps))
            index = _ShardIndex(cps, index.offsets_and_lengths.copy())
            members = [
                idx for idx in np.ndindex(cps) if index.get_chunk_slice(idx) is not None
            ]
            if reverse:
                members.reverse()
            payload = bytearray(index_size if at_start else 0)
            slices = {idx: index.get_chunk_slice(idx) for idx in members}
            for n, idx in enumerate(members):
                if n:
                    payload.extend(bytes(gap))
                oldstart, oldend = slices[idx]
                start = len(payload)
                payload.extend(data[oldstart:oldend])
                index.set_chunk_slice(idx, slice(start, len(payload)))
            encoded = sync(codec._encode_shard_index(index)).to_bytes()
            if at_start:
                payload[:index_size] = encoded
            else:
                payload.extend(encoded)
            p.write_bytes(payload)
    return source


def layouts():
    wide = WORK / "wide"
    if not (wide / "layout-complete").exists():
        h.fixture(
            wide,
            True,
            "raw",
            shape=SHAPE,
            inner=(2, 64, 64),
            factor=(2, 4, 4),
            consolidate=True,
        )
        repack(wide, gap=131072, reverse=True)
        (wide / "layout-complete").write_text(
            "library index repack, gap=131072, reverse=True"
        )
    return [
        ("ordinary", BASE / "ordinary"),
        ("sharded", BASE / "sharded"),
        ("wide", wide),
    ]


def measure(cmd, env):
    p = subprocess.run(
        ["/usr/bin/time", "-l", *map(str, cmd)], env=env, capture_output=True, text=True
    )
    if p.returncode:
        return {"error": p.stderr[-5000:], "code": p.returncode}
    value = json.loads(p.stdout)
    m = re.search(r"(\d+)\s+maximum resident set size", p.stderr)
    value["process_peak_rss_bytes"] = int(m.group(1)) if m else None
    m = re.search(r"([\d.]+)\s+user\s+([\d.]+)\s+sys", p.stderr)
    value["cpu_seconds"] = sum(map(float, m.groups())) if m else None
    return value


def traffic(server, source, sharded):
    data = [
        r
        for r in server.records
        if r["method"] == "GET" and r["key"].split("/")[0] in VARS
    ]
    indexes = []
    if sharded:
        g = zarr.open_group(str(source), mode="r")
        for r in data:
            a = g[r["key"].split("/")[0]]
            codec = a.metadata.codecs[0]
            n = int(np.prod(np.array(a.shards) // a.chunks)) * 16 + (
                4 if len(codec.index_codecs) == 2 else 0
            )
            end = len(server.cache[r["key"]])
            if r["range"] and (
                r["range"].startswith("bytes=-")
                or tuple(r["span"] or ()) == (end - n, end)
            ):
                indexes.append(r)
    payload = [r for r in data if r not in indexes]
    events = []
    signatures = {(v["key"], v["range"]) for v in payload}
    for v in server.activity:
        if v["method"] == "GET" and (v["key"], v["range"]) in signatures:
            events.extend([(v["start"], 1), (v["end"], -1)])
    current = peak = 0
    for _, d in sorted(events):
        current += d
        peak = max(peak, current)
    gets = [v for v in server.records if v["method"] == "GET"]
    return {
        "heads": len(server.records) - len(gets),
        "gets": len(gets),
        "body_bytes": sum(v["bytes"] for v in gets),
        "index_requests": len(indexes),
        "index_bytes": sum(v["bytes"] for v in indexes),
        "payload_requests": len(payload),
        "payload_bytes": sum(v["bytes"] for v in payload),
        "full_shard_gets": sum(v["range"] is None for v in data) if sharded else 0,
        "payload_concurrency_peak": peak,
        "duplicate_ranges": sum(
            n - 1 for n in h.Counter((v["key"], v["range"]) for v in data).values()
        ),
    }


def benchmark(repetitions=3, pilot=False, hints=False):
    name = "hints" if hints else "policy-pilot" if pilot else "benchmark"
    work = WORK / name
    work.mkdir(parents=True, exist_ok=True)
    rows = []
    binary_hash = hashlib.sha256(BINARY.read_bytes()).hexdigest()
    for layout, source in layouts():
        if hints and layout != "wide":
            continue
        if pilot and layout == "ordinary":
            continue
        for dense, link in itertools.product(
            [True] if hints else [False, True],
            ["8ms-100Mbps", "40ms-1000Mbps"]
            if hints
            else ["local-http", "8ms-100Mbps"]
            if not pilot
            else ["local-http", "8ms-100Mbps", "40ms-1000Mbps"],
        ):
            server = Server(
                source,
                latency=0.04
                if link.startswith("40")
                else 0.008
                if link.startswith("8")
                else 0,
                bandwidth=125_000_000
                if link.startswith("40")
                else 12_500_000
                if link.startswith("8")
                else 0,
            ).start()
            r = h.request(server.url, work / "unused", shape=SHAPE, dense=dense)
            r["budget"]["chunk_edge"] = 32
            if not dense:
                r["spatial"]["bbox"] = [10, 10, 95, 95]
            file = work / "request.json"
            file.write_text(json.dumps(r))
            try:
                readers = (
                    ["cost", "hinted-cost", "fixed", "python"]
                    if hints
                    else ["individual", "adjacent", "fixed", "cost", "python"]
                    if pilot
                    else ["baseline", "cost", "python"]
                )
                for rep in range(repetitions):
                    order = (
                        readers[rep % len(readers) :] + readers[: rep % len(readers)]
                    )
                    for reader in order:
                        server.records.clear()
                        server.activity.clear()
                        server.next_byte = 0.0
                        binary = (
                            BASE / "zarr_subset_oracle"
                            if reader == "baseline"
                            else BINARY
                        )
                        cmd = (
                            [sys.executable, Path(ref.__file__), file]
                            if reader == "python"
                            else [binary, file, "stream"]
                        )
                        env = {
                            **ENV,
                            "TILE_RIPPER_ZARR_COALESCE": reader
                            if reader in ["individual", "adjacent", "fixed", "cost"]
                            else "cost",
                        }
                        if reader == "hinted-cost":
                            env.update(
                                TILE_RIPPER_ZARR_LATENCY_MS="40"
                                if link.startswith("40")
                                else "8",
                                TILE_RIPPER_ZARR_BANDWIDTH_MBPS="125"
                                if link.startswith("40")
                                else "12.5",
                                TILE_RIPPER_ZARR_MAX_GAP="1048576",
                                TILE_RIPPER_ZARR_OVERFETCH_RATIO="8",
                            )
                        result = measure(cmd, env)
                        label = f"{layout}-{dense}-{link}-{rep}-{reader}"
                        (work / (label + ".json")).write_text(json.dumps(result))
                        (work / (label + "-http.json")).write_text(
                            json.dumps(server.records)
                        )
                        row = {
                            "layout": layout,
                            "dense": dense,
                            "link": link,
                            "rep": rep,
                            "reader": reader,
                            "raw": str(work / (label + ".json")),
                        }
                        if "error" in result:
                            row.update(result)
                            rows.append(row)
                            print(label, "FAIL", result["error"][-500:], flush=True)
                            continue
                        row.update(
                            traffic(server, source, layout != "ordinary"),
                            seconds=result["seconds"],
                            cpu_seconds=result["cpu_seconds"],
                            peak_rss_bytes=result["process_peak_rss_bytes"],
                        )
                        if reader == "python":
                            row.update(
                                valid=result["valid_pixels"],
                                sum=result["sum"],
                                output_bytes=result["decoded_output_bytes"],
                                first_seconds=result["first_variable_seconds"],
                            )
                        else:
                            rr = result["runs"][0]
                            observed = rr["observed"]
                            s = observed["scheduler"]
                            checked = h.verify(r, source, result)
                            row.update(
                                valid=rr["valid_pixels"],
                                sum=rr["sum"],
                                output_bytes=observed["decoded_bytes"],
                                first_seconds=rr["first_chunk_seconds"],
                                reservation=s["reserved_bytes_peak"],
                                active_workers=s["active_workers_peak"],
                                read_decode_seconds=observed["read_decode_seconds"],
                                retrieval_seconds=observed["source_retrieval_seconds"],
                                decode_adapter_seconds=max(
                                    0,
                                    observed["read_decode_seconds"]
                                    - observed["source_retrieval_seconds"],
                                ),
                                ranges=s.get("zarr_v3_ranges"),
                                science=checked,
                                selected_bytes=result["inspection"]["plan"][
                                    "estimates"
                                ]["selected_compressed_payload_bytes"],
                            )
                            row["overfetch_bytes"] = (
                                row["payload_bytes"] - row["selected_bytes"]
                            )
                            assert row["reservation"] <= r["budget"]["buffer_bytes"]
                        assert (
                            not row["full_shard_gets"]
                            and row["payload_concurrency_peak"] <= 4
                        )
                        assert all(
                            v["if_match"] and v["if_match"].startswith('"')
                            for v in server.records
                            if v["method"] == "GET"
                            and v["key"].split("/")[0] in VARS
                            and v["status"] in (200, 206)
                        )
                        rows.append(row)
                        print(
                            label,
                            round(row["seconds"], 4),
                            row["payload_requests"],
                            row["payload_bytes"],
                            flush=True,
                        )
                        (DOCS / (name + ".json")).write_text(
                            json.dumps(
                                {
                                    "rows": rows,
                                    "repetitions": repetitions,
                                    "candidate_sha256": hashlib.sha256(
                                        BINARY.read_bytes()
                                    ).hexdigest(),
                                },
                                indent=2,
                            )
                        )
                successful = [
                    v
                    for v in rows
                    if v["layout"] == layout
                    and v["dense"] == dense
                    and v["link"] == link
                    and "error" not in v
                ]
                first = successful[0]
                assert all(
                    v["valid"] == first["valid"]
                    and np.isclose(v["sum"], first["sum"], rtol=1e-12)
                    and v["output_bytes"] == first["output_bytes"]
                    for v in successful
                )
                selected = next(
                    v["selected_bytes"] for v in successful if "selected_bytes" in v
                )
                for row in successful:
                    row["selected_bytes"] = selected
                    row["overfetch_bytes"] = row["payload_bytes"] - selected
                    assert row["overfetch_bytes"] >= 0
                assert hashlib.sha256(BINARY.read_bytes()).hexdigest() == binary_hash
                (DOCS / (name + ".json")).write_text(
                    json.dumps(
                        {
                            "rows": rows,
                            "repetitions": repetitions,
                            "candidate_sha256": binary_hash,
                        },
                        indent=2,
                    )
                )
            finally:
                server.shutdown()
                server.server_close()


def benchmark_science():
    work = WORK / "benchmark-science"
    work.mkdir(parents=True, exist_ok=True)
    rows = []
    for (layout, source), dense in itertools.product(layouts(), [False, True]):
        server = Server(source).start()
        label = f"{layout}-{dense}"
        r = h.request(server.url, work / "unused", shape=SHAPE, dense=dense)
        r["budget"]["chunk_edge"] = 32
        if not dense:
            r["spatial"]["bbox"] = [10, 10, 95, 95]
        try:
            dump = work / (label + "-chunks")
            result = h.run(r, work / (label + ".json"), mode="dump", dump=dump, env=ENV)
            rows.append(
                {"layout": layout, "dense": dense, **h.verify(r, source, result, dump)}
            )
            (work / (label + "-result.json")).write_text(json.dumps(result))
            print("exact benchmark science", label, flush=True)
        finally:
            server.shutdown()
            server.server_close()
    (DOCS / "benchmark-science.json").write_text(json.dumps(rows, indent=2))


def original_gates(actions):
    h.WORK = WORK / "science"
    h.DOCS = DOCS / "science"
    h.DOCS.mkdir(parents=True, exist_ok=True)
    for action in actions:
        getattr(h, action)()


def extended():
    work = WORK / "extended"
    work.mkdir(parents=True, exist_ok=True)
    rows = []
    source = h.fixture(
        work / "source",
        True,
        "gzip",
        shape=(8, 32, 32),
        inner=(2, 8, 8),
        factor=(2, 4, 4),
        consolidate=True,
    )
    repack(source, reverse=True)
    # Force a present all-NaN chunk with noncanonical payload bits through sharding.
    a = zarr.open_array(str(source / "temperature"), mode="r+").with_config(
        {"write_empty_chunks": True}
    )
    values = np.full((2, 8, 8), np.nan, dtype="<f4")
    values.view("<u4")[:] = 0x7FC0BEEF
    a[:2, :8, :8] = values

    def cell(
        label,
        policy="cost",
        fault=None,
        budget=64 * 1024 * 1024,
        mode="stream",
        ok=True,
    ):
        server = Server(source).start()
        server.payload_fault = ["corrupt"] if fault == "corrupt" else fault
        if mode == "cancel-inflight":
            server.payload_delay = 0.2
        r = h.request(server.url, work / "unused", shape=(8, 32, 32), dense=True)
        r["budget"]["buffer_bytes"] = budget
        r["budget"]["chunk_edge"] = 8
        dump = work / (label + "-chunks") if ok and mode == "stream" else None
        try:
            result = h.run(
                r,
                work / (label + ".json"),
                mode="dump" if dump else mode,
                dump=dump,
                check=False,
                env={
                    **ENV,
                    "TILE_RIPPER_ZARR_COALESCE": policy,
                    "TILE_RIPPER_TEST_CONTROL": server.url.replace(
                        "/store/", "/control/wait-payload"
                    ),
                },
            )
            assert ("error" not in result) == ok, (
                label,
                result.get("error", "unexpected successful finish"),
            )
            row = {
                "case": label,
                "passed": True,
                "error": result.get("error"),
                "traffic": traffic(server, source, True),
            }
            if ok:
                row["science"] = h.verify(r, source, result, dump)
                if mode == "warm":
                    assert result["runs"][0]["chunks"] == result["runs"][1]["chunks"]
                    assert row["traffic"]["index_requests"] == 6
                sched = result["runs"][0]["observed"]["scheduler"]
                row["reservation"] = sched["reserved_bytes_peak"]
                row["jobs"] = sched["admitted_jobs"]
                row["range_metrics"] = sched.get("zarr_v3_ranges")
                assert row["reservation"] <= budget
            (work / (label + "-result.json")).write_text(json.dumps(result))
            (work / (label + "-http.json")).write_text(json.dumps(server.records))
            rows.append(row)
            print(label, "PASS", flush=True)
        finally:
            server.shutdown()
            server.server_close()

    for policy in ["individual", "adjacent", "fixed", "cost"]:
        cell("physical-reversal-" + policy, policy=policy)
    cell("prepared-index-reuse", mode="warm")
    for budget in [131072, 196608, 262144, 524288, 1048576]:
        cell("small-budget-" + str(budget), budget=budget, ok=budget > 131072)
    for label, fault, ok in [
        ("retry-503", [503, None], True),
        ("retry-interrupted", ["interrupt", None], True),
        ("retry-exhaustion", [503] * 64, False),
        ("truncated-exhaustion", ["interrupt"] * 64, False),
        ("bad-framing", ["bad-range"], False),
        ("ignored-range", ["ignore-range"], False),
        ("conditional-412", [412], False),
        ("weak-version", ["weak"], False),
        ("mutated-index-to-payload", "mutate", False),
        ("corrupt-merged-body", "corrupt", False),
    ]:
        cell(label, fault=fault, ok=ok)
    cell("cancel-during-merged-read", mode="cancel-inflight", ok=False)
    # Missing physical shard, present shard with all missing entries, no CRC, start index.
    for location, crc in [("start", True), ("end", False)]:
        alternate = h.fixture(
            work / f"{location}-{crc}",
            True,
            "raw",
            shape=(8, 32, 32),
            inner=(2, 8, 8),
            factor=(2, 4, 4),
            index_location=location,
            index_crc=crc,
        )
        repack(alternate, reverse=True)
        original = source
        source = alternate
        cell("index-variant-" + location + "-" + str(crc))
        source = original
    # Store a valid all-MAX index via the library and omit another shard entirely.
    from zarr.codecs.sharding import _ShardIndex

    absent = h.fixture(
        work / "missing-shards",
        True,
        "gzip",
        shape=(8, 32, 32),
        inner=(2, 8, 8),
        factor=(2, 4, 4),
    )
    g = zarr.open_group(str(absent), mode="r")
    a = g["signal"]
    codec = a.metadata.codecs[0]
    cps = tuple(o // i for o, i in zip(a.shards, a.chunks))
    keys = sorted(p for p in (absent / "signal/c").rglob("*") if p.is_file())
    keys[0].write_bytes(
        sync(codec._encode_shard_index(_ShardIndex.create_empty(cps))).to_bytes()
    )
    keys[-1].unlink()
    original = source
    source = absent
    cell("empty-index-and-absent-shard")
    source = original
    (DOCS / "extended.json").write_text(json.dumps(rows, indent=2))
    # Record the existing integrity boundary explicitly: an unchecked raw codec
    # cannot discover length-preserving damage when the server lies about its
    # strong ETag. This is not a claimed successful corruption-rejection gate.
    raw = work / "end-False"
    server = Server(raw).start()
    server.payload_fault = ["corrupt"]
    r = h.request(server.url, work / "unused", shape=(8, 32, 32), dense=True)
    r["budget"]["chunk_edge"] = 8
    try:
        result = h.run(r, work / "raw-damage.json", env=ENV)
        mismatch = False
        try:
            h.verify(r, raw, result)
        except AssertionError:
            mismatch = True
        assert mismatch
        qualification = {
            "case": "raw-body-damage-with-unchanged-lying-etag",
            "successful_finish": True,
            "independent_science_mismatch": mismatch,
            "scope": "No payload checksum in this raw pipeline; framing and source identity cannot authenticate selectively read bytes against a dishonest server.",
        }
        (DOCS / "integrity-qualification.json").write_text(
            json.dumps(qualification, indent=2)
        )
        (work / "raw-damage-result.json").write_text(json.dumps(result))
    finally:
        server.shutdown()
        server.server_close()


def remote_memory():
    work = WORK / "remote-memory"
    work.mkdir(parents=True, exist_ok=True)
    rows = []
    for budget, nt in itertools.product([4 * 1024 * 1024, 8 * 1024 * 1024], [4, 256]):
        source = h.fixture(
            work / str(nt),
            True,
            "raw",
            shape=(nt, 128, 128),
            inner=(4, 32, 32),
            factor=(2, 4, 4),
        )
        server = Server(source).start()
        r = h.request(server.url, work / "unused", shape=(nt, 128, 128), dense=True)
        r["budget"].update(buffer_bytes=budget, chunk_edge=32)
        r["time"] = {
            "kind": "interval",
            "start": "2025-01-01T00:00:00Z",
            "end": str(
                np.datetime64("2025-01-01", "s")
                + np.timedelta64((nt - 1) * 3600 + 1, "s")
            )
            + "Z",
        }
        file = work / (str(budget) + "-" + str(nt) + ".json")
        file.write_text(json.dumps(r))
        try:
            result = measure([BINARY.parent / "chunk_stream", file], ENV)
            assert "error" not in result, result
            observed = result["observed"]
            sched = observed["scheduler"]
            assert observed["decoded_bytes"] == nt * 128 * 128 * 8
            assert sched["reserved_bytes_peak"] <= r["budget"]["buffer_bytes"]
            assert result["process_peak_rss_bytes"] < 192 * 1024 * 1024
            metrics = sched["zarr_v3_ranges"]
            if budget == 8 * 1024 * 1024:
                assert sum(m["merged_requests"] for m in metrics) > 0
            row = {
                "nt": nt,
                "budget": budget,
                "output_bytes": observed["decoded_bytes"],
                "peak_rss_bytes": result["process_peak_rss_bytes"],
                "reservation": sched["reserved_bytes_peak"],
                "metrics": metrics,
                "traffic": traffic(server, source, True),
            }
            rows.append(row)
            (work / (str(budget) + "-" + str(nt) + "-result.json")).write_text(
                json.dumps(result)
            )
            print(
                "remote merged memory",
                nt,
                row["peak_rss_bytes"],
                row["reservation"],
                flush=True,
            )
        finally:
            server.shutdown()
            server.server_close()
    for small, big in zip(rows[::2], rows[1::2]):
        assert big["peak_rss_bytes"] - small["peak_rss_bytes"] < 64 * 1024 * 1024
    (DOCS / "remote-memory.json").write_text(json.dumps(rows, indent=2))


def public_regression():
    import requests
    import zarr_v3_public as public

    old = json.loads((ROOT / "docs/zarr-v3-input/public.json").read_text())
    versions = old["result"]["inspection"]["sources"][0]["object_identities"]
    identities = []
    for key, expected in versions.items():
        if key == "zarr.json":
            continue
        response = requests.head(
            public.URL + key, headers={"If-Match": expected["etag"]}, timeout=45
        )
        identities.append(
            {
                "key": key,
                "status": response.status_code,
                "old": expected,
                "etag": response.headers.get("ETag"),
                "length": response.headers.get("Content-Length"),
            }
        )
        assert (
            response.status_code == 200
            and response.headers.get("ETag") == expected["etag"]
        ), identities[-1]
    (DOCS / "public-identities.json").write_text(json.dumps(identities, indent=2))
    h.WORK = WORK
    h.DOCS = DOCS
    public.main()
    result = json.loads((DOCS / "public.json").read_text())
    for key, expected in versions.items():
        if key != "zarr.json":
            assert (
                result["result"]["inspection"]["sources"][0]["object_identities"][key]
                == expected
            )


if __name__ == "__main__":
    DOCS.mkdir(parents=True, exist_ok=True)
    WORK.mkdir(parents=True, exist_ok=True)
    action = sys.argv[1]
    if action in ["pilot", "benchmark"]:
        benchmark(int(sys.argv[2]) if len(sys.argv) > 2 else 3, pilot=action == "pilot")
    elif action == "hints":
        benchmark(int(sys.argv[2]) if len(sys.argv) > 2 else 3, hints=True)
    elif action == "remote-memory":
        remote_memory()
    elif action == "public":
        public_regression()
    elif action == "benchmark-science":
        benchmark_science()
    elif action == "extended":
        extended()
    elif action == "science":
        original_gates(
            sys.argv[2:]
            or ["checks", "edges", "extras", "nan_bits", "faults", "memory"]
        )
