//! LuaTeX-only primitives: math/delimiter codes, math char definitions and the
//! scan-time `\mathstyle`. Expected values are `luatex --ini` (TeX Live 2026)
//! output for the same input.

use tex_core::engine::{Engine, EngineKind, InteractionMode};

fn run(body: &str) -> Vec<String> {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.set_interaction_mode(InteractionMode::Nonstop);
    let pre = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\catcode`\\$=3 \\catcode`\\^=7 \\catcode`\\_=8\n\
\\directlua{tex.enableprimitives(\"\",tex.extraprimitives())}\n\
\\def\\t#1{\\immediate\\write16{#1}}\\def\\s#1{\\t{[\\the#1]}}\n";
    e.input.push_file("t.tex".to_string(), format!("{pre}{body}\n\\end\n").into_bytes());
    e.run();
    e.term
        .lines()
        .filter(|l| l.starts_with('['))
        .map(str::to_string)
        .collect()
}

#[test]
fn math_and_delimiter_codes_use_luatex_packing() {
    let out = run(
        "\\mathcode`a=\"7141 \\s{\\mathcode`a}\n\
\\Umathcode`a=1 2 \"41 \\s{\\Umathcode`a}\n\
\\Umathcode`b=3 255 \"1F600 \\s{\\Umathcode`b}\n\
\\Umathcodenum`c=\"2A00041 \\s{\\Umathcodenum`c}\n\
\\delcode`(=\"161300 \\s{\\delcode`(}\n\
\\Udelcode`)=3 \"1F600 \\s{\\Udelcodenum`)}\n\
\\s{\\mathcode`z} \\s{\\delcode`z}\n\
\\mathcode`z=\"8000 \\s{\\mathcode`z}\n\
\\mathcode`A=\"123 \\s{\\mathcode`A}\n\
\\delcode`y=-1 \\s{\\delcode`y}",
    );
    assert_eq!(
        out,
        [
            "[31457345]", "[35651649]", "[-10357248]", "[44040257]", "[1446656]", "[529530880]",
            "[31457402]", "[-1]", "[16777216]", "[16777251]", "[2100991]"
        ]
    );
}

#[test]
fn umathchardef_meaning_and_value() {
    let out = run(
        "\\mathchardef\\a=\"7123 \\t{[\\meaning\\a]} \\s\\a\n\
\\Umathchardef\\b=1 2 \"41 \\t{[\\meaning\\b]} \\s\\b\n\
\\Umathcharnumdef\\d=\"2A00041 \\t{[\\meaning\\d]} \\s\\d",
    );
    assert_eq!(
        out,
        [
            "[\\mathchar\"7123]", "[28963]", "[\\Umathchar\"1\"02\"000041]", "[35651649]",
            "[\\Umathchar\"5\"02\"000041]", "[44040257]"
        ]
    );
}

#[test]
fn mathstyle_follows_scan_time_style() {
    let out = run(
        "\\font\\x=cmr10 \\textfont0=\\x \\scriptfont0=\\x \\scriptscriptfont0=\\x\n\
\\textfont1=\\x \\scriptfont1=\\x \\scriptscriptfont1=\\x\n\
\\textfont2=\\x \\scriptfont2=\\x \\scriptscriptfont2=\\x \\textfont3=\\x \\scriptfont3=\\x \\scriptscriptfont3=\\x\n\
\\fontdimen22\\x=1pt \\hsize=100pt \\parindent=0pt\n\
\\def\\ms#1{\\edef\\zz{\\mathstyle}\\t{[#1:\\zz]}}\n\
\\ms{out}\n\
$$\\ms{D}\\scriptstyle\\ms{S}\\crampedscriptscriptstyle\\ms{c}\\displaystyle\\ms{D}\\crampedtextstyle\\ms{ct}$$\n\
$$x^{\\ms{sup}}_{\\ms{sub}}$$\n\
$${\\ms{a}\\over \\ms{b}}$$\n\
$$\\mathchoice{\\ms{cD}}{\\ms{cT}}{\\ms{cS}}{\\ms{cSS}}$$\n\
$$\\Ustack{\\ms{st}}\\radical\"161 {\\ms{rad}}\\overline{\\ms{ov}}\\underline{\\ms{un}}$$",
    );
    assert_eq!(
        out,
        [
            "[out:-1]", "[D:0]", "[S:4]", "[c:7]", "[D:0]", "[ct:3]", "[sup:4]", "[sub:5]", "[a:0]",
            "[b:1]", "[cD:0]", "[cT:2]", "[cS:4]", "[cSS:6]", "[st:2]", "[rad:1]", "[ov:1]", "[un:0]"
        ]
    );
}

