//! .bst style-file parsing: tokenize commands and function bodies.

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Str(String),
    Int(i32),
    FnRef(String), // 'name — a quoted function/variable reference
    /// A braced group in a body: an anonymous procedure literal.
    Proc(std::rc::Rc<Vec<Tok>>),
    Ident(String), // bare identifier, lowercased
}

#[derive(Debug)]
pub enum BstCommand {
    Entry {
        fields: Vec<String>,
        ints: Vec<String>,
        strs: Vec<String>,
    },
    Integers(Vec<String>),
    Strings(Vec<String>),
    Function(String, Vec<Tok>),
    Macro(String, String),
    Read,
    Execute(String),
    Iterate(String),
    Reverse(String),
    Sort,
}

pub struct BstParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> BstParser<'a> {
    pub fn new(text: &'a str) -> Self {
        BstParser {
            b: text.as_bytes(),
            i: 0,
        }
    }

    fn skip_ws(&mut self) {
        loop {
            while self.i < self.b.len() && (self.b[self.i] as char).is_ascii_whitespace() {
                self.i += 1;
            }
            if self.i < self.b.len() && self.b[self.i] == b'%' {
                while self.i < self.b.len() && self.b[self.i] != b'\n' {
                    self.i += 1;
                }
                continue;
            }
            break;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    /// Read the next non-ws token word (letters/digits/other non-delimiters).
    fn scan_word(&mut self) -> String {
        let start = self.i;
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c == b'{' || c == b'}' || c == b'%' || c == b'"' || (c as char).is_ascii_whitespace()
            {
                break;
            }
            self.i += 1;
        }
        String::from_utf8_lossy(&self.b[start..self.i]).into_owned()
    }

    /// Expect and consume the next `{`, then scan a brace group's raw text.
    /// Double-quoted string literals inside the group protect braces (bst
    /// strings have no escapes; they run to the next `"`).
    fn scan_group(&mut self) -> Option<String> {
        self.skip_ws();
        if self.peek() != Some(b'{') {
            return None;
        }
        self.i += 1;
        let mut depth = 1usize;
        let start = self.i;
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c == b'"' {
                // string literal: skip to the closing quote, braces inside
                // do not count
                self.i += 1;
                while self.i < self.b.len() && self.b[self.i] != b'"' {
                    self.i += 1;
                }
                self.i += 1;
                continue;
            }
            if c == b'%' {
                while self.i < self.b.len() && self.b[self.i] != b'\n' {
                    self.i += 1;
                }
                continue;
            }
            if c == b'{' {
                depth += 1;
            } else if c == b'}' {
                depth -= 1;
                if depth == 0 {
                    let s = String::from_utf8_lossy(&self.b[start..self.i]).into_owned();
                    self.i += 1;
                    return Some(s);
                }
            }
            self.i += 1;
        }
        None
    }

    /// Scan a whitespace-separated list of names inside a group.
    fn scan_name_group(&mut self) -> Option<Vec<String>> {
        let g = self.scan_group()?;
        Some(
            g.split_ascii_whitespace()
                .map(|w| w.to_ascii_lowercase())
                .collect(),
        )
    }

    /// Tokenize a function body group.
    fn scan_body(&mut self) -> Option<Vec<Tok>> {
        let g = self.scan_group()?;
        Some(tokenize_body(&g))
    }

    pub fn parse(mut self) -> Vec<BstCommand> {
        let mut cmds = Vec::new();
        loop {
            self.skip_ws();
            let Some(c) = self.peek() else { break };
            if c != b'%' && c != b'{' && c != b'}' && c != b'"' {
                let word = self.scan_word();
                let lw = word.to_ascii_lowercase();
                match lw.as_str() {
                    "entry" => {
                        let fields = self.scan_name_group().unwrap_or_default();
                        let ints = self.scan_name_group().unwrap_or_default();
                        let strs = self.scan_name_group().unwrap_or_default();
                        cmds.push(BstCommand::Entry { fields, ints, strs });
                    }
                    "integers" => {
                        let names = self.scan_name_group().unwrap_or_default();
                        cmds.push(BstCommand::Integers(names));
                    }
                    "strings" => {
                        let names = self.scan_name_group().unwrap_or_default();
                        cmds.push(BstCommand::Strings(names));
                    }
                    "function" => {
                        let name_g = self.scan_group().unwrap_or_default();
                        let fname = name_g.trim().to_ascii_lowercase();
                        let body = self.scan_body().unwrap_or_default();
                        cmds.push(BstCommand::Function(fname, body));
                    }
                    "macro" => {
                        let name_g = self.scan_group().unwrap_or_default();
                        let mname = name_g.trim().to_ascii_lowercase();
                        if let Some(body) = self.scan_body() {
                            let val = match body.first() {
                                Some(Tok::Str(s)) => s.clone(),
                                _ => String::new(),
                            };
                            cmds.push(BstCommand::Macro(mname, val));
                        }
                    }
                    "read" => cmds.push(BstCommand::Read),
                    "execute" => {
                        let name_g = self.scan_group().unwrap_or_default();
                        cmds.push(BstCommand::Execute(name_g.trim().to_ascii_lowercase()));
                    }
                    "iterate" => {
                        let name_g = self.scan_group().unwrap_or_default();
                        cmds.push(BstCommand::Iterate(name_g.trim().to_ascii_lowercase()));
                    }
                    "reverse" => {
                        let name_g = self.scan_group().unwrap_or_default();
                        cmds.push(BstCommand::Reverse(name_g.trim().to_ascii_lowercase()));
                    }
                    "sort" => cmds.push(BstCommand::Sort),
                    "comment" => {
                        // skip to end of line
                        while self.i < self.b.len() && self.b[self.i] != b'\n' {
                            self.i += 1;
                        }
                    }
                    _ => {
                        // unknown command word: skip it
                    }
                }
            } else {
                self.i += 1;
            }
        }
        cmds
    }
}
/// Tokenize the raw text of a wizard-function body. Braced groups become
/// nested `Proc` literals (anonymous functions), exactly as bibtex treats
/// them as single stack literals.
pub fn tokenize_body(text: &str) -> Vec<Tok> {
    let b = text.as_bytes();
    let mut i = 0usize;
    tokenize_range(b, &mut i, b.len())
}

