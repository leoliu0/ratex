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
fn pdffilesize_missing_is_empty() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\edef\missing{\pdffilesize{uenc.dfu}}
\edef\present{\pdffilesize{ot1enc.dfu}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let body = |nm: &[u8]| {
        let id = e.cs.lookup(nm).unwrap();
        match e.eqtb.resolve(id) {
            Some(Equiv::Macro(m)) => e.tokens_to_string(&m.body),
            other => panic!("{:?}", other.map(|eq| eq.kind_name())),
        }
    };
    assert_eq!(body(b"missing"), "", "missing file should yield empty filesize, got {:?}", body(b"missing"));
    assert!(
        !body(b"present").is_empty(),
        "ot1enc.dfu should have a size, got {:?}",
        body(b"present")
    );
}


#[test]
fn edef_stops_at_closing_brace() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\edef\x{abc}
\edef\y{POP}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let body = |nm: &[u8]| {
        let id = e.cs.lookup(nm).unwrap_or_else(|| panic!("\\{} missing", String::from_utf8_lossy(nm)));
        match e.eqtb.resolve(id) {
            Some(Equiv::Macro(m)) => e.tokens_to_string(&m.body),
            other => panic!("{:?}", other.map(|eq| eq.kind_name())),
        }
    };
    assert_eq!(body(b"x"), "abc");
    assert_eq!(body(b"y"), "POP");
}

#[test]
fn edef_keeps_let_rbrace_cs() {
    // tex.web: \let\e=} is not expandable; \edef stores the CS, not `}`.
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\let\e=}
\edef\x{a\e b}
\edef\y{POP}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let body = |nm: &[u8]| {
        let id = e.cs.lookup(nm).unwrap_or_else(|| panic!("\\{} missing", String::from_utf8_lossy(nm)));
        match e.eqtb.resolve(id) {
            Some(Equiv::Macro(m)) => e.tokens_to_string(&m.body),
            other => panic!("{:?}", other.map(|eq| eq.kind_name())),
        }
    };
    assert_eq!(body(b"y"), "POP", "edef closed early; leftover leaked");
    let xb = body(b"x");
    assert!(xb.contains('e') || xb.contains('\\'), "want CS \\e in body, got [{xb}]");
    assert!(!xb.contains('}'), "}} must not be substituted into edef body, got [{xb}]");
}

#[test]
fn ignorespaces_is_primitive() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\def\foo{\ignorespaces}
\foo    X
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert!(meaning_prim(&e, b"ignorespaces").is_some());
}



#[test]
fn expanded_stops_at_closing_brace() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\edef\x{\expanded{abc}}
\edef\y{POP}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let body = |nm: &[u8]| {
        let id = e.cs.lookup(nm).unwrap_or_else(|| panic!("\\{} missing", String::from_utf8_lossy(nm)));
        match e.eqtb.resolve(id) {
            Some(Equiv::Macro(m)) => e.tokens_to_string(&m.body),
            other => panic!("{:?}", other.map(|eq| eq.kind_name())),
        }
    };
    assert_eq!(body(b"x"), "abc", "got {:?}", body(b"x"));
    assert_eq!(body(b"y"), "POP");
}

#[test]
fn expanded_double_brace_then_following_macro() {
    // \exp_args:Ne pattern: \expanded{{#2}} must not eat the next token
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\after{AFTER}
\edef\x{\expanded{{abc}}\after}
\edef\y{POP}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let body = |nm: &[u8]| {
        let id = e.cs.lookup(nm).unwrap_or_else(|| panic!("\\{} missing", String::from_utf8_lossy(nm)));
        match e.eqtb.resolve(id) {
            Some(Equiv::Macro(m)) => e.tokens_to_string(&m.body),
            other => panic!("{:?}", other.map(|eq| eq.kind_name())),
        }
    };
    assert_eq!(body(b"x"), "{abc}AFTER", "got {:?}", body(b"x"));
    assert_eq!(body(b"y"), "POP");
}

