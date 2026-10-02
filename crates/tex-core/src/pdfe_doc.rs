//! PDF reader behind the Lua `pdfe` library (`lpdfelib.c`).
//!
//! LuaTeX's `pdfe` is a thin layer over pplib (`ppload.c` and friends): the
//! whole file is parsed up front, strings keep their raw (still escaped)
//! spelling next to the decoded one, numbers are doubles, dictionaries keep
//! file order and duplicate keys, and references resolve through the newest
//! cross-reference section. This module reproduces that object model; the
//! lopdf based reader in `pdf_images.rs` normalises too much (decoded strings,
//! `f32` reals) to serve it. Encryption keys come from lopdf's
//! `EncryptionState`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io::Read;
use std::rc::Rc;

/// A name: the spelling in the file and the `#xx`-decoded one.
pub struct PName {
    pub enc: Vec<u8>,
    pub dec: Vec<u8>,
}

/// A string: the spelling in the file (escapes or hex digits kept) and the
/// decoded bytes.
pub struct PStr {
    pub enc: Vec<u8>,
    pub dec: Vec<u8>,
    pub hex: bool,
}

/// `ppobjtp` values of pplib.
pub const T_NONE: i64 = 0;
pub const T_NULL: i64 = 1;
pub const T_BOOL: i64 = 2;
pub const T_INT: i64 = 3;
pub const T_NUM: i64 = 4;
pub const T_NAME: i64 = 5;
pub const T_STRING: i64 = 6;
pub const T_ARRAY: i64 = 7;
pub const T_DICT: i64 = 8;
pub const T_STREAM: i64 = 9;
pub const T_REF: i64 = 10;

#[derive(Clone)]
pub enum Obj {
    None,
    Null,
    Bool(bool),
    Int(i64),
    Num(f64),
    Name(Rc<PName>),
    Str(Rc<PStr>),
    Array(Rc<Vec<Obj>>),
    Dict(Rc<PDict>),
    Stream(Rc<PStream>),
    Ref(u32),
}

impl Obj {
    pub fn type_code(&self) -> i64 {
        match self {
            Obj::None => T_NONE,
            Obj::Null => T_NULL,
            Obj::Bool(_) => T_BOOL,
            Obj::Int(_) => T_INT,
            Obj::Num(_) => T_NUM,
            Obj::Name(_) => T_NAME,
            Obj::Str(_) => T_STRING,
            Obj::Array(_) => T_ARRAY,
            Obj::Dict(_) => T_DICT,
            Obj::Stream(_) => T_STREAM,
            Obj::Ref(_) => T_REF,
        }
    }
}

#[derive(Default)]
pub struct PDict {
    pub keys: Vec<Rc<PName>>,
    pub vals: Vec<Obj>,
}

impl PDict {
    /// `ppdict_get_obj`: the first entry whose file spelling is `name`.
    pub fn get(&self, name: &[u8]) -> Option<&Obj> {
        self.keys.iter().position(|k| k.enc == name).map(|i| &self.vals[i])
    }
    pub fn len(&self) -> usize {
        self.keys.len()
    }
}

pub struct PStream {
    pub dict: Rc<PDict>,
    /// Offset of the stream data in the file.
    pub offset: usize,
    /// Object number and version the stream was read from, when the document
    /// is encrypted (the key depends on them).
    pub owner: Option<(u32, u32)>,
}

struct Entry {
    offset: usize,
    version: u32,
    obj: Obj,
}

struct Xref {
    entries: BTreeMap<u32, Entry>,
    count: usize,
    offset: usize,
    /// Trailer dictionary (the stream dictionary for xref streams).
    trailer: Rc<PDict>,
    stream: bool,
    prev: Option<usize>,
}

/// Encryption state of a document (`ppcrypt`).
struct Crypt {
    state: lopdf::EncryptionState,
}

#[derive(Default)]
struct State {
    xrefs: Vec<Xref>,
    top: usize,
    crypt: Option<Crypt>,
    log: Vec<String>,
}

pub const CRYPT_NONE: i32 = 0;
pub const CRYPT_DONE: i32 = 1;
pub const CRYPT_FAIL: i32 = -1;
pub const CRYPT_PASS: i32 = -2;

pub struct Doc {
    data: Vec<u8>,
    version: [u8; 9],
    pub status: Cell<i32>,
    state: RefCell<State>,
}

fn ignored(c: u8) -> bool {
    matches!(c, 0 | 9 | 10 | 12 | 13 | 32)
}

fn name_byte(c: u8) -> bool {
    !matches!(c, 0..=32 | b'%' | b'(' | b')' | b'/' | b'<' | b'>' | b'[' | b']' | b'{' | b'}')
}

fn hexval(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// `ppname_is`: a prefix comparison, as in pplib.
fn name_is(name: &PName, s: &str) -> bool {
    name.enc.starts_with(s.as_bytes())
}

fn ref_str(num: u32, version: u32) -> String {
    format!("{num} {version} R")
}

/// Escaping of a decrypted literal string (`ppstring_byte_escape`).
fn string_escape(c: u8) -> Option<Result<u8, ()>> {
    // None: intact; Some(Err): octal; Some(Ok(ch)): backslash + ch.
    match c {
        8 => Some(Ok(b'b')),
        9 => Some(Ok(b't')),
        10 => Some(Ok(b'n')),
        12 => Some(Ok(b'f')),
        13 => Some(Ok(b'r')),
        0..=31 => Some(Err(())),
        b'(' => Some(Ok(b'(')),
        b')' => Some(Ok(b')')),
        b'\\' => Some(Ok(b'\\')),
        128..=255 => Some(Err(())),
        _ => None,
    }
}

fn neg_power10(n: usize) -> f64 {
    format!("1e-{}", n.min(308)).parse().unwrap()
}

/// The object scanner (`ppscan_obj`).
struct Parser<'a> {
    data: &'a [u8],
    pos: usize,
    crypt: Option<&'a lopdf::EncryptionState>,
    cur: (u32, u32),
}

