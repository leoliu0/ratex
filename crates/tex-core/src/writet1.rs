//! pdfTeX's Type 1 font file writer (writet1.c `writet1`): the program is
//! read line by line the way `t1_getline` does (tabs become spaces, CR ends
//! a line, runs of spaces collapse, empty lines vanish), then either copied
//! whole (`t1_include`) or subset (`t1_subset_ascii_part` ..
//! `t1_subset_end`): the cleartext /Encoding is rewritten for the used
//! glyphs, /UniqueID is dropped, unused CharStrings are removed and unused
//! Subrs are replaced by a bare `return`. The four eexec lead bytes become
//! zeros and everything after `mark currentfile closefile` is omitted, so
//! /Length3 is 0. A map entry's SlantFont/ExtendFont rewrite /FontMatrix,
//! /ItalicAngle and the font name (`t1_modify_fm`, `t1_modify_italic`).

use crate::pdf_fonts::{Type1Keys, FONTBBOX1_CODE, ITALIC_ANGLE_CODE};
use std::collections::BTreeSet;
use std::ops::Bound;

/// A written FontFile stream.
pub(crate) struct WrittenType1 {
    pub data: Vec<u8>,
    pub length1: usize,
    pub length2: usize,
    /// writefont.c `fd->gl_tree` after marking: the requested glyphs plus
    /// `seac` components (the /CharSet).
    pub charset: BTreeSet<String>,
    /// `fd->font_dim` and `fd->fontname` as the pass left them: the keys
    /// the program declares, `/ItalicAngle` and the name adjusted for the
    /// map entry's slant and extension.
    pub keys: Type1Keys,
}

/// A map entry's `SlantFont` and `ExtendFont` in thousandths (`fm_slant`,
/// `fm_extend`; 1000 is stored as 0).
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct T1Transform {
    pub slant: i32,
    pub extend: i32,
}

/// Partial download (`is_subsetted`).
pub(crate) struct T1Subset<'a> {
    /// `fd->gl_tree` before marking.
    pub glyphs: &'a BTreeSet<String>,
    /// The six-letter subset tag.
    pub tag: &'a str,
    /// `fd->all_glyphs`: keep every CharString and Subr (an included PDF
    /// font without a /CharSet), but still rewrite the cleartext.
    pub all_glyphs: bool,
}

const EEXEC: &[u8] = b"currentfile eexec";
const CHARSTRINGS: &[u8] = b"/CharStrings";
const CS_TOKEN_PAIRS: [(&[u8], &[u8]); 4] = [
    (b" RD", b"NP"),
    (b" -|", b"|"),
    (b" RD", b"noaccess put"),
    (b" -|", b"noaccess put"),
];

const CS_CALLSUBR: usize = 10;
const CS_RETURN: usize = 11;
const CS_ESCAPE: usize = 12;
const CS_1BYTE_MAX: usize = 32;
const CS_SEAC: usize = CS_1BYTE_MAX + 6;
const CS_DIV: usize = CS_1BYTE_MAX + 12;
const CS_CALLOTHERSUBR: usize = CS_1BYTE_MAX + 16;
const CS_POP: usize = CS_1BYTE_MAX + 17;
const CS_MAX: usize = CS_1BYTE_MAX + 34;
const CC_STACK_SIZE: usize = 24;

/// writet1.c `cc_init`: (nargs, bottom, clear) of the valid commands.
fn cc_entry(command: usize) -> Option<(usize, bool, bool)> {
    Some(match command {
        1 | 3 | 5 | 13 | 21 => (2, true, true), // hstem vstem rlineto hsbw rmoveto
        4 | 6 | 7 | 22 => (1, true, true),      // vmoveto hlineto vlineto hmoveto
        8 => (6, true, true),                   // rrcurveto
        9 => (0, false, true),                  // closepath
        10 | 11 => (if command == 10 { 1 } else { 0 }, false, false),
        14 => (0, false, true), // endchar
        30 | 31 => (4, true, true),
        32 => (0, false, true),       // dotsection
        33 | 34 => (6, true, true),   // vstem3 hstem3
        CS_SEAC => (5, true, true),
        39 => (4, true, true), // sbw
        CS_DIV => (2, false, false),
        CS_CALLOTHERSUBR | CS_POP => (0, false, false),
        65 => (2, true, true), // setcurrentpoint
        _ => return None,
    })
}

fn standard_glyph_name(code: i32) -> &'static str {
    crate::pdffile::standard_encoding_name(code).unwrap_or(".notdef")
}

#[derive(Clone, Default)]
struct CsEntry {
    name: String,
    /// `" RD " + charstring + rest of the line + "\n"`
    data: Vec<u8>,
    cslen: usize,
    used: bool,
    valid: bool,
}

fn hexval(c: Option<u8>) -> i32 {
    match c {
        Some(c @ b'A'..=b'F') => i32::from(c - b'A' + 10),
        Some(c @ b'a'..=b'f') => i32::from(c - b'a' + 10),
        Some(c @ b'0'..=b'9') => i32::from(c - b'0'),
        _ => -1,
    }
}

fn cencrypt(plain: u8, cr: &mut u16) -> u8 {
    let cipher = plain ^ (*cr >> 8) as u8;
    *cr = (u16::from(cipher).wrapping_add(*cr))
        .wrapping_mul(52_845)
        .wrapping_add(22_719);
    cipher
}

fn cdecrypt(cipher: u8, cr: &mut u16) -> u8 {
    let plain = cipher ^ (*cr >> 8) as u8;
    *cr = (u16::from(cipher).wrapping_add(*cr))
        .wrapping_mul(52_845)
        .wrapping_add(22_719);
    plain
}

/// str_suffix: `s` ends the line, ignoring one trailing newline.
fn has_suffix(line: &[u8], s: &[u8]) -> bool {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.ends_with(s)
}

fn contains(line: &[u8], s: &[u8]) -> bool {
    line.windows(s.len()).any(|w| w == s)
}

fn find(line: &[u8], s: &[u8]) -> Option<usize> {
    line.windows(s.len()).position(|w| w == s)
}

