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

## TeXres vs TeX Live 2026 (100-document benchmark)

This is the section to cite. The per-document results are in
[`scripts/bench/results/bench100.md`](scripts/bench/results/bench100.md),
slowest first, and every run is in
[`bench100.json`](scripts/bench/results/bench100.json).

**Result, in short.** TeXres was faster than TeX Live on all 100 documents in
all three scenarios. The ratio is TeXres wall time divided by TeX Live wall
time, as the median of 5 runs per cell:

| Scenario | TeXres faster | Median ratio | Worst ratio |
| --- | ---: | ---: | ---: |
| (a) cold build | 100 / 100 | 0.60 | 0.98 (`pdf_siunitx_tables`, 3.54 / 3.63 s) |
| (b) no-change rebuild | 100 / 100 | 0.13 | 0.26 (`gh_lkmpg_book`, 0.030 / 0.114 s) |
| (c) one-line edit rebuild | 100 / 100 | 0.57 | 0.98 (`gh_lkmpg_book`, 6.62 / 6.77 s) |

| Group | Documents | (a) median / worst | (b) median / worst | (c) median / worst |
| --- | ---: | ---: | ---: | ---: |
| generated (`corpus100`) | 70 | 0.60 / 0.98 | 0.11 / 0.19 | 0.56 / 0.94 |
| public (arXiv, GitHub; never trained on) | 30 | 0.57 / 0.96 | 0.16 / 0.26 | 0.57 / 0.98 |
| pdfLaTeX | 60 | 0.56 / 0.98 | 0.14 / 0.26 | 0.56 / 0.98 |
| XeLaTeX | 25 | 0.59 / 0.81 | 0.11 / 0.16 | 0.51 / 0.74 |
| LuaLaTeX | 15 | 0.74 / 0.88 | 0.13 / 0.17 | 0.72 / 0.87 |

The margin is small on two documents. `pdf_siunitx_tables` (siunitx inside
tables) is 0.98 cold and 0.94 after an edit; `gh_lkmpg_book` (193 pages) is
0.96 cold and 0.98 after an edit. The next closest are `lua_beamer` (0.88)
and `pdf_beamer_metropolis` (0.87). The four closest were measured again one
at a time, unpinned, 9 runs per cell, load average 1.4-1.8
([`bench100_recheck9.md`](scripts/bench/results/bench100_recheck9.md)):
`pdf_siunitx_tables` 0.99 / 0.10 / 0.95, `gh_lkmpg_book` 0.96 / 0.24 / 0.96,
`lua_beamer` 0.91 / 0.13 / 0.89 and `pdf_beamer_metropolis` 0.89 / 0.12 /
0.88 for (a) / (b) / (c).

On those two documents a single engine pass gains little or nothing; the
builds win on fixed cost. Measured with `perf stat` over TeX Live's converged
aux files: on `gh_lkmpg_book` TeXres takes 26.1G cycles against 28.0G for
`pdflatex`; on `pdf_siunitx_tables` it takes 4.52G against 4.28G. A one-line
document costs TeXres about 65 ms per pass and `pdflatex` about 155 ms. The
gap on siunitx tables is branch mispredictions (48M against 28M). Inside an
alignment entry TeXres looks up the meaning of every control sequence it
fetches to test for a row delimiter; TeX gets that from `cur_cmd`.

### Scenarios

- **(a) cold build**: `texres -pdf|-xelatex|-lualatex main.tex` with an empty
  cache root, against `latexmk -pdf|-xelatex|-lualatex -interaction=nonstopmode
  main.tex` in a freshly copied directory. Both run every pass and the
  bibliography tool (TeXres: built-in BibTeX and Biber; TeX Live: `bibtex`,
  `biber 2.22`). The cache root keeps one directory, `texmf-var`, between cold
  runs: the machine-wide luaotfload font database and font caches. TeX Live
  keeps the same data in `$HOME/.texlive2026/texmf-var`, which `latexmk`
  never clears.
- **(b) no-change rebuild**: the same command again, after a converged build.
- **(c) one-line edit**: generated documents change `BENCH-A` to `BENCH-B`.
  Public documents get ` BENCH-B` appended to one plain-text line of a body
  file; the line is chosen automatically and recorded in the results. Then
  the same build command runs.

### Method

- Binary: `scripts/build_pgo.sh` at commit `69c9ca4`, a profile-guided build
  with fat LTO in one codegen unit. The release workflow ships this build for
  Linux x86_64 and macOS arm64. Windows, linux-aarch64, macOS x86_64 and
  `cargo install` get the plain release build, which is slower and is not
  measured here (see "Profile-guided build").
