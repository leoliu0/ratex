# Boot Debugging State (post-session-12)

## Applied fixes this session (all built, boot still fails at ~line 1773+)
1. scan.rs: scan_int/scan_dimen initial fetches now get_x_raw (expanding) — IIR \@parse@version works.
2. expand.rs CsName arm: resolves \endcsname via eqtb.resolve (not cur_prim) — 106→84 endcsname errors.
3. build.rs: par-start no longer pushes eqtb group level (paragraphs aren't groups); end_paragraph no longer pops.
4. control.rs: \countdef/\dimendef/\skipdef/\muskipdef/\toksdef/\chardef/\mathchardef assign GLOBAL (tex.web).
5. Diagnostics added: SILENT-DISPATCH (control.rs dispatch `_ =>` arm), MISSNUM (scan.rs scan_int), SGET-BIG (scan.rs SGET loop), DEF/DEFALL dumps (control.rs finish_def), OFWO-CALLED (expand.rs expand_macro, unused — \@onefilewithoptions never expands during boot).

## Root phenomena (evidence in /tmp/boot.log, binary ./target/debug/pdflatex)
- Boot dies around latex.ltx line ~1770 with a runaway SGET (scan_general_text_expanded, scan.rs:594)
  that starts ~line 1670-1750 and eats to EOF (\dump swallowed → format_done=false, exit 1).
- The \newdimen chain (latex.ltx:344-404 e@alloc block) misfires: \maxdimen ends up Equiv::CharDef
  instead of DimenReg (SILENT-DISPATCH cs=\maxdimen equiv=CharDef) → its assignment text
  `=16383.99999pt` leaks to the typesetter → 80 "No font selected (char ignored)" errors
  (build.rs append_char, cur_font==0).
- 84 "Missing \endcsname" from UNDEFINED cs (\@currname, \curr@fontshape, \@gobble...) — downstream
  of the same region: macros whose defs got mangled in the 1600-1900 range.
- isolated repro: /tmp/textest/ea.tex = latex.ltx lines 341-404 verbatim + \catcode`\@=11 + minimal
  aux defs + \newdimen\foo — reproduces Missing-number cascade. ch.tex (minimal \e@ch@ck) PASSES,
  so the bug is in the e@alloc chain interaction, not the basic braced-arg grab.
- expand_macro's undelimited arg grab handles {groups} correctly (expand.rs ~line 72: cc==1 →
  scan_balanced_raw). scan_balanced_raw strips outer braces correctly.
- MISSNUM trace shows tok=cc2 `}` read from source "T:e@alloc 9/35" — scanner reading e@alloc BODY
  tokens raw → suspect \e@ch@ck was NEVER DEFINED (finish_def DEF-dump printed e@alloc but NEVER
  e@ch@ck despite the filter including it; last DEFALL run printed NOTHING — check finish_def
  actually runs / binary staleness).
- \wlog texts appear to leak to typesetter too (chars from `\string#6=\string#1\the\allocationnumber`
  show as NOFONT) — verify do_write path vs \immediate prefix handling.

## Next steps (in order)
1. In ea.tex context, dump WHICH defs registered (DEFALL run printed zero — suspicious, verify binary
   rebuilt: ./build.sh) and whether \e@ch@ck exists: add `\message{\ifdefined\e@ch@ck Y\else N\fi}`.
2. Fix the \e@alloc/\e@ch@ck def/call misfire (the region latex.ltx 344-404 is the FIRST construct
   past the \countdef block; everything after depends on it).
3. Make dispatch's silent `_ => {}` arm an error (or keep IFTRACE) to catch all such equiv-misses.
4. nullfont: append_char should silently no-op (real TeX nullfont) — removes 80 boot errors.
5. After alloc block passes, re-run boot; expect progress into IIR/fontmath; then \dump.
6. \dump → format_done → user file runs (latextest.tex: `\relax x \end`).

## Regressions to keep green
/tmp/textest: d2.tex (dimendef direct), cs5.tex (gdef+csname), cs7.tex (xdef+csname+@catcode),
g8.tex (providesfile+filelist chain), ch.tex (e@ch@ck minimal). All pass as of this session.

## Infra note
All 9 fan-out agents + 1 probe died to provider outages (grok-4.6 + gemini-3.7 both retry-exhausted).
Only TexMk's Cargo.toml bin entry landed. Everything else in the fan-out (BoxesPack, LineBreak,
PageShip, MathLayout, HAlign, PdfEmbed, BibRust) remains TODO from scratch. Re-fan-out when providers
recover; task specs are in the archived task batch (see conversation).

## Session-12 final update
7. nullfont silenced (append_char early-return): boot errors 941 (was 1090); boot now REACHES
   line 3255+ ("Extra \or" errors at 3255/3256 = an \ifcase chain misfiring there) — the runaway
   SGET still starts at file line ~1773 but no longer poisons everything downstream.
8. Current top errors: Extra \or @3255/3256 (\ifcase), Missing number @729/@18949, Missing
   \endcsname (collected="ds@", hit=\CurrentOption UNDEF) @18709 — \CurrentOption undefined when
   \@onefilewithoptions-area code runs. \e@ch@ck IS definable in isolation (chd.tex CHDEF-Y).
