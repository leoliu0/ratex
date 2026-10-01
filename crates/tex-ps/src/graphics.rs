//! Graphics state, paths, colour and painting: PostScript operators compiled
//! to a PDF content stream.
//!
//! Paths are kept in device space (the EPS default user space, which is also
//! the PDF page space). Strokes honour the CTM at stroke time: conformal CTMs
//! scale the line width and dash pattern, others stroke inside `q cm … Q`.
//! PDF graphics state (colours, line parameters) is emitted lazily, only when
//! a painting operator needs it, and mirrors `gsave`/`grestore` with `q`/`Q`.

use std::rc::Rc;

use crate::interp::{err, Interp, OpFn, Res, MAX_GSTACK};
use crate::types::{deg_sin_cos, Key, Matrix, Value};

const MAX_OUTPUT: usize = 256 << 20;
const MAX_PATH: usize = 4 << 20;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    Curve([f64; 6]),
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DevColor {
    Gray(f64),
    Rgb(f64, f64, f64),
    Cmyk(f64, f64, f64, f64),
}

#[derive(Clone, Debug)]
pub(crate) enum ColorSpace {
    Gray,
    Rgb,
    Cmyk,
    Indexed { base: Box<ColorSpace>, hival: u32, lookup: Rc<Vec<u8>> },
    Separation { alt: Box<ColorSpace>, tint: Value },
    DeviceN { n: usize, alt: Box<ColorSpace>, tint: Value },
    Pattern,
}

impl ColorSpace {
    pub(crate) fn ncomp(&self) -> usize {
        match self {
            Self::Gray | Self::Indexed { .. } | Self::Separation { .. } => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
            Self::DeviceN { n, .. } => *n,
            Self::Pattern => 0,
        }
    }

