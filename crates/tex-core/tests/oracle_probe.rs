//! Differential micro-probes: small expl3-idiom files through BOTH the real
//! pdflatex (oracle, -ini -etex) and the Rust engine (INITEX + catcode
//! preamble); compare the file each engine writes via \immediate\write,
//! whitespace-normalized (real TeX pads \write output with cosmetic spaces).
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use tex_core::engine::Engine;

const DIR: &str = "/tmp/oracle_probe";
const OUT: &str = "probe.out";

// Each Engine applies the production process-memory ceiling. Running dozens
// of full INITEX instances in one test process concurrently makes unrelated
// probes consume each other's budget, so differential runs are serialized.
static ORACLE_SERIAL: Mutex<()> = Mutex::new(());

const PRE: &str = r#"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\$=3 \catcode`\^=7 \catcode`\_=11 \catcode`\:=11 \catcode`\@=11
\long\def\use_i:nn#1#2{#1}
\long\def\use_ii:nn#1#2{#2}
\long\def\use_none:n#1{}
\let\if_true:\iftrue \let\if_false:\iffalse \let\else:\else \let\fi:\fi
\def\scan_stop{}
 \let\cs:w\csname \let\cs_end:\endcsname \let\exp_after:wN\expandafter
 \let\if_cs_exist:w\ifcsname \let\if_meaning:w\ifx \let\if_cs_exist:N\ifcsname
 \let\cs_gset:Npn\gdef \protected\def\cs_gset_protected:Npn{\long\gdef}
\immediate\openout15=probe.out
"#;

fn run_mine(dir: &str, src: &str) -> Result<String, String> {
    let out_file = format!("{dir}/{OUT}");
    let _ = std::fs::remove_file(&out_file);
    let mut e = Engine::new(true);
    e.out_dir = format!("{dir}/");
    e.init_primitives();
    e.add_nullfont();
    // Keep INITEX mode: canonical pdfTeX executes output routines there,
    // and format sources legitimately use \patterns.
    e.interaction_mode = tex_core::engine::InteractionMode::Nonstop;
    e.input
        .push_file("probe.tex".to_string(), src.as_bytes().to_vec());
    e.run();
    match std::fs::read_to_string(&out_file) {
        Ok(s) if s.is_empty() => Err(format!("empty {OUT}\nTERM:\n{}", e.term)),
        Ok(s) => Ok(s),
        Err(_) => Err(format!("no {OUT} produced\nTERM:\n{}", e.term)),
    }
}

fn run_oracle(dir: &str, src: &str) -> Result<String, String> {
    if !Path::new("/usr/bin/pdflatex").exists() {
        return Err("no oracle".to_string());
    }
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(format!("{dir}/probe.tex"), src).unwrap();
    let _ = std::fs::remove_file(format!("{dir}/{OUT}"));
    let _ = Command::new("/usr/bin/pdflatex")
        .args([
            "-ini",
            "-etex",
            "-interaction=nonstopmode",
            "-output-directory",
            dir,
            &format!("{dir}/probe.tex"),
        ])
        .current_dir(dir)
        .output();
    match std::fs::read_to_string(format!("{dir}/{OUT}")) {
        Ok(s) => Ok(s),
        Err(_) => Err(format!("no {OUT} (see {dir}/probe.log)")),
    }
}

fn oracle_errors(dir: &str) -> Vec<String> {
    let log = std::fs::read_to_string(format!("{dir}/probe.log")).unwrap_or_default();
    log.lines()
        .filter(|l| l.starts_with("! "))
        .take(6)
        .map(|l| l.to_string())
        .collect()
}

/// content comparison with ALL whitespace removed (write-space padding is
/// cosmetic in both engines)
fn norm(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

fn compare(name: &str, src: &str, require_oracle_clean: bool) {
    let _serial = ORACLE_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = format!("{DIR}_{name}");
    std::fs::create_dir_all(&dir).unwrap();
    let full = format!("{PRE}{src}\n\\closeout15\n\\end\n");
    let m = match run_mine(&dir, &full) {
        Ok(m) => m,
        Err(e) => panic!("{name} mine-fail: {e}"),
    };
    let o = match run_oracle(&dir, &full) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("SKIP {name}: {e}");
            return;
        }
    };
    if require_oracle_clean {
        let errs = oracle_errors(&dir);
        if !errs.is_empty() {
            panic!("{name} oracle had errors:\n{}", errs.join("\n"));
        }
    }
    if norm(&m) != norm(&o) {
        panic!(
            "{name} mismatch\nMINE:   {}\nORACLE: {}",
            m.trim(),
            o.trim()
        );
    }
    eprintln!("OK {name}");
}

