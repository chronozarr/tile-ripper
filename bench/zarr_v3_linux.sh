# Clean Linux feature builds in rust:1.92-slim, with the same pinned registry.
# Use: docker run --rm -v "$PWD:/src" -v "$HOME/.cargo/registry:/usr/local/cargo/registry" -w /src rust:1.92-slim sh bench/zarr_v3_linux.sh
set -eu
export DEBIAN_FRONTEND=noninteractive CARGO_INCREMENTAL=0
receipt=/src/prototype-output/zarr-v3-input/distribution
mkdir -p "$receipt"
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential cmake pkg-config python3 > "$receipt/tools.log"
export GDAL_CONFIG=/no-gdal-installed
rustc --version > "$receipt/rustc.txt"
for profile in native zarr; do
    if [ "$profile" = native ]; then features='native-cog'; else features=''; fi
    cargo test --locked --offline --release --no-default-features --features "$features" --target-dir /tmp/v3-clean > "$receipt/$profile-tests.log" 2>&1
    cargo build --locked --offline --release --no-default-features --features "$features" --bin tile-ripper --target-dir /tmp/v3-clean > "$receipt/$profile-build.log" 2>&1
    cp /tmp/v3-clean/release/tile-ripper "$receipt/linux-$profile"
    ldd "$receipt/linux-$profile" > "$receipt/$profile-ldd.txt"
    if ldd "$receipt/linux-$profile" | grep -Ei 'libgdal|libproj|libtiff'; then exit 1; fi
    "$receipt/linux-$profile" --help > "$receipt/$profile-help.txt"
    python3 - "$profile" <<'PY'
import json,subprocess,sys
from pathlib import Path
r=json.loads(Path('/src/prototype-output/zarr-v3-input/smoke.json').read_text());r['source']['assets'][0]['locator']='/src/prototype-output/zarr-v3-input/smoke';r['sink']={'kind':'callback'}
p=Path('/tmp/v3-request.json');p.write_text(json.dumps(r))
a=subprocess.run(['/src/prototype-output/zarr-v3-input/distribution/linux-'+sys.argv[1],'stream',str(p)],capture_output=True,text=True)
assert a.returncode==0,a.stderr
Path('/src/prototype-output/zarr-v3-input/distribution/'+sys.argv[1]+'-stream.json').write_text(a.stdout)
PY
    rm -rf /tmp/v3-clean
 done
unset GDAL_CONFIG
apt-get install -y -qq --no-install-recommends libgdal-dev > "$receipt/gdal-tools.log"
cargo test --locked --offline --release --all-features --target-dir /tmp/v3-clean > "$receipt/combined-tests.log" 2>&1
cargo build --locked --offline --release --all-features --bin tile-ripper --target-dir /tmp/v3-clean > "$receipt/combined-build.log" 2>&1
cp /tmp/v3-clean/release/tile-ripper "$receipt/linux-combined"
ldd "$receipt/linux-combined" > "$receipt/combined-ldd.txt"
"$receipt/linux-combined" --help > "$receipt/combined-help.txt"
