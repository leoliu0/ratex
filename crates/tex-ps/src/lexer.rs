//! EPS container/DSC parsing and the PostScript token scanner.

use crate::interp::{err, Interp, Res};
use crate::types::{EpsBoundingBox, FileId, Value};

/// Unwraps a binary DOS EPS wrapper if present.
pub fn extract_ps_payload(input: &[u8]) -> &[u8] {
    if input.len() >= 30 && input.starts_with(&[0xC5, 0xD0, 0xD3, 0xC6]) {
        let ps_offset = u32::from_le_bytes([input[4], input[5], input[6], input[7]]) as usize;
        let ps_length = u32::from_le_bytes([input[8], input[9], input[10], input[11]]) as usize;
        if let Some(end) = ps_offset.checked_add(ps_length) {
            if end <= input.len() {
                return &input[ps_offset..end];
            }
        }
    }
    input
}

/// Parses `%%BoundingBox:` and `%%HiResBoundingBox:` from the EPS header
/// comments, preferring the high-resolution box. `(atend)` boxes are looked
/// up in the trailer.
pub fn extract_bounding_box(input: &[u8]) -> Option<EpsBoundingBox> {
    let mut regular = None;
    let mut hires = None;
    for line in input.split(|&b| b == b'\n' || b == b'\r') {
        let line = trim_ascii(line);
        if let Some(rest) = line.strip_prefix(b"%%HiResBoundingBox:") {
            if hires.is_none() {
                hires = parse_bbox_numbers(rest);
            }
        } else if let Some(rest) = line.strip_prefix(b"%%BoundingBox:") {
            if regular.is_none() {
                regular = parse_bbox_numbers(rest);
            }
        }
        if hires.is_some() && regular.is_some() {
            break;
        }
    }
    hires.or(regular)
}

fn trim_ascii(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(s.len());
    let end = s.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(start, |e| e + 1);
    &s[start..end]
}

fn parse_bbox_numbers(s: &[u8]) -> Option<EpsBoundingBox> {
    let text = std::str::from_utf8(s).ok()?;
    let mut parts = text.split_whitespace().map(|p| p.parse::<f64>().ok().filter(|v| v.is_finite()));
    let (llx, lly, urx, ury) = (parts.next()??, parts.next()??, parts.next()??, parts.next()??);
    (urx > llx && ury > lly).then_some(EpsBoundingBox { llx, lly, urx, ury })
}

