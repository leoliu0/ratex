# NOTES-BibRust — Rust BibTeX engine (tex-bibtex)

## Layout
One engine, a module-by-module port of bibtex.web 0.99e plus TeX Live's
bibtex.ch changes, in `crates/tex-bibtex/src`:
- `input.rs`: character classes (8-bit: bytes 128-255 are letters) and the
  line scanner shared by the `.aux`, `.bst` and `.bib` readers.
- `engine.rs`: global state, `run(args, version)` and file lookup.
- `aux.rs`, `bst.rs`, `bib.rs`: the three readers; `.bst` commands run as
  soon as they are read, so messages interleave exactly as in BibTeX.
- `exec.rs`: the stack machine and built-ins (`format.name$` included);
  `text.rs`: side-effect-free string built-ins (`purify$`, `change.case$`,
  `width$`, `text.prefix$`, `substring$`, `add.period$`).
- `log.rs`: terminal and `.blg` output and the job history.

`tex_bibtex::run` is the only entry point, used by the `bibtex` binary,
`ratex`/`texmk` (tex-cli `src/bibtex/mod.rs`) and tex-runtime.

## Behavior
- Ground truth: `/usr/bin/bibtex` (TeX Live 2026). The `.bbl`, the `.blg`
  messages and the exit status match it byte for byte; only the usage
  statistics TeX Live appends to the `.blg` are not reproduced.
- Exit status as in TeX Live: 0 spotless or warnings, 2 error messages (the
  `.bbl` is still complete and callers continue with it), 3 fatal error,
  1 unreadable `.aux` or unusable command line.
- TeX Live sizes: `ent_str_size`=500, `glob_str_size`=200000 (these seed
  `entry.max$`/`global.max$`), `max_print_line`=79.
- Flags: `-terse`, `-min-crossrefs=N` (default 2); other options are
  reported on stderr and ignored.

## Tests
`crates/tex-bibtex/tests/oracle.rs` runs every case directory under
`tests/oracle/` (aux files and `args`; styles and databases in `shared/`)
and compares against `expected.{bbl,blg,status}` produced by TeX Live's
`bibtex` (banner and usage statistics removed from the `.blg`). To add a
case, create the directory, generate the expected files with
`/usr/bin/bibtex`, and list it in `oracle_cases!`.
