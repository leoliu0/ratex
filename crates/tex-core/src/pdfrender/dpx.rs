//! The xdvipdfmx special interpreter for XeTeX's direct PDF output.
//!
//! XeTeX writes an XDV file that xdvipdfmx turns into a PDF. Ratex has no
//! XDV stage: the shipped page is rendered straight into a content stream and
//! the `\special`s that TeX Live's drivers (`xetex.def`, `hxetex.def`,
//! `l3backend-xetex.def`, `pgfsys-xetex.def`, ...) rely on are interpreted
//! here with xdvipdfmx's semantics: the graphics-state stack and colour
//! stack of `pdfdev.c`/`pdfdraw.c`/`pdfcolor.c`, the `pdf:` family of
//! `spc_pdfm.c`, the `x:` family of `spc_xtx.c`, `color`/`background` of
//! `spc_color.c`, annotation line breaking of `dvi.c`/`pdfdoc.c`, named
//! objects, form XObjects, destinations, outlines, document information.
//!
//! Content streams use absolute page coordinates (bp from the lower left
//! corner of the page, y up); what dvipdfmx calls "user space" is this frame
//! with the DVI origin folded into the page offset. This module owns the
//! state; document level output (objects, outlines, name tree, catalog) is
//! finished by `dpx_doc.rs`.

use super::{sp_to_bp, RenderCtx};
use crate::dpx_obj::{Obj, Parser};
use std::collections::{BTreeMap, HashMap};

// ---------------------------------------------------------------- numbers

/// C `ROUND(x, y)` of dvipdfmx: `floor(x / y + 0.5) * y`.
pub(super) fn round_to(x: f64, y: f64) -> f64 {
    (x / y + 0.5).floor() * y
}

/// `p_dtoa`: `prec` decimals with trailing zeros removed.
pub(super) fn fmt_prec(v: f64, prec: usize) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let mut s = format!("{v:.prec$}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".to_string();
    }
    s
}

/// `%g` of `ROUND(v, 0.001)`, as `pdf_color_set_color` prints components.
fn fmt_g3(v: f64) -> String {
    fmt_prec(round_to(v, 0.001), 3)
}

pub(crate) type Matrix6 = [f64; 6];

const IDENTITY: Matrix6 = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `pdf_concatmatrix(M1, M0)`: apply `M0` first, then `M1` (row vectors).
pub(super) fn mat_mul(m0: &Matrix6, m1: &Matrix6) -> Matrix6 {
    [
        m0[0] * m1[0] + m0[1] * m1[2],
        m0[0] * m1[1] + m0[1] * m1[3],
        m0[2] * m1[0] + m0[3] * m1[2],
        m0[2] * m1[1] + m0[3] * m1[3],
        m0[4] * m1[0] + m0[5] * m1[2] + m1[4],
        m0[4] * m1[1] + m0[5] * m1[3] + m1[5],
    ]
}

fn mat_apply(m: &Matrix6, x: f64, y: f64) -> (f64, f64) {
    (x * m[0] + y * m[2] + m[4], x * m[1] + y * m[3] + m[5])
}

/// `p_dtoa` of pdfdev.c: `prec` decimals, trailing zeros removed, and no
/// integer digit for a value below one (`.3985`).
pub(super) fn p_dtoa(value: f64, prec: usize) -> String {
    let scale = 10f64.powi(prec as i32);
    let neg = value < 0.0;
    let v = value.abs();
    let mut int_part = v.trunc();
    let frac = v - int_part;
    let mut g = (frac * scale + 0.5) as i64;
    if g == scale as i64 {
        g = 0;
        int_part += 1.0;
    }
    let mut out = String::new();
    if int_part != 0.0 {
        if neg {
            out.push('-');
        }
        out.push_str(&format!("{int_part:.0}"));
    } else if g == 0 {
        return "0".to_string();
    } else if neg {
        out.push('-');
    }
    if g != 0 {
        let digits = format!("{g:0prec$}");
        out.push('.');
        out.push_str(digits.trim_end_matches('0'));
    }
    out
}

/// Device precision of xdvipdfmx (`pdfdecimaldigits`, 3).
const PRECISION: usize = 3;

/// `pdf_dev_sprint_matrix`: the linear part with two more digits than the
/// translation.
fn fmt_matrix(m: &Matrix6) -> String {
    format!(
        "{} {} {} {} {} {}",
        p_dtoa(m[0], PRECISION + 2),
        p_dtoa(m[1], PRECISION + 2),
        p_dtoa(m[2], PRECISION + 2),
        p_dtoa(m[3], PRECISION + 2),
        p_dtoa(m[4], PRECISION),
        p_dtoa(m[5], PRECISION)
    )
}

// ----------------------------------------------------------------- colour

/// A device colour (`pdf_color` of the three device families dvipdfmx's
/// colour stack handles).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Color {
    Gray(f64),
    Rgb([f64; 3]),
    Cmyk([f64; 4]),
}

impl Color {
    fn black() -> Color {
        Color::Gray(0.0)
    }

    /// `pdf_color_is_white`
    fn is_white(&self) -> bool {
        match self {
            Color::Gray(g) => *g == 1.0,
            Color::Rgb(v) => v.iter().all(|&c| c == 1.0),
            Color::Cmyk(v) => v.iter().all(|&c| c == 0.0),
        }
    }

    /// `pdf_color_set_color`: the operator (` 0 g`, ` 1 0 0 RG`, ...).
    fn ops(&self, fill: bool) -> String {
        let (values, op): (&[f64], &str) = match self {
            Color::Gray(g) => (std::slice::from_ref(g), if fill { "g" } else { "G" }),
            Color::Rgb(v) => (v, if fill { "rg" } else { "RG" }),
            Color::Cmyk(v) => (v, if fill { "k" } else { "K" }),
        };
        let mut s = String::new();
        for v in values {
            s.push(' ');
            s.push_str(&fmt_g3(*v));
        }
        s.push(' ');
        s.push_str(op);
        s
    }
}

