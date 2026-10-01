#!/bin/bash
set -euo pipefail

# Build native macOS .pkg installer for ratex.
# Usage: build-pkg.sh BUNDLE_DIR ARCH VERSION [OUTPUT_DIR]
# (stale defaults for the version or architecture would mislabel the package)
usage="usage: $0 BUNDLE_DIR ARCH VERSION [OUTPUT_DIR]"
BUNDLE_DIR="${1:?$usage}"
ARCH="${2:?$usage}"
VERSION="${3:?$usage}"
OUTPUT_DIR="${4:-dist}"

mkdir -p "$OUTPUT_DIR"

STAGE="$(mktemp -d -t ratex-pkg-XXXXXX)"
trap 'rm -rf "$STAGE"' EXIT

echo "==> Staging macOS package files for $ARCH (v$VERSION)..."
mkdir -p "$STAGE/bin" "$STAGE/share/tex-suite"

# Copy binaries & symlinks
cp -a "$BUNDLE_DIR/bin/"* "$STAGE/bin/"

# Copy share directory if present
if [ -d "$BUNDLE_DIR/share/tex-suite" ]; then
    cp -a "$BUNDLE_DIR/share/tex-suite/"* "$STAGE/share/tex-suite/"
fi

# Build .pkg
PKG_NAME="ratex-v${VERSION}-macos-${ARCH}.pkg"
PKG_PATH="$OUTPUT_DIR/$PKG_NAME"

echo "==> Running pkgbuild for $PKG_NAME..."
pkgbuild \
    --root "$STAGE" \
    --identifier "io.github.leoliu0.ratex" \
    --version "$VERSION" \
    --install-location "/usr/local" \
    "$PKG_PATH"

echo "==> Created native macOS installer: $PKG_PATH"
