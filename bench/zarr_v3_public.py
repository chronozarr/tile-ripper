"""Bounded actual public GFS science oracle; no public-provider speed claim."""

import os
import hashlib
import json
import numpy as np
import zarr
from zarr.storage import FsspecStore
import requests
import zarr_v3_input as h
from zarr_v3_compare import measure

URL = "https://s3.us-west-2.amazonaws.com/us-west-2.opendata.source.coop/dynamical/noaa-gfs-analysis/v0.1.0.zarr/"


def main():
    work = h.WORK / "public"
    work.mkdir(parents=True, exist_ok=True)
    response = requests.get(URL + "zarr.json", timeout=45)
    response.raise_for_status()
    root = response.json()
    metadata = root["consolidated_metadata"]["metadata"]
    crs = metadata["spatial_ref"]["attributes"]["crs_wkt"]
    # Open independent reader against the actual public URL, using current Zarr.
    store = FsspecStore.from_url(
        URL,
        read_only=True,
        storage_options={
            "client_kwargs": {"timeout": __import__("aiohttp").ClientTimeout(total=90)}
        },
    )
    g = zarr.open_group(store=store, mode="r", use_consolidated=True)
    times = g["time"][:]
    i = int(np.searchsorted(times, np.datetime64("2023-01-15", "s").astype("int64")))
    ti = [i, i + 6]
    r = {
        "source": {
            "kind": "assets",
            "assets": [
                {"id": "gfs", "locator": URL, "format": "zarr_v3", "datetime": None}
            ],
        },
        "spatial": {"kind": "bounds", "crs": crs, "bbox": [-101, 34, -100.75, 34.25]},
        "time": {
            "kind": "instants",
            "values": [str(np.datetime64(int(times[j]), "s")) + "Z" for j in ti],
        },
        "variables": [{"kind": "dataset", "path": "temperature_2m"}],
        "quality": [],
        "grid": {"kind": "native"},
        "operations": [],
        "sink": {"kind": "callback"},
        "budget": {
            "buffer_bytes": 256 * 1024 * 1024,
            "max_nodes": 100000,
            "chunk_edge": 32,
        },
    }
    (work / "root.json").write_bytes(response.content)
    request_file = work / "request.json"
    request_file.write_text(json.dumps(r))
    result = measure(
        [h.BINARY, request_file, "dump", work / "chunks"], os.environ.copy()
    )
    assert (
        result["inspection"]["sources"][0]["object_identities"]["zarr.json"]["etag"]
        == response.headers["ETag"]
    )
    seen = set()
    samples = 0
    for n, c in enumerate(result["runs"][0]["chunks"]):
        w = c["window"]
        t = c["indices"][0]
        ref = np.ascontiguousarray(
            g["temperature_2m"][
                t, w["y"] : w["y"] + w["height"], w["x"] : w["x"] + w["width"]
            ]
        )
        assert (work / "chunks" / f"{n}.bin").read_bytes() == ref.astype(
            "<f4"
        ).tobytes()
        assert (work / "chunks" / f"{n}.mask").read_bytes() == (~np.isnan(ref)).astype(
            "u1"
        ).tobytes()
        np.testing.assert_array_equal(
            c["coordinates"][0], g["longitude"][w["x"] : w["x"] + w["width"]]
        )
        np.testing.assert_array_equal(
            c["coordinates"][1], g["latitude"][w["y"] : w["y"] + w["height"]]
        )
        assert c["crs"] == crs
        samples += ref.size
        seen.add(t)
    assert seen == set(ti)
    receipt = {
        "url": URL,
        "catalog": "https://dynamical.org/catalog/models/noaa-gfs/",
        "zarr_python": zarr.__version__,
        "root_etag": response.headers.get("ETag"),
        "root_sha256": hashlib.sha256(response.content).hexdigest(),
        "selected_time_indices": ti,
        "times": r["time"],
        "samples": samples,
        "raw_bits_coordinates_masks_exact": True,
        "result": result,
        "speed_claim": False,
    }
    (work / "result.json").write_text(json.dumps(receipt, indent=2))
    (h.DOCS / "public.json").write_text(json.dumps(receipt, indent=2))
    print(json.dumps({k: v for k, v in receipt.items() if k != "result"}, indent=2))


if __name__ == "__main__":
    main()
