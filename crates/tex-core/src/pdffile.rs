//! Serialize a PdfDoc into PDF bytes: xref, catalog, page tree, content
//! streams (flate), Type 1 font embedding with encodings and widths, link
//! annotations, named destinations, and outlines.

use crate::pdf_fonts::{parse_metrics, parse_type1, Type1Program};
use crate::pdfout::{Annot, EmbedFont, PdfDoc};
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::collections::{BTreeSet, HashMap};
use std::io::Write;

pub(crate) fn flate(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::fast());
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

#[inline]
fn char_is_used(chars: &[u64; 4], character: u8) -> bool {
    chars[character as usize / 64] & (1_u64 << (character as usize % 64)) != 0
}

/// Attach observed character usage and trim the PDF widths/Unicode tables to
/// the smallest required code interval. The full encoding vector remains
/// available to the Type 1 subsetter for code-to-glyph-name resolution.
pub fn set_font_usage(font: &mut EmbedFont, used_chars: [u64; 4]) {
    let Some(first) = (0..=u8::MAX).find(|&c| char_is_used(&used_chars, c)) else {
        return;
    };
    let last = (0..=u8::MAX)
        .rev()
        .find(|&c| char_is_used(&used_chars, c))
        .unwrap_or(first);
    let old_first = font.first_char as usize;
    let start = (first as usize).saturating_sub(old_first);
    let end = (last as usize)
        .saturating_sub(old_first)
        .saturating_add(1)
        .min(font.widths.len());
    if start < end {
        font.widths = font.widths[start..end].to_vec();
        font.first_char = first;
        font.last_char = last;
    }
    font.to_unicode
        .retain(|(code, _)| char_is_used(&used_chars, *code));
    font.used_chars = used_chars;
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|part| part == needle)
}

fn eexec_decrypt(cipher: &[u8]) -> Vec<u8> {
    let mut state = 55_665_u16;
    cipher
        .iter()
        .map(|&byte| {
            let plain = byte ^ (state >> 8) as u8;
            state = (byte as u16)
                .wrapping_add(state)
                .wrapping_mul(52_845)
                .wrapping_add(22_719);
            plain
        })
        .collect()
}

fn eexec_encrypt(plain: &[u8]) -> Vec<u8> {
    let mut state = 55_665_u16;
    plain
        .iter()
        .map(|&byte| {
            let cipher = byte ^ (state >> 8) as u8;
            state = (cipher as u16)
                .wrapping_add(state)
                .wrapping_mul(52_845)
                .wrapping_add(22_719);
            cipher
        })
        .collect()
}

fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && bytes[at].is_ascii_whitespace() {
        at += 1;
    }
    at
}

fn token_end(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && !bytes[at].is_ascii_whitespace() {
        at += 1;
    }
    at
}

fn ps_int_after(bytes: &[u8], key: &[u8]) -> Option<i32> {
    let mut at = find_bytes(bytes, key)? + key.len();
    at = skip_space(bytes, at);
    let end = token_end(bytes, at);
    std::str::from_utf8(&bytes[at..end]).ok()?.parse().ok()
}

fn charstring_decrypt(cipher: &[u8]) -> Vec<u8> {
    let mut state = 4_330_u16;
    cipher
        .iter()
        .map(|&byte| {
            let plain = byte ^ (state >> 8) as u8;
            state = (byte as u16)
                .wrapping_add(state)
                .wrapping_mul(52_845)
                .wrapping_add(22_719);
            plain
        })
        .collect()
}

fn standard_encoding_name(code: i32) -> Option<&'static str> {
    const DIGITS: [&str; 10] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    const UPPER: [&str; 26] = [
        "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R",
        "S", "T", "U", "V", "W", "X", "Y", "Z",
    ];
    const LOWER: [&str; 26] = [
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r",
        "s", "t", "u", "v", "w", "x", "y", "z",
    ];
    match code {
        32 => Some("space"),
        33 => Some("exclam"),
        34 => Some("quotedbl"),
        35 => Some("numbersign"),
        36 => Some("dollar"),
        37 => Some("percent"),
        38 => Some("ampersand"),
        39 => Some("quoteright"),
        40 => Some("parenleft"),
        41 => Some("parenright"),
        42 => Some("asterisk"),
        43 => Some("plus"),
        44 => Some("comma"),
        45 => Some("hyphen"),
        46 => Some("period"),
        47 => Some("slash"),
        48..=57 => Some(DIGITS[(code - 48) as usize]),
        58 => Some("colon"),
        59 => Some("semicolon"),
        60 => Some("less"),
        61 => Some("equal"),
        62 => Some("greater"),
        63 => Some("question"),
        64 => Some("at"),
        65..=90 => Some(UPPER[(code - 65) as usize]),
        91 => Some("bracketleft"),
        92 => Some("backslash"),
        93 => Some("bracketright"),
        94 => Some("asciicircum"),
        95 => Some("underscore"),
        96 => Some("quoteleft"),
        97..=122 => Some(LOWER[(code - 97) as usize]),
        123 => Some("braceleft"),
        124 => Some("bar"),
        125 => Some("braceright"),
        126 => Some("asciitilde"),
        161 => Some("exclamdown"),
        162 => Some("cent"),
        163 => Some("sterling"),
        164 => Some("fraction"),
        165 => Some("yen"),
        166 => Some("florin"),
        167 => Some("section"),
        168 => Some("currency"),
        169 => Some("quotesingle"),
        170 => Some("quotedblleft"),
        171 => Some("guillemotleft"),
        172 => Some("guilsinglleft"),
        173 => Some("guilsinglright"),
        174 => Some("fi"),
        175 => Some("fl"),
        177 => Some("endash"),
        178 => Some("dagger"),
        179 => Some("daggerdbl"),
        180 => Some("periodcentered"),
        182 => Some("paragraph"),
        183 => Some("bullet"),
        184 => Some("quotesinglbase"),
        185 => Some("quotedblbase"),
        186 => Some("quotedblright"),
        187 => Some("guillemotright"),
        188 => Some("ellipsis"),
        189 => Some("perthousand"),
        191 => Some("questiondown"),
        193 => Some("grave"),
        194 => Some("acute"),
        195 => Some("circumflex"),
        196 => Some("tilde"),
        197 => Some("macron"),
        198 => Some("breve"),
        199 => Some("dotaccent"),
        200 => Some("dieresis"),
        202 => Some("ring"),
        203 => Some("cedilla"),
        205 => Some("hungarumlaut"),
        206 => Some("ogonek"),
        207 => Some("caron"),
        208 => Some("emdash"),
        225 => Some("AE"),
        227 => Some("ordfeminine"),
        232 => Some("Lslash"),
        233 => Some("Oslash"),
        234 => Some("OE"),
        235 => Some("ordmasculine"),
        241 => Some("ae"),
        245 => Some("dotlessi"),
        248 => Some("lslash"),
        249 => Some("oslash"),
        250 => Some("oe"),
        251 => Some("germandbls"),
        _ => None,
    }
}

fn charstring_number(bytes: &[u8], at: &mut usize) -> Option<i32> {
    let byte = *bytes.get(*at)?;
    *at += 1;
    match byte {
        32..=246 => Some(byte as i32 - 139),
        247..=250 => {
            let next = *bytes.get(*at)? as i32;
            *at += 1;
            Some((byte as i32 - 247) * 256 + next + 108)
        }
        251..=254 => {
            let next = *bytes.get(*at)? as i32;
            *at += 1;
            Some(-(byte as i32 - 251) * 256 - next - 108)
        }
        255 => {
            let raw = bytes.get(*at..*at + 4)?;
            *at += 4;
            Some(i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]))
        }
        _ => None,
    }
}

fn charstring_plain(cipher: &[u8], len_iv: i32) -> Option<Vec<u8>> {
    if len_iv == -1 {
        return Some(cipher.to_vec());
    }
    let len_iv = usize::try_from(len_iv).ok()?;
    let mut bytes = charstring_decrypt(cipher);
    if len_iv > bytes.len() {
        return None;
    }
    bytes.drain(..len_iv);
    Some(bytes)
}

