//! Font expansion and character protrusion of Lua fonts (`\adjustspacing`,
//! `\protrudechars`, `expansion_factor` / `left_protruding` /
//! `right_protruding`, `\efcode` / `\lpcode` / `\rpcode`). Expected values come
//! from `luatex --ini` (TeX Live 2026) running the same input; every case
//! sets a paragraph in a font made by `font.define` from `font.read_tfm("cmr10")`
//! and compares the line boxes: width, glue set and sign, the margin kerns
//! (side, width, character and its expansion) and the `expansion_factor` of
//! the glyphs (run lengths) and of the font kerns.

use tex_core::engine::{Engine, EngineKind};

fn run_luatex(body: &str) -> Engine {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.add_nullfont();
    for (c, cat) in [(b'{', 1), (b'}', 2), (b'#', 6), (b' ', 10), (b'\n', 5)] {
        e.eqtb.cat[c as usize] = cat;
    }
    e.input.push_file(
        "t.tex".to_string(),
        format!("\\directlua{{tex.enableprimitives('',tex.extraprimitives())}}\n{body}\n\\end\n").into_bytes(),
    );
    e.run();
    e
}

fn assert_clean(e: &Engine) {
    assert_eq!(e.error_count, 0, "errors: {:?}\n{}", e.diagnostics, e.term);
}

