"""Independent rasterio -> Zarr reference for Wave 1 product outputs.

This process imports no Tile-Ripper modules.  The native result manifest fixes
the approved source windows and group order; independently encoded product
tables below define calibration, units, nodata, and quality policies.

Usage:
  python bench/wave1_reference.py REQUEST.json NATIVE_MANIFEST.json NEW_OUTPUT.zarr

Only explicit, unsigned loopback HTTP asset URLs are accepted.  The output is
written in 256-pixel chunks to a sibling staging directory and atomically
renamed into place after all arrays and metadata have been completed.
"""

from __future__ import annotations

import argparse
from contextlib import contextmanager
from dataclasses import dataclass
import datetime as dt
import hashlib
import ipaddress
import json
import math
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

import numpy as np
from pyproj import CRS as PyprojCRS
import rasterio
from rasterio.features import geometry_mask
from rasterio.warp import transform_geom
from rasterio.windows import Window
import zarr


CHUNK_EDGE = 256


@dataclass(frozen=True)
class Band:
    dtype: str
    nodata: int | float | str | None
    scale: float
    offset: float
    unit: str


REFLECTANCE_I16 = Band("int16", -9999, 0.0001, 0.0, "reflectance")
ANGLE_U16 = Band("uint16", 40000, 0.01, 0.0, "degree")
LANDSAT_SR = Band("uint16", 0, 0.0000275, -0.2, "reflectance")


def product_band(product: str, name: str) -> Band:
    """Return independently transcribed product storage/calibration metadata."""
    if product == "landsat-c2-l2-sr":
        if name in {f"SR_B{i}" for i in range(1, 8)}:
            return LANDSAT_SR
        if name == "QA_PIXEL":
            return Band("uint16", 1, 1.0, 0.0, "bit_field")
        if name == "QA_RADSAT":
            return Band("uint16", None, 1.0, 0.0, "bit_field")
        if name == "SR_QA_AEROSOL":
            return Band("uint8", 1, 1.0, 0.0, "bit_field")
    if product == "hls-l30":
        if name in {"B01", "B02", "B03", "B04", "B05", "B06", "B07", "B09"}:
            return REFLECTANCE_I16
        if name in {"B10", "B11"}:
            return Band("int16", -9999, 0.01, 0.0, "degree_celsius")
        if name == "Fmask":
            return Band("uint8", 255, 1.0, 0.0, "bit_field")
        if name in {"SZA", "SAA", "VZA", "VAA"}:
            return ANGLE_U16
    if product == "hls-s30":
        if name in {
            "B01", "B02", "B03", "B04", "B05", "B06", "B07",
            "B08", "B8A", "B09", "B10", "B11", "B12",
        }:
            return REFLECTANCE_I16
        if name == "Fmask":
            return Band("uint8", 255, 1.0, 0.0, "bit_field")
        if name in {"SZA", "SAA", "VZA", "VAA"}:
            return ANGLE_U16
    if product == "opera-dswx-hls":
        if name in {"WTR", "BWTR", "CONF", "WTR-1", "WTR-2", "LAND", "CLOUD"}:
            return Band("uint8", 255, 1.0, 0.0, "class")
        if name == "DIAG":
            return Band("uint16", 65535, 1.0, 0.0, "five_digit_test_code")
        if name == "SHAD":
            return Band("uint8", None, 1.0, 0.0, "class")
        if name == "DEM":
            return Band("float32", None, 1.0, 0.0, "meter_above_wgs84_ellipsoid")
    if product == "opera-rtc-s1":
        if name in {"VV", "VH", "HH", "HV"}:
            # OPERA specimen TIFFs mark NaN as nodata.  Tile-Ripper preserves
            # all stored values and treats NaN invalid without inventing a
            # finite sentinel, so the Zarr fill remains null.
            return Band("float32", "NaN", 1.0, 0.0, "linear_gamma0_power")
        if name == "mask":
            return Band("uint8", 255, 1.0, 0.0, "class")
    raise ValueError(f"unsupported independent product band: {product}/{name}")


def product_source_validity(product: str, name: str, profile: Band, values: np.ndarray) -> np.ndarray:
    """Apply documented product fill independently of physical TIFF nodata."""
    valid = (
        ~np.isnan(values)
        if values.dtype.kind == "f"
        else np.ones(values.shape, dtype=bool)
    )
    product_nodata = normalized_nodata(profile.nodata)
    if product_nodata == "NaN":
        valid &= ~np.isnan(values)
    elif product_nodata is not None:
        valid &= values != profile.nodata
    if product == "landsat-c2-l2-sr" and name in {"QA_PIXEL", "SR_QA_AEROSOL"}:
        valid &= (values.astype(profile.dtype, copy=False) & 1) == 0
    return valid