9. Keep SILENT-DISPATCH/MISSNUM/SGET-BIG traces until boot passes; they are env-gated (IFTRACE).
Next: (a) investigate \ifcase at 3255, (b) trace why \@currname/\CurrentOption are unset in the
9714 InputIfFileExists/\@addtofilelist path, (c) find the SGET that starts ~1670-1750 (add env-gated
print at SGET-START of cur file line + first collected token; it is the LAST SGET-START before the
first SGET-BIG at log line 4 with IFTRACE=1).

## Session-13: alloc-block isolated (ea3.tex reproducer)
- /tmp/textest/ea3.tex = latex.ltx lines 344-417 verbatim (\countdef\m@ne .. \let\float@count)
  + \catcode`\@=11 + aux defs + \newtoks\LRI + \everyjob\expandafter{...} + messages.
  THIS IS THE CORRECT SLICE ([343:417] 0-indexed; earlier repros were off-by-one/mid-body —
  slice errors caused phantom symptoms: "afl panic" session-11 and ea "File ended" variants).
- ea3 run: "File ended while scanning conditional [ea3.tex:79]" — line 79 = latex.ltx:416 =
  the trailing \fi of \e@ch@ck's def body — an \ifnum pushed INSIDE e@ch@ck's body at def-time
  means the body collect ended EARLY (or an if executed during playback). ONE focused bug.
- IfState now printed at EOF ("File ended while scanning conditional [file:line,...]") — the
  push site is the \ifnum#1<#2 at latex.ltx:408.
- Working theory: \def collection of \e@ch@ck (405-416) terminates early; verify by dumping the
  stored macro body (finish_def DEFALL dump prints body via tokens_to_string — NOTE: param-ref
  tokens print invisibly there; fix tokens_to_string to print PRn as "#n" first).
- Boot THESCAN-fail evidence chain: \the\LaTeXReleaseInfo (line 727) fails => \newtoks (726)
  failed to define the register => the e@alloc chain is THE boot blocker. Everything else
  (84 endcsname errors with hit=\@currname/\CurrentOption UNDEFINED, runaway SGET ~1700-9400+,
  Extra \or @3255) is downstream noise of the same root.
- Fixes landed this session: the_scan handles CountReg/DimenReg/SkipReg/MuSkipReg/ToksReg
  operands (scan.rs); scan_token_list tex.web recovery for leading non-brace token
  (control.rs — stores [\expandafter] + balanced group for `\toks-assign\expandafter{...}`).
- Boot errors now 927 (from 1090 at session start).

## Session-14 (final for this context)
- ea3.tex harness NOW CORRECT (slice [340:481], full alloc block): the "failures" in earlier
  variants were MY slice off-by-ones (afl/ea4 lesson: ALWAYS verify slice endpoints against
  the def boundaries; ea3 line N = latex line N+334 for [340:481]).
- Harness state: alloc block defines cleanly (DEF dumps perfect: e@alloc 6 params, e@ch@ck 4,
  PR refs verified). \newtoks\LRI + \the\LRI still fail at the tail: "You can't use \the"
  (THESCAN fail cs=LRI) + scan_token_list recovery loop hitting EOF on `\LRI{abc}` (tt2.tex
  reproduces: toksdef'd cs assignment `\LRI{abc}` enters the recovery loop instead of
  scan_balanced_raw — the first fetch returns a CS where `{` expected: suspected the
  cs_assign→scan_token_list first-fetch path or an endline/PAR_END aliasing).
- Fixes landed this session: the_scan register operands (CountReg/DimenReg/SkipReg/MuSkipReg/
  ToksReg); scan_token_list recovery (needs follow-up per above); tex.web if_limit (engine
  field no_cond_expand + guards in get_x_raw/get_token + set in IfNum/IfDim arms); tokens_to_string
  renders #n for param-refs; diagnostic prints (SILENT-DISPATCH, MISSNUM, SGET-BIG, IFNUM values,
  DEFINABLE/DEF dumps) all env-gated: IFTRACE, DEFTRACE, DEFALL, SKIPTRACE, EDEFTRACE.
- BOOT TREND: 1090 → 920 errors; execution reaches latex.ltx ~12442 (was stuck ~1770); remaining
  top errors: "Missing $ (subscript)" @10561, split@name @12442, \the-fails (\LaTeXReleaseInfo,
  \exp, \@nameuse) — all \the\<toksreg>-class per the tt2/ea3 root.
- NEXT (highest value first):
  1. Fix scan_token_list first-fetch (tt2.tex is the 5-line repro: expect THE-abc).
  2. Re-test ea3 (expect LRI-Y, THE-abc, EJ-OK, zero errors).
  3. Rerun boot; the \the-cascade should collapse; re-census.
  4. Then: \dump → format_done → user file path (latextest.tex).
- Regressions: nb/iir/gl2 equivalents = /tmp/textest/{g8,cs7,cs5,d2,ch}.tex all green;
  ea3.tex green through alloc block except the tail \the\toks issue above.

## Session-14 close: BREAKTHROUGH
- scan_token_list fixed properly: brace-led value path restored, recovery only for non-brace
  leading CS. tt2.tex: THE-abc ✓ (the \the\<toksreg> + toksdef path works end-to-end).
- FULL REGRESSION SET GREEN: ea3, tt2, d2, cs7, g8, ch — all 0 errors.
- BOOT: now executes to latex.ltx ~line 12831 (font series/shape rules), 10003 errors but the
  mix is SEMANTIC not scanner-level: \f@encoding UNDEFINED x7430 (@mandatory@arg playback),
  \DeclareFontSeriesChangeRule UNDEFINED x1424 (a loop), \DeclareFontShapeChangeRule x75,
  split@name arg errors. These are FONT-DECLARATION state macros — the region latex.ltx
  ~12400-12830 (font distance/series-change rules) — presumably defs that failed earlier or
  need hooks my engine lacks.
- NEXT: (1) find why \f@encoding/\DeclareFontSeriesChangeRule are undefined at 12831 — trace
  their defining constructs (\def\DeclareFontSeriesChangeRule etc. in the 12300-12800 region);
  (2) check for an edef/arg grab eating the defs; (3) after font region clears, expect rapid
  progress to \dump (currently NO \dump reached).
- The 11 THESCAN fails remaining: \LaTeXReleaseInfo (line 727 everyjob), \exp, \@nameuse —
  each needs the same class of fix as the ones already landed.

## Session-14 addendum: \def hijack localized
- DEFWATCH (env DEFWATCH=1, in control.rs CharDef arm + do_def_register) caught it:
  "\\chardef targeting \\def at line 622" — a \chardef (from \newbox's e@alloc conditional
  #2 = \ifnum..\expandafter\chardef\else..\fi, played via \global#2#6\allocationnumber)
  ends up scanning \def as its TARGET instead of #6. After that ALL \defs silently no-op
  (SILENT-DISPATCH cs=\def equiv=CharDef) → only ~202 defs ever registered (DEFTRACE count)
  → everything downstream undefined (\f@encoding x7430, \hidewidth, \narrower, ...).
- Suspect: \expandafter\chardef\else interaction — my expand_prim(Else) during \expandafter
  expansion misorders the pushed-back \chardef vs the post-\fi stream, so \chardef's
  scan_definable_cs consumes the wrong token. Compare tex.web: \expandafter saves \chardef,
  expands \else (terminates conditional, skips to after \fi), THEN re-inserts \chardef.
- NEXT: (1) inspect expand_prim(ExpandAfter) + Else arm ordering; (2) single-step a minimal
  repro: \countdef\allocationnumber=21 \chardef\x=1 — then the \newbox conditional idiom
  isolated: \def\cond{\ifnum0<1 \expandafter\chardef\else \expandafter\relax\fi}
  \cond\zz1 — expect \zz = CharDef(1), \def untouched.
- DEFWATCH stays in the tree (env-gated) until the boot passes.

## Session-14 final: minimal repro of the \def hijack (xf.tex)
- /tmp/textest/xf.tex reproduces: \cond body = \ifnum..\expandafter\chardef\else..\fi #1\allocationnumber;
  call \cond\zzA => "Undefined \zzA at cond 19/20" — the \expandafter\chardef\else idiom
  mis-executes: after \expandafter expands \else (which ends the accepted-if via skip_to_fi),
  the saved \chardef is re-inserted but \zzA is consumed/lost before \chardef's target scan.
- tex.web semantics to match: \expandafter saves t1=\chardef (RAW), expands the NEXT token
  (\else => conditional ends, skip-to-fi consumes [\expandafter,\relax]), then back_input t1
  so \chardef precedes [\zzA, \allocationnumber].
- Audit sites: expand_prim(ExpandAfter) arm, expand_prim(Else) arm ordering, and
  scan_definable_cs (add PR-token guard like do_let has).
- DEFWATCH instrumentation (control.rs) pinpoints all such hijacks: DEFWATCH=1.

## Session-14 FINAL addendum: xf.tex trace facts
- Trace of the minimal repro (DEFTRACE+IFTRACE): DEFINABLE count=4 (allocationnumber, @, cond,
  hidewidthX) — NO zzA target scan; the \chardef dispatch never happened; \zzA reached dispatch
  as undefined cs ("Undefined \zzA at cond 19/20", body pos 19/20 — NOTE: stored cond body should
  be ~12 tokens; the 20-count needs checking against the actual stored Macro body).
- IFPUSH accepting=false src=T:cond 4/20 tok=0x80000154(@) — the if pushed mid-body with cur_tok=@
  and NO IFNUM/IFX value print — the \ifnum went through an arm that lacks tracing, OR the body
  token stream is misaligned. Compare: EXPANDAFTER t1=0x1(\chardef) t2=0x156(\zzA) — t2 correctly
  = \zzA, pushed back; then the \chardef should dispatch next. It didn't reach scan_definable_cs.
- Prime suspect now: the get_token "Some(tok)" path after expand_prim(ExpandAfter) — the
  pushed t2 (\zzA) ordering vs t1 (\chardef) return, OR the intervening IFPUSH(false) skip
  (skip_branch) eating [\chardef, \zzA]. The IFPUSH(false) fired BEFORE the ExpandAfter line
  in the log (line order: IFPUSH@log, EXPANDAFTER@log after) — actually IFPUSH printed first,
  meaning the \ifnum evaluated FALSE (0<0 garbage) — the numbers scanned WRONG (\allocationnumber
  and/or \@cclvi unreadable at that point) — so the FIRST error is the \ifnum operand scan,
  and \chardef/\zzA loss is downstream of the FALSE-branch skip.
- NEXT SESSION: (1) add IFNUM operand print already in place — rerun xf to see a/b/rel values;
  (2) if a=0 or b=0, trace why \allocationnumber/\@cclvi CountReg/CharDef reads failed inside
  macro playback; (3) after \ifnum resolves TRUE, verify expandafter/else ordering.
- Boot total: 1090 → 920 errors, execution depth latex.ltx ~1770 → ~12831. All fixes and the
  full debug instrumentation catalog are in NOTES-BootDump.md + this file above.

## Session-15: xf2.tex = clean 38-line repro of THE boot blocker
- xf2.tex (full \newbox/e@alloc idiom with correct catcodes): `\newbox\strutbox` →
  DEFWATCH "chardef targeting \def" — \def becomes CharDef → all later defs die.
- Key trace (DEFTRACE): `EXPANDAFTER t1=chardef t2=def` — the \expandafter's expanding
  fetch (get_token) consumed [\else(borrowed from #2-substituted conditional), \expandafter,
  \e@alloc@chardef, \fi, \strutbox, \allocationnumber, \wlog{...}, \message{...}] — i.e. the
  \else's skip_to_fi did NOT stop at the conditional's \fi — it ran until just before \def
  (line 37) — then get_token returned \def as t2.
- IFPUSH evidence: both conditionals evaluated TRUE correctly (IFNUM 1 60 255/256 true) —
  the if condition is fine; the failure is in expand_prim(Else)→skip_to_fi NOT terminating
  at the expected \fi, OR the \fi token inside the SUBSTITUTED #2 arg stream not resolving
  to Prim::Fi (check: does the arg-substituted \fi cs resolve? skip_branch/skip_to_fi use
  eqtb.resolve(t.cs_id()) — a \fi token delivered from a SUBSTITUTED ARG toklist may carry
  a different cs_id or the no_cond_expand guard (added session-14) may be RETURNING \fi RAW
  from get_token while skip_to_fi uses raw_token + resolve — mismatch if no_cond_expand>0
  leaked from an earlier IfNum (check no_cond_expand balance!). SUSPECT #1: my session-14
  no_cond_expand guard broke \else/\fi expansion that skip_to_fi depended on. Try: disable
  the guard, re-run xf2.
- Instrumentation in place: DEFSCAN (scan_definable_cs target+pushed_top+line), DEFWATCH,
  EXPANDAFTER with token NAMES, IFNUM operands, IFPUSH src/tok. All env-gated
  (DEFWATCH/DEFTRACE/DEFALL/IFTRACE/SKIPTRACE/EDEFTRACE).
- Boot census at this state: ~12831 deep, 10003 errors, all from this one root (every \def
  after the first \newbox dies).

## Handoff state (session-15 end)
- All 6 regression micro-tests GREEN (ea3 tt2 d2 cs7 g8 ch).
- Boot: 10003 errors, depth ~12831 — ONE root cause left: the \expandafter\chardef\else
  idiom in \newbox's e@alloc conditional (xf2.tex = 38-line repro). Fix it and the def
  cascade collapses; then \dump becomes reachable.
- SUSPECT #1 (untested): the session-14 no_cond_expand guard interfering with \else/\fi
  expansion inside skip_to_fi — first experiment: disable the guard, re-run xf2.tex.
- SUSPECT #2: arg-substituted PR tokens returned raw from get_token during the
  \expandafter expansion (t2 should be \strutbox after the skip; check the player).

## Session-16: guard ruled out; exact failure signature captured
- SUSPECT #1 (no_cond_expand guard) RULED OUT: disabling both guards changes nothing
  (chardef still targets def). Guards re-enabled.
- SKIPFI trace: the skip DOES reach its fi (depth=0, pops correctly) - then DEFSCAN
  target=def fires with pushed_top=NONE. The substituted tokens after the conditional
  ([PR6=strutbox, allocationnumber, wlog{...}, message{...}]) are consumed between the
  skip end and the chardef target scan. The #2-arg toklist POPS when its fi is consumed;
  the remaining e@alloc body tokens [PR6, allocationnumber, wlog...] appear lost/mis-read;
  the fetch then jumps to the FILE next cs (def at line 37).
- NEXT (concrete): instrument input.rs TokList exhaustion: when the #2-arg toklist is
  exhausted mid-expandafter-expansion, what happens to the ENCLOSING body remaining
  tokens (PR6 at body pos 21)? Suspect nested param substitution pops one source too
  many at arg-toklist exhaustion. Fix direction: toklist exhaustion must pop ONLY the
  arg-toklist and resume the enclosing body at its saved position (consider inline
  pre-substitution of PR tokens before pushing the body - avoids nested-pop entirely).

## Session-17: new trace fact for the expandafter bug
- EXPANDAFTER t1=chardef t2=def srcs=[T:e@alloc 23/35 << F:37] — after t2's fetch the body
  pointer is at 23/35. The #2-conditional tokens were spliced around body pos 20-29, so a
  CORRECT skip would leave the pointer at ~30 (PR6=\strutbox). Pointer at 23 + t2=\def means
  the \else-skip consumed the else-branch AND the post-fi body tail, OR the player rewound.
- Prime suspect now: input.rs toklist pop/exhaustion during nested substitution (the #2-arg
  spliced inline; when skip_to_fi crossed the arg's \fi boundary, source bookkeeping jumped).
- Inspect input.rs push_toks/Source::TokList pop logic + how expand_macro substitutes PR
  args (inline splice vs nested source) around expand.rs push_macro.

## Session-18 note
- tok_ring was empty at the DEFWATCH point (ring maintained in raw_token under IFTRACE but
  the hijack window used get_token paths that bypass it, or the ring drained). Next session:
  move the ring fill to get_token's return path, or print ring inside the CharDef arm right
  after scan_definable_cs. The failing sequence remains: \expandafter(t1=chardef, t2=def)
  — get_token's t2 fetch consumed [\else-skip + \strutbox + \allocationnumber + \wlog{...}
  + \message{...}] and landed on \def; the \else's skip_to_fi stopped at the message-line
  \fi (line 36) instead of the conditional's own \fi.
- Wait — insight for next session: the skip's fi-at-depth-0 (SKIPFI log line 86) may BE the
  message-line \fi, with the conditional's \fi consumed EARLIER as part of the IfNum arm's
  own scan or the \else's do_if path — i.e. the \fi after the #2-arg's else-branch may have
  been delivered OUT OF ORDER relative to the if-stack push. Compare if_stack depth at the
  \else: if the conditional's \ifnum pushed TWICE (once via the spliced #2, once via the
  \global-adjacent path), the first \fi pops the wrong one.
- Everything else in this workspace is now parallelized across 8 agents (BoxesPack-2,
  LineBreak-2, PageShip-2, MathLayout-2, HAlign-2, PdfEmbed-2, BibRust-2, TexMk-2) with
  strict file ownership; Main keeps expand.rs/scan.rs/control.rs/input.rs/io.rs/engine.rs.

## HANDOFF SUMMARY (current truth)
- Build: clean. Regressions: ea3 tt2 d2 cs7 g8 ch all 0 errors. Boot: exit 1, ~10003 errors,
  depth latex.ltx ~12831.
- THE one blocker: \newbox (ltx:620) — \expandafter\chardef\else idiom — \chardef targets \def
  (DEFWATCH) → all later kernel defs no-op (SILENT-DISPATCH) → 10k-error cascade.
- 38-line repro: /tmp/textest/xf2.tex (DEFWATCH=1 reproduces instantly).
- Traces in place: EXPANDAFTER(t1/t2 names+srcs) in expand.rs; DEFSCAN(target+pushed_top+line),
  DEFWATCH(+ring), SUBTRACE(toklist_next deliveries: SUB-enter/TOK) in scanner.rs; SKIPFI in
  skip_to_fi; MISSNUM in scan_int; SILENT-DISPATCH in control.rs dispatch.
- Established facts: the \ifnum evaluates TRUE (1<256) correctly; the \else's skip_to_fi trace
  shows it consuming [space,\expandafter,\e@alloc@chardef,space,\fi] (the else-branch) and
  popping at a fi depth=0 — yet t2=\def arrives from the FILE (line 37) with pushed stack EMPTY
  and the body pointer mid-list. The LAZY in_param substitution (scanner.rs toklist_next) and
  the interaction between skip_to_fi's raw_token reads and the arg-exhaustion boundary
  (the conditional's \fi IS the arg's last token) is the unexplored mechanism.
- PRIMARY SUSPECT (untested): toklist_next delivers the substituted arg tokens; when the arg's
  last token (the \fi) is delivered AND consumed by skip_to_fi, something in the arg-exhaustion
  path (in_param=false / param_idx+=1 / body pos resume) skips the remaining BODY tokens
  [PR6=\strutbox, \allocationnumber, \wlog{...}] and lands on the file's next line. Check:
  does the \expandafter arm's pushed t2 get clobbered by the skip's own pushed.push(t0) in the
  SKIPTRACE instrumentation (instrumentation-off runs fail identically, so no), or does the
  toklist pop/continue skip body positions.
- NEXT ACTION: single-step toklist_next with SUBTRACE showing EVERY delivered token (not just
  SUB-enter) inside the window between "IFNUM ... true" and "DEFSCAN target=def" — the first
  delivered token AFTER the conditional's \fi is the tell: \strutbox (player fine, bug
  elsewhere) vs \def (player skipped body positions).

## SESSION 19 — ROOT CAUSE FOUND AND FIXED (Main)
THE bug (10k-error cascade): scanner.rs tokenize_char CAT_ESCAPE arm set end_state=1 (mid-line)
for control WORDS; tex.web requires state=S (skip-blanks) after control words. Spaces after
`\def\newbox   {...}` were delivered as tokens → became phantom parameter-text delimiters →
every such macro consumed call-site input at expansion (newbox's [sp,sp,sp] prefix ate
\strutbox + the next \message line → #6 arg = \def → \chardef\def → all later defs broke).
FIX: in the first-char letter branch, end_state=2. Boot errors: 10003 → 8756.
xf2.tex: NB-Y + DEF-OK. Regressions ea3/tt2/d2/cs7/g8/ch all green.

## NEXT TWO BOOT BLOCKERS (identified, handed to BootDump-3)
1. latex.ltx:585 `\global\let =\space` — \let definee must accept ANY token incl. active chars.
   Needs active-char meaning table (eqtb per-byte) + main-loop active-char dispatch.
2. latex.ltx:729 `\ifnum0\ifnum\patch@level=0 \ifx...1\fi\fi>0` — operand scans must expand
   conditionals (true get_x_token semantics); the no_cond_expand guard returns Else/Fi raw
   and breaks the idiom. Replace guard with proper conditional processing during scans.
Also early: "!!Nosyntaxforthecurrentdirectorycouldbefound" (line 6) — \@currdir parse nit.

## Session-19 (BootDump-3 takeover; Main's root fix + two scanner-domain fixes)
- Main found the TRUE root of the 20-session chardef/def hijack: tokenize_char left state=M
  after control WORDS; phantom spaces became param-text delimiters (end_state=2 fix). All my
  expandafter/else/eager-splice hypotheses were downstream noise. xf2 green, boot 10003→8756.
- FIX 1 (mine, scanner.rs): state-0/state-2 skip loops and state-1 spacer->S transition gated
  on RAW byte `b == b' '` — an ACTIVE space (cat 13) was swallowed pre-tokenize, so
  `\global\let =\space` (ltx:585) had NO definee ("Missing control sequence inserted") and all
  later active spaces stayed undefined CS (the "Undefined \ " flood). Now catcode-only skip
  (tex.web: only cat-10 spacers skipped). No new active-char table needed: CAT_ACTIVE chars
  already tokenize as interned CS tokens and flow through eqtb resolve (Alias-follow).
  => ltx:585 binds; "Undefined \ " errors gone.
- FIX 2 (mine): REMOVED the session-14 no_cond_expand mechanism entirely (engine field + guards
  in get_x_raw/get_token + counters in IfNum/IfDim). tex.web never suppresses conditional
  expansion during operand scans; the guard returned inner \fi RAW to scan_relational ->
  "Missing relational operator" at ltx:729 (\ifnum0\ifnum\patch@level=0 \ifx...\fi\fi>0).
  Conditionals now expand/recurse inside scans; skip paths still use raw fetches.
  => ltx:729 evaluates; release banner prints.
- REGRESSION NOTE: my state-1 edit initially dropped `self.line_pos += 1` before tokenize_char
  (escape scans consumed their own escape byte -> infinite loop, "\end" scanned as "\\").
  Restored; symptom was 2M raw-call spins with line_pos frozen. Anyone hitting hangs must
  rebuild — binaries older than ~13:45 have it.
- Main's forwarded PageShip repros: t4 (\count0=1 hang) = the above line_pos bug, already
  fixed; t5 (\toks0={\relax} under -ini) = expected INITEX behavior (braces cat-12 in raw ini,
  tex.web-faithful); passes under -plain. All toks/output arms have scan_optional_equals.
- VERIFIED GREEN (snapshot build w/ all fixes): xf2 ea3 tt2 d2 cs7 g8 ch act1 = 0 errors.
  act1 covers both new idioms (active-space \let+\obeyspaces, ifnum-nested-in-ifnum).
- BOOT: 8756 -> 6197 errors. Remaining flood: "Missing $ inserted (subscript)" (L3/expl text
  typeset, ltx 2100-3205) = downstream of the NEXT blocker:
- NEXT BLOCKER (diagnosed, not yet fixed): "Extra \fi at ltx:1731". Evidence: no IFPUSH in the
  whole trace between ltx:387 and the failure; DEFALL shows d@ii(1736) registered but
  \@ifundefined(1729) and d@i(1731) NOT -> lines 735..1731 were consumed by a conditional SKIP
  that stopped at d@i's body \fi (1731) = "Extra \fi", then played the rest of d@i's def as
  code ("\@ifundefin@d@ii undefined" at 1733). Root: if_stack imbalance in the ltx:729
  `\ifnum0\ifnum\patch@level=0 \ifx...\fi\fi>0` window after FIX 2: outer scan_int's digit-loop
  pushes back the terminating space mid-operand-scan while inner ifnum/ifx states are open;
  pops then mis-balance (trace shows an anomalous IFPUSH accepting=false at line=734 during
  what should be the outer's skip) and a leaked state eats everything to 1731.
  NEXT STEP: instrument IFPUSH/POP/EXTRAFI with if_stack depth snapshots in the 725-740 window;
  likely fix = don't push back the absorbed terminator space across an expanding-fetch
  boundary, or make skip_branch/do_if state accounting re-entrancy safe. After this clears,
  expect the L3 subscript flood to collapse (defs register again) and rapid progress to \dump.

## Session-19 addendum: ltx:1731 root-cause narrowed to the 889-skip (instrumented)
- Instrumented skip_branch/skip_to_fi (SKIPBRANCH-START/SKIPTOFI-START/POP/FIPOP, env SKIPTRACE)
  in the /tmp/bd3build snapshot. Findings:
  1. The ltx:729 release block is FULLY BALANCED (IFPUSH 4509/4514/4522 + FIPOPs 4567-4573: stack
     4->0 at ltx:759). NOT the leak site.
  2. THE LEAK: `SKIPTOFI-START stack=1 file-line=889` = the `\ifx\directlua\@undefined\else`
     ltluatex-module gate (ltx:889). TRUE (directlua undefined) -> skip_to_fi of the whole
     ltluatex module. The skip NEVER pops at the module's closing \fi; it runs to ltx:1731
     (SKIPTOFI-POP stack=0 file-line=1731), consuming the defs at 1729-1735 => \@ifundefined/d@i
     never register; execution resumes mid-d@i-def: "#2{" -> group, "\fi" -> "Extra \fi" (empty
     stack), "\@ifundefin@d@ii" -> undefined (defined later at 1736 — DEFALL confirms ONLY d@ii
     registered). Then everything downstream cascade (6197 errors).
  3. The skip's depth trajectory: oscillates 0-5 through 890-1013, depth=3 at the blank line
     1029 (PAR_END token — also note: skip paths treat PAR_END (0xffff_fffe) as a CS token,
     resolve(0x7ffffffe) -> None -> silently skipped; instrumentation must guard cs.name or it
     panics at token.rs:99), depth=2 at 1234, pop at 1731. So the module region 890-1731 counts
     NET non-zero: the skip's \if*/\fi depth accounting loses the module's closing \fi.
  4. Raw counts in the skip window (bootC.out, /tmp): if-openers 58, fi-closers 64 (mixed with
     later skips — re-derive in isolation). Candidate mechanisms to check next session:
     a) skip counting a resolved-to-Fi ALIAS (e.g. \let\repeat\fi tokens) where real tex.web
        skips by eq_type WITHOUT alias-following — our resolve() follows aliases;
     b) an \if*-opener INSIDE a def param text or \csname construction counted +1 whose mate is
        never seen;
     c) PAR_END swallowing interacting with the counter (a \par between an \if and its \fi).
  5. NEXT SESSION: isolate the 889-skip in a micro-test (verbatim ltx 885-1110 in -plain), dump
     per-token counted +/- with line numbers, find the exact token where real TeX and we diverge.
     Fix likely = skip should match tex.web get_nb_token semantics (NO alias-following when
     classifying conditionals) or handle PAR_END explicitly.
- All instrumentation lives only in the /tmp/bd3build snapshot; workspace files carry only the
  env-gated ARGTRACE/LOAD prints (harmless, remove when boot passes).

## Session-19 close: integration batch status
- APPLIED (mine): scan_glue keyword lookahead fix (HAlign regression): the plus/minus loop used
  skip_spaces_relax which SWALLOWED a restored \relax token, re-reading the caller's sentinel
  multiple times (\hskip1em\relax\crcr). Now: blank-space-only lookahead per keyword, token
  restored untouched; dead diagnostic locals (c2/c3/c4/t2/t3/t4) removed. Verified: sg1.tex
  (\hskip 1em\relax\crcr) 0 errors; full regression set (xf2 ea3 tt2 d2 cs7 g8 ch act1) all 0.
- PENDING (apply when tree compiles — blocked by page.rs/math.rs mid-flight):
  1. engine.rs init_primitives: d! leaders/cleaders/xleaders/wd/ht/dp (prim variants exist).
  2. control.rs try_assignment Wd|Ht|Dp arm -> build.rs do_box_dimen_assign + top-level
     MathCharDef dispatch arm (spec in NOTES-MathLayout.md).
  3. scan.rs value positions \wd/\ht/\dp -> boxes::box_reg_dimen.
- ltx:1731 blocker: fully diagnosed (889-skip overshoot; see addendum above). Isolation
  micro-test + fix is the next session's first action; all instrumentation lives in the
  /tmp/bd3build snapshot only.
- Main batch #2 addendum: outputpenalty/floatingpenalty int-param registrations APPLIED (engine.rs:473-474). STILL QUEUED: leaders/cleaders/xleaders/wd/ht/dp d! registrations; control.rs Wd|Ht|Dp arm + MathCharDef top-level arm; scan.rs \wd/\ht/\dp value positions; NEW PageShip bug — register operands inside token-list playback fail (repros /tmp/s1.tex /tmp/s2.tex /tmp/c6.tex); scan_glue fix APPLIED and verified (see close).

## Session-19 FINAL: ltx:1731 CASCADE CLEARED — root cause = \unless double-count in skips
- ROOT CAUSE (verified): skip_branch/skip_to_fi/skip_case_skip counted `\unless` AND its
  following conditional as TWO openers; tex.web treats the pair as ONE. latex.ltx has two
  `\unless\ifnum...` / `\unless\ifeof...` pairs at ltx:975/983 (inside the ltluatex module
  gated by `\ifx\directlua\@undefined\else` at 889 — TRUE for us) => the 889-skip counted +2
  excess openers, missed the module's closing \fi, and consumed everything to ltx:1731,
  swallowing the \@ifundefined/d@i defs (1729-1735). Classic 20-session ghost, unrelated to
  eager/lazy args or expandafter.
- FIX (expand.rs, all three skip paths): new helper skip_count_unless_target — on Prim::Unless
  fetch the next token; count +1 ONLY if it is a conditional prim (full tex.web list incl.
  nested \unless). Workspace-applied, compiles clean.
- RESULT: latex.ltx now processes with ZERO errors (was 6197; 10003 at session start).
  All 9 micro-tests green on the workspace binary (xf2 ea3 tt2 d2 cs7 g8 ch act1 sg1).
- LAST BLOCKER before \dump: PageShip-4's fresh page builder panics at page.rs:184
  `page_list[..st.processed]` — st.processed=4 (carried PageState) vs page_list.len()=1
  ("range end index 4 out of range for slice of length 1"). Reported to PageShip-4 + Main.
  Once clamped/rehydrated, \dump => format_done => document path.
- Instrumentation cleanup: SKIPBRANCH/SKIPTOFI/SKIPCNT/FIPOP prints live only in the
  /tmp/bd3build snapshot; workspace expand.rs carries only the env-gated ARGTRACE/LOAD.

## Session-19 addendum 2: documentclass loop (\@preamblecmds/\@onlypreamble) — analyzed, NOT fixed
- Symptom: \documentclass → LOOPGROW exit 7: toklist cycle [@preamblecmds 1/12 → @onlypreamble
  7/10 → @preamblecmds 3/12 → @preamblecmds 1/12 → ...] to 50001 sources.
- Mechanism: ltx:1224 `\def\@onlypreamble#1{\expandafter\gdef\expandafter\@preamblecmds
  \expandafter{\@preamblecmds\do#1}}` — three chained \expandafters; the innermost expands
  \@preamblecmds (whose body self-references via `\do\@preamblecmds`, appended by ltx:1228).
  With plain.tex's `\do=#1-executor` still bound, each body playback re-executes
  \@onlypreamble/\@preamblecmds → unbounded nested toklists. Real TeX terminates because the
  expandafter cascade's expansion products are COLLECTED RAW by the gdef body collector.
- ATTEMPTED FIX (tex.web-exact expandafter: raw-fetch a and b, expand b exactly ONCE, back-input
  a before products): compiles; the boot then died differently ("Undefined \GenericInfo" at
  1117 when the prim-result token was dropped; with the result streamed, a runaway gdef body
  collection — the `{` and the macro-body source ended up ordered so do_def's param text
  consumed sources). REVERTED to the original arm (t2=get_token) — current state: latex.ltx
  boot 0 errors, 9/9 regressions green, documentclass loop = the frontier blocker.
- NEXT SESSION precise plan: (1) reproduce with MACTRACE to see \@onlypreamble's captured #1
  per cycle; (2) the key invariant to restore: during the EX cascade the intermediate t1 tokens
  (\gdef, \@preamblecmds, {) must be back-input in nest order BEFORE the innermost expansion
  products, and the products must be consumed by the gdef collector RAW — i.e. implement
  expand_after with an explicit product buffer instead of source-pushing, or make expand_macro
  accept a "prepend tokens" mode; (3) alternative quick probe: \let\do\@gobble before the
  gate to see if the loop needs \do's execute semantics (it should not fire at all in real
  \document because \do is \gdef\do#1{\global\let#1\@notprerr} there — verify OUR \document
  def body's `##1` collapse actually stored `#1`).

## Session 20 (Main) — expl3 load path
Fixes landed:
- \futurelet: park peeked token on input.stack so tb's expansion plays first (two-stack hazard).
- scan_file_name: raw_token (don't expand \ifeof), quoted names, stop on braces; do_input uses it.
- pdfTeX existence primitives: \pdffilesize, \pdfstrcmp, \filesize, \pdfmdfivesum, ...
- after_prefix skips spaces (\protected \def).
- Tokenizer skips CAT_IGNORED/INVALID; EOL with ignore cat produces no token.
- get_x_raw continues expanding when \expandafter yields another expandable (\ifx).

Boot now inputs expl3.ltx + expl3-code.tex. Census ~10002 errors, first unique expl3
failure Missing CS at expl3-code.tex:1538 (\cs_gset_protected:Npn bootstrap). Format
not dumped. Isolated \IfFileExists pattern (\openin\@inputcheck"expl3.ltx" \ifeof) works.

## Session 21 — protected macros, #{, lastnamedcs
Fixes:
- Main dispatch executes e-TeX protected macros (cs_gset_protected:Npn bootstrap).
- pop_level no longer underflows; extra `}` is "Too many }'s" not a panic.
- \lastnamedcs primitive; \csname records last_named_cs.
- Token::is_space() is any catcode 10 (expl3 `~`).
- \the\chardef prints the decimal value.
- #{ parameter convention: last arg delimited by `{`, brace reread.

Boot: expl3.ltx + expl3-code.tex load. ~15 errors (was 10002). First unique
failure still expl3-code.tex:1882 (\use:e / \cs_split_function:N /
\prg_gset_conditional). Isolated \expandafter\tmp\string: with `:` letter
still does not expand \string (ARG=\string:). Format not dumped.

## Session 22 — expansion LIFO
Unread tokens (`pushed`) and expansion output must share one LIFO:
inner `\ifnum\the\count@` used to back up `<` onto `pushed` while `\the`
left digits on the input stack, so the outer scan_int saw `<` first.

Fix: macro replacement (`expand_macro`) uses `push_tokens` (same LIFO as
`\string`/`\the`). Isolated tests: `\two@digits{\the\count0}` → 08,
`\cs_to_str:N\cs_if_exist:N` → `cs_if_exist:N`, `#{`, lastnamedcs.

Boot: latex.ltx:175 gone. Still ~29 errors, first unique expl3-code.tex:1882
(`\cs_split_function:N` / `\use:e` got `\c_false_bool`). Triple `\expandafter`
through `\cs_to_str:N` still not feeding `\__cs_split_function_auxi:w`.

## Session 22b — alias expandafter
get_token used eqtb.get not resolve on ExpandAfter's result, so
`\exp_after:wN` (\let to \expandafter) was dispatched unexpanded.
Fix: resolve + continue if expandable prim or unprotected macro.

Isolated `\cs_split_function:N\cs_if_exist:N` → `{cs_if_exist}{N}\c_true_bool`.
Boot 29→20 errors; 1882 now Missing `{` got letter `p` (variants list), not
`\c_false_bool` — colon split is working in the format boot too.
