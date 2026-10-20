# Clean local source installation in a disposable Linux builder; never publishes.
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential cmake pkg-config time > /src/prototype-output/native-cog/linux-install-tools.log
if ldconfig -p | grep -Ei 'libgdal|libproj|libtiff'; then exit 1; fi
export GDAL_CONFIG=/no-gdal-installed
/usr/bin/time -v cargo install --path . --locked --no-default-features --features native-cog \
    --root /tmp/tile-ripper-native-install --target-dir /src/prototype-output/native-cog/linux-install-target \
    2> /src/prototype-output/native-cog/linux-install-build.log
/tmp/tile-ripper-native-install/bin/tile-ripper --help > /src/prototype-output/native-cog/linux-installed-help.txt
/tmp/tile-ripper-native-install/bin/tile-ripper plan /src/prototype-output/native-cog/linux-runtime-request.json --json > /src/prototype-output/native-cog/linux-installed-plan.json
ldd /tmp/tile-ripper-native-install/bin/tile-ripper > /src/prototype-output/native-cog/linux-installed-ldd.txt
stat -c '%s' /tmp/tile-ripper-native-install/bin/tile-ripper > /src/prototype-output/native-cog/linux-installed-size.txt
cp /tmp/tile-ripper-native-install/bin/tile-ripper /src/prototype-output/native-cog/linux-installed-tile-ripper
