//! Fonts and text.
//!
//! Type 3 fonts run their glyph procedures, so their glyphs become ordinary
//! paths. Every other font is rendered with the closest of the 14 standard PDF
//! fonts (as Ghostscript substitutes missing fonts), positioned with the
//! standard fonts' metrics; the PDF importer embeds the matching URW outlines.

use std::rc::Rc;

use crate::base14::{
    GlyphName, DINGBATS_WIDTHS, LATIN_GLYPHS, LATIN_WIDTHS, STANDARD_ENCODING, SYMBOL_WIDTHS,
};
use crate::graphics::{extend_path, grestore, gsave, put_num_prec, Output, Seg};
use crate::interp::{err, Interp, OpFn, Res};
use crate::types::{FxMap, Key, Matrix, PsArray, PsDict, Value};

pub(crate) const BASE14_NAMES: [&str; 14] = [
    "Helvetica",
    "Helvetica-Bold",
    "Helvetica-Oblique",
    "Helvetica-BoldOblique",
    "Times-Roman",
    "Times-Bold",
    "Times-Italic",
    "Times-BoldItalic",
    "Courier",
    "Courier-Bold",
    "Courier-Oblique",
    "Courier-BoldOblique",
    "Symbol",
    "ZapfDingbats",
];

const SYMBOL: u8 = 12;
const DINGBATS: u8 = 13;

/// The standard font closest to a PostScript font name.
pub(crate) fn base14_for(name: &[u8]) -> u8 {
    if let Some(k) = BASE14_NAMES.iter().position(|n| n.as_bytes() == name) {
        return k as u8;
    }
    let lower = String::from_utf8_lossy(name).to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if has(&["symbol"]) {
        return SYMBOL;
    }
    if has(&["dingbat"]) {
        return DINGBATS;
    }
    let family = if has(&["courier", "mono", "typewriter", "consol", "fixed"]) {
        8
    } else if has(&["times", "roman", "georgia", "garamond", "palatino", "bookman", "century", "schoolbook", "minion", "cambria", "nimbusrom"])
        || (lower.contains("serif") && !lower.contains("sans"))
    {
        4
    } else {
        0
    };
    let bold = has(&["bold", "black", "heavy", "demi"]);
    let italic = has(&["italic", "oblique", "slant", "ital"]);
    family + u8::from(bold) + 2 * u8::from(italic)
}

/// Advance width of a standard-font glyph in 1/1000 em.
pub(crate) fn base14_width(base: u8, glyph: &[u8]) -> f64 {
    let table: &[(GlyphName, u16)] = match base {
        SYMBOL => &SYMBOL_WIDTHS,
        DINGBATS => &DINGBATS_WIDTHS,
        _ => {
            return LATIN_GLYPHS
                .binary_search_by(|g| (*g).cmp(glyph))
                .map_or(0.0, |k| f64::from(LATIN_WIDTHS[usize::from(base)][k]));
        }
    };
    table.binary_search_by(|(g, _)| (*g).cmp(glyph)).map_or(0.0, |k| f64::from(table[k].1))
}

/// A PDF font resource: a standard font with a custom encoding assembled from
/// the glyph names actually shown.
pub(crate) struct FontRes {
    pub(crate) base: u8,
    pub(crate) codes: Vec<Option<Rc<[u8]>>>,
    by_name: FxMap<Rc<[u8]>, u8>,
}

impl Output {
    /// PDF font resource index and code for a glyph of a standard font.
    fn font_code(&mut self, base: u8, glyph: &Rc<[u8]>, preferred: u8) -> (usize, u8) {
        for (k, res) in self.fonts.iter().enumerate() {
            if res.base == base {
                if let Some(&code) = res.by_name.get(glyph) {
                    return (k, code);
                }
            }
        }
        for (k, res) in self.fonts.iter_mut().enumerate() {
            if res.base != base {
                continue;
            }
            let free = if res.codes[usize::from(preferred)].is_none() {
                Some(preferred)
            } else {
                (32..=255).chain(0..32).find(|&c| res.codes[usize::from(c)].is_none())
            };
            if let Some(code) = free {
                res.codes[usize::from(code)] = Some(glyph.clone());
                res.by_name.insert(glyph.clone(), code);
                return (k, code);
            }
        }
        let mut res = FontRes { base, codes: vec![None; 256], by_name: FxMap::default() };
        res.codes[usize::from(preferred)] = Some(glyph.clone());
        res.by_name.insert(glyph.clone(), preferred);
        self.fonts.push(res);
        (self.fonts.len() - 1, preferred)
    }
}

