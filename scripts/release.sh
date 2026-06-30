#!/usr/bin/env bash
# Build release archives for the current platform.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_DIR"

TARGET="${1:-$(rustc -vV | awk '/host/ { print $2 }')}"
VERSION="$(cargo pkgid | cut -d# -f2 | cut -d@ -f2)"

echo "Building step-cli v${VERSION} for ${TARGET}..."
cargo build --release --target "$TARGET"

mkdir -p dist

case "$OSTYPE" in
  msys*|cygwin*|win32*)
    BIN="target/${TARGET}/release/step.exe"
    ARCHIVE="dist/step-${VERSION}-${TARGET}.zip"
    7z a "$ARCHIVE" "$BIN" README.md LICENSE 2>/dev/null || zip -j "$ARCHIVE" "$BIN" README.md LICENSE
    ;;
  *)
    BIN="target/${TARGET}/release/step"
    ARCHIVE="dist/step-${VERSION}-${TARGET}.tar.gz"
    tar czf "$ARCHIVE" -C "target/${TARGET}/release" step -C "$PROJECT_DIR" README.md LICENSE 2>/dev/null || \
      tar czf "$ARCHIVE" -C "target/${TARGET}/release" step
    ;;
esac

echo "Created ${ARCHIVE}"
