//! HDF5 fixtures are independently created by bench/swot_quality.py fixtures.
#![cfg(feature = "native-hdf5")]
use tile_ripper::{
    cog::LayoutCache,
    swot::{self, Policy},
    *,
};
fn request(name: &str) -> Request {
    let path = format!("{}/tests/data/swot/{name}.h5", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_value(serde_json::json!({"source":{"kind":"assets","assets":[{"id":"source","locator":path,"format":"hdf5","datetime":null}]},"spatial":{"kind":"array_slice","start":[1,2],"count":[12,10],"step":[3,5],"axes":["/y","/x"]},"time":null,"variables":swot::PATHS.iter().map(|p|serde_json::json!({"kind":"dataset","path":p})).collect::<Vec<_>>(),"grid":{"kind":"native"},"sink":{"kind":"callback"},"budget":{"buffer_bytes":4194304,"max_nodes":4096,"chunk_edge":16}})).unwrap()
}
#[test]
fn differing_physical_shapes_align_native_unsigned_values_and_coordinates() -> Result<()> {
    for name in ["quality-le", "quality-be"] {
        let r = request(name);
        let p = swot::prepare(&r, Policy::cookbook(), &mut LayoutCache::default())?;
        let info = p.explain();
        assert_eq!(
            info["quality_policy"]["policy"]["accepted"],
            serde_json::json!(["good", "suspect"])
        );
        let mut stream = p.into_stream()?;
        let mut seen = std::collections::BTreeSet::new();
        let mut high_bit = false;
        for tile in stream.by_ref() {
            let t = tile?;
            let base = &t.layers[0];
            assert!(base.pixels.as_f32().is_some());
            assert!(t.layers[1].pixels.as_u8().is_some());
            assert!(t.layers[2].pixels.as_u32().is_some());
            assert!(t.layers[4].pixels.as_u32().is_some());
            for c in &t.layers {
                assert_eq!(c.window, base.window);
                assert_eq!(c.coordinates, base.coordinates);
            }
            for (i, flag) in t.layers[2].pixels.as_u32().unwrap().iter().enumerate() {
                high_bit |= *flag == 2147483648;
                if t.validity[i] != 0 {
                    assert!(t.layers[1].pixels.as_u8().unwrap()[i] <= 1);
                }
            }
            for row in 0..base.window.height {
                for col in 0..base.window.width {
                    let y = base.window.y + row * 3;
                    let x = base.window.x + col * 5;
                    assert_eq!(
                        base.pixel_center(col, row),
                        Some([200000. + 100. * x as f64, 4000000. + 100. * y as f64])
                    );
                    assert!(seen.insert((y, x)));
                }
            }
        }
        let report = stream.finish()?;
        assert_eq!(seen.len(), 120);
        assert!(high_bit);
        assert_eq!(report.decoded_bytes, 120 * 17);
        assert!(
            report.scheduler["reserved_bytes_peak"].as_u64().unwrap()
                <= r.budget.buffer_bytes as u64
        );
    }
    Ok(())
}
#[test]
fn policies_reject_unknown_contradictory_and_duplicate_conditions() {
    let r = request("quality-le");
    for policy in [
        Policy::Bitwise {
            require: vec!["not_a_flag".into()],
            exclude: vec![],
        },
        Policy::Bitwise {
            require: vec!["bright_land".into()],
            exclude: vec!["bright_land".into()],
        },
        Policy::Bitwise {
            require: vec![],
            exclude: vec!["bright_land".into(), "bright_land".into()],
        },
        Policy::Summary { accepted: vec![] },
    ] {
        assert!(swot::prepare(&r, policy, &mut LayoutCache::default()).is_err());
    }
    let mut duplicate = r.clone();
    duplicate.variables[1] = duplicate.variables[0].clone();
    assert!(swot::prepare(&duplicate, Policy::Raw, &mut LayoutCache::default()).is_err());
}
#[test]
fn cancellation_drop_and_repeated_execution_preserve_finish_contract() -> Result<()> {
    let p = swot::prepare(
        &request("quality-le"),
        Policy::cookbook(),
        &mut LayoutCache::default(),
    )?;
    let mut cancelled = p.clone().into_stream()?;
    cancelled.next().unwrap()?;
    cancelled.cancel();
    assert!(cancelled.finish().is_err());
    let mut dropped = p.clone().into_stream()?;
    dropped.next().unwrap()?;
    drop(dropped);
    for _ in 0..2 {
        assert_eq!(p.clone().into_stream()?.finish()?.decoded_bytes, 120 * 17);
    }
    Ok(())
}
#[test]
fn mutation_after_provisional_tile_prevents_completion() -> Result<()> {
    use std::{fs, io::Write};
    let dir = std::env::temp_dir().join(format!("tile-ripper-swot-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    let path = dir.join("changed.h5");
    fs::copy(
        format!(
            "{}/tests/data/swot/quality-le.h5",
            env!("CARGO_MANIFEST_DIR")
        ),
        &path,
    )?;
    let mut r = request("quality-le");
    if let Source::Assets { assets } = &mut r.source {
        assets[0].locator = path.to_string_lossy().into();
    }
    let p = swot::prepare(&r, Policy::cookbook(), &mut LayoutCache::default())?;
    let mut stream = p.into_stream()?;
    stream.next().unwrap()?;
    fs::OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(&[0])?;
    assert!(stream.finish().is_err());
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn unsigned_initialization_is_independent_of_scientific_missingness() -> Result<()> {
    for size in [1, 4] {
        for fill in ["None", "17"] {
            let mut r = request(&format!("initial-{size}-{fill}"));
            r.variables = vec![Variable::Dataset {
                path: "/signal".into(),
            }];
            r.spatial = Spatial::ArraySlice {
                start: [0, 0],
                count: [4, 4],
                step: [1, 1],
                axes: ["/y".into(), "/x".into()],
            };
            let mut s = acquisition::prepare(&r, &mut LayoutCache::default())?
                .into_stream(stream::StreamOptions { capacity: 0 })?;
            let mut count = 0;
            for chunk in s.by_ref() {
                let c = chunk?;
                assert!(c.nodata.is_none());
                assert!(c.validity().all(|v| v));
                let expected = if fill == "17" { 17 } else { 0 };
                if size == 1 {
                    assert!(c.pixels.as_u8().unwrap().iter().all(|v| *v == expected));
                } else {
                    assert!(c
                        .pixels
                        .as_u32()
                        .unwrap()
                        .iter()
                        .all(|v| *v == u32::from(expected)));
                }
                count += c.pixels.len();
            }
            assert_eq!(count, 16);
            assert_eq!(s.finish()?.filled_source_chunks, 4);
        }
    }
    Ok(())
}

#[test]
fn bitwise_requires_all_requested_bits_and_excludes_undefined_bits() -> Result<()> {
    let p = swot::prepare(
        &request("quality-le"),
        Policy::Raw,
        &mut LayoutCache::default(),
    )?;
    let mut stream = p.into_stream()?;
    let mut tile = stream.next().unwrap()?;
    let policy = Policy::Bitwise {
        require: vec!["bright_land".into(), "large_uncert_suspect".into()],
        exclude: vec![],
    };
    for (word, expected) in [(32u32, 0), (128, 0), (160, 1), (161, 0), (u32::MAX, 0)] {
        if let Pixels::UInt32(v) = &mut tile.layers[2].pixels {
            v[0] = word;
        }
        assert_eq!(policy.mask(&tile.layers)?[0], expected);
    }
    tile.layers[4].window.x += 1;
    assert!(policy.mask(&tile.layers).is_err());
    stream.finish()?;
    Ok(())
}
