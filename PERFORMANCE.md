# Performance

## What is measured

The distributed executable is `texres` (`target/release/texres`). Invoked as
`texres`, it is the build driver: it re-executes itself for every TeX and BibTeX
pass. To time or profile one engine pass, run the same executable through a
link named `pdflatex` (or `xelatex`/`lualatex`):

```sh
ln -s "$PWD/target/release/texres" /tmp/texres-bench/pdflatex
```

Keep profiling variants in a separate Cargo target directory or under `/tmp`;
do not add suffixed executables to `target/release`.

## TeXres vs TeX Live 2026 (reproducible benchmark)

This is the section to cite. It uses only documents committed in this
repository and one script that reruns everything.

**Result, in short.** TeXres is faster than TeX Live on a cold build of five
of the eight documents (10-70%) and, apart from LuaLaTeX, answers a no-change
rebuild in about 10 ms against about 75 ms. It is slower on Beamer (1.8x),
TikZ/pgfplots (1.7x) and LuaLaTeX, where a cold start spends about 45 seconds
building the font database (details below). A single pdfLaTeX pass on the
three 12-15 page text documents is within 6% of `pdflatex` in either
direction, and 27% slower on the 113-page document.

### Table

Median wall time in seconds over 7 runs, TeXres / TeX Live. The ratio in
parentheses is how many times faster or slower TeXres was. Minimum and maximum
of every cell, and CPU time, are in
[`scripts/bench/results/bench_tl.md`](scripts/bench/results/bench_tl.md); the
raw per-run times are in
[`bench_tl.json`](scripts/bench/results/bench_tl.json) next to it.

| Document | Pages | (a) cold build | (b) no-change rebuild | (c) one-line-edit rebuild | (d) single engine pass |
| --- | ---: | ---: | ---: | ---: | ---: |
| article_math | 15 | 0.994 / 1.10 (1.11× faster) | 0.008 / 0.076 (9.97× faster) | 0.339 / 0.407 (1.20× faster) | 0.320 / 0.302 (1.06× slower) |
| article_biblatex | 12 | 4.79 / 5.77 (1.20× faster) | 0.012 / 0.075 (6.18× faster) | 1.34 / 1.38 (1.04× faster) | 1.32 / 1.27 (1.04× slower) |
| article_natbib | 12 | 1.04 / 1.76 (1.70× faster) | 0.008 / 0.075 (9.60× faster) | 0.267 / 0.447 (1.67× faster) | 0.260 / 0.267 (1.03× faster) |
| beamer_deck | 82 | 7.88 / 4.45 (1.77× slower) | 0.010 / 0.080 (7.87× faster) | 4.07 / 2.30 (1.77× slower) | 4.05 / 2.15 (1.89× slower) |
| tikz_pgfplots | 9 | 24.74 / 14.88 (1.66× slower) | 0.009 / 0.079 (8.85× faster) | 8.25 / 5.01 (1.64× slower) | 8.26 / 4.93 (1.68× slower) |
| xelatex_fontspec | 8 | 1.96 / 2.15 (1.10× faster) | 0.007 / 0.073 (9.79× faster) | 0.665 / 0.858 (1.29× faster) | 0.645 / 0.701 (1.09× faster) |
| lualatex_fontspec | 7 | 52.50 / 4.23 (12.40× slower) | 1.71 / 0.083 (20.53× slower) | 1.72 / 1.47 (1.17× slower) | 48.96 / 1.28 (38.10× slower) |
| long_thesis | 113 | 3.04 / 3.57 (1.17× faster) | 0.011 / 0.076 (6.93× faster) | 0.773 / 0.792 (1.03× faster) | 0.760 / 0.596 (1.27× slower) |

Scenarios:

- **(a) cold build**: `texres -pdf|-xelatex|-lualatex main.tex` with an empty
  cache root, against `latexmk -pdf|-xelatex|-lualatex -interaction=nonstopmode
  main.tex` in a freshly copied directory. Both run every pass and the
  bibliography tool (TeXres: built-in BibTeX and Biber; TeX Live: `bibtex`,
  `biber 2.22`).
- **(b) no-change rebuild**: the same command again, after a converged build.
- **(c) one-line edit**: one word in the middle of `main.tex` changed
  (`BENCH-A` to `BENCH-B`), then the same build command.