/// Dependency interpreter, not an outline rewriter. Subroutines share the
/// caller's operand stack; in particular, hint replacement passes its target
/// through OtherSubr 3 and `pop`. Scanning subroutines in isolation loses it.
struct CharStringTrace<'a> {
    subrs: &'a HashMap<usize, Vec<u8>>,
    used_subrs: BTreeSet<usize>,
    seac: BTreeSet<&'static str>,
    stack: Vec<Option<f64>>,
    other_results: Vec<Option<f64>>,
    budget: usize,
}

impl<'a> CharStringTrace<'a> {
    fn new(subrs: &'a HashMap<usize, Vec<u8>>) -> Self {
        Self {
            subrs,
            used_subrs: BTreeSet::new(),
            seac: BTreeSet::new(),
            stack: Vec::new(),
            other_results: Vec::new(),
            budget: 2_000_000,
        }
    }

    fn pop_index(&mut self) -> Option<usize> {
        let value = self.stack.pop()??;
        if value < 0.0 || value > i32::MAX as f64 || value.fract() != 0.0 {
            return None;
        }
        Some(value as usize)
    }

    fn consume(&mut self, count: usize) -> Option<()> {
        self.stack.truncate(self.stack.len().checked_sub(count)?);
        Some(())
    }

    /// Returns true for endchar/seac, false for return. Bound both recursion
    /// and total work so a malformed font falls back to full embedding.
    fn execute(&mut self, bytes: &[u8], depth: usize) -> Option<bool> {
        if depth > 32 {
            return None;
        }
        let mut at = 0;
        while at < bytes.len() {
            self.budget = self.budget.checked_sub(1)?;
            if bytes[at] >= 32 {
                if self.stack.len() >= 96 {
                    return None;
                }
                self.stack
                    .push(Some(charstring_number(bytes, &mut at)? as f64));
                continue;
            }
            let operator = bytes[at];
            at += 1;
            match operator {
                10 => {
                    let index = self.pop_index()?;
                    self.used_subrs.insert(index);
                    let subrs = self.subrs;
                    if self.execute(subrs.get(&index)?, depth + 1)? {
                        return Some(true);
                    }
                }
                11 if depth > 0 => return Some(false),
                14 => return Some(true),
                1 | 3 | 5 | 13 | 21 => self.consume(2)?,
                4 | 6 | 7 | 22 => self.consume(1)?,
                8 => self.consume(6)?,
                30 | 31 => self.consume(4)?,
                9 => {} // closepath has no operands
                12 => {
                    let escaped = *bytes.get(at)?;
                    at += 1;
                    match escaped {
                        0 => {} // dotsection
                        1 | 2 => self.consume(6)?,
                        6 => {
                            let accent = self.pop_index()?;
                            let base = self.pop_index()?;
                            self.consume(3)?;
                            self.seac.insert(standard_encoding_name(base as i32)?);
                            self.seac.insert(standard_encoding_name(accent as i32)?);
                            return Some(true);
                        }
                        7 => self.consume(4)?,
                        12 => {
                            let divisor = self.stack.pop()?;
                            let dividend = self.stack.pop()?;
                            let value = match (dividend, divisor) {
                                (_, Some(0.0)) => return None,
                                (Some(a), Some(b)) => {
                                    let quotient = a / b;
                                    if !quotient.is_finite() {
                                        return None;
                                    }
                                    Some(quotient)
                                }
                                _ => None,
                            };
                            self.stack.push(value);
                        }
                        16 => {
                            let other = self.pop_index()?;
                            let count = self.pop_index()?;
                            let start = self.stack.len().checked_sub(count)?;
                            self.other_results.clear();
                            match (other, count) {
                                // Flex returns its final point. Its geometry
                                // cannot affect dependency indices.
                                (0, 3) => self.other_results.extend([None, None]),
                                (1 | 2, 0) => {}
                                (3, 1) => {
                                    self.other_results.push(self.stack[start]);
                                    // Old interpreters substitute the no-op
                                    // Subr 3 when hint replacement is absent.
                                    if self.subrs.get(&3)?.as_slice() != [11] {
                                        return None;
                                    }
                                    self.used_subrs.insert(3);
                                }
                                // Custom PostScript OtherSubrs need a full
                                // interpreter; never guess their results.
                                _ => return None,
                            }
                            self.stack.truncate(start);
                        }
                        17 => self.stack.push(self.other_results.pop()?),
                        33 => self.consume(2)?,
                        _ => return None,
                    }
                }
                _ => return None,
            }
        }
        None
    }
}

/// Remove unused glyphs and subroutines, following calls and StandardEncoding
/// base/accent glyphs referenced by `seac` composites. The
/// original charstrings and hint programs are copied byte-for-byte.
fn subset_type1(
    data: &[u8],
    length1: usize,
    length2: usize,
    length3: usize,
    glyphs: &BTreeSet<String>,
) -> Option<Type1Program> {
    let encrypted_end = length1.checked_add(length2)?;
    let program_end = encrypted_end.checked_add(length3)?;
    if length2 == 0 || program_end > data.len() {
        return None;
    }
    let plain = eexec_decrypt(&data[length1..encrypted_end]);
    if plain.len() < 4 {
        return None;
    }
    let chars = find_bytes(&plain[4..], b"/CharStrings")? + 4;
    struct SubrEntry {
        range: std::ops::Range<usize>,
        data: std::ops::Range<usize>,
        index: usize,
    }
    let mut subr_entries = Vec::new();
    if let Some(relative) = find_bytes(&plain[4..chars], b"/Subrs") {
        let mut subr_at = relative + 4 + b"/Subrs".len();
        subr_at = skip_space(&plain, subr_at);
        subr_at = token_end(&plain, subr_at); // declared array length
        subr_at = skip_space(&plain, subr_at);
        if &plain[subr_at..token_end(&plain, subr_at)] != b"array" {
            return None;
        }
        subr_at = token_end(&plain, subr_at);
        loop {
            subr_at = skip_space(&plain, subr_at);
            let dup_end = token_end(&plain, subr_at);
            if &plain[subr_at..dup_end] != b"dup" {
                break;
            }
            let entry_start = subr_at;
            subr_at = skip_space(&plain, dup_end);
            let index_end = token_end(&plain, subr_at);
            let index: usize = std::str::from_utf8(&plain[subr_at..index_end])
                .ok()?
                .parse()
                .ok()?;
            subr_at = skip_space(&plain, index_end);
            let length_end = token_end(&plain, subr_at);
            let subr_len: usize = std::str::from_utf8(&plain[subr_at..length_end])
                .ok()?
                .parse()
                .ok()?;
            subr_at = skip_space(&plain, length_end);
            let marker_end = token_end(&plain, subr_at);
            if !matches!(&plain[subr_at..marker_end], b"RD" | b"-|") {
                return None;
            }
            subr_at = marker_end;
            if subr_at >= plain.len() || !plain[subr_at].is_ascii_whitespace() {
                return None;
            }
            subr_at += 1;
            let binary_start = subr_at;
            let binary_end = binary_start.checked_add(subr_len)?;
            if binary_end > chars {
                return None;
            }
            subr_at = skip_space(&plain, binary_end);
            let terminator_end = token_end(&plain, subr_at);
            if !matches!(&plain[subr_at..terminator_end], b"NP" | b"|" | b"noaccess") {
                return None;
            }
            let noaccess = &plain[subr_at..terminator_end] == b"noaccess";
            subr_at = terminator_end;
            if noaccess {
                subr_at = skip_space(&plain, subr_at);
                let end = token_end(&plain, subr_at);
                if &plain[subr_at..end] != b"put" {
                    return None;
                }
                subr_at = end;
            }
            subr_entries.push(SubrEntry {
                range: entry_start..subr_at,
                data: binary_start..binary_end,
                index,
            });
        }
    }
    let begin = find_bytes(&plain[chars..], b"begin")? + chars + b"begin".len();
    let mut at = begin;
    struct CharStringEntry {
        range: std::ops::Range<usize>,
        data: std::ops::Range<usize>,
        name: String,
    }
    let mut entries = Vec::new();
    let mut parsed = 0usize;
    loop {
        at = skip_space(&plain, at);
        if at >= plain.len() || plain[at..].starts_with(b"end") {
            break;
        }
        if plain[at] != b'/' {
            return None;
        }
        let entry_start = at;
        let name_end = token_end(&plain, at + 1);
        let name = std::str::from_utf8(&plain[at + 1..name_end]).ok()?;
        at = skip_space(&plain, name_end);
        let length_end = token_end(&plain, at);
        let char_len: usize = std::str::from_utf8(&plain[at..length_end])
            .ok()?
            .parse()
            .ok()?;
        at = skip_space(&plain, length_end);
        let marker_end = token_end(&plain, at);
        if !matches!(&plain[at..marker_end], b"RD" | b"-|") {
            return None;
        }
        at = marker_end;
        if at >= plain.len() || !plain[at].is_ascii_whitespace() {
            return None;
        }
        // `RD`/`-|` consumes one delimiter byte before the binary string.
        at += 1;
        let binary_start = at;
        let binary_end = at.checked_add(char_len)?;
        if binary_end > plain.len() {
            return None;
        }
        at = skip_space(&plain, binary_end);
        let terminator_end = token_end(&plain, at);
        if !matches!(&plain[at..terminator_end], b"ND" | b"|-" | b"noaccess") {
            return None;
        }
        let noaccess = &plain[at..terminator_end] == b"noaccess";
        at = terminator_end;
        if noaccess {
            at = skip_space(&plain, at);
            let end = token_end(&plain, at);
            if &plain[at..end] != b"def" {
                return None;
            }
            at = end;
        }
        parsed += 1;
        entries.push(CharStringEntry {
            range: entry_start..at,
            data: binary_start..binary_end,
            name: name.to_owned(),
        });
    }
    if parsed == 0 {
        return None;
    }
    let len_iv = ps_int_after(&plain[..chars], b"/lenIV").unwrap_or(4);
    let subrs: HashMap<_, _> = subr_entries
        .iter()
        .map(|entry| {
            Some((
                entry.index,
                charstring_plain(&plain[entry.data.clone()], len_iv)?,
            ))
        })
        .collect::<Option<_>>()?;
    if subrs.len() != subr_entries.len() {
        return None;
    }
    let mut trace = CharStringTrace::new(&subrs);
    let mut keep_glyphs = glyphs.clone();
    keep_glyphs.insert(".notdef".to_owned());
    let mut checked_glyphs = BTreeSet::new();
    loop {
        let mut processed = false;
        for entry in &entries {
            if !keep_glyphs.contains(&entry.name) || !checked_glyphs.insert(entry.name.clone()) {
                continue;
            }
            processed = true;
            trace.stack.clear();
            trace.other_results.clear();
            trace.execute(&charstring_plain(&plain[entry.data.clone()], len_iv)?, 0)?;
            keep_glyphs.extend(trace.seac.iter().map(|name| (*name).to_owned()));
        }
        if !processed {
            break;
        }
    }
    // Missing dependencies or an unsupported program leave the original font
    // intact, including glyphs that a custom OtherSubr could reference.
    if !keep_glyphs.is_subset(&checked_glyphs) {
        return None;
    }
    let mut removals: Vec<_> = entries
        .iter()
        .filter(|entry| !keep_glyphs.contains(&entry.name))
        .map(|entry| entry.range.clone())
        .collect();
    removals.extend(
        subr_entries
            .iter()
            .filter(|entry| !trace.used_subrs.contains(&entry.index))
            .map(|entry| entry.range.clone()),
    );
    if removals.is_empty() {
        return None;
    }
    removals.sort_unstable_by_key(|range| range.start);
    let mut compact = Vec::with_capacity(plain.len());
    let mut copied = 0;
    for range in removals {
        compact.extend_from_slice(&plain[copied..range.start]);
        copied = range.end;
    }
    compact.extend_from_slice(&plain[copied..]);
    let encrypted = eexec_encrypt(&compact);
    let mut subset = Vec::with_capacity(length1 + encrypted.len() + length3);
    subset.extend_from_slice(&data[..length1]);
    subset.extend_from_slice(&encrypted);
    subset.extend_from_slice(&data[encrypted_end..program_end]);
    Some(Type1Program {
        data: subset,
        length1,
        length2: encrypted.len(),
        length3,
    })
}

