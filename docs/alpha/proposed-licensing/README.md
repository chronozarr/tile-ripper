# Historical licensing proposal

This directory preserves the October 19 evidence and proposed project grant.
The owner approved Apache-2.0 on October 26; the active project files are
[LICENSE](../../../LICENSE) and [NOTICE](../../../NOTICE).
Its package counts and non-vendored handoff assumptions are historical. Use the
[current source/notice review](../licensing-closeout.md) and
[current handoff procedure](../../consolidation/source-handoff.md).
The current collector requires `--output NEW_DIRECTORY` so these originals are
not overwritten.

`LICENSE` is the unmodified Apache-2.0 text from the Apache Software Foundation.
Its location here does not grant a license to Tile-Ripper. There is no root
LICENSE or Cargo license field. Do not copy this file to the root until authority
is confirmed. The standard appendix contains its original template, not a claim
about Tile-Ripper's owner.

Proposed installation after approval: copy this LICENSE to the repository root,
add `license = "Apache-2.0"` to Cargo.toml, and retain this NOTICE, attribution
index and original third-party texts with the source. The exact project copyright
holder/year statement must come from the authorized owner; it is not invented here.
The project NOTICE has no invented copyright statement. A project NOTICE is
optional absent applicable inherited NOTICE obligations; preserving inherited
notices when redistributing their covered material is required.

## Scope and evidence

THIRD-PARTY-LICENSES.txt contains 267 exact deduplicated license/notice texts for
379 locked registry package versions (all targets and optional metadata nodes).
third-party-index.json maps packages to text hashes and exact registry paths or
revision-pinned URLs. upstream/ stores recovered originals for offline reproduction.
Rebuild with `python3 scripts/license-bundle.py` after `cargo fetch --locked`.
The collector also reads AUTHORS and British-spelled LICENCE files. The previous
41-missing-notices claim overstated the issue because that scan missed LICENCE.

recovered-notices.json records repository revisions from .cargo_vcs_info.json.
extra-notices.json recovers the exact C-Blosc submodule and its FastLZ/bitshuffle
and other embedded licenses. GeoZero's v0.14.0 tag was resolved to a fixed revision
and its src/lib.rs matched the registry byte-for-byte. r-efi's packaged AUTHORS
contains the actual grants/copyright text. No rights were inferred merely from
an unrelated project's license. The index includes inactive dependencies; the
active-dependencies.json lists 289 third-party packages on macOS arm64 and 284 on
Linux arm64, as resolved by cargo tree (plus the project in each target list).

## Requirements versus acknowledgments

Required when distributing covered software: retain relevant copyright and
license/disclaimer texts (MIT/ISC/BSD/Zlib/Unicode/CDLA); include Apache license
and applicable NOTICE text; preserve upstream attribution; identify modifications
to modified Apache files. Upstream dependencies are unmodified. AND clauses
require all applicable licenses. Where OR offers a choice, prefer Apache-2.0 for
Rust dual grants, BSD/MIT/Zlib permissive routes for native library alternatives;
do not select GPL/LGPL alternatives solely because they are in a union.
The bundle conservatively retains alternate and inactive-platform texts as
reference; it does not require complying with mutually alternative licenses at once.

Optional: a new project copyright line in NOTICE, a general contributor thank-you,
badges, links and software-paper citations unless specific upstream terms require
them. They are not substitutes for required license text. Dataset attribution can
be mandatory independently; see DATA-ATTRIBUTION.md.

## Remaining exceptions

Blosc's C submodule notices are recovered exactly. The Rust wrapper revision itself
has no standalone license/copyright file; its Cargo BSD-3-Clause declaration and
author metadata remain the evidence for that wrapper. No wrapper copyright line
was invented. Include this residual notice gap in any binary/vendor review.

- **json_dotpath 1.1.0 (active through cql2/STAC):** Cargo metadata declares MIT,
  author Ondřej Hruška. The registry archive and exact upstream Git revision
  1d929a83c86bce3aa2a53249bd339e3431ff90b5 contain no license/copyright text.
  The original host returned an HTML bot challenge; a GitHub fork contains that
  exact revision and confirms the absent files. No fabricated copyright notice
  was substituted. Obtain upstream confirmation/exact notice before claiming a
  complete binary/vendored-source redistribution notice set. Contacting anyone
  is not authorized or performed. This draft index marks the omission explicitly.
- **pdqselect 0.1.0:** metadata says Apache-2.0/MIT, no repository/revision or
  standalone license. Its packaged attribution names Rust sort_unstable and
  stjepang/orlp pdqsort adaptations. That exact comment is preserved. It is not
  active in either tested target tree; don't treat its missing historical notice
  chain as a cleared binary dependency or copy an unrelated current pdqsort license.
- **winapi GNU import-library packages 0.4.0:** inline 2016 winapi-rs copyright and
  dual-license grant retained; archive has no standalone licenses or recorded
  revision. Windows import-library provenance is not independently cleared.
  These packages are inactive on both tested arm64 platforms.

These exceptions do not describe code vendored in Tile-Ripper. The proposed
source-only handoff supplies project source and lockfile; Cargo obtains dependencies
separately and users build locally. Source handoff does not redistribute their
compiled code. Do not reuse this reasoning for distributing a binary, vendor
archive, container image or GDAL bundle. The source license can be Apache-2.0
subject to the owner/provenance approvals in ../licensing-closeout.md; the complete
installed runtime must not be described as exclusively Apache-2.0.
