"""Use the pinned V8 source without fetching unused Chromium submodules."""
import argparse
import re
import subprocess
import tomllib
from pathlib import Path


def run(*args):
    return subprocess.run(args, check=True, text=True, capture_output=True).stdout.strip()


def prepare(workspace, checkout):
    manifest_path = workspace / 'Cargo.toml'
    lock_path = workspace / 'Cargo.lock'
    manifest = manifest_path.read_bytes()
    lock = lock_path.read_bytes()
    config = tomllib.loads(manifest.decode('utf-8'))['patch']['crates-io']['v8']
    url = config['git']
    revision = config['rev']
    if url != 'https://github.com/openai-oss-forks/rusty_v8.git' or not re.fullmatch(r'[0-9a-f]{40}', revision):
        raise RuntimeError('Unexpected V8 source; manual adaptation required')
    entries = [p for p in tomllib.loads(lock.decode('utf-8'))['package'] if p['name'] == 'v8']
    if len(entries) != 1:
        raise RuntimeError('Expected exactly one locked V8 package')
    expected_source = f'git+{url}?rev={revision}#{revision}'
    if entries[0].get('source') != expected_source:
        raise RuntimeError('Locked V8 source does not match the manifest')
    if not checkout.exists():
        run('git', 'init', str(checkout))
        run('git', '-C', str(checkout), 'remote', 'add', 'origin', url)
        run('git', '-C', str(checkout), 'fetch', '--no-tags', '--depth=1', 'origin', revision)
        run('git', '-C', str(checkout), 'checkout', '--detach', 'FETCH_HEAD')
    if run('git', '-C', str(checkout), 'rev-parse', 'HEAD') != revision:
        raise RuntimeError('V8 checkout does not match the pinned commit')
    source_version = tomllib.loads((checkout / 'Cargo.toml').read_text(encoding='utf-8'))['package']['version']
    if source_version != entries[0]['version']:
        raise RuntimeError('V8 checkout does not match the locked version')
    manifest_line = f'v8 = {{ git = "{url}", rev = "{revision}" }}'.encode()
    lock_line = f'source = "{expected_source}"'.encode()
    if manifest.count(manifest_line) != 1 or lock.count(lock_line) != 1:
        raise RuntimeError('V8 source lines are missing or ambiguous')
    newline = b'\r\n' if lock_line + b'\r\n' in lock else b'\n'
    if lock_line + newline not in lock:
        raise RuntimeError('V8 lock line has no expected terminator')
    # Keep all other locked packages untouched. The local source is the same Git
    # commit and the CI resolver supplies its separately verified native archive.
    local_line = f'v8 = {{ path = "{checkout.as_posix()}" }}'.encode()
    manifest_path.write_bytes(manifest.replace(manifest_line, local_line, 1))
    lock_path.write_bytes(lock.replace(lock_line + newline, b'', 1))
    print(f'Pinned V8 source: {revision}; submodules not required for prebuilt V8')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--checkout', type=Path, required=True)
    args = parser.parse_args()
    prepare(args.workspace.resolve(), args.checkout.resolve())
