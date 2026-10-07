# TeXres

[![CI](https://github.com/leoliu0/texres/actions/workflows/ci.yml/badge.svg)](https://github.com/leoliu0/texres/actions/workflows/ci.yml)
[![Release](https://github.com/leoliu0/texres/actions/workflows/release.yml/badge.svg)](https://github.com/leoliu0/texres/actions/workflows/release.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

TeXres is a TeX distribution in a single executable, written in Rust. It
compiles LaTeX documents with pdfLaTeX, XeLaTeX or LuaLaTeX, and includes the
LaTeX formats, a package archive, fonts, BibTeX and Biber. You don't need
TeX Live, and nothing is downloaded during a build.

```bash
texres paper.tex
```

This runs LaTeX, BibTeX or Biber as many times as needed and writes
`paper.pdf`. Output is compared against TeX Live 2026 in CI.

There is also a C library and a WebAssembly module that compile documents in
memory; see [docs/libraries.md](docs/libraries.md).

## Install

### macOS (Homebrew)

```bash
brew install leoliu0/texres/texres
```

Or download the macOS installer (`.pkg`, Apple Silicon or Intel) from the [latest release](https://github.com/leoliu0/texres/releases/latest).

### Linux (x86_64 and ARM64)

Download the package from the [latest release](https://github.com/leoliu0/texres/releases/latest). For ARM64, use the file with `arm64` or `aarch64` in its name.

```bash
# Debian / Ubuntu
sudo apt install ./texres_0.7.2_amd64.deb

# Fedora / RHEL / openSUSE
sudo dnf install ./texres-0.7.2-1.x86_64.rpm

# Arch (AUR)
yay -S texres-bin    # or build from source: yay -S texres

# Any other glibc distribution (installs to ~/.local)
tar -xzf tex-suite-v0.7.2-linux-x86_64.tar.gz && ./tex-suite-linux-x86_64/install.sh
```

ARM64 builds need glibc 2.36 or newer (Debian 12 or later), which includes
Linux Docker containers on Apple Silicon. Alpine and other musl systems are
not supported.

### Windows

Download `texres-setup-…-windows-x64.exe` from the [latest release](https://github.com/leoliu0/texres/releases/latest).

### From source

```bash
git clone https://github.com/leoliu0/texres.git && cd texres
cargo build --release --locked --bin texres   # needs stable Rust (1.88+)
./install.sh --from-source                     # into ~/.local; add --system for /usr/local
```

## Usage

```bash
texres paper.tex                         # engine picked from the preamble
texres -xelatex paper.tex                # or -pdf, -lualatex
texres -output-directory=build paper.tex # write output to build/
texres -pvc paper.tex                    # rebuild whenever an input changes
texres -c paper.tex                      # remove cached build files
texres latexdiff old.tex new.tex diff.tex && texres diff.tex
```

Documents loading `fontspec`, `xeCJK`, `ctex`, `unicode-math` or `polyglossia`
run as XeLaTeX automatically; everything else runs as pdfLaTeX.
`texres --help` lists all options. Exit status: 0 converged, 1 build failed,
2 usage error.

**Watch mode:** `-pvc` (also `--watch`, `-w`) builds once, then rebuilds
whenever a file the last build read changes: the main file, `\input` and
`\include` files, `.bib` files, images, local packages and fonts. It combines
with every build option except `-c`/`-C`. Saves that leave the content
unchanged are ignored, and one editor save gives one rebuild. A failed
rebuild prints its errors and keeps watching; Ctrl-C stops with status 0.

```text
$ texres -pvc paper.tex
texmk: [14:02:11] build OK (3 pages, 1.42 s)
texmk: watching 4 files (Ctrl-C to stop)
texmk: [14:02:40] changed: intro.tex
texmk: [14:02:41] build OK (3 pages, 0.36 s)
texmk: watching 4 files (Ctrl-C to stop)
```

**Bibliographies:** `\bibliography` runs BibTeX. `biblatex` (default
`backend=biber`) runs the built-in Biber, which writes the same `.bbl` as
Biber 2.22.

**Fonts:** fonts are looked up in your project folder and in the bundled set
(Latin Modern, TeX Gyre, STIX Two, Libertinus, IPAex, Harano Aji, Fandol and
others). System fonts are not used. To use another font, put the file next to
your document and load it with `Path=./`.

**Single tools:** a symlink to `texres` named `pdflatex`, `xelatex`,
`lualatex`, `bibtex` or `biber` runs one pass of that tool; named `latexmk` it
behaves like `texres`. The packages install only `texres`, so an existing
TeX Live is left alone.

## Speed

Median wall time over 7 runs on one Linux machine (64-core Threadripper PRO),
TeXres 0.7.2 against TeX Live 2026 (`latexmk`). A cold build starts with an
empty TeXres cache and a clean directory for `latexmk`; both run every pass
and the bibliography tool. The documents are in the repository.

| Document | Cold build, TeXres / TeX Live | One-line edit, TeXres / TeX Live |
| --- | ---: | ---: |
| 113-page thesis, BibTeX | 3.04 s / 3.57 s | 0.77 s / 0.79 s |
| 12-page article, natbib | 1.04 s / 1.76 s | 0.27 s / 0.45 s |
| 12-page article, biblatex | 4.79 s / 5.77 s | 1.34 s / 1.38 s |
| 82-page Beamer deck | 7.88 s / 4.45 s | 4.07 s / 2.30 s |
| 9-page TikZ and pgfplots figures | 24.7 s / 14.9 s | 8.25 s / 5.01 s |
| 7-page LuaLaTeX | 52.5 s / 4.23 s | 1.72 s / 1.47 s |

TeXres is faster on most pdfLaTeX and XeLaTeX documents, and an unchanged
rebuild takes about 10 ms. It is about 1.7 times slower on Beamer and
TikZ/pgfplots, and the first LuaLaTeX build of each document takes about 50
seconds. Method, all scenarios, hardware and a script to rerun it:
[PERFORMANCE.md](PERFORMANCE.md).

## Editor setup

Find the path with `command -v texres` (Homebrew: `"$(brew --prefix)/bin/texres"`).

**TeXstudio:** in *Options > Configure TeXstudio > Commands*, set **Latexmk**
to the following, using your path:

```text
"/opt/homebrew/bin/texres" -pdf -interaction=nonstopmode "%.tex"
```

Then under *Build* choose **Latexmk** as the default compiler. Use the full
path, because macOS apps started from the Dock don't see your shell `PATH`.

**VS Code (LaTeX Workshop):** add to `settings.json`:

```json
"latex-workshop.latex.tools": [
  { "name": "texres", "command": "texres", "args": ["-pdf", "-interaction=nonstopmode", "%DOC%"] }
],
"latex-workshop.latex.recipes": [{ "name": "texres", "tools": ["texres"] }]
```

The `.synctex.gz` file next to the PDF lets the editor jump between source
and PDF.

## Switching from an older install to Homebrew (macOS)

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

## More

- [DIAGNOSTICS.md](DIAGNOSTICS.md): error messages
- [ARTIFACTS.md](ARTIFACTS.md): caching and incremental builds
- [PERFORMANCE.md](PERFORMANCE.md): speed
- [docs/libraries.md](docs/libraries.md): C / WebAssembly / Rust embedding
- [docs/internals.md](docs/internals.md): engine details, architecture, tests
- Environment: `TEX_RS_CACHE_DIR` (cache root), `SOURCE_DATE_EPOCH` (fixed `\today`), `NO_COLOR`

## License

MIT or Apache-2.0, at your option. Bundled fonts and packages keep their own
licenses, shipped under `share/tex-suite/texmf/doc/fonts`.
