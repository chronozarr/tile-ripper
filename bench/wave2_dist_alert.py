#!/usr/bin/env python3
"""Bounded DIST-ALERT V1 independent capture and offline/native qualification.

Capture requires an explicitly supplied mode-0600 Earthdata netrc. Credentials
are sent only to URS. Temporary resource queries stay in memory. Repeated
verification uses sparse, exact-offset mirrors and makes no network requests.
No Tile-Ripper scientific reader supplies the independent rasterio references.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
import math
import netrc
import os
import re
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urljoin, urlsplit

import numpy as np
import rasterio
import requests
import tifffile
from rasterio.features import geometry_mask
from rasterio.warp import transform_bounds, transform_geom
from rasterio.windows import Window

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/wave2/dist-alert"
DOC = ROOT / "docs/products/wave2"
COLLECTION = "OPERA_L3_DIST-ALERT-HLS_V1"
CONCEPT = "C2746980408-LPCLOUD"
GRANULE = "OPERA_L3_DIST-ALERT-HLS_T17SPU_20261021T160701Z_20261023T063220Z_S2A_30_v1"
ARCHIVE = "https://data.lpdaac.earthdatacloud.nasa.gov"
LOGIN = "https://urs.earthdata.nasa.gov"
RESOURCE = "https://d1nklfio7vscoe.cloudfront.net"
GATE = (
    "/s3-2d2df3a34830d5223d1e9547cd713408/lp-prod-protected.s3.us-west-2.amazonaws.com/"
)
BASE = f"{ARCHIVE}/lp-prod-protected/{COLLECTION}/{GRANULE}/{GRANULE}"
LAYERS = [
    "VEG-DIST-STATUS",
    "GEN-DIST-STATUS",
    "VEG-DIST-CONF",
    "GEN-DIST-CONF",
    "VEG-DIST-DATE",
    "GEN-DIST-DATE",
    "VEG-LAST-DATE",
    "GEN-LAST-DATE",
    "DATA-MASK",
]
WINDOWS = [(1792, 1792, 32, 32), (3628, 3628, 32, 32)]
MAX_CAPTURE = 4 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def digest(body):
    return hashlib.sha256(body).hexdigest()


def origin(url):
    u = urlsplit(url)
    return f"{u.scheme}://{u.netloc}"


def session():
    s = requests.Session()
    s.trust_env = False
    s.headers.update(
        {"Accept-Encoding": "identity", "User-Agent": "tile-ripper-dist-independent/1"}
    )
    return s


def resolve(canonical, credentials):
    """Check an exact LPDAAC directed redirect chain, with isolated cookies."""
    path = urlsplit(canonical).path
    resource_path = GATE + path.removeprefix("/lp-prod-protected/")
    sessions = {key: session() for key in (ARCHIVE, LOGIN, RESOURCE)}
    current = canonical
    trace = []
    for _ in range(8):
        src = origin(current)
        p = urlsplit(current).path
        require(src in sessions, "DIST redirect escaped admitted origins")
        require(
            (src == ARCHIVE and p in (path, "/login"))
            or (src == LOGIN and p == "/oauth/authorize")
            or (src == RESOURCE and p == resource_path),
            "DIST redirect escaped admitted paths",
        )
        r = sessions[src].get(
            current,
            headers={"Range": "bytes=0-0"} if p in (path, resource_path) else {},
            auth=credentials if src == LOGIN else None,
            allow_redirects=False,
            stream=True,
            timeout=(10, 30),
        )
        if r.status_code in (301, 302, 303, 307, 308):
            target = urljoin(current, r.headers.get("Location", ""))
            dst, dp = origin(target), urlsplit(target).path
            require(
                (src == ARCHIVE and dst == LOGIN and dp == "/oauth/authorize")
                or (src == LOGIN and dst == ARCHIVE and dp == "/login")
                or (src == ARCHIVE and dst == ARCHIVE and dp == path)
                or (src == ARCHIVE and dst == RESOURCE and dp == resource_path),
                "DIST redirect transition changed",
            )
            trace.append(
                {
                    "from_origin": src,
                    "to_origin": dst,
                    "status": r.status_code,
                    "temporary_query_present": bool(urlsplit(target).query),
                }
            )
            r.close()
            current = target
            continue
        require(
            src == RESOURCE and r.status_code == 206,
            "DIST exact resource did not return HTTP 206",
        )
        match = re.fullmatch(r"bytes 0-0/(\d+)", r.headers.get("Content-Range", ""))
        require(match is not None, "DIST resource length unavailable")
        etag = r.headers.get("ETag", "")
        require(
            etag.startswith('"') and etag.endswith('"'),
            "DIST resource lacks strong ETag",
        )
        require(len(r.raw.read(2)) == 1, "DIST probe body changed")
        r.close()
        return current, etag, int(match[1]), trace
    raise RuntimeError("DIST redirect budget exceeded")


def fetch_range(url, etag, size, start, end):
    with session().get(
        url,
        headers={"Range": f"bytes={start}-{end}", "If-Match": etag},
        stream=True,
        allow_redirects=False,
        timeout=(10, 30),
    ) as r:
        require(r.status_code == 206, "DIST capture range status changed")
        require(r.headers.get("ETag") == etag, "DIST capture ETag changed")
        require(
            r.headers.get("Content-Range") == f"bytes {start}-{end}/{size}",
            "DIST capture range framing changed",
        )
        body = r.raw.read(end - start + 2)
        require(len(body) == end - start + 1, "DIST capture range length changed")
        return body


def reference(path, window):
    with rasterio.open(path) as ds:
        a = ds.read(1, window=Window(*window))
        values, counts = np.unique(a, return_counts=True)
        tags = ds.tags()
        nodata = ds.nodata
        valid = np.ones(a.shape, dtype=bool) if nodata is None else a != nodata
        return {
            "source_shape": [ds.height, ds.width],
            "dtype": str(a.dtype),
            "nodata": nodata,
            "crs": ds.crs.to_string(),
            "transform": list(ds.transform)[:6],
            "block_shape": list(ds.block_shapes[0]),
            "overviews": ds.overviews(1),
            "compression": ds.compression.name,
            "scale": ds.scales[0],
            "offset": ds.offsets[0],
            "units": ds.units[0],
            "metadata": tags,
            "window": list(window),
            "unique_counts": {str(int(v)): int(n) for v, n in zip(values, counts)},
            "valid_count": int(valid.sum()),
            "raw_sha256": digest(a.tobytes()),
            "raw_base64": base64.b64encode(a.tobytes()).decode("ascii"),
            "source_valid_sha256": digest(valid.astype("uint8").tobytes()),
            "sample_first_4x8": a[:4, :8].tolist(),
        }


def public_reference(row):
    return {k: v for k, v in row.items() if k != "raw_base64"}


def capture(netrc_path):
    mode = netrc_path.stat().st_mode
    require(
        stat.S_ISREG(mode) and mode & 0o077 == 0,
        "explicit netrc must be private regular file",
    )
    a = netrc.netrc(str(netrc_path)).authenticators("urs.earthdata.nasa.gov")
    require(a and a[0] and a[2], "explicit netrc lacks URS credentials")
    credentials = (a[0], a[2])
    WORK.mkdir(parents=True, exist_ok=True)
    collection_url = f"https://cmr.earthdata.nasa.gov/search/collections.umm_json?concept_id={CONCEPT}"
    granule_url = f"https://cmr.earthdata.nasa.gov/search/granules.umm_json?collection_concept_id={CONCEPT}&granule_ur={GRANULE}"
    metadata = {}
    for key, url in (("collection", collection_url), ("granule", granule_url)):
        r = session().get(url, timeout=30)
        require(r.status_code == 200, "CMR inspection failed")
        value = r.json()
        require(value["hits"] == 1, "CMR identity is ambiguous")
        metadata[key] = value["items"][0]
        save(WORK / f"cmr-{key}.json", value["items"][0])
    rows, total = [], 0
    for name in LAYERS:
        canonical = f"{BASE}_{name}.tif"
        cache = WORK / "captured-rows" / f"{name}.json"
        if cache.exists():
            row = json.loads(cache.read_text())
            require(row["href"] == canonical, "DIST cached canonical identity changed")
            if row.get("tile_guard_bytes") != 4:
                resource, etag, size, _ = resolve(canonical, credentials)
                require(
                    etag == row["etag"] and size == row["size"],
                    "DIST cached source mutated",
                )
                ranges = [row["ranges"][0]]
                with (ROOT / row["mirror"]).open("r+b") as stream:
                    for old in row["ranges"][1:]:
                        start, end = (
                            max(0, old["start"] - 4),
                            min(size - 1, old["end"] + 4),
                        )
                        body = fetch_range(resource, etag, size, start, end)
                        stream.seek(start)
                        stream.write(body)
                        ranges.append(
                            {
                                "start": start,
                                "end": end,
                                "bytes": len(body),
                                "sha256": digest(body),
                            }
                        )
                row["ranges"] = ranges
                row["tile_guard_bytes"] = 4
                save(cache, row)
            rows.append(row)
            total += sum(r["bytes"] for r in row["ranges"])
            print(f"{name}: using frozen captured ranges", flush=True)
            continue
        try:
            resource, etag, size, trace = resolve(canonical, credentials)
        except (RuntimeError, requests.RequestException):
            resource, etag, size, trace = resolve(canonical, credentials)
        head = fetch_range(resource, etag, size, 0, min(65535, size - 1))
        total += len(head)
        with tifffile.TiffFile(io.BytesIO(head)) as tiff:
            page = tiff.pages[0]
            require(page.shape == (3660, 3660), "DIST shape changed")
            selected = set()
            nx = math.ceil(page.imagewidth / page.tilewidth)
            for x, y, width, height in WINDOWS:
                for ty in range(
                    y // page.tilelength, (y + height - 1) // page.tilelength + 1
                ):
                    for tx in range(
                        x // page.tilewidth, (x + width - 1) // page.tilewidth + 1
                    ):
                        selected.add(ty * nx + tx)
            ranges = [
                (
                    max(0, page.dataoffsets[i] - 4),
                    min(size - 1, page.dataoffsets[i] + page.databytecounts[i] + 3),
                )
                for i in sorted(selected)
            ]
            physical = {
                "bits_per_sample": page.bitspersample,
                "sample_format": int(page.sampleformat),
                "compression_code": int(page.compression),
                "predictor": int(page.tags[317].value) if 317 in page.tags else 1,
                "photometric": int(page.photometric),
                "geokey_directory": list(page.tags[34735].value),
                "model_pixel_scale": list(page.tags[33550].value),
                "model_tiepoint": list(page.tags[33922].value),
                "gdal_metadata_xml": page.tags[42112].value,
                "gdal_nodata": page.tags[42113].value if 42113 in page.tags else None,
            }
        pieces = [(0, head)]
        for start, end in ranges:
            require(
                total + end - start + 1 <= MAX_CAPTURE,
                "DIST capture byte budget exceeded",
            )
            body = fetch_range(resource, etag, size, start, end)
            total += len(body)
            pieces.append((start, body))
        path = WORK / "range-mirrors" / f"{name}.tif"
        path.parent.mkdir(exist_ok=True)
        with path.open("wb") as stream:
            stream.truncate(size)
            for start, body in pieces:
                stream.seek(start)
                stream.write(body)
        refs = [reference(path, w) for w in WINDOWS]
        for ref in refs:
            require(
                ref["crs"] == "EPSG:32617"
                and ref["transform"] == [30, 0, 600000, 0, -30, 3900000],
                "DIST native grid changed",
            )
        receipts = [
            {
                "start": start,
                "end": start + len(body) - 1,
                "bytes": len(body),
                "sha256": digest(body),
            }
            for start, body in pieces
        ]
        row = {
            "tile_guard_bytes": 4,
            "name": name,
            "href": canonical,
            "etag": etag,
            "size": size,
            "mirror": str(path.relative_to(ROOT)),
            "ranges": receipts,
            "physical": physical,
            "references": refs,
            "redirect_trace": trace,
        }
        rows.append(row)
        save(cache, row)
        print(
            f"{name}: {sum(len(b) for _, b in pieces)} source bytes; dtype {refs[0]['dtype']}; fill {refs[0]['nodata']}; codes {refs[0]['unique_counts']}"
        )
    manifest = {
        "schema": "tile-ripper-wave2-dist-alert-v1",
        "collection": COLLECTION,
        "collection_concept_id": CONCEPT,
        "granule": GRANULE,
        "captured_source_bytes": total,
        "capture_byte_cap": MAX_CAPTURE,
        "reader": {
            "rasterio": rasterio.__version__,
            "GDAL": rasterio.__gdal_version__,
            "tifffile": tifffile.__version__,
        },
        "metadata": metadata,
        "assets": rows,
    }
    save(WORK / "manifest.json", manifest)
    public = json.loads(json.dumps(manifest))
    for row in public["assets"]:
        row["references"] = [public_reference(r) for r in row["references"]]
    save(DOC / "dist-alert-inspection.json", public)
    make_request(manifest)


def make_request(manifest):
    item = manifest["metadata"]["granule"]["umm"]
    attrs = {x["Name"]: x["Values"] for x in item["AdditionalAttributes"]}
    sensing = attrs["SENSING_TIME"][0]
    with rasterio.open(ROOT / manifest["assets"][0]["mirror"]) as ds:
        bbox = list(
            transform_bounds(
                ds.crs,
                "EPSG:4326",
                *rasterio.windows.bounds(Window(1808, 1808, 32, 32), ds.transform),
            )
        )
    request = {
        "product": "opera-dist-alert",
        "aoi": {"kind": "bounds", "crs": "EPSG:4326", "bbox": bbox},
        "start": "2026-10-21T00:00:00Z",
        "end": "2026-10-21T23:59:59Z",
        "bands": LAYERS,
        "quality_policies": [
            "dist_veg_confirmed",
            "dist_gen_confirmed",
            "dist_current_land",
        ],
        "observations": [
            {
                "id": GRANULE,
                "datetime": sensing,
                "collection": COLLECTION,
                "platform": "sentinel-2a",
                "processing": "L3",
                "properties": {k: v[0] if len(v) == 1 else v for k, v in attrs.items()},
                "assets": {
                    row["name"]: {
                        "href": row["href"],
                        "etag": row["etag"],
                        "metadata": {},
                    }
                    for row in manifest["assets"]
                },
            }
        ],
    }
    save(DOC / "dist-alert-request.json", request)


def verify():
    manifest = json.loads((WORK / "manifest.json").read_text())
    for row in manifest["assets"]:
        path = ROOT / row["mirror"]
        require(path.stat().st_size == row["size"], "DIST mirror size changed")
        with path.open("rb") as stream:
            for receipt in row["ranges"]:
                stream.seek(receipt["start"])
                require(
                    digest(stream.read(receipt["bytes"])) == receipt["sha256"],
                    "DIST frozen range changed",
                )
        for expected in row["references"]:
            require(
                reference(path, expected["window"]) == expected,
                "DIST independent reference changed",
            )
        print(f"{row['name']}: PASS (offline rasterio)")
    return manifest


def replay(binary, output, edge=False):
    """Run ordinary CLI against guarded offline mirrors, independently reopen Zarr."""
    manifest = verify()
    sources = manifest["assets"]
    records = []
    state = {
        "mode": None,
        "data_read": False,
        "fault_seen": False,
        "mutation_seen": False,
    }
    blocked, release = threading.Event(), threading.Event()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_HEAD(self):
            self.serve(False)

        def do_GET(self):
            self.serve(True)

        def serve(self, body):
            index = self.path.removesuffix(".tif").removeprefix("/")
            if not index.isdigit() or int(index) >= len(sources):
                self.send_error(404)
                return
            row = sources[int(index)]
            if self.headers.get("If-Match") not in (None, row["etag"]):
                self.send_error(412)
                return
            start, end = 0, row["size"] - 1
            match = re.fullmatch(r"bytes=(\d+)-(\d+)", self.headers.get("Range", ""))
            if body:
                if not match:
                    self.send_error(416)
                    return
                start, end = map(int, match.groups())
                if not any(
                    start >= p["start"] and end <= p["end"] for p in row["ranges"]
                ):
                    records.append(
                        {
                            "rejected": True,
                            "source": row["name"],
                            "start": start,
                            "end": end,
                        }
                    )
                    self.send_error(416)
                    return
            if body and start > 65535:
                if state["mode"] == "fault":
                    state["fault_seen"] = True
                    self.send_error(503)
                    return
                if state["mode"] == "cancel":
                    blocked.set()
                    release.wait(5)
                state["data_read"] = True
            response_etag = (
                '"mutated"'
                if state["mode"] == "mutation" and state["data_read"] and not body
                else row["etag"]
            )
            if response_etag != row["etag"]:
                state["mutation_seen"] = True
            self.send_response(206 if body else 200)
            self.send_header("Content-Length", str(end - start + 1))
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("ETag", response_etag)
            if body:
                self.send_header("Content-Range", f"bytes {start}-{end}/{row['size']}")
            self.end_headers()
            records.append(
                {
                    "method": "GET" if body else "HEAD",
                    "source": row["name"],
                    "start": start,
                    "end": end,
                }
            )
            if body:
                with (ROOT / row["mirror"]).open("rb") as stream:
                    stream.seek(start)
                    try:
                        self.wfile.write(stream.read(end - start + 1))
                    except (BrokenPipeError, ConnectionResetError):
                        pass

    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    try:
        request = json.loads((DOC / "dist-alert-request.json").read_text())
        if edge:
            with rasterio.open(ROOT / sources[0]["mirror"]) as ds:
                request["aoi"]["bbox"] = list(
                    transform_bounds(
                        ds.crs,
                        "EPSG:4326",
                        *rasterio.windows.bounds(
                            Window(3628, 3628, 32, 32), ds.transform
                        ),
                    )
                )
        request["quality_policies"] = [
            "dist_veg_confirmed",
            "dist_gen_confirmed",
            "dist_veg_confirmed_ongoing",
            "dist_gen_confirmed_ongoing",
            "dist_current_land",
            "dist_current_assessed",
        ]
        for i, row in enumerate(sources):
            request["observations"][0]["assets"][row["name"]]["href"] = (
                f"http://127.0.0.1:{srv.server_port}/{i}.tif"
            )
            request["observations"][0]["assets"][row["name"]]["metadata"][
                "canonical_href"
            ] = row["href"]
        runtime = WORK / (output.name + "-request-runtime.json")
        save(runtime, request)
        require(not output.exists(), "DIST replay output already exists")
        planned = invoke(
            [
                str(binary.resolve()),
                "plan",
                str(runtime),
                "--backend",
                "native",
                "--json",
            ]
        )
        require(planned["returncode"] == 0, "DIST ordinary plan failed")
        plan = json.loads(planned.pop("stdout"))
        records.clear()
        streamed = invoke(
            [
                str(binary.resolve()),
                "stream",
                str(runtime),
                "--backend",
                "native",
                "--json",
                "--values",
            ],
            streaming=True,
        )
        require(streamed["returncode"] == 0, "DIST ordinary stream failed")
        events = [json.loads(line) for line in streamed.pop("stdout").splitlines()]
        require(
            sum(e.get("event") == "chunk" for e in events) == 9,
            "DIST stream omitted native chunks",
        )
        stream_science = validate_stream(events, request, manifest)
        stream_requests = list(records)
        records.clear()
        proc = invoke(
            [
                str(binary.resolve()),
                "fetch",
                str(runtime),
                "--output",
                str(output),
                "--output-format",
                "zarr_v2",
                "--backend",
                "native",
                "--json",
            ]
        )
        if proc["returncode"]:
            print(proc["stderr"][-4000:])
            print(json.dumps(records[-20:], indent=2))
        require(proc["returncode"] == 0, "DIST native replay failed")
        json.loads(proc.pop("stdout"))
        comparisons = validate_output(output, request, manifest)
        if edge:
            require(
                next(c for c in comparisons if c["band"] == "DATA-MASK")[
                    "source_valid_zero_count"
                ]
                > 0,
                "DIST edge fixture missed current no-data/masked class",
            )
        native = json.loads((output / "manifest.json").read_text())
        fetch_requests = list(records)
        reference_output = output.with_name(output.name + ".rasterio-reference")
        mature = invoke(
            [
                sys.executable,
                str(Path(__file__).resolve()),
                "reference",
                "--native-manifest",
                str(output / "manifest.json"),
                "--request",
                str(runtime),
                "--output",
                str(reference_output),
            ]
        )
        require(
            mature["returncode"] == 0,
            "DIST independent complete rasterio writer failed",
        )
        mature_metrics = json.loads(mature.pop("stdout"))
        import zarr

        n, r = (
            zarr.open_group(str(output), mode="r"),
            zarr.open_group(str(reference_output), mode="r"),
        )
        for record in native["outputs"]:
            left, right = n[record["group"]], r[record["group"]]
            require(
                set(left.array_keys()) == set(right.array_keys()),
                "DIST independent complete output arrays differ",
            )
            for key in left.array_keys():
                a, b = left[key][...], right[key][...]
                require(
                    a.dtype == b.dtype
                    and a.shape == b.shape
                    and a.tobytes() == b.tobytes(),
                    "DIST independent complete output values differ",
                )
        faults = {}
        for mode in ("fault", "mutation"):
            state.update(mode=mode, data_read=False)
            failed = output.with_name(output.name + "." + mode)
            result = invoke(
                [
                    str(binary.resolve()),
                    "fetch",
                    str(runtime),
                    "--output",
                    str(failed),
                    "--backend",
                    "native",
                    "--json",
                ]
            )
            require(
                result["returncode"] != 0 and not failed.exists(),
                f"DIST {mode} published output",
            )
            require(state[mode + "_seen"], "DIST injected failure was not exercised")
            faults[mode] = {
                "nonzero_exit": True,
                "no_published_output": True,
                "injection_observed": True,
            }
        state.update(mode="cancel", data_read=False)
        cancelled = output.with_name(output.name + ".cancel")
        with tempfile.TemporaryFile() as cout, tempfile.TemporaryFile() as cerr:
            child = subprocess.Popen(
                [
                    str(binary.resolve()),
                    "fetch",
                    str(runtime),
                    "--output",
                    str(cancelled),
                    "--backend",
                    "native",
                    "--json",
                ],
                stdout=cout,
                stderr=cerr,
            )
            require(
                blocked.wait(10), "DIST cancellation did not reach held data request"
            )
            child.send_signal(signal.SIGINT)
            time.sleep(0.05)
            release.set()
            code = child.wait(timeout=10)
        require(
            code != 0 and not cancelled.exists(), "DIST cancellation published output"
        )
        faults["cancel"] = {
            "interrupted_during_held_source_read": True,
            "no_published_output": True,
        }
        state["mode"] = None
        from wave1_products import work_metrics

        class Wire:
            records = fetch_requests

        report = {
            "granule": GRANULE,
            "scope": "guarded frozen exact-offset COGs; mature-reader native values/masks/coordinates and independent Zarr/xarray reopen",
            "plan_process": planned,
            "planning_seconds": plan["planning_seconds"],
            "stream_science": stream_science,
            "stream_process": streamed,
            "stream_requests": stream_requests,
            "fetch_process": proc,
            "native_writer": native["observed"],
            "native_completion": native["completion"],
            "physical_work": work_metrics(native, Wire()),
            "mature_reference_process": mature,
            "mature_reference_output": mature_metrics,
            "complete_outputs_equal_bits": True,
            "comparisons": comparisons,
            "faults": faults,
            "http_requests": fetch_requests,
            "http_body_bytes": sum(
                x["end"] - x["start"] + 1
                for x in fetch_requests
                if x["method"] == "GET"
            ),
            "output_objects": sum(p.is_file() for p in output.rglob("*")),
            "output_bytes": sum(
                p.stat().st_size for p in output.rglob("*") if p.is_file()
            ),
            "binary_sha256": digest(binary.read_bytes()),
            "phase_scope": "native plan, stream and fetch are separate fresh executions; source capture/auth/discovery measured separately; reference elapsed includes Python startup, local sparse raster reads and complete equivalent arrays written to Zarr; reference writes smaller metadata than the native provenance/physical-index manifest, so timing is not a matched total-metadata workload",
        }
        save(
            DOC
            / (
                "dist-alert-validation-edge.json"
                if edge
                else "dist-alert-validation.json"
            ),
            report,
        )
    finally:
        srv.shutdown()
        srv.server_close()


def validate_output(output, request, frozen):
    import xarray as xr
    import zarr

    native = json.loads((output / "manifest.json").read_text())
    require(native["status"] == "complete", "DIST output is incomplete")
    zg = zarr.open_group(str(output), mode="r")
    results = []
    by_name = {row["name"]: row for row in frozen["assets"]}
    for record in native["outputs"]:
        name = record["asset_id"].removeprefix("p000_")
        row = by_name[name]
        with (
            xr.open_zarr(
                str(output),
                group=record["group"],
                consolidated=False,
                decode_cf=False,
                chunks=None,
            ) as ds,
            rasterio.open(ROOT / row["mirror"]) as src,
        ):
            ds.load()
            w = record["window"]
            win = Window(w["x"], w["y"], w["width"], w["height"])
            values = src.read(1, window=win)
            west, south, east, north = request["aoi"]["bbox"]
            polygon = {
                "type": "Polygon",
                "coordinates": [
                    [
                        [west, south],
                        [east, south],
                        [east, north],
                        [west, north],
                        [west, south],
                    ]
                ],
            }
            aoi = geometry_mask(
                [transform_geom("EPSG:4326", src.crs, polygon)],
                out_shape=values.shape,
                transform=src.window_transform(win),
                invert=True,
            )
            if not record["aoi_mask_applied"]:
                aoi[:] = True
            elif src.nodata is not None:
                values[~aoi] = src.nodata
            require(
                ds["values"].dtype == values.dtype
                and ds["values"].values[0].tobytes() == values.tobytes(),
                f"{name}: native stored values changed",
            )
            valid = aoi if src.nodata is None else aoi & (values != src.nodata)
            np.testing.assert_array_equal(ds["valid"].values[0], valid.astype("uint8"))
            np.testing.assert_array_equal(ds["aoi_mask"].values[0], aoi.astype("uint8"))
            affine = src.window_transform(win)
            np.testing.assert_allclose(
                ds.x.values,
                affine.c + (np.arange(w["width"]) + 0.5) * affine.a,
                rtol=0,
                atol=1e-8,
            )
            np.testing.assert_allclose(
                ds.y.values,
                affine.f + (np.arange(w["height"]) + 0.5) * affine.e,
                rtol=0,
                atol=1e-8,
            )
            require(
                zg[record["group"]].attrs["crs"] == src.crs.to_string(),
                "DIST output CRS changed",
            )
            for policy in request["quality_policies"]:
                key = f"quality_{policy}"
                if key in ds:
                    expected = valid & (
                        np.isin(
                            values,
                            [3, 6] if policy.endswith("ongoing") else [3, 6, 7, 8],
                        )
                        if name.endswith("STATUS")
                        else np.isin(
                            values, [1, 2] if policy.endswith("assessed") else [1]
                        )
                    )
                    np.testing.assert_array_equal(
                        ds[key].values[0], expected.astype("uint8")
                    )
            source = next(
                x for x in native["sources"] if x["asset_id"] == record["asset_id"]
            )
            require(
                source["object_version"] == row["etag"]
                and source["item_id"] == GRANULE
                and source["product_version"] == "1",
                "DIST native provenance changed",
            )
            physical = source["object_identities"]["native_cog"]
            require(
                physical["gdal_metadata_xml"].rstrip("\0\n")
                == row["physical"]["gdal_metadata_xml"].rstrip("\0\n")
                and physical["geokey_directory"] == row["physical"]["geokey_directory"],
                "DIST physical metadata provenance changed",
            )
            unit = "days since 2020-12-31" if name.endswith("DATE") else "1"
            require(
                ds["values"].attrs["units"] == unit
                and ds["values"].attrs["scale_factor"] == 1.0
                and ds["values"].attrs["add_offset"] == 0.0,
                "DIST calibration or date epoch changed",
            )
            stamp = np.datetime64(
                request["observations"][0]["datetime"].removesuffix("Z"), "ns"
            ).astype("int64")
            require(ds.time.values[0] == stamp, "DIST source sensing timestamp changed")
            results.append(
                {
                    "band": name,
                    "window": w,
                    "dtype": str(values.dtype),
                    "values_sha256": digest(values.tobytes()),
                    "raw_values_equal_bits": True,
                    "source_valid_and_aoi_equal": True,
                    "coordinates_equal": True,
                    "quality_arrays": [k for k in ds if k.startswith("quality_")],
                    "valid_count": int(valid.sum()),
                    "source_valid_zero_count": int(
                        np.count_nonzero(valid & (values == 0))
                    ),
                }
            )
    require(len(results) == len(LAYERS), "DIST native output omitted admitted layers")
    return results


def validate_stream(events, request, frozen):
    rows = {r["name"]: r for r in frozen["assets"]}
    checks = []
    for event in events:
        if event.get("event") != "chunk":
            continue
        name = event["band"]
        w = event["window"]
        with rasterio.open(ROOT / rows[name]["mirror"]) as src:
            win = Window(w["x"], w["y"], w["width"], w["height"])
            reference = src.read(1, window=win)
            actual = np.frombuffer(
                base64.b64decode(event["values_le_base64"]),
                dtype=reference.dtype.newbyteorder("<"),
            ).reshape(reference.shape)
            aoi = (
                np.frombuffer(base64.b64decode(event["aoi_mask_base64"]), dtype="uint8")
                .reshape(reference.shape)
                .astype(bool)
            )
            require(
                actual[aoi].tobytes() == reference[aoi].tobytes(),
                "DIST stream native included values changed",
            )
            source_valid = (
                np.ones(reference.shape, dtype=bool)
                if src.nodata is None
                else reference != src.nodata
            )
            for policy, result in event["quality"].items():
                accept = [3, 6] if policy.endswith("ongoing") else [3, 6, 7, 8]
                if name == "DATA-MASK":
                    accept = [1, 2] if policy.endswith("assessed") else [1]
                expected = (aoi & source_valid & np.isin(reference, accept)).astype(
                    "uint8"
                )
                require(
                    base64.b64decode(result["mask_base64"]) == expected.tobytes(),
                    "DIST stream policy changed",
                )
            checks.append(
                {
                    "band": name,
                    "included_native_values_equal_bits": True,
                    "quality_masks_equal": True,
                    "dtype": event["dtype"],
                    "units": event["units"],
                    "window": w,
                }
            )
    require(
        len(checks) == 9 and any(e.get("event") == "complete" for e in events),
        "DIST stream lacks terminal completion",
    )
    return checks


def invoke(command, streaming=False):
    start = time.perf_counter()
    first = None
    lines = []
    with tempfile.TemporaryFile() as stderr:
        child = subprocess.Popen(
            command, stdout=subprocess.PIPE, stderr=stderr, text=True
        )
        for line in child.stdout:
            lines.append(line)
            if streaming and first is None and json.loads(line).get("event") == "chunk":
                first = time.perf_counter() - start
        child.stdout.close()
        _, status, usage = os.wait4(child.pid, 0)
        child.returncode = os.waitstatus_to_exitcode(status)
        stderr.seek(0)
        return {
            "returncode": child.returncode,
            "stdout": "".join(lines),
            "stderr": stderr.read().decode(),
            "wall_seconds": time.perf_counter() - start,
            "first_chunk_seconds": first,
            "cpu_seconds": usage.ru_utime + usage.ru_stime,
            "peak_rss_bytes": usage.ru_maxrss
            * (1 if sys.platform == "darwin" else 1024),
        }


def complete_reference(native_manifest, request_path, output):
    """Independent rasterio/NumPy masks and Zarr writer; plan windows only reused."""
    import zarr

    require(not output.exists(), "DIST independent output already exists")
    frozen = json.loads((WORK / "manifest.json").read_text())
    native = json.loads(native_manifest.read_text())
    request = json.loads(request_path.read_text())
    by_name = {r["name"]: r for r in frozen["assets"]}
    root = zarr.open_group(str(output), mode="w")
    west, south, east, north = request["aoi"]["bbox"]
    polygon = {
        "type": "Polygon",
        "coordinates": [
            [[west, south], [east, south], [east, north], [west, north], [west, south]]
        ],
    }
    ns = np.datetime64(
        request["observations"][0]["datetime"].removesuffix("Z"), "ns"
    ).astype("int64")
    for rec in native["outputs"]:
        name = rec["asset_id"].removeprefix("p000_")
        with rasterio.open(ROOT / by_name[name]["mirror"]) as src:
            w = rec["window"]
            win = Window(w["x"], w["y"], w["width"], w["height"])
            affine = src.window_transform(win)
            values = src.read(1, window=win)
            aoi = geometry_mask(
                [transform_geom("EPSG:4326", src.crs, polygon)],
                out_shape=values.shape,
                transform=affine,
                invert=True,
            )
            if not rec["aoi_mask_applied"]:
                aoi[:] = True
            elif src.nodata is not None:
                values[~aoi] = src.nodata
            valid = aoi if src.nodata is None else aoi & (values != src.nodata)
            arrays = {
                "values": values[None],
                "aoi_mask": aoi.astype("uint8")[None],
                "valid": valid.astype("uint8")[None],
                "x": affine.c + (np.arange(w["width"]) + 0.5) * affine.a,
                "y": affine.f + (np.arange(w["height"]) + 0.5) * affine.e,
                "time": np.asarray([ns], dtype="<i8"),
                "spatial_ref": np.asarray(0, dtype="<i4"),
            }
            for policy in request["quality_policies"]:
                relevant = (
                    (name == "VEG-DIST-STATUS" and policy.startswith("dist_veg"))
                    or (name == "GEN-DIST-STATUS" and policy.startswith("dist_gen"))
                    or (name == "DATA-MASK" and policy.startswith("dist_current"))
                )
                if relevant:
                    accept = [3, 6] if policy.endswith("ongoing") else [3, 6, 7, 8]
                    if name == "DATA-MASK":
                        accept = [1, 2] if policy.endswith("assessed") else [1]
                    arrays["quality_" + policy] = (
                        valid & np.isin(values, accept)
                    ).astype("uint8")[None]
            g = root.require_group(rec["group"])
            g.attrs.update({"crs": src.crs.to_string(), "transform": list(affine)[:6]})
            for key, array in arrays.items():
                z = g.create_dataset(key, data=array, compressor=None, fill_value=None)
                z.attrs["_ARRAY_DIMENSIONS"] = (
                    ["time", "y", "x"]
                    if array.ndim == 3
                    else [key]
                    if array.ndim == 1
                    else []
                )
                if key == "values":
                    z.attrs.update(
                        {
                            "units": "days since 2020-12-31"
                            if name.endswith("DATE")
                            else "1",
                            "scale_factor": 1.0,
                            "add_offset": 0.0,
                            "source_nodata": src.nodata,
                        }
                    )
    files = [p for p in output.rglob("*") if p.is_file()]
    print(
        json.dumps(
            {
                "writer": "independent rasterio/NumPy/Zarr-Python",
                "output_objects": len(files),
                "output_bytes": sum(p.stat().st_size for p in files),
                "arrays": "values,aoi_mask,valid,x,y,time,spatial_ref and each requested relevant policy",
            }
        )
    )


def live(binary, output, netrc_path):
    require(netrc_path is not None, "live requires explicit --netrc")
    mode = netrc_path.stat().st_mode
    require(
        stat.S_ISREG(mode) and mode & 0o077 == 0,
        "explicit netrc must be private regular file",
    )
    require(not output.exists(), "DIST live output already exists")
    request = json.loads((DOC / "dist-alert-request.json").read_text())
    bbox = ",".join(map(str, request["aoi"]["bbox"]))
    discover = invoke(
        [
            str(binary.resolve()),
            "discover",
            "--product",
            "opera-dist-alert",
            "--bbox",
            bbox,
            "--start",
            request["start"],
            "--end",
            request["end"],
            "--bands",
            ",".join(LAYERS),
            "--limit",
            "4",
        ]
    )
    require(discover["returncode"] == 0, "DIST ordinary live discovery failed")
    candidates = json.loads(discover.pop("stdout"))
    require(
        any(o["id"] == GRANULE for o in candidates["observations"]),
        "DIST discovery did not return exact pinned alert",
    )
    proc = invoke(
        [
            str(binary.resolve()),
            "fetch",
            str(DOC / "dist-alert-request.json"),
            "--output",
            str(output),
            "--output-format",
            "zarr_v2",
            "--backend",
            "native",
            "--auth",
            "earthdata",
            "--netrc",
            str(netrc_path),
            "--json",
        ]
    )
    if proc["returncode"]:
        print(re.sub(r"https?://[^\s]+", "[redacted URL]", proc["stderr"])[-1000:])
    require(proc["returncode"] == 0, "DIST ordinary authenticated live fetch failed")
    json.loads(proc.pop("stdout"))
    frozen = json.loads((WORK / "manifest.json").read_text())
    comparisons = validate_output(output, request, frozen)
    native = json.loads((output / "manifest.json").read_text())
    save(
        DOC / "dist-alert-live-validation.json",
        {
            "granule": GRANULE,
            "discovery_process": discover,
            "discovery": candidates,
            "fetch_process": proc,
            "comparisons": comparisons,
            "observed": native["observed"],
            "transport": native["inspection"].get("transport"),
            "scope": "ordinary authenticated HTTPS production fetch compared with independently decoded exact-version frozen rasterio references; no credentials, cookies or temporary resource URLs persisted",
            "binary_sha256": digest(binary.read_bytes()),
        },
    )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument(
        "action",
        choices=["capture", "verify", "replay", "reference", "live"],
        nargs="?",
        default="verify",
    )
    p.add_argument("--netrc", type=Path)
    p.add_argument("--edge", action="store_true")
    p.add_argument("--native-manifest", type=Path)
    p.add_argument("--request", type=Path)
    p.add_argument("--binary", type=Path, default=ROOT / "target/debug/tile-ripper")
    p.add_argument("--output", type=Path, default=WORK / "native.zarr")
    args = p.parse_args()
    if args.action == "capture":
        require(args.netrc is not None, "capture requires explicit --netrc")
        capture(args.netrc)
    elif args.action == "replay":
        replay(args.binary, args.output, args.edge)
    elif args.action == "reference":
        complete_reference(args.native_manifest, args.request, args.output)
    elif args.action == "live":
        live(args.binary, args.output, args.netrc)
    else:
        verify()


if __name__ == "__main__":
    try:
        main()
    except requests.RequestException:
        raise SystemExit(
            "DIST provider request failed; temporary URLs and credentials suppressed"
        ) from None