impl<'a> Parser<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Parser { data, pos, crypt: None, cur: (0, 0) }
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        self.pos += 1;
        self.peek()
    }

    /// `ppscan_find`: skip white space and comments.
    fn find(&mut self) -> Option<u8> {
        loop {
            match self.peek() {
                Some(c) if ignored(c) => self.pos += 1,
                Some(b'%') => loop {
                    match self.next() {
                        None => return None,
                        Some(10) | Some(13) => break,
                        _ => {}
                    }
                },
                other => return other,
            }
        }
    }

    fn keyword(&mut self, k: &[u8]) -> bool {
        if self.data.len() >= self.pos + k.len() && &self.data[self.pos..self.pos + k.len()] == k {
            self.pos += k.len();
            true
        } else {
            false
        }
    }

    /// `iof_get_usize`: decimal digits.
    fn uint(&mut self) -> Option<u64> {
        let start = self.pos;
        let mut n: u64 = 0;
        while let Some(c) = self.peek() {
            if !c.is_ascii_digit() {
                break;
            }
            n = n.wrapping_mul(10).wrapping_add(u64::from(c - b'0'));
            self.pos += 1;
        }
        (self.pos > start).then_some(n)
    }

    fn num(&mut self, negative: bool) -> Obj {
        let mut int: i64 = 0;
        while let Some(c) = self.peek().filter(u8::is_ascii_digit) {
            int = int.wrapping_mul(10).wrapping_add(i64::from(c - b'0'));
            self.pos += 1;
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            let value = self.frac(int as f64);
            Obj::Num(if negative { -value } else { value })
        } else {
            Obj::Int(if negative { int.wrapping_neg() } else { int })
        }
    }

    fn frac(&mut self, mut number: f64) -> f64 {
        let mut exp = 0usize;
        while let Some(c) = self.peek().filter(u8::is_ascii_digit) {
            number = number * 10.0 + f64::from(c - b'0');
            exp += 1;
            self.pos += 1;
        }
        if exp > 0 {
            number *= neg_power10(exp);
        }
        number
    }

    fn name(&mut self) -> Rc<PName> {
        let start = self.pos;
        while self.peek().is_some_and(name_byte) {
            self.pos += 1;
        }
        let enc = self.data[start..self.pos].to_vec();
        let mut dec = Vec::with_capacity(enc.len());
        let mut i = 0;
        while i < enc.len() {
            if enc[i] == b'#' && i + 2 < enc.len() {
                if let (Some(h), Some(l)) = (hexval(enc[i + 1]), hexval(enc[i + 2])) {
                    dec.push((h << 4) | l);
                    i += 3;
                    continue;
                }
            }
            dec.push(enc[i]);
            i += 1;
        }
        Rc::new(PName { enc, dec })
    }

    fn string(&mut self) -> Rc<PStr> {
        if let Some(crypt) = self.crypt {
            return self.crypt_string(crypt);
        }
        let mut enc = Vec::new();
        let mut escapes = false;
        let mut balance = 0usize;
        while let Some(c) = self.peek() {
            match c {
                b'\\' => {
                    escapes = true;
                    enc.push(c);
                    if let Some(n) = self.next() {
                        enc.push(n);
                        self.pos += 1;
                    }
                }
                b'(' => {
                    balance += 1;
                    enc.push(c);
                    self.pos += 1;
                }
                b')' => {
                    if balance == 0 {
                        self.pos += 1;
                        break;
                    }
                    balance -= 1;
                    enc.push(c);
                    self.pos += 1;
                }
                _ => {
                    enc.push(c);
                    self.pos += 1;
                }
            }
        }
        if !escapes {
            return Rc::new(PStr { dec: enc.clone(), enc, hex: false });
        }
        let mut dec = Vec::with_capacity(enc.len());
        let mut i = 0;
        while i < enc.len() {
            if enc[i] != b'\\' {
                dec.push(enc[i]);
                i += 1;
                continue;
            }
            i += 1;
            if i >= enc.len() {
                break;
            }
            match enc[i] {
                b'0'..=b'7' => {
                    let mut v = u32::from(enc[i] - b'0');
                    i += 1;
                    if i < enc.len() && (b'0'..=b'7').contains(&enc[i]) {
                        v = (v << 3) + u32::from(enc[i] - b'0');
                        i += 1;
                        if i < enc.len() && (b'0'..=b'7').contains(&enc[i]) {
                            v = (v << 3) + u32::from(enc[i] - b'0');
                            i += 1;
                        }
                    }
                    dec.push(v as u8);
                }
                b'n' => {
                    dec.push(b'\n');
                    i += 1;
                }
                b'r' => {
                    dec.push(b'\r');
                    i += 1;
                }
                b't' => {
                    dec.push(b'\t');
                    i += 1;
                }
                b'b' => {
                    dec.push(8);
                    i += 1;
                }
                b'f' => {
                    dec.push(12);
                    i += 1;
                }
                10 | 13 => i += 1,
                other => {
                    dec.push(other);
                    i += 1;
                }
            }
        }
        Rc::new(PStr { enc, dec, hex: false })
    }

    fn decrypt(&self, crypt: &lopdf::EncryptionState, bytes: Vec<u8>) -> Vec<u8> {
        let mut obj = lopdf::Object::String(bytes.clone(), lopdf::StringFormat::Literal);
        match lopdf::encryption::decrypt_object(crypt, (self.cur.0, self.cur.1 as u16), &mut obj) {
            Ok(()) => match obj {
                lopdf::Object::String(b, _) => b,
                _ => bytes,
            },
            Err(_) => bytes,
        }
    }

    /// `ppscan_crypt_string`.
    fn crypt_string(&mut self, crypt: &lopdf::EncryptionState) -> Rc<PStr> {
        let mut raw = Vec::new();
        let mut balance = 0usize;
        let mut encode = false;
        let mut c = self.peek();
        while let Some(ch) = c {
            match ch {
                b'\\' => {
                    c = self.next();
                    let Some(e) = c else { break };
                    encode = true;
                    match e {
                        b'0'..=b'7' => {
                            let mut b = u32::from(e - b'0');
                            c = self.next();
                            if let Some(d @ b'0'..=b'7') = c {
                                b = (b << 3) + u32::from(d - b'0');
                                c = self.next();
                                if let Some(d @ b'0'..=b'7') = c {
                                    b = (b << 3) + u32::from(d - b'0');
                                    c = self.next();
                                }
                            }
                            raw.push(b as u8);
                        }
                        b'n' => {
                            raw.push(b'\n');
                            c = self.next();
                        }
                        b'r' => {
                            raw.push(b'\r');
                            c = self.next();
                        }
                        b't' => {
                            raw.push(b'\t');
                            c = self.next();
                        }
                        b'b' => {
                            raw.push(8);
                            c = self.next();
                        }
                        b'f' => {
                            raw.push(12);
                            c = self.next();
                        }
                        10 | 13 => c = self.next(),
                        other => {
                            raw.push(other);
                            c = self.next();
                        }
                    }
                }
                b'(' => {
                    balance += 1;
                    encode = true;
                    raw.push(b'(');
                    c = self.next();
                }
                b')' => {
                    if balance == 0 {
                        self.pos += 1;
                        break;
                    }
                    balance -= 1;
                    raw.push(b')');
                    c = self.next();
                }
                _ => {
                    if string_escape(ch).is_some() {
                        encode = true;
                    }
                    raw.push(ch);
                    c = self.next();
                }
            }
        }
        let dec = self.decrypt(crypt, raw);
        if !encode {
            return Rc::new(PStr { enc: dec.clone(), dec, hex: false });
        }
        let mut enc = Vec::with_capacity(dec.len());
        for &b in &dec {
            match string_escape(b) {
                None => enc.push(b),
                Some(Err(())) => enc.extend_from_slice(format!("\\{:03o}", b).as_bytes()),
                Some(Ok(ch)) => {
                    enc.push(b'\\');
                    enc.push(ch);
                }
            }
        }
        Rc::new(PStr { enc, dec, hex: false })
    }

    fn hex_string(&mut self) -> Rc<PStr> {
        if let Some(crypt) = self.crypt {
            return self.crypt_hex(crypt);
        }
        let start = self.pos;
        while self.peek().is_some_and(|c| hexval(c).is_some() || ignored(c)) {
            self.pos += 1;
        }
        let enc = self.data[start..self.pos].to_vec();
        if self.peek() == Some(b'>') {
            self.pos += 1;
        }
        let mut dec = Vec::new();
        let mut i = 0;
        while i < enc.len() {
            let Some(h1) = hexval(enc[i]) else {
                i += 1;
                continue;
            };
            i += 1;
            let mut h2 = 0;
            while i < enc.len() {
                if let Some(v) = hexval(enc[i]) {
                    h2 = v;
                    break;
                }
                i += 1;
            }
            i += 1;
            dec.push((h1 << 4) | h2);
        }
        Rc::new(PStr { enc, dec, hex: true })
    }

    /// `ppscan_crypt_base16`.
    fn crypt_hex(&mut self, crypt: &lopdf::EncryptionState) -> Rc<PStr> {
        let mut raw = Vec::new();
        let mut c = self.peek();
        while let Some(ch) = c.filter(|&ch| ch != b'>') {
            let Some(h1) = hexval(ch) else {
                if ignored(ch) {
                    c = self.next();
                    continue;
                }
                break;
            };
            let mut h2 = 0;
            loop {
                c = self.next();
                match c {
                    Some(d) if hexval(d).is_some() => {
                        h2 = hexval(d).unwrap();
                        c = self.next();
                        break;
                    }
                    Some(d) if ignored(d) => continue,
                    _ => break,
                }
            }
            raw.push((h1 << 4) | h2);
        }
        if c == Some(b'>') {
            self.pos += 1;
        }
        let dec = self.decrypt(crypt, raw);
        let enc = dec.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
        Rc::new(PStr { enc, dec, hex: true })
    }

    /// Scan one object and append it to `items`; `false` where pplib's
    /// `ppscan_obj` returns NULL.
    fn item(&mut self, items: &mut Vec<Obj>) -> bool {
        let Some(c) = self.peek() else { return false };
        match c {
            b'0'..=b'9' => {
                let o = self.num(false);
                items.push(o);
            }
            b'.' => {
                self.pos += 1;
                let v = self.frac(0.0);
                items.push(Obj::Num(v));
            }
            b'+' => {
                self.pos += 1;
                let o = self.num(false);
                items.push(o);
            }
            b'-' => {
                self.pos += 1;
                let o = self.num(true);
                items.push(o);
            }
            b'/' => {
                self.pos += 1;
                let n = self.name();
                items.push(Obj::Name(n));
            }
            b'(' => {
                self.pos += 1;
                let s = self.string();
                items.push(Obj::Str(s));
            }
            b'[' => {
                self.pos += 1;
                let mut sub = Vec::new();
                loop {
                    match self.find() {
                        Some(b']') => break,
                        _ => {
                            if !self.item(&mut sub) {
                                return false;
                            }
                        }
                    }
                }
                self.pos += 1;
                items.push(Obj::Array(Rc::new(sub)));
            }
            b'<' => {
                if self.next() == Some(b'<') {
                    self.pos += 1;
                    let mut sub = Vec::new();
                    loop {
                        match self.find() {
                            Some(b'>') => break,
                            _ => {
                                if !self.item(&mut sub) {
                                    return false;
                                }
                            }
                        }
                    }
                    self.pos += 1;
                    if self.peek() == Some(b'>') {
                        self.pos += 1;
                    }
                    items.push(Obj::Dict(Rc::new(make_dict(sub))));
                } else {
                    let s = self.hex_string();
                    items.push(Obj::Str(s));
                }
            }
            b'R' => {
                let n = items.len();
                if n >= 2 {
                    if let (Obj::Int(num), Obj::Int(_)) = (&items[n - 2], &items[n - 1]) {
                        let num = *num as u32;
                        self.pos += 1;
                        items.pop();
                        items[n - 2] = Obj::Ref(num);
                        return true;
                    }
                }
                return false;
            }
            b't' => {
                if self.next() == Some(b'r') && self.next() == Some(b'u') && self.next() == Some(b'e') {
                    self.pos += 1;
                    items.push(Obj::Bool(true));
                } else {
                    return false;
                }
            }
            b'f' => {
                if self.next() == Some(b'a')
                    && self.next() == Some(b'l')
                    && self.next() == Some(b's')
                    && self.next() == Some(b'e')
                {
                    self.pos += 1;
                    items.push(Obj::Bool(false));
                } else {
                    return false;
                }
            }
            b'n' => {
                if self.next() == Some(b'u') && self.next() == Some(b'l') && self.next() == Some(b'l') {
                    self.pos += 1;
                    items.push(Obj::Null);
                } else {
                    return false;
                }
            }
            _ => return false,
        }
        true
    }

    fn object(&mut self) -> Option<Obj> {
        let mut items = Vec::new();
        self.item(&mut items).then(|| items.pop()).flatten()
    }
}

