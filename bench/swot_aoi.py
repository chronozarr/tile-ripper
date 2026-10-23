"""Independent PROJ/GEOS center oracle and bounded real SWOT AOI acceptance.

Frozen modes never contact NASA. Coordinates are read in separate guarded h5py
and NetCDF-C children. GEOS evaluates every source-global native center after
PROJ inverse transformation; neither the native window nor a local affine is
an input to the oracle. GDAL rasterizer differences are retained separately.
"""

import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

import numpy as np
import psutil
import pyproj
from rasterio.features import rasterize
from rasterio.transform import Affine
from rasterio.warp import transform as gdal_transform
import shapely
from shapely.geometry import Polygon, box, mapping
import xarray as xr
import zarr

import swot_discovery as discovery
import swot_job as job
from hdf5_selective_mirror import Mirror, server

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/swot-aoi"
DOCS = ROOT / "docs/swot-aoi"
ORACLE = ROOT / "target/release/examples/swot_aoi_oracle"
TOLERANCE = 1e-10
WORK.mkdir(parents=True, exist_ok=True)
DOCS.mkdir(parents=True, exist_ok=True)


def write(path, value):
    job.write(path, value)


def save(name, value):
    write(DOCS / f"{name}.json", value)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def oracle(request, name):
    directory = Path(tempfile.mkdtemp(prefix=name + "-", dir=WORK))
    write(directory / "request.json", request)
    env = os.environ.copy()
    env["TILE_RIPPER_HDF5_METADATA_POLICY"] = "page16k"
    started = time.perf_counter()
    process = subprocess.run(
        [str(ORACLE), str(directory / "request.json")],
        capture_output=True,
        env=env,
        timeout=120,
    )
    (directory / "stdout.json").write_bytes(process.stdout)
    (directory / "stderr.txt").write_bytes(process.stderr)
    assert process.returncode == 0, (name, process.stderr.decode())
    return json.loads(process.stdout), {
        "directory": str(directory),
        "wall_seconds": time.perf_counter() - started,
        "request_sha256": job.digest(directory / "request.json"),
        "stdout_sha256": sha(process.stdout),
    }


def geometry(aoi):
    if aoi["kind"] == "bounds":
        return box(*aoi["bbox"])
    return Polygon(aoi["coordinates"][0], aoi["coordinates"][1:])


def independent_mask(aoi, crs, y, x):
    """All full-source rows, no dependency on native derived window/bounds."""
    polygon = geometry(aoi)
    inverse = pyproj.Transformer.from_crs(crs, "EPSG:4326", always_xy=True)
    result = np.empty((len(y), len(x)), dtype="u1")
    for row, yy in enumerate(y):
        lon, lat = inverse.transform(x, np.full(len(x), yy))
        points = shapely.points(lon, lat)
        result[row] = shapely.covers(polygon, points) | (
            shapely.distance(polygon.boundary, points) <= TOLERANCE
        )
    positions = np.argwhere(result)
    window = None
    if len(positions):
        first, last = positions.min(axis=0), positions.max(axis=0)
        window = dict(
            start=first.tolist(), count=(last - first + 1).tolist(), step=[1, 1]
        )
    return result, window


def densified_projected(aoi, crs):
    forward = pyproj.Transformer.from_crs("EPSG:4326", crs, always_xy=True)
    rings = []
    poly = geometry(aoi)
    for ring in [poly.exterior, *poly.interiors]:
        points = []
        coords = np.array(ring.coords)
        for a, b in zip(coords[:-1], coords[1:]):
            n = max(1, int(np.ceil(np.max(np.abs(b - a)) / 0.0001)))
            points.extend(a + (b - a) * i / n for i in range(n))
        points.append(points[0])
        points = np.array(points)
        xx, yy = forward.transform(points[:, 0], points[:, 1])
        rings.append(list(zip(xx, yy)))
    return Polygon(rings[0], rings[1:])


