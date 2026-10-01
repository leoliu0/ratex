//! The Lua side of Lua fonts (luatex `luafont.c`): converting a font table to
//! the engine's [`LuaFont`] (`font.define`, `font.setfont`,
//! `font.addcharacters`) and a font back to a Lua table (`font.getfont`,
//! `font.getcopy`, `font.each`, `font.read_tfm`).

use tex_lua::{CallbackLua, LuaBytes, LuaTable, Value};

use crate::engine::Engine;
use crate::lua_font::*;
use crate::tfm::{Font, FontId};

/// luatex `MATH_param_names` (index 0 is `nil`; 1.. are the OpenType MATH
/// constants).
pub(crate) const MATH_PARAM_NAMES: &[&str] = &[
    "nil",
    "ScriptPercentScaleDown",
    "ScriptScriptPercentScaleDown",
    "DelimitedSubFormulaMinHeight",
    "DisplayOperatorMinHeight",
    "MathLeading",
    "AxisHeight",
    "AccentBaseHeight",
    "FlattenedAccentBaseHeight",
    "SubscriptShiftDown",
    "SubscriptTopMax",
    "SubscriptBaselineDropMin",
    "SuperscriptShiftUp",
    "SuperscriptShiftUpCramped",
    "SuperscriptBottomMin",
    "SuperscriptBaselineDropMax",
    "SubSuperscriptGapMin",
    "SuperscriptBottomMaxWithSubscript",
    "SpaceAfterScript",
    "UpperLimitGapMin",
    "UpperLimitBaselineRiseMin",
    "LowerLimitGapMin",
    "LowerLimitBaselineDropMin",
    "StackTopShiftUp",
    "StackTopDisplayStyleShiftUp",
    "StackBottomShiftDown",
    "StackBottomDisplayStyleShiftDown",
    "StackGapMin",
    "StackDisplayStyleGapMin",
    "StretchStackTopShiftUp",
    "StretchStackBottomShiftDown",
    "StretchStackGapAboveMin",
    "StretchStackGapBelowMin",
    "FractionNumeratorShiftUp",
    "FractionNumeratorDisplayStyleShiftUp",
    "FractionDenominatorShiftDown",
    "FractionDenominatorDisplayStyleShiftDown",
    "FractionNumeratorGapMin",
    "FractionNumeratorDisplayStyleGapMin",
    "FractionRuleThickness",
    "FractionDenominatorGapMin",
    "FractionDenominatorDisplayStyleGapMin",
    "SkewedFractionHorizontalGap",
    "SkewedFractionVerticalGap",
    "OverbarVerticalGap",
    "OverbarRuleThickness",
    "OverbarExtraAscender",
    "UnderbarVerticalGap",
    "UnderbarRuleThickness",
    "UnderbarExtraDescender",
    "RadicalVerticalGap",
    "RadicalDisplayStyleVerticalGap",
    "RadicalRuleThickness",
    "RadicalExtraAscender",
    "RadicalKernBeforeDegree",
    "RadicalKernAfterDegree",
    "RadicalDegreeBottomRaisePercent",
    "MinConnectorOverlap",
    "SubscriptShiftDownWithSuperscript",
    "FractionDelimiterSize",
    "FractionDelimiterDisplayStyleSize",
    "NoLimitSubFactor",
    "NoLimitSupFactor",
];

/// `MATH_param_max`.
pub(crate) const MATH_PARAM_MAX: usize = MATH_PARAM_NAMES.len() - 1;

/// luatex `ligature_type_strings` (`""` entries are unused slots).
const LIGATURE_TYPE_STRINGS: &[&str] = &[
    "=:", "=:|", "|=:", "|=:|", "", "=:|>", "|=:>", "|=:|>", "", "", "", "|=:|>>",
];
const FONT_FORMAT_STRINGS: &[&str] = &["unknown", "type1", "type3", "truetype", "opentype"];
const FONT_EMBEDDING_STRINGS: &[&str] = &["unknown", "no", "subset", "full"];
const FONT_TYPE_STRINGS: &[&str] = &["unknown", "virtual", "real"];
const FONT_MODE_STRINGS: &[&str] = &["unknown", "horizontal", "vertical"];

// ------------------------------------------------------------------ reading

/// A raw field of a font table (luatex reads font tables with `rawget`).
fn field(t: &LuaTable, key: &str) -> Option<Value> {
    t.raw_get::<Option<Value>>(key).ok().flatten()
}

/// `lua_isnumber`: a number or a string that converts to one.
fn value_number(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_number() {
        return Some(n);
    }
    let s = v.as_string_handle()?;
    let bytes = s.to_bytes();
    std::str::from_utf8(&bytes).ok()?.trim().parse::<f64>().ok()
}

/// luatex `lua_roundnumber`.
fn round(x: f64) -> i32 {
    let r = (x + 0.5).floor();
    if r >= f64::from(i32::MAX) {
        i32::MAX
    } else if r <= f64::from(i32::MIN) {
        i32::MIN
    } else {
        r as i32
    }
}

/// `lua_numeric_field_by_index`.
fn num_field(t: &LuaTable, key: &str, default: i32) -> i32 {
    field(t, key).and_then(|v| value_number(&v)).map_or(default, round)
}

fn bool_field(t: &LuaTable, key: &str, default: bool) -> bool {
    field(t, key).and_then(|v| v.as_boolean()).unwrap_or(default)
}

/// `lua_type == LUA_TSTRING` fields.
fn str_field(t: &LuaTable, key: &str) -> Option<Vec<u8>> {
    field(t, key)?.as_string_handle().map(|s| s.to_bytes())
}

/// `n_enum_field`.
fn enum_field(t: &LuaTable, key: &str, default: usize, names: &[&str]) -> usize {
    let Some(v) = field(t, key) else {
        return default;
    };
    if let Some(n) = v.as_number() {
        return round(n).max(0) as usize;
    }
    if let Some(s) = v.as_string_handle() {
        let s = s.to_bytes();
        if let Some(i) = names.iter().position(|n| n.as_bytes() == s.as_slice()) {
            return i;
        }
    }
    default
}

fn hex4(out: &mut Vec<u8>, u: i64) {
    out.extend_from_slice(format!("{u:04X}").as_bytes());
}

/// luatex's UTF-16 hex for one code point of a `tounicode` table/number.
fn tounicode_hex(out: &mut Vec<u8>, u: i64) {
    if u < 0xD7FF || (u > 0xDFFF && u <= 0xFFFF) {
        hex4(out, u);
    } else {
        let v = u - 0x10000;
        hex4(out, v / 1024 + 0xD800);
        hex4(out, v % 1024 + 0xDC00);
    }
}

/// `characters[c].tounicode`.
fn read_tounicode(t: &LuaTable) -> Option<Vec<u8>> {
    let v = field(t, "tounicode")?;
    if let Some(n) = v.as_number() {
        let u = i64::from(round(n));
        if u < 0 {
            return None;
        }
        let mut out = Vec::new();
        tounicode_hex(&mut out, u);
        return Some(out);
    }
    if let Some(tab) = v.as_table() {
        let items: Vec<Value> = tab.sequence_values().unwrap_or_default();
        let mut out = Vec::new();
        for item in &items {
            let Some(n) = item.as_number() else { break };
            let u = i64::from(round(n));
            if u < 0 {
                return None;
            }
            tounicode_hex(&mut out, u);
        }
        return (!out.is_empty()).then_some(out);
    }
    v.as_string_handle().map(|s| s.to_bytes())
}

/// What `font_char_from_lua` needs to know about the font being read.
struct ParseCtx<'a> {
    font: FontId,
    name: &'a str,
    has_math: bool,
    /// luatex `l_fonts`: 1-based local fonts (index 0 unused).
    local_fonts: &'a [FontId],
    atsize: i32,
    warnings: &'a mut Vec<String>,
}

/// luatex `sp_to_dvi`.
fn sp_to_dvi(sp: i32, atsize: i32) -> i32 {
    let mult = f64::from(atsize) / 65536.0;
    (f64::from(sp) * 16.0 / mult).floor() as i32
}

fn store_math_kerns(t: &LuaTable, key: &str) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    if let Some(tab) = field(t, key).and_then(|v| v.as_table()) {
        let n = tab.raw_len().unwrap_or(0);
        for i in 1..=n {
            let Some(entry) = tab.raw_geti::<Option<Value>>(i as i64).ok().flatten().and_then(|v| v.as_table())
            else {
                continue;
            };
            let h = field(&entry, "height").and_then(|v| value_number(&v)).map(round);
            let k = field(&entry, "kern").and_then(|v| value_number(&v)).map(round);
            if let (Some(h), Some(k)) = (h, k) {
                out.push((h, k));
            }
        }
    }
    out
}

fn read_variants(t: &LuaTable, key: &str) -> Option<Vec<MathVariant>> {
    let tab = field(t, key)?.as_table()?;
    let mut out = Vec::new();
    let mut k = 1;
    while let Some(entry) = tab.raw_geti::<Option<Value>>(k).ok().flatten().and_then(|v| v.as_table()) {
        out.push(MathVariant {
            glyph: num_field(&entry, "glyph", 0),
            extender: num_field(&entry, "extender", 0),
            start: num_field(&entry, "start", 0),
            end: num_field(&entry, "end", 0),
            advance: num_field(&entry, "advance", 0),
        });
        k += 1;
    }
    Some(out)
}

