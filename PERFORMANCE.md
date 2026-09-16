# Performance

Use `target/release/texmk` as the canonical distribution executable. It embeds
the TeX engine and BibTeX, and a distribution exposes `pdflatex`, `xelatex`,
`lualatex`, `tex-bibtex`, `bibtex`, and `latexmk` as aliases. The production
LaTeX format and support archive are compressed and embedded. Keep profiling
variants in `/tmp` or a separate Cargo target directory; do not add suffixed
executables to `target/release`.

The release profile uses fat LTO, one code-generation unit, aborting panics,
and stripped debug data. At runtime the engine avoids unused font-map parsing,
compresses completed pages while later pages are typeset, prepares independent
font subsets concurrently, and writes compact PDFs directly. PNG passthrough
is validated without decoding the complete image; the normal mode caps PNG
compression at the measured speed optimum, while `--optimize-pdf-size` enables
the slower size search.

## Current measurements

The 2026-09-14 comparison used fresh project and artifact-cache directories on
a warm Linux filesystem, fixed `SOURCE_DATE_EPOCH`, release binaries, and
alternating tool order. Times cover process start through exit. The small input
is a one-page article. The realistic input is the 70-page `trust_own`
manuscript copied to `/tmp`; the source tree itself was never modified.

| Scenario | tex-rs | TeX Live | Relative result |
| --- | ---: | ---: | ---: |
| One-page `texmk`, clean artifacts (9 trials) | 79.2 ms | `latexmk` 133.7 ms | 1.69x faster |
| 70-page `texmk`, clean artifacts (5 trials) | 1,634.8 ms | `latexmk` 1,985.6 ms | 1.21x faster |
| One-page unchanged `texmk` (21 trials) | 1.37 ms | `latexmk` 36.24 ms | 26.5x faster |
| 70-page unchanged `texmk` (15 trials) | 3.00 ms | `latexmk` 40.90 ms | 13.6x faster |
| 70-page prepared direct pass (7 trials) | 402.8 ms | `pdflatex` 383.9 ms | 4.9% slower |

The clean `trust_own` build requires four dependent TeX executions in both
drivers. The first discovers citations and labels; BibTeX then creates the
bibliography. The second incorporates it and changes pagination, the third
resolves references against that pagination, and the fourth verifies that the
new auxiliary state is a fixed point. An `.aux` file is executable TeX input,
so a driver cannot soundly infer the fourth result from matching PDF sizes or
from a selected set of familiar auxiliary commands. In this measurement
`texmk` ran BibTeX once, while `latexmk` ran it three times. `texmk --verbose`
reports which auxiliary files changed after each pass.

The direct-pass result is the honest boundary for the requested 300 ms target:
this manuscript has not reached it. A phase-timed sample attributed about 94%
of the instrumented process to the TeX typesetting loop. Format loading,
filename lookup, font preparation, image work, PDF serialization, and the
final write together took about 28 ms. Reaching 300 ms on this input therefore
requires a substantial typesetting-loop improvement. Unchanged builds avoid
that work through a fully validated dependency cache.

The optional prepared filename index was also measured in 15 alternating
direct-pass trials. Its median was 401.6 ms versus 410.5 ms for parsing the
ordinary `ls-R` data, an 8.9 ms improvement. The generated index occupied
7.8 MiB, remains outside release archives, and is useful only for the local TeX
installation that created it.

The optimized PDF was 307,420 bytes versus TeX Live's 375,510 bytes, 18.1%
smaller, and both files passed `qpdf --check`. With the document date fixed,
66 of 70 pages had byte-identical 150-DPI renders; the full document had
99.989% exact pixel agreement. `pdftotext -layout` still differs in four
localized math/layout regions, so these measurements do not claim exact text
extraction parity.

The clean one-page run leaves only its 28,655-byte PDF in the source directory;
`latexmk` leaves five files totaling about 34,807 bytes. The realistic run
leaves one 307,420-byte PDF; `latexmk` leaves eight files totaling about
518,430 bytes. The corresponding private tex-rs state measured 43,936 bytes
and 533,257 bytes. It is governed by the bounded cache policy described in
[ARTIFACTS.md](ARTIFACTS.md).

## Reproducing and profiling

To measure the optional system-tree resolver, explicitly enable it and prepare
an installation index with roots that belong to that TeX installation:

```sh
cargo build --release --workspace
target/release/tex-index target/release/tex-index-data \
  /usr/share/texmf-dist /var/lib/texmf
```

Run texmk with `--allow-system-texmf`; a direct engine invocation can set
`TEX_RS_ALLOW_SYSTEM_TEXMF=1`. The engine looks for `tex-index-data` beside its
executable. Set `TEX_INDEX_DIR` to another directory, or to an empty value to
disable the index. A missing, stale, or invalid index safely falls back to the
normal resolver and should be regenerated after the TeX installation changes.

`PHASE_TIMING=1` prints compilation phases. `TEXDEBUG=TEX_PDF_SERIAL` disables
the PDF workers for controlled comparisons. Use `scripts/bench_cold.py` for
fresh-process single-pass experiments. Keep auxiliary inputs, OS cache state,
CPU placement, and prepared installation metadata consistent, and never mix
measurements with concurrent builds.

An earlier 2026-09-13 change-isolation benchmark measured a 351.178 ms baseline
and a 323.400 ms optimized median across eleven alternating trials over a
different three-document, 78-page set. Its optimized range was
316.607--325.328 ms, with identical 150-DPI renders and extracted layout text
for that set. Those numbers describe the isolated PDF/font optimizations and
are not directly comparable to the current 70-page manuscript measurement.

Benchmark binaries, profiles, copied sources, and rendered pages are transient
and are not versioned.
