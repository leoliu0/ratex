# ratex

[![CI](https://github.com/leoliu0/ratex/actions/workflows/ci.yml/badge.svg)](https://github.com/leoliu0/ratex/actions/workflows/ci.yml)
[![Release](https://github.com/leoliu0/ratex/actions/workflows/release.yml/badge.svg)](https://github.com/leoliu0/ratex/actions/workflows/release.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

**ratex** is an ultra-fast, self-contained, pure-Rust TeX engine and typesetting toolchain. Built from scratch with zero unsafe memory compromises, it provides a high-performance, all-in-one replacement for traditional TeX engines and build tools.


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
- **All-in-One Engine & Toolchain**: Combines the TeX engine, package resolver, BibTeX interpreter, and build convergence into a single unified `ratex` command.
- **Sub-10ms Incremental Builds**: Built-in cryptographic dependency graph and auxiliary state validator enables near-instant rebuilds on document edits.
- **Memory-Safe Pure Rust**: Written with strict bounds checks, eliminating buffer overflows, segfaults, and memory corruption bugs common in legacy C TeX engines.
- **Compiler-Grade Diagnostics**: Beautiful rustc-style error reporting with physical source line excerpts, underlines, and actionable fix suggestions.

---

## Installation

### Linux
Download the native package for your distribution from [GitHub Releases](https://github.com/leoliu0/ratex/releases/tag/v0.2.0):

```bash
# Ubuntu / Debian (.deb)
sudo apt install ./ratex_0.2.0_amd64.deb

# Fedora / RHEL / openSUSE (.rpm)
sudo dnf install ./ratex-0.2.0-1.x86_64.rpm

# Arch Linux (AUR)
yay -S ratex-bin   # or: paru -S ratex-bin

# Any Linux (Universal Tarball Installer)
tar -xzf tex-suite-v0.2.0-linux-x86_64.tar.gz && sudo ./tex-suite-linux-x86_64/install.sh
```

### macOS
Download and run the native installer package:
- [macOS Apple Silicon (.pkg)](https://github.com/leoliu0/ratex/releases/download/v0.2.0/ratex-v0.2.0-macos-aarch64.pkg)
- [macOS Intel (.pkg)](https://github.com/leoliu0/ratex/releases/download/v0.2.0/ratex-v0.2.0-macos-x86_64.pkg)

### Windows
- [Download Windows Setup (.exe)](https://github.com/leoliu0/ratex/releases/download/v0.2.0/ratex-setup-v0.2.0-windows-x64.exe)

---

## Usage

### Single-Command Build
`ratex` is an all-in-one compiler. It automatically tracks dependencies, resolves packages in memory, runs embedded BibTeX passes, and converges auxiliary state in milliseconds:

```bash
# Compile document (automatically converges bibtex and cross-references)
ratex paper.tex

# Output PDF to a specific directory
ratex -output-directory=build paper.tex

# Clean auxiliary build artifacts and cache
ratex -c
```

### Editor Setup
Configure your editor or build system to invoke `ratex`:
#### TeXstudio Setup
- **Manual configuration:**
  1. Open **Options** &rarr; **Configure TeXstudio** &rarr; **Build**.
  2. Set **Default Compiler** to:
     ```text
     ratex -pdf -interaction=nonstopmode %.tex
     ```
  3. Press **F5** to compile.

#### VS Code (LaTeX Workshop) Setup
Add this recipe to your VS Code `settings.json`:

```json
"latex-workshop.latex.tools": [
  {
    "name": "ratex",
    "command": "ratex",
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
cargo build --release

# Install locally into ~/.local/bin:
./install.sh --prefix ~/.local

# Or install system-wide into /usr/local/bin:
sudo ./install.sh
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
