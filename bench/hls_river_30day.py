"""Matched HLS river experiment: frozen sources, Rasterio baseline, full pipelines.

Run with the sibling Chronozarr checkout's Python environment. Runtime SAS URLs
stay in memory. Outputs are new candidate directories, never published data.
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
import csv
from dataclasses import asdict
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import threading
import time

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
DEFAULT = ROOT / "prototype-output/hls-river-30day-20261028"
BANDS = ["B03", "B04", "B8A"]


def save(path, value):
    Path(path).write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def http():
    import requests

    session = requests.Session()
    session.trust_env = False
    session.headers["Accept-Encoding"] = "identity"
    return session


def head(url, expected=None, session=None):
    from requests import RequestException

    owned = session is None
    session = http() if owned else session
    try:
        response = session.head(url, timeout=(10, 45), allow_redirects=False)
    except RequestException:
        raise RuntimeError("source HEAD transport failed") from None
    finally:
        if owned:
            session.close()
    if response.status_code != 200:
        raise RuntimeError(f"source HEAD HTTP {response.status_code}")
    etag, size = (
        response.headers.get("ETag", ""),
        int(response.headers.get("Content-Length", 0)),
    )
    assert (
        etag.startswith('"')
        and etag.endswith('"')
        and not etag.startswith("W/")
        and size > 0
    )
    if expected is not None:
        assert (etag, size) == (expected["etag"], expected["bytes"]), (
            "source identity changed"
        )
    return etag, size


def geometry(bbox, source_transform, crs):
    """Independent PROJ/GDAL AOI envelope and pixel-centre inclusion."""
    from pyproj import Transformer
    from rasterio.features import geometry_mask
    from rasterio.transform import Affine

    x0, y0, x1, y1 = bbox
    corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]
    ring = []
    for a, b in zip(corners, corners[1:]):
        count = max(1, math.ceil(max(abs(b[0] - a[0]), abs(b[1] - a[1])) / 0.001))
        ring.extend(
            (a[0] + (b[0] - a[0]) * i / count, a[1] + (b[1] - a[1]) * i / count)
            for i in range(count)
        )
    ring.append(ring[0])
    transformer = Transformer.from_crs("EPSG:4326", crs, always_xy=True)
    xs, ys = transformer.transform(*zip(*ring))
    affine = Affine(*source_transform)
    points = [~affine * p for p in zip(xs, ys)]
    left = math.floor(min(p[0] for p in points))
    top = math.floor(min(p[1] for p in points))
    right = math.ceil(max(p[0] for p in points))
    bottom = math.ceil(max(p[1] for p in points))
    window = [left, top, right - left, bottom - top]
    cropped = affine * Affine.translation(left, top)
    polygon = {"type": "Polygon", "coordinates": [list(zip(xs, ys))]}
    mask = geometry_mask(
        [polygon], (window[3], window[2]), cropped, invert=True
    ).astype("uint8")
    return window, list(cropped)[:6], mask


def prepare(work):
    import planetary_computer
    import rasterio

    began = time.perf_counter()
    work.mkdir(parents=True, exist_ok=True)
    if (work / "protocol.json").exists():
        raise FileExistsError(
            "protocol already frozen; use it or select a new --work directory"
        )
    if not (work / "catalog.json").exists():
        query = {
            "collections": ["hls2-s30"],
            "bbox": [-75.13, -7.76, -74.88, -7.51],
            "datetime": "2025-07-01T00:00:00Z/2025-07-30T23:59:59.999Z",
            "limit": 100,
        }
        with http() as session:
            response = session.post(
                "https://planetarycomputer.microsoft.com/api/stac/v1/search",
                json=query,
                timeout=(10, 45),
            )
            response.raise_for_status()
        save(work / "catalog-query.json", query)
        save(work / "catalog.json", response.json())
    catalog = json.loads((work / "catalog.json").read_text())
    assert not any(link.get("rel") == "next" for link in catalog.get("links", [])), (
        "catalog is incomplete"
    )
    items = sorted(
        (x for x in catalog["features"] if ".T18MVS." in x["id"]),
        key=lambda x: x["properties"]["datetime"],
    )
    assert len(items) == 8
    bbox = [-75.075, -7.68, -74.985, -7.59]
    assets = [(item, band) for item in items for band in [*BANDS, "Fmask"]]

    # One ordinary SDK container token avoids per-object signer rate limits.
    planetary_computer.sign(items[0]["assets"][BANDS[0]]["href"])

    def pin(pair):
        item, band = pair
        href = item["assets"][band]["href"]
        url = planetary_computer.sign(href)
        etag, size = head(url)
        return {
            "item": item["id"],
            "band": band,
            "href": href,
            "etag": etag,
            "bytes": size,
        }

    with ThreadPoolExecutor(max_workers=4) as pool:
        pins = list(pool.map(pin, assets))
    url = planetary_computer.sign(pins[0]["href"])
    with rasterio.Env(
        GDAL_HTTP_NETRC="NO",
        GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
        GDAL_HTTP_TIMEOUT="45",
    ):
        with rasterio.open(url) as src:
            native = {
                "crs": src.crs.to_string(),
                "transform": list(src.transform)[:6],
                "shape": list(src.shape),
                "scale": src.scales[0],
                "offset": src.offsets[0],
                "nodata": src.nodata,
            }
    window, affine, mask = geometry(bbox, native["transform"], native["crs"])
    assert (
        window[0] >= 0
        and window[1] >= 0
        and window[0] + window[2] <= native["shape"][1]
        and window[1] + window[3] <= native["shape"][0]
    )
    np.save(work / "independent-aoi-mask.npy", mask)
    request = {
        "product": "hls-s30",
        "provider": "planetary_computer",
        "aoi": {"kind": "bounds", "crs": "EPSG:4326", "bbox": bbox},
        "start": "2025-07-01T00:00:00Z",
        "end": "2025-07-30T23:59:59.999Z",
        "bands": [*BANDS, "Fmask"],
        "max_observations": 8,
        "buffer_bytes": 64 * 2**20,
        "chunk_edge": 256,
        "quality_policies": ["hls_fmask_clear"],
        "observations": [],
    }
    for item in items:
        props = item["properties"]
        request["observations"].append(
            {
                "id": item["id"],
                "datetime": props["datetime"],
                "collection": item["collection"],
                "platform": props.get("platform"),
                "processing": props.get("landsat:correction"),
                "properties": props,
                "assets": {
                    p["band"]: {
                        "href": p["href"],
                        "etag": p["etag"],
                        "metadata": item["assets"][p["band"]],
                    }
                    for p in pins
                    if p["item"] == item["id"]
                },
            }
        )
    save(work / "request.json", request)
    protocol = {
        "schema": "hls-river-matched-v1",
        "bbox": bbox,
        "parent_demo_bbox": [-75.13, -7.76, -74.88, -7.51],
        "site": "Ucayali River, Santa María reach, Peru",
        "dates": [request["start"], request["end"]],
        "bands": BANDS,
        "qa": {
            "band": "Fmask",
            "fill": 255,
            "reject_bits": [1, 2, 3, 4],
            "water_bit": "retained",
            "aerosol_bits": "retained",
            "reserved_bit_0": "must be zero for nonfill",
        },
        "items": [
            {"id": x["id"], "datetime": x["properties"]["datetime"]} for x in items
        ],
        "pins": pins,
        "native": native,
        "window": window,
        "transform": affine,
        "aoi_pixels": int(mask.sum()),
        "pixel_area_m2": abs(affine[0] * affine[4]),
        "encoder": {
            "chunk_size": 256,
            "n_lods": 2,
            "workers": 1,
            "codec": "zstd",
            "level": 5,
        },
        "acquisition_workers": 4,
        "preparation_seconds": time.perf_counter() - began,
        "timing_scope": "fresh process: source signing/revalidation, acquisition, same final playback, same analysis; common catalog search/pin freeze and build excluded; public-server cache uncontrolled",
        "analysis": "NDWI=(green-NIR)/(green+NIR), NDVI=(NIR-red)/(NIR+red); shared clear mask; positive NDWI is a water-index candidate, not validated river area; no composite or gap-fill",
    }
    save(work / "protocol.json", protocol)
    print(
        json.dumps(
            {
                "prepared": True,
                "items": len(items),
                "assets": len(pins),
                "shape": [window[3], window[2]],
                "aoi_pixels": int(mask.sum()),
                "native_crs": native["crs"],
            }
        ),
        flush=True,
    )


def python_pipeline(protocol, work, out):
    """Ordinary Rasterio + NumPy, streaming straight to the common encoder."""
    import planetary_computer
    import rasterio
    from rasterio.windows import Window
    from chronozarr import encode

    selected_window, selected_transform, aoi = geometry(
        protocol["bbox"], protocol["native"]["transform"], protocol["native"]["crs"]
    )
    assert selected_window == protocol["window"]
    assert selected_transform == protocol["transform"]
    pending = []
    window = Window(*protocol["window"])
    expected_transform = protocol["native"]["transform"]
    acquisition_s = 0.0
    source_checks = []
    planetary_computer.sign(protocol["pins"][0]["href"])
    local = threading.local()
    sessions = []

    def read_asset(pin):
        # The standard PC library caches a container token within this process.
        # Each source is still independently pinned before and after GDAL reads.
        url = planetary_computer.sign(pin["href"])
        if not hasattr(local, "session"):
            local.session = http()
            sessions.append(local.session)
        head(url, pin, local.session)
        with rasterio.Env(
            GDAL_HTTP_NETRC="NO",
            GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
            GDAL_HTTP_TIMEOUT="45",
            GDAL_HTTP_MAX_RETRY="0",
            GDAL_NUM_THREADS="1",
            GDAL_HTTP_HEADERS="If-Match: " + pin["etag"],
            CPL_LOG=os.devnull,
        ):
            with rasterio.open(url) as src:
                assert src.crs.to_string() == protocol["native"]["crs"]
                assert list(src.transform)[:6] == expected_transform
                assert list(src.shape) == protocol["native"]["shape"]
                expected_dtype = "uint8" if pin["band"] == "Fmask" else "int16"
                assert src.dtypes[0] == expected_dtype
                assert src.nodata == (255 if pin["band"] == "Fmask" else -9999)
                if pin["band"] != "Fmask":
                    assert (src.scales[0], src.offsets[0]) == (0.0001, 0)
                values = src.read(1, window=window)
        head(url, pin, local.session)
        return values

    def frames():
        nonlocal acquisition_s
        # GDAL's cache limit is process-global, unlike per-object HTTP headers.
        with (
            rasterio.Env(GDAL_CACHEMAX=8 * 2**20),
            ThreadPoolExecutor(max_workers=4) as pool,
        ):
            for item in protocol["items"]:
                assert not pending
                start = time.perf_counter()
                pins = [
                    next(
                        p
                        for p in protocol["pins"]
                        if p["item"] == item["id"] and p["band"] == band
                    )
                    for band in [*BANDS, "Fmask"]
                ]
                raw = list(pool.map(read_asset, pins))
                original_window_hashes = {
                    band: hashlib.sha256(values.tobytes()).hexdigest()
                    for band, values in zip([*BANDS, "Fmask"], raw)
                }
                # Match ordinary filled polygon cropping: pixels outside the
                # requested AOI carry source nodata, including in the QA plane.
                # All source sample bits inside the AOI remain unchanged.
                for band, values in zip([*BANDS, "Fmask"], raw):
                    values[aoi == 0] = 255 if band == "Fmask" else -9999
                data = np.stack(raw[:3])
                qa = raw[3]
                assert np.all((qa == 255) | ((qa & 1) == 0)), "reserved HLS bit set"
                mask = (
                    aoi
                    & np.all(data != -9999, axis=0)
                    & (qa != 255)
                    & ((qa & 0x1E) == 0)
                ).astype("uint8")
                acquisition_s += time.perf_counter() - start
                source_checks.append(
                    {
                        "item": item["id"],
                        "original_window_sha256": original_window_hashes,
                        "asset_sha256": {
                            band: hashlib.sha256(values.tobytes()).hexdigest()
                            for band, values in zip([*BANDS, "Fmask"], raw)
                        },
                        "mask_sha256": hashlib.sha256(mask.tobytes()).hexdigest(),
                    }
                )
                pending.append(mask)
                yield data
        for session in sessions:
            session.close()

    def masks():
        for _ in protocol["items"]:
            yield pending.pop()

    started = time.perf_counter()
    report = encode(
        frames(),
        out / "playback",
        times=np.array(
            [
                np.datetime64(x["datetime"].removesuffix("Z"), "ms")
                for x in protocol["items"]
            ]
        ),
        bands=[{"name": b, "scale": 0.0001, "offset": 0, "units": "1"} for b in BANDS],
        crs=protocol["native"]["crs"],
        transform=protocol["transform"],
        nodata=None,
        mask=masks(),
        provenance={
            "sources": [p["href"] for p in protocol["pins"]],
            "composite": "none: one source item per timestep",
            "gap_fill": "none",
            "notes": json.dumps(
                {"pins": protocol["pins"], "qa": protocol["qa"]}, sort_keys=True
            ),
        },
        **protocol["encoder"],
    )
    return {
        "acquisition_s": acquisition_s,
        "encode_and_acquisition_s": time.perf_counter() - started,
        "encode": asdict(report),
        "source_checks": source_checks,
    }


def tile_pipeline(protocol, work, out):
    from chronozarr import convert_tile_ripper

    started = time.perf_counter()
    command = [
        str(ROOT / "target/release/tile-ripper"),
        "fetch",
        str(work / "request.json"),
        "--output",
        str(out / "observations.zarr"),
        "--gdal-free",
        "--workers",
        "4",
        "--per-host",
        "4",
    ]
    with (out / "fetch.log").open("w") as log:
        result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT)
    if result.returncode:
        raise RuntimeError(
            f"Tile-Ripper fetch failed with exit {result.returncode}; see fetch.log"
        )
    acquisition_s = time.perf_counter() - started
    started = time.perf_counter()
    report = convert_tile_ripper(
        out / "observations.zarr",
        out / "playback",
        bands=BANDS,
        quality_policy="hls_fmask_clear",
        **protocol["encoder"],
    )
    return {
        "acquisition_s": acquisition_s,
        "encode_s": time.perf_counter() - started,
        "encode": asdict(report),
    }


def analyze(protocol, out):
    from chronozarr import open_store

    store = open_store(out / "playback")
    expected_times = np.array(
        [
            np.datetime64(x["datetime"].removesuffix("Z"), "ms")
            for x in protocol["items"]
        ]
    )
    assert np.array_equal(store.times, expected_times), (
        "output times differ from frozen items"
    )
    assert list(store.bands) == protocol["bands"] == BANDS
    assert store.attrs.crs == protocol["native"]["crs"]
    assert store.dtype == np.dtype("int16")
    assert list(store.levels[0].transform) == protocol["transform"]
    assert all(
        (b.scale, b.offset, b.units) == (0.0001, 0, "1") for b in store.attrs.bands
    )
    assert set(store.attrs.provenance["sources"]) == {
        p["href"] for p in protocol["pins"]
    }
    _, _, aoi = geometry(
        protocol["bbox"], protocol["native"]["transform"], protocol["native"]["crs"]
    )
    assert int(aoi.sum()) == protocol["aoi_pixels"]
    rows = []
    numerator, count = None, None
    all_clear = None
    high_clear_masks, water_masks = [], []
    digest = hashlib.sha256()
    for t, item in enumerate(protocol["items"]):
        raw = store.read(t)
        valid = store.read_mask(t).astype(bool)
        assert not valid[aoi == 0].any(), "valid data outside AOI"
        digest.update(raw.tobytes())
        digest.update(valid.tobytes())
        physical = raw.astype("float64") * 0.0001
        green, red, nir = physical
        ndwi_valid = valid & ((green + nir) != 0)
        ndvi_valid = valid & ((nir + red) != 0)
        ndwi = np.divide(
            green - nir, green + nir, out=np.full(green.shape, np.nan), where=ndwi_valid
        )
        ndvi = np.divide(
            nir - red, nir + red, out=np.full(green.shape, np.nan), where=ndvi_valid
        )
        wet = ndwi_valid & (ndwi > 0)
        nclear, nindex, nwet = int(valid.sum()), int(ndwi_valid.sum()), int(wet.sum())
        rows.append(
            {
                "time": item["datetime"],
                "item": item["id"],
                "aoi_pixels": protocol["aoi_pixels"],
                "clear_pixels": nclear,
                "clear_fraction": nclear / protocol["aoi_pixels"],
                "ndwi_defined_pixels": nindex,
                "positive_ndwi_pixels": nwet,
                "positive_ndwi_fraction_of_defined_clear": nwet / nindex
                if nindex
                else None,
                "observed_positive_ndwi_km2": nwet * protocol["pixel_area_m2"] / 1e6
                if nindex
                else None,
                "median_ndwi": float(np.median(ndwi[ndwi_valid])) if nindex else None,
                "median_ndvi": float(np.median(ndvi[ndvi_valid]))
                if ndvi_valid.any()
                else None,
            }
        )
        if nclear / protocol["aoi_pixels"] >= 0.8:
            high_clear_masks.append(ndwi_valid)
            water_masks.append((item["datetime"], wet))
        numerator = wet.astype("uint16") if numerator is None else numerator + wet
        count = ndwi_valid.astype("uint16") if count is None else count + ndwi_valid
        all_clear = ndwi_valid if all_clear is None else all_clear & ndwi_valid
    np.savez_compressed(
        out / "water-index-support.npz",
        positive_count=numerator,
        clear_count=count,
        all_dates_clear=all_clear,
    )
    with (out / "analysis.csv").open("w") as f:
        writer = csv.DictWriter(f, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)
    common = (
        np.logical_and.reduce(high_clear_masks)
        if high_clear_masks
        else np.zeros_like(all_clear)
    )
    common_rows = [
        {
            "time": date,
            "common_clear_pixels": int(common.sum()),
            "positive_ndwi_pixels_on_common_support": int((wet & common).sum()),
            "positive_ndwi_km2_on_common_support": int((wet & common).sum())
            * protocol["pixel_area_m2"]
            / 1e6,
        }
        for date, wet in water_masks
    ]
    save(
        out / "analysis.json",
        {
            "rows": rows,
            "payload_sha256": digest.hexdigest(),
            "all_dates_clear_pixels": int(all_clear.sum()),
            "ever_clear_pixels": int((count > 0).sum()),
            "never_clear_aoi_pixels": int(((count == 0) & (aoi != 0)).sum()),
            "exploratory_high_clear_threshold": 0.8,
            "common_support_high_clear_dates": common_rows,
        },
    )


def worker(route, work, out):
    out.mkdir(parents=True, exist_ok=False)
    protocol = json.loads((work / "protocol.json").read_text())
    started = time.perf_counter()
    stages = (python_pipeline if route == "python" else tile_pipeline)(
        protocol, work, out
    )
    analysis_started = time.perf_counter()
    analyze(protocol, out)
    stages["analysis_s"] = time.perf_counter() - analysis_started
    stages["total_internal_s"] = time.perf_counter() - started
    stages["route"] = route
    save(out / "stages.json", stages)
    print(
        json.dumps({"route": route, "seconds": stages["total_internal_s"]}), flush=True
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["prepare", "worker"])
    parser.add_argument("--work", type=Path, default=DEFAULT)
    parser.add_argument("--route", choices=["python", "tile"])
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if args.mode == "prepare":
        prepare(args.work)
    else:
        worker(args.route, args.work, args.out)


if __name__ == "__main__":
    main()