/// Character key of a `kerns` / `ligatures` table: a code point or
/// `"right_boundary"`.
fn lig_kern_key(key: &Value) -> Option<i32> {
    if let Some(n) = key.as_number() {
        let k = n as i64;
        return (k >= 0).then(|| k as i32);
    }
    let s = key.as_string_handle()?.to_bytes();
    (s == b"right_boundary").then_some(RIGHT_BOUNDARY)
}

/// luatex `read_char_packets`: the `commands` table.
fn read_commands(t: &LuaTable, ctx: &mut ParseCtx<'_>) -> Result<Option<Vec<VfCommand>>, String> {
    let Some(tab) = field(t, "commands").and_then(|v| v.as_table()) else {
        return Ok(None);
    };
    if tab.pairs::<Value, Value>().map_err(|e| format!("{e:?}"))?.is_empty() {
        return Ok(None);
    }
    let l_fonts = ctx.local_fonts;
    let max_f = l_fonts.len().saturating_sub(1);
    let first = l_fonts.get(1).copied().unwrap_or(0);
    let local = |n: i32| -> FontId {
        if n == 0 {
            0
        } else if n as usize > max_f || n < 0 {
            first
        } else {
            l_fonts[n as usize]
        }
    };
    let mut out = Vec::new();
    let mut have_font = false;
    let n = tab.raw_len().map_err(|e| format!("{e:?}"))?;
    for i in 1..=n {
        let Some(cmd) = tab.raw_geti::<Option<Value>>(i as i64).ok().flatten().and_then(|v| v.as_table()) else {
            return Err("vf command: commands has to be a table".to_string());
        };
        let Some(name) = cmd.raw_geti::<Option<Value>>(1).ok().flatten().and_then(|v| v.as_string_handle()) else {
            continue;
        };
        let name = name.to_bytes();
        let arg = |k: i64| cmd.raw_geti::<Option<Value>>(k).ok().flatten();
        let arg_num = |k: i64| arg(k).and_then(|v| value_number(&v)).map_or(0, round);
        match name.as_slice() {
            b"font" => {
                if l_fonts.is_empty() {
                    return Err("vf command: no font table found".to_string());
                }
                have_font = true;
                out.push(VfCommand::Font(local(arg_num(2))));
            }
            b"char" => {
                if l_fonts.is_empty() {
                    return Err("vf command: no font table found".to_string());
                }
                if !have_font {
                    out.push(VfCommand::Font(first));
                    have_font = true;
                }
                out.push(VfCommand::Char(arg_num(2) as u32));
            }
            b"slot" => {
                if l_fonts.is_empty() {
                    return Err("vf command: no font table found".to_string());
                }
                let n = arg_num(2);
                let sf = if n == 0 { ctx.font } else { local(n) };
                out.push(VfCommand::Font(sf));
                have_font = true;
                out.push(VfCommand::Char(arg_num(3) as u32));
            }
            b"comment" | b"nop" => out.push(VfCommand::Nop),
            b"push" => out.push(VfCommand::Push),
            b"pop" => out.push(VfCommand::Pop),
            b"rule" => {
                out.push(VfCommand::Rule(sp_to_dvi(arg_num(2), ctx.atsize), sp_to_dvi(arg_num(3), ctx.atsize)))
            }
            b"right" => out.push(VfCommand::Right(sp_to_dvi(arg_num(2), ctx.atsize))),
            b"down" => out.push(VfCommand::Down(sp_to_dvi(arg_num(2), ctx.atsize))),
            b"pdf" => {
                let len = cmd.raw_len().map_err(|e| format!("{e:?}"))?;
                let mut mode = 0u8; // set_origin
                let mut is_mode = false;
                let text;
                if len == 3 {
                    let second = arg(2);
                    let mode_value = match second.as_ref().and_then(|v| v.as_string_handle()) {
                        Some(s) if s.to_bytes() == b"mode" => {
                            is_mode = true;
                            arg(3)
                        }
                        _ => second,
                    };
                    mode = pdf_literal_mode(mode_value.as_ref());
                    text = arg(3);
                } else {
                    text = arg(2);
                }
                if is_mode {
                    out.push(VfCommand::PdfMode(mode));
                } else {
                    let Some(s) = text.and_then(|v| v.as_string_handle()) else {
                        return Err("vf command: invalid packet pdf literal".to_string());
                    };
                    out.push(VfCommand::Pdf { mode, data: s.to_bytes() });
                }
            }
            b"special" => {
                let Some(s) = arg(2).and_then(|v| v.as_string_handle()) else {
                    return Err("vf command: invalid packet special".to_string());
                };
                out.push(VfCommand::Special(s.to_bytes()));
            }
            b"lua" => out.push(VfCommand::Lua(arg(2).ok_or("vf command: invalid packet lua")?)),
            b"node" => out.push(VfCommand::Node(arg(2).ok_or("vf command: invalid packet node")?)),
            b"image" => out.push(VfCommand::Image(arg(2).ok_or("vf command: invalid packet image")?)),
            b"scale" => {
                let f = arg(2).and_then(|v| value_number(&v)).unwrap_or(0.0);
                out.push(VfCommand::Scale(f as f32));
            }
            _ => return Err("vf command: unknown packet command".to_string()),
        }
    }
    Ok(Some(out))
}

/// luatex's `pdf` packet mode: `direct`, `page`, `text`, `font`, `raw`,
/// `origin`, or a number.
fn pdf_literal_mode(v: Option<&Value>) -> u8 {
    let Some(v) = v else { return crate::lua_font::PDF_SET_ORIGIN };
    if let Some(s) = v.as_string_handle() {
        return match s.to_bytes().as_slice() {
            b"direct" => crate::lua_font::PDF_DIRECT_ALWAYS,
            b"page" => crate::lua_font::PDF_DIRECT_PAGE,
            b"text" => crate::lua_font::PDF_DIRECT_TEXT,
            b"font" => crate::lua_font::PDF_DIRECT_FONT,
            b"raw" => crate::lua_font::PDF_DIRECT_RAW,
            _ => crate::lua_font::PDF_SET_ORIGIN,
        };
    }
    let n = round(v.as_number().unwrap_or(0.0));
    if (0..crate::lua_font::PDF_SCAN_SPECIAL as i32).contains(&n) {
        n as u8
    } else {
        crate::lua_font::PDF_SET_ORIGIN
    }
}

/// luatex `set_charinfo_extensible`: an extensible recipe is stored as a list
/// of vertical variants (`extender` 1 marks the repeated pieces).
pub(crate) fn extensible_variants(e: &Extensible) -> Vec<MathVariant> {
    let piece = |glyph: i32, extender: i32| MathVariant { glyph, extender, start: 0, end: 0, advance: 0 };
    let (top, bot, mid, rep) = (e.top, e.bot, e.mid, e.rep);
    let mut out = Vec::new();
    if bot == 0 && top == 0 && mid == 0 && rep != 0 {
        out.push(piece(rep, 0));
        out.push(piece(rep, 1));
        return out;
    }
    if bot != 0 {
        out.push(piece(bot, 0));
    }
    if rep != 0 {
        out.push(piece(rep, 1));
    }
    if mid != 0 {
        out.push(piece(mid, 0));
        if rep != 0 {
            out.push(piece(rep, 1));
        }
    }
    if top != 0 {
        out.push(piece(top, 0));
    }
    out
}

