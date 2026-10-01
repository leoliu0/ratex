//! PostScript objects, matrices and the EPS bounding box.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::rc::Rc;

/// Interned PostScript name.
pub(crate) type NameId = u32;
/// Index into the interpreter's file table.
pub(crate) type FileId = u32;

/// Small deterministic hasher (FxHash). Dictionary iteration order depends on
/// the hasher; a fixed one keeps `forall` order, and so the output, reproducible.
#[derive(Default, Clone, Copy)]
pub(crate) struct FxHasher(u64);

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl Hasher for FxHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            self.add(u64::from_le_bytes(chunk.try_into().unwrap()));
        }
        for &b in chunks.remainder() {
            self.add(u64::from(b));
        }
    }
    fn write_u8(&mut self, n: u8) {
        self.add(u64::from(n));
    }
    fn write_u32(&mut self, n: u32) {
        self.add(u64::from(n));
    }
    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }
    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }
}

pub(crate) type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

/// A dictionary key. Strings are converted to names and integral reals to
/// integers, as PostScript requires; other composites compare by identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    Name(NameId),
    Int(i64),
    Real(u64),
    Bool(bool),
    Null,
    Mark,
    Identity(usize),
}

/// A PostScript string: a shared byte buffer plus the visible interval.
#[derive(Clone, Debug)]
pub(crate) struct PsString {
    pub(crate) data: Rc<RefCell<Vec<u8>>>,
    pub(crate) start: u32,
    pub(crate) len: u32,
}

impl PsString {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        let len = bytes.len() as u32;
        Self { data: Rc::new(RefCell::new(bytes)), start: 0, len }
    }

    pub(crate) fn to_vec(&self) -> Vec<u8> {
        let data = self.data.borrow();
        data[self.start as usize..(self.start + self.len) as usize].to_vec()
    }

    pub(crate) fn with<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        let data = self.data.borrow();
        f(&data[self.start as usize..(self.start + self.len) as usize])
    }

    pub(crate) fn interval(&self, start: u32, len: u32) -> Self {
        Self { data: self.data.clone(), start: self.start + start, len }
    }

    fn same(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.data, &other.data) && self.start == other.start && self.len == other.len
    }
}

/// A PostScript array or procedure body: shared elements plus the visible interval.
#[derive(Clone, Debug)]
pub(crate) struct PsArray {
    pub(crate) data: Rc<RefCell<Vec<Value>>>,
    pub(crate) start: u32,
    pub(crate) len: u32,
}

impl PsArray {
    pub(crate) fn new(items: Vec<Value>) -> Self {
        let len = items.len() as u32;
        Self { data: Rc::new(RefCell::new(items)), start: 0, len }
    }

    #[inline]
    pub(crate) fn get(&self, index: u32) -> Value {
        self.data.borrow()[(self.start + index) as usize].clone()
    }

    pub(crate) fn set(&self, index: u32, value: Value) {
        self.data.borrow_mut()[(self.start + index) as usize] = value;
    }

    pub(crate) fn to_vec(&self) -> Vec<Value> {
        let data = self.data.borrow();
        data[self.start as usize..(self.start + self.len) as usize].to_vec()
    }

    pub(crate) fn interval(&self, start: u32, len: u32) -> Self {
        Self { data: self.data.clone(), start: self.start + start, len }
    }

    fn same(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.data, &other.data) && self.start == other.start && self.len == other.len
    }
}

/// A PostScript dictionary (reference semantics).
#[derive(Clone, Debug)]
pub(crate) struct PsDict(pub(crate) Rc<RefCell<FxMap<Key, Value>>>);

impl PsDict {
    pub(crate) fn new() -> Self {
        Self(Rc::new(RefCell::new(FxMap::default())))
    }

    #[inline]
    pub(crate) fn get(&self, key: &Key) -> Option<Value> {
        self.0.borrow().get(key).cloned()
    }

    pub(crate) fn put(&self, key: Key, value: Value) {
        self.0.borrow_mut().insert(key, value);
    }

    pub(crate) fn len(&self) -> usize {
        self.0.borrow().len()
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// A PostScript object.
#[derive(Clone, Debug)]
pub(crate) enum Value {
    Int(i64),
    Real(f64),
    Bool(bool),
    Null,
    Mark,
    Name(NameId),
    ExecName(NameId),
    String(PsString),
    ExecString(PsString),
    Array(PsArray),
    Proc(PsArray),
    Dict(PsDict),
    Operator(u16),
    File(FileId),
    ExecFile(FileId),
    Save(u32),
    /// A `gstate` object: index into the interpreter's saved graphics states.
    GState(u32),
}

impl Value {
    pub(crate) fn as_f64(&self) -> Option<f64> {
        match *self {
            Self::Int(i) => Some(i as f64),
            Self::Real(r) => Some(r),
            _ => None,
        }
    }

    pub(crate) fn is_exec(&self) -> bool {
        matches!(
            self,
            Self::ExecName(_) | Self::ExecString(_) | Self::Proc(_) | Self::Operator(_) | Self::ExecFile(_)
        )
    }

    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Self::Int(_) => "integertype",
            Self::Real(_) => "realtype",
            Self::Bool(_) => "booleantype",
            Self::Null => "nulltype",
            Self::Mark => "marktype",
            Self::Name(_) | Self::ExecName(_) => "nametype",
            Self::String(_) | Self::ExecString(_) => "stringtype",
            Self::Array(_) | Self::Proc(_) => "arraytype",
            Self::Dict(_) => "dicttype",
            Self::Operator(_) => "operatortype",
            Self::File(_) | Self::ExecFile(_) => "filetype",
            Self::Save(_) => "savetype",
            Self::GState(_) => "gstatetype",
        }
    }

