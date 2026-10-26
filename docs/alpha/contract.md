# Current private candidate contract

This contract covers the implementation qualified through `a5e18c2`, with local
consolidation tracked in [the checklist](../consolidation.md). Cargo version remains
`0.1.0-alpha.1`; no released version is being replaced. Source distribution awaits
the [owner decisions and handoff review](closeout.md). API and manifest schemas
may change. The October 19 contract is retained in Git history at `0f1a1b4`.

## Build and platform scope

Cargo defaults to the GDAL feature. The complete native candidate is built with
`--no-default-features --features native-cog,native-hdf5,authenticated-http`.
Zarr-only, native-COG-only and native-HDF5-only builds remain available.
GeoTIFF writing requires GDAL. C/C++ codec build tools are still needed for native
builds. See [installation](../installation.md) and [repeatable validation](../consolidation/validation.md).

The measured platform scope is macOS ARM64 and Linux ARM64 containers. Rust 1.91
is the declared minimum; exact current toolchain/build results are recorded in the
consolidation receipt. Windows, Intel macOS, Linux x86_64, musl and WASM remain
unverified. Container evidence does not replace a fresh physical-machine trial.

## Qualified profiles

Each linked report controls the admitted source version, layout, variables,
geographic selection and access route. Descriptor availability does not qualify
every variable, release or specimen in a mission family.

| Profile | Contract and evidence |
| --- | --- |
| Earth Search Sentinel-2 C1 L2A | [Native UInt16 polygon acquisition](../native-sentinel/workflow.md), separate observations and native quality/AOI handling; [scientific Zarr output](../scientific-output/workflow.md). |
| Landsat 8/9 C2 SR, HLS L30/S30 v2, OPERA DSWx-HLS and RTC-S1 | [Wave 1 qualification](../products/qualification.md), exact bands, radiometry, raw QA/classification and separately qualified public/Earthdata routes. [Provider routing](../providers/qualification.md) governs automatic versus explicit access. |
| Landsat 4/5 TM, Landsat 7 ETM+, DSWx-S1 and DIST-ALERT | [Wave 2 qualification](../products/wave2/qualification.md); sensor/release-specific policies, raw SLC-off gaps and source lineage. |
| GOES-19 CMIPC C13/C01, SMAP SPL3SMP_E v6 global AM/PM, provisional NISAR GCOV frequency-A HHHH | [Wave 3 qualification](../products/wave3/qualification.md): native signed science/quality, exact stored axes, distinct navigation models and explicit archive policies. |
| NISAR geographic HHHH on inspected 10/20 m UTM16N grids, northern SMAP AM/PM, GOES-19 mesoscale-1 C02 | [Wave 4 qualification](../products/wave4/qualification.md): source-center AOI membership, exact inspected shapes/registration and separate raw science, source-validity, AOI and quality masks. |
| SWOT Raster D PGD0 | [Held-out source qualification](../held-out-qualification/report.md), [geographic selection](../swot-aoi/workflow.md) and [one-to-four-observation jobs](../swot-job/workflow.md). [Authenticated ranges](../earthdata-auth/workflow.md) and [opt-in transport resume](../transfer-resume/workflow.md) have separate bounded contracts. |
| Narrow archived NetCDF-4/HDF5 and SWOT C experiments | [Physical HDF5 admission](../native-hdf5/workflow.md) and [SWOT C limits](../swot-wse/report.md); these do not establish generic live archive support. |
| WeatherBench2 ERA5 64×32 and NASA POWER IMERG daily Zarr v2 | [ERA5](../cf-zarr-workflow.md) and [IMERG](../second-zarr-workflow.md): admitted Float32/NaN, dimensions, calendars, coordinates and codecs. |
| Public GFS Zarr v3, local custom-grid Zarr/direct GeoTIFF | [Exact v3 input profile](../zarr-v3-input/workflow.md), [local Zarr](../zarr-workflow.md) and [admitted native TIFF](../native-cog/report.md). Other layouts fail admission. |

The [AlphaEarth GeoZarr prototype](../alphaearth/workflow.md) provides bounded
native Int8 streaming only. Durable embedding output remains outside that
prototype. The [coverage roadmap](../products/roadmap.md) identifies candidates;
ATL06/08, ATL03, GEDI and NISAR HVHV are not runtime-qualified by this contract.

## Plans and physical identity

`plan` and dry runs perform bounded discovery, metadata/coordinate reads and
source-identity checks. They are network operations. Saved inspection JSON is
not executable; fetch prepares the original request again. Estimates and observed
traffic are separate. GDAL's physical wire totals may be unavailable.

Native selection retains original grids and types. Stored scale/offset and units
remain metadata; xarray's default CF decoding can transform the in-memory values.
Use `decode_cf=False` when comparing raw types/bits. Native source missingness,
AOI inclusion and named quality decisions are separate; inspect the applicable
product report before combining masks or observations.

Credentials are explicit and confined to the qualified archive policy. Private
netrc files and signed resources stay outside request/provenance files. Automatic
provider routing follows the [provider contract](../providers/qualification.md);
prepared acquisitions never switch providers. Public catalog relevance does not
prove pixel coverage, measurement quality or collection completeness.

## Completion and failure

Native scientific fetch uses staging, final version validation and no-replace
publication. Complete output and stream receipts authorize use; chunks delivered
before `finish()` remain provisional. Caught failures remove newly owned outputs.
Abrupt termination can leave incomplete/staging directories; inspect them and
choose a new destination. Existing output directories are rejected.

The only qualified resume mechanism is the explicit SWOT source-range transport
checkpoint. Its consumer restarts and must finish validation. General product
checkpoint/resume is unsupported. Per-object ETags do not create an atomic
whole-dataset snapshot or authenticate the provider. Payload reservations do not
bound process RSS or all GDAL wire bytes.

No implicit calibration, reprojection, resampling, mosaicking, averaging,
compositing, longitude wrapping, universal cloud mask, generic CF/HDF5 support,
Python acquisition bindings or scheduler is supplied. There is no consistent
provider speed advantage over tuned GDAL/xarray.