/// `colordefs` of spc_util.c (dvips colour names).
fn named_color(name: &str) -> Option<Color> {
    let table: &[(&str, Color)] = include!("dpx_colors.in");
    table.iter().find(|(n, _)| *n == name).map(|(_, c)| c.clone())
}

pub(super) const SOURCE_DVIPS: u8 = 0;
pub(super) const SOURCE_BCOLOR: u8 = 1;

// ------------------------------------------------------------ parse tools

fn is_blank(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | 0x0b)
}

pub(super) fn skip_blank(p: &mut Parser) {
    while p.pos < p.s.len() && matches!(p.s[p.pos], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
        p.pos += 1;
    }
}

/// `parse_c_ident`
pub(super) fn c_ident(p: &mut Parser) -> Option<String> {
    let start = p.pos;
    let first = *p.s.get(p.pos)?;
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    while p.pos < p.s.len() && (p.s[p.pos].is_ascii_alphanumeric() || p.s[p.pos] == b'_') {
        p.pos += 1;
    }
    Some(String::from_utf8_lossy(&p.s[start..p.pos]).into_owned())
}

/// `spc_util_read_numbers`: up to `n` numbers.
pub(super) fn read_numbers(p: &mut Parser, n: usize) -> Vec<f64> {
    let mut out = Vec::new();
    skip_blank(p);
    while out.len() < n {
        match p.number() {
            Some(v) => out.push(v),
            None => break,
        }
        skip_blank(p);
    }
    out
}

/// `spc_util_read_length` (the `true` prefix needs a magnification of 1).
pub(super) fn read_length(p: &mut Parser) -> Result<f64, ()> {
    let v = p.number().ok_or(())?;
    skip_blank(p);
    let mut unit = 1.0;
    if let Some(mut q) = c_ident(p) {
        if let Some(rest) = q.strip_prefix("true") {
            q = rest.to_string();
            if q.is_empty() {
                skip_blank(p);
                q = c_ident(p).ok_or(())?;
            }
        }
        unit *= match q.as_str() {
            "pt" => 72.0 / 72.27,
            "in" => 72.0,
            "cm" => 72.0 / 2.54,
            "mm" => 72.0 / 25.4,
            "bp" => 1.0,
            "pc" => 12.0 * 72.0 / 72.27,
            "dd" => 1238.0 / 1157.0 * 72.0 / 72.27,
            "cc" => 12.0 * 1238.0 / 1157.0 * 72.0 / 72.27,
            "sp" => 72.0 / (72.27 * 65536.0),
            _ => return Err(()),
        };
    }
    Ok(v * unit)
}

// `transform_info` flags
pub(crate) const INFO_HAS_WIDTH: u8 = 1 << 0;
pub(crate) const INFO_HAS_HEIGHT: u8 = 1 << 1;
pub(crate) const INFO_HAS_USER_BBOX: u8 = 1 << 2;
pub(crate) const INFO_DO_CLIP: u8 = 1 << 3;
pub(crate) const INFO_DO_HIDE: u8 = 1 << 4;

/// dvipdfmx `transform_info`.
#[derive(Clone, Debug)]
pub(crate) struct TransformInfo {
    pub width: f64,
    pub height: f64,
    pub depth: f64,
    pub matrix: Matrix6,
    pub bbox: [f64; 4],
    pub flags: u8,
}

impl Default for TransformInfo {
    fn default() -> Self {
        TransformInfo {
            width: 0.0,
            height: 0.0,
            depth: 0.0,
            matrix: IDENTITY,
            bbox: [0.0; 4],
            flags: 0,
        }
    }
}

/// Image options of `spc_util_read_blahblah` beyond the transformation.
#[derive(Default)]
pub(super) struct ImageOpts {
    pub page: i32,
    pub pagebox: u8,
    pub named: Option<String>,
}

