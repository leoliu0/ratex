//! .bib database parsing replicating bibtex.web's reading semantics:
//! whitespace-run compression inside values, brace protection, macro
//! substitution, `#` concatenation.

use crate::classes::{is_alpha, is_white, lex_class, LexClass};
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct RawEntry {
    pub etype: String, // lowercased
    pub key: String,
    /// 1-based line where the entry started (for `Repeated entry` errors).
    pub line: usize,
    pub fields: Vec<(String, Vec<u8>)>,
    /// declared field names that appeared more than once (web's
    /// `I'm ignoring KEY's extra "F" field` warnings, emitted at read time)
    pub dup_fields: Vec<String>,
}

#[derive(Debug, Default)]
pub struct BibFile {
    pub entries: Vec<RawEntry>,
    pub strings: Vec<(String, Vec<u8>)>, // macro definitions in order
    pub preamble_parts: Vec<Vec<u8>>,
    /// parse-level warnings ("line N: ...")
    pub warnings: Vec<String>,
}
pub struct BibParser<'a> {
    b: &'a [u8],
    i: usize,
    pub macros: HashMap<String, Vec<u8>>,
    pub warnings: Vec<String>,
    line: usize,
}

impl<'a> BibParser<'a> {
    pub fn new(text: &'a str, macros: HashMap<String, Vec<u8>>) -> Self {
        BibParser {
            b: text.as_bytes(),
            i: 0,
            macros,
            warnings: Vec::new(),
            line: 1,
        }
    }

    fn warn(&mut self, msg: String) {
        self.warnings.push(format!("line {}: {}", self.line, msg));
    }

    fn cur(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn bump(&mut self) {
        if self.cur() == Some(b'\n') {
            self.line += 1;
        }
        self.i += 1;
    }

    /// Skip whitespace (space, tab, newline, and other illegal/control chars
    /// are treated as skippable inter-token junk; bibtex eats white_space).
    fn skip_white(&mut self) {
        while let Some(c) = self.cur() {
            if c == b'\t' || c == b' ' || c == b'\n' || c == b'\r' || c < 32 {
                self.bump();
            } else {
                break;
            }
        }
    }

    /// Parse the whole file, extending `macros` with @string definitions.
    pub fn parse(mut self) -> BibFile {
        let mut out = BibFile::default();
        loop {
            self.skip_white();
            let Some(c) = self.cur() else { break };
            if c != b'@' {
                // stray junk between entries: skip it (bibtex complains; we ignore)
                self.bump();
                continue;
            }
            self.bump(); // consume '@'
            self.skip_white();
            let name = self.scan_ident();
            let lname = name.to_ascii_lowercase();
            self.skip_white();
            match lname.as_str() {
                "comment" => {
                    // bibtex.web: the comment command itself is flushed;
                    // everything up to the next `@` is junk anyway.
                }
                "preamble" => {
                    let open = self.cur();
                    if open == Some(b'{') || open == Some(b'(') {
                        self.bump();
                        let close = if open == Some(b'{') { b'}' } else { b')' };
                        let val = self.scan_value(close);
                        out.preamble_parts.push(val);
                        self.skip_white();
                        if self.cur() == Some(close) {
                            self.bump();
                        }
                    }
                }
                "string" => {
                    let open = self.cur();
                    if open == Some(b'{') || open == Some(b'(') {
                        self.bump();
                        let close = if open == Some(b'{') { b'}' } else { b')' };
                        self.skip_white();
                        let mname = self.scan_ident().to_ascii_lowercase();
                        self.skip_white();
                        if self.cur() == Some(b'=') {
                            self.bump();
                            let val = self.scan_value(close);
                            self.macros.insert(mname, val);
                        }
                        self.skip_white();
                        if self.cur() == Some(close) {
                            self.bump();
                        }
                    }
                }
                _ => {
                    // an entry
                    let open = self.cur();
                    if open != Some(b'{') && open != Some(b'(') {
                        self.warn(format!("entry type `{}` has no delimiter", name));
                        continue;
                    }
                    self.bump();
                    let close = if open == Some(b'{') { b'}' } else { b')' };
                    self.skip_white();
                    // entry key: everything up to ',' or close delimiter
                    let mut key = Vec::new();
                    while let Some(c) = self.cur() {
                        if c == b',' || c == close {
                            break;
                        }
                        key.push(c);
                        self.bump();
                    }
                    let key = String::from_utf8_lossy(&key).trim().to_string();
                    let mut ent = RawEntry {
                        etype: lname,
                        key,
                        line: self.line,
                        fields: Vec::new(),
                        dup_fields: Vec::new(),
                    };
                    // fields
                    loop {
                        self.skip_white();
                        let Some(c) = self.cur() else { break };
                        if c == close {
                            self.bump();
                            break;
                        }
                        if c == b',' {
                            self.bump();
                            continue;
                        }
                        // field name
                        let fname = self.scan_ident().to_ascii_lowercase();
                        self.skip_white();
                        if self.cur() != Some(b'=') {
                            self.warn(format!(
                                "missing `=` after field `{}` in `{}`",
                                fname, ent.key
                            ));
                            // skip to next comma/close
                            while let Some(c) = self.cur() {
                                if c == b',' || c == close {
                                    break;
                                }
                                self.bump();
                            }
                            continue;
                        }
                        self.bump();
                        let val = self.scan_value(close);
                        if ent.fields.iter().any(|(n, _)| *n == fname) {
                            ent.dup_fields.push(fname);
                        } else {
                            ent.fields.push((fname, val));
                        }
                    }
                    out.entries.push(ent);
                }
            }
        }
        out.strings = self
            .macros
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.warnings = self.warnings;
        out
    }

    /// Scan an identifier (letters, digits, and bibtex's extended set).
    fn scan_ident(&mut self) -> String {
        let mut s = Vec::new();
        while let Some(c) = self.cur() {
            if is_alpha(c)
                || c.is_ascii_digit()
                || c == b'.'
                || c == b'_'
                || c == b':'
                || c == b'+'
                || c == b'-'
                || c == b'/'
                || c == b'$'
            {
                s.push(c);
                self.bump();
            } else {
                break;
            }
        }
        String::from_utf8_lossy(&s).into_owned()
    }

    /// Scan a field value: parts joined by `#`; each part is {braced},
    /// "quoted", a number, or a macro name. `close` is the entry's outer
    /// delimiter so we don't overrun.
    fn scan_value(&mut self, close: u8) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        loop {
            self.skip_white();
            let Some(c) = self.cur() else { break };
            match c {
                b'{' => {
                    self.bump();
                    self.scan_braced_value(&mut out);
                }
                b'"' => {
                    self.bump();
                    self.scan_quoted_value(&mut out);
                }
                b'0'..=b'9' => {
                    while let Some(d) = self.cur() {
                        if d.is_ascii_digit() {
                            out.push(d);
                            self.bump();
                        } else {
                            break;
                        }
                    }
                }
                _ => {
                    // macro name
                    let mname = self.scan_ident().to_ascii_lowercase();
                    if mname.is_empty() {
                        // give up to avoid looping
                        self.bump();
                        continue;
                    }
                    match self.macros.get(&mname) {
                        Some(v) => {
                            // compress leading whitespace of first token if
                            // this is the first part of the value
                            let mut vv: &[u8] = v;
                            if out.is_empty() && !vv.is_empty() && is_white(vv[0]) {
                                vv = trim_leading_white(vv);
                                // a leading white in the macro becomes a
                                // single space only if value nonempty after
                            }
                            // compress double space at junction
                            if !out.is_empty()
                                && out.last() == Some(&b' ')
                                && !vv.is_empty()
                                && is_white(vv[0])
                            {
                                vv = &vv[1..];
                            }
                            out.extend_from_slice(vv);
                        }
                        None => {
                            self.warnings
                                .push(format!("line {}: undefined macro `{}`", self.line, mname));
                        }
                    }
                }
            }
            self.skip_white();
            if self.cur() == Some(b'#') {
                self.bump();
                continue;
            }
            let _ = close;
            break;
        }
        out
    }

