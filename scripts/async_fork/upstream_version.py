"""Use the newest official release whose source is included in the synced commit."""

import argparse
import json
import os
import re
import subprocess
import tomllib
from pathlib import Path
from urllib.parse import quote

VERSION = re.compile(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?')


def version_key(version):
    match = VERSION.fullmatch(version)
    if not match:
        raise ValueError(f'Invalid upstream version: {version}')
    pre = match[4]
    parts = tuple((0, int(part)) if part.isdigit() else (1, part) for part in (pre or '').split('.'))
    return (*map(int, match.group(1, 2, 3)), pre is None, parts)


def gh_json(*args):
    return json.loads(subprocess.check_output(['gh', *args], text=True))


def resolve(upstream, releases, api):
    candidates = [r['tagName'] for r in releases if not r['isDraft'] and r['tagName'].startswith('rust-v') and VERSION.fullmatch(r['tagName'][6:])]
    for tag in sorted(candidates, key=lambda value: version_key(value[6:]), reverse=True):
        version = tag[6:]
        obj = api(f'repos/openai/codex/git/ref/tags/{quote(tag, safe="")}')['object']
        for _ in range(4):
            if obj['type'] != 'tag':
                break
            obj = api(f'repos/openai/codex/git/tags/{obj["sha"]}')['object']
        if obj['type'] != 'commit':
            continue
        commit = api(f'repos/openai/codex/commits/{obj["sha"]}')
        anchor = obj['sha']
        files = commit.get('files', [])
        # Official release tags use a version-only commit on top of main.
        # Compare its actual source parent, not the off-main version stamp.
        if len(files) == 1 and files[0]['filename'] == 'codex-rs/Cargo.toml' and len(commit['parents']) == 1:
            edits = [line for line in files[0].get('patch', '').splitlines() if line.startswith(('+', '-')) and not line.startswith(('+++', '---'))]
            if edits == ['-version = "0.0.0"', f'+version = "{version}"']:
                anchor = commit['parents'][0]['sha']
        # Files are returned only on comparison page one; page two gives the
        # same ancestry status without downloading an unnecessary source diff.
        comparison = api(f'repos/openai/codex/compare/{anchor}...{upstream}?per_page=1&page=2')
        if comparison['status'] in ('ahead', 'identical'):
            return {'version': version, 'official_tag': tag, 'release_commit': obj['sha'], 'release_source': anchor, 'upstream': upstream}
    raise RuntimeError('No official release source is included in the synced upstream commit')


def load_record(repo, path):
    record = json.loads(path.read_text())
    version_key(record['version'])
    state = json.loads((repo / '.github/async-fork-state.json').read_text())
    if record['upstream'] != state['upstream'] or record['official_tag'] != 'rust-v' + record['version']:
        raise ValueError('Official version record does not match the synced upstream')
    return record


def stamp_workspace(repo, record):
    """Stamp build copies only; preserve every external dependency and checksum."""
    manifest = repo / 'codex-rs/Cargo.toml'
    original = manifest.read_text()
    workspace = tomllib.loads(original)['workspace']
    old, new = workspace['package']['version'], record['version']
    if old not in ('0.0.0', new):
        raise ValueError(f'Workspace version {old} conflicts with official version {new}')
    names = set()
    root = manifest.parent.resolve()
    manifests = {root: tomllib.loads(original)}
    for folder, directories, files in os.walk(root):
        directories[:] = [name for name in directories if name not in ('target', '.git')]
        if 'Cargo.toml' not in files:
            continue
        directory = Path(folder)
        if directory not in manifests:
            manifests[directory] = tomllib.loads((directory / 'Cargo.toml').read_text())
        data = manifests[directory]
        package = data.get('package', {})
        if package.get('version') != {'workspace': True}:
            continue
        # Cargo also adds path dependencies as implicit workspace members.
        # Find the owning workspace instead of relying on workspace.members.
        owner = None
        if 'workspace' in package:
            owner = (directory / package['workspace']).resolve()
        else:
            for parent in (directory, *directory.parents):
                if not parent.is_relative_to(root):
                    break
                candidate = parent / 'Cargo.toml'
                if candidate.is_file():
                    if parent not in manifests:
                        manifests[parent] = tomllib.loads(candidate.read_text())
                    parent_data = manifests[parent]
                    if 'workspace' in parent_data:
                        owner = parent
                        break
        if owner == root:
            names.add(package['name'])
    lock_path = manifest.with_name('Cargo.lock')
    chunks = re.split(r'(?m)(?=^\[\[package\]\]$)', lock_path.read_text())
    for index, chunk in enumerate(chunks):
        if not chunk.startswith('[[package]]'):
            continue
        package = tomllib.loads(chunk)['package'][0]
        if 'source' not in package and package['name'] in names:
            if package['version'] not in (old, new):
                raise ValueError('Workspace lockfile version does not match its manifest')
            chunks[index] = re.sub(
                r'(?m)^(version\s*=\s*")' + re.escape(package['version']) + r'("[^\n]*)$',
                lambda match: match[1] + new + match[2], chunk, count=1,
            )
    lock = ''.join(chunks)
    for name in names:
        lock = lock.replace(f'"{name} {old}"', f'"{name} {new}"')
    changed, count = re.subn(r'(?ms)(^\[workspace\.package\]\n.*?^version\s*=\s*")[^"]+("[^\n]*)', lambda m: m[1] + new + m[2], original, count=1)
    if count != 1:
        raise ValueError('Could not locate the workspace version stamp')
    tomllib.loads(changed)
    tomllib.loads(lock)
    manifest.write_text(changed)
    lock_path.write_text(lock)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('resolve', 'stamp'))
    parser.add_argument('--file', type=Path, required=True)
    args = parser.parse_args()
    repo = Path.cwd()
    if args.command == 'resolve':
        state = json.loads((repo / '.github/async-fork-state.json').read_text())
        releases = gh_json('release', 'list', '--repo', 'openai/codex', '--limit', '100', '--json', 'tagName,isDraft')
        record = resolve(state['upstream'], releases, lambda path: gh_json('api', path))
        args.file.write_text(json.dumps(record, indent=2) + '\n')
    else:
        record = load_record(repo, args.file)
        stamp_workspace(repo, record)
    print(f'Official upstream version: {record["version"]} ({record["official_tag"]})')


if __name__ == '__main__':
    main()