/// writet1.c `t1_scan_num` (`skip(p, ' ')` then `sscanf("%g")`): the C
/// `float` and the offset where pdfTeX resumes, past the run of digits,
/// `.`, `e`, `E`, `+` and `-`.
fn scan_num_resume(s: &[u8]) -> Option<(f32, usize)> {
    let start = s.iter().take_while(|&&b| b == b' ').count();
    let p = &s[start..];
    let lead = p.iter().take_while(|b| b.is_ascii_whitespace()).count();
    let q = &p[lead..];
    let mut n = usize::from(matches!(q.first(), Some(b'+' | b'-')));
    let digits = q[n..].iter().take_while(|b| b.is_ascii_digit()).count();
    n += digits;
    let mut frac = 0;
    if q.get(n) == Some(&b'.') {
        frac = q[n + 1..].iter().take_while(|b| b.is_ascii_digit()).count();
        n += 1 + frac;
    }
    if digits + frac == 0 {
        return None;
    }
    if matches!(q.get(n), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(q.get(n + 1), Some(b'+' | b'-')));
        let exp = q[n + 1 + sign..].iter().take_while(|b| b.is_ascii_digit()).count();
        if exp > 0 {
            n += 1 + sign + exp;
        }
    }
    let value: f32 = std::str::from_utf8(&q[..n]).ok()?.parse().ok()?;
    let resume = p
        .iter()
        .take_while(|&&b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
        .count();
    Some((value, start + resume))
}

fn scan_num(s: &[u8]) -> Option<f64> {
    scan_num_resume(s).map(|(value, _)| f64::from(value))
}

/// C `printf("%g")`: six significant digits, trailing zeros removed.
fn format_g(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() { "-0" } else { "0" }.to_owned();
    }
    let scientific = format!("{value:.5e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let trim = |text: &str| {
        if text.contains('.') {
            text.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            text.to_owned()
        }
    };
    if !(-4..6).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trim(mantissa), exponent.abs())
    } else {
        trim(&format!("{value:.*}", (5 - exponent) as usize))
    }
}

/// A tiny `sscanf`: literal words, `%i` (strtol base 0) and `%s` items;
/// whitespace in the format matches any run of input whitespace. Returns
/// the converted items, stopping at the first mismatch like C does.
enum Scan<'a> {
    Lit(&'a [u8]),
    Int,
    Word,
}

enum Item<'a> {
    Int(i64),
    Word(&'a [u8]),
}

fn sscanf<'a>(input: &'a [u8], format: &[Scan<'_>]) -> Vec<Item<'a>> {
    let mut items = Vec::new();
    let mut at = 0;
    let skip_ws = |at: &mut usize| {
        while input.get(*at).is_some_and(u8::is_ascii_whitespace) {
            *at += 1;
        }
    };
    for (index, part) in format.iter().enumerate() {
        match part {
            Scan::Lit(word) => {
                if index > 0 {
                    skip_ws(&mut at);
                }
                if !input[at..].starts_with(word) {
                    break;
                }
                at += word.len();
            }
            Scan::Int => {
                skip_ws(&mut at);
                let start = at;
                let negative = input.get(at) == Some(&b'-');
                if matches!(input.get(at), Some(b'+' | b'-')) {
                    at += 1;
                }
                let (radix, prefix) = if input.get(at) == Some(&b'0')
                    && matches!(input.get(at + 1), Some(b'x' | b'X'))
                    && input.get(at + 2).is_some_and(u8::is_ascii_hexdigit)
                {
                    (16, 2)
                } else if input.get(at) == Some(&b'0') {
                    (8, 0)
                } else {
                    (10, 0)
                };
                at += prefix;
                let digits = input[at..]
                    .iter()
                    .take_while(|&&b| (b as char).is_digit(radix))
                    .count();
                if digits == 0 {
                    at = start;
                    break;
                }
                let text = std::str::from_utf8(&input[at..at + digits]).unwrap_or("0");
                let value = i64::from_str_radix(text, radix).unwrap_or(i64::MAX);
                at += digits;
                items.push(Item::Int(if negative { -value } else { value }));
            }
            Scan::Word => {
                skip_ws(&mut at);
                let len = input[at..]
                    .iter()
                    .take(255)
                    .take_while(|b| !b.is_ascii_whitespace())
                    .count();
                if len == 0 {
                    break;
                }
                items.push(Item::Word(&input[at..at + len]));
                at += len;
            }
        }
    }
    items
}

fn valid_code(code: i64) -> Option<usize> {
    (0..256).contains(&code).then_some(code as usize)
}

struct Writer<'a> {
    input: &'a [u8],
    pos: usize,
    eof: bool,
    /// End of the cleartext and of the eexec part in `input`.
    clear_end: usize,
    /// Hex-encoded eexec part (writet1.c `t1_pfa`).
    pfa: bool,
    last_hexbyte: i32,
    in_eexec: u8,
    dr: u16,
    er: u16,
    cs: bool,
    scan: bool,
    synthetic: bool,
    cslen: usize,
    cs_start: usize,
    len_iv: usize,
    line: Vec<u8>,
    out: Vec<u8>,
    encrypt: bool,
    length1: usize,
    length2: usize,
    /// The keys written so far (`fd->font_dim`, `fd->fontname`).
    keys: Type1Keys,
    transform: T1Transform,
    // CharString state
    subr_tab: Vec<CsEntry>,
    subr_array_start: Vec<u8>,
    subr_array_end: Vec<u8>,
    subr_max: i64,
    cs_tab: Vec<CsEntry>,
    cs_size: usize,
    cs_dict_start: Vec<u8>,
    cs_dict_end: Vec<u8>,
    cs_size_pos: usize,
    cs_token_pair: Option<(&'static [u8], &'static [u8])>,
    stack: Vec<i32>,
    last_arg_other_subr3: i32,
    depth: usize,
}

