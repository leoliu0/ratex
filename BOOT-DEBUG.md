# Rust TeX engine — boot debug state (2026-08-30)

## Status
- Workspace: /home/leo/dd/tex; crates tex-core, tex-kpse, tex-cli (pdflatex, tex-bibtex, texmk).
- Build: ALWAYS `./build.sh` (plain cargo misses tex-core changes; script touches lib.rs+sources first).
- Test: `./target/debug/pdflatex -output-directory /tmp/textest /tmp/textest/latextest.tex`
  (-plain mode = INITEX+plain macros for micro-tests; unit tests in /tmp/textest/*.tex).
- Oracle: real pdflatex installed (TeX Live 2026).

## latex.ltx boot progress
- Boot runs to ~line 2235/22600 (L3/ltcmd layer). Error census ~10k, dominated by
  "No font selected (char ignored)" = chars typeset that should have been consumed by
  \IncludeInRelease module-gobbling machinery.
- First failures: "Extra \fi at 587", "Extra \else at 826 [EndIncludeInRelease 4/61]",
  "Undefined \if@skipping@module". Module-skip (gobble to \EndIncludeInRelease) misfires.

## Engine bugs fixed this session (all verified via micro-tests)
1. \expandafter returned tokens reversed (must deliver t1 first, push t2).
2. get_token now skips space tokens (tex.web get_x_token).
3. scan_digits/decimal constants read RAW; stop at first space, absorb exactly one
   (`\ifnum1<10 0\fi` → true-branch `0`).
4. M-state + space → state 2 (skip_blanks): next line's indent no longer leaks into macro bodies.
5. scan_dimen units: incremental prefix matching (pt vs filll), fil/fill/filll set
   engine.cur_fill_order; trim over-read letters (push back).
6. scan_glue: consume ALL keyword letters ("minus" is 5!); stretch/shrink orders from cur_fill_order.
7. Active chars tokenize as CS tokens (interned per byte), incl. via ^^ notation.
8. Escape-name scan: first char expands ^^; continuation reads RAW letters only, terminator
   stays in buffer (fixes \def^^L etc.).
9. ifx_equal: undefined cs == \relax → true (critical for \csname...\endcsname\relax tests).
10. get_token: expandable prim delivered by expand_prim (e.g. \expandafter\ifx) is re-expanded.
11. scan_delimited: braced-group match with single `{` delimiter checked di>=len (panic fix).
12. \futurelet pushes back only t2,t3 (target consumed) — was re-executing peeked tokens.
13. do_let: skips optional `=` (was aliasing `=` as CharTok source).
14. New params: defaulthyphenchar, defaultskewchar, errorcontextlines (IntParam);
    hfuzz, vfuzz, overfullrule, nulldelimiterspace, scriptspace, topskip (DimParam).
    NUM_INT_PARAMS=64, NUM_DIM_PARAMS=27.

## Suspicion for next step
- \futurelet/@ifnch [peek] chain now correct; remaining misalignment likely in
  \@gobble@IncludeInRelease delimited grab (\EndIncludeInRelease delimiter) or
  \kernel@ifnextchar branch (@xifnch space-skip path) — check \@ifnch def at latex.ltx:1108/1762.
- Errors at 587/826/878/1202/2235 all involve conditionals + delimited grabs across
  \ExplSyntaxOn regions (catcode _:=11 toggling) — verify \ExplSyntaxOn catcode assignment.
- Debug helpers present: DEFTRACE/MACTRACE/SKIPTRACE env vars; DEFINABLE/EXPANDAFTER/GLUEKW/
  UNITFAIL/EALLOC ARGS/IIR ARGS eprintlns (some still in code — clean up later).

## Session-2 findings (later)
15. \futurelet now assigns CharTok meaning for char peeks (kernel@ifnextchar works with [ peeks).
16. NEW primary suspect for remaining 10k errors: \@gobble@IncludeInRelease's delimited grab
    (#1 up to \EndIncludeInRelease) and \@check@IncludeInRelease's grab (#1 up to
    \IncludeInRelease) MIS their delimiters: trace shows \IncludeInRelease EXPANDED with
    arg {\@check@IncludeInRelease} (should have been consumed as a grab delimiter).
    => inspect scan_delimited CS-delimiter matching when the grabbed content spans a
    toklist whose tokens were grabbed from ANOTHER macro's arg (nested toklists).
17. IFTRACE/EXTRAFI/FIREACH/MOD eprintlns are env-gated (IFTRACE/DEFTRACE); remove when done.
18. First boot errors in order: "Extra \fi at 587" (\loop def region, token char `1`
    dispatched as \fi with stale cur_prim!), then "Extra \else at 826".
    => STALE cur_prim: main_loop routes a CHAR token by the previous token's prim.
    Check: main_loop must use the prim of the CURRENT token only (set_cur_char clears it,
    but some paths deliver chars without set_cur_char - e.g. CharTok arm sets cur_prim=None
    correctly; look at main_loop's match on cur_prim for char tokens).

## Session-3 findings
19. Boot stalls at latex.ltx line 2172 (ltcmd/L3): 5710 "No font selected" + 1177
    "Missing $ (subscript)" = L3 code text TYPESET instead of defined
    => \ExplSyntaxOn/_ catcode pairing broken by earlier module-skip desync.
20. MOD trace: first \@IncludeInRele@se call receives #1 = MODULE CONTENT (misaligned
    args) => \kernel@ifnextchar decided [ EXISTS when it did not, or \@IncludeInRelease's
    [#2] grabbed the wrong group. Investigate \futurelet/\@ifnch decision with a micro-test
    of the EXACT defs from latex.ltx:1100-1120 (kernel@ifnextchar, @ifnch, @xifnch,
    @execifchar) copied verbatim into a plain-mode test file.
21. "Extra \fi at 587" is NON-FATAL (boot continues); the FATAL failure is reading
    terminating at line 2172, never reaching \dump at 22466 => format_done never set.
22. Micro-tests that pass: nb.tex (newbox/e@alloc), iir.tex (csname+ifx+parse@version),
    gl2.tex (fill glue), ff.tex (^^L active), mod.tex (simplified module chain).
    => next micro-test MUST use the verbatim latex.ltx defs (1100-1120) not simplifications;
    simplifications have twice masked real bugs.

## Session-4 findings
23. FIXED: \futurelet arity — tex.web futurelet = target,B,C (A:=C's meaning; B,C replay).
    Was grabbing 3 tokens after target (stealing C's successor) → kernel@ifnextchar and
    ALL optional-arg macros broken. fut3.tex now prints YRAN ✓.
24. NEW focus: \@ifnextchar mis-grabs #3. Trace: FUTLET peek = char(1,'{') but should be
    char(12,']'). The `\def\reserved@b{#3}` chain: #3 grabbed {OPT-ARG} group, `]` should
    remain for the futurelet peek but is consumed somewhere in do_def/do_let/arg playback.
    => instrument: print each raw token (value+name) during \@ifnextchar expansion.
25. After futurelet fix, re-check boot census — the earlier 10k estimate may improve.

## Session-4 addendum (final state)
- Boot census unchanged at 10005 — the \@ifnextchar #3 mis-grab must be fixed BEFORE the
  futurelet fix can cascade. kic.tex reproduces: expected WITH-BRACKET/OPT-ARG split,
  got OPT-ARG/[typeset/NO-OPT.
- Confirmed via FUTLET trace: peek = char(1,'{') but char(12,']') expected at that point,
  i.e. the `]` between {OPT-ARG} and {NO-OPT} disappeared during \@ifnextchar's #3 grab
  or \reserved@a/\reserved@b collection.
- All debug eprintlns: IFTRACE-gated (IFPUSH/EXTRAFI/FIREACH/IFX/FUTLET), DEFTRACE-gated
  (DEFINABLE/EXPANDAFTER/IIR ARGS/MOD/LOG), MACTRACE (MAC), SKIPTRACE (SKIPSTART).
  Cleanup: remove or keep behind cfg(feature="debug") when boot succeeds.

## Session-5: ROOT CAUSE ISOLATED (reproduces in 68-line micro-test)
Micro-test /tmp/textest/irm.tex + nmr.tex: verbatim latex.ltx lines 761-883 + \fmtversion +
\newif\if@includeinrelease + one plain module = reproduces the boot failure!
Sequence: \IncludeInRelease{date}[tag]{name} ... \EndIncludeInRelease.
MOD trace: \@IncludeInRelease expands with #1 = {\@check@IncludeInRelease} (the check
macro's own cs as a braced arg) and #2 = the REST of \IncludeInRelease's own body text.
=> the token pair `\IncludeInRelease\@check@IncludeInRelease` (latex.ltx:828-829, inside
\@gobble@IncludeInRelease's body) re-fired \IncludeInRelease as a CALL with #1 := the
check cs. That means \@check@IncludeInRelease's delimited grab (#1 up to the
\IncludeInRelease delimiter) did NOT consume the delimiter token.
=> inspect scan_delimited CS-delimiter matching during in_param playback in this exact
   configuration (check called from gobble body; content = PR1 playback).
FIX THIS → module machinery works → boot reaches \dump → format_done → document path.

## Session-6 (latest)
26. FIXED: added get_x_raw() (tex.web get_x_token: expands, does NOT skip spaces).
    scan_int/scan_digits now use it: expandables continue a number, a space terminates it.
    This fixed the \ifnum0..=11 gate — fo2.tex now prints FIRST-PATH/SECOND-PATH correctly.
27. irm.tex (plain \IncludeInRelease module, verbatim defs) STILL fails: \@IncludeInRelease
    expands with #1={\@check@IncludeInRelease} and #2=\IncludeInRelease's own body tail —
    i.e. the token pair \IncludeInRelease\@check@IncludeInRelease (latex.ltx:828) re-fired
    \IncludeInRelease as a call. The \@check@IncludeInRelease delimited grab consumed its
    content but NOT the \IncludeInRelease delimiter.
    => NEXT: single-step scan_delimited for the check's grab inside \@gobble's body
       playback (in_param inline content then delimiter in body tokens). Suspect: after
       param exhaustion the delimiter token returns from toklist body but scan_delimited
       compares against delim[di] where delim was collected with a different cs id, OR the
       grab terminates on the braced-group branch. Ring-trace raw fetches during the check.

## Session-7: PARAM-TEXT FIX + module args aligned
28. FIXED: do_def param-text loop used EXPANDING get_token — a defined macro used as a
    delimiter (e.g. \def\@gobble#1\EndIncludeInRelease right after \def\EndIncludeInRelease)
    EXPANDED during collection. tex.web scans param text non-expanding → now raw_token.
29. After fix: IIR ARGS are correctly aligned (#1={2020-10-01} ✓). irm.tex census 233
    (was ~chaos). Remaining in irm: \newif\iffoo defines the cs but emits 2 "Undefined
    \iffoo" during \@if's csname dance and leaves the flag TRUE instead of FALSE
    (\newif is plain.tex 263-266 with the \uppercase{\gdef\if@12{}} trick — test nif.tex).
    => \newif/\@if/\string/\csname interplay next.
30. Boot census 10002, reading reached line 2177 pre-fix; re-measure after next run.

## Session-7 final state
31. After get_x_raw + param-text fixes the boot's non-typeset errors moved to lines
    1731/1808/1817: "Illegal unit" inside \MakeRobust\bigbreak — the \ifdim\<skip-cs>
    comparisons inside \bigbreak's body mis-evaluate when MakeRobust expands/inspects it
    (leftover `<`, `-200` tokens = \ifnum\penalty<-200 leaked). NEXT: test \ifdim with
    skip-register operands and \MakeRobust's \csname\string#1\space machinery in a
    verbatim micro-test (latex.ltx 1720-1820 defs).
32. \newif still buggy (nif.tex): 2 spurious "Undefined \iffoo" during \@if's csname dance
    and flag ends TRUE instead of FALSE. Affects first \if@includeinrelease use.
33. Regression: nb/iir/gl2 pass. Boot census ~10002; reading beyond 2177 into ltcmd.

## Session-8 BREAKTHROUGH
34. Boot reads 22461/22466 — FOUR LINES from \dump! Census 10005 → 1090.
35. Remaining before \dump completes:
    a. "Missing } in expanded text at line 0" (fatal-looking; from an edef/scan_toks).
    b. `\@input{latex2e-first-aid-for-external-files.ltx}` at 22461 mangles to
       "File `#1.#2' not found" — the filename arg arrived UNEXPANDED (literal #1.#2):
       the \@input chain (@input → \@iinput \expandafter... or \reserved@c/@filef@tch)
       breaks in an \expandafter/\csname context. This is the LAST input before \dump.
    c. Errors at 22264/22300: the \ifnum 0 \ifx...\fi >\z@ gate — \z@ (a dimen) as an
       \ifnum operand → "Missing number, treated as zero" — real TeX accepts \dimen in
       scan_int?? (verify tex.web §469; likely scan_int accepts dimen refs). Non-fatal.
36. If (b) is fixed, \dump at 22466 runs → format_done → document path opens.

## Session-8 final finding
37. The LAST blocker before \dump: `\@input{latex2e-first-aid-for-external-files.ltx}`
    (line 22461). The error "File `#1.#2' not found" = \@missingfileerror received
    LITERAL #1/#2 param tokens — a macro body containing PR_REF(0x4000000n) tokens was
    played WITHOUT in_param substitution (raw playback of a body: PR tokens leaked).
    => global fix candidate: toklist playback should ALWAYS substitute PR tokens
       (or bodies must be stripped of PR tokens when stored via non-arg paths,
       e.g. edef of \filename@area\filename@base after \filename@parse).
    Audit: find where PR_REF tokens enter eqtb-stored bodies (edef capturing PR).

## Session-8 final
38. \openin now routes through kpse (was literal fs::File::open — first-aid file unfound).
39. The true boot terminator: scan_general_text_expanded started at latex.ltx:18743
    (\@onefilewithoptions def region — the package-loading machinery) NEVER RETURNS —
    swallows everything to EOF including \dump. This edef:
    \edef\reserved@c{\def\noexpand\reserved@c####1\detokenize\expandafter{\expanded{.#4}}\noexpand\@nil{\def\noexpand\reserved@a{####1}}}\reserved@c
    (the \detokenize\expandafter{\expanded{...}} + #### doubling dance). My Detokenize
    uses NON-expanding scan (tex.web detokenize EXPANDS its text) and the ####1 handling
    through edef-body-collection → playback → inner-def needs verification.
    => fix scan order/expansion in Detokenize + verify #### doubling; this is the last
       blocker: after it, \dump runs → format_done → \documentclass path opens.

## Session-8 close
40. Added ## collapse in scan_general_text_expanded (kept; harmless). Census unchanged
    (1090) — the 18743 runaway mechanism is NOT the #### doubling; SGET-START at F:18743
    fired while reading the \@onefilewithoptions DEF (collect is raw — so an edef
    executed at that reading position from OTHER playback, or the line_no is stale from
    in_param playback of a body grabbed near 18743). Next: tag SGET-START with the
    macro-name history ring (last_macros) to identify the caller, then fix.

## Session-9 (final)
41. The runaway expanding grab at the boot tail is the `\xdef\@filelist{\@filelist,#1}`
    inside \@addtofilelist (latex.ltx ~22464, right before \dump). Caller ring:
    reserved@a ×4 (\@filelist's stored body plays reserved@a tokens — the \@filelist
    chain contains \@gobble/file entries and \let\reserved@a\@filelist aliases from
    line 22446). The xdef's expanding scan never finds its closing brace.
    => NEXT: micro-test \@addtofilelist + \@filelist=\@gobble + two \addtofilelist calls
       (verbatim defs from latex.ltx 22440-22466); inspect the xdef grab of
       {\@filelist,#1} when \@filelist = \@gobble{file1},file2 form.
42. This is the LAST blocker: after it, \dump → format_done → user file runs.

## Session-9 addendum
43. afl.tex (verbatim \@filelist/\@addtofilelist + two calls) PANICS:
    build.rs:459 end_box saved_lists.pop().unwrap() on empty — a stray `}` reaches
    end_group/end_box during the \@addtofilelist xdef chain. Root: \xdef\@filelist
    {\@filelist,#1} — first call stores {file1} (scan_general_text_expanded STRIPS the
    outer braces — tex.web scan_toks KEEPS them in the macro body); the brace
    strip/store asymmetry then unbalances groups on the second call.
    => FIX: scan_general_text_expanded must PRESERVE the outer {} in the collected
       tokens when used for macro-body xdef (tex.web scan_toks keeps braces); OR add
       the braces at the Expanded/xdef call site. Also audit scan_balanced_raw users.
44. This panic is the current boot blocker (xdef of \@filelist runs at latex.ltx tail).

## Session-9 close
45. afl.tex PANIC reproduced minimal: \@filelist=\@gobble + \@addtofilelist{file1.ltx}
    + {file2.sty} → build.rs end_box saved_lists empty. The xdef body store/play of the
    {file1.ltx} braces vs dispatch-level begin_group/end_group state is inconsistent
    (a `}` reaches end_box with no saved_lists entry). Trace with IFTRACE + a counter
    print in begin_group/end_box to find the first imbalance.
46. All state committed to BOOT-DEBUG.md. Regressions nb/iir/gl2 green.

## Session-10 (final): afl panic root-caused
47. afl.tex ENDBOX trace: kinds=1 saved=2 at first `}` — a \message{ADD-OK} begin_group
    (kinds+saved+targets) ran INSIDE a paragraph that auto-started when the stored
    \@filelist chars (`file1.ltx`/`file2.sty`, xdef-collected as bare char tokens after
    the \@gobble absorbed the comma) reached dispatch and entered hmode via the
    par-start at build.rs:909 (pushes saved_lists + eqtb level, NO box_kinds).
    The `}` then end_box'd the par's saved_lists instead of the message's → kinds/saved
    pairing permanently skewed → third end_box → PANIC on empty saved_lists.
    NOTE: in real TeX the stored \@filelist = \@gobble{file1}, {file2} macro entries —
    NOT bare chars; the bare chars mean the xdef EXPANDED \gobble's empty body and the
    {file1} arg braces are correct, but the PLAYBACK typesets the file names because
    \@filelist's stored body is a plain char list (real LaTeX stores them as
    {\@gobble{file1},...} one-macro structure — check the xdef expansion of
    \@filelist: real stores `\@gobble {file1.ltx}` — i.e. the \@filelist cs itself
    expands via \@gobble each time — my stored version lost the \@gobble prefix).
    => NEXT: in the xdef, \filelist must expand to \@gobble{file1} (the \@gobble stays
       in the body). Verify why my expand of \@filelist = \@gobble consumed the `,`
       AND left the {file1} bare instead of storing [\@gobble, {file1}]. The xdef text
       is {\@filelist,#1} — expanding \@filelist=\@gobble plays \gobble which grabs the
       `,` — tex.web: identical — BUT real TeX \xdef result: \@filelist := \@gobble
       {file1.ltx} — because \gobble's expansion is EMPTY and {file1} remains as a
       GROUP in the edef text ✓ — my stored body lacks... the { } braces of #1? THE
       ARG BRACES: real tex.web scan_toks KEEPS the braces of undelimited group args
       in edef?? NO — #1 in edef expands to the tokens WITHOUT braces, and tex.web
       ADDS braces?? — tex.web: macros with text args: the braces are NOT part of the
       arg. Real \@filelist after two adds = \@gobble {file1.ltx}, {file2.sty}?? —
       grep a real .log/\showthe. THIS determines the correct stored form.

## Session-10 close
48. ORACLE (real pdflatex): after two \@addtofilelist calls, \@filelist =
    macro:->file1.ltx,file2.sty — BARE char text (xdef expands \@gobble away, arg
    braces stripped). My engine stores the same ✓. So the afl panic is NOT storage.
49. Panic sequence: par auto-start (build.rs:909, pushes saved_lists without
    box_kinds) fires while a begin_group (kinds+saved+targets) is open; the `}`
    end_box pops box_kinds+saved_lists MISMATCHED (pops the par's saved instead of
    the group's), skewing counts; a later cleanup end_box panics on empty.
    => NEXT: make end_box pop SAVED_LISTS only when box_kinds had an entry
       (par-level saved_lists pair with eqtb.push_level, not end_box), i.e.
       separate par-save stack from group-save stack; or make end_box in hmode
       just pop the eqtb group level like tex.web (group end closes \begingroup
       without packaging in real TeX — tex.web group_end in hmode does NOT call
       end_graf; it pops save-restore only).
50. After this fix: rerun afl.tex → ADD-OK; then boot → \dump → format_done.

## Session-11 close
51. afl ENDBOX trace with pars=0: between \message's BEGINGROUP-ctl (kinds->1 saved->1)
    and the first ENDBOX (kinds=1 saved=2), an UNTRACED saved_lists.push fired — the
    remaining untraced sites are build.rs:918/936 (par / internal-vbox start on mode
    transition) and math.rs/align.rs. I.e. a PARAGRAPH auto-started inside the \message
    group — chars entered hmode during/after the \@addtofilelist xdefs.
    => NEXT: trace build.rs:918/936 pushes (PUSH-918/PUSH-936 tags inserted at wrong
       lines by an earlier patch — re-insert by content); find what typesets chars
       after \@addtofilelist (suspect the stored \@filelist body plays chars via
       \message's expanding scan or \@expandtwoargs-style playback).
52. Engine state committed conceptually via BOOT-DEBUG.md (no git repo in /home/leo/dd/tex
    — "git add" fails; consider git init). Regressions: nb/iir/gl2 pass; afl panics;
    boot reads all 22466 lines but \dump is swallowed by the same typeset/par cascade
    at the tail (SGET-START F:18743 = stale line; macros ring = @addtofilelist/fmtversion).
