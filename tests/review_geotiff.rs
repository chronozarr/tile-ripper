#[cfg(feature = "gdal")]
use gdal::{
    raster::{Buffer, RasterCreationOptions},
    DriverManager,
};
#[cfg(all(feature = "gdal", feature = "native-cog"))]
use std::process::Command;
#[cfg(feature = "gdal")]
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
#[cfg(feature = "gdal")]
use tile_ripper::{acquisition, cog::LayoutCache, *};

#[cfg(feature = "gdal")]
static NEXT: AtomicUsize = AtomicUsize::new(0);

#[cfg(feature = "gdal")]
struct Fixture(PathBuf);

#[cfg(feature = "gdal")]
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tile-ripper-review-geotiff-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}

#[cfg(feature = "gdal")]
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(feature = "gdal")]
fn write_uint16(path: &Path) {
    let mut dataset = DriverManager::get_driver_by_name("GTiff")
        .unwrap()
        .create_with_band_type_with_options::<u16, _>(
            path,
            4,
            4,
            1,
            &RasterCreationOptions::from_iter([
                "TILED=YES",
                "BLOCKXSIZE=16",
                "BLOCKYSIZE=16",
                "COMPRESS=DEFLATE",
            ]),
        )
        .unwrap();
    dataset
        .set_geo_transform(&[0., 10., 0., 40., 0., -10.])
        .unwrap();
    dataset
        .set_spatial_ref(&gdal::spatial_ref::SpatialRef::from_epsg(32617).unwrap())
        .unwrap();
    dataset
        .rasterband(1)
        .unwrap()
        .write((0, 0), (4, 4), &mut Buffer::new((4, 4), vec![7_u16; 16]))
        .unwrap();
}

#[cfg(feature = "gdal")]
fn request(source: &Path, sink: Sink) -> Request {
    Request {
        source: Source::Assets {
            assets: vec![Asset {
                etag: None,
                id: "source".into(),
                locator: source.to_string_lossy().into(),
                format: Format::Cog,
                datetime: None,
            }],
        },
        spatial: Spatial::Bounds {
            crs: "EPSG:32617".into(),
            bbox: [0., 0., 40., 40.],
        },
        time: None,
        variables: vec![Variable::Band { index: 1 }],
        quality: vec![],
        grid: Grid::Native,
        operations: vec![],
        sink,
        budget: Budget {
            buffer_bytes: 4096,
            max_nodes: 64,
            chunk_edge: 4,
        },
    }
}

#[test]
#[cfg(feature = "gdal")]
fn prepared_geotiff_fetch_uses_its_bound_sources_and_writes_uint16() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source.tif");
    let output = fixture.0.join("output");
    write_uint16(&source);

    let prepared = acquisition::prepare(
        &request(
            &source,
            Sink::GeoTiffDirectory {
                path: output.to_string_lossy().into(),
            },
        ),
        &mut LayoutCache::default(),
    )
    .unwrap();
    prepared.fetch().unwrap();

    assert!(output.join("chunk-1.tif").is_file());
}

/// The native reader's physical metadata is installed during preparation.
/// Before the prepared-source handoff fix, `fetch_on` replaced it with an empty
/// `Sources`, so this ordinary subprocess fetch reported zero prepared native
/// metadata sources even though the fixture was successfully decoded.
#[test]
#[cfg(all(feature = "gdal", feature = "native-cog"))]
fn native_fetch_retains_prepared_source_bindings() {
    let fixture = Fixture::new();
    let output = fixture.0.join("output");
    let spec = fixture.0.join("request.json");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/native_cog/qa_uint16_no_nodata.tif");
    fs::write(
        &spec,
        serde_json::to_vec(&request(
            &source,
            Sink::GeoTiffDirectory {
                path: output.to_string_lossy().into(),
            },
        ))
        .unwrap(),
    )
    .unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_tile-ripper"))
        .args(["fetch", spec.to_str().unwrap(), "--backend", "native"])
        .env_remove("TILE_RIPPER_COG_BACKEND")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
    let native = &manifest["observed"]["scheduler"]["native_metadata"];
    assert_eq!(native["physical_metadata_sources"], 1, "{native}");
    assert!(native["physical_metadata_bytes"]
        .as_u64()
        .is_some_and(|n| n > 0));
}

#[cfg(all(feature = "native-cog", not(feature = "gdal")))]
mod native_sink_validation {
    use std::{
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };
    use tile_ripper::{acquisition, cog::LayoutCache, *};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "tile-ripper-review-geotiff-native-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn request(name: &str, sink: Sink) -> Request {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/native_cog")
            .join(name);
        Request {
            source: Source::Assets {
                assets: vec![Asset {
                    etag: None,
                    id: "source".into(),
                    locator: source.to_string_lossy().into(),
                    format: Format::Cog,
                    datetime: None,
                }],
            },
            spatial: Spatial::Bounds {
                crs: "EPSG:32617".into(),
                bbox: [500000., 3999040., 500960., 4000000.],
            },
            time: None,
            variables: vec![Variable::Band { index: 1 }],
            quality: vec![],
            grid: Grid::Native,
            operations: vec![],
            sink,
            budget: Budget {
                buffer_bytes: 16 * 1024 * 1024,
                max_nodes: 4096,
                chunk_edge: 64,
            },
        }
    }

    #[test]
    fn typed_native_cogs_are_rejected_before_any_geotiff_destination_is_created() {
        let fixture = Fixture::new();
        for (source, sink) in [
            (
                "classification_uint8.tif",
                Sink::GeoTiffDirectory {
                    path: fixture.0.join("uint8").to_string_lossy().into(),
                },
            ),
            (
                "hls_int16.tif",
                Sink::GeoTiffDirectory {
                    path: fixture.0.join("int16").to_string_lossy().into(),
                },
            ),
            (
                "radar_float32.tif",
                // This is the convenience sink; preparation normalizes it to
                // GeoTIFF for COGs, so it must receive the same gate.
                Sink::Directory {
                    path: fixture.0.join("float32").to_string_lossy().into(),
                },
            ),
        ] {
            let path = match &sink {
                Sink::Directory { path } | Sink::GeoTiffDirectory { path } => path.clone(),
                _ => unreachable!(),
            };
            let error =
                match acquisition::prepare(&request(source, sink), &mut LayoutCache::default()) {
                    Ok(_) => panic!("{source} unexpectedly prepared for GeoTIFF output"),
                    Err(error) => error.to_string(),
                };
            assert!(error.contains("GeoTIFF directory output currently supports UInt16"));
            assert!(!Path::new(&path).exists());
        }
    }

    #[test]
    fn direct_public_plan_applies_the_directory_to_geotiff_capability_gate() {
        let fixture = Fixture::new();
        let output = fixture.0.join("float32");
        let request = request(
            "radar_float32.tif",
            Sink::Directory {
                path: output.to_string_lossy().into(),
            },
        );
        let error = match tile_ripper::plan(&request, &mut LayoutCache::default()) {
            Ok(_) => panic!("Float32 COG unexpectedly planned for a GeoTIFF directory"),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains("use zarr_directory for UInt8, Int16, or Float32"),
            "{error}"
        );
        assert!(!output.exists());
    }
}
