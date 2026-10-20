"""Bounded public Sentinel-2 acquisition, independently checked by GDAL/PROJ.

Source arrays stay local; only provenance and compact reference receipts are saved.
"""

import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import urllib.request
import zlib

import numpy as np
import rasterio
from rasterio.features import geometry_mask
from rasterio.warp import transform_geom
from rasterio.windows import Window
from shapely.geometry import shape


ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/native-sentinel/live"
TARGET = Path(os.environ.get("TILE_RIPPER_NATIVE_TARGET", ROOT / "target/native-sentinel/release"))


def head(url):
    with urllib.request.urlopen(urllib.request.Request(url, method="HEAD"), timeout=45) as response:
        return dict(etag=response.headers["ETag"], bytes=int(response.headers["Content-Length"]))


def dense(aoi):
    rings = []
    for ring in aoi["coordinates"]:
        points = []
        for a, b in zip(ring, ring[1:]):
            n = max(1, math.ceil(max(abs(a[0]-b[0]), abs(a[1]-b[1])) / 0.001))
            points.extend([[a[0]+(b[0]-a[0])*i/n, a[1]+(b[1]-a[1])*i/n] for i in range(n)])
        points.append(ring[-1])
        rings.append(points)
    return dict(type="Polygon", coordinates=rings)


