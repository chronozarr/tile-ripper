#!/usr/bin/env python3
"""Automated public trial smoke; this is not a human usability result."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    binary, output = args.binary.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {'status': 'running', 'human_trial': False,
              'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'commands': []}
    try:
        import xarray as xr
        import zarr
        for label, request in [('era5', 'examples/weatherbench2.json'), ('goes', 'examples/product-goes-abi.json')]:
            for action in ('plan', 'fetch'):
                command = [str(binary), action, str(ROOT / request), '--gdal-free']
                if action == 'fetch':
                    command += ['--output', str(output / label)]
                started = time.monotonic()
                result = subprocess.run(command, text=True, capture_output=True, cwd=ROOT)
                (output / (label + '-' + action + '.json')).write_text(result.stdout)
                (output / (label + '-' + action + '.stderr')).write_text(result.stderr)
                report['commands'].append({'dataset': label, 'action': action, 'exit_code': result.returncode,
                                           'seconds': time.monotonic() - started})
                if result.returncode:
                    raise RuntimeError(f'{label} {action} failed; inspect its local stderr log')
            manifest = json.loads((output / label / 'manifest.json').read_text())
            if manifest['status'] != 'complete':
                raise RuntimeError('incomplete manifest: ' + label)
        with xr.open_zarr(str(output / 'era5/era5.zarr'), consolidated=False, chunks=None, decode_cf=False) as ds:
            ds.load()
            assert dict(ds.sizes) == {'time': 2, 'latitude': 4, 'longitude': 3}
            assert ds['2m_temperature'].dtype.name == 'float32' and ds['2m_temperature'].attrs['units'] == 'K'
            report['era5'] = {'sizes': dict(ds.sizes), 'dtype': 'float32', 'units': 'K'}
        group = zarr.open_group(str(output / 'goes'), mode='r')
        observations = []
        for name in group['observations'].group_keys():
            with xr.open_zarr(str(output / 'goes'), group='observations/' + name, consolidated=False, chunks=None, decode_cf=False) as ds:
                ds.load()
                assert ds.CMI.dtype.name == 'int16' and ds.DQF.dtype.name == 'int8'
                observations.append({'id': name, 'sizes': dict(ds.sizes), 'CMI': 'int16', 'DQF': 'int8'})
        assert observations
        report['goes'] = observations
        report['status'] = 'passed'
    except Exception as error:
        report['status'], report['error'] = 'failed', str(error)
    finally:
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    sys.exit(main())
