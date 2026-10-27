#!/usr/bin/env python3
"""Bounded independent qualification for the real southern DIST-ANN-HLS tile.

This is intentionally a separate harness from the conventional-northern
specimen.  The producer stores 20LRP in EPSG:32620 with a negative northing;
that unusual but observed native convention is an input contract, not a CRS
normalisation opportunity.
"""
from __future__ import annotations

import argparse
import ast
import hashlib
import json
import stat
import sys
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urljoin, urlsplit

import numpy as np
import requests
import tifffile

import opera_dist_ann_qualification as shared

ROOT = Path(__file__).resolve().parents[1]
EXAMPLE = ROOT / "examples/opera-dist-ann-hls-south.json"
WORK = ROOT / "prototype-output/dist-ann-southern"
DOCS = ROOT / "docs/products/dist-ann-southern"
SOURCE_RECEIPT = DOCS / "source-receipt.json"
QUALIFICATION_RECEIPT = DOCS / "qualification-receipt.json"
CMR_RECEIPT = DOCS / "cmr-receipt.json"
ARCHIVE = shared.ARCHIVE
URS = shared.URS
RESOURCE = shared.RESOURCE
MAX_SOURCE_BYTES = shared.MAX_SOURCE_BYTES


class QualificationError(RuntimeError):
    pass


def require(value, message):
    if not value:
        raise QualificationError(message)


def dump(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + "\n")


def clean(value):
    return shared.clean(value)


def origin(url):
    parsed = urlsplit(url)
    return f"{parsed.scheme}://{parsed.netloc}"


def credentials(path):
    mode = path.stat().st_mode
    require(stat.S_ISREG(mode) and mode & 0o077 == 0, "explicit netrc must be a regular mode-0600 file")
    return shared.private_credentials(path)


