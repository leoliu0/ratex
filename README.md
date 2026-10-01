# ratex

[![CI](https://github.com/leoliu0/ratex/actions/workflows/ci.yml/badge.svg)](https://github.com/leoliu0/ratex/actions/workflows/ci.yml)
[![Release](https://github.com/leoliu0/ratex/actions/workflows/release.yml/badge.svg)](https://github.com/leoliu0/ratex/actions/workflows/release.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

**ratex** is a self-contained TeX toolchain written in Rust: a
pdfTeX-compatible engine with XeTeX- and LuaTeX-style modes, a latexmk-style
build driver, BibTeX, and an embedded TeX package archive, shipped as one
executable named `ratex`.

## Verification

The Linux release workflow runs the font and graphics fixtures listed in
[`scripts/fixtures/fonts/manifest.json`](scripts/fixtures/fonts/manifest.json)
through the built binary (`scripts/test_fonts.py`) with filesystem isolation,
pdf.js and Poppler rendering/text extraction, and TeX Live 2026 as the
reference. The extracted archive is run on every release platform, and the
shell installer, `.deb`, macOS `.pkg`, and Windows installers are installed
and exercised. The workspace test suite,
the C and WebAssembly libraries, and the browser module are tested as well.
See [PERFORMANCE.md](PERFORMANCE.md) for how speed is measured.

---

## Key Features

- **Validated incremental builds**: Dependency and auxiliary-state checks reuse unchanged results without re-running the typesetting engine. Per-job locks protect active compilations from cache cleanup. See [ARTIFACTS.md](ARTIFACTS.md).
- **Self-contained typesetting**: The executable embeds the LaTeX formats, package resources, and the font families described below. Compilation needs neither TeX Live nor runtime font downloads.
- **One executable**: `ratex` contains the TeX engine, package resolver, BibTeX, and the build driver. It runs each TeX and BibTeX pass in a child process of the same executable, which isolates crashes and memory use between passes. EPS figures are converted by the built-in PostScript interpreter.
- **Engine personalities**: invoked through a link named `pdflatex`, `xelatex`, or `lualatex`, the executable runs a single engine pass; named `bibtex` it runs BibTeX; named `latexmk` it behaves like `ratex`.
- **SyncTeX by default**: A PDF-adjacent `.synctex.gz` maps rendered text to source lines for forward/inverse search in editors such as VS Code, TeXstudio, VimTeX, and AUCTeX.
- **Structured diagnostics**: Errors show the physical source location, an excerpt with a caret, macro-expansion and include context, and a hint when one is known. See [DIAGNOSTICS.md](DIAGNOSTICS.md).
- **SVG images**: `\includegraphics` accepts `.svg` files; they are rasterized in memory to PNG without calling Inkscape.
- **`latexdiff`**: `ratex latexdiff old.tex new.tex` marks up token-level differences with `\DIFadd`/`\DIFdel`. If a system `latexdiff` is on `PATH` it is tried first; otherwise the built-in diff is used.
- **Embedded C API (`libtex`) & WebAssembly (`tex.wasm`)**: Compile complete LaTeX documents in memory from C/C++, Node.js, or browsers. These libraries run every TeX and BibTeX pass inside the calling process, without subprocesses or disk access. Rust programs can call the same pipeline through the `tex-runtime` crate. See [docs/libraries.md](docs/libraries.md).
- **Rust implementation**: `unsafe` code is limited to libc calls (file locks, resource limits, local time), an AVX2 token scan, the C ABI, and the embedded Lua VM.

---

## Installation

### Linux
Download the native package for your distribution from [GitHub Releases](https://github.com/leoliu0/ratex/releases/tag/v0.4.6):

```bash
# Ubuntu / Debian (.deb)
sudo apt install ./ratex_0.4.6_amd64.deb

# Fedora / RHEL / openSUSE (.rpm)
sudo dnf install ./ratex-0.4.6-1.x86_64.rpm

# Arch Linux (AUR): prebuilt binary or source build
yay -S ratex-bin
yay -S ratex

# Any Linux (archive with installer; installs to ~/.local by default)
tar -xzf tex-suite-v0.4.6-linux-x86_64.tar.gz && ./tex-suite-linux-x86_64/install.sh
```

### macOS
Install the maintained Homebrew package (updated automatically after successful releases):

```bash
brew tap leoliu0/ratex https://github.com/leoliu0/ratex.git
brew install leoliu0/ratex/ratex
```

This tap is independent of `homebrew/core`; `brew install ratex` uses core's separately reviewed version.

Or download and run the native installer package:
- [macOS Apple Silicon (.pkg)](https://github.com/leoliu0/ratex/releases/download/v0.4.6/ratex-v0.4.6-macos-aarch64.pkg)
- [macOS Intel (.pkg)](https://github.com/leoliu0/ratex/releases/download/v0.4.6/ratex-v0.4.6-macos-x86_64.pkg)

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
- [Download Windows Setup (.exe)](https://github.com/leoliu0/ratex/releases/download/v0.4.6/ratex-setup-v0.4.6-windows-x64.exe)

---

## Usage

### Single-Command Build
`ratex` tracks dependencies, resolves packages from its embedded archive, runs BibTeX when the auxiliary state requires it, and repeats TeX passes (at most five) until the auxiliary files stop changing:

```bash
# Compile a document
ratex paper.tex

# Write the PDF and SyncTeX file to another directory
ratex -output-directory=build paper.tex

# Remove the document's cached state (keeps the PDF)
ratex -c paper.tex

# Also remove the PDF and SyncTeX file if ratex created them and they are unchanged
ratex -C paper.tex
```

Other options (`ratex --help` prints the full list): `-aux-directory DIR`,
`--cache-directory DIR`, `-jobname NAME`, `--keep-intermediates`/`-k`,
`--keep-logs`, `--optimize-pdf-size`, `-interaction=MODE` (default
`nonstopmode`), `-halt-on-error`, `--verbose`/`-V`, and the engine selectors
`-pdf`, `-xelatex`, `-lualatex`. Without a selector the pdfLaTeX-compatible
mode is used. Other options starting with `-` are passed to the engine.

Exit status: 0 when the build converged, 1 on an engine or BibTeX failure or
no convergence, 2 on a usage error.

The single-pass personalities (`pdflatex`, `lualatex`, `xelatex` links) take
pdfTeX's web2c options (`pdflatex --help`). `-ini` dumps `JOBNAME.fmt` into
the output directory and `-fmt=NAME`, `&NAME`, a `%&NAME` first line, or
`-progname=NAME` load such a Ratex dump (`NAME` equal to the program selects
the built-in format). `-cnf-line=VAR=VALUE` sets a search or policy variable
such as `TEXINPUTS` or `openout_any`; `-kpathsea-debug=N` (nonzero) traces
file lookups in the transcript. `-translate-file=TCXNAME`, `-8bit` and a
`%&-translate-file=` first line select which bytes 128-255 print as
themselves rather than as `^^xx`: `pdflatex` and the built-in format use
TeX Live's `cp227.tcx` table, `-ini` without a table prints `^^` notation,
and the transcript and terminal carry the exact bytes TeX Live writes
(`max_print_line` counts printed bytes). Deliberate differences: e-TeX is
always on (also under `-ini` without `-etex`), missing files are never
generated (`-mktex`), and `-output-format=dvi`, `-enc`, `-mltex`, `-ipc` are
rejected. As in TeX's nonstop mode, a primitive `\input` of a missing file or
an `\openout` that cannot be opened stops the job, since no other name can be
supplied, and so does a terminal `\read`. `\pdffilesize`, `\pdfmdfivesum
file`, `\pdffilemoddate` and `\pdffiledump` search the TeX input path only
(kpse_find_tex), so a TFM, encoding or map file is not found by them.
`\pdffilemoddate` reports a file's modification time like pdfTeX;
files served from the embedded package archive have no timestamp and report
`D:19700101000000Z`.

Environment variables:
- `TEX_RS_CACHE_DIR`: cache root (default: `$XDG_CACHE_HOME/tex-rs` or
  `~/.cache/tex-rs` on Linux, `~/Library/Caches/tex-rs` on macOS,
  `%LOCALAPPDATA%\tex-rs\cache` on Windows).
- `SOURCE_DATE_EPOCH`: fixed UTC value for `\year`, `\month`, `\day`, and
  `\time` (and therefore `\today`).
- `NO_COLOR`, `CLICOLOR=0`, `CLICOLOR_FORCE=1`: control colored diagnostics.

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

**Optional aliases:** the release packages install only `ratex`, so they do
not replace TeX Live's `latexmk` or other compiler commands. To opt into a
Ratex-backed `latexmk` command for your editor, create an isolated alias:

```bash
mkdir -p "$HOME/.local/ratex-editor/bin"
ln -s "$(command -v ratex)" "$HOME/.local/ratex-editor/bin/latexmk"
```

Set TeXstudio's **Commands → Latexmk** executable to that alias's absolute
path, with `-pdf -interaction=nonstopmode "%.tex"` as its arguments.
Do not add this directory to PATH unless you explicitly want other programs
to select the alias too. To undo it, remove only that symlink and restore
your previous TeXstudio command. `ln -s` deliberately refuses to overwrite
an existing file. Links named `pdflatex`, `xelatex`, `lualatex`, or `bibtex`
work the same way and run a single engine or BibTeX pass.


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
# Compare two versions and write the marked-up source:
ratex latexdiff old.tex new.tex diff.tex

# Then compile the diff to PDF:
ratex diff.tex
```

### Fonts and Unicode

The bundled font inventory includes Latin Modern text/math and native OTF faces,
CM-Super with EC/LH metrics, LGR Greek, Wadalab Japanese, IPA/IPAex,
Harano Aji, Arphic Chinese, Korean UHC/Un-fonts and Nanum, `stmaryrd`, and
`bbding`. Classic `CJKutf8` families `min`, `goth`, `gbsn`, `gkai`, `bsmi`,
`bkai`, and `mj` use their own matching metrics and outlines, without system fonts.
Exact package versions, hashes, and resource paths are in
[`packages.lock.json`](crates/tex-kpse/assets/packages.lock.json).

Ratex's `fontspec` and `xeCJK` support selects real native fonts, also in the
default pdfLaTeX-compatible mode:

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
Face options (`UprightFont`, `BoldFont`, `ItalicFont`, `BoldItalicFont`,
`SlantedFont`, `BoldSlantedFont`, `SmallCapsFont`, including the `*` shorthand)
select faces as in fontspec. When a family has no face for a requested shape
(IPAex fonts, for example, have no bold or italic), Ratex follows fontspec under
XeTeX: it uses the nearest available shape and prints a font-shape warning.
Missing font files or families, unsupported features, missing glyphs, and
forbidden embedding remain errors.
Use project-local font files or bundled names; Ratex does not search OS font stores.

Mapped TrueType, CFF OpenType, and collection faces are embedded as CID fonts
with glyph addressing and Unicode extraction maps. Subsets are shared across
pages, sizes, aliases, and forms. Type 1 fonts retain their Type 1 representation.
Font licenses, notices, and required corresponding sources ship under
`share/tex-suite/texmf/doc/fonts`; the engine's MIT/Apache license does not
replace those licenses.

**Engine modes and limits:** `-xelatex` runs Ratex's XeTeX-compatible mode with
the embedded XeLaTeX format; it is not the XeTeX program and says so on the
terminal. `-lualatex` runs the LuaTeX-compatible mode with the embedded
LuaLaTeX format and an in-tree Lua VM, so `\directlua` works. `unicode-math`
(OpenType math), `luatexja`, and `ctex` font sets do not currently compile in
any mode; use classic LaTeX mathematics and `CJKutf8` or the native font
selectors above. Native fonts must be selected after loading a format; dumping
native font state is rejected rather than silently losing it.

The Lua VM (`tex-lua`) is an ordinary Rust library (`rlib`). Native Lua C
modules loaded with `require`/`package.loadlib` resolve the `lua_*`/`luaL_*`
functions from the host executable, which the workspace links with
`--export-dynamic` (`.cargo/config.toml`); a host that embeds `tex-lua` must
link the same way. Host Rust code that needs to allocate inside a native
callback uses `Lua::create_callback`, whose `CallbackLua` is only borrowed for
the duration of the call. In Lua 5.3 mode (LuaTeX's dialect) every byte >= 0x80
is a letter in a name, so the names `LuaFunction::get_upvalue` and
`set_upvalue` return are the bytes of the source text (`Vec<u8>`), and error
messages, `debug.getlocal`/`getupvalue` and `string.dump` keep them. An error
raised by a function handle that a callback calls while a coroutine (an async
script, or one resumed from Lua) runs reaches that coroutine's `pcall` as the
same Lua value; only a call made by the host alone, with no Lua code running,
returns the message with its stack traceback.

---

## Build from Source

Requirements: a current stable Rust toolchain (CI builds with `stable`; the
code uses APIs stabilized in Rust 1.88).
```bash
git clone https://github.com/leoliu0/ratex.git
cd ratex
cargo build --release --locked --bin ratex

# Install into ~/.local (builds the workspace first if needed):
./install.sh --from-source

# Or install system-wide into /usr/local:
sudo ./install.sh --from-source --system
```

Developer notes (binary dispatch, driver algorithm, debugging variables,
test harnesses) are in [docs/internals.md](docs/internals.md).

---

## Architecture

For the native C API and browser/Node.js WebAssembly module, see
[Building and using libtex](docs/libraries.md).

The project is a Cargo workspace:

```
ratex/
├── crates/
│   ├── tex-core/        # TeX engine: expansion, typesetting, math, alignment, pages, PDF output, SyncTeX
│   ├── tex-kpse/        # kpathsea-style resolver and the embedded zstd-compressed package archive
│   ├── tex-bibtex/      # BibTeX implementation
│   ├── tex-cli/         # `ratex` executable: build driver, engine/BibTeX personalities, latexdiff
│   ├── tex-lua/         # Lua VM used by the LuaTeX-compatible mode
│   ├── tex-mplib/       # MetaPost engine (mplib)
│   ├── tex-ps/          # PostScript/EPS interpreter and PDF renderer
│   ├── tex-runtime/     # in-process, in-memory compilation API
│   ├── libtex/          # C ABI over tex-runtime
│   └── tex-wasm/        # WebAssembly bindings over tex-runtime
├── packaging/           # installers and native packages (Linux, macOS, Windows, AUR)
└── scripts/             # packaging, font/corpus test harnesses, library builds
```

---

## License

Dual-licensed under either:
- **MIT License** ([LICENSE-MIT](LICENSE-MIT))
- **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE))
