#!/usr/bin/env node
// Independent metadata-only introspection with icechunk-js 0.6.0. The library's
// public getChunk resolves the requested reference; replacing its internal fetch
// method exposes that parsed payload without downloading the GRIB. This helper
// is qualification instrumentation, never a production source adapter.
// Example (install dependency in /tmp, not this Rust repo):
// npm --prefix /tmp/icechunk-reference install --no-audit --no-fund icechunk-js@0.6.0
// node bench/icechunk_reference_payload.mjs \
//   /tmp/icechunk-reference/node_modules/icechunk-js/dist/index.js \
//   docs/icechunk/virtual-reference.json
import { pathToFileURL } from 'node:url';
import { writeFile } from 'node:fs/promises';
const [modulePath, output] = process.argv.slice(2);
if (!modulePath || !output) throw new Error('Pass absolute icechunk-js dist/index.js path and output JSON');
const { Repository, HttpStorage } = await import(pathToFileURL(modulePath));
const root = 'https://dynamical-noaa-gfs.s3.us-west-2.amazonaws.com/noaa-gfs-analysis-virtual/v0.1.0.icechunk';
const snapshot = 'SJSV5KG1GSWR2XQD2SW0';
const session = await (await Repository.open({ storage: new HttpStorage(root) })).checkoutSnapshot(snapshot);
let payload;
if (typeof session.fetchChunkPayload !== 'function') throw new Error('Reference instrumentation requires icechunk-js 0.6.0');
session.fetchChunkPayload = async (parsed) => { payload = parsed; return new Uint8Array(); };
await session.getChunk('/temperature_2m', [48072, 0, 0]);
if (payload?.type !== 'virtual') throw new Error('Expected virtual reference');
const receipt = {
  created_utc: new Date().toISOString(),
  library: 'icechunk-js 0.6.0', repository_url: root, snapshot_id: snapshot,
  array: '/temperature_2m', chunk_coordinates: [48072, 0, 0], payload,
  conclusion: 'No original virtual-reference ETag or last-modified checksum is supplied. A runtime object version pin alone cannot prove external bytes equal those at snapshot creation.',
  payload_data_fetched: false,
};
await writeFile(output, JSON.stringify(receipt, null, 2) + '\n');
console.log(JSON.stringify(receipt));
