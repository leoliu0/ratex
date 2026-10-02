//! XeTeX math primitives and math-code values (`\Umathcode`, `\Umathchardef`,
//! `\Udelcode`, `\Udelimiter`, `\Uradical`, `\Umathaccent`, the `\XeTeXmath*`
//! aliases, `\mathcode`/`\delcode` in XeTeX's packed layout).
//!
//! EVERY expectation in this file was produced by running TeX Live 2026's
//! `xetex -ini -etex` on the same source (`PRE` + the case): `[..]` lines are
//! `\write16` output, errors are the `! ` lines of the log without the final
//! period, `{..}` lines are `\tracingassigns`/`\tracingrestores` output and
//! blocks are the `\showlists`/`\showbox` text.

use tex_core::engine::{Engine, EngineKind, InteractionMode};

const PRE: &str = r"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\$=3 \catcode`\^=7 \catcode`\_=8 \catcode`\~=13
\tracingonline1 \showboxbreadth100 \showboxdepth100
\def\v#1{\immediate\write16{[#1]}}
";

fn run(src: &str) -> Engine {
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng.input.push_file("test.tex".into(), format!("{PRE}{src}").into_bytes());
    eng.run();
    eng
}

fn check_values(src: &str, vals: &[&str], errors: &[&str]) {
    let eng = run(src);
    let got: Vec<&str> = eng.term.lines().filter(|l| l.starts_with('[') && l.ends_with(']')).collect();
    assert_eq!(got, vals, "values; term: {}", eng.term);
    let errs: Vec<&str> = eng
        .diagnostics
        .iter()
        .map(|d| d.original_message.as_deref().unwrap_or(d.message.as_str()))
        .collect();
    assert_eq!(errs, errors, "errors");
}

fn check_trace(src: &str, expected: &[&str]) {
    let eng = run(src);
    let mut got = Vec::new();
    let text = &eng.log;
    let mut rest = text.as_str();
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find(|c| c == '{' || c == '}') else { break };
        if after.as_bytes()[close] == b'}' {
            let body = &after[..close];
            if ["changing ", "into ", "reassigning ", "globally changing ", "restoring ", "retaining "]
                .iter()
                .any(|p| body.starts_with(p))
            {
                got.push(body.to_string());
            }
            rest = &after[close + 1..];
        } else {
            rest = &after[close..];
        }
    }
    assert_eq!(got, expected, "log: {}", eng.log);
}

fn check_block(src: &str, expected: &str) {
    let eng = run(src);
    let lines: Vec<&str> = eng.term.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.starts_with("### ") || l.starts_with("> \\box"))
        .unwrap_or_else(|| panic!("no show output; term: {}", eng.term));
    let got = lines[start..].join("\n");
    assert_eq!(got.trim_end(), expected, "term: {}", eng.term);
}

/// The math codes, delimiter codes and `\Umathchardef`s of a format.
/// Expectations are the values `xetex -ini` reports after the same assignments.
#[test]
fn math_codes_survive_a_format_round_trip() {
    let eng = run(
        r#"\Umathcode`A="3 "5 "1D400 \Umathcodenum`B="1FFFFF \mathcode`C="7143 \Umathcode"3B1="1 "2 "3B1
\Udelcode`D="3 "1D400 \delcode`E="161362 \Umathchardef\foo="2 "7 "10FFF \Umathcode`F="7 "FF "10FFFF
\dump"#,
    );
    let path = std::env::temp_dir().join(format!("xetex_math_{}.fmt", std::process::id()));
    tex_core::format::save_format(&eng, &path).expect("save format");
    let loaded = tex_core::format::load_format(&path);
    let _ = std::fs::remove_file(&path);
    let mut eng = loaded.expect("load format");
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng.input.push_file(
        "test.tex".into(),
        format!(
            "{PRE}\\v{{\\the\\Umathcodenum`A,\\the\\Umathcodenum`B,\\the\\Umathcodenum`C,\\the\\Umathcodenum\"3B1,\\the\\Umathcodenum`F}}\\v{{\\the\\Udelcodenum`D,\\the\\delcode`E,\\the\\Udelcodenum`.}}\\v{{\\meaning\\foo,\\meaning\\Umathcode}}\\end"
        )
        .into_bytes(),
    );
    eng.run();
    let got: Vec<&str> = eng.term.lines().filter(|l| l.starts_with('[') && l.ends_with(']')).collect();
    assert_eq!(
        got,
        [
            "[90297344,2097151,31457347,35652529,4293984255]",
            "[1080153088,1446754,0]",
            "[\\Umathchar\"2\"7\"10FFF,\\Umathcode]"
        ],
        "term: {}",
        eng.term
    );
}

