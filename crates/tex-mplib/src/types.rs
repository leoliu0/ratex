//! Core data structures and types for MetaPost and mplib.

use std::fmt::Write;

/// A 2D point or vector in PostScript points (bp, 1/72 inch).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pair {
    pub x: f64,
    pub y: f64,
}

impl Pair {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }

    #[inline]
    pub fn angle_rad(self) -> f64 {
        self.y.atan2(self.x)
    }

    #[inline]
    pub fn angle_deg(self) -> f64 {
        self.angle_rad().to_degrees()
    }

    #[inline]
    pub fn from_polar(r: f64, theta_deg: f64) -> Self {
        let rad = theta_deg.to_radians();
        Self {
            x: r * rad.cos(),
            y: r * rad.sin(),
        }
    }

    #[inline]
    pub fn normalized(self) -> Self {
        let l = self.len();
        if l > 1e-12 {
            Self {
                x: self.x / l,
                y: self.y / l,
            }
        } else {
            Self::ZERO
        }
    }

    #[inline]
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y
    }
}

impl std::ops::Add for Pair {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl std::ops::Sub for Pair {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl std::ops::Mul<f64> for Pair {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f64) -> Self {
        Self {
            x: self.x * rhs,
            y: self.y * rhs,
        }
    }
}

impl std::ops::Mul<Pair> for f64 {
    type Output = Pair;
    #[inline]
    fn mul(self, rhs: Pair) -> Pair {
        Pair {
            x: self * rhs.x,
            y: self * rhs.y,
        }
    }
}

impl std::ops::Div<f64> for Pair {
    type Output = Self;
    #[inline]
    fn div(self, rhs: f64) -> Self {
        Self {
            x: self.x / rhs,
            y: self.y / rhs,
        }
    }
}

impl std::ops::Neg for Pair {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
        }
    }
}

/// Color representation in MetaPost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Color {
    None,
    Gray(f64),
    Rgb(f64, f64, f64),
    Cmyk(f64, f64, f64, f64),
}

impl Color {
    pub const BLACK: Self = Self::Gray(0.0);
    pub const WHITE: Self = Self::Gray(1.0);
    pub const RED: Self = Self::Rgb(1.0, 0.0, 0.0);
    pub const GREEN: Self = Self::Rgb(0.0, 1.0, 0.0);
    pub const BLUE: Self = Self::Rgb(0.0, 0.0, 1.0);

    pub fn to_postscript(self) -> String {
        match self {
            Self::None => String::new(),
            Self::Gray(g) => format!("{g:.4} setgray"),
            Self::Rgb(r, g, b) => format!("{r:.4} {g:.4} {b:.4} setrgbcolor"),
            Self::Cmyk(c, m, y, k) => format!("{c:.4} {m:.4} {y:.4} {k:.4} setcmykcolor"),
        }
    }

    pub fn to_svg_str(self) -> String {
        match self {
            Self::None => "none".to_string(),
            Self::Gray(g) => {
                let v = (g.clamp(0.0, 1.0) * 255.0).round() as u8;
                format!("rgb({v},{v},{v})")
            }
            Self::Rgb(r, g, b) => {
                let r8 = (r.clamp(0.0, 1.0) * 255.0).round() as u8;
                let g8 = (g.clamp(0.0, 1.0) * 255.0).round() as u8;
                let b8 = (b.clamp(0.0, 1.0) * 255.0).round() as u8;
                format!("rgb({r8},{g8},{b8})")
            }
            Self::Cmyk(c, m, y, k) => {
                let r = (1.0 - c) * (1.0 - k);
                let g = (1.0 - m) * (1.0 - k);
                let b = (1.0 - y) * (1.0 - k);
                let r8 = (r.clamp(0.0, 1.0) * 255.0).round() as u8;
                let g8 = (g.clamp(0.0, 1.0) * 255.0).round() as u8;
                let b8 = (b.clamp(0.0, 1.0) * 255.0).round() as u8;
                format!("rgb({r8},{g8},{b8})")
            }
        }
    }
}

