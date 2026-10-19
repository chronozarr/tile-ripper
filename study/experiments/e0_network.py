"""E0: which regime is this machine in? Latency + throughput vs concurrency to the two hosts.

Bounded volume: ~250 MiB total from S3 (public bucket), a few KB from source.coop.
Uses plain HTTP/1.1 threads (S3 negotiates HTTP/1.1 anyway), one connection per in-flight request.
"""

from __future__ import annotations

import concurrent.futures as cf
import sys
import time

import httpx

from common import S3_HOST, TGE_BASE, USER_AGENT, save, summarize

# A real C1 B04 COG (219 MB). Only ranges are requested.
B04 = f"https://{S3_HOST}/sentinel-2-c1-l2a/17/S/PV/2025/6/S2C_T17SPV_20250626T160859_L2A/B04.tif"
TGE_PROBE = f"{TGE_BASE}/catalog.json"
HDR = {"User-Agent": USER_AGENT}


def timed_range(
    client: httpx.Client, url: str, start: int, length: int
) -> tuple[float, float, int]:
    """Return (ttfb_s, total_s, nbytes) for one ranged GET."""
    t0 = time.perf_counter()
    with client.stream(
        "GET", url, headers={**HDR, "Range": f"bytes={start}-{start + length - 1}"}
    ) as r:
        r.raise_for_status()
        ttfb = time.perf_counter() - t0
        n = sum(len(c) for c in r.iter_bytes(65536))
    return ttfb, time.perf_counter() - t0, n


def latency(url: str, label: str, reps: int = 12) -> dict:
    cold, warm = [], []
    for _ in range(reps):  # new connection each time: TCP + TLS + request
        with httpx.Client(http2=False, timeout=30) as c:
            cold.append(timed_range(c, url, 0, 1024)[1])
    with httpx.Client(http2=False, timeout=30) as c:  # keep-alive: request only
        timed_range(c, url, 0, 1024)
        for _ in range(reps):
            warm.append(timed_range(c, url, 0, 1024)[1])
    out = {"url": label, "new_conn_s": summarize(cold), "keepalive_s": summarize(warm)}
    print(
        label,
        "new-conn median %.0f ms | keep-alive median %.0f ms"
        % (out["new_conn_s"]["median"] * 1e3, out["keepalive_s"]["median"] * 1e3),
    )
    return out


def throughput(url: str, concurrency: int, size: int, reps: int = 2) -> dict:
    """C parallel ranged GETs of `size` bytes each, distinct offsets; aggregate MB/s."""
    rates, ttfbs = [], []
    for rep in range(reps):
        base = (
            4 * 2**20 + rep * concurrency * size
        )  # skip header region, vary offsets across reps
        limits = httpx.Limits(
            max_connections=concurrency, max_keepalive_connections=concurrency
        )
        with httpx.Client(http2=False, timeout=120, limits=limits) as c:
            timed_range(c, url, 0, 1024)  # prime DNS/TLS pool entry
            t0 = time.perf_counter()
            with cf.ThreadPoolExecutor(concurrency) as ex:
                res = list(
                    ex.map(
                        lambda i: timed_range(c, url, base + i * size, size),
                        range(concurrency),
                    )
                )
            wall = time.perf_counter() - t0
        nbytes = sum(r[2] for r in res)
        rates.append(nbytes / wall / 1e6)
        ttfbs += [r[0] for r in res]
    out = {
        "concurrency": concurrency,
        "req_bytes": size,
        "MBps": summarize(rates),
        "ttfb_s": summarize(ttfbs),
    }
    print(
        "C=%-3d size=%.1f MiB  agg %.1f MB/s (%.0f Mbit/s)  ttfb median %.0f ms"
        % (
            concurrency,
            size / 2**20,
            out["MBps"]["median"],
            out["MBps"]["median"] * 8,
            out["ttfb_s"]["median"] * 1e3,
        )
    )
    return out


def main() -> None:
    print("== latency")
    lat = [
        latency(B04, "s3 us-west-2 (e84-earth-search-sentinel-data)"),
        latency(TGE_PROBE, "source.coop (tge-labs)"),
    ]
    print("== throughput, 1.3 MiB ranges (one S2 tile) and 4 MiB ranges, S3")
    tp = []
    for size in (1_300_000, 4 * 2**20):
        for c in (1, 4, 8, 16, 32, 64):
            if (
                4 * 2**20 + 2 * c * size > 210_000_000
            ):  # stay inside the 219 MB file; also bounds volume
                continue
            tp.append(throughput(B04, c, size))
    save("e0_network", {"latency": lat, "throughput_s3": tp})


if __name__ == "__main__":
    sys.exit(main())