def geometry_oracles():
    rows, discrepancies = [], []
    for crs, lon, lat in [
        ("EPSG:32619", -69, 42),
        ("EPSG:32601", -177, 68),
        ("EPSG:32755", 147, -35),
    ]:
        forward = pyproj.Transformer.from_crs("EPSG:4326", crs, always_xy=True)
        center = forward.transform(lon, lat)
        x = center[0] + np.arange(-20, 21) * 100.0
        y = center[1] + np.arange(-20, 21) * 100.0
        cases = {
            "closed-boundary": dict(
                kind="bounds",
                crs="EPSG:4326",
                bbox=[lon, lat, lon + 0.012, lat + 0.012],
            ),
            "tiny-zero-centers": dict(
                kind="bounds",
                crs="EPSG:4326",
                bbox=[lon + 0.0001, lat + 0.0001, lon + 0.00011, lat + 0.00011],
            ),
            "outside": dict(
                kind="bounds",
                crs="EPSG:4326",
                bbox=[lon + 0.05, lat + 0.05, lon + 0.06, lat + 0.06],
            ),
            "partial": dict(
                kind="bounds",
                crs="EPSG:4326",
                bbox=[lon - 0.05, lat - 0.05, lon + 0.001, lat + 0.001],
            ),
            "concave-hole": dict(
                kind="polygon",
                crs="EPSG:4326",
                coordinates=[
                    [
                        [lon - 0.012, lat - 0.012],
                        [lon + 0.012, lat - 0.011],
                        [lon + 0.011, lat + 0.003],
                        [lon + 0.004, lat + 0.003],
                        [lon + 0.003, lat + 0.013],
                        [lon - 0.012, lat + 0.012],
                        [lon - 0.012, lat - 0.012],
                    ],
                    [
                        [lon - 0.005, lat - 0.005],
                        [lon, lat - 0.005],
                        [lon, lat],
                        [lon - 0.005, lat],
                        [lon - 0.005, lat - 0.005],
                    ],
                ],
            ),
            "curved-geographic-edge": dict(
                kind="bounds",
                crs="EPSG:4326",
                bbox=[lon - 0.8, lat, lon + 0.8, lat + 0.03],
            ),
        }
        cases["inside-roundoff-band"] = dict(
            kind="bounds",
            crs="EPSG:4326",
            bbox=[lon + TOLERANCE * 0.5, lat, lon + 0.012, lat + 0.012],
        )
        cases["outside-roundoff-band"] = dict(
            kind="bounds",
            crs="EPSG:4326",
            bbox=[lon + TOLERANCE * 2.0, lat, lon + 0.012, lat + 0.012],
        )
        winding = copy.deepcopy(cases["concave-hole"])
        winding["coordinates"] = [
            list(reversed(ring)) for ring in winding["coordinates"]
        ]
        cases["concave-hole-reversed-winding"] = winding
        for name, aoi in cases.items():
            for reverse_y, reverse_x in [
                (False, False),
                (True, False),
                (False, True),
                (True, True),
            ]:
                yy, xx = (y[::-1] if reverse_y else y), (x[::-1] if reverse_x else x)
                points = np.array(geometry(aoi).exterior.coords).tolist()
                out, run = oracle(
                    dict(aoi=aoi, crs=crs, y=yy.tolist(), x=xx.tolist(), points=points),
                    name,
                )
                expected, window = independent_mask(aoi, crs, yy, xx)
                actual = np.asarray(out["source_global_mask"], dtype="u1")
                differences = np.argwhere(actual != expected).tolist()
                if differences:
                    discrepancies.append(
                        dict(
                            case=name,
                            crs=crs,
                            directions=[reverse_y, reverse_x],
                            kind="unexpected-native-vs-PROJ-GEOS",
                            positions=differences,
                        )
                    )
                    save("discrepancies", discrepancies)
                assert not differences, discrepancies[-1:]
                assert out["report"]["window"] == window
                assert out["report"]["included_native_centers"] == int(expected.sum())
                px, py = forward.transform(*np.array(points).T)
                err = float(
                    np.max(
                        np.abs(
                            np.asarray(out["projected_points"])
                            - np.column_stack([px, py])
                        )
                    )
                )
                gx, gy = gdal_transform("EPSG:4326", crs, *np.array(points).T)
                gdal_err = float(
                    np.max(
                        np.abs(
                            np.asarray(out["projected_points"])
                            - np.column_stack([gx, gy])
                        )
                    )
                )
                assert max(err, gdal_err) < 1e-6, (err, gdal_err)
                transform = Affine(
                    xx[1] - xx[0],
                    0,
                    xx[0] - (xx[1] - xx[0]) / 2,
                    0,
                    yy[1] - yy[0],
                    yy[0] - (yy[1] - yy[0]) / 2,
                )
                projected = densified_projected(aoi, crs)
                burned = rasterize(
                    [(mapping(projected), 1)],
                    out_shape=expected.shape,
                    transform=transform,
                    all_touched=False,
                    dtype="u1",
                )
                gd = np.argwhere(burned != expected).tolist()
                if gd:
                    inverse = pyproj.Transformer.from_crs(
                        crs, "EPSG:4326", always_xy=True
                    )
                    delta_lon, delta_lat = inverse.transform(
                        [xx[c] for r, c in gd], [yy[r] for r, c in gd]
                    )
                    distances = shapely.distance(
                        geometry(aoi).boundary, shapely.points(delta_lon, delta_lat)
                    )
                    assert float(max(distances)) <= TOLERANCE * 1.01, (
                        name,
                        crs,
                        gd,
                        distances,
                    )
                    discrepancies.append(
                        dict(
                            case=name,
                            crs=crs,
                            directions=[reverse_y, reverse_x],
                            kind="GDAL-default-rasterizer-vs-closed-boundary",
                            positions=gd,
                            maximum_true_geographic_boundary_distance_degrees=float(
                                max(distances)
                            ),
                            explanation="All differences are within the declared roundoff band of the true geographic boundary. GDAL's directional burn edges and forward polygon approximation differ from this closed GEOS coverage contract. Full-source oracle agrees exactly with native output.",
                        )
                    )
                if name == "curved-geographic-edge":
                    corners = Polygon(
                        list(zip(*forward.transform(*np.array(points).T)))
                    )
                    flat_x, flat_y = np.meshgrid(xx, yy)
                    chord = shapely.covers(corners, shapely.points(flat_x, flat_y))
                    cd = np.argwhere(chord != expected).tolist()
                    if cd:
                        discrepancies.append(
                            dict(
                                case=name,
                                crs=crs,
                                kind="corners-only-projection-vs-true-geographic-edges",
                                positions=cd,
                                explanation="Transforming only corners loses true geographic center inclusions. Native selection evaluates the original geographic polygon.",
                            )
                        )
                rows.append(
                    dict(
                        case=name,
                        crs=crs,
                        reversed_y=reverse_y,
                        reversed_x=reverse_x,
                        center_count=int(expected.sum()),
                        window=window,
                        compared_centers=expected.size,
                        max_proj_error_metres=err,
                        max_gdal_proj_error_metres=gdal_err,
                        **run,
                    )
                )
    save("discrepancies", discrepancies)
    save(
        "geometry-oracles",
        dict(
            cases=rows,
            all_passed=True,
            compared_centers=sum(r["compared_centers"] for r in rows),
            pyproj=pyproj.__version__,
            proj=pyproj.proj_version_str,
            shapely=shapely.__version__,
            geos=shapely.geos_version_string,
            mask_contract="source-global PROJ inverse + GEOS covers or boundary distance <=1e-10 degrees",
        ),
    )
    print(f"{len(rows)} independent geometry oracles passed", flush=True)