- Each document is built once with both tools first. The two PDFs must have
  the same page count and the same `pdftotext -layout` text after whitespace
  is removed; otherwise the document is not timed. All 100 passed. The PDFs
  of this binary are also byte-identical to those of `da0e76f` on all 100
  documents.
- Every timed run starts from an identical restored state. The tools
  alternate, and the order flips every run. Wall time covers the whole
  command. `SOURCE_DATE_EPOCH` and `FORCE_SOURCE_DATE` are set. `PATH` is
  `/usr/bin:/usr/bin/vendor_perl:/bin`, so only the system TeX Live is used.
- 16 documents ran at once, each pinned with `taskset` to its own 4 logical
  CPUs (2 physical cores), which both tools of that document share. Load
  average was 5.9 at the start and 5.6 at the end. `xe_natbib_bibtex` was
  measured again on its own with the same binary and settings, because TeX
  Live's `xelatex` exited with code 1 once during the parallel sweep.

### Caveats

- **The first LuaLaTeX build on a new machine** builds the font database
  (a scan of the system fonts) for both tools. The cold builds above start
  with that database already built on both sides. It was measured once at an
  earlier commit: TeXres 65 s, TeX Live 61 s. It has not been re-measured
  since the `getinfo` memo (c18e106), which should make it shorter.
- **SyncTeX**: TeXres writes it by default, and `latexmk` does not unless
  asked. That costs TeXres about 1% of a pass; it is included in all times
  above.
- **One machine, one operating system.** Treat the absolute times as
  specific to this machine.

### Hardware and software

| | |
| --- | --- |
| Date | 2026-10-08 |
| CPU | AMD Ryzen Threadripper PRO 5995WX 64-Cores, 64 cores / 128 threads; governor `performance` |
| RAM / OS | 504 GiB; Arch Linux, Linux 7.2.8-arch1-2 |
| TeXres | texres 0.7.2, `scripts/build_pgo.sh` at `69c9ca4` |
| TeX Live | pdfTeX 3.141592653-2.6-1.40.29, XeTeX 3.141592653-2.6-0.999998, LuaHBTeX 1.24.0 (TeX Live 2026, Arch Linux packages); Latexmk 4.87; biber 2.22 |

### Corpus

There are 70 generated documents (`scripts/bench/corpus100/`, committed;
regenerate with `scripts/bench/gen_corpus100.py`) and 30 real public ones.
The public documents are fetched by pinned arXiv version or GitHub commit and
checked by a sha256 over the unpacked tree (`scripts/bench/public_manifest.json`,
`scripts/bench/fetch_public.py`). Third-party sources are not committed. PGO
trains on a variant of the generated corpus with other text and data, and
never on the measured documents or the public ones.

- Engines: 60 pdfLaTeX, 25 XeLaTeX, 15 LuaLaTeX.
- Classes: article, report, book, memoir, scrartcl/scrbook, amsart,
  revtex4-1/-2, elsarticle, IEEEtran, llncs, svjour3, lipics, lmcs,
  ecta/ectj, jcap/jhep, beamer (Madrid, Boadilla, metropolis), standalone,
  letter, ctexart/ctexbook, tufte.
- Packages: amsmath/amsthm, hyperref, cleveref, biblatex+biber (numeric,
  alphabetic, authortitle, ieee, chicago), natbib/BibTeX, tikz, pgfplots,
  siunitx, booktabs/longtable/tabularx, listings, algorithm2e, xcolor,
  tcolorbox, fontspec (TeX Gyre, Latin Modern), unicode-math, polyglossia
  (de, fr, es, pl, ru, el), xeCJK/ctex, microtype, glossaries,
  makeidx/imakeidx, subcaption, footmisc.
- Sizes: 1 to about 300 pages.

### Reproduce

```sh
scripts/build_pgo.sh /tmp/texres-pgo/texres          # about 15 minutes
python3 scripts/bench100.py --texres /tmp/texres-pgo/texres --jobs 16 --cpus-per-job 4 --texres-note "$(git rev-parse --short HEAD) PGO"
python3 scripts/bench100.py --texres BIN --docs pdf_siunitx_tables,gh_lkmpg_book --runs 9   # one at a time
python3 scripts/bench100.py --texres BIN --resume     # skip documents already measured with this binary
```

It needs `/usr/bin/{pdflatex,xelatex,lualatex,latexmk,bibtex}` from TeX Live,
`biber`, and poppler's `pdfinfo`/`pdftotext`. Before you trust parallel
numbers on another machine, compare a few documents with `--jobs 1`. Here,
16 jobs of 4 CPUs agreed with one-at-a-time runs to within about 3%. The
full sweep takes about 6 minutes.

### Eight-document benchmark (historical)

