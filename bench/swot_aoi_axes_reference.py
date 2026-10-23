"""Isolated full-axis reference: one mature reader, no native acquisition code."""

import hashlib
import json
from pathlib import Path
import sys

import numpy as np


def run(path):
    request = json.loads(Path(path).read_text())
    if request["reader"] == "h5py":
        import h5py

        assert "netCDF4" not in sys.modules
        source = h5py.File(request["source"], "r")
        x, y = source["x"][:], source["y"][:]
        wkt = source["crs"].attrs["crs_wkt"]
        versions = dict(reader=h5py.__version__, hdf5=h5py.version.hdf5_version)
    elif request["reader"] == "netcdf4":
        import netCDF4

        assert "h5py" not in sys.modules
        source = netCDF4.Dataset(request["source"])
        source.set_auto_maskandscale(False)
        x, y = source["x"][:], source["y"][:]
        wkt = source["crs"].getncattr("crs_wkt")
        versions = dict(
            reader=netCDF4.__version__,
            hdf5=netCDF4.__hdf5libversion__,
            netcdf_c=netCDF4.getlibversion(),
        )
    else:
        raise ValueError("explicit independent reader required")
    source.close()
    if isinstance(wkt, bytes):
        wkt = wkt.decode()
    print(
        json.dumps(
            dict(
                x=x.tolist(),
                y=y.tolist(),
                wkt=wkt,
                versions=versions,
                coordinate_sha256=[
                    hashlib.sha256(np.asarray(axis, dtype="<f8").tobytes()).hexdigest()
                    for axis in [y, x]
                ],
            )
        )
    )


if __name__ == "__main__":
    run(sys.argv[1])
