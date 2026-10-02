//! LuaTeX font system. Expected values come from `luatex --ini` /
//! `luahbtex --ini` (TeX Live 2026) running the same input.

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

/// `font.read_tfm` of cmr10 carries the metrics luatex reports, and the
/// table is accepted back by `font.define`; the resulting font kerns and
/// measures like the TFM font does in luatex.
#[test]
fn read_tfm_table_round_trips_through_define() {
    let e = run_luatex(
        r#"\directlua{
 local t = font.read_tfm("cmr10", 655360)
 assert(t.checksum == 1274110073 and t.designsize == 655360 and t.size == 655360)
 assert(t.parameters.space == 218453 and t.parameters.quad == 655361)
 local a = t.characters[65]
 assert(a.width == 491521 and a.height == 447828 and a.depth == 0)
 assert(a.kerns[86] == -72819)
 assert(t.characters[102].ligatures[105].char == 12)
 t.name = "mycmr"
 local id = font.define(t)
 font.current(id)
 assert(font.getfont(id).characters[65].width == 491521)
 texio.write_nl("ID=" .. id .. " max=" .. font.max())
}
\setbox0\hbox{AV}
\message{WD=\the\wd0 FN=\fontname\font FD=\the\fontdimen2\font}"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}\n{}", e.diagnostics, e.term);
    assert!(e.term.contains("ID=1 max=1"), "{}", e.term);
    // 13.8889pt = 2 x 4.91521pt - 1.11113pt of kern.
    assert!(e.term.contains("WD=13.8889ptFN=mycmrFD=3.33333pt"), "{}", e.term);
}

/// A hand-written table for an OpenType font (glyph indices) is embedded as
/// a subset CID font whose text extracts as typeset.
#[test]
fn opentype_font_from_table_is_embedded_with_tounicode() {
    let src = r#"\directlua{
 local file = kpse.find_file("lmroman10-regular.otf")
 if not file then texio.write_nl("NOFONT") return end
 local id = font.define{
  name="lmroman10-regular", psname="LMRoman10-Regular", fullname="LMRoman10-Regular",
  filename=file, format="opentype", embedding="subset", encodingbytes=2, type="real", subfont=1,
  size=655360, designsize=655360,
  characters={
   [32]={index=3,width=332871},
   [65]={index=27,width=491520,height=469237,depth=0},
   [86]={index=111,width=491520,height=447610,depth=14417},
   [111]={index=81,width=327680,height=293601,depth=7208},
   [102]={index=55,width=200540,height=462028,depth=0},
   [105]={index=66,width=182190,height=430571,depth=0},
   [99]={index=43,width=290980,height=293601,depth=7208},
   [101]={index=50,width=290980,height=293601,depth=7208},
  },
  parameters={slant=0, space=332871, space_stretch=166436, space_shrink=110957, x_height=430555, quad=655360, extra_space=0},
 }
 font.current(id)
}
\outputmode=1 \pagewidth=100pt \pageheight=50pt \pdfvariable compresslevel=0 \pdfvariable objcompresslevel=0
\setbox0\hbox{AVo office}
\message{WD=\the\wd0 HT=\the\ht0 FN=\fontname\font}
\shipout\hbox{\raise10pt\copy0}"#;
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.add_nullfont();
    for (c, cat) in [(b'{', 1), (b'}', 2), (b'#', 6), (b' ', 10), (b'\n', 5)] {
        e.eqtb.cat[c as usize] = cat;
    }
    e.input.push_file(
        "t.tex".to_string(),
        format!("\\directlua{{tex.enableprimitives('',tex.extraprimitives())}}\n{src}\n\\end\n").into_bytes(),
    );
    e.run();
    if e.term.contains("NOFONT") {
        return;
    }
    assert_eq!(e.error_count, 0, "errors: {:?}\n{}", e.diagnostics, e.term);
    // luatex: WD=47.8592ptHT=7.15999ptFN=lmroman10-regular
    assert!(e.term.contains("WD=47.8592ptHT=7.15999ptFN=lmroman10-regular"), "{}", e.term);
    // the table's `subfont=1` (what luaotfload supplies) names the first face of a plain font file
    e.embed_used_fonts().expect("the font program of a Lua font is embedded");
    let text: String = e
        .pdf_doc
        .native_bindings
        .values()
        .flat_map(|bindings| bindings.iter().flat_map(|b| b.entries.iter().map(|(_, _, t)| t.clone())))
        .collect();
    assert!(text.contains('A') && text.contains('V') && text.contains('o') && text.contains('f') && text.contains('e'), "ToUnicode text: {text:?}");
}

