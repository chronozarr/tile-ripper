#!/usr/bin/env python3
"""Independently validate pinned authenticated HLS native COG outputs.

The harness imports no Tile-Ripper modules. It can run an already-built
Tile-Ripper binary to produce two new Zarr outputs, or validate an explicit
existing pair. It resolves each temporary provider URL only in memory and
confines Basic authorization to the exact URS origin. Reports retain only the
unsigned canonical URLs, provider ETags, and sanitized redirect origins.

Examples:

  python bench/wave1_hls_auth.py \
    --netrc PRIVATE.netrc --binary target/release/tile-ripper --work NEW_DIR \
    --report validation.json

  python bench/wave1_hls_auth.py \
    --netrc PRIVATE.netrc --l30-output l30.zarr --s30-output s30.zarr
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
import xml.etree.ElementTree as ET
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


ARCHIVE = "https://data.lpdaac.earthdatacloud.nasa.gov"
LOGIN = "https://urs.earthdata.nasa.gov"
RESOURCE = "https://d1nklfio7vscoe.cloudfront.net"
PREFIX = "/lp-prod-protected/"
GET_GATE = "/s3-2d2df3a34830d5223d1e9547cd713408"
REDIRECTS = {301, 302, 303, 307, 308}
REPO = Path(__file__).resolve().parents[1]

CASES = (
    {
        "label": "HLS L30 V2 B04 and Fmask",
        "product": "hls-l30",
        "request": REPO / "docs/products/hls-l30-authenticated-request.json",
        "granule_id": "HLS.L30.T17SPU.2026291T154740.v2.0",
        "concept_id": "G4382581583-LPCLOUD",
        "datetime": "2026-10-18T15:47:40.202Z",
        "bbox": [-78.901, 34.999, -78.9, 35.0],
        "assets": {
            "p000_B04": {
                "canonical_url": "https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected/HLSL30.020/HLS.L30.T17SPU.2026291T154740.v2.0/HLS.L30.T17SPU.2026291T154740.v2.0.B04.tif",
                "etag": '"c1eea635b7169a322f1194219709d2c1-1"',
                "size_bytes": 9_495_244,
            },
            "p000_Fmask": {
                "canonical_url": "https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected/HLSL30.020/HLS.L30.T17SPU.2026291T154740.v2.0/HLS.L30.T17SPU.2026291T154740.v2.0.Fmask.tif",
                "etag": '"8287e3130a18bd2150f3f5151fda7608-1"',
                "size_bytes": 376_531,
            },
        },
    },
    {
        "label": "HLS S30 V2 B04 and Fmask",
        "product": "hls-s30",
        "request": REPO / "docs/products/hls-s30-authenticated-request.json",
        "granule_id": "HLS.S30.T59UNU.2026295T235129.v2.0",
        "concept_id": "G4386229287-LPCLOUD",
        "datetime": "2026-10-22T23:51:43.692Z",
        "bbox": [171.5, 52.8, 171.501, 52.801],
        "assets": {
            "p000_B04": {
                "canonical_url": "https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected/HLSS30.020/HLS.S30.T59UNU.2026295T235129.v2.0/HLS.S30.T59UNU.2026295T235129.v2.0.B04.tif",
                "etag": '"ffeba64feaf023b15e8944517bfde064-1"',
                "size_bytes": 24_793_915,
            },
            "p000_Fmask": {
                "canonical_url": "https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected/HLSS30.020/HLS.S30.T59UNU.2026295T235129.v2.0/HLS.S30.T59UNU.2026295T235129.v2.0.Fmask.tif",
                "etag": '"488b863361e4da26908ebc4207d7f6df-1"',
                "size_bytes": 1_546_961,
            },
        },
    },
)


class ValidationError(Exception):
    """A sanitized validation failure that contains no temporary URL."""


def require(condition, message):
    if not condition:
        raise ValidationError(message)


def origin(url):
    p = urlparse(url)
    return f"{p.scheme}://{p.netloc}"


def make_session():
    session = requests.Session()
    session.trust_env = False
    session.headers.update(
        {
            "Accept-Encoding": "identity",
            "User-Agent": "tile-ripper-independent-validation/1",
        }
    )
    return session


def resolve_signed_get(canonical, etag, netrc_path):
    parsed = urlparse(canonical)
    require(origin(canonical) == ARCHIVE, "canonical HLS origin changed")
    require(not parsed.query and not parsed.fragment, "canonical HLS URL is signed")
    require(
        parsed.path.startswith((PREFIX + "HLSL30.020/", PREFIX + "HLSS30.020/")),
        "canonical HLS release path changed",
    )
    require(parsed.path.endswith(".tif"), "canonical HLS object is not a TIFF")
    key = parsed.path[len(PREFIX):]
    expected_resource_path = GET_GATE + "/lp-prod-protected.s3.us-west-2.amazonaws.com/" + key

    auth = netrc.netrc(netrc_path).authenticators("urs.earthdata.nasa.gov")
    require(auth and auth[0] and auth[2], "explicit URS credentials are missing")
    login_auth = (auth[0], auth[2])
    sessions = {ARCHIVE: make_session(), LOGIN: make_session(), RESOURCE: make_session()}
    current = canonical
    transitions = []
    for _ in range(8):
        current_origin = origin(current)
        parsed_current = urlparse(current)
        require(current_origin in sessions, "authenticated redirect escaped allowed origins")
        if current_origin == ARCHIVE:
            require(
                parsed_current.path in (parsed.path, "/login"),
                "authenticated redirect changed the archive path",
            )
        elif current_origin == LOGIN:
            require(
                parsed_current.path == "/oauth/authorize",
                "authenticated redirect changed the URS path",
            )
        else:
            require(
                parsed_current.path == expected_resource_path,
                "authenticated redirect changed the resource object",
            )

        object_request = current == canonical or current_origin == RESOURCE
        headers = {"Range": "bytes=0-0", "If-Match": etag} if object_request else {}
        response = sessions[current_origin].get(
            current,
            headers=headers,
            auth=login_auth if current_origin == LOGIN else None,
            allow_redirects=False,
            stream=True,
            timeout=(10, 30),
        )
        status = response.status_code
        if status in REDIRECTS:
            target = urljoin(current, response.headers["Location"])
            target_origin = origin(target)
            target_path = urlparse(target).path
            allowed = (
                (current_origin == ARCHIVE and target_origin == LOGIN and target_path == "/oauth/authorize")
                or (current_origin == LOGIN and target_origin == ARCHIVE and target_path == "/login")
                or (current_origin == ARCHIVE and target_origin == ARCHIVE and target_path == parsed.path)
                or (current_origin == ARCHIVE and target_origin == RESOURCE and target_path == expected_resource_path)
            )
            require(allowed, "authenticated redirect transition changed")
            transitions.append((current_origin, target_origin, status))
            response.close()
            current = target
            continue
        require(
            current_origin == RESOURCE and status == 206,
            "authenticated range probe did not resolve to the resource",
        )
        require(response.headers.get("ETag") == etag, "provider ETag changed")
        content_range = response.headers.get("Content-Range", "")
        match = re.fullmatch(r"bytes 0-0/(\d+)", content_range)
        require(match is not None, "provider range framing changed")
        source_size = int(match.group(1))
        require(source_size > 0, "provider source size is invalid")
        response.close()
        return current, transitions, source_size
    raise AssertionError("redirect hop budget exhausted")


def xml_items(text):
    return {node.attrib.get("name"): node.text for node in ET.fromstring(text).findall("Item")}


def iso_ns(value):
    match = re.fullmatch(r"(.+?)(?:\.(\d+))?(Z|[+-]\d\d:\d\d)", value)
    if match is None:
        raise ValidationError("invalid timestamp")
    base, fraction, zone = match.groups()
    fraction = ((fraction or "") + "000000")[:6]
    parsed = datetime.fromisoformat(
        f"{base}.{fraction}{'+00:00' if zone == 'Z' else zone}"
    )
    if parsed.tzinfo is None:
        raise ValidationError("timestamp lacks a timezone")
    delta = parsed.astimezone(timezone.utc) - datetime(1970, 1, 1, tzinfo=timezone.utc)
    return (
        delta.days * 86_400_000_000_000
        + delta.seconds * 1_000_000_000
        + delta.microseconds * 1_000
    )


def sha256_array(array):
    values = np.asarray(array)
    little = values.astype(values.dtype.newbyteorder("<"), copy=False)
    return hashlib.sha256(np.ascontiguousarray(little).tobytes()).hexdigest()


def compact_transport(observed):
    native = observed["transport"]["native_cog_authenticated"]
    keys = [
        "wire_head",
        "wire_get",
        "redirects",
        "login_requests",
        "refreshes",
        "route_cache_hits",
        "response_body_bytes",
        "redirect_body_bytes",
        "peak_in_flight",
    ]
    return {asset: {key: values[key] for key in keys} for asset, values in sorted(native.items())}


def load_pinned_request(case):
    request = json.loads(case["request"].read_text())
    observation = request["observations"][0]
    if (
        request["product"] != case["product"]
        or request["bands"] != ["B04", "Fmask"]
        or request["quality_policies"] != ["hls_fmask_clear"]
        or request["aoi"] != {
            "kind": "bounds",
            "crs": "EPSG:4326",
            "bbox": case["bbox"],
        }
        or request["start"] != case["datetime"]
        or request["end"] != case["datetime"]
        or observation["id"] != case["granule_id"]
        or observation["datetime"] != case["datetime"]
        or observation["properties"].get("cmr_concept_id") != case["concept_id"]
        or observation["assets"]["B04"]["href"]
        != case["assets"]["p000_B04"]["canonical_url"]
        or observation["assets"]["Fmask"]["href"]
        != case["assets"]["p000_Fmask"]["canonical_url"]
        or observation["assets"]["B04"].get("etag") is not None
        or observation["assets"]["Fmask"].get("etag") is not None
    ):
        raise ValidationError("pinned request semantics changed")
    return request


def validate_case(case, zarr_path, netrc_path):
    request = load_pinned_request(case)
    manifest_path = zarr_path / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    require(manifest["status"] == "complete", "native output is incomplete")
    require(
        manifest["processing"]
        == [
            "native stored typed values; calibration is metadata only",
            "one source asset/band and native grid per observation; chronological order",
            "no reprojection, resampling, mosaicking or implicit alignment",
            "valid and aoi_mask distinguish source nodata from excluded AOI centers",
        ],
        "native processing contract changed",
    )
    observation = request["observations"][0]
    bbox = request["aoi"]["bbox"]
    groups = {entry["asset_id"]: entry["group"] for entry in manifest["outputs"]}
    sources = {entry["asset_id"]: entry for entry in manifest["sources"]}
    require(set(groups) == set(case["assets"]), "native output groups changed")
    require(set(sources) == set(case["assets"]), "native output sources changed")
    results = []
    for asset_id in ("p000_B04", "p000_Fmask"):
        source = sources[asset_id]
        canonical = source["reference"]
        etag = source["object_version"]
        pinned = case["assets"][asset_id]
        require(canonical == pinned["canonical_url"], "manifest canonical URL changed")
        require(etag == pinned["etag"], "manifest provider ETag changed")
        require(source["item_id"] == case["granule_id"], "manifest granule ID changed")
        require(source["product"] == case["product"], "manifest product changed")
        require(source["product_version"] == "2.0", "manifest product version changed")
        signed_url, transitions, source_size = resolve_signed_get(
            canonical, etag, netrc_path
        )
        require(source_size == pinned["size_bytes"], "provider source size changed")
        group = groups[asset_id]
        ds_zarr = xr.open_zarr(
            zarr_path,
            group=group,
            consolidated=False,
            decode_times=False,
            mask_and_scale=False,
        )
        attrs = dict(zarr.open_group(zarr_path, mode="r")[group].attrs)
        window_dict = attrs["source_window"]
        window = Window(window_dict["x"], window_dict["y"], window_dict["width"], window_dict["height"])
        try:
            with rasterio.Env(
                GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
                CPL_VSIL_CURL_USE_HEAD="NO",
                CPL_CURL_VERBOSE="NO",
                CPL_LOG="/dev/null",
                GDAL_HTTP_NETRC="NO",
                GDAL_HTTP_HEADERS=f"If-Match: {etag}",
            ):
                with rasterio.open(signed_url) as src:
                    raw = src.read(1, window=window)
                    source_transform = src.window_transform(window)
                    source_dtype = np.dtype(src.dtypes[0])
                    source_nodata = src.nodata
                    source_crs = src.crs.to_string()
                    tags = src.tags()
                    source_scale = src.scales[0]
                    source_offset = src.offsets[0]
        except Exception:
            raise ValidationError(
                f"mature source read failed for {case['product']} {asset_id}"
            ) from None

        rows, cols = np.indices(raw.shape)
        xs, ys = xy(source_transform, rows, cols, offset="center")
        xs = np.asarray(xs, dtype=np.float64)
        ys = np.asarray(ys, dtype=np.float64)
        lons, lats = warp_transform(source_crs, "EPSG:4326", xs.ravel(), ys.ravel())
        lons = np.asarray(lons).reshape(raw.shape)
        lats = np.asarray(lats).reshape(raw.shape)
        independent_aoi = (
            (lons >= bbox[0]) & (lons <= bbox[2]) & (lats >= bbox[1]) & (lats <= bbox[3])
        )
        aoi = ds_zarr["aoi_mask"].values[0].astype(bool)
        valid = ds_zarr["valid"].values[0].astype(bool)
        values = ds_zarr["values"].values[0]
        independent_valid = independent_aoi & (raw != source_nodata)
        expected_values = raw.copy()
        expected_values[~independent_aoi] = np.asarray(source_nodata, dtype=source_dtype)

        require(np.array_equal(aoi, independent_aoi), "AOI mask mismatch")
        require(np.array_equal(valid, independent_valid), "validity mask mismatch")
        require(np.array_equal(values, expected_values), "stored source values mismatch")
        expected_x = source_transform.c + source_transform.a * (np.arange(raw.shape[1]) + 0.5)
        expected_y = source_transform.f + source_transform.e * (np.arange(raw.shape[0]) + 0.5)
        require(
            np.array_equal(ds_zarr["x"].values, expected_x), "x coordinate mismatch"
        )
        require(
            np.array_equal(ds_zarr["y"].values, expected_y), "y coordinate mismatch"
        )
        require(
            int(ds_zarr["time"].values[0]) == iso_ns(observation["datetime"]),
            "stored time mismatch",
        )
        require(attrs["datetime"] == observation["datetime"], "datetime metadata mismatch")
        require(np.dtype(values.dtype) == source_dtype, "stored dtype mismatch")
        require(attrs["crs"] == source_crs, "stored CRS mismatch")
        require(attrs["source_nodata"] == source_nodata, "stored nodata mismatch")
        require(
            (1.0 if attrs["source_scale"] is None else attrs["source_scale"])
            == source_scale,
            "stored scale mismatch",
        )
        require(
            (0.0 if attrs["source_offset"] is None else attrs["source_offset"])
            == source_offset,
            "stored offset mismatch",
        )

        physical = source["object_identities"]["native_cog"]
        retained_tags = xml_items(physical["gdal_metadata_xml"])
        for key in ("SENSING_TIME", "HLS_PROCESSING_TIME"):
            require(retained_tags[key] == tags[key], f"retained {key} mismatch")
        require(
            iso_ns(tags["SENSING_TIME"]) // 1_000_000
            == iso_ns(observation["datetime"]) // 1_000_000,
            "source sensing time mismatch",
        )
        require(
            datetime.fromisoformat(
                tags["HLS_PROCESSING_TIME"].replace("Z", "+00:00")
            ).tzinfo
            is not None,
            "source processing time lacks a timezone",
        )

        quality = None
        if asset_id == "p000_Fmask":
            quality_values = ds_zarr["quality_hls_fmask_clear"].values[0].astype(bool)
            independent_quality = independent_valid & ((raw.astype(np.uint8) & np.uint8(0x1F)) == 0)
            require(
                np.array_equal(quality_values, independent_quality),
                "derived HLS Fmask quality mismatch",
            )
            quality = {
                "policy": "hls_fmask_clear",
                "definition": "valid AOI sample with Fmask bits 0-4 all zero; water and aerosol do not reject",
                "accepted": int(independent_quality.sum()),
                "matches": True,
            }

        results.append(
            {
                "asset_id": asset_id,
                "canonical_url": canonical,
                "etag": etag,
                "source_size_bytes": source_size,
                "redirect_hops": [
                    {"from_origin": a, "to_origin": b, "status": status}
                    for a, b, status in transitions
                ],
                "window": window_dict,
                "shape": list(raw.shape),
                "crs": source_crs,
                "dtype": source_dtype.name,
                "dtype_bits": source_dtype.itemsize * 8,
                "nodata": source_nodata,
                "scale": source_scale,
                "offset": source_offset,
                "source_values_sha256": sha256_array(raw),
                "stored_values_sha256": sha256_array(values),
                "stored_values_match_source_after_aoi_fill": True,
                "coordinates_match_rasterio_pixel_centers": True,
                "aoi_mask_matches_independent_wgs84_center_test": True,
                "aoi_pixels": int(independent_aoi.sum()),
                "valid_matches_aoi_and_source_nodata": True,
                "valid_pixels": int(independent_valid.sum()),
                "time_matches_request_nanoseconds": True,
                "request_time": observation["datetime"],
                "source_sensing_time": tags["SENSING_TIME"],
                "source_processing_time": tags["HLS_PROCESSING_TIME"],
                "retained_processing_tags_match_rasterio": True,
                "quality": quality,
            }
        )

    observed = manifest["observed"]
    scheduler = observed["scheduler"]
    return {
        "label": case["label"],
        "product": request["product"],
        "granule_id": observation["id"],
        "cmr_concept_id": observation["properties"]["cmr_concept_id"],
        "request_file": case["request"].name,
        "manifest_status": manifest["status"],
        "manifest_sha256": hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
        "processing_contract_verified": True,
        "assets": results,
        "observed": {
            "chunks": observed["chunks"],
            "decoded_bytes": observed["decoded_bytes"],
            "non_nodata_pixels": observed["non_nodata_pixels"],
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


def file_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def private_netrc(value):
    path = Path(value).expanduser()
    if path.is_symlink() or not path.is_file():
        raise ValidationError("--netrc must name an explicit regular file")
    if os.name == "posix" and stat.S_IMODE(path.stat().st_mode) & 0o077:
        raise ValidationError("--netrc must not be accessible by group or other users")
    credentials = netrc.netrc(path).authenticators("urs.earthdata.nasa.gov")
    if not credentials or not credentials[0] or not credentials[2]:
        raise ValidationError("--netrc lacks explicit URS credentials")
    return path.resolve()


def run_fetch(binary, work, netrc_path):
    binary = binary.expanduser().resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValidationError("--binary must name an executable file")
    work = work.expanduser().resolve()
    work.mkdir(parents=True, exist_ok=True)
    outputs = {
        "hls-l30": work / "hls-l30.zarr",
        "hls-s30": work / "hls-s30.zarr",
    }
    for output in outputs.values():
        if output.exists() or output.is_symlink():
            raise ValidationError("--work already contains a named validation output")
    for case in CASES:
        load_pinned_request(case)
        completed = subprocess.run(
            [
                str(binary),
                "fetch",
                str(case["request"]),
                "--auth",
                "earthdata",
                "--netrc",
                str(netrc_path),
                "--gdal-free",
                "--output",
                str(outputs[case["product"]]),
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if completed.returncode != 0:
            raise ValidationError(f"native fetch failed for {case['product']}")
        try:
            result = json.loads(completed.stdout)
        except Exception:
            raise ValidationError(
                f"native fetch returned invalid JSON for {case['product']}"
            ) from None
        if result.get("status") != "complete":
            raise ValidationError(f"native fetch was incomplete for {case['product']}")
    return outputs, {"mode": "binary", "binary_sha256": file_sha256(binary)}


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--netrc",
        required=True,
        metavar="PRIVATE",
        help="explicit mode-0600 netrc file containing URS credentials",
    )
    parser.add_argument(
        "--binary",
        type=Path,
        help="already-built native-cog+authenticated-http Tile-Ripper binary",
    )
    parser.add_argument(
        "--work",
        type=Path,
        help="work directory for two new outputs; required with --binary",
    )
    parser.add_argument(
        "--l30-output",
        type=Path,
        help="existing completed L30 Zarr output",
    )
    parser.add_argument(
        "--s30-output",
        type=Path,
        help="existing completed S30 Zarr output",
    )
    parser.add_argument(
        "--report",
        type=Path,
        help="new sanitized JSON receipt; stdout when omitted",
    )
    args = parser.parse_args()
    binary_mode = args.binary is not None or args.work is not None
    existing_mode = args.l30_output is not None or args.s30_output is not None
    if binary_mode == existing_mode:
        parser.error(
            "choose --binary with --work, or --l30-output with --s30-output"
        )
    if binary_mode and (args.binary is None or args.work is None):
        parser.error("--binary and --work are required together")
    if existing_mode and (args.l30_output is None or args.s30_output is None):
        parser.error("--l30-output and --s30-output are required together")
    return args


def main():
    args = parse_args()
    report = None if args.report is None else args.report.expanduser().resolve()
    if report is not None and (report.exists() or report.is_symlink()):
        raise ValidationError("--report destination already exists")
    netrc_path = private_netrc(args.netrc)
    if args.binary is not None:
        outputs, execution = run_fetch(args.binary, args.work, netrc_path)
    else:
        outputs = {
            "hls-l30": args.l30_output.expanduser().resolve(),
            "hls-s30": args.s30_output.expanduser().resolve(),
        }
        if any(not (path / "manifest.json").is_file() for path in outputs.values()):
            raise ValidationError("existing output lacks a manifest.json")
        execution = {"mode": "existing_outputs"}

    cases = [
        validate_case(case, outputs[case["product"]], netrc_path) for case in CASES
    ]
    document = {
        "schema_version": 1,
        "document_type": "bounded_live_hls_authenticated_validation",
        "status": "passed",
        "validated_at_utc": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "scope": "Two explicitly pinned, narrow production HLS V2 examples; one L30 and one S30 granule, B04 and Fmask only. This evidence does not claim archive-wide qualification.",
        "reference": {
            "reader": f"rasterio {rasterio.__version__} / GDAL {rasterio.__gdal_version__}",
            "zarr_reader": f"xarray {xr.__version__} / zarr-python {zarr.__version__}",
            "authentication": "ephemeral signed GET URL resolved in memory through exact canonical, URS, canonical-login, canonical-object, and fixed CloudFront-object routes",
            "credential_confinement": "ambient netrc disabled; Basic authorization supplied only to the exact URS origin; exact-origin cookie sessions; no credentials or signed URLs persisted",
            "conditional_identity": "If-Match used for resolver probe and mature-reader requests; the harness pins provider strong ETags and Content-Range total sizes, while the manifest stores ETags and canonical unsigned URLs",
        },
        "execution": execution,
        "cases": cases,
    }
    serialized = json.dumps(document, indent=2) + "\n"
    if report is None:
        sys.stdout.write(serialized)
    else:
        report.write_text(serialized)
        print(
            json.dumps(
                {"status": document["status"], "cases": [c["product"] for c in cases]}
            )
        )


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception:
        # Request libraries and GDAL may include a temporary signed query in
        # exception text. Never echo exception details from this harness.
        print(
            "wave1_hls_auth: validation failed; temporary route details suppressed",
            file=sys.stderr,
        )
        raise SystemExit(1) from None
