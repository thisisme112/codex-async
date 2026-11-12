#!/usr/bin/env bash
set -euo pipefail
REPO_ROOT=$(git rev-parse --show-toplevel)
export CODEX_REPO_ROOT="$REPO_ROOT"
export STABLE_GIT_COMMIT
STABLE_GIT_COMMIT=$(git rev-parse HEAD)
BUILD_JOBS=${ASYNC_BUILD_JOBS:-1}
case "$BUILD_JOBS" in 1|2) ;; *) echo 'ASYNC_BUILD_JOBS must be 1 or 2' >&2; exit 1 ;; esac
SUITE=${ASYNC_VALIDATION_SUITE:-all}
case "$SUITE" in all|runtime|core|cli|async) ;; *) echo 'Unknown ASYNC_VALIDATION_SUITE' >&2; exit 1 ;; esac
echo "Validating suite=$SUITE with build_jobs=$BUILD_JOBS"
export CARGO_BUILD_JOBS="$BUILD_JOBS" CARGO_INCREMENTAL=0 RAYON_NUM_THREADS="$BUILD_JOBS" CMAKE_BUILD_PARALLEL_LEVEL="$BUILD_JOBS"
export CARGO_TARGET_DIR="$REPO_ROOT/codex-rs/target/async-ci"
if [[ "$SUITE" == all || "$SUITE" == cli ]]; then
  cd "$REPO_ROOT"
  export ASYNC_UPSTREAM_VERSION_FILE=${ASYNC_UPSTREAM_VERSION_FILE:-"$RUNNER_TEMP/async-official-version.json"}
  if [[ ! -f "$ASYNC_UPSTREAM_VERSION_FILE" ]]; then
    python3 scripts/async_fork/upstream_version.py resolve --file "$ASYNC_UPSTREAM_VERSION_FILE"
  fi
  python3 scripts/async_fork/upstream_version.py stamp --file "$ASYNC_UPSTREAM_VERSION_FILE"
fi

# Rust can spend several minutes on one crate without printing a new line.
# Keep resource telemetry in the same durable log without making model requests.
python3 -u - "$REPO_ROOT" <<'PY' &
import datetime
import shutil
import sys
import time
from pathlib import Path

while True:
    memory = dict(line.split(':', 1) for line in Path('/proc/meminfo').read_text().splitlines())
    available_mib = int(memory['MemAvailable'].split()[0]) // 1024
    free_mib = shutil.disk_usage(sys.argv[1]).free // (1024 * 1024)
    timestamp = datetime.datetime.now(datetime.UTC).isoformat(timespec='seconds')
    print(f'[build heartbeat] {timestamp} available_memory={available_mib}MiB free_disk={free_mib}MiB', flush=True)
    time.sleep(60)
PY
MONITOR_PID=$!
trap 'kill "$MONITOR_PID" 2>/dev/null || true; wait "$MONITOR_PID" 2>/dev/null || true' EXIT

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
# The dependency cache can restore V8's build metadata without its generated
# native archive. Re-run just that build script against the verified archive.
cargo clean --profile dev-small -p v8
cargo build --locked --profile dev-small -p codex-bwrap -p codex-code-mode-host --bins
mkdir -p "$CARGO_TARGET_DIR/dev-small/codex-resources"
cp "$CARGO_TARGET_DIR/dev-small/bwrap" "$CARGO_TARGET_DIR/dev-small/codex-resources/bwrap"
export CODEX_BWRAP_SHA256
CODEX_BWRAP_SHA256=$(sha256sum "$CARGO_TARGET_DIR/dev-small/bwrap" | cut -d ' ' -f1)
if [[ "$SUITE" == all || "$SUITE" == runtime ]]; then
  just test -p codex-code-mode-protocol -p codex-code-mode -p codex-code-mode-host --lib --cargo-profile dev-small -j 1
  just test -p codex-code-mode-runtime --lib --cargo-profile dev-small -j 1 -E 'test(session_runtime) | test(cell_actor)'
  just test -p codex-code-mode-host --test completion_ready --cargo-profile dev-small -j 1
fi
if [[ "$SUITE" == all || "$SUITE" == core ]]; then
  just test -p codex-core --lib --cargo-profile dev-small -j 1 -E 'test(tools::code_mode) | test(tools::context) | test(tools::handlers::unified_exec::tests) | test(tools::handlers::shell_spec::tests) | test(tools::network_approval) | test(unified_exec::) | test(session::completion_withdrawal)'
fi
if [[ "$SUITE" == all || "$SUITE" == core || "$SUITE" == async ]]; then
  just test -p codex-core --test async_completion --cargo-profile dev-small -j 1
fi
if [[ "$SUITE" == all || "$SUITE" == cli ]]; then
  just test -p codex-tui --lib --cargo-profile dev-small -j 1 -E 'test(code_mode_startup)'
  cargo build --locked --profile dev-small -p codex-cli --bin codex
  "$CARGO_TARGET_DIR/dev-small/codex" --version
  "$CARGO_TARGET_DIR/dev-small/codex" --help >/dev/null
  cargo build --locked --profile dev-small -p codex-exec-server-cli --bin codex-exec-server
  ASYNC_PACKAGE_PREBUILT=1 bash "$REPO_ROOT/scripts/async_fork/package.sh"
fi
