//! PDF page rendering: traverses box trees into content streams, tracks
//! used fonts, emits rules, raw literals, colors, leaders, link
//! annotations, named destinations, and \pdfsavepos position recording.

use crate::boxes::{leader_dims, LeaderBody, Node, NodeList, HBOX};
use crate::build::RULE_FILL;
use crate::engine::Engine;
use crate::pdfout::{Annot, PdfPage};
use crate::prim::{DimParam, IntParam};

pub(crate) mod dpx;
mod dpx_doc;
mod dpx_page;
mod dpx_text;
mod lr;
mod lua_glyph;
pub(crate) use lua_glyph::with_vf_packet;

/// TeX sp to PDF bp
#[inline]
pub fn sp_to_bp(sp: i64) -> f64 {
    sp as f64 * 72.0 / (72.27 * 65536.0)
}

/// PDF bp to TeX sp
#[inline]
pub fn bp_to_sp(bp: f64) -> i32 {
    (bp * 72.27 * 65536.0 / 72.0).round() as i32
}

const ONE_HUNDRED_BP_SP: i64 = 6_578_176;

/// scaled points per bp (`sp_per_bp`): 72.27 * 65536 / 72.
pub(crate) const SP_PER_BP: f64 = 72.27 * 65536.0 / 72.0;
/// pdfTeX's `divide_scaled`: return the rounded decimal value and the
/// corresponding displacement on TeX's scaled-point raster.
pub(crate) fn divide_scaled(mut s: i64, mut m: i64, decimal_digits: u32) -> (i64, i64) {
    if m == 0 {
        return (0, 0);
    }
    let mut sign = 1;
    if s < 0 {
        sign = -sign;
        s = -s;
    }
    if m < 0 {
        sign = -sign;
        m = -m;
    }

    let ten_pow = 10_i64.pow(decimal_digits);
    let mut quotient = s / m;
    let mut remainder = s % m;
    for _ in 0..decimal_digits {
        quotient = 10 * quotient + (10 * remainder) / m;
        remainder = (10 * remainder) % m;
    }
    if 2 * remainder >= m {
        quotient += 1;
        remainder -= m;
    }

    (sign * quotient, sign * (s - remainder / ten_pow))
}

fn push_decimal(buf: &mut String, mut value: i64, decimal_digits: u32) {
    if value < 0 {
        buf.push('-');
        value = -value;
    }
    let ten_pow = 10_i64.pow(decimal_digits);
    push_i64(buf, value / ten_pow);
    let mut fraction = value % ten_pow;
    if fraction == 0 {
        return;
    }
    let mut width = decimal_digits as usize;
    while fraction % 10 == 0 {
        fraction /= 10;
        width -= 1;
    }
    buf.push('.');
    use std::fmt::Write;
    let _ = write!(buf, "{fraction:0width$}");
}

/// pdfTeX `pdf_print_bp`: print `sp` as bp with `digits` decimals (pdfTeX
/// `fixed_decimal_digits`; trailing zeros trimmed) and return the
/// corresponding displacement on the sp raster (`scaled_out`), exactly as
/// `divide_scaled(s, one_hundred_bp, digits + 2)` does.
#[inline]
pub(crate) fn push_bp_sp(buf: &mut String, sp: i64, digits: u32) -> i64 {
    let (value, out) = divide_scaled(sp, ONE_HUNDRED_BP_SP, digits + 2);
    push_decimal(buf, value, digits);
    out
}

/// pdfTeX /Widths entry in tenths of a glyph-space unit: `divide_scaled(w,
/// pdf_font_size[f], 4)` (writefont.c `create_charwidth_array`), where
/// `pdf_font_size` is the at size snapped to 6 decimals of bp
/// (`pdf_use_font`). Dividing by the raw at size rounds some widths off.
pub(crate) fn pdf_width_tenths(width: i32, at_size: i32) -> i32 {
    if at_size == 0 {
        return 0;
    }
    divide_scaled(width as i64, pdf_font_size(at_size), 4).0 as i32
}

/// pdfTeX `pdf_font_size[f]` (`pdf_use_font`): the at size snapped to 6
/// decimals of bp.
pub(crate) fn pdf_font_size(at_size: i32) -> i64 {
    divide_scaled(at_size as i64, ONE_HUNDRED_BP_SP, 6).1
}

/// pdfTeX `round_xn_over_d` in i128-safe form: round `x * n / d` half-up on
/// the magnitude, sign restored.
#[inline]
pub(crate) fn round_xn_over_d(x: i64, n: i64, d: i64) -> i64 {
    let neg = (x < 0) ^ (n < 0);
    let mut a = x as i128 * n as i128;
    if a < 0 {
        a = -a;
    }
    let dd = if d <= 0 { 1 } else { d as i128 };
    let mut q = a / dd;
    let r = a % dd;
    if 2 * r >= dd {
        q += 1;
    }
    if neg {
        -(q as i64)
    } else {
        q as i64
    }
}

/// pdfTeX `ext_xn_over_d` (utils.c): `x*n/d` in double precision, rounded
/// half away from zero and truncated to an integer.
pub(crate) fn ext_xn_over_d(x: i64, n: i64, d: i64) -> i32 {
    let mut r = (x as f64 * n as f64) / d as f64;
    if r > f64::EPSILON {
        r += 0.5;
    } else {
        r -= 0.5;
    }
    r as i32
}

/// pdfTeX `max_dimen`.
const MAX_DIMEN: i64 = 0x3FFF_FFFF;

/// pdfTeX `gap_amount`: the move from `cur_pos` to the nearest point of the
/// \pdfsnapy grid through `refpos` that the snap glue's stretch (forward)
/// or shrink (backward) can reach; 0 when neither can.
fn gap_amount(glue: &crate::boxes::Glue, cur_pos: i64, refpos: i64) -> i64 {
    let unit = glue.width as i64;
    if unit == 0 {
        return 0;
    }
    let stretch = if glue.stretch_order > 0 { MAX_DIMEN } else { glue.stretch as i64 };
    let shrink = if glue.shrink_order > 0 { MAX_DIMEN } else { glue.shrink as i64 };
    let last = refpos + unit * ((cur_pos - refpos) / unit);
    let next = last + unit;
    let back = if cur_pos - last < shrink { cur_pos - last } else { MAX_DIMEN };
    let forward = if next - cur_pos < stretch { next - cur_pos } else { MAX_DIMEN };
    if back == MAX_DIMEN && forward == MAX_DIMEN {
        0
    } else if forward <= back {
        forward
    } else {
        -back
    }
}

/// pdfTeX `get_vpos`: the vertical position reached after `nodes` starting
/// at `cur_v`, moving like `vlist_out` with a fresh glue rounding state.
fn get_vpos(nodes: &[Node], cur_v: i64, sign: u8, order: u8, set: f64) -> i64 {
    let mut v = cur_v;
    let mut glue_state = GlueState::default();
    for node in nodes {
        v += match node {
            Node::Box { h, d, .. } => (*h + *d) as i64,
            Node::Rule { height, depth, .. } => (*height + *depth) as i64,
            Node::NativeGlyphRun { height, depth, .. } => (*height + *depth) as i64,
            Node::Whatsit(
                crate::boxes::WhatIt::PdfRefXImage { h, d, .. }
                | crate::boxes::WhatIt::PdfRefXForm { h, d, .. }
                | crate::boxes::WhatIt::XePic { h, d, .. },
            _) => (*h + *d) as i64,
            Node::Glue(g, _) | Node::Leaders { glue: g, .. } => glue_state.advance(g, sign, order, set),
            Node::Kern(k, _) | Node::ExplicitKern(k, _) | Node::AccentKern(k, _) | Node::ItalicKern(k, _) | Node::SpaceAdjKern(k, _) => *k as i64,
            Node::ExKern { width, ex, .. } => (*width + *ex) as i64,
            _ => 0,
        };
    }
    v
}

/// pdfTeX `pdf_print_bp` for a bp coordinate: quantize to sp, then print
/// with `divide_scaled(s, one_hundred_bp, digits+2)` / `pdf_print_real(.., digits)`.
#[inline]
fn push_print_bp(buf: &mut String, bp: f64, digits: u32) {
    let sp_per_bp = 72.27 * 65536.0 / 72.0;
    push_bp_sp(buf, (bp * sp_per_bp).round() as i64, digits);
}

/// pdfTeX's minimum move threshold `min_bp_val`: `divide_scaled(one_hundred_bp,
/// 10^(fixed_decimal_digits+2), 0)` (66 sp with the default 3 decimals).
fn min_bp_val(digits: u32) -> i64 {
    divide_scaled(ONE_HUNDRED_BP_SP, 10_i64.pow(digits + 2), 0).0
}
/// pdfTeX's TJ-continuation threshold `@'100000` (1/1000 em units).
const GAP_SPLIT_LIMIT: i64 = 32768;
/// pdfTeX `matrix_entry` (utils.c §1278): an accumulated page CTM.
#[derive(Clone, Copy)]
struct Matrix {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

/// pdfTeX `pos_entry` (utils.c §1296): the pen at `\pdfsave` time and the
/// matrix depth to unwind to at the matching `\pdfrestore`.
struct SavePoint {
    pos_h: i64,
    pos_v: i64,
    matrix_depth: usize,
    source: Option<crate::input::SourceMark>,
}

/// pdfTeX `DO_ROUND` (utils.c §1485): round half away from zero onto the sp raster.
#[inline]
fn do_round(x: f64) -> i64 {
    if x > 0.0 {
        (x + 0.5) as i64
    } else {
        (x - 0.5) as i64
    }
}

/// pdfTeX `pdfsetmatrix` parsing (utils.c §1415: `sscanf(" %lf %lf %lf %lf %c")`
/// must yield exactly 4 numbers). Stricter than pdfTeX, which then echoes the
/// raw token string (an input with more than four numbers therefore silently
/// corrupts the content stream there); we require exactly four finite numbers
/// and echo the string only when it is plain PDF number syntax, so a
/// malformed matrix can never reach the PDF.
fn parse_matrix(s: &str) -> Option<[f64; 4]> {
    let mut it = s.split_ascii_whitespace();
    let mut v = [0.0f64; 4];
    for slot in v.iter_mut() {
        let t = it.next()?;
        let n: f64 = t.parse().ok()?;
        if !n.is_finite() {
            return None;
        }
        *slot = n;
    }
    if it.next().is_some() {
        return None;
    }
    Some(v)
}
/// PDF real/integer syntax (ISO 32000 §7.3.3): optional sign, digits with at
/// most one `.`, at least one digit; no exponent.
fn is_pdf_number(t: &str) -> bool {
    let t = t.strip_prefix(['+', '-']).unwrap_or(t);
    let mut digits = 0;
    let mut dots = 0;
    for b in t.bytes() {
        match b {
            b'0'..=b'9' => digits += 1,
            b'.' => dots += 1,
            _ => return false,
        }
    }
    digits > 0 && dots <= 1
}
/// Print a parsed `\pdfsetmatrix` component that is not plain PDF number
/// syntax. Matrix entries are dimensionless numbers (not bp dimensions), so
/// they are never re-quantized through the sp raster: integral values print
/// bare, others use Rust's shortest round-tripping decimal form.
fn push_matrix_num(buf: &mut String, v: f64) {
    use std::fmt::Write;
    if v.fract() == 0.0 && v.abs() < (i64::MAX as f64) {
        let _ = write!(buf, "{}", v as i64);
    } else {
        let _ = write!(buf, "{}", v);
    }
}

/// pdfTeX `do_matrixtransform` (utils.c §1489): `(x y 1) × M`, each
/// component rounded back onto the sp raster half-away-from-zero.
#[inline]
fn matrix_transform_point(m: &Matrix, x: f64, y: f64) -> (f64, f64) {
    (
        do_round(x * m.a + y * m.c + m.e) as f64,
        do_round(x * m.b + y * m.d + m.f) as f64,
    )
}

/// pdfTeX `matrixtransformrect` (utils.c §1500): transform all four corners
/// and return the axis-aligned bounding box.
fn matrix_transform_rect(m: &Matrix, llx: f64, lly: f64, urx: f64, ury: f64) -> [f64; 4] {
    let (x1, y1) = matrix_transform_point(m, llx, lly);
    let (x2, y2) = matrix_transform_point(m, llx, ury);
    let (x3, y3) = matrix_transform_point(m, urx, lly);
    let (x4, y4) = matrix_transform_point(m, urx, ury);
    [
        x1.min(x2).min(x3).min(x4),
        y1.min(y2).min(y3).min(y4),
        x1.max(x2).max(x3).max(x4),
        y1.max(y2).max(y3).max(y4),
    ]
}

pub struct RenderCtx<'a> {
    pub eng: &'a mut Engine,
    pub content: String,
    pub used_fonts: Vec<(usize, u32)>, // (engine font/binding key, PDF resource number)
    pub page_height_bp: f64,
    pub cur_font: usize,
    pub cur_pdf_font: u32,
    /// pdfTeX `fixed_decimal_digits` and the derived `min_bp_val`.
    decimal_digits: u32,
    min_bp_val: i64,
    /// pdfTeX `cur_s`: box nesting depth during shipout (-1 outside the
    /// shipped box); running links continue only into boxes at their level.
    cur_s: i32,
    /// baseline of the hlist being shipped (`base_line` in `hlist_out`)
    base_line_sp: i64,
    pub annots: Vec<Annot>,
    pub dests: Vec<crate::pdfout::Dest>,
    pub page_fonts: Vec<(usize, u32)>, // (engine font/binding key, resource number)
    // containing-box context for leaders grids and null-rule sentinels (sp)
    pub left_edge_sp: i64,
    /// reference point (h, baseline v) of the shipped box: the origin
    /// `\gleaders` align to (pdf backend `shipbox_refpos`)
    ship_ref_sp: (i64, i64),
    pub box_w_sp: i64,
    pub box_h_sp: i64,
    pub box_d_sp: i64,
    /// e-TeX `box_lr` of the box about to be shipped (set with box_w_sp)
    box_lr: u8,
    /// etex.ch `cur_dir`: 1 while shipping right-to-left (reflected) text
    cur_dir: u8,
    /// etex.ch `LR_problems` accumulated during this ship_out
    lr_problems: i32,
    // pdfTeX canonical text-object state (pdftex.web §16237+): one persistent
    // BT..ET per text section with relative Td / scaled Tm positioning, all
    // deltas accumulated on the integer sp raster exactly as pdfTeX does.
    doing_text: bool,
    doing_string: bool,
    doing_hex_string: bool,
    advance_cache: std::collections::HashMap<(u16, u32), i64>,
    font_programs: std::collections::HashMap<u16, std::rc::Rc<crate::font_program::FontProgram>>,
    cur_tm_a: i32,
    pdf_f: u16,
    last_f: u32,
    last_f_size: i64,
    pdf_h: i64,
    pdf_v: i64,
    tj_start_h: i64,
    delta_h: i64,
    origin_h: i64,
    origin_v: i64,
    /// luatex `pdf.h.m`/`pdf.v.m`: where the current `cm` origin is, in units
    /// of the last output digit from the page's bottom left
    lua_cm: (i64, i64),
    /// `callback_defined(process_rule)` when the shipout started: 0 none,
    /// positive a function, -1 registered as `false`
    process_rule_cb: i8,
    page_height_sp: i64,
    scaled_out: i64,
    // pdfTeX `matrix_stack` + `pos_stack` (utils.c §1276-1303): CTMs
    // accumulated by \pdfsetmatrix during this shipout, and the pen
    // positions recorded by \pdfsave waiting for a matching \pdfrestore.
    // `page_mode` mirrors pdfTeX's `page_mode`: matrix tracking applies to
    // page shipout only; save/restore bookkeeping applies to both.
    page_mode: bool,
    matrix_stack: Vec<Matrix>,
    pos_stack: Vec<SavePoint>,
    pub display_list: crate::boxes::DisplayList,
    cjk_text: Option<char>,
    /// Virtual font of each VF-backed engine font used on this page.
    vf_fonts: crate::FxHashMap<u16, Option<std::rc::Rc<crate::fontload::VfFont>>>,
    /// pdfTeX `pdf_ximage_list`: images painted here, in first-use order.
    pub ximage_list: Vec<i32>,
    /// pdfTeX `pdf_xform_list`: forms painted here, in first-use order.
    pub xform_list: Vec<i32>,
    /// XeTeX: page-local state of the xdvipdfmx special interpreter.
    dpx: dpx::DpxPage,
    /// XeTeX: the pdfdev.c text state of native glyph runs.
    dpxt: dpx_text::DpxText,
}