impl<'a> Writer<'a> {
    fn new(input: &'a [u8], length1: usize, transform: T1Transform) -> Self {
        let clear_end = length1.min(input.len());
        let eexec_part = &input[clear_end..];
        let lead = eexec_part.iter().take_while(|b| b.is_ascii_whitespace()).count();
        // A PFA's eexec part is hex text; a PFB's (or binary PFA's) is not.
        let probe = &eexec_part[lead..eexec_part.len().min(lead + 64)];
        let pfa = probe.len() >= 4
            && probe.iter().all(|b| b.is_ascii_hexdigit() || b.is_ascii_whitespace());
        Writer {
            input,
            pos: 0,
            eof: false,
            clear_end,
            pfa,
            last_hexbyte: 0,
            in_eexec: 0,
            dr: 55_665,
            er: 55_665,
            cs: false,
            scan: true,
            synthetic: false,
            cslen: 0,
            cs_start: 0,
            len_iv: 4,
            line: Vec::with_capacity(256),
            out: Vec::new(),
            encrypt: false,
            length1: 0,
            length2: 0,
            keys: Type1Keys::default(),
            transform,
            subr_tab: Vec::new(),
            subr_array_start: Vec::new(),
            subr_array_end: Vec::new(),
            subr_max: -1,
            cs_tab: Vec::new(),
            cs_size: 0,
            cs_dict_start: Vec::new(),
            cs_dict_end: Vec::new(),
            cs_size_pos: 0,
            cs_token_pair: None,
            stack: Vec::with_capacity(CC_STACK_SIZE),
            last_arg_other_subr3: 3,
            depth: 0,
        }
    }

    fn getbyte(&mut self) -> Option<u8> {
        let byte = self.input.get(self.pos).copied();
        match byte {
            Some(_) => self.pos += 1,
            None => self.eof = true,
        }
        byte
    }

    fn edecrypt(&mut self, cipher: Option<u8>) -> u8 {
        let cipher = if self.pfa {
            let mut c = cipher;
            while matches!(c, Some(10 | 13)) {
                c = self.getbyte();
            }
            let high = hexval(c);
            let low = hexval(self.getbyte());
            self.last_hexbyte = (high << 4) + low;
            self.last_hexbyte as u8
        } else {
            cipher.unwrap_or(0xff)
        };
        let plain = cipher ^ (self.dr >> 8) as u8;
        self.dr = (u16::from(cipher).wrapping_add(self.dr))
            .wrapping_mul(52_845)
            .wrapping_add(22_719);
        plain
    }

    fn eencrypt(&mut self, plain: u8) -> u8 {
        let cipher = plain ^ (self.er >> 8) as u8;
        self.er = (u16::from(cipher).wrapping_add(self.er))
            .wrapping_mul(52_845)
            .wrapping_add(22_719);
        cipher
    }

    fn prefix(&self, s: &[u8]) -> bool {
        self.line.starts_with(s)
    }

    fn charstrings(&self) -> bool {
        contains(&self.line, CHARSTRINGS)
    }

    fn subrs(&self) -> bool {
        self.prefix(b"/Subrs")
    }

    fn end_eexec(&self) -> bool {
        has_suffix(&self.line, b"mark currentfile closefile")
    }

    /// writet1.c `t1_getline`.
    fn getline(&mut self) -> Option<()> {
        loop {
            if self.eof {
                return None; // "unexpected end of file"
            }
            self.line.clear();
            self.cslen = 0;
            let mut eexec_scan: i32 = 0;
            let Some(mut c) = self.getbyte() else {
                return Some(());
            };
            loop {
                let mut b = if self.in_eexec == 1 { self.edecrypt(Some(c)) } else { c };
                // append_char_to_buf
                if b == 9 {
                    b = 32;
                }
                if b == 13 {
                    b = 10;
                }
                if b != b' ' || self.line.last().is_some_and(|&last| last != b' ') {
                    self.line.push(b);
                }
                if self.in_eexec == 0 && (0..EEXEC.len() as i32).contains(&eexec_scan) {
                    if self.line.get(eexec_scan as usize) == Some(&EEXEC[eexec_scan as usize]) {
                        eexec_scan += 1;
                    } else {
                        eexec_scan = -1;
                    }
                }
                if b == 10 || (self.pfa && eexec_scan == EEXEC.len() as i32 && b == 32) {
                    break;
                }
                if self.cs
                    && self.cslen == 0
                    && self.line.len() > 4
                    && (has_suffix(&self.line, b" RD ") || has_suffix(&self.line, b" -| "))
                {
                    let mut p = self.line.len() - 5;
                    while self.line[p] != b' ' {
                        p = p.checked_sub(1)?;
                    }
                    let len = scan_num(&self.line[p + 1..])?;
                    if !(0.0..=65_535.0).contains(&len) {
                        return None;
                    }
                    self.cslen = len as usize;
                    self.cs_start = self.line.len();
                    for _ in 0..self.cslen {
                        let byte = self.getbyte();
                        let plain = self.edecrypt(byte);
                        self.line.push(plain);
                    }
                }
                match self.getbyte() {
                    Some(next) => c = next,
                    None => break,
                }
            }
            // append_eol
            let len = self.line.len();
            if len > 1 && self.line[len - 1] != 10 {
                self.line.push(10);
            }
            let len = self.line.len();
            if len > 2 && self.line[len - 2] == 32 {
                self.line[len - 2] = 10;
                self.line.pop();
            }
            if self.line.len() < 2 {
                continue;
            }
            if eexec_scan == EEXEC.len() as i32 {
                self.in_eexec = 1;
            }
            return Some(());
        }
    }

    fn putline(&mut self) {
        if self.line.len() <= 1 {
            return;
        }
        if self.encrypt {
            for i in 0..self.line.len() {
                let cipher = self.eencrypt(self.line[i]);
                self.out.push(cipher);
            }
        } else {
            self.out.extend_from_slice(&self.line);
        }
    }

    fn puts(&mut self, s: &[u8]) {
        self.line.clear();
        self.line.extend_from_slice(s);
        self.putline();
    }

    /// `eol`: end the line with a newline unless it is a single byte.
    fn eol(&mut self) {
        if self.line.len() > 1 && self.line.last() != Some(&10) {
            self.line.push(10);
        }
    }

    /// writet1.c `t1_scan_param`: `/lenIV`, then `t1_scan_keys`.
    fn scan_param(&mut self, tag: Option<&str>) -> Option<()> {
        if !self.scan || self.line.first() != Some(&b'/') {
            return Some(());
        }
        if self.prefix(b"/lenIV") {
            let (value, _) = scan_num_resume(&self.line[b"/lenIV".len()..])?;
            if (value as i32) < 0 {
                return None; // "negative value of lenIV is not supported"
            }
            self.len_iv = value as usize;
            return Some(());
        }
        self.scan_keys(tag)
    }

