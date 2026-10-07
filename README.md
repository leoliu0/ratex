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
sudo apt install ./texres_0.7.3_amd64.deb

# Fedora / RHEL / openSUSE
sudo dnf install ./texres-0.7.3-1.x86_64.rpm

# Arch (AUR)
yay -S texres-bin    # or build from source: yay -S texres

# Any other glibc distribution (installs to ~/.local)
tar -xzf tex-suite-v0.7.3-linux-x86_64.tar.gz && ./tex-suite-linux-x86_64/install.sh
```

ARM64 builds need glibc 2.36 or newer (Debian 12 or later), which includes
Linux Docker containers on Apple Silicon. Alpine and other musl systems are
not supported.

### Windows

1. Download `texres-setup-…-windows-x64.exe` from the
   [latest release](https://github.com/leoliu0/texres/releases/latest) and run it.
2. Keep **Add texres to environment PATH** ticked. It installs to
   `C:\Program Files\texres\bin\texres.exe`.
3. Open a new PowerShell or Command Prompt window (old windows don't see the
   new `PATH`) and check:

   ```powershell
   texres --version
   ```

For a portable copy without the installer, unzip
`tex-suite-…-windows-x86_64.zip` and run `bin\texres.exe` from there.

### From source

```bash
git clone https://github.com/leoliu0/texres.git && cd texres
cargo build --release --locked --bin texres   # needs stable Rust (1.88+)
./install.sh --from-source                     # into ~/.local; add --system for /usr/local
```

## Usage

**Build a document.** `texres paper.tex` writes `paper.pdf` and
`paper.synctex.gz` next to the source. It runs LaTeX, BibTeX or Biber and the
index tools as often as needed. A second run with no changes takes a few
milliseconds.

**Choose the engine.** Documents that load `fontspec`, `xeCJK`, `ctex`,
`unicode-math` or `polyglossia` run as XeLaTeX, everything else as pdfLaTeX.
To choose yourself, add `-pdf`, `-xelatex` or `-lualatex`:

```bash
texres -lualatex paper.tex
```

**Write the output elsewhere.** `texres -outdir=build paper.tex` puts the PDF
in `build/`. Auxiliary files (`.aux`, `.bbl`, `.toc`) live in a private cache,
not in your project. `-k` copies them next to the PDF.

**Rebuild on every save.** `texres -pvc paper.tex` builds, then rebuilds when a
file the build read changes. Ctrl-C stops it.

```text
$ texres -pvc paper.tex
texmk: [14:02:11] build OK (3 pages, 1.42 s)
texmk: watching 4 files (Ctrl-C to stop)
texmk: [14:02:40] changed: intro.tex
texmk: [14:02:41] build OK (3 pages, 0.36 s)
```

**Clean up.** `texres -c paper.tex` removes the cached build state and keeps
the PDF. `texres -C paper.tex` also removes the PDF and SyncTeX file.

**Read errors.** Errors print with the file, line and a hint:

```text
error: Undefined control sequence \printtotl
  --> chapters/results.tex:1:9
  |
1 | Result: \printtotl
  |         ^^^^^^^^^^
  = help: check the command spelling; if a package defines it, load that package before use