/// 2D Affine Transformation matrix:
/// [ xx  yx  0 ]
/// [ xy  yy  0 ]
/// [ x0  y0  1 ]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub xx: f64,
    pub yx: f64,
    pub xy: f64,
    pub yy: f64,
    pub x0: f64,
    pub y0: f64,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        xx: 1.0,
        yx: 0.0,
        xy: 0.0,
        yy: 1.0,
        x0: 0.0,
        y0: 0.0,
    };

    pub fn shifted(dx: f64, dy: f64) -> Self {
        Self {
            xx: 1.0,
            yx: 0.0,
            xy: 0.0,
            yy: 1.0,
            x0: dx,
            y0: dy,
        }
    }

    pub fn scaled(s: f64) -> Self {
        Self {
            xx: s,
            yx: 0.0,
            xy: 0.0,
            yy: s,
            x0: 0.0,
            y0: 0.0,
        }
    }

    pub fn xscaled(s: f64) -> Self {
        Self {
            xx: s,
            yx: 0.0,
            xy: 0.0,
            yy: 1.0,
            x0: 0.0,
            y0: 0.0,
        }
    }

    pub fn yscaled(s: f64) -> Self {
        Self {
            xx: 1.0,
            yx: 0.0,
            xy: 0.0,
            yy: s,
            x0: 0.0,
            y0: 0.0,
        }
    }

    pub fn rotated(deg: f64) -> Self {
        let rad = deg.to_radians();
        let c = rad.cos();
        let s = rad.sin();
        Self {
            xx: c,
            yx: s,
            xy: -s,
            yy: c,
            x0: 0.0,
            y0: 0.0,
        }
    }

    pub fn slanted(s: f64) -> Self {
        Self {
            xx: 1.0,
            yx: 0.0,
            xy: s,
            yy: 1.0,
            x0: 0.0,
            y0: 0.0,
        }
    }

    pub fn apply(&self, p: Pair) -> Pair {
        Pair {
            x: p.x * self.xx + p.y * self.xy + self.x0,
            y: p.x * self.yx + p.y * self.yy + self.y0,
        }
    }

    pub fn compose(&self, other: &Self) -> Self {
        Self {
            xx: self.xx * other.xx + self.yx * other.xy,
            yx: self.xx * other.yx + self.yx * other.yy,
            xy: self.xy * other.xx + self.yy * other.xy,
            yy: self.xy * other.yx + self.yy * other.yy,
            x0: self.x0 * other.xx + self.y0 * other.xy + other.x0,
            y0: self.x0 * other.yx + self.y0 * other.yy + other.y0,
        }
    }
}

/// A knot in a cubic Bézier path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Knot {
    pub p: Pair,
    pub right_control: Pair,
    pub left_control: Pair,
}

impl Knot {
    pub fn new(p: Pair) -> Self {
        Self {
            p,
            right_control: p,
            left_control: p,
        }
    }

    pub fn with_controls(left: Pair, p: Pair, right: Pair) -> Self {
        Self {
            p,
            right_control: right,
            left_control: left,
        }
    }
}

/// A MetaPost path composed of knots.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub knots: Vec<Knot>,
    pub closed: bool,
}

impl Path {
    pub fn empty() -> Self {
        Self {
            knots: Vec::new(),
            closed: false,
        }
    }

    pub fn line(start: Pair, end: Pair) -> Self {
        Self {
            knots: vec![Knot::new(start), Knot::new(end)],
            closed: false,
        }
    }

    pub fn rectangle(ll: Pair, ur: Pair) -> Self {
        let lr = Pair::new(ur.x, ll.y);
        let ul = Pair::new(ll.x, ur.y);
        Self {
            knots: vec![
                Knot::new(ll),
                Knot::new(lr),
                Knot::new(ur),
                Knot::new(ul),
            ],
            closed: true,
        }
    }