def axes(mirror, name):
    rows, result = [], None
    for reader in ["h5py", "netcdf4"]:
        directory = Path(tempfile.mkdtemp(prefix=f"axes-{name}-{reader}-", dir=WORK))
        write(
            directory / "request.json",
            dict(reader=reader, source=str(mirror / "source.nc")),
        )
        env, trace = job.held.guard_environment(mirror, directory.name)
        p = subprocess.run(
            [
                sys.executable,
                str(ROOT / "bench/swot_aoi_axes_reference.py"),
                str(directory / "request.json"),
            ],
            env=env,
            capture_output=True,
            timeout=60,
        )
        (directory / "stdout.json").write_bytes(p.stdout)
        (directory / "stderr.txt").write_bytes(p.stderr)
        events = trace.read_text().splitlines() if trace.exists() else []
        assert (
            p.returncode == 0
            and events
            and not any("CAPTURE_MISS" in e for e in events)
        ), (p.returncode, p.stderr.decode(), events[-5:])
        current = json.loads(p.stdout)
        versions = current.pop("versions")
        current["crs"] = pyproj.CRS.from_wkt(current["wkt"]).to_string()
        assert result is None or result == current, (
            "h5py/NetCDF-C full source axes or CRS disagree"
        )
        result = current
        rows.append(
            dict(
                reader=reader,
                versions=versions,
                guarded_reads=len(events),
                capture_misses=0,
                trace_sha256=job.digest(trace),
            )
        )
    return result, rows


def control_aoi(entry, source_axes):
    """AOI inside already qualified captures; no manual indices in scientist input."""
    spatial = entry["request"]["spatial"]
    yy, xx = np.asarray(source_axes["y"]), np.asarray(source_axes["x"])
    first_y, first_x = spatial["start"]
    ny, nx = spatial["count"]
    inverse = pyproj.Transformer.from_crs(
        source_axes["crs"], "EPSG:4326", always_xy=True
    )

    def ring(fractions):
        points = [
            [xx[first_x] + u * (nx - 1) * 100, yy[first_y] + v * (ny - 1) * 100]
            for u, v in fractions
        ]
        lon, lat = inverse.transform(*np.array(points).T)
        return np.column_stack([lon, lat]).tolist()

    # Margin preserves the known 128-square source envelope; the hole and
    # concave notch force AOI mask zeros inside that envelope.
    margin = 0.4 / (nx - 1)
    return dict(
        kind="polygon",
        crs="EPSG:4326",
        coordinates=[
            ring(
                [
                    (-margin, -margin),
                    (1 + margin, -margin),
                    (1 + margin, 0.55),
                    (0.72, 0.55),
                    (0.72, 1 + margin),
                    (-margin, 1 + margin),
                    (-margin, -margin),
                ]
            ),
            ring([(0.2, 0.2), (0.4, 0.2), (0.4, 0.4), (0.2, 0.4), (0.2, 0.2)]),
        ],
    )