/// Build an EmbedFont from a font's PFB bytes / encoding vector / TFM widths.
/// A missing PFB yields a font-dict-only entry (viewer substitutes by
/// BaseFont name); a missing encoding vector uses the font's built-in one.
pub fn make_embed_font(
    base_font: String,
    pfb: Option<&[u8]>,
    encoding: Option<&[String]>,
    first_char: u8,
    last_char: u8,
    widths_10000: Vec<i32>,
) -> EmbedFont {
    let (font_file, length1, length2, length3, metrics) = match pfb {
        Some(bytes) => {
            let p = parse_type1(bytes);
            let m = parse_metrics(&p.data[..p.length1.min(p.data.len())]);
            (p.data, p.length1, p.length2, p.length3, m)
        }
        None => (Vec::new(), 0, 0, 0, parse_metrics(b"")),
    };
    // no external encoding: adopt the font's own /Encoding array (if
    // the cleartext declares one) so extractors see accurate glyph names
    let encoding_diff = match encoding {
        Some(e) => Some(e.to_vec()),
        None => pfb.and_then(|bytes| {
            let prog = parse_type1(bytes);
            let end = prog.length1.min(prog.data.len());
            crate::pdf_fonts::builtin_encoding(&prog.data[..end])
        }),
    };
    // /ToUnicode: resolve every encoded slot through the glyph list,
    // keeping only non-identity mappings (ASCII slots extract natively)
    let to_unicode = encoding_diff
        .iter()
        .flat_map(|d| d.iter().enumerate())
        .filter_map(|(slot, g)| {
            if g.is_empty() || slot > 255 {
                return None;
            }
            let uni = crate::pdf_fonts::glyph_to_unicode(g)?;
            if slot < 0x80 && uni.len() == 1 && uni.as_bytes()[0] == slot as u8 {
                return None;
            }
            Some((slot as u8, uni))
        })
        .collect();
    EmbedFont {
        obj_font: 0,
        base_font,
        font_file,
        length1,
        length2,
        length3,
        encoding_diff,
        first_char,
        last_char,
        widths: widths_10000,
        font_matrix_scale: 1.0,
        font_bbox: metrics.font_bbox,
        italic_angle: metrics.italic_angle,
        ascent: metrics.ascent,
        descent: metrics.descent,
        cap_height: metrics.cap_height,
        stem_v: metrics.stem_v,
        flags: 4,
        to_unicode,
        used_chars: [0; 4],
    }
}

// ---------------------------------------------------------------- encryption

/// Configuration for standard PDF encryption (Standard security handler, /V 2, /R 3, 128-bit key).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfEncryptConfig {
    pub user_password: Vec<u8>,
    pub owner_password: Vec<u8>,
    pub permissions: i32,
    pub file_id: Option<[u8; 16]>,
}

impl PdfEncryptConfig {
    pub fn new(user_password: impl AsRef<[u8]>, owner_password: impl AsRef<[u8]>) -> Self {
        Self {
            user_password: user_password.as_ref().to_vec(),
            owner_password: owner_password.as_ref().to_vec(),
            permissions: -4,
            file_id: None,
        }
    }
}

impl Default for PdfEncryptConfig {
    fn default() -> Self {
        Self::new("", "")
    }
}

/// Computed PDF standard encryption dictionary fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StandardEncryptionDict {
    pub filter: &'static str,
    pub v: i32,
    pub r: i32,
    pub length: i32,
    pub p: i32,
    pub o: [u8; 32],
    pub u: [u8; 32],
    pub file_encryption_key: [u8; 16],
}

impl StandardEncryptionDict {
    pub fn to_pdf_dict(&self) -> String {
        let hex_o: String = self.o.iter().map(|b| format!("{:02X}", b)).collect();
        let hex_u: String = self.u.iter().map(|b| format!("{:02X}", b)).collect();
        format!(
            "<< /Filter /Standard /V {} /R {} /Length {} /P {} /O <{}> /U <{}> >>",
            self.v, self.r, self.length, self.p, hex_o, hex_u
        )
    }
}

/// Standard 32-byte password padding string (ISO 32000-1 §7.6.3.3).
pub const STANDARD_ENCRYPTION_PADDING: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41,
    0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80,
    0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

fn pad_password(pwd: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let len = pwd.len().min(32);
    out[..len].copy_from_slice(&pwd[..len]);
    if len < 32 {
        out[len..].copy_from_slice(&STANDARD_ENCRYPTION_PADDING[..32 - len]);
    }
    out
}

struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    fn new(key: &[u8]) -> Self {
        assert!(!key.is_empty() && key.len() <= 256);
        let mut s = [0u8; 256];
        for (i, v) in s.iter_mut().enumerate() {
            *v = i as u8;
        }
        let mut j: u8 = 0;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, j as usize);
        }
        Rc4 { s, i: 0, j: 0 }
    }

    fn apply(&mut self, data: &mut [u8]) {
        for b in data.iter_mut() {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[self.i as usize]);
            self.s.swap(self.i as usize, self.j as usize);
            let k = self.s[(self.s[self.i as usize].wrapping_add(self.s[self.j as usize])) as usize];
            *b ^= k;
        }
    }
}

/// Algorithm 3.3: Compute /O hash for Revision 3 (128-bit key) using MD5 and RC4.
pub fn compute_o_hash(user_pwd: &[u8], owner_pwd: &[u8]) -> [u8; 32] {
    let effective_owner = if owner_pwd.is_empty() {
        user_pwd
    } else {
        owner_pwd
    };
    let padded_owner = pad_password(effective_owner);
    let mut digest = md5::compute(&padded_owner).0;
    for _ in 0..50 {
        digest = md5::compute(&digest).0;
    }
    let key = &digest[..16];
    let mut text = pad_password(user_pwd);
    let mut rc4 = Rc4::new(key);
    rc4.apply(&mut text);
    for i in 1..=19u8 {
        let mut round_key = [0u8; 16];
        for k in 0..16 {
            round_key[k] = key[k] ^ i;
        }
        let mut round_rc4 = Rc4::new(&round_key);
        round_rc4.apply(&mut text);
    }
    text
}

/// Algorithm 3.2: Compute file encryption key for Revision 3 (128-bit key).
pub fn compute_file_encryption_key(
    user_pwd: &[u8],
    o_hash: &[u8; 32],
    permissions: i32,
    file_id: &[u8],
) -> [u8; 16] {
    let padded_user = pad_password(user_pwd);
    let p_bytes = (permissions as u32).to_le_bytes();
    let mut ctx = md5::Context::new();
    ctx.consume(&padded_user);
    ctx.consume(o_hash);
    ctx.consume(&p_bytes);
    ctx.consume(file_id);
    let mut digest = ctx.finalize().0;
    for _ in 0..50 {
        digest = md5::compute(&digest[..16]).0;
    }
    let mut key = [0u8; 16];
    key.copy_from_slice(&digest[..16]);
    key
}

/// Algorithm 3.5: Compute /U hash for Revision 3 (128-bit key) using MD5 and RC4.
pub fn compute_u_hash(file_encryption_key: &[u8; 16], file_id: &[u8]) -> [u8; 32] {
    let mut ctx = md5::Context::new();
    ctx.consume(&STANDARD_ENCRYPTION_PADDING);
    ctx.consume(file_id);
    let mut u_digest = ctx.finalize().0;
    let mut rc4 = Rc4::new(file_encryption_key);
    rc4.apply(&mut u_digest);
    for i in 1..=19u8 {
        let mut round_key = [0u8; 16];
        for k in 0..16 {
            round_key[k] = file_encryption_key[k] ^ i;
        }
        let mut round_rc4 = Rc4::new(&round_key);
        round_rc4.apply(&mut u_digest);
    }
    let mut u_val = [0u8; 32];
    u_val[..16].copy_from_slice(&u_digest);
    u_val
}

/// Generate standard PDF encryption dictionary (/V 2, /R 3, 128-bit key).
pub fn generate_encryption_dictionary(
    config: &PdfEncryptConfig,
    file_id: &[u8; 16],
) -> (StandardEncryptionDict, String) {
    let o = compute_o_hash(&config.user_password, &config.owner_password);
    let key = compute_file_encryption_key(
        &config.user_password,
        &o,
        config.permissions,
        file_id,
    );
    let u = compute_u_hash(&key, file_id);
    let dict = StandardEncryptionDict {
        filter: "Standard",
        v: 2,
        r: 3,
        length: 128,
        p: config.permissions,
        o,
        u,
        file_encryption_key: key,
    };
    let dict_str = dict.to_pdf_dict();
    (dict, dict_str)
}

// ---------------------------------------------------------------- PDF/A validation

/// Validate PDF/A metadata: checks that if PDF/A compliance is requested,
/// an /OutputIntents dictionary containing /OutputConditionIdentifier (sRGB) is present.
pub fn validate_pdfa_metadata(doc: &PdfDoc) -> Result<(), String> {
    let extra = String::from_utf8_lossy(&doc.catalog_extra);
    let pdfa_requested = doc.pdfa
        || extra.contains("/GTS_PDFA1")
        || extra.contains("PDF/A")
        || extra.contains("PDFA")
        || doc
            .minor_version
            .is_some_and(|v| v <= 4 && (extra.contains("OutputIntent") || extra.contains("GTS_")));
    if pdfa_requested && extra.contains("/OutputIntents") {
        if !extra.contains("/OutputConditionIdentifier") || !extra.contains("sRGB") {
            return Err("PDF/A validation error: /OutputIntents dictionary must contain /OutputConditionIdentifier (sRGB)".to_string());
        }
    }
    Ok(())
}

