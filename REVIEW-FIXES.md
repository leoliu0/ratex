# Review fixes and performance results

Reviewed and repaired the current working tree without reverting existing work. No commits were created.

## Correctness fixes

- Fixed the undefined math-style helper that prevented compilation and preserved explicit `\nolimits` when attaching both scripts.
- Fixed `texmk` citation tracking after preliminary BibTeX, and required stable auxiliary files before declaring convergence.
- Preserved form XObject font mappings, emitted their font resource dictionaries, and embedded fonts used only inside forms.
- Refreshed cached input files after disk changes and invalidated disk entries on TeX output-file creation. Existing active input buffers remain valid.
- Removed persistent negative file-lookup caching and allowed newly created local files to override earlier distribution matches.
- Regenerated the bundled format for the current format and semantics versions. Packaging validates format headers and rejects explicitly supplied stale formats.
- Fixed the PDF parity test's missing per-page score calculation and aggregate score.
- Guarded Unix-specific memory and time APIs, supplied Windows time conversions, recognized executable suffixes, and made modification-time access portable.
- Repaired additional defects exposed by the full suite: paragraph-token assignment, split-mark bookkeeping, discarded `\message` output, error-message accounting, and discretionary material at paragraph start.
- Removed an unconditional display-debug print. Corrected INITEX test fixtures that lacked required category codes and a page-output fixture that incorrectly enabled initialization mode.

## Performance and cache changes

- Replaced runtime decompression and scanning of the complete embedded package archive with a build-time sorted index and individually compressed members. Case-insensitive fallback is indexed too. A test compares every indexed member byte-for-byte with the source archive.
- Removed duplicate page-cache handling and checked the dependency cache before constructing an engine. Cache identity includes the executable, invocation, and relevant environment. Only successful jobs write cache entries; images, external formats, and additional auxiliary-file types participate in dependency tracking.
- Reused initialized primitive mappings while loading formats, avoiding construction of a second complete engine.

Local measurements use five fresh-process samples per timing case. They characterize these workloads, not a universal document speedup.

| Measurement | Before | After |
| --- | ---: | ---: |
| Median access time for seven embedded package/font files (390,959 decoded bytes) | 1,525.704 ms | 1.257 ms |
| Median format-load time (ten loads per sample) | 87.818 ms/load | 86.192 ms/load |
| Measured format-loading peak resident memory | 36,696 KiB | 32,104 KiB |

The PDF writer tracks painted character codes, subsets Type 1 CharStrings
and reachable Subrs, trims width and Unicode tables, assigns deterministic
subset names, and packs finished files into PDF 1.5 object streams.