    /// Scan a braced value, compressing whitespace runs to single spaces,
    /// braces nested verbatim, `@` protected.
    fn scan_braced_value(&mut self, out: &mut Vec<u8>) {
        let mut depth = 0usize;
        // leading whitespace right after `{` -> one space (bibtex behavior)
        if matches!(
            self.cur(),
            Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
        ) {
            out.push(b' ');
            self.skip_ws_bytes();
        }
        while let Some(c) = self.cur() {
            if depth == 0 && c == b'}' {
                self.bump();
                return;
            }
            match c {
                b'{' => {
                    depth += 1;
                    out.push(b'{');
                    self.bump();
                }
                b'}' => {
                    depth -= 1;
                    out.push(b'}');
                    self.bump();
                }
                b' ' | b'\t' | b'\n' | b'\r' => {
                    out.push(b' ');
                    self.skip_ws_bytes();
                }
                _ => {
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    /// Scan a quoted value; braces protect the closing quote.
    fn scan_quoted_value(&mut self, out: &mut Vec<u8>) {
        let mut depth = 0usize;
        if matches!(
            self.cur(),
            Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
        ) {
            out.push(b' ');
            self.skip_ws_bytes();
        }
        while let Some(c) = self.cur() {
            if depth == 0 && c == b'"' {
                self.bump();
                return;
            }
            match c {
                b'{' => {
                    depth += 1;
                    out.push(b'{');
                    self.bump();
                }
                b'}' => {
                    depth = depth.saturating_sub(1);
                    out.push(b'}');
                    self.bump();
                }
                b' ' | b'\t' | b'\n' | b'\r' => {
                    out.push(b' ');
                    self.skip_ws_bytes();
                }
                _ => {
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    fn skip_ws_bytes(&mut self) {
        while let Some(c) = self.cur() {
            if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                self.bump();
            } else {
                break;
            }
        }
    }
}

fn trim_leading_white(v: &[u8]) -> &[u8] {
    let mut k = 0;
    while k < v.len() && lex_class(v[k]) == LexClass::WhiteSpace {
        k += 1;
    }
    &v[k..]
}