- **(d) single engine pass**: one `pdflatex`/`xelatex`/`lualatex` run with
  `-interaction=nonstopmode -halt-on-error`, over the auxiliary files
  (`.aux`, `.toc`, `.bbl`, ...) of a converged TeX Live build, restored before
  every run. For TeXres this is the executable run through a link of that name
  with an empty cache root each time.

### What the numbers show, and where TeXres is slower

- **Cold builds** of the pdfLaTeX text documents (math, biblatex, natbib,
  the 113-page thesis) and the XeLaTeX document are 10-70% faster. In the
  natbib and thesis documents `latexmk` runs `pdflatex` four times and `bibtex`
  three times. The single-pass times for those two documents are equal or
  slower for TeXres, so the cold-build gain does not come from faster passes;
  it appears to be build-driver overhead (an inference: the runs were counted,
  the overhead was not profiled).
- **Beamer and TikZ/pgfplots are slower**: 1.6-1.9x on every scenario that
  runs the engine, including the single pass. The cause has not been
  investigated here.
- **LuaLaTeX is much slower to start.** The first LuaLaTeX build of a
  document takes 48-53 s, against 4 s for TeX Live, whose luaotfload font
  database is already built in `$HOME`. A one-line LuaLaTeX document costs the
  same 47 s (checked separately), so this is start-up cost rather than
  typesetting; [ARTIFACTS.md](ARTIFACTS.md) and the CI notes call it cold Lua
  font-database startup. The cost is paid per document, not per machine: in a
  separate check a second, different document built in the same cache root
  paid 47 s again. Once a document's cache exists, a one-line edit costs
  1.72 s against 1.47 s (1.17x slower), but the no-change rebuild still takes
  1.7 s where every other document takes about 10 ms. Scenario (d) uses an
  empty cache root each time, so it pays the 49 s every run.
- **No-change rebuilds** compare a TeXres cache validation with `latexmk`
  noticing that nothing changed. Both are fast, and 10 ms against 75 ms says
  little about typesetting speed; most of the 75 ms is probably `latexmk`'s own
  start-up (an inference, not profiled).
- **Spread is small.** For every cell with a median over 1 second, the
  slowest of the 7 runs is at most 6% above the fastest. The 1-minute load
  average was 0.7-1.7 during the run, including the benchmark itself.

### Equivalence check, exclusions and corpus caveats

Before timing, each document is built once with both tools. The PDFs must have
the same page count and the same `pdftotext -layout` text after removing all
whitespace; otherwise the document is reported and excluded. All eight timed
documents passed. One additional document, `toc_wrap_canary`, is **excluded on
purpose and is part of the run**: a long, wrapping list-of-figures entry is
hyphenated by TeXres (`re-lates`) where `pdflatex` does not break the
word. Page count is the same; only that line break differs. While building the
corpus the same difference appeared on the long-caption list-of-figures and
list-of-tables pages of two other documents, so those documents use short
captions. That is a correctness difference found while benchmarking, not
something the table hides.

The LuaLaTeX document loads its helper functions with `dofile` instead of
the `luacode` environment, because TeXres 0.7.2 fails on `\begin{luacode}`
("File ended while scanning use of \luacode@grab@lines") where TeX Live does
not. The XeLaTeX and LuaLaTeX documents load the TeX Gyre fonts by file name
(`texgyretermes.otf`, ...) because TeXres uses only its bundled fonts, and
TeX Live's XeTeX on this machine does not find them by family name.

### Corpus

Committed in [`scripts/bench/corpus/`](scripts/bench/corpus/); regenerate
byte-for-byte with `python3 scripts/bench/gen_corpus.py`. All text is synthetic.

| Document | Engine | Pages | Contents |
| --- | --- | ---: | --- |
| `article_math` | pdfLaTeX | 15 | `amsmath`/`amsthm`/`hyperref`, theorems, 40+ labelled equations, cross-references, table of contents |
| `article_biblatex` | pdfLaTeX | 12 | `biblatex` with `backend=biber`, 80 entries |
| `article_natbib` | pdfLaTeX | 12 | `natbib`, `plainnat`, BibTeX, 80 entries |
| `beamer_deck` | pdfLaTeX | 82 | Beamer (Madrid), overlays, columns, tables, 40+ frames |
| `tikz_pgfplots` | pdfLaTeX | 9 | 8 pgfplots 2D plots, 3 surface plots, 6 TikZ diagrams |
| `xelatex_fontspec` | XeLaTeX | 8 | `fontspec`, `unicode-math`, TeX Gyre fonts, accented, Greek and Cyrillic text |
| `lualatex_fontspec` | LuaLaTeX | 7 | as above, plus Lua computed by `\directlua` |
| `long_thesis` | pdfLaTeX | 113 | `report`, 12 chapters, 84 sections, 84 floats, `natbib` + BibTeX with 120 entries, lists of figures and tables |
| `toc_wrap_canary` | pdfLaTeX | 1 | excluded on purpose (see above) |