POLICY_BANDS = {
    "landsat_pixel_clear": {"QA_PIXEL"},
    "landsat_radiometrically_unsaturated": {"QA_RADSAT"},
    "landsat_aerosol_valid": {"SR_QA_AEROSOL"},
    "landsat_aerosol_recommended": {"SR_QA_AEROSOL"},
    "hls_fmask_clear": {"Fmask"},
    "hls_fmask_clear_low_aerosol": {"Fmask"},
    "dswx_wtr_water": {"WTR"},
    "dswx_bwtr_water": {"BWTR"},
    "dswx_conf_unmasked_water": {"CONF"},
    "dswx_conf_any_water": {"CONF"},
    "dswx_wtr1_water": {"WTR-1"},
    "dswx_wtr2_water": {"WTR-2"},
    "dswx_shadow_free": {"SHAD"},
    "dswx_cloud_obstruction_free": {"CLOUD"},
    "opera_rtc_finite": {"VV", "VH", "HH", "HV"},
    "opera_rtc_mask_valid": {"mask"},
    "opera_rtc_mask_unaffected": {"mask"},
}


# Product aliases are independently transcribed here so requests that use a
# semantic band name still bind each quality policy to its canonical raw band.
BAND_ALIASES = {
    "landsat-c2-l2-sr": {
        "SR_B1": ("coastal", "coastal_aerosol"),
        "SR_B2": ("blue",),
        "SR_B3": ("green",),
        "SR_B4": ("red",),
        "SR_B5": ("nir", "nir_narrow"),
        "SR_B6": ("swir1", "swir_1"),
        "SR_B7": ("swir2", "swir_2"),
        "QA_PIXEL": ("pixel_qa", "qa_pixel"),
        "QA_RADSAT": ("radsat_qa", "qa_radsat"),
        "SR_QA_AEROSOL": ("aerosol_qa", "qa_aerosol"),
    },
    "hls-l30": {"Fmask": ("fmask", "qa")},
    "hls-s30": {"Fmask": ("fmask", "qa")},
    "opera-dswx-hls": {
        "WTR": ("water_classification",),
        "BWTR": ("binary_water",),
        "CONF": ("confidence",),
        "DIAG": ("diagnostic",),
        "WTR-1": ("wtr1",),
        "WTR-2": ("wtr2",),
        "LAND": ("land_cover",),
        "SHAD": ("terrain_shadow",),
        "CLOUD": ("hls_cloud_snow_fmask",),
        "DEM": ("elevation",),
    },
    "opera-rtc-s1": {
        "VV": ("vv",),
        "VH": ("vh",),
        "HH": ("hh",),
        "HV": ("hv",),
        "mask": ("layover_shadow_mask",),
    },
}


def require_domain(values: np.ndarray, allowed: set[int], label: str) -> None:
    observed = set(np.unique(values).astype("uint64").tolist())
    extra = observed - allowed
    if extra:
        raise ValueError(f"{label} contains undefined values: {sorted(extra)[:8]}")


