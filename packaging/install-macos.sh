#!/bin/sh
# install-macos.sh — installer for the Rust TeX engine suite on macOS.
#
# Installs ratex plus the runtime texmf overlay, clears Gatekeeper
# quarantine attributes, and wires up shell profiles.
#
# Works from an extracted release bundle (bin/ + share/tex-suite/ beside
# this script, or tex-suite-macos-<arch>/ subdir) or a cargo checkout
# (--from-source).
#
# POSIX sh; verified with `sh -n`. See --help for usage.

set -eu

MARK_BEGIN='# >>> tex-suite >>>'
MARK_END='# <<< tex-suite <<<'
DATA_MANIFEST_NAME='.tex-suite-managed-files-v1'
DATA_MANIFEST_HEADER='TEX-SUITE-MANAGED-FILES-1'

BIN_NAMES="ratex"

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
log() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }

usage() {
    cat <<'EOF'
Usage: install-macos.sh [options]

Install ratex and its runtime texmf overlay on macOS.
Detects Apple Silicon (arm64) vs Intel (x86_64), clears com.apple.quarantine
on installed binaries, and updates ~/.zshrc (default shell) and
~/.bash_profile when present.

Modes (auto-detected unless overridden):
  bundle       binaries + share/tex-suite found next to this script or in a
               tex-suite-macos-<arch>/ subdirectory
  from-source  --from-source: cargo build --release --workspace, or reuse
               an existing target/release from a checkout

Options:
  --prefix DIR     Install prefix (default: $HOME/.local; bins -> DIR/bin)
  --system         Install under /usr/local instead (run with sudo)
  --app-support    Put runtime data in ~/Library/Application Support/tex-suite
                   instead of PREFIX/share/tex-suite
  --bundle DIR     Use an explicit extracted bundle directory as source
  --from-source    Build from the cargo workspace and install the result
  --no-build       Fail instead of auto-running cargo build
  --data-dir DIR   Override the runtime data directory (wins over
                   --app-support)
  --link           Symlink binaries into PREFIX/bin instead of copying
  --no-path        Do not edit shell profiles
  --skip-verify    Skip the post-install ratex version check
  --uninstall      Remove installed binaries, data dir, and profile block
  -h, --help       Show this help and exit

Environment overrides:
  TEX_SUITE_DATA   default data directory when none of --data-dir,
                   --app-support, --prefix/--system is given
  CARGO_TARGET_DIR cargo target directory (used for source builds)

Examples:
  ./install.sh                          # user install to ~/.local
  sudo ./install.sh --system            # system install to /usr/local
  ./install.sh --uninstall              # undo a user install
EOF
}

# ---------------------------------------------------------------- arg parsing
PREFIX=""
DATA_DIR=""
BUNDLE_ARG=""
FROM_SOURCE=0
NO_BUILD=0
DO_LINK=0
NO_PATH=0
SKIP_VERIFY=0
UNINSTALL=0
APP_SUPPORT=0

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix)      [ $# -ge 2 ] || die "--prefix needs a directory"; PREFIX="$2"; shift 2 ;;
        --prefix=*)    PREFIX="${1#*=}"; shift ;;
        --system)      PREFIX="/usr/local"; shift ;;
        --app-support) APP_SUPPORT=1; shift ;;
        --bundle)      [ $# -ge 2 ] || die "--bundle needs a directory"; BUNDLE_ARG="$2"; shift 2 ;;
        --bundle=*)    BUNDLE_ARG="${1#*=}"; shift ;;
        --data-dir)    [ $# -ge 2 ] || die "--data-dir needs a directory"; DATA_DIR="$2"; shift 2 ;;
        --data-dir=*)  DATA_DIR="${1#*=}"; shift ;;
        --from-source) FROM_SOURCE=1; shift ;;
        --no-build)    NO_BUILD=1; shift ;;
        --link)        DO_LINK=1; shift ;;
        --no-path)     NO_PATH=1; shift ;;
        --skip-verify) SKIP_VERIFY=1; shift ;;
        --uninstall)   UNINSTALL=1; shift ;;
        -h|--help)     usage; exit 0 ;;
        *)             printf 'unknown option: %s\n\n' "$1" >&2; usage >&2; exit 1 ;;
    esac
done

PREFIX_DEFAULTED=0
if [ -z "$PREFIX" ]; then
    PREFIX="${HOME:?HOME is not set}/.local"
    PREFIX_DEFAULTED=1
fi
# The profile block exports TEX_SUITE_DATA for the active install; it must
# not redirect the data of an install to an explicitly chosen location.
if [ -z "$DATA_DIR" ]; then
    if [ "$APP_SUPPORT" = 1 ]; then
        DATA_DIR="$HOME/Library/Application Support/tex-suite"
    elif [ "$PREFIX_DEFAULTED" = 1 ] && [ -n "${TEX_SUITE_DATA:-}" ]; then
        DATA_DIR="$TEX_SUITE_DATA"
    else
        DATA_DIR="$PREFIX/share/tex-suite"
    fi
