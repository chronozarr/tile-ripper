#!/usr/bin/env python3
"""Validate one pinned current ASF OPERA RTC-S1 acquisition independently.

The harness imports no Tile-Ripper modules. It resolves temporary provider URLs
only in memory, confines Basic authorization to the exact URS origin, and
persists only canonical unsigned URLs, strong ETags, and sanitized origins.

Examples:

  python bench/wave1_rtc_auth.py --netrc PRIVATE.netrc \
    --binary target/release/tile-ripper --work NEW_DIR --report receipt.json

  python bench/wave1_rtc_auth.py --netrc PRIVATE.netrc \
    --output existing.zarr --report receipt.json
"""

from __future__ import annotations

import argparse
import hashlib
import json
import netrc
import os
import re
import stat
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urljoin, urlparse

import numpy as np
import rasterio
import requests
import xarray as xr
import zarr
from rasterio.transform import xy
from rasterio.warp import transform as warp_transform
from rasterio.windows import Window


ARCHIVE = "https://cumulus.asf.earthdatacloud.nasa.gov"
LOGIN = "https://urs.earthdata.nasa.gov"
RESOURCE = "https://d1bctmng8vmwga.cloudfront.net"
PREFIX = "/OPERA/OPERA_L2_RTC-S1/"
HEAD_GATE = "/head-b339e3a40716b5999fea2aad1aee9f03"
GET_GATE = "/s3-b339e3a40716b5999fea2aad1aee9f03"
BUCKET_PATH = "/asf-cumulus-prod-opera-products.s3.us-west-2.amazonaws.com/"
REDIRECTS = {301, 302, 303, 307, 308}
REPO = Path(__file__).resolve().parents[1]

GRANULE = "OPERA_L2_RTC-S1_T117-249581-IW3_20261023T171312Z_20261023T230650Z_S1C_30_v1.0"
DATETIME = "2026-10-23T17:13:12Z"
CMR_CONCEPT = "G4386083295-ASF"
COLLECTION_CONCEPT = "C2777436413-ASF"
# A narrow window across the observed RTC validity edge. It contains finite
# backscatter and explicit source-missing pixels without approaching a TIFF
# storage boundary or expanding the acquisition materially.
BBOX = [4.639, 71.0923, 4.6477, 71.0947]
PINS = {
    "VV": ('"2a3f244bbfc8cc69269e131eaf4731d3-1"', 6_335_822),
    "VH": ('"acd8dfa800478954f49a8466356bcd54-1"', 6_485_899),
    "mask": ('"df499b7a7ea29cf4d980d6cb980529a0-1"', 49_713),
}


class ValidationError(Exception):
    """A sanitized failure that contains no credential or temporary URL."""


def require(condition, message):
    if not condition:
        raise ValidationError(message)


def canonical(layer):
    return f"{ARCHIVE}{PREFIX}{GRANULE}/{GRANULE}_{layer}.tif"


def origin(url):
    value = urlparse(url)
    return f"{value.scheme}://{value.netloc}"


def private_netrc(value):
    path = Path(value).expanduser()
    if path.is_symlink() or not path.is_file():
        raise ValidationError("--netrc must name an explicit regular file")
    if os.name == "posix" and stat.S_IMODE(path.stat().st_mode) & 0o077:
        raise ValidationError("--netrc must not be accessible by group or other users")
    auth = netrc.netrc(path).authenticators("urs.earthdata.nasa.gov")
    if not auth or not auth[0] or not auth[2]:
        raise ValidationError("--netrc lacks explicit URS credentials")
    return path.resolve(), (auth[0], auth[2])


def session():
    value = requests.Session()
    value.trust_env = False
    value.headers.update(
        {
            "Accept-Encoding": "identity",
            "User-Agent": "tile-ripper-independent-rtc-validation/1",
        }
    )
    return value


def expected_resource_path(canonical_url, method):
    path = urlparse(canonical_url).path
    require(path.startswith("/OPERA/"), "canonical RTC path changed")
    key = path[len("/OPERA/") :]
    return (HEAD_GATE if method == "HEAD" else GET_GATE) + BUCKET_PATH + key


def allowed_transition(current, target, canonical_url, method):
    a, b = origin(current), origin(target)
    target_path = urlparse(target).path
    resource_path = expected_resource_path(canonical_url, method)
    return (
        (a == ARCHIVE and b == LOGIN and target_path == "/oauth/authorize")
        or (a == LOGIN and b == ARCHIVE and target_path == "/login")
        or (a == ARCHIVE and b == ARCHIVE and target_path == urlparse(canonical_url).path)
        or (a == ARCHIVE and b == RESOURCE and target_path == resource_path)
    )


