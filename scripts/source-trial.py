#!/usr/bin/env python3
"""Install, smoke-test and uninstall from a clean tracked-source archive."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--toolchain', default='1.92.0')
    args = parser.parse_args()
    if subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT):
        parser.error('commit the candidate before creating its clean source trial')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    source, prefix = output / 'source', output / 'install'
    report = {'status': 'running', 'revision': revision, 'toolchain': args.toolchain,
              'human_trial': False, 'commands': []}
    cargo = ['cargo', '+' + args.toolchain]

    def run(name, command, cwd=ROOT):
        print(name, flush=True)
        started = time.monotonic()
        with (output / (name + '.log')).open('w') as log:
            result = subprocess.run(command, cwd=cwd, stdout=log, stderr=subprocess.STDOUT)
        report['commands'].append({'name': name, 'command': command, 'exit_code': result.returncode,
                                   'seconds': time.monotonic() - started})
        if result.returncode:
            raise RuntimeError(name + ' failed')

    try:
        archive = output / 'source.tar'
        run('archive', ['git', 'archive', '--format=tar', '--output', str(archive), revision])
        report['archive_sha256'] = hashlib.sha256(archive.read_bytes()).hexdigest()
        source.mkdir()
        with tarfile.open(archive) as snapshot:
            snapshot.extractall(source, filter='data')
        assert not (source / 'prototype-output').exists() and not (source / 'target').exists()
        run('install', cargo + ['install', '--path', '.', '--locked', '--offline', '--jobs', '2',
                                '--no-default-features', '--features', 'native-cog,native-hdf5,authenticated-http',
                                '--root', str(prefix), '--target-dir', str(output / 'build')], source)
        binary = prefix / 'bin/tile-ripper'
        report['binary_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
        run('help', [str(binary), '--help'], source)
        run('list', cargo + ['install', '--list', '--root', str(prefix)], source)
        run('environment', ['rustup', 'run', args.toolchain, 'sh', 'scripts/check-environment.sh', '--native', str(binary)], source)
        run('smoke', [sys.executable, str(source / 'scripts/trial-smoke.py'), '--binary', str(binary),
                       '--output', str(output / 'live-smoke')], source)
        report['live_smoke'] = json.loads((output / 'live-smoke/report.json').read_text())
        if report['live_smoke']['status'] != 'passed':
            raise RuntimeError('live smoke failed')
        report['status'] = 'passed'
    except Exception as error:
        report['status'], report['error'] = 'failed', str(error)
    finally:
        if (prefix / 'bin/tile-ripper').exists():
            try:
                run('uninstall', cargo + ['uninstall', '--root', str(prefix), 'tile-ripper'])
                report['uninstalled'] = not (prefix / 'bin/tile-ripper').exists()
            except Exception as error:
                report['status'], report['error'] = 'failed', str(error)
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    sys.exit(main())