def verify_aoi(row, manifest, full_axes, sources=None):
    row["science"] = job.verify(row, manifest, sources=sources)
    root = zarr.open_group(row["output"], mode="r")
    details = []
    for entry, coords in zip(manifest["observations"], full_axes):
        g = root[f"observations/{entry['id']}"]
        resolved = entry["geographic_selection"]
        assert resolved["grid"]["coordinate_sha256"] == [
            sha(np.asarray(coords[axis], dtype="<f8").tobytes()) for axis in ["y", "x"]
        ], "complete native axes differ from independent full-source references"
        expected, window = independent_mask(
            resolved["aoi"], coords["crs"], coords["y"], coords["x"]
        )
        assert window == resolved["window"]
        sy, sx = window["start"]
        ny, nx = window["count"]
        expected = expected[sy : sy + ny, sx : sx + nx]
        actual = g["aoi_mask"][:]
        assert actual.dtype == np.dtype("u1") and np.array_equal(expected, actual)
        assert int(actual.sum()) == resolved["included_centers"]
        assert sha(actual.tobytes()) == resolved["mask_sha256"]
        for name, values, first, count in [
            ("y", coords["y"], sy, ny),
            ("x", coords["x"], sx, nx),
        ]:
            assert np.array_equal(g[name][:], values[first : first + count])
        # Explicitly requested demo statistic, never an implicit raw-array mask.
        composite = (
            actual.astype(bool)
            & g["source_valid_wse"][:].astype(bool)
            & g["quality_mask"][:].astype(bool)
        )
        ds = xr.open_zarr(
            row["output"],
            group=f"observations/{entry['id']}",
            consolidated=False,
            mask_and_scale=False,
            decode_times=False,
        )
        assert np.array_equal(ds["aoi_mask"].values, expected)
        ds.close()
        details.append(
            dict(
                id=entry["id"],
                native_crs=coords["crs"],
                window=window,
                included_native_centers=int(actual.sum()),
                outside_centers_in_window=int((actual == 0).sum()),
                accepted_aoi_wse_count=int(composite.sum()),
                quality_accepted_wse_exists=bool(composite.any()),
                mask_sha256=sha(actual.tobytes()),
            )
        )
    row["geographic_science"] = details
    return details


