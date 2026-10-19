#!/bin/sh
# Run before Cargo, so diagnostics also work when the executable cannot load GDAL.
set -eu
for tool in cargo rustc cc cmake pkg-config gdal-config gdalinfo; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "Missing $tool. See docs/installation.md for platform prerequisites." >&2
        exit 1
    fi
done
rustc --version
cargo --version
gdal-config --version
gdalinfo --version
if ! gdalinfo --formats | grep -q GTiff; then
    echo 'GDAL lacks GTiff support. Install the platform GDAL package.' >&2
    exit 1
fi
if [ "$#" -gt 0 ]; then
    if ! "$1" --help >/dev/null; then
        echo 'Executable failed to load/run. Rebuild against the installed system GDAL; do not mix Homebrew/Conda prefixes. See docs/installation.md.' >&2
        exit 1
    fi
fi
echo 'Basic build/runtime checks passed; this does not validate remote access or a dataset.'