fi
case "$PREFIX$DATA_DIR" in
    *'
'*) die "install paths must not contain a newline" ;;
esac

case "$PREFIX" in
    /*) ;;
    *)  die "--prefix must be an absolute path (got: $PREFIX)" ;;
esac
case "$DATA_DIR" in
    /*) ;;
    *)  die "--data-dir/TEX_SUITE_DATA must be an absolute path (got: $DATA_DIR)" ;;
esac
[ ! -L "$DATA_DIR" ] || die "refusing a data directory that is a symlink: $DATA_DIR"

BIN_DIR="$PREFIX/bin"
DATA_MANIFEST="$DATA_DIR/$DATA_MANIFEST_NAME"
OLD_MANIFEST_VALID=0

if [ "$(id -u)" != 0 ] && [ "$PREFIX" = "/usr/local" ]; then
    warn "/usr/local is admin-owned on macOS; sudo is usually required for --system."
fi
if [ "$UNINSTALL" = 0 ] && [ "$PREFIX_DEFAULTED" = 1 ] && [ "$(id -u)" = 0 ] \
    && [ -n "${SUDO_USER:-}" ]; then
    die "refusing a per-user install to $PREFIX as root under sudo (it would leave root-owned files); run without sudo, or use --system / --prefix DIR"
fi

case "$(uname -s)" in
    Darwin) ;;
    *) die "this installer targets macOS (Darwin); on Linux use install-linux.sh" ;;
esac

detect_arch() {
    # arm64 = Apple Silicon; x86_64 = Intel (also reported under Rosetta)
    case "$(uname -m)" in
        arm64|aarch64) echo arm64 ;;
        x86_64)        echo x86_64 ;;
        *)             uname -m ;;
    esac
}
ARCH=$(detect_arch)
log "Detected macOS ($ARCH)"

# Prefer a native arm64 build on Apple Silicon even when the shell runs
# under Rosetta (arch reports x86_64 there).
if [ "$ARCH" = x86_64 ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = 1 ]; then
    warn "CPU is Apple Silicon but this shell runs under Rosetta; preferring the arm64 bundle."
    ARCH=arm64
fi

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

# ------------------------------------------------------------------ payload
SRC_BIN=""; SRC_FMT=""; SRC_TEXMF=""
STAGE_TEXMF=""

TMP_FILE=""

cleanup() {
    [ -z "${STAGE_TEXMF:-}" ] || rm -rf -- "$STAGE_TEXMF" 2>/dev/null || true
    [ -z "${TMP_FILE:-}" ] || rm -f -- "$TMP_FILE" 2>/dev/null || true
}
# Signal traps must exit: a handler that returns lets the script continue
# with the staging directory already deleted. `exit` runs the EXIT trap.
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

try_bundle() {
    [ -x "$1/bin/ratex" ] || return 1
    # Absolute, so --link never creates a symlink relative to the caller's cwd.
    _b=$(CDPATH= cd -- "$1" && pwd) || return 1
    SRC_BIN="$_b/bin"
    if [ -f "$_b/share/tex-suite/pdflatex.fmt" ]; then
        SRC_FMT="$_b/share/tex-suite/pdflatex.fmt"
    else
        SRC_FMT=""
    fi
    if [ -d "$_b/share/tex-suite/texmf" ]; then
        SRC_TEXMF="$_b/share/tex-suite/texmf"
    else
        SRC_TEXMF=""
    fi
    return 0
}

check_font_payload() {
    [ -n "$SRC_TEXMF" ] && [ -d "$SRC_TEXMF/doc/fonts" ] || return 0
    for _fp in NOTICES-FONTS.txt sources.tar.zst packages.lock.json; do
        [ -f "$SRC_TEXMF/doc/fonts/$_fp" ] \
            || die "font redistribution payload incomplete in $SRC_TEXMF/doc/fonts"
    done
}

find_repo_root() {
    for _r in "$SCRIPT_DIR" "$SCRIPT_DIR/.." "$SCRIPT_DIR/../.."; do
        if [ -f "$_r/Cargo.toml" ] && [ -d "$_r/crates" ]; then
            (CDPATH= cd -- "$_r" && pwd)
            return 0
        fi
    done
    return 1
}

resolve_payload() {
    if [ -n "$BUNDLE_ARG" ]; then
        [ -d "$BUNDLE_ARG" ] || die "--bundle: not a directory: $BUNDLE_ARG"
        try_bundle "$BUNDLE_ARG" \
            || die "--bundle $BUNDLE_ARG has no executable bin/ratex"
        check_font_payload
        log "Using release bundle at $BUNDLE_ARG"
        return 0
    fi

    if [ "$FROM_SOURCE" = 0 ]; then
        # bundle dirs use the aarch64 token (package_dist.py); accept arm64 too
        for _cand in "$SCRIPT_DIR" "$SCRIPT_DIR/tex-suite-macos-$ARCH" \
            "$SCRIPT_DIR/tex-suite-macos-aarch64" "$SCRIPT_DIR/tex-suite-macos-arm64" \
            "$SCRIPT_DIR"/tex-suite-macos-*/; do
            _cand=${_cand%/}
            [ -d "$_cand" ] && try_bundle "$_cand" || continue
            check_font_payload
            log "Using release bundle at $_cand"
            return 0
        done
    fi

    _repo=$(find_repo_root) || die "no release bundle found and this is not a cargo checkout; pass --bundle DIR or run from the repo"
    _target="${CARGO_TARGET_DIR:-$_repo/target}/release"
    if [ ! -x "$_target/ratex" ] || { [ "$FROM_SOURCE" = 1 ] && [ "$NO_BUILD" = 0 ]; }; then
        [ "$NO_BUILD" = 1 ] && die "--no-build given but a build is required ($_target/ratex is missing)"
        command -v cargo >/dev/null 2>&1 || die "cargo not found; install Rust (https://rustup.rs) or pass --bundle DIR"
        log "Building release binaries: cargo build --release --workspace"
        ( CDPATH= cd -- "$_repo" && cargo build --release --workspace ) || die "cargo build failed"
    fi
    [ -x "$_target/ratex" ] || die "build did not produce $_target/ratex"
    SRC_BIN="$_target"
    SRC_FMT=""
    _assets_dir="$_repo/crates/tex-kpse/assets"
    _legal_dir="$_assets_dir/legal"
    _sources_zst="$_assets_dir/sources.tar.zst"
    _packages_lock="$_assets_dir/packages.lock.json"
    if [ ! -d "$_legal_dir" ] || [ ! -f "$_legal_dir/NOTICES-FONTS.txt" ] \
        || [ ! -f "$_sources_zst" ] || [ ! -f "$_packages_lock" ]; then
        die "required font redistribution assets missing in $_assets_dir (expected legal/NOTICES-FONTS.txt, sources.tar.zst, and packages.lock.json)"
    fi

    STAGE_TEXMF=$(mktemp -d 2>/dev/null || mktemp -d -t texsuite-stage)
    mkdir -p -- "$STAGE_TEXMF/doc/fonts" || die "cannot create temporary font redistribution directory"
    for _t in "$_repo/texmf" "$_repo/packaging/texmf"; do
        if [ -d "$_t" ]; then
            cp -R -- "$_t"/. "$STAGE_TEXMF/" || die "failed to copy texmf overlay"
            break
        fi
    done
    cp -R -- "$_legal_dir"/. "$STAGE_TEXMF/doc/fonts/" || die "failed to copy font legal notices"
    cp -f -- "$_sources_zst" "$STAGE_TEXMF/doc/fonts/sources.tar.zst" || die "failed to copy font sources archive"
    cp -f -- "$_packages_lock" "$STAGE_TEXMF/doc/fonts/packages.lock.json" || die "failed to copy font lock inventory"
    SRC_TEXMF="$STAGE_TEXMF"
    log "Installing from checkout: $_repo (target: $_target)"
}