def offline():
    full_axes, references = [], []
    m = job.manifest()
    resolutions = []
    real_grid_oracles = []
    for i, (entry, native, reference) in enumerate(
        zip(
            m["observations"],
            [job.HELD_NATIVE, job.ORIGINAL],
            [job.HELD_REFERENCE, job.ORIGINAL],
        )
    ):
        coords, refs = axes(reference, entry["id"])
        full_axes.append(coords)
        references.append(refs)
        aoi = control_aoi(entry, coords)
        with server(Mirror(native)) as srv:
            draft = copy.deepcopy(entry)
            draft["request"]["source"]["assets"][0]["locator"] = srv.url
            out, measured = oracle(
                dict(mode="inspect", observation=draft, aoi=aoi), f"inspect-{i}"
            )
            assert not srv.errors, srv.errors
            measured["mirror_requests"] = len(srv.events)
            measured["mirror_bytes"] = sum(e["bytes"] for e in srv.events)
            inverse = pyproj.Transformer.from_crs(
                coords["crs"], "EPSG:4326", always_xy=True
            )
            for name, lo, hi in [
                ("partial-native-domain", -200.0, 250.0),
                ("entirely-outside-native-domain", -500.0, -400.0),
            ]:
                native_points = np.array(
                    [[lo, lo], [hi, lo], [hi, hi], [lo, hi], [lo, lo]]
                )
                lon, lat = inverse.transform(
                    native_points[:, 0] + coords["x"][0],
                    native_points[:, 1] + coords["y"][0],
                )
                variant = dict(
                    kind="polygon",
                    crs="EPSG:4326",
                    coordinates=[np.column_stack([lon, lat]).tolist()],
                )
                got, measurement = oracle(
                    dict(mode="inspect", observation=draft, aoi=variant), f"{name}-{i}"
                )
                expected_variant, window_variant = independent_mask(
                    variant, coords["crs"], coords["y"], coords["x"]
                )
                assert got["report"]["window"] == window_variant
                assert got["report"]["included_native_centers"] == int(
                    expected_variant.sum()
                )
                if name.startswith("entirely"):
                    assert got["observation"] is None and not expected_variant.any()
                else:
                    assert (
                        0
                        < got["report"]["coverage"]["fraction_of_projected_aoi_in_grid"]
                        < 1
                    )
                assert not srv.errors, srv.errors
                real_grid_oracles.append(
                    dict(id=entry["id"], case=name, report=got["report"], **measurement)
                )
        original_asset = entry["request"]["source"]["assets"][0]
        entry.update(out["observation"])
        entry["request"]["source"]["assets"][0] = original_asset
        expected, window = independent_mask(
            aoi, coords["crs"], coords["y"], coords["x"]
        )
        assert entry["geographic_selection"]["window"] == window
        assert entry["geographic_selection"]["included_centers"] == int(expected.sum())
        resolutions.append(dict(report=out["report"], **measured))
    write(WORK / "resolved-controls.json", m)
    save("resolved-controls", m)
    write(
        ROOT / "examples/swot-aoi-selection.json",
        dict(
            catalogs=[
                "docs/swot-discovery/known-original.json",
                "docs/swot-discovery/known-held.json",
            ],
            selection=dict(
                observations=[
                    dict(
                        concept_id=concept,
                        observation_id=e["id"],
                        aoi=e["geographic_selection"]["aoi"],
                        quality_policy=e["quality_policy"],
                        expected_etag=e["request"]["source"]["assets"][0]["etag"],
                    )
                    for e, concept in zip(
                        m["observations"],
                        ["G3927559976-POCLOUD", "G3927359490-POCLOUD"],
                    )
                ],
                source_budget=m["observations"][0]["request"]["budget"],
                limits=m["limits"],
            ),
        ),
    )
    fixtures = job.Fixtures()
    runs = []
    try:
        template = fixtures.job()
        for entry, fixture in zip(m["observations"], template["observations"]):
            entry["request"]["source"] = fixture["request"]["source"]
        checkpoint = Path(tempfile.mkdtemp(prefix="checkpoint-", dir=WORK)) / "ranges"
        for name, edge in [("geographic-first", 64), ("geographic-replay-edge17", 17)]:
            m["limits"]["output_chunk_edge"] = edge
            m["observations"][0]["request"]["budget"]["chunk_edge"] = (
                19 if edge == 17 else 32
            )
            fixtures.reset()
            row = job.run(name, m, fixtures, checkpoint=checkpoint)
            verify_aoi(row, m, full_axes)
            runs.append(row)
        assert runs[0]["geographic_science"] == runs[1]["geographic_science"]
        later_fault = copy.deepcopy(m)
        old = later_fault["observations"][1]["geographic_selection"]["mask_sha256"]
        later_fault["observations"][1]["geographic_selection"]["mask_sha256"] = (
            "0" if old[0] != "0" else "1"
        ) + old[1:]
        fixtures.reset()
        later_abort = job.run(
            "later-aoi-mismatch-aborts-whole-dataset", later_fault, fixtures
        )
        assert (
            later_abort["returncode"] != 0
            and len(later_abort["receipt"]["provisional_source_receipts"]) == 1
        )
        altered = copy.deepcopy(m)
        altered["observations"][0]["geographic_selection"]["aoi"]["coordinates"][1][1][
            0
        ] += 0.00001
        fixtures.reset()
        changed = job.run(
            "changed-polygon-checkpoint-refused",
            altered,
            fixtures,
            checkpoint=checkpoint,
        )
        assert changed["returncode"] != 0
        fixtures.reset()
        same_membership = job.run(
            "subpixel-polygon-change-fresh-acquisition", altered, fixtures
        )
        assert same_membership["returncode"] == 0
        same_mask = zarr.open_group(same_membership["output"], mode="r")[
            "observations/held_out_20240217/aoi_mask"
        ][:]
        assert (
            sha(same_mask.tobytes())
            == altered["observations"][0]["geographic_selection"]["mask_sha256"]
        )
        altered["observations"][0]["geographic_selection"]["aoi"]["coordinates"][1][1][
            0
        ] += 0.005
        fixtures.reset()
        changed_without_checkpoint = job.run(
            "changed-polygon-mask-identity-refused", altered, fixtures
        )
        assert changed_without_checkpoint["returncode"] != 0
        empty_source = job.manifest()["observations"][0]
        empty_source["request"]["spatial"]["start"] = [600, 400]
        empty_aoi = control_aoi(empty_source, full_axes[0])
        with server(Mirror(job.HELD_NATIVE)) as srv:
            empty_source["request"]["source"]["assets"][0]["locator"] = srv.url
            empty_out, _ = oracle(
                dict(mode="inspect", observation=empty_source, aoi=empty_aoi),
                "no-accepted-wse-inspection",
            )
            assert not srv.errors, srv.errors
        empty = copy.deepcopy(m)
        empty["observations"] = [empty_out["observation"]]
        empty["observations"][0]["request"]["source"] = template["observations"][0][
            "request"
        ]["source"]
        fixtures.reset()
        zero_quality = job.run(
            "geographic-centers-with-zero-accepted-wse", empty, fixtures
        )
        zero_details = verify_aoi(zero_quality, empty, [full_axes[0]])
        assert (
            zero_details[0]["included_native_centers"] > 0
            and zero_details[0]["accepted_aoi_wse_count"] == 0
        )
        save(
            "frozen-workflow",
            dict(
                resolutions=resolutions,
                independent_axis_references=references,
                real_grid_partial_and_outside_oracles=real_grid_oracles,
                runs=[job.compact(r) for r in runs],
                checkpoint_polygon_change=job.compact(changed),
                subpixel_change_same_discrete_mask=job.compact(same_membership),
                stale_mask_identity=job.compact(changed_without_checkpoint),
                later_aoi_mismatch_atomic_abort=job.compact(later_abort),
                included_centers_zero_accepted_wse=job.compact(zero_quality),
                public_provider_requests=0,
                all_passed=True,
            ),
        )
    finally:
        fixtures.close()
    print(
        "frozen geographic multi-observation science, masks, chunks and checkpoints passed",
        flush=True,
    )


