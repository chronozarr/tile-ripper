"""Verify finished native chunks against h5py, including a remote range source.

python bench/hdf5_verify.py SOURCE.nc_OR_URL stdout.json CHUNK_DIRECTORY
"""

import json
import sys
from pathlib import Path

import h5py
import numpy as np
from hdf5_reference import Remote


def main():
    source, receipt, directory = sys.argv[1:]
    result = json.loads(Path(receipt).read_text())
    assert result["status"] == "complete"  # oracle prints only after finish()
    selection = result["inspection"]["logical_request"]["spatial"]
    run = result["runs"][0]
    directory = Path(directory)
    remote = Remote(source) if source.startswith(("http://", "https://")) else None
    values = 0
    valid_count = 0
    total = 0.0
    maximum = 0.0
    with h5py.File(remote if remote is not None else source, "r") as f:
        for i, c in enumerate(run["chunks"]):
            ds = f[c["variable"]["path"]]
            w = c["window"]
            dtype = "<u2" if ds.dtype.kind == "u" else "<f4"
            step = selection["step"]
            slices = (
                slice(w["y"], w["y"] + w["height"] * step[0], step[0]),
                slice(w["x"], w["x"] + w["width"] * step[1], step[1]),
            )
            raw = np.fromfile(directory / f"{i}.bin", dtype=dtype).reshape(
                w["height"], w["width"]
            )
            assert raw.tobytes() == ds[slices].astype(dtype).tobytes()
            for axis, s, actual in zip(
                selection["axes"], slices, c["coordinates"][::-1]
            ):
                assert np.array_equal(f[axis][s].astype("f8"), actual)
            scale = float(np.asarray(ds.attrs.get("scale_factor", 1)).item())
            offset = float(np.asarray(ds.attrs.get("add_offset", 0)).item())
            assert c["scale"] is None or c["scale"] == scale
            assert c["offset"] is None or c["offset"] == offset
            valid = ~np.isnan(raw)
            if "_FillValue" in ds.attrs:
                valid &= raw != np.asarray(ds.attrs["_FillValue"]).item()
            physical = raw[valid].astype("f8") * scale + offset
            values += raw.size
            valid_count += physical.size
            total += physical.sum()
            maximum = max(maximum, float(physical.max()) if physical.size else 0.0)
    if remote is not None:
        remote.close()
    print(
        json.dumps(
            {
                "verified_values": values,
                "valid_values": valid_count,
                "mean_unpacked_value": total / valid_count if valid_count else None,
                "max_unpacked_value": maximum,
                "whole_object_download": False
                if remote is not None
                else "reference is local",
                "reference_range_bytes": sum(b - a for a, b in remote.events)
                if remote is not None
                else None,
            }
        )
    )


if __name__ == "__main__":
    main()
