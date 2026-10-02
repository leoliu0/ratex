//! LuaTeX-only primitives: math/delimiter codes, math char definitions and the
//! scan-time `\mathstyle`. Expected values are `luatex --ini` (TeX Live 2026)
//! output for the same input.

use tex_core::engine::{Engine, EngineKind, InteractionMode};

fn run(body: &str) -> Vec<String> {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.add_nullfont();
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

/// Dimensions of formulas typeset with the traditional cm fonts through the
/// `\\Umath` parameters `fixup_math_parameters` derives from them: scripts,
/// operators with limits, delimiters, rules, spacing classes, and the
/// `\\Umath` parameters / `\\mathscriptsmode` the document changes.
#[test]
fn traditional_font_math_dimensions_follow_luatex() {
    let body = r#"\def\s#1{\t{[#1]}}
\font\tenrm=cmr10 \font\sevenrm=cmr7 \font\fiverm=cmr5
\font\teni=cmmi10 \font\seveni=cmmi7 \font\fivei=cmmi5
\font\tensy=cmsy10 \font\sevensy=cmsy7 \font\fivesy=cmsy5
\font\tenex=cmex10
\textfont0=\tenrm \scriptfont0=\sevenrm \scriptscriptfont0=\fiverm
\textfont1=\teni \scriptfont1=\seveni \scriptscriptfont1=\fivei
\textfont2=\tensy \scriptfont2=\sevensy \scriptscriptfont2=\fivesy
\textfont3=\tenex \scriptfont3=\tenex \scriptscriptfont3=\tenex
\thinmuskip=3mu \medmuskip=4mu plus 2mu minus 4mu \thickmuskip=5mu plus 5mu
\delcode`(="028300 \delcode`)="029301 \delimiterfactor=901 \delimitershortfall=5pt
\mathcode`+="202B \mathcode`=="303D \mathcode`,="602C
\mathchardef\sum="1350 \mathchardef\int="1352 \mathchardef\leq="3114
\scriptspace=0.5pt \nulldelimiterspace=1.2pt
\def\b#1{\setbox0\hbox{$#1$}\s{\the\wd0/\the\ht0/\the\dp0}}
\b{a^b_c}
\b{x^2 y_i z^{a_b}}
\b{\displaystyle a^b_c \scriptstyle d^e_f}
\b{(a+b)^2 \leq c, d}
\b{\displaystyle\sum_{i=1}^n x}
\b{\sum_{i=1}^n x}
\b{\displaystyle\int_0^1 f}
\b{\displaystyle\int\nolimits_0^1 f}
\b{\mathop{xy}\limits^a_b}
\b{\left(a\right)}
\b{\left(\vrule height 30pt depth 20pt\right)}
\b{\overline{x}\underline{y}}
\b{a\mathbin{+}\mathrel{=}\mathpunct{,}\mathinner{b}\mathopen{(}\mathclose{)}}
\b{\scriptstyle a+b=c}
\Umathordbinspacing\textstyle=7mu plus 1mu \Umathbinordspacing\scriptstyle=2mu
\b{a+b} \b{\scriptstyle a+b}
\Umathsubshiftdown\textstyle=4pt \Umathsupshiftup\textstyle=5pt \Umathspaceafterscript\textstyle=1pt
\b{a^b_c} \mathscriptsmode=4 \b{a^b_c} \b{a_c} \b{a^b}
\Umathoverbarvgap\textstyle=3pt \Umathunderbarkern\textstyle=2pt \b{\overline{x}\underline{y}}
"#;
    let out = run(body);
    assert_eq!(
        out,
        [
            "[9.35963pt/8.49002pt/2.47217pt]",
            "[32.02812pt/8.14003pt/1.94444pt]",
            "[18.03296pt/8.99002pt/3.02655pt]",
            "[59.35745pt/8.14003pt/2.5pt]",
            "[21.82637pt/16.51393pt/12.79865pt]",
            "[31.39182pt/8.04175pt/3.00005pt]",
            "[17.6389pt/21.12231pt/15.789pt]",
            "[22.12503pt/15.65013pt/9.11122pt]",
            "[10.97687pt/10.31941pt/9.4722pt]",
            "[13.06369pt/7.5pt/2.5pt]",
            "[17.90002pt/30.0pt/24.50026pt]",
            "[10.97687pt/6.30544pt/3.94434pt]",
            "[41.7997pt/7.5pt/2.5pt]",
            "[23.70589pt/4.8611pt/0.83334pt]",
            "[23.46631pt/6.94444pt/0.83333pt]",
            "[14.90372pt/4.8611pt/0.83334pt]",
            "[9.85963pt/9.8611pt/2.47217pt]",
            "[9.85963pt/9.09718pt/3.23608pt]",
            "[9.85963pt/4.30554pt/3.23608pt]",
            "[9.80255pt/9.09718pt/0.0pt]",
            "[10.97687pt/8.1055pt/5.54436pt]"
        ]
    );
}