def discover(netrc_path):
    """Fetch the exact public CMR identity and retain no authentication data."""
    query = (
        "https://cmr.earthdata.nasa.gov/search/granules.umm_json?"
        "collection_concept_id=C2519119034-LPCLOUD&"
        "granule_ur=OPERA_L3_DIST-ANN-HLS_T20LRP_2025_20260109T150354Z_30_v1"
    )
    legacy_query = (
        "https://cmr.earthdata.nasa.gov/search/granules.json?"
        "collection_concept_id=C2519119034-LPCLOUD&"
        "granule_ur=OPERA_L3_DIST-ANN-HLS_T20LRP_2025_20260109T150354Z_30_v1"
    )
    session = shared.http_session()
    try:
        response = session.get(query, timeout=(15, 60))
        require(response.status_code == 200, f"CMR returned HTTP {response.status_code}")
        items = response.json().get("items", [])
        legacy_response = session.get(legacy_query, timeout=(15, 60))
        require(legacy_response.status_code == 200, f"CMR granules.json returned HTTP {legacy_response.status_code}")
        legacy_entries = legacy_response.json().get("feed", {}).get("entry", [])
    finally:
        session.close()
    require(len(items) == 1, "CMR did not return exactly the pinned southern granule")
    require(len(legacy_entries) == 1 and legacy_entries[0]["id"] == "G3953708293-LPCLOUD", "CMR granules.json identity changed")
    item = items[0]
    umm = item["umm"]
    meta = item["meta"]
    begin = umm["TemporalExtent"]["RangeDateTime"]["BeginningDateTime"]
    end = umm["TemporalExtent"]["RangeDateTime"]["EndingDateTime"]
    require(meta["concept-id"] == "G3953708293-LPCLOUD", "CMR concept changed")
    require(begin == "2025-01-03T14:02:38.837653Z" and end == "2025-12-29T14:25:53.638110Z", "CMR coverage endpoints changed")
    legacy_start, legacy_end = legacy_entries[0]["time_start"], legacy_entries[0]["time_end"]
    require(legacy_start == "2025-01-03T14:02:38.837Z" and legacy_end == "2025-12-29T14:25:53.638Z", "CMR granules.json coverage endpoints changed")
    urls = [row["URL"] for row in umm["RelatedUrls"] if row.get("Type") == "GET DATA" and row["URL"].endswith(".tif")]
    require(len(urls) == 21, "CMR did not advertise exactly 21 TIFF layers")
    assets = {}
    stem = meta["native-id"] + "_"
    for href in urls:
        name = Path(urlsplit(href).path).stem
        require(name.startswith(stem), "CMR data name differs from granule stem")
        assets[name.removeprefix(stem)] = {"href": href, "metadata": {}}
    require(len(assets) == 21, "CMR layer names are duplicated")
    cmr = {
        "schema": "tile-ripper-dist-ann-southern-cmr-receipt-v1",
        "captured_utc": datetime.now(timezone.utc).isoformat(),
        "status": "fresh_exact_granule_record",
        "collection": {"concept_id": meta["collection-concept-id"], "short_name": umm["CollectionReference"]["ShortName"], "version": umm["CollectionReference"]["Version"]},
        "granule": {"concept_id": meta["concept-id"], "id": meta["native-id"], "revision_id": meta["revision-id"], "revision_date": meta["revision-date"]},
        "temporal_extent": {"coverage_start": legacy_start, "coverage_end": legacy_end, "summary_year": 2025, "authority": "CMR granules.json exact discovery representation"},
        "umm_temporal_extent": {"coverage_start": begin, "coverage_end": end, "precision_note": "UMM retains richer provider microseconds; it is recorded separately and is not silently substituted for the granules.json selection authority."},
        "spatial_coverage": umm["SpatialExtent"]["HorizontalSpatialDomain"]["Geometry"],
        "catalog_grid_attributes_non_authoritative": {
            "observed": {row["Name"]: row["Values"] for row in umm["AdditionalAttributes"] if row["Name"] in {"MGRS_TILE_ID", "ULX", "ULY", "HORIZONTAL_CS_CODE", "HORIZONTAL_CS_NAME"}},
            "contradiction": "CMR text says UTM ZONE 21 despite MGRS tile 20LRP and every inspected GeoTIFF ProjectedCSTypeGeoKey=32620.",
            "coordinate_authority": "The inspected named GeoTIFF grid (GeoKeys and affine), together with the MGRS tile identity, governs native coordinates. Catalog grid text is retained as evidence only and cannot relabel the source.",
        },
        "asset_names": sorted(assets),
    }
    dump(CMR_RECEIPT, cmr)
    return assets, cmr


def probe_asset(asset, auth):
    """Resolve canonical redirects and observe immutable object framing."""
    session = shared.http_session()
    current = asset["href"]
    trace = []
    try:
        for _ in range(10):
            src = origin(current)
            require(urlsplit(current).scheme == "https" and src in {ARCHIVE, URS, RESOURCE}, "source redirect used unadmitted origin")
            response = session.get(current, headers={"Range": "bytes=0-0"} if src != URS else {}, auth=auth if src == URS else None, allow_redirects=False, stream=True, timeout=(15, 60))
            if response.status_code in (301, 302, 303, 307, 308):
                target = urljoin(current, response.headers.get("Location", ""))
                dst = origin(target)
                require(urlsplit(target).scheme == "https" and dst in {ARCHIVE, URS, RESOURCE}, "source redirect target used unadmitted origin")
                if src == URS:
                    require(dst == ARCHIVE, "URS did not redirect back to LP DAAC")
                elif src == RESOURCE:
                    raise QualificationError("resource host unexpectedly redirected")
                trace.append({"from": src, "to": dst, "status": response.status_code})
                response.close()
                current = target
                continue
            require(response.status_code == 206, f"source probe returned HTTP {response.status_code}")
            length = int(response.headers["Content-Range"].split("/")[-1])
            etag = response.headers.get("ETag")
            require(etag and len(response.raw.read(2)) == 1, "source probe body or ETag changed")
            response.close()
            return {"etag": etag, "size_bytes": length}, trace
        raise QualificationError("source redirect budget exceeded")
    finally:
        session.close()


