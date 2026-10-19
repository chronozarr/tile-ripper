"""Bounded STAC integration, independent rasterio/GDAL oracle, and local benchmarks.
No public downloads. Run --worker only through the harness.
"""

import argparse
import copy
import hashlib
import http.server
import json
import math
import os
from pathlib import Path
import random
import platform
import re
import shutil
import statistics
import subprocess
import sys
import threading
import time
import urllib.request
import numpy as np
import rasterio
from rasterio.features import geometry_mask
from rasterio.shutil import copy as rio_copy
from rasterio.transform import from_origin
from rasterio.warp import transform_geom, transform
from rasterio.windows import Window
from run import process

REPO = Path(__file__).resolve().parent.parent
COLLECTION = "sentinel-2-c1-l2a"


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def reset(self):
        self.counts = dict(
            stac_requests=0,
            stac_bytes=0,
            heads=0,
            range_gets=0,
            whole_gets=0,
            raster_bytes=0,
            response_seconds=0.0,
            conditional_gets=0,
            rejected_conditionals=0,
            injected_failures=0,
        )


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
        self.server.queries.append(body)
        if getattr(self.server, "advance_on_second", False):
            self.server.advanced_searches += 1
            if self.server.advanced_searches >= 2:
                self.server.current_etag = '"new-fixture"'
        self.respond_page(0, body.get("datetime", ""))

    def respond_page(self, page, interval):
        self.server.counts["stac_requests"] += 1
        features = [] if interval.startswith("2020-") else self.server.features
        # One item per page, regardless of the requested limit, exercises pagination.
        batch = features[page : page + 1]
        links = (
            []
            if page + 1 >= len(features)
            else [
                dict(
                    rel="next",
                    href=f"http://127.0.0.1:{self.server.server_port}/page/{page + 1}",
                    method="GET",
                )
            ]
        )
        data = json.dumps(
            dict(type="FeatureCollection", features=batch, links=links)
        ).encode()
        self.server.counts["stac_bytes"] += len(data)
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_HEAD(self):
        self.asset(False)

    def do_GET(self):
        if self.path.startswith("/page/"):
            self.respond_page(int(self.path.rsplit("/", 1)[-1]), "")
        else:
            self.asset(True)

    def asset(self, body):
        name = self.path.split("?")[0].lstrip("/")
        path = self.server.root / name
        if name not in self.server.files or not path.exists():
            self.send_error(404)
            return
        size = path.stat().st_size
        if body and self.headers.get("If-Match"):
            self.server.counts["conditional_gets"] += 1
            self.server.seen_conditions.append(self.headers["If-Match"])
            actual_etag = (
                '"changed-fixture"'
                if getattr(self.server, "changed", False)
                else getattr(self.server, "current_etag", '"fixed-fixture"')
            )
            if self.headers["If-Match"] != actual_etag:
                self.server.counts["rejected_conditionals"] += 1
                self.send_error(412)
                return
        start, end = 0, size - 1
        if "Range" in self.headers:
            m = re.fullmatch(r"bytes=(\d+)-(\d*)", self.headers["Range"])
            if not m:
                self.send_error(416)
                return
            start = int(m[1])
            end = min(size - 1, int(m[2]) if m[2] else size - 1)
            if start > end:
                self.send_error(416)
                return
        if body and start > 32768 and getattr(self.server, "fail_data", False):
            self.server.counts["injected_failures"] += 1
            self.send_error(503)
            return
        self.send_response(206 if "Range" in self.headers else 200)
        self.send_header("Content-Length", str(end - start + 1))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header(
            "ETag", getattr(self.server, "current_etag", '"fixed-fixture"')
        )
        if "Range" in self.headers:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.end_headers()
        if body:
            t = time.perf_counter()
            with path.open("rb") as f:
                f.seek(start)
                self.wfile.write(f.read(end - start + 1))
            self.server.counts[
                "range_gets" if "Range" in self.headers else "whole_gets"
            ] += 1
            self.server.counts["raster_bytes"] += end - start + 1
            self.server.counts["response_seconds"] += time.perf_counter() - t
        else:
            self.server.counts["heads"] += 1