/// `spc_read_dimtrns_pdfm` / `spc_util_read_blahblah`: the keys of `pdf:ann`,
/// `pdf:btrans`, `pdf:bxobj`, `pdf:image`, ... With `image` the `page`,
/// `pagebox` and `named` keys are accepted too.
pub(super) fn read_ti(
    p: &mut Parser,
    image: Option<&mut ImageOpts>,
) -> Result<TransformInfo, String> {
    let mut image = image;
    let mut t = TransformInfo { flags: INFO_DO_CLIP, ..Default::default() };
    let (mut xscale, mut yscale, mut rotate) = (1.0f64, 1.0f64, 0.0f64);
    let (mut has_scale, mut has_xscale, mut has_yscale, mut has_rotate, mut has_matrix) =
        (false, false, false, false, false);
    skip_blank(p);
    while p.pos < p.s.len() {
        let Some(key) = c_ident(p) else { break };
        skip_blank(p);
        let bad = || format!("Unrecognized key or invalid value for dimension/transformation: {key}");
        match key.as_str() {
            "width" => {
                t.width = read_length(p).map_err(|_| bad())?;
                t.flags |= INFO_HAS_WIDTH;
            }
            "height" => {
                t.height = read_length(p).map_err(|_| bad())?;
                t.flags |= INFO_HAS_HEIGHT;
            }
            "depth" => {
                t.depth = read_length(p).map_err(|_| bad())?;
                t.flags |= INFO_HAS_HEIGHT;
            }
            "scale" => {
                let v = p.number().ok_or_else(bad)?;
                xscale = v;
                yscale = v;
                has_scale = true;
            }
            "xscale" => {
                xscale = p.number().ok_or_else(bad)?;
                has_xscale = true;
            }
            "yscale" => {
                yscale = p.number().ok_or_else(bad)?;
                has_yscale = true;
            }
            "rotate" => {
                rotate = std::f64::consts::PI * p.number().ok_or_else(bad)? / 180.0;
                has_rotate = true;
            }
            "bbox" => {
                let v = read_numbers(p, 4);
                if v.len() != 4 {
                    return Err(bad());
                }
                t.bbox = [v[0], v[1], v[2], v[3]];
                t.flags |= INFO_HAS_USER_BBOX;
            }
            "matrix" => {
                let v = read_numbers(p, 6);
                if v.len() != 6 {
                    return Err(bad());
                }
                t.matrix = [v[0], v[1], v[2], v[3], v[4], v[5]];
                has_matrix = true;
            }
            "clip" => {
                if p.number().ok_or_else(bad)? != 0.0 {
                    t.flags |= INFO_DO_CLIP;
                } else {
                    t.flags &= !INFO_DO_CLIP;
                }
            }
            "hide" => t.flags |= INFO_DO_HIDE,
            "page" if image.is_some() => {
                let v = p.number().ok_or_else(bad)?;
                image.as_mut().unwrap().page = v as i32;
            }
            "pagebox" if image.is_some() => {
                let name = c_ident(p).ok_or_else(bad)?;
                let b = match name.as_str() {
                    "cropbox" => crate::boxes::PDFBOX_CROP,
                    "mediabox" => crate::boxes::PDFBOX_MEDIA,
                    "artbox" => crate::boxes::PDFBOX_ART,
                    "trimbox" => crate::boxes::PDFBOX_TRIM,
                    "bleedbox" => crate::boxes::PDFBOX_BLEED,
                    _ => return Err(bad()),
                };
                image.as_mut().unwrap().pagebox = b;
            }
            "named" if image.is_some() => {
                // a PDF string naming the page
                let mut q = Parser::new(&p.s[p.pos..]);
                let s = q.object();
                p.pos += q.pos;
                match s {
                    Some(Obj::Str(s)) => image.as_mut().unwrap().named = Some(String::from_utf8_lossy(&s).into_owned()),
                    _ => return Err(bad()),
                }
            }
            _ => return Err(bad()),
        }
        skip_blank(p);
    }
    let _ = (is_blank as fn(u8) -> bool,);
    if has_xscale && t.flags & INFO_HAS_WIDTH != 0 {
        xscale = 1.0;
    } else if has_yscale && t.flags & INFO_HAS_HEIGHT != 0 {
        yscale = 1.0;
    } else if has_scale && (has_xscale || has_yscale) {
        return Err("Can't supply overall scale along with axis scales.".to_string());
    }
    if !has_matrix {
        let (c, s) = (rotate.cos(), rotate.sin());
        t.matrix = [xscale * c, xscale * s, -yscale * s, yscale * c, 0.0, 0.0];
    }
    let _ = has_rotate;
    if t.flags & INFO_HAS_USER_BBOX == 0 {
        t.flags &= !INFO_DO_CLIP;
    }
    Ok(t)
}

// ---------------------------------------------------------- state: types

/// One entry of dvipdfmx's graphics-state stack: the CTM (in the absolute
/// page frame) and the fill/stroke colours installed in the content stream.
#[derive(Clone, Debug)]
pub(crate) struct Gs {
    pub ctm: Matrix6,
    pub fill: Color,
    pub stroke: Color,
}

impl Gs {
    pub(super) fn initial() -> Gs {
        Gs { ctm: IDENTITY, fill: Color::black(), stroke: Color::black() }
    }
}

/// `pdf_doc` breaking_state + `dvi.c` box tracking.
#[derive(Default)]
pub(crate) struct AnnotState {
    /// the dictionary of the pending `pdf:bann`
    pub dict: Option<Vec<(String, Obj)>>,
    pub broken: bool,
    /// the union of the boxes seen since the annotation (re)started
    pub rect: Option<[f64; 4]>,
    pub link_annot: bool,
    pub compute_boxes: bool,
    pub tagged_depth: i32,
    pub marked_depth: i32,
}

/// A form XObject being grabbed (`pdf:bxobj` .. `pdf:exobj`): everything of
/// the surrounding page state that the form content replaces.
pub(crate) struct FormFrame {
    pub(super) obj: i32,
    pub(super) ref_pt: (f64, f64),
    pub(super) bbox: [f64; 4],
    pub(super) saved_content: String,
    pub(super) saved_gs: Vec<Gs>,
    pub(super) saved_res: Vec<(String, Obj)>,
    pub(super) saved_used_fonts: Vec<(usize, u32)>,
    pub(super) saved_page_fonts: Vec<(usize, u32)>,
    pub(super) saved_ximages: Vec<i32>,
    pub(super) saved_xforms: Vec<i32>,
    pub(super) saved_cur_font: usize,
    pub(super) saved_cur_pdf_font: u32,
    pub(super) q_depth: usize,
}

/// A form XObject known by name.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FormInfo {
    pub obj: i32,
    pub defined: bool,
    /// bounding box relative to the reference point (llx, lly, urx, ury)
    pub bbox: [f64; 4],
}

/// Document-level state of the special interpreter. Lives in the `Engine`
/// for the whole job.
#[derive(Default)]
pub(crate) struct Dpx {
    /// `color_stack`: (stroke, fill, source); index 0 is the permanent base
    pub colors: Vec<(Color, Color, u8)>,
    pub bgcolor: Option<Color>,
    /// user objects by object number, written at the end of the job
    pub objs: BTreeMap<i32, Obj>,
    /// `@name` -> object number
    pub names: HashMap<String, i32>,
    /// `pdf:docinfo` and `pdf:put @docinfo`
    pub docinfo: Vec<(String, Obj)>,
    /// `pdf:docview` and `pdf:put @catalog`
    pub catalog: Vec<(String, Obj)>,
    /// `pdf:dest`, `pdf:names Dests`: key bytes and destination array
    pub dests: Vec<(Vec<u8>, Obj)>,
    /// other `pdf:names` categories
    pub name_trees: BTreeMap<String, Vec<(Vec<u8>, Obj)>>,
    pub outlines: super::dpx_doc::Outlines,
    pub annot: AnnotState,
    /// `coords` of specials.c (bcontent origins)
    pub coords: Vec<(f64, f64)>,
    /// `scaleFactors` of spc_xtx.c
    pub scale_factors: Vec<(f64, f64)>,
    /// `pdf:pageresources`
    pub page_resources: Option<Obj>,
    pub bop: String,
    pub eop: String,
    pub forms: HashMap<String, FormInfo>,
    pub form_stack: Vec<FormFrame>,
    /// `pdf:majorversion`, `pdf:minorversion`
    pub version: (Option<i32>, Option<i32>),
    /// the page being rendered (1-based)
    pub page_no: usize,
    pub taint_keys: Vec<String>,
    /// warned about unsupported specials, once each
    pub warned: std::collections::HashSet<String>,
    /// images placed by name: `pdf:image @ident`
    pub image_names: HashMap<String, i32>,
    /// bookmark `pdf:outline` lowest level seen
    pub lowest_level: i32,
    /// the paper size set by `papersize=` specials (bp), `None` = A4
    pub paper: Option<(f64, f64)>,
    /// dvipdfmx's `landscape_mode`
    pub landscape: bool,
}