fn same(name: &str, src: &str) {
    compare(name, src, true);
}

#[test]
fn probe_def_param_conditional() {
    same(
        "def-param-conditional",
        r#"\long\def\zzb \fi: \use_none:n #1 \if_false:
  { BODY #1 \else: }
\immediate\write15{ZZB:[\meaning\zzb]}
"#,
    );
}

#[test]
fn probe_use_ii_edef() {
    same(
        "use-ii-edef",
        r#"\edef\t{\if_true:\use_ii:nn {\noexpand\a}{\noexpand\b}\else:\fi}
\edef\u{\if_false:\use_ii:nn {\noexpand\a}{\noexpand\b}\else:KEEP\fi}
\immediate\write15{T:[\meaning\t]}
\immediate\write15{U:[\meaning\u]}
"#,
    );
}

#[test]
fn probe_hash_eol_brace() {
    same(
        "hash-eol-brace",
        r#"\catcode 32 = 9
\endlinechar = 32
\long\def\parm#1#2#3#4#
  { GOT:#1/#2/#3/#4 }
\immediate\write15{DEF:[\meaning\parm]}
\immediate\write15{\parm a b c {d e} tail}
"#,
    );
}

#[test]
fn probe_csname_endcsname_group() {
    same(
        "csname-group",
        r#"\expandafter\def\csname my:cs\endcsname#1{V:[#1]}
\immediate\write15{\csname my:cs\endcsname hello}
\immediate\write15{[EXISTS \expandafter\ifx\csname my:cs\endcsname\relax NO\else YES\fi]}
"#,
    );
}

#[test]
fn probe_expanded_protected_error() {
    // \expanded of a protected macro: error, nothing expands (TeXbook)
    compare(
        "expanded-protected",
        r#"\protected\def\pt{P}
\edef\x{a\expanded{b\pt c}d}
\immediate\write15{X:[\meaning\x]}
"#,
        false,
    );
}

#[test]
fn probe_detokenize_braces() {
    compare(
        "detokenize-braces",
        r#"\edef\d{\detokenize{ab{c}d}}
\immediate\write15{D:[\meaning\d]}
"#,
        false,
    );
}

#[test]
fn probe_scantokens_group() {
    compare(
        "scantokens-group",
        r#"\begingroup
\scantokens{\catcode`\Q=11 xQy}
\endgroup
\immediate\write15{AFTER:[\meaning\xQy]}
"#,
        false,
    );
}

#[test]
fn probe_let_brace_alias() {
    compare(
        "let-brace",
        r#"\let\beg\{
\def\test{a\beg b}
\immediate\write15{T:[\meaning\test]}
"#,
        false,
    );
}

#[test]
fn probe_edef_cond_brace_skew() {
    compare(
        "edef-cond-brace-skew",
        r#"\edef\y{\if_false: { \else: OK}\fi}
\immediate\write15{Y:[\meaning\y]}
"#,
        false,
    );
}

#[test]
fn probe_def_body_cond() {
    // conditional inside a \def BODY (not expanded): stored verbatim
    compare(
        "def-body-cond",
        r#"\def\w{\if_false: { \else: OK}\fi}
\immediate\write15{W:[\meaning\w]}
"#,
        false,
    );
}

#[test]
fn probe_auxw_call() {
    // the stored aux-macro shape + direct call (bypassing prg_gset stubs)
    compare(
        "auxw-call",
        r#"\long\def\__cs_if_exist_c_aux:w \fi: \use_none:n #1 \if_false:
  { \fi: \exp_after:wN \if_meaning:w \cs:w #1 \cs_end: \scan_stop: \else: }
\def\zzz{X}
\immediate\write15{AUX:[\meaning\__cs_if_exist_c_aux:w]}
% emulate the caller context exactly: an open \ifcs-true conditional whose
% true text is the aux call, terminated by \else:/\fi:
\immediate\write15{C1:[\if_cs_exist:w zzz \cs_end:
  \__cs_if_exist_c_aux:w
\fi:
\use_none:n {zzz}
\if_false:
  {TRUE-ARM}
\else:
  {FALSE-ARM}
\fi]}
\immediate\write15{C2:[\if_cs_exist:w qqq \cs_end:
  \__cs_if_exist_c_aux:w
\fi:
\use_none:n {qqq}
\if_false:
  {TRUE-ARM}
\else:
  {FALSE-ARM}
\fi]}
"#,
        false,
    );
}
#[test]
fn probe_end_column_cond() {
    compare(
        "end-col-cond",
        r#"
\def\@boolean#1#2{%
  \long\def#1{%
    #2%
      \expandafter\true@sw
    \else
      \expandafter\false@sw
    \fi
  }%
}
\def\@boole@def#1#{\@boolean{#1}}
\long\def\true@sw#1#2{#1}
\long\def\false@sw#1#2{#2}
\@boole@def\@ifx#1{\ifx#1}
\@boole@def\@ifx@empty#1{\ifx\@empty#1}
\@boole@def\@ifnum#1{\ifnum#1}
\def\@empty{}
\gdef\@toplist{}
\gdef\@botlist{}
\gdef\@dbltoplist{}
\gdef\@deferlist{}
\chardef\@ne=1
\countdef\pagegrid@cur=100
\pagegrid@cur=1

\def\test{%
 \@ifx@empty\@toplist{%
  \@ifx@empty\@botlist{%
   \@ifx@empty\@dbltoplist{%
    \@ifx@empty\@deferlist{%
     \@ifnum{\pagegrid@cur=\@ne}{%
      \false@sw
     }{%
      \true@sw
     }%
    }{%
     \true@sw
    }%
   }{%
    \true@sw
   }%
  }{%
   \true@sw
  }%
 }{%
  \true@sw
 }%
 {TRUE}%
 {FALSE}%
}
\immediate\write15{\test}
\end
 "#,
        true,
    );
}

#[test]
fn probe_insert_before_first_box() {
    same(
        "insert-before-first-box",
        r#"
\vsize=100pt \hsize=100pt \maxdepth=4pt \topskip=10pt
\count100=1000 \dimen100=100pt \skip100=5pt
\insert100{\hrule height10pt}
\vskip20pt \kern7pt \penalty100
\hrule height3pt depth2pt
\penalty10000
\immediate\write15{FIRST: \the\pagegoal / \the\pagetotal / \the\pagedepth}
\vskip4pt
\hrule height5pt depth1pt
\penalty10000
\immediate\write15{SECOND: \the\pagegoal / \the\pagetotal / \the\pagedepth}
"#,
    );
}

#[test]
fn probe_hyphenation_replaces_cross_letter_kern() {
    same(
        "hyphenation-cross-letter-kern",
        r#"
\font\testfont=ptmri7t at 10pt \testfont \hyphenchar\testfont=45
\hyphenation{en-vironment}
\lefthyphenmin=2 \righthyphenmin=2
\pretolerance=-1 \tolerance=10000 \hbadness=10000
\parindent=0pt \parfillskip=0pt plus 1fil
\setbox0=\vbox{\hsize=14pt \noindent\hskip0pt environment\par
\setbox1=\lastbox \unskip\unpenalty
\setbox2=\lastbox \setbox3=\hbox{\unhbox2}
\immediate\write15{FIRST-LINE-WIDTH: \the\wd3}}
"#,
    );
}

#[test]
fn probe_discretionary_and_final_hyphen_penalties() {
    same(
        "discretionary-final-hyphen-penalties",
        r#"
\font\testfont=cmr10 at 10pt \testfont
\parindent=0pt \parfillskip=0pt plus 1fil
\pretolerance=-1 \tolerance=10000 \linepenalty=10
\hyphenpenalty=50 \exhyphenpenalty=10000
\adjdemerits=0 \doublehyphendemerits=0 \emergencystretch=0pt
\spaceskip=3.33333pt plus 1.66666pt minus 1.11111pt
\def\measure#1{\finalhyphendemerits=#1
\setbox0=\vbox{\hsize=60pt
aaa aaa aaa aaa aaa aaa aaa aaa aaaa\discretionary{-}{}{}aaaa\par
\setbox1=\lastbox \setbox2=\hbox{\unhbox1}
\immediate\write15{FINAL-LINE-#1: \the\wd2}}}
\measure{0}
\measure{100000000}
"#,
    );
}

#[test]
fn probe_explicit_hyphen_before_noncharacter() {
    same(
        "explicit-hyphen-before-noncharacter",
        r#"
\font\testfont=cmr10 at 10pt \hyphenchar\testfont=45 \testfont
\hsize=30pt \parindent=0pt \parfillskip=0pt plus 1fil
\pretolerance=10000 \tolerance=10000 \hbadness=10000
\setbox0=\vbox{\noindent AAAA-\hbox{Z}BBBB\par
\global\count0=\prevgraf}
\immediate\write15{BOX-LINES: \the\count0}
\setbox0=\vbox{\noindent AAAA-ZBBBB\par
\global\count0=\prevgraf}
\immediate\write15{CHAR-LINES: \the\count0}
"#,
    );
}

#[test]
fn probe_hyphenation_requires_glue_lookahead() {
    same(
        "hyphenation-glue-lookahead",
        r#"
\font\testfont=ptmri7t at 10pt \testfont \hyphenchar\testfont=45
\hyphenation{en-vironment}
\lefthyphenmin=2 \righthyphenmin=2
\pretolerance=-1 \tolerance=10000 \hbadness=10000
\parindent=0pt \parfillskip=0pt plus 1fil
\def\measure#1#2{\setbox0=\vbox{\hsize=14pt
\noindent#2environment\par
\count0=\prevgraf
\immediate\write15{#1: \the\count0}}}
\measure{INITIAL}{}
\measure{BOX}{\hbox{}}
\measure{GLUE}{\hskip0pt}
\measure{PUNCTUATION}{\hskip0pt(}
"#,
    );
}

#[test]
fn probe_font_expansion_retains_rounding_badness() {
    same(
        "font-expansion-rounding-badness",
        r#"
\font\testfont=cmr10 at 10pt \testfont \hyphenchar\testfont=45
\pdfadjustspacing=2 \pdffontexpand\testfont 20 20 1 autoexpand
\hyphenation{per-sistence}
\lefthyphenmin=2 \righthyphenmin=2
\linepenalty=10 \hyphenpenalty=50 \adjdemerits=10000
\doublehyphendemerits=10000 \finalhyphendemerits=5000
\pretolerance=-1 \tolerance=10000 \hbadness=10000
\parindent=0pt \parfillskip=0pt plus 1fil
\leftskip=0pt plus 1fil \rightskip=0pt plus 1fil
\spaceskip=3.33333pt
\setbox0=\hbox{and persistence}\hsize=\wd0 \advance\hsize by -0.5pt
\setbox0=\vbox{and persistence\par \count0=\prevgraf
\immediate\write15{EXPANSION-LINES: \the\count0}}
"#,
    );
}

#[test]
fn probe_sentence_factor_scales_spaceskip() {
    same(
        "sentence-factor-scales-spaceskip",
        r#"
\font\testfont=cmr10 at 10pt \testfont
\spaceskip=3pt
\sfcode`\.=3000
\setbox0=\hbox{a. b}
\immediate\write15{SENTENCE-SPACE-BOX: \the\wd0}
"#,
    );
}

#[test]
fn probe_explicit_discretionary_preserves_following_text() {
    same(
        "explicit-discretionary-following-text",
        r#"
\font\testfont=cmr10 at 10pt \testfont
\hsize=100pt \parindent=0pt \parfillskip=0pt plus 1fil
\pretolerance=10000 \tolerance=10000 \hbadness=10000
\setbox0=\vbox{\discretionary{}{}{AAAA} AAAA AAAA\par
\global\setbox1=\lastbox}
\setbox2=\hbox{\unhbox1}
\immediate\write15{NATURAL-LINE-WIDTH: \the\wd2}
\pdfadjustspacing=2 \pdffontexpand\testfont 20 20 1 autoexpand
\hsize=97pt \parfillskip=0pt
\setbox0=\vbox{\discretionary{}{}{AAAA} AAAA AAAA\par
\global\setbox1=\lastbox}
\setbox2=\hbox{\unhbox1}
\count0=\wd2 \divide\count0 by655
\immediate\write15{EXPANDED-LINE-HUNDREDTHS: \the\count0}
"#,
    );
}

#[test]
fn probe_hangindent_line_shapes() {
    // tex.web §25121–25148/§26030–26036: with \parshape empty and
    // \hangindent nonzero, lines <=|>|hang_after| (hangafter>=0) switch to
    // width hsize-|hangindent| and shift hangindent (0 when negative).
    // Before the port the shape was ignored: every line kept full hsize.
    same(
        "hangindent-line-shapes",
        r#"
\font\testfont=cmr10 at 10pt \testfont
\parindent=0pt \parfillskip=0pt plus 1fil
\pretolerance=-1 \tolerance=10000 \hbadness=10000
\def\measure#1#2#3{%
\setbox0=\vbox{\hsize=100pt \hangindent=#1 \hangafter=#2
wordA wordB wordC wordD wordE wordF wordG wordH wordI\par
\setbox1=\lastbox \unskip\unpenalty
\setbox2=\lastbox \unskip\unpenalty
\setbox3=\lastbox
\immediate\write15{#3: \the\wd3 /\the\wd2 /\the\wd1}}}%
\measure{0pt}{1}{PLAIN}
\measure{20pt}{2}{POS-AFTER-2}
\measure{20pt}{-1}{NEG-AFTER}
\measure{-20pt}{2}{NEG-INDENT}
\measure{30pt}{0}{AFTER-ZERO}
\hangindent=20pt \hangafter=2 \looseness=3
\setbox0=\vbox{}
\immediate\write15{AFTER-VBOX: \the\hangindent /\the\hangafter /\the\looseness}
\insert0{}
\immediate\write15{AFTER-INSERT: \the\hangindent /\the\hangafter /\the\looseness}
"#,
    );
}

#[test]
fn probe_unboxing_preserves_nest_prevdepth() {
    same(
        "unboxing-preserves-nest-prevdepth",
        r#"
\hsize=100pt \baselineskip=12pt
\setbox0=\vbox{\hbox{\vrule height8pt depth5pt width1pt}}
\setbox1=\vbox{\hbox{\vrule height10pt depth3pt width1pt}
\unvbox0
\dimen0=\prevdepth
\immediate\write15{PRESERVED-PREVDEPTH: \the\dimen0}
\hbox{\vrule height4pt depth0pt width1pt}
\setbox2=\lastbox
\skip0=\lastskip
\immediate\write15{FOLLOWING-BASELINE-GLUE: \the\skip0}}
"#,
    );
}

#[test]
fn probe_vsplit_respects_forced_break_and_rule_height() {
    same(
        "vsplit-forced-break-and-rule-height",
        r#"
\splittopskip=0pt
\setbox0=\vbox{\hrule height10pt width1pt\penalty-10000
\hrule height20pt width1pt}
\setbox1=\vsplit0 to100pt
\immediate\write15{FORCED-REMAINDER: \the\ht0}
\setbox0=\vbox{\hrule height10pt width1pt\vskip0pt
\hrule height20pt width1pt\vskip0pt\hrule height30pt width1pt}
\setbox1=\vsplit0 to10pt
\immediate\write15{RULE-REMAINDER: \the\ht0}
"#,
    );
}

#[test]
fn probe_vsplit_depth_and_remainder_topskip() {
    same(
        "vsplit-depth-and-remainder-topskip",
        r#"
\splittopskip=0pt \splitmaxdepth=100pt
\setbox0=\vbox{\hbox{\vrule height10pt depth5pt width1pt}\penalty0
\hbox{\vrule height10pt depth0pt width1pt}}
\setbox1=\vsplit0 to10pt
\immediate\write15{TRAILING-DEPTH: \the\dp1; REST: \the\ht0}
\setbox0=\vbox{\kern-10pt\hrule height10pt width1pt\penalty-10000
\hrule height20pt width1pt}
\setbox1=\vsplit0 to0pt
\immediate\write15{ZERO-TARGET-REMAINDER: \the\ht0}
\splittopskip=10pt plus2pt
\setbox0=\vbox{\hrule height1pt width1pt\penalty-10000
\mark{saved}\kern7pt\vskip2pt\hrule height3pt width1pt}
\setbox1=\vsplit0 to1pt
\immediate\write15{PADDED-REMAINDER: \the\ht0}
\setbox1=\vsplit0 to10pt
\immediate\write15{PRESERVED-MARK: \splitfirstmark}
"#,
    );
}

#[test]
fn probe_deferred_log_write_preserves_vertical_breakpoint() {
    same(
        "deferred-log-write-vertical-breakpoint",
        r#"
\splittopskip=0pt
\setbox0=\vbox{\write-1{}\vskip12pt\hbox{}}
\setbox1=\vsplit0 to0pt
\setbox1=\vbox{\unvbox1}
\immediate\write15{SENTINEL-PREFIX-HEIGHT: \the\ht1}
"#,
    );
}

#[test]
fn probe_deferred_stream_operations_execute_in_shipout_order() {
    same(
        "deferred-stream-operation-order",
        r#"
\setbox0=\vbox{\openout14=ordered.tmp
\write14{OPEN-WRITE-CLOSE}
\closeout14}
\shipout\box0
\openin14=ordered.tmp
\read14 to\got
\closein14
\immediate\write15{ORDER:[\got]}
"#,
    );
}

#[test]
fn probe_end_forces_residual_whatsits_through_page_builder() {
    same(
        "end-residual-whatsits-page-builder",
        r#"
\hsize=40pt \vsize=20pt
\count0=0
\output={\global\advance\count0 by1
\immediate\write15{FINAL-BOX-\the\count0: \the\ht255/\the\dp255}
\shipout\box255}
\hbox{x}\penalty-10000
\write-1{}\write-1{}
"#,
    );
}

#[test]
fn probe_box_register_consumption_preserves_assignment_scope() {
    same(
        "box-register-consumption-scope",
        r#"
\setbox0=\hbox{OUTER}
\begingroup
  \setbox0=\hbox{INNER}
  \box0
\endgroup
\ifvoid0
  \immediate\write15{BOX-LOCAL: VOID}
\else
  \immediate\write15{BOX-LOCAL: PRESENT}
\fi

\setbox1=\hbox{OUTER}
\setbox9=\hbox{%
  \begingroup
    \setbox1=\hbox{INNER}
    \unhbox1
  \endgroup}
\ifvoid1
  \immediate\write15{UNHBOX-LOCAL: VOID}
\else
  \immediate\write15{UNHBOX-LOCAL: PRESENT}
\fi

\setbox2=\vbox{\hbox{OUTER}}
\begingroup
  \setbox9=\vsplit2 to100pt
\endgroup
\ifvoid2
  \immediate\write15{VSPLIT-OUTER: VOID}
\else
  \immediate\write15{VSPLIT-OUTER: PRESENT}
\fi

\setbox3=\vbox{\hbox{OUTER}}
\begingroup
  \setbox3=\vbox{\hbox{INNER}}
  \setbox9=\vsplit3 to100pt
\endgroup
\ifvoid3
  \immediate\write15{VSPLIT-LOCAL: VOID}
\else
  \immediate\write15{VSPLIT-LOCAL: PRESENT}
\fi
"#,
    );
}

#[test]
fn probe_output_reads_completed_page_dimensions() {
    same(
        "output-completed-page-dimensions",
        r#"
\hsize=100pt \vsize=30pt \maxdepth=4pt
\baselineskip=12pt \lineskip=0pt \lineskiplimit=0pt
\output={\ifnum\count0=0 \immediate\write15{OUTPUT-GOAL: \the\pagegoal;
 TOTAL: \the\pagetotal; DEPTH: \the\pagedepth;
 STRETCH: \the\pagestretch; SHRINK: \the\pageshrink;
 BOX: \the\ht255/\the\dp255}
\fi \global\advance\count0 by1
\setbox0=\box255 \global\deadcycles=0}
\hbox{\vrule height10pt depth2pt width1pt}
\vskip3pt plus4pt minus1pt
\hbox{\vrule height5pt depth1pt width1pt}
\penalty-10000
"#,
    );
}

#[test]
fn probe_page_builder_fires_before_display_tokens_run() {
    same(
        "page-builder-before-display",
        r#"
\font\ten=cmr10 \textfont0=\ten \ten
\font\teni=cmmi10 \font\tensy=cmsy10 \font\tenex=cmex10
\textfont1=\teni
\textfont2=\tensy \scriptfont2=\tensy \scriptscriptfont2=\tensy
\textfont3=\tenex \scriptfont3=\tenex \scriptscriptfont3=\tenex
\hsize=40pt \vsize=18pt \baselineskip=12pt
\output={\ifnum\count11=0
\immediate\write15{FIRST-OUTPUT-MARK: \the\count10}\fi
\global\advance\count11 by1 \setbox0=\box255 \global\deadcycles=0}
\count10=0 \count11=0
\everydisplay={\global\count10=1}
A A A A A A A A A A A A A A A A A A A A $$x$$
\immediate\write15{AFTER: \the\count10}
"#,
    );
}

#[test]
fn probe_display_output_preserves_reinserted_page_material() {
    same(
        "display-output-preserves-reinserted-material",
        r#"
\font\ten=cmr10 \textfont0=\ten \ten
\font\teni=cmmi10 \font\tensy=cmsy10 \font\tenex=cmex10
\textfont1=\teni
\textfont2=\tensy \scriptfont2=\tensy \scriptscriptfont2=\tensy
\textfont3=\tenex \scriptfont3=\tenex \scriptscriptfont3=\tenex
\hsize=40pt \vsize=18pt \baselineskip=12pt
\count10=0
\output={\global\advance\count10 by1
  \ifnum\count10=1
    \global\vsize=1000pt \unvbox255 \penalty\outputpenalty
  \else
    \shipout\box255
  \fi
  \global\deadcycles=0}
\write15{KEPT-WRITE}
A A A A A A A A A A A A A A A A A A A A $$x$$
\immediate\write15{AFTER-DISPLAY: \the\count10}
"#,
    );
}

#[test]
fn probe_display_continuation_preserves_parshape_line_numbers() {
    same(
        "display-continuation-parshape",
        r#"
\input plain
\setbox0=\vbox{
\hsize=90pt
\parshape=6 0pt30pt 0pt40pt 0pt50pt 0pt60pt 0pt70pt 0pt80pt
\everydisplay={\edef\displaymeasure{\the\displaywidth}
\immediate\write15{DISPLAY-WIDTH: \displaymeasure}}
\noindent A$$x$$B$$y$$C\par
\edef\paragraphmeasure{\the\prevgraf}
\immediate\write15{PARAGRAPH-LINES: \paragraphmeasure}
\setbox1=\lastbox
\immediate\write15{LAST-LINE-WIDTH: \the\wd1}
}
"#,
    );
}

#[test]
fn probe_prevgraf_follows_enclosing_vertical_nest() {
    same(
        "prevgraf-enclosing-vertical-nest",
        r#"
\input plain
\prevgraf=9
\setbox0=\vbox{
\xdef\innerstart{\the\prevgraf}
\noindent A$$x$$B\par
\xdef\innerend{\the\prevgraf}}
\edef\outerbefore{\the\prevgraf}
\setbox0=\hbox{\xdef\horizontalread{\the\prevgraf}\prevgraf=11}
\edef\outerafter{\the\prevgraf}
\setbox0=\vbox{\prevgraf=7
\halign{#\cr\setbox1=\vbox{\noindent A$$x$$B\par}a\cr}
\xdef\afteralignment{\the\prevgraf}}
\immediate\write15{INNER: \innerstart/\innerend;
OUTER: \outerbefore/\horizontalread/\outerafter;
ALIGNMENT: \afteralignment}
\vsize=30pt
\output={\xdef\outputcounter{\the\prevgraf}\prevgraf=17
\setbox0=\box255\global\deadcycles=0}
\hbox{\vrule height5pt depth0pt width1pt}\penalty-10000
\edef\restoredcounter{\the\prevgraf}
\immediate\write15{OUTPUT: \outputcounter; RESTORED: \restoredcounter}
"#,
    );
}

#[test]
fn probe_output_reinsertion_preserves_break_penalty() {
    same(
        "output-reinsertion-break-penalty",
        r#"
\hsize=100pt \vsize=30pt
\output={\ifnum\count0=0 \unvbox255\vskip5pt
\else\setbox0=\box255\fi
\global\advance\count0 by1 \global\deadcycles=0}
\hbox{\vrule height10pt depth2pt width1pt}
\penalty-10003
\edef\breakstate{LAST-PENALTY: \the\lastpenalty; LAST-SKIP: \the\lastskip}
\immediate\write15{\breakstate}
"#,
    );
}

#[test]
fn probe_trailing_insert_preserves_paragraph_depth() {
    same(
        "trailing-insert-paragraph-depth",
        r#"
\hsize=100pt \vsize=100pt
\setbox0=\vbox{
\prevdepth=7pt
\noindent\hbox{\vrule height5pt depth3pt width1pt}
\insert3{\hbox{\vrule height2pt depth1pt width1pt}}\par
\dimen0=\prevdepth
\immediate\write15{INNER-DEPTH: \the\dimen0}}
\prevdepth=7pt
\noindent\hbox{\vrule height5pt depth3pt width1pt}
\insert3{\hbox{\vrule height2pt depth1pt width1pt}}\par
\dimen0=\prevdepth
\immediate\write15{OUTER-DEPTH: \the\dimen0}
"#,
    );
}

// A multi-line insertion that fits `\dimen n` but NOT the remaining page
// space must split at page room (tex.web §19612-19679), not defer whole or
// place whole. Old code subtracted the full height and only split to the
// class budget; the oracle splits across pages 1-2.
#[test]
fn probe_insertion_splits_to_page_space_not_dimen() {
    same(
        "insertion-page-space-split",
        r#"
\hsize=200pt \vsize=100pt \topskip=0pt \maxdepth=10pt
\baselineskip=0pt \lineskip=0pt \lineskiplimit=0pt
\count10=1000 \dimen10=250pt \skip10=0pt
\splittopskip=0pt \splitmaxdepth=20pt
\output={\global\advance\count1 by1
\immediate\write15{PAGE\the\count1: INS\the\ht10/\the\dp10 PEN\the\insertpenalties TOT\the\pagetotal GOAL\the\pagegoal}
\setbox10=\vbox{}\shipout\box255\global\deadcycles=0}
\hbox{\vrule height70pt depth0pt width50pt}
\insert10{\hbox{\vrule height12pt width50pt}\vskip2pt\hbox{\vrule height12pt width50pt}\vskip2pt\hbox{\vrule height12pt width50pt}\vskip2pt\hbox{\vrule height12pt width50pt}\vskip2pt\hbox{\vrule height12pt width50pt}}

\hbox{\vrule height5pt depth0pt width50pt}
\penalty-10000

\hbox{\vrule height20pt depth0pt width50pt}
"#,
    );
}

// tex.web §22507/§12956/§22611: an \insert inside a displayed equation is
// migrated out of the formula hlist by the display's hpack and spliced into
// the vertical list after the display box. Without the migration the insert
// node is trapped inside the equation box, never reaches the page builder,
// and the class box ships empty (document2609.05162 footnote 42).
#[test]
fn probe_display_insert_migrates_to_page() {
    same(
        "display-insert-migrates",
        r#"
\font\ten=cmr10 \textfont0=\ten \ten
\font\teni=cmmi10 \font\tensy=cmsy10 \font\tenex=cmex10
\textfont1=\teni
\textfont2=\tensy \scriptfont2=\tensy \scriptscriptfont2=\tensy
\textfont3=\tenex \scriptfont3=\tenex \scriptscriptfont3=\tenex
\hsize=200pt \vsize=200pt \topskip=0pt \maxdepth=10pt
\baselineskip=0pt \lineskip=0pt \lineskiplimit=0pt
\count10=1000 \dimen10=200pt \skip10=0pt
\output={\global\advance\count1 by1
\immediate\write15{PG\the\count1: HT10\the\ht10/\the\dp10 IP\the\insertpenalties}
\setbox10=\vbox{}\shipout\box255\global\deadcycles=0}
\hbox{\vrule height5pt width20pt}
$$a\insert10{\vbox{\hbox{\vrule height4pt width50pt}}}b$$\par
\hbox{\vrule height10pt width20pt}
\penalty-10000
\hbox{\vrule height20pt width20pt}
"#,
    );
}