# ------------------------------------------------------------ quarantine mgmt
clear_quarantine() {
    command -v xattr >/dev/null 2>&1 || { warn "xattr not found; skipping quarantine clear"; return 0; }
    for _t in $BIN_NAMES; do
        _p="$BIN_DIR/$_t"
        if [ -e "$_p" ] && [ ! -L "$_p" ]; then
            xattr -d com.apple.quarantine "$_p" 2>/dev/null || true
        fi
    done
    # DATA_DIR may be a caller-owned shared tree. Clear attributes only on the
    # exact entries recorded by this install, never recursively on the root or
    # through a payload symlink.
    if [ -f "$DATA_MANIFEST" ] && [ ! -L "$DATA_MANIFEST" ]; then
        while IFS="$(printf '\t')" read -r _cq_kind _cq_rel; do
            case "$_cq_kind" in
                F|D) ;;
                *) continue ;;
            esac
            safe_manifest_relative "$_cq_rel" || continue
            _cq_path="$DATA_DIR/$_cq_rel"
            if data_parent_is_contained "$_cq_path" \
                && [ -e "$_cq_path" ] && [ ! -L "$_cq_path" ]; then
                xattr -d com.apple.quarantine "$_cq_path" 2>/dev/null || true
            fi
        done < "$DATA_MANIFEST"
        xattr -d com.apple.quarantine "$DATA_MANIFEST" 2>/dev/null || true
    fi
    # Ad-hoc sign so Gatekeeper never kills a quarantined copy that slipped
    # through (codeship-free local builds are unsigned otherwise).
    if command -v codesign >/dev/null 2>&1; then
        for _t in $BIN_NAMES; do
            case "$_t" in *.fmt) continue ;; esac
            [ -f "$BIN_DIR/$_t" ] && [ ! -L "$BIN_DIR/$_t" ] \
                && codesign --force --sign - "$BIN_DIR/$_t" >/dev/null 2>&1 || true
        done
    fi
    log "Cleared com.apple.quarantine on installed binaries."
}