impl Dpx {
    pub(crate) fn new() -> Dpx {
        Dpx {
            colors: vec![(Color::black(), Color::black(), SOURCE_DVIPS)],
            annot: AnnotState { link_annot: true, tagged_depth: -1, ..Default::default() },
            taint_keys: ["Title", "Author", "Subject", "Keywords", "Creator", "Producer", "Contents", "Subj", "TU", "T", "TM"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            lowest_level: 255,
            ..Default::default()
        }
    }

    fn current_colors(&self) -> (&Color, &Color) {
        let top = self.colors.last().expect("base colour");
        (&top.0, &top.1)
    }
}

/// Page-local state of the interpreter (lives in the `RenderCtx`).
pub(crate) struct DpxPage {
    pub gs: Vec<Gs>,
    /// `@resources` of the page being built
    pub res: Vec<(String, Obj)>,
}

impl DpxPage {
    pub(crate) fn new() -> DpxPage {
        DpxPage { gs: vec![Gs::initial()], res: Vec::new() }
    }
}

/// The position a special is executed at (`spc_env`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Env {
    pub x: f64,
    pub y: f64,
}

impl<'a> RenderCtx<'a> {
    // ---------------------------------------------------------- small utils

    pub(super) fn dpx_warn(&mut self, message: &str) {
        self.eng.warning_at(&format!("xdvipdfmx warning: {message}"), None);
    }

    pub(super) fn dpx_warn_once(&mut self, key: &str, message: &str) {
        if self.eng.dpx.warned.insert(key.to_string()) {
            self.dpx_warn(message);
        }
    }

    pub(super) fn dpx_env(&self, cur_h: i64, cur_v: i64) -> Env {
        Env { x: sp_to_bp(cur_h), y: -sp_to_bp(cur_v) }
    }

    pub(super) fn dpx_gs(&self) -> &Gs {
        self.dpx.gs.last().expect("graphics state base")
    }

    /// Append `text` as page content in graphics mode (`pdf_doc_add_page_content`
    /// after `graphics_mode()`).
    pub(super) fn dpx_emit(&mut self, text: &str) {
        self.end_text();
        self.content.push_str(text);
        self.content.push('\n');
    }

    /// `pdf_dev_transform(p, NULL)`: through the current CTM.
    pub(super) fn dpx_transform(&self, x: f64, y: f64) -> (f64, f64) {
        mat_apply(&self.dpx_gs().ctm, x, y)
    }

    // ----------------------------------------------------- graphics state

    /// `pdf_dev_gsave`
    pub(crate) fn dpx_gsave(&mut self) {
        let top = self.dpx_gs().clone();
        self.dpx_emit("q");
        self.dpx.gs.push(top);
    }

    /// `pdf_dev_grestore`
    pub(crate) fn dpx_grestore(&mut self) {
        if self.dpx.gs.len() <= 1 {
            self.dpx_warn("Too many grestores.");
            return;
        }
        self.dpx_emit("Q");
        self.dpx.gs.pop();
    }

    /// `pdf_dev_grestore_to`
    pub(super) fn dpx_grestore_to(&mut self, depth: usize) {
        while self.dpx.gs.len() > depth + 1 {
            self.dpx_emit("Q");
            self.dpx.gs.pop();
        }
    }

    /// `pdf_dev_concat`
    pub(crate) fn dpx_concat(&mut self, m: &Matrix6) {
        let det = m[0] * m[3] - m[1] * m[2];
        if det.abs() < 2.5e-16 {
            self.dpx_warn("Transformation matrix not invertible.");
            return;
        }
        const EPS: f64 = 2.5e-16;
        if (m[0] - 1.0).abs() <= EPS && m[1].abs() <= EPS && m[2].abs() <= EPS && (m[3] - 1.0).abs() <= EPS && m[4].abs() <= EPS && m[5].abs() <= EPS {
            return;
        }
        let text = format!("{} cm", fmt_matrix(m));
        self.dpx_emit(&text);
        let ctm = self.dpx_gs().ctm;
        self.dpx.gs.last_mut().unwrap().ctm = mat_mul(m, &ctm);
    }

    /// `pdf_dev_set_color` with `force` set: the operator is written
    /// whether or not the colour changed (`pdf_color_compare` never reports
    /// equal colours).
    pub(super) fn dpx_set_color(&mut self, color: &Color, fill: bool) {
        let ops = color.ops(fill);
        self.dpx_emit(&ops);
        let gs = self.dpx.gs.last_mut().unwrap();
        if fill {
            gs.fill = color.clone();
        } else {
            gs.stroke = color.clone();
        }
    }

    /// `pdf_dev_reset_color`: install the colour stack's current colours.
    pub(crate) fn dpx_reset_color(&mut self) {
        let (s, f) = {
            let (s, f) = self.eng.dpx.current_colors();
            (s.clone(), f.clone())
        };
        self.dpx_set_color(&s, false);
        self.dpx_set_color(&f, true);
    }