/// A shipped \pdfxform box: its content stream and resources.
pub struct RenderedForm {
    pub content: Vec<u8>,
    pub fonts: Vec<(usize, u32)>,
    /// pdfTeX `pdf_image_procset` of the form
    pub image_procset: u8,
    /// pdfTeX `pdf_xform_list` of the form
    pub xforms: Vec<i32>,
    /// pdfTeX `pdf_ximage_list` of the form
    pub ximages: Vec<i32>,
}

#[inline]
fn push_i64(s: &mut String, mut v: i64) {
    if v == 0 {
        s.push('0');
        return;
    }
    if v < 0 {
        s.push('-');
        v = -v;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while v > 0 {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        s.push(buf[i] as char);
    }
}

/// pdfTeX `pdf_print_char`: bytes <= 32, `(`, `)`, `\` and bytes > 127 are
/// written as three-digit octal escapes (`pdf_print_octal`), all others raw.
#[inline]
fn push_pdf_char(s: &mut String, b: u8) {
    if b <= 32 || b == b'(' || b == b')' || b == b'\\' || b > 127 {
        s.push('\\');
        s.push((b'0' + (b >> 6)) as char);
        s.push((b'0' + ((b >> 3) & 7)) as char);
        s.push((b'0' + (b & 7)) as char);
    } else {
        s.push(b as char);
    }
}

#[cfg(test)]
mod text_encoding_tests {
    use super::push_pdf_char;

    #[test]
    fn literal_text_round_trips_all_font_bytes_and_kerning() {
        let mut text = String::from("[(");
        let mut expected = Vec::new();
        for byte in 0..=255 {
            push_pdf_char(&mut text, byte);
            push_pdf_char(&mut text, b'7');
            expected.extend([byte, b'7']);
        }
        text.push_str(")-20(A)30(B)] TJ");
        let content = lopdf::content::Content::decode(text.as_bytes()).unwrap();
        let operands = content.operations[0].operands[0].as_array().unwrap();
        assert_eq!(operands[0].as_str().unwrap(), expected);
        assert_eq!(operands[1].as_i64().unwrap(), -20);
        assert_eq!(operands[2].as_str().unwrap(), b"A");
        assert_eq!(operands[3].as_i64().unwrap(), 30);
        assert_eq!(operands[4].as_str().unwrap(), b"B");
    }

    /// pdflatex writes `(`, `)`, `\`, space and high bytes as octal, DEL raw.
    #[test]
    fn literal_text_escapes_like_pdf_print_char() {
        let mut text = String::new();
        for byte in [b'(', b')', b'\\', b' ', b'!', 127, 200, 12] {
            push_pdf_char(&mut text, byte);
        }
        assert_eq!(text, "\\050\\051\\134\\040!\x7f\\310\\014");
    }

    /// /Widths divide by pdfTeX's snapped `pdf_font_size`, not the raw at
    /// size: cmmi10 `G` (515276sp at 10pt) is 786.3 in pdflatex output,
    /// while 515276/655360 alone rounds to 786.2; SFTI1000 `M` likewise.
    #[test]
    fn widths_use_pdftex_snapped_font_size() {
        assert_eq!(super::pdf_width_tenths(515_276, 655_360), 7863);
        assert_eq!(super::pdf_width_tenths(327_680, 655_360), 5000);
        assert_eq!(super::pdf_width_tenths(1, 0), 0);
    }
}
/// pdfTeX `pdf_print_bp` for a bp value with `digits` decimals.
fn push_pdfnum(buf: &mut String, v: f64, digits: u32) {
    push_print_bp(buf, v, digits);
}

fn pdfnum(v: f64, digits: u32) -> String {
    let mut s = String::with_capacity(16);
    push_pdfnum(&mut s, v, digits);
    s
}

impl Engine {
    /// Fresh emitter context: no text object open, origin at the page
    /// bottom-left (pdfTeX `pdf_origin_h := 0; pdf_origin_v :=
    /// cur_page_height`, set in `pdf_ship_out`).
    fn new_ctx(&mut self, page_height_sp: i64) -> RenderCtx<'_> {
        let decimal_digits = self.pdf_doc.decimal_digits;
        // luatex ship_out reads `callback_defined(process_rule)` once per shipout
        let process_rule_cb = if self.engine_kind == crate::engine::EngineKind::LuaTeX {
            self.cb_state(crate::lua_callbacks::Cb::ProcessRule)
        } else {
            0
        };
        RenderCtx {
            eng: self,
            process_rule_cb,
            content: String::new(),
            used_fonts: Vec::new(),
            page_height_bp: sp_to_bp(page_height_sp),
            cur_font: 0,
            cur_pdf_font: 0,
            decimal_digits,
            min_bp_val: min_bp_val(decimal_digits),
            cur_s: -1,
            base_line_sp: 0,
            annots: Vec::new(),
            dests: Vec::new(),
            page_fonts: Vec::new(),
            left_edge_sp: 0,
            ship_ref_sp: (0, 0),
            box_w_sp: 0,
            box_h_sp: 0,
            box_d_sp: 0,
            box_lr: 0,
            cur_dir: 0,
            lr_problems: 0,
            doing_text: false,
            doing_string: false,
            doing_hex_string: false,
            advance_cache: std::collections::HashMap::new(),
            font_programs: std::collections::HashMap::new(),
            cur_tm_a: 0,
            pdf_f: 0,
            last_f: 0,
            last_f_size: 0,
            pdf_h: 0,
            pdf_v: page_height_sp,
            tj_start_h: 0,
            delta_h: 0,
            origin_h: 0,
            origin_v: page_height_sp,
            lua_cm: (0, 0),
            page_height_sp,
            scaled_out: 0,
            page_mode: true,
            matrix_stack: Vec::new(),
            pos_stack: Vec::new(),
            display_list: crate::boxes::DisplayList::new(),
            cjk_text: None,
            vf_fonts: crate::FxHashMap::default(),
            ximage_list: Vec::new(),
            xform_list: Vec::new(),
            dpx: dpx::DpxPage::new(),
            dpxt: dpx_text::DpxText::new(),
        }
    }

    /// tex.web `prepare_mag` (§288): the first use freezes \mag for the
    /// job; a later different value is an error and reverts (globally) to
    /// the frozen one, an out-of-range value is replaced by 1000.
    pub fn prepare_mag(&mut self) -> i32 {
        let mut mag = self.eqtb.int_params[IntParam::Mag.idx() as usize];
        let mag_set = self.pdf_doc.mag;
        if mag_set > 0 && mag != mag_set {
            self.error(&format!(
                "Incompatible magnification ({mag}); the previous value will be retained ({mag_set})"
            ));
            mag = mag_set;
            self.eqtb.assign_int_param(IntParam::Mag, mag, true);
        }
        if !(1..=32768).contains(&mag) {
            self.error(&format!("Illegal magnification has been changed to 1000 ({mag})"));
            mag = 1000;
            self.eqtb.assign_int_param(IntParam::Mag, mag, true);
        }
        self.pdf_doc.mag = mag;
        mag
    }

    /// Render a shipped page box into a PdfPage. Also records
    /// \pdfsavepos results (\pdflastxpos/\pdflastypos) from the last
    /// SavePos node on the page.
    pub fn render_page(&mut self, page_box: &Node) -> PdfPage {
        if self.engine_kind == crate::engine::EngineKind::XeTeX {
            return self.render_page_xetex(page_box);
        }
        // pdf_ship_out initializes the PDF output on the first page or form.
        self.init_pdf_output();
        // pdfTeX "Calculate page dimensions and margins": a zero
        // \pdfpagewidth/\pdfpageheight means box size plus twice the offset
        let dim = |p: DimParam| self.eqtb.dim_params[p.idx() as usize];
        let (bw, bht) = match page_box {
            Node::Box { w, h, d, .. } => (*w, h.saturating_add(*d)),
            _ => (0, 0),
        };
        let mut width_sp = dim(DimParam::PdfPageWidth);
        if width_sp == 0 {
            width_sp = bw + 2 * (dim(DimParam::PdfHOrigin) + dim(DimParam::HOffset));
        }
        let mut height_sp = dim(DimParam::PdfPageHeight);
        if height_sp == 0 {
            height_sp = bht + 2 * (dim(DimParam::PdfVOrigin) + dim(DimParam::VOffset));
        }
        let w_bp = sp_to_bp(width_sp as i64);
        let h_bp = sp_to_bp(height_sp as i64);
        if self.synctex_active() {
            let page = (self.pdf_doc.pages.len() + 1) as u32;
            self.synctex
                .record_page_size(page, width_sp as i64, height_sp as i64);
        }
        let mag = self.prepare_mag();
        self.pdf_page_group_val = 0;
        let mut ctx = self.new_ctx(height_sp as i64);
        // pdfTeX "Adjust transformation matrix for the magnification ratio":
        // the page stream opens with `m 0 0 m 0 0 cm`, m = mag/1000
        if mag != 1000 {
            push_decimal(&mut ctx.content, mag as i64, 3);
            ctx.content.push_str(" 0 0 ");
            push_decimal(&mut ctx.content, mag as i64, 3);
            ctx.content.push_str(" 0 0 cm\n");
        }
        // ship_out's this_box is the shipped box: running rule dimensions
        // at its top level take its width/height/depth (§624, §633)
        if let Node::Box { w, h, d, lr, .. } = page_box {
            (ctx.box_w_sp, ctx.box_h_sp, ctx.box_d_sp) = (*w as i64, *h as i64, *d as i64);
            ctx.box_lr = *lr;
        }
        let x0 = ctx.eng.eqtb.dim_params[DimParam::PdfHOrigin.idx() as usize] as i64
            + ctx.eng.eqtb.dim_params[DimParam::HOffset.idx() as usize] as i64;
        let y0 = ctx.eng.eqtb.dim_params[DimParam::PdfVOrigin.idx() as usize] as i64
            + ctx.eng.eqtb.dim_params[DimParam::VOffset.idx() as usize] as i64;
        // pdfTeX "Start stream of page/form contents": cur_h/cur_v are the
        // offsets and the box height when the stacks re-emit their colors
        ctx.colorstack_startpage(x0, y0 + ctx.box_h_sp);
        ctx.ship_ref_sp = (x0, y0 + ctx.box_h_sp);
        if let Node::Box {
            list,
            kind,
            h,
            glue_sign,
            glue_order,
            glue_set,
            ..
        } = page_box
        {
            if *kind == HBOX {
                // pdftex pdf_ship_out: cur_v := height(p), so the shipped
                // hbox's baseline sits one box height below the origin
                ctx.ship_hlist(list, x0, y0 + *h as i64, *glue_sign, *glue_order, *glue_set);
            } else {
                // vbox/vtop: the top of the page material sits at the origin
                ctx.ship_vlist(list, x0, y0, *glue_sign, *glue_order, *glue_set);
            }
        }
        // pdfTeX's link stack outlives the page; annotation indices do not
        for link in ctx.eng.pdf_doc.link_stack.iter_mut() {
            link.annot = None;
        }
        // etex.ch "Check for LR anomalies at the end of ship_out"
        if ctx.lr_problems > 0 {
            let problems = std::mem::take(&mut ctx.lr_problems);
            ctx.eng.report_lr_problems(problems, None);
        }
        // pdfTeX `pdfshipoutend` (utils.c §1367): a save left unmatched at
        // the end of the shipout is fatal (no output file).
        if let Some(save) = ctx.pos_stack.last() {
            let count = ctx.pos_stack.len();
            let message = if count == 1 {
                "Unmatched \\pdfsave: the shipped page ended before a matching \\pdfrestore"
                    .to_string()
            } else {
                format!("Unmatched \\pdfsave: the shipped page ended with {count} saves still open")
            };
            ctx.eng.fatal_error_at(
                &message,
                save.source
                    .as_ref()
                    .map(crate::input::SourceMark::to_context),
            );
        }
        // engine-level results
        ctx.eng.pdf_doc.pages_attr = ctx.eng.pdf_pages_attr.clone().into_bytes();
        // "Write out page object" takes the page group before the pending
        // images are written (a PDF page's group object resets it).
        let group = ctx.eng.pdf_page_group_val;
        ctx.write_pending_images();
        let image_procset = ctx.image_procset();
        let omit_procset =
            ctx.eng.eqtb.int_params[crate::prim::IntParam::PdfOmitProcset.idx() as usize];
        PdfPage {
            content: {
                ctx.end_text();
                std::mem::take(&mut ctx.content).into_bytes()
            },
            width: w_bp.round() as i32,
            height: h_bp.round() as i32,
            width_sp: width_sp as i64,
            height_sp: height_sp as i64,
            annots: std::mem::take(&mut ctx.annots),
            annot_refs: std::mem::take(&mut ctx.eng.lua_tex.late_annots),
            fonts: std::mem::take(&mut ctx.page_fonts),
            dests: std::mem::take(&mut ctx.dests),
            attr_extra: ctx.eng.pdf_page_attr.as_bytes().to_vec(),
            resources_extra: ctx.eng.pdf_page_resources.clone(),
            display_list: Some(std::mem::take(&mut ctx.display_list)),
            // "Generate ProcSet if desired": forced by \pdfomitprocset < 0,
            // omitted by > 0, by default only for PDF 1.x
            procset: omit_procset < 0 || (omit_procset == 0 && ctx.eng.pdf_doc.major_version < 2),
            image_procset,
            xforms: std::mem::take(&mut ctx.xform_list),
            ximages: std::mem::take(&mut ctx.ximage_list),
            group,
            media_box: ctx.eng.eqtb.int_params[crate::prim::IntParam::PdfOmitMediaBox.idx() as usize] == 0,
        }
    }

    pub fn render_form_box(&mut self, node: &Node, w: i32, h: i32, d: i32) -> RenderedForm {
        // pdf_ship_out initializes the PDF output on the first page or form.
        self.init_pdf_output();
        // pdf_ship_out for a form sets cur_page_height to height + depth:
        // form coordinates start at the bottom of the box, so the baseline
        // sits at y = depth and the dictionary spans [0, h + d]. out_form
        // lowers the placement by the same depth.
        // pdf_ship_out resets the page group for forms too.
        self.pdf_page_group_val = 0;
        let mut ctx = self.new_ctx(h as i64 + d as i64);
        // pdfTeX `pdfshipoutbegin(false)` for forms: matrix/annotation
        // tracking is page-shipout only, and color stacks restart from
        // their initial values (`colorstackpagestart`).
        ctx.page_mode = false;
        ctx.eng.color_stacks.form_start();
        ctx.box_w_sp = w as i64;
        ctx.box_h_sp = h as i64;
        ctx.box_d_sp = d as i64;
        ctx.ship_ref_sp = (0, h as i64);
        ctx.ship_vlist(&vec![node.clone()], 0, 0, 0, 0, 0.0);
        ctx.end_text();
        if let Some(save) = ctx.pos_stack.last() {
            let count = ctx.pos_stack.len();
            let message = if count == 1 {
                "Unmatched \\pdfsave: the shipped form ended before a matching \\pdfrestore"
                    .to_string()
            } else {
                format!("Unmatched \\pdfsave: the shipped form ended with {count} saves still open")
            };
            ctx.eng.fatal_error_at(
                &message,
                save.source
                    .as_ref()
                    .map(crate::input::SourceMark::to_context),
            );
        }
        ctx.write_pending_images();
        RenderedForm {
            image_procset: ctx.image_procset(),
            content: ctx.content.into_bytes(),
            fonts: ctx.page_fonts,
            xforms: ctx.xform_list,
            ximages: ctx.ximage_list,
        }
    }
}

