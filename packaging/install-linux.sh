#!/bin/sh
# install-linux.sh — installer for the Rust TeX engine suite on Linux.
#
# Installs pdflatex/xelatex/lualatex/bibtex/texmk/latexmk plus the runtime
# data directory (pdflatex.fmt + texmf overlay). Works from:
#   1. an extracted release bundle (bin/ + share/tex-suite/ next to this
#      script, or a tex-suite-linux-<arch>/ subdir), or
#   2. a source checkout (--from-source builds with cargo, or reuses a
#      fresh target/release).
#
# POSIX sh; verified with `sh -n`. See --help for usage.

set -eu

MARK_BEGIN='# >>> tex-suite >>>'
MARK_END='# <<< tex-suite <<<'

BIN_NAMES="pdflatex xelatex lualatex bibtex texmk latexmk tex-index pdflatex.fmt"

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
log() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }

usage() {
    cat <<'EOF'
Usage: install-linux.sh [options]

Install the Rust TeX suite (pdflatex, xelatex, lualatex, bibtex, texmk,
latexmk) and its runtime data (pdflatex.fmt, texmf overlay) on Linux.

Modes (auto-detected unless overridden):
  bundle       binaries + share/tex-suite found next to this script or in a
               tex-suite-linux-<arch>/ subdirectory
  from-source  --from-source: cargo build --release --workspace, or reuse
               an existing target/release from a checkout

Options:
  --prefix DIR     Install prefix (default: $HOME/.local; bins -> DIR/bin,
                   data -> DIR/share/tex-suite)
  --system         Install under /usr/local instead (run with sudo)
  --bundle DIR     Use an explicit extracted bundle directory as source
  --from-source    Build from the cargo workspace and install the result
  --no-build       Fail instead of auto-running cargo build
  --data-dir DIR   Override the runtime data directory
                   (default: PREFIX/share/tex-suite)
  --link           Symlink binaries into PREFIX/bin instead of copying
  --no-path        Do not edit shell profiles
  --skip-verify    Skip the post-install pdflatex version check
  --uninstall      Remove installed binaries, data dir, and profile block
  -h, --help       Show this help and exit

Environment overrides:
  TEX_SUITE_DATA   default data directory when --data-dir is not given
  CARGO_TARGET_DIR cargo target directory (used for source builds)

Examples:
  ./install.sh                          # user install to ~/.local
  sudo ./install.sh --system            # system install to /usr/local
  ./install.sh --uninstall              # undo a user install
EOF
}

# ---------------------------------------------------------------- arg parsing
PREFIX=""
DATA_DIR="${TEX_SUITE_DATA:-}"
BUNDLE_ARG=""
FROM_SOURCE=0
NO_BUILD=0
DO_LINK=0
NO_PATH=0
SKIP_VERIFY=0
UNINSTALL=0

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix)      [ $# -ge 2 ] || die "--prefix needs a directory"; PREFIX="$2"; shift 2 ;;
        --prefix=*)    PREFIX="${1#*=}"; shift ;;
        --system)      PREFIX="/usr/local"; shift ;;
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

[ -n "$PREFIX" ] || PREFIX="${HOME:?HOME is not set}/.local"
[ -n "$DATA_DIR" ] || DATA_DIR="$PREFIX/share/tex-suite"

