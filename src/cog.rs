use crate::{fail, model::*, Result};
use gdal::{Dataset, Metadata};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::Path};
const MAX_LAYOUT_TILES: usize = 65_536;
const MAX_CACHE_FILE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Identity {
    size: u64,
    modified_ns: u128,
}
#[derive(Serialize, Deserialize)]
struct Entry {
    adapter_version: u32,
    identity: Identity,
    band: usize,
    layout: Layout,
}
/// Local-only cache. No remote identity means no persistent reuse.
#[derive(Default)]
pub struct LayoutCache {
    entries: HashMap<String, Entry>,
}
impl LayoutCache {
    pub fn load(path: &Path) -> Result<Self> {
        if fs::metadata(path)?.len() > MAX_CACHE_FILE_BYTES {
            return fail("layout cache file exceeds 32 MiB metadata limit");
        }
        let entries: HashMap<String, Entry> = serde_json::from_slice(&fs::read(path)?)?;
        let mut total = 0usize;
        for e in entries.values() {
            total = total
                .checked_add(e.layout.tiles.len())
                .ok_or("layout cache size overflow")?;
            if e.layout.block.contains(&0)
                || e.layout.width == 0
                || e.layout.height == 0
                || !e.layout.transform.iter().all(|v| v.is_finite())
            {
                return fail("invalid cached layout");
            }
        }
        if total > MAX_LAYOUT_TILES {
            return fail("layout cache exceeds 65536 tile metadata entries");
        }
        Ok(Self { entries })
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        fs::write(path, serde_json::to_vec_pretty(&self.entries)?)?;
        Ok(())
    }
    pub fn resolve(&mut self, asset: &Asset, band: usize) -> Result<(Layout, bool)> {
        let identity = if remote(&asset.locator) {
            None
        } else {
            let meta = fs::metadata(&asset.locator).map_err(|_| "cannot stat local asset")?;
            Some(Identity {
                size: meta.len(),
                modified_ns: meta
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos(),
            })
        };
        let key = if identity.is_some() {
            Some(format!(
                "{}:{band}",
                fs::canonicalize(&asset.locator)?.display()
            ))
        } else {
            None
        };
        if let (Some(identity), Some(key)) = (&identity, &key) {
            if let Some(entry) = self.entries.get(key) {
                if entry.adapter_version == 1 && entry.identity == *identity && entry.band == band {
                    return Ok((entry.layout.clone(), true));
                }
            }
        }
        let layout = inspect(asset, band)?;
        if let (Some(identity), Some(key)) = (identity, key) {
            if self
                .entries
                .values()
                .map(|e| e.layout.tiles.len())
                .sum::<usize>()
                + layout.tiles.len()
                > MAX_LAYOUT_TILES
            {
                self.entries.clear();
            }
            self.entries.insert(
                key,
                Entry {
                    adapter_version: 1,
                    identity,
                    band,
                    layout: layout.clone(),
                },
            );
        }
        Ok((layout, false))
    }
}
pub fn remote(locator: &str) -> bool {
    locator.starts_with("https://") || locator.starts_with("http://")
}
pub fn open(asset: &Asset) -> Result<Dataset> {
    let locator = if remote(&asset.locator) {
        format!("/vsicurl/{}", asset.locator)
    } else {
        asset.locator.clone()
    };
    Dataset::open(Path::new(&locator))
        .map_err(|_| "GDAL could not open asset (locator omitted)".into())
}
pub fn inspect(asset: &Asset, band: usize) -> Result<Layout> {
    let ds = open(asset)?;
    if ds.driver().short_name() != "GTiff" {
        return fail("COG adapter requires GeoTIFF");
    }
    let b = ds.rasterband(band).map_err(|_| "invalid band")?;
    if b.band_type().name() != "UInt16" {
        return fail("prototype COG execution supports UInt16 only");
    }
    if !b.mask_flags()?.is_all_valid() && !b.mask_flags()?.is_nodata() {
        return fail(
            "explicit mask/alpha requires a validity-carrying adapter; unsupported in this slice",
        );
    }
    let transform = ds.geo_transform()?;
    if !transform.iter().all(|x| x.is_finite())
        || transform[2] != 0.
        || transform[4] != 0.
        || transform[1] <= 0.
        || transform[5] >= 0.
    {
        return fail("prototype requires a north-up grid with positive x and negative y spacing");
    }
    let srs = ds.spatial_ref()?;
    let crs = srs.authority().unwrap_or(ds.projection());
    if crs.is_empty() {
        return fail("missing CRS");
    }
    let (width, height) = ds.raster_size();
    let (bx, by) = b.block_size();
    if bx == 0
        || by == 0
        || width
            .div_ceil(bx)
            .checked_mul(height.div_ceil(by))
            .ok_or("layout tile count overflow")?
            > MAX_LAYOUT_TILES
    {
        return fail("layout exceeds prototype metadata budget of 65536 tiles");
    }
    let mut tiles = Vec::new();
    for y in 0..height.div_ceil(by) {
        for x in 0..width.div_ceil(bx) {
            let value = |prefix: &str| {
                b.metadata_item(&format!("{prefix}_{x}_{y}"), "TIFF")
                    .and_then(|s| s.parse().ok())
            };
            tiles.push(Tile {
                x,
                y,
                offset: value("BLOCK_OFFSET"),
                bytes: value("BLOCK_SIZE"),
            });
        }
    }
    Ok(Layout {
        width,
        height,
        transform,
        crs,
        band,
        block: [bx, by],
        dtype: b.band_type().name(),
        nodata: b.no_data_value(),
        scale: b.scale(),
        offset: b.offset(),
        tiles,
    })
}
/// Outer pixel envelope, clipped to source extent; half-open pixel bounds.
pub fn window(layout: &Layout, spatial: &Spatial) -> Result<Option<Window>> {
    let Spatial::Bounds {
        crs,
        bbox: [xmin, ymin, xmax, ymax],
    } = spatial
    else {
        return fail("polygon selection is not implemented");
    };
    if *crs != layout.crs {
        return fail(
            "bounds CRS must equal source authority string; reprojection is not implemented",
        );
    }
    let t = layout.transform;
    let x0 = ((xmin - t[0]) / t[1])
        .floor()
        .clamp(0., layout.width as f64) as usize;
    let x1 = ((xmax - t[0]) / t[1]).ceil().clamp(0., layout.width as f64) as usize;
    let y0 = ((ymax - t[3]) / t[5])
        .floor()
        .clamp(0., layout.height as f64) as usize;
    let y1 = ((ymin - t[3]) / t[5])
        .ceil()
        .clamp(0., layout.height as f64) as usize;
    Ok((x1 > x0 && y1 > y0).then_some(Window {
        x: x0,
        y: y0,
        width: x1.saturating_sub(x0),
        height: y1.saturating_sub(y0),
    }))
}
pub fn tiles(layout: &Layout, w: &Window) -> Vec<Tile> {
    layout
        .tiles
        .iter()
        .filter(|t| {
            t.x >= w.x / layout.block[0]
                && t.x <= (w.x + w.width - 1) / layout.block[0]
                && t.y >= w.y / layout.block[1]
                && t.y <= (w.y + w.height - 1) / layout.block[1]
        })
        .cloned()
        .collect()
}
