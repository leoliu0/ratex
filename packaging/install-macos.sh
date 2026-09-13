#!/bin/sh
# install-macos.sh — installer for the Rust TeX engine suite on macOS.
#
# Installs pdflatex/xelatex/lualatex/bibtex/texmk/latexmk plus the runtime
# data directory (pdflatex.fmt + texmf overlay), clears Gatekeeper
# quarantine attributes, and wires up ~/.zshrc / ~/.bash_profile.
#
# Works from an extracted release bundle (bin/ + share/tex-suite/ beside
# this script, or tex-suite-macos-<arch>/ subdir) or a cargo checkout
# (--from-source).
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
Usage: install-macos.sh [options]

Install the Rust TeX suite (pdflatex, xelatex, lualatex, bibtex, texmk,
latexmk) and its runtime data (pdflatex.fmt, texmf overlay) on macOS.
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

[ -n "$PREFIX" ] || PREFIX="${HOME:?HOME is not set}/.local"
if [ -z "$DATA_DIR" ]; then
    if [ "$APP_SUPPORT" = 1 ]; then
        DATA_DIR="$HOME/Library/Application Support/tex-suite"
    else
        DATA_DIR="$PREFIX/share/tex-suite"
    fi
fi

case "$PREFIX" in
    /*) ;;
    *)  die "--prefix must be an absolute path (got: $PREFIX)" ;;
esac

BIN_DIR="$PREFIX/bin"

if [ "$(id -u)" != 0 ] && [ "$PREFIX" = "/usr/local" ]; then
    warn "/usr/local is admin-owned on macOS; sudo is usually required for --system."
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
        # bundle dirs use the aarch64 token (package_dist.py); accept arm64 too
        for _tok in "$ARCH" aarch64 arm64; do
            _d="$SCRIPT_DIR/tex-suite-macos-$_tok"
            if [ -d "$_d" ] && try_bundle "$_d"; then log "Using release bundle at $_d"; return 0; fi
        done
        for _cand in "$SCRIPT_DIR"/tex-suite-macos-*/; do
            [ -d "$_cand" ] || continue
            _cand=${_cand%/}
            if try_bundle "$_cand"; then log "Using release bundle at $_cand"; return 0; fi
        done
    fi

    _repo=$(find_repo_root) || die "no release bundle found and this is not a cargo checkout; pass --bundle DIR or run from the repo"
    _target="${CARGO_TARGET_DIR:-$_repo/target}/release"
    if [ "$FROM_SOURCE" = 1 ] || [ ! -x "$_target/pdflatex" ]; then
        [ "$NO_BUILD" = 1 ] && die "--no-build given but a build is required (target/release incomplete)"
        command -v cargo >/dev/null 2>&1 || die "cargo not found; install Rust (https://rustup.rs) or pass --bundle DIR"
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

# ------------------------------------------------------------ quarantine mgmt
clear_quarantine() {
    command -v xattr >/dev/null 2>&1 || { warn "xattr not found; skipping quarantine clear"; return 0; }
    for _t in $BIN_NAMES; do
        _p="$BIN_DIR/$_t"
        if [ -e "$_p" ] || [ -L "$_p" ]; then
            xattr -dr com.apple.quarantine "$_p" 2>/dev/null \
                || xattr -d com.apple.quarantine "$_p" 2>/dev/null \
                || true
        fi
    done
    [ -d "$DATA_DIR" ] && xattr -dr com.apple.quarantine "$DATA_DIR" 2>/dev/null || true
    # Ad-hoc sign so Gatekeeper never kills a quarantined copy that slipped
    # through (codeship-free local builds are unsigned otherwise).
    if command -v codesign >/dev/null 2>&1; then
        for _t in $BIN_NAMES; do
            case "$_t" in *.fmt) continue ;; esac
            [ -f "$BIN_DIR/$_t" ] && codesign --force --sign - "$BIN_DIR/$_t" >/dev/null 2>&1 || true
        done
    fi
    log "Cleared com.apple.quarantine on installed binaries."
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
        if [ "$(dscl . -read "/Users/$USER" UserShell 2>/dev/null)" = "UserShell: /bin/zsh" ]; then
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

    clear_quarantine
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
  - open a new terminal (or:  source ~/.zshrc )
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
    # legacy Application Support location, if it was created by an older run
    _legacy="$HOME/Library/Application Support/tex-suite"
    if [ "$DATA_DIR" != "$_legacy" ] && [ -d "$_legacy" ]; then
        rm -rf -- "$_legacy" && log "  removed $_legacy (legacy data dir)" \
            || { warn "could not remove $_legacy"; _rc=1; }
    fi
    log "Cleaning shell profiles"
    for _f in "$HOME/.zshrc" "$HOME/.bash_profile" "$HOME/.bashrc" "$HOME/.profile"; do
        strip_block "$_f" || true
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
