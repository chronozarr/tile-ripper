# Current source handoff and fork maintenance

The candidate source contains two modified dependency crates. Both are required
path dependencies and must stay in an internal source snapshot. The former
non-vendored handoff procedure does not apply. Project ownership and the intended
project license still require the owner's decisions before external distribution.
No root project license has been installed.

## Source contents

Include the project source, tests, examples, scripts, documentation, Cargo
manifests/lockfiles and both complete tracked `vendor/` directories. Preserve their
original license texts, patch notes and the current notice evidence. A Git source
archive naturally excludes ignored build outputs, captured datasets, environments
and private credentials. Record its commit and SHA-256. An internal recovery
snapshot is not permission to publish it.

Other Cargo dependencies are obtained separately from their pinned registry
versions; system GDAL is also separately installed. No compiled executable, GDAL
runtime, container or acquired provider dataset belongs in a source handoff.
Binary redistribution requires its own actual linked-component review. The
historical notice exceptions remain visible rather than being treated as cleared.

## Vendored fork record

The [machine-readable audit](vendor/vendor-audit.json) records each distributed
file's hash, changed files, omitted upstream packaging files and exact crate
archive checksum. The patches describe all modifications to files supplied by
the corresponding published crate. Added notices/patch notes and every retained
lockfile are also listed. The source comparison has not widened HDF5 admission.

| Fork | Published base and local changes |
| --- | --- |
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