/// State of a running Type 3 glyph procedure.
pub(crate) struct GlyphCtx {
    width: Option<(f64, f64)>,
}

struct FontInfo {
    dict: PsDict,
    font_type: i64,
    matrix: Matrix,
    encoding: Option<PsArray>,
    base14: u8,
}

impl Interp {
    fn font_info(&mut self, dict: &PsDict) -> Res<FontInfo> {
        let get = |k| dict.get(&Key::Name(k));
        let font_type = get(self.n.font_type).and_then(|v| v.as_f64()).unwrap_or(1.0) as i64;
        let matrix = match get(self.n.font_matrix) {
            Some(m) => self.matrix_from(&m)?,
            None => return err("invalidfont", "font has no FontMatrix"),
        };
        let encoding = match get(self.n.encoding) {
            Some(Value::Array(a) | Value::Proc(a)) => Some(a),
            _ => None,
        };
        let base14 = match get(self.n.base14) {
            Some(Value::Int(k)) if (0..14).contains(&k) => k as u8,
            _ => 0,
        };
        Ok(FontInfo { dict: dict.clone(), font_type, matrix, encoding, base14 })
    }

    fn current_font(&mut self) -> Res<FontInfo> {
        let Some(font) = self.gs.font.clone() else {
            return err("invalidfont", "no current font");
        };
        self.font_info(&font)
    }

    fn glyph_name(&self, info: &FontInfo, code: u8) -> crate::types::NameId {
        match info.encoding.as_ref().filter(|e| u32::from(code) < e.len).map(|e| e.get(u32::from(code))) {
            Some(Value::Name(n) | Value::ExecName(n)) => n,
            _ => self.n.notdef,
        }
    }

    /// Renders (unless suppressed) one glyph at the current point and returns
    /// its advance in user space.
    fn draw_glyph(&mut self, info: &FontInfo, code: u8, glyph: crate::types::NameId, text: &mut TextBlock) -> Res<(f64, f64)> {
        if info.font_type == 3 {
            text.flush(self);
            return self.draw_type3(info, code, glyph);
        }
        let name = self.name_text(glyph);
        let w = base14_width(info.base14, &name);
        if self.emitting() && self.gs.device.is_some() {
            let (res, pdf_code) = self.out.font_code(info.base14, &name, code);
            let (ux, uy) = self.current_user_point()?;
            text.glyph(res, pdf_code, &info.matrix, (ux, uy), &self.gs.ctm);
        }
        Ok(info.matrix.apply_delta(w, 0.0))
    }

    fn draw_type3(&mut self, info: &FontInfo, code: u8, glyph: crate::types::NameId) -> Res<(f64, f64)> {
        let (ux, uy) = self.current_user_point()?;
        let build_glyph = info.dict.get(&Key::Name(self.n.build_glyph));
        let build_char = info.dict.get(&Key::Name(self.n.build_char));
        gsave(self, false)?;
        self.gs.ctm = info.matrix.then(&Matrix::translate(ux, uy)).then(&self.gs.ctm);
        self.new_path();
        let outer = self.glyph.replace(GlyphCtx { width: None });
        let result = (|| {
            self.push(Value::Dict(info.dict.clone()))?;
            match (build_glyph, build_char) {
                (Some(proc), _) => {
                    self.push(Value::Name(glyph))?;
                    self.call(proc)
                }
                (None, Some(proc)) => {
                    self.push(Value::Int(i64::from(code)))?;
                    self.call(proc)
                }
                (None, None) => err("invalidfont", "Type 3 font without BuildGlyph or BuildChar"),
            }
        })();
        let width = std::mem::replace(&mut self.glyph, outer).and_then(|g| g.width);
        grestore(self);
        result?;
        let (wx, wy) = width.unwrap_or((0.0, 0.0));
        Ok(info.matrix.apply_delta(wx, wy))
    }

    fn advance(&mut self, dx: f64, dy: f64) -> Res {
        let (cx, cy) = self.gs.cp.map_or_else(|| err("nocurrentpoint", "show"), Ok)?;
        let (tx, ty) = self.gs.ctm.apply_delta(dx, dy);
        self.gs.cp = Some((cx + tx, cy + ty));
        Ok(())
    }

