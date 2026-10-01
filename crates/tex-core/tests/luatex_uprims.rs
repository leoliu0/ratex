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
