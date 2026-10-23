"""One mature reader and one operation per process; flushed phases survive aborts.

Usage: python swot_reference_probe.py READER FILE OPERATION
Operations: open, enumerate, attributes:NAME, read:NAME, close, environment.
Stored values are read with NetCDF automatic masking/scaling disabled.
"""

import hashlib
import json
import sys


def phase(name, **extra):
    print(json.dumps({"phase": name, **extra}), flush=True)


def main():
    reader, path, operation = sys.argv[1:]
    if reader == "netcdf4":
        import netCDF4

        phase(
            "environment",
            reader=netCDF4.__version__,
            netcdf_c=netCDF4.getlibversion(),
            hdf5=netCDF4.__hdf5libversion__,
            extension=netCDF4._netCDF4.__file__,
        )

        def open_file():
            return netCDF4.Dataset(path, "r")
    else:
        import h5py

        phase(
            "environment",
            reader=h5py.__version__,
            hdf5=h5py.version.hdf5_version,
            extension=h5py.h5f.__file__,
        )

        def open_file():
            return h5py.File(path, "r")

    if operation == "environment":
        return
    phase("open.begin")
    f = open_file()
    phase("open.end")
    if reader == "netcdf4":
        f.set_auto_maskandscale(False)
    if operation == "open":
        # Explicit exit omits cleanup, distinguishing opening from destruction.
        import os

        os._exit(0)
    if operation == "enumerate":
        phase("enumerate.begin")
        if reader == "netcdf4":
            phase(
                "enumerate.end",
                groups=list(f.groups),
                dimensions=list(f.dimensions),
                variables=list(f.variables),
                attributes=f.ncattrs(),
            )
        else:
            phase("enumerate.end", variables=list(f), attributes=list(f.attrs))
    if ":" in operation:
        action, name = operation.split(":", 1)
        phase(operation + ".begin")
        v = f[name]
        if action == "attributes":
            attrs = (
                {k: v.getncattr(k) for k in v.ncattrs()}
                if reader == "netcdf4"
                else dict(v.attrs)
            )
            phase(operation + ".end", attributes=list(attrs))
        else:
            # Original all-invalid window and matching coordinate vectors.
            value = (
                v[600:728, 400:528]
                if v.ndim == 2
                else v[600:728]
                if name == "y"
                else v[400:528]
            )
            phase(
                operation + ".end",
                shape=value.shape,
                sha256=hashlib.sha256(value.tobytes()).hexdigest(),
            )
    phase("close.begin")
    f.close()
    phase("close.end")


if __name__ == "__main__":
    main()
