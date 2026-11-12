"""Reuse an existing release when all package/build inputs are unchanged."""

import argparse
import json
import os
import subprocess
from pathlib import Path

from upstream_version import load_record

# Workflow/cache settings and prose do not change the delivered package.
# Keep installer, native setup, metadata and all actual build inputs here.
PACKAGE_INPUTS = (
    'codex-rs',
    'scripts/codex_package',
    'scripts/build_codex_package.py',
    'scripts/async_fork/package.sh',
    'scripts/async_fork/upstream_version.py',
    'scripts/async_fork/install-package.sh',
    'scripts/async_fork/native_deps.py',
    'third_party/v8',
    'MODULE.bazel',
    'MODULE.bazel.lock',
    '.github/async-fork-state.json',
    'LICENSE',
    'NOTICE',
)


def git(repo, *args):
    return subprocess.check_output(
        ['git', *args], cwd=repo, text=True, stderr=subprocess.PIPE
    ).strip()


def fingerprint(repo, ref):
    return tuple(git(repo, 'rev-parse', f'{ref}:{path}') for path in PACKAGE_INPUTS)


def matching_release(repo, releases, fetch=True, version=None):
    current = fingerprint(repo, 'HEAD')
    for release in [item for item in releases if not item['isDraft']][:3]:
        tag = release['tagName']
        if version is not None and not tag.startswith(f'rust-v{version}+async.'):
            continue
        if fetch:
            subprocess.run(
                ['git', 'fetch', '--no-tags', '--depth=1', 'origin', f'refs/tags/{tag}'],
                cwd=repo, check=True,
            )
            ref = 'FETCH_HEAD'
        else:
            ref = f'refs/tags/{tag}'
        try:
            same_inputs = fingerprint(repo, ref) == current
        except subprocess.CalledProcessError:
            # Old releases can predate one of the required package inputs.
            continue
        if same_inputs:
            return tag
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('release_list', type=Path)
    parser.add_argument('--force', action='store_true')
    parser.add_argument('--version-file', type=Path, required=True)
    args = parser.parse_args()
    repo = Path.cwd()
    sha = git(repo, 'rev-parse', 'HEAD')
    releases = json.loads(args.release_list.read_text())
    record = load_record(repo, args.version_file)
    version = record['version']
    exact_tag = f'rust-v{version}+async.{sha[:12]}'
    existing = next((item['tagName'] for item in releases if not item['isDraft'] and item['tagName'] == exact_tag), None)
    existing = existing or (None if args.force else matching_release(repo, releases, version=version))
    tag = existing or exact_tag
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        output.write(f'sha={sha}\ntag={tag}\nversion={version}\nversion_record={json.dumps(record)}\nbuild={str(existing is None).lower()}\n')
    message = f'Reusing published package {tag}: package inputs are unchanged.' if existing else f'Building package from {sha}.'
    print(message)
    if summary := os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(summary, 'a') as output:
            output.write(message + '\n')


if __name__ == '__main__':
    main()