impl Engine {
    /// pdfTeX `pdf_write_image` for an image not yet written: an included
    /// PDF page's form (its /Group refers to the page-group object, written
    /// here once per page, or is copied inline), or the transparency group
    /// shared by PNGs with alpha. Raster samples are embedded at the end.
    pub(crate) fn write_ximage(&mut self, obj: i32) {
        use crate::engine::ImageKind;
        let Some(image) = self.pdf_images.get_mut(&obj) else {
            return;
        };
        if image.written {
            return;
        }
        image.written = true;
        image.used = true;
        match image.kind {
            ImageKind::Pdf => {
                let Some(form) = image.pdf_form.take() else {
                    return;
                };
                let attr = image.attr.take();
                let path = image.path.clone();
                let group = match form.group_dict() {
                    Some(_) if self.pdf_page_group_val == 0 => {
                        let suppress = self.eqtb.int_params
                            [crate::prim::IntParam::PdfSuppressWarningPageGroup.idx() as usize];
                        if suppress == 0 {
                            self.warning_at(
                                &format!("pdfTeX warning (file {path}): PDF inclusion: multiple pdfs with page group included in a single page"),
                                None,
                            );
                        }
                        None
                    }
                    Some(dict) => {
                        let group = std::mem::take(&mut self.pdf_page_group_val);
                        self.pdf_doc.objects.push((group, dict.to_vec()));
                        Some(group)
                    }
                    None => None,
                };
                let bytes = form.write(attr.as_deref(), group);
                self.pdf_doc.objects.push((obj, bytes));
            }
            // writepng.c `write_additional_png_objects`
            ImageKind::Png
                if image.group_ref > 0
                    && self.transparent_page_group > 0
                    && !self.transparent_page_group_written =>
            {
                self.transparent_page_group_written = true;
                self.pdf_doc.objects.push((
                    self.transparent_page_group,
                    b"<</Type/Group /S/Transparency /CS/DeviceRGB /I true>>".to_vec(),
                ));
            }
            _ => {}
        }
    }

    /// A \pdfxform's /ProcSet depends on \pdfomitprocset when pdfTeX writes
    /// the form (with the first page using it, or at \immediate): insert it
    /// into the form's resources then.
    pub(crate) fn write_form_procset(&mut self, obj: i32) {
        let Some(pending) = self.pdf_form_procsets.remove(&obj) else {
            return;
        };
        let omit = self.eqtb.int_params[crate::prim::IntParam::PdfOmitProcset.idx() as usize];
        if !(omit < 0 || (omit == 0 && self.pdf_doc.major_version < 2)) {
            return;
        }
        let entry = crate::pdffile::procset_entry(pending.text, pending.images);
        if let Some((_, bytes)) = self.pdf_doc.objects.iter_mut().rev().find(|(n, _)| *n == obj) {
            if pending.offset <= bytes.len() {
                bytes.splice(pending.offset..pending.offset, entry.into_bytes());
            }
        }
    }
}

/// Glue placement inside one shipped box (tex.web §625 hlist_out, §634
/// vlist_out): the stretch or shrink of matching-order glue accumulates in
/// `cur_glue` and each glue advances by its width plus the change in the
/// ROUNDED running total, so rounding never drifts across a list. No
/// clamping of the width: negative glue must survive so that cancellation
/// pairs (LaTeX \@xaddvskip, setspace) stay balanced.
#[derive(Default)]
struct GlueState {
    cur_glue: f64,
    cur_g: i64,
}

impl GlueState {
    fn advance(&mut self, glue: &crate::boxes::Glue, sign: u8, order: u8, set: f64) -> i64 {
        let delta = match sign {
            1 if glue.stretch_order == order => glue.stretch as f64,
            2 if glue.shrink_order == order => -(glue.shrink as f64),
            _ => return glue.width as i64,
        };
        let previous = self.cur_g;
        self.cur_glue += delta;
        // vet_glue: keep the product within TeX's ±billion range
        self.cur_g = (set * self.cur_glue).clamp(-1e9, 1e9).round() as i64;
        glue.width as i64 + self.cur_g - previous
    }
}

/// pdfTeX literal modes (`set_origin`, `direct_page`, `direct_always`)
pub const LITERAL_SET_ORIGIN: u8 = 0;
pub const LITERAL_DIRECT_PAGE: u8 = 1;
pub const LITERAL_DIRECT_ALWAYS: u8 = 2;
/// pdfTeX `MAX_COLORSTACKS`
const MAX_COLORSTACKS: usize = 32768;

/// One pdfTeX color stack (utils.c `colstack_type`). Page and form
/// shipouts keep separate stacks; an empty string stands for pdfTeX's NULL
/// (nothing is written for it). The state lives for the whole job.
#[derive(Clone, Debug)]
pub struct ColorStack {
    page_stack: Vec<String>,
    form_stack: Vec<String>,
    page_current: String,
    form_current: String,
    form_init: String,
    pub literal_mode: u8,
    /// re-emit the current value at the start of every page
    page_start: bool,
}

impl ColorStack {
    fn new(init: String, literal_mode: u8, page_start: bool) -> Self {
        ColorStack {
            page_stack: Vec::new(),
            form_stack: Vec::new(),
            page_current: init.clone(),
            form_current: init.clone(),
            form_init: init,
            literal_mode,
            page_start,
        }
    }

    fn current_mut(&mut self, page_mode: bool) -> &mut String {
        if page_mode {
            &mut self.page_current
        } else {
            &mut self.form_current
        }
    }

    fn push(&mut self, page_mode: bool, value: String) {
        let (stack, current) = if page_mode {
            (&mut self.page_stack, &mut self.page_current)
        } else {
            (&mut self.form_stack, &mut self.form_current)
        };
        stack.push(std::mem::replace(current, value));
    }

    /// the restored current value, or None for an empty stack
    fn pop(&mut self, page_mode: bool) -> Option<String> {
        let (stack, current) = if page_mode {
            (&mut self.page_stack, &mut self.page_current)
        } else {
            (&mut self.form_stack, &mut self.form_current)
        };
        *current = stack.pop()?;
        Some(current.clone())
    }
}

/// All color stacks of the job; stack 0 is pdfTeX's predefined one
/// (`colstacks_first_init`: "0 g 0 G", direct, page start).
#[derive(Clone, Debug)]
pub struct ColorStacks(Vec<ColorStack>);

impl Default for ColorStacks {
    fn default() -> Self {
        ColorStacks(vec![ColorStack::new(
            "0 g 0 G".to_string(),
            LITERAL_DIRECT_ALWAYS,
            true,
        )])
    }
}

impl ColorStacks {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    fn get_mut(&mut self, stack: usize) -> Option<&mut ColorStack> {
        self.0.get_mut(stack)
    }

    /// pdfTeX `newcolorstack`: the new stack number, None when all
    /// `MAX_COLORSTACKS` are in use
    pub fn new_stack(&mut self, init: String, literal_mode: u8, page_start: bool) -> Option<i32> {
        if self.0.len() == MAX_COLORSTACKS {
            return None;
        }
        self.0.push(ColorStack::new(init, literal_mode, page_start));
        Some(self.0.len() as i32 - 1)
    }

    /// pdfTeX `colorstackpagestart` for forms: every form starts from the
    /// stacks' initial values
    fn form_start(&mut self) {
        for cs in &mut self.0 {
            cs.form_stack.clear();
            cs.form_current = cs.form_init.clone();
        }
    }
}

impl<'a> RenderCtx<'a> {
    /// pdfTeX `literal(s, literal_mode, false)` for color stack data
    fn colorstack_literal(&mut self, s: &str, mode: u8, cur_h: i64, cur_v: i64) {
        match mode {
            LITERAL_SET_ORIGIN => {
                self.end_text();
                self.set_origin(cur_h, cur_v);
            }
            LITERAL_DIRECT_PAGE => self.end_text(),
            _ => self.end_string_nl(),
        }
        self.content.push_str(s);
        self.content.push('\n');
    }

    /// pdfTeX `pdf_out_colorstack_startpage`: every page-start stack whose
    /// current value is not the default "0 g 0 G" re-emits it, so color
    /// carries across page breaks
    fn colorstack_startpage(&mut self, cur_h: i64, cur_v: i64) {
        for i in 0..self.eng.color_stacks.len() {
            let Some(cs) = self.eng.color_stacks.get_mut(i) else {
                continue;
            };
            if !cs.page_start || cs.page_current == "0 g 0 G" || cs.page_current.is_empty() {
                continue;
            }
            let (s, mode) = (cs.page_current.clone(), cs.literal_mode);
            self.colorstack_literal(&s, mode, cur_h, cur_v);
        }
    }

    fn y_pdf(&self, tex_y_bp: f64) -> f64 {
        self.page_height_bp - tex_y_bp
    }

    /// pdfTeX `matrixused` (utils.c §1290): a `\pdfsetmatrix` CTM is active
    /// for annotation geometry only during page shipout with a non-empty
    /// matrix stack.
    fn matrix_used(&self) -> Option<&Matrix> {
        if self.page_mode {
            self.matrix_stack.last()
        } else {
            None
        }
    }

    /// pdfTeX `set_rect_dimens` + `matrixtransformrect` + output conversion:
    /// take a DVI-space rectangle (x from the left edge, y downward from
    /// the top, on the sp raster) and return the emitted PDF bottom-up bp
    /// rectangle, transformed by the active CTM exactly as `do_annot` /
    /// `end_link` do (pdftex.web §36445, utils.c §1500).
    fn page_rect(&self, left: i64, top_down: i64, right: i64, bottom_down: i64) -> [f64; 4] {
        let h = self.page_height_sp as f64;
        let (llx, lly, urx, ury) = match self.matrix_used() {
            Some(m) => {
                let r = matrix_transform_rect(
                    m,
                    left as f64,
                    h - bottom_down as f64,
                    right as f64,
                    h - top_down as f64,
                );
                (r[0], r[1], r[2], r[3])
            }
            None => (
                left as f64,
                h - bottom_down as f64,
                right as f64,
                h - top_down as f64,
            ),
        };
        [
            sp_to_bp(llx as i64),
            sp_to_bp(lly as i64),
            sp_to_bp(urx as i64),
            sp_to_bp(ury as i64),
        ]
    }

    /// pdfTeX `set_rect_dimens`: the DVI-space rectangle (left, top, right,
    /// bottom; y downward) of a link/annotation whatsit at (`cur_h`,
    /// `cur_v`); running dimensions (`RULE_FILL`) reach to the enclosing
    /// box's right edge, height and depth. Returns that raw rectangle and
    /// the emitted one (CTM applied, then widened by `margin`).
    fn set_rect_dimens(
        &self,
        cur_h: i64,
        cur_v: i64,
        (wd, ht, dp): (i32, i32, i32),
        margin: i64,
    ) -> ([i64; 4], [f64; 4]) {
        let (x, y) = (self.left_edge_sp, self.base_line_sp);
        let right = if wd == RULE_FILL { x + self.box_w_sp } else { cur_h + wd as i64 };
        let top = if ht == RULE_FILL { y - self.box_h_sp } else { cur_v - ht as i64 };
        let bottom = if dp == RULE_FILL { y + self.box_d_sp } else { cur_v + dp as i64 };
        let raw = [cur_h, top, right, bottom];
        (raw, self.margined_rect(raw, margin))
    }

    fn margined_rect(&self, [left, top, right, bottom]: [i64; 4], margin: i64) -> [f64; 4] {
        let [llx, lly, urx, ury] = self.page_rect(left, top, right, bottom);
        let m = sp_to_bp(margin);
        [llx - m, lly - m, urx + m, ury + m]
    }

    fn link_margin(&self) -> i64 {
        self.eng.eqtb.dim_params[DimParam::PdfLinkMargin.idx() as usize] as i64
    }

    /// pdfTeX `do_link` (\pdfstartlink) and `append_link` (a running link
    /// continuing into a new box at its nesting level): one /Link
    /// annotation per box, starting at `cur_h`. Records it on `link`.
    fn start_link_annot(&mut self, link: &mut crate::pdfout::OpenLink, cur_h: i64, cur_v: i64) {
        let (raw, rect) = self.set_rect_dimens(cur_h, cur_v, link.dims, self.link_margin());
        self.annots.push(Annot {
            rect,
            uri: link.uri.clone(),
            dest: link.dest.clone(),
            attr: link.attr.clone(),
            subtype: Some("/Link".to_string()),
        });
        link.annot = Some(self.annots.len() - 1);
        link.raw = raw;
    }

    /// pdfTeX `end_link`: a running-width link ends at the current point
    /// (re-transformed when a CTM is active, `matrixrecalculate`).
    fn end_link(&mut self, cur_h: i64) {
        let Some(link) = self.eng.pdf_doc.link_stack.pop() else {
            self.eng.error("pdf_link_stack empty, \\pdfendlink used without \\pdfstartlink?");
            return;
        };
        let (Some(index), true) = (link.annot, link.dims.0 == RULE_FILL) else {
            return;
        };
        let margin = self.link_margin();
        if self.matrix_used().is_some() {
            let [left, top, _, bottom] = link.raw;
            self.annots[index].rect = self.margined_rect([left, top, cur_h + margin, bottom], margin);
        } else {
            self.annots[index].rect[2] = sp_to_bp(cur_h + margin);
        }
    }

    /// ship a vbox's vertical list with its top edge at y (`vlist_out`)
    pub fn ship_vlist(&mut self, list: &NodeList, x: i64, y: i64, sign: u8, order: u8, set: f64) {
        let saved_dvi = if self.cur_s >= 0 && self.eng.engine_kind == crate::engine::EngineKind::XeTeX {
            Some(self.dpx.cursor)
        } else {
            None
        };
        self.cur_s += 1;
        if self.cur_s > 0 && self.eng.engine_kind == crate::engine::EngineKind::XeTeX {
            self.dpx_mark_depth(self.cur_s);
        }
        self.vlist_nodes(list, x, y, sign, order, set);
        if self.cur_s > 0 && self.eng.engine_kind == crate::engine::EngineKind::XeTeX {
            self.dpx_mark_depth(self.cur_s - 1);
            if let Some(cursor) = saved_dvi {
                self.dpx.cursor = cursor;
            }
        }
        self.cur_s -= 1;
    }

