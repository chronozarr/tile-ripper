"""Exercise payload faults after metadata reuse in both bounded-selection strategies."""

import hashlib
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import threading

import numpy as np
import rasterio
from rasterio.shutil import copy as rio_copy
from rasterio.transform import from_origin
from rasterio.windows import Window

import consumer_validation as consumer
import cog_io_trace
import native_cog_checks as inherited


ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/native-http/selection-faults"
TARGET = Path(os.environ.get("TILE_RIPPER_NATIVE_TARGET", ROOT / "target/native-http/release"))


class Handler(inherited.Handler):
    def setup(self):
        super().setup()
        self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def serve(self, body):
        server = self.server
        lo = int(self.headers.get("Range", "bytes=0-0")[6:].split("-")[0])
        payload = body and lo >= server.first_payload - 4
        mode = server.injection if payload else ""
        if payload:
            server.payload_requests += 1
        if mode in ["503-once", "short-once"]:
            mode = mode.removesuffix("-once") if server.payload_requests == 1 else ""
        server.mode = mode
        return super().serve(body)


def oversized_wrapped_copy(path, destination):
    """Keep decoded imagery; enlarge a payload and mismatch its readable wrapper.

    The upstream parser keeps the short wrapped payload while attempting a direct
    read. This forces three encoded copies, rather than the usual two.
    """
    raw = bytearray(path.read_bytes())
    assert raw[:4] == b"II*\0"
    at = struct.unpack_from("<I", raw, 4)[0]
    count = struct.unpack_from("<H", raw, at)[0]
    entries = {struct.unpack_from("<H", raw, at + 2 + i * 12)[0]: at + 2 + i * 12
               for i in range(count)}
    offset_entry, count_entry = entries[324], entries[325]
    assert struct.unpack_from("<H", raw, offset_entry + 2)[0] == 4
    assert struct.unpack_from("<H", raw, count_entry + 2)[0] == 4
    offsets = struct.unpack_from("<I", raw, offset_entry + 8)[0]
    counts = struct.unpack_from("<I", raw, count_entry + 8)[0]
    offset, length = struct.unpack_from("<I", raw, offsets)[0], struct.unpack_from("<I", raw, counts)[0]
    payload = bytes(raw[offset:offset+length])
    encoded = 500000
    assert len(payload) < encoded - 8
    new_offset = len(raw) + 4
    raw.extend(struct.pack("<I", encoded - 1))
    raw.extend(payload + bytes(encoded - len(payload)))
    raw.extend(bytes(4))
    struct.pack_into("<I", raw, offsets, new_offset)
    struct.pack_into("<I", raw, counts, encoded)
    destination.write_bytes(raw)


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    consumer.BIN = TARGET / "tile-ripper-native-stats"
    staging, path = WORK / "staging.tif", WORK / "selection.tif"
    values = np.random.default_rng(20261020).integers(0, 65536, (512, 512), dtype=np.uint16)
    with rasterio.open(staging, "w", driver="GTiff", width=512, height=512, count=1,
                       dtype="uint16", crs="EPSG:32617", transform=from_origin(600000, 4000000, 10, 10),
                       nodata=65535) as ds:
        ds.write(values, 1)
        ds.scales, ds.offsets = [0.0001], [-0.1]
    rio_copy(staging, path, driver="COG", BLOCKSIZE=256, COMPRESS="DEFLATE", PREDICTOR=2,
             OVERVIEWS="NONE")
    expected = WORK / "selection-expected-u16le.bin"
    with rasterio.open(staging) as ds, expected.open("wb") as output:
        for y in [120, 376]:
            for x in [120, 376]:
                reference = ds.read(1, window=Window(x, y, min(256, 504-x), min(256, 504-y)))
                output.write(reference.astype("<u2").tobytes())

    def exact_reference(name):
        result = subprocess.run([str(TARGET / "examples/native_cog_oracle"), str(WORK / f"{name}.json"),
                                 str(expected)], capture_output=True, text=True, check=True)
        record = json.loads(result.stdout)
        assert record == dict(samples_compared_bit_for_bit=147456, chunks=4, complete=True)
        return record["samples_compared_bit_for_bit"]
    # TIFF physical offsets come from an independent parser, not the native reader.
    first_payload = min(block["offset"] for image in cog_io_trace.tiff_layout(path)["images"]
                        for block in image["blocks"])
    server = inherited.Server(("127.0.0.1", 0), Handler)
    server.path, server.etag = path, '"selection-frozen"'
    server.first_payload = first_payload
    threading.Thread(target=server.serve_forever, daemon=True).start()
    rows = []
    try:
        for budget in [900000, 2 * 2**20]:
            req = inherited.request(f"http://127.0.0.1:{server.server_port}/selection.tif", edge=256, buffer=budget)
            req["spatial"]["bbox"] = [601200, 3994960, 605040, 3998800]
            req["source"]["assets"][0]["etag"] = '"selection-frozen"'
            for injection in ["", "503-once", "short-once", "503", "short", "range", "wrong-etag", "412",
                              "cancel", "sink-failure", "late-mutation"]:
                server.etag = '"selection-frozen"'
                server.injection = injection if injection not in ["cancel", "sink-failure", "late-mutation"] else ""
                server.mode, server.gets, server.heads = "", 0, 0
                server.payload_requests, server.conditions = 0, []
                args = {"cancel": ["--cancel-after", "1"], "sink-failure": ["--max-chunk-bytes", "1"],
                        "late-mutation": ["--pause-after", "1"]}.get(injection, [])
                mutate = (lambda _: setattr(server, "etag", '"changed"')) if injection == "late-mutation" else None
                name = f"selection-{budget}-{injection or 'correct'}"
                result = consumer.invoke(req, WORK / f"{name}.json", args=args, mutate=mutate)
                success = injection in ["", "503-once", "short-once"]
                if success:
                    inherited.oracle(result, path, req)
                    scheduler = result["rows"][-1]["observed"]["scheduler"]
                    strategy = scheduler["native_strategies"][0]["strategy"]
                    assert strategy["read_scope"] == "bounded_selection", strategy
                    assert bool(strategy["coalesced_groups"]) == (budget == 2 * 2**20), strategy
                    assert scheduler["reserved_bytes_peak"] <= budget
                else:
                    assert result["code"] != 0 and not any(r["type"] == "complete" for r in result["rows"]), result
                    if injection in ["cancel", "late-mutation"]:
                        assert any(r["type"] == "provisional_chunk" for r in result["rows"]), result
                compared = exact_reference(name) if success and injection == "" else 0
                assert server.payload_requests > 0
                assert all(v == '"selection-frozen"' for v in server.conditions)
                rows.append(dict(name=name, passed=True, budget_bytes=budget, injected=injection,
                                 exit_code=result["code"], peak_rss_bytes=result["peak_rss_bytes"],
                                 payload_requests=server.payload_requests,
                                 samples_compared_bit_for_bit=compared,
                                 provisional_chunks=sum(r["type"] == "provisional_chunk" for r in result["rows"])))
                print(name, "PASS", flush=True)
        padded = WORK / "wrapped-fallback.tif"
        oversized_wrapped_copy(path, padded)
        server.path = padded
        for budget, success in [(1600000, False), (4 * 2**20, True)]:
            server.etag, server.injection, server.mode = '"selection-frozen"', "", ""
            server.gets, server.heads, server.payload_requests, server.conditions = 0, 0, 0, []
            req = inherited.request(f"http://127.0.0.1:{server.server_port}/wrapped-fallback.tif", edge=256, buffer=budget)
            req["spatial"]["bbox"] = [601200, 3994960, 605040, 3998800]
            req["source"]["assets"][0]["etag"] = server.etag
            name = f"wrapped-three-copies-{budget}"
            result = consumer.invoke(req, WORK / f"{name}.json")
            if success:
                inherited.oracle(result, path, req)
                compared = exact_reference(name)
                scheduler = result["rows"][-1]["observed"]["scheduler"]
                assert scheduler["reserved_bytes_peak"] <= budget
                assert scheduler["native_strategies"][0]["task_reserved_bytes"] >= 3 * 500000
            else:
                compared = 0
                assert result["code"] != 0 and not any(r["type"] == "complete" for r in result["rows"]), result
                assert "buffer" in result["rows"][-1]["error"], result
                assert server.payload_requests == 0, result
            rows.append(dict(name=name, passed=True, budget_bytes=budget, decoded_tile_bytes=131072,
                             encoded_payload_bytes=500000, expected_encoded_copies_peak=3,
                             exit_code=result["code"], peak_rss_bytes=result["peak_rss_bytes"],
                             payload_requests=server.payload_requests))
            rows[-1]["samples_compared_bit_for_bit"] = compared
            print(name, "PASS", flush=True)
    finally:
        server.shutdown()
        server.server_close()
    # The buffered path is codec-independent, but verify every inherited lossless
    # UInt16 combination using GDAL-written sources and GDAL-decoded references.
    for compression, predictor in [(None, 1), ("deflate", 1), ("deflate", 2), ("lzw", 1), ("lzw", 2)]:
        for big in ["NO", "YES"]:
            for endian in ["LITTLE", "BIG"]:
                name = f"buffered-profile-{compression}-{predictor}-{big}-{endian}"
                fixture = WORK / f"{name}.tif"
                with rasterio.open(fixture, "w", driver="GTiff", width=512, height=512, count=1,
                                   dtype="uint16", tiled=True, blockxsize=256, blockysize=256,
                                   crs="EPSG:32617", transform=from_origin(600000, 4000000, 10, 10),
                                   nodata=65535, compress=compression, predictor=predictor,
                                   BIGTIFF=big, ENDIANNESS=endian) as ds:
                    ds.write(values, 1)
                    ds.scales, ds.offsets = [0.0001], [-0.1]
                req = inherited.request(fixture, edge=256, buffer=4 * 2**20)
                req["spatial"]["bbox"] = [601200, 3994960, 605040, 3998800]
                result = consumer.invoke(req, WORK / f"{name}.json")
                inherited.oracle(result, fixture, req)
                compared = exact_reference(name)
                scheduler = result["rows"][-1]["observed"]["scheduler"]
                strategy = scheduler["native_strategies"][0]["strategy"]
                assert strategy["read_scope"] == "bounded_selection" and strategy["decoded_cache_limit_bytes"] == 0
                assert scheduler["reserved_bytes_peak"] <= req["budget"]["buffer_bytes"]
                rows.append(dict(name=name, passed=True, compression=compression, predictor=predictor,
                                 bigtiff=big, byte_order=endian, exit_code=result["code"],
                                 samples_compared_bit_for_bit=compared,
                                 peak_rss_bytes=result["peak_rss_bytes"]))
                print(name, "PASS", flush=True)
    receipt = dict(passed=True, checks=rows, consumer_sha256=hashlib.sha256(consumer.BIN.read_bytes()).hexdigest(),
                   samples_compared_bit_for_bit=sum(r.get("samples_compared_bit_for_bit", 0) for r in rows),
                   harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest())
    (ROOT / "docs/native-http/selection-faults.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