    /// Shows `bytes` with per-glyph extra displacement from `extra`.
    fn show_text(&mut self, bytes: &[u8], mut extra: impl FnMut(&mut Interp, usize, u8, (f64, f64)) -> Res<(f64, f64)>) -> Res {
        let info = self.current_font()?;
        if self.gs.cp.is_none() {
            return err("nocurrentpoint", "show");
        }
        let mut text = TextBlock::default();
        let result = (|| {
            for (k, &code) in bytes.iter().enumerate() {
                let glyph = self.glyph_name(&info, code);
                let w = self.draw_glyph(&info, code, glyph, &mut text)?;
                let (dx, dy) = extra(self, k, code, w)?;
                if (dx, dy) != w {
                    text.reposition();
                }
                self.advance(dx, dy)?;
            }
            Ok(())
        })();
        text.flush(self);
        result
    }

    fn string_width(&mut self, bytes: &[u8]) -> Res<(f64, f64)> {
        let info = self.current_font()?;
        let saved = (self.gs.cp, self.gs.path.len());
        let origin = self.gs.ctm.apply(0.0, 0.0);
        self.gs.cp = Some(origin);
        self.suppress += 1;
        let mut total = (0.0, 0.0);
        let result = (|| {
            let mut text = TextBlock::default();
            for &code in bytes {
                let glyph = self.glyph_name(&info, code);
                let (dx, dy) = self.draw_glyph(&info, code, glyph, &mut text)?;
                total.0 += dx;
                total.1 += dy;
                self.advance(dx, dy)?;
            }
            Ok(())
        })();
        self.suppress -= 1;
        self.gs.cp = saved.0;
        self.gs.path.truncate(saved.1);
        result.map(|()| total)
    }
}

/// Accumulates the text objects of one `show` for standard fonts.
#[derive(Default)]
struct TextBlock {
    ops: Vec<u8>,
    res: Option<usize>,
    pending: Vec<u8>,
    needs_tm: bool,
    color: Option<crate::graphics::DevColor>,
}

impl TextBlock {
    fn glyph(&mut self, res: usize, code: u8, font_matrix: &Matrix, at: (f64, f64), ctm: &Matrix) {
        if self.res != Some(res) {
            self.flush_tj();
            self.ops.extend_from_slice(format!("/F{} 1 Tf\n", res + 1).as_bytes());
            self.res = Some(res);
            self.needs_tm = true;
        }
        if self.needs_tm || self.ops.is_empty() {
            self.flush_tj();
            let tm = Matrix::scale(1000.0, 1000.0).then(font_matrix).then(&Matrix::translate(at.0, at.1)).then(ctm);
            for v in tm.to_array() {
                put_num_prec(&mut self.ops, v, 5);
                self.ops.push(b' ');
            }
            self.ops.extend_from_slice(b"Tm\n");
            self.needs_tm = false;
        }
        self.pending.push(code);
    }

    fn reposition(&mut self) {
        self.needs_tm = true;
    }

    fn flush_tj(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        self.ops.push(b'<');
        for b in self.pending.drain(..) {
            self.ops.extend_from_slice(format!("{b:02X}").as_bytes());
        }
        self.ops.extend_from_slice(b"> Tj\n");
    }

    fn flush(&mut self, i: &mut Interp) {
        self.flush_tj();
        if self.ops.is_empty() {
            return;
        }
        if let Some(c) = self.color.or(i.gs.device) {
            i.sync_fill(c);
        }
        i.out.content.extend_from_slice(b"BT\n");
        i.out.content.append(&mut self.ops);
        i.out.content.extend_from_slice(b"ET\n");
        self.res = None;
        self.needs_tm = true;
    }
}

// ---- font dictionaries ---------------------------------------------------------

/// `key findfont`: fonts defined by the program, else a standard-font stand-in.
pub(crate) fn findfont(i: &mut Interp) -> Res {
    i.need(1)?;
    let key_value = i.arg(0).clone();
    let key = i.key_of(&key_value);
    if let Some(font) = i.font_directory.get(&key) {
        i.pop_n(1);
        return i.push(font);
    }
    let name = match key {
        Key::Name(n) => i.name_text(n),
        _ => return err("invalidfont", "findfont"),
    };
    let base = base14_for(&name);
    let dict = PsDict::new();
    let encoding = match base {
        SYMBOL => i.symbol_encoding.clone(),
        DINGBATS => i.dingbats_encoding.clone(),
        _ => i.standard_encoding.clone(),
    };
    let matrix = i.matrix_value(Matrix::scale(0.001, 0.001))?;
    let bbox = i.new_array([-200, -250, 1200, 950].map(Value::Int).to_vec())?;
    let entries = [
        (i.n.font_name, Value::Name(i.names.intern(&name))),
        (i.n.font_type, Value::Int(1)),
        (i.n.font_matrix, matrix),
        (i.n.encoding, Value::Array(encoding)),
        (i.n.font_bbox, Value::Array(bbox)),
        (i.n.paint_type, Value::Int(0)),
        (i.n.base14, Value::Int(i64::from(base))),
        (i.n.fid, Value::Null),
    ];
    for (k, v) in entries {
        i.dict_put(&dict, Key::Name(k), v)?;
    }
    i.dict_put(&i.font_directory.clone(), key, Value::Dict(dict.clone()))?;
    i.pop_n(1);
    i.push(Value::Dict(dict))
}

