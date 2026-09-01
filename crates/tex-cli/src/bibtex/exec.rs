//! BibTeX stack-machine interpreter: built-in functions, variables,
//! name formatting, case conversion, purification, and width metrics.
//!
//! Ported from bibtex.web (BibTeX 0.99e) to keep semantics byte-exact.

use std::collections::HashMap;
use std::rc::Rc;

use super::bst::Tok;
use super::Logger;

// ---------------------------------------------------------------------------
// character classes (lex_class in bibtex.web)
// ---------------------------------------------------------------------------

fn is_white(c: u8) -> bool {
    c == b' ' || (0x09..=0x0d).contains(&c)
}
fn is_alpha(c: u8) -> bool {
    c.is_ascii_alphabetic()
}
fn is_sep_char(c: u8) -> bool {
    c == b'~' || c == b'-'
}

fn as_str(l: &Lit) -> Option<&str> {
    match l {
        Lit::Str(s) => Some(s.as_str()),
        _ => None,
    }
}

fn as_int(l: &Lit) -> Option<i64> {
    match l {
        Lit::Int(i) => Some(*i),
        _ => None,
    }
}
fn to_lower(c: u8) -> u8 {
    if c.is_ascii_uppercase() { c + 32 } else { c }
}
fn to_upper(c: u8) -> u8 {
    if c.is_ascii_lowercase() { c - 32 } else { c }
}

/// The 13 special control sequences (accented / foreign characters).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SpecKind {
    I,
    J,
    Oe,
    OeUpper,
    Ae,
    AeUpper,
    Aa,
    AaUpper,
    O,
    OUpper,
    L,
    LUpper,
    Ss,
}