    /// `pdf_color_push`
    pub(super) fn dpx_color_push(&mut self, stroke: Color, fill: Color, source: u8) {
        if self.eng.dpx.colors.len() >= 128 {
            self.dpx_warn("Color stack overflow. Just ignore.");
            return;
        }
        self.eng.dpx.colors.push((stroke, fill, source));
        self.dpx_reset_color();
    }

    /// `pdf_color_pop`
    pub(super) fn dpx_color_pop(&mut self, source: u8) {
        let stack = &mut self.eng.dpx.colors;
        if stack.len() <= 1 {
            self.dpx_warn("Color stack underflow. Just ignore.");
            return;
        }
        match (1..stack.len()).rev().find(|&i| stack[i].2 == source) {
            Some(i) => {
                stack.remove(i);
            }
            None => {
                self.dpx_warn(if source == SOURCE_BCOLOR {
                    "Color stack pop: no matching bcolor entry found. Stack unchanged."
                } else {
                    "Color stack pop: no matching push entry found. Stack unchanged."
                });
                return;
            }
        }
        self.dpx_reset_color();
    }

    /// xdvipdfmx `do_glyphs` for a native font with a colour: push the
    /// colour around the glyph array. XeText calls this for fonts with the
    /// `color=` feature.
    pub(crate) fn dpx_push_text_color(&mut self, r: f64, g: f64, b: f64) {
        let c = Color::Rgb([r, g, b]);
        self.dpx_color_push(c.clone(), c, SOURCE_DVIPS);
    }

    /// Matching pop of `dpx_push_text_color`.
    pub(crate) fn dpx_pop_text_color(&mut self) {
        self.dpx_color_pop(SOURCE_DVIPS);
    }

    // -------------------------------------------------------- colour parse

    /// `spc_read_color_pdf`
    pub(super) fn dpx_read_color_pdf(&mut self, p: &mut Parser) -> Result<Color, ()> {
        skip_blank(p);
        match p.peek() {
            Some(b'@') => {
                self.dpx_warn("pdf:bcolor with a ColorSpace resource (@name) is not supported.");
                Err(())
            }
            Some(b'/') => {
                let name = p.name().ok_or(())?;
                skip_blank(p);
                let (n, make): (usize, fn(&[f64]) -> Color) = match name.as_str() {
                    "DeviceGray" => (1, |v| Color::Gray(v[0])),
                    "DeviceRGB" => (3, |v| Color::Rgb([v[0], v[1], v[2]])),
                    "DeviceCMYK" => (4, |v| Color::Cmyk([v[0], v[1], v[2], v[3]])),
                    _ => {
                        self.dpx_warn(&format!("Unknown ColorSpace name specified: {name}"));
                        return Err(());
                    }
                };
                let bracket = p.peek() == Some(b'[');
                if bracket {
                    p.pos += 1;
                }
                let v = read_numbers(p, n);
                if v.len() != n {
                    self.dpx_warn("Wrong number of color components for this ColorSpace...");
                    return Err(());
                }
                if bracket {
                    skip_blank(p);
                    if p.peek() == Some(b']') {
                        p.pos += 1;
                    }
                }
                Ok(make(&v))
            }
            _ => {
                let bracket = p.peek() == Some(b'[');
                if bracket {
                    p.pos += 1;
                    skip_blank(p);
                }
                let v = read_numbers(p, 4);
                let color = match v.len() {
                    1 => Color::Gray(v[0]),
                    3 => Color::Rgb([v[0], v[1], v[2]]),
                    4 => Color::Cmyk([v[0], v[1], v[2], v[3]]),
                    _ => {
                        let name = c_ident(p).ok_or(())?;
                        match named_color(&name) {
                            Some(c) => c,
                            None => {
                                self.dpx_warn(&format!("Unrecognized color name: {name}, keep the current color"));
                                return Err(());
                            }
                        }
                    }
                };
                if bracket {
                    skip_blank(p);
                    if p.peek() == Some(b']') {
                        p.pos += 1;
                    } else {
                        self.dpx_warn("Unbalanced '[' and ']' in color specification.");
                        return Err(());
                    }
                }
                if let Color::Gray(g) = color {
                    if !(0.0..=1.0).contains(&g) {
                        return Err(());
                    }
                }
                Ok(color)
            }
        }
    }

    /// `spc_util_read_pdfcolor`: errors fall back to `default`.
    pub(super) fn dpx_read_pdfcolor(&mut self, p: &mut Parser, default: &Color) -> Color {
        skip_blank(p);
        if p.pos >= p.s.len() {
            return default.clone();
        }
        self.dpx_read_color_pdf(p).unwrap_or_else(|_| default.clone())
    }

    /// `spc_util_read_pdfcolor` for `pdf:bcolor`: no colour at all is an
    /// error, an unreadable one the default.
    pub(super) fn dpx_read_pdfcolor_opt(&mut self, p: &mut Parser, default: &Color) -> Option<Color> {
        skip_blank(p);
        if p.pos >= p.s.len() {
            return None;
        }
        Some(self.dpx_read_pdfcolor(p, default))
    }