#[test]
fn xdef_csname_cases_leave_following_edef() {
    let run = |src: &str, label: &str| {
        let mut e = boot();
        run_tex(&mut e, src);
        assert_eq!(e.error_count, 0, "{label} errors:\n{}", e.term);
        let y = e.cs.lookup(b"y").unwrap_or_else(|| panic!("{label}: \\y missing; term={}", e.term));
        match e.eqtb.resolve(y) {
            Some(Equiv::Macro(m)) => assert_eq!(e.tokens_to_string(&m.body), "POP", "{label}"),
            other => panic!("{label} y {:?}", other.map(|eq| eq.kind_name())),
        }
    };
    run(
        r#"
\catcode`\{=1 \catcode`\}=2
\xdef\foo{\csname abc\endcsname}
\edef\y{POP}
"#,
        "plain csname",
    );
    run(
        r#"
\catcode`\{=1 \catcode`\}=2
\xdef\foo{\csname\expanded{abc}\endcsname}
\edef\y{POP}
"#,
        "csname+expanded",
    );
    run(
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\escapechar=-1
\def\identity#1{#1}
\def\expne#1#2{\expandafter#1\expanded{{#2}}}
\def\sanitize#1{\expandafter\string\csname n=#1\endcsname}
\def\resolve#1{\expne\identity{\sanitize{#1}}}
\xdef\foo{\csname\expandafter\resolve\expandafter{uenc.dfu}\endcsname}
\edef\y{POP}
"#,
        "csname+ne+sanitize",
    );
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
\count1=\numexpr 4+5\relax
\count2=\number\numexpr 6+7\relax
"#,
    );
    eprintln!(
        "numexpr counts c0={} c1={} c2={} errors={} term={}",
        e.eqtb.count[0], e.eqtb.count[1], e.eqtb.count[2], e.error_count, e.term
    );
    assert_eq!(e.eqtb.count[1], 9, "\\numexpr 4+5; term={}", e.term);
    assert_eq!(e.eqtb.count[2], 13, "\\number\\numexpr 6+7; term={}", e.term);
    assert_eq!(e.eqtb.count[0], 3, "\\the\\numexpr 1+2 failed; term={}", e.term);
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
}

#[test]
fn if_nested_true_in_operand_takes_else() {
    // tex.web 498 / pdftex: \\if T\\iftrue F\\fi T is false.
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\if T\iftrue F\fi T\count0=1\else\count0=2\fi
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(e.eqtb.count[0], 2, "term={}", e.term);
}

#[test]
fn if_nested_false_in_operand_takes_then() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\if T\iffalse F\fi T\count0=1\else\count0=2\fi
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(e.eqtb.count[0], 1, "term={}", e.term);
}

#[test]
fn openin_read_ifeof_terminates() {
    let path = "/tmp/rust-tex-ifeof.dat";
    std::fs::write(path, "one\ntwo\n").unwrap();
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\count0=0
\def\loop#1\repeat{\def\iterate{#1\relax\expandafter\iterate\fi}\iterate}
\openin1=/tmp/rust-tex-ifeof.dat
\loop
  \read1 to \ln
  \advance\count0 by 1
  \if T\ifeof1F\fi T\relax
\repeat
"#,
    );
    assert!(
        e.eqtb.count[0] >= 2 && e.eqtb.count[0] <= 5,
        "read loop count={} term={}",
        e.eqtb.count[0],
        e.term
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
}

#[test]
fn csname_filename_equals_in_edef() {
    // expl3 \\__file_name_expand:n: \\csname __file_name=#1\\endcsname inside \\edef
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\expandname#1{\expandafter\string\csname __file_name=#1\endcsname}
\edef\x{\expandname{ot1enc.dfu}}
\def\y{\x}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"x").expect("\\x");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let s = e.tokens_to_string(&m.body);
            assert!(
                s.contains("ot1enc.dfu"),
                "edef csname trick body={s:?} term={}",
                e.term
            );
        }
        other => panic!("\\x not a macro: {other:?}"),
    }
}

#[test]
fn csname_inside_expanded_inside_edef() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\id#1{#1}
\def\expandname#1{\expandafter\string\csname __file_name=#1\endcsname}
\edef\x{\expandafter\id\expanded{{\expandname{ot1enc.dfu}}}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"x").expect("\\x");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let s = e.tokens_to_string(&m.body);
            assert!(
                s.contains("ot1enc.dfu"),
                "exp_args:Ne pattern body={s:?} term={}",
                e.term
            );
        }
        other => panic!("\\x not a macro: {other:?}"),
    }
}

#[test]
fn expanded_double_brace_preserves_group() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\take#1{\def\got{#1}}
\expandafter\take\expanded{{hello}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"got").expect("\\got");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let s = e.tokens_to_string(&m.body);
            assert_eq!(s, "hello", "got={s:?} term={}", e.term);
        }
        other => panic!("\\got not a macro: {other:?}"),
    }
}


