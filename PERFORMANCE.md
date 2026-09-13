Use `target/release/pdflatex` as the single canonical release executable. Future release builds update this path. Keep benchmark and diagnostic variants under a temporary directory or `tmp/benchmarks`, using a separate Cargo target directory; do not add suffixed executables to `target/release`.

The engine avoids parsing unused font-map options, compresses completed pages during typesetting, prepares independent font subsets concurrently and writes compact PDFs directly. Raw PDF extensions retain the existing normalization path in memory. These changes preserve output geometry and use the existing compression settings.

The optional filename index removes repeated parsing of a TeX installation's `ls-R` database. Build and prepare it with the output directory beside the engine:

```sh
cargo build --release --workspace
target/release/tex-index target/release/tex-index-data /usr/share/texmf-dist /var/lib/texmf
```

Supply the roots belonging to your installation. Index preparation writes only to the specified output directory. The engine looks for `tex-index-data` beside its executable; set `TEX_INDEX_DIR` to use another directory, or set it to an empty value to disable this feature. Missing, stale or invalid indexes use the ordinary resolver. Regenerate the indexes after updating the installation. A first compilation without prepared indexes still includes the ordinary database parsing cost.

For profiling, `PHASE_TIMING=1` prints compilation phases. `TEXDEBUG=TEX_PDF_SERIAL` disables the new PDF workers for comparison. Measure fresh processes with consistent auxiliary inputs and removed PDF/dependency/page caches; report OS cache state and prepared installation metadata separately. Avoid mixing timings from concurrent builds or different CPU-affinity configurations.

The 2026-09-13 paired benchmark measured a 351.178 ms baseline and a 323.400 ms optimized median across eleven alternating warm-filesystem trials, a 27.778 ms (7.91%) reduction. The optimized range was 316.607--325.328 ms; the 300 ms full-process target was not reached. All 78 pages in the three-document benchmark set had identical 150-DPI raster output and identical `pdftotext -layout` output, and all PDFs passed `qpdf --check`.

Benchmark binaries, profiles, copied source trees, and rendered outputs are transient and are not versioned. Recreate measurements with `scripts/bench_cold.py`; keep experimental binaries outside `target/release`.