# --------------------------------------------------------------- profile mgmt
# Remove tex-suite blocks from a profile. With NEEDLE arguments, only blocks
# containing one of them as an exact line are removed (uninstall keeps the
# block of another prefix). An unterminated block is kept verbatim. The file
# is rewritten in place, so symlinked dotfiles, modes and owners survive.
strip_block() {
    _f="$1"
    [ -f "$_f" ] || return 0
    grep -qF "$MARK_BEGIN" "$_f" 2>/dev/null || return 0
    TMP_FILE=$(mktemp 2>/dev/null || mktemp -t texsuite) || die "cannot create a temporary file"
    TEXSUITE_N1="${2:-}" TEXSUITE_N2="${3:-}" awk -v b="$MARK_BEGIN" -v e="$MARK_END" '
        BEGIN {
            n1 = ENVIRON["TEXSUITE_N1"]; n2 = ENVIRON["TEXSUITE_N2"]
            any = (n1 == "" && n2 == "")
        }
        !inb && index($0, b) { inb = 1; hit = any; buf = $0 "\n"; next }
        inb {
            buf = buf $0 "\n"
            if ((n1 != "" && $0 == n1) || (n2 != "" && $0 == n2)) hit = 1
            if (index($0, e)) { if (!hit) printf "%s", buf; inb = 0; buf = "" }
            next
        }
        { print }
        END { if (inb) printf "%s", buf }
    ' "$_f" > "$TMP_FILE" || die "cannot rewrite $_f"
    if ! cmp -s -- "$TMP_FILE" "$_f"; then
        cat -- "$TMP_FILE" > "$_f" || die "cannot rewrite $_f"
        log "  cleaned tex-suite block from $_f"
    fi
    rm -f -- "$TMP_FILE"
    TMP_FILE=""
}

write_block() {
    _f="$1"; shift
    if [ ! -f "$_f" ] && ! touch -- "$_f" 2>/dev/null; then
        warn "cannot edit $_f; skipping"
        return 0
    fi
    [ -w "$_f" ] || { warn "$_f is not writable; skipping"; return 0; }
    strip_block "$_f"
    {
        printf '%s\n' "$MARK_BEGIN"
        for _line in "$@"; do printf '%s\n' "$_line"; done
        printf '%s\n' "$MARK_END"
    } >> "$_f"
    log "  updated $_f"
}

# Single-quote for sh: every byte, including " $ ` \, stays literal.
sh_quote() {
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

path_line() {
    printf 'export PATH=%s:"$PATH"' "$(sh_quote "$BIN_DIR")"
}

update_profiles() {
    [ "$NO_PATH" = 1 ] && { log "Skipping shell profile updates (--no-path)"; return 0; }
    _l1=$(path_line)
    _l2="export TEX_SUITE_DATA=$(sh_quote "$DATA_DIR")"
    _l3="export TEXMFLOCAL=$(sh_quote "$DATA_DIR/texmf")"
    _touched=0
    # zsh is the macOS default shell; bash_profile covers bash users.
    for _f in "$HOME/.zshrc" "$HOME/.bash_profile"; do
        if [ -f "$_f" ]; then
            write_block "$_f" "$_l1" "$_l2" "$_l3"
            _touched=1
        fi
    done
    # Also keep ~/.profile for other login shells.
    if [ -f "$HOME/.profile" ]; then
        write_block "$HOME/.profile" "$_l1" "$_l2" "$_l3"
        _touched=1
    fi
    if [ "$_touched" = 0 ]; then
        # Fresh macOS home: create the default-shell profile.
        if [ "$(dscl . -read "/Users/${USER:-$(id -un)}" UserShell 2>/dev/null)" = "UserShell: /bin/zsh" ]; then
            write_block "$HOME/.zshrc" "$_l1" "$_l2" "$_l3"
        else
            write_block "$HOME/.bash_profile" "$_l1" "$_l2" "$_l3"
        fi
    fi
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *) log "Open a new terminal (or:  source ~/.zshrc ) to add $BIN_DIR to PATH." ;;
    esac
}

# ------------------------------------------------------------------ install
install_ratex() {
    _src="$SRC_BIN/ratex"
    _dst="$BIN_DIR/ratex"
    rm -f -- "$_dst" 2>/dev/null || true   # never write through a stale symlink
    if [ "$DO_LINK" = 1 ]; then
        ln -s -- "$_src" "$_dst" || die "failed to link $_dst -> $_src"
    else
        cp -f -- "$_src" "$_dst" || die "failed to copy $_src -> $_dst"
        chmod 755 -- "$_dst" 2>/dev/null || true
    fi
    log "  installed $_dst"
}