case "$PREFIX" in
    /*) ;;
    *)  die "--prefix must be an absolute path (got: $PREFIX)" ;;
esac

BIN_DIR="$PREFIX/bin"

if [ "$(id -u)" != 0 ] && [ "$PREFIX" = "/usr/local" ]; then
    die "system install to /usr/local needs root: re-run with sudo"
fi

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)  echo x86_64 ;;
        aarch64|arm64) echo aarch64 ;;
        *)             uname -m ;;
    esac
}
ARCH=$(detect_arch)

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

# ------------------------------------------------------------------ payload
SRC_BIN=""; SRC_FMT=""; SRC_TEXMF=""

try_bundle() {
    _b="$1"
    [ -d "$_b/bin" ] || return 1
    [ -f "$_b/share/tex-suite/pdflatex.fmt" ] || return 1
    SRC_BIN="$_b/bin"
    SRC_FMT="$_b/share/tex-suite/pdflatex.fmt"
    if [ -d "$_b/share/tex-suite/texmf" ]; then
        SRC_TEXMF="$_b/share/tex-suite/texmf"
    else
        SRC_TEXMF=""
    fi
    return 0
}

find_repo_root() {
    # packaging/ lives one level below the workspace root.
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
            || die "--bundle $BUNDLE_ARG has no bin/ + share/tex-suite/pdflatex.fmt"
        log "Using release bundle at $BUNDLE_ARG"
        return 0
    fi

    if [ "$FROM_SOURCE" = 0 ]; then
        try_bundle "$SCRIPT_DIR" && { log "Using release bundle at $SCRIPT_DIR"; return 0; }
        _d="$SCRIPT_DIR/tex-suite-linux-$ARCH"
        if [ -d "$_d" ] && try_bundle "$_d"; then log "Using release bundle at $_d"; return 0; fi
        for _cand in "$SCRIPT_DIR"/tex-suite-linux-*/; do
            [ -d "$_cand" ] || continue
            _cand=${_cand%/}
            if try_bundle "$_cand"; then log "Using release bundle at $_cand"; return 0; fi
        done
    fi

    # source checkout
    _repo=$(find_repo_root) || die "no release bundle found and this is not a cargo checkout; pass --bundle DIR or run from the repo"
    _target="${CARGO_TARGET_DIR:-$_repo/target}/release"
    if [ "$FROM_SOURCE" = 1 ] || [ ! -x "$_target/pdflatex" ]; then
        [ "$NO_BUILD" = 1 ] && die "--no-build given but a build is required (target/release incomplete)"
        command -v cargo >/dev/null 2>&1 || die "cargo not found; install Rust or pass --bundle DIR"
        log "Building release binaries: cargo build --release --workspace"
        ( CDPATH= cd -- "$_repo" && cargo build --release --workspace ) || die "cargo build failed"
    fi
    [ -x "$_target/pdflatex" ] || die "build did not produce $_target/pdflatex"
    SRC_BIN="$_target"
    for _f in "$_repo/pdflatex.fmt" "$_target/pdflatex.fmt" "$_repo/crates/tex-cli/assets/default.fmt"; do
        if [ -f "$_f" ]; then SRC_FMT="$_f"; break; fi
    done
    [ -n "$SRC_FMT" ] || warn "no pdflatex.fmt in the payload; engine falls back to its embedded default format"
    for _t in "$_repo/texmf" "$_repo/packaging/texmf"; do
        if [ -d "$_t" ]; then SRC_TEXMF="$_t"; break; fi
    done
    log "Installing from checkout: $_repo (target: $_target)"
}

# --------------------------------------------------------------- profile mgmt
strip_block() {
    _f="$1"
    [ -f "$_f" ] || return 0
    grep -qF "$MARK_BEGIN" "$_f" 2>/dev/null || return 0
    awk -v b="$MARK_BEGIN" -v e="$MARK_END" '
        index($0, b) { skip=1 }
        !skip { print }
        index($0, e) { skip=0 }
    ' "$_f" > "$_f.texsuite.tmp" && mv -- "$_f.texsuite.tmp" "$_f"
    log "  cleaned tex-suite block from $_f"
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

update_profiles() {
    [ "$NO_PATH" = 1 ] && { log "Skipping shell profile updates (--no-path)"; return 0; }
    _l1="export PATH=\"$BIN_DIR:\$PATH\""
    _l2="export TEX_SUITE_DATA=\"$DATA_DIR\""
    _l3="export TEXMFLOCAL=\"$DATA_DIR/texmf\""
    _touched=0
    for _f in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile" "$HOME/.bash_profile"; do
        if [ -f "$_f" ]; then
            write_block "$_f" "$_l1" "$_l2" "$_l3"
            _touched=1
        fi
    done
    [ "$_touched" = 1 ] || write_block "$HOME/.profile" "$_l1" "$_l2" "$_l3"
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *) log "Open a new shell (or: . ~/.profile) to add $BIN_DIR to PATH." ;;
    esac
}

# ------------------------------------------------------------------ install
install_one() {
    _t="$1"; shift
    _src=""
    for _n in "$@"; do
        if [ -f "$SRC_BIN/$_n" ]; then _src="$SRC_BIN/$_n"; break; fi
    done
    [ -n "$_src" ] || { warn "source binary not found for '$_t' (looked for: $*); skipping"; return 0; }
    _dst="$BIN_DIR/$_t"
    if [ "$DO_LINK" = 1 ]; then
        ln -sf -- "$_src" "$_dst" || cp -f -- "$_src" "$_dst"
    else
        rm -f -- "$_dst" 2>/dev/null || true   # never write through a stale symlink
        cp -f -- "$_src" "$_dst" || die "failed to copy $_src -> $_dst"
    fi
    chmod 755 -- "$_dst" 2>/dev/null || true
    log "  installed $_dst"
}

