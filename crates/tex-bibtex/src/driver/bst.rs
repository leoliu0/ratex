//! .bst (BibTeX style) tokenizer and parser.
//!
//! Produces an ordered list of top-level commands; function bodies are kept
//! as pre-tokenized instruction streams for the stack interpreter in `exec`.

/// One token inside a function body.
#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    /// `#123` integer literal
    Int(i64),
    /// `"string"` literal
    Str(String),
    /// `'name` quoted well-known-function reference
    Quoted(String),
    /// bare identifier: builtin, variable value push, or function call
    Ident(String),
    /// inline `{...}` function literal (if$/while$ branch)
    FnLit(Vec<Tok>),
}

#[derive(Clone, Debug)]
pub enum BstCmd {
    Entry {
        fields: Vec<String>,
        ints: Vec<String>,
        strs: Vec<String>,
    },
    Function(String, Vec<Tok>),
    Integers(Vec<String>),
    Strings(Vec<String>),
    Macro(String, String),
    Read,
    Sort,
    Execute(String),
    Iterate(String),
    Reverse(String),
}

pub struct BstProgram {
    pub cmds: Vec<BstCmd>,
}

fn is_bst_id_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'$' | b'-' | b'+' | b'*')
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Lexer {
            src: src.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c == b'%' {
                while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                    self.pos += 1;
                }
            } else if c.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Read a `{...}` group (verbatim contents, braces balanced).
    /// Double-quoted strings inside a .bst group may contain unbalanced
    /// braces (e.g. `"\bibitem[{"`), so the scanner skips over them.
    fn group(&mut self) -> Result<String, String> {
        self.skip_ws();
        if self.pos >= self.src.len() || self.src[self.pos] != b'{' {
            return Err(format!("expected '{{' at byte {}", self.pos));
        }
        self.pos += 1;
        let start = self.pos;
        let mut depth = 1;
        while self.pos < self.src.len() {
            match self.src[self.pos] {
                b'"' => {
                    // .bst string literal: scan to the closing quote
                    self.pos += 1;
                    while self.pos < self.src.len() && self.src[self.pos] != b'"' {
                        self.pos += 1;
                    }
                    if self.pos >= self.src.len() {
                        return Err("unterminated string literal in .bst group".into());
                    }
                }
                b'%' => {
                    while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                    continue;
                }
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        let s = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
                        self.pos += 1;
                        return Ok(s);
                    }
                }
                _ => {}
            }
            self.pos += 1;
        }
        Err("unterminated '{' group in .bst file".into())
    }

    /// Read a bare identifier (command name, function name).
    /// bibtex.web id_class: letters, digits, and `. $ _ + - * : / ^`.
    fn ident(&mut self) -> Option<String> {
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.src.len() {
            if is_bst_id_char(self.src[self.pos]) {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            None
        } else {
            Some(String::from_utf8_lossy(&self.src[start..self.pos]).into_owned())
        }
    }

    /// Tokenize a function body (contents of a `{...}` group).
    /// Nested `{...}` groups are inline function literals (if$/while$
    /// branches); double-quoted strings may contain unbalanced braces.
    fn body(src: &str) -> Result<Vec<Tok>, String> {
        let mut toks = Vec::new();
        let mut i = 0usize;
        Self::scan_body(src, &mut i, &mut toks, false)?;
        Ok(toks)
    }

    fn scan_body(
        src: &str,
        i: &mut usize,
        toks: &mut Vec<Tok>,
        nested: bool,
    ) -> Result<(), String> {
        let b = src.as_bytes();
        while *i < b.len() {
            let c = b[*i];
            if c == b'%' {
                while *i < b.len() && b[*i] != b'\n' {
                    *i += 1;
                }
            } else if c.is_ascii_whitespace() {
                *i += 1;
            } else if c == b'"' {
                // string literal: ends at the next '"'
                let start = *i + 1;
                let mut j = start;
                while j < b.len() && b[j] != b'"' {
                    j += 1;
                }
                if j >= b.len() {
                    return Err("unterminated string literal in function body".into());
                }
                toks.push(Tok::Str(src[start..j].to_string()));
                *i = j + 1;
            } else if c == b'#' {
                let start = *i + 1;
                let mut j = start;
                if j < b.len() && (b[j] == b'-' || b[j] == b'+') {
                    j += 1;
                }
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                if j == start {
                    return Err("'#' not followed by a number".into());
                }
                toks.push(Tok::Int(
                    src[start..j].parse::<i64>().map_err(|e| e.to_string())?,
                ));
                *i = j;
            } else if c == b'\'' {
                let start = *i + 1;
                let mut j = start;
                while j < b.len() {
                    if is_bst_id_char(b[j]) {
                        j += 1;
                    } else {
                        break;
                    }
                }
                if j == start {
                    return Err("''' not followed by a name".into());
                }
                toks.push(Tok::Quoted(src[start..j].to_string()));
                *i = j;
            } else if c == b':' && *i + 1 < b.len() && b[*i + 1] == b'=' {
                toks.push(Tok::Ident(":=".to_string()));
                *i += 2;
            } else if c == b'{' {
                *i += 1;
                let mut inner = Vec::new();
                Self::scan_body(src, i, &mut inner, true)?;
                toks.push(Tok::FnLit(inner));
            } else if c == b'}' {
                if !nested {
                    return Err(format!("unbalanced '}}' in function body at offset {}", i));
                }
                *i += 1;
                return Ok(());
            } else {
                // identifier / symbol token
                let start = *i;
                while *i < b.len() {
                    let ch = b[*i];
                    if ch.is_ascii_whitespace()
                        || ch == b'%'
                        || ch == b'"'
                        || ch == b'#'
                        || ch == b'\''
                        || ch == b'{'
                        || ch == b'}'
                        || (ch == b':' && *i + 1 < b.len() && b[*i + 1] == b'=')
                    {
                        break;
                    }
                    *i += 1;
                }
                toks.push(Tok::Ident(src[start..*i].to_string()));
            }
        }
        if nested {
            return Err("unterminated '{' function literal in body".into());
        }
        Ok(())
    }
}

