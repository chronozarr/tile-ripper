#!/usr/bin/env python3
"""Bounded, independent qualification of the pinned public DSWx-NI prototype.

This script never asks for ambient credentials and never issues an unbounded or
whole-object GET.  TIFF metadata and selected compressed tiles are fetched only
through conditional Range requests with per-object 2 MiB / 128-request caps.
It records classes actually encountered; absence of a class is not a failure
and is never promoted to a science claim.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import http.server
import io
import json
import subprocess
import tempfile
import threading
import xml.etree.ElementTree as ET
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import requests
import tifffile
import rasterio
import xarray as xr
import zarr
from rasterio.windows import Window
from rasterio.warp import transform

ROOT = Path(__file__).resolve().parents[1]
DOC = ROOT / "docs/products/opera-expansion"
EXAMPLE = ROOT / "examples/opera-dswx-ni-prototype.json"
WORK = ROOT / "prototype-output/opera-expansion/dswx-ni"
MAX_BYTES = 2 * 1024 * 1024
MAX_REQUESTS = 128
PAGE = 4096
SOURCES = {
    "WTR": ("B01_WTR", '"4c24ac7234e0d63d71ade7f8269b455d"', 732280),
    "BWTR": ("B02_BWTR", '"63c17a56666857346c31dd92df615ffd"', 559556),
    "CONF": ("B03_CONF", '"5bcf61a8f64146e91a7d50984b4162d7"', 3242895),
    "DIAG": ("B04_DIAG", '"f67e4e123f22f440267381d6bd2ae57d"', 5731855),
}
PREFIX = "https://opera-pst-rs-pop1.s3.us-west-2.amazonaws.com/transfer/rileykk/dswx_ni/job-WF-SCIFLO_L3_DSWx_NI-MS_41_15-10-20260416T152808.731058Z/pge_output_dir/"
GRANULE = "OPERA_L3_DSWx-NI_T42RVP_20260112T004524Z_20260417T094901Z_LSAR_30_v0.1"


class QualificationError(RuntimeError):
    pass


def require(value, message):
    if not value:
        raise QualificationError(message)


def save(path: Path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + "\n")


def href(band):
    suffix, _, _ = SOURCES[band]
    return f"{PREFIX}{GRANULE}_{suffix}.tif"


class RangeFile(io.RawIOBase):
    """Seekable page cache backed only by checked conditional byte ranges."""

    def __init__(self, band):
        self.band = band
        self.url = href(band)
        _, self.etag, self.size = SOURCES[band]
        self.position = 0
        self.pages = {}
        self.events = []
        self.bytes = 0
        self.http = requests.Session()
        self.http.trust_env = False
        self.http.headers["Accept-Encoding"] = "identity"

    def readable(self):
        return True

    def seekable(self):
        return True

    def tell(self):
        return self.position

    def seek(self, offset, whence=0):
        self.position = offset if whence == 0 else self.position + offset if whence == 1 else self.size + offset
        require(0 <= self.position <= self.size, "TIFF parser sought outside pinned object")
        return self.position

    def _range(self, start, end):
        require(0 <= start <= end < self.size, "invalid source range")
        require(end - start + 1 <= MAX_BYTES, "one source range exceeds cap")
        require(self.bytes + end - start + 1 <= MAX_BYTES, "per-object source-byte cap exceeded")
        require(len(self.events) < MAX_REQUESTS, "per-object request cap exceeded")
        response = self.http.get(
            self.url,
            headers={"Range": f"bytes={start}-{end}", "If-Match": self.etag},
            allow_redirects=False,
            timeout=(10, 45),
            stream=True,
        )
        try:
            require(
                response.status_code == 206,
                f"{self.band} source did not return a bounded partial response (HTTP {response.status_code})",
            )
            require(response.headers.get("ETag") == self.etag, f"{self.band} source ETag changed")
            require(
                response.headers.get("Content-Range") == f"bytes {start}-{end}/{self.size}",
                f"{self.band} source range framing changed: {response.headers.get('Content-Range')}",
            )
            data = response.raw.read(end - start + 2)
        finally:
            response.close()
        require(len(data) == end - start + 1, "source range body length changed")
        self.bytes += len(data)
        self.events.append({"start": start, "end": end, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
        return data

    def read(self, size=-1):
        require(size >= 0, "TIFF parser requested an unbounded source read")
        end = min(self.size, self.position + size)
        current = self.position
        self.position = end
        out = bytearray()
        while current < end:
            page = current // PAGE
            data = self.pages.get(page)
            if data is None:
                start = page * PAGE
                data = self._range(start, min(self.size - 1, start + PAGE - 1))
                self.pages[page] = data
            offset = current - page * PAGE
            take = min(end - current, len(data) - offset)
            out += data[offset:offset + take]
            current += take
        return bytes(out)

    def readinto(self, buffer):
        data = self.read(len(buffer))
        buffer[: len(data)] = data
        return len(data)

    def tile(self, page, index):
        offset = page.dataoffsets[index]
        count = page.databytecounts[index]
        data = self._range(offset, offset + count - 1)
        decoded, _, _ = page.decode(data, index)
        return np.asarray(decoded)[0, :, :, 0]

    def close(self):
        self.http.close()


def inspect_source(band):
    source = RangeFile(band)
    try:
        image = tifffile.TiffFile(source)
        page = image.pages[0]
        require(page.shape == (3660, 3660) and page.dtype == np.dtype("uint8"), "unexpected DSWx-NI image layout")
        require((page.tilelength, page.tilewidth) == (512, 512), "unexpected DSWx-NI tile geometry")
        # Deliberately avoid materializing ColorMap and other large TIFF tags:
        # the GDAL XML is the version-bound metadata contract we retain.
        metadata_tag = page.tags.get("GDAL_METADATA")
        gdal_metadata = str(metadata_tag.value) if metadata_tag else ""
        tags = {}
        if gdal_metadata:
            tags.update(
                {
                    item.attrib["name"]: item.text or ""
                    for item in ET.fromstring(gdal_metadata)
                    if "name" in item.attrib
                }
            )
        # A small, fixed set covers an interior tile, a tile boundary, and
        # candidate edge tiles. We deliberately do not scan all imagery just
        # to manufacture class occurrences.
        sampled = {}
        for index in (0, 1, 7, 8, 9, 56, 63):
            tile = source.tile(page, index)
            values, counts = np.unique(tile, return_counts=True)
            sampled[str(index)] = {str(int(v)): int(c) for v, c in zip(values, counts)}
        return {
            "href": source.url,
            "etag": source.etag,
            "size_bytes": source.size,
            "layout": {
                "shape": list(page.shape), "dtype": str(page.dtype),
                "tile_shape": [page.tilelength, page.tilewidth],
                "compression": str(page.compression), "predictor": str(page.predictor),
                "pages": len(image.pages), "sampled_tile_class_counts": sampled,
            },
            "tags": tags,
            "bounded_ranges": source.events,
            "range_bytes": source.bytes,
            "range_requests": len(source.events),
        }
    finally:
        source.close()


def capture():
    receipt = {
        "schema": "tile-ripper-dswx-ni-prototype-source-receipt-v1",
        "status": "bounded_source_inspection",
        "captured_utc": datetime.now(timezone.utc).isoformat(),
        "scope": "Pinned public engineering TIFF metadata plus selected compressed tiles only; no full-object fallback and no claim of final or production qualification.",
        "caps": {"per_object_bytes": MAX_BYTES, "per_object_requests": MAX_REQUESTS},
        "granule": GRANULE,
        "sources": {band: inspect_source(band) for band in SOURCES},
    }
    # Assert the observed prototype contract once, before writing any receipt.
    for band, value in receipt["sources"].items():
        layout = value["layout"]
        require(layout["shape"] == [3660, 3660] and layout["dtype"] == "uint8", f"{band} layout drift")
        require(layout["tile_shape"] == [512, 512], f"{band} tile layout drift")
        for key, expected in {
            "DSWX_PRODUCT_VERSION": "0.1", "SOFTWARE_VERSION": "0.3.0",
            "PRODUCT_SOURCE": "NISAR_GCOV", "SENSOR": "LSAR", "SPACECRAFT_NAME": "NISAR",
        }.items():
            require(value["tags"].get(key) == expected, f"{band} metadata {key} drift")
    save(DOC / "dswx-ni-source-receipt.json", receipt)
    save(WORK / "source-receipt.json", receipt)
    return receipt


def aoi_for_window(x, y, width, height):
    xs = np.array([399960 + 30 * x, 399960 + 30 * (x + width)])
    ys = np.array([2900040 - 30 * y, 2900040 - 30 * (y + height)])
    lon, lat = transform("EPSG:32642", "EPSG:4326", [xs[0], xs[1], xs[0], xs[1]], [ys[0], ys[0], ys[1], ys[1]])
    return [min(lon), min(lat), max(lon), max(lat)]


def invoke(binary, request, output):
    require(not output.exists(), "qualification output already exists")
    with tempfile.TemporaryDirectory(prefix="tile-ripper-dswx-ni-") as temporary:
        spec = Path(temporary) / "request.json"
        spec.write_text(json.dumps(request))
        completed = subprocess.run(
            [str(binary), "fetch", str(spec), "--output", str(output), "--output-format", "zarr_v2", "--backend", "native", "--workers", "1", "--per-host", "1", "--json"],
            cwd=ROOT, text=True, capture_output=True, timeout=180,
        )
    require(completed.returncode == 0, "Tile-Ripper prototype fetch failed: " + completed.stderr[-400:])
    return json.loads(completed.stdout)


def digest(array):
    return hashlib.sha256(np.ascontiguousarray(array).tobytes()).hexdigest()


def independent_window(band, window):
    """Read one window through GDAL from a proxy that only admits checked ranges."""
    with BoundedProxy(band) as locator:
        with rasterio.Env(GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", GDAL_HTTP_MULTIRANGE="SERIAL", GDAL_NUM_THREADS="1"):
            with rasterio.open(locator) as source:
                require(source.dtypes == ("uint8",), f"{band} GDAL dtype changed")
                require(source.shape == (3660, 3660), f"{band} GDAL shape changed")
                require(source.nodata == 255, f"{band} GDAL nodata changed")
                w = Window(window["x"], window["y"], window["width"], window["height"])
                values = source.read(1, window=w)
                affine = source.window_transform(w)
                return values, affine, str(source.crs)


class BoundedProxy:
    """Local GDAL endpoint whose every upstream byte is an ETag-checked range."""
    def __init__(self, band):
        self.source = RangeFile(band)

    def __enter__(self):
        outer = self
        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"
            def log_message(self, *_): pass
            def do_HEAD(self):
                self.send_response(200); self.send_header("Content-Length", outer.source.size); self.send_header("Accept-Ranges", "bytes"); self.send_header("ETag", outer.source.etag); self.end_headers()
            def do_GET(self):
                header = self.headers.get("Range", "")
                try:
                    unit, bounds = header.split("=", 1); start, end = bounds.split("-", 1)
                    require(unit == "bytes" and start and end, "GDAL attempted non-bounded source request")
                    data = outer.source._range(int(start), int(end))
                except Exception:
                    self.send_error(416); return
                self.send_response(206); self.send_header("Content-Length", len(data)); self.send_header("Content-Range", f"bytes {start}-{end}/{outer.source.size}"); self.send_header("Accept-Ranges", "bytes"); self.send_header("ETag", outer.source.etag); self.end_headers(); self.wfile.write(data)
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True); self.thread.start()
        return f"http://127.0.0.1:{self.server.server_port}/source.tif"

    def __exit__(self, *_):
        self.server.shutdown(); self.server.server_close(); self.thread.join(); self.source.close()


def verify(binary, request, output):
    manifest = json.loads((output / "manifest.json").read_text())
    require(manifest["status"] == "complete", "published product output lacks completion status")
    root = zarr.open_group(str(output), mode="r")
    require(root.attrs["status"] == "complete", "reopened Zarr root lacks completion status")
    expected_assets = {f"p000_{band}" for band in SOURCES}
    require({x["asset_id"] for x in manifest["outputs"]} == expected_assets, "not all four layers were written")
    sources = {x["asset_id"]: x for x in manifest["sources"]}
    results = []
    for entry in manifest["outputs"]:
        band = entry["asset_id"].split("_", 1)[1]
        source = sources[entry["asset_id"]]
        _, etag, _ = SOURCES[band]
        require(source["reference"] == href(band) and source["object_version"] == etag, f"{band} provenance pin changed")
        raw, affine, crs = independent_window(band, entry["window"])
        x = affine.c + (np.arange(raw.shape[1]) + .5) * affine.a
        y = affine.f + (np.arange(raw.shape[0]) + .5) * affine.e
        xx, yy = np.meshgrid(x, y)
        lon, lat = transform("EPSG:32642", "EPSG:4326", xx.ravel(), yy.ravel())
        west, south, east, north = request["aoi"]["bbox"]
        aoi = ((np.asarray(lon).reshape(raw.shape) >= west) & (np.asarray(lon).reshape(raw.shape) <= east) & (np.asarray(lat).reshape(raw.shape) >= south) & (np.asarray(lat).reshape(raw.shape) <= north))
        if not entry["aoi_mask_applied"]:
            aoi[:] = True
        expected = raw.copy()
        expected[~aoi] = 255
        valid = aoi & (expected != 255)
        group = root[entry["group"]]
        with xr.open_zarr(str(output), group=entry["group"], consolidated=False, decode_cf=False, chunks=None) as ds:
            ds.load()
            actual = ds["values"].values[0]
            require(actual.dtype == expected.dtype and actual.tobytes() == expected.tobytes(), f"{band} raw source bits differ")
            np.testing.assert_array_equal(ds["valid"].values[0], valid.astype("u1"))
            np.testing.assert_array_equal(ds["aoi_mask"].values[0], aoi.astype("u1"))
            np.testing.assert_allclose(ds.x.values, x, rtol=0, atol=1e-8)
            np.testing.assert_allclose(ds.y.values, y, rtol=0, atol=1e-8)
            require(group.attrs["crs"] == crs and group.attrs["source"] == source, f"{band} Zarr provenance/CRS differs")
            wanted = {"WTR": {"dswx_ni_wtr_openwater": [1], "dswx_ni_wtr_water": [1, 3], "dswx_ni_wtr_edgewater": [1, 3, 8]}, "BWTR": {"dswx_ni_bwtr_water": [1]}}.get(band, {})
            for name, codes in wanted.items():
                key = "quality_" + name
                require(key in ds, f"{band} missing {key}")
                np.testing.assert_array_equal(ds[key].values[0], (valid & np.isin(expected, codes)).astype("u1"))
            results.append({"band": band, "window": entry["window"], "values_sha256": digest(expected), "values_equal_bits": True, "valid_equal": True, "aoi_equal": True, "coordinates_equal": True, "quality_masks_equal": True, "class_counts": {str(int(v)): int(n) for v, n in zip(*np.unique(raw, return_counts=True))}})
    # Stream values are provisional; compare every native chunk byte-for-byte
    # with the durable output and require a validated completion event.
    with tempfile.TemporaryDirectory(prefix="tile-ripper-dswx-ni-stream-") as temporary:
        spec = Path(temporary) / "request.json"; spec.write_text(json.dumps(request))
        stream = subprocess.run([str(binary), "stream", str(spec), "--backend", "native", "--workers", "1", "--per-host", "1", "--values", "--json"], cwd=ROOT, text=True, capture_output=True, timeout=180)
    require(stream.returncode == 0, "prototype stream failed: " + stream.stderr[-400:])
    rows = [json.loads(line) for line in stream.stdout.splitlines()]
    require(rows and rows[-1].get("event") == "complete" and rows[-1].get("source_versions_validated"), "stream did not finish with validation")
    covered = {x["asset_id"]: np.zeros((x["window"]["height"], x["window"]["width"]), dtype=bool) for x in manifest["outputs"]}
    by_id = {x["asset_id"]: x for x in manifest["outputs"]}
    chunks = 0
    for row in rows:
        if row.get("event") != "chunk":
            continue
        chunks += 1; entry = by_id[row["asset_id"]]; w = row["window"]; whole = entry["window"]
        y0, x0 = w["y"] - whole["y"], w["x"] - whole["x"]
        selection = (0, slice(y0, y0 + w["height"]), slice(x0, x0 + w["width"]))
        actual = np.frombuffer(base64.b64decode(row["values_le_base64"]), dtype="u1").reshape(w["height"], w["width"])
        np.testing.assert_array_equal(actual, root[entry["group"]]["values"][selection])
        np.testing.assert_array_equal(np.frombuffer(base64.b64decode(row["aoi_mask_base64"]), dtype="u1").reshape(actual.shape), root[entry["group"]]["aoi_mask"][selection])
        np.testing.assert_array_equal(np.frombuffer(base64.b64decode(row["source_valid_mask_base64"]), dtype="u1").reshape(actual.shape), (actual != 255).astype("u1"))
        covered[row["asset_id"]][y0:y0+w["height"], x0:x0+w["width"]] = True
    require(chunks > 0 and all(mask.all() for mask in covered.values()), "stream omitted output cells")
    return {"status": "passed", "independent_reader": "rasterio " + rasterio.__version__, "reopen_reader": "xarray " + xr.__version__, "layers": results, "stream": {"chunks": chunks, "values_aoi_source_valid_equal": True, "completion_versions_validated": True}}


def qualify(binary):
    receipt = json.loads((DOC / "dswx-ni-source-receipt.json").read_text())
    cases = {
        "interior": {"x": 100, "y": 100, "width": 24, "height": 24},
        "fill_edge": {"x": 20, "y": 3600, "width": 24, "height": 24},
        "tile_crossing": {"x": 500, "y": 500, "width": 24, "height": 24},
    }
    comparisons = {}
    for name, window in cases.items():
        request = json.loads(EXAMPLE.read_text())
        request["aoi"]["bbox"] = aoi_for_window(**window)
        output = WORK / ("qualified-verified.zarr" if name == "tile_crossing" else f"qualified-{name}.zarr")
        result = {"reused_completed_output": True} if output.exists() else invoke(binary, request, output)
        comparisons[name] = {"window": window, "aoi": request["aoi"]["bbox"], "output": str(output.relative_to(ROOT)), "fetch": result, "comparison": verify(binary, request, output)}
    manifest = json.loads((WORK / "qualified-verified.zarr" / "manifest.json").read_text())
    # The binary's semantic output is compared by the durable-output checker
    # when xarray/zarr are installed. Keep this receipt compact and bind every
    # selected source identity rather than copying arrays into documentation.
    observed = manifest.get("observed", {})
    report = {
        "schema": "tile-ripper-dswx-ni-prototype-qualification-v1",
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "status": "passed",
        "scope": "one pinned public engineering prototype, bounded tile-crossing AOI; no production/final-v1.0 claim",
        "windows": cases,
        "source_receipt": "dswx-ni-source-receipt.json",
        "observed": observed,
        "comparisons": comparisons,
    }
    save(DOC / "dswx-ni-qualification.json", report)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture", action="store_true", help="inspect selected header/tile ranges and write a compact receipt")
    parser.add_argument("--qualify", action="store_true", help="run the supplied native binary against a tile-crossing AOI")
    parser.add_argument("--binary", type=Path, help="native-Cog Tile-Ripper binary; required for --qualify")
    args = parser.parse_args()
    if not args.capture and not args.qualify:
        parser.error("choose --capture and/or --qualify")
    if args.capture:
        capture()
    if args.qualify:
        require(args.binary is not None and args.binary.is_file(), "--qualify requires an existing --binary")
        require((DOC / "dswx-ni-source-receipt.json").exists(), "run --capture first")
        qualify(args.binary)


if __name__ == "__main__":
    try:
        main()
    except QualificationError as error:
        raise SystemExit(f"DSWx-NI qualification failed: {error}")