fn tokenize_range(b: &[u8], i: &mut usize, end: usize) -> Vec<Tok> {
    let mut toks = Vec::new();
    while *i < end {
        let c = b[*i];
        if (c as char).is_ascii_whitespace() {
            *i += 1;
            continue;
        }
        if c == b'%' {
            while *i < end && b[*i] != b'\n' {
                *i += 1;
            }
            continue;
        }
        if c == b'{' {
            // anonymous procedure: tokenize the nested range
            *i += 1;
            let inner = tokenize_range(b, i, end); // stops at matching `}`
            toks.push(Tok::Proc(std::rc::Rc::new(inner)));
            continue;
        }
        if c == b'}' {
            *i += 1;
            return toks; // end of this nested range
        }
        if c == b'"' {
            *i += 1;
            let start = *i;
            while *i < end && b[*i] != b'"' {
                *i += 1;
            }
            toks.push(Tok::Str(
                String::from_utf8_lossy(&b[start..*i]).into_owned(),
            ));
            *i += 1; // past closing quote
            continue;
        }
        if c == b'\'' {
            *i += 1;
            let start = *i;
            while *i < end {
                let d = b[*i];
                if d == b'{'
                    || d == b'}'
                    || d == b'"'
                    || d == b'%'
                    || (d as char).is_ascii_whitespace()
                {
                    break;
                }
                *i += 1;
            }
            toks.push(Tok::FnRef(
                String::from_utf8_lossy(&b[start..*i]).to_ascii_lowercase(),
            ));
            continue;
        }
        if c == b'#' {
            *i += 1;
            let mut neg = false;
            if *i < end && b[*i] == b'-' {
                neg = true;
                *i += 1;
            }
            let start = *i;
            while *i < end && b[*i].is_ascii_digit() {
                *i += 1;
            }
            let n: i32 = String::from_utf8_lossy(&b[start..*i]).parse().unwrap_or(0);
            toks.push(Tok::Int(if neg { -n } else { n }));
            continue;
        }
        // bare identifier
        let start = *i;
        while *i < end {
            let d = b[*i];
            if d == b'{'
                || d == b'}'
                || d == b'"'
                || d == b'\''
                || d == b'%'
                || (d as char).is_ascii_whitespace()
            {
                break;
            }
            *i += 1;
        }
        let w = String::from_utf8_lossy(&b[start..*i]).into_owned();
        toks.push(Tok::Ident(w.to_ascii_lowercase()));
    }
    toks
}
