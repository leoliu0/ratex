# NOTES-PageShip (PageShip-4, page builder session)

## What was implemented (crates/tex-core/src/page.rs, full rewrite)

TeX82 page builder per tex.web build_page/try_break/fire_up/vsplit_page:

- **Accounting** (`PageState`): `page_so_far` = natural height with the
  "recent contributions" running depth folded in; stretch/shrink tracked by
  glue order (pt/fil/fill/filll, fil-glue width folded into its order bucket);
  depth beyond `\maxdepth` charged to height; `\prevdepth` sentinel
  `-1000pt` (DEPTH_NONE) for fresh pages.
- **Contributions**: glue/kern/penalty/mark nodes arriving before the first
  box of a fresh page are DISCARDED (tex.web); first box gets `\topskip`
  glue (width = max(0, topskip − box height), an actual node, so it appears
  in \box255); interline glue (baselineskip vs lineskiplimit → lineskip)
  between boxes; inserts start the page without topskip (tex.web
  `insert_there` state ≈ goal_set).
- **Breakpoints**: glue after a box (penalty 0) and penalties < 10000 after
  a box; cost = badness + penalty + `\insertpenalties` (tex.web @1003/1004,
  deplorable=100000 at awful badness). Best = least cost, ties keep the
  LATER break (tex.web `c≤best_cost` updates). Fire when: penalty ≤ −10000,
  best cost ≤ 0, or page overfull (`total − \vsize > shrink`, fires at best).
  `\vsize` is read LIVE per evaluation → mid-page `\vsize` changes honored.
- **\topmark/\botmark**: on fire_up, top ← bot; first/bot/splitfirst/splitbot
  reset; first set when bot empty (tex.web).
- **fire_up**: drains the page prefix, splits insert nodes out of the page
  material, places each class into `\box N` (see inserts), packs the rest
  into `\box255` (WITHOUT inserts — inserts must never be in the shipped
  box), resets page accounting + `\insertpenalties`, then pushes
  `\output` toks + OUT_END_TOKEN. Dead cycles: `dead_cycles` incremented per
  fire_up; if `> \maxdeadcycles` → error "Output loop---N consecutive dead
  cycles" and force-shipout of \box255. `ship_box` resets `dead_cycles=0`.
- **Inserts**: class n scaling = height·`\count n`/1000 (count 1000 exact,
  count ≤ 0 contributes no page height and is never split); running
  per-class use vs `\dimen n` budget; budget exceeded → `\insertpenalties`
  += insert node's `cost` field (STAND-IN for `\floatingpenalty` — no
  IntParam exists yet). At fire_up: leftover in `\box N` + this page's
  insert boxes are combined; if scaled height > `\dimen n` the list is
  split tex.web-vsplit-style (`split_vlist`): break before a box / at
  glue-kern after a box / at `\penalty-10000`, break glue discarded,
  `\splittopskip` pad above the first kept box, `\splitmaxdepth` excess
  depth → height; splitfirst/splitbot marks taken from the kept part;
  remainder carried over as a fresh `Node::Ins` at the FRONT of page_list
  (page_processed adjusted), so it lands on the next page.

## Integration contracts / needs

1. **BoxesPack do_insert** (agreed): emit `Node::Ins{num,height,depth,cost,
   box_node}` with height/depth = the packed vbox's natural h/d, cost ≥ 0 =
   "floating penalty" for `\insertpenalties` (0 acceptable).
2. **Main/prim.rs**: add `IntParam::OutputPenalty` (`\outputpenalty`) and
   `IntParam::FloatingPenalty` (`\floatingpenalty`) when convenient. Until
   then: fire_up does NOT export the break penalty (it does NOT clobber
   `\count0` — that is the page number; the old code wrongly wrote the
   penalty there), and plain.tex's `\floatingpenalty20000` in \vfootnote
   has no effect.