    pub fn circle(center: Pair, radius: f64) -> Self {
        // Standard 4-point Bézier approximation for circle: c = 4/3 * (sqrt(2)-1) ~ 0.55228475
        let k = 0.5522847498307935 * radius;
        let p0 = Pair::new(center.x + radius, center.y);
        let p1 = Pair::new(center.x, center.y + radius);
        let p2 = Pair::new(center.x - radius, center.y);
        let p3 = Pair::new(center.x, center.y - radius);

        Self {
            knots: vec![
                Knot::with_controls(
                    Pair::new(p0.x, p0.y - k),
                    p0,
                    Pair::new(p0.x, p0.y + k),
                ),
                Knot::with_controls(
                    Pair::new(p1.x + k, p1.y),
                    p1,
                    Pair::new(p1.x - k, p1.y),
                ),
                Knot::with_controls(
                    Pair::new(p2.x, p2.y + k),
                    p2,
                    Pair::new(p2.x, p2.y - k),
                ),
                Knot::with_controls(
                    Pair::new(p3.x - k, p3.y),
                    p3,
                    Pair::new(p3.x + k, p3.y),
                ),
            ],
            closed: true,
        }
    }

    pub fn transformed(&self, t: &Transform) -> Self {
        Self {
            knots: self
                .knots
                .iter()
                .map(|k| Knot {
                    p: t.apply(k.p),
                    left_control: t.apply(k.left_control),
                    right_control: t.apply(k.right_control),
                })
                .collect(),
            closed: self.closed,
        }
    }