/// luatex `font_char_from_lua`.
fn char_from_lua(t: &LuaTable, code: i32, ctx: &mut ParseCtx<'_>) -> Result<LuaCharInfo, String> {
    let mut co = LuaCharInfo::default();
    co.width = num_field(t, "width", 0);
    co.height = num_field(t, "height", 0);
    co.depth = num_field(t, "depth", 0);
    co.italic = num_field(t, "italic", 0);
    co.vert_italic = num_field(t, "vert_italic", 0);
    co.index = num_field(t, "index", 0) as u32;
    co.expansion_factor = num_field(t, "expansion_factor", 1000);
    co.left_protruding = num_field(t, "left_protruding", 0);
    co.right_protruding = num_field(t, "right_protruding", 0);
    co.used = bool_field(t, "used", false);
    co.name = str_field(t, "name");
    co.tounicode = read_tounicode(t);
    if ctx.has_math {
        co.top_accent = num_field(t, "top_accent", i32::MIN);
        co.bot_accent = num_field(t, "bot_accent", i32::MIN);
        let next = num_field(t, "next", -1);
        if next >= 0 {
            co.next = Some(next as u32);
        }
        if let Some(ext) = field(t, "extensible").and_then(|v| v.as_table()) {
            let e = Extensible {
                top: num_field(&ext, "top", 0),
                bot: num_field(&ext, "bot", 0),
                mid: num_field(&ext, "mid", 0),
                rep: num_field(&ext, "rep", 0),
            };
            if e.top != 0 || e.bot != 0 || e.mid != 0 || e.rep != 0 {
                co.vert_variants = extensible_variants(&e);
                co.extensible = Some(e);
            } else {
                ctx.warnings.push(format!(
                    "lua-loaded font {} char U+{:X} has an invalid extensible field",
                    ctx.name, code
                ));
            }
        }
        if let Some(v) = read_variants(t, "horiz_variants") {
            co.hor_variants = v;
        }
        if let Some(v) = read_variants(t, "vert_variants") {
            co.vert_variants = v;
            co.extensible = None;
        }
        if let Some(mk) = field(t, "mathkern").and_then(|v| v.as_table()) {
            co.math_kerns = MathKerns {
                top_left: store_math_kerns(&mk, "top_left"),
                top_right: store_math_kerns(&mk, "top_right"),
                bottom_right: store_math_kerns(&mk, "bottom_right"),
                bottom_left: store_math_kerns(&mk, "bottom_left"),
            };
        }
    }
    if let Some(kt) = field(t, "kerns").and_then(|v| v.as_table()) {
        let pairs = kt.pairs::<Value, Value>().map_err(|e| format!("{e:?}"))?;
        if !pairs.is_empty() {
            for (k, v) in &pairs {
                match lig_kern_key(k) {
                    Some(key) => {
                        let kern = value_number(v).map_or(0, round);
                        co.kerns.insert(key, kern);
                    }
                    None => ctx.warnings.push(format!(
                        "lua-loaded font {} char U+{:X} has an invalid kern field",
                        ctx.name, code
                    )),
                }
            }
            if co.kerns.is_empty() {
                ctx.warnings.push(format!(
                    "lua-loaded font {} char U+{:X} has an invalid kerns field",
                    ctx.name, code
                ));
            }
        }
    }
    co.commands = read_commands(t, ctx)?;
    if let Some(lt) = field(t, "ligatures").and_then(|v| v.as_table()) {
        let pairs = lt.pairs::<Value, Value>().map_err(|e| format!("{e:?}"))?;
        if !pairs.is_empty() {
            for (k, v) in &pairs {
                let key = lig_kern_key(k);
                let entry = v.as_table();
                let r = entry.as_ref().map_or(-1, |e| num_field(e, "char", -1));
                match (key, entry) {
                    (Some(key), Some(entry)) if r != -1 => {
                        let op = enum_field(&entry, "type", 0, LIGATURE_TYPE_STRINGS) as u8;
                        co.ligatures.insert(key, LuaLig { op, replacement: r as u32 });
                    }
                    _ => ctx.warnings.push(format!(
                        "lua-loaded font {} char U+{:X} has an invalid ligature field",
                        ctx.name, code
                    )),
                }
            }
            if co.ligatures.is_empty() {
                ctx.warnings.push(format!(
                    "lua-loaded font {} char U+{:X} has an invalid ligatures field",
                    ctx.name, code
                ));
            }
        }
    }
    Ok(co)
}

/// The size/limit clamps of `font_from_lua`.
fn clamp_field(t: &LuaTable, key: &str, default: i32, min: i32, max: i32) -> i32 {
    num_field(t, key, default).clamp(min, max)
}

/// Everything `font_from_lua` reads besides the engine-level font record.
pub(crate) struct ParsedFont {
    pub lua: LuaFont,
    pub params: Vec<i32>,
    pub hyphen_char: i32,
    pub skew_char: i32,
    pub warnings: Vec<String>,
}

fn local_fonts_of(
    eng: &mut Engine,
    t: &LuaTable,
    f: FontId,
    name: &[u8],
    virtual_font: bool,
    allow_missing: bool,
) -> Result<Vec<FontId>, String> {
    let name_str = String::from_utf8_lossy(name).into_owned();
    let fonts = field(t, "fonts").and_then(|v| v.as_table());
    let count = fonts
        .as_ref()
        .map(|ft| ft.pairs::<Value, Value>().map(|p| p.len()).unwrap_or(0))
        .unwrap_or(0);
    if count > 0 {
        let ft = fonts.expect("fonts table");
        let mut l_fonts = vec![0 as FontId; count + 1];
        for i in 1..=count {
            let Some(entry) = ft.raw_geti::<Option<Value>>(i as i64).ok().flatten().and_then(|v| v.as_table())
            else {
                return Err(format!("invalid local font at index {i} in lua-loaded font '{name_str}' (1)"));
            };
            if let Some(id) = field(&entry, "id").and_then(|v| value_number(&v)) {
                let id = round(id);
                l_fonts[i] = if id == 0 { f } else { id as FontId };
                continue;
            }
            let Some(lname) = str_field(&entry, "name") else {
                return Err(format!("invalid local font at index {i} in lua-loaded font '{name_str}' (1)"));
            };
            let size = num_field(&entry, "size", -1000);
            l_fonts[i] = if lname == name { f } else { eng.lua_find_font_id(&lname, size) };
        }
        Ok(l_fonts)
    } else if virtual_font || allow_missing {
        Ok(Vec::new())
    } else {
        Ok(vec![0, f])
    }
}

