#!/bin/sh
# Run before Cargo, so diagnostics also work when the executable cannot load GDAL.
set -eu
mode=default
if [ "${1:-}" = --native ]; then
    mode=native
    shift
fi
required_tools='cargo rustc cc cmake pkg-config'
if [ "$mode" = default ]; then
    required_tools="$required_tools gdal-config gdalinfo"
fi
for tool in $required_tools; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "Missing $tool. See docs/installation.md for platform prerequisites." >&2
        exit 1
    fi
done
rustc --version
cargo --version
if [ "$mode" = default ]; then
    gdal-config --version
    gdalinfo --version
    if ! gdalinfo --formats | grep -q GTiff; then
        echo 'GDAL lacks GTiff support. Install the platform GDAL package.' >&2
        exit 1
    fi
fi
if [ "$#" -gt 0 ]; then
    if ! "$1" --help >/dev/null; then
        echo 'Executable failed to load/run. Check its build tools and linked runtime. See docs/installation.md.' >&2
        exit 1
    fi
fi
echo 'Basic build/runtime checks passed; this does not validate remote access or a dataset.'
