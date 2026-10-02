//! The `pdfscanner` library (lpdfscannerlib.c): a content stream tokenizer that
//! calls `operatortable[operator](scanner, info)` with the operands collected on
//! a stack. The tokenizer is a port of the C one, byte quirks included (a byte
//! >= 0x80 reads as a negative `char`, which switches to the next stream or
//! ends the scan).

use std::any::Any;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use tex_lua::{CallbackLua, Lua, LuaApi, LuaResult, LuaTable, LuaValueKind, UserDataTrait, Value};


/// `streamGetChar` at the end of the data (a byte of 0xFF reads as -1 too).
const EOF: i32 = -1000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ty {
    Integer = 1,
    Real,
    Boolean,
    Name,
    Operator,
    Str,
    StartArray,
    StopArray,
    StartDict,
    StopDict,
}

#[derive(Clone, Debug)]
struct Tok {
    ty: Ty,
    value: f64,
    string: Vec<u8>,
}

impl Tok {
    fn new(ty: Ty) -> Tok {
        Tok { ty, value: 0.0, string: Vec::new() }
    }
    fn with_string(ty: Ty, string: Vec<u8>) -> Tok {
        Tok { ty, value: string.len() as f64, string }
    }
}

#[derive(Default)]
struct St {
    buf: Vec<u8>,
    pos: usize,
    streams: VecDeque<Vec<u8>>,
    operands: Vec<Tok>,
    /// 0 outside an inline image, 1 after `ID`, 2 after the image data
    inline: u8,
}

struct ScannerUd(Rc<RefCell<St>>);

impl UserDataTrait for ScannerUd {
    fn type_name(&self) -> &'static str {
        "pdfscanner"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn is_name_end(c: i32) -> bool {
    matches!(c, 0x20 | 0x0A | 0x0D | 0x09 | 0x2F | 0x5B | 0x28 | 0x3C)
}

impl St {
    fn next_stream(&mut self) {
        if let Some(s) = self.streams.pop_front() {
            self.buf = s;
            self.pos = 0;
        }
    }

    fn get(&mut self) -> i32 {
        let mut i = EOF;
        if self.pos < self.buf.len() {
            i = i32::from(self.buf[self.pos] as i8);
            self.pos += 1;
        }
        if i < 0 && !self.streams.is_empty() {
            self.next_stream();
            i = self.get();
        }
        i
    }

    fn look(&mut self) -> i32 {
        let mut i = EOF;
        if self.pos < self.buf.len() {
            i = i32::from(self.buf[self.pos] as i8);
        }
        if i < 0 && !self.streams.is_empty() {
            self.next_stream();
            i = self.get();
        }
        i
    }

    fn token(&mut self, mut c: i32) -> Option<Tok> {
        loop {
            if self.inline == 1 {
                self.inline = 2;
                return Some(self.inline_image(c));
            } else if self.inline == 2 {
                self.inline = 0;
                return Some(Tok::with_string(Ty::Operator, b"EI".to_vec()));
            }
            if c < 0 {
                return None;
            }
            return match c as u8 {
                b'(' => Some(self.string()),
                b')' => None,
                b'[' => Some(Tok::new(Ty::StartArray)),
                b']' => Some(Tok::new(Ty::StopArray)),
                b'/' => Some(self.name()),
                b'<' => {
                    let c = self.get();
                    if c == i32::from(b'<') {
                        Some(Tok::new(Ty::StartDict))
                    } else {
                        Some(self.hexstring(c))
                    }
                }
                b'>' => {
                    if self.get() == i32::from(b'>') {
                        Some(Tok::new(Ty::StopDict))
                    } else {
                        None
                    }
                }
                b'%' => {
                    loop {
                        c = self.get();
                        if c == i32::from(b'\n') || c == i32::from(b'\r') || c == -1 || c == EOF {
                            break;
                        }
                    }
                    c = self.get();
                    continue;
                }
                b' ' | b'\r' | b'\n' | b'\t' => {
                    c = self.get();
                    continue;
                }
                b'0'..=b'9' | b'-' | b'.' => Some(self.number(c)),
                _ => Some(self.operator(c)),
            };
        }
    }

