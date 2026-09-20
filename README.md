# ratex

[![CI](https://github.com/leoliu0/ratex/actions/workflows/ci.yml/badge.svg)](https://github.com/leoliu0/ratex/actions/workflows/ci.yml)
[![Release](https://github.com/leoliu0/ratex/actions/workflows/release.yml/badge.svg)](https://github.com/leoliu0/ratex/actions/workflows/release.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

**ratex** is an ultra-fast, self-contained, pure-Rust TeX engine and typesetting toolchain. Built from scratch with zero unsafe memory compromises, it provides a high-performance, all-in-one replacement for traditional TeX engines and build tools.


## Verification and Performance

The v0.4.1 Linux candidate passes the 29-case font matrix through both the
packaged standalone binary and an installed copy. Checks use filesystem
isolation, disabled networking, pinned reference fonts, and Poppler/pdf.js
rendering and text extraction. The repaired 106-project public corpus cohort
also compiles cleanly from unchanged sources. The workspace suite and the C and
WebAssembly interfaces pass their execution checks, including Chromium.

The 1,000-project corpus campaign remains a separate release gate. Its failures
are retained rather than hidden by aggregate pixel scores. Historical
3,000-project results did not establish the standalone font coverage above.
See [PERFORMANCE.md](PERFORMANCE.md) for benchmark scope and measurements.

---

## Key Features

- **Validated incremental builds**: Dependency and auxiliary-state checks reuse unchanged results without re-running the typesetting engine.
- **Self-contained typesetting**: The source build embeds the LaTeX format, package resources, and the pinned Latin, Cyrillic, Greek, and CJK font families described below. Compilation needs neither TeX Live nor runtime font downloads.
- **All-in-One Engine & Toolchain**: Combines the TeX engine, package resolver, BibTeX interpreter, and build convergence into a single unified `ratex` command.
- **SyncTeX by Default**: Automatic `.synctex.gz` coordinate generation matching PDF boxes to source lines for instant forward/inverse search in VS Code, TeXstudio, VimTeX, and AUCTeX.
- **Compiler-Grade Diagnostics**: Beautiful rustc-style error reporting with physical source line excerpts, underlines, and actionable fix suggestions streamed directly to the terminal.
- **Native SVG & Vector Graphics**: First-class support for `.svg` via pure-Rust in-memory rasterization directly in `\includegraphics`—no Inkscape or external shell execution required.
- **Built-in `latexdiff`**: Integrated visual document diffing with `ratex latexdiff old.tex new.tex` computing word/token LCS differences and injecting standard revision markup.
- **Embedded C API (`libtex`) & WebAssembly (`tex.wasm`)**: Compile complete LaTeX documents in-memory from C/C++, Node.js, or client-side browser runtimes without spawning subprocesses or touching disk.
- **Memory-Safe Pure Rust**: Written with strict bounds checks, eliminating buffer overflows, segfaults, and memory corruption bugs common in legacy C TeX engines.
---

## Installation

### Linux
Download the native package for your distribution from [GitHub Releases](https://github.com/leoliu0/ratex/releases/tag/v0.4.1):

```bash
# Ubuntu / Debian (.deb)
sudo apt install ./ratex_0.4.1_amd64.deb

# Fedora / RHEL / openSUSE (.rpm)
sudo dnf install ./ratex-0.4.1-1.x86_64.rpm

# Arch Linux (AUR): prebuilt binary or source build
yay -S ratex-bin
yay -S ratex

# Any Linux (Universal Tarball Installer)
tar -xzf tex-suite-v0.4.1-linux-x86_64.tar.gz && sudo ./tex-suite-linux-x86_64/install.sh
```

### macOS
Download and run the native installer package:
- [macOS Apple Silicon (.pkg)](https://github.com/leoliu0/ratex/releases/download/v0.4.1/ratex-v0.4.1-macos-aarch64.pkg)
- [macOS Intel (.pkg)](https://github.com/leoliu0/ratex/releases/download/v0.4.1/ratex-v0.4.1-macos-x86_64.pkg)

### Windows
- [Download Windows Setup (.exe)](https://github.com/leoliu0/ratex/releases/download/v0.4.1/ratex-setup-v0.4.1-windows-x64.exe)

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

### Document Revision Diffing (`latexdiff`)
```bash
# Compare two versions and write visual markup directly:
ratex latexdiff old.tex new.tex diff.tex

# Or compile diff directly to PDF:
ratex diff.tex
```

### Fonts and Unicode in the source build

The bundled font inventory includes Latin Modern text/math and native OTF faces,
CM-Super with EC/LH metrics, LGR Greek, Wadalab Japanese, IPA/IPAex,
Harano Aji, Arphic Chinese, Korean UHC/Un-fonts and Nanum, `stmaryrd`, and
`bbding`. Classic `CJKutf8` families `min`, `goth`, `gbsn`, `gkai`, `bsmi`,
`bkai`, and `mj` use their own matching metrics and outlines, without system fonts.
Exact package versions, hashes, and resource paths are in
[`packages.lock.json`](crates/tex-kpse/assets/packages.lock.json).

Ratex's `fontspec` and `xeCJK` adapters select real native fonts:

```latex
\documentclass{article}
\usepackage{fontspec}
\usepackage{xeCJK}
\setmainfont{Latin Modern Roman}
\setCJKmainfont{IPAexMincho}
\begin{document}
Roman text, \textbf{bold}, \textit{italic}, and 日本語のテスト。
\end{document}
```

Supported commands include `\setmainfont`, `\setsansfont`, `\setmonofont`,
`\fontspec`, `\newfontfamily`, `\newfontface`, `\defaultfontfeatures`,
`\addfontfeatures`, the corresponding CJK family selectors, and NFSS
family/style/size switching. Selection options include `Path`, `Extension`,
explicit style files, `FontIndex`, numeric `Scale`, `Script`, `Language`,
ligatures, kerning, number features, `RawFeature`, and variation coordinates.
The selected face must actually provide the requested style, feature, and glyphs;
missing resources and forbidden embedding are errors, not font substitutions.
Use project-local font files or bundled names; Ratex does not search OS font stores.

Mapped TrueType, CFF OpenType, and collection faces are embedded as CID fonts
with glyph addressing and Unicode extraction maps. Subsets are shared across
pages, sizes, aliases, and forms. Type 1 fonts retain their Type 1 representation.
Font licenses, notices, and required corresponding sources ship under
`share/tex-suite/texmf/doc/fonts`; the engine's MIT/Apache license does not
replace those licenses.

**Engine limits:** these adapters are not XeTeX or LuaTeX emulation.
`-xelatex` and `-lualatex` are compatibility selectors for Ratex, not launches
of those engines. OpenType MATH/`unicode-math`, Lua execution/`luatexja`,
`ctex`, vertical Japanese layout, and full bidirectional paragraph layout
are not supported. Use classic LaTeX mathematics and `CJKutf8` or the native
font selectors above. Native Latin hyphenation uses the existing ASCII-word
patterns; arbitrary Unicode hyphenation is not implied by shaping support.
Native fonts must be selected after loading a format; dumping native font state
is rejected rather than silently losing it.

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

For the native C API and browser/Node.js WebAssembly module, see
[Building and using libtex](docs/libraries.md).

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