/// The luaharfbuzz API luaotfload uses behaves like luahbtex's: shaping
/// "office fi Tx" with the Latin Modern face gives luahbtex's glyph run.
#[test]
fn luaharfbuzz_shapes_like_luahbtex() {
    let e = run_luatex(
        r#"\directlua{
 local file = kpse.find_file("lmroman10-regular.otf")
 if not file then texio.write_nl("NOFONT") return end
 local hb = require("luaharfbuzz")
 local face = hb.Face.new(file)
 assert(face:get_upem() == 1000 and face:get_glyph_count() == 821)
 assert(face:get_name(hb.ot.NAME_ID_POSTSCRIPT_NAME) == "LMRoman10-Regular")
 local font = hb.Font.new(face)
 font:set_scale(1000, 1000)
 assert(font:get_nominal_glyph(string.byte("f")) == 55)
 assert(font:get_glyph_h_advance(55) == 306)
 local buf = hb.Buffer.new()
 buf:add_utf8("office fi Tx")
 buf:set_direction(hb.Direction.new("ltr"))
 buf:set_script(hb.Script.new("Latn"))
 buf:set_language(hb.Language.new("en"))
 assert(hb.shape_full(font, buf, {hb.Feature.new("+liga"), hb.Feature.new("+kern")}, {}))
 local g = buf:get_glyphs()
 local got = {}
 for i, x in ipairs(g) do got[i] = x.codepoint .. ":" .. x.cluster .. ":" .. x.x_advance end
 texio.write_nl("HB=" .. table.concat(got, ","))
 local f = hb.Feature.new("+smcp[2:5]")
 assert(tostring(f) == "smcp[2:5]" and f.value == 1 and f.start == 2 and f._end == 5)
 assert(tostring(hb.Feature.new("-kern")) == "-kern")
 assert(hb.Feature.new("ss01=3").value == 3)
 local gsub = hb.Tag.new("GSUB")
 local ok, si = face:ot_layout_find_script(gsub, hb.Tag.new("latn"))
 assert(ok and si >= 0)
}"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}\n{}", e.diagnostics, e.term);
    if e.term.contains("NOFONT") {
        return;
    }
    let line = e.term.lines().find(|l| l.contains("HB=")).unwrap_or_default().to_string();
    // The first nine entries of luahbtex's run ("office fi": ffi is one glyph).
    assert!(line.contains("HB=81:0:500.0,123:1:833.0,43:4:444.0,50:5:444.0,103:6:333.0,125:7:556.0,103:9:333.0,104:10:639.0,116:11:528.0"), "{line}");
}

/// The font library exports luatex's member list.
#[test]
fn font_library_has_luatex_members() {
    let e = run_luatex(
        r#"\directlua{
 local names = {"addcharacters","current","define","each","fonts","frozen","getcopy","getfont",
   "getparameters","id","max","nextid","read_tfm","read_vf","setexpansion","setfont","settounicode"}
 local want = {}
 for _, n in ipairs(names) do want[n] = true; assert(font[n] ~= nil, n) end
 for k in pairs(font) do assert(want[k], "unexpected font." .. k) end
 for _, n in ipairs{"apply_afmfile","apply_featurefile","close","fields","info","open","to_table"} do
   assert(type(fontloader[n]) == "function", n)
 end
}"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}\n{}", e.diagnostics, e.term);
}