```

The full TeX log stays in the cache; `texres` prints its path when a build
fails. `--keep-logs` copies it to `paper.log` next to the PDF. So do the
options editors pass: `-interaction=...`, `-file-line-error` and
`-synctex=...`. `-verbose` also prints TeX's own output. With
`-interaction=nonstopmode` a document with errors still gets a PDF, and the
exit status is 1. See [DIAGNOSTICS.md](DIAGNOSTICS.md).

**Bibliographies.** `\bibliography` runs the built-in BibTeX. `biblatex`
(default `backend=biber`) runs the built-in Biber, which writes the same
`.bbl` as Biber 2.22.

**Fonts.** Fonts come from your project folder and the bundled set (Latin
Modern, TeX Gyre, STIX Two, Libertinus, IPAex, Harano Aji, Fandol and others).
System fonts are not used. To use another font, put the file next to your
document and load it with `Path=./`.

**Compare versions.** `texres latexdiff old.tex new.tex diff.tex` writes a
marked-up `diff.tex`; build it with `texres diff.tex`.

**Single tools.** A link to `texres` named `pdflatex`, `xelatex`, `lualatex`,
`bibtex` or `biber` runs one pass of that tool. Named `latexmk`, it behaves
like `texres`. The packages install only `texres`, so an existing TeX Live is
left alone. On Windows, run this in a PowerShell opened with *Run as
administrator*; the hard link takes no extra disk space. Without admin
rights, copy `texres.exe` to a folder of yours on `PATH` under the new name
instead (about 800 MB).

```powershell
New-Item -ItemType HardLink -Path "C:\Program Files\texres\bin\latexmk.exe" -Target "C:\Program Files\texres\bin\texres.exe"
```

**Exit status.** 0: the build finished. 1: TeX, BibTeX or Biber reported an
error, or the build did not settle. 2: bad command line.

`texres --help` lists all options. All commands above work the same in
PowerShell, Command Prompt and macOS/Linux shells; on Windows, `texres` is
`texres.exe`, and Ctrl-C stops `-pvc`.

## Speed

TeXres 0.7.3 was measured against TeX Live 2026 (`latexmk`) on 100 documents:
70 generated and 30 public papers, books and slide decks. pdfLaTeX, XeLaTeX
and LuaLaTeX are all included. The binary is the profile-guided build that
the Linux x86_64 and macOS arm64 releases ship. The machine is one Linux
workstation (64-core Threadripper PRO). Each cell is the median of 5 runs.
Both tools run every pass and the bibliography tool, and the PDFs were
checked to have the same pages and text.

| Scenario | TeXres faster | Median TeXres / TeX Live | Closest document |
| --- | ---: | ---: | --- |
| Cold build | 100 of 100 | 0.60 | 0.98, siunitx tables (3.54 s / 3.63 s) |
| No-change rebuild | 100 of 100 | 0.13 | 0.26, 193-page book (0.030 s / 0.114 s) |
| One-line edit | 100 of 100 | 0.57 | 0.98, 193-page book (6.62 s / 6.77 s) |

On most documents TeXres needs a little over half of TeX Live's time, but on
the slowest two it is only 2-4% ahead. Both cold builds started with the
machine's font database already built. On a new machine the first LuaLaTeX
build scans the fonts for both tools, which takes about a minute either way.
TeXres also writes SyncTeX by default, and that cost is included. Method,
per-group numbers and a script to rerun it are in
[PERFORMANCE.md](PERFORMANCE.md).

## Editor setup

Any editor that can run `latexmk` can run `texres`; it accepts latexmk's
options. Three rules apply to every editor:

1. Point the editor at `texres`.
   - **Windows:** the installer puts `texres` on your `PATH`, so plain `texres`
     works in editors started after the install (restart any that were
     open). The full path is `C:\Program Files\texres\bin\texres.exe`; check
     with `(Get-Command texres).Source` in PowerShell or `where texres` in
     Command Prompt.
   - **macOS and Linux:** use the full path. Find it with `command -v texres`
     (Homebrew: `"$(brew --prefix)/bin/texres"`, usually
     `/opt/homebrew/bin/texres` on Apple Silicon). Apps started from the Dock
     or a desktop menu do not see the `PATH` of your shell.
2. Leave out `-pdf`. It forces pdfLaTeX and turns off the XeLaTeX detection.
   Use `-xelatex` or `-lualatex` only to force those engines.
3. Keep `-interaction=nonstopmode` (or `-file-line-error`, or
   `-synctex=1`). With one of them `texres` writes `paper.log` next to the PDF,
   where editors look for errors.

`texres` writes `paper.synctex.gz` on every build, so jumping between source
and PDF works with any SyncTeX viewer. It does not read `latexmkrc` files, and
it ignores latexmk's viewer options (`-pv`, `-view=...`).

### VS Code (LaTeX Workshop)

Open the settings JSON (Command Palette, *Preferences: Open User Settings
(JSON)*) and add:

```json
"latex-workshop.latex.tools": [
  {
    "name": "texres",
    "command": "/opt/homebrew/bin/texres",
    "args": ["-verbose", "-synctex=1", "-interaction=nonstopmode", "-file-line-error", "-outdir=%OUTDIR%", "%DOC%"]
  }
],
"latex-workshop.latex.recipes": [
  { "name": "texres", "tools": ["texres"] }
]
```

Replace the command with your path. On Windows use
`"command": "texres"`, or the full path with doubled backslashes:
`"C:\\Program Files\\texres\\bin\\texres.exe"`. LaTeX Workshop reads errors
and warnings from what the tool prints, so keep `-verbose`. Build with
Ctrl+Alt+B (or save the file). The built-in PDF viewer (*View LaTeX PDF*)
uses `paper.synctex.gz`: Ctrl+Alt+J jumps from the source to the PDF, and
Ctrl+click in the PDF jumps back to the source.

### TeXstudio

1. Open *Options > Configure TeXstudio > Commands*. Set **Latexmk** to (with
   your path):
   ```text
   "/opt/homebrew/bin/texres" -synctex=1 -interaction=nonstopmode %.tex
   ```
   On Windows:
   ```text
   "C:/Program Files/texres/bin/texres.exe" -synctex=1 -interaction=nonstopmode %.tex
   ```
2. On the *Build* page, set **Default Compiler** to *Latexmk*.

TeXstudio reads errors from `paper.log`. Do not add `-file-line-error`;
TeXstudio expects TeX's `!` lines. The internal viewer jumps to the PDF
position after each build, and Ctrl+click in the PDF goes to the source.

### Vim and Neovim (vimtex)

vimtex runs latexmk; point it at `texres` and drop its default `-pdf`.
In `init.lua`:

```lua
vim.g.vimtex_compiler_latexmk = { executable = "texres" }
vim.g.vimtex_compiler_latexmk_engines = { _ = "" }
vim.g.vimtex_view_method = "zathura"
```

Or in `.vimrc`:

```vim
let g:vimtex_compiler_latexmk = {'executable': 'texres'}
let g:vimtex_compiler_latexmk_engines = {'_': ''}
let g:vimtex_view_method = 'zathura'
```

`\ll` starts continuous mode (`texres -pvc`), and errors appear in the
quickfix list after each build. `% !TeX program = xelatex` on the first line
of a document still selects `-xelatex`. With zathura, `\lv` jumps to the PDF
and Ctrl+click in zathura jumps back to Vim. vimtex also reads `$pdf_mode`
from `~/.latexmkrc`; if that file sets `$pdf_mode = 1`, vimtex adds `-pdf`
again, so remove the line.

For Okular instead of zathura:

```lua
vim.g.vimtex_view_general_viewer = "okular"
vim.g.vimtex_view_general_options = "--unique file:@pdf\\#src:@line@tex"
```

In Okular, set *Settings > Configure Okular > Editor* to *Custom Text Editor*
with the command `nvim --headless -c "VimtexInverseSearch %l '%f'"`, then
Shift+click in the PDF to jump to the source.

On Windows, use SumatraPDF:

```lua
vim.g.vimtex_view_general_viewer = "SumatraPDF"
vim.g.vimtex_view_general_options = "-reuse-instance -forward-search @tex @line @pdf"
```

In SumatraPDF, *Settings > Options > Set inverse search command line*:
`nvim --headless -c "VimtexInverseSearch %l '%f'"`, then double-click in the
PDF to jump to the source.

### Emacs (AUCTeX)

Add a TeXres command and make it the default:

```elisp
(with-eval-after-load 'tex
  (add-to-list 'TeX-command-list
               '("TeXres" "texres -verbose %S%(mode)%(file-line-error) %t"
                 TeX-run-TeX nil (latex-mode LaTeX-mode) :help "Build with TeXres")))
