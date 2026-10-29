# Current source handoff and fork maintenance

The candidate source contains three modified dependency crates. Both are required
path dependencies and must stay in an internal source snapshot. The former
non-vendored handoff procedure does not apply. The owner approved Apache-2.0 on 26 October 2026; [LICENSE](../../LICENSE) and
[NOTICE](../../NOTICE) apply to the project source while original dependency
licenses and notices remain intact.

## Source contents

Include the project source, tests, examples, scripts, documentation, Cargo
manifests/lockfiles and all complete tracked `vendor/` directories. Preserve their
original license texts, patch notes and the current notice evidence. A Git source
archive naturally excludes ignored build outputs, captured datasets, environments
and private credentials. Record its commit and SHA-256. Private recovery snapshots and acquired datasets stay outside the public repository.

Other Cargo dependencies are obtained separately from their pinned registry
versions; system GDAL is also separately installed. No compiled executable, GDAL
runtime, container or acquired provider dataset belongs in a source handoff.
Binary redistribution requires its own actual linked-component review. The
historical notice exceptions remain visible rather than being treated as cleared.

## Vendored fork record

The [consolidation audit](vendor/vendor-audit.json) and [OPERA expansion audit](../products/opera-expansion/dependency-audit/vendor-audit.json) record each distributed
file's hash, changed files, omitted upstream packaging files and exact crate
archive checksum. The patches describe all modifications to files supplied by
the corresponding published crate. Added notices/patch notes and every retained
lockfile are also listed. The source comparison has not widened HDF5 admission.

| Fork | Published base and local changes |
| --- | --- |
| `tiff-reader 0.9.0` | Published crate SHA-256 `5f5f0c80d1e2039b4cf5218b4fb55c93fbc1a466a1db893936407a00c3d59de2`; bounded one-shot Zstandard filter using the already pinned zstd dependency, fixed output and context reservation, no change to TIFF parsing/predictors. See the [new fork audit](../products/opera-expansion/dependency-audit/vendor-audit.json) and [patch note](../../vendor/tiff-reader/TILE_RIPPER_PATCH.md). |
| `hdf5-reader 0.9.3` | Published crate SHA-256 `c50e22fc5c53e8e23edd603050ac342ea4cde1898b64bf86266d60f6a9cc3963`; recorded upstream revision `3d708fd40c6809f5d215bd2d9f15e9a05de84bd0`. Changes affect legacy v1 traversal, canonical integer rejection, storage-fill/CF distinction and fill-write-time decoding. See [complete patch](vendor/hdf5-reader.patch) and [patch note](../../vendor/hdf5-reader/TILE_RIPPER_PATCH.md). |
| `zarrs_filesystem 0.3.13` | Published crate SHA-256 `a63fc5f551c941a98a5942814030c937ec979effb35e5edbf7f6334323aefd78`. The default-off macOS writer option retains per-write fsync while deferring full device flush to transactional publication. See [complete patch](vendor/zarrs_filesystem.patch) and [patch note](../../vendor/zarrs_filesystem/TILE-RIPPER-PATCH.md). |

Maintain these pinned forks locally for this candidate. Keep root and standalone
test locks, notices, full patches and regressions together. Rebase or remove a fork
only after the relevant upstream behavior is available and the same scientific,
failure and durability gates pass. Upstream proposals are a separate action; no
contact or submission is part of local consolidation.

Reproduce the fork and dependency evidence into new directories:

```sh
python3 scripts/vendor-audit.py --output prototype-output/vendor-audit-new
python3 scripts/license-inventory.py > prototype-output/license-inventory-new.json
python3 scripts/license-bundle.py --output prototype-output/license-bundle-new
```

The bundle includes registry and vendored packages, exact packaged notices,
historically recovered originals and the additional revision-pinned originals
recovered during consolidation. Metadata declarations, exact recovered texts and
unresolved omissions remain distinguishable. It is source evidence rather than
a grant of rights to Tile-Ripper or blanket binary clearance. See the
[licensing closeout](../alpha/licensing-closeout.md) for the current review result.
