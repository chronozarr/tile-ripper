"""DuckDB HTTP probe: run a query on a fresh connection and capture every HTTP request it makes.

Uses DuckDB's own structured HTTP log (duckdb_logs_parsed('HTTP')), plus the EXPLAIN ANALYZE
"HTTPFS HTTP Stats" block the TGE docs use, so both agree with the documented method.
"""

from __future__ import annotations

import re
import time
from dataclasses import asdict, dataclass
from datetime import datetime
from pathlib import Path

import duckdb

EXT_DIR = str(Path(__file__).resolve().parent / ".duckdb_ext")


@dataclass
class Req:
    type: str  # HEAD | GET
    url: str
    start_s: float  # seconds since the first request of this run
    dur_s: float
    rng: tuple[int, int] | None
    nbytes: int
    status: str


def connect(
    extra_sql: list[str] | None = None, log: bool = True
) -> duckdb.DuckDBPyConnection:
    con = duckdb.connect(config={"extension_directory": EXT_DIR})
    con.execute("LOAD httpfs; LOAD spatial; SET TimeZone='UTC'")
    for s in extra_sql or []:
        con.execute(s)
    if log:
        con.execute("CALL enable_logging('HTTP')")
    return con


def _parse_ts(s: str) -> float:
    return datetime.fromisoformat(s).timestamp()


def fetch_requests(con: duckdb.DuckDBPyConnection) -> list[Req]:
    rows = con.execute(
        "SELECT to_json(request)::VARCHAR, to_json(response)::VARCHAR FROM duckdb_logs_parsed('HTTP')"
    ).fetchall()
    import json

    reqs = []
    for rq, rs in rows:
        rq, rs = json.loads(rq), json.loads(rs or "{}")
        rng = None
        m = re.match(r"bytes=(\d+)-(\d+)", (rq.get("headers") or {}).get("Range", ""))
        if m:
            rng = (int(m.group(1)), int(m.group(2)))
        h = rs.get("headers") or {}
        n = int(h.get("Content-Length", 0)) if rq["type"] == "GET" else 0
        reqs.append(
            Req(
                rq["type"],
                rq["url"],
                _parse_ts(rq["start_time"]),
                rq["duration_ms"] / 1e3,
                rng,
                n,
                rs.get("status", ""),
            )
        )
    reqs.sort(key=lambda r: r.start_s)
    if reqs:
        t0 = reqs[0].start_s
        for r in reqs:
            r.start_s -= t0
    return reqs


def waves(reqs: list[Req]) -> int:
    """Sequential depth: number of non-overlapping request waves (the 'chain depth' the TGE docs describe)."""
    depth, end = 0, -1.0
    for r in sorted(reqs, key=lambda r: r.start_s):
        if r.start_s > end:
            depth += 1
            end = r.start_s + r.dur_s
        else:
            end = max(end, r.start_s + r.dur_s)
    return depth


def summarize_requests(reqs: list[Req]) -> dict:
    gets = [r for r in reqs if r.type == "GET"]
    return {
        "head": sum(r.type == "HEAD" for r in reqs),
        "get": len(gets),
        "bytes_in": sum(r.nbytes for r in gets),
        "waves": waves(reqs),
        "span_s": max((r.start_s + r.dur_s for r in reqs), default=0.0),
        "max_get_bytes": max((r.nbytes for r in gets), default=0),
    }


def run_cold(
    sql: str, extra_sql: list[str] | None = None, warm_repeat: bool = True
) -> dict:
    """Fresh connection (cold: no metadata/object cache). Optionally repeat on the same connection (warm)."""
    con = connect(extra_sql)
    t = time.perf_counter()
    tbl = con.execute(sql).to_arrow_table()
    wall = time.perf_counter() - t
    reqs = fetch_requests(con)
    out = {
        "wall_s": wall,
        "rows": tbl.num_rows,
        **summarize_requests(reqs),
        "requests": [asdict(r) for r in reqs],
    }
    if warm_repeat:
        con.execute("CALL truncate_duckdb_logs()")
        t = time.perf_counter()
        con.execute(sql).to_arrow_table()
        wreqs = fetch_requests(con)
        out["warm_wall_s"] = time.perf_counter() - t
        out["warm_get"] = sum(r.type == "GET" for r in wreqs)
    con.close()
    return out
