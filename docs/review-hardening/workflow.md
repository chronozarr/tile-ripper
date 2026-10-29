# Acquisition review hardening

This follow-up addresses the 26 October 2026 code review. It changes acquisition
lifecycle behavior, not the admitted scientific product profiles or production
dependencies. The separate [OPERA assessment](../products/opera-expansion-assessment.md)
is a proposal; no new product is enabled by it.

The implementation preserves prepared source bindings during GeoTIFF writing,
rejects unsupported GeoTIFF storage types during planning, bounds local metadata
reads before parsing, and uses one staged publication protocol for directory
outputs. Legacy convenience fetch calls use the prepared lifecycle so source
validation precedes publication. Native and remote COG planning also uses
prepared source bindings. Prepared acquisitions retain their selected
backend and scheduler settings across execution threads and streams.

Directory outputs are written in a unique hidden sibling. The final destination
is reserved by an atomic no-replace rename only after writing, completion checks
and synchronization. Existing files, directories and dangling links are refused.
An abrupt process exit can retain a hidden sibling, which is never automatically
adopted on retry. A failure after the commit rename retains the complete output
and reports unconfirmed durability.

Scientific Zarr writers retain their synchronization on original writing handles.
The shared publication step also synchronizes staged regular files and directories
to cover legacy writers. A successful file sweep is not a power-cut certification
or evidence that an earlier, ignored handle error never occurred.

The [Rust workflow](../../.github/workflows/rust.yml) runs the six supported
feature configurations on Rust 1.91 and 1.92, plus formatting, Clippy, vendored
crate tests and both native-statistics consumer configurations. Ordinary Cargo
tests include hermetic HTTP controls; provider access and protected credentials
are unnecessary. Historical ignored tests retain their owning harnesses.

## Repeating validation

The original Wave 4 and consolidation receipts remain historical evidence bound
to their original source hashes. The validation command still checks those hashes
by default. For an intentional follow-up, select its own exact source receipt:

```sh
python3 scripts/validate.py --toolchain 1.92.0 \
  --source-receipt docs/review-hardening/acceptance.json \
  --output prototype-output/review-hardening/rust-1.92-new
```

The selector does not skip source checks: every hash in the selected receipt
must match. Run the same command with Rust 1.91.0 for the minimum-toolchain
matrix. Use a new result directory for each run.

The [frozen science replay](../consolidation/validation.md#independent-frozen-scientific-replay)
remains separate. It uses guarded local captures, disables external HTTP in
Python references, compares raw values and navigation through independent readers,
and rechecks the original capture manifests. Historical captures and receipts
are not rewritten to fit the new implementation. Current results and limits are
recorded in this directory's acceptance receipt after verification.

## Verified results

The [acceptance receipt](acceptance.json) binds the follow-up source hashes to:

- Passing six-build matrices on [Rust 1.91](checks/rust-1.91.json) and
  [Rust 1.92](checks/rust-1.92.json), including formatting, Clippy, both vendored
  suites, both consumer configurations and Python syntax checks.
- Passing [frozen scientific replay](checks/science-replay.json): 11 groups,
  including the original NISAR full-coordinate oracle over 1,229,401,152 centers
  and northern-SMAP full-coordinate checks. Original capture manifests remain
  unchanged.
- A passing [local authenticated HTTPS GeoTIFF reproduction](checks/authenticated-geotiff.json),
  using an explicitly supplied private test trust root and dummy credentials.
- An independent integration review with no remaining blocking findings.

Ignored tests retain their existing helper/external-fixture requirements. No
protected production-provider acquisition, hosted CI run, package release or
new product-profile qualification is claimed by these local results.
