"""Native-reader science/profile/transport faults. Loopback only; generated fixtures."""

import hashlib
import http.server
import json
import os
from pathlib import Path
import struct
import threading
import zlib

import numpy as np
import rasterio
from rasterio.transform import from_origin
from rasterio.windows import Window
import consumer_validation as consumer

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/native-cog/checks"
BIN = ROOT / "prototype-output/native-cog/native/tile-ripper-native-stats"
consumer.BIN = BIN


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def do_HEAD(self):
        self.serve(False)

    def do_GET(self):
        self.serve(True)

    def serve(self, body):
        s = self.server
        s.heads += not body
        s.gets += body
        if self.headers.get("If-Match") not in (None, s.etag):
            self.send_error(412)
            return
        data = s.path.read_bytes()
        lo, hi = 0, len(data) - 1
        if body and self.headers.get("Range"):
            lo, hi = map(int, self.headers["Range"][6:].split("-"))
            hi = min(hi, len(data) - 1)
            s.conditions.append(self.headers.get("If-Match"))
        mode = s.mode if body else ""
        if mode == "503" or (mode == "503-once" and s.gets == 1):
            self.send_error(503)
            return
        if mode == "412":
            self.send_error(412)
            return
        status = 200 if mode == "200" or not body else 206
        self.send_response(status)
        if mode != "missing-etag":
            self.send_header("ETag", '"changed"' if mode == "wrong-etag" else s.etag)
        length = hi - lo + 1
        self.send_header(
            "Content-Length", str(length + 1 if mode == "length" else length)
        )
        if body:
            self.send_header(
                "Content-Range",
                f"bytes {lo + int(mode == 'range')}-{hi}/{len(data) + int(mode == 'total')}",
            )
        if mode == "encoding":
            self.send_header("Content-Encoding", "gzip")
        short = mode == "short" or (mode == "short-once" and s.gets == 1)
        if short:
            self.send_header("Connection", "close")
        self.end_headers()
        if body:
            try:
                self.wfile.write(data[lo:hi] if short else data[lo : hi + 1])
            except (BrokenPipeError, ConnectionResetError):
                pass
        if short:
            self.close_connection = True


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def handle_error(self, *args):
        pass


def request(path, edge=16, buffer=16 * 2**20):
    return dict(
        source=dict(
            kind="assets",
            assets=[
                dict(
                    id="science",
                    locator=str(path),
                    format="cog",
                    datetime="2025-01-01T00:00:00Z",
                )
            ],
        ),
        spatial=dict(
            kind="bounds", crs="EPSG:32617", bbox=[600030, 3999370, 600710, 3999950]
        ),
        variables=[dict(kind="band", index=1)],
        grid=dict(kind="native"),
        sink=dict(kind="callback"),
        budget=dict(buffer_bytes=buffer, max_nodes=100000, chunk_edge=edge),
    )


def fixture(path, **opts):
    path.unlink(missing_ok=True)
    config = dict(
        driver="GTiff",
        width=80,
        height=64,
        count=1,
        dtype="uint16",
        tiled=True,
        blockxsize=32,
        blockysize=32,
        crs="EPSG:32617",
        transform=from_origin(600000, 4000000, 10, 10),
        nodata=65535,
        compress="deflate",
        predictor=2,
    )
    config.update(opts)
    values = (np.arange(80 * 64, dtype=np.uint16) * 17).reshape(64, 80)
    values[::3, ::5] = 65535
    values[0, 0] = 0
    with (
        rasterio.Env(GDAL_TIFF_INTERNAL_MASK=True),
        rasterio.open(path, "w", **config) as ds,
    ):
        ds.write(values.astype(config["dtype"]), 1)
        ds.scales = [0.0001]
        ds.offsets = [-0.1]
    return path


