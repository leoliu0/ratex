//! Regression tests for expl3 bootstrap tricks that latex.ltx depends on.
use tex_core::engine::Engine;
use tex_core::eqtb::Equiv;
use tex_core::prim::Prim;

fn boot() -> Engine {
    let mut e = Engine::new(true);
    e.init_primitives();
    e
}

fn run_tex(e: &mut Engine, src: &str) {
    e.input.push_file("t.tex".to_string(), src.as_bytes().to_vec());
    e.run();
}

fn meaning_prim(e: &Engine, name: &[u8]) -> Option<Prim> {
    let id = e.cs.lookup(name)?;
    match e.eqtb.resolve(id) {
        Some(Equiv::Prim(p)) => Some(*p),
        _ => None,
    }
}

#[test]
fn let_global_primitive_copy_survives_group() {
    // expl3-code.tex:275-279 — \let\tex_let:D\let then
    // \begingroup \def\__kernel_primitive:NN#1#2{\tex_global:D\tex_let:D#2#1}
    // \__kernel_primitive:NN\iffalse\tex_iffalse:D \endgroup
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\catcode`\:=11 \catcode`\_=11
\let\tex_global:D\global
\let\tex_let:D\let
\begingroup
  \long\def\__kernel_primitive:NN#1#2{\tex_global:D\tex_let:D#2#1}
  \__kernel_primitive:NN\iffalse\tex_iffalse:D
  \__kernel_primitive:NN\iftrue\tex_iftrue:D
\endgroup
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(
        meaning_prim(&e, b"tex_iffalse:D"),
        Some(Prim::IfFalse),
        "tex_iffalse:D must survive the group as \\iffalse"
    );
    assert_eq!(
        meaning_prim(&e, b"tex_iftrue:D"),
        Some(Prim::IfTrue),
        "tex_iftrue:D must survive the group as \\iftrue"
    );
}

#[test]
fn simple_def_no_group_leak() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\def\foo{bar}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(
        e.eqtb.cur_level, 1,
        "simple \\def leaked groups; term={}",
        e.term
    );
}

#[test]
fn nul_begingroup_in_def_body() {
    // expl3-code.tex:25635-25642 — catcode 0 = begin-group, then
    // \def\foo{\x{\expandafter^^@\iffalse}\fi}}
    // The extra `}` closes the ^^@ begin-group so \iffalse stays in the body.
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\^=7
\catcode0=1
\def\foo{\x{\expandafter^^@\iffalse}\fi}}
\catcode0=12
\def\marker{ok}
        "#,
    );
    eprintln!(
        "nul_def term={:?} level={} errors={}",
        e.term, e.eqtb.cur_level, e.error_count
    );
    let foo = e.cs.lookup(b"foo").expect("\\foo defined");
    match e.eqtb.resolve(foo) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            eprintln!("nul_def body={body:?} nparams={}", m.num_params);
            assert!(
                body.contains("iffalse"),
                "\\iffalse must remain inside \\foo body, got {body:?}"
            );
        }
        other => panic!("\\foo should be a macro, got {other:?}"),
    }
    assert!(
        e.cs.lookup(b"marker").and_then(|id| e.eqtb.resolve(id)).is_some(),
        "tokens after the def must still execute"
    );
    assert_eq!(e.eqtb.cur_level, 1, "group level leaked");
}

#[test]
fn nul_begingroup_hash_brace_def() {
    // \def\foo#1#{...} form used by expl3 Npn (parameter text ends at `{`).
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\^=7
\catcode`\:=11 \catcode`\_=11
\let\if_false:\iffalse
\let\fi:\fi
\catcode0=1
\def\foo#1#{\bar{\expandafter^^@\if_false:}\fi:}}
\catcode0=12
\def\marker{ok}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(e.eqtb.cur_level, 1, "group level leaked: {}", e.term);
    let foo = e.cs.lookup(b"foo").expect("\\foo defined");
    match e.eqtb.resolve(foo) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            assert!(
                body.contains("if_false") || body.contains("iffalse"),
                "\\if_false: must remain inside \\foo body, got {body:?}"
            );
        }
        other => panic!("\\foo should be a macro, got {other:?}"),
    }
}

