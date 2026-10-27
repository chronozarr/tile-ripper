#!/usr/bin/env python3
"""Compare one identity-bound logical payload from current Icechunk and legacy Zarr.

Uses the dependencies and forwarding meter in icechunk_qualification.py, plus
fsspec[http]. This does not register interchangeable datasets or providers.
"""
from __future__ import annotations
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path

import icechunk_qualification as q


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('docs/icechunk/legacy-comparison.json'))
    args = parser.parse_args()
    out = {
        'created_utc': datetime.now(timezone.utc).isoformat(),
        'selection': {'start': [15000,224,316], 'count': [1,2,2],
                      'time_utc': '2023-01-16T00:00:00Z', 'latitude': [34,33.75],
                      'longitude': [-101,-100.75]},
        'limit': 'Selected values and logical compressed payload hashes only; not archive-wide equivalence',
    }
    with q.proxy(32*1024*1024) as (base,meter):
        session, group = q.open_product(base,'materialized',q.DEFAULT_SNAPSHOTS['materialized'])
        array = group['temperature_2m']
        mark = meter.mark('materialized_selection')
        value = array[15000:15001,224:226,316:318]
        out['icechunk'] = {'snapshot_id':session.snapshot_id, 'values':q.numeric_receipt(value),
                           'transport':meter.since(mark)}
        legacy = q.zarr.open_group(base+'/legacy/',mode='r')['temperature_2m']
        out['legacy_metadata'] = legacy.metadata.to_dict()
        mark = meter.mark('legacy_selection')
        reference = legacy[15000:15001,224:226,316:318]
        out['legacy'] = {'values':q.numeric_receipt(reference), 'transport':meter.since(mark)}
        out['bit_equal_selected_values'] = bool(q.np.array_equal(value,reference,equal_nan=True))
        # Identify payload by exact compressed size, not assumed object names.
        payload = {kind:[e for e in out[kind]['transport']['events'] if e.get('response_body_bytes') == 3985993]
                   for kind in ['icechunk','legacy']}
        out['bit_equal_selected_compressed_payload'] = bool(
            len(payload['icechunk'])==len(payload['legacy'])==1
            and payload['icechunk'][0]['sha256']==payload['legacy'][0]['sha256'])
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(out,indent=2,allow_nan=False)+'\n')
    print(json.dumps({'output':str(args.output),
                      'bit_equal_selected_values':out['bit_equal_selected_values'],
                      'bit_equal_selected_compressed_payload':out['bit_equal_selected_compressed_payload']}))


if __name__=='__main__': main()