    /// writet1.c `t1_scan_keys`: records the descriptor keys the program
    /// declares and, for a subset (`tag`), writes the tagged `/FontName`.
    fn scan_keys(&mut self, tag: Option<&str>) -> Option<()> {
        if self.transform != T1Transform::default() {
            if self.prefix(b"/FontMatrix") {
                return self.modify_font_matrix();
            }
            if self.prefix(b"/ItalicAngle") {
                return self.modify_italic();
            }
        }
        if self.prefix(b"/FontType") {
            let (value, _) = scan_num_resume(&self.line[b"/FontType".len()..])?;
            return (value as i32 == 1).then_some(()); // "Type%d fonts unsupported by pdfTeX"
        }
        // font_key order: the first matching key wins
        const KEYS: [(&[u8], usize); 7] = [
            (b"Ascender", crate::pdf_fonts::ASCENT_CODE),
            (b"CapHeight", crate::pdf_fonts::CAPHEIGHT_CODE),
            (b"Descender", crate::pdf_fonts::DESCENT_CODE),
            (b"ItalicAngle", ITALIC_ANGLE_CODE),
            (b"StdVW", crate::pdf_fonts::STEMV_CODE),
            (b"XHeight", crate::pdf_fonts::XHEIGHT_CODE),
            (b"FontBBox", FONTBBOX1_CODE),
        ];
        let key = KEYS.iter().find(|(name, _)| self.line[1..].starts_with(name));
        let is_name = self.line[1..].starts_with(b"FontName");
        if key.is_none() && !is_name {
            return Some(());
        }
        let key_len = key.map_or(b"FontName".len(), |(name, _)| name.len());
        let mut p = 1 + key_len;
        while self.line.get(p) == Some(&b' ') {
            p += 1;
        }
        let Some(&(_, k)) = key else {
            if self.line.get(p) != Some(&b'/') {
                return None; // "a name expected"
            }
            let r = p + 1;
            let end = self.line[r..].iter().position(|&b| b == b' ' || b == 10).map_or(self.line.len(), |at| r + at);
            let mut name = String::from_utf8_lossy(&self.line[r..end]).into_owned();
            if self.transform.slant != 0 {
                name.push_str(&format!("-Slant_{}", self.transform.slant));
            }
            if self.transform.extend != 0 {
                name.push_str(&format!("-Extend_{}", self.transform.extend));
            }
            if let Some(tag) = tag {
                let mut rewritten = self.line[..r].to_vec();
                rewritten.extend_from_slice(tag.as_bytes());
                rewritten.push(b'+');
                rewritten.extend_from_slice(name.as_bytes());
                rewritten.extend_from_slice(&self.line[end..]);
                self.line = rewritten;
                self.eol();
            }
            self.keys.font_name = Some(name);
            return Some(());
        };
        if matches!(k, crate::pdf_fonts::STEMV_CODE | FONTBBOX1_CODE)
            && matches!(self.line.get(p), Some(b'[' | b'{'))
        {
            p += 1;
        }
        if k == FONTBBOX1_CODE {
            for slot in 0..4 {
                let (value, resume) = scan_num_resume(&self.line[p..])?;
                self.keys.dims[k + slot] = Some(value as i32);
                p += resume;
            }
        } else {
            self.keys.dims[k] = Some(scan_num_resume(&self.line[p..])?.0 as i32);
        }
        Some(())
    }

    /// writet1.c `t1_modify_fm`: the slant transform is applied before the
    /// extension to the six `/FontMatrix` numbers.
    fn modify_font_matrix(&mut self) -> Option<()> {
        let open = self
            .line
            .iter()
            .position(|&b| b == b'[')
            .or_else(|| self.line.iter().position(|&b| b == b'{'))?; // "FontMatrix: an array expected"
        let close = if self.line[open] == b'[' { b']' } else { b'}' };
        let mut p = open + 1;
        let mut a = [0f32; 6];
        for slot in &mut a {
            let (value, resume) = scan_num_resume(&self.line[p..])?;
            *slot = value;
            p += resume;
        }
        if self.transform.slant != 0 {
            let slant = f64::from(self.transform.slant) * 1e-3;
            for i in [0, 2, 4] {
                a[i] = (f64::from(a[i]) + f64::from(a[i + 1]) * slant) as f32;
            }
        }
        if self.transform.extend != 0 {
            let extend = f64::from(self.transform.extend) * 1e-3;
            for i in [0, 2, 4] {
                a[i] = (f64::from(a[i]) * extend) as f32;
            }
        }
        let tail = self.line[p..].iter().position(|&b| b == close)?; // "cannot find the corresponding character"
        let mut rewritten = self.line[..=open].to_vec();
        for value in a {
            rewritten.extend_from_slice(format_g(f64::from(value)).as_bytes());
            rewritten.push(b' ');
        }
        rewritten.extend_from_slice(&self.line[p + tail..]);
        self.line = rewritten;
        self.eol();
        Some(())
    }

    /// writet1.c `t1_modify_italic`: only a slant changes the angle; an
    /// extension alone leaves the line (and the descriptor preset) alone.
    fn modify_italic(&mut self) -> Option<()> {
        if self.transform.slant == 0 {
            return Some(());
        }
        let space = self.line.iter().position(|&b| b == b' ')?;
        let (value, resume) = scan_num_resume(&self.line[space + 1..])?;
        let angle = (f64::from(value)
            - (f64::from(self.transform.slant) * 1e-3).atan() * (180.0 / std::f64::consts::PI))
            as f32;
        let mut rewritten = self.line[..=space].to_vec();
        rewritten.extend_from_slice(format_g(f64::from(angle)).as_bytes());
        rewritten.extend_from_slice(&self.line[space + 1 + resume..]);
        self.line = rewritten;
        self.eol();
        self.keys.dims[ITALIC_ANGLE_CODE] = Some(f64::from(angle).round() as i32);
        Some(())
    }

