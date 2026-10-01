//! Byte-string built-ins without side effects: `add.period$`,
//! `purify$`, `substring$`, `text.length$`, `text.prefix$`, the special
//! character table, `width$` metrics and `change.case$`'s special
//! character conversion (all ported from bibtex.web).

use crate::input::{is_alpha, is_white, lex_class, ALPHA, NUMERIC, SEP, WHITE};

/// The 13 control sequences BibTeX knows as special characters.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cs {
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

impl Cs {
    /// A lower-case special character makes a token a `von` token.
    pub fn is_lower_case(self) -> bool {
        !matches!(self, Cs::OeUpper | Cs::AeUpper | Cs::AaUpper | Cs::OUpper | Cs::LUpper)
    }
}

pub fn control_seq(name: &[u8]) -> Option<Cs> {
    Some(match name {
        b"i" => Cs::I,
        b"j" => Cs::J,
        b"oe" => Cs::Oe,
        b"OE" => Cs::OeUpper,
        b"ae" => Cs::Ae,
        b"AE" => Cs::AeUpper,
        b"aa" => Cs::Aa,
        b"AA" => Cs::AaUpper,
        b"o" => Cs::O,
        b"O" => Cs::OUpper,
        b"l" => Cs::L,
        b"L" => Cs::LUpper,
        b"ss" => Cs::Ss,
        _ => return None,
    })
}

/// `char_width`: cmr10 widths (hundredths of a point) for printable ASCII.
const CHAR_WIDTH: [u16; 95] = [
    278, 278, 500, 833, 500, 833, 778, 278, 389, 389, 500, 778, 278, 333, 278, 500, // ' '..'/'
    500, 500, 500, 500, 500, 500, 500, 500, 500, 500, 278, 278, 278, 778, 472, 472, // '0'..'?'
    778, 750, 708, 722, 764, 681, 653, 785, 750, 361, 514, 778, 625, 917, 750, 778, // '@'..'O'
    681, 778, 736, 556, 722, 750, 750, 1028, 750, 750, 611, 278, 500, 278, 500, 278, // 'P'..'_'
    278, 500, 556, 444, 556, 444, 306, 500, 556, 278, 306, 528, 278, 833, 556, 500, // '`'..'o'
    556, 528, 392, 394, 389, 556, 528, 722, 528, 528, 444, 500, 1000, 500, 500, // 'p'..'~'
];

pub fn char_width(c: u8) -> i32 {
    match c {
        b' '..=b'~' => CHAR_WIDTH[(c - b' ') as usize] as i32,
        _ => 0,
    }
}

/// Width of a special character whose control sequence starts with `first`.
pub fn special_width(cs: Cs, first: u8) -> i32 {
    match cs {
        Cs::Ss => 500,
        Cs::Ae => 722,
        Cs::Oe => 778,
        Cs::AeUpper => 903,
        Cs::OeUpper => 1014,
        _ => char_width(first),
    }
}

/// `add.period$`: `None` when the string stays as it is.
pub fn add_period(s: &[u8]) -> Option<Vec<u8>> {
    if s.is_empty() {
        return None;
    }
    // the last non-`}` byte, or the first byte if all are `}`
    let last = s.iter().rev().find(|&&c| c != b'}').copied().unwrap_or(s[0]);
    if matches!(last, b'.' | b'?' | b'!') {
        return None;
    }
    let mut t = Vec::with_capacity(s.len() + 1);
    t.extend_from_slice(s);
    t.push(b'.');
    Some(t)
}

/// `substring$` with 1-based `start` (negative counts from the end):
/// `None` when the whole string is pushed back unchanged.
pub fn substring(s: &[u8], start: i32, len: i32) -> Option<&[u8]> {
    let n = s.len() as i64;
    let (start, mut len) = (start as i64, len as i64);
    if len >= n && (start == 1 || start == -1) {
        return None;
    }
    if len <= 0 || start == 0 || start > n || start < -n {
        return Some(&[]);
    }
    if start > 0 {
        if len > n - (start - 1) {
            len = n - (start - 1);
        }
        let from = (start - 1) as usize;
        Some(&s[from..from + len as usize])
    } else {
        let start = -start;
        if len > n - (start - 1) {
            len = n - (start - 1);
        }
        let end = (n - (start - 1)) as usize;
        Some(&s[end - len as usize..end])
    }
}

/// Skip a special character: `i` is just past its `{\`, `level` is 1.
fn skip_special(s: &[u8], mut i: usize, level: &mut u32) -> usize {
    while i < s.len() && *level > 0 {
        match s[i] {
            b'}' => *level -= 1,
            b'{' => *level += 1,
            _ => {}
        }
        i += 1;
    }
    i
}