/// `ppdict_create`: pairs whose key is not a name are dropped.
fn make_dict(items: Vec<Obj>) -> PDict {
    let mut dict = PDict::default();
    let mut it = items.into_iter();
    while let (Some(k), Some(v)) = (it.next(), it.next()) {
        if let Obj::Name(name) = k {
            dict.keys.push(name);
            dict.vals.push(v);
        }
    }
    dict
}

impl Doc {
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// Bytes held for the document: the file and the object tables.
    pub fn memory_usage(&self) -> usize {
        self.data.len() + self.objects() * std::mem::size_of::<Entry>()
    }

    /// `(major, minor)` of the header version.
    pub fn version(&self) -> (i64, i64) {
        (i64::from(self.version[0]) - 48, i64::from(self.version[2]) - 48)
    }

    /// Messages pplib logged since the last call.
    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut self.state.borrow_mut().log)
    }

    fn log(&self, message: String) {
        self.state.borrow_mut().log.push(message);
    }

    /// `ppdoc_mem`: parse a document; `None` where pplib returns NULL.
    pub fn open(data: Vec<u8>) -> Option<Rc<Doc>> {
        if data.len() < 10 || &data[..5] != b"%PDF-" {
            return None;
        }
        let mut version = [0u8; 9];
        let mut i = 5;
        while i < 9 && !ignored(data[i]) {
            version[i - 5] = data[i];
            i += 1;
        }
        let doc = Doc { data, version, status: Cell::new(CRYPT_PASS), state: RefCell::new(State::default()) };
        let xref_offset = doc.tail()?;
        let top = doc.load_xref(xref_offset, &mut vec![xref_offset])?;
        {
            let mut st = doc.state.borrow_mut();
            if st.xrefs.is_empty() {
                return None;
            }
            st.top = top;
        }
        doc.crypt_pass(Some(b""), None);
        Some(Rc::new(doc))
    }

    /// `ppdoc_tail`: the offset after `startxref`.
    fn tail(&self) -> Option<usize> {
        let len = self.data.len();
        let mut pos = len;
        let mut back = 1;
        loop {
            pos = pos.checked_sub(10)?;
            let c = *self.data.get(pos)?;
            pos += 1;
            match c {
                0 | 9 | 10 | 12 | 13 | 32 | b'0'..=b'9' | b'%' | b'E' | b'O' | b'F' => {
                    if back > 4 {
                        return None;
                    }
                    back += 1;
                }
                b's' | b't' | b'a' | b'r' | b'x' | b'e' | b'f' => {
                    let tail = &self.data[pos..];
                    let p = tail.iter().position(u8::is_ascii_digit)?;
                    let mut n = 0usize;
                    for &d in tail[p..].iter().take_while(|d| d.is_ascii_digit()) {
                        n = n.wrapping_mul(10).wrapping_add(usize::from(d - b'0'));
                    }
                    return Some(n);
                }
                _ => return None,
            }
            // the next probe goes 10 bytes back from the byte after `c`
        }
    }

    fn load_xref(&self, offset: usize, visited: &mut Vec<usize>) -> Option<usize> {
        if offset >= self.data.len() {
            return None;
        }
        let mut parser = Parser::new(&self.data, offset);
        parser.find();
        let xref = if parser.keyword(b"xref") { self.load_xref_table(parser, offset)? } else { self.load_xref_stream(parser, offset)? };
        self.chain_xref(xref, visited)
    }

    fn load_xref_table(&self, mut p: Parser<'_>, offset: usize) -> Option<Xref> {
        let mut entries = BTreeMap::new();
        let mut count = 0usize;
        p.find();
        while let Some(first) = p.uint() {
            p.find();
            let n = p.uint()?;
            if n == 0 {
                p.find();
                continue;
            }
            count += n as usize;
            for k in 0..n {
                p.find();
                let start = p.pos;
                let end = (start + 18).min(self.data.len());
                let item = &self.data[start..end];
                p.pos = end;
                let number = (first + k) as u32;
                if item.len() == 18 && item[17] == b'n' {
                    let digits = |s: &[u8]| s.iter().take_while(|c| c.is_ascii_digit()).fold(0u64, |a, &c| a.wrapping_mul(10).wrapping_add(u64::from(c - b'0')));
                    let off = digits(&item[..10]);
                    let mut q = 11;
                    while q < item.len() && (item[q] == b' ' || item[q] == b'0') {
                        q += 1;
                    }
                    let version = digits(&item[q..]);
                    entries.insert(number, Entry { offset: off as usize, version: version as u32, obj: Obj::None });
                } else {
                    count -= 1;
                }
            }
            p.find();
        }
        if !p.keyword(b"trailer") {
            return None;
        }
        p.find();
        let Obj::Dict(trailer) = p.object()? else { return None };
        Some(Xref { entries, count, offset, trailer, stream: false, prev: None })
    }

    fn load_xref_stream(&self, mut p: Parser<'_>, offset: usize) -> Option<Xref> {
        // skip "N G obj"
        p.find();
        p.uint()?;
        p.find();
        p.uint()?;
        p.find();
        if !p.keyword(b"obj") {
            return None;
        }
        p.find();
        let Obj::Dict(dict) = p.object()? else { return None };
        let stream = self.start_stream(&mut p)?;
        let stream = PStream { dict: dict.clone(), offset: stream, owner: None };
        let rget_uint = |name: &[u8]| match dict.get(name) {
            Some(Obj::Int(n)) if *n >= 0 => Some(*n as u64),
            _ => None,
        };
        let widths: Vec<u64> = match dict.get(b"W") {
            Some(Obj::Array(a)) => (0..3).map(|i| match a.get(i) { Some(Obj::Int(n)) if *n >= 0 => *n as u64, _ => 0 }).collect(),
            _ => vec![0, 0, 0],
        };
        if widths.iter().any(|&w| w > 8) {
            return None;
        }
        let (w1, w2, w3) = (widths[0] as usize, widths[1] as usize, widths[2] as usize);
        let sections: Vec<(u64, u64)> = match dict.get(b"Index") {
            Some(Obj::Array(a)) => a.chunks(2).filter(|c| c.len() == 2).map(|c| match (&c[0], &c[1]) {
                (Obj::Int(f), Obj::Int(n)) if *f >= 0 && *n >= 0 => Some((*f as u64, *n as u64)),
                _ => None,
            }).collect::<Option<Vec<_>>>()?,
            _ => vec![(0, rget_uint(b"Size").unwrap_or(0))],
        };
        let data = self.stream_data(&stream, true)?;
        let mut at = 0usize;
        let mut entries = BTreeMap::new();
        let mut count = 0usize;
        let field = |at: &mut usize, w: usize| -> Option<u64> {
            let s = data.get(*at..*at + w)?;
            *at += w;
            Some(s.iter().fold(0u64, |a, &b| (a << 8) | u64::from(b)))
        };
        for (first, n) in sections {
            count += n as usize;
            for k in 0..n {
                let f1 = if w1 == 0 { 1 } else { field(&mut at, w1)? };
                let f2 = field(&mut at, w2)?;
                let f3 = field(&mut at, w3)?;
                let number = (first + k) as u32;
                match f1 {
                    0 => count -= 1,
                    1 => {
                        entries.insert(number, Entry { offset: f2 as usize, version: f3 as u32, obj: Obj::None });
                    }
                    2 => {
                        entries.insert(number, Entry { offset: 0, version: 0, obj: Obj::None });
                    }
                    _ => return None,
                }
            }
        }
        Some(Xref { entries, count, offset, trailer: dict, stream: true, prev: None })
    }

    /// `ppxref_load_chain`: register the section after loading `/Prev`. An
    /// empty section is a proxy for its predecessor.
    fn chain_xref(&self, xref: Xref, visited: &mut Vec<usize>) -> Option<usize> {
        let prev = match xref.trailer.get(b"Prev") {
            Some(Obj::Int(n)) if *n >= 0 => {
                let off = *n as usize;
                if visited.contains(&off) {
                    return None;
                }
                visited.push(off);
                Some(self.load_xref(off, visited)?)
            }
            _ => None,
        };
        if xref.entries.is_empty() {
            if let Some(prev) = prev {
                return Some(prev);
            }
        }
        let mut xref = xref;
        xref.prev = prev;
        let mut st = self.state.borrow_mut();
        st.xrefs.push(xref);
        Some(st.xrefs.len() - 1)
    }

    /// `ppscan_start_stream`.
    fn start_stream(&self, p: &mut Parser<'_>) -> Option<usize> {
        p.find();
        if !p.keyword(b"stream") {
            return None;
        }
        match p.peek() {
            Some(13) => {
                if p.next() == Some(10) {
                    p.pos += 1;
                }
            }
            Some(10) => p.pos += 1,
            _ => {}
        }
        Some(p.pos)
    }

    // ---- lookups -------------------------------------------------------

    fn chain(&self) -> Vec<usize> {
        let st = self.state.borrow();
        let mut out = Vec::new();
        let mut cur = Some(st.top);
        while let Some(i) = cur {
            out.push(i);
            cur = st.xrefs[i].prev;
        }
        out
    }

    /// `ppxref_find`: the object of reference `num` in the newest section.
    pub fn find(&self, num: u32) -> Option<Obj> {
        let st = self.state.borrow();
        let mut cur = Some(st.top);
        while let Some(i) = cur {
            if let Some(e) = st.xrefs[i].entries.get(&num) {
                return Some(e.obj.clone());
            }
            cur = st.xrefs[i].prev;
        }
        None
    }

    /// `ppobj_rget_obj`.
    pub fn resolve(&self, o: &Obj) -> Obj {
        match o {
            Obj::Ref(n) => self.find(*n).unwrap_or(Obj::None),
            other => other.clone(),
        }
    }

    pub fn trailer(&self) -> Rc<PDict> {
        let st = self.state.borrow();
        st.xrefs[st.top].trailer.clone()
    }

    pub fn dict_rget(&self, d: &PDict, name: &str) -> Option<Obj> {
        d.get(name.as_bytes()).map(|o| self.resolve(o))
    }

    pub fn dict_rget_dict(&self, d: &PDict, name: &str) -> Option<Rc<PDict>> {
        match self.dict_rget(d, name)? {
            Obj::Dict(d) => Some(d),
            _ => None,
        }
    }

    fn rget_uint(&self, o: &Obj) -> Option<u64> {
        match self.resolve(o) {
            Obj::Int(n) if n >= 0 => Some(n as u64),
            _ => None,
        }
    }

    pub fn catalog(&self) -> Option<Rc<PDict>> {
        self.dict_rget_dict(&self.trailer(), "Root")
    }

    pub fn info(&self) -> Option<Rc<PDict>> {
        self.dict_rget_dict(&self.trailer(), "Info")
    }

    pub fn objects(&self) -> usize {
        let st = self.state.borrow();
        let mut n = 0;
        let mut cur = Some(st.top);
        while let Some(i) = cur {
            n += st.xrefs[i].count;
            cur = st.xrefs[i].prev;
        }
        n
    }

    /// `pprect` lookup with `/Parent` inheritance (`ppdict_get_box`).
    pub fn get_box(&self, dict: &Rc<PDict>, name: &str) -> Option<[f64; 4]> {
        let mut dict = dict.clone();
        loop {
            if let Some(Obj::Array(a)) = self.dict_rget(&dict, name) {
                if a.len() == 4 {
                    let n = |o: &Obj| match o {
                        Obj::Int(i) => Some(*i as f64),
                        Obj::Num(f) => Some(*f),
                        _ => None,
                    };
                    if let (Some(a0), Some(a1), Some(a2), Some(a3)) = (n(&a[0]), n(&a[1]), n(&a[2]), n(&a[3])) {
                        return Some([a0, a1, a2, a3]);
                    }
                }
            }
            dict = self.dict_rget_dict(&dict, "Parent")?;
        }
    }

    // ---- loading of the bodies -----------------------------------------

    /// Load the object at `entry`'s offset (`ppdoc_load_entry`).
    fn load_entry(&self, num: u32, offset: usize, version: u32) -> Obj {
        let st = self.state.borrow();
        let crypt = st.crypt.as_ref().map(|c| &c.state);
        let mut p = Parser::new(&self.data, offset);
        p.crypt = crypt;
        p.cur = (num, version);
        let start_ok = (|| {
            p.find();
            if p.uint()? != u64::from(num) {
                return None;
            }
            p.find();
            if p.uint()? != u64::from(version) {
                return None;
            }
            p.find();
            if !p.keyword(b"obj") {
                return None;
            }
            p.find();
            Some(())
        })();
        if offset >= self.data.len() || start_ok.is_none() {
            drop(st);
            self.log(format!("invalid {} offset {}", ref_str(num, version), offset));
            return Obj::None;
        }
        let Some(mut obj) = p.object() else {
            drop(st);
            self.log(format!("invalid {} object at offset {}", ref_str(num, version), offset));
            return Obj::None;
        };
        match &obj {
            Obj::Dict(d) => {
                let d = d.clone();
                if let Some(off) = self.start_stream(&mut p) {
                    let owner = crypt.is_some().then_some((num, version));
                    obj = Obj::Stream(Rc::new(PStream { dict: d, offset: off, owner }));
                }
            }
            Obj::Int(n) => {
                let n = *n as u32;
                p.find();
                if p.uint().is_some() && p.find() == Some(b'R') {
                    drop(st);
                    return self.find(n).map(|_| Obj::Ref(n)).unwrap_or(Obj::None);
                }
            }
            _ => {}
        }
        obj
    }

    fn set_entry(&self, xref: usize, num: u32, obj: Obj) {
        if let Some(e) = self.state.borrow_mut().xrefs[xref].entries.get_mut(&num) {
            e.obj = obj;
        }
    }

    /// `ppobj_preloaded`: resolve a reference while the body is not loaded.
    fn preloaded(&self, o: &Obj) -> Obj {
        let Obj::Ref(n) = o else { return o.clone() };
        let found = {
            let st = self.state.borrow();
            let mut cur = Some(st.top);
            let mut hit = None;
            while let Some(i) = cur {
                if let Some(e) = st.xrefs[i].entries.get(n) {
                    hit = Some((i, e.offset, e.version, e.obj.clone()));
                    break;
                }
                cur = st.xrefs[i].prev;
            }
            hit
        };
        match found {
            None => Obj::None,
            Some((i, off, ver, obj)) => {
                if !matches!(obj, Obj::None) || off == 0 {
                    return obj;
                }
                let loaded = self.load_entry(*n, off, ver);
                self.set_entry(i, *n, loaded.clone());
                loaded
            }
        }
    }

    /// `ppdoc_load_entries`.
    fn load_entries(&self) {
        let mut refs: Vec<(usize, usize, u32)> = Vec::new(); // (offset, xref, number)
        for xi in self.chain() {
            let st = self.state.borrow();
            for (num, e) in &st.xrefs[xi].entries {
                if e.offset > 0 {
                    refs.push((e.offset, xi, *num));
                }
            }
        }
        refs.sort_by_key(|r| r.0);
        let mut redundant = Vec::new();
        for &(offset, xi, num) in &refs {
            let (loaded, version) = {
                let st = self.state.borrow();
                let e = &st.xrefs[xi].entries[&num];
                (!matches!(e.obj, Obj::None), e.version)
            };
            if loaded {
                continue;
            }
            if self.is_xref_stream_offset(offset) {
                continue;
            }
            let obj = self.load_entry(num, offset, version);
            if matches!(obj, Obj::Ref(_)) {
                redundant.push((xi, num));
            }
            self.set_entry(xi, num, obj);
        }
        for (xi, num) in redundant {
            let target = {
                let st = self.state.borrow();
                match &st.xrefs[xi].entries[&num].obj {
                    Obj::Ref(t) => {
                        let t = *t;
                        drop(st);
                        self.find(t)
                    }
                    _ => None,
                }
            };
            if let Some(t) = target {
                self.set_entry(xi, num, t);
            }
        }
        // object streams before anything else needs their contents
        for &(offset, xi, num) in &refs {
            let (obj, version) = {
                let st = self.state.borrow();
                let e = &st.xrefs[xi].entries[&num];
                (e.obj.clone(), e.version)
            };
            let Obj::Stream(s) = obj else { continue };
            let is_xs = self.state.borrow().xrefs[xi].stream;
            let is_objstm = matches!(s.dict.get(b"Type"), Some(Obj::Name(n)) if name_is(n, "ObjStm"));
            if is_xs && is_objstm && !self.load_objstm(&s, xi) {
                self.log(format!("invalid objects stream {} at offset {}", ref_str(num, version), offset));
            }
        }
    }

    fn is_xref_stream_offset(&self, offset: usize) -> bool {
        let st = self.state.borrow();
        st.xrefs.iter().any(|x| x.stream && x.offset == offset)
    }

    /// `ppdoc_load_objstm`.
    fn load_objstm(&self, s: &PStream, xi: usize) -> bool {
        let (Some(items), Some(first)) = (
            s.dict.get(b"N").and_then(|o| self.rget_uint(o)),
            s.dict.get(b"First").and_then(|o| self.rget_uint(o)),
        ) else {
            return false;
        };
        let Some(data) = self.stream_data(s, true) else { return false };
        let first = first as usize;
        if first >= data.len() {
            return false;
        }
        let mut p = Parser::new(&data, 0);
        let mut invalid = 0;
        for i in 0..items {
            p.find();
            let Some(objnum) = p.uint() else { return false };
            p.find();
            let Some(offset) = p.uint() else { return false };
            let vacant = {
                let st = self.state.borrow();
                st.xrefs[xi].entries.get(&(objnum as u32)).map(|e| matches!(e.obj, Obj::None))
            };
            if vacant != Some(true) {
                self.log(format!("invalid compressed object number {objnum} at position {i}"));
                invalid += 1;
                continue;
            }
            if first + offset as usize >= data.len() {
                self.log(format!("invalid compressed object offset {offset} at position {i}"));
                invalid += 1;
                continue;
            }
            let save = p.pos;
            p.pos = first + offset as usize;
            p.find();
            match p.object() {
                Some(obj) => self.set_entry(xi, objnum as u32, obj),
                None => {
                    invalid += 1;
                    self.log(format!("invalid compressed object {} at stream offset {}", ref_str(objnum as u32, 0), offset));
                }
            }
            p.pos = save;
        }
        invalid == 0
    }

    // ---- encryption -----------------------------------------------------

    /// `ppdoc_crypt_pass`: the status after (re)trying the passwords.
    pub fn crypt_pass(&self, user: Option<&[u8]>, owner: Option<&[u8]>) -> i32 {
        if self.status.get() == CRYPT_PASS {
            let status = self.crypt_init(user, owner);
            self.status.set(status);
            if matches!(status, CRYPT_NONE | CRYPT_DONE) {
                self.load_entries();
            }
        }
        self.status.get()
    }

    fn crypt_init(&self, user: Option<&[u8]>, owner: Option<&[u8]>) -> i32 {
        let trailer = self.trailer();
        let Some(enc_obj) = trailer.get(b"Encrypt") else { return CRYPT_NONE };
        let Obj::Dict(encrypt) = self.preloaded(enc_obj) else { return CRYPT_FAIL };
        for v in &encrypt.vals {
            self.preloaded(v);
        }
        if let Some(Obj::Name(n)) = encrypt.get(b"Filter") {
            if !name_is(n, "Standard") {
                return CRYPT_FAIL;
            }
        }
        let uint = |name: &[u8]| match encrypt.get(name) {
            Some(Obj::Int(n)) if *n >= 0 => Some(*n),
            _ => None,
        };
        let v = uint(b"V").unwrap_or(0);
        if !(1..=5).contains(&v) {
            return CRYPT_FAIL;
        }
        let Some(r) = uint(b"R") else { return CRYPT_FAIL };
        if !matches!(encrypt.get(b"P"), Some(Obj::Int(_))) {
            return CRYPT_FAIL;
        }
        let string = |name: &[u8]| match encrypt.get(name) {
            Some(Obj::Str(s)) => Some(s.dec.len()),
            _ => None,
        };
        let (Some(ulen), Some(olen)) = (string(b"U"), string(b"O")) else { return CRYPT_FAIL };
        let hash = if v < 5 { 32 } else { 48 };
        if ulen < hash || olen < hash {
            return CRYPT_FAIL;
        }
        if v < 5 {
            match trailer.get(b"ID") {
                Some(Obj::Array(a)) if matches!(a.first(), Some(Obj::Str(_))) => {}
                _ => return CRYPT_FAIL,
            }
        } else {
            if string(b"UE").is_none_or(|n| n < 32) || string(b"OE").is_none_or(|n| n < 32) || string(b"Perms") != Some(16) {
                return CRYPT_FAIL;
            }
        }
        if !(1..=6).contains(&r) {
            return CRYPT_FAIL;
        }
        if user.is_none() && owner.is_none() {
            return CRYPT_PASS;
        }
        // lopdf does the key derivation and password checks
        let mut doc = lopdf::Document::new();
        doc.objects.insert((1, 0), self.to_lopdf(&Obj::Dict(encrypt.clone()), 0));
        doc.trailer.set("Encrypt", lopdf::Object::Reference((1, 0)));
        if let Some(id) = trailer.get(b"ID") {
            doc.trailer.set("ID", self.to_lopdf(id, 0));
        }
        let Ok(alg) = lopdf::encryption::PasswordAlgorithm::try_from(&doc) else { return CRYPT_FAIL };
        let mut key = None;
        if let Some(u) = user {
            if alg.authenticate_user_password(&doc, u).is_ok() {
                key = lopdf::EncryptionState::decode(&doc, u).ok();
            }
        }
        if key.is_none() && r >= 5 {
            if let Some(o) = owner {
                if alg.authenticate_owner_password(&doc, o).is_ok() {
                    key = lopdf::EncryptionState::decode(&doc, o).ok();
                }
            }
        }
        match key {
            Some(state) => {
                self.state.borrow_mut().crypt = Some(Crypt { state });
                CRYPT_DONE
            }
            None => CRYPT_PASS,
        }
    }

    fn to_lopdf(&self, o: &Obj, depth: usize) -> lopdf::Object {
        use lopdf::Object as L;
        if depth > 16 {
            return L::Null;
        }
        match o {
            Obj::None | Obj::Null => L::Null,
            Obj::Bool(b) => L::Boolean(*b),
            Obj::Int(i) => L::Integer(*i),
            Obj::Num(f) => L::Real(*f as f32),
            Obj::Name(n) => L::Name(n.dec.clone()),
            Obj::Str(s) => L::String(s.dec.clone(), if s.hex { lopdf::StringFormat::Hexadecimal } else { lopdf::StringFormat::Literal }),
            Obj::Array(a) => L::Array(a.iter().map(|x| self.to_lopdf(x, depth + 1)).collect()),
            Obj::Dict(d) => L::Dictionary(self.dict_to_lopdf(d, depth)),
            Obj::Stream(s) => L::Dictionary(self.dict_to_lopdf(&s.dict, depth)),
            Obj::Ref(n) => match self.find(*n) {
                Some(t) if depth < 8 && !matches!(t, Obj::Ref(_)) => self.to_lopdf(&t, depth + 1),
                _ => L::Reference((*n, 0)),
            },
        }
    }

    fn dict_to_lopdf(&self, d: &PDict, depth: usize) -> lopdf::Dictionary {
        let mut out = lopdf::Dictionary::new();
        for (k, v) in d.keys.iter().zip(&d.vals) {
            out.set(k.dec.clone(), self.to_lopdf(v, depth + 1));
        }
        out
    }

    // ---- streams -------------------------------------------------------

    /// The stream bytes: raw (after decryption) or decoded as far as pplib
    /// can; `None` when the filters cannot even start.
    pub fn stream_data(&self, s: &PStream, decode: bool) -> Option<Vec<u8>> {
        let length = s.dict.get(b"Length").and_then(|o| self.rget_uint(o)).unwrap_or(0) as usize;
        if s.dict.get(b"F").is_some() {
            // external file streams are not supported
            return Some(Vec::new());
        }
        let start = s.offset.min(self.data.len());
        let end = start.saturating_add(length).min(self.data.len());
        let mut data = self.data[start..end].to_vec();
        if let Some((num, ver)) = s.owner {
            let st = self.state.borrow();
            if let Some(crypt) = &st.crypt {
                let mut stream = lopdf::Stream::new(self.dict_to_lopdf(&s.dict, 0), data.clone());
                stream.allows_compression = false;
                let mut obj = lopdf::Object::Stream(stream);
                if lopdf::encryption::decrypt_object(&crypt.state, (num, ver as u16), &mut obj).is_ok() {
                    if let lopdf::Object::Stream(st) = obj {
                        data = st.content;
                    }
                }
            }
        }
        if !decode {
            return Some(data);
        }
        let filters = self.filters_of(&s.dict);
        for (kind, params) in filters {
            match self.apply_filter(&kind, params.as_ref(), &data) {
                Some(out) => data = out,
                None => break,
            }
        }
        Some(data)
    }

    /// Recognised filters with their parameter dictionaries
    /// (`ppstream_info`); unknown names are skipped.
    fn filters_of(&self, dict: &PDict) -> Vec<(Vec<u8>, Option<Rc<PDict>>)> {
        let Some(fobj) = self.dict_rget(dict, "Filter") else { return Vec::new() };
        let pobj = self.dict_rget(dict, "DecodeParms");
        let names: Vec<Option<Rc<PName>>> = match fobj {
            Obj::Name(n) => vec![Some(n)],
            Obj::Array(a) => a.iter().map(|o| if let Obj::Name(n) = o { Some(n.clone()) } else { None }).collect(),
            _ => return Vec::new(),
        };
        let mut out = Vec::new();
        for (i, name) in names.into_iter().enumerate() {
            let Some(name) = name else { continue };
            const KNOWN: [&str; 10] = [
                "ASCIIHexDecode", "ASCII85Decode", "RunLengthDecode", "FlateDecode", "LZWDecode",
                "CCITTFaxDecode", "DCTDecode", "JBIG2Decode", "JPXDecode", "Crypt",
            ];
            let Some(kind) = KNOWN.iter().find(|k| name_is(&name, k)) else { continue };
            let params = match &pobj {
                Some(Obj::Dict(d)) if i == 0 => Some(d.clone()),
                Some(Obj::Array(a)) => match a.get(i).map(|o| self.resolve(o)) {
                    Some(Obj::Dict(d)) => Some(d),
                    _ => None,
                },
                _ => None,
            };
            out.push((kind.as_bytes().to_vec(), params));
        }
        out
    }

    fn apply_filter(&self, kind: &[u8], params: Option<&Rc<PDict>>, data: &[u8]) -> Option<Vec<u8>> {
        let int = |name: &[u8]| -> Option<i64> {
            match params?.get(name)? {
                Obj::Int(n) => Some(*n),
                _ => None,
            }
        };
        match kind {
            b"ASCIIHexDecode" => Some(ascii_hex(data)),
            b"ASCII85Decode" => Some(ascii85(data)),
            b"RunLengthDecode" => Some(run_length(data)),
            b"FlateDecode" | b"LZWDecode" => {
                let out = if kind == b"FlateDecode" {
                    inflate(data)
                } else {
                    lzw(data, int(b"EarlyChange") != Some(0))
                };
                Some(predict(out, int(b"Predictor").unwrap_or(1), int(b"Columns").filter(|&c| c != 0).unwrap_or(1), int(b"Colors").filter(|&c| c != 0).unwrap_or(1), int(b"BitsPerComponent").filter(|&c| c != 0).unwrap_or(8)))
            }
            b"Crypt" => Some(data.to_vec()),
            _ => None,
        }
    }

    // ---- pages ---------------------------------------------------------

    /// `pppage_node`: `(kids, count, type)` of a page tree node.
    fn page_node(&self, d: &PDict) -> (Option<Rc<Vec<Obj>>>, u64, Option<Rc<PName>>) {
        let (mut kids, mut count, mut ty) = (None, 0, None);
        for (k, v) in d.keys.iter().zip(&d.vals) {
            match k.enc.first() {
                Some(b'T') if name_is(k, "Type") => {
                    ty = if let Obj::Name(n) = v { Some(n.clone()) } else { None };
                }
                Some(b'C') if name_is(k, "Count") => {
                    if let Some(n) = self.rget_uint(v) {
                        count = n;
                    }
                }
                Some(b'K') if name_is(k, "Kids") => {
                    kids = if let Obj::Array(a) = self.resolve(v) { Some(a) } else { None };
                }
                _ => {}
            }
        }
        (kids, count, ty)
    }

    /// The root of the page tree: `(object number, dictionary)`.
    fn pages_root(&self) -> Option<(u32, Rc<PDict>)> {
        let cat = self.catalog()?;
        let Some(Obj::Ref(n)) = cat.get(b"Pages") else { return None };
        match self.find(*n)? {
            Obj::Dict(d) => Some((*n, d)),
            _ => None,
        }
    }

    fn is_page_type(ty: &Option<Rc<PName>>) -> bool {
        ty.as_ref().is_some_and(|t| name_is(t, "Page"))
    }

    pub fn page_count(&self) -> u64 {
        let Some((_, root)) = self.pages_root() else { return 0 };
        let (kids, count, ty) = self.page_node(&root);
        if kids.is_none() {
            return u64::from(Self::is_page_type(&ty));
        }
        count
    }

    /// `ppdoc_page`: the object number and dictionary of page `index`.
    pub fn page(&self, index: u64) -> Option<(u32, Rc<PDict>)> {
        let (root_num, root) = self.pages_root()?;
        let (kids, mut count, ty) = self.page_node(&root);
        let mut index = index;
        let Some(mut kids) = kids else {
            return (index == 1 && Self::is_page_type(&ty)).then_some((root_num, root));
        };
        if index < 1 || index > count {
            return None;
        }
        'scan: loop {
            let size = kids.len();
            let from_start = index <= count / 2;
            if !from_start {
                if size == 0 {
                    return None;
                }
                index = count - index + 1;
            }
            for i in 0..size {
                let r = if from_start { &kids[i] } else { &kids[size - 1 - i] };
                let Obj::Ref(n) = r else { return None };
                let Some(Obj::Dict(d)) = self.find(*n) else { return None };
                let (k2, c2, t2) = self.page_node(&d);
                if let Some(k2) = k2 {
                    if index <= c2 {
                        if !from_start {
                            index = c2 - index + 1;
                        }
                        kids = k2;
                        count = c2;
                        continue 'scan;
                    }
                    index -= c2;
                    continue;
                }
                if index == 1 && Self::is_page_type(&t2) {
                    return Some((*n, d));
                }
                index = index.wrapping_sub(1);
            }
            return None;
        }
    }

    /// `ppdoc_first_page` / `ppdoc_next_page` walk: all pages in order.
    pub fn pages(&self) -> Vec<(u32, Rc<PDict>)> {
        let mut out = Vec::new();
        let Some((root_num, root)) = self.pages_root() else { return out };
        // stack of (kids, next index)
        let mut stack: Vec<(Rc<Vec<Obj>>, usize)> = Vec::new();
        // descend from a node; true when iteration goes on
        let mut node: Option<(u32, Rc<PDict>)> = Some((root_num, root));
        loop {
            // pppages_group_first
            let mut pending = node.take();
            let mut found = None;
            while let Some((num, dict)) = pending.take() {
                let (kids, _, ty) = self.page_node(&dict);
                match kids {
                    Some(kids) => match kids.first() {
                        None => break, // empty /Kids: continue with the next page
                        Some(Obj::Ref(n)) => match self.find(*n) {
                            Some(Obj::Dict(d)) => {
                                stack.push((kids.clone(), 0));
                                pending = Some((*n, d));
                            }
                            _ => return out,
                        },
                        Some(_) => return out,
                    },
                    None => {
                        if Self::is_page_type(&ty) {
                            found = Some((num, dict));
                        } else {
                            return out;
                        }
                    }
                }
            }
            if let Some(p) = found {
                out.push(p);
            }
            // ppdoc_next_page
            loop {
                let Some(top) = stack.last_mut() else { return out };
                top.1 += 1;
                if top.1 < top.0.len() {
                    let Obj::Ref(n) = &top.0[top.1] else { return out };
                    let n = *n;
                    let Some(Obj::Dict(d)) = self.find(n) else { return out };
                    node = Some((n, d));
                    break;
                }
                stack.pop();
            }
        }
    }
}

