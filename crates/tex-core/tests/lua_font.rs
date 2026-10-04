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
    let bytes = tex_core::pdffile::write_pdf(&e.pdf_doc).unwrap();
    let parsed = lopdf::Document::load_mem(&bytes).unwrap();
    let parent = parsed
        .objects
        .values()
        .filter_map(|object| object.as_dict().ok())
        .find(|dict| dict.get(b"Subtype").and_then(lopdf::Object::as_name).ok() == Some(b"Type0"))
        .expect("composite font");
    let descendant = parsed.dereference(&parent.get(b"DescendantFonts").unwrap().as_array().unwrap()[0]).unwrap().1;
    let descriptor = parsed.dereference(descendant.as_dict().unwrap().get(b"FontDescriptor").unwrap()).unwrap().1;
    let descriptor = descriptor.as_dict().unwrap();
    // The exact table above under LuaTeX 1.24 / TeX Live 2026: bounds
    // come from the complete font program, not this table's d/p metrics.
    let bbox: Vec<i64> = descriptor
        .get(b"FontBBox").unwrap().as_array().unwrap()
        .iter().map(|value| value.as_i64().unwrap()).collect();
    assert_eq!(bbox, [-430, -290, 1417, 1127]);
    assert_eq!(descriptor.get(b"Ascent").unwrap().as_i64().unwrap(), 1127);
    assert_eq!(descriptor.get(b"Descent").unwrap().as_i64().unwrap(), -290);
    let cmap = parsed.dereference(parent.get(b"ToUnicode").unwrap()).unwrap().1;
    let cmap = String::from_utf8(cmap.as_stream().unwrap().decompressed_content().unwrap()).unwrap();
    for unicode in ["<0041>", "<0056>", "<006F>", "<0066>", "<0065>"] {
        assert!(cmap.contains(unicode), "missing Unicode mapping {unicode}: {cmap}");
    }
}

/// LuaTeX's Pagella Math descriptor keeps tall glyph bounds separate from
/// baseline metrics; conflating them changes fractions' PDF reading order.
#[test]
fn math_font_descriptor_uses_baseline_metrics_not_glyph_bounds() {
    let mut e = run_luatex(
        r#"\directlua{
 local file = "/<embedded>/fonts/opentype/public/tex-gyre-math/texgyrepagella-math.otf"
 if not lfs.attributes(file) then texio.write_nl("NOFONT") return end
 local id = font.define{
  name="pagella-math", psname="TeXGyrePagellaMath-Regular",
  filename=file, format="opentype", embedding="subset", encodingbytes=2,
  type="real", subfont=1, size=655360, designsize=655360,
  characters={[65]={index=1,width=655360,height=655360}},
 }
 font.current(id)
}
\outputmode=1 \shipout\hbox{A}"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}\n{}", e.diagnostics, e.term);
    if e.term.contains("NOFONT") {
        return;
    }
    let bytes = tex_core::driver::finish_pdf(&mut e, false).expect("PDF output");
    let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");
    let page = *pdf.get_pages().values().next().unwrap();
    let fonts = pdf.get_page_resources(page).unwrap().0.unwrap().get(b"Font").unwrap();
    let fonts = pdf.dereference(fonts).unwrap().1.as_dict().unwrap();
    let parent = pdf.dereference(fonts.iter().next().unwrap().1).unwrap().1.as_dict().unwrap();
    let descendant = pdf.dereference(&parent.get(b"DescendantFonts").unwrap().as_array().unwrap()[0]).unwrap().1;
    let descriptor = pdf.dereference(descendant.as_dict().unwrap().get(b"FontDescriptor").unwrap()).unwrap().1.as_dict().unwrap();
    let bbox: Vec<i64> = descriptor.get(b"FontBBox").unwrap().as_array().unwrap()
        .iter().map(|value| value.as_i64().unwrap()).collect();
    assert_eq!(bbox, [-851, -1775, 3580, 2275]);
    assert_eq!(descriptor.get(b"Ascent").unwrap().as_i64().unwrap(), 726);
    assert_eq!(descriptor.get(b"Descent").unwrap().as_i64().unwrap(), -274);
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

/// `/usr/bin/texlua` reports this metadata even when Lua's `io.open` is
/// replaced: native fontloader.info does not route immutable fonts through Lua IO.
#[test]
fn fontloader_info_retains_native_metadata_with_replaced_lua_io() {
    let e = run_luatex(
        r#"\directlua{
 local file = "/<embedded>/fonts/opentype/public/lm/lmroman10-regular.otf"
 if not lfs.attributes(file) then texio.write_nl("NOFONT") return end
 local open = io.open
 io.open = function() error("Lua IO must not load immutable font metadata") end
 local ok, info = pcall(fontloader.info, file)
 io.open = open
 assert(ok, info)
 assert(info.fontname == "LMRoman10-Regular")
 assert(info.fullname == "LMRoman10-Regular")
 assert(info.familyname == "LM Roman 10")
 assert(info.units_per_em == 1000 and info.italicangle == 0)
 assert(info.pfminfo.weight == 400 and info.pfminfo.width == 5)
 assert(info.filename == file)
}"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}\n{}", e.diagnostics, e.term);
}

