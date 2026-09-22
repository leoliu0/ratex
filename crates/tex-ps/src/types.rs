//! Core PostScript data types, graphics state, and path definitions.

use std::collections::HashMap;

/// A PostScript object value.
#[derive(Clone, Debug, PartialEq)]
pub enum PsValue {
    Integer(i64),
    Real(f64),
    Boolean(bool),
    String(Vec<u8>),
    LiteralName(String),    // /name
    ExecutableName(String), // name
    Array(Vec<PsValue>),
    Procedure(Vec<PsValue>), // { ... }
    Dict(HashMap<String, PsValue>),
    Mark,
    Null,
}

impl PsValue {
    pub fn to_f64(&self) -> Option<f64> {
        match self {
            Self::Integer(i) => Some(*i as f64),
            Self::Real(r) => Some(*r),
            _ => None,
        }
    }

    pub fn to_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(i) => Some(*i),
            Self::Real(r) => Some(*r as i64),
            _ => None,
        }
    }

    pub fn to_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::LiteralName(s) | Self::ExecutableName(s) => Some(s),
            Self::String(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }
}

/// A path construction operator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathOp {
    MoveTo(f64, f64),
    LineTo(f64, f64),
    CurveTo(f64, f64, f64, f64, f64, f64),
    Close,
}

/// PostScript color specification.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PsColor {
    Gray(f64),
    Rgb(f64, f64, f64),
    Cmyk(f64, f64, f64, f64),
}

impl Default for PsColor {
    fn default() -> Self {
        Self::Gray(0.0)
    }
}

/// 2D affine transformation matrix: [a, b, c, d, tx, ty]
/// [ a  b  0 ]
/// [ c  d  0 ]
/// [ tx ty 1 ]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Default for Matrix {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    pub fn translate(tx: f64, ty: f64) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx,
            ty,
        }
    }

    pub fn scale(sx: f64, sy: f64) -> Self {
        Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            tx: 0.0,
            ty: 0.0,
        }
    }

    pub fn rotate(deg: f64) -> Self {
        let rad = deg.to_radians();
        let cos = rad.cos();
        let sin = rad.sin();
        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            tx: 0.0,
            ty: 0.0,
        }
    }

    pub fn transform_point(&self, x: f64, y: f64) -> (f64, f64) {
        (
            x * self.a + y * self.c + self.tx,
            x * self.b + y * self.d + self.ty,
        )
    }

    pub fn transform_delta(&self, dx: f64, dy: f64) -> (f64, f64) {
        (dx * self.a + dy * self.c, dx * self.b + dy * self.d)
    }

    pub fn multiply(&self, rhs: &Self) -> Self {
        Self {
            a: self.a * rhs.a + self.b * rhs.c,
            b: self.a * rhs.b + self.b * rhs.d,
            c: self.c * rhs.a + self.d * rhs.c,
            d: self.c * rhs.b + self.d * rhs.d,
            tx: self.tx * rhs.a + self.ty * rhs.c + rhs.tx,
            ty: self.tx * rhs.b + self.ty * rhs.d + rhs.ty,
        }
    }

    pub fn invert(&self) -> Option<Self> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let inv_det = 1.0 / det;
        Some(Self {
            a: self.d * inv_det,
            b: -self.b * inv_det,
            c: -self.c * inv_det,
            d: self.a * inv_det,
            tx: (self.c * self.ty - self.d * self.tx) * inv_det,
            ty: (self.b * self.tx - self.a * self.ty) * inv_det,
        })
    }
}

/// PostScript graphics state.
#[derive(Clone, Debug)]
pub struct GraphicsState {
    pub ctm: Matrix,
    pub current_point: Option<(f64, f64)>,
    pub path: Vec<PathOp>,
    pub line_width: f64,
    pub line_cap: u8,
    pub line_join: u8,
    pub miter_limit: f64,
    pub dash: Option<(Vec<f64>, f64)>,
    pub color: PsColor,
    pub font_name: String,
    pub font_size: f64,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: Matrix::IDENTITY,
            current_point: None,
            path: Vec::new(),
            line_width: 1.0,
            line_cap: 0,
            line_join: 0,
            miter_limit: 10.0,
            dash: None,
            color: PsColor::Gray(0.0),
            font_name: "Helvetica".to_string(),
            font_size: 10.0,
        }
    }
}

/// Parsed EPS bounding box metadata.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EpsBoundingBox {
    pub llx: f64,
    pub lly: f64,
    pub urx: f64,
    pub ury: f64,
}

impl EpsBoundingBox {
    pub fn width(&self) -> f64 {
        (self.urx - self.llx).max(1.0)
    }

    pub fn height(&self) -> f64 {
        (self.ury - self.lly).max(1.0)
    }
}
