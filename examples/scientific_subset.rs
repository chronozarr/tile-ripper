//! cargo run --release --locked --example scientific_subset -- ./new-era5-output
use tile_ripper::{
    acquisition, cog::LayoutCache, Asset, Budget, Format, Grid, Request, Result, Sink, Source,
    Spatial, Time, Variable,
};
fn main() -> Result<()> {
    let output = std::env::args()
        .nth(1)
        .ok_or("provide a new output directory")?;
    let request = Request {
        source: Source::Assets { assets: vec![Asset {
            id: "era5".into(),
            locator: "https://storage.googleapis.com/weatherbench2/datasets/era5/1959-2023_01_10-6h-64x32_equiangular_conservative.zarr/".into(),
            format: Format::ZarrV2, datetime: None, etag: None,
        }] },
        spatial: Spatial::Bounds { crs: "EPSG:4326".into(), bbox: [100., -10., 110., 0.] },
        time: Some(Time::Interval { start: "1960-02-02T12:00:00Z".into(), end: "1960-02-02T18:00:00Z".into() }),
        variables: vec![Variable::Dataset {path: "2m_temperature".into()}],
        grid: Grid::Native, quality: vec![], operations: vec![],
        sink: Sink::Directory {path: output},
        budget: Budget {buffer_bytes: 32*1024*1024, max_nodes: 100000, chunk_edge: 512},
    };
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default())?;
    println!("{}", prepared.human());
    let result = prepared.fetch()?;
    println!("status: {}", result["status"]);
    Ok(())
}
