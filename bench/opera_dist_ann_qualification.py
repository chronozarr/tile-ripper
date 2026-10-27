#!/usr/bin/env python3
"""Authenticated, bounded DIST-ANN-HLS v1 qualification.

The only credential input is the explicitly passed private netrc.  URS receives
basic authentication; LP DAAC resource origins receive no credentials.  Signed
resource URLs remain process-memory only and never enter a receipt or log.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
import netrc
import shutil
import stat
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urljoin, urlsplit

import numpy as np
import requests
import rasterio
import tifffile
import xarray as xr
import zarr
from rasterio.transform import Affine
from rasterio.warp import transform

ROOT = Path(__file__).resolve().parents[1]
EXAMPLE = ROOT / "examples/opera-dist-ann-hls.json"
WORK = ROOT / "prototype-output/opera-expansion/dist-ann"
DOC_RECEIPT = ROOT / "docs/products/opera-expansion/dist-ann-qualification-receipt.json"
SOURCE_RECEIPT = ROOT / "docs/products/opera-expansion/dist-ann-source-receipt.json"
ARCHIVE = "https://data.lpdaac.earthdatacloud.nasa.gov"
URS = "https://urs.earthdata.nasa.gov"
RESOURCE = "https://d1nklfio7vscoe.cloudfront.net"
MAX_SOURCE_BYTES = 8 * 1024 * 1024
MAX_RANGE = 2 * 1024 * 1024


class QualificationError(RuntimeError):
    pass


def require(value, message):
    if not value:
        raise QualificationError(message)


def dump(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + "\n")


def clean(value):
    """Keep a receipt useful without retaining ephemeral source locations."""
    if isinstance(value, dict):
        return {k: clean(v) for k, v in value.items() if k not in {"href", "reference", "canonical_locator"}}
    if isinstance(value, list):
        return [clean(v) for v in value]
    return value


def origin(url):
    p = urlsplit(url)
    return f"{p.scheme}://{p.netloc}"


def private_credentials(path):
    mode = path.stat().st_mode
    require(stat.S_ISREG(mode) and mode & 0o077 == 0, "explicit netrc must be a regular mode-0600 file")
    auth = netrc.netrc(str(path)).authenticators("urs.earthdata.nasa.gov")
    require(auth and auth[0] and auth[2], "explicit netrc lacks URS credentials")
    return auth[0], auth[2]


def http_session():
    session = requests.Session()
    session.trust_env = False
    session.headers.update({"Accept-Encoding": "identity", "User-Agent": "tile-ripper-dist-ann-independent/1"})
    return session


def resolve_source(canonical, expected_etag, expected_size, credentials):
    """Resolve LP DAAC redirects while sending credentials only to URS."""
    current = canonical
    session = http_session()
    trace = []
    try:
        for _ in range(10):
            src = origin(current)
            require(urlsplit(current).scheme == "https" and src in {ARCHIVE, URS, RESOURCE}, "source redirect used an unadmitted origin")
            response = session.get(
                current,
                headers={"Range": "bytes=0-0"} if src != URS else {},
                auth=credentials if src == URS else None,
                allow_redirects=False,
                stream=True,
                timeout=(15, 60),
            )
            if response.status_code in (301, 302, 303, 307, 308):
                target = urljoin(current, response.headers.get("Location", ""))
                dst = origin(target)
                require(urlsplit(target).scheme == "https" and dst in {ARCHIVE, URS, RESOURCE}, "source redirect target used an unadmitted origin")
                if src == URS:
                    require(dst == ARCHIVE, "URS redirect did not return to LP DAAC")
                elif src == RESOURCE:
                    raise QualificationError("resource host unexpectedly redirected")
                trace.append({"from": src, "to": dst, "status": response.status_code, "temporary_query_present": bool(urlsplit(target).query)})
                response.close()
                current = target
                continue
            require(response.status_code == 206, f"source probe returned HTTP {response.status_code}")
            match = response.headers.get("Content-Range", "")
            require(match == f"bytes 0-0/{expected_size}", "source probe size changed")
            require(response.headers.get("ETag") == expected_etag, "source probe ETag changed")
            require(len(response.raw.read(2)) == 1, "source probe body changed")
            response.close()
            return session, current, trace
        raise QualificationError("source redirect budget exceeded")
    except Exception:
        session.close()
        raise


class RangeFile(io.RawIOBase):
    """Seekable, ETag-bound TIFF reader with a strict 8 MiB/object cap."""
    def __init__(self, asset, header, credentials):
        self.asset = asset
        self.etag = header["etag"]
        self.size = header["size_bytes"]
        self.session, self.resource, self.trace = resolve_source(asset["href"], self.etag, self.size, credentials)
        self.position = 0
        self.pages = {}
        self.bytes = 0
        self.events = []

    def readable(self): return True
    def seekable(self): return True
    def tell(self): return self.position
    def seek(self, offset, whence=0):
        self.position = offset if whence == 0 else self.position + offset if whence == 1 else self.size + offset
        require(0 <= self.position <= self.size, "TIFF parser sought outside source object")
        return self.position

    def _range(self, start, end):
        require(0 <= start <= end < self.size, "invalid source range")
        require(end - start + 1 <= MAX_RANGE, "one source range exceeds 2 MiB cap")
        require(self.bytes + end - start + 1 <= MAX_SOURCE_BYTES, "independent source read exceeds 8 MiB/object cap")
        response = self.session.get(
            self.resource,
            headers={"Range": f"bytes={start}-{end}", "If-Match": self.etag},
            allow_redirects=False, stream=True, timeout=(15, 60),
        )
        try:
            require(response.status_code == 206, f"source range returned HTTP {response.status_code}")
            require(response.headers.get("ETag") == self.etag, "source range ETag changed")
            require(response.headers.get("Content-Range") == f"bytes {start}-{end}/{self.size}", "source range framing changed")
            data = response.raw.read(end - start + 2)
        finally:
            response.close()
        require(len(data) == end - start + 1, "source range body length changed")
        self.bytes += len(data)
        self.events.append({"start": start, "end": end, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
        return data

    def read(self, size=-1):
        require(size >= 0, "TIFF parser requested an unbounded source read")
        stop = min(self.size, self.position + size)
        out = bytearray()
        while self.position < stop:
            page = self.position // 4096
            data = self.pages.get(page)
            if data is None:
                start = page * 4096
                data = self._range(start, min(self.size - 1, start + 4095))
                self.pages[page] = data
            offset = self.position - page * 4096
            take = min(stop - self.position, len(data) - offset)
            out += data[offset:offset + take]
            self.position += take
        return bytes(out)

    def readinto(self, target):
        data = self.read(len(target)); target[:len(data)] = data; return len(data)
    def close(self):
        self.session.close()
        super().close()


def source_window(asset, header, window, credentials):
    """Decode just the TIFF tiles intersecting one delivered native window."""
    remote = RangeFile(asset, header, credentials)
    try:
        image = tifffile.TiffFile(remote)
        page = image.pages[0]
        dtype = np.dtype(header["dtype"])
        require(page.shape == tuple(header["shape"]) and page.dtype == dtype, "independent TIFF layout changed")
        require(page.is_tiled, "annual source is no longer tiled")
        x, y, width, height = (window[k] for k in ("x", "y", "width", "height"))
        require(0 <= x < page.imagewidth and 0 <= y < page.imagelength and x + width <= page.imagewidth and y + height <= page.imagelength, "delivered window outside source")
        result = np.empty((height, width), dtype=dtype)
        tw, th = page.tilewidth, page.tilelength
        across = (page.imagewidth + tw - 1) // tw
        for ty in range(y // th, (y + height - 1) // th + 1):
            for tx in range(x // tw, (x + width - 1) // tw + 1):
                index = ty * across + tx
                offset, count = page.dataoffsets[index], page.databytecounts[index]
                require(count <= MAX_RANGE, "one compressed TIFF tile exceeds bounded range cap")
                decoded, _, _ = page.decode(remote._range(offset, offset + count - 1), index)
                tile = np.asarray(decoded)[0, :, :, 0]
                sx0, sy0 = tx * tw, ty * th
                sx1, sy1 = min(sx0 + tw, page.imagewidth), min(sy0 + th, page.imagelength)
                ix0, iy0 = max(x, sx0), max(y, sy0)
                ix1, iy1 = min(x + width, sx1), min(y + height, sy1)
                result[iy0-y:iy1-y, ix0-x:ix1-x] = tile[iy0-sy0:iy1-sy0, ix0-sx0:ix1-sx0]
        return result, {"ranges": len(remote.events), "bytes": remote.bytes, "redirects": [{"from": x["from"], "to": x["to"], "status": x["status"]} for x in remote.trace]}
    finally:
        remote.close()


def aoi_mask(request, header, window, apply):
    if not apply:
        return np.ones((window["height"], window["width"]), dtype=bool)
    west, south, east, north = request["aoi"]["bbox"]
    affine = Affine(*header["transform"])
    shifted = affine * Affine.translation(window["x"], window["y"])
    # Native geographic-bound selection is defined at every pixel centre.  A
    # four-corner transformed polygon bends UTM edges into chords and is not a
    # valid oracle for a broad AOI, so transform centre coordinates directly.
    xs = shifted.c + (np.arange(window["width"]) + 0.5) * shifted.a
    ys = shifted.f + (np.arange(window["height"]) + 0.5) * shifted.e
    included = np.zeros((len(ys), len(xs)), dtype=bool)
    for start in range(0, len(ys), 128):
        stop = min(start + 128, len(ys))
        xx = np.broadcast_to(xs, (stop - start, len(xs))).ravel()
        yy = np.repeat(ys[start:stop], len(xs))
        lon, lat = transform(header["crs"], "EPSG:4326", xx, yy)
        included[start:stop] = (
            (np.asarray(lon).reshape(stop - start, len(xs)) >= west)
            & (np.asarray(lon).reshape(stop - start, len(xs)) <= east)
            & (np.asarray(lat).reshape(stop - start, len(xs)) >= south)
            & (np.asarray(lat).reshape(stop - start, len(xs)) <= north)
        )
    return included


def epoch_ns(value):
    return int(np.datetime64(value.removesuffix("Z"), "ns").astype("int64"))


def expected_quality(name, raw, valid):
    if name.endswith("current_year_confirmed"):
        codes = (3, 6, 7, 8)
    elif name.endswith("previous_year_confirmed"):
        codes = (9, 10)
    else:
        codes = (3, 6, 7, 8, 9, 10)
    return (valid & np.isin(raw, codes)).astype("u1")


def invoke(binary, netrc_path, request, output, label, stream=False):
    logs = WORK / "runs"
    logs.mkdir(parents=True, exist_ok=True)
    # The output path is a fresh-attempt identity.  Do not overwrite prior
    # stdout/stderr evidence when a transient network retry is necessary.
    attempt = f"{output.stem}-{label}"
    request_path = logs / f"{attempt}-request.json"
    stdout = logs / f"{attempt}-{'stream' if stream else 'fetch'}.stdout"
    stderr = logs / f"{attempt}-{'stream' if stream else 'fetch'}.stderr"
    dump(request_path, request)
    command = [str(binary), "stream" if stream else "fetch", str(request_path), "--gdal-free", "--auth", "earthdata", "--netrc", str(netrc_path), "--backend", "native", "--workers", "1", "--per-host", "1", "--json"]
    if stream:
        command.append("--values")
    else:
        command += ["--output", str(output), "--output-format", "zarr_v2"]
    with stdout.open("w") as out, stderr.open("w") as err:
        completed = subprocess.run(command, cwd=ROOT, stdout=out, stderr=err, text=True, timeout=600)
    require(completed.returncode == 0, f"{label} {'stream' if stream else 'fetch'} failed; inspect ignored run logs")
    return stdout


def inspect_case(binary, netrc_path, request, output, label, credentials, headers, replay=False):
    if replay:
        require(output.is_dir(), f"replay output is absent: {output}")
    else:
        require(not output.exists(), f"refusing to overwrite existing output {output.name}")
        invoke(binary, netrc_path, request, output, label)
    manifest = json.loads((output / "manifest.json").read_text())
    require(manifest.get("status") == "complete", "durable output is not complete")
    root = zarr.open_group(str(output), mode="r")
    require(root.attrs.get("status") == "complete", "reopened Zarr root is not complete")
    expected_assets = {f"p000_{name}" for name in request["bands"]}
    require({row["asset_id"] for row in manifest["outputs"]} == expected_assets, "durable output omitted a requested layer")
    sources = {row["asset_id"]: row for row in manifest["sources"]}
    results = []
    for entry in manifest["outputs"]:
        asset_id = entry["asset_id"]
        band = asset_id.removeprefix("p000_")
        header = headers[band]
        asset = request["observations"][0]["assets"][band]
        source = sources[asset_id]
        require(source["object_version"] == header["etag"] and source["requested_band"] == band, f"{band}: provenance version changed")
        raw, source_io = source_window(asset, header, entry["window"], credentials)
        aoi = aoi_mask(request, header, entry["window"], entry["aoi_mask_applied"])
        delivered = raw.copy()
        nodata = np.asarray(header["nodata"], dtype=raw.dtype).item()
        delivered[~aoi] = nodata
        valid = aoi & (raw != nodata)
        affine = Affine(*header["transform"]) * Affine.translation(entry["window"]["x"], entry["window"]["y"])
        x = affine.c + (np.arange(raw.shape[1]) + 0.5) * affine.a
        y = affine.f + (np.arange(raw.shape[0]) + 0.5) * affine.e
        with xr.open_zarr(str(output), group=entry["group"], consolidated=False, decode_cf=False, chunks=None) as ds:
            ds.load()
            actual = ds["values"].values[0]
            if actual.dtype != delivered.dtype or actual.tobytes() != delivered.tobytes():
                where = np.argwhere(actual != delivered)
                if where.size:
                    yy, xx = map(int, where[0])
                    raise QualificationError(
                        f"{band}: durable/source bits differ at delivered[{yy},{xx}] "
                        f"source_pixel[{entry['window']['y'] + yy},{entry['window']['x'] + xx}]: "
                        f"durable={actual[yy, xx]!r}, independent={delivered[yy, xx]!r}, "
                        f"raw={raw[yy, xx]!r}, aoi={int(aoi[yy, xx])}, nodata={nodata!r}"
                    )
                raise QualificationError(f"{band}: durable/source dtype differs: {actual.dtype} vs {delivered.dtype}")
            np.testing.assert_array_equal(ds["valid"].values[0], valid.astype("u1"))
            np.testing.assert_array_equal(ds["aoi_mask"].values[0], aoi.astype("u1"))
            np.testing.assert_allclose(ds.x.values, x, rtol=0, atol=1e-8)
            np.testing.assert_allclose(ds.y.values, y, rtol=0, atol=1e-8)
            group = root[entry["group"]]
            require(group.attrs["crs"] == header["crs"] and group.attrs["source"] == source, f"{band}: CRS or source provenance changed")
            if band.endswith("DIST-STATUS"):
                for policy in request["quality_policies"]:
                    if (policy.startswith("dist_ann_veg") and band.startswith("VEG-")) or (policy.startswith("dist_ann_gen") and band.startswith("GEN-")):
                        key = "quality_" + policy
                        require(key in ds, f"{band}: missing durable {key}")
                        np.testing.assert_array_equal(ds[key].values[0], expected_quality(policy, raw, valid))
            else:
                require(not any(key.startswith("quality_") for key in ds.data_vars), f"{band}: unexpected cross-band quality mask")
            start = request["observations"][0]["properties"]["temporal_extent"]["coverage_start"]
            end = request["observations"][0]["properties"]["temporal_extent"]["coverage_end"]
            np.testing.assert_array_equal(ds["time"].values, np.array([epoch_ns(start)], dtype="i8"))
            np.testing.assert_array_equal(ds["time_bounds"].values, np.array([[epoch_ns(start), epoch_ns(end)]], dtype="i8"))
        values, counts = np.unique(raw, return_counts=True)
        observed = {str(int(v)): int(n) for v, n in zip(values, counts)}
        # Exact categorical counts are useful for the two status masks.  For
        # continuous/count/date layers, retain an honest compact summary
        # rather than turning the tracked receipt into a value inventory.
        if not band.endswith("DIST-STATUS"):
            observed = {"distinct_values": int(len(values)), "minimum": int(values.min()), "maximum": int(values.max()), "source_nodata_count": int(np.count_nonzero(raw == nodata))}
        results.append({"band": band, "window": entry["window"], "dtype": str(raw.dtype), "values_sha256": hashlib.sha256(np.ascontiguousarray(delivered).tobytes()).hexdigest(), "raw_values_equal_bits": True, "source_valid_equal": True, "aoi_mask_equal": True, "quality_masks_equal": True, "coordinates_equal": True, "time_bounds_equal_provider_endpoints_ns": True, "observed_source_classes": observed, "independent_source_io": source_io})
    stream_log = invoke(binary, netrc_path, request, output, label, stream=True)
    rows = [json.loads(line) for line in stream_log.read_text().splitlines() if line.strip()]
    require(rows and rows[-1].get("event") == "complete" and rows[-1].get("source_versions_validated") is True, "stream did not terminate with source-version validation")
    covered = {row["asset_id"]: np.zeros((row["window"]["height"], row["window"]["width"]), dtype=bool) for row in manifest["outputs"]}
    by_asset = {row["asset_id"]: row for row in manifest["outputs"]}
    chunks = 0
    for row in rows:
        if row.get("event") != "chunk":
            continue
        chunks += 1
        entry = by_asset[row["asset_id"]]
        whole, part = entry["window"], row["window"]
        y0, x0 = part["y"] - whole["y"], part["x"] - whole["x"]
        y1, x1 = y0 + part["height"], x0 + part["width"]
        dtype = np.dtype(row["dtype"]).newbyteorder("<")
        values = np.frombuffer(base64.b64decode(row["values_le_base64"]), dtype=dtype).reshape(part["height"], part["width"])
        group = root[entry["group"]]
        np.testing.assert_array_equal(values, group["values"][0, y0:y1, x0:x1])
        stream_aoi = np.frombuffer(base64.b64decode(row["aoi_mask_base64"]), dtype="u1").reshape(values.shape)
        stream_valid = np.frombuffer(base64.b64decode(row["source_valid_mask_base64"]), dtype="u1").reshape(values.shape)
        durable_aoi = group["aoi_mask"][0, y0:y1, x0:x1]
        durable_valid = group["valid"][0, y0:y1, x0:x1]
        np.testing.assert_array_equal(stream_aoi, durable_aoi)
        np.testing.assert_array_equal((stream_valid.astype(bool) & stream_aoi.astype(bool)).astype("u1"), durable_valid)
        for policy, result in row.get("quality", {}).items():
            expected = group["quality_" + policy][0, y0:y1, x0:x1]
            actual = np.frombuffer(base64.b64decode(result["mask_base64"]), dtype="u1").reshape(values.shape)
            np.testing.assert_array_equal(actual, expected)
        covered[row["asset_id"]][y0:y1, x0:x1] = True
    require(chunks > 0 and all(mask.all() for mask in covered.values()), "stream omitted durable output cells")
    return {"label": label, "output": str(output.relative_to(ROOT)), "status": "passed", "layers": results, "stream": {"chunks": chunks, "raw_bits_and_masks_match_durable_output": True, "completion_versions_validated": True}}


def confirmed_aoi(request, headers, credentials):
    """A bounded (<8 MiB/object) status scan; returns None if no confirmation occurs."""
    for band in ("VEG-DIST-STATUS", "GEN-DIST-STATUS"):
        asset, header = request["observations"][0]["assets"][band], headers[band]
        require(header["size_bytes"] <= MAX_SOURCE_BYTES, f"{band} cannot be safely scanned under cap")
        remote = RangeFile(asset, header, credentials)
        try:
            body = remote._range(0, remote.size - 1)
            with rasterio.io.MemoryFile(body) as memory, memory.open() as ds:
                data = ds.read(1)
                hit = np.argwhere(np.isin(data, [3, 6, 7, 8]))
                if hit.size == 0:
                    continue
                y, x = map(int, hit[0])
                half = 4
                x0, y0 = max(0, x-half), max(0, y-half)
                x1, y1 = min(ds.width, x+half+1), min(ds.height, y+half+1)
                xs = [ds.transform.c + ds.transform.a*x0, ds.transform.c + ds.transform.a*x1]
                ys = [ds.transform.f + ds.transform.e*y0, ds.transform.f + ds.transform.e*y1]
                lon, lat = transform(ds.crs, "EPSG:4326", [xs[0], xs[1], xs[0], xs[1]], [ys[0], ys[0], ys[1], ys[1]])
                return {"band": band, "class": int(data[y, x]), "bbox": [min(lon), min(lat), max(lon), max(lat)], "scan_bytes": remote.bytes, "scan_ranges": len(remote.events)}
        finally:
            remote.close()
    return None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--netrc", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--replay-output", type=Path, help="verify an already-complete fresh output without fetching it again")
    parser.add_argument("--capture", action="store_true")
    parser.add_argument("--qualify", action="store_true")
    parser.add_argument("--no-confirmed-second-aoi", action="store_true")
    args = parser.parse_args()
    require(args.capture or args.qualify, "choose --capture and/or --qualify")
    require(args.binary.is_file(), "--binary must be an executable file")
    credentials = private_credentials(args.netrc)
    request = json.loads(EXAMPLE.read_text())
    # Twenty-one authenticated COG transports reserve independently.  Keep
    # this qualification-only override out of the frozen public example.
    request.setdefault("buffer_bytes", 128 * 1024 * 1024)
    request.setdefault("chunk_edge", 64)
    headers = json.loads(SOURCE_RECEIPT.read_text())["sources"]
    require(len(request["bands"]) == len(headers) == 21 and set(request["bands"]) == set(headers), "example/header receipt must bind exactly 21 layers")
    for band in request["bands"]:
        require(request["observations"][0]["assets"][band]["etag"] == headers[band]["etag"], f"{band}: request pin differs from source receipt")
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    require(not (args.output and args.replay_output), "--output and --replay-output are mutually exclusive")
    output = args.replay_output or args.output or WORK / f"qualified-all21-{stamp}.zarr"
    report = {"schema": "tile-ripper-opera-dist-ann-qualification-v2", "captured_utc": datetime.now(timezone.utc).isoformat(), "product": request["product"], "granule": request["observations"][0]["id"], "status": "incomplete", "scope": "21 exact-version authenticated annual COGs; independent rasterio/tifffile source windows; no credentials, cookies, or signed resource URLs retained", "source_header_receipt": str(SOURCE_RECEIPT.relative_to(ROOT)), "caps": {"independent_source_bytes_per_tiff": MAX_SOURCE_BYTES, "independent_source_range_bytes": MAX_RANGE}, "redirect_origin_allowlist": [ARCHIVE, URS, RESOURCE], "credentials_sent_only_to": URS, "provider_temporal_extent": request["observations"][0]["properties"]["temporal_extent"], "binary": {"path": str(args.binary.relative_to(ROOT)), "sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest()}, "cases": []}
    try:
        if args.qualify:
            first = inspect_case(args.binary, args.netrc, request, output, "all21", credentials, headers, replay=args.replay_output is not None)
            report["cases"].append(first)
            statuses = [x["observed_source_classes"] for x in first["layers"] if x["band"].endswith("DIST-STATUS")]
            confirmed = any(any(int(code) in {3, 6, 7, 8} and count for code, count in classes.items()) for classes in statuses)
            rejected = any(classes.get("0", 0) for classes in statuses)
            if confirmed and rejected:
                report["confirmed_status_scan"] = {"status": "not_needed; broad first AOI independently exercised confirmed and rejected status classes", "observed_status_classes": statuses}
            if not args.no_confirmed_second_aoi and args.replay_output is None:
                if confirmed and rejected:
                    pass
                else:
                    hit = confirmed_aoi(request, headers, credentials)
                    report["confirmed_status_scan"] = hit or {"status": "not_found_under_bounded_status_scan"}
                if not (confirmed and rejected) and hit:
                    second = json.loads(json.dumps(request))
                    second["aoi"]["bbox"] = hit["bbox"]
                    second_output = output.with_name(output.stem + "-confirmed.zarr")
                    report["cases"].append(inspect_case(args.binary, args.netrc, second, second_output, "confirmed-all21", credentials, headers))
            report["status"] = "passed"
        else:
            report["status"] = "source_pins_checked_not_qualified"
    except Exception as error:
        report["failure"] = str(error)
        dump(DOC_RECEIPT, clean(report))
        raise
    dump(DOC_RECEIPT, clean(report))
    print(json.dumps(clean(report), indent=2, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except QualificationError as error:
        raise SystemExit(f"qualification failed: {error}")
