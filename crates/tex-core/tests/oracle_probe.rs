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
\let\if_cs_exist:w\ifcs \let\if_meaning:w\ifx \let\if_cs_exist:N\ifcs
\let\cs_gset:Npn\gdef \protected\def\cs_gset_protected:Npn{\long\gdef}
\immediate\openout15=probe.out
"#;

fn run_mine(src: &str) -> Result<String, String> {
    std::env::set_current_dir(DIR).ok();
    let _ = std::fs::remove_file(OUT);
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.input
        .push_file("probe.tex".to_string(), src.as_bytes().to_vec());
    e.run();
    match std::fs::read_to_string(OUT) {
        Ok(s) => Ok(s),
        Err(_) => Err(format!("no {OUT} produced\nTERM:\n{}", e.term)),
    }
}

fn run_oracle(src: &str) -> Result<String, String> {
    if !Path::new("/usr/bin/pdflatex").exists() {
        return Err("no oracle".to_string());
    }
    std::fs::create_dir_all(DIR).unwrap();
    std::fs::write(format!("{DIR}/probe.tex"), src).unwrap();
    let _ = std::fs::remove_file(format!("{DIR}/{OUT}"));
    let _ = Command::new("/usr/bin/pdflatex")
        .args([
            "-ini",
            "-etex",
            "-interaction=nonstopmode",
            "-output-directory",
            DIR,
            &format!("{DIR}/probe.tex"),
        ])
        .current_dir(DIR)
        .output();
    match std::fs::read_to_string(format!("{DIR}/{OUT}")) {
        Ok(s) => Ok(s),
        Err(_) => Err(format!("no {OUT} (see {DIR}/probe.log)")),
    }
}

fn oracle_errors() -> Vec<String> {
    let log = std::fs::read_to_string(format!("{DIR}/probe.log")).unwrap_or_default();
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
    let full = format!("{PRE}{src}\n\\closeout15\n\\end\n");
    let m = match run_mine(&full) {
        Ok(m) => m,
        Err(e) => panic!("{name} mine-fail: {e}"),
    };
    let o = match run_oracle(&full) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("SKIP {name}: {e}");
            return;
        }
    };
    if require_oracle_clean {
        let errs = oracle_errors();
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

// real \prg_gset_conditional:Npnn from expl3 (stubs are NOT real). verbatim:
// expl3-code.tex 1656-1657, 1666-1674, 1708-1731, 1751-1787 region defs
// are too many; instead use the REAL file: \input latex.ltx then run the call?
// latex.ltx boot is what we're debugging — so use the SYSTEM LaTeX format via
// a doc-mode probe separately: see probe_auxw_call_latex (ignored without
// latex; parent runs manually).