def measured_cli(args, directory, name, timeout=240):
    """Source inspection CPU/RSS are measured separately from source reservations."""
    import resource

    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started, peak = time.perf_counter(), 0
    env = os.environ.copy()
    env["TILE_RIPPER_HDF5_METADATA_POLICY"] = "page16k"
    with (
        (directory / f"{name}.json").open("wb") as out,
        (directory / f"{name}.stderr").open("wb") as err,
    ):
        p = subprocess.Popen(
            [str(job.BINARY), *map(str, args)], stdout=out, stderr=err, env=env
        )
        while p.poll() is None:
            try:
                peak = max(peak, psutil.Process(p.pid).memory_info().rss)
            except psutil.NoSuchProcess:
                pass
            if time.perf_counter() - started > timeout:
                p.kill()
                p.wait()
                raise TimeoutError(name)
            time.sleep(0.005)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    return dict(
        returncode=p.returncode,
        wall_seconds=time.perf_counter() - started,
        cpu_seconds=after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime,
        sampled_process_peak_rss_bytes=peak,
        stdout=str(directory / f"{name}.json"),
        stderr=str(directory / f"{name}.stderr"),
    )


def additional(netrc):
    # Fixed before capture: first previously structurally inspected candidate,
    # small WGS84 box at its reported footprint center, no candidate substitution.
    receipt_path = DOCS / "additional-capture-budget-v3.json"
    candidate = json.loads(
        (ROOT / "docs/swot-discovery/additional-inspections.json").read_text()
    )["inspections"][0]["candidate"]
    aoi = dict(kind="bounds", crs="EPSG:4326", bbox=[-71.85, 41.53, -71.84, 41.54])
    bounds = dict(
        concept_id=candidate["concept_id"],
        source_url=candidate["source_url"],
        aoi=aoi,
        native_capture_bytes=6 * 1024 * 1024,
        native_range_gets=512,
        reference_additional_bytes=512 * 1024,
        reference_additional_gets=192,
        rule="first prior additional 100m PGD0 candidate; fixed footprint-center geographic AOI; no source replacement, no product expansion",
        amendment="Original 4 MiB cap failed and is preserved in additional-initial-cap-failure.json. Frozen native planning measured 4148750 encoded science bytes plus metadata for the same fixed AOI. Resume existing pinned bytes under 6 MiB, without broadening source/product/geography.",
        reference_amendment="NetCDF-C eager metadata open consumed 128 tiny missing ranges (59697 bytes), before numerical verification. Preserve additional-reference-initial-cap-failure.json and allow 64 further exact metadata misses under the unchanged 512 KiB reference byte cap.",
    )
    if receipt_path.exists():
        assert json.loads(receipt_path.read_text()) == bounds
    else:
        write(receipt_path, bounds)
    os.environ["NETRC"] = str(netrc)
    directory = WORK / "additional"
    directory.mkdir(exist_ok=True)
    native_path, reference_path = directory / "native", directory / "reference"

    class Session(job.held.OfficialSession):
        def __init__(self, cap):
            super().__init__()
            self.calls, self.cap = 0, cap

        def get(self, *args, **kwargs):
            self.calls += 1
            if self.calls > self.cap:
                raise RuntimeError(
                    "predeclared additional-source request cap exhausted"
                )
            return super().get(*args, **kwargs)

    if not (directory / "resolved.json").exists():
        retained_gets = 0
        if (native_path / "manifest.json").exists():
            existing = Mirror(native_path)
            retained_gets = existing.info["provider_gets"]
            existing.info["cap"] = bounds["native_capture_bytes"]
            existing.save()
        mirror = Mirror(
            native_path,
            url=candidate["source_url"],
            cap=bounds["native_capture_bytes"],
            session=Session(max(0, bounds["native_range_gets"] - retained_gets)),
        )
        draft = job.manifest()["observations"][1]
        draft["id"] = "additional_20240216"
        draft["request"]["source"]["assets"][0].update(
            id=candidate["concept_id"],
            locator=candidate["source_url"],
            datetime=candidate["acquisition_start"],
            etag=mirror.info["etag"],
        )
        with server(mirror) as srv:
            draft["request"]["source"]["assets"][0]["locator"] = srv.url
            resolved, measured = oracle(
                dict(mode="inspect", observation=draft, aoi=aoi),
                "additional-inspection",
            )
            assert resolved["report"]["native_centers_intersect"], (
                "fixed additional AOI has zero centers; no replacement"
            )
            m = job.manifest()
            m["observations"] = [resolved["observation"]]
            row = job.run("additional-bounded-mirror-capture", m)
            assert row["returncode"] == 0, row["error"]
            assert not srv.errors, srv.errors
        mirror.seal()
        mirror.verify()
        resolved["observation"]["request"]["source"]["assets"][0]["locator"] = (
            candidate["source_url"]
        )
        write(
            directory / "resolved.json",
            dict(resolved=resolved, inspection=measured, native_capture=mirror.info),
        )
    preserved = json.loads((directory / "resolved.json").read_text())
    if not reference_path.exists():
        shutil.copytree(native_path, reference_path)
    reference = Mirror(reference_path)
    native_info = Mirror(native_path).info
    reference.info["cap"] = (
        native_info["provider_bytes"] + bounds["reference_additional_bytes"]
    )
    reference.save()
    reference = Mirror(
        reference_path,
        url=candidate["source_url"],
        session=Session(bounds["reference_additional_gets"]),
    )
    receipt_file = DOCS / "additional-reference-completion.json"
    attempts = (
        json.loads(receipt_file.read_text())
        if receipt_file.exists()
        else dict(attempts=[], added=[])
    )
    while True:
        probe = job.held.probe(
            reference_path,
            "netcdf4",
            "enumerate",
            f"aoi-additional-{len(attempts['attempts'])}",
        )
        attempts["attempts"].append(job.held.compact_probe(probe))
        write(receipt_file, attempts)
        if probe["returncode"] == 0:
            break
        assert probe["misses"], (
            "reference failed without a capture miss; preserve failure"
        )
        for start, end in probe["misses"]:
            for a, b in reference.missing(start, end):
                if (
                    reference.info["provider_gets"] - native_info["provider_gets"]
                    >= bounds["reference_additional_gets"]
                ):
                    raise RuntimeError("additional reference request cap exhausted")
                data = reference.read(a, b - a)
                attempts["added"].append(
                    dict(start=a, end=b, bytes=len(data), sha256=sha(data))
                )
        reference.seal()
        reference.capture = True
        write(receipt_file, attempts)
    reference.seal()
    coords, refs = axes(reference_path, "additional")
    m = job.manifest()
    m["observations"] = [preserved["resolved"]["observation"]]
    with server(Mirror(native_path)) as srv:
        m["observations"][0]["request"]["source"]["assets"][0]["locator"] = srv.url
        row = job.run("additional-frozen-scientific-qualification", m)
        verify_aoi(row, m, [coords], sources=[reference_path])
        assert not srv.errors, srv.errors
    write(directory / "axes.json", coords)
    write(DOCS / "additional-resolved.json", preserved["resolved"]["observation"])
    save(
        "additional-qualification",
        dict(
            candidate=candidate,
            aoi=aoi,
            inspection=preserved["inspection"],
            resolution=preserved["resolved"]["report"],
            native_capture=preserved["native_capture"],
            reference_added_bytes=reference.info["provider_bytes"]
            - native_info["provider_bytes"],
            reference_added_gets=reference.info["provider_gets"]
            - native_info["provider_gets"],
            independent_axis_references=refs,
            run=job.compact(row),
            all_passed=True,
        ),
    )
    print(
        "fixed additional granule passed independent bounded AOI scientific qualification",
        flush=True,
    )


