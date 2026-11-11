#!/usr/bin/env bash
set -euo pipefail
REPO_ROOT=$(git rev-parse --show-toplevel)
export CODEX_REPO_ROOT="$REPO_ROOT"
export STABLE_GIT_COMMIT
STABLE_GIT_COMMIT=$(git rev-parse HEAD)
export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 RAYON_NUM_THREADS=1 CMAKE_BUILD_PARALLEL_LEVEL=1
export CARGO_TARGET_DIR="$REPO_ROOT/target/async-ci"
TOOLS="$RUNNER_TEMP/async-ci-tools"
mkdir -p "$TOOLS"
export PATH="$TOOLS:$PATH"

# Official release assets, pinned with their published SHA256 digests.
python3 - "$TOOLS" <<'PY'
import hashlib
import io
import sys
import tarfile
import urllib.request
from pathlib import Path

destination = Path(sys.argv[1])
assets = [
    ('just', 'https://github.com/casey/just/releases/download/1.59.0/just-1.59.0-x86_64-unknown-linux-musl.tar.gz', '409044617b6f06517c1035e55ca38c8c856ff8b650e41bc2aa1ef82d5903a23e'),
    ('cargo-nextest', 'https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.151/cargo-nextest-0.9.151-x86_64-unknown-linux-musl.tar.gz', '6b284d6fde9670dfb6ee10ea9d9abbc85f95643d3f592472d420d82e257b3a6c'),
]
for name, url, expected in assets:
    content = urllib.request.urlopen(url, timeout=120).read()
    if hashlib.sha256(content).hexdigest() != expected:
        raise RuntimeError(f'Checksum mismatch: {name}')
    with tarfile.open(fileobj=io.BytesIO(content)) as archive:
        member = next(item for item in archive if item.isfile() and Path(item.name).name == name)
        target = destination / name
        target.write_bytes(archive.extractfile(member).read())
        target.chmod(0o755)
PY

# Use the candidate's own locked V8 version and upstream checksum verifier.
python3 - "$REPO_ROOT" "$RUNNER_TEMP/async-v8.env" <<'PY'
import sys
from pathlib import Path

sys.path.insert(0, str(Path(sys.argv[1]) / 'scripts'))
from codex_package.targets import TARGET_SPECS
from codex_package.v8 import resolve_codex_v8_cargo_env

values = resolve_codex_v8_cargo_env(TARGET_SPECS['x86_64-unknown-linux-gnu'])
Path(sys.argv[2]).write_text(''.join(f'{key}={value}\n' for key, value in values.items()))
PY
set -a
source "$RUNNER_TEMP/async-v8.env"
set +a

cd "$REPO_ROOT/codex-rs"
rustup show active-toolchain
cargo build --locked --profile dev-small -p codex-bwrap -p codex-code-mode-host --bins
mkdir -p "$CARGO_TARGET_DIR/dev-small/codex-resources"
cp "$CARGO_TARGET_DIR/dev-small/bwrap" "$CARGO_TARGET_DIR/dev-small/codex-resources/bwrap"
export CODEX_BWRAP_SHA256
CODEX_BWRAP_SHA256=$(sha256sum "$CARGO_TARGET_DIR/dev-small/bwrap" | cut -d ' ' -f1)
just test -p codex-core --lib --cargo-profile dev-small -j 1 -E 'test(tools::code_mode) | test(tools::context) | test(tools::handlers::unified_exec::tests) | test(tools::handlers::shell_spec::tests)'
just test -p codex-core --test async_completion --cargo-profile dev-small -j 1
cargo build --locked --profile dev-small -p codex-cli --bin codex
"$CARGO_TARGET_DIR/dev-small/codex" --version
"$CARGO_TARGET_DIR/dev-small/codex" --help >/dev/null