def quality(policy: str, values: np.ndarray) -> np.ndarray:
    """Evaluate one raw layer using definitions independent of Tile-Ripper."""
    if policy == "landsat_pixel_clear":
        word = values.astype("uint16", copy=False)
        cloud = (word >> 8) & 3
        shadow = (word >> 10) & 3
        snow = (word >> 12) & 3
        cirrus = (word >> 14) & 3
        if np.any((shadow == 2) | (snow == 2) | (cirrus == 2)):
            raise ValueError("QA_PIXEL contains a reserved confidence state")
        fill = (word & 1) != 0
        if np.any(fill & (word != 1)):
            raise ValueError("QA_PIXEL fill code has other fields")
        clear = (word & (1 << 6)) != 0
        expected_clear = (word & ((1 << 1) | (1 << 3))) == 0
        contradictory = (~fill) & (
            (clear != expected_clear)
            | (((word & (1 << 3)) != 0) != (cloud == 3))
            | (((word & (1 << 4)) != 0) != (shadow == 3))
            | (((word & (1 << 5)) != 0) != (snow == 3))
            | (((word & (1 << 2)) != 0) != (cirrus == 3))
        )
        if np.any(contradictory):
            raise ValueError("QA_PIXEL flag and confidence fields conflict")
        return ((word & 0x003F) == 0) & clear
    if policy == "landsat_radiometrically_unsaturated":
        word = values.astype("uint16", copy=False)
        if np.any((word & np.uint16(~0x097F & 0xFFFF)) != 0):
            raise ValueError("QA_RADSAT contains an unused bit")
        return word == 0
    if policy in {"landsat_aerosol_valid", "landsat_aerosol_recommended"}:
        word = values.astype("uint16", copy=False)
        documented = {1, 2, 4, 32, 66, 68, 96, 100, 130, 132, 160, 164, 192, 194, 196, 224, 228}
        require_domain(word, documented, "SR_QA_AEROSOL")
        accepted = {2, 66, 130, 194} if policy.endswith("_valid") else {2, 66, 130}
        return np.isin(word, tuple(accepted))
    if policy in {"hls_fmask_clear", "hls_fmask_clear_low_aerosol"}:
        word = values.astype("uint8", copy=False)
        if np.any((word != 255) & ((word & 1) != 0)):
            raise ValueError("HLS Fmask reserved cirrus bit is set")
        accepted = (word & 0x1E) == 0
        if policy.endswith("_low_aerosol"):
            accepted &= ((word >> 6) & 3) <= 1
        return accepted
    if policy == "dswx_wtr_water":
        value = values.astype("uint8", copy=False)
        require_domain(value, {0, 1, 2, 252, 253, 254, 255}, "WTR")
        return np.isin(value, (1, 2))
    if policy == "dswx_bwtr_water":
        value = values.astype("uint8", copy=False)
        require_domain(value, {0, 1, 252, 253, 254, 255}, "BWTR")
        return value == 1
    if policy in {"dswx_conf_unmasked_water", "dswx_conf_any_water"}:
        value = values.astype("uint8", copy=False)
        allowed = set(range(0, 5)) | set(range(10, 15)) | set(range(20, 25)) | {254, 255}
        require_domain(value, allowed, "CONF")
        if policy.endswith("_unmasked_water"):
            return (value >= 1) & (value <= 4)
        return ((value >= 1) & (value <= 4)) | ((value >= 11) & (value <= 14)) | ((value >= 21) & (value <= 24))
    if policy in {"dswx_wtr1_water", "dswx_wtr2_water"}:
        value = values.astype("uint8", copy=False)
        require_domain(value, {0, 1, 2, 254, 255}, "WTR intermediate")
        return np.isin(value, (1, 2))
    if policy == "dswx_shadow_free":
        value = values.astype("uint8", copy=False)
        require_domain(value, {0, 1}, "SHAD")
        return value == 1
    if policy == "dswx_cloud_obstruction_free":
        value = values.astype("uint8", copy=False)
        require_domain(value, set(range(16)) | {255}, "CLOUD")
        return np.isin(value, (0, 8))
    if policy == "opera_rtc_finite":
        if values.dtype.kind != "f":
            raise ValueError("RTC finite policy requires Float32")
        return np.isfinite(values)
    if policy in {"opera_rtc_mask_valid", "opera_rtc_mask_unaffected"}:
        value = values.astype("uint8", copy=False)
        require_domain(value, {0, 1, 2, 3, 255}, "RTC mask")
        return value <= 3 if policy.endswith("_valid") else value == 0
    raise ValueError(f"unsupported quality policy: {policy}")


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def json_safe(value):
    if isinstance(value, dict):
        return {str(key): json_safe(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [json_safe(item) for item in value]
    if isinstance(value, np.generic):
        return json_safe(value.item())
    if isinstance(value, float) and not math.isfinite(value):
        return "NaN" if math.isnan(value) else ("Infinity" if value > 0 else "-Infinity")
    return value


def read_json(path: Path) -> dict:
    value = json.loads(path.read_text())
    if not isinstance(value, dict):
        raise ValueError(f"expected JSON object: {path}")
    return value


def loopback_url(value: str) -> str:
    parsed = urllib.parse.urlsplit(value)
    if parsed.scheme != "http" or parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError("reference assets require unsigned loopback HTTP URLs")
    host = parsed.hostname
    if host != "localhost":
        try:
            if host is None or not ipaddress.ip_address(host).is_loopback:
                raise ValueError("reference asset host is not loopback")
        except ValueError as error:
            raise ValueError("reference asset host is not loopback") from error
    return value


def head(url: str, etag: str) -> dict:
    request = urllib.request.Request(url, headers={"If-Match": etag}, method="HEAD")
    with urllib.request.urlopen(request, timeout=15) as response:
        observed = response.headers.get("ETag")
        if observed != etag:
            raise ValueError("loopback object ETag differs from explicit request")
        return {
            "etag": observed,
            "bytes": int(response.headers["Content-Length"]),
            "last_modified": response.headers.get("Last-Modified"),
        }


@contextmanager
def curl_diagnostics():
    """Capture GDAL/libcurl stderr without mixing diagnostics into JSON stdout."""
    duplicate = os.dup(2)
    with tempfile.TemporaryFile() as stream:
        try:
            os.dup2(stream.fileno(), 2)
            yield stream
        finally:
            os.dup2(duplicate, 2)
            os.close(duplicate)


def curl_counts(stream) -> dict:
    stream.seek(0)
    text = stream.read().decode("utf-8", "replace")
    ranges = [(int(a), int(b)) for a, b in re.findall(r"^< Content-Range: bytes (\d+)-(\d+)/", text, re.M | re.I)]
    return {
        "http_gets": len(re.findall(r"^> GET ", text, re.M)),
        "http_heads": len(re.findall(r"^> HEAD ", text, re.M)),
        "http_range_response_bytes": sum(end - start + 1 for start, end in ranges),
        "http_range_responses": len(ranges),
    }


def request_geometry(request: dict, destination_crs) -> dict:
    aoi = request["aoi"]
    if aoi["kind"] == "bounds":
        west, south, east, north = map(float, aoi["bbox"])
        geometry = {
            "type": "Polygon",
            "coordinates": [[
                [west, south], [east, south], [east, north],
                [west, north], [west, south],
            ]],
        }
    elif aoi["kind"] == "polygon":
        geometry = {"type": "Polygon", "coordinates": aoi["coordinates"]}
    else:
        raise ValueError("independent reference supports bounds or polygon AOIs")
    return transform_geom(aoi["crs"], destination_crs, geometry)


def normalized_nodata(value):
    if isinstance(value, str) and value.lower() == "nan":
        return "NaN"
    if isinstance(value, float) and math.isnan(value):
        return "NaN"
    return value


def same_number(left, right, tolerance=1e-12) -> bool:
    if left is None or right is None:
        return left is right
    if normalized_nodata(left) == "NaN" or normalized_nodata(right) == "NaN":
        return normalized_nodata(left) == normalized_nodata(right)
    return math.isclose(float(left), float(right), rel_tol=0.0, abs_tol=tolerance)


def time_ns(value: str | None) -> int:
    if value is None:
        return np.iinfo("int64").min
    normalized = value[:-1] if value.endswith("Z") else value
    # Validate timezone syntax separately; numpy then retains nanosecond digits.
    dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    return int(np.datetime64(normalized, "ns").astype("int64"))


def zarr_array(group, name, shape, chunks, dtype, fill_value, attrs):
    array = group.create_dataset(
        name,
        shape=shape,
        chunks=chunks,
        dtype=dtype,
        compressor=None,
        fill_value=fill_value,
        overwrite=False,
        order="C",
    )
    array.attrs.update(json_safe(attrs))
    return array


def crs_attributes(crs, transform) -> dict:
    projection = PyprojCRS.from_user_input(crs)
    attrs = json_safe(projection.to_cf())
    wkt = projection.to_wkt()
    attrs.update({
        "crs": projection.to_string(),
        "spatial_ref": wkt,
        "crs_wkt": wkt,
        "GeoTransform": " ".join(format(value, ".17g") for value in transform.to_gdal()),
    })
    return attrs


def operation_parameters(crs: PyprojCRS) -> dict[str, float]:
    operation = crs.coordinate_operation
    if operation is None or operation.method_name != "Transverse Mercator":
        return {}
    return {parameter.name: float(parameter.value) for parameter in operation.params}


def dswx_psd_wgs84_utm10n(source: PyprojCRS, approved: PyprojCRS) -> bool:
    """Recognize the one public DSWx specimen's PSD CRS representation.

    Its GeoKeys select a user-defined datum over the WGS84 ellipsoid and UTM
    zone 10 projection. This remains narrower than same-ellipsoid equivalence.
    """
    datum_name = re.sub(r"[_\s]+", " ", source.datum.name).strip().lower()
    source_parameters = operation_parameters(source)
    approved_parameters = operation_parameters(approved)
    expected_parameters = {
        "Latitude of natural origin": 0.0,
        "Longitude of natural origin": -123.0,
        "Scale factor at natural origin": 0.9996,
        "False easting": 500_000.0,
        "False northing": 0.0,
    }
    return bool(
        source.is_projected
        and approved.is_projected
        and datum_name == "not specified based on wgs 84 spheroid"
        and approved.to_epsg() == 32610
        and source.utm_zone == approved.utm_zone == "10N"
        and source.prime_meridian.longitude == approved.prime_meridian.longitude == 0.0
        and all(
            axis.unit_name.lower() in {"metre", "meter"}
            and math.isclose(axis.unit_conversion_factor, 1.0, rel_tol=0.0, abs_tol=1e-15)
            for axis in (*source.axis_info, *approved.axis_info)
        )
        and math.isclose(source.ellipsoid.semi_major_metre, 6_378_137.0, rel_tol=0.0, abs_tol=1e-8)
        and math.isclose(
            source.ellipsoid.inverse_flattening,
            298.257223563,
            rel_tol=0.0,
            abs_tol=1e-12,
        )
        and math.isclose(approved.ellipsoid.semi_major_metre, 6_378_137.0, rel_tol=0.0, abs_tol=1e-8)
        and math.isclose(
            approved.ellipsoid.inverse_flattening,
            298.257223563,
            rel_tol=0.0,
            abs_tol=1e-12,
        )
        and source_parameters.keys() == expected_parameters.keys()
        and approved_parameters.keys() == expected_parameters.keys()
        and all(
            math.isclose(source_parameters[name], expected, rel_tol=0.0, abs_tol=1e-12)
            and math.isclose(approved_parameters[name], expected, rel_tol=0.0, abs_tol=1e-12)
            for name, expected in expected_parameters.items()
        )
    )


def equivalent_crs(product: str, left, right) -> bool:
    source = PyprojCRS.from_user_input(left)
    approved = PyprojCRS.from_user_input(right)
    if source.equals(approved, ignore_axis_order=True):
        return True
    return product == "opera-dswx-hls" and dswx_psd_wgs84_utm10n(source, approved)


def canonical_band(product: str, name: str) -> str:
    for canonical, aliases in BAND_ALIASES.get(product, {}).items():
        if name == canonical or name in aliases:
            return canonical
    return name


def resolve_asset(request: dict, source: dict, product: str) -> tuple[dict, str, dict]:
    item_id = source.get("item_id")
    band = source.get("requested_band")
    asset_id = source.get("asset_id", "")
    if band is None and "_" in asset_id:
        band = asset_id.split("_", 1)[1]
    observations = request.get("observations") or []
    if item_id is None:
        match = re.match(r"^p(\d+)_", asset_id)
        matches = [observations[int(match.group(1))]] if match and int(match.group(1)) < len(observations) else []
    else:
        matches = [item for item in observations if item.get("id") == item_id]
    if len(matches) != 1:
        raise ValueError(f"cannot resolve request observation for {asset_id}")
    observation = matches[0]
    if not isinstance(band, str):
        raise ValueError(f"cannot resolve request band for {asset_id}")
    band = canonical_band(product, band)
    candidates = (band, *BAND_ALIASES.get(product, {}).get(band, ()))
    asset_matches = [name for name in candidates if name in observation.get("assets", {})]
    if len(asset_matches) != 1:
        raise ValueError(f"cannot resolve one request asset for {asset_id}")
    return observation, band, observation["assets"][asset_matches[0]]


def selected_policies(request: dict, record: dict, band: str) -> list[str]:
    selected = [
        name for name in request.get("quality_policies", [])
        if name in POLICY_BANDS and band in POLICY_BANDS[name]
    ]
    unknown = set(request.get("quality_policies", [])) - set(POLICY_BANDS)
    if unknown:
        raise ValueError(f"unknown independent quality policies: {sorted(unknown)}")
    approved = [entry["policy"] for entry in record.get("derived_masks", [])]
    if approved and approved != selected:
        raise ValueError("native manifest quality bindings differ from independent definitions")
    return selected


def validate_manifest(native: dict) -> tuple[list[dict], dict[str, dict]]:
    if native.get("status") != "complete" or native.get("format") != "zarr_v2":
        raise ValueError("native approval manifest must be a complete Zarr v2 result")
    outputs = native.get("outputs")
    sources = native.get("sources")
    if not isinstance(outputs, list) or not isinstance(sources, list):
        raise ValueError("native approval manifest lacks outputs or sources")
    by_id = {}
    for source in sources:
        key = source.get("asset_id")
        if not isinstance(key, str) or key in by_id:
            raise ValueError("native source identities are missing or duplicated")
        by_id[key] = source
    for index, record in enumerate(outputs):
        if record.get("group") != f"observations/{index:06}":
            raise ValueError("native output group order is not canonical")
        if record.get("asset_id") not in by_id:
            raise ValueError("native output refers to an unknown source")
        window = record.get("window") or {}
        if any(not isinstance(window.get(key), int) or window[key] < 0 for key in ("x", "y")):
            raise ValueError("native output window has invalid origin")
        if any(not isinstance(window.get(key), int) or window[key] <= 0 for key in ("width", "height")):
            raise ValueError("native output window has invalid shape")
    return outputs, by_id


def fsync_tree(path: Path) -> None:
    for current, directories, files in os.walk(path, topdown=False):
        current_path = Path(current)
        for name in files:
            with (current_path / name).open("rb") as stream:
                os.fsync(stream.fileno())
        descriptor = os.open(current_path, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


def build(request_path: Path, native_path: Path, destination: Path) -> dict:
    started = time.monotonic()
    request = read_json(request_path)
    native = read_json(native_path)
    outputs, sources = validate_manifest(native)
    product = request.get("product")
    if not isinstance(product, str):
        raise ValueError("request lacks product")
    if destination.exists() or destination.is_symlink():
        raise FileExistsError("reference output must be a new path")
    destination.parent.mkdir(parents=True, exist_ok=True)
    stage = destination.parent / f".{destination.name}.reference-partial-{os.getpid()}-{uuid.uuid4().hex}"
    stage.mkdir()
    setup_started = time.monotonic()
    logical_reads = 0
    logical_read_bytes = 0
    samples = 0
    result_outputs = []
    identities = {}
    observed_http = {
        "http_gets": 0,
        "http_heads": 0,
        "http_range_response_bytes": 0,
        "http_range_responses": 0,
    }
    try:
        root = zarr.open_group(str(stage), mode="w")
        root.attrs.update(json_safe({
            "title": "Independent rasterio Wave 1 reference",
            "status": "incomplete",
            "layout": "one source asset/band per observation group in approved native order",
            "processing": (
                "native stored values; no calibration, reprojection, resampling, "
                "mosaic, or cross-band alignment"
            ),
            "reference_implementation": {
                "reader": f"rasterio {rasterio.__version__}",
                "writer": f"zarr-python {zarr.__version__}",
                "chunk_edge": CHUNK_EDGE,
                "compression": "none",
                "policy_source": "independently transcribed USGS/NASA product definitions",
            },
            "request_sha256": digest(request_path),
            "native_approval_manifest_sha256": digest(native_path),
        }))
        observations_group = root.require_group("observations")
        setup_seconds = time.monotonic() - setup_started
        write_started = time.monotonic()
        with curl_diagnostics() as curl_stream:
            for index, record in enumerate(outputs):
                source = sources[record["asset_id"]]
                observation, band_name, request_asset = resolve_asset(request, source, product)
                profile = product_band(product, band_name)
                href = loopback_url(request_asset["href"])
                etag = request_asset.get("etag")
                if not isinstance(etag, str) or etag.startswith("W/") or not etag.startswith('"'):
                    raise ValueError("reference source requires a strong explicit ETag")
                before = head(href, etag)
                identities[record["asset_id"]] = before
                window = record["window"]
                x, y = window["x"], window["y"]
                width, height = window["width"], window["height"]
                policies = selected_policies(request, record, band_name)
                with rasterio.Env(
                    GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
                    GDAL_HTTP_MULTIRANGE="SERIAL",
                    GDAL_NUM_THREADS="1",
                    GDAL_CACHEMAX=16 * 1024 * 1024,
                    GDAL_HTTP_HEADERS=f"If-Match: {etag}\r\n",
                    CPL_CURL_VERBOSE="YES",
                ):
                    with rasterio.open(href) as dataset:
                        if dataset.count != 1:
                            raise ValueError("Wave 1 reference assets must have one raster band")
                        if x + width > dataset.width or y + height > dataset.height:
                            raise ValueError("native-approved window exceeds source grid")
                        actual_dtype = np.dtype(dataset.dtypes[0])
                        if actual_dtype != np.dtype(profile.dtype):
                            raise ValueError(f"source dtype differs from {product}/{band_name} definition")
                        if record.get("dtype") and record["dtype"] != profile.dtype:
                            raise ValueError("native manifest dtype differs from independent definition")
                        source_nodata = normalized_nodata(dataset.nodata)
                        if profile.nodata == "NaN":
                            if source_nodata not in (None, "NaN"):
                                raise ValueError("RTC source has an unexpected finite nodata")
                        elif source_nodata is not None and normalized_nodata(profile.nodata) != source_nodata:
                            raise ValueError("source nodata differs from independent product definition")
                        source_scale = float(dataset.scales[0])
                        source_offset = float(dataset.offsets[0])
                        if source_scale != 1.0 and not same_number(source_scale, profile.scale):
                            raise ValueError("source scale differs from independent product definition")
                        if source_offset != 0.0 and not same_number(source_offset, profile.offset):
                            raise ValueError("source offset differs from independent product definition")
                        approved_transform = record.get("native_grid", {}).get("transform")
                        approved_crs = record.get("native_grid", {}).get("crs")
                        subset_window = Window(x, y, width, height)
                        subset_transform = dataset.window_transform(subset_window)
                        if approved_crs is None or not equivalent_crs(product, dataset.crs, approved_crs):
                            raise ValueError("native-approved CRS differs from rasterio source")
                        if approved_transform is None or not np.allclose(
                            approved_transform, subset_transform.to_gdal(), rtol=0.0, atol=1e-9,
                        ):
                            raise ValueError("native-approved transform differs from rasterio source")
                        geometry = request_geometry(request, dataset.crs)
                        approved_nodata = record.get("nodata")
                        if source_nodata == "NaN":
                            if normalized_nodata(approved_nodata) not in (None, "NaN"):
                                raise ValueError("native-approved RTC nodata is unexpectedly finite")
                        elif not same_number(approved_nodata, source_nodata):
                            raise ValueError("native-approved nodata differs from source raster metadata")
                        source_crs_wkt = dataset.crs.to_wkt()
                        normalized_crs = PyprojCRS.from_user_input(approved_crs).to_string()
                        group = observations_group.require_group(f"{index:06}")
                        group.attrs.update(json_safe({
                            "asset_id": record["asset_id"],
                            "variable": record.get("variable"),
                            "datetime": record.get("datetime"),
                            "source": {
                                "observation_id": observation["id"],
                                "collection": observation["collection"],
                                "band": band_name,
                                "href": href,
                                "etag": etag,
                                "metadata": request_asset.get("metadata", {}),
                            },
                            "source_window": window,
                            "source_grid": {
                                "width": dataset.width,
                                "height": dataset.height,
                                "crs": dataset.crs.to_string(),
                                "original_crs_wkt": source_crs_wkt,
                                "normalized_crs": normalized_crs,
                                "transform": list(dataset.transform.to_gdal()),
                                "block_shape": list(dataset.block_shapes[0]),
                                "dtype": str(actual_dtype),
                            },
                            "transform": list(subset_transform.to_gdal()),
                            "crs": normalized_crs,
                            "original_crs_wkt": source_crs_wkt,
                            "normalized_crs": normalized_crs,
                            "source_nodata": approved_nodata,
                            "source_raster_nodata": source_nodata,
                            "product_specification_nodata": profile.nodata,
                            "source_scale": source_scale,
                            "source_offset": source_offset,
                            "aoi_mask_applied": bool(record.get("aoi_mask_applied")),
                        }))
                        x_values = subset_transform.c + (np.arange(width, dtype="float64") + 0.5) * subset_transform.a
                        y_values = subset_transform.f + (np.arange(height, dtype="float64") + 0.5) * subset_transform.e
                        x_array = zarr_array(group, "x", (width,), (min(CHUNK_EDGE, width),), "<f8", None, {
                            "_ARRAY_DIMENSIONS": ["x"], "axis": "X", "long_name": "native x pixel center",
                            "standard_name": "projection_x_coordinate", "units": "m",
                        })
                        y_array = zarr_array(group, "y", (height,), (min(CHUNK_EDGE, height),), "<f8", None, {
                            "_ARRAY_DIMENSIONS": ["y"], "axis": "Y", "long_name": "native y pixel center",
                            "standard_name": "projection_y_coordinate", "units": "m",
                        })
                        x_array[:] = x_values
                        y_array[:] = y_values
                        time_array = zarr_array(group, "time", (1,), (1,), "<i8", None, {
                            "_ARRAY_DIMENSIONS": ["time"], "standard_name": "time",
                            "units": "nanoseconds since 1970-01-01 00:00:00", "calendar": "proleptic_gregorian",
                        })
                        time_array[:] = [time_ns(record.get("datetime"))]
                        spatial_ref = zarr_array(
                            group, "spatial_ref", (), (), "<i4", None,
                            crs_attributes(approved_crs, subset_transform),
                        )
                        spatial_ref[()] = np.int32(0)
                        shape = (1, height, width)
                        chunks = (1, min(CHUNK_EDGE, height), min(CHUNK_EDGE, width))
                        fill = None if normalized_nodata(approved_nodata) in (None, "NaN") else approved_nodata
                        ancillary = ["valid", "aoi_mask", *[f"quality_{name}" for name in policies]]
                        value_attrs = {
                            "_ARRAY_DIMENSIONS": ["time", "y", "x"],
                            "grid_mapping": "spatial_ref",
                            "ancillary_variables": " ".join(ancillary),
                            "source_nodata": approved_nodata,
                            "product_specification_nodata": profile.nodata,
                            "scale_factor": profile.scale,
                            "add_offset": profile.offset,
                            "units": profile.unit,
                            "long_name": band_name,
                            "calibration_precedence": (
                                "independent product specification checked against TIFF metadata; "
                                "stored values unmodified"
                            ),
                            "quality_policies": policies,
                        }
                        values_out = zarr_array(
                            group, "values", shape, chunks, actual_dtype.newbyteorder("<"),
                            fill, value_attrs,
                        )

                        def mask_attrs(label):
                            return {
                                "_ARRAY_DIMENSIONS": ["time", "y", "x"],
                                "grid_mapping": "spatial_ref",
                                "long_name": label,
                                "flag_values": [0, 1],
                                "flag_meanings": "excluded included",
                            }

                        valid_out = zarr_array(
                            group, "valid", shape, chunks, "|u1", 0,
                            mask_attrs("AOI included and valid under physical and product fill semantics"),
                        )
                        aoi_out = zarr_array(
                            group, "aoi_mask", shape, chunks, "|u1", 0,
                            mask_attrs("AOI pixel center inclusion; source nodata may still be included"),
                        )
                        quality_out = {}
                        for policy in policies:
                            attrs = mask_attrs(policy)
                            attrs.update({
                                "quality_policy": policy,
                                "derivation": (
                                    "independent policy over raw stored samples, intersected with "
                                    "source validity and AOI inclusion"
                                ),
                                "flag_meanings": "rejected accepted",
                            })
                            quality_out[policy] = zarr_array(
                                group, f"quality_{policy}", shape, chunks, "|u1", 0, attrs,
                            )
                        value_hash = hashlib.sha256()
                        valid_hash = hashlib.sha256()
                        aoi_hash = hashlib.sha256()
                        quality_hashes = {name: hashlib.sha256() for name in policies}
                        for top in range(0, height, CHUNK_EDGE):
                            tile_height = min(CHUNK_EDGE, height - top)
                            strip_values = np.empty((tile_height, width), dtype=actual_dtype)
                            strip_valid = np.empty((tile_height, width), dtype="uint8")
                            strip_aoi = np.empty((tile_height, width), dtype="uint8")
                            strip_quality = {
                                name: np.empty((tile_height, width), dtype="uint8")
                                for name in policies
                            }
                            for left in range(0, width, CHUNK_EDGE):
                                tile_width = min(CHUNK_EDGE, width - left)
                                source_window = Window(x + left, y + top, tile_width, tile_height)
                                data = dataset.read(1, window=source_window)
                                logical_reads += 1
                                logical_read_bytes += data.nbytes
                                tile_transform = dataset.window_transform(source_window)
                                if record.get("aoi_mask_applied"):
                                    included = geometry_mask(
                                        [geometry], out_shape=data.shape, transform=tile_transform,
                                        invert=True, all_touched=False,
                                    )
                                else:
                                    included = np.ones(data.shape, dtype=bool)
                                # Match the native reader: excluded values are
                                # replaced only when it has an explicit finite
                                # source nodata.  NaN itself remains invalid.
                                finite_nodata = normalized_nodata(approved_nodata) not in (None, "NaN")
                                if finite_nodata:
                                    data = data.copy()
                                    data[~included] = approved_nodata
                                physical_valid = (
                                    ~np.isnan(data)
                                    if data.dtype.kind == "f"
                                    else np.ones(data.shape, dtype=bool)
                                )
                                if finite_nodata:
                                    physical_valid &= data != approved_nodata
                                product_valid = product_source_validity(
                                    product, band_name, profile, data,
                                )
                                valid = included & physical_valid & product_valid
                                selection = np.s_[0:1, top:top + tile_height, left:left + tile_width]
                                values_out[selection] = data[np.newaxis, ...]
                                valid_bytes = valid.astype("uint8")
                                aoi_bytes = included.astype("uint8")
                                valid_out[selection] = valid_bytes[np.newaxis, ...]
                                aoi_out[selection] = aoi_bytes[np.newaxis, ...]
                                strip_values[:, left:left + tile_width] = data
                                strip_valid[:, left:left + tile_width] = valid_bytes
                                strip_aoi[:, left:left + tile_width] = aoi_bytes
                                for policy in policies:
                                    accepted = quality(policy, data) & valid
                                    accepted_bytes = accepted.astype("uint8")
                                    quality_out[policy][selection] = accepted_bytes[np.newaxis, ...]
                                    strip_quality[policy][:, left:left + tile_width] = accepted_bytes
                                samples += data.size
                            value_hash.update(np.ascontiguousarray(
                                strip_values.astype(actual_dtype.newbyteorder("<"), copy=False)
                            ).tobytes())
                            valid_hash.update(strip_valid.tobytes())
                            aoi_hash.update(strip_aoi.tobytes())
                            for policy in policies:
                                quality_hashes[policy].update(strip_quality[policy].tobytes())
                        result_outputs.append({
                            "group": record["group"],
                            "asset_id": record["asset_id"],
                            "observation_id": observation["id"],
                            "band": band_name,
                            "window": window,
                            "dtype": str(actual_dtype),
                            "shape": [1, height, width],
                            "crs": normalized_crs,
                            "original_crs_wkt": source_crs_wkt,
                            "transform": list(subset_transform.to_gdal()),
                            "nodata": approved_nodata,
                            "source_raster_nodata": source_nodata,
                            "scale": profile.scale,
                            "offset": profile.offset,
                            "unit": profile.unit,
                            "values_sha256": value_hash.hexdigest(),
                            "valid_sha256": valid_hash.hexdigest(),
                            "aoi_mask_sha256": aoi_hash.hexdigest(),
                            "quality_sha256": {name: value.hexdigest() for name, value in quality_hashes.items()},
                        })
                after = head(href, etag)
                if after != before:
                    raise ValueError("loopback object identity changed during reference read")
            observed_http = curl_counts(curl_stream)
        write_seconds = time.monotonic() - write_started
        finalize_started = time.monotonic()
        root.attrs["status"] = "complete"
        manifest = {
            "schema_version": 1,
            "document_type": "independent_wave1_reference",
            "status": "complete",
            "format": "zarr_v2",
            "writer": f"zarr-python {zarr.__version__}",
            "request_sha256": digest(request_path),
            "native_approval_manifest_sha256": digest(native_path),
            "sources": identities,
            "outputs": result_outputs,
            "processing": [
                "native stored typed values; calibration is metadata only",
                "native-approved source windows and group order",
                "no reprojection, resampling, mosaicking, or implicit alignment",
                "quality masks independently implemented from product specifications",
            ],
            "metrics": {
                "setup_seconds": setup_seconds,
                "write_seconds": write_seconds,
                "source_reads": logical_reads,
                "source_read_calls": logical_reads,
                "source_decoded_bytes": logical_read_bytes,
                "source_identity_heads": 2 * len(result_outputs),
                **observed_http,
            },
        }
        (stage / "manifest.json").write_text(json.dumps(json_safe(manifest), indent=2, allow_nan=False) + "\n")
        fsync_tree(stage)
        if destination.exists() or destination.is_symlink():
            raise FileExistsError("reference destination appeared before publication")
        os.rename(stage, destination)
        parent_fd = os.open(destination.parent, os.O_RDONLY)
        try:
            os.fsync(parent_fd)
        finally:
            os.close(parent_fd)
        finalize_seconds = time.monotonic() - finalize_started
    except BaseException:
        shutil.rmtree(stage, ignore_errors=True)
        raise
    output_files = [path for path in destination.rglob("*") if path.is_file()]
    return {
        "status": "complete",
        "format": "zarr_v2",
        "output": str(destination),
        "observations": len(result_outputs),
        "samples": samples,
        "source_read_calls": logical_reads,
        "source_reads": logical_reads,
        "source_decoded_bytes": logical_read_bytes,
        "source_identity_heads": 2 * len(result_outputs),
        **observed_http,
        "output_objects": len(output_files),
        "output_bytes": sum(path.stat().st_size for path in output_files),
        "setup_seconds": setup_seconds,
        "write_seconds": write_seconds,
        "finalize_seconds": finalize_seconds,
        "wall_seconds": time.monotonic() - started,
        "manifest": str(destination / "manifest.json"),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("request", type=Path)
    parser.add_argument("native_manifest", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    result = build(args.request, args.native_manifest, args.output)
    print(json.dumps(json_safe(result), separators=(",", ":"), allow_nan=False))


if __name__ == "__main__":
    main()