    /// The reading half of `writet1` without output: the cleartext up to
    /// `eexec` (taking the program's own `/Encoding`) and the private
    /// dictionary up to `/Subrs` or `/CharStrings`.
    fn scan_program(&mut self, encoding: &mut Option<Vec<String>>) -> Option<()> {
        loop {
            self.getline()?;
            if encoding.is_none() && self.prefix(b"/Encoding") {
                let (names, _) = self.builtin_enc()?;
                *encoding = Some(names.into_iter().map(Option::unwrap_or_default).collect());
            } else {
                self.scan_param(None)?;
            }
            if self.in_eexec != 0 {
                break;
            }
        }
        self.start_eexec();
        loop {
            self.getline()?;
            if self.charstrings() || self.subrs() {
                return Some(());
            }
            self.scan_param(None)?;
        }
    }

    /// writet1.c `t1_start_eexec`.
    fn start_eexec(&mut self) {
        self.length1 = self.out.len();
        if !self.pfa && self.pos < self.clear_end {
            // t1_check_block_len: the cleartext block's final newline
            self.getbyte();
        }
        for _ in 0..4 {
            let byte = self.getbyte();
            self.edecrypt(byte);
        }
        self.line.clear();
        self.line.extend_from_slice(&[0; 4]);
        self.encrypt = true;
        self.putline();
    }

    /// writet1.c `t1_stop_eexec`; /Length3 stays 0.
    fn stop_eexec(&mut self) -> Option<()> {
        self.length2 = self.out.len() - self.length1;
        self.encrypt = false;
        if self.pfa {
            let byte = self.getbyte();
            let c = self.edecrypt(byte);
            if !(c == 10 || c == 13) {
                if self.last_hexbyte == 0 {
                    self.puts(b"00");
                } else {
                    return None; // "unexpected data after eexec"
                }
            }
        }
        self.cs = false;
        self.in_eexec = 2;
        Some(())
    }

    /// writet1.c `t1_include`: the whole program.
    fn include(&mut self) -> Option<()> {
        loop {
            self.getline()?;
            self.scan_param(None)?;
            self.putline();
            if self.in_eexec != 0 {
                break;
            }
        }
        self.start_eexec();
        loop {
            self.getline()?;
            self.scan_param(None)?;
            self.putline();
            if self.charstrings() || self.subrs() {
                break;
            }
        }
        self.cs = true;
        loop {
            self.getline()?;
            self.putline();
            if self.end_eexec() {
                break;
            }
        }
        self.stop_eexec()
    }

    /// writet1.c `t1_builtin_enc`: the program's encoding vector; true when
    /// it is `StandardEncoding`.
    fn builtin_enc(&mut self) -> Option<(Vec<Option<String>>, bool)> {
        let mut names: Vec<Option<String>> = vec![None; 256];
        if has_suffix(&self.line, b"def") {
            let rest = &self.line[b"/Encoding".len()..];
            let word = sscanf(rest, &[Scan::Word]);
            if let Some(Item::Word(b"StandardEncoding")) = word.first() {
                for (code, slot) in names.iter_mut().enumerate() {
                    let name = standard_glyph_name(code as i32);
                    if name != ".notdef" {
                        *slot = Some(name.to_owned());
                    }
                }
                return Some((names, true));
            }
            return None; // "cannot subset font (unknown predefined encoding)"
        }
        if self.prefix(b"/Encoding [") || self.prefix(b"/Encoding[") {
            let mut counter = 0usize;
            let mut r = find(&self.line, b"[")? + 1;
            if self.line.get(r) == Some(&b' ') {
                r += 1;
            }
            loop {
                while self.line.get(r) == Some(&b'/') {
                    r += 1;
                    let start = r;
                    while !matches!(self.line.get(r), Some(32 | 10 | b']' | b'/') | None) {
                        r += 1;
                    }
                    let name = String::from_utf8_lossy(&self.line[start..r]).into_owned();
                    if self.line.get(r) == Some(&b' ') {
                        r += 1;
                    }
                    if counter > 255 {
                        return None; // "encoding vector contains more than 256 names"
                    }
                    if name != ".notdef" {
                        names[counter] = Some(name);
                    }
                    counter += 1;
                }
                if !matches!(self.line.get(r), Some(10 | b'%')) {
                    let rest = &self.line[r..];
                    if rest.starts_with(b"] def") || rest.starts_with(b"] readonly def") {
                        break;
                    }
                    return None; // "a name or `] def' or `] readonly def' expected"
                }
                self.getline()?;
                r = 0;
            }
            return Some((names, false));
        }
        let mut p = find(&self.line, b"\n")?;
        loop {
            if self.line.get(p) == Some(&10) {
                self.getline()?;
                p = 0;
            }
            let rest = &self.line[p..];
            let put = sscanf(rest, &[Scan::Lit(b"dup"), Scan::Int, Scan::Word]);
            let copy = sscanf(
                rest,
                &[Scan::Lit(b"dup"), Scan::Lit(b"dup"), Scan::Int, Scan::Lit(b"exch"), Scan::Int],
            );
            let interval = sscanf(
                rest,
                &[
                    Scan::Lit(b"dup"),
                    Scan::Lit(b"dup"),
                    Scan::Int,
                    Scan::Int,
                    Scan::Lit(b"getinterval"),
                    Scan::Int,
                ],
            );
            if let [Item::Int(code), Item::Word(word)] = put[..] {
                if let (Some(b'/'), Some(code)) = (word.first(), valid_code(code)) {
                    if &word[1..] != b".notdef" {
                        names[code] = Some(String::from_utf8_lossy(&word[1..]).into_owned());
                    }
                    p += find(&self.line[p..], b" put")? + 4;
                    if self.line.get(p) == Some(&b' ') {
                        p += 1;
                    }
                    continue;
                }
            }
            if let [Item::Int(b), Item::Int(a)] = copy[..] {
                if let (Some(a), Some(b)) = (valid_code(a), valid_code(b)) {
                    names[b] = names[a].clone();
                    p += find(&self.line[p..], b" get put")? + 8;
                    if self.line.get(p) == Some(&b' ') {
                        p += 1;
                    }
                    continue;
                }
            }
            if let [Item::Int(a), Item::Int(c), Item::Int(b)] = interval[..] {
                if let (Some(a), Some(b), Some(c)) = (valid_code(a), valid_code(b), valid_code(c)) {
                    for i in 0..c {
                        names[b + i] = names.get(a + i).cloned().flatten();
                    }
                    p += find(&self.line[p..], b" putinterval")? + 12;
                    if self.line.get(p) == Some(&b' ') {
                        p += 1;
                    }
                    continue;
                }
            }
            if (p == 0 || self.line[p - 1] == b' ') && &self.line[p..] == b"def\n" {
                return Some((names, false));
            }
            while !matches!(self.line.get(p), Some(b' ' | 10) | None) {
                p += 1;
            }
            if self.line.get(p).is_none() {
                return None;
            }
            if self.line.get(p) == Some(&b' ') {
                p += 1;
            }
        }
    }

