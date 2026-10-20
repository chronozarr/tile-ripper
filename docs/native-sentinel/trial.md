# Private native Sentinel trial

Technical preparation is ready for a supervised scientist trial on macOS ARM64
or Linux ARM64/glibc. No independent human trial has been completed. Existing
ownership/licensing approval remains a condition of source distribution; this
guide is prepared locally, not an invitation or published release.

Use a fixed maintainer-supplied source revision. Follow the
[installation and request recipe](workflow.md), using
`--no-default-features --features native-cog` for both the CLI and standalone
consumer. Required build tools are Rust 1.92, C/C++, CMake and pkg-config. GDAL,
PROJ, libtiff and Python are unnecessary. Run on a machine/container without them
to establish the installation experience rather than only choosing a native
backend in an executable that still links GDAL.

1. Install the CLI into a new isolated directory and build the native consumer.
2. Inspect `docs/native-sentinel/request.json` with `tile-ripper plan --json`.
   It should select the 28 July 2025 `S2A_T17SPV_20250728T161108_L2A` red and NIR
   assets in requested order, with WGS84 query bounds and native `EPSG:32617` grids.
3. Run the consumer. Require one final `complete` line and a zero exit status.
   The [live receipt](live.json) records the reference windows, valid counts,
   checksums, source versions and exact stored-value comparison for this request.
   If the provider has legitimately changed the source ETag or catalog, preserve
   the discrepancy for investigation rather than treating the old checksum as
   an unversioned universal expected value.
4. Change the polygon, inclusive date interval and one supported band for your
   own small AOI within the documented limits. Inspect selected dates, source
   footprints, native grid and pixel centers. Retain provisional arrays only
   until `finish()` succeeds; commit your analysis after its receipt.
5. Check a deliberate unsupported request, such as a wrapped antimeridian AOI
   or B10, produces a clear error and no successful completion record.
6. Uninstall by removing only the isolated installation/output directories you
   created. No global GDAL or Python environment needs changing.

Record platform/architecture, source revision, build/install outcome and time,
whether GDAL/PROJ/libtiff were absent, request, selected item/bands/native grid,
final receipt and any unexpected failure. Note confusing instructions, whether
you could consume actual chunks in your own Rust analysis, and any maintainer
help needed. Preserve error text with credentials removed. No broad performance
or newcomer-success claim follows from the automated maintainer checks.
