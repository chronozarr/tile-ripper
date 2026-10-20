"""Native-only STAC integration against independent GDAL masks and late faults."""

import copy
import hashlib
import json
import os
from pathlib import Path
import threading
import zlib

import numpy as np
import rasterio
from rasterio.features import geometry_mask
from rasterio.warp import transform_geom
from rasterio.windows import Window

import consumer_validation as consumer
import stac_workflow as fixture

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/native-sentinel/workflow"
BIN = Path(os.environ.get("TILE_RIPPER_STATS_BINARY", ROOT / "target/native-sentinel/release/tile-ripper-native-stats"))


def unified(request):
    return dict(
        source=dict(kind="stac", endpoint=request["endpoint"], collection=request["collection"], limits=dict(max_items=8, page_size=1, max_assets=8, max_output_pixels=1000000)),
        spatial=request["aoi"],
        time=dict(kind="interval", start=request["start"], end=request["end"]),
        variables=[dict(kind="named", name=b) for b in request["bands"]],
        grid=dict(kind="native"), sink=dict(kind="callback"),
        budget=dict(buffer_bytes=16 * 2**20, max_nodes=100000, chunk_edge=64),
    )


def verify(result, request, server):
    assert result["code"] == 0, result
    rows = result["rows"]
    assert rows[-1]["type"] == "complete"
    provenance = rows[0]["provenance"]
    expected = [(f"scene-{i}", {"B04": "red", "B08": "nir"}[b]) for i in range(2) for b in request["bands"]]
    assert [(s["item_id"], s["asset_key"]) for s in provenance["sources"]] == expected
    assert all(s["object_identity"]["etag"] == '"fixed-fixture"' for s in provenance["sources"])
    assert "NEVER_SAVE_THIS" not in json.dumps(provenance)
    assert provenance["query"]["coordinate_order"] == "longitude, latitude"
    refs = {s["asset_id"]: server.root / f"scene{s['item_id'][-1]}-{s['asset_key']}.tif" for s in provenance["sources"]}
    source_order = {s["asset_id"]: i for i, s in enumerate(provenance["sources"])}
    previous = (-1, -1, -1)
    count = valid = total = 0
    with rasterio.Env(GDAL_CACHEMAX=32 * 1024**2):
        for row in rows[1:-1]:
            assert row["type"] == "provisional_chunk"
            w = row["window"]
            order = (source_order[row["asset_id"]], w["y"], w["x"])
            assert order > previous
            previous = order
            with rasterio.open(refs[row["asset_id"]]) as ds:
                window = Window(w["x"], w["y"], w["width"], w["height"])
                transform = ds.window_transform(window)
                a = ds.read(1, window=window)
                geometry = transform_geom("EPSG:4326", ds.crs, fixture.dense_wgs(request))
                full_mask = geometry_mask([geometry], out_shape=(ds.height, ds.width), transform=ds.transform, invert=True)
                inside = full_mask[w["y"]:w["y"]+w["height"], w["x"]:w["x"]+w["width"]]
                assert inside.any()
                a[~inside] = ds.nodata
                assert row["adler32_le"] == zlib.adler32(a.astype("<u2").tobytes())
                assert row["transform"] == list(transform.to_gdal())
                assert row["crs"] == ds.crs.to_string()
                assert (row["nodata"], row["scale"], row["offset"]) == (ds.nodata, ds.scales[0], ds.offsets[0])
                assert row["datetime"] == "2025-07-28T15:00:00+00:00"
                n = int(np.count_nonzero(a != ds.nodata))
                assert row["valid_pixels"] == n
                assert row["native_sum"] == int(a[a != ds.nodata].sum())
                valid += n
                total += int(a[a != ds.nodata].sum())
                count += a.size
    assert rows[-1]["valid_pixels"] == valid and rows[-1]["native_sum"] == total
    assert result["peak_rss_bytes"] < 192 * 2**20
    return dict(samples_checked=count, valid_pixels=valid, chunks=rows[-1]["chunks"])


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    consumer.BIN = BIN
    server = fixture.Server(("127.0.0.1", 0), fixture.Handler)
    server.root = WORK / "fixtures"
    server.files, server.features = fixture.create(server.root, server.server_port)
    server.queries, server.seen_conditions = [], []
    server.reset()
    # Even an unhelpful server can return off-date and duplicate candidates.
    off_date = copy.deepcopy(server.features[0])
    off_date["id"] = "wrong-date"
    off_date["properties"]["datetime"] = "2024-07-28T15:00:00Z"
    server.features.extend([off_date, copy.deepcopy(server.features[0])])
    threading.Thread(target=server.serve_forever, daemon=True).start()
    receipt = []
    try:
        for large, reverse in [(False, False), (True, False), (True, True)]:
            request = fixture.request(server.server_port, "unused", large=large)
            if reverse:
                request["bands"].reverse()
            name = f"success-{large}-{reverse}"
            result = consumer.invoke(unified(request), WORK / f"{name}.json")
            check = verify(result, request, server)
            receipt.append(dict(name=name, passed=True, **check, peak_rss_bytes=result["peak_rss_bytes"], seconds=result["seconds"]))
        base = unified(fixture.request(server.server_port, "unused", large=True))
        for name, args, mutation in [
            ("cancel-after-delivery", ["--cancel-after", "1"], None),
            ("late-source-mutation", ["--pause-after", "1"], lambda _: setattr(server, "current_etag", '"changed-fixture"')),
            ("consumer-failure", ["--max-chunk-bytes", "1"], None),
        ]:
            server.current_etag = '"fixed-fixture"'
            result = consumer.invoke(base, WORK / f"{name}.json", args=args, mutate=mutation)
            assert result["code"] != 0 and not any(r["type"] == "complete" for r in result["rows"]), result
            if mutation:
                assert any(r["type"] == "provisional_chunk" for r in result["rows"])
                assert "version" in result["rows"][-1]["error"].lower() or "conflict" in result["rows"][-1]["error"].lower()
            receipt.append(dict(name=name, passed=True, exit_code=result["code"], terminal=result["rows"][-1]))
        server.current_etag = '"fixed-fixture"'
        empty = copy.deepcopy(base)
        empty["time"] = dict(kind="interval", start="2020-01-01T00:00:00Z", end="2020-01-02T00:00:00Z")
        result = consumer.invoke(empty, WORK / "empty.json")
        assert result["code"] == 0 and result["rows"][-1]["chunks"] == 0
        receipt.append(dict(name="empty-date-selection", passed=True))
        unsupported = copy.deepcopy(base)
        unsupported["variables"] = [dict(kind="named", name="B10")]
        result = consumer.invoke(unsupported, WORK / "unsupported.json")
        assert result["code"] != 0 and "unsupported Sentinel-2" in result["rows"][-1]["error"]
        receipt.append(dict(name="unsupported-band", passed=True))
        assert server.counts["whole_gets"] == 0
        assert server.counts["conditional_gets"] > 0
        assert all(c.startswith('"') for c in server.seen_conditions)
        record = dict(passed=True, binary_sha256=hashlib.sha256(BIN.read_bytes()).hexdigest(), checks=receipt, http=server.counts,
                      notes=["Generated loopback STAC/COGs; independent GDAL masks and per-chunk checksums.", "Exact sample-array equality is separately gated by native_sentinel_checks.py."])
        (ROOT / "docs/native-sentinel/workflow-checks.json").write_text(json.dumps(record, indent=2) + "\n")
        print(json.dumps(record, indent=2))
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
