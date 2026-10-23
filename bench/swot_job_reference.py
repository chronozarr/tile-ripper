"""Independent scientific worker: h5py OR NetCDF-C; never native-reader tooling.

The caller installs capture_guard in this process. Missing sparse source bytes
are fatal, including metadata reads. The established quality reference performs
the five raw reads and calculations; this wrapper adds exact axes/time/attributes.
"""

import contextlib
import base64
import io
import json
from pathlib import Path
import sys

import numpy as np
import swot_quality_reference as science


def normalize(value):
    if isinstance(value, bytes):
        return value.decode().rstrip("\0")
    if isinstance(value, np.ndarray):
        return (
            normalize(value.item())
            if value.ndim == 0
            else [normalize(v) for v in value]
        )
    if isinstance(value, np.generic):
        return normalize(value.item())
    if isinstance(value, float) and not np.isfinite(value):
        return {"nonfinite": str(value)}
    if isinstance(value, (str, int, float, bool)) or value is None:
        return value
    return {"uninterpreted_storage_type": type(value).__name__}


def run(request):
    transcript = io.StringIO()
    with contextlib.redirect_stdout(transcript):
        science.run(request)
    result = json.loads(transcript.getvalue())
    out = Path(request["dump"])
    slices = science.selection(request["spatial"])
    if request["reader"] == "h5py":
        import h5py

        source = h5py.File(request["local"], "r", rdcc_nbytes=2 * 1024 * 1024)

        def attributes(obj):
            return dict(obj.attrs)
    else:
        import netCDF4

        source = netCDF4.Dataset(request["local"])
        source.set_auto_maskandscale(False)

        def attributes(obj):
            return {name: obj.getncattr(name) for name in obj.ncattrs()}

    metadata = {"root": {k: normalize(v) for k, v in attributes(source).items()}}
    has_time = "t" in (source if request["reader"] == "h5py" else source.variables)
    names = science.PATHS + ["y", "x", "crs"] + (["t"] if has_time else [])
    for name in names:
        obj = source[name]
        metadata[name] = {k: normalize(v) for k, v in attributes(obj).items()}
    if request["reader"] == "h5py":
        storage = {}
        for name in ["root"] + names:
            obj = source if name == "root" else source[name]
            storage[name] = {}
            for key in obj.attrs:
                attr = obj.attrs.get_id(key)
                if attr.dtype.kind not in "iufSU":
                    continue
                raw = np.empty(attr.shape, dtype=attr.dtype)
                attr.read(raw)
                storage[name][key] = {
                    "shape": list(attr.shape),
                    "bytes": base64.b64encode(raw.tobytes()).decode(),
                }
        (out / "attribute-storage.json").write_text(json.dumps(storage))
    for name, selected in zip(["y", "x"], slices):
        (out / f"{name}.bin").write_bytes(
            np.asarray(source[name][selected], dtype="<f8").tobytes()
        )
    if has_time:
        (out / "t.bin").write_bytes(np.asarray(source["t"][...], dtype="<f8").tobytes())
    source.close()
    (out / "attributes.json").write_text(json.dumps(metadata, allow_nan=False))
    result["attributes"] = {k: len(v) for k, v in metadata.items()}
    print(json.dumps(result, allow_nan=False))


if __name__ == "__main__":
    run(json.loads(Path(sys.argv[1]).read_text()))