    /// writet1.c `t1_subset_ascii_part`.
    fn subset_ascii_part(&mut self, subset: &T1Subset<'_>) -> Option<()> {
        self.getline()?;
        while !self.prefix(b"/Encoding") {
            self.scan_param(Some(subset.tag))?;
            let len = self.line.len();
            let unique_id_def =
                self.prefix(b"/UniqueID") && len >= 4 && &self.line[len - 4..len - 1] == b"def";
            if !unique_id_def {
                self.putline();
            }
            self.getline()?;
        }
        let (names, standard) = self.builtin_enc()?;
        if standard {
            self.puts(b"/Encoding StandardEncoding def\n");
        } else {
            self.puts(b"/Encoding 256 array\n0 1 255 {1 index exch /.notdef put} for\n");
            let mut written = 0;
            for glyph in subset.glyphs {
                // the first code of a name (create_t1_glyph_tree)
                if let Some(code) = names.iter().position(|name| name.as_deref() == Some(glyph)) {
                    self.puts(format!("dup {code} /{glyph} put\n").as_bytes());
                    written += 1;
                }
            }
            if written == 0 {
                self.puts(b"dup 0 /.notdef put\n");
            }
            self.puts(b"readonly def\n");
        }
        loop {
            self.getline()?;
            self.scan_param(Some(subset.tag))?;
            if !self.prefix(b"/UniqueID") {
                self.putline();
            }
            if self.in_eexec != 0 {
                return Some(());
            }
        }
    }

    /// writet1.c `cs_store`.
    fn cs_store(&mut self, is_subr: bool) -> Option<()> {
        let space = find(&self.line, b" ")?;
        let mut data = self.line.get(self.cs_start.checked_sub(4)?..self.cs_start + self.cslen)?.to_vec();
        let tail = &self.line[self.cs_start + self.cslen..];
        data.extend_from_slice(&tail[..tail.iter().position(|&b| b == 10)?]);
        data.push(10);
        let entry = CsEntry {
            name: String::new(),
            data,
            cslen: self.cslen,
            used: false,
            valid: true,
        };
        if is_subr {
            let subr = scan_num(&self.line[space + 1..])?;
            if !(0.0..self.subr_tab.len() as f64).contains(&subr) {
                return None; // "Subrs array: entry index out of range"
            }
            if self.cs_token_pair.is_none() {
                let buf = &entry.data;
                self.cs_token_pair = CS_TOKEN_PAIRS
                    .iter()
                    .find(|(start, end)| buf.starts_with(start) && has_suffix(buf, end))
                    .copied();
            }
            let slot = &mut self.subr_tab[subr as usize];
            let used = slot.used;
            *slot = CsEntry { used, ..entry };
        } else {
            if self.cs_tab.len() >= self.cs_size {
                return None; // "CharStrings dict: more entries than dict size"
            }
            let name = String::from_utf8_lossy(self.line.get(1..space)?).into_owned();
            self.cs_tab.push(CsEntry { name, ..entry });
        }
        Some(())
    }

    /// writet1.c `t1_read_subrs`.
    fn read_subrs(&mut self, tag: &str) -> Option<()> {
        self.getline()?;
        while !(self.charstrings() || self.subrs()) {
            self.scan_param(Some(tag))?;
            if !self.prefix(b"/UniqueID") {
                self.putline();
            }
            self.getline()?;
        }
        loop {
            // found:
            self.cs = true;
            self.scan = false;
            if !self.subrs() {
                return Some(());
            }
            let subr_size = scan_num(&self.line[b"/Subrs ".len()..])?;
            if !(0.0..=65_535.0).contains(&subr_size) {
                return None;
            }
            let subr_size = subr_size as usize;
            if subr_size == 0 {
                while !self.charstrings() {
                    self.getline()?;
                }
                return Some(());
            }
            self.subr_tab = vec![CsEntry::default(); subr_size];
            self.subr_array_start = self.line.clone();
            self.getline()?;
            while self.cslen != 0 {
                self.cs_store(true)?;
                self.getline()?;
            }
            for entry in self.subr_tab.iter_mut().take(4) {
                entry.used = true;
            }
            const POST_SUBRS_SCAN: usize = 5;
            let mut end = Vec::new();
            let mut scanned = 0;
            while scanned < POST_SUBRS_SCAN {
                if self.charstrings() {
                    break;
                }
                end.extend_from_slice(&self.line);
                self.getline()?;
                scanned += 1;
            }
            self.subr_array_end = end;
            if scanned < POST_SUBRS_SCAN {
                return Some(());
            }
            // CharStrings not found: a synthetic font
            self.subr_tab.clear();
            self.subr_array_start.clear();
            self.subr_array_end.clear();
            self.cs_token_pair = None;
            self.cs = false;
            self.synthetic = true;
            while !(self.charstrings() || self.subrs()) {
                self.getline()?;
            }
        }
    }

    fn cc_get(&self, n: isize) -> Option<i32> {
        let index = if n < 0 { self.stack.len().checked_sub(n.unsigned_abs())? } else { n as usize };
        self.stack.get(index).copied()
    }

    fn cc_pop(&mut self, n: usize) -> Option<()> {
        let len = self.stack.len().checked_sub(n)?;
        self.stack.truncate(len);
        Some(())
    }

    fn cc_push(&mut self, value: i32) -> Option<()> {
        if self.stack.len() >= CC_STACK_SIZE {
            return None;
        }
        self.stack.push(value);
        Some(())
    }

