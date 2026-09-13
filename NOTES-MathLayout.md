# NOTES-MathLayout.md — math list -> hlist (TeX82 App. G) session notes

Agent: MathLayout-4. File owned: `crates/tex-core/src/math.rs` (full rewrite).
Status: complete + compiles clean; driver smoke-tested (see bottom for run status).

## What was implemented (all in math.rs)

- **Style ladder** `GStyle = u8`, tex.web encoding: 0=D 1=D' 2=T 3=T' 4=S 5=S'
  6=SS 7=SS'. Public `math_to_hlist(&[Node], MathStyle)` kept as entry;
  internal `mlist_to_hlist(&[Node], GStyle)`. Derived styles per tex.web §738:
  numerator = `g|1` (cramped same level), denominator = `(g&!1)+2`,
  sup = script at same parity, sub = cramped script at next level,
  `\left...\right` after-style = D->T else cramped (§817).
- **Spacing** tex.web §761 magic table as `SPACING[8][8]` (0/1/2/3, 4=forbidden).
  Bin demotion (§760): bin -> ord when left ∈ {bin,op,rel,open,punct,start} or
  right ∈ {rel,close,punct,end}. Mu unit = quad(fam2,text)/18; thin=3mu,
  med=4mu, thick=5mu emitted as Kern. \binoppenalty after bin, \relpenalty
  after rel (before the inter-atom space), per §766.
- **make_scripts** (§745-746): sup shifts with sup1/2/3 (cramped/script
  selection), sub with sub1/2, sup/sub_drop = 0 in D/T else x_height; the
  `bh + 4x/5` floor for both; clearance rule `4*rule_thickness` between stacked
  scripts (corner mathkerns omitted — split-half approximation). Italic
  correction `delta` of single-char nucleus: full kern before sup (or sub when
  alone), half when both. `\scriptspace` kern before the script boxes.
- **make_op limits** (`Node::OpLimits`): big ops (MathChar class Op, or
  DelimBox) tuck scripts at `op.h - x/2` / `op.d + x/2`; non-big ops use the
  script rules. above/below centered in hboxes of the max width (fil glue),
  stacked in a vbox whose baseline is the op baseline. `\limits`/`\nolimits`/
  `\displaylimits` read from `Engine::math_limits` (set by maincontrol) at
  script-attach time; default = class Op and style < S.
- **make_fraction** (§741-742): num/den converted at §738 styles, centered to
  common width, vbox = [num, kern, rule(width=w, height=r), kern, den] with
  the rule centered on the axis via box `shift` (`k2 + r/2 - axis_height`).
  `thickness < 0` = default rule (fontdimen 8 of fam 3 = cmex, tex.web
  `default_rule_thickness`), `0` = \atop (no rule; gaps equalized around the
  axis), `> 0` explicit (\above). `withdelims` pairs sized to the finished
  fraction via var_delimiter. num1/2 + denom1/2 chosen by `style < 4`.
- **make_radical** (§752-753): body in a vbox [rule, kern(clearance), body]
  (clearance = x/4 if body low else rt/3); surd = var_delimiter over the full
  27-bit `\radical` code, `needed = body.h + clearance + rt`, shifted down to
  `-(bh+clearance)`; body pulled left under the surd hook by
  `kern(-(surd.w - surd.w/8))`; whole thing hpacked into one Ord box.
  The 27-bit delimiter code is packed into `Radical.thickness` with sentinel
  `thickness = -1 - code` (see in-file comment; creation and consumption both
  in math.rs — Frac.thickness semantics unchanged: <0 default, 0 atop, >0
  explicit).
