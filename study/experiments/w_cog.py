"""Worker: duckdb-cog (community extension v0.3.0) tasks, each in a fresh process (its remote caches are process-wide).

duckdb-cog fetches through Rust object_store, so its requests are invisible from Python. For accounting, the worker
starts a tiny logging reverse proxy on localhost (http://127.0.0.1:PORT/<path> -> the S3 bucket) and points the
extension at it. The proxy records method, Range, status and bytes per request. It adds one local hop; request
counts and bytes are exact, wall times carry that small overhead (stated in the report).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import duckdb
import httpx
import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from common import S3_HOST, USER_AGENT  # noqa: E402
from planwin import WINDOWS, bounds  # noqa: E402

UPSTREAM = f"https://{S3_HOST}"
EXT_DIR = str(HERE / ".duckdb_ext")
LOG: list[dict] = []


def start_proxy() -> int:
    client = httpx.Client(
        http2=False,
        timeout=60,
        headers={"User-Agent": USER_AGENT},
        limits=httpx.Limits(max_connections=None, max_keepalive_connections=64),
    )

    class H(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *a):  # silence
            pass

        def fwd(self, method: str) -> None:
            t0 = time.perf_counter()
            fwd_h = {"Accept-Encoding": "identity"}
            for k in ("Range", "If-Match", "If-None-Match", "If-Modified-Since"):
                if self.headers.get(k):
                    fwd_h[k] = self.headers[k]
            r = client.request(method, UPSTREAM + self.path, headers=fwd_h)
            self.send_response(r.status_code)
            for k in (
                "Content-Length",
                "Content-Range",
                "ETag",
                "Last-Modified",
                "Accept-Ranges",
                "Content-Type",
            ):
                if k in r.headers:
                    self.send_header(k, r.headers[k])
            self.end_headers()
            if method == "GET":
                self.wfile.write(r.content)
            LOG.append(
                {
                    "method": method,
                    "path": self.path.split("/")[-1],
                    "range": self.headers.get("Range"),
                    "status": r.status_code,
                    "nbytes": len(r.content) if method == "GET" else 0,
                    "dur_s": time.perf_counter() - t0,
                }
            )

        def do_GET(self):
            self.fwd("GET")

        def do_HEAD(self):
            self.fwd("HEAD")

    srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
    srv.daemon_threads = True
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv.server_address[1]


def req_summary() -> dict:
    gets = [r for r in LOG if r["method"] == "GET"]
    return {
        "requests": len(LOG),
        "head": sum(r["method"] == "HEAD" for r in LOG),
        "get": len(gets),
        "bytes": sum(r["nbytes"] for r in gets),
        "status": {
            str(c): sum(1 for r in LOG if r["status"] == c)
            for c in {r["status"] for r in LOG}
        },
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--task", choices=["meta", "window", "zonal", "search"], required=True
    )
    ap.add_argument("--workload", default="W_A_chip400")
    ap.add_argument("--scenes", type=int, default=24)
    ap.add_argument("--frozen", required=True)
    a = ap.parse_args()

    port = start_proxy()
    con = duckdb.connect(config={"extension_directory": EXT_DIR})
    con.execute("LOAD cog")
    scenes = json.loads(Path(a.frozen).read_text())["scenes"][: a.scenes]
    local = lambda href: f"http://127.0.0.1:{port}/" + href.split(".amazonaws.com/")[1]  # noqa: E731
    out: dict = {"task": a.task, "workload": a.workload}
    xmin, ymin, xmax, ymax = bounds(WINDOWS[a.workload])
    w = WINDOWS[a.workload]

    if a.task == "meta":
        url = local(scenes[0]["red"])
        t0 = time.perf_counter()
        rows = con.execute(f"SELECT * FROM read_cog('{url}')").to_arrow_table()
        out["t_work_s"] = time.perf_counter() - t0
        out["columns"] = rows.column_names
        out["tile_rows_all_levels"] = rows.num_rows
        out["requests_read_cog"] = req_summary()
        LOG.clear()
        t0 = time.perf_counter()
        md = con.execute(f"SELECT RS_MetaData('{url}')").fetchone()[0]
        out["rs_metadata"] = md if isinstance(md, dict) else str(md)
        out["t_metadata_s"] = time.perf_counter() - t0
        out["requests_rs_metadata"] = req_summary()
    elif a.task == "window":
        assets = [(s["id"], b, s[b]) for s in scenes for b in ("red", "nir")]
        values = ",".join(f"('{sid}','{b}','{local(h)}')" for sid, b, h in assets)
        t0 = time.perf_counter()
        tbl = con.execute(f"""SELECT sid, band, RS_BandAsArray(url, 1, [{xmin}, {ymin}, {xmax}, {ymax}]) AS arr
                              FROM (VALUES {values}) v(sid, band, url)""").to_arrow_table()
        out["t_work_s"] = time.perf_counter() - t0
        hashes, shapes = {}, set()
        for sid, band, arr in zip(
            tbl["sid"].to_pylist(), tbl["band"].to_pylist(), tbl["arr"].to_pylist()
        ):
            a16 = np.array([np.nan if v is None else v for v in arr])
            shapes.add(len(arr))
            hashes[f"{sid}|{band}"] = (
                hashlib.sha256(
                    np.nan_to_num(a16, nan=0).astype("<u2").tobytes()
                ).hexdigest()
                if len(arr) == w["h"] * w["w"]
                else "bad-shape"
            )
        out["array_lengths"] = sorted(shapes)
        out["hashes"] = hashes
        out["requests_detail"] = req_summary()
    elif a.task == "zonal":
        assets = [(s["id"], s["red"]) for s in scenes]
        values = ",".join(f"('{sid}','{local(h)}')" for sid, h in assets)
        t0 = time.perf_counter()
        tbl = con.execute(f"""SELECT sid, RS_ZonalStats(url, [{xmin}, {ymin}, {xmax}, {ymax}], 1, 'mean') AS m, 0 AS n
                              FROM (VALUES {values}) v(sid, url)""").to_arrow_table()  # ONE call per scene: the best case
        out["t_work_s"] = time.perf_counter() - t0
        out["zonal"] = {
            sid: [m, n]
            for sid, m, n in zip(
                tbl["sid"].to_pylist(), tbl["m"].to_pylist(), tbl["n"].to_pylist()
            )
        }
    elif a.task == "search":
        t0 = time.perf_counter()
        tbl = con.execute("""SELECT item_id, datetime::VARCHAR AS datetime, href FROM read_stac_search(
              'https://earth-search.aws.element84.com/v1/search', collections := ['sentinel-2-c1-l2a'],
              bbox := [-79.10, 35.88, -79.02, 35.95], datetime := '2025-01-01T00:00:00Z/2025-12-31T23:59:59Z', max_rows := 5000)""").to_arrow_table()
        out["t_work_s"] = time.perf_counter() - t0
        out["rows"] = tbl.num_rows
        out["items"] = len(set(tbl["item_id"].to_pylist()))
        out["columns"] = tbl.column_names
    out["requests"] = req_summary()
    print("RESULT_JSON " + json.dumps(out, default=str))


if __name__ == "__main__":
    main()