def geokeys(page):
    keys = page.geotiff_tags
    result = {}
    for key, value in keys.items():
        if not key.endswith("GeoKey"):
            continue
        try:
            result[str(key)] = int(value)
        except (TypeError, ValueError):
            result[str(key)] = str(value)
    return result


def header_and_sample(name, asset, auth):
    identity, trace = probe_asset(asset, auth)
    remote = shared.RangeFile(asset, identity, auth)
    try:
        image = tifffile.TiffFile(remote)
        page = image.pages[0]
        tags = {tag.name: str(tag.value) for tag in page.tags.values() if tag.name in {"GDAL_NODATA", "GDAL_METADATA", "GeoAsciiParamsTag", "GeoKeyDirectoryTag", "ModelTiepointTag", "ModelPixelScaleTag"}}
        keys = geokeys(page)
        epsg = keys.get("ProjectedCSTypeGeoKey")
        require(str(epsg) in {"32620", "EPSG:32620"}, f"{name}: GeoKeys do not identify EPSG:32620")
        scale = page.geotiff_tags["ModelPixelScale"]
        tie = page.geotiff_tags["ModelTiepoint"]
        transform = [float(scale[0]), 0.0, float(tie[3]), 0.0, -float(scale[1]), float(tie[4])]
        require(transform == [30.0, 0.0, 799980.0, 0.0, -30.0, -1099980.0], f"{name}: native negative-northing affine changed")
        nodata = np.asarray(page.tags[42113].value, dtype=page.dtype).item()
        header = {
            **identity,
            "shape": [page.imagelength, page.imagewidth],
            "dtype": str(page.dtype),
            "nodata": nodata,
            "block": [page.tilewidth, page.tilelength],
            "compression": str(page.compression).lower().removeprefix("compression."),
            "crs": "EPSG:32620",
            "transform": transform,
            "geokeys": keys,
            "source_geotiff_tags": tags,
            "redirects": trace,
        }
    finally:
        remote.close()
    sample, source_io = shared.source_window(asset, header, {"x": 0, "y": 0, "width": 16, "height": 16}, auth)
    sample_dir = WORK / "source-samples"
    sample_dir.mkdir(parents=True, exist_ok=True)
    np.save(sample_dir / f"{name}.npy", sample, allow_pickle=False)
    return header | {
        "sample_window": [0, 0, 16, 16],
        "sample_values_sha256_le": hashlib.sha256(np.ascontiguousarray(sample).tobytes()).hexdigest(),
        "sample_counts": {str(int(value)): int(count) for value, count in zip(*np.unique(sample, return_counts=True))},
        "header_io": {"bytes": remote.bytes, "ranges": len(remote.events)},
        "sample_io": source_io,
    }