safe_manifest_relative() {
    case "$1" in
        ''|/*|..|../*|*/..|*/../*|*"$(printf '\t')"*) return 1 ;;
        *) return 0 ;;
    esac
}

manifest_has() {
    _mh_kind="$1"
    _mh_path="$2"
    [ -f "$DATA_MANIFEST" ] && [ ! -L "$DATA_MANIFEST" ] || return 1
    IFS= read -r _mh_header < "$DATA_MANIFEST" || return 1
    [ "$_mh_header" = "$DATA_MANIFEST_HEADER" ] || return 1
    _mh_record=$(printf '%s\t%s' "$_mh_kind" "$_mh_path")
    grep -Fqx -- "$_mh_record" "$DATA_MANIFEST"
}

validate_existing_manifest() {
    OLD_MANIFEST_VALID=0
    if [ ! -e "$DATA_MANIFEST" ] && [ ! -L "$DATA_MANIFEST" ]; then
        return 0
    fi
    if [ -L "$DATA_MANIFEST" ] || [ ! -f "$DATA_MANIFEST" ]; then
        die "refusing an unrecognized ownership manifest: $DATA_MANIFEST"
    fi
    IFS= read -r _vm_header < "$DATA_MANIFEST" || _vm_header=''
    [ "$_vm_header" = "$DATA_MANIFEST_HEADER" ] \
        || die "refusing an unrecognized ownership manifest: $DATA_MANIFEST"
    grep -Fqx -- "$(printf 'PREFIX\t%s' "$PREFIX")" "$DATA_MANIFEST" \
        || die "ownership manifest belongs to a different install prefix: $DATA_MANIFEST"
    grep -Fqx -- "$(printf 'DATA\t%s' "$DATA_DIR")" "$DATA_MANIFEST" \
        || die "ownership manifest belongs to a different data directory: $DATA_MANIFEST"
    while IFS="$(printf '\t')" read -r _vm_kind _vm_rel; do
        case "$_vm_kind" in
            "$DATA_MANIFEST_HEADER") [ -z "$_vm_rel" ] || die "invalid ownership manifest header: $DATA_MANIFEST" ;;
            PREFIX) [ "$_vm_rel" = "$PREFIX" ] || die "ownership manifest prefix mismatch: $DATA_MANIFEST" ;;
            DATA) [ "$_vm_rel" = "$DATA_DIR" ] || die "ownership manifest data mismatch: $DATA_MANIFEST" ;;
            B)
                safe_manifest_relative "$_vm_rel" && case "$_vm_rel" in */*) false ;; *) true ;; esac \
                    || die "unsafe binary ownership record in $DATA_MANIFEST"
                ;;
            F|D) safe_manifest_relative "$_vm_rel" || die "unsafe data ownership record in $DATA_MANIFEST" ;;
            C)
                case "$_vm_rel" in
                    bin|share|.) ;;
                    *) die "unsafe created-directory record in $DATA_MANIFEST" ;;
                esac
                ;;
            *) die "unrecognized ownership record in $DATA_MANIFEST: $_vm_kind" ;;
        esac
    done < "$DATA_MANIFEST"
    OLD_MANIFEST_VALID=1
}

preflight_destination() {
    _pf_kind="$1"
    _pf_rel="$2"
    _pf_path="$3"
    if [ -e "$_pf_path" ] || [ -L "$_pf_path" ]; then
        if [ "$OLD_MANIFEST_VALID" != 1 ] || ! manifest_has "$_pf_kind" "$_pf_rel"; then
            die "refusing to overwrite unowned path: $_pf_path"
        fi
        if [ -d "$_pf_path" ] && [ ! -L "$_pf_path" ]; then
            die "managed file destination is a directory: $_pf_path"
        fi
    fi
}

