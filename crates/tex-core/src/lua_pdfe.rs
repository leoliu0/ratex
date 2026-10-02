//! The `pdfe` library (lpdfelib.c): read access to PDF files and streams.
//!
//! Dictionaries, arrays, streams and references are userdata, everything else
//! is plain Lua. The userdata carry the document so they stay usable after
//! the document is closed. Return conventions and the quirks of lpdfelib.c
//! (0-based indices in the `get<type>` family, `openstream(s, false)`
//! decoding, resolved values in `__index`, ...) follow LuaTeX 1.24.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use tex_lua::{CallbackLua, Lua, LuaApi, LuaResult, LuaString, LuaTable, LuaValueKind, UserDataRef, UserDataTrait, Value};

use crate::lua_bridge::{bytes_of, with_engine};
use crate::pdfe_doc::{Doc, Obj, PDict, PStream, T_ARRAY, T_DICT, T_STREAM};

macro_rules! plain_userdata {
    ($ty:ident, $name:literal) => {
        impl UserDataTrait for $ty {
            fn type_name(&self) -> &'static str {
                $name
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn Any {
                self
            }
        }
    };
}

/// `pdfe` document; `None` once closed.
struct DocUd(Option<Rc<Doc>>);
struct DictUd {
    doc: Rc<Doc>,
    dict: Rc<PDict>,
}
struct ArrayUd {
    doc: Rc<Doc>,
    array: Rc<Vec<Obj>>,
}
struct StreamUd {
    doc: Rc<Doc>,
    stream: Rc<PStream>,
    /// 0 closed, 1 opened, 2 reading.
    open: u8,
    decode: bool,
    data: Vec<u8>,
    pos: usize,
}
struct RefUd {
    doc: Rc<Doc>,
    onum: u32,
}
plain_userdata!(DocUd, "luatex.pdfe");
plain_userdata!(DictUd, "luatex.pdfe.dictionary");
plain_userdata!(ArrayUd, "luatex.pdfe.array");
plain_userdata!(StreamUd, "luatex.pdfe.stream");
plain_userdata!(RefUd, "luatex.pdfe.reference");

/// The streaming chunk of pplib's buffers.
const CHUNK: usize = 0x40000;

#[derive(Clone)]
struct Mts {
    doc: LuaTable,
    dict: LuaTable,
    array: LuaTable,
    stream: LuaTable,
    reference: LuaTable,
}
type Shared = Rc<RefCell<Option<Mts>>>;

/// LuaTeX's `normal_warning`: `warning  (what): message` on its own line, on
/// the terminal and in the log.
fn warning(what: &str, message: &str) {
    let _ = with_engine(|e| e.lua_texio_print(3, true, format!("warning  ({what}): {message}\n").as_bytes()));
}

fn warn(message: &str) {
    warning("pdfe lib", message);
}

fn log_all(doc: &Doc) {
    for message in doc.take_log() {
        warning("pdfe", &message);
    }
}

fn mts(sh: &Shared) -> Mts {
    sh.borrow().clone().expect("pdfe metatables")
}

fn nil(cb: &mut CallbackLua<'_>) -> LuaResult<Value> {
    cb.pack(Option::<i64>::None)
}

fn mk_doc(cb: &mut CallbackLua<'_>, sh: &Shared, doc: Rc<Doc>) -> LuaResult<Value> {
    let ud = cb.create_userdata(DocUd(Some(doc)))?;
    let v = cb.pack(ud)?;
    v.set_metatable(Some(&mts(sh).doc))?;
    Ok(v)
}

fn mk_dict(cb: &mut CallbackLua<'_>, sh: &Shared, doc: &Rc<Doc>, dict: &Rc<PDict>) -> LuaResult<Value> {
    let ud = cb.create_userdata(DictUd { doc: doc.clone(), dict: dict.clone() })?;
    let v = cb.pack(ud)?;
    v.set_metatable(Some(&mts(sh).dict))?;
    Ok(v)
}

fn mk_array(cb: &mut CallbackLua<'_>, sh: &Shared, doc: &Rc<Doc>, array: &Rc<Vec<Obj>>) -> LuaResult<Value> {
    let ud = cb.create_userdata(ArrayUd { doc: doc.clone(), array: array.clone() })?;
    let v = cb.pack(ud)?;
    v.set_metatable(Some(&mts(sh).array))?;
    Ok(v)
}

fn mk_stream(cb: &mut CallbackLua<'_>, sh: &Shared, doc: &Rc<Doc>, stream: &Rc<PStream>) -> LuaResult<Value> {
    let ud = cb.create_userdata(StreamUd { doc: doc.clone(), stream: stream.clone(), open: 0, decode: false, data: Vec::new(), pos: 0 })?;
    let v = cb.pack(ud)?;
    v.set_metatable(Some(&mts(sh).stream))?;
    Ok(v)
}

fn mk_ref(cb: &mut CallbackLua<'_>, sh: &Shared, doc: &Rc<Doc>, onum: u32) -> LuaResult<Value> {
    let ud = cb.create_userdata(RefUd { doc: doc.clone(), onum })?;
    let v = cb.pack(ud)?;
    v.set_metatable(Some(&mts(sh).reference))?;
    Ok(v)
}

