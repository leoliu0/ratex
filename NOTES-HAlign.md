# NOTES-HAlign (align.rs — \halign implementation)

## Status
COMPLETE. `cargo test -p tex-core --lib align::` → 6 passed, 0 failed/ignored
(including `\hbox`-cell and `\vbox`-nesting coverage).

## Architecture (how \halign works in this engine)
- `begin_halign`: requires vmode. `scan_align_preamble` consumes `{` then
  scans entries `u # v` until `\cr`/`\crcr`. Entry separators are `&` at
  depth 0 (LaTeX tabular emits exactly these between column templates) and
  preamble `\span` (adds +1 span, resets to u-part accumulation). The final
  entry at `\cr` is ALWAYS pushed, even when completely empty (`#\cr` is a
  legal single bare column — this bit us once). Then pushes the alignment
  group: saved_lists + eqtb level + box_targets/shifts + box_kinds=**7**
  (build.rs end_box pops these for kind 7 and calls `finish_halign`).
- Cells: `align_start_cell` pushes a cell group (saved_lists/eqtb/targets/
  shifts/kinds=**8**) + RestrictedHorizontal, records the row slot
  (span = preamble span), then peeks ONE expanding token: `\omit` → skip
  template (PH_OMIT bit); otherwise the u part is pushed as an input source
  (name `<align-u>`) and the peeked token re-queued UNDERNEATH it (input
  stack order, NOT the `pushed` vec — `pushed` has highest priority and
  would preempt the u part).
- Close (&/\cr): `align_push_close(row_continues)` pushes
  `[v part..., \crcr-token]` as source `<align-cell>` (v skipped if omitted)
  and sets PH_CLOSE. **The sentinel is the `\crcr` PRIMITIVE token
  (`cs.lookup(b"crcr")`), NOT a magic token.** expand.rs intercepts
  CELL_END_TOKEN inside `get_token_inner`, i.e. inside EVERY token fetch —
  a scanner lookahead (e.g. `\hskip1em`'s fill check after `\def\quad
  {\hskip1em\relax}` minus the \relax) swallows CELL_END and packs the cell
  MID-SCAN; a `\crcr` cs token is pushed back by scanners and only acted on
  at dispatch depth (tex.web frozen-\cr semantics).
- `align_cr` dispatch: PH_CLOSE set → clear it and finish synchronously
  (`align_finish_cell_now` / `align_finish_noalign_now` via
  `align_pop_cell_group`, guarded on box_kinds.last()==8); else if phase
  IDLE → ignore (between rows the inspector consumes \cr/\crcr); else push
  close for row end.
- Row end: `align_finish_row` stores the row, `align_row_inspect` peeks the
  next token: `\noalign` → `align_noalign` (typesets text in
  InternalVertical via a kind-8 group, sentinel-finished, then re-inspects);
  `\cr`/`\crcr` → consumed; `}` → align_done=true, token pushed back
  (end_box kind 7 does the rest); anything else → start next row
  (\everycr plays BEFORE the row's u part, i.e. pushed last/on top).
- `\span` in data: row slot span += 1, discard any u-part remainder, peek
  for \omit (LaTeX `\multispan` emits `\span\omit` pairs), push the absorbed
  column's u part. Base col stays; next cell starts at col+1+span.
- `finish_halign`: widths[0..n] = max natural width of single-column cells;
  span cells (sorted by span) distribute any deficit equally over the
  covered columns; each cell's inner list is RE-PACKED to
  sum(widths[lo..=hi]) + span*tabskip (the "unset box" pass — \hfil
  stretches); tabskip glue between grid columns; noalign entries
  (span == NOALIGN_SPAN = u16::MAX) inserted as-is; interrow
  baselineskip/lineskip glue via `align_interline` (page.rs-style formula,
  stretch/shrink copied from \baselineskip); vpack(VBOX) appended via
  setbox_target/append_box_node.
- Nested \halign (tabular in a p-cell): state saved/restored through a
  thread-local ALIGN_STACK keyed by `self as *const Engine as usize`.
- Phase encoding lives in `align_state` (unused elsewhere): 0=idle, 1=u
  playing, 2=content, +4=omit, +8=close-pending. `align_scanning_cell` =
  "cell is followed by another in this row" (decides next-cell vs row end
  at sentinel time). `align_cell_toks`/`align_noalign_toks` engine fields
  are now UNUSED by this module.
- \valign: maincontrol.rs errors "\\valign not implemented" and ends the
  job — graceful, unchanged.

## Known simplifications vs tex.web
- Unset-box glue: re-packing the cell list means finite glue stretches too
  (TeX freezes finite glue in unset boxes). Only observable with glue like
  `\hskip 0pt plus 2pt` in a template.
- \everycr plays at row start before the u part (TeX: after every \cr);
  equivalent for the empty-\everycr case used by plain/LaTeX.
- Span deficit distribution is an equal split; TeX uses per-span nodes
  (only differs with multiple conflicting spans).

## RESOLVED: begin_box brace bug (was blocking tabular; fixed by BoxesPack-4)
`\hbox{a}` used to leave a stale box_kinds entry: begin_box pushed the box's
group context but did not consume the `{`; the `{` then dispatched →
begin_group → a SECOND group entry, and the single `}` popped only the inner
one. tex.web begin_box does scan_spec THEN scan_left_brace. BoxesPack-4
fixed begin_box to consume the brace itself (and hardened end_box against
desync). align.rs needed no change: its pop guard
(box_kinds.last()==CELL_GROUP_KIND) rejected premature finishes gracefully
while the bug existed.

## Other integration notes for Main
- `setbox0=\halign{...}`: DONE by Main (io.rs do_setbox `b"halign"` arm →
  setbox_target + begin_halign; finish_halign honors setbox_target).
- Test harness facts: an UNBOOTED engine has plain-catcode defaults
  (`{`=12!) — tests must set `\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
  \catcode`\&=4`; call `Engine::init_primitives()` + `add_nullfont()`
  (font 0 = nullfont convention; real fonts start at 1); engine default
  \baselineskip is 0pt.

## Regression suite (in align.rs #[cfg(test)])
- halign_two_rows_two_columns: the acceptance driver (with `\def\quad` and
  catcodes; cmr10). Verifies 2-row vbox, equal row widths = sum of column
  widths, per-cell repack targets, fil-glue set 8.33 on 'a'+'quad' cell,
  natural-width 'ccc' cell, col0 = 3*wc+quad, col1 = 2*wb.
- noalign_hrule_between_rows: `\hbox` cells; rule lands inside a noalign
  vbox between rows (exercises begin_box-inside-a-cell).
- span_cell_covers_two_columns: `\span\omit` widens cols 1+2 equally.
- valign_errors_gracefully / misplaced_align_tokens_error.
- halign_inside_vbox: `\vbox{\halign…}` group nesting end-to-end.