// ---- stream filters -----------------------------------------------------

fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut hi: Option<u8> = None;
    for &c in data {
        if c == b'>' {
            break;
        }
        let Some(v) = hexval(c) else { continue };
        match hi.take() {
            Some(h) => out.push((h << 4) | v),
            None => hi = Some(v),
        }
    }
    if let Some(h) = hi {
        out.push(h << 4);
    }
    out
}

fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut group = [0u32; 5];
    let mut n = 0;
    for &c in data {
        match c {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0, 0, 0, 0]),
            b'!'..=b'u' => {
                group[n] = u32::from(c - b'!');
                n += 1;
                if n == 5 {
                    let v = group.iter().fold(0u32, |a, &d| a.wrapping_mul(85).wrapping_add(d));
                    out.extend_from_slice(&v.to_be_bytes());
                    n = 0;
                }
            }
            c if ignored(c) => {}
            _ => break,
        }
    }
    if n > 1 {
        for g in group.iter_mut().skip(n) {
            *g = 84;
        }
        let v = group.iter().fold(0u32, |a, &d| a.wrapping_mul(85).wrapping_add(d));
        out.extend_from_slice(&v.to_be_bytes()[..n - 1]);
    }
    out
}

fn run_length(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let l = data[i];
        i += 1;
        match l {
            128 => break,
            0..=127 => {
                let n = usize::from(l) + 1;
                let end = (i + n).min(data.len());
                out.extend_from_slice(&data[i..end]);
                i = end;
            }
            _ => {
                if let Some(&b) = data.get(i) {
                    out.extend(std::iter::repeat_n(b, 257 - usize::from(l)));
                }
                i += 1;
            }
        }
    }
    out
}