/// The margin kerns with `\protrudechars`:
/// protruding characters shorten the lines (2) or are only kerned (1).
#[test]
fn protrusion_margin_kerns_and_line_widths() {
    let e = run_luatex(r####"\begingroup\catcode`\%=12 \catcode`\#=12 \directlua{
local nl = string.char(10)
local function r4(x) return tostring(math.floor(x * 10000 + 0.5) / 10000) end
local function summary(box)
 local out = {}
 for l in node.traverse_id(node.id("hlist"), box.list) do
  local parts = {"w=" .. l.width .. " gs=" .. r4(l.glue_set) .. " sign=" .. l.glue_sign}
  local last, run = nil, 0
  local function flush() if run > 0 then parts[#parts+1] = last .. "x" .. run end last, run = nil, 0 end
  for n in node.traverse(l.list) do
   if n.id == node.id("glyph") then
    if n.expansion_factor == last then run = run + 1 else flush(); last, run = n.expansion_factor, 1 end
   elseif n.id == node.id("margin_kern") then
    flush(); parts[#parts+1] = "mk" .. n.subtype .. ":" .. n.width .. ":" .. n.glyph.char .. ":" .. n.glyph.expansion_factor
   elseif n.id == node.id("kern") and n.expansion_factor ~= 0 then
    flush(); parts[#parts+1] = "k" .. n.kern .. ":" .. n.expansion_factor
   end
  end
  flush()
  out[#out+1] = table.concat(parts, " ")
 end
 return table.concat(out, " | ")
end
function check(box, expect)
 local got = summary(box)
 local f = os.getenv("SUMOUT")
 if f then local fh = io.open(f, "a"); fh:write(got, nl, "--", nl); fh:close()
 elseif got ~= expect then error("MISMATCH:" .. nl .. got .. nl .. "---expected---" .. nl .. expect) end
end
 local t = font.read_tfm("cmr10", 655360)
 for c, ch in pairs(t.characters) do
  if c >= 97 and c <= 122 then ch.left_protruding = 100 ch.right_protruding = 200 end
  if c == 46 or c == 44 then ch.right_protruding = 700 end
  if c == 84 then ch.left_protruding = 300 end
 end
 tex.definefont(true, "xf", font.define(t))
}\endgroup
\xf \parindent=0pt \parfillskip=0pt plus 1fil \tolerance=10000 \pretolerance=-1 \baselineskip=12pt \hbadness=10000 \hfuzz=1000pt \righthyphenmin=62
\hsize=110pt
\protrudechars=2
\setbox1\vbox{The quick brown fox jumps over the lazy dog, and then runs away again, barking loudly. Tall trees, taller towers.\par}
\directlua{check(tex.box[1], [==[w=7208960 gs=42.8336 sign=1 mk0:-196608:84:0 0x8 mk1:-131072:107:0 | w=7208960 gs=0.1833 sign=1 mk0:-65536:98:0 0x20 mk1:-131072:101:0 | w=7208960 gs=1.3958 sign=1 mk0:-65536:108:0 0x19 mk1:-131072:115:0 | w=7208960 gs=0.5251 sign=2 mk0:-65536:97:0 0x24 mk1:-458753:46:0 | w=7208960 gs=16.1944 sign=1 mk0:-196608:84:0 0x23 mk1:-458753:46:0]==])}
\protrudechars=1
\setbox1\vbox{The quick brown fox jumps over the lazy dog, and then runs away again, barking loudly. Tall trees, taller towers.\par}
\directlua{check(tex.box[1], [==[w=7208960 gs=0.0 sign=0 mk0:-196608:84:0 0x3 mk1:-131072:101:0 | w=7208960 gs=2.6944 sign=1 mk0:-65536:113:0 0x18 mk1:-131072:115:0 | w=7208960 gs=2.3625 sign=1 mk0:-65536:111:0 0x18 mk1:-131072:100:0 | w=7208960 gs=4.3611 sign=1 mk0:-65536:116:0 0x18 mk1:-458753:44:0 | w=7208960 gs=1.1333 sign=1 mk0:-65536:98:0 0x24 mk1:-458753:44:0 | w=7208960 gs=61.2222 sign=1 mk0:-65536:116:0 0x13 mk1:-458753:46:0]==])}
"####);
    assert_clean(&e);
}

/// `\adjustspacing` 2, 1 and 3 with stretch, shrink and step in the font
/// table: glyph and font-kern expansion of every line, the expansion kept by
/// the glyph of a margin kern, and the lines `\adjustspacing` 2 chooses.
#[test]
fn expansion_factors_of_glyphs_and_kerns() {
    let e = run_luatex(r####"\begingroup\catcode`\%=12 \catcode`\#=12 \directlua{
local nl = string.char(10)
local function r4(x) return tostring(math.floor(x * 10000 + 0.5) / 10000) end
local function summary(box)
 local out = {}
 for l in node.traverse_id(node.id("hlist"), box.list) do
  local parts = {"w=" .. l.width .. " gs=" .. r4(l.glue_set) .. " sign=" .. l.glue_sign}
  local last, run = nil, 0
  local function flush() if run > 0 then parts[#parts+1] = last .. "x" .. run end last, run = nil, 0 end
  for n in node.traverse(l.list) do
   if n.id == node.id("glyph") then
    if n.expansion_factor == last then run = run + 1 else flush(); last, run = n.expansion_factor, 1 end
   elseif n.id == node.id("margin_kern") then
    flush(); parts[#parts+1] = "mk" .. n.subtype .. ":" .. n.width .. ":" .. n.glyph.char .. ":" .. n.glyph.expansion_factor
   elseif n.id == node.id("kern") and n.expansion_factor ~= 0 then
    flush(); parts[#parts+1] = "k" .. n.kern .. ":" .. n.expansion_factor
   end
  end
  flush()
  out[#out+1] = table.concat(parts, " ")
 end
 return table.concat(out, " | ")
end
function check(box, expect)
 local got = summary(box)
 local f = os.getenv("SUMOUT")
 if f then local fh = io.open(f, "a"); fh:write(got, nl, "--", nl); fh:close()
 elseif got ~= expect then error("MISMATCH:" .. nl .. got .. nl .. "---expected---" .. nl .. expect) end
end
 local t = font.read_tfm("cmr10", 655360)
 t.stretch = 20 t.shrink = 20 t.step = 5
 for c, ch in pairs(t.characters) do
  if c == 102 then ch.expansion_factor = 400 end
  if c == 109 then ch.expansion_factor = 0 end
  if c >= 97 and c <= 122 then ch.left_protruding = 100 ch.right_protruding = 200 end
 end
 tex.definefont(true, "xf", font.define(t))
}\endgroup
\xf \parindent=0pt \parfillskip=0pt plus 1fil \tolerance=10000 \pretolerance=-1 \baselineskip=12pt \hbadness=10000 \hfuzz=1000pt \righthyphenmin=62
\hsize=150pt \protrudechars=2
\adjustspacing=2
\setbox1\vbox{AVA Tower Wave Away To Yo AVAVAV Wa Ty Vo Yes Tavo Way Aware Awry Watch Typewriter Wow, Yo-yo. TAVA Voyage Waves.\par}
\directlua{check(tex.box[1], [==[w=9830400 gs=1.821 sign=1 20000x1 k-72819:-1456 20000x1 k-72819:-1456 20000x2 k-54614:-1092 20000x1 k-18205:-364 20000x1 k-18205:-364 20000x3 k-54614:-1092 20000x1 k-18205:-364 20000x1 k-18205:-364 20000x3 k-18205:-364 20000x1 k-18205:-364 20000x2 k-54614:-1092 20000x2 k-54614:-1092 20000x1 mk1:-131072:111:20000 | w=9830400 gs=0.027 sign=1 -10000x1 k-72819:-1456 -10000x1 k-72819:-1456 -10000x1 k-72819:-1456 -10000x1 k-72819:-1456 -10000x1 k-72819:-1456 -10000x2 k-54614:-1092 -10000x2 k-18205:-364 -10000x2 k-54614:-1092 -10000x2 k-54614:-1092 -10000x3 k-54614:-1092 -10000x1 k-18205:-364 -10000x1 k-18205:-364 -10000x2 k-54614:-1092 -10000x1 k-18205:-364 -10000x1 mk1:-131072:121:-10000 | w=9830400 gs=2.1973 sign=1 20000x2 k-18205:-364 20000x8 k-54614:-1092 20000x3 k-18205:-364 20000x2 k-18205:-364 20000x2 k18205:364 20000x7 mk1:-131072:114:20000 | w=9830400 gs=0.0065 sign=1 -20000x1 k-54614:-1092 -20000x1 k-18205:-364 -20000x3 k-54614:-1092 -20000x2 k-18205:-364 -20000x3 k-54614:-1092 -20000x1 k-72819:-1456 -20000x1 k-72819:-1456 -20000x2 k-54614:-1092 -20000x1 k-18205:-364 -20000x1 k-18205:-364 -20000x4 k-54614:-1092 -20000x1 k-18205:-364 -20000x1 k-18205:-364 -20000x3]==])}
\adjustspacing=1
\setbox1\vbox{AVA Tower Wave Away To Yo AVAVAV Wa Ty Vo Yes Tavo Way Aware Awry Watch Typewriter Wow, Yo-yo. TAVA Voyage Waves.\par}
\directlua{check(tex.box[1], [==[w=9830400 gs=1.821 sign=1 20000x1 k-72819:-1456 20000x1 k-72819:-1456 20000x2 k-54614:-1092 20000x1 k-18205:-364 20000x1 k-18205:-364 20000x3 k-54614:-1092 20000x1 k-18205:-364 20000x1 k-18205:-364 20000x3 k-18205:-364 20000x1 k-18205:-364 20000x2 k-54614:-1092 20000x2 k-54614:-1092 20000x1 mk1:-131072:111:20000 | w=9830400 gs=0.027 sign=1 -10000x1 k-72819:-1456 -10000x1 k-72819:-1456 -10000x1 k-72819:-1456 -10000x1 k-72819:-1456 -10000x1 k-72819:-1456 -10000x2 k-54614:-1092 -10000x2 k-18205:-364 -10000x2 k-54614:-1092 -10000x2 k-54614:-1092 -10000x3 k-54614:-1092 -10000x1 k-18205:-364 -10000x1 k-18205:-364 -10000x2 k-54614:-1092 -10000x1 k-18205:-364 -10000x1 mk1:-131072:121:-10000 | w=9830400 gs=2.1973 sign=1 20000x2 k-18205:-364 20000x8 k-54614:-1092 20000x3 k-18205:-364 20000x2 k-18205:-364 20000x2 k18205:364 20000x7 mk1:-131072:114:20000 | w=9830400 gs=0.0065 sign=1 -20000x1 k-54614:-1092 -20000x1 k-18205:-364 -20000x3 k-54614:-1092 -20000x2 k-18205:-364 -20000x3 k-54614:-1092 -20000x1 k-72819:-1456 -20000x1 k-72819:-1456 -20000x2 k-54614:-1092 -20000x1 k-18205:-364 -20000x1 k-18205:-364 -20000x4 k-54614:-1092 -20000x1 k-18205:-364 -20000x1 k-18205:-364 -20000x3]==])}
\adjustspacing=3 \hsize=120pt
\setbox1\vbox{AVA Tower Wave Away To Yo AVAVAV Wa Ty Vo Yes Tavo Way Aware Awry Watch Typewriter Wow, Yo-yo. TAVA Voyage Waves.\par}
\directlua{check(tex.box[1], [==[w=7864320 gs=0.0416 sign=1 20000x18 mk1:-131072:111:20000 | w=7864320 gs=2.5907 sign=1 20000x14 mk1:-131072:111:20000 | w=7864320 gs=0.0336 sign=1 10000x19 mk1:-131072:121:10000 | w=7864320 gs=3.1146 sign=1 20000x19 | w=7864320 gs=0.9477 sign=2 -20000x21]==])}
"####);
    assert_clean(&e);
}

/// `\expandglyphsinfont` and `\efcode` / `\lpcode` / `\rpcode` set the
/// limits and the codes of a Lua font: the result is the one of a font table
/// carrying the same numbers.
#[test]
fn font_expansion_set_with_primitives() {
    let e = run_luatex(r####"\begingroup\catcode`\%=12 \catcode`\#=12 \directlua{
local nl = string.char(10)
local function r4(x) return tostring(math.floor(x * 10000 + 0.5) / 10000) end
local function summary(box)
 local out = {}
 for l in node.traverse_id(node.id("hlist"), box.list) do
  local parts = {"w=" .. l.width .. " gs=" .. r4(l.glue_set) .. " sign=" .. l.glue_sign}
  local last, run = nil, 0
  local function flush() if run > 0 then parts[#parts+1] = last .. "x" .. run end last, run = nil, 0 end
  for n in node.traverse(l.list) do
   if n.id == node.id("glyph") then
    if n.expansion_factor == last then run = run + 1 else flush(); last, run = n.expansion_factor, 1 end
   elseif n.id == node.id("margin_kern") then
    flush(); parts[#parts+1] = "mk" .. n.subtype .. ":" .. n.width .. ":" .. n.glyph.char .. ":" .. n.glyph.expansion_factor
   elseif n.id == node.id("kern") and n.expansion_factor ~= 0 then
    flush(); parts[#parts+1] = "k" .. n.kern .. ":" .. n.expansion_factor
   end
  end
  flush()
  out[#out+1] = table.concat(parts, " ")
 end
 return table.concat(out, " | ")
end
function check(box, expect)
 local got = summary(box)
 local f = os.getenv("SUMOUT")
 if f then local fh = io.open(f, "a"); fh:write(got, nl, "--", nl); fh:close()
 elseif got ~= expect then error("MISMATCH:" .. nl .. got .. nl .. "---expected---" .. nl .. expect) end
end
 tex.definefont(true, "xf", font.define(font.read_tfm("cmr10", 655360)))
}\endgroup
\xf \parindent=0pt \parfillskip=0pt plus 1fil \tolerance=10000 \pretolerance=-1 \baselineskip=12pt \hbadness=10000 \hfuzz=1000pt \righthyphenmin=62
\expandglyphsinfont\xf 20 20 5
\efcode\xf`f=400 \efcode\xf`m=0
\lpcode\xf`T=300 \rpcode\xf`.=700
\adjustspacing=2 \protrudechars=2 \hsize=150pt
\setbox1\vbox{AVA Tower Wave Away To Yo AVAVAV Wa Ty Vo Yes Tavo Way Aware Awry Watch Typewriter Wow, Yo-yo. TAVA Voyage Waves.\par}
\directlua{check(tex.box[1], [==[w=9830400 gs=1.5809 sign=1 20000x1 k-72819:-1456 20000x1 k-72819:-1456 20000x2 k-54614:-1092 20000x1 k-18205:-364 20000x1 k-18205:-364 20000x3 k-54614:-1092 20000x1 k-18205:-364 20000x1 k-18205:-364 20000x3 k-18205:-364 20000x1 k-18205:-364 20000x2 k-54614:-1092 20000x2 k-54614:-1092 20000x1 | w=9830400 gs=0.0432 sign=2 -20000x1 k-72819:-1456 -20000x1 k-72819:-1456 -20000x1 k-72819:-1456 -20000x1 k-72819:-1456 -20000x1 k-72819:-1456 -20000x2 k-54614:-1092 -20000x2 k-18205:-364 -20000x2 k-54614:-1092 -20000x2 k-54614:-1092 -20000x3 k-54614:-1092 -20000x1 k-18205:-364 -20000x1 k-18205:-364 -20000x2 k-54614:-1092 -20000x1 k-18205:-364 -20000x1 | w=9830400 gs=1.7973 sign=1 20000x2 k-18205:-364 20000x8 k-54614:-1092 20000x3 k-18205:-364 20000x2 k-18205:-364 20000x2 k18205:364 20000x7 | w=9830400 gs=3.8887 sign=1 0x25 mk1:-458753:46:-20000]==])}
"####);
    assert_clean(&e);
}

/// The codes of a Lua font read back and assign like luatex's: characters
/// the font lacks read as 0 and ignore assignments; `\leftmarginkern` and
/// `\rightmarginkern` report the margin kerns of an hbox (`0pt` without).
#[test]
fn font_codes_and_margin_kern_queries() {
    let e = run_luatex(r####"\begingroup\catcode`\%=12 \catcode`\#=12 \directlua{
 local t = font.read_tfm("cmr10", 655360)
 for c, ch in pairs(t.characters) do ch.expansion_factor = 777 ch.left_protruding = 11 end
 t.characters[65].right_protruding = -33
 tex.definefont(true, "xf", font.define(t))
 tex.definefont(true, "xg", font.define(font.read_tfm("cmr10", 655360)))
}\endgroup
\message{[\the\efcode\xf`A:\the\lpcode\xf`A:\the\rpcode\xf`A:\the\efcode\xf 200:\the\lpcode\xf 200:\the\efcode\xg`A]}
\efcode\xf`A=500 \lpcode\xf`A=-5 \rpcode\xf`A=1234 \efcode\xf 200=5 \lpcode\xf 200=7
\message{[\the\efcode\xf`A:\the\lpcode\xf`A:\the\rpcode\xf`A:\the\efcode\xf 200:\the\lpcode\xf 200]}
\message{[\the\efcode\xf 8364]}
\setbox2\hbox{\xf A\kern1pt}
\message{<\leftmarginkern2:\rightmarginkern2>}
\setbox2\hbox{\directlua{
 local mk = node.new("margin_kern", 0) mk.width = -65536 mk.glyph = node.new("glyph") mk.glyph.font = font.id("xf") mk.glyph.char = 65
 local mr = node.new("margin_kern", 1) mr.width = -131072 mr.glyph = node.new("glyph") mr.glyph.font = font.id("xf") mr.glyph.char = 66
 local g = node.new("glue")
 mk.next = g g.prev = mk g.next = mr mr.prev = g
 node.write(mk)
}}
\message{<\leftmarginkern2:\rightmarginkern2>}
"####);
    assert_clean(&e);
    let term = e.term.replace('\n', "");
    let found: Vec<&str> = ["[777:11:-33:0:0:1000]", "[500:-5:1234:0:0]", "[0]", "<0pt:0pt>", "<-1.0pt:-2.0pt>"].iter().copied().filter(|m| term.contains(m)).collect();
    assert_eq!(found.len(), 5, "{term}");
}

/// The page content of the expanded paragraph: the text-matrix scale of each
/// line (`1.02 0 0 1 x y Tm`), the position of its first glyph, and its text.
#[test]
fn expanded_glyphs_are_scaled_in_the_pdf() {
    let mut e = run_luatex(r####"\begingroup\catcode`\%=12 \catcode`\#=12 \directlua{
local nl = string.char(10)
local function r4(x) return tostring(math.floor(x * 10000 + 0.5) / 10000) end
local function summary(box)
 local out = {}
 for l in node.traverse_id(node.id("hlist"), box.list) do
  local parts = {"w=" .. l.width .. " gs=" .. r4(l.glue_set) .. " sign=" .. l.glue_sign}
  local last, run = nil, 0
  local function flush() if run > 0 then parts[#parts+1] = last .. "x" .. run end last, run = nil, 0 end
  for n in node.traverse(l.list) do
   if n.id == node.id("glyph") then
    if n.expansion_factor == last then run = run + 1 else flush(); last, run = n.expansion_factor, 1 end
   elseif n.id == node.id("margin_kern") then
    flush(); parts[#parts+1] = "mk" .. n.subtype .. ":" .. n.width .. ":" .. n.glyph.char .. ":" .. n.glyph.expansion_factor
   elseif n.id == node.id("kern") and n.expansion_factor ~= 0 then
    flush(); parts[#parts+1] = "k" .. n.kern .. ":" .. n.expansion_factor
   end
  end
  flush()
  out[#out+1] = table.concat(parts, " ")
 end
 return table.concat(out, " | ")
end
function check(box, expect)
 local got = summary(box)
 local f = os.getenv("SUMOUT")
 if f then local fh = io.open(f, "a"); fh:write(got, nl, "--", nl); fh:close()
 elseif got ~= expect then error("MISMATCH:" .. nl .. got .. nl .. "---expected---" .. nl .. expect) end
end
 local t = font.read_tfm("cmr10", 655360)
 t.stretch = 20 t.shrink = 20 t.step = 5
 for c, ch in pairs(t.characters) do
  if c == 102 then ch.expansion_factor = 400 end
  if c == 109 then ch.expansion_factor = 0 end
  if c >= 97 and c <= 122 then ch.left_protruding = 100 ch.right_protruding = 200 end
 end
 tex.definefont(true, "xf", font.define(t))
}\endgroup
\xf \parindent=0pt \parfillskip=0pt plus 1fil \tolerance=10000 \pretolerance=-1 \baselineskip=12pt \hbadness=10000 \hfuzz=1000pt \righthyphenmin=62
\hsize=150pt \protrudechars=2 \adjustspacing=2
\setbox1\vbox{away Tower Wave Away To Yo AVAVAV Wa Ty Vo yes Tavo Way aware awry watch typewriter Wow, yo-yo. TAVA voyage waves.\par}
\outputmode=1 \pdfvariable compresslevel=0 \pdfvariable objcompresslevel=0 \pagewidth=250pt \pageheight=100pt
\shipout\copy1"####);
    assert_clean(&e);
    let bytes = tex_core::driver::finish_pdf(&mut e, false).expect("PDF output");
    let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");
    let page = *pdf.get_pages().values().next().expect("one page");
    let content = String::from_utf8_lossy(&pdf.get_page_content(page)).into_owned();
    // "a 0 0 1 x y Tm [(..)..]TJ" -> "a x text"
    let mut lines = Vec::new();
    let mut rest = content.as_str();
    while let Some(at) = rest.find(" Tm") {
        let nums: Vec<&str> = rest[..at].split_whitespace().rev().take(6).collect();
        let (a, x) = (nums[5], nums[1]);
        let after = &rest[at..];
        let end = after.find("]TJ").expect("TJ after Tm");
        let mut text = String::new();
        let mut depth = 0;
        for ch in after[..end].chars() {
            match ch {
                '(' if depth == 0 => depth = 1,
                ')' if depth == 1 => depth = 0,
                c if depth == 1 => text.push(c),
                _ => {}
            }
        }
        lines.push(format!("{:.2} {:.2} {}", a.parse::<f64>().unwrap(), x.parse::<f64>().unwrap(), text));
        rest = &after[end..];
    }
    assert_eq!(lines, ["1.02 -1.00 awayTowerWaveAwayToYo", "1.00 0.00 AVAVAVWaTyVoyesTavoWay", "0.98 -1.00 awareawrywatchtypewriterWow,", "1.00 -1.00 yo-yo.TAVAvoyagewaves."], "{content}");
}

/// LuaTeX retains a positive half-step when font shrink can cover the
/// excess: zero glue stretch must not prematurely force a syllable break.
#[test]
fn shrink_half_step_preserves_zero_glue_active_path() {
    let e = run_luatex(r####"\directlua{
local f=font.define{name='shrink-transition',size=655360,step=1,stretch=20,shrink=20,characters={
 [65]={width=1000000},[66]={width=1000000},[67]={width=1000000},[68]={width=1000000},[45]={width=100000}}}
function check_shrink_transition(expected)
 local head,tail
 local function append(n) if tail then tail.next=n;n.prev=tail else head=n end;tail=n end
 local function glyph(c) local n=node.new('glyph');n.font=f;n.char=c;return n end
 append(glyph(65));append(glyph(66))
 local d=node.new('disc');d.subtype=3;d.penalty=50;d.pre=glyph(45);append(d)
 append(glyph(67));local g=node.new('glue');g.width=100000;append(g)
 append(glyph(68));local p=node.new('penalty');p.penalty=10000;append(p)
 local fill=node.new('glue');fill.stretch=65536;fill.stretch_order=2;append(fill)
 local lines=tex.linebreak(head,{hsize=2080000,pretolerance=-1,tolerance=200,adjustspacing=2,linepenalty=0})
 local text={}
 for line in node.traverse_id(node.id('hlist'),lines) do
  local chars={}
  for n in node.traverse_id(node.id('glyph'),line.list) do table.insert(chars,string.char(n.char)) end
  table.insert(text,table.concat(chars,''))
 end
 local got=table.concat(text,'|')
 assert(got==expected,'expected '..expected..', got '..got)
 node.flush_list(lines)
end
}
\rightskip=0pt\relax \directlua{check_shrink_transition('ABC|D')}
\rightskip=0pt plus 0.2pt\relax \directlua{check_shrink_transition('AB-|CD')}
"####);
    assert_clean(&e);
}
