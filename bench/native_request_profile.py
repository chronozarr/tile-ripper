"""Bounded native/GDAL request diagnosis on the existing frozen Sentinel mirror.

Uses preserved binaries, never captures provider bytes, and leaves raw timing and
physical-range records in ignored prototype-output/native-sentinel. Server-side
connection IDs identify actual accepted TCP sockets; request durations include
the configured delay and shared-link transfer, not TLS or public-provider RTT.
"""

from __future__ import annotations

import argparse
from collections import Counter
import copy
import hashlib
import json
from pathlib import Path
import statistics
import threading
import time
import urllib.parse

import cog_io_trace as trace
import real_multiscene as m
import snapshot_comparison as snapshot

ROOT = m.ROOT
WORK = ROOT / "prototype-output/native-sentinel"
FROZEN = ROOT / "docs/native-cog/benchmark.json"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class ProfileServer(m.Server):
    def reset(self):
        super().reset()
        with self.lock:
            self.profile_records = []


class ProfileHandler(m.Handler):
    def setup(self):
        super().setup()
        with self.server.lock:
            self.server.next_connection += 1
            self.connection_id = self.server.next_connection

    def end_headers(self):
        self.header_finished = time.perf_counter()
        return super().end_headers()

    def serve(self, body):
        started = time.perf_counter()
        self.header_finished = None
        super().serve(body)
        source = urllib.parse.urlsplit(self.path).path
        size = self.server.mirror.sources[source]["bytes"]
        left, right = self.headers.get("Range", f"bytes=0-{size - 1}").removeprefix("bytes=").split("-")
        record = dict(
            method="GET" if body else "HEAD",
            source=source,
            start=int(left),
            end=min(int(right) if right else size - 1, size - 1),
            connection=self.connection_id,
            conditional=self.headers.get("If-Match"),
            started=started,
            seconds=time.perf_counter() - started,
            header_seconds=(self.header_finished - started if self.header_finished else None),
        )
        with self.server.lock:
            self.server.profile_records.append(record)


def start(mirror, latency, mbps):
    server = ProfileServer(("127.0.0.1", 0), ProfileHandler)
    server.mirror = mirror
    server.latency = latency / 1000
    server.link = m.engine.SharedLink(mbps * 1e6 / 8 if mbps else None)
    server.lock = threading.Lock()
    server.next_connection = 0
    server.reset()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def selected_blocks(layout, workload):
    x, y, w, h = trace.WINDOWS[workload]
    image = max(layout["images"], key=lambda a: a["width"] * a["height"])
    return sorted(
        (b for b in image["blocks"]
         if b["x"] * image["block_width"] < x + w
         and (b["x"] + 1) * image["block_width"] > x
         and b["y"] * image["block_height"] < y + h
         and (b["y"] + 1) * image["block_height"] > y),
        key=lambda b: b["offset"],
    )


def adjacency(layouts, workload):
    sources = []
    for key, layout in layouts.items():
        if workload != "many-small" and int(key[1:].split(".")[0]) >= 6:
            continue
        blocks = selected_blocks(layout, workload)
        gaps = [b["offset"] - a["offset"] - a["bytes"] for a, b in zip(blocks, blocks[1:])]
        # Pure counterfactual: exact selected blocks grouped across <=8-byte
        # COG framing gaps, with no unselected imagery. No production prefetch.
        groups = []
        for b in blocks:
            lo, hi = b["offset"], b["offset"] + b["bytes"]
            if groups and lo - groups[-1][1] <= 8:
                groups[-1][1] = hi
            else:
                groups.append([lo, hi])
        sources.append(dict(
            source=key, selected_blocks=len(blocks), gaps_bytes=gaps,
            groups_at_most_8_byte_gap=len(groups),
            merged_range_bytes=[hi - lo for lo, hi in groups],
            added_gap_bytes=sum(hi - lo for lo, hi in groups) - sum(b["bytes"] for b in blocks),
        ))
    return dict(
        sources=sources,
        selected_blocks=sum(s["selected_blocks"] for s in sources),
        possible_groups=sum(s["groups_at_most_8_byte_gap"] for s in sources),
        added_gap_bytes=sum(s["added_gap_bytes"] for s in sources),
        largest_merged_range_bytes=max(max(s["merged_range_bytes"]) for s in sources),
    )