#[test]
fn nested_exp_args_ne_inside_edef() {
    // expl3 \\__kernel_file_name_sanitize:n: two \\exp_args:Ne inside \\edef
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\id#1{#1}
\def\expandname#1{\expandafter\string\csname __file_name=#1\endcsname}
\def\expe#1#2{\expandafter#1\expanded{{#2}}}
\edef\x{\expe\id{\expe\id{\expandname{ot1enc.dfu}}}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"x").expect("\\x");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let s = e.tokens_to_string(&m.body);
            assert!(
                s.contains("ot1enc.dfu"),
                "nested exp_args:Ne body={s:?} term={}",
                e.term
            );
        }
        other => panic!("\\x not a macro: {other:?}"),
    }
}


#[test]
fn csname_expandafter_firstofone_xdef() {
    // latex.ltx \@kernel@make@file@csname
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\@firstofone#1{#1}
\def\expandname#1{\expandafter\string\csname __file_name=#1\endcsname}
\def\resolve#1\@nil{\expandname{#1}}
\xdef\x{\csname\expandafter\resolve\@firstofone{ot1enc.dfu}\@nil\endcsname}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
}

#[test]
fn lowercase_leaves_following_groups() {
    // utf8.def:328 — \lowercase{\InputIfFileExists{#1enc.dfu}}{yes}{no}
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\InputIfFileExists#1#2#3{\def\FN{#1}\def\YES{#2}\def\NO{#3}}
\lowercase{\InputIfFileExists{OT1enc.dfu}}{yes}{no}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let body = |nm: &[u8]| {
        let id = e.cs.lookup(nm).unwrap_or_else(|| panic!("\\{} missing", String::from_utf8_lossy(nm)));
        match e.eqtb.resolve(id) {
            Some(Equiv::Macro(m)) => e.tokens_to_string(&m.body),
            other => panic!("\\{} {:?}", String::from_utf8_lossy(nm), other.map(|eq| eq.kind_name())),
        }
    };
    assert_eq!(body(b"FN"), "ot1enc.dfu", "filename not lowercased");
    assert_eq!(body(b"YES"), "yes", "true-branch eaten by \\lowercase");
    assert_eq!(body(b"NO"), "no", "false-branch eaten by \\lowercase");
}

#[test]
fn f_expansion_expands_following_macro() {
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
\def\f{hello}
\def\expnff#1#2{\expandafter#1\expandafter{\exp:w\exp_end_continue_f:w#2}}
\def\absorb#1{\def\OUT{#1}}
\expnff\absorb{\f}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"OUT").expect("\\OUT");

    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let body = e.tokens_to_string(&m.body);
            assert_eq!(body, "hello", "f-type should expand \\f, got {body:?}; term={}", e.term);
        }
        other => panic!("\\OUT {:?}", other.map(|eq| eq.kind_name())),
    }
}



#[test]
fn dimexpr_assign_two_pt() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\dimen0=\dimexpr 2pt\relax
\dimen1=\dimexpr 2pt+3pt\relax
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(e.eqtb.dimen[0], 2 * 65536, "\\dimen0=\\dimexpr 2pt; term={}", e.term);
    assert_eq!(e.eqtb.dimen[1], 5 * 65536, "\\dimen1=\\dimexpr 2pt+3pt; term={}", e.term);
}

#[test]
fn dimendef_dimexpr_assign() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\dimendef\foo=2
\foo=\dimexpr 4pt\relax
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(e.eqtb.dimen[2], 4 * 65536, "\\foo=\\dimexpr 4pt; term={}", e.term);
}


#[test]
fn csname_expanded_letters() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\catcode`\:=11 \catcode`\_=11
\let\tex_expanded:D\expanded
\expandafter\def\csname \tex_expanded:D{zzw}\endcsname{OK}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"zzw").expect("zzw interned");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            assert_eq!(e.tokens_to_string(&m.body), "OK", "body {:?}", m.body);
        }
        other => panic!("zzw not a macro: {:?}", other.map(|x| x.kind_name())),
    }
}

#[test]
fn csname_noexpand_expanded_letters() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\expandafter\def\csname \noexpand\expanded{zyw}\endcsname{OK}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"zyw").expect("zyw interned");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            assert_eq!(e.tokens_to_string(&m.body), "OK");
        }
        other => panic!("zyw not a macro: {:?}", other.map(|x| x.kind_name())),
    }
}

