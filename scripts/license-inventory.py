#!/usr/bin/env python3
"""Inventory every locked package and packaged notice, including nested native code.
Run after cargo fetch --locked. This collects evidence; it does not certify licenses.
"""
import hashlib
import json
from pathlib import Path
import subprocess

metadata = json.loads(subprocess.check_output(
    ['cargo', 'metadata', '--locked', '--offline', '--all-features', '--format-version', '1']))
base = Path(__file__).resolve().parents[1]
rows = []
for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
    root = Path(package['manifest_path']).parent
    if root == base:
        continue
    notices = []
    for path in sorted(root.rglob('*')):
        if 'target' in path.relative_to(root).parts:
            continue
        if path.is_file() and path.name.lower().startswith(('license', 'licence', 'copying', 'copyright', 'notice', 'authors')):
            notices.append({'path': str(path.relative_to(root)),
                            'sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
    rows.append({'name': package['name'], 'version': package['version'],
                 'source_kind': 'registry' if package['source'] else 'vendored',
                 'source': package['source'], 'license': package['license'],
                 'license_file': package['license_file'], 'repository': package['repository'],
                 'notices': notices})
print(json.dumps({'scope': 'all Cargo metadata targets, including build and target-specific packages',
                  'packages': rows}, indent=2))