/// Validate a catalog dictionary string for PDF/A /OutputIntents conformance tags.
pub fn validate_pdfa_catalog(catalog_str: &str) -> Result<(), String> {
    if !catalog_str.contains("/OutputIntents") {
        return Err("PDF/A metadata validation failed: missing /OutputIntents in catalog".to_string());
    }
    if !catalog_str.contains("/OutputConditionIdentifier") || !catalog_str.contains("sRGB") {
        return Err("PDF/A metadata validation failed: /OutputIntents must contain /OutputConditionIdentifier (sRGB)".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------- serializer

struct PdfBuilder {
    packable: Vec<bool>,
    objs: Vec<Option<Vec<u8>>>, // index n-1 holds object n
}

impl PdfBuilder {
    fn new() -> Self {
        PdfBuilder {
            objs: Vec::new(),
            packable: Vec::new(),
        }
    }

    fn alloc(&mut self) -> usize {
        if self.objs.len() >= crate::engine::MAX_PAGE_LIST {
            return self.objs.len().max(1);
        }
        self.objs.push(None);
        self.objs.len()
    }

    fn set(&mut self, num: usize, body: String) {
        if num == 0 || num > crate::engine::MAX_PAGE_LIST {
            return;
        }
        if self.objs.len() < num {
            self.objs.resize(num, None);
        }
        self.objs[num - 1] = Some(body.into_bytes());
        self.packable.resize(self.objs.len(), false);
        self.packable[num - 1] = true;
    }

    fn set_bytes(&mut self, num: usize, body: Vec<u8>) {
        if num == 0 || num > crate::engine::MAX_PAGE_LIST {
            return;
        }
        if self.objs.len() < num {
            self.objs.resize(num, None);
        }
        self.objs[num - 1] = Some(body);
        self.packable.resize(self.objs.len(), false);
        self.packable[num - 1] = false;
    }

    fn set_stream(&mut self, num: usize, dict_extra: &str, data: &[u8], compress: bool) {
        if num == 0 || num > crate::engine::MAX_PAGE_LIST {
            return;
        }
        if self.objs.len() < num {
            self.objs.resize(num, None);
        }
        let data = if compress { flate(data) } else { data.to_vec() };
        self.set_encoded_stream(num, dict_extra, &data, compress);
    }

    fn set_encoded_stream(&mut self, num: usize, dict_extra: &str, data: &[u8], compress: bool) {
        if num == 0 || num > crate::engine::MAX_PAGE_LIST {
            return;
        }
        if self.objs.len() < num {
            self.objs.resize(num, None);
        }
        let filter = if compress {
            " /Filter /FlateDecode"
        } else {
            ""
        };
        let mut body = format!(
            "<< /Length {}{} {}>>\nstream\n",
            data.len(),
            filter,
            dict_extra
        )
        .into_bytes();
        body.extend_from_slice(&data);
        body.extend_from_slice(b"\nendstream");
        self.objs[num - 1] = Some(body);
        self.packable.resize(self.objs.len(), false);
        self.packable[num - 1] = false;
    }
}

/// Compact f64 formatting for PDF numbers: no trailing zero runs.
fn num(v: f64) -> String {
    if !v.is_finite() || v.abs() < 1e-5 {
        return "0".to_string();
    }
    let sign = if v < -1e-5 { "-" } else { "" };
    let abs_v = v.abs();
    let int_part = abs_v as i64;
    let frac = ((abs_v - int_part as f64) * 10000.0).round() as i64;
    let (int_part, frac) = if frac >= 10000 {
        (int_part + 1, 0)
    } else {
        (int_part, frac)
    };
    if frac == 0 {
        format!("{sign}{int_part}")
    } else {
        let mut f = frac;
        let mut width = 4;
        while f % 10 == 0 && width > 0 {
            f /= 10;
            width -= 1;
        }
        format!("{sign}{int_part}.{f:0width$}", width = width)
    }
}

/// Escape a PDF literal string body.
fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a PDF name (identifier).
fn escape_pdf_name(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '#' | '(' | ')' | '<' | '>' | '[' | ']' | '/' | '%' => format!("#{:02x}", c as u8),
            _ => c.to_string(),
        })
        .collect()
}

/// PDF text string: ASCII as literal, otherwise UTF-16BE hex.
fn pdf_text_string(s: &str) -> String {
    if s.is_ascii() {
        return format!("({})", escape_string(s));
    }
    let mut hex = String::from("FEFF");
    for u in s.encode_utf16() {
        hex += &format!("{:04X}", u);
    }
    format!("<{}>", hex)
}

type FontFileKey = (usize, usize, usize, u64);

struct PreparedType1 {
    program: Type1Program,
    pdf_name: String,
}

fn font_file_key(font: &EmbedFont) -> FontFileKey {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in &font.font_file {
        hash = (hash ^ byte as u64).wrapping_mul(0x1000_0000_01b3);
    }
    (font.length1, font.length2, font.length3, hash)
}

fn required_glyphs(font: &EmbedFont) -> Option<BTreeSet<String>> {
    if font.used_chars == [0; 4] {
        return None;
    }
    let encoding = font.encoding_diff.as_ref()?;
    let mut glyphs = BTreeSet::new();
    for code in 0..=u8::MAX {
        if char_is_used(&font.used_chars, code) {
            let glyph = encoding.get(code as usize)?;
            if glyph.is_empty() {
                return None;
            }
            glyphs.insert(glyph.clone());
        }
    }
    Some(glyphs)
}

fn subset_font_name(base_font: &str, key: FontFileKey, glyphs: &BTreeSet<String>) -> String {
    let mut hash = key.3;
    for glyph in glyphs {
        for &byte in glyph.as_bytes() {
            hash = (hash ^ byte as u64).wrapping_mul(0x1000_0000_01b3);
        }
    }
    let mut tag = String::with_capacity(6);
    for _ in 0..6 {
        tag.push((b'A' + (hash % 26) as u8) as char);
        hash = hash.rotate_right(7) ^ (hash >> 11);
    }
    format!("{tag}+{base_font}")
}

fn rename_type1_font(program: &mut Type1Program, new_name: &str) -> bool {
    let Some(font_name) = find_bytes(&program.data[..program.length1], b"/FontName") else {
        return false;
    };
    let mut start = skip_space(&program.data, font_name + b"/FontName".len());
    if program.data.get(start) != Some(&b'/') {
        return false;
    }
    start += 1;
    let end = token_end(&program.data, start);
    let old_len = end - start;
    program
        .data
        .splice(start..end, new_name.as_bytes().iter().copied());
    if new_name.len() >= old_len {
        program.length1 += new_name.len() - old_len;
    } else {
        program.length1 -= old_len - new_name.len();
    }
    true
}

pub fn write_pdf(doc: &PdfDoc) -> Vec<u8> {
    let mut b = PdfBuilder::new();
    for (obj_num, obj_body) in &doc.objects {
        b.set_bytes(*obj_num as usize, obj_body.clone());
    }
    let catalog_obj = b.alloc(); // after user objects
    let pages_obj = b.alloc();

    // Collect named destinations first (first definition wins, sorted),
    // so the names object is only allocated when needed.
    let mut named: Vec<(String, usize, f64, f64, u8, Option<f64>)> = Vec::new();
    for (pi, page) in doc.pages.iter().enumerate() {
        for dest in &page.dests {
            if !named.iter().any(|(n, ..)| n == &dest.name) {
                named.push((dest.name.clone(), pi, dest.x, dest.y, dest.kind, dest.zoom));
            }
        }
    }
    named.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let names_obj = if named.is_empty() && doc.names_extra.is_empty() {
        0
    } else {
        b.alloc()
    };

    // Font objects: font dict, descriptor, optional font file and
    // optional /ToUnicode CMap.
    struct FontObjs {
        font: usize,
        desc: usize,
        file: Option<usize>,
        tounicode: Option<usize>,
    }
    // A single PFB can back several TeX font instances (different sizes or
    // encodings). Union their glyphs before subsetting so those instances can
    // continue sharing one embedded font program.
    // Sizes and encodings can share the same program. Use the standard
    // slice hash to identify equal bytes, computing the stable subset-name
    // fingerprint only once for each complete program.
    let mut key_cache = HashMap::new();
    let font_keys: Vec<_> = doc
        .fonts
        .iter()
        .map(|font| {
            *key_cache
                .entry((
                    font.length1,
                    font.length2,
                    font.length3,
                    font.font_file.as_slice(),
                ))
                .or_insert_with(|| font_file_key(font))
        })
        .collect();
    let mut glyphs_by_file: HashMap<FontFileKey, Option<BTreeSet<String>>> = HashMap::new();
    for (font, &key) in doc.fonts.iter().zip(&font_keys) {
        if font.font_file.is_empty() {
            continue;
        }
        let requested = required_glyphs(font);
        match glyphs_by_file.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(requested);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                match (entry.get_mut(), requested) {
                    (Some(current), Some(additional)) => current.extend(additional),
                    (slot, None) => *slot = None,
                    (None, Some(_)) => {}
                }
            }
        }
    }
    let mut prepared_files: HashMap<FontFileKey, PreparedType1> = HashMap::new();
    let mut attempted_files = BTreeSet::new();
    let jobs: Vec<_> = doc
        .fonts
        .iter()
        .zip(&font_keys)
        .filter_map(|(font, &key)| {
            if !attempted_files.insert(key) {
                return None;
            }
            let Some(Some(glyphs)) = glyphs_by_file.get(&key) else {
                return None;
            };
            Some((font, key, glyphs))
        })
        .collect();
    let prepare = |&(font, key, glyphs): &(&EmbedFont, FontFileKey, &BTreeSet<String>)| {
        let mut subset = subset_type1(
            &font.font_file,
            font.length1,
            font.length2,
            font.length3,
            glyphs,
        )?;
        let pdf_name = subset_font_name(&font.base_font, key, glyphs);
        rename_type1_font(&mut subset, &pdf_name).then_some((
            key,
            PreparedType1 {
                program: subset,
                pdf_name,
            },
        ))
    };
    if !cfg!(target_arch = "wasm32") && jobs.len() >= 4 && !crate::debug_flag("TEX_PDF_SERIAL") {
        // At most three helper threads, with TeX's thread processing a share.
        let workers = std::thread::available_parallelism().map_or(1, |n| n.get().min(4));
        std::thread::scope(|scope| {
            let chunks: Vec<_> = jobs.chunks(jobs.len().div_ceil(workers)).collect();
            let mut handles = Vec::new();
            for &chunk in chunks.iter().skip(1) {
                let prepare = &prepare;
                match std::thread::Builder::new()
                    .name("pdf-subset".into())
                    .spawn_scoped(scope, move || {
                        chunk.iter().filter_map(prepare).collect::<Vec<_>>()
                    }) {
                    Ok(handle) => handles.push(handle),
                    Err(_) => prepared_files.extend(chunk.iter().filter_map(&prepare)),
                }
            }
            prepared_files.extend(chunks[0].iter().filter_map(&prepare));
            for handle in handles {
                prepared_files.extend(handle.join().expect("font subset worker"));
            }
        });
    } else {
        prepared_files.extend(jobs.iter().filter_map(prepare));
    }

    // Deduplicate font files and ToUnicode CMaps across font instances.
    let mut file_cache: HashMap<FontFileKey, usize> = HashMap::new();
    let mut tounicode_cache: HashMap<Vec<(u8, String)>, usize> = HashMap::new();

    let font_objs: Vec<FontObjs> = doc
        .fonts
        .iter()
        .zip(&font_keys)
        .map(|(f, &key)| {
            let font = b.alloc();
            let desc = b.alloc();
            let file = if f.font_file.is_empty() {
                None
            } else {
                Some(*file_cache.entry(key).or_insert_with(|| b.alloc()))
            };
            let tounicode = if f.to_unicode.is_empty() {
                None
            } else {
                Some(
                    *tounicode_cache
                        .entry(f.to_unicode.clone())
                        .or_insert_with(|| b.alloc()),
                )
            };
            FontObjs {
                font,
                desc,
                file,
                tounicode,
            }
        })
        .collect();
    for (object, fonts) in &doc.form_fonts {
        let mut dict = String::from("<<");
        for (index, number) in fonts {
            if let Some(font) = font_objs.get(*index) {
                dict.push_str(&format!(" /F{number} {} 0 R", font.font));
            }
        }
        dict.push_str(" >>");
        b.set(*object as usize, dict);
    }
    // Page objects: content stream, page dict, one object per annotation.
    let page_objs: Vec<(usize, usize, Vec<usize>)> = doc
        .pages
        .iter()
        .map(|p| {
            let content = b.alloc();
            let page = b.alloc();
            let annots = (0..p.annots.len()).map(|_| b.alloc()).collect();
            (content, page, annots)
        })
        .collect();

    // Outlines: root + one entry per \pdfoutline (flat, linked as siblings).
    let outlines: Option<(usize, Vec<usize>)> = if doc.outlines.is_empty() {
        None
    } else {
        let root = b.alloc();
        let entries = (0..doc.outlines.len()).map(|_| b.alloc()).collect();
        Some((root, entries))
    };

    let info_obj = b.alloc();
    // ---- emit fonts
    let mut emitted_files: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut emitted_tounicode: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for ((f, fo), key) in doc.fonts.iter().zip(&font_objs).zip(&font_keys) {
        let prepared = prepared_files.get(key);
        let pdf_name = prepared.map_or(f.base_font.as_str(), |font| font.pdf_name.as_str());
        let first = f.first_char as i32;
        let last = f.last_char as i32;
        let n = (last - first + 1).max(1) as usize;
        let mut widths = String::from("[ ");
        for k in 0..n {
            widths.push_str(&num(f.widths.get(k).copied().unwrap_or(0) as f64 / 10.0));
            widths.push(' ');
        }
        widths.push(']');
        let mut enc = String::new();
        if let Some(diffs) = &f.encoding_diff {
            enc.push_str(" /Encoding << /Type /Encoding /Differences [ ");
            for (slot, g) in diffs.iter().enumerate() {
                if !g.is_empty()
                    && (f.used_chars == [0; 4]
                        || (slot <= u8::MAX as usize && char_is_used(&f.used_chars, slot as u8)))
                {
                    enc.push_str(&format!("{} /{} ", slot, escape_pdf_name(g)));
                }
            }
            enc.push_str(" ] >>");
        }
        let tounicode_ref = match fo.tounicode {
            Some(o) => format!(" /ToUnicode {} 0 R", o),
            None => String::new(),
        };
        b.set(
            fo.font,
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /{} /FirstChar {} /LastChar {} /Widths {} /FontDescriptor {} 0 R{}{} >>",
                escape_pdf_name(pdf_name), first, last, widths, fo.desc, enc, tounicode_ref
            ),
        );
        let (ascent, descent) = if f.ascent - f.descent > 3000.0 {
            (f.ascent, f.ascent - 3000.0)
        } else {
            (f.ascent, f.descent)
        };
        let mut desc = format!(
            "<< /Type /FontDescriptor /FontName /{} /Flags {} /FontBBox [{} {} {} {}] /ItalicAngle {} /Ascent {} /Descent {} /CapHeight {} /StemV {}",
            escape_pdf_name(pdf_name),
            f.flags,
            num(f.font_bbox[0]), num(f.font_bbox[1]), num(f.font_bbox[2]), num(f.font_bbox[3]),
            num(f.italic_angle), num(ascent), num(descent), num(f.cap_height), num(f.stem_v),
        );
        if let Some(file) = fo.file {
            desc.push_str(&format!(" /FontFile {} 0 R", file));
        }
        desc.push_str(" >>");
        b.set(fo.desc, desc);
        if let Some(file) = fo.file {
            if emitted_files.insert(file) {
                let (font_data, length1, length2, length3) = prepared.map_or(
                    (&f.font_file[..], f.length1, f.length2, f.length3),
                    |font| {
                        (
                            &font.program.data[..],
                            font.program.length1,
                            font.program.length2,
                            font.program.length3,
                        )
                    },
                );
                b.set_stream(
                    file,
                    &format!(
                        "/Length1 {} /Length2 {} /Length3 {}",
                        length1, length2, length3
                    ),
                    font_data,
                    true,
                );
            }
        }
        if let Some(to) = fo.tounicode {
            if emitted_tounicode.insert(to) {
                b.set_stream(to, "", to_unicode_cmap(&f.to_unicode).as_bytes(), true);
            }
        }
    }

    // ---- emit pages
    for (i, page) in doc.pages.iter().enumerate() {
        let (content_obj, page_obj, annot_objs) = &page_objs[i];
        if let Some(compressed) = doc.compressed_page(i) {
            b.set_encoded_stream(*content_obj, "", compressed, true);
        } else {
            b.set_stream(*content_obj, "", &page.content, true);
        }
        let mut fonts_res = String::new();
        for (fidx, fnum) in &page.fonts {
            if let Some(fo) = font_objs.get(*fidx) {
                fonts_res.push_str(&format!("/F{} {} 0 R ", fnum, fo.font));
            }
        }
        let mut annots_res = String::new();
        for (a, aobj) in page.annots.iter().zip(annot_objs) {
            emit_annot(&mut b, *aobj, a);
            annots_res.push_str(&format!("{} 0 R ", aobj));
        }
        let annots = if annots_res.is_empty() {
            String::new()
        } else {
            format!(" /Annots [ {}]", annots_res)
        };
        let page_attr = String::from_utf8_lossy(&page.attr_extra);
        let res_extra = String::from_utf8_lossy(&page.resources_extra);
        let res_extra_str = if res_extra.trim().is_empty() {
            String::new()
        } else {
            format!(" {}", res_extra.trim())
        };
        let mut xobj_entries = Vec::new();
        for (obj_num, bytes) in &doc.objects {
            if bytes.starts_with(b"<< /Type /XObject") {
                let fm_name = format!("/Fm{} Do", obj_num);
                if page
                    .content
                    .windows(fm_name.len())
                    .any(|part| part == fm_name.as_bytes())
                {
                    xobj_entries.push(format!("/Fm{} {} 0 R", obj_num, obj_num));
                }
                let im_name = format!("/Im{} Do", obj_num);
                if page
                    .content
                    .windows(im_name.len())
                    .any(|part| part == im_name.as_bytes())
                {
                    xobj_entries.push(format!("/Im{} {} 0 R", obj_num, obj_num));
                }
            }
        }
        let xobj_str = if xobj_entries.is_empty() {
            String::new()
        } else {
            format!(" /XObject << {} >>", xobj_entries.join(" "))
        };
        b.set(
            *page_obj,
            format!(
                "<< /Type /Page /Parent {} 0 R /MediaBox [0 0 {} {}] /Contents {} 0 R /Resources << /Font << {} >> /ProcSet [/PDF /Text]{}{} >>{}{} >>",
                pages_obj,
                num(page.width_bp),
                num(page.height_bp),
                content_obj,
                fonts_res,
                xobj_str,
                res_extra_str,
                annots,
                if page_attr.trim().is_empty() {
                    String::new()
                } else {
                    format!(" {}", page_attr)
                }
            ),
        );
    }

    // ---- emit the page tree node
    let kids: Vec<String> = page_objs
        .iter()
        .map(|(_, p, _)| format!("{} 0 R", p))
        .collect();
    let pages_attr = String::from_utf8_lossy(&doc.pages_attr);
    b.set(
        pages_obj,
        format!(
            "<< /Type /Pages /Count {} /Kids [ {} ]{} >>",
            page_objs.len(),
            kids.join(" "),
            if pages_attr.trim().is_empty() {
                String::new()
            } else {
                format!(" {}", pages_attr)
            }
        ),
    );

    // ---- emit named destination tree
    if names_obj != 0 {
        let mut body = String::from("<< ");
        if !named.is_empty() {
            body.push_str("/Names [ ");
            for (name, pi, x, y, kind, zoom) in &named {
                let page_ref = page_objs[*pi].1;
                let term = match kind {
                    1 => "/Fit".to_string(),
                    2 => format!("/FitH {}", num(*y)),
                    3 => format!("/FitV {}", num(*x)),
                    4 => "/FitB".to_string(),
                    5 => format!("/FitBH {}", num(*y)),
                    6 => format!("/FitBV {}", num(*x)),
                    7 => format!("/FitR {} {} {} {}", num(*x), num(*y), num(*x), num(*y)),
                    _ => format!(
                        "/XYZ {} {} {}",
                        num(*x),
                        num(*y),
                        zoom.map(|z| num(z)).unwrap_or_else(|| "null".to_string())
                    ),
                };
                body.push_str(&format!(
                    "({}) [{} 0 R {}] ",
                    escape_string(name),
                    page_ref,
                    term
                ));
            }
            body.push_str(" ] ");
        }
        // \pdfnames entries merge into the same /Names dictionary
        body.push_str(&String::from_utf8_lossy(&doc.names_extra));
        body.push_str(" >>");
        b.set(names_obj, body);
    }

    // ---- emit outlines
    if let Some((root, entries)) = &outlines {
        let n = entries.len();
        for (k, ((title, dest, count), eobj)) in doc.outlines.iter().zip(entries).enumerate() {
            let mut body = format!("<< /Title {} /Parent {} 0 R", pdf_text_string(title), root);
            if k > 0 {
                body.push_str(&format!(" /Prev {} 0 R", entries[k - 1]));
            }
            if k + 1 < n {
                body.push_str(&format!(" /Next {} 0 R", entries[k + 1]));
            }
            if !dest.is_empty() {
                body.push_str(&format!(" /Dest ({})", escape_string(dest)));
            }
            if *count != 0 {
                body.push_str(&format!(" /Count {}", count));
            }
            body.push_str(" >>");
            b.set(*eobj, body);
        }
        b.set(
            *root,
            format!(
                "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {} >>",
                entries[0],
                entries[n - 1],
                n
            ),
        );
    }

    // ---- emit info
    let info_str = String::from_utf8_lossy(&doc.info);
    let mut info_body = format!("<< {}", info_str);
    if !info_str.contains("/Producer") {
        info_body.push_str(" /Producer (tex-rs)");
    }
    if !info_str.contains("/Creator") {
        info_body.push_str(" /Creator (tex-rs)");
    }
    info_body.push_str(" >>");
    b.set(info_obj, info_body);
    // ---- emit catalog
    let mut cat = format!("<< /Type /Catalog /Pages {} 0 R", pages_obj);
    if names_obj != 0 {
        cat.push_str(&format!(" /Names << /D {} 0 R >>", names_obj));
    }
    if let Some((page, view)) = &doc.open_action {
        if let Some(&(content, page_obj, _)) =
            page_objs.get((*page).checked_sub(1).unwrap_or(0) as usize)
        {
            let _ = content;
            let view_s = view.trim();
            let view_pdf =
                if !view_s.is_empty() && view_s.chars().all(|c| c.is_ascii_alphanumeric()) {
                    format!("/{}", view_s)
                } else {
                    view_s.to_string()
                };
            cat.push_str(&format!(" /OpenAction [{} 0 R {}]", page_obj, view_pdf));
        }
    }
    if let Some((root, _)) = &outlines {
        cat.push_str(&format!(" /Outlines {} 0 R", root));
    }
    let has_tagged_pdf = doc
        .pages
        .iter()
        .any(|p| p.display_list.as_ref().is_some_and(|dl| dl.has_structure_tags()));
    if has_tagged_pdf {
        let struct_tree_root_obj = b.alloc();
        cat.push_str(&format!(
            " /MarkInfo << /Marked true >> /StructTreeRoot {} 0 R",
            struct_tree_root_obj
        ));
        b.set(
            struct_tree_root_obj,
            "<< /Type /StructTreeRoot /RoleMap << /H1 /H /H2 /H /H3 /H /H4 /H /H5 /H /H6 /H >> >>"
                .to_string(),
        );
    }
    let extra = String::from_utf8_lossy(&doc.catalog_extra);
    let pdfa_requested = doc.pdfa
        || extra.contains("/GTS_PDFA1")
        || extra.contains("PDF/A")
        || extra.contains("PDFA")
        || doc
            .minor_version
            .is_some_and(|v| v <= 4 && (extra.contains("OutputIntent") || extra.contains("GTS_")));

    if pdfa_requested && !extra.contains("/OutputIntents") {
        cat.push_str(" /OutputIntents [ << /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (sRGB) /Info (sRGB) >> ]");
    }
    if !extra.trim().is_empty() {
        cat.push(' ');
        cat.push_str(&extra);
    }
    cat.push_str(" >>");
    b.set(catalog_obj, cat);

    let (encrypt_obj, file_id) = if let Some(enc_cfg) = &doc.encrypt {
        let fid: [u8; 16] = enc_cfg.file_id.unwrap_or_else(|| {
            let mut ctx = md5::Context::new();
            if !doc.info.is_empty() {
                ctx.consume(&doc.info);
            } else {
                ctx.consume(b"ratex-default-doc-id");
            }
            ctx.finalize().0
        });
        let (_enc_dict, dict_str) = generate_encryption_dictionary(enc_cfg, &fid);
        let enc_obj = b.alloc();
        b.set_bytes(enc_obj, dict_str.into_bytes());
        (Some(enc_obj), Some(fid))
    } else {
        (None, None)
    };

    // Raw extension objects can contain duplicate keys, uncompressed streams
    // and arbitrary syntax. Preserve the existing parser's normalization for
    // those documents, in memory, before writing the final file once.
    let normalize = !doc.objects.is_empty()
        || !doc.names_extra.is_empty()
        || !doc.pages_attr.is_empty()
        || doc.encrypt.is_some()
        || doc.pdfa
        || doc.minor_version.is_some_and(|v| v <= 4)
        || ["/Type", "/Pages", "/Names", "/Outlines", "/OpenAction"]
            .iter()
            .any(|key| {
                doc.catalog_extra
                    .windows(key.len())
                    .any(|s| s == key.as_bytes())
            })
        || doc.pages.iter().any(|p| {
            !p.attr_extra.is_empty()
                || !p.resources_extra.is_empty()
                || p.annots.iter().any(|a| {
                    ["/Type", "/Subtype", "/Rect", "/A ", "/Dest"]
                        .iter()
                        .any(|key| a.attr.contains(key))
                })
        });
    if normalize {
        crate::pdfcompact::serialize_compatible(
            &b.objs,
            catalog_obj,
            info_obj,
            encrypt_obj,
            file_id,
            doc.minor_version,
        )
    } else {
        crate::pdfcompact::serialize(
            &b.objs,
            &b.packable,
            catalog_obj,
            info_obj,
            encrypt_obj,
            file_id,
            doc.minor_version,
        )
    }
}

