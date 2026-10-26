#!/usr/bin/env python3
"""Collect exact notice evidence; unresolved entries remain explicit, never invented."""
import hashlib
import json
from pathlib import Path
import subprocess
import argparse

base = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, required=True,
                    help='New directory; historical evidence is never overwritten')
args = parser.parse_args()
review = base / 'docs/alpha/proposed-licensing'
output_directory = args.output.resolve()
output_directory.mkdir(parents=True, exist_ok=False)
metadata = json.loads(subprocess.check_output([
    'cargo', 'metadata', '--locked', '--offline', '--all-features', '--format-version', '1'], cwd=base))
recovered = json.loads((review / 'recovered-notices.json').read_text())
extra = json.loads((review / 'extra-notices.json').read_text())
current_recovery = base / 'docs/consolidation/recovered-notices.json'
if current_recovery.exists():
    recovered += json.loads(current_recovery.read_text())
texts = {}
rows = []

def add(data):
    digest = hashlib.sha256(data).hexdigest()
    texts[digest] = data.decode('utf-8', errors='replace')
    return digest

for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
    root = Path(package['manifest_path']).parent
    if root == base:
        continue
    row = {'name': package['name'], 'version': package['version'],
           'source_kind': 'registry' if package['source'] else 'vendored',
           'declared_license': package['license'], 'notices': []}
    for path in sorted(root.rglob('*')):
        if 'target' in path.relative_to(root).parts:
            continue
        if path.is_file() and path.name.lower().startswith((
                'license', 'licence', 'copying', 'copyright', 'notice', 'authors')):
            prefix = 'registry:' if package['source'] else 'vendor/' + package['name'] + '/'
            row['notices'].append({'source': prefix + str(path.relative_to(root)),
                                   'sha256': add(path.read_bytes())})
    for upstream in recovered:
        if (upstream['name'], upstream['version']) == (package['name'], package['version']):
            for file in upstream['files']:
                evidence = base / file['local_path'] if 'local_path' in file else review / 'upstream' / (file['sha256'] + '.txt')
                data = evidence.read_bytes()
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
    if not row['notices']:
        row['review_required'] = 'Exact original notice text absent from the current evidence; see current licensing closeout.'
    rows.append(row)

(output_directory / 'third-party-index.json').write_text(json.dumps(rows, indent=2) + '\n')
header = '''PROPOSED THIRD-PARTY LICENSE EVIDENCE — NOT A PROJECT LICENSE GRANT

All resolved third-party packages are indexed in third-party-index.json, including
inactive optional/target dependencies. Exact original texts are deduplicated by
SHA-256. Alternate license texts are retained as evidence, not selected together.
See docs/consolidation/source-handoff.md for the current source scope and exceptions.
The source includes modified hdf5-reader and zarrs_filesystem crates with original
notices. This evidence bundle does not apply a license to Tile-Ripper.

'''
with (output_directory / 'THIRD-PARTY-LICENSES.txt').open('w') as output:
    output.write(header)
    for digest, text in sorted(texts.items()):
        output.write('\n===== SHA256 ' + digest + ' =====\n' + text + '\n')
print(f'{len(rows)} packages; {len(texts)} unique exact notice texts')