    fn string(&mut self) -> Tok {
        let mut found = Vec::new();
        let mut level = 1;
        loop {
            let mut c = self.get();
            if c == EOF {
                break;
            }
            if c == i32::from(b'(') {
                level += 1;
            } else if c == i32::from(b')') {
                level -= 1;
                if level < 1 {
                    break;
                }
            } else if c == i32::from(b'\\') {
                let next = self.get();
                c = match u8::try_from(next).ok() {
                    Some(b'(' | b')' | b'\\') => next,
                    Some(b'\n' | b'\r') => 0,
                    Some(b'n') => 10,
                    Some(b'r') => 13,
                    Some(b't') => 9,
                    Some(b'b') => 8,
                    Some(b'f') => 12,
                    Some(d @ b'0'..=b'7') => {
                        let d1 = i32::from(d - b'0');
                        let n2 = self.look();
                        if (i32::from(b'0')..=i32::from(b'7')).contains(&n2) {
                            let d2 = self.get() - i32::from(b'0');
                            let n3 = self.look();
                            if (i32::from(b'0')..=i32::from(b'7')).contains(&n3) {
                                let d3 = self.get() - i32::from(b'0');
                                d1 * 64 + d2 * 8 + d3
                            } else {
                                d1 * 8 + d2
                            }
                        } else {
                            d1
                        }
                    }
                    _ => next,
                };
            }
            if c >= 0 {
                found.push(c as u8);
            }
        }
        Tok::with_string(Ty::Str, found)
    }

    fn number(&mut self, mut c: i32) -> Tok {
        let mut value = 0.0f64;
        let mut ty = Ty::Integer;
        let mut fraction = 0i32;
        let mut negative = false;
        if c == i32::from(b'-') {
            negative = true;
            c = self.get();
        }
        if c == i32::from(b'.') {
            ty = Ty::Real;
            fraction = 1;
        } else {
            value = f64::from(c - i32::from(b'0'));
        }
        c = self.look();
        let digit = |c: i32| (i32::from(b'0')..=i32::from(b'9')).contains(&c) || c == i32::from(b'.');
        if digit(c) {
            c = self.get();
            loop {
                if c == i32::from(b'.') {
                    ty = Ty::Real;
                    fraction = 1;
                } else {
                    let i = f64::from(c - i32::from(b'0'));
                    if fraction > 0 {
                        value += i / 10f64.powi(fraction);
                        fraction += 1;
                    } else {
                        value = value * 10.0 + i;
                    }
                }
                c = self.look();
                if !digit(c) {
                    break;
                }
                c = self.get();
            }
        }
        if negative {
            value = -value;
        }
        Tok { ty, value, string: Vec::new() }
    }

    fn name(&mut self) -> Tok {
        let mut found = Vec::new();
        let mut c = self.get();
        loop {
            if c == EOF {
                break;
            }
            found.push(c as u8);
            c = self.look();
            if is_name_end(c) || c == EOF {
                break;
            }
            c = self.get();
        }
        Tok::with_string(Ty::Name, found)
    }

    fn hexstring(&mut self, mut c: i32) -> Tok {
        let mut found = Vec::new();
        let mut odd = true;
        let mut hexval = 0i32;
        while c != i32::from(b'>') && c != EOF {
            if let Some(v) = u8::try_from(c).ok().and_then(|b| char::from(b).to_digit(16)) {
                if odd {
                    hexval = 16 * v as i32;
                } else {
                    hexval += v as i32;
                    found.push(hexval as u8);
                }
                odd = !odd;
            }
            c = self.get();
        }
        Tok::with_string(Ty::Str, found)
    }

    fn inline_image(&mut self, mut c: i32) -> Tok {
        let mut found: Vec<u8> = Vec::new();
        if c == i32::from(b' ') {
            c = self.get();
        }
        found.push(c as u8);
        let is_space = |c: i32| matches!(c, 0 | 0x20 | 0x0A | 0x0D | 0x09 | 0x0B);
        loop {
            c = self.look();
            if c == EOF {
                break;
            }
            let last = *found.last().unwrap_or(&0);
            if c == i32::from(b'E') && (last == b'\n' || last == b'\r') {
                c = self.get();
                found.push(c as u8);
                c = self.look();
                if c == i32::from(b'I') {
                    c = self.get();
                    found.push(c as u8);
                    c = self.look();
                    if is_space(c) {
                        found.pop();
                        found.pop();
                        if found.last() == Some(&b'\n') {
                            found.pop();
                        }
                        if found.last() == Some(&b'\r') {
                            found.pop();
                        }
                        break;
                    }
                    c = self.get();
                    found.push(c as u8);
                } else {
                    c = self.get();
                    found.push(c as u8);
                }
            } else {
                c = self.get();
                found.push(c as u8);
            }
        }
        Tok::with_string(Ty::Str, found)
    }