/// luatex `font_from_lua`: read the font table `t` as font `f`.
pub(crate) fn font_from_lua(eng: &mut Engine, f: FontId, t: &LuaTable) -> Result<ParsedFont, String> {
    let mut lf = LuaFont::default();
    let mut warnings = Vec::new();
    lf.area = Some(str_field(t, "area").unwrap_or_default());
    lf.filename = str_field(t, "filename");
    lf.encodingname = str_field(t, "encodingname");
    let Some(name) = str_field(t, "name") else {
        return Err(format!("lua-loaded font '{f}' has no name!"));
    };
    lf.name = name.clone();
    lf.fullname = Some(str_field(t, "fullname").unwrap_or_else(|| name.clone()));
    lf.psname = str_field(t, "psname");
    lf.units_per_em = num_field(t, "units_per_em", 0);
    lf.designsize = num_field(t, "designsize", 655360);
    lf.size = num_field(t, "size", lf.designsize);
    lf.checksum = field(t, "checksum").and_then(|v| value_number(&v)).map_or(0, |n| n as i64 as u32);
    lf.direction = num_field(t, "direction", 0);
    lf.encodingbytes = num_field(t, "encodingbytes", 0) as u8;
    lf.streamprovider = num_field(t, "streamprovider", 0) as u8;
    lf.oldmath = bool_field(t, "oldmath", false);
    lf.tounicode = num_field(t, "tounicode", 0) as u8;
    lf.slant = clamp_field(t, "slant", 0, -2000, 2000);
    lf.extend = clamp_field(t, "extend", 1000, -5000, 5000);
    lf.squeeze = clamp_field(t, "squeeze", 1000, -5000, 5000);
    lf.width = clamp_field(t, "width", 0, 0, 5000);
    lf.mode = clamp_field(t, "mode", 0, 0, 3);
    let default_hyphen = eng.eqtb.int_params[crate::prim::IntParam::Defaulthyphenchar.idx() as usize];
    let default_skew = eng.eqtb.int_params[crate::prim::IntParam::Defaultskewchar.idx() as usize];
    let hyphen_char = num_field(t, "hyphenchar", default_hyphen);
    let skew_char = num_field(t, "skewchar", default_skew);
    lf.used = bool_field(t, "used", false);
    lf.attributes = str_field(t, "attributes").filter(|s| !s.is_empty());
    lf.ftype = match enum_field(t, "type", 0, FONT_TYPE_STRINGS) {
        1 => FontType::Virtual,
        2 => FontType::Real,
        _ => FontType::Unknown,
    };
    lf.format = match enum_field(t, "format", 0, FONT_FORMAT_STRINGS) {
        1 => FontFormat::Type1,
        2 => FontFormat::Type3,
        3 => FontFormat::TrueType,
        4 => FontFormat::OpenType,
        _ => FontFormat::Unknown,
    };
    let mode_of = |n: usize| match n {
        1 => FontMode::Horizontal,
        2 => FontMode::Vertical,
        _ => FontMode::Unknown,
    };
    lf.writingmode = mode_of(enum_field(t, "writingmode", 0, FONT_MODE_STRINGS));
    lf.identity = mode_of(enum_field(t, "identity", 0, FONT_MODE_STRINGS));
    lf.embedding = match enum_field(t, "embedding", 0, FONT_EMBEDDING_STRINGS) {
        1 => FontEmbedding::No,
        2 => FontEmbedding::Subset,
        3 => FontEmbedding::Full,
        _ => FontEmbedding::Unknown,
    };
    if lf.encodingbytes == 0 && matches!(lf.format, FontFormat::OpenType | FontFormat::TrueType) {
        lf.encodingbytes = 2;
    }
    lf.subfont = num_field(t, "subfont", 0);

    let local_fonts = local_fonts_of(eng, t, f, &name, lf.ftype == FontType::Virtual, false)?;

    // parameters
    let mut params = vec![0i32; 7];
    if let Some(pt) = field(t, "parameters").and_then(|v| v.as_table()) {
        let pairs = pt.pairs::<Value, Value>().map_err(|e| format!("{e:?}"))?;
        let mut n = 7usize;
        for (k, _) in &pairs {
            if let Some(i) = k.as_number().filter(|x| x.fract() == 0.0) {
                n = n.max(i as i64 as usize);
            }
        }
        params.resize(n, 0);
        for i in 1..=7i64 {
            if let Some(v) = pt.raw_geti::<Option<Value>>(i).ok().flatten().and_then(|v| v.as_number()) {
                params[i as usize - 1] = round(v);
            }
        }
        for (k, v) in &pairs {
            let value = value_number(v).map_or(0, round);
            if let Some(i) = k.as_number().filter(|x| x.fract() == 0.0) {
                let i = i as i64;
                if i >= 8 {
                    params[i as usize - 1] = if v.as_number().is_some() { value } else { 0 };
                }
            } else if let Some(s) = k.as_string_handle() {
                let idx = match s.to_bytes().as_slice() {
                    b"slant" => 1,
                    b"space" => 2,
                    b"space_stretch" => 3,
                    b"space_shrink" => 4,
                    b"x_height" => 5,
                    b"quad" => 6,
                    b"extra_space" => 7,
                    _ => 0,
                };
                if idx > 0 {
                    params[idx - 1] = value;
                }
            }
        }
    }

    // math parameters
    lf.nomath = bool_field(t, "nomath", false);
    if !lf.nomath {
        if let Some(mt) = field(t, "MathConstants").and_then(|v| v.as_table()) {
            for (k, v) in mt.pairs::<Value, Value>().map_err(|e| format!("{e:?}"))? {
                let idx = if let Some(n) = k.as_number() {
                    round(n)
                } else if let Some(s) = k.as_string_handle() {
                    let s = s.to_bytes();
                    MATH_PARAM_NAMES
                        .iter()
                        .position(|n| n.as_bytes() == s.as_slice())
                        .map_or(-1, |p| p as i32)
                } else {
                    0
                };
                let value = value_number(&v).map_or(0, round);
                if idx > 0 {
                    let idx = idx as usize;
                    if lf.math_params.len() < idx {
                        lf.math_params.resize(idx, 0);
                    }
                    lf.math_params[idx - 1] = value;
                }
            }
            lf.oldmath = false;
        } else {
            lf.oldmath = true;
        }
        if bool_field(t, "oldmath", false) {
            lf.oldmath = true;
        }
    } else {
        lf.oldmath = true;
    }

    // cidinfo
    if let Some(ct) = field(t, "cidinfo").and_then(|v| v.as_table()) {
        lf.cidinfo = Some(CidInfo {
            version: num_field(&ct, "version", 0),
            supplement: num_field(&ct, "supplement", 0),
            registry: str_field(&ct, "registry").unwrap_or_else(|| b"Adobe".to_vec()),
            ordering: str_field(&ct, "ordering").unwrap_or_else(|| b"Identity".to_vec()),
        });
    }

    // characters
    let name_str = String::from_utf8_lossy(&name).into_owned();
    if let Some(ct) = field(t, "characters").and_then(|v| v.as_table()) {
        let pairs = ct.pairs::<Value, Value>().map_err(|e| format!("{e:?}"))?;
        let mut bc: i32 = -1;
        let mut ec: i32 = 0;
        let mut any = false;
        for (k, v) in &pairs {
            if let (Some(n), Some(_)) = (k.as_number(), v.as_table()) {
                let i = n as i64;
                if i >= 0 {
                    let i = i as i32;
                    any = true;
                    ec = ec.max(i);
                    if bc < 0 || i < bc {
                        bc = i;
                    }
                }
            }
        }
        if any {
            lf.bc = bc;
            lf.ec = ec;
            let mut ctx = ParseCtx {
                font: f,
                name: &name_str,
                has_math: !lf.nomath,
                local_fonts: &local_fonts,
                atsize: lf.size,
                warnings: &mut warnings,
            };
            for (k, v) in &pairs {
                let Some(ctab) = v.as_table() else { continue };
                if let Some(n) = k.as_number() {
                    let i = n as i64;
                    if i >= 0 {
                        let co = char_from_lua(&ctab, i as i32, &mut ctx)?;
                        lf.chars.insert(i as u32, co);
                    }
                } else if let Some(s) = k.as_string_handle() {
                    match s.to_bytes().as_slice() {
                        b"left_boundary" => lf.left_boundary = Some(char_from_lua(&ctab, LEFT_BOUNDARY, &mut ctx)?),
                        b"right_boundary" => {
                            lf.right_boundary = Some(char_from_lua(&ctab, RIGHT_BOUNDARY, &mut ctx)?)
                        }
                        _ => {}
                    }
                }
            }
            // `kerns`/`ligatures` mentioning `right_boundary` create the
            // (empty) right boundary character.
            if lf.right_boundary.is_none()
                && lf.chars.values().any(|c| c.kerns.contains_key(&RIGHT_BOUNDARY) || c.ligatures.contains_key(&RIGHT_BOUNDARY))
            {
                lf.right_boundary = Some(LuaCharInfo::default());
            }
            // font expansion (handled last, like luatex)
            let mut step = num_field(t, "step", 0).clamp(0, 100);
            if step != 0 {
                let mut shrink = num_field(t, "shrink", 0).clamp(0, 500);
                shrink -= shrink % step;
                let mut stretch = num_field(t, "stretch", 0).clamp(0, 1000);
                stretch -= stretch % step;
                lf.step = step;
                lf.shrink = shrink.max(0);
                lf.stretch = stretch.max(0);
            } else {
                step = 0;
                lf.step = step;
            }
        } else {
            warnings.push(format!("lua-loaded font '{f}' with name '{name_str}' has no characters"));
        }
    } else {
        warnings.push(format!("lua-loaded font '{f}' with name '{name_str}' has no character table"));
    }
    lf.chars.shrink_to_fit();
    // The local fonts are part of the font: virtual commands and the PDF
    // writer resolve `Font` commands through them.
    Ok(ParsedFont { lua: lf, params, hyphen_char, skew_char, warnings })
}

/// `cache = "no" | "renew"` keep luatex from remembering the table.
pub(crate) fn cache_allowed(t: &LuaTable) -> bool {
    !matches!(str_field(t, "cache").as_deref(), Some(b"no") | Some(b"renew"))
}

/// luatex `characters_from_lua` (`font.addcharacters`): add or replace
/// characters of an existing Lua font.
pub(crate) fn characters_from_lua(eng: &mut Engine, f: FontId, t: &LuaTable) -> Result<Vec<String>, String> {
    let Some(font) = eng.eqtb.fonts.get(usize::from(f)).cloned() else {
        return Err("that integer id is not a valid font".to_string());
    };
    let Some(lua_font) = font.lua.as_ref() else {
        return Err("that integer id is not a valid font".to_string());
    };
    let mut lf = (**lua_font).clone();
    let mut warnings = Vec::new();
    lf.nomath = bool_field(t, "nomath", false);
    lf.ftype = match enum_field(t, "type", match lf.ftype {
        FontType::Unknown => 0,
        FontType::Virtual => 1,
        FontType::Real => 2,
    }, FONT_TYPE_STRINGS) {
        1 => FontType::Virtual,
        2 => FontType::Real,
        _ => FontType::Unknown,
    };
    let name = lf.name.clone();
    let local_fonts = local_fonts_of(eng, t, f, &name, false, lf.ftype == FontType::Virtual)?;
    let name_str = String::from_utf8_lossy(&name).into_owned();
    if let Some(ct) = field(t, "characters").and_then(|v| v.as_table()) {
        let pairs = ct.pairs::<Value, Value>().map_err(|e| format!("{e:?}"))?;
        let mut ctx = ParseCtx {
            font: f,
            name: &name_str,
            has_math: !lf.nomath,
            local_fonts: &local_fonts,
            atsize: lf.size,
            warnings: &mut warnings,
        };
        for (k, v) in &pairs {
            let (Some(n), Some(ctab)) = (k.as_number(), v.as_table()) else { continue };
            let i = n as i64;
            if i < 0 {
                continue;
            }
            let co = char_from_lua(&ctab, i as i32, &mut ctx)?;
            if lf.bc <= 0 && lf.chars.is_empty() || (i as i32) < lf.bc {
                lf.bc = i as i32;
            }
            lf.ec = lf.ec.max(i as i32);
            lf.chars.insert(i as u32, co);
        }
    }
    let new_font = std::rc::Rc::new(lf);
    let mut updated = (*font).clone();
    updated.lua = Some(new_font);
    eng.eqtb.fonts[usize::from(f)] = std::rc::Rc::new(updated);
    Ok(warnings)
}

// ------------------------------------------------------------------ writing

fn set_int(t: &LuaTable, key: &str, v: i32) -> Result<(), String> {
    t.raw_set(key, i64::from(v)).map_err(|e| format!("{e:?}"))
}

fn set_bytes(t: &LuaTable, key: &str, v: &[u8]) -> Result<(), String> {
    t.raw_set(key, LuaBytes(v.to_vec())).map_err(|e| format!("{e:?}"))
}

fn set_bool(t: &LuaTable, key: &str, v: bool) -> Result<(), String> {
    t.raw_set(key, v).map_err(|e| format!("{e:?}"))
}

fn pair_table(cx: &mut CallbackLua<'_>, pairs: &[(i32, i32)]) -> Result<LuaTable, String> {
    let list = cx.create_table().map_err(|e| format!("{e:?}"))?;
    for (i, (h, k)) in pairs.iter().enumerate() {
        let e = cx.create_table().map_err(|e| format!("{e:?}"))?;
        set_int(&e, "height", *h)?;
        set_int(&e, "kern", *k)?;
        list.raw_seti(i as i64 + 1, e).map_err(|e| format!("{e:?}"))?;
    }
    Ok(list)
}