def resolve(canonical_url, method, etag, credentials):
    parsed = urlparse(canonical_url)
    require(origin(canonical_url) == ARCHIVE, "canonical RTC origin changed")
    require(not parsed.query and not parsed.fragment, "canonical RTC URL is signed")
    require(parsed.path.startswith(PREFIX), "canonical RTC release path changed")
    require(parsed.path.endswith(("_VV.tif", "_VH.tif", "_mask.tif")), "unsupported RTC layer")
    sessions = {ARCHIVE: session(), LOGIN: session(), RESOURCE: session()}
    current = canonical_url
    transitions = []
    for _ in range(8):
        current_origin = origin(current)
        require(current_origin in sessions, "authenticated redirect escaped allowed origins")
        current_path = urlparse(current).path
        if current_origin == RESOURCE:
            require(
                current_path == expected_resource_path(canonical_url, method),
                "authenticated resource path changed",
            )
        object_request = current == canonical_url or current_origin == RESOURCE
        headers = {"If-Match": etag} if object_request else {}
        if method == "GET" and object_request:
            headers["Range"] = "bytes=0-0"
        response = sessions[current_origin].request(
            method,
            current,
            headers=headers,
            auth=credentials if current_origin == LOGIN else None,
            allow_redirects=False,
            stream=True,
            timeout=(10, 30),
        )
        if response.status_code in REDIRECTS:
            target = urljoin(current, response.headers["Location"])
            require(
                allowed_transition(current, target, canonical_url, method),
                "authenticated redirect transition changed",
            )
            transitions.append((current_origin, origin(target), response.status_code))
            response.close()
            current = target
            continue
        expected_status = 200 if method == "HEAD" else 206
        require(current_origin == RESOURCE, "authenticated route did not reach resource")
        require(response.status_code == expected_status, "authenticated object request failed")
        require(response.headers.get("ETag") == etag, "provider ETag changed")
        if method == "HEAD":
            size = int(response.headers.get("Content-Length", "0"))
        else:
            match = re.fullmatch(r"bytes 0-0/(\d+)", response.headers.get("Content-Range", ""))
            require(match is not None, "provider range framing changed")
            size = int(match.group(1))
            require(len(response.content) == 1, "provider range body changed")
        response.close()
        require(size > 0, "provider source size is invalid")
        return current, transitions, size
    raise ValidationError("authenticated redirect hop budget exhausted")


def request_document():
    assets = {
        layer: {"href": canonical(layer), "metadata": {}}
        for layer in ("VV", "VH", "mask")
    }
    return {
        "product": "opera-rtc-s1",
        "aoi": {"kind": "bounds", "crs": "EPSG:4326", "bbox": BBOX},
        "start": DATETIME,
        "end": DATETIME,
        "bands": ["VV", "VH", "mask"],
        "quality_policies": ["opera_rtc_finite", "opera_rtc_mask_unaffected"],
        "observations": [
            {
                "id": GRANULE,
                "datetime": DATETIME,
                "collection": "OPERA_L2_RTC-S1_V1",
                "platform": "SENTINEL_1C",
                "processing": "RTC-S1",
                "properties": {
                    "cmr_concept_id": CMR_CONCEPT,
                    "collection_concept_id": COLLECTION_CONCEPT,
                },
                "assets": assets,
            }
        ],
    }


def file_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def array_sha256(array):
    value = np.ascontiguousarray(array)
    return hashlib.sha256(value.view(np.uint8)).hexdigest()


def iso_ns(value):
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(timezone.utc)
    delta = parsed - datetime(1970, 1, 1, tzinfo=timezone.utc)
    return delta.days * 86_400_000_000_000 + delta.seconds * 1_000_000_000 + delta.microseconds * 1_000