/// `pushvalue`: the Lua values an object pushes (nil, boolean, integer,
/// number, name, string plus hex flag, array plus size, dictionary plus size,
/// stream plus dictionary plus size, reference plus number).
fn pushvalue(cb: &mut CallbackLua<'_>, sh: &Shared, doc: &Rc<Doc>, o: &Obj) -> LuaResult<Vec<Value>> {
    Ok(match o {
        Obj::None | Obj::Null => vec![nil(cb)?],
        Obj::Bool(b) => vec![cb.pack(*b)?],
        Obj::Int(i) => vec![cb.pack(*i)?],
        Obj::Num(f) => vec![cb.pack(*f)?],
        Obj::Name(n) => {
            let s = cb.create_bytes(&n.dec)?;
            vec![cb.pack(s)?]
        }
        Obj::Str(s) => {
            let b = cb.create_bytes(&s.enc)?;
            vec![cb.pack(b)?, cb.pack(s.hex)?]
        }
        Obj::Array(a) => vec![mk_array(cb, sh, doc, a)?, cb.pack(a.len() as i64)?],
        Obj::Dict(d) => vec![mk_dict(cb, sh, doc, d)?, cb.pack(d.len() as i64)?],
        Obj::Stream(s) => {
            vec![mk_stream(cb, sh, doc, s)?, mk_dict(cb, sh, doc, &s.dict)?, cb.pack(s.dict.len() as i64)?]
        }
        Obj::Ref(n) => {
            if *n == 0 {
                vec![]
            } else {
                vec![mk_ref(cb, sh, doc, *n)?, cb.pack(i64::from(*n))?]
            }
        }
    })
}

fn push_all(cb: &mut CallbackLua<'_>, values: Vec<Value>) -> LuaResult<usize> {
    let mut n = 0;
    for v in values {
        n += cb.push(v)?;
    }
    Ok(n)
}

/// `type, value...` as returned by the `getfrom*` functions.
fn typed(cb: &mut CallbackLua<'_>, sh: &Shared, doc: &Rc<Doc>, o: &Obj, key: Option<&[u8]>) -> LuaResult<usize> {
    let mut out = Vec::new();
    if let Some(k) = key {
        let s = cb.create_bytes(k)?;
        out.push(cb.pack(s)?);
    }
    out.push(cb.pack(o.type_code())?);
    out.extend(pushvalue(cb, sh, doc, o)?);
    push_all(cb, out)
}

fn ud_arg<T: 'static>(cb: &mut CallbackLua<'_>, i: usize) -> Option<UserDataRef<T>> {
    if cb.arg_count() < i {
        return None;
    }
    cb.arg::<Value>(i).ok()?.as_userdata::<T>()
}

fn doc_arg(cb: &mut CallbackLua<'_>, i: usize) -> Option<Rc<Doc>> {
    match ud_arg::<DocUd>(cb, i) {
        Some(d) => d.borrow().ok().and_then(|d| d.0.clone()),
        None => {
            warn("lua <pdfe document> expected");
            None
        }
    }
}

fn dict_arg(cb: &mut CallbackLua<'_>, i: usize) -> Option<(Rc<Doc>, Rc<PDict>)> {
    match ud_arg::<DictUd>(cb, i).and_then(|d| d.borrow().ok().map(|d| (d.doc.clone(), d.dict.clone()))) {
        Some(x) => Some(x),
        None => {
            warn("lua <pdfe dictionary> expected");
            None
        }
    }
}

fn array_arg(cb: &mut CallbackLua<'_>, i: usize) -> Option<(Rc<Doc>, Rc<Vec<Obj>>)> {
    match ud_arg::<ArrayUd>(cb, i).and_then(|d| d.borrow().ok().map(|d| (d.doc.clone(), d.array.clone()))) {
        Some(x) => Some(x),
        None => {
            warn("lua <pdfe array> expected");
            None
        }
    }
}

fn stream_arg(cb: &mut CallbackLua<'_>, i: usize) -> Option<UserDataRef<StreamUd>> {
    match ud_arg::<StreamUd>(cb, i) {
        Some(s) => Some(s),
        None => {
            warn("lua <pdfe stream> expected");
            None
        }
    }
}

/// `luaL_checkstring`: strings and numbers.
fn bytes_arg(cb: &mut CallbackLua<'_>, i: usize) -> LuaResult<Vec<u8>> {
    match cb.arg_kind(i) {
        Some(LuaValueKind::String) => {
            let s: LuaString = cb.arg(i)?;
            Ok(bytes_of(&s))
        }
        Some(LuaValueKind::Integer) => Ok(cb.arg::<i64>(i)?.to_string().into_bytes()),
        Some(LuaValueKind::Float) => {
            let f: f64 = cb.arg(i)?;
            Ok(if f.fract() == 0.0 && f.abs() < 1e15 { format!("{f:.1}") } else { format!("{f}") }.into_bytes())
        }
        _ => Err(cb.type_error(i, "string")),
    }
}

/// `lua_tointeger`: integral numbers, 0 for anything else.
fn lua_tointeger(cb: &mut CallbackLua<'_>, i: usize) -> i64 {
    match cb.arg_kind(i) {
        Some(LuaValueKind::Integer) => cb.arg::<i64>(i).unwrap_or(0),
        Some(LuaValueKind::Float) => cb.arg::<f64>(i).map_or(0, |f| if f.is_finite() && f.fract() == 0.0 { f as i64 } else { 0 }),
        _ => 0,
    }
}

