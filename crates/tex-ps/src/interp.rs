//! PostScript interpreter core: objects, stacks, the execution loop, and the
//! non-graphics operators (stack, arithmetic, control, dictionaries, arrays,
//! strings, conversions, files and resources).
//!
//! Hostile input is bounded: execution steps, operand/execution/dictionary/
//! graphics-state stack depths, nested callbacks (glyph procedures, data
//! sources, tint transforms), total allocation, and output size all have hard
//! limits that raise PostScript errors instead of exhausting the host.

use std::rc::Rc;

use crate::files::{FileKind, PsFile};
use crate::graphics::{GState, Output};
use crate::types::{
    EpsBoundingBox, FileId, FxMap, Key, Matrix, NameId, PsArray, PsDict, PsString, Value,
};

/// Execution error in the PostScript interpreter.
#[derive(Clone, Debug, PartialEq)]
pub struct PsError {
    pub error_type: String,
    pub message: String,
}

impl PsError {
    pub fn new(error_type: &str, message: &str) -> Self {
        Self { error_type: error_type.to_string(), message: message.to_string() }
    }
}

impl std::fmt::Display for PsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error_type, self.message)
    }
}

impl std::error::Error for PsError {}

/// Execution step and memory budgets (fields so tests can lower them).
#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) steps: u64,
    pub(crate) vm: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { steps: 50_000_000, vm: VM_BUDGET }
    }
}

const MAX_OSTACK: usize = 500_000;
const MAX_ESTACK: usize = 5_000;
const MAX_DSTACK: usize = 1_000;
pub(crate) const MAX_GSTACK: usize = 1_000;
const MAX_NESTING: u32 = 32;
const MAX_BIND_DEPTH: u32 = 64;
/// Total bytes of strings, arrays, dictionary entries, path segments and
/// image samples the program may allocate over its whole run.
pub(crate) const VM_BUDGET: usize = 256 << 20;
pub(crate) const MAX_OBJECT_LEN: usize = 16 << 20;

