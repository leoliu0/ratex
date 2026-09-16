#!/bin/bash
set -euo pipefail

# Helper to publish or update ratex and/or ratex-bin on Arch Linux AUR.
# Requirements:
#   1. An active Arch User Repository account (https://aur.archlinux.org).
#   2. Your SSH public key uploaded to your AUR account settings.

TARGET="${1:-ratex-bin}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

publish_one() {
    local pkg="$1"
    local pkgbuild_src="$2"

    echo "==> Preparing $pkg for Arch AUR..."
    local temp_dir
    temp_dir="$(mktemp -d -t "aur-${pkg}-XXXXXX")"
    trap 'rm -rf "$temp_dir"' RETURN

    # Generate .SRCINFO
    local srcinfo
    srcinfo="$(makepkg -p "$pkgbuild_src" --printsrcinfo)"

    echo "==> Cloning ssh://aur@aur.archlinux.org/${pkg}.git..."
    if git clone "ssh://aur@aur.archlinux.org/${pkg}.git" "$temp_dir/repo" 2>/dev/null; then
        cd "$temp_dir/repo"
    else
        echo "==> Repository not yet created on AUR or empty. Initializing new repo..."
        mkdir -p "$temp_dir/repo"
        cd "$temp_dir/repo"
        git init
        git remote add origin "ssh://aur@aur.archlinux.org/${pkg}.git"
    fi

    cp "$pkgbuild_src" "$temp_dir/repo/PKGBUILD"
    printf '%s\n' "$srcinfo" > "$temp_dir/repo/.SRCINFO"

    git add PKGBUILD .SRCINFO
    version="$(printf '%s\n' "$srcinfo" | awk '/pkgver = / {print $3; exit}')"
    if git diff --staged --quiet; then
        echo "==> No changes to commit for $pkg on AUR."
        return 0
    fi

    git commit -m "Update to v${version}"
    echo "==> Pushing to AUR: ssh://aur@aur.archlinux.org/${pkg}.git..."
    git push origin master

    echo "==> Successfully released $pkg v${version} to Arch Linux AUR!"
}

cd "$SCRIPT_DIR"

case "$TARGET" in
    ratex-bin)
        makepkg --printsrcinfo > .SRCINFO
        publish_one "ratex-bin" "$SCRIPT_DIR/PKGBUILD"
        ;;
    ratex)
        makepkg -p PKGBUILD.source --printsrcinfo > .SRCINFO.source
        publish_one "ratex" "$SCRIPT_DIR/PKGBUILD.source"
        ;;
    all)
        makepkg --printsrcinfo > .SRCINFO
        publish_one "ratex-bin" "$SCRIPT_DIR/PKGBUILD"
        makepkg -p PKGBUILD.source --printsrcinfo > .SRCINFO.source
        publish_one "ratex" "$SCRIPT_DIR/PKGBUILD.source"
        ;;
    *)
        echo "Usage: $0 [ratex-bin|ratex|all]" >&2
        exit 1
        ;;
esac
