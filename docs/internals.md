# Ratex internals

Developer notes for working on the workspace. User-facing behavior is in
[README.md](../README.md), caches and packaging in [ARTIFACTS.md](../ARTIFACTS.md),
error reporting in [DIAGNOSTICS.md](../DIAGNOSTICS.md), measurement in
[PERFORMANCE.md](../PERFORMANCE.md), and the embedding APIs in
[libraries.md](libraries.md).

## Where the algorithms are documented

Typesetting algorithms are documented in the module comments of
`crates/tex-core/src`, next to the code:

| Module | Topic |
| --- | --- |
| `linebreak.rs` | Knuth–Plass paragraph breaking (port of tex.web `line_break`) |
| `hyphen.rs` | Liang pattern trie and `\hyphenation` exceptions |
| `boxes.rs` | node lists, glue, `hpack`/`vpack` |
| `build.rs` | list building, paragraphs, page-builder hook |
| `page.rs` | page builder (`build_page`), inserts, `\output` |
| `math.rs` | math lists to horizontal lists (TeX82 Appendix G) |
| `align.rs` | `\halign`/`\valign`: preamble, cells, spans, `\noalign` |
| `format.rs` | `.fmt` dump/load wire format |
| `node_arena.rs` | generation-checked node storage |
| `pdffile.rs`, `pdf_fonts.rs` | PDF serialization, Type 1 parsing and embedding |
| `writet1.rs` | writet1.c port: Type 1 FontFile cleartext/`/Encoding`/eexec/Subrs rewrite, `/Length1-3` |
| `pdf_images.rs`, `pdf_encodings.rs` | pdftoepdf port: one shared `PdfSource` per included file; with `\pdfinclusioncopyfonts=0` Type 1/Type1C fonts the font map knows are replaced by the map's program (xpdf base-encoding tables) |
| `diagnostics.rs` | structured diagnostics and their output bounds |

## Executables and dispatch

`crates/tex-cli` defines these Cargo binaries: `ratex`, `texmk`, `pdflatex`,
`xelatex`, `lualatex`, `tex-bibtex`, and `latexdiff`. Releases ship only
`ratex`.

- `ratex` (`src/bin/ratex.rs`) is `texmk::main`. It decides what to run in this
  order:
  1. `TEXMK_INTERNAL_MODE=engine|bibtex|latexdiff` (set by the driver for its
     child processes; the engine program name comes from
     `TEX_SUITE_PROGRAM_NAME`, default `pdflatex`);
  2. a first argument `latexdiff`;
  3. the invoked file name: `pdflatex`, `xelatex`, `lualatex` run one engine
     pass, `bibtex`/`tex-bibtex` run BibTeX, `latexdiff` runs the diff, and
     anything else (`ratex`, `texmk`, `latexmk`) runs the build driver.

  Engine and BibTeX personalities started this way set `TEX_RS_HERMETIC=1`,
  so they resolve TeX files only from the embedded archive (see below).
- The `pdflatex` Cargo binary calls the engine entry point directly and does
  **not** set `TEX_RS_HERMETIC`, so it also searches `TEXINPUTS`, `TEXMFHOME`,
  executable-relative `texmf` trees, and system TeX trees. Use `ratex` through
  a link named `pdflatex` to reproduce shipped behavior.
- The `xelatex` and `lualatex` Cargo binaries are the small launcher in
  `src/bin/launcher.rs`: it re-executes the sibling `texmk` executable with
  `TEXMK_INTERNAL_MODE` and `TEX_SUITE_PROGRAM_NAME` set.
- `crates/tex-kpse` has a `tex-index OUTPUT_DIRECTORY TEXMF_ROOT...` binary
  that prebuilds filename indexes for `ls-R` databases of system TeX trees
  (non-hermetic use only). The engine looks for indexes in `TEX_INDEX_DIR`, or
  in `tex-index-data` beside the executable; an empty `TEX_INDEX_DIR` disables
  them.

