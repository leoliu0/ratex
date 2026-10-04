//! LuaTeX's `glyph_info` callback (printing.c `print_character_info`): with a
//! function registered, every character a box display prints (`\showbox`,
//! `\showlists`, `\tracingoutput`, the short display of an overfull box,
//! ligature and discretionary parts included) is the string
//! `glyph_info(<glyph node>)` returns; nil or false prints `"<hex>` and so
//! does a wrong type (after "callback should return a string, false or nil,
//! not: <type>" on stderr, which a test cannot see). Registered as `false`
//! also prints `"<hex>`; unregistered, the character itself.
//!
//! Each expectation is what TeX Live 2026 `luatex -ini -interaction=nonstopmode
//! t.tex` writes to the log for the same source (`t.tex`: the SETUP lines, the
//! `\font\bnd=bndlig` line for the TFM cases, `\directlua{LIB}`, the case's
//! body and `\end`), reduced to the display blocks by `blocks` below. The
//! fonts are a table font made by `LIB` (no files) and
//! `tests/fixtures/bndlig.tfm`; nothing here reads a TeX tree.

use tex_core::engine::{Engine, EngineKind, InteractionMode};

const SETUP: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6\n\\directlua{tex.enableprimitives('',tex.extraprimitives())}\n\\tracingonline1 \\showboxbreadth100 \\showboxdepth100\n";

const LIB: &str = r#" local function ch(w) return {width=w*65536, height=7*65536, depth=0} end
 local f = {
  name='tf', psname='tf', format='type1', type='real', filename='tf.pfb', embedding='no', encodingbytes=1,
  size=10*65536, designsize=10*65536, units_per_em=1000,
  parameters={slant=0, space=3*65536, space_stretch=65536, space_shrink=65536, x_height=4*65536, quad=10*65536, extra_space=0},
  characters={
   [45]=ch(2), [65]=ch(6), [66]=ch(7), [67]=ch(5), [68]=ch(5), [69]=ch(5), [70]=ch(5), [71]=ch(5), [72]=ch(5), [102]=ch(4), [105]=ch(3), [11]=ch(8), [12]=ch(7), [0x4E2D]=ch(10),
  },
 }
 f.characters[102].ligatures={[105]={type=0,char=12},[102]={type=0,char=11}}
 f.characters[65].kerns={[66]=65536*2}
 FID=font.define(f)
 function G(c) local g=node.new('glyph') g.font=FID g.char=c g.lang=0 return g end
 function W(...) local h for _,c in ipairs{...} do local g=G(c) if h then node.insert_after(h,node.tail(h),g) else h=g end end node.write(h) end
"#;

/// The tests run in parallel in one process: lift the memory cap (measured
/// on the whole process) and pin the clock.
fn pin_environment() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var("TEX_MEM_LIMIT_MIB", "0");
        std::env::set_var("FORCE_SOURCE_DATE", "1");
        std::env::set_var("SOURCE_DATE_EPOCH", "1700000000");
    });
}

/// Run `body` after SETUP and LIB; with `tfm`, `\bnd` is
/// `tests/fixtures/bndlig.tfm` (`\font\bnd=tests/fixtures/bndlig`, found
/// relative to the crate directory, loaded before LIB so Lua's font is
/// font 2 as in luatex).
fn run(tfm: bool, body: &str) -> Engine {
    pin_environment();
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.add_nullfont();
    for (c, cat) in [(b'{', 1), (b'}', 2), (b'#', 6), (b' ', 10), (b'\n', 5)] {
        e.eqtb.cat[c as usize] = cat;
    }
    e.set_interaction_mode(InteractionMode::Nonstop);
    e.job_name = "job".to_string();
    let font = if tfm { "\\font\\bnd=tests/fixtures/bndlig\n" } else { "" };
    e.input.push_file("t.tex".to_string(), format!("{SETUP}{font}\\directlua{{\n{LIB}}}\n{body}\\end\n").into_bytes());
    e.run();
    e.finish_job_diagnostics();
    e
}