def summarize_requests(records, layouts, workload):
    get = [r for r in records if r["method"] == "GET"]
    account = trace.analyze(records, layouts, workload)
    requests = account["requests"]
    categories = Counter()
    kinds = {}
    for r in requests:
        if r["start"] in (0, 4096) and r["end"] - r["start"] + 1 == 4096:
            category = "metadata_page"
        elif r.get("exact_metadata_bytes", 0) and not r.get("selected_payload_bytes", 0):
            category = "metadata_or_read_ahead"
        elif r.get("selected_payload_bytes", 0):
            category = "selected_tile_payload"
        else:
            category = "other"
        categories[category] += 1
        kinds[(r["source"], r["start"], r["end"])] = category
    connections = Counter(r["connection"] for r in records)
    get_connections = Counter(r["connection"] for r in get)

    def concurrency(events):
        active = peak = 0
        for _, change in sorted(events):
            active += change
            peak = max(peak, active)
        return peak

    events, payload_events = [], []
    category_seconds = Counter()
    category_header_seconds = Counter()
    for r in get:
        kind = kinds[(r["source"], r["start"], r["end"])]
        category_seconds[kind] += r["seconds"]
        category_header_seconds[kind] += r["header_seconds"]
        events.extend([(r["started"], 1), (r["started"] + r["seconds"], -1)])
        if kind == "selected_tile_payload":
            payload_events.extend([(r["started"], 1), (r["started"] + r["seconds"], -1)])
    return dict(
        request_categories=dict(categories),
        request_category_server_seconds_sum=dict(category_seconds),
        request_category_header_seconds_sum=dict(category_header_seconds),
        physical_bytes=account["totals"],
        tcp_connections=len(connections),
        requests_on_reused_connections=sum(max(0, n - 1) for n in connections.values()),
        requests_per_connection=sorted(connections.values()),
        get_tcp_connections=len(get_connections),
        get_requests_on_reused_connections=sum(max(0, n - 1) for n in get_connections.values()),
        get_requests_per_connection=sorted(get_connections.values()),
        peak_concurrent_get=concurrency(events),
        peak_concurrent_payload_get=concurrency(payload_events),
        get_server_seconds_sum=sum(r["seconds"] for r in get),
        get_header_seconds_sum=sum(r["header_seconds"] for r in get),
        get_server_seconds_median=statistics.median(r["seconds"] for r in get),
        # Complete records are retained only in the ignored raw folder.
    ), account


