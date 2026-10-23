#![cfg(feature = "native-hdf5")]
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use tile_ripper::{
    cog::LayoutCache,
    swot::{self, GridInspection},
    Pixels, Request, Result, Source, Spatial,
};
fn request() -> Request {
    serde_json::from_value(json!({"source":{"kind":"assets","assets":[{"id":"fixture","format":"hdf5","locator":format!("{}/tests/data/swot/grid-inspection.h5",env!("CARGO_MANIFEST_DIR"))}]},
        "spatial":{"kind":"array_slice","start":[1,2],"count":[5,7],"step":[2,3],"axes":["/y","/x"]},
        "variables":swot::PATHS.iter().map(|p|json!({"kind":"dataset","path":p})).collect::<Vec<_>>(),"grid":{"kind":"native"},"sink":{"kind":"callback"},"budget":{"buffer_bytes":8388608,"max_nodes":4096,"chunk_edge":7}})).unwrap()
}
fn values(p: swot::Prepared) -> Result<Value> {
    let mut stream = p.into_stream()?;
    let mut result = Vec::new();
    for tile in stream.by_ref() {
        let tile = tile?;
        let mut layers = Vec::new();
        for c in tile.layers {
            let bytes: Vec<u8> = match &c.pixels {
                Pixels::UInt8(v) => v.clone(),
                Pixels::UInt16(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
                Pixels::UInt32(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
                Pixels::Float32(v) => v.iter().flat_map(|v| v.to_bits().to_le_bytes()).collect(),
            };
            layers.push(json!({"bytes":bytes,"mask":c.validity().collect::<Vec<_>>(),"coordinates":c.coordinates}));
        }
        result.push(json!({"layers":layers,"quality":tile.validity}));
    }
    stream.finish()?;
    Ok(json!(result))
}
#[test]
fn immutable_grid_reuses_profiles_full_axes_and_indexes_across_concurrent_requests() -> Result<()> {
    let first = request();
    let grid = GridInspection::inspect(&first)?;
    let desc = grid.describe();
    assert_eq!(desc["grid"]["crs"], "EPSG:32619");
    assert_eq!(desc["retention"]["source_inspections"], 1);
    assert_eq!(desc["retention"]["metadata_parser_opens"], 1);
    assert_eq!(desc["retention"]["profile_discoveries"], 5);
    assert_eq!(desc["retention"]["full_index_scans"], 5);
    assert_eq!(desc["retention"]["coordinate_axis_reads"], 2);
    grid.with_grid(|_, y, x| {
        assert_eq!([y.len(), x.len()], [37, 53]);
        assert_eq!(grid.describe()["retention"]["metadata_parser_opens"], 1);
        assert!(grid.prepare(&first, swot::Policy::Raw).is_ok());
        Ok(())
    })?;
    let before = (
        desc["retention"]["metadata_range_misses"].clone(),
        desc["retention"]["retention_reservation_bytes"].clone(),
    );
    let mut second = first.clone();
    second.spatial = Spatial::ArraySlice {
        start: [12, 13],
        count: [4, 6],
        step: [1, 2],
        axes: ["/y".into(), "/x".into()],
    };
    std::thread::scope(|scope| -> Result<()> {
        let mut workers = Vec::new();
        for r in [&first, &second, &first, &second] {
            let copy = grid.clone();
            workers.push(scope.spawn(move || -> Result<()> {
                let actual = values(copy.prepare(r, swot::Policy::cookbook())?)?;
                let expected = values(swot::prepare(
                    r,
                    swot::Policy::cookbook(),
                    &mut LayoutCache::default(),
                )?)?;
                assert_eq!(actual, expected);
                Ok(())
            }));
        }
        for worker in workers {
            worker.join().unwrap()?;
        }
        Ok(())
    })?;
    let after = grid.describe();
    assert_eq!(
        before,
        (
            after["retention"]["metadata_range_misses"].clone(),
            after["retention"]["retention_reservation_bytes"].clone()
        )
    );
    assert_eq!(after["retention"]["coordinate_axis_reads"], 2);
    Ok(())
}
#[test]
fn incompatible_later_selection_and_metadata_budget_fail_explicitly() -> Result<()> {
    let r = request();
    let grid = GridInspection::inspect(&r)?;
    let mut small = r.clone();
    small.budget.buffer_bytes /= 2;
    assert!(grid
        .prepare(&small, swot::Policy::Raw)
        .err()
        .unwrap()
        .to_string()
        .contains("budget"));
    let mut unknown = r.clone();
    if let Spatial::ArraySlice { axes, .. } = &mut unknown.spatial {
        axes[1] = "/other_x".into();
    }
    assert!(grid.prepare(&unknown, swot::Policy::Raw).is_err());
    let mut outside = r.clone();
    if let Spatial::ArraySlice { start, .. } = &mut outside.spatial {
        start[1] = 10000;
    }
    assert!(grid.prepare(&outside, swot::Policy::Raw).is_err());
    let mut tiny = r;
    tiny.budget.buffer_bytes = 4096;
    assert!(GridInspection::inspect(&tiny).is_err());
    Ok(())
}
struct CopySource(PathBuf);
impl CopySource {
    fn new(r: &mut Request) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "grid-inspection-{}-{}.h5",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let Source::Assets { assets } = &mut r.source else {
            unreachable!()
        };
        fs::copy(&assets[0].locator, &p).unwrap();
        assets[0].locator = p.to_string_lossy().into_owned();
        Self(p)
    }
}
impl Drop for CopySource {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
#[test]
fn source_mutation_between_inspection_preparation_and_execution_is_rejected() -> Result<()> {
    let mut r = request();
    let copy = CopySource::new(&mut r);
    let grid = GridInspection::inspect(&r)?;
    let prepared = grid.prepare(&r, swot::Policy::Raw)?;
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(&copy.0)?
        .write_all(&[0])?;
    assert!(grid.prepare(&r, swot::Policy::Raw).is_err());
    assert!(values(prepared).is_err());
    Ok(())
}
#[test]
fn same_locator_different_versions_have_separate_grid_identities() -> Result<()> {
    let mut r = request();
    let copy = CopySource::new(&mut r);
    let a = GridInspection::inspect(&r)?;
    // Replacing the file preserves valid scientific content but changes the
    // immutable filesystem identity. A future inspection owns a fresh binding.
    let next = copy.0.with_extension("next.h5");
    fs::copy(
        format!(
            "{}/tests/data/swot/grid-inspection.h5",
            env!("CARGO_MANIFEST_DIR")
        ),
        &next,
    )?;
    fs::rename(next, &copy.0)?;
    let b = GridInspection::inspect(&r)?;
    assert_ne!(
        a.describe()["source_identity"],
        b.describe()["source_identity"]
    );
    assert_eq!(a.describe()["grid"], b.describe()["grid"]);
    assert!(a.prepare(&r, swot::Policy::Raw).is_err());
    assert!(b.prepare(&r, swot::Policy::Raw).is_ok());
    Ok(())
}

#[cfg(feature = "authenticated-http")]
#[test]
#[ignore = "run by bench/grid_inspection.py faults with its private HTTPS fixture"]
fn authenticated_changed_coordinates_etag_and_concurrent_preparations() -> Result<()> {
    use std::sync::Arc;
    use tile_ripper::authenticated_http::{AuthenticatedHttp, Config};
    let input: Value =
        serde_json::from_slice(&fs::read(std::env::var("TILE_RIPPER_GRID_FIXTURE")?)?)?;
    let session = || -> Result<Arc<AuthenticatedHttp>> {
        let mut config: Config = serde_json::from_value(input["policy"].clone())?;
        config
            .additional_roots
            .push(reqwest::Certificate::from_pem(&fs::read(
                input["ca"].as_str().unwrap(),
            )?)?);
        Ok(Arc::new(AuthenticatedHttp::new(
            config,
            input["netrc"].as_str().unwrap(),
        )?))
    };
    let mut r: Request = serde_json::from_value(input["request"].clone())?;
    let grid = GridInspection::inspect_authenticated(&r, session()?)?;
    let initial = grid.describe();
    std::thread::scope(|scope| -> Result<()> {
        let mut workers = Vec::new();
        for offset in 0..4 {
            let grid = grid.clone();
            let mut request = r.clone();
            if let Spatial::ArraySlice { start, .. } = &mut request.spatial {
                start[0] += offset;
            }
            workers.push(scope.spawn(move || values(grid.prepare(&request, swot::Policy::Raw)?)));
        }
        for worker in workers {
            assert!(worker.join().unwrap().is_ok());
        }
        Ok(())
    })?;
    let after = grid.describe();
    assert_eq!(
        initial["retention"]["metadata_range_misses"],
        after["retention"]["metadata_range_misses"]
    );
    assert_eq!(after["retention"]["metadata_parser_opens"], 1);
    let prepared = grid.prepare(&r, swot::Policy::Raw)?;
    let response = reqwest::blocking::get(format!(
        "{}/coordinates-version",
        input["control"].as_str().unwrap()
    ))?;
    assert!(response.status().is_success());
    assert!(grid.prepare(&r, swot::Policy::Raw).is_err());
    assert!(values(prepared).is_err());
    let Source::Assets { assets } = &mut r.source else {
        unreachable!()
    };
    assets[0].etag = Some("\"coordinate-version-two\"".into());
    let updated = GridInspection::inspect_authenticated(&r, session()?)?;
    assert_ne!(
        initial["source_identity"],
        updated.describe()["source_identity"]
    );
    assert_ne!(
        initial["grid"]["coordinate_sha256"],
        updated.describe()["grid"]["coordinate_sha256"]
    );
    assert!(values(updated.prepare(&r, swot::Policy::Raw)?).is_ok());
    assert!(
        grid.prepare(&r, swot::Policy::Raw).is_err(),
        "old conflict must remain sticky"
    );
    Ok(())
}

#[test]
fn invalid_geography_is_refused_before_source_access() -> Result<()> {
    let mut request = request();
    let Source::Assets { assets } = &mut request.source else {
        unreachable!()
    };
    assets[0].locator = "/nonexistent-grid-inspection-source.h5".into();
    let entry = tile_ripper::swot_job::Observation {
        id: "draft".into(),
        request,
        quality_policy: swot::Policy::Raw,
        catalog: None,
        geographic_selection: None,
    };
    let aoi = Spatial::Bounds {
        crs: "EPSG:3857".into(),
        bbox: [0., 0., 1., 1.],
    };
    let error = tile_ripper::swot_aoi::inspect(entry, &aoi)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("EPSG:4326"), "{error}");
    Ok(())
}