/// `text.length$`: braces don't count, a special character counts once.
pub fn text_length(s: &[u8]) -> usize {
    let mut n = 0;
    let mut level = 0u32;
    let mut i = 0;
    while i < s.len() {
        i += 1;
        match s[i - 1] {
            b'{' => {
                level += 1;
                if level == 1 && i < s.len() && s[i] == b'\\' {
                    i = skip_special(s, i + 1, &mut level);
                    n += 1;
                }
            }
            b'}' => level = level.saturating_sub(1),
            _ => n += 1,
        }
    }
    n
}

/// `text.prefix$` for `n > 0`: the first `n` text characters, with any
/// open braces closed.
pub fn text_prefix(s: &[u8], n: usize) -> Vec<u8> {
    let mut num = 0;
    let mut level = 0u32;
    let mut i = 0;
    while i < s.len() && num < n {
        i += 1;
        match s[i - 1] {
            b'{' => {
                level += 1;
                if level == 1 && i < s.len() && s[i] == b'\\' {
                    i = skip_special(s, i + 1, &mut level);
                    num += 1;
                }
            }
            b'}' => level = level.saturating_sub(1),
            _ => num += 1,
        }
    }
    let mut out = Vec::with_capacity(i + level as usize);
    out.extend_from_slice(&s[..i]);
    out.resize(i + level as usize, b'}');
    out
}

/// `purify$`.
pub fn purify(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut level = 0u32;
    let n = s.len();
    let mut i = 0;
    while i < n {
        let c = s[i];
        match lex_class(c) {
            WHITE | SEP => out.push(b' '),
            ALPHA | NUMERIC => out.push(c),
            _ if c == b'{' => {
                level += 1;
                if level == 1 && i + 1 < n && s[i + 1] == b'\\' {
                    // <Purify a special character>
                    i += 1;
                    while i < n && level > 0 {
                        i += 1;
                        let ys = i;
                        while i < n && is_alpha(s[i]) {
                            i += 1;
                        }
                        if let Some(cs) = control_seq(&s[ys..i]) {
                            out.push(s[ys]);
                            if matches!(cs, Cs::Oe | Cs::OeUpper | Cs::Ae | Cs::AeUpper | Cs::Ss) {
                                out.push(s[ys + 1]);
                            }
                        }
                        while i < n && level > 0 && s[i] != b'\\' {
                            match s[i] {
                                b'}' => level -= 1,
                                b'{' => level += 1,
                                c if matches!(lex_class(c), ALPHA | NUMERIC) => out.push(c),
                                _ => {}
                            }
                            i += 1;
                        }
                    }
                    i -= 1;
                }
            }
            _ if c == b'}' => level = level.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Case {
    Title,
    Lower,
    Upper,
    Bad,
}

fn convert(b: &mut [u8], conv: Case) {
    match conv {
        Case::Title | Case::Lower => b.make_ascii_lowercase(),
        Case::Upper => b.make_ascii_uppercase(),
        Case::Bad => {}
    }
}

/// `<Convert a special character>` for `change.case$`: `i` is at the
/// opening brace (`level` already counts it). Returns the index of the
/// last byte consumed.
pub fn convert_special(buf: &mut Vec<u8>, mut i: usize, conv: Case, level: &mut i32) -> usize {
    i += 1;
    while i < buf.len() && *level > 0 {
        i += 1;
        let xs = i;
        while i < buf.len() && is_alpha(buf[i]) {
            i += 1;
        }
        if let Some(cs) = control_seq(&buf[xs..i]) {
            match conv {
                Case::Title | Case::Lower => {
                    if !cs.is_lower_case() {
                        buf[xs..i].make_ascii_lowercase();
                    }
                }
                Case::Upper => match cs {
                    Cs::L | Cs::O | Cs::Oe | Cs::Ae | Cs::Aa => buf[xs..i].make_ascii_uppercase(),
                    Cs::I | Cs::J | Cs::Ss => {
                        // <Convert, then remove the control sequence>:
                        // drop the backslash and the white space after it
                        buf[xs..i].make_ascii_uppercase();
                        buf.remove(xs - 1);
                        i -= 1;
                        let ws = i;
                        while i < buf.len() && is_white(buf[i]) {
                            i += 1;
                        }
                        buf.drain(ws..i);
                        i = ws;
                    }
                    _ => {}
                },
                Case::Bad => {}
            }
        }
        let xs = i;
        while i < buf.len() && *level > 0 && buf[i] != b'\\' {
            match buf[i] {
                b'}' => *level -= 1,
                b'{' => *level += 1,
                _ => {}
            }
            i += 1;
        }
        convert(&mut buf[xs..i], conv);
    }
    i - 1
}