def compact_row(row):
    keys = [
        "backend", "workload", "rep", "latency_ms", "bandwidth_mbps",
        "wall_seconds", "first_chunk_seconds", "peak_rss_bytes", "cpu_seconds",
        "http", "workers", "layout_inspection_seconds", "planning_compute_seconds",
        "read_worker_seconds", "decode_assembly_seconds", "profile", "passed",
    ]
    result = {k: row[k] for k in keys if k in row}
    obs = row.get("observed") or {}
    result["native_observed"] = {
        k: obs[k] for k in [
            "scheduler", "source_retrieval_seconds", "source_data_requests",
            "source_data_bytes", "read_decode_seconds",
        ] if k in obs
    }
    return result


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--label", required=True)
    ap.add_argument("--reps", type=int, default=1)
    ap.add_argument("--native-binary", type=Path, default=ROOT / "prototype-output/native-cog/native/tile-ripper-native-stats")
    ap.add_argument("--workloads", nargs="+", choices=list(trace.WINDOWS), default=list(trace.WINDOWS))
    ap.add_argument("--output", type=Path, default=ROOT / "docs/native-sentinel/request-profile.json")
    args = ap.parse_args()
    folder = WORK / args.label
    folder.mkdir(parents=True, exist_ok=False)
    frozen = json.loads(FROZEN.read_text())
    sources = json.loads((m.WORK / "sources.json").read_text())["sources"]
    mirror = m.Mirror(m.WORK / "mirror", sources)
    digest = mirror.verify_pages()
    assert digest == frozen["mirror_index_sha256"]
    refs = {a["id"]: {**a, "format": "cog", "local": mirror.path(f"/{i}.tif")} for i, a in enumerate(sources)}
    layouts = {f"/{i}.tif": trace.tiff_layout(mirror.path(f"/{i}.tif")) for i in range(len(sources))}
    modes = {
        "native": args.native_binary.resolve(),
        "tile-ripper-gdal": ROOT / "prototype-output/native-cog/baseline/tile-ripper-native-stats",
        "gdal-control": None,
    }
    hashes = {k: sha(p) for k, p in modes.items() if p}
    assert hashes["tile-ripper-gdal"] == frozen["binary_sha256"]["tile-ripper-gdal"]
    rows = []
    for latency, mbps in [(0, 0), (8, 100)]:
        server = start(mirror, latency, mbps)
        try:
            for workload in args.workloads:
                req = m.request(sources if workload == "many-small" else sources[:6], server, trace.WINDOWS[workload])
                tuned = next(r["workers"] for r in frozen["rows"] if (r["backend"], r["workload"], r["latency_ms"]) == ("gdal-control", workload, latency))
                for rep in range(args.reps):
                    canonical = None
                    for mode, binary in modes.items():
                        dest = folder / f"{latency}-{workload}-{rep}-{mode}"
                        if binary:
                            assert sha(binary) == hashes[mode], "preserved binary changed"
                            m.CONSUMER = binary
                            row = m.measure("rust-stream", req, dest, server, refs)
                            chunks = [json.loads(s) for s in (dest / "stdout.json").read_text().splitlines()][1:-1]
                            normalized = [{k: v for k, v in c.items() if k != "node_id"} for c in chunks]
                            if canonical is None:
                                canonical = normalized
                            assert normalized == canonical, "native/baseline chunk contract differs"
                        else:
                            row = snapshot.measure("gdal-snapshot-stream", req, dest, server, refs, tuned)
                        # Oracle runs locally after acquisition; all HTTP records
                        # belong to the timed consumer, without oracle traffic.
                        records = copy.deepcopy(server.profile_records)
                        assert len(records) == row["http"]["head"] + row["http"]["get"]
                        summary, physical = summarize_requests(records, layouts, workload)
                        assert summary["physical_bytes"]["body_bytes"] == row["http"]["body_bytes"]
                        (dest / "requests.json").write_text(json.dumps(records, indent=2))
                        (dest / "physical.json").write_text(json.dumps(physical, indent=2))
                        row.pop("request_records", None)
                        row.pop("stages", None)
                        row.update(backend=mode, workload=workload, rep=rep, latency_ms=latency, bandwidth_mbps=mbps, profile=summary)
                        obs = row.get("observed") or {}
                        row["decode_assembly_seconds"] = (obs["read_decode_seconds"] - obs["source_retrieval_seconds"] if obs.get("source_retrieval_seconds") is not None else None)
                        rows.append(row)
                        (dest / "measurement.json").write_text(json.dumps(row, indent=2))
                        print(latency, workload, mode, row["http"]["get"], summary["request_categories"], "connections", summary["tcp_connections"], "seconds", round(row["wall_seconds"], 3), flush=True)
        finally:
            server.shutdown()
            server.server_close()
    report = dict(
        decision="diagnosis only; range transport unchanged",
        source_commit="40518b5",
        harness_sha256=sha(Path(__file__)),
        frozen_benchmark_sha256=sha(FROZEN),
        mirror_index_sha256=digest,
        binary_sha256=hashes,
        timing_scope="diagnostic replay; frozen 72-trial matrix remains performance authority; server timing includes artificial latency/link; local HTTP/1.1 has no TLS",
        repetitions=args.reps,
        raw_directory=str(folder.relative_to(ROOT)),
        all_passed=all(r["passed"] for r in rows),
        adjacency={n: adjacency(layouts, n) for n in args.workloads},
        frozen_summary=[{
            k: (v["median"] if isinstance(v, dict) and "median" in v else v)
            for k, v in r.items()
            if k in ["backend", "workload", "latency_ms", "runs", "failures",
                     "wall_seconds", "read_decode_seconds", "decode_assembly_seconds",
                     "peak_rss_bytes", "http"]
        } for r in frozen["summary"]],
        rows=[compact_row(row) for row in rows],
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    (folder / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    assert report["all_passed"]


if __name__ == "__main__":
    main()
