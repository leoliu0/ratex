# Performance

## What is measured

The distributed executable is `ratex` (`target/release/ratex`). Invoked as
`ratex`, it is the build driver: it re-executes itself for every TeX and BibTeX
pass. To time or profile one engine pass, run the same executable through a
link named `pdflatex` (or `xelatex`/`lualatex`):

```sh
ln -s "$PWD/target/release/ratex" /tmp/ratex-bench/pdflatex
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
- The **driver cache**: `ratex` keeps per-job auxiliary state and a manifest
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
  /tmp/ratex-bench/pdflatex -interaction=batchmode main.tex
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
  cargo build --release --locked --bin ratex
ln -s "$PWD/target/fp/release/ratex" /tmp/ratex-prof/pdflatex
TEX_RS_CACHE_DIR=$(mktemp -d) perf record --call-graph fp -- \
  /tmp/ratex-prof/pdflatex -interaction=batchmode main.tex
perf report --no-children
```

## Baseline measurements (commit `a7b7a95`)

One prepared single engine pass (fresh cache, auxiliary files restored) of the
70-page `trust_own` manuscript on a Linux x86_64 machine:

| Measurement | Ratex | TeX Live 2026 `pdflatex` |
| --- | ---: | ---: |
| User-space instructions | 25.87 G | 6.42 G |
| CPU time | ~2.05 s | ~0.66 s |

Within that Ratex pass, loading the format took about 74 ms and PDF
serialization plus writing about 41 ms; the remainder was TeX execution,
including decompression of embedded package and font files. The TeX Live
column was re-measured with the same counter (median of seven passes); the
4.11 G and 0.41 s first recorded here could not be reproduced.

## Post-review measurements (commit `87e5563`)

Same method and machine, median of seven passes:

| Measurement | Ratex (post-review) | TeX Live 2026 `pdflatex` |
| --- | ---: | ---: |
| User-space instructions, `trust_own` single pass | 7.34 G | 6.42 G |
| CPU time, `trust_own` single pass | ~0.88 s | ~0.66 s |
| Format load | ~35 ms | — |
| PDF serialize + write | ~36 ms | — |

Across the five benchmark documents that build (70, 90, 81, 29 and 49
pages), a prepared pass now executes 49.3 G instructions in total, down from
135.2 G at the baseline; their PDFs match TeX Live's in extracted text
(`pdftotext -layout`) and in 50 dpi page renders.