/// PostScript whitespace (NUL, TAB, LF, FF, CR, SP) plus the Ctrl-D job
/// separator that printer drivers leave in EPS files.
#[inline]
pub(crate) fn is_space(b: u8) -> bool {
    matches!(b, 0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ' | 0x04)
}

#[inline]
fn is_delimiter(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

#[inline]
pub(crate) fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parses a PostScript number token: integers (32-bit; larger ones become
/// reals), reals with optional exponent, and `base#digits` radix numbers.
pub(crate) fn parse_number(tok: &[u8]) -> Option<Value> {
    if let Some(hash) = tok.iter().position(|&b| b == b'#') {
        let base: u32 = std::str::from_utf8(&tok[..hash]).ok()?.parse().ok()?;
        if !(2..=36).contains(&base) || hash + 1 == tok.len() || tok[..hash].iter().any(|b| !b.is_ascii_digit()) {
            return None;
        }
        let mut v: u64 = 0;
        for &b in &tok[hash + 1..] {
            let d = (b as char).to_digit(base)?;
            v = v * u64::from(base) + u64::from(d);
            if v > u64::from(u32::MAX) {
                return None;
            }
        }
        return Some(Value::Int(i64::from(v as u32 as i32)));
    }
    let body = match tok.first() {
        Some(b'+' | b'-') => &tok[1..],
        _ => tok,
    };
    if body.is_empty() {
        return None;
    }
    if body.iter().all(u8::is_ascii_digit) {
        let text = std::str::from_utf8(tok).ok()?;
        if body.len() <= 10 {
            let v: i64 = text.parse().ok()?;
            return Some(crate::interp::int_value(v));
        }
        return text.parse::<f64>().ok().map(Value::Real);
    }
    // [digits][.digits][(e|E)[sign]digits] with at least one mantissa digit.
    let mut k = 0;
    let int_digits = body.iter().take_while(|b| b.is_ascii_digit()).count();
    k += int_digits;
    let mut frac_digits = 0;
    if body.get(k) == Some(&b'.') {
        k += 1;
        frac_digits = body[k..].iter().take_while(|b| b.is_ascii_digit()).count();
        k += frac_digits;
    }
    if int_digits + frac_digits == 0 {
        return None;
    }
    if matches!(body.get(k), Some(b'e' | b'E')) {
        k += 1;
        if matches!(body.get(k), Some(b'+' | b'-')) {
            k += 1;
        }
        let exp_digits = body[k..].iter().take_while(|b| b.is_ascii_digit()).count();
        if exp_digits == 0 {
            return None;
        }
        k += exp_digits;
    }
    if k != body.len() {
        return None;
    }
    let v: f64 = std::str::from_utf8(tok).ok()?.parse().ok()?;
    v.is_finite().then_some(Value::Real(v))
}

enum Tok {
    Eof,
    Obj(Value),
    Open,
    Close,
}

impl Interp {
    /// Scans one object from a file; procedures are scanned whole.
    pub(crate) fn scan_token(&mut self, f: FileId) -> Res<Option<Value>> {
        let mut procs: Vec<Vec<Value>> = Vec::new();
        loop {
            match self.scan_one(f)? {
                Tok::Eof => {
                    if procs.is_empty() {
                        return Ok(None);
                    }
                    return err("syntaxerror", "unterminated procedure");
                }
                Tok::Open => procs.push(Vec::new()),
                Tok::Close => match procs.pop() {
                    None => return err("syntaxerror", "unmatched '}'"),
                    Some(items) => {
                        let proc = Value::Proc(self.new_array(items)?);
                        match procs.last_mut() {
                            Some(parent) => parent.push(proc),
                            None => return Ok(Some(proc)),
                        }
                    }
                },
                Tok::Obj(v) => match procs.last_mut() {
                    Some(parent) => {
                        self.alloc(std::mem::size_of::<Value>())?;
                        parent.push(v);
                    }
                    None => return Ok(Some(v)),
                },
            }
        }
    }

    fn scan_one(&mut self, f: FileId) -> Res<Tok> {
        let b = loop {
            match self.getc(f)? {
                None => return Ok(Tok::Eof),
                Some(b) if is_space(b) => {}
                Some(b'%') => loop {
                    match self.getc(f)? {
                        None => return Ok(Tok::Eof),
                        Some(b'\n' | b'\r' | 0x0C) => break,
                        Some(_) => {}
                    }
                },
                Some(b) => break b,
            }
        };
        match b {
            b'{' => Ok(Tok::Open),
            b'}' => Ok(Tok::Close),
            b'[' => Ok(Tok::Obj(Value::ExecName(self.n.lbracket))),
            b']' => Ok(Tok::Obj(Value::ExecName(self.n.rbracket))),
            b'(' => self.scan_string(f).map(Tok::Obj),
            b')' => err("syntaxerror", "unmatched ')'"),
            b'<' => match self.getc(f)? {
                Some(b'<') => Ok(Tok::Obj(Value::ExecName(self.n.ldict))),
                Some(b'~') => self.scan_ascii85(f).map(Tok::Obj),
                Some(c) => {
                    self.ungetc(f, c);
                    self.scan_hex(f).map(Tok::Obj)
                }
                None => err("syntaxerror", "unterminated hex string"),
            },
            b'>' => match self.getc(f)? {
                Some(b'>') => Ok(Tok::Obj(Value::ExecName(self.n.rdict))),
                _ => err("syntaxerror", "unexpected '>'"),
            },
            b'/' => {
                let immediate = match self.getc(f)? {
                    Some(b'/') => true,
                    Some(c) => {
                        self.ungetc(f, c);
                        false
                    }
                    None => false,
                };
                let word = self.scan_word(f, None)?;
                let id = self.names.intern(&word);
                if immediate {
                    match self.lookup(id) {
                        Some(v) => Ok(Tok::Obj(v)),
                        None => err("undefined", self.name_string(id)),
                    }
                } else {
                    Ok(Tok::Obj(Value::Name(id)))
                }
            }
            first => {
                let word = self.scan_word(f, Some(first))?;
                Ok(Tok::Obj(match parse_number(&word) {
                    Some(v) => v,
                    None => Value::ExecName(self.names.intern(&word)),
                }))
            }
        }
    }

    /// Reads a regular token; consumes one terminating whitespace character
    /// (CR LF counts as one), leaves delimiters for the next scan.
    fn scan_word(&mut self, f: FileId, first: Option<u8>) -> Res<Vec<u8>> {
        let mut word = Vec::with_capacity(16);
        word.extend(first);
        loop {
            match self.getc(f)? {
                None => break,
                Some(b'\r') => {
                    match self.getc(f)? {
                        Some(b'\n') | None => {}
                        Some(c) => self.ungetc(f, c),
                    }
                    break;
                }
                Some(b) if is_space(b) => break,
                Some(b) if is_delimiter(b) => {
                    self.ungetc(f, b);
                    break;
                }
                Some(b) => {
                    if word.len() >= 65_535 {
                        return err("limitcheck", "name or number too long");
                    }
                    word.push(b);
                }
            }
        }
        Ok(word)
    }

    fn scan_string(&mut self, f: FileId) -> Res<Value> {
        let mut s = Vec::new();
        let mut depth = 1u32;
        loop {
            let Some(b) = self.getc(f)? else {
                return err("syntaxerror", "unterminated string");
            };
            match b {
                b'\\' => {
                    let Some(e) = self.getc(f)? else {
                        return err("syntaxerror", "unterminated string");
                    };
                    match e {
                        b'n' => s.push(b'\n'),
                        b'r' => s.push(b'\r'),
                        b't' => s.push(b'\t'),
                        b'b' => s.push(0x08),
                        b'f' => s.push(0x0C),
                        b'\n' => {}
                        b'\r' => match self.getc(f)? {
                            Some(b'\n') | None => {}
                            Some(c) => self.ungetc(f, c),
                        },
                        b'0'..=b'7' => {
                            let mut v = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.getc(f)? {
                                    Some(d @ b'0'..=b'7') => v = v * 8 + u32::from(d - b'0'),
                                    Some(c) => {
                                        self.ungetc(f, c);
                                        break;
                                    }
                                    None => break,
                                }
                            }
                            s.push(v as u8);
                        }
                        other => s.push(other),
                    }
                }
                b'(' => {
                    depth += 1;
                    s.push(b);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    s.push(b);
                }
                b'\r' => {
                    match self.getc(f)? {
                        Some(b'\n') | None => {}
                        Some(c) => self.ungetc(f, c),
                    }
                    s.push(b'\n');
                }
                _ => s.push(b),
            }
            if s.len() > crate::interp::MAX_OBJECT_LEN {
                return err("limitcheck", "string too long");
            }
        }
        Ok(Value::String(self.new_string(s)?))
    }

    fn scan_hex(&mut self, f: FileId) -> Res<Value> {
        let mut s = Vec::new();
        let mut high: Option<u8> = None;
        loop {
            let Some(b) = self.getc(f)? else {
                return err("syntaxerror", "unterminated hex string");
            };
            if b == b'>' {
                break;
            }
            if is_space(b) {
                continue;
            }
            let Some(d) = hex_digit(b) else {
                return err("syntaxerror", "invalid character in hex string");
            };
            match high.take() {
                Some(h) => s.push(h << 4 | d),
                None => high = Some(d),
            }
            if s.len() > crate::interp::MAX_OBJECT_LEN {
                return err("limitcheck", "string too long");
            }
        }
        if let Some(h) = high {
            s.push(h << 4);
        }
        Ok(Value::String(self.new_string(s)?))
    }

    fn scan_ascii85(&mut self, f: FileId) -> Res<Value> {
        let mut decoder = crate::files::Ascii85::default();
        let mut s = Vec::new();
        loop {
            let Some(b) = self.getc(f)? else {
                return err("syntaxerror", "unterminated ASCII85 string");
            };
            if decoder.push(b, &mut s)? {
                break;
            }
            if s.len() > crate::interp::MAX_OBJECT_LEN {
                return err("limitcheck", "string too long");
            }
        }
        Ok(Value::String(self.new_string(s)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_follow_postscript_syntax() {
        let real = |s: &[u8]| match parse_number(s) {
            Some(Value::Real(r)) => Some(r),
            _ => None,
        };
        let int = |s: &[u8]| match parse_number(s) {
            Some(Value::Int(i)) => Some(i),
            _ => None,
        };
        assert_eq!(int(b"-17"), Some(-17));
        assert_eq!(int(b"+5"), Some(5));
        assert_eq!(int(b"16#FFFE"), Some(0xFFFE));
        assert_eq!(int(b"16#FFFFFFFF"), Some(-1));
        assert_eq!(int(b"8#777"), Some(511));
        assert_eq!(real(b".5"), Some(0.5));
        assert_eq!(real(b"5."), Some(5.0));
        assert_eq!(real(b"-1.5e3"), Some(-1500.0));
        assert_eq!(real(b"1E2"), Some(100.0));
        // 32-bit overflow turns into a real.
        assert_eq!(real(b"4294967296"), Some(4294967296.0));
        // Rust float syntax that PostScript does not accept stays a name.
        for name in [&b"inf"[..], b"nan", b"infinity", b"1e", b"e5", b".", b"+", b"1.2.3", b"36#", b"1#0"] {
            assert!(parse_number(name).is_none(), "{}", String::from_utf8_lossy(name));
        }
    }

    #[test]
    fn dos_header_with_overflowing_offsets_is_ignored() {
        let mut data = vec![0xC5, 0xD0, 0xD3, 0xC6];
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        data.resize(40, 0);
        assert_eq!(extract_ps_payload(&data).len(), 40);
    }
}
