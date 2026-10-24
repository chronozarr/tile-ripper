# Fresh Linux ARM64 GDAL-free binaries and native gates. Run the stdlib HTTPS
# benchmark separately with these preserved binaries after other work finishes.
# /work is this checkout; registry is read-only at /cache/registry. Only the
# milestone's ignored work/evidence directories need writeable mounts.
set -eu
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential cmake pkg-config python3 ca-certificates > /tmp/source-validation-tools.log
mkdir -p /tmp/validation-cargo /tmp/validation-target
ln -sfn /cache/registry /tmp/validation-cargo/registry
export CARGO_HOME=/tmp/validation-cargo CARGO_INCREMENTAL=0
work=/work/prototype-output/source-validation
features=native-hdf5,native-cog,authenticated-http,experimental-output
uname -a
rustc --version
for version in baseline candidate; do
    # Root crates with the same name/path-relative arguments must never share
    # Cargo fingerprints when switching source trees with older file mtimes.
    export CARGO_TARGET_DIR="/tmp/validation-target-$version"
    if test "$version" = baseline; then
        source_root="$work/baseline-source"
    else
        source_root=/work
    fi
    cargo build --locked --offline --release --no-default-features \
        --features "$features" --manifest-path "$source_root/Cargo.toml" \
        --bin tile-ripper --example source_validation_bench
    mkdir -p "$work/linux/$version"
    cp "$CARGO_TARGET_DIR/release/tile-ripper" "$work/linux/$version/"
    cp "$CARGO_TARGET_DIR/release/examples/source_validation_bench" "$work/linux/$version/"
    ldd "$work/linux/$version/tile-ripper"
done
id validation-test >/dev/null 2>&1 || useradd --uid 1000 --create-home validation-test
chown -R validation-test /tmp/validation-cargo /tmp/validation-target-candidate
su -s /bin/sh validation-test -c '
    export CARGO_HOME=/tmp/validation-cargo CARGO_TARGET_DIR=/tmp/validation-target-candidate CARGO_INCREMENTAL=0
    cd /work
    cargo test --release -j 1 --locked --offline --no-default-features \
        --features native-hdf5,native-cog,authenticated-http,experimental-output \
        --lib --test native_cog --test hdf5 --test grid_inspection \
        --test swot_aoi --test swot_job --test swot_discovery -- --test-threads=1
'
printf 'SOURCE_VALIDATION_LINUX_COMPLETE\n'
