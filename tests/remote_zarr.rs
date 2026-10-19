use tile_ripper::{acquisition, cog::LayoutCache, Request, Sink, Source};
#[test]
fn private_or_signed_http_group_urls_fail_without_network_or_secret_diagnostics() {
    let mut r: Request =
        serde_json::from_str(include_str!("../examples/unified-zarr.json")).unwrap();
    if let Source::Assets { assets } = &mut r.source {
        assets[0].locator =
            "https://user:do-not-leak@example.invalid/store.zarr?token=private".into();
    }
    let error = acquisition::prepare(&r, &mut LayoutCache::default())
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("public URL"));
    assert!(!error.contains("do-not-leak"));
    assert!(!error.contains("token=private"));
}

#[test]
#[ignore = "run by bench/remote_zarr.py against its controlled HTTP fixture"]
fn prepared_http_changes_and_cancellation() {
    let file = std::env::var("TILE_RIPPER_HTTP_REQUEST").unwrap();
    let mode = std::env::var("TILE_RIPPER_HTTP_MODE").unwrap();
    let r: Request = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    let prepared = acquisition::prepare(&r, &mut LayoutCache::default()).unwrap();
    if mode == "cancel" {
        let mut delivered = 0;
        let error = prepared
            .execute(|_| {
                delivered += 1;
                Err("consumer cancelled".into())
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("consumer cancelled"));
        assert_eq!(delivered, 1);
        return;
    }
    let Source::Assets { assets } = &r.source else {
        panic!("explicit fixture required")
    };
    let root = assets[0].locator.split("/store/").next().unwrap();
    reqwest::blocking::get(format!(
        "{root}/control/{}",
        if mode == "isolation" {
            "metadata"
        } else {
            &mode
        }
    ))
    .unwrap()
    .error_for_status()
    .unwrap();
    if mode == "isolation" {
        let newer = acquisition::prepare(&r, &mut LayoutCache::default()).unwrap();
        let report = newer.execute(|_| Ok(())).unwrap();
        assert_eq!(report.source_data_requests, Some(42));
    }
    let error = prepared.fetch().unwrap_err().to_string();
    assert!(
        error.contains("version conflict") || error.contains("appeared"),
        "{error}"
    );
    if let Sink::Directory { path } = &r.sink {
        assert!(!std::path::Path::new(path).exists());
    }
}