The first size optimization stopped at 614,349 bytes. A byte breakdown showed
that embedded fonts accounted for 191,782 of the remaining 238,828-byte gap
against latexmk. The dependency scanner cleared its operand stack on calls,
so ordinary hint replacement made it retain all font subroutines. The new
dependency interpreter shares the caller's stack, follows nested calls and
`seac` components, and handles hint replacement and flex using the
[Adobe Type 1 specification](https://www.adobe.com/content/dam/acom/en/devnet/font/pdfs/T1_SPEC.pdf).
Unsupported instructions, missing dependencies and excessive recursion/work
fall back to the original font. Drawing and hint programs remain byte-for-byte
unchanged. Unencrypted charstrings (`lenIV = -1`) are handled correctly too.

Font pruning now copies retained ranges once instead of repeatedly shifting
the remaining buffer. Font hashes are computed once per instance, and a font
that cannot be subset is attempted only once. Text runs use escaped literal
strings, splitting only at nonzero kerning adjustments; coordinates and
advances are unchanged.

| `trust_own`, 70 pages | Previous Rust output | Optimized Rust output | System latexmk |
| --- | ---: | ---: | ---: |
| PDF bytes | 614,349 | **339,346** | 375,521 |
| Compressed font bytes | 376,385 | 124,056 | 184,603 |
| Other compressed stream bytes | 229,494 | 206,825 | 181,262 |
| Remaining PDF structure bytes | 8,470 | 8,465 | 9,656 |
| Median fresh-process wall time | 461.6 ms (earlier run) | **422.0 ms** | 1,117.8 ms |

The optimized PDF is **44.8% smaller than the previous Rust output**, **9.6%
smaller than latexmk's output**, and 79.4% smaller than the original
1,643,506-byte Rust PDF. The new timing measurements each use seven runs with
the same saved auxiliary inputs restored and generated PDF/dependency/page
caches removed before every run. OS filesystem caches were not flushed.
Rust timing measures one engine pass; latexmk timing measures its workflow,
which invoked pdfLaTeX twice and BibTeX twice in the retained logs.
Their timing ratio is not a comparison
between the two underlying engines alone. Observed ranges were
415.3–432.8 ms for Rust and 1,019.7–1,132.0 ms for latexmk.

The compiled [PDF](tmp/trust-own-optimized/main.pdf),
[timing samples](tmp/trust-own-optimized/benchmark.json), and
[validation details](tmp/trust-own-optimized/validation.json) are retained.
All 70 pages had zero differing raster samples at 150 DPI against the previous
Rust output. `pdftotext -layout` output was byte-identical; its SHA-256 is
`125501428ebaf363267f5145cf408bda7ff9ae05ccf96cb659414b70309fd941`.
`qpdf --check` found no syntax or stream encoding errors. These checks establish
preservation of the existing Rust output, not full rendering parity with pdfTeX.

The installed release compiled the smoke document in a median 138.589 ms without a dependency-cache hit and 1.079 ms with a hit. These are final-build measurements, not a before/after comparison. Per-member compression increases embedded package data from approximately 20 MiB to 34 MiB.

## Cold compilation follow-up

The latest changes reduce `trust_own` compilation from **419.9 ms to 358.1 ms**
with an ordinary release build, or **335.9 ms with profile-guided optimization**.
The latter uses 20.0% less time than the baseline. It is **3.33× faster than
the measured latexmk workflow**, and **1.16× faster than a single system
pdfLaTeX pass**. This does not establish the requested 10× speedup.

| Build / command | Median of seven fresh processes | PDF bytes |
| --- | ---: | ---: |
| Archived Rust baseline | 419.877 ms | 339,346 |
| Updated ordinary Rust release | 358.139 ms | 339,346 |
| Profile-guided Rust release | **335.925 ms** | **339,346** |
| `/usr/bin/pdflatex` (one pass) | 389.272 ms | 375,521 |
| `/usr/bin/latexmk -pdf` (workflow) | 1,118.993 ms | 375,521 |

The benchmark alternates execution order, restores the same saved `.aux`,
`.out` and `.bbl` inputs before every trial, and removes the generated PDF,
dependency/page caches, `.fdb_latexmk` and `.fls`. OS file caches are not flushed.
Every command exits successfully. The Rust executables run from the same
directory so executable-relative TeX search roots remain identical. This is
a fresh-process benchmark with prepared auxiliary inputs, not a build with
all bibliography and reference state absent.

Measured costs led to these changes:

- Balanced argument scanning now checks only the argument being consumed.
  Previously, each group first scanned the entire remaining replacement list
  for frozen tokens, causing repeated work proportional to the remaining list.
  Unrelated later frozen tokens also disabled the fast path. The argument copy
  now uses a bulk slice copy when no guard needs conversion.
- Selector macros transfer large argument buffers directly into the input
  stack. Parameterless macros bypass argument-buffer construction. Raw token
  fetching avoids calling alignment interception when it is disabled.
- The `ls-R` resolver stores offsets into its original text and a single index
  of case-folded filenames. It materializes paths only for requested entries,
  preserving exact-case precedence, Unicode case folding and duplicate-file
  order. Loading the system database fell from **43.6 ms to 16.9 ms** in the
  instrumented ordinary builds.
- `PHASE_TIMING=1` now reports startup, format load, TeX execution, font/image
  preparation, PDF serialization and PDF compaction. The benchmark harness
  captures these diagnostics and excludes them from timed trials.
- A separate [profile-guided build](https://doc.rust-lang.org/rustc/profile-guided-optimization.html)
  was trained on this manuscript, a five-page article and a three-slide deck.
  Both compiler and profile merger use LLVM 22.1.8; no `target-cpu=native`
  setting was used. Training profiles contain execution counts, not reusable
  document output.

Use the canonical executable `target/release/pdflatex`. Historical benchmark
variants are archived under `tmp/benchmarks/2026-09-13/bin`. For example, from a document directory:

```sh
/home/leo/dd/tex/target/release/pdflatex -interaction=nonstopmode -halt-on-error main.tex
```

The target for 10× this latexmk measurement is **111.9 ms**. Instrumented
profile-guided runs still spend approximately **309 ms in TeX execution**,
including package/font lookup, versus about 7 ms in startup/format loading and
24 ms in font preparation/PDF output. These diagnostic phase samples were
collected separately from the benchmark. Eliminating startup or PDF output
alone cannot bridge the gap; roughly two-thirds of the remaining total time
must be removed. Further large gains need changes to macro-token execution
and typesetting, and have not been demonstrated by this work.

Both updated builds retain **339,346-byte, 70-page PDFs**, with byte-identical
extracted text and zero differing raster samples at 150 DPI against the
archived Rust baseline. Ordinary and profile-guided builds also agree on the
five-page article and three-slide deck. `qpdf --check` passes.

Retained artifacts: [timings](tmp/trust-speed/timings-final.json),
[phase measurements](tmp/trust-speed/phases.json),
[validation](tmp/trust-speed/validation-final.json),
[compiler/source/binary provenance](tmp/trust-speed/provenance.json),
[profile-guided PDF](tmp/trust-speed/pgo.pdf), and
[merged LLVM profile](tmp/trust-speed/merged.profdata).
The retained profile can rebuild a separate executable from this source state:

```sh
CARGO_TARGET_DIR=target/pgo \
RUSTFLAGS="-Cprofile-use=/home/leo/dd/tex/tmp/trust-speed/merged.profdata" \
cargo build --release -p tex-cli --bin pdflatex --locked --offline
cp target/release/pdflatex.fmt target/pgo/release/pdflatex.fmt
```

Regenerate training profiles when changing the interpreter or compiler;
the retained timings describe these workloads and this machine.

## Validation

- `cargo test --workspace --locked --offline`: **299 passed, 0 failed, 6 ignored**. Coverage includes driver/cache/form-font/input-cache regressions, embedded-member equivalence, bundled-format loading, 20 differential tests against the installed reference pdfLaTeX, Type 1 dependency-stack/flex/composite/error cases, round-trip decoding of all 256 font bytes in literal text strings, guarded/cross-source argument scanning, large selector arguments, and compact filename-index precedence/order.
- `cargo check --workspace --all-targets --locked --offline`: passed.
- `cargo build --release --workspace --locked --offline`: passed.
- `git diff --check`: passed.
- Built the Linux x86_64 distribution, installed it into an isolated `/tmp` prefix, and successfully compiled a document exercising common packages, references, mathematics, and a font used only in a form.
- That installed-build PDF had zero differing raster pixels at 150 DPI compared with the earlier corrected build. Extracted text included the form text, with no MuPDF warnings.
- Exercised the parity test's Python comparison on synthetic PDFs: identical pages passed and a deliberately changed page failed.
- Verified that packaging accepts the regenerated format and rejects the archived stale format.

## Remaining limits

Native Windows and macOS execution was not available locally. Release CI now includes build checks and focused regression tests, but those runners must establish native-platform results. Unix shell-based driver tests run only on Unix.

The six existing ignored tests were not enabled as part of the full-suite run. The private-document parity corpus was not rerun; the synthetic comparison check and smoke-document raster comparison do not establish full corpus parity. Existing compiler warnings remain.

## TeX execution follow-up (300 ms target)

The updated TeX execution phase measured **292.325 ms median** in five diagnostic runs. The complete fresh-process manuscript compile measured **324.819 ms median**, versus **337.885 ms** for the previous PGO executable in nine alternating trials. The 300 ms end-to-end target remains unmet. Timings use identical auxiliary inputs, removed output/dependency/page caches and warm OS filesystem caches.

The retained changes reduce argument retention and replacement copying, add guarded SIMD scanning, remove unnecessary node copies, reduce filename/font-map allocations and bypass general aligned-allocation machinery for ordinary layouts. The final executable uses native CPU code generation and PGO trained on three documents. Compiler-tuning and input-stack experiments without useful overall gains were discarded.

The manuscript PDF is **339,308 bytes and 70 pages**. Across the manuscript, article and slide fixtures, all 78 pages retain identical 150-DPI raster output and extracted text, and all PDFs pass `qpdf --check`. The fixed source snapshot passes **307 tests (0 failed, 6 ignored)** and all-target workspace checks.

Concurrent paragraph and math edits in the shared workspace required an isolated source snapshot for reliable validation. The benchmark executable was built from that retained snapshot. Details, individual timings, phase measurements, validation records, source archive and build provenance are in [the performance report](tmp/trust-speed-next/README.md). Historical benchmark executables are now in [the benchmark archive](tmp/benchmarks/2026-09-13/README.md); use `target/release/pdflatex` for current builds. The generated manuscript is [main.pdf](tmp/trust-speed-next/main.pdf).

Implemented the next performance batch: lazy font-map parsing, validated installation filename indexes, bounded page compression, parallel font subsetting, shared-program fingerprint reuse, and direct compact PDF output with in-memory normalization for extension objects. The live workspace passes all-target checks and 316 tests (six ignored). All 78 validation pages retain exact raster and text output. Paired fresh-process medians improved from 351.178 ms to 323.400 ms; trust_own shrank from 339,308 to 337,489 bytes. The full-process 300 ms target remains unmet. Macro argument/replacement sharing experiments were removed after failing to improve speed. See [the implementation report](tmp/trust-speed-implemented/README.md) and [index setup instructions](PERFORMANCE.md).