### Method

- Every timed run starts from an identical restored state (a copy of the
  source directory, or of the converged directory, with modification times
  preserved) and, for TeXres, a restored or empty cache root. Restoring is not
  timed. Each cell is 7 runs; within a cell the two tools alternate and the
  order flips every run.
- Wall time is measured around the whole command. CPU time is user plus system
  time of the process tree. `SOURCE_DATE_EPOCH` and `FORCE_SOURCE_DATE` are set.
  `PATH` is `/usr/bin:/usr/bin/vendor_perl:/bin`, so only the system TeX Live
  is used; TeXres is the explicit path given to the script.
- One verification build of each document with each tool comes before the timed
  runs. It also warms the file-system cache and anything TeX Live keeps in
  `$HOME` (for example the luaotfload database). **That warm state favours
  TeX Live in scenario (a); TeXres always starts with an empty cache root.**
- The CPU governor was `performance`; runs were not pinned to cores.
  TeXres uses extra threads for PDF compression and font subsetting; they are
  included in wall and CPU time.

### Hardware and software

| | |
| --- | --- |
| Date | 2026-10-07 |
| CPU | AMD Ryzen Threadripper PRO 5995WX 64-Cores, 64 cores / 128 threads, 1 socket |
| RAM | 504 GiB |
| OS | Arch Linux, Linux 7.2.8-arch1-2 |
| TeXres | texres 0.7.2 (Rust TeX engine), release build of commit `4bae77a` |
| TeX Live | pdfTeX 3.141592653-2.6-1.40.29 / XeTeX 3.141592653-2.6-0.999998 / LuaHBTeX 1.24.0 (TeX Live 2026, Arch Linux packages); Latexmk 4.87; biber 2.22 (Arch `biber` package, `/usr/bin/vendor_perl/biber`) |

These are numbers from one many-core workstation on one operating system;
treat the absolute times as specific to it.

### Reproduce

```sh
cargo build --release --locked --bin texres
python3 scripts/bench_tl.py --texres target/release/texres
# fewer runs / a subset (at least 5 runs per cell):
python3 scripts/bench_tl.py --texres target/release/texres --runs 5 --docs article_math,beamer_deck
```

Needs `/usr/bin/{pdflatex,xelatex,lualatex,latexmk,bibtex}` from TeX Live,
`biber`, and `pdfinfo`/`pdftotext` (poppler). The script writes
`scripts/bench/results/bench_tl.json` and `bench_tl.md`; use `--out-dir` to
write elsewhere. A full run took about 31 minutes here, of which about 12
minutes is TeXres building the LuaLaTeX font database.

## 100-document benchmark (TeXres vs TeX Live 2026)

`scripts/bench100.py` extends the benchmark above to 100 documents: 70
generated ones (`scripts/bench/corpus100/`, committed, regenerate with
`scripts/bench/gen_corpus100.py`) and 30 real public ones, fetched by pinned
arXiv version or GitHub commit and verified by a sha256 over the unpacked tree
(`scripts/bench/public_manifest.json`, `scripts/bench/fetch_public.py`). Third-
party sources are not committed.

- Engines: 60 pdfLaTeX, 25 XeLaTeX, 15 LuaLaTeX.
- Classes: article, report, book, memoir, scrartcl/scrbook, amsart, revtex4-1/-2,
  elsarticle, IEEEtran, llncs, svjour3, lipics, lmcs, ecta/ectj, jcap/jhep, beamer
  (Madrid, Boadilla, metropolis), standalone, letter, ctexart/ctexbook, tufte.