fn split_names(group: &str) -> Vec<String> {
    group
        .lines()
        .map(|line| match line.find('%') {
            Some(i) => &line[..i],
            None => line,
        })
        .flat_map(|line| line.split_whitespace())
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}
/// Parse a whole .bst file into its command list.
pub fn parse_bst(src: &str) -> Result<BstProgram, String> {
    let mut lx = Lexer::new(src);
    let mut cmds = Vec::new();
    loop {
        lx.skip_ws();
        if lx.pos >= lx.src.len() {
            break;
        }
        let name = match lx.ident() {
            Some(n) => n,
            None => {
                return Err(format!(
                    "unexpected character {:?} at byte {}",
                    lx.src[lx.pos] as char, lx.pos
                ))
            }
        };
        match name.to_ascii_lowercase().as_str() {
            "entry" => {
                let fields = split_names(&lx.group()?);
                let ints = split_names(&lx.group()?);
                let strs = split_names(&lx.group()?);
                cmds.push(BstCmd::Entry { fields, ints, strs });
            }
            "function" => {
                let fname = lx.group()?.trim().to_string();
                let body = Lexer::body(&lx.group()?)?;
                cmds.push(BstCmd::Function(fname, body));
            }
            "integers" => cmds.push(BstCmd::Integers(split_names(&lx.group()?))),
            "strings" => cmds.push(BstCmd::Strings(split_names(&lx.group()?))),
            "macro" => {
                let mname = lx.group()?;
                let mut val = lx.group()?;
                let val_trim = val.trim().to_string();
                if val_trim.starts_with('"') && val_trim.ends_with('"') && val_trim.len() >= 2 {
                    val = val_trim[1..val_trim.len() - 1].to_string();
                }
                cmds.push(BstCmd::Macro(mname, val));
            }
            "read" => cmds.push(BstCmd::Read),
            "sort" => cmds.push(BstCmd::Sort),
            "execute" => cmds.push(BstCmd::Execute(lx.group()?.trim().to_string())),
            "iterate" => cmds.push(BstCmd::Iterate(lx.group()?.trim().to_string())),
            "reverse" => cmds.push(BstCmd::Reverse(lx.group()?.trim().to_string())),
            "comment" => {
                let _ = lx.group();
            }
            other => return Err(format!("unknown .bst command \"{other}\"")),
        }
    }
    Ok(BstProgram { cmds })
}