preflight_install() {
    [ ! -L "$BIN_DIR" ] || die "refusing a binary directory that is a symlink: $BIN_DIR"
    [ ! -L "$DATA_DIR/texmf" ] \
        || die "refusing a texmf directory that is a symlink: $DATA_DIR/texmf"
    preflight_destination B ratex "$BIN_DIR/ratex"
    if [ -n "$SRC_FMT" ]; then
        preflight_destination B pdflatex.fmt "$BIN_DIR/pdflatex.fmt"
        preflight_destination F pdflatex.fmt "$DATA_DIR/pdflatex.fmt"
    else
        for _pf_format in "$BIN_DIR/pdflatex.fmt" "$DATA_DIR/pdflatex.fmt"; do
            if [ -e "$_pf_format" ] || [ -L "$_pf_format" ]; then
                legacy_format_is_owned \
                    || die "unowned format override blocks the embedded format: $_pf_format"
            fi
        done
    fi
    if [ -n "$SRC_TEXMF" ]; then
        if [ -e "$DATA_DIR/texmf" ] && [ ! -d "$DATA_DIR/texmf" ]; then
            die "refusing to install through non-directory $DATA_DIR/texmf"
        fi
        if [ -d "$DATA_DIR/texmf" ] && find "$DATA_DIR/texmf" -type l -print -quit | grep -q .; then
            die "refusing to update a texmf tree containing symlinks: $DATA_DIR/texmf"
        fi
        (CDPATH= cd -- "$SRC_TEXMF" && find . \( -type f -o -type l \) -print) |
            while IFS= read -r _pf_rel; do
                _pf_rel=${_pf_rel#./}
                safe_manifest_relative "$_pf_rel" \
                    || die "unsafe texmf payload path: $_pf_rel"
                preflight_destination F "texmf/$_pf_rel" "$DATA_DIR/texmf/$_pf_rel"
            done
        # Directories are recorded too; reject them before anything changes.
        (CDPATH= cd -- "$SRC_TEXMF" && find . -type d -print) |
            while IFS= read -r _pf_rel; do
                [ "$_pf_rel" = . ] && continue
                safe_manifest_relative "${_pf_rel#./}" \
                    || die "unsafe texmf payload directory: ${_pf_rel#./}"
            done
    fi
}

legacy_format_is_owned() {
    [ "$OLD_MANIFEST_VALID" = 1 ] \
        && manifest_has B pdflatex.fmt \
        && manifest_has F pdflatex.fmt
}

# Every failure must reach the caller: this runs on the left of `||`, where
# `set -e` is ignored and a `die` in a pipeline subshell only ends that subshell.
manifest_records() {
    printf '%s\n' "$DATA_MANIFEST_HEADER" || return 1
    printf 'PREFIX\t%s\n' "$PREFIX" || return 1
    printf 'DATA\t%s\n' "$DATA_DIR" || return 1
    printf 'B\tratex\n' || return 1
    if [ -n "$SRC_FMT" ]; then
        printf 'B\tpdflatex.fmt\nF\tpdflatex.fmt\n' || return 1
    fi
    # Directories this install created (relative to PREFIX); uninstall
    # removes them again when they are empty.
    if [ "$CREATED_BIN" = 1 ]; then printf 'C\tbin\n' || return 1; fi
    if [ "$CREATED_SHARE" = 1 ]; then printf 'C\tshare\n' || return 1; fi
    if [ "$CREATED_PREFIX" = 1 ]; then printf 'C\t.\n' || return 1; fi
    if [ -n "$SRC_TEXMF" ]; then
        (CDPATH= cd -- "$SRC_TEXMF" && find . \( -type f -o -type l \) -print) |
            LC_ALL=C sort |
            while IFS= read -r _wm_rel; do
                _wm_rel=${_wm_rel#./}
                safe_manifest_relative "$_wm_rel" \
                    || die "unsafe texmf payload path: $_wm_rel"
                printf 'F\ttexmf/%s\n' "$_wm_rel" || exit 1
            done || return 1
        (CDPATH= cd -- "$SRC_TEXMF" && find . -depth -type d -print) |
            while IFS= read -r _wm_rel; do
                [ "$_wm_rel" = . ] && continue
                _wm_rel=${_wm_rel#./}
                safe_manifest_relative "$_wm_rel" \
                    || die "unsafe texmf payload directory: $_wm_rel"
                printf 'D\ttexmf/%s\n' "$_wm_rel" || exit 1
            done || return 1
    fi
    printf 'D\ttexmf\n'
}

write_data_manifest() {
    TMP_FILE="$DATA_MANIFEST.tmp.$$"
    manifest_records > "$TMP_FILE" || die "cannot write data ownership manifest"
    mv -- "$TMP_FILE" "$DATA_MANIFEST" \
        || die "cannot install data ownership manifest at $DATA_MANIFEST"
    TMP_FILE=""
}

data_parent_is_contained() {
    _dp_path="$1"
    _dp_parent=${_dp_path%/*}
    _dp_root=$(CDPATH= cd -- "$DATA_DIR" 2>/dev/null && pwd -P) || return 1
    _dp_parent=$(CDPATH= cd -- "$_dp_parent" 2>/dev/null && pwd -P) || return 1
    case "$_dp_parent" in
        "$_dp_root"|"$_dp_root"/*) return 0 ;;
        *) return 1 ;;
    esac
}

remove_manifest_entries() {
    _rm_keep_manifest="${1:-no}"
    [ -e "$DATA_MANIFEST" ] || return 0
    if [ -L "$DATA_MANIFEST" ] || [ ! -f "$DATA_MANIFEST" ]; then
        warn "data ownership manifest is not a regular file; preserving data: $DATA_MANIFEST"
        return 0
    fi
    IFS= read -r _rm_header < "$DATA_MANIFEST" || _rm_header=''
    if [ "$_rm_header" != "$DATA_MANIFEST_HEADER" ]; then
        warn "data ownership manifest is invalid; preserving data: $DATA_MANIFEST"
        return 0
    fi
    _rm_created_bin=0; _rm_created_share=0; _rm_created_prefix=0
    while IFS="$(printf '\t')" read -r _rm_kind _rm_rel; do
        case "$_rm_kind" in
            "$DATA_MANIFEST_HEADER"|PREFIX|DATA) continue ;;
            C)
                case "$_rm_rel" in
                    bin) _rm_created_bin=1 ;;
                    share) _rm_created_share=1 ;;
                    .) _rm_created_prefix=1 ;;
                    *) warn "ignoring unsafe created-directory record: $_rm_rel" ;;
                esac
                continue
                ;;
        esac
        safe_manifest_relative "$_rm_rel" || {
            warn "ignoring unsafe managed-data path: $_rm_rel"
            continue
        }
        case "$_rm_kind" in
            B)
                case "$_rm_rel" in */*) warn "ignoring unsafe managed binary path: $_rm_rel"; continue ;; esac
                _rm_path="$BIN_DIR/$_rm_rel"
                if [ -f "$_rm_path" ] || [ -L "$_rm_path" ]; then
                    rm -f -- "$_rm_path" || warn "could not remove $_rm_path"
                fi
                ;;
            F)
                _rm_path="$DATA_DIR/$_rm_rel"
                if ! data_parent_is_contained "$_rm_path"; then
                    warn "preserving managed-data path through an unsafe parent: $_rm_path"
                elif [ -f "$_rm_path" ] || [ -L "$_rm_path" ]; then
                    rm -f -- "$_rm_path" || warn "could not remove $_rm_path"
                fi
                ;;
            D)
                _rm_path="$DATA_DIR/$_rm_rel"
                if data_parent_is_contained "$_rm_path" && [ -d "$_rm_path" ] && [ ! -L "$_rm_path" ]; then
                    rmdir -- "$_rm_path" 2>/dev/null || true
                fi
                ;;
            *) warn "ignoring unknown data ownership record: $_rm_kind" ;;
        esac
    done < "$DATA_MANIFEST"
    if [ "$_rm_keep_manifest" != keep ]; then
        rm -f -- "$DATA_MANIFEST" || warn "could not remove $DATA_MANIFEST"
        rmdir -- "$DATA_DIR" 2>/dev/null || true
        # Non-recursive: a directory holding anything else is kept.
        [ "$_rm_created_bin" = 0 ] || rmdir -- "$BIN_DIR" 2>/dev/null || true
        [ "$_rm_created_share" = 0 ] || rmdir -- "$PREFIX/share" 2>/dev/null || true
        [ "$_rm_created_prefix" = 0 ] || rmdir -- "$PREFIX" 2>/dev/null || true
    fi
}