/// The same SFNT seeks/reads under `/usr/bin/texlua`: metadata reads, an
/// outline-table read, then backward seeks must keep the original file bytes.
#[test]
fn embedded_font_stream_preserves_metadata_to_outline_transitions() {
    let e = run_luatex(
        r#"\directlua{
 local file = "/<embedded>/fonts/opentype/public/lm/lmroman10-regular.otf"
 if not lfs.attributes(file) then texio.write_nl("NOFONT") return end
 local f = assert(io.open(file, "rb"))
 assert(f:read(4) == "OTTO")
 local count = string.unpack(">I2", f:read(2))
 f:seek("set", 12)
 local tables = {}
 for i = 1, count do
  local tag, checksum, offset, length = string.unpack(">c4I4I4I4", f:read(16))
  tables[tag] = {offset=offset,length=length}
 end
 local head = assert(tables.head)
 f:seek("set", head.offset + 12)
 assert(string.unpack(">I4", f:read(4)) == 0x5F0F3CF5)
 f:seek("set", head.offset + 18)
 assert(string.unpack(">I2", f:read(2)) == 1000)
 local outline = assert(tables["CFF "])
 f:seek("set", outline.offset)
 assert(f:read(1):byte() == 1)
 f:seek("set", outline.offset + outline.length - 1)
 assert(type(f:read(1)) == "string")
 f:seek("set", head.offset + 18)
 assert(string.unpack(">I2", f:read(2)) == 1000)
 assert(f:seek("end") == lfs.attributes(file, "size"))
 f:seek("end", -1)
 assert(type(f:read(1)) == "string" and f:read(1) == nil)
 f:seek("set", head.offset + 12)
 assert(string.unpack(">I4", f:read(4)) == 0x5F0F3CF5)
 f:close()
 assert(not pcall(f.read, f, 1))
 local closed = assert(io.open(file, "rb"))
 assert(closed:read(4) == "OTTO")
 closed:close()
 assert(not pcall(closed.read, closed, 1))
}"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}\n{}", e.diagnostics, e.term);
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

/// luatex's Info dictionary (`luatex --ini` with `\pdfvariable suppressptexinfo=1`
/// and `ptexuseunderscore=1`): /Producer `LuaTeX-1.24.0` and a `PTEX.FullBanner`
/// that neither of the pdfTeX switches touches.
#[test]
fn info_dictionary_names_luatex() {
    let mut e = run_luatex(
        r"\pdfvariable suppressptexinfo=1 \pdfvariable ptexuseunderscore=1 \outputmode=1 \shipout\hbox{}",
    );
    assert_eq!(e.error_count, 0, "errors: {:?}\n{}", e.diagnostics, e.term);
    let bytes = tex_core::driver::finish_pdf(&mut e, false).expect("PDF output");
    let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");
    let info = pdf.trailer.get(b"Info").unwrap().as_reference().unwrap();
    let info = pdf.get_dictionary(info).unwrap();
    let text = |key: &[u8]| String::from_utf8_lossy(info.get(key).unwrap().as_str().unwrap()).into_owned();
    assert_eq!(text(b"Producer"), "LuaTeX-1.24.0");
    assert!(text(b"PTEX.FullBanner").starts_with("This is LuaTeX, Version 1.24.0"));
    assert!(info.get(b"PTEX_FullBanner").is_err() && info.get(b"PTEX.Fullbanner").is_err());
}

#[test]
fn character_snapshot_retains_zero_successor_and_variant_precedence() {
    let e = run_luatex(
        r#"\directlua{
 local id = font.define{
  name="sparse-character-data", size=655360,
  characters={
   [66]={width=100,next=0},
   [68]={width=100,next=69,extensible={rep=70},vert_variants={{glyph=71,advance=17}}}
  }
 }
 local c = font.getcopy(id).characters
 assert(c[66].next == 0)
 assert(c[68].next == nil and c[68].vert_variants[1].glyph == 71)
}"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}\n{}", e.diagnostics, e.term);
}

#[test]
fn unicode_snapshot_preserves_boundary_and_binary_mappings() {
    let e = run_luatex(
        r#"\directlua{
 local short = '00a100b200c300d4'
 local long = short .. 'a'
 local binary = string.rep('00a1',5) .. string.char(254,255)
 local id = font.define{name='unicode-boundary',size=655360,characters={
  [65]={width=10,tounicode=short},
  [66]={width=10,tounicode=long},
  [67]={width=10,tounicode=binary},
  [68]={width=10,tounicode=''},
  [69]={width=10},
  [70]={width=10,tounicode=128512},
  [71]={width=10,tounicode={65,128512,66,67,68,69}}
 }}
 local c = font.getcopy(id).characters
 assert(c[65].tounicode == short)
 assert(c[66].tounicode == long)
 assert(c[67].tounicode == binary)
 assert(c[68].tounicode == '' and c[69].tounicode == nil)
 assert(c[70].tounicode == 'D83DDE00')
 assert(c[71].tounicode == '0041D83DDE000042004300440045')
}"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}\n{}", e.diagnostics, e.term);
}