    /// `spc_read_color_color`: dvips syntax (`rgb r g b`, `cmyk ..`, names).
    fn dpx_read_color_dvips(&mut self, p: &mut Parser) -> Result<Color, ()> {
        skip_blank(p);
        let q = c_ident(p).ok_or(())?;
        skip_blank(p);
        let (n, make): (usize, fn(&[f64]) -> Color) = match q.as_str() {
            "rgb" => (3, |v| Color::Rgb([v[0], v[1], v[2]])),
            "cmyk" => (4, |v| Color::Cmyk([v[0], v[1], v[2], v[3]])),
            "gray" => (1, |v| Color::Gray(v[0])),
            "hsb" => (3, |v| {
                let (h, s, vv) = (v[0], v[1], v[2]);
                let (mut r, mut g, mut b) = (vv, vv, vv);
                if s != 0.0 {
                    let h6 = h * 6.0;
                    let i = h6.floor() as i32;
                    let f = h6 - f64::from(i);
                    let v1 = vv * (1.0 - s);
                    let v2 = vv * (1.0 - s * f);
                    let v3 = vv * (1.0 - s * (1.0 - f));
                    (r, g, b) = match i {
                        0 => (vv, v3, v1),
                        1 => (v2, vv, v1),
                        2 => (v1, vv, v3),
                        3 => (v1, v2, vv),
                        4 => (v3, v1, vv),
                        _ => (vv, v1, v2),
                    };
                }
                Color::Rgb([r, g, b])
            }),
            _ => {
                return named_color(&q).ok_or_else(|| {
                    self.dpx_warn(&format!("Unrecognized color name: {q}"));
                });
            }
        };
        let v = read_numbers(p, n);
        if v.len() != n || v.iter().any(|c| !(0.0..=1.0).contains(c)) {
            self.dpx_warn(&format!("Invalid value for {q} color specification."));
            return Err(());
        }
        Ok(make(&v))
    }

    // ------------------------------------------------------- the dispatcher

    /// Execute one `\special` with xdvipdfmx semantics (`dvi_do_special`:
    /// text is closed first, then the matching handler runs).
    pub(super) fn dpx_special(&mut self, text: &str, cur_h: i64, cur_v: i64) {
        self.end_text();
        let env = self.dpx_env(cur_h, cur_v);
        let bytes = text.as_bytes();
        let mut p = Parser::new(bytes);
        skip_blank(&mut p);
        if p.pos >= bytes.len() {
            return;
        }
        let rest_start = p.pos;
        let res: Result<(), String> = if bytes[rest_start..].starts_with(b"pdf:") {
            p.pos += 4;
            skip_blank(&mut p);
            let cmd = c_ident(&mut p);
            // `pdf:direct:` / `pdf:page:` (pdftex compat)
            let compat = p.peek() == Some(b':');
            match cmd {
                Some(cmd) if compat => {
                    p.pos += 1;
                    skip_blank(&mut p);
                    match cmd.as_str() {
                        "direct" | "page" => self.dpx_literal_body(&mut p, env, cmd == "direct", true),
                        _ => Err(format!("unknown pdf: special {cmd}")),
                    }
                }
                Some(cmd) => {
                    skip_blank(&mut p);
                    self.dpx_pdfm(&cmd, &mut p, env, cur_h, cur_v)
                }
                None => Err("empty pdf: special".to_string()),
            }
        } else if bytes[rest_start..].starts_with(b"x:") {
            p.pos += 2;
            skip_blank(&mut p);
            match c_ident(&mut p) {
                Some(cmd) => {
                    skip_blank(&mut p);
                    self.dpx_xtx(&cmd, &mut p, env)
                }
                None => Err("empty x: special".to_string()),
            }
        } else if bytes[rest_start..].starts_with(b"dvipdfmx:") {
            p.pos += 9;
            skip_blank(&mut p);
            match c_ident(&mut p).as_deref() {
                Some("config") | Some("catch_phantom") => Ok(()),
                other => Err(format!("unknown dvipdfmx: special {other:?}")),
            }
        } else {
            let word = c_ident(&mut p);
            match word.as_deref() {
                Some("color") => {
                    skip_blank(&mut p);
                    self.dpx_color_special(&mut p)
                }
                Some("background") => {
                    skip_blank(&mut p);
                    match self.dpx_read_color_dvips(&mut p) {
                        Ok(c) => {
                            self.eng.dpx.bgcolor = Some(c);
                            Ok(())
                        }
                        Err(()) => Err("No valid color specified?".to_string()),
                    }
                }
                Some("pdfcolorstack") => Err("pdfcolorstack special is not supported".to_string()),
                Some("papersize") | Some("landscape") => Ok(()),
                _ if bytes[rest_start..].starts_with(b"papersize=") => Ok(()),
                _ => {
                    // PostScript, html: and unknown specials draw nothing
                    // in xdvipdfmx's PDF output model either
                    Ok(())
                }
            }
        };
        if let Err(message) = res {
            let shown: String = text.chars().take(60).collect();
            self.dpx_warn(&format!("Interpreting special command failed: {message} (xxx \"{shown}\")"));
        }
    }

    /// `color push|pop|<spec>`
    fn dpx_color_special(&mut self, p: &mut Parser) -> Result<(), String> {
        let save = p.pos;
        let word = c_ident(p);
        match word.as_deref() {
            Some("push") => {
                skip_blank(p);
                let c = self
                    .dpx_read_color_dvips(p)
                    .map_err(|()| "No valid color specified?".to_string())?;
                self.dpx_color_push(c.clone(), c, SOURCE_DVIPS);
                Ok(())
            }
            Some("pop") => {
                self.dpx_color_pop(SOURCE_DVIPS);
                Ok(())
            }
            _ => {
                p.pos = save;
                let c = self
                    .dpx_read_color_dvips(p)
                    .map_err(|()| "No valid color specified?".to_string())?;
                // `pdf_color_clear_stack` + `pdf_color_set`
                let stack = &mut self.eng.dpx.colors;
                if stack.len() > 1 {
                    self.dpx_warn("You've mistakenly made a global color change within nested colors.");
                }
                let stack = &mut self.eng.dpx.colors;
                stack.truncate(1);
                stack[0] = (c.clone(), c, SOURCE_DVIPS);
                self.dpx_reset_color();
                Ok(())
            }
        }
    }