def empty_live(netrc):
    workflow = json.loads((DOCS / "live-workflow.json").read_text())
    selected = json.loads((Path(workflow["directory"]) / "selection.json").read_text())
    selected["selection"]["observations"] = selected["selection"]["observations"][:1]
    coords, _ = axes(job.HELD_REFERENCE, "live-zero-centers")
    inverse = pyproj.Transformer.from_crs(coords["crs"], "EPSG:4326", always_xy=True)
    lon, lat = inverse.transform(coords["x"][700] + 50.0, coords["y"][70] + 50.0)
    aoi = dict(
        kind="bounds",
        crs="EPSG:4326",
        bbox=[lon - 0.0000005, lat - 0.0000005, lon + 0.0000005, lat + 0.0000005],
    )
    selected["selection"]["observations"][0]["aoi"] = aoi
    expected, window = independent_mask(aoi, coords["crs"], coords["y"], coords["x"])
    assert not expected.any() and window is None
    directory = Path(tempfile.mkdtemp(prefix="live-zero-centers-", dir=WORK))
    write(directory / "selection.json", selected)
    row = measured_cli(
        [
            "discover-job",
            directory / "selection.json",
            "--auth",
            "earthdata",
            "--netrc",
            netrc,
            "--output",
            directory / "no-job.json",
        ],
        directory,
        "review",
    )
    review = json.loads((directory / "review.json").read_text())
    assert row["returncode"] != 0 and not (directory / "no-job.json").exists()
    assert (
        review["status"] == "not_ready_zero_native_centers"
        and len(review["observations"]) == 1
    )
    assert review["observations"][0]["aoi_resolution"]["included_native_centers"] == 0
    save(
        "live-zero-centers",
        dict(
            **row,
            aoi=aoi,
            review=review,
            independent_source_global_centers=0,
            manifest_written=False,
            all_passed=True,
        ),
    )
    print(
        "live zero-center candidate explicitly reported; no job written or source substituted",
        flush=True,
    )