    fn vlist_nodes(&mut self, list: &NodeList, x: i64, y: i64, sign: u8, order: u8, set: f64) {
        // etex.ch: shipping right-to-left, x is the vlist's right edge and
        // boxes and rules hang leftwards from it
        let rtl = self.cur_dir == 1;
        let mut cur_y = y;
        let mut glue_state = GlueState::default();
        // pdfTeX `final_skip` of \pdfsnapy nodes set by \pdfsnapycomp
        let mut final_skips: Vec<(usize, i64)> = Vec::new();
        for (index, n) in list.iter().enumerate() {
            match n {
                Node::Box {
                    h,
                    d,
                    w,
                    shift,
                    glue_sign,
                    glue_order,
                    glue_set,
                    list: inner,
                    kind,
                    lr,
                    ..
                } => {
                    let (bh, bd) = (*h as i64, *d as i64);
                    let sh = if rtl { -(*shift as i64) } else { *shift as i64 };

                    // thread containing-box context for the inner list
                    let saved = (
                        self.left_edge_sp,
                        self.box_w_sp,
                        self.box_h_sp,
                        self.box_d_sp,
                    );
                    self.left_edge_sp = x + sh;
                    (self.box_w_sp, self.box_h_sp, self.box_d_sp) =
                        (*w as i64, *h as i64, *d as i64);
                    self.box_lr = *lr;
                    if *kind == HBOX {
                        // tex.web: a box's shift_amount is horizontal when
                        // the box sits in a VLIST (display boxes arrive here
                        // centered via shift = s + d)
                        let baseline = cur_y + bh;
                        self.ship_hlist(
                            inner,
                            x + sh,
                            baseline,
                            *glue_sign,
                            *glue_order,
                            *glue_set,
                        );
                    } else {
                        // vbox/vtop: the shift is horizontal
                        self.ship_vlist(inner, x + sh, cur_y, *glue_sign, *glue_order, *glue_set);
                    }
                    self.left_edge_sp = saved.0;
                    self.box_w_sp = saved.1;
                    self.box_h_sp = saved.2;
                    self.box_d_sp = saved.3;
                    cur_y += bh + bd;
                }
                Node::Rule {
                    width,
                    height,
                    depth,
                    subtype,
                    index,
                    ..
                } => {
                    // hrule in a vlist: null width fills the containing box
                    let w_sp = if *width == RULE_FILL {
                        self.box_w_sp
                    } else {
                        *width as i64
                    };
                    let (rh, rd) = (*height as i64, *depth as i64);
                    let y1 = cur_y + rh; // top of rule
                    self.place_rule((*width, *height, *depth, *subtype, *index), if rtl { x - w_sp } else { x }, y1 + rd, w_sp, rh + rd);
                    cur_y += rh + rd;
                }
                Node::Glue(g, _) => {
                    cur_y += glue_state.advance(g, sign, order, set);
                }
                Node::NativeGlyphRun {
                    run,
                    start,
                    end,
                    height,
                    depth,
                    ..
                } => {
                    self.emit_native_glyph_run_sp(run, *start, *end, x, cur_y + *height as i64);
                    cur_y += (*height + *depth) as i64;
                }
                Node::Leaders { glue, kind, body, .. } => {
                    let adv = glue_state.advance(glue, sign, order, set);
                    let (lw, lh, ld) = leader_dims(body);
                    match body {
                        // rule body: one rect spanning the whole advance;
                        // null width fills the containing box
                        LeaderBody::Rule { width, height, depth, subtype } => {
                            let w_sp = if *width == RULE_FILL {
                                self.box_w_sp
                            } else {
                                *width as i64
                            };
                            if w_sp > 0 && adv > 0 {
                                let rx = if rtl { x - w_sp } else { x };
                                self.place_rule((*width, *height, *depth, *subtype, 0), rx, cur_y + adv, w_sp, adv);
                            }
                        }
                        LeaderBody::Box(b) => {
                            // leader_wd = height + depth of the body box
                            let (kind, edge) = match *kind {
                                crate::boxes::LEADERS_G => (crate::boxes::LEADERS_A, self.ship_ref_sp.1),
                                kind => (kind, self.left_edge_sp),
                            };
                            let (positions, _, _) = crate::boxes::leader_layout(
                                kind,
                                (lh + ld) as i64,
                                adv,
                                edge,
                                cur_y,
                            );
                            for pos in positions {
                                self.ship_leader_copy(b, x, pos, true);
                            }
                        }
                    }
                    let _ = lw;
                    cur_y += adv;
                }
                Node::Kern(k, _)
                | Node::ExplicitKern(k, _)
                | Node::AccentKern(k, _) | Node::ItalicKern(k, _) | Node::SpaceAdjKern(k, _)
                | Node::MarginKern { width: k, .. } => {
                    cur_y += *k as i64;
                }
                Node::Penalty(_, _) | Node::Mark { .. } => {}
                Node::Whatsit(w @ crate::boxes::WhatIt::XePic { h, d, .. }, _) => {
                    // xetex.web resets cur_v to the saved DVI position,
                    // not to the picture's nominal bottom edge.
                    let saved_v = self.dpx.cursor.tex.1;
                    cur_y += *h as i64;
                    self.emit_whatsit_sp(w, x, cur_y);
                    cur_y = saved_v + *d as i64;
                }
                Node::Whatsit(
                    w @ (crate::boxes::WhatIt::PdfRefXImage { h, d, .. }
                    | crate::boxes::WhatIt::PdfRefXForm { h, d, .. }),
                _) => {
                    cur_y += *h as i64;
                    self.emit_whatsit_sp(w, x, cur_y);
                    cur_y += *d as i64;
                }
                Node::Whatsit(crate::boxes::WhatIt::PdfSnapYComp(ratio), _) => {
                    // pdfTeX `do_snapy_comp`: move by `ratio`/1000 of the gap
                    // the next \pdfsnapy will meet; it makes up the rest
                    let next_snap = list[index + 1..].iter().position(|n| {
                        matches!(n, Node::Whatsit(crate::boxes::WhatIt::PdfSnapY(_), _))
                    });
                    if let Some(offset) = next_snap {
                        let q = index + 1 + offset;
                        let Node::Whatsit(crate::boxes::WhatIt::PdfSnapY(glue), _) = &list[q] else {
                            unreachable!()
                        };
                        let tmp_v = get_vpos(&list[index..q], cur_y, sign, order, set);
                        let g = gap_amount(glue, tmp_v, self.eng.pdf_snap_refpos.1);
                        let g2 = round_xn_over_d(g, i64::from(*ratio), 1000);
                        cur_y += g2;
                        // 1sp records that the compensation has been done
                        let rest = if g == g2 { 1 } else { g - g2 };
                        final_skips.push((q, rest));
                    }
                }
                Node::Whatsit(crate::boxes::WhatIt::PdfSnapY(glue), _) => {
                    // pdfTeX `do_snapy`
                    cur_y += match final_skips.iter().find(|(at, _)| *at == index) {
                        Some((_, skip)) => *skip,
                        None => gap_amount(glue, cur_y, self.eng.pdf_snap_refpos.1),
                    };
                }
                Node::Whatsit(w, _) => {
                    self.emit_whatsit_sp(w, x, cur_y);
                }
                Node::Ins { box_node, .. } => {
                    if let Node::Box { list: inner, .. } = &**box_node {
                        self.ship_vlist(inner, x, cur_y, 0, 0, 0.0);
                    }
                }
                Node::VAdjust(items, _) | Node::PreAdjust(items, _) => {
                    self.ship_vlist(items, x, cur_y, 0, 0, 0.0);
                }
                _ => {}
            }
        }
    }

    /// ship an hbox's horizontal list with baseline at y (`hlist_out`): a
    /// running link open at this box nesting level gets a new annotation
    /// over this box (pdfTeX "Create link annotations for the current hbox").
    pub fn ship_hlist(&mut self, list: &NodeList, x: i64, y: i64, sign: u8, order: u8, set: f64) {
        let saved_dvi = if self.cur_s >= 0 && self.eng.engine_kind == crate::engine::EngineKind::XeTeX {
            Some(self.dpx.cursor)
        } else {
            None
        };
        self.cur_s += 1;
        if self.cur_s > 0 && self.eng.engine_kind == crate::engine::EngineKind::XeTeX {
            self.dpx_mark_depth(self.cur_s);
        }
        let saved = (self.left_edge_sp, self.base_line_sp);
        (self.left_edge_sp, self.base_line_sp) = (x, y);
        if self.page_mode && self.eng.pdf_doc.gen_running_link {
            let mut stack = std::mem::take(&mut self.eng.pdf_doc.link_stack);
            for link in stack.iter_mut() {
                if link.nesting == self.cur_s && link.dims.0 == RULE_FILL {
                    self.start_link_annot(link, x, y);
                }
            }
            self.eng.pdf_doc.link_stack = stack;
        }
        self.hlist_nodes(list, x, y, sign, order, set);
        (self.left_edge_sp, self.base_line_sp) = saved;
        if self.cur_s > 0 && self.eng.engine_kind == crate::engine::EngineKind::XeTeX {
            self.dpx_mark_depth(self.cur_s - 1);
            if let Some(cursor) = saved_dvi {
                self.dpx.cursor = cursor;
            }
        }
        self.cur_s -= 1;
    }

    fn hlist_nodes(&mut self, list: &NodeList, x: i64, y: i64, sign: u8, order: u8, set: f64) {
        // etex.ch hlist_out with TeXXeT material: LR stack and reversal
        if self.eng.texxet_nodes {
            return self.hlist_nodes_lr(list, x, y, sign, order, set);
        }
        let mut cur_x = x;
        let mut glue_state = GlueState::default();
        for n in list {
            cur_x = self.hlist_node_out(n, cur_x, y, &mut glue_state, sign, order, set);
        }
    }

    /// output one hlist node at `cur_x`; returns the position after it
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn hlist_node_out(
        &mut self,
        n: &Node,
        mut cur_x: i64,
        y: i64,
        glue_state: &mut GlueState,
        sign: u8,
        order: u8,
        set: f64,
    ) -> i64 {
        {
            match n {
                Node::Char { c, font, .. } | Node::Ligature { c, font, .. }
                    if self.eng.eqtb.fonts.get(usize::from(*font)).is_some_and(|f| f.lua.is_some()) =>
                {
                    cur_x += self.emit_lua_glyph(*font, u32::from(*c), cur_x, y, 0, 0, 0);
                }
                Node::LuaGlyph(g) => {
                    cur_x += self.emit_lua_glyph(g.font, g.c, cur_x, y, g.xoffset, g.yoffset, g.expansion_factor);
                }
                Node::Char { c, font, .. } => {
                    let adv = self.font_char_advance_sp(*font, *c);
                    self.emit_char_sp(*font, *c, cur_x, y, 0);
                    cur_x += adv;
                }
                Node::Ligature {
                    c, font, lig_width, ..
                } => {
                    let adv = self.font_lig_advance_sp(*font, *lig_width);
                    self.emit_char_sp(*font, *c, cur_x, y, 0);
                    cur_x += adv;
                }
                Node::NativeGlyphRun {
                    run,
                    start,
                    end,
                    width,
                    ..
                } => {
                    self.emit_native_glyph_run_sp(run, *start, *end, cur_x, y);
                    self.dpx_advance_h(i64::from(*width), i64::from(*width));
                    cur_x += *width as i64;
                }
                Node::Glue(g, _) => {
                    let adv = glue_state.advance(g, sign, order, set);
                    cur_x += adv;
                }
                Node::Kern(k, _)
                | Node::ExplicitKern(k, _)
                | Node::AccentKern(k, _) | Node::ItalicKern(k, _) | Node::SpaceAdjKern(k, _)
                | Node::MarginKern { width: k, .. }
                | Node::MathKern(k, 1.., _) => {
                    cur_x += *k as i64;
                }
                Node::ExKern { width, ex, .. } => {
                    cur_x += (*width + *ex) as i64;
                }
                Node::Penalty(_, _) => {}
                Node::Rule {
                    width,
                    height,
                    depth,
                    subtype,
                    index,
                    ..
                } => {
                    // vrule in an hlist: null height/depth fill the containing box
                    let h_sp = if *height == RULE_FILL {
                        self.box_h_sp
                    } else {
                        *height as i64
                    };
                    let d_sp = if *depth == RULE_FILL {
                        self.box_d_sp
                    } else {
                        *depth as i64
                    };
                    let (rw, rh, rd) = (*width as i64, h_sp, d_sp);
                    self.place_rule((*width, *height, *depth, *subtype, *index), cur_x, y + rd, rw, rh + rd);
                    if self.eng.engine_kind == crate::engine::EngineKind::XeTeX
                        && rw > 0 && rh + rd > 0 && *subtype != crate::boxes::RULE_EMPTY
                    {
                        self.dpx_advance_h(rw, rw);
                    }
                    cur_x += rw;
                }
                Node::Box {
                    w,
                    h,
                    d,
                    shift,
                    glue_sign,
                    glue_order,
                    glue_set,
                    list: inner,
                    kind,
                    lr,
                    ..
                } => {
                    let (bw, bh, sh) = (*w as i64, *h as i64, *shift as i64);

                    // thread containing-box context for the inner list
                    let saved = (
                        self.left_edge_sp,
                        self.box_w_sp,
                        self.box_h_sp,
                        self.box_d_sp,
                    );
                    // etex.ch: right-to-left, a box is entered at its right
                    // edge (`if cur_dir=right_to_left then cur_h:=edge`)
                    let start_x = if self.cur_dir == 1 { cur_x + bw } else { cur_x };
                    self.left_edge_sp = if *kind == HBOX { start_x } else { start_x + sh };
                    (self.box_w_sp, self.box_h_sp, self.box_d_sp) =
                        (*w as i64, *h as i64, *d as i64);
                    self.box_lr = *lr;
                    if *kind == HBOX {
                        // hbox: the shift is vertical (baseline moves down)
                        let baseline = y + sh;
                        self.ship_hlist(inner, start_x, baseline, *glue_sign, *glue_order, *glue_set);
                    } else {
                        // vbox/vtop in an hlist: the shift is vertical
                        self.ship_vlist(
                            inner,
                            start_x,
                            y + sh - bh,
                            *glue_sign,
                            *glue_order,
                            *glue_set,
                        );
                    }
                    self.left_edge_sp = saved.0;
                    self.box_w_sp = saved.1;
                    self.box_h_sp = saved.2;
                    self.box_d_sp = saved.3;
                    cur_x += bw;
                    let _ = (bh, d);
                }
                Node::Disc(dc) => {
                    for nn in &dc.no_break {
                        match nn {
                            Node::Char { c, font, .. }
                                if self.eng.eqtb.fonts.get(usize::from(*font)).is_some_and(|f| f.lua.is_some()) =>
                            {
                                cur_x += self.emit_lua_glyph(*font, u32::from(*c), cur_x, y, 0, 0, 0);
                            }
                            Node::Char { c, font, .. } => {
                                let adv = self.font_char_advance_sp(*font, *c);
                                self.emit_char_sp(*font, *c, cur_x, y, 0);
                                cur_x += adv;
                            }
                            Node::NativeGlyphRun {
                                run,
                                start,
                                end,
                                width,
                                ..
                            } => {
                                self.emit_native_glyph_run_sp(run, *start, *end, cur_x, y);
                                self.dpx_advance_h(i64::from(*width), i64::from(*width));
                                cur_x += *width as i64;
                            }
                            other => {
                                let single: NodeList = vec![other.clone()];
                                let (w, _, _) = crate::boxes::hlist_dims(&single, &self.eng.eqtb);
                                let mut fresh = GlueState::default();
                                self.hlist_node_out(other, cur_x, y, &mut fresh, sign, order, set);
                                cur_x += w as i64;
                            }
                        }
                    }
                }
                Node::Leaders { glue, kind, body, .. } => {
                    let adv = glue_state.advance(glue, sign, order, set);
                    let (lw, lh, ld) = leader_dims(body);
                    match body {
                        // rule body: one rect over the whole advance; null
                        // height/depth fill the containing box
                        LeaderBody::Rule { width, height, depth, subtype } => {
                            let h_sp = if *height == RULE_FILL {
                                self.box_h_sp
                            } else {
                                *height as i64
                            };
                            let d_sp = if *depth == RULE_FILL {
                                self.box_d_sp
                            } else {
                                *depth as i64
                            };
                            let (rh, rd) = (h_sp, d_sp);
                            if adv > 0 && rh + rd > 0 {
                                self.place_rule((*width, *height, *depth, *subtype, 0), cur_x, y + rd, adv, rh + rd);
                            }
                        }
                        LeaderBody::Box(b) => {
                            // etex.ch right-to-left: the first box starts
                            // 10sp earlier (each copy is then entered at its
                            // right edge by ship_leader_copy)
                            let first = if self.cur_dir == 1 { cur_x - 10 } else { cur_x };
                            let (kind, edge) = match *kind {
                                crate::boxes::LEADERS_G => (crate::boxes::LEADERS_A, self.ship_ref_sp.0),
                                kind => (kind, self.left_edge_sp),
                            };
                            let (positions, _, _) = crate::boxes::leader_layout(
                                kind,
                                lw as i64,
                                adv,
                                edge,
                                first,
                            );
                            for pos in positions {
                                self.ship_leader_copy(b, pos, y, false);
                            }
                        }
                    }
                    let _ = (lh, ld);
                    cur_x += adv;
                }
                Node::Whatsit(w, _) => {
                    self.emit_whatsit_sp(w, cur_x, y);
                    if let crate::boxes::WhatIt::PdfRefXImage { w, .. }
                    | crate::boxes::WhatIt::PdfRefXForm { w, .. }
                    | crate::boxes::WhatIt::XePic { w, .. } = w
                    {
                        cur_x += *w as i64;
                    }
                }
                Node::Mark { .. } | Node::Ins { .. } => {}
                _ => {}
            }
        }
        cur_x
    }

