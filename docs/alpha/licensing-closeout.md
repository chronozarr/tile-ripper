# Current source and notice review

The current source includes modified `hdf5-reader 0.9.3` and
`zarrs_filesystem 0.3.13`. Their original MIT/Apache texts are retained beside the
source, and their changes are documented in full [fork diffs and file hashes](../consolidation/vendor/vendor-audit.json).
The October 19 no-vendoring conclusion is superseded; its evidence remains in
Git history and the unchanged proposed-licensing directory.

No project license has been installed. Git authorship does not establish the
owner's authority or resolve institutional/collaborator rights. The exact owner,
provenance and license decisions are listed in [the current closeout](closeout.md).
The earlier Apache-2.0 text and NOTICE remain proposals only.

## Current third-party evidence

The [current inventory](../consolidation/license-inventory.json) contains 426
third-party package versions, including optional and target-specific packages and
the two vendored forks. [The current text index](../consolidation/notices/third-party-index.json)
maps exact packaged and recovered originals to SHA-256 texts in the
[notice bundle](../consolidation/notices/THIRD-PARTY-LICENSES.txt).
The collector now includes path dependencies and writes a new output directory;
it cannot overwrite a historical bundle by default.

Nine additional packages' missing notices were recovered from the Git revisions
recorded in their registry packages. Ten packages' `src/lib.rs` files
matched the same revision byte-for-byte. [Recovery provenance](../consolidation/recovered-notices.json)
records the exact URLs, revisions, local files and hashes. Original notices for
the vendored forks are present in the distributed source.

## Unresolved upstream evidence

Six packages still lack exact notice texts in the combined current evidence:

- `json_dotpath 1.1.0`: the earlier exact-revision audit found no original notice;
  its MIT metadata declaration remains separate evidence.
- `crc32c 0.6.8`: the recorded revision matched the packaged source, but contained
  no applicable original notice file; its license metadata remains separate.
- `lerc-band-materialize`, `lerc-core` and `lerc-reader 0.4.3`: the recorded
  `i-norden/lerc-rust` revision returned HTTP 404 during this recovery.
- `proj4rs 0.2.1`: the recorded `3liz/proj4rs` revision returned HTTP 422 during
  the immutable-tree lookup.

[The recovery result](../consolidation/upstream/unresolved.json) retains these
failures without substituting a current unrelated license. The historical
Blosc-wrapper, pdqselect and Windows import-library exceptions also remain as
described in [the earlier bundle](proposed-licensing/README.md); recovered C-Blosc
submodule texts remain present. Metadata declarations and inline attributions do
not become invented original notices.

These unresolved registry packages are not copied into the source snapshot;
Cargo obtains them separately. They remain material to any later binary or
expanded-vendor review. A bundled GDAL runtime would add its own actual linked
components and terms. This review records the source and evidence; it does not
claim complete binary redistribution clearance. Use the
[current source contents and handoff](../consolidation/source-handoff.md), with
the owner's project-rights decision, for the proposed source-only candidate.