do_install() {
    resolve_payload
    _new_prefix=0; _new_share=0; _new_bin=0
    [ -e "$PREFIX" ] || [ -L "$PREFIX" ] || _new_prefix=1
    [ -e "$PREFIX/share" ] || [ -L "$PREFIX/share" ] || _new_share=1
    [ -e "$BIN_DIR" ] || [ -L "$BIN_DIR" ] || _new_bin=1
    mkdir -p -- "$BIN_DIR" "$DATA_DIR" || die "cannot create $BIN_DIR / $DATA_DIR (use sudo for --system)"
    validate_existing_manifest
    preflight_install
    # A reinstall inherits the created-directory records of the previous one.
    CREATED_PREFIX=0; CREATED_SHARE=0; CREATED_BIN=0
    if [ "$_new_prefix" = 1 ] || manifest_has C .; then CREATED_PREFIX=1; fi
    if [ -d "$PREFIX/share" ] && { [ "$_new_share" = 1 ] || manifest_has C share; }; then
        CREATED_SHARE=1
    fi
    if [ "$_new_bin" = 1 ] || manifest_has C bin; then CREATED_BIN=1; fi
    if [ "$OLD_MANIFEST_VALID" = 1 ]; then
        remove_manifest_entries keep
    fi

    log "Installing binaries to $BIN_DIR"
    install_ratex
    log "Installing runtime data to $DATA_DIR"
    if [ -n "$SRC_FMT" ]; then
        rm -f -- "$DATA_DIR/pdflatex.fmt" "$BIN_DIR/pdflatex.fmt" 2>/dev/null || true
        cp -f -- "$SRC_FMT" "$DATA_DIR/pdflatex.fmt" || die "failed to copy format file"
        log "  installed $DATA_DIR/pdflatex.fmt"
        ln -sf -- "$DATA_DIR/pdflatex.fmt" "$BIN_DIR/pdflatex.fmt" 2>/dev/null \
            || cp -f -- "$SRC_FMT" "$BIN_DIR/pdflatex.fmt" 2>/dev/null || true
    else
        # Releases before the embedded-format layout installed these two
        # exact paths. Leaving either behind makes it override the format in
        # the new engine, so remove regular files and symlinks during upgrade.
        if legacy_format_is_owned; then
            for _legacy_fmt in "$BIN_DIR/pdflatex.fmt" "$DATA_DIR/pdflatex.fmt"; do
                if [ -f "$_legacy_fmt" ] || [ -L "$_legacy_fmt" ]; then
                    rm -f -- "$_legacy_fmt" \
                        || die "failed to remove legacy format $_legacy_fmt"
                    log "  removed legacy $_legacy_fmt"
                elif [ -e "$_legacy_fmt" ]; then
                    warn "legacy format path is not a file; leaving it unchanged: $_legacy_fmt"
                fi
            done
        else
            for _legacy_fmt in "$BIN_DIR/pdflatex.fmt" "$DATA_DIR/pdflatex.fmt"; do
                if [ -e "$_legacy_fmt" ] || [ -L "$_legacy_fmt" ]; then
                    warn "preserving unowned format override: $_legacy_fmt"
                fi
            done
        fi
    fi
    if [ -n "$SRC_TEXMF" ]; then
        mkdir -p -- "$DATA_DIR/texmf" || die "cannot create $DATA_DIR/texmf"
        cp -R -- "$SRC_TEXMF"/. "$DATA_DIR/texmf" || die "failed to copy texmf tree"
        log "  installed $DATA_DIR/texmf (exported as TEXMFLOCAL)"
    else
        mkdir -p -- "$DATA_DIR/texmf"
        warn "no texmf overlay in the payload; created empty $DATA_DIR/texmf"
    fi
    write_data_manifest

    clear_quarantine
    update_profiles

    if [ "$SKIP_VERIFY" = 0 ]; then
        log "Verifying installation"
        if [ ! -x "$BIN_DIR/ratex" ]; then
            die "verification failed: $BIN_DIR/ratex is missing or not executable"
        fi
        _ok=0
        for _flag in -version --version -v; do
            if _out=$("$BIN_DIR/ratex" "$_flag" 2>/dev/null); then
                printf '  %s\n' "$_out"
                _ok=1
                break
            fi
        done
        if [ "$_ok" = 1 ]; then
            log "Verification passed."
        else
            warn "ratex is installed but did not answer a version flag; check manually."
        fi
    fi

    cat <<EOF

Done. Next steps:
  - open a new terminal (or:  source ~/.zshrc )
  - compile:  ratex paper.tex
  - data dir: $DATA_DIR
  - uninstall later with:  $0 --uninstall --prefix "$PREFIX" --data-dir "$DATA_DIR"
EOF
}