#[test]
fn ifdefined_undefined_skips_let() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\catcode`\:=11 \catcode`\_=11
\let\tex_ifdefined:D\ifdefined
\let\tex_fi:D\fi
\let\tex_let:D\let
\let\tex_expanded:D\expanded
\tex_ifdefined:D \normalend
  \tex_let:D \tex_expanded:D \normalexpanded
\tex_fi:D
\expandafter\def\csname \tex_expanded:D{zzw}\endcsname{OK}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(
        meaning_prim(&e, b"tex_expanded:D"),
        Some(Prim::Expanded),
        "ifdefined\\normalend must not clobber \\tex_expanded:D"
    );
    let id = e.cs.lookup(b"zzw").expect("zzw interned");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            assert_eq!(e.tokens_to_string(&m.body), "OK");
        }
        other => panic!("zzw not a macro: {:?}", other.map(|x| x.kind_name())),
    }
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
#[test]
fn filehook_xdef_does_not_eat_withhooks() {
    // latex.ltx filehook: \xdef\@curr@file via nested \csname+\string
    // (\@kernel@make@file@csname shape), immediately followed by the
    // {\edef\reserved@a{...}} group shape of \@input@file@exists@with@hooks.
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\@=11
\escapechar=-1
\def\@firstofone#1{#1}
\def\resolve#1\@nil{\expandafter\string\csname __file_name_expand:n=#1\endcsname}
\xdef\@curr@file{\csname\expandafter\resolve\@firstofone{uenc.dfu}\@nil\endcsname}
{\gdef\reserved@a{POP}}


"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let cf = e
        .cs
        .lookup(b"@curr@file")
        .unwrap_or_else(|| panic!("\\@curr@file missing; term:\n{}", e.term));
    match e.eqtb.resolve(cf) {
        Some(Equiv::Macro(m)) => {
            let s = e.tokens_to_string(&m.body);
            assert!(
                !s.contains("POP"),
                "\\@curr@file ate the withhooks group: {s:?}"
            );
            assert!(
                !s.contains("edef"),
                "\\@curr@file swallowed the \\edef: {s:?}"
            );
        }
        other => panic!(
            "\\@curr@file not a macro: {:?}",
            other.map(|o| o.kind_name())
        ),
    }
    let ra = e
        .cs
        .lookup(b"reserved@a")
        .unwrap_or_else(|| panic!("\\reserved@a missing; term:\n{}", e.term));
    match e.eqtb.resolve(ra) {
        Some(Equiv::Macro(m)) => {
            assert_eq!(e.tokens_to_string(&m.body), "POP", "\\reserved@a body");
        }
        other => panic!(
            "\\reserved@a not a macro: {:?} (group-local \\edef also yields None when not eaten)",
            other.map(|o| o.kind_name())
        ),
    }
}

#[test]
fn expanded_inside_csname_inside_xdef() {
    // \expanded inside \csname inside \xdef, with a nested inner \csname
    // carrying its own \endcsname; then the same withhooks-shaped group.
    // \resolve is undelimited here, so the argument is braced (as in
    // xdef_csname_cases_leave_following_edef case 3).
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\@=11
\escapechar=-1
\def\identity#1{#1}
\def\expne#1#2{\expandafter#1\expanded{{#2}}}
\def\sanitize#1{\expandafter\string\csname n=#1\endcsname}
\def\resolve#1{\expne\identity{\sanitize{#1}}}
\xdef\@curr@file{\csname\expandafter\resolve\expandafter{uenc.dfu}\endcsname}
{\gdef\reserved@a{POP}}


"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let cf = e
        .cs
        .lookup(b"@curr@file")
        .unwrap_or_else(|| panic!("\\@curr@file missing; term:\n{}", e.term));
    match e.eqtb.resolve(cf) {
        Some(Equiv::Macro(m)) => {
            let s = e.tokens_to_string(&m.body);
            assert!(
                !s.contains("POP"),
                "\\@curr@file ate the withhooks group: {s:?}"
            );
            assert!(
                !s.contains("edef"),
                "\\@curr@file swallowed the \\edef: {s:?}"
            );
        }
        other => panic!(
            "\\@curr@file not a macro: {:?}",
            other.map(|o| o.kind_name())
        ),
    }
    let ra = e
        .cs
        .lookup(b"reserved@a")
        .unwrap_or_else(|| panic!("\\reserved@a missing; term:\n{}", e.term));
    match e.eqtb.resolve(ra) {
        Some(Equiv::Macro(m)) => {
            assert_eq!(e.tokens_to_string(&m.body), "POP", "\\reserved@a body");
        }
        other => panic!(
            "\\reserved@a not a macro: {:?} (group-local \\edef also yields None when not eaten)",
            other.map(|o| o.kind_name())
        ),
    }
}