    fn operator(&mut self, mut c: i32) -> Tok {
        let mut found = Vec::new();
        loop {
            found.push(c as u8);
            c = self.look();
            if c < 0 || is_name_end(c) {
                break;
            }
            c = self.get();
        }
        if found == b"ID" {
            self.inline = 1;
        }
        match found.as_slice() {
            b"false" => Tok { ty: Ty::Boolean, value: 0.0, string: Vec::new() },
            b"true" => Tok { ty: Ty::Boolean, value: 1.0, string: Vec::new() },
            _ => Tok::with_string(Ty::Operator, found),
        }
    }

    /// `operandstack_backup`: the index of the start token of the array or
    /// dictionary that ends at the top (the tokens stay on the stack).
    fn backup(&self) -> Option<usize> {
        let top = self.operands.last()?;
        let (stop, start) = match top.ty {
            Ty::StopDict => (Ty::StopDict, Ty::StartDict),
            Ty::StopArray => (Ty::StopArray, Ty::StartArray),
            _ => return Some(self.operands.len() - 1),
        };
        let mut balance = 0;
        let mut i = self.operands.len();
        while i > 0 {
            i -= 1;
            if self.operands[i].ty == stop {
                balance += 1;
            } else if self.operands[i].ty == start {
                balance -= 1;
            }
            if balance == 0 {
                return Some(i);
            }
        }
        None
    }
}

fn type_name(ty: Ty) -> &'static str {
    match ty {
        Ty::Integer => "integer",
        Ty::Real => "real",
        Ty::Boolean => "boolean",
        Ty::Name => "name",
        Ty::Operator => "operator",
        Ty::Str => "string",
        Ty::StartArray | Ty::StopArray => "array",
        Ty::StartDict | Ty::StopDict => "dict",
    }
}

/// The Lua stack the C code builds its tables on. `push_token` and the two
/// helpers recurse over the operand stack exactly like lpdfscannerlib.c, including
/// its unbalanced handling of nested arrays and dictionaries (an outer container
/// never sees its own end token, so it swallows the rest of the operands and may
/// leave a stray key on the Lua stack); the library returns whatever is on top.
struct Build<'a, 'b> {
    cb: &'a mut CallbackLua<'b>,
    ops: &'a [Tok],
    /// `_nextoperand`
    next: usize,
    stack: Vec<Value>,
}

impl Build<'_, '_> {
    fn fetch(&mut self) -> Option<Tok> {
        let t = self.ops.get(self.next).cloned();
        self.next += 1;
        t
    }

    fn new_table(&mut self) -> LuaResult<LuaTable> {
        let t = self.cb.create_table()?;
        let v = self.cb.pack(t.clone())?;
        self.stack.push(v);
        Ok(t)
    }

    fn push_str(&mut self, bytes: &[u8]) -> LuaResult<()> {
        let s = self.cb.create_bytes(bytes)?;
        let v = self.cb.pack(s)?;
        self.stack.push(v);
        Ok(())
    }

    /// `lua_rawseti(L, -2, n)`
    fn rawseti(&mut self, n: i64) -> LuaResult<()> {
        let v = self.stack.pop();
        if let (Some(v), Some(t)) = (v, self.stack.last().and_then(Value::as_table)) {
            t.raw_seti(n, v)?;
        }
        Ok(())
    }

    fn push_token(&mut self) -> LuaResult<()> {
        let Some(tok) = self.ops.get(self.next.wrapping_sub(1)).cloned() else { return Ok(()) };
        self.new_table()?;
        self.push_str(type_name(tok.ty).as_bytes())?;
        self.rawseti(1)?;
        match tok.ty {
            Ty::Str | Ty::Name => self.push_str(&tok.string)?,
            Ty::Real | Ty::Integer => {
                let v = self.cb.pack(tok.value)?;
                self.stack.push(v);
            }
            Ty::Boolean => {
                let v = self.cb.pack(tok.value != 0.0)?;
                self.stack.push(v);
            }
            Ty::StartArray => self.push_array()?,
            Ty::StartDict => self.push_dict()?,
            _ => {
                let v = self.cb.pack(Option::<i64>::None)?;
                self.stack.push(v);
            }
        }
        self.rawseti(2)
    }

    fn push_array(&mut self) -> LuaResult<()> {
        let mut balance = 1;
        let mut index = 1;
        self.new_table()?;
        let mut token = self.fetch();
        while let Some(t) = token {
            match t.ty {
                Ty::StopArray => balance -= 1,
                Ty::StartArray => balance += 1,
                _ => {}
            }
            if balance == 0 {
                break;
            }
            self.push_token()?;
            self.rawseti(index)?;
            index += 1;
            token = self.fetch();
        }
        Ok(())
    }

