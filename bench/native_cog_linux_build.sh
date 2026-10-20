set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential cmake pkg-config time > /src/prototype-output/native-cog/linux-tools.log
if ldconfig -p | grep -Ei 'libgdal|libproj|libtiff'; then exit 1; fi
export GDAL_CONFIG=/no-gdal-installed
export CARGO_TARGET_DIR=/src/prototype-output/native-cog/linux-cli-target
/usr/bin/time -v cargo build --locked --release --no-default-features --features native-cog 2> /src/prototype-output/native-cog/linux-cli-build.log
export CARGO_TARGET_DIR=/src/prototype-output/native-cog/linux-consumer-target
/usr/bin/time -v cargo build --locked --release --manifest-path consumers/native-stats/Cargo.toml --no-default-features --features native-cog 2> /src/prototype-output/native-cog/linux-consumer-build.log
cargo tree --locked --no-default-features --features native-cog > /src/prototype-output/native-cog/linux-tree.txt
ldd /src/prototype-output/native-cog/linux-cli-target/release/tile-ripper > /src/prototype-output/native-cog/linux-ldd.txt
ldd "$CARGO_TARGET_DIR/release/tile-ripper-native-stats" >> /src/prototype-output/native-cog/linux-ldd.txt
"$CARGO_TARGET_DIR/release/tile-ripper-native-stats" /src/prototype-output/native-cog/linux-request.json > /src/prototype-output/native-cog/linux-stdout.jsonl
stat -c '%s %n' /src/prototype-output/native-cog/linux-cli-target/release/tile-ripper "$CARGO_TARGET_DIR/release/tile-ripper-native-stats"
cat /src/prototype-output/native-cog/linux-ldd.txt