/// `key font definefont`.
pub(crate) fn definefont(i: &mut Interp) -> Res {
    let font = i.dict_at(0)?;
    let key_value = i.arg(1).clone();
    let key = i.key_of(&key_value);
    let font_type = font.get(&Key::Name(i.n.font_type)).and_then(|v| v.as_f64()).unwrap_or(1.0);
    if font.get(&Key::Name(i.n.font_matrix)).is_none() {
        return err("invalidfont", "definefont: no FontMatrix");
    }
    if font_type != 3.0 && font.get(&Key::Name(i.n.base14)).is_none() {
        let name = match font.get(&Key::Name(i.n.font_name)) {
            Some(Value::Name(n) | Value::ExecName(n)) => i.name_text(n),
            Some(Value::String(s)) => Rc::from(s.to_vec()),
            _ => match key {
                Key::Name(n) => i.name_text(n),
                _ => Rc::from(&b""[..]),
            },
        };
        i.dict_put(&font, Key::Name(i.n.base14), Value::Int(i64::from(base14_for(&name))))?;
    }
    if font.get(&Key::Name(i.n.encoding)).is_none() {
        let enc = i.standard_encoding.clone();
        i.dict_put(&font, Key::Name(i.n.encoding), Value::Array(enc))?;
    }
    i.dict_put(&font, Key::Name(i.n.fid), Value::Null)?;
    i.dict_put(&i.font_directory.clone(), key, Value::Dict(font.clone()))?;
    i.pop_n(2);
    i.push(Value::Dict(font))
}

fn transformed_font(i: &mut Interp, font: &PsDict, m: Matrix) -> Res<PsDict> {
    let copy = PsDict::new();
    let entries: Vec<(Key, Value)> = font.0.borrow().iter().map(|(k, v)| (*k, v.clone())).collect();
    let mut fm = Matrix::scale(0.001, 0.001);
    for (k, v) in entries {
        if k == Key::Name(i.n.font_matrix) {
            fm = i.matrix_from(&v)?;
        } else {
            i.dict_put(&copy, k, v)?;
        }
    }
    let matrix = i.matrix_value(fm.then(&m))?;
    i.dict_put(&copy, Key::Name(i.n.font_matrix), matrix)?;
    Ok(copy)
}

fn bytes_at(i: &Interp, k: usize) -> Res<Vec<u8>> {
    Ok(i.string_at(k)?.to_vec())
}

macro_rules! ops {
    ($($name:literal => $f:expr,)*) => {
        &[$(($name, $f as OpFn),)*]
    };
}