    /// writet1.c `cs_mark` for a CharString (`name`) or a Subr. `gl_tree`
    /// receives `seac` components. Every pdfTeX failure here is fatal; the
    /// caller then keeps the program unchanged.
    fn cs_mark(&mut self, name: Option<&str>, subr: i32, gl_tree: &mut BTreeSet<String>) -> Option<()> {
        self.depth += 1;
        if self.depth > 64 {
            return None;
        }
        let result = self.cs_mark_inner(name, subr, gl_tree);
        self.depth -= 1;
        result
    }

    fn cs_mark_inner(
        &mut self,
        name: Option<&str>,
        subr: i32,
        gl_tree: &mut BTreeSet<String>,
    ) -> Option<()> {
        let (is_subr, index) = match name {
            None => {
                let index = usize::try_from(subr).ok().filter(|&i| i < self.subr_tab.len())?;
                if !self.subr_tab[index].valid {
                    return Some(());
                }
                (true, index)
            }
            Some(name) => match self.cs_tab.iter().position(|entry| entry.name == name) {
                Some(index) => (false, index),
                // "glyph `%s' undefined"
                None => return Some(()),
            },
        };
        let entry = if is_subr { &mut self.subr_tab[index] } else { &mut self.cs_tab[index] };
        if !entry.valid || (entry.used && !is_subr) {
            return Some(());
        }
        entry.used = true;
        let cslen = entry.cslen;
        let mut plain = Vec::with_capacity(cslen);
        let mut cr = 4_330_u16;
        for &byte in entry.data.get(4..4 + cslen)? {
            plain.push(cdecrypt(byte, &mut cr));
        }
        let mut at = self.len_iv.min(plain.len());
        let mut last_cmd = 0;
        while at < plain.len() {
            let b = usize::from(plain[at]);
            at += 1;
            if b >= 32 {
                let value = if b <= 246 {
                    b as i32 - 139
                } else if b <= 250 {
                    let next = i32::from(*plain.get(at)?);
                    at += 1;
                    ((b as i32 - 247) << 8) + 108 + next
                } else if b <= 254 {
                    let next = i32::from(*plain.get(at)?);
                    at += 1;
                    -((b as i32 - 251) << 8) - 108 - next
                } else {
                    let raw = plain.get(at..at + 4)?;
                    at += 4;
                    i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])
                };
                self.cc_push(value)?;
                continue;
            }
            let mut command = b;
            if command == CS_ESCAPE {
                command = usize::from(*plain.get(at)?) + CS_1BYTE_MAX;
                at += 1;
            }
            if command >= CS_MAX {
                return None; // "command value out of range"
            }
            let (nargs, bottom, clear) = cc_entry(command)?; // "command not valid"
            if bottom && self.stack.len() != nargs {
                return None; // "less/more arguments on stack than required"
            }
            last_cmd = command;
            match command {
                CS_CALLSUBR => {
                    let target = self.cc_get(-1)?;
                    self.cc_pop(1)?;
                    self.cs_mark(None, target, gl_tree)?;
                    if !self.subr_tab.get(usize::try_from(target).ok()?)?.valid {
                        return None; // "cannot call subr"
                    }
                }
                CS_DIV => {
                    self.cc_pop(2)?;
                    self.cc_push(0)?;
                }
                CS_CALLOTHERSUBR => {
                    if self.cc_get(-1)? == 3 {
                        self.last_arg_other_subr3 = self.cc_get(-3)?;
                    }
                    let count = self.cc_get(-2)?.checked_add(2)?;
                    self.cc_pop(usize::try_from(count).ok()?)?;
                }
                CS_POP => self.cc_push(self.last_arg_other_subr3)?,
                CS_SEAC => {
                    let base = self.cc_get(3)?;
                    let accent = self.cc_get(4)?;
                    self.stack.clear();
                    if !(0..256).contains(&base) || !(0..256).contains(&accent) {
                        return None;
                    }
                    let base = standard_glyph_name(base);
                    let accent = standard_glyph_name(accent);
                    self.cs_mark(Some(base), 0, gl_tree)?;
                    self.cs_mark(Some(accent), 0, gl_tree)?;
                    gl_tree.insert(base.to_owned());
                    gl_tree.insert(accent.to_owned());
                }
                _ => {
                    if clear {
                        self.stack.clear();
                    }
                }
            }
        }
        if is_subr && last_cmd != CS_RETURN {
            // append_cs_return: "last command in subr is not a RETURN"
            plain.push(CS_RETURN as u8);
            let entry = &mut self.subr_tab[index];
            let mut data = entry.data[..4].to_vec();
            let mut cr = 4_330_u16;
            data.extend(plain.iter().map(|&byte| cencrypt(byte, &mut cr)));
            data.extend_from_slice(&entry.data[4 + entry.cslen..]);
            entry.data = data;
            entry.cslen += 1;
        }
        Some(())
    }

    /// writet1.c `t1_mark_glyphs`.
    fn mark_glyphs(&mut self, gl_tree: &mut BTreeSet<String>, all_glyphs: bool) -> Option<()> {
        if self.synthetic || all_glyphs {
            for entry in self.cs_tab.iter_mut().chain(self.subr_tab.iter_mut()) {
                if entry.valid {
                    entry.used = true;
                }
            }
            self.subr_max = self.subr_tab.len() as i64 - 1;
            return Some(());
        }
        self.cs_mark(Some(".notdef"), 0, gl_tree)?;
        // libavl traversal: glyphs inserted after the cursor are visited too
        let mut cursor: Option<String> = None;
        loop {
            let next = match &cursor {
                None => gl_tree.iter().next(),
                Some(current) => gl_tree
                    .range::<String, _>((Bound::Excluded(current), Bound::Unbounded))
                    .next(),
            };
            let Some(glyph) = next.cloned() else {
                break;
            };
            self.cs_mark(Some(&glyph), 0, gl_tree)?;
            cursor = Some(glyph);
        }
        self.subr_max = self
            .subr_tab
            .iter()
            .rposition(|entry| entry.used)
            .map_or(-1, |index| index as i64);
        Some(())
    }

    /// writet1.c `t1_flush_cs`.
    fn flush_cs(&mut self, is_subr: bool) -> Option<()> {
        let (start_line, size_pos, count) = if is_subr {
            (
                std::mem::take(&mut self.subr_array_start),
                b"/Subrs ".len(),
                (self.subr_max + 1) as usize,
            )
        } else {
            let count = self.cs_tab.iter().filter(|entry| entry.used).count();
            (std::mem::take(&mut self.cs_dict_start), self.cs_size_pos, count)
        };
        let digits = start_line
            .get(size_pos..)?
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        self.line.clear();
        self.line.extend_from_slice(&start_line[..size_pos]);
        self.line.extend_from_slice(count.to_string().as_bytes());
        self.line.extend_from_slice(&start_line[size_pos + digits..]);
        self.eol();
        self.putline();

        let tab = if is_subr {
            let mut tab = std::mem::take(&mut self.subr_tab);
            tab.truncate(count);
            tab
        } else {
            std::mem::take(&mut self.cs_tab)
        };
        let mut return_cs = Vec::new();
        let pair = if is_subr { self.cs_token_pair? } else { (&b""[..], &b""[..]) };
        if is_subr {
            let mut cr = 4_330_u16;
            return_cs.extend((0..self.len_iv).map(|_| cencrypt(0, &mut cr)));
            return_cs.push(cencrypt(CS_RETURN as u8, &mut cr));
        }
        for (index, entry) in tab.iter().enumerate() {
            if entry.used {
                self.line.clear();
                if is_subr {
                    self.line.extend_from_slice(format!("dup {index} {}", entry.cslen).as_bytes());
                } else {
                    self.line.extend_from_slice(format!("/{} {}", entry.name, entry.cslen).as_bytes());
                }
                self.line.extend_from_slice(&entry.data);
                self.putline();
            } else if is_subr {
                self.line.clear();
                self.line.extend_from_slice(format!("dup {index} {}", return_cs.len()).as_bytes());
                self.line.extend_from_slice(pair.0);
                self.line.push(b' ');
                self.line.extend_from_slice(&return_cs);
                self.putline();
                self.line.clear();
                self.line.push(b' ');
                self.line.extend_from_slice(pair.1);
                self.eol();
                self.putline();
            }
        }
        self.line = if is_subr {
            std::mem::take(&mut self.subr_array_end)
        } else {
            std::mem::take(&mut self.cs_dict_end)
        };
        self.eol();
        self.putline();
        Some(())
    }

    /// writet1.c `t1_subset_charstrings`.
    fn subset_charstrings(&mut self, gl_tree: &mut BTreeSet<String>, all_glyphs: bool) -> Option<()> {
        // t1_check_unusual_charstring
        let at = find(&self.line, CHARSTRINGS)? + CHARSTRINGS.len();
        if !matches!(sscanf(&self.line[at..], &[Scan::Int])[..], [Item::Int(_)]) {
            let mut joined = self.line.clone();
            *joined.last_mut()? = b' ';
            self.getline()?;
            joined.extend_from_slice(&self.line);
            self.line = joined;
            self.eol();
        }
        self.cs_size_pos = find(&self.line, CHARSTRINGS)? + CHARSTRINGS.len() + 1;
        let cs_size = scan_num(self.line.get(self.cs_size_pos..)?)?;
        if !(0.0..=65_535.0).contains(&cs_size) {
            return None;
        }
        self.cs_size = cs_size as usize;
        self.cs_tab = Vec::with_capacity(self.cs_size);
        self.cs_dict_start = self.line.clone();
        self.getline()?;
        while self.cslen != 0 {
            self.cs_store(false)?;
            self.getline()?;
        }
        self.cs_dict_end = self.line.clone();
        self.mark_glyphs(gl_tree, all_glyphs)?;
        if !self.subr_array_start.is_empty() {
            // "mismatched subroutine begin/end token pairs" is fatal
            self.cs_token_pair?;
            self.flush_cs(true)?;
        }
        self.flush_cs(false)
    }

    /// writet1.c `t1_subset_end`.
    fn subset_end(&mut self) -> Option<()> {
        if self.synthetic {
            while !contains(&self.line, b"definefont") {
                self.getline()?;
                self.putline();
            }
            while !self.end_eexec() {
                self.getline()?;
            }
            self.putline();
        } else {
            while !self.end_eexec() {
                self.getline()?;
                self.putline();
            }
        }
        self.stop_eexec()
    }

    fn finish(self, charset: BTreeSet<String>) -> WrittenType1 {
        WrittenType1 {
            data: self.out,
            length1: self.length1,
            length2: self.length2,
            charset,
            keys: self.keys,
        }
    }
}

