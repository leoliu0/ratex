//! .bib database file parser: entries, @string macros, @preamble.
//!
//! Follows BibTeX semantics: entry types and field names are case-folded,
//! cite keys keep their case but match case-insensitively, values may be
//! braced, quoted, numeric, bare macro names, or `#`-concatenations.

use std::collections::HashMap;

use super::Logger;

#[derive(Debug, Clone)]
pub struct BibEntry {
    pub cite: String,
    pub type_name: String, // lowercased, "" if absent
    pub fields: Vec<(String, String)>, // lowercased name -> value
}

#[derive(Debug, Default)]
pub struct Database {
    pub entries: Vec<BibEntry>,
    pub macros: HashMap<String, String>,
    pub preamble: Vec<String>,
}

impl Database {
    pub fn find_lc(&self, key: &str) -> Option<usize> {
        let lc = key.to_ascii_lowercase();
        self.entries.iter().position(|e| e.cite.to_ascii_lowercase() == lc)
    }
}

struct Scanner<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Scanner<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && (self.b[self.i].is_ascii_whitespace() || self.b[self.i] == b'%')
        {
            if self.b[self.i] == b'%' {
                while self.i < self.b.len() && self.b[self.i] != b'\n' {
                    self.i += 1;
                }
            } else {
                self.i += 1;
            }
        }
    }

    fn skip_balanced(&mut self) {
        // skips a {...} or (...) group starting at self.i
        if self.i >= self.b.len() {
            return;
        }
        let open = self.b[self.i];
        let close = if open == b'{' { b'}' } else if open == b'(' { b')' } else { return };
        let mut depth = 1usize;
        self.i += 1;
        while self.i < self.b.len() && depth > 0 {
            match self.b[self.i] {
                b'\\' => self.i += 1, // skip escaped char
                c if c == open => depth += 1,
                c if c == close => depth -= 1,
                _ => {}
            }
            self.i += 1;
        }
    }

    /// identifier (letters, digits, and a few punctuation chars)
    fn ident(&mut self) -> String {
        let start = self.i;
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.' || c == b':' || c == b'+' {
                self.i += 1;
            } else {
                break;
            }
        }
        String::from_utf8_lossy(&self.b[start..self.i]).into_owned()
    }
}

/// Parse a field value (with `#` concatenation). Returns `None` when the
/// value references an undefined macro: bibtex then treats the field as if
/// it were never given.
fn read_value(
    sc: &mut Scanner,
    db: &Database,
    log: &mut Logger,
    path: &str,
    line: &mut usize,
) -> Option<String> {
    let mut out = String::new();
    loop {
        sc.ws();
        if sc.i >= sc.b.len() {
            break;
        }
        let c = sc.b[sc.i];
        match c {
            b'{' => {
                let start = sc.i + 1;
                let mut depth = 1usize;
                sc.i += 1;
                while sc.i < sc.b.len() && depth > 0 {
                    match sc.b[sc.i] {
                        b'\\' => sc.i += 1,
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        b'\n' => *line += 1,
                        _ => {}
                    }
                    sc.i += 1;
                }
                let end = if depth == 0 { sc.i - 1 } else { sc.i };
                out.push_str(&String::from_utf8_lossy(&sc.b[start..end]));
            }
            b'"' => {
                let start = sc.i + 1;
                let mut depth = 0usize;
                sc.i += 1;
                while sc.i < sc.b.len() {
                    let ch = sc.b[sc.i];
                    if ch == b'{' {
                        depth += 1;
                    } else if ch == b'}' {
                        if depth > 0 {
                            depth -= 1;
                        }
                    } else if ch == b'"' && depth == 0 {
                        break;
                    } else if ch == b'\n' {
                        *line += 1;
                    }
                    sc.i += 1;
                }
                out.push_str(&String::from_utf8_lossy(&sc.b[start..sc.i.min(sc.b.len())]));
                sc.i += 1; // closing quote
            }
            b'0'..=b'9' => {
                let v = sc.ident();
                out.push_str(&v);
            }
            _ => {
                let name = sc.ident();
                if name.is_empty() {
                    // stray punctuation; skip a char to guarantee progress
                    sc.i += 1;
                } else {
                    match db.macros.get(&name.to_ascii_lowercase()) {
                        Some(v) => out.push_str(v),
                        None => {
                            log.warn(format!(
                                "{path}: undefined macro \"{name}\" at line {line}; dropping the field"
                            ));
                            return None;
                        }
                    }
                }
            }
        }
        sc.ws();
        if sc.i < sc.b.len() && sc.b[sc.i] == b'#' {
            sc.i += 1;
            continue;
        }
        break;
    }
    Some(out.trim().to_string())
}

