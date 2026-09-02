//! Differential micro-probes: small expl3-idiom files through BOTH the real
//! pdflatex (oracle, -ini -etex) and the Rust engine (INITEX + catcode
//! preamble); compare the file each engine writes via \immediate\write,
//! whitespace-normalized (real TeX pads \write output with cosmetic spaces).
use std::path::Path;
use std::process::Command;
use tex_core::engine::Engine;

const DIR: &str = "/tmp/oracle_probe";
const OUT: &str = "probe.out";

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
    e.input
        .push_file("probe.tex".to_string(), src.as_bytes().to_vec());
    e.run();
    match std::fs::read_to_string(&out_file) {
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
    compare("expanded-protected", r#"\protected\def\pt{P}
\edef\x{a\expanded{b\pt c}d}
\immediate\write15{X:[\meaning\x]}
"#, false);
}

#[test]
fn probe_detokenize_braces() {
    compare("detokenize-braces", r#"\edef\d{\detokenize{ab{c}d}}
\immediate\write15{D:[\meaning\d]}
"#, false);
}

#[test]
fn probe_scantokens_group() {
    compare("scantokens-group", r#"\begingroup
\scantokens{\catcode`\Q=11 xQy}
\endgroup
\immediate\write15{AFTER:[\meaning\xQy]}
"#, false);
}

#[test]
fn probe_let_brace_alias() {
    compare("let-brace", r#"\let\beg\{
\def\test{a\beg b}
\immediate\write15{T:[\meaning\test]}
"#, false);
}

#[test]
fn probe_edef_cond_brace_skew() {
    compare("edef-cond-brace-skew", r#"\edef\y{\if_false: { \else: OK}\fi}
\immediate\write15{Y:[\meaning\y]}
"#, false);
}

#[test]
fn probe_def_body_cond() {
    // conditional inside a \def BODY (not expanded): stored verbatim
    compare("def-body-cond", r#"\def\w{\if_false: { \else: OK}\fi}
\immediate\write15{W:[\meaning\w]}
"#, false);
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
fn probe_tl_item_loop() {
    // expl3-code.tex 4645-4670 verbatim shapes: f-recursion terminated by
    // \prg_break:n raw-grabbing to \prg_break_point:
    compare("tl-item-loop", r#"\def\prg_do_nothing:{}
\cs_new_eq_placeholder
\def\q__tl_recursion_tail{\q__tl_recursion_tail}
\let\prg_break_point:\prg_do_nothing:
\long\def\prg_break:n #1#2 \prg_break_point: {#1}
\def\prg_break: #1 \prg_break_point: { }
\def\int_eval:n ##1{\number\dimexpr0 ##1\relax}
\long\def\__tl_if_recursion_tail_break:nN #1#2
  {\exp_args:Nf \__tl_if_recursion_tail_break_test:nN
     { \tl_to_str:n {#1} } #2}
\def\__tl_if_recursion_tail_break_test:nN #1#2
  {\exp_args:No \__tl_if_recursion_tail_break_test_aux:nN
     { \str_length:w #1 \s__test } #2}
% too deep: just the essential call chain
\def\__tl_item:nn #1#2
  {
    \__tl_if_recursion_tail_break:nN {#2} \prg_break:
    \if_num:w #1 = 1
      { \prg_break:n { \unexpanded {#2} } }
    \else:
      { \exp_args:Nf \__tl_item:nn { \number\numexpr #1 - 1 \relax } }
    \fi:
    #2
    \q__tl_recursion_tail
    \prg_break_point:
  }
\immediate\write15{IT1:[\__tl_item:nn{1}{a,b,c}]}
"#, false);
}

// real \prg_gset_conditional:Npnn from expl3 (stubs are NOT real). verbatim:
// expl3-code.tex 1656-1657, 1666-1674, 1708-1731, 1751-1787 region defs
// are too many; instead use the REAL file: \input latex.ltx then run the call?
// latex.ltx boot is what we're debugging — so use the SYSTEM LaTeX format via
// a doc-mode probe separately: see probe_auxw_call_latex (ignored without
// latex; parent runs manually).

#[test]
fn probe_expanded_conditional_arms() {
    same("expanded-cond-arms", r#"\immediate\write15{E1:[\expanded{\if_false:{X}\else:{Y}\fi}]}
\immediate\write15{E2:[\expanded{\if_true:{P}\if_false:{{\else:{Q}}}\fi}]}
\immediate\write15{E3:[\expanded{\if_false:{{\else:{R}}}\fi}]}
\edef\E{\expanded{\if_false:{ \else: OK}\fi}}
\immediate\write15{E4:[\meaning\E]}
"#);
}