`TEXMK_LIB=DIR` makes the driver run `DIR/<engine>` (or `DIR/pdflatex`) and
`DIR/tex-bibtex` (or `DIR/bibtex`) instead of re-executing itself; tests use it
to inject tools.

## Build driver algorithm (`src/bin/texmk.rs`)

1. Parse options, set `TEX_RS_HERMETIC=1`, convert EPS figures in the source
   directory, and select the engine (`-pdf`/`-xelatex`/`-lualatex`, otherwise
   pdfLaTeX). Locate the per-job cache directory and take its lock.
2. Each pass (at most `MAX_PASSES = 5`): snapshot every file in the auxiliary
   directory (size and hash), run the engine child with
   `-interaction=nonstopmode` unless the user chose a mode, and stream its
   output through a bounded capture (128 KiB head plus tail, 1 MiB per
   stream).
3. Signals are scanned from the output and the `.log`: `Rerun to get`,
   `Label(s) may have changed`, `There were undefined references/citations`,
   per-entry `Citation ... undefined`, and `No file <job>.bbl`.
4. If the engine reported a result-cache hit and the auxiliary graph
   (`.aux` plus `\@input` children, bounded in depth, count, and size) is
   complete, the build is converged.
5. BibTeX runs when the aux has `\bibdata`, its inputs exist, and any of these
   hold: the `.bbl` is missing, the citation set changed since the previous
   pass, the bibliography signature changed (tool identity, BibTeX-related
   environment, `\citation`/`\bibdata`/`\bibstyle` commands, and the resolved
   `.bib`/`.bst` files and their hashes),
   the `.bbl` no longer matches what BibTeX wrote, the aux graph is
   incomplete, or citations are undefined on a first build. A current
   project-supplied `<job>.bbl` is adopted instead of running BibTeX; if
   BibTeX fails and the project has a `.bbl`, that file is used. After a
   successful BibTeX run another pass follows.
6. The build is stable when the auxiliary snapshot did not change during the
   pass, or on a first pass that started from existing auxiliary state when
   there are no pending signals and the only auxiliary files are `.aux` files containing nothing but inert lines
   (`\relax`, `\@abspage@last`, and a few `\providecommand` lines). A sticky
   `Rerun to get` message alone does not force another pass.
7. Without stability after five passes the build fails. On success the PDF
   and SyncTeX file are published atomically and the manifest is written;
   see [ARTIFACTS.md](../ARTIFACTS.md) for cache contents and cleanup.

## Resource resolution

In hermetic mode (`TEX_RS_HERMETIC` set to anything but empty, `0`, or
`false`) `tex_kpse::Kpse` searches the project directory, the embedded
zstd-chunked package archive, and `$TEX_SUITE_DATA/texmf` (or
`$RATEX_DATA_DIR/texmf`) when set. Otherwise it additionally searches
`TEXMFHOME`, `TEXMFVAR`, `TEXMFCONFIG`, `TEXMFLOCAL`, `TEXMFDIST`, `~/texmf`,
`~/.texlive/texmf-var`, executable-relative `texmf`/`share/tex-suite/texmf`
trees, and the standard system roots, and honors `TEXINPUTS`, `TFMFONTS`,
`VFFONTS`, `TTFONTS`, and similar path variables.

The embedded packages and their pinned versions are listed in
`crates/tex-kpse/assets/packages.lock.json`; formats are embedded from
`crates/tex-cli/assets/*.fmt.zst`.

## Debugging and tuning variables

