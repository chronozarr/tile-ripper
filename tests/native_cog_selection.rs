#![cfg(all(feature = "native-cog", not(feature = "gdal")))]

use flate2::{write::ZlibEncoder, Compression};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
use tile_ripper::{acquisition, cog::LayoutCache, *};

static NEXT: AtomicUsize = AtomicUsize::new(0);
const SIDE: usize = 768;
const BLOCK: usize = 256;

#[derive(Clone, Copy)]
enum Sample {
    Byte,
    Int16,
    Float32,
}
impl Sample {
    fn bytes(self) -> usize {
        match self {
            Self::Byte => 1,
            Self::Int16 => 2,
            Self::Float32 => 4,
        }
    }
    fn format(self) -> u16 {
        match self {
            Self::Byte => 1,
            Self::Int16 => 2,
            Self::Float32 => 3,
        }
    }
    fn nodata(self) -> &'static str {
        match self {
            Self::Byte => "255\0",
            Self::Int16 | Self::Float32 => "-9999\0",
        }
    }
    fn value(self, x: usize, y: usize) -> Vec<u8> {
        let index = y * SIDE + x;
        match self {
            Self::Byte => vec![if index.is_multiple_of(97) {
                255
            } else {
                ((x * 5 + y * 7) % 241) as u8
            }],
            Self::Int16 => {
                let value = if index.is_multiple_of(97) {
                    -9999
                } else {
                    ((x * 37 + y * 17) % 24001) as i16 - 12000
                };
                value.to_le_bytes().to_vec()
            }
            Self::Float32 => {
                let value = match index % 19 {
                    0 => f32::from_bits(0x7fc0_1234),
                    1 => f32::from_bits(0x7fc0_5678),
                    2 => -0.0,
                    3 => -9999.0,
                    _ => (x as f32 - y as f32) * 0.03125,
                };
                value.to_bits().to_le_bytes().to_vec()
            }
        }
    }
}

