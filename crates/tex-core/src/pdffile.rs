//! Serialize a PdfDoc into PDF bytes: xref, catalog, page tree, content
//! streams (flate), Type 1 font embedding with encodings and widths, link
//! annotations, named destinations, and outlines.

use crate::pdf_fonts::{parse_type1, Type1Keys, Type1Program};
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

/// `StandardEncoding`'s glyph name for `code` (writet1.c `standard_glyph_names`).
pub(crate) fn standard_encoding_name(code: i32) -> Option<&'static str> {
    let name = *crate::pdf_encodings::STANDARD.get(usize::try_from(code).ok()?)?;
    (!name.is_empty()).then_some(name)
}

/// A Type 1 program decoded once and shared by every engine font (size,
/// expansion step, code binding) that embeds it.
pub struct Type1Source {
    font_file: std::rc::Rc<Vec<u8>>,
    length1: usize,
    length2: usize,
    length3: usize,
    keys: std::rc::Rc<Type1Keys>,
    content_hash: [u8; 16],
    /// The program's own encoding (writet1.c `t1_builtin_enc`), if declared.
    pub builtin_encoding: Option<std::rc::Rc<[String]>>,
}

impl Type1Source {
    pub fn new(pfb: &[u8]) -> Self {
        let program = parse_type1(pfb);
        let scanned = crate::writet1::scan_type1(&program.data, program.length1);
        Type1Source {
            keys: std::rc::Rc::new(scanned.keys),
            content_hash: md5::compute(&program.data).0,
            font_file: std::rc::Rc::new(program.data),
            length1: program.length1,
            length2: program.length2,
            length3: program.length3,
            builtin_encoding: scanned.encoding.map(Into::into),
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
    // the cleartext declares one) so code-to-glyph lookups stay accurate
    let encoding_diff =
        encoding.or_else(|| source.and_then(|source| source.builtin_encoding.clone()));
    let (font_file, length1, length2, length3, keys, content_hash) = match source {
        Some(source) => (
            source.font_file.clone(),
            source.length1,
            source.length2,
            source.length3,
            source.keys.clone(),
            source.content_hash,
        ),
        None => (
            std::rc::Rc::new(Vec::new()),
            0,
            0,
            0,
            Default::default(),
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
        font_bbox: [0.0; 4],
        italic_angle: 0.0,
        ascent: 0.0,
        descent: 0.0,
        cap_height: 0.0,
        stem_v: 0.0,
        flags: 4,
        to_unicode: Vec::new(),
        used_chars: [0; 4],
        is_cid: false,
        is_native: false,
        legacy_cids: Vec::new(),
        native_cids: Vec::new(),
        used_gids: std::collections::BTreeSet::new(),
        to_unicode_2byte: Vec::new(),
        font_attr: String::new(),
        t1_preset: [0; crate::pdf_fonts::INT_KEYS_NUM],
        t1_keys: keys,
        init_order: 0,
        t1_slant: 0,
        t1_extend: 0,
        desc_obj: 0,
        pdftex: None,
        xe: None,
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
    /// Numbers the Lua callbacks chose for objects the writer must not
    /// hand out again.
    claimed: std::collections::HashSet<usize>,
}

impl PdfBuilder {
    fn new() -> Self {
        PdfBuilder {
            objs: Vec::new(),
            packable: Vec::new(),
            claimed: std::collections::HashSet::new(),
        }
    }

    fn alloc(&mut self) -> usize {
        if self.objs.len() >= crate::engine::MAX_PAGE_LIST {
            return self.objs.len().max(1);
        }
        loop {
            self.objs.push(None);
            if !self.claimed.contains(&self.objs.len()) || self.objs.len() >= crate::engine::MAX_PAGE_LIST {
                return self.objs.len();
            }
        }
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

/// pdfTeX `pdf_print_mag_bp`: a page coordinate (bp, on the sp raster)
/// scaled by \mag and printed by `pdf_print_bp` with `fixed_decimal_digits`.
/// `scale` is `(\mag, fixed_decimal_digits)`.
fn mag_bp(v: f64, scale: (i32, u32)) -> String {
    mag_bp_sp((v * crate::pdfrender::SP_PER_BP).round() as i64, scale)
}

fn mag_bp_sp(mut sp: i64, (mag, digits): (i32, u32)) -> String {
    if mag > 0 && mag != 1000 {
        sp = crate::pdfrender::round_xn_over_d(sp, mag as i64, 1000);
    }
    let mut out = String::new();
    crate::pdfrender::push_bp_sp(&mut out, sp, digits);
    out
}

/// Explicit destination array for a page object.
fn dest_array(page_ref: usize, d: &Dest, scale: (i32, u32)) -> String {
    let (x, y) = (mag_bp(d.x, scale), mag_bp(d.y, scale));
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

use crate::writet1::T1Transform;

/// The program, its lengths, and the map entry's slant and extension: one
/// writefont.c `fd_entry` (`fm->ff_name`, `fm_slant`, `fm_extend`).
type FontFileKey = (usize, usize, usize, [u8; 16], T1Transform);

struct PreparedType1 {
    program: Type1Program,
    pdf_name: String,
    /// The /CharSet of a subset.
    charset: Option<BTreeSet<String>>,
    /// The descriptor keys the writing pass left (`fd->font_dim`).
    keys: Type1Keys,
}

fn font_file_key(font: &EmbedFont, transform: T1Transform) -> FontFileKey {
    (font.length1, font.length2, font.length3, font.content_hash, transform)
}

/// Merge a font's glyph demand into its program's: a subset's glyphs are
/// unioned and any request for the whole program wins.
fn merge_glyph_demand(
    demand: &mut HashMap<FontFileKey, Option<BTreeSet<String>>>,
    key: FontFileKey,
    requested: Option<BTreeSet<String>>,
) {
    match demand.entry(key) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(requested);
        }
        std::collections::hash_map::Entry::Occupied(mut entry) => match (entry.get_mut(), requested) {
            (Some(current), Some(additional)) => current.extend(additional),
            (slot, None) => *slot = None,
            (None, Some(_)) => {}
        },
    }
}

/// writefont.c `fd->gl_tree` of one font: the glyph names of the used codes.
/// A map encoding contributes no `.notdef` (`mark_reenc_glyphs`); the
/// program's builtin encoding does (`t1_subset_ascii_part` takes over the
/// `tx_tree` codes as they are).
fn required_glyphs(font: &EmbedFont) -> Option<BTreeSet<String>> {
    if font.used_chars == [0; 4] || !font.allow_subsetting {
        return None;
    }
    let encoding = font.encoding_diff.as_ref()?;
    let builtin = font.pdftex.as_ref().is_some_and(|pdftex| pdftex.enc_file.is_none());
    let mut glyphs = BTreeSet::new();
    for code in 0..=u8::MAX {
        if char_is_used(&font.used_chars, code) {
            match encoding.get(code as usize).map(String::as_str) {
                Some("" | ".notdef") | None => {
                    if builtin {
                        glyphs.insert(".notdef".to_owned());
                    }
                }
                Some(glyph) => {
                    glyphs.insert(glyph.to_owned());
                }
            }
        }
    }
    Some(glyphs)
}

/// The descriptor's font name (writefont.c `fd->fontname`): the program's
/// own `/FontName`, else the map's PostScript name, followed by
/// `-Slant_n` and `-Extend_n` for a transformed map entry.
fn type1_font_name(font: &EmbedFont, transform: T1Transform) -> String {
    let mut name = font.t1_keys.font_name.clone().unwrap_or_else(|| font.base_font.clone());
    if transform.slant != 0 {
        name.push_str(&format!("-Slant_{}", transform.slant));
    }
    if transform.extend != 0 {
        name.push_str(&format!("-Extend_{}", transform.extend));
    }
    name
}

/// writefont.c `fix_fontmetrics`, `write_fontmetrics` and the
/// `/FontDescriptor` dictionary around them for a Type 1 program. A key
/// that is unset is omitted; the bounding box needs all four numbers.
fn type1_descriptor(
    name: &str,
    flags: i32,
    mut dims: [Option<i32>; crate::pdf_fonts::INT_KEYS_NUM],
    charset: Option<&BTreeSet<String>>,
    file: Option<usize>,
) -> String {
    use crate::pdf_fonts::{ASCENT_CODE, CAPHEIGHT_CODE, DESCENT_CODE, FONTBBOX1_CODE};
    let bbox = FONTBBOX1_CODE;
    let has_bbox = dims[bbox..bbox + 4].iter().all(Option::is_some);
    if has_bbox {
        dims[ASCENT_CODE] = dims[ASCENT_CODE].or(dims[bbox + 3]);
        dims[DESCENT_CODE] = dims[DESCENT_CODE].or(dims[bbox + 1]);
        dims[CAPHEIGHT_CODE] = dims[CAPHEIGHT_CODE].or(dims[bbox + 3]);
    }
    let mut desc = format!("<< /Type /FontDescriptor /FontName /{name} /Flags {flags}");
    if has_bbox {
        desc.push_str(&format!(
            " /FontBBox [{} {} {} {}]",
            dims[bbox].unwrap_or(0),
            dims[bbox + 1].unwrap_or(0),
            dims[bbox + 2].unwrap_or(0),
            dims[bbox + 3].unwrap_or(0),
        ));
    }
    for (k, key) in ["Ascent", "CapHeight", "Descent", "ItalicAngle", "StemV", "XHeight"]
        .into_iter()
        .enumerate()
    {
        if let Some(value) = dims[k] {
            desc.push_str(&format!(" /{key} {value}"));
        }
    }
    if let Some(file) = file {
        if let Some(charset) = charset {
            desc.push_str(" /CharSet (");
            for glyph in charset {
                desc.push('/');
                desc.push_str(glyph);
            }
            desc.push(')');
        }
        desc.push_str(&format!(" /FontFile {file} 0 R"));
    }
    desc.push_str(" >>");
    desc
}

/// writefont.c `write_fontfile`: the written program, or the program as
/// read when pdfTeX's pass could not process it.
fn set_type1_font_file(b: &mut PdfBuilder, file: usize, prepared: Option<&PreparedType1>, font: &EmbedFont) {
    let (data, length1, length2, length3) = match prepared {
        Some(prepared) => (
            &prepared.program.data[..],
            prepared.program.length1,
            prepared.program.length2,
            prepared.program.length3,
        ),
        None => (&font.font_file[..], font.length1, font.length2, font.length3),
    };
    let dict = format!("/Length1 {length1} /Length2 {length2} /Length3 {length3}");
    b.set_stream(file, &dict, data, true);
}

/// utils.c `make_subset_tag`: six letters from the MD5 of the sorted glyph
/// names, the font name and the collision round `round` (a C `int`).
fn pdftex_subset_tag(glyphs: &BTreeSet<String>, font_name: &str, round: i32) -> String {
    let mut md5 = md5::Context::new();
    for glyph in glyphs {
        md5.consume(glyph.as_bytes());
        md5.consume(b" ");
    }
    md5.consume(font_name.as_bytes());
    md5.consume(round.to_ne_bytes());
    let digest = md5.finalize().0;
    let mut a = [0i32; 6];
    a[0] = digest[..13].iter().map(|&d| i32::from(d)).sum();
    for i in 1..6 {
        a[i] = a[i - 1] - i32::from(digest[i - 1]) + i32::from(digest[(i + 12) % 16]);
    }
    a.iter().map(|&v| (b'A' + (v % 26) as u8) as char).collect()
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

/// pdfobj.c `write_string`: a literal string (a hex string when it is mostly unprintable).
fn pdf_literal_string(s: &[u8]) -> String {
    let printable = |c: u8| (32..=126).contains(&c);
    let nescc = s.iter().filter(|&&c| !printable(c)).count();
    if nescc > s.len() / 3 {
        let mut out = String::from("<");
        for &c in s {
            out.push_str(&format!("{c:02X}"));
        }
        out.push('>');
        return out;
    }
    let mut out = String::from("(");
    for &c in s {
        match c {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(c as char);
            }
            c if printable(c) => out.push(c as char),
            c => out.push_str(&format!("\\{c:03o}")),
        }
    }
    out.push(')');
    out
}

/// xdvipdfmx's Type0/CIDFont objects of a XeTeX native font
/// (`dpx_font::build_xe_font`).
fn write_xe_font(
    b: &mut PdfBuilder,
    doc: &PdfDoc,
    f: &EmbedFont,
    xe: &crate::pdfout::XeFont,
    font_obj: usize,
    tags: &mut BTreeSet<String>,
) -> Result<(), String> {
    // six letter subset tag from the program and the glyph set, unique per document
    let mut tag = make_subset_tag(&f.content_hash, &format!("{}{:?}{}", f.face_index, xe.used, xe.vertical));
    tag.pop();
    let mut salt = 0u32;
    while !tags.insert(tag.clone()) {
        salt += 1;
        tag = make_subset_tag(&f.content_hash, &format!("{salt}{}{:?}", f.face_index, xe.used));
        tag.pop();
    }
    let p = crate::dpx_font::build_xe_font(&f.font_file, f.face_index, xe.vertical, &xe.used, &tag)
        .map_err(|e| format!("Cannot embed native font `{}`: {e}", f.base_font))?;
    let file = b.alloc();
    b.set_stream(file, &p.file_dict, &p.file, true);
    let mut d = format!("<< /Type /FontDescriptor /FontName /{}", escape_pdf_name(&p.cid_name));
    for (k, v) in &p.descriptor {
        d.push_str(&format!(" /{k} {v}"));
    }
    d.push_str(&format!(" {} {} 0 R", p.file_key, file));
    if doc.major_version < 2 {
        let cidset = b.alloc();
        b.set_stream(cidset, "", &p.cidset, true);
        d.push_str(&format!(" /CIDSet {cidset} 0 R"));
    }
    d.push_str(" >>");
    let desc = b.alloc();
    b.set(desc, d);
    let cidfont = b.alloc();
    let mut c = format!(
        "<< /Type /Font /Subtype {} /BaseFont /{} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {desc} 0 R /DW {}",
        if p.cff { "/CIDFontType0" } else { "/CIDFontType2" },
        escape_pdf_name(&p.cid_name),
        crate::dpx_font::pdf_number(p.dw),
    );
    if !p.w.is_empty() {
        let w = b.alloc();
        b.set(w, crate::dpx_font::w_array_text(&p.w));
        c.push_str(&format!(" /W {w} 0 R"));
    }
    if let Some(dw2) = p.dw2 {
        c.push_str(&format!(" /DW2 [{} {}]", crate::dpx_font::pdf_number(dw2[0]), crate::dpx_font::pdf_number(dw2[1])));
    }
    if !p.w2.is_empty() {
        let w2 = b.alloc();
        let mut s = String::from("[");
        for e in &p.w2 {
            s.push_str(&format!(
                " {} {} {} {} {}",
                crate::dpx_font::pdf_number(e[0]),
                crate::dpx_font::pdf_number(e[1]),
                crate::dpx_font::pdf_number(e[2]),
                crate::dpx_font::pdf_number(e[3]),
                crate::dpx_font::pdf_number(e[4])
            ));
        }
        s.push_str(" ]");
        b.set(w2, s);
        c.push_str(&format!(" /W2 {w2} 0 R"));
    }
    if !p.cff {
        match &p.cid_to_gid_map {
            None => c.push_str(" /CIDToGIDMap /Identity"),
            Some(map) => {
                let m = b.alloc();
                b.set_stream(m, "", map, true);
                c.push_str(&format!(" /CIDToGIDMap {m} 0 R"));
            }
        }
    }
    c.push_str(" >>");
    b.set(cidfont, c);
    let mut t0 = format!(
        "<< /Type /Font /Subtype /Type0 /BaseFont /{} /Encoding /{} /DescendantFonts [ {cidfont} 0 R ]",
        escape_pdf_name(&p.type0_name),
        if xe.vertical { "Identity-V" } else { "Identity-H" },
    );
    if let Some(cmap) = &p.tounicode {
        let tu = b.alloc();
        b.set_stream(tu, "", cmap.as_bytes(), true);
        t0.push_str(&format!(" /ToUnicode {tu} 0 R"));
    }
    t0.push_str(" >>");
    b.set(font_obj, t0);
    Ok(())
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
         /CMapName /TeXres-Legacy-Encoding def\n\
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
         /CMapName /TeXres-Native-Encoding def\n\
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
    // Engine-reserved numbers (\pdfpageref pages, \pdffontobjnum fonts,
    // unused reservations) stay below the writer's own objects.
    let reserved = usize::try_from(doc.reserved_objects).unwrap_or(0);
    if reserved > b.objs.len() && reserved <= crate::engine::MAX_PAGE_LIST {
        b.objs.resize(reserved, None);
        b.packable.resize(reserved, false);
    }
    b.claimed = doc.fonts.iter().filter(|f| f.desc_obj > 0).map(|f| f.desc_obj as usize).collect();
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
    // optional /ToUnicode CMap, descendant CIDFont, and encoding CMap (for
    // Type 1 pdfTeX fonts: the shared /Encoding and the /Widths array).
    struct FontObjs {
        font: usize,
        desc: usize,
        file: Option<usize>,
        tounicode: Option<usize>,
        cidfont: Option<usize>,
        encoding: Option<usize>,
        widths: Option<usize>,
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

    let font_keys: Vec<_> = doc
        .fonts
        .iter()
        .map(|font| font_file_key(font, T1Transform { slant: font.t1_slant, extend: font.t1_extend }))
        .collect();
    // xdvipdfmx (`pdf_font_load_type1`) embeds a TFM font's Type 1 program as a Type1C CFF of
    // its own, one per font; a program it cannot read is a fatal error there.
    let mut xe_tags: BTreeSet<String> = BTreeSet::new();
    let mut xdpx_type1: HashMap<usize, crate::dpx_t1::Type1C> = HashMap::new();
    // pdfencoding.c: the fonts of one map encoding share its /Encoding and ToUnicode CMap, made
    // for the codes all of them use
    struct XdpxEncoding {
        glyphs: Vec<Option<String>>,
        used: [bool; 256],
        ps_name: String,
    }
    let mut xdpx_encodings: BTreeMap<String, XdpxEncoding> = BTreeMap::new();
    if doc.xdvipdfmx {
        for (index, font) in doc.fonts.iter().enumerate() {
            let (Some(pdftex), false, false) = (&font.pdftex, font.font_file.is_empty(), is_sfnt(font)) else {
                continue;
            };
            if !pdftex.source_is_pfb {
                return Err(format!(
                    "xdvipdfmx:fatal: Sorry, pfa format not supported; please convert the font to pfb, e.g., with t1binary. (font `{}`)",
                    pdftex.tfm_name,
                ));
            }
            let mut tag = make_subset_tag(&font.content_hash, &format!("{}{:?}", pdftex.tfm_name, font.used_chars));
            tag.pop();
            let mut salt = 0u32;
            while xe_tags.contains(&tag) {
                salt += 1;
                tag = make_subset_tag(&font.content_hash, &format!("{salt}{}{:?}", pdftex.tfm_name, font.used_chars));
                tag.pop();
            }
            // (a map encoding is `encoding_id >= 0`; else the program's own encoding)
            let encoding = font.encoding_diff.as_deref().filter(|_| pdftex.enc_file.is_some());
            let converted = crate::dpx_t1::type1_to_type1c(
                &font.font_file,
                font.length1,
                encoding,
                &font.used_chars,
                &tag,
            )
            .map_err(|e| format!("xdvipdfmx:fatal: {e} (font `{}`)", pdftex.tfm_name))?;
            xe_tags.insert(tag);
            if let Some(enc_file) = &pdftex.enc_file {
                let group = xdpx_encodings.entry(enc_file.clone()).or_insert_with(|| XdpxEncoding {
                    glyphs: converted.encoding.clone(),
                    used: [false; 256],
                    ps_name: pdftex.enc_ps_name.clone().unwrap_or_else(|| enc_file.clone()),
                });
                for (all, &one) in group.used.iter_mut().zip(&converted.used) {
                    *all |= one;
                }
            }
            xdpx_type1.insert(index, converted);
        }
    }
    // epdf.c `copyFont`: the map entries' programs that replaced the fonts
    // of included PDF files go through the same writing pass.
    let imported: Vec<EmbedFont> = doc
        .imported_fonts
        .iter()
        .map(|f| make_embed_font(f.base_font.clone(), Some(&f.program), None, Vec::new(), [0; 4]))
        .collect();
    let imported_keys: Vec<FontFileKey> = doc
        .imported_fonts
        .iter()
        .zip(&imported)
        .map(|(f, font)| font_file_key(font, T1Transform { slant: f.slant, extend: f.extend }))
        .collect();

    // 1. Prepare Type 1 subsets
    let mut glyphs_by_file: HashMap<FontFileKey, Option<BTreeSet<String>>> = HashMap::new();
    let mut all_glyph_files: std::collections::HashSet<FontFileKey> = std::collections::HashSet::new();
    for (index, (font, &key)) in doc.fonts.iter().zip(&font_keys).enumerate() {
        if font.font_file.is_empty() || is_sfnt(font) || xdpx_type1.contains_key(&index) {
            continue;
        }
        merge_glyph_demand(&mut glyphs_by_file, key, required_glyphs(font));
    }
    for (font, &key) in doc.imported_fonts.iter().zip(&imported_keys) {
        if font.all_glyphs && font.subsettable {
            all_glyph_files.insert(key);
        }
        merge_glyph_demand(&mut glyphs_by_file, key, font.subsettable.then(|| font.glyphs.clone()));
    }
    let mut prepared_files: HashMap<FontFileKey, PreparedType1> = HashMap::new();
    let mut attempted_files = BTreeSet::new();
    struct Type1Job<'a> {
        data: &'a [u8],
        key: FontFileKey,
        glyphs: Option<&'a BTreeSet<String>>,
        font_name: String,
        all_glyphs: bool,
        tag: Option<String>,
    }
    // utils.c make_subset_tag keeps the document's tags distinct
    let mut subset_tags = BTreeSet::new();
    let jobs: Vec<_> = doc
        .fonts
        .iter()
        .zip(&font_keys)
        .enumerate()
        .map(|(index, (font, key))| (font, key, xdpx_type1.contains_key(&index)))
        .chain(imported.iter().zip(&imported_keys).map(|(font, key)| (font, key, false)))
        .filter_map(|(font, &key, converted)| {
            if converted || is_sfnt(font) || font.font_file.is_empty() || !attempted_files.insert(key) {
                return None;
            }
            let glyphs = glyphs_by_file.get(&key)?.as_ref();
            let font_name = type1_font_name(font, key.4);
            let tag = match glyphs {
                Some(glyphs) => Some(
                    (0..)
                        .map(|round| pdftex_subset_tag(glyphs, &font_name, round))
                        .find(|tag| subset_tags.insert(tag.clone()))?,
                ),
                None => None,
            };
            Some(Type1Job {
                data: &font.font_file,
                key,
                glyphs,
                font_name,
                all_glyphs: all_glyph_files.contains(&key),
                tag,
            })
        })
        .collect();
    let prepare = |job: &Type1Job<'_>| {
        let key = job.key;
        let subset = job.glyphs.zip(job.tag.as_deref()).and_then(|(glyphs, tag)| {
            let request = crate::writet1::T1Subset { glyphs, tag, all_glyphs: job.all_glyphs };
            let written = crate::writet1::write_type1(job.data, key.0, key.1, key.4, Some(&request))?;
            Some((written, format!("{tag}+{}", job.font_name)))
        });
        // pdfTeX gives up on a program it cannot subset; texres embeds it whole
        let (written, pdf_name, subsetted) = match subset {
            Some((written, pdf_name)) => (written, pdf_name, true),
            None => (
                crate::writet1::write_type1(job.data, key.0, key.1, key.4, None)?,
                job.font_name.clone(),
                false,
            ),
        };
        let charset = subsetted.then_some(written.charset);
        Some((
            key,
            PreparedType1 {
                program: Type1Program {
                    length1: written.length1,
                    length2: written.length2,
                    length3: 0,
                    data: written.data,
                },
                pdf_name,
                charset,
                keys: written.keys,
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
        if is_sfnt(font) && font.xe.is_none() {
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
            for &(_, gid) in &font.native_cids {
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
    let mut pdftex_tounicode_cache: HashMap<std::rc::Rc<str>, usize> = HashMap::new();
    let mut desc_cache: HashMap<FontFileKey, usize> = HashMap::new();
    let mut encoding_cache: BTreeMap<&str, usize> = BTreeMap::new();
    // epdf.c `epdf_create_fontdescriptor`: the descriptor an included font
    // created is the one every font of that program, slant and extension
    // uses
    for (font, &key) in doc.imported_fonts.iter().zip(&imported_keys) {
        desc_cache.entry(key).or_insert(font.desc_obj as usize);
    }

    let font_objs: Vec<FontObjs> = doc
        .fonts
        .iter()
        .zip(&font_keys)
        .enumerate()
        .map(|(index, (f, &key))| {
            let font = if f.obj_font > 0 {
                f.obj_font as usize
            } else {
                b.alloc()
            };
            if f.xe.is_some() {
                return FontObjs { font, desc: 0, file: None, tounicode: None, cidfont: None, encoding: None, widths: None };
            }
            let sfnt = is_sfnt(f);
            // writefont.c: fonts of one Type 1 program share its descriptor
            let own_type1 = xdpx_type1.contains_key(&index);
            let desc = if sfnt || f.font_file.is_empty() || own_type1 {
                if f.desc_obj > 0 { f.desc_obj as usize } else { b.alloc() }
            } else {
                *desc_cache
                    .entry(key)
                    .or_insert_with(|| if f.desc_obj > 0 { f.desc_obj as usize } else { b.alloc() })
            };
            let cidfont = if sfnt { Some(b.alloc()) } else { None };
            let encoding = if own_type1 {
                None
            } else if sfnt {
                Some(b.alloc())
            } else {
                // writeenc.c: one /Encoding object per encoding file
                let enc_file = f.pdftex.as_ref().and_then(|pdftex| pdftex.enc_file.as_deref());
                enc_file.map(|name| *encoding_cache.entry(name).or_insert_with(|| b.alloc()))
            };
            let widths = (!sfnt && f.pdftex.is_some()).then(|| b.alloc());
            let file = if f.font_file.is_empty() {
                None
            } else if own_type1 {
                Some(b.alloc())
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
            let tounicode = if own_type1 {
                None
            } else if sfnt {
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
            } else if let Some(pdftex) = &f.pdftex {
                pdftex.tounicode.as_ref().map(|cmap| {
                    *pdftex_tounicode_cache.entry(cmap.clone()).or_insert_with(|| b.alloc())
                })
            } else if f.to_unicode.is_empty() {
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
                cidfont,
                encoding,
                widths,
            }
        })
        .collect();
    // epdf.c: a font file the document's own fonts do not write yet
    for key in &imported_keys {
        file_cache.entry(*key).or_insert_with(|| b.alloc());
    }
    for (object, fonts) in &doc.form_fonts {
        let mut dict = String::from("<<");
        for entry in font_resource_entries(fonts, &doc.resname_prefix, |index| {
            font_objs.get(index).map(|font| font.font)
        }) {
            dict.push(' ');
            dict.push_str(&entry);
        }
        dict.push_str(" >>");
        b.set(*object as usize, dict);
    }
    // Page objects: content stream, page dict, one object per annotation.
    let page_objs: Vec<(usize, usize, Vec<usize>)> = doc
        .pages
        .iter()
        .enumerate()
        .map(|(index, p)| {
            let content = b.alloc();
            let page = match doc.page_objnums.get(&index) {
                Some(&reserved) => reserved as usize,
                None => b.alloc(),
            };
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

    let info_obj = (!doc.omit_info_dict).then(|| b.alloc());
    // ---- emit fonts
    let mut emitted_files: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut emitted_tounicode: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut emitted_encoding: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut emitted_descriptors: std::collections::HashSet<usize> = std::collections::HashSet::new();
    // pdfTeX walks font objects newest first: the most recently initialized
    // font of a program creates (and presets) the shared descriptor
    let mut descriptor_owner: HashMap<FontFileKey, usize> = HashMap::new();
    for (index, (font, &key)) in doc.fonts.iter().zip(&font_keys).enumerate() {
        if is_sfnt(font) || font.font_file.is_empty() {
            continue;
        }
        let owner = descriptor_owner.entry(key).or_insert(index);
        if font.init_order > doc.fonts[*owner].init_order {
            *owner = index;
        }
    }
    // epdf.c created the descriptor of these programs while a page was shipped
    // out, before `do_pdf_font` reaches any document font at the end of the
    // job: no font presets it from its TFM, and its /StemV is the included
    // font's
    let mut import_created: HashMap<FontFileKey, i32> = HashMap::new();
    for (font, &key) in doc.imported_fonts.iter().zip(&imported_keys) {
        import_created.entry(key).or_insert(font.stem_v);
    }

    // pdfencoding.c `pdf_encoding_complete`: the /Encoding entry and the ToUnicode CMap (made on
    // first use) of each map encoding
    let mut xdpx_enc_entry: HashMap<String, String> = HashMap::new();
    let mut xdpx_enc_tu: HashMap<String, (Option<String>, Option<usize>)> = HashMap::new();
    for (enc_file, group) in &xdpx_encodings {
        let entry = match crate::dpx_t1::encoding_resource(&group.glyphs, &group.used, &|g| escape_pdf_name(g)) {
            crate::dpx_t1::EncodingResource::None => String::new(),
            crate::dpx_t1::EncodingResource::Name(name) => format!(" /Encoding /{name}"),
            crate::dpx_t1::EncodingResource::Dict(dict) => {
                let obj = b.alloc();
                b.set(obj, dict);
                format!(" /Encoding {obj} 0 R")
            }
        };
        xdpx_enc_entry.insert(enc_file.clone(), entry);
        let text = crate::dpx_t1::to_unicode_cmap(
            &group.glyphs,
            &group.used,
            &format!("{}-UTF16", group.ps_name),
        );
        xdpx_enc_tu.insert(enc_file.clone(), (text, None));
    }
    for (index, ((f, fo), key)) in doc.fonts.iter().zip(&font_objs).zip(&font_keys).enumerate() {
        if let Some(xe) = &f.xe {
            write_xe_font(&mut b, doc, f, xe, fo.font, &mut xe_tags)?;
            continue;
        }
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
                    for &(code, gid) in &f.native_cids {
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

            let to = fo.tounicode.filter(|_| !attr_defines_key(&f.font_attr, "ToUnicode"));
            if let Some(to) = to {
                if emitted_tounicode.insert(to) {
                    let to_cmap = if f.is_native {
                        to_unicode_cmap_2byte(&f.to_unicode_2byte)
                    } else {
                        to_unicode_cmap(&f.to_unicode)
                    };
                    b.set_stream(to, "", to_cmap.as_bytes(), true);
                }
            }

            let to_ref = match to {
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
        } else if let (Some(t1c), Some(pdftex)) = (xdpx_type1.get(&index), &f.pdftex) {
            // xdvipdfmx `pdf_font_load_type1`: /FontFile3 /Type1C, widths of the program
            let file = fo.file.expect("a converted font owns its program object");
            b.set_stream(file, "/Subtype /Type1C", &t1c.data, true);
            let n = crate::dpx_font::pdf_number;
            let name = escape_pdf_name(&t1c.base_font);
            let mut desc = format!(
                "<< /Type /FontDescriptor /CapHeight {} /Ascent {} /Descent {} /ItalicAngle {} /StemV {} /Flags {} /FontBBox [{} {} {} {}] /FontFile3 {file} 0 R",
                n(t1c.cap_height), n(t1c.ascent), n(t1c.descent), n(t1c.italic_angle), n(t1c.stem_v),
                t1c.flags, n(t1c.font_bbox[0]), n(t1c.font_bbox[1]), n(t1c.font_bbox[2]), n(t1c.font_bbox[3]),
            );
            if doc.major_version < 2 {
                desc.push_str(" /CharSet ");
                desc.push_str(&pdf_literal_string(t1c.charset.as_bytes()));
            }
            desc.push_str(&format!(" /FontName /{name} >>"));
            b.set(fo.desc, desc);
            let widths_obj = fo.widths.expect("pdfTeX fonts own a /Widths object");
            let widths: Vec<String> = t1c.widths.iter().map(|&w| n(w)).collect();
            b.set(widths_obj, format!("[{}]", widths.join(" ")));
            let to = if attr_defines_key(&f.font_attr, "ToUnicode") {
                String::new()
            } else if let Some(enc_file) = &pdftex.enc_file {
                // a map encoding's CMap is the encoding's, named after it
                let slot = xdpx_enc_tu.get_mut(enc_file).expect("every map encoding has a group");
                match &slot.0 {
                    Some(text) => {
                        let obj = *slot.1.get_or_insert_with(|| {
                            let obj = b.alloc();
                            b.set_stream(obj, "", text.as_bytes(), true);
                            obj
                        });
                        format!(" /ToUnicode {obj} 0 R")
                    }
                    None => String::new(),
                }
            } else {
                // `pdf_create_ToUnicode_CMap(fullname, enc_vec, usedchars)`: a built-in encoding's
                // CMap is named after the tagged font and made before missing glyphs are dropped
                let mut used = [false; 256];
                for (code, u) in used.iter_mut().enumerate() {
                    *u = char_is_used(&f.used_chars, code as u8);
                }
                match crate::dpx_t1::to_unicode_cmap(&t1c.encoding, &used, &format!("{}-UTF16", t1c.base_font)) {
                    Some(text) => {
                        let obj = b.alloc();
                        b.set_stream(obj, "", text.as_bytes(), true);
                        format!(" /ToUnicode {obj} 0 R")
                    }
                    None => String::new(),
                }
            };
            let encoding = pdftex
                .enc_file
                .as_ref()
                .map(|enc_file| xdpx_enc_entry[enc_file].clone())
                .unwrap_or_default();
            // a built-in encoding's ToUnicode is made by the font loader, a map encoding's later
            let (before, after) = if pdftex.enc_file.is_none() { (to, String::new()) } else { (String::new(), to) };
            b.set(
                fo.font,
                format!(
                    "<< /Type /Font /Subtype /Type1{before} /Widths {widths_obj} 0 R /FirstChar {} /LastChar {}{encoding}{after} /BaseFont /{name} /FontDescriptor {} 0 R{} >>",
                    t1c.first_char, t1c.last_char, fo.desc, font_attr_entry(&f.font_attr)
                ),
            );
        } else {
            let prepared = prepared_files.get(key);
            let fallback_name = type1_font_name(f, key.4);
            let pdf_name = prepared.map_or(fallback_name.as_str(), |font| font.pdf_name.as_str());
            if let Some(pdftex) = &f.pdftex {
                // writefont.c write_fontdictionary
                let widths_obj = fo.widths.expect("pdfTeX fonts own a /Widths object");
                let mut widths = String::from("[");
                for (k, &width) in f.widths.iter().enumerate() {
                    if k > 0 {
                        widths.push(' ');
                    }
                    // write_charwidth_array: C `int` division and remainder
                    widths.push_str(&(width / 10).to_string());
                    if width % 10 != 0 {
                        widths.push_str(&format!(".{}", width % 10));
                    }
                }
                widths.push(']');
                b.set(widths_obj, widths);
                let mut dict = format!(
                    "<< /Type /Font /Subtype /Type1 /BaseFont /{} /FontDescriptor {} 0 R /FirstChar {} /LastChar {} /Widths {} 0 R",
                    pdf_name, fo.desc, f.first_char, f.last_char, widths_obj
                );
                if let Some(encoding) = fo.encoding {
                    dict.push_str(&format!(" /Encoding {encoding} 0 R"));
                }
                if let Some(to) = fo.tounicode.filter(|_| !attr_defines_key(&f.font_attr, "ToUnicode")) {
                    dict.push_str(&format!(" /ToUnicode {to} 0 R"));
                    if emitted_tounicode.insert(to) {
                        let cmap = pdftex.tounicode.as_deref().unwrap_or_default();
                        b.set_stream(to, "", cmap.as_bytes(), true);
                    }
                }
                dict.push_str(&font_attr_entry(&f.font_attr));
                dict.push_str(" >>");
                b.set(fo.font, dict);
            } else {
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
                let to = fo.tounicode.filter(|_| !attr_defines_key(&f.font_attr, "ToUnicode"));
                let tounicode_ref = match to {
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
                if let Some(to) = to {
                    if emitted_tounicode.insert(to) {
                        b.set_stream(to, "", to_unicode_cmap(&f.to_unicode).as_bytes(), true);
                    }
                }
            }
            if emitted_descriptors.insert(fo.desc) {
                // writefont.c write_fontdescriptor: the TFM preset of the
                // newest-initialized font of this program, overridden by
                // the program's own keys (write_fontmetrics order)
                let owner = descriptor_owner.get(key).map_or(f, |&index| &doc.fonts[index]);
                let dims = std::array::from_fn(|k| {
                    f.t1_keys.dims[k].or_else(|| match import_created.get(key) {
                        Some(&stem_v) => (k == crate::pdf_fonts::STEMV_CODE).then_some(stem_v),
                        None => Some(owner.t1_preset[k]),
                    })
                });
                let charset = match prepared {
                    Some(font) => font.charset.as_ref(),
                    None => glyphs_by_file.get(key).and_then(Option::as_ref),
                };
                b.set(
                    fo.desc,
                    type1_descriptor(
                        pdf_name,
                        if fo.file.is_some() { 4 } else { f.flags },
                        dims,
                        charset.filter(|_| !doc.omit_charset),
                        fo.file,
                    ),
                );
            }
            if let Some(file) = fo.file {
                if emitted_files.insert(file) {
                    set_type1_font_file(&mut b, file, prepared, f);
                }
            }
        }
    }
    // epdf.c `copyFont`: descriptor, program and tagged /BaseFont name of
    // every font of an included PDF file that the map entry replaced
    for ((replaced, font), &key) in doc.imported_fonts.iter().zip(&imported).zip(&imported_keys) {
        let prepared = prepared_files.get(&key);
        let pdf_name =
            prepared.map_or_else(|| type1_font_name(font, key.4), |font| font.pdf_name.clone());
        if replaced.name_obj != 0 {
            b.set(replaced.name_obj as usize, format!("/{pdf_name}"));
        }
        let file = file_cache.get(&key).copied();
        if emitted_descriptors.insert(replaced.desc_obj as usize) {
            // create_fontdescriptor presets only /StemV, from the PDF
            let keys = prepared.map_or(&*font.t1_keys, |font| &font.keys);
            let mut dims = keys.dims;
            dims[crate::pdf_fonts::STEMV_CODE].get_or_insert(replaced.stem_v);
            let charset = match prepared {
                Some(font) => font.charset.as_ref(),
                None => glyphs_by_file.get(&key).and_then(Option::as_ref),
            };
            b.set(
                replaced.desc_obj as usize,
                type1_descriptor(&pdf_name, 4, dims, charset.filter(|_| !doc.omit_charset), file),
            );
        }
        if let Some(file) = file {
            if emitted_files.insert(file) {
                set_type1_font_file(&mut b, file, prepared, font);
            }
        }
    }
    // writeenc.c write_enc: each encoding file's object lists the codes
    // of every font using it, consecutive codes sharing one number
    for (&enc_file, &object) in &encoding_cache {
        let mut used = [0u64; 4];
        let mut names = None;
        for font in &doc.fonts {
            if font.pdftex.as_ref().and_then(|pdftex| pdftex.enc_file.as_deref()) == Some(enc_file) {
                for (word, font_word) in used.iter_mut().zip(font.used_chars) {
                    *word |= font_word;
                }
                names = names.or(font.encoding_diff.as_ref());
            }
        }
        let mut differences = String::new();
        let mut previous = -2;
        for code in (0..=u8::MAX).filter(|&code| char_is_used(&used, code)) {
            let name = names
                .and_then(|names| names.get(code as usize))
                .filter(|name| !name.is_empty())
                .map_or(".notdef", String::as_str);
            if i32::from(code) != previous + 1 {
                if previous != -2 {
                    differences.push(' ');
                }
                differences.push_str(&code.to_string());
            }
            differences.push('/');
            differences.push_str(name);
            previous = i32::from(code);
        }
        b.set(object, format!("<< /Type /Encoding /Differences [{differences}] >>"));
    }

    // ---- emit pages
    for (i, page) in doc.pages.iter().enumerate() {
        let (content_obj, page_obj, annot_objs) = &page_objs[i];
        if let Some(compressed) = doc.compressed_page(i) {
            b.set_encoded_stream(*content_obj, "", compressed, true);
        } else {
            b.set_stream(*content_obj, "", &page.content, true);
        }
        let fonts_res: String = font_resource_entries(&page.fonts, &doc.resname_prefix, |index| {
            font_objs.get(index).map(|fo| fo.font)
        })
        .iter()
        .map(|entry| format!("{entry} "))
        .collect();
        let mut annots_res = String::new();
        for (a, aobj) in page.annots.iter().zip(annot_objs) {
            emit_annot(&mut b, *aobj, a, (doc.mag, doc.decimal_digits));
            annots_res.push_str(&format!("{} 0 R ", aobj));
        }
        for r in &page.annot_refs {
            annots_res.push_str(&format!("{r} 0 R "));
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
        let xobj_entries = xobject_resource_entries(doc, &page.xforms, &page.ximages);
        let xobj_str = if xobj_entries.is_empty() {
            String::new()
        } else {
            format!(" /XObject << {} >>", xobj_entries.join(" "))
        };
        // pdfTeX "Write out page object": pdf_print_mag_bp of the page size
        // in sp, omitted when \pdfpageattr supplies its own /MediaBox.
        let mut media_box = String::new();
        if page.media_box && !page_attr.contains("/MediaBox") {
            media_box.push_str(" /MediaBox [0 0 ");
            media_box.push_str(&mag_bp_sp(page.width_sp, (doc.mag, doc.decimal_digits)));
            media_box.push(' ');
            media_box.push_str(&mag_bp_sp(page.height_sp, (doc.mag, doc.decimal_digits)));
            media_box.push(']');
        }
        // pdfTeX "Write out resources dictionary": additional resources,
        // fonts (when used), XObjects, then the ProcSet
        let mut resources = res_extra_str;
        if !fonts_res.is_empty() {
            resources.push_str(&format!(" /Font << {} >>", fonts_res));
        }
        resources.push_str(&xobj_str);
        if page.procset {
            resources.push_str(&procset_entry(!fonts_res.is_empty(), page.image_procset));
        }
        let group = if page.group > 0 {
            format!(" /Group {} 0 R", page.group)
        } else {
            String::new()
        };
        b.set(
            *page_obj,
            format!(
                "<< /Type /Page /Parent {} 0 R{} /Contents {} 0 R /Resources <<{} >>{}{}{} >>",
                pages_obj,
                media_box,
                content_obj,
                resources,
                if page_attr.trim().is_empty() {
                    String::new()
                } else {
                    format!(" {}", page_attr)
                },
                group,
                annots,
            ),
        );
    }

    // ---- emit the page tree node
    // LuaTeX `page_order_index`: pages sort by their location, stably
    let mut kid_order: Vec<usize> = (0..page_objs.len()).collect();
    if doc.page_order.iter().any(|&location| location != 0) {
        kid_order.sort_by_key(|&i| doc.page_order.get(i).copied().unwrap_or(0));
    }
    let kids: Vec<String> = kid_order
        .iter()
        .map(|&i| format!("{} 0 R", page_objs[i].1))
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
        b.set(
            num_dest_objs[n],
            dest_array(page_objs[*pi].1, dest, (doc.mag, doc.decimal_digits)),
        );
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
                    dest_array(page_objs[*pi].1, dest, (doc.mag, doc.decimal_digits))
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
    if let Some(info_obj) = info_obj {
        b.set(info_obj, info_dictionary(doc));
    }
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
                ctx.consume(b"texres-default-doc-id");
            }
            ctx.finalize().0
        });
        let (_enc_dict, dict_str) = generate_encryption_dictionary(enc_cfg, &fid);
        let enc_obj = b.alloc();
        b.set_bytes(enc_obj, dict_str.into_bytes());
        (Some(enc_obj), Some(fid))
    } else {
        (None, trailer_id(doc))
    };
    // LuaTeX's user supplied /ID array replaces the computed one
    let mut trailer_extra = doc.trailer_extra.clone();
    let file_id = if doc.trailer_id_raw.is_empty() || encrypt_obj.is_some() {
        file_id
    } else {
        trailer_extra.extend_from_slice(b" /ID ");
        trailer_extra.extend_from_slice(&doc.trailer_id_raw);
        None
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
            &trailer_extra,
            encrypt_obj,
            file_id,
            (doc.major_version, doc.minor_version.unwrap_or(5)),
        )
    } else {
        crate::pdfcompact::serialize(
            &b.objs,
            &b.packable,
            catalog_obj,
            info_obj,
            &trailer_extra,
            encrypt_obj,
            file_id,
            (doc.major_version, doc.minor_version.unwrap_or(5)),
        )
    })
}

/// pdfTeX "Generate font resources": `/F<ff><prefix> <obj> 0 R` per distinct
/// resource name, in first-use order. Fonts sharing a name (one TFM at
/// several sizes) share the dictionary, so only the first entry is kept.
fn font_resource_entries(
    fonts: &[(usize, u32)],
    prefix: &str,
    font_object: impl Fn(usize) -> Option<usize>,
) -> Vec<String> {
    let mut names: Vec<u32> = Vec::new();
    let mut entries = Vec::new();
    for &(index, number) in fonts {
        if names.contains(&number) {
            continue;
        }
        if let Some(object) = font_object(index) {
            names.push(number);
            entries.push(format!("/F{number}{prefix} {object} 0 R"));
        }
    }
    entries
}

/// pdfTeX "Generate XObject resources": the forms painted (`/Fm<n>`), then
/// the images (`/Im<n>`), each named by its creation count.
fn xobject_resource_entries(doc: &PdfDoc, xforms: &[i32], ximages: &[i32]) -> Vec<String> {
    let prefix = &doc.resname_prefix;
    let forms = xforms.iter().map(|&object| {
        let name = doc.form_names.get(&object).copied().unwrap_or(object);
        format!("/Fm{name}{prefix} {object} 0 R")
    });
    let images = ximages.iter().map(|&object| {
        let name = doc.image_names.get(&object).copied().unwrap_or(object);
        format!("/Im{name}{prefix} {object} 0 R")
    });
    forms.chain(images).collect()
}

/// pdftex.web `pdf_print_info`: /Producer unless the user's `\pdfinfo` gives
/// one, that text, then /Creator, /CreationDate, /ModDate and /Trapped
/// (each only when not given), and /PTEX.Fullbanner.
fn info_dictionary(doc: &PdfDoc) -> String {
    let user = String::from_utf8_lossy(&doc.info);
    let given = |key: &str| user.contains(key);
    let mut dict = String::from("<<\n");
    if !given("/Producer") {
        dict.push_str(&format!("/Producer ({})\n", doc.producer));
    }
    if !user.is_empty() {
        dict.push_str(&user);
        dict.push('\n');
    }
    if !given("/Creator") {
        dict.push_str("/Creator (TeX)\n");
    }
    if !doc.info_omit_date && !doc.start_time.is_empty() {
        for key in ["CreationDate", "ModDate"] {
            if doc.xdvipdfmx && key == "ModDate" {
                continue;
            }
            if !given(&format!("/{key}")) {
                dict.push_str(&format!("/{key} ({})\n", doc.start_time));
            }
        }
    }
    if !doc.xdvipdfmx && !given("/Trapped") {
        dict.push_str("/Trapped /False\n");
    }
    if let Some(key) = doc.ptex_banner_key.filter(|_| !doc.xdvipdfmx) {
        dict.push_str(&format!("/{key} ({})\n", escape_string(doc.banner)));
    }
    dict.push_str(">>");
    dict
}

/// pdftex.web "Output the trailer": `print_ID_alt` (the MD5 of the
/// `\pdftrailerid` text, nothing for empty text) or `print_ID` (the MD5 of the
/// start time and the output file name).
fn trailer_id(doc: &PdfDoc) -> Option<[u8; 16]> {
    match &doc.trailer_id_text {
        Some(text) if text.is_empty() => None,
        Some(text) => Some(md5::compute(text).0),
        None if doc.start_time.is_empty() => None,
        None => {
            let mut ctx = md5::Context::new();
            ctx.consume(doc.start_time.as_bytes());
            ctx.consume(doc.output_name.as_bytes());
            Some(ctx.finalize().0)
        }
    }
}

/// pdfTeX "Generate ProcSet if desired": /Text with fonts, /ImageB, /ImageC
/// and /ImageI for the image color types used (`pdf_image_procset`).
pub(crate) fn procset_entry(text: bool, images: u8) -> String {
    use crate::engine::{IMAGE_COLOR_B, IMAGE_COLOR_C, IMAGE_COLOR_I};
    let mut entry = String::from(" /ProcSet [ /PDF");
    for (used, name) in [
        (text, " /Text"),
        (images & IMAGE_COLOR_B != 0, " /ImageB"),
        (images & IMAGE_COLOR_C != 0, " /ImageC"),
        (images & IMAGE_COLOR_I != 0, " /ImageI"),
    ] {
        if used {
            entry.push_str(name);
        }
    }
    entry.push_str(" ]");
    entry
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

/// Whether a raw dictionary-entry text (`\pdffontattr`, `\pdfximage attr`)
/// defines `key` at its top level. Such a user entry replaces the entry the
/// writer would generate for that key: a dictionary must not repeat a key.
pub(crate) fn attr_defines_key(attr: &str, key: &str) -> bool {
    let bytes = attr.as_bytes();
    let is_delimiter = |b: u8| b.is_ascii_whitespace() || b"()<>[]{}/%".contains(&b);
    // Top-level objects of the text: (start, end) byte ranges.
    let mut items: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        match bytes[i] {
            b if b.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            b'%' => {
                while i < bytes.len() && !matches!(bytes[i], b'\n' | b'\r') {
                    i += 1;
                }
                continue;
            }
            b'/' => {
                i += 1;
                while i < bytes.len() && !is_delimiter(bytes[i]) {
                    i += 1;
                }
            }
            b'(' => {
                let mut depth = 0;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 1,
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
            }
            b'[' | b'<' if bytes[i] == b'[' || bytes.get(i + 1) == Some(&b'<') => {
                // array or dictionary: skip to the matching closer
                let mut depth = 0_i32;
                while i < bytes.len() {
                    match bytes[i] {
                        b'(' => {
                            let mut paren = 0;
                            while i < bytes.len() {
                                match bytes[i] {
                                    b'\\' => i += 1,
                                    b'(' => paren += 1,
                                    b')' => {
                                        paren -= 1;
                                        if paren == 0 {
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                                i += 1;
                            }
                        }
                        b'[' => depth += 1,
                        b'<' if bytes.get(i + 1) == Some(&b'<') => {
                            depth += 1;
                            i += 1;
                        }
                        b']' => depth -= 1,
                        b'>' if bytes.get(i + 1) == Some(&b'>') => {
                            depth -= 1;
                            i += 1;
                        }
                        _ => {}
                    }
                    i += 1;
                    if depth <= 0 {
                        break;
                    }
                }
            }
            b'<' => {
                while i < bytes.len() && bytes[i] != b'>' {
                    i += 1;
                }
                i += 1;
            }
            _ => {
                while i < bytes.len() && !is_delimiter(bytes[i]) {
                    i += 1;
                }
                if i == start {
                    i += 1;
                }
            }
        }
        items.push((start, i.min(bytes.len())));
    }
    let word = |item: usize| items.get(item).map(|&(s, e)| &bytes[s..e]);
    let is_number = |item: usize| word(item).is_some_and(|w| !w.is_empty() && w.iter().all(u8::is_ascii_digit));
    let wanted = key.as_bytes();
    let mut at = 0;
    while let Some(name) = word(at) {
        if name.first() == Some(&b'/') && &name[1..] == wanted {
            return true;
        }
        // key, then one value (an indirect reference is three objects)
        at += if is_number(at + 1) && is_number(at + 2) && word(at + 3) == Some(b"R") {
            4
        } else {
            2
        };
    }
    false
}

fn emit_annot(b: &mut PdfBuilder, obj: usize, a: &Annot, scale: (i32, u32)) {
    let [x0, y0, x1, y1] = a.rect;
    // pdfTeX writes only /Type /Annot (plus /Subtype /Link for links), the
    // rectangle and the user's attributes: no default /Border, and a
    // \pdfannot's own /Subtype is the only one.
    let mut body = String::from("<< /Type /Annot");
    if let Some(subtype) = &a.subtype {
        body.push_str(" /Subtype ");
        body.push_str(subtype);
    }
    body.push_str(&format!(
        " /Rect [{} {} {} {}]",
        mag_bp(x0, scale),
        mag_bp(y0, scale),
        mag_bp(x1, scale),
        mag_bp(y1, scale)
    ));
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

    #[test]
    fn attr_keys_are_found_at_top_level_only() {
        let attr = "/Group<</S/Transparency/K false>> /Other [ /ToUnicode ] /Mixed (a)/Ref 5 0 R /ToUnicode 7 0 R";
        assert!(attr_defines_key(attr, "Group"));
        assert!(attr_defines_key(attr, "Ref"));
        assert!(attr_defines_key(attr, "ToUnicode"));
        assert!(!attr_defines_key(attr, "S"));
        assert!(!attr_defines_key("/Other [ /ToUnicode ] /X (/ToUnicode)", "ToUnicode"));
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
    fn font_usage_trims_widths_but_keeps_the_encoding() {
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
        // the subsetter still resolves any code through the full vector
        let encoding = font.encoding_diff.as_ref().unwrap();
        assert_eq!((encoding.len(), encoding[65].as_str()), (256, "A"));
    }
}
