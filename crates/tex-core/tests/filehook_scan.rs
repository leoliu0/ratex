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
    e.input
        .push_file("t.tex".to_string(), src.as_bytes().to_vec());
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

#[test]
fn split_group_toklist_falls_back_to_file() {
    use tex_core::token::Token;
    let mut e = boot();
    e.eqtb.cat[b'}' as usize] = 2;
    e.input.push_file("remaining.tex".into(), b"Z}}Q".to_vec());
    e.input.push_toks(
        vec![Token::char(1, b'{' as u32), Token::char(11, b'X' as u32)],
        "<argument>",
    );
    // The caller has already consumed the outer opening brace. The nested
    // group's closing brace and the outer closing brace are in the file.
    let argument = e.scan_balanced_raw(true);
    assert_eq!(e.tokens_to_string(&argument), "{XZ}");
    assert_eq!(e.raw_token(), Token::char(11, b'Q' as u32));
    assert_eq!(e.error_count, 0, "{}", e.term);
}

#[test]
fn expandafter_expands_active_characters_once() {
    let mut e = boot();
    run_tex(
        &mut e,
        &(PRE.to_string()
            + r#"
\catcode`\~=13
\def~{OK}
\expandafter\def\expandafter~\expandafter{~}
\edef\result{~}
\def\after{DONE}
"#),
    );
    assert_eq!(body(&e, b"result"), "OK");
    assert_eq!(body(&e, b"after"), "DONE");
    assert_eq!(e.error_count, 0, "{}", e.term);
}

#[test]
fn dimension_character_constant_does_not_expand_active_token() {
    let mut e = boot();
    run_tex(
        &mut e,
        &(PRE.to_string()
            + r#"
\catcode`\~=13 \def~{WRONG}
\edef\result{\number\dimexpr`~sp\relax}
\def\after{DONE}
"#),
    );
    assert_eq!(body(&e, b"result"), "126");
    assert_eq!(body(&e, b"after"), "DONE");
    assert_eq!(e.error_count, 0, "{}", e.term);
}

#[test]
fn dimension_expression_can_supply_an_internal_unit() {
    let mut e = boot();
    run_tex(
        &mut e,
        &(PRE.to_string()
            + r#"
\catcode`\~=13 \def~{WRONG}
\edef\result{\number\dimexpr.25\dimexpr`~sp\relax\relax}
\def\after{DONE}
"#),
    );
    assert_eq!(body(&e, b"result"), "31");
    assert_eq!(body(&e, b"after"), "DONE");
    assert_eq!(e.error_count, 0, "{}", e.term);
}

#[test]
fn vertical_split_preserves_dimension_register_zero() {
    let mut e = boot();
    run_tex(
        &mut e,
        &(PRE.to_string()
            + r#"
\dimen0=73pt
\setbox0=\vbox{\hbox{\vrule height10pt}\vskip2pt\hbox{\vrule height10pt}}
\setbox2=\vsplit0 to12pt
\edef\result{\number\dimen0}
"#),
    );
    assert_eq!(body(&e, b"result"), "4784128");
    assert_eq!(e.error_count, 0, "{}", e.term);
}

#[test]
fn expandafter_the_reenters_expansion_but_direct_the_does_not() {
    let mut e = boot();
    run_tex(
        &mut e,
        &(PRE.to_string()
            + r#"
\def\foo{OK}\def\empty{}\toks0={\foo}
\edef\direct{\the\toks0}
\edef\indirect{\expandafter\empty\the\toks0}
"#),
    );
    assert_eq!(body(&e, b"direct"), "foo ");
    assert_eq!(body(&e, b"indirect"), "OK");
    assert_eq!(e.error_count, 0, "{}", e.term);
}

#[test]
fn protected_active_token_prints_as_its_character() {
    let mut e = boot();
    run_tex(
        &mut e,
        &(PRE.to_string()
            + r#"
\catcode`\~=13 \protected\def~{OK}
\edef\saved{~}
\edef\printed{\expandafter\string\saved}
"#),
    );
    assert_eq!(body(&e, b"saved"), "~");
    assert_eq!(body(&e, b"printed"), "~");
    assert_eq!(e.error_count, 0, "{}", e.term);
}

#[test]
fn write_serialization_preserves_utf8_token_bytes() {
    use tex_core::token::Token;
    let mut e = boot();
    let text = "Sant’Anna";
    let chars: Vec<_> = text.bytes().map(Token::other).collect();
    assert_eq!(e.write_tokens_to_string(&chars), text);
    let active: Vec<_> = text
        .bytes()
        .map(|b| Token::from_cs(e.active_cs_id(b)))
        .collect();
    assert_eq!(e.write_tokens_to_string(&active), text);

    let accent = Token::from_cs(e.cs.intern(b"\""));
    assert_eq!(
        e.write_tokens_to_string(&[accent, Token::letter(b'a')]),
        "\\\"a"
    );
    let control_word = Token::from_cs(e.cs.intern(b"a"));
    assert_eq!(
        e.write_tokens_to_string(&[control_word, Token::letter(b'x')]),
        "\\a x"
    );
}

#[test]
fn assignment_does_not_prefetch_a_conditional_number() {
    let mut e = boot();
    run_tex(
        &mut e,
        &(PRE.to_string()
            + r#"
\def\odd#1#2\stop{\count0=\if-#1-0\else0\expandafter#1\fi#2\relax}
\odd-1\stop
\edef\result{\the\count0}
\def\after{DONE}
"#),
    );
    assert_eq!(body(&e, b"result"), "-1");
    assert_eq!(body(&e, b"after"), "DONE");
    assert_eq!(e.error_count, 0, "{}", e.term);
}
