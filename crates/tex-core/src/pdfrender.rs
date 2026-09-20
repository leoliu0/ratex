//! PDF page rendering: traverses box trees into content streams, tracks
//! used fonts, emits rules, raw literals, colors, leaders, link
//! annotations, named destinations, and \pdfsavepos position recording.

use crate::boxes::{leader_dims, LeaderBody, Node, NodeList, HBOX};
use crate::build::RULE_FILL;
use crate::engine::Engine;
use crate::pdfout::{Annot, PdfPage};
use crate::prim::DimParam;

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
const SP_PER_BP: f64 = 72.27 * 65536.0 / 72.0;
/// pdfTeX's `divide_scaled`: return the rounded decimal value and the
/// corresponding displacement on TeX's scaled-point raster.
fn divide_scaled(mut s: i64, mut m: i64, decimal_digits: u32) -> (i64, i64) {
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

/// pdfTeX `pdf_print_bp`: print `sp` as bp with 3 decimals (trailing zeros
/// trimmed) and return the corresponding displacement on the sp raster
/// (`scaled_out`), exactly as `divide_scaled(s, one_hundred_bp, 5)` does.
#[inline]
fn push_bp_sp(buf: &mut String, sp: i64) -> i64 {
    let (value, out) = divide_scaled(sp, ONE_HUNDRED_BP_SP, 5);
    push_decimal(buf, value, 3);
    out
}

/// pdfTeX `round_xn_over_d` in i128-safe form: round `x * n / d` half-up on
/// the magnitude, sign restored.
#[inline]
fn round_xn_over_d(x: i64, n: i64, d: i64) -> i64 {
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

/// pdfTeX `pdf_print_bp` for a bp coordinate: quantize to sp, then print
/// with `divide_scaled(s, one_hundred_bp, digits+2)` / `pdf_print_real(..., 3)`
/// semantics (3 decimals, trailing zeros trimmed).
#[inline]
fn push_print_bp(buf: &mut String, bp: f64) {
    let sp_per_bp = 72.27 * 65536.0 / 72.0;
    push_bp_sp(buf, (bp * sp_per_bp).round() as i64);
}

/// pdfTeX's minimum move threshold: `divide_scaled(one_hundred_bp,
/// 10^(fixed_decimal_digits+2), 0)` with the default 3 decimals = 66 sp.
const MIN_BP_VAL: i64 = 66;
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
/// and emit them canonicalized, so a malformed matrix can never reach the PDF.
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
/// Print a parsed `\pdfsetmatrix` component. Matrix entries are
/// dimensionless numbers (not bp dimensions), so they are never re-quantized
/// through the sp raster: integral values print bare, others use Rust's
/// shortest round-tripping decimal form.
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

/// An open \pdfstartlink .. \pdfendlink region accumulating its extent.
/// Coordinates are raw page-space (x from the left edge, y measured
/// downward from the top); an active `\pdfsetmatrix` CTM is applied once at
/// closing time, mirroring pdfTeX's running-link `matrixrecalculate`
/// (pdftex.web §36546).
struct LinkFrame {
    uri: Option<String>,
    dest: Option<String>,
    attr: String,
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

pub struct RenderCtx<'a> {
    pub eng: &'a mut Engine,
    pub content: String,
    pub used_fonts: Vec<(usize, u16)>, // (engine font/binding key, PDF resource number)
    pub page_height_bp: f64,
    pub cur_font: usize,
    pub cur_pdf_font: u16,
    links: Vec<LinkFrame>,
    pub annots: Vec<Annot>,
    pub dests: Vec<crate::pdfout::Dest>,
    pub page_fonts: Vec<(usize, u16)>, // (engine font/binding key, resource number)
    // containing-box context for leaders grids and null-rule sentinels (sp)
    pub left_edge_sp: i64,
    pub box_w_sp: i64,
    pub box_h_sp: i64,
    pub box_d_sp: i64,
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
    last_f: u16,
    last_f_size: i64,
    pdf_h: i64,
    pdf_v: i64,
    tj_start_h: i64,
    delta_h: i64,
    origin_h: i64,
    origin_v: i64,
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
    pub color_stack: Vec<String>,
    pub display_list: crate::boxes::DisplayList,
    cjk_text: Option<char>,
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

#[inline]
fn push_pdf_char(s: &mut String, b: u8) {
    match b {
        b'(' | b')' | b'\\' => {
            s.push('\\');
            s.push(b as char);
        }
        32..=126 => s.push(b as char),
        _ => {
            // Three octal digits keep a following ASCII digit from becoming
            // part of the escape. Never encode font bytes as UTF-8 characters.
            s.push('\\');
            s.push((b'0' + (b >> 6)) as char);
            s.push((b'0' + ((b >> 3) & 7)) as char);
            s.push((b'0' + (b & 7)) as char);
        }
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
}
/// pdfTeX `pdf_print_bp` for a bp value: quantize to sp, then print with
/// `divide_scaled(s, one_hundred_bp, digits+2)` / `pdf_print_real(..., 3)`
/// semantics (3 decimals, trailing zeros trimmed).
fn push_pdfnum(buf: &mut String, v: f64) {
    push_print_bp(buf, v);
}

fn pdfnum(v: f64) -> String {
    let mut s = String::with_capacity(16);
    push_pdfnum(&mut s, v);
    s
}

impl Engine {
    /// Fresh emitter context: no text object open, origin at the page
    /// bottom-left (pdfTeX `pdf_origin_h := 0; pdf_origin_v :=
    /// cur_page_height`, set in `pdf_ship_out`).
    fn new_ctx(&mut self, page_height_sp: i64) -> RenderCtx<'_> {
        RenderCtx {
            eng: self,
            content: String::new(),
            used_fonts: Vec::new(),
            page_height_bp: sp_to_bp(page_height_sp),
            cur_font: 0,
            cur_pdf_font: 0,
            links: Vec::new(),
            annots: Vec::new(),
            dests: Vec::new(),
            page_fonts: Vec::new(),
            left_edge_sp: 0,
            box_w_sp: 0,
            box_h_sp: 0,
            box_d_sp: 0,
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
            page_height_sp,
            scaled_out: 0,
            page_mode: true,
            matrix_stack: Vec::new(),
            pos_stack: Vec::new(),
            color_stack: Vec::new(),
            display_list: crate::boxes::DisplayList::new(),
            cjk_text: None,
        }
    }

    /// Render a shipped page box into a PdfPage. Also copies the outline
    /// list into the document and records \pdfsavepos results
    /// (\pdflastxpos/\pdflastypos) from the last SavePos node on the page.
    pub fn render_page(&mut self, page_box: &Node) -> PdfPage {
        let width_sp = self.eqtb.dim_params[DimParam::PdfPageWidth.idx() as usize];
        let height_sp = self.eqtb.dim_params[DimParam::PdfPageHeight.idx() as usize];
        let w_bp = sp_to_bp(width_sp as i64);
        let h_bp = sp_to_bp(height_sp as i64);
        let mut ctx = self.new_ctx(height_sp as i64);
        ctx.box_w_sp = width_sp as i64;
        ctx.box_h_sp = height_sp as i64;
        let x0 = ctx.eng.eqtb.dim_params[DimParam::PdfHOrigin.idx() as usize] as i64
            + ctx.eng.eqtb.dim_params[DimParam::HOffset.idx() as usize] as i64;
        let y0 = ctx.eng.eqtb.dim_params[DimParam::PdfVOrigin.idx() as usize] as i64
            + ctx.eng.eqtb.dim_params[DimParam::VOffset.idx() as usize] as i64;
        if let Node::Box {
            list,
            kind,
            glue_sign,
            glue_order,
            glue_set,
            ..
        } = page_box
        {
            if *kind == HBOX {
                // shipped hbox: baseline sits at the top-left origin
                ctx.ship_hlist(list, x0, y0, *glue_sign, *glue_order, *glue_set);
            } else {
                // vbox/vtop: the top of the page material sits at the origin
                ctx.ship_vlist(list, x0, y0, *glue_sign, *glue_order, *glue_set);
            }
        }
        // close any links left open at the end of the page
        while let Some(fr) = ctx.links.pop() {
            ctx.close_link(fr);
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
        ctx.eng.pdf_doc.outlines = ctx.eng.pdf_outlines.clone();
        ctx.eng.pdf_doc.pages_attr = ctx.eng.pdf_pages_attr.clone().into_bytes();
        if ctx.eng.synctex_enabled {
            let page_num = (ctx.eng.pdf_doc.pages.len() + 1) as u32;
            for item in &ctx.display_list.items {
                if let crate::boxes::DisplayItem::GlyphRun {
                    x_bp,
                    y_bp,
                    source_file_id,
                    source_line,
                    ..
                } = item
                {
                    if *source_file_id > 0 && *source_line > 0 {
                        let x_sp = bp_to_sp(*x_bp);
                        let y_from_top_bp = (h_bp - *y_bp).max(0.0);
                        let y_sp = bp_to_sp(y_from_top_bp);
                        ctx.eng.synctex.record_point(
                            page_num,
                            *source_file_id,
                            *source_line,
                            x_sp as i64,
                            y_sp as i64,
                        );
                    }
                }
            }
        }
        PdfPage {
            content: {
                ctx.end_text();
                std::mem::take(&mut ctx.content).into_bytes()
            },
            width: w_bp.round() as i32,
            height: h_bp.round() as i32,
            width_bp: w_bp,
            height_bp: h_bp,
            annots: std::mem::take(&mut ctx.annots),
            fonts: std::mem::take(&mut ctx.page_fonts),
            dests: std::mem::take(&mut ctx.dests),
            attr_extra: ctx.eng.pdf_page_attr.as_bytes().to_vec(),
            resources_extra: ctx.eng.pdf_page_resources.clone(),
            display_list: Some(std::mem::take(&mut ctx.display_list)),
        }
    }

    pub fn render_form_box(
        &mut self,
        node: &Node,
        w: i32,
        h: i32,
        d: i32,
    ) -> (Vec<u8>, Vec<(usize, u16)>) {
        // Form coordinates are baseline-relative: the dictionary spans [-d, h].
        let mut ctx = self.new_ctx(h as i64);
        // pdfTeX `pdfshipoutbegin(false)` for forms: matrix/annotation
        // tracking is page-shipout only.
        ctx.page_mode = false;
        ctx.box_w_sp = w as i64;
        ctx.box_h_sp = h as i64;
        ctx.box_d_sp = d as i64;
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
        (ctx.content.into_bytes(), ctx.page_fonts)
    }
}

// Glue advance during shipout, tex.web §12438: the glue ratio applies to
// the matching-order component only; every other glue contributes its
// natural width. No clamping: negative glue widths must survive so that
// cancellation pairs (LaTeX \@xaddvskip, setspace) stay balanced.
fn glue_advance(
    width: i32,
    stretch: i32,
    shrink: i32,
    stretch_order: u8,
    shrink_order: u8,
    sign: u8,
    order: u8,
    set: f64,
) -> f64 {
    let w = sp_to_bp(width as i64);
    match sign {
        1 if stretch_order == order => w + set * sp_to_bp(stretch as i64),
        2 if shrink_order == order => w - set * sp_to_bp(shrink as i64),
        _ => w,
    }
}

fn glue_advance_sp(
    width: i32,
    stretch: i32,
    shrink: i32,
    stretch_order: u8,
    shrink_order: u8,
    sign: u8,
    order: u8,
    set: f64,
) -> i64 {
    let w = width as i64;
    match sign {
        1 if stretch_order == order => w + (set * stretch as f64).round() as i64,
        2 if shrink_order == order => w - (set * shrink as f64).round() as i64,
        _ => w,
    }
}

impl<'a> RenderCtx<'a> {
    fn y_pdf(&self, tex_y_bp: f64) -> f64 {
        self.page_height_bp - tex_y_bp
    }

    /// grow every open link region to include the point/extent at (x, y)
    fn note_point(&mut self, x: f64, y: f64) {
        for fr in self.links.iter_mut() {
            if x < fr.min_x {
                fr.min_x = x;
            }
            if x > fr.max_x {
                fr.max_x = x;
            }
            if y < fr.min_y {
                fr.min_y = y;
            }
            if y > fr.max_y {
                fr.max_y = y;
            }
        }
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

    fn close_link(&mut self, fr: LinkFrame) {
        // pdfTeX `end_link` on a running-width link: `matrixrecalculate`
        // re-transforms the rect stored at `\pdfstartlink` time with the
        // CTM active at closing time. The frame accumulates the raw pen
        // extent; the transform happens once here.
        if self.matrix_used().is_some() {
            let [x0, y0, x1, y1] = self.page_rect(
                (fr.min_x * SP_PER_BP).round() as i64,
                (fr.min_y * SP_PER_BP).round() as i64,
                (fr.max_x * SP_PER_BP).round() as i64,
                (fr.max_y * SP_PER_BP).round() as i64,
            );
            self.annots.push(Annot {
                rect: [x0, y0, x1, y1],
                uri: fr.uri,
                dest: fr.dest,
                attr: fr.attr,
                subtype: Some("/Link".to_string()),
            });
            return;
        }
        // PDF rect: y grows upward; tex y grows upward within the page
        let y0 = self.y_pdf(fr.max_y);
        let y1 = self.y_pdf(fr.min_y);
        self.annots.push(Annot {
            rect: [fr.min_x, y0, fr.max_x, y1],
            uri: fr.uri,
            dest: fr.dest,
            attr: fr.attr,
            subtype: Some("/Link".to_string()),
        });
    }

    /// ship a vertical list with its top edge at y
    pub fn ship_vlist(&mut self, list: &NodeList, x: i64, y: i64, sign: u8, order: u8, set: f64) {
        let mut cur_y = y;
        for n in list {
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
                    ..
                } => {
                    let (bh, bd, sh) = (*h as i64, *d as i64, *shift as i64);

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
                } => {
                    // hrule in a vlist: null width fills the containing box
                    let w_sp = if *width == RULE_FILL {
                        self.box_w_sp
                    } else {
                        *width as i64
                    };
                    let (rh, rd) = (*height as i64, *depth as i64);
                    let y1 = cur_y + rh; // top of rule
                    self.emit_rect_sp(x, y1 + rd, w_sp, rh + rd);
                    cur_y += rh + rd;
                }
                Node::Glue(g) => {
                    cur_y += glue_advance_sp(
                        g.width,
                        g.stretch,
                        g.shrink,
                        g.stretch_order,
                        g.shrink_order,
                        sign,
                        order,
                        set,
                    );
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
                Node::Leaders { glue, kind, body } => {
                    let adv = glue_advance_sp(
                        glue.width,
                        glue.stretch,
                        glue.shrink,
                        glue.stretch_order,
                        glue.shrink_order,
                        sign,
                        order,
                        set,
                    );
                    let (lw, lh, ld) = leader_dims(body);
                    match body {
                        // rule body: one rect spanning the whole advance;
                        // null width fills the containing box
                        LeaderBody::Rule { width, .. } => {
                            let w_sp = if *width == RULE_FILL {
                                self.box_w_sp
                            } else {
                                *width as i64
                            };
                            if w_sp > 0 && adv > 0 {
                                self.emit_rect_sp(x, cur_y + adv, w_sp, adv);
                            }
                        }
                        LeaderBody::Box(b) => {
                            // leader_wd = height + depth of the body box
                            let (positions, _, _) = crate::boxes::leader_layout(
                                *kind,
                                (lh + ld) as i64,
                                adv,
                                self.left_edge_sp,
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
                Node::Kern(k) | Node::ExplicitKern(k) | Node::MarginKern { width: k, .. } => {
                    cur_y += *k as i64;
                }
                Node::Penalty(_) | Node::Mark { .. } => {}
                Node::Whatsit(
                    w @ (crate::boxes::WhatIt::PdfRefXImage { h, d, .. }
                    | crate::boxes::WhatIt::PdfRefXForm { h, d, .. }),
                ) => {
                    cur_y += *h as i64;
                    self.emit_whatsit_sp(w, x, cur_y);
                    cur_y += *d as i64;
                }
                Node::Whatsit(w) => {
                    self.note_point(sp_to_bp(x), self.y_pdf(sp_to_bp(cur_y)));
                    self.emit_whatsit_sp(w, x, cur_y);
                }
                Node::Ins { box_node, .. } => {
                    if let Node::Box { list: inner, .. } = &**box_node {
                        self.ship_vlist(inner, x, cur_y, 0, 0, 0.0);
                    }
                }
                Node::VAdjust(items) => {
                    self.ship_vlist(items, x, cur_y, 0, 0, 0.0);
                }
                _ => {}
            }
        }
    }

    /// ship a horizontal list with baseline at y
    pub fn ship_hlist(&mut self, list: &NodeList, x: i64, y: i64, sign: u8, order: u8, set: f64) {
        let mut cur_x = x;
        for n in list {
            match n {
                Node::Char { c, font } => {
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
                    cur_x += *width as i64;
                }
                Node::Glue(g) => {
                    let adv = glue_advance_sp(
                        g.width,
                        g.stretch,
                        g.shrink,
                        g.stretch_order,
                        g.shrink_order,
                        sign,
                        order,
                        set,
                    );
                    cur_x += adv;
                }
                Node::Kern(k) | Node::ExplicitKern(k) | Node::MarginKern { width: k, .. } => {
                    cur_x += *k as i64;
                }
                Node::Penalty(_) => {}
                Node::Rule {
                    width,
                    height,
                    depth,
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
                    self.emit_rect_sp(cur_x, y + rd, rw, rh + rd);
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
                    self.left_edge_sp = if *kind == HBOX { cur_x } else { cur_x + sh };
                    (self.box_w_sp, self.box_h_sp, self.box_d_sp) =
                        (*w as i64, *h as i64, *d as i64);
                    if *kind == HBOX {
                        // hbox: the shift is vertical (baseline moves down)
                        let baseline = y + sh;
                        self.ship_hlist(inner, cur_x, baseline, *glue_sign, *glue_order, *glue_set);
                    } else {
                        // vbox/vtop in an hlist: the shift is vertical
                        self.ship_vlist(
                            inner,
                            cur_x,
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
                            Node::Char { c, font } => {
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
                                cur_x += *width as i64;
                            }
                            other => {
                                let single: NodeList = vec![other.clone()];
                                let (w, _, _) = crate::boxes::hlist_dims(&single, &self.eng.eqtb);
                                self.ship_hlist(&single, cur_x, y, sign, order, set);
                                cur_x += w as i64;
                            }
                        }
                    }
                }
                Node::Leaders { glue, kind, body } => {
                    let adv = glue_advance_sp(
                        glue.width,
                        glue.stretch,
                        glue.shrink,
                        glue.stretch_order,
                        glue.shrink_order,
                        sign,
                        order,
                        set,
                    );
                    let (lw, lh, ld) = leader_dims(body);
                    match body {
                        // rule body: one rect over the whole advance; null
                        // height/depth fill the containing box
                        LeaderBody::Rule { height, depth, .. } => {
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
                                self.emit_rect_sp(cur_x, y + rd, adv, rh + rd);
                            }
                        }
                        LeaderBody::Box(b) => {
                            let (positions, _, _) = crate::boxes::leader_layout(
                                *kind,
                                lw as i64,
                                adv,
                                self.left_edge_sp,
                                cur_x,
                            );
                            for pos in positions {
                                self.ship_leader_copy(b, pos, y, false);
                            }
                        }
                    }
                    let _ = (lh, ld);
                    cur_x += adv;
                }
                Node::Whatsit(w) => {
                    self.note_point(sp_to_bp(cur_x), self.y_pdf(sp_to_bp(y)));
                    self.emit_whatsit_sp(w, cur_x, y);
                    if let crate::boxes::WhatIt::PdfRefXImage { w, .. }
                    | crate::boxes::WhatIt::PdfRefXForm { w, .. } = w
                    {
                        cur_x += *w as i64;
                    }
                }
                Node::Mark { .. } | Node::Ins { .. } => {}
                _ => {}
            }
        }
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
        self.left_edge_sp = x;
        (self.box_w_sp, self.box_h_sp, self.box_d_sp) = (*w as i64, *h as i64, *d as i64);
        if vertical {
            if *kind == HBOX {
                // hbox as a vlist item: top edge at `at`, baseline below
                self.ship_hlist(inner, x, at + bh + sh, *glue_sign, *glue_order, *glue_set);
            } else {
                self.ship_vlist(inner, x + sh, at, *glue_sign, *glue_order, *glue_set);
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

    fn font_char_advance_bp(&self, f: u16, c: u8) -> f64 {
        let w = self.font_char_width(f, c) as i64;
        let ratio = self.eng.eqtb.expand.get(f as usize).map_or(0, |x| x.ratio);
        let is_already_scaled = self
            .eng
            .eqtb
            .expand
            .get(f as usize)
            .map_or(false, |x| x.blink != 0);
        if is_already_scaled || ratio == 0 {
            sp_to_bp(w)
        } else {
            let h_scale = (1000 + ratio) as f64 / 1000.0;
            sp_to_bp(((w as f64) * h_scale).round() as i64)
        }
    }

    fn font_lig_advance_bp(&self, f: u16, lig_width: i32) -> f64 {
        let w = lig_width as i64;
        let ratio = self.eng.eqtb.expand.get(f as usize).map_or(0, |x| x.ratio);
        let is_already_scaled = self
            .eng
            .eqtb
            .expand
            .get(f as usize)
            .map_or(false, |x| x.blink != 0);
        if is_already_scaled || ratio == 0 {
            sp_to_bp(w)
        } else {
            let h_scale = (1000 + ratio) as f64 / 1000.0;
            sp_to_bp(((w as f64) * h_scale).round() as i64)
        }
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

    fn ensure_font(&mut self, f: u16, binding: usize) -> u16 {
        let f = crate::pdfout::font_resource_key(f, binding);
        if self.cur_font == f && self.cur_pdf_font != 0 {
            return self.cur_pdf_font;
        }
        if let Some((_, num)) = self.used_fonts.iter().find(|(id, _)| *id == f) {
            self.cur_font = f;
            self.cur_pdf_font = *num;
            return *num;
        }
        let Ok(num) = u16::try_from(self.used_fonts.len() + 1) else {
            self.eng
                .error("PDF page exceeds the supported font resource count");
            return 0;
        };
        self.used_fonts.push((f, num));
        self.page_fonts.push((f, num));
        self.cur_font = f;
        self.cur_pdf_font = num;
        num
    }

    /// pdfTeX `pdf_print_real(m, d)`: print m/10^d, trimming trailing zeros.
    fn push_real(&mut self, m: i64, d: u32) {
        push_decimal(&mut self.content, m, d);
    }

    /// pdfTeX `pdf_print_bp(s)`: print a sp displacement as bp (3 decimals).
    fn push_bp(&mut self, sp: i64) {
        let out = push_bp_sp(&mut self.content, sp);
        self.scaled_out = out;
    }

    /// pdfTeX `pdf_set_origin(h, v)`: re-center the text/print origin at the
    /// TeX-space point (h, v_down), emitting `cm` when the move is visible.
    /// `scaled_out`-snapped so the recorded origin matches the printed raster.
    fn set_origin(&mut self, h_sp: i64, v_down_sp: i64) {
        if (h_sp - self.origin_h).abs() >= MIN_BP_VAL
            || (v_down_sp - self.origin_v).abs() >= MIN_BP_VAL
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
        if (h_sp - self.origin_h).abs() >= MIN_BP_VAL
            || (v_down_sp - self.origin_v).abs() >= MIN_BP_VAL
        {
            self.content.push_str("1 0 0 1 ");
            self.push_bp(h_sp - self.origin_h);
            self.content.push(' ');
            self.push_bp(self.origin_v - v_down_sp);
            self.content.push_str(" cm\n");
        }
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
    fn set_font(&mut self, f: u16, binding: usize) {
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
        push_i64(&mut self.content, num as i64);
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
            self.push_real(v, 3);
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
    fn begin_string(&mut self, cur_h: i64, cur_v: i64, f: u16, binding: usize, ratio: i32) {
        let mut must_set_text_pos = false;
        if !self.doing_text {
            self.begin_text();
            must_set_text_pos = true;
        }
        if self.pdf_f != f || self.cur_font != crate::pdfout::font_resource_key(f, binding) {
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
        let (v, v_out) = if (cur_v - self.pdf_v).abs() >= MIN_BP_VAL {
            divide_scaled(self.pdf_v - cur_v, ONE_HUNDRED_BP_SP, 5)
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

    fn emit_char(&mut self, f: u16, c: u8, x: f64, y: f64) {
        let x_sp = (x * SP_PER_BP).round() as i64;
        let v_sp = (y * SP_PER_BP).round() as i64;
        self.emit_char_sp(f, c, x_sp, v_sp, 0);
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
        let size_bp = sp_to_bp(at_size_sp);
        let x = sp_to_bp(x_sp);
        let y = sp_to_bp(v_sp);
        self.note_point(x, y + 0.75 * size_bp);
        self.note_point(x + 0.5 * size_bp, y - 0.25 * size_bp);
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
            let file_name = self.eng.input.current_file_name();
            let line = self.eng.input.current_file_line();
            let file_id = if file_name.is_empty() {
                0
            } else {
                self.eng.synctex.get_or_register_file(&file_name)
            };
            self.display_list.push(crate::boxes::DisplayItem::GlyphRun {
                font: f,
                x_bp,
                y_bp,
                glyphs: vec![code],
                tag: None,
                span: None,
                source_file_id: file_id,
                source_line: line,
            });
        }
    }

    /// pdfTeX `output_one_char`: begin the string (emitting Tf/Tm/Td as the
    /// canonical state machine requires), print the char, advance the raster.
    fn emit_char_sp(&mut self, f: u16, c: u8, x_sp: i64, v_sp: i64, inherited_ratio: i32) {
        let text = self.cjk_text;
        if text.is_some() {
            self.cjk_text = Some('\u{00A0}');
        }
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
        let size_bp = sp_to_bp(at_size_sp);
        let x = sp_to_bp(x_sp);
        let y = sp_to_bp(v_sp);
        // approximate glyph extent feeds link rectangles
        self.note_point(x, y + 0.75 * size_bp);
        self.note_point(x + 0.5 * size_bp, y - 0.25 * size_bp);
        if at_size_sp <= 0 {
            return; // nullfont: nothing to draw
        }
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
        if let Some(bases) = self.eng.font_loader.vf_bases.get(&f).cloned() {
            let key = self
                .eng
                .eqtb
                .fonts
                .get(f as usize)
                .map(|ff| (ff.tfm_name.clone(), ff.at_size));
            let steps = key
                .and_then(|k| {
                    self.eng.font_loader.vf_fonts.get(&k).cloned().or_else(|| {
                        let base_name = if let Some(idx) = k.0.rfind(['+', '-']) {
                            if idx > 0 && k.0[idx + 1..].chars().all(|c| c.is_ascii_digit()) {
                                &k.0[..idx]
                            } else {
                                &k.0
                            }
                        } else {
                            &k.0
                        };
                        self.eng
                            .font_loader
                            .vf_fonts
                            .get(&(base_name.to_string(), k.1))
                            .cloned()
                    })
                })
                .and_then(|vf| vf.chars.get(c as usize).cloned().flatten());
            let tfm_name = self
                .eng
                .eqtb
                .fonts
                .get(f as usize)
                .map(|ff| ff.tfm_name.clone())
                .unwrap_or_default();
            if let Some(steps) = steps {
                for (step_idx, st) in steps.iter().enumerate() {
                    let Some(&bfid) = bases.get(st.base as usize) else {
                        self.eng.error(&format!(
                            "Virtual font `{tfm_name}` references missing base font index {}",
                            st.base
                        ));
                        continue;
                    };
                    if bfid == u16::MAX {
                        self.eng.error(&format!(
                            "Virtual font `{tfm_name}` requires missing base font index {}",
                            st.base
                        ));
                        continue;
                    }
                    let text = logical_ch.map(|ch| if step_idx > 0 { '\u{00A0}' } else { ch });
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
            } else {
                self.eng.error(&format!(
                    "Virtual font `{tfm_name}` has no character packet for slot {c}"
                ));
            }
            return;
        }
        if let Some(ch) = logical_ch {
            let mut utf8 = [0; 4];
            self.emit_cjk_char_sp(f, c, x_sp, v_sp, ratio, ch.encode_utf8(&mut utf8));
            return;
        }
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
        self.eng.pdf_doc.record_font_char(base_f as usize, c);
        self.begin_string(x_sp, v_sp, f, 0, ratio);
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
            let file_name = self.eng.input.current_file_name();
            let line = self.eng.input.current_file_line();
            let file_id = if file_name.is_empty() {
                0
            } else {
                self.eng.synctex.get_or_register_file(&file_name)
            };
            self.display_list.push(crate::boxes::DisplayItem::GlyphRun {
                font: f,
                x_bp,
                y_bp,
                glyphs: vec![c],
                tag: None,
                span: None,
                source_file_id: file_id,
                source_line: line,
            });
        }
    }

    fn begin_hex_string(&mut self, cur_h: i64, cur_v: i64, f: u16, binding: usize, ratio: i32) {
        let mut must_set_text_pos = false;
        if !self.doing_text {
            self.begin_text();
            must_set_text_pos = true;
        }
        if self.pdf_f != f
            || self.cur_font != crate::pdfout::font_resource_key(f, binding)
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
        let (v, v_out) = if (cur_v - self.pdf_v).abs() >= MIN_BP_VAL {
            divide_scaled(self.pdf_v - cur_v, ONE_HUNDRED_BP_SP, 5)
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
        } else if let Some(native) = self.eng.font_loader.native_fonts.get(&fid) {
            if let Ok(face) = native.program.face() {
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

    fn emit_native_glyph_run_sp(
        &mut self,
        run: &std::rc::Rc<crate::native_layout::NativeRun>,
        start: usize,
        end: usize,
        cur_x: i64,
        y: i64,
    ) {
        if start >= end || start >= run.glyphs.len() {
            return;
        }
        let bound_end = end.min(run.glyphs.len());
        let fid = run.font;
        let at_size_sp = self
            .eng
            .eqtb
            .fonts
            .get(fid as usize)
            .map(|ff| ff.at_size as i64)
            .unwrap_or(0);
        if at_size_sp <= 0 {
            return;
        }
        let (_, m) = divide_scaled(at_size_sp, ONE_HUNDRED_BP_SP, 6);
        let ratio = self.font_ratio(fid);
        let file_name = self.eng.input.current_file_name();
        let line = self.eng.input.current_file_line();
        let file_id = if file_name.is_empty() {
            0
        } else {
            self.eng.synctex.get_or_register_file(&file_name)
        };
        let x_bp = sp_to_bp(cur_x);
        let y_bp = self.y_pdf(sp_to_bp(y));
        let size_bp = sp_to_bp(at_size_sp);
        self.note_point(x_bp, y_bp + 0.75 * size_bp);

        self.display_list
            .push(crate::boxes::DisplayItem::NativeGlyphRun {
                run: run.clone(),
                start,
                end: bound_end,
                x_bp,
                y_bp,
                tag: None,
                span: None,
                source_file_id: file_id,
                source_line: line,
            });

        if self.eng.synctex_enabled && file_id > 0 && line > 0 {
            let page_num = (self.eng.pdf_doc.pages.len() + 1) as u32;
            let y_from_top_sp = (self.page_height_sp - y).max(0);
            self.eng
                .synctex
                .record_point(page_num, file_id, line, cur_x, y_from_top_sp);
        }

        let mut pen_x = cur_x;
        let mut idx = start;
        while idx < bound_end {
            let c_start = run.glyphs[idx].cluster_start;
            let c_end = run.glyphs[idx].cluster_end;
            let mut j = idx + 1;
            while j < bound_end
                && run.glyphs[j].cluster_start == c_start
                && run.glyphs[j].cluster_end == c_end
            {
                j += 1;
            }
            let cluster_glyph_count = j - idx;
            let extraction = if (c_start as usize) < run.text.len()
                && (c_end as usize) <= run.text.len()
                && c_start <= c_end
            {
                &run.text[c_start as usize..c_end as usize]
            } else {
                ""
            };

            if cluster_glyph_count > 1 {
                self.end_string();
                let mut actual_hex = String::from("FEFF");
                for u in extraction.encode_utf16() {
                    use std::fmt::Write;
                    let _ = write!(&mut actual_hex, "{:04X}", u);
                }
                self.content.push_str("/Span << /ActualText <");
                self.content.push_str(&actual_hex);
                self.content.push_str("> >> BDC\n");
            }

            for k in idx..j {
                let g = &run.glyphs[k];
                let txt = if cluster_glyph_count == 1 {
                    extraction
                } else if k == idx {
                    extraction
                } else {
                    ""
                };
                let (binding_idx, code) =
                    self.eng
                        .pdf_doc
                        .get_or_alloc_native_code(fid as usize, g.glyph_id, txt);
                let glyph_target_x = pen_x + g.x_offset as i64;
                let glyph_target_y = y - g.y_offset as i64;

                self.begin_hex_string(glyph_target_x, glyph_target_y, fid, binding_idx, ratio);
                use std::fmt::Write;
                let _ = write!(&mut self.content, "{:04X}", code);

                let nom_sp = self.native_glyph_nom_advance_sp(fid, g.glyph_id);
                let (_, nom_out) = if self.cur_tm_a == 0 {
                    divide_scaled(nom_sp, m, 4)
                } else {
                    let (_, out) = divide_scaled(
                        round_xn_over_d(nom_sp, 1000, 1000 + self.cur_tm_a as i64),
                        m,
                        4,
                    );
                    (0, out)
                };
                self.delta_h += nom_out;
                pen_x += g.x_advance as i64;
            }

            if cluster_glyph_count > 1 {
                self.end_string();
                self.content.push_str("EMC\n");
            }

            idx = j;
        }

        self.note_point(sp_to_bp(pen_x), y_bp - 0.25 * size_bp);
    }

    /// pdfTeX `pdf_set_rule`: close the text object, then draw inside a
    /// `q..Q` scope with a temporary origin shift (hairlines stroke).
    fn emit_rect_sp(&mut self, x_sp: i64, v_down_sp: i64, w_sp: i64, h_sp: i64) {
        if w_sp == 0 || h_sp == 0 {
            return;
        }
        let x = sp_to_bp(x_sp);
        let y = self.y_pdf(sp_to_bp(v_down_sp));
        let w = sp_to_bp(w_sp);
        let h = sp_to_bp(h_sp);
        self.note_point(x, y);
        self.note_point(x + w, y + h);
        self.display_list.push(crate::boxes::DisplayItem::Rule {
            x_bp: x,
            y_bp: y,
            width_bp: w,
            height_bp: h,
        });
        self.end_text();
        self.content.push_str("q\n");
        const ONE_BP: i64 = 65782;
        if h_sp <= ONE_BP {
            self.set_origin_temp(x_sp, v_down_sp - (h_sp + 1) / 2);
            self.content.push_str("[]0 d 0 J ");
            self.push_bp(h_sp);
            self.content.push_str(" w 0 0 m ");
            self.push_bp(w_sp);
            self.content.push_str(" 0 l S\n");
        } else if w_sp <= ONE_BP {
            self.set_origin_temp(x_sp + (w_sp + 1) / 2, v_down_sp);
            self.content.push_str("[]0 d 0 J ");
            self.push_bp(w_sp);
            self.content.push_str(" w 0 0 m 0 ");
            self.push_bp(h_sp);
            self.content.push_str(" l S\n");
        } else {
            self.set_origin_temp(x_sp, v_down_sp);
            self.content.push_str("0 0 ");
            self.push_bp(w_sp);
            self.content.push(' ');
            self.push_bp(h_sp);
            self.content.push_str(" re f\n");
        }
        self.content.push_str("Q\n");
    }
    fn emit_whatsit_sp(&mut self, w: &crate::boxes::WhatIt, cur_h: i64, cur_v: i64) {
        use crate::boxes::WhatIt::*;
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
            PdfColorPush(color) => {
                // colorstack default literal mode is direct_always: the
                // string closes but the text object stays open
                self.end_string_nl();
                self.color_stack.push(color.clone());
                self.content.push_str(color);
                self.content.push('\n');
            }
            PdfColorSet(color) => {
                self.end_string_nl();
                if let Some(top) = self.color_stack.last_mut() {
                    *top = color.clone();
                } else {
                    self.color_stack.push(color.clone());
                }
                self.content.push_str(color);
                self.content.push('\n');
            }
            PdfColorPop => {
                self.end_string_nl();
                self.color_stack.pop();
                let prev_color = self
                    .color_stack
                    .last()
                    .map(|s| s.as_str())
                    .unwrap_or("0 g 0 G");
                self.content.push_str(prev_color);
                self.content.push('\n');
            }
            PdfRefXImage { obj, w, h, d } => {
                if let Some(image) = self.eng.pdf_images.get_mut(obj) {
                    image.used = true;
                }
                self.end_text();
                let v_sp = cur_v;
                let w_sp = *w as i64;
                let hd_sp = (*h + *d) as i64;
                self.content.push_str("q\n");
                let sx = pdfnum(sp_to_bp(w_sp));
                let sy = pdfnum(sp_to_bp(hd_sp));
                self.content.push_str(&format!("{sx} 0 0 {sy} "));
                self.push_bp(cur_h - self.origin_h);
                self.content.push(' ');
                self.push_bp(self.origin_v - (v_sp + *d as i64));
                self.content.push_str(&format!(" cm /Im{obj} Do\nQ\n"));
            }
            PdfRefXForm { obj, .. } => {
                self.end_text();
                self.content.push_str("q\n1 0 0 1 ");
                self.push_bp(cur_h - self.origin_h);
                self.content.push(' ');
                self.push_bp(self.origin_v - cur_v);
                self.content.push_str(&format!(" cm /Fm{obj} Do\nQ\n"));
            }
            PdfSetMatrix { matrix, source } => {
                // pdfTeX `pdf_out_setmatrix` + `pdfsetmatrix` (utils.c §1406):
                // a valid matrix is exactly four numbers; the emitted
                // literal is `set_origin` mode, so the CTM first moves to
                // the current pen and the supplied matrix concatenates at
                // that origin. Malformed input emits no content at all
                // (canonical `\pdfsetmatrix` "Unrecognized format." error).
                // pdfTeX echoes the raw token string on success; we
                // canonicalize the parsed numbers instead, so a malformed
                // stream can never slip through a token-level parse.
                let Some([a, b, c, d]) = parse_matrix(matrix) else {
                    self.end_text();
                    let message = format!(
                        "Invalid \\pdfsetmatrix value; expected exactly four finite numbers; got `{matrix}`"
                    );
                    self.eng.fatal_error_at(
                        &message,
                        source.as_ref().map(crate::input::SourceMark::to_context),
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
                let mut buf = String::new();
                for v in [a, b, c, d] {
                    if !buf.is_empty() {
                        buf.push(' ');
                    }
                    push_matrix_num(&mut buf, v);
                }
                self.content.push_str(&buf);
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
                    source: source.clone(),
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
                        source.as_ref().map(crate::input::SourceMark::to_context),
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
                            .as_ref()
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
            PdfDest { name, kind, params } => {
                // first definition of a name wins
                if !self.dests.iter().any(|d| &d.name == name) {
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
                        name: name.clone(),
                        x: px,
                        y: py,
                        kind: *kind,
                        zoom: zm,
                    });
                }
            }
            PdfAnnot { attr, wd, ht, dp } => {
                // pdfTeX `do_annot` -> `set_rect_dimens` (pdftex.web §36430):
                // left = cur_h, right = cur_h + width, top = cur_v - height,
                // bottom = cur_v + depth (DVI y grows downward), then the
                // rect goes through `matrixtransformrect` when a matrix is
                // active and is emitted bottom-up.
                let left = cur_h;
                let base = cur_v;
                let rect = self.page_rect(
                    left,
                    base - *ht as i64,
                    left + *wd as i64,
                    base + *dp as i64,
                );
                self.annots.push(Annot {
                    rect,
                    uri: None,
                    dest: None,
                    attr: attr.clone(),
                    subtype: None,
                });
            }
            PdfStartLink { attr, uri, name } => {
                let x = sp_to_bp(cur_h);
                let y = self.y_pdf(sp_to_bp(cur_v));
                self.links.push(LinkFrame {
                    uri: uri.clone(),
                    dest: name.clone(),
                    attr: attr.clone(),
                    min_x: x,
                    min_y: y,
                    max_x: x,
                    max_y: y,
                });
            }
            PdfEndLink => {
                if let Some(fr) = self.links.pop() {
                    // include the pen position at closing time
                    let mut fr = fr;
                    let x = sp_to_bp(cur_h);
                    let y = self.y_pdf(sp_to_bp(cur_v));
                    if x < fr.min_x {
                        fr.min_x = x;
                    }
                    if x > fr.max_x {
                        fr.max_x = x;
                    }
                    if y < fr.min_y {
                        fr.min_y = y;
                    }
                    if y > fr.max_y {
                        fr.max_y = y;
                    }
                    self.close_link(fr);
                }
            }
            Special(_) => {
                // DVI \special has no direct PDF meaning; ignored
            }
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
                self.eng.fire_write(*stream, &toks, src.as_ref());
            }
            OpenOut {
                stream,
                path,
                create_parent,
                source,
            } => {
                let p = path.clone();
                let src = source.clone();
                self.eng
                    .exec_openout(*stream, &p, *create_parent, src.as_ref());
            }
            CloseOut { stream, source } => {
                let src = source.clone();
                self.eng.exec_closeout(*stream, src.as_ref());
            }
            CjkText(text) => {
                self.cjk_text = *text;
            }
            _ => {}
        }
    }
}