fn variants_table(cx: &mut CallbackLua<'_>, vs: &[MathVariant]) -> Result<LuaTable, String> {
    let list = cx.create_table().map_err(|e| format!("{e:?}"))?;
    for (i, v) in vs.iter().enumerate() {
        let e = cx.create_table().map_err(|e| format!("{e:?}"))?;
        set_int(&e, "glyph", v.glyph)?;
        set_int(&e, "extender", v.extender)?;
        set_int(&e, "start", v.start)?;
        set_int(&e, "end", v.end)?;
        set_int(&e, "advance", v.advance)?;
        list.raw_seti(i as i64 + 1, e).map_err(|e| format!("{e:?}"))?;
    }
    Ok(list)
}

/// Table of the virtual `commands` of one character (`font_commands_to_lua`).
fn commands_table(cx: &mut CallbackLua<'_>, cmds: &[VfCommand]) -> Result<LuaTable, String> {
    let list = cx.create_table().map_err(|e| format!("{e:?}"))?;
    let mut i = 1i64;
    for c in cmds {
        let e = cx.create_table().map_err(|e| format!("{e:?}"))?;
        let name = |n: &str| LuaBytes(n.as_bytes().to_vec());
        match c {
            VfCommand::Font(id) => {
                e.raw_seti(1, name("font")).map_err(|e| format!("{e:?}"))?;
                e.raw_seti(2, i64::from(*id)).map_err(|e| format!("{e:?}"))?;
            }
            VfCommand::Push => e.raw_seti(1, name("push")).map_err(|e| format!("{e:?}"))?,
            VfCommand::Pop => e.raw_seti(1, name("pop")).map_err(|e| format!("{e:?}"))?,
            VfCommand::Char(ch) => {
                e.raw_seti(1, name("char")).map_err(|e| format!("{e:?}"))?;
                e.raw_seti(2, i64::from(*ch)).map_err(|e| format!("{e:?}"))?;
            }
            VfCommand::Rule(h, w) => {
                e.raw_seti(1, name("rule")).map_err(|e| format!("{e:?}"))?;
                e.raw_seti(2, i64::from(*h)).map_err(|e| format!("{e:?}"))?;
                e.raw_seti(3, i64::from(*w)).map_err(|e| format!("{e:?}"))?;
            }
            VfCommand::Right(r) => {
                e.raw_seti(1, name("right")).map_err(|e| format!("{e:?}"))?;
                e.raw_seti(2, i64::from(*r)).map_err(|e| format!("{e:?}"))?;
            }
            VfCommand::Down(d) => {
                e.raw_seti(1, name("down")).map_err(|e| format!("{e:?}"))?;
                e.raw_seti(2, i64::from(*d)).map_err(|e| format!("{e:?}"))?;
            }
            VfCommand::Pdf { mode, .. } => {
                e.raw_seti(1, name("pdf")).map_err(|e| format!("{e:?}"))?;
                e.raw_seti(2, i64::from(*mode)).map_err(|e| format!("{e:?}"))?;
                e.raw_seti(3, name("<pdf data>")).map_err(|e| format!("{e:?}"))?;
            }
            VfCommand::PdfMode(m) => {
                e.raw_seti(1, name("mode")).map_err(|e| format!("{e:?}"))?;
                e.raw_seti(2, i64::from(*m)).map_err(|e| format!("{e:?}"))?;
            }
            VfCommand::Special(_) => {
                e.raw_seti(1, name("special")).map_err(|e| format!("{e:?}"))?;
                e.raw_seti(2, name("<special data>")).map_err(|e| format!("{e:?}"))?;
            }
            VfCommand::Lua(_) => {
                e.raw_seti(1, name("lua")).map_err(|e| format!("{e:?}"))?;
                e.raw_seti(2, name("<lua data>")).map_err(|e| format!("{e:?}"))?;
            }
            VfCommand::Image(_) => {
                e.raw_seti(1, name("image")).map_err(|e| format!("{e:?}"))?;
                e.raw_seti(2, name("<image data>")).map_err(|e| format!("{e:?}"))?;
            }
            VfCommand::Node(_) => {
                e.raw_seti(1, name("node")).map_err(|e| format!("{e:?}"))?;
                e.raw_seti(2, name("<node data>")).map_err(|e| format!("{e:?}"))?;
            }
            VfCommand::Nop => e.raw_seti(1, name("nop")).map_err(|e| format!("{e:?}"))?,
            // luatex stores the scale packet but does not dump it.
            VfCommand::Scale(_) => continue,
        }
        list.raw_seti(i, e).map_err(|e| format!("{e:?}"))?;
        i += 1;
    }
    Ok(list)
}

/// luatex `font_char_to_lua`.
fn char_to_lua(cx: &mut CallbackLua<'_>, lf: &LuaFont, co: &LuaCharInfo) -> Result<LuaTable, String> {
    let t = cx.create_table().map_err(|e| format!("{e:?}"))?;
    set_int(&t, "width", co.width)?;
    set_int(&t, "height", co.height)?;
    set_int(&t, "depth", co.depth)?;
    if co.italic != 0 {
        set_int(&t, "italic", co.italic)?;
    }
    if co.vert_italic != 0 {
        set_int(&t, "vert_italic", co.vert_italic)?;
    }
    if co.top_accent != 0 && co.top_accent != i32::MIN {
        set_int(&t, "top_accent", co.top_accent)?;
    }
    if co.bot_accent != 0 && co.bot_accent != i32::MIN {
        set_int(&t, "bot_accent", co.bot_accent)?;
    }
    if co.expansion_factor != 1000 {
        set_int(&t, "expansion_factor", co.expansion_factor)?;
    }
    if co.left_protruding != 0 {
        set_int(&t, "left_protruding", co.left_protruding)?;
    }
    if co.right_protruding != 0 {
        set_int(&t, "right_protruding", co.right_protruding)?;
    }
    if lf.encodingbytes == 2 {
        set_int(&t, "index", co.index as i32)?;
    }
    if let Some(name) = &co.name {
        set_bytes(&t, "name", name)?;
    }
    if let Some(u) = &co.tounicode {
        set_bytes(&t, "tounicode", u)?;
    }
    let has_ext = co.extensible.is_some() || !co.hor_variants.is_empty() || !co.vert_variants.is_empty();
    if let Some(next) = co.next.filter(|_| !has_ext) {
        set_int(&t, "next", next as i32)?;
    }
    if co.used {
        set_bool(&t, "used", true)?;
    }
    if has_ext {
        if !co.hor_variants.is_empty() {
            let v = variants_table(cx, &co.hor_variants)?;
            t.raw_set("horiz_variants", v).map_err(|e| format!("{e:?}"))?;
        }
        if !co.vert_variants.is_empty() {
            let v = variants_table(cx, &co.vert_variants)?;
            t.raw_set("vert_variants", v).map_err(|e| format!("{e:?}"))?;
        }
    }
    if !co.kerns.is_empty() {
        let kt = cx.create_table().map_err(|e| format!("{e:?}"))?;
        for (k, v) in &co.kerns {
            if *k == RIGHT_BOUNDARY {
                kt.raw_set("right_boundary", i64::from(*v)).map_err(|e| format!("{e:?}"))?;
            } else {
                kt.raw_set(i64::from(*k), i64::from(*v)).map_err(|e| format!("{e:?}"))?;
            }
        }
        t.raw_set("kerns", kt).map_err(|e| format!("{e:?}"))?;
    }
    if !co.ligatures.is_empty() {
        let lt = cx.create_table().map_err(|e| format!("{e:?}"))?;
        for (k, lig) in &co.ligatures {
            let e = cx.create_table().map_err(|e| format!("{e:?}"))?;
            set_int(&e, "type", i32::from(lig.op))?;
            set_int(&e, "char", lig.replacement as i32)?;
            if *k == RIGHT_BOUNDARY {
                lt.raw_set("right_boundary", e).map_err(|e| format!("{e:?}"))?;
            } else {
                lt.raw_set(i64::from(*k), e).map_err(|e| format!("{e:?}"))?;
            }
        }
        t.raw_set("ligatures", lt).map_err(|e| format!("{e:?}"))?;
    }
    let mk = &co.math_kerns;
    if !(mk.top_right.is_empty() && mk.top_left.is_empty() && mk.bottom_right.is_empty() && mk.bottom_left.is_empty()) {
        let m = cx.create_table().map_err(|e| format!("{e:?}"))?;
        for (key, list) in [
            ("top_right", &mk.top_right),
            ("top_left", &mk.top_left),
            ("bottom_right", &mk.bottom_right),
            ("bottom_left", &mk.bottom_left),
        ] {
            if !list.is_empty() {
                let l = pair_table(cx, list)?;
                m.raw_set(key, l).map_err(|e| format!("{e:?}"))?;
            }
        }
        t.raw_set("mathkern", m).map_err(|e| format!("{e:?}"))?;
    }
    if let Some(cmds) = &co.commands {
        let c = commands_table(cx, cmds)?;
        t.raw_set("commands", c).map_err(|e| format!("{e:?}"))?;
    }
    Ok(t)
}

