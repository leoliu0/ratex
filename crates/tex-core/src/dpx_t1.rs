//! Type 1 → Type 1C conversion of xdvipdfmx 20260113 (`pst.c`/`pst_obj.c`, `t1_load.c`,
//! `t1_char.c`, `type1.c`): `pdf_font_load_type1` embeds a Type 1 program as a CFF
//! (`/FontFile3 /Subtype /Type1C`) holding only the used glyphs, with the Type 1 charstrings
//! converted to Type 2 charstrings and no subroutines.
//!
//! The functions follow the C code line by line (including its quirks) so that the output is
//! byte-identical to xdvipdfmx's. Where the C code aborts (`ERROR`), the conversion fails and the
//! caller keeps the program as it is.

use crate::dpx_cff::{index_size, pack_index, Dict, STD_STRINGS};

type R<T> = Result<T, String>;

const CFF_STDSTR_MAX: usize = 391;
const CS_STR_LEN_MAX: usize = 65536;
const CS_SUBR_NEST_MAX: i32 = 10;
const CS_STEM_ZONE_MAX: usize = 96;
const CS_ARG_STACK_MAX: usize = 48;
const PS_ARG_STACK_MAX: usize = CS_STEM_ZONE_MAX * 2 + 2;

// ---------------------------------------------------------------------------------------------
// pst.c / pst_obj.c: the PostScript tokenizer
// ---------------------------------------------------------------------------------------------

const PST_NAME_LEN_MAX: usize = 127;
const PST_STRING_LEN_MAX: usize = 32767;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(Vec<u8>),
    Int(i32),
    Real(f64),
    Bool(bool),
    Null,
    Str(Vec<u8>),
    Mark,
    /// `PST_TYPE_UNKNOWN`: operators and everything the scanner does not know.
    Op(Vec<u8>),
}

impl Tok {
    fn is_op(&self, s: &str) -> bool {
        matches!(self, Tok::Op(v) if v == s.as_bytes())
    }
    fn is_number(&self) -> bool {
        matches!(self, Tok::Int(_) | Tok::Real(_))
    }
    /// `pst_getRV`
    fn rv(&self) -> f64 {
        match self {
            Tok::Int(v) => f64::from(*v),
            Tok::Real(v) => *v,
            _ => 0.0,
        }
    }
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | 0x0c | b'\r' | b'\n' | 0)
}

fn is_delim(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'/' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'%')
}

fn xtoi(c: u8) -> i32 {
    match c {
        b'0'..=b'9' => i32::from(c - b'0'),
        b'a'..=b'f' => i32::from(c - b'a') + 10,
        b'A'..=b'F' => i32::from(c - b'A') + 10,
        _ => -1,
    }
}

/// C `strtol`: (value, end); `end == start` when nothing was converted; `overflow` is `errno`.
fn c_strtol(b: &[u8], start: usize, base: u32) -> (i64, usize, bool) {
    let at = |i: usize| b.get(i).copied().unwrap_or(0);
    let mut i = start;
    while matches!(at(i), b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') {
        i += 1;
    }
    let neg = match at(i) {
        b'-' => {
            i += 1;
            true
        }
        b'+' => {
            i += 1;
            false
        }
        _ => false,
    };
    let digits_start = i;
    let mut value: i128 = 0;
    let mut overflow = false;
    while let Some(d) = (at(i) as char).to_digit(36).filter(|&d| d < base) {
        value = value * i128::from(base) + i128::from(d);
        if value > i128::from(i64::MAX) + 1 {
            overflow = true;
            value = i128::from(i64::MAX) + 1;
        }
        i += 1;
    }
    if i == digits_start {
        return (0, start, false);
    }
    let v = if neg { -value } else { value };
    if v > i128::from(i64::MAX) {
        return (i64::MAX, i, true);
    }
    if v < i128::from(i64::MIN) {
        return (i64::MIN, i, true);
    }
    (v as i64, i, overflow)
}