/// What `writet1` reads from a program before writing anything: the
/// descriptor keys it declares and its own encoding vector (glyph name per
/// code, empty for `.notdef`).
pub(crate) struct ScannedType1 {
    pub keys: Type1Keys,
    pub encoding: Option<Vec<String>>,
}

/// Scan the program `data` (cleartext of `length1` bytes) the way `writet1`
/// does. A program pdfTeX would refuse keeps what was read before the
/// failure.
pub(crate) fn scan_type1(data: &[u8], length1: usize) -> ScannedType1 {
    let mut writer = Writer::new(data, length1, T1Transform::default());
    let mut encoding = None;
    let _ = writer.scan_program(&mut encoding);
    ScannedType1 { keys: writer.keys, encoding }
}

/// writet1.c `writet1`: the FontFile stream pdfTeX writes for the program
/// `data` (cleartext of `length1` bytes, then `length2` eexec bytes),
/// whole (`subset` None) or as a subset, for a map entry with the given
/// slant and extension. None where pdfTeX would fail.
pub(crate) fn write_type1(
    data: &[u8],
    length1: usize,
    length2: usize,
    transform: T1Transform,
    subset: Option<&T1Subset<'_>>,
) -> Option<WrittenType1> {
    if length2 == 0 {
        return None;
    }
    let mut writer = Writer::new(data, length1, transform);
    writer.out.reserve(data.len());
    let Some(subset) = subset else {
        writer.include()?;
        return Some(writer.finish(BTreeSet::new()));
    };
    let mut gl_tree = subset.glyphs.clone();
    writer.subset_ascii_part(subset)?;
    writer.start_eexec();
    writer.read_subrs(subset.tag)?;
    writer.subset_charstrings(&mut gl_tree, subset.all_glyphs)?;
    writer.subset_end()?;
    Some(writer.finish(gl_tree))
}