    /// PostScript `eq`: numbers by value, strings by content (and against
    /// names by text), other composites by identity.
    pub(crate) fn ps_eq(&self, other: &Self, name_text: &dyn Fn(NameId) -> Rc<[u8]>) -> bool {
        use Value::*;
        match (self, other) {
            (Int(a), Int(b)) => a == b,
            (Int(_) | Real(_), Int(_) | Real(_)) => self.as_f64() == other.as_f64(),
            (Bool(a), Bool(b)) => a == b,
            (Null, Null) | (Mark, Mark) => true,
            (Name(a) | ExecName(a), Name(b) | ExecName(b)) => a == b,
            (String(a) | ExecString(a), String(b) | ExecString(b)) => {
                a.same(b) || a.with(|x| b.with(|y| x == y))
            }
            (String(s) | ExecString(s), Name(n) | ExecName(n))
            | (Name(n) | ExecName(n), String(s) | ExecString(s)) => s.with(|x| *x == *name_text(*n)),
            (Array(a) | Proc(a), Array(b) | Proc(b)) => a.same(b),
            (Dict(a), Dict(b)) => a.same(b),
            (Operator(a), Operator(b)) => a == b,
            (File(a) | ExecFile(a), File(b) | ExecFile(b)) => a == b,
            (Save(a), Save(b)) | (GState(a), GState(b)) => a == b,
            _ => false,
        }
    }
}

/// 2D affine transformation `[a b c d tx ty]` in PostScript's row-vector convention.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Matrix {
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
    pub(crate) const IDENTITY: Self = Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };

    pub(crate) fn new(m: [f64; 6]) -> Self {
        Self { a: m[0], b: m[1], c: m[2], d: m[3], tx: m[4], ty: m[5] }
    }

    pub(crate) fn to_array(self) -> [f64; 6] {
        [self.a, self.b, self.c, self.d, self.tx, self.ty]
    }

    pub(crate) fn translate(tx: f64, ty: f64) -> Self {
        Self { tx, ty, ..Self::IDENTITY }
    }

    pub(crate) fn scale(sx: f64, sy: f64) -> Self {
        Self { a: sx, d: sy, ..Self::IDENTITY }
    }

    pub(crate) fn rotate(deg: f64) -> Self {
        let (sin, cos) = deg_sin_cos(deg);
        Self { a: cos, b: sin, c: -sin, d: cos, tx: 0.0, ty: 0.0 }
    }

    #[inline]
    pub(crate) fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (x * self.a + y * self.c + self.tx, x * self.b + y * self.d + self.ty)
    }

    #[inline]
    pub(crate) fn apply_delta(&self, dx: f64, dy: f64) -> (f64, f64) {
        (dx * self.a + dy * self.c, dx * self.b + dy * self.d)
    }

    /// `self` followed by `rhs`.
    pub(crate) fn then(&self, rhs: &Self) -> Self {
        Self {
            a: self.a * rhs.a + self.b * rhs.c,
            b: self.a * rhs.b + self.b * rhs.d,
            c: self.c * rhs.a + self.d * rhs.c,
            d: self.c * rhs.b + self.d * rhs.d,
            tx: self.tx * rhs.a + self.ty * rhs.c + rhs.tx,
            ty: self.tx * rhs.b + self.ty * rhs.d + rhs.ty,
        }
    }

    pub(crate) fn invert(&self) -> Option<Self> {
        let det = self.a * self.d - self.b * self.c;
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let inv = 1.0 / det;
        Some(Self {
            a: self.d * inv,
            b: -self.b * inv,
            c: -self.c * inv,
            d: self.a * inv,
            tx: (self.c * self.ty - self.d * self.tx) * inv,
            ty: (self.b * self.tx - self.a * self.ty) * inv,
        })
    }
}

/// Sine and cosine of an angle in degrees, exact at multiples of 90°.
pub(crate) fn deg_sin_cos(deg: f64) -> (f64, f64) {
    let r = deg.rem_euclid(360.0);
    if r == 0.0 {
        (0.0, 1.0)
    } else if r == 90.0 {
        (1.0, 0.0)
    } else if r == 180.0 {
        (0.0, -1.0)
    } else if r == 270.0 {
        (-1.0, 0.0)
    } else {
        let rad = deg.to_radians();
        (rad.sin(), rad.cos())
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