| Variable | Effect |
| --- | --- |
| `PHASE_TIMING=1` | print per-phase times of an engine pass to stderr |
| `TEXDEBUG=lookups` | append `[kpse:lookup]` lines for TeX input lookups to the transcript |
| `TEXDEBUG=TEX_PDF_SERIAL` | disable the page-compression and font-subset worker threads |
| `TEX_MEM_LIMIT_MIB=N` | engine resident-memory limit in MiB (default 512, `0` disables); on Unix the address-space limit is set to 4×N or 2 GiB, whichever is larger |
| `TEX_EXPANSION_LIMIT=N` | opt-in cap on cumulative expansions (default 0 = unlimited) |
| `TEX_RS_CACHE_DIR=DIR` | cache root for the driver and engine result caches |
| `TEX_INDEX_DIR=DIR` | location of `tex-index` filename indexes (empty disables) |
| `TEXMK_LIB=DIR` | driver uses tools from `DIR` instead of itself |
| `SOURCE_DATE_EPOCH=N` | fixed `\year`/`\month`/`\day`/`\time` |

`TEXDEBUG` takes a comma-separated list of flags.

## Test harnesses

- `crates/tex-core/tests/oracle_probe.rs`: differential micro-probes that run
  small INITEX files through `/usr/bin/pdflatex -ini -etex` and the Rust
  engine and compare what each writes with `\immediate\write`.
- `crates/tex-core/tests/doc_parity.rs` (ignored by default): exact 150-DPI
  RGB comparison of prepared `*-rust` and `*-reference` PDFs; run with
  `TEX_PARITY_ROOT=DIR cargo test -p tex-core --test doc_parity -- --ignored`.
  Requires PyMuPDF and NumPy.
- `scripts/test_fonts.py`: the font and graphics fixture gate used by the
  release workflow (isolation, pdf.js/Poppler rendering, TeX Live reference).
- `scripts/test_corpus.py`: corpus compilation against a reference TeX Live;
  `--mode campaign` (default) converges both engines and gates on raster
  parity, `--mode single-pass` is a diagnostic only.
- `scripts/bench_cold.py`: fresh-process timing harness; see
  [PERFORMANCE.md](../PERFORMANCE.md).

## BibTeX (`crates/tex-bibtex`)

One engine, a module-by-module port of `bibtex.web` 0.99e plus TeX Live's
`bibtex.ch`. `tex_bibtex::run(args, version)` is the only entry point; the
`bibtex` binary, `ratex`/`texmk` (`crates/tex-cli/src/bibtex/mod.rs`) and
tex-runtime call it.

- `input.rs`: character classes (bytes 128-255 are letters) and the line
  scanner shared by the `.aux`, `.bst` and `.bib` readers. `engine.rs`: state,
  `run`, file lookup. `aux.rs`, `bst.rs`, `bib.rs`: the readers; `.bst`
  commands run as soon as they are read, so messages interleave as in BibTeX.
  `exec.rs`: stack machine and built-ins; `text.rs`: string built-ins
  (`purify$`, `change.case$`, `width$`, `text.prefix$`, `substring$`,
  `add.period$`). `log.rs`: terminal and `.blg` output.
- Ground truth is TeX Live 2026 `bibtex`: the `.bbl`, the `.blg` messages and
  the exit status match byte for byte. The `.blg` banner names Ratex, TeX
  Live's usage statistics are not written, and files from the embedded
  archive are announced as `<embedded:NAME>`.
- Exit status: 0 spotless or warnings, 2 error messages (the `.bbl` is
  complete; callers continue with it), 3 fatal, 1 unreadable `.aux` or bad
  command line.
- TeX Live sizes: `ent_str_size = 500`, `glob_str_size = 200000` (these seed
  `entry.max$`/`global.max$`), `max_print_line = 79`.
- Flags: `-terse`, `-min-crossrefs=N` (default 2); other options are reported
  on stderr and ignored.
- Tests: `crates/tex-bibtex/tests/oracle.rs` runs every case directory under
  `tests/oracle/` (aux files and `args`; shared styles and databases in
  `shared/`) against `expected.{bbl,blg,status}` produced by
  `/usr/bin/bibtex` with the banner and usage statistics removed. To add a
  case, create the directory, generate the expected files with TeX Live, and
  list it in `oracle_cases!`.
