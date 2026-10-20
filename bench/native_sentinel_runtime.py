"""Run actual polygon/date/band discovery+stream in a GDAL-free Linux runtime.

Requires live.json from the independent GDAL oracle; no geospatial Python imports.
"""

import ctypes.util
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import time

ROOT = Path("/src")
WORK = ROOT / "prototype-output/native-sentinel"


def main():
    assert all(ctypes.util.find_library(n) is None for n in ["gdal", "proj", "tiff"])
    tree = (WORK / "linux-tree.txt").read_text()
    assert not any(n in tree for n in ["gdal v", "gdal-sys v", "proj-sys v", "libtiff"])
    env = {**os.environ, "TILE_RIPPER_WORKERS": "2"}
    reference = json.loads((ROOT / "docs/native-sentinel/live.json").read_text())
    request = ROOT / "docs/native-sentinel/request.json"
    cli = WORK / "linux-tile-ripper"
    consumer = WORK / "linux-tile-ripper-native-stats"
    t = time.monotonic()
    plan = json.loads(subprocess.check_output([str(cli), "plan", str(request), "--json"], env=env, text=True))
    assert plan["query"]["coordinate_order"] == "longitude, latitude"
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        p = subprocess.Popen([str(consumer), str(request)], env=env, stdout=stdout, stderr=stderr)
        _, status, usage = os.wait4(p.pid, 0)
        p.returncode = os.waitstatus_to_exitcode(status)
        stderr.seek(0)
        assert p.returncode == 0, stderr.read().decode()
        stdout.seek(0)
        rows = [json.loads(s) for s in stdout.read().decode().splitlines()]
    assert rows[-1]["type"] == "complete"
    source_keys = ["item_id", "asset_key", "requested_band", "datetime", "href", "object_identity"]
    sources = [{k: s[k] for k in source_keys} for s in rows[0]["provenance"]["sources"]]
    assert sources == reference["selected_sources"]
    fields = ["asset_id", "datetime", "variable", "window", "transform", "crs", "dtype", "nodata", "scale", "offset", "valid_pixels", "native_sum", "adler32_le"]
    chunks = [{k: r[k] for k in fields} for r in rows[1:-1]]
    assert chunks == reference["independent_chunk_references"]
    assert usage.ru_maxrss * 1024 < 192 * 2**20
    libraries = subprocess.check_output(["ldd", str(consumer)], text=True)
    assert not any(n in libraries.lower() for n in ["gdal", "libproj", "libtiff"])
    record = dict(passed=True, platform=platform.platform(), architecture=platform.machine(),
        gdal_proj_libtiff_absent=True, clean_source_install=True,
        real_stac_discovery_and_masked_native_consumption=True,
        independent_gdal_expected_chunks_verified=len(chunks),
        source_versions_verified=True, peak_rss_bytes=usage.ru_maxrss * 1024,
        plan_and_acquisition_seconds=time.monotonic()-t, dynamic_libraries=libraries,
        binaries={p.name: dict(bytes=p.stat().st_size, sha256=hashlib.sha256(p.read_bytes()).hexdigest()) for p in [cli, consumer]})
    (WORK / "linux-runtime.json").write_text(json.dumps(record, indent=2) + "\n")
    (ROOT / "docs/native-sentinel/linux-runtime.json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()
