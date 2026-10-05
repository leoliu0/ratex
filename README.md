# texres

[![CI](https://github.com/leoliu0/texres/actions/workflows/ci.yml/badge.svg)](https://github.com/leoliu0/texres/actions/workflows/ci.yml)
[![Release](https://github.com/leoliu0/texres/actions/workflows/release.yml/badge.svg)](https://github.com/leoliu0/texres/actions/workflows/release.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

**texres** is a self-contained TeX toolchain written in Rust: pdfTeX-, XeTeX-
and LuaTeX-compatible engines, a latexmk-style build driver, BibTeX, and an
embedded TeX package archive, shipped as one executable named `texres`.

## Verification

The Linux CI and release workflows run the font, graphics and engine fixtures listed in
[`scripts/fixtures/fonts/manifest.json`](scripts/fixtures/fonts/manifest.json)
through the built binary (`scripts/test_fonts.py`) with filesystem isolation,
pdf.js and Poppler rendering/text extraction, and TeX Live 2026 as the
reference. The extracted archive is run on every release platform, and the
shell installer, `.deb`, macOS `.pkg`, and Windows installers are installed
and exercised. The workspace test suite,
the C and WebAssembly libraries, and the browser module are tested as well.
The exact minimum documents from [#17](https://github.com/leoliu0/texres/issues/17)
and [#18](https://github.com/leoliu0/texres/issues/18) run under LuaLaTeX and
XeLaTeX respectively, checking Spanish text, embedded fonts and rendering.
LuaLaTeX and XeLaTeX also have subsystem probes and paired package-interaction
documents: source loading, token scanners, register/group scope, Lua callbacks,
node ownership, fonts, math, Unicode, bidirectional text, CJK line breaking and
vertical typesetting. Their complete extracted text is compared with the matching
TeX Live 2026 engine, alongside font-program checks and both renderers.
Use `scripts/test_fonts.py --engine lualatex` or `--engine xelatex` with
`--texres` and `--output` to select a suite; its report groups failures by engine
and feature family and retains compilation and viewer evidence.
See [PERFORMANCE.md](PERFORMANCE.md) for how speed is measured.

## Issue assistant

The issue bot reads the complete issue and triggering comment, selects the
reported `-pdf`, `-xelatex` or `-lualatex` command, and includes the actual
command, binary version and bounded compiler log in its reply. `/reproduce`,
`/test` and `/fix` reuse the issue's document unless the comment supplies a
replacement; an explicit engine option can override the reported engine.
Suggestions and feature requests without TeX are left for maintainer review,
not answered with an irrelevant request for a compilation snippet. AI
diagnosis receives the full report and treats proposed causes as hypotheses.

---

## Key Features

- **Validated incremental builds**: Dependency and auxiliary-state checks reuse unchanged results without re-running the typesetting engine. Per-job locks protect active compilations from cache cleanup. See [ARTIFACTS.md](ARTIFACTS.md).
- **Self-contained typesetting**: The executable embeds the LaTeX formats, package resources, and the font families described below. Compilation needs neither TeX Live nor runtime font downloads.
- **One executable**: `texres` contains the TeX engine, package resolver, BibTeX, and the build driver. It runs each TeX and BibTeX pass in a child process of the same executable, which isolates crashes and memory use between passes. EPS figures are converted by the built-in PostScript interpreter.
- **Engine personalities**: invoked through a link named `pdflatex`, `xelatex`, or `lualatex`, the executable runs a single engine pass; named `bibtex` it runs BibTeX; named `latexmk` it behaves like `texres`.
- **SyncTeX by default**: A PDF-adjacent `.synctex.gz` maps rendered text to source lines for forward/inverse search in editors such as VS Code, TeXstudio, VimTeX, and AUCTeX.
- **Structured diagnostics**: Errors show the physical source location, an excerpt with a caret, macro-expansion and include context, and a hint when one is known. See [DIAGNOSTICS.md](DIAGNOSTICS.md).
- **SVG images**: `\includegraphics` accepts `.svg` files; they are rasterized in memory to PNG without calling Inkscape.
- **`latexdiff`**: `texres latexdiff old.tex new.tex` marks up token-level differences with `\DIFadd`/`\DIFdel`. If a system `latexdiff` is on `PATH` it is tried first; otherwise the built-in diff is used.
- **Embedded C API (`libtex`) & WebAssembly (`tex.wasm`)**: Compile complete LaTeX documents in memory from C/C++, Node.js, or browsers. These libraries run every TeX and BibTeX pass inside the calling process, without subprocesses or disk access. Rust programs can call the same pipeline through the `tex-runtime` crate. See [docs/libraries.md](docs/libraries.md).
- **Rust implementation**: `unsafe` code is limited to libc calls (file locks, resource limits, local time), an AVX2 token scan, the C ABI, and the embedded Lua VM.

---

## Installation

### Linux
Download the native package for your distribution from [GitHub Releases](https://github.com/leoliu0/texres/releases/tag/v0.6.0):

```bash
# Ubuntu / Debian (.deb)
sudo apt install ./texres_0.6.0_amd64.deb

# Fedora / RHEL / openSUSE (.rpm)
sudo dnf install ./texres-0.6.0-1.x86_64.rpm

# Arch Linux (AUR; still published under the former name, currently 0.4.6)
yay -S ratex-bin
yay -S ratex

# Linux with glibc (archive with installer; installs to ~/.local by default)
tar -xzf tex-suite-v0.6.0-linux-x86_64.tar.gz && ./tex-suite-linux-x86_64/install.sh
```

Linux releases support **x86_64 and ARM64 (aarch64)**. ARM64 prebuilt binaries
require Debian 12 or another distribution with **glibc 2.36 or newer**, including
Linux Docker containers on Apple Silicon. Alpine Linux and other musl-based
distributions are not supported by these prebuilt binaries.

For ARM64, download the [Debian package](https://github.com/leoliu0/texres/releases/download/v0.6.0/texres_0.6.0_arm64.deb),
[RPM](https://github.com/leoliu0/texres/releases/download/v0.6.0/texres-0.6.0-1.aarch64.rpm),
[Arch package](https://github.com/leoliu0/texres/releases/download/v0.6.0/texres-0.6.0-1-aarch64.pkg.tar.zst),
or [archive](https://github.com/leoliu0/texres/releases/download/v0.6.0/tex-suite-v0.6.0-linux-aarch64.tar.gz):

```bash
# Ubuntu / Debian ARM64
sudo apt install ./texres_0.6.0_arm64.deb

# Fedora / RHEL / openSUSE ARM64
sudo dnf install ./texres-0.6.0-1.aarch64.rpm

# Arch Linux ARM (downloaded native package)
sudo pacman -U ./texres-0.6.0-1-aarch64.pkg.tar.zst

# ARM64 archive; installs to ~/.local by default
tar -xzf tex-suite-v0.6.0-linux-aarch64.tar.gz && ./tex-suite-linux-aarch64/install.sh
```

For a Debian 12 ARM64 container on Apple Silicon, start
`docker run --rm -it --platform linux/arm64 debian:12 bash`, then run:

```bash
apt-get update && apt-get install -y ca-certificates curl
curl -fLO https://github.com/leoliu0/texres/releases/download/v0.6.0/texres_0.6.0_arm64.deb
apt-get install -y ./texres_0.6.0_arm64.deb
texres --version
```

### macOS
Install the maintained Homebrew package (updated automatically after successful releases):

```bash
brew tap leoliu0/texres https://github.com/leoliu0/texres.git
brew install leoliu0/texres/texres
```

This tap is independent of `homebrew/core`; `brew install texres` uses core's separately reviewed version.

Or download and run the native installer package:
- [macOS Apple Silicon (.pkg)](https://github.com/leoliu0/texres/releases/download/v0.6.0/texres-v0.6.0-macos-aarch64.pkg)
- [macOS Intel (.pkg)](https://github.com/leoliu0/texres/releases/download/v0.6.0/texres-v0.6.0-macos-x86_64.pkg)

#### Migrating from the GitHub installer to Homebrew

Homebrew does not overwrite an installation in `~/.local` or a GitHub `.pkg`
installation in `/usr/local`. Remove or stop selecting the old copy before
switching editors.

1. In Terminal, run `type -a texres` to identify existing copies.
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
   cannot establish ownership. Inspect `pkgutil --files io.github.leoliu0.texres`
   for a `.pkg` installation. Move only confirmed old TeXres files aside before
   Homebrew links into `/usr/local`; do not delete other TeX tools or use
   `brew link --overwrite`. `pkgutil --forget` alone does not remove files.
4. Install the maintained tap using the commands above. Open a new Terminal,
   run `type -a texres` and `"$(brew --prefix)/bin/texres" --version`, then configure
   TeXstudio with that absolute Homebrew path as described below.

If core's `texres` is already installed, use `brew uninstall texres` before
installing `leoliu0/texres/texres`. Neither uninstall your documents nor TeX Live
just to change which executable your editor uses.

### Windows
- [Download Windows Setup (.exe)](https://github.com/leoliu0/texres/releases/download/v0.6.0/texres-setup-v0.6.0-windows-x64.exe)

---

## Usage

### Single-Command Build
`texres` tracks dependencies, resolves packages from its embedded archive, runs BibTeX or the embedded pure-Rust Biber for biblatex's default backend when auxiliary state requires it, and repeats TeX passes (at most five) until the auxiliary files stop changing. A link named `biber` runs Biber directly with its CLI options, including `--quiet` and `--output-directory`:

```bash
# Compile a document
texres paper.tex

# Write the PDF and SyncTeX file to another directory
texres -output-directory=build paper.tex

# Remove the document's cached state (keeps the PDF)
texres -c paper.tex

# Also remove the PDF and SyncTeX file if texres created them and they are unchanged
texres -C paper.tex
```

Other options (`texres --help` prints the full list): `-aux-directory DIR`,
`--cache-directory DIR`, `-jobname NAME`, `--keep-intermediates`/`-k`,
`--keep-logs`, `--optimize-pdf-size`, `-interaction=MODE` (default
`nonstopmode`), `-halt-on-error`, `--verbose`/`-V`, and the engine selectors
`-pdf`, `-xelatex`, `-lualatex`. Without a selector, a document whose preamble
loads `fontspec`, `xeCJK`, `ctex` (or a `ctex` class), `unicode-math`, or
`polyglossia` runs as XeLaTeX (a Unicode engine is required, as in TeX Live);
any other document runs as pdfLaTeX. Other options starting with `-` are
passed to the engine.

Exit status: 0 when the build converged, 1 on an engine or BibTeX failure or
no convergence, 2 on a usage error.

The single-pass personalities (`pdflatex`, `lualatex`, `xelatex` links) take
pdfTeX's web2c options (`pdflatex --help`). `-ini` dumps `JOBNAME.fmt` into
the output directory and `-fmt=NAME`, `&NAME`, a `%&NAME` first line, or
`-progname=NAME` load such a TeXres dump (`NAME` equal to the program selects
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

PDF output follows pdfTeX's own bookkeeping: font, form and image resources
are named `/F<n>`, `/Fm<n>` and `/Im<n>` from the owner font number and the
per-document form/image counts, `\pdfuniqueresname` appends pdfTeX's
CRC-32/base-62 job tag, and objects are numbered in creation order from 1.
The Info dictionary lists Producer, user `\pdfinfo` keys, Creator, dates,
Trapped and `PTEX.Fullbanner` (`PTEX_Fullbanner` with `\pdfptexuseunderscore`,
absent under `\pdfsuppressptexinfo`), and `\pdftrailerid` fixes the `/ID`.
Under LuaTeX the Producer is `LuaTeX-1.24.0` and the banner key is always
`PTEX.FullBanner` (luatex ignores `\pdfsuppressptexinfo` and the underscore
spelling).
`-ini` starts with pdfTeX's `\pdfminorversion=4` and `\pdfcompresslevel=9`.
There is no DVI writer, so `\pdfoutput` starts at 1 where TeX Live's `-ini`
starts at 0.

Environment variables:
- `TEX_RS_CACHE_DIR`: cache root (default: `$XDG_CACHE_HOME/tex-rs` or
  `~/.cache/tex-rs` on Linux, `~/Library/Caches/tex-rs` on macOS,
  `%LOCALAPPDATA%\tex-rs\cache` on Windows).
- `SOURCE_DATE_EPOCH`: fixed UTC value for `\year`, `\month`, `\day`, and
  `\time` (and therefore `\today`).
- `NO_COLOR`, `CLICOLOR=0`, `CLICOLOR_FORCE=1`: control colored diagnostics.

### Editor Setup
Configure your editor or build system to invoke `texres`:
#### TeXstudio Setup
1. Find the executable in Terminal: `command -v texres`. For Homebrew, use
   `echo "$(brew --prefix)/bin/texres"` and verify that path with `--version`.
   Typical paths are `/opt/homebrew/bin/texres` (Apple Silicon) and
   `/usr/local/bin/texres` (Intel). Use your actual prefix.
2. Open **Options → Configure TeXstudio → Commands** (on macOS,
   **TeXstudio → Preferences → Commands**). Set **Latexmk** to the command
   below, replacing the executable path with yours:
   ```text
   "/opt/homebrew/bin/texres" -pdf -interaction=nonstopmode "%.tex"
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

**Optional aliases:** the release packages install only `texres`, so they do
not replace TeX Live's `latexmk` or other compiler commands. To opt into a
TeXres-backed `latexmk` command for your editor, create an isolated alias:

```bash
mkdir -p "$HOME/.local/texres-editor/bin"
ln -s "$(command -v texres)" "$HOME/.local/texres-editor/bin/latexmk"
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
    "name": "texres",
    "command": "texres",
    "args": ["-pdf", "-interaction=nonstopmode", "%DOC%"]
  }
],
"latex-workshop.latex.recipes": [
  { "name": "texres", "tools": ["texres"] }
]
```

The compiler writes `document.synctex.gz` beside `document.pdf`, including
when `-output-directory` selects another directory. Keep both files together
for editor forward/inverse search. `hyperref` internal links and table-of-contents
entries resolve to clickable PDF destinations.

### Document Revision Diffing (`latexdiff`)
```bash
# Compare two versions and write the marked-up source:
texres latexdiff old.tex new.tex diff.tex

# Then compile the diff to PDF:
texres diff.tex
```

### Fonts and Unicode

The bundled font inventory includes Latin Modern text/math and native OTF faces,
CM-Super with EC/LH metrics, LGR Greek, Wadalab Japanese, IPA/IPAex,
Harano Aji, Arphic Chinese, Korean UHC/Un-fonts and Nanum, `stmaryrd`, and
`bbding`. Classic `CJKutf8` families `min`, `goth`, `gbsn`, `gkai`, `bsmi`,
`bkai`, and `mj` use their own matching metrics and outlines, without system fonts.
Exact package versions, hashes, and resource paths are in
[`packages.lock.json`](crates/tex-kpse/assets/packages.lock.json).

Documents that use `fontspec`, `xeCJK`, `ctex`, `unicode-math`, or `polyglossia`
run on the XeTeX engine (see *Engine modes and limits* below) with the
upstream TeX Live packages, so native fonts behave as in TeX Live's `xelatex`:

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

The font syntax of XeTeX (`"Family/B:feature"`, `"[file.otf]:+liga"`,
`mapping=tex-text`, `color=`, `embolden=`, ...) and fontspec's whole interface
(`\setmainfont`, `\newfontfamily`, `\setCJKmainfont`, `Path`, `Extension`,
`BoldFont`, `FontIndex`, `Scale`, OpenType features, ...) are the upstream
implementations. Their diagnostics are TeX Live's as well: a missing font ends
the run with fontspec's `The font "..." cannot be found` error, a missing
shape produces `Font shape ... undefined`, and a character that a font lacks
is reported as `Missing character: There is no ...`.
Fonts are hermetic: a font is found among the project's files, the bundled
font archive (by file name, or by family, PostScript, or full name from the
bundled font index), and nothing else. TeXres does not search OS font stores,
so a document that selects a system font by name (`Times New Roman`) fails
where TeX Live with that font installed succeeds; ship the font file with the
project and select it with `Path=./`. The bundled OpenType fonts include
Latin Modern (text and math), TeX Gyre (text and math), STIX Two, XITS,
Libertinus, Harano Aji, IPAex, Fandol (the default fonts of `ctex`), and
the other families listed in the lock file.

Mapped TrueType, CFF OpenType, and collection faces are embedded as CID fonts
with glyph addressing and Unicode extraction maps. Subsets are shared across
pages, sizes, aliases, and forms. Type 1 fonts retain their Type 1 representation.
Font licenses, notices, and required corresponding sources ship under
`share/tex-suite/texmf/doc/fonts`; the engine's MIT/Apache license does not
replace those licenses.

**Engine modes and limits:** `-xelatex` (or a link named `xelatex`) runs the
XeTeX engine, version 3.141592653-2.6-0.999998 as in TeX Live 2026, with the
embedded XeLaTeX format built from TeX Live's `xelatex.ini`; the terminal
banner reads `This is XeTeX, Version 3.141592653-2.6-0.999998 (TeXres x.y.z)`.
Output goes to the PDF directly, without an XDV file: `\special`s are
interpreted as `xdvipdfmx` does, pages default to A4 unless `\pdfpagewidth`
and `\pdfpageheight` are set, and the PDF carries xdvipdfmx's producer data.
Shell escape (`\write18`) is never run. pdfLaTeX has no native fonts: loading
`fontspec` there fails with fontspec's own engine error, as in TeX Live.
`-lualatex` runs the LuaTeX-compatible mode with the embedded LuaLaTeX format
and an in-tree Lua VM, so `\directlua` works. `luatexja` and a few LuaTeX-only
packages do not compile in any mode. Native fonts are loaded after the format
is read, as in XeTeX; dumping native font state is rejected.

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
With the optional `sandbox` feature, `SandboxConfig::with_stdlib(Stdlib::Bit32)`
enables Lua 5.3's `bit32` independently of the `math` library; it is hidden until
selected.

---

## Build from Source

Requirements: a current stable Rust toolchain (CI builds with `stable`; the
code uses APIs stabilized in Rust 1.88).
```bash
git clone https://github.com/leoliu0/texres.git
cd texres
cargo build --release --locked --bin texres

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
texres/
├── crates/
│   ├── tex-core/        # TeX engine: expansion, typesetting, math, alignment, pages, PDF output, SyncTeX
│   ├── tex-kpse/        # kpathsea-style resolver and the embedded zstd-compressed package archive
│   ├── tex-bibtex/      # BibTeX implementation
│   ├── tex-cli/         # `texres` executable: build driver, engine/BibTeX personalities, latexdiff
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
