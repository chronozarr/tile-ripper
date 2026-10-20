# Clean local source installation; no publish or deployment.
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential cmake pkg-config time > /src/prototype-output/native-sentinel/linux-tools.log
if ldconfig -p | grep -Ei 'libgdal|libproj|libtiff'; then exit 1; fi
export GDAL_CONFIG=/no-gdal-installed
/usr/bin/time -v cargo install --path . --locked --no-default-features --features native-cog \
  --root /tmp/tile-ripper-sentinel-install --target-dir /tmp/tile-ripper-sentinel-install-target \
  2> /src/prototype-output/native-sentinel/linux-install-build.log
cp /tmp/tile-ripper-sentinel-install/bin/tile-ripper /src/prototype-output/native-sentinel/linux-tile-ripper
/usr/bin/time -v cargo build --locked --release --manifest-path consumers/native-stats/Cargo.toml \
  --no-default-features --features native-cog --target-dir /tmp/tile-ripper-sentinel-consumer-target \
  2> /src/prototype-output/native-sentinel/linux-consumer-build.log
cp /tmp/tile-ripper-sentinel-consumer-target/release/tile-ripper-native-stats /src/prototype-output/native-sentinel/linux-tile-ripper-native-stats
cargo tree --locked --no-default-features --features native-cog > /src/prototype-output/native-sentinel/linux-tree.txt
ldd /src/prototype-output/native-sentinel/linux-tile-ripper > /src/prototype-output/native-sentinel/linux-ldd.txt
ldd /src/prototype-output/native-sentinel/linux-tile-ripper-native-stats >> /src/prototype-output/native-sentinel/linux-ldd.txt
/src/prototype-output/native-sentinel/linux-tile-ripper --help > /src/prototype-output/native-sentinel/linux-installed-help.txt