/// `parameters` (also the result of `font.getparameters`).
pub(crate) fn parameters_to_lua(cx: &mut CallbackLua<'_>, params: &[i32]) -> Result<LuaTable, String> {
    let t = cx.create_table().map_err(|e| format!("{e:?}"))?;
    for (i, p) in params.iter().enumerate() {
        let key = match i + 1 {
            1 => Some("slant"),
            2 => Some("space"),
            3 => Some("space_stretch"),
            4 => Some("space_shrink"),
            5 => Some("x_height"),
            6 => Some("quad"),
            7 => Some("extra_space"),
            _ => None,
        };
        match key {
            Some(k) => set_int(&t, k, *p)?,
            None => t.raw_seti(i as i64 + 1, i64::from(*p)).map_err(|e| format!("{e:?}"))?,
        }
    }
    Ok(t)
}

/// luatex `font_to_lua` without the cache: a fresh table for `lf`.
pub(crate) fn font_to_lua(
    cx: &mut CallbackLua<'_>,
    lf: &LuaFont,
    params: &[i32],
    used: bool,
    pdf_attr: Option<&[u8]>,
) -> Result<LuaTable, String> {
    let t = cx.create_table().map_err(|e| format!("{e:?}"))?;
    set_bytes(&t, "name", &lf.name)?;
    if let Some(area) = &lf.area {
        set_bytes(&t, "area", area)?;
    }
    if let Some(v) = &lf.filename {
        set_bytes(&t, "filename", v)?;
    }
    if let Some(v) = &lf.fullname {
        set_bytes(&t, "fullname", v)?;
    }
    if let Some(v) = &lf.psname {
        set_bytes(&t, "psname", v)?;
    }
    if let Some(v) = &lf.encodingname {
        set_bytes(&t, "encodingname", v)?;
    }
    set_bool(&t, "used", used)?;
    set_bytes(&t, "type", FONT_TYPE_STRINGS[lf.ftype as usize].as_bytes())?;
    set_bytes(&t, "format", FONT_FORMAT_STRINGS[lf.format as usize].as_bytes())?;
    set_bytes(&t, "writingmode", FONT_MODE_STRINGS[lf.writingmode as usize].as_bytes())?;
    set_bytes(&t, "identity", FONT_MODE_STRINGS[lf.identity as usize].as_bytes())?;
    set_bytes(&t, "embedding", FONT_EMBEDDING_STRINGS[lf.embedding as usize].as_bytes())?;
    set_int(&t, "streamprovider", i32::from(lf.streamprovider))?;
    set_int(&t, "units_per_em", lf.units_per_em)?;
    set_int(&t, "size", lf.size)?;
    set_int(&t, "designsize", lf.designsize)?;
    t.raw_set("checksum", i64::from(lf.checksum)).map_err(|e| format!("{e:?}"))?;
    set_int(&t, "slant", lf.slant)?;
    set_int(&t, "extend", lf.extend)?;
    set_int(&t, "squeeze", lf.squeeze)?;
    set_int(&t, "mode", lf.mode)?;
    set_int(&t, "width", lf.width)?;
    set_int(&t, "direction", lf.direction)?;
    set_int(&t, "encodingbytes", i32::from(lf.encodingbytes))?;
    set_int(&t, "subfont", lf.subfont)?;
    set_bool(&t, "oldmath", lf.oldmath)?;
    set_int(&t, "tounicode", i32::from(lf.tounicode))?;
    if lf.shrink != 0 {
        set_int(&t, "shrink", lf.shrink)?;
    }
    if lf.stretch != 0 {
        set_int(&t, "stretch", lf.stretch)?;
    }
    if lf.step != 0 {
        set_int(&t, "step", lf.step)?;
    }
    if let Some(a) = pdf_attr {
        set_bytes(&t, "attributes", a)?;
    }
    let p = parameters_to_lua(cx, params)?;
    t.raw_set("parameters", p).map_err(|e| format!("{e:?}"))?;
    let mc = cx.create_table().map_err(|e| format!("{e:?}"))?;
    for (i, v) in lf.math_params.iter().enumerate() {
        let k = i + 1;
        if k <= MATH_PARAM_MAX {
            set_int(&mc, MATH_PARAM_NAMES[k], *v)?;
        } else {
            mc.raw_seti(k as i64, i64::from(*v)).map_err(|e| format!("{e:?}"))?;
        }
    }
    t.raw_set("MathConstants", mc).map_err(|e| format!("{e:?}"))?;
    let ct = cx.create_table().map_err(|e| format!("{e:?}"))?;
    if let Some(co) = &lf.left_boundary {
        let c = char_to_lua(cx, lf, co)?;
        ct.raw_set("left_boundary", c).map_err(|e| format!("{e:?}"))?;
    }
    if let Some(co) = &lf.right_boundary {
        let c = char_to_lua(cx, lf, co)?;
        ct.raw_set("right_boundary", c).map_err(|e| format!("{e:?}"))?;
    }
    let mut codes: Vec<u32> = lf.chars.keys().copied().collect();
    codes.sort_unstable();
    for code in codes {
        let c = char_to_lua(cx, lf, &lf.chars[&code])?;
        ct.raw_set(i64::from(code), c).map_err(|e| format!("{e:?}"))?;
    }
    t.raw_set("characters", ct).map_err(|e| format!("{e:?}"))?;
    Ok(t)
}

// -------------------------------------------------------- TFM as a Lua font

/// luatex's view of a TFM font (`read_tfm_info`): per-character kern and
/// ligature tables built from the lig/kern program, boundary characters and
/// math lists.
pub(crate) fn lua_font_from_tfm(font: &Font, name: &[u8]) -> LuaFont {
    use crate::tfm::{TAG_EXT, TAG_LIG, TAG_LIST};
    let mut lf = LuaFont {
        name: name.to_vec(),
        area: None,
        designsize: font.dsize,
        size: font.at_size,
        slant: 0,
        extend: 1000,
        squeeze: 1000,
        bc: i32::from(font.bc),
        ec: i32::from(font.ec),
        ..LuaFont::default()
    };
    // The checksum is not kept by the TFM parser; callers that need it
    // (font.read_tfm) fill it in from the file.
    let lig_kern = &font.lig_kern;
    // A step list: `skip_byte` semantics of tex.web §545.
    let program = |start: usize, mut visit: Box<dyn FnMut(&crate::tfm::LigStep) + '_>| {
        let mut k = start;
        let mut guard = 0;
        while k < lig_kern.len() && guard < 100_000 {
            let s = &lig_kern[k];
            if s.skip <= 128 {
                visit(s);
            }
            if s.skip == 0 {
                k += 1;
            } else if s.skip >= 128 {
                break;
            } else {
                k += usize::from(s.skip) + 1;
            }
            guard += 1;
        }
    };
    let bchar = font.bchar.map(i32::from);
    let kern_of = |s: &crate::tfm::LigStep| -> i32 {
        let idx = 256 * (usize::from(s.op) - 128) + usize::from(s.rem);
        font.kerns.get(idx).copied().unwrap_or(0)
    };
    // left boundary program: the last instruction, when its skip byte is 255
    let bch_label = match lig_kern.last() {
        Some(last) if last.skip == 255 => Some(256 * usize::from(last.op) + usize::from(last.rem)),
        _ => None,
    };
    if let Some(start) = bch_label {
        if start < lig_kern.len() {
            let mut co = LuaCharInfo::default();
            program(
                start,
                Box::new(|s| {
                    if s.op >= 128 {
                        co.kerns.entry(i32::from(s.next_char)).or_insert_with(|| kern_of(s));
                    } else {
                        co.ligatures
                            .entry(i32::from(s.next_char))
                            .or_insert(LuaLig { op: s.op, replacement: u32::from(s.rem) });
                    }
                }),
            );
            if !co.kerns.is_empty() || !co.ligatures.is_empty() {
                lf.left_boundary = Some(co);
            }
        }
    }
    for code in font.bc..=font.ec {
        if !font.char_present(code) {
            continue;
        }
        let ci = &font.chars[usize::from(code)];
        let mut co = LuaCharInfo {
            width: ci.width,
            height: ci.height,
            depth: ci.depth,
            italic: ci.italic,
            index: u32::from(code),
            ..LuaCharInfo::default()
        };
        match ci.tag {
            TAG_LIST => co.next = Some(u32::from(ci.remainder)),
            TAG_EXT => {
                if let Some(e) = font.ext.get(usize::from(ci.remainder)) {
                    let e = Extensible {
                        top: i32::from(e.top),
                        bot: i32::from(e.bot),
                        mid: i32::from(e.mid),
                        rep: i32::from(e.rep),
                    };
                    co.vert_variants = extensible_variants(&e);
                    co.extensible = Some(e);
                }
            }
            TAG_LIG => {
                let mut start = usize::from(ci.remainder);
                if let Some(s) = lig_kern.get(start) {
                    if s.skip > 128 {
                        start = 256 * usize::from(s.op) + usize::from(s.rem);
                    }
                }
                program(
                    start,
                    Box::new(|s| {
                        let is_bchar = bchar == Some(i32::from(s.next_char));
                        if s.op >= 128 {
                            let kern = kern_of(s);
                            if is_bchar {
                                co.kerns.entry(RIGHT_BOUNDARY).or_insert(kern);
                            }
                            co.kerns.entry(i32::from(s.next_char)).or_insert(kern);
                        } else {
                            let lig = LuaLig { op: s.op, replacement: u32::from(s.rem) };
                            if is_bchar {
                                co.ligatures.entry(RIGHT_BOUNDARY).or_insert(lig);
                            }
                            co.ligatures.entry(i32::from(s.next_char)).or_insert(lig);
                        }
                    }),
                );
            }
            _ => {}
        }
        lf.chars.insert(u32::from(code), co);
    }
    if let Some(b) = font.bchar {
        if let Some(src) = lf.chars.get(&u32::from(b)).cloned() {
            lf.right_boundary = Some(src);
        } else {
            lf.right_boundary = Some(LuaCharInfo::default());
        }
    }
    lf
}

