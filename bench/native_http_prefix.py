"""Measure prefix overfetch on a single-page-metadata COG, without real capture."""

import argparse
import hashlib
import json
from pathlib import Path
import random
import threading
import time

import numpy as np
import rasterio
from rasterio.transform import from_origin
from rasterio.shutil import copy as rio_copy

import cog_io_trace as trace
import consumer_validation as consumer
import native_cog_checks as checks
import stac_workflow as fixture

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/native-http/prefix-onepage"


class Handler(fixture.Handler):
    def asset(self, body):
        if self.server.latency_ms:
            time.sleep(self.server.latency_ms / 1000)
        super().asset(body)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--four-k", type=Path, required=True)
    ap.add_argument("--eight-k", type=Path, required=True)
    ap.add_argument("--baseline", type=Path, default=ROOT / "prototype-output/native-http/baseline/tile-ripper-native-stats")
    args = ap.parse_args()
    WORK.mkdir(parents=True, exist_ok=True)
    source = WORK / "onepage.tif"
    values = np.random.default_rng(20261020).integers(0, 65536, (256, 256), dtype=np.uint16)
    staging = WORK / "staging.tif"
    with rasterio.open(staging, "w", driver="GTiff", width=256, height=256, count=1,
        dtype="uint16", nodata=0, crs="EPSG:32617", transform=from_origin(600000, 4000000, 10, 10),
        tiled=True, blockxsize=32, blockysize=32, compress="deflate", predictor=2) as ds:
        ds.write(values, 1)
        ds.scales, ds.offsets = (0.0001,), (-0.1,)
    source.unlink(missing_ok=True)
    rio_copy(staging, source, driver="COG", BLOCKSIZE=128, COMPRESS="DEFLATE", PREDICTOR=2)
    layout = trace.tiff_layout(source)
    assert source.stat().st_size > 8192
    assert max(end for _, end in layout["metadata"]) <= 4096
    server = fixture.Server(("127.0.0.1", 0), Handler)
    server.root, server.files = WORK, [source.name]
    server.seen_conditions, server.current_etag = [], '"fixed-fixture"'
    threading.Thread(target=server.serve_forever, daemon=True).start()
    request = dict(source=dict(kind="assets", assets=[dict(id="onepage", format="cog",
        locator=f"http://127.0.0.1:{server.server_port}/{source.name}", datetime="2025-01-01T00:00:00Z")]),
        spatial=dict(kind="bounds", crs="EPSG:32617", bbox=[600010, 3999830, 600170, 3999990]),
        variables=[dict(kind="band", index=1)], grid=dict(kind="native"), sink=dict(kind="callback"),
        budget=dict(buffer_bytes=16*2**20, max_nodes=10000, chunk_edge=32))
    modes = {"baseline": args.baseline, "optimized-4k": args.four_k, "optimized-8k": args.eight_k}
    hashes = {k:hashlib.sha256(v.read_bytes()).hexdigest() for k,v in modes.items()}
    rows = []
    rng = random.Random(20261020)
    try:
        for latency in [0, 8]:
            server.latency_ms = latency
            for rep in range(3):
                order = list(modes)
                rng.shuffle(order)
                for mode in order:
                    server.reset()
                    consumer.BIN = modes[mode]
                    result = consumer.invoke(request, WORK / f"{latency}-{rep}-{mode}.json")
                    checks.oracle(result, source, request)
                    assert server.counts["whole_gets"] == 0
                    assert hashlib.sha256(modes[mode].read_bytes()).hexdigest() == hashes[mode]
                    rows.append(dict(backend=mode, latency_ms=latency, rep=rep,
                        wall_seconds=result["seconds"], peak_rss_bytes=result["peak_rss_bytes"],
                        gets=server.counts["range_gets"], heads=server.counts["heads"], body_bytes=server.counts["raster_bytes"], passed=True))
    finally:
        server.shutdown()
        server.server_close()
    record = dict(passed=True, source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
        metadata_intervals=layout["metadata"],
        binary_sha256=hashes, rows=rows,
        notes=["Synthetic 256x256 tiled UInt16 COG with metadata fitting the first 4KiB.",
               "Loopback; 0/8ms delay per GET and HEAD; no bandwidth shaping or TLS.",
               "Exact sample oracle runs after each measured consumer. Fixed larger prefix overfetch is counted as response-body bytes."])
    (ROOT / "docs/native-http/prefix-onepage.json").write_text(json.dumps(record, indent=2)+"\n")
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()
