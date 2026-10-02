//! LuaTeX math typesetting details checked against `luatex --ini` (TeX Live
//! 2026): script options, scripts on non-noad tails, math rule subtypes,
//! `node.make_extensible`. Every expectation is the output of luatex for the
//! same input.

use tex_core::engine::{Engine, EngineKind, InteractionMode};

fn run(body: &str) -> Vec<String> {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.add_nullfont();
    e.set_interaction_mode(InteractionMode::Nonstop);
    let pre = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\catcode`\\$=3 \\catcode`\\^=7 \\catcode`\\_=8\n\
\\directlua{tex.enableprimitives(\"\",tex.extraprimitives())}\n\
\\def\\t#1{\\immediate\\write16{#1}}\\def\\s#1{\\t{[#1]}}\n";
    e.input.push_file("t.tex".to_string(), format!("{pre}{body}\n\\end\n").into_bytes());
    e.run();
    e.term
        .lines()
        .filter(|l| l.starts_with('['))
        .map(|l| l.trim_end_matches(')').to_string())
        .collect()
}

/// `\Unosubscript`/`\Unosuperscript` keep the style of the noad (the script
/// fonts differ from the text fonts here); a script after a style, glue or
/// kern node goes on a new empty noad, whose empty nucleus is an empty hbox.
#[test]
fn script_options_and_empty_nuclei_follow_luatex() {
    let body = r##"\font\x=cmr10 \font\xs=cmr7 \font\is=cmmi7 \font\i=cmmi10 \font\y=cmsy10 \font\z=cmex10
\textfont0=\x \scriptfont0=\xs \scriptscriptfont0=\xs
\textfont1=\i \scriptfont1=\is \scriptscriptfont1=\is
\textfont2=\y \scriptfont2=\y \scriptscriptfont2=\y
\textfont3=\z \scriptfont3=\z \scriptscriptfont3=\z
\thinmuskip=0mu \medmuskip=0mu \thickmuskip=0mu
\def\b#1{\setbox0\hbox{$#1$}\s{\the\wd0/\the\ht0/\the\dp0}}
\b{x\Unosubscript{a}} \b{x\Unosuperscript{a}} \b{x\Usubscript{a}}
\b{x\Unosuperscript{a}\Unosubscript{b}}
\b{\Unosuperscript x} \b{\displaystyle\Unosuperscript x} \b{\displaystyle^a} \b{x\mskip3mu^a}
\b{{}} \b{a{}b}
"##;
    assert_eq!(
        run(body),
        [
        "[11.00116pt/4.30554pt/1.49998pt]",
        "[11.00116pt/7.93446pt/0.0pt]",
        "[10.05292pt/4.30554pt/1.49998pt]",
        "[11.00116pt/7.93446pt/4.91544pt]",
        "[5.71527pt/7.93446pt/0.0pt]",
        "[5.71527pt/8.43446pt/0.0pt]",
        "[4.33765pt/7.1428pt/0.0pt]",
        "[11.71954pt/6.6428pt/0.0pt]",
        "[0.0pt/0.0pt/0.0pt]",
        "[9.57755pt/6.94444pt/0.0pt]"
        ]
    );
}

/// `\mathrulesmode` marks fraction, over/underline and radical rules with the
/// math rule subtypes and the size (`index`); `node.make_extensible` builds
/// the same boxes as the typesetter.
#[test]
fn math_rule_subtypes_and_make_extensible_follow_luatex() {
    let body = r##"\font\x=cmr10 \font\i=cmmi10 \font\y=cmsy10 \font\z=cmex10
\textfont0=\x \scriptfont0=\x \scriptscriptfont0=\x
\textfont1=\i \scriptfont1=\i \scriptscriptfont1=\i
\textfont2=\y \scriptfont2=\y \scriptscriptfont2=\y
\textfont3=\z \scriptfont3=\z \scriptscriptfont3=\z
\delcode`(="028300
\directlua{
function rules(n, out)
  for l in node.traverse(n) do
    if l.id == node.id("rule") then out.n = (out.n or 0) + 1 out[out.n] = l.subtype .. ":" .. l.index
    elseif l.id == 0 or l.id == 1 then rules(l.list, out) end
  end
  return out
end
function showrules() local o = rules(tex.box[0].list, {}) texio.write_nl("[" .. table.concat(o, ",") .. "]") end
}
\def\b#1{\setbox0\hbox{$#1$}\directlua{showrules()}}
\mathrulesmode=1
\b{{a\over b}} \b{\overline{x}\underline{y}} \b{\Uradical 3 "70 x} \b{\scriptstyle{a\over b}} \b{\scriptscriptstyle\overline{x}}
\b{{a\above 0pt b}} \b{{a\above 1pt b}} \b{\Uroot 3 "70 {a}{x}}
\mathrulesmode=0
\b{{a\over b}} \b{\overline{x}} \b{\Uradical 3 "70 x}
\setbox0\hbox{}
\def\ext#1#2#3{\directlua{tex.box[0]=node.make_extensible(\fontid\z,#1,tex.sp("#2"),#3)}\s{\the\wd0/\the\ht0/\the\dp0}}
\ext{0x30}{30pt}{65536}
\ext{0x3A}{40pt}{65536}
\ext{0x3A}{40pt}{tex.sp("1pt")}
\directlua{texio.write_nl("[" .. tostring(node.make_extensible(1,2)) .. "]")}
"##;
    assert_eq!(
        run(body),
        [
        "[7:0]",
        "[5:0,6:0]",
        "[8:0]",
        "[7:1]",
        "[5:2]",
        "[]",
        "[7:0]",
        "[8:0]",
        "[0:0]",
        "[0:0]",
        "[0:0]",
        "[8.75002pt/36.00034pt/0.0pt]",
        "[8.8889pt/42.00043pt/0.0pt]",
        "[8.8889pt/42.00043pt/0.0pt]",
        "[nil]"
        ]
    );
}

/// A MathConstants table without `SubscriptShiftDownWithSuperscript` leaves it
/// undefined (not zero), so the sub/sup shift falls back to `SubscriptShiftDown`.
#[test]
fn missing_math_constants_stay_undefined() {
    let body = r##"\directlua{
local f = font.read_tfm("cmr10", 655360)
f.name = "mc1" f.fullname = "mc1"
f.MathConstants = {AxisHeight = 200000, SubscriptShiftDown = 161873, SuperscriptShiftUp = 300000, FractionDelimiterSize = 661913, NoLimitSupFactor = 100}
_G.fid = font.define(f)
font.current(fid)
}
\textfont2=\font \scriptfont2=\font \scriptscriptfont2=\font
\textfont3=\font \scriptfont3=\font \scriptscriptfont3=\font
\s{\the\Umathsubsupshiftdown\textstyle} \s{\the\Umathsubshiftdown\textstyle}
\s{\the\Umathsubsupshiftdown\scriptstyle} \s{\the\Umathsupshiftup\crampedtextstyle}
\s{\the\Umathsubshiftdrop\textstyle}
"##;
    assert_eq!(
        run(body),
        [
        "[2.46999pt]",
        "[2.46999pt]",
        "[2.46999pt]",
        "[16383.99998pt]",
        "[16383.99998pt]"
        ]
    );
}