    /// ship one leader body copy. Horizontal (`vertical == false`): the
    /// copy's baseline sits at `at`. Vertical: the copy's top edge sits
    /// at `at` (a vlist item position).
    fn ship_leader_copy(&mut self, b: &Node, x: i64, at: i64, vertical: bool) {
        let Node::Box {
            w,
            h,
            d,
            shift,
            glue_sign,
            glue_order,
            glue_set,
            list: inner,
            kind,
            lr,
            ..
        } = b
        else {
            return;
        };
        let (bh, sh) = (*h as i64, *shift as i64);
        let saved = (
            self.left_edge_sp,
            self.box_w_sp,
            self.box_h_sp,
            self.box_d_sp,
        );
        // etex.ch right-to-left: a horizontal copy is entered at its right
        // edge (`cur_h:=cur_h+leader_wd`), a vertical one hangs left of the
        // vlist's right edge (`cur_h:=left_edge-shift_amount`)
        let rtl = self.cur_dir == 1;
        let x = if rtl && !vertical { x + *w as i64 } else { x };
        let sh_h = if rtl { -sh } else { sh };
        self.left_edge_sp = x;
        (self.box_w_sp, self.box_h_sp, self.box_d_sp) = (*w as i64, *h as i64, *d as i64);
        self.box_lr = *lr;
        if vertical {
            if *kind == HBOX {
                // hbox as a vlist item: top edge at `at`, baseline below
                self.ship_hlist(inner, x, at + bh + sh, *glue_sign, *glue_order, *glue_set);
            } else {
                self.ship_vlist(inner, x + sh_h, at, *glue_sign, *glue_order, *glue_set);
            }
        } else if *kind == HBOX {
            self.ship_hlist(inner, x, at + sh, *glue_sign, *glue_order, *glue_set);
        } else {
            self.ship_vlist(inner, x + sh, at - bh, *glue_sign, *glue_order, *glue_set);
        }
        self.left_edge_sp = saved.0;
        self.box_w_sp = saved.1;
        self.box_h_sp = saved.2;
        self.box_d_sp = saved.3;
    }

    fn font_char_width(&self, f: u16, c: u8) -> i32 {
        self.eng
            .eqtb
            .fonts
            .get(f as usize)
            .map(|ff| ff.char_width(c))
            .unwrap_or(0)
    }

    fn font_char_advance_sp(&self, f: u16, c: u8) -> i64 {
        let w = self.font_char_width(f, c) as i64;
        let ratio = self.eng.eqtb.expand.get(f as usize).map_or(0, |x| x.ratio);
        let is_already_scaled = self
            .eng
            .eqtb
            .expand
            .get(f as usize)
            .map_or(false, |x| x.blink != 0);
        if is_already_scaled || ratio == 0 {
            w
        } else {
            round_xn_over_d(w, 1000 + ratio as i64, 1000)
        }
    }

    fn font_lig_advance_sp(&self, f: u16, lig_width: i32) -> i64 {
        let w = lig_width as i64;
        let ratio = self.eng.eqtb.expand.get(f as usize).map_or(0, |x| x.ratio);
        let is_already_scaled = self
            .eng
            .eqtb
            .expand
            .get(f as usize)
            .map_or(false, |x| x.blink != 0);
        if is_already_scaled || ratio == 0 {
            w
        } else {
            round_xn_over_d(w, 1000 + ratio as i64, 1000)
        }
    }

    /// pdfTeX `pdf_set_font` resource lookup: the number `n` of the font's
    /// `/F<n>` resource. A raw font is named after the font `ff` that owns
    /// its dictionary (`set_ff`), so sizes of one TFM share a name; this
    /// engine's semantic remaps of a font's code space have no pdfTeX
    /// counterpart and take their own number above every font number.
    fn ensure_font(&mut self, f: u16, binding: crate::pdfout::FontBinding) -> u32 {
        // pdf_set_font: `if not font_used[f] then pdf_init_font(f)`
        let ff = self.eng.pdf_init_font(f);
        let key = binding.resource_key(f);
        if self.cur_font == key && self.cur_pdf_font != 0 {
            return self.cur_pdf_font;
        }
        if let Some((_, num)) = self.used_fonts.iter().find(|(id, _)| *id == key) {
            self.cur_font = key;
            self.cur_pdf_font = *num;
            return *num;
        }
        let num = if binding == crate::pdfout::FontBinding::RAW {
            Ok(u32::from(ff))
        } else {
            u32::try_from(key)
        };
        let Ok(num) = num else {
            self.eng
                .error("PDF page exceeds the supported font resource count");
            return 0;
        };
        self.used_fonts.push((key, num));
        self.page_fonts.push((key, num));
        self.cur_font = key;
        self.cur_pdf_font = num;
        num
    }

    /// pdfTeX `pdf_print_real(m, d)`: print m/10^d, trimming trailing zeros.
    fn push_real(&mut self, m: i64, d: u32) {
        push_decimal(&mut self.content, m, d);
    }

    /// pdfTeX `pdf_print_bp(s)`: print a sp displacement as bp.
    fn push_bp(&mut self, sp: i64) {
        let out = push_bp_sp(&mut self.content, sp, self.decimal_digits);
        self.scaled_out = out;
    }

    /// pdfTeX `pdf_set_origin(h, v)`: re-center the text/print origin at the
    /// TeX-space point (h, v_down), emitting `cm` when the move is visible.
    /// `scaled_out`-snapped so the recorded origin matches the printed raster.
    fn set_origin(&mut self, h_sp: i64, v_down_sp: i64) {
        if self.eng.engine_kind == crate::engine::EngineKind::LuaTeX {
            // luatex pdf_set_pos: the origin is the absolute position rounded
            // to the output raster, `cm` the difference of two such positions
            if let Some((dh, dv)) = self.lua_cm_move(h_sp, v_down_sp) {
                self.print_lua_cm(dh, dv);
                self.lua_cm = (self.lua_cm.0 + dh, self.lua_cm.1 + dv);
            }
            self.origin_h = h_sp;
            self.origin_v = v_down_sp;
        } else if (h_sp - self.origin_h).abs() >= self.min_bp_val
            || (v_down_sp - self.origin_v).abs() >= self.min_bp_val
        {
            self.content.push_str("1 0 0 1 ");
            self.push_bp(h_sp - self.origin_h);
            self.origin_h += self.scaled_out;
            self.content.push(' ');
            self.push_bp(self.origin_v - v_down_sp);
            self.origin_v -= self.scaled_out;
            self.content.push_str(" cm\n");
        }
        self.pdf_h = self.origin_h;
        self.tj_start_h = self.pdf_h;
        self.pdf_v = self.origin_v;
    }

    /// pdfTeX `pdf_set_origin_temp`: emit the re-centering `cm` without
    /// updating the tracked origin (used inside a `q..Q` scope).
    fn set_origin_temp(&mut self, h_sp: i64, v_down_sp: i64) {
        if self.eng.engine_kind == crate::engine::EngineKind::LuaTeX {
            if let Some((dh, dv)) = self.lua_cm_move(h_sp, v_down_sp) {
                self.print_lua_cm(dh, dv);
            }
        } else if (h_sp - self.origin_h).abs() >= self.min_bp_val
            || (v_down_sp - self.origin_v).abs() >= self.min_bp_val
        {
            self.content.push_str("1 0 0 1 ");
            self.push_bp(h_sp - self.origin_h);
            self.content.push(' ');
            self.push_bp(self.origin_v - v_down_sp);
            self.content.push_str(" cm\n");
        }
    }

    /// luatex `calc_pdfpos` in page mode: the move from the tracked origin to
    /// the absolute position `(h_sp, v_down_sp)`, in units of the last output
    /// digit; `None` when the origin already is there.
    fn lua_cm_move(&self, h_sp: i64, v_down_sp: i64) -> Option<(i64, i64)> {
        // k1 = 10^digits / one_bp, rounded away from zero at one half (`i64round`)
        let k1 = 10f64.powi(self.decimal_digits as i32) / SP_PER_BP;
        let round = |r: f64| (if r > 0.0 { r + 0.5 } else { r - 0.5 }) as i64;
        let (h, v) = (round(h_sp as f64 * k1), round((self.page_height_sp - v_down_sp) as f64 * k1));
        (h != self.lua_cm.0 || v != self.lua_cm.1).then(|| (h - self.lua_cm.0, v - self.lua_cm.1))
    }

    fn print_lua_cm(&mut self, dh: i64, dv: i64) {
        self.content.push_str("1 0 0 1 ");
        push_decimal(&mut self.content, dh, self.decimal_digits);
        self.content.push(' ');
        push_decimal(&mut self.content, dv, self.decimal_digits);
        self.content.push_str(" cm\n");
    }

    /// pdfTeX `pdf_begin_text`.
    fn begin_text(&mut self) {
        let (h, v) = (0, self.page_height_sp);
        self.set_origin(h, v);
        self.content.push_str("BT\n");
        self.doing_text = true;
        self.pdf_f = 0;
        self.last_f = 0;
        self.last_f_size = 0;
        self.doing_string = false;
        self.cur_tm_a = 0;
        self.doing_hex_string = false;
    }

    /// pdfTeX `pdf_end_string`.
    fn end_string(&mut self) {
        if self.doing_hex_string {
            self.content.push_str(">]TJ");
            self.doing_hex_string = false;
            self.doing_string = false;
        } else if self.doing_string {
            self.content.push_str(")]TJ");
            self.doing_string = false;
        }
    }

    /// pdfTeX `pdf_end_string_nl`.
    fn end_string_nl(&mut self) {
        if self.doing_hex_string {
            self.content.push_str(">]TJ\n");
            self.doing_hex_string = false;
            self.doing_string = false;
        } else if self.doing_string {
            self.content.push_str(")]TJ\n");
            self.doing_string = false;
        }
    }
    /// pdfTeX `pdf_end_text`.
    fn end_text(&mut self) {
        if self.dpxt.in_text() {
            self.dpxt_graphics_mode();
        }
        self.end_pdftex_text();
    }
    /// The pdfTeX-style text object only (the dpx text engine keeps its own).
    fn end_pdftex_text(&mut self) {
        if self.doing_text {
            self.end_string_nl();
            self.content.push_str("ET\n");
            self.doing_text = false;
        }
        self.doing_hex_string = false;
    }

    /// auto-expand ratio of an engine font (`get_font_auto_expand_ratio`).
    fn font_ratio(&self, f: u16) -> i32 {
        self.eng.eqtb.expand.get(f as usize).map_or(0, |x| x.ratio)
    }

    /// pdfTeX `pdf_set_font`: dedup on (resource number, font size).
    fn set_font(&mut self, f: u16, binding: crate::pdfout::FontBinding) {
        self.pdf_f = f;
        let at_size_sp = self
            .eng
            .eqtb
            .fonts
            .get(f as usize)
            .map(|ff| ff.at_size as i64)
            .unwrap_or(0);
        let base_f = self
            .eng
            .eqtb
            .expand
            .get(f as usize)
            .and_then(|ex| if ex.blink != 0 { Some(ex.blink) } else { None })
            .unwrap_or(f);
        let num = self.ensure_font(base_f, binding);
        if num == self.last_f && at_size_sp == self.last_f_size {
            return;
        }
        let (font_size_pdf, _) = divide_scaled(at_size_sp, ONE_HUNDRED_BP_SP, 6);
        self.content.push_str("/F");
        push_i64(&mut self.content, i64::from(num));
        self.content.push_str(&self.eng.pdf_doc.resname_prefix);
        self.content.push(' ');
        self.push_real(font_size_pdf, 4);
        self.content.push_str(" Tf");
        self.last_f = num;
        self.last_f_size = at_size_sp;
    }

    /// pdfTeX `pdf_set_text_pos(v, v_out, f)`: emit `Tm` (scaled matrix) or
    /// the relative `Td` move, keeping `pdf_h`/`pdf_v` on the sp raster.
    /// `new_tm_a` is the effective auto-expand ratio (thousandths) of the
    /// glyph being placed — inherited through VF expansion, not recomputed
    /// from the base font id.
    fn set_text_pos(&mut self, cur_h: i64, cur_v: i64, v: i64, v_out: i64, new_tm_a: i32) {
        self.content.push(' ');
        if new_tm_a != 0 || self.cur_tm_a != 0 {
            self.push_real(1000 + new_tm_a as i64, 3);
            self.content.push_str(" 0 0 1 ");
            self.push_bp(cur_h - self.origin_h);
            self.pdf_h = self.origin_h + self.scaled_out;
            self.content.push(' ');
            self.push_bp(self.origin_v - cur_v);
            self.pdf_v = self.origin_v - self.scaled_out;
            self.content.push_str(" Tm");
            self.cur_tm_a = new_tm_a;
        } else {
            // works only for unexpanded fonts
            self.push_bp(cur_h - self.tj_start_h);
            self.pdf_h = self.tj_start_h + self.scaled_out;
            self.content.push(' ');
            self.push_real(v, self.decimal_digits);
            self.pdf_v -= v_out;
            self.content.push_str(" Td");
        }
        self.tj_start_h = self.pdf_h;
        self.delta_h = 0;
    }

    /// pdfTeX `pdf_begin_string(f)` + the char emission of `output_one_char`.
    /// `cur_h`/`cur_v` are the pen position in TeX space (sp, v downward).
    /// `ratio` is the effective auto-expand ratio in thousandths: for a real
    /// font it is `get_font_auto_expand_ratio(f)`; a VF glyph recursed with
    /// an inherited ratio from its expanded wrapper carries it explicitly.
    fn begin_string(
        &mut self,
        cur_h: i64,
        cur_v: i64,
        f: u16,
        binding: crate::pdfout::FontBinding,
        ratio: i32,
    ) {
        let mut must_set_text_pos = false;
        if !self.doing_text {
            self.begin_text();
            must_set_text_pos = true;
        }
        if self.pdf_f != f || self.cur_font != binding.resource_key(f) {
            self.end_string();
            self.set_font(f, binding);
        }
        let at_size_sp = self
            .eng
            .eqtb
            .fonts
            .get(f as usize)
            .map(|ff| ff.at_size as i64)
            .unwrap_or(0);
        let (_, m) = divide_scaled(at_size_sp, ONE_HUNDRED_BP_SP, 6);
        let gap = cur_h - (self.tj_start_h + self.delta_h);
        let (s, s_out) = if self.cur_tm_a == 0 {
            divide_scaled(gap, m, 3)
        } else {
            let (s, _) = divide_scaled(
                round_xn_over_d(gap, 1000, 1000 + self.cur_tm_a as i64),
                m,
                3,
            );
            // s_out is unused when |s| >= 32768: the matrix is reset below
            let s_out = if s.abs() < GAP_SPLIT_LIMIT {
                let mut o = round_xn_over_d(
                    round_xn_over_d(m, s.abs(), 1000),
                    1000 + self.cur_tm_a as i64,
                    1000,
                );
                if s < 0 {
                    o = -o;
                }
                o
            } else {
                0
            };
            (s, s_out)
        };
        let (v, v_out) = if (cur_v - self.pdf_v).abs() >= self.min_bp_val {
            divide_scaled(self.pdf_v - cur_v, ONE_HUNDRED_BP_SP, self.decimal_digits + 2)
        } else {
            (0, 0)
        };
        if !must_set_text_pos {
            must_set_text_pos = v != 0 || s.abs() >= GAP_SPLIT_LIMIT || ratio != self.cur_tm_a;
        }
        if must_set_text_pos {
            self.end_string();
            self.set_font(f, binding);
            self.set_text_pos(cur_h, cur_v, v, v_out, ratio);
        }
        let s = if must_set_text_pos { 0 } else { s };
        let s_out = if must_set_text_pos { 0 } else { s_out };
        if !self.doing_string {
            self.content.push_str(" [");
            if s == 0 {
                self.content.push('(');
            }
        }
        if s != 0 {
            if self.doing_string {
                self.content.push(')');
            }
            push_i64(&mut self.content, -s);
            self.content.push('(');
            self.delta_h += s_out;
        }
        self.doing_string = true;
    }

