# Build artifacts and caches

`texres document.tex` keeps the project directory clean by default. The final
PDF and its `document.synctex.gz` editor-navigation sidecar are written beside
the source (or together in the selected output directory). Auxiliary files and
the complete TeX transcript live in a persistent per-job cache
(`<cache root>/texmk/jobs/<16 hex digits>/`). The cache makes later builds fast
and preserves the log needed for diagnostics; on failure `texres` prints the
transcript's path.

The default cache root follows the platform convention:

- Linux: `$XDG_CACHE_HOME/tex-rs`, or `~/.cache/tex-rs`
- macOS: `~/Library/Caches/tex-rs`
- Windows: `%LOCALAPPDATA%\tex-rs\cache`

Set `TEX_RS_CACHE_DIR` or pass `texres --cache-directory DIR` to choose another
root. At most once per hour, `texres` removes inactive entries older than 30
days and evicts the oldest inactive jobs until managed caches target 512 MiB.
The active job and jobs locked by a running build are never collected, so a
single unusually large build may temporarily exceed that target. Foreign
directory names are ignored; directories with a managed job name but no valid
manifest are discarded. Collection never removes project sources or project
output.

Cache hits are content-validated. The record is tied to the complete engine
invocation and checks the source, format override, relevant search environment,
all loaded files, auxiliary state, staged PDF and SyncTeX sidecar, and requires
the retained transcript to exist. Each published output uses a same-directory
temporary file and atomic replacement, so no individual file is exposed while
partially written.

A newly created or changed auxiliary file always triggers a real convergence
pass before caching. Even apparently empty LaTeX boilerplate can change a later
pass through file-existence checks, redefined input hooks, or page-count state;
the cache is written only after the complete auxiliary snapshot is unchanged.
`texres` rejects symlinks and special files inside an auxiliary-state tree, and
aborts if that tree cannot be read or changes while it is being hashed. This
keeps convergence checks complete and prevents preexisting state links from
redirecting a managed build outside the selected auxiliary directory.

Use `texres --keep-logs document.tex` to copy the transcript beside the PDF,
or `texres --keep-intermediates document.tex` (short form `-k`) to copy all
auxiliary files. `texres -c document.tex` removes the matching private cache
and exported files that have not been modified. `texres -C document.tex` also
removes unchanged PDF and SyncTeX outputs that `texres` originally created.
It preserves preexisting or subsequently modified outputs.

A direct engine pass (the executable invoked through a link named `pdflatex`,
`xelatex`, or `lualatex`) keeps the traditional behavior: without directory
options it writes the PDF, SyncTeX sidecar, transcript, and auxiliary files
beside the source. Use `-output-directory DIR` for the PDF and SyncTeX sidecar,
and `-aux-directory DIR` for the transcript and auxiliary files. Its result
cache is private; override its location with `--cache-directory DIR` or
`TEX_RS_CACHE_DIR`.

PNG conversion uses the normal speed setting by default. Pass
`--optimize-pdf-size` to a direct engine pass or to `texres` to spend more CPU
selecting smaller lossless image streams.

## Distribution footprint

Release archives (`tex-suite-v<version>-<platform>-<arch>.tar.gz`, or `.zip`
on Windows) contain one executable, `bin/texres`, plus installer scripts, `README.txt`,
license files, `manifest.json`, and `share/tex-suite/texmf/doc/fonts/` with
font licenses, notices, and corresponding sources. The
executable embeds the TeX engine, BibTeX, Biber, the LaTeX formats, packages, fonts,
and maps. No command aliases are shipped; the executable dispatches on the name
it is invoked as (`pdflatex`, `xelatex`, `lualatex`, `bibtex`, `biber`, `latexmk`,
`latexdiff`), so users may create such links themselves. The archive does not
carry a raw `pdflatex.fmt` unless a distributor explicitly supplies
`scripts/package_dist.py --fmt FILE`.

Resolution is self-contained: project inputs remain ordinary files, while TeX
support files come from the embedded archive (and from
`$TEX_SUITE_DATA/texmf` or `$TEXRES_DATA_DIR/texmf` when either variable is
set). `TEXINPUTS`, `TEXMFHOME`, and system TeX trees are ignored. Here,
self-contained refers to the TeX toolchain and its runtime data. A document's
own `.tex`, image, bibliography, and local style files remain its inputs.
Platform executables also use the operating system ABI; for example, the Linux
build dynamically links glibc (`libc`, `libm`) and `libgcc_s` while requiring
no TeX Live installation or companion data files. Linux archives are published
for `x86_64` and `aarch64`, with matching native packages: Debian `amd64`/`arm64`
`.deb` files and `x86_64`/`aarch64` RPM and Arch packages. The Linux ARM64 release
executable is built natively in the official `rust:1-bookworm` Docker image on
an ARM64 runner, giving it a Debian 12 / glibc 2.36 runtime floor. It is suitable
for ARM64 Linux containers on Apple Silicon; neither the build nor execution
requires QEMU or cross compilation.