fn dict_get<'a>(d: &'a PDict, key: &[u8]) -> Option<&'a Obj> {
    d.get(key)
}

/// The `totable` collector entry: `{ type, value, extra... }`, or the value
/// itself when `flat` and the object pushes a single value.
fn totable(cb: &mut CallbackLua<'_>, sh: &Shared, doc: &Rc<Doc>, o: &Obj, flat: bool) -> LuaResult<Value> {
    let vals = pushvalue(cb, sh, doc, o)?;
    if flat && vals.len() < 2 {
        return match vals.into_iter().next() {
            Some(v) => Ok(v),
            None => nil(cb),
        };
    }
    let t = cb.create_table()?;
    t.raw_seti(1, o.type_code())?;
    for (i, v) in vals.into_iter().enumerate() {
        t.raw_seti(i as i64 + 2, v)?;
    }
    cb.pack(t)
}

/// The object a `get<type>` call addresses (the `pdfelib_get_value_*`
/// macros): a dictionary (or reference to one) with a name key, an array (or
/// reference to one) with a 0-based index.
fn get_value(cb: &mut CallbackLua<'_>) -> LuaResult<Option<(Rc<Doc>, Obj)>> {
    if cb.arg_count() <= 1 {
        return Ok(None);
    }
    let a1 = cb.arg::<Value>(1)?;
    let k2 = cb.arg_kind(2);
    if !matches!(a1.kind(), LuaValueKind::Userdata) && !a1.is_lightuserdata() {
        match k2 {
            Some(LuaValueKind::String) => warn("lua <pdfe dictionary> expected"),
            Some(LuaValueKind::Integer | LuaValueKind::Float) => warn("lua <pdfe array> expected"),
            _ => warn("invalid arguments"),
        }
        return Ok(None);
    }
    match k2 {
        Some(LuaValueKind::String) => {
            let key = bytes_arg(cb, 2)?;
            if let Some(d) = a1.as_userdata::<DictUd>() {
                let d = d.borrow()?;
                return Ok(d.dict.get(&key).map(|o| (d.doc.clone(), d.doc.resolve(o))));
            }
            if let Some(r) = a1.as_userdata::<RefUd>() {
                let r = r.borrow()?;
                if let Some(Obj::Dict(d)) = r.doc.find(r.onum) {
                    return Ok(d.get(&key).map(|o| (r.doc.clone(), r.doc.resolve(o))));
                }
            }
            Ok(None)
        }
        Some(LuaValueKind::Integer | LuaValueKind::Float) => {
            let index = lua_tointeger(cb, 2);
            let at = |doc: &Rc<Doc>, a: &Vec<Obj>| usize::try_from(index).ok().and_then(|i| a.get(i)).map(|o| (doc.clone(), doc.resolve(o)));
            if let Some(a) = a1.as_userdata::<ArrayUd>() {
                let a = a.borrow()?;
                return Ok(at(&a.doc, &a.array));
            }
            if let Some(r) = a1.as_userdata::<RefUd>() {
                let r = r.borrow()?;
                if let Some(Obj::Array(a)) = r.doc.find(r.onum) {
                    return Ok(at(&r.doc, &a));
                }
            }
            Ok(None)
        }
        _ => {
            warn("second argument should be integer or string");
            Ok(None)
        }
    }
}

fn read_input(cb: &mut CallbackLua<'_>) -> LuaResult<Vec<u8>> {
    let v = cb.arg::<Value>(1)?;
    if !v.is_lightuserdata() && v.kind() != LuaValueKind::String {
        return Err(cb.error("bad <pdfe> argument: string or lightuserdata expected"));
    }
    let len: i64 = cb.arg(2)?;
    let len = len.max(0) as usize;
    let mut buf = if v.is_lightuserdata() {
        let p = v.as_lightuserdata().unwrap_or(std::ptr::null_mut());
        if p.is_null() {
            return Err(cb.error("bad <pdfe> document"));
        }
        // SAFETY: the caller promises `len` readable bytes, as for lpdfelib.c.
        unsafe { std::slice::from_raw_parts(p as *const u8, len).to_vec() }
    } else if v.kind() == LuaValueKind::String {
        let mut b = bytes_arg(cb, 1)?;
        b.resize(len, 0);
        b
    } else {
        return Err(cb.error("bad <pdfe> argument: string or lightuserdata expected"));
    };
    buf.truncate(len);
    Ok(buf)
}

/// djb2 over signed chars, as `get_stream_checksum` in pdftoepdf.c.
fn stream_checksum(data: &[u8]) -> String {
    let mut hash: u64 = 5381;
    for &b in data {
        hash = hash.wrapping_shl(5).wrapping_add(hash).wrapping_add(b as i8 as i64 as u64);
    }
    format!("{hash:x}")
}

pub(crate) const STREAM_URI: &str = "data:application/pdf,";

macro_rules! reg {
    ($lua:expr, $tbl:expr, $name:literal, $f:expr) => {
        $tbl.set($name, $lua.create_callback($f).map_err(|e| format!("{}: {e:?}", $name))?)
            .map_err(|e| format!("{}: {e:?}", $name))?
    };
}

