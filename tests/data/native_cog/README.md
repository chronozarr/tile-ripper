# Typed native COG fixtures

These small tiled GeoTIFFs were generated independently with Rasterio 1.4.4
(GDAL 3.12.4). They use a WGS 84 / UTM zone 17N grid, 16 x 16 Deflate tiles,
and storage-domain values chosen to exercise product-like cases:

- `hls_int16.tif`: signed HLS-style reflectance with `scale=0.0001` and
  `nodata=-9999`, using horizontal predictor 2.
- `classification_uint8.tif`: byte classification codes with `nodata=255`,
  using horizontal predictor 2.
- `classification_palette_uint8.tif`: the same raw classification codes with a
  256-entry TIFF palette, verifying storage-domain index reads without RGB
  expansion.
- `opera_dswx_palette_uint8.tif`: an OPERA DSWx-HLS 1.0-like palette raster
  using its observed user-defined UTM/WGS 84-ellipsoid GeoKeys, generated with
  the CRS read independently from the captured provider file.
- `radar_float32.tif`: Float32 radar-like values, finite `nodata=-9999`, two
  distinct NaN payloads, and floating-point predictor 3.
- `radar_float32_nan_nodata.tif`: the same raw Float32 values and NaN payloads
  with the observed RTC-style `GDAL_NODATA=nan` token.
- `qa_uint16_no_nodata.tif`: raw UInt16 QA words with no nodata tag, used to
  verify that polygon AOI masks remain independent from unmodified raw codes.
- `qa_uint16_allzero_no_nodata.tif`: an all-zero UInt16 QA band with no nodata
  tag, used to prove valid zero chunks are physically stored in Zarr v2.
- `pixel_is_point_uint16.tif`: UInt16 data with a PixelIsPoint GeoKey and a
  center tiepoint, used to verify the RFC 33 half-pixel normalization against
  Rasterio's corner-based affine transform.
- `unsupported_int32.tif` and `unsupported_float32_predictor2.tif`: negative
  fixtures proving that type and predictor gates remain narrow.

The values and metadata are asserted by `tests/native_cog.rs`; Rasterio is not
needed to run the Rust tests.
