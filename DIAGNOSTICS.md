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
through the parent chain. Structural
EOF checks report where an unfinished conditional, group, math formula, box, or
alignment opened.

Diagnostics are written once to the transcript and, except in `batchmode`, to
standard error. Successful progress remains on standard output. The command-line
controls are:

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
source location. `\errhelp` supplies the hint for `\errmessage`; ordinary engine
errors retain their specific built-in guidance.
Messages, help text, traces, control-sequence names, include depth, and source
windows all have fixed output bounds so malformed input cannot produce an
unbounded diagnostic.

Library users can inspect `Engine::diagnostics`; each `Diagnostic` contains the
message, primary `SourceContext`, highlight width, expansion frames, include
frames, and optional help. `Diagnostic::render` produces the command-line form.
