"""Freeze bounded real NASA/OPERA COG reads and verify them offline.

The ``capture`` action requires rasterio and performs exact-object HEAD and
If-Match range requests.  It records the ranges that rasterio needed for a
32x32 reference window, then assembles a sparse, full-offset TIFF containing
only those ranges.  The default ``verify`` action is network-free.

Examples:
  python bench/wave1_nasa_probe.py capture
  python bench/wave1_nasa_probe.py verify
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import math
import os
import re
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "prototype-output/wave1/nasa"
MIRRORS = OUT / "range-mirrors"
REFERENCES = OUT / "references"
LOGS = OUT / "logs"
METADATA = OUT / "metadata"
MANIFEST = OUT / "manifest.json"
CHILD = r"""
import base64, hashlib, json, math, sys
import numpy as np
import rasterio
from rasterio.windows import Window

url, x, y, width, height = sys.argv[1], *map(int, sys.argv[2:])
with rasterio.Env(GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", CPL_CURL_VERBOSE="YES"):
    with rasterio.open(url) as dataset:
        array = dataset.read(1, window=Window(x, y, width, height))
        tags = dataset.tags()
        retained_tags = {
            key: tags[key] for key in (
                "Fmask bit description", "Fmask description", "HLS_PROCESSING_TIME",
                "LAYER_DESCRIPTION", "PLATFORM", "PROCESSING_DATETIME",
                "PROCESSING_INFORMATION_FILTERING_APPLIED",
                "PROCESSING_INFORMATION_OUTPUT_BACKSCATTER_EXPRESSION_CONVENTION",
                "PROCESSING_INFORMATION_OUTPUT_BACKSCATTER_NORMALIZATION_CONVENTION",
                "PRODUCT_VERSION", "SENSING_TIME", "SOFTWARE_VERSION", "SPACECRAFT_NAME",
            ) if key in tags
        }
        nodata = dataset.nodata
        valid = np.ones(array.shape, dtype=bool)
        if nodata is not None:
            valid = ~np.isnan(array) if math.isnan(nodata) else array != nodata
        values = array[valid]
        result = {
            "driver": dataset.driver,
            "shape": list(array.shape),
            "source_shape": [dataset.height, dataset.width],
            "dtype": array.dtype.str,
            "nodata": "NaN" if nodata is not None and math.isnan(nodata) else nodata,
            "block_shape": list(dataset.block_shapes[0]),
            "compression": str(dataset.compression).split(".")[-1].lower(),
            "scale": dataset.scales[0],
            "offset": dataset.offsets[0],
            "unit": dataset.units[0],
            "description": dataset.descriptions[0],
            "metadata_tags": retained_tags,
            "crs": dataset.crs.to_string(),
            "transform": list(dataset.transform)[:6],
            "window": [x, y, width, height],
            "valid_count": int(values.size),
            "minimum": float(values.min()) if values.size else None,
            "maximum": float(values.max()) if values.size else None,
            "raw_sha256": hashlib.sha256(array.tobytes(order="C")).hexdigest(),
            "raw_base64": base64.b64encode(array.tobytes(order="C")).decode("ascii"),
        }
print(json.dumps(result, separators=(",", ":")))
"""


@dataclass(frozen=True)
class Asset:
    name: str
    url: str
    size: int
    etag: str
    window: tuple[int, int, int, int]
    role: str


@dataclass(frozen=True)
class Auxiliary:
    name: str
    url: str
    size: int
    etag: str


DSWX = (
    "https://opera-pst-rs-pop1.s3.us-west-2.amazonaws.com/products/DSWx_HLS/"
    "OPERA_L3_DSWx-HLS_T10TCT_20211211T191811Z_20241025T180315Z_S2A_30_v1.0/"
    "OPERA_L3_DSWx-HLS_T10TCT_20211211T191811Z_20241025T180315Z_S2A_30_v1.0"
)
RTC = (
    "https://opera-pst-rs-pop1.s3.us-west-2.amazonaws.com/products/RTC_S1/"
    "OPERA_L2_RTC-S1_T001-000288-IW3_20240110T181405Z_20240410T013737Z_S1A_30_v1.0/"
    "OPERA_L2_RTC-S1_T001-000288-IW3_20240110T181405Z_20240410T013737Z_S1A_30_v1.0"
)
HLS_L30 = (
    "https://hls.gsfc.nasa.gov/data/RSE2.0.data.code/data/"
    "HLS.L30.T15VUE.2022231T172609.v2.0/"
    "HLS.L30.T15VUE.2022231T172609.v2.0"
)
HLS_S30 = (
    "https://hls.gsfc.nasa.gov/data/RSE2.0.data.code/data/"
    "HLS.S30.T17SPV.2022023T160551.v2.0/"
    "HLS.S30.T17SPV.2022023T160551.v2.0"
)

ASSETS = [
    Asset(
        "hls_l30_b04", HLS_L30 + ".B04.tif", 24_519_548,
        '"176237c-5f67b3da224c0"', (1792, 1792, 32, 32), "red surface reflectance",
    ),
    Asset(
        "hls_l30_fmask", HLS_L30 + ".Fmask.tif", 918_102,
        '"e0256-5f67b3dc0a940"', (1792, 1792, 32, 32), "quality bit field",
    ),
    Asset(
        "hls_s30_b04", HLS_S30 + ".B04.tif", 30_351_978,
        '"1cf226a-5f77d81310780"', (1792, 1792, 32, 32), "red surface reflectance",
    ),
    Asset(
        "hls_s30_fmask", HLS_S30 + ".Fmask.tif", 3_333_122,
        '"32dc02-5f77d814f8c00"', (1792, 1792, 32, 32), "quality bit field",
    ),
    Asset(
        "dswx_hls_wtr", DSWX + "_B01_WTR.tif", 230_575,
        '"bb7699fcb8072344e4d4732dc3f96bd8"', (1824, 2560, 32, 32),
        "masked water classification",
    ),
    Asset(
        "dswx_hls_diag", DSWX + "_B04_DIAG.tif", 2_059_342,
        '"6fcdd39e9879aa3b224b36a12f4af6f6"', (1824, 2560, 32, 32),
        "five-test decimal diagnostic code",
    ),
    Asset(
        "rtc_s1_vv", RTC + "_VV.tif", 6_168_401,
        '"f635843d25090993e2c13b8870d4f837"', (1536, 704, 32, 32), "gamma0 power VV",
    ),
    Asset(
        "rtc_s1_vh", RTC + "_VH.tif", 6_401_605,
        '"99706aff792ef99359e31778d7210434"', (1536, 704, 32, 32), "gamma0 power VH",
    ),
    Asset(
        "rtc_s1_mask", RTC + "_mask.tif", 43_901,
        '"378f67e458d82fc604fe1b04e96aa0f5"', (1536, 704, 32, 32),
        "layover/shadow validity classes",
    ),
]

AUXILIARY = [
    Auxiliary("hls_l30_cmr", HLS_L30 + ".cmr.xml", 6_957, '"1b2d-5f67b3dc0a940"'),
    Auxiliary("hls_s30_cmr", HLS_S30 + ".cmr.xml", 8_672, '"21e0-5f77d814f8c00"'),
    Auxiliary("dswx_hls_metadata", DSWX + ".met.json", 73_864, '"6ff81180dcf46b4bca2d64e7305ccbac"'),
    Auxiliary("rtc_s1_metadata", RTC + ".met.json", 25_020, '"36e0789cf7b5a6b3af6a796072cccdf1"'),
    Auxiliary("rtc_s1_hdf5", RTC + ".h5", 101_197, '"0613faeb3bd300dc30979f1c97b1f0a6"'),
]

OBSERVATIONS = [
    {
        "id": "HLS.L30.T15VUE.2022231T172609.v2.0",
        "datetime": "2022-08-19T17:26:09.755564Z",
        "collection": "HLSL30.020",
        "asset_names": {"red": "hls_l30_b04", "quality": "hls_l30_fmask"},
    },
    {
        "id": "HLS.S30.T17SPV.2022023T160551.v2.0",
        "datetime": "2022-01-23T16:13:26.247240Z",
        "collection": "HLSS30.020",
        "asset_names": {"red": "hls_s30_b04", "quality": "hls_s30_fmask"},
    },
    {
        "id": "OPERA_L3_DSWx-HLS_T10TCT_20211211T191811Z_20241025T180315Z_S2A_30_v1.0",
        "datetime": "2021-12-11T19:18:11Z",
        "collection": "OPERA_L3_DSWX-HLS_V1",
        "asset_names": {"water": "dswx_hls_wtr", "diagnostic": "dswx_hls_diag"},
    },
    {
        "id": "OPERA_L2_RTC-S1_T001-000288-IW3_20240110T181405Z_20240410T013737Z_S1A_30_v1.0",
        "datetime": "2024-01-10T18:14:05Z",
        "collection": "OPERA_L2_RTC-S1_V1",
        "asset_names": {"vv": "rtc_s1_vv", "vh": "rtc_s1_vh", "mask": "rtc_s1_mask"},
    },
]


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def request(asset: Asset | Auxiliary, method: str, byte_range: tuple[int, int] | None = None):
    headers = {"Accept-Encoding": "identity", "If-Match": asset.etag}
    if byte_range:
        headers["Range"] = f"bytes={byte_range[0]}-{byte_range[1]}"
    req = urllib.request.Request(asset.url, headers=headers, method=method)
    return urllib.request.urlopen(req, timeout=30)


def identity(asset: Asset | Auxiliary) -> dict:
    with request(asset, "HEAD") as response:
        got_size = int(response.headers["Content-Length"])
        got_etag = response.headers.get("ETag")
        if (got_size, got_etag) != (asset.size, asset.etag):
            raise RuntimeError(f"{asset.name}: object identity changed")
        return {
            "url": asset.url,
            "size": got_size,
            "etag": got_etag,
            "last_modified": response.headers.get("Last-Modified"),
            "accept_ranges": response.headers.get("Accept-Ranges"),
            "content_type": response.headers.get("Content-Type"),
        }


def merge_ranges(ranges: list[tuple[int, int]]) -> list[tuple[int, int]]:
    merged: list[list[int]] = []
    for start, end in sorted(set(ranges)):
        if not merged or start > merged[-1][1] + 1:
            merged.append([start, end])
        else:
            merged[-1][1] = max(merged[-1][1], end)
    return [(a, b) for a, b in merged]


def remote_reference(asset: Asset) -> tuple[dict, list[tuple[int, int]], str]:
    result = subprocess.run(
        [sys.executable, "-c", CHILD, asset.url, *map(str, asset.window)],
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode:
        raise RuntimeError(f"{asset.name}: rasterio remote read failed: {result.stderr.splitlines()[-1]}")
    ranges = merge_ranges(
        [(int(a), int(b)) for a, b in re.findall(r"Range: bytes=(\d+)-(\d+)", result.stderr, re.I)]
    )
    if not ranges:
        raise RuntimeError(f"{asset.name}: rasterio issued no observable range reads")
    return json.loads(result.stdout), ranges, result.stderr


def build_mirror(asset: Asset, ranges: list[tuple[int, int]]) -> tuple[Path, list[dict]]:
    path = MIRRORS / f"{asset.name}.tif"
    with path.open("wb") as stream:
        stream.truncate(asset.size)
    receipts = []
    with path.open("r+b") as stream:
        for start, end in ranges:
            with request(asset, "GET", (start, end)) as response:
                if response.status != 206:
                    raise RuntimeError(f"{asset.name}: expected HTTP 206, got {response.status}")
                body = response.read(end - start + 2)
                if len(body) != end - start + 1:
                    raise RuntimeError(f"{asset.name}: short range body")
                expected = f"bytes {start}-{end}/{asset.size}"
                if response.headers.get("Content-Range") != expected:
                    raise RuntimeError(f"{asset.name}: invalid Content-Range")
                if response.headers.get("ETag") != asset.etag:
                    raise RuntimeError(f"{asset.name}: range identity changed")
            stream.seek(start)
            stream.write(body)
            receipts.append({"start": start, "end": end, "bytes": len(body), "sha256": sha256(body)})
    return path, receipts


def local_reference(path: Path, window: tuple[int, int, int, int]) -> dict:
    result = subprocess.run(
        [sys.executable, "-c", CHILD, str(path), *map(str, window)],
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode:
        raise RuntimeError(f"offline rasterio read failed: {result.stderr.splitlines()[-1]}")
    return json.loads(result.stdout)


def comparable(reference: dict) -> dict:
    return {k: v for k, v in reference.items() if k != "raw_base64"}


def capture() -> None:
    for directory in (MIRRORS, REFERENCES, LOGS, METADATA):
        directory.mkdir(parents=True, exist_ok=True)
    rows = []
    for asset in ASSETS:
        pinned = identity(asset)
        reference, ranges, log = remote_reference(asset)
        mirror, receipts = build_mirror(asset, ranges)
        offline = local_reference(mirror, asset.window)
        if reference != offline:
            raise RuntimeError(f"{asset.name}: sparse mirror differs from live rasterio reference")
        (REFERENCES / f"{asset.name}.json").write_text(json.dumps(reference, indent=2) + "\n")
        # Curl diagnostics can contain ephemeral signed URLs on authenticated providers.
        # These public sources are unsigned, but retain only redacted request facts anyway.
        safe_log = "\n".join(
            line for line in log.splitlines()
            if re.search(r"^(> (HEAD|GET)|> Range:|< HTTP/|< Content-Range:|< Content-Length:|< ETag:)", line, re.I)
        )
        (LOGS / f"{asset.name}.ranges.log").write_text(safe_log + "\n")
        stat = mirror.stat()
        rows.append({
            "name": asset.name,
            "role": asset.role,
            "identity": pinned,
            "reference": comparable(reference),
            "ranges": receipts,
            "source_body_bytes": sum(r["bytes"] for r in receipts),
            "sparse_mirror": str(mirror.relative_to(ROOT)),
            "sparse_logical_bytes": stat.st_size,
            "sparse_allocated_bytes": stat.st_blocks * 512,
        })
        print(f"{asset.name}: {rows[-1]['source_body_bytes']} source bytes in {len(receipts)} ranges")
    row_by_name = {row["name"]: row for row in rows}
    observations = []
    for configured in OBSERVATIONS:
        observation = {
            "id": configured["id"],
            "datetime": configured["datetime"],
            "collection": configured["collection"],
            "platform": None,
            "processing": None,
            "assets": {},
        }
        for canonical_band, name in configured["asset_names"].items():
            row = row_by_name[name]
            observation["assets"][canonical_band] = {
                "href": row["sparse_mirror"],
                "etag": row["identity"]["etag"],
                "metadata": row["reference"],
            }
        observations.append(observation)
    auxiliary_rows = []
    for auxiliary in AUXILIARY:
        pinned = identity(auxiliary)
        with request(auxiliary, "GET") as response:
            body = response.read(auxiliary.size + 1)
        if len(body) != auxiliary.size:
            raise RuntimeError(f"{auxiliary.name}: auxiliary body length changed")
        path = METADATA / Path(urllib.parse.urlsplit(auxiliary.url).path).name
        path.write_bytes(body)
        auxiliary_rows.append({
            "name": auxiliary.name,
            "identity": pinned,
            "local_path": str(path.relative_to(ROOT)),
            "sha256": sha256(body),
        })
    MANIFEST.write_text(json.dumps({
        "schema": "tile-ripper-wave1-nasa-range-mirror-v1",
        "network_free_default": "verify",
        "reference_reader": "rasterio",
        "assets": rows,
        "observations": observations,
        "auxiliary_metadata": auxiliary_rows,
    }, indent=2) + "\n")


def verify() -> None:
    manifest = json.loads(MANIFEST.read_text())
    configured = {asset.name: asset for asset in ASSETS}
    for row in manifest["assets"]:
        asset = configured[row["name"]]
        path = ROOT / row["sparse_mirror"]
        if path.stat().st_size != row["identity"]["size"]:
            raise RuntimeError(f"{asset.name}: sparse mirror size changed")
        with path.open("rb") as stream:
            for receipt in row["ranges"]:
                stream.seek(receipt["start"])
                body = stream.read(receipt["bytes"])
                if sha256(body) != receipt["sha256"]:
                    raise RuntimeError(f"{asset.name}: mirrored range changed")
        expected = json.loads((REFERENCES / f"{asset.name}.json").read_text())
        observed = local_reference(path, asset.window)
        if expected != observed:
            raise RuntimeError(f"{asset.name}: offline reference changed")
        print(f"{asset.name}: PASS")
    for row in manifest.get("auxiliary_metadata", []):
        body = (ROOT / row["local_path"]).read_bytes()
        if len(body) != row["identity"]["size"] or sha256(body) != row["sha256"]:
            raise RuntimeError(f"{row['name']}: frozen metadata changed")
        print(f"{row['name']}: PASS")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", nargs="?", choices=["capture", "verify"], default="verify")
    args = parser.parse_args()
    try:
        __import__("rasterio")
    except ImportError as error:
        raise SystemExit("rasterio is required for independent reference reads") from error
    capture() if args.action == "capture" else verify()


if __name__ == "__main__":
    main()
