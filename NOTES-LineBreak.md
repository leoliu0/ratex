# NOTES-LineBreak.md — hyphen.rs + linebreak.rs rewrite (LineBreak-5)

## Status
- `cargo check -p tex-core`: 0 errors (verified repeatedly during integration).
- Acceptance driver `/tmp/lbdrv` (path-dep on a /tmp snapshot of tex-core; snapshot's
  math.rs was mechanically patched there only — workspace untouched):
  40 line boxes for a 299-word paragraph at hsize 200pt (cmr10 @ 10pt),
  max overfull 0.000pt after full shrink, `ACCEPTANCE OK`.
- Hyphenation verified against REAL `tex \showhyphens{...}` (TeX Live installed):
  considerable → con-sid-er-able [3,6,8]; hyphenation → hy-phen-ation [2,6];
  algorithm → al-go-rithm [2,4]; table → ta-ble [2] (exception). Unit test
  `hyphen::tests::considerable` encodes these and passes.

## hyphen.rs
- `Trie { trans, values, exceptions }` — engine.rs field type unchanged
  (`pub hyphen_trie: crate::hyphen::Trie`), so no engine.rs edit needed.
- Patterns: key chars = letters + '.' boundary marks; digit values land in
  `values[(pos, v)]` slots (max-merged on insert). Lookup wraps word as
  `.word.`; odd vals = points.
- **Point convention: k = number of letters BEFORE the break.** Valid range
  k ∈ [lefthyphenmin, n−righthyphenmin] (tex.web: `k:=l_hyf to hn-r_hyf`).
  Gotcha found: it is `k >= left`, NOT `k > left` (hy|phenation is legal).
- `\hyphenation{}` exceptions take priority over patterns; same min filtering.
- Loaders: `Trie::load_hyphen_file(path)` / `load_hyphen_str(&str)` parse
  `\patterns{...}` + `\hyphenation{...}` blocks with %-comments; return
  (n_patterns, n_exceptions).
- `Engine::load_hyphenation_file(path)` lives in hyphen.rs — **Main: nobody
  calls it yet. Hook it where the format/plain.tex executes `\patterns`
  (iniTeX boot); without it every paragraph is set unhyphenated.**
- engine.rs `hyphen_exceptions: Vec<(String, Vec<u8>)>` is now redundant
  (Trie owns the exception map) — Main may delete it.

## linebreak.rs — tex.web port
- `break_paragraph(hlist) -> Node` SIGNATURE UNCHANGED (build.rs caller OK).
  Input contract: end_paragraph appends `\penalty10000` + `\parfillskip`
  before calling; break_paragraph prepends structural `\leftskip` node at
  list[0] and adds NOTHING at the tail (tex has no trailing rightskip node or
  final penalty — the last break is virtual at pos = len). Fixed the old
  double-parfillskip + double-indent bugs.
- Feasible breakpoints: penalty < inf; glue whose prev is not
  glue/penalty/explicit-kern; explicit kern followed by glue (tex's
  kern_break — the kern is zeroed at line end); discs (gated by pass).
- Active nodes: tex fields (pos, btype, line, fitness, demerits, start_w/st/sh,
  prev). Champions are min-demerits per (line, fitness) class per candidate.
