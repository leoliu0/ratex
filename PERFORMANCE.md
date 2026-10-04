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
module docstring.

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

## Baseline measurements (commit `a7b7a95`)

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

## Post-review and XeTeX-cutover measurements (commit `f979b24`)

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

## Engine-coverage verification

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

