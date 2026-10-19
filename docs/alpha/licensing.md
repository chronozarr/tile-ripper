> Historical readiness audit. The [licensing closeout](licensing-closeout.md) corrects
> the LICENCE/AUTHORS scan omission and supersedes the missing-notice conclusions.

# Licensing audit — 2026-10-19

Scope: candidate based on 5187801, complete Cargo metadata graph (379 third-party
package versions, all targets/build dependencies), packaged license files, bundled
native sources, current macOS dynamic linkage, and repository provenance indicators.
This is an evidence audit, not an assertion that metadata alone clears distribution.

## Recommendation and unresolved decision

Recommend **Apache-2.0** for project-owned Rust/source documentation once the owner
confirms rights. It provides explicit contributor patent terms and one clear grant.
`MIT OR Apache-2.0` is also plausible for project-owned code and conventional in Rust,
but adds a choice of grant without removing any third-party obligations. Neither
option relicenses dependencies, provider data, or copied material. Apache-only
third-party portions remain Apache-only even in a dual-licensed project.

Do not install a root license yet. Git history lists James H. Gearon as author,
but does not establish employer/institution ownership or permissions for every
historical study/experiment. Confirm authority to license project code, docs and
study material; identify any externally copied snippets and their original notices.
No root license or Cargo license field has been added; `publish=false` remains.
The study's earlier MIT strategy is a proposal, not a license grant.

## Rust tree and native code

[rust-license-inventory.json](rust-license-inventory.json) lists every registry
package, expression, repository and hashes/paths of recursively found packaged
LICENSE/COPYING/COPYRIGHT/NOTICE files. Reproduce after `cargo fetch --locked` with
`python3 scripts/license-inventory.py > docs/alpha/rust-license-inventory.json`.
This includes inactive platform dependencies, so it exceeds one binary's link set.

All declared expressions offer permissive routes; no dependency requires choosing
a GPL/LGPL alternative. AND clauses must all be honored. Preserve Apache notices
and modification notices where applicable, MIT/ISC/BSD copyright and permission
text, Unicode-3.0 notices, and CDLA-Permissive-2.0 terms for root certificate data.
`unaccent 0.1.1` has no expression but its packaged LICENSE is MIT (CROWDTECH,
2025). This was read directly, not inferred from another project.

**Notice-completeness blocker:** 41 package archives contain no standalone notice
file matching these names, including gdal/gdal-sys, stac, zarrs and blosc-src.
Their metadata is not a replacement for original copyright notices. In particular,
blosc-src 0.3.8 bundles C sources referencing LICENSE.txt, but omits that file via
its package include rules. Recover notices from the precise upstream revisions,
including C-Blosc/BloscLZ and per-file native contributions, before distribution.
This is not evidence that these packages are forbidden; clearance is incomplete.
The inventory is not a ready-to-distribute THIRD-PARTY-NOTICES bundle.

Native source review includes aws-lc-sys's combined ISC/Apache/MIT/BSD obligations,
ring's BoringSSL/other-bits/fiat texts, encoding_rs's WHATWG BSD text, moka's
Apache NOTICE, lz4-sys's wrapper and embedded LZ4 BSD text, and zstd-sys's wrapper
and embedded Zstd BSD/GPL choice (choose BSD for relevant library sources).
Cargo wrapper metadata alone misses some embedded library notices. Retain all
applicable embedded texts; do not assume a wrapper's MIT grant covers its C code.

## GDAL and binary redistribution

[macos-linkage.json](macos-linkage.json) recursively records 193 non-system image
entries, including aliases; this is not 193 unique projects.
[macos-native-licenses.json](macos-native-licenses.json) maps 83 installed formulae
to current Homebrew license metadata. Formula metadata covers more than linked
library files and is not sufficient to determine each file's obligations.
Actual linkage includes libspatialite/librttopo, poppler, libheif/x265 and LGPL
libraries. Do not label the complete runtime MIT merely because GDAL core is MIT.
No self-contained binary bundle is cleared. A minimal rebuilt GDAL runtime,
platform-specific data/resources and exact binary/source notices would need
separate compatibility and redistribution review. Dynamic linking does not
by itself eliminate those obligations.

References: [GDAL's explicit dependency-license warning](https://gdal.org/en/stable/license.html),
[Apache-2.0 terms](https://www.apache.org/licenses/LICENSE-2.0.html),
[Apache licensing FAQ](https://www.apache.org/foundation/license-faq.html).

## Copied code and data attribution

Searches of tracked src/tests/examples and historical study scripts found no
explicit external code copyright headers or declared vendored source directory.
This negative search is not proof of original authorship. No code was compared
against every possible upstream; owner provenance attestation remains required.
The initial study and its external benchmark references must be covered separately
or excluded from an eventual source release, with an explicit scope statement.

Keep third-party software notices distinct from dataset attribution. Sentinel
redistribution should carry the relevant Copernicus year/modified-data attribution
and provider terms; preserve collection/item license links and source identity.
ERA5/WeatherBench2 and NASA POWER IMERG users must follow their provider citation
and upstream data terms; software licensing grants no rights to those datasets.
A final NOTICE/attribution document should identify project ownership, applicable
upstream NOTICE contents, and provider acknowledgments. Do not invent a copyright
holder or treat the acquisition manifest alone as complete legal attribution.

Decision: unresolved ownership and incomplete exact upstream notices block a
licensed distributable alpha. They do not justify guessing a license.