#[test]
fn exp_args_nnx_noexpand_target() {
    // expl3 \\exp_args:NNx -> \\use:x {\\exp_not:N #1 \\exp_not:N #2 {#3}}
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\protected\long\def\expargs#1#2#3{\edef\tmp{\noexpand#1\noexpand#2{#3}}\tmp}
\def\set#1#2{\def#1{#2}}
\expargs\set\foo{abc}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"foo").expect("foo interned");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            assert_eq!(e.tokens_to_string(&m.body), "abc", "body {:?}", m.body);
        }
        other => panic!("foo {:?}", other.map(|x| x.kind_name())),
    }
}

fn show_body(e: &Engine, nm: &[u8]) -> String {
    let id = e.cs.lookup(nm).unwrap_or_else(|| panic!("\\{} missing; term={}", String::from_utf8_lossy(nm), e.term));
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => e.tokens_to_string(&m.body),
        other => panic!("\\{} {:?}", String::from_utf8_lossy(nm), other.map(|x| x.kind_name())),
    }
}

#[test]
fn edef_two_noexpands() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\protected\long\def\tlset#1#2{\def#1{#2}}
\def\ltmp{}
\edef\tmp{\noexpand\tlset\noexpand\ltmp{foo}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(show_body(&e, b"tmp"), "\\tlset\\ltmp{foo}", "tmp={}", show_body(&e, b"tmp"));
}

#[test]
fn edef_two_noexpands_via_let() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\protected\long\def\tlset#1#2{\def#1{#2}}
\def\ltmp{}
\let\expnot\noexpand
\edef\tmp{\expnot\tlset\expnot\ltmp{foo}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(show_body(&e, b"tmp"), "\\tlset\\ltmp{foo}", "tmp={}", show_body(&e, b"tmp"));
}

#[test]
fn edef_noexpand_then_execute() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\protected\long\def\tlset#1#2{\def#1{#2}}
\def\ltmp{}
\let\expnot\noexpand
\edef\tmp{\expnot\tlset\expnot\ltmp{foo}}
\tmp
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(show_body(&e, b"ltmp"), "foo", "ltmp={}", show_body(&e, b"ltmp"));
}

#[test]
fn usex_two_stage_noexpand() {
    // latex \\use:x { \\exp_not:N #1 \\exp_not:N #2 {#3} }
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\protected\long\def\tlset#1#2{\def#1{#2}}
\def\ltmp{}
\let\expnot\noexpand
\def\useX#1{\edef\tmp{#1}\tmp}
\useX{\expnot\tlset\expnot\ltmp{foo}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(show_body(&e, b"ltmp"), "foo", "tmp={} ltmp={}", show_body(&e, b"tmp"), show_body(&e, b"ltmp"));
}

#[test]
fn usex_via_expargs_macro() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\protected\long\def\tlset#1#2{\def#1{#2}}
\def\ltmp{}
\let\expnot\noexpand
\def\useX#1{\edef\tmp{#1}\tmp}
\protected\long\def\expargs#1#2#3{\useX{\expnot#1\expnot#2{#3}}}
\expargs\tlset\ltmp{foo}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(show_body(&e, b"ltmp"), "foo", "tmp={} ltmp={}", show_body(&e, b"tmp"), show_body(&e, b"ltmp"));
}

#[test]
fn input_from_macro_runs_file_before_rest() {
    let path = std::env::temp_dir().join("tex_input_order.tex");
    std::fs::write(&path, r"\def\z{file}").unwrap();
    let mut e = boot();
    let src = format!(
        "\\catcode`\\{{=1 \\catcode`\\}}=2 \\catcode`\\#=6\n\\def\\x{{\\input {} \\def\\z{{after}}}}\n\\x\n",
        path.display()
    );
    run_tex(&mut e, &src);
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(show_body(&e, b"z"), "after", "file must run before leftover macro tokens");
}