def oracle(result, path, req):
    assert result["code"] == 0, result
    rows = result["rows"]
    assert rows[-1]["type"] == "complete"
    assert rows[0]["provenance"]["sources"]
    covered = set()
    chunks = rows[1:-1]
    with rasterio.open(path) as ds:
        c0, r0 = (~ds.transform) * (
            req["spatial"]["bbox"][0],
            req["spatial"]["bbox"][3],
        )
        c1, r1 = (~ds.transform) * (
            req["spatial"]["bbox"][2],
            req["spatial"]["bbox"][1],
        )
        edge = req["budget"]["chunk_edge"]
        expected = {
            (x, y, min(edge, int(c1) - x), min(edge, int(r1) - y))
            for y in range(int(r0), int(r1), edge)
            for x in range(int(c0), int(c1), edge)
        }
        for c in chunks:
            w = c["window"]
            key = tuple(w[k] for k in ["x", "y", "width", "height"])
            assert key not in covered
            covered.add(key)
            win = Window(*key)
            a = ds.read(1, window=win)
            valid = ds.read_masks(1, window=win) != 0
            assert c["adler32_le"] == zlib.adler32(a.astype("<u2").tobytes())
            assert c["valid_pixels"] == int(valid.sum()) and c["native_sum"] == float(
                a[valid].sum(dtype=np.float64)
            )
            assert c["transform"] == list(ds.window_transform(win).to_gdal()) and c[
                "crs"
            ] == str(ds.crs)
            assert (c["nodata"], c["scale"], c["offset"]) == (
                ds.nodata,
                ds.scales[0],
                ds.offsets[0],
            )
            assert c["datetime"] == req["source"]["assets"][0]["datetime"]
        assert covered == expected
    assert result["peak_rss_bytes"] <= 192 * 2**20


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    rows = []

    def run(name, req, success=False, path=None, args=(), mutate=None):
        result = consumer.invoke(req, WORK / f"{name}.json", args=args, mutate=mutate)
        complete = any(r["type"] == "complete" for r in result["rows"])
        if success:
            oracle(result, path, req)
        else:
            assert result["code"] != 0 and not complete, (name, result)
        row = dict(
            name=name,
            passed=True,
            exit_code=result["code"],
            chunks=sum(r["type"] == "provisional_chunk" for r in result["rows"]),
            peak_rss_bytes=result["peak_rss_bytes"],
            seconds=result["seconds"],
            last=result["rows"][-1] if result["rows"] else None,
        )
        rows.append(row)
        print(name, "PASS", flush=True)
        return result

    for compress, predictor in [
        (None, 1),
        ("deflate", 1),
        ("deflate", 2),
        ("lzw", 1),
        ("lzw", 2),
    ]:
        for big, endian in [
            ("NO", "LITTLE"),
            ("NO", "BIG"),
            ("YES", "LITTLE"),
            ("YES", "BIG"),
        ]:
            name = f"profile-{compress}-{predictor}-{big}-{endian}"
            path = fixture(
                WORK / f"{name}.tif",
                compress=compress,
                predictor=predictor,
                BIGTIFF=big,
                ENDIANNESS=endian,
            )
            run(name, request(path), True, path)
    for nodata in [None, 0]:
        name = "nodata-" + str(nodata)
        p = fixture(WORK / f"{name}.tif", nodata=nodata)
        run(name, request(p), True, p)
    base = fixture(WORK / "base.tif")
    p = fixture(WORK / "pixel-is-point.tif")
    with rasterio.open(p, "r+") as ds:
        ds.update_tags(AREA_OR_POINT="Point")
    run("pixel-is-point", request(p))
    # Profiles whose source validity/grid semantics cannot be preserved are rejected.
    for name, opts in [
        ("stripped", dict(tiled=False, blockysize=16)),
        ("float", dict(dtype="float32", predictor=1)),
        ("oversized-tile", dict(blockxsize=2048, blockysize=2048)),
        ("rotated", dict(transform=rasterio.Affine(10, 1, 600000, 0, -10, 4000000))),
    ]:
        path = fixture(WORK / f"{name}.tif", **opts)
        run(name, request(path))
    path = fixture(WORK / "mask.tif")
    with rasterio.Env(GDAL_TIFF_INTERNAL_MASK=True), rasterio.open(path, "r+") as ds:
        ds.write_mask(np.zeros((64, 80), np.uint8))
    run("explicit-mask", request(path))
    req = request(base)
    req["spatial"]["crs"] = "EPSG:4326"
    run("foreign-crs", req)
    req = request(base)
    req["variables"][0]["index"] = 2
    run("band2", req)
    run("buffer-bound", request(base, buffer=4096))
    # Corrupt the actual classic IFD/tag arrays, retaining otherwise valid files.
    original = base.read_bytes()
    at = struct.unpack_from("<I", original, 4)[0]

    def patch(name, edit):
        data = bytearray(original)
        edit(data)
        p = WORK / f"{name}.tif"
        p.write_bytes(data)
        run(name, request(p))

    patch("bad-magic", lambda d: d.__setitem__(slice(0, 2), b"XX"))
    patch("too-many-tags", lambda d: struct.pack_into("<H", d, at, 65535))
    n = struct.unpack_from("<H", original, at)[0]
    entries = {
        struct.unpack_from("<H", original, at + 2 + j * 12)[0]: at + 2 + j * 12
        for j in range(n)
    }
    patch(
        "tag-value-overflow",
        lambda d: struct.pack_into("<I", d, entries[324] + 4, 0xFFFFFFFF),
    )
    patch("ifd-cycle", lambda d: struct.pack_into("<I", d, at + 2 + n * 12, at))
    patch(
        "invalid-xml",
        lambda d: d.__setitem__(
            slice(d.find(b"<GDALMetadata>"), d.find(b"<GDALMetadata>") + 1), b"!"
        ),
    )
    patch(
        "unknown-compression",
        lambda d: struct.pack_into("<H", d, entries[259] + 8, 999),
    )
    patch(
        "bad-geokeys",
        lambda d: struct.pack_into("<I", d, entries[34735] + 8, len(d) + 100),
    )
    offsets_entry = entries[324]
    offsets = struct.unpack_from("<I", original, offsets_entry + 8)[0]
    patch(
        "tile-outside-file", lambda d: struct.pack_into("<I", d, offsets, len(d) + 100)
    )
    first = struct.unpack_from("<I", original, offsets)[0]
    patch(
        "corrupt-deflate", lambda d: d.__setitem__(slice(first, first + 8), b"broken!!")
    )
    p = WORK / "truncated.tif"
    p.write_bytes(original[:-100])
    run("truncated-file", request(p))
    # Uncompressed block length mismatch must fail before copying output.
    p = fixture(WORK / "short-block.tif", compress=None, predictor=1)
    raw = bytearray(p.read_bytes())
    a = struct.unpack_from("<I", raw, 4)[0]
    n = struct.unpack_from("<H", raw, a)[0]
    for j in range(n):
        e = a + 2 + j * 12
        if struct.unpack_from("<H", raw, e)[0] == 325:
            ptr = struct.unpack_from("<I", raw, e + 8)[0]
            struct.pack_into("<I", raw, ptr, 1)
    p.write_bytes(raw)
    run("decoded-length", request(p))
    # A tiny compressed tile claiming a much larger output must stay bounded.
    data = bytearray(original)
    counts_entry = entries[325]
    counts_at = struct.unpack_from("<I", data, counts_entry + 8)[0]
    bomb = zlib.compress(b"\0" * (4 * 1024 * 1024))
    data.extend(bomb)
    struct.pack_into("<I", data, offsets, len(data) - len(bomb))
    count_type = struct.unpack_from("<H", data, counts_entry + 2)[0]
    struct.pack_into("<H" if count_type == 3 else "<I", data, counts_at, len(bomb))
    p = WORK / "deflate-output-limit.tif"
    p.write_bytes(data)
    run("deflate-output-limit", request(p))
    # The same metadata parser must enforce BigTIFF 64-bit allocation limits.
    p = fixture(WORK / "big-overflow.tif", BIGTIFF="YES")
    data = bytearray(p.read_bytes())
    a = struct.unpack_from("<Q", data, 8)[0]
    struct.pack_into("<Q", data, a, 0xFFFFFFFFFFFFFFFF)
    p.write_bytes(data)
    run("bigtiff-ifd-overflow", request(p))
    server = Server(("127.0.0.1", 0), Handler)
    server.path = base
    server.etag = '"frozen"'
    server.mode = ""
    server.gets = server.heads = 0
    server.conditions = []
    threading.Thread(target=server.serve_forever, daemon=True).start()
    req = request(f"http://127.0.0.1:{server.server_port}/private.tif?secret=canary")
    req["source"]["assets"][0]["etag"] = server.etag
    try:
        for mode in [
            "",
            "503-once",
            "short-once",
            "503",
            "short",
            "200",
            "range",
            "total",
            "length",
            "missing-etag",
            "wrong-etag",
            "encoding",
            "412",
        ]:
            server.mode = mode
            server.gets = server.heads = 0
            server.conditions = []
            result = run(
                "http-" + (mode or "correct"),
                req,
                mode in ("", "503-once", "short-once"),
                base,
            )
            assert all(v == '"frozen"' for v in server.conditions)
            assert "canary" not in json.dumps(result["rows"]) + result["stderr"]
            rows[-1].update(gets=server.gets, heads=server.heads)
        server.mode = ""
        server.etag = '"frozen"'
        server.gets = 0
        run(
            "late-remote-mutation",
            req,
            args=["--pause-after", "1"],
            mutate=lambda _: setattr(server, "etag", '"changed"'),
        )
        server.etag = '"frozen"'
        run("cancel", req, args=["--cancel-after", "1"])
        run("sink-failure", req, args=["--max-chunk-bytes", "1"])
    finally:
        server.shutdown()
        server.server_close()
    p = fixture(WORK / "local-mutation.tif")
    run(
        "late-local-mutation",
        request(p),
        args=["--pause-after", "1"],
        mutate=lambda _: os.utime(p, None),
    )
    report = dict(
        passed=True,
        checks=rows,
        binary_sha256=hashlib.sha256(BIN.read_bytes()).hexdigest(),
        harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
    )
    (WORK / "report.json").write_text(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
