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