/// The same for an OpenType-style math font (a Lua font with
/// `MathConstants`, `mathkern`, `next` and vertical variants).
#[test]
fn opentype_font_math_dimensions_follow_luatex() {
    let body = r#"\def\s#1{\t{[#1]}}
\directlua{
local function load(name, size) return font.read_tfm(name, size) end
local names = {"MathConstants"}
local C = {"ScriptPercentScaleDown","ScriptScriptPercentScaleDown","DelimitedSubFormulaMinHeight","DisplayOperatorMinHeight","MathLeading","AxisHeight","AccentBaseHeight","FlattenedAccentBaseHeight","SubscriptShiftDown","SubscriptTopMax","SubscriptBaselineDropMin","SuperscriptShiftUp","SuperscriptShiftUpCramped","SuperscriptBottomMin","SuperscriptBaselineDropMax","SubSuperscriptGapMin","SuperscriptBottomMaxWithSubscript","SpaceAfterScript","UpperLimitGapMin","UpperLimitBaselineRiseMin","LowerLimitGapMin","LowerLimitBaselineDropMin","StackTopShiftUp","StackTopDisplayStyleShiftUp","StackBottomShiftDown","StackBottomDisplayStyleShiftDown","StackGapMin","StackDisplayStyleGapMin","StretchStackTopShiftUp","StretchStackBottomShiftDown","StretchStackGapAboveMin","StretchStackGapBelowMin","FractionNumeratorShiftUp","FractionNumeratorDisplayStyleShiftUp","FractionDenominatorShiftDown","FractionDenominatorDisplayStyleShiftDown","FractionNumeratorGapMin","FractionNumeratorDisplayStyleGapMin","FractionRuleThickness","FractionDenominatorGapMin","FractionDenominatorDisplayStyleGapMin","SkewedFractionHorizontalGap","SkewedFractionVerticalGap","OverbarVerticalGap","OverbarRuleThickness","OverbarExtraAscender","UnderbarVerticalGap","UnderbarRuleThickness","UnderbarExtraDescender","RadicalVerticalGap","RadicalDisplayStyleVerticalGap","RadicalRuleThickness","RadicalExtraAscender","RadicalKernBeforeDegree","RadicalKernAfterDegree","RadicalDegreeBottomRaisePercent","MinConnectorOverlap","SubscriptShiftDownWithSuperscript","FractionDelimiterSize","FractionDelimiterDisplayStyleSize","NoLimitSubFactor","NoLimitSupFactor"}
local function build(size, id)
  local rm = load("cmr10", size)
  local mi = load("cmmi10", size)
  local ex = load("cmex10", size)
  local sy = load("cmsy10", size)
  local t = mi
  t.name = "otfm" .. id
  t.fullname = t.name
  local ch = {}
  for c = 0, 127 do
    local src = rm.characters[c]
    if (c >= 65 and c <= 90) or (c >= 97 and c <= 122) or (c < 32) then src = mi.characters[c] or src end
    if src then ch[c] = src end
  end
  for c, v in pairs(ex.characters) do
    local n = {}
    for k, x in pairs(v) do n[k] = x end
    n.next = v.next and (0x1F000 + v.next) or nil
    if v.extensible then
      local e = v.extensible
      local vv = {}
      local function add(g, rep) if g and g ~= 0 then vv[#vv+1] = {glyph = 0x1F000 + g, extender = rep and 1 or 0, start = 0, ["end"] = 0, advance = (ex.characters[g].height + ex.characters[g].depth)} end end
      if e.bot then add(e.bot) end
      if e.rep then add(e.rep, true) end
      if e.mid and e.mid ~= 0 then add(e.mid) add(e.rep, true) end
      if e.top then add(e.top) end
      n.vert_variants = vv
      n.extensible = nil
    end
    ch[0x1F000 + c] = n
  end
  ch[0x28] = {width = ch[0x28].width, height = ch[0x28].height, depth = ch[0x28].depth, italic = 0, next = 0x1F000}
  ch[0x29] = {width = ch[0x29].width, height = ch[0x29].height, depth = ch[0x29].depth, italic = 0, next = 0x1F001}
  ch[0x61].top_accent = ch[0x61].width * 6 // 10
  for _, c in ipairs{0x61, 0x62, 0x78, 0x79, 0x31} do
    ch[c].italic = 20000 + c * 30
    ch[c].mathkern = {
      top_right = {{height = 100000, kern = 30000}, {height = 300000, kern = 50000}},
      bottom_right = {{height = 50000, kern = -20000}, {height = 250000, kern = 40000}},
      top_left = {{height = 100000, kern = 10000}},
      bottom_left = {{height = 150000, kern = 25000}, {height = 400000, kern = 35000}},
    }
  end
  t.characters = ch
  local mc = {}
  for i, n in ipairs(C) do mc[n] = 20000 + 997 * i end
  mc.AxisHeight = math.floor(size * 0.25)
  mc.RadicalDegreeBottomRaisePercent = 60
  mc.ScriptPercentScaleDown = 70
  mc.ScriptScriptPercentScaleDown = 50
  mc.DisplayOperatorMinHeight = math.floor(size * 1.4)
  mc.MinConnectorOverlap = 10000
  t.MathConstants = mc
  return t
end
_G.buildfont = function(size, i) return font.define(build(size, i)) end
}
\directlua{font.current(buildfont(655360, 1))}
\textfont0=\font \textfont1=\font \textfont2=\font \textfont3=\font
\directlua{font.current(buildfont(458752, 2))}
\scriptfont0=\font \scriptfont1=\font \scriptfont2=\font \scriptfont3=\font
\directlua{font.current(buildfont(327680, 3))}
\scriptscriptfont0=\font \scriptscriptfont1=\font \scriptscriptfont2=\font \scriptscriptfont3=\font
\thinmuskip=3mu \medmuskip=4mu plus 2mu minus 4mu \thickmuskip=5mu plus 5mu
\Udelcode`(="0 "28 \Udelcode`)="0 "29
\Umathcode`a="7 1 "61 \Umathcode`b="7 1 "62 \Umathcode`x="7 1 "78 \Umathcode`y="7 1 "79
\Umathcode`1="0 0 "31 \Umathcode`2="0 0 "32
\Umathcode`+="2 0 "2B \Umathcode`=="3 0 "3D \Umathcode`(="4 0 "28 \Umathcode`)="5 0 "29
\Umathchardef\sum="1 0 "1F050
\Umathchardef\int="1 0 "1F052
\def\b#1{\setbox0\hbox{$#1$}\s{\the\wd0/\the\ht0/\the\dp0}}
\b{a^b_x} \b{a^{1+2}_y} \b{\displaystyle a^b}
\b{\displaystyle\sum_x^y a} \b{\sum_x^y a} \b{\displaystyle\int_x^y a}
\b{\displaystyle\sum\nolimits_x^y a} \b{\mathop{ab}\limits^x_y}
\b{\left(a\right)} \b{\left(\vrule height 30pt\right)} \b{\left(\vrule height 90pt\right)}
\b{a+b=x} \b{(a)+(b)} \b{\overline{a}\underline{b}} \b{\vcenter{\hbox{a}}}
\b{a^{b^{x}}_{a_y}} \b{a\mathop{x}\nolimits^a} \b{a\mkern3mu b\mskip 4mu plus 2mu x}
\Umathsubshiftdown\textstyle=4pt \Umathspaceafterscript\textstyle=1pt \mathscriptsmode=3 \b{a^b_x}
"#;
    let out = run(body);
    assert_eq!(
        out,
        [
            "[10.86568pt/5.4249pt/2.99867pt]",
            "[20.28354pt/5.65823pt/4.35977pt]",
            "[10.86568pt/5.37926pt/0.0pt]",
            "[21.74657pt/15.46927pt/9.1386pt]",
            "[22.79749pt/9.98058pt/2.97258pt]",
            "[17.30211pt/18.58043pt/12.24977pt]",
            "[26.68637pt/12.98058pt/5.97258pt]",
            "[10.27716pt/10.56776pt/4.99963pt]",
            "[13.41327pt/7.5pt/2.5pt]",
            "[16.34929pt/33.11104pt/28.11104pt]",
            "[16.34929pt/91.1942pt/86.1942pt]",
            "[41.9079pt/6.94444pt/0.83333pt]",
            "[38.0549pt/7.5pt/2.5pt]",
            "[10.27716pt/7.27483pt/3.1062pt]",
            "[2.64294pt/3.57639pt/0.0pt]",
            "[15.95943pt/5.4249pt/5.66635pt]",
            "[17.64607pt/7.13329pt/0.0pt]",
            "[20.24133pt/6.94444pt/0.0pt]",
            "[11.40962pt/4.30554pt/1.18753pt]"
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
