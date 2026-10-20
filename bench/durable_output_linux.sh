# Run in rust:1.92-slim at /src. Target and tools are removed with the container.
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential cmake pkg-config > /src/prototype-output/durable-output/linux-tools.log
if ldconfig -p | grep -Ei 'libgdal|libproj|libtiff'; then exit 1; fi
export GDAL_CONFIG=/no-gdal-installed
cargo test --release --locked --offline --no-default-features --features native-cog,experimental-output \
  --target-dir /tmp/durable-target --lib scientific_output
cargo build --release --locked --offline --no-default-features --features native-cog,experimental-output \
  --target-dir /tmp/durable-target --bin tile-ripper --example durable_output_candidate
cp /tmp/durable-target/release/tile-ripper /src/prototype-output/durable-output/linux-tile-ripper
cp /tmp/durable-target/release/examples/durable_output_candidate /src/prototype-output/durable-output/linux-candidate
ldd /tmp/durable-target/release/tile-ripper > /src/prototype-output/durable-output/linux-ldd.txt