fn emit_annot(b: &mut PdfBuilder, obj: usize, a: &Annot) {
    let [x0, y0, x1, y1] = a.rect;
    // /Border comes from the annotation attributes when present (hyperref
    // passes pdfborder explicitly); duplicating the key makes qpdf flag
    // every link object
    let border = if a.attr.contains("/Border") {
        ""
    } else {
        " /Border [0 0 0]"
    };
    let mut body = format!(
        "<< /Type /Annot /Subtype {} /Rect [{} {} {} {}]{}",
        a.subtype.as_deref().unwrap_or("/Link"),
        num(x0),
        num(y0),
        num(x1),
        num(y1),
        border
    );
    if let Some(uri) = &a.uri {
        body.push_str(&format!(" /A << /S /URI /URI ({}) >>", escape_string(uri)));
    }
    if let Some(dest) = &a.dest {
        body.push_str(&format!(" /Dest ({})", escape_string(dest)));
    }
    if !a.attr.is_empty() {
        body.push(' ');
        body.push_str(&a.attr);
    }
    body.push_str(" >>");
    b.set(obj, body);
}
/// Build a /ToUnicode CMap stream body from (code, Unicode string) pairs.
/// The stream is written flate-compressed; bfchar blocks are chunked at
/// 100 entries (PDF limit).
fn to_unicode_cmap(map: &[(u8, String)]) -> String {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n\
         /CMapType 2 def\n\
         1 begincodespacerange\n\
         <00> <FF>\n\
         endcodespacerange\n",
    );
    for chunk in map.chunks(100) {
        s.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (code, uni) in chunk {
            let mut hex = String::new();
            for u in uni.encode_utf16() {
                hex += &format!("{:04X}", u);
            }
            s.push_str(&format!("<{:02X}> <{}>\n", code, hex));
        }
        s.push_str("endbfchar\n");
    }
    s.push_str(
        "endcmap\n\
         CMapName currentdict /CMap defineresource pop\n\
         end\n\
         end",
    );
    s
}