#[test]
fn undelimited_braced_arg() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\tmpA#1{#1}
\tmpA{\def\marker{ok}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert!(
        e.cs.lookup(b"marker").and_then(|id| e.eqtb.resolve(id)).is_some(),
        "\\tmpA{{...}} did not splice; term={}",
        e.term
    );
}

#[test]
fn the_numexpr_digits() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\count0=\the\numexpr 1+2\relax
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(e.eqtb.count[0], 3, "\\the\\numexpr 1+2 failed; term={}", e.term);
}

#[test]
fn catcode_backtick_nul() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\^=7
\catcode`\^^@=1
\def\foo{\x{\expandafter^^@\iffalse}\fi}}
\catcode0=12
\def\marker{ok}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(e.eqtb.cur_level, 1, "group leaked: {}", e.term);
    let foo = e.cs.lookup(b"foo").expect("\\foo defined");
    match e.eqtb.resolve(foo) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            assert!(
                body.contains("iffalse"),
                "\\iffalse must remain in body, got {body:?}"
            );
        }
        other => panic!("\\foo should be a macro, got {other:?}"),
    }
}

#[test]
fn catcode_the_numexpr_nul() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\^=7
\catcode\the\numexpr 0\relax=\the\numexpr 1\relax
\def\foo{\x{\expandafter^^@\iffalse}\fi}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(e.eqtb.cur_level, 1, "group leaked: {}", e.term);
    assert_eq!(
        e.eqtb.cat[0],
        1,
        "catcode 0 not 1 after \\the\\numexpr assign; term={}",
        e.term
    );
    let foo = e.cs.lookup(b"foo").expect("\\foo defined");
    match e.eqtb.resolve(foo) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            assert!(
                body.contains("iffalse"),
                "\\iffalse must remain in body, got {body:?}"
            );
        }
        other => panic!("\\foo should be a macro, got {other:?}"),
    }
}

#[test]
fn catcode_numexpr_backtick_nul() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\^=7
\catcode\the\numexpr `\^^@\relax=\the\numexpr 1\relax
\def\foo{\x{\expandafter^^@\iffalse}\fi}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(e.eqtb.cur_level, 1, "group leaked: {}", e.term);
    assert_eq!(
        e.eqtb.cat[0],
        1,
        "catcode 0 not 1 after \\numexpr backtick; term={} cat0={}",
        e.term,
        e.eqtb.cat[0]
    );
}

#[test]
fn f_expansion_active_nul() {
    // expl3-code.tex:2765-2773 — \romannumeral + `^^@ (active) stops f-expansion
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\^=7
\catcode`\:=11 \catcode`\_=11
\let\exp:w\romannumeral
\begingroup
\catcode`\^^@=13
\global\protected\def\exp_end_continue_f:w{`^^@}
\gdef^^@{\def\HIT{1}}
\endgroup
\edef\out{\exp:w\exp_end_continue_f:w ok}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"out").expect("\\out defined");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            assert!(
                body.contains("ok"),
                "f-expansion should yield ok, got {body:?}; term={}",
                e.term
            );
        }
        other => panic!("\\out should be a macro, got {other:?} term={}", e.term),
    }
    assert!(
        e.cs.lookup(b"HIT").and_then(|id| e.eqtb.resolve(id)).is_none(),
        "active ^^@ error handler ran; term={}",
        e.term
    );

}

#[test]
fn f_expansion_inside_expanded() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\^=7
\catcode`\:=11 \catcode`\_=11
\let\exp:w\romannumeral
\begingroup
\catcode`\^^@=13
\global\protected\def\exp_end_continue_f:w{`^^@}
\gdef^^@{\def\HIT{1}}
\endgroup
\edef\out{\expanded{\exp:w\exp_end_continue_f:w ok}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert!(
        e.cs.lookup(b"HIT").and_then(|id| e.eqtb.resolve(id)).is_none(),
        "^^@ handler ran inside \\expanded; term={}",
        e.term
    );
    let id = e.cs.lookup(b"out").expect("\\out defined");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            assert!(body.contains("ok"), "got {body:?}");
        }
        other => panic!("\\out not a macro: {other:?}"),
    }
}