    fn pdf_char_width(&mut self, f: u16, character: u8) -> Result<i64, String> {
        let key = (f, 0x1_0000 | u32::from(character));
        if let Some(&width) = self.advance_cache.get(&key) {
            return Ok(width);
        }
        let font = self
            .eng
            .eqtb
            .fonts
            .get(f as usize)
            .cloned()
            .ok_or_else(|| format!("Missing font {f} during shipout"))?;
        let program = if let Some(program) = self.font_programs.get(&f) {
            program.clone()
        } else {
            let program = self.eng.font_loader.program_for_font(&font)?;
            self.font_programs.insert(f, program.clone());
            program
        };
        let width = if program.is_type1() {
            font.char_width(character) as i64
        } else {
            let face = program.face()?;
            let (glyph, _) =
                crate::font_program::legacy_glyph(&face, font.encoding.as_deref(), character)
                    .map_err(|error| format!("Font `{}`: {error}", font.tfm_name))?;
            let advance = face
                .glyph_hor_advance(ttf_parser::GlyphId(glyph))
                .unwrap_or(0) as f64;
            let pdf_width = (advance * 1000.0 / face.units_per_em() as f64).round() as i64;
            round_xn_over_d(font.at_size as i64, pdf_width, 1000)
        };
        self.advance_cache.insert(key, width);
        Ok(width)
    }

    /// pdfTeX `adv_char_width(f, c)`: advance `delta_h` on the same raster.
    /// The font id is the expanded clone (canonical `auto_expand_vf` maps
    /// VF local bases through `auto_expand_font`), so its baked width is
    /// already pre-scaled; no synthetic scaling here.
    fn adv_char_width(&mut self, f: u16, w: i64) {
        let at_size_sp = self
            .eng
            .eqtb
            .fonts
            .get(f as usize)
            .map(|ff| ff.at_size as i64)
            .unwrap_or(0);
        let (_, m) = divide_scaled(at_size_sp, ONE_HUNDRED_BP_SP, 6);
        let s_out = if self.cur_tm_a == 0 {
            let (_, out) = divide_scaled(w, m, 4);
            out
        } else {
            let (s, _) = divide_scaled(round_xn_over_d(w, 1000, 1000 + self.cur_tm_a as i64), m, 4);
            let mut o = round_xn_over_d(
                round_xn_over_d(m, s.abs(), 10000),
                1000 + self.cur_tm_a as i64,
                1000,
            );
            if s < 0 {
                o = -o;
            }
            o
        };
        self.delta_h += s_out;
    }

    /// The parsed virtual font behind engine font `f`; expanded copies
    /// (`name+20`, `name-15`) fall back to their base font's packets.
    fn vf_font(&mut self, f: u16) -> Option<std::rc::Rc<crate::fontload::VfFont>> {
        if let Some(cached) = self.vf_fonts.get(&f) {
            return cached.clone();
        }
        let vf = self.eng.eqtb.fonts.get(f as usize).and_then(|font| {
            let vf_fonts = &self.eng.font_loader.vf_fonts;
            let name = &font.tfm_name;
            vf_fonts.get(&(name.clone(), font.at_size)).cloned().or_else(|| {
                let idx = name.rfind(['+', '-'])?;
                (idx > 0 && name[idx + 1..].chars().all(|c| c.is_ascii_digit()))
                    .then(|| vf_fonts.get(&(name[..idx].to_string(), font.at_size)).cloned())
                    .flatten()
            })
        });
        self.vf_fonts.insert(f, vf.clone());
        vf
    }

    fn emit_cjk_char_sp(
        &mut self,
        f: u16,
        c: u8,
        x_sp: i64,
        v_sp: i64,
        inherited_ratio: i32,
        semantic_text: &str,
    ) {
        let at_size_sp = self
            .eng
            .eqtb
            .fonts
            .get(f as usize)
            .map(|ff| ff.at_size as i64)
            .unwrap_or(0);
        if at_size_sp <= 0 {
            return;
        }
        let self_ratio = self.font_ratio(f);
        let ratio = if self_ratio != 0 {
            self_ratio
        } else {
            inherited_ratio
        };
        let base_f = self
            .eng
            .eqtb
            .expand
            .get(f as usize)
            .and_then(|ex| if ex.blink != 0 { Some(ex.blink) } else { None })
            .unwrap_or(f);
        let advance = match self.pdf_char_width(f, c) {
            Ok(width) => width,
            Err(error) => {
                self.eng.error(&error);
                return;
            }
        };
        let (binding_idx, code) =
            self.eng
                .pdf_doc
                .get_or_alloc_legacy_code(base_f as usize, c, semantic_text);
        self.begin_string(x_sp, v_sp, f, binding_idx, ratio);
        push_pdf_char(&mut self.content, code);
        self.adv_char_width(f, advance);
        let x_bp = sp_to_bp(x_sp);
        let y_bp = self.y_pdf(sp_to_bp(v_sp));
        let merged = if let Some(crate::boxes::DisplayItem::GlyphRun {
            font: last_f,
            y_bp: last_y,
            glyphs,
            ..
        }) = self.display_list.items.last_mut()
        {
            if *last_f == f && (*last_y - y_bp).abs() < 1e-3 {
                glyphs.push(code);
                true
            } else {
                false
            }
        } else {
            false
        };
        if !merged {
            self.display_list.push(crate::boxes::DisplayItem::GlyphRun {
                font: f,
                x_bp,
                y_bp,
                glyphs: vec![code],
                tag: None,
                span: None,
            });
        }
    }

    /// pdfTeX `output_one_char`: begin the string (emitting Tf/Tm/Td as the
    /// canonical state machine requires), print the char, advance the raster.
    fn emit_char_sp(&mut self, f: u16, c: u8, x_sp: i64, v_sp: i64, inherited_ratio: i32) {
        let text = self.cjk_text;
        self.emit_char_sp_with_text(f, c, x_sp, v_sp, inherited_ratio, text);
    }

    fn emit_char_sp_with_text(
        &mut self,
        f: u16,
        c: u8,
        x_sp: i64,
        v_sp: i64,
        inherited_ratio: i32,
        logical_ch: Option<char>,
    ) {
        let at_size_sp = self
            .eng
            .eqtb
            .fonts
            .get(f as usize)
            .map(|ff| ff.at_size as i64)
            .unwrap_or(0);
        if at_size_sp <= 0 {
            return; // nullfont: nothing to draw
        }
        self.eng.ensure_vf_bases(f);
        let virtual_font = self.eng.font_loader.vf_bases.contains_key(&f);
        let (x_sp, v_sp) = if self.eng.engine_kind == crate::engine::EngineKind::XeTeX {
            if self.dpxt.in_text() {
                self.dpxt_graphics_mode();
            }
            let Some((advance, height, depth)) = self.dpx_tfm_metrics(f, c) else { return };
            let (x, v) = self.dpx_sync(x_sp, v_sp);
            self.dpx_advance_h(self.font_char_advance_sp(f, c), advance);
            if virtual_font {
                // VF packets have their own push/pop; track their physical
                // glyphs rather than the virtual character's metric box.
                (x, v)
            } else {
                self.dpx_track_box(x, v, advance, height, depth);
                self.dpx_compensate(x, v)
            }
        } else {
            (x_sp, v_sp)
        };
        let self_ratio = self.font_ratio(f);
        let ratio = if self_ratio != 0 {
            self_ratio
        } else {
            inherited_ratio
        };
        // Virtual font: expand the glyph into its mapped steps in the base
        // fonts (kerns included as offsets). The VF font itself is never
        // registered as a page resource. Offsets advance on the exact sp
        // raster, as pdfTeX's do_vf_packet does.
        if virtual_font {
            let font_name = |ctx: &Self| {
                ctx.eng.eqtb.fonts.get(f as usize).map(|ff| ff.tfm_name.clone()).unwrap_or_default()
            };
            let Some(steps) = self.vf_font(f).and_then(|vf| vf.chars.get(c as usize).cloned().flatten())
            else {
                self.eng.error(&format!(
                    "Virtual font `{}` has no character packet for slot {c}",
                    font_name(self)
                ));
                return;
            };
            let saved_dvi = if self.eng.engine_kind == crate::engine::EngineKind::XeTeX {
                let saved = self.dpx.cursor;
                self.dpx.cursor = dpx::DviCursor { tex: (x_sp, v_sp), reader: (x_sp, v_sp) };
                Some(saved)
            } else {
                None
            };
            let first_glyph = steps.iter().position(|st| st.rule.is_none());
            for (step_idx, st) in steps.iter().enumerate() {
                if let Some((wd, ht)) = st.rule {
                    // pdf_set_rule(cur_h, cur_v, wd, ht): stands on cur_v
                    let (x, v) = (x_sp + st.dx as i64, v_sp + st.dy as i64);
                    if self.eng.engine_kind == crate::engine::EngineKind::XeTeX {
                        let (x, v) = self.dpx_sync(x, v);
                        self.dpx_track_box(x, v, i64::from(wd), i64::from(ht), 0);
                        self.dpx_rule(x, v, i64::from(wd), i64::from(ht));
                    } else {
                        self.emit_rect_sp(x, v, wd as i64, ht as i64);
                    }
                    continue;
                }
                let base = self.eng.font_loader.vf_bases.get(&f).and_then(|bases| bases.get(st.base as usize));
                let bfid = match base {
                    Some(&bfid) if bfid != u16::MAX => bfid,
                    _ => {
                        let problem = if base.is_some() { "requires missing" } else { "references missing" };
                        let message = format!(
                            "Virtual font `{}` {problem} base font index {}",
                            font_name(self),
                            st.base
                        );
                        self.eng.error(&message);
                        continue;
                    }
                };
                let text = logical_ch.map(|ch| if Some(step_idx) != first_glyph { '\u{00A0}' } else { ch });
                // A base can itself be virtual (notably Korean Hangul).
                // Carry source semantics until reaching a real outline.
                self.emit_char_sp_with_text(
                    bfid,
                    st.ch,
                    x_sp + st.dx as i64,
                    v_sp + st.dy as i64,
                    ratio,
                    text,
                );
            }
            if let Some(cursor) = saved_dvi {
                self.dpx.cursor = cursor;
            }
            return;
        }
        if let Some(ch) = logical_ch {
            let is_cjk_font = self.eng.eqtb.fonts.get(f as usize).is_some_and(|font| {
                font.tfm_name.starts_with("ud")
                    || font.tfm_name.starts_with("ipx")
                    || font.tfm_name.starts_with("cjk")
                    || font.tfm_name.starts_with("song")
                    || font.tfm_name.starts_with("hei")
                    || font.tfm_name.starts_with("kai")
                    || font.tfm_name.starts_with("fs")
                    || font.encoding.as_ref().is_some_and(|e| e.iter().any(|s| s.starts_with("uni") || s.starts_with("u")))
                    || self.eng.font_loader.program_for_font(font).is_ok_and(|p| p.kind != crate::font_program::FontProgramKind::Type1)
            });
            if is_cjk_font {
                let mut utf8 = [0; 4];
                self.emit_cjk_char_sp(f, c, x_sp, v_sp, ratio, ch.encode_utf8(&mut utf8));
                self.cjk_text = None;
                return;
            }
        }
        let base_f = self
            .eng
            .eqtb
            .expand
            .get(f as usize)
            .and_then(|ex| if ex.blink != 0 { Some(ex.blink) } else { None })
            .unwrap_or(f);
        let mut advance = match self.pdf_char_width(f, c) {
            Ok(width) => width,
            Err(error) => {
                self.eng.error(&error);
                return;
            }
        };
        // a Lua font is never cloned: the glyph carries its expansion, and
        // the advance the text matrix scales is that of the expanded glyph
        if ratio != 0 && self.eng.engine_kind == crate::engine::EngineKind::LuaTeX {
            advance = round_xn_over_d(advance, 1000 + i64::from(ratio), 1000);
        }
        self.eng.pdf_doc.record_font_char(base_f as usize, c);
        self.begin_string(x_sp, v_sp, f, crate::pdfout::FontBinding::RAW, ratio);
        push_pdf_char(&mut self.content, c);
        self.adv_char_width(f, advance);
        let x_bp = sp_to_bp(x_sp);
        let y_bp = self.y_pdf(sp_to_bp(v_sp));
        let merged = if let Some(crate::boxes::DisplayItem::GlyphRun {
            font: last_f,
            y_bp: last_y,
            glyphs,
            ..
        }) = self.display_list.items.last_mut()
        {
            if *last_f == f && (*last_y - y_bp).abs() < 1e-3 {
                glyphs.push(c);
                true
            } else {
                false
            }
        } else {
            false
        };
        if !merged {
            self.display_list.push(crate::boxes::DisplayItem::GlyphRun {
                font: f,
                x_bp,
                y_bp,
                glyphs: vec![c],
                tag: None,
                span: None,
            });
        }
    }

    fn begin_hex_string(
        &mut self,
        cur_h: i64,
        cur_v: i64,
        f: u16,
        binding: crate::pdfout::FontBinding,
        ratio: i32,
    ) {
        let mut must_set_text_pos = false;
        if !self.doing_text {
            self.begin_text();
            must_set_text_pos = true;
        }
        if self.pdf_f != f
            || self.cur_font != binding.resource_key(f)
            || (self.doing_string && !self.doing_hex_string)
        {
            self.end_string();
            self.set_font(f, binding);
        }
        let at_size_sp = self
            .eng
            .eqtb
            .fonts
            .get(f as usize)
            .map(|ff| ff.at_size as i64)
            .unwrap_or(0);
        let (_, m) = divide_scaled(at_size_sp, ONE_HUNDRED_BP_SP, 6);
        let gap = cur_h - (self.tj_start_h + self.delta_h);
        let (s, s_out) = if self.cur_tm_a == 0 {
            divide_scaled(gap, m, 3)
        } else {
            let (s, _) = divide_scaled(
                round_xn_over_d(gap, 1000, 1000 + self.cur_tm_a as i64),
                m,
                3,
            );
            let s_out = if s.abs() < GAP_SPLIT_LIMIT {
                let mut o = round_xn_over_d(
                    round_xn_over_d(m, s.abs(), 1000),
                    1000 + self.cur_tm_a as i64,
                    1000,
                );
                if s < 0 {
                    o = -o;
                }
                o
            } else {
                0
            };
            (s, s_out)
        };
        let (v, v_out) = if (cur_v - self.pdf_v).abs() >= self.min_bp_val {
            divide_scaled(self.pdf_v - cur_v, ONE_HUNDRED_BP_SP, self.decimal_digits + 2)
        } else {
            (0, 0)
        };
        if !must_set_text_pos {
            must_set_text_pos = v != 0 || s.abs() >= GAP_SPLIT_LIMIT || ratio != self.cur_tm_a;
        }
        if must_set_text_pos {
            self.end_string();
            self.set_font(f, binding);
            self.set_text_pos(cur_h, cur_v, v, v_out, ratio);
        }
        let s = if must_set_text_pos { 0 } else { s };
        let s_out = if must_set_text_pos { 0 } else { s_out };
        if !self.doing_string {
            self.content.push_str(" [");
            if s == 0 {
                self.content.push('<');
            }
        }
        if s != 0 {
            if self.doing_hex_string {
                self.content.push('>');
            } else if self.doing_string {
                self.content.push(')');
            }
            push_i64(&mut self.content, -s);
            self.content.push('<');
            self.delta_h += s_out;
        }
        self.doing_string = true;
        self.doing_hex_string = true;
    }