// ------------------------------------------------------------------ library

use tex_lua::{Lua, LuaApi};

/// Run `f` on the engine whose Lua call is in progress.
fn on_engine<R>(f: impl FnOnce(&mut Engine) -> Result<R, String>) -> Result<R, String> {
    crate::lua_bridge::with_engine(f)?
}

fn fail(cx: &mut CallbackLua<'_>, message: String) -> tex_lua::LuaError {
    cx.error(message)
}

/// Everything needed to turn an engine font into a Lua table.
struct FontSnapshot {
    lua: std::rc::Rc<LuaFont>,
    params: Vec<i32>,
    used: bool,
    attributes: Option<Vec<u8>>,
}

impl Engine {
    /// luatex `font_used`/`font_touched`.
    pub(crate) fn lua_font_frozen(&self, f: FontId) -> bool {
        self.lua_fonts.touched.contains(&f) || self.lua_fonts.used.contains(&f)
    }

    fn lua_font_snapshot(&mut self, id: i64) -> Option<FontSnapshot> {
        if id <= 0 || !self.lua_font_valid(id) {
            return None;
        }
        let f = id as FontId;
        let font = self.eqtb.fonts[usize::from(f)].clone();
        let mut params = self.eqtb.font_params[usize::from(f)].clone();
        if params.len() < 7 {
            params.resize(7, 0);
        }
        let used = self.lua_fonts.used.contains(&f);
        let lua = match &font.lua {
            Some(lua) => lua.clone(),
            None => {
                let mut lf = lua_font_from_tfm(&font, font.tfm_name.as_bytes());
                lf.checksum = self.tfm_checksum(&font.tfm_name).unwrap_or(0);
                // `\font` runs `do_vf`: real unless a virtual font exists
                let virtual_font =
                    self.font_loader.vf_fonts.contains_key(&(font.tfm_name.clone(), font.at_size));
                lf.ftype = if virtual_font { FontType::Virtual } else { FontType::Real };
                lf.direction = if virtual_font { 0 } else { -1 };
                std::rc::Rc::new(lf)
            }
        };
        Some(FontSnapshot { lua, params, used, attributes: None })
    }
}

/// The Lua source of the library; `B` holds the native helpers.
const FONT_PRELUDE: &str = r##"
local B = __ratex_font_bridge
__ratex_font_bridge = nil
local cache = {}
__ratex_font_cache = cache
local type, rawget, error, select, setmetatable = type, rawget, error, select, setmetatable

local function remember(id, t)
  local mode = rawget(t, "cache")
  if mode == "no" or mode == "renew" then cache[id] = nil else cache[id] = t end
end

font = {}

function font.define(...)
  local n = select("#", ...)
  local id, t
  if n == 2 then
    id, t = ...
  elseif n == 1 then
    t = ...
  else
    error("font creation failed, no table passed")
  end
  if type(t) ~= "table" then
    error("bad argument #" .. n .. " to 'define' (table expected, got " .. type(t) .. ")")
  end
  local rid = B.define(id, t)
  remember(rid, t)
  return rid
end

function font.setfont(id, t)
  if type(id) ~= "number" then
    error("bad argument #1 to 'setfont' (number expected, got " .. type(id) .. ")")
  end
  if id ~= 0 then
    if type(t) ~= "table" then
      error("bad argument #" .. select("#", id, t) .. " to 'setfont' (table expected, got " .. type(t) .. ")")
    end
    B.setfont(id, t)
    remember(id, t)
  end
end

function font.addcharacters(id, t)
  if type(id) ~= "number" then
    error("bad argument #1 to 'addcharacters' (number expected, got " .. type(id) .. ")")
  end
  if id ~= 0 then
    if type(t) ~= "table" then
      error("bad argument #2 to 'addcharacters' (table expected, got " .. type(t) .. ")")
    end
    B.addcharacters(id, t)
  end
end

function font.getfont(id)
  if type(id) ~= "number" then
    error("bad argument #1 to 'getfont' (number expected, got " .. type(id) .. ")")
  end
  if id ~= 0 and B.valid(id) then
    return cache[id] or B.getfont(id)
  end
  return nil
end

function font.getcopy(id)
  if type(id) ~= "number" then
    error("bad argument #1 to 'getcopy' (number expected, got " .. type(id) .. ")")
  end
  if id ~= 0 and B.valid(id) then
    return B.getfont(id)
  end
  return nil
end

font.getparameters = B.getparameters
font.read_tfm = B.read_tfm
font.read_vf = B.read_vf
font.current = B.current
font.max = B.max
font.frozen = B.frozen
font.settounicode = B.settounicode
font.setexpansion = B.setexpansion
font.nextid = B.nextid
font.id = B.id

local function each_next(m, i)
  i = i + 1
  while i <= m and not B.valid(i) do i = i + 1 end
  if i > m then return nil end
  return i, cache[i] or B.getfont(i)
end

function font.each()
  return each_next, B.max(), 0
end

font.fonts = setmetatable({}, {
  __index = function(_, i) return font.getfont(i) end,
  __newindex = function(_, i, t) return font.setfont(i, t) end,
})
"##;

