"""Independent GDAL/PROJ oracle for the narrow GDAL-free Sentinel path.

Run with prototype-output/zarr-env/bin/python. Generated imagery, request JSON,
exact sample arrays and full inspection documents stay in prototype-output/.
The receipt in docs/ contains counts and source/output hashes, never imagery.

The reference explicitly densifies source edges to <=0.001 degrees, maps with
GDAL/PROJ through rasterio, clips with GEOS/Shapely, then burns GDAL's default
pixel-center rasterizer once on the complete source grid and crops that mask.
This source-global convention keeps pixel inclusion independent of chunk size.
The oracle does not import Tile-Ripper geometry/planning code.
Native metadata and every stored UInt16 value are compared independently.

For a real bounded acquisition, --external-request request.json --sources
sources.json accepts an ordered list of expected selected source snapshots:
[{"id": "...", "locator": "/snapshot.tif", "datetime": "...",
  "requested_band": "red"}]. These expectations must come from independently
checked source selection, rather than being copied from Tile-Ripper's plan.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import time

import numpy as np
import rasterio
from rasterio.features import rasterize
from rasterio.transform import from_origin
from rasterio.warp import transform as proj_transform
from rasterio.windows import Window
import shapely
from shapely.geometry import Polygon, box, mapping, shape

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/native-sentinel/checks"
RECEIPT = ROOT / "docs/native-sentinel/correctness.json"
BIN = ROOT / "prototype-output/native-sentinel/target/release/examples/native_sentinel_oracle"
SAMPLE_KEYS = ["x", "y", "width", "height"]


def digest(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as src:
        for data in iter(lambda: src.read(1024 * 1024), b""):
            h.update(data)
    return h.hexdigest()


def write_json(path, value):
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(json.dumps(value, indent=2) + "\n")


def suffix(prefix, ending):
    return Path(str(prefix) + "." + ending)


def fixture(path, epsg, lon, lat, **overrides):
    """Small deterministic, tiled Sentinel-like one-band stored-value source."""
    x, y = proj_transform("EPSG:4326", f"EPSG:{epsg}", [lon], [lat])
    config = dict(
        driver="GTiff", width=128, height=96, count=1, dtype="uint16",
        tiled=True, blockxsize=32, blockysize=32, crs=f"EPSG:{epsg}",
        transform=from_origin(round(x[0] / 20) * 20 - 1280,
                              round(y[0] / 20) * 20 + 960, 20, 20),
        nodata=65535, compress="deflate", predictor=2,
    )
    config.update(overrides)
    values = ((np.arange(config["height"] * config["width"], dtype=np.uint32)
               * 47 + epsg) % 65535).reshape(config["height"], config["width"])
    values[::7, ::11] = 65535
    values[0, 0] = 0
    with rasterio.open(path, "w", **config) as ds:
        ds.write(values.astype(config["dtype"]), 1)
        ds.scales = [0.0001]
        ds.offsets = [-0.1]
    return Path(path)


def request(path, spatial, edge=23):
    return dict(
        source=dict(kind="assets", assets=[dict(id="B04", locator=str(path),
                    format="cog", datetime="2025-07-14T10:05:59Z")]),
        spatial=spatial,
        time=dict(kind="interval", start="2025-07-14T00:00:00Z",
                  end="2025-07-15T00:00:00Z"),
        variables=[dict(kind="band", index=1)], grid=dict(kind="native"),
        sink=dict(kind="callback"),
        budget=dict(buffer_bytes=4 * 2**20, max_nodes=100000, chunk_edge=edge),
    )


def source_spatial(path, rings, geographic=True):
    """Specify test geometry in source pixel coordinates, never native output."""
    with rasterio.open(path) as ds:
        out = []
        for ring in rings:
            native = [ds.transform * tuple(p) for p in ring]
            if geographic:
                xx, yy = proj_transform(ds.crs, "EPSG:4326",
                                        [p[0] for p in native], [p[1] for p in native])
                out.append(list(map(list, zip(xx, yy))))
            else:
                out.append([list(p) for p in native])
        return dict(kind="polygon", crs="EPSG:4326" if geographic else str(ds.crs),
                    coordinates=out)


def transformed_geometry(spatial, ds):
    if spatial["kind"] == "bounds" and spatial["crs"] == str(ds.crs):
        return None
    if spatial["kind"] == "bounds":
        x0, y0, x1, y1 = spatial["bbox"]
        rings = [[[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]]
    else:
        rings = spatial["coordinates"]
    projected = []
    changing = rasterio.crs.CRS.from_string(spatial["crs"]) != ds.crs
    for ring in rings:
        dense = []
        for a, b in zip(ring[:-1], ring[1:]):
            steps = (max(1, math.ceil(max(abs(b[0] - a[0]), abs(b[1] - a[1])) / 0.001))
                     if changing else 1)
            dense.extend([(a[0] + (b[0] - a[0]) * i / steps,
                           a[1] + (b[1] - a[1]) * i / steps) for i in range(steps)])
        dense.append(tuple(ring[-1]))
        if changing:
            xx, yy = proj_transform(spatial["crs"], ds.crs,
                                    [p[0] for p in dense], [p[1] for p in dense])
            dense = list(zip(xx, yy))
        projected.append(dense)
    return Polygon(projected[0], projected[1:]).intersection(box(*ds.bounds))


def selection_window(ds, spatial, geometry):
    if geometry is not None and (geometry.is_empty or geometry.area <= 0):
        return None
    xmin, ymin, xmax, ymax = spatial["bbox"] if geometry is None else geometry.bounds
    t = ds.transform
    ax, bx = (xmin - t.c) / t.a, (xmax - t.c) / t.a
    ay, by = (ymin - t.f) / t.e, (ymax - t.f) / t.e
    x0 = max(0, min(ds.width, math.floor(min(ax, bx))))
    x1 = max(0, min(ds.width, math.ceil(max(ax, bx))))
    y0 = max(0, min(ds.height, math.floor(min(ay, by))))
    y1 = max(0, min(ds.height, math.ceil(max(ay, by))))
    if x1 <= x0 or y1 <= y0:
        return None
    return dict(x=x0, y=y0, width=x1 - x0, height=y1 - y0)


def expected_sources(req, supplied=None):
    sources = supplied if supplied is not None else req["source"]["assets"]
    predicate = req.get("time")
    out = []
    for source in sources:
        # Test timestamps all use canonical RFC3339 UTC; parsing also supports offsets.
        import datetime
        def instant(value):
            return datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
        if predicate:
            value = instant(source["datetime"])
            if predicate["kind"] == "interval":
                if not instant(predicate["start"]) <= value <= instant(predicate["end"]):
                    continue
            elif value not in [instant(v) for v in predicate["values"]]:
                continue
        out.append(source)
    return out


def reference(req, prefix, supplied=None):
    """Write exact independently derived metadata+bytes; no native plan consulted."""
    rows, selections, samples, valid = [], [], 0, 0
    with suffix(prefix, "u16le").open("wb") as data:
        for source in expected_sources(req, supplied):
            with rasterio.open(source["locator"]) as ds:
                geometry = transformed_geometry(req["spatial"], ds)
                window = selection_window(ds, req["spatial"], geometry)
                if window is None:
                    continue
                # Independent GDAL reference uses the source affine exactly
                # once. Re-rasterizing on shifted window affines can change an
                # exact pixel-center tie through floating-point roundoff.
                full_mask = (rasterize([(mapping(geometry), 1)],
                                      out_shape=(ds.height, ds.width),
                                      transform=ds.transform, fill=0,
                                      dtype="uint8", all_touched=False)
                             if geometry is not None else None)
                source_rows = []
                edge = req["budget"]["chunk_edge"]
                for y in range(window["y"], window["y"] + window["height"], edge):
                    for x in range(window["x"], window["x"] + window["width"], edge):
                        w = dict(x=x, y=y, width=min(edge, window["x"] + window["width"] - x),
                                 height=min(edge, window["y"] + window["height"] - y))
                        win = Window(*(w[k] for k in SAMPLE_KEYS))
                        transform = ds.window_transform(win)
                        mask = (full_mask[y:y + w["height"], x:x + w["width"]]
                                if full_mask is not None else np.ones((w["height"], w["width"]), "uint8"))
                        if not mask.any():
                            continue
                        values = ds.read(1, window=win)
                        values[mask == 0] = ds.nodata
                        count = int(np.count_nonzero(values != ds.nodata))
                        row = dict(type="provisional_chunk", asset_id=source["id"],
                            datetime=source["datetime"], variable=dict(kind="band", index=1),
                            indices=[], window=w, transform=list(transform.to_gdal()), crs=str(ds.crs),
                            nodata=ds.nodata, scale=ds.scales[0], offset=ds.offsets[0],
                            coordinates=None, dtype="uint16", samples=values.size,
                            data_offset_bytes=samples * 2, valid_pixels=count,
                            first_pixel_center=list(transform * (0.5, 0.5)),
                            last_pixel_center=list(transform * (w["width"] - 0.5, w["height"] - 0.5)))
                        data.write(values.astype("<u2").tobytes())
                        samples += values.size
                        valid += count
                        source_rows.append(row)
                if source_rows:
                    selections.append(dict(asset_id=source["id"], datetime=source["datetime"],
                        window=window, crs=str(ds.crs), transform=list(ds.transform.to_gdal()),
                        width=ds.width, height=ds.height, requested_band=source.get("requested_band"),
                        reference_geometry=geometry))
                    rows.extend(source_rows)
    complete = dict(type="complete", chunks=len(rows), samples=samples, valid_pixels=valid)
    suffix(prefix, "ndjson").write_text("".join(json.dumps(r) + "\n" for r in rows + [complete]))
    return rows, complete, selections


def compare_output(prefix, expected_prefix, expected_rows, totals, selections):
    rows = [json.loads(line) for line in suffix(prefix, "ndjson").read_text().splitlines()]
    assert rows and rows[0]["type"] == "provisional_start", "missing stream provenance"
    assert rows[-1]["type"] == "complete", "stream never validated completion"
    chunks = rows[1:-1]
    assert chunks == expected_rows, first_difference(chunks, expected_rows)
    assert suffix(prefix, "u16le").read_bytes() == suffix(expected_prefix, "u16le").read_bytes(), \
        "stored values / polygon mask differ bit for bit"
    for key in ["chunks", "samples", "valid_pixels"]:
        assert rows[-1][key] == totals[key], (key, rows[-1][key], totals[key])
    provenance = rows[0]["provenance"]
    plan = provenance["plan"]
    actual = plan["selected"]
    assert len(actual) == len(selections), "selection count"
    maximum_geometry_error = 0.0
    for got, want in zip(actual, selections):
        for key in ["asset_id", "datetime", "window"]:
            assert got[key] == want[key], (key, got[key], want[key])
        assert got["timestamps"] == [want["datetime"]], "selection date/order"
        for key in ["width", "height", "crs", "transform"]:
            assert got["layout"][key] == want[key], (key, got["layout"][key], want[key])
        if want["reference_geometry"] is not None:
            error = shape(got["mask_geometry"]).hausdorff_distance(want["reference_geometry"])
            assert error <= 0.001, f"transformed/clipped geometry error {error} m exceeds 1 mm"
            maximum_geometry_error = max(maximum_geometry_error, error)
        if want["requested_band"] is not None:
            matched = [s for s in provenance["sources"] if s["asset_id"] == want["asset_id"]]
            assert len(matched) == 1 and matched[0]["requested_band"] == want["requested_band"]
    read_nodes = [n for n in plan["nodes"] if n["operation"]["kind"] == "read"]
    assert len(read_nodes) == len(expected_rows), "plan/read filtering differs"
    for node, chunk in zip(read_nodes, expected_rows):
        assert node["asset_id"] == chunk["asset_id"]
        assert node["operation"]["read"]["window"] == chunk["window"]
        assert node["operation"]["read"]["band"] == 1
    observed = rows[-1]["observed"]
    assert observed["chunks"] == totals["chunks"]
    assert observed["non_nodata_pixels"] == totals["valid_pixels"]
    return maximum_geometry_error


def first_difference(actual, expected):
    for n, (a, b) in enumerate(zip(actual, expected)):
        if a != b:
            return f"chunk {n}: " + repr({k: (a.get(k), b.get(k)) for k in set(a) | set(b) if a.get(k) != b.get(k)})
    return f"chunk count {len(actual)} != {len(expected)}"


def reconstructed_hashes(req, prefix):
    """Reassemble delivered pixels independently of each acquisition's chunks."""
    rows = [json.loads(line) for line in suffix(prefix, "ndjson").read_text().splitlines()]
    chunks = [r for r in rows if r["type"] == "provisional_chunk"]
    values = np.fromfile(suffix(prefix, "u16le"), dtype="<u2")
    fingerprints = {}
    for source in expected_sources(req):
        with rasterio.open(source["locator"]) as ds:
            canvas = np.full((ds.height, ds.width), ds.nodata, dtype="<u2")
            covered = np.zeros((ds.height, ds.width), dtype="bool")
            for chunk in chunks:
                if chunk["asset_id"] != source["id"]:
                    continue
                w = chunk["window"]
                region = np.s_[w["y"]:w["y"] + w["height"], w["x"]:w["x"] + w["width"]]
                assert not covered[region].any(), "overlapping native chunk deliveries"
                covered[region] = True
                start = chunk["data_offset_bytes"] // 2
                canvas[region] = values[start:start + chunk["samples"]].reshape(w["height"], w["width"])
            fingerprints[source["id"]] = hashlib.sha256(canvas.tobytes()).hexdigest()
    return fingerprints