    pub fn bounding_box(&self) -> (f64, f64, f64, f64) {
        if self.knots.is_empty() {
            return (0.0, 0.0, 0.0, 0.0);
        }

        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;

        let update_pt = |pt: Pair, min_x: &mut f64, min_y: &mut f64, max_x: &mut f64, max_y: &mut f64| {
            *min_x = min_x.min(pt.x);
            *min_y = min_y.min(pt.y);
            *max_x = max_x.max(pt.x);
            *max_y = max_y.max(pt.y);
        };

        let n = self.knots.len();
        for i in 0..n {
            update_pt(self.knots[i].p, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
            let next_idx = if i + 1 < n {
                i + 1
            } else if self.closed {
                0
            } else {
                continue;
            };

            let p0 = self.knots[i].p;
            let p1 = self.knots[i].right_control;
            let p2 = self.knots[next_idx].left_control;
            let p3 = self.knots[next_idx].p;

            // Check roots of derivative of cubic Bézier:
            // B(t) = (1-t)^3 p0 + 3(1-t)^2 t p1 + 3(1-t) t^2 p2 + t^3 p3
            // B'(t) = 3 [ (p1-p0) (1-t)^2 + 2(p2-p1)(1-t)t + (p3-p2)t^2 ]
            //       = 3 [ a t^2 + b t + c ] where
            //       a = p3 - 3*p2 + 3*p1 - p0
            //       b = 2*(p2 - 2*p1 + p0)
            //       c = p1 - p0
            for coord in 0..2 {
                let (c0, c1, c2, c3) = if coord == 0 {
                    (p0.x, p1.x, p2.x, p3.x)
                } else {
                    (p0.y, p1.y, p2.y, p3.y)
                };

                let a = c3 - 3.0 * c2 + 3.0 * c1 - c0;
                let b = 2.0 * (c2 - 2.0 * c1 + c0);
                let c = c1 - c0;

                if a.abs() < 1e-12 {
                    if b.abs() > 1e-12 {
                        let t = -c / b;
                        if (0.0..=1.0).contains(&t) {
                            let pt = eval_bezier(p0, p1, p2, p3, t);
                            update_pt(pt, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
                        }
                    }
                } else {
                    let discr = b * b - 4.0 * a * c;
                    if discr >= 0.0 {
                        let sqrt_d = discr.sqrt();
                        let t1 = (-b + sqrt_d) / (2.0 * a);
                        let t2 = (-b - sqrt_d) / (2.0 * a);
                        for t in [t1, t2] {
                            if (0.0..=1.0).contains(&t) {
                                let pt = eval_bezier(p0, p1, p2, p3, t);
                                update_pt(pt, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
                            }
                        }
                    }
                }
            }
        }

        (min_x, min_y, max_x, max_y)
    }
}

fn eval_bezier(p0: Pair, p1: Pair, p2: Pair, p3: Pair, t: f64) -> Pair {
    let mt = 1.0 - t;
    let mt2 = mt * mt;
    let mt3 = mt2 * mt;
    let t2 = t * t;
    let t3 = t2 * t;
    Pair {
        x: mt3 * p0.x + 3.0 * mt2 * t * p1.x + 3.0 * mt * t2 * p2.x + t3 * p3.x,
        y: mt3 * p0.y + 3.0 * mt2 * t * p1.y + 3.0 * mt * t2 * p2.y + t3 * p3.y,
    }
}

/// Dash specification: pattern and offset.
#[derive(Clone, Debug, PartialEq)]
pub struct Dash {
    pub pattern: Vec<f64>,
    pub offset: f64,
}

/// Drawing Pen in MetaPost.
#[derive(Clone, Debug, PartialEq)]
pub struct Pen {
    pub width: f64,
    pub height: f64,
    pub angle: f64,
}

impl Pen {
    pub fn default_pen() -> Self {
        Self {
            width: 0.5,
            height: 0.5,
            angle: 0.0,
        }
    }

    pub fn circle(diameter: f64) -> Self {
        Self {
            width: diameter,
            height: diameter,
            angle: 0.0,
        }
    }
}

/// A rendered MetaPost graphical object.
#[derive(Clone, Debug, PartialEq)]
pub enum MpObject {
    Fill {
        path: Path,
        color: Color,
    },
    Stroke {
        path: Path,
        color: Color,
        width: f64,
        dash: Option<Dash>,
        line_cap: u8,
        line_join: u8,
        miter_limit: f64,
    },
    Text {
        text: String,
        font: String,
        scale: f64,
        transform: Transform,
        color: Color,
    },
    StartClip {
        path: Path,
    },
    StopClip,
}

impl MpObject {
    pub fn bounding_box(&self) -> (f64, f64, f64, f64) {
        match self {
            Self::Fill { path, .. } => path.bounding_box(),
            Self::Stroke { path, width, .. } => {
                let (x0, y0, x1, y1) = path.bounding_box();
                let hw = width * 0.5;
                (x0 - hw, y0 - hw, x1 + hw, y1 + hw)
            }
            Self::Text { scale, transform, .. } => {
                let p0 = transform.apply(Pair::ZERO);
                let p1 = transform.apply(Pair::new(*scale * 10.0, *scale * 10.0));
                (
                    p0.x.min(p1.x),
                    p0.y.min(p1.y),
                    p0.x.max(p1.x),
                    p0.y.max(p1.y),
                )
            }
            Self::StartClip { path } => path.bounding_box(),
            Self::StopClip => (0.0, 0.0, 0.0, 0.0),
        }
    }
}

/// A completed MetaPost figure (produced by beginfig ... endfig).
#[derive(Clone, Debug, PartialEq)]
pub struct MpFigure {
    pub charcode: i32,
    pub bounding_box: (f64, f64, f64, f64),
    pub objects: Vec<MpObject>,
}

impl MpFigure {
    pub fn new(charcode: i32, objects: Vec<MpObject>) -> Self {
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;

        for obj in &objects {
            if matches!(obj, MpObject::StopClip) {
                continue;
            }
            let (x0, y0, x1, y1) = obj.bounding_box();
            min_x = min_x.min(x0);
            min_y = min_y.min(y0);
            max_x = max_x.max(x1);
            max_y = max_y.max(y1);
        }

        let bounding_box = if min_x.is_finite() && min_y.is_finite() {
            (min_x, min_y, max_x, max_y)
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };

        Self {
            charcode,
            bounding_box,
            objects,
        }
    }

    pub fn width(&self) -> f64 {
        (self.bounding_box.2 - self.bounding_box.0).max(0.0)
    }

    pub fn height(&self) -> f64 {
        self.bounding_box.3.max(0.0)
    }

    pub fn depth(&self) -> f64 {
        (-self.bounding_box.1).max(0.0)
    }

    pub fn to_postscript(&self) -> String {
        let (llx, lly, urx, ury) = self.bounding_box;
        let mut ps = String::new();
        let _ = writeln!(ps, "%!PS-Adobe-3.0 EPSF-3.0");
        let _ = writeln!(
            ps,
            "%%BoundingBox: {} {} {} {}",
            llx.floor() as i32,
            lly.floor() as i32,
            urx.ceil() as i32,
            ury.ceil() as i32
        );
        let _ = writeln!(
            ps,
            "%%HiResBoundingBox: {:.4} {:.4} {:.4} {:.4}",
            llx, lly, urx, ury
        );
        let _ = writeln!(ps, "%%Creator: Ratex MetaPost 3.00");
        let _ = writeln!(ps, "%%EndComments");
        let _ = writeln!(ps, "gsave");

        for obj in &self.objects {
            match obj {
                MpObject::Fill { path, color } => {
                    let _ = writeln!(ps, "gsave");
                    let _ = write_path_ps(&mut ps, path);
                    let _ = writeln!(ps, "{}", color.to_postscript());
                    let _ = writeln!(ps, "fill grestore");
                }
                MpObject::Stroke {
                    path,
                    color,
                    width,
                    dash,
                    line_cap,
                    line_join,
                    miter_limit,
                } => {
                    let _ = writeln!(ps, "gsave");
                    let _ = write_path_ps(&mut ps, path);
                    let _ = writeln!(ps, "{}", color.to_postscript());
                    let _ = writeln!(ps, "{width:.4} setlinewidth");
                    let _ = writeln!(ps, "{line_cap} setlinecap");
                    let _ = writeln!(ps, "{line_join} setlinejoin");
                    let _ = writeln!(ps, "{miter_limit:.4} setmiterlimit");
                    if let Some(d) = dash {
                        let pat_str = d
                            .pattern
                            .iter()
                            .map(|v| format!("{v:.4}"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let _ = writeln!(ps, "[{pat_str}] {:.4} setdash", d.offset);
                    }
                    let _ = writeln!(ps, "stroke grestore");
                }
                MpObject::Text {
                    text,
                    font,
                    scale,
                    transform,
                    color,
                } => {
                    let _ = writeln!(ps, "gsave");
                    let _ = writeln!(
                        ps,
                        "[{:.4} {:.4} {:.4} {:.4} {:.4} {:.4}] concat",
                        transform.xx,
                        transform.yx,
                        transform.xy,
                        transform.yy,
                        transform.x0,
                        transform.y0
                    );
                    let _ = writeln!(ps, "/{} findfont {:.4} scalefont setfont", font, scale * 10.0);
                    let _ = writeln!(ps, "{}", color.to_postscript());
                    let _ = writeln!(ps, "0 0 moveto ({}) show grestore", escape_ps_string(text));
                }
                MpObject::StartClip { path } => {
                    let _ = writeln!(ps, "gsave");
                    let _ = write_path_ps(&mut ps, path);
                    let _ = writeln!(ps, "clip newpath");
                }
                MpObject::StopClip => {
                    let _ = writeln!(ps, "grestore");
                }
            }
        }

        let _ = writeln!(ps, "grestore");
        let _ = writeln!(ps, "%%EOF");
        ps
    }

    pub fn to_svg(&self) -> String {
        let (llx, lly, urx, ury) = self.bounding_box;
        let width = (urx - llx).max(1.0);
        let height = (ury - lly).max(1.0);

        let mut svg = String::new();
        let _ = writeln!(
            svg,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{:.4} {:.4} {:.4} {:.4}" width="{:.4}" height="{:.4}">"#,
            llx, -ury, width, height, width, height
        );
        let _ = writeln!(svg, r#"<g transform="scale(1, -1)">"#);

        for obj in &self.objects {
            match obj {
                MpObject::Fill { path, color } => {
                    let d = path_to_svg_d(path);
                    let _ = writeln!(
                        svg,
                        r#"<path d="{d}" fill="{}" stroke="none"/>"#,
                        color.to_svg_str()
                    );
                }
                MpObject::Stroke {
                    path,
                    color,
                    width,
                    dash,
                    line_cap,
                    line_join,
                    miter_limit,
                } => {
                    let d = path_to_svg_d(path);
                    let cap_str = match line_cap {
                        1 => "round",
                        2 => "square",
                        _ => "butt",
                    };
                    let join_str = match line_join {
                        1 => "round",
                        2 => "bevel",
                        _ => "miter",
                    };
                    let dash_attr = if let Some(d) = dash {
                        let pat_str = d
                            .pattern
                            .iter()
                            .map(|v| format!("{v:.2}"))
                            .collect::<Vec<_>>()
                            .join(",");
                        format!(r#" stroke-dasharray="{pat_str}" stroke-dashoffset="{:.2}""#, d.offset)
                    } else {
                        String::new()
                    };

                    let _ = writeln!(
                        svg,
                        r#"<path d="{d}" fill="none" stroke="{}" stroke-width="{:.4}" stroke-linecap="{cap_str}" stroke-linejoin="{join_str}" stroke-miterlimit="{miter_limit:.4}"{dash_attr}/>"#,
                        color.to_svg_str(),
                        width
                    );
                }
                MpObject::Text {
                    text,
                    scale,
                    transform,
                    color,
                    ..
                } => {
                    let pt = transform.apply(Pair::ZERO);
                    let _ = writeln!(
                        svg,
                        r#"<text x="{:.4}" y="{:.4}" font-size="{:.4}" fill="{}">{}</text>"#,
                        pt.x,
                        pt.y,
                        scale * 10.0,
                        color.to_svg_str(),
                        text
                    );
                }
                MpObject::StartClip { .. } => {}
                MpObject::StopClip => {}
            }
        }

        let _ = writeln!(svg, "</g></svg>");
        svg
    }
}

fn write_path_ps(ps: &mut String, path: &Path) {
    if path.knots.is_empty() {
        return;
    }
    let _ = writeln!(ps, "{:.4} {:.4} moveto", path.knots[0].p.x, path.knots[0].p.y);
    let n = path.knots.len();
    for i in 0..n {
        let next_idx = if i + 1 < n {
            i + 1
        } else if path.closed {
            0
        } else {
            break;
        };

        let p1 = path.knots[i].right_control;
        let p2 = path.knots[next_idx].left_control;
        let p3 = path.knots[next_idx].p;

        if (p1 == path.knots[i].p) && (p2 == p3) {
            let _ = writeln!(ps, "{:.4} {:.4} lineto", p3.x, p3.y);
        } else {
            let _ = writeln!(
                ps,
                "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} curveto",
                p1.x, p1.y, p2.x, p2.y, p3.x, p3.y
            );
        }
    }
    if path.closed {
        let _ = writeln!(ps, "closepath");
    }
}

fn path_to_svg_d(path: &Path) -> String {
    if path.knots.is_empty() {
        return String::new();
    }
    let mut d = format!("M {:.4},{:.4}", path.knots[0].p.x, path.knots[0].p.y);
    let n = path.knots.len();
    for i in 0..n {
        let next_idx = if i + 1 < n {
            i + 1
        } else if path.closed {
            0
        } else {
            break;
        };

        let p1 = path.knots[i].right_control;
        let p2 = path.knots[next_idx].left_control;
        let p3 = path.knots[next_idx].p;

        if (p1 == path.knots[i].p) && (p2 == p3) {
            let _ = write!(d, " L {:.4},{:.4}", p3.x, p3.y);
        } else {
            let _ = write!(
                d,
                " C {:.4},{:.4} {:.4},{:.4} {:.4},{:.4}",
                p1.x, p1.y, p2.x, p2.y, p3.x, p3.y
            );
        }
    }
    if path.closed {
        d.push_str(" Z");
    }
    d
}

fn escape_ps_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '(' => out.push_str(r"\("),
            ')' => out.push_str(r"\)"),
            '\\' => out.push_str(r"\\"),
            _ => out.push(c),
        }
    }
    out
}
