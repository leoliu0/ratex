//! XeTeX's native glyphs as xdvipdfmx writes them: `do_glyphs` of dvi.c
//! (colour and opacity around the glyph array, ActualText, box tracking) on
//! top of the text engine of pdfdev.c (`pdf_dev_set_string`,
//! `pdf_dev_set_font`, `start_string`, `dev_set_text_matrix`, text/graphics
//! mode switches). Positions are computed in sp of the DVI frame exactly as
//! dvipdfmx does and printed with `p_dtoa`, so the TJ adjustments and the
//! rounding-error compensation of `Td` come out the same.

use super::{RenderCtx, ONE_HUNDRED_BP_SP};
use crate::dpx_font::NativeMetrics;
use crate::native_layout::NativeRun;
use std::rc::Rc;

/// pdfdev.c: `pdf_init_device(dvi2pts, precision = 3, ..)`
const PRECISION: usize = 3;
/// `min_bp_val = ROUND(1.0/(ten_pow[precision]*dvi2pts), 1)`
const MIN_BP_VAL: i64 = 66;

/// dvi.c `do_scales`: `unit_num / unit_den * 72 / 254000` of XeTeX's
/// preamble (25400000 / 473628672).
fn dvi2pts() -> f64 {
    let d = 25400000.0f64 / 473628672.0;
    d * (72.0 / 254000.0)
}

/// `p_dtoa(value, prec)` as the scaled integer `value * 10^prec`.
fn scaled(value: f64, prec: usize) -> i64 {
    let p = 10f64.powi(prec as i32);
    let (neg, v) = if value < 0.0 { (true, -value) } else { (false, value) };
    let mut i = v.trunc();
    let mut g = ((v - i) * p + 0.5) as i64;
    if g == p as i64 {
        g = 0;
        i += 1.0;
    }
    let n = i as i64 * p as i64 + g;
    if neg { -n } else { n }
}

/// Print a scaled integer like `p_dtoa` prints its double.
fn push_scaled(out: &mut String, n: i64, prec: usize) {
    let p = 10i64.pow(prec as u32);
    if n == 0 {
        out.push('0');
        return;
    }
    let (neg, a) = (n < 0, n.abs());
    if neg {
        out.push('-');
    }
    let (i, g) = (a / p, a % p);
    if i != 0 {
        out.push_str(&i.to_string());
    }
    if g != 0 {
        out.push('.');
        let digits = format!("{g:0prec$}");
        out.push_str(digits.trim_end_matches('0'));
    }
}

/// C `ROUND(v, acc)`
fn round_acc(v: f64, acc: f64) -> f64 {
    (v / acc + 0.5).floor() * acc
}

