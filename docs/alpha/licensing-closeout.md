# Licensing closeout — proposed source alpha, 2026-10-19

Recommendation remains Apache-2.0 for the authorized project-owned source. Exact
[proposed files](proposed-licensing/README.md) are staged for review only. Nothing
has been applied to the project. MIT/Apache dual licensing is also permissive but
offers no remedy for missing authority, inherited notices or GDAL runtime terms.

## Provenance established, and its limits

Git has one named author through 5187801. Initial ada67bb preserves the earlier
research/experiments; 6277ee6 adds the Rust library, CLI and tests as new files.
The saved .dgov/plans/planner/tasks/prototype.toml describes agent implementation
from the local planning brief and preserved study. Later commits trace STAC,
local/HTTP Zarr and scientific-profile additions. This supports an agent-assisted
project implementation history; it is not proof of ownership or originality.
No Git remote is configured. No vendored third-party implementation was found
among tracked project files. Searches of src/tests/examples/bench/study scripts
found no declared copied-from code or external copyright headers. The scientific
algorithms/FFI calls use upstream libraries; use of an API is not by itself copying
its implementation. No all-upstream code similarity search or legal clearance is
claimed. The historical study discusses projects whose licenses do not transfer
to Tile-Ripper automatically.

## Exact confirmations needed from the owner

1. Identify the copyright owner(s) and authority to grant Apache-2.0 copyright
   and relevant patent rights for the authored source, tests, examples, docs and
   historical study/experiment files. Confirm whether employer, university,
   funding or collaborator agreements require institutional review.
2. Confirm whether any code/text was copied or adapted from outside the recorded
   implementation, and provide its original source/license/required notices.
   If so, narrow the grant or preserve those terms before applying a license.
3. Approve Apache-2.0 and the scope above, or identify exclusions/other owners.
   Supply the exact owner/year wording if a project copyright line is desired.

Git author identity is not a substitute for these confirmations. No consent is
assumed from the request to prepare licensing proposals.

## Compatibility assessment

All declared Rust dependency expressions offer permissive choices, including the
AND terms in AWS-LC, ring, encoding_rs, moka and unicode-ident. Exact packaged
notices and missing upstream originals have been assembled and hash-indexed.
The old 41-package missing-file count was a scan bug: it omitted LICENCE and
AUTHORS. Most gaps are resolved; [remaining exception groups](proposed-licensing/README.md)
remain explicitly bounded. json_dotpath and the residual Blosc wrapper notice gap affect active packages.
The embedded C-Blosc license chain itself has been recovered.
Its manifest MIT declaration supports a permissive assessment but does not
recover the missing original notice. No unconditional whole-binary clearance.

Apache-2.0 is a compatible proposed license for authorized Tile-Ripper source
using these permissively licensed libraries, subject to the noted provenance
and notice exceptions. The complete GDAL-linked executable is a different scope:
[GDAL warns](https://gdal.org/en/stable/license.html) that its dependencies may
add LGPL/GPL/proprietary terms. The audited Homebrew runtime actually links
GPL/LGPL components. Apache-only redistribution of that whole runtime is not
cleared. GPL-2.0-only combinations and additional runtime terms require review;
dynamic linking alone is not a compliance exemption. No GDAL code, data resources,
plugins, container, runtime or executable will be supplied in this alpha source
handoff. Users install system GDAL separately and compile locally; don't describe
the resulting runtime as solely Apache-2.0.

## Distribution obligations

For the proposed non-vendored source handoff: apply the authorized project license,
retain project/inherited attribution and the reviewed notices/docs/lockfile, exclude
ignored build artifacts and acquired datasets, and supply a fixed source revision.
No dependency source, Rust executable or GDAL runtime is redistributed. The
original upstream distributors supply Cargo and native packages independently.
Retain the audit/bundle as a courtesy and for local binary notice reference, with
its exceptions visible. A missing notice for a separately acquired dependency is
not the same as a missing notice for code copied into the distributed source.

For any future vendor/binary/container handoff: complete json_dotpath's exact
notice and any applicable inactive-platform exceptions; establish the actual
binary link set, all original copyright texts and license choices; provide required
licenses/NOTICE/source or relinking materials where applicable. That work is
outside this source-only alpha. Do not claim this draft bundle clears it.

Required notices derive from actual distributed material and license conditions.
A new project NOTICE is optional here; the proposed NOTICE is informational.
Provider data attribution/citation obligations are separate and documented without
inventing a universal dataset license. [Apache terms](https://www.apache.org/licenses/LICENSE-2.0.html)
and original texts in the proposal are the controlling sources, not this summary.