    fn native_glyph_nom_advance_sp(&mut self, fid: u16, gid: u16) -> i64 {
        if let Some(&adv) = self.advance_cache.get(&(fid, u32::from(gid))) {
            return adv;
        }
        let at_size_sp = self
            .eng
            .eqtb
            .fonts
            .get(fid as usize)
            .map(|ff| ff.at_size as i64)
            .unwrap_or(0);
        let adv_sp = if at_size_sp <= 0 {
            0
        } else if let Some(program) = self
            .eng
            .eqtb
            .fonts
            .get(fid as usize)
            .and_then(|f| f.native.as_ref())
            .map(|native| native.program.clone())
            .or_else(|| self.eng.lua_font_program(fid))
        {
            if let Ok(face) = program.face() {
                let upem = face.units_per_em() as i64;
                if upem > 0 {
                    let adv = face
                        .glyph_hor_advance(ttf_parser::GlyphId(gid))
                        .unwrap_or(0) as f64;
                    let pdf_width = (adv * 1000.0 / upem as f64).round() as i64;
                    round_xn_over_d(at_size_sp, pdf_width, 1000)
                } else {
                    0
                }
            } else {
                0
            }
        } else {
            0
        };
        self.advance_cache.insert((fid, u32::from(gid)), adv_sp);
        adv_sp
    }


    /// A native word or glyph node: the xdvipdfmx text engine (`dpx_text.rs`).
    fn emit_native_glyph_run_sp(
        &mut self,
        run: &std::rc::Rc<crate::native_layout::NativeRun>,
        start: usize,
        end: usize,
        cur_x: i64,
        y: i64,
    ) {
        self.dpx_native_run(run, start, end, cur_x, y);
    }

    /// pdfTeX `pdf_set_rule`: close the text object, then draw inside a
    /// `q..Q` scope with a temporary origin shift (hairlines stroke).
    fn emit_rect_sp(&mut self, x_sp: i64, v_down_sp: i64, w_sp: i64, h_sp: i64) {
        self.emit_rule_sp(x_sp, v_down_sp, w_sp, h_sp, None);
    }

    /// `emit_rect_sp`, and luatex's outline rule (`Some(stroke width in sp)`,
    /// 0 keeps the current line width) which strokes the rectangle.
    fn emit_rule_sp(&mut self, x_sp: i64, v_down_sp: i64, w_sp: i64, h_sp: i64, outline: Option<i64>) {
        // §624/§633: a rule is drawn only when rule_ht>0 and rule_wd>0
        if w_sp <= 0 || h_sp <= 0 {
            return;
        }
        let lua = self.eng.engine_kind == crate::engine::EngineKind::LuaTeX;
        let x = sp_to_bp(x_sp);
        let y_down = sp_to_bp(v_down_sp);
        let y = self.y_pdf(y_down);
        let w = sp_to_bp(w_sp);
        let h = sp_to_bp(h_sp);
        self.display_list.push(crate::boxes::DisplayItem::Rule {
            x_bp: x,
            y_bp: y,
            width_bp: w,
            height_bp: h,
        });
        self.end_text();
        self.content.push_str("q\n");
        // pdftex.web `pdf_set_rule`: `(h + 1)/2` is Pascal real division and
        // the real argument reaches the scaled parameter truncated toward
        // zero, so an even 0.4pt hairline is centered 13108sp (not 13107sp)
        // above its bottom edge. luatex pdfrule.c rounds `0.5 * size` instead
        // (13107sp) and writes `[] 0 d 0 J `.
        const ONE_BP: i64 = 65782;
        let dash = if lua { "[] 0 d 0 J " } else { "[]0 d 0 J " };
        if h_sp <= ONE_BP {
            let y = if lua { v_down_sp - (h_sp + 1) / 2 } else { (v_down_sp as f64 - (h_sp + 1) as f64 / 2.0) as i64 };
            self.set_origin_temp(x_sp, y);
            self.content.push_str(dash);
            self.push_bp(h_sp);
            self.content.push_str(" w 0 0 m ");
            self.push_bp(w_sp);
            self.content.push_str(" 0 l S\n");
        } else if w_sp <= ONE_BP {
            let x = if lua { x_sp + (w_sp + 1) / 2 } else { (x_sp as f64 + (w_sp + 1) as f64 / 2.0) as i64 };
            self.set_origin_temp(x, v_down_sp);
            self.content.push_str(dash);
            self.push_bp(w_sp);
            self.content.push_str(" w 0 0 m 0 ");
            self.push_bp(h_sp);
            self.content.push_str(" l S\n");
        } else {
            self.set_origin_temp(x_sp, v_down_sp);
            if let Some(stroke) = outline {
                self.content.push_str(dash);
                if stroke > 0 {
                    self.push_bp(stroke);
                    self.content.push_str(" w ");
                }
            }
            self.content.push_str("0 0 ");
            self.push_bp(w_sp);
            self.content.push(' ');
            self.push_bp(h_sp);
            self.content.push_str(if outline.is_some() { " re S\n" } else { " re f\n" });
        }
        self.content.push_str("Q\n");
    }

    /// luatex `pdf_place_rule`: the rule `(width, height, depth, subtype,
    /// index)` of size `w_sp` by `h_sp` (in the orientation of the list)
    /// with its lower left corner at `(x_sp, v_down_sp)`. A user rule is
    /// drawn by the `process_rule` callback, which gets the node and the size;
    /// the math rules become user rules when the callback is registered.
    fn place_rule(&mut self, node: (i32, i32, i32, u8, i32), x_sp: i64, v_down_sp: i64, w_sp: i64, h_sp: i64) {
        use crate::boxes::{RULE_EMPTY, RULE_MATH_OVER, RULE_MATH_RADICAL, RULE_OUTLINE, RULE_USER};
        let (width, height, depth, subtype, index) = node;
        let lua = self.eng.engine_kind == crate::engine::EngineKind::LuaTeX;
        if self.eng.engine_kind == crate::engine::EngineKind::XeTeX {
            // dvi_rule: only rules with both dimensions positive are set
            if w_sp > 0 && h_sp > 0 && subtype != RULE_EMPTY {
                let (x_sp, v_down_sp) = self.dpx_sync(x_sp, v_down_sp);
                self.dpx_track_box(x_sp, v_down_sp, w_sp, h_sp, 0);
                self.dpx_rule(x_sp, v_down_sp, w_sp, h_sp);
            }
            return;
        }
        let callback = self.process_rule_cb != 0;
        let mut s = subtype;
        if lua && (RULE_MATH_OVER..=RULE_MATH_RADICAL).contains(&s) {
            s = if callback { RULE_USER } else { crate::boxes::RULE_NORMAL };
        }
        if s == RULE_EMPTY {
            // only takes space
        } else if lua && s == RULE_USER {
            if callback && w_sp > 0 && h_sp > 0 {
                self.run_process_rule((width, height, depth, subtype, index), x_sp, v_down_sp, w_sp, h_sp);
            }
        } else {
            self.emit_rule_sp(x_sp, v_down_sp, w_sp, h_sp, (lua && s == RULE_OUTLINE).then_some(i64::from(index)));
        }
    }

    /// `q`, move to the rule's lower left corner, run `process_rule` with the
    /// rule node and its size, `Q`; what the callback `pdf.print`s lands in
    /// the page content.
    fn run_process_rule(&mut self, node: (i32, i32, i32, u8, i32), x_sp: i64, v_down_sp: i64, w_sp: i64, h_sp: i64) {
        use crate::lua_callbacks::{Cb, CbArg};
        let (width, height, depth, subtype, index) = node;
        self.end_text();
        self.content.push_str("q\n");
        self.set_origin_temp(x_sp, v_down_sp);
        if self.process_rule_cb > 0 {
            let rule = Node::Rule { width, height, depth, subtype, index, attr: crate::boxes::Attr::NONE };
            let handle = self.eng.lua_nodes_from_engine(vec![rule]) as u32;
            // vlist_out/hlist_out give a rule without a direction the box's (TLT)
            self.eng.lua_nodes.node_mut(handle).f[crate::lua_node_conv::sl::R_DIR] = 0;
            self.eng.lua_tex.pdf_pos = (x_sp as i32, (self.page_height_sp - v_down_sp) as i32);
            self.eng.lua_tex.pdf_print.clear();
            let _ = self.eng.lua_cb_call(Cb::ProcessRule, "process_rule", vec![CbArg::Node(handle), CbArg::Int(w_sp), CbArg::Int(h_sp)]);
            self.eng.lua_nodes.flush_list(handle);
            self.flush_lua_pdf_print(x_sp, v_down_sp);
        }
        self.content.push_str("\nQ\n");
    }
    /// pdfTeX `out_image`: paint image `obj` (`width` by `height`, total
    /// height plus depth) with its lower left corner at (`cur_h`, `cur_v`).
    /// Raster images scale a unit square (4 decimals of bp); included PDF
    /// pages scale their own /BBox (6 decimals) and shift by its origin.
    fn out_image(&mut self, obj: i32, width: i32, height: i32, cur_h: i64, cur_v: i64, transform: u8) {
        use crate::engine::ImageKind;
        let Some(image) = self.eng.pdf_images.get_mut(&obj) else {
            return;
        };
        image.used = true;
        let (img_w, img_h) = if image.rotate == 90 || image.rotate == 270 {
            (image.image_height as i64, image.image_width as i64)
        } else {
            (image.image_width as i64, image.image_height as i64)
        };
        let (kind, group_ref, orig_x, orig_y) =
            (image.kind, image.group_ref, image.orig_x as i64, image.orig_y as i64);
        self.end_text();
        self.content.push_str("q\n");
        if !self.ximage_list.contains(&obj) {
            self.ximage_list.push(obj);
        }
        let (width, height) = (width as i64, height as i64);
        if transform != 0 {
            // LuaTeX pdfimage.c place_img with a rule transform: rotation by quarter turns,
            // mirrored when bit 2 is set
            let is_pdf = kind == ImageKind::Pdf;
            let (mut a0, mut a3) = if is_pdf { (1.0e6 / img_w as f64, 1.0e6 / img_h as f64) } else {
                let s = 1.0e6 / ONE_HUNDRED_BP_SP as f64;
                (s, s)
            };
            let (mut a1, mut a2) = (0.0f64, 0.0f64);
            let (mut xoff, mut yoff) = if is_pdf { (orig_x as f64 / img_w as f64, orig_y as f64 / img_h as f64) } else { (0.0, 0.0) };
            let digits = if is_pdf { 6 } else { 4 };
            let t = i32::from(transform);
            if (t & 7) > 3 {
                a0 = -a0;
                xoff = -xoff;
            }
            match t & 3 {
                1 => {
                    a1 = a0;
                    a2 = -a3;
                    a3 = 0.0;
                    a0 = 0.0;
                    let tmp = yoff;
                    yoff = xoff;
                    xoff = -tmp;
                }
                2 => {
                    a0 = -a0;
                    a3 = -a3;
                    xoff = -xoff;
                    yoff = -yoff;
                }
                3 => {
                    a1 = -a0;
                    a2 = a3;
                    a3 = 0.0;
                    a0 = 0.0;
                    let tmp = yoff;
                    yoff = -xoff;
                    xoff = tmp;
                }
                _ => {}
            }
            let (wd, ht) = (width as f64, height as f64);
            xoff *= wd;
            yoff *= ht;
            let (a0, a1, a2, a3) = (a0 * wd, a1 * ht, a2 * wd, a3 * ht);
            let mut a4 = (cur_h - self.origin_h) as f64 - xoff;
            let mut a5 = (self.origin_v - cur_v) as f64 - yoff;
            let mut k = t;
            if (t & 7) > 3 {
                k += 1;
            }
            match k & 3 {
                1 => a4 += wd,
                2 => {
                    a4 += wd;
                    a5 += ht;
                }
                3 => a5 += ht,
                _ => {}
            }
            let round = |v: f64| (v + 0.5).floor() as i64;
            self.push_real(round(a0), digits);
            self.content.push(' ');
            self.push_real(round(a1), digits);
            self.content.push(' ');
            self.push_real(round(a2), digits);
            self.content.push(' ');
            self.push_real(round(a3), digits);
            self.content.push(' ');
            self.push_bp(round(a4));
            self.content.push(' ');
            self.push_bp(round(a5));
        } else if kind != ImageKind::Pdf {
            if kind == ImageKind::Png && group_ref > 0 && self.eng.pdf_page_group_val == 0 {
                self.eng.pdf_page_group_val = group_ref;
            }
            let sx = ext_xn_over_d(width, 1_000_000, ONE_HUNDRED_BP_SP);
            let sy = ext_xn_over_d(height, 1_000_000, ONE_HUNDRED_BP_SP);
            self.push_real(sx as i64, 4);
            self.content.push_str(" 0 0 ");
            self.push_real(sy as i64, 4);
            self.content.push(' ');
            self.push_bp(cur_h - self.origin_h);
            self.content.push(' ');
            self.push_bp(self.origin_v - cur_v);
        } else {
            // the page group object of a PDF page is numbered on first use
            if group_ref != 0 && self.eng.pdf_page_group_val == 0 {
                self.eng.pdf_page_group_val = if group_ref == -1 {
                    let group = self.eng.alloc_pdf_obj();
                    if let Some(image) = self.eng.pdf_images.get_mut(&obj) {
                        image.group_ref = group;
                    }
                    group
                } else {
                    group_ref
                };
            }
            let sx = ext_xn_over_d(width, 1_000_000, img_w);
            let sy = ext_xn_over_d(height, 1_000_000, img_h);
            self.push_real(sx as i64, 6);
            self.content.push_str(" 0 0 ");
            self.push_real(sy as i64, 6);
            self.content.push(' ');
            self.push_bp(cur_h - self.origin_h - ext_xn_over_d(width, orig_x, img_w) as i64);
            self.content.push(' ');
            self.push_bp(self.origin_v - cur_v - ext_xn_over_d(height, orig_y, img_h) as i64);
        }
        let name = self.eng.pdf_doc.image_names.get(&obj).copied().unwrap_or(obj);
        let prefix = &self.eng.pdf_doc.resname_prefix;
        self.content.push_str(&format!(" cm\n/Im{name}{prefix} Do\nQ\n"));
    }

    /// pdfTeX "Write out pending images" and "Write out pending forms":
    /// images and forms first painted by this page or form.
    fn write_pending_images(&mut self) {
        for index in 0..self.ximage_list.len() {
            let obj = self.ximage_list[index];
            self.eng.write_ximage(obj);
        }
        for index in 0..self.xform_list.len() {
            let obj = self.xform_list[index];
            self.eng.ship_pdf_form(obj);
            self.eng.write_form_procset(obj);
        }
    }

    /// pdfTeX `pdf_image_procset` of the images painted here.
    fn image_procset(&self) -> u8 {
        self.ximage_list
            .iter()
            .filter_map(|obj| self.eng.pdf_images.get(obj))
            .fold(0, |procset, image| procset | image.color)
    }