fn spec_kind(name: &[u8]) -> Option<SpecKind> {
    Some(match name {
        b"i" => SpecKind::I,
        b"j" => SpecKind::J,
        b"oe" => SpecKind::Oe,
        b"OE" => SpecKind::OeUpper,
        b"ae" => SpecKind::Ae,
        b"AE" => SpecKind::AeUpper,
        b"aa" => SpecKind::Aa,
        b"AA" => SpecKind::AaUpper,
        b"o" => SpecKind::O,
        b"O" => SpecKind::OUpper,
        b"l" => SpecKind::L,
        b"L" => SpecKind::LUpper,
        b"ss" => SpecKind::Ss,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// width$ metrics: char_width table (cmr10, units of 1/1000 em)
// ---------------------------------------------------------------------------

const SS_WIDTH: i64 = 500;
const AE_WIDTH: i64 = 722;
const OE_WIDTH: i64 = 778;
const UPPER_AE_WIDTH: i64 = 903;
const UPPER_OE_WIDTH: i64 = 1014;

fn char_width(c: u8) -> i64 {
    const W: [i64; 95] = [
        // 32 .. 126, from bibtex.web (cmr10)
        278, 278, 500, 833, 500, 833, 778, 278, 389, 389, 500, 778, 278, 333, 278, 500, 500, 500,
        500, 500, 500, 500, 500, 500, 500, 500, 278, 278, 278, 778, 472, 472, 778, 750, 708, 722,
        764, 681, 653, 785, 750, 361, 514, 778, 625, 917, 750, 778, 681, 778, 736, 556, 722, 750,
        750, 1028, 750, 750, 611, 278, 500, 278, 500, 278, 278, 500, 556, 444, 556, 444, 306, 500,
        556, 278, 306, 528, 278, 833, 556, 500, 556, 528, 392, 394, 389, 556, 528, 722, 528, 528,
        444, 500, 1000, 500, 500,
    ];
    if (32..127).contains(&c) {
        W[(c - 32) as usize]
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// stack literals
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum FnRef {
    Named(String),
    Inline(Rc<Vec<Tok>>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Lit {
    Str(String),
    /// a missing field
    Missing,
    Int(i64),
    /// quoted function reference or inline `{...}` literal
    Fn(FnRef),
    /// error recovery: popped from an empty stack
    Empty,
}

#[derive(Clone)]
enum Sym {
    Builtin(Builtin),
    Wiz(Rc<Vec<Tok>>),
    Field(usize),
    EntInt(usize),
    EntStr(usize),
    GlbInt(usize),
    GlbStr(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Gt,
    Lt,
    Eq,
    Plus,
    Minus,
    Concat,
    Gets,
    AddPeriod,
    CallType,
    ChangeCase,
    ChrToInt,
    Cite,
    Duplicate,
    Empty,
    FormatName,
    If,
    IntToChr,
    IntToStr,
    Missing,
    Newline,
    NumNames,
    Pop,
    Preamble,
    Purify,
    Quote,
    Skip,
    Stack,
    Substring,
    Swap,
    TextLength,
    TextPrefix,
    Top,
    Type,
    Warning,
    While,
    Width,
    Write,
}

const BUILTINS: &[(&str, Builtin)] = &[
    (">", Builtin::Gt),
    ("<", Builtin::Lt),
    ("=", Builtin::Eq),
    ("+", Builtin::Plus),
    ("-", Builtin::Minus),
    ("*", Builtin::Concat),
    (":=", Builtin::Gets),
    ("add.period$", Builtin::AddPeriod),
    ("call.type$", Builtin::CallType),
    ("change.case$", Builtin::ChangeCase),
    ("chr.to.int$", Builtin::ChrToInt),
    ("cite$", Builtin::Cite),
    ("duplicate$", Builtin::Duplicate),
    ("empty$", Builtin::Empty),
    ("format.name$", Builtin::FormatName),
    ("if$", Builtin::If),
    ("int.to.chr$", Builtin::IntToChr),
    ("int.to.str$", Builtin::IntToStr),
    ("missing$", Builtin::Missing),
    ("newline$", Builtin::Newline),
    ("num.names$", Builtin::NumNames),
    ("pop$", Builtin::Pop),
    ("preamble$", Builtin::Preamble),
    ("purify$", Builtin::Purify),
    ("quote$", Builtin::Quote),
    ("skip$", Builtin::Skip),
    ("stack$", Builtin::Stack),
    ("substring$", Builtin::Substring),
    ("swap$", Builtin::Swap),
    ("text.length$", Builtin::TextLength),
    ("text.prefix$", Builtin::TextPrefix),
    ("top$", Builtin::Top),
    ("type$", Builtin::Type),
    ("warning$", Builtin::Warning),
    ("while$", Builtin::While),
    ("width$", Builtin::Width),
    ("write$", Builtin::Write),
];

/// limits — matches the TeX Live bibtex 0.99e build (ent_str_size,
/// glob_str_size), which is what entry.max$/global.max$ are seeded from
pub const ENT_STR_SIZE: usize = 500;
pub const GLOB_STR_SIZE: usize = 200000;

/// A runtime entry (post-READ).
pub struct RtEntry {
    pub cite: String,
    pub type_name: String,
    pub fields: Vec<Option<String>>, // aligned with field_names
    pub ent_ints: Vec<i64>,
    pub ent_strs: Vec<String>,
}

pub struct Interp<'a> {
    pub log: &'a mut Logger,
    syms: HashMap<String, Sym>,
    glb_ints: Vec<i64>,
    glb_strs: Vec<String>,
    pub entries: Vec<RtEntry>,
    pub order: Vec<usize>,
    cur: Option<usize>,
    mess_with_entries: bool,
    out: String,
    out_buf: Vec<u8>,
    preamble: String,
    depth: u32,
    /// per-db-index crossref resolution actions recorded during READ
    xref_canon: HashMap<usize, String>,
    xref_inherit: HashMap<usize, Vec<(String, String)>>,
    xref_clear: std::collections::HashSet<usize>,
}

type R = Result<(), String>;

impl<'a> Interp<'a> {
    pub fn new(log: &'a mut Logger) -> Self {
        let mut syms = HashMap::new();
        for (name, b) in BUILTINS {
            syms.insert((*name).to_string(), Sym::Builtin(*b));
        }
        // built-in variables: entry.max$, global.max$, sort.key$, crossref
        syms.insert("entry.max$".into(), Sym::GlbInt(0));
        syms.insert("global.max$".into(), Sym::GlbInt(1));
        syms.insert("sort.key$".into(), Sym::EntStr(0));
        syms.insert("crossref".into(), Sym::Field(0));
        Interp {
            log,
            syms,
            glb_ints: vec![ENT_STR_SIZE as i64, GLOB_STR_SIZE as i64],
            glb_strs: Vec::new(),
            entries: Vec::new(),
            order: Vec::new(),
            cur: None,
            mess_with_entries: false,
            out: String::new(),
            out_buf: Vec::new(),
            preamble: String::new(),
            depth: 0,
            xref_canon: HashMap::new(),
            xref_inherit: HashMap::new(),
            xref_clear: std::collections::HashSet::new(),
        }
    }

    pub fn preamble(&mut self, s: &str) {
        self.preamble.push_str(s);
    }

    pub fn take_output(&mut self) -> String {
        // end of program: flush any partial line (output_bbl_line)
        if !self.out_buf.is_empty() {
            let buf = std::mem::take(&mut self.out_buf);
            self.write_line(&buf);
        }
        std::mem::take(&mut self.out)
    }

    /// output_bbl_line: write buf[0..n) as a line after stripping trailing
    /// whitespace. An all-whitespace line is dropped entirely (no newline);
    /// an empty buffer writes a blank line.
    fn write_line(&mut self, buf: &[u8]) {
        let mut n = buf.len();
        while n > 0 && is_white(buf[n - 1]) {
            n -= 1;
        }
        if buf.is_empty() {
            self.out.push('\n');
            return;
        }
        if n == 0 {
            return;
        }
        self.out.push_str(&String::from_utf8_lossy(&buf[..n]));
        self.out.push('\n');
    }

    /// add_out_pool + "Break that line" (max_print_line=79, min_print_line=3)
    fn add_out_pool(&mut self, s: &str) {
        self.out_buf.extend_from_slice(s.as_bytes());
        let mut unbreakable_tail = false;
        while self.out_buf.len() > 79 && !unbreakable_tail {
            let end = self.out_buf.len();
            // look backwards from index 79 for whitespace (down to index 3)
            let mut p = 79usize;
            while p >= 3 && !is_white(self.out_buf[p]) {
                p -= 1;
            }
            if p >= 3 && is_white(self.out_buf[p]) {
                // break at p: the whitespace itself is dropped
                let head: Vec<u8> = self.out_buf[..p].to_vec();
                let rest: Vec<u8> = self.out_buf[p + 1..].to_vec();
                self.write_line(&head);
                self.out_buf = vec![b' ', b' '];
                self.out_buf.extend_from_slice(&rest);
            } else {
                // look forward from index 80 for whitespace
                let mut q = 80usize;
                while q < end && !is_white(self.out_buf[q]) {
                    q += 1;
                }
                if q == end {
                    unbreakable_tail = true;
                } else {
                    while q + 1 < end && is_white(self.out_buf[q + 1]) {
                        q += 1;
                    }
                    let head: Vec<u8> = self.out_buf[..q].to_vec();
                    let rest: Vec<u8> = self.out_buf[q + 1..].to_vec();
                    self.write_line(&head);
                    self.out_buf = vec![b' ', b' '];
                    self.out_buf.extend_from_slice(&rest);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // declarations from .bst commands
    // ------------------------------------------------------------------

    pub fn declare_fields(&mut self, names: &[String]) {
        for n in names {
            if self.syms.contains_key(n) {
                self.log.warn(format!("duplicate field declaration \"{n}\""));
                continue;
            }
            let idx = self.field_count();
            self.syms.insert(n.clone(), Sym::Field(idx));
        }
        // field 0 is the implicit `crossref`; real fields start after it, but
        // since we inserted crossref first every Field index lines up with
        // its slot in RtEntry::fields.
    }

    pub fn declare_entry_ints(&mut self, names: &[String]) {
        for n in names {
            if self.syms.contains_key(n) {
                self.log.warn(format!("duplicate entry-integer \"{n}\""));
                continue;
            }
            let idx = self.ent_int_count();
            self.syms.insert(n.clone(), Sym::EntInt(idx));
        }
    }

    pub fn declare_entry_strs(&mut self, names: &[String]) {
        for n in names {
            if self.syms.contains_key(n) {
                self.log.warn(format!("duplicate entry-string \"{n}\""));
                continue;
            }
            let idx = self.ent_str_count(); // 0 taken by sort.key$
            self.syms.insert(n.clone(), Sym::EntStr(idx));
        }
    }

    pub fn declare_ints(&mut self, names: &[String]) {
        for n in names {
            if self.syms.contains_key(n) {
                self.log.warn(format!("duplicate integer variable \"{n}\""));
                continue;
            }
            let idx = self.glb_ints.len();
            self.glb_ints.push(0);
            self.syms.insert(n.clone(), Sym::GlbInt(idx));
        }
    }

    pub fn declare_strs(&mut self, names: &[String]) {
        for n in names {
            if self.syms.contains_key(n) {
                self.log.warn(format!("duplicate string variable \"{n}\""));
                continue;
            }
            let idx = self.glb_strs.len();
            self.glb_strs.push(String::new());
            self.syms.insert(n.clone(), Sym::GlbStr(idx));
        }
    }

    pub fn define_function(&mut self, name: &str, body: Vec<Tok>) {
        let name = &name.to_ascii_lowercase();
        if name == "default.type" {
            self.syms.insert(name.to_string(), Sym::Wiz(Rc::new(body)));
            return;
        }
        if self.syms.contains_key(name) {
            self.log.warn(format!("duplicate function \"{name}\""));
        }
        self.syms.insert(name.to_string(), Sym::Wiz(Rc::new(body)));
    }

    fn field_count(&self) -> usize {
        self.syms
            .values()
            .filter(|s| matches!(s, Sym::Field(_)))
            .count()
            .max(1)
    }

    fn ent_int_count(&self) -> usize {
        self.syms
            .values()
            .filter(|s| matches!(s, Sym::EntInt(_)))
            .count()
    }

    fn ent_str_count(&self) -> usize {
        // sort.key$ is EntStr(0)
        self.syms
            .values()
            .filter(|s| matches!(s, Sym::EntStr(_)))
            .count()
    }

    pub fn num_fields(&self) -> usize {
        self.field_count()
    }

    /// index of a declared field (or the implicit `crossref`)
    pub fn field_index(&self, name: &str) -> Option<usize> {
        match self.lookup(name) {
            Some(Sym::Field(i)) => Some(i),
            _ => None,
        }
    }

    pub fn num_ent_ints(&self) -> usize {
        self.ent_int_count()
    }

    pub fn num_ent_strs(&self) -> usize {
        self.ent_str_count()
    }

    // ------------------------------------------------------------------
    // READ support: load selected entries
    // ------------------------------------------------------------------

    pub fn load_entries(&mut self, entries: Vec<RtEntry>) {
        self.order = (0..entries.len()).collect();
        self.entries = entries;
    }

    /// canonicalize a child's crossref value to the parent's spelling
    pub fn canonicalize_crossref(&mut self, db_idx: usize, parent_key: &str) {
        self.xref_canon.insert(db_idx, parent_key.to_string());
    }

    /// record an inherited field for a child
    pub fn inherit_field(&mut self, db_idx: usize, name: &str, val: &str) {
        self.xref_inherit
            .entry(db_idx)
            .or_default()
            .push((name.to_string(), val.to_string()));
    }

    /// drop a child's crossref field (bad or under-crossed reference)
    pub fn clear_crossref(&mut self, db_idx: usize) {
        self.xref_clear.insert(db_idx);
    }

    /// apply recorded crossref actions to a freshly built field vector
    pub fn apply_inherited(&self, db_idx: usize, fields: &mut [Option<String>]) {
        // clearing kills the crossref value but never the inheritance
        if self.xref_clear.contains(&db_idx) {
            if let Some(idx) = self.field_index("crossref") {
                if idx < fields.len() {
                    fields[idx] = None;
                }
            }
        } else if let Some(spell) = self.xref_canon.get(&db_idx) {
            if let Some(idx) = self.field_index("crossref") {
                if idx < fields.len() {
                    fields[idx] = Some(spell.clone());
                }
            }
        }
        if let Some(pairs) = self.xref_inherit.get(&db_idx) {
            for (name, val) in pairs {
                if let Some(idx) = self.field_index(name) {
                    if idx < fields.len() && fields[idx].is_none() {
                        fields[idx] = Some(val.clone());
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // stack helpers
    // ------------------------------------------------------------------

    fn push(&mut self, l: Lit, st: &mut Vec<Lit>) {
        st.push(l);
    }

    fn pop(&mut self, st: &mut Vec<Lit>) -> Lit {
        match st.pop() {
            Some(l) => l,
            None => {
                self.log.warn("---the literal stack is empty---");
                Lit::Empty
            }
        }
    }

    fn pop_str(&mut self, st: &mut Vec<Lit>) -> Lit {
        match self.pop(st) {
            l @ (Lit::Str(_) | Lit::Missing | Lit::Empty) => l,
            other => {
                self.log.warn(format!("wrong stack literal type: {other:?}, expected string"));
                Lit::Empty
            }
        }
    }

    fn pop_int(&mut self, st: &mut Vec<Lit>) -> Lit {
        match self.pop(st) {
            l @ (Lit::Int(_) | Lit::Empty) => l,
            other => {
                self.log.warn(format!("wrong stack literal type: {other:?}, expected integer"));
                Lit::Empty
            }
        }
    }

    fn pop_fn(&mut self, st: &mut Vec<Lit>) -> Lit {
        match self.pop(st) {
            l @ (Lit::Fn(_) | Lit::Empty) => l,
            other => {
                self.log.warn(format!("wrong stack literal type: {other:?}, expected function"));
                Lit::Empty
            }
        }
    }

    fn as_str(l: &Lit) -> Option<&str> {
        match l {
            Lit::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    fn as_int(l: &Lit) -> Option<i64> {
        match l {
            Lit::Int(i) => Some(*i),
            _ => None,
        }
    }

    // ------------------------------------------------------------------
    // variable access
    // ------------------------------------------------------------------

    /// .bst identifiers resolve case-insensitively (bibtex.web lowercases
    /// hash entries), so `Volume` finds the `volume` field
    fn lookup(&self, name: &str) -> Option<Sym> {
        self.syms.get(&name.to_ascii_lowercase()).cloned()
    }

    fn push_ident(&mut self, name: &str, st: &mut Vec<Lit>) -> R {
        let Some(sym) = self.lookup(name) else {
            return Err(format!("unknown function or variable \"{name}\""));
        };
        match sym {
            Sym::Builtin(b) => {
                self.builtin(b, name, st)?;
            }
            Sym::Wiz(body) => self.exec(&body, st)?,
            Sym::Field(idx) => {
                if !self.mess_with_entries {
                    self.log.warn("you can't access a field outside of an entry context");
                    self.push(Lit::Empty, st);
                } else {
                    let e = self.cur.unwrap();
                    match &self.entries[e].fields[idx] {
                        Some(v) => self.push(Lit::Str(v.clone()), st),
                        None => self.push(Lit::Missing, st),
                    }
                }
            }
            Sym::EntInt(idx) => {
                if !self.mess_with_entries {
                    self.log.warn("you can't access an entry variable outside of an entry context");
                    self.push(Lit::Empty, st);
                } else {
                    let e = self.cur.unwrap();
                    self.push(Lit::Int(self.entries[e].ent_ints[idx]), st);
                }
            }
            Sym::EntStr(idx) => {
                if !self.mess_with_entries {
                    self.log.warn("you can't access an entry variable outside of an entry context");
                    self.push(Lit::Empty, st);
                } else {
                    let e = self.cur.unwrap();
                    self.push(Lit::Str(self.entries[e].ent_strs[idx].clone()), st);
                }
            }
            Sym::GlbInt(idx) => {
                let v = self.glb_ints[idx];
                self.push(Lit::Int(v), st);
            }
            Sym::GlbStr(idx) => {
                let v = self.glb_strs[idx].clone();
                self.push(Lit::Str(v), st);
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // execution
    // ------------------------------------------------------------------

    pub fn exec_toks(&mut self, toks: &[Tok], st: &mut Vec<Lit>) -> R {
        for t in toks {
            if st.len() > 100_000 {
                return Err("literal stack overflow".into());
            }
            match t {
                Tok::Int(i) => self.push(Lit::Int(*i), st),
                Tok::Str(s) => self.push(Lit::Str(s.clone()), st),
                Tok::Quoted(name) => {
                    if self.lookup(name).is_none() {
                        return Err(format!("unknown function \"{name}\""));
                    }
                    self.push(Lit::Fn(FnRef::Named(name.clone())), st);
                }
                Tok::Ident(name) => self.exec_ident(name, st)?,
                Tok::FnLit(toks2) => {
                    self.push(Lit::Fn(FnRef::Inline(Rc::new(toks2.clone()))), st);
                }
            }
        }
        Ok(())
    }

    fn exec_ident(&mut self, name: &str, st: &mut Vec<Lit>) -> R {
        self.push_ident(name, st)
    }

    pub fn exec_fn_named(&mut self, name: &str, st: &mut Vec<Lit>) -> R {
        self.push_ident(name, st)
    }

    fn exec_fn_ref(&mut self, f: &FnRef, st: &mut Vec<Lit>) -> R {
        match f {
            FnRef::Named(name) => self.push_ident(name, st),
            FnRef::Inline(toks) => self.exec(toks, st),
        }
    }

    fn exec(&mut self, toks: &[Tok], st: &mut Vec<Lit>) -> R {
        self.depth += 1;
        if self.depth > 200 {
            self.depth -= 1;
            return Err("execution recursion too deep".into());
        }
        let r = self.exec_toks(toks, st);
        self.depth -= 1;
        r
    }

    /// EXECUTE command
    pub fn cmd_execute(&mut self, name: &str) -> R {
        let mut st = Vec::new();
        self.mess_with_entries = false;
        self.exec_fn_named(name, &mut st)?;
        self.check_stack(&mut st, name);
        Ok(())
    }

    /// ITERATE command (in current sorted order)
    pub fn cmd_iterate(&mut self, name: &str) -> R {
        let mut st = Vec::new();
        self.mess_with_entries = true;
        for k in 0..self.order.len() {
            self.cur = Some(self.order[k]);
            self.exec_fn_named(name, &mut st)?;
            self.check_stack(&mut st, name);
        }
        self.cur = None;
        Ok(())
    }

    /// REVERSE command
    pub fn cmd_reverse(&mut self, name: &str) -> R {
        let mut st = Vec::new();
        self.mess_with_entries = true;
        for k in (0..self.order.len()).rev() {
            self.cur = Some(self.order[k]);
            self.exec_fn_named(name, &mut st)?;
            self.check_stack(&mut st, name);
        }
        self.cur = None;
        Ok(())
    }

    /// SORT command: stable sort by sort.key$ (ASCII byte order, ties by position)
    pub fn cmd_sort(&mut self) {
        if std::env::var("TEXBIB_DEBUG_KEYS").is_ok() {
            for &e in &self.order {
                eprintln!("KEY {}\t{}", self.entries[e].cite, self.entries[e].ent_strs[0]);
            }
        }
        // stable by (sort.key$, current position): WEB's less_than breaks ties
        // by index in sorted_cites
        let mut keyed: Vec<(String, usize)> = self
            .order
            .iter()
            .map(|&e| (self.entries[e].ent_strs[0].clone(), e))
            .collect();
        keyed.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        self.order = keyed.into_iter().map(|(_, e)| e).collect();
    }

    fn check_stack(&mut self, st: &mut Vec<Lit>, name: &str) {
        if !st.is_empty() {
            self.log
                .warn(format!("after executing \"{name}\" the stack isn't empty; clearing"));
            st.clear();
        }
    }

    // ------------------------------------------------------------------
    // built-in functions
    // ------------------------------------------------------------------

    fn builtin(&mut self, b: Builtin, name: &str, st: &mut Vec<Lit>) -> R {
        match b {
            Builtin::Skip => {}
            Builtin::Pop | Builtin::Stack => {
                let _ = self.pop(st);
            }
            Builtin::Top => {
                let l = self.pop(st);
                self.log
                    .info(format!("Top of stack: {}", fmt_lit(&l)));
            }
            Builtin::Gt | Builtin::Lt | Builtin::Eq => {
                let top = self.pop(st);
                let sec = self.pop(st);
                let res = match b {
                    Builtin::Eq => match (&top, &sec) {
                        (Lit::Int(a), Lit::Int(c)) => a == c,
                        (Lit::Str(a), Lit::Str(c)) => a == c,
                        (Lit::Empty, _) | (_, Lit::Empty) => false,
                        (Lit::Missing, Lit::Missing) => true,
                        _ => {
                            self.log
                                .warn(format!("---they aren't the same literal types ({name})"));
                            false
                        }
                    },
                    _ => match (as_int(&top), as_int(&sec)) {
                        (Some(a), Some(c)) => {
                            if b == Builtin::Gt {
                                c > a
                            } else {
                                c < a
                            }
                        }
                        _ => {
                            self.log
                                .warn(format!("{name} requires integer literals; pushing 0"));
                            false
                        }
                    },
                };
                self.push(Lit::Int(res as i64), st);
            }
            Builtin::Plus | Builtin::Minus => {
                let a = self.pop_int(st);
                let c = self.pop_int(st);
                match (as_int(&a), as_int(&c)) {
                    (Some(a), Some(c)) => {
                        let v = if b == Builtin::Plus { c.wrapping_add(a) } else { c.wrapping_sub(a) };
                        self.push(Lit::Int(v), st);
                    }
                    _ => self.push(Lit::Int(0), st),
                }
            }
            Builtin::Concat => {
                let a = self.pop_str(st);
                let c = self.pop_str(st);
                match (as_str(&a), as_str(&c)) {
                    (Some(a), Some(c)) => {
                        let mut s = String::with_capacity(c.len() + a.len());
                        s.push_str(c);
                        s.push_str(a);
                        self.push(Lit::Str(s), st);
                    }
                    _ => self.push(Lit::Str(String::new()), st),
                }
            }
            Builtin::Gets => {
                let target = self.pop_fn(st);
                let value = self.pop(st);
                match target {
                    Lit::Fn(FnRef::Named(tname)) => self.assign(&tname, value, st)?,
                    _ => self.log.warn(":= needs a quoted variable on top of the stack"),
                }
            }
            Builtin::AddPeriod => {
                let l = self.pop_str(st);
                match l {
                    Lit::Str(s) if !s.is_empty() => {
                        let b = s.as_bytes();
                        let mut k = b.len();
                        while k > 0 {
                            k -= 1;
                            if b[k] != b'}' {
                                break;
                            }
                        }
                        let last = b[k];
                        let need = !(last == b'.' || last == b'?' || last == b'!');
                        // Insert before the trailing `}` group so
                        // `\emph{Title}` becomes `\emph{Title.}`.
                        let insert_at = if b[k] == b'}' { 0 } else { k + 1 };
                        let mut s2 = s;
                        if need {
                            s2.insert_str(insert_at, ".");
                        }
                        self.push(Lit::Str(s2), st);
                    }
                    _ => self.push(Lit::Str(String::new()), st),
                }
            }
            Builtin::CallType => {
                if !self.mess_with_entries {
                    self.log.warn("you can't call.type$ outside of an entry context");
                } else {
                    let e = self.cur.unwrap();
                    let tname = self.entries[e].type_name.clone();
                    if tname.is_empty() {
                        // no type: do nothing
                    } else if self.wiz_fn_exists(&tname) {
                        self.exec_fn_named(&tname, st)?;
                    } else if self.wiz_fn_exists("default.type") {
                        self.exec_fn_named("default.type", st)?;
                    }
                }
            }
            Builtin::Cite => {
                if !self.mess_with_entries {
                    self.log.warn("you can't cite$ outside of an entry context");
                    self.push(Lit::Empty, st);
                } else {
                    let e = self.cur.unwrap();
                    let c = self.entries[e].cite.clone();
                    self.push(Lit::Str(c), st);
                }
            }
            Builtin::Type => {
                if !self.mess_with_entries {
                    self.log.warn("you can't type$ outside of an entry context");
                    self.push(Lit::Empty, st);
                } else {
                    let e = self.cur.unwrap();
                    let t = self.entries[e].type_name.clone();
                    if t.is_empty() || !self.wiz_fn_exists(&t) {
                        self.push(Lit::Str(String::new()), st);
                    } else {
                        self.push(Lit::Str(t), st);
                    }
                }
            }
            Builtin::Duplicate => {
                let l = self.pop(st);
                self.push(l.clone(), st);
                self.push(l, st);
            }
            Builtin::Swap => {
                let a = self.pop(st);
                let c = self.pop(st);
                self.push(a, st);
                self.push(c, st);
            }
            Builtin::Empty => {
                let l = self.pop(st);
                let e = match l {
                    Lit::Missing => true,
                    Lit::Str(s) => s.bytes().all(is_white),
                    Lit::Empty => false,
                    _ => {
                        self.log.warn("empty$ needs a string or missing field; pushing 0");
                        false
                    }
                };
                self.push(Lit::Int(e as i64), st);
            }
            Builtin::Missing => {
                let l = self.pop(st);
                let e = matches!(l, Lit::Missing);
                self.push(Lit::Int(e as i64), st);
            }
            Builtin::If => {
                let f1 = self.pop_fn(st);
                let f2 = self.pop_fn(st);
                let cond = self.pop_int(st);
                match (f1, f2, as_int(&cond)) {
                    (Lit::Fn(f1), Lit::Fn(f2), Some(c)) => {
                        // two functions then integer: branch on the integer
                        let pick = if c > 0 { f2 } else { f1 };
                        self.exec_fn_ref(&pick, st)?;
                    }
                    _ => self.log.warn("if$ needs two functions and an integer"),
                }
            }
            Builtin::While => {
                let body = self.pop_fn(st);
                let test = self.pop_fn(st);
                match (body, test) {
                    (Lit::Fn(body), Lit::Fn(test)) => loop {
                        let mut inner = Vec::new();
                        self.exec_fn_ref(&test, &mut inner)?;
                        let cond = self.pop_int(&mut inner);
                        match as_int(&cond) {
                            Some(c) if c > 0 => self.exec_fn_ref(&body, st)?,
                            Some(_) => break,
                            None => {
                                self.log.warn("while$ test didn't leave an integer");
                                break;
                            }
                        }
                        if st.len() > 100_000 {
                            return Err("literal stack overflow in while$".into());
                        }
                    },
                    _ => self.log.warn("while$ needs two functions"),
                }
            }
            Builtin::Preamble => {
                let p = self.preamble.clone();
                self.push(Lit::Str(p), st);
            }
            Builtin::Quote => self.push(Lit::Str("\"".into()), st),
            Builtin::Newline => {
                let buf = std::mem::take(&mut self.out_buf);
                self.write_line(&buf);
            }
            Builtin::Write => {
                let l = self.pop_str(st);
                if let Some(s) = as_str(&l) {
                    self.add_out_pool(s);
                }
            }
            Builtin::Warning => {
                let l = self.pop_str(st);
                if let Some(s) = as_str(&l) {
                    self.log.warn(s.to_string());
                }
            }
            Builtin::ChrToInt => {
                let l = self.pop_str(st);
                match as_str(&l).map(str::as_bytes) {
                    Some([c]) => self.push(Lit::Int(*c as i64), st),
                    _ => {
                        self.log.warn("chr.to.int$ needs a single-character string");
                        self.push(Lit::Int(0), st);
                    }
                }
            }
            Builtin::IntToChr => {
                let l = self.pop_int(st);
                match as_int(&l) {
                    Some(c) if (0..128).contains(&c) => {
                        self.push(Lit::Str(((c as u8) as char).to_string()), st);
                    }
                    _ => {
                        self.log.warn("int.to.chr$ argument isn't valid ASCII");
                        self.push(Lit::Str(String::new()), st);
                    }
                }
            }
            Builtin::IntToStr => {
                let l = self.pop_int(st);
                match as_int(&l) {
                    Some(c) => self.push(Lit::Str(c.to_string()), st),
                    None => self.push(Lit::Str(String::new()), st),
                }
            }
            Builtin::NumNames => {
                let l = self.pop_str(st);
                match as_str(&l) {
                    Some(s) => {
                        let n = count_names(s.as_bytes());
                        self.push(Lit::Int(n as i64), st);
                    }
                    None => self.push(Lit::Int(0), st),
                }
            }
            Builtin::FormatName => {
                let fmt = self.pop_str(st);
                let which = self.pop_int(st);
                let list = self.pop_str(st);
                match (as_str(&fmt), as_int(&which), as_str(&list)) {
                    (Some(fmt), Some(which), Some(list)) => {
                        let out = format_name(fmt.as_bytes(), which, list.as_bytes());
                        self.push(Lit::Str(out), st);
                    }
                    _ => self.push(Lit::Str(String::new()), st),
                }
            }
            Builtin::Substring => {
                let len = self.pop_int(st);
                let start = self.pop_int(st);
                let s = self.pop_str(st);
                match (as_int(&len), as_int(&start), as_str(&s)) {
                    (Some(len), Some(start), Some(s)) => {
                        let out = substring(s, len, start);
                        self.push(Lit::Str(out), st);
                    }
                    _ => self.push(Lit::Str(String::new()), st),
                }
            }
            Builtin::TextLength => {
                let l = self.pop_str(st);
                match as_str(&l) {
                    Some(s) => {
                        let n = text_length(s.as_bytes());
                        self.push(Lit::Int(n as i64), st);
                    }
                    None => self.push(Lit::Str(String::new()), st),
                }
            }
            Builtin::TextPrefix => {
                let n = self.pop_int(st);
                let s = self.pop_str(st);
                match (as_int(&n), as_str(&s)) {
                    (Some(n), Some(s)) => {
                        let out = text_prefix(s, n);
                        self.push(Lit::Str(out), st);
                    }
                    _ => self.push(Lit::Str(String::new()), st),
                }
            }
            Builtin::Purify => {
                let l = self.pop_str(st);
                match as_str(&l) {
                    Some(s) => {
                        let out = purify(s.as_bytes());
                        self.push(Lit::Str(out), st);
                    }
                    None => self.push(Lit::Str(String::new()), st),
                }
            }
            Builtin::ChangeCase => {
                let mode = self.pop_str(st);
                let s = self.pop_str(st);
                match (as_str(&mode), as_str(&s)) {
                    (Some(mode), Some(s)) => {
                        let out = change_case(mode.as_bytes(), s.as_bytes());
                        self.push(Lit::Str(out), st);
                    }
                    _ => self.push(Lit::Str(String::new()), st),
                }
            }
            Builtin::Width => {
                let l = self.pop_str(st);
                match as_str(&l) {
                    Some(s) => {
                        let w = width(s.as_bytes());
                        self.push(Lit::Int(w), st);
                    }
                    None => self.push(Lit::Int(0), st),
                }
            }
        }
        Ok(())
    }

    fn wiz_fn_exists(&self, name: &str) -> bool {
        matches!(self.lookup(name), Some(Sym::Wiz(_)))
    }

    fn assign(&mut self, target: &str, value: Lit, st: &mut Vec<Lit>) -> R {
        let Some(sym) = self.lookup(target) else {
            self.log.warn(format!(":= to unknown variable \"{target}\""));
            return Ok(());
        };
        match sym {
            Sym::Field(idx) => {
                if !self.mess_with_entries {
                    self.log.warn("you can't assign to an entry variable here");
                } else if let Lit::Str(s) = value {
                    let e = self.cur.unwrap();
                    self.entries[e].fields[idx] = Some(trunc(&s, ENT_STR_SIZE, target, self.log));
                } else {
                    self.log.warn(format!(":= to \"{target}\" needs a string"));
                }
            }
            Sym::EntInt(idx) => {
                if !self.mess_with_entries {
                    self.log.warn("you can't assign to an entry variable here");
                } else if let Lit::Int(v) = value {
                    let e = self.cur.unwrap();
                    self.entries[e].ent_ints[idx] = v;
                } else {
                    self.log.warn(format!(":= to \"{target}\" needs an integer"));
                }
            }
            Sym::EntStr(idx) => {
                if !self.mess_with_entries {
                    self.log.warn("you can't assign to an entry variable here");
                } else if let Lit::Str(s) = value {
                    let e = self.cur.unwrap();
                    self.entries[e].ent_strs[idx] = trunc(&s, ENT_STR_SIZE, target, self.log);
                } else {
                    self.log.warn(format!(":= to \"{target}\" needs a string"));
                }
            }
            Sym::GlbInt(idx) => {
                if let Lit::Int(v) = value {
                    self.glb_ints[idx] = v;
                } else {
                    self.log.warn(format!(":= to \"{target}\" needs an integer"));
                }
            }
            Sym::GlbStr(idx) => {
                if let Lit::Str(s) = value {
                    self.glb_strs[idx] = trunc(&s, GLOB_STR_SIZE, target, self.log);
                } else {
                    self.log.warn(format!(":= to \"{target}\" needs a string"));
                }
            }
            _ => self.log.warn(format!("you can't assign to \"{target}\"")),
        }
        let _ = st;
        Ok(())
    }
}

fn trunc(s: &str, cap: usize, what: &str, log: &mut Logger) -> String {
    if s.len() > cap {
        log.warn(format!(
            "you've exceeded {cap}, the entry string size, assigning to \"{what}\""
        ));
        // keep whole chars as far as possible
        let mut end = cap;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s[..end].to_string()
    } else {
        s.to_string()
    }
}

fn fmt_lit(l: &Lit) -> String {
    match l {
        Lit::Str(s) => format!("\"{s}\""),
        Lit::Int(i) => i.to_string(),
        Lit::Fn(FnRef::Named(n)) => format!("function {n}"),
        Lit::Fn(FnRef::Inline(_)) => "inline function literal".into(),
        Lit::Missing => "missing field".into(),
        Lit::Empty => "(empty stack)".into(),
    }
}

// ---------------------------------------------------------------------------
// substring$ (1-based; negative start counts from the end)
// ---------------------------------------------------------------------------

fn substring(s: &str, len: i64, start: i64) -> String {
    let sp = s.len() as i64;
    if len >= sp && (start == 1 || start == -1) {
        return s.to_string();
    }
    if len <= 0 || start == 0 || start > sp || start < -sp {
        return String::new();
    }
    if start > 0 {
        let from = (start - 1) as usize;
        let avail = sp as usize - from;
        let take = (len as usize).min(avail);
        s[from..from + take].to_string()
    } else {
        let from_end = (-start) as usize; // 1 = last char
        let end = sp as usize - (from_end - 1);
        let from = end.saturating_sub(len as usize);
        s[from..end].to_string()
    }
}

// ---------------------------------------------------------------------------
// text.length$ / text.prefix$ (special chars count as one)
// ---------------------------------------------------------------------------

/// Scan forward over one "special character" ({\...}) starting at `i` where
/// b[i] is the `{`. Returns index just past the matching `}`.
fn skip_special(b: &[u8], mut i: usize) -> usize {
    // b[i] == '{'; brace level relative
    let mut lvl = 0usize;
    while i < b.len() {
        match b[i] {
            b'{' => lvl += 1,
            b'}' => {
                lvl -= 1;
                if lvl == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}

fn text_length(b: &[u8]) -> usize {
    let mut n = 0usize;
    let mut i = 0usize;
    let mut lvl = 0i32;
    while i < b.len() {
        let c = b[i];
        i += 1;
        if c == b'{' {
            lvl += 1;
            if lvl == 1 && i < b.len() && b[i] == b'\\' {
                i = skip_special_from_backslash(b, i);
                n += 1;
                continue;
            }
            continue; // a non-special '{' isn't a text character
        } else if c == b'}' {
            if lvl > 0 {
                lvl -= 1;
            }
            continue;
        }
        n += 1;
    }
    n
}

/// b[i] points at the backslash following the opening brace.
/// Consumes through the matching `}` (or end), WEB-style.
fn skip_special_from_backslash(b: &[u8], mut i: usize) -> usize {
    let mut lvl = 1i32;
    while i < b.len() && lvl > 0 {
        match b[i] {
            b'}' => lvl -= 1,
            b'{' => lvl += 1,
            _ => {}
        }
        i += 1;
    }
    i
}

fn text_prefix(s: &str, n: i64) -> String {
    if n <= 0 {
        return String::new();
    }
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut count = 0i64;
    let mut i = 0usize;
    let mut lvl = 0i32;
    while i < b.len() && count < n {
        let c = b[i];
        i += 1;
        if c == b'{' {
            lvl += 1;
            if lvl == 1 && i < b.len() && b[i] == b'\\' {
                // special character: copy whole group, count as one
                let start = i - 1;
                i = skip_special_from_backslash(b, i);
                out.extend_from_slice(&b[start..i]);
                count += 1;
                continue;
            }
            out.push(c);
            continue;
        }
        if c == b'}' {
            if lvl > 0 {
                lvl -= 1;
            }
            out.push(c); // copied, but not a text character
            continue;
        }
        out.push(c);
        count += 1;
    }
    while lvl > 0 {
        out.push(b'}');
        lvl -= 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// purify$
// ---------------------------------------------------------------------------

fn purify(b: &[u8]) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut lvl = 0i32;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if is_white(c) || is_sep_char(c) {
            out.push(b' ');
        } else if is_alpha(c) || c.is_ascii_digit() {
            out.push(c);
        } else if c == b'{' {
            lvl += 1;
            if lvl == 1 && i + 1 < b.len() && b[i + 1] == b'\\' {
                // special character
                i += 1; // at backslash
                while i < b.len() && lvl > 0 {
                    i += 1; // skip backslash
                    let xs = i;
                    while i < b.len() && is_alpha(b[i]) {
                        i += 1;
                    }
                    if let Some(kind) = spec_kind(&b[xs..i]) {
                        out.push(b[xs]);
                        if matches!(kind, SpecKind::Oe | SpecKind::Ae | SpecKind::Ss) {
                            out.push(b[xs + 1]);
                        }
                    }
                    while i < b.len() && lvl > 0 && b[i] != b'\\' {
                        let c2 = b[i];
                        if is_alpha(c2) || c2.is_ascii_digit() {
                            out.push(c2);
                        } else if c2 == b'}' {
                            lvl -= 1;
                        } else if c2 == b'{' {
                            lvl += 1;
                        }
                        i += 1;
                    }
                }
                i = i.saturating_sub(1); // unskip the final '}'
            }
        } else if c == b'}' {
            if lvl > 0 {
                lvl -= 1;
            }
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// change.case$
// ---------------------------------------------------------------------------

fn change_case(mode: &[u8], src: &[u8]) -> String {
    #[derive(PartialEq, Clone, Copy)]
    enum M {
        Title,
        Lower,
        Upper,
        Bad,
    }
    let m = match mode {
        [b't' | b'T'] => M::Title,
        [b'l' | b'L'] => M::Lower,
        [b'u' | b'U'] => M::Upper,
        _ => M::Bad,
    };
    let mut buf: Vec<u8> = src.to_vec();
    let mut prev_colon = false;
    let mut lvl = 0i32;
    let mut i = 0usize;
    while i < buf.len() {
        let c = buf[i];
        if c == b'{' {
            lvl += 1;
            // special char? needs `{\` and 4 bytes minimum (WEB: ptr+4 <= len)
            let mut special = lvl == 1 && i + 4 <= buf.len() && buf[i + 1] == b'\\';
            if special && m == M::Title {
                if i == 0 {
                    special = false;
                } else if prev_colon && is_white(buf[i - 1]) {
                    special = false;
                }
            }
            if special {
                i += 1; // skip '{'
                while i < buf.len() && lvl > 0 {
                    i += 1; // skip '\'
                    let xs = i;
                    while i < buf.len() && is_alpha(buf[i]) {
                        i += 1;
                    }
                    let name = buf[xs..i].to_vec();
                    if let Some(kind) = spec_kind(&name) {
                        match m {
                            M::Title | M::Lower => {
                                if matches!(
                                    kind,
                                    SpecKind::LUpper
                                        | SpecKind::OUpper
                                        | SpecKind::OeUpper
                                        | SpecKind::AeUpper
                                        | SpecKind::AaUpper
                                ) {
                                    for k in xs..i {
                                        buf[k] = to_lower(buf[k]);
                                    }
                                }
                            }
                            M::Upper => match kind {
                                SpecKind::L
                                | SpecKind::O
                                | SpecKind::Oe
                                | SpecKind::Ae
                                | SpecKind::Aa => {
                                    for k in xs..i {
                                        buf[k] = to_upper(buf[k]);
                                    }
                                }
                                SpecKind::I | SpecKind::J | SpecKind::Ss => {
                                    // uppercase, remove backslash, drop following ws
                                    for k in xs..i {
                                        buf[k] = to_upper(buf[k]);
                                    }
                                    // remove backslash: shift [xs..] left by 1
                                    for k in xs..buf.len() {
                                        buf[k - 1] = buf[k];
                                    }
                                    buf.pop();
                                    i -= 1;
                                    while i < buf.len() && is_white(buf[i]) {
                                        // remove whitespace after control seq
                                        for k in i + 1..buf.len() {
                                            buf[k - 1] = buf[k];
                                        }
                                        buf.pop();
                                    }
                                    let _ = xs;
                                }
                                _ => {}
                            },
                            M::Bad => {}
                        }
                    }
                    // scan to next control sequence or close brace
                    let x2 = i;
                    while i < buf.len() && lvl > 0 && buf[i] != b'\\' {
                        match buf[i] {
                            b'}' => lvl -= 1,
                            b'{' => lvl += 1,
                            _ => {}
                        }
                        i += 1;
                    }
                    // convert the non-control-sequence segment
                    match m {
                        M::Title | M::Lower => {
                            for k in x2..i {
                                buf[k] = to_lower(buf[k]);
                            }
                        }
                        M::Upper => {
                            for k in x2..i {
                                buf[k] = to_upper(buf[k]);
                            }
                        }
                        M::Bad => {}
                    }
                }
                i = i.saturating_sub(1); // unskip the '}'
            }
            prev_colon = false;
        } else if c == b'}' {
            if lvl == 0 {
                // brace unbalanced complaint; tolerate
            } else {
                lvl -= 1;
            }
            prev_colon = false;
        } else if lvl == 0 {
            match m {
                M::Title => {
                    if i > 0 && !(prev_colon && is_white(buf[i - 1])) {
                        buf[i] = to_lower(buf[i]);
                    }
                    if buf[i] == b':' {
                        prev_colon = true;
                    } else if !is_white(buf[i]) {
                        prev_colon = false;
                    }
                }
                M::Lower => buf[i] = to_lower(buf[i]),
                M::Upper => buf[i] = to_upper(buf[i]),
                M::Bad => {}
            }
        }
        i += 1;
    }
    String::from_utf8_lossy(&buf).into_owned()
}

// ---------------------------------------------------------------------------
// width$
// ---------------------------------------------------------------------------

fn width(b: &[u8]) -> i64 {
    let mut w = 0i64;
    let mut lvl = 0i32;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if c == b'{' {
            lvl += 1;
            if lvl == 1 && i + 1 < b.len() && b[i + 1] == b'\\' {
                i += 1; // skip '{'; loop below skips '\'
                while i < b.len() && lvl > 0 {
                    i += 1; // skip backslash
                    let xs = i;
                    while i < b.len() && is_alpha(b[i]) {
                        i += 1;
                    }
                    if i == xs {
                        // non-alpha control sequence: skip one char
                        if i < b.len() {
                            i += 1;
                        }
                    } else if let Some(kind) = spec_kind(&b[xs..i]) {
                        w += match kind {
                            SpecKind::Ss => SS_WIDTH,
                            SpecKind::Ae => AE_WIDTH,
                            SpecKind::Oe => OE_WIDTH,
                            SpecKind::AeUpper => UPPER_AE_WIDTH,
                            SpecKind::OeUpper => UPPER_OE_WIDTH,
                            _ => char_width(b[xs]),
                        };
                    }
                    // skip whitespace following the control sequence
                    while i < b.len() && is_white(b[i]) {
                        i += 1;
                    }
                    while i < b.len() && lvl > 0 && b[i] != b'\\' {
                        match b[i] {
                            b'}' => lvl -= 1,
                            b'{' => lvl += 1,
                            c2 => w += char_width(c2),
                        }
                        i += 1;
                    }
                }
                i = i.saturating_sub(1);
            } else {
                w += char_width(b'{');
            }
        } else if c == b'}' {
            if lvl > 0 {
                lvl -= 1;
            }
            w += char_width(b'}');
        } else {
            w += char_width(c);
        }
        i += 1;
    }
    w
}

// ---------------------------------------------------------------------------
// name-list handling (num.names$ / format.name$)
// ---------------------------------------------------------------------------

/// Count names: occurrences of brace-protected "and" + 1.
fn count_names(b: &[u8]) -> usize {
    let mut n = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        let (_, next) = scan_one_name(b, i);
        n += 1;
        i = next;
    }
    n
}

/// Scan one name starting at `start`. Returns (end_of_name, next_scan_pos).
/// Mirrors WEB: find brace-protected " and "; the name ends just before it.
fn scan_one_name(b: &[u8], start: usize) -> (usize, usize) {
    let mut i = start;
    let mut lvl = 0i32;
    let mut prev_white = false;
    while i < b.len() {
        let c = b[i];
        match c {
            b'a' | b'A' => {
                i += 1;
                if prev_white
                    && i + 2 <= b.len()
                    && (b[i] == b'n' || b[i] == b'N')
                    && (b[i + 1] == b'd' || b[i + 1] == b'D')
                    && i + 2 < b.len()
                    && is_white(b[i + 2])
                {
                    // name ended before " and "
                    let mut end = i - 1; // before the space preceding 'a'
                    while end > start && is_white(b[end - 1]) {
                        end -= 1;
                    }
                    return (end, i + 2);
                }
                prev_white = false;
            }
            b'{' => {
                lvl += 1;
                i += 1;
                while lvl > 0 && i < b.len() {
                    match b[i] {
                        b'}' => lvl -= 1,
                        b'{' => lvl += 1,
                        _ => {}
                    }
                    i += 1;
                }
                prev_white = false;
            }
            b'}' => {
                if lvl > 0 {
                    lvl -= 1;
                }
                i += 1;
                prev_white = false;
            }
            _ => {
                i += 1;
                prev_white = is_white(c);
            }
        }
    }
    (b.len(), b.len())
}

// ---------------------------------------------------------------------------
// format.name$
// ---------------------------------------------------------------------------

struct NameTokens {
    buf: Vec<u8>,
    starts: Vec<usize>,
    seps: Vec<u8>, // sep before token i
}

impl NameTokens {
    fn tok(&self, i: usize) -> &[u8] {
        let s = self.starts[i];
        let e = if i + 1 < self.starts.len() {
            self.starts[i + 1]
        } else {
            self.buf.len()
        };
        &self.buf[s..e]
    }
    fn n(&self) -> usize {
        self.starts.len()
    }
}

/// Tokenize a single name (already isolated from the list).
/// Returns the tokens plus the token indexes at which top-level commas sat.
fn tokenize_name(b: &[u8]) -> (NameTokens, Vec<usize>) {
    let mut t = NameTokens { buf: Vec::new(), starts: Vec::new(), seps: Vec::new() };
    let mut commas: Vec<usize> = Vec::new();
    let mut lvl = 0i32;
    let mut token_starting = true;
    // sep recorded for the token about to start (WEB: name_sep_char)
    let mut pending_sep: Option<u8> = None;
    let mut i = 0usize;

    while i < b.len() {
        let c = b[i];
        match c {
            b',' => {
                if commas.len() < 2 {
                    commas.push(t.starts.len());
                    pending_sep = Some(b',');
                }
                i += 1;
                token_starting = true;
            }
            b'{' => {
                if token_starting {
                    t.starts.push(t.buf.len());
                    t.seps.push(pending_sep.take().unwrap_or(b' '));
                    token_starting = false;
                }
                lvl += 1;
                t.buf.push(c);
                i += 1;
                while lvl > 0 && i < b.len() {
                    match b[i] {
                        b'}' => lvl -= 1,
                        b'{' => lvl += 1,
                        _ => {}
                    }
                    t.buf.push(b[i]);
                    i += 1;
                }
            }
            b'}' => {
                // stray close brace: record verbatim
                if token_starting {
                    t.starts.push(t.buf.len());
                    t.seps.push(pending_sep.take().unwrap_or(b' '));
                    token_starting = false;
                }
                t.buf.push(c);
                i += 1;
            }
            _ => {
                if is_white(c) {
                    if !token_starting {
                        pending_sep = Some(b' ');
                    }
                    i += 1;
                    token_starting = true;
                } else if is_sep_char(c) {
                    if !token_starting {
                        pending_sep = Some(c);
                    }
                    i += 1;
                    token_starting = true;
                } else {
                    if token_starting {
                        t.starts.push(t.buf.len());
                        t.seps.push(pending_sep.take().unwrap_or(b' '));
                        token_starting = false;
                    }
                    t.buf.push(c);
                    i += 1;
                }
            }
        }
    }
    (t, commas)
}

/// von token test: first brace-level-0 letter (or level-1 special char) is lowercase.
fn is_von_token(tok: &[u8]) -> bool {
    let mut p = 0usize;
    while p < tok.len() {
        let c = tok[p];
        if c.is_ascii_uppercase() {
            return false;
        }
        if c.is_ascii_lowercase() {
            return true;
        }
        if c == b'{' {
            p += 1;
            if p + 2 < tok.len() && tok[p] == b'\\' {
                p += 1;
                let xs = p;
                while p < tok.len() && is_alpha(tok[p]) {
                    p += 1;
                }
                if let Some(kind) = spec_kind(&tok[xs..p]) {
                    return matches!(
                        kind,
                        SpecKind::I
                            | SpecKind::J
                            | SpecKind::Oe
                            | SpecKind::Ae
                            | SpecKind::Aa
                            | SpecKind::O
                            | SpecKind::L
                            | SpecKind::Ss
                    );
                }
                // scan the group for its first letter
                let mut lvl = 1i32;
                while p < tok.len() && lvl > 0 {
                    let c2 = tok[p];
                    if c2.is_ascii_uppercase() {
                        return false;
                    }
                    if c2.is_ascii_lowercase() {
                        return true;
                    }
                    match c2 {
                        b'}' => lvl -= 1,
                        b'{' => lvl += 1,
                        _ => {}
                    }
                    p += 1;
                }
                return false;
            } else {
                let mut lvl = 1i32;
                p += 1;
                while lvl > 0 && p < tok.len() {
                    match tok[p] {
                        b'}' => lvl -= 1,
                        b'{' => lvl += 1,
                        _ => {}
                    }
                    p += 1;
                }
            }
        } else {
            p += 1;
        }
    }
    false
}

struct NameParts {
    t: NameTokens,
    first: (usize, usize),
    von: (usize, usize),
    last: (usize, usize),
    jr: (usize, usize),
}

fn split_name(b: &[u8]) -> NameParts {
    let (t, comma_positions) = tokenize_name(b);
    let n = t.n();
    let num_commas = comma_positions.len().min(2);
    // drop commas beyond the first two (warned elsewhere)

    let (von_start, von_end, last_end, first, jr);
    match num_commas {
        0 => {
            let le = n;
            let mut vs = 0usize;
            let mut ve;
            let mut found = false;
            while vs + 1 < le {
                if is_von_token(t.tok(vs)) {
                    found = true;
                    break;
                }
                vs += 1;
            }
            if found {
                ve = le - 1;
                while ve > vs {
                    if is_von_token(t.tok(ve - 1)) {
                        break;
                    }
                    ve -= 1;
                }
            } else {
                while vs > 0 {
                    let s = t.seps[vs];
                    if !(is_sep_char(s) && s != b'~') {
                        break;
                    }
                    vs -= 1;
                }
                ve = vs;
            }
            last_end = le;
            von_start = vs;
            von_end = ve;
            first = (0, vs);
            jr = (le, le);
        }
        1 => {
            let le = comma_positions[0];
            let mut vs = 0usize;
            let ve = le.saturating_sub(1).max(vs);
            let mut ve = ve;
            while ve > vs {
                if is_von_token(t.tok(ve - 1)) {
                    break;
                }
                ve -= 1;
            }
            last_end = le;
            von_start = vs;
            von_end = ve;
            first = (le, n);
            jr = (le, le);
        }
        _ => {
            let le = comma_positions[0];
            let c2 = comma_positions[1];
            let vs = 0usize;
            let mut ve = le.saturating_sub(1).max(vs);
            while ve > vs {
                if is_von_token(t.tok(ve - 1)) {
                    break;
                }
                ve -= 1;
            }
            last_end = le;
            von_start = vs;
            von_end = ve;
            first = (c2, n);
            jr = (le, c2);
        }
    }
    NameParts { t, first, von: (von_start, von_end), last: (von_end, last_end), jr }
}

fn enough_text_chars(out: &[u8], from: usize, enough: usize) -> bool {
    let mut num = 0usize;
    let mut lvl = 0i32;
    let mut i = from;
    while i < out.len() && num < enough {
        let c = out[i];
        i += 1;
        if c == b'{' {
            lvl += 1;
            if lvl == 1 && i < out.len() && out[i] == b'\\' {
                // whole special char counts as one
                i = skip_special_from_backslash(out, i);
                num += 1;
                continue;
            }
        } else if c == b'}' {
            if lvl > 0 {
                lvl -= 1;
            }
        }
        num += 1;
    }
    num >= enough
}

/// output one token: full (verbatim) or abbreviated (first letter or
/// `{\...}` special char).
fn output_token(out: &mut Vec<u8>, tok: &[u8], full: bool) {
    if full {
        out.extend_from_slice(tok);
        return;
    }
    let mut p = 0usize;
    while p < tok.len() {
        let c = tok[p];
        if is_alpha(c) {
            out.push(c);
            return;
        }
        if c == b'{' && p + 1 < tok.len() && tok[p + 1] == b'\\' {
            // copy the special character group
            out.push(b'{');
            out.push(b'\\');
            p += 2;
            let mut lvl = 1i32;
            while p < tok.len() && lvl > 0 {
                match tok[p] {
                    b'}' => lvl -= 1,
                    b'{' => lvl += 1,
                    _ => {}
                }
                out.push(tok[p]);
                p += 1;
            }
            return;
        }
        p += 1;
    }
}

/// format.name$: format string, name number (1-based), list of names.
pub fn format_name(fmt: &[u8], which: i64, list: &[u8]) -> String {
    // 1. isolate the `which`-th name
    if which < 1 {
        return String::new();
    }
    let mut pos = 0usize;
    let mut count = 0i64;
    let mut name_end = list.len();
    let mut name_start = 0usize;
    while count < which && pos < list.len() {
        count += 1;
        name_start = pos;
        let (end, next) = scan_one_name(list, pos);
        name_end = end;
        pos = next;
    }
    if count < which {
        // not that many names: use the last one scanned (best effort)
        name_start = name_end;
    }
    let name = &list[name_start..name_end];
    let parts = split_name(name);

    // 2. walk the format string
    let mut out: Vec<u8> = Vec::new();
    let mut lvl = 0i32;
    let mut i = 0usize;
    while i < fmt.len() {
        match fmt[i] {
            b'{' => {
                lvl += 1;
                i += 1;
                if lvl != 1 {
                    continue;
                }
                // pass 1: find the letter and decide
                let gstart = i;
                let mut alpha_found = false;
                let mut double_letter = false;
                let mut end_of_group = false;
                let mut to_be_written = true;
                let mut part: Option<(usize, usize)> = None;
                while !end_of_group && i < fmt.len() {
                    let c = fmt[i];
                    if is_alpha(c) {
                        i += 1;
                        if alpha_found {
                            to_be_written = false;
                        } else {
                            let lc = c.to_ascii_lowercase();
                            let rng = match lc {
                                b'f' => Some(parts.first),
                                b'v' => Some(parts.von),
                                b'l' => Some(parts.last),
                                b'j' => Some(parts.jr),
                                _ => None,
                            };
                            match rng {
                                Some(r) => {
                                    part = Some(r);
                                    if r.0 == r.1 {
                                        to_be_written = false;
                                    }
                                    if i < fmt.len() && fmt[i].to_ascii_lowercase() == lc {
                                        double_letter = true;
                                        i += 1;
                                    }
                                }
                                None => to_be_written = false,
                            }
                            alpha_found = true;
                        }
                    } else if c == b'}' {
                        lvl -= 1;
                        i += 1;
                        end_of_group = true;
                    } else if c == b'{' {
                        lvl += 1;
                        i += 1;
                        while lvl > 1 && i < fmt.len() {
                            match fmt[i] {
                                b'}' => lvl -= 1,
                                b'{' => lvl += 1,
                                _ => {}
                            }
                            i += 1;
                        }
                    } else {
                        i += 1;
                    }
                }
                if end_of_group && to_be_written {
                    // pass 2: output
                    let (mut cur, last) = part.unwrap();
                    let mut j = gstart;
                    let mut lvl2 = 1i32;
                    let part_out_start = out.len();
                    while lvl2 > 0 && j < fmt.len() {
                        let c = fmt[j];
                        if is_alpha(c) && lvl2 == 1 {
                            j += 1;
                            if double_letter {
                                j += 1; // skip the second letter of a doubled spec
                            }
                            // output this part's tokens
                            let full = double_letter;
                            let mut use_default = true;
                            let mut inter: Option<(usize, usize)> = None;
                            if j < fmt.len() && fmt[j] == b'{' {
                                use_default = false;
                                j += 1;
                                let s = j;
                                let mut d = 1i32;
                                while j < fmt.len() && d > 0 {
                                    match fmt[j] {
                                        b'}' => d -= 1,
                                        b'{' => d += 1,
                                        _ => {}
                                    }
                                    if d == 0 {
                                        break;
                                    }
                                    j += 1;
                                }
                                inter = Some((s, j));
                            }
                            while cur < last {
                                output_token(&mut out, parts.t.tok(cur), full);
                                cur += 1;
                                if cur < last {
                                    if use_default {
                                        if !full {
                                            out.push(b'.');
                                        }
                                        let sep = if cur < parts.t.seps.len() {
                                            parts.t.seps[cur]
                                        } else {
                                            b' '
                                        };
                                        if is_sep_char(sep) {
                                            out.push(sep);
                                        } else if cur + 1 == last
                                            || !enough_text_chars(
                                                &out,
                                                part_out_start,
                                                3,
                                            )
                                        {
                                            out.push(b'~');
                                        } else {
                                            out.push(b' ');
                                        }
                                    } else if let Some((s, e)) = inter {
                                        out.extend_from_slice(&fmt[s..e]);
                                    }
                                }
                            }
                            if !use_default {
                                if let Some((_, e)) = inter {
                                    j = e + 1; // skip past the nested group's '}'
                                }
                            }
                        } else if c == b'}' {
                            lvl2 -= 1;
                            j += 1;
                            if lvl2 > 0 {
                                out.push(b'}');
                            }
                        } else if c == b'{' {
                            lvl2 += 1;
                            j += 1;
                            out.push(b'{');
                        } else {
                            out.push(c);
                            j += 1;
                        }
                    }
                    // discretionary tie
                    let n = out.len();
                    if n > part_out_start && out[n - 1] == b'~' {
                        // WEB: decr(ex_buf_ptr) first, so the count excludes
                        // the tie itself
                        let ok = enough_text_chars(&out[..n - 1], part_out_start, 3);
                        if n >= 2 && out[n - 2] == b'~' {
                            out.truncate(n - 1);
                        } else if !ok {
                            // short part: keep the tie
                        } else {
                            out[n - 1] = b' ';
                        }
                    }
                }
            }
            b'}' => {
                if lvl > 0 {
                    lvl -= 1;
                }
                i += 1;
            }
            c => {
                if lvl == 0 {
                    out.push(c);
                }
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