pub(crate) static OPERATORS: &[(&str, OpFn)] = ops! {
    "findfont" => findfont,
    "definefont" => definefont,
    "undefinefont" => |i: &mut Interp| {
        i.need(1)?;
        let k = i.arg(0).clone();
        let key = i.key_of(&k);
        i.pop_n(1);
        i.font_directory.0.borrow_mut().remove(&key);
        Ok(())
    },
    "composefont" => |_: &mut Interp| err("invalidfont", "composite fonts are not supported"),
    "scalefont" => |i: &mut Interp| {
        let s = i.f64_at(0)?;
        let font = i.dict_at(1)?;
        let copy = transformed_font(i, &font, Matrix::scale(s, s))?;
        i.pop_n(2);
        i.push(Value::Dict(copy))
    },
    "makefont" => |i: &mut Interp| {
        let m = i.matrix_at(0)?;
        let font = i.dict_at(1)?;
        let copy = transformed_font(i, &font, m)?;
        i.pop_n(2);
        i.push(Value::Dict(copy))
    },
    "setfont" => |i: &mut Interp| { let f = i.dict_at(0)?; i.pop_n(1); i.gs.font = Some(f); Ok(()) },
    "currentfont" => |i: &mut Interp| {
        let f = match i.gs.font.clone() { Some(f) => f, None => { i.push(Value::Name(i.n.font))?; findfont(i)?; i.dict_at(0)? } };
        if i.gs.font.is_none() { i.pop_n(1); }
        i.push(Value::Dict(f))
    },
    "rootfont" => |i: &mut Interp| {
        let f = i.gs.font.clone().map_or(Value::Null, Value::Dict);
        i.push(f)
    },
    "selectfont" => |i: &mut Interp| {
        i.need(2)?;
        let m = match i.arg(0) {
            Value::Array(_) | Value::Proc(_) => i.matrix_at(0)?,
            _ => { let s = i.f64_at(0)?; Matrix::scale(s, s) }
        };
        let key = i.arg(1).clone();
        i.pop_n(2);
        i.push(key)?;
        findfont(i)?;
        let font = i.dict_at(0)?;
        i.pop_n(1);
        let copy = transformed_font(i, &font, m)?;
        i.gs.font = Some(copy);
        Ok(())
    },
    "show" => |i: &mut Interp| {
        let s = bytes_at(i, 0)?;
        i.pop_n(1);
        i.show_text(&s, |_, _, _, w| Ok(w))
    },
    "ashow" => |i: &mut Interp| {
        let s = bytes_at(i, 0)?;
        let ay = i.f64_at(1)?;
        let ax = i.f64_at(2)?;
        i.pop_n(3);
        i.show_text(&s, |_, _, _, w| Ok((w.0 + ax, w.1 + ay)))
    },
    "widthshow" => |i: &mut Interp| {
        let s = bytes_at(i, 0)?;
        let ch = i.int_at(1)?;
        let cy = i.f64_at(2)?;
        let cx = i.f64_at(3)?;
        i.pop_n(4);
        i.show_text(&s, |_, _, c, w| Ok(if i64::from(c) == ch { (w.0 + cx, w.1 + cy) } else { w }))
    },
    "awidthshow" => |i: &mut Interp| {
        let s = bytes_at(i, 0)?;
        let ay = i.f64_at(1)?;
        let ax = i.f64_at(2)?;
        let ch = i.int_at(3)?;
        let cy = i.f64_at(4)?;
        let cx = i.f64_at(5)?;
        i.pop_n(6);
        i.show_text(&s, |_, _, c, w| {
            let (x, y) = (w.0 + ax, w.1 + ay);
            Ok(if i64::from(c) == ch { (x + cx, y + cy) } else { (x, y) })
        })
    },
    "xshow" => |i: &mut Interp| xyshow(i, 1),
    "yshow" => |i: &mut Interp| xyshow(i, 2),
    "xyshow" => |i: &mut Interp| xyshow(i, 3),
    "kshow" => |i: &mut Interp| {
        let s = bytes_at(i, 0)?;
        let proc = i.proc_at(1)?;
        i.pop_n(2);
        for (k, &code) in s.iter().enumerate() {
            i.show_text(&[code], |_, _, _, w| Ok(w))?;
            if let Some(&next) = s.get(k + 1) {
                i.push(Value::Int(i64::from(code)))?;
                i.push(Value::Int(i64::from(next)))?;
                i.call(Value::Proc(proc.clone()))?;
            }
        }
        Ok(())
    },
    "cshow" => |i: &mut Interp| {
        let s = bytes_at(i, 0)?;
        let proc = i.proc_at(1)?;
        i.pop_n(2);
        for &code in &s {
            let (wx, wy) = i.string_width(&[code])?;
            i.push(Value::Int(i64::from(code)))?;
            i.push(Value::Real(wx))?;
            i.push(Value::Real(wy))?;
            i.call(Value::Proc(proc.clone()))?;
        }
        Ok(())
    },
    "glyphshow" => |i: &mut Interp| {
        i.need(1)?;
        let glyph = match *i.arg(0) {
            Value::Name(n) | Value::ExecName(n) => n,
            Value::Int(_) => i.n.notdef,
            _ => return err("typecheck", "glyphshow"),
        };
        i.pop_n(1);
        let info = i.current_font()?;
        if i.gs.cp.is_none() { return err("nocurrentpoint", "glyphshow"); }
        let name = i.name_text(glyph);
        let code = info.encoding.as_ref().and_then(|e| {
            (0..e.len.min(256)).find(|&k| matches!(e.get(k), Value::Name(n) | Value::ExecName(n) if n == glyph))
        });
        let preferred = code.map_or_else(|| STANDARD_ENCODING.iter().position(|g| **g == *name).unwrap_or(0) as u8, |c| c as u8);
        let mut text = TextBlock::default();
        let result = i.draw_glyph(&info, preferred, glyph, &mut text);
        text.flush(i);
        let (dx, dy) = result?;
        i.advance(dx, dy)
    },
    "stringwidth" => |i: &mut Interp| {
        let s = bytes_at(i, 0)?;
        let (wx, wy) = i.string_width(&s)?;
        i.pop_n(1);
        i.push(Value::Real(wx))?;
        i.push(Value::Real(wy))
    },
    "charpath" => |i: &mut Interp| {
        i.bool_at(0)?;
        let s = bytes_at(i, 1)?;
        i.pop_n(2);
        i.charpaths.push(Vec::new());
        let result = i.show_text(&s, |_, _, _, w| Ok(w));
        let segs: Vec<Seg> = i.charpaths.pop().unwrap_or_default();
        result?;
        let cp = i.gs.cp;
        extend_path(i, segs, cp)
    },
    "setcachedevice" => |i: &mut Interp| {
        let wy = i.f64_at(4)?;
        let wx = i.f64_at(5)?;
        i.pop_n(6);
        if let Some(g) = i.glyph.as_mut() { g.width = Some((wx, wy)); }
        Ok(())
    },
    "setcachedevice2" => |i: &mut Interp| {
        let wy = i.f64_at(8)?;
        let wx = i.f64_at(9)?;
        i.pop_n(10);
        if let Some(g) = i.glyph.as_mut() { g.width = Some((wx, wy)); }
        Ok(())
    },
    "setcharwidth" => |i: &mut Interp| {
        let wy = i.f64_at(0)?;
        let wx = i.f64_at(1)?;
        i.pop_n(2);
        if let Some(g) = i.glyph.as_mut() { g.width = Some((wx, wy)); }
        Ok(())
    },
    "setcachelimit" => |i: &mut Interp| { i.need(1)?; i.pop_n(1); Ok(()) },
    "setcacheparams" => |i: &mut Interp| {
        let n = i.ostack.iter().rev().position(|v| matches!(v, Value::Mark)).map_or(0, |p| p + 1);
        i.pop_n(n);
        Ok(())
    },
    "currentcacheparams" => |i: &mut Interp| { i.push(Value::Mark)?; i.push(Value::Int(0))?; i.push(Value::Int(0)) },
};