#[test]
fn hash_hash_one_stays_literal_in_inner_def() {
    // tex.web: \def\outer#1#2{\def\inner##1#2{}} \outer{LIST}{SEARCH}
    // stores \inner as macro:#1SEARCH->  (np=1, delim=SEARCH)
    // Bug we hit at expl3 4558: ##1 was stored as PAR_REF 1 of \outer,
    // so \inner was defined np=0 with prefix LIST SEARCH.
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\outer#1#2{\def\inner##1#2{}}
\outer{LIST}{SEARCH}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"inner").expect("\\inner missing");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let prefix = e.tokens_to_string(&m.prefix);
            let delims: Vec<String> = m.params.iter().map(|d| e.tokens_to_string(d)).collect();
            assert_eq!(m.num_params, 1, "np={} prefix=[{}] delims={:?} body=[{}]", m.num_params, prefix, delims, e.tokens_to_string(&m.body));
            assert_eq!(prefix, "", "prefix leaked list into param text: [{prefix}]");
            assert_eq!(delims, vec!["SEARCH".to_string()], "delim={delims:?}");
            assert_eq!(e.tokens_to_string(&m.body), "");
        }
        other => panic!("\\inner {:?}", other.map(|x| x.kind_name())),
    }
}

#[test]
fn group_local_let_restores_noexpand() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\def\tmp{TMP}
\begingroup
\let\noexpand\tmp
\endgroup
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(meaning_prim(&e, b"noexpand"), Some(Prim::NoExpand), "\\noexpand leaked; meaning={}", {
        let id = e.cs.lookup(b"noexpand").unwrap();
        e.meaning_of(tex_core::token::Token::from_cs(id))
    });
}

#[test]
fn group_local_let_to_undefined_restores() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\def\keep{KEEP}
\begingroup
\let\keep\undefinedcs
\endgroup
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    assert_eq!(show_body(&e, b"keep"), "KEEP");
}


fn inner_macro(e: &Engine) -> (u8, String, Vec<String>) {
    let id = e.cs.lookup(b"inner").expect("\\inner missing");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => (
            m.num_params,
            e.tokens_to_string(&m.prefix),
            m.params.iter().map(|d| e.tokens_to_string(d)).collect(),
        ),
        other => panic!("\\inner {:?}", other.map(|x| x.kind_name())),
    }
}

