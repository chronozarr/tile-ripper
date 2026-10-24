#!/usr/bin/env python3
"""Independently inspect the pinned current OPERA DSWx-HLS V1.1 COGs.

The harness imports no Tile-Ripper modules. It requires an explicit private
netrc, confines Basic authorization to the exact URS origin, resolves provider
URLs only in memory, and never reports or persists their query strings.

Example:

  python bench/wave1_dswx_auth.py \
    --netrc PRIVATE.netrc --binary target/release/tile-ripper \
    --work NEW_DIR --report dswx-auth-validation.json

  python bench/wave1_dswx_auth.py \
    --netrc PRIVATE.netrc --output EXISTING.zarr
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import netrc
import re
import stat
import subprocess
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urljoin, urlparse

import numpy as np
import rasterio
import requests
import tifffile
import xarray as xr
import zarr
from rasterio.transform import xy
from rasterio.warp import transform as warp_transform
from rasterio.windows import Window


ARCHIVE = "https://archive.podaac.earthdata.nasa.gov"
LOGIN = "https://urs.earthdata.nasa.gov"
RESOURCE = "https://deotb6e7tfubr.cloudfront.net"
PREFIX = "/podaac-ops-cumulus-protected/"
COLLECTION = "OPERA_L3_DSWX-HLS_V1"
GET_GATE = "/s3-edaf5da92e0ce48fb61175c28b67e95d"
REDIRECTS = {301, 302, 303, 307, 308}
GRANULE = "OPERA_L3_DSWx-HLS_T17SPU_20261018T154740Z_20261022T014315Z_L8_30_v1.1"
BASE = f"{ARCHIVE}{PREFIX}{COLLECTION}/{GRANULE}"
REPO = Path(__file__).resolve().parents[1]
REQUEST = REPO / "docs/products/dswx-authenticated-request.json"

PRIMARY_REFERENCES = {
    "cmr_collection": "https://cmr.earthdata.nasa.gov/search/collections.umm_json?concept_id=C2617126679-POCLOUD",
    "cmr_granule": "https://cmr.earthdata.nasa.gov/search/granules.umm_json?collection_concept_id=C2617126679-POCLOUD&granule_ur=OPERA_L3_DSWx-HLS_T17SPU_20261018T154740Z_20261022T014315Z_L8_30_v1.1",
    "product_specification_v1_0_1": "https://archive.podaac.earthdata.nasa.gov/podaac-ops-cumulus-docs/web-misc/opera/OPERA_DSWx-HLS_ProductSpec_v1.0.0_D-107395_RevB.pdf",
    "proteus_release_v1_0_1": "https://github.com/nasa/PROTEUS/releases/tag/v1.0.1",
    "proteus_v1_0_1_cloud_classes": "https://github.com/nasa/PROTEUS/blob/v1.0.1/src/proteus/dswx_hls.py#L1604-L1627",
    "proteus_release_v1_0_2": "https://github.com/nasa/PROTEUS/releases/tag/v1.0.2",
    "proteus_cloud_classes": "https://github.com/nasa/PROTEUS/blob/v1.0.2/src/proteus/dswx_hls.py#L1754-L1777",
    "proteus_cloud_masking": "https://github.com/nasa/PROTEUS/blob/v1.0.2/src/proteus/dswx_hls.py#L2089-L2125",
}

CASES = {
    "B01_WTR": {
        "etag": '"4d3dbf6fddde5e1f52a319be735cf4c5-1"',
        "size_bytes": 144_597,
        "sha256": "b46b519203668a3e648d845868ba608c7104836d4f55bc104bd0fd67923c0794",
        "dtype": "uint8",
        "nodata": 255,
        "allowed": {0, 1, 2, 252, 253, 254, 255},
    },
    "B04_DIAG": {
        "etag": '"e2f33e8a704c472947421b1be64de69d-1"',
        "size_bytes": 370_937,
        "sha256": "b1ebcbe38b7a94101fd62cd869953fbb3ddccf21532998cd9be7049af374e34e",
        "dtype": "uint16",
        "nodata": 65_535,
    },
    "B08_SHAD": {
        "etag": '"d011fa1fbf02ec9e1aaf8d5ad2969115-1"',
        "size_bytes": 979_399,
        "sha256": "190c030166af11dffcef08a430a8a73680ce8c80d81631c45e0263731b4fce1d",
        "dtype": "uint8",
        "nodata": None,
        "allowed": {0, 1},
    },
    "B09_CLOUD": {
        "etag": '"de4e730c7efb6427dfeb079da320e0f8-1"',
        "size_bytes": 97_466,
        "sha256": "4885b24eafd951b399afb8e673c0186ccc914a8b5b2eb1c292355e975a4558ee",
        "dtype": "uint8",
        "nodata": 255,
        "allowed": set(range(16)) | {254, 255},
    },
}

GEOKEY_DIRECTORY = [
    1, 1, 0, 7,
    1024, 0, 1, 1,
    1025, 0, 1, 1,
    1026, 34737, 22, 0,
    2049, 34737, 7, 22,
    2054, 0, 1, 9102,
    3072, 0, 1, 32617,
    3076, 0, 1, 9001,
]


class ValidationError(Exception):
    """A sanitized validation failure that contains no temporary URL."""


def require(condition, message):
    if not condition:
        raise ValidationError(message)


def origin(url):
    parsed = urlparse(url)
    return f"{parsed.scheme}://{parsed.netloc}"


def make_session():
    session = requests.Session()
    session.trust_env = False
    session.headers.update(
        {"Accept-Encoding": "identity", "User-Agent": "tile-ripper-dswx-inspection/1"}
    )
    return session


def read_credentials(path):
    info = path.stat()
    require(stat.S_ISREG(info.st_mode), "netrc is not a regular file")
    require(info.st_mode & 0o077 == 0, "netrc must not grant group or other access")
    auth = netrc.netrc(path).authenticators("urs.earthdata.nasa.gov")
    require(auth and auth[0] and auth[2], "explicit URS credentials are missing")
    return auth[0], auth[2]


def destination_class(target_origin, target_path, canonical_path, resource_path):
    if target_origin == LOGIN and target_path == "/oauth/authorize":
        return "earthdata_login"
    if target_origin == ARCHIVE and target_path == "/login":
        return "archive_login_callback"
    if target_origin == ARCHIVE and target_path == canonical_path:
        return "canonical_object"
    if target_origin == RESOURCE and target_path == resource_path:
        return "exact_resource_object"
    return "rejected"


def fetch_exact(canonical, pin, credentials):
    parsed = urlparse(canonical)
    require(origin(canonical) == ARCHIVE, "canonical DSWx origin changed")
    require(not parsed.query and not parsed.fragment, "canonical DSWx URL is signed")
    required_prefix = f"{PREFIX}{COLLECTION}/"
    require(parsed.path.startswith(required_prefix), "canonical DSWx collection changed")
    require(parsed.path.endswith(".tif"), "canonical DSWx object is not a TIFF")
    key = parsed.path[len(PREFIX):]
    resource_path = (
        GET_GATE
        + "/podaac-ops-cumulus-protected.s3.us-west-2.amazonaws.com/"
        + key
    )
    sessions = {ARCHIVE: make_session(), LOGIN: make_session(), RESOURCE: make_session()}
    current = canonical
    trace = []
    for _ in range(8):
        current_origin = origin(current)
        current_parsed = urlparse(current)
        require(current_origin in sessions, "authenticated redirect escaped allowed origins")
        if current_origin == ARCHIVE:
            require(
                current_parsed.path in (parsed.path, "/login"),
                "authenticated redirect changed the archive path",
            )
        elif current_origin == LOGIN:
            require(
                current_parsed.path == "/oauth/authorize",
                "authenticated redirect changed the URS path",
            )
        else:
            require(
                current_parsed.path == resource_path,
                "authenticated redirect changed the resource object",
            )
        object_request = current == canonical or current_origin == RESOURCE
        headers = {}
        if object_request:
            headers = {
                "Range": f"bytes=0-{pin['size_bytes'] - 1}",
                "If-Match": pin["etag"],
            }
        response = sessions[current_origin].get(
            current,
            headers=headers,
            auth=credentials if current_origin == LOGIN else None,
            allow_redirects=False,
            stream=True,
            timeout=(10, 60),
        )
        status = response.status_code
        if status in REDIRECTS:
            require("Location" in response.headers, "redirect omitted Location")
            target = urljoin(current, response.headers["Location"])
            target_origin = origin(target)
            target_path = urlparse(target).path
            kind = destination_class(target_origin, target_path, parsed.path, resource_path)
            require(kind != "rejected", "authenticated redirect transition changed")
            trace.append(
                {
                    "from_origin": current_origin,
                    "to_origin": target_origin,
                    "status": status,
                    "destination": kind,
                    "temporary_query_present": bool(urlparse(target).query),
                }
            )
            response.close()
            current = target
            continue
        require(
            current_origin == RESOURCE and status == 206,
            "authenticated range did not resolve to the resource",
        )
        require(response.headers.get("ETag") == pin["etag"], "provider ETag changed")
        expected_range = f"bytes 0-{pin['size_bytes'] - 1}/{pin['size_bytes']}"
        require(
            response.headers.get("Content-Range") == expected_range,
            "provider range framing changed",
        )
        body = response.raw.read(pin["size_bytes"] + 1)
        response.close()
        require(len(body) == pin["size_bytes"], "provider object length changed")
        require(hashlib.sha256(body).hexdigest() == pin["sha256"], "provider bytes changed")
        return body, trace
    raise ValidationError("authenticated redirect hop budget exhausted")


def tiff_value(page, code):
    return page.tags[code].value if code in page.tags else None


def mask_receipt(mask):
    packed = np.ascontiguousarray(mask, dtype=np.uint8)
    return {
        "true_count": int(np.count_nonzero(packed)),
        "uint8_sha256": hashlib.sha256(packed.tobytes()).hexdigest(),
    }


def inspect_layer(label, pin, body):
    with rasterio.MemoryFile(body) as memory, memory.open() as dataset:
        require(dataset.driver == "GTiff", f"{label} driver changed")
        require(dataset.count == 1, f"{label} band count changed")
        require((dataset.width, dataset.height) == (3660, 3660), f"{label} shape changed")
        require(dataset.dtypes[0] == pin["dtype"], f"{label} dtype changed")
        require(dataset.nodata == pin["nodata"], f"{label} nodata changed")
        require(str(dataset.crs) == "EPSG:32617", f"{label} CRS changed")
        require(
            tuple(dataset.transform)[:6] == (30.0, 0.0, 600000.0, 0.0, -30.0, 3900000.0),
            f"{label} transform changed",
        )
        require(dataset.block_shapes == [(512, 512)], f"{label} tile shape changed")
        require(dataset.overviews(1) == [4, 16, 63, 126], f"{label} overviews changed")
        tags = dataset.tags()
        for key, expected in {
            "PRODUCT_ID": "dswx_hls",
            "PRODUCT_TYPE": "DSWx-HLS",
            "PRODUCT_VERSION": "1.1",
            "SOFTWARE_VERSION": "1.0.2",
            "PRODUCT_LEVEL": "3",
            "PRODUCT_SOURCE": "HLS",
            "HLS_DATASET": "HLS.L30.T17SPU.2026291T154740.v2.0",
            "SENSING_TIME": "2026-10-18T15:47:40.2028270Z",
            "PROCESSING_DATETIME": "2026-10-22T01:43:15Z",
            "AREA_OR_POINT": "Area",
        }.items():
            require(tags.get(key) == expected, f"{label} metadata {key} changed")
        array = dataset.read(1)
        values, counts = np.unique(array, return_counts=True)
        observed = {int(value): int(count) for value, count in zip(values, counts)}
        if "allowed" in pin:
            require(set(observed) <= pin["allowed"], f"{label} class domain changed")
        if label == "B04_DIAG":
            valid = all(
                value == 65_535
                or (len(str(value)) <= 5 and set(str(value)) <= {"0", "1"})
                for value in observed
            )
            require(valid, "DIAG contains a value outside the five decimal-coded test flags")
        if label == "B09_CLOUD":
            require(8 in observed, "CLOUD does not exercise aerosol reassignment class 8")
            require(
                tags.get("AEROSOL_CLASS_REMAPPING_ENABLED") == "TRUE",
                "CLOUD aerosol remapping metadata changed",
            )
        masks = {"source_valid": mask_receipt(array != pin["nodata"])}
        if label == "B01_WTR":
            masks["science_classes_0_1_2"] = mask_receipt(np.isin(array, [0, 1, 2]))
            masks["water_classes_1_2"] = mask_receipt(np.isin(array, [1, 2]))
        elif label == "B09_CLOUD":
            masks["low_three_obstruction_bits_clear"] = mask_receipt(
                (array != 255) & ((array & 0b111) == 0)
            )
            masks["low_three_obstruction_bits_set"] = mask_receipt(
                (array != 255) & ((array & 0b111) != 0)
            )
        selected_tags = {
            key: tags[key]
            for key in (
                "PRODUCT_ID",
                "PRODUCT_TYPE",
                "PRODUCT_VERSION",
                "SOFTWARE_VERSION",
                "PRODUCT_LEVEL",
                "PRODUCT_SOURCE",
                "HLS_DATASET",
                "SENSING_TIME",
                "PROCESSING_DATETIME",
                "AREA_OR_POINT",
                "SENSOR",
                "SPACECRAFT_NAME",
                "AEROSOL_CLASS_REMAPPING_ENABLED",
                "MASK_ADJACENT_TO_CLOUD_MODE",
                "OCEAN_MASKING_ENABLED",
            )
            if key in tags
        }
        raster = {
            "dtype": dataset.dtypes[0],
            "nodata": dataset.nodata,
            "shape": [dataset.height, dataset.width],
            "crs": str(dataset.crs),
            "transform": list(tuple(dataset.transform)[:6]),
            "bounds": list(dataset.bounds),
            "block_shape": list(dataset.block_shapes[0]),
            "overviews": dataset.overviews(1),
            "compression": dataset.compression.name,
            "unique_counts": {str(value): count for value, count in observed.items()},
            "sample_window_x3051_y831_w4_h5": array[831:836, 3051:3055].tolist(),
            "metadata": selected_tags,
            "mask_receipts": masks,
        }
    with tifffile.TiffFile(io.BytesIO(body)) as tiff:
        page = tiff.pages[0]
        geokeys = list(tiff_value(page, 34735))
        require(geokeys == GEOKEY_DIRECTORY, f"{label} GeoKey directory changed")
        require(
            list(tiff_value(page, 33550)) == [30.0, 30.0, 0.0],
            f"{label} ModelPixelScale changed",
        )
        require(
            list(tiff_value(page, 33922)) == [0.0, 0.0, 0.0, 600000.0, 3900000.0, 0.0],
            f"{label} ModelTiepoint changed",
        )
        raw_nodata = tiff_value(page, 42113)
        if isinstance(raw_nodata, bytes):
            raw_nodata = raw_nodata.rstrip(b"\0").decode("ascii")
        tiff_info = {
            "byte_order": tiff.byteorder,
            "bigtiff": tiff.is_bigtiff,
            "ifd_count": len(tiff.pages),
            "bits_per_sample": int(tiff_value(page, 258)),
            "sample_format": int(tiff_value(page, 339)),
            "compression_code": int(tiff_value(page, 259)),
            "predictor": int(tiff_value(page, 317)),
            "model_pixel_scale": list(tiff_value(page, 33550)),
            "model_tiepoint": list(tiff_value(page, 33922)),
            "geokey_directory": geokeys,
            "geo_ascii_params": tiff_value(page, 34737).rstrip("\0"),
            "gdal_nodata": raw_nodata,
        }
    return {"raster": raster, "tiff": tiff_info}


def iso_ns(value):
    match = re.fullmatch(r"(.+?)(?:\.(\d+))?(Z|[+-]\d\d:\d\d)", value)
    require(match is not None, "timestamp is not RFC3339")
    base, fraction, zone = match.groups()
    fraction = ((fraction or "") + "000000")[:6]
    parsed = datetime.fromisoformat(
        f"{base}.{fraction}{'+00:00' if zone == 'Z' else zone}"
    )
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


def run_native_fetch(binary, work, netrc_path):
    binary = binary.expanduser().resolve()
    require(binary.is_file(), "--binary does not name a file")
    work = work.expanduser().resolve()
    work.mkdir(parents=True, exist_ok=True)
    output = work / "opera-dswx-hls-v1.1.zarr"
    require(not output.exists(), "native output already exists")
    command = [
        str(binary),
        "fetch",
        str(REQUEST),
        "--auth",
        "earthdata",
        "--netrc",
        str(netrc_path),
        "--output",
        str(output),
        "--output-format",
        "zarr_v2",
        "--backend",
        "native",
        "--json",
    ]
    completed = subprocess.run(
        command,
        cwd=REPO,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=180,
        check=False,
    )
    require(completed.returncode == 0, "native Tile-Ripper fetch failed")
    return output, hashlib.sha256(binary.read_bytes()).hexdigest()


def validate_native_output(output, bodies):
    request = json.loads(REQUEST.read_text())
    require(
        request["bands"] == ["WTR", "DIAG", "SHAD", "CLOUD"],
        "pinned request bands changed",
    )
    require(
        request["quality_policies"]
        == ["dswx_wtr_water", "dswx_shadow_free", "dswx_cloud_obstruction_free"],
        "pinned request policies changed",
    )
    manifest_path = output / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    require(manifest["status"] == "complete", "native output is incomplete")
    outputs = {entry["asset_id"]: entry["group"] for entry in manifest["outputs"]}
    sources = {entry["asset_id"]: entry for entry in manifest["sources"]}
    labels = {
        "p000_WTR": "B01_WTR",
        "p000_DIAG": "B04_DIAG",
        "p000_SHAD": "B08_SHAD",
        "p000_CLOUD": "B09_CLOUD",
    }
    require(set(outputs) == set(labels), "native output asset groups changed")
    require(set(sources) == set(labels), "native output source identities changed")
    bbox = request["aoi"]["bbox"]
    observation = request["observations"][0]
    results = {}
    for asset_id, label in labels.items():
        pin = CASES[label]
        source = sources[asset_id]
        require(source["object_version"] == pin["etag"], f"{label} manifest ETag changed")
        require(source["item_id"] == GRANULE, f"{label} manifest granule changed")
        require(source["product_version"] == "1.1", f"{label} product version changed")
        require(
            source["reference"] == f"{BASE}_{label}.tif",
            f"{label} canonical reference changed",
        )
        physical = source["object_identities"]["native_cog"]
        require(physical["geokey_directory"] == GEOKEY_DIRECTORY, f"{label} retained GeoKeys changed")
        require(physical["raster_type"] == "PixelIsArea", f"{label} raster type changed")
        require(physical["normalized_crs"] == "EPSG:32617", f"{label} normalized CRS changed")

        group = outputs[asset_id]
        dataset = xr.open_zarr(
            output,
            group=group,
            consolidated=False,
            decode_times=False,
            mask_and_scale=False,
        )
        attrs = dict(zarr.open_group(output, mode="r")[group].attrs)
        source_attrs = attrs["source"]
        require(source_attrs["product_band"]["unit"] == "1", f"{label} CF unit changed")
        require(source_attrs["product_version"] == "1.1", f"{label} stored version changed")
        window_data = attrs["source_window"]
        window = Window(
            window_data["x"],
            window_data["y"],
            window_data["width"],
            window_data["height"],
        )
        with rasterio.MemoryFile(bodies[label]) as memory, memory.open() as source_raster:
            raw = source_raster.read(1, window=window)
            source_transform = source_raster.window_transform(window)
            source_nodata = source_raster.nodata
            source_crs = source_raster.crs.to_string()
        rows, columns = np.indices(raw.shape)
        xs, ys = xy(source_transform, rows, columns, offset="center")
        longitudes, latitudes = warp_transform(
            source_crs,
            "EPSG:4326",
            np.asarray(xs).ravel(),
            np.asarray(ys).ravel(),
        )
        longitudes = np.asarray(longitudes).reshape(raw.shape)
        latitudes = np.asarray(latitudes).reshape(raw.shape)
        independent_aoi = (
            (longitudes >= bbox[0])
            & (longitudes <= bbox[2])
            & (latitudes >= bbox[1])
            & (latitudes <= bbox[3])
        )
        independent_valid = independent_aoi.copy()
        if source_nodata is not None:
            independent_valid &= raw != source_nodata
        expected_values = raw.copy()
        if source_nodata is not None:
            expected_values[~independent_aoi] = np.asarray(source_nodata, dtype=raw.dtype)
        values = dataset["values"].values[0]
        valid = dataset["valid"].values[0].astype(bool)
        aoi = dataset["aoi_mask"].values[0].astype(bool)
        require(np.array_equal(values, expected_values), f"{label} stored values differ")
        require(np.array_equal(valid, independent_valid), f"{label} validity differs")
        require(np.array_equal(aoi, independent_aoi), f"{label} AOI mask differs")
        expected_x = source_transform.c + source_transform.a * (np.arange(raw.shape[1]) + 0.5)
        expected_y = source_transform.f + source_transform.e * (np.arange(raw.shape[0]) + 0.5)
        require(np.array_equal(dataset["x"].values, expected_x), f"{label} x coordinates differ")
        require(np.array_equal(dataset["y"].values, expected_y), f"{label} y coordinates differ")
        require(
            int(dataset["time"].values[0]) == iso_ns(observation["datetime"]),
            f"{label} stored time differs",
        )
        expected_quality = None
        quality_name = None
        quality_definition = None
        if label == "B01_WTR":
            quality_name = "quality_dswx_wtr_water"
            quality_definition = "valid AOI sample with WTR class 1 or 2"
            expected_quality = independent_valid & np.isin(raw, [1, 2])
        elif label == "B08_SHAD":
            quality_name = "quality_dswx_shadow_free"
            quality_definition = "valid AOI sample with SHAD class 1"
            expected_quality = independent_valid & (raw == 1)
        elif label == "B09_CLOUD":
            quality_name = "quality_dswx_cloud_obstruction_free"
            quality_definition = (
                "valid AOI sample with CLOUD bits 0-2 clear; bit 3 is retained"
            )
            expected_quality = independent_valid & ((raw.astype(np.uint8) & 0b111) == 0)
        quality_receipt = None
        if quality_name:
            observed_quality = dataset[quality_name].values[0].astype(bool)
            require(
                np.array_equal(observed_quality, expected_quality),
                f"{label} derived quality differs",
            )
            quality_receipt = {
                "dataset": quality_name,
                "definition": quality_definition,
                "accepted": int(expected_quality.sum()),
                "uint8_sha256": sha256_array(expected_quality.astype(np.uint8)),
                "matches_independent_definition": True,
            }
        results[label] = {
            "asset_id": asset_id,
            "group": group,
            "etag": pin["etag"],
            "window": window_data,
            "shape": list(raw.shape),
            "dtype": raw.dtype.name,
            "nodata": source_nodata,
            "crs": source_crs,
            "unit": source_attrs["product_band"]["unit"],
            "source_values_sha256": sha256_array(raw),
            "source_unique_counts": {
                str(int(value)): int(count)
                for value, count in zip(*np.unique(raw, return_counts=True))
            },
            "stored_values_sha256": sha256_array(values),
            "values_match_independent_rasterio": True,
            "aoi_mask_matches_wgs84_pixel_centers": True,
            "valid_matches_aoi_and_source_fill": True,
            "aoi_pixels": int(independent_aoi.sum()),
            "valid_pixels": int(independent_valid.sum()),
            "coordinates_match_source_pixel_centers": True,
            "time_matches_request_nanoseconds": True,
            "quality": quality_receipt,
        }
    observed = manifest["observed"]
    scheduler = observed["scheduler"]
    transport = scheduler["native_metadata"]["authenticated_transport"]
    fields = (
        "wire_head",
        "wire_get",
        "redirects",
        "login_requests",
        "refreshes",
        "route_cache_hits",
        "response_body_bytes",
        "redirect_body_bytes",
        "peak_in_flight",
    )
    return {
        "result": "pass",
        "output_path_recorded": False,
        "request_file": REQUEST.name,
        "request_sha256": hashlib.sha256(REQUEST.read_bytes()).hexdigest(),
        "manifest_sha256": hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
        "processing_contract": manifest["processing"],
        "layers": results,
        "observed": {
            "chunks": observed["chunks"],
            "decoded_bytes": observed["decoded_bytes"],
            "non_nodata_pixels": observed["non_nodata_pixels"],
            "source_data_requests": observed["source_data_requests"],
            "source_data_bytes": observed["source_data_bytes"],
            "scheduler_buffer_bytes": scheduler["buffer_bytes"],
            "scheduler_reserved_bytes_peak": scheduler["reserved_bytes_peak"],
            "prepared_native_metadata_bytes": scheduler["prepared_native_metadata_bytes"],
            "authenticated_transport_reserved_bytes": scheduler[
                "authenticated_transport_reserved_bytes"
            ],
            "transport": {
                asset: {field: values[field] for field in fields}
                for asset, values in sorted(transport.items())
            },
        },
    }


def run(netrc_path, output=None, binary_sha256=None):
    credentials = read_credentials(netrc_path)
    layers = {}
    bodies = {}
    for label, pin in CASES.items():
        canonical = f"{BASE}_{label}.tif"
        body, trace = fetch_exact(canonical, pin, credentials)
        bodies[label] = body
        layers[label] = {
            "canonical_url": canonical,
            "etag": pin["etag"],
            "size_bytes": pin["size_bytes"],
            "sha256": pin["sha256"],
            "authenticated_get_trace": trace,
            **inspect_layer(label, pin, body),
        }
    wtr_science = layers["B01_WTR"]["raster"]["mask_receipts"]["science_classes_0_1_2"]
    cloud_clear = layers["B09_CLOUD"]["raster"]["mask_receipts"][
        "low_three_obstruction_bits_clear"
    ]
    require(
        wtr_science == cloud_clear,
        "WTR science classes do not align with the independent CLOUD obstruction policy",
    )
    report = {
        "schema": "tile-ripper.wave1-dswx-current-inspection.v1",
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "result": "pass",
        "collection": {
            "concept_id": "C2617126679-POCLOUD",
            "short_name": COLLECTION,
            "catalog_version": "1.0",
        },
        "granule": {
            "concept_id": "G4382655954-POCLOUD",
            "id": GRANULE,
            "product_version": "1.1",
            "software_version": "1.0.2",
            "sensing_time": "2026-10-18T15:47:40.202Z",
            "source_hls": "HLS.L30.T17SPU.2026291T154740.v2.0",
        },
        "primary_references": PRIMARY_REFERENCES,
        "security": {
            "credentials_source": "explicit private netrc argument",
            "basic_authorization_origin": LOGIN,
            "allowed_origins": [ARCHIVE, LOGIN, RESOURCE],
            "temporary_provider_queries_persisted": False,
            "conditional_request": "Range plus strong If-Match",
        },
        "scope": {
            "layers_inspected": list(CASES),
            "object_bytes_read": sum(case["size_bytes"] for case in CASES.values()),
            "reader": "independent rasterio MemoryFile plus tifffile",
        },
        "layers": layers,
        "findings": {
            "cloud_bit_3": "aerosol class reassignment in PROTEUS v1.0.2; not the HLS Fmask water bit",
            "cloud_8_obstruction": "class 8 alone is not treated as cloud obstruction by PROTEUS v1.0.2",
            "diag_encoding": "UInt16 decimal digits encode five Boolean tests; 65535 is fill",
            "shadow_nodata": "B08 SHAD has no GDAL_NODATA tag; its domain is 0 shadow and 1 not-shadow",
            "independent_policy_check": {
                "rule": "CLOUD is non-fill and bits 0..2 are clear; bit 3 is retained",
                "result": "pixel-exact match to WTR classes 0, 1, and 2 for this granule",
                "mask_receipt": cloud_clear,
            },
        },
    }
    if output is not None:
        report["native_output_validation"] = validate_native_output(output, bodies)
        report["native_output_validation"]["binary_sha256"] = binary_sha256
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--netrc", required=True, type=Path, help="explicit private netrc")
    parser.add_argument("--output", type=Path, help="validate an existing native Zarr output")
    parser.add_argument("--binary", type=Path, help="run this Tile-Ripper binary before validation")
    parser.add_argument("--work", type=Path, help="new work directory used with --binary")
    parser.add_argument("--report", type=Path, help="write sanitized JSON report")
    args = parser.parse_args()
    require(not (args.output and args.binary), "use either --output or --binary")
    require((args.binary is None) == (args.work is None), "--binary and --work are required together")
    netrc_path = args.netrc.expanduser().resolve()
    output = args.output.expanduser().resolve() if args.output else None
    binary_sha256 = None
    if args.binary:
        output, binary_sha256 = run_native_fetch(args.binary, args.work, netrc_path)
    report = run(netrc_path, output, binary_sha256)
    rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.report:
        args.report.write_text(rendered)
    else:
        print(rendered, end="")


if __name__ == "__main__":
    try:
        main()
    except (OSError, netrc.NetrcParseError, requests.RequestException, ValidationError) as exc:
        print(f"DSWx inspection failed: {exc}", file=__import__("sys").stderr)
        raise SystemExit(1)
