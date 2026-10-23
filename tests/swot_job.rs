#![cfg(feature = "native-hdf5")]
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tile_ripper::{
    swot,
    swot_job::{Job, Manifest},
    Result,
};

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "tile-ripper-job-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn output(&self) -> PathBuf {
        self.0.join("result.zarr")
    }
    fn stages(&self) -> Vec<PathBuf> {
        fs::read_dir(&self.0)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("partial-"))
            .map(|e| e.path())
            .collect()
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn manifest(n: usize) -> Manifest {
    let observations:Vec<_>=["quality-le","quality-be"].iter().take(n).enumerate().map(|(i,name)|json!({"id":format!("observation_{i}"),"quality_policy":swot::Policy::cookbook(),"request":{
        "source":{"kind":"assets","assets":[{"id":"source","locator":format!("{}/tests/data/swot/{name}.h5",env!("CARGO_MANIFEST_DIR")),"format":"hdf5","datetime":if i==0 {"2024-05-01T00:00:00Z"} else {"2024-01-01T00:00:00Z"}}]},
        "spatial":{"kind":"array_slice","start":[1,2],"count":[12-i,10-i],"step":[3,5],"axes":["/y","/x"]},"time":null,
        "variables":swot::PATHS.iter().map(|p|json!({"kind":"dataset","path":p})).collect::<Vec<_>>(),"grid":{"kind":"native"},"sink":{"kind":"callback"},
        "budget":{"buffer_bytes":4194304,"max_nodes":4096,"chunk_edge":16}}})).collect();
    serde_json::from_value(json!({"job_schema":1,"profile":"swot_raster_d_pgd0","observations":observations,"limits":{"memory_bytes":67108864,"output_chunk_edge":8,"max_output_objects":10000}})).unwrap()
}
fn read(path: PathBuf) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
#[test]
fn one_and_multiple_observations_preserve_manifest_order_types_and_masks() -> Result<()> {
    for n in [1, 2] {
        let root = Root::new();
        let m = manifest(n);
        let plan = Job::new(&m)?.plan()?;
        assert_eq!(plan["observations"][0]["id"], "observation_0");
        let report = Job::new(&m)?.fetch(root.output(), Arc::default())?;
        assert_eq!(report["status"], "complete");
        assert!(
            report["observed"]["working_reservation_peak_bytes"]
                .as_u64()
                .unwrap()
                <= m.limits.memory_bytes as u64
        );
        assert_eq!(report["observations"].as_array().unwrap().len(), n);
        for i in 0..n {
            let group = root.output().join(format!("observations/observation_{i}"));
            for (name, dtype) in [
                "wse",
                "wse_qual",
                "wse_qual_bitwise",
                "wse_uncert",
                "n_wse_pix",
            ]
            .iter()
            .zip(["<f4", "|u1", "<u4", "<f4", "<u4"])
            {
                let a = read(group.join(name).join(".zarray"));
                assert_eq!(a["dtype"], dtype);
                assert_eq!(a["shape"], json!([12 - i, 10 - i]));
                assert!(group.join(format!("source_valid_{name}/.zarray")).exists());
            }
            assert_eq!(
                read(group.join("quality_mask/.zattrs"))["mask_role"],
                "derived quality policy; not source validity or an AOI mask"
            );
            assert!(!group.join("aoi_mask").exists());
            assert_eq!(
                report["observations"][i]["job_final_validation"]["status"],
                "validated"
            );
        }
        assert!(root.stages().is_empty());
    }
    Ok(())
}
#[test]
fn contract_rejects_duplicate_sources_ids_empty_unsupported_policies_and_budgets() -> Result<()> {
    let original = manifest(2);
    for mutate in 0..9 {
        let mut m = original.clone();
        match mutate {
            0 => m.observations[1].id = m.observations[0].id.clone(),
            1 => m.observations[1].request.source = m.observations[0].request.source.clone(),
            2 => m.limits.memory_bytes = 32 * 1024 * 1024,
            3 => m.limits.max_output_objects = 8,
            4 => {
                m.observations[0].request.spatial = serde_json::from_value(
                    json!({"kind":"array_slice","start":[0,0],"count":[0,1],"step":[1,1],"axes":["/y","/x"]}),
                )?
            }
            5 => m.observations[0].quality_policy = swot::Policy::Summary { accepted: vec![] },
            6 => m.observations[0]
                .request
                .variables
                .pop()
                .map(|_| ())
                .unwrap(),
            7 => m.observations[0].id = "../escape".into(),
            _ => {
                m.observations[0].request.grid = tile_ripper::Grid::Target {
                    crs: "EPSG:4326".into(),
                    resolution: [1., 1.],
                    resampling: "nearest".into(),
                }
            }
        }
        assert!(Job::new(&m).is_err(), "case {mutate}");
    }
    let mut m = manifest(1);
    if let tile_ripper::Source::Assets { assets } = &mut m.observations[0].request.source {
        assets[0].locator = "https://archive.example/object?Signature=SECRET_MARKER".into();
    }
    let error = m.validate().unwrap_err().to_string();
    assert!(!error.contains("SECRET_MARKER"));
    Ok(())
}
#[test]
fn four_distinct_sources_with_equal_timestamps_stay_four_observations() -> Result<()> {
    let root = Root::new();
    let mut m = manifest(1);
    let entry = m.observations[0].clone();
    for i in 1..4 {
        let mut e = entry.clone();
        e.id = format!("observation_{i}");
        m.observations.push(e);
    }
    for (i, e) in m.observations.iter_mut().enumerate() {
        let path = root.0.join(format!("source-{i}.h5"));
        fs::copy(e.asset()?.locator.clone(), &path)?;
        if let tile_ripper::Source::Assets { assets } = &mut e.request.source {
            assets[0].locator = path.to_string_lossy().into_owned();
        }
    }
    let report = Job::new(&m)?.fetch(root.output(), Arc::default())?;
    assert_eq!(report["observations"].as_array().unwrap().len(), 4);
    m.observations.push(entry);
    assert!(Job::new(&m).is_err());
    Ok(())
}
#[test]
fn later_source_failure_never_publishes_an_earlier_observation() -> Result<()> {
    let root = Root::new();
    let mut m = manifest(2);
    if let tile_ripper::Spatial::ArraySlice { start, .. } = &mut m.observations[1].request.spatial {
        start[0] = 99999;
    }
    let error = Job::new(&m)?
        .fetch(root.output(), Arc::default())
        .unwrap_err();
    let failure = error
        .downcast_ref::<tile_ripper::swot_job::Failure>()
        .unwrap();
    assert_eq!(failure.receipt["status"], "failed");
    assert_eq!(
        failure.receipt["provisional_source_receipts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(!root.output().exists());
    assert!(root.stages().is_empty());
    Ok(())
}
fn observe_stage(
    root: PathBuf,
    relative: &str,
    action: impl FnOnce() + Send + 'static,
) -> std::thread::JoinHandle<()> {
    let relative = relative.to_owned();
    std::thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            for e in fs::read_dir(&root).unwrap().flatten() {
                if e.file_name().to_string_lossy().contains("partial-")
                    && e.path().join(&relative).exists()
                {
                    action();
                    return;
                }
            }
            assert!(Instant::now() < until, "writer stage not observed");
            std::thread::sleep(Duration::from_micros(100));
        }
    })
}
#[test]
fn cancellation_during_writing_and_publication_races_preserve_destination() -> Result<()> {
    for race in [false, true] {
        let root = Root::new();
        let m = manifest(1);
        let flag = Arc::new(AtomicBool::new(false));
        let other = flag.clone();
        let destination = root.output();
        let thread = observe_stage(
            root.0.clone(),
            "observations/observation_0/wse/.zarray",
            move || {
                if race {
                    fs::create_dir(&destination).unwrap();
                    fs::write(destination.join("unrelated"), b"preserve").unwrap();
                } else {
                    other.store(true, Ordering::Release);
                }
            },
        );
        let result = Job::new(&m)?.fetch(root.output(), flag);
        thread.join().unwrap();
        assert!(result.is_err());
        assert!(root.stages().is_empty());
        if race {
            assert_eq!(fs::read(root.output().join("unrelated"))?, b"preserve");
        } else {
            assert!(!root.output().exists());
        }
    }
    Ok(())
}
#[test]
fn previously_read_source_mutating_during_second_observation_fails_final_sweep() -> Result<()> {
    let root = Root::new();
    let mut m = manifest(2);
    let first = root.0.join("first.h5");
    fs::copy(m.observations[0].asset()?.locator.clone(), &first)?;
    if let tile_ripper::Source::Assets { assets } = &mut m.observations[0].request.source {
        assets[0].locator = first.to_string_lossy().into_owned();
    }
    let thread = observe_stage(
        root.0.clone(),
        "observations/observation_1/wse/.zarray",
        move || {
            use std::io::Write;
            fs::OpenOptions::new()
                .write(true)
                .open(first)
                .unwrap()
                .write_all(b"changed!")
                .unwrap();
        },
    );
    let result = Job::new(&m)?.fetch(root.output(), Arc::default());
    thread.join().unwrap();
    let error = result.unwrap_err().to_string();
    assert!(error.contains("source identity changed"), "{error}");
    assert!(!root.output().exists());
    assert!(root.stages().is_empty());
    Ok(())
}
#[test]
fn existing_destination_and_precancelled_job_do_not_create_staging() -> Result<()> {
    let root = Root::new();
    let m = manifest(1);
    fs::create_dir(root.output())?;
    assert!(Job::new(&m)?.fetch(root.output(), Arc::default()).is_err());
    fs::remove_dir(root.output())?;
    assert!(Job::new(&m)?
        .fetch(root.output(), Arc::new(AtomicBool::new(true)))
        .is_err());
    assert!(root.stages().is_empty());
    Ok(())
}
