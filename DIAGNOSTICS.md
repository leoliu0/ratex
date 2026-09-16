# Diagnostics

The engine reports TeX errors with the physical source location, a bounded
source excerpt, a caret, macro-expansion context, include ancestry, and a short
recovery hint when one is known. For example:

```text
! Undefined control sequence \printtotl
  --> chapters/results.tex:18:9
   |
18 | Result: \printtotl
   |         ^^^^^^^^^^
  = while expanding: \resultsrow -> \printtotl
  = included from main.tex:42:1
  = help: check the command spelling; if a package defines it, load that package before use
```

Locations refer to the bytes TeX actually read. The scanner records the start
and physical width of each file token, so `^^` spellings, active characters,
tabs, non-UTF-8 input, generated `\csname` tokens, and very long control
sequences do not shift the reported location. Carets use display width for tabs,
combining marks, and wide Unicode characters.

Macro traces start with the call visible in the source and continue toward the
operation that failed. Runaway arguments and definitions retain that trace even
when the input file has reached EOF. If the trace exceeds `\errorcontextlines`,
an ellipsis separates the outer source call from the newest retained frames.
Errors in included files point into the child and list the exact `\input` sites
through the parent chain. Structural EOF checks report where an unfinished
conditional, group, math formula, box, or alignment opened. Numeric scanners
identify the offending operand, state the legal range, and say whether TeX
ignored the assignment, substituted zero, or left the old value unchanged.
Arithmetic errors name the operation and preserve the destination value.

Inspection primitives such as `\show`, `\showthe`, `\showtokens`, `\showbox`,
`\showlists`, `\showgroups`, and `\showifs` use the same structured, bounded
output instead of dumping internal engine state. File, font, image, PDF object,
input-stream, output-stream, format, and final PDF write failures name the
resource and retain the operating-system error when available. Missing glyphs
in text and math are warnings: they include the character code and selected
font name, point to the input atom, and explain that the character was omitted.
Invalid PDF object references are rejected before they can leave broken page
resources. Deferred `\pdfsave`, `\pdfrestore`, and `\pdfsetmatrix` failures
retain the command that created them, and invalid `\pdfmatch` expressions
include the regular-expression parser's reason.

Errors and ordinary warnings are written once to the transcript and, except
while `batchmode` is active, to standard error. Box-quality warnings preserve
TeX's `\tracingonline` rule: they are always written to the transcript and are
shown on standard error only when `\tracingonline` is positive. Successful
progress remains on standard output. Interaction-mode changes apply when each
message occurs, so a later mode change cannot retroactively hide or reveal
earlier output. The command-line controls are:

- `-interaction=errorstopmode` stops at the first error and is the default.
- `-interaction=nonstopmode` and `-interaction=scrollmode` continue after
  recoverable errors, produce a PDF when possible, and still exit with status 1.
- `-interaction=batchmode` has the same recovery policy without terminal output;
  diagnostics remain in the `.log` file.
- `-halt-on-error` stops after the first error in every interaction mode.
- `--max-errors=N` bounds recovery in the continuing modes.
- `-file-line-error` is accepted for compatibility; rich file and line output is
  always enabled.

`\errorcontextlines` limits macro and include notes without changing the primary
source location. Macro history and include ancestry each receive that bounded
budget. An explicit zero hides contextual notes; the negative default used by
current LaTeX formats selects five useful frames. `\errhelp` supplies the hint
for `\errmessage`; obsolete interactive instructions are removed while any
remaining package advice is retained. Ordinary engine errors receive specific
built-in guidance. Messages, help text, traces, control-sequence names, include
depth, inspection output, and source windows all have fixed output bounds so
malformed input cannot produce an unbounded diagnostic.

Library users can inspect `Engine::diagnostics`; each `Diagnostic` contains its
`DiagnosticSeverity`, message, primary `SourceContext`, highlight width,
expansion frames, include frames, and optional help. At most 128 diagnostics are
retained. If more occur, one warning marks the omitted entries and the newest
event remains available; transcript output has its own byte bound. The list also
records the terminal diagnostic that stops recovery at `--max-errors`.
`Diagnostic::render` produces the command-line form.
