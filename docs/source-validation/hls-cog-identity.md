# HLS COG source validation and request reuse

This proposed change preserves fresh source identity checks during preparation,
before execution and after execution. Anonymous readers reuse the exact
asset/locator, strong ETag and positive length from the enclosing check, removing
the adjacent inspection/open HEADs. Each execution owns an immutable receipt
view. Prepared metadata and terminal failure invalidation remain shared; a
receipt from another execution is not installed as ambient validation state.
Only HTTP 200 HEADs can create receipts. Pre/post checks compare the original
ETag and length. Standalone and authenticated readers retain their existing HEAD
fallbacks. Every range still checks If-Match, ETag, HTTP status, Content-Range
including total size, encoding and body bounds.

Anonymous HEADs and ranges share connections without a global identity cache.
Initial TIFF metadata reads fetch at most 8 KiB and retain ordinary 4 KiB pages;
cache caps and payload isolation remain. Exact selected tiles may use a bounded
selection buffer when its reservation admits the same worker concurrency;
UInt8, Int16 and Float32 sample bits and output order are preserved. Tight
budgets retain the window fallback.

Planetary Computer signing reuses only a verified, unexpired container grant
within one preparation, preserving each exact canonical source identity. Signed
queries remain runtime-only. Southern HLS admission is product-private and
requires inspected version/grid/hemisphere evidence; it preserves the actual
source CRS and negative northings without relabelling or reprojection. Other
UTM rules remain unchanged.

Focused regressions cover fresh/repeated/concurrent validation, same-ETag length
changes, HTTP-200-only receipt creation, no-cache paths, bad range poisoning,
standalone fallback, malformed/truncated metadata, exact tile coverage, typed
sample bits and tight-budget fallback. HTTP profiling is opt-in and records
safe filenames and transaction counters without signed queries or bodies.

This branch depends on the existing Icechunk acquisition branch; no new product
family, output format or public identity-receipt API is introduced.