/// Parse one .bib file into the database.
pub fn parse_bib(src: &str, path: &str, db: &mut Database, log: &mut Logger) {
    let mut sc = Scanner { b: src.as_bytes(), i: 0 };
    let mut line = 1usize;
    let mut seen_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    while sc.i < sc.b.len() {
        if sc.b[sc.i] != b'@' {
            if sc.b[sc.i] == b'\n' {
                line += 1;
            }
            sc.i += 1;
            continue;
        }
        sc.i += 1;
        let cmd = sc.ident().to_ascii_lowercase();
        sc.ws();
        if sc.i >= sc.b.len() || (sc.b[sc.i] != b'{' && sc.b[sc.i] != b'(') {
            continue; // malformed @command; skip
        }
        match cmd.as_str() {
            "comment" => sc.skip_balanced(),
            "string" => {
                sc.i += 1; // delimiter
                sc.ws();
                let name = sc.ident().to_ascii_lowercase();
                sc.ws();
                if sc.i < sc.b.len() && sc.b[sc.i] == b'=' {
                    sc.i += 1;
                    let val = read_value(&mut sc, db, log, path, &mut line).unwrap_or_default();
                    if db.macros.insert(name.clone(), val).is_some() {
                        log.warn(format!("{path}: redefinition of string \"{name}\""));
                    }
                }
                // skip to closing delimiter
                while sc.i < sc.b.len() && sc.b[sc.i] != b'}' && sc.b[sc.i] != b')' {
                    if sc.b[sc.i] == b'\n' {
                        line += 1;
                    }
                    sc.i += 1;
                }
                sc.i += 1;
            }
            "preamble" => {
                sc.i += 1;
                let val = read_value(&mut sc, db, log, path, &mut line).unwrap_or_default();
                db.preamble.push(val);
                while sc.i < sc.b.len() && sc.b[sc.i] != b'}' && sc.b[sc.i] != b')' {
                    sc.i += 1;
                }
                sc.i += 1;
            }
            etype => {
                sc.i += 1; // opening delimiter
                sc.ws();
                // cite key: up to ',' or closing delimiter
                let kstart = sc.i;
                while sc.i < sc.b.len() {
                    let c = sc.b[sc.i];
                    if c == b',' || c == b'}' || c == b')' || c == b'\n' {
                        break;
                    }
                    sc.i += 1;
                }
                let cite = String::from_utf8_lossy(&sc.b[kstart..sc.i]).trim().to_string();
                let mut fields: Vec<(String, String)> = Vec::new();
                loop {
                    sc.ws();
                    if sc.i >= sc.b.len() || sc.b[sc.i] == b'}' || sc.b[sc.i] == b')' {
                        break;
                    }
                    if sc.b[sc.i] == b',' {
                        sc.i += 1;
                        continue;
                    }
                    let fname = sc.ident().to_ascii_lowercase();
                    sc.ws();
                    if sc.b.get(sc.i) != Some(&b'=') {
                        // not a field assignment; skip token
                        continue;
                    }
                    sc.i += 1;
                    let val = match read_value(&mut sc, db, log, path, &mut line) {
                        Some(v) => v,
                        None => continue,
                    };
                    if fields.iter().any(|(n, _)| *n == fname) {
                        log.warn(format!(
                            "{path}: repeated field \"{fname}\" in entry \"{cite}\"; keeping first"
                        ));
                    } else {
                        fields.push((fname, val));
                    }
                }
                // consume closing delimiter
                while sc.i < sc.b.len() && sc.b[sc.i] != b'}' && sc.b[sc.i] != b')' {
                    sc.i += 1;
                }
                sc.i += 1;
                if cite.is_empty() {
                    log.warn(format!("{path}: anonymous entry at line {line} ignored"));
                    continue;
                }
                let lc = cite.to_ascii_lowercase();
                if !seen_keys.insert(lc) {
                    log.warn(format!("{path}: repeated entry \"{cite}\"; keeping first"));
                    continue;
                }
                db.entries.push(BibEntry {
                    cite,
                    type_name: etype.to_string(),
                    fields,
                });
            }
        }
    }
}