def capture(netrc_path):
    auth = credentials(netrc_path)
    assets, cmr = discover(netrc_path)
    sources = {}
    for name in sorted(assets):
        sources[name] = header_and_sample(name, assets[name], auth)
        assets[name]["etag"] = sources[name]["etag"]
    receipt = {
        "schema": "tile-ripper-dist-ann-southern-source-receipt-v1",
        "captured_utc": datetime.now(timezone.utc).isoformat(),
        "status": "authenticated_bounded_header_and_16x16_sample_inspection",
        "scope": "Exact-object LP DAAC headers, native GeoTIFF keys, and bounded samples. No credentials, cookies, or signed URLs retained.",
        "granule": cmr["granule"]["id"],
        "coverage_start": cmr["temporal_extent"]["coverage_start"],
        "coverage_end": cmr["temporal_extent"]["coverage_end"],
        "native_contract": {"crs": "EPSG:32620", "transform": [30.0, 0.0, 799980.0, 0.0, -30.0, -1099980.0], "false_northing": 0.0, "hemisphere_normalization": "prohibited"},
        "sources": sources,
    }
    dump(SOURCE_RECEIPT, clean(receipt))
    ordered = [name for name in json.loads((ROOT / "examples/opera-dist-ann-hls.json").read_text())["bands"]]
    require(set(ordered) == set(assets), "southern CMR layer set differs from public profile")
    request = {
        "product": "opera-dist-ann-hls",
        "provider": "earthdata",
        "aoi": {"kind": "bounds", "crs": "EPSG:4326", "bbox": [-60.252, -10.025, -60.244, -10.017]},
        "start": cmr["temporal_extent"]["coverage_start"],
        "end": cmr["temporal_extent"]["coverage_end"],
        "bands": ordered,
        "quality_policies": ["dist_ann_veg_confirmed", "dist_ann_gen_current_year_confirmed"],
        "max_observations": 1,
        "observations": [{"id": cmr["granule"]["id"], "datetime": cmr["temporal_extent"]["coverage_start"], "collection": cmr["collection"]["short_name"], "processing": "L3", "properties": {"temporal_extent": {key: cmr["temporal_extent"][key] for key in ("coverage_start", "coverage_end", "summary_year")}, "cmr_temporal_extent_authority": cmr["temporal_extent"]["authority"], "cmr_umm_temporal_extent": cmr["umm_temporal_extent"], "cmr_granule_concept_id": cmr["granule"]["concept_id"]}, "assets": {name: assets[name] for name in ordered}}],
    }
    dump(EXAMPLE, request)
    return request


def assert_native_contract(headers):
    expected = [30.0, 0.0, 799980.0, 0.0, -30.0, -1099980.0]
    require(len(headers) == 21, "source receipt must bind all 21 layers")
    for name, header in headers.items():
        require(header["crs"] == "EPSG:32620", f"{name}: delivered CRS is not EPSG:32620")
        require(header["transform"] == expected, f"{name}: delivered native affine differs")
        require(str(header["geokeys"].get("ProjectedCSTypeGeoKey")) in {"32620", "EPSG:32620"}, f"{name}: original projected GeoKey differs")
        require("32720" not in json.dumps(header["geokeys"]), f"{name}: southern EPSG remapping is prohibited")


def assert_explicit_native_metadata(output_dir, headers):
    """Check CF spatial_ref and retained raw GeoTIFF identity without refetching."""
    manifest = json.loads((output_dir / "manifest.json").read_text())
    sources = {source["asset_id"]: source for source in manifest["sources"]}
    root = shared.zarr.open_group(str(output_dir), mode="r")
    checked = []
    for entry in manifest["outputs"]:
        band = entry["asset_id"].removeprefix("p000_")
        group = root[entry["group"]]
        spatial = group["spatial_ref"].attrs
        require(spatial["epsg_code"] == "EPSG:32620", f"{band}: spatial_ref epsg_code changed")
        require(float(spatial["false_northing"]) == 0.0, f"{band}: spatial_ref false_northing changed")
        require(np.all(group["y"][:] < 0), f"{band}: output y coordinates are not all negative")
        source = sources[entry["asset_id"]]
        raw = source["object_identities"]["native_cog"]
        captured = headers[band]["source_geotiff_tags"]
        require(raw["normalized_crs"] == "EPSG:32620", f"{band}: retained source CRS changed")
        require(raw["geokey_directory"] == list(ast.literal_eval(captured["GeoKeyDirectoryTag"])), f"{band}: retained GeoKey directory differs from captured TIFF")
        require(raw["model_tiepoint"] == list(ast.literal_eval(captured["ModelTiepointTag"])), f"{band}: retained model tiepoint differs from captured TIFF")
        convention = source["native_coordinate_convention"]
        require(convention["coordinate_shift_m"] == 0 and convention["data_reprojection"] is False and convention["crs_relabeling"] is False, f"{band}: source coordinate convention was changed")
        checked.append({"band": band, "spatial_ref_epsg_code": spatial["epsg_code"], "spatial_ref_false_northing": float(spatial["false_northing"]), "all_native_y_coordinates_negative": True, "source_geokey_directory_matches_captured_tiff": True, "source_model_tiepoint_matches_captured_tiff": True, "native_coordinate_shift_m": 0, "data_reprojection": False, "crs_relabeling": False})
    return checked