- Deactivation: b > inf_bad (can't shrink) or forced break; record-then-
  deactivate order preserved (tex §838). Artificial-demerits rescue: fires on
  final_pass when the ONLY active would vanish with no champion yet — note it
  applies to HOPELESS actives too, not only forced breaks (without this the
  active list can drain mid-paragraph and the pass loop spins forever).
- Pass sequence: pretolerance (no pattern discs) → tolerance (discs) →
  emergency stretch (background dstretch[0] += emergencystretch, final_pass)
  → guaranteed by rescue; plus a `final_ran` guard with single-overfull-line
  fallback if a bug ever makes the final pass fail.
- Demerits (tex.web @<Compute the demerits@>): (lp+b)² cap 1e8, ±p² terms,
  double_hyphen_demerits for consecutive disc breaks, final_hyphen_demerits
  when penultimate break is a disc (cur_p = null, break_type hyphenated),
  adj_demerits when |fitness Δ| > 1. Fitness classes very_loose..tight = 0..3
  with tex thresholds (12/99).
- Width model: cumulative arrays with disc no_break fill + replace_count dead
  nodes; `endsum` (line end) vs `startsum` (next-line start after prune of
  glue/penalty/explicit-kern) per breakpoint; \leftskip+\rightskip widths and
  stretch/shrink subtracted as tex's "background".
- Line building (post_line_break equivalent): disc → pre_break appended to
  line end, post_break prepended to next, dead nodes skipped, prune next-line
  start iff post_break empty; glue/penalty/explicit-kern breaks drop the break
  node and prune; implicit kerns are NOT pruned at line start (tex-exact).
  Parshape indent inserted as a leading Kern (approximation; tex uses glue).
- Interline glue: still zero-glue placeholders between line boxes — page
  builder owns baseline spacing (kept old contract; build.rs:968 comment).
- Overfull: reported via `Engine::error` on final_pass when nat − target >
  hfuzz; `\overfullrule` appended. Underfull reporting not implemented.
- REMOVED: dead `REMAINDER` thread_local + `disc_pre_empty` (no external
  callers; vsplit_box uses engine field `vsplat_remainder` as before).

## Integration hooks / open items for Main
1. Call `Engine::load_hyphenation_file("/usr/share/texmf-dist/tex/generic/
   hyphen/hyphen.tex")` (or wire `\patterns`/`\hyphenation` primitives to
   `hyphen_trie.add_pattern/add_exception`) at format boot.
2. `looseness` / `\parshape` looseness optimization not implemented (param
   parsed but unused) — tex's best_line adjustment skipped.
3. Discs with non-empty no_break + replace_count (ligature-splitting
   hyphenation) are width-modeled correctly but hyphenate_list currently
   inserts only simple discs (replace_count 0); builder-produced ligature
   discs would need the same treatment in hyphenate_list.
4. Underfull box warnings + tracingparagraphs output not implemented.

## Post-acceptance fix (PageShip-4 report)
- start_state's disc branch indexed `cum_w[cand+1+replace_count]` unclamped;
  a disc near the paragraph end (dead zone past list end) panicked with
  "index out of bounds: len n+1, index n+1". Clamped to n (degenerate-para
  repro: rule + \penalty10000 + \parfillskip → vbox, no panic; driver still
  ACCEPTANCE OK). `after_prune` was already clamped via f.min(n).

## Discretionary replacement correction (2026-09-12)

- Explicit `\discretionary` replacement text is owned by `DiscNode::no_break`;
  its `replace_count` is zero because no following source nodes duplicate it.
  A nonzero count incorrectly discarded following text during line construction.
- Expansion measurement includes characters inside unbroken discretionary text.
- `probe_explicit_discretionary_preserves_following_text` compares natural and
  expanded line widths with system pdfTeX.

## Vertical split correction (2026-09-12)

- `vsplit_box` now follows TeX's candidate loop: includes rule dimensions,
  evaluates height without trailing depth, and stops at a forced/overfull
  candidate without subsequently choosing an artificial end-of-list break.
- Kern-before-glue and non-discardable predecessors are legal breakpoints.
  Zero-height targets use the same algorithm, including negative kerns.
- Remainder pruning preserves marks and inserts adjusted `\splittopskip`.
  Owned node vectors are split instead of cloning both halves.
- Differential probes cover forced breaks, rules, trailing depth, zero
  targets, and remainder marks/topskip; verified against system pdfTeX.

## Display continuation and semantic nest counters (2026-09-12)

- Paragraph line numbering continues from the enclosing vertical nest's
  `\prevgraf`; each completed display adds three before text resumes.
  `\parshape` and hanging-indent lookup use these absolute line numbers.
  Club and widow penalties remain relative to the current fragment.
- Existing Engine-owned nest frames now save the paragraph counter.
  Reads and assignments locate the enclosing vertical record, including
  through horizontal boxes and math. Boxes, insertions, alignments, and
  output routines preserve the enclosing counter without a thread-local stack.
- Differential regressions cover successive displays with a changing shape,
  nested boxes/alignments, horizontal assignments, and output-routine isolation.
  The CLI counter scenario matches pdfTeX on all ten observations.
- Remaining diagnostic-context discrepancy: direct `\write` expansion exposes
  the enclosing counter instead of pdfTeX's mode-zero value. Counter probes
  capture the value with `\edef` before writing, or inspect it with `\message`.
