# NOTES-BibRust — Rust BibTeX engine (tex-bibtex)

## What was built
`crates/tex-cli/src/bin/bibtex.rs` + `crates/tex-cli/src/bibtex/{mod,bib,bst,exec}.rs`.
Binary: `tex-bibtex <dir/jobname>` (also accepts explicit `.aux`). Reads
`<dir/jobname>.aux`, writes `.bbl` + `.blg` alongside. Exit 0 = success
(warnings ok), 1 = errors, 2 = fatal (missing files/bad aux). Accepted flags:
`-terse`, `-min-crossrefs=N` (default 2).

Wiring: bin root uses `#[path = "../bibtex/mod.rs"] mod bibtex;` because
tex-cli has no lib target. Only external dep is `tex_kpse` (Bst/Bib file
lookup; aux-dir-relative paths are tried first).

## Semantics — ported from bibtex.web 0.99e (fetched from texlive-source)
Ground truth oracle: `/usr/bin/bibtex` (TeX Live 2026). Verified BYTE-EXACT
.bbl parity against it on references.bib (173 entries, synthetic aux of all
keys) with styles: **rfs, plain, abbrv, alpha, unsrt, plainnat, siam, acm,
apalike, ieeetr, chicago** — all identical. Plus 4 crossref scenarios
(parent cited / 2 children / 3 children / parent-after-children), @string,
@preamble, accented names (`{\'\i}`), nested `{}` titles.

### Load-bearing details (each was a real parity bug)
- `.bst` function bodies contain inline `{...}` function literals for if$/
  while$ branches; strings may hold UNBALANCED braces (`"\bibitem[{"`), so
  group extraction and body lexing must be quote-aware (bst.rs).
- FUNCTION/EXECUTE/etc. group names need trimming (` p ` != `p`).
- .bst identifiers resolve CASE-INSENSITIVELY (chicago.bst uses `Volume` for
  the `volume` field). Centralized in `Interp::lookup`.
- `%` comments inside ENTRY/INTEGERS/STRINGS groups must be stripped
  (apalike has `%    month  not used in apalike`).
- TeX Live bibtex build constants: **ent_str_size=500, glob_str_size=200000**
  (NOT the stock WEB 100/1000). entry.max$/global.max$ seed from these; sort
  keys truncate at 500. Database fields are never truncated.
- Sorting: stable by sort.key$ bytes, ties by current cite-list position
  (WEB less_than). ITERATE/REVERSE run in current sorted order.
- Discretionary tie in format.name$: the enough_text_chars(3) count must
  EXCLUDE the trailing tie itself (WEB decr(ex_buf_ptr) first).
- .bbl line wrapping: buffer flushed on newline$; write$ wraps at 79
  (max_print_line), breaking at whitespace (scan back to index 3, else
  forward from 80), continuation lines indented with 2 spaces; all-space
  lines dropped, empty buffer writes blank line. Trailing whitespace stripped
  per line.
- empty$: missing field = empty; string empty iff all whitespace.
- `preamble$` pushes the concatenation of all @preamble strings.

### Crossref model (sequential, stateful — matches bibtex.web)
- Aux cites seed the list; .bib entries are read IN ORDER only if their key
  is already cited. Storing a child's crossref ADDS the parent (count=1) or
  increments (if parent itself was crossref-added); a parent defined before
  its children is therefore never read → "A bad cross reference---entry ...
  refers to entry ..., which doesn't exist" error + child's crossref cleared.
- Read parents: children inherit missing fields (never `crossref`) and the
  child's crossref value is canonicalized to the parent's spelling.
- Crossref-added parents are included only at count >= min_crossrefs;
  under-count clears children's crossref fields but KEEPS inheritance.
- Aux-cited-but-missing keys warn "I didn't find a database entry".

### Undefined macro in a .bib value
Real bibtex silently drops the field (journal=jf with no @string behaved as
missing). We warn in .blg and skip the field.

## Known deviations (minor)
- .blg wording/ordering approximates real bibtex (not byte-exact; nothing
  parses it strictly).
- Bad-crossref error text uses our format with same content.
- `@string` redefinition warns and overrides (real errors).
- No `\bibstyle` fallback: missing \bibstyle/\bibdata are fatal (real same).

## Integration hooks
- `texmk` invokes `tex-bibtex <dir/jobname>` — contract confirmed with
  TexMk-4 (exit 0 incl. warnings).
- For latexmk-like flows the .bbl lands next to the .aux; natbib/rfs
  .bbl requires `\usepackage{natbib}`.
