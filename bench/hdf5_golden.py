"""Recreate the small independent HDF5 files used by Rust tests."""

import sys
from pathlib import Path

import h5py
import numpy as np


def main():
    output = Path(sys.argv[1])
    output.mkdir(parents=True, exist_ok=True)
    for name, libver in [("v1-be", "earliest"), ("unsupported-fixed", "latest")]:
        with h5py.File(output / (name + ".h5"), "w", libver=libver) as f:
            g = f.create_group("science")
            g.create_dataset("y", data=np.arange(8, dtype=">f8") * 2 + 1)
            g.create_dataset("x", data=np.arange(11, dtype=">i2") * 3 + 5)
            ds = g.create_dataset(
                "signal",
                shape=(8, 11),
                dtype=">u2",
                chunks=(3, 4),
                fillvalue=65535,
                compression="gzip",
                shuffle=True,
                fletcher32=True,
            )
            ds.attrs["_FillValue"] = np.uint16(65535)
            ds.attrs["scale_factor"] = np.float64(0.01)
            ds.attrs["add_offset"] = np.float64(-2)
            ds.attrs["units"] = np.bytes_("m")
            ds[:] = np.arange(88, dtype=">u2").reshape(8, 11)
            ds[3:6, 4:8] = 65535
    for name, cf in [("storage-fill-valid", None), ("distinct-cf-fill", 65535)]:
        with h5py.File(output / (name + ".h5"), "w", libver="earliest") as f:
            g = f.create_group("science")
            g.create_dataset("y", data=np.arange(8, dtype="f8"))
            g.create_dataset("x", data=np.arange(11, dtype="f8"))
            ds = g.create_dataset(
                "signal", shape=(8, 11), dtype="u2", chunks=(3, 4), fillvalue=7
            )
            ds[0, 0] = 3
            if cf is not None:
                ds.attrs["_FillValue"] = np.uint16(cf)


if __name__ == "__main__":
    main()
