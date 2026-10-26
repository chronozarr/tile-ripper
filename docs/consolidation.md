# Tile-Ripper consolidation checklist

Audit date: 25 October 2026. Development baseline: `a5e18c2` on
`codex/acquisition-planner`. Finish the candidate contract, source handoff and
repeatable validation before starting another product wave.

## Current development

The implementation is 64 commits beyond `main`, which still contains the
original study at `ada67bb`. The development worktree is
`/Users/jakegearon/.codex/worktrees/acquisition-planner/tile-ripper`.
Neither checkout has a remote; no release tag exists. The implementation tree
was clean before this documentation pass. The main checkout has three untracked
experiment `.gitignore` files, left untouched.

Wave 4 is complete within its inspected source profiles: geographic NISAR HHHH
on two UTM16N grids, northern-polar SMAP AM/PM and GOES-19 mesoscale-1 C02.
[Qualification](products/wave4/qualification.md) and its
[acceptance receipt](products/wave4/acceptance.json) retain the science, access,
fault and platform evidence. Every source hash listed in that receipt matches
the audited checkout. Other products keep their own qualification boundaries;
the [coverage roadmap](products/roadmap.md) distinguishes qualified profiles
from candidates. ATL06, ATL08, GEDI and NISAR HVHV remain unimplemented or
unqualified, as specified in the product reports.

## Closeout work in priority order

| Priority | Loose end | Completion condition |
| --- | --- | --- |
| 1 | The current implementation has no consolidated candidate contract. `ALPHA.md`, the alpha contract and its human trial describe the earlier October 19 candidate. | Choose the next candidate's supported profiles and feature builds; reconcile its installation, limitations, examples and human trial against one fixed revision. Preserve historical science reports. |
| 1 | The old source-handoff procedure excludes vendored dependencies, but Cargo now requires two local forks. Its licensing audit also says no vendored implementation was found. | Review the actual current source contents and both forks, their original notices and local changes; replace the old handoff procedure. Obtain the ownership/provenance/license decisions already identified in the alpha closeout before distribution. |
| 1 | All implementation history is local and lives outside the main checkout. | Establish the intended integration and backup destination, preserve the qualified revision and its evidence, then perform the agreed integration. Do not infer a remote or public-release destination. |
| 2 | Verification is spread across milestone commands, Python harnesses and ignored captures. No `.github` workflow or single local validation entrypoint exists. | Provide one documented local command for the supported build/test matrix and separate commands for independent replay and live-provider gates. Document capture prerequisites and skipped tests; keep live runs explicit and write new results separately from sealed receipts. |
| 2 | Alpha usability has only agent/automated evidence. | Run the chosen candidate's complete install, plan, fetch, inspect and uninstall trial with a scientist; record their actual feedback and resolve failures. |
| 2 | Minimum-toolchain and platform evidence is uneven. | Extend the Rust 1.91 compile check below to tests and each supported feature build, or adjust the declared minimum. Keep Windows, Intel macOS and Linux x86_64 outside the verified matrix until exercised. |
| 3 | Two vendored forks need an explicit maintenance decision. | Retain exact upstream bases, patches and tests; choose whether to maintain, rebase or propose each change upstream. The [HDF5 patch note](../vendor/hdf5-reader/TILE_RIPPER_PATCH.md) and [filesystem patch note](../vendor/zarrs_filesystem/TILE-RIPPER-PATCH.md) describe the current changes. |
| 3 | Local build artifacts occupy about 40 GiB in the root `target/` alone. | Inventory ignored captures and build directories, retain evidence needed for replay, then remove only agreed regenerable artifacts. No cleanup was performed in this audit. |

The existing [alpha closeout](alpha/closeout.md) records unresolved owner decisions.
Its October 19 source-only procedure cannot describe the current vendored tree
without revision. `publish = false` remains appropriate while the candidate is
being consolidated.

## Validation during this audit

These checks ran against `a5e18c2` on macOS ARM64:

| Check | Result |
| --- | --- |
| `cargo test --locked --all-features --all-targets -- --test-threads=1` | 216 passed, zero failed, 9 ignored |
| `cargo test --locked --no-default-features --features native-cog,native-hdf5,authenticated-http -- --test-threads=1` | 187 passed, zero failed, 8 ignored |
| `cargo clippy --locked --all-features --all-targets -- -D warnings` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo +1.91.0 check --locked --no-default-features --features native-cog,native-hdf5,authenticated-http` | Passed; a compile check, not minimum-toolchain test qualification |
| Local Markdown targets in README, ALPHA and docs, excluding the proposed licensing bundle | Zero missing targets |
| Wave 4 receipt source checksums | All matched |

Ignored tests include subprocess helpers and controls requiring external
ownership, real-source probes or dedicated HTTP/HTTPS fixtures. The ordinary
Cargo suite does not replace those harnesses. This audit did not rerun live
provider acquisition, independent scientific replay or Linux portability;
those remain the milestone receipts' evidence. No consistent provider speed
advantage is established.

Audit logs are local at `/tmp/tile-ripper-audit-{native-tests,all-tests,clippy,fmt,msrv}.log`;
they are temporary diagnostics, not a new sealed acceptance package.
