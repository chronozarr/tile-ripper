# Preserved control

Control commit: `fe6fcd1` on `codex/acquisition-planner`. The existing scientific and frozen benchmark receipts remain under `docs/zarr-v3-input/`. `baseline.json` records their SHA-256 hashes, the frozen ordinary/sharded fixture hashes, and executable copies under ignored `prototype-output/zarr-coalescing/baseline-fe6fcd1/`. Benchmarks use these copies as the previous implementation.

The stream keeps plan order and uses consumer acknowledgements before releasing admitted work. Coalesced payload storage must be released before its scheduler reservation. HTTP ranges retain strong ETag/If-Match validation and exact 206 framing. Publication, output defaults, COG and v2 behavior remain governed by their existing implementations and validation contracts.