/// Non-local control flow and errors.
#[derive(Debug)]
pub(crate) enum Flow {
    Error(&'static str, String),
    Stop,
    Quit,
}

pub(crate) type Res<T = ()> = Result<T, Flow>;
pub(crate) type OpFn = fn(&mut Interp) -> Res;

#[inline]
pub(crate) fn err<T>(kind: &'static str, message: impl Into<String>) -> Res<T> {
    Err(Flow::Error(kind, message.into()))
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Num {
    I(i64),
    R(f64),
}

impl Num {
    pub(crate) fn f(self) -> f64 {
        match self {
            Self::I(i) => i as f64,
            Self::R(r) => r,
        }
    }
}

/// A PostScript integer result; values outside the 32-bit range become reals.
#[inline]
pub(crate) fn int_value(v: i64) -> Value {
    if (i32::MIN as i64..=i32::MAX as i64).contains(&v) {
        Value::Int(v)
    } else {
        Value::Real(v as f64)
    }
}

#[inline]
fn real_value(v: f64) -> Res<Value> {
    if v.is_finite() {
        Ok(Value::Real(v))
    } else {
        err("undefinedresult", "result is not a finite number")
    }
}

/// Execution stack entries.
pub(crate) enum Frame {
    Proc { arr: PsArray, pos: u32 },
    File(FileId),
    For { proc: PsArray, cur: Num, step: Num, limit: f64 },
    Repeat { proc: PsArray, left: i64 },
    Loop { proc: PsArray },
    ForallArray { proc: PsArray, arr: PsArray, idx: u32 },
    ForallString { proc: PsArray, s: PsString, idx: u32 },
    ForallDict { proc: PsArray, entries: Rc<Vec<(Value, Value)>>, idx: usize },
    Stopped,
    /// Pops `systemdict` pushed by `eexec` once the decrypted file ends.
    EexecEnd,
}

impl Frame {
    fn is_loop(&self) -> bool {
        matches!(
            self,
            Self::For { .. }
                | Self::Repeat { .. }
                | Self::Loop { .. }
                | Self::ForallArray { .. }
                | Self::ForallString { .. }
                | Self::ForallDict { .. }
        )
    }
}

/// Name interner.
#[derive(Default)]
pub(crate) struct Names {
    map: FxMap<Rc<[u8]>, NameId>,
    list: Vec<Rc<[u8]>>,
}

impl Names {
    pub(crate) fn intern(&mut self, bytes: &[u8]) -> NameId {
        if let Some(&id) = self.map.get(bytes) {
            return id;
        }
        let id = self.list.len() as NameId;
        let rc: Rc<[u8]> = Rc::from(bytes);
        self.list.push(rc.clone());
        self.map.insert(rc, id);
        id
    }

    #[inline]
    pub(crate) fn text(&self, id: NameId) -> &Rc<[u8]> {
        &self.list[id as usize]
    }
}

macro_rules! known_names {
    ($($field:ident = $text:literal,)*) => {
        /// Interned names the interpreter refers to directly.
        pub(crate) struct Known { $(pub(crate) $field: NameId,)* }
        impl Known {
            fn new(names: &mut Names) -> Self {
                Self { $($field: names.intern($text),)* }
            }
        }
    };
}

known_names! {
    font_matrix = b"FontMatrix",
    font_type = b"FontType",
    encoding = b"Encoding",
    font_name = b"FontName",
    font_bbox = b"FontBBox",
    build_glyph = b"BuildGlyph",
    build_char = b"BuildChar",
    fid = b"FID",
    base14 = b"RatexBase14",
    paint_type = b"PaintType",
    errorname = b"errorname",
    command = b"command",
    newerror = b"newerror",
    image_type = b"ImageType",
    width = b"Width",
    height = b"Height",
    bits_per_component = b"BitsPerComponent",
    decode = b"Decode",
    image_matrix = b"ImageMatrix",
    data_source = b"DataSource",
    multiple_data_sources = b"MultipleDataSources",
    interpolate = b"Interpolate",
    eod_count = b"EODCount",
    eod_string = b"EODString",
    filter = b"Filter",
    device_gray = b"DeviceGray",
    device_rgb = b"DeviceRGB",
    device_cmyk = b"DeviceCMYK",
    indexed = b"Indexed",
    separation = b"Separation",
    device_n = b"DeviceN",
    pattern = b"Pattern",
    icc_based = b"ICCBased",
    cie_a = b"CIEBasedA",
    cie_abc = b"CIEBasedABC",
    cie_def = b"CIEBasedDEF",
    cie_defg = b"CIEBasedDEFG",
    cal_gray = b"CalGray",
    cal_rgb = b"CalRGB",
    lab = b"Lab",
    n = b"N",
    font = b"Font",
    standard_encoding = b"StandardEncoding",
    iso_latin1_encoding = b"ISOLatin1Encoding",
    page_size = b"PageSize",
    hw_resolution = b"HWResolution",
    notdef = b".notdef",
    lbracket = b"[",
    rbracket = b"]",
    ldict = b"<<",
    rdict = b">>",
}

pub(crate) struct Interp {
    pub(crate) ostack: Vec<Value>,
    pub(crate) estack: Vec<Frame>,
    pub(crate) dstack: Vec<PsDict>,
    pub(crate) names: Names,
    pub(crate) n: Known,
    ops: Vec<OpFn>,
    op_names: Vec<NameId>,
    pub(crate) files: Vec<PsFile>,
    pub(crate) systemdict: PsDict,
    userdict: PsDict,
    error_state: PsDict,
    pub(crate) font_directory: PsDict,
    resources: PsDict,
    pub(crate) standard_encoding: PsArray,
    pub(crate) iso_latin1_encoding: PsArray,
    pub(crate) symbol_encoding: PsArray,
    pub(crate) dingbats_encoding: PsArray,
    pub(crate) gs: GState,
    pub(crate) gstack: Vec<crate::graphics::Saved>,
    pub(crate) gstates: Vec<GState>,
    pub(crate) out: Output,
    pub(crate) bbox: EpsBoundingBox,
    pub(crate) limits: Limits,
    steps: u64,
    vm: usize,
    nesting: u32,
    run_bases: Vec<usize>,
    rand_state: u32,
    packing: bool,
    pub(crate) glyph: Option<crate::fonts::GlyphCtx>,
    /// Paint operators record nothing while measuring glyphs (`stringwidth`).
    pub(crate) suppress: u32,
    /// Paths collected by `charpath` from Type 3 glyph procedures.
    pub(crate) charpaths: Vec<Vec<crate::graphics::Seg>>,
    /// Open `save` levels: (save id, graphics stack depth, journal length).
    saves: Vec<(u32, usize, usize)>,
    next_save: u32,
    /// Dictionary changes made while a `save` is open, undone by `restore`.
    journal: Vec<(PsDict, Key, Option<Value>)>,
}

impl Interp {
    pub(crate) fn new(bbox: EpsBoundingBox) -> Self {
        let mut names = Names::default();
        let n = Known::new(&mut names);
        let mut interp = Self {
            ostack: Vec::new(),
            estack: Vec::new(),
            dstack: Vec::new(),
            names,
            n,
            ops: Vec::new(),
            op_names: Vec::new(),
            files: Vec::new(),
            systemdict: PsDict::new(),
            userdict: PsDict::new(),
            error_state: PsDict::new(),
            font_directory: PsDict::new(),
            resources: PsDict::new(),
            standard_encoding: PsArray::new(Vec::new()),
            iso_latin1_encoding: PsArray::new(Vec::new()),
            symbol_encoding: PsArray::new(Vec::new()),
            dingbats_encoding: PsArray::new(Vec::new()),
            gs: GState::default(),
            gstack: Vec::new(),
            gstates: Vec::new(),
            out: Output::default(),
            bbox,
            limits: Limits::default(),
            steps: 0,
            vm: 0,
            nesting: 0,
            run_bases: Vec::new(),
            rand_state: 0,
            packing: false,
            glyph: None,
            suppress: 0,
            charpaths: Vec::new(),
            saves: Vec::new(),
            next_save: 0,
            journal: Vec::new(),
        };
        interp.init();
        interp
    }

    fn init(&mut self) {
        for table in [OPERATORS, crate::graphics::OPERATORS, crate::fonts::OPERATORS, crate::images::OPERATORS] {
            for &(name, f) in table {
                let id = self.names.intern(name.as_bytes());
                let index = self.ops.len() as u16;
                self.ops.push(f);
                self.op_names.push(id);
                self.systemdict.put(Key::Name(id), Value::Operator(index));
            }
        }
        let encodings = [
            &crate::base14::STANDARD_ENCODING,
            &crate::base14::ISO_LATIN1_ENCODING,
            &crate::base14::SYMBOL_ENCODING,
            &crate::base14::DINGBATS_ENCODING,
        ]
        .map(|table| {
            PsArray::new(table.iter().map(|g| Value::Name(self.names.intern(g))).collect())
        });
        let [standard, latin1, symbol, dingbats] = encodings;
        self.standard_encoding = standard;
        self.iso_latin1_encoding = latin1;
        self.symbol_encoding = symbol;
        self.dingbats_encoding = dingbats;

        let globaldict = PsDict::new();
        let errordict = PsDict::new();
        let statusdict = PsDict::new();
        let product = self.key_bytes(b"product");
        statusdict.put(product, Value::String(PsString::new(b"Ratex".to_vec())));
        let handle_key = self.key_bytes(b"handleerror");
        let handle = self.systemdict.get(&handle_key).unwrap();
        errordict.put(handle_key, handle);
        let entries: [(&[u8], Value); 16] = [
            (b"systemdict", Value::Dict(self.systemdict.clone())),
            (b"userdict", Value::Dict(self.userdict.clone())),
            (b"globaldict", Value::Dict(globaldict.clone())),
            (b"errordict", Value::Dict(errordict)),
            (b"statusdict", Value::Dict(statusdict)),
            (b"$error", Value::Dict(self.error_state.clone())),
            (b"FontDirectory", Value::Dict(self.font_directory.clone())),
            (b"GlobalFontDirectory", Value::Dict(self.font_directory.clone())),
            (b"SharedFontDirectory", Value::Dict(self.font_directory.clone())),
            (b"StandardEncoding", Value::Array(self.standard_encoding.clone())),
            (b"ISOLatin1Encoding", Value::Array(self.iso_latin1_encoding.clone())),
            (b"true", Value::Bool(true)),
            (b"false", Value::Bool(false)),
            (b"null", Value::Null),
            (b"mark", Value::Mark),
            (b"languagelevel", Value::Int(3)),
        ];
        for (name, value) in entries {
            let key = self.key_bytes(name);
            self.systemdict.put(key, value);
        }
        self.error_state.put(Key::Name(self.n.newerror), Value::Bool(false));
        self.dstack = vec![self.systemdict.clone(), globaldict, self.userdict.clone()];
        self.run_program(PRELUDE).expect("the PostScript prelude runs");
        self.steps = 0;
        self.vm = 0;
    }
}

/// Resources defined in PostScript itself.
const PRELUDE: &[u8] = b"
/CIDInit 32 dict begin
  /begincmap {} def /endcmap {} def
  /begincodespacerange {pop mark} def /endcodespacerange {cleartomark} def
  /begincidrange {pop mark} def /endcidrange {cleartomark} def
  /begincidchar {pop mark} def /endcidchar {cleartomark} def
  /beginbfrange {pop mark} def /endbfrange {cleartomark} def
  /beginbfchar {pop mark} def /endbfchar {cleartomark} def
  /beginnotdefrange {pop mark} def /endnotdefrange {cleartomark} def
  /beginnotdefchar {pop mark} def /endnotdefchar {cleartomark} def
  /beginrearrangedfont {pop pop} def /endrearrangedfont {} def
  /usecmap {pop} def /usefont {pop} def /usematrix {pop} def
currentdict end /ProcSet defineresource pop
systemdict begin
/currentuserparams {
  << /MaxPatternCache 32000 /MaxFontItem 12500 /MaxFormItem 100000 /MaxUPathItem 0
     /MaxScreenItem 65536 /MaxLocalVM 1000000 /VMReclaim 0 /VMThreshold 40000
     /MaxDictStack 1000 /MaxExecStack 5000 /MaxOpStack 500000 /MinFontCompress 100
     /JobName () >>
} bind def
/currentsystemparams {
  << /MaxFontCache 400000 /CurFontCache 0 /MaxFormCache 100000 /MaxPatternCache 100000
     /MaxScreenStorage 100000 /CurScreenStorage 0 /MaxDisplayList 100000 /CurDisplayList 0
     /MaxOutlineCache 65000 /MaxUPathCache 0 /MaxImageBuffer 1000000 /MaxRasterMemory 1000000
     /MaxStoredScreenCache 0 /MaxSourceList 0 /MaxDisplayAndSourceList 0 >>
} bind def
/uappend { cvx exec } bind def
/ufill { gsave newpath uappend fill grestore } bind def
/ueofill { gsave newpath uappend eofill grestore } bind def
/ustroke {
  gsave newpath
  dup length 6 eq { dup 0 get type dup /integertype eq exch /realtype eq or } { false } ifelse
  { exch uappend concat } { uappend } ifelse
  stroke grestore
} bind def
/execform {
  gsave
  dup /Matrix get concat
  dup /BBox get aload pop exch 3 index sub exch 2 index sub rectclip
  dup /PaintProc get exec
  grestore
} bind def
end
";

impl Interp {

    pub(crate) fn key_bytes(&mut self, name: &[u8]) -> Key {
        Key::Name(self.names.intern(name))
    }

    pub(crate) fn name_text(&self, id: NameId) -> Rc<[u8]> {
        self.names.text(id).clone()
    }

    pub(crate) fn name_string(&self, id: NameId) -> String {
        String::from_utf8_lossy(self.names.text(id)).into_owned()
    }

    /// Runs a complete PostScript program.
    pub(crate) fn run_program(&mut self, program: &[u8]) -> Result<(), PsError> {
        let f = self.open_bytes(Rc::from(program));
        self.estack.push(Frame::File(f));
        match self.run(0) {
            Ok(()) | Err(Flow::Quit) | Err(Flow::Stop) => Ok(()),
            Err(Flow::Error(kind, message)) => Err(PsError::new(kind, &message)),
        }
    }

    // ---- allocation and limits -------------------------------------------

    pub(crate) fn alloc(&mut self, bytes: usize) -> Res {
        self.vm = self.vm.saturating_add(bytes);
        if self.vm > self.limits.vm {
            return err("VMerror", "PostScript memory budget exhausted");
        }
        Ok(())
    }

    pub(crate) fn alloc_len(&mut self, len: i64, unit: usize) -> Res<usize> {
        if len < 0 {
            return err("rangecheck", "negative length");
        }
        if len as u64 > MAX_OBJECT_LEN as u64 {
            return err("limitcheck", "object too large");
        }
        self.alloc(len as usize * unit)?;
        Ok(len as usize)
    }

    pub(crate) fn new_string(&mut self, bytes: Vec<u8>) -> Res<PsString> {
        self.alloc_len(bytes.len() as i64, 1)?;
        Ok(PsString::new(bytes))
    }

    pub(crate) fn new_array(&mut self, items: Vec<Value>) -> Res<PsArray> {
        self.alloc_len(items.len() as i64, std::mem::size_of::<Value>())?;
        Ok(PsArray::new(items))
    }

    pub(crate) fn dict_put(&mut self, dict: &PsDict, key: Key, value: Value) -> Res {
        let mut map = dict.0.borrow_mut();
        let old = map.insert(key, value);
        if old.is_none() {
            self.vm = self.vm.saturating_add(64);
        }
        if !self.saves.is_empty() {
            self.journal.push((dict.clone(), key, old));
            self.vm = self.vm.saturating_add(64);
        }
        if self.vm > self.limits.vm {
            return err("VMerror", "PostScript memory budget exhausted");
        }
        Ok(())
    }

    fn dict_remove(&mut self, dict: &PsDict, key: Key) {
        let old = dict.0.borrow_mut().remove(&key);
        if old.is_some() && !self.saves.is_empty() {
            self.journal.push((dict.clone(), key, old));
        }
    }

    /// `restore`: undoes dictionary changes and graphics states since the save.
    fn restore(&mut self, id: u32) -> Res {
        let Some(pos) = self.saves.iter().position(|s| s.0 == id) else {
            return err("invalidrestore", "restore");
        };
        let (_, level, mark) = self.saves[pos];
        self.saves.truncate(pos);
        for (dict, key, old) in self.journal.drain(mark..).rev() {
            let mut map = dict.0.borrow_mut();
            match old {
                Some(v) => map.insert(key, v),
                None => map.remove(&key),
            };
        }
        crate::graphics::restore_to(self, level);
        Ok(())
    }

    // ---- operand stack ---------------------------------------------------

    #[inline]
    pub(crate) fn push(&mut self, v: Value) -> Res {
        if self.ostack.len() >= MAX_OSTACK {
            return err("stackoverflow", "operand stack overflow");
        }
        self.ostack.push(v);
        Ok(())
    }

    #[inline]
    pub(crate) fn need(&self, n: usize) -> Res {
        if self.ostack.len() < n {
            return err("stackunderflow", "operand stack underflow");
        }
        Ok(())
    }

    /// The operand `i` places below the top (0 = top); `need` must have succeeded.
    #[inline]
    pub(crate) fn arg(&self, i: usize) -> &Value {
        &self.ostack[self.ostack.len() - 1 - i]
    }

    #[inline]
    pub(crate) fn pop_n(&mut self, n: usize) {
        let len = self.ostack.len();
        self.ostack.truncate(len - n);
    }

    pub(crate) fn pop(&mut self) -> Res<Value> {
        self.ostack.pop().map_or_else(|| err("stackunderflow", "operand stack underflow"), Ok)
    }

    pub(crate) fn num_at(&self, i: usize) -> Res<Num> {
        self.need(i + 1)?;
        match *self.arg(i) {
            Value::Int(v) => Ok(Num::I(v)),
            Value::Real(v) => Ok(Num::R(v)),
            _ => err("typecheck", "expected a number"),
        }
    }

    #[inline]
    pub(crate) fn f64_at(&self, i: usize) -> Res<f64> {
        self.num_at(i).map(Num::f)
    }

    pub(crate) fn int_at(&self, i: usize) -> Res<i64> {
        self.need(i + 1)?;
        match *self.arg(i) {
            Value::Int(v) => Ok(v),
            _ => err("typecheck", "expected an integer"),
        }
    }

    pub(crate) fn bool_at(&self, i: usize) -> Res<bool> {
        self.need(i + 1)?;
        match *self.arg(i) {
            Value::Bool(v) => Ok(v),
            _ => err("typecheck", "expected a boolean"),
        }
    }

    pub(crate) fn proc_at(&self, i: usize) -> Res<PsArray> {
        self.need(i + 1)?;
        match self.arg(i) {
            Value::Proc(a) | Value::Array(a) => Ok(a.clone()),
            _ => err("typecheck", "expected a procedure"),
        }
    }

    pub(crate) fn dict_at(&self, i: usize) -> Res<PsDict> {
        self.need(i + 1)?;
        match self.arg(i) {
            Value::Dict(d) => Ok(d.clone()),
            _ => err("typecheck", "expected a dictionary"),
        }
    }

    pub(crate) fn string_at(&self, i: usize) -> Res<PsString> {
        self.need(i + 1)?;
        match self.arg(i) {
            Value::String(s) | Value::ExecString(s) => Ok(s.clone()),
            _ => err("typecheck", "expected a string"),
        }
    }

    pub(crate) fn array_at(&self, i: usize) -> Res<PsArray> {
        self.need(i + 1)?;
        match self.arg(i) {
            Value::Array(a) | Value::Proc(a) => Ok(a.clone()),
            _ => err("typecheck", "expected an array"),
        }
    }

    /// Reads a numeric array (matrix, dash pattern, bounding box).
    pub(crate) fn numbers(&self, arr: &PsArray) -> Res<Vec<f64>> {
        let data = arr.data.borrow();
        data[arr.start as usize..(arr.start + arr.len) as usize]
            .iter()
            .map(|v| v.as_f64().map_or_else(|| err("typecheck", "expected numbers"), Ok))
            .collect()
    }

    pub(crate) fn matrix_from(&self, v: &Value) -> Res<Matrix> {
        match v {
            Value::Array(a) | Value::Proc(a) if a.len == 6 => {
                let m = self.numbers(a)?;
                Ok(Matrix::new([m[0], m[1], m[2], m[3], m[4], m[5]]))
            }
            Value::Array(_) | Value::Proc(_) => err("rangecheck", "matrix must have 6 elements"),
            _ => err("typecheck", "expected a matrix"),
        }
    }

    pub(crate) fn matrix_at(&self, i: usize) -> Res<Matrix> {
        self.need(i + 1)?;
        self.matrix_from(self.arg(i))
    }

    /// Stores a matrix into an existing 6-element array and returns it.
    pub(crate) fn store_matrix(&mut self, target: &PsArray, m: Matrix) -> Res<Value> {
        if target.len != 6 {
            return err("rangecheck", "matrix must have 6 elements");
        }
        for (i, v) in m.to_array().into_iter().enumerate() {
            target.set(i as u32, real_value(v)?);
        }
        Ok(Value::Array(target.clone()))
    }

    pub(crate) fn matrix_value(&mut self, m: Matrix) -> Res<Value> {
        let arr = self.new_array(vec![Value::Null; 6])?;
        self.store_matrix(&arr, m)
    }

    // ---- dictionaries ----------------------------------------------------

    pub(crate) fn key_of(&mut self, v: &Value) -> Key {
        match v {
            Value::Name(n) | Value::ExecName(n) => Key::Name(*n),
            Value::String(s) | Value::ExecString(s) => {
                let bytes = s.to_vec();
                Key::Name(self.names.intern(&bytes))
            }
            Value::Int(i) => Key::Int(*i),
            Value::Real(r) => {
                if r.fract() == 0.0 && r.abs() < 9.0e15 {
                    Key::Int(*r as i64)
                } else {
                    Key::Real(r.to_bits())
                }
            }
            Value::Bool(b) => Key::Bool(*b),
            Value::Null => Key::Null,
            Value::Mark => Key::Mark,
            Value::Array(a) | Value::Proc(a) => Key::Identity(Rc::as_ptr(&a.data) as usize + a.start as usize),
            Value::Dict(d) => Key::Identity(Rc::as_ptr(&d.0) as usize),
            Value::Operator(i) => Key::Identity(usize::from(*i) << 1 | 1),
            Value::File(f) | Value::ExecFile(f) => Key::Identity((*f as usize) << 2 | 2),
            Value::Save(s) => Key::Identity((*s as usize) << 3 | 3),
            Value::GState(s) => Key::Identity((*s as usize) << 3 | 5),
        }
    }

    pub(crate) fn value_of_key(key: &Key) -> Value {
        match *key {
            Key::Name(n) => Value::Name(n),
            Key::Int(i) => Value::Int(i),
            Key::Real(bits) => Value::Real(f64::from_bits(bits)),
            Key::Bool(b) => Value::Bool(b),
            Key::Mark => Value::Mark,
            Key::Null | Key::Identity(_) => Value::Null,
        }
    }

    #[inline]
    pub(crate) fn lookup(&self, name: NameId) -> Option<Value> {
        let key = Key::Name(name);
        self.dstack.iter().rev().find_map(|d| d.get(&key))
    }

    pub(crate) fn lookup_key(&self, key: &Key) -> Option<Value> {
        self.dstack.iter().rev().find_map(|d| d.get(key))
    }

    // ---- execution -------------------------------------------------------

    pub(crate) fn push_proc_frame(&mut self, arr: PsArray) -> Res {
        if arr.len == 0 {
            return Ok(());
        }
        self.push_frame(Frame::Proc { arr, pos: 0 })
    }

    pub(crate) fn push_frame(&mut self, frame: Frame) -> Res {
        if self.estack.len() >= MAX_ESTACK {
            return err("execstackoverflow", "execution stack overflow");
        }
        self.estack.push(frame);
        Ok(())
    }

    /// Executes an object found in a procedure body or scanned from a file.
    #[inline]
    fn exec_direct(&mut self, obj: Value) -> Res {
        match obj {
            Value::ExecName(n) => match self.lookup(n) {
                Some(v) => self.exec_resolved(v),
                None => err("undefined", self.name_string(n)),
            },
            Value::Operator(i) => (self.ops[i as usize])(self),
            Value::ExecString(_) | Value::ExecFile(_) => self.exec_resolved(obj),
            other => self.push(other),
        }
    }

    /// Executes an object obtained by name lookup or by `exec`.
    pub(crate) fn exec_resolved(&mut self, mut v: Value) -> Res {
        loop {
            match v {
                Value::Proc(arr) => return self.push_proc_frame(arr),
                Value::Operator(i) => return (self.ops[i as usize])(self),
                Value::ExecName(n) => {
                    self.tick()?;
                    v = match self.lookup(n) {
                        Some(found) => found,
                        None => return err("undefined", self.name_string(n)),
                    };
                }
                Value::ExecString(s) => {
                    let f = self.open_temp(Rc::from(s.to_vec()));
                    return self.push_frame(Frame::File(f));
                }
                Value::ExecFile(f) => return self.push_frame(Frame::File(f)),
                other => return self.push(other),
            }
        }
    }

    #[inline]
    fn tick(&mut self) -> Res {
        self.steps += 1;
        if self.steps > self.limits.steps {
            return err("limitcheck", "PostScript execution step limit exceeded");
        }
        Ok(())
    }

    /// Runs until the execution stack shrinks to `base` entries.
    fn run(&mut self, base: usize) -> Res {
        while self.estack.len() > base {
            self.tick()?;
            if let Err(flow) = self.step() {
                self.unwind(flow, base)?;
            }
        }
        Ok(())
    }

    fn unwind(&mut self, flow: Flow, base: usize) -> Res {
        if let Flow::Quit = flow {
            self.estack.truncate(base);
            return Err(flow);
        }
        if let Flow::Error(kind, message) = &flow {
            if *kind == "limitcheck" && message.contains("step limit") {
                self.estack.truncate(base);
                return Err(flow);
            }
        }
        let stopped = self.estack[base..].iter().rposition(|f| matches!(f, Frame::Stopped));
        match stopped {
            Some(offset) => {
                self.estack.truncate(base + offset);
                if let Flow::Error(kind, message) = flow {
                    let name = self.names.intern(kind.as_bytes());
                    let command = self.names.intern(message.as_bytes());
                    let state = self.error_state.clone();
                    state.put(Key::Name(self.n.errorname), Value::Name(name));
                    state.put(Key::Name(self.n.command), Value::Name(command));
                    state.put(Key::Name(self.n.newerror), Value::Bool(true));
                }
                self.push(Value::Bool(true))
            }
            None => {
                self.estack.truncate(base);
                Err(flow)
            }
        }
    }

    fn step(&mut self) -> Res {
        let top = self.estack.len() - 1;
        match &mut self.estack[top] {
            Frame::Proc { arr, pos } => {
                let obj = arr.get(*pos);
                *pos += 1;
                if *pos >= arr.len {
                    self.estack.pop();
                }
                self.exec_direct(obj)
            }
            Frame::File(f) => {
                let f = *f;
                match self.scan_token(f)? {
                    Some(obj) => self.exec_direct(obj),
                    None => {
                        self.estack.truncate(top);
                        if self.files[f as usize].temporary {
                            self.close_file(f);
                        }
                        Ok(())
                    }
                }
            }
            Frame::For { proc, cur, step, limit } => {
                let value = *cur;
                let done = if step.f() >= 0.0 { value.f() > *limit } else { value.f() < *limit };
                if done {
                    self.estack.pop();
                    return Ok(());
                }
                *cur = match (value, *step) {
                    (Num::I(a), Num::I(b)) => Num::I(a + b),
                    (a, b) => Num::R(a.f() + b.f()),
                };
                let proc = proc.clone();
                self.push(match value {
                    Num::I(i) => Value::Int(i),
                    Num::R(r) => Value::Real(r),
                })?;
                self.push_proc_frame(proc)
            }
            Frame::Repeat { proc, left } => {
                if *left <= 0 {
                    self.estack.pop();
                    return Ok(());
                }
                *left -= 1;
                let proc = proc.clone();
                self.push_proc_frame(proc)
            }
            Frame::Loop { proc } => {
                let proc = proc.clone();
                if proc.len == 0 {
                    // `{} loop` spins without other side effects; the step limit ends it.
                    return Ok(());
                }
                self.push_proc_frame(proc)
            }
            Frame::ForallArray { proc, arr, idx } => {
                if *idx >= arr.len {
                    self.estack.pop();
                    return Ok(());
                }
                let v = arr.get(*idx);
                *idx += 1;
                let proc = proc.clone();
                self.push(v)?;
                self.push_proc_frame(proc)
            }
            Frame::ForallString { proc, s, idx } => {
                if *idx >= s.len {
                    self.estack.pop();
                    return Ok(());
                }
                let byte = s.data.borrow()[(s.start + *idx) as usize];
                *idx += 1;
                let proc = proc.clone();
                self.push(Value::Int(i64::from(byte)))?;
                self.push_proc_frame(proc)
            }
            Frame::ForallDict { proc, entries, idx } => {
                if *idx >= entries.len() {
                    self.estack.pop();
                    return Ok(());
                }
                let (k, v) = entries[*idx].clone();
                *idx += 1;
                let proc = proc.clone();
                self.push(k)?;
                self.push(v)?;
                self.push_proc_frame(proc)
            }
            Frame::Stopped => {
                self.estack.pop();
                self.push(Value::Bool(false))
            }
            Frame::EexecEnd => {
                self.estack.pop();
                if self.dstack.len() > 3 && self.dstack.last().is_some_and(|d| d.same(&self.systemdict)) {
                    self.dstack.pop();
                }
                Ok(())
            }
        }
    }

    /// Calls a procedure synchronously (glyph procedures, data sources, tint
    /// transforms) and returns once it has finished.
    pub(crate) fn call(&mut self, proc: Value) -> Res {
        if self.nesting >= MAX_NESTING {
            return err("limitcheck", "callbacks nested too deeply");
        }
        let base = self.estack.len();
        self.nesting += 1;
        self.run_bases.push(base);
        let result = self.exec_resolved(proc).and_then(|()| self.run(base));
        self.run_bases.pop();
        self.nesting -= 1;
        if result.is_err() {
            self.estack.truncate(base);
        }
        result
    }

    // ---- files -------------------------------------------------------------

    pub(crate) fn open_bytes(&mut self, data: Rc<[u8]>) -> FileId {
        self.add_file(PsFile::new(FileKind::Bytes { data, pos: 0 }))
    }

    pub(crate) fn add_file(&mut self, file: PsFile) -> FileId {
        // Reuse closed in-memory files so string execution in loops does not
        // grow the table without bound.
        if let Some(i) = self.files.iter().position(|f| f.reusable()) {
            if !self.estack.iter().any(|fr| matches!(fr, Frame::File(id) if *id as usize == i)) {
                self.files[i] = file;
                return i as FileId;
            }
        }
        self.files.push(file);
        (self.files.len() - 1) as FileId
    }

    pub(crate) fn current_file(&self) -> Option<FileId> {
        self.estack.iter().rev().find_map(|f| match f {
            Frame::File(id) => Some(*id),
            _ => None,
        })
    }
}

// ---- formatting helpers ---------------------------------------------------

/// Formats a real the way PostScript's `cvs` does (`%g`-like, always with a
/// decimal point or exponent).
pub(crate) fn format_real(r: f64) -> String {
    if r == 0.0 {
        return "0.0".into();
    }
    let exp = r.abs().log10().floor() as i32;
    if !(-5..6).contains(&exp) {
        let s = format!("{:.5e}", r);
        let (mantissa, e) = s.split_once('e').unwrap();
        let mut mantissa = mantissa.trim_end_matches('0').to_string();
        if mantissa.ends_with('.') {
            mantissa.push('0');
        }
        let e: i32 = e.parse().unwrap_or(0);
        return format!("{mantissa}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    }
    let decimals = (5 - exp).max(0) as usize;
    let mut s = format!("{:.*}", decimals, r);
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.push('0');
        }
    } else {
        s.push_str(".0");
    }
    s
}

impl Interp {
    /// Text of an object for `cvs` / `=`.
    pub(crate) fn text_of(&self, v: &Value) -> Vec<u8> {
        match v {
            Value::Int(i) => i.to_string().into_bytes(),
            Value::Real(r) => format_real(*r).into_bytes(),
            Value::Bool(b) => if *b { b"true".to_vec() } else { b"false".to_vec() },
            Value::Name(n) | Value::ExecName(n) => self.names.text(*n).to_vec(),
            Value::String(s) | Value::ExecString(s) => s.to_vec(),
            Value::Operator(i) => self.names.text(self.op_names[*i as usize]).to_vec(),
            _ => b"--nostringval--".to_vec(),
        }
    }
}

// ---- operators ---------------------------------------------------------------

macro_rules! ops {
    ($($name:literal => $f:expr,)*) => {
        &[$(($name, $f as OpFn),)*]
    };
}

fn noop(_: &mut Interp) -> Res {
    Ok(())
}

fn pop1(i: &mut Interp) -> Res {
    i.need(1)?;
    i.pop_n(1);
    Ok(())
}

fn pop2(i: &mut Interp) -> Res {
    i.need(2)?;
    i.pop_n(2);
    Ok(())
}

pub(crate) static OPERATORS: &[(&str, OpFn)] = ops! {
    // stack
    "pop" => pop1,
    "exch" => |i: &mut Interp| { i.need(2)?; let n = i.ostack.len(); i.ostack.swap(n - 1, n - 2); Ok(()) },
    "dup" => |i: &mut Interp| { i.need(1)?; let v = i.arg(0).clone(); i.push(v) },
    "copy" => op_copy,
    "index" => |i: &mut Interp| {
        let n = i.int_at(0)?;
        if n < 0 { return err("rangecheck", "index"); }
        i.need(n as usize + 2)?;
        let v = i.arg(n as usize + 1).clone();
        i.pop_n(1);
        i.push(v)
    },
    "roll" => |i: &mut Interp| {
        let j = i.int_at(0)?;
        let n = i.int_at(1)?;
        if n < 0 { return err("rangecheck", "roll"); }
        i.need(n as usize + 2)?;
        i.pop_n(2);
        if n > 0 {
            let len = i.ostack.len();
            let slice = &mut i.ostack[len - n as usize..];
            slice.rotate_right(j.rem_euclid(n) as usize);
        }
        Ok(())
    },
    "clear" => |i: &mut Interp| { i.ostack.clear(); Ok(()) },
    "count" => |i: &mut Interp| { let n = i.ostack.len() as i64; i.push(Value::Int(n)) },
    "mark" => |i: &mut Interp| i.push(Value::Mark),
    "[" => |i: &mut Interp| i.push(Value::Mark),
    "<<" => |i: &mut Interp| i.push(Value::Mark),
    "cleartomark" => |i: &mut Interp| { let n = i.count_to_mark()?; i.pop_n(n + 1); Ok(()) },
    "counttomark" => |i: &mut Interp| { let n = i.count_to_mark()?; i.push(Value::Int(n as i64)) },
    "]" => |i: &mut Interp| {
        let n = i.count_to_mark()?;
        let len = i.ostack.len();
        let items = i.ostack.split_off(len - n);
        i.ostack.pop();
        let arr = i.new_array(items)?;
        i.push(Value::Array(arr))
    },
    ">>" => |i: &mut Interp| {
        let n = i.count_to_mark()?;
        if n % 2 != 0 { return err("rangecheck", "odd number of dictionary operands"); }
        let len = i.ostack.len();
        let items = i.ostack.split_off(len - n);
        i.ostack.pop();
        let dict = PsDict::new();
        let mut it = items.into_iter();
        while let (Some(k), Some(v)) = (it.next(), it.next()) {
            let key = i.key_of(&k);
            i.dict_put(&dict, key, v)?;
        }
        i.push(Value::Dict(dict))
    },
    // arithmetic
    "add" => |i: &mut Interp| arith(i, |a, b| a.checked_add(b), |a, b| a + b),
    "sub" => |i: &mut Interp| arith(i, |a, b| a.checked_sub(b), |a, b| a - b),
    "mul" => |i: &mut Interp| arith(i, |a, b| a.checked_mul(b), |a, b| a * b),
    "div" => |i: &mut Interp| {
        let b = i.f64_at(0)?;
        let a = i.f64_at(1)?;
        if b == 0.0 { return err("undefinedresult", "division by zero"); }
        let v = real_value(a / b)?;
        i.pop_n(2);
        i.push(v)
    },
    "idiv" => |i: &mut Interp| {
        let b = i.int_at(0)?;
        let a = i.int_at(1)?;
        if b == 0 { return err("undefinedresult", "division by zero"); }
        i.pop_n(2);
        i.push(int_value(a.wrapping_div(b)))
    },
    "mod" => |i: &mut Interp| {
        let b = i.int_at(0)?;
        let a = i.int_at(1)?;
        if b == 0 { return err("undefinedresult", "division by zero"); }
        i.pop_n(2);
        i.push(int_value(a.wrapping_rem(b)))
    },
    "neg" => |i: &mut Interp| unary(i, |a| int_value(-a), |a| Value::Real(-a)),
    "abs" => |i: &mut Interp| unary(i, |a| int_value(a.abs()), |a| Value::Real(a.abs())),
    "ceiling" => |i: &mut Interp| unary(i, Value::Int, |a| Value::Real(a.ceil())),
    "floor" => |i: &mut Interp| unary(i, Value::Int, |a| Value::Real(a.floor())),
    "round" => |i: &mut Interp| unary(i, Value::Int, |a| Value::Real((a + 0.5).floor())),
    "truncate" => |i: &mut Interp| unary(i, Value::Int, |a| Value::Real(a.trunc())),
    "sqrt" => |i: &mut Interp| {
        let a = i.f64_at(0)?;
        if a < 0.0 { return err("rangecheck", "sqrt of a negative number"); }
        i.pop_n(1);
        i.push(Value::Real(a.sqrt()))
    },
    "atan" => |i: &mut Interp| {
        let den = i.f64_at(0)?;
        let num = i.f64_at(1)?;
        if num == 0.0 && den == 0.0 { return err("undefinedresult", "atan 0 0"); }
        let deg = num.atan2(den).to_degrees();
        i.pop_n(2);
        i.push(Value::Real(if deg < 0.0 { deg + 360.0 } else { deg }))
    },
    "cos" => |i: &mut Interp| { let a = i.f64_at(0)?; i.pop_n(1); i.push(Value::Real(crate::types::deg_sin_cos(a).1)) },
    "sin" => |i: &mut Interp| { let a = i.f64_at(0)?; i.pop_n(1); i.push(Value::Real(crate::types::deg_sin_cos(a).0)) },
    "exp" => |i: &mut Interp| {
        let e = i.f64_at(0)?;
        let b = i.f64_at(1)?;
        if b < 0.0 && e.fract() != 0.0 { return err("undefinedresult", "exp"); }
        let v = real_value(b.powf(e))?;
        i.pop_n(2);
        i.push(v)
    },
    "ln" => |i: &mut Interp| log_op(i, f64::ln),
    "log" => |i: &mut Interp| log_op(i, f64::log10),
    "rand" => |i: &mut Interp| {
        i.rand_state = i.rand_state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let v = i64::from(i.rand_state >> 1);
        i.push(Value::Int(v))
    },
    "srand" => |i: &mut Interp| { let s = i.int_at(0)?; i.pop_n(1); i.rand_state = s as u32; Ok(()) },
    "rrand" => |i: &mut Interp| { let s = i64::from(i.rand_state); i.push(Value::Int(s)) },
    // relational, boolean and bitwise
    "eq" => |i: &mut Interp| { i.need(2)?; let r = i.values_eq(i.arg(1), i.arg(0)); i.pop_n(2); i.push(Value::Bool(r)) },
    "ne" => |i: &mut Interp| { i.need(2)?; let r = !i.values_eq(i.arg(1), i.arg(0)); i.pop_n(2); i.push(Value::Bool(r)) },
    "ge" => |i: &mut Interp| compare(i, |o| o.is_ge()),
    "gt" => |i: &mut Interp| compare(i, |o| o.is_gt()),
    "le" => |i: &mut Interp| compare(i, |o| o.is_le()),
    "lt" => |i: &mut Interp| compare(i, |o| o.is_lt()),
    "and" => |i: &mut Interp| logic(i, |a, b| a & b, |a, b| a & b),
    "or" => |i: &mut Interp| logic(i, |a, b| a | b, |a, b| a | b),
    "xor" => |i: &mut Interp| logic(i, |a, b| a ^ b, |a, b| a ^ b),
    "not" => |i: &mut Interp| {
        i.need(1)?;
        let v = match *i.arg(0) {
            Value::Bool(b) => Value::Bool(!b),
            Value::Int(n) => Value::Int(!n),
            _ => return err("typecheck", "not"),
        };
        i.pop_n(1);
        i.push(v)
    },
    "bitshift" => |i: &mut Interp| {
        let shift = i.int_at(0)?;
        let v = i.int_at(1)? as i32 as u32;
        let r = if shift >= 32 || shift <= -32 { 0 } else if shift >= 0 { v << shift } else { v >> -shift };
        i.pop_n(2);
        i.push(Value::Int(i64::from(r as i32)))
    },
    // control
    "exec" => |i: &mut Interp| { let v = i.pop()?; i.exec_resolved(v) },
    "if" => |i: &mut Interp| {
        let proc = i.proc_at(0)?;
        let cond = i.bool_at(1)?;
        i.pop_n(2);
        if cond { i.push_proc_frame(proc) } else { Ok(()) }
    },
    "ifelse" => |i: &mut Interp| {
        let p2 = i.proc_at(0)?;
        let p1 = i.proc_at(1)?;
        let cond = i.bool_at(2)?;
        i.pop_n(3);
        i.push_proc_frame(if cond { p1 } else { p2 })
    },
    "for" => |i: &mut Interp| {
        let proc = i.proc_at(0)?;
        let limit = i.num_at(1)?;
        let step = i.num_at(2)?;
        let init = i.num_at(3)?;
        i.pop_n(4);
        let (cur, step) = match (init, step) {
            (Num::I(a), Num::I(b)) => (Num::I(a), Num::I(b)),
            (a, b) => (Num::R(a.f()), Num::R(b.f())),
        };
        let limit = limit.f();
        i.push_frame(Frame::For { proc, cur, step, limit })
    },
    "repeat" => |i: &mut Interp| {
        let proc = i.proc_at(0)?;
        let n = i.int_at(1)?;
        if n < 0 { return err("rangecheck", "repeat count"); }
        i.pop_n(2);
        i.push_frame(Frame::Repeat { proc, left: n })
    },
    "loop" => |i: &mut Interp| { let proc = i.proc_at(0)?; i.pop_n(1); i.push_frame(Frame::Loop { proc }) },
    "exit" => |i: &mut Interp| {
        let base = i.run_bases.last().copied().unwrap_or(0);
        for idx in (base..i.estack.len()).rev() {
            match &i.estack[idx] {
                f if f.is_loop() => { i.estack.truncate(idx); return Ok(()); }
                Frame::Stopped | Frame::File(_) => break,
                _ => {}
            }
        }
        err("invalidexit", "exit outside a loop")
    },
    "stop" => |_: &mut Interp| Err(Flow::Stop),
    "stopped" => |i: &mut Interp| {
        let v = i.pop()?;
        i.push_frame(Frame::Stopped)?;
        i.exec_resolved(v)
    },
    "forall" => op_forall,
    "quit" => |_: &mut Interp| Err(Flow::Quit),
    "countexecstack" => |i: &mut Interp| { let n = i.estack.len() as i64; i.push(Value::Int(n)) },
    "execstack" => |i: &mut Interp| {
        let arr = i.array_at(0)?;
        let n = i.estack.len().min(arr.len as usize) as u32;
        i.pop_n(1);
        i.push(Value::Array(arr.interval(0, n)))
    },
    "start" => noop,
    "handleerror" => noop,
    // dictionaries
    "dict" => |i: &mut Interp| {
        let n = i.int_at(0)?;
        if n < 0 { return err("rangecheck", "dict"); }
        i.pop_n(1);
        i.alloc(64)?;
        i.push(Value::Dict(PsDict::new()))
    },
    "begin" => |i: &mut Interp| {
        let d = i.dict_at(0)?;
        if i.dstack.len() >= MAX_DSTACK { return err("dictstackoverflow", "dictionary stack overflow"); }
        i.pop_n(1);
        i.dstack.push(d);
        Ok(())
    },
    "end" => |i: &mut Interp| {
        if i.dstack.len() <= 3 { return err("dictstackunderflow", "end"); }
        i.dstack.pop();
        Ok(())
    },
    "def" => |i: &mut Interp| {
        i.need(2)?;
        let key = { let k = i.arg(1).clone(); i.key_of(&k) };
        let value = i.arg(0).clone();
        i.pop_n(2);
        let dict = i.dstack.last().unwrap().clone();
        i.dict_put(&dict, key, value)
    },
    "load" => |i: &mut Interp| {
        i.need(1)?;
        let key = { let k = i.arg(0).clone(); i.key_of(&k) };
        match i.lookup_key(&key) {
            Some(v) => { i.pop_n(1); i.push(v) }
            None => err("undefined", match key { Key::Name(n) => i.name_string(n), _ => "load".into() }),
        }
    },
    "store" => |i: &mut Interp| {
        i.need(2)?;
        let key = { let k = i.arg(1).clone(); i.key_of(&k) };
        let value = i.arg(0).clone();
        i.pop_n(2);
        let dict = i.dstack.iter().rev().find(|d| d.0.borrow().contains_key(&key)).cloned()
            .unwrap_or_else(|| i.dstack.last().unwrap().clone());
        i.dict_put(&dict, key, value)
    },
    "undef" => |i: &mut Interp| {
        i.need(2)?;
        let dict = i.dict_at(1)?;
        let key = { let k = i.arg(0).clone(); i.key_of(&k) };
        i.pop_n(2);
        i.dict_remove(&dict, key);
        Ok(())
    },
    "known" => |i: &mut Interp| {
        i.need(2)?;
        let dict = i.dict_at(1)?;
        let key = { let k = i.arg(0).clone(); i.key_of(&k) };
        let known = dict.0.borrow().contains_key(&key);
        i.pop_n(2);
        i.push(Value::Bool(known))
    },
    "where" => |i: &mut Interp| {
        i.need(1)?;
        let key = { let k = i.arg(0).clone(); i.key_of(&k) };
        i.pop_n(1);
        match i.dstack.iter().rev().find(|d| d.0.borrow().contains_key(&key)).cloned() {
            Some(d) => { i.push(Value::Dict(d))?; i.push(Value::Bool(true)) }
            None => i.push(Value::Bool(false)),
        }
    },
    "currentdict" => |i: &mut Interp| { let d = i.dstack.last().unwrap().clone(); i.push(Value::Dict(d)) },
    "countdictstack" => |i: &mut Interp| { let n = i.dstack.len() as i64; i.push(Value::Int(n)) },
    "dictstack" => |i: &mut Interp| {
        let arr = i.array_at(0)?;
        if (arr.len as usize) < i.dstack.len() { return err("rangecheck", "dictstack"); }
        for (k, d) in i.dstack.iter().enumerate() { arr.set(k as u32, Value::Dict(d.clone())); }
        let n = i.dstack.len() as u32;
        i.pop_n(1);
        i.push(Value::Array(arr.interval(0, n)))
    },
    "cleardictstack" => |i: &mut Interp| { i.dstack.truncate(3); Ok(()) },
    "maxlength" => |i: &mut Interp| {
        let d = i.dict_at(0)?;
        let n = d.0.borrow().capacity().max(d.len()) as i64;
        i.pop_n(1);
        i.push(Value::Int(n))
    },
    // arrays and strings
    "array" => |i: &mut Interp| {
        let n = i.int_at(0)?;
        let len = i.alloc_len(n, std::mem::size_of::<Value>())?;
        i.pop_n(1);
        i.push(Value::Array(PsArray::new(vec![Value::Null; len])))
    },
    "packedarray" => |i: &mut Interp| {
        let n = i.int_at(0)?;
        if n < 0 { return err("rangecheck", "packedarray"); }
        i.need(n as usize + 1)?;
        i.pop_n(1);
        let len = i.ostack.len();
        let items = i.ostack.split_off(len - n as usize);
        let arr = i.new_array(items)?;
        i.push(Value::Array(arr))
    },
    "setpacking" => |i: &mut Interp| { let b = i.bool_at(0)?; i.pop_n(1); i.packing = b; Ok(()) },
    "currentpacking" => |i: &mut Interp| { let b = i.packing; i.push(Value::Bool(b)) },
    "string" => |i: &mut Interp| {
        let n = i.int_at(0)?;
        let len = i.alloc_len(n, 1)?;
        i.pop_n(1);
        i.push(Value::String(PsString::new(vec![0; len])))
    },
    "length" => |i: &mut Interp| {
        i.need(1)?;
        let n = match i.arg(0) {
            Value::Array(a) | Value::Proc(a) => a.len as i64,
            Value::String(s) | Value::ExecString(s) => s.len as i64,
            Value::Dict(d) => d.len() as i64,
            Value::Name(n) | Value::ExecName(n) => i.names.text(*n).len() as i64,
            _ => return err("typecheck", "length"),
        };
        i.pop_n(1);
        i.push(Value::Int(n))
    },
    "get" => op_get,
    "put" => op_put,
    "getinterval" => |i: &mut Interp| {
        let count = i.int_at(0)?;
        let index = i.int_at(1)?;
        i.need(3)?;
        let len = match i.arg(2) {
            Value::Array(a) | Value::Proc(a) => a.len,
            Value::String(s) | Value::ExecString(s) => s.len,
            _ => return err("typecheck", "getinterval"),
        };
        if index < 0 || count < 0 || index + count > i64::from(len) {
            return err("rangecheck", "getinterval");
        }
        let v = match i.arg(2) {
            Value::Array(a) => Value::Array(a.interval(index as u32, count as u32)),
            Value::Proc(a) => Value::Proc(a.interval(index as u32, count as u32)),
            Value::String(s) => Value::String(s.interval(index as u32, count as u32)),
            Value::ExecString(s) => Value::ExecString(s.interval(index as u32, count as u32)),
            _ => unreachable!(),
        };
        i.pop_n(3);
        i.push(v)
    },
    "putinterval" => |i: &mut Interp| {
        i.need(3)?;
        let index = i.int_at(1)?;
        match (i.arg(2).clone(), i.arg(0).clone()) {
            (Value::Array(dst) | Value::Proc(dst), Value::Array(src) | Value::Proc(src)) => {
                if index < 0 || index + i64::from(src.len) > i64::from(dst.len) { return err("rangecheck", "putinterval"); }
                for (k, v) in src.to_vec().into_iter().enumerate() { dst.set(index as u32 + k as u32, v); }
            }
            (Value::String(dst) | Value::ExecString(dst), Value::String(src) | Value::ExecString(src)) => {
                if index < 0 || index + i64::from(src.len) > i64::from(dst.len) { return err("rangecheck", "putinterval"); }
                let bytes = src.to_vec();
                let at = (dst.start + index as u32) as usize;
                dst.data.borrow_mut()[at..at + bytes.len()].copy_from_slice(&bytes);
            }
            _ => return err("typecheck", "putinterval"),
        }
        i.pop_n(3);
        Ok(())
    },
    "aload" => |i: &mut Interp| {
        let arr = i.array_at(0)?;
        let v = i.pop()?;
        for item in arr.to_vec() { i.push(item)?; }
        i.push(v)
    },
    "astore" => |i: &mut Interp| {
        let arr = i.array_at(0)?;
        let n = arr.len as usize;
        i.need(n + 1)?;
        let v = i.pop()?;
        let len = i.ostack.len();
        let items = i.ostack.split_off(len - n);
        for (k, item) in items.into_iter().enumerate() { arr.set(k as u32, item); }
        i.push(v)
    },
    "search" => |i: &mut Interp| search(i, false),
    "anchorsearch" => |i: &mut Interp| search(i, true),
    "token" => op_token,
    // type, attribute and conversion
    "type" => |i: &mut Interp| {
        i.need(1)?;
        let t = i.arg(0).type_name();
        let id = i.names.intern(t.as_bytes());
        i.pop_n(1);
        i.push(Value::ExecName(id))
    },
    "cvlit" => |i: &mut Interp| {
        let v = i.pop()?;
        i.push(match v {
            Value::ExecName(n) => Value::Name(n),
            Value::ExecString(s) => Value::String(s),
            Value::Proc(a) => Value::Array(a),
            Value::ExecFile(f) => Value::File(f),
            other => other,
        })
    },
    "cvx" => |i: &mut Interp| {
        let v = i.pop()?;
        i.push(match v {
            Value::Name(n) => Value::ExecName(n),
            Value::String(s) => Value::ExecString(s),
            Value::Array(a) => Value::Proc(a),
            Value::File(f) => Value::ExecFile(f),
            other => other,
        })
    },
    "xcheck" => |i: &mut Interp| { i.need(1)?; let x = i.arg(0).is_exec(); i.pop_n(1); i.push(Value::Bool(x)) },
    "executeonly" => |i: &mut Interp| i.need(1),
    "noaccess" => |i: &mut Interp| i.need(1),
    "readonly" => |i: &mut Interp| i.need(1),
    "rcheck" => |i: &mut Interp| { i.need(1)?; i.pop_n(1); i.push(Value::Bool(true)) },
    "wcheck" => |i: &mut Interp| { i.need(1)?; i.pop_n(1); i.push(Value::Bool(true)) },
    "cvi" => |i: &mut Interp| {
        i.need(1)?;
        let r = match i.arg(0).clone() {
            Value::Int(n) => n as f64,
            Value::Real(r) => r,
            Value::String(s) | Value::ExecString(s) => match crate::lexer::parse_number(&s.to_vec()) {
                Some(v) => v.as_f64().unwrap(),
                None => return err("syntaxerror", "cvi"),
            },
            _ => return err("typecheck", "cvi"),
        };
        let t = r.trunc();
        if !(i32::MIN as f64..=i32::MAX as f64).contains(&t) { return err("rangecheck", "cvi"); }
        i.pop_n(1);
        i.push(Value::Int(t as i64))
    },
    "cvr" => |i: &mut Interp| {
        i.need(1)?;
        let r = match i.arg(0).clone() {
            Value::Int(n) => n as f64,
            Value::Real(r) => r,
            Value::String(s) | Value::ExecString(s) => match crate::lexer::parse_number(&s.to_vec()) {
                Some(v) => v.as_f64().unwrap(),
                None => return err("syntaxerror", "cvr"),
            },
            _ => return err("typecheck", "cvr"),
        };
        i.pop_n(1);
        i.push(Value::Real(r))
    },
    "cvn" => |i: &mut Interp| {
        let v = match i.string_at(0)? {
            s => {
                let id = i.names.intern(&s.to_vec());
                if matches!(i.arg(0), Value::ExecString(_)) { Value::ExecName(id) } else { Value::Name(id) }
            }
        };
        i.pop_n(1);
        i.push(v)
    },
    "cvs" => |i: &mut Interp| {
        let dst = i.string_at(0)?;
        i.need(2)?;
        let text = i.text_of(i.arg(1));
        if text.len() > dst.len as usize { return err("rangecheck", "cvs"); }
        let at = dst.start as usize;
        dst.data.borrow_mut()[at..at + text.len()].copy_from_slice(&text);
        i.pop_n(2);
        i.push(Value::String(dst.interval(0, text.len() as u32)))
    },
    "cvrs" => |i: &mut Interp| {
        let dst = i.string_at(0)?;
        let radix = i.int_at(1)?;
        let num = i.num_at(2)?;
        if !(2..=36).contains(&radix) { return err("rangecheck", "cvrs radix"); }
        let text = if radix == 10 {
            i.text_of(&match num { Num::I(v) => Value::Int(v), Num::R(r) => Value::Real(r) })
        } else {
            let mut v = match num { Num::I(v) => v as i32 as u32, Num::R(r) => r as i64 as i32 as u32 };
            let mut digits = Vec::new();
            loop {
                let d = (v % radix as u32) as u8;
                digits.push(if d < 10 { b'0' + d } else { b'A' + d - 10 });
                v /= radix as u32;
                if v == 0 { break; }
            }
            digits.reverse();
            digits
        };
        if text.len() > dst.len as usize { return err("rangecheck", "cvrs"); }
        let at = dst.start as usize;
        dst.data.borrow_mut()[at..at + text.len()].copy_from_slice(&text);
        i.pop_n(3);
        i.push(Value::String(dst.interval(0, text.len() as u32)))
    },
    // virtual memory and environment
    "bind" => |i: &mut Interp| {
        i.need(1)?;
        if let Value::Proc(a) | Value::Array(a) = i.arg(0).clone() { i.bind(&a, 0); }
        Ok(())
    },
    "null" => |i: &mut Interp| i.push(Value::Null),
    "version" => |i: &mut Interp| { let s = i.new_string(b"3010".to_vec())?; i.push(Value::String(s)) },
    "product" => |i: &mut Interp| { let s = i.new_string(b"Ratex".to_vec())?; i.push(Value::String(s)) },
    "revision" => |i: &mut Interp| i.push(Value::Int(1)),
    "serialnumber" => |i: &mut Interp| i.push(Value::Int(0)),
    "realtime" => |i: &mut Interp| i.push(Value::Int(0)),
    "usertime" => |i: &mut Interp| i.push(Value::Int(0)),
    "vmstatus" => |i: &mut Interp| {
        let used = i.vm as i64;
        i.push(Value::Int(0))?;
        i.push(int_value(used))?;
        let budget = i.limits.vm as i64;
        i.push(int_value(budget))
    },
    "vmreclaim" => pop1,
    "setvmthreshold" => pop1,
    "setglobal" => pop1,
    "currentglobal" => |i: &mut Interp| i.push(Value::Bool(false)),
    "gcheck" => |i: &mut Interp| { i.need(1)?; i.pop_n(1); i.push(Value::Bool(false)) },
    "save" => |i: &mut Interp| {
        crate::graphics::gsave(i, true)?;
        let id = i.next_save;
        i.next_save += 1;
        i.saves.push((id, i.gstack.len() - 1, i.journal.len()));
        i.push(Value::Save(id))
    },
    "restore" => |i: &mut Interp| {
        i.need(1)?;
        let Value::Save(id) = *i.arg(0) else { return err("typecheck", "restore"); };
        i.pop_n(1);
        i.restore(id)
    },
    "setuserparams" => pop1,
    "setsystemparams" => pop1,
    "setdevparams" => pop2,
    "setpagedevice" => pop1,
    "currentpagedevice" => |i: &mut Interp| {
        let d = PsDict::new();
        let size = i.new_array(vec![Value::Real(i.bbox.width()), Value::Real(i.bbox.height())])?;
        let res = i.new_array(vec![Value::Real(72.0), Value::Real(72.0)])?;
        d.put(Key::Name(i.n.page_size), Value::Array(size));
        d.put(Key::Name(i.n.hw_resolution), Value::Array(res));
        i.push(Value::Dict(d))
    },
    "internaldict" => |i: &mut Interp| { i.need(1)?; i.pop_n(1); i.push(Value::Dict(PsDict::new())) },
    "echo" => pop1,
    "prompt" => noop,
    "executive" => noop,
    "flush" => noop,
    "=" => pop1,
    "==" => pop1,
    "print" => pop1,
    "pstack" => noop,
    "stack" => noop,
    // resources
    "findresource" => op_findresource,
    "defineresource" => op_defineresource,
    "undefineresource" => |i: &mut Interp| {
        i.need(2)?;
        let cat = { let c = i.arg(0).clone(); i.key_of(&c) };
        let key = { let k = i.arg(1).clone(); i.key_of(&k) };
        i.pop_n(2);
        if let Some(Value::Dict(d)) = i.resources.get(&cat) { d.0.borrow_mut().remove(&key); }
        Ok(())
    },
    "resourcestatus" => |i: &mut Interp| {
        i.need(2)?;
        let cat = { let c = i.arg(0).clone(); i.key_of(&c) };
        let key = { let k = i.arg(1).clone(); i.key_of(&k) };
        i.pop_n(2);
        let known = matches!(i.resources.get(&cat), Some(Value::Dict(d)) if d.0.borrow().contains_key(&key))
            || (cat == Key::Name(i.n.font) && i.font_directory.0.borrow().contains_key(&key));
        if known {
            i.push(Value::Int(0))?;
            i.push(Value::Int(0))?;
        }
        i.push(Value::Bool(known))
    },
    "resourceforall" => |i: &mut Interp| { i.need(4)?; i.pop_n(4); Ok(()) },
    "findencoding" => |i: &mut Interp| {
        i.need(1)?;
        let enc = i.arg(0).clone();
        i.pop_n(1);
        let cat = Value::Name(i.n.encoding);
        i.push(enc)?;
        i.push(cat)?;
        op_findresource(i)
    },
    // files
    "currentfile" => |i: &mut Interp| {
        let f = match i.current_file() { Some(f) => f, None => i.open_bytes(Rc::from(&b""[..])) };
        i.push(Value::File(f))
    },
    "file" => |i: &mut Interp| {
        i.need(2)?;
        let name = i.string_at(1)?.to_vec();
        i.pop_n(2);
        match name.as_slice() {
            b"%stdin" => { let f = i.open_bytes(Rc::from(&b""[..])); i.push(Value::File(f)) }
            b"%stdout" | b"%stderr" | b"%lineedit" | b"%statementedit" => {
                let f = i.add_file(PsFile::new(FileKind::Sink));
                i.push(Value::File(f))
            }
            _ => err("invalidfileaccess", String::from_utf8_lossy(&name).into_owned()),
        }
    },
    "run" => |_: &mut Interp| err("invalidfileaccess", "run"),
    "deletefile" => |_: &mut Interp| err("invalidfileaccess", "deletefile"),
    "renamefile" => |_: &mut Interp| err("invalidfileaccess", "renamefile"),
    "status" => |i: &mut Interp| {
        i.need(1)?;
        let open = match *i.arg(0) {
            Value::File(f) | Value::ExecFile(f) => Some(!i.files[f as usize].closed),
            Value::String(_) | Value::ExecString(_) => None,
            _ => return err("typecheck", "status"),
        };
        i.pop_n(1);
        i.push(Value::Bool(open.unwrap_or(false)))
    },
    "read" => |i: &mut Interp| {
        let f = i.file_at(0)?;
        i.pop_n(1);
        match i.getc(f)? {
            Some(b) => { i.push(Value::Int(i64::from(b)))?; i.push(Value::Bool(true)) }
            None => i.push(Value::Bool(false)),
        }
    },
    "readstring" => |i: &mut Interp| read_into(i, crate::files::ReadMode::Binary),
    "readhexstring" => |i: &mut Interp| read_into(i, crate::files::ReadMode::Hex),
    "readline" => |i: &mut Interp| read_into(i, crate::files::ReadMode::Line),
    "write" => pop2,
    "writestring" => pop2,
    "writehexstring" => pop2,
    "bytesavailable" => |i: &mut Interp| {
        let f = i.file_at(0)?;
        i.pop_n(1);
        let n = i.files[f as usize].bytes_available();
        i.push(Value::Int(n))
    },
    "closefile" => |i: &mut Interp| { let f = i.file_at(0)?; i.pop_n(1); i.close_file(f); Ok(()) },
    "flushfile" => |i: &mut Interp| {
        let f = i.file_at(0)?;
        i.pop_n(1);
        if i.files[f as usize].is_input() { while i.getc(f)?.is_some() { i.tick()?; } }
        Ok(())
    },
    "resetfile" => |i: &mut Interp| { let f = i.file_at(0)?; i.pop_n(1); i.files[f as usize].reset(); Ok(()) },
    "fileposition" => |i: &mut Interp| {
        let f = i.file_at(0)?;
        let pos = i.files[f as usize].position().map_or_else(|| err("ioerror", "fileposition"), Ok)?;
        i.pop_n(1);
        i.push(int_value(pos as i64))
    },
    "setfileposition" => |i: &mut Interp| {
        let pos = i.int_at(0)?;
        let f = i.file_at(1)?;
        if pos < 0 || !i.files[f as usize].seek(pos as usize) { return err("ioerror", "setfileposition"); }
        i.pop_n(2);
        Ok(())
    },
    "filter" => crate::files::op_filter,
    "eexec" => crate::files::op_eexec,
};

fn op_copy(i: &mut Interp) -> Res {
    i.need(1)?;
    match i.arg(0).clone() {
        Value::Int(n) => {
            if n < 0 { return err("rangecheck", "copy"); }
            i.need(n as usize + 1)?;
            i.pop_n(1);
            if i.ostack.len() + n as usize > MAX_OSTACK { return err("stackoverflow", "copy"); }
            let len = i.ostack.len();
            i.ostack.extend_from_within(len - n as usize..);
            Ok(())
        }
        Value::Array(dst) | Value::Proc(dst) => {
            let src = i.array_at(1)?;
            if src.len > dst.len { return err("rangecheck", "copy"); }
            for (k, v) in src.to_vec().into_iter().enumerate() { dst.set(k as u32, v); }
            i.pop_n(2);
            i.push(Value::Array(dst.interval(0, src.len)))
        }
        Value::String(dst) | Value::ExecString(dst) => {
            let src = i.string_at(1)?;
            if src.len > dst.len { return err("rangecheck", "copy"); }
            let bytes = src.to_vec();
            let at = dst.start as usize;
            dst.data.borrow_mut()[at..at + bytes.len()].copy_from_slice(&bytes);
            i.pop_n(2);
            i.push(Value::String(dst.interval(0, src.len)))
        }
        Value::Dict(dst) => {
            let src = i.dict_at(1)?;
            let entries: Vec<(Key, Value)> = src.0.borrow().iter().map(|(k, v)| (*k, v.clone())).collect();
            for (k, v) in entries { i.dict_put(&dst, k, v)?; }
            i.pop_n(2);
            i.push(Value::Dict(dst))
        }
        _ => err("typecheck", "copy"),
    }
}

fn op_get(i: &mut Interp) -> Res {
    i.need(2)?;
    let v = match i.arg(1).clone() {
        Value::Array(a) | Value::Proc(a) => {
            let idx = i.int_at(0)?;
            if idx < 0 || idx >= i64::from(a.len) { return err("rangecheck", "get"); }
            a.get(idx as u32)
        }
        Value::String(s) | Value::ExecString(s) => {
            let idx = i.int_at(0)?;
            if idx < 0 || idx >= i64::from(s.len) { return err("rangecheck", "get"); }
            Value::Int(i64::from(s.data.borrow()[(s.start + idx as u32) as usize]))
        }
        Value::Dict(d) => {
            let key = { let k = i.arg(0).clone(); i.key_of(&k) };
            match d.get(&key) {
                Some(v) => v,
                None => {
                    let what = match key { Key::Name(n) => i.name_string(n), _ => "get".into() };
                    return err("undefined", what);
                }
            }
        }
        _ => return err("typecheck", "get"),
    };
    i.pop_n(2);
    i.push(v)
}

fn op_put(i: &mut Interp) -> Res {
    i.need(3)?;
    match i.arg(2).clone() {
        Value::Array(a) | Value::Proc(a) => {
            let idx = i.int_at(1)?;
            if idx < 0 || idx >= i64::from(a.len) { return err("rangecheck", "put"); }
            a.set(idx as u32, i.arg(0).clone());
        }
        Value::String(s) | Value::ExecString(s) => {
            let idx = i.int_at(1)?;
            let byte = i.int_at(0)?;
            if idx < 0 || idx >= i64::from(s.len) || !(0..=255).contains(&byte) { return err("rangecheck", "put"); }
            s.data.borrow_mut()[(s.start + idx as u32) as usize] = byte as u8;
        }
        Value::Dict(d) => {
            let key = { let k = i.arg(1).clone(); i.key_of(&k) };
            let value = i.arg(0).clone();
            i.dict_put(&d, key, value)?;
        }
        _ => return err("typecheck", "put"),
    }
    i.pop_n(3);
    Ok(())
}

fn op_forall(i: &mut Interp) -> Res {
    let proc = i.proc_at(0)?;
    i.need(2)?;
    let frame = match i.arg(1).clone() {
        Value::Array(arr) | Value::Proc(arr) => Frame::ForallArray { proc, arr, idx: 0 },
        Value::String(s) | Value::ExecString(s) => Frame::ForallString { proc, s, idx: 0 },
        Value::Dict(d) => {
            let entries = d.0.borrow().iter().map(|(k, v)| (Interp::value_of_key(k), v.clone())).collect();
            Frame::ForallDict { proc, entries: Rc::new(entries), idx: 0 }
        }
        _ => return err("typecheck", "forall"),
    };
    i.pop_n(2);
    i.push_frame(frame)
}

fn op_token(i: &mut Interp) -> Res {
    i.need(1)?;
    match i.arg(0).clone() {
        Value::String(s) | Value::ExecString(s) => {
            let f = i.open_temp(Rc::from(s.to_vec()));
            let tok = i.scan_token(f);
            let consumed = i.files[f as usize].position().unwrap_or(0) as u32;
            i.close_file(f);
            let tok = tok?;
            i.pop_n(1);
            match tok {
                Some(v) => {
                    let rest = s.interval(consumed.min(s.len), s.len - consumed.min(s.len));
                    i.push(Value::String(rest))?;
                    i.push(v)?;
                    i.push(Value::Bool(true))
                }
                None => i.push(Value::Bool(false)),
            }
        }
        Value::File(f) | Value::ExecFile(f) => {
            let tok = i.scan_token(f)?;
            i.pop_n(1);
            match tok {
                Some(v) => { i.push(v)?; i.push(Value::Bool(true)) }
                None => i.push(Value::Bool(false)),
            }
        }
        _ => err("typecheck", "token"),
    }
}

fn search(i: &mut Interp, anchored: bool) -> Res {
    let seek = i.string_at(0)?.to_vec();
    let s = i.string_at(1)?;
    let hay = s.to_vec();
    let found = if anchored {
        hay.starts_with(&seek).then_some(0)
    } else if seek.is_empty() {
        Some(0)
    } else {
        hay.windows(seek.len()).position(|w| w == seek.as_slice())
    };
    i.pop_n(2);
    match found {
        Some(at) => {
            let at = at as u32;
            let n = seek.len() as u32;
            i.push(Value::String(s.interval(at + n, s.len - at - n)))?;
            i.push(Value::String(s.interval(at, n)))?;
            if !anchored {
                i.push(Value::String(s.interval(0, at)))?;
            }
            i.push(Value::Bool(true))
        }
        None => {
            i.push(Value::String(s))?;
            i.push(Value::Bool(false))
        }
    }
}

fn read_into(i: &mut Interp, mode: crate::files::ReadMode) -> Res {
    let dst = i.string_at(0)?;
    let f = i.file_at(1)?;
    i.pop_n(2);
    let (n, complete) = i.read_string(f, &dst, mode)?;
    i.push(Value::String(dst.interval(0, n as u32)))?;
    i.push(Value::Bool(complete))
}

fn arith(i: &mut Interp, int_op: fn(i64, i64) -> Option<i64>, real_op: fn(f64, f64) -> f64) -> Res {
    let b = i.num_at(0)?;
    let a = i.num_at(1)?;
    let v = match (a, b) {
        (Num::I(x), Num::I(y)) => match int_op(x, y) {
            Some(r) => int_value(r),
            None => real_value(real_op(x as f64, y as f64))?,
        },
        (x, y) => real_value(real_op(x.f(), y.f()))?,
    };
    i.pop_n(2);
    i.push(v)
}

fn unary(i: &mut Interp, int_op: fn(i64) -> Value, real_op: fn(f64) -> Value) -> Res {
    let v = match i.num_at(0)? {
        Num::I(x) => int_op(x),
        Num::R(x) => real_op(x),
    };
    i.pop_n(1);
    i.push(v)
}

fn log_op(i: &mut Interp, f: fn(f64) -> f64) -> Res {
    let a = i.f64_at(0)?;
    if a <= 0.0 {
        return err("rangecheck", "logarithm of a non-positive number");
    }
    i.pop_n(1);
    i.push(Value::Real(f(a)))
}

fn compare(i: &mut Interp, test: fn(std::cmp::Ordering) -> bool) -> Res {
    i.need(2)?;
    let ord = match (i.arg(1), i.arg(0)) {
        (Value::String(a) | Value::ExecString(a), Value::String(b) | Value::ExecString(b)) => {
            a.with(|x| b.with(|y| x.cmp(y)))
        }
        (a, b) => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => match x.partial_cmp(&y) {
                Some(o) => o,
                None => return err("undefinedresult", "comparison"),
            },
            _ => return err("typecheck", "comparison"),
        },
    };
    i.pop_n(2);
    i.push(Value::Bool(test(ord)))
}

fn logic(i: &mut Interp, bool_op: fn(bool, bool) -> bool, int_op: fn(i64, i64) -> i64) -> Res {
    i.need(2)?;
    let v = match (i.arg(1), i.arg(0)) {
        (Value::Bool(a), Value::Bool(b)) => Value::Bool(bool_op(*a, *b)),
        (Value::Int(a), Value::Int(b)) => Value::Int(int_op(*a, *b)),
        _ => return err("typecheck", "boolean or integer operands expected"),
    };
    i.pop_n(2);
    i.push(v)
}

fn op_findresource(i: &mut Interp) -> Res {
    i.need(2)?;
    let cat = { let c = i.arg(0).clone(); i.key_of(&c) };
    let key_value = i.arg(1).clone();
    let key = i.key_of(&key_value);
    if cat == Key::Name(i.n.font) {
        i.pop_n(1);
        return crate::fonts::findfont(i);
    }
    let found = match i.resources.get(&cat) {
        Some(Value::Dict(d)) => d.get(&key),
        _ => None,
    };
    // Every category exists as a (possibly empty) resource category.
    let found = found.or_else(|| {
        if cat != i.key_bytes(b"Category") {
            return None;
        }
        let d = PsDict::new();
        let categories = match i.resources.get(&cat) {
            Some(Value::Dict(c)) => c,
            _ => {
                let c = PsDict::new();
                i.resources.put(cat, Value::Dict(c.clone()));
                c
            }
        };
        categories.put(key, Value::Dict(d.clone()));
        Some(Value::Dict(d))
    });
    let found = found.or_else(|| {
        if cat != Key::Name(i.n.encoding) {
            return None;
        }
        if key == Key::Name(i.n.standard_encoding) {
            Some(Value::Array(i.standard_encoding.clone()))
        } else if key == Key::Name(i.n.iso_latin1_encoding) {
            Some(Value::Array(i.iso_latin1_encoding.clone()))
        } else {
            None
        }
    });
    match found {
        Some(v) => {
            i.pop_n(2);
            i.push(v)
        }
        None => {
            let what = match key { Key::Name(n) => i.name_string(n), _ => "findresource".into() };
            err("undefinedresource", what)
        }
    }
}

fn op_defineresource(i: &mut Interp) -> Res {
    i.need(3)?;
    let cat = { let c = i.arg(0).clone(); i.key_of(&c) };
    if cat == Key::Name(i.n.font) {
        i.pop_n(1);
        return crate::fonts::definefont(i);
    }
    let instance = i.arg(1).clone();
    let key = { let k = i.arg(2).clone(); i.key_of(&k) };
    let dict = match i.resources.get(&cat) {
        Some(Value::Dict(d)) => d,
        _ => {
            let d = PsDict::new();
            i.resources.put(cat, Value::Dict(d.clone()));
            d
        }
    };
    i.dict_put(&dict, key, instance.clone())?;
    i.pop_n(3);
    i.push(instance)
}

impl Interp {
    fn count_to_mark(&self) -> Res<usize> {
        self.ostack
            .iter()
            .rev()
            .position(|v| matches!(v, Value::Mark))
            .map_or_else(|| err("unmatchedmark", "no mark on the stack"), Ok)
    }

