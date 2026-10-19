use crate::{
    cog, fail,
    model::*,
    planner::{push, FormatPlanner},
    Result,
};
use serde::Deserialize;
use std::{fs, path::Path};
#[derive(Deserialize)]
struct Array {
    zarr_format: u8,
    shape: Vec<usize>,
    chunks: Vec<usize>,
    dtype: String,
    order: String,
    dimension_separator: Option<String>,
    compressor: serde_json::Value,
    filters: serde_json::Value,
}
#[derive(Deserialize)]
struct Attributes {
    tile_ripper_grid: GeoGrid,
}
#[derive(Deserialize)]
struct GeoGrid {
    crs: String,
    transform: [f64; 6],
}
pub struct ZarrPlanner;
impl FormatPlanner for ZarrPlanner {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            selection: vec!["2D native_bounds with explicit georeferencing".into()],
            reads: vec!["chunk_object".into()],
            decoding: "not implemented".into(),
            compression: "metadata preserved, execution unsupported".into(),
            materialization: false,
        }
    }
    fn plan_asset(&mut self, asset: &Asset, r: &Request, p: &mut Plan) -> Result<()> {
        if cog::remote(&asset.locator) {
            return fail("Zarr planning prototype requires local metadata");
        }
        if r.variables.len() != 1
            || !matches!(&r.variables[0],Variable::Dataset { path } if path=="/")
        {
            return fail("Zarr prototype accepts one array selected by dataset path /");
        }
        let root = Path::new(&asset.locator);
        let a: Array = serde_json::from_slice(&fs::read(root.join(".zarray"))?)?;
        if a.zarr_format != 2
            || a.shape.len() != 2
            || a.chunks.len() != 2
            || a.shape.contains(&0)
            || a.chunks.contains(&0)
            || a.order != "C"
        {
            return fail("Zarr planner supports nonempty 2D C-order v2 arrays only; shards/v3 require another adapter");
        }
        let sep = a.dimension_separator.unwrap_or_else(|| ".".into());
        if sep != "." && sep != "/" {
            return fail("invalid Zarr dimension separator");
        }
        let attrs: Attributes =
            serde_json::from_slice(&fs::read(root.join(".zattrs"))?).map_err(|_| {
                "Zarr bounds selection requires explicit tile_ripper_grid georeferencing in .zattrs"
            })?;
        let t = attrs.tile_ripper_grid.transform;
        if !t.iter().all(|x| x.is_finite()) || t[1] <= 0. || t[5] >= 0. || t[2] != 0. || t[4] != 0.
        {
            return fail("Zarr prototype requires finite north-up georeferencing");
        }
        let layout = Layout {
            width: a.shape[1],
            height: a.shape[0],
            transform: t,
            crs: attrs.tile_ripper_grid.crs,
            band: 0,
            block: [a.chunks[1], a.chunks[0]],
            dtype: a.dtype,
            nodata: None,
            scale: None,
            offset: None,
            tiles: vec![],
        };
        let Some(window) = cog::window(&layout, &r.spatial)? else {
            return Ok(());
        };
        p.selected.push(Selected {
            mask_geometry: None,
            asset_id: asset.id.clone(),
            datetime: asset.datetime.clone(),
            reason: "requested array and native bounds intersect".into(),
            capabilities: self.capabilities(),
            layout: Some(layout),
            window: window.clone(),
        });
        p.estimates.selected_compressed_payload_bytes = None;
        p.limitations.push(format!("Zarr planning only; missing chunks use fill_value at execution; compressor={}, filter_count={}", a.compressor.get("id").and_then(|v| v.as_str()).unwrap_or("none/unknown"), a.filters.as_array().map_or(0, |f| f.len())));
        let inspect = push(
            p,
            r,
            &asset.id,
            vec![],
            Operation::Inspect { cached: false },
        )?;
        for y in window.y / a.chunks[0]..=(window.y + window.height - 1) / a.chunks[0] {
            for x in window.x / a.chunks[1]..=(window.x + window.width - 1) / a.chunks[1] {
                push(
                    p,
                    r,
                    &asset.id,
                    vec![inspect],
                    Operation::Read {
                        read: Read::ChunkObject {
                            key: format!("{y}{sep}{x}"),
                            indices: vec![y, x],
                        },
                    },
                )?;
            }
        }
        Ok(())
    }
}