/// Install the `font` library (luatex `lfontlib.c`).
pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let b: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;
    macro_rules! callback {
        ($name:literal, $body:expr) => {
            b.set(
                $name,
                lua.create_callback($body).map_err(|e| format!("{}: {e:?}", $name))?,
            )
            .map_err(|e| format!("{}: {e:?}", $name))?
        };
    }

    callback!("define", |cx| {
        let id: Option<i64> = cx.arg(1)?;
        let t: LuaTable = cx.arg(2)?;
        let id = on_engine(|e| {
            let (f, fresh) = match id {
                Some(id) => {
                    if id <= 0 || !e.lua_font_valid(id) {
                        return Err("font creation failed, invalid id passed".to_string());
                    }
                    (id as FontId, false)
                }
                None => (e.lua_new_font(), true),
            };
            match font_from_lua(e, f, &t) {
                Ok(parsed) => {
                    e.lua_install_font(f, parsed);
                    Ok(i64::from(f))
                }
                Err(msg) => {
                    if fresh {
                        e.lua_delete_font(f);
                    }
                    Err(msg)
                }
            }
        })
        .map_err(|m| fail(cx, m))?;
        cx.push(id)
    });

    callback!("setfont", |cx| {
        let id: i64 = cx.arg(1)?;
        let t: LuaTable = cx.arg(2)?;
        on_engine(|e| {
            if id <= 0 || !e.lua_font_valid(id) {
                return Err("that integer id is not a valid font".to_string());
            }
            let f = id as FontId;
            if e.lua_font_frozen(f) {
                return Err("that font has been accessed already, changing it is forbidden".to_string());
            }
            let parsed = font_from_lua(e, f, &t)?;
            e.lua_install_font(f, parsed);
            Ok(())
        })
        .map_err(|m| fail(cx, m))?;
        Ok(0)
    });

    callback!("addcharacters", |cx| {
        let id: i64 = cx.arg(1)?;
        let t: LuaTable = cx.arg(2)?;
        on_engine(|e| {
            if id <= 0 || !e.lua_font_valid(id) {
                return Err("that integer id is not a valid font".to_string());
            }
            let warnings = characters_from_lua(e, id as FontId, &t)?;
            for message in warnings {
                e.warning_at(&format!("luatex warning (font): {message}"), None);
            }
            Ok(())
        })
        .map_err(|m| fail(cx, m))?;
        Ok(0)
    });

    callback!("valid", |cx| {
        let id: i64 = cx.arg(1)?;
        let ok = on_engine(|e| Ok(e.lua_font_valid(id))).map_err(|m| fail(cx, m))?;
        cx.push(ok)
    });

    callback!("max", |cx| {
        let n = on_engine(|e| Ok(e.lua_font_max())).map_err(|m| fail(cx, m))?;
        cx.push(n)
    });

    callback!("getfont", |cx| {
        let id: i64 = cx.arg(1)?;
        let snapshot = on_engine(|e| Ok(e.lua_font_snapshot(id))).map_err(|m| fail(cx, m))?;
        match snapshot {
            Some(s) => {
                let t = font_to_lua(cx, &s.lua, &s.params, s.used, s.attributes.as_deref())
                    .map_err(|m| fail(cx, m))?;
                cx.push(t)
            }
            None => cx.push(()),
        }
    });

    callback!("getparameters", |cx| {
        let id: i64 = cx.arg(1)?;
        let params = on_engine(|e| Ok(e.lua_font_snapshot(id).map(|s| s.params))).map_err(|m| fail(cx, m))?;
        match params {
            Some(p) => {
                let t = parameters_to_lua(cx, &p).map_err(|m| fail(cx, m))?;
                cx.push(t)
            }
            None => Ok(0),
        }
    });

    callback!("current", |cx| {
        let id: Option<i64> = cx.arg(1)?;
        let id = id.unwrap_or(0);
        if id > 0 {
            on_engine(|e| {
                if !e.lua_font_valid(id) {
                    return Err("expected a valid font id".to_string());
                }
                e.eqtb.define_cur_font(id as FontId, false);
                Ok(())
            })
            .map_err(|m| fail(cx, m))?;
            Ok(0)
        } else {
            let cur = on_engine(|e| Ok(i64::from(e.eqtb.cur_font_val))).map_err(|m| fail(cx, m))?;
            cx.push(cur)
        }
    });

    callback!("frozen", |cx| {
        let id: i64 = cx.arg(1)?;
        if id == 0 {
            return Err(fail(cx, "expected an integer argument".to_string()));
        }
        let r = on_engine(|e| Ok(e.lua_font_valid(id).then(|| e.lua_font_frozen(id as FontId))))
            .map_err(|m| fail(cx, m))?;
        cx.push(r)
    });

    callback!("nextid", |cx| {
        let keep: Option<bool> = if cx.arg_count() == 1 { Some(cx.arg::<bool>(1)?) } else { None };
        let id = on_engine(|e| {
            Ok(if keep == Some(true) { e.lua_new_font() } else { e.lua_next_font_id() })
        })
        .map_err(|m| fail(cx, m))?;
        cx.push(i64::from(id))
    });

    callback!("id", |cx| {
        let name: tex_lua::LuaString = cx.arg(1)?;
        let name = name.to_bytes();
        let id = on_engine(|e| {
            let found = e
                .cs
                .lookup(&name)
                .and_then(|cs| e.eqtb.resolve(cs).cloned())
                .and_then(|eq| match eq {
                    crate::eqtb::Equiv::FontRef(k) => Some(i64::from(k)),
                    _ => None,
                });
            Ok(found.unwrap_or(-1))
        })
        .map_err(|m| fail(cx, m))?;
        cx.push(id)
    });

    callback!("settounicode", |cx| {
        let font: i64 = cx.arg(1)?;
        let code: i64 = cx.arg(2)?;
        let value: Option<tex_lua::LuaString> = match cx.arg_kind(3) {
            Some(tex_lua::LuaValueKind::String) => Some(cx.arg(3)?),
            _ => None,
        };
        let value = value.map(|v| v.to_bytes());
        on_engine(|e| {
            if font == 0 || !e.lua_font_valid(font) {
                return Err("that integer id is not a valid font".to_string());
            }
            e.lua_set_tounicode(font as FontId, code, value);
            Ok(())
        })
        .map_err(|m| fail(cx, m))?;
        Ok(0)
    });

    callback!("setexpansion", |cx| {
        let font: i64 = cx.arg(1)?;
        let stretch: i64 = cx.arg(2)?;
        let shrink: i64 = cx.arg(3)?;
        let step: i64 = cx.arg(4)?;
        if font != 0 {
            on_engine(|e| {
                if !e.lua_font_valid(font) {
                    return Err("that integer id is not a valid font".to_string());
                }
                e.lua_set_expansion(font as FontId, stretch as i32, shrink as i32, step as i32);
                Ok(())
            })
            .map_err(|m| fail(cx, m))?;
        }
        Ok(0)
    });

    callback!("read_tfm", |cx| {
        let name = match cx.arg_kind(1) {
            Some(tex_lua::LuaValueKind::String) => cx.arg::<tex_lua::LuaString>(1)?.to_bytes(),
            _ => return Err(fail(cx, "expected tfm name as first argument".to_string())),
        };
        let size = match cx.arg_kind(2) {
            Some(tex_lua::LuaValueKind::Integer | tex_lua::LuaValueKind::Float) => cx.arg::<f64>(2)?,
            _ => return Err(fail(cx, "expected an integer size as second argument".to_string())),
        };
        if name.is_empty() {
            return Err(fail(cx, "expected tfm name as first argument".to_string()));
        }
        let size = round(size);
        let loaded = on_engine(|e| Ok(e.lua_read_tfm(&name, size))).map_err(|m| fail(cx, m))?;
        match loaded {
            Some((lf, params)) => {
                let t = font_to_lua(cx, &lf, &params, false, None).map_err(|m| fail(cx, m))?;
                cx.push(t)
            }
            None => Err(fail(cx, "font loading failed".to_string())),
        }
    });

    callback!("read_vf", |cx| {
        let name = match cx.arg_kind(1) {
            Some(tex_lua::LuaValueKind::String) => cx.arg::<tex_lua::LuaString>(1)?.to_bytes(),
            _ => return Err(fail(cx, "expected vf name as first argument".to_string())),
        };
        if name.is_empty() {
            return Err(fail(cx, "expected vf name as first argument".to_string()));
        }
        let size = match cx.arg_kind(2) {
            Some(tex_lua::LuaValueKind::Integer | tex_lua::LuaValueKind::Float) => round(cx.arg::<f64>(2)?),
            _ => return Err(fail(cx, "expected an integer size as second argument".to_string())),
        };
        let data = on_engine(|e| Ok(e.lua_read_vf_bytes(&name))).map_err(|m| fail(cx, m))?;
        let Some(data) = data else {
            return cx.push(());
        };
        let t = crate::lua_font_vf::vf_table(cx, &name, &data, size).map_err(|m| fail(cx, m))?;
        cx.push(t)
    });

    install_vf(lua)?;
    lua.set_global("__ratex_font_bridge", b).map_err(|e| format!("{e:?}"))?;
    lua.execute(FONT_PRELUDE).map_err(|e| format!("font library: {e:?}"))?;
    Ok(())
}

/// The `vf` library (luatex `lfontlib.c` `vflib`): helpers for the Lua
/// functions of virtual fonts.
fn install_vf(lua: &mut Lua) -> Result<(), String> {
    use crate::pdfrender::with_vf_packet;
    let vf: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;
    macro_rules! vf_fn {
        ($name:literal, |$cx:ident| $body:expr) => {
            vf.set(
                $name,
                lua.create_callback(move |$cx| $body).map_err(|e| format!("{}: {e:?}", $name))?,
            )
            .map_err(|e| format!("{}: {e:?}", $name))?
        };
    }
    vf_fn!("char", |cx| {
        let k: i64 = cx.arg(1)?;
        with_vf_packet("char", |ctx, st| ctx.vf_lib_char(st, k as u32)).map_err(|m| fail(cx, m))?;
        Ok(0)
    });
    vf_fn!("down", |cx| {
        let i: i64 = cx.arg(1)?;
        with_vf_packet("down", |ctx, st| ctx.vf_lib_down(st, i as i32)).map_err(|m| fail(cx, m))?;
        Ok(0)
    });
    vf_fn!("fontid", |cx| {
        let i: i64 = cx.arg(1)?;
        with_vf_packet("fontid", |_, st| st.set_font(i as FontId)).map_err(|m| fail(cx, m))?;
        Ok(0)
    });
    vf_fn!("nop", |cx| {
        with_vf_packet("nop", |_, _| ()).map_err(|m| fail(cx, m))?;
        Ok(0)
    });
    vf_fn!("pop", |cx| {
        with_vf_packet("pop", |ctx, st| ctx.vf_lib_pop(st))
            .and_then(|r| r)
            .map_err(|m| fail(cx, m))?;
        Ok(0)
    });
    vf_fn!("push", |cx| {
        with_vf_packet("push", |ctx, st| ctx.vf_lib_push(st)).map_err(|m| fail(cx, m))?;
        Ok(0)
    });
    vf_fn!("right", |cx| {
        let i: i64 = cx.arg(1)?;
        with_vf_packet("right", |ctx, st| ctx.vf_lib_right(st, i as i32)).map_err(|m| fail(cx, m))?;
        Ok(0)
    });
    vf_fn!("rule", |cx| {
        let h: i64 = cx.arg(1)?;
        let v: i64 = cx.arg(2)?;
        with_vf_packet("rule", |ctx, st| ctx.vf_lib_rule(st, h as i32, v as i32)).map_err(|m| fail(cx, m))?;
        Ok(0)
    });
    vf_fn!("special", |cx| {
        let s: tex_lua::LuaString = cx.arg(1)?;
        let data = s.to_bytes();
        with_vf_packet("special", |ctx, st| ctx.vf_lib_special(st, &data)).map_err(|m| fail(cx, m))?;
        Ok(0)
    });
    vf_fn!("pdf", |cx| {
        let n = cx.arg_count();
        let s: tex_lua::LuaString = cx.arg(n.max(1))?;
        let data = s.to_bytes();
        with_vf_packet("pdf", |ctx, st| ctx.vf_lib_pdf(st, &data)).map_err(|m| fail(cx, m))?;
        Ok(0)
    });
    lua.set_global("vf", vf).map_err(|e| format!("{e:?}"))
}
