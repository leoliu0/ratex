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

/// Like [`run`], but keeps the terminal lines `keep` accepts and returns the
/// messages of the errors as well.
fn run_keep(body: &str, keep: impl Fn(&str) -> bool) -> (Vec<String>, Vec<String>) {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.add_nullfont();
    e.set_interaction_mode(InteractionMode::Nonstop);
    let pre = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\catcode`\\$=3 \\catcode`\\^=7 \\catcode`\\_=8\n\
\\directlua{tex.enableprimitives(\"\",tex.extraprimitives())}\n\
\\def\\t#1{\\immediate\\write16{#1}}\\def\\s#1{\\t{[#1]}}\n";
    e.input.push_file("t.tex".to_string(), format!("{pre}{body}\n\\end\n").into_bytes());
    e.run();
    let lines = e
        .term
        .lines()
        .filter(|l| keep(l))
        .map(|l| l.trim_end_matches(')').to_string())
        .collect();
    let errors = e.diagnostics.iter().map(|d| d.message.trim_end_matches('.').to_string()).collect();
    (lines, errors)
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

/// The boxes the math, paragraph, alignment and display code build carry
/// luatex's list subtypes (`math_char_list`, `math_numerator_list`, `radical`,
/// `indent`, `line`, `cell`, `equation`, ...), and the math rules theirs.
#[test]
fn list_and_rule_subtypes_follow_luatex() {
    let body = r##"\font\x=cmr10 \font\xs=cmr7 \font\is=cmmi7 \font\i=cmmi10 \font\y=cmsy10 \font\z=cmex10
\textfont0=\x \scriptfont0=\xs \scriptscriptfont0=\xs
\textfont1=\i \scriptfont1=\is \scriptscriptfont1=\is
\textfont2=\y \scriptfont2=\y \scriptscriptfont2=\y
\textfont3=\z \scriptfont3=\z \scriptscriptfont3=\z
\hsize=100pt \parindent=5pt \delcode`(="028300 \delcode`)="029301 \hbadness=10000
\directlua{
function sub(n)
  local o, k = {}, 0
  for l in node.traverse(n) do
    if l.id == 0 or l.id == 1 then k = k + 1 o[k] = (l.id==0 and "h" or "v") .. l.subtype .. "(" .. sub(l.list) .. ")"
    elseif l.id == 2 then k = k + 1 o[k] = "r" .. l.subtype end
  end
  return table.concat(o, " ")
end
function dump(b) texio.write_nl("[" .. sub(b or tex.box[0]) .. "]") end
}
\def\b#1{\setbox0\hbox{$#1$}\directlua{dump()}}
\b{a\over b} \b{\Uradical 3 "70 {x}} \b{\overline{x}\underline{x}} \b{\left(x\right)}
\b{\Uroot 0 "70 {a}{x}} \b{\mathaccent"7016 x} \b{x_a^b}
\setbox0\vbox{\x\noindent a\par\indent b\par} \directlua{dump()}
\setbox0\vbox{\x\halign{#\cr a\cr}} \directlua{dump()}
\setbox0\vbox{\hsize=50pt \noindent $$a\eqno b$$} \directlua{dump()}
\setbox0\vbox{\hbox{a}\hbox{b}} \directlua{dump()}"##;
    let out = run(body);
    let expected = [
        "[h2(h19(h13() v19(h16() r0 h0()) h13()))]",
        "[h2(h0(h9() v28(r0 h20())))]",
        "[h2(v25(r0 h20()) v26(h20() r0))]",
        "[h2(h0(h9() h9()))]",
        "[h2(h0(h23() h9() v28(r0 h20())))]",
        "[h2(v27(h9() h20()))]",
        "[h2(v24(h21() h22()))]",
        "[v0(h1() h1(h3()))]",
        "[v0(h4(h5()))]",
        "[v0(h6(h6() h7()))]",
        "[v0(h2() h2())]",
    ];
    assert_eq!(out, expected);
}

/// `make_extensible` callback: it gets the delimiter's attribute list (nil
/// without attributes), a box made by `node.new` keeps luatex's unset
/// direction (`-RTT` in `\showbox`), and `node.make_extensible` puts its
/// attribute list argument on everything it builds.
#[test]
fn make_extensible_callback_attributes_and_direction_follow_luatex() {
    let body = r##"\font\x=cmr10 \font\i=cmmi10 \font\y=cmsy10 \font\z=cmex10
\textfont0=\x \scriptfont0=\x \scriptscriptfont0=\x
\textfont1=\i \scriptfont1=\i \scriptscriptfont1=\i
\textfont2=\y \scriptfont2=\y \scriptscriptfont2=\y
\textfont3=\z \scriptfont3=\z \scriptscriptfont3=\z
\delcode`(="028300 \delcode`)="029301
\showboxbreadth=100 \showboxdepth=100 \tracingonline1
\directlua{
callback.register("make_extensible", function(f,c,v,o,h,a)
  local k = a and a.next
  texio.write_nl("[CB " .. c .. " " .. v .. " " .. tostring(h) .. " " .. type(a) .. " " .. tostring(k and k.number) .. "=" .. tostring(k and k.value) .. "]")
  return nil
end)
}
\attribute7=5 \setbox0\hbox{$\left(\vrule height 30pt depth 10pt\right)$}
\attribute7=-"7FFFFFFF \setbox0\hbox{$\left(\vrule height 30pt depth 10pt\right)$}
\attribute3=2 \setbox0\hbox{$\Uradical 3 "70 {\vrule height 30pt depth 30pt}$}
\attribute3=-"7FFFFFFF
\directlua{callback.register("make_extensible", function() return node.new("hlist") end)}
\setbox0\hbox{$\left(\vrule height 30pt depth 10pt\right)$} \showbox0
\directlua{
callback.register("make_extensible", function()
  local n = node.new("vlist") n.dir = "TRT" n.height = 1000 n.width = 2000
  return n
end)
}
\setbox0\hbox{$\left(\vrule height 30pt depth 10pt\right)$} \showbox0
\directlua{
local al = node.new("hlist")
node.set_attribute(al, 7, 5)
local b = node.make_extensible(\number\fontid\z, 0x30, tex.sp("100pt"), 0, false, al.attr)
local cs = {}
for n in node.traverse(b.list) do table.insert(cs, node.type(n.id) .. ":" .. tostring(n.attr and n.attr.next and n.attr.next.number)) end
texio.write_nl("[ME " .. node.type(b.id) .. " " .. tostring(b.attr and b.attr.next and b.attr.next.number) .. " " .. table.concat(cs, ",") .. "]")
texio.write_nl("[ME2 " .. tostring(node.make_extensible(\number\fontid\z, 0x30, tex.sp("100pt")).attr) .. "]")
}"##;
    let (out, errors) = run_keep(body, |l| l.starts_with('[') || l.contains("direction -RTT") || l.contains("direction TRT"));
    assert_eq!(
        out,
        [
        "[CB 48 3604480 false userdata 7=5]",
        "[CB 49 3604480 false userdata 7=5]",
        "[CB 48 3604480 false nil nil=nil]",
        "[CB 49 3604480 false nil nil=nil]",
        "[CB 116 3991139 false userdata 3=2]",
        "..\\hbox(0.0+0.0)x0.0, shifted -2.5, direction -RTT",
        "..\\hbox(0.0+0.0)x0.0, shifted -2.5, direction -RTT",
        "..\\vbox(0.01526+0.0)x0.03052, shifted -2.49237, direction TRT",
        "..\\vbox(0.01526+0.0)x0.03052, shifted -2.49237, direction TRT",
        "[ME vlist 7 hlist:7,hlist:7,hlist:7,hlist:7,hlist:7,hlist:7,hlist:7,hlist:7,hlist:7,hlist:7,hlist:7,hlist:7,hlist:7]",
        "[ME2 nil]"
        ]
    );
    // (\showbox is reported among the diagnostics)
    let _ = errors;
}

/// A callback result that is no box is a fatal error, as in luatex.
#[test]
fn make_extensible_callback_result_must_be_a_box() {
    let body = r##"\font\x=cmr10 \font\i=cmmi10 \font\y=cmsy10 \font\z=cmex10
\textfont0=\x \scriptfont0=\x \scriptscriptfont0=\x
\textfont1=\i \scriptfont1=\i \scriptscriptfont1=\i
\textfont2=\y \scriptfont2=\y \scriptscriptfont2=\y
\textfont3=\z \scriptfont3=\z \scriptscriptfont3=\z
\delcode`(="028300 \delcode`)="029301
\directlua{callback.register("make_extensible", function() return node.new("kern") end)}
\s{before}
\setbox0\hbox{$\left(\vrule height 30pt depth 10pt\right)$}
\s{after}"##;
    let (out, errors) = run_keep(body, |l| l.starts_with('['));
    assert_eq!(out, ["[before]"]);
    assert_eq!(
        errors,
        [
        "error:  (fonts): invalid extensible character 48 created for font 4, [h|v]list expected"
        ]
    );
}

/// What follows the alignment of a display is checked at once
/// (`finish_display_alignment`): assignments, then `$$` or
/// `\Ustopdisplaymath`; `\suppressmathparerror` skips a `\par` there.
#[test]
fn display_alignment_end_follows_luatex() {
    let body = r##"\font\x=cmr10 \font\i=cmmi10 \font\y=cmsy10 \font\z=cmex10
\textfont0=\x \scriptfont0=\x \scriptscriptfont0=\x
\textfont1=\i \scriptfont1=\i \scriptscriptfont1=\i
\textfont2=\y \scriptfont2=\y \scriptscriptfont2=\y
\textfont3=\z \scriptfont3=\z \scriptscriptfont3=\z
\baselineskip=0pt \lineskip=0pt \lineskiplimit=0pt \hsize=100pt \parindent=0pt \tolerance=10000 \hbadness=10000 \vbadness=10000 \catcode`\&=4
\directlua{
function dump(b)
  local o = {}
  for n in node.traverse(b.list) do
    local t = node.type(n.id)
    if n.id == 0 or n.id == 1 then t = t .. "(" .. n.width .. "," .. n.height .. ")"
    elseif n.id == node.id("penalty") then t = t .. "(" .. n.penalty .. ")"
    elseif n.id == node.id("glue") then t = t .. "(" .. n.subtype .. ")" end
    table.insert(o, t)
  end
  texio.write_nl("[" .. table.concat(o, " ") .. "]")
end
}
\long\def\C#1{\setbox1\vbox{\x\noindent a #1\par}\directlua{dump(tex.box[1])}}
\suppressmathparerror=0
\C{$$\halign{#\cr b\cr}$$ d}
\C{$$\halign{#\cr b\cr} c $$ d}
\C{$$\halign{#\cr b\cr}\par$$ d}
\C{$$\halign{#\cr b\cr}\par\par$$ d}
\C{$$\halign{#\cr b\cr}\count1=5 \relax $$ d}
\C{$$\halign{#\cr b\cr}$ c}
\C{\Ustartdisplaymath\halign{#\cr b\cr}\Ustopdisplaymath d}
\C{\Ustartdisplaymath\halign{#\cr b\cr}$$ d}
\C{$$\halign{#\cr b\cr}\Ustopmath d}
\suppressmathparerror=1
\C{$$\halign{#\cr b\cr}\par$$ d}
\C{$$\halign{#\cr b\cr}\par\par$$ d}
\C{$$\halign{#\cr b\cr} c $$ d}"##;
    let (out, errors) = run_keep(body, |l| l.starts_with('['));
    assert_eq!(
        out,
        [
        "[hlist(6553600,282168) penalty(0) glue(4) glue(1) hlist(364090,455111) penalty(0) glue(5) glue(1) hlist(6553600,455111)]",
        "[hlist(6553600,282168) penalty(0) glue(4) glue(1) hlist(364090,455111) penalty(0) glue(5) penalty(0) glue(6) glue(1) hlist(341106,455111) penalty(0) glue(7)]",
        "[hlist(6553600,282168) penalty(0) glue(4) glue(1) hlist(364090,455111) penalty(0) glue(5) penalty(0) glue(6) glue(1) hlist(341106,455111) penalty(0) glue(7)]",
        "[hlist(6553600,282168) penalty(0) glue(4) glue(1) hlist(364090,455111) penalty(0) glue(5) glue(3) glue(2) hlist(6553600,0) penalty(0) glue(6) glue(1) hlist(341106,455111) penalty(0) glue(7)]",
        "[hlist(6553600,282168) penalty(0) glue(4) glue(1) hlist(364090,455111) penalty(0) glue(5) glue(1) hlist(6553600,455111)]",
        "[hlist(6553600,282168) penalty(0) glue(4) glue(1) hlist(364090,455111) penalty(0) glue(5) glue(1) hlist(6553600,282168)]",
        "[hlist(6553600,282168) penalty(0) glue(4) glue(1) hlist(364090,455111) penalty(0) glue(5) glue(1) hlist(6553600,455111)]",
        "[hlist(6553600,282168) penalty(0) glue(4) glue(1) hlist(364090,455111) penalty(0) glue(5) glue(1) hlist(6553600,455111)]",
        "[hlist(6553600,282168) penalty(0) glue(4) glue(1) hlist(364090,455111) penalty(0) glue(5) glue(1) hlist(6553600,455111)]",
        "[hlist(6553600,282168) penalty(0) glue(4) glue(1) hlist(364090,455111) penalty(0) glue(5) glue(1) hlist(6553600,455111)]",
        "[hlist(6553600,282168) penalty(0) glue(4) glue(1) hlist(364090,455111) penalty(0) glue(5) glue(1) hlist(6553600,455111)]",
        "[hlist(6553600,282168) penalty(0) glue(4) glue(1) hlist(364090,455111) penalty(0) glue(5) glue(1) hlist(6553600,455111)]"
        ]
    );
    let errors: Vec<_> = errors.iter().filter(|m| m.starts_with("Display math should end with")).collect();
    assert_eq!(
        errors,
        [
        "Display math should end with \\Ustopdisplaymath",
        "Display math should end with $$",
        "Display math should end with \\Ustopdisplaymath",
        "Display math should end with $$",
        "Display math should end with \\Ustopdisplaymath",
        "Display math should end with $$",
        "Display math should end with $$",
        "Display math should end with \\Ustopdisplaymath",
        "Display math should end with \\Ustopdisplaymath",
        "Display math should end with $$"
        ]
    );
}

/// With `\mathrulethicknessmode` the bar of an over/underline noad takes its
/// thickness from the math font of the `fam` a Lua callback sets.
#[test]
fn over_and_under_bar_thickness_uses_noad_fam() {
    let body = r##"\font\x=cmr10 \font\y=cmsy10 \font\z=cmex10
\directlua{
local f = font.read_tfm("cmr10", 655360)
f.name = "mc1" f.fullname = "mc1"
f.MathConstants = {AxisHeight = 200000, OverbarRuleThickness = 30000, UnderbarRuleThickness = 50000}
_G.newfid = font.define(f)
}
\textfont0=\x \scriptfont0=\x \scriptscriptfont0=\x
\textfont1=\x \scriptfont1=\x \scriptscriptfont1=\x
\textfont2=\y \scriptfont2=\y \scriptscriptfont2=\y
\textfont3=\z \scriptfont3=\z \scriptscriptfont3=\z
\directlua{font.current(newfid)}
\textfont1=\font \scriptfont1=\font \scriptscriptfont1=\font
\Umathoverbarrule\textstyle=3pt \Umathunderbarrule\textstyle=4pt
\Umathoverbarrule\displaystyle=3pt \Umathunderbarrule\displaystyle=4pt
\Umathoverbarvgap\textstyle=1pt \Umathoverbarkern\textstyle=2pt \Umathunderbarvgap\textstyle=1pt \Umathunderbarkern\textstyle=2pt
\Umathoverbarvgap\displaystyle=1pt \Umathoverbarkern\displaystyle=2pt \Umathunderbarvgap\displaystyle=1pt \Umathunderbarkern\displaystyle=2pt
\directlua{
FAM = nil
function bars(n, out)
  for l in node.traverse(n) do
    if l.id == node.id("rule") then table.insert(out, l.height) end
    if l.id == 0 or l.id == 1 then bars(l.list, out) end
  end
  return out
end
callback.register("mlist_to_hlist", function(h, s, p)
  for n in node.traverse(h) do
    if n.id == node.id("noad") and FAM then node.direct.setfam(node.direct.todirect(n), FAM) end
  end
  return node.mlist_to_hlist(h, s, p)
end)
}
\def\F#1{\directlua{FAM=#1}}
\def\b#1{\setbox0\hbox{$#1$}\directlua{texio.write_nl("[" .. table.concat(bars(tex.box[0].list, {}), ",") .. "]")}}
\def\bd#1{\setbox0\hbox{$\displaystyle#1$}\directlua{texio.write_nl("[" .. table.concat(bars(tex.box[0].list, {}), ",") .. "]")}}
\b{\overline{x}\underline{y}}
\mathrulethicknessmode=1
\b{\overline{x}\underline{y}}
\F1
\b{\overline{x}\underline{y}}
\bd{\overline{x}\underline{y}}
\mathrulethicknessmode=0
\b{\overline{x}\underline{y}}
\mathrulethicknessmode=1
\F0
\b{\overline{x}\underline{y}}
\F7
\b{\overline{x}\underline{y}}
\F{-1}
\b{\overline{x}\underline{y}}"##;
    let (out, errors) = run_keep(body, |l| l.starts_with('['));
    assert_eq!(
        out,
        [
        "[196608,262144]",
        "[196608,262144]",
        "[30000,50000]",
        "[30000,50000]",
        "[196608,262144]",
        "[196608,262144]",
        "[196608,262144]",
        "[196608,262144]"
        ]
    );
    assert!(errors.is_empty(), "{errors:?}");
}

/// A Lua fraction noad with delimiter nodes, null ones included, is a
/// delimited fraction (unshifted empty boxes); one without is axis-shifted.
/// A `\withdelims` fraction shows its delimiter nodes to Lua.
#[test]
fn fraction_delimiter_nodes_follow_luatex() {
    let body = r##"\font\x=cmr10 \font\i=cmmi10 \font\y=cmsy10 \font\z=cmex10
\textfont0=\x \scriptfont0=\x \scriptscriptfont0=\x
\textfont1=\i \scriptfont1=\i \scriptscriptfont1=\i
\textfont2=\y \scriptfont2=\y \scriptscriptfont2=\y
\textfont3=\z \scriptfont3=\z \scriptscriptfont3=\z
\delcode`(="028300 \delcode`)="029301 \delcode`/="02F30E \delcode`[="02002C3 \delcode`]="02003C5
\directlua{
MODE = 0
function shape(l, d)
  local o = {}
  for n in node.traverse(l) do
    if n.id == 0 or n.id == 1 then
      local t = (n.id == 0 and "h" or "v") .. "(" .. n.width .. "," .. n.height .. "," .. n.depth .. "," .. n.shift .. ")"
      if d > 0 then t = t .. "{" .. shape(n.list, d - 1) .. "}" end
      table.insert(o, t)
    end
  end
  return table.concat(o, " ")
end
callback.register("mlist_to_hlist", function(h, s, p)
  for n in node.traverse(h) do
    if n.id == node.id("fraction") then
      if MODE == 1 or MODE == 3 then n.left = node.new("delim") end
      if MODE == 2 or MODE == 3 then n.right = node.new("delim") end
      if MODE == 9 then
        texio.write_nl("[FR " .. tostring(n.left and n.left.id) .. " " .. tostring(n.right and n.right.id) .. " " .. tostring(n.left and n.left.small_fam) .. " " .. tostring(n.left and n.left.large_char) .. " " .. tostring(n.middle and n.middle.id) .. "]")
      end
    end
  end
  return node.mlist_to_hlist(h, s, p)
end)
}
\def\M#1{\directlua{MODE=#1}}
\def\b#1{\setbox0\hbox{$#1$}\directlua{texio.write_nl("[" .. shape(tex.box[0].list, 2) .. "]")}}
\def\bd#1{\setbox0\hbox{$\displaystyle#1$}\directlua{texio.write_nl("[" .. shape(tex.box[0].list, 2) .. "]")}}
\M0 \b{a\over b}
\M1 \b{a\over b}
\M2 \b{a\over b}
\M3 \b{a\over b}
\M3 \bd{a\over b}
\M3 \b{a\atop b}
\M3 \b{a\above 2pt b}
\M3 \b{a\overwithdelims() b}
\M1 \b{a\overwithdelims() b}
\M3 \b{a\Uskewed/ b}
\M9 \b{a\over b}
\M9 \b{a\overwithdelims() b}
\M9 \b{a\atopwithdelims[] b}
\M9 \b{a\Uskewedwithdelims()/ b}"##;
    let (out, errors) = run_keep(body, |l| l.starts_with('['));
    assert_eq!(
        out,
        [
        "[h(346416,540204,330591,0){h(0,0,0,-163840){} v(346416,540204,330591,0){h(346416,282168,0,0) h(346416,455111,0,0)} h(0,0,0,-163840){}}]",
        "[h(346416,540204,330591,0){h(0,0,0,0){} v(346416,540204,330591,0){h(346416,282168,0,0) h(346416,455111,0,0)} h(0,0,0,-163840){}}]",
        "[h(346416,540204,330591,0){h(0,0,0,-163840){} v(346416,540204,330591,0){h(346416,282168,0,0) h(346416,455111,0,0)} h(0,0,0,0){}}]",
        "[h(346416,540204,330591,0){h(0,0,0,0){} v(346416,540204,330591,0){h(346416,282168,0,0) h(346416,455111,0,0)} h(0,0,0,0){}}]",
        "[h(346416,540204,330591,0){h(0,0,0,0){} v(346416,540204,330591,0){h(346416,282168,0,0) h(346416,455111,0,0)} h(0,0,0,0){}}]",
        "[h(346416,581447,234471,0){h(0,0,0,0){} v(346416,581447,234471,0){h(346416,282168,0,0) h(346416,455111,0,0)} h(0,0,0,0){}}]",
        "[h(346416,642616,487879,0){h(0,0,0,0){} v(346416,642616,487879,0){h(346416,282168,0,0) h(346416,455111,0,0)} h(0,0,0,0){}}]",
        "[h(346416,540204,330591,0){h(0,0,0,0){} v(346416,540204,330591,0){h(346416,282168,0,0) h(346416,455111,0,0)} h(0,0,0,0){}}]",
        "[h(646791,557059,330591,0){h(0,0,0,0){} v(346416,540204,330591,0){h(346416,282168,0,0) h(346416,455111,0,0)} h(300375,26213,760226,-530846){}}]",
        "[h(627674,491520,163840,0){h(0,0,0,0){} h(627674,491520,163840,0){h(346416,364088,0,-81920) h(0,491520,163840,0) h(281258,455111,81920,81920)} h(0,0,0,0){}}]",
        "[FR nil nil nil nil nil]",
        "[h(346416,540204,330591,0){h(0,0,0,-163840){} v(346416,540204,330591,0){h(346416,282168,0,0) h(346416,455111,0,0)} h(0,0,0,-163840){}}]",
        "[FR 27 27 0 0 nil]",
        "[h(947166,557059,330591,0){h(300375,26213,760226,-530846){} v(346416,540204,330591,0){h(346416,282168,0,0) h(346416,455111,0,0)} h(300375,26213,760226,-530846){}}]",
        "[FR 27 27 2 195 nil]",
        "[h(1365868,581447,234471,0){h(509726,382293,54613,0){} v(346416,581447,234471,0){h(346416,282168,0,0) h(346416,455111,0,0)} h(509726,382293,54613,0){}}]",
        "[FR 27 27 0 1 27]",
        "[h(1306702,557059,229380,0){h(300375,26213,760226,-530846){} h(627674,491520,163840,0){h(346416,364088,0,-81920) h(0,491520,163840,0) h(281258,455111,81920,81920)} h(378653,26213,760226,-530846){}}]"
        ]
    );
    assert!(errors.is_empty(), "{errors:?}");
}
