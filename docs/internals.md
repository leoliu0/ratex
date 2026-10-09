# TeXres internals

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
| `eqtb.rs`, `trace.rs` | eqtb and save stack; e-TeX `\tracingassigns`/`\tracingrestores`/`\tracinggroups`/`\tracingifs` events (queued, then written in order with the escape character of the moment) |
| `format.rs` | `.fmt` dump/load wire format |
| `node_arena.rs` | generation-checked node storage |
| `pdffile.rs`, `pdf_fonts.rs` | PDF serialization, Type 1 parsing and embedding |
| `pdftex.rs`, `pdfrender.rs` | pdfTeX backend state: resource names (`/F`, `/Fm`, `/Im` and the `\pdfuniqueresname` tag), form shipping on first paint, query primitives, Info/trailer inputs |
| `writet1.rs` | writet1.c port: Type 1 FontFile cleartext/`/Encoding`/eexec/Subrs rewrite, `/Length1-3` |
| `writet3.rs` | pkin.c/writet3.c port: a TFM without a pdftex.map entry is a PK font (pdfTeX only) — the bundled `fonts/pk/ljfour` files mktexpk would make at `\pdfpkresolution`, read as bitmaps and written as a Type 3 font (one per size, `d1` inline-image glyphs, `/a<code>` names); text advances by `getpkcharwidth` |
| `pdf_images.rs`, `pdf_encodings.rs` | pdftoepdf port: one shared `PdfSource` per included file; with `\pdfinclusioncopyfonts=0` Type 1/Type1C fonts the font map knows are replaced by the map's program (xpdf base-encoding tables) and their descriptor is never preset from a TFM; every other object is copied as it is, unembedded standard fonts included — only the EPS converter's PDFs (`tex_ps::EPS_PDF_PRODUCER`) get the standard-font programs Ghostscript would embed |
| `pdfrender/dpx.rs`, `pdfrender/dpx_text.rs` | XeTeX's xdvipdfmx-compatible PDF driver: separate cached DVI and reader positions, text matrices and glyph runs; raw TFM metric words use the driver's `sqxfw` rounding for annotation bounds |
| `dpx_font.rs`, `dpx_cff.rs`, `dpx_tt.rs`, `dpx_t1.rs` | xdvipdfmx native font objects, Unicode CMaps, TrueType/CFF subsetting and TFM Type 1 → Type1C conversion |
| `diagnostics.rs` | structured diagnostics and their output bounds |

The XeTeX driver serializes PDF object numbers with eight decimal places of
precision. Restoring graphics state invalidates the device font and synthetic
text matrix, matching xdvipdfmx's reset before the next text run.

TFM Type 1 text uses the same xdvipdfmx text-state machine as native glyphs,
with literal PDF strings and truncating TJ adjustments. XeTeX embeds PFB
programs as Type1C, shares map encodings and their Unicode CMaps, and rejects
PFA containers without guessing from encrypted bytes. pdfTeX and LuaTeX keep
their existing font writers.
XeTeX resolves mapped TFM programs and encodings at first PDF font use,
after preceding map specials. Unprefixed `pdf:mapline`/`mapfile` and
`x:fontmapline`/`fontmapfile` replace entries, as in xdvipdfmx.

XeTeX copies included PDF fonts unchanged instead of applying pdfTeX's
font-map replacement. Native-word widths contribute to `\predisplaysize`;
vertical-top math nuclei are reboxed as vertical lists, preserving their
baseline and limit placement.

LuaTeX keeps one linked list through text passes and the pre-linebreak,
linebreak, and horizontal packing filters. With property cleanup enabled,
`lua_node.rs` removes a freed node's Lua property before recycling its handle.
Use `None::<i64>` to write Lua nil: Rust `()` produces zero return values,
not a value accepted by `LuaTable::raw_set`.

Lua glue orders are normal, fi, fil, fill, and filll (0–4). The engine keeps
the existing TeX wire values and stores fi as `GLUE_FI`; translate at Lua
boundaries with `lua_node_pack` rather than copying the internal order.

LuaTeX native PDF `FontBBox` describes the complete font program in 1000-unit
coordinates. `Ascent` and `Descent` come from baseline metrics in the font
headers, not the potentially multi-em math glyph bounds. Per-character Lua
metrics remain typesetting metrics, not a substitute for either.

