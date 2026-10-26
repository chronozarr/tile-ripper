#!/usr/bin/env python3
"""Create and verify a private local Git and sparse-science recovery snapshot."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if subprocess.run(['git', 'diff', '--quiet', 'HEAD'], cwd=ROOT).returncode:
        parser.error('commit tracked changes before taking a source recovery snapshot')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    bundle = output / 'history.bundle'
    subprocess.run(['git', 'bundle', 'create', str(bundle), '--all'], cwd=ROOT, check=True)
    subprocess.run(['git', 'bundle', 'verify', str(bundle)], cwd=ROOT, check=True)
    with tempfile.TemporaryDirectory(prefix='tile-ripper-bundle-check-') as tmp:
        clone = Path(tmp) / 'restored'
        subprocess.run(['git', 'clone', '--quiet', '--no-checkout', str(bundle), str(clone)], check=True)
        subprocess.run(['git', 'cat-file', '-e', revision + '^{commit}'], cwd=clone, check=True)
    snapshot = output / ('internal-source-' + revision[:12] + '.tar')
    subprocess.run(['git', 'archive', '--format=tar', '--output', str(snapshot), revision], cwd=ROOT, check=True)
    mirrors = []
    archive_path = output / 'scientific-extents.tar.gz'
    with tarfile.open(archive_path, 'w:gz') as archive:
        def add(name, data):
            info = tarfile.TarInfo(name)
            info.size, info.mode = len(data), 0o600
            archive.addfile(info, io.BytesIO(data))
        for wave in ('wave3', 'wave4'):
            base = ROOT / 'prototype-output' / wave
            for path in sorted(base.rglob('manifest.json')):
                source_path = path.parent / 'source.nc'
                if not source_path.is_file():
                    continue
                manifest = json.loads(path.read_text())
                relative = str(path.parent.relative_to(ROOT / 'prototype-output'))
                extents = []
                expected_hashes = {(a, b): h for a, b, h in manifest['extent_sha256']}
                with source_path.open('rb') as source:
                    for a, b in manifest['extents']:
                        source.seek(a)
                        data = source.read(b - a)
                        expected = expected_hashes[(a, b)]
                        assert len(data) == b - a and hashlib.sha256(data).hexdigest() == expected
                        member = relative + f'/extents/{a}-{b}.bin'
                        add(member, data)
                        extents.append({'start': a, 'end': b, 'member': member, 'sha256': expected})
                add(relative + '/manifest.json', path.read_bytes())
                mirrors.append({'path': relative, 'size': manifest['size'], 'extents': extents})
            # Adjacent original inspections/reference metadata are needed by replay.
            for path in sorted(base.rglob('*.json')):
                if path.name == 'manifest.json' or any(p.endswith('.zarr') for p in path.parts):
                    continue
                add(str(path.relative_to(ROOT / 'prototype-output')), path.read_bytes())
        add('mirrors.json', (json.dumps(mirrors, indent=2) + '\n').encode())
    # Verify by rebuilding temporary sparse files and hashing every backed-up extent.
    verified_bytes = 0
    with tarfile.open(archive_path) as archive, tempfile.TemporaryDirectory(prefix='tile-ripper-recovery-check-') as tmp:
        for index, mirror in enumerate(mirrors):
            restored = Path(tmp) / str(index)
            with restored.open('wb') as dest:
                dest.truncate(mirror['size'])
                for extent in mirror['extents']:
                    data = archive.extractfile(extent['member']).read()
                    assert hashlib.sha256(data).hexdigest() == extent['sha256']
                    dest.seek(extent['start'])
                    dest.write(data)
            with restored.open('rb') as source:
                for extent in mirror['extents']:
                    source.seek(extent['start'])
                    data = source.read(extent['end'] - extent['start'])
                    assert hashlib.sha256(data).hexdigest() == extent['sha256']
                    verified_bytes += len(data)
    report = {'status': 'passed', 'revision': revision, 'private_local_recovery': True,
              'mirrors': len(mirrors), 'verified_extent_bytes': verified_bytes,
              'files': [{'name': p.name, 'bytes': p.stat().st_size, 'sha256': digest(p)}
                        for p in (bundle, snapshot, archive_path)]}
    (output / 'recovery.json').write_text(json.dumps(report, indent=2) + '\n')
    (output / 'RESTORE.md').write_text(
        '# Private local recovery\n\n'
        'Clone history.bundle into a new directory. The internal source tar contains tracked files only.\n'
        'scientific-extents.tar.gz stores bounded captured bytes, not complete provider objects.\n'
        'For each mirrors.json entry, create a sparse source.nc of the recorded size, write each\n'
        'extent at its start offset, verify its SHA-256, and restore the original manifest/inspection JSON.\n'
        'Use the guarded replay tools; never open an incomplete sparse file as a complete source.\n'
        'Keep this snapshot private. It supplies neither a project license nor an off-machine backup.\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