/// The box displays, `\showlists` reports and callback output lines of a log
/// (luatex and TeXres frame their error messages differently): a block
/// starts at a line beginning with one of the display openers and ends at an
/// empty line or an error message line.
fn blocks(log: &str) -> String {
    const STARTS: [&str; 8] =
        ["> \\box", "### ", "Overfull", "Underfull", "\\hbox", "\\vbox", "Completed box", "CALL"];
    const NEW_BLOCK: [&str; 6] = ["> \\box", "### ", "CALL", "Overfull", "Underfull", "Completed"];
    let begins = |line: &str, set: &[&str]| set.iter().any(|s| line.starts_with(s));
    let mut out: Vec<String> = Vec::new();
    let mut cur: Option<Vec<&str>> = None;
    for line in log.split('\n') {
        if line.is_empty() || ["! ", "l.", "  -->", "warning:"].iter().any(|s| line.starts_with(s)) {
            out.extend(cur.take().map(|c| c.join("\n")));
        } else if begins(line, &STARTS) {
            if cur.is_some() && begins(line, &NEW_BLOCK) {
                out.extend(cur.take().map(|c| c.join("\n")));
            }
            cur.get_or_insert_with(Vec::new).push(line);
        } else if let Some(c) = cur.as_mut() {
            c.push(line);
        }
    }
    out.extend(cur.map(|c| c.join("\n")));
    out.join("\n\n")
}

