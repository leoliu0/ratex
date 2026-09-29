# ratex

[![CI](https://github.com/leoliu0/ratex/actions/workflows/ci.yml/badge.svg)](https://github.com/leoliu0/ratex/actions/workflows/ci.yml)
[![Release](https://github.com/leoliu0/ratex/actions/workflows/release.yml/badge.svg)](https://github.com/leoliu0/ratex/actions/workflows/release.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

**ratex** is an ultra-fast, self-contained, pure-Rust TeX engine and typesetting toolchain. Built from scratch with zero unsafe memory compromises, it provides a high-performance, all-in-one replacement for traditional TeX engines and build tools.


## Verification and Performance

The Linux release gates run all 39 font and graphics fixtures through both
the packaged standalone binary and an installed copy. Checks use filesystem
isolation, disabled networking, pinned reference fonts, and Poppler/pdf.js
rendering and text extraction. The workspace suite, C and WebAssembly
interfaces, and Chromium browser module are also exercised.

On the frozen 1,000-project corpus, Ratex compiles 942 projects from unchanged
sources, up from 927. Genuine `latexmk` compiles 891: 883 with its default
bibliography rules, plus eight using their existing shipped bibliographies.
All 891 also compile with Ratex, and no previous Ratex successes were lost.
The remaining 58 fail in both runs. This compares compilation coverage, not
whole-document visual parity; the standalone fixture suite checks rendering
and font embedding separately. Historical 3,000-project results did not establish
the standalone font coverage above.
See [PERFORMANCE.md](PERFORMANCE.md) for benchmark scope and measurements.

---

## Key Features

- **Validated incremental builds**: Dependency and auxiliary-state checks reuse unchanged results without re-running the typesetting engine. Per-job locks protect active compilations from cache cleanup, including concurrent startup.
- **Self-contained typesetting**: The source build embeds the LaTeX format, package resources, and the pinned Latin, Cyrillic, Greek, and CJK font families described below. Compilation needs neither TeX Live nor runtime font downloads.
- **All-in-One Engine & Toolchain**: Combines the TeX engine, package resolver, BibTeX interpreter, and build convergence into a single unified `ratex` command.
- **SyncTeX by Default**: A PDF-adjacent `.synctex.gz` maps rendered text to source lines for forward/inverse search in VS Code, TeXstudio, VimTeX, and AUCTeX.
- **Compiler-Grade Diagnostics**: Beautiful rustc-style error reporting with physical source line excerpts, underlines, and actionable fix suggestions streamed directly to the terminal.
- **Native SVG & Vector Graphics**: First-class support for `.svg` via pure-Rust in-memory rasterization directly in `\includegraphics`—no Inkscape or external shell execution required.
- **Built-in `latexdiff`**: Integrated visual document diffing with `ratex latexdiff old.tex new.tex` computing word/token LCS differences and injecting standard revision markup.
- **Embedded C API (`libtex`) & WebAssembly (`tex.wasm`)**: Compile complete LaTeX documents in-memory from C/C++, Node.js, or client-side browser runtimes without spawning subprocesses or touching disk.
- **Memory-Safe Pure Rust**: Written with strict bounds checks, eliminating buffer overflows, segfaults, and memory corruption bugs common in legacy C TeX engines.
---

## Installation

### Linux
Download the native package for your distribution from [GitHub Releases](https://github.com/leoliu0/ratex/releases/tag/v0.4.5):

```bash
# Ubuntu / Debian (.deb)
sudo apt install ./ratex_0.4.5_amd64.deb

# Fedora / RHEL / openSUSE (.rpm)
sudo dnf install ./ratex-0.4.5-1.x86_64.rpm

# Arch Linux (AUR): prebuilt binary or source build
yay -S ratex-bin
yay -S ratex

# Any Linux (Universal Tarball Installer)
tar -xzf tex-suite-v0.4.5-linux-x86_64.tar.gz && sudo ./tex-suite-linux-x86_64/install.sh
```

### macOS
Install the maintained Homebrew package (updated automatically after successful releases):

```bash
brew tap leoliu0/ratex https://github.com/leoliu0/ratex.git
brew install leoliu0/ratex/ratex
```

This tap is independent of `homebrew/core`; `brew install ratex` uses core's separately reviewed version.

Download and run the native installer package:
- [macOS Apple Silicon (.pkg)](https://github.com/leoliu0/ratex/releases/download/v0.4.5/ratex-v0.4.5-macos-aarch64.pkg)
- [macOS Intel (.pkg)](https://github.com/leoliu0/ratex/releases/download/v0.4.5/ratex-v0.4.5-macos-x86_64.pkg)

#### Migrating from the GitHub installer to Homebrew

Homebrew does not overwrite an installation in `~/.local` or a GitHub `.pkg`
installation in `/usr/local`. Remove or stop selecting the old copy before
switching editors.

1. In Terminal, run `type -a ratex` to identify existing copies.
2. For an installation made with the archive's `install.sh`, run its uninstaller
   from the extracted archive with the **same prefix and data directory** used
   during installation:
   ```bash
   ./install.sh --uninstall --prefix "$HOME/.local"
   # System install, only if you originally used --system:
   sudo ./install.sh --uninstall --prefix /usr/local
   ```
   Include your original `--data-dir` or `--app-support` option if used.
   The uninstaller preserves unmanaged files; a missing ownership manifest means
   it will not remove that installation. Do not run it against a Homebrew prefix.
3. For the native GitHub `.pkg` or a manually copied binary, the shell uninstaller
   cannot establish ownership. Inspect `pkgutil --files io.github.leoliu0.ratex`
   for a `.pkg` installation. Move only confirmed old Ratex files aside before
   Homebrew links into `/usr/local`; do not delete other TeX tools or use
   `brew link --overwrite`. `pkgutil --forget` alone does not remove files.
4. Install the maintained tap using the commands above. Open a new Terminal,
   run `type -a ratex` and `"$(brew --prefix)/bin/ratex" --version`, then configure
   TeXstudio with that absolute Homebrew path as described below.

If core's `ratex` is already installed, use `brew uninstall ratex` before
installing `leoliu0/ratex/ratex`. Neither uninstall your documents nor TeX Live
just to change which executable your editor uses.

### Windows
- [Download Windows Setup (.exe)](https://github.com/leoliu0/ratex/releases/download/v0.4.5/ratex-setup-v0.4.5-windows-x64.exe)

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
1. Find the executable in Terminal: `command -v ratex`. For Homebrew, use
   `echo "$(brew --prefix)/bin/ratex"` and verify that path with `--version`.
   Typical paths are `/opt/homebrew/bin/ratex` (Apple Silicon) and
   `/usr/local/bin/ratex` (Intel). Use your actual prefix.
2. Open **Options → Configure TeXstudio → Commands** (on macOS,
   **TeXstudio → Preferences → Commands**). Set **Latexmk** to the command
   below, replacing the executable path with yours:
   ```text
   "/opt/homebrew/bin/ratex" -pdf -interaction=nonstopmode "%.tex"
   ```
3. Under **Build**, select **Latexmk** as **Default Compiler** and
   **Compile & View** as **Build & View**. The command belongs in **Commands**,
   not in the Default Compiler selector. Press **F6** to compile or **F5**
   to compile and view.

An absolute executable path avoids macOS GUI apps depending on your Terminal
PATH. Quoting `"%.tex"` handles project paths containing spaces. Do not paste
`$(brew --prefix)` into TeXstudio: its command field is not a shell.
Keep the PDF and `.synctex.gz` together for the internal viewer's source navigation.
See the [TeXstudio command documentation](https://texstudio-org.github.io/configuration.html#configuring-the-latex-related-commands).

**Optional aliases:** the maintained Homebrew tap installs only `ratex`, so it
does not replace TeX Live's `latexmk` or other compiler commands. To opt into a
Ratex-backed `latexmk` command for your editor, create an isolated alias:

```bash
mkdir -p "$HOME/.local/ratex-editor/bin"
ln -s "$(brew --prefix)/bin/ratex" "$HOME/.local/ratex-editor/bin/latexmk"
```

Set TeXstudio's **Commands → Latexmk** executable to that alias's absolute
path, with `-pdf -interaction=nonstopmode "%.tex"` as its arguments.
Do not add this directory to PATH unless you explicitly want other programs
to select the alias too. To undo it, remove only that symlink and restore
your previous TeXstudio command. `ln -s` deliberately refuses to overwrite
an existing file.


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

The compiler writes `document.synctex.gz` beside `document.pdf`, including
when `-output-directory` selects another directory. Keep both files together
for editor forward/inverse search. `hyperref` internal links and table-of-contents
entries resolve to clickable PDF destinations.

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
