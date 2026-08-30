# NOTES-BoxesPack.md — box packing, leaders, box registers (boxes.rs + build.rs)

Owner: BoxesPack-4. Files touched: `crates/tex-core/src/boxes.rs`, `crates/tex-core/src/build.rs` (exclusively).

## Verified against tex.web (fetched CTAN tex.web to /tmp/tex.web) and real `tex` binary

### boxes.rs — packing core (tex.web §649-668, §2337)
- `tex_badness(t,s)`: exact tex.web algorithm (r=297t/s via the two overflow branches,
  badness=(r³+2¹⁸)/2¹⁸, cap INF_BAD; badness(2t,t)=800, badness(3t,t)=2699 — note the
  approximation's off-by-one vs 100(t/s)³). scaled::badness (exact cube) left untouched
  for linebreak; packers now use tex_badness for byte-compat.
- `compute_glue_set`: filll→fill→fil→normal order pick (highest nonzero first);
  sign reverts to normal + ratio 0 when that order's total is 0.
- `finish_glue` (shared tail): overfull → last_badness=10⁶ and ratio clamped to exactly
  1.0 (sign stays normal when there was no shrink at all — matches real tex showbox);
  underfull/loose/tight badness computed only at normal order and only for non-empty lists.
- Overfull `\hbox` appends an overfull-rule node (width=\overfullrule) iff excess>\hfuzz;
  vbox overfull adds no rule (per tex.web).
- `hpack(list,w,kind,eqtb)` / `vpack(list,h,kind,eqtb)` signatures UNCHANGED (page.rs,
  linebreak.rs, math.rs, align.rs consumers). New: `hpack_add` (m=additional, i.e. \spread),
  `vpack_add_md` / `vpack_md` (max-depth param: box_max_depth for \vbox/\vtop via end_box,
  split_max_depth for \vsplit, page_max_depth for the page builder), `vtop_md`.
- vpack depth clamp (d>l → height+=d−l, depth=l) happens BEFORE glue setting, per tex.web.
- vtop readjust per tex.web package(): height = RAW height of first box/rule item (no shift
  adjustment!), depth := depth−h+height, height=0 if list empty or first item isn't box/rule.
- PackResult gained fields: delta, stretch[4], shrink[4], sign, order (badness kept).
  All consumers use r.node/r.badness — source compatible.
- vlist_dims rewritten to tex.web: glue/kern add running depth and reset it; box width
  competes as width+shift; penalty/mark/ins/whatsit do nothing.
- Null rule dims: RULE_FILL (i32::MIN) sentinels stay in nodes (real tex keeps `*` in
  showbox); max-forms ignore them naturally, sum-forms never see them (hrule never in
  hlists, vrule never in vlists, hrule depth set 0) — exactly tex.web's
  "highly negative is ignored" trick. Do NOT resolve them at pack time.
- Leaders: `Node::Leaders { glue, kind, body: LeaderBody }` (LEADERS_A/C/X = 0/1/2).
  Leader glue participates in stretch/shrink sums and natural dims; body height/depth
  counts in hlists, body width in vlists.
- `leader_layout(kind, leader_wd, total_w, left_edge, cur) -> (positions, lx, cur_end)`:
  tex.web hlist_out/vlist_out §12456/§12625 math incl. the ±10 rounding compensation,
  a_leaders left_edge grid snap, c_leaders lr/2, x_leaders lx=lr/(lq+1).

### build.rs
- begin_box: to/spread scanned via scan_keyword (tex.web scan_spec — they are CHAR
  keywords, not cs), then **consumes the opening `{`** (tex.web scan_left_brace) — this
  was the group-desync root cause (double group: box_kinds leaked).
- end_box: spread handled inside packing (additional), vpack uses \boxmaxdepth, kinds
  0/1/2/8 run `report_pack_warnings` (Overfull/Underfull/Loose/Tight, tex.web wording,
  \hfuzz/\vfuzz + \hbadness/\vbadness gating, `) has occurred while \output is active`
  when in_output, `detected at line N` otherwise; log always, term only if
  \tracingonline>0). last_badness set for all packed kinds (tex.web semantics).
- **CRITICAL FIX for PageShip/Main escalation**: begin_box had LOST its
  `saved_lists.push((mode, cur_list, prev_depth, space_factor))` in an edit collision —
  outer mode was never saved → after any \vbox/\hrule-in-vmode append the engine drifted
  into horizontal mode (PageShip's H/V repro, linebreak panic, lost eject penalties).
  Restored + end_box saved-pop hardened to unwrap_or (desync-tolerant).
- do_insert: syntax fixed to `\insert N [to D] {…}` (keyword-conditional), pushes kind **8**
  (insert group); end_box kind 8 packs vbox (to-D as exactly-target) then wraps into
  `Node::Ins{num, height, depth, cost, box_node}` with cost = InsertPenalties (reset 0),
  appended via normal vertical append. **Contract agreed with PageShip-4.**
- Leaders: `begin_leaders(kind)` (wire to prims; see integration needs), scans the leader
  object (\hbox/\vbox/\vtop/\vcenter via group machinery, \box/\copy/\lastbox, \hrule/\vrule
  via new scan_rule_dims), then finish_leaders scans the mandatory glue (\hskip-family in
  h-mode, \vskip-family in v-mode, error "Leaders not followed by proper glue") and appends
  Node::Leaders. Pending leader kinds ride a thread_local stack (LEADER_KINDS) — Engine is
  single-threaded; avoids touching Main's Engine struct.
- scan_rule_dims/make_rule: tex.web scan_rule_spec defaults — \hrule: width→hsize,
  height 0.4pt, **depth 0** (was 0.4pt — bug); \vrule: width 0.4pt, height/depth null
  sentinels.
- `\hfil`/`\hfill`/`\hfilneg`/`\hss` (and v-variants) were built with **width 1pt** and
  \hss with normal orders — fixed to TeX: fil glues width 0, \hss = 0pt plus 1fil minus
  1fil (order 1). (linebreak.rs:79 builds parfillskip as Glue::fil(1, ONE) — 1pt wide;
  LineBreak owner may want fil(1, 0).)
- box_move/scan_box_after_move: \moveleft/\moveright/\raise/\lower accept the full box
  spec incl. \vcenter/\copy/\lastbox; \box/\copy after a move get shift := move amount
  (tex.web replaces, not adds); error text now tex.web's "A <box> was supposed to be here".
- un_skip/last_skip_value treat Node::Leaders like glue (tex.web: leaders are glue nodes).
- New APIs for Main to wire (see below): box_reg_dimen(idx,which)/do_box_dimen_assign(which),
  report_pack_warnings, print_scaled (tex.web print_scaled, 5 decimals + rounding).

## Integration needs (files I do not own)
1. prim.rs: add `Leaders, CLeaders, XLeaders, Wd, Ht, Dp` variants.
2. engine.rs registration: `d!(eng, b"leaders", Leaders); d!(eng, b"cleaders", CLeaders);
   d!(eng, b"xleaders", XLeaders); d!(eng, b"wd", Wd); d!(eng, b"ht", Ht); d!(eng, b"dp", Dp);`
3. control.rs/maincontrol.rs dispatch:
   `Leaders|CLeaders|XLeaders => self.begin_leaders(0|1|2)` (main position);
   `Wd|Ht|Dp => self.do_box_dimen_assign(0|1|2)` in try_assignment.
4. scan.rs value positions: `\wd<n>`/`\ht<n>`/`\dp<n>` in scan_int/scan_dimen →
   `box_reg_dimen(n, 0|1|2)`.
5. pdfrender.rs ship_hlist/ship_vlist: render Node::Leaders (currently falls into `_=>{}`):
   advance = glue_advance(glue fields, sign/order/set); rule body → rect of body h+d;
   box body → boxes::leader_layout for copies + ship each. Same for null-rule sentinels
   (RULE_FILL dims) at shipout: hrule width→box width (\vbox), vrule height/depth→box
   height/depth (\hbox).
6. TeX defaults missing (eqtb all-zero): hfuzz=6554 (0.1pt), vfuzz=6554, hbadness=1000,
   vbadness=1000, overfullrule=327680 (5pt), boxmaxdepth=262144 (4pt),
   maxdepth=262144. Without hbadness=1000 every minor underfull spams.
7. page.rs should call vpack_md(list, h, VBOX, eqtb, page_max_depth) instead of plain
   vpack (plain = no depth clamp, tex.web max_dimen). PageShip informed.
8. Inner braces inside \hbox{a{b}c} become plain-group boxes (control.rs begin_group
   treats every `{` as group+box on close) — pre-existing architecture, Main's call.

## Verification
/tmp/boxtest (driver crate, path-dep on tex-core, NOT in the crate): 17 test groups —
- hpack [char,fil,char]: natural width/height/depth, sign=stretching order=fil,
  glue_set=10.0 exact; \hbox to 100pt glue_set bit-identical to f64 delta/stretch.
- tex_badness pins (100/800/2699/10000-cap/4713 large-t branch) verified vs tex.web formula.
- Overfull hbox/vbox warnings: exact text "Overfull \hbox (50.00000pt too wide) detected
  at line N", overfull rule appended, vbox "too high" without rule, hfuzz/hbadness gating,
  tracingonline terminal suppression.
- Underfull/Loose/Tight incl. badness-10000-with-no-glue case, shrink cap ratio 1.0.
- vpack box_max_depth clamp before glue set; vbox to+fil set; vtop readjust; spread.
- Leaders glue sums; vlist_dims tex semantics (incl. width+shift); leader_layout a/c/x
  positions hand-computed; print_scaled vectors; \wd/\ht/\dp accessors; void box reads 0.
- Engine-level (vrepro bin): `\vbox to30pt{}\par\end` → mode stays Vertical, vbox on
  page_list; `+\penalty-10000` → page_list=2 (eject reaches page builder). This was the
  Main/PageShip escalation — root cause was begin_box losing its saved_lists push.
All passing; workspace `cargo check -p tex-core` 0 errors (when siblings are mid-edit
there can be transient errors in THEIR files only).

## Gaps remaining
- \wd/\ht/\dp assignments + value positions need prim wiring (see above); assignment
  ignores grouping (no save-stack entry kind exists for box dims).
- \vadjust still a stub (append_vadjust); \hbox in vmode should be adjusted_hbox_group.
- Inner-brace grouping inside box lists (architecture, Main's domain).
- TeX parameter defaults (item 6) not set anywhere — Main/eqtb decision.
- hpack reports do not print box contents (short_display/show_box) after the message.
- linebreak.rs parfillskip width-1pt (owner LineBreak-5, FYI).

## Follow-up (MathLayout escalation)
- end_box now treats plain groups (kinds 5/6) opened in math mode as pure grouping
  (tex.web math_group): state popped, math-mode cur_list restored, NO box packaged or
  appended, setbox_target/shipout_pending left intact for the enclosing box. Fixes
  `\setbox4=\hbox{$\left({a\over b}\right)$}` junk-box + swallowed-setbox repro from
  NOTES-MathLayout.md item 2. True fix for brace pairing in math belongs in control.rs
  dispatch (Main), but end_box is now safe either way.
- Driver re-run after change: ALL PACKING TESTS PASSED; cargo check -p tex-core 0 errors
  from boxes.rs/build.rs.
- CRITICAL (PageShip catch #2): end_box had ALSO lost `self.mode = outer_mode;` in an
  edit collision — mode restore now happens unconditionally right after the saved-state
  pops (covers all paths incl. math-group early return). Verified: vrepro A/B still
  mode=Vertical with material + penalties on page_list; driver ALL PASS.
