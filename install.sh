#!/bin/sh
# install.sh — cross-platform entry point for the Rust TeX suite installer.
#
# Detects the operating system and delegates:
#   Linux   -> packaging/install-linux.sh   (or install-linux.sh at archive root)
#   macOS   -> packaging/install-macos.sh
#   Windows (Git Bash / MSYS2 / Cygwin) -> points at install-windows.ps1 / install.bat
#
# All arguments (including --help and --uninstall) are forwarded verbatim to
# the platform installer. POSIX sh; verified with `sh -n`.

set -eu

usage() {
    cat <<'EOF'
Usage: ./install.sh [options]

Cross-platform installer for the Rust TeX suite: pdflatex, xelatex,
lualatex, bibtex, texmk, latexmk, plus the runtime data directory
(pdflatex.fmt + texmf overlay). Auto-dispatches to the native installer
for Linux, macOS, and (via guidance) Windows.

Modes (auto-detected unless overridden):
  bundle       run this script from an extracted release archive: the
               binaries under bin/ and data under share/tex-suite/ beside
               it are installed directly
  from-source  run it from a cargo checkout (or pass --from-source) to
               build with `cargo build --release --workspace` first

Options (forwarded to the platform installer):
  --prefix DIR     Install prefix (default: $HOME/.local; bins -> DIR/bin,
                   data -> DIR/share/tex-suite)
  --system         Install under /usr/local instead (run with sudo)
  --bundle DIR     Use an explicit extracted bundle directory as source
  --from-source    Build from the cargo workspace and install the result
  --no-build       Fail instead of auto-running cargo build
  --data-dir DIR   Override the runtime data directory
  --link           Symlink binaries into PREFIX/bin instead of copying
  --no-path        Do not edit shell profiles
  --skip-verify    Skip the post-install pdflatex version check
  --uninstall      Remove installed binaries, data dir, and profile block
  -h, --help       Show this help and exit

macOS-only:
  --app-support    Put runtime data in ~/Library/Application Support/tex-suite

Examples:
  ./install.sh                          # user install to ~/.local
  sudo ./install.sh --system            # system install to /usr/local
  ./install.sh --from-source            # cargo build --release, then install
  ./install.sh --uninstall              # clean removal

Windows (Git Bash / MSYS2 / Cygwin): run one of the native installers:
  powershell -ExecutionPolicy Bypass -File packaging/install-windows.ps1
  packaging\install.bat
EOF
}

script_dir() {
    CDPATH= cd -- "$(dirname -- "$0")" && pwd
}

# Locate a child installer: packaging/<name> (source repo) or <name> beside
# this script (extracted archive layout).
find_child() {
    _n="$1"
    _d=$(script_dir)
    if [ -f "$_d/packaging/$_n" ]; then
        printf '%s\n' "$_d/packaging/$_n"
        return 0
    fi
    if [ -f "$_d/$_n" ]; then
        printf '%s\n' "$_d/$_n"
        return 0
    fi
    return 1
}

run_child() {
    _s=$1; shift
    if [ -x "$_s" ]; then
        exec "$_s" "$@"
    else
        exec sh "$_s" "$@"
    fi
}

windows_help() {
    _d=$(script_dir)
    cat <<EOF
Detected a Windows bash-style environment ($(uname -s 2>/dev/null || echo unknown)).
This shell installer does not run natively on Windows. Use one of the
bundled Windows installers instead:

EOF
    if [ -f "$_d/packaging/install-windows.ps1" ] || [ -f "$_d/install-windows.ps1" ]; then
        cat <<'EOF'
  PowerShell (recommended, run from the archive or repo root):
    powershell -ExecutionPolicy Bypass -File .\packaging\install-windows.ps1
    powershell -ExecutionPolicy Bypass -File .\packaging\install-windows.ps1 --uninstall
EOF
    else
        cat <<'EOF'
  PowerShell (recommended):
    powershell -ExecutionPolicy Bypass -File packaging\install-windows.ps1
EOF
    fi
    if [ -f "$_d/packaging/install.bat" ] || [ -f "$_d/install.bat" ]; then
        cat <<'EOF'

  Or from cmd.exe:
    packaging\install.bat
EOF
    fi
    cat <<'EOF'

The Windows installer places binaries and data under
%LOCALAPPDATA%\tex-suite and adds the bin directory to your user PATH.
EOF
}

# Top-level help prints the cross-platform usage, not the child's.
case "${1:-}" in
    -h|--help) usage; exit 0 ;;
esac

# Operating-system dispatch.
case "$(uname -s 2>/dev/null || echo UNKNOWN)" in
    Linux)
        _s=$(find_child install-linux.sh) || {
            printf 'error: install-linux.sh not found next to install.sh or in packaging/\n' >&2
            exit 1
        }
        run_child "$_s" "$@"
        ;;
    Darwin)
        _s=$(find_child install-macos.sh) || {
            printf 'error: install-macos.sh not found next to install.sh or in packaging/\n' >&2
            exit 1
        }
        run_child "$_s" "$@"
        ;;
    MINGW*|MSYS*|CYGWIN*|Windows_NT)
        case "${1:-}" in
            -h|--help) usage; exit 0 ;;
        esac
        windows_help
        exit 1
        ;;
    *)
        printf 'error: unsupported operating system: %s\n' "$(uname -s 2>/dev/null || echo unknown)" >&2
        printf 'Supported: Linux, macOS, Windows (via packaging/install-windows.ps1).\n' >&2
        exit 1
        ;;
esac
