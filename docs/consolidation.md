# Tile-Ripper consolidation status

Local consolidation completed on 26 October 2026. The qualified implementation
through `a5e18c2` is integrated into local `main`, with current candidate
instructions, repeatable checks and recovery records. No runtime source or
production dependency version changed. The candidate remains unreleased;
Apache-2.0 licensing and a public source remote were subsequently approved;
the human usability trial remains open.

## Completed locally

| Checklist item | Result |
| --- | --- |
| Current candidate contract | [ALPHA](../ALPHA.md), [profile contract](alpha/contract.md), [installation](installation.md), [scientist trial](alpha/human-trial.md) and feedback now describe the current native candidate and its qualified profiles. Cargo defaults remain GDAL; the complete native build is the trial's primary path. |
| Source handoff and notices | [Current source handoff](consolidation/source-handoff.md) includes both required vendored forks. The inventory covers 426 third-party packages; the bundle contains 299 unique original texts. Nine additional packages' notices were recovered, while six exact-notice gaps remain explicit. Apache-2.0 is now approved and applied; original dependency notices are preserved. |
| Integration | Local `main` was advanced by fast-forward from `ada67bb`; both checkouts have clean Git status. The three pre-existing experiment ignore files were preserved in recovery and committed unchanged. The subsequently approved public remote is `chronozarr/tile-ripper`; no package/binary release tag was created. |
| Repeatable validation | [One local entrypoint](consolidation/validation.md) runs the locked six-build matrix, both fork suites, standalone consumer, Clippy, formatting, Python syntax and documentation checks. Separate replay and live/source-trial commands use fresh result directories. |
| Minimum Rust and platform checks | Rust 1.91 and 1.92 both pass every host matrix build and fork suite. A fresh Linux ARM64/1.92 container passes 187 native tests under UID 1000 without GDAL/libhdf5 linkage. |
| Independent science and lifecycle replay | 19 Wave 3 science cases, 18 Wave 4 cases, 61 fault controls, all 1,229,401,152 original NISAR centers and all four million northern-SMAP centers pass in a separate sandbox. Original reports, examples and capture manifests are unchanged. |
| Clean source installation | A tracked-source archive at `bbc6cf1` built from an empty target, installed to an isolated prefix, fetched public ERA5/GOES, reopened output with xarray and uninstalled successfully. The native executable has no GDAL/libhdf5 linkage. This is automated evidence; no human usability result is claimed. |
| Fork maintenance | Full patches reapply to checksum-pinned published crate archives and reproduce every modified file hash. Keep both pinned forks locally for this candidate; rebase/remove only after equivalent upstream behavior passes the same gates. |
| Recovery and cleanup | A verified Git bundle, internal source archive and compact backup of nine scientific mirrors preserve the current work. Sixteen regenerable Cargo build directories containing about 67.1 GiB of allocated artifacts were removed; scientific captures, environments, outputs and logs were retained. All nine mirrors' captured extent hashes still match. |

## Verification results

The same results passed under Rust 1.91.0 and 1.92.0 on macOS ARM64:

| Test build | Passed | Ignored |
| --- | ---: | ---: |
| Default GDAL | 132 | 2 |
| Zarr-only | 94 | 1 |
| Native COG | 126 | 3 |
| Native HDF5 | 135 | 2 |
| Complete native | 187 | 8 |
| All features | 216 | 9 |
| Standalone HDF5 fork | 356 | 0 |
| Standalone filesystem fork | 6 | 0 |

Every build has zero failures. Both consumer configurations, Clippy and formatting
pass. Ignored tests retain their explicit helper/external-fixture requirements;
the replay is separate from protected-provider authentication. No protected live
acquisition was repeated during consolidation. Existing archive qualifications
remain scoped to their exact source/version/policy receipts.

[Acceptance index](consolidation/acceptance.json),
[Rust 1.91](consolidation/checks/rust-1.91.json),
[Rust 1.92](consolidation/checks/rust-1.92.json),
[Linux](consolidation/checks/linux-arm64.json),
[science replay](consolidation/checks/science-replay.json),
[clean source trial](consolidation/checks/source-trial.json) and
[cleanup record](consolidation/checks/cleanup.json) preserve compact results and log hashes.
Full logs and new outputs remain under the development worktree's
`prototype-output/consolidation/`; the Linux log is copied into the recovery snapshot.

## Source publication and remaining human trial

The owner approved Apache-2.0 on 26 October 2026. The root LICENSE/NOTICE and
Cargo metadata are active. Source is published at
[chronozarr/tile-ripper](https://github.com/chronozarr/tile-ripper), with docs at
[tileripper.com](https://tileripper.com). The old personal Tile-Ripper URL is a
redirect to ChronoZarr and was preserved. No force-push of ChronoZarr occurred.

The Git remote provides an off-machine copy of tracked source/history. Ignored
scientific captures remain in the local recovery copies described below. Choose
the first human tester and feedback channel; the trial and template are ready,
but automated checks cannot supply another person's usability assessment.

The six registry-package exact-notice gaps and historical wrapper/platform
exceptions remain [documented engineering boundaries](alpha/licensing-closeout.md)
for a later binary or expanded-vendor review. They have not been replaced by
invented notices. Windows, Intel macOS, Linux x86_64, musl and WASM remain outside
the verified platform matrix. Point/track products, NISAR HVHV, uninspected grids
and other mission releases remain candidates in the [coverage roadmap](products/roadmap.md).

## Local recovery locations

The private recovery root is
`/Users/jakegearon/.local/share/tile-ripper/recovery/`:

- `2026-10-26-before-integration/` preserves the earlier source revision, verified
  history and scientific extents, original experiment ignore files and validated
  native executables.
- `2026-10-26-consolidated/` preserves final consolidated source/history, scientific
  extents and validation logs. `recovery.json` records commit and archive hashes;
  `RESTORE.md` describes recovery. Bundle verification includes a new clone;
  scientific verification reconstructs sparse files and hashes every saved extent.

The development worktree remains at
`/Users/jakegearon/.codex/worktrees/acquisition-planner/tile-ripper` because its
ignored scientific corpus is retained. Routine code work can use the integrated
`/Users/jakegearon/projects/tile-ripper` checkout. Build directories can be recreated
with the documented locked commands; validated executables are also retained in
the private recovery copy. Source and docs publication followed the owner's approval. No package tag, upstream submission or ChronoZarr change was made.