/// Compact a generated PDF by recompressing streams and packing indirect
/// objects into PDF 1.5 object streams.
pub fn optimize_pdf_file(path: &str) {
    // Keep this native. External PDF processors can alter font encodings,
    // color profiles and raster output.
    if let Ok(mut doc) = lopdf::Document::load(path) {
        doc.compress();
        let mut modern_bytes = Vec::new();
        if doc.save_modern(&mut modern_bytes).is_ok() {
            if let Ok(meta_old) = tex_kpse::fs::metadata(path) {
                if (modern_bytes.len() as u64) < meta_old.len() && !modern_bytes.is_empty() {
                    let _ = tex_kpse::fs::write(path, &modern_bytes);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn charstring_encrypt(plain: &[u8]) -> Vec<u8> {
        let mut state = 4_330_u16;
        plain
            .iter()
            .map(|&byte| {
                let cipher = byte ^ (state >> 8) as u8;
                state = (cipher as u16)
                    .wrapping_add(state)
                    .wrapping_mul(52_845)
                    .wrapping_add(22_719);
                cipher
            })
            .collect()
    }

    fn encrypted_entry(prefix: &str, plain: &[u8], suffix: &str) -> Vec<u8> {
        let encrypted = charstring_encrypt(plain);
        let mut entry = format!("{prefix} {} RD ", encrypted.len()).into_bytes();
        entry.extend_from_slice(&encrypted);
        entry.extend_from_slice(suffix.as_bytes());
        entry
    }

    #[test]
    fn font_usage_trims_widths_and_unicode() {
        let mut font = make_embed_font(
            "Test".to_owned(),
            None,
            Some(&(0..=255).map(|c| format!("g{c}")).collect::<Vec<_>>()),
            0,
            255,
            (0..=255).collect(),
        );
        font.to_unicode = vec![
            (69, "E".to_owned()),
            (70, "F".to_owned()),
            (72, "H".to_owned()),
        ];
        let mut usage = [0_u64; 4];
        usage[1] = (1 << (70 - 64)) | (1 << (72 - 64));
        set_font_usage(&mut font, usage);
        assert_eq!((font.first_char, font.last_char), (70, 72));
        assert_eq!(font.widths, vec![70, 71, 72]);
        assert_eq!(
            font.to_unicode,
            vec![(70, "F".to_owned()), (72, "H".to_owned())]
        );
    }

    #[test]
    fn type1_trace_preserves_caller_stack_and_hint_replacement() {
        let subrs = HashMap::from([
            (3, vec![11]),
            (4, vec![140, 142, 12, 16, 12, 17, 10, 11]),
            (7, vec![139, 159, 1, 11]),
            (8, vec![139, 169, 3, 11]),
            (9, vec![11]),
        ]);
        let mut trace = CharStringTrace::new(&subrs);
        // Call the same wrapper with two different hint-subroutine indices.
        assert_eq!(
            trace.execute(&[146, 143, 10, 147, 143, 10, 14], 0),
            Some(true)
        );
        assert_eq!(trace.used_subrs, BTreeSet::from([3, 4, 7, 8]));
        assert!(trace.stack.is_empty());
    }

    #[test]
    fn type1_trace_preserves_return_values_and_fractional_division() {
        let subrs = HashMap::from([(0, vec![141, 11]), (2, vec![11])]);
        let mut trace = CharStringTrace::new(&subrs);
        assert_eq!(trace.execute(&[139, 10, 10, 14], 0), Some(true));
        assert_eq!(trace.used_subrs, BTreeSet::from([0, 2]));
        let mut trace = CharStringTrace::new(&subrs);
        // (1 / 2) / (1 / 4) = Subr 2, with no integer truncation.
        assert_eq!(
            trace.execute(&[140, 141, 12, 12, 140, 143, 12, 12, 12, 12, 10, 14], 0),
            Some(true)
        );
        assert_eq!(trace.used_subrs, BTreeSet::from([2]));
    }

    #[test]
    fn type1_trace_handles_flex_and_extended_seac() {
        let subrs = HashMap::from([(0, vec![142, 139, 12, 16, 12, 17, 12, 17, 12, 33, 11])]);
        let mut trace = CharStringTrace::new(&subrs);
        assert_eq!(trace.execute(&[189, 139, 139, 139, 10, 14], 0), Some(true));
        assert!(trace.stack.is_empty());
        // A composite using StandardEncoding AE (225) and acute (194).
        assert_eq!(
            trace.execute(&[139, 139, 139, 247, 117, 247, 86, 12, 6], 0),
            Some(true)
        );
        assert_eq!(trace.seac, BTreeSet::from(["AE", "acute"]));
    }

    #[test]
    fn type1_trace_rejects_unsupported_and_malformed_programs() {
        let subrs = HashMap::from([(0, vec![139, 10, 11])]);
        for program in [
            vec![139, 10, 14],          // recursive subroutine
            vec![140, 10, 14],          // missing subroutine
            vec![139, 143, 12, 16, 14], // custom OtherSubr
            vec![140, 139, 12, 12, 14], // division by zero
            vec![255, 1],               // truncated operand
            vec![12, 17, 10, 14],       // pop without OtherSubr results
        ] {
            assert_eq!(CharStringTrace::new(&subrs).execute(&program, 0), None);
        }
        let mut trace = CharStringTrace::new(&subrs);
        trace.budget = 0;
        assert_eq!(trace.execute(&[14], 0), None);
        assert_eq!(
            charstring_plain(&[139, 10, 14], -1),
            Some(vec![139, 10, 14])
        );
        assert_eq!(charstring_plain(&[14], -2), None);
        assert_eq!(charstring_plain(&[14], 4), None);
    }

    #[test]
    fn type1_subset_keeps_used_charstrings_and_reachable_subrs() {
        let mut private = vec![0, 0, 0, 0];
        private.extend_from_slice(b"/lenIV 4 def\n/Subrs 2 array\n");
        private.extend(encrypted_entry("dup 0", &[0, 0, 0, 0, 11], " NP\n"));
        private.extend(encrypted_entry("dup 1", &[0, 0, 0, 0, 11], " NP\n"));
        private.extend_from_slice(b"ND\n2 index /CharStrings 3 dict dup begin\n");
        private.extend(encrypted_entry("/.notdef", &[0, 0, 0, 0, 14], " ND\n"));
        private.extend(encrypted_entry("/A", &[0, 0, 0, 0, 139, 10, 14], " ND\n"));
        private.extend(encrypted_entry("/B", &[0, 0, 0, 0, 140, 10, 14], " ND\n"));
        private.extend_from_slice(b"end\n");

        let clear = b"%!PS-AdobeFont-1.0: Test 1.0\ncurrentfile eexec\n";
        let encrypted = eexec_encrypt(&private);
        let trailer = b"cleartomark\n";
        let mut program = clear.to_vec();
        program.extend_from_slice(&encrypted);
        program.extend_from_slice(trailer);
        let subset = subset_type1(
            &program,
            clear.len(),
            encrypted.len(),
            trailer.len(),
            &["A".to_owned()].into_iter().collect(),
        )
        .expect("synthetic Type 1 program should be subset");
        let decrypted =
            eexec_decrypt(&subset.data[subset.length1..subset.length1 + subset.length2]);
        assert!(find_bytes(&decrypted, b"/A ").is_some());
        assert!(find_bytes(&decrypted, b"/B ").is_none());
        assert!(find_bytes(&decrypted, b"dup 0 ").is_some());
        assert!(find_bytes(&decrypted, b"dup 1 ").is_none());
    }
}