const MATH_FONTS: &str = "\\font\\x=cmr10 \\font\\y=cmsy10 \\font\\z=cmex10\n\
\\textfont0=\\x \\scriptfont0=\\x \\scriptscriptfont0=\\x\n\
\\textfont1=\\x \\scriptfont1=\\x \\scriptscriptfont1=\\x\n\
\\textfont2=\\y \\scriptfont2=\\y \\scriptscriptfont2=\\y\n\
\\textfont3=\\z \\scriptfont3=\\z \\scriptscriptfont3=\\z\n\
\\delcode`(=\"028300 \\delcode`)=\"029301 \\delcode`/=\"02F30E \\delcode`[=\"02002C3 \\delcode`]=\"02003C5 \\delcode`.=0\n\
\\hsize=100pt \\parindent=0pt \\thinmuskip=0mu \\medmuskip=0mu \\thickmuskip=0mu \\newlinechar=0\n\
\\showboxbreadth=1000 \\showboxdepth=1000 \\tracingonline1\n\
\\def\\b#1{\\setbox0\\hbox{$#1$}\\t{[\\the\\wd0:\\the\\ht0:\\the\\dp0]}}\n\
\\def\\d#1{\\setbox0\\hbox{$\\displaystyle#1$}\\t{[\\the\\wd0:\\the\\ht0:\\the\\dp0]}}\n\
\\def\\s#1{\\setbox0\\hbox{$\\scriptstyle#1$}\\t{[\\the\\wd0:\\the\\ht0:\\the\\dp0]}}\n";