struct Fixture(PathBuf);
impl Fixture {
    fn new(sample: Sample) -> Self {
        let path = std::env::temp_dir().join(format!(
            "tile-ripper-selection-{}-{}.tif",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        write_tiff(&path, sample);
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Construct a classic little-endian tiled GeoTIFF independently of the reader.
/// Contiguous Deflate tiles make the exact selected range grouping observable.
fn write_tiff(path: &Path, sample: Sample) {
    let tiles: Vec<Vec<u8>> = (0..SIDE / BLOCK)
        .flat_map(|ty| {
            (0..SIDE / BLOCK).map(move |tx| {
                let mut encoded = ZlibEncoder::new(Vec::new(), Compression::fast());
                for y in ty * BLOCK..(ty + 1) * BLOCK {
                    for x in tx * BLOCK..(tx + 1) * BLOCK {
                        encoded.write_all(&sample.value(x, y)).unwrap();
                    }
                }
                encoded.finish().unwrap()
            })
        })
        .collect();
    let short = |v: u16| v.to_le_bytes().to_vec();
    let long = |v: usize| (v as u32).to_le_bytes().to_vec();
    let doubles = |v: &[f64]| v.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>();
    let keys: Vec<u8> = [
        1_u16, 1, 0, 3, 1024, 0, 1, 1, 1025, 0, 1, 1, 3072, 0, 1, 32617,
    ]
    .iter()
    .flat_map(|v| v.to_le_bytes())
    .collect();
    let mut tags = vec![
        (256_u16, 4_u16, 1_u32, long(SIDE)),
        (257, 4, 1, long(SIDE)),
        (258, 3, 1, short((sample.bytes() * 8) as u16)),
        (259, 3, 1, short(8)),
        (262, 3, 1, short(1)),
        (277, 3, 1, short(1)),
        (284, 3, 1, short(1)),
        (317, 3, 1, short(1)),
        (322, 4, 1, long(BLOCK)),
        (323, 4, 1, long(BLOCK)),
        (324, 4, tiles.len() as u32, vec![0; tiles.len() * 4]),
        (
            325,
            4,
            tiles.len() as u32,
            tiles.iter().flat_map(|v| long(v.len())).collect(),
        ),
        (339, 3, 1, short(sample.format())),
        (33550, 12, 3, doubles(&[1., 1., 0.])),
        (33922, 12, 6, doubles(&[0., 0., 0., 500000., 4000000., 0.])),
        (34735, 3, 16, keys),
        (
            42113,
            2,
            sample.nodata().len() as u32,
            sample.nodata().as_bytes().to_vec(),
        ),
    ];
    tags.sort_by_key(|t| t.0);
    let mut bytes = vec![0; 8 + 2 + tags.len() * 12 + 4];
    bytes[..8].copy_from_slice(b"II\x2a\0\x08\0\0\0");
    bytes[8..10].copy_from_slice(&(tags.len() as u16).to_le_bytes());
    let mut offsets_at = 0;
    for (i, (tag, kind, count, values)) in tags.into_iter().enumerate() {
        let entry = 10 + i * 12;
        bytes[entry..entry + 2].copy_from_slice(&tag.to_le_bytes());
        bytes[entry + 2..entry + 4].copy_from_slice(&kind.to_le_bytes());
        bytes[entry + 4..entry + 8].copy_from_slice(&count.to_le_bytes());
        if values.len() <= 4 {
            bytes[entry + 8..entry + 8 + values.len()].copy_from_slice(&values);
        } else {
            let offset = bytes.len();
            bytes[entry + 8..entry + 12].copy_from_slice(&(offset as u32).to_le_bytes());
            bytes.extend(values);
            if tag == 324 {
                offsets_at = offset;
            }
        }
    }
    for (i, tile) in tiles.into_iter().enumerate() {
        let offset = bytes.len() as u32;
        bytes[offsets_at + i * 4..offsets_at + (i + 1) * 4].copy_from_slice(&offset.to_le_bytes());
        bytes.extend(tile);
    }
    fs::write(path, bytes).unwrap();
}

fn request(path: &Path, spatial: Spatial) -> Request {
    Request {
        source: Source::Assets {
            assets: vec![Asset {
                id: "selection-fixture".into(),
                locator: path.to_string_lossy().into(),
                format: Format::Cog,
                datetime: Some("2025-07-13T15:17:29Z".into()),
                etag: None,
            }],
        },
        spatial,
        time: None,
        variables: vec![Variable::Band { index: 1 }],
        quality: vec![],
        grid: Grid::Native,
        operations: vec![],
        sink: Sink::Callback,
        budget: Budget {
            buffer_bytes: 16 << 20,
            max_nodes: 256,
            chunk_edge: 256,
        },
    }
}

fn assert_values(chunk: &Chunk, sample: Sample) {
    let expected: Vec<_> = (chunk.window.y..chunk.window.y + chunk.window.height)
        .flat_map(|y| {
            (chunk.window.x..chunk.window.x + chunk.window.width)
                .flat_map(move |x| sample.value(x, y))
        })
        .collect();
    let actual: Vec<_> = match &chunk.pixels {
        Pixels::UInt8(v) => v.clone(),
        Pixels::Int16(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
        Pixels::Float32(v) => v.iter().flat_map(|v| v.to_bits().to_le_bytes()).collect(),
        _ => panic!("unexpected stored sample type"),
    };
    assert_eq!(actual, expected, "source window {:?}", chunk.window);
    assert_eq!(chunk.crs, "EPSG:32617");
    assert_eq!(chunk.datetime.as_deref(), Some("2025-07-13T15:17:29Z"));
    assert_eq!(
        chunk.transform,
        [
            500000. + chunk.window.x as f64,
            1.,
            0.,
            4000000. - chunk.window.y as f64,
            0.,
            -1.,
        ]
    );
}

#[test]
fn misaligned_selection_coalesces_exact_six_tiles_and_preserves_all_typed_bits() {
    for sample in [Sample::Byte, Sample::Int16, Sample::Float32] {
        let fixture = Fixture::new(sample);
        let request = request(
            &fixture.0,
            Spatial::Bounds {
                crs: "EPSG:32617".into(),
                bbox: [500242., 3999648., 500574., 3999981.],
            },
        );
        let prepared = acquisition::prepare(&request, &mut LayoutCache::default()).unwrap();
        let selected = &prepared.plan().selected[0];
        assert_eq!(selected.window.x, 242);
        assert_eq!(selected.window.y, 19);
        assert_eq!(selected.window.width, 332);
        assert_eq!(selected.window.height, 333);
        let layout = selected.layout.as_ref().unwrap();
        let tiles = cog::tiles(layout, &selected.window);
        assert_eq!(tiles.len(), 6);
        let payload: u64 = tiles.iter().map(|t| t.bytes.unwrap()).sum();
        let mut windows = Vec::new();
        let report = prepared
            .execute(|chunk| {
                assert_values(&chunk, sample);
                windows.push((chunk.window.x, chunk.window.y));
                Ok(())
            })
            .unwrap();
        assert_eq!(windows, [(242, 19), (498, 19), (242, 275), (498, 275)]);
        assert_eq!(report.source_data_requests, Some(1));
        assert_eq!(report.source_data_bytes, Some(payload));
        let strategy = &report.scheduler["native_strategies"][0]["strategy"];
        assert_eq!(strategy["read_scope"], "bounded_selection");
        assert_eq!(
            strategy["retained_selection_bytes"],
            332 * 333 * sample.bytes()
        );
        assert_eq!(strategy["coalesced_groups"], 1);
        assert!(report.scheduler["reserved_bytes_peak"].as_u64().unwrap() <= 16 << 20);
    }
}

#[test]
fn tight_buffer_keeps_exact_window_delivery_when_the_selection_cannot_fit() {
    let fixture = Fixture::new(Sample::Byte);
    let mut request = request(
        &fixture.0,
        Spatial::Bounds {
            crs: "EPSG:32617".into(),
            bbox: [500000., 3999232., 500768., 4000000.],
        },
    );
    request.budget.buffer_bytes = 768 << 10;
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default()).unwrap();
    let layout = prepared.plan().selected[0].layout.as_ref().unwrap();
    let payload: u64 = layout.tiles.iter().map(|t| t.bytes.unwrap()).sum();
    let mut chunks = 0;
    let report = prepared
        .execute(|chunk| {
            assert_values(&chunk, Sample::Byte);
            chunks += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(chunks, 9);
    assert_eq!(report.source_data_requests, Some(9));
    assert_eq!(report.source_data_bytes, Some(payload));
    assert!(report.scheduler["reserved_bytes_peak"].as_u64().unwrap() <= 768 << 10);
    assert_eq!(
        report.scheduler["native_strategies"][0]["strategy"]["read_scope"],
        "planned_windows"
    );
}

#[test]
fn a_masked_tile_gap_never_broadens_the_selected_compressed_tile_set() {
    let fixture = Fixture::new(Sample::Byte);
    let request = request(
        &fixture.0,
        Spatial::Polygon {
            crs: "EPSG:32617".into(),
            coordinates: serde_json::json!([
                [
                    [500000., 4000000.],
                    [500768., 4000000.],
                    [500768., 3999232.],
                    [500000., 3999232.],
                    [500000., 4000000.]
                ],
                [
                    [500256., 3999744.],
                    [500512., 3999744.],
                    [500512., 3999488.],
                    [500256., 3999488.],
                    [500256., 3999744.]
                ]
            ]),
        },
    );
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default()).unwrap();
    let layout = prepared.plan().selected[0].layout.as_ref().unwrap();
    let payload: u64 = layout
        .tiles
        .iter()
        .filter(|tile| (tile.x, tile.y) != (1, 1))
        .map(|t| t.bytes.unwrap())
        .sum();
    let mut chunks = 0;
    let report = prepared
        .execute(|chunk| {
            assert_values(&chunk, Sample::Byte);
            assert_ne!((chunk.window.x, chunk.window.y), (256, 256));
            chunks += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(chunks, 8);
    assert_eq!(report.source_data_requests, Some(8));
    assert_eq!(report.source_data_bytes, Some(payload));
    assert_eq!(
        report.scheduler["native_strategies"][0]["strategy"]["read_scope"],
        "planned_windows"
    );
}