#[test]
fn expanded_unexpanded_keeps_hash_hash_for_inner_def() {
    // expl3 \\prg_new_protected_conditional uses \\use:e { \\exp_not:n { body } }
    // then \\cs_new_protected:Npn. If \\unexpanded is skipped, e-scan ## collapse
    // turns ##1 into #1 which the outer \\def stores as PAR_REF 1 (4558 CDB).
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\expanded{\unexpanded{\def\tlin#1#2{\def\inner##1#2{}}}}
\tlin{LIST}{SEARCH}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let (np, prefix, delims) = inner_macro(&e);
    assert_eq!(np, 1, "np={np} prefix=[{prefix}] delims={delims:?}");
    assert_eq!(prefix, "", "list leaked into prefix [{prefix}]");
    assert_eq!(delims, vec!["SEARCH".to_string()]);
}

#[test]
fn expandafter_o_braces_once_expanded_list() {
    // \\exp_args:No \\tlin \\marks {SEARCH} must become \\tlin {\\sA\\sB} {SEARCH}
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\sA{A}\def\sB{B}
\def\marks{\sA\sB}
\def\tlin#1#2{\def\inner##1#2{}}
\expandafter\tlin\expandafter{\marks}{SEARCH}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let (np, prefix, delims) = inner_macro(&e);
    assert_eq!(np, 1, "np={np} prefix=[{prefix}] delims={delims:?}");
    assert_eq!(prefix, "");
    assert_eq!(delims, vec!["SEARCH".to_string()]);
}


#[test]
fn unexpanded_via_macro_param_keeps_hash_hash() {
    // prg_new_protected_conditional: \\use:e { \\exp_not:n { #8 } } where #8 is the
    // user body containing ##1. Collapse here stores PAR_REF 1 in \\tl_if_in:nnTF.
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\foo#1{\expanded{\unexpanded{#1}}}
\foo{\def\tlin#1#2{\def\inner##1#2{}}}
\tlin{LIST}{SEARCH}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let (np, prefix, delims) = inner_macro(&e);
    assert_eq!(np, 1, "np={np} prefix=[{prefix}] delims={delims:?} term={}", e.term);
    assert_eq!(prefix, "", "list leaked into prefix [{prefix}]");
    assert_eq!(delims, vec!["SEARCH".to_string()]);
}


#[test]
fn use_none_then_unexpanded_in_expanded_keeps_hash_hash() {
    // expl3 \\__prg_generate_conditional_test:w slow path:
    // \\use:e { \\use_none:n {#8} \\exp_not:n { {#8} } }
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\usenone#1{}
\def\gen#1{\expanded{\usenone{#1}\unexpanded{#1}}}
\gen{\def\tlin#1#2{\def\inner##1#2{}}}
\tlin{LIST}{SEARCH}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let (np, prefix, delims) = inner_macro(&e);
    assert_eq!(np, 1, "np={np} prefix=[{prefix}] delims={delims:?} term={}", e.term);
    assert_eq!(prefix, "", "list leaked into prefix [{prefix}]");
    assert_eq!(delims, vec!["SEARCH".to_string()]);
}

#[test]
fn test_macro_use_none_then_unexpanded() {
    // closer to \\__prg_generate_conditional_test:w #1\\stop #2 { #2 {#1} }
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\usenone#1{}
\def\test#1\stop#2{#2{#1}}
\def\gen#1{\expanded{\test#1\stop\usenone\unexpanded{{#1}}}}
\gen{\def\tlin#1#2{\def\inner##1#2{}}}
\tlin{LIST}{SEARCH}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let (np, prefix, delims) = inner_macro(&e);
    assert_eq!(np, 1, "np={np} prefix=[{prefix}] delims={delims:?} term={}", e.term);
    assert_eq!(prefix, "");
    assert_eq!(delims, vec!["SEARCH".to_string()]);
}

#[test]
fn edef_unexpanded_hashes_are_literal_not_outer_params() {
    // latex.ltx `\__hook_set_normalise_fn:nn`: `\cs_gset_protected:Npx`
    // `\__hook_normalise_fn:nn #1#2 { \cs_set:Npn \__hook_tmp:w
    // \exp_not:V \c__hook_nine_parameters_tl ... }`. `#1` from
    // `\unexpanded` must not become parameter 1 of the outer xdef.
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\edef\outer#1#2{\unexpanded{#1#2#3}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"outer").expect("\\outer missing");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            assert_eq!(m.num_params, 2, "np={} body=[{}]", m.num_params, e.tokens_to_string(&m.body));
            assert_eq!(e.tokens_to_string(&m.body), "#1#2#3", "unexpanded hashes bound to outer params");
        }
        other => panic!("{:?}", other.map(|eq| eq.kind_name())),
    }
}


#[test]
fn long_macro_allows_par_in_braced_arg() {
    // nameref.sty:124 — `\@firstoftwo` is `\long`; its unused branch starts
    // with a blank line. Non-long scan_balanced_raw treated that as runaway.
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\long\def\firstoftwo#1#2{#1}
\firstoftwo{\def\marker{ok}}{

\def\unused#1{no}}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"marker").expect("\\marker missing");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => assert_eq!(e.tokens_to_string(&m.body), "ok"),
        other => panic!("{:?}", other.map(|eq| eq.kind_name())),
    }
}


#[test]
fn unless_ifcsname_inverts() {
    // kvsetkeys.sty: `\unless\ifcsname KV@#1@#2\endcsname` must take the
    // else branch when the key macro exists.
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\def\foo{}
\unless\ifcsname foo\endcsname
  \def\has{no}
\else
  \def\has{yes}
\fi
\unless\ifcsname undefinedxyz\endcsname
  \def\miss{no}
\else
  \def\miss{yes}
\fi
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let body = |nm: &[u8]| {
        let id = e.cs.lookup(nm).unwrap();
        match e.eqtb.resolve(id) {
            Some(Equiv::Macro(m)) => e.tokens_to_string(&m.body),
            other => panic!("{:?}", other.map(|eq| eq.kind_name())),
        }
    };
    assert_eq!(body(b"has"), "yes", "existing cs: unless\\ifcsname should take else");
    assert_eq!(body(b"miss"), "no", "missing cs: unless\\ifcsname should take then");
}

#[test]
fn ifnum_lastpenalty_is_internal_int() {
    // latex.ltx \\fix@penalty: \\ifnum\\lastpenalty=\\z@
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\ifnum\lastpenalty=0 \def\ok{yes}\else\def\ok{no}\fi
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"ok").expect("\\ok missing");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => assert_eq!(e.tokens_to_string(&m.body), "yes"),
        other => panic!("{:?}", other.map(|eq| eq.kind_name())),
    }
}

#[test]
fn toks_expandafter_does_not_expand_group() {
    // Knuth scan_toks xpand=false: the group is copied raw after the
    // expandafter chain, so Car is stored. \\edef{\\the\\toks} then runs it.
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\@=11
\long\def\GTS@Car#1#2\GTS@Nil{#1}
\def\GTS@GlobalString{Introduction}
\toks0=\expandafter\expandafter\expandafter{\expandafter\GTS@Car\GTS@GlobalString{}{}{}{}\GTS@Nil}
\edef\got{\the\toks0}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"got").expect("\\got missing");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let s = e.tokens_to_string(&m.body);
            assert_eq!(s, "I", "Car should yield first token, got [{s}]");
        }
        other => panic!("{:?}", other.map(|eq| eq.kind_name())),
    }
}