    pub(crate) fn values_eq(&self, a: &Value, b: &Value) -> bool {
        a.ps_eq(b, &|n| self.name_text(n))
    }

    pub(crate) fn file_at(&self, i: usize) -> Res<FileId> {
        self.need(i + 1)?;
        match *self.arg(i) {
            Value::File(f) | Value::ExecFile(f) => Ok(f),
            _ => err("typecheck", "expected a file"),
        }
    }

    /// `bind`: replaces executable names that resolve to operators, recursing
    /// into nested procedures (depth-limited; arrays may contain themselves).
    fn bind(&mut self, arr: &PsArray, depth: u32) {
        if depth > MAX_BIND_DEPTH {
            return;
        }
        for k in 0..arr.len {
            match arr.get(k) {
                Value::ExecName(n) => {
                    if let Some(op @ Value::Operator(_)) = self.lookup(n) {
                        arr.set(k, op);
                    }
                }
                Value::Proc(inner) => self.bind(&inner, depth + 1),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_limited(src: &[u8], limits: Limits) -> Result<(), PsError> {
        let mut i = Interp::new(EpsBoundingBox { llx: 0.0, lly: 0.0, urx: 10.0, ury: 10.0 });
        i.limits = limits;
        i.run_program(src)
    }

    #[test]
    fn endless_programs_hit_the_step_limit() {
        let limits = Limits { steps: 200_000, ..Limits::default() };
        for src in [&b"{} loop"[..], b"{ 1 pop } loop", b"/f { f } def f", b"{ { stop } stopped pop } loop", b"0 0 1 {} for"] {
            let e = run_limited(src, limits).unwrap_err();
            assert_eq!(e.error_type, "limitcheck", "{}", String::from_utf8_lossy(src));
        }
    }

    #[test]
    fn allocation_is_bounded_over_the_whole_run() {
        let limits = Limits { vm: 1 << 20, ..Limits::default() };
        for src in [&b"{ 1000 array pop } loop"[..], b"{ 1000 string pop } loop", b"save { /x 1 def } loop", b"{ 0 0 moveto 1 1 lineto } loop"] {
            let e = run_limited(src, limits).unwrap_err();
            assert_eq!(e.error_type, "VMerror", "{}", String::from_utf8_lossy(src));
        }
    }
}
