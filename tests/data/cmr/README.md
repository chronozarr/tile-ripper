# Frozen NASA CMR metadata

`collection.json` is the exact collection resolution response for
`SWOT_L2_HR_Raster_D`, version D, provider POCLOUD. `original.json` and `held.json`
are the exact bounded bbox/time granule responses for the two known controls.
These are public catalog metadata, with no Earthdata credentials or scientific
archive object contents.

[Capture URLs, sizes and SHA-256 hashes](../../../docs/swot-discovery/catalog-captures.json)
and [the predeclared live protocol](../../../docs/swot-discovery/protocol.json)
document provenance. `tests/swot_discovery.rs` derives deterministic malformed,
conflicting, duplicate, multi-date, reordered and bounded-failure variants in
memory. `bench/swot_discovery.py offline` serves the exact captures over explicit
numeric loopback HTTP and verifies the actual scientist-facing CLI pagination.
Neither offline path contacts NASA or opens scientific archive objects.
