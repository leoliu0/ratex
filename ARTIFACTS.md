# Build artifacts and caches

`texmk document.tex` keeps the project directory clean by default. The final
PDF is written beside the source, while auxiliary files and the complete TeX
transcript live in a persistent per-job cache. The cache makes later builds
fast and still preserves the log needed to diagnose a failed or successful
build.

The default cache root follows the platform convention:

- Linux: `$XDG_CACHE_HOME/tex-rs`, or `~/.cache/tex-rs`
- macOS: `~/Library/Caches/tex-rs`
- Windows: `%LOCALAPPDATA%\tex-rs\cache`

Set `TEX_RS_CACHE_DIR` or pass `texmk --cache-directory DIR` to choose another
root. At most once per hour, `texmk` removes inactive entries older than 30
days and evicts the oldest inactive jobs until managed caches target 512 MiB.
The active job is never collected, so a single unusually large build may
temporarily exceed that target. Foreign directory names are ignored; corrupt
directories with a managed job name are discarded. Collection never removes
project sources or project output.

Cache hits are content-validated. The record is tied to the complete engine
invocation and checks the source, format override, relevant search environment,
all loaded files, auxiliary state, and staged PDF, and requires the retained
transcript to exist. Publication of
the final PDF and requested retained files uses a same-directory temporary file
and atomic replacement, so an interrupted or failed rebuild cannot expose a
partially written result.

A newly created or changed auxiliary file always triggers a real convergence
pass before caching. Even apparently empty LaTeX boilerplate can change a later
pass through file-existence checks, redefined input hooks, or page-count state;
the cache is written only after the complete auxiliary snapshot is unchanged.
Texmk rejects symlinks and special files inside an auxiliary-state tree, and
aborts if that tree cannot be read or changes while it is being hashed. This
keeps convergence checks complete and prevents preexisting state links from
redirecting a managed build outside the selected auxiliary directory.

Use `texmk --keep-logs document.tex` to copy the transcript beside the PDF,
or `texmk --keep-intermediates document.tex` (short form `-k`) to copy all
auxiliary files. `texmk -c document.tex` removes the matching private cache
and exported files that have not been modified. `texmk -C document.tex` also
removes an unchanged PDF that `texmk` originally created. It preserves a PDF
that predated the build or was changed afterwards.

`pdflatex` keeps the traditional direct-engine behavior: without directory
options it writes the PDF, transcript, and auxiliary files beside the source.
Use `-output-directory DIR` for the PDF and `-aux-directory DIR` for the
transcript and auxiliary files. Its dependency cache is private; override its
location with `--cache-directory DIR`.

PNG conversion uses the normal speed setting by default. Pass
`--optimize-pdf-size` to `pdflatex` or `texmk` to spend more CPU selecting
smaller lossless image streams. JPEG data, compatible PNG streams, and
imported PDF pages remain pass-through data.

## Distribution footprint

Release archives contain one full executable, `texmk`. It embeds the TeX
engine, BibTeX, the production LaTeX format, packages, fonts, and maps. On
Linux and macOS every public command is a relative symlink to `texmk`; Windows
uses small launchers because zip archives do not preserve symlinks portably.
The archive does not carry a second raw `pdflatex.fmt` unless a distributor
explicitly supplies `scripts/package_dist.py --fmt FILE`.

Resolution is self-contained by default: project inputs remain ordinary
files, while TeX support files come from the executable. Use
`texmk --allow-system-texmf` or set `TEX_RS_ALLOW_SYSTEM_TEXMF=1` for a direct
engine alias to search an installed TeX tree as well.

Here, self-contained refers to the TeX toolchain and its runtime data. A
document's own `.tex`, image, bibliography, and local style files remain its
inputs. Platform executables also use the operating system ABI; for example,
the Linux build dynamically links glibc and libgcc while requiring no TeX Live
installation or companion data files.

`manifest.json` records regular-file hashes separately from symlink targets,
and packaging verifies the completed archive before returning success. The
installers keep an ownership manifest and remove or replace only paths created
by an earlier tex-suite install.

### Testing the self-contained contract

The regression suite verifies self-containment through observable behavior:

- `one_copied_texmk_builds_with_embedded_latex_and_bibtex_resources` copies
  only `texmk` into a fresh directory, clears its environment, poisons the
  standard TeX tree variables, and builds a document that needs LaTeX,
  extensionless generic inputs, T1 and TS1 fonts, NewTX, and BibTeX's
  `plain.bst`. It verifies the PDF and the embedded-resource paths recorded in
  the TeX and BibTeX logs.
- `copied_texmk_ignores_external_tex_trees_until_explicitly_enabled` creates
  packages available only through `TEXINPUTS`, `TEXMFHOME`, and an
  executable-adjacent TeX tree. Every default build must fail with a useful
  missing-file diagnostic. The same inputs must succeed with
  `--allow-system-texmf`, and a later default build must not reuse cache state
  produced by that opt-in build.
- `copied_texmk_symlink_personalities_need_no_sibling_executables` creates one
  physical executable and the shipped relative aliases. It checks dispatch by
  version banner, compiles through the `pdflatex` alias, and runs the `bibtex`
  alias with the embedded style database.
- `scripts/test_package_dist.py` verifies the archive representation: Unix
  aliases are relative symlinks, Windows aliases contain only the small
  launcher, regular files and links are disjoint in the manifest, and stale
  full-engine Windows aliases are rejected.
- The `tex-kpse` unit tests compare every indexed embedded file byte-for-byte
  with the source archive and exercise default-extension lookup. This catches
  a complete or well-formed index that points at the wrong payload.

Run the focused checks with:

```sh
cargo test -p tex-cli --test driver copied_texmk_
python3 scripts/test_package_dist.py
```

The full workspace suite includes the archive-index checks. Native CI should
also execute an extracted archive on each supported operating system. A
Windows runner is required to exercise process forwarding by the small `.exe`
launcher.

## Corpus and benchmark retention

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