#[test]
fn f_expansion_continue_nw() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\^=7
\catcode`\:=11 \catcode`\_=11
\let\exp:w\romannumeral
\begingroup
\catcode`\^^@=13
\global\protected\def\exp_end_continue_f:w{`^^@}
\global\def\exp_end_continue_f:nw#1{`^^@#1}
\gdef^^@{\def\HIT{1}}
\endgroup
\def\ok{OK}
\edef\out{\exp:w\exp_end_continue_f:nw{\ok}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert!(
        e.cs.lookup(b"HIT").and_then(|id| e.eqtb.resolve(id)).is_none(),
        "^^@ handler ran in nw; term={}",
        e.term
    );
    let id = e.cs.lookup(b"out").expect("\\out defined");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            assert!(body.contains("OK"), "nw f-expansion got {body:?}");
        }
        other => panic!("\\out not a macro: {other:?}"),
    }
}

#[test]
fn delimited_char_needle_in_haystack() {
    // expl3 \tl_if_in:nn: \def\tmp#1o{} then \tmp NnpcofeVvx {} {} o
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\tmp#1o{}
\expandafter\def\expandafter\out\expandafter{\tmp NnpcofeVvx {} {} o}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"out").expect("\\out defined");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            assert!(
                body.contains("feVvx"),
                "delimited o should leave the tail, got {body:?}"
            );
        }
        other => panic!("\\out not a macro: {other:?} term={}", e.term),
    }
}

#[test]
fn expanded_detokenize_letters() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\edef\out{\expanded{\detokenize{oooo}}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"out").expect("\\out defined");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            assert_eq!(body, "oooo", "detokenize inside expanded got {body:?}");
        }
        other => panic!("\\out not a macro: {other:?}"),
    }
}

#[test]
fn use_e_detokenize_then_delimited() {
    // \str_if_in:nn skeleton: \use:e { ... \detokenize ... } then \tl_if_in
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\usee#1{\expanded{#1}}
\usee{\def\hay{\detokenize{NnpcofeVvx}}}
\usee{\def\ndl{\detokenize{o}}}
\edef\out{\hay}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"out").expect("\\out defined");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            assert_eq!(body, "NnpcofeVvx", "got {body:?}");
        }
        other => panic!("\\out not a macro: {other:?} term={}", e.term),
    }
}

#[test]
fn tl_if_in_skeleton() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\long\def\tlifempty#1{%
  \if\relax\detokenize\expandafter{#1}\relax
    \expandafter\usefirst
  \else
    \expandafter\usesecond
  \fi
}
\long\def\usefirst#1#2{#1}
\long\def\usesecond#1#2{#2}
\long\def\tlifin#1#2{%
  \def\tltmp##1#2{}%
  \tlifempty{\tltmp#1{}#2}%
    {\def\res{F}}%
    {\def\res{T}}%
}
\tlifin{NnpcofeVvx}{o}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"res").expect("\\res defined");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            assert_eq!(body, "T", "\\res got {body:?}");
        }
        other => panic!("\\res not a macro: {other:?} term={}", e.term),
    }
}

