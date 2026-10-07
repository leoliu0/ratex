//! LuaTeX 1.24 semantics of the Lua <-> TeX text interface, catcode tables
//! and attributes. Every expectation was produced by `luatex --ini` (TeX
//! Live 2026) on the same input.

use tex_core::engine::{Engine, EngineKind, InteractionMode};

/// `luatex --ini` with braces/parameter catcodes and every extra primitive
/// enabled, as the probes did.
fn luatex_ini() -> Engine {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.set_interaction_mode(InteractionMode::Nonstop);
    e
}

const PREAMBLE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6\n\
\\directlua{tex.enableprimitives(\"\",tex.extraprimitives())}\n";

fn run(e: &mut Engine, body: &str) {
    let src = format!("{PREAMBLE}{body}\n\\end\n");
    e.input.push_file("t.tex".to_string(), src.into_bytes());
    e.run();
}

/// The `[...]` lines a run wrote with `\immediate\write16`.
fn shown(e: &Engine) -> Vec<String> {
    e.term
        .lines()
        .filter(|l| l.starts_with('[') && l.ends_with(']'))
        .map(str::to_string)
        .collect()
}

fn errors(e: &Engine) -> Vec<String> {
    e.diagnostics.iter().map(|d| d.message.clone()).collect()
}

#[test]
fn tex_print_family_follows_luatex_line_rules() {
    let mut e = luatex_ini();
    run(
        &mut e,
        r#"\def\show#1{\immediate\write16{[#1]}}
\edef\x{\directlua{tex.sprint("a") tex.sprint("b")}}\show{\meaning\x}
\edef\x{\directlua{tex.sprint("  a  ")}}\show{\meaning\x}
\edef\x{\directlua{tex.print("a") tex.print("b")}}\show{\meaning\x}
\edef\x{\directlua{tex.print("  a  ")}}\show{\meaning\x}
\edef\x{\directlua{tex.print("a  ") tex.sprint("b")}}\show{\meaning\x}
\edef\x{\directlua{tex.write("a  b\string\\x{ }  ")}}\show{\meaning\x}
\edef\x{\directlua{tex.sprint(-2, "\string\\x{ }")}}\show{\meaning\x}
\edef\x{\directlua{tex.cprint(11, "a1{ }")}}\show{\meaning\x}
\edef\x{\directlua{tex.cprint(99, "a1{ }")}}\show{\meaning\x}
\edef\x{\directlua{tex.print(1, "x")}}\show{\meaning\x}
\edef\x{\directlua{tex.print({"a","b"})}}\show{\meaning\x}
\edef\x{\directlua{tex.sprint(3.5, 7)}}\show{\meaning\x}
\edef\x{\directlua{tex.sprint(1.5)}}\show{\meaning\x}
\edef\x{\directlua{tex.sprint(7)}}\show{\meaning\x}
\edef\x{\directlua{tex.tprint({"a "},{-2,"\string\\b c"})}}\show{\meaning\x}
\edef\x{\directlua{tex.print("\string\\relax") tex.print("q")}}\show{\meaning\x}
\endlinechar=`\A
\edef\x{\directlua{tex.print("p") tex.print("q") tex.print("r")}}\show{\meaning\x}
\endlinechar=13
\everyeof{EOF}\edef\x{\directlua{tex.print("p")}}\show{\meaning\x}\everyeof{}
\initcatcodetable 3
\edef\x{\directlua{tex.print(3, "\string\\relax{}")}}\show{\meaning\x}
\edef\x{\directlua{tex.print(77, "{}")}}\show{\meaning\x}
\edef\x{\directlua{tex.sprint("\string\\relax") tex.sprint("bar")}}\show{\meaning\x}
\edef\x{\directlua{tex.sprint("a") token.put_next(token.create("relax"))}}\show{\meaning\x}
\edef\x{\directlua{tex.sprint("a", token.create("relax"), "b")}}\show{\meaning\x}
\edef\x{\directlua{tex.print("\string\\relax")}a}\show{\meaning\x}
\edef\x{\directlua{tex.sprint("\string\\relax")}a}\show{\meaning\x}
\edef\x{\directlua{tex.print("\string\\relax", "")}a}\show{\meaning\x}"#,
    );
    assert!(errors(&e).is_empty(), "errors: {:?}", errors(&e));
    let expected = [
        "[macro:->ab]",
        "[macro:-> a ]",
        "[macro:->a b]",
        "[macro:->a]",
        "[macro:->a b]",
        "[macro:->a b\\x{ }]",
        "[macro:->\\x{ }]",
        "[macro:->a1{ }]",
        "[macro:->a1{ }]",
        "[macro:->x]",
        "[macro:->a b]",
        "[macro:->7]",
        "[macro:->1.5]",
        "[macro:->7]",
        "[macro:->a \\b c]",
        "[macro:->\\relax q]",
        "[macro:->pAqAr]",
        "[macro:->p]",
        "[macro:->\\relax {}]",
        "[macro:->{}]",
        "[macro:->\\relax bar]",
        "[macro:->a\\relax ]",
        "[macro:->a\\relax b]",
        "[macro:->\\relax a]",
        "[macro:->\\relax a]",
        "[macro:->\\relax a]",
    ];
    assert_eq!(shown(&e), expected, "term: {}", e.term);
}

/// textoken.c `str_toks` decodes UTF-8: `\detokenize`, `\string`, `\meaning`
/// and `\luaescapestring` give one character token per scalar, so biblatex's
/// `\DeclareRangeChars*{–—}` builds `\do\–\do\—` single-character control
/// sequences that work as alphabetic constants.
#[test]
fn string_conversions_yield_one_token_per_unicode_scalar() {
    let mut e = luatex_ini();
    run(
        &mut e,
        r#"\def\show#1{\immediate\write16{[#1]}}
\def\count#1#2\relax{\number`#1\ifx\relax#2\relax\else,\count#2\relax\fi}
\show{\expandafter\count\detokenize{–é—}\relax}
\show{\expandafter\count\string\–\relax}
\def\m{ü}\show{\expandafter\count\meaning\m\relax}
\show{\expandafter\count\luaescapestring{ä"}\relax}
\def\defdochars#1#2{\ifx#2\relax\else
  \xdef#1{\unexpanded\expandafter{#1}\noexpand\do\expandafter\noexpand\csname#2\endcsname}%
  \expandafter\defdochars\expandafter#1\fi}
\def\foo{}\expandafter\defdochars\expandafter\foo\detokenize{–—}\relax
\def\do#1{\uccode`#1=`\%}\foo
\show{\the\uccode"2013,\the\uccode"2014}"#,
    );
    assert!(errors(&e).is_empty(), "errors: {:?}", errors(&e));
    assert_eq!(
        shown(&e),
        [
            "[8211,233,8212]",
            "[92,8211]",
            "[109,97,99,114,111,58,45,62,252]",
            "[228,92,34]",
            "[37,37]",
        ],
        "term: {}",
        e.term
    );
}

#[test]
fn printed_bytes_that_are_not_utf8_read_as_replacement_characters() {
    let mut e = luatex_ini();
    run(
        &mut e,
        r#"\edef\x{\directlua{tex.sprint(string.char(233))}}\immediate\write16{[\number\expandafter`\x]}
\edef\x{\directlua{tex.sprint(string.char(195,169))}}\immediate\write16{[\number\expandafter`\x]}
\edef\x{\directlua{tex.sprint(string.char(233).."zq")}}\immediate\write16{[\meaning\x]}
\edef\x{\directlua{tex.sprint("a"..string.char(255))}}\immediate\write16{[\meaning\x]}
\directlua{texio.write(string.char(233))}"#,
    );
    assert_eq!(
        errors(&e),
        vec!["String contains an invalid utf-8 sequence".to_string(); 3],
        "term: {}",
        e.term
    );
    assert_eq!(
        shown(&e),
        ["[65533]", "[233]", "[macro:->\u{FFFD}]", "[macro:->a\u{FFFD}]"],
        "term: {}",
        e.term
    );
}

#[test]
fn texio_writes_to_terminal_and_log_like_luatex() {
    let mut e = luatex_ini();
    run(
        &mut e,
        r#"\directlua{texio.write("A") texio.write("B")}\directlua{texio.write_nl("C")}\directlua{texio.write_nl("log","L") texio.write("term","T") texio.write("term and log","X")}
\newlinechar=`\| \directlua{texio.write_nl("p|q")}
\directlua{texio.write_nl("term", "y", "z")}"#,
    );
    assert!(errors(&e).is_empty(), "errors: {:?}", errors(&e));
    assert!(e.term.contains("AB\nCTX\np\nq\ny\nz"), "term: {:?}", e.term);
    assert!(e.log.contains("AB\nC\nLX\np\nq"), "log: {:?}", e.log);
    assert!(!e.log.contains('y') && !e.term.contains('L'), "term: {:?} log: {:?}", e.term, e.log);
}

#[test]
fn catcode_tables_are_live_and_grouped() {
    let mut e = luatex_ini();
    run(
        &mut e,
        r#"\def\s#1{\immediate\write16{[#1:\the\catcode`X/\the\catcode`Y/\the\catcode`Z/\the\catcodetable]}}
\initcatcodetable1
\s{A}
\catcodetable1 \catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\X=13 \s{B}
\catcodetable0 \s{C}
\catcodetable1 \s{D}
\catcodetable0
{\catcodetable1 \catcode`\X=3 \s{E}}\s{F}
\catcodetable1 \s{G}\catcodetable0
{\catcode`\Y=13 \catcodetable1 \s{H}}\s{I}
{\catcode`\Z=13 \savecatcodetable2 }\s{J}\catcodetable2 \s{K}\catcodetable0
{\catcodetable1 \global\catcode`\X=4 }\catcodetable1 \s{L}\catcodetable0
{\catcodetable1 \catcode`\Y=7 \savecatcodetable3 }\catcodetable3 \s{M}\catcodetable0
\catcodetable5 \s{N}
\savecatcodetable0
\initcatcodetable0
\initcatcodetable 32768
{\global\catcodetable1 }\s{O}
\catcodetable0 \s{P}
\catcodetable1 {\catcode`Y=11 \catcodetable0 \catcode`\Y=5 \catcodetable1 \s{Q}}\s{R}\catcodetable0 \s{S}"#,
    );
    assert_eq!(errors(&e), vec!["Invalid \\catcode table".to_string(); 4]);
    let expected = [
        "[A:11/11/11/0]",
        "[B:13/11/11/1]",
        "[C:11/11/11/0]",
        "[D:13/11/11/1]",
        "[E:3/11/11/1]",
        "[F:11/11/11/0]",
        "[G:13/11/11/1]",
        "[H:13/11/11/1]",
        "[I:11/11/11/0]",
        "[J:11/11/11/0]",
        "[K:11/11/13/2]",
        "[L:4/11/11/1]",
        "[M:4/7/11/3]",
        "[N:11/11/11/0]",
        "[O:4/11/11/1]",
        "[P:11/11/11/0]",
        "[Q:4/11/11/1]",
        "[R:4/11/11/1]",
        "[S:11/11/11/0]",
    ];
    assert_eq!(shown(&e), expected, "term: {}", e.term);
}

#[test]
fn catcode_tables_attributes_and_unicode_codes_survive_the_format() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("luatex-fmt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.fmt");
    let mut e = luatex_ini();
    let src = format!(
        "{PREAMBLE}{}",
        r#"\initcatcodetable 4 \catcodetable4 \catcode`\{=1 \catcode`\}=2 \catcode`\X=13 \catcode"4E00=11 \catcodetable0
\catcode"4E01=11 \catcode"4E02=13 \attribute7=42 \attributedef\myattr=9 \myattr=-5
\sfcode"4E03=999 \delcode"4E05="123456
\catcodetable 4
"#
    );
    e.input.push_file("t.tex".to_string(), src.into_bytes());
    e.run();
    assert!(errors(&e).is_empty(), "errors: {:?}", errors(&e));
    tex_core::format::save_format(&e, &path).unwrap();

    let mut e = tex_core::format::load_format(&path).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    e.set_interaction_mode(InteractionMode::Nonstop);
    e.input.push_file(
        "u.tex".to_string(),
        br#"\immediate\write16{[\the\catcodetable/\the\catcode`X/\the\catcode"4E00/\the\catcode"4E01]}
\catcodetable0 \immediate\write16{[\the\catcode"4E01/\the\catcode"4E02/\the\catcode`X/\the\attribute7/\the\myattr/\meaning\myattr]}
\immediate\write16{[\the\sfcode"4E03/\the\delcode"4E05]}
\catcodetable4 \immediate\write16{[\the\catcode`X]}
\end
"#
        .to_vec(),
    );
    e.run();
    assert!(errors(&e).is_empty(), "errors: {:?}", errors(&e));
    assert_eq!(
        shown(&e),
        ["[4/13/11/12]", "[11/13/11/42/-5/\\attribute9]", "[999/1193046]", "[13]"],
        "term: {}",
        e.term
    );
}

#[test]
fn attributes_are_registers_of_their_own() {
    let mut e = luatex_ini();
    run(
        &mut e,
        r#"\message{[\the\attribute1][\the\count1]}
\attribute1=5 \message{[\the\attribute1][\the\count1]}
{\attribute1=7 \message{[\the\attribute1]}}\message{[\the\attribute1]}
\attributedef\foo=6 \message{[\meaning\foo][\the\foo]}
\foo=-3 \message{[\the\attribute6][\directlua{tex.print(tex.attribute[6])}][\directlua{tex.print(tex.attribute[2])}]}
\directlua{tex.attribute[2]=9}\message{[\the\attribute2]}
\advance\attribute2 by 3 \multiply\foo by 2 \message{[\the\attribute2][\the\foo][\the\count2]}
\count2=\attribute1 \message{[\the\count2]}
\message{[\the\attribute65535]}
\attribute65536=1"#,
    );
    let expected = "[-2147483647][0][5][0][7][5][\\attribute6][-2147483647][-3][-3][-2147483647][9][12][-6][0][5][-2147483647]";
    let compact: String = e.term.split_whitespace().collect::<Vec<_>>().join("");
    assert!(compact.contains(expected), "term: {}", e.term);
    assert_eq!(errors(&e).len(), 1, "errors: {:?}", errors(&e));
}

/// luatex --ini on the same input: `\Umath` stack and fraction delimiter
/// parameters start undefined, are filled in by assigning the math family
/// fonts, follow grouping and `\global`, and drive `\atop` and delimited
/// fractions; `\Ustack{..}` typesets like a group.
#[test]
fn umath_stack_and_fraction_delimiter_parameters_follow_luatex() {
    let mut e = luatex_ini();
    run(
        &mut e,
        r#"\catcode`\$=3 \catcode`\^=7
\def\show#1{\immediate\write16{[#1]}}
\def\q{\show{\the\Umathstacknumup\displaystyle/\the\Umathstacknumup\textstyle/\the\Umathstackdenomdown\displaystyle/\the\Umathstackdenomdown\scriptstyle}\show{\the\Umathstackvgap\displaystyle/\the\Umathstackvgap\textstyle/\the\Umathfractiondelsize\displaystyle/\the\Umathfractiondelsize\scriptscriptstyle}}
\q
\font\ti=cmmi10 \font\sy=cmsy10 \font\ex=cmex10 \font\rm=cmr10 \font\sysev=cmsy7 \font\syfive=cmsy5
\textfont0=\rm \scriptfont0=\rm \scriptscriptfont0=\rm
\textfont1=\ti \scriptfont1=\ti \scriptscriptfont1=\ti
\textfont2=\sy \scriptfont2=\sysev \scriptscriptfont2=\syfive
\show{\the\Umathstacknumup\textstyle/\the\Umathstackvgap\textstyle}
\textfont3=\ex \scriptfont3=\ex \scriptscriptfont3=\ex
\q
{\Umathstacknumup\textstyle=1pt \q}\q
{\global\Umathstackdenomdown\scriptstyle=3pt }\q
\Umathstackvgap\displaystyle=10pt \q
\Umathfractiondelsize\textstyle=30pt \Umathfractiondelsize\scriptscriptstyle=1pt \q
\delcode`(="028300 \delcode`)="029301
\def\b#1#2{\setbox0\hbox{$#1#2$}\show{\the\ht0/\the\dp0}}
\b\displaystyle{\Ustack{1\atop 2}}
\b\displaystyle{{1\atop 2}}
\b\displaystyle{{1\over 2}}
\b\textstyle{\Ustack{1\atop 2}}
\b\textstyle{{1\atop 2}}
\b\textstyle{{1\atopwithdelims() 2}}
\b\displaystyle{{1\atopwithdelims() 2}}
\b{}{\Ustack{1\over 2}}
\b{}{\Ustack{12}}
\b{}{\Ustack{x}^2}
\b{}{{x}^2}
\Umathstacknumup\textstyle=0pt \Umathstackdenomdown\textstyle=0pt \Umathstackvgap\textstyle=0pt \q
\b\textstyle{{1\atop 2}}
\Umathstacknumup\bogus=2pt
"#,
    );
    let expected = [
        "[16383.99998pt/16383.99998pt/16383.99998pt/16383.99998pt]",
        "[16383.99998pt/16383.99998pt/16383.99998pt/16383.99998pt]",
        "[4.4373pt/16383.99998pt]",
        "[6.76508pt/4.4373pt/6.85951pt/2.4095pt]",
        "[2.79985pt/1.19994pt/23.9pt/7.09999pt]",
        "[6.76508pt/1.0pt/6.85951pt/2.4095pt]",
        "[2.79985pt/1.19994pt/23.9pt/7.09999pt]",
        "[6.76508pt/4.4373pt/6.85951pt/2.4095pt]",
        "[2.79985pt/1.19994pt/23.9pt/7.09999pt]",
        "[6.76508pt/4.4373pt/6.85951pt/3.0pt]",
        "[2.79985pt/1.19994pt/23.9pt/7.09999pt]",
        "[6.76508pt/4.4373pt/6.85951pt/3.0pt]",
        "[10.0pt/1.19994pt/23.9pt/7.09999pt]",
        "[6.76508pt/4.4373pt/6.85951pt/3.0pt]",
        "[10.0pt/1.19994pt/23.9pt/1.0pt]",
        "[14.61945pt/8.26944pt]",
        "[14.61945pt/8.26944pt]",
        "[13.20952pt/6.85951pt]",
        "[10.88174pt/3.44841pt]",
        "[10.88174pt/3.44841pt]",
        "[17.50014pt/12.50015pt]",
        "[14.61945pt/9.50012pt]",
        "[10.38176pt/4.54442pt]",
        "[6.44444pt/0.0pt]",
        "[10.07336pt/0.0pt]",
        "[10.07336pt/0.0pt]",
        "[6.76508pt/0.0pt/6.85951pt/3.0pt]",
        "[10.0pt/0.0pt/23.9pt/1.0pt]",
        "[9.66667pt/3.22223pt]",
    ];
    assert_eq!(shown(&e), expected, "term: {}", e.term);
    let errors = errors(&e);
    assert!(errors[0].starts_with("Missing math style, treated as \\displaystyle"), "{errors:?}");
    assert_eq!(errors[1], "Undefined control sequence \\bogus");
}

#[test]
fn lua_code_and_pattern_text_leave_par_out() {
    // luababel.def keeps blank lines inside \directlua (issue #17)
    let mut e = luatex_ini();
    run(
        &mut e,
        r#"\directlua{texio.write_nl("[" .. [[a\par b]] .. "]")}
\directlua{texio.write_nl("[" .. [[x

y]] .. "]")}
\language=5 \patterns{a1b

b1c}
\directlua{texio.write_nl("[" .. lang.patterns(lang.new(5)) .. "]")}"#,
    );
    assert_eq!(shown(&e), ["[ab]", "[x y]", "[a1b b1c ]"], "term: {}", e.term);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
}

#[test]
fn patterns_load_after_the_format() {
    // texlang.c `new_patterns` has no INITEX check: babel loads
    // hyph-es.tex at run time under LuaLaTeX (issue #17)
    let mut e = luatex_ini();
    e.ini_mode = false;
    run(
        &mut e,
        r#"\language=5 \patterns{a1b b1c}
\directlua{texio.write_nl("[" .. lang.patterns(lang.new(5)) .. "]")}"#,
    );
    assert_eq!(shown(&e), ["[a1b b1c ]"], "term: {}", e.term);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
}

#[test]
fn long_caret_notation_follows_luatex() {
    // luababel.def spells characters as ^^^^XXXX (issue #17)
    let mut e = luatex_ini();
    run(
        &mut e,
        r#"\def\show#1{\immediate\write16{[#1]}}\catcode`\^=7
\count255=`^^^^200d \show{\the\count255}
\count255=`^^^^^^01f600 \show{\the\count255}
\def\x^^^^0041{OK}\show{\meaning\xA}
\catcode30=12 \edef\y{^^^abc}\show{\meaning\y}
\edef\y{^^^^zz}\show{\meaning\y}"#,
    );
    assert_eq!(
        shown(&e),
        ["[8205]", "[128512]", "[macro:->OK]", "[macro:->^^^abc]", "[macro:->^^^^zz]"],
        "term: {}",
        e.term
    );
    assert_eq!(errors(&e), ["^^^^ needs four hex digits"]);
}

#[test]
fn mathchardef_reads_umathcharnum_values_like_luatex() {
    // spanish.ldf saves `\the\mathcode` with \mathchardef (issue #17)
    let mut e = luatex_ini();
    run(
        &mut e,
        r#"\def\show#1{\immediate\write16{[#1]}}
\mathcode`\.="013A \mathchardef\m=\mathcode`\. \show{\the\mathcode`\.=\the\m}
\mathchardef\n=16777274 \show{\the\n}"#,
    );
    assert_eq!(shown(&e), ["[16777274=314]", "[314]"], "term: {}", e.term);
    assert!(errors(&e).is_empty(), "{:?}", errors(&e));
}
