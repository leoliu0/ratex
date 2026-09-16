#!/bin/bash
set -euo pipefail

# Helper to publish or update ratex-bin on Arch Linux AUR.
# Requirements:
#   1. An active Arch User Repository account (https://aur.archlinux.org).
#   2. Your SSH public key uploaded to your AUR account settings.

AUR_PKG="ratex-bin"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "==> Preparing $AUR_PKG for Arch AUR..."
cd "$SCRIPT_DIR"

# Ensure .SRCINFO is up to date
makepkg --printsrcinfo > .SRCINFO

TEMP_DIR="$(mktemp -d -t aur-ratex-XXXXXX)"
trap 'rm -rf "$TEMP_DIR"' EXIT

echo "==> Cloning ssh://aur@aur.archlinux.org/${AUR_PKG}.git..."
if git clone "ssh://aur@aur.archlinux.org/${AUR_PKG}.git" "$TEMP_DIR/repo" 2>/dev/null; then
    cd "$TEMP_DIR/repo"
else
    echo "==> Repository not yet created on AUR or empty. Initializing new repo..."
    mkdir -p "$TEMP_DIR/repo"
    cd "$TEMP_DIR/repo"
    git init
    git remote add origin "ssh://aur@aur.archlinux.org/${AUR_PKG}.git"
fi

cp "$SCRIPT_DIR/PKGBUILD" "$SCRIPT_DIR/.SRCINFO" "$TEMP_DIR/repo/"
git add PKGBUILD .SRCINFO

if git diff --staged --quiet; then
    echo "==> No changes to commit for $AUR_PKG on AUR."
    exit 0
fi

VERSION="$(grep '^pkgver = ' .SRCINFO | head -1 | cut -d' ' -f3)"
git commit -m "Update to v${VERSION}"
echo "==> Pushing to AUR: ssh://aur@aur.archlinux.org/${AUR_PKG}.git..."
git push origin master

echo "==> Successfully released $AUR_PKG v${VERSION} to Arch Linux AUR!"
