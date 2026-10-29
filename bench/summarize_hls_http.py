"""Validate and summarize credential-safe native/requests/libcurl timelines."""

from __future__ import annotations

import argparse
from collections import Counter
import json
from pathlib import Path
import statistics

from hls_river_30day import save


def concurrency(rows):
    events = []
    for r in rows:
        events.extend([(r["start_unix_s"], 1), (r["end_unix_s"], -1)])
    if not events:
        return {}
    active = peak = 0
    durations = Counter()
    previous = min(x[0] for x in events)
    for stamp, change in sorted(events):
        durations[active] += stamp - previous
        active += change
        assert active >= 0
        peak = max(peak, active)
        previous = stamp
    assert active == 0
    elapsed = max(x[0] for x in events) - min(x[0] for x in events)
    return {
        "peak": peak,
        "source_timeline_s": elapsed,
        "seconds_by_active_requests": dict(sorted(durations.items())),
        "mean_active_in_source_timeline": sum(k * v for k, v in durations.items())
        / elapsed,
    }


def summarize(paths):
    pending, complete, summaries = {}, [], []
    for path in paths:
        for line in path.read_text().splitlines():
            r = json.loads(line)
            if r["event"] == "trace_init":
                continue
            if r["event"] == "trace_summary":
                assert r["starts"] == r["ends"]
                assert (
                    not r["issues"] and not r["inexact"] and not r["pending_transfers"]
                )
                summaries.append(r)
                continue
            assert r["event"] in {"start", "end"}, r
            key = r["client"], r["pid"], r["id"]
            if r["event"] == "start":
                assert key not in pending
                pending[key] = r
                continue
            start = pending.pop(key)
            assert r.get("timing_exact", True), "unqualified libcurl endpoint"
            assert r["status"] == (200 if start["method"] == "HEAD" else 206), r
            elapsed = r["time_unix_s"] - start["time_unix_s"]
            assert elapsed > 0
            complete.append(
                {
                    **{
                        k: v
                        for k, v in start.items()
                        if k not in {"time_unix_s", "event"}
                    },
                    "start_unix_s": start["time_unix_s"],
                    "end_unix_s": r["time_unix_s"],
                    "seconds": elapsed,
                    "status": r["status"],
                    "bytes": r["bytes"],
                }
            )
    assert not pending, "unbalanced HTTP starts"
    assert complete
    complete.sort(key=lambda r: r["start_unix_s"])
    for r in complete:
        r["relative_start_s"] = r["start_unix_s"] - complete[0]["start_unix_s"]
        r["relative_end_s"] = r["end_unix_s"] - complete[0]["start_unix_s"]
        r["category"] = (
            "identity"
            if r["method"] == "HEAD"
            else (
                "header" if (r.get("range") or "").startswith("bytes=0-") else "payload"
            )
        )
    groups = {}
    for category in ["identity", "header", "payload"]:
        rows = [r for r in complete if r["category"] == category]
        groups[category] = {
            "requests": len(rows),
            "body_bytes": sum(r["bytes"] for r in rows),
            "transaction_seconds_sum": sum(r["seconds"] for r in rows),
            "median_transaction_s": statistics.median(r["seconds"] for r in rows)
            if rows
            else None,
            "concurrency": concurrency(rows),
        }
    return {
        "status": "passed",
        "files": [str(p) for p in paths],
        "transactions": len(complete),
        "source_files": len({r["source"] for r in complete}),
        "methods": dict(Counter(r["method"] for r in complete)),
        "clients": dict(Counter(r["client"] for r in complete)),
        "body_bytes": sum(r["bytes"] for r in complete),
        "concurrency": concurrency(complete),
        "categories": groups,
        "curl_summaries": summaries,
        "requests": complete,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trace", type=Path, nargs="+", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = summarize(args.trace)
    save(args.output, result)
    print(json.dumps({k: v for k, v in result.items() if k != "requests"}, indent=2))
