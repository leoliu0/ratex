//! Repro for utf8.def:335 — filehook IfFileExists + with@hooks sitting
//! after \xdef\@curr@file{\csname...\endcsname}.
use tex_core::engine::Engine;
use tex_core::eqtb::Equiv;

fn boot() -> Engine {
    let mut e = Engine::new(true);
    e.init_primitives();
    e
}

fn run_tex(e: &mut Engine, src: &str) {
    e.input.push_file("t.tex".to_string(), src.as_bytes().to_vec());
    e.run();
}

fn body(e: &Engine, nm: &[u8]) -> String {
    let id = e
        .cs
        .lookup(nm)
        .unwrap_or_else(|| panic!("\\{} missing; term={}", String::from_utf8_lossy(nm), e.term));
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => e.tokens_to_string(&m.body),
        other => panic!(
            "\\{} {:?}",
            String::from_utf8_lossy(nm),
            other.map(|eq| eq.kind_name())
        ),
    }
}

const PRE: &str = r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\@=11
\escapechar=-1
"#;

#[test]
fn xdef_csname_leaves_following_group() {
    let mut e = boot();
    run_tex(
        &mut e,
        &(PRE.to_string()
            + r#"
\xdef\foo{\csname abc\endcsname}
{\gdef\reserved@a{POP}}

"#),
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(body(&e, b"reserved@a"), "POP");
}

#[test]
fn xdef_csname_string_leaves_following_group() {
    let mut e = boot();
    run_tex(
        &mut e,
        &(PRE.to_string()
            + r#"
\def\resolve#1{\expandafter\string\csname n=#1\endcsname}
\xdef\foo{\csname\expandafter\resolve\expandafter{uenc.dfu}\endcsname}
{\gdef\reserved@a{POP}}

"#),
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(body(&e, b"reserved@a"), "POP", "foo={:?}", body(&e, b"foo"));
}

#[test]
fn xdef_ne_sanitize_leaves_withhooks_group() {
    let mut e = boot();
    run_tex(
        &mut e,
        &(PRE.to_string()
            + r#"
\def\identity#1{#1}
\def\expne#1#2{\expandafter#1\expanded{{#2}}}
\def\sanitize#1{\expandafter\string\csname n=#1\endcsname}
\def\resolve#1{\expne\identity{\sanitize{#1}}}
\xdef\foo{\csname\expandafter\resolve\expandafter{uenc.dfu}\endcsname}
\def\@input@file@exists@with@hooks#1{\edef\reserved@a{HOOK#1}}
\@input@file@exists@with@hooks{FILE}
"#),
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(
        body(&e, b"reserved@a"),
        "HOOKFILE",
        "foo={:?} term={}",
        body(&e, b"foo"),
        e.term
    );
}

#[test]
fn xdef_ne_sanitize_leaves_braced_edef() {
    let mut e = boot();
    run_tex(
        &mut e,
        &(PRE.to_string()
            + r#"
\def\identity#1{#1}
\def\expne#1#2{\expandafter#1\expanded{{#2}}}
\def\sanitize#1{\expandafter\string\csname n=#1\endcsname}
\def\resolve#1{\expne\identity{\sanitize{#1}}}
\xdef\foo{\csname\expandafter\resolve\expandafter{uenc.dfu}\endcsname}
{\gdef\reserved@a{POP}}

"#),
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(
        body(&e, b"reserved@a"),
        "POP",
        "foo={:?} term={}",
        body(&e, b"foo"),
        e.term
    );
}

#[test]
fn setcurr_fexpand_leaves_withhooks() {
    // \@filehook@set@CurrentFile: f-expand \@curr@file then parse, withhooks follows
    let mut e = boot();
    run_tex(
        &mut e,
        &(PRE.to_string()
            + r#"
\catcode`\^=7 \catcode`\:=11 \catcode`\_=11
\let\exp:w\romannumeral
\begingroup
\catcode`\^^@=13
\global\protected\def\exp_end_continue_f:w{`^^@}
\gdef^^@{\errmessage{bad f}}
\endgroup
\def\expnf#1#2{\expandafter#1\expandafter{\exp:w\exp_end_continue_f:w#2}}
\def\identity#1{#1}
\def\sanitize#1{\expandafter\string\csname n=#1\endcsname}
\def\parse#1{\expandafter\identity\expanded{{\sanitize{#1}}}}
\def\@curr@file{uenc.dfu}
\expnf\parse{\@curr@file}
\def\@input@file@exists@with@hooks#1{\gdef\reserved@a{HOOK#1}}
\@input@file@exists@with@hooks{FILE}
"#),
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(body(&e, b"reserved@a"), "HOOKFILE");
}

#[test]
fn iffileexists_edef_leaves_true_branch() {
    // latex.ltx:9670 — \\edef\\@filef@und{\\IfFileExists@@{#1}} then #2/#3.
    // Nested \\expanded+\\csname must not steal the true branch.
    let mut e = boot();
    run_tex(
        &mut e,
        &(PRE.to_string()
            + r#"
\def\@empty{}
\long\def\IfFileExists@#1#2#3{\edef\@filef@und{\inner{#1}}\ifx\@filef@und\@empty #3\else #2\fi}
\def\sanitize#1{\expandafter\string\csname n=#1\endcsname}
\def\inner#1{\expanded{{\sanitize{#1}}}}
\IfFileExists@{uenc.dfu}{\gdef\reserved@a{YES}}{\gdef\reserved@a{NO}}
"#),
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(body(&e, b"reserved@a"), "YES");
}



