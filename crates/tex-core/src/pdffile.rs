//! Serialize a PdfDoc into PDF bytes: xref, catalog, page tree, content
//! streams (flate), Type 1 font embedding with encodings and widths, link
//! annotations, named destinations, and outlines.

use crate::pdf_fonts::{parse_metrics, parse_type1, Type1Metrics, Type1Program};
use crate::pdfout::{
    Annot, Dest, DestId, EmbedFont, EmbedFontSubtype, GotoTarget, Outline, PdfAction, PdfDoc,
};
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

/// Attach observed character usage and trim the PDF widths table to the
/// smallest required code interval. The full encoding vector remains
/// available to the Type 1 subsetter for code-to-glyph-name resolution.
fn set_font_usage(font: &mut EmbedFont, used_chars: [u64; 4]) {
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

/// A Type 1 program decoded once and shared by every engine font (size,
/// expansion step, code binding) that embeds it.
pub struct Type1Source {
    font_file: std::rc::Rc<Vec<u8>>,
    length1: usize,
    length2: usize,
    length3: usize,
    metrics: Type1Metrics,
    content_hash: [u8; 16],
    /// The program's own `/Encoding` array, if the cleartext declares one.
    pub builtin_encoding: Option<std::rc::Rc<[String]>>,
}

impl Type1Source {
    pub fn new(pfb: &[u8]) -> Self {
        let program = parse_type1(pfb);
        let cleartext = &program.data[..program.length1.min(program.data.len())];
        let metrics = parse_metrics(cleartext);
        let builtin_encoding = crate::pdf_fonts::builtin_encoding(cleartext).map(Into::into);
        Type1Source {
            content_hash: md5::compute(&program.data).0,
            font_file: std::rc::Rc::new(program.data),
            length1: program.length1,
            length2: program.length2,
            length3: program.length3,
            metrics,
            builtin_encoding,
        }
    }
}

/// Build an EmbedFont for the 256-slot code space from a Type 1 source,
/// encoding vector, TFM widths (1/10000 units per slot) and the codes used.
/// A missing source yields a font-dict-only entry (viewer substitutes by
/// BaseFont name); a missing encoding vector uses the font's built-in one.
pub fn make_embed_font(
    base_font: String,
    source: Option<&Type1Source>,
    encoding: Option<std::rc::Rc<[String]>>,
    widths_10000: Vec<i32>,
    used_chars: [u64; 4],
) -> EmbedFont {
    // no external encoding: adopt the font's own /Encoding array (if
    // the cleartext declares one) so extractors see accurate glyph names
    let encoding_diff =
        encoding.or_else(|| source.and_then(|source| source.builtin_encoding.clone()));
    // /ToUnicode (pdfTeX write_tounicode): every encoded slot with a known
    // Unicode value, ASCII identities included, whether used or not
    let to_unicode = encoding_diff
        .iter()
        .flat_map(|d| d.iter().take(256).enumerate())
        .filter_map(|(slot, g)| {
            if g.is_empty() {
                return None;
            }
            Some((slot as u8, crate::pdf_fonts::glyph_to_unicode(g)?))
        })
        .collect();
    let (font_file, length1, length2, length3, metrics, content_hash) = match source {
        Some(source) => (
            source.font_file.clone(),
            source.length1,
            source.length2,
            source.length3,
            source.metrics.clone(),
            source.content_hash,
        ),
        None => (
            std::rc::Rc::new(Vec::new()),
            0,
            0,
            0,
            Type1Metrics::default(),
            md5::compute(b"").0,
        ),
    };
    let mut font = EmbedFont {
        obj_font: 0,
        base_font,
        font_file,
        length1,
        length2,
        length3,
        is_truetype: false,
        subtype: EmbedFontSubtype::Type1,
        face_index: 0,
        variations: Vec::new(),
        allow_subsetting: true,
        content_hash,
        units_per_em: 1000,
        encoding_diff,
        first_char: 0,
        last_char: 255,
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
        is_cid: false,
        is_native: false,
        legacy_cids: Vec::new(),
        native_cids: Vec::new(),
        used_gids: std::collections::BTreeSet::new(),
        to_unicode_2byte: Vec::new(),
        font_attr: String::new(),
    };
    set_font_usage(&mut font, used_chars);
    font
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
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
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
            let k =
                self.s[(self.s[self.i as usize].wrapping_add(self.s[self.j as usize])) as usize];
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
    let key = compute_file_encryption_key(&config.user_password, &o, config.permissions, file_id);
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
        return Err(
            "PDF/A metadata validation failed: missing /OutputIntents in catalog".to_string(),
        );
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

/// Whether `body` can sit between `(` and `)` as one literal string:
/// unescaped parentheses balance and no lone backslash ends it.
fn literal_body_is_closed(body: &[u8]) -> bool {
    let mut depth = 0usize;
    let mut i = 0;
    while i < body.len() {
        match body[i] {
            b'\\' if i + 1 == body.len() => return false,
            b'\\' => i += 1,
            b'(' => depth += 1,
            b')' if depth == 0 => return false,
            b')' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    depth == 0
}

/// Escape the parentheses of `body` that have no partner and a trailing
/// lone backslash; everything else, escapes included, stays as written.
fn close_literal_body(body: &str) -> String {
    let b = body.as_bytes();
    let mut out = Vec::with_capacity(b.len() + 2);
    let mut open = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' if i + 1 == b.len() => out.extend_from_slice(b"\\\\"),
            b'\\' => {
                out.extend_from_slice(&b[i..i + 2]);
                i += 1;
            }
            b'(' => {
                open.push(out.len());
                out.push(b'(');
            }
            b')' if open.pop().is_some() => out.push(b')'),
            b')' => out.extend_from_slice(b"\\)"),
            c => out.push(c),
        }
        i += 1;
    }
    for at in open.into_iter().rev() {
        out.insert(at, b'\\');
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Decode a literal string body: escapes yield bytes (read as Latin-1,
/// close to PDFDocEncoding), other characters stand for themselves.
fn decode_literal_body(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(e) = chars.next() else { break };
        let byte = match e {
            'n' => b'\n',
            'r' => b'\r',
            't' => b'\t',
            'b' => 0x08,
            'f' => 0x0c,
            '0'..='7' => {
                let mut v = e as u32 - '0' as u32;
                for _ in 0..2 {
                    match chars.peek() {
                        Some(&d @ '0'..='7') => {
                            v = v * 8 + (d as u32 - '0' as u32);
                            chars.next();
                        }
                        _ => break,
                    }
                }
                v as u8
            }
            '\r' => {
                chars.next_if_eq(&'\n');
                continue;
            }
            '\n' => continue,
            other => {
                out.push(other);
                continue;
            }
        };
        out.push(char::from(byte));
    }
    out
}

/// PDF string as pdfTeX's `pdf_print_str` writes it: a string already in
/// `(...)` or even-length `<hex>` form is kept, anything else becomes the
/// body of a literal string as is, so escapes in the text (hyperref's
/// `\376\377\000S...` bookmark titles) keep their PDF meaning. Where
/// pdfTeX would corrupt the file, unpartnered parentheses and a trailing
/// lone backslash are escaped. Non-ASCII characters (a token list can hold
/// Unicode, unlike pdfTeX's byte strings) turn the decoded text into a
/// UTF-16BE text string.
fn pdftex_string(s: &str) -> String {
    let b = s.as_bytes();
    if let [b'<', hex @ .., b'>'] = b {
        if b.len().is_multiple_of(2) && hex.iter().all(u8::is_ascii_hexdigit) {
            return s.to_owned();
        }
    }
    let body = match b {
        [b'(', inner @ .., b')'] if literal_body_is_closed(inner) => &s[1..s.len() - 1],
        _ => s,
    };
    if s.is_ascii() {
        return if body.len() < s.len() {
            s.to_owned()
        } else {
            format!("({})", close_literal_body(body))
        };
    }
    let mut hex = String::from("<FEFF");
    for u in decode_literal_body(body).encode_utf16() {
        hex += &format!("{:04X}", u);
    }
    hex.push('>');
    hex
}

/// Explicit destination array for a page object.
fn dest_array(page_ref: usize, d: &Dest) -> String {
    let (x, y) = (num(d.x), num(d.y));
    let view = match d.kind {
        1 => "/Fit".to_string(),
        2 => format!("/FitH {y}"),
        3 => format!("/FitV {x}"),
        4 => "/FitB".to_string(),
        5 => format!("/FitBH {y}"),
        6 => format!("/FitBV {x}"),
        7 => format!("/FitR {x} {y} {x} {y}"),
        _ => format!(
            "/XYZ {x} {y} {}",
            d.zoom.map(num).unwrap_or_else(|| "null".to_string())
        ),
    };
    format!("[{page_ref} 0 R {view}]")
}

/// pdfTeX `write_action` as an inline dictionary (user actions verbatim).
/// `page_obj` maps a 0-based page index to its object. `None` when the
/// target has no object in this file: a missing page, or a local thread.
fn pdf_action(
    action: &PdfAction,
    page_obj: impl Fn(usize) -> Option<usize>,
    num_dests: &BTreeMap<i32, usize>,
) -> Option<String> {
    let (file, new_window, body) = match action {
        PdfAction::User(dict) => return Some(dict.clone()),
        PdfAction::Goto {
            file,
            new_window,
            target,
        } => {
            let body = match (target, file) {
                (GotoTarget::Page(page, view), None) => {
                    format!("/S /GoTo /D [{} 0 R {view}]", page_obj(*page as usize - 1)?)
                }
                (GotoTarget::Page(page, view), Some(_)) => {
                    format!("/S /GoToR /D [{} {view}]", page - 1)
                }
                (GotoTarget::Dest(DestId::Name(name)), None) => {
                    format!("/S /GoTo /D ({})", escape_string(name))
                }
                (GotoTarget::Dest(DestId::Name(name)), Some(_)) => {
                    format!("/S /GoToR /D ({})", escape_string(name))
                }
                (GotoTarget::Dest(DestId::Num(n)), None) => {
                    format!("/S /GoTo /D {} 0 R", num_dests.get(n)?)
                }
                // rejected while scanning, as in pdfTeX
                (GotoTarget::Dest(DestId::Num(_)), Some(_)) => return None,
            };
            (file, *new_window, body)
        }
        PdfAction::Thread { file, id } => {
            let d = match (id, file) {
                (DestId::Name(name), _) => format!("({})", escape_string(name)),
                (DestId::Num(n), Some(_)) => n.to_string(),
                (DestId::Num(_), None) => return None,
            };
            (file, None, format!("/S /Thread /D {d}"))
        }
    };
    let mut out = String::from("<< ");
    if let Some(file) = file {
        out.push_str(&format!("/F {} ", pdftex_string(file)));
        if let Some(new_window) = new_window {
            out.push_str(&format!("/NewWindow {new_window} "));
        }
    }
    out.push_str(&body);
    out.push_str(" >>");
    Some(out)
}

#[derive(Clone, Default)]
struct OutlineNode {
    parent: Option<usize>,
    prev: Option<usize>,
    next: Option<usize>,
    first: Option<usize>,
    last: Option<usize>,
    /// /Count: visible descendants when open, negated when closed
    count: i32,
}

/// \pdfoutline items linked into the PDF outline tree.
struct OutlineTree {
    nodes: Vec<OutlineNode>,
    /// top-level items in order
    top: Vec<usize>,
    /// root /Count: number of visible items
    root_count: i32,
}

impl OutlineTree {
    /// pdfTeX's linking: an item with `count n` (n != 0) adopts the next |n|
    /// items as children, recursively. Counts follow `open_subentries`: an
    /// item's magnitude is its child count plus the descendants of its
    /// open children; the sign of `n` marks it open (+) or closed (-).
    fn build(items: &[Outline]) -> Self {
        let mut nodes = vec![OutlineNode::default(); items.len()];
        let mut top = Vec::new();
        // open parents with the number of children still to adopt
        let mut stack: Vec<(usize, u32)> = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let parent = stack.last().map(|&(p, _)| p);
            let prev = match parent {
                Some(p) => nodes[p].last,
                None => top.last().copied(),
            };
            nodes[i].parent = parent;
            nodes[i].prev = prev;
            if let Some(prev) = prev {
                nodes[prev].next = Some(i);
            }
            match parent {
                Some(p) => {
                    nodes[p].first.get_or_insert(i);
                    nodes[p].last = Some(i);
                }
                None => top.push(i),
            }
            if let Some((_, left)) = stack.last_mut() {
                *left -= 1;
            }
            if item.count != 0 {
                stack.push((i, item.count.unsigned_abs()));
            }
            while stack.last().is_some_and(|&(_, left)| left == 0) {
                stack.pop();
            }
        }
        // children follow their parent, so a reverse pass sees every
        // subtree complete before its parent
        let mut entries = vec![0i32; items.len()];
        let mut root_count = 0;
        for i in (0..items.len()).rev() {
            let open = items[i].count > 0;
            nodes[i].count = if open { entries[i] } else { -entries[i] };
            let visible = 1 + if open { entries[i] } else { 0 };
            match nodes[i].parent {
                Some(p) => entries[p] += visible,
                None => root_count += visible,
            }
        }
        OutlineTree {
            nodes,
            top,
            root_count,
        }
    }
}

type FontFileKey = (usize, usize, usize, [u8; 16]);

struct PreparedType1 {
    program: Type1Program,
    pdf_name: String,
}

fn font_file_key(font: &EmbedFont) -> FontFileKey {
    (font.length1, font.length2, font.length3, font.content_hash)
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
    let mut hash = u64::from_le_bytes(key.3[..8].try_into().unwrap());
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
fn make_subset_tag(content_hash: &[u8; 16], base_font: &str) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for &b in content_hash {
        h = (h ^ b as u64).wrapping_mul(0x1000_0000_01b3);
    }
    for &b in base_font.as_bytes() {
        h = (h ^ b as u64).wrapping_mul(0x1000_0000_01b3);
    }
    let mut tag = String::with_capacity(7);
    for _ in 0..6 {
        let c = b'A' + (h % 26) as u8;
        tag.push(c as char);
        h /= 26;
    }
    tag.push('+');
    tag
}

fn extract_cff_table(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() >= 2 && bytes[0] == 1 && bytes[1] == 0 {
        return Some(bytes);
    }
    if bytes.len() < 12 {
        return None;
    }
    let num_tables = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    for i in 0..num_tables {
        let rec = 12 + i * 16;
        if rec + 16 > bytes.len() {
            break;
        }
        if &bytes[rec..rec + 4] == b"CFF " {
            let offset = u32::from_be_bytes([
                bytes[rec + 8],
                bytes[rec + 9],
                bytes[rec + 10],
                bytes[rec + 11],
            ]) as usize;
            let length = u32::from_be_bytes([
                bytes[rec + 12],
                bytes[rec + 13],
                bytes[rec + 14],
                bytes[rec + 15],
            ]) as usize;
            if offset + length <= bytes.len() {
                return Some(&bytes[offset..offset + length]);
            }
        }
    }
    None
}

fn to_encoding_cmap_1byte(entries: &[(u8, u16)]) -> String {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> def\n\
         /CMapName /Ratex-Legacy-Encoding def\n\
         /CMapType 1 def\n\
         1 begincodespacerange\n\
         <00> <FF>\n\
         endcodespacerange\n",
    );
    for chunk in entries.chunks(100) {
        s.push_str(&format!("{} begincidchar\n", chunk.len()));
        for &(slot, cid) in chunk {
            s.push_str(&format!("<{:02X}> {}\n", slot, cid));
        }
        s.push_str("endcidchar\n");
    }
    s.push_str(
        "endcmap\n\
         CMapName currentdict /CMap defineresource pop\n\
         end\n\
         end",
    );
    s
}

fn to_encoding_cmap_2byte(entries: &[(u16, u16)]) -> String {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> def\n\
         /CMapName /Ratex-Native-Encoding def\n\
         /CMapType 1 def\n\
         1 begincodespacerange\n\
         <0000> <FFFF>\n\
         endcodespacerange\n",
    );
    for chunk in entries.chunks(100) {
        s.push_str(&format!("{} begincidchar\n", chunk.len()));
        for &(code, cid) in chunk {
            s.push_str(&format!("<{:04X}> {}\n", code, cid));
        }
        s.push_str("endcidchar\n");
    }
    s.push_str(
        "endcmap\n\
         CMapName currentdict /CMap defineresource pop\n\
         end\n\
         end",
    );
    s
}

fn to_unicode_cmap_2byte(map: &[(u16, String)]) -> String {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n\
         /CMapType 2 def\n\
         1 begincodespacerange\n\
         <0000> <FFFF>\n\
         endcodespacerange\n",
    );
    for chunk in map.chunks(100) {
        s.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (code, uni) in chunk {
            let mut hex = String::new();
            for u in uni.encode_utf16() {
                use std::fmt::Write;
                let _ = write!(&mut hex, "{:04X}", u);
            }
            s.push_str(&format!("<{:04X}> <{}>\n", code, hex));
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

pub fn write_pdf(doc: &PdfDoc) -> Result<Vec<u8>, String> {
    let mut b = PdfBuilder::new();
    for (obj_num, obj_body) in &doc.objects {
        b.set_bytes(*obj_num as usize, obj_body.clone());
    }
    let catalog_obj = b.alloc(); // after user objects
    let pages_obj = b.alloc();

    // Collect destinations first (first definition wins): named ones go to
    // the sorted /Dests tree, numbered ones become standalone objects
    // referenced by `goto num` actions.
    let mut named: Vec<(&str, usize, &Dest)> = Vec::new();
    let mut numbered: BTreeMap<i32, (usize, &Dest)> = BTreeMap::new();
    for (pi, page) in doc.pages.iter().enumerate() {
        for dest in &page.dests {
            match &dest.id {
                DestId::Name(name) => {
                    if !named.iter().any(|(n, ..)| n == name) {
                        named.push((name, pi, dest));
                    }
                }
                DestId::Num(n) => {
                    numbered.entry(*n).or_insert((pi, dest));
                }
            }
        }
    }
    // pdfTeX replaces a referenced but undefined name by a fixed destination
    // on the first page, so the link still works.
    let fixed = Dest {
        id: DestId::Num(0),
        x: 0.0,
        y: 0.0,
        kind: 1,
        zoom: None,
    };
    for name in doc.unresolved_dest_names() {
        named.push((name, 0, &fixed));
    }
    named.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let names_obj = if named.is_empty() && doc.names_extra.is_empty() {
        0
    } else {
        b.alloc()
    };

    // Font objects: font dict, descriptor, optional font file,
    // optional /ToUnicode CMap, descendant CIDFont, and encoding CMap.
    struct FontObjs {
        font: usize,
        desc: usize,
        file: Option<usize>,
        tounicode: Option<usize>,
        cidfont: Option<usize>,
        encoding: Option<usize>,
    }

    #[derive(Clone, PartialEq, Eq, Hash, Debug)]
    struct SfntKey {
        content_hash: [u8; 16],
        face_index: u32,
        variations: Vec<(u32, u32)>,
    }

    struct PreparedSfnt {
        pdf_name: String,
        stream_data: Vec<u8>,
        is_cff: bool,
        remapper: subsetter::GlyphRemapper,
        cid_widths: Vec<i32>,
    }

    let is_sfnt = |f: &EmbedFont| f.is_cid || f.subtype != EmbedFontSubtype::Type1;

    let font_keys: Vec<_> = doc.fonts.iter().map(font_file_key).collect();

    // 1. Prepare Type 1 subsets
    let mut glyphs_by_file: HashMap<FontFileKey, Option<BTreeSet<String>>> = HashMap::new();
    for (font, &key) in doc.fonts.iter().zip(&font_keys) {
        if font.font_file.is_empty() || is_sfnt(font) {
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
    struct Type1Job<'a> {
        data: &'a [u8],
        name: &'a str,
        key: FontFileKey,
        glyphs: &'a Option<BTreeSet<String>>,
    }
    let jobs: Vec<_> = doc
        .fonts
        .iter()
        .zip(&font_keys)
        .filter_map(|(font, &key)| {
            if is_sfnt(font) || !attempted_files.insert(key) {
                return None;
            }
            let glyphs = glyphs_by_file.get(&key)?;
            Some(Type1Job {
                data: &font.font_file,
                name: &font.base_font,
                key,
                glyphs,
            })
        })
        .collect();
    let prepare = |job: &Type1Job<'_>| {
        let glyphs = job.glyphs.as_ref()?;
        let key = job.key;
        let mut subset = subset_type1(job.data, key.0, key.1, key.2, glyphs)?;
        let pdf_name = subset_font_name(job.name, key, glyphs);
        rename_type1_font(&mut subset, &pdf_name).then_some((
            key,
            PreparedType1 {
                program: subset,
                pdf_name,
            },
        ))
    };
    if !cfg!(target_arch = "wasm32") && jobs.len() >= 4 && !crate::debug_flag("TEX_PDF_SERIAL") {
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

    // 2. Prepare SFNT (TrueType & CFF) subsets
    let mut sfnt_glyphs_by_key: HashMap<SfntKey, BTreeSet<u16>> = HashMap::new();
    let mut sfnt_samples: HashMap<SfntKey, usize> = HashMap::new();

    for (idx, font) in doc.fonts.iter().enumerate() {
        if is_sfnt(font) {
            let key = SfntKey {
                content_hash: font.content_hash,
                face_index: font.face_index,
                variations: font
                    .variations
                    .iter()
                    .map(|(t, v)| (u32::from_be_bytes(t.to_bytes()), v.to_bits()))
                    .collect(),
            };
            sfnt_samples.entry(key.clone()).or_insert(idx);
            let gids = sfnt_glyphs_by_key.entry(key).or_default();
            gids.extend(&font.used_gids);
            for &(_, gid, _) in &font.legacy_cids {
                gids.insert(gid);
            }
            for &(_, gid, _) in &font.native_cids {
                gids.insert(gid);
            }
        }
    }

    let mut prepared_sfnts: HashMap<SfntKey, PreparedSfnt> = HashMap::new();
    for (key, sample_idx) in &sfnt_samples {
        let sample_font = &doc.fonts[*sample_idx];
        let all_used_gids = &sfnt_glyphs_by_key[key];
        let mut face = ttf_parser::Face::parse(&sample_font.font_file, sample_font.face_index)
            .map_err(|error| format!("Cannot embed font `{}`: {error:?}", sample_font.base_font))?;
        for &(tag, value) in &sample_font.variations {
            face.set_variation(tag, value).ok_or_else(|| {
                format!(
                    "Cannot instantiate font `{}` at {tag}={value}",
                    sample_font.base_font
                )
            })?;
        }
        let is_cff = face.tables().cff.is_some();
        let upem = face.units_per_em() as f64;
        let pdf_name = if sample_font.allow_subsetting {
            format!(
                "{}{}",
                make_subset_tag(&sample_font.content_hash, &sample_font.base_font),
                sample_font.base_font
            )
        } else {
            sample_font.base_font.clone()
        };

        let mut remapper = subsetter::GlyphRemapper::new();
        remapper.remap(0);
        let mut to_measure = BTreeSet::new();
        to_measure.insert(0);

        if !sample_font.allow_subsetting {
            let num_glyphs = face.number_of_glyphs();
            for gid in 0..num_glyphs {
                remapper.remap(gid);
                to_measure.insert(gid);
            }
        } else {
            for &gid in all_used_gids {
                if gid >= face.number_of_glyphs() {
                    return Err(format!(
                        "Font `{}` uses invalid glyph {gid}",
                        sample_font.base_font
                    ));
                }
                remapper.remap(gid);
                to_measure.insert(gid);
            }
        }

        let variations: Vec<_> = sample_font
            .variations
            .iter()
            .map(|(tag, value)| (subsetter::Tag::new(&tag.to_bytes()), *value))
            .collect();
        let subsetted = subsetter::subset_with_variations(
            &sample_font.font_file,
            sample_font.face_index,
            &variations,
            &remapper,
        )
        .map_err(|error| {
            format!(
                "Cannot prepare embedded font `{}`: {error}",
                sample_font.base_font
            )
        })?;

        let stream_data = if is_cff {
            extract_cff_table(&subsetted)
                .ok_or_else(|| {
                    format!("Font `{}` subset has no CFF program", sample_font.base_font)
                })?
                .to_vec()
        } else {
            subsetted
        };

        let mut max_cid = 0u16;
        for &gid in &to_measure {
            if let Some(cid) = remapper.get(gid) {
                if cid > max_cid {
                    max_cid = cid;
                }
            }
        }
        let mut cid_widths = vec![0i32; (max_cid as usize) + 1];
        for &gid in &to_measure {
            if let Some(cid) = remapper.get(gid) {
                let advance = face
                    .glyph_hor_advance(ttf_parser::GlyphId(gid))
                    .unwrap_or(0) as f64;
                cid_widths[cid as usize] = (advance * 1000.0 / upem).round() as i32;
            }
        }

        prepared_sfnts.insert(
            key.clone(),
            PreparedSfnt {
                pdf_name,
                stream_data,
                is_cff,
                remapper,
                cid_widths,
            },
        );
    }

    // Deduplicate font files and ToUnicode CMaps across font instances.
    let mut file_cache: HashMap<FontFileKey, usize> = HashMap::new();
    let mut sfnt_file_cache: HashMap<SfntKey, usize> = HashMap::new();
    let mut tounicode_cache: HashMap<Vec<(u8, String)>, usize> = HashMap::new();
    let mut tounicode_2byte_cache: HashMap<Vec<(u16, String)>, usize> = HashMap::new();

    let font_objs: Vec<FontObjs> = doc
        .fonts
        .iter()
        .zip(&font_keys)
        .map(|(f, &key)| {
            let font = b.alloc();
            let desc = b.alloc();
            let sfnt = is_sfnt(f);
            let cidfont = if sfnt { Some(b.alloc()) } else { None };
            let encoding = if sfnt { Some(b.alloc()) } else { None };
            let file = if f.font_file.is_empty() {
                None
            } else if sfnt {
                let sfnt_k = SfntKey {
                    content_hash: f.content_hash,
                    face_index: f.face_index,
                    variations: f
                        .variations
                        .iter()
                        .map(|(t, v)| (u32::from_be_bytes(t.to_bytes()), v.to_bits()))
                        .collect(),
                };
                Some(*sfnt_file_cache.entry(sfnt_k).or_insert_with(|| b.alloc()))
            } else {
                Some(*file_cache.entry(key).or_insert_with(|| b.alloc()))
            };
            let tounicode = if sfnt {
                if f.is_native {
                    if f.to_unicode_2byte.is_empty() {
                        None
                    } else {
                        Some(
                            *tounicode_2byte_cache
                                .entry(f.to_unicode_2byte.clone())
                                .or_insert_with(|| b.alloc()),
                        )
                    }
                } else {
                    if f.to_unicode.is_empty() {
                        None
                    } else {
                        Some(
                            *tounicode_cache
                                .entry(f.to_unicode.clone())
                                .or_insert_with(|| b.alloc()),
                        )
                    }
                }
            } else {
                if f.to_unicode.is_empty() {
                    None
                } else {
                    Some(
                        *tounicode_cache
                            .entry(f.to_unicode.clone())
                            .or_insert_with(|| b.alloc()),
                    )
                }
            };
            FontObjs {
                font,
                desc,
                file,
                tounicode,
                cidfont,
                encoding,
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

    // Numbered destinations, plus a fixed stand-in for every `goto num`
    // target that no \pdfdest defines (only possible with a page to point at).
    let mut num_dest_objs: BTreeMap<i32, usize> =
        numbered.keys().map(|&n| (n, b.alloc())).collect();
    if !page_objs.is_empty() {
        for item in &doc.outlines {
            if let Some(PdfAction::Goto {
                file: None,
                target: GotoTarget::Dest(DestId::Num(n)),
                ..
            }) = item.action
            {
                num_dest_objs.entry(n).or_insert_with(|| b.alloc());
            }
        }
    }

    // Outlines: root + one item per \pdfoutline.
    let outline_objs: Option<(usize, Vec<usize>)> = if doc.outlines.is_empty() {
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
    let mut emitted_encoding: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for ((f, fo), key) in doc.fonts.iter().zip(&font_objs).zip(&font_keys) {
        if is_sfnt(f) {
            let sfnt_k = SfntKey {
                content_hash: f.content_hash,
                face_index: f.face_index,
                variations: f
                    .variations
                    .iter()
                    .map(|(t, v)| (u32::from_be_bytes(t.to_bytes()), v.to_bits()))
                    .collect(),
            };
            let Some(prep) = prepared_sfnts.get(&sfnt_k) else {
                continue;
            };
            let (ascent, descent) = if f.ascent - f.descent > 3000.0 {
                (f.ascent, f.ascent - 3000.0)
            } else {
                (f.ascent, f.descent)
            };
            let mut desc = format!(
                "<< /Type /FontDescriptor /FontName /{} /Flags {} /FontBBox [{} {} {} {}] /ItalicAngle {} /Ascent {} /Descent {} /CapHeight {} /StemV {}",
                escape_pdf_name(&prep.pdf_name),
                f.flags,
                num(f.font_bbox[0]), num(f.font_bbox[1]), num(f.font_bbox[2]), num(f.font_bbox[3]),
                num(f.italic_angle), num(ascent), num(descent), num(f.cap_height), num(f.stem_v),
            );
            if let Some(file) = fo.file {
                let font_file_key = if prep.is_cff {
                    "/FontFile3"
                } else {
                    "/FontFile2"
                };
                desc.push_str(&format!(" {} {} 0 R", font_file_key, file));
            }
            desc.push_str(" >>");
            b.set(fo.desc, desc);

            if let Some(file) = fo.file {
                if emitted_files.insert(file) {
                    if prep.is_cff {
                        b.set_stream(file, "/Subtype /CIDFontType0C", &prep.stream_data, true);
                    } else {
                        b.set_stream(
                            file,
                            &format!("/Length1 {}", prep.stream_data.len()),
                            &prep.stream_data,
                            true,
                        );
                    }
                }
            }

            let cid_subtype = if prep.is_cff {
                "/CIDFontType0"
            } else {
                "/CIDFontType2"
            };
            let cid_to_gid = if prep.is_cff {
                ""
            } else {
                " /CIDToGIDMap /Identity"
            };
            let mut w_str = String::from("[ 0 [ ");
            for w in &prep.cid_widths {
                w_str.push_str(&w.to_string());
                w_str.push(' ');
            }
            w_str.push_str("] ]");
            b.set(
                fo.cidfont.unwrap(),
                format!(
                    "<< /Type /Font /Subtype {} /BaseFont /{} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {} 0 R{} /W {} >>",
                    cid_subtype, escape_pdf_name(&prep.pdf_name), fo.desc, cid_to_gid, w_str
                ),
            );

            let enc_obj = fo.encoding.unwrap();
            if emitted_encoding.insert(enc_obj) {
                let enc_cmap = if f.is_native {
                    let mut entries = Vec::new();
                    for &(code, gid, _) in &f.native_cids {
                        let cid = prep.remapper.get(gid).ok_or_else(|| {
                            format!(
                                "Font `{}` lost used glyph {gid} during subsetting",
                                f.base_font
                            )
                        })?;
                        entries.push((code, cid));
                    }
                    to_encoding_cmap_2byte(&entries)
                } else {
                    let mut entries = Vec::new();
                    for &(slot, gid, _) in &f.legacy_cids {
                        let cid = prep.remapper.get(gid).ok_or_else(|| {
                            format!(
                                "Font `{}` lost used glyph {gid} during subsetting",
                                f.base_font
                            )
                        })?;
                        entries.push((slot, cid));
                    }
                    to_encoding_cmap_1byte(&entries)
                };
                b.set_stream(enc_obj, "", enc_cmap.as_bytes(), true);
            }

            if let Some(to) = fo.tounicode {
                if emitted_tounicode.insert(to) {
                    let to_cmap = if f.is_native {
                        to_unicode_cmap_2byte(&f.to_unicode_2byte)
                    } else {
                        to_unicode_cmap(&f.to_unicode)
                    };
                    b.set_stream(to, "", to_cmap.as_bytes(), true);
                }
            }

            let to_ref = match fo.tounicode {
                Some(o) => format!(" /ToUnicode {} 0 R", o),
                None => String::new(),
            };
            b.set(
                fo.font,
                format!(
                    "<< /Type /Font /Subtype /Type0 /BaseFont /{} /Encoding {} 0 R /DescendantFonts [ {} 0 R ]{}{} >>",
                    escape_pdf_name(&prep.pdf_name), enc_obj, fo.cidfont.unwrap(), to_ref,
                    font_attr_entry(&f.font_attr)
                ),
            );
        } else {
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
                            || (slot <= u8::MAX as usize
                                && char_is_used(&f.used_chars, slot as u8)))
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
                    "<< /Type /Font /Subtype /Type1 /BaseFont /{} /FirstChar {} /LastChar {} /Widths {} /FontDescriptor {} 0 R{}{}{} >>",
                    escape_pdf_name(pdf_name), first, last, widths, fo.desc, enc, tounicode_ref,
                    font_attr_entry(&f.font_attr)
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
                    let dict = format!(
                        "/Length1 {} /Length2 {} /Length3 {}",
                        length1, length2, length3
                    );
                    b.set_stream(file, &dict, font_data, true);
                }
            }
            if let Some(to) = fo.tounicode {
                if emitted_tounicode.insert(to) {
                    b.set_stream(to, "", to_unicode_cmap(&f.to_unicode).as_bytes(), true);
                }
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
        let (forms, images) = painted_xobjects(&page.content);
        let mut xobj_entries = Vec::new();
        for (obj_num, bytes) in &doc.objects {
            if bytes.starts_with(b"<< /Type /XObject") {
                if forms.contains(obj_num) {
                    xobj_entries.push(format!("/Fm{} {} 0 R", obj_num, obj_num));
                }
                if images.contains(obj_num) {
                    xobj_entries.push(format!("/Im{} {} 0 R", obj_num, obj_num));
                }
            }
        }
        let xobj_str = if xobj_entries.is_empty() {
            String::new()
        } else {
            format!(" /XObject << {} >>", xobj_entries.join(" "))
        };
        // pdfTeX "Write out page object": pdf_print_mag_bp of the page size
        // in sp, omitted when \pdfpageattr supplies its own /MediaBox.
        let mut media_box = String::new();
        if !page_attr.contains("/MediaBox") {
            media_box.push_str(" /MediaBox [0 0 ");
            crate::pdfrender::push_bp_sp(&mut media_box, page.width_sp);
            media_box.push(' ');
            crate::pdfrender::push_bp_sp(&mut media_box, page.height_sp);
            media_box.push(']');
        }
        b.set(
            *page_obj,
            format!(
                "<< /Type /Page /Parent {} 0 R{} /Contents {} 0 R /Resources << /Font << {} >> /ProcSet [/PDF /Text]{}{} >>{}{} >>",
                pages_obj,
                media_box,
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

    // ---- emit destinations
    for (n, (pi, dest)) in &numbered {
        b.set(num_dest_objs[n], dest_array(page_objs[*pi].1, dest));
    }
    for (n, obj) in &num_dest_objs {
        if !numbered.contains_key(n) {
            // pdfTeX replaces a referenced but undefined `num` destination
            // by a fixed one on the first page
            b.set(*obj, format!("[{} 0 R /Fit]", page_objs[0].1));
        }
    }
    if names_obj != 0 {
        let mut body = String::from("<< ");
        if !named.is_empty() {
            body.push_str("/Names [ ");
            for (name, pi, dest) in &named {
                body.push_str(&format!(
                    "({}) {} ",
                    escape_string(name),
                    dest_array(page_objs[*pi].1, dest)
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
    if let Some((root, entries)) = &outline_objs {
        let tree = OutlineTree::build(&doc.outlines);
        let obj = |i: Option<usize>| i.map_or(*root, |i| entries[i]);
        for (i, (item, eobj)) in doc.outlines.iter().zip(entries).enumerate() {
            let node = &tree.nodes[i];
            let mut body = format!(
                "<< /Title {} /Parent {} 0 R",
                pdftex_string(&item.title),
                obj(node.parent)
            );
            if let Some(prev) = node.prev {
                body.push_str(&format!(" /Prev {} 0 R", entries[prev]));
            }
            if let Some(next) = node.next {
                body.push_str(&format!(" /Next {} 0 R", entries[next]));
            }
            if let (Some(first), Some(last)) = (node.first, node.last) {
                body.push_str(&format!(
                    " /First {} 0 R /Last {} 0 R",
                    entries[first], entries[last]
                ));
            }
            if node.count != 0 {
                body.push_str(&format!(" /Count {}", node.count));
            }
            if let Some(action) = item.action.as_ref().and_then(|action| {
                pdf_action(
                    action,
                    |page| page_objs.get(page).map(|p| p.1),
                    &num_dest_objs,
                )
            }) {
                body.push_str(&format!(" /A {}", action));
            }
            if !item.attr.is_empty() {
                body.push(' ');
                body.push_str(&item.attr);
            }
            body.push_str(" >>");
            b.set(*eobj, body);
        }
        let (first, last) = (tree.top[0], tree.top[tree.top.len() - 1]);
        b.set(
            *root,
            format!(
                "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {} >>",
                entries[first], entries[last], tree.root_count
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
        cat.push_str(&format!(" /Names << /Dests {} 0 R >>", names_obj));
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
    if let Some((root, _)) = &outline_objs {
        cat.push_str(&format!(" /Outlines {} 0 R", root));
    }
    let has_tagged_pdf = doc.pages.iter().any(|p| {
        p.display_list
            .as_ref()
            .is_some_and(|dl| dl.has_structure_tags())
    });
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
    Ok(if normalize {
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
    })
}

/// Object numbers a content stream paints as `/Fm<n> Do` (forms) and
/// `/Im<n> Do` (images), found in one pass over the stream.
fn painted_xobjects(content: &[u8]) -> (BTreeSet<i32>, BTreeSet<i32>) {
    let mut forms = BTreeSet::new();
    let mut images = BTreeSet::new();
    let mut at = 0;
    while let Some(offset) = content[at..].iter().position(|&byte| byte == b'/') {
        at += offset + 1;
        let set = match content.get(at..at + 2) {
            Some(b"Fm") => &mut forms,
            Some(b"Im") => &mut images,
            _ => continue,
        };
        let digits = &content[at + 2..];
        let len = digits.iter().take_while(|byte| byte.is_ascii_digit()).count();
        // `/Fm<n> Do` names use the decimal object number without padding.
        if len == 0 || (len > 1 && digits[0] == b'0') || !digits[len..].starts_with(b" Do") {
            continue;
        }
        if let Some(number) = std::str::from_utf8(&digits[..len]).ok().and_then(|n| n.parse().ok()) {
            set.insert(number);
        }
    }
    (forms, images)
}

/// `\pdffontattr` text as a font dictionary suffix (pdfTeX prints it after
/// /ToUnicode).
fn font_attr_entry(attr: &str) -> String {
    let attr = attr.trim();
    if attr.is_empty() {
        String::new()
    } else {
        format!(" {attr}")
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
        body.push_str(&format!(
            " /A << /S /GoTo /D ({}) >>",
            escape_string(dest)
        ));
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
    fn pdftex_strings_keep_escapes_and_stay_valid() {
        for (text, pdf) in [
            // hyperref's escaped UTF-16 titles are written verbatim
            (r"\376\377\000A\000\050", r"(\376\377\000A\000\050)"),
            ("(literal)", "(literal)"),
            ("<FEFF0041>", "<FEFF0041>"),
            ("<xyz>", "(<xyz>)"),
            ("", "()"),
            // unpartnered parentheses and a final backslash would end or
            // swallow the string: escape just those
            (r"a(b", r"(a\(b)"),
            (r"c)d\", r"(c\)d\\)"),
            ("(a) (b)", "((a) (b))"),
            (r"(x\)", r"(\(x\))"),
            // non-ASCII text: decoded escapes plus Unicode as UTF-16BE
            (r"Ü\101\(", "<FEFF00DC00410028>"),
        ] {
            assert_eq!(pdftex_string(text), pdf, "{text:?}");
        }
    }

    #[test]
    fn font_usage_trims_widths_but_maps_every_encoded_code() {
        let mut usage = [0_u64; 4];
        usage[1] = (1 << (70 - 64)) | (1 << (72 - 64));
        let font = make_embed_font(
            "Test".to_owned(),
            None,
            Some((0..=255).map(|c| if c == 65 { "A".into() } else { format!("uni{:04X}", 0x100 + c) }).collect()),
            (0..=255).collect(),
            usage,
        );
        assert_eq!((font.first_char, font.last_char), (70, 72));
        assert_eq!(font.widths, vec![70, 71, 72]);
        // pdfTeX's ToUnicode covers all 256 codes, ASCII identities included
        assert_eq!(font.to_unicode.len(), 256);
        assert_eq!(font.to_unicode[65], (65, "A".to_owned()));
        assert_eq!(font.to_unicode[70], (70, "\u{146}".to_owned()));
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