def assert_last_date_metadata(output_dir, request):
    """Prove producer LAST-DATE update lineage is retained, not conflated with CMR."""
    manifest = json.loads((output_dir / "manifest.json").read_text())
    sources = {source["asset_id"]: source for source in manifest["sources"]}
    root = shared.zarr.open_group(str(output_dir), mode="r")
    catalog_end = request["observations"][0]["properties"]["temporal_extent"]["coverage_end"]
    expected_update = "2025-12-29T14:15:39.774933Z"
    checked = []
    for band, expected_description in {
        "VEG-LAST-DATE": "Day_of_last_land_observation_for_vegetation_disturbance_detection",
        "GEN-LAST-DATE": "Day_of_last_land_observation_for_generic_disturbance_detection",
    }.items():
        asset_id = f"p000_{band}"
        provenance = sources[asset_id].get("annual_last_date_temporal_provenance")
        require(provenance is not None, f"{band}: missing retained annual last-date temporal provenance")
        require(provenance["tiff_update_timestamp"] == expected_update, f"{band}: source TIFF Update_Date changed")
        require(provenance["catalog_coverage_interval"]["coverage_end"] == catalog_end, f"{band}: catalog coverage provenance changed")
        require(provenance["tiff_update_timestamp"] != catalog_end, f"{band}: distinct TIFF and catalog times were conflated")
        require(provenance["tiff_layer_description"] == expected_description, f"{band}: producer layer description changed")
        require(provenance["tiff_input_dist_alert_granule"].endswith("T20LRP_20251228T140910Z_20251230T014403Z_L9_30_v1"), f"{band}: producer alert lineage changed")
        require("not equated" in provenance["temporal_relation"], f"{band}: provenance does not describe the deliberate non-equality")
        group = next(entry["group"] for entry in manifest["outputs"] if entry["asset_id"] == asset_id)
        require(root[group].attrs["source"]["annual_last_date_temporal_provenance"] == provenance, f"{band}: durable source provenance changed")
        checked.append({"band": band, "tiff_update_timestamp": expected_update, "catalog_coverage_end": catalog_end, "source_update_is_distinct_from_catalog_coverage_end": True, "tiff_input_dist_alert_granule": provenance["tiff_input_dist_alert_granule"]})
    return checked


