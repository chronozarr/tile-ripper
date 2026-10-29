//! Immutable per-acquisition runtime choices.
//!
//! Library callers may prepare more than one acquisition concurrently. Reading
//! mutable process environment during their execution would let one request
//! change another request's backend or scheduler limits, so preparation
//! captures the relevant values and scoped workers inherit that snapshot.
use crate::{cog, model::*, Result};
use std::{cell::RefCell, marker::PhantomData, rc::Rc, sync::Arc};

#[derive(Clone)]
pub(crate) struct RuntimeConfig {
    backend: Option<cog::Backend>,
    workers: usize,
    per_host: usize,
    gdal_thread_configuration: Vec<(&'static str, String)>,
    zarr_ranges: Option<crate::zarr_ranges::Config>,
}

impl RuntimeConfig {
    pub(crate) fn capture(request: &Request) -> Result<Arc<Self>> {
        let has_cog = matches!(
            &request.source,
            Source::Assets { assets } if assets.iter().any(|asset| asset.format == Format::Cog)
        ) || matches!(
            &request.source,
            Source::Stac { .. } | Source::Catalog { .. }
        );
        let has_zarr_v3 = matches!(
            &request.source,
            Source::Assets { assets } if assets.iter().any(|asset| asset.format.is_v3())
        );
        // Nested preparation inherits choices already captured by its caller.
        // A new source kind may additionally need a backend/range policy that
        // the outer request never used (e.g. a COG prepared in a Zarr callback).
        if let Some(config) = active() {
            let needs_backend = has_cog && config.backend.is_none();
            let needs_ranges = has_zarr_v3 && config.zarr_ranges.is_none();
            if !needs_backend && !needs_ranges {
                return Ok(config);
            }
            let mut expanded = (*config).clone();
            if needs_backend {
                expanded.backend = Some(cog::backend_from_environment()?);
            }
            if needs_ranges {
                expanded.zarr_ranges = Some(crate::zarr_ranges::Config::from_env(1)?);
            }
            return Ok(Arc::new(expanded));
        }
        Ok(Arc::new(Self {
            backend: has_cog.then(cog::backend_from_environment).transpose()?,
            workers: configured("TILE_RIPPER_WORKERS", default_workers())?,
            per_host: configured("TILE_RIPPER_PER_HOST", 4)?,
            gdal_thread_configuration: cog::capture_thread_configuration()?,
            zarr_ranges: has_zarr_v3
                .then(|| crate::zarr_ranges::Config::from_env(1))
                .transpose()?,
        }))
    }

    /// STAC request limits may install caller-local GDAL options after an
    /// outer acquisition has already pinned backend and scheduler choices.
    /// Refresh only that GDAL snapshot; retain the outer immutable choices.
    pub(crate) fn with_current_gdal(self: &Arc<Self>) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            backend: self.backend,
            workers: self.workers,
            per_host: self.per_host,
            gdal_thread_configuration: cog::capture_thread_configuration()?,
            zarr_ranges: self.zarr_ranges.clone(),
        }))
    }

    pub(crate) fn enter(self: &Arc<Self>) -> Scope {
        ACTIVE.with(|active| active.borrow_mut().push(self.clone()));
        Scope(PhantomData)
    }

    pub(crate) fn backend(&self) -> Option<cog::Backend> {
        self.backend
    }

    pub(crate) fn workers(&self) -> usize {
        self.workers
    }

    pub(crate) fn per_host(&self) -> usize {
        self.per_host
    }

    pub(crate) fn gdal_thread_configuration(&self) -> Vec<(&'static str, String)> {
        self.gdal_thread_configuration.clone()
    }

    pub(crate) fn zarr_ranges(&self, concurrency: usize) -> Result<crate::zarr_ranges::Config> {
        self.zarr_ranges
            .as_ref()
            .map(|config| config.with_concurrency(concurrency))
            .ok_or_else(|| {
                "Zarr v3 range configuration was not captured for this acquisition".into()
            })
    }
}

thread_local! {
    static ACTIVE: RefCell<Vec<Arc<RuntimeConfig>>> = const { RefCell::new(Vec::new()) };
}

// The scope owns a thread-local stack entry and must be dropped by that same
// thread. `Rc` makes this invariant visible to the type system.
pub(crate) struct Scope(PhantomData<Rc<()>>);

impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            active
                .borrow_mut()
                .pop()
                .expect("runtime configuration scope stack underflow");
        });
    }
}

pub(crate) fn active() -> Option<Arc<RuntimeConfig>> {
    ACTIVE.with(|active| active.borrow().last().cloned())
}

pub(crate) fn worker_count() -> Result<usize> {
    if let Some(config) = active() {
        return Ok(config.workers().min(config.per_host()));
    }
    Ok(configured("TILE_RIPPER_WORKERS", default_workers())?
        .min(configured("TILE_RIPPER_PER_HOST", 4)?))
}