    fn push_dict(&mut self) -> LuaResult<()> {
        let mut balance = 1;
        let mut needs_key = true;
        self.new_table()?;
        let mut token = self.fetch();
        while let Some(t) = token {
            match t.ty {
                Ty::StopDict => balance -= 1,
                Ty::StartDict => balance += 1,
                _ => {}
            }
            if balance == 0 {
                break;
            } else if needs_key {
                self.push_str(&t.string)?;
                needs_key = false;
            } else {
                self.push_token()?;
                needs_key = true;
                // lua_rawset(L, -3)
                let v = self.stack.pop();
                let k = self.stack.pop();
                if let (Some(v), Some(k), Some(t)) = (v, k, self.stack.last().and_then(Value::as_table)) {
                    t.raw_set(k, v)?;
                }
            }
            token = self.fetch();
        }
        Ok(())
    }
}

fn state(cb: &mut CallbackLua<'_>) -> LuaResult<Rc<RefCell<St>>> {
    let ok = cb.arg_count() >= 1 && cb.arg_kind(1) == Some(LuaValueKind::Userdata);
    if ok {
        if let Some(ud) = cb.arg::<Value>(1)?.as_userdata::<ScannerUd>() {
            return Ok(ud.borrow()?.0.clone());
        }
    }
    Err(cb.arg_error(1, "pdfscanner expected"))
}

/// `scanner_popsingular`: the top operand when it has type `ty`.
fn pop_singular(cb: &mut CallbackLua<'_>, st: &Rc<RefCell<St>>, ty: Ty) -> LuaResult<Option<Value>> {
    let mut s = st.borrow_mut();
    let Some(top) = s.operands.last() else { return Ok(None) };
    if top.ty != ty {
        return Ok(None);
    }
    let value = match ty {
        Ty::StopArray | Ty::StopDict => {
            let Some(clear) = s.backup() else { return Ok(None) };
            let ops = s.operands.clone();
            let mut b = Build { cb, ops: &ops, next: clear + 1, stack: Vec::new() };
            b.push_token()?;
            // lua_rawgeti(L, -1, 2): the table on top holds the value
            let inner = match b.stack.last().and_then(Value::as_table) {
                Some(t) => t.raw_geti::<Value>(2)?,
                None => b.cb.pack(Option::<i64>::None)?,
            };
            s.operands.truncate(clear);
            return Ok(Some(inner));
        }
        Ty::Real | Ty::Integer => cb.pack(top.value)?,
        Ty::Boolean => cb.pack(top.value != 0.0)?,
        Ty::Name | Ty::Str => {
            let b = cb.create_bytes(&top.string)?;
            cb.pack(b)?
        }
        _ => return Ok(None),
    };
    let clear = s.operands.len() - 1;
    s.operands.truncate(clear);
    Ok(Some(value))
}

fn pop_to(cb: &mut CallbackLua<'_>, tys: &[Ty]) -> LuaResult<usize> {
    let st = state(cb)?;
    for ty in tys {
        if let Some(v) = pop_singular(cb, &st, *ty)? {
            return cb.push(v);
        }
    }
    cb.push(Option::<i64>::None)
}

fn pop_any(cb: &mut CallbackLua<'_>) -> LuaResult<usize> {
    let st = state(cb)?;
    let mut s = st.borrow_mut();
    if s.operands.is_empty() {
        drop(s);
        return cb.push(Option::<i64>::None);
    }
    let Some(clear) = s.backup() else {
        drop(s);
        return cb.push(Option::<i64>::None);
    };
    let ops = s.operands.clone();
    let mut b = Build { cb, ops: &ops, next: clear + 1, stack: Vec::new() };
    b.push_token()?;
    let top = b.stack.pop();
    s.operands.truncate(clear);
    drop(s);
    match top {
        Some(v) => cb.push(v),
        None => cb.push(Option::<i64>::None),
    }
}

/// The decoded data of a pdfe stream userdata (`ppstream_all`).
fn stream_data(v: &Value) -> Option<Vec<u8>> {
    let (doc, stream) = crate::lua_pdfe::stream_of(v)?;
    Some(doc.stream_data(&stream, true).unwrap_or_default())
}

/// `luaL_checkudata(L, 1, "luatex.pdfe.stream")` failing: only a stream is accepted.
fn not_a_stream(cb: &mut CallbackLua<'_>, v: &Value) -> tex_lua::LuaError {
    let got = v.get_metatable().and_then(|m| m.get::<Option<String>>("__name").ok().flatten()).unwrap_or_else(|| "userdata".to_string());
    cb.arg_error(1, &format!("luatex.pdfe.stream expected, got {got}"))
}

