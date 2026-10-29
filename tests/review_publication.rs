//! Publication regressions for the public prepared-acquisition API.
#![cfg(feature = "gdal")]

use gdal::{raster::Buffer, DriverManager};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
use tile_ripper::{acquisition, cog::LayoutCache, *};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tile-ripper-review-publication-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn source(&self) -> PathBuf {
        self.0.join("source.tif")
    }

    fn output(&self) -> PathBuf {
        self.0.join("result")
    }

    fn stages(&self) -> Vec<PathBuf> {
        fs::read_dir(&self.0)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                path.file_name()?
                    .to_str()?
                    .contains(".result.partial-")
                    .then_some(path)
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_source(path: &Path) {
    let mut dataset = DriverManager::get_driver_by_name("GTiff")
        .unwrap()
        .create_with_band_type::<u16, _>(path, 4, 4, 1)
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

fn request(source: &Path, output: &Path) -> Request {
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
        sink: Sink::GeoTiffDirectory {
            path: output.to_string_lossy().into(),
        },
        budget: Budget {
            buffer_bytes: 4096,
            max_nodes: 64,
            chunk_edge: 4,
        },
    }
}

#[test]
fn public_geotiff_publication_is_no_replace_and_leaves_no_staging() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let output = fixture.output();
    write_source(&source);
    let prepared =
        acquisition::prepare(&request(&source, &output), &mut LayoutCache::default()).unwrap();
    prepared.fetch().unwrap();
    assert!(output.join("manifest.json").is_file());
    assert!(output.join("chunk-1.tif").is_file());
    assert!(fixture.stages().is_empty());
    assert!(prepared.fetch().is_err());
    assert!(output.join("manifest.json").is_file());
    assert!(fixture.stages().is_empty());
}

#[test]
fn preexecution_source_change_cleans_owned_stage_without_creating_destination() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let output = fixture.output();
    write_source(&source);
    let prepared =
        acquisition::prepare(&request(&source, &output), &mut LayoutCache::default()).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&source)
        .unwrap()
        .write_all(b"changed")
        .unwrap();
    assert!(prepared.fetch().is_err());
    assert!(!output.exists());
    assert!(fixture.stages().is_empty());
}