pub(crate) fn workers() -> Result<usize> {
    active().map_or_else(
        || configured("TILE_RIPPER_WORKERS", default_workers()),
        |config| Ok(config.workers()),
    )
}

pub(crate) fn per_host() -> Result<usize> {
    active().map_or_else(
        || configured("TILE_RIPPER_PER_HOST", 4),
        |config| Ok(config.per_host()),
    )
}

pub(crate) fn zarr_ranges(concurrency: usize) -> Result<crate::zarr_ranges::Config> {
    active().map_or_else(
        || crate::zarr_ranges::Config::from_env(concurrency),
        |config| config.zarr_ranges(concurrency),
    )
}

fn default_workers() -> usize {
    std::thread::available_parallelism().map_or(1, |count| count.get().min(2))
}

fn configured(name: &str, default: usize) -> Result<usize> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|count| (1..=64).contains(count))
            .ok_or_else(|| format!("{name} must be an integer from 1 to 64").into()),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(_) => Err(format!("invalid {name}").into()),
    }
}

#[cfg(all(test, feature = "gdal", feature = "native-cog"))]
mod tests {
    use super::*;

    fn request() -> Request {
        Request {
            source: Source::Assets {
                assets: vec![Asset {
                    etag: None,
                    id: "fixture".into(),
                    locator: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/data/native_cog/hls_int16.tif")
                        .to_string_lossy()
                        .into(),
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
            sink: Sink::Callback,
            budget: Budget {
                buffer_bytes: 16 * 1024 * 1024,
                max_nodes: 4096,
                chunk_edge: 64,
            },
        }
    }

    #[test]
    fn prepared_runtime_ignores_later_backend_and_scheduler_environment_changes() {
        const CHILD: &str = "TILE_RIPPER_RUNTIME_CONFIG_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "runtime_config::tests::prepared_runtime_ignores_later_backend_and_scheduler_environment_changes",
                    "--exact",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }

        // This subprocess owns its process environment, avoiding races with
        // other tests while demonstrating the post-preparation mutation that
        // prepared executions must ignore.
        unsafe {
            std::env::set_var("TILE_RIPPER_COG_BACKEND", "native");
            std::env::set_var("TILE_RIPPER_WORKERS", "1");
            std::env::set_var("TILE_RIPPER_PER_HOST", "1");
        }
        let mut request = request();
        let Source::Assets { assets } = &mut request.source else {
            unreachable!()
        };
        let mut second = assets[0].clone();
        second.id = "second".into();
        assets.push(second);
        let prepared =
            crate::acquisition::prepare(&request, &mut crate::cog::LayoutCache::default()).unwrap();
        unsafe {
            std::env::set_var("TILE_RIPPER_COG_BACKEND", "gdal");
            std::env::set_var("TILE_RIPPER_WORKERS", "2");
            std::env::set_var("TILE_RIPPER_PER_HOST", "2");
        }
        let report = prepared
            .execute(|chunk| {
                let values = chunk
                    .pixels
                    .as_i16()
                    .ok_or("expected retained native Int16")?;
                assert_eq!(values[0], -9999);
                assert_eq!(values[1], -11987);
                Ok(())
            })
            .unwrap();
        assert_eq!(report.scheduler["workers"], 1);
        assert_eq!(report.scheduler["per_host"], 1);
        assert_eq!(report.chunks, 2);
        assert_eq!(
            prepared.plan().selected[0].capabilities.decoding,
            "tiff-reader 0.9 storage-domain UInt8/UInt16/Int16/Float32"
        );

        // Stream starts its producer on a new thread; `execute_cancellable`
        // must enter the prepared scope there as well.
        let mut stream = prepared.into_stream(Default::default()).unwrap();
        let chunk = stream.next().unwrap().unwrap();
        assert_eq!(chunk.pixels.as_i16().unwrap()[1], -11987);
        drop(chunk);
        let streamed = stream.finish().unwrap();
        assert_eq!(streamed.scheduler["workers"], 1);
        assert_eq!(streamed.scheduler["per_host"], 1);
        assert_eq!(streamed.chunks, 2);

        unsafe {
            std::env::set_var("TILE_RIPPER_COG_BACKEND", "native");
        }
        let mut outer_request = request.clone();
        let Source::Assets { assets } = &mut outer_request.source else {
            unreachable!()
        };
        for asset in assets {
            asset.format = Format::ZarrV2;
        }
        let outer = RuntimeConfig::capture(&outer_request).unwrap();
        let _outer = outer.enter();
        let nested =
            crate::acquisition::prepare(&request, &mut crate::cog::LayoutCache::default()).unwrap();
        nested
            .execute(|chunk| {
                assert_eq!(chunk.pixels.as_i16().unwrap()[1], -11987);
                Ok(())
            })
            .unwrap();
    }
}