Lua PDF text follows [upstream positioning](https://github.com/TeX-Live/texlive-source/blob/trunk/texk/web2c/luatexdir/pdf/pdfpage.c):
font changes establish an absolute text matrix. The glyph pen retains
1/10000-em units and truncates when emitting coarser TJ adjustments; pdfTeX
keeps its integer-sp raster and relative text moves.

OpenType Lua fonts share a PDF font owner when their `filename` and `fullname`
match ([`font_shareable`](https://github.com/TeX-Live/texlive-source/blob/trunk/texk/web2c/luatexdir/pdf/pdffont.c));
`pdf.getfontname` reports that owner. Their two-byte codes are allocated per
glyph and the ToUnicode text is settled when fonts are written, after
`finish_pdffile` (where luaotfload's harf mode assigns final `tounicode`
values), following `write_cid_tounicode`: the sharers are visited by id and
their marked characters by code (the owner also holds every sharer's marks),
and a glyph takes its first value from the showing font's `tounicode`, then
the owner's, or the character code when neither font enables `tounicode`.

Lua characters keep scalar metrics inline; kerning, ligatures, math variants
and kerns, successors, extensible recipes and virtual packets live in optional
`LuaCharExtras`. Ordinary characters allocate no extras. Packet presence is
distinct from its contents, so an empty packet is not an absent packet.
Short ToUnicode byte sequences also stay inline; long sequences retain owned
storage. Font snapshots and rendering read the exact bytes through borrowed
accessors without allocating a glyph-sized copy.

LuaTeX's expansion solver retains a positive shrink half-step; pdfTeX keeps
its negative half-step. With no glue stretch, that distinction determines
whether an expandable discretionary candidate stays active.

Embedded Lua file handles keep file bytes outside the Lua string heap.
Immutable SFNT fonts have exact build-time metadata byte ranges, including
table directories, names, baseline metrics, layout/variation tables and CFF
metadata INDEXes. Seeking and reading those ranges does not inflate the
complete font. Reads outside them materialize the original program; line,
numeric and whole-file reads retain their normal semantics. Explicit close
releases any decoded buffer immediately, even when the handle stays live.
`loadfile` and `dofile` still read complete chunks. `LuaBytes` transfers its
owned byte vector into the VM without a second full-buffer copy.

`fontloader.info` also reuses parsed SFNT metadata for exact immutable virtual
paths. Its name selection and returned values match the runtime parser;
project and external fonts still use that parser, without basename shadowing.

LPeg captures are traced by the Lua VM rather than held as independent
registry roots. Unreachable pattern/closure cycles are collectable, while
live patterns retain their captured values across collection.

Lua GC ownership records retain each slot's strong pool reference but reuse
the allocation's tagged pointer for its type. Concrete typed conversions and
destruction preserve slot reuse, boxed allocations and pool lifetime; moving
between generations does not copy an allocation or change collection pacing.

## Executables and dispatch

`crates/tex-cli` defines these Cargo binaries: `texres`, `texmk`, `pdflatex`,
`tex-bibtex`, and `latexdiff`; the `xelatex` and `lualatex` personalities exist
only as links to `texres` (or `texmk`). Releases ship only `texres`.

- `texres` (`src/bin/texres.rs`) is `texmk::main`. It decides what to run in this
  order:
  1. `TEXMK_INTERNAL_MODE=engine|bibtex|latexdiff` (set by the driver for its
     child processes; the engine program name comes from
     `TEX_SUITE_PROGRAM_NAME`, default `pdflatex`);
  2. a first argument `latexdiff`, or `fmt` (the source formatter,
     `tex_format::cli::run`; see below);
  3. the invoked file name: `pdflatex`, `xelatex`, `lualatex` run one engine
     pass, `bibtex`/`tex-bibtex` run BibTeX, `makeindex` runs makeindex,
     `latexdiff` runs the diff, and anything else (`texres`, `texmk`,
     `latexmk`) runs the build driver.

  Engine and BibTeX personalities started this way set `TEX_RS_HERMETIC=1`,
  so they resolve TeX files only from the embedded archive (see below).
- The `pdflatex` Cargo binary calls the engine entry point directly and does
  **not** set `TEX_RS_HERMETIC`, so it also searches `TEXINPUTS`, `TEXMFHOME`,
  executable-relative `texmf` trees, and system TeX trees. Use `texres` through
  a link named `pdflatex` to reproduce shipped behavior.
- The engine kind follows the program name: `pdflatex` runs pdfTeX, `xelatex`
  runs XeTeX, `lualatex` runs LuaTeX. Each program loads its own format
  (`default.fmt.zst`, `xelatex.fmt.zst`, `lualatex.fmt.zst` under
  `crates/tex-cli/assets`, rebuilt by `scripts/build_formats.py` from TeX
  Live's `pdflatex.ini`, `xelatex.ini`, `lualatex.ini`), unless `-fmt` names
  another dump, which must have been made by the same engine. An empty
  asset boots the format from the sources on every run.
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
   directory, and select the engine (`-pdf`/`-xelatex`/`-lualatex`; otherwise
   `xelatex` when the preamble loads `fontspec`, `xeCJK`, `ctex`, a `ctex`
   class, `unicode-math`, or `polyglossia`, else pdfLaTeX). Locate the per-job cache directory and take its lock.
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
   Biber runs instead when biblatex wrote `<job>.bcf`, and only on a complete
   control file (one Biber can parse): a pass that stopped early leaves the
   root element unclosed, and such a file is never handed to Biber (after a
   finished pass, a warning reports it). Before the
   first pass, a bibliography tool also refreshes the `.bbl` from the previous
   build's auxiliary state, except when that state comes from a pass that did
   not finish: the manifest's `unfinished-pass` flag is set before a pass
   starts and cleared once one finishes, so after an aborted (error limit,
   emergency stop) or interrupted pass LaTeX reruns before BibTeX, Biber or
   makeindex reads its possibly truncated `.aux`, `.bcf` or `.idx`.
   Then, as latexmk does, makeindex (`makeindex -o X.ind X.idx`, run in the
   auxiliary directory) processes each `.idx` file the pass announced with
   `Writing index file X.idx` (makeidx, multind, imakeidx, index.sty), unless
   its input and `X.mst` style are those of the run that wrote the existing
   `X.ind`. An `X.ind` rewritten since by imakeidx (which runs makeindex
   itself through `\write18`, with its `options=`) is left alone, so the next
   pass reads imakeidx's output as in TeX Live.
6. The build is stable when the auxiliary snapshot did not change during the
   pass, or on a first pass that started from existing auxiliary state when
   there are no pending signals and the only auxiliary files are `.aux` files containing nothing but inert lines
   (`\relax`, `\@abspage@last`, and a few `\providecommand` lines). A sticky
   `Rerun to get` message alone does not force another pass.
7. Without stability after five passes the build fails. On success the PDF
   and SyncTeX file are published atomically and the manifest is written;
   see [ARTIFACTS.md](../ARTIFACTS.md) for cache contents and cleanup.

### Watch mode (`-pvc`, `--watch`, `-w`)

`watch_main` runs the algorithm above as `build` in a loop: build, derive the
dependency set, wait for a change, repeat. `-c`/`-C` are rejected with it.
A build that fails after the driver located its files (TeX errors, a missing
`\input`, a failed BibTeX run) prints its report and the loop keeps watching;
a failure before that (unreadable options, missing main file on the first
build) ends the run with the build's status. Ctrl-C sets a flag (`SIGINT`
handler, or the console control handler on Windows); the loop finishes any
running build, releases the job lock, and exits 0.

- **Dependencies** (`watch_dependencies`) are read from data the driver
  already keeps; nothing else tracks reads. The engine's `-recorder` file
  (`<job>.fls`, plain web2c `PWD`/`INPUT`/`OUTPUT`) supplies `INPUT` lines
  (`\input`/`\include` files, packages, classes, images). What the recorder
  lacks comes from a private file in the job's engine cache
  (`.texmk-watch-dependencies`, never exported), which the engine writes only
  when texmk names it in `TEX_RS_TEXMK_WATCH_DEPENDENCIES`: `FONT` lines (the
  font files from `FontLoader::dependency_files`) and `MISSING` lines (the
  engine's failed lookups in project directories), so creating a file that a
  failed build asked for triggers the next build. The `.bib`/`.bst` files
  named by the `.aux` graph
  (or the `.bcf` data sources for Biber) and a project-supplied `<job>.bbl`
  are added with the resolvers the bibliography signature uses. Dropped:
  embedded-archive paths, anything under the private jobs directory, and every
  file the build writes (`OUTPUT` lines, the ownership manifest's PDF,
  SyncTeX, exports and auxiliary files), so a build never triggers itself.
  The set is recomputed after every build. If a failed build left the
  recorder untouched, the previous set is kept.
- **Detection** (`src/watch/mod.rs`, pure Rust polling): every 250 ms each
  tracked path's size, modification time and inode are compared with the
  values recorded with its content hash. Only a mismatch, or a modification
  time within 2 s of the recording (which a same-size rewrite could leave
  unchanged), makes the scan re-hash the file, so a touch or an identical
  save is no change. A change is pending until no tracked file has moved for
  250 ms, then the baseline is re-recorded and, if any content really
  differs, the build starts; atomic renames, truncate-and-write and
  backup-and-rename saves therefore yield one build, and a file deleted and
  recreated stays tracked. Polling avoids a platform backend (inotify,
  FSEvents) and behaves the same on network and container filesystems.
- **Changes during a build.** Files tracked before the build keep the
  baseline recorded when it started, so an edit made while it ran is found by
  the first scan afterwards and schedules exactly one more build however many
  saves it took. A file the build discovered and that was modified after the
  build's start (`WatchInputs::started`, taken after EPS conversion) may have
  been read stale and is treated as changed.
- **Output.** After each build: `texmk: [HH:MM:SS] build OK|FAILED (N pages,
  S s)` and `texmk: watching N files (Ctrl-C to stop)` (existing files only);
  after each detected change: `texmk: [HH:MM:SS] changed: FILE...`. The
  driver's own `build OK` line is replaced by the status line; failure
  messages are unchanged.
- **Tests.** `crates/tex-cli/tests/watch.rs` drives the real binary and waits
  on its status lines with bounded timeouts; the polling logic has unit tests
  in `src/watch/mod.rs`.

## Resource resolution

In hermetic mode (`TEX_RS_HERMETIC` set to anything but empty, `0`, or
`false`) `tex_kpse::Kpse` searches the project directory, the embedded
zstd-chunked package archive, and `$TEX_SUITE_DATA/texmf` (or
`$TEXRES_DATA_DIR/texmf`) when set. Otherwise it additionally searches
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

- `crates/tex-core/tests/etex_tracing.rs`: e-TeX trace transcripts
  (`\tracingassigns`, `\tracingrestores`, `\tracinggroups`, `\tracingifs`)
  checked against `pdftex -ini -etex` output.
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

Glue values carry a spec identity (`Glue::spec`: `NO_SPEC`, the shared
`ZERO_SPEC`, or an id minted by `Eqtb::new_spec`). Copies keep it, glue read
by `scan_glue` and arithmetic results get a new one, and eqtb assignment
applies tex.web's `trap_zero_glue`. e-TeX's `reassigning` test compares
these identities, as TeX compares spec pointers. The format dump stores each
glue's identity and the next free id.

Group types follow tex.web's `cur_group` codes so `\tracinggroups` and
`\showgroups` agree with pdfTeX: `\eqno`/`\leqno` push their own math
shift group, `\middle` ends the `\left` group and begins another (shown as
`\middle`), `\mathchoice` parts are math choice groups opened before their
`{` is read, and `\vadjust pre` is a separate adjustment node
(`Node::PreAdjust`) whose group `\showgroups` prints as `\insert1`; its
material precedes the line, display or alignment row that holds it.

Tests that run the built binaries must stay correct when `cargo test` runs in
parallel or several times at once, which share the temp directory and the
user's `HOME`:

- Name temp files and directories with the process id (plus a per-test serial
  or nonce), never with a fixed name.
- A test that expects an engine cache hit sets `SOURCE_DATE_EPOCH` together
  with `FORCE_SOURCE_DATE=1` (without it the cache key holds the live minute
  and a run that starts in the next minute misses), and gives the process a
  private `TEX_RS_CACHE_DIR` and `HOME` (every record snapshots the unindexed
  `~/.texlive/texmf-var` tree, which any TeX Live run extends through
  `mktextfm`).
- Executables that the test or texmk will run (fake engines, copied binaries)
  are created with `install_executable`/`copy_executable` from
  `crates/tex-cli/tests/support`, not with `std::fs::write` plus `chmod`. A
  file this process holds open for writing is duplicated into children that
  other test threads fork, and executing it before they `exec` fails with
  ETXTBSY ("Text file busy").

## makeindex (`crates/tex-makeindex`)

A line-by-line port of TeX Live's makeindex 2.18: `scan.rs` (`scanid.c`),
`style.rs` (`scanst.c`), `sort.rs` (`sortid.c` and Nelson Beebe's `qsort.c`,
whose comparison order decides which of two identical entries is dropped and
the comparison count in the transcript) and `gen.rs` (`genind.c`).
`tex_makeindex::run_cli(args, host)` is the entry point; `DirHost` maps
relative names to a job's directories and finds style files as kpathsea does
(`./name`, then the TeX tree).

- Ground truth is `/usr/bin/makeindex`: the `.ind`, the `.ilg` and the exit
  status match byte for byte. The `.ilg` banner names TeXres.
- Options: `-c -g -i -l -L -q -r -T -o -p -s -t`; `-p even|odd|any` reads
  the page from `X.log`; a lone `X.idx` takes `X.mst` as its style; `-L` and
  `-T` collate with the environment's locale.
- Undefined behaviour of the C code that TeX Live's binary shows is
  reproduced where it is deterministic: a page number with more than ten
  fields overwrites the `level`, `actual` and `encap` characters, `-l`
  comparisons may read past keys that end in a blank, and an unterminated
  style string reports the remains of the previous one.
- Tests: `crates/tex-makeindex/tests/oracle.rs` runs every directory under
  `tests/oracle/` (inputs and `args`) against `expected/` (the files
  `/usr/bin/makeindex` wrote when run with `args` in a copy of the directory,
  and `status`). To add a case, create the directory, run TeX Live's
  makeindex in a copy, keep the new files in `expected/`, and list the case
  in `oracle_cases!`.

## BibTeX (`crates/tex-bibtex`)

One engine, a module-by-module port of `bibtex.web` 0.99e plus TeX Live's
`bibtex.ch`. `tex_bibtex::run(args, version)` is the only entry point; the
`bibtex` binary, `texres`/`texmk` (`crates/tex-cli/src/bibtex/mod.rs`) and
tex-runtime call it.

- `input.rs`: character classes (bytes 128-255 are letters) and the line
  scanner shared by the `.aux`, `.bst` and `.bib` readers. `engine.rs`: state,
  `run`, file lookup. `auxfile.rs`, `bst.rs`, `bib.rs`: the readers; `.bst`
  commands run as soon as they are read, so messages interleave as in BibTeX.
  `exec.rs`: stack machine and built-ins; `text.rs`: string built-ins
  (`purify$`, `change.case$`, `width$`, `text.prefix$`, `substring$`,
  `add.period$`). `log.rs`: terminal and `.blg` output.
- Ground truth is TeX Live 2026 `bibtex`: the `.bbl`, the `.blg` messages and
  the exit status match byte for byte. The `.blg` banner names TeXres, TeX
  Live's usage statistics are not written, and files from the embedded
  archive are announced as `<embedded:NAME>`.
- Exit status: 0 spotless or warnings, 2 error messages (the `.bbl` is
  complete; callers continue with it), 3 fatal, 1 unreadable `.aux` or bad
  command line.
- TeX Live sizes: `ent_str_size = 500`, `glob_str_size = 200000` (these seed
  `entry.max$`/`global.max$`), `max_print_line = 79`.
- Flags: `-terse`, `-min-crossrefs=N` (default 2); other options are reported
  on stderr and ignored.
- Tests: `crates/tex-bibtex/tests/oracle.rs` runs every case directory under
  `tests/oracle/` (aux files and `args`; shared styles and databases in
  `shared/`) against `expected.{bbl,blg,status}` produced by
  `/usr/bin/bibtex` with the banner and usage statistics removed. To add a
  case, create the directory, generate the expected files with TeX Live, and
  list it in `oracle_cases!`.

## Biber (`crates/tex-biber`)

A port of Biber 2.22 (Perl source: `biber-2.22/lib/`). Ground truth is the
pinned 2.22 binary: the `.bbl` (and tool-mode output) matches byte for byte and
the `.blg` warnings match. Biber prints two blocks of messages in Perl hash
order; only those are compared order-insensitively. The `biber` binary and the
`texres` personality symlinked as `biber` share `tex_biber::run_configured`.

- Pipeline (`lib.rs`): read datasources (`bib.rs`, `biblatexml.rs`, remote
  sources in `remote.rs`), latex-decode every non-verbatim/uri `.bib` field
  (Biber's `parse_decode`, before sourcemaps), sourcemaps, `normalize` into
  typed fields/names/lists/dates, inheritance, citekey selection, validation,
  then `process::prepare` and per-datalist `process::contextualize`, and
  `output.rs`.
- Uniqueness (`uniqueness.rs`) ports `process_namedis`, the
  uniquename/uniquelist fixpoint and `DataList::set_uniquelist` literally,
  including state that persists across passes. DataList state is keyed by
  the Names object (`NameList::id`): inherited and cloned lists share it and
  the last citekey wins. Visible names follow `process_visible_names`.
- Option lookups follow `getblxoption`: entry options, then per-type, then
  global; `label{name,title,date}spec` are per entry type.
- Perl regexes (sourcemaps, nosort, nonamestring, ...) run on a Perl-compatible
  VM (`perl_regex.rs`, `perl_pattern.rs`, `perl_vm.rs`); constructs that need a
  Perl interpreter (code blocks) are errors.
- Perl's `/l` follows `LC_ALL`, `LC_CTYPE`, then `LANG` (unset is the C
  locale) on every platform, as the Linux oracle does; nothing asks libc or
  the OS. `test_biber.py` therefore pins `LC_ALL=C.UTF-8` for `expected.*`
  and `LC_ALL=C` for `expected-c.*`.
- Remote datasources (`remote.rs`, `remote/`) port LWP::UserAgent 6.76,
  LWP::Protocol::http(s), Net::HTTP and Net::FTP over std sockets, with rustls
  (ring) for TLS: no curl or native TLS library. Status lines in `Could not
  fetch` errors are LWP's, including its internal 500s; CA selection follows
  Biber's `%ENV` edits, LWP's ssl_opts and IO::Socket::SSL's checks, with the
  oracle's Mozilla::CA bundle (`remote-ca.pem`) built in.
- Tests: `scripts/test_biber.py --biber BIN [--committed-bcf]` compares every
  fixture in `scripts/fixtures/biber/` (`--regen` re-records them with the
  oracle); `crates/tex-biber/tests/corpus.rs` runs the same corpus through the
  library, `tests/errors.rs` fatal tool errors, and `tests/remote.rs` remote
  datasources against local HTTP/FTP/NNTP/Gopher servers. Generated fixture
  families have `scripts/generate_biber_*.py` writers.
- Differential fuzzing: `scripts/biber_fuzz.py --biber BIN --seed N --count
  2000 --jobs J` builds random documents, generates BCFs with
  `/usr/bin/pdflatex` and compares the oracle's `.bbl` with the candidate's;
  failures keep inputs and a replay command. Fix every mismatch by porting the
  Perl logic and add a minimised oracle fixture for each root cause.

## Source formatter (`crates/tex-format`)

`texres fmt` is `tex_format::cli::run`. The library has these parts:

- `format.rs`: one pass over the lines with a small lexer. It keeps a stack
  of open frames (`{` groups, environments, `\item` bodies, `\[`, `\(`, and
  `[` option lists that end a line); a line's indentation is the number of
  indenting frames open after its leading closers (`}`, `\end{..}`, `]`,
  `\]`, and the end of the previous `\item` for an `\item` line). Several
  frames opened on one line add one level. Verbatim environments, verbatim
  arguments that run past the line end (`\url`, `\index`, ...) and guarded
  groups (after `\obeylines`, `\obeyspaces`, `\catcode` of a blank or line
  end, `\endlinechar`, or a command whose definition uses them) make the
  lines that start inside them `Kept`: copied byte for byte. Material after
  a verbatim start on the same line is never modified either. `@` is a
  letter in `.sty`/`.cls` files (`SourceKind::Package`) and after
  `\makeatletter` (until `\makeatother` or the end of the enclosing `{}`
  group); elsewhere `\Q@x@` with a verbatim `\Q` is read as `\Q` plus an
  `@`-delimited argument, since a document may also be `\input` with `@` a
  letter and the verbatim reading changes nothing. Blank runs are
  recorded during lexing; only those may become line breaks (`\item` split,
  wrapping) or have tabs replaced. Wrapping re-lexes the line from a saved
  state up to the chosen break, so continuation lines get the indentation a
  second run computes (formatting is idempotent); the state is only saved
  for lines that may be too wide (indentation at most the current level, a
  tab at most a tab stop). Column alignment runs after the pass, per
  environment instance. Lines between `% texres-fmt: off` and `on` (or
  tex-fmt's `% tex-fmt: ...`) and lines marked `skip` are lexed, to keep
  the frame stack right, and emitted as `Kept`; a marker inside raw
  material is text.
- `Extras::scan` collects project definitions (`\lstnewenvironment`,
  `\DefineVerbatimEnvironment`, `\newminted`, `\newmintinline`,
  `\DeclareUrlCommand`, xparse `v` arguments, `\newenvironment` built on
  verbatim, commands defined with `\verb`/`\catcode`/`\obeylines`,
  `\MakeShortVerb`) and feeds every control word to `SectionFacts::note`.
  The CLI scans the files being formatted and the `.tex`/`.sty`/`.cls`
  files next to them, then the project classes, packages and `\input`,
  `\include`, `\subfile`, `\import` files they name (`SectionFacts::wanted`
  and `resolve`), looked up in the listed directories.
- `sections.rs`: where a blank line (a `\par`) may go before a sectioning
  command. It is a no-op only if the command starts with `\par` itself, as
  `\@startsection` does, given that `\par\par` reads like `\par`.
  `par_sections` returns the commands for which that is known: the class is
  in `VETTED_CLASSES` (a project class counts only as a layer over one),
  every package is in `SAFE_PACKAGES` or a project file, no project file
  defines, `\let`s, patches, hooks (`cmd/section/...`) or `\csname`-defines
  the command or `\@startsection` (a definition whose body starts with
  `\par` or `\@startsection` is fine), every named file was found, and
  there is no `\DocumentMetadata`. A sectioning command preceded (across
  blanks, comments, `{` and `*`) by a control word other than a few
  harmless ones (`\clearpage`, `\appendix`, ...) counts as touched.
  `SAFE_PACKAGES` was built from TeX Live 2025: packages whose sources and
  every file they load neither define nor patch nor hook into `\part`,
  `\section`, `\subsection`, `\subsubsection` or `\@startsection`, plus
  packages whose only hits were uses (natbib's `\bibsection`) or patches
  after the leading `\par` (parskip, biblatex's `refsection` hook), checked
  by reading them; titlesec (it assigns `\thetitle` before its `\par`) and
  placeins are out. Extending either list needs the same check.
- `packages.rs`: the classes and packages whose TeX Live 2026 sources (with
  every file they load) were read by hand for verbatim constructs; what
  they define is in `format.rs`'s tables. `packages/screened.rs`, written
  by scripts/generate_fmt_screened_packages.py, adds those whose sources
  and loaded files use none of the means of reading text otherwise
  (control sequences named like `catcode`, `verbatim`, `rescan`,
  `endlinechar`, `lst...`, `\url`/`\index` aliases, `\begin` of verbatim-like
  environments, xparse `v`, Lua input callbacks, computed loads, loads of
  endfloat/showlabels/ltxdoc); rerun it after changing the hand-checked
  lists or for a new TeX Live. `SectionFacts::unvetted` makes
  `format_source` return `FormatError::UnknownPackage` when the project
  loads anything else that is not a project file or in `known-packages`.
  A load by macro name (`\LoadClass{\@tufte@class}`) counts as the names
  the project defines the macro to (`\def`, `\newcommand` and the like
  with a plain name as body); any other definition of it, none at all, or
  a kernel scratch macro (`\@temp...`, `\reserved@...`) makes it unknown.
  `\string\usepackage` is text, not a load.
- `tokens.rs`: the safety check. Input and output are tokenized as TeX reads
  them (category codes of a LaTeX document, `@` as in the formatter, state
  N/M/S per line, trailing spaces dropped, `^^` notation). Verbatim material
  is compared character by character, blanks and line ends included: the
  arguments of verbatim commands (found by the formatter's `Lexicon` and
  scanned with its `scan_span`), short-verb text and verbatim environment
  bodies. Both texts are read one line at a time and compared as they go
  (`Stream`, tokens packed into 64 bits); identical texts are not compared.
  The normalization (`Normalizer`) turns runs of `\par` into one and drops
  a `\par` right before a sectioning command that `par_sections` allows
  (both only after an inactive character, `}`, `$` or `&`: after a control
  sequence the first `\par` may be its argument, as with `\fbox` followed
  by blank lines), and (with `align-columns`) drops spaces next to `&`; any
  other `\par` that appears or disappears is a difference. The formatter
  applies the same rules (`ends_safely`, `par_sections`) before it drops or
  adds a blank line. A difference makes `format_source` return an error and
  the file is left unchanged.
- `bib.rs`: BibTeX databases. `parse` splits the text into entries, kept
  pieces and text outside entries the way BibTeX does (an entry runs to its
  matching `}`, or for `@type(...)` to the first `)` outside braces);
  `@string`, `@preamble`, `@comment`, entries after a `%` on their line and
  entries that do not follow `@type{key, name = value, ...}` exactly are
  kept. `format_bib` rewrites entries with one field per line and aligned
  `=`, copying types, keys, names and values byte for byte, puts one blank
  line next to each entry, then parses its own output and compares the
  items (text outside entries only up to blanks).
- `diff.rs` (unified diffs, Myers with linear-space bisection) and
  `config.rs` (`.texresfmt.toml`: top-level keys only, unknown keys are
  errors).

Tests: `crates/tex-format/tests/format.rs` has one test per rule and safety
case and checks idempotence for each; `sections.rs` and `bib.rs` have unit
tests. The output-identity check is manual: format every document of the
benchmark corpus and the repository's `.tex` fixtures (and their `.bib`
files), rebuild both versions with `SOURCE_DATE_EPOCH`/`FORCE_SOURCE_DATE`
and compare the PDFs byte for byte, the `.bbl` files, and the counts of
errors and warnings in the logs. The CLI formats files on all cores
(`parallel_map`) and reports results in order.

## Reference moved from the README

### Single-pass engine personalities

The single-pass personalities (`pdflatex`, `lualatex`, `xelatex` links) take
pdfTeX's web2c options (`pdflatex --help`). `-ini` dumps `JOBNAME.fmt` into
the output directory. As in TeX Live, an `-ini` run first loads the format
that a leading `&NAME` names (if `NAME.fmt` is missing, `pdflatex` and
`xelatex` load their default format and `lualatex` stops) or, except under
`lualatex`, that a `%&NAME` first line names when that format exists; `-fmt`
loads nothing under `-ini`. Otherwise `-fmt=NAME`, `&NAME`, a `%&NAME` first
line, or `-progname=NAME` load such a TeXres dump (`NAME` equal to the
program selects the built-in format). `-cnf-line=VAR=VALUE` sets a search
or policy variable such as `TEXINPUTS` or `openout_any`; `-kpathsea-debug=N`
(nonzero) traces file lookups in the transcript. `-translate-file=TCXNAME`,
`-8bit` and a `%&-translate-file=` first line select which bytes 128-255 print as
themselves rather than as `^^xx`: `pdflatex` and the built-in format use
TeX Live's `cp227.tcx` table, `-ini` without a table prints `^^` notation,
and the transcript and terminal carry the exact bytes TeX Live writes
(`max_print_line` counts printed bytes). Deliberate differences: e-TeX is
always on (also under `-ini` without `-etex`), missing files are never
generated (`-mktex`), and `-output-format=dvi`, `-enc`, `-mltex`, `-ipc` are
rejected. As in TeX's nonstop mode, a primitive `\input` of a missing file or
an `\openout` that cannot be opened stops the job, since no other name can be
supplied, and so does a terminal `\read`. `\pdffilesize`, `\pdfmdfivesum
file`, `\pdffilemoddate` and `\pdffiledump` search the TeX input path only
(kpse_find_tex), so a TFM, encoding or map file is not found by them.
`\pdffilemoddate` reports a file's modification time like pdfTeX;
files served from the embedded package archive have no timestamp and report
`D:19700101000000Z`.

PDF output follows pdfTeX's own bookkeeping: font, form and image resources
are named `/F<n>`, `/Fm<n>` and `/Im<n>` from the owner font number and the
per-document form/image counts, `\pdfuniqueresname` appends pdfTeX's
CRC-32/base-62 job tag, and objects are numbered in creation order from 1.
The Info dictionary lists Producer, user `\pdfinfo` keys, Creator, dates,
Trapped and `PTEX.Fullbanner` (`PTEX_Fullbanner` with `\pdfptexuseunderscore`,
absent under `\pdfsuppressptexinfo`), and `\pdftrailerid` fixes the `/ID`.
Under LuaTeX the Producer is `LuaTeX-1.24.0` and the banner key is always
`PTEX.FullBanner` (luatex ignores `\pdfsuppressptexinfo` and the underscore
spelling).
`-ini` starts with pdfTeX's `\pdfminorversion=4` and `\pdfcompresslevel=9`.
There is no DVI writer, so `\pdfoutput` starts at 1 where TeX Live's `-ini`
starts at 0.

### XeTeX and LuaTeX modes

**Engine modes and limits:** `-xelatex` (or a link named `xelatex`) runs the
XeTeX engine, version 3.141592653-2.6-0.999998 as in TeX Live 2026, with the
embedded XeLaTeX format built from TeX Live's `xelatex.ini`; the terminal
banner reads `This is XeTeX, Version 3.141592653-2.6-0.999998 (TeXres x.y.z)`.
Output goes to the PDF directly, without an XDV file: `\special`s are
interpreted as `xdvipdfmx` does, pages default to A4 unless `\pdfpagewidth`
and `\pdfpageheight` are set, and the PDF carries xdvipdfmx's producer data.
Shell escape (`\write18`) follows web2c: restricted by default (only the
`shell_escape_commands` list, such as `latexminted` for `minted`, run; they are
found on `PATH` and started with `/bin/sh -c` in the working directory with
`TEXMF_OUTPUT_DIRECTORY` set to the auxiliary or output directory and
`SELFAUTOLOC` to the directory of a `kpsewhich`; `makeindex` commands that
need no shell features run the embedded makeindex in-process instead, on the
job's files in that directory), `-shell-escape` allows any
command and `-no-shell-escape` none. pdfLaTeX has no native fonts: loading
`fontspec` there fails with fontspec's own engine error, as in TeX Live.
`-lualatex` runs the LuaTeX-compatible mode with the embedded LuaLaTeX format
and an in-tree Lua VM, so `\directlua` works. `luatexja` and a few LuaTeX-only
packages do not compile in any mode. Native fonts are loaded after the format
is read, as in XeTeX; dumping native font state is rejected.

### Native fonts and embedding

The font syntax of XeTeX (`"Family/B:feature"`, `"[file.otf]:+liga"`,
`mapping=tex-text`, `color=`, `embolden=`, ...) and fontspec's whole interface
(`\setmainfont`, `\newfontfamily`, `\setCJKmainfont`, `Path`, `Extension`,
`BoldFont`, `FontIndex`, `Scale`, OpenType features, ...) are the upstream
implementations. Their diagnostics are TeX Live's as well: a missing font ends
the run with fontspec's `The font "..." cannot be found` error, a missing
shape produces `Font shape ... undefined`, and a character that a font lacks
is reported as `Missing character: There is no ...`.
Fonts are hermetic: a font is found among the project's files, the bundled
font archive (by file name, or by family, PostScript, or full name from the
bundled font index), and nothing else. TeXres does not search OS font stores,
so a document that selects a system font by name (`Times New Roman`) fails
where TeX Live with that font installed succeeds; ship the font file with the
project and select it with `Path=./`. The bundled OpenType fonts include
Latin Modern (text and math), TeX Gyre (text and math), STIX Two, XITS,
Libertinus, Harano Aji, IPAex, Fandol (the default fonts of `ctex`), and
the other families listed in the lock file.

Mapped TrueType, CFF OpenType, and collection faces are embedded as CID fonts
with glyph addressing and Unicode extraction maps. Subsets are shared across
pages, sizes, aliases, and forms. Type 1 fonts retain their Type 1 representation.
Font licenses, notices, and required corresponding sources ship under
`share/tex-suite/texmf/doc/fonts`; the engine's MIT/Apache license does not
replace those licenses.

### Lua VM (`tex-lua`)

The Lua VM (`tex-lua`) is an ordinary Rust library (`rlib`). Native Lua C
modules loaded with `require`/`package.loadlib` resolve the `lua_*`/`luaL_*`
functions from the host executable, which the workspace links with
`--export-dynamic` (`.cargo/config.toml`); a host that embeds `tex-lua` must
link the same way. Host Rust code that needs to allocate inside a native
callback uses `Lua::create_callback`, whose `CallbackLua` is only borrowed for
the duration of the call. In Lua 5.3 mode (LuaTeX's dialect) every byte >= 0x80
is a letter in a name, so the names `LuaFunction::get_upvalue` and
`set_upvalue` return are the bytes of the source text (`Vec<u8>`), and error
messages, `debug.getlocal`/`getupvalue` and `string.dump` keep them. An error
raised by a function handle that a callback calls while a coroutine (an async
script, or one resumed from Lua) runs reaches that coroutine's `pcall` as the
same Lua value; only a call made by the host alone, with no Lua code running,
returns the message with its stack traceback.
With the optional `sandbox` feature, `SandboxConfig::with_stdlib(Stdlib::Bit32)`
enables Lua 5.3's `bit32` independently of the `math` library; it is hidden until
selected.

### Verification

The Linux CI and release workflows run the font, graphics and engine fixtures listed in
[`scripts/fixtures/fonts/manifest.json`](scripts/fixtures/fonts/manifest.json)
through the built binary (`scripts/test_fonts.py`) with filesystem isolation,
pdf.js and Poppler rendering/text extraction, and TeX Live 2026 as the
reference. The extracted archive is run on every release platform, and the
shell installer, `.deb`, macOS `.pkg`, and Windows installers are installed
and exercised. The workspace test suite,
the C and WebAssembly libraries, and the browser module are tested as well.
The exact minimum documents from [#17](https://github.com/leoliu0/texres/issues/17)
and [#18](https://github.com/leoliu0/texres/issues/18) run under LuaLaTeX and
XeLaTeX respectively, checking Spanish text, embedded fonts and rendering.
LuaLaTeX and XeLaTeX also have subsystem probes and paired package-interaction
documents: source loading, token scanners, register/group scope, Lua callbacks,
node ownership, fonts, math, Unicode, bidirectional text, CJK line breaking and
vertical typesetting. Their complete extracted text is compared with the matching
TeX Live 2026 engine, alongside font-program checks and both renderers.
Use `scripts/test_fonts.py --engine lualatex` or `--engine xelatex` with
`--texres` and `--output` to select a suite; its report groups failures by engine
and feature family and retains compilation and viewer evidence.
See [PERFORMANCE.md](PERFORMANCE.md) for how speed is measured.

### Issue assistant

The issue bot reads the complete issue and triggering comment, selects the
reported `-pdf`, `-xelatex` or `-lualatex` command, and includes the actual
command, binary version and bounded compiler log in its reply. `/reproduce`,
`/test` and `/fix` reuse the issue's document unless the comment supplies a
replacement; an explicit engine option can override the reported engine.
Suggestions and feature requests without TeX are left for maintainer review,
not answered with an irrelevant request for a compilation snippet. AI
diagnosis receives the full report and treats proposed causes as hypotheses.

### Workspace layout

The project is a Cargo workspace:

```
texres/
├── crates/
│   ├── tex-core/        # TeX engine: expansion, typesetting, math, alignment, pages, PDF output, SyncTeX
│   ├── tex-kpse/        # kpathsea-style resolver and the embedded zstd-compressed package archive
│   ├── tex-bibtex/      # BibTeX implementation
│   ├── tex-biber/       # Biber (biblatex backend) implementation
│   ├── tex-cli/         # `texres` executable: build driver, engine/BibTeX personalities, latexdiff
│   ├── tex-format/      # `texres fmt`: LaTeX source and BibTeX database formatter
│   ├── tex-lua/         # Lua VM used by the LuaTeX-compatible mode
│   ├── tex-mplib/       # MetaPost engine (mplib)
│   ├── tex-ps/          # PostScript/EPS interpreter and PDF renderer
│   ├── tex-runtime/     # in-process, in-memory compilation API
│   ├── libtex/          # C ABI over tex-runtime
│   └── tex-wasm/        # WebAssembly bindings over tex-runtime
├── packaging/           # installers and native packages (Linux, macOS, Windows, AUR)
└── scripts/             # packaging, font/corpus test harnesses, library builds
```