def polygon(x0, y0, x1, y1):
    return dict(
        type="Polygon", coordinates=[[[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]]
    )


def create(root, port):
    root.mkdir(parents=True, exist_ok=True)
    files = []
    features = []
    rng = np.random.default_rng(20261019)
    for scene in range(2):
        assets = {}
        x0 = 600000 + scene * 5120
        y0 = 4000020
        crs = "EPSG:32617" if scene == 0 else "EPSG:32618"
        if scene == 1:
            xs, ys = transform("EPSG:32617", crs, [x0], [y0])
            x0, y0 = xs[0], ys[0]
        for band, res, n in [("red", 10, 512), ("nir", 20, 256)]:
            file = f"scene{scene}-{band}.tif"
            files.append(file)
            data = rng.integers(1, 30000, (n, n), dtype=np.uint16)
            if scene == 1:
                data[n // 2 :, : n // 2] = 0  # partial/nodata region
            with rasterio.open(
                root / "staging.tif",
                "w",
                driver="GTiff",
                width=n,
                height=n,
                count=1,
                dtype="uint16",
                crs=crs,
                transform=from_origin(x0, y0, res, res),
                nodata=0,
            ) as ds:
                ds.write(data, 1)
                ds.scales = (0.0001,)
                ds.offsets = (-0.1,)
            rio_copy(
                root / "staging.tif",
                root / file,
                driver="COG",
                BLOCKSIZE=128,
                COMPRESS="DEFLATE",
                PREDICTOR=2,
            )
            assets[band] = dict(
                href=f"http://127.0.0.1:{port}/{file}?signature=NEVER_SAVE_THIS",
                type="image/tiff; application=geotiff; profile=cloud-optimized",
                roles=["data"],
                **{"raster:bands": [dict(nodata=0, scale=0.0001, offset=-0.1)]},
            )
        geom = transform_geom(crs, "EPSG:4326", polygon(x0, y0 - 5120, x0 + 5120, y0))
        features.append(
            dict(
                type="Feature",
                stac_version="1.0.0",
                id=f"scene-{scene}",
                collection=COLLECTION,
                geometry=geom,
                properties=dict(
                    datetime="2025-07-28T15:00:00Z", platform="sentinel-2a"
                ),
                assets=assets,
                links=[],
            )
        )
    (root / "staging.tif").unlink()
    # Deliberately false bbox candidate: real footprint misses AOI and must be rejected.
    false = copy.deepcopy(features[0])
    false["id"] = "bbox-only-false-positive"
    false["geometry"] = transform_geom(
        "EPSG:32617", "EPSG:4326", polygon(600000, 3994900, 600100, 3995000)
    )
    false["bbox"] = [-80, 35, -78, 37]
    features.append(false)
    return files, features


def request(port, output, large=False):
    x0, x1, y0, y1 = (
        (604000, 606000, 3997000, 3999500)
        if large
        else (604900, 605500, 3997000, 3997600)
    )
    outer = [[x0, y0], [x1, y0], [x1 - 70, y1], [x0, y1], [x0, y0]]
    hole = [
        [605000, y0 + 100],
        [605000, y0 + 200],
        [605080, y0 + 200],
        [605080, y0 + 100],
        [605000, y0 + 100],
    ]
    geom = transform_geom(
        "EPSG:32617", "EPSG:4326", dict(type="Polygon", coordinates=[outer, hole])
    )
    return dict(
        endpoint=f"http://127.0.0.1:{port}",
        collection=COLLECTION,
        provider="earth_search",
        aoi=dict(kind="polygon", crs="EPSG:4326", coordinates=geom["coordinates"]),
        start="2025-07-28T00:00:00Z",
        end="2025-07-28T23:59:59Z",
        bands=["B04", "B08"],
        output=str(output),
        limits=dict(
            page_size=1,
            max_items=8,
            chunk_edge=64,
            buffer_bytes=65536,
            max_output_pixels=1000000,
        ),
    )


def discover_baseline(req):
    body = dict(
        collections=[req["collection"]],
        intersects=dict(type="Polygon", coordinates=req["aoi"]["coordinates"]),
        datetime=req["start"] + "/" + req["end"],
        limit=1,
    )
    data = json.load(
        urllib.request.urlopen(
            urllib.request.Request(
                req["endpoint"] + "/search",
                data=json.dumps(body).encode(),
                headers={"Content-Type": "application/json"},
            )
        )
    )
    items = data["features"]
    while any(link["rel"] == "next" for link in data["links"]):
        data = json.load(
            urllib.request.urlopen(
                next(link["href"] for link in data["links"] if link["rel"] == "next")
            )
        )
        items.extend(data["features"])
    # Same selected IDs, frozen from the independent fixture contract. This isolates
    # extraction/transport rather than timing a second geometry discovery engine.
    return [i for i in items if i["id"] in ("scene-0", "scene-1")]


def dense_wgs(req):
    # Reference geometry mapping uses rasterio's independently executed GDAL/PROJ path.
    rings = []
    for ring in req["aoi"]["coordinates"]:
        points = []
        for a, b in zip(ring, ring[1:]):
            n = max(1, math.ceil(max(abs(a[0] - b[0]), abs(a[1] - b[1])) / 0.001))
            points.extend(
                [
                    [a[0] + (b[0] - a[0]) * j / n, a[1] + (b[1] - a[1]) * j / n]
                    for j in range(n)
                ]
            )
        points.append(ring[-1])
        rings.append(points)
    return dict(type="Polygon", coordinates=rings)


def baseline(req):
    t = time.perf_counter()
    items = discover_baseline(req)
    discovery = time.perf_counter() - t
    out = Path(req["output"])
    out.mkdir()
    planning = read = masking = writing = 0.0
    outputs = []
    with rasterio.Env(
        GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
        GDAL_NUM_THREADS="1",
        GDAL_CACHEMAX=32 * 1024**2,
        GDAL_INGESTED_BYTES_AT_OPEN="32768",
        GDAL_HTTP_MAX_RETRY="2",
        GDAL_HTTP_TIMEOUT="45",
    ):
        for item in items:
            for key in ("red", "nir"):
                t = time.perf_counter()
                head = urllib.request.urlopen(
                    urllib.request.Request(item["assets"][key]["href"], method="HEAD")
                )
                etag = head.headers["ETag"]
                head.close()
                with (
                    rasterio.Env(GDAL_HTTP_HEADERS=f"If-Match: {etag}\r\n"),
                    rasterio.open(item["assets"][key]["href"]) as src,
                ):
                    geom = transform_geom("EPSG:4326", src.crs, dense_wgs(req))
                    coords = np.asarray(geom["coordinates"][0])
                    c0, r1 = (~src.transform) * (coords[:, 0].min(), coords[:, 1].min())
                    c1, r0 = (~src.transform) * (coords[:, 0].max(), coords[:, 1].max())
                    c0, r0 = max(0, math.floor(c0)), max(0, math.floor(r0))
                    c1, r1 = (
                        min(src.width, math.ceil(c1)),
                        min(src.height, math.ceil(r1)),
                    )
                    win = Window(c0, r0, c1 - c0, r1 - r0)
                    planning += time.perf_counter() - t
                    file = f"{item['id']}-{key}.tif"
                    t = time.perf_counter()
                    with rasterio.open(
                        out / file,
                        "w",
                        driver="GTiff",
                        width=win.width,
                        height=win.height,
                        count=1,
                        dtype="uint16",
                        crs=src.crs,
                        transform=src.window_transform(win),
                        nodata=src.nodata,
                        tiled=True,
                        blockxsize=256,
                        blockysize=256,
                        compress="deflate",
                        predictor=2,
                    ) as dst:
                        dst.scales = src.scales
                        dst.offsets = src.offsets
                        writing += time.perf_counter() - t
                        edge = req["limits"]["chunk_edge"]
                        for y in range(r0, r1, edge):
                            for x in range(c0, c1, edge):
                                w = Window(x, y, min(edge, c1 - x), min(edge, r1 - y))
                                t = time.perf_counter()
                                m = geometry_mask(
                                    [geom],
                                    out_shape=(int(w.height), int(w.width)),
                                    transform=src.window_transform(w),
                                    invert=True,
                                )
                                masking += time.perf_counter() - t
                                if not m.any():
                                    continue
                                t = time.perf_counter()
                                a = src.read(1, window=w)
                                read += time.perf_counter() - t
                                t = time.perf_counter()
                                a[~m] = src.nodata
                                masking += time.perf_counter() - t
                                t = time.perf_counter()
                                dst.write(
                                    a,
                                    1,
                                    window=Window(x - c0, y - r0, w.width, w.height),
                                )
                                writing += time.perf_counter() - t
                    outputs.append(
                        dict(file=file, href=item["assets"][key]["href"].split("?")[0])
                    )
    manifest = dict(
        outputs=outputs,
        discovery_seconds=discovery,
        planning_seconds=planning,
        read_decode_seconds=read,
        masking_seconds=masking,
        sink_seconds=writing,
    )
    (out / "baseline.json").write_text(json.dumps(manifest))
    return manifest


def verify(req, root, fixture, manifest):
    nodata_pixels = 0
    valid_pixels = 0
    assert len(manifest["outputs"]) == 4
    resolutions = set()
    crses = set()
    refs = {s["asset_id"]: s for s in manifest.get("sources", [])}
    for output in manifest["outputs"]:
        href = refs[output["asset_id"]]["href"] if refs else output["href"]
        source = fixture / href.split("/")[-1].split("?")[0]
        with rasterio.open(source) as src, rasterio.open(root / output["file"]) as dst:
            col, row = (~src.transform) * (dst.transform.c, dst.transform.f)
            col, row = round(col), round(row)
            geom = transform_geom("EPSG:4326", src.crs, dense_wgs(req))
            inside = geometry_mask(
                [geom], out_shape=dst.shape, transform=dst.transform, invert=True
            )
            expected = src.read(1, window=Window(col, row, dst.width, dst.height))
            expected[~inside] = src.nodata
            actual = dst.read(1)
            assert np.array_equal(actual, expected), (
                output["file"],
                int(np.count_nonzero(actual != expected)),
            )
            assert dst.transform == src.window_transform(
                Window(col, row, dst.width, dst.height)
            )
            assert (
                dst.crs == src.crs
                and dst.nodata == src.nodata
                and dst.scales == src.scales
                and dst.offsets == src.offsets
            )
            resolutions.add(dst.res[0])
            crses.add(dst.crs.to_epsg())
            nodata_pixels += int(np.count_nonzero(actual == 0))
            valid_pixels += int(np.count_nonzero(actual))
    assert (
        resolutions == {10.0, 20.0}
        and crses == {32617, 32618}
        and nodata_pixels > 0
        and valid_pixels > 0
    )
    assert "NEVER_SAVE_THIS" not in json.dumps(manifest) if refs else True
    return dict(
        outputs=4,
        exact_pixels=True,
        nodata_pixels=nodata_pixels,
        valid_pixels=valid_pixels,
        native_resolutions=sorted(resolutions),
        native_crses=sorted(crses),
    )


def probes(plan, fixture):
    sources = {s["asset_id"]: s for s in plan["sources"]}
    ranges = set()
    for node in plan["plan"]["nodes"]:
        op = node["operation"]
        if op["kind"] != "read":
            continue
        for tile in op["read"].get("tiles", []):
            if tile["bytes"]:
                ranges.add((node["asset_id"], tile["offset"], tile["bytes"]))
    total = 0
    start = time.perf_counter()
    for asset, offset, length in sorted(ranges):
        req = urllib.request.Request(
            sources[asset]["href"],
            headers={
                "Range": f"bytes={offset}-{offset + length - 1}",
                "If-Match": sources[asset]["object_identity"]["etag"],
            },
        )
        with urllib.request.urlopen(req) as response:
            body = response.read()
            assert len(body) == length
            total += len(body)
    transfer = time.perf_counter() - start
    decode = 0.0
    pixels = 0
    for selected in plan["plan"]["selected"]:
        ref = sources[selected["asset_id"]]
        file = fixture / ref["href"].split("/")[-1]
        w = selected["window"]
        # The COG bytes and dataset metadata are loaded before starting decode timing.
        with rasterio.io.MemoryFile(file.read_bytes()) as memory, memory.open() as ds:
            start = time.perf_counter()
            a = ds.read(1, window=Window(w["x"], w["y"], w["width"], w["height"]))
            decode += time.perf_counter() - start
            pixels += a.size
    return dict(
        transfer_only_seconds=transfer,
        transfer_body_bytes=total,
        transfer_range_requests=len(ranges),
        decode_and_window_assembly_from_memory_seconds=decode,
        decoded_pixels=pixels,
        scope="independent probes, not additive stages of production RasterIO; COG header open excluded from decode timer",
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--worker")
    ap.add_argument("--output", default="prototype-output/stac-benchmark")
    ap.add_argument("--reps", type=int, default=5)
    args = ap.parse_args()
    if args.worker:
        print(json.dumps(baseline(json.loads(Path(args.worker).read_text()))))
        return
    root = Path(args.output).resolve()
    root.mkdir(parents=True, exist_ok=True)
    server = Server(("127.0.0.1", 0), Handler)
    server.root = root / "fixtures"
    server.files, server.features = create(server.root, server.server_port)
    server.queries = []
    server.seen_conditions = []
    server.reset()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    binary = REPO / "target/release/tile-ripper"
    runs = []
    rng = random.Random(20261019)
    checks = {}
    probe_runs = []
    try:
        if (root / "out").exists():
            shutil.rmtree(root / "out")
        req = request(server.server_port, root / "out")
        path = root / "request.json"
        path.write_text(json.dumps(req))
        server.reset()
        result = subprocess.run(
            [str(binary), "stac-plan", str(path), "--discover-only"],
            capture_output=True,
            text=True,
            check=True,
        )
        d = json.loads(result.stdout)
        assert (
            d["discovery"]["returned_items"] == 3
            and d["discovery"]["selected_items"] == 2
            and server.counts["raster_bytes"] == 0
        )
        checks["pagination_exact_footprints_discover_only"] = dict(server.counts)
        empty = copy.deepcopy(req)
        empty["start"] = "2020-01-01T00:00:00Z"
        empty["end"] = "2020-01-02T00:00:00Z"
        path.write_text(json.dumps(empty))
        server.reset()
        p = subprocess.run(
            [str(binary), "stac-plan", str(path)],
            capture_output=True,
            text=True,
            check=True,
        )
        assert (
            not json.loads(p.stdout)["plan"]["selected"]
            and server.counts["raster_bytes"] == 0
        )
        empty["output"] = str(root / "empty-output")
        if Path(empty["output"]).exists():
            shutil.rmtree(empty["output"])
        path.write_text(json.dumps(empty))
        server.reset()
        p = subprocess.run(
            [str(binary), "stac-fetch", str(path)],
            capture_output=True,
            text=True,
            check=True,
        )
        assert (
            not json.loads(p.stdout)["outputs"] and server.counts["raster_bytes"] == 0
        )
        checks["empty_query"] = True
        tiny = copy.deepcopy(req)
        g = transform_geom(
            "EPSG:32617", "EPSG:4326", polygon(604999.9, 3997000.1, 605000.1, 3997000.3)
        )
        tiny["aoi"]["coordinates"] = g["coordinates"]
        path.write_text(json.dumps(tiny))
        p = subprocess.run(
            [str(binary), "stac-plan", str(path)],
            capture_output=True,
            text=True,
            check=True,
        )
        assert not json.loads(p.stdout)["plan"]["selected"]
        checks["footprint_overlap_without_selected_pixel_centers"] = True
        over = copy.deepcopy(req)
        over["limits"]["max_items"] = 1
        path.write_text(json.dumps(over))
        p = subprocess.run(
            [str(binary), "stac-plan", str(path)], capture_output=True, text=True
        )
        assert p.returncode != 0 and "max_items" in p.stderr
        checks["no_silent_truncation"] = True
        # Limit rejection, mutation precondition and failed-read cleanup are distinct cases.
        capped = copy.deepcopy(req)
        capped["limits"]["max_output_pixels"] = 1
        path.write_text(json.dumps(capped))
        p = subprocess.run(
            [str(binary), "stac-fetch", str(path)], capture_output=True, text=True
        )
        assert (
            p.returncode != 0
            and "max_output_pixels" in p.stderr
            and not Path(req["output"]).exists()
        ), (
            p.returncode,
            p.stderr[:1000],
            Path(req["output"]).exists(),
            server.counts,
            server.seen_conditions,
        )
        checks["output_budget_preflight"] = True
        server.changed = True
        path.write_text(json.dumps(req))
        server.reset()
        p = subprocess.run(
            [str(binary), "stac-fetch", str(path)], capture_output=True, text=True
        )
        assert (
            p.returncode != 0
            and server.counts["rejected_conditionals"] > 0
            and not Path(req["output"]).exists()
        )
        assert "NEVER_SAVE_THIS" not in p.stderr
        server.changed = False
        checks["changed_object_rejected"] = True
        failed = copy.deepcopy(req)
        failed["limits"]["max_retries"] = 0
        path.write_text(json.dumps(failed))
        server.reset()
        server.fail_data = True
        p = subprocess.run(
            [str(binary), "stac-fetch", str(path)], capture_output=True, text=True
        )
        assert (
            p.returncode != 0
            and server.counts["injected_failures"] > 0
            and not Path(req["output"]).exists()
        )
        server.fail_data = False
        checks["failed_read_output_cleanup"] = True
        server.advance_on_second = True
        server.advanced_searches = 0
        server.current_etag = '"fixed-fixture"'
        path.write_text(json.dumps(req))
        subprocess.run(
            [
                "cargo",
                "test",
                "--locked",
                "--test",
                "stac_workflow",
                "prepared_versions_do_not_reuse_another_snapshot_cache",
                "--",
                "--ignored",
            ],
            cwd=REPO,
            env={**os.environ, "TILE_RIPPER_STAC_TEST_REQUEST": str(path)},
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            check=True,
        )
        server.advance_on_second = False
        server.current_etag = '"fixed-fixture"'
        checks["library_prepared_snapshots_do_not_share_stale_cache"] = True
        for large in (False, True):
            for rep in range(args.reps):
                order = ["tile-ripper", "gdal-stac-tuned"]
                rng.shuffle(order)
                for config in order:
                    out = root / "out"
                    if out.exists():
                        shutil.rmtree(out)
                    req = request(server.server_port, out, large)
                    path.write_text(json.dumps(req))
                    server.reset()
                    cmd = (
                        [str(binary), "stac-fetch", str(path)]
                        if config == "tile-ripper"
                        else [
                            sys.executable,
                            str(Path(__file__)),
                            "--worker",
                            str(path),
                        ]
                    )
                    result, wall, rss = process(cmd)
                    observed = dict(server.counts)
                    if config == "tile-ripper":
                        assert observed["conditional_gets"] > 0
                    equality = verify(req, out, server.root, result)
                    runs.append(
                        dict(
                            workload="large" if large else "small",
                            config=config,
                            rep=rep,
                            wall_seconds=wall,
                            peak_rss_bytes=rss,
                            observed=observed,
                            stages=result.get("observed", result),
                            discovery=result.get("discovery"),
                            equality=equality,
                        )
                    )
        for large in (False, True):
            req = request(server.server_port, root / "probe-unused", large)
            path.write_text(json.dumps(req))
            planned = json.loads(
                subprocess.check_output(
                    [str(binary), "stac-plan", str(path)], text=True
                )
            )
            for rep in range(args.reps):
                probe_runs.append(
                    dict(
                        workload="large" if large else "small",
                        rep=rep,
                        **probes(planned, server.root),
                    )
                )
    finally:
        server.shutdown()
        server.server_close()
    summaries = []
    for workload, config in sorted({(r["workload"], r["config"]) for r in runs}):
        rows = [r for r in runs if r["workload"] == workload and r["config"] == config]
        v = [r["wall_seconds"] for r in rows]
        summaries.append(
            dict(
                workload=workload,
                config=config,
                n=len(v),
                median_seconds=statistics.median(v),
                min_seconds=min(v),
                max_seconds=max(v),
                stdev_seconds=statistics.stdev(v) if len(v) > 1 else 0,
                peak_rss_bytes=max(r["peak_rss_bytes"] for r in rows),
                observed=[r["observed"] for r in rows],
            )
        )
    result = dict(
        checks=checks,
        summary=summaries,
        runs=runs,
        independent_probes=probe_runs,
        environment=dict(
            platform=platform.platform(),
            python=sys.version,
            rustc=subprocess.check_output(["rustc", "--version"], text=True).strip(),
            source_sha256=hashlib.sha256(
                b"".join(
                    p.name.encode() + p.read_bytes()
                    for p in sorted((REPO / "src").glob("*.rs"))
                )
            ).hexdigest(),
            fixture_sha256={
                p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                for p in sorted(server.root.glob("*.tif"))
            },
            rasterio=rasterio.__version__,
            gdal=rasterio.__gdal_version__,
            rust_gdal=subprocess.check_output(
                ["gdal-config", "--version"], text=True
            ).strip(),
            cargo_lock_sha256=hashlib.sha256(
                (REPO / "Cargo.lock").read_bytes()
            ).hexdigest(),
        ),
        limitations=[
            "loopback HTTP, one machine, warm OS cache; no throughput superiority claim",
            "GDAL read and decode fused; server response time is not isolated client transfer latency",
            "source metadata immutable in fixture; production remote cache remains index-free",
        ],
    )
    (root / "results.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(dict(checks=checks, summary=summaries), indent=2))


if __name__ == "__main__":
    main()
