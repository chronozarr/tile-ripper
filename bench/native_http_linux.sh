# Clean local source build/install. Container-local targets avoid shared-fs artifacts.
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential cmake pkg-config time > /src/prototype-output/native-http/linux-tools.log
if ldconfig -p | grep -Ei 'libgdal|libproj|libtiff'; then exit 1; fi
export GDAL_CONFIG=/no-gdal-installed
/usr/bin/time -v cargo install --path . --locked --no-default-features --features native-cog \
  --root /tmp/tile-ripper-native-http-install --target-dir /tmp/native-http-cli-target \
  2> /src/prototype-output/native-http/linux-install.log
cp /tmp/tile-ripper-native-http-install/bin/tile-ripper /src/prototype-output/native-http/linux-tile-ripper
/usr/bin/time -v cargo build --locked --release --manifest-path consumers/native-stats/Cargo.toml \
  --no-default-features --features native-cog --target-dir /tmp/native-http-consumer-target \
  2> /src/prototype-output/native-http/linux-consumer.log
cp /tmp/native-http-consumer-target/release/tile-ripper-native-stats /src/prototype-output/native-http/linux-native-stats
cargo build --locked --release --no-default-features --features native-cog --example native_cog_oracle \
  --target-dir /tmp/native-http-cli-target 2> /src/prototype-output/native-http/linux-oracle-build.log
cp /tmp/native-http-cli-target/release/examples/native_cog_oracle /src/prototype-output/native-http/linux-exact-oracle
cargo tree --locked --no-default-features --features native-cog > /src/prototype-output/native-http/linux-tree.txt
ldd /src/prototype-output/native-http/linux-tile-ripper > /src/prototype-output/native-http/linux-ldd.txt
ldd /src/prototype-output/native-http/linux-native-stats >> /src/prototype-output/native-http/linux-ldd.txt