fn scan(cb: &mut CallbackLua<'_>, mt: &LuaTable) -> LuaResult<usize> {
    if cb.arg_count() != 3 {
        return Ok(0);
    }
    for i in [2usize, 3] {
        if cb.arg_kind(i) != Some(LuaValueKind::Table) {
            return Err(cb.type_error(i, "table"));
        }
    }
    let ops: LuaTable = cb.arg(2)?;
    let info: Value = cb.arg(3)?;
    let source: Value = cb.arg(1)?;
    let mut st = St::default();
    match cb.arg_kind(1) {
        Some(LuaValueKind::String) => {
            let s: tex_lua::LuaString = cb.arg(1)?;
            st.buf = crate::lua_bridge::bytes_of(&s);
        }
        Some(LuaValueKind::Table) => {
            let t: LuaTable = cb.arg(1)?;
            let mut i = 1;
            loop {
                let v: Value = t.raw_geti(i)?;
                if v.kind() != LuaValueKind::Userdata {
                    break;
                }
                match stream_data(&v) {
                    Some(data) => st.streams.push_back(data),
                    None => return Err(not_a_stream(cb, &v)),
                }
                i += 1;
            }
            if let Some(first) = st.streams.pop_front() {
                st.buf = first;
            }
        }
        Some(LuaValueKind::Userdata) => match stream_data(&source) {
            Some(data) => st.buf = data,
            None => return Err(not_a_stream(&mut *cb, &source)),
        },
        _ => return Err(cb.type_error(1, "userdata")),
    }
    let shared = Rc::new(RefCell::new(st));
    let ud = cb.create_userdata(ScannerUd(shared.clone()))?;
    let scanner = cb.pack(ud)?;
    scanner.set_metatable(Some(mt))?;
    let mut token = {
        let mut s = shared.borrow_mut();
        let c = s.get();
        s.token(c)
    };
    while let Some(tok) = token {
        if tok.ty == Ty::Operator {
            let name = cb.create_bytes(&tok.string)?;
            let f: Value = ops.raw_get(name)?;
            if let Some(f) = f.as_function() {
                f.call::<_, ()>((scanner.clone(), info.clone()))?;
            }
            shared.borrow_mut().operands.clear();
        } else {
            shared.borrow_mut().operands.push(tok);
        }
        token = {
            let mut s = shared.borrow_mut();
            let c = s.get();
            s.token(c)
        };
    }
    shared.borrow_mut().operands.clear();
    Ok(0)
}

pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let e = |e: tex_lua::LuaError| format!("pdfscanner: {e:?}");
    let mt = lua.create_table().map_err(e)?;
    mt.set("__name", "pdfscanner").map_err(e)?;
    mt.set("__index", mt.clone()).map_err(e)?;
    macro_rules! method {
        ($name:literal, $f:expr) => {
            mt.set($name, lua.create_callback($f).map_err(e)?).map_err(e)?
        };
    }
    method!("done", |cb| {
        let st = state(cb)?;
        let mut s = st.borrow_mut();
        while s.get() >= 0 {}
        Ok(0)
    });
    method!("pop", pop_any);
    for name in ["popnumber", "popNumber"] {
        mt.set(name, lua.create_callback(|cb| pop_to(cb, &[Ty::Real, Ty::Integer])).map_err(e)?).map_err(e)?;
    }
    for (names, ty) in [
        (["popname", "popName"], Ty::Name),
        (["popstring", "popString"], Ty::Str),
        (["poparray", "popArray"], Ty::StopArray),
        (["popdictionary", "popDict"], Ty::StopDict),
        (["popboolean", "popBool"], Ty::Boolean),
    ] {
        for name in names {
            mt.set(name, lua.create_callback(move |cb| pop_to(cb, &[ty])).map_err(e)?).map_err(e)?;
        }
    }
    lua.registry_set("pdfscanner", &mt).map_err(e)?;
    let t = lua.create_table().map_err(e)?;
    let scan_mt = mt.clone();
    t.set("scan", lua.create_callback(move |cb| scan(cb, &scan_mt)).map_err(e)?).map_err(e)?;
    lua.set_global("pdfscanner", t).map_err(e)?;
    lua.load("package.loaded.pdfscanner = pdfscanner").exec().map_err(|err| format!("pdfscanner: {}", lua.get_error_message(err).message()))
}