/// `run`, but keeping the terminal lines of a `\\showlists` (from the first
/// `### ` line to the blank line that ends it), with line numbers blanked.
fn run_lists(body: &str) -> Vec<String> {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.set_interaction_mode(InteractionMode::Nonstop);
    let pre = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\catcode`\\$=3 \\catcode`\\^=7 \\catcode`\\_=8\n\
\\directlua{tex.enableprimitives(\"\",tex.extraprimitives())}\n";
    e.input.push_file("t.tex".to_string(), format!("{pre}{MATH_FONTS}{body}\n\\end\n").into_bytes());
    e.run();
    let lines: Vec<&str> = e.term.lines().collect();
    let start = lines.iter().position(|l| l.starts_with("### ")).unwrap_or(0);
    let end = lines[start..].iter().position(|l| l.is_empty()).map_or(lines.len(), |n| start + n);
    lines[start..end].iter().map(|l| blank_line_number(l)).collect()
}

fn blank_line_number(l: &str) -> String {
    match l.split_once(" at line ") {
        Some((head, tail)) => format!("{head} at line N{}", tail.trim_start_matches(|c: char| c.is_ascii_digit())),
        None => l.to_string(),
    }
}

#[test]
fn luatex_math_noads_have_luatex_box_dimensions() {
    // (kind, formula): `b` text style, `d` display style, `s` script style
    let cases: [(&str, &str); 62] = [
        ("b", "\\Uroot 3 \"70 {a}{x}"),
        ("b", "\\Uradical 3 \"70 x"),
        ("b", "\\radical\"161 y"),
        ("b", "\\Uradical width 12pt left 3 \"70 x"),
        ("d", "\\Uradical 3 \"70 {x^2_3 \\over y}"),
        ("d", "\\Uroot 3 \"70 {abcd}{x+y}"),
        ("d", "\\Uroot 3 \"70 {}{x+y}"),
        ("s", "\\Uradical 3 \"70 {abcdklmnop}"),
        ("b", "\\Uoverdelimiter 3 \"70 {a}"),
        ("b", "\\Uunderdelimiter 3 \"70 {a}"),
        ("b", "\\Udelimiterover 0 \"28 {b}"),
        ("b", "\\Udelimiterunder 0 \"28 {b}"),
        ("d", "\\Uoverdelimiter 3 \"70 {abcdklmnop}"),
        ("d", "\\Udelimiterover width 40pt left 3 \"70 {ab}"),
        ("d", "\\Udelimiterunder width 40pt right 3 \"70 {ab}"),
        ("d", "\\Uoverdelimiter width 40pt middle 3 \"70 {ab}"),
        ("b", "\\Uhextensible width 20pt 3 \"70"),
        ("b", "\\Uhextensible width 20pt middle 3 \"70"),
        ("d", "\\Uhextensible width 60pt 3 \"70"),
        ("s", "\\Uhextensible width 60pt 3 \"70"),
        ("b", "\\Umathaccent 0 0 \"62 {b}"),
        ("b", "\\Umathaccent bottom 0 0 \"62 {b}"),
        ("b", "\\Umathaccent both fixed 0 1 \"62 0 1 \"63 {b}"),
        ("b", "\\Umathaccent overlay 0 0 \"62 fraction 500 {b}"),
        ("b", "\\Umathaccent 0 0 \"62 {b}^2_3"),
        ("b", "\\mathaccent\"7162 z"),
        ("d", "\\Umathaccent 0 0 \"5E {abcdklm}"),
        ("d", "\\Umathaccent fixed 0 0 \"5E {abcdklm}"),
        ("d", "\\Umathaccent 0 0 \"5E {xy}^2_3"),
        ("d", "\\Umathaccent bottom 0 0 \"5E {x}^2"),
        ("d", "\\Umathaccent 0 0 \"5E {\\Umathaccent 0 0 \"7E {x}}"),
        ("d", "\\Umathaccent overlay 0 0 \"2F {x}"),
        ("d", "\\Umathaccent 0 0 \"5E fraction 2000 {abc}"),
        ("b", "a\\Uskewedwithdelims/()  b"),
        ("b", "a \\Uskewed/ b"),
        ("b", "a \\Uskewed/ exact b"),
        ("b", "{a \\Uskewed/ noaxis b}"),
        ("d", "{a\\over b}\\Uskewed/ c"),
        ("d", "a \\Uskewedwithdelims/() {b \\over c}"),
        ("d", "{a \\atopwithdelims[] b}"),
        ("d", "{a\\above 3pt b}"),
        ("d", "{a\\abovewithdelims.. 2pt b}"),
        ("d", "{a \\above exact 5pt b}"),
        ("d", "{a \\above norule 5pt b}"),
        ("s", "{a \\over b}"),
        ("b", "\\left(a\\right)"),
        ("b", "\\Uleft height 20pt depth 5pt ( a\\Uright )"),
        ("b", "\\Uleft exact height 20pt depth 5pt ( a\\Uright )"),
        ("b", "\\Uleft axis height 20pt depth 5pt ( a\\Uright )"),
        ("b", "\\Uleft class 2 ( a\\Uright class 3 )"),
        ("d", "\\Uleft height 30pt depth 10pt axis [ a \\Umiddle exact height 10pt (\\Uright ["),
        ("d", "\\Uleft height 30pt depth 10pt axis class 3 [ a \\Umiddle class 4 (b \\Uright class 0 )"),
        ("b", "a\\Uvextensible height 20pt (b"),
        ("d", "a\\Uvextensible height 40pt noaxis [ b"),
        ("d", "a\\Uvextensible height 40pt axis exact [ b"),
        ("d", "a\\Uvextensible [ b\\Uvextensible ["),
        ("b", "x\\Unosuperscript{a}"),
        ("b", "x\\Unosubscript{a}"),
        ("b", "x\\Unosubscript{a}\\Usuperscript{b}"),
        ("b", "x\\Usuperscript{b}\\Unosubscript{a}"),
        ("b", "x\\Unosuperscript{a}\\Unosubscript{c}"),
        ("b", "{xy}\\Unosubscript{a}"),
    ];
    let mut body = String::from(MATH_FONTS);
    for (cmd, formula) in cases {
        body.push_str(&format!("\\{cmd}{{{formula}}}\n"));
    }
    let out = run(&body);
    let expected: [&str; 62] = [
        "[17.50005pt:9.00278pt:3.39731pt]",
        "[15.27782pt:9.00278pt:3.39731pt]",
        "[10.41669pt:9.51103pt:1.94444pt]",
        "[15.27782pt:9.00278pt:3.39731pt]",
        "[20.27783pt:21.13069pt:9.26958pt]",
        "[46.5279pt:11.02707pt:3.11743pt]",
        "[28.47229pt:9.28265pt:3.11743pt]",
        "[63.33351pt:9.35pt:3.05008pt]",
        "[10.00002pt:6.30553pt:11.60013pt]",
        "[10.00002pt:0.39998pt:17.57233pt]",
        "[5.55557pt:15.55556pt:0.0pt]",
        "[5.55557pt:6.94444pt:4.16666pt]",
        "[53.3335pt:10.0pt:18.00018pt]",
        "[40.0pt:8.05556pt:0.0pt]",
        "[40.0pt:6.94444pt:19.66684pt]",
        "[40.0pt:8.94443pt:18.00018pt]",
        "[10.55559pt:0.0pt:18.00018pt]",
        "[20.0pt:0.0pt:18.00018pt]",
        "[10.55559pt:0.0pt:18.00018pt]",
        "[10.55559pt:0.0pt:18.00018pt]",
        "[5.55557pt:9.58334pt:0.0pt]",
        "[5.55557pt:6.94444pt:6.94444pt]",
        "[5.55557pt:9.58334pt:4.30554pt]",
        "[5.55557pt:6.94444pt:0.0pt]",
        "[10.55559pt:10.07336pt:4.41544pt]",
        "[4.44444pt:6.94444pt:0.0pt]",
        "[37.22234pt:9.58334pt:0.0pt]",
        "[37.22234pt:9.58334pt:0.0pt]",
        "[15.69449pt:10.57336pt:3.91544pt]",
        "[10.27782pt:10.57336pt:6.94444pt]",
        "[5.2778pt:9.31749pt:0.0pt]",
        "[5.2778pt:7.15277pt:0.0pt]",
        "[15.27782pt:9.58334pt:0.0pt]",
        "[19.7223pt:8.50005pt:3.50006pt]",
        "[10.55559pt:7.5pt:2.5pt]",
        "[15.5556pt:7.5pt:2.5pt]",
        "[10.55559pt:7.5pt:2.5pt]",
        "[10.00002pt:12.32062pt:6.85951pt]",
        "[19.7223pt:10.88176pt:4.69841pt]",
        "[21.11118pt:11.07062pt:6.85951pt]",
        "[5.55557pt:17.30554pt:14.94444pt]",
        "[5.55557pt:13.80554pt:11.44444pt]",
        "[5.55557pt:11.07062pt:8.14438pt]",
        "[5.55557pt:24.30554pt:21.94444pt]",
        "[5.55557pt:8.24286pt:5.04442pt]",
        "[12.77782pt:7.5pt:2.5pt]",
        "[16.8056pt:20.0pt:5.0pt]",
        "[16.8056pt:15.00014pt:15.00015pt]",
        "[16.8056pt:22.5pt:2.5pt]",
        "[12.77782pt:7.5pt:2.5pt]",
        "[24.44452pt:32.5pt:7.5pt]",
        "[26.11119pt:32.5pt:7.5pt]",
        "[17.91673pt:20.0pt:0.0pt]",
        "[18.33339pt:40.0pt:0.0pt]",
        "[18.33339pt:6.94444pt:0.83333pt]",
        "[26.11119pt:6.94444pt:0.83333pt]",
        "[10.27782pt:7.93446pt:0.0pt]",
        "[10.27782pt:4.30554pt:1.49998pt]",
        "[10.83337pt:10.57336pt:2.47217pt]",
        "[10.83337pt:10.57336pt:2.47217pt]",
        "[10.27782pt:7.93446pt:2.47217pt]",
        "[15.69449pt:4.30554pt:2.44444pt]",
    ];
    assert_eq!(out.len(), expected.len());
    for (i, (got, want)) in out.iter().zip(expected).enumerate() {
        assert_eq!(got, want, "case {i}: {}", cases[i].1);
    }
}

#[test]
fn luatex_math_noads_show_lists_like_luatex() {
    let cases: [(&str, &[&str]); 12] = [
        ("a \\Uskewed/ exact noaxis b", &[
            "### math mode entered at line N",
            "\\mathord",
            ".\\fam1 b",
            "this will be denominator of:",
            "\\fraction, thickness 0.0",
            "\\\\mathord",
            "\\.\\fam1 a",
            "### restricted horizontal mode entered at line N",
            "spacefactor 1000",
            "### vertical mode entered at line N",
            "prevdepth ignored",
        ]),
        ("{a \\above exact norule 2pt b}", &[
            "### math mode entered at line N",
            "\\mathord",
            ".\\fraction, thickness 2.0",
            ".\\\\mathord",
            ".\\.\\fam1 a",
            "./\\mathord",
            "./.\\fam1 b",
            "### restricted horizontal mode entered at line N",
            "spacefactor 1000",
            "### vertical mode entered at line N",
            "prevdepth ignored",
        ]),
        ("\\Umathaccent bottom 0 0 \"62 {b}\\Umathaccent 0 0 \"62 fraction 700 {b}", &[
            "### math mode entered at line N",
            "\\Umathaccent bottom\\fam0 b",
            ".\\fam1 b",
            "\\Umathaccent fraction=700 \\fam0 b",
            ".\\fam1 b",
            "### restricted horizontal mode entered at line N",
            "spacefactor 1000",
            "### vertical mode entered at line N",
            "prevdepth ignored",
        ]),
        ("\\Umathaccent both fixed 0 1 \"62 0 1 \"63 {b}", &[
            "### math mode entered at line N",
            "\\Umathaccent both fixed \\fam1 b\\fam1 c",
            ".\\fam1 b",
            "### restricted horizontal mode entered at line N",
            "spacefactor 1000",
            "### vertical mode entered at line N",
            "prevdepth ignored",
        ]),
        ("\\Uroot 3 \"70 {a}{x}", &[
            "### math mode entered at line N",
            "\\Uroot\"370000",
            "/\\fam1 a",
            ".\\fam1 x",
            "### restricted horizontal mode entered at line N",
            "spacefactor 1000",
            "### vertical mode entered at line N",
            "prevdepth ignored",
        ]),
        ("x\\Unosuperscript{a}\\Unosubscript{b}\\Usuperscript{c}", &[
            "### math mode entered at line N",
            "\\mathord",
            ".\\fam1 x",
            "^\\fam1 a",
            "_\\fam1 b",
            "\\mathord",
            ".{}",
            "^\\fam1 c",
            "### restricted horizontal mode entered at line N",
            "spacefactor 1000",
            "### vertical mode entered at line N",
            "prevdepth ignored",
        ]),
        ("x^{\\showlists}", &[
            "### math mode entered at line N",
            "### math mode entered at line N",
            "\\mathord",
            ".\\fam1 x",
            "^\\fam0 ",
        ]),
        ("x_{\\showlists}", &[
            "### math mode entered at line N",
            "### math mode entered at line N",
            "\\mathord",
            ".\\fam1 x",
            "_\\fam0 ",
        ]),
        ("\\Umathaccent 0 0 \"62 {\\showlists}", &[
            "### math mode entered at line N",
            "### math mode entered at line N",
            "\\Umathaccent\\fam0 b",
            ".\\fam0 ",
        ]),
        ("\\overline{\\showlists}", &[
            "### math mode entered at line N",
            "### math mode entered at line N",
            "\\overline",
            ".\\fam0 ",
        ]),
        ("\\mathord{\\showlists}", &[
            "### math mode entered at line N",
            "### math mode entered at line N",
            "\\mathord",
            ".\\fam0 ",
        ]),
        ("{a\\over \\showlists}", &[
            "### math mode entered at line N",
            "this will be denominator of:",
            "\\fraction, thickness = default",
            "\\\\mathord",
            "\\.\\fam1 a",
            "### math mode entered at line N",
            "\\mathord",
            ".\\fam0 ",
        ]),
    ];
    for (formula, want) in cases {
        let shown = if formula.contains("\\showlists") { formula.to_string() } else { format!("{formula}\\showlists") };
        let got = run_lists(&format!("\\setbox0\\hbox{{${shown}$}}"));
        assert_eq!(got, want, "{formula}");
    }
}
