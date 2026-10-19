#!/usr/bin/env python3
"""Collect exact notice evidence; unresolved entries remain explicit, never invented."""
import hashlib
import json
from pathlib import Path
import subprocess

base = Path(__file__).resolve().parents[1]
review = base / 'docs/alpha/proposed-licensing'
metadata = json.loads(subprocess.check_output([
    'cargo', 'metadata', '--locked', '--offline', '--format-version', '1'], cwd=base))
recovered = json.loads((review / 'recovered-notices.json').read_text())
extra = json.loads((review / 'extra-notices.json').read_text())
texts = {}
rows = []

def add(data):
    digest = hashlib.sha256(data).hexdigest()
    texts[digest] = data.decode('utf-8', errors='replace')
    return digest

for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
    if package['source'] is None:
        continue
    root = Path(package['manifest_path']).parent
    row = {'name': package['name'], 'version': package['version'],
           'declared_license': package['license'], 'notices': []}
    for path in sorted(root.rglob('*')):
        if path.is_file() and path.name.lower().startswith((
                'license', 'licence', 'copying', 'copyright', 'notice', 'authors')):
            row['notices'].append({'source': 'registry:' + str(path.relative_to(root)),
                                   'sha256': add(path.read_bytes())})
    for upstream in recovered:
        if (upstream['name'], upstream['version']) == (package['name'], package['version']):
            for file in upstream['files']:
                data = (review / 'upstream' / (file['sha256'] + '.txt')).read_bytes()
                assert hashlib.sha256(data).hexdigest() == file['sha256']
                row['notices'].append({'source': file['url'], 'sha256': add(data)})
    for file in extra:
        if (file['name'], file['version']) == (package['name'], package['version']) and 'sha256' in file:
            data = (review / 'upstream' / (file['sha256'] + '.txt')).read_bytes()
            assert hashlib.sha256(data).hexdigest() == file['sha256']
            row['notices'].append({'source': file['url'], 'sha256': add(data)})
    if package['name'].startswith('winapi-') or package['name'] == 'pdqselect':
        # Preserve the exact packaged attribution comment; don't invent missing holders.
        lines = []
        for line in (root / 'src/lib.rs').read_text().splitlines(keepends=True):
            if not line.startswith('//'):
                break
            lines.append(line)
        row['notices'].append({'source': 'registry:src/lib.rs initial attribution',
                               'sha256': add(''.join(lines).encode())})
    if package['name'] == 'unaccent':
        row['review_note'] = 'Packaged LICENSE is MIT; Cargo expression is absent.'
    if package['name'] in ('json_dotpath', 'pdqselect') or package['name'].startswith('winapi-'):
        row['review_required'] = 'Standalone exact upstream license/copyright text incomplete; see audit.'
    rows.append(row)

(review / 'third-party-index.json').write_text(json.dumps(rows, indent=2) + '\n')
header = '''PROPOSED THIRD-PARTY LICENSE EVIDENCE — NOT A PROJECT LICENSE GRANT

All locked registry packages are indexed in third-party-index.json, including
inactive optional/target dependencies. Exact original texts are deduplicated by
SHA-256. Alternate license texts are retained as evidence, not selected together.
See README.md for license choices, required notices and unresolved exceptions.
No GDAL runtime, vendored crate, or executable is included in the source alpha.

'''
with (review / 'THIRD-PARTY-LICENSES.txt').open('w') as output:
    output.write(header)
    for digest, text in sorted(texts.items()):
        output.write('\n===== SHA256 ' + digest + ' =====\n' + text + '\n')
print(f'{len(rows)} packages; {len(texts)} unique exact notice texts')