def qualify(binary, netrc_path, replay_output=None, output=None):
    require(binary.is_file(), "--binary must name an executable file")
    binary = binary.resolve()
    auth = credentials(netrc_path)
    request = json.loads(EXAMPLE.read_text())
    headers = json.loads(SOURCE_RECEIPT.read_text())["sources"]
    assert_native_contract(headers)
    request["buffer_bytes"] = 128 * 1024 * 1024
    request["chunk_edge"] = 64
    for band in request["bands"]:
        require(request["observations"][0]["assets"][band]["etag"] == headers[band]["etag"], f"{band}: example pin differs from header receipt")
    shared.EXAMPLE = EXAMPLE
    shared.WORK = WORK
    shared.SOURCE_RECEIPT = SOURCE_RECEIPT
    shared.DOC_RECEIPT = QUALIFICATION_RECEIPT
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    chosen = replay_output or output or WORK / f"qualified-all21-{stamp}.zarr"
    original_inspect = shared.inspect_case
    def southern_inspect(*args, **kwargs):
        report = original_inspect(*args, **kwargs)
        for layer in report["layers"]:
            require(headers[layer["band"]]["transform"][5] < 0, f"{layer['band']}: negative native y was not retained")
        output_dir = args[3]
        report["southern_last_date_temporal_provenance"] = assert_last_date_metadata(output_dir, request)
        report["explicit_native_coordinate_metadata"] = assert_explicit_native_metadata(output_dir, headers)
        return report
    shared.inspect_case = southern_inspect
    report = {"schema": "tile-ripper-dist-ann-southern-qualification-v1", "captured_utc": datetime.now(timezone.utc).isoformat(), "status": "incomplete", "granule": request["observations"][0]["id"], "scope": "Fresh southern EPSG:32620 annual specimen: all 21 independent source windows, durable Zarr, raw values/masks/coordinates/time bounds, and stream completion. Native negative northing is preserved without a 10 Mm shift or EPSG:32720 relabel.", "source_header_receipt": str(SOURCE_RECEIPT.relative_to(ROOT)), "cmr_receipt": str(CMR_RECEIPT.relative_to(ROOT)), "caps": {"independent_source_bytes_per_tiff": shared.MAX_SOURCE_BYTES, "independent_source_range_bytes": shared.MAX_RANGE, "buffer_bytes": request["buffer_bytes"], "chunk_edge": request["chunk_edge"]}, "native_contract": {"crs": "EPSG:32620", "transform": headers[request["bands"][0]]["transform"], "false_northing": 0.0, "hemisphere_normalization": "prohibited"}, "binary": {"path": str(binary.relative_to(ROOT)), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}, "cases": []}
    try:
        case = southern_inspect(binary, netrc_path, request, chosen, "all21", auth, headers, replay=replay_output is not None)
        report["cases"].append(case)
        report["status"] = "passed"
    except Exception as exc:
        report["failure"] = str(exc)
        dump(QUALIFICATION_RECEIPT, clean(report))
        raise
    dump(QUALIFICATION_RECEIPT, clean(report))
    print(json.dumps(clean(report), indent=2, sort_keys=True))


def posthoc(output_dir):
    """Append metadata-only assertions to a passed receipt; performs no source I/O."""
    headers = json.loads(SOURCE_RECEIPT.read_text())["sources"]
    assert_native_contract(headers)
    report = json.loads(QUALIFICATION_RECEIPT.read_text())
    require(report.get("status") == "passed", "posthoc metadata checks require a passed qualification")
    report["explicit_native_coordinate_metadata"] = assert_explicit_native_metadata(output_dir, headers)
    report["southern_last_date_temporal_provenance"] = assert_last_date_metadata(output_dir, json.loads(EXAMPLE.read_text()))
    report["posthoc_native_metadata_checked_utc"] = datetime.now(timezone.utc).isoformat()
    dump(QUALIFICATION_RECEIPT, clean(report))
    print(json.dumps({"status": "passed", "checked_layers": len(report["explicit_native_coordinate_metadata"])}, sort_keys=True))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--netrc", type=Path, required=True)
    parser.add_argument("--capture", action="store_true")
    parser.add_argument("--qualify", action="store_true")
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--replay-output", type=Path)
    parser.add_argument("--posthoc-output", type=Path, help="append source-free native-coordinate metadata assertions to the passed receipt")
    args = parser.parse_args()
    require(args.capture or args.qualify or args.posthoc_output, "choose --capture, --qualify, or --posthoc-output")
    if args.capture:
        capture(args.netrc)
    if args.qualify:
        require(args.binary is not None, "--qualify requires --binary")
        require(not (args.output and args.replay_output), "--output and --replay-output are mutually exclusive")
        qualify(args.binary, args.netrc, args.replay_output, args.output)
    if args.posthoc_output:
        require(not args.capture and not args.qualify, "--posthoc-output is standalone")
        posthoc(args.posthoc_output)


if __name__ == "__main__":
    try:
        main()
    except QualificationError as error:
        raise SystemExit(f"qualification failed: {error}")
