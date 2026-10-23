//! Frozen authenticated preparation benchmark. No catalog substitution is
//! admitted by production discovery; fixtures exercise the identical native
//! inspection/resolution/admission calls with catalog provenance omitted.
#[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
mod enabled {
    use serde_json::{json, Value};
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Instant,
    };
    use tile_ripper::{
        authenticated_http::{AuthenticatedHttp, Config},
        swot, swot_aoi,
        swot_job::Manifest,
        Asset, Result, Source, Spatial,
    };
    struct AllocationCounter;
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);
    static TOTAL: AtomicUsize = AtomicUsize::new(0);
    unsafe impl GlobalAlloc for AllocationCounter {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let p = System.alloc(layout);
            if !p.is_null() {
                let n = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
                PEAK.fetch_max(n, Ordering::Relaxed);
                TOTAL.fetch_add(layout.size(), Ordering::Relaxed);
            }
            p
        }
        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            System.dealloc(p, layout);
        }
    }
    #[global_allocator]
    static ALLOCATOR: AllocationCounter = AllocationCounter;
    fn allocations() -> Value {
        json!({"live_rust_allocation_bytes":LIVE.load(Ordering::Relaxed),"peak_rust_allocation_bytes":PEAK.load(Ordering::Relaxed),"cumulative_rust_allocation_bytes":TOTAL.load(Ordering::Relaxed),"scope":"System allocator calls from Rust; TLS/native libraries and allocator overhead excluded; RSS sampled separately"})
    }
    fn session(asset: &Asset, input: &Value) -> Result<Arc<AuthenticatedHttp>> {
        let policy = input["policies"]
            .as_array()
            .ok_or("missing policies")?
            .iter()
            .find(|p| p["canonical_url"] == asset.locator)
            .ok_or("missing exact policy")?;
        let mut config: Config = serde_json::from_value(policy.clone())?;
        config
            .additional_roots
            .push(reqwest::Certificate::from_pem(&std::fs::read(
                input["ca"].as_str().ok_or("missing CA")?,
            )?)?);
        Ok(Arc::new(AuthenticatedHttp::new(
            config,
            input["netrc"].as_str().ok_or("missing private netrc")?,
        )?))
    }
    fn generation(manifest: &mut Manifest, aois: &[Spatial], input: &Value) -> Result<Value> {
        let mut observations = Vec::new();
        let mut resolutions = Vec::new();
        for (entry, aoi) in manifest.observations.iter_mut().zip(aois) {
            let grid = swot::GridInspection::inspect_authenticated(
                &entry.request,
                session(entry.asset()?, input)?,
            )?;
            let inspected = grid.describe();
            let crs = inspected["grid"]["crs"]
                .as_str()
                .ok_or("missing inspected CRS")?;
            let (resolved, report) = grid
                .with_grid(|d, y, x| swot_aoi::resolve_grid(aoi, crs, y, x, &d["grid_mapping"]))?;
            let resolved = resolved.ok_or("benchmark requires positive native centers")?;
            entry.request.spatial = resolved.window.spatial();
            entry.geographic_selection = Some(resolved);
            let Source::Assets { assets } = &mut entry.request.source else {
                unreachable!()
            };
            assets[0].etag = inspected["source_identity"]["version"]["etag"]
                .as_str()
                .map(str::to_owned);
            let prepared = grid.prepare(&entry.request, entry.quality_policy.clone())?;
            observations.push(json!({"id":entry.id,"inspection":prepared.explain()}));
            resolutions
                .push(json!({"id":entry.id,"aoi_resolution":report,"grid_inspection":inspected}));
        }
        manifest.validate()?;
        Ok(json!({"observations":observations,"resolutions":resolutions}))
    }
    pub fn run() -> Result<()> {
        let path = std::env::args().nth(1).ok_or("input JSON required")?;
        let input: Value = serde_json::from_slice(&std::fs::read(path)?)?;
        let mut manifest: Manifest = serde_json::from_value(input["manifest"].clone())?;
        let aois: Vec<Spatial> = serde_json::from_value(input["aois"].clone())?;
        if manifest.observations.len() != aois.len() {
            return Err("AOI count mismatch".into());
        }
        let started = Instant::now();
        let before = allocations();
        let review = generation(&mut manifest, &aois, &input)?;
        let result = json!({"manifest":manifest,"review":review,"generation_seconds":started.elapsed().as_secs_f64(),"allocations_before":before,"allocations_after":allocations()});
        println!("{}", serde_json::to_string(&result)?);
        Ok(())
    }
}
fn main() -> tile_ripper::Result<()> {
    #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
    {
        enabled::run()
    }
    #[cfg(not(all(feature = "native-hdf5", feature = "authenticated-http")))]
    {
        Err("requires native-hdf5,authenticated-http".into())
    }
}