    fn initial(&self) -> Vec<f64> {
        match self {
            Self::Gray | Self::Indexed { .. } => vec![0.0],
            Self::Rgb => vec![0.0; 3],
            Self::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            Self::Separation { .. } => vec![1.0],
            Self::DeviceN { n, .. } => vec![1.0; *n],
            Self::Pattern => Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GState {
    pub(crate) ctm: Matrix,
    pub(crate) path: Vec<Seg>,
    /// Current point and current subpath start, in device space.
    pub(crate) cp: Option<(f64, f64)>,
    start: Option<(f64, f64)>,
    lw: f64,
    cap: u8,
    join: u8,
    miter: f64,
    dash: Rc<Vec<f64>>,
    dash_offset: f64,
    pub(crate) space: ColorSpace,
    comps: Vec<f64>,
    /// The colour painted; `None` for pattern colours, which are not rendered.
    pub(crate) device: Option<DevColor>,
    pub(crate) font: Option<crate::types::PsDict>,
    flat: f64,
    stroke_adjust: bool,
    overprint: bool,
    /// Set by `nulldevice`: painting is discarded until the state is restored.
    null_device: bool,
}

impl Default for GState {
    fn default() -> Self {
        Self {
            ctm: Matrix::IDENTITY,
            path: Vec::new(),
            cp: None,
            start: None,
            lw: 1.0,
            cap: 0,
            join: 0,
            miter: 10.0,
            dash: Rc::new(Vec::new()),
            dash_offset: 0.0,
            space: ColorSpace::Gray,
            comps: vec![0.0],
            device: Some(DevColor::Gray(0.0)),
            font: None,
            flat: 1.0,
            stroke_adjust: false,
            overprint: false,
            null_device: false,
        }
    }
}

/// PDF graphics state already written to the content stream.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PdfState {
    fill: DevColor,
    stroke: DevColor,
    lw: f64,
    cap: u8,
    join: u8,
    miter: f64,
    dash: (Rc<Vec<f64>>, f64),
}

impl Default for PdfState {
    fn default() -> Self {
        Self {
            fill: DevColor::Gray(0.0),
            stroke: DevColor::Gray(0.0),
            lw: 1.0,
            cap: 0,
            join: 0,
            miter: 10.0,
            dash: (Rc::new(Vec::new()), 0.0),
        }
    }
}

pub(crate) struct Saved {
    gs: GState,
    pdf: PdfState,
    by_save: bool,
    /// Pushed by `clipsave`: `cliprestore` reverts only the clip.
    clip_only: bool,
    emitted: bool,
}

#[derive(Default)]
pub(crate) struct Output {
    pub(crate) content: Vec<u8>,
    pub(crate) pdf: PdfState,
    pub(crate) fonts: Vec<crate::fonts::FontRes>,
    pub(crate) images: Vec<crate::images::ImageRes>,
}

/// Appends a number with at most `decimals` fractional digits, trailing zeros trimmed.
pub(crate) fn put_num_prec(out: &mut Vec<u8>, v: f64, decimals: u32) {
    let scale = 10f64.powi(decimals as i32);
    let scaled = (v * scale).round();
    let mut n = if scaled.is_finite() && scaled.abs() < 9.0e15 { scaled as i64 } else { 0 };
    if n < 0 {
        out.push(b'-');
        n = -n;
    }
    let unit = scale as i64;
    let int = n / unit;
    let mut frac = n % unit;
    let mut digits = [0u8; 20];
    let mut k = digits.len();
    let mut x = int;
    loop {
        k -= 1;
        digits[k] = b'0' + (x % 10) as u8;
        x /= 10;
        if x == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[k..]);
    if frac != 0 {
        let mut f = [b'0'; 12];
        for slot in f[..decimals as usize].iter_mut().rev() {
            *slot = b'0' + (frac % 10) as u8;
            frac /= 10;
        }
        let mut end = decimals as usize;
        while f[end - 1] == b'0' {
            end -= 1;
        }
        out.push(b'.');
        out.extend_from_slice(&f[..end]);
    }
}

#[inline]
pub(crate) fn put_num(out: &mut Vec<u8>, v: f64) {
    put_num_prec(out, v, 4);
}

pub(crate) fn put_nums(out: &mut Vec<u8>, vals: &[f64], op: &[u8]) {
    for &v in vals {
        put_num(out, v);
        out.push(b' ');
    }
    out.extend_from_slice(op);
    out.push(b'\n');
}

pub(crate) fn put_matrix(out: &mut Vec<u8>, m: &Matrix) {
    for v in m.to_array() {
        put_num_prec(out, v, 6);
        out.push(b' ');
    }
    out.extend_from_slice(b"cm\n");
}

fn put_color(out: &mut Vec<u8>, c: DevColor, stroke: bool) {
    match c {
        DevColor::Gray(g) => put_nums(out, &[g], if stroke { b"G" } else { b"g" }),
        DevColor::Rgb(r, g, b) => put_nums(out, &[r, g, b], if stroke { b"RG" } else { b"rg" }),
        DevColor::Cmyk(c, m, y, k) => put_nums(out, &[c, m, y, k], if stroke { b"K" } else { b"k" }),
    }
}

fn put_path(out: &mut Vec<u8>, path: &[Seg], inv: Option<&Matrix>) {
    let map = |x: f64, y: f64| inv.map_or((x, y), |m| m.apply(x, y));
    for seg in path {
        match *seg {
            Seg::Move(x, y) => {
                let (x, y) = map(x, y);
                put_nums(out, &[x, y], b"m");
            }
            Seg::Line(x, y) => {
                let (x, y) = map(x, y);
                put_nums(out, &[x, y], b"l");
            }
            Seg::Curve(c) => {
                let (x1, y1) = map(c[0], c[1]);
                let (x2, y2) = map(c[2], c[3]);
                let (x3, y3) = map(c[4], c[5]);
                put_nums(out, &[x1, y1, x2, y2, x3, y3], b"c");
            }
            Seg::Close => out.extend_from_slice(b"h\n"),
        }
    }
}

fn has_marks(path: &[Seg]) -> bool {
    path.iter().any(|s| !matches!(s, Seg::Move(..)))
}

impl Interp {
    pub(crate) fn emitting(&self) -> bool {
        self.suppress == 0 && self.charpaths.is_empty() && !self.gs.null_device
    }

    pub(crate) fn check_output(&self) -> Res {
        if self.out.content.len() > MAX_OUTPUT {
            return err("limitcheck", "PDF content stream too large");
        }
        Ok(())
    }

    pub(crate) fn sync_fill(&mut self, c: DevColor) {
        if self.out.pdf.fill != c {
            put_color(&mut self.out.content, c, false);
            self.out.pdf.fill = c;
        }
    }

    fn sync_stroke_params(&mut self, c: DevColor, scale: f64) {
        let out = &mut self.out;
        if out.pdf.stroke != c {
            put_color(&mut out.content, c, true);
            out.pdf.stroke = c;
        }
        let lw = self.gs.lw * scale;
        if out.pdf.lw != lw {
            put_nums(&mut out.content, &[lw], b"w");
            out.pdf.lw = lw;
        }
        if out.pdf.cap != self.gs.cap {
            put_nums(&mut out.content, &[f64::from(self.gs.cap)], b"J");
            out.pdf.cap = self.gs.cap;
        }
        if out.pdf.join != self.gs.join {
            put_nums(&mut out.content, &[f64::from(self.gs.join)], b"j");
            out.pdf.join = self.gs.join;
        }
        if out.pdf.miter != self.gs.miter {
            put_nums(&mut out.content, &[self.gs.miter], b"M");
            out.pdf.miter = self.gs.miter;
        }
        let dash: Rc<Vec<f64>> = if scale == 1.0 {
            self.gs.dash.clone()
        } else {
            Rc::new(self.gs.dash.iter().map(|d| d * scale).collect())
        };
        let offset = self.gs.dash_offset * scale;
        if *out.pdf.dash.0 != *dash || out.pdf.dash.1 != offset {
            put_dash(&mut out.content, &dash, offset);
            out.pdf.dash = (dash, offset);
        }
    }

    fn user_point(&self, x: f64, y: f64) -> Res<(f64, f64)> {
        match self.gs.ctm.invert() {
            Some(inv) => Ok(inv.apply(x, y)),
            None => err("undefinedresult", "singular current transformation matrix"),
        }
    }

    pub(crate) fn current_user_point(&self) -> Res<(f64, f64)> {
        let (x, y) = self.gs.cp.map_or_else(|| err("nocurrentpoint", "no current point"), Ok)?;
        self.user_point(x, y)
    }

    fn push_seg(&mut self, seg: Seg) -> Res {
        if self.gs.path.len() >= MAX_PATH {
            return err("limitcheck", "path too long");
        }
        self.alloc(std::mem::size_of::<Seg>())?;
        self.gs.path.push(seg);
        Ok(())
    }

    pub(crate) fn move_to_device(&mut self, x: f64, y: f64) -> Res {
        if let Some(Seg::Move(..)) = self.gs.path.last() {
            self.gs.path.pop();
        }
        self.push_seg(Seg::Move(x, y))?;
        self.gs.cp = Some((x, y));
        self.gs.start = Some((x, y));
        Ok(())
    }

    fn line_to_device(&mut self, x: f64, y: f64) -> Res {
        if self.gs.cp.is_none() {
            return err("nocurrentpoint", "lineto");
        }
        self.reopen_subpath()?;
        self.push_seg(Seg::Line(x, y))?;
        self.gs.cp = Some((x, y));
        Ok(())
    }

    fn curve_to_device(&mut self, c: [f64; 6]) -> Res {
        if self.gs.cp.is_none() {
            return err("nocurrentpoint", "curveto");
        }
        self.reopen_subpath()?;
        self.push_seg(Seg::Curve(c))?;
        self.gs.cp = Some((c[4], c[5]));
        Ok(())
    }

    /// After `closepath`, drawing continues in a new subpath at the start point.
    fn reopen_subpath(&mut self) -> Res {
        if let Some(Seg::Close) = self.gs.path.last() {
            let (x, y) = self.gs.cp.unwrap();
            self.push_seg(Seg::Move(x, y))?;
        }
        Ok(())
    }

    fn close_path(&mut self) -> Res {
        match self.gs.path.last() {
            None | Some(Seg::Close) => Ok(()),
            Some(_) => {
                self.push_seg(Seg::Close)?;
                self.gs.cp = self.gs.start;
                Ok(())
            }
        }
    }

    pub(crate) fn new_path(&mut self) {
        self.gs.path.clear();
        self.gs.cp = None;
        self.gs.start = None;
    }

    /// Appends an arc (user space centre/radius, degrees) as Bézier curves.
    fn arc(&mut self, cx: f64, cy: f64, r: f64, a1: f64, a2: f64, clockwise: bool) -> Res {
        let sweep = if clockwise { -(a1 - a2).rem_euclid(360.0) } else { (a2 - a1).rem_euclid(360.0) };
        let sweep = if sweep == 0.0 && a1 != a2 { if clockwise { -360.0 } else { 360.0 } } else { sweep };
        let ctm = self.gs.ctm;
        let (s1, c1) = deg_sin_cos(a1);
        let (x0, y0) = ctm.apply(cx + r * c1, cy + r * s1);
        if self.gs.cp.is_some() {
            self.line_to_device(x0, y0)?;
        } else {
            self.move_to_device(x0, y0)?;
        }
        let segments = (sweep.abs() / 90.0).ceil().max(1.0) as usize;
        let step = sweep / segments as f64;
        let k = 4.0 / 3.0 * (step.to_radians() / 4.0).tan();
        let mut a = a1;
        for _ in 0..segments {
            let b = a + step;
            let (sa, ca) = deg_sin_cos(a);
            let (sb, cb) = deg_sin_cos(b);
            let p1 = ctm.apply(cx + r * (ca - k * sa), cy + r * (sa + k * ca));
            let p2 = ctm.apply(cx + r * (cb + k * sb), cy + r * (sb - k * cb));
            let p3 = ctm.apply(cx + r * cb, cy + r * sb);
            self.curve_to_device([p1.0, p1.1, p2.0, p2.1, p3.0, p3.1])?;
            a = b;
        }
        Ok(())
    }

    /// `arct`/`arcto`: returns the two tangent points in user space.
    fn arc_tangent(&mut self) -> Res<[f64; 4]> {
        let r = self.f64_at(0)?;
        let y2 = self.f64_at(1)?;
        let x2 = self.f64_at(2)?;
        let y1 = self.f64_at(3)?;
        let x1 = self.f64_at(4)?;
        let (x0, y0) = self.current_user_point()?;
        self.pop_n(5);
        let (dx1, dy1) = (x0 - x1, y0 - y1);
        let (dx2, dy2) = (x2 - x1, y2 - y1);
        let (l1, l2) = (dx1.hypot(dy1), dx2.hypot(dy2));
        let cross = dx1 * dy2 - dy1 * dx2;
        let (p1x, p1y) = self.gs.ctm.apply(x1, y1);
        if l1 == 0.0 || l2 == 0.0 || cross.abs() < 1e-12 * l1 * l2 || r == 0.0 {
            self.line_to_device(p1x, p1y)?;
            return Ok([x1, y1, x1, y1]);
        }
        let (u1, v1) = (dx1 / l1, dy1 / l1);
        let (u2, v2) = (dx2 / l2, dy2 / l2);
        let cos = (u1 * u2 + v1 * v2).clamp(-1.0, 1.0);
        let half = cos.acos() / 2.0;
        let dist = r.abs() / half.tan();
        let (t1x, t1y) = (x1 + u1 * dist, y1 + v1 * dist);
        let (t2x, t2y) = (x1 + u2 * dist, y1 + v2 * dist);
        let (bx, by) = (u1 + u2, v1 + v2);
        let bl = bx.hypot(by);
        let centre_dist = r.abs() / half.sin();
        let (cx, cy) = (x1 + bx / bl * centre_dist, y1 + by / bl * centre_dist);
        let start = (t1y - cy).atan2(t1x - cx).to_degrees();
        let end = (t2y - cy).atan2(t2x - cx).to_degrees();
        // Turning left along p0 -> p1 -> p2 means a counter-clockwise arc.
        let left = (x1 - x0) * (y2 - y1) - (y1 - y0) * (x2 - x1) > 0.0;
        self.arc(cx, cy, r.abs(), start, end, !left)?;
        Ok([t1x, t1y, t2x, t2y])
    }

    pub(crate) fn fill_path(&mut self, even_odd: bool) -> Res {
        if let Some(acc) = self.charpaths.last_mut() {
            acc.append(&mut self.gs.path);
            self.new_path();
            return Ok(());
        }
        if self.emitting() && has_marks(&self.gs.path) {
            if let Some(c) = self.gs.device {
                self.sync_fill(c);
                put_path(&mut self.out.content, &self.gs.path, None);
                self.out.content.extend_from_slice(if even_odd { b"f*\n" } else { b"f\n" });
                self.check_output()?;
            }
        }
        self.new_path();
        Ok(())
    }

    pub(crate) fn stroke_path(&mut self) -> Res {
        if let Some(acc) = self.charpaths.last_mut() {
            acc.append(&mut self.gs.path);
            self.new_path();
            return Ok(());
        }
        if self.emitting() && has_marks(&self.gs.path) {
            if let Some(c) = self.gs.device {
                let m = self.gs.ctm;
                let det = m.a * m.d - m.b * m.c;
                let norm = m.a.hypot(m.b).max(m.c.hypot(m.d));
                let tol = 1e-9 * norm;
                let conformal = ((m.a - m.d).abs() <= tol && (m.b + m.c).abs() <= tol)
                    || ((m.a + m.d).abs() <= tol && (m.b - m.c).abs() <= tol);
                if conformal {
                    self.sync_stroke_params(c, det.abs().sqrt());
                    put_path(&mut self.out.content, &self.gs.path, None);
                    self.out.content.extend_from_slice(b"S\n");
                } else if let Some(inv) = m.invert() {
                    self.sync_stroke_params(c, 1.0);
                    let out = &mut self.out.content;
                    out.extend_from_slice(b"q\n");
                    put_matrix(out, &m);
                    put_path(out, &self.gs.path, Some(&inv));
                    out.extend_from_slice(b"S\nQ\n");
                }
                self.check_output()?;
            }
        }
        self.new_path();
        Ok(())
    }

    fn clip_path(&mut self, even_odd: bool) -> Res {
        if self.emitting() && has_marks(&self.gs.path) {
            put_path(&mut self.out.content, &self.gs.path, None);
            self.out.content.extend_from_slice(if even_odd { b"W* n\n" } else { b"W n\n" });
            self.check_output()?;
        }
        Ok(())
    }

    // ---- colour --------------------------------------------------------------

    pub(crate) fn parse_color_space(&mut self, v: &Value, depth: u32) -> Res<ColorSpace> {
        if depth > 8 {
            return err("limitcheck", "colour space nested too deeply");
        }
        let n = &self.n;
        let (family, arr) = match v {
            Value::Name(id) | Value::ExecName(id) => (*id, None),
            Value::Array(a) | Value::Proc(a) if a.len > 0 => match a.get(0) {
                Value::Name(id) | Value::ExecName(id) => (id, Some(a.clone())),
                _ => return err("typecheck", "colour space"),
            },
            _ => return err("typecheck", "colour space"),
        };
        let elem = |k: u32| arr.as_ref().filter(|a| a.len > k).map(|a| a.get(k));
        Ok(if family == n.device_gray || family == n.cie_a || family == n.cal_gray {
            ColorSpace::Gray
        } else if family == n.device_rgb || family == n.cie_abc || family == n.cie_def || family == n.cal_rgb || family == n.lab {
            ColorSpace::Rgb
        } else if family == n.device_cmyk || family == n.cie_defg {
            ColorSpace::Cmyk
        } else if family == n.pattern {
            ColorSpace::Pattern
        } else if family == n.icc_based {
            let ncomp = match elem(1) {
                Some(Value::Dict(d)) => d.get(&Key::Name(self.n.n)).and_then(|v| v.as_f64()).unwrap_or(3.0),
                _ => 3.0,
            };
            match ncomp as i64 {
                1 => ColorSpace::Gray,
                4 => ColorSpace::Cmyk,
                _ => ColorSpace::Rgb,
            }
        } else if family == n.indexed {
            let (Some(base), Some(hival), Some(lookup)) = (elem(1), elem(2), elem(3)) else {
                return err("rangecheck", "Indexed colour space");
            };
            let base = self.parse_color_space(&base, depth + 1)?;
            let hival = hival.as_f64().unwrap_or(0.0).clamp(0.0, 4095.0) as u32;
            let nb = base.ncomp();
            let table = match lookup {
                Value::String(s) | Value::ExecString(s) => s.to_vec(),
                Value::Proc(p) => {
                    let mut table = Vec::with_capacity((hival as usize + 1) * nb);
                    for idx in 0..=hival {
                        self.push(Value::Int(i64::from(idx)))?;
                        self.call(Value::Proc(p.clone()))?;
                        let mut comps = Vec::with_capacity(nb);
                        for _ in 0..nb {
                            comps.push(self.pop()?.as_f64().unwrap_or(0.0));
                        }
                        table.extend(comps.iter().rev().map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8));
                    }
                    table
                }
                _ => return err("typecheck", "Indexed lookup table"),
            };
            ColorSpace::Indexed { base: Box::new(base), hival, lookup: Rc::new(table) }
        } else if family == n.separation {
            let (Some(alt), Some(tint)) = (elem(2), elem(3)) else {
                return err("rangecheck", "Separation colour space");
            };
            ColorSpace::Separation { alt: Box::new(self.parse_color_space(&alt, depth + 1)?), tint }
        } else if family == n.device_n {
            let (Some(Value::Array(names) | Value::Proc(names)), Some(alt), Some(tint)) = (elem(1), elem(2), elem(3)) else {
                return err("rangecheck", "DeviceN colour space");
            };
            ColorSpace::DeviceN { n: names.len as usize, alt: Box::new(self.parse_color_space(&alt, depth + 1)?), tint }
        } else {
            return err("undefined", format!("colour space /{}", self.name_string(family)));
        })
    }

    pub(crate) fn resolve_color(&mut self, space: &ColorSpace, comps: &[f64], depth: u32) -> Res<Option<DevColor>> {
        let c = |k: usize| comps.get(k).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        Ok(Some(match space {
            ColorSpace::Gray => DevColor::Gray(c(0)),
            ColorSpace::Rgb => DevColor::Rgb(c(0), c(1), c(2)),
            ColorSpace::Cmyk => DevColor::Cmyk(c(0), c(1), c(2), c(3)),
            ColorSpace::Pattern => return Ok(None),
            ColorSpace::Indexed { base, hival, lookup } => {
                let idx = comps.first().copied().unwrap_or(0.0).round().clamp(0.0, f64::from(*hival)) as usize;
                let nb = base.ncomp();
                let vals: Vec<f64> = (0..nb)
                    .map(|k| f64::from(lookup.get(idx * nb + k).copied().unwrap_or(0)) / 255.0)
                    .collect();
                return self.resolve_color(base, &vals, depth + 1);
            }
            ColorSpace::Separation { alt, tint } | ColorSpace::DeviceN { alt, tint, .. } => {
                if depth > 8 {
                    return err("limitcheck", "colour space nested too deeply");
                }
                for &v in comps {
                    self.push(crate::types::Value::Real(v))?;
                }
                self.call(tint.clone())?;
                let na = alt.ncomp();
                self.need(na)?;
                let mut vals = Vec::with_capacity(na);
                for k in (0..na).rev() {
                    vals.push(self.f64_at(k)?);
                }
                self.pop_n(na);
                return self.resolve_color(alt, &vals, depth + 1);
            }
        }))
    }

    fn set_color(&mut self, space: ColorSpace, comps: Vec<f64>) -> Res {
        let device = self.resolve_color(&space, &comps, 0)?;
        self.gs.space = space;
        self.gs.comps = comps;
        self.gs.device = device;
        Ok(())
    }

    fn device_color(&self) -> DevColor {
        self.gs.device.unwrap_or(DevColor::Gray(0.0))
    }
}

fn put_dash(out: &mut Vec<u8>, dash: &[f64], offset: f64) {
    out.push(b'[');
    for (k, &d) in dash.iter().enumerate() {
        if k > 0 {
            out.push(b' ');
        }
        put_num(out, d);
    }
    out.extend_from_slice(b"] ");
    put_nums(out, &[offset], b"d");
}

fn gray_of(c: DevColor) -> f64 {
    match c {
        DevColor::Gray(g) => g,
        DevColor::Rgb(r, g, b) => 0.3 * r + 0.59 * g + 0.11 * b,
        DevColor::Cmyk(c, m, y, k) => 1.0 - (0.3 * c + 0.59 * m + 0.11 * y + k).min(1.0),
    }
}

fn rgb_of(c: DevColor) -> [f64; 3] {
    match c {
        DevColor::Gray(g) => [g; 3],
        DevColor::Rgb(r, g, b) => [r, g, b],
        DevColor::Cmyk(c, m, y, k) => [1.0 - (c + k).min(1.0), 1.0 - (m + k).min(1.0), 1.0 - (y + k).min(1.0)],
    }
}

fn cmyk_of(c: DevColor) -> [f64; 4] {
    match c {
        DevColor::Cmyk(c, m, y, k) => [c, m, y, k],
        DevColor::Gray(g) => [0.0, 0.0, 0.0, 1.0 - g],
        DevColor::Rgb(r, g, b) => {
            let (c, m, y) = (1.0 - r, 1.0 - g, 1.0 - b);
            let k = c.min(m).min(y);
            [c - k, m - k, y - k, k]
        }
    }
}

fn hsb_to_rgb(h: f64, s: f64, v: f64) -> [f64; 3] {
    let h = (h.clamp(0.0, 1.0) * 6.0) % 6.0;
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as u8 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

fn rgb_to_hsb([r, g, b]: [f64; 3]) -> [f64; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let s = if max > 0.0 { delta / max } else { 0.0 };
    let h = if delta == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    [h, s, max]
}

// ---- graphics state stack -------------------------------------------------------

pub(crate) fn gsave(i: &mut Interp, by_save: bool) -> Res {
    if i.gstack.len() >= MAX_GSTACK {
        return err("limitcheck", "too many nested gsave levels");
    }
    let emitted = i.emitting();
    i.gstack.push(Saved { gs: i.gs.clone(), pdf: i.out.pdf.clone(), by_save, clip_only: false, emitted });
    if emitted {
        i.out.content.extend_from_slice(b"q\n");
    }
    Ok(())
}

pub(crate) fn grestore(i: &mut Interp) {
    let Some(top) = i.gstack.last() else { return };
    if top.by_save {
        i.gs = top.gs.clone();
        i.out.pdf = top.pdf.clone();
        if top.emitted {
            i.out.content.extend_from_slice(b"Q\nq\n");
        }
    } else {
        let saved = i.gstack.pop().unwrap();
        i.gs = saved.gs;
        i.out.pdf = saved.pdf;
        if saved.emitted {
            i.out.content.extend_from_slice(b"Q\n");
        }
    }
}

/// `restore`: unwinds the graphics state stack to the entry pushed by `save`.
pub(crate) fn restore_to(i: &mut Interp, level: usize) {
    while i.gstack.len() > level {
        let saved = i.gstack.pop().unwrap();
        if saved.emitted {
            i.out.content.extend_from_slice(b"Q\n");
        }
        i.gs = saved.gs;
        i.out.pdf = saved.pdf;
    }
}

/// Closes every open `q` at the end of the program.
pub(crate) fn finish(i: &mut Interp) {
    restore_to(i, 0);
}

// ---- operators ---------------------------------------------------------------------

macro_rules! ops {
    ($($name:literal => $f:expr,)*) => {
        &[$(($name, $f as OpFn),)*]
    };
}

fn pop_n_op(i: &mut Interp, n: usize) -> Res {
    i.need(n)?;
    i.pop_n(n);
    Ok(())
}

fn transform_op(i: &mut Interp, f: fn(&Matrix, f64, f64) -> Option<(f64, f64)>) -> Res {
    i.need(1)?;
    let (m, used) = match i.arg(0) {
        Value::Array(_) | Value::Proc(_) => (i.matrix_at(0)?, 1),
        _ => (i.gs.ctm, 0),
    };
    let y = i.f64_at(used)?;
    let x = i.f64_at(used + 1)?;
    let Some((rx, ry)) = f(&m, x, y) else {
        return err("undefinedresult", "singular matrix");
    };
    i.pop_n(used + 2);
    i.push(Value::Real(rx))?;
    i.push(Value::Real(ry))
}

/// `translate`/`scale`/`rotate`/`concat`: with a trailing matrix operand the
/// result is stored there instead of modifying the CTM.
fn ctm_op(i: &mut Interp, nargs: usize, make: fn(&[f64]) -> Matrix) -> Res {
    i.need(1)?;
    let target = match i.arg(0) {
        Value::Array(a) | Value::Proc(a) if nargs > 0 => Some(a.clone()),
        _ => None,
    };
    let base = usize::from(target.is_some());
    let mut args = Vec::with_capacity(nargs);
    for k in (0..nargs).rev() {
        args.push(i.f64_at(base + k)?);
    }
    let m = make(&args);
    i.pop_n(base + nargs);
    match target {
        Some(arr) => {
            let v = i.store_matrix(&arr, m)?;
            i.push(v)
        }
        None => {
            i.gs.ctm = m.then(&i.gs.ctm);
            Ok(())
        }
    }
}

fn rect_args(i: &mut Interp) -> Res<(Vec<[f64; 4]>, usize)> {
    i.need(1)?;
    if let Value::Array(a) | Value::Proc(a) = i.arg(0) {
        let nums = i.numbers(a)?;
        return Ok((nums.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect(), 1));
    }
    let h = i.f64_at(0)?;
    let w = i.f64_at(1)?;
    let y = i.f64_at(2)?;
    let x = i.f64_at(3)?;
    Ok((vec![[x, y, w, h]], 4))
}

fn rect_path(i: &mut Interp, rects: &[[f64; 4]]) -> Res<Vec<Seg>> {
    let m = i.gs.ctm;
    let mut path = Vec::with_capacity(rects.len() * 5);
    for &[x, y, w, h] in rects {
        let p = [m.apply(x, y), m.apply(x + w, y), m.apply(x + w, y + h), m.apply(x, y + h)];
        path.push(Seg::Move(p[0].0, p[0].1));
        for q in &p[1..] {
            path.push(Seg::Line(q.0, q.1));
        }
        path.push(Seg::Close);
    }
    i.alloc(path.len() * std::mem::size_of::<Seg>())?;
    Ok(path)
}

fn with_path(i: &mut Interp, path: Vec<Seg>, paint: fn(&mut Interp) -> Res) -> Res {
    let saved = (std::mem::replace(&mut i.gs.path, path), i.gs.cp, i.gs.start);
    let result = paint(i);
    (i.gs.path, i.gs.cp, i.gs.start) = saved;
    result
}

fn set_device(i: &mut Interp, space: ColorSpace, n: usize) -> Res {
    let mut comps = Vec::with_capacity(n);
    for k in (0..n).rev() {
        comps.push(i.f64_at(k)?);
    }
    i.pop_n(n);
    i.set_color(space, comps)
}

fn push_reals(i: &mut Interp, vals: &[f64]) -> Res {
    for &v in vals {
        i.push(Value::Real(v))?;
    }
    Ok(())
}

pub(crate) static OPERATORS: &[(&str, OpFn)] = ops! {
    "gsave" => |i: &mut Interp| gsave(i, false),
    "grestore" => |i: &mut Interp| { grestore(i); Ok(()) },
    "grestoreall" => |i: &mut Interp| {
        while i.gstack.last().is_some_and(|s| !s.by_save) { grestore(i); }
        grestore(i);
        Ok(())
    },
    "initgraphics" => |i: &mut Interp| {
        let font = i.gs.font.take();
        i.gs = GState { font, ..GState::default() };
        Ok(())
    },
    "clipsave" => |i: &mut Interp| {
        gsave(i, false)?;
        i.gstack.last_mut().unwrap().clip_only = true;
        Ok(())
    },
    "nulldevice" => |i: &mut Interp| { i.gs.null_device = true; Ok(()) },
    "gstate" => |i: &mut Interp| {
        i.alloc(1024)?;
        let id = i.gstates.len() as u32;
        let gs = i.gs.clone();
        i.gstates.push(gs);
        i.push(Value::GState(id))
    },
    "currentgstate" => |i: &mut Interp| {
        i.need(1)?;
        let Value::GState(id) = *i.arg(0) else { return err("typecheck", "currentgstate"); };
        i.gstates[id as usize] = i.gs.clone();
        Ok(())
    },
    "setgstate" => |i: &mut Interp| {
        i.need(1)?;
        let Value::GState(id) = *i.arg(0) else { return err("typecheck", "setgstate"); };
        i.pop_n(1);
        i.gs = i.gstates[id as usize].clone();
        Ok(())
    },
    "currentcolortransfer" => |i: &mut Interp| {
        for _ in 0..4 {
            let p = i.new_array(Vec::new())?;
            i.push(Value::Proc(p))?;
        }
        Ok(())
    },
    "currentcolorscreen" => |i: &mut Interp| {
        for _ in 0..4 {
            i.push(Value::Real(60.0))?;
            i.push(Value::Real(45.0))?;
            let p = i.new_array(Vec::new())?;
            i.push(Value::Proc(p))?;
        }
        Ok(())
    },
    "cliprestore" => |i: &mut Interp| {
        if i.gstack.last().is_some_and(|s| s.clip_only) {
            let current = i.gs.clone();
            grestore(i);
            i.gs = current;
        }
        Ok(())
    },
    "setlinewidth" => |i: &mut Interp| { let w = i.f64_at(0)?; i.pop_n(1); i.gs.lw = w.abs(); Ok(()) },
    "currentlinewidth" => |i: &mut Interp| { let w = i.gs.lw; i.push(Value::Real(w)) },
    "setlinecap" => |i: &mut Interp| {
        let c = i.int_at(0)?;
        if !(0..=2).contains(&c) { return err("rangecheck", "setlinecap"); }
        i.pop_n(1);
        i.gs.cap = c as u8;
        Ok(())
    },
    "currentlinecap" => |i: &mut Interp| { let c = i.gs.cap; i.push(Value::Int(i64::from(c))) },
    "setlinejoin" => |i: &mut Interp| {
        let j = i.int_at(0)?;
        if !(0..=2).contains(&j) { return err("rangecheck", "setlinejoin"); }
        i.pop_n(1);
        i.gs.join = j as u8;
        Ok(())
    },
    "currentlinejoin" => |i: &mut Interp| { let j = i.gs.join; i.push(Value::Int(i64::from(j))) },
    "setmiterlimit" => |i: &mut Interp| {
        let m = i.f64_at(0)?;
        if m < 1.0 { return err("rangecheck", "setmiterlimit"); }
        i.pop_n(1);
        i.gs.miter = m;
        Ok(())
    },
    "currentmiterlimit" => |i: &mut Interp| { let m = i.gs.miter; i.push(Value::Real(m)) },
    "setdash" => |i: &mut Interp| {
        let offset = i.f64_at(0)?;
        let pattern = i.array_at(1)?;
        let dash = i.numbers(&pattern)?;
        if dash.iter().any(|d| *d < 0.0) || (!dash.is_empty() && dash.iter().all(|d| *d == 0.0)) {
            return err("rangecheck", "setdash");
        }
        i.pop_n(2);
        i.gs.dash = Rc::new(dash);
        i.gs.dash_offset = offset;
        Ok(())
    },
    "currentdash" => |i: &mut Interp| {
        let items = i.gs.dash.iter().map(|d| Value::Real(*d)).collect();
        let arr = i.new_array(items)?;
        let off = i.gs.dash_offset;
        i.push(Value::Array(arr))?;
        i.push(Value::Real(off))
    },
    "setflat" => |i: &mut Interp| { let f = i.f64_at(0)?; i.pop_n(1); i.gs.flat = f; Ok(()) },
    "currentflat" => |i: &mut Interp| { let f = i.gs.flat; i.push(Value::Real(f)) },
    "setstrokeadjust" => |i: &mut Interp| { let b = i.bool_at(0)?; i.pop_n(1); i.gs.stroke_adjust = b; Ok(()) },
    "currentstrokeadjust" => |i: &mut Interp| { let b = i.gs.stroke_adjust; i.push(Value::Bool(b)) },
    "setoverprint" => |i: &mut Interp| { let b = i.bool_at(0)?; i.pop_n(1); i.gs.overprint = b; Ok(()) },
    "currentoverprint" => |i: &mut Interp| { let b = i.gs.overprint; i.push(Value::Bool(b)) },
    "setsmoothness" => |i: &mut Interp| pop_n_op(i, 1),
    "currentsmoothness" => |i: &mut Interp| i.push(Value::Real(0.02)),
    "settransfer" => |i: &mut Interp| pop_n_op(i, 1),
    "currenttransfer" => |i: &mut Interp| { let p = i.new_array(Vec::new())?; i.push(Value::Proc(p)) },
    "setcolortransfer" => |i: &mut Interp| pop_n_op(i, 4),
    "setblackgeneration" => |i: &mut Interp| pop_n_op(i, 1),
    "setundercolorremoval" => |i: &mut Interp| pop_n_op(i, 1),
    "currentblackgeneration" => |i: &mut Interp| { let p = i.new_array(Vec::new())?; i.push(Value::Proc(p)) },
    "currentundercolorremoval" => |i: &mut Interp| { let p = i.new_array(Vec::new())?; i.push(Value::Proc(p)) },
    "setscreen" => |i: &mut Interp| pop_n_op(i, 3),
    "currentscreen" => |i: &mut Interp| {
        i.push(Value::Real(60.0))?;
        i.push(Value::Real(45.0))?;
        let p = i.new_array(Vec::new())?;
        i.push(Value::Proc(p))
    },
    "setcolorscreen" => |i: &mut Interp| pop_n_op(i, 12),
    "sethalftone" => |i: &mut Interp| pop_n_op(i, 1),
    "currenthalftone" => |i: &mut Interp| {
        let d = crate::types::PsDict::new();
        let spot = i.new_array(Vec::new())?;
        let entries: [(&[u8], Value); 4] = [
            (b"HalftoneType", Value::Int(1)),
            (b"Frequency", Value::Real(60.0)),
            (b"Angle", Value::Real(45.0)),
            (b"SpotFunction", Value::Proc(spot)),
        ];
        for (k, v) in entries {
            let key = i.key_bytes(k);
            d.put(key, v);
        }
        i.push(Value::Dict(d))
    },
    "setcolorrendering" => |i: &mut Interp| pop_n_op(i, 1),
    "currentcolorrendering" => |i: &mut Interp| i.push(Value::Dict(crate::types::PsDict::new())),
    "findcolorrendering" => |i: &mut Interp| {
        i.need(1)?;
        i.pop_n(1);
        let name = i.names.intern(b"DefaultColorRendering");
        i.push(Value::Name(name))?;
        i.push(Value::Bool(false))
    },
    "setgray" => |i: &mut Interp| set_device(i, ColorSpace::Gray, 1),
    "setrgbcolor" => |i: &mut Interp| set_device(i, ColorSpace::Rgb, 3),
    "setcmykcolor" => |i: &mut Interp| set_device(i, ColorSpace::Cmyk, 4),
    "sethsbcolor" => |i: &mut Interp| {
        let b = i.f64_at(0)?;
        let s = i.f64_at(1)?;
        let h = i.f64_at(2)?;
        i.pop_n(3);
        i.set_color(ColorSpace::Rgb, hsb_to_rgb(h, s, b).to_vec())
    },
    "currentgray" => |i: &mut Interp| { let g = gray_of(i.device_color()); i.push(Value::Real(g)) },
    "currentrgbcolor" => |i: &mut Interp| { let c = rgb_of(i.device_color()); push_reals(i, &c) },
    "currentcmykcolor" => |i: &mut Interp| { let c = cmyk_of(i.device_color()); push_reals(i, &c) },
    "currenthsbcolor" => |i: &mut Interp| { let c = rgb_to_hsb(rgb_of(i.device_color())); push_reals(i, &c) },
    "setcolorspace" => |i: &mut Interp| {
        i.need(1)?;
        let v = i.arg(0).clone();
        let space = i.parse_color_space(&v, 0)?;
        i.pop_n(1);
        let comps = space.initial();
        i.set_color(space, comps)
    },
    "currentcolorspace" => |i: &mut Interp| {
        let name = match i.gs.space {
            ColorSpace::Gray => i.n.device_gray,
            ColorSpace::Cmyk => i.n.device_cmyk,
            ColorSpace::Pattern => i.n.pattern,
            _ => i.n.device_rgb,
        };
        let arr = i.new_array(vec![Value::Name(name)])?;
        i.push(Value::Array(arr))
    },
    "setcolor" => |i: &mut Interp| {
        let space = i.gs.space.clone();
        if let ColorSpace::Pattern = space {
            i.need(1)?;
            let n = i.ostack.iter().rev().position(|v| matches!(v, Value::Dict(_))).map_or(1, |p| p + 1);
            i.need(n)?;
            i.pop_n(n);
            i.gs.device = None;
            return Ok(());
        }
        set_device(i, space.clone(), space.ncomp())
    },
    "currentcolor" => |i: &mut Interp| { let c = i.gs.comps.clone(); push_reals(i, &c) },
    "setpattern" => |i: &mut Interp| {
        i.need(1)?;
        i.pop_n(1);
        i.gs.space = ColorSpace::Pattern;
        i.gs.comps.clear();
        i.gs.device = None;
        Ok(())
    },
    "makepattern" => |i: &mut Interp| {
        let d = i.dict_at(1)?;
        i.matrix_at(0)?;
        i.pop_n(2);
        i.push(Value::Dict(d))
    },
    "shfill" => |i: &mut Interp| pop_n_op(i, 1),
    // matrices
    "matrix" => |i: &mut Interp| { let v = i.matrix_value(Matrix::IDENTITY)?; i.push(v) },
    "initmatrix" => |i: &mut Interp| { i.gs.ctm = Matrix::IDENTITY; Ok(()) },
    "identmatrix" => |i: &mut Interp| {
        let arr = i.array_at(0)?;
        i.pop_n(1);
        let v = i.store_matrix(&arr, Matrix::IDENTITY)?;
        i.push(v)
    },
    "defaultmatrix" => |i: &mut Interp| {
        let arr = i.array_at(0)?;
        i.pop_n(1);
        let v = i.store_matrix(&arr, Matrix::IDENTITY)?;
        i.push(v)
    },
    "currentmatrix" => |i: &mut Interp| {
        let arr = i.array_at(0)?;
        i.pop_n(1);
        let ctm = i.gs.ctm;
        let v = i.store_matrix(&arr, ctm)?;
        i.push(v)
    },
    "setmatrix" => |i: &mut Interp| { let m = i.matrix_at(0)?; i.pop_n(1); i.gs.ctm = m; Ok(()) },
    "translate" => |i: &mut Interp| ctm_op(i, 2, |a| Matrix::translate(a[0], a[1])),
    "scale" => |i: &mut Interp| ctm_op(i, 2, |a| Matrix::scale(a[0], a[1])),
    "rotate" => |i: &mut Interp| ctm_op(i, 1, |a| Matrix::rotate(a[0])),
    "concat" => |i: &mut Interp| { let m = i.matrix_at(0)?; i.pop_n(1); i.gs.ctm = m.then(&i.gs.ctm); Ok(()) },
    "concatmatrix" => |i: &mut Interp| {
        let target = i.array_at(0)?;
        let m2 = i.matrix_at(1)?;
        let m1 = i.matrix_at(2)?;
        i.pop_n(3);
        let v = i.store_matrix(&target, m1.then(&m2))?;
        i.push(v)
    },
    "invertmatrix" => |i: &mut Interp| {
        let target = i.array_at(0)?;
        let m = i.matrix_at(1)?;
        let Some(inv) = m.invert() else { return err("undefinedresult", "singular matrix"); };
        i.pop_n(2);
        let v = i.store_matrix(&target, inv)?;
        i.push(v)
    },
    "transform" => |i: &mut Interp| transform_op(i, |m, x, y| Some(m.apply(x, y))),
    "dtransform" => |i: &mut Interp| transform_op(i, |m, x, y| Some(m.apply_delta(x, y))),
    "itransform" => |i: &mut Interp| transform_op(i, |m, x, y| m.invert().map(|inv| inv.apply(x, y))),
    "idtransform" => |i: &mut Interp| transform_op(i, |m, x, y| m.invert().map(|inv| inv.apply_delta(x, y))),
    // paths
    "newpath" => |i: &mut Interp| { i.new_path(); Ok(()) },
    "currentpoint" => |i: &mut Interp| { let (x, y) = i.current_user_point()?; push_reals(i, &[x, y]) },
    "moveto" => |i: &mut Interp| {
        let y = i.f64_at(0)?;
        let x = i.f64_at(1)?;
        i.pop_n(2);
        let (dx, dy) = i.gs.ctm.apply(x, y);
        i.move_to_device(dx, dy)
    },
    "rmoveto" => |i: &mut Interp| {
        let dy = i.f64_at(0)?;
        let dx = i.f64_at(1)?;
        let (cx, cy) = i.gs.cp.map_or_else(|| err("nocurrentpoint", "rmoveto"), Ok)?;
        i.pop_n(2);
        let (tx, ty) = i.gs.ctm.apply_delta(dx, dy);
        i.move_to_device(cx + tx, cy + ty)
    },
    "lineto" => |i: &mut Interp| {
        let y = i.f64_at(0)?;
        let x = i.f64_at(1)?;
        if i.gs.cp.is_none() { return err("nocurrentpoint", "lineto"); }
        i.pop_n(2);
        let (dx, dy) = i.gs.ctm.apply(x, y);
        i.line_to_device(dx, dy)
    },
    "rlineto" => |i: &mut Interp| {
        let dy = i.f64_at(0)?;
        let dx = i.f64_at(1)?;
        let (cx, cy) = i.gs.cp.map_or_else(|| err("nocurrentpoint", "rlineto"), Ok)?;
        i.pop_n(2);
        let (tx, ty) = i.gs.ctm.apply_delta(dx, dy);
        i.line_to_device(cx + tx, cy + ty)
    },
    "curveto" => |i: &mut Interp| {
        let mut a = [0.0; 6];
        for k in 0..6 { a[5 - k] = i.f64_at(k)?; }
        if i.gs.cp.is_none() { return err("nocurrentpoint", "curveto"); }
        i.pop_n(6);
        let m = i.gs.ctm;
        let (p1, p2, p3) = (m.apply(a[0], a[1]), m.apply(a[2], a[3]), m.apply(a[4], a[5]));
        i.curve_to_device([p1.0, p1.1, p2.0, p2.1, p3.0, p3.1])
    },
    "rcurveto" => |i: &mut Interp| {
        let mut a = [0.0; 6];
        for k in 0..6 { a[5 - k] = i.f64_at(k)?; }
        let (cx, cy) = i.gs.cp.map_or_else(|| err("nocurrentpoint", "rcurveto"), Ok)?;
        i.pop_n(6);
        let m = i.gs.ctm;
        let d = |x: f64, y: f64| { let (tx, ty) = m.apply_delta(x, y); (cx + tx, cy + ty) };
        let (p1, p2, p3) = (d(a[0], a[1]), d(a[2], a[3]), d(a[4], a[5]));
        i.curve_to_device([p1.0, p1.1, p2.0, p2.1, p3.0, p3.1])
    },
    "arc" => |i: &mut Interp| {
        let a2 = i.f64_at(0)?; let a1 = i.f64_at(1)?; let r = i.f64_at(2)?; let y = i.f64_at(3)?; let x = i.f64_at(4)?;
        i.pop_n(5);
        i.arc(x, y, r, a1, a2, false)
    },
    "arcn" => |i: &mut Interp| {
        let a2 = i.f64_at(0)?; let a1 = i.f64_at(1)?; let r = i.f64_at(2)?; let y = i.f64_at(3)?; let x = i.f64_at(4)?;
        i.pop_n(5);
        i.arc(x, y, r, a1, a2, true)
    },
    "arct" => |i: &mut Interp| i.arc_tangent().map(|_| ()),
    "arcto" => |i: &mut Interp| { let t = i.arc_tangent()?; push_reals(i, &t) },
    "closepath" => |i: &mut Interp| i.close_path(),
    "flattenpath" => |_: &mut Interp| Ok(()),
    "reversepath" => |_: &mut Interp| Ok(()),
    "strokepath" => |_: &mut Interp| Ok(()),
    "initclip" => |_: &mut Interp| Ok(()),
    "clippath" => |i: &mut Interp| {
        let b = i.bbox;
        i.new_path();
        i.move_to_device(b.llx, b.lly)?;
        i.line_to_device(b.urx, b.lly)?;
        i.line_to_device(b.urx, b.ury)?;
        i.line_to_device(b.llx, b.ury)?;
        i.close_path()
    },
    "pathbbox" => |i: &mut Interp| {
        if i.gs.path.is_empty() { return err("nocurrentpoint", "pathbbox"); }
        let inv = i.gs.ctm.invert().map_or_else(|| err("undefinedresult", "pathbbox"), Ok)?;
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut add = |x: f64, y: f64| {
            let (ux, uy) = inv.apply(x, y);
            x0 = x0.min(ux); y0 = y0.min(uy); x1 = x1.max(ux); y1 = y1.max(uy);
        };
        for seg in &i.gs.path {
            match *seg {
                Seg::Move(x, y) | Seg::Line(x, y) => add(x, y),
                Seg::Curve(c) => { add(c[0], c[1]); add(c[2], c[3]); add(c[4], c[5]); }
                Seg::Close => {}
            }
        }
        push_reals(i, &[x0, y0, x1, y1])
    },
    "pathforall" => |i: &mut Interp| {
        let procs = [i.proc_at(3)?, i.proc_at(2)?, i.proc_at(1)?, i.proc_at(0)?];
        let inv = i.gs.ctm.invert().map_or_else(|| err("undefinedresult", "pathforall"), Ok)?;
        i.pop_n(4);
        for seg in i.gs.path.clone() {
            let (k, pts): (usize, Vec<(f64, f64)>) = match seg {
                Seg::Move(x, y) => (0, vec![(x, y)]),
                Seg::Line(x, y) => (1, vec![(x, y)]),
                Seg::Curve(c) => (2, vec![(c[0], c[1]), (c[2], c[3]), (c[4], c[5])]),
                Seg::Close => (3, Vec::new()),
            };
            for (x, y) in pts {
                let (ux, uy) = inv.apply(x, y);
                push_reals(i, &[ux, uy])?;
            }
            i.call(Value::Proc(procs[k].clone()))?;
        }
        Ok(())
    },
    "setbbox" => |i: &mut Interp| pop_n_op(i, 4),
    "ucache" => |_: &mut Interp| Ok(()),
    // painting
    "fill" => |i: &mut Interp| i.fill_path(false),
    "eofill" => |i: &mut Interp| i.fill_path(true),
    "stroke" => |i: &mut Interp| i.stroke_path(),
    "clip" => |i: &mut Interp| i.clip_path(false),
    "eoclip" => |i: &mut Interp| i.clip_path(true),
    "rectfill" => |i: &mut Interp| {
        let (rects, n) = rect_args(i)?;
        let path = rect_path(i, &rects)?;
        i.pop_n(n);
        with_path(i, path, |i| i.fill_path(false))
    },
    "rectclip" => |i: &mut Interp| {
        let (rects, n) = rect_args(i)?;
        let path = rect_path(i, &rects)?;
        i.pop_n(n);
        i.gs.path = path;
        i.clip_path(false)?;
        i.new_path();
        Ok(())
    },
    "rectstroke" => |i: &mut Interp| {
        i.need(1)?;
        let extra = match i.arg(0) {
            Value::Array(a) | Value::Proc(a) if a.len == 6 && i.ostack.len() >= 2
                && matches!(i.arg(1), Value::Array(_) | Value::Proc(_) | Value::Int(_) | Value::Real(_))
                && !(matches!(i.arg(1), Value::Int(_) | Value::Real(_)) && i.ostack.len() < 5) => Some(i.matrix_at(0)?),
            _ => None,
        };
        if extra.is_some() { let m = i.pop()?; drop(m); }
        let (rects, n) = rect_args(i)?;
        let path = rect_path(i, &rects)?;
        i.pop_n(n);
        let saved_ctm = i.gs.ctm;
        if let Some(m) = extra { i.gs.ctm = m.then(&saved_ctm); }
        let result = with_path(i, path, |i| i.stroke_path());
        i.gs.ctm = saved_ctm;
        result
    },
    "erasepage" => |_: &mut Interp| Ok(()),
    "showpage" => |_: &mut Interp| Ok(()),
    "copypage" => |_: &mut Interp| Ok(()),
};

pub(crate) fn extend_path(i: &mut Interp, segs: Vec<Seg>, cp: Option<(f64, f64)>) -> Res {
    i.alloc(segs.len() * std::mem::size_of::<Seg>())?;
    i.gs.path.extend(segs);
    if cp.is_some() {
        i.gs.cp = cp;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_written_compactly() {
        let mut out = Vec::new();
        for v in [0.0, -0.0, 1.0, -2.5, 0.12345, 100.00004, -0.00004, 12345678.9] {
            put_num(&mut out, v);
            out.push(b' ');
        }
        assert_eq!(String::from_utf8(out).unwrap(), "0 0 1 -2.5 0.1235 100 0 12345678.9 ");
    }
}
