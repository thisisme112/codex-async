#!/usr/bin/env bash
set -euo pipefail
REPO_ROOT=$(git rev-parse --show-toplevel)
export CODEX_REPO_ROOT="$REPO_ROOT"
export STABLE_GIT_COMMIT
STABLE_GIT_COMMIT=$(git rev-parse HEAD)
BUILD_JOBS=${ASYNC_BUILD_JOBS:-1}
case "$BUILD_JOBS" in 1|2) ;; *) echo 'ASYNC_BUILD_JOBS must be 1 or 2' >&2; exit 1 ;; esac
export CARGO_BUILD_JOBS="$BUILD_JOBS" CARGO_INCREMENTAL=0 RAYON_NUM_THREADS="$BUILD_JOBS" CMAKE_BUILD_PARALLEL_LEVEL="$BUILD_JOBS"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$REPO_ROOT/codex-rs/target/async-ci"}
OUTPUT=${ASYNC_PACKAGE_OUTPUT:-"$RUNNER_TEMP/async-package"}
PACKAGE="$RUNNER_TEMP/codex-async-package"
BINARIES="$CARGO_TARGET_DIR/dev-small"
VERSION_FILE=${ASYNC_UPSTREAM_VERSION_FILE:-"$RUNNER_TEMP/async-official-version.json"}
cd "$REPO_ROOT"
if [[ ! -f "$VERSION_FILE" ]]; then
  python3 scripts/async_fork/upstream_version.py resolve --file "$VERSION_FILE"
fi
python3 scripts/async_fork/upstream_version.py stamp --file "$VERSION_FILE"
VERSION=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$VERSION_FILE")
mkdir -p "$OUTPUT"
python3 -u - <<'PY' &
import datetime
import shutil
import time
from pathlib import Path
while True:
    memory = dict(line.split(':', 1) for line in Path('/proc/meminfo').read_text().splitlines())
    print(f'[package heartbeat] {datetime.datetime.now(datetime.UTC).isoformat(timespec="seconds")} available_memory={int(memory["MemAvailable"].split()[0]) // 1024}MiB free_disk={shutil.disk_usage(".").free // (1024 * 1024)}MiB', flush=True)
    time.sleep(60)
PY
MONITOR_PID=$!
trap 'kill "$MONITOR_PID" 2>/dev/null || true; wait "$MONITOR_PID" 2>/dev/null || true' EXIT
if [[ ${ASYNC_PACKAGE_PREBUILT:-0} != 1 ]]; then
  python3 - "$REPO_ROOT" "$RUNNER_TEMP/package-v8.env" <<'PY'
import sys
from pathlib import Path
sys.path.insert(0, str(Path(sys.argv[1]) / 'scripts'))
from codex_package.targets import TARGET_SPECS
from codex_package.v8 import resolve_codex_v8_cargo_env
values = resolve_codex_v8_cargo_env(TARGET_SPECS['x86_64-unknown-linux-gnu'])
Path(sys.argv[2]).write_text(''.join(f'{key}={value}\n' for key, value in values.items()))
PY
  set -a
  source "$RUNNER_TEMP/package-v8.env"
  set +a
  cd "$REPO_ROOT/codex-rs"
  cargo build --locked --profile dev-small -p codex-bwrap --bin bwrap
  export CODEX_BWRAP_SHA256
  CODEX_BWRAP_SHA256=$(sha256sum "$BINARIES/bwrap" | cut -d ' ' -f1)
  cargo build --locked --profile dev-small --bin codex --bin codex-code-mode-host --bin codex-exec-server
fi
cd "$REPO_ROOT"
python3 scripts/build_codex_package.py \
  --target x86_64-unknown-linux-gnu --package-version "$VERSION" \
  --package-dir "$PACKAGE" --entrypoint-bin "$BINARIES/codex" \
  --code-mode-host-bin "$BINARIES/codex-code-mode-host" \
  --exec-server-bin "$BINARIES/codex-exec-server" --bwrap-bin "$BINARIES/bwrap"