def discover(request):
    source = request["source"]
    geometry = dict(type="Polygon", coordinates=request["spatial"]["coordinates"])
    body = dict(collections=[source["collection"]], intersects=geometry,
                datetime=f"{request['time']['start']}/{request['time']['end']}", limit=source["limits"]["page_size"])
    query = urllib.request.Request(source["endpoint"] + "/search", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    items = []
    while query is not None:
        with urllib.request.urlopen(query, timeout=45) as response:
            page = json.load(response)
        items.extend(page["features"])
        assert len(items) <= source["limits"]["max_items"]
        link = next((l for l in page.get("links", []) if l["rel"] == "next"), None)
        if link:
            method = link.get("method", "GET")
            next_body = {**body, **link.get("body", {})} if link.get("merge") else link.get("body", {})
            query = urllib.request.Request(link["href"], method=method,
                data=json.dumps(next_body).encode() if method == "POST" else None,
                headers={"Content-Type": "application/json"})
        else:
            query = None
    items.sort(key=lambda i: i["id"])
    aoi = shape(geometry)
    expected = []
    for item in items:
        if shape(item["geometry"]).intersection(aoi).area <= 0:
            continue
        for band in request["variables"]:
            key = {"B04": "red", "B08": "nir"}[band["name"]]
            expected.append((item["id"], key, item["assets"][key]["href"]))
    return expected


def main():
    import consumer_validation as consumer
    WORK.mkdir(parents=True, exist_ok=True)
    path = ROOT / "docs/native-sentinel/request.json"
    request = json.loads(path.read_text())
    expected_sources = discover(request)
    assert expected_sources
    consumer.BIN = TARGET / "tile-ripper-native-stats"
    result = consumer.invoke(request, WORK / "request.json")
    assert result["code"] == 0, result
    rows = result["rows"]
    assert rows[-1]["type"] == "complete"
    sources = rows[0]["provenance"]["sources"]
    assert [(s["item_id"], s["asset_key"], s["href"]) for s in sources] == expected_sources
    assert all(head(s["href"]) == s["object_identity"] for s in sources)
    prefix = WORK / "exact"
    oracle = TARGET / "examples/native_sentinel_oracle"
    subprocess.run([str(oracle), str(path), str(prefix)], check=True)
    exact = [json.loads(s) for s in prefix.with_suffix(".ndjson").read_text().splitlines()]
    raw = prefix.with_suffix(".u16le").read_bytes()
    assert exact[-1]["type"] == "complete"
    assert len(exact) == len(rows)
    refs = {s["asset_id"]: s for s in sources}
    fields = ["asset_id", "datetime", "variable", "window", "transform", "crs", "dtype", "nodata", "scale", "offset", "valid_pixels", "native_sum", "adler32_le"]
    references = []
    masks = {}
    samples = 0
    with rasterio.Env(GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", GDAL_NUM_THREADS="1", GDAL_CACHEMAX=32*2**20, GDAL_INGESTED_BYTES_AT_OPEN="32768", GDAL_HTTP_TIMEOUT="45"):
        for row, byte_row in zip(rows[1:-1], exact[1:-1]):
            source = refs[row["asset_id"]]
            assert byte_row["asset_id"] == row["asset_id"] and byte_row["window"] == row["window"]
            w = row["window"]
            window = Window(w["x"], w["y"], w["width"], w["height"])
            with rasterio.Env(GDAL_HTTP_HEADERS=f"If-Match: {source['object_identity']['etag']}\r\n"):
                with rasterio.open(source["href"]) as ds:
                    expected = ds.read(1, window=window)
                    transform = ds.window_transform(window)
                    g = transform_geom("EPSG:4326", ds.crs, dense(request["spatial"]))
                    if source["href"] not in masks:
                        masks[source["href"]] = geometry_mask([g], out_shape=(ds.height, ds.width), transform=ds.transform, invert=True)
                    inside = masks[source["href"]][w["y"]:w["y"]+w["height"], w["x"]:w["x"]+w["width"]]
                    expected[~inside] = ds.nodata
                    assert row["transform"] == list(transform.to_gdal())
                    assert row["crs"] == ds.crs.to_string()
                    assert row["nodata"] == ds.nodata
                    # Missing TIFF scale/offset means conventional 1/0; STAC
                    # calibration remains separately preserved in provenance.
                    assert (row["scale"] if row["scale"] is not None else 1) == ds.scales[0]
                    assert (row["offset"] if row["offset"] is not None else 0) == ds.offsets[0]
                    data = expected.astype("<u2").tobytes()
                    begin = byte_row["data_offset_bytes"]
                    assert raw[begin:begin+len(data)] == data
                    assert row["adler32_le"] == zlib.adler32(data)
                    valid = expected != ds.nodata
                    assert row["valid_pixels"] == int(valid.sum())
                    assert row["native_sum"] == int(expected[valid].sum())
                    references.append({k: row[k] for k in fields})
                    samples += expected.size
    assert sum(r["samples"] for r in exact[1:-1]) * 2 == len(raw)
    assert all(head(s["href"]) == s["object_identity"] for s in sources)
    assert result["peak_rss_bytes"] < 192 * 2**20
    source_fields = ["item_id", "asset_key", "requested_band", "datetime", "href", "object_identity"]
    record = dict(passed=True, real_public_stac=True, independent_discovery_checked=True,
        request=request, query=rows[0]["provenance"]["query"],
        selected_sources=[{k: s[k] for k in source_fields} for s in sources],
        independent_chunk_references=references, samples_compared_bit_for_bit=samples,
        exact_values_sha256=hashlib.sha256(raw).hexdigest(),
        native_consumer_sha256=hashlib.sha256(consumer.BIN.read_bytes()).hexdigest(),
        peak_rss_bytes=result["peak_rss_bytes"], wall_seconds=result["seconds"], completion=rows[-1],
        oracle=dict(rasterio=rasterio.__version__, gdal=rasterio.__gdal_version__, proj=rasterio.__proj_version__, rasterization="GDAL full-source pixel-center mask then crop; all_touched=false"),
        notes=["Acquisition includes live discovery, version pinning, native COG reads, masks and final HEAD validation.", "Independent discovery and GDAL reference/byte comparison excluded from consumer timing.", "Raw samples remain ignored local artifacts; no source imagery committed."])
    (ROOT / "docs/native-sentinel/live.json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps({k:v for k,v in record.items() if k not in ["independent_chunk_references", "request", "completion"]}, indent=2))


if __name__ == "__main__":
    main()
