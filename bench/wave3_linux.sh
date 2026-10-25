# Clean Linux ARM64 GDAL-free build/test. Source and cached registry are read-only;
# compiler/runtime/target are isolated in the disposable existing rust:1.92-slim image.
set -eu
uname -a
rustc --version
sha256sum Cargo.toml src/hdf5.rs src/scientific_output/arrays.rs src/products/arrays.rs src/products/arrays/smap.rs src/products/arrays/nisar.rs src/cli/products.rs
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential cmake pkg-config > /tmp/wave3-tools.log
useradd --uid 1000 --create-home wave3-test
mkdir /tmp/wave3-target /tmp/wave3-cargo
ln -s /usr/local/cargo/registry /tmp/wave3-cargo/registry
chown -R wave3-test /tmp/wave3-target /tmp/wave3-cargo
su -s /bin/sh wave3-test -c '
  id
  export CARGO_HOME=/tmp/wave3-cargo CARGO_TARGET_DIR=/tmp/wave3-target CARGO_BUILD_JOBS=2
  /usr/local/cargo/bin/cargo test --locked --offline --release --no-default-features \
    --features native-cog,native-hdf5,authenticated-http --all-targets -- --test-threads=2
'
ldd /tmp/wave3-target/release/tile-ripper
if ldd /tmp/wave3-target/release/tile-ripper | grep -Ei 'gdal|libhdf5'; then exit 1; fi
