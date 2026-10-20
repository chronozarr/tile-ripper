use crate::{fail, model::*, Result};
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
        let mut saved: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&fs::read(path)?)?;
        saved.retain(|_, e| e["adapter_version"] == 2 || e["adapter_version"] == 3);
        let entries: HashMap<String, Entry> = serde_json::from_value(saved.into())?;
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
                if entry.adapter_version == adapter_version()?
                    && entry.identity == *identity
                    && entry.band == band
                {
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
                    adapter_version: adapter_version()?,
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
pub use crate::spatial::window;
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

#[cfg(feature = "gdal")]
#[path = "cog_gdal.rs"]
mod gdal_impl;
#[cfg(feature = "gdal")]
pub use gdal_impl::open;
#[cfg(feature = "gdal")]
pub(crate) use gdal_impl::{
    apply_thread_configuration, clear_remote_cache, thread_configuration, ConditionalHeaders,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    #[cfg(feature = "gdal")]
    Gdal,
    #[cfg(feature = "native-cog")]
    Native,
}
/// Resolve the reader selected by `TILE_RIPPER_COG_BACKEND`; GDAL is the
/// default when compiled. Keep that environment selection fixed from request
/// preparation until all related `Prepared` values and streams are dropped.
pub fn backend() -> Result<Backend> {
    let default = if cfg!(feature = "gdal") {
        "gdal"
    } else {
        "native"
    };
    match std::env::var("TILE_RIPPER_COG_BACKEND")
        .as_deref()
        .unwrap_or(default)
    {
        #[cfg(feature = "gdal")]
        "gdal" => Ok(Backend::Gdal),
        #[cfg(feature = "native-cog")]
        "native" => Ok(Backend::Native),
        _ => fail("COG backend unavailable; enable gdal or native-cog and select gdal/native"),
    }
}
/// Require Tile-Ripper to be compiled without its `gdal` feature. This guard
/// permits feature-free Zarr and does not change backend state. It cannot detect
/// GDAL linked independently by a host application; inspect the final executable
/// for a whole-process linkage guarantee.
pub fn require_gdal_free() -> Result<()> {
    if cfg!(feature = "gdal") {
        return fail("GDAL-free operation requires a build without the gdal feature; install with --no-default-features --features native-cog for COG/Sentinel-2");
    }
    Ok(())
}
#[allow(unused_variables)]
pub fn inspect(asset: &Asset, band: usize) -> Result<Layout> {
    match backend()? {
        #[cfg(feature = "gdal")]
        Backend::Gdal => gdal_impl::inspect(asset, band),
        #[cfg(feature = "native-cog")]
        Backend::Native => crate::native_cog::inspect(asset, band),
    }
}
#[cfg(not(feature = "gdal"))]
pub(crate) fn thread_configuration() -> Result<Vec<(&'static str, String)>> {
    Ok(vec![])
}
#[cfg(not(feature = "gdal"))]
pub(crate) fn apply_thread_configuration(_: &[(&str, String)]) -> Result<()> {
    Ok(())
}
#[cfg(not(feature = "gdal"))]
pub(crate) fn clear_remote_cache(_: &Asset) -> Result<()> {
    Ok(())
}

fn adapter_version() -> Result<u32> {
    match backend()? {
        #[cfg(feature = "gdal")]
        Backend::Gdal => Ok(2),
        #[cfg(feature = "native-cog")]
        Backend::Native => Ok(3),
    }
}

pub(crate) fn native_selected() -> bool {
    #[cfg(feature = "native-cog")]
    {
        backend().is_ok_and(|b| b == Backend::Native)
    }
    #[cfg(not(feature = "native-cog"))]
    {
        false
    }
}
