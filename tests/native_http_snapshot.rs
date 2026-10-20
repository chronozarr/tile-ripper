#![cfg(feature = "native-cog")]

/// The bounded loopback STAC fixture changes its version on the second search.
/// Two prepared acquisitions must own separate immutable metadata snapshots.
#[test]
#[ignore = "requires TILE_RIPPER_NATIVE_HTTP_SNAPSHOT_REQUEST from the HTTP gate"]
fn prepared_metadata_is_private_version_bound_and_repeatable() {
    use tile_ripper::{acquisition, cog::LayoutCache, Request};
    let path = std::env::var("TILE_RIPPER_NATIVE_HTTP_SNAPSHOT_REQUEST").unwrap();
    let request: Request = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut cache = LayoutCache::default();
    let older = acquisition::prepare(&request, &mut cache).unwrap();
    let newer = acquisition::prepare(&request, &mut cache).unwrap();
    assert_ne!(
        older.explain()["sources"][0]["object_identity"]["etag"],
        newer.explain()["sources"][0]["object_identity"]["etag"]
    );
    let mut totals = Vec::new();
    for _ in 0..2 {
        let (mut sum, mut chunks) = (0u64, 0usize);
        let report = newer
            .execute(|chunk| {
                sum += chunk
                    .pixels
                    .as_u16()
                    .ok_or("expected UInt16")?
                    .iter()
                    .map(|v| u64::from(*v))
                    .sum::<u64>();
                chunks += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(chunks, report.chunks);
        assert!(chunks > 0);
        totals.push((sum, chunks));
    }
    assert_eq!(totals[0], totals[1]);
    let mut delivered = 0;
    let stale = older.execute(|_| {
        delivered += 1;
        Ok(())
    });
    assert!(stale.is_err());
    assert_eq!(delivered, 0);
}