    /// The `x:` family of spc_xtx.c.
    fn dpx_xtx(&mut self, cmd: &str, p: &mut Parser, env: Env) -> Result<(), String> {
        match cmd {
            "gsave" => {
                self.dpx_gsave();
                Ok(())
            }
            "grestore" => {
                self.dpx_grestore();
                self.dpx_reset_color();
                Ok(())
            }
            "scale" | "bscale" => {
                let v = read_numbers(p, 2);
                if v.len() < 2 {
                    return Err("two numbers expected".to_string());
                }
                if cmd == "bscale" {
                    if v[0].abs() < 1e-7 || v[1].abs() < 1e-7 {
                        return Err("degenerate scale".to_string());
                    }
                    self.eng.dpx.scale_factors.push((1.0 / v[0], 1.0 / v[1]));
                }
                self.dpx_do_transform(env, v[0], 0.0, 0.0, v[1], 0.0, 0.0);
                Ok(())
            }
            "escale" => {
                let (fx, fy) = self.eng.dpx.scale_factors.pop().ok_or("escale without bscale")?;
                self.dpx_do_transform(env, fx, 0.0, 0.0, fy, 0.0, 0.0);
                Ok(())
            }
            "rotate" => {
                let v = read_numbers(p, 1);
                if v.is_empty() {
                    return Err("a number expected".to_string());
                }
                let (s, c) = (v[0] * std::f64::consts::PI / 180.0).sin_cos();
                self.dpx_do_transform(env, c, s, -s, c, 0.0, 0.0);
                Ok(())
            }
            "papersize" => Ok(()),
            "backgroundcolor" => {
                let c = self
                    .dpx_read_color_pdf(p)
                    .map_err(|()| "No valid color specified?".to_string())?;
                self.eng.dpx.bgcolor = Some(c);
                Ok(())
            }
            "fontmapline" | "fontmapfile" => {
                let rest = String::from_utf8_lossy(&p.s[p.pos..]).into_owned();
                let is_file = cmd == "fontmapfile";
                self.eng.process_map_item(rest.trim(), is_file);
                Ok(())
            }
            _ => Err(format!("unknown x: special {cmd}")),
        }
    }

    /// `spc_handler_xtx_do_transform`: `cm` about the current point.
    fn dpx_do_transform(&mut self, env: Env, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) {
        let m = [a, b, c, d, (1.0 - a) * env.x - c * env.y + e, (1.0 - d) * env.y - b * env.x + f];
        self.dpx_concat(&m);
    }

    // --------------------------------------------------- literal and code

    /// `spc_get_current_point`: the point relative to the innermost
    /// `pdf:bcontent` origin.
    pub(super) fn dpx_current_point(&self, env: Env) -> (f64, f64) {
        match self.eng.dpx.coords.last() {
            Some(&(cx, cy)) => (env.x - cx, env.y - cy),
            None => (env.x, env.y),
        }
    }

    /// `spc_handler_pdfm_literal` after the keyword scan; `compat` is the
    /// `pdf:page:`/`pdf:direct:` spelling (always a single payload).
    pub(super) fn dpx_literal_body(&mut self, p: &mut Parser, env: Env, direct: bool, _compat: bool) -> Result<(), String> {
        let (cx, cy) = self.dpx_current_point(env);
        if p.pos < p.s.len() {
            let payload = String::from_utf8_lossy(&p.s[p.pos..]).into_owned();
            if !direct {
                self.dpx_concat(&[1.0, 0.0, 0.0, 1.0, cx, cy]);
            }
            self.dpx_emit(&payload);
            if !direct {
                self.dpx_concat(&[1.0, 0.0, 0.0, 1.0, -cx, -cy]);
            }
        }
        Ok(())
    }

    /// `pdf:bcontent`
    pub(super) fn dpx_bcontent(&mut self, env: Env) {
        self.dpx_gsave();
        let (xpos, ypos) = self.eng.dpx.coords.last().copied().unwrap_or((0.0, 0.0));
        self.dpx_concat(&[1.0, 0.0, 0.0, 1.0, env.x - xpos, env.y - ypos]);
        self.eng.dpx.coords.push((env.x, env.y));
        self.dpx_set_compensation();
    }

    /// `pdf:econtent`
    pub(super) fn dpx_econtent(&mut self) {
        self.eng.dpx.coords.pop();
        self.dpx_set_compensation();
        self.dpx_grestore();
        self.dpx_reset_color();
    }

    /// `dvi_set_compensation`: glyph and rule positions inside `bcontent`
    /// are expressed relative to its origin. Positions here stay absolute
    /// (the translating `cm` is part of the stream), so the compensation
    /// needs no state of its own.
    fn dpx_set_compensation(&mut self) {}

    /// `pdf:btrans`
    pub(super) fn dpx_btrans(&mut self, p: &mut Parser, env: Env) -> Result<(), String> {
        let ti = read_ti(p, None)?;
        let (cx, cy) = self.dpx_current_point(env);
        let mut m = ti.matrix;
        m[4] += (1.0 - m[0]) * cx - m[2] * cy;
        m[5] += (1.0 - m[3]) * cy - m[1] * cx;
        self.dpx_gsave();
        self.dpx_concat(&m);
        Ok(())
    }

    /// `pdf:etrans`
    pub(super) fn dpx_etrans(&mut self) {
        self.dpx_grestore();
        self.dpx_reset_color();
    }

    // ----------------------------------------------------------- page hooks

    /// `pdf_dev_bop` + `pdf_doc_begin_page` for the page being rendered:
    /// the background of the colour stack is installed.
    pub(super) fn dpx_begin_page(&mut self, scale: f64, x_origin: f64, y_origin: f64) {
        self.dpx = DpxPage::new();
        self.eng.dpx.page_no = self.eng.pdf_doc.pages.len() + 1;
        // pdf_dev_bop: the DVI origin becomes the origin of the page content
        self.dpx_gsave();
        self.dpx_concat(&[scale, 0.0, 0.0, scale, x_origin, y_origin]);
        self.dpx_reset_color();
        let bop = self.eng.dpx.bop.clone();
        if !bop.is_empty() {
            self.dpx_emit(&bop);
        }
    }