/// A returned string is printed for the character; nil and false print `"<hex>`, and so do `true`, a number and a table (wrong types); the empty string prints nothing; `\newlinechar` in the string ends the line.
#[test]
fn strings_and_other_returns() {
    let e = run(false, r#"\directlua{
 callback.register('glyph_info', function(n)
   local c = n.char
   if c == 65 then return '<A>' end
   if c == 66 then return nil end
   if c == 67 then return false end
   if c == 68 then return true end
   if c == 69 then return 42 end
   if c == 70 then return {} end
   if c == 71 then return '' end
   if c == 72 then return 'x'..string.char(10)..'y' end
   return '?'
 end)
}
\newlinechar=10
\setbox1\hbox{\directlua{W(65,66,67,68,69,70,71,72,0x4E2D)}}
\showbox1
"#);
    assert_eq!(blocks(&e.log), r#"> \box1=
\hbox(7.0+0.0)x55.0, direction TLT
.\FONT1 <A>
.\kern2.0 (font)
.\FONT1 "42
.\FONT1 "43
.\FONT1 "44
.\FONT1 "45
.\FONT1 "46
.\FONT1 
.\FONT1 x
y
.\FONT1 ?"#, "{}", e.log);
}

/// `print_font_and_char` asks for the ligature glyph itself and, through `short_display`, for each component (subtype 1); the three lists of a discretionary are displayed with their glyphs.
#[test]
fn ligature_and_discretionary() {
    let e = run(false, r#"\directlua{
 callback.register('glyph_info', function(n)
   return '<'..n.char..':'..n.subtype..(n.components and 'L' or '')..'>'
 end)
}
\setbox1\hbox{\directlua{font.current(FID)}A\discretionary{AB}{fi}{B}\-ffi B}
\showbox1
\setbox1\hbox{\directlua{W(65,66,102,105,102,102,105,0x4E2D)}}
\showbox1
"#);
    assert_eq!(blocks(&e.log), r#"> \box1=
\hbox(7.0+0.0)x36.0, direction TLT
.\FONT1 <65:0>
.\discretionary (penalty 0)
..< \FONT1 <65:0>
..< \kern2.0 (font)
..< \FONT1 <66:0>
..> \FONT1 <12:2L> (ligature <102:1><105:1>)
..= \kern2.0 (font)
..= \FONT1 <66:0>
.\discretionary (penalty 0)
..< \FONT1 <45:0>
.\FONT1 <11:2L> (ligature <102:1><102:1>)
.\FONT1 <105:0>
.\glue(\spaceskip) 3.0 plus 1.0 minus 1.0
.\FONT1 <66:0>

> \box1=
\hbox(7.0+0.0)x43.0, direction TLT
.\FONT1 <65:0>
.\kern2.0 (font)
.\FONT1 <66:0>
.\FONT1 <12:2L> (ligature <102:0><105:0>)
.\FONT1 <11:2L> (ligature <102:0><102:0>)
.\FONT1 <105:0>
.\FONT1 <20013:0>"#, "{}", e.log);
}

/// The short display of an overfull `\hbox` (in a `\vbox` too) asks the callback for every character, ligature components and discretionary parts included, and then `\showbox` asks again.
#[test]
fn overfull_short_display() {
    let e = run(false, r#"\directlua{
 callback.register('glyph_info', function(n) return '<'..n.char..'>' end)
}
\hfuzz=0pt
\directlua{font.current(FID)}\setbox1\hbox to 1pt{\directlua{W(65,66,102,105,102,102,105)}\discretionary{AB}{fi}{B}\hbox{\directlua{W(66)}}}
\setbox1\vbox{\hbox to 1pt{\directlua{W(65,66)}}}
\showbox1
"#);
    assert_eq!(blocks(&e.log), r#"Overfull \hbox (46.0pt too wide) detected at line 24
\FONT1 <65><66><102><105><102><102><105><65><66><102><105>[]

\hbox(7.0+0.0)x1.0, direction TLT
.\FONT1 <65>
.\kern2.0 (font)
.\FONT1 <66>
.\FONT1 <12> (ligature <102><105>)
.\FONT1 <11> (ligature <102><102>)
.\FONT1 <105>
.\discretionary (penalty 0)
..< \FONT1 <65>
..< \kern2.0 (font)
..< \FONT1 <66>
..> \FONT1 <12> (ligature <102><105>)
..= \FONT1 <66>
.\hbox(7.0+0.0)x7.0, direction TLT
..\FONT1 <66>

Overfull \hbox (14.0pt too wide) detected at line 25
\FONT1 <65><66>

\hbox(7.0+0.0)x1.0, direction TLT
.\FONT1 <65>
.\kern2.0 (font)
.\FONT1 <66>

> \box1=
\vbox(7.0+0.0)x1.0, direction TLT
.\hbox(7.0+0.0)x1.0, direction TLT
..\FONT1 <65>
..\kern2.0 (font)
..\FONT1 <66>"#, "{}", e.log);
}

/// TFM characters: subtype 0 in a finished box, 1 while typed into a list `\showlists` shows; glyphs made by Lua keep their own.
#[test]
fn tfm_font_and_showlists() {
    let e = run(true, r#"\directlua{
 callback.register('glyph_info', function(n) return '<'..n.char..':'..n.subtype..'>' end)
}
\setbox1\hbox{\bnd xy}
\showbox1
\hbox{\bnd xy\directlua{W(65)}\showlists}
\setbox1\hbox{\directlua{W(65)}\hbox{\bnd xy}}
\showbox1
"#);
    assert_eq!(blocks(&e.log), r#"> \box1=
\hbox(1.0+0.0)x47.99998, direction TLT
.\bnd <120:0>
.\bnd <121:0>

### restricted horizontal mode entered at line 26
\bnd <120:1>
\bnd <121:1>
\FONT1 <65:0>
spacefactor 1000

### vertical mode entered at line 0
prevdepth ignored

> \box1=
\hbox(7.0+0.0)x53.99998, direction TLT
.\FONT1 <65:0>
.\hbox(1.0+0.0)x47.99998, direction TLT
..\bnd <120:0>
..\bnd <121:0>"#, "{}", e.log);
}

/// The glyph node handed over carries its attribute list.
#[test]
fn node_attributes() {
    let e = run(true, r#"\directlua{
 callback.register('glyph_info', function(n)
   return '['..n.char..':'..tostring(node.has_attribute(n,7))..']'
 end)
}
\attribute7=5
\setbox1\hbox{\bnd x\attribute7=-"7FFFFFFF y\attribute7=9 x\directlua{W(65)}}
\showbox1
"#);
    assert_eq!(blocks(&e.log), r#"> \box1=
\hbox(7.0+0.0)x69.99998, direction TLT
.\bnd [120:5]
.\bnd [121:nil]
.\bnd [120:9]
.\FONT1 [65:9]"#, "{}", e.log);
}

/// No callback prints characters; a function prints its strings; registered as `false` the callback is "defined" and prints `"<hex>`; nil unregisters it.
#[test]
fn registered_false_and_unregistered() {
    let e = run(false, r#"\setbox1\hbox{\directlua{W(65,66)}}
\showbox1
\directlua{callback.register('glyph_info', function(n) return 'x'..n.char end)}
\showbox1
\directlua{callback.register('glyph_info', false)}
\showbox1
\directlua{callback.register('glyph_info', function(n) return 'y'..n.char end)}
\showbox1
\directlua{callback.register('glyph_info', nil)}
\showbox1
"#);
    assert_eq!(blocks(&e.log), r#"> \box1=
\hbox(7.0+0.0)x15.0, direction TLT
.\FONT1 A
.\kern2.0 (font)
.\FONT1 B

> \box1=
\hbox(7.0+0.0)x15.0, direction TLT
.\FONT1 x65
.\kern2.0 (font)
.\FONT1 x66

> \box1=
\hbox(7.0+0.0)x15.0, direction TLT
.\FONT1 "41
.\kern2.0 (font)
.\FONT1 "42

> \box1=
\hbox(7.0+0.0)x15.0, direction TLT
.\FONT1 y65
.\kern2.0 (font)
.\FONT1 y66

> \box1=
\hbox(7.0+0.0)x15.0, direction TLT
.\FONT1 A
.\kern2.0 (font)
.\FONT1 B"#, "{}", e.log);
}

/// What the callback itself writes appears where luatex writes it: in the middle of the display, before its string.
#[test]
fn callback_output_is_interleaved() {
    let e = run(false, r#"\directlua{
 N = 0
 callback.register('glyph_info', function(n)
   N = N + 1
   texio.write_nl('CALL '..N..' '..n.char)
   return 'g'..N
 end)
}
\setbox1\hbox{\directlua{W(65,66)}}
\showbox1
\hfuzz=0pt
\setbox1\hbox to 1pt{\directlua{W(65,66)}}
"#);
    assert_eq!(blocks(&e.log), r#"> \box1=
\hbox(7.0+0.0)x15.0, direction TLT
.\FONT1 

CALL 1 65g1
.\kern2.0 (font)
.\FONT1 

CALL 2 66g2

Overfull \hbox (14.0pt too wide) detected at line 31
\FONT1 

CALL 3 65g3

CALL 4 66g4

\hbox(7.0+0.0)x1.0, direction TLT
.\FONT1 

CALL 5 65g5
.\kern2.0 (font)
.\FONT1 

CALL 6 66g6"#, "{}", e.log);
}

/// `\tracingoutput` of the shipped box (the PDF/DVI font setup of the table font is not under test).
#[test]
fn tracingoutput_and_showlists() {
    let e = run(false, r#"\directlua{
 callback.register('glyph_info', function(n) return '<'..n.char..'>' end)
}
\setbox1\hbox{\directlua{W(65,66,102,105)}}
\tracingoutput1 \shipout\copy1
\showlists
"#);
    assert_eq!(blocks(&e.log), r#"Completed box being shipped out [0]
\hbox(7.0+0.0)x22.0, direction TLT
.\FONT1 <65>
.\kern2.0 (font)
.\FONT1 <66>
.\FONT1 <12> (ligature <102><105>)

### vertical mode entered at line 0
prevdepth ignored"#, "{}", e.log);
}

/// An error in the callback is reported and the character prints as
/// `"<hex>`.
#[test]
fn error_in_callback_prints_hex() {
    let e = run(
        false,
        r####"\directlua{callback.register('glyph_info', function(n) error('boom') end)}
\setbox1\hbox{\directlua{W(65,66)}}
\showbox1
"####,
    );
    assert!(e.log.contains("\"41") && e.log.contains("\"42"), "{}", e.log);
    assert!(e.log.contains("boom"), "{}", e.log);
    assert!(e.error_count >= 2, "{}", e.log);
}