#[test]
fn gts_remove_left_strips_phantomsection() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\@=11 \catcode`\%=14
\toksdef\toks@=0
\def\GTS@Nil{}
\def\GTS@End{}
\long\def\GTS@Car#1#2\GTS@Nil{#1}
\long\def\GTS@Cdr#1#2\GTS@Nil{#2}
\def\Hy@phantomsection{}
\def\GTS@GlobalString{\Hy@phantomsection Introduction}
\long\def\GTS@TestLeftEnd#1\GTS@End{\xdef\GTS@GlobalString{\the\toks@}\GTS@RemoveLeft}
\long\def\GTS@TestLeft#1#2{%
  \def\GTS@temp{#1}%
  \ifx\GTS@temp\GTS@Token
    \toks@\expandafter\expandafter\expandafter{\expandafter#2\GTS@GlobalString\GTS@Nil}%
    \expandafter\GTS@TestLeftEnd
  \fi
}
\def\GTS@PredefinedLeftCmds{%
  \GTS@TestLeft\Hy@phantomsection\GTS@Cdr
}
\def\GTS@RemoveLeft{%
  \toks@\expandafter\expandafter\expandafter{\expandafter\GTS@Car\GTS@GlobalString{}{}{}{}\GTS@Nil}%
  \edef\GTS@Token{\the\toks@}%
  \GTS@PredefinedLeftCmds
  \GTS@End
}
\GTS@RemoveLeft
\edef\got{\GTS@GlobalString}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"got").expect("\\got missing");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let s = e.tokens_to_string(&m.body);
            assert_eq!(s, "Introduction", "stripped title, got [{s}]");
        }
        other => panic!("{:?}", other.map(|eq| eq.kind_name())),
    }
}

#[test]
fn bookmark_bracket_prefix_ifnextchar() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\[=12 \catcode`\]=12 \catcode`\@=11
\def\@ifnextchar#1#2#3{%
  \let\reserved@d=#1%
  \def\reserved@a{#2}\def\reserved@b{#3}%
  \futurelet\@let@token\@ifnch
}
\def\@ifnch{%
  \ifx\@let@token\reserved@d
    \let\reserved@c\reserved@a
  \else
    \let\reserved@c\reserved@b
  \fi
  \reserved@c
}
\def\BOOKMARK{\@ifnextchar[{\@BOOKMARK}{\@@BOOKMARK[1][-]}}
\def\@BOOKMARK[#1]{\@ifnextchar[{\@@BOOKMARK[{#1}]}{\@@BOOKMARK[{#1}][-]}}
\def\@@BOOKMARK[#1][#2]#3#4#5{\def\got{#1/#2/#3/#4/#5}}
\BOOKMARK[1][-]{section.1}{Introduction}{1}
"#,
    );
    assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    let id = e.cs.lookup(b"got").expect("\\got missing");
    match e.eqtb.resolve(id) {
        Some(Equiv::Macro(m)) => {
            let s = e.tokens_to_string(&m.body);
            assert_eq!(s, "1/-/section.1/Introduction/1", "got [{s}]");
        }
        other => panic!("{:?}", other.map(|eq| eq.kind_name())),
    }
}