    fn emit_whatsit_sp(&mut self, w: &crate::boxes::WhatIt, cur_h: i64, cur_v: i64) {
        use crate::boxes::WhatIt::*;
        use crate::boxes::ColorStackCmd;
        match w {
            PdfLiteral { data, origin } => {
                // scan_pdf_origin: 0 = set_origin, 1 = direct (always),
                // 2 = page — pdfTeX `literal()` closes the string/text per
                // mode, then prints the data on its own line.
                match *origin {
                    0 => {
                        self.end_text();
                        self.set_origin(cur_h, cur_v);
                    }
                    1 => self.end_string_nl(),
                    _ => self.end_text(),
                }
                self.content.push_str(data);
                self.content.push('\n');
            }
            PdfColorStack { stack, cmd, data } => {
                // pdfTeX `pdf_out_colorstack`
                let stack_no = *stack as usize;
                let page_mode = self.page_mode;
                let Some(cs) = self.eng.color_stacks.get_mut(stack_no) else {
                    self.eng.warning_at(
                        &format!("Color stack {stack} is not initialized for use!"),
                        None,
                    );
                    return;
                };
                let mode = cs.literal_mode;
                let out = match cmd {
                    ColorStackCmd::Set => {
                        *cs.current_mut(page_mode) = data.clone();
                        data.clone()
                    }
                    ColorStackCmd::Push => {
                        cs.push(page_mode, data.clone());
                        data.clone()
                    }
                    ColorStackCmd::Pop => match cs.pop(page_mode) {
                        Some(current) => current,
                        None => {
                            let kind = if page_mode { "page" } else { "form" };
                            self.eng.warning_at(
                                &format!("pop empty color {kind} stack {stack}"),
                                None,
                            );
                            return;
                        }
                    },
                    ColorStackCmd::Current => cs.current_mut(page_mode).clone(),
                };
                if !out.is_empty() {
                    self.colorstack_literal(&out, mode, cur_h, cur_v);
                }
            }
            PdfRefXImage { obj, w, h, d, transform } => {
                self.out_image(*obj, *w, *h + *d, cur_h, cur_v + *d as i64, *transform)
            }
            PdfSnapRefPoint => self.eng.pdf_snap_refpos = (cur_h, cur_v),
            XePic { .. } => {
                // xetex.web `pic_out`: the node becomes a `pdf:image` special
                let saved = self.dpx.cursor.tex;
                let text = crate::xetex_pic::pic_out_text(w);
                self.handle_special(&text, cur_h, cur_v);
                self.dpx.cursor.tex = saved;
            }
            PdfRefXForm { obj, d, .. } => {
                if !self.xform_list.contains(obj) {
                    self.xform_list.push(*obj);
                }
                self.end_text();
                self.content.push_str("q\n1 0 0 1 ");
                self.push_bp(cur_h - self.origin_h);
                self.content.push(' ');
                // pdftex.web out_form: `cur_v := cur_v + obj_xform_depth`
                self.push_bp(self.origin_v - cur_v - i64::from(*d));
                let name = self.eng.pdf_doc.form_names.get(obj).copied().unwrap_or(*obj);
                let prefix = &self.eng.pdf_doc.resname_prefix;
                self.content.push_str(&format!(" cm\n/Fm{name}{prefix} Do\nQ\n"));
            }
            PdfSetMatrix { matrix, source } => {
                // pdfTeX `pdf_out_setmatrix` + `pdfsetmatrix` (utils.c §1406):
                // a valid matrix is exactly four numbers; the emitted
                // literal is `set_origin` mode, so the CTM first moves to
                // the current pen and the supplied matrix concatenates at
                // that origin. Malformed input emits no content at all
                // (canonical `\pdfsetmatrix` "Unrecognized format." error).
                // pdfTeX echoes the raw token string on success. We echo it
                // too when every entry is plain PDF number syntax, and
                // otherwise (`1e3`, `inf`, ...) emit the parsed numbers
                // canonicalized, so a malformed stream can never slip
                // through a token-level parse.
                let Some([a, b, c, d]) = parse_matrix(matrix) else {
                    self.end_text();
                    let message = format!(
                        "Invalid \\pdfsetmatrix value; expected exactly four finite numbers; got `{matrix}`"
                    );
                    self.eng.fatal_error_at(
                        &message,
                        source.as_deref().map(crate::input::SourceMark::to_context),
                    );
                    return;
                };
                // utils.c §1414: the stack accumulates in page mode only
                // (forms have no annotation geometry to correct). §1420:
                // e/f anchor the transform at the pen in bottom-origin sp
                // (`cur_page_height - cur_v`). §1424: pdfTeX's row-vector
                // multiplication order, new matrix x top of stack.
                if self.page_mode {
                    let v_up = self.page_height_sp - cur_v;
                    let e = cur_h as f64 * (1.0 - a) - v_up as f64 * c;
                    let f = v_up as f64 * (1.0 - d) - cur_h as f64 * b;
                    let top = self.matrix_stack.last().copied();
                    self.matrix_stack.push(match top {
                        Some(y0) => Matrix {
                            a: a * y0.a + b * y0.c,
                            b: a * y0.b + b * y0.d,
                            c: c * y0.a + d * y0.c,
                            d: c * y0.b + d * y0.d,
                            e: e * y0.a + f * y0.c + y0.e,
                            f: e * y0.b + f * y0.d + y0.f,
                        },
                        None => Matrix { a, b, c, d, e, f },
                    });
                }
                self.end_text();
                self.set_origin(cur_h, cur_v);
                if matrix.split_ascii_whitespace().all(is_pdf_number) {
                    self.content.push_str(matrix);
                } else {
                    let mut buf = String::new();
                    for v in [a, b, c, d] {
                        if !buf.is_empty() {
                            buf.push(' ');
                        }
                        push_matrix_num(&mut buf, v);
                    }
                    self.content.push_str(&buf);
                }
                self.content.push_str(" 0 0 cm\n");
            }
            PdfSave { source } => {
                // pdfTeX `pdf_out_save`: `checkpdfsave(cur_h, cur_v)` then
                // `literal("q", set_origin)` (utils.c §1319). The save point
                // is pushed unconditionally (page or form); the matrix depth
                // only carries page-mode meaning.
                self.pos_stack.push(SavePoint {
                    pos_h: cur_h,
                    pos_v: cur_v,
                    matrix_depth: self.matrix_stack.len(),
                    source: source.as_deref().cloned(),
                });
                self.end_text();
                self.set_origin(cur_h, cur_v);
                self.content.push_str("q\n");
            }
            PdfRestore { source } => {
                // pdfTeX `checkpdfrestore` (utils.c §1339): an unmatched
                // restore only warns; skip the `Q` entirely so the stream
                // never carries a state-pop below the stack (an unbalanced
                // Q is a malformed PDF). A matched restore unwinds the
                // accumulated matrix to the depth saved by `\pdfsave`.
                if self.pos_stack.last().is_none() {
                    self.eng.warning_at(
                        "Unmatched \\pdfrestore: no preceding \\pdfsave exists in this shipped box",
                        source.as_deref().map(crate::input::SourceMark::to_context),
                    );
                    return;
                }
                let sp = self.pos_stack.pop().expect("non-empty above");
                let (diff_h, diff_v) = (cur_h - sp.pos_h, cur_v - sp.pos_v);
                if diff_h != 0 || diff_v != 0 {
                    self.eng.warning_at(
                        &format!(
                            "Misplaced \\pdfrestore: position changed by ({diff_h}sp, {diff_v}sp) since the matching \\pdfsave"
                        ),
                        source
                            .as_deref()
                            .map(crate::input::SourceMark::to_context),
                    );
                }
                if self.page_mode {
                    self.matrix_stack.truncate(sp.matrix_depth);
                }
                self.end_text();
                self.set_origin(cur_h, cur_v);
                self.content.push_str("Q\n");
            }
            SyncPoint { file_id, line } => {
                if self.page_mode && self.eng.synctex_active() {
                    let page = (self.eng.pdf_doc.pages.len() + 1) as u32;
                    self.eng.synctex.record_point(page, *file_id, *line, cur_h, cur_v);
                }
            }
            PdfDest { id, kind, params } => {
                // pdftex.web `do_dest`: the first shipped definition of an
                // identifier wins; later ones warn and are ignored
                let duplicate = if self.page_mode {
                    !self.eng.pdf_doc.shipped_dests.insert(id.clone())
                } else {
                    self.dests.iter().any(|d| &d.id == id)
                };
                if duplicate && self.page_mode {
                    self.eng.warn_dest_dup(id);
                }
                if !duplicate {
                    // explicit coordinates are page-absolute sp from the
                    // bottom-left corner; the sentinel -32768 keeps the
                    // anchor position
                    let ax_sp = cur_h;
                    let ay_sp = self.page_height_sp - cur_v;
                    let pv = |i: usize, anchor: i64| {
                        if params[i] == crate::pdfout::PDF_POS_CURRENT {
                            (anchor, true)
                        } else {
                            (params[i] as i64, false)
                        }
                    };
                    // anchor position: x from the pen, y from the baseline
                    // XYZ: left top zoom; FitH/FitBH: top; FitV/FitBV: left;
                    // FitR: left bottom right top
                    let ((mut px, fx), (mut py, fy)) = match kind {
                        2 | 5 => ((ax_sp, true), pv(0, ay_sp)),
                        3 | 6 => (pv(0, ax_sp), (ay_sp, true)),
                        7 => (pv(0, ax_sp), pv(3, ay_sp)),
                        _ => (pv(0, ax_sp), pv(1, ay_sp)),
                    };
                    // pdfTeX `do_dest` with an active matrix runs the anchor
                    // through `set_rect_dimens` + `matrixtransformrect`
                    // (pdftex.web §36720): degenerate to the pen point, that
                    // is `matrixtransformpoint` on the sp raster. Explicit
                    // coordinates are page-absolute and stay untouched.
                    if let Some(m) = self.matrix_used() {
                        if fx || fy {
                            let (tx, ty) = matrix_transform_point(m, px as f64, py as f64);
                            if fx {
                                px = tx as i64;
                            }
                            if fy {
                                py = ty as i64;
                            }
                        }
                    }
                    let (px, py) = (sp_to_bp(px), sp_to_bp(py));
                    let zm = if *kind == 0 && params[2] > 0 {
                        Some(params[2] as f64 / 1000.0)
                    } else {
                        None
                    };
                    self.dests.push(crate::pdfout::Dest {
                        id: id.clone(),
                        x: px,
                        y: py,
                        kind: *kind,
                        zoom: zm,
                    });
                }
            }
            PdfAnnot { attr, wd, ht, dp } => {
                // pdfTeX `do_annot` -> `set_rect_dimens` (pdftex.web §36430)
                // with running dimensions from the enclosing box, no margin
                let (_, rect) = self.set_rect_dimens(cur_h, cur_v, (*wd, *ht, *dp), 0);
                self.annots.push(Annot {
                    rect,
                    uri: None,
                    dest: None,
                    attr: attr.clone(),
                    subtype: None,
                });
            }
            PdfStartLink { attr, uri, name, wd, ht, dp } => {
                // pdfTeX `do_link`: links exist only on shipped pages
                if self.page_mode {
                    let mut link = crate::pdfout::OpenLink {
                        nesting: self.cur_s,
                        dims: (*wd, *ht, *dp),
                        uri: uri.clone(),
                        dest: name.clone(),
                        attr: attr.clone(),
                        annot: None,
                        raw: [0; 4],
                    };
                    self.start_link_annot(&mut link, cur_h, cur_v);
                    self.eng.pdf_doc.link_stack.push(link);
                }
            }
            PdfEndLink => {
                if self.page_mode {
                    self.end_link(cur_h);
                }
            }
            Special(s) => {
                self.handle_special(&crate::tex_bytes::text_to_display(s), cur_h, cur_v);
            }
            LateLua { code, func } => self.run_late_lua(code, *func, cur_h, cur_v),
            SavePos { .. } => {
                // position is relative to the page edges, in sp
                self.eng.pdf_last_x = cur_h as i32;
                self.eng.pdf_last_y = (self.page_height_sp - cur_v) as i32;
            }
            Write {
                stream,
                tokens,
                source,
            } => {
                let toks = tokens.clone();
                let src = source.clone();
                self.eng.fire_write(*stream, &toks, src.as_deref());
            }
            OpenOut {
                stream,
                names,
                create_parent,
                source,
            } => {
                let p = names.0.clone();
                let src = source.clone();
                self.eng
                    .exec_openout(*stream, &p, *create_parent, src.as_deref());
            }
            CloseOut { stream, source } => {
                let src = source.clone();
                self.eng.exec_closeout(*stream, src.as_deref());
            }
            CjkText(text) => {
                self.cjk_text = *text;
            }
            // pdftex.web hlist/vlist out: `gen_running_link := on`
            PdfRunningLink(on) => {
                self.eng.pdf_doc.gen_running_link = *on;
            }
            _ => {}
        }
    }
    fn handle_special(&mut self, text: &str, cur_h: i64, cur_v: i64) {
        if self.eng.engine_kind == crate::engine::EngineKind::XeTeX {
            let (cur_h, cur_v) = self.dpx_sync(cur_h, cur_v);
            return self.dpx_special(text, cur_h, cur_v);
        }
        self.handle_special_legacy(text, cur_h, cur_v);
    }

    fn handle_special_legacy(&mut self, text: &str, _cur_h: i64, _cur_v: i64) {
        use std::fmt::Write;
        let trimmed = text.trim();
        if let Some(content) = trimmed
            .strip_prefix("pdf:literal")
            .or_else(|| trimmed.strip_prefix("pdf:code"))
        {
            let payload = content.trim();
            let payload = payload.strip_prefix("direct").unwrap_or(payload).trim();
            self.end_text();
            self.content.push_str(payload);
            self.content.push('\n');
        } else if let Some(spec) = trimmed.strip_prefix("color push") {
            let spec = spec.trim();
            self.end_text();
            if let Some(rest) = spec.strip_prefix("rgb") {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if parts.len() >= 3 {
                    let _ = writeln!(
                        self.content,
                        "{} {} {} rg {} {} {} RG",
                        parts[0], parts[1], parts[2], parts[0], parts[1], parts[2]
                    );
                }
            } else if let Some(rest) = spec.strip_prefix("gray") {
                let g = rest.trim();
                let _ = writeln!(self.content, "{g} g {g} G");
            } else if let Some(rest) = spec.strip_prefix("cmyk") {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if parts.len() >= 4 {
                    let _ = writeln!(
                        self.content,
                        "{} {} {} {} k {} {} {} {} K",
                        parts[0], parts[1], parts[2], parts[3], parts[0], parts[1], parts[2], parts[3]
                    );
                }
            }
        } else if trimmed == "color pop" {
            self.end_text();
            self.content.push_str("0 g 0 G\n");
        } else if let Some(rest) = trimmed.strip_prefix("x:scale") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 2 {
                let sx: f64 = parts[0].parse().unwrap_or(1.0);
                let sy: f64 = parts[1].parse().unwrap_or(1.0);
                self.end_text();
                let _ = writeln!(self.content, "{sx:.4} 0 0 {sy:.4} 0 0 cm");
            }
        } else if let Some(rest) = trimmed.strip_prefix("x:rotate") {
            let deg: f64 = rest.trim().parse().unwrap_or(0.0);
            let rad = deg.to_radians();
            let cos = rad.cos();
            let sin = rad.sin();
            self.end_text();
            let _ = writeln!(self.content, "{cos:.4} {sin:.4} {:.4} {cos:.4} 0 0 cm", -sin);
        } else if trimmed == "x:gsave" {
            self.end_text();
            self.content.push_str("q\n");
        } else if trimmed == "x:grestore" {
            self.end_text();
            self.content.push_str("Q\n");
        }
    }
}

impl<'a> RenderCtx<'a> {
    /// `\latelua`: run the code (or Lua function) at the node's position, then
    /// put what `pdf.print` wrote into the content stream the way lpdflib.c
    /// `luapdfprint` does: the literal mode first closes the text or string (or
    /// moves the origin), then the text follows verbatim.
    fn run_late_lua(&mut self, code: &[u8], func: i32, cur_h: i64, cur_v: i64) {
        // pdf.getpos: the position in sp from the page's bottom left, as \pdfsavepos
        self.eng.lua_tex.pdf_pos = (cur_h as i32, (self.page_height_sp - cur_v) as i32);
        self.eng.lua_tex.pdf_print.clear();
        self.eng.lua_tex.in_late_lua = true;
        if func > 0 {
            self.eng.call_lua_function(func);
        } else if let Err(err) = self.eng.execute_directlua(code) {
            self.eng.error(&format!("LuaTeX error: {err}"));
        }
        self.eng.lua_tex.in_late_lua = false;
        self.flush_lua_pdf_print(cur_h, cur_v);
    }

    /// Put what `pdf.print` queued into the content stream, as lpdflib.c
    /// `luapdfprint` does; `(cur_h, cur_v)` is the position the default
    /// (origin) mode moves to.
    fn flush_lua_pdf_print(&mut self, cur_h: i64, cur_v: i64) {
        for (mode, text) in std::mem::take(&mut self.eng.lua_tex.pdf_print) {
            match mode {
                0 => {
                    self.end_text();
                    self.set_origin(cur_h, cur_v);
                }
                1 => self.end_text(),
                2 => {
                    if !self.doing_text {
                        self.begin_text();
                    }
                }
                _ => self.end_string_nl(),
            }
            self.content.push_str(&String::from_utf8_lossy(&text));
        }
    }
}