# ---------------------------------------------------------------- uninstall
do_uninstall() {
    log "Uninstalling tex-suite from prefix $PREFIX (data: $DATA_DIR)"
    _rc=0
    validate_existing_manifest
    if [ "$OLD_MANIFEST_VALID" = 1 ]; then
        remove_manifest_entries
    else
        warn "no ownership manifest found; preserving install and data files"
    fi
    # Legacy Application Support location used by older runs. Only an install
    # of this same prefix is removed; another prefix's data stays untouched.
    _legacy="$HOME/Library/Application Support/tex-suite"
    _legacy_manifest="$_legacy/$DATA_MANIFEST_NAME"
    if [ "$DATA_DIR" != "$_legacy" ] && [ -f "$_legacy_manifest" ] && [ ! -L "$_legacy_manifest" ] \
        && grep -Fqx -- "$(printf 'PREFIX\t%s' "$PREFIX")" "$_legacy_manifest"; then
        _current_data_dir="$DATA_DIR"
        _current_data_manifest="$DATA_MANIFEST"
        DATA_DIR="$_legacy"
        DATA_MANIFEST="$_legacy_manifest"
        validate_existing_manifest
        remove_manifest_entries
        if [ -d "$_legacy" ]; then
            warn "preserving unmanaged files in legacy data directory: $_legacy"
        else
            log "  removed $_legacy (legacy data dir)"
        fi
        DATA_DIR="$_current_data_dir"
        DATA_MANIFEST="$_current_data_manifest"
    fi
    log "Cleaning shell profiles"
    # Only the block that puts this prefix on PATH (current quoting, or the
    # double-quoted form written by earlier installers).
    _cur_line=$(path_line)
    _old_line="export PATH=\"$BIN_DIR:\$PATH\""
    for _f in "$HOME/.zshrc" "$HOME/.bash_profile" "$HOME/.bashrc" "$HOME/.profile"; do
        strip_block "$_f" "$_cur_line" "$_old_line"
        if [ -f "$_f" ] && grep -qF "$MARK_BEGIN" "$_f" 2>/dev/null; then
            warn "kept a tex-suite block for another install in $_f"
        fi
    done
    cat <<EOF
Uninstall complete. To drop the variables from your CURRENT shell session run:
  unset TEX_SUITE_DATA TEXMFLOCAL
(or simply open a new terminal)
EOF
    return $_rc
}

# --------------------------------------------------------------------- main
if [ "$UNINSTALL" = 1 ]; then
    do_uninstall
else
    do_install
fi