- Packages: amsmath/amsthm, hyperref, cleveref, biblatex+biber (numeric,
  alphabetic, authortitle, ieee, chicago), natbib/BibTeX, tikz, pgfplots, siunitx,
  booktabs/longtable/tabularx, listings, algorithm2e, xcolor, tcolorbox, fontspec
  (TeX Gyre, Latin Modern), unicode-math, polyglossia (de, fr, es, pl, ru, el),
  xeCJK/ctex, microtype, glossaries, makeidx/imakeidx, subcaption, footmisc.
- Sizes: 1 to about 300 pages.

Each document is built once with both tools; a document that TeX Live cannot
build, that TeXres cannot build, or whose `pdftotext -layout` text or page count
differs is not timed but stays in the results with its status. Scenarios: (a)
cold full build (fresh TeXres cache vs clean latexmk directory), (b) no-change
rebuild, (c) one-line edit rebuild (generated documents: `BENCH-A` becomes
`BENCH-B`; public documents: ` BENCH-B` is appended to one plain-text line of a
body file, chosen automatically and recorded). `--scenarios abcd` adds (d), one
engine pass. At least 5 runs per cell, medians; tools alternate and flip order
every run.

The ratio is TeXres wall time divided by TeX Live wall time (below 1: TeXres is
faster). The results (`scripts/bench/results/bench100.{json,md}`) list every
document, slowest first, with the count of documents faster and slower per
scenario.

```sh
python3 scripts/bench100.py --texres BIN                      # everything, one document at a time
python3 scripts/bench100.py --texres BIN --jobs 16 --cpus-per-job 4   # parallel, each job pinned to its own cores
python3 scripts/bench100.py --texres BIN --docs pdf_letter,arxiv_bert,engine=lua,source=github
python3 scripts/bench100.py --texres BIN --resume             # skip documents already measured with this binary
```

Parallel jobs are pinned with `taskset` to disjoint physical cores (both tools
of one document share a CPU set). On a 128-thread machine, 16 jobs of 4 CPUs
gave medians within about 3% of one-at-a-time runs on four test documents
(biblatex, tikz, XeLaTeX, letter); check this on your machine with `--jobs 1`
before trusting parallel numbers. The whole corpus then takes about 10 minutes.
Copy the binary under test to a stable path first, and pass `--texres-note` to
record its commit in the results.

## Build profile

`[profile.release]` in the workspace `Cargo.toml`:

| Setting | Value |
| --- | --- |
| `lto` | `"thin"` |
| `codegen-units` | `16` |
| `panic` | `"abort"` |
| `debug` | `0` |
| `strip` | `"none"` (symbols stay, so profilers can name functions) |

The `ffi-release` profile used for `libtex` inherits `release` but sets
`panic = "unwind"` (see [docs/libraries.md](docs/libraries.md)).

## Runtime design relevant to speed

- The LaTeX formats are embedded zstd-compressed and loaded at startup.
- TeX support files come from an embedded zstd-compressed package archive
  (`crates/tex-kpse`); no TeX installation is scanned.
- The default font map (`pdftex.map`) is parsed only on first use.
- From the second shipped page on, page content streams are compressed on a
  worker thread while later pages are typeset.
- When at least four Type 1 font subsets are needed, they are prepared on up
  to four threads (not on `wasm32`).
- `TEXDEBUG=TEX_PDF_SERIAL` disables both workers for controlled comparisons.
- PNG conversion uses the normal speed setting; `--optimize-pdf-size` spends
  more CPU searching for smaller lossless streams.

## Caches: always start cold when timing

Two caches can make a run skip typesetting entirely:

- The **engine result cache**: a direct engine pass records its inputs (source,
  every loaded file, auxiliary state, relevant environment, executable
  identity) and, when nothing changed, republishes the previous PDF, SyncTeX
  file, and transcript without typesetting.
- The **driver cache**: `texres` keeps per-job auxiliary state and a manifest
  and can declare an unchanged build converged after one cache hit.

Both live under the cache root: `TEX_RS_CACHE_DIR`, or `--cache-directory DIR`,
or the platform default listed in [ARTIFACTS.md](ARTIFACTS.md). For timing,
give every run a fresh, empty cache root, and restore the same auxiliary files
(`.aux`, `.bbl`, `.toc`, ...) before each run so that every run does the same
work. Fix `SOURCE_DATE_EPOCH` so date-dependent output does not vary.

## Measuring

Prefer instruction counts to wall time; they are far less sensitive to machine
load:

```sh
TEX_RS_CACHE_DIR=$(mktemp -d) SOURCE_DATE_EPOCH=1700000000 \
  perf stat -e instructions:u,task-clock -- \
  /tmp/texres-bench/pdflatex -interaction=batchmode main.tex
```

Compare against TeX Live's `pdflatex` on the same prepared inputs, run
alternately, with the same auxiliary files restored before each run.

`PHASE_TIMING=1` prints per-phase wall times of a direct engine pass to
standard error: `startup`, `format`, `typeset`, `pdf_serialize`, `pdf_write`,
and `finish`. It also reports `ls-R` loading whenever a TeX tree with an
`ls-R` database is searched.

`scripts/bench_cold.py` is a fresh-process harness for a prepared set of
documents (`prepare`, `run`, `profile`, and `pgo` subcommands); see its
module docstring. It was written for private manuscripts that are not
published, so it cannot be rerun from this repository; the harness for the
public corpus is `scripts/bench_tl.py` (first section above).

## Profiling

Build a frame-pointer binary in its own target directory and record call
graphs with `perf`:

```sh
RUSTFLAGS="-C force-frame-pointers=yes" CARGO_TARGET_DIR=target/fp \
  cargo build --release --locked --bin texres
ln -s "$PWD/target/fp/release/texres" /tmp/texres-prof/pdflatex
TEX_RS_CACHE_DIR=$(mktemp -d) perf record --call-graph fp -- \
  /tmp/texres-prof/pdflatex -interaction=batchmode main.tex
perf report --no-children
```

## Historical measurements

The three sections below are earlier instruction-count and CPU-time
measurements on private manuscripts (70, 89, 81, 29 and 49 pages) that are not
published. Nobody else can reproduce them, and they predate the benchmark
above. They are kept as a record of how the engine changed between those
commits; do not compare them with the public table.

## Baseline measurements (commit `a7b7a95`, historical)

One prepared single engine pass (fresh cache, auxiliary files restored) of the
70-page `trust_own` manuscript on a Linux x86_64 machine:

| Measurement | TeXres | TeX Live 2026 `pdflatex` |
| --- | ---: | ---: |
| User-space instructions | 25.87 G | 6.42 G |
| CPU time | ~2.05 s | ~0.66 s |

Within that TeXres pass, loading the format took about 74 ms and PDF
serialization plus writing about 41 ms; the remainder was TeX execution,
including decompression of embedded package and font files. The TeX Live
column was re-measured with the same counter (median of seven passes); the
4.11 G and 0.41 s first recorded here could not be reproduced.

## Post-review and XeTeX-cutover measurements (commit `f979b24`, historical)

Same prepared-pass method and machine; median of three instruction-counter
passes and seven timing passes:

| Measurement | TeXres (post-review) | TeX Live 2026 `pdflatex` |
| --- | ---: | ---: |
| User-space instructions, `trust_own` single pass | 7.29 G | 6.41 G |
| CPU time, `trust_own` single pass | ~0.88 s | ~0.64 s |
| Format load | ~32 ms | — |
| PDF serialize + write | ~37 ms | — |

Across the current five benchmark documents (70, 89, 81, 29 and 49 pages),
a prepared pass executes 48.8 G instructions in total, versus 48.9 G in the
pre-XeTeX binary. All five PDFs remain byte-identical to that binary and match
TeX Live in extracted text (`pdftotext -layout`) and 50 dpi page renders.
The original pre-review source snapshots totaled 135.2 G instructions.

## Engine-coverage verification (historical)

Paired prepared-pass counters, three samples per document, comparing the
XeTeX-cutover baseline binary with the verified engine-coverage binary
(`5db6ea7872b5`):

| Measurement | Baseline | Engine-coverage source |
| --- | ---: | ---: |
| `trust_own` instructions | 7.335 G | 7.342 G |
| Five-document instruction total | 49.015 G | 49.058 G |

The measured total increased by 0.088%; all five PDFs are byte-identical.
The expanded isolated TeX Live 2026 gate passes all 127 fixtures: 23 pdfTeX,
45 LuaLaTeX and 59 XeLaTeX. Cold Unicode math completes under the unchanged
512 MiB resident-memory limit. Immutable font metadata uses exact precomputed
values and byte windows instead of inflating full font programs during scans.

