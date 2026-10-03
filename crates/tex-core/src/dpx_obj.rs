//! Small PDF object model for the xdvipdfmx special interpreter
//! (`pdfobj.c` / `pdfparse.c` of dvipdfmx): objects parsed from `\special`
//! text, with `@name` references resolved through a callback, and a
//! serializer producing the text the PDF writer stores as an object body.

/// A PDF object. Dictionaries keep insertion order; `Ref` is an indirect
/// reference `n 0 R`; a `Stream` owns its already-encoded data.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Obj {
    Null,
    Bool(bool),
    Num(f64),
    Name(String),
    Str(Vec<u8>),
    Arr(Vec<Obj>),
    Dict(Vec<(String, Obj)>),
    Stream(Vec<(String, Obj)>, Vec<u8>),
    Ref(i32),
}

impl Obj {
    pub(crate) fn dict_get(&self, key: &str) -> Option<&Obj> {
        match self {
            Obj::Dict(d) | Obj::Stream(d, _) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub(crate) fn dict_entries(&self) -> Option<&Vec<(String, Obj)>> {
        match self {
            Obj::Dict(d) | Obj::Stream(d, _) => Some(d),
            _ => None,
        }
    }

    pub(crate) fn dict_entries_mut(&mut self) -> Option<&mut Vec<(String, Obj)>> {
        match self {
            Obj::Dict(d) | Obj::Stream(d, _) => Some(d),
            _ => None,
        }
    }

    pub(crate) fn is_dict(&self) -> bool {
        matches!(self, Obj::Dict(_))
    }

    pub(crate) fn as_num(&self) -> Option<f64> {
        match self {
            Obj::Num(n) => Some(*n),
            _ => None,
        }
    }
}

/// `pdf_add_dict`: set `key`, replacing an existing entry.
pub(crate) fn dict_set(d: &mut Vec<(String, Obj)>, key: &str, value: Obj) {
    match d.iter_mut().find(|(k, _)| k == key) {
        Some(entry) => entry.1 = value,
        None => d.push((key.to_string(), value)),
    }
}

/// `pdf_merge_dict`: every entry of `src` replaces or extends `dst`.
pub(crate) fn dict_merge(dst: &mut Vec<(String, Obj)>, src: &[(String, Obj)]) {
    for (k, v) in src {
        dict_set(dst, k, v.clone());
    }
}

fn is_white(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// Parser over a byte slice. `lookup` resolves `@name` references (without
/// the `@`); a failed lookup makes the whole object fail, like dvipdfmx.
pub(crate) struct Parser<'a, 'f> {
    pub(crate) s: &'a [u8],
    pub(crate) pos: usize,
    lookup: Option<&'f mut dyn FnMut(&str) -> Option<Obj>>,
}

impl<'a, 'f> Parser<'a, 'f> {
    pub(crate) fn new(s: &'a [u8]) -> Self {
        Parser { s, pos: 0, lookup: None }
    }

    pub(crate) fn with_lookup(s: &'a [u8], lookup: &'f mut dyn FnMut(&str) -> Option<Obj>) -> Self {
        Parser { s, pos: 0, lookup: Some(lookup) }
    }

    pub(crate) fn skip_white(&mut self) {
        while self.pos < self.s.len() {
            let b = self.s[self.pos];
            if is_white(b) {
                self.pos += 1;
            } else if b == b'%' {
                while self.pos < self.s.len() && !matches!(self.s[self.pos], b'\n' | b'\r') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    pub(crate) fn at_end(&mut self) -> bool {
        self.skip_white();
        self.pos >= self.s.len()
    }

    pub(crate) fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    /// An identifier up to white space or a delimiter (`parse_ident`).
    pub(crate) fn ident(&mut self) -> Option<String> {
        self.skip_white();
        let start = self.pos;
        while self.pos < self.s.len() && !is_white(self.s[self.pos]) && !is_delim(self.s[self.pos]) {
            self.pos += 1;
        }
        (self.pos > start).then(|| String::from_utf8_lossy(&self.s[start..self.pos]).into_owned())
    }

    /// `parse_opt_ident`: `@name` -> `name`.
    pub(crate) fn opt_ident(&mut self) -> Option<String> {
        self.skip_white();
        if self.peek() == Some(b'@') {
            self.pos += 1;
            let start = self.pos;
            while self.pos < self.s.len()
                && !is_white(self.s[self.pos])
                && !matches!(self.s[self.pos], b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'/' | b'%')
            {
                self.pos += 1;
            }
            return (self.pos > start).then(|| String::from_utf8_lossy(&self.s[start..self.pos]).into_owned());
        }
        None
    }

    /// `parse_float_decimal`: an optionally signed decimal number as f64.
    pub(crate) fn number(&mut self) -> Option<f64> {
        self.skip_white();
        let start = self.pos;
        let mut p = self.pos;
        if matches!(self.s.get(p), Some(b'+' | b'-')) {
            p += 1;
        }
        let mut digits = 0;
        while matches!(self.s.get(p), Some(b'0'..=b'9')) {
            p += 1;
            digits += 1;
        }
        if self.s.get(p) == Some(&b'.') {
            p += 1;
            while matches!(self.s.get(p), Some(b'0'..=b'9')) {
                p += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            return None;
        }
        let text = std::str::from_utf8(&self.s[start..p]).ok()?;
        self.pos = p;
        text.parse().ok()
    }

    pub(crate) fn name(&mut self) -> Option<String> {
        self.skip_white();
        if self.peek() != Some(b'/') {
            return None;
        }
        self.pos += 1;
        let mut out = Vec::new();
        while self.pos < self.s.len() && !is_white(self.s[self.pos]) && !is_delim(self.s[self.pos]) {
            let b = self.s[self.pos];
            if b == b'#' && self.pos + 2 < self.s.len() + 0 {
                let hex = std::str::from_utf8(&self.s[self.pos + 1..(self.pos + 3).min(self.s.len())]).ok();
                if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(v);
                    self.pos += 3;
                    continue;
                }
            }
            out.push(b);
            self.pos += 1;
        }
        Some(String::from_utf8_lossy(&out).into_owned())
    }

    fn literal_string(&mut self) -> Option<Vec<u8>> {
        // at '('
        self.pos += 1;
        let mut depth = 1;
        let mut out = Vec::new();
        while self.pos < self.s.len() {
            let b = self.s[self.pos];
            self.pos += 1;
            match b {
                b'(' => {
                    depth += 1;
                    out.push(b);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(out);
                    }
                    out.push(b);
                }
                b'\\' => {
                    let Some(&e) = self.s.get(self.pos) else { break };
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut v = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.s.get(self.pos) {
                                    Some(&d @ b'0'..=b'7') => {
                                        v = v * 8 + u32::from(d - b'0');
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v as u8);
                        }
                        b'\r' => {
                            if self.s.get(self.pos) == Some(&b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        other => out.push(other),
                    }
                }
                _ => out.push(b),
            }
        }
        None
    }

    fn hex_string(&mut self) -> Option<Vec<u8>> {
        // at '<'
        self.pos += 1;
        let mut out = Vec::new();
        let mut hi: Option<u8> = None;
        while self.pos < self.s.len() {
            let b = self.s[self.pos];
            self.pos += 1;
            if b == b'>' {
                if let Some(h) = hi {
                    out.push(h << 4);
                }
                return Some(out);
            }
            if is_white(b) {
                continue;
            }
            let v = (b as char).to_digit(16)? as u8;
            match hi.take() {
                Some(h) => out.push((h << 4) | v),
                None => hi = Some(v),
            }
        }
        None
    }

    /// `parse_pdf_object_extended`: one object; `None` on a syntax error.
    pub(crate) fn object(&mut self) -> Option<Obj> {
        self.skip_white();
        match self.peek()? {
            b'<' if self.s.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                let mut d: Vec<(String, Obj)> = Vec::new();
                loop {
                    self.skip_white();
                    if self.s.get(self.pos) == Some(&b'>') && self.s.get(self.pos + 1) == Some(&b'>') {
                        self.pos += 2;
                        break;
                    }
                    let key = self.name()?;
                    let value = self.object()?;
                    dict_set(&mut d, &key, value);
                }
                Some(Obj::Dict(d))
            }
            b'<' => self.hex_string().map(Obj::Str),
            b'(' => self.literal_string().map(Obj::Str),
            b'[' => {
                self.pos += 1;
                let mut a = Vec::new();
                loop {
                    self.skip_white();
                    if self.peek()? == b']' {
                        self.pos += 1;
                        break;
                    }
                    a.push(self.object()?);
                }
                Some(Obj::Arr(a))
            }
            b'/' => self.name().map(Obj::Name),
            b'@' => {
                let name = self.opt_ident()?;
                match self.lookup.as_mut() {
                    Some(f) => f(&name),
                    None => None,
                }
            }
            b'+' | b'-' | b'.' | b'0'..=b'9' => {
                let save = self.pos;
                let n = self.number()?;
                // `n g R`
                if n >= 0.0 && n.fract() == 0.0 {
                    let after = self.pos;
                    if let Some(g) = self.number() {
                        if g.fract() == 0.0 && g >= 0.0 {
                            self.skip_white();
                            if self.peek() == Some(b'R')
                                && self
                                    .s
                                    .get(self.pos + 1)
                                    .is_none_or(|&b| is_white(b) || is_delim(b))
                            {
                                self.pos += 1;
                                return Some(Obj::Ref(n as i32));
                            }
                        }
                    }
                    self.pos = after;
                }
                let _ = save;
                Some(Obj::Num(n))
            }
            _ => {
                let start = self.pos;
                let word = self.ident()?;
                match word.as_str() {
                    "true" => Some(Obj::Bool(true)),
                    "false" => Some(Obj::Bool(false)),
                    "null" => Some(Obj::Null),
                    _ => {
                        self.pos = start;
                        None
                    }
                }
            }
        }
    }
}

/// A real number as dvipdfmx prints it: at most eight decimals, no trailing
/// zeros.
pub(crate) fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let mut s = format!("{v:.8}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if s == "-0" {
        s = "0".to_string();
    }
    s
}

fn push_name(out: &mut Vec<u8>, name: &str) {
    out.push(b'/');
    for &b in name.as_bytes() {
        if b <= 32 || b >= 127 || is_delim(b) || b == b'#' {
            out.extend_from_slice(format!("#{b:02x}").as_bytes());
        } else {
            out.push(b);
        }
    }
}

fn push_string(out: &mut Vec<u8>, s: &[u8]) {
    if s.iter().all(|&b| (32..127).contains(&b)) {
        out.push(b'(');
        for &b in s {
            if matches!(b, b'(' | b')' | b'\\') {
                out.push(b'\\');
            }
            out.push(b);
        }
        out.push(b')');
    } else {
        out.push(b'<');
        for &b in s {
            out.extend_from_slice(format!("{b:02x}").as_bytes());
        }
        out.push(b'>');
    }
}

impl Obj {
    /// Serialize as the body of an indirect object (streams included).
    pub(crate) fn write(&self, out: &mut Vec<u8>) {
        match self {
            Obj::Null => out.extend_from_slice(b"null"),
            Obj::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
            Obj::Num(n) => out.extend_from_slice(fmt_num(*n).as_bytes()),
            Obj::Name(n) => push_name(out, n),
            Obj::Str(s) => push_string(out, s),
            Obj::Ref(n) => out.extend_from_slice(format!("{n} 0 R").as_bytes()),
            Obj::Arr(a) => {
                out.push(b'[');
                for (i, o) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(b' ');
                    }
                    o.write(out);
                }
                out.push(b']');
            }
            Obj::Dict(d) => write_dict(out, d),
            Obj::Stream(d, data) => {
                let mut d = d.clone();
                dict_set(&mut d, "Length", Obj::Num(data.len() as f64));
                write_dict(out, &d);
                out.extend_from_slice(b"\nstream\n");
                out.extend_from_slice(data);
                out.extend_from_slice(b"\nendstream");
            }
        }
    }

    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }
}

fn write_dict(out: &mut Vec<u8>, d: &[(String, Obj)]) {
    out.extend_from_slice(b"<<");
    for (k, v) in d {
        out.push(b' ');
        push_name(out, k);
        out.push(b' ');
        v.write(out);
    }
    out.extend_from_slice(b" >>");
}

/// Text-string re-encoding of `modify_strings` for XDV input: strings under
/// the `taint` keys that are not already UTF-16BE with a BOM are taken as
/// UTF-8 and converted when they hold non-ASCII bytes.
pub(crate) fn reencode_text_strings(o: &mut Obj, key: Option<&str>, taint: &[String]) {
    match o {
        Obj::Str(s) => {
            if key.is_some_and(|k| taint.iter().any(|t| t == k))
                && !s.starts_with(&[0xfe, 0xff])
                && s.iter().any(|&b| b > 127)
            {
                if let Ok(text) = std::str::from_utf8(s) {
                    let mut v = vec![0xfe, 0xff];
                    for u in text.encode_utf16() {
                        v.extend_from_slice(&u.to_be_bytes());
                    }
                    *s = v;
                }
            }
        }
        Obj::Arr(a) => {
            for x in a {
                reencode_text_strings(x, key, taint);
            }
        }
        Obj::Dict(d) | Obj::Stream(d, _) => {
            for (k, v) in d.iter_mut() {
                let k = k.clone();
                reencode_text_strings(v, Some(&k), taint);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_writes_dictionaries() {
        let src = b"<</Type/Annot/Rect[1 2.5 -3 4]/A<</S/GoTo/D(a\\(b)>>/R 12 0 R/B true>>";
        let mut p = Parser::new(src);
        let o = p.object().unwrap();
        assert_eq!(o.dict_get("R"), Some(&Obj::Ref(12)));
        let text = String::from_utf8(o.to_bytes()).unwrap();
        assert!(text.contains("/Rect [1 2.5 -3 4]") || text.contains("/Rect[1 2.5 -3 4]") || text.contains("[1 2.5 -3 4]"));
        assert!(text.contains("(a\\(b)"));
    }

    #[test]
    fn names_resolve_through_lookup() {
        let mut f = |n: &str| (n == "x").then_some(Obj::Ref(7));
        let mut p = Parser::with_lookup(b"[@x /Fit]", &mut f);
        assert_eq!(p.object().unwrap(), Obj::Arr(vec![Obj::Ref(7), Obj::Name("Fit".into())]));
    }

    #[test]
    fn binary_strings_are_hex() {
        let o = Obj::Str(vec![0xfe, 0xff, 0, b'A']);
        assert_eq!(o.to_bytes(), b"<feff0041>");
    }
}

#[cfg(test)]
mod docinfo_tests {
    use super::*;

    #[test]
    fn dict_without_separating_blanks() {
        let mut p = Parser::new(b"<</Title()/Subject()/Creator(LaTeX with hyperref)/Author()/Keywords()>>");
        match p.object() {
            Some(Obj::Dict(d)) => assert_eq!(d.len(), 5),
            other => panic!("{other:?}"),
        }
    }
}
