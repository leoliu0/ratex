//! `luaharfbuzz` (the HarfBuzz binding shipped with luahbtex and used by
//! luaotfload's harf renderer), implemented over rustybuzz/ttf-parser.
//!
//! Rust provides the primitive operations on Lua-owned parsed faces; the
//! class layer (`Face`, `Font`, `Buffer`,
//! `Feature`, `Tag`, `Script`, `Direction`, `Language`, `Blob`, `ot`, ...) is
//! Lua with the member names of luaharfbuzz.

use std::any::Any;
use std::rc::Rc;

use tex_lua::{CallbackLua, Lua, LuaApi, LuaBytes, LuaString, LuaTable, UserDataTrait, Value};

struct HbFace {
    // Field order matters: `face` borrows `_data` and must drop first.
    face: rustybuzz::Face<'static>,
    _data: Rc<Vec<u8>>,
}

/// The Lua handle owns the font bytes; closing a font releases them immediately,
/// and unreachable HarfBuzz faces release them when Lua collects the handle.
struct FaceHandle(Option<Rc<HbFace>>);

impl UserDataTrait for FaceHandle {
    fn type_name(&self) -> &'static str {
        "luaharfbuzz.face"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn face(value: Value) -> Result<Rc<HbFace>, String> {
    let handle = value
        .as_userdata::<FaceHandle>()
        .ok_or_else(|| "luaharfbuzz: invalid face".to_string())?;
    let handle = handle.borrow().map_err(|e| format!("{e:?}"))?;
    handle.0.clone().ok_or_else(|| "luaharfbuzz: closed face".to_string())
}

fn tag_of(n: i64) -> ttf_parser::Tag {
    ttf_parser::Tag(n as u32)
}

fn err(cx: &mut CallbackLua<'_>, e: impl std::fmt::Debug) -> tex_lua::LuaError {
    cx.error(format!("luaharfbuzz: {e:?}"))
}

fn arr(cx: &mut CallbackLua<'_>, values: &[i64]) -> Result<LuaTable, tex_lua::LuaError> {
    let t = cx.create_table_with_capacity(values.len(), 0)?;
    for (i, v) in values.iter().enumerate() {
        t.raw_seti(i as i64 + 1, *v)?;
    }
    Ok(t)
}

fn layout_table<'a>(
    f: &'a ttf_parser::Face<'static>,
    which: i64,
) -> Option<ttf_parser::opentype_layout::LayoutTable<'a>> {
    if which == i64::from(u32::from_be_bytes(*b"GSUB")) {
        f.tables().gsub
    } else if which == i64::from(u32::from_be_bytes(*b"GPOS")) {
        f.tables().gpos
    } else {
        None
    }
}

fn weight_name(weight: u16) -> &'static str {
    match weight {
        ..=150 => "Thin",
        151..=250 => "ExtraLight",
        251..=350 => "Light",
        351..=450 => "Regular",
        451..=550 => "Medium",
        551..=650 => "Demi",
        651..=750 => "Bold",
        751..=850 => "ExtraBold",
        _ => "Black",
    }
}

fn embedded_face_meta(
    cx: &mut CallbackLua<'_>,
    info: &tex_kpse::EmbeddedFontInfo,
    filename: &LuaString,
) -> Result<LuaTable, tex_lua::LuaError> {
    let t = cx.create_table_with_capacity(0, 17)?;
    for (key, value) in [
        ("fontname", info.fontname),
        ("fullname", info.fullname),
        ("familyname", info.familyname),
        ("copyright", info.copyright),
        ("version", info.version),
        ("weight", weight_name(info.weight)),
    ] {
        t.set(key, cx.create_bytes(value.as_bytes())?)?;
    }
    t.set("italicangle", f64::from(f32::from_bits(info.italic_angle_bits)))?;
    t.set("units_per_em", i64::from(info.units_per_em))?;
    t.set("ascent", i64::from(info.ascent))?;
    t.set("descent", -i64::from(info.descender))?;
    t.set("glyphcnt", i64::from(info.glyph_count))?;
    t.set("glyphmax", i64::from(info.glyph_count) - 1)?;
    t.set("glyphmin", 0i64)?;
    t.set("filename", filename)?;
    t.set("table_version", 20190101i64)?;
    let pfminfo = cx.create_table_with_capacity(0, 3)?;
    pfminfo.set("weight", i64::from(info.weight))?;
    pfminfo.set("width", i64::from(info.width))?;
    pfminfo.set("panose_set", false)?;
    t.set("pfminfo", pfminfo)?;
    Ok(t)
}

const DEFAULT_LANGUAGE_INDEX: i64 = 0xFFFF;

pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let n: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;
    macro_rules! native {
        ($name:literal, $body:expr) => {
            n.set($name, lua.create_callback($body).map_err(|e| format!("{}: {e:?}", $name))?)
                .map_err(|e| format!("{}: {e:?}", $name))?
        };
    }

    native!("embedded_info", |cx| {
        let filename = cx.arg::<LuaString>(1)?;
        let faces = filename.as_str().and_then(|path| tex_kpse::embedded_font_info(&path));
        match faces {
            Some([info]) => {
                let t = embedded_face_meta(cx, info, &filename)?;
                cx.push(t)
            }
            Some(infos) if !infos.is_empty() => {
                let t = cx.create_table_with_capacity(infos.len(), 0)?;
                for (index, info) in infos.iter().enumerate() {
                    let meta = embedded_face_meta(cx, info, &filename)?;
                    t.raw_seti(index as i64 + 1, meta)?;
                }
                cx.push(t)
            }
            _ => cx.push(Option::<i64>::None),
        }
    });

    native!("face_new", |cx| {
        let data = cx.arg::<LuaString>(1)?.to_bytes();
        let index = cx.arg::<Option<i64>>(2)?.unwrap_or(0).max(0) as u32;
        let data = Rc::new(data);
        // SAFETY: `HbFace` keeps the Rc alive for as long as the face.
        let slice: &'static [u8] = unsafe { std::mem::transmute::<&[u8], &'static [u8]>(data.as_slice()) };
        match rustybuzz::Face::from_slice(slice, index) {
            Some(face) => {
                let handle = cx.create_userdata(FaceHandle(Some(Rc::new(HbFace { face, _data: data }))))?;
                cx.push(handle)
            }
            None => cx.push(Option::<i64>::None),
        }
    });

    native!("face_close", |cx| {
        let value: Value = cx.arg(1)?;
        let handle = value.as_userdata::<FaceHandle>().ok_or_else(|| cx.error("luaharfbuzz: invalid face"))?;
        handle.borrow_mut().map_err(|e| err(cx, e))?.0 = None;
        Ok(0)
    });

    native!("face_info", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let a = cx.push(i64::from(f.face.units_per_em()))?;
        let b = cx.push(i64::from(f.face.number_of_glyphs()))?;
        Ok(a + b)
    });

    native!("face_tables", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let tags: Vec<i64> = f.face.raw_face().table_records.into_iter().map(|r| i64::from(r.tag.0)).collect();
        let t = arr(cx, &tags)?;
        cx.push(t)
    });

    native!("face_table", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let tag = tag_of(cx.arg(2)?);
        match f.face.raw_face().table(tag) {
            Some(d) => {
                let s = cx.create_bytes(d)?;
                cx.push(s)
            }
            None => cx.push(Option::<i64>::None),
        }
    });

    native!("face_name", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let id: i64 = cx.arg(2)?;
        let mut best: Option<String> = None;
        for name in f.face.names() {
            if i64::from(name.name_id) == id {
                if let Some(s) = name.to_string() {
                    // Prefer Windows Unicode names (platform 3) like hb's lookup.
                    if best.is_none() || name.platform_id == ttf_parser::PlatformId::Windows {
                        best = Some(s);
                    }
                }
            }
        }
        cx.push(best.map(|s| LuaBytes(s.into_bytes())))
    });

    native!("nominal", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let cp: i64 = cx.arg(2)?;
        let g = char::from_u32(cp as u32).and_then(|c| f.face.glyph_index(c)).map(|g| i64::from(g.0));
        cx.push(g)
    });

    native!("advance", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let g = ttf_parser::GlyphId(cx.arg::<i64>(2)? as u16);
        let vertical = cx.arg::<Option<bool>>(3)?.unwrap_or(false);
        let adv = if vertical {
            f.face.glyph_ver_advance(g).map(i64::from).unwrap_or(0)
        } else {
            f.face.glyph_hor_advance(g).map(i64::from).unwrap_or(0)
        };
        cx.push(adv)
    });

    native!("glyph_extents", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let g = ttf_parser::GlyphId(cx.arg::<i64>(2)? as u16);
        match f.face.glyph_bounding_box(g) {
            Some(r) => {
                let t = cx.create_table_with_capacity(0, 4)?;
                t.set("x_bearing", i64::from(r.x_min))?;
                t.set("y_bearing", i64::from(r.y_max))?;
                t.set("width", i64::from(r.x_max) - i64::from(r.x_min))?;
                t.set("height", i64::from(r.y_min) - i64::from(r.y_max))?;
                cx.push(t)
            }
            None => cx.push(Option::<i64>::None),
        }
    });

    native!("h_extents", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let t = cx.create_table_with_capacity(0, 3)?;
        t.set("ascender", i64::from(f.face.ascender()))?;
        t.set("descender", i64::from(f.face.descender()))?;
        t.set("line_gap", i64::from(f.face.line_gap()))?;
        cx.push(t)
    });

    native!("glyph_name", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let g = ttf_parser::GlyphId(cx.arg::<i64>(2)? as u16);
        cx.push(f.face.glyph_name(g).map(|s| LuaBytes(s.as_bytes().to_vec())))
    });

    native!("glyph_from_name", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let name = cx.arg::<LuaString>(2)?.to_string_lossy();
        cx.push(f.face.glyph_index_by_name(&name).map(|g| i64::from(g.0)))
    });

    native!("unicodes", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let mut set = std::collections::BTreeSet::new();
        if let Some(cmap) = f.face.tables().cmap {
            for st in cmap.subtables {
                if st.is_unicode() {
                    st.codepoints(|c| {
                        set.insert(i64::from(c));
                    });
                }
            }
        }
        let v: Vec<i64> = set.into_iter().collect();
        let t = arr(cx, &v)?;
        cx.push(t)
    });

    // layout_tags(face, table, kind, script_index, lang_index) -> list of tags
    // kind 1 scripts, 2 languages of a script, 3 features of a language system.
    native!("layout_tags", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let which: i64 = cx.arg(2)?;
        let kind: i64 = cx.arg(3)?;
        let si: i64 = cx.arg::<Option<i64>>(4)?.unwrap_or(0);
        let li: i64 = cx.arg::<Option<i64>>(5)?.unwrap_or(DEFAULT_LANGUAGE_INDEX);
        let mut out: Vec<i64> = Vec::new();
        let mut found = false;
        if let Some(lt) = layout_table(&f.face, which) {
            found = true;
            match kind {
                1 => out.extend(lt.scripts.into_iter().map(|s| i64::from(s.tag.0))),
                2 => {
                    if let Some(s) = lt.scripts.get(si as u16) {
                        out.extend(s.languages.into_iter().map(|l| i64::from(l.tag.0)));
                    }
                }
                _ => {
                    if let Some(s) = lt.scripts.get(si as u16) {
                        let ls = if li == DEFAULT_LANGUAGE_INDEX {
                            s.default_language
                        } else {
                            s.languages.get(li as u16)
                        };
                        if let Some(ls) = ls {
                            let mut seen = Vec::new();
                            for fi in ls.required_feature.into_iter().chain(ls.feature_indices) {
                                if let Some(feat) = lt.features.get(fi) {
                                    if !seen.contains(&feat.tag.0) {
                                        seen.push(feat.tag.0);
                                        out.push(i64::from(feat.tag.0));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if !found || out.is_empty() {
            return cx.push(Option::<i64>::None);
        }
        let t = arr(cx, &out)?;
        cx.push(t)
    });

    // layout_find(face, table, kind, a, b, tag) -> found, index
    native!("layout_find", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let which: i64 = cx.arg(2)?;
        let kind: i64 = cx.arg(3)?;
        let si: i64 = cx.arg::<Option<i64>>(4)?.unwrap_or(0);
        let li: i64 = cx.arg::<Option<i64>>(5)?.unwrap_or(DEFAULT_LANGUAGE_INDEX);
        let tag = tag_of(cx.arg(6)?);
        let (mut ok, mut idx) = (false, 0i64);
        if let Some(lt) = layout_table(&f.face, which) {
            match kind {
                1 => {
                    if let Some(i) = lt.scripts.index(tag) {
                        ok = true;
                        idx = i64::from(i);
                    } else {
                        idx = 0xFFFF;
                    }
                }
                2 => {
                    idx = DEFAULT_LANGUAGE_INDEX;
                    if let Some(s) = lt.scripts.get(si as u16) {
                        if let Some(i) = s.languages.index(tag) {
                            ok = true;
                            idx = i64::from(i);
                        }
                    }
                }
                _ => {
                    idx = 0xFFFF;
                    if let Some(s) = lt.scripts.get(si as u16) {
                        let ls = if li == DEFAULT_LANGUAGE_INDEX {
                            s.default_language
                        } else {
                            s.languages.get(li as u16)
                        };
                        if let Some(ls) = ls {
                            for fi in ls.required_feature.into_iter().chain(ls.feature_indices) {
                                if lt.features.get(fi).is_some_and(|ft| ft.tag == tag) {
                                    ok = true;
                                    idx = i64::from(fi);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        let a = cx.push(ok)?;
        let b = cx.push(idx)?;
        Ok(a + b)
    });

    // shape(face, cps, clusters, direction, script_tag, language, features, xscale, yscale)
    native!("shape", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let cps: Vec<i64> = cx.arg::<LuaTable>(2)?.sequence_values()?;
        let clusters: Vec<i64> = cx.arg::<LuaTable>(3)?.sequence_values()?;
        let dir: Option<String> = cx.arg::<Option<LuaString>>(4)?.map(|s| s.to_string_lossy());
        let script: Option<i64> = cx.arg(5)?;
        let lang: Option<String> = cx.arg::<Option<LuaString>>(6)?.map(|s| s.to_string_lossy());
        let ft: LuaTable = cx.arg(7)?;
        let xs: f64 = cx.arg(8)?;
        let ys: f64 = cx.arg(9)?;
        let mut features = Vec::new();
        for i in 1..=ft.raw_len()? {
            let t: LuaTable = ft.raw_geti(i as i64)?;
            let tag: i64 = t.get("tag")?;
            let value: i64 = t.get::<Option<i64>>("value")?.unwrap_or(1);
            let start: Option<i64> = t.get("start")?;
            let end: Option<i64> = t.get("_end")?;
            let lo = start.map_or(0, |v| v.max(0) as usize);
            let hi = end.map_or(usize::MAX, |v| v.max(0) as usize);
            features.push(rustybuzz::Feature::new(tag_of(tag), value as u32, lo..hi));
        }
        let mut buf = rustybuzz::UnicodeBuffer::new();
        for (i, cp) in cps.iter().enumerate() {
            let c = char::from_u32(*cp as u32).unwrap_or('\u{FFFD}');
            buf.add(c, clusters.get(i).copied().unwrap_or(i as i64) as u32);
        }
        match dir.as_deref() {
            Some("ltr") => buf.set_direction(rustybuzz::Direction::LeftToRight),
            Some("rtl") => buf.set_direction(rustybuzz::Direction::RightToLeft),
            Some("ttb") => buf.set_direction(rustybuzz::Direction::TopToBottom),
            Some("btt") => buf.set_direction(rustybuzz::Direction::BottomToTop),
            _ => {}
        }
        if let Some(s) = script {
            if let Some(s) = rustybuzz::Script::from_iso15924_tag(tag_of(s)) {
                buf.set_script(s);
            }
        }
        if let Some(l) = lang.filter(|l| !l.is_empty()) {
            if let Ok(l) = l.parse::<rustybuzz::Language>() {
                buf.set_language(l);
            }
        }
        buf.guess_segment_properties();
        let out = rustybuzz::shape(&f.face, &features, buf);
        let upem = f64::from(f.face.units_per_em().max(1));
        let sc = |v: i32, s: f64| (f64::from(v) * s / upem).round() as i64;
        let infos = out.glyph_infos();
        let pos = out.glyph_positions();
        let t = cx.create_table_with_capacity(infos.len(), 0)?;
        for (i, (g, p)) in infos.iter().zip(pos).enumerate() {
            let e = cx.create_table_with_capacity(0, 7)?;
            e.set("codepoint", i64::from(g.glyph_id))?;
            e.set("cluster", i64::from(g.cluster))?;
            e.set("x_advance", sc(p.x_advance, xs) as f64)?;
            e.set("y_advance", sc(p.y_advance, ys) as f64)?;
            e.set("x_offset", sc(p.x_offset, xs) as f64)?;
            e.set("y_offset", sc(p.y_offset, ys) as f64)?;
            e.set("flags", i64::from(g.unsafe_to_break()))?;
            t.raw_seti(i as i64 + 1, e)?;
        }
        cx.push(t)
    });

    native!("meta", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let t = cx.create_table_with_capacity(0, 6)?;
        t.set("italic_angle", f64::from(f.face.italic_angle().unwrap_or(0.0)))?;
        t.set("weight", i64::from(f.face.weight().to_number()))?;
        t.set("width", i64::from(f.face.width().to_number()))?;
        t.set("ascent", i64::from(f.face.ascender()))?;
        t.set("descent", -i64::from(f.face.descender()))?;
        t.set("weight_name", cx.create_bytes(weight_name(f.face.weight().to_number()).as_bytes())?)?;
        cx.push(t)
    });

    native!("cmap_pairs", |cx| {
        let f = face(cx.arg(1)?).map_err(|m| cx.error(m))?;
        let t = cx.create_table()?;
        if let Some(cmap) = f.face.tables().cmap {
            for st in cmap.subtables {
                if st.is_unicode() {
                    st.codepoints(|c| {
                        if let Some(g) = st.glyph_index(c) {
                            if t.raw_get::<Option<i64>>(i64::from(g.0)).ok().flatten().is_none() {
                                let _ = t.raw_set(i64::from(g.0), i64::from(c));
                            }
                        }
                    });
                }
            }
        }
        cx.push(t)
    });

    lua.set_global("__ratex_hb", n).map_err(|e| format!("{e:?}"))?;
    lua.execute(HB_LUA).map_err(|e| format!("luaharfbuzz: {e:?}"))?;
    Ok(())
}

const HB_LUA: &str = r##"
local N = __ratex_hb
__ratex_hb = nil

local hb = {}
local function class(name)
  local c = {}
  c.__index = c
  c.__name = "harfbuzz." .. name
  return c
end
local function tagnum(s)
  s = tostring(s)
  s = (s .. "    "):sub(1, 4)
  return string.unpack(">I4", s)
end

-- Blob -------------------------------------------------------------------
local Blob = class("Blob")
Blob.new = function(data) return setmetatable({ data = data }, Blob) end
Blob.new_from_file = function(name)
  local f = io.open(name, "rb")
  if not f then return nil end
  local d = f:read("a")
  f:close()
  return Blob.new(d)
end
function Blob:get_length() return #self.data end
function Blob:get_data() return self.data end
hb.Blob = Blob

-- Tag --------------------------------------------------------------------
local Tag = class("Tag")
Tag.__eq = function(a, b) return a.n == b.n end
Tag.__tostring = function(t) return (string.pack(">I4", t.n)) end
Tag.new = function(s)
  if s == nil then return setmetatable({ n = 0 }, Tag) end
  return setmetatable({ n = tagnum(s) }, Tag)
end
hb.Tag = Tag

-- Script -----------------------------------------------------------------
local Script = class("Script")
Script.__eq = function(a, b) return a.n == b.n end
Script.__tostring = function(s) return (string.pack(">I4", s.n)) end
Script.new = function(s)
  s = tostring(s)
  s = s:sub(1, 1):upper() .. s:sub(2):lower()
  return setmetatable({ n = tagnum(s) }, Script)
end
Script.from_iso15924_tag = function(t) return setmetatable({ n = t.n }, Script) end
function Script:to_iso15924_tag() return setmetatable({ n = self.n }, Tag) end
hb.Script = Script

-- Direction --------------------------------------------------------------
local Direction = class("Direction")
Direction.__eq = function(a, b) return a.s == b.s end
Direction.__tostring = function(d) return d.s end
Direction.new = function(s)
  s = tostring(s):lower():sub(1, 1)
  local map = { l = "ltr", r = "rtl", t = "ttb", b = "btt" }
  return setmetatable({ s = map[s] or "invalid" }, Direction)
end
function Direction:is_valid() return self.s ~= "invalid" end
function Direction:is_horizontal() return self.s == "ltr" or self.s == "rtl" end
function Direction:is_vertical() return self.s == "ttb" or self.s == "btt" end
function Direction:is_forward() return self.s == "ltr" or self.s == "ttb" end
function Direction:is_backward() return self.s == "rtl" or self.s == "btt" end
hb.Direction = Direction

-- Language ---------------------------------------------------------------
local Language = class("Language")
Language.__eq = function(a, b) return a.s == b.s end
Language.__tostring = function(l) return l.s end
Language.new = function(s)
  if s == nil then return setmetatable({ s = "" }, Language) end
  return setmetatable({ s = tostring(s):lower():gsub("_", "-") }, Language)
end
hb.Language = Language

-- Feature ----------------------------------------------------------------
local Feature = class("Feature")
Feature.__tostring = function(f)
  local s = (f.value == 0 and "-" or "") .. tostring(f.tag)
  if f.start or f._end then
    s = s .. "[" .. (f.start or "") .. ":" .. (f._end or "") .. "]"
  end
  if f.value ~= 0 and f.value ~= 1 then s = s .. "=" .. f.value end
  return s
end
Feature.new = function(str)
  local s = tostring(str):gsub("^%s+", ""):gsub("%s+$", "")
  local value = 1
  local sign = s:sub(1, 1)
  if sign == "-" then value = 0; s = s:sub(2) elseif sign == "+" then s = s:sub(2) end
  local tag, rest = s:match("^([^%s=%[%]\"']+)(.*)$")
  if not tag or #tag > 4 or #tag == 0 then return nil end
  local from, to
  local a, b, r2 = rest:match("^%[(%d*):?(%d*)%](.*)$")
  if a then
    rest = r2
    from = a ~= "" and tonumber(a) or nil
    if b ~= "" then to = tonumber(b) elseif rest:find("^%[%d+%]") == nil and str:find(":") then to = nil end
    if str:match("%[(%d+)%]") then to = from + 1 end
  end
  local v = rest:match("^=(%S+)$")
  if v then
    if v == "on" or v == "true" then value = 1
    elseif v == "off" or v == "false" then value = 0
    else value = tonumber(v) end
    if value == nil then return nil end
  elseif rest ~= "" then
    return nil
  end
  return setmetatable({ tag = Tag.new(tag), value = value, start = from, _end = to }, Feature)
end
hb.Feature = Feature

-- Variation (struct only; normalized variation coords are not supported) ---
local Variation = class("Variation")
Variation.new = function(s)
  local tag, v = tostring(s):match("^(%w+)=([%d%.%-]+)$")
  if not tag then return nil end
  return setmetatable({ tag = Tag.new(tag), value = tonumber(v) }, Variation)
end
Variation.__tostring = function(v) return tostring(v.tag) .. "=" .. v.value end
hb.Variation = Variation

hb.Set = class("Set")
hb.Set.add = function() end

-- Face -------------------------------------------------------------------
local Face = class("Face")
Face.new_from_blob = function(blob, index)
  local h = N.face_new(blob:get_data(), index or 0)
  if not h then return nil end
  local upem, n = N.face_info(h)
  return setmetatable({ h = h, upem = upem, nglyphs = n, blob = blob }, Face)
end
Face.new = function(name, index)
  local b = Blob.new_from_file(name)
  if not b then return nil end
  return Face.new_from_blob(b, index)
end
function Face:blob() return self.blob end
function Face:get_upem() return self.upem end
function Face:get_glyph_count() return self.nglyphs end
function Face:get_name(id) return N.face_name(self.h, id) end
function Face:get_table(tag)
  local d = N.face_table(self.h, tag.n)
  return d and Blob.new(d) or nil
end
local function totags(list)
  if not list then return nil end
  for i = 1, #list do list[i] = setmetatable({ n = list[i] }, Tag) end
  return list
end
function Face:get_table_tags() return totags(N.face_tables(self.h)) end
function Face:collect_unicodes() return N.unicodes(self.h) end
function Face:ot_layout_get_script_tags(t) return totags(N.layout_tags(self.h, t.n, 1)) end
function Face:ot_layout_get_language_tags(t, si) return totags(N.layout_tags(self.h, t.n, 2, si)) end
function Face:ot_layout_get_feature_tags(t, si, li) return totags(N.layout_tags(self.h, t.n, 3, si, li)) end
function Face:ot_layout_find_script(t, s) return N.layout_find(self.h, t.n, 1, 0, 0, s.n) end
function Face:ot_layout_find_language(t, si, l) return N.layout_find(self.h, t.n, 2, si, 0, l.n) end
function Face:ot_layout_find_feature(t, si, li, f) return N.layout_find(self.h, t.n, 3, si, li, f.n) end
-- Colour tables and variable-font instancing are not provided.
function Face:ot_color_has_palettes() return false end
function Face:ot_color_has_layers() return false end
function Face:ot_color_has_png() return false end
function Face:ot_color_has_svg() return false end
function Face:ot_color_palette_get_count() return 0 end
function Face:ot_var_has_data() return false end
function Face:ot_var_get_axis_infos() return nil end
function Face:ot_var_named_instance_get_infos() return nil end
hb.Face = Face

-- Font -------------------------------------------------------------------
local Font = class("Font")
Font.new = function(face)
  local s = face:get_upem()
  return setmetatable({ face = face, xs = s, ys = s }, Font)
end
function Font:set_scale(x, y) self.xs, self.ys = x, y or x end
function Font:get_scale() return self.xs, self.ys end
local function sc(v, s, upem) return math.floor(v * s / upem + 0.5) end
local function scf(v, s, upem) return math.floor(v * s / upem + 0.5) + 0.0 end
function Font:get_nominal_glyph(cp) return N.nominal(self.face.h, cp) end
function Font:get_glyph_h_advance(g)
  return sc(N.advance(self.face.h, g), self.xs, self.face.upem)
end
function Font:get_glyph_v_advance(g)
  return sc(N.advance(self.face.h, g, true), self.ys, self.face.upem)
end
function Font:get_glyph_extents(g)
  local e = N.glyph_extents(self.face.h, g)
  if not e then return nil end
  local u = self.face.upem
  return {
    x_bearing = scf(e.x_bearing, self.xs, u), y_bearing = scf(e.y_bearing, self.ys, u),
    width = scf(e.width, self.xs, u), height = scf(e.height, self.ys, u),
  }
end
function Font:get_h_extents()
  local e = N.h_extents(self.face.h)
  local u = self.face.upem
  return {
    ascender = scf(e.ascender, self.ys, u), descender = scf(e.descender, self.ys, u),
    line_gap = scf(e.line_gap, self.ys, u),
  }
end
function Font:get_glyph_name(g) return N.glyph_name(self.face.h, g) end
function Font:get_glyph_from_name(n) return N.glyph_from_name(self.face.h, n) end
function Font:set_var_coords_normalized(coords) self.coords = coords end
function Font:get_var_coords_normalized() return self.coords end
function Font:ot_color_glyph_get_png() return nil end
hb.Font = Font

-- Buffer -----------------------------------------------------------------
local Buffer = class("Buffer")
Buffer.CLUSTER_LEVEL_DEFAULT = 0
Buffer.CLUSTER_LEVEL_MONOTONE_GRAPHEMES = 0
Buffer.CLUSTER_LEVEL_MONOTONE_CHARACTERS = 1
Buffer.CLUSTER_LEVEL_CHARACTERS = 2
Buffer.FLAG_DEFAULT = 0
Buffer.FLAG_BOT = 1
Buffer.FLAG_EOT = 2
Buffer.FLAG_PRESERVE_DEFAULT_IGNORABLES = 4
Buffer.FLAG_REMOVE_DEFAULT_IGNORABLES = 8
Buffer.FLAG_DO_NOT_INSERT_DOTTED_CIRCLE = 16
Buffer.GLYPH_FLAG_UNSAFE_TO_BREAK = 1
Buffer.GLYPH_FLAG_DEFINED = 1
Buffer.new = function()
  return setmetatable({ cps = {}, clusters = {}, flags = 0, level = 0 }, Buffer)
end
function Buffer:add_codepoints(t, offset, length)
  offset = offset or 0
  length = (length == nil or length < 0) and (#t - offset) or length
  for i = offset + 1, offset + length do
    self.cps[#self.cps + 1] = t[i]
    self.clusters[#self.clusters + 1] = i - 1
  end
end
function Buffer:add(cp, cluster)
  self.cps[#self.cps + 1] = cp
  self.clusters[#self.clusters + 1] = cluster or #self.cps - 1
end
function Buffer:add_utf8(s, offset, length)
  offset = offset or 0
  local stop = (length == nil or length < 0) and #s or offset + length
  local i = offset + 1
  while i <= stop do
    local cp = utf8.codepoint(s, i)
    self.cps[#self.cps + 1] = cp
    self.clusters[#self.clusters + 1] = i - 1
    i = utf8.offset(s, 2, i)
  end
end
function Buffer:clear_contents() self.cps, self.clusters, self.glyphs = {}, {}, nil end
function Buffer:pre_allocate() return true end
function Buffer:get_length() return #(self.glyphs or self.cps) end
function Buffer:set_direction(d) self.direction = d end
function Buffer:get_direction() return self.direction or Direction.new("invalid") end
function Buffer:set_script(s) self.script = s end
function Buffer:get_script() return self.script or Script.new("Zzzz") end
function Buffer:set_language(l) self.language = l end
function Buffer:get_language() return self.language or Language.new() end
function Buffer:set_flags(f) self.flags = f end
function Buffer:get_flags() return self.flags end
function Buffer:set_cluster_level(l) self.level = l end
function Buffer:get_cluster_level() return self.level end
function Buffer:set_replacement_codepoint(c) self.replacement = c end
function Buffer:get_replacement_codepoint() return self.replacement or 0xFFFD end
function Buffer:set_invisible_glyph(g) self.invisible = g end
function Buffer:get_invisible_glyph() return self.invisible or 0 end
function Buffer:guess_segment_properties() end
function Buffer:reverse()
  local g = self.glyphs
  if g then
    local n = #g
    for i = 1, n // 2 do g[i], g[n + 1 - i] = g[n + 1 - i], g[i] end
  else
    local c, k = self.cps, self.clusters
    local n = #c
    for i = 1, n // 2 do
      c[i], c[n + 1 - i] = c[n + 1 - i], c[i]
      k[i], k[n + 1 - i] = k[n + 1 - i], k[i]
    end
  end
end
function Buffer:get_glyphs() return self.glyphs end
hb.Buffer = Buffer

-- shape ------------------------------------------------------------------
hb.shape_full = function(font, buf, features, shapers)
  local fl = {}
  for i, f in ipairs(features) do
    fl[i] = { tag = f.tag.n, value = f.value, start = f.start, _end = f._end }
  end
  buf.glyphs = N.shape(
    font.face.h, buf.cps, buf.clusters,
    buf.direction and buf.direction.s or nil,
    buf.script and buf.script.n or nil,
    buf.language and buf.language.s or nil,
    fl, font.xs, font.ys)
  if buf.direction == nil or buf.direction.s == "invalid" then
    buf.direction = Direction.new("ltr")
  end
  return true
end
hb.version = function() return "14.5.0" end
hb.shapers = function() return "ot", "fallback" end

-- Script of a code point for the principal scripts (ranges of UAX #24).
local script_ranges = {
  { 0x41, 0x5A, "Latn" }, { 0x61, 0x7A, "Latn" }, { 0xC0, 0x24F, "Latn" }, { 0x1E00, 0x1EFF, "Latn" },
  { 0x370, 0x3FF, "Grek" }, { 0x1F00, 0x1FFF, "Grek" }, { 0x400, 0x52F, "Cyrl" },
  { 0x590, 0x5FF, "Hebr" }, { 0x600, 0x6FF, "Arab" }, { 0x750, 0x77F, "Arab" },
  { 0x900, 0x97F, "Deva" }, { 0xE00, 0xE7F, "Thai" }, { 0x3040, 0x309F, "Hira" },
  { 0x30A0, 0x30FF, "Kana" }, { 0x4E00, 0x9FFF, "Hani" }, { 0x3400, 0x4DBF, "Hani" },
  { 0x20000, 0x2A6DF, "Hani" }, { 0xAC00, 0xD7AF, "Hang" }, { 0x1100, 0x11FF, "Hang" },
  { 0x300, 0x36F, "Zinh" },
}
hb.unicode = {
  script = function(cp)
    for _, r in ipairs(script_ranges) do
      if cp >= r[1] and cp <= r[2] then return Script.new(r[3]) end
    end
    return Script.COMMON
  end,
}

local ot = {
  NAME_ID_COPYRIGHT = 0, NAME_ID_FONT_FAMILY = 1, NAME_ID_FONT_SUBFAMILY = 2,
  NAME_ID_UNIQUE_ID = 3, NAME_ID_FULL_NAME = 4, NAME_ID_VERSION_STRING = 5,
  NAME_ID_POSTSCRIPT_NAME = 6, NAME_ID_TRADEMARK = 7, NAME_ID_MANUFACTURER = 8,
  NAME_ID_DESIGNER = 9, NAME_ID_DESCRIPTION = 10, NAME_ID_VENDOR_URL = 11,
  NAME_ID_DESIGNER_URL = 12, NAME_ID_LICENSE = 13, NAME_ID_LICENSE_URL = 14,
  NAME_ID_TYPOGRAPHIC_FAMILY = 16, NAME_ID_TYPOGRAPHIC_SUBFAMILY = 17,
  NAME_ID_MAC_FULL_NAME = 18, NAME_ID_SAMPLE_TEXT = 19, NAME_ID_CID_FINDFONT_NAME = 20,
  NAME_ID_WWS_FAMILY = 21, NAME_ID_WWS_SUBFAMILY = 22, NAME_ID_LIGHT_BACKGROUND = 23,
  NAME_ID_DARK_BACKGROUND = 24, NAME_ID_VARIATIONS_PS_PREFIX = 25, NAME_ID_INVALID = 0xFFFF,
  LAYOUT_NO_SCRIPT_INDEX = 0xFFFF, LAYOUT_NO_FEATURE_INDEX = 0xFFFF,
  LAYOUT_DEFAULT_LANGUAGE_INDEX = 0xFFFF, LAYOUT_NO_VARIATIONS_INDEX = 0xFFFFFFFF,
}
hb.ot = ot

-- fontloader (luatex's fontforge-based loader) over the same parsed faces ----
local fontloader = {}
local FIELDS = {
  "table_version", "fontname", "fullname", "familyname", "weight", "copyright", "version",
  "italicangle", "units_per_em", "ascent", "descent", "glyphcnt", "glyphmax", "glyphmin",
  "glyphs", "filename", "design_size", "pfminfo",
}
local function ttc_count(data)
  if data:sub(1, 4) == "ttcf" then return (string.unpack(">I4", data, 9)) end
  return 1
end
local function load_face(filename, index)
  local f = io.open(filename, "rb")
  if not f then error("font loading failed for " .. tostring(filename)) end
  local data = f:read("a")
  f:close()
  local h = N.face_new(data, index or 0)
  if not h then error("font loading failed for " .. tostring(filename)) end
  return h, data
end
local function face_meta(h, filename)
  local upem, n = N.face_info(h)
  local m = N.meta(h)
  return {
    fontname = N.face_name(h, 6) or "", fullname = N.face_name(h, 4) or "",
    familyname = N.face_name(h, 1) or "", copyright = N.face_name(h, 0) or "",
    version = N.face_name(h, 5) or "", weight = m.weight_name,
    italicangle = m.italic_angle, units_per_em = upem,
    ascent = m.ascent, descent = m.descent, glyphcnt = n, glyphmax = n - 1, glyphmin = 0,
    filename = filename, table_version = 20190101,
    pfminfo = { weight = m.weight, width = m.width, panose_set = false },
  }
end
fontloader.fields = function() return FIELDS end
fontloader.info = function(filename)
  if type(filename) == "string" then
    local meta = N.embedded_info(filename)
    if meta then return meta end
  end
  local h, data = load_face(filename, 0)
  local count = ttc_count(data)
  if count > 1 then
    local out = {}
    for i = 0, count - 1 do
      local hi = i == 0 and h or N.face_new(data, i)
      out[#out + 1] = face_meta(hi, filename)
      N.face_close(hi)
    end
    return out
  end
  local meta = face_meta(h, filename)
  N.face_close(h)
  return meta
end
local Loaded = {}
fontloader.open = function(filename, fontname)
  local h, data = load_face(filename, 0)
  local count = ttc_count(data)
  if fontname and count > 1 then
    local found
    for i = 0, count - 1 do
      local hi = i == 0 and h or N.face_new(data, i)
      if N.face_name(hi, 6) == fontname then found = hi break end
      N.face_close(hi)
    end
    if not found then error("font loading failed for " .. filename .. "(" .. fontname .. ")") end
    h = found
  end
  return setmetatable({ h = h, filename = filename }, Loaded)
end
fontloader.close = function(font)
  if font.h then N.face_close(font.h) font.h = nil end
end
fontloader.to_table = function(font)
  if not font.h then return false end
  local t = face_meta(font.h, font.filename)
  local names, widths = {}, {}
  local rev = N.cmap_pairs(font.h)
  local glyphs = {}
  for g = 0, t.glyphcnt - 1 do
    local e = N.glyph_extents(font.h, g)
    glyphs[g] = {
      name = N.glyph_name(font.h, g),
      width = N.advance(font.h, g),
      unicode = rev[g] or -1,
      boundingbox = e and { e.x_bearing, e.y_bearing + e.height, e.x_bearing + e.width, e.y_bearing } or nil,
    }
  end
  t.glyphs = glyphs
  return t
end
local function unsupported(name)
  return function() error("fontloader." .. name .. " is not supported (no FontForge engine)") end
end
fontloader.apply_featurefile = unsupported("apply_featurefile")
fontloader.apply_afmfile = unsupported("apply_afmfile")
fontloader.glyphs = nil
_G.fontloader = fontloader
package.loaded["fontloader"] = fontloader

package.loaded["luaharfbuzz"] = hb
"##;
