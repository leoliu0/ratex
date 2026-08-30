# NOTES-TexMk.md

## Session 1 (2026-08-30) — texmk driver complete

### Delivered
- `crates/tex-cli/src/bin/texmk.rs` (was placeholder): full latexmk-style driver,
  deliberately **std-only** (no tex-core/tex-kpse import) — it is a pure process driver
  and compiles standalone: `rustc --edition 2021 -o texmk crates/tex-cli/src/bin/texmk.rs`.
  `cargo check -p tex-cli` passes.

### Semantics implemented
- Pass loop, max 5 pdflatex passes (`-interaction=nonstopmode` always added).
- Stability signals, parsed from captured stdout **plus** `<job>.log` if present:
  - rerun hints: "Rerun to get" / "Label(s) may have changed"
  - "There were undefined references" / "There were undefined citations"
  - per-line "Citation … undefined"
  - "No file <job>.bbl" plus a direct `.bbl`-existence check (bibdata && !bbl)
- Artifact hash snapshot each pass: `.aux/.toc/.lof/.lot` (existence+content via
  DefaultHasher) — appearance/disappearance counts as a change.
- bibtex trigger: `.aux` has `\bibdata` AND (first pass: bbl missing || undef cites;
  later passes: `\citation` set changed since previous pass). After the first bibtex
  run, later reruns ONLY on citation-set change (prevents infinite bibtex loops when
  an entry is simply missing from the .bib).
- Stuck detection: identical artifacts + identical signals on consecutive passes ⇒
  exit 1 with a targeted message (deterministic engine cannot progress); avoids
  burning all 5 passes on unfixable docs.
- `\citation{a,b}` multi-key lines parsed; `\bibdata{...}` line-prefix match.
- CLI: `file.tex`, `-output-directory DIR` (also `-outdir`, `--output-directory`,
  and `-opt=value` forms; dir auto-created like latexmk), `-jobname`, `-interaction=X`
  (space or = form), `-halt-on-error`, unknown flags forwarded verbatim,
  `--silent|-q` (suppresses engine output + banners; failures still print output),
  `-h/--help`, `-v/--version`.
- Tool resolution: sibling of the texmk executable first, then PATH. pdflatex = `pdflatex`;
  bibtex tries `tex-bibtex` then `bibtex` (Cargo bin is `tex-bibtex`).
- bibtex invoked as `tex-bibtex <dir/job>` WITHOUT `.aux` extension — contract confirmed
  with BibRust-4 (reads <dir/job>.aux, writes .bbl/.blg alongside, exit 0/nonzero).
- Exit codes: 0 converged; 1 pdflatex/bibtex failure, missing tools/input, stuck, or
  5-pass exhaustion; 2 usage.

### Validation (shim harness, all green — final binary)
Fake pdflatex/tex-bibtex bash shims + texmk copy in `/tmp/texmk-shim/`; stateful shim
simulates a refs+cite doc. Test dirs under `/tmp/textest-mk/`:
- A: refs+cite fresh build → **pdflatex x3, bibtex x1, exit 0, m.pdf written** (the
  acceptance sequence, driver side)
- B: stable doc → 1 pass, 0 bibtex, exit 0
- C: diverging doc → exactly 5 passes, exit 1
- D: bibtex exits 1 → texmk exit 1
- E: `--silent` → zero output lines, exit 0
- F: `-output-directory=out` → all artifacts in out/, exit 0
- Edge: missing file → 1; no args → 2; `=`-form flags OK; --version OK.

### Real-engine status (blocked on boot, NOT on texmk)
- Ran the real `target/debug/pdflatex` (12:56 build) via texmk on `/tmp/textest/m.tex`
  (created: 2 sections + \ref/\label + \cite x2 + \bibliographystyle{plain} +
  \bibliography{refs}; `/tmp/textest/refs.bib` with knuth1984/lamport1994 created).
- Result: LaTeX format boot still fails (`latex.ltx` line 12831, `\f@encoding` cascade;
  root = the known `\expandafter\chardef\else` def-hijack, see NOTES-BootDump) →
  pdflatex exits 1 → texmk correctly reports "pdflatex failed on pass 1", exit 1.
  This is Main's file territory (expand.rs/scan.rs/control.rs); texmk needs no change
  once boot passes. Re-run: `cd /tmp/textest && ./bin/texmk m.tex`
  (`/tmp/textest/bin/` holds copies of pdflatex + tex-bibtex + current texmk; refresh
  the copies from target/debug after the next build).
- Note: `cargo check -p tex-cli` went from 4 tex-core errors to green DURING this
  session (Main landing fixes) — after the next `build.sh`, the real acceptance
  (m.tex → 3 passes + bibtex → m.pdf) should just work through the same driver paths
  the shims validated.

### Integration notes for Main
- Nothing to wire: bin entry `texmk` already in crates/tex-cli/Cargo.toml.
- pdflatex.rs does NOT write eng.term to a `<job>.log` file; texmk parses stdout
  (engine warnings reach stdout via \write16→term). If a .log writer is added later,
  texmk already parses both surfaces.
- `\openout` prefixes `out_dir` (io.rs:79) — aux/toc land in -output-directory. ✓
