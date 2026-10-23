# Clean Linux ARM64 GDAL-free build and immutable-grid/geographic/source/job regression gates.
# Mount this checkout at /work read-only and the Cargo registry at /cache/registry.
set -eu
uname -a
rustc --version
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential cmake pkg-config > /tmp/swot-aoi-tools.log
useradd --uid 1000 --create-home aoi-test
mkdir -p /tmp/aoi-target /tmp/aoi-cargo
ln -s /cache/registry /tmp/aoi-cargo/registry
chown -R aoi-test /tmp/aoi-target /tmp/aoi-cargo
su -s /bin/sh aoi-test -c '
  export CARGO_HOME=/tmp/aoi-cargo CARGO_TARGET_DIR=/tmp/aoi-target CARGO_INCREMENTAL=0
  cd /work
  cargo test --locked --offline --no-default-features \
    --features native-hdf5,authenticated-http --test grid_inspection --test hdf5 --test swot_aoi --test swot_job --test swot_discovery -- --test-threads=1
  cargo test --locked --offline --no-default-features \
    --features native-hdf5,authenticated-http --lib swot_aoi::tests
  cargo build --locked --offline --no-default-features \
    --features native-hdf5,authenticated-http --bin tile-ripper
  /tmp/aoi-target/debug/tile-ripper discover-job --help
  ldd /tmp/aoi-target/debug/tile-ripper
'