    /// `pdf_dev_eop` + `doc_fill_page_background` + `spc_pdfm_at_end_page`.
    /// Returns the page's resource dictionary entries.
    pub(super) fn dpx_end_page(&mut self, width_bp: f64, height_bp: f64) -> Vec<(String, Obj)> {
        self.end_text();
        let eop = self.eng.dpx.eop.clone();
        if !eop.is_empty() {
            self.dpx_emit(&eop);
        }
        let depth = self.dpx.gs.len() - 1;
        if depth != 1 {
            self.dpx_warn(&format!("Unbalenced q/Q nesting...: {depth}"));
            self.dpx_grestore_to(0);
        } else {
            self.dpx_grestore();
        }
        // doc_fill_page_background: the colour at the end of the page
        if let Some(bg) = self.eng.dpx.bgcolor.clone().filter(|c| !c.is_white()) {
            let mut s = String::from("q");
            s.push_str(&bg.ops(true));
            s.push_str(&format!(
                " q n 0 0 {} {} re f Q Q\n",
                p_dtoa(width_bp, PRECISION),
                p_dtoa(height_bp, PRECISION)
            ));
            self.content.insert_str(0, &s);
        }
        // pdf:pageresources
        if let Some(Obj::Dict(cats)) = self.eng.dpx.page_resources.clone() {
            for (cat, value) in cats {
                self.dpx_put_resource(&cat, value);
            }
        }
        std::mem::take(&mut self.dpx.res)
    }

    /// `safeputresdict`: merge `value` into the resource category `cat` of
    /// the page (or the form being grabbed).
    pub(super) fn dpx_put_resource(&mut self, cat: &str, value: Obj) {
        let res = &mut self.dpx.res;
        match (res.iter_mut().find(|(k, _)| k == cat), value) {
            (None, v) => res.push((cat.to_string(), v)),
            (Some((_, Obj::Dict(old))), Obj::Dict(new)) => {
                for (k, v) in new {
                    if old.iter().any(|(ok, _)| *ok == k) {
                        // "Object already defined in dict! (ignored)"
                        continue;
                    }
                    old.push((k, v));
                }
            }
            (Some(slot), v @ Obj::Ref(_)) => {
                // the new indirect dictionary replaces the old category
                slot.1 = v;
            }
            (Some(_), _) => {}
        }
    }

    // ----------------------------------------------------- box tracking

    /// `dvi_is_tracking_boxes`
    pub(crate) fn dpx_tracking(&self) -> bool {
        let a = &self.eng.dpx.annot;
        a.compute_boxes && a.link_annot && a.marked_depth >= a.tagged_depth
    }

    /// `pdf_doc_expand_box`
    pub(super) fn dpx_expand_box(&mut self, rect: [f64; 4]) {
        let a = &mut self.eng.dpx.annot;
        a.rect = Some(match a.rect {
            Some(r) => [r[0].min(rect[0]), r[1].min(rect[1]), r[2].max(rect[2]), r[3].max(rect[3])],
            None => rect,
        });
    }

    /// `pdf_dev_set_rule`: a rule `w_sp` by `h_sp` whose lower left corner is
    /// at `x_sp`, `v_down_sp` below the origin. Thin rules are strokes.
    pub(crate) fn dpx_rule(&mut self, x_sp: i64, v_down_sp: i64, w_sp: i64, h_sp: i64) {
        self.end_text();
        let ypos = -v_down_sp;
        let bp = |sp: i64| p_dtoa(sp_to_bp(sp), PRECISION);
        let thickness = sp_to_bp(w_sp.min(h_sp));
        let body = if !(0.0..=5.0).contains(&thickness) {
            format!("{} {} {} {} re f", bp(x_sp), bp(ypos), bp(w_sp), bp(h_sp))
        } else if w_sp > h_sp {
            format!(
                "{} w {} {} m {} {} l S",
                p_dtoa(sp_to_bp(h_sp), PRECISION + 1),
                bp(x_sp),
                bp(ypos + h_sp / 2),
                bp(x_sp + w_sp),
                bp(ypos + h_sp / 2)
            )
        } else {
            format!(
                "{} w {} {} m {} {} l S",
                p_dtoa(sp_to_bp(w_sp), PRECISION + 1),
                bp(x_sp + w_sp / 2),
                bp(ypos),
                bp(x_sp + w_sp / 2),
                bp(ypos + h_sp)
            )
        };
        self.dpx_emit(&format!("q {body} Q"));
    }

    /// `pdf_dev_set_rect` + `pdf_doc_expand_box`: a box with its lower left
    /// reference point (`h_sp`, baseline `v_sp` down from the page top)
    /// `width_sp` wide, `height_sp` above and `depth_sp` below the baseline.
    pub(crate) fn dpx_track_box(&mut self, h_sp: i64, v_sp: i64, width_sp: i64, height_sp: i64, depth_sp: i64) {
        if !self.dpx_tracking() {
            return;
        }
        let x = sp_to_bp(h_sp);
        let y = -sp_to_bp(v_sp);
        let (w, h, d) = (sp_to_bp(width_sp), sp_to_bp(height_sp), sp_to_bp(depth_sp));
        let corners = [(x, y - d), (x + w, y - d), (x + w, y + h), (x, y + h)];
        let mut r = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for (cx, cy) in corners {
            let (tx, ty) = self.dpx_transform(cx, cy);
            r = [r[0].min(tx), r[1].min(ty), r[2].max(tx), r[3].max(ty)];
        }
        self.dpx_expand_box(r);
    }

    /// `dvi_push`/`dvi_pop` + `dvi_mark_depth`: called when TeX's DVI
    /// stack would change; `depth` is the new stack depth.
    pub(super) fn dpx_mark_depth(&mut self, depth: i32) {
        let (link, marked, tagged) = {
            let a = &self.eng.dpx.annot;
            (a.link_annot, a.marked_depth, a.tagged_depth)
        };
        // end of a "logical unit" that has been broken
        if link && marked == tagged && depth == tagged - 1 {
            self.dpx_break_annot();
        }
        self.eng.dpx.annot.marked_depth = depth;
    }
}
