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

## BibTeX compatibility notes

The BibTeX implementation (`crates/tex-bibtex`) targets byte-identical `.bbl`
output with TeX Live's `bibtex` 0.99. Behaviors that matter for parity:

- String limits follow the TeX Live build: `ent_str_size = 500`,
  `glob_str_size = 200000` (`entry.max$`/`global.max$` start from these).
- `.bst` identifiers resolve case-insensitively (e.g. `chicago.bst` uses
  `Volume` for the `volume` field).
- `SORT` is stable: by `sort.key$` bytes, ties keep the current order.
- `write$` output wraps at 79 columns, breaking at whitespace found scanning
  back to index 3 (else forward from 80); continuation lines are indented
  by two spaces.
- Cross-referenced parents are included when cited at least
  `-min-crossrefs` times (default 2).
