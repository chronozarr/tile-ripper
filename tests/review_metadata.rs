use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
use tile_ripper::{acquisition, cog::LayoutCache, plan, Request};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "tile-ripper-metadata-review-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn request(&self, variable: &str) -> Request {
        serde_json::from_value(serde_json::json!({
            "source":{"kind":"assets","assets":[{
                "id":"source", "locator":self.0, "format":"zarr_v2", "datetime":null
            }]},
            "spatial":{"kind":"bounds","crs":"EPSG:32617","bbox":[0,0,2,2]},
            "time":null, "variables":[{"kind":"dataset","path":variable}],
            "grid":{"kind":"native"},
            "sink":{"kind":"directory","path":self.0.join("out")},
            "budget":{"buffer_bytes":1048576,"max_nodes":100,"chunk_edge":2}
        }))
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn oversized_json(path: &Path) {
    let mut bytes = b"{\"padding\":\"".to_vec();
    bytes.resize(1024 * 1024 + 1, b'x');
    bytes.extend_from_slice(b"\"}");
    fs::write(path, bytes).unwrap();
}

#[test]
fn preparation_caps_root_attributes_before_snapshot_inspection() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join(".zgroup"), r#"{"zarr_format":2}"#).unwrap();
    oversized_json(&fixture.0.join(".zattrs"));
    let error = acquisition::prepare(&fixture.request("value"), &mut LayoutCache::default())
        .err()
        .expect("oversized attributes must fail preparation")
        .to_string();
    assert!(error.contains("metadata exceeds 1 MiB"), "{error}");
    assert!(!fixture.0.join("out").exists());
}

#[test]
fn legacy_root_array_planning_cannot_bypass_the_metadata_limit() {
    let fixture = Fixture::new();
    oversized_json(&fixture.0.join(".zarray"));
    let error = plan(&fixture.request("/"), &mut LayoutCache::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("metadata exceeds 1 MiB"), "{error}");
    assert!(!fixture.0.join("out").exists());
}

#[test]
fn preparation_caps_v3_group_metadata_too() {
    let fixture = Fixture::new();
    oversized_json(&fixture.0.join("zarr.json"));
    let mut request = fixture.request("value");
    let tile_ripper::Source::Assets { assets } = &mut request.source else {
        unreachable!()
    };
    assets[0].format = tile_ripper::Format::ZarrV3;
    let error = acquisition::prepare(&request, &mut LayoutCache::default())
        .err()
        .expect("oversized v3 metadata must fail preparation")
        .to_string();
    assert!(error.contains("metadata exceeds 1 MiB"), "{error}");
}