/// C `strtod` for decimal input: (value, end).
fn c_strtod(b: &[u8], start: usize) -> (f64, usize) {
    let at = |i: usize| b.get(i).copied().unwrap_or(0);
    let mut i = start;
    while matches!(at(i), b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') {
        i += 1;
    }
    let num_start = i;
    if matches!(at(i), b'+' | b'-') {
        i += 1;
    }
    let mut digits = 0;
    while at(i).is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if at(i) == b'.' {
        i += 1;
        while at(i).is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return (0.0, start);
    }
    if matches!(at(i), b'e' | b'E') {
        let mut j = i + 1;
        if matches!(at(j), b'+' | b'-') {
            j += 1;
        }
        if at(j).is_ascii_digit() {
            while at(j).is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    let text = String::from_utf8_lossy(&b[num_start..i]);
    (text.parse::<f64>().unwrap_or(0.0), i)
}

struct Lex<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Lex<'a> {
    fn new(b: &'a [u8]) -> Self {
        Lex { b, pos: 0 }
    }

    fn at(&self, i: usize) -> u8 {
        self.b.get(i).copied().unwrap_or(0)
    }

    fn len(&self) -> usize {
        self.b.len()
    }

    /// `PST_TOKEN_END`
    fn token_end(&self, i: usize) -> bool {
        i >= self.len() || is_delim(self.b[i]) || is_space(self.b[i])
    }

    fn skip_white_spaces(&mut self) {
        while self.pos < self.len() && is_space(self.b[self.pos]) {
            self.pos += 1;
        }
    }

    fn skip_line(&mut self) {
        while self.pos < self.len() && self.b[self.pos] != b'\n' && self.b[self.pos] != b'\r' {
            self.pos += 1;
        }
        if self.pos < self.len() && self.b[self.pos] == b'\r' {
            self.pos += 1;
        }
        if self.pos < self.len() && self.b[self.pos] == b'\n' {
            self.pos += 1;
        }
    }

    fn skip_comments(&mut self) {
        while self.pos < self.len() && self.b[self.pos] == b'%' {
            self.skip_line();
            self.skip_white_spaces();
        }
    }

    /// `pst_parse_any`
    fn parse_any(&mut self) -> Tok {
        let mut cur = self.pos;
        while cur < self.len() && !self.token_end(cur) {
            cur += 1;
        }
        let data = self.b[self.pos..cur].to_vec();
        // (a stray delimiter would stall the C scanner: step over it)
        self.pos = if cur == self.pos { cur + 1 } else { cur };
        Tok::Op(data)
    }

    fn parse_boolean(&mut self) -> Option<Tok> {
        let rest = &self.b[self.pos..];
        if rest.starts_with(b"true") && self.token_end(self.pos + 4) {
            self.pos += 4;
            Some(Tok::Bool(true))
        } else if rest.starts_with(b"false") && self.token_end(self.pos + 5) {
            self.pos += 5;
            Some(Tok::Bool(false))
        } else {
            None
        }
    }

    fn parse_null(&mut self) -> Option<Tok> {
        if self.b[self.pos..].starts_with(b"null") && self.token_end(self.pos + 4) {
            self.pos += 4;
            Some(Tok::Null)
        } else {
            None
        }
    }

    /// `pst_parse_number`
    fn parse_number(&mut self) -> Option<Tok> {
        let (lval, cur, errno) = c_strtol(self.b, self.pos, 10);
        let c = self.at(cur);
        if errno || c == b'.' || c == b'e' || c == b'E' {
            let (dval, cur) = c_strtod(self.b, self.pos);
            if dval.is_finite() && self.token_end(cur) && cur > self.pos {
                self.pos = cur;
                return Some(Tok::Real(dval));
            }
        } else if cur != self.pos && self.token_end(cur) {
            self.pos = cur;
            return Some(Tok::Int(lval as i32));
        } else if (2..=36).contains(&lval) && c == b'#' && self.at(cur + 1).is_ascii_alphanumeric() {
            let cur = cur + 1;
            // strtod allows a leading "0x" for hex numbers, but we don't
            if lval != 16 || (self.at(cur + 1) != b'x' && self.at(cur + 1) != b'X') {
                let (v, end, errno) = c_strtol(self.b, cur, lval as u32);
                if !errno && self.token_end(end) {
                    self.pos = end;
                    return Some(Tok::Int(v as i32));
                }
            }
        }
        None
    }

    /// `pst_parse_name`
    fn parse_name(&mut self) -> Tok {
        let mut cur = self.pos + 1;
        let mut name: Vec<u8> = Vec::new();
        while !self.token_end(cur) {
            let mut c = self.b[cur];
            cur += 1;
            if c == b'#' {
                if cur + 2 >= self.len() {
                    break;
                }
                let hi = xtoi(self.at(cur));
                let val = if hi < 0 {
                    hi
                } else {
                    let lo = xtoi(self.at(cur + 1));
                    if lo < 0 {
                        cur += 1;
                        lo
                    } else {
                        cur += 2;
                        hi << 4 | lo
                    }
                };
                if val <= 0 {
                    continue;
                }
                c = val as u8;
            }
            if name.len() < PST_NAME_LEN_MAX {
                name.push(c);
            }
        }
        self.pos = cur;
        Tok::Name(name)
    }

    /// `pst_parse_string`
    fn parse_string(&mut self) -> R<Option<Tok>> {
        if self.pos + 2 >= self.len() {
            return Ok(None);
        }
        match self.b[self.pos] {
            b'(' => Ok(self.parse_literal().map(Tok::Str)),
            b'<' if self.b[self.pos + 1] == b'~' => Err("ASCII85 string not supported yet.".into()),
            b'<' => Ok(self.parse_hex().map(Tok::Str)),
            _ => Ok(None),
        }
    }

    /// `esctouc`: (unescaped, valid, new position)
    fn esctouc(&self, mut cur: usize) -> (u8, bool, usize) {
        let escaped = self.at(cur);
        match escaped {
            b'\\' | b')' | b'(' => (escaped, true, cur + 1),
            b'n' => (b'\n', true, cur + 1),
            b'r' => (b'\r', true, cur + 1),
            b't' => (b'\t', true, cur + 1),
            b'b' => (8, true, cur + 1),
            b'f' => (0x0c, true, cur + 1),
            b'\r' => {
                let skip = if cur + 1 < self.len() && self.at(cur + 1) == b'\n' { 2 } else { 1 };
                (0, false, cur + skip)
            }
            b'\n' => (0, false, cur + 1),
            _ => {
                // ostrtouc
                let start = cur;
                let mut val = 0u32;
                while cur < self.len() && cur < start + 3 && (b'0'..=b'7').contains(&self.b[cur]) {
                    val = val << 3 | u32::from(self.b[cur] - b'0');
                    cur += 1;
                }
                (val as u8, !(val > 255 || cur == start), cur)
            }
        }
    }

    /// `pst_string_parse_literal`
    fn parse_literal(&mut self) -> Option<Vec<u8>> {
        let mut cur = self.pos;
        if cur + 2 > self.len() || self.b[cur] != b'(' {
            return None;
        }
        cur += 1;
        let mut buf: Vec<u8> = Vec::new();
        let mut balance = 1;
        let mut c = 0u8;
        while cur < self.len() && buf.len() < PST_STRING_LEN_MAX && balance > 0 {
            c = self.b[cur];
            cur += 1;
            match c {
                b'\\' => {
                    let (unescaped, valid, next) = self.esctouc(cur);
                    cur = next;
                    if valid {
                        buf.push(unescaped);
                    }
                }
                b'(' => {
                    balance += 1;
                    buf.push(b'(');
                }
                b')' => {
                    balance -= 1;
                    if balance > 0 {
                        buf.push(b')');
                    }
                }
                b'\r' => {
                    if cur < self.len() && self.b[cur] == b'\n' {
                        cur += 1;
                    }
                    buf.push(b'\n');
                }
                _ => buf.push(c),
            }
        }
        if c != b')' {
            return None;
        }
        self.pos = cur;
        Some(buf)
    }

    /// `pst_string_parse_hex`
    fn parse_hex(&mut self) -> Option<Vec<u8>> {
        let mut cur = self.pos;
        if cur + 2 > self.len() || self.b[cur] != b'<' || self.b[cur + 1] == b'<' {
            return None;
        }
        cur += 1;
        let mut buf: Vec<u8> = Vec::new();
        let skip_ws = |cur: &mut usize, s: &Self| {
            while *cur < s.len() && is_space(s.b[*cur]) {
                *cur += 1;
            }
        };
        while cur < self.len() && buf.len() < PST_STRING_LEN_MAX {
            skip_ws(&mut cur, self);
            if self.at(cur) == b'>' {
                break;
            }
            let hi = xtoi(self.at(cur)).max(0);
            cur += 1;
            skip_ws(&mut cur, self);
            if self.at(cur) == b'>' {
                break;
            }
            let lo = if cur < self.len() {
                let v = xtoi(self.at(cur)).max(0);
                cur += 1;
                v
            } else {
                0
            };
            buf.push((hi << 4 | lo) as u8);
        }
        let closing = self.at(cur);
        cur += 1;
        if closing != b'>' {
            return None;
        }
        self.pos = cur;
        Some(buf)
    }

    /// `pst_get_token`
    fn token(&mut self) -> R<Option<Tok>> {
        self.skip_white_spaces();
        self.skip_comments();
        if self.pos >= self.len() {
            return Ok(None);
        }
        let c = self.b[self.pos];
        let obj = match c {
            b'/' => Some(self.parse_name()),
            b'[' | b'{' => {
                self.pos += 1;
                Some(Tok::Mark)
            }
            b'<' => {
                if self.pos + 1 >= self.len() {
                    return Ok(None);
                }
                let c2 = self.b[self.pos + 1];
                if c2 == b'<' {
                    self.pos += 2;
                    Some(Tok::Mark)
                } else if c2.is_ascii_hexdigit() || c2 == b'~' {
                    self.parse_string()?
                } else {
                    None
                }
            }
            b'(' => self.parse_string()?,
            b'>' => {
                if self.pos + 1 >= self.len() || self.b[self.pos + 1] != b'>' {
                    return Err("Unexpected end of ASCII hex string marker.".into());
                }
                self.pos += 2;
                Some(Tok::Op(b">>".to_vec()))
            }
            b']' | b'}' => {
                self.pos += 1;
                Some(Tok::Op(vec![c]))
            }
            _ => {
                if c == b't' || c == b'f' {
                    self.parse_boolean()
                } else if c == b'n' {
                    self.parse_null()
                } else if c == b'+' || c == b'-' || c.is_ascii_digit() || c == b'.' {
                    self.parse_number()
                } else {
                    None
                }
            }
        };
        Ok(Some(match obj {
            Some(t) => t,
            None => self.parse_any(),
        }))
    }
}

// ---------------------------------------------------------------------------------------------
// t1_load.c
// ---------------------------------------------------------------------------------------------

const T1_EEKEY: u16 = 55665;
const T1_CHARKEY: u16 = 4330;

/// `t1_decrypt`: the first `skip` plain bytes are dropped.
fn t1_decrypt(key: u16, src: &[u8], skip: usize) -> Vec<u8> {
    if src.len() < skip {
        return Vec::new();
    }
    let mut key = key;
    let mut out = Vec::with_capacity(src.len() - skip);
    for (i, &c) in src.iter().enumerate() {
        if i >= skip {
            out.push(c ^ (key >> 8) as u8);
        }
        key = key.wrapping_add(u16::from(c)).wrapping_mul(52845).wrapping_add(22719);
    }
    out
}

static STANDARD_ENCODING: [&str; 256] = [
    ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    "space", "exclam", "quotedbl", "numbersign", "dollar", "percent", "ampersand", "quoteright",
    "parenleft", "parenright", "asterisk", "plus", "comma", "hyphen", "period", "slash", "zero",
    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "colon",
    "semicolon", "less", "equal", "greater", "question", "at", "A", "B", "C", "D", "E", "F",
    "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X",
    "Y", "Z", "bracketleft", "backslash", "bracketright", "asciicircum", "underscore",
    "quoteleft", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p",
    "q", "r", "s", "t", "u", "v", "w", "x", "y", "z", "braceleft", "bar", "braceright",
    "asciitilde", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", ".notdef", ".notdef", "exclamdown", "cent", "sterling", "fraction", "yen",
    "florin", "section", "currency", "quotesingle", "quotedblleft", "guillemotleft",
    "guilsinglleft", "guilsinglright", "fi", "fl", ".notdef", "endash", "dagger", "daggerdbl",
    "periodcentered", ".notdef", "paragraph", "bullet", "quotesinglbase", "quotedblbase",
    "quotedblright", "guillemotright", "ellipsis", "perthousand", ".notdef", "questiondown",
    ".notdef", "grave", "acute", "circumflex", "tilde", "macron", "breve", "dotaccent",
    "dieresis", ".notdef", "ring", "cedilla", ".notdef", "hungarumlaut", "ogonek", "caron",
    "emdash", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", "AE", ".notdef", "ordfeminine", ".notdef", ".notdef", ".notdef", ".notdef",
    "Lslash", "Oslash", "OE", "ordmasculine", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", "ae", ".notdef", ".notdef", ".notdef", "dotlessi", ".notdef", ".notdef",
    "lslash", "oslash", "oe", "germandbls", ".notdef", ".notdef", ".notdef", ".notdef",
];

static ISO_LATIN1_ENCODING: [&str; 256] = [
    ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    "space", "exclam", "quotedbl", "numbersign", "dollar", "percent", "ampersand",
    "quotesingle", "parenleft", "parenright", "asterisk", "plus", "comma", "hyphen", "period",
    "slash", "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    "colon", "semicolon", "less", "equal", "greater", "question", "at", "A", "B", "C", "D", "E",
    "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W",
    "X", "Y", "Z", "bracketleft", "backslash", "bracketright", "asciicircum", "underscore",
    "grave", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p",
    "q", "r", "s", "t", "u", "v", "w", "x", "y", "z", "braceleft", "bar", "braceright",
    "asciitilde", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef", ".notdef",
    ".notdef", ".notdef", "dotlessi", "quoteleft", "quoteright", "circumflex", "tilde",
    "macron", "breve", "dotaccent", "dieresis", ".notdef", "ring", "cedilla", ".notdef",
    "hungarumlaut", "ogonek", "caron", "space", "exclamdown", "cent", "sterling", "currency",
    "yen", "brokenbar", "section", "dieresis", "copyright", "ordfeminine", "guillemotleft",
    "logicalnot", "hyphen", "registered", "macron", "degree", "plusminus", "twosuperior",
    "threesuperior", "acute", "mu", "paragraph", "periodcentered", "cedilla", "onesuperior",
    "ordmasculine", "guillemotright", "onequarter", "onehalf", "threequarters", "questiondown",
    "Agrave", "Aacute", "Acircumflex", "Atilde", "Adieresis", "Aring", "AE", "Ccedilla",
    "Egrave", "Eacute", "Ecircumflex", "Edieresis", "Igrave", "Iacute", "Icircumflex",
    "Idieresis", "Eth", "Ntilde", "Ograve", "Oacute", "Ocircumflex", "Otilde", "Odieresis",
    "multiply", "Oslash", "Ugrave", "Uacute", "Ucircumflex", "Udieresis", "Yacute", "Thorn",
    "germandbls", "agrave", "aacute", "acircumflex", "atilde", "adieresis", "aring", "ae",
    "ccedilla", "egrave", "eacute", "ecircumflex", "edieresis", "igrave", "iacute",
    "icircumflex", "idieresis", "eth", "ntilde", "ograve", "oacute", "ocircumflex", "otilde",
    "odieresis", "divide", "oslash", "ugrave", "uacute", "ucircumflex", "udieresis", "yacute",
    "thorn", "ydieresis",
];

/// The `cff_font` fields `t1_load_font` fills.
struct T1Font {
    topdict: Dict,
    private: Dict,
    /// `cff->_string` while loading, `cff->string` after `cff_update_string`
    strings: Vec<Vec<u8>>,
    /// `cff->charsets` (format 0): the SID of glyph `gid` is `charset[gid - 1]`
    charset: Vec<u16>,
    /// `cff->cstrings`, indexed by gid (`.notdef` is 0)
    cstrings: Vec<Vec<u8>>,
    /// `cff->subrs[0]`
    subrs: Option<Vec<Vec<u8>>>,
}

/// `cff_add_string(.., unique = 0)`
fn add_string_plain(strings: &mut Vec<Vec<u8>>, s: &[u8]) -> u16 {
    strings.push(s.to_vec());
    (strings.len() - 1 + CFF_STDSTR_MAX) as u16
}

/// `cff_add_string(.., unique = 1)`
fn add_string_unique(strings: &mut Vec<Vec<u8>>, s: &[u8]) -> u16 {
    if let Some(i) = STD_STRINGS.iter().position(|x| x.as_bytes() == s) {
        return i as u16;
    }
    if let Some(i) = strings.iter().position(|x| x == s) {
        return (i + CFF_STDSTR_MAX) as u16;
    }
    add_string_plain(strings, s)
}

/// A C string value: cut at the first NUL.
fn c_str(v: &[u8]) -> &[u8] {
    &v[..v.iter().position(|&b| b == 0).unwrap_or(v.len())]
}

type EncVec = Vec<Option<String>>;

fn seek_operator(lex: &mut Lex, op: &str) -> R<bool> {
    while lex.pos < lex.len() {
        match lex.token()? {
            Some(tok) if tok.is_op(op) => return Ok(true),
            Some(_) => {}
            None => return Ok(false),
        }
    }
    Ok(false)
}

fn get_next_key(lex: &mut Lex) -> R<Option<Vec<u8>>> {
    while lex.pos < lex.len() {
        match lex.token()? {
            Some(Tok::Name(n)) => return Ok(Some(n)),
            Some(_) => {}
            None => break,
        }
    }
    Ok(None)
}

/// `parse_svalue`
fn parse_svalue(lex: &mut Lex) -> R<Option<Vec<u8>>> {
    Ok(match lex.token()? {
        Some(Tok::Name(v)) | Some(Tok::Str(v)) => Some(c_str(&v).to_vec()),
        _ => None,
    })
}

/// `parse_bvalue`
fn parse_bvalue(lex: &mut Lex) -> R<Option<f64>> {
    Ok(match lex.token()? {
        Some(Tok::Bool(b)) => Some(f64::from(u8::from(b))),
        _ => None,
    })
}

/// `parse_nvalue`: the number of values read (-1 on error).
fn parse_nvalue(lex: &mut Lex, value: &mut Vec<f64>, max: usize) -> R<i32> {
    value.clear();
    let Some(mut tok) = lex.token()? else { return Ok(-1) };
    let mut argn: i32 = 0;
    if tok.is_number() && max > 0 {
        value.push(tok.rv());
        argn = 1;
    } else if tok == Tok::Mark {
        // It does not distinguish '[' and '{'...
        loop {
            if lex.pos >= lex.len() {
                // the C loop ends with `tok` still the mark
                break;
            }
            match lex.token()? {
                None => return Ok(-1),
                Some(t) => tok = t,
            }
            if !(tok.is_number() && (argn as usize) < max) {
                break;
            }
            value.push(tok.rv());
            argn += 1;
        }
        if !(tok.is_op("]") || tok.is_op("}")) {
            argn = -1;
        }
    }
    Ok(argn)
}

fn named_encoding(table: &[&str; 256]) -> EncVec {
    table
        .iter()
        .map(|n| (*n != ".notdef").then(|| (*n).to_owned()))
        .collect()
}

/// `try_put_or_putinterval`
fn try_put_or_putinterval(enc: &mut EncVec, lex: &mut Lex) -> R<()> {
    let int_in = |tok: Option<Tok>, hi: i64| -> Option<i64> {
        match tok {
            Some(Tok::Int(v)) if i64::from(v) >= 0 && i64::from(v) <= hi => Some(i64::from(v)),
            _ => None,
        }
    };
    let Some(num1) = int_in(lex.token()?, 255) else { return Ok(()) };
    let tok = lex.token()?;
    match tok {
        None => {}
        Some(t) if t.is_op("exch") => {
            // dup num exch num get put
            let Some(num2) = int_in(lex.token()?, 255) else { return Ok(()) };
            if !lex.token()?.is_some_and(|t| t.is_op("get")) {
                return Ok(());
            }
            if !lex.token()?.is_some_and(|t| t.is_op("put")) {
                return Ok(());
            }
            enc[num1 as usize] = enc[num2 as usize].clone();
        }
        Some(Tok::Int(v)) if i64::from(v) + num1 <= 255 && v >= 0 => {
            // dup num1 num2 getinterval num3 exch putinterval
            let num2 = i64::from(v);
            if !lex.token()?.is_some_and(|t| t.is_op("getinterval")) {
                return Ok(());
            }
            let num3 = match lex.token()? {
                Some(Tok::Int(v)) if v >= 0 && i64::from(v) + num2 <= 255 => i64::from(v),
                _ => return Ok(()),
            };
            if !lex.token()?.is_some_and(|t| t.is_op("exch")) {
                return Ok(());
            }
            if !lex.token()?.is_some_and(|t| t.is_op("putinterval")) {
                return Ok(());
            }
            for i in 0..num2 {
                if let Some(name) = enc[(num1 + i) as usize].clone() {
                    enc[(num3 + i) as usize] = Some(name);
                }
            }
        }
        Some(_) => {}
    }
    Ok(())
}

/// `parse_encoding`; `enc` is `enc_vec` (None: encoding_id >= 0, the vector is not wanted).
fn parse_encoding(enc: &mut Option<EncVec>, lex: &mut Lex) -> R<()> {
    let tok = lex.token()?;
    if tok.as_ref().is_some_and(|t| t.is_op("StandardEncoding")) {
        if let Some(e) = enc {
            *e = named_encoding(&STANDARD_ENCODING);
        }
    } else if tok.as_ref().is_some_and(|t| t.is_op("ISOLatin1Encoding")) {
        if let Some(e) = enc {
            *e = named_encoding(&ISO_LATIN1_ENCODING);
        }
    } else if tok.as_ref().is_some_and(|t| t.is_op("ExpertEncoding")) {
        if enc.is_some() {
            return Err("ExpertEncoding not supported.".into());
        }
    } else {
        seek_operator(lex, "array")?;
        // Pick all sequences that match "dup n /Name put" until "def" or "readonly".
        while lex.pos < lex.len() {
            let Some(tok) = lex.token()? else { break };
            if tok.is_op("def") || tok.is_op("readonly") {
                break;
            }
            if !tok.is_op("dup") {
                continue;
            }
            let tok = lex.token()?;
            if tok.as_ref().is_some_and(|t| t.is_op("dup")) {
                // possibly putinterval type
                if let Some(e) = enc {
                    try_put_or_putinterval(e, lex)?;
                }
                continue;
            }
            let code = match tok {
                Some(Tok::Int(v)) if (0..=255).contains(&v) => v as usize,
                _ => continue,
            };
            let Some(Tok::Name(name)) = lex.token()? else { continue };
            if let Some(e) = enc {
                e[code] = Some(String::from_utf8_lossy(&name).into_owned());
            }
            if !lex.token()?.is_some_and(|t| t.is_op("put")) {
                if let Some(e) = enc {
                    e[code] = None;
                }
            }
        }
    }
    Ok(())
}

/// `parse_subrs` (mode 0)
fn parse_subrs(font: &mut T1Font, lex: &mut Lex, len_iv: i32) -> R<()> {
    let count = match lex.token()? {
        Some(Tok::Int(v)) if v >= 0 => v as usize,
        _ => return Err("Parsing Subrs failed.".into()),
    };
    if count == 0 {
        font.subrs = None;
        return Ok(());
    }
    if !lex.token()?.is_some_and(|t| t.is_op("array")) {
        return Err("Parsing Subrs failed.".into());
    }
    let mut subrs: Vec<Vec<u8>> = vec![Vec::new(); count];
    let mut i = 0;
    // dup subr# n-bytes RD n-binary-bytes NP
    while i < count {
        let Some(tok) = lex.token()? else { return Err("Parsing Subrs failed.".into()) };
        if tok.is_op("ND") || tok.is_op("|-") || tok.is_op("def") {
            break;
        }
        if !tok.is_op("dup") {
            continue;
        }
        let idx = match lex.token()? {
            Some(Tok::Int(v)) if v >= 0 && (v as usize) < count => v as usize,
            _ => return Err("Parsing Subrs failed.".into()),
        };
        let len = match lex.token()? {
            Some(Tok::Int(v)) if v >= 0 && v as usize <= CS_STR_LEN_MAX => v as usize,
            _ => return Err("Parsing Subrs failed.".into()),
        };
        let tok = lex.token()?;
        if !tok.as_ref().is_some_and(|t| t.is_op("RD") || t.is_op("-|"))
            && !seek_operator(lex, "readstring")?
        {
            return Err("Parsing Subrs failed.".into());
        }
        lex.pos += 1;
        if lex.pos + len >= lex.len() {
            return Err("Parsing Subrs failed.".into());
        }
        let raw = &lex.b[lex.pos..lex.pos + len];
        if len_iv >= 0 {
            subrs[idx] = t1_decrypt(T1_CHARKEY, raw, len_iv as usize);
        } else if len > 0 {
            subrs[idx] = raw.to_vec();
        }
        lex.pos += len;
        i += 1;
    }
    if font.subrs.is_none() {
        font.subrs = Some(subrs);
    }
    // else: Adobe's OPO_____.PFB and OPBO____.PFB have two /Subrs dicts; the other is ignored
    Ok(())
}

/// `parse_charstrings` (mode 0)
fn parse_charstrings(font: &mut T1Font, lex: &mut Lex, len_iv: i32) -> R<()> {
    // /CharStrings n dict dup begin
    // /GlyphName n-bytes RD -n-binary-bytes- ND
    // ...
    let count = match lex.token()? {
        Some(Tok::Int(v)) if v >= 0 && v <= 64999 => v as usize,
        // Ignores non dict "/CharStrings ..."
        _ => return Ok(()),
    };
    let mut cstrings: Vec<Vec<u8>> = vec![Vec::new(); count];
    let mut charset: Vec<u16> = vec![0; count.saturating_sub(1)];
    seek_operator(lex, "begin")?;
    let mut have_notdef = false;
    for i in 0..count {
        let tok = lex.token()?;
        let gid;
        let glyph_name;
        match tok {
            Some(Tok::Name(name)) => {
                glyph_name = c_str(&name).to_vec();
                if glyph_name == b".notdef" {
                    gid = 0;
                    have_notdef = true;
                } else if have_notdef {
                    gid = i;
                } else if i == count - 1 {
                    return Err("No .notdef glyph???".into());
                } else {
                    gid = i + 1;
                }
            }
            Some(t) if t.is_op("end") => break,
            _ => return Err("Parsing CharStrings failed.".into()),
        }
        if gid > 0 {
            charset[gid - 1] = add_string_plain(&mut font.strings, &glyph_name);
        }
        let len = match lex.token()? {
            Some(Tok::Int(v)) if v >= 0 && v as usize <= CS_STR_LEN_MAX => v as usize,
            _ => return Err("Parsing CharStrings failed.".into()),
        };
        let tok = lex.token()?;
        if !tok.as_ref().is_some_and(|t| t.is_op("RD") || t.is_op("-|"))
            && !seek_operator(lex, "readstring")?
        {
            return Err("Parsing CharStrings failed.".into());
        }
        if lex.pos + len + 1 >= lex.len() {
            return Err("Parsing CharStrings failed.".into());
        }
        lex.pos += 1;
        let raw = &lex.b[lex.pos..lex.pos + len];
        cstrings[gid] = if len_iv >= 0 {
            t1_decrypt(T1_CHARKEY, raw, len_iv as usize)
        } else {
            raw.to_vec()
        };
        lex.pos += len;
        let tok = lex.token()?;
        if !tok.as_ref().is_some_and(|t| t.is_op("ND") || t.is_op("|-")) {
            return Err("Parsing CharStrings failed.".into());
        }
    }
    font.cstrings = cstrings;
    font.charset = charset;
    Ok(())
}

/// `parse_part2`
fn parse_part2(font: &mut T1Font, lex: &mut Lex) -> R<()> {
    let mut len_iv: i32 = 4;
    let mut argv: Vec<f64> = Vec::new();
    while lex.pos < lex.len() {
        let Some(key) = get_next_key(lex)? else { break };
        let key_s = String::from_utf8_lossy(&key).into_owned();
        match key_s.as_str() {
            "Subrs" => parse_subrs(font, lex, len_iv)?,
            "CharStrings" => parse_charstrings(font, lex, len_iv)?,
            "lenIV" => {
                let argn = parse_nvalue(lex, &mut argv, 1)?;
                if argn != 1 {
                    return Err(format!("{} values expected but only {argn} read.", 1));
                }
                len_iv = argv[0] as i32;
            }
            "BlueValues" | "OtherBlues" | "FamilyBlues" | "FamilyOtherBlues" | "StemSnapH"
            | "StemSnapV" => {
                // Operand values are delta in CFF font dictionary encoding.
                let argn = parse_nvalue(lex, &mut argv, 127)?;
                if argn < 0 {
                    return Err(format!("0 values expected but only {argn} read."));
                }
                font.private.add(&key_s, argn as usize)?;
                for k in (0..argn as usize).rev() {
                    let v = if k == 0 { argv[0] } else { argv[k] - argv[k - 1] };
                    font.private.set(&key_s, k, v)?;
                }
            }
            "StdHW" | "StdVW" | "BlueScale" | "BlueShift" | "BlueFuzz" | "LanguageGroup"
            | "ExpansionFactor" => {
                // StdHW and StdVW: an array in Type 1, a number in CFF
                let argn = parse_nvalue(lex, &mut argv, 1)?;
                if argn != 1 {
                    return Err(format!("1 values expected but only {argn} read."));
                }
                font.private.add(&key_s, 1)?;
                font.private.set(&key_s, 0, argv[0])?;
            }
            "ForceBold" => {
                let Some(v) = parse_bvalue(lex)? else {
                    return Err("1 values expected but only -1 read.".into());
                };
                if v != 0.0 {
                    font.private.add(&key_s, 1)?;
                    font.private.set(&key_s, 0, 1.0)?;
                }
            }
            // MinFeature, RndStemUp, UniqueID, Password ignored.
            _ => {}
        }
    }
    Ok(())
}

/// `parse_part1`
fn parse_part1(font: &mut T1Font, enc: &mut Option<EncVec>, lex: &mut Lex) -> R<()> {
    // skip PostScript code inserted before the beginning of the font dictionary
    if !seek_operator(lex, "begin")? {
        return Err("Reading PFB (ASCII part) file failed.".into());
    }
    let mut argv: Vec<f64> = Vec::new();
    while lex.pos < lex.len() {
        let Some(key) = get_next_key(lex)? else { break };
        let key_s = String::from_utf8_lossy(&key).into_owned();
        let expect = |argn: i32, n: i32| -> R<()> {
            if argn == n {
                Ok(())
            } else {
                Err(format!("{n} values expected but only {argn} read."))
            }
        };
        match key_s.as_str() {
            "Encoding" => parse_encoding(enc, lex)?,
            "FontName" => {
                let Some(name) = parse_svalue(lex)? else {
                    return Err("1 values expected but only -1 read.".into());
                };
                if name.len() > 127 {
                    // cff_set_name is applied to the truncated name; replaced afterwards anyway
                }
            }
            "FontType" => {
                let argn = parse_nvalue(lex, &mut argv, 1)?;
                expect(argn, 1)?;
                if argv[0] != 1.0 {
                    return Err(format!("FontType {} not supported.", argv[0] as i32));
                }
            }
            "ItalicAngle" | "StrokeWidth" | "PaintType" => {
                let argn = parse_nvalue(lex, &mut argv, 1)?;
                expect(argn, 1)?;
                if argv[0] != 0.0 {
                    font.topdict.add(&key_s, 1)?;
                    font.topdict.set(&key_s, 0, argv[0])?;
                }
            }
            "UnderLinePosition" | "UnderLineThickness" => {
                // (the C spelling never matches a font's `UnderlinePosition`; and if it did
                // `cff_dict_add` would abort on the unknown operator)
                let argn = parse_nvalue(lex, &mut argv, 1)?;
                expect(argn, 1)?;
                return Err("CFF: Unknown CFF DICT operator.".into());
            }
            "FontBBox" => {
                let argn = parse_nvalue(lex, &mut argv, 4)?;
                expect(argn, 4)?;
                font.topdict.add(&key_s, 4)?;
                for k in (0..4).rev() {
                    font.topdict.set(&key_s, k, argv[k])?;
                }
            }
            "FontMatrix" => {
                let argn = parse_nvalue(lex, &mut argv, 6)?;
                expect(argn, 6)?;
                if argv[0] != 0.001
                    || argv[1] != 0.0
                    || argv[2] != 0.0
                    || argv[3] != 0.001
                    || argv[4] != 0.0
                    || argv[5] != 0.0
                {
                    font.topdict.add(&key_s, 6)?;
                    for k in (0..6).rev() {
                        font.topdict.set(&key_s, k, argv[k])?;
                    }
                }
            }
            "version" | "Notice" | "FullName" | "FamilyName" | "Weight" | "Copyright" => {
                // FontInfo
                let Some(strval) = parse_svalue(lex)? else {
                    return Err("1 values expected but only -1 read.".into());
                };
                font.topdict.add(&key_s, 1)?;
                // `cff_get_sid` finds only the standard strings here (`cff->string` is empty)
                let sid = match STD_STRINGS.iter().position(|x| x.as_bytes() == strval.as_slice()) {
                    Some(i) => i as u16,
                    None => add_string_plain(&mut font.strings, &strval),
                };
                font.topdict.set(&key_s, 0, f64::from(sid))?;
            }
            "IsFixedPitch" => {
                let Some(v) = parse_bvalue(lex)? else {
                    return Err("1 values expected but only -1 read.".into());
                };
                if v != 0.0 {
                    font.private.add(&key_s, 1)?;
                    font.private.set(&key_s, 0, 1.0)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// `t1_get_fontname`
fn t1_get_fontname(clear: &[u8]) -> R<String> {
    let mut lex = Lex::new(clear);
    if !seek_operator(&mut lex, "begin")? {
        return Err("Failed to read Type 1 font.".into());
    }
    while lex.pos < lex.len() {
        let Some(key) = get_next_key(&mut lex)? else { break };
        if key == b"FontName" {
            if let Some(name) = parse_svalue(&mut lex)? {
                let n = &name[..name.len().min(127)];
                return Ok(String::from_utf8_lossy(n).into_owned());
            }
        }
    }
    Ok(String::new())
}

/// `t1_load_font` (mode 0): `clear` is the ASCII segment, `binary` the (encrypted) eexec part.
fn t1_load_font(clear: &[u8], binary: &[u8], enc: &mut Option<EncVec>) -> R<T1Font> {
    let mut font = T1Font {
        topdict: Dict::default(),
        private: Dict::default(),
        strings: Vec::new(),
        charset: Vec::new(),
        cstrings: Vec::new(),
        subrs: None,
    };
    parse_part1(&mut font, enc, &mut Lex::new(clear))?;
    let decrypted = t1_decrypt(T1_EEKEY, binary, 0);
    if decrypted.len() < 4 {
        return Err("Reading PFB (BINARY part) file failed.".into());
    }
    parse_part2(&mut font, &mut Lex::new(&decrypted[4..]))?;
    Ok(font)
}

impl T1Font {
    /// `cff_match_string`
    fn match_string(&self, glyph: &[u8], sid: u16) -> bool {
        let sid = sid as usize;
        if sid < CFF_STDSTR_MAX {
            STD_STRINGS[sid].as_bytes() == glyph
        } else {
            self.strings.get(sid - CFF_STDSTR_MAX).is_some_and(|s| s == glyph)
        }
    }

    /// `cff_glyph_lookup`: 0 (`.notdef`) when the glyph is not in the font
    fn glyph_lookup(&self, glyph: Option<&str>) -> usize {
        glyph_lookup(&self.charset, |sid, g| self.match_string(g, sid), glyph)
    }
}

fn glyph_lookup(charset: &[u16], matches: impl Fn(u16, &[u8]) -> bool, glyph: Option<&str>) -> usize {
    let Some(glyph) = glyph.filter(|g| *g != ".notdef") else { return 0 };
    charset
        .iter()
        .position(|&sid| matches(sid, glyph.as_bytes()))
        .map_or(0, |i| i + 1)
}

// ---------------------------------------------------------------------------------------------
// t1_char.c: Type 1 charstring → Type 2 charstring
// ---------------------------------------------------------------------------------------------

const CS_STACK_ERROR: i32 = -2;
const CS_PARSE_ERROR: i32 = -1;
const CS_PARSE_OK: i32 = 0;
const CS_SUBR_RETURN: i32 = 2;
const CS_CHAR_END: i32 = 3;

const T1_CS_PHASE_INIT: i32 = 0;
const T1_CS_PHASE_PATH: i32 = 2;
const T1_CS_PHASE_FLEX: i32 = 3;

const HSTEM: u8 = 0;
const VSTEM: u8 = 1;

const T1_CS_FLAG_USE_HINTMASK: u32 = 1 << 0;
const T1_CS_FLAG_USE_CNTRMASK: u32 = 1 << 1;
const T1_CS_FLAG_USE_SEAC: u32 = 1 << 2;

const CS_HINT_DECL: i32 = -1;
const CS_FLEX_CTRL: i32 = -2;

const CS_HSTEM: i32 = 1;
const CS_VSTEM: i32 = 3;
const CS_VMOVETO: i32 = 4;
const CS_RLINETO: i32 = 5;
const CS_HLINETO: i32 = 6;
const CS_VLINETO: i32 = 7;
const CS_RRCURVETO: i32 = 8;
const CS_CLOSEPATH: i32 = 9;
const CS_HSBW: i32 = 13;
const CS_ENDCHAR: i32 = 14;
const CS_HSTEMHM: i32 = 18;
const CS_HINTMASK: i32 = 19;
const CS_CNTRMASK: i32 = 20;
const CS_RMOVETO: i32 = 21;
const CS_HMOVETO: i32 = 22;
const CS_VSTEMHM: i32 = 23;
const CS_RCURVELINE: i32 = 24;
const CS_RLINECURVE: i32 = 25;
const CS_VHCURVETO: i32 = 30;
const CS_HVCURVETO: i32 = 31;
const CS_ESCAPE: i32 = 12;

const CS_DOTSECTION: u8 = 0;
const CS_VSTEM3: u8 = 1;
const CS_HSTEM3: u8 = 2;
const CS_SEAC: u8 = 6;
const CS_SBW: u8 = 7;
const CS_DIV: u8 = 12;
const CS_CALLOTHERSUBR: u8 = 16;
const CS_POP: u8 = 17;
const CS_SETCURRENTPOINT: u8 = 33;
const CS_HFLEX: i32 = 34;
const CS_FLEX: i32 = 35;
const CS_HFLEX1: i32 = 36;

fn is_path_operator(o: i32) -> bool {
    (CS_VMOVETO..=CS_CLOSEPATH).contains(&o)
        || ((CS_RMOVETO..=CS_HVCURVETO).contains(&o) && o != CS_VSTEMHM && o != 29 && o != 28)
}

#[derive(Clone, Debug)]
struct CPath {
    ty: i32,
    args: Vec<f64>,
}

#[derive(Clone, Copy, Debug)]
struct Stem {
    id: i32,
    dir: u8,
    pos: f64,
    del: f64,
}

#[derive(Default)]
struct Chardesc {
    flags: u32,
    sbx: f64,
    sby: f64,
    wx: f64,
    wy: f64,
    /// llx, lly, urx, ury
    bbox: [f64; 4],
    seac_adx: f64,
    seac_ady: f64,
    seac_bchar: u8,
    seac_achar: u8,
    stems: Vec<Stem>,
    path: Vec<CPath>,
}

/// `t1_ginfo`
#[derive(Default, Clone)]
struct Ginfo {
    wx: f64,
    bbox: [f64; 4],
    use_seac: bool,
    seac_bchar: u8,
    seac_achar: u8,
}

impl Chardesc {
    /// `add_stem`: the stem id, or -1 when there are too many
    fn add_stem(&mut self, pos: f64, del: f64, dir: u8) -> i32 {
        let pos = pos + if dir == HSTEM { self.sby } else { self.sbx };
        if let Some(s) = self
            .stems
            .iter()
            .find(|s| s.dir == dir && s.pos == pos && s.del == del)
        {
            return s.id;
        }
        if self.stems.len() == CS_STEM_ZONE_MAX {
            return -1;
        }
        let id = self.stems.len() as i32;
        self.stems.push(Stem { id, dir, pos, del });
        id
    }

    /// `get_stem`
    fn get_stem(&self, id: i32) -> R<usize> {
        self.stems
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| "Type 1 charstring: unknown stem".to_string())
    }
}

struct Interp<'a> {
    subrs: Option<&'a [Vec<u8>]>,
    status: i32,
    phase: i32,
    nest: i32,
    cs: [f64; CS_ARG_STACK_MAX + 1],
    cs_top: usize,
    ps: [f64; PS_ARG_STACK_MAX],
    ps_top: usize,
}

impl<'a> Interp<'a> {
    fn new(subrs: Option<&'a [Vec<u8>]>) -> Self {
        Interp {
            subrs,
            status: CS_PARSE_OK,
            phase: T1_CS_PHASE_INIT,
            nest: 0,
            cs: [0.0; CS_ARG_STACK_MAX + 1],
            cs_top: 0,
            ps: [0.0; PS_ARG_STACK_MAX],
            ps_top: 0,
        }
    }

    /// `RESET_STATE`
    fn reset(&mut self) {
        self.status = CS_PARSE_OK;
        self.phase = T1_CS_PHASE_INIT;
        self.nest = 0;
        self.ps_top = 0;
    }

    /// `add_charpath`
    fn add_charpath(&mut self, cd: &mut Chardesc, ty: i32, args: &[f64]) {
        cd.path.push(CPath { ty, args: args.to_vec() });
        if ty >= 0 && self.phase != T1_CS_PHASE_FLEX && is_path_operator(ty) {
            self.phase = T1_CS_PHASE_PATH;
        }
    }

    /// `ADD_PATH(cd, ty, n)`
    fn add_path(&mut self, cd: &mut Chardesc, ty: i32, n: usize) {
        let args = self.cs[self.cs_top - n..self.cs_top].to_vec();
        self.add_charpath(cd, ty, &args);
    }

    fn do_operator1(&mut self, cd: &mut Chardesc, data: &[u8], pos: &mut usize) {
        let mut op = i32::from(data[*pos]);
        *pos += 1;
        macro_rules! check {
            ($n:expr) => {
                if self.cs_top < $n {
                    self.status = CS_STACK_ERROR;
                    return;
                }
            };
        }
        match op {
            CS_CLOSEPATH => self.cs_top = 0,
            CS_HSBW => {
                check!(2);
                self.cs_top -= 1;
                cd.wx = self.cs[self.cs_top];
                cd.wy = 0.0;
                self.cs_top -= 1;
                cd.sbx = self.cs[self.cs_top];
                cd.sby = 0.0;
                self.cs_top = 0;
            }
            CS_HSTEM | CS_VSTEM => {
                check!(2);
                let id = cd.add_stem(
                    self.cs[self.cs_top - 2],
                    self.cs[self.cs_top - 1],
                    if op == CS_HSTEM { HSTEM } else { VSTEM },
                );
                if id < 0 {
                    // Too many hints...
                    self.status = CS_PARSE_ERROR;
                    return;
                }
                self.cs[self.cs_top] = f64::from(id);
                self.cs_top += 1;
                self.add_path(cd, CS_HINT_DECL, 1);
                self.cs_top = 0;
            }
            CS_RMOVETO => {
                check!(2);
                if self.phase < T1_CS_PHASE_PATH {
                    self.cs[self.cs_top - 2] += cd.sbx;
                    self.cs[self.cs_top - 1] += cd.sby;
                }
                self.add_path(cd, op, 2);
                self.cs_top = 0;
            }
            CS_HMOVETO | CS_VMOVETO => {
                check!(1);
                let mut argn = 1;
                if self.phase < T1_CS_PHASE_PATH {
                    // the reference point of the first moveto differs between Type 1 and 2
                    if op == CS_HMOVETO {
                        self.cs[self.cs_top - 1] += cd.sbx;
                        if cd.sby != 0.0 {
                            self.cs[self.cs_top] = cd.sby;
                            self.cs_top += 1;
                            argn = 2;
                            op = CS_RMOVETO;
                        }
                    } else {
                        self.cs[self.cs_top - 1] += cd.sby;
                        if cd.sbx != 0.0 {
                            self.cs[self.cs_top] = self.cs[self.cs_top - 1];
                            self.cs[self.cs_top - 1] = cd.sbx;
                            self.cs_top += 1;
                            argn = 2;
                            op = CS_RMOVETO;
                        }
                    }
                }
                self.add_path(cd, op, argn);
                self.cs_top = 0;
            }
            CS_ENDCHAR => {
                self.status = CS_CHAR_END;
                self.cs_top = 0;
            }
            CS_RLINETO => {
                check!(2);
                self.add_path(cd, op, 2);
                self.cs_top = 0;
            }
            CS_HLINETO | CS_VLINETO => {
                check!(1);
                self.add_path(cd, op, 1);
                self.cs_top = 0;
            }
            CS_RRCURVETO => {
                check!(6);
                self.add_path(cd, op, 6);
                self.cs_top = 0;
            }
            CS_VHCURVETO | CS_HVCURVETO => {
                check!(4);
                self.add_path(cd, op, 4);
                self.cs_top = 0;
            }
            11 => {}
            _ => self.status = CS_PARSE_ERROR,
        }
    }

    /// `do_othersubr0`: six flex control points become one flex path.
    fn do_othersubr0(&mut self, cd: &mut Chardesc) {
        if self.ps_top < 1 {
            self.status = CS_PARSE_ERROR;
            return;
        }
        let Some(first) = cd.path.iter().position(|p| p.ty == CS_FLEX_CTRL) else {
            self.status = CS_PARSE_ERROR;
            return;
        };
        // the six marks that follow must end the path
        if first + 7 != cd.path.len() {
            self.status = CS_PARSE_ERROR;
            return;
        }
        let mut args = cd.path[first].args.clone();
        args.resize(13, 0.0);
        for i in 1..7 {
            let cur = &cd.path[first + i];
            if cur.ty != CS_FLEX_CTRL || cur.args.len() != 2 {
                self.status = CS_PARSE_ERROR;
                return;
            }
            if i == 1 {
                args[0] += cur.args[0];
                args[1] += cur.args[1];
            } else {
                args[2 * i - 2] = cur.args[0];
                args[2 * i - 1] = cur.args[1];
            }
        }
        // the first pair is relative from the starting point
        self.ps_top -= 1;
        args[12] = self.ps[self.ps_top]; // flex depth
        cd.path.truncate(first + 1);
        cd.path[first] = CPath { ty: CS_FLEX, args };
        self.phase = T1_CS_PHASE_PATH;
    }

    /// `do_othersubr2`: mark a flex control point
    fn do_othersubr2(&mut self, cd: &mut Chardesc) {
        if self.phase != T1_CS_PHASE_FLEX || cd.path.is_empty() {
            self.status = CS_PARSE_ERROR;
            return;
        }
        let last = cd.path.last_mut().unwrap();
        match last.ty {
            CS_RMOVETO => {}
            CS_HMOVETO => {
                last.args.truncate(1);
                last.args.push(0.0);
            }
            CS_VMOVETO => {
                let a0 = last.args[0];
                last.args.truncate(1);
                last.args.push(a0);
                last.args[0] = 0.0;
            }
            _ => {
                self.status = CS_PARSE_ERROR;
                return;
            }
        }
        last.ty = CS_FLEX_CTRL;
    }

    /// `do_othersubr13`: counter control
    fn do_othersubr13(&mut self, cd: &mut Chardesc) {
        // After #12 callothersubr or hsbw or sbw.
        if self.phase != T1_CS_PHASE_INIT {
            self.status = CS_PARSE_ERROR;
            return;
        }
        let mut groups: Vec<Vec<f64>> = vec![Vec::new(); CS_STEM_ZONE_MAX];
        let mut num_groups = [0usize; 2];
        for (k, dir) in [HSTEM, VSTEM].into_iter().enumerate() {
            if self.ps_top < 1 {
                self.status = CS_STACK_ERROR;
                return;
            }
            self.ps_top -= 1;
            let num = self.ps[self.ps_top] as i32;
            if num < 0 || num as usize > CS_STEM_ZONE_MAX {
                self.status = CS_PARSE_ERROR;
                return;
            }
            let num = num as usize;
            let mut n = 0usize;
            let mut pos = 0.0;
            while self.ps_top >= 2 && n < num {
                // add_stem() adds the side bearing
                self.ps_top -= 1;
                pos += self.ps[self.ps_top];
                self.ps_top -= 1;
                let del = self.ps[self.ps_top];
                let id = cd.add_stem(
                    if del < 0.0 { pos + del } else { pos },
                    if del < 0.0 { -del } else { del },
                    dir,
                );
                if id < 0 || groups[n].len() >= CS_STEM_ZONE_MAX {
                    self.status = CS_PARSE_ERROR;
                    return;
                }
                groups[n].push(f64::from(id));
                pos += del;
                if del < 0.0 {
                    pos = 0.0;
                    n += 1;
                }
            }
            if n != num {
                self.status = CS_STACK_ERROR;
                return;
            }
            num_groups[k] = num;
        }
        for group in groups.iter().take(num_groups[0].max(num_groups[1])) {
            self.add_charpath(cd, CS_CNTRMASK, group);
        }
        cd.flags |= T1_CS_FLAG_USE_CNTRMASK;
    }

    fn do_callothersubr(&mut self, cd: &mut Chardesc) {
        if self.cs_top < 2 {
            self.status = CS_STACK_ERROR;
            return;
        }
        self.cs_top -= 1;
        let subrno = self.cs[self.cs_top] as i32;
        self.cs_top -= 1;
        let mut argn = self.cs[self.cs_top] as i32;
        if (self.cs_top as i32) < argn {
            self.status = CS_STACK_ERROR;
            return;
        }
        if self.ps_top as i32 + argn > PS_ARG_STACK_MAX as i32 {
            self.status = CS_PARSE_ERROR;
            return;
        }
        while argn > 0 {
            argn -= 1;
            self.cs_top -= 1;
            self.ps[self.ps_top] = self.cs[self.cs_top];
            self.ps_top += 1;
        }
        match subrno {
            0 => self.do_othersubr0(cd),
            1 => self.phase = T1_CS_PHASE_FLEX,
            2 => self.do_othersubr2(cd),
            3 => cd.flags |= T1_CS_FLAG_USE_HINTMASK,
            12 => {
                // Othersubr12 call must immediately follow the hsbw or sbw.
                if self.phase != T1_CS_PHASE_INIT {
                    self.status = CS_PARSE_ERROR;
                }
            }
            13 => self.do_othersubr13(cd),
            _ => self.status = CS_PARSE_ERROR, // Unknown othersubr
        }
    }

    fn do_operator2(&mut self, cd: &mut Chardesc, data: &[u8], pos: &mut usize) {
        *pos += 1;
        if data.len() < *pos + 1 {
            self.status = CS_PARSE_ERROR;
            return;
        }
        let op = data[*pos];
        *pos += 1;
        macro_rules! check {
            ($n:expr) => {
                if self.cs_top < $n {
                    self.status = CS_STACK_ERROR;
                    return;
                }
            };
        }
        match op {
            CS_SBW => {
                check!(4);
                self.cs_top -= 1;
                cd.wy = self.cs[self.cs_top];
                self.cs_top -= 1;
                cd.wx = self.cs[self.cs_top];
                self.cs_top -= 1;
                cd.sby = self.cs[self.cs_top];
                self.cs_top -= 1;
                cd.sbx = self.cs[self.cs_top];
                self.cs_top = 0;
            }
            CS_HSTEM3 | CS_VSTEM3 => {
                check!(6);
                for i in (0..=2usize).rev() {
                    let id = cd.add_stem(
                        self.cs[self.cs_top - 2 * i - 2],
                        self.cs[self.cs_top - 2 * i - 1],
                        if op == CS_HSTEM3 { HSTEM } else { VSTEM },
                    );
                    if id < 0 {
                        self.status = CS_PARSE_ERROR;
                        return;
                    }
                    self.cs[self.cs_top] = f64::from(id);
                    self.cs_top += 1;
                    self.add_path(cd, CS_HINT_DECL, 1);
                    self.cs_top -= 1;
                }
                self.cs_top = 0;
            }
            CS_SETCURRENTPOINT => {
                check!(2);
                self.cs_top = 0;
            }
            CS_POP => {
                // PS interpreter operand stack to the BuildChar operand stack
                if self.ps_top < 1 {
                    self.status = CS_PARSE_ERROR;
                    return;
                }
                if self.cs_top + 1 > CS_ARG_STACK_MAX {
                    self.status = CS_STACK_ERROR;
                    return;
                }
                self.ps_top -= 1;
                self.cs[self.cs_top] = self.ps[self.ps_top];
                self.cs_top += 1;
            }
            CS_DOTSECTION => {}
            CS_DIV => {
                check!(2);
                self.cs[self.cs_top - 2] /= self.cs[self.cs_top - 1];
                self.cs_top -= 1;
            }
            CS_CALLOTHERSUBR => self.do_callothersubr(cd),
            CS_SEAC => {
                check!(5);
                cd.flags |= T1_CS_FLAG_USE_SEAC;
                self.cs_top -= 1;
                cd.seac_achar = self.cs[self.cs_top] as i32 as u8;
                self.cs_top -= 1;
                cd.seac_bchar = self.cs[self.cs_top] as i32 as u8;
                self.cs_top -= 1;
                cd.seac_ady = self.cs[self.cs_top];
                self.cs_top -= 1;
                cd.seac_adx = self.cs[self.cs_top];
                // compensate the difference of the glyph origin
                cd.seac_ady += cd.sby;
                self.cs_top -= 1;
                cd.seac_adx += cd.sbx - self.cs[self.cs_top];
                self.cs_top = 0;
            }
            _ => self.status = CS_PARSE_ERROR,
        }
    }

    fn get_integer(&mut self, data: &[u8], pos: &mut usize) {
        let b0 = data[*pos];
        *pos += 1;
        let result: i32 = if b0 == 28 {
            // shortint
            if data.len() < *pos + 2 {
                self.status = CS_PARSE_ERROR;
                return;
            }
            let v = i32::from(data[*pos]) * 256 + i32::from(data[*pos + 1]);
            *pos += 2;
            if v > 0x7fff {
                v - 0x10000
            } else {
                v
            }
        } else if (32..=246).contains(&b0) {
            i32::from(b0) - 139
        } else if (247..=250).contains(&b0) {
            if data.len() < *pos + 1 {
                self.status = CS_PARSE_ERROR;
                return;
            }
            let v = (i32::from(b0) - 247) * 256 + i32::from(data[*pos]) + 108;
            *pos += 1;
            v
        } else if (251..=254).contains(&b0) {
            if data.len() < *pos + 1 {
                self.status = CS_PARSE_ERROR;
                return;
            }
            let v = -(i32::from(b0) - 251) * 256 - i32::from(data[*pos]) - 108;
            *pos += 1;
            v
        } else {
            self.status = CS_PARSE_ERROR;
            return;
        };
        if self.cs_top + 1 > CS_ARG_STACK_MAX {
            self.status = CS_STACK_ERROR;
            return;
        }
        self.cs[self.cs_top] = f64::from(result);
        self.cs_top += 1;
    }

    /// `get_longint` (Type 1: 32-bit integer)
    fn get_longint(&mut self, data: &[u8], pos: &mut usize) {
        *pos += 1;
        if data.len() < *pos + 4 {
            self.status = CS_PARSE_ERROR;
            return;
        }
        let mut result = i32::from(data[*pos] as i8);
        *pos += 1;
        for _ in 1..4 {
            result = result.wrapping_mul(256).wrapping_add(i32::from(data[*pos]));
            *pos += 1;
        }
        if self.cs_top + 1 > CS_ARG_STACK_MAX {
            self.status = CS_STACK_ERROR;
            return;
        }
        self.cs[self.cs_top] = f64::from(result);
        self.cs_top += 1;
    }

    /// `t1char_build_charpath`: parse a charstring and build the charpath
    fn build_charpath(&mut self, cd: &mut Chardesc, data: &[u8], pos: &mut usize) -> R<()> {
        if self.nest > CS_SUBR_NEST_MAX {
            return Err("Subroutine nested too deeply.".into());
        }
        self.nest += 1;
        while *pos < data.len() && self.status == CS_PARSE_OK {
            let b0 = data[*pos];
            if b0 == 255 {
                self.get_longint(data, pos);
            } else if i32::from(b0) == 11 {
                self.status = CS_SUBR_RETURN;
            } else if b0 == 10 {
                if self.cs_top < 1 {
                    self.status = CS_STACK_ERROR;
                } else {
                    self.cs_top -= 1;
                    let idx = self.cs[self.cs_top] as i32;
                    let subr = match self.subrs {
                        Some(subrs) if idx >= 0 && (idx as usize) < subrs.len() => &subrs[idx as usize],
                        _ => return Err("Invalid Subr#.".into()),
                    };
                    let mut p = 0usize;
                    self.build_charpath(cd, subr, &mut p)?;
                    *pos += 1;
                }
            } else if i32::from(b0) == CS_ESCAPE {
                self.do_operator2(cd, data, pos);
            } else if b0 < 32 && b0 != 28 {
                self.do_operator1(cd, data, pos);
            } else {
                self.get_integer(data, pos);
            }
        }
        if self.status == CS_SUBR_RETURN {
            self.status = CS_PARSE_OK;
        } else if self.status < CS_PARSE_OK {
            return Err(format!(
                "Parsing charstring failed: (status={}, stack={})",
                self.status, self.cs_top
            ));
        }
        self.nest -= 1;
        Ok(())
    }
}

/// `put_numbers` (Type 2 five-byte encoding)
fn put_numbers(argv: &[f64], dest: &mut Vec<u8>) -> R<()> {
    for &value in argv {
        let mut ivalue = (value + 0.5).floor() as i32;
        if value >= 32768.0 || value <= -32769.0 || value.is_nan() {
            // This number cannot be represented as a single operand.
            return Err("Argument value too large. (This is bug)".into());
        } else if (value - f64::from(ivalue)).abs() > 3.0e-5 {
            // 16.16-bit signed fixed value
            dest.push(255);
            ivalue = value.floor() as i32; // mantissa
            dest.push((ivalue >> 8) as u8);
            dest.push(ivalue as u8);
            let frac = ((value - f64::from(ivalue)) * 65536.0) as i32; // fraction
            dest.push((frac >> 8) as u8);
            dest.push(frac as u8);
        } else if (-107..=107).contains(&ivalue) {
            dest.push((ivalue + 139) as u8);
        } else if (108..=1131).contains(&ivalue) {
            let v = 0xf700 + ivalue - 108;
            dest.push((v >> 8) as u8);
            dest.push(v as u8);
        } else if (-1131..=-108).contains(&ivalue) {
            let v = 0xfb00 - ivalue - 108;
            dest.push((v >> 8) as u8);
            dest.push(v as u8);
        } else {
            // shortint
            dest.push(28);
            dest.push((ivalue >> 8) as u8);
            dest.push(ivalue as u8);
        }
    }
    Ok(())
}

/// `do_postproc`: the bounding box, and a very simple charstring compression
fn do_postproc(cd: &mut Chardesc) -> R<()> {
    if cd.path.is_empty() {
        return Ok(());
    }
    // dummy large values
    cd.bbox = [100000.0, 100000.0, -100000.0, -100000.0];
    let (mut x, mut y) = (0.0f64, 0.0f64);
    let update = |bbox: &mut [f64; 4], x: f64, y: f64| {
        if bbox[0] > x {
            bbox[0] = x;
        }
        if bbox[2] < x {
            bbox[2] = x;
        }
        if bbox[1] > y {
            bbox[1] = y;
        }
        if bbox[3] < y {
            bbox[3] = y;
        }
    };
    let old = std::mem::take(&mut cd.path);
    let mut out: Vec<CPath> = Vec::with_capacity(old.len());
    for mut cur in old {
        // `prev` is the last path element that survived; TRY_COMPACT
        let can_compact = |out: &Vec<CPath>, cur: &CPath| {
            out.last()
                .is_some_and(|prev| prev.args.len() + cur.args.len() < CS_ARG_STACK_MAX)
        };
        let mut merged = false;
        match cur.ty {
            CS_RMOVETO => {
                x += cur.args[0];
                y += cur.args[1];
                update(&mut cd.bbox, x, y);
            }
            CS_RLINETO => {
                x += cur.args[0];
                y += cur.args[1];
                update(&mut cd.bbox, x, y);
                if can_compact(&out, &cur) {
                    let prev = out.last_mut().unwrap();
                    if prev.ty == CS_RLINETO {
                        prev.args.extend_from_slice(&cur.args);
                        merged = true;
                    } else if prev.ty == CS_RRCURVETO {
                        prev.args.extend_from_slice(&cur.args);
                        prev.ty = CS_RCURVELINE;
                        merged = true;
                    }
                }
            }
            CS_HMOVETO => {
                x += cur.args[0];
                update(&mut cd.bbox, x, y);
            }
            CS_HLINETO => {
                x += cur.args[0];
                update(&mut cd.bbox, x, y);
                if can_compact(&out, &cur) {
                    let prev = out.last_mut().unwrap();
                    if (prev.ty == CS_VLINETO && prev.args.len() % 2 == 1)
                        || (prev.ty == CS_HLINETO && prev.args.len() % 2 == 0)
                    {
                        prev.args.extend_from_slice(&cur.args);
                        merged = true;
                    }
                }
            }
            CS_VMOVETO => {
                y += cur.args[0];
                update(&mut cd.bbox, x, y);
            }
            CS_VLINETO => {
                y += cur.args[0];
                update(&mut cd.bbox, x, y);
                if can_compact(&out, &cur) {
                    let prev = out.last_mut().unwrap();
                    if (prev.ty == CS_HLINETO && prev.args.len() % 2 == 1)
                        || (prev.ty == CS_VLINETO && prev.args.len() % 2 == 0)
                    {
                        prev.args.extend_from_slice(&cur.args);
                        merged = true;
                    }
                }
            }
            CS_RRCURVETO => {
                for i in 0..3 {
                    x += cur.args[2 * i];
                    y += cur.args[2 * i + 1];
                    update(&mut cd.bbox, x, y);
                }
                if can_compact(&out, &cur) {
                    let prev = out.last_mut().unwrap();
                    if prev.ty == CS_RRCURVETO {
                        prev.args.extend_from_slice(&cur.args);
                        merged = true;
                    } else if prev.ty == CS_RLINETO {
                        prev.args.extend_from_slice(&cur.args);
                        prev.ty = CS_RLINECURVE;
                        merged = true;
                    }
                }
            }
            CS_VHCURVETO => {
                y += cur.args[0];
                update(&mut cd.bbox, x, y);
                x += cur.args[1];
                y += cur.args[2];
                update(&mut cd.bbox, x, y);
                x += cur.args[3];
                update(&mut cd.bbox, x, y);
                if can_compact(&out, &cur) {
                    let prev = out.last_mut().unwrap();
                    if (prev.ty == CS_HVCURVETO && (prev.args.len() / 4) % 2 == 1)
                        || (prev.ty == CS_VHCURVETO && (prev.args.len() / 4) % 2 == 0)
                    {
                        prev.args.extend_from_slice(&cur.args);
                        merged = true;
                    }
                }
            }
            CS_HVCURVETO => {
                x += cur.args[0];
                update(&mut cd.bbox, x, y);
                x += cur.args[1];
                y += cur.args[2];
                update(&mut cd.bbox, x, y);
                y += cur.args[3];
                update(&mut cd.bbox, x, y);
                if can_compact(&out, &cur) {
                    let prev = out.last_mut().unwrap();
                    if (prev.ty == CS_VHCURVETO && (prev.args.len() / 4) % 2 == 1)
                        || (prev.ty == CS_HVCURVETO && (prev.args.len() / 4) % 2 == 0)
                    {
                        prev.args.extend_from_slice(&cur.args);
                        merged = true;
                    }
                }
            }
            CS_FLEX => {
                for i in 0..6 {
                    x += cur.args[2 * i];
                    // (the C code reads `args[2*1+1]` for every point)
                    y += cur.args[2 * 1 + 1];
                    update(&mut cd.bbox, x, y);
                }
                let a = &mut cur.args;
                if a[12] == 50.0 {
                    if a[1] == 0.0 && a[11] == 0.0 && a[5] == 0.0 && a[7] == 0.0 && a[3] + a[9] == 0.0 {
                        a[1] = a[2]; // dx2
                        a[2] = a[3]; // dy2
                        a[3] = a[4]; // dx3
                        a[4] = a[6]; // dx4
                        a[5] = a[8]; // dx5
                        a[6] = a[10]; // dx6
                        a.truncate(7);
                        cur.ty = CS_HFLEX;
                    } else if a[5] == 0.0 && a[7] == 0.0 && (a[1] + a[3] + a[9] + a[11]) == 0.0 {
                        a[5] = a[6]; // dx4
                        a[6] = a[8]; // dx5
                        a[7] = a[9]; // dy5
                        a[8] = a[10]; // dx6
                        a.truncate(9);
                        cur.ty = CS_HFLEX1;
                    }
                }
            }
            CS_HINT_DECL | CS_CNTRMASK => {}
            ty => return Err(format!("Unexpected Type 2 charstring command {ty}.")),
        }
        if !merged {
            out.push(cur);
        }
    }
    cd.path = out;
    // Had no path. Fix lower-left point.
    if cd.bbox[0] > cd.bbox[2] {
        cd.bbox[0] = cd.wx;
        cd.bbox[2] = cd.wx;
    }
    if cd.bbox[1] > cd.bbox[3] {
        cd.bbox[1] = cd.wy;
        cd.bbox[3] = cd.wy;
    }
    Ok(())
}

/// `t1char_encode_charpath`
fn t1char_encode_charpath(cd: &Chardesc, default_width: f64, nominal_width: f64) -> R<Vec<u8>> {
    let mut dst: Vec<u8> = Vec::new();
    let check = |dst: &Vec<u8>, n: usize| -> R<()> {
        if dst.len() + n >= CS_STR_LEN_MAX {
            Err("Buffer overflow.".into())
        } else {
            Ok(())
        }
    };
    // Advance Width
    if cd.wx != default_width {
        put_numbers(&[cd.wx - nominal_width], &mut dst)?;
    }
    let mut curr = 0usize;
    // Hint Declaration
    {
        let mut num_hstems = 0usize;
        let mut reset = true;
        let mut i = 0usize;
        while i < cd.stems.len() && cd.stems[i].dir == HSTEM {
            num_hstems += 1;
            let stem0 = if reset {
                cd.stems[i].pos
            } else {
                cd.stems[i].pos - (cd.stems[i - 1].pos + cd.stems[i - 1].del)
            };
            put_numbers(&[stem0, cd.stems[i].del], &mut dst)?;
            reset = false;
            if 2 * num_hstems > CS_ARG_STACK_MAX - 3 {
                check(&dst, 1)?;
                dst.push((if cd.flags & T1_CS_FLAG_USE_HINTMASK != 0 { CS_HSTEMHM } else { CS_HSTEM }) as u8);
                reset = true;
            }
            i += 1;
        }
        if !reset {
            check(&dst, 1)?;
            dst.push((if cd.flags & T1_CS_FLAG_USE_HINTMASK != 0 { CS_HSTEMHM } else { CS_HSTEM }) as u8);
        }
        reset = true;
        let mut num_vstems = 0usize;
        if cd.stems.len() > num_hstems {
            for i in num_hstems..cd.stems.len() {
                num_vstems += 1;
                let stem0 = if reset {
                    cd.stems[i].pos
                } else {
                    cd.stems[i].pos - (cd.stems[i - 1].pos + cd.stems[i - 1].del)
                };
                put_numbers(&[stem0, cd.stems[i].del], &mut dst)?;
                reset = false;
                if 2 * num_vstems > CS_ARG_STACK_MAX - 3 {
                    check(&dst, 1)?;
                    dst.push((if cd.flags & T1_CS_FLAG_USE_HINTMASK != 0 { CS_VSTEMHM } else { CS_VSTEM }) as u8);
                    reset = true;
                }
            }
            if !reset {
                check(&dst, 1)?;
                if cd.flags & (T1_CS_FLAG_USE_HINTMASK | T1_CS_FLAG_USE_CNTRMASK) != 0 {
                    // The vstem hint operator can be omitted if hstem and vstem hints are both
                    // declared at the beginning of a charstring, and is followed directly by the
                    // hintmask or cntrmask operators.
                    let followed = cd
                        .path
                        .get(curr)
                        .is_some_and(|p| p.ty == CS_HINT_DECL || p.ty == CS_CNTRMASK);
                    if !followed {
                        dst.push(CS_VSTEMHM as u8);
                    }
                } else {
                    dst.push(CS_VSTEM as u8);
                }
            }
        }
    }
    // Path Construction and Hint Replacement
    while curr < cd.path.len() && cd.path[curr].ty != CS_ENDCHAR {
        let p = &cd.path[curr];
        match p.ty {
            CS_HINT_DECL => {
                let mut hintmask = vec![0u8; cd.num_stems_bytes()];
                while curr < cd.path.len() && cd.path[curr].ty == CS_HINT_DECL {
                    let stem_idx = cd.get_stem(cd.path[curr].args[0] as i32)?;
                    hintmask[stem_idx / 8] |= 1 << (7 - (stem_idx % 8));
                    curr += 1;
                }
                if cd.flags & T1_CS_FLAG_USE_HINTMASK != 0 {
                    check(&dst, cd.num_stems_bytes() + 1)?;
                    dst.push(CS_HINTMASK as u8);
                    dst.extend_from_slice(&hintmask);
                }
            }
            CS_CNTRMASK => {
                let mut cntrmask = vec![0u8; cd.num_stems_bytes()];
                for &a in &p.args {
                    let stem_idx = cd.get_stem(a as i32)?;
                    cntrmask[stem_idx / 8] |= 1 << (7 - (stem_idx % 8));
                }
                check(&dst, cd.num_stems_bytes() + 1)?;
                dst.push(CS_CNTRMASK as u8);
                dst.extend_from_slice(&cntrmask);
                curr += 1;
            }
            CS_RMOVETO | CS_HMOVETO | CS_VMOVETO | CS_RLINETO | CS_HLINETO | CS_VLINETO
            | CS_RRCURVETO | CS_HVCURVETO | CS_VHCURVETO | CS_RLINECURVE | CS_RCURVELINE => {
                put_numbers(&p.args, &mut dst)?;
                check(&dst, 1)?;
                dst.push(p.ty as u8);
                curr += 1;
            }
            CS_FLEX | CS_HFLEX | CS_HFLEX1 => {
                put_numbers(&p.args, &mut dst)?;
                check(&dst, 2)?;
                dst.push(CS_ESCAPE as u8);
                dst.push(p.ty as u8);
                curr += 1;
            }
            ty => return Err(format!("Unknown Type 2 charstring command: {ty}")),
        }
    }
    // (adx ady bchar achar) endchar
    if cd.flags & T1_CS_FLAG_USE_SEAC != 0 {
        put_numbers(
            &[cd.seac_adx, cd.seac_ady, f64::from(cd.seac_bchar), f64::from(cd.seac_achar)],
            &mut dst,
        )?;
        check(&dst, 2)?;
    }
    check(&dst, 1)?;
    dst.push(CS_ENDCHAR as u8);
    Ok(dst)
}

impl Chardesc {
    fn num_stems_bytes(&self) -> usize {
        (self.stems.len() + 7) / 8
    }

    fn ginfo(&self) -> Ginfo {
        Ginfo {
            wx: self.wx,
            bbox: self.bbox,
            use_seac: self.flags & T1_CS_FLAG_USE_SEAC != 0,
            seac_bchar: self.seac_bchar,
            seac_achar: self.seac_achar,
        }
    }
}

/// `t1char_get_metrics`
fn t1char_get_metrics(src: &[u8], subrs: Option<&[Vec<u8>]>) -> R<Ginfo> {
    let mut cd = Chardesc::default();
    let mut it = Interp::new(subrs);
    it.reset();
    it.cs_top = 0;
    let mut pos = 0usize;
    it.build_charpath(&mut cd, src, &mut pos)?;
    do_postproc(&mut cd)?;
    Ok(cd.ginfo())
}

/// `t1char_convert_charstring`
fn t1char_convert_charstring(
    src: &[u8],
    subrs: Option<&[Vec<u8>]>,
    default_width: f64,
    nominal_width: f64,
) -> R<(Vec<u8>, Ginfo)> {
    let mut cd = Chardesc::default();
    let mut it = Interp::new(subrs);
    it.reset();
    it.cs_top = 0;
    let mut pos = 0usize;
    it.build_charpath(&mut cd, src, &mut pos)?;
    do_postproc(&mut cd)?;
    cd.stems.sort_by(|a, b| {
        a.dir
            .cmp(&b.dir)
            .then(a.pos.partial_cmp(&b.pos).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.del.partial_cmp(&b.del).unwrap_or(std::cmp::Ordering::Equal))
    });
    let dst = t1char_encode_charpath(&cd, default_width, nominal_width)?;
    Ok((dst, cd.ginfo()))
}

// ---------------------------------------------------------------------------------------------
// type1.c: pdf_font_load_type1
// ---------------------------------------------------------------------------------------------

const FONT_FLAG_FIXEDPITCH: i32 = 1 << 0;
const FONT_FLAG_SERIF: i32 = 1 << 1;
const FONT_FLAG_SYMBOLIC: i32 = 1 << 2;
const FONT_FLAG_ITALIC: i32 = 1 << 6;
const FONT_FLAG_SMALLCAP: i32 = 1 << 17;
const FONT_FLAG_FORCEBOLD: i32 = 1 << 18;

/// What `pdf_font_load_type1` writes for one Type 1 font.
pub struct Type1C {
    /// `TAG+FontName` (`/BaseFont` and `/FontName`)
    pub base_font: String,
    /// The `/FontFile3` stream (`/Subtype /Type1C`)
    pub data: Vec<u8>,
    /// The `/CharSet` string: `/name` for each glyph of the subset (`.notdef` excluded)
    pub charset: String,
    pub cap_height: f64,
    pub ascent: f64,
    pub descent: f64,
    pub italic_angle: f64,
    pub stem_v: f64,
    pub flags: i32,
    pub font_bbox: [f64; 4],
    pub first_char: u8,
    pub last_char: u8,
    /// `/Widths`: `last_char - first_char + 1` entries, 0 for unused codes
    pub widths: Vec<f64>,
    /// The used codes that survived (`usedchars` after glyphs missing from the program were
    /// dropped): what `pdf_encoding_add_usedchars` adds to a map encoding's set
    pub used: [bool; 256],
    /// The encoding vector the font was loaded with (`enc_vec`)
    pub encoding: Vec<Option<String>>,
}

/// numbers.h `ROUND`
fn round_to(v: f64, acc: f64) -> f64 {
    (v / acc + 0.5).floor() * acc
}

/// The end of the eexec part: a PFB's trailer (zeros and `cleartomark`) is not part of it.
fn binary_end(program: &[u8], start: usize) -> usize {
    let mut end = program.len();
    let strip = |end: &mut usize, f: &dyn Fn(u8) -> bool| {
        while *end > start && f(program[*end - 1]) {
            *end -= 1;
        }
    };
    let ws = |c: u8| matches!(c, b'\r' | b'\n' | b' ' | b'\t');
    strip(&mut end, &ws);
    if program[start..end].ends_with(b"{restore}if") {
        end -= b"{restore}if".len();
        strip(&mut end, &ws);
    }
    if program[start..end].ends_with(b"cleartomark") {
        end -= b"cleartomark".len();
        strip(&mut end, &|c| ws(c) || c == b'0');
        end
    } else {
        program.len()
    }
}

/// `pdf_font_load_type1`: the Type 1 program `program` (`length1` bytes of cleartext, then
/// the eexec part) as a Type 1C font holding the glyphs of the used codes.
///
/// `encoding` is the map entry's encoding vector (`encoding_id >= 0`); without it the
/// program's own encoding is used. `used` is the bitset of the used codes and `tag` the six
/// letter subset tag.
pub fn type1_to_type1c(
    program: &[u8],
    length1: usize,
    encoding: Option<&[String]>,
    used: &[u64; 4],
    tag: &str,
) -> R<Type1C> {
    if length1 == 0 || length1 >= program.len() {
        return Err("Type 1 program without an eexec part".into());
    }
    let clear = &program[..length1];
    if !(clear.starts_with(b"%!PS-AdobeFont") || clear.starts_with(b"%!FontType1") || clear.starts_with(b"%!PS"))
    {
        return Err("Not a PFB font file?".into());
    }
    let binary = &program[length1..binary_end(program, length1)];
    // (dvi.c: "type1 fonts with pfa format are not supported"; the ASCII hex of a PFA's eexec
    // part cannot be told apart from a PFB's binary part by anything but its first bytes)
    if binary.len() >= 4 && binary[..4].iter().all(u8::is_ascii_hexdigit) {
        return Err(
            "Sorry, pfa format not supported; please convert the font to pfb, e.g., with t1binary.".into(),
        );
    }
    let fontname = t1_get_fontname(clear)?;

    let mut builtin: Option<EncVec> = if encoding.is_none() { Some(vec![None; 256]) } else { None };
    let mut font = t1_load_font(clear, binary, &mut builtin)?;
    if font.cstrings.is_empty() {
        return Err("Type 1 font without CharStrings".into());
    }
    let enc_vec: EncVec = match encoding {
        Some(names) => (0..256)
            .map(|c| names.get(c).filter(|n| !n.is_empty()).cloned())
            .collect(),
        None => builtin.unwrap_or_else(|| vec![None; 256]),
    };
    let fullname = format!("{tag}+{fontname}");

    // get_font_attr: defaultWidthX, CapHeight, etc.
    let mut flags = 0i32;
    let (mut capheight, mut ascent, mut descent);
    if font.topdict.known("FontBBox") {
        capheight = font.topdict.get("FontBBox", 3)?;
        ascent = capheight;
        descent = font.topdict.get("FontBBox", 1)?;
    } else {
        capheight = 680.0;
        ascent = 690.0;
        descent = -190.0;
    }
    let stemv = if font.private.known("StdVW") { font.private.get("StdVW", 0)? } else { 88.0 };
    let italic_angle = if font.topdict.known("ItalicAngle") {
        let a = font.topdict.get("ItalicAngle", 0)?;
        if a != 0.0 {
            flags |= FONT_FLAG_ITALIC;
        }
        a
    } else {
        0.0
    };
    let mut defaultwidth = 500.0;
    let nominalwidth = 0.0;
    let metrics = |font: &T1Font, gid: usize| -> R<Ginfo> {
        t1char_get_metrics(&font.cstrings[gid], font.subrs.as_deref())
    };
    // (the glyph lookup answers 0, the .notdef, for a missing glyph: the first name always wins)
    let gid = font.glyph_lookup(Some("space"));
    if gid < font.cstrings.len() {
        defaultwidth = metrics(&font, gid)?.wx;
    }
    for name in ["H", "P", "Pi", "Rho"] {
        let gid = font.glyph_lookup(Some(name));
        if gid < font.cstrings.len() {
            capheight = metrics(&font, gid)?.bbox[3];
            break;
        }
    }
    for name in ["p", "q", "mu", "eta"] {
        let gid = font.glyph_lookup(Some(name));
        if gid < font.cstrings.len() {
            descent = metrics(&font, gid)?.bbox[1];
            break;
        }
    }
    for name in ["b", "h", "lambda"] {
        let gid = font.glyph_lookup(Some(name));
        if gid < font.cstrings.len() {
            ascent = metrics(&font, gid)?.bbox[3];
            break;
        }
    }
    if defaultwidth != 0.0 {
        font.private.add("defaultWidthX", 1)?;
        font.private.set("defaultWidthX", 0, defaultwidth)?;
    }
    if font.private.known("ForceBold") && font.private.get("ForceBold", 0)? != 0.0 {
        flags |= FONT_FLAG_FORCEBOLD;
    }
    if font.private.known("IsFixedPitch") && font.private.get("IsFixedPitch", 0)? != 0.0 {
        flags |= FONT_FLAG_FIXEDPITCH;
    }
    if !fontname.contains("Sans") {
        flags |= FONT_FLAG_SERIF;
    }
    if fontname.contains("Caps") {
        flags |= FONT_FLAG_SMALLCAP;
    }
    flags |= FONT_FLAG_SYMBOLIC; // FIXME

    let defaultwidth = if font.private.known("defaultWidthX") {
        font.private.get("defaultWidthX", 0)?
    } else {
        0.0
    };

    // Create CFF encoding, charset, sort glyphs
    let mut usedchars = [false; 256];
    for (code, u) in usedchars.iter_mut().enumerate() {
        *u = used[code / 64] >> (code % 64) & 1 != 0;
    }
    let mut new_strings: Vec<Vec<u8>> = Vec::new();
    let mut gid_map: Vec<usize> = vec![font.glyph_lookup(Some(".notdef"))];
    let mut ranges: Vec<(u8, u8)> = Vec::new();
    let mut supps: Vec<(u8, u16)> = Vec::new();
    let mut charset: Vec<u16> = Vec::new();
    let mut charset_text = String::new();
    {
        let mut prev: i32 = -2;
        for code in 0..256usize {
            let glyph = enc_vec[code].as_deref();
            if !usedchars[code] {
                continue;
            }
            if glyph == Some(".notdef") {
                // Character mapped to .notdef used in font
                usedchars[code] = false;
                continue;
            }
            let gid = font.glyph_lookup(glyph);
            if gid < 1 || gid >= font.cstrings.len() {
                // Glyph missing in font
                usedchars[code] = false;
                continue;
            }
            let glyph = glyph.expect("a found glyph has a name");
            let duplicate = (0..code).find(|&d| usedchars[d] && enc_vec[d].as_deref() == Some(glyph));
            let sid = add_string_unique(&mut new_strings, glyph.as_bytes());
            if let Some(d) = duplicate {
                supps.push((d as u8, sid));
            } else {
                gid_map.push(gid);
                charset.push(sid);
                if code as i32 != prev + 1 {
                    ranges.push((code as u8, 0));
                } else {
                    ranges.last_mut().expect("a range is open").1 += 1;
                }
                prev = code as i32;
                charset_text.push('/');
                charset_text.push_str(glyph);
            }
        }
    }

    // The charstrings; the Type 1 seac operator may add the glyphs of its accent and base.
    let mut cstrings: Vec<Vec<u8>> = Vec::new();
    let mut widths: Vec<f64> = Vec::new();
    {
        let mut gid = 0usize;
        while gid < gid_map.len() {
            let src = &font.cstrings[gid_map[gid]];
            let (dst, gm) =
                t1char_convert_charstring(src, font.subrs.as_deref(), defaultwidth, nominalwidth)?;
            cstrings.push(dst);
            if gm.use_seac {
                // seac.achar and seac.bchar must be contained in the CFF standard string;
                // those characters need not be encoded.
                let achar_name = STANDARD_ENCODING[gm.seac_achar as usize];
                let bchar_name = STANDARD_ENCODING[gm.seac_bchar as usize];
                let achar_gid = font.glyph_lookup(Some(achar_name));
                let bchar_gid = font.glyph_lookup(Some(bchar_name));
                for (name, g) in [(achar_name, achar_gid), (bchar_name, bchar_gid)] {
                    if !gid_map.contains(&g) {
                        gid_map.push(g);
                        // `cff_get_seac_sid`: -1 (stored as an `s_SID`) when not a standard string
                        charset.push(
                            STD_STRINGS.iter().position(|x| *x == name).map_or(0xffff, |i| i as u16),
                        );
                        charset_text.push('/');
                        charset_text.push_str(name);
                    }
                }
            }
            widths.push(gm.wx);
            gid += 1;
        }
    }
    let num_glyphs = gid_map.len();

    // Now we can update the String Index
    {
        let old = std::mem::take(&mut font.strings);
        let mut remap = |sid: u16| -> R<u16> {
            let s: Vec<u8> = if (sid as usize) < CFF_STDSTR_MAX {
                STD_STRINGS[sid as usize].as_bytes().to_vec()
            } else {
                old.get(sid as usize - CFF_STDSTR_MAX).cloned().ok_or("Invalid SID")?
            };
            Ok(add_string_unique(&mut new_strings, &s))
        };
        font.topdict.remap_sids(&mut remap)?;
        font.private.remap_sids(&mut remap)?;
    }
    font.strings = new_strings;
    let new_charset = charset;

    // add_metrics
    if !font.topdict.known("FontBBox") {
        return Err("No FontBBox?".into());
    }
    let scaling = if font.topdict.known("FontMatrix") { 1000.0 * font.topdict.get("FontMatrix", 0)? } else { 1.0 };
    let mut font_bbox = [0.0; 4];
    for (i, v) in font_bbox.iter_mut().enumerate() {
        *v = round_to(font.topdict.get("FontBBox", i)?, 1.0);
    }
    let (first_char, last_char, widths_out);
    if num_glyphs <= 1 {
        // This must be an error.
        first_char = 0u8;
        last_char = 0u8;
        widths_out = vec![0.0];
    } else {
        let lookup = |glyph: Option<&str>| -> usize {
            glyph_lookup(
                &new_charset,
                |sid, g| {
                    let sid = sid as usize;
                    if sid < CFF_STDSTR_MAX {
                        STD_STRINGS[sid].as_bytes() == g
                    } else {
                        font.strings.get(sid - CFF_STDSTR_MAX).is_some_and(|s| s == g)
                    }
                },
                glyph,
            )
        };
        let mut first = 255usize;
        let mut last = 0usize;
        let mut norm = [0.0f64; 256];
        for code in 0..256usize {
            if usedchars[code] {
                first = first.min(code);
                last = last.max(code);
                norm[code] = scaling * widths.get(lookup(enc_vec[code].as_deref())).copied().unwrap_or(0.0);
            }
        }
        if first > last {
            return Err("No glyphs actually used???".into());
        }
        first_char = first as u8;
        last_char = last as u8;
        // (pdf_check_tfm_widths only warns)
        widths_out = (first..=last)
            .map(|code| if usedchars[code] { round_to(norm[code], 0.1) } else { 0.0 })
            .collect();
    }

    // write_fontfile
    if !font.topdict.known("CharStrings") {
        font.topdict.add("CharStrings", 1)?;
    }
    if !font.topdict.known("charset") {
        font.topdict.add("charset", 1)?;
    }
    if !font.topdict.known("Encoding") {
        font.topdict.add("Encoding", 1)?;
    }
    let private_size = font.private.pack().len();
    // Private dict is required (but may have size 0)
    if !font.topdict.known("Private") {
        font.topdict.add("Private", 2)?;
    }
    let topdict_len = font.topdict.pack().len();

    if fullname.len() > 127 {
        return Err("FontName string length too large...".into());
    }
    let mut out: Vec<u8> = vec![1, 0, 4, 4]; // header
    pack_index(&[fullname.clone().into_bytes()], &mut out)?; // Name
    let topdict_pos = out.len();
    out.resize(out.len() + index_size(1, topdict_len), 0); // Top DICT
    pack_index(&font.strings, &mut out)?; // Strings
    out.extend_from_slice(&[0, 0]); // Global Subrs
    // Encoding
    font.topdict.set("Encoding", 0, out.len() as f64)?;
    let format: u8 = if supps.is_empty() { 1 } else { 1 | 0x80 };
    out.push(format);
    out.push(ranges.len() as u8);
    for &(first, n_left) in &ranges {
        out.push(first);
        out.push(n_left);
    }
    if format & 0x80 != 0 {
        out.push(supps.len() as u8);
        for &(code, sid) in &supps {
            out.push(code);
            out.extend_from_slice(&sid.to_be_bytes());
        }
    }
    // charset
    font.topdict.set("charset", 0, out.len() as f64)?;
    out.push(0);
    for &sid in &new_charset {
        out.extend_from_slice(&sid.to_be_bytes());
    }
    // CharStrings
    font.topdict.set("CharStrings", 0, out.len() as f64)?;
    pack_index(&cstrings, &mut out)?;
    // Private
    if private_size > 0 {
        let packed = font.private.pack();
        font.topdict.set("Private", 1, out.len() as f64)?;
        font.topdict.set("Private", 0, packed.len() as f64)?;
        out.extend_from_slice(&packed);
    }
    // Finally Top DICT
    let td = font.topdict.pack();
    if td.len() != topdict_len {
        return Err("Top DICT size changed".into());
    }
    let mut td_index = Vec::new();
    pack_index(&[td], &mut td_index)?;
    out[topdict_pos..topdict_pos + td_index.len()].copy_from_slice(&td_index);

    Ok(Type1C {
        base_font: fullname,
        data: out,
        charset: charset_text,
        cap_height: capheight,
        ascent,
        descent,
        italic_angle,
        stem_v: stemv,
        flags,
        font_bbox,
        first_char,
        last_char,
        widths: widths_out,
        used: usedchars,
        encoding: enc_vec,
    })
}

// ---------------------------------------------------------------------------------------------
// pdfencoding.c: the /Encoding and ToUnicode of a map encoding
// ---------------------------------------------------------------------------------------------

/// `pdf_create_ToUnicode_CMap(enc_name, enc_vec, is_used)`: a CMap of the used codes' glyph
/// names, or none when it is empty or a used glyph has no Unicode mapping at all
/// ("Glyphs with no Unicode mapping found. Removing ToUnicode CMap.").
pub fn to_unicode_cmap(glyphs: &[Option<String>], used: &[bool; 256], cmap_name: &str) -> Option<String> {
    let mut bits = [0u64; 4];
    let mut names = vec![String::new(); 256];
    for code in 0..256usize {
        if !used[code] {
            continue;
        }
        if let Some(name) = glyphs.get(code).and_then(|g| g.as_deref()) {
            crate::pdf_fonts::glyph_to_unicode(name)?;
            names[code] = name.to_owned();
            bits[code / 64] |= 1 << (code % 64);
        }
    }
    crate::dpx_font::type1_to_unicode_cmap(&names, &bits, cmap_name)
}

/// The `/Encoding` entry of a map encoding (`create_encoding_resource`).
#[derive(Debug, PartialEq, Eq)]
pub enum EncodingResource {
    /// No difference to report and no base encoding: no `/Encoding` key
    None,
    /// A predefined encoding, written as a name
    Name(&'static str),
    /// `<< /BaseEncoding /WinAnsiEncoding /Differences [..] >>`, an indirect object; the text
    /// is the dictionary
    Dict(String),
}

/// `is_similar_charset`: at least 64 codes empty or the same as in `WinAnsiEncoding`
fn is_similar_to_winansi(glyphs: &[Option<String>]) -> bool {
    let mut same = 0;
    for code in 0..256 {
        let differs = glyphs[code].as_deref().is_some_and(|g| g != crate::pdf_encodings::WIN_ANSI[code]);
        if !differs {
            same += 1;
            if same >= 64 {
                return true;
            }
        }
    }
    false
}

/// `pdf_encoding_complete` for one non-predefined encoding: `glyphs` is the encoding vector
/// (`.notdef` and empty slots as `None`) and `is_used` the codes every font of the encoding uses.
pub fn encoding_resource(
    glyphs: &[Option<String>],
    is_used: &[bool; 256],
    escape_name: &dyn Fn(&str) -> String,
) -> EncodingResource {
    let glyphs: Vec<Option<String>> = (0..256)
        .map(|c| glyphs.get(c).cloned().flatten().filter(|g| g != ".notdef"))
        .collect();
    let base = is_similar_to_winansi(&glyphs).then_some("WinAnsiEncoding");
    // make_encoding_differences
    let mut differences: Vec<String> = Vec::new();
    let mut count = 0;
    let mut skipping = true;
    for code in 0..256usize {
        let Some(glyph) = glyphs[code].as_deref().filter(|_| is_used[code]) else {
            skipping = true;
            continue;
        };
        if base.is_none() || crate::pdf_encodings::WIN_ANSI[code] != glyph {
            if skipping {
                differences.push(code.to_string());
            }
            differences.push(format!("/{}", escape_name(glyph)));
            skipping = false;
            count += 1;
        } else {
            skipping = true;
        }
    }
    if count == 0 {
        return match base {
            Some(name) => EncodingResource::Name(name),
            None => EncodingResource::None,
        };
    }
    let mut dict = String::from("<< ");
    if let Some(base) = base {
        dict.push_str(&format!("/BaseEncoding /{base} "));
    }
    dict.push_str(&format!("/Differences [{}] >>", differences.join(" ")));
    EncodingResource::Dict(dict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Read;
    use std::path::{Path, PathBuf};

    // Expectations come from real xdvipdfmx 20260113 output: the `/Subtype/Type1C` streams of the
    // TeX Live reference PDFs in /tmp/ratex-issues/tl-cache/*-fc2/work/main.pdf. The tests skip
    // silently when those assets (or the Type 1 fonts of the TeX Live tree) are absent.

    const TL_CACHE: &str = "/tmp/ratex-issues/tl-cache";
    const T1_ROOT: &str = "/usr/share/texmf-dist/fonts/type1";

    fn find(h: &[u8], n: &[u8]) -> Option<usize> {
        h.windows(n.len()).position(|w| w == n)
    }

    /// All Flate-decoded `/Subtype/Type1C` streams of a pdf written by xdvipdfmx.
    fn type1c_streams(pdf: &[u8]) -> Vec<Vec<u8>> {
        let key = b"/Subtype/Type1C";
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(i) = find(&pdf[from..], key).map(|i| i + from) {
            from = i + key.len();
            let rest = &pdf[from..];
            let Some(lpos) = find(&rest[..rest.len().min(200)], b"/Length ").map(|p| p + 8) else {
                continue;
            };
            let digits: String =
                rest[lpos..].iter().take_while(|b| b.is_ascii_digit()).map(|&b| b as char).collect();
            let Ok(len) = digits.parse::<usize>() else { continue };
            let Some(spos) = find(rest, b"stream").map(|p| p + 6) else { continue };
            let mut s = spos;
            if rest[s] == b'\r' {
                s += 1;
            }
            if rest[s] == b'\n' {
                s += 1;
            }
            let mut dec = flate2::read::ZlibDecoder::new(&rest[s..s + len]);
            let mut buf = Vec::new();
            if dec.read_to_end(&mut buf).is_ok() {
                out.push(buf);
            }
        }
        out
    }

    fn tl_pdfs() -> Vec<(String, PathBuf)> {
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(TL_CACHE) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                let p = e.path().join("work/main.pdf");
                if n.ends_with("-fc2") && p.exists() {
                    v.push((n, p));
                }
            }
        }
        v.sort();
        v
    }

    /// FontName → pfb path, for every Type 1 program of the TeX Live tree.
    fn pfb_index() -> HashMap<String, PathBuf> {
        fn walk(dir: &Path, map: &mut HashMap<String, PathBuf>) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, map);
                } else if p.extension().is_some_and(|x| x == "pfb") {
                    let Ok(b) = std::fs::read(&p) else { continue };
                    let head = &b[..b.len().min(8192)];
                    if let Some(i) = find(head, b"/FontName") {
                        let rest = &head[i + 9..];
                        let rest = &rest[rest.iter().position(|&c| c == b'/').unwrap_or(0) + 1..];
                        let name: String = rest
                            .iter()
                            .take_while(|c| !c.is_ascii_whitespace() && **c != b'/')
                            .map(|&c| c as char)
                            .collect();
                        map.entry(name).or_insert(p);
                    }
                }
            }
        }
        let mut map = HashMap::new();
        walk(Path::new(T1_ROOT), &mut map);
        map
    }

    struct Reference {
        name: String,
        /// code → glyph name of the reference's Encoding
        enc: Vec<Option<String>>,
        used: [u64; 4],
    }

    /// The used codes and the encoding of an xdvipdfmx Type 1C font.
    fn read_reference(cff: &[u8]) -> Reference {
        let be16 = |p: usize| usize::from(cff[p]) << 8 | usize::from(cff[p + 1]);
        let index = |p: usize| -> (Vec<(usize, usize)>, usize) {
            let count = be16(p);
            if count == 0 {
                return (Vec::new(), p + 2);
            }
            let osz = usize::from(cff[p + 2]);
            let off = |i: usize| (0..osz).fold(0usize, |a, k| a << 8 | usize::from(cff[p + 3 + i * osz + k]));
            let base = p + 3 + (count + 1) * osz - 1;
            let items = (0..count).map(|i| (base + off(i), base + off(i + 1))).collect();
            (items, base + off(count))
        };
        let (names, p) = index(cff[2] as usize);
        let name = String::from_utf8_lossy(&cff[names[0].0..names[0].1]).into_owned();
        let (tops, p) = index(p);
        let (strs, _) = index(p);
        // Top DICT: the Encoding and charset offsets
        let td = &cff[tops[0].0..tops[0].1];
        let mut stack: Vec<f64> = Vec::new();
        let (mut enc_off, mut cs_off) = (0usize, 0usize);
        let mut i = 0;
        while i < td.len() {
            let b = td[i];
            match b {
                0..=21 => {
                    let mut op = usize::from(b);
                    if b == 12 {
                        i += 1;
                        op = 22 + usize::from(td[i]);
                    }
                    if op == 16 {
                        enc_off = stack[0] as usize;
                    } else if op == 15 {
                        cs_off = stack[0] as usize;
                    }
                    stack.clear();
                    i += 1;
                }
                28 => {
                    stack.push(f64::from(i16::from_be_bytes([td[i + 1], td[i + 2]])));
                    i += 3;
                }
                29 => {
                    stack.push(f64::from(i32::from_be_bytes([td[i + 1], td[i + 2], td[i + 3], td[i + 4]])));
                    i += 5;
                }
                30 => {
                    i += 1;
                    while td[i] & 0x0f != 0x0f && td[i] >> 4 != 0x0f {
                        i += 1;
                    }
                    i += 1;
                    stack.push(0.0);
                }
                32..=246 => {
                    stack.push(f64::from(b) - 139.0);
                    i += 1;
                }
                247..=250 => {
                    stack.push((f64::from(b) - 247.0) * 256.0 + f64::from(td[i + 1]) + 108.0);
                    i += 2;
                }
                251..=254 => {
                    stack.push(-(f64::from(b) - 251.0) * 256.0 - f64::from(td[i + 1]) - 108.0);
                    i += 2;
                }
                _ => panic!("bad Top DICT"),
            }
        }
        let sid_name = |sid: usize| -> String {
            if sid < CFF_STDSTR_MAX {
                STD_STRINGS[sid].to_owned()
            } else {
                let (a, b) = strs[sid - CFF_STDSTR_MAX];
                String::from_utf8_lossy(&cff[a..b]).into_owned()
            }
        };
        assert_eq!(cff[cs_off], 0, "charset format");
        let (cs, _) = index(
            // CharStrings offset is not needed beyond the glyph count
            {
                let mut p = 0;
                let mut st: Vec<f64> = Vec::new();
                let mut j = 0;
                while j < td.len() {
                    let b = td[j];
                    match b {
                        0..=21 => {
                            let mut op = usize::from(b);
                            if b == 12 {
                                j += 1;
                                op = 22 + usize::from(td[j]);
                            }
                            if op == 17 {
                                p = st[0] as usize;
                            }
                            st.clear();
                            j += 1;
                        }
                        28 => {
                            st.push(f64::from(i16::from_be_bytes([td[j + 1], td[j + 2]])));
                            j += 3;
                        }
                        29 => {
                            st.push(f64::from(i32::from_be_bytes([td[j + 1], td[j + 2], td[j + 3], td[j + 4]])));
                            j += 5;
                        }
                        30 => {
                            j += 1;
                            while td[j] & 0x0f != 0x0f && td[j] >> 4 != 0x0f {
                                j += 1;
                            }
                            j += 1;
                            st.push(0.0);
                        }
                        32..=246 => {
                            st.push(f64::from(b) - 139.0);
                            j += 1;
                        }
                        247..=250 => {
                            st.push((f64::from(b) - 247.0) * 256.0 + f64::from(td[j + 1]) + 108.0);
                            j += 2;
                        }
                        _ => {
                            st.push(-(f64::from(b) - 251.0) * 256.0 - f64::from(td[j + 1]) - 108.0);
                            j += 2;
                        }
                    }
                }
                p
            },
        );
        let nglyphs = cs.len();
        let gname = |gid: usize| sid_name(be16(cs_off + 1 + 2 * (gid - 1)));
        // Encoding: format 1 ranges (gid = running number), then supplements
        let format = cff[enc_off];
        assert_eq!(format & 0x7f, 1, "encoding format");
        let nr = usize::from(cff[enc_off + 1]);
        let mut enc: Vec<Option<String>> = vec![None; 256];
        let mut used = [0u64; 4];
        let mut gid = 1usize;
        for r in 0..nr {
            let first = usize::from(cff[enc_off + 2 + 2 * r]);
            let n_left = usize::from(cff[enc_off + 3 + 2 * r]);
            for k in 0..=n_left {
                assert!(gid < nglyphs);
                enc[first + k] = Some(gname(gid));
                used[(first + k) / 64] |= 1 << ((first + k) % 64);
                gid += 1;
            }
        }
        if format & 0x80 != 0 {
            let p = enc_off + 2 + 2 * nr;
            let ns = usize::from(cff[p]);
            for s in 0..ns {
                let code = usize::from(cff[p + 1 + 3 * s]);
                let sid = be16(p + 2 + 3 * s);
                // the C code records the *first* code of a duplicate glyph
                assert_eq!(enc[code].as_deref(), Some(sid_name(sid).as_str()));
            }
        }
        Reference { name, enc, used }
    }

    /// Convert every Type 1C font of every TL reference PDF and compare with the stream.
    fn compare_with_tl(only: Option<&str>) -> (Vec<String>, Vec<String>) {
        let mut same = Vec::new();
        let mut different = Vec::new();
        let pfbs = pfb_index();
        for (doc, pdf) in tl_pdfs() {
            if only.is_some_and(|o| !doc.starts_with(o)) {
                continue;
            }
            let bytes = std::fs::read(&pdf).unwrap();
            for cff in type1c_streams(&bytes) {
                let r = read_reference(&cff);
                let (tag, fontname) = r.name.split_once('+').unwrap();
                let Some(path) = pfbs.get(fontname) else { continue };
                let prog = crate::pdf_fonts::parse_type1(&std::fs::read(path).unwrap());
                let got_builtin = type1_to_type1c(&prog.data, prog.length1, None, &r.used, tag);
                let encoding: Vec<String> = r.enc.iter().map(|n| n.clone().unwrap_or_default()).collect();
                let got_enc = type1_to_type1c(&prog.data, prog.length1, Some(&encoding), &r.used, tag);
                let label = format!("{doc}:{}", r.name);
                let ok = |g: &R<Type1C>| g.as_ref().is_ok_and(|t| t.data == cff);
                if ok(&got_builtin) || ok(&got_enc) {
                    same.push(label);
                } else {
                    let g = got_builtin.as_ref().or(got_enc.as_ref());
                    different.push(match g {
                        Ok(t) => format!(
                            "{label}: {} vs {} bytes, first difference at {:?}",
                            t.data.len(),
                            cff.len(),
                            t.data.iter().zip(&cff).position(|(a, b)| a != b)
                        ),
                        Err(e) => format!("{label}: {e}"),
                    });
                }
            }
        }
        (same, different)
    }

    #[test]
    fn type1_fonts_match_xdvipdfmx() {
        let (same, different) = compare_with_tl(None);
        assert!(different.is_empty(), "{} fonts differ:\n{}", different.len(), different.join("\n"));
        if !tl_pdfs().is_empty() && Path::new(T1_ROOT).exists() {
            assert!(!same.is_empty(), "no reference font compared");
        }
    }
    #[test]
    fn map_encoding_resource_follows_pdfencoding_c() {
        let mut glyphs: Vec<Option<String>> = crate::pdf_encodings::WIN_ANSI
            .iter()
            .map(|g| (*g != ".notdef").then(|| (*g).to_owned()))
            .collect();
        glyphs[28] = Some("fi".into());
        let mut used = [false; 256];
        used[65] = true;
        let esc = |g: &str| g.to_owned();
        // nothing differs from the predefined encoding: its name
        assert_eq!(encoding_resource(&glyphs, &used, &esc), EncodingResource::Name("WinAnsiEncoding"));
        used[28] = true;
        assert_eq!(
            encoding_resource(&glyphs, &used, &esc),
            EncodingResource::Dict("<< /BaseEncoding /WinAnsiEncoding /Differences [28 /fi] >>".into())
        );
        // a charset unlike WinAnsi: every used code is listed
        let sparse: Vec<Option<String>> = (0..256).map(|c| (c < 10).then(|| format!("g{c}"))).collect();
        let mut used = [false; 256];
        used[3] = true;
        used[4] = true;
        used[7] = true;
        // (empty slots count as similar to WinAnsiEncoding: it is the base)
        assert_eq!(
            encoding_resource(&sparse, &used, &esc),
            EncodingResource::Dict("<< /BaseEncoding /WinAnsiEncoding /Differences [3 /g3 /g4 7 /g7] >>".into())
        );
    }

    #[test]
    fn pfa_programs_are_refused_like_xdvipdfmx() {
        let pfbs = pfb_index();
        let Some(path) = pfbs.get("CMR10") else { return };
        let prog = crate::pdf_fonts::parse_type1(&std::fs::read(path).unwrap());
        let hex: String = prog.data[prog.length1..prog.length1 + prog.length2]
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect();
        let mut pfa = prog.data[..prog.length1].to_vec();
        pfa.extend_from_slice(hex.as_bytes());
        let err = type1_to_type1c(&pfa, prog.length1, None, &[u64::MAX; 4], "ABCDEF").err();
        assert!(err.is_some_and(|e| e.contains("pfa format not supported")));
    }
}