(add-hook 'LaTeX-mode-hook (lambda () (setq TeX-command-default "TeXres")))
```

`C-c C-c` then builds, and `` C-c ` `` steps through the errors. AUCTeX reads
them from the output, so keep `-verbose`. With `TeX-source-correlate-mode`
on, `%S` adds `-synctex=1` and forward search works with the viewer set in
`TeX-view-program-selection`.

### Sublime Text (LaTeXTools)

In *Preferences > Package Settings > LaTeXTools > Settings – User*:

```json
"builder": "traditional",
"builder_settings": {
  "command": ["/opt/homebrew/bin/texres", "-cd", "-f", "-interaction=nonstopmode", "-synctex=1"]
}
```

On Windows, the first entry is `"texres"` or
`"C:\\Program Files\\texres\\bin\\texres.exe"`.

LaTeXTools prints a note that the command does not select the engine; that is
expected, `texres` picks it. Errors come from `paper.log`. Forward and inverse
search use the viewer LaTeXTools is set up for (Skim, SumatraPDF, Okular,
zathura or Evince).

### TeXShop (macOS)

Create `~/Library/TeXShop/Engines/TeXres.engine` with:

```bash
#!/bin/bash
/opt/homebrew/bin/texres -synctex=1 -interaction=nonstopmode "$1"
```

Make it executable (`chmod +x ~/Library/TeXShop/Engines/TeXres.engine`),
restart TeXShop and pick *TeXres* in the engine menu next to *Typeset*. Or put
`% !TEX TS-program = TeXres` on the first line of the document.

### Other editors

If the editor has a latexmk setting, replace `latexmk` with the full path to
`texres` and remove `-pdf`. If it can only run a program called `latexmk`,
make a link: `ln -s "$(command -v texres)" ~/bin/latexmk` on macOS and Linux,
or the `New-Item -ItemType HardLink` command under *Single tools* on Windows
(the link then sits next to `texres.exe`, already on `PATH`).
On macOS and Linux, put `~/bin` early in the editor's `PATH`.

With `-pvc`, `texres` runs the `$compiling_cmd`, `$success_cmd` and
`$failure_cmd` commands that editors set with `-e`, as latexmk does. Other
`-e` code and `-r` are refused with a message, and so are DVI and PostScript
modes (`-dvi`, `-ps`, `-pdfdvi`, `-pdfps`).

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
   TeXstudio with that absolute Homebrew path as described in [Editor setup](#editor-setup).

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