`manifest.json` in each archive records regular-file SHA-256 hashes separately
from symlink targets, and packaging verifies the completed archive before
returning success. The installers keep an ownership manifest and remove or
replace only paths created by an earlier tex-suite install.

### Release publication

On version tags, publication is all-or-nothing: every Linux (`x86_64` and
`aarch64`), macOS, Windows, and library build must succeed before publication.
The workflow requires every archive, native installer/package, and per-platform
checksum file, then verifies all recorded SHA-256 hashes before uploading
release assets. A missing ARM archive or native package blocks publication.
Windows packaging requires working Poppler tools before running PDF verification.

### Testing the self-contained contract

The regression suite verifies self-containment through observable behavior:

- `one_copied_texmk_builds_with_embedded_latex_and_bibtex_resources` copies
  only the driver executable into a fresh directory, clears its environment,
  poisons the standard TeX tree variables, and builds a document that needs
  LaTeX, extensionless generic inputs, T1 and TS1 fonts, NewTX, and BibTeX's
  `plain.bst`. It verifies the PDF and the embedded-resource paths recorded in
  the TeX and BibTeX logs.
- `copied_texmk_ignores_external_tex_trees_until_explicitly_enabled` creates
  packages available only through `TEXINPUTS`, `TEXMFHOME`, and an adjacent
  tree. Every build fails with a useful missing-file diagnostic, the
  `--allow-system-texmf` option is rejected, and a later build does not reuse
  state, proving external TeX installations cannot contaminate builds.
- `copied_texmk_symlink_personalities_need_no_sibling_executables` creates one
  physical executable plus relative links to it. It checks dispatch by
  version banner, compiles through the `pdflatex` link, and runs the `bibtex`
  link with the embedded style database.
- `scripts/test_package_dist.py` checks the archive and installer contract:
  manifest categories, Windows staging, zip compression, format validation,
  and installer ownership/upgrade/uninstall behavior.
- The `tex-kpse` unit test `indexed_packages_match_archive_bytes` compares
  indexed embedded files byte-for-byte with the source archive. This catches
  a complete or well-formed index that points at the wrong payload.
- `bundled_font_scan_preserves_name_and_style_metadata` canonicalizes the
  bundled font directories with luaotfload's real path resolver before
  reading the faces. On Windows, the drive-qualified paths still address
  the same read-only embedded tree.

Run the focused checks with:

```sh
cargo test -p tex-cli --test driver copied_texmk_
python3 scripts/test_package_dist.py
```

The full workspace suite includes the archive-index checks. The release
workflow also executes an extracted archive and the native installers on each
supported operating system.
Native Linux ARM64 CI and release jobs run the same workspace and packaging
tests, full 127-case TeX Live 2026 reference/font/text/render gate, extracted
archive smoke, and shell-installer smoke as x86_64 Linux; the release additionally
installs and exercises the native Debian package. The independent ARM64 2 GiB
memory-bounded build gate remains required.
Linux CI removes completed dev-profile test builds before the release build
to reclaim disk for the reference TeX Live installation and archive staging.
XeTeX fixture paths use forward slashes when inserted into TeX input,
including on Windows.
The font suite also runs the exact minimum documents from issues #17 and #18
with LuaLaTeX and XeLaTeX. Both must preserve Spanish text and embed their
fonts; Linux compares their rendering with the matching TeX Live 2026 engine.
The LuaLaTeX case uses the suite's general geometry tolerances; the XeLaTeX
accent case requires at least 0.98 ink IoU in both renderers. Fixture clocks
share a fixed epoch and UTC timezone without changing the reported sources.
Unix release archive and installer font smokes allow 180 seconds per document
for cold Lua font-database startup. The macOS 15 arm64 release builder uses
Rust 1.98.1 after a Rust 1.99 dependency-archive failure; regular macOS CI
continues to exercise the latest stable compiler.

