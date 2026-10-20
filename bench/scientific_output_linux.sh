# Clean source installation; run inside rust:1.92-slim at /src.
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential cmake pkg-config time > /src/prototype-output/scientific-output/linux-tools.log
if ldconfig -p | grep -Ei 'libgdal|libproj|libtiff'; then exit 1; fi
export GDAL_CONFIG=/no-gdal-installed
/usr/bin/time -v cargo install --path . --locked --no-default-features --features native-cog \
  --root /tmp/scientific-output-install --target-dir /tmp/scientific-output-clean-target \
  > /src/prototype-output/scientific-output/linux-install-stdout.log \
  2> /src/prototype-output/scientific-output/linux-install.log
cp /tmp/scientific-output-install/bin/tile-ripper /src/prototype-output/scientific-output/linux-tile-ripper
ldd /tmp/scientific-output-install/bin/tile-ripper > /src/prototype-output/scientific-output/linux-ldd.txt
cargo tree --locked --no-default-features --features native-cog > /src/prototype-output/scientific-output/linux-tree.txt