def invoke(binary, req_path, prefix, compare=False):
    command = [str(binary), str(req_path)]
    if compare:
        command.append("--compare")
    env = {**os.environ, "TILE_RIPPER_COG_BACKEND": "native"}
    # The test oracle's own GDAL installation is deliberately outside the native process.
    start = time.monotonic()
    result = subprocess.run(command + [str(prefix)], env=env, text=True,
                            capture_output=True, timeout=90)
    return result, time.monotonic() - start


def synthetic_cases():
    cases = []
    shapes = {
        "rectangle": [[[12.2, 13.7], [111.6, 13.7], [111.6, 82.3], [12.2, 82.3], [12.2, 13.7]]],
        "concavity": [[[7.3, 8.1], [116.8, 8.1], [116.8, 41.4], [60.7, 41.4],
                       [60.7, 87.2], [7.3, 87.2], [7.3, 8.1]]],
        "hole": [[[4.2, 3.8], [123.1, 3.8], [123.1, 91.4], [4.2, 91.4], [4.2, 3.8]],
                 [[35.3, 29.6], [91.8, 29.6], [91.8, 70.2], [35.3, 70.2], [35.3, 29.6]]],
        "partial": [[[-31.2, 21.4], [71.3, -13.7], [140.8, 76.2], [44.9, 119.3], [-31.2, 21.4]]],
        "outside": [[[150, 110], [166, 110], [166, 125], [150, 125], [150, 110]]],
        "tiny-no-centers": [[[10.1, 10.1], [10.2, 10.1], [10.2, 10.2], [10.1, 10.2], [10.1, 10.1]]],
        "clip-splits-polygon": [[[10.2, 20.2], [30.2, 20.2], [30.2, 120.2], [70.2, 120.2],
                                 [70.2, 20.2], [90.2, 20.2], [90.2, 140.2], [10.2, 140.2], [10.2, 20.2]]],
    }
    sources = []
    for label, epsg, lon, lat in [("north17", 32617, -80.1, 36.1),
                                 ("north31", 32631, 3.1, 50.1),
                                 ("south56", 32756, 151.2, -33.8),
                                 ("zone-adjacent", 32618, -77.99, 36.1)]:
        path = fixture(WORK / f"{label}.tif", epsg, lon, lat)
        sources.append(path)
        for shape, rings in shapes.items():
            spatial = source_spatial(path, rings)
            cases.append((f"{label}-{shape}", request(path, spatial), True))
        reversed_rings = [list(reversed(r)) for r in shapes["hole"]]
        cases.append((f"{label}-reversed-rings", request(path, source_spatial(path, reversed_rings)), True))
        coordinates = source_spatial(path, shapes["rectangle"])["coordinates"][0]
        xs, ys = zip(*coordinates)
        cases.append((f"{label}-geographic-bounds", request(path, dict(kind="bounds", crs="EPSG:4326",
                            bbox=[min(xs), min(ys), max(xs), max(ys)])), True))

    path = sources[0]
    boundaries = {
        "centers-rectangle": [[[5.5, 8.5], [75.5, 8.5], [75.5, 75.5], [5.5, 75.5], [5.5, 8.5]]],
        "centers-triangle": [[[5.5, 8.5], [75.5, 78.5], [5.5, 78.5], [5.5, 8.5]]],
        "centers-hole": [[[2.5, 2.5], [90.5, 2.5], [90.5, 90.5], [2.5, 90.5], [2.5, 2.5]],
                         [[20.5, 20.5], [60.5, 20.5], [60.5, 60.5], [20.5, 60.5], [20.5, 20.5]]],
        "outer-pixel-edges": [[[0, 0], [128, 0], [128, 96], [0, 96], [0, 0]]],
        "edge-touch-outside": [[[128, 1], [140, 1], [140, 90], [128, 90], [128, 1]]],
        "slanted-concavity": [[[3.5, 4.5], [90.5, 4.5], [60.5, 38.5], [90.5, 80.5],
                               [3.5, 80.5], [29.5, 38.5], [3.5, 4.5]]],
    }
    for name, rings in boundaries.items():
        for reverse in (False, True):
            r = [list(reversed(r)) for r in rings] if reverse else rings
            cases.append((name + ("-reversed" if reverse else ""),
                          request(path, source_spatial(path, r, geographic=False), edge=17), True))
    with rasterio.open(path) as ds:
        native_bounds = dict(kind="bounds", crs=str(ds.crs), bbox=list(ds.bounds))
    cases.append(("native-bounds-full", request(path, native_bounds), True))
    base = request(path, source_spatial(path, shapes["hole"]))
    ordered = copy.deepcopy(base)
    ordered["source"]["assets"] = [
        dict(id="late-B04", locator=str(path), format="cog", datetime="2025-07-15T00:00:00Z"),
        dict(id="early-B08", locator=str(path), format="cog", datetime="2025-07-14T00:00:00Z"),
        dict(id="excluded", locator="/not-opened-outside-time.tif", format="cog", datetime="2025-07-13T23:59:59Z"),
    ]
    cases.append(("asset-date-band-request-order", ordered, True))
    instants = copy.deepcopy(ordered)
    instants["time"] = dict(kind="instants", values=["2025-07-14T00:00:00Z"])
    cases.append(("explicit-instant-filter", instants, True))
    empty_time = copy.deepcopy(base)
    empty_time["time"] = dict(kind="interval", start="2020-01-01T00:00:00Z", end="2020-01-02T00:00:00Z")
    cases.append(("date-empty-selection", empty_time, True))

    # Explicit source-global versus window-local affine roundoff regressions.
    # Both layouts have Sentinel-like 60m north-up UTM geometry. The 1830-square
    # grid is an ordinary S2 60m tile; the tall fixture retains the original
    # adversarial row8192-scale case. Every group must reconstruct identically
    # for all three output chunk sizes, as well as match the full-grid GDAL mask.
    for label, width, height, top, right, bottom in [
        ("ordinary60m", 1830, 1830, 1071.5, 175.5, 1238.5),
        ("large-row", 192, 8384, 8200.5, 175.5, 8367.5),
    ]:
        p = fixture(WORK / f"{label}.tif", 32617, -80.1, 36.1,
                    width=width, height=height,
                    transform=from_origin(579740, 3996380, 60, 60))
        shapes = {
            "rectangle": [[[5.5, top], [right, top], [right, bottom], [5.5, bottom], [5.5, top]]],
            "triangle": [[[5.5, top], [right, top + 170], [5.5, top + 170], [5.5, top]]],
            "hole": [[[5.5, top], [right, top], [right, bottom], [5.5, bottom], [5.5, top]],
                     [[35.5, top + 30], [145.5, top + 30], [145.5, top + 130],
                      [35.5, top + 130], [35.5, top + 30]]],
        }
        for name, rings in shapes.items():
            for edge in [17, 32, 64]:
                req = request(p, source_spatial(p, rings, geographic=False), edge=edge)
                cases.append((f"source-global-{label}-{name}-chunk{edge}", req, True))

    bad_spatial = {
        "antimeridian": dict(kind="bounds", crs="EPSG:4326", bbox=[179.9, 10, -179.9, 10.1]),
        "antimeridian-polygon": dict(kind="polygon", crs="EPSG:4326", coordinates=[[[179.9, 10], [-179.9, 10], [-179.9, 10.1], [179.9, 10.1], [179.9, 10]]]),
        "oversize-geographic-span": dict(kind="bounds", crs="EPSG:4326", bbox=[-85, 34, -78, 37]),
        "unsupported-input-crs": {**base["spatial"], "crs": "EPSG:4269"},
        "unsupported-input-wkt": {**base["spatial"], "crs": 'GEOGCS["WGS 84"]'},
        "wrong-hemisphere": dict(kind="bounds", crs="EPSG:4326", bbox=[-80.1, -36.1, -80, -36]),
        "too-distant-utm-zone": dict(kind="bounds", crs="EPSG:4326", bbox=[2.9, 36, 3, 36.1]),
        "polar-latitude": dict(kind="bounds", crs="EPSG:4326", bbox=[-80, 84, -79.9, 84.1]),
        "bowtie-polygon": dict(kind="polygon", crs="EPSG:4326", coordinates=[[[-80.1, 36], [-80, 36.1], [-80.1, 36.1], [-80, 36], [-80.1, 36]]]),
        "unclosed-ring": {**base["spatial"], "coordinates": [base["spatial"]["coordinates"][0][:-1]]},
        "degenerate-ring": dict(kind="polygon", crs="EPSG:4326", coordinates=[[[-80, 36], [-80, 36], [-80, 36], [-80, 36]]]),
        "hole-outside-shell": {**base["spatial"], "coordinates": [base["spatial"]["coordinates"][1], base["spatial"]["coordinates"][0]]},
        "three-dimensional-position": dict(kind="polygon", crs="EPSG:4326", coordinates=[[[-80, 36, 1], [-80.1, 36, 1], [-80.1, 36.1, 1], [-80, 36, 1]]]),
        "empty-polygon": dict(kind="polygon", crs="EPSG:4326", coordinates=[]),
    }
    for name, spatial in bad_spatial.items():
        req = copy.deepcopy(base)
        req["spatial"] = spatial
        cases.append((name, req, False))
    for name, overrides in [("uint8-profile", dict(dtype="uint8", nodata=255)),
                             ("strip-profile", dict(tiled=False)),
                             ("missing-mask-nodata", dict(nodata=None)),
                             ("non-utm-source-transform", dict(crs="EPSG:3857"))]:
        p = fixture(WORK / f"{name}.tif", 32617, -80.1, 36.1, **overrides)
        req = copy.deepcopy(base)
        req["source"]["assets"][0]["locator"] = str(p)
        cases.append((name, req, False))
    invalid_band = copy.deepcopy(base)
    invalid_band["variables"] = [dict(kind="band", index=2)]
    cases.append(("unsupported-band-index", invalid_band, False))
    return cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BIN)
    parser.add_argument("--build", action="store_true")
    parser.add_argument("--external-request", type=Path)
    parser.add_argument("--sources", type=Path)
    parser.add_argument("--name", default="real-sentinel")
    parser.add_argument("--receipt", type=Path, default=RECEIPT)
    options = parser.parse_args()
    WORK.mkdir(parents=True, exist_ok=True)
    if options.build:
        subprocess.run(["cargo", "build", "--release", "--no-default-features", "--features", "native-cog",
                        "--example", "native_sentinel_oracle", "--target-dir", str(BIN.parents[2])],
                       cwd=ROOT, check=True)
    supplied = None
    if options.external_request:
        if not options.sources:
            parser.error("--sources is required with --external-request")
        supplied = json.loads(options.sources.read_text())
        cases = [(options.name, json.loads(options.external_request.read_text()), True)]
    else:
        cases = synthetic_cases()
    results = []
    invariance = {}
    for name, req, success in cases:
        prefix = WORK / name
        req_path = suffix(prefix, "request.json")
        write_json(req_path, req)
        # Remove old completion records so a failed run cannot inherit success.
        for ending in ["ndjson", "u16le"]:
            suffix(prefix, ending).unlink(missing_ok=True)
        result = dict(name=name, expected="complete" if success else "explicit_rejection")
        try:
            expected_prefix = WORK / f"{name}-gdal-reference"
            if success:
                expected_rows, totals, selections = reference(req, expected_prefix, supplied)
            proc, seconds = invoke(options.binary, req_path, prefix)
            result.update(exit_code=proc.returncode, seconds=round(seconds, 6))
            suffix(prefix, "stderr.txt").write_text(proc.stderr)
            if success:
                assert proc.returncode == 0, proc.stderr
                geometry_error = compare_output(prefix, expected_prefix, expected_rows, totals, selections)
                # A second execution asks the public Rust consumer to compare
                # independently authored metadata and every expected sample.
                compared, _ = invoke(options.binary, req_path, expected_prefix, compare=True)
                assert compared.returncode == 0, compared.stderr
                complete = json.loads(compared.stdout.splitlines()[-1])
                assert complete["type"] == "complete" and complete["compared"]
                result.update(chunks=totals["chunks"], samples=totals["samples"],
                              valid_pixels=totals["valid_pixels"], exact_stored_values=True,
                              exact_metadata=True, maximum_geometry_error_m=geometry_error,
                              oracle_sha256=digest(suffix(expected_prefix, "u16le")),
                              native_sha256=digest(suffix(prefix, "u16le")))
                if name.startswith("source-global-"):
                    group = name.rsplit("-chunk", 1)[0]
                    reconstructed = reconstructed_hashes(req, prefix)
                    result["reconstructed_source_sha256"] = reconstructed
                    invariance.setdefault(group, []).append((req["budget"]["chunk_edge"], reconstructed))
            else:
                output = suffix(prefix, "ndjson")
                rows = [json.loads(l) for l in output.read_text().splitlines()] if output.exists() else []
                assert proc.returncode != 0 and not any(r["type"] == "complete" for r in rows), \
                    f"unsupported request completed: {name}"
                result["rejection"] = proc.stderr.strip()[:500]
            result["passed"] = True
        except Exception as error:
            result.update(passed=False, error=str(error))
        results.append(result)
        print(name, "PASS" if result["passed"] else "FAIL: " + result["error"], flush=True)
    if not options.external_request:
        groups = sorted({name.rsplit("-chunk", 1)[0] for name, _, _ in cases
                         if name.startswith("source-global-")})
        for group in groups:
            observations = sorted(invariance.get(group, []))
            passed = (len(observations) == 3 and [o[0] for o in observations] == [17, 32, 64]
                      and all(o[1] == observations[0][1] for o in observations))
            results.append(dict(name=group + "-chunk-size-invariance", expected="chunk_size_invariance",
                                passed=passed, chunk_edges=[o[0] for o in observations],
                                reconstructed_source_sha256=observations[0][1] if observations else None))
            print(group + "-chunk-size-invariance", "PASS" if passed else "FAIL", flush=True)
    source_paths = sorted({s["locator"] for _, req, _ in cases
                           for s in (supplied or req["source"].get("assets", []))
                           if Path(s["locator"]).is_file()})
    receipt = dict(schema_version=1, scope="independent GDAL reference; narrow native Sentinel geometry",
        reference=dict(rasterio=rasterio.__version__, gdal=rasterio.__gdal_version__,
                       proj=rasterio.__proj_version__, shapely=shapely.__version__,
                       densification_degrees=0.001, transformed_geometry_tolerance_m=0.001,
                       rasterization="GDAL default pixel-center on full source grid, then crop; all_touched=false",
                       native_samples="UInt16 stored values; no calibration or raster reprojection"),
        binary_sha256=digest(options.binary),
        sources=[dict(name=Path(p).name, sha256=digest(p)) for p in source_paths],
        accepted=sum(r["passed"] for r in results), failed=sum(not r["passed"] for r in results),
        success_cases=sum(r["expected"] == "complete" for r in results),
        rejection_cases=sum(r["expected"] == "explicit_rejection" for r in results),
        chunk_invariance_checks=sum(r["expected"] == "chunk_size_invariance" for r in results),
        samples_compared_bit_for_bit=sum(r.get("samples", 0) for r in results if r["passed"]),
        results=results)
    write_json(options.receipt, receipt)
    print(json.dumps({k: receipt[k] for k in ["accepted", "failed", "success_cases", "rejection_cases", "samples_compared_bit_for_bit"]}))
    return 0 if receipt["failed"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