do_install() {
    resolve_payload

    mkdir -p -- "$BIN_DIR" "$DATA_DIR" || die "cannot create $BIN_DIR / $DATA_DIR (use sudo for --system)"

    log "Installing binaries to $BIN_DIR"
    install_one pdflatex pdflatex
    install_one xelatex xelatex
    install_one lualatex lualatex
    install_one bibtex  bibtex tex-bibtex
    install_one texmk   texmk
    install_one tex-index tex-index
    install_one latexmk latexmk texmk

    log "Installing runtime data to $DATA_DIR"
    if [ -n "$SRC_FMT" ]; then
        cp -f -- "$SRC_FMT" "$DATA_DIR/pdflatex.fmt" || die "failed to copy format file"
        log "  installed $DATA_DIR/pdflatex.fmt"
        # the engine also checks next to the executable; keep it in sync
        ln -sf -- "$DATA_DIR/pdflatex.fmt" "$BIN_DIR/pdflatex.fmt" 2>/dev/null \
            || cp -f -- "$SRC_FMT" "$BIN_DIR/pdflatex.fmt" 2>/dev/null || true
    fi
    if [ -n "$SRC_TEXMF" ]; then
        rm -rf -- "$DATA_DIR/texmf"
        cp -R -- "$SRC_TEXMF" "$DATA_DIR/texmf" || die "failed to copy texmf tree"
        log "  installed $DATA_DIR/texmf (exported as TEXMFLOCAL)"
    else
        mkdir -p -- "$DATA_DIR/texmf"
        warn "no texmf overlay in the payload; created empty $DATA_DIR/texmf"
    fi

    update_profiles

    if [ "$SKIP_VERIFY" = 0 ]; then
        log "Verifying installation"
        if [ ! -x "$BIN_DIR/pdflatex" ]; then
            die "verification failed: $BIN_DIR/pdflatex is missing or not executable"
        fi
        _ok=0
        for _flag in -version --version -v; do
            if _out=$("$BIN_DIR/pdflatex" "$_flag" 2>/dev/null); then
                printf '  %s\n' "$_out"
                _ok=1
                break
            fi
        done
        if [ "$_ok" = 1 ]; then
            [ -x "$BIN_DIR/texmk" ] && "$BIN_DIR/texmk" --version 2>/dev/null | sed 's/^/  /' || true
            log "Verification passed."
        else
            warn "pdflatex is installed but did not answer a version flag; check manually."
        fi
    fi

    cat <<EOF

Done. Next steps:
  - open a new shell (or:  . ~/.profile )
  - compile:  pdflatex paper.tex   or   latexmk -pdf paper.tex
  - data dir: $DATA_DIR
  - uninstall later with:  $0 --uninstall --prefix "$PREFIX" --data-dir "$DATA_DIR"
EOF
}

# ---------------------------------------------------------------- uninstall
do_uninstall() {
    log "Uninstalling tex-suite from prefix $PREFIX (data: $DATA_DIR)"
    _rc=0
    for _t in $BIN_NAMES; do
        if [ -e "$BIN_DIR/$_t" ] || [ -L "$BIN_DIR/$_t" ]; then
            rm -f -- "$BIN_DIR/$_t" && log "  removed $BIN_DIR/$_t" \
                || { warn "could not remove $BIN_DIR/$_t"; _rc=1; }
        fi
    done
    if [ -d "$DATA_DIR" ]; then
        rm -rf -- "$DATA_DIR" && log "  removed $DATA_DIR" \
            || { warn "could not remove $DATA_DIR"; _rc=1; }
    fi
    if [ -f /etc/profile.d/tex-suite.sh ]; then
        rm -f -- /etc/profile.d/tex-suite.sh 2>/dev/null \
            && log "  removed /etc/profile.d/tex-suite.sh" \
            || warn "/etc/profile.d/tex-suite.sh still present (remove with sudo)"
    fi
    log "Cleaning shell profiles"
    for _f in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile" "$HOME/.bash_profile"; do
        strip_block "$_f" || true
    done
    cat <<EOF
Uninstall complete. To drop the variables from your CURRENT shell session run:
  unset TEX_SUITE_DATA TEXMFLOCAL
(or simply open a new shell)
EOF
    return $_rc
}

# --------------------------------------------------------------------- main
if [ "$UNINSTALL" = 1 ]; then
    do_uninstall
else
    do_install
fi