#[test]
fn str_if_in_with_use_e() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\long\def\usee#1{\expanded{#1}}
\long\def\tlifempty#1{%
  \if\relax\detokenize\expandafter{#1}\relax
    \expandafter\usefirst
  \else
    \expandafter\usesecond
  \fi
}
\long\def\usefirst#1#2{#1}
\long\def\usesecond#1#2{#2}
\protected\long\def\tlifinTF#1#2#3#4{%
  \def\tltmp##1#2{}%
  \tlifempty{\tltmp#1{}#2}{#4}{#3}%
}
\long\def\strifinF#1#2#3{%
  \usee{\tlifinTF{\detokenize{#1}}{\detokenize{#2}}}%
    {\usefirst}{\usesecond}{}{#3}%
}
\strifinF{NnpcofeVvx}{o}{\def\res{FAIL}}
\ifx\res\undefined \def\res{OK} \fi
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"res").expect("\\res defined");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            assert_eq!(body, "OK", "\\res got {body:?}");
        }
        other => panic!("\\res not a macro: {other:?} term={}", e.term),
    }
}

#[test]
fn expl3_exact_str_if_in_test() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\long\def\usee#1{\expanded{#1}}
\long\def\usei#1#2{#1}
\long\def\useii#1#2{#2}
\def\prgrettrue{\expandafter\usei\romannumeral}
\def\prgretfalse{\expandafter\useii\romannumeral}
\chardef\expend=0
\long\def\tlifempty#1{%
  \if\relax\detokenize\expandafter{#1}\relax
    \expandafter\usei
  \else
    \expandafter\useii
  \fi
}
\protected\long\def\tlifinTF#1#2#3#4{%
  \def\tltmp##1#2{}%
  \tlifempty{\tltmp#1{}#2}{\prgretfalse}{\prgrettrue}%
  \expend #3 #4%
}
\protected\long\def\strifinF#1#2#3{%
  \usee{\tlifinTF{\detokenize{#1}}{\detokenize{#2}}}%
    {\prgrettrue}{\prgretfalse}\expend{}{#3}%
}
\strifinF{NnpcofeVvx}{o}{\def\res{FAIL}}
\ifx\res\undefined \def\res{OK} \fi
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"res").expect("\\res defined");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            assert_eq!(body, "OK", "\\res got {body:?}");
        }
        other => panic!("\\res not a macro: {other:?} term={}", e.term),
    }
}

#[test]
fn tltmp_expansion_probe() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\usee#1{\expanded{#1}}
\def\tltmp#1o{}
\edef\resA{\tltmp NnpcofeVvx{}o}
\edef\resB{\expanded{\detokenize\expandafter{\resA}}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let idA = e.cs.lookup(b"resA").expect("\\resA defined");
    let idB = e.cs.lookup(b"resB").expect("\\resB defined");
    let bodyA = match e.eqtb.resolve(idA) {
        Some(Equiv::Macro(m)) => e.tokens_to_string(&m.body),
        _ => String::new(),
    };
    let bodyB = match e.eqtb.resolve(idB) {
        Some(Equiv::Macro(m)) => e.tokens_to_string(&m.body),
        _ => String::new(),
    };
    eprintln!("bodyA = {bodyA:?}, bodyB = {bodyB:?}");
    assert_eq!(bodyA, "feVvx{}o");
}

#[test]
fn tl_analysis_bgroup_egroup_def() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\@=11 \catcode`\^=7
\def\csnewprotectedNpn#1#{\protected\long\gdef#1}
\def\groupbegin{\begingroup}
\def\groupend{\endgroup}
\def\charsetcatcodegroupbeginN#1{\catcode`#1=1 }
\def\charsetcatcodegroupendN#1{\catcode`#1=2 }
\groupbegin
  \charsetcatcodegroupbeginN\^^@ % {
  \csnewprotectedNpn\bgrp
    { \groupnw { \expafter ^^@ \iffalse } \fi } }
  \charsetcatcodegroupendN\^^@
  \csnewprotectedNpn\egrp
    { \groupnw { \iffalse { \fi ^^@ } } % }
\groupend
"#,
    );
    if e.error_count > 0 {
        eprintln!("TERM OUTPUT:\n{}", e.term);
    }
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id_b = e.cs.lookup(b"bgrp").expect("\\bgrp defined");
    let id_e = e.cs.lookup(b"egrp").expect("\\egrp defined");
    assert!(e.eqtb.resolve(id_b).is_some());
    assert!(e.eqtb.resolve(id_e).is_some());
}
