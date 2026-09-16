# ratex

[![CI](https://github.com/leoliu0/ratex/actions/workflows/ci.yml/badge.svg)](https://github.com/leoliu0/ratex/actions/workflows/ci.yml)
[![Release](https://github.com/leoliu0/ratex/actions/workflows/release.yml/badge.svg)](https://github.com/leoliu0/ratex/actions/workflows/release.yml)
[![AUR](https://img.shields.io/aur/version/ratex-bin?color=blue)](https://aur.archlinux.org/packages/ratex-bin)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

**ratex** is an ultra-fast, self-contained, pure-Rust TeX engine and typesetting toolchain. Built from scratch with zero unsafe memory compromises, it serves as a high-performance modern replacement for `pdflatex`, `xelatex`, `lualatex`, and `texmk`.
---

## Performance Highlights

Tested and verified against **3,000 real-world arXiv papers** across mathematics, physics, and computer science:

| Workload | ratex | TeX Live (`pdflatex` / `latexmk`) | Advantage |
|---|---|---|---|
| **Incremental Rebuild (Warm)** | **$0.8 - 8.2\text{ ms}$** | $40 - 60\text{ ms}$ | **$10\times - 70\times$ FASTER** ⚡ |
| **Short Papers (1–3 pages) Cold** | **$11 - 13\text{ ms}$** | $39 - 41\text{ ms}$ | **$3.1\times - 3.6\times$ FASTER** ⚡ |
| **Full 3,000-Paper Corpus Throughput** | **$89\text{ papers / min}$** | $53\text{ papers / min}$ | **$1.7\times$ FASTER** ⚡ |
| **Clean Compiles Across arXiv** | **$2,620\text{ papers}$** | $2,613\text{ papers}$ | **More robust than TeX Live** |
| **Visual Document Parity** | **$96.39\%$ mean parity** | Baseline ($100\%$) | **Publication-grade visual fidelity** |

---

## Key Features

- **100% Self-Contained**: Embeds the LaTeX format, over 24,000 packages, all AMS math symbols, and CJK (Chinese, Japanese, Korean) fonts directly in the binary. No external TeX Live installation needed.
- **Drop-in Engine Replacement**: Replaces `pdflatex`, `xelatex`, and `lualatex` everywhere. On macOS and Windows, optionally replaces `latexmk` with `texmk` for zero-configuration IDE compatibility.
- **Sub-10ms Incremental Builds**: Built-in cryptographic dependency graph and auxiliary state validator enables near-instant rebuilds on document edits.
- **Memory-Safe Pure Rust**: Written with strict bounds checks, eliminating buffer overflows, segfaults, and memory corruption bugs common in legacy C TeX engines.
- **Compiler-Grade Diagnostics**: Beautiful rustc-style error reporting with physical source line excerpts, underlines, and actionable fix suggestions.

---

## Installation

| Platform | Package Format | Download / Command |
|---|---|---|
| **macOS (Apple Silicon)** | Native `.pkg` | [Download ratex-v0.1.0-macos-arm64.pkg](https://github.com/leoliu0/ratex/releases/download/v0.1.0/ratex-v0.1.0-macos-arm64.pkg) |
| **macOS (Intel)** | Native `.pkg` | [Download ratex-v0.1.0-macos-x86_64.pkg](https://github.com/leoliu0/ratex/releases/download/v0.1.0/ratex-v0.1.0-macos-x86_64.pkg) |
| **Windows (x64)** | Setup Wizard `.exe` | [Download ratex-setup-v0.1.0-windows-x64.exe](https://github.com/leoliu0/ratex/releases/download/v0.1.0/ratex-setup-v0.1.0-windows-x64.exe) |
| **Arch Linux / Manjaro** | AUR (`ratex-bin`) | `yay -S ratex-bin` (or `paru -S ratex-bin`) |
| **Ubuntu / Debian** | `.deb` (x86_64) | [Download ratex_0.1.0_amd64.deb](https://github.com/leoliu0/ratex/releases/download/v0.1.0/ratex_0.1.0_amd64.deb) |
| **Fedora / RHEL / openSUSE** | `.rpm` (x86_64) | [Download ratex-0.1.0-1.x86_64.rpm](https://github.com/leoliu0/ratex/releases/download/v0.1.0/ratex-0.1.0-1.x86_64.rpm) |
| **Universal Linux** | Standalone `.tar.gz` | [Download tex-suite-v0.1.0-linux-x86_64.tar.gz](https://github.com/leoliu0/ratex/releases/download/v0.1.0/tex-suite-v0.1.0-linux-x86_64.tar.gz) |

### macOS Installation
Download the `.pkg` installer above and double-click to install into `/usr/local/bin` (sets up `PATH` automatically).

### Windows Installation
Download `ratex-setup-v0.1.0-windows-x64.exe` and follow the setup wizard. It automatically adds ratex to your `PATH` and prompts to configure editor compatibility.

### Arch Linux (AUR)
```bash
yay -S ratex-bin    # pre-compiled binary
# or: yay -S ratex  # build from source
```

### Ubuntu / Debian (`.deb`)
```bash
sudo apt install ./ratex_0.1.0_amd64.deb
```

---

## How to Use

### 1. Command Line (CLI)

```bash
# Multi-pass auto-converging build (replaces latexmk; converges bibtex and citations automatically)
texmk paper.tex

# Output PDF to a specific directory
texmk -output-directory=build paper.tex

# Clean auxiliary build cache
texmk -c

# Direct single-pass compile (drop-in pdflatex replacement)
pdflatex paper.tex
```

### 2. TeXstudio Setup
- If you selected the `latexmk` replacement option in the Mac/Windows installer, **TeXstudio works out of the box** (press `F5`).
- **Manual configuration:**
  1. Open **Options** &rarr; **Configure TeXstudio** &rarr; **Build**.
  2. Set **Default Compiler** to:
     ```text
     texmk -pdf -interaction=nonstopmode %.tex
     ```
  3. Press **F5** to compile.

### 3. VS Code (LaTeX Workshop) Setup
Add this recipe to your VS Code `settings.json`:

```json
"latex-workshop.latex.tools": [
  {
    "name": "ratex",
    "command": "texmk",
    "args": ["-pdf", "-interaction=nonstopmode", "%DOC%"]
  }
],
"latex-workshop.latex.recipes": [
  { "name": "ratex", "tools": ["ratex"] }
]
```

---

## Build from Source

Requirements: Rust 1.80+ (`cargo`).

```bash
git clone https://github.com/leoliu0/ratex.git
cd ratex
cargo build --release --bin texmk

# Binary is generated at target/release/texmk
./target/release/texmk paper.tex
```

---

## Architecture

The project is structured as a modular Cargo workspace:

```
ratex/
├── crates/
│   ├── tex-core/     # Pure-Rust TeX state machine, math layout, line breaking, and PDF generator
│   ├── tex-kpse/     # In-memory package resolver, font loader, and kpathsea emulator
│   ├── tex-bibtex/   # Native pure-Rust BibTeX interpreter
│   └── tex-cli/      # Unified multi-pass driver, CLI aliases, and artifact cache
├── packaging/        # Standalone cross-platform distribution installers (Linux, macOS, Windows)
└── scripts/          # Corpus testing, benchmark suites, and packaging tools
```

---

## License

Dual-licensed under either:
- **MIT License** ([LICENSE-MIT](LICENSE-MIT))
- **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE))
