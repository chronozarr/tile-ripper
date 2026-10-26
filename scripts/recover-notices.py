#!/usr/bin/env python3
"""Recover absent packaged notices from the package's recorded GitHub revision."""
import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import hashlib
import json
from pathlib import Path, PurePosixPath
import subprocess
from urllib.request import Request, urlopen
from urllib.error import HTTPError

ROOT = Path(__file__).resolve().parents[1]


def get(url):
    try:
        with urlopen(Request(url, headers={"User-Agent": "tile-ripper-source-audit"}), timeout=30) as response:
            return response.read()
    except HTTPError as error:
        raise RuntimeError(f'HTTP {error.code}: {url}') from error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--index", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    missing = {(r['name'], r['version']) for r in json.loads(args.index.read_text()) if not r['notices']}
    packages = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--offline', '--all-features', '--format-version', '1'], cwd=ROOT))['packages']
    groups = {}
    unresolved = []
    for p in packages:
        if (p['name'], p['version']) not in missing:
            continue
        root = Path(p['manifest_path']).parent
        vcs = json.loads((root / '.cargo_vcs_info.json').read_text())
        repository = p.get('repository') or ''
        if not repository.startswith('https://github.com/'):
            unresolved.append({'name': p['name'], 'version': p['version'], 'reason': 'not a GitHub source; prior exact-revision exception retained'})
            continue
        slug = '/'.join(repository[len('https://github.com/'):].split('/')[:2]).removesuffix('.git')
        groups.setdefault((slug, vcs['git']['sha1']), []).append((p, root, vcs.get('path_in_vcs', '')))

    def recover(group):
        (slug, revision), entries = group
        tree = json.loads(get(f'https://api.github.com/repos/{slug}/git/trees/{revision}?recursive=1'))
        if tree.get('truncated'):
            raise RuntimeError('truncated upstream tree: ' + slug)
        rows = []
        for package, root, prefix in entries:
            allowed = {str(PurePosixPath(prefix)), '.'}
            allowed.update(str(p) for p in PurePosixPath(prefix).parents)
            files = []
            for item in tree['tree']:
                path = PurePosixPath(item['path'])
                if item['type'] != 'blob' or str(path.parent) not in allowed or not path.name.lower().startswith(('license', 'licence', 'notice', 'copyright', 'authors')):
                    continue
                url = f'https://raw.githubusercontent.com/{slug}/{revision}/{path}'
                data = get(url)
                digest = hashlib.sha256(data).hexdigest()
                (args.output / (digest + '.txt')).write_bytes(data)
                files.append({'url': url, 'sha256': digest,
                              'local_path': str((args.output / (digest + '.txt')).relative_to(ROOT))})
            source_path = str(PurePosixPath(prefix) / 'src/lib.rs')
            original = get(f'https://raw.githubusercontent.com/{slug}/{revision}/{source_path}')
            matches = original == (root / 'src/lib.rs').read_bytes()
            if not matches:
                raise RuntimeError('upstream source mismatch: ' + package['name'])
            rows.append({'name': package['name'], 'version': package['version'], 'repository': slug,
                         'revision': revision, 'source_match': source_path, 'files': files})
        return rows

    rows = []
    with ThreadPoolExecutor(max_workers=4) as pool:
        futures = {pool.submit(recover, group): group for group in groups.items()}
        for future in as_completed(futures):
            try:
                rows.extend(future.result())
            except Exception as error:
                (_, revision), entries = futures[future]
                unresolved.extend({'name': p['name'], 'version': p['version'],
                                   'revision': revision, 'reason': str(error)} for p, _, _ in entries)
    (args.output / 'recovered-notices.json').write_text(json.dumps(sorted(rows, key=lambda r: r['name']), indent=2) + '\n')
    unresolved.extend({'name': row['name'], 'version': row['version'], 'revision': row['revision'],
                       'reason': 'source matched, but no applicable original notice file was present at this revision'}
                      for row in rows if not row['files'])
    (args.output / 'unresolved.json').write_text(json.dumps(unresolved, indent=2) + '\n')
    print(json.dumps({'packages_with_recovered_notices': sum(bool(row['files']) for row in rows),
                      'source_verified_packages': len(rows), 'unresolved': unresolved}))


if __name__ == '__main__':
    main()