def run_fetch(binary, work, netrc_path):
    binary = binary.expanduser().resolve()
    require(binary.is_file() and os.access(binary, os.X_OK), "--binary must name an executable")
    work = work.expanduser().resolve()
    work.mkdir(parents=True, exist_ok=True)
    output = work / "rtc-current.zarr"
    request_path = work / "rtc-current-request.json"
    require(not output.exists() and not output.is_symlink(), "--work output already exists")
    require(not request_path.exists() and not request_path.is_symlink(), "--work request already exists")
    request_path.write_text(json.dumps(request_document(), indent=2) + "\n")
    completed = subprocess.run(
        [
            str(binary),
            "fetch",
            str(request_path),
            "--auth",
            "earthdata",
            "--netrc",
            str(netrc_path),
            "--gdal-free",
            "--output",
            str(output),
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        raise ValidationError("native RTC fetch failed")
    try:
        result = json.loads(completed.stdout)
    except Exception:
        raise ValidationError("native RTC fetch returned invalid JSON") from None
    require(result.get("status") == "complete", "native RTC fetch was incomplete")
    return output, {"mode": "binary", "binary_sha256": file_sha256(binary)}


def compact_transport(observed):
    sessions = observed["scheduler"]["native_metadata"]["authenticated_transport"]
    result = {}
    for asset_id, stats in sessions.items():
        result[asset_id] = {
            key: stats[key]
            for key in (
                "wire_head",
                "wire_get",
                "redirects",
                "login_requests",
                "refreshes",
                "route_cache_hits",
                "response_body_bytes",
                "redirect_body_bytes",
                "peak_in_flight",
                "statuses",
                "origins",
            )
        }
    return result


def validate(output, credentials):
    manifest_path = output / "manifest.json"
    require(manifest_path.is_file(), "output lacks manifest.json")
    manifest = json.loads(manifest_path.read_text())
    require(manifest.get("status") == "complete", "native output is incomplete")
    groups = {entry["asset_id"]: entry["group"] for entry in manifest["outputs"]}
    sources = {entry["asset_id"]: entry for entry in manifest["sources"]}
    expected_ids = {"p000_VV", "p000_VH", "p000_mask"}
    require(set(groups) == expected_ids and set(sources) == expected_ids, "RTC output groups changed")
    results = []
    common_grid = None
    for layer in ("VV", "VH", "mask"):
        asset_id = f"p000_{layer}"
        source = sources[asset_id]
        expected_etag, expected_size = PINS[layer]
        require(source["reference"] == canonical(layer), "manifest canonical URL changed")
        require(source["object_version"] == expected_etag, "manifest provider ETag changed")
        require(source["item_id"] == GRANULE, "manifest granule ID changed")
        require(source["product"] == "opera-rtc-s1", "manifest product changed")
        require(source["datetime"] == DATETIME, "manifest acquisition time changed")
        band = source["product_band"]
        require(band["name"] == layer, "manifest product band changed")
        require(band["scale"] == 1.0 and band["offset"] == 0.0, "RTC calibration changed")
        require(band["unit"] == "1", "RTC backscatter/class unit must be dimensionless")
        signed, transitions, size = resolve(canonical(layer), "GET", expected_etag, credentials)
        require(size == expected_size, "provider source size changed")
        _, head_transitions, head_size = resolve(canonical(layer), "HEAD", expected_etag, credentials)
        require(head_size == expected_size, "provider HEAD size changed")

        group = groups[asset_id]
        ds = xr.open_zarr(output, group=group, consolidated=False, decode_times=False, mask_and_scale=False)
        attrs = dict(zarr.open_group(output, mode="r")[group].attrs)
        value_attrs = dict(ds["values"].attrs)
        window_value = attrs["source_window"]
        window = Window(window_value["x"], window_value["y"], window_value["width"], window_value["height"])
        try:
            with rasterio.Env(
                GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
                CPL_VSIL_CURL_USE_HEAD="NO",
                CPL_CURL_VERBOSE="NO",
                CPL_LOG="/dev/null",
                GDAL_HTTP_NETRC="NO",
                GDAL_HTTP_HEADERS=f"If-Match: {expected_etag}",
            ):
                with rasterio.open(signed) as src:
                    raw = src.read(1, window=window)
                    source_transform = src.window_transform(window)
                    source_dtype = np.dtype(src.dtypes[0])
                    source_nodata = src.nodata
                    source_crs = src.crs.to_string()
                    source_scale = src.scales[0]
                    source_offset = src.offsets[0]
                    source_shape = list(src.shape)
                    source_block = list(src.block_shapes[0])
        except Exception:
            raise ValidationError(f"mature source read failed for RTC {layer}") from None

        rows, cols = np.indices(raw.shape)
        xs, ys = xy(source_transform, rows, cols, offset="center")
        lons, lats = warp_transform(source_crs, "EPSG:4326", np.asarray(xs).ravel(), np.asarray(ys).ravel())
        lons = np.asarray(lons).reshape(raw.shape)
        lats = np.asarray(lats).reshape(raw.shape)
        aoi_expected = (lons >= BBOX[0]) & (lons <= BBOX[2]) & (lats >= BBOX[1]) & (lats <= BBOX[3])
        if source_dtype == np.dtype("float32"):
            source_valid = np.isfinite(raw)
            expected_values = raw
            require(source_valid.any() and (~source_valid).any(), "RTC validation window no longer crosses the validity edge")
        else:
            source_valid = raw != np.asarray(source_nodata, dtype=source_dtype)
            expected_values = raw.copy()
            expected_values[~aoi_expected] = np.asarray(source_nodata, dtype=source_dtype)
            require(source_valid.any() and (~source_valid).any(), "RTC mask window no longer crosses the validity edge")
        valid_expected = aoi_expected & source_valid
        values = ds["values"].values[0]
        aoi = ds["aoi_mask"].values[0].astype(bool)
        valid = ds["valid"].values[0].astype(bool)
        require(np.array_equal(aoi, aoi_expected), "RTC AOI mask mismatch")
        require(np.array_equal(valid, valid_expected), "RTC validity mismatch")
        if source_dtype == np.dtype("float32"):
            require(np.array_equal(values.view(np.uint32), expected_values.view(np.uint32)), "RTC Float32 bits changed")
        else:
            require(np.array_equal(values, expected_values), "RTC mask values changed")
        require(np.dtype(values.dtype) == source_dtype, "RTC stored dtype changed")
        require(attrs["crs"] == source_crs, "RTC stored CRS changed")
        require(value_attrs.get("product_band") == band, "RTC product calibration metadata changed")
        require(int(ds["time"].values[0]) == iso_ns(DATETIME), "RTC timestamp changed")
        require((1.0 if attrs["source_scale"] is None else attrs["source_scale"]) == source_scale, "RTC scale changed")
        require((0.0 if attrs["source_offset"] is None else attrs["source_offset"]) == source_offset, "RTC offset changed")
        expected_x = source_transform.c + source_transform.a * (np.arange(raw.shape[1]) + 0.5)
        expected_y = source_transform.f + source_transform.e * (np.arange(raw.shape[0]) + 0.5)
        require(np.array_equal(ds["x"].values, expected_x), "RTC x coordinates changed")
        require(np.array_equal(ds["y"].values, expected_y), "RTC y coordinates changed")
        physical = source["object_identities"]["native_cog"]
        expected_token = "nan" if source_dtype == np.dtype("float32") else "255"
        require(physical["gdal_nodata_text"].lower() == expected_token, "RTC physical nodata token changed")
        require(physical["normalized_crs"] == source_crs, "RTC normalized CRS changed")
        if source_dtype == np.dtype("float32"):
            require(attrs["source_nodata"] is None, "RTC Float32 normalized nodata changed")
        else:
            require(attrs["source_nodata"] == 255, "RTC mask normalized nodata changed")

        if common_grid is None:
            common_grid = (source_crs, tuple(source_transform), raw.shape)
        else:
            require(common_grid == (source_crs, tuple(source_transform), raw.shape), "RTC layers are not co-registered")
        quality_name = "quality_opera_rtc_finite" if layer != "mask" else "quality_opera_rtc_mask_unaffected"
        quality = ds[quality_name].values[0].astype(bool)
        quality_expected = valid_expected if layer != "mask" else valid_expected & (raw == 0)
        require(np.array_equal(quality, quality_expected), "RTC derived quality mismatch")
        results.append(
            {
                "asset_id": asset_id,
                "canonical_url": canonical(layer),
                "etag": expected_etag,
                "source_size_bytes": size,
                "redirect_hops": [
                    {"from_origin": a, "to_origin": b, "status": status}
                    for a, b, status in transitions
                ],
                "head_redirect_hops": [
                    {"from_origin": a, "to_origin": b, "status": status}
                    for a, b, status in head_transitions
                ],
                "source_shape": source_shape,
                "source_block_shape": source_block,
                "window": window_value,
                "shape": list(raw.shape),
                "crs": source_crs,
                "transform": list(source_transform)[:6],
                "dtype": source_dtype.name,
                "nodata": "NaN" if source_dtype == np.dtype("float32") else source_nodata,
                "scale": source_scale,
                "offset": source_offset,
                "source_values_sha256": array_sha256(raw),
                "stored_values_sha256": array_sha256(values),
                "stored_values_match_expected_aoi_policy": True,
                "stored_values_equal_source_bits": bool(
                    np.array_equal(values.view(np.uint32), raw.view(np.uint32))
                    if source_dtype == np.dtype("float32")
                    else np.array_equal(values, raw)
                ),
                "coordinates_match_rasterio_pixel_centers": True,
                "aoi_mask_matches_independent_wgs84_center_test": True,
                "aoi_pixels": int(aoi_expected.sum()),
                "valid_pixels": int(valid_expected.sum()),
                "derived_quality_matches": True,
                "physical_nodata_token": physical["gdal_nodata_text"],
            }
        )
    observed = manifest["observed"]
    scheduler = observed["scheduler"]
    return {
        "schema_version": 1,
        "document_type": "bounded_current_opera_rtc_authenticated_validation",
        "status": "passed",
        "validated_at_utc": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "scope": "One explicitly pinned current ASF OPERA RTC-S1 V1 granule; VV, VH, and mask over one narrow AOI. This does not claim archive-wide qualification.",
        "cmr": {
            "collection_concept_id": COLLECTION_CONCEPT,
            "granule_concept_id": CMR_CONCEPT,
            "granule_id": GRANULE,
            "acquisition_datetime": DATETIME,
        },
        "reference": {
            "reader": f"rasterio {rasterio.__version__}",
            "zarr_reader": f"xarray {xr.__version__} / zarr-python {zarr.__version__}",
            "authentication": "ephemeral signed object URLs resolved in memory through exact ASF, URS, ASF login, exact canonical, and fixed CloudFront paths",
            "credential_confinement": "ambient netrc disabled; Basic authorization supplied only to the exact URS origin; no credentials or signed URLs persisted",
            "conditional_identity": "If-Match used for resolver probes and mature-reader requests; manifest stores strong provider ETags and canonical unsigned URLs",
        },
        "request": request_document(),
        "manifest_sha256": hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
        "assets": results,
        "co_registration_verified": True,
        "linear_gamma0_preserved_without_db_conversion": True,
        "boundary_selection": {
            "final_aoi": "Rounded WGS84 bounds cross the observed source-validity edge without placing any source center exactly on an AOI edge.",
            "diagnostic": "An earlier deliberately exact-center bound differed from an inclusive longitude/latitude predicate at two edge centers because native masking follows GDAL polygon scanline edge ownership. The final reference retains both valid and source-missing pixels while avoiding that boundary tie.",
        },
        "observed": {
            "chunks": observed["chunks"],
            "decoded_bytes": observed["decoded_bytes"],
            "source_data_requests": observed["source_data_requests"],
            "source_data_bytes": observed["source_data_bytes"],
            "scheduler_buffer_bytes": scheduler["buffer_bytes"],
            "scheduler_reserved_bytes_peak": scheduler["reserved_bytes_peak"],
            "prepared_native_metadata_bytes": scheduler["prepared_native_metadata_bytes"],
            "authenticated_transport_reserved_bytes": scheduler["authenticated_transport_reserved_bytes"],
            "output_buffered_bytes_peak": observed["output_io"]["buffered_bytes_peak"],
            "transport": compact_transport(observed),
        },
    }


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--netrc", required=True, metavar="PRIVATE")
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--work", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    binary_mode = args.binary is not None or args.work is not None
    existing_mode = args.output is not None
    if binary_mode == existing_mode:
        parser.error("choose --binary with --work, or --output")
    if binary_mode and (args.binary is None or args.work is None):
        parser.error("--binary and --work are required together")
    return args


def main():
    args = parse_args()
    netrc_path, credentials = private_netrc(args.netrc)
    if args.binary is not None:
        output, execution = run_fetch(args.binary, args.work, netrc_path)
    else:
        output = args.output.expanduser().resolve()
        execution = {"mode": "existing_output"}
    receipt = validate(output, credentials)
    receipt["execution"] = execution
    encoded = json.dumps(receipt, indent=2, sort_keys=True) + "\n"
    if args.report is None:
        sys.stdout.write(encoded)
    else:
        report = args.report.expanduser().resolve()
        require(not report.exists() and not report.is_symlink(), "--report destination already exists")
        report.parent.mkdir(parents=True, exist_ok=True)
        report.write_text(encoded)


if __name__ == "__main__":
    try:
        main()
    except ValidationError as error:
        print(f"RTC validation failed: {error}", file=sys.stderr)
        raise SystemExit(1)