Linux CI and release builds run the engine subsystem probes and paired
LuaLaTeX/XeLaTeX package-interaction documents from the same manifest.
`compare_text` fixtures compare the complete whitespace/NFC-normalized text
from both Poppler and pdf.js with the matching TeX Live 2026 reference, preserving
character order and multiplicity; expected excerpts alone are not a parity check.
`reference_passes` and `reference_bibtex` cover multipass references and
bibliographies. Reference compilation, page-count and viewer failures fail the
gate rather than being treated as unavailable comparison data.

Paired runs pin physical fonts, font-selection profiles, amsmath, unicode-math
and CJK support to the locked runtime payload, while retaining the genuine
host TeX Live engines, kernel and other general packages. Both Lua paths
positively load the same generic/CMEX Unicode mappings and enable Type 1
ToUnicode generation; variant selectors remain part of the comparison.
Generated wrappers preserve the original document, jobname and bibliography
workflow. Reference-free archive/installer smokes resolve those mappings from
the binary's embedded tree and need no host TeX or archive reconstruction.

The workflows upload `target/font-evidence-full/report.json` and failed-case
artifacts even when the gate fails. The report includes engine/family counts,
failure reasons, executable identity, reference versions and bounded subprocess
output; retained case directories contain the PDFs, renders and logs.
Text mismatch reasons identify the first differing normalized character and
include bounded surrounding excerpts, so a shared prefix does not hide a later
ordering, duplication or missing-character failure.
The image-scan fixture includes its three-page PDF input in source control.
Diagnostic output and retained transcripts use UTF-8 even under legacy
Windows console and locale encodings.
Lua system-library oracles retain platform-specific gzip header bytes and
native Windows path separators instead of assuming POSIX output.

The font suite's deliberate missing-glyph fixture sets `expect_missing_glyphs`:
TeX Live itself emits `.notdef` for these characters. This exempts only that
resolution check; the expected warning, font-program bounds, text and render
checks remain required. Other fixtures still reject unexpected `.notdef`.

## Corpus and benchmark artifacts

### Source acquisition

Download a separate source sample without replacing the existing corpus:

```sh
python3 scripts/download_corpus.py \
  --target 1000 --out-dir corpus/additional --workers 3
```

Use `--candidates-file PATH` with a JSON list of `[arxiv_id, archive]` pairs
to reuse a selected candidate pool or exclude IDs from an earlier sample.
The downloader uses arXiv's dedicated export host, spaces source requests
across all workers, and harvests OAI metadata serially with a three-second
delay. PDF-only submissions and downloads without an extracted TeX entry
point do not count toward the requested project total.

Preserve the source manifest and acquisition settings. Downloaded sources
are test inputs, not evidence of successful compilation or embedded-font
coverage. Standalone checks must exercise the packaged binary without
external TEXMF resources; a reference TeX Live environment stays separate.

Corpus qualification checks clean, converged builds, PDF/font validity,
page counts and geometry, raster warnings, and measured pixel parity.
Producer metadata is recorded, not used to identify the engine or gate
parity: documents can override it, and engine personalities use TeX-compatible
values.

### Generated evidence retention

`scripts/test_corpus.py` and `scripts/bench_cold.py` retain compact failure
evidence by default. Child output is consumed as it is produced, hashed in
full, scanned for errors, and stored as at most 1 MiB: a 128 KiB head plus a
tail. Successful logs, PDFs, renders, and copied workspaces are deleted after
their metrics and hashes are recorded. Reports and checkpoints remain.

Use `--retain all` for a diagnostic run that needs every artifact,
`--retain none` for metrics only, or `--keep-work` to preserve copied source
trees. `--max-capture-bytes N` changes the stream limit; zero explicitly
selects an unlimited capture.

Historical harness trees are never removed automatically. Create a reviewable
cleanup plan first:

```sh
python3 scripts/prune_corpus_artifacts.py \
  --root output/campaign-old \
  --plan /tmp/campaign-old-prune.json
```

After inspecting that JSON file, apply exactly that plan:

```sh
python3 scripts/prune_corpus_artifacts.py \
  --apply /tmp/campaign-old-prune.json
```

Application revalidates the report and every planned file's path, type, size,
timestamp, and (for compacted logs) SHA-256 before changing anything. The
pruner only acts inside harness-managed `work`, `pdf`, `results`, and `worst`
directories. It refuses `trust_own` trees and preserves reports, gates,
qualification data, checkpoints, and bounded final failure evidence.
