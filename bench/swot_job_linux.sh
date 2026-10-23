# Focused serial job/namespace checks on Linux ARM64 in existing rust:1.92-slim.
# Source/registry are read-only; outputs/target live on the container filesystem.
set -eu
uname -a
rustc --version
id
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential cmake pkg-config > /tmp/tools.log
useradd --uid 1000 --create-home job-test
mkdir /tmp/job-target
chown job-test /tmp/job-target
su -s /bin/sh job-test -c '
  id
  export CARGO_HOME=/usr/local/cargo CARGO_TARGET_DIR=/tmp/job-target
  /usr/local/cargo/bin/cargo test --locked --offline --no-default-features \
    --features native-hdf5,authenticated-http --lib \
    checkpoint::tests::job_root_lock_static_quotas_and_lowered_limit_inventory -- --exact
  /usr/local/cargo/bin/cargo test --locked --offline --no-default-features \
    --features native-hdf5,authenticated-http --test swot_job
'
