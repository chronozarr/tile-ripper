#!/usr/bin/env python3
"""Replay the bounded, authenticated DIST-ALERT-S1 v1 source capture.

This intentionally has no Earthdata login implementation.  Protected-object
authentication is an exact redirect-policy concern owned by Tile-Ripper's
reviewed Rust client; copying it here would create a second credential path.
The capture directory is supplied explicitly, never discovered.  This replay
checks every admitted layer with rasterio, and cross-checks four representative
raw windows with the separate ``gdal_translate`` executable.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import math
import subprocess
import tempfile
from pathlib import Path

import numpy as np
import rasterio
import zarr
from affine import Affine
from rasterio.features import geometry_mask
from rasterio.warp import transform_geom
from rasterio.windows import Window


ROOT = Path(__file__).resolve().parents[1]
RECEIPT = ROOT / "docs/products/opera-expansion/dist-s1-cmr-receipt.json"
PREFIX = "OPERA_L3_DIST-ALERT-S1_T15UWR_20260228T235832Z_20260714T134753Z_S1C_30_v1.0"
WINDOW = Window(3114, 3550, 32, 32)
REPRESENTATIVE = {"GEN-DIST-STATUS", "GEN-METRIC", "GEN-DIST-DATE", "GEN-DIST-CONF"}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def digest(values: np.ndarray) -> str:
    return hashlib.sha256(values.tobytes()).hexdigest()


def require_bits(actual: np.ndarray, expected: np.ndarray, message: str) -> None:
    """Compare stored samples, not numeric values.

    In particular, this preserves Float32 NaN payload bits instead of treating
    all NaNs as equivalent.
    """
    require(actual.dtype == expected.dtype and actual.shape == expected.shape, f"{message}: layout differs")
    require(actual.tobytes() == expected.tobytes(), message)


def same_fill(observed: float | None, expected: object) -> bool:
    if expected == "NaN":
        return observed is not None and math.isnan(observed)
    return observed == expected


def gdal_raw(path: Path, layer: str) -> bytes:
    # ENVI's data file is the unmodified native little-endian samples; its
    # header stays separate, so this is a raw-byte comparison rather than a
    # GTiff re-encoding comparison.
    with tempfile.TemporaryDirectory(prefix="tile-ripper-dist-s1-") as directory:
        output = Path(directory) / layer
        subprocess.run(
            [
                "gdal_translate",
                "-q",
                "-of",
                "ENVI",
                "-srcwin",
                str(int(WINDOW.col_off)),
                str(int(WINDOW.row_off)),
                str(int(WINDOW.width)),
                str(int(WINDOW.height)),
                str(path),
                str(output),
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        return output.read_bytes()


def replay(capture_dir: Path) -> dict:
    receipt = json.loads(RECEIPT.read_text())
    shared = receipt["shared_native_contract"]
    checked: dict[str, dict] = {}
    for layer, expected in receipt["layers"].items():
        path = capture_dir / f"{PREFIX}_{layer}.tif"
        require(path.is_file(), f"missing captured {layer}")
        require(path.stat().st_size == expected["bytes"], f"{layer}: byte size changed")
        require(path.stat().st_size <= receipt["capture"]["per_object_cap_bytes"], f"{layer}: exceeds capture cap")
        with rasterio.open(path) as dataset:
            require(dataset.count == 1, f"{layer}: expected one band")
            require(dataset.width == shared["shape"][1] and dataset.height == shared["shape"][0], f"{layer}: shape changed")
            require(str(dataset.crs) == shared["crs"], f"{layer}: CRS changed")
            require(list(dataset.transform.to_gdal()) == shared["transform"], f"{layer}: transform changed")
            require(list(dataset.block_shapes[0]) == shared["block"], f"{layer}: tile shape changed")
            require(str(dataset.compression).lower().endswith("zstd"), f"{layer}: expected ZSTD COG")
            require(dataset.dtypes[0] == expected["dtype"], f"{layer}: dtype changed")
            require(same_fill(dataset.nodata, expected["fill"]), f"{layer}: nodata changed")
            values = dataset.read(1, window=WINDOW)
            require(digest(values) == expected["window_raw_sha256"], f"{layer}: oracle window changed")
            valid = ~np.isnan(values) if np.issubdtype(values.dtype, np.floating) else values != dataset.nodata
            require(int(valid.sum()) == 1024, f"{layer}: oracle window is not fully source-valid")
            checked[layer] = {"raw_sha256": digest(values), "valid_pixels": int(valid.sum())}
            if layer in REPRESENTATIVE:
                require(gdal_raw(path, layer) == values.tobytes(), f"{layer}: rasterio/GDAL raw bytes differ")
    return {"layers": checked, "representative_gdal_raw_match": sorted(REPRESENTATIVE)}


def requested_aoi_mask(request: dict, crs: str, transform: Affine, shape: tuple[int, int]) -> np.ndarray:
    aoi = request.get("aoi")
    require(isinstance(aoi, dict) and aoi.get("kind") == "bounds", "replay supports saved bounds AOIs")
    require(aoi.get("crs") == "EPSG:4326", "replay requires an EPSG:4326 saved AOI")
    bounds = aoi.get("bbox")
    require(isinstance(bounds, list) and len(bounds) == 4, "saved AOI bounds are malformed")
    west, south, east, north = bounds
    geometry = {
        "type": "Polygon",
        "coordinates": [[[west, south], [east, south], [east, north], [west, north], [west, south]]],
    }
    native_geometry = transform_geom("EPSG:4326", crs, geometry, precision=-1)
    return ~geometry_mask([native_geometry], out_shape=shape, transform=transform, all_touched=False)


def expected_coordinates(transform: Affine, shape: tuple[int, int]) -> tuple[np.ndarray, np.ndarray]:
    require(transform.b == 0.0 and transform.d == 0.0, "qualification requires a north-up native grid")
    height, width = shape
    x = transform.c + transform.a * (np.arange(width, dtype=np.float64) + 0.5)
    y = transform.f + transform.e * (np.arange(height, dtype=np.float64) + 0.5)
    return x, y


def dtype_for_stream(value: str) -> np.dtype:
    return {
        "uint8": np.dtype("u1"),
        "int16": np.dtype("<i2"),
        "float32": np.dtype("<f4"),
    }.get(value, np.dtype("V0"))


def stream_chunks(stream_path: Path) -> tuple[dict[str, dict], dict]:
    chunks: dict[str, dict] = {}
    complete: dict | None = None
    for line in stream_path.read_text().splitlines():
        event = json.loads(line)
        if event.get("event") == "chunk":
            layer = event.get("band")
            require(isinstance(layer, str) and layer not in chunks, "stream contains duplicate or unnamed layer")
            require("values_le_base64" in event, "stream capture lacks --values base64 samples")
            chunks[layer] = event
        elif event.get("event") == "complete":
            require(complete is None, "stream has multiple completion records")
            complete = event
    require(complete is not None and complete.get("source_versions_validated") is True, "stream did not complete with source versions validated")
    return chunks, complete


def replay_zarr(capture_dir: Path, zarr_dir: Path, request_path: Path, stream_path: Path) -> dict:
    """Reopen a Tile-Ripper Zarr result and compare delivered native pixels.

    Values outside the AOI are deliberately not used as an equality oracle:
    native source values and fills are preserved separately from the explicit
    `aoi_mask`.  Within the AOI, all raw samples must match exactly, including
    Float32 values; `valid` must be the intersection of AOI and source-valid.
    """
    root = zarr.open_group(str(zarr_dir), mode="r")
    manifest = json.loads((zarr_dir / "manifest.json").read_text())
    require(manifest.get("status") == "complete", "Zarr manifest is not complete")
    receipt = json.loads(RECEIPT.read_text())
    request = json.loads(request_path.read_text())
    chunks, complete = stream_chunks(stream_path)
    expected_layers = set(receipt["layers"])
    seen: set[str] = set()
    checks: dict[str, dict] = {}
    for index in root["observations"].keys():
        group = root["observations"][index]
        layer = group.attrs["source"]["requested_band"]
        require(layer in expected_layers and layer not in seen, f"unexpected or duplicate Zarr layer {layer}")
        seen.add(layer)
        values = np.asarray(group["values"][0])
        stored_aoi = np.asarray(group["aoi_mask"][0]).astype(bool)
        valid = np.asarray(group["valid"][0]).astype(bool)
        window = group.attrs["source_window"]
        path = capture_dir / f"{PREFIX}_{layer}.tif"
        with rasterio.open(path) as dataset:
            source_window = Window(window["x"], window["y"], window["width"], window["height"])
            source = dataset.read(1, window=source_window)
            native_nodata = dataset.nodata
            source_valid = ~np.isnan(source) if np.issubdtype(source.dtype, np.floating) else source != native_nodata
            native_transform = dataset.transform * Affine.translation(source_window.col_off, source_window.row_off)
            expected_crs = str(dataset.crs)
        expected_x, expected_y = expected_coordinates(native_transform, source.shape)
        expected_aoi = requested_aoi_mask(request, expected_crs, native_transform, source.shape)
        require(values.shape == source.shape, f"{layer}: Zarr/source shape differs")
        require(group.attrs["crs"] == expected_crs, f"{layer}: Zarr CRS differs from native source")
        require(group.attrs["transform"] == list(native_transform.to_gdal()), f"{layer}: Zarr transform differs from native source")
        require(group["spatial_ref"].attrs["crs"] == expected_crs, f"{layer}: spatial_ref CRS differs")
        require_bits(np.asarray(group["x"][:]), expected_x, f"{layer}: x coordinates differ")
        require_bits(np.asarray(group["y"][:]), expected_y, f"{layer}: y coordinates differ")
        require(np.array_equal(stored_aoi, expected_aoi), f"{layer}: AOI mask differs from saved request geometry")
        require_bits(values[expected_aoi], source[expected_aoi], f"{layer}: AOI raw sample bits differ")
        require(np.array_equal(valid, expected_aoi & source_valid), f"{layer}: validity mask differs")
        stream = chunks.get(layer)
        require(stream is not None, f"{layer}: missing value-bearing stream chunk")
        stream_window = stream["window"]
        require(stream_window == window, f"{layer}: stream/Zarr source windows differ")
        require(stream["crs"] == expected_crs and stream["transform"] == list(native_transform.to_gdal()), f"{layer}: stream native grid differs")
        stream_dtype = dtype_for_stream(stream["dtype"])
        require(stream_dtype == source.dtype, f"{layer}: stream dtype differs")
        stream_values = np.frombuffer(base64.b64decode(stream["values_le_base64"]), dtype=stream_dtype).reshape(source.shape)
        stream_aoi = np.frombuffer(base64.b64decode(stream["aoi_mask_base64"]), dtype=np.uint8).astype(bool).reshape(source.shape)
        stream_valid = np.frombuffer(base64.b64decode(stream["source_valid_mask_base64"]), dtype=np.uint8).astype(bool).reshape(source.shape)
        require_bits(stream_values, values, f"{layer}: stream/Zarr raw sample bits differ")
        require(np.array_equal(stream_aoi, expected_aoi), f"{layer}: stream AOI mask differs from saved request geometry")
        stream_source_valid = ~np.isnan(stream_values) if np.issubdtype(stream_values.dtype, np.floating) else stream_values != native_nodata
        require(np.array_equal(stream_valid, stream_source_valid), f"{layer}: stream source-valid mask differs")
        checks[layer] = {"aoi_raw_bits_match": True, "stream_raw_bits_match": True, "aoi_pixels": int(expected_aoi.sum()), "source_valid_pixels": int(valid.sum())}
        if layer == "GEN-DIST-STATUS":
            confirmed = np.asarray(group["quality_dist_s1_confirmed"][0]).astype(bool)
            ongoing = np.asarray(group["quality_dist_s1_confirmed_ongoing"][0]).astype(bool)
            expected_confirmed = expected_aoi & source_valid & np.isin(source, [3, 6, 7, 8])
            expected_ongoing = expected_aoi & source_valid & np.isin(source, [3, 6])
            require(np.array_equal(confirmed, expected_confirmed), "confirmed mask differs")
            require(np.array_equal(ongoing, expected_ongoing), "ongoing mask differs")
            stream_confirmed = np.frombuffer(base64.b64decode(stream["quality"]["dist_s1_confirmed"]["mask_base64"]), dtype=np.uint8).astype(bool).reshape(source.shape)
            stream_ongoing = np.frombuffer(base64.b64decode(stream["quality"]["dist_s1_confirmed_ongoing"]["mask_base64"]), dtype=np.uint8).astype(bool).reshape(source.shape)
            require(np.array_equal(stream_confirmed, expected_confirmed), "stream confirmed mask differs")
            require(np.array_equal(stream_ongoing, expected_ongoing), "stream ongoing mask differs")
            checks[layer]["quality_masks_match"] = True
    require(seen == expected_layers == set(chunks), "Zarr or stream omits an admitted DIST-S1 layer")
    require(complete["observed"].get("chunks") == len(expected_layers), "stream completion chunk count differs")
    return {"zarr_reopened": True, "stream_values_replayed": True, "layers": checks}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--capture-dir", type=Path, required=True, help="explicit bounded-capture directory")
    parser.add_argument("--zarr-dir", type=Path, help="optional completed Tile-Ripper Zarr v2 result to reopen")
    parser.add_argument("--request", type=Path, help="saved request used for AOI and output checks")
    parser.add_argument("--stream-jsonl", type=Path, help="saved live `stream --values` JSONL")
    parser.add_argument("--output", type=Path, help="optional replay receipt path")
    args = parser.parse_args()
    result = replay(args.capture_dir)
    if args.zarr_dir:
        require(args.request is not None and args.stream_jsonl is not None, "Zarr replay requires --request and --stream-jsonl")
        result["zarr"] = replay_zarr(args.capture_dir, args.zarr_dir, args.request, args.stream_jsonl)
    encoded = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.write_text(encoded)
    else:
        print(encoded, end="")


if __name__ == "__main__":
    main()