#[test]
fn defaults_of_math_and_delimiter_codes() {
    check_values(
        r####"
\v{A:\the\Umathcodenum`A,\the\mathcode`A}
\v{a:\the\Umathcodenum`a,\the\mathcode`a}
\v{0:\the\Umathcodenum`0,\the\mathcode`0}
\v{9:\the\Umathcodenum`9,\the\mathcode`9}
\v{bang:\the\Umathcodenum`!,\the\mathcode`!}
\v{e-acute:\the\Umathcodenum"E9,\the\mathcode"E9}
\v{alpha:\the\Umathcodenum"3B1}
\v{plane1:\the\Umathcodenum"1D400,\the\Umathcodenum"10FFFF}
\v{alpha-as-mathcode:\the\mathcode"3B1}
\v{del-period:\the\delcode`.,\the\Udelcodenum`.}
\v{del-A:\the\delcode`A,\the\Udelcodenum`A,\the\delcode"3B1}
\v{count:\number\Umathcodenum`A,\number\mathcode`A}
\end
"####,
        &["[A:31457345,28993]", "[a:31457377,29025]", "[0:14680112,28720]", "[9:14680121,28729]", "[bang:33,33]", "[e-acute:233,233]", "[alpha:945]", "[plane1:119808,1114111]", "[alpha-as-mathcode:0]", "[del-period:0,0]", "[del-A:-1,-1,-1]", "[count:31457345,28993]"],
        &["Extended mathchar used as mathchar (945)"],
    );
}

#[test]
fn umathcode_forms_and_ranges() {
    check_values(
        r####"
\Umathcode`A="3 "5 "1D400
\v{A:\the\Umathcodenum`A}
\v{A-as-mathcode:\the\mathcode`A}
\Umathcode`B="7 "FF "10FFFF
\v{B:\the\Umathcodenum`B}
\Umathcode"3B1="1 "2 "3B1
\v{alpha:\the\Umathcodenum"3B1}
\Umathcode`C="8 "1 "41
\v{C:\the\Umathcodenum`C}
\Umathcode`D="1 "100 "41
\v{D:\the\Umathcodenum`D}
\Umathcode`E="1 "1 "110000
\v{E:\the\Umathcodenum`E}
\Umathcode"110000="1 "1 "41
\Umathcode-1="1 "1 "41
\XeTeXmathcode`F="2 "3 "46
\v{F:\the\XeTeXmathcodenum`F}
\end
"####,
        &["[A:90297344]", "[A-as-mathcode:0]", "[B:4293984255]", "[alpha:35652529]", "[C:16777281]", "[D:2097217]", "[E:18874368]", "[F:54526022]"],
        &["Extended mathchar used as mathchar (90297344)", "Bad math class (8)", "Bad math family (256)", "Bad character code (1114112)", "Bad character code (1114112)", "Bad character code (-1)"],
    );
}

#[test]
fn umathcodenum_values() {
    check_values(
        r####"
\Umathcodenum`A="3051D400
\v{A:\the\Umathcodenum`A}
\Umathcodenum`B="7FFFFFF
\v{B:\the\Umathcodenum`B}
\Umathcodenum`C="1FFFFF
\v{C:\the\Umathcodenum`C,\the\mathcode`C}
\Umathcodenum`D=-1
\v{D:\the\Umathcodenum`D}
\Umathcodenum`E="110000
\v{E:\the\Umathcodenum`E}
\Umathcodenum`F="7FF00041
\v{F:\the\Umathcodenum`F}
\XeTeXmathcodenum`G="2000047
\v{G:\the\Umathcodenum`G}
\end
"####,
        &["[A:0]", "[B:2097151]", "[C:2097151,32768]", "[D:2097151]", "[E:0]", "[F:2146435137]", "[G:33554503]"],
        &["Bad XeTeX math character code (810669056)", "Bad active XeTeX math code (134217727)", "Bad active XeTeX math code (-1)", "Bad XeTeX math character code (1114112)"],
    );
}

#[test]
fn mathcode_assignment_and_conversion() {
    check_values(
        r####"
\mathcode`A="7142
\v{A:\the\mathcode`A,\the\Umathcodenum`A}
\mathcode`B="8000
\v{B:\the\mathcode`B,\the\Umathcodenum`B}
\mathcode`C="7FFF
\v{C:\the\mathcode`C,\the\Umathcodenum`C}
\mathcode`D=32769
\v{D:\the\mathcode`D}
\mathcode`E=-1
\v{E:\the\mathcode`E}
\mathcode"3B1="2323
\v{alpha:\the\mathcode"3B1,\the\Umathcodenum"3B1}
\mathcode"110000=1
\Umathcode`F="1 "10 "41
\v{F-as-mathcode:\the\mathcode`F}
\Umathcode`G="9 "1 "41
\Umathcode`H="1 "1 "141
\v{H-as-mathcode:\the\mathcode`H}
\end
"####,
        &["[A:28994,31457346]", "[B:32768,2097151]", "[C:32767,266338559]", "[D:0]", "[E:0]", "[alpha:8995,54525987]", "[F-as-mathcode:0]", "[H-as-mathcode:0]"],
        &["Invalid code (32769), should be in the range 0..32768", "Invalid code (-1), should be in the range 0..32768", "Bad character code (1114112)", "Extended mathchar used as mathchar (270532673)", "Bad math class (9)", "Extended mathchar used as mathchar (18874689)"],
    );
}

#[test]
fn udelcode_forms_and_ranges() {
    check_values(
        r####"
\Udelcode`A="3 "1D400
\v{A:\the\Udelcodenum`A}
\v{A-as-delcode:\the\delcode`A}
\Udelcode`B="1 "300 "41
\v{B:\the\Udelcodenum`B}
\Udelcode`C="FF "10FFFF
\v{C:\the\Udelcodenum`C}
\Udelcode`D="1 "110000
\v{D:\the\Udelcodenum`D}
\Udelcode"110000="1 "41
\XeTeXdelcode`E="2 "45
\v{E:\the\XeTeXdelcodenum`E}
\v{Udelcode-as-number:\the\Udelcode`A}
\v{Umathcode-as-number:\the\Umathcode`A}
\end
"####,
        &["[A:1080153088]", "[A-as-delcode:0]", "[B:1075839744]", "[C:1609629695]", "[D:1075838976]", "[E:1077936197]", "[Udelcode-as-number:0]", "[Umathcode-as-number:0]"],
        &["Extended delcode used as delcode", "Bad character code (1114112)", "Bad character code (1114112)", "Can't use \\Udelcode as a number (try \\Udelcodenum)", "Can't use \\Umathcode as a number (try \\Umathcodenum)"],
    );
}

#[test]
fn delcode_assignment_and_udelcodenum() {
    check_values(
        r####"
\delcode`A="161362
\v{A:\the\delcode`A,\the\Udelcodenum`A}
\delcode`B=16777216
\v{B:\the\delcode`B}
\delcode`C=-5
\v{C:\the\delcode`C,\the\Udelcodenum`C}
\delcode`D=16777215
\v{D:\the\delcode`D}
\Udelcodenum`E=-7
\v{E:\the\delcode`E,\the\Udelcodenum`E}
\Udelcodenum`F="7FFFFFFF
\v{F:\the\Udelcodenum`F}
\Udelcodenum`G=1073741824
\v{G:\the\Udelcodenum`G}
\XeTeXdelcodenum`H="12345678
\v{H:\the\Udelcodenum`H,\the\delcode`H}
\delcode"3B1="123456
\v{alpha:\the\delcode"3B1,\the\Udelcodenum"3B1}
\delcode"110000=1
\end
"####,
        &["[A:1446754,1446754]", "[B:0]", "[C:-5,-5]", "[D:16777215]", "[E:-7,-7]", "[F:2147483647]", "[G:1073741824]", "[H:305419896,305419896]", "[alpha:1193046,1193046]"],
        &["Invalid code (16777216), should be at most 16777215", "Bad character code (1114112)"],
    );
}

#[test]
fn shorthand_definitions() {
    check_values(
        r####"
\Umathchardef\foo="2 "7 "10FFF
\v{foo:\meaning\foo,\the\foo,\number\foo}
\Umathcharnumdef\bar="7B1D400
\v{bar:\meaning\bar,\the\bar}
\Umathcharnumdef\baz="1FFFFF
\v{baz:\meaning\baz,\the\baz}
\XeTeXmathchardef\qux="1 "7F "10FFFF
\v{qux:\meaning\qux,\the\qux}
\XeTeXmathcharnumdef\quux="3000041
\v{quux:\meaning\quux}
\Umathchardef\bad="8 "1 "41
\v{bad:\meaning\bad}
\Umathchardef\bad="1 "256 "41
\v{bad:\meaning\bad}
\Umathchardef\bad="1 "1 "110000
\v{bad:\meaning\bad}
\Umathcharnumdef\bad="1FFFFE
\v{bad:\meaning\bad}
\Umathcharnumdef\bad="3FFFFF
\v{bad:\meaning\bad}
\mathchardef\old="7123
\v{old:\meaning\old,\the\old}
\mathchardef\old="8000
\v{old:\meaning\old}
\mathchardef\old=-1
\v{old:\meaning\old}
\v{ifx:\ifx\foo\foo T\else F\fi\ifx\bar\baz T\else F\fi}
\end
"####,
        &["[foo:\\Umathchar\"2\"7\"10FFF,121704447,121704447]", "[bar:\\Umathchar\"0\"0\"0,0]", "[baz:\\Umathchar\"0\"0\"1FFFFF,2097151]", "[qux:\\Umathchar\"1\"7F\"10FFFF,2133917695]", "[quux:\\Umathchar\"0\"3\"41]", "[bad:\\Umathchar\"0\"1\"41]", "[bad:\\Umathchar\"1\"0\"41]", "[bad:\\Umathchar\"1\"1\"0]", "[bad:\\Umathchar\"0\"0\"0]", "[bad:\\Umathchar\"0\"0\"1FFFFF]", "[old:\\mathchar\"7123,28963]", "[old:\\mathchar\"0]", "[old:\\mathchar\"0]", "[ifx:TF]"],
        &["Bad XeTeX math character code (129094656)", "Bad math class (8)", "Bad math family (598)", "Bad character code (1114112)", "Bad XeTeX math character code (2097150)", "Bad active XeTeX math code (4194303)", "Bad mathchar (32768)", "Bad mathchar (-1)"],
    );
}

#[test]
fn meanings_of_names_and_aliases() {
    check_values(
        r####"
\v{\meaning\Umathcode}
\v{\meaning\XeTeXmathcode}
\v{\meaning\Umathcodenum}
\v{\meaning\XeTeXmathcodenum}
\v{\meaning\Udelcode}
\v{\meaning\XeTeXdelcode}
\v{\meaning\Udelcodenum}
\v{\meaning\XeTeXdelcodenum}
\v{\meaning\Umathchardef}
\v{\meaning\XeTeXmathchardef}
\v{\meaning\Umathcharnumdef}
\v{\meaning\XeTeXmathcharnumdef}
\v{\meaning\Umathchar}
\v{\meaning\XeTeXmathchar}
\v{\meaning\Umathcharnum}
\v{\meaning\XeTeXmathcharnum}
\v{\meaning\Udelimiter}
\v{\meaning\XeTeXdelimiter}
\v{\meaning\Uradical}
\v{\meaning\XeTeXradical}
\v{\meaning\Umathaccent}
\v{\meaning\XeTeXmathaccent}
\v{\meaning\mathchar}
\v{\meaning\delimiter}
\v{\meaning\radical}
\v{\meaning\mathaccent}
\v{\meaning\mathcode}
\v{\meaning\delcode}
\v{\meaning\mathchardef}
\v{\ifx\Umathcode\XeTeXmathcode T\else F\fi\ifx\Uradical\radical T\else F\fi}
\end
"####,
        &["[\\Umathcode]", "[\\Umathcode]", "[\\Umathcodenum]", "[\\Umathcodenum]", "[\\Udelcode]", "[\\Udelcode]", "[\\Udelcodenum]", "[\\Udelcodenum]", "[\\Umathchardef]", "[\\Umathchardef]", "[\\Umathcharnumdef]", "[\\Umathcharnumdef]", "[\\Umathchar]", "[\\Umathchar]", "[\\Umathcharnum]", "[\\Umathcharnum]", "[\\Udelimiter]", "[\\Udelimiter]", "[\\Uradical]", "[\\Uradical]", "[\\Umathaccent]", "[\\Umathaccent]", "[\\mathchar]", "[\\delimiter]", "[\\radical]", "[\\mathaccent]", "[\\mathcode]", "[\\delcode]", "[\\mathchardef]", "[TF]"],
        &[],
    );
}

#[test]
fn math_code_scoping_and_global() {
    check_values(
        r####"
\Umathcode`A="3 "5 "1D400
{\Umathcode`A="1 "1 "41 \global\Umathcode`B="1 "1 "42 \mathcode`C="7143
 \Udelcode`A="1 "41 \global\delcode`B="123456 \Umathcodenum`D="2000044
 \v{in:\the\Umathcodenum`A,\the\Umathcodenum`B,\the\Umathcodenum`C}}
\v{out:\the\Umathcodenum`A,\the\Umathcodenum`B,\the\Umathcodenum`C,\the\Umathcodenum`D}
\v{del:\the\Udelcodenum`A,\the\Udelcodenum`B}
{\Umathcode"3B1="1 "2 "3B1 \Udelcode"3B1="1 "3B1 {\global\Umathcode"3B2="1 "2 "3B2}}
\v{greek:\the\Umathcodenum"3B1,\the\Umathcodenum"3B2,\the\Udelcodenum"3B1}
\end
"####,
        &["[in:18874433,18874434,31457347]", "[out:90297344,18874434,31457347,31457348]", "[del:-1,1193046]", "[greek:945,35652530,-1]"],
        &[],
    );
}

#[test]
fn math_code_tracing_assigns_and_restores() {
    check_trace(
        r####"
\tracingassigns=1 \tracingrestores=1
\Umathcode`A="3 "5 "1D400
\Umathcode`A="3 "5 "1D400
{\Umathcode`A="1 "1 "41 \global\Umathcode`B="1 "1 "41 \Umathcode"3B1="1 "1 "3B1 \mathcode`C="7143 \mathcode`C="7143 \mathcode`D="8000}
\Udelcode`A="3 "1D400
\Udelcode`A="3 "1D400
{\Udelcode`A="1 "41 \delcode`B="123456 \Udelcodenum`C="7FFFFFFF \delcode`C=-1}
\mathcode`1="7131
\Umathcodenum"3B1="3B1
\Umathchardef\foo="2 "7 "10FFF
{\Umathchardef\foo="2 "7 "10FFF \Umathcharnumdef\bar="1D400}
\mathchardef\baz="7123
\end
"####,
        &["into \\tracingassigns=1", "changing \\tracingrestores=0", "into \\tracingrestores=1", "changing \\mathcode65=31457345", "into \\mathcode65=90297344", "reassigning \\mathcode65=90297344", "changing \\mathcode65=90297344", "into \\mathcode65=18874433", "globally changing \\mathcode66=31457346", "into \\mathcode66=18874433", "changing \\mathcode945=945", "into \\mathcode945=18875313", "reassigning \\mathcode67=31457347", "reassigning \\mathcode67=31457347", "changing \\mathcode68=31457348", "into \\mathcode68=2097151", "restoring \\mathcode68=31457348", "restoring \\mathcode945=945", "restoring \\mathcode65=90297344", "changing \\delcode65=-1", "into \\delcode65=1080153088", "reassigning \\delcode65=1080153088", "changing \\delcode65=1080153088", "into \\delcode65=1075839041", "changing \\delcode66=-1", "into \\delcode66=1193046", "changing \\delcode67=-1", "into \\delcode67=2147483647", "changing \\delcode67=2147483647", "into \\delcode67=-1", "restoring \\delcode67=-1", "restoring \\delcode66=-1", "restoring \\delcode65=1080153088", "changing \\mathcode49=14680113", "into \\mathcode49=31457329", "reassigning \\mathcode945=945", "changing \\foo=undefined", "into \\foo=\\relax", "changing \\foo=\\relax", "into \\foo=\\Umathchar\"2\"7\"10FFF", "changing \\foo=\\Umathchar\"2\"7\"10FFF", "into \\foo=\\relax", "changing \\foo=\\relax", "into \\foo=\\Umathchar\"2\"7\"10FFF", "changing \\bar=undefined", "into \\bar=\\relax", "changing \\bar=\\relax", "into \\bar=\\Umathchar\"0\"0\"1D400", "restoring \\bar=undefined", "restoring \\foo=\\Umathchar\"2\"7\"10FFF", "changing \\baz=undefined", "into \\baz=\\relax", "changing \\baz=\\relax", "into \\baz=\\mathchar\"7123"],
    );
}

#[test]
fn math_char_given_in_arithmetic() {
    check_values(
        r####"
\Umathchardef\foo="2 "7 "10FFF
\count1=\foo \v{count:\the\count1}
\count2=\Umathcodenum`A \v{count:\the\count2}
\count3=\mathcode`A \v{count:\the\count3}
\count4=\mathcode`1 \v{count:\the\count4}
\end
"####,
        &["[count:121704447]", "[count:31457345]", "[count:28993]", "[count:28721]"],
        &[],
    );
}

#[test]
fn showlists_extended_math_characters() {
    check_block(
        r####"
\Udelcode`(="1 "3B1
\Udelcode`)="2 "1F600
\delcode`[="161362
\delcode`]="161362
\Umathcode`X="3 "5 "1D400
\Umathchardef\foo="2 "7 "10FFF
\Umathcharnumdef\bar="1D400
\mathchardef\baz="7123
\hbox{$\Umathchar"1 "2 "3B1 \Umathcharnum"1D400 \foo \bar \baz X é \mathchar"7141 \mathchar"2142 \delimiter"4162362 \Udelimiter"2 "3 "1F600
\Umathaccent "2 "3 "4 x \Umathaccent fixed "2 "3 "4 x \Umathaccent bottom "2 "3 "4 x \Umathaccent bottom fixed "2 "3 "4 x \mathaccent"7141 y
\radical"161362 x \Uradical "3 "1F600 y \Uradical 300 "1F600 {}
\left( x \middle[ y \right) \left. a \right.
\showlists
$}
\end
"####,
        "### math mode entered at line 13\n\\mathop\n.\\fam2 α\n\\mathord\n.\\fam0 𝐀\n\\mathbin\n.\\fam7 𐿿\n\\mathord\n.\\fam0 𝐀\n\\mathord\n.\\fam1 #\n\\mathrel\n.\\fam5 𝐀\n\\mathord\n.\\fam0 é\n\\mathord\n.\\fam1 A\n\\mathbin\n.\\fam1 B\n\\mathopen\n.\\fam1 b\n\\mathbin\n.\\fam3 😀\n\\accent\\fam3 ^^D\n.\\fam1 x\n\\accent\\fam3 ^^D\\limits\n.\\fam1 x\n\\accent\\fam3 ^^D\\nolimits\n.\\fam1 x\n\\accent\\fam3 ^^D\\nolimits\n.\\fam1 x\n\\accent\\fam1 A\n.\\fam1 y\n\\radical\"161362\n.\\fam1 x\n\\radical\"1F900000\n.\\fam1 y\n\\radical\"1F600000\n.{}\n\\mathinner\n.\\left\"4B1000\n.\\mathord\n..\\fam1 x\n.\\middle\"161362\n.\\mathord\n..\\fam1 y\n.\\right\"1F800000\n\\mathinner\n.\\left\"0\n.\\mathord\n..\\fam1 a\n.\\right\"0\n### restricted horizontal mode entered at line 13\nspacefactor 1000\n### vertical mode entered at line 0\nprevdepth ignored",
    );
}

#[test]
fn showlists_variable_family_and_fam() {
    check_block(
        r####"
\Umathcode`X="7 "3 "58
\mathcode`Y="7159
\Umathchardef\vv="7 "4 "3B1
\hbox{$\fam5 \Umathchar"7 "9 "41 X Y A 1 \vv \mathchar"7142 \Umathaccent "7 "7 "41 x \mathaccent"7141 y
\fam254 A X \fam256 A X \fam-1 A X
\showlists
$}
\end
"####,
        "### math mode entered at line 8\n\\mathord\n.\\fam5 A\n\\mathord\n.\\fam5 X\n\\mathord\n.\\fam5 Y\n\\mathord\n.\\fam5 A\n\\mathord\n.\\fam5 1\n\\mathord\n.\\fam5 α\n\\mathord\n.\\fam5 B\n\\accent\\fam5 A\n.\\fam5 x\n\\accent\\fam5 A\n.\\fam5 y\n\\mathord\n.\\fam254 A\n\\mathord\n.\\fam254 X\n\\mathord\n.\\fam1 A\n\\mathord\n.\\fam3 X\n\\mathord\n.\\fam1 A\n\\mathord\n.\\fam3 X\n### restricted horizontal mode entered at line 8\nspacefactor 1000\n### vertical mode entered at line 0\nprevdepth ignored",
    );
}

#[test]
fn showlists_scripts_and_math_groups() {
    check_block(
        r####"
\Umathchardef\foo="2 "7 "10FFF
\hbox{$x^\Umathchar"7 "3 "3B1 _\foo \mathop\foo \mathbin\Umathcharnum"1D400 \mathrel{\foo\foo} \mathpunct{\mathchar"7141}
\left\Udelimiter"1 "2 "1F600 \foo \right\delimiter"4162362
\showlists
$}
\end
"####,
        "### math mode entered at line 6\n\\mathord\n.\\fam1 x\n^\\fam3 α\n_\\fam7 𐿿\n\\mathop\n.\\fam7 𐿿\n\\mathbin\n.\\fam0 𝐀\n\\mathrel\n.\\mathbin\n..\\fam7 𐿿\n.\\mathbin\n..\\fam7 𐿿\n\\mathpunct\n.\\fam1 A\n\\mathinner\n.\\left\"1F800000\n.\\mathbin\n..\\fam7 𐿿\n.\\right\"162362\n### restricted horizontal mode entered at line 6\nspacefactor 1000\n### vertical mode entered at line 0\nprevdepth ignored",
    );
}

#[test]
fn showlists_active_math_characters() {
    check_block(
        r####"
\def\foo{\Umathchar"2 "5 "3B0 }
\Umathcode`+="0 "0 "1FFFFF
\Umathcodenum`<="1FFFFF
\mathcode`>="8000
\Umathcode"3B1="7 "2 "1FFFFF
\catcode`+=13 \def+{\Umathchar"2 "1 "3B2 }
\catcode`<=13 \def<{\Umathchar"3 "2 "3B3 }
\catcode`>=13 \def>{\foo}
\catcode"3B1=13 \defα{\Umathchar"4 "3 "3B5 }
\hbox{$+ < > α x^+ \mathop< \showlists
$}
\end
"####,
        "### math mode entered at line 14\n\\mathbin\n.\\fam1 β\n\\mathrel\n.\\fam2 γ\n\\mathbin\n.\\fam5 ΰ\n\\mathopen\n.\\fam3 ε\n\\mathord\n.\\fam1 x\n^\\fam1 β\n\\mathop\n.\\fam2 γ\n### restricted horizontal mode entered at line 14\nspacefactor 1000\n### vertical mode entered at line 0\nprevdepth ignored",
    );
}

#[test]
fn math_commands_in_horizontal_mode() {
    check_block(
        r####"
\Umathchardef\foo="2 "7 "10FFF
\setbox0\hbox{\Umathchar"1 "2 "3B1 \showlists
}
\end
"####,
        "### math mode entered at line 6\n\\mathop\n.\\fam2 α\n### restricted horizontal mode entered at line 6\nspacefactor 1000\n### vertical mode entered at line 0\nprevdepth ignored",
    );
}

#[test]
fn errors_in_math_scanning() {
    check_values(
        r####"
\hbox{$\mathchar"8000 \mathchar-1 \delimiter"8000000 \radical"8000000 x \mathaccent"8000 x
\Umathchar"8 "1 "41 \Umathchar"1 "256 "41 \Umathchar"1 "1 "110000 \Umathcharnum"110000 \Umathcharnum"1FFFFE
\Uradical 256 "41 x \Uradical "1 "110000 x \Umathaccent "8 "1 "41 x \Umathaccent fixed "1 "1 "110000 x
\Udelimiter "8 "1 "41 \Udelimiter "1 "300 "41 \Udelimiter "1 "1 "110000
\left x \right\relax \left\Udelimiter "1 "256 "41 \right. \left\delimiter"8000000 \right.
{a\abovewithdelims x y 1pt b}
{a\overwithdelims . y b}
$}
\end
"####,
        &[],
        &["Bad mathchar (32768)", "Bad mathchar (-1)", "Bad delimiter code (134217728)", "Bad delimiter code (134217728)", "Bad mathchar (32768)", "Bad math class (8)", "Bad math family (598)", "Bad character code (1114112)", "Bad XeTeX math character code (1114112)", "Bad XeTeX math character code (2097150)", "Bad math family (256)", "Bad character code (1114112)", "Bad math class (8)", "Bad character code (1114112)", "Bad math class (8)", "Bad math family (768)", "Bad character code (1114112)", "Missing delimiter (. inserted)", "Missing delimiter (. inserted)", "Bad math family (598)", "Bad delimiter code (134217728)", "Missing delimiter (. inserted)", "Missing delimiter (. inserted)", "Missing number, treated as zero", "Illegal unit of measure (pt inserted)", "Missing delimiter (. inserted)"],
    );
}

#[test]
fn math_commands_outside_math_insert_a_dollar() {
    check_values(
        r####"
\Umathchardef\foo="2 "7 "10FFF
\mathchardef\old="7123
\setbox0\hbox{\Umathchar"1 "2 "3B1 $}
\setbox0\hbox{\Umathcharnum"1D400 $}
\setbox0\hbox{\Udelimiter"1 "2 "3B1 $}
\setbox0\hbox{\Uradical "1 "3B1 x$}
\setbox0\hbox{\Umathaccent "1 "1 "3B1 x$}
\setbox0\hbox{\foo $}
\setbox0\hbox{\old $}
\setbox0\hbox{\mathchar"141 $}
\setbox0\hbox{\delimiter"4162362 $}
\end
"####,
        &[],
        &["Missing $ inserted", "Missing $ inserted", "Missing $ inserted", "Missing $ inserted", "Missing $ inserted", "Missing $ inserted", "Missing $ inserted", "Missing $ inserted", "Missing $ inserted"],
    );
}

#[test]
fn showlists_delimiters_and_radicals_scanning() {
    check_block(
        r####"
\Udelcode`(="1 "3B1
\Udelcode`)="2 "1F600
\delcode`[="161362
\delcode`.="0
\Udelcodenum`|="40000000
\delcode`/="7FFFFFF
\delcode`<="8000000
\Udelcodenum`>=1073741823
\Udelcodenum`?="5FFFFFFF
\hbox{$\left( \left[ \left. \left| \left/ \left< \left> \left? \left\Udelimiter"3 "7 "3B1
\right\delimiter"4162362 \right) \right] \right. \right| \right/ \right< \right> \right?
\radical"7FFFFFF {} \radical 0 {} \Uradical 255 "10FFFF {}
a\abovewithdelims( ) 1pt b
\showlists
$}
\end
"####,
        "### math mode entered at line 14\n\\mathord\n.\\fam1 b\nthis will begin denominator of:\n\\fraction, thickness 1.0, left-delimiter \"4B1000, right-delimiter \"1F800000\n\\\\mathinner\n\\.\\left\"4B1000\n\\.\\mathinner\n\\..\\left\"161362\n\\..\\mathinner\n\\...\\left\"0\n\\...\\mathinner\n\\....\\left\"0\n\\....\\mathinner\n\\.....\\left\"0\n\\.....\\mathinner\n\\......\\left\"0\n\\......\\mathinner\n\\.......\\left\"FFFFFF\n\\.......\\mathinner\n\\........\\left\"20FEFF000\n\\........\\mathinner\n\\.........\\left\"AB1000\n\\.........\\right\"162362\n\\........\\right\"1F800000\n\\.......\\right\"0\n\\......\\mathord\n\\.......\\fam0 ]\n\\......\\right\"0\n\\.....\\right\"0\n\\....\\right\"0\n\\...\\right\"0\n\\..\\right\"FFFFFF\n\\.\\right\"20FEFF000\n\\\\radical\"FFFFFF\n\\.{}\n\\\\radical\"0\n\\.{}\n\\\\radical\"11FEFF000\n\\.{}\n\\\\mathord\n\\.\\fam1 a\n### restricted horizontal mode entered at line 14\nspacefactor 1000\n### vertical mode entered at line 0\nprevdepth ignored",
    );
}

#[test]
fn showlists_accent_forms() {
    check_block(
        r####"
\hbox{$\fam3 \Umathaccent "7 "1 "41 x
\Umathaccent "0 "255 "42 x
\Umathaccent fixed "0 "1 "43 x_1^2
\Umathaccent bottom "2 "3 "44 {xy}
\Umathaccent bottom fixed "1 "5 "1D400 \Umathchar"1 "2 "3B1
\mathaccent"7141 x \mathaccent"0142 {} \mathaccent"2143 \mathchar"2144
\showlists
$}
\end
"####,
        "### math mode entered at line 5\n\\accent\\fam3 A\n.\\fam3 x\n\\accent\\fam0 B\n.\\fam3 x\n\\accent\\fam1 C\\limits\n.\\fam3 x\n^\\fam3 2\n_\\fam3 1\n\\accent\\fam3 D\\nolimits\n.\\mathord\n..\\fam3 x\n.\\mathord\n..\\fam3 y\n\\accent\\fam5 𝐀\\nolimits\n.\\fam2 α\n\\accent\\fam3 A\n.\\fam3 x\n\\accent\\fam1 B\n.{}\n\\accent\\fam1 C\n.\\fam1 D\n### restricted horizontal mode entered at line 5\nspacefactor 1000\n### vertical mode entered at line 0\nprevdepth ignored",
    );
}

#[test]
fn showlists_math_codes_of_large_characters() {
    check_block(
        r####"
\Umathcode"3B1="7 "8 "3B1
\Umathcode"1D44E="3 "9 "1D44E
\Umathcode"10FFFF="1 "1 "10FFFF
\Umathcode"A0="2 "1 "A0
\Umathcode"85="2 "1 "85
\Umathcode"7F="2 "1 "7F
\Umathcode"1="2 "1 "1
\Umathcode"D800="2 "1 "D800
\catcode"3B1=11
\hbox{$α 𝑎 é ü \showlists
$}
\end
"####,
        "### math mode entered at line 14\n\\mathord\n.\\fam8 α\n\\mathrel\n.\\fam9 𝑎\n\\mathord\n.\\fam0 é\n\\mathord\n.\\fam0 ü\n### restricted horizontal mode entered at line 14\nspacefactor 1000\n### vertical mode entered at line 0\nprevdepth ignored",
    );
}

#[test]
fn math_families_up_to_255() {
    check_values(
        r####"
\textfont255=\nullfont \scriptfont200=\nullfont \scriptscriptfont255=\nullfont
\v{\the\textfont255,\the\scriptfont200,\the\scriptscriptfont255}
\textfont16=\nullfont \v{\the\textfont16}
\textfont256=\nullfont
\scriptfont-1=\nullfont
\scriptscriptfont300=\nullfont
{\textfont17=\nullfont}
\v{\the\textfont17}
\end
"####,
        &["[\\nullfont ,\\nullfont ,\\nullfont ]", "[\\nullfont ]", "[\\nullfont ]"],
        &["Bad math family (256)", "Bad math family (-1)", "Bad math family (300)"],
    );
}