/// `dev_sprint_bp`: the number text of `value` (sp) and its rounding error
/// in sp. `origin`: added to the printed value (the DVI origin in the text
/// line matrix when the stream does not translate to it).
fn sprint_bp(out: &mut String, value: i64, origin_milli: i64) -> i64 {
    let bp = value as f64 * dvi2pts();
    let err_bp = bp - round_acc(bp, 0.001);
    push_scaled(out, scaled(bp, PRECISION) + origin_milli, PRECISION);
    (err_bp / dvi2pts()).round() as i64
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Motion {
    Graphics,
    Text,
    Str,
}

/// `TEXT_WMODE_HH` and `TEXT_WMODE_VH` (XeTeX's directions are horizontal)
const WMODE_HH: i32 = 0;
const WMODE_VH: i32 = 4;

/// A `dev_font`: a native font instance of one size.
#[derive(Clone, Copy, Debug)]
struct DevFont {
    rep: u16,
    sptsize: i64,
    extend: f64,
    slant: f64,
    bold: f64,
    wmode: bool,
}

/// `pdev.text_state` and the device fonts of the page.
pub(crate) struct DpxText {
    mode: Motion,
    font: Option<usize>,
    offset: i64,
    ref_x: i64,
    ref_y: i64,
    slant: f64,
    extend: f64,
    rotate: i32,
    bold_param: f64,
    force_reset: bool,
    /// BT was written without a text matrix: the first Td carries the DVI
    /// origin when the content stream is not translated to it
    origin_pending: bool,
    fonts: Vec<DevFont>,
    /// The DVI origin: sp from the context's origin and printed milli-bp of
    /// the line matrix translation.
    org_sp: (i64, i64),
    org_milli: (i64, i64),
}

impl DpxText {
    pub(crate) fn new() -> DpxText {
        DpxText {
            mode: Motion::Graphics,
            font: None,
            offset: 0,
            ref_x: 0,
            ref_y: 0,
            slant: 0.0,
            extend: 1.0,
            rotate: WMODE_HH,
            bold_param: 0.0,
            force_reset: false,
            origin_pending: false,
            fonts: Vec::new(),
            org_sp: (0, 0),
            org_milli: (0, 0),
        }
    }

    /// Place the DVI origin at `(x_sp, y_sp)` (sp, y down) in the frame of
    /// the content stream, `(x_milli, y_milli)` being its coordinates in
    /// milli-bp, y up.
    pub(crate) fn set_origin(&mut self, org_sp: (i64, i64), org_milli: (i64, i64)) {
        self.org_sp = org_sp;
        self.org_milli = org_milli;
    }

    pub(crate) fn in_text(&self) -> bool {
        self.mode != Motion::Graphics
    }
}

fn utf16_pdf_string(units: &[u16]) -> String {
    // pdf_dev_begin_actualtext: PDFDocEncoding when every unit is below
    // 0x100 and outside 0x80..0xA1, else UTF-16BE with a BOM; bytes above
    // 0x7e are written as octal escapes (equivalent to the raw bytes)
    let doc = units.iter().all(|&u| u <= 0xff && !(u > 0x7f && u < 0xa1));
    let mut s = String::new();
    let put = |c: u8, s: &mut String| {
        if c == b'(' || c == b')' || c == b'\\' {
            s.push('\\');
            s.push(c as char);
        } else if c < b' ' || c > 0x7e {
            s.push_str(&format!("\\{c:03o}"));
        } else {
            s.push(c as char);
        }
    };
    if !doc {
        s.push_str("\\376\\377");
    }
    for &u in units {
        if !doc {
            put((u >> 8) as u8, &mut s);
        }
        put((u & 0xff) as u8, &mut s);
    }
    s
}

impl<'a> RenderCtx<'a> {
    // ----------------------------------------------------------- fonts

    /// The metrics of a native font (cached for the job).
    fn native_metrics(&mut self, fid: u16) -> Option<Rc<NativeMetrics>> {
        let nf = self.eng.eqtb.fonts.get(fid as usize)?.native.clone()?;
        let key = (nf.program.content_hash, nf.program.face_index, nf.vertical);
        if let Some(m) = self.eng.pdf_doc.xe_metrics.get(&key) {
            return Some(m.clone());
        }
        match NativeMetrics::read(&nf.program.data, nf.program.face_index, nf.vertical) {
            Ok(m) => {
                let m = Rc::new(m);
                self.eng.pdf_doc.xe_metrics.insert(key, m.clone());
                Some(m)
            }
            Err(e) => {
                self.eng.error(&format!("Cannot read the metrics of native font `{}`: {e}", nf.full_name));
                None
            }
        }
    }

    /// `pdf_dev_locate_font`: the device font of native font `fid` (same
    /// program, direction and synthetic options share a PDF font).
    fn dpxt_locate_font(&mut self, fid: u16, nf: &crate::native_font::NativeFont, sptsize: i64) -> usize {
        let ext = crate::native_font::d2fix(nf.extend as f64);
        let slant = crate::native_font::d2fix(nf.slant as f64);
        let bold = crate::native_font::d2fix(nf.embolden as f64);
        let rep = match self.eng.pdf_doc.xe_fid_rep.get(&fid) {
            Some(&rep) => rep,
            None => {
                let key = crate::pdfout::XeGroupKey {
                    hash: nf.program.content_hash,
                    face_index: nf.program.face_index,
                    variations: nf.program.variations.iter().map(|(t, v)| (u32::from_be_bytes(t.to_bytes()), v.to_bits())).collect(),
                    vertical: nf.vertical,
                    extend: ext,
                    slant,
                    embolden: bold,
                };
                self.eng.pdf_doc.xe_group_rep(fid, key)
            }
        };
        if let Some(i) = self.dpxt.fonts.iter().position(|f| f.rep == rep && f.sptsize == sptsize) {
            return i;
        }
        self.dpxt.fonts.push(DevFont {
            rep,
            sptsize,
            extend: ext as f64 / 65536.0,
            slant: slant as f64 / 65536.0,
            bold: bold as f64 / 65536.0,
            wmode: nf.vertical,
        });
        self.dpxt.fonts.len() - 1
    }

    // -------------------------------------------------- text state machine

    fn dpxt_set_text_matrix(&mut self, xpos: i64, ypos: i64, slant: f64, extend: f64, rotate: i32) {
        let (a, b, c, d) = match rotate {
            WMODE_VH => (slant, 1.0, -extend, 0.0),
            _ => (extend, 0.0, slant, 1.0),
        };
        let out = &mut self.content;
        out.push(' ');
        for (i, v) in [a, b, c, d].into_iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            push_scaled(out, scaled(v, PRECISION + 2), PRECISION + 2);
        }
        let k = dvi2pts();
        out.push(' ');
        push_scaled(out, scaled(xpos as f64 * k, PRECISION) + self.dpxt.org_milli.0, PRECISION);
        out.push(' ');
        push_scaled(out, scaled(ypos as f64 * k, PRECISION) + self.dpxt.org_milli.1, PRECISION);
        out.push_str(" Tm");
        let t = &mut self.dpxt;
        t.ref_x = xpos;
        t.ref_y = ypos;
        t.slant = slant;
        t.extend = extend;
        t.rotate = rotate;
        t.origin_pending = false;
    }

    fn dpxt_reset_text_state(&mut self) {
        self.content.push_str(" BT");
        let t = &self.dpxt;
        if t.force_reset || t.slant != 0.0 || t.extend != 1.0 || (t.rotate != WMODE_HH && t.rotate != 5) {
            let (s, e, r) = (t.slant, t.extend, t.rotate);
            self.dpxt_set_text_matrix(0, 0, s, e, r);
        } else {
            self.dpxt.origin_pending = true;
        }
        let t = &mut self.dpxt;
        t.ref_x = 0;
        t.ref_y = 0;
        t.offset = 0;
        t.force_reset = false;
    }

    fn dpxt_text_mode(&mut self) {
        match self.dpxt.mode {
            Motion::Text => {}
            Motion::Str => self.content.push_str(">]TJ"),
            Motion::Graphics => self.dpxt_reset_text_state(),
        }
        self.dpxt.mode = Motion::Text;
        self.dpxt.offset = 0;
    }

    /// `pdf_dev_graphics_mode`; also the hook of `end_text`.
    pub(super) fn dpxt_graphics_mode(&mut self) {
        match self.dpxt.mode {
            Motion::Graphics => return,
            Motion::Str => {
                self.content.push_str(">]TJ");
                self.dpxt_leave_text();
            }
            Motion::Text => self.dpxt_leave_text(),
        }
        self.dpxt.mode = Motion::Graphics;
    }

    fn dpxt_leave_text(&mut self) {
        if self.dpxt.bold_param != 0.0 {
            self.content.push_str(" 0 Tr");
            self.dpxt.bold_param = 0.0;
        }
        self.content.push_str(" ET\n");
        self.dpxt.force_reset = false;
        self.dpxt.font = None;
    }

    fn dpxt_start_string(&mut self, xpos: i64, ypos: i64, slant: f64, extend: f64, rotate: i32) {
        let delx = xpos - self.dpxt.ref_x;
        let dely = ypos - self.dpxt.ref_y;
        let mut org = if self.dpxt.origin_pending { self.dpxt.org_milli } else { (0, 0) };
        if self.dpxt.origin_pending {
            self.dpxt.origin_pending = false;
        }
        let (err_x, err_y);
        self.content.push(' ');
        match rotate {
            WMODE_VH => {
                let desired_x = dely;
                let desired_y = (-(delx as f64 - dely as f64 * slant) / extend) as i64;
                let mut s = String::new();
                err_y = sprint_bp(&mut s, desired_x, 0);
                s.push(' ');
                let e = sprint_bp(&mut s, desired_y, 0);
                err_x = -e;
                self.content.push_str(&s);
            }
            _ => {
                let desired_x = ((delx as f64 - dely as f64 * slant) / extend) as i64;
                let desired_y = dely;
                let mut s = String::new();
                err_x = sprint_bp(&mut s, desired_x, std::mem::take(&mut org.0));
                s.push(' ');
                err_y = sprint_bp(&mut s, desired_y, std::mem::take(&mut org.1));
                self.content.push_str(&s);
            }
        }
        self.content.push_str(" Td[<");
        self.dpxt.ref_x = xpos - err_x;
        self.dpxt.ref_y = ypos - err_y;
        self.dpxt.offset = 0;
    }

    fn dpxt_string_mode(&mut self, xpos: i64, ypos: i64, slant: f64, extend: f64, rotate: i32) {
        if self.dpxt.mode == Motion::Graphics {
            self.dpxt_reset_text_state();
        }
        if self.dpxt.mode != Motion::Str {
            if self.dpxt.force_reset {
                self.dpxt_set_text_matrix(xpos, ypos, slant, extend, rotate);
                self.content.push_str("[<");
                self.dpxt.force_reset = false;
            } else {
                self.dpxt_start_string(xpos, ypos, slant, extend, rotate);
            }
        }
        self.dpxt.mode = Motion::Str;
    }

    /// `pdf_dev_set_font`
    fn dpxt_set_font(&mut self, idx: usize) {
        self.dpxt_text_mode();
        let f = self.dpxt.fonts[idx];
        // autorotate: the direction stays horizontal (dir_mode 0)
        let text_rotate = i32::from(f.wmode) << 2;
        let t = &mut self.dpxt;
        if f.slant != t.slant || f.extend != t.extend || ((text_rotate - t.rotate).abs() % 5) != 0 {
            t.force_reset = true;
        }
        t.slant = f.slant;
        t.extend = f.extend;
        t.rotate = text_rotate;
        let num = self.ensure_font(f.rep, crate::pdfout::FontBinding::remapped(0));
        let mut s = format!(" /F{num}{} ", self.eng.pdf_doc.resname_prefix);
        push_scaled(&mut s, scaled(f.sptsize as f64 * dvi2pts(), PRECISION + 1), PRECISION + 1);
        s.push_str(" Tf");
        self.content.push_str(&s);
        if f.bold > 0.0 || f.bold != self.dpxt.bold_param {
            if f.bold <= 0.0 {
                self.content.push_str(" 0 Tr");
            } else {
                self.content.push_str(&format!(" 2 Tr {:.6} w", f.bold));
            }
        }
        self.dpxt.bold_param = f.bold;
        self.dpxt.font = Some(idx);
    }

    /// `pdf_dev_set_string` for one glyph: `xpos`/`ypos` in the DVI frame
    /// (y up), `width` the nominal advance.
    fn dpxt_set_string(&mut self, xpos: i64, ypos: i64, gid: u16, width: i64, idx: usize) {
        if self.dpxt.font != Some(idx) {
            self.dpxt_set_font(idx);
        }
        let f = self.dpxt.fonts[idx];
        self.eng.pdf_doc.xe_note_glyph(f.rep, gid);
        let text_xorigin = self.dpxt.ref_x;
        let text_yorigin = self.dpxt.ref_y;
        let delh = text_xorigin + self.dpxt.offset - xpos;
        let delv = ypos - text_yorigin;
        let word_space_max = (3.0 * f.extend * f.sptsize as f64) as i64;
        let kern: i64;
        if self.dpxt.force_reset || delv.abs() > MIN_BP_VAL || delh.abs() > word_space_max {
            self.dpxt_text_mode();
            kern = 0;
        } else {
            kern = (1000.0 / f.extend * delh as f64 / f.sptsize as f64) as i32 as i64;
        }
        if self.dpxt.mode != Motion::Str {
            let (s, e, r) = (f.slant, f.extend, self.dpxt.rotate);
            self.dpxt_string_mode(xpos, ypos, s, e, r);
        } else if kern != 0 {
            self.dpxt.offset -= (kern as f64 * f.extend * (f.sptsize as f64 / 1000.0)) as i32 as i64;
            self.content.push('>');
            self.content.push_str(&(if f.wmode { -kern } else { kern }).to_string());
            self.content.push('<');
        }
        self.content.push_str(&format!("{gid:04x}"));
        self.dpxt.offset += width;
    }

    // ------------------------------------------------------------ do_glyphs

    /// The `NativeGlyphRun` node at pen `cur_x`, baseline `y` (sp in the
    /// context frame, y down): dvi.c `do_glyphs`.
    pub(super) fn dpx_native_run(&mut self, run: &Rc<NativeRun>, start: usize, end: usize, cur_x: i64, y: i64) {
        if start >= end || start >= run.glyphs.len() {
            return;
        }
        let end = end.min(run.glyphs.len());
        let fid = run.font;
        let Some(nf) = self.eng.eqtb.fonts.get(fid as usize).and_then(|f| f.native.clone()) else {
            return;
        };
        let sptsize = self.eng.eqtb.fonts[fid as usize].at_size as i64;
        if sptsize <= 0 {
            return;
        }
        let Some(metrics) = self.native_metrics(fid) else { return };
        self.end_pdftex_text();
        self.display_list.push(crate::boxes::DisplayItem::NativeGlyphRun {
            run: run.clone(),
            start,
            end,
            x_bp: super::sp_to_bp(cur_x),
            y_bp: self.y_pdf(super::sp_to_bp(y)),
            tag: None,
            span: None,
        });
        let idx = self.dpxt_locate_font(fid, &nf, sptsize);
        let f = self.dpxt.fonts[idx];

        let actual_text = run.actual_text && !run.is_glyph_node();
        if actual_text {
            let units: Vec<u16> = run.text.encode_utf16().collect();
            self.dpxt_graphics_mode();
            self.content.push_str("\n/Span << /ActualText (");
            self.content.push_str(&utf16_pdf_string(&units));
            self.content.push_str(") >> BDC");
        }
        let colored = nf.colored;
        if colored {
            let c = |shift: u32| f64::from((nf.rgba >> shift) & 0xff) / 255.0;
            self.dpx_push_text_color(c(24), c(16), c(8));
            // the transparency ExtGState of dvi.c do_fnt (ca/CA = alpha / 255)
            let alpha = f64::from(nf.rgba & 0xff) / 255.0;
            let name = format!("Xtx_Gs_{:08x}", fid);
            use crate::dpx_obj::Obj;
            self.dpx_put_resource(
                "ExtGState",
                Obj::Dict(vec![(
                    name.clone(),
                    Obj::Dict(vec![
                        ("Type".into(), Obj::Name("ExtGState".into())),
                        ("ca".into(), Obj::Num(alpha)),
                        ("CA".into(), Obj::Num(alpha)),
                    ]),
                )]),
            );
            self.dpxt_graphics_mode();
            self.dpx_gsave();
            self.dpx_emit(&format!(" /{name} gs "));
        }
        let tracking = self.dpx_tracking();
        let mut xloc = 0i64;
        for (k, g) in run.glyphs[start..end].iter().enumerate() {
            if k == 0 {
                xloc = i64::from(g.x_offset);
            }
            let gid = g.glyph_id;
            let mut advance = 0i64;
            if gid < metrics.num_glyphs {
                advance = (sptsize as f64 * (f64::from(metrics.advance[gid as usize]) / metrics.upem) * f.extend) as i32 as i64;
                if tracking {
                    let ascent = (sptsize as f64 * (f64::from(metrics.ascent) / metrics.upem)) as i32 as i64;
                    let descent = (sptsize as f64 * (f64::from(metrics.descent) / metrics.upem)) as i32 as i64;
                    self.dpx_track_box(cur_x + xloc, y - i64::from(g.y_offset), advance, ascent, -descent);
                }
            }
            // dvi_set_compensation: inside pdf:bcontent the position is relative
            // to the content origin (applied after the tracking box)
            let (ch, cv) = self.dpx_compensate(cur_x + xloc, y - i64::from(g.y_offset));
            self.dpxt_set_string(ch - self.dpxt.org_sp.0, -(cv - self.dpxt.org_sp.1), gid, advance, idx);
            xloc += i64::from(g.x_advance);
        }
        if colored {
            self.dpxt_graphics_mode();
            self.dpx_grestore();
            self.dpx_pop_text_color();
        }
        if actual_text {
            self.dpxt_graphics_mode();
            self.content.push_str(" EMC\n");
        }
        let _ = ONE_HUNDRED_BP_SP;
    }
}