def live(netrc):
    directory = Path(tempfile.mkdtemp(prefix="live-", dir=WORK))
    catalogs, catalog_results = discovery.catalog(directory)
    selected = json.loads((ROOT / "examples/swot-aoi-selection.json").read_text())
    selected["catalogs"] = list(map(str, catalogs))
    axes_all = [
        axes(job.HELD_REFERENCE, "live-held")[0],
        axes(job.ORIGINAL, "live-original")[0],
    ]
    sources = [job.HELD_REFERENCE, job.ORIGINAL]
    third = DOCS / "additional-resolved.json"
    if third.exists():
        entry = json.loads(third.read_text())
        cpath = directory / "additional-candidates.json"
        catalog_call = measured_cli(
            [
                "discover",
                "--collection",
                "SWOT_L2_HR_Raster_D",
                "--bbox",
                "-71.85,41.53,-71.84,41.54",
                "--start",
                "2024-02-16T02:45:00Z",
                "--end",
                "2024-02-16T03:00:00Z",
                "--limit",
                "20",
                "--output",
                cpath,
            ],
            directory,
            "additional-catalog",
        )
        assert catalog_call["returncode"] == 0
        catalog_results.append(
            dict(**catalog_call, report=json.loads(cpath.read_text()))
        )
        selected["catalogs"].append(str(cpath))
        selected["selection"]["observations"].append(
            dict(
                concept_id=entry["request"]["source"]["assets"][0]["id"],
                observation_id=entry["id"],
                aoi=entry["geographic_selection"]["aoi"],
                quality_policy=entry["quality_policy"],
                expected_etag=entry["request"]["source"]["assets"][0]["etag"],
            )
        )
        axes_all.append(json.loads((WORK / "additional/axes.json").read_text()))
        sources.append(WORK / "additional/reference")
    write(directory / "selection.json", selected)
    generated = measured_cli(
        [
            "discover-job",
            directory / "selection.json",
            "--auth",
            "earthdata",
            "--netrc",
            netrc,
            "--output",
            directory / "job.json",
        ],
        directory,
        "review",
    )
    assert generated["returncode"] == 0, (directory / "review.stderr").read_text()
    manifest = json.loads((directory / "job.json").read_text())
    review = json.loads((directory / "review.json").read_text())
    auth = ["--auth", "earthdata", "--netrc", str(netrc)]
    plan = job.run("aoi-live-plan", manifest, command="plan", extra=auth)
    assert plan["returncode"] == 0, plan["error"]
    checkpoint = directory / "checkpoint"
    runs = []
    for name in [
        "aoi-live-three-observation",
        "aoi-live-complete-checkpoint-reconstruction",
    ]:
        row = job.run(name, manifest, checkpoint=checkpoint, extra=auth)
        verify_aoi(row, manifest, axes_all, sources=sources)
        runs.append(row)
    save("generated-job", manifest)
    save("live-review", job.compact(review))
    save(
        "live-workflow",
        dict(
            directory=str(directory),
            discovery=catalog_results,
            generation=generated,
            review=job.compact(review),
            plan=job.compact(plan),
            first=job.compact(runs[0]),
            reconstruction=job.compact(runs[1]),
            all_passed=True,
        ),
    )
    print(
        "live geographic discovery, inspected windows, checkpointed Zarr and independent three-source science passed",
        flush=True,
    )


def final_replay(netrc):
    workflow = json.loads((DOCS / "live-workflow.json").read_text())
    manifest = json.loads((DOCS / "generated-job.json").read_text())
    sources = [job.HELD_REFERENCE, job.ORIGINAL, WORK / "additional/reference"][
        : len(manifest["observations"])
    ]
    coords = [axes(source, f"final-{i}")[0] for i, source in enumerate(sources)]
    row = job.run(
        "aoi-final-binary-checkpoint-reconstruction",
        manifest,
        checkpoint=Path(workflow["directory"]) / "checkpoint",
        extra=["--auth", "earthdata", "--netrc", str(netrc)],
    )
    verify_aoi(row, manifest, coords, sources=sources)
    save("final-binary-reconstruction", job.compact(row))
    print(
        "final binary reconstructed and independently verified all native observations",
        flush=True,
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "mode",
        choices=[
            "geometry",
            "offline",
            "additional",
            "live",
            "empty-live",
            "final-replay",
        ],
    )
    parser.add_argument("--netrc", type=Path)
    args = parser.parse_args()
    if (
        args.mode in ["additional", "live", "empty-live", "final-replay"]
        and args.netrc is None
    ):
        parser.error("explicit --netrc required for bounded provider operations")
    {
        "geometry": geometry_oracles,
        "offline": offline,
        "additional": lambda: additional(args.netrc),
        "live": lambda: live(args.netrc),
        "empty-live": lambda: empty_live(args.netrc),
        "final-replay": lambda: final_replay(args.netrc),
    }[args.mode]()
