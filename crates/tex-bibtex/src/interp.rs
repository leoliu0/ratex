//! The bibtex virtual machine: literal stack, variables, and every
//! built-in function, following bibtex.web.

use crate::bst::Tok;
use crate::classes::*;
use crate::names;
use crate::outbuf::OutBuf;
use std::collections::{HashMap, HashSet};

pub const ENT_STR_SIZE: i64 = 100;
pub const GLOB_STR_SIZE: i64 = 1000;

#[derive(Clone, Debug)]
pub enum Lit {
    Str(String),
    Int(i64),
    /// A quoted name reference ('foo).
    Fn(String),
    /// An anonymous braced procedure literal ({...}).
    Proc(std::rc::Rc<Vec<Tok>>),
    /// A missing field; the payload is the field name (used in warnings).
    Missing(String),
}

impl Lit {
    fn type_name(&self) -> &'static str {
        match self {
            Lit::Str(_) => "string",
            Lit::Int(_) => "integer",
            Lit::Fn(_) | Lit::Proc(_) => "function",
            Lit::Missing(_) => "missing field",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Entry {
    pub etype: String,
    pub fields: HashMap<String, String>,
    pub evars: HashMap<String, String>,
    pub eints: HashMap<String, i64>,
}

pub struct Interp {
    pub fns: HashMap<String, Vec<Tok>>,
    pub globals_i: HashMap<String, i64>,
    pub globals_s: HashMap<String, String>,
    pub macros: HashMap<String, String>,
    pub fields: HashSet<String>,
    pub entry_str_vars: HashSet<String>,
    pub entry_int_vars: HashSet<String>,
    pub entries: HashMap<String, Entry>,
    pub cites: Vec<String>,
    pub orig_idx: HashMap<String, usize>,
    pub stack: Vec<Lit>,
    pub out: OutBuf,
    pub blg: Vec<String>,
    pub warnings: usize,
    pub errors: usize,
    pub preamble: String,
    pub line_len_violations: usize,
    cur: Option<String>,
}

type R = Result<(), String>;

impl Default for Interp {
    fn default() -> Self {
        Self::new()
    }
}

impl Interp {
    pub fn new() -> Self {
        let mut globals_i: HashMap<String, i64> = HashMap::new();
        // bibtex.web pre-defines these int_global_vars to the string sizes
        globals_i.insert("entry.max$".into(), ENT_STR_SIZE);
        globals_i.insert("global.max$".into(), GLOB_STR_SIZE);
        Interp {
            fns: HashMap::new(),
            globals_i,
            globals_s: HashMap::new(),
            macros: HashMap::new(),
            fields: HashSet::new(),
            entry_str_vars: HashSet::new(),
            entry_int_vars: HashSet::new(),
            entries: HashMap::new(),
            cites: Vec::new(),
            orig_idx: HashMap::new(),
            stack: Vec::new(),
            out: OutBuf::new(),
            blg: Vec::new(),
            warnings: 0,
            errors: 0,
            preamble: String::new(),
            line_len_violations: 0,
            cur: None,
        }
    }

    pub fn warn(&mut self, msg: &str) {
        self.warnings += 1;
        self.blg.push(format!("Warning--{}", msg));
    }

    fn wrong_stk_lit(&mut self, lit: &Lit, want: &str) {
        self.warnings += 1;
        self.blg
            .push(format!("Warning--{} isn't a {}", lit_desc(lit), want));
    }

    fn push(&mut self, l: Lit) {
        self.stack.push(l);
    }

    fn pop(&mut self) -> Result<Lit, String> {
        self.stack.pop().ok_or_else(|| "literal stack empty".into())
    }

    fn pop_str(&mut self) -> Result<String, String> {
        match self.pop()? {
            Lit::Str(s) => Ok(s),
            other => {
                self.wrong_stk_lit(&other, "string literal");
                Ok(String::new())
            }
        }
    }

    fn pop_int(&mut self) -> Result<i64, String> {
        match self.pop()? {
            Lit::Int(n) => Ok(n),
            other => {
                self.wrong_stk_lit(&other, "integer literal");
                Ok(0)
            }
        }
    }

    pub fn exec_fn(&mut self, name: &str) -> R {
        let lname = name.to_ascii_lowercase();
        if is_builtin(&lname) {
            return self.exec_builtin(&lname);
        }
        if let Some(body) = self.fns.get(&lname) {
            let body = body.clone();
            return self
                .exec_body(&body)
                .map_err(|e| format!("{}, in function `{}`", e, lname));
        }
        // bare identifier: push variable / field / macro values
        self.push_ident(&lname)
    }

    fn push_ident(&mut self, name: &str) -> R {
        if self.entry_int_vars.contains(name) {
            let v = match &self.cur {
                Some(k) => self
                    .entries
                    .get(k)
                    .and_then(|e| e.eints.get(name))
                    .copied()
                    .unwrap_or(0),
                None => 0,
            };
            self.push(Lit::Int(v));
            return Ok(());
        }
        if self.entry_str_vars.contains(name) {
            let v = match &self.cur {
                Some(k) => self
                    .entries
                    .get(k)
                    .and_then(|e| e.evars.get(name))
                    .cloned()
                    .unwrap_or_default(),
                None => String::new(),
            };
            self.push(Lit::Str(v));
            return Ok(());
        }
        if self.globals_i.contains_key(name) {
            self.push(Lit::Int(self.globals_i[name]));
            return Ok(());
        }
        if self.globals_s.contains_key(name) {
            self.push(Lit::Str(self.globals_s[name].clone()));
            return Ok(());
        }
        if self.fields.contains(name) {
            match &self.cur.clone() {
                Some(k) => match self.entries.get(k).and_then(|e| e.fields.get(name)) {
                    Some(v) => self.push(Lit::Str(v.clone())),
                    None => self.push(Lit::Missing(name.to_string())),
                },
                None => {
                    self.cant_mess();
                    self.push(Lit::Missing(name.to_string()));
                }
            }
            return Ok(());
        }
        if let Some(v) = self.macros.get(name) {
            self.push(Lit::Str(v.clone()));
            return Ok(());
        }
        self.warnings += 1;
        self.blg.push(format!(
            "Warning--I didn't find a database entry for `{}`",
            name
        ));
        self.push(Lit::Str(String::new()));
        Ok(())
    }

    fn cant_mess(&mut self) {
        self.warnings += 1;
        self.blg
            .push("Warning--you can't mess with entries in between \\read commands".into());
    }

    pub fn exec_body(&mut self, body: &[Tok]) -> R {
        for tok in body {
            match tok {
                Tok::Str(s) => self.push(Lit::Str(s.clone())),
                Tok::Int(n) => self.push(Lit::Int(*n as i64)),
                Tok::FnRef(f) => self.push(Lit::Fn(f.clone())),
                Tok::Proc(b) => self.push(Lit::Proc(b.clone())),
                Tok::Ident(id) => {
                    // control flow needs stack discipline: execute directly
                    if id == "if$" || id == "while$" || is_builtin(id) || self.fns.contains_key(id)
                    {
                        self.exec_fn(id)?;
                    } else {
                        self.push_ident(id)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn sort_key_of(&self, cite: &str) -> Vec<u8> {
        self.entries
            .get(cite)
            .and_then(|e| e.evars.get("sort.key$"))
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default()
    }

    pub fn sort_cites(&mut self) {
        let entries = &self.entries;
        let orig = &self.orig_idx;
        let key = |c: &String| -> Vec<u8> {
            entries
                .get(c)
                .and_then(|e| e.evars.get("sort.key$"))
                .map(|s| s.as_bytes().to_vec())
                .unwrap_or_default()
        };
        let mut keyed: Vec<(Vec<u8>, usize, String)> = self
            .cites
            .iter()
            .map(|c| (key(c), orig[c], c.clone()))
            .collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        self.cites = keyed.into_iter().map(|(_, _, c)| c).collect();
    }

    pub fn iterate(&mut self, fname: &str, reverse: bool) -> R {
        let order: Vec<String> = if reverse {
            self.cites.iter().rev().cloned().collect()
        } else {
            self.cites.clone()
        };
        for cite in order {
            self.cur = Some(cite);
            self.exec_fn(fname)?;
        }
        self.cur = None;
        Ok(())
    }

    pub fn execute(&mut self, fname: &str) -> R {
        self.cur = None;
        self.exec_fn(fname)
    }

    // ---------------- built-ins ----------------

    fn exec_builtin(&mut self, name: &str) -> R {
        match name {
            "=" => self.b_equals(),
            ">" => self.b_gt(),
            "<" => self.b_lt(),
            "+" => self.b_plus(),
            "-" => self.b_minus(),
            "*" => self.b_concat(),
            ":=" => self.b_gets(),
            "add.period$" => self.b_add_period(),
            "call.type$" => self.b_call_type(),
            "change.case$" => self.b_change_case(),
            "chr.to.int$" => self.b_chr_to_int(),
            "cite$" => self.b_cite(),
            "duplicate$" => self.b_duplicate(),
            "empty$" => self.b_empty(),
            "format.name$" => self.b_format_name(),
            "if$" => self.b_if(),
            "int.to.chr$" => self.b_int_to_chr(),
            "int.to.str$" => self.b_int_to_str(),
            "missing$" => self.b_missing(),
            "newline$" => {
                self.out.flush_line();
                Ok(())
            }
            "num.names$" => self.b_num_names(),
            "preamble$" => {
                let p = self.preamble.clone();
                self.push(Lit::Str(p));
                Ok(())
            }
            "pop$" => {
                self.pop()?;
                Ok(())
            }
            "purify$" => {
                let s = self.pop_str()?;
                self.push(Lit::Str(purify(&s)));
                Ok(())
            }
            "quote$" => {
                self.push(Lit::Str("\"".into()));
                Ok(())
            }
            "skip$" => Ok(()),
            "stack$" => {
                while self.stack.pop().is_some() {}
                Ok(())
            }
            "substring$" => self.b_substring(),
            "swap$" => self.b_swap(),
            "text.length$" => {
                let s = self.pop_str()?;
                self.push(Lit::Int(text_length(&s)));
                Ok(())
            }
            "text.prefix$" => self.b_text_prefix(),
            "top$" => {
                if let Some(l) = self.stack.pop() {
                    self.blg.push(lit_desc(&l).to_string());
                }
                Ok(())
            }
            "type$" => self.b_type(),
            "warning$" => {
                let s = self.pop_str()?;
                self.warn(&s);
                Ok(())
            }
            "while$" => self.b_while(),
            "width$" => {
                let s = self.pop_str()?;
                self.push(Lit::Int(s.len() as i64));
                Ok(())
            }
            "write$" => {
                let s = self.pop_str()?;
                self.out.add_out_pool(&s);
                Ok(())
            }
            _ => Err(format!("unknown built-in function {}", name)),
        }
    }

    fn b_equals(&mut self) -> R {
        let a = self.pop()?;
        let b = self.pop()?;
        let eq = match (&a, &b) {
            (Lit::Str(x), Lit::Str(y)) => x == y,
            (Lit::Int(x), Lit::Int(y)) => x == y,
            (Lit::Missing(_), Lit::Missing(_)) => {
                self.warnings += 1;
                self.blg
                    .push("Warning--missing field(s) compared with `=`".into());
                false
            }
            _ => {
                self.warnings += 1;
                self.blg.push(format!(
                    "Warning--they aren't the same literal types ({} vs {})",
                    a.type_name(),
                    b.type_name()
                ));
                false
            }
        };
        self.push(Lit::Int(eq as i64));
        Ok(())
    }

    fn b_gt(&mut self) -> R {
        let a = self.pop_int()?;
        let b = self.pop_int()?;
        self.push(Lit::Int((b > a) as i64));
        Ok(())
    }

    fn b_lt(&mut self) -> R {
        let a = self.pop_int()?;
        let b = self.pop_int()?;
        self.push(Lit::Int((b < a) as i64));
        Ok(())
    }

    fn b_plus(&mut self) -> R {
        let a = self.pop_int()?;
        let b = self.pop_int()?;
        self.push(Lit::Int(b.wrapping_add(a)));
        Ok(())
    }

    fn b_minus(&mut self) -> R {
        let a = self.pop_int()?;
        let b = self.pop_int()?;
        self.push(Lit::Int(b.wrapping_sub(a)));
        Ok(())
    }

    fn b_concat(&mut self) -> R {
        let a = self.pop_str()?;
        let b = self.pop_str()?;
        let mut s = b;
        s.push_str(&a);
        self.push(Lit::Str(s));
        Ok(())
    }

    fn b_gets(&mut self) -> R {
        let name = match self.pop()? {
            Lit::Fn(n) => n,
            other => {
                self.wrong_stk_lit(&other, "function literal");
                return Ok(());
            }
        };
        let value = self.pop()?;
        let lname = name.to_ascii_lowercase();
        if lname == "sort.key$" || self.entry_str_vars.contains(&lname) {
            let s = match value {
                Lit::Str(s) => s,
                Lit::Missing(_) => String::new(),
                other => {
                    self.wrong_stk_lit(&other, "string literal");
                    String::new()
                }
            };
            let mut s = s;
            let ent_max = *self.globals_i.get("entry.max$").unwrap_or(&ENT_STR_SIZE);
            if s.len() as i64 > ent_max {
                self.warn(&format!(
                    "string size exceeded for entry string variable `{}`",
                    lname
                ));
                // truncate at a byte boundary (ASCII styles are unaffected)
                let mut cut = ent_max.max(0) as usize;
                while cut > 0 && !s.is_char_boundary(cut) {
                    cut -= 1;
                }
                s.truncate(cut);
            }
            self.assign_entry_str(&lname, s);
            return Ok(());
        }
        if self.entry_int_vars.contains(&lname) {
            let n = match value {
                Lit::Int(n) => n,
                other => {
                    self.wrong_stk_lit(&other, "integer literal");
                    0
                }
            };
            if let Some(k) = &self.cur {
                if let Some(e) = self.entries.get_mut(k) {
                    e.eints.insert(lname, n);
                }
            }
            return Ok(());
        }
        if self.globals_s.contains_key(&lname) {
            let s = match value {
                Lit::Str(s) => s,
                other => {
                    self.wrong_stk_lit(&other, "string literal");
                    String::new()
                }
            };
            let mut s = s;
            let glob_max = *self.globals_i.get("global.max$").unwrap_or(&GLOB_STR_SIZE);
            if s.len() as i64 > glob_max {
                self.warn("string size exceeded for global string variable");
                let mut cut = glob_max.max(0) as usize;
                while cut > 0 && !s.is_char_boundary(cut) {
                    cut -= 1;
                }
                s.truncate(cut);
            }
            self.globals_s.insert(lname, s);
            return Ok(());
        }
        if self.globals_i.contains_key(&lname) {
            let n = match value {
                Lit::Int(n) => n,
                other => {
                    self.wrong_stk_lit(&other, "integer literal");
                    0
                }
            };
            self.globals_i.insert(lname, n);
            return Ok(());
        }
        self.warnings += 1;
        self.blg
            .push(format!("Warning--you can't assign to type `{}`", lname));
        Ok(())
    }

    fn assign_entry_str(&mut self, name: &str, val: String) {
        if let Some(e) = self.cur.as_ref().and_then(|key| self.entries.get_mut(key)) {
            e.evars.insert(name.to_string(), val);
        }
        // no current entry: value dropped (bibtex would have complained)
    }

    fn b_add_period(&mut self) -> R {
        let s = self.pop_str()?;
        if s.is_empty() {
            self.push(Lit::Str(s));
            return Ok(());
        }
        let b = s.as_bytes();
        let mut i = b.len();
        while i > 0 {
            i -= 1;
            if b[i] != b'}' {
                break;
            }
        }
        let c = b[i];
        if c == b'.' || c == b'?' || c == b'!' {
            self.push(Lit::Str(s));
        } else {
            let mut t = s;
            t.push('.');
            self.push(Lit::Str(t));
        }
        Ok(())
    }

    fn b_call_type(&mut self) -> R {
        let Some(k) = self.cur.clone() else {
            self.cant_mess();
            return Ok(());
        };
        let Some(e) = self.entries.get(&k) else {
            return Ok(());
        };
        let et = e.etype.clone();
        if self.fns.contains_key(&et) {
            self.exec_fn(&et)
        } else if self.fns.contains_key("default.type") {
            self.exec_fn("default.type")
        } else {
            Ok(())
        }
    }

    fn b_change_case(&mut self) -> R {
        let conv_s = self.pop_str()?;
        let s = self.pop_str()?;
        let conv = match conv_s.as_bytes().first() {
            Some(b't') | Some(b'T') if conv_s.len() == 1 => Conv::Title,
            Some(b'l') | Some(b'L') if conv_s.len() == 1 => Conv::Lower,
            Some(b'u') | Some(b'U') if conv_s.len() == 1 => Conv::Upper,
            _ => {
                self.warn(&format!(
                    "`{}` is an illegal case-conversion string",
                    conv_s
                ));
                Conv::Bad
            }
        };
        self.push(Lit::Str(change_case(&s, conv)));
        Ok(())
    }

    fn b_chr_to_int(&mut self) -> R {
        let s = self.pop_str()?;
        if s.len() == 1 {
            self.push(Lit::Int(s.as_bytes()[0] as i64));
        } else {
            self.warnings += 1;
            self.blg
                .push(format!("Warning--\"{}\" isn't a single character", s));
            self.push(Lit::Int(0));
        }
        Ok(())
    }

    fn b_cite(&mut self) -> R {
        match self.cur.clone() {
            Some(k) => {
                self.push(Lit::Str(k));
                Ok(())
            }
            None => {
                self.cant_mess();
                self.push(Lit::Str(String::new()));
                Ok(())
            }
        }
    }

    fn b_duplicate(&mut self) -> R {
        let l = self.pop()?;
        self.push(l.clone());
        self.push(l);
        Ok(())
    }

    fn b_empty(&mut self) -> R {
        match self.pop()? {
            Lit::Missing(_) => self.push(Lit::Int(1)),
            Lit::Str(s) => {
                let all_white = s.as_bytes().iter().all(|&c| is_white(c));
                self.push(Lit::Int(all_white as i64));
            }
            other => {
                self.wrong_stk_lit(&other, "string literal");
                self.push(Lit::Int(0));
            }
        }
        Ok(())
    }

    fn b_format_name(&mut self) -> R {
        let fmt = self.pop_str()?;
        let which = self.pop_int()?;
        let list = self.pop_str()?;
        self.push(Lit::Str(names::format_name(&fmt, which, &list)));
        Ok(())
    }

    fn b_if(&mut self) -> R {
        let f1 = self.pop()?;
        let f2 = self.pop()?;
        let cond = self.pop()?;
        if !is_function_lit(&f1) || !is_function_lit(&f2) {
            self.wrong_stk_lit(&f1, "function literal");
            self.wrong_stk_lit(&f2, "function literal");
            return Ok(());
        }
        let truth = match cond {
            Lit::Int(n) => n > 0,
            other => {
                self.wrong_stk_lit(&other, "integer literal");
                return Ok(());
            }
        };
        if truth {
            self.exec_fn_lit(&f2)
        } else {
            self.exec_fn_lit(&f1)
        }
    }

    fn b_while(&mut self) -> R {
        let body = self.pop()?;
        let test = self.pop()?;
        if !is_function_lit(&body) || !is_function_lit(&test) {
            self.wrong_stk_lit(&body, "function literal");
            self.wrong_stk_lit(&test, "function literal");
            return Ok(());
        };
        loop {
            self.exec_fn_lit(&test)?;
            let n = match self.pop()? {
                Lit::Int(n) => n,
                other => {
                    self.wrong_stk_lit(&other, "integer literal");
                    return Ok(());
                }
            };
            if n > 0 {
                self.exec_fn_lit(&body)?;
            } else {
                return Ok(());
            }
        }
    }

    fn b_int_to_chr(&mut self) -> R {
        let n = self.pop_int()?;
        if (0..=127).contains(&n) {
            self.push(Lit::Str((n as u8 as char).to_string()));
        } else {
            self.warnings += 1;
            self.blg.push(format!("Warning--{} isn't valid ASCII", n));
            self.push(Lit::Str(String::new()));
        }
        Ok(())
    }

    fn b_int_to_str(&mut self) -> R {
        let n = self.pop_int()?;
        if n < 0 {
            self.warnings += 1;
            self.blg.push(format!("Warning--integer {} is negative", n));
            self.push(Lit::Str(String::new()));
        } else {
            self.push(Lit::Str(n.to_string()));
        }
        Ok(())
    }

    fn b_missing(&mut self) -> R {
        match self.pop()? {
            Lit::Missing(_) => self.push(Lit::Int(1)),
            _ => self.push(Lit::Int(0)),
        }
        Ok(())
    }

    fn b_num_names(&mut self) -> R {
        let s = self.pop_str()?;
        self.push(Lit::Int(names::num_names(&s)));
        Ok(())
    }

    fn b_substring(&mut self) -> R {
        let len = self.pop_int()?;
        let start = self.pop_int()?;
        let s = self.pop_str()?;
        let slen = s.len() as i64;
        let sub: &[u8] = if len >= slen && (start == 1 || start == -1) {
            s.as_bytes()
        } else if len <= 0 || start == 0 || start > slen || start < -slen {
            &[]
        } else if start > 0 {
            let from = (start - 1) as usize;
            let take = (len as usize).min(slen as usize - from);
            &s.as_bytes()[from..from + take]
        } else {
            let a = (-start) as usize;
            let end = slen as usize - (a - 1);
            let take = (len as usize).min(end);
            &s.as_bytes()[end - take..end]
        };
        self.push(Lit::Str(String::from_utf8_lossy(sub).into_owned()));
        Ok(())
    }

    fn b_swap(&mut self) -> R {
        let a = self.pop()?;
        let b = self.pop()?;
        self.push(a);
        self.push(b);
        Ok(())
    }

    fn b_text_prefix(&mut self) -> R {
        let n = self.pop_int()?;
        let s = self.pop_str()?;
        self.push(Lit::Str(text_prefix(&s, n)));
        Ok(())
    }

    fn b_type(&mut self) -> R {
        match self.cur.clone() {
            Some(k) => {
                let t = self
                    .entries
                    .get(&k)
                    .map(|e| e.etype.clone())
                    .unwrap_or_default();
                self.push(Lit::Str(t));
                Ok(())
            }
            None => {
                self.cant_mess();
                self.push(Lit::Str(String::new()));
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Conv {
    Title,
    Lower,
    Upper,
    Bad,
}

fn is_function_lit(l: &Lit) -> bool {
    matches!(l, Lit::Fn(_) | Lit::Proc(_))
}

impl Interp {
    /// Execute a function literal (named reference or inline procedure).
    fn exec_fn_lit(&mut self, l: &Lit) -> R {
        match l {
            Lit::Fn(n) => self.exec_fn(n),
            Lit::Proc(b) => self.exec_body(b),
            _ => Ok(()),
        }
    }
}

fn lit_desc(l: &Lit) -> String {
    match l {
        Lit::Str(s) => format!("\"{}\"", s),
        Lit::Int(n) => n.to_string(),
        Lit::Fn(f) => format!("`{}'", f),
        Lit::Proc(_) => "{procedure}".to_string(),
        Lit::Missing(f) => format!("missing field `{}`", f),
    }
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "=" | ">"
            | "<"
            | "+"
            | "-"
            | "*"
            | ":="
            | "add.period$"
            | "call.type$"
            | "change.case$"
            | "chr.to.int$"
            | "cite$"
            | "duplicate$"
            | "empty$"
            | "format.name$"
            | "if$"
            | "int.to.chr$"
            | "int.to.str$"
            | "missing$"
            | "newline$"
            | "num.names$"
            | "preamble$"
            | "pop$"
            | "purify$"
            | "quote$"
            | "skip$"
            | "stack$"
            | "substring$"
            | "swap$"
            | "text.length$"
            | "text.prefix$"
            | "top$"
            | "type$"
            | "warning$"
            | "while$"
            | "width$"
            | "write$"
    )
}

/// change.case$: full port including special-character handling.
fn change_case(s: &str, conv: Conv) -> String {
    let mut buf = s.as_bytes().to_vec();
    let mut level = 0i32;
    let mut prev_colon = false;
    let mut i = 0usize;
    while i < buf.len() {
        if buf[i] == b'{' {
            level += 1;
            let mut do_special = level == 1 && i + 4 <= buf.len() && buf[i + 1] == b'\\';
            if do_special && conv == Conv::Title && (i == 0 || (prev_colon && is_white(buf[i - 1])))
            {
                do_special = false;
            }
            if do_special {
                convert_special(&mut buf, &mut i, &mut level, conv);
            }
            prev_colon = false;
            i += 1;
        } else if buf[i] == b'}' {
            if level > 0 {
                level -= 1;
            }
            prev_colon = false;
            i += 1;
        } else {
            if level == 0 {
                match conv {
                    Conv::Title => {
                        if i != 0 && !(prev_colon && is_white(buf[i - 1])) {
                            buf[i] = to_lower(buf[i]);
                        }
                        if buf[i] == b':' {
                            prev_colon = true;
                        } else if !is_white(buf[i]) {
                            prev_colon = false;
                        }
                    }
                    Conv::Lower => buf[i] = to_lower(buf[i]),
                    Conv::Upper => buf[i] = to_upper(buf[i]),
                    Conv::Bad => {}
                }
            }
            i += 1;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Convert a special character: `i` is at the `{` (which raised the level
/// to 1). On return `i` is at the matching `}` and the level is 1.
fn convert_special(buf: &mut Vec<u8>, i: &mut usize, level: &mut i32, conv: Conv) {
    *i += 1; // at the backslash
    while *i < buf.len() && *level > 0 {
        *i += 1; // skip the backslash
        let x = *i;
        while *i < buf.len() && is_alpha(buf[*i]) {
            *i += 1;
        }
        let cs = buf[x..*i].to_vec();
        if let Some(k) = lookup_ctrl_seq(&cs) {
            match conv {
                Conv::Title | Conv::Lower => {
                    if matches!(
                        k,
                        CtrlSeq::LUpper
                            | CtrlSeq::OUpper
                            | CtrlSeq::OeUpper
                            | CtrlSeq::AeUpper
                            | CtrlSeq::AaUpper
                    ) {
                        for b in &mut buf[x..*i] {
                            *b = to_lower(*b);
                        }
                    }
                }
                Conv::Upper => match k {
                    CtrlSeq::L | CtrlSeq::O | CtrlSeq::Oe | CtrlSeq::Ae | CtrlSeq::Aa => {
                        for b in &mut buf[x..*i] {
                            *b = to_upper(*b);
                        }
                    }
                    CtrlSeq::I | CtrlSeq::J | CtrlSeq::Ss => {
                        // convert, then remove the backslash and any
                        // following white space
                        for b in &mut buf[x..*i] {
                            *b = to_upper(*b);
                        }
                        let mut xp = x;
                        while xp < *i {
                            buf[xp - 1] = buf[xp];
                            xp += 1;
                        }
                        xp -= 1;
                        while *i < buf.len() && is_white(buf[*i]) {
                            *i += 1;
                        }
                        let shift = *i - xp;
                        for t in *i..buf.len() {
                            buf[t - shift] = buf[t];
                        }
                        buf.truncate(buf.len() - shift);
                        *i = xp;
                    }
                    _ => {}
                },
                Conv::Bad => {}
            }
        }
        let mut x = *i;
        while x < buf.len() && *level > 0 && buf[x] != b'\\' {
            if buf[x] == b'}' {
                *level -= 1;
            } else if buf[x] == b'{' {
                *level += 1;
            }
            x += 1;
        }
        // convert a non-control sequence
        match conv {
            Conv::Title | Conv::Lower => {
                for b in &mut buf[*i..x] {
                    *b = to_lower(*b);
                }
            }
            Conv::Upper => {
                for b in &mut buf[*i..x] {
                    *b = to_upper(*b);
                }
            }
            Conv::Bad => {}
        }
        *i = x;
    }
    // i is now at the matching `}` (or end); the caller advances past it
}

/// purify$
pub fn purify(s: &str) -> String {
    let input = s.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut level = 0i32;
    let mut i = 0usize;
    while i < input.len() {
        let c = input[i];
        match lex_class(c) {
            LexClass::WhiteSpace | LexClass::SepChar => {
                out.push(b' ');
                i += 1;
            }
            LexClass::Alpha | LexClass::Numeric => {
                out.push(c);
                i += 1;
            }
            _ => {
                if c == b'{' {
                    level += 1;
                    if level == 1 && i + 1 < input.len() && input[i + 1] == b'\\' {
                        i += 1; // at the backslash
                        while i < input.len() && level > 0 {
                            i += 1; // skip the backslash
                            let ys = i;
                            while i < input.len() && is_alpha(input[i]) {
                                i += 1;
                            }
                            if let Some(k) = lookup_ctrl_seq(&input[ys..i]) {
                                out.push(input[ys]);
                                if matches!(
                                    k,
                                    CtrlSeq::Oe
                                        | CtrlSeq::OeUpper
                                        | CtrlSeq::Ae
                                        | CtrlSeq::AeUpper
                                        | CtrlSeq::Ss
                                ) {
                                    out.push(input[ys + 1]);
                                }
                            }
                            while i < input.len() && level > 0 && input[i] != b'\\' {
                                let d = input[i];
                                match lex_class(d) {
                                    LexClass::Alpha | LexClass::Numeric => out.push(d),
                                    _ => {
                                        if d == b'}' {
                                            level -= 1;
                                        } else if d == b'{' {
                                            level += 1;
                                        }
                                    }
                                }
                                i += 1;
                            }
                        }
                        i -= 1; // unskip the `}` (or last character)
                    }
                } else if c == b'}' && level > 0 {
                    level -= 1;
                }
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// text.prefix$: braces do not count as text characters, while a
/// brace-level-one special-character group counts as one character.
fn text_prefix(s: &str, n: i64) -> String {
    if n <= 0 {
        return String::new();
    }
    let b = s.as_bytes();
    let mut i = 0usize;
    let mut num = 0i64;
    let mut level = 0i32;
    while i < b.len() && num < n {
        match b[i] {
            b'{' => {
                level += 1;
                i += 1;
                if level == 1 && i < b.len() && b[i] == b'\\' {
                    while i < b.len() && level > 0 {
                        match b[i] {
                            b'{' => level += 1,
                            b'}' => level -= 1,
                            _ => {}
                        }
                        i += 1;
                    }
                    num += 1;
                }
            }
            b'}' => {
                level = (level - 1).max(0);
                i += 1;
            }
            _ => {
                i += 1;
                num += 1;
            }
        }
    }
    let mut out = b[..i].to_vec();
    out.extend(std::iter::repeat_n(b'}', level.max(0) as usize));
    String::from_utf8_lossy(&out).into_owned()
}

/// text.length$
pub fn text_length(s: &str) -> i64 {
    let b = s.as_bytes();
    let mut num = 0i64;
    let mut i = 0usize;
    let mut level = 0i32;
    while i < b.len() {
        i += 1;
        let c = b[i - 1];
        if c == b'{' {
            level += 1;
            if level == 1 && i < b.len() && b[i] == b'\\' {
                i += 1;
                while i < b.len() && level > 0 {
                    if b[i] == b'}' {
                        level -= 1;
                    } else if b[i] == b'{' {
                        level += 1;
                    }
                    i += 1;
                }
                num += 1;
            }
        } else if c == b'}' {
            if level > 0 {
                level -= 1;
            }
        } else {
            num += 1;
        }
    }
    num
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_prefix_counts_special_character_groups_once() {
        let surname = crate::names::format_name("{ll}", 1, r#"H{\"a}ggstr\"om, O."#);
        assert_eq!(text_prefix(&surname, 3), r#"H{\"a}g"#);
    }
}