3. **build.rs bug (BoxesPack's)**: `do_shipout` hbox/vbox path sets
   `setbox_target=255` + `shipout_pending`; in `end_box` the setbox_target
   branch assigns box255 and RETURNS, so `shipout_pending` never fires and
   the box is never shipped. Driver avoids it via
   `\output={\setbox255=\vbox{...}\shipout\box255}`; the `\shipout\vbox{...}`
   form is broken until fixed.
4. **\topinsert/\midinsert/\pageinsert** (plain.tex l.1176-1192) are pure
   macros over `\insert\topins` + penalties — nothing page.rs-specific is
   needed beyond this machinery; they need the format layer (Main's boot)
   plus `\newinsert` register setup. Note `\pageinsert` material is supposed
   to hold the whole page (cost via `\floatingpenalty`); fine as-is.
5. `\skip N` per insert class is NOT applied by the engine (tex.web leaves
   it to the output routine — plain does `\vskip\skip\footins`; the driver's
   output routine does the same).

## Testing

- Driver: /tmp/pagedrive.tex (3 pages, vsize=120pt, three 30pt boxes per
  page, `\insert254` 4×20pt lines with `\dimen254=45pt` → split; output
  routine glues \box254 under \box255 and ships).
- Dead-cycle check: `\output={}` + `\maxdeadcycles=2` must print the
  "Output loop" error and force-shipout.
- Known sibling-midflight state: linebreak.rs/boxes.rs errors during my
  session were NOT mine; page.rs itself compiles clean (cargo check -p).

## ACCEPTANCE RESULT (final session state)

/tmp/pagedrive.tex (v3): PASS — exit 0, 3-page PDF (1146 bytes), [1] ship
marks x3 (tracingpages). `\showbox254` after page-2's fire:
`\vbox height 40.0pt [ \vbox 20.0pt, \vbox 20.0pt ]` = exactly the kept
part of the 4x20pt insert under `\dimen254=45pt` (split 40/40, remainder
carried). Fire condition for page 1 = overfull (insert pushes total past
\vsize). i1.tex additionally proves fire+split+ship with plain
`\shipout\box255`.

Notes:
- `\showbox254` right after page 1's fire correctly shows VOID: the insert
  was contributed after the chosen breakpoint, so it belongs to page 2
  (tex.web: inserts are placed at fire_up only when drained with the page).
- tracingpages marks print `\count0+1` and count0 is never incremented
  (tex.web leaves that to the format), so all marks read [1]; cosmetic.
- Fire gating: build_page fires ONLY when a fire condition is met
  (eject / cost<=0 / overfull-at-best); list exhaustion keeps the best
  break remembered (BreakSpot::carried across calls).
- vpack_add_md (BoxesPack) used for box255 (page \maxdepth clamp) and the
  split part (\splitmaxdepth clamp).
- Driver-construct caveats (scanner, Main/BootDump domain): \message,
  \vskip\skipN, \setbox inside \output-toks playback fail silently or
  "Missing number" at digits; driver uses file-level equivalents instead.
  `\showthe\output` also prints nothing. Tree state 14:30 Aug 30.
- pagedead.tex (dead-cycle forced shipout) runs exit 0; the incremental
  fire_up -> dead_cycles -> maxdeadcycles -> ship_box-reset path is
  exercised by the passing normal flow (ship_box resets each page, fires
  repeat 3x).

## POST-YIELD FIX (BootDump-3 report)

Panic `range end index 4 out of range for slice of length 1` at
`page_list[..st.processed]` during latex.ltx boot: page_processed is not
saved/restored with page_list when build.rs swaps lists (par_page_lists,
groups), so the restored index can outrun the restored list. Fix:
clamp `st.processed = page_processed.min(page_list.len())` at build_page
entry, and clamp `cut = cut.min(page_list.len())` in fire_up before the
drain (stale carried breakpoint after a swap). Semantics note for others:
if you swap page_list behind the builder's back, either reset
page_processed/page_total/page_goal_set/page_best_break too, or rely on
these clamps (they make it safe, but accounting restarts at the clamp).
Re-verified after clamp: pagedrive.tex still 3-page PDF exit 0.

## Nest depth separation (2026-09-12)

- `build_page` updates page depth without overwriting the enclosing nest's
  `prev_depth`. Unboxing preserves nest depth, as TeX's link splice does.
- This prevents reprocessed float markers from erasing a restored `\prevdepth`
  and changing the following paragraph's interline glue.
- Document `2609.04356`: exact 150dpi parity improved from 82.746203% to
  99.695966%; all eight pages exceed 98.675%.
- Full campaign: `output/campaign-depth-separated-104/report.json`.
  All 104 Rust runs converged cleanly; mean parity 95.956799%, 73 documents
  at least 95%. The 97% mean / 95% document-floor goal remains unmet.

## Output-register lifetime and deferred write sentinels (2026-09-12)

- Output routines read the completed page's goal, total, and glue registers;
  the reset next-page counters are not their visible values. Page depth is
  cleared before output, matching TeX's "Start a new current page".
- REVTeX previously read pagegoal=16383.99998pt instead of 672pt inside
  its float-fit calculation. Its negative computed page height admitted
  floats that could not fit. All four traced float decisions now match pdfTeX.
- All non-immediate writes retain structural whatsits, including log-only
  `\write-1{}`. LaTeX uses that node to permit a split before clearpage's
  top glue; emitting it eagerly caused blank pages with correct splitting.
- Added `\ignoreprimitiveerror`: bit 1 retains pdfTeX's log-only diagnostic
  for infinite shrink during splitting. Other masks still report an error.
  LaTeX's split-mark extraction uses this facility. Format version is now 8
  because the serialized integer parameter table has an additional entry.
- Integrated campaign: `output/campaign-pagination-integrated-104/report.json`.
  All 104 runs clean/converged; 97 page-count matches, mean 96.220185%,
  75 documents at least 95%, minimum 89.343704%. Goal remains unmet.
- Differential regressions cover output-register reads and the deferred
  log-write breakpoint. Rendered REVTeX equation/paragraph placement checked.

## Output paragraph-counter isolation (2026-09-12)

- The output nest starts with `\prevgraf=0` and restores the outer counter
  when it closes, alongside `\prevdepth`. Output paragraphs cannot consume
  line numbers belonging to a display-interrupted outer paragraph.
- `probe_prevgraf_follows_enclosing_vertical_nest` verifies this against pdfTeX.

## Integrated accent and paragraph campaign (2026-09-12)

- Report: `output/campaign-accent-nest-integrated-104/report.json`.
- All 104 Rust runs compile and converge cleanly. Mean exact parity is
  96.525073% (previously 96.220185%); 80 documents reach 95%, and 99 page
  counts match. Minimum document parity is 89.863962%; the goal remains unmet.
- ACL `2609.03218` improves from 89.343704% to 95.009611%; its accented
  caption and following line positions now match the rendered reference.
- Wrapped-paragraph document `2609.05041` improves from 89.573137% to
  95.228646%; it still has 70 pages versus the reference's 69.
- NeurIPS `2609.05403` improves to 92.040568%; residual vertical offsets and
  its 46-versus-47 page count remain. Rendered page 4 inspected.
- Final verification: 123 core tests pass (5 ignored), 27 differential
  probes pass, and the production CLI rebuild succeeds.

## Breakpoint penalty lifetime (2026-09-12)

- `fire_up` leaves the selected penalty on the contribution list, rewritten
  to 10000, rather than deleting it. Output-routine material precedes this
  sentinel; `\lastskip` and `\lastpenalty` therefore observe the same tail
  as pdfTeX. Glue breakpoints expose `\outputpenalty=10000`.
- Deleting the sentinel made LaTeX's `\addpenalty` compensate the wrong
  trailing skip. A NeurIPS in-text figure gained an extra 5.5pt before the
  next heading; the reduced figure/heading example now matches exactly.
- `probe_output_reinsertion_preserves_break_penalty` fails before the fix
  (last penalty 0, last skip 5pt) and passes afterward (10000, 0pt).
- Full report: `output/campaign-breakpoint-lifetime-104/report.json`.
  All 104 runs clean; mean 96.700987%, 82 documents at least 95%,
  100 page-count matches, minimum 90.368670%.
- Predator-prey document `2609.04834` improves from 89.863962% to
  96.748053% with matching 51-page output. Its isolated alignment display
  already matches pdfTeX; no speculative alignment-spacing change was applied.

## Paragraph depth after migrated insertions (2026-09-12)

- Final-line depth scans in `end_paragraph` skip migrated insertions, marks,
  writes, and penalties instead of stopping at them and retaining the
  pre-paragraph depth. This applies in outer and internal vertical mode.
- `probe_trailing_insert_preserves_paragraph_depth` reproduces the error:
  a line with depth 3pt followed by an insertion incorrectly left the old
  7pt depth in both modes. Both now report 3pt.
- In the reduced trust-document theorem, `\prevdepth` after the footnoted
  paragraph now matches pdfTeX's 2.5pt rather than zero. Subsequent prose,
  the displayed equation, and resumed prose match reference baselines.