- **var_delimiter + make_extensible** (§716-723): candidate order = small char,
  large char, TAG_LIST "next larger" chain; accept first with
  `h+d >= needed + \delimitershortfall` or `1000*(h+d) >= needed*\delimiterfactor`
  (factor falls back to 901 if unset); TAG_EXT -> extensible: top/mid/bot/rep
  from `Font::ext`, rep count = ceil over needed, mid splits reps half/half,
  stacked in a vbox patched to `h = axis + (needed-total)/2` with matching
  shift (tex.web's axis centering). Null delimiter = `\nulldelimiterspace` kern.
  Fallback = tallest candidate seen.
- **make_left_right**: `\right` splices the raw inner list into the enclosing
  mlist between `DelimBox{size:0}` (open) and `DelimBox{size:1}` (close)
  markers; conversion buffers between markers (nested \left..\right handled by
  a frame stack; nested finalize splices into the parent buffer), measures the
  body, sizes both delimiters to `h+d` with floor = fontdimen 20 (fam 2) in
  D/T else 21. Result atom class = Inner; after-style per §817.
  `DelimBox{size:2}` = plain delimiter atom (Ord); maincontrol's `\middle`
  appends size:1 — inside my exec-loop intercept for Prim::Middle I re-issue
  it as size:2; a bare size:1 at top level degrades to a close delimiter.
- **make_accent** (§747-753 approximation): accent baseline at
  `body.h - x_height(accent font)` (x-height line on body top), horizontal
  center + skew kern = kern(body char -> \skewchar of body font) for
  single-char bodies (reads eqtb.skew_char override, else TFM skewchar).
- **\mathchoice** (`Node::Choice` + following `Node::ChoiceAlt` bodies):
  selects branch `style>>1` (D/T/S/SS), converts only the chosen one at the
  current style. SEE BLOCKER below: maincontrol's dispatch cannot feed these
  nodes yet.
- **scan_delim_int** now implements tex.web scan_delimiter: char token with
  delcode >= 0 -> delcode (fixes `\left(` which previously scan_int'ed the
  char and got 0); `\delimiter` cs -> scan 27-bit; otherwise back_input +
  scan_int with range check.
- **scan_math_group_or_token** rewritten as an EXECUTING scan: `{...}` groups
  are run as nested math lists (full generality: \frac/\sqrt/\left inside
  script and fraction arguments), nested literal braces become boxed Ord
  atoms; single CS token dispatches into a temp list. Used by scripts,
  radicals, accents, fractions (denominator). Intercepts Prim::Delimiter and
  Prim::Middle before main_dispatch (maincontrol loses `\delimiter` atoms in
  mlists — stores them in `last_delim` only).
- **exit_math**: drops the old crude DelimBox wrapping (\left/\right now flow
  through pop_math_group_delimited markers); converts at D (display) or T
  (inline); clears left_delim/right_delim/math_limits leak guards.
- **Removed dead code**: MathState struct, sign_shift, shift_up, style_up/
  style_down duplicates, center_to, old script_shift (sup1 was read from
  param(9) = num2 — wrong slot; now param 13), old spacing_between.

## Font parameter conventions used
- Math params are fontdimens of fam 2 (symbol) / fam 3 (extension) at the
  style's size: num1=p8, num2=p9, denom1=p11, denom2=p12, sup1=p13, sup2=p14,
  sup3=p15, sub1=p16, sub2=p17, delim1=p20, delim2=p21, axis=p22 (fam 2);
  default_rule_thickness = p8 of fam 3 (tex.web). Read through
  `eqtb.font_params` (live \fontdimen values, copied at load) with TFM
  fallback and safe defaults if fam 2/3 is unassigned.

## Known gaps / approximations (documented, all non-panicking)
1. `\mathchoice` conversion logic is in and correct, but maincontrol's
   MathChoice arm pushes 5 `push_math_group()`s that are closed by `}` through
   end_box (build.rs) — those never append ChoiceAlt and desync box_kinds /
   saved_lists / math_lists. NEEDS MAIN: route MathChoice to a math.rs
   function (e.g. `begin_choice_groups()` that also balances box_kinds) and
   make end_box delegate to math.rs when a choice group is on top. Until
   then `\mathchoice` in input will misbehave.
2. Spacing across a `\left...\right` boundary uses class Inner for the whole
   box (TeX uses Open/Close at the edges; edge spacing across the boundary is
   not computed in my two-level buffering).
3. `\middle` at top level (outside my exec scan) is sized/classed as a close
   delimiter rather than matched to the \left..\right height.
4. Unbraced multi-token denominators (`$x\over ab$`) take one token — engine
   architecture scans eagerly; `\frac{..}{..}` is unaffected.
5. make_scripts skips the \mathkern corner-kern refinement (split-half
   clearance instead); make_accent uses the center+skew approximation above.
6. `\displaystyle`/`\textstyle` etc. have no primitives registered
   (prim.rs/engine.rs are not mine). `Node::Style` mid-list IS handled by
   mlist_to_hlist, so once Main registers them (-> append `Node::Style`) they
   work immediately. Main's `math_style_stack` is still only read.
7. Bin demotion for pass-through nodes uses nearest-atom classification; TeX's
   exact interplay with intervening glue/whatsits may differ in exotic inputs.

## Driver (acceptance smoke test)
- `/tmp/mathdrv/mathdrv.tex` + binary in `/tmp/mathdrv/target/debug/pdflatex`
  (isolated throwaway build in /tmp/mathcheck + /tmp/mathdrv so the
  concurrently-edited workspace is never built; the workspace copy's stub
  harness hangs on trivial input — use the workspace build for real runs).
- Sets up cmr/cmmi/cmsy/cmex as fams 0-3, delcodes/mathcodes, defines
  \sum/\sqrt/\frac via \mathchar/\radical/\over, then \setbox+\showbox of:
  x^2, a\over b, \sqrt{x}, \sum\limits_{i=1}^n, \left(\frac{a}{b}\right),
  then ships them to PDF.
- Run status: see session log below.

## Session result (final)

`cargo check -p tex-core` PASSES (workspace-wide, zero errors). Driver
`/tmp/mathdrv/mathdrv.tex` (run: `pdflatex -plain -output-directory /tmp/mathdrv mathdrv.tex`,
binary in `/tmp/mathdrv/target/debug/pdflatex` built from a private snapshot
so the concurrent workspace was never built) prints for the 5 acceptance
constructs:

1. `x^2` -> hbox 12.089pt high: x-box + \scriptspace kern + sup box shifted up.
2. `a\over b` -> fraction vbox 20.05pt: a / kern / rule(0.04pt) / kern / b,
   axis-centered (shift -2.01pt), num/den centered by fil glue.
3. `\sqrt{x}` -> surd box (cmex, axis-centered) + kern + vbox[rule, kern, x].
4. `\sum\limits_{i=1}^n` -> limits vbox 20.09pt: n / kern / axis-centered Sigma
   / kern / i=1, each centered to common width; \binoppenalty slot present.
5. `\left(\frac{a}{b}\right)` -> delimiters axis-centered (right parenbig
   shift -8.01pt) around the fraction; one Inner atom. NO panics, NO errors.

Verified against the real `tex` oracle (\showbox of the same constructs):
- big-op/delimiter axis shift formula confirmed: shift = (h-d)/2 - axis_height
  (oracle -7.50006 for \sum, -8.10007 for parenleftbig; ours -7.5 / -8.01).
- both-scripts stack in a vbox with kern = (su-d_sup)+(sd-h_sub), shift = sd
  (oracle: vbox 11.0418+0.0 shifted 3.00005; ours matches structurally).

## For MAIN — three cross-file items (I did not touch these files)
1. **RESOLVED — no tfm.rs bug**: the "metric drift" (cmmi fontdimen5 4.03 vs
   4.31, cmr paren 7.05 vs 7.5, axis 2.05 vs 2.5) was entirely the
   scaled_to_string display bug (item 2). After that fix, engine values match
   tftopl exactly; tfm.rs was left untouched.
2. **`{...}` groups inside math mode corrupt box state** (same family as
   BOOT-DEBUG #49): control.rs `begin_group` + build.rs `end_box` treat a math
   braced group as an hpack box group: end_box hpacks an EMPTY cur_list,
   appends a junk Box into the math list, and can consume
   `setbox_target`/`shipout_pending` meant for an enclosing box. Repro:
   `\setbox4=\hbox{$\left({a\over b}\right)$}\showbox4` -> void/empty box.
   Fix: route math-mode `{`/`}` to math.rs (push/pop a math list) instead of
   the hpack machinery. BoxesPack-4 (build.rs) notified with the repro.
   My driver sidesteps it with `\def\frac#1#2{#1\over#2}` (brace-free
   expansion, TeX-equivalent for single-token args).
3. **`\mathchoice` wiring — FIXED in follow-up batch** (see below).
   Minor engine quirks also fixed or noted: `\textfont<fam>=` scan order
   FIXED; `\font` at-scan FIXED (fontload.rs); MathCharDef dispatch: group
   content handled via math.rs intercept, top-level control.rs hole remains
   for BootDump-3/Main; io.rs `scaled_to_string` is actually scan.rs — FIXED.

## Follow-up batch (same session, per Main's grant)

Ownership granted: tfm.rs, io.rs (scaled_to_string only), maincontrol.rs
(\mathchoice, \textfont order, \font at-scan, MathCharDef). NOT touched:
control.rs/scan.rs/expand.rs/engine.rs/scanner.rs (BootDump-3), build.rs
(BoxesPack-4).

1. **tfm.rs: NO BUG.** The entire "metric drift" was scan.rs
   `scaled_to_string` mis-displaying. After the formatter fix, engine values
   match tftopl/real-tex exactly: cmr10 paren 7.5/2.5pt (tftopl CHARHT 0.75 /
   CHARDP 0.25), cmmi x-height 4.30556pt, Sigma shift -7.50006 == oracle.
   tfm.rs left untouched.
2. **scaled_to_string** — actually lives in **scan.rs:903** (not io.rs; no
   io.rs change was needed). Rewrote: digits now extracted by successive
   x10 long division (the old code divided before multiplying, dropping the
   first decimal digit: 7.5pt printed as 7.05pt, 2.7778pt as 0.0pt) plus
   round-half-up on the 5th digit with carry into the integer part, trailing
   zeros trimmed (min one decimal — TeX's "1.0pt" form). BootDump-3 notified
   via hub; the change is confined to one function.
3. **maincontrol.rs**:
   a. MathChoice arm now calls `math.rs::begin_mathchoice()` which scans the
      four style groups immediately (tex.web semantics) and appends
      Choice+ChoiceAlt nodes — no group-stack games, works with build.rs
      untouched. Verified: inline \\mathchoice picks the text branch.
   b. \\textfont/\\scriptfont/\\scriptscriptfont now scan
      `<fam number> <optional => <font id>` (tex.web any_math_fonts) —
      `\\textfont0=\\tenrm` works; verified in driver.
   c. \\font at-scan: do_font (fontload.rs — granted via this item) now uses
      a tex.web-style name scanner on the non-space-skipping fetch
      (get_x_raw) so "cmr10 at 10pt" splits correctly, and at/scaled are
      matched with scan_keyword. Verified: "cmr10 at 10.0pt" loads.
   d. MathCharDef dispatch: the top-level dispatch hole is in control.rs
      (BootDump-3's). Interim: math.rs run_math_token materializes
      Equiv::MathCharDef inside math groups (verified:
      \\mathchardef\\sumG + use inside \\sqrt{...} works). The control.rs
      hole for top-level math content remains for BootDump-3/Main: in the
      dispatch fallback, `Equiv::MathCharDef(v) if mode.is_m() =>
      append_mathchar(v)`.
4. Extra verification after the fixes: driver run6 — exit 0, 0 errors,
   0 void boxes; thick spaces now 2.77779pt, x^2 sup shift -9.88889pt,
   \\sum axis shift -7.50006pt (== oracle).

## Follow-up addendum: braced-group repro RESOLVED end-to-end

BoxesPack-4 landed the build.rs end_box fix (plain groups in math mode no
longer package junk boxes or consume setbox targets). Re-verified with the
original repro using the braces-heavy `\def\frac#1#2{{#1\over#2}}`:
`\setbox4=\hbox{$\left(\frac{a}{b}\right)$}` now produces
[cmr paren 7.5+2.5 (zero shift), fraction vbox (a / 0.4pt rule / b,
axis-centered), cmex parenrightbig shifted -8.10007 (== real-tex oracle)].
The math.rs driver no longer needs the brace-free \frac workaround.

Follow-up batch file touches: scan.rs (scaled_to_string only), fontload.rs
(do_font name/keyword scan), maincontrol.rs (MathChoice + TextFont arms),
math.rs (begin_mathchoice + MathCharDef intercept in run_math_token).
tfm.rs untouched (no bug). cargo check -p tex-core: 0 errors.

## Style and display-spacing corrections (2026-09-12)

- Operator limit placement is resolved in the conversion style, including
  fraction denominators, trailing directives, and directives between scripts.
- Delimiter lookup uses direct text/script/scriptscript font-table indices.
- Display interline glue retains baseline/lineskip stretch and shrink.
  A stretched 80pt vbox now places its equation at the same vertical coordinate
  as pdfTeX; previously the equation was 24.44bp too high.
- Verification: 42 math-layout tests pass; a numbered REVTeX equation retains
  both formula and tag in the rendered PDF.

## Scripts on accented characters (2026-09-12)

- Scripts on a single-character accent attach to the character before the
  accent is positioned. They no longer clear the taller accent glyph.
- `\hat\beta_p^{\rm PR}` now measures 9.58334pt / 3.83327pt / 17.86461pt
  (height / depth / width), matching pdfTeX; height was 11.89449pt.
- CLI comparisons match pdfTeX for bare and braced characters, grouped accents,
  compound nuclei, already-scripted nuclei, and `\tilde g_r^t`.
- `accent_scripts_swap` protects the character/script placement regression.
