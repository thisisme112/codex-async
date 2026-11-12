#!/usr/bin/env bash
set -euo pipefail
umask 022
SOURCE_DIR=$(cd -- "$(dirname -- "$0")" && pwd)
PREFIX=${1:-"$HOME/.local"}
[[ $# -le 1 ]] || { echo 'Usage: ./install.sh [prefix]' >&2; exit 1; }
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'This package requires Linux x86_64.' >&2; exit 1; }
cd "$SOURCE_DIR"
sha256sum --check --quiet PACKAGE_SHA256SUMS
VERSION=$(cat VERSION)
case "$VERSION" in ''|*[!A-Za-z0-9.+_-]*) echo 'Invalid package version' >&2; exit 1 ;; esac
BUILD_ID=$VERSION
if [[ -f BUILD_ID ]]; then BUILD_ID=$(cat BUILD_ID); fi
case "$BUILD_ID" in ''|*[!A-Za-z0-9.+_-]*) echo 'Invalid package build ID' >&2; exit 1 ;; esac
for executable in bin/codex bin/codex-code-mode-host bin/codex-exec-server codex-resources/bwrap; do
  dependencies=$(ldd "$executable" 2>&1 || true)
  if [[ "$dependencies" == *'not found'* ]]; then
    echo "Missing system libraries for $executable:" >&2
    echo "$dependencies" >&2
    echo 'Ubuntu/Debian: install libc6, libstdc++6, libssl3 and libcap2.' >&2
    exit 1
  fi
done
mkdir -p "$PREFIX/bin" "$PREFIX/share/codex-async/releases"
PREFIX=$(cd "$PREFIX" && pwd)
case "$PREFIX/" in "$SOURCE_DIR/"*) echo 'Choose an installation prefix outside the unpacked package.' >&2; exit 1 ;; esac
DESTINATION="$PREFIX/share/codex-async/releases/$BUILD_ID"
LAUNCHER="$PREFIX/bin/codex-async"
if [[ -e "$LAUNCHER" && ! -L "$LAUNCHER" ]]; then
  echo "$LAUNCHER already exists and is not a managed symlink." >&2
  exit 1
fi
if [[ -L "$LAUNCHER" ]]; then
  case "$(readlink "$LAUNCHER")" in
    "$PREFIX/share/codex-async/releases/"*/bin/codex) ;;
    *) echo "$LAUNCHER points outside this installation; refusing to overwrite it." >&2; exit 1 ;;
  esac
fi
if [[ -d "$DESTINATION" ]]; then
  (cd "$DESTINATION" && sha256sum --check --quiet PACKAGE_SHA256SUMS)
  cmp --silent PACKAGE_SHA256SUMS "$DESTINATION/PACKAGE_SHA256SUMS"
else
  STAGING=$(mktemp -d "$PREFIX/share/codex-async/releases/.install.XXXXXXXX")
  trap 'rm -rf -- "$STAGING"' EXIT
  cp -a "$SOURCE_DIR/." "$STAGING/"
  mv -T -- "$STAGING" "$DESTINATION"
  trap - EXIT
fi
ln -sfn -- "$DESTINATION/bin/codex" "$LAUNCHER"
"$LAUNCHER" --version
printf 'Installed %s\nRun: %s\n' "$VERSION" "$LAUNCHER"