cp scripts/async_fork/install-package.sh "$PACKAGE/install.sh"
cp LICENSE NOTICE "$PACKAGE/"
printf '%s\n' "$VERSION" > "$PACKAGE/VERSION"
printf '%s-sha%s\n' "$VERSION" "${STABLE_GIT_COMMIT:0:12}" > "$PACKAGE/BUILD_ID"
cat > "$PACKAGE/INSTALL.txt" <<'TXT'
Codex Async preview — Linux x86_64

Unpack this entire archive, then run:
  bash ./install.sh
  ~/.local/bin/codex-async

No root access is needed. Your existing codex executable is preserved.
Requires glibc 2.35+ (Ubuntu 22.04+, Debian 12+) and system libssl3/libcap2.
Keep the whole installed directory: host, executor and sandbox are required.

Interactive CLI completion notifications and wait_for_tasks are enabled
automatically. No extra API service or system-prompt file is needed.
Requires a running CLI; notification state does not survive shutdown.
Uses the dev-small profile: full functionality, without release optimization.
Program and package versions follow the matching official release, including
alpha/beta suffixes. BUILD_ID and BUILD_INFO.json identify this fork build.
Source is available from the GitHub release tag.
TXT
python3 - "$PACKAGE" "$REPO_ROOT" "$VERSION" "$VERSION_FILE" <<'PY'
import json
import platform
import subprocess
import sys
from pathlib import Path
package, root = map(Path, sys.argv[1:3])
sha = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
state = json.loads((root / '.github/async-fork-state.json').read_text())
official = json.loads(Path(sys.argv[4]).read_text())
(package / 'BUILD_INFO.json').write_text(json.dumps({'version':sys.argv[3], 'build_id':(package/'BUILD_ID').read_text().strip(), 'source_commit':sha, 'official_upstream':state['upstream'], 'official_release':official, 'profile':'dev-small', 'target':'x86_64-unknown-linux-gnu', 'build_glibc':platform.libc_ver()[1]}, indent=2) + '\n')
PY
chmod +x "$PACKAGE/install.sh"
(cd "$PACKAGE" && find . -type f ! -name PACKAGE_SHA256SUMS -print0 | sort -z | xargs -0 sha256sum > PACKAGE_SHA256SUMS)
python3 - "$PACKAGE" "$OUTPUT/codex-async-linux-x86_64.tar.gz" <<'PY'
import sys
sys.path.insert(0, 'scripts')
from pathlib import Path
from codex_package.archive import write_archive
write_archive(Path(sys.argv[1]), Path(sys.argv[2]), force=False)
PY
# Check the delivered archive and installed paths, rather than build-tree paths.
SMOKE=$(mktemp -d "$RUNNER_TEMP/package-smoke.XXXXXXXX")
mkdir -p "$SMOKE/unpacked"
tar -xzf "$OUTPUT/codex-async-linux-x86_64.tar.gz" -C "$SMOKE/unpacked"
bash "$SMOKE/unpacked/install.sh" "$SMOKE/installed"
[[ $("$SMOKE/installed/bin/codex-async" --version) == "codex-cli $VERSION" ]]
timeout 30 "$SMOKE/installed/bin/codex-async" --help >/dev/null
timeout 30 "$SMOKE/installed/bin/codex-async" features list > "$OUTPUT/features.txt"
timeout 30 "$SMOKE/unpacked/bin/codex-code-mode-host" --help >/dev/null
timeout 30 "$SMOKE/unpacked/bin/codex-exec-server" --version
timeout 30 "$SMOKE/unpacked/codex-path/rg" --version
timeout 30 "$SMOKE/unpacked/codex-resources/bwrap" --version
cp "$PACKAGE/BUILD_INFO.json" "$PACKAGE/INSTALL.txt" "$OUTPUT/"
(cd "$OUTPUT" && sha256sum codex-async-linux-x86_64.tar.gz > SHA256SUMS)
echo "Package ready: $OUTPUT/codex-async-linux-x86_64.tar.gz"