fn inflate(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut d = flate2::read::ZlibDecoder::new(data);
    let mut buf = [0u8; 8192];
    loop {
        match d.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    out
}

fn lzw(data: &[u8], early: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut table: Vec<Vec<u8>> = Vec::new();
    let reset = |t: &mut Vec<Vec<u8>>| {
        t.clear();
        t.extend((0..=255u8).map(|b| vec![b]));
        t.push(Vec::new()); // 256 clear
        t.push(Vec::new()); // 257 eod
    };
    reset(&mut table);
    let (mut bits, mut acc, mut nacc) = (9u32, 0u32, 0u32);
    let mut prev: Option<Vec<u8>> = None;
    for &b in data {
        acc = (acc << 8) | u32::from(b);
        nacc += 8;
        while nacc >= bits {
            let code = ((acc >> (nacc - bits)) & ((1 << bits) - 1)) as usize;
            nacc -= bits;
            acc &= (1 << nacc) - 1;
            if code == 256 {
                reset(&mut table);
                bits = 9;
                prev = None;
                continue;
            }
            if code == 257 {
                return out;
            }
            let entry = if code < table.len() {
                table[code].clone()
            } else if let Some(p) = &prev {
                let mut e = p.clone();
                e.push(p[0]);
                e
            } else {
                return out;
            };
            out.extend_from_slice(&entry);
            if let Some(p) = prev.take() {
                let mut e = p;
                e.push(entry[0]);
                table.push(e);
            }
            prev = Some(entry);
            let size = table.len() + usize::from(early);
            bits = match size {
                0..=511 => 9,
                512..=1023 => 10,
                1024..=2047 => 11,
                _ => 12,
            };
        }
    }
    out
}

/// PNG and TIFF predictors (`ppstream_predictor`).
fn predict(data: Vec<u8>, predictor: i64, columns: i64, colors: i64, bpc: i64) -> Vec<u8> {
    if predictor <= 1 {
        return data;
    }
    let bits_per_pixel = (colors * bpc) as usize;
    let bpp = bits_per_pixel.div_ceil(8).max(1);
    let row = ((columns as usize) * bits_per_pixel).div_ceil(8);
    if row == 0 {
        return data;
    }
    if predictor == 2 {
        if bpc != 8 {
            return data;
        }
        let mut out = data;
        for line in out.chunks_mut(row) {
            for i in (colors as usize)..line.len() {
                line[i] = line[i].wrapping_add(line[i - colors as usize]);
            }
        }
        return out;
    }
    let mut out: Vec<u8> = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; row];
    for chunk in data.chunks(row + 1) {
        let kind = chunk[0];
        let mut line = chunk[1..].to_vec();
        line.resize(row, 0);
        for i in 0..row {
            let a = if i >= bpp { line[i - bpp] } else { 0 };
            let b = prev[i];
            let c = if i >= bpp { prev[i - bpp] } else { 0 };
            let add = match kind {
                1 => a,
                2 => b,
                3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                4 => {
                    let p = i16::from(a) + i16::from(b) - i16::from(c);
                    let (da, db, dc) = ((p - i16::from(a)).abs(), (p - i16::from(b)).abs(), (p - i16::from(c)).abs());
                    if da <= db && da <= dc { a } else if db <= dc { b } else { c }
                }
                _ => 0,
            };
            line[i] = line[i].wrapping_add(add);
        }
        out.extend_from_slice(&line);
        prev = line;
    }
    out
}
