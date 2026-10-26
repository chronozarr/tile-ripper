# Private alpha closeout — 0.1.0-alpha.1 candidate

> Historical October 19 closeout. The current tree requires vendored
> `hdf5-reader` and `zarrs_filesystem` sources, so the non-vendored handoff below
> is no longer a usable procedure for this branch. See the
> [current consolidation checklist](../consolidation.md) before preparing a candidate.

The source alpha is prepared for approval and a first human trial. No license is
applied, no tag exists, and no distribution/publication is authorized by this report.
The earlier readiness work is preserved in local commit `022d9f1`.
No format, decoder, scheduling, performance or architecture changes were made.

## Decisions requiring approval

- Identify the actual copyright owner(s), authority to grant Apache-2.0 and any
  institutional/employer/collaborator review required for source and historical
  research material. Git author identity is not authority.
- Confirm external copy/adaptation provenance, or supply its sources and terms.
- Approve Apache-2.0 scope and any exact project copyright statement.

The exact proposed [LICENSE](proposed-licensing/LICENSE), [NOTICE](proposed-licensing/NOTICE),
[third-party texts/index](proposed-licensing/README.md) and
[data attribution guidance](proposed-licensing/DATA-ATTRIBUTION.md) are review files,
not a project grant. [Licensing closeout](licensing-closeout.md) details the evidence.

## Upstream exceptions and distribution boundary

The corrected collector handles LICENCE/AUTHORS as well as LICENSE. It recovers
Blosc's exact submodule licenses and other omitted originals. The review bundle
contains original text and hash/source references for all locked packages, with
an explicit gap for json_dotpath 1.1.0 (MIT declared but original notice absent).
The Blosc wrapper itself still has only its metadata declaration; its C submodule
notices are recovered. Additional inactive pdqselect/Windows import-library exceptions are documented.
No blanket claim of complete binary notice clearance is made.

These are separately acquired dependencies. The chosen private-alpha handoff is
**non-vendored source and lockfile only**, with tester-managed system GDAL and a
local Cargo build. No dependency archive, executable, runtime or container is
included. Retain the audit exceptions visibly. Resolve the active upstream notice
before any binary/vendor redistribution; never treat system GDAL's complete linked
runtime as solely Apache-2.0. Source handoff still requires owner/provenance approval.

## Readiness and actual platform evidence

The source workflow is otherwise ready for a small supervised human trial after
the authority/license gate. Source install, output understanding and correctness
are supported by agent/automated evidence; human usability remains unvalidated.

| Platform | Evidence and limits |
| --- | --- |
| macOS arm64 | Homebrew GDAL 3.12.4, Rust 1.92.0; prior 37 Rust tests; final candidate empty-target install, public S2/ERA5 fetch/open and uninstall |
| Ubuntu 24.04 arm64 | Container with apt GDAL 3.8.4, Rust 1.92.0; prior clean install, 37 serial release tests; final candidate also fetched S2/ERA5 and uninstalled |
| Other macOS/Linux architectures, Windows | Untested; not excluded by design |

The final candidate command receipts are recorded in [verification](verification.md).
A container is not a fresh physical-machine trial; Homebrew provisioning reused
the host setup. GDAL dependency and API instability are documented limitations.
No performance advantage is promised. [ALPHA.md](../../ALPHA.md) is the short
entrypoint and supported-features matrix. [Exact contract](contract.md) retains
layout limits. [Human trial](human-trial.md) includes two bounded real examples,
plan/output inspection, timing, errors, source compatibility and feedback.

## Proposed distribution procedure — do not execute yet

1. Obtain the three ownership/provenance/license decisions above; apply the
   approved license/metadata and any required scope exclusions/notices in a
   separate reviewable commit. Keep `publish=false`.
2. Freeze that approved revision. Recheck locked installation and the two small
   examples there; run the human trial and record actual results. A failed trial
   should produce corrections before broader private invitations.
3. With explicit tag/distribution approval, create annotated tag
   `v0.1.0-alpha.1` at the approved revision and provide read-only access to the
   intended private recipients, or an approved source-only snapshot with revision
   and checksum. No remote currently exists here; do not invent a public URL.
4. Source contents: project source/tests/examples/docs/scripts, Cargo manifests
   and lockfile, approved license and notices. Exclude ignored .alpha/, target/,
   .venv/, prototype-output/, acquired data and credentials. Do not vendor Cargo
   dependencies or include system GDAL. No release automation is needed.
5. Give the first tester only ALPHA.md and the source folder; ALPHA.md leads to
   the self-contained human-trial.md and feedback-template.md. The maintainer
   records the exact approved revision and agreed feedback channel before handoff.

For an internal first trial before an alpha tag, an approved fixed commit is
sufficient; tagging need not precede learning from a real user. All trial setup,
install, S2/ERA5 commands, inspection and uninstall are in one guide.

Stop here: licensing authority and any institutional decisions are reserved for
the owner. No merge, tag, release archive, binary, package, upload, domain change,
or tileripper.com deployment has occurred. No real person has completed the trial.