/// `xshow`/`yshow`/`xyshow`: explicit per-glyph displacements.
fn xyshow(i: &mut Interp, which: u8) -> Res {
    let arr = i.array_at(0)?;
    let nums = i.numbers(&arr)?;
    let s = bytes_at(i, 1)?;
    i.pop_n(2);
    let per = if which == 3 { 2 } else { 1 };
    if nums.len() < s.len() * per {
        return err("rangecheck", "not enough displacements");
    }
    i.show_text(&s, |_, k, _, _| {
        Ok(match which {
            1 => (nums[k], 0.0),
            2 => (0.0, nums[k]),
            _ => (nums[2 * k], nums[2 * k + 1]),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_names_map_to_the_closest_standard_font() {
        let name = |s: &str| BASE14_NAMES[usize::from(base14_for(s.as_bytes()))];
        assert_eq!(name("Helvetica-Bold"), "Helvetica-Bold");
        assert_eq!(name("ArialMT"), "Helvetica");
        assert_eq!(name("Arial-BoldItalicMT"), "Helvetica-BoldOblique");
        assert_eq!(name("TimesNewRomanPSMT"), "Times-Roman");
        assert_eq!(name("DejaVuSerif-Italic"), "Times-Italic");
        assert_eq!(name("DejaVuSans"), "Helvetica");
        assert_eq!(name("CourierNewPS-BoldMT"), "Courier-Bold");
        assert_eq!(name("Symbol"), "Symbol");
    }

    #[test]
    fn standard_font_widths_come_from_the_afm_metrics() {
        assert_eq!(base14_width(0, b"A"), 667.0);
        assert_eq!(base14_width(4, b"space"), 250.0);
        assert_eq!(base14_width(8, b"W"), 600.0);
        assert_eq!(base14_width(SYMBOL, b"alpha"), 631.0);
        assert_eq!(base14_width(0, b"no-such-glyph"), 0.0);
    }
}