`scripts/bench_tl.py` times the eight documents of `scripts/bench/corpus/`
with the same scenarios plus (d), one engine pass. Its committed results
([`bench_tl.md`](scripts/bench/results/bench_tl.md)) come from the plain
release build of `4bae77a`, before the interpreter work. At that commit
TeXres was slower on Beamer, TikZ/pgfplots and LuaLaTeX. Rerun it with
`python3 scripts/bench_tl.py --texres BIN`.

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

## Profile-guided build

`scripts/build_pgo.sh [OUTPUT]` builds an instrumented `texres`, runs a cold
build of every document of the training corpora (`scripts/bench/corpus`
without its LuaLaTeX document, and a variant of `scripts/bench/corpus100` with
other text and data, eight at a time) and rebuilds with the recorded profile
(`-Cprofile-use`). The 100-document corpus brings the packages the
eight-document one leaves out (beamer themes, siunitx tables, CJK,
KOMA-Script, memoir, indexes, fontspec/luaotfload). LuaLaTeX training builds
see only the TeX Live fonts (an empty `OSFONTDIR` and a `luaotfload.conf`
with `location-precedence = texmf`), so the profile does not depend on the
machine's fonts. It needs `llvm-profdata` of the LLVM that `rustc`
uses (`rustup component add llvm-tools` or the distribution's `llvm`
package). Nothing it generates is committed: the profile belongs to the exact
sources it was recorded from.

Both builds of the script link with fat LTO in a single codegen unit
(`--config` overrides; the plain release profile keeps thin LTO, which needs
far less memory). The settings enter cargo's symbol hashes, so the
instrumented and the optimized build must use the same ones, or the profile
silently goes unused. One pdfLaTeX pass over the converged TeX Live aux files,
same profile data, thin vs fat: `gh_lkmpg_book` 32.9G vs 31.6G cycles,
`pdf_siunitx_tables` 5.13G vs 4.86G, `pdf_beamer_metropolis` 6.08G vs 5.80G
(branch misses and instruction-cache misses drop by 10 to 45%). Slide decks
of the training variant differ from the measured decks in their text
(`gen_corpus100.py` shifts every text generator by `--seed-offset`), so they
are trained on too.

Why it pays: the engine is a token interpreter whose hot paths (token fetch,
macro expansion, `\def`, conditionals) run through a dozen large functions,
about 100 KiB of machine code interleaved with cold paths. Without a profile
the compiler cannot tell hot from cold blocks, and the processor spends its
time fetching instructions. One pdfLaTeX pass over the `tikz_pgfplots` aux
files of a converged TeX Live build (`perf stat`, user space):

| | instructions | cycles (lowest of 3) | L1 instruction-cache misses | wall (7 runs, median) |
| --- | ---: | ---: | ---: | ---: |
| `/usr/bin/pdflatex` | 58.6 G | 21.9 G | 0.05 G | 8.31 s |
| TeXres 0.7.2 | 72.2 G | 39.7 G | 3.8 G | 14.81 s |
| TeXres, source changes only | 66.6 G | 28.7 G | 1.8 G | 11.74 s |
| TeXres, `build_pgo.sh` | 57.1 G | 21.0 G | 1.0 G | 8.46 s |

The machine was loaded by other jobs during these runs (load average above
80, hence the slow absolute times); the tools alternated, so the ratios hold.
A profile taken without `tikz_pgfplots` gave the same cycles, so the gain does
not come from training on the measured document. On the Beamer deck a plain
release build retires 4.1G taken branches and 67M instruction-cache misses per
pass, against 2.5G and 1.5M for `pdflatex`; the profile-laid-out build takes
them to 2.1G and 25M. Output is byte-identical.

## Runtime design relevant to speed

- The LaTeX formats are embedded zstd-compressed and loaded at startup. A
  format is stored as independent 1 MiB zstd frames behind a skippable frame
  that indexes them (`scripts/build_formats.py`, and `-ini` dumps alike), so
  that up to four threads decode it at once: with a single frame, decoding
  took about half of the format load (roughly 40 of 80 ms for XeLaTeX).
- Inside an alignment entry, tokens that cannot end the entry (anything but
  `&`, a control sequence \let to it, `\cr` or `\crcr` at brace depth zero)
  take the same fast token fetch and bulk argument scans as outside one.
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
  identity, the dates `\pdffilemoddate` reported) and, when nothing changed,
  republishes the previous PDF, SyncTeX file, and transcript without
  typesetting. What the run produced itself is not an input: a file read back
  after the run wrote it through `\openout` (beamer's `.vrb`), and a file first
  read after a `\write18` command ran and gone at the end (minted's
  `latexminted` config file), which is recorded as absent.
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

