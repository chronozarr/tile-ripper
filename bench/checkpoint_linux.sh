# Focused Linux ARM64 evidence, inside the existing rust:1.92-slim image.
# Source and Cargo registry are mounted read-only; /tmp is the Linux filesystem.
# Run from the checkout with:
# docker run --rm -v "$PWD:/src:ro" -v "$HOME/.cargo/registry:/usr/local/cargo/registry:ro" \
#   -w /src rust:1.92-slim sh bench/checkpoint_linux.sh
set -eu
export DEBIAN_FRONTEND=noninteractive
uname -a
rustc --version
id
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential cmake pkg-config > /tmp/tools.log
useradd --uid 1000 --create-home checkpoint-test
mkdir /tmp/checkpoint-target /tmp/foreign-private
chown checkpoint-test /tmp/checkpoint-target /tmp/foreign-private
chmod 700 /tmp/foreign-private
su -s /bin/sh checkpoint-test -c '
  id
  export CARGO_HOME=/usr/local/cargo CARGO_TARGET_DIR=/tmp/checkpoint-target
  /usr/local/cargo/bin/cargo test --locked --offline --no-default-features \
    --features native-hdf5,authenticated-http --lib checkpoint::tests -- --nocapture
'
# Root can open this 0700 directory, so the explicit UID comparison is tested,
# independently of the non-root permission-denial tests above.
export CARGO_TARGET_DIR=/tmp/checkpoint-target TILE_RIPPER_UNIT_FOREIGN_ROOT=/tmp/foreign-private
cargo test --locked --offline --no-default-features --features native-hdf5,authenticated-http \
  --lib checkpoint::tests::foreign_owned_private_directory_is_rejected -- --exact --ignored --nocapture