macro_rules! simple_get {
    ($lua:expr, $t:expr, $name:literal, |$doc:ident| $body:expr) => {
        reg!($lua, $t, $name, move |cb| {
            let Some($doc) = doc_arg(cb, 1) else { return Ok(0) };
            $body(cb)
        })
    };
}

pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let e = |e: tex_lua::LuaError| format!("pdfe: {e:?}");
    let sh: Shared = Rc::new(RefCell::new(None));
    let new_mt = |lua: &mut Lua| -> Result<LuaTable, String> { lua.create_table().map_err(e) };
    let mt = Mts { doc: new_mt(lua)?, dict: new_mt(lua)?, array: new_mt(lua)?, stream: new_mt(lua)?, reference: new_mt(lua)? };
    for (table, name) in [
        (&mt.doc, "luatex.pdfe"),
        (&mt.dict, "luatex.pdfe.dictionary"),
        (&mt.array, "luatex.pdfe.array"),
        (&mt.stream, "luatex.pdfe.stream"),
        (&mt.reference, "luatex.pdfe.reference"),
    ] {
        table.set("__name", name).map_err(e)?;
    }
    *sh.borrow_mut() = Some(mt.clone());
    let t = lua.create_table().map_err(e)?;

    // ---- type, tostring, length, index metamethods ----
    reg!(lua, t, "type", |cb| {
        let v = cb.arg::<Value>(0 + 1)?;
        let name = if v.as_userdata::<DocUd>().is_some() {
            "pdfe"
        } else if v.as_userdata::<DictUd>().is_some() {
            "pdfe.dictionary"
        } else if v.as_userdata::<ArrayUd>().is_some() {
            "pdfe.array"
        } else if v.as_userdata::<RefUd>().is_some() {
            "pdfe.reference"
        } else if v.as_userdata::<StreamUd>().is_some() {
            "pdfe.stream"
        } else {
            return Ok(0);
        };
        cb.push(name)
    });

    fn ptr<T: ?Sized>(p: *const T) -> String {
        if p.is_null() {
            "(nil)".to_string()
        } else {
            format!("{:#x}", p as *const u8 as usize)
        }
    }
    let tostr = |lua: &mut Lua, mt: &LuaTable, f: fn(&Value) -> Option<String>| -> Result<(), String> {
        let func = lua
            .create_callback(move |cb| {
                let v = cb.arg::<Value>(1)?;
                match f(&v) {
                    Some(s) => cb.push(s),
                    None => Ok(0),
                }
            })
            .map_err(e)?;
        mt.set("__tostring", func).map_err(e)
    };
    tostr(lua, &mt.doc, |v| {
        let d = v.as_userdata::<DocUd>()?;
        let d = d.borrow().ok()?;
        Some(format!("<pdfe {}>", d.0.as_ref().map_or_else(|| "(nil)".to_string(), |d| ptr(Rc::as_ptr(d)))))
    })?;
    tostr(lua, &mt.dict, |v| {
        let d = v.as_userdata::<DictUd>()?;
        let d = d.borrow().ok()?;
        Some(format!("<pdfe.dictionary {}>", ptr(Rc::as_ptr(&d.dict))))
    })?;
    tostr(lua, &mt.array, |v| {
        let d = v.as_userdata::<ArrayUd>()?;
        let d = d.borrow().ok()?;
        Some(format!("<pdfe.array {}>", ptr(Rc::as_ptr(&d.array))))
    })?;
    tostr(lua, &mt.stream, |v| {
        let d = v.as_userdata::<StreamUd>()?;
        let d = d.borrow().ok()?;
        Some(format!("<pdfe.stream {}>", ptr(Rc::as_ptr(&d.stream))))
    })?;
    tostr(lua, &mt.reference, |v| {
        let d = v.as_userdata::<RefUd>()?;
        let d = d.borrow().ok()?;
        Some(format!("<pdfe.reference {}>", d.onum))
    })?;

    let len = |lua: &mut Lua, mt: &LuaTable, f: fn(&Value) -> Option<usize>| -> Result<(), String> {
        let func = lua
            .create_callback(move |cb| {
                let v = cb.arg::<Value>(1)?;
                match f(&v) {
                    Some(n) => cb.push(n as i64),
                    None => Ok(0),
                }
            })
            .map_err(e)?;
        mt.set("__len", func).map_err(e)
    };
    len(lua, &mt.dict, |v| Some(v.as_userdata::<DictUd>()?.borrow().ok()?.dict.len()))?;
    len(lua, &mt.array, |v| Some(v.as_userdata::<ArrayUd>()?.borrow().ok()?.array.len()))?;
    len(lua, &mt.stream, |v| Some(v.as_userdata::<StreamUd>()?.borrow().ok()?.stream.dict.len()))?;

    // `pdfelib_pushvalue`: a single value.
    fn single(cb: &mut CallbackLua<'_>, sh: &Shared, doc: &Rc<Doc>, o: &Obj) -> LuaResult<usize> {
        let vals = pushvalue(cb, sh, doc, o)?;
        match o {
            // the pplib quirk: a reference yields its number
            Obj::Ref(n) => {
                let v = if *n == 0 { nil(cb)? } else { cb.pack(i64::from(*n))? };
                cb.push(v)
            }
            _ => match vals.into_iter().next() {
                Some(v) => cb.push(v),
                None => cb.push(Option::<i64>::None),
            },
        }
    }
    {
        let sh = sh.clone();
        let f = lua
            .create_callback(move |cb| {
                let v = cb.arg::<Value>(1)?;
                let Some(d) = v.as_userdata::<DocUd>() else { return Ok(0) };
                let Some(doc) = d.borrow()?.0.clone() else { return Ok(0) };
                if cb.arg_kind(2) != Some(LuaValueKind::String) {
                    return Ok(0);
                }
                let key = bytes_arg(cb, 2)?;
                match key.as_slice() {
                    b"catalog" | b"Catalog" => match doc.catalog() {
                        Some(d) => {
                            let v = mk_dict(cb, &sh, &doc, &d)?;
                            cb.push(v)
                        }
                        None => Ok(0),
                    },
                    b"info" | b"Info" => match doc.info() {
                        Some(d) => {
                            let v = mk_dict(cb, &sh, &doc, &d)?;
                            cb.push(v)
                        }
                        None => Ok(0),
                    },
                    b"trailer" | b"Trailer" => {
                        let v = mk_dict(cb, &sh, &doc, &doc.trailer())?;
                        cb.push(v)
                    }
                    b"pages" | b"Pages" => {
                        let t = cb.create_table()?;
                        for (i, (_, d)) in doc.pages().iter().enumerate() {
                            let v = mk_dict(cb, &sh, &doc, d)?;
                            t.raw_seti(i as i64 + 1, v)?;
                        }
                        cb.push(t)
                    }
                    _ => Ok(0),
                }
            })
            .map_err(e)?;
        mt.doc.set("__index", f).map_err(e)?;
    }
    {
        let sh = sh.clone();
        let f = lua
            .create_callback(move |cb| {
                let v = cb.arg::<Value>(1)?;
                let Some(a) = v.as_userdata::<ArrayUd>() else { return Ok(0) };
                let (doc, arr) = {
                    let a = a.borrow()?;
                    (a.doc.clone(), a.array.clone())
                };
                if !matches!(cb.arg_kind(2), Some(LuaValueKind::Integer | LuaValueKind::Float)) {
                    return Ok(0);
                }
                let idx: i64 = cb.arg(2)?;
                match usize::try_from(idx - 1).ok().and_then(|i| arr.get(i)) {
                    Some(o) => {
                        let o = doc.resolve(o);
                        single(cb, &sh, &doc, &o)
                    }
                    None => Ok(0),
                }
            })
            .map_err(e)?;
        mt.array.set("__index", f).map_err(e)?;
    }
    fn dict_index(cb: &mut CallbackLua<'_>, sh: &Shared, doc: &Rc<Doc>, dict: &PDict) -> LuaResult<usize> {
        match cb.arg_kind(2) {
            Some(LuaValueKind::String) => {
                let key = bytes_arg(cb, 2)?;
                match dict.get(&key) {
                    Some(o) => {
                        let o = doc.resolve(o);
                        single(cb, sh, doc, &o)
                    }
                    None => Ok(0),
                }
            }
            Some(LuaValueKind::Integer | LuaValueKind::Float) => {
                let idx: i64 = cb.arg(2)?;
                match usize::try_from(idx - 1).ok().and_then(|i| dict.vals.get(i)) {
                    Some(o) => single(cb, sh, doc, &o.clone()),
                    None => Ok(0),
                }
            }
            _ => Ok(0),
        }
    }
    {
        let sh = sh.clone();
        let f = lua
            .create_callback(move |cb| {
                let v = cb.arg::<Value>(1)?;
                let Some(d) = v.as_userdata::<DictUd>() else { return Ok(0) };
                let (doc, dict) = {
                    let d = d.borrow()?;
                    (d.doc.clone(), d.dict.clone())
                };
                dict_index(cb, &sh, &doc, &dict)
            })
            .map_err(e)?;
        mt.dict.set("__index", f).map_err(e)?;
    }
    {
        let sh = sh.clone();
        let f = lua
            .create_callback(move |cb| {
                let v = cb.arg::<Value>(1)?;
                let Some(s) = v.as_userdata::<StreamUd>() else { return Ok(0) };
                let (doc, dict) = {
                    let s = s.borrow()?;
                    (s.doc.clone(), s.stream.dict.clone())
                };
                dict_index(cb, &sh, &doc, &dict)
            })
            .map_err(e)?;
        mt.stream.set("__index", f).map_err(e)?;
    }

    // ---- opening ----
    {
        let sh = sh.clone();
        reg!(lua, t, "open", move |cb| {
            let name = bytes_arg(cb, 1)?;
            #[cfg(unix)]
            let path = {
                use std::os::unix::ffi::OsStrExt;
                std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&name))
            };
            #[cfg(not(unix))]
            let path = std::path::PathBuf::from(String::from_utf8_lossy(&name).into_owned());
            let doc = std::fs::read(&path).ok().and_then(Doc::open);
            match doc {
                Some(doc) => {
                    log_all(&doc);
                    let v = mk_doc(cb, &sh, doc)?;
                    cb.push(v)
                }
                None => {
                    warn(&format!("no valid pdf file '{}'", String::from_utf8_lossy(&name)));
                    Ok(0)
                }
            }
        });
    }
    {
        let sh = sh.clone();
        reg!(lua, t, "new", move |cb| {
            let data = read_input(cb)?;
            if cb.arg_count() == 2 {
                match Doc::open(data) {
                    Some(doc) => {
                        log_all(&doc);
                        let v = mk_doc(cb, &sh, doc)?;
                        cb.push(v)
                    }
                    None => {
                        warn("no valid pdf mem stream");
                        Ok(0)
                    }
                }
            } else {
                let id = bytes_arg(cb, 3)?;
                if id.len() > 2048 {
                    return Err(cb.error("<pdfe> stream has a too long id"));
                }
                let mut path = STREAM_URI.as_bytes().to_vec();
                path.extend_from_slice(&id);
                path.extend_from_slice(stream_checksum(&data).as_bytes());
                let name = String::from_utf8_lossy(&path).into_owned();
                let registered = with_engine(|e| e.pdfe_register_memstream(name.clone(), data)).unwrap_or(false);
                if !registered {
                    return Err(cb.error("pdf inclusion: reading pdf Stream failed"));
                }
                let s = cb.create_bytes(&path)?;
                cb.push(s)
            }
        });
    }
    fn close(cb: &mut CallbackLua<'_>) -> LuaResult<usize> {
        if let Some(d) = ud_arg::<DocUd>(cb, 1) {
            d.borrow_mut()?.0 = None;
        } else {
            warn("lua <pdfe document> expected");
        }
        Ok(0)
    }
    reg!(lua, t, "close", close);
    {
        // pdfelib_free: the document's __gc releases it like `close`
        let f = lua.create_callback(close).map_err(e)?;
        mt.doc.set("__gc", f).map_err(e)?;
    }
    reg!(lua, t, "unencrypt", move |cb| {
        let doc = match ud_arg::<DocUd>(cb, 1) {
            Some(d) => d.borrow()?.0.clone(),
            None => {
                warn("lua <pdfe document> expected");
                None
            }
        };
        let top = cb.arg_count();
        if let (Some(doc), true) = (doc, top > 1) {
            let pass = |i: usize, cb: &mut CallbackLua<'_>| -> LuaResult<Option<Vec<u8>>> {
                if i <= top && cb.arg_kind(i) == Some(LuaValueKind::String) {
                    Ok(Some(bytes_arg(cb, i)?))
                } else {
                    Ok(None)
                }
            };
            let user = pass(2, cb)?;
            let owner = pass(3, cb)?;
            let status = doc.crypt_pass(user.as_deref(), owner.as_deref());
            log_all(&doc);
            return cb.push(i64::from(status));
        }
        cb.push(i64::from(crate::pdfe_doc::CRYPT_FAIL))
    });

    // ---- statistics ----
    simple_get!(lua, t, "getsize", |doc| |cb: &mut CallbackLua<'_>| cb.push(doc.size() as i64));
    simple_get!(lua, t, "getversion", |doc| |cb: &mut CallbackLua<'_>| {
        let (major, minor) = doc.version();
        cb.push((major, minor))
    });
    simple_get!(lua, t, "getstatus", |doc| |cb: &mut CallbackLua<'_>| cb.push(i64::from(doc.status.get())));
    simple_get!(lua, t, "getnofobjects", |doc| |cb: &mut CallbackLua<'_>| cb.push(doc.objects() as i64));
    simple_get!(lua, t, "getnofpages", |doc| |cb: &mut CallbackLua<'_>| cb.push(doc.page_count() as i64));
    simple_get!(lua, t, "getmemoryusage", |doc| |cb: &mut CallbackLua<'_>| cb.push((doc.memory_usage() as i64, 0i64)));

    // ---- starting points ----
    for (name, which) in [("getcatalog", 0), ("gettrailer", 1), ("getinfo", 2)] {
        let sh = sh.clone();
        let f = lua
            .create_callback(move |cb| {
                let Some(doc) = doc_arg(cb, 1) else { return Ok(0) };
                let d = match which {
                    0 => doc.catalog(),
                    1 => Some(doc.trailer()),
                    _ => doc.info(),
                };
                match d {
                    Some(d) => {
                        let v = mk_dict(cb, &sh, &doc, &d)?;
                        cb.push(v)
                    }
                    None => Ok(0),
                }
            })
            .map_err(e)?;
        t.set(name, f).map_err(e)?;
    }
    {
        let sh = sh.clone();
        reg!(lua, t, "getpage", move |cb| {
            let Some(doc) = doc_arg(cb, 1) else { return Ok(0) };
            let n: i64 = cb.arg(2)?;
            if n <= 0 || n as u64 > doc.page_count() {
                return Ok(0);
            }
            match doc.page(n as u64) {
                Some((_, d)) => {
                    let v = mk_dict(cb, &sh, &doc, &d)?;
                    cb.push(v)
                }
                None => Ok(0),
            }
        });
    }
    {
        let sh = sh.clone();
        reg!(lua, t, "getpages", move |cb| {
            let Some(doc) = doc_arg(cb, 1) else { return Ok(0) };
            let t = cb.create_table()?;
            for (i, (_, d)) in doc.pages().iter().enumerate() {
                let v = mk_dict(cb, &sh, &doc, d)?;
                t.raw_seti(i as i64 + 1, v)?;
            }
            cb.push(t)
        });
    }
    {
        let sh = sh.clone();
        reg!(lua, t, "pagestotable", move |cb| {
            let Some(doc) = doc_arg(cb, 1) else { return Ok(0) };
            let t = cb.create_table()?;
            for (i, (num, d)) in doc.pages().iter().enumerate() {
                let row = cb.create_table()?;
                let v = mk_dict(cb, &sh, &doc, d)?;
                row.raw_seti(1, v)?;
                row.raw_seti(2, d.len() as i64)?;
                row.raw_seti(3, i64::from(*num))?;
                t.raw_seti(i as i64 + 1, row)?;
            }
            cb.push(t)
        });
    }
    reg!(lua, t, "getbox", move |cb| {
        if cb.arg_count() > 1 && cb.arg_kind(2) == Some(LuaValueKind::String) {
            let Some((doc, dict)) = dict_arg(cb, 1) else { return Ok(0) };
            let key = bytes_arg(cb, 2)?;
            let key = String::from_utf8_lossy(&key).into_owned();
            if let Some(b) = doc.get_box(&dict, &key) {
                let t = cb.create_table()?;
                for (i, v) in b.iter().enumerate() {
                    t.raw_seti(i as i64 + 1, *v)?;
                }
                return cb.push(t);
            }
        }
        Ok(0)
    });

    // ---- indexed access ----
    {
        let sh = sh.clone();
        reg!(lua, t, "getfromreference", move |cb| {
            let r = match ud_arg::<RefUd>(cb, 1).and_then(|r| r.borrow().ok().map(|r| (r.doc.clone(), r.onum))) {
                Some(r) => r,
                None => {
                    warn("lua <pdfe reference> expected");
                    return Ok(0);
                }
            };
            match r.0.find(r.1) {
                Some(o) => typed(cb, &sh, &r.0, &o, None),
                None => Ok(0),
            }
        });
    }
    {
        let sh = sh.clone();
        reg!(lua, t, "getfromarray", move |cb| {
            let Some((doc, arr)) = array_arg(cb, 1) else { return Ok(0) };
            let index: i64 = cb.arg(2)?;
            match usize::try_from(index - 1).ok().and_then(|i| arr.get(i)) {
                Some(o) => typed(cb, &sh, &doc, o, None),
                None => Ok(0),
            }
        });
    }
    fn from_dict(cb: &mut CallbackLua<'_>, sh: &Shared, doc: &Rc<Doc>, d: &PDict) -> LuaResult<usize> {
        if cb.arg_kind(2) == Some(LuaValueKind::String) {
            let key = bytes_arg(cb, 2)?;
            return match dict_get(d, &key) {
                Some(o) => typed(cb, sh, doc, o, None),
                None => Ok(0),
            };
        }
        let index: i64 = cb.arg(2)?;
        match usize::try_from(index - 1).ok().filter(|&i| i < d.len()) {
            Some(i) => {
                let key = d.keys[i].dec.clone();
                typed(cb, sh, doc, &d.vals[i], Some(&key))
            }
            None => Ok(0),
        }
    }
    {
        let sh = sh.clone();
        reg!(lua, t, "getfromdictionary", move |cb| {
            let Some((doc, d)) = dict_arg(cb, 1) else { return Ok(0) };
            from_dict(cb, &sh, &doc, &d)
        });
    }
    {
        let sh = sh.clone();
        reg!(lua, t, "getfromstream", move |cb| {
            let Some((doc, dict)) = ud_arg::<StreamUd>(cb, 1).and_then(|s| s.borrow().ok().map(|s| (s.doc.clone(), s.stream.dict.clone()))) else {
                return Ok(0);
            };
            from_dict(cb, &sh, &doc, &dict)
        });
    }

    // ---- collectors ----
    {
        let sh = sh.clone();
        reg!(lua, t, "arraytotable", move |cb| {
            let Some((doc, arr)) = array_arg(cb, 1) else { return Ok(0) };
            let flat = cb.arg_kind(2) == Some(LuaValueKind::Boolean);
            let t = cb.create_table()?;
            let mut j = 0;
            for o in arr.iter() {
                let v = totable(cb, &sh, &doc, o, flat)?;
                j += 1;
                t.raw_seti(j, v)?;
            }
            cb.push(t)
        });
    }
    {
        let sh = sh.clone();
        reg!(lua, t, "dictionarytotable", move |cb| {
            let Some((doc, d)) = dict_arg(cb, 1) else { return Ok(0) };
            let flat = cb.arg_kind(2) == Some(LuaValueKind::Boolean);
            let t = cb.create_table()?;
            for (k, o) in d.keys.iter().zip(&d.vals) {
                let v = totable(cb, &sh, &doc, o, flat)?;
                let key = cb.create_bytes(&k.dec)?;
                t.raw_set(key, v)?;
            }
            cb.push(t)
        });
    }

    // ---- typed getters ----
    {
        reg!(lua, t, "getstring", move |cb| {
            let how = match cb.arg_kind(3) {
                Some(LuaValueKind::Boolean) => {
                    if cb.arg::<bool>(3)? {
                        1
                    } else {
                        2
                    }
                }
                _ => 0,
            };
            let Some((_, Obj::Str(s))) = get_value(cb)? else { return Ok(0) };
            let bytes = if how == 1 { &s.dec } else { &s.enc };
            let b = cb.create_bytes(bytes)?;
            let n = cb.push(b)?;
            if how == 2 {
                Ok(n + cb.push(s.hex)?)
            } else {
                Ok(n)
            }
        });
    }
    reg!(lua, t, "getinteger", move |cb| match get_value(cb)? {
        // lua_pushinteger(L, (int) value)
        Some((_, Obj::Int(i))) => cb.push(i64::from(i as i32)),
        _ => Ok(0),
    });
    reg!(lua, t, "getnumber", move |cb| match get_value(cb)? {
        Some((_, Obj::Int(i))) => cb.push(i as f64),
        Some((_, Obj::Num(f))) => cb.push(f),
        _ => Ok(0),
    });
    reg!(lua, t, "getboolean", move |cb| match get_value(cb)? {
        Some((_, Obj::Bool(b))) => cb.push(b),
        _ => Ok(0),
    });
    reg!(lua, t, "getname", move |cb| match get_value(cb)? {
        Some((_, Obj::Name(n))) => {
            let b = cb.create_bytes(&n.dec)?;
            cb.push(b)
        }
        _ => Ok(0),
    });
    {
        let sh = sh.clone();
        reg!(lua, t, "getdictionary", move |cb| match get_value(cb)? {
            Some((doc, Obj::Dict(d))) => {
                let v = mk_dict(cb, &sh, &doc, &d)?;
                cb.push(v)
            }
            _ => Ok(0),
        });
    }
    {
        let sh = sh.clone();
        reg!(lua, t, "getarray", move |cb| match get_value(cb)? {
            Some((doc, Obj::Array(a))) => {
                let v = mk_array(cb, &sh, &doc, &a)?;
                cb.push(v)
            }
            _ => Ok(0),
        });
    }
    {
        let sh = sh.clone();
        reg!(lua, t, "getstream", move |cb| match get_value(cb)? {
            Some((doc, Obj::Stream(s))) => {
                let a = mk_stream(cb, &sh, &doc, &s)?;
                let b = mk_dict(cb, &sh, &doc, &s.dict)?;
                push_all(cb, vec![a, b])
            }
            _ => Ok(0),
        });
    }

    // ---- streams ----
    fn whole(cb: &mut CallbackLua<'_>) -> LuaResult<usize> {
        let Some(s) = stream_arg(cb, 1) else { return Ok(0) };
        let (doc, stream) = {
            let mut s = s.borrow_mut()?;
            s.open = 0;
            s.decode = false;
            s.data = Vec::new();
            (s.doc.clone(), s.stream.clone())
        };
        let decode = cb.arg_count() > 1 && cb.arg_kind(2) == Some(LuaValueKind::Boolean) && cb.arg::<bool>(2)?;
        let data = doc.stream_data(&stream, decode).unwrap_or_default();
        log_all(&doc);
        let n = data.len() as i64;
        let b = cb.create_bytes(&data)?;
        cb.push((b, n))
    }
    reg!(lua, t, "readwholestream", whole);
    reg!(lua, t, "openstream", move |cb| {
        let Some(s) = stream_arg(cb, 1) else { return Ok(0) };
        let decode = cb.arg_count() > 1 && cb.arg_kind(2) == Some(LuaValueKind::Boolean);
        {
            let mut s = s.borrow_mut()?;
            if s.open == 0 {
                if cb.arg_count() > 1 {
                    s.decode = decode;
                }
                s.open = 1;
            }
        }
        cb.push(true)
    });
    reg!(lua, t, "closestream", move |cb| {
        if let Some(s) = stream_arg(cb, 1) {
            let mut s = s.borrow_mut()?;
            if s.open > 0 {
                s.open = 0;
                s.decode = false;
                s.data = Vec::new();
                s.pos = 0;
            }
        }
        Ok(0)
    });
    reg!(lua, t, "readfromstream", move |cb| {
        let Some(s) = stream_arg(cb, 1) else { return Ok(0) };
        let chunk = {
            let mut s = s.borrow_mut()?;
            match s.open {
                1 => {
                    let data = s.doc.stream_data(&s.stream, s.decode).unwrap_or_default();
                    log_all(&s.doc);
                    s.data = data;
                    s.pos = 0;
                    s.open = 2;
                }
                2 => {}
                _ => return Ok(0),
            }
            let end = (s.pos + CHUNK).min(s.data.len());
            let chunk = s.data[s.pos..end].to_vec();
            s.pos = end;
            chunk
        };
        let n = chunk.len() as i64;
        let b = cb.create_bytes(&chunk)?;
        cb.push((b, n))
    });
    {
        let f = lua.create_callback(whole).map_err(e)?;
        mt.stream.set("__call", f).map_err(e)?;
    }

    let _ = (T_ARRAY, T_DICT, T_STREAM);
    lua.set_global("pdfe", t).map_err(e)?;
    // luaL_openlib registers the library in package.loaded; the Lua error
    // messages name its functions after it ("pdfe.getfromarray")
    lua.load("package.loaded.pdfe = pdfe").exec().map_err(|err| format!("pdfe: {}", lua.get_error_message(err).message()))
}

impl crate::engine::Engine {
    /// Register a PDF held in memory (`pdfe.new(stream, length, id)`) under
    /// its `data:application/pdf,` name; `false` when it is not a PDF.
    pub(crate) fn pdfe_register_memstream(&mut self, name: String, data: Vec<u8>) -> bool {
        if Doc::open(data.clone()).is_none() {
            return false;
        }
        self.pdfe_memstreams.insert(name, data);
        true
    }
}
