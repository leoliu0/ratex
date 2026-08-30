//! Type 1 (PFB) font parsing for PDF embedding: splits the PFB container
//! into cleartext / encrypted / trailer segments (yielding the /Length1,
//! /Length2, /Length3 values the PDF /FontFile entry needs) and extracts
//! FontDescriptor metrics from the cleartext portion.

/// A Type 1 font program assembled from PFB segments (or a bare PFA).
#[derive(Debug, Clone)]
pub struct Type1Program {
    /// full program: cleartext + encrypted + trailer, concatenated
    pub data: Vec<u8>,
    /// bytes of the cleartext (ASCII) portion, through the `eexec` line
    pub length1: usize,
    /// bytes of the eexec-encrypted binary portion
    pub length2: usize,
    /// bytes of the trailer after the encrypted portion (usually 0)
    pub length3: usize,
}

/// Parse a PFB container: `0x80 0x01/0x02 <len:u32le> data ... 0x80 0x03`.
/// Returns None if the container structure is unrecognizable.
pub fn parse_pfb(pfb: &[u8]) -> Option<Type1Program> {
    if pfb.len() < 2 || pfb[0] != 0x80 {
        return None;
    }
    let mut out = Vec::with_capacity(pfb.len());
    let mut length1 = 0usize;
    let mut length2 = 0usize;
    let mut seg = 0u8; // segment ordinal: 0 = cleartext, 1 = encrypted
    let mut i = 0usize;
    while i < pfb.len() {
        if pfb[i] != 0x80 || i + 1 >= pfb.len() {
            return None;
        }
        match pfb[i + 1] {
            1 | 2 => {
                if i + 6 > pfb.len() {
                    return None;
                }
                let len =
                    u32::from_le_bytes([pfb[i + 2], pfb[i + 3], pfb[i + 4], pfb[i + 5]]) as usize;
                let start = i + 6;
                let end = start.saturating_add(len).min(pfb.len());
                if seg == 0 && pfb[i + 1] == 1 {
                    length1 = end - start;
                } else if seg == 1 && pfb[i + 1] == 2 {
                    length2 = end - start;
                }
                seg += 1;
                out.extend_from_slice(&pfb[start..end]);
                i = end;
            }
            3 => break,
            _ => return None,
        }
    }
    let length3 = out.len() - length1 - length2;
    Some(Type1Program { data: out, length1, length2, length3 })
}

/// Accept either a PFB container or a bare PFA/program; compute the
/// /Length1../Length3 values either way.
pub fn parse_type1(bytes: &[u8]) -> Type1Program {
    if let Some(p) = parse_pfb(bytes) {
        return p;
    }
    match eexec_end(bytes) {
        Some(n) => Type1Program { data: bytes.to_vec(), length1: n, length2: bytes.len() - n, length3: 0 },
        None => Type1Program { data: bytes.to_vec(), length1: bytes.len(), length2: 0, length3: 0 },
    }
}

/// Bytes through the end of the `eexec` line (cleartext portion length).
fn eexec_end(program: &[u8]) -> Option<usize> {
    let pos = find_sub(program, b"eexec")?;
    let mut end = pos + 5;
    while end < program.len()
        && matches!(program[end], b'\r' | b'\n' | b' ' | b'\t')
    {
        end += 1;
        if program[end - 1] == b'\n' {
            break;
        }
    }
    Some(end)
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// FontDescriptor metrics parsed from the cleartext of a Type 1 program.
/// All lengths in 1/1000 font units (as stored in the PFB); angles in degrees.
#[derive(Debug, Clone)]
pub struct Type1Metrics {
    pub font_bbox: [f64; 4],
    pub italic_angle: f64,
    pub ascent: f64,
    pub descent: f64,
    pub cap_height: f64,
    pub stem_v: f64,
}

impl Default for Type1Metrics {
    fn default() -> Self {
        Type1Metrics {
            font_bbox: [0.0; 4],
            italic_angle: 0.0,
            ascent: 0.0,
            descent: 0.0,
            cap_height: 0.0,
            stem_v: 0.0,
        }
    }
}

/// Extract /FontBBox, /ItalicAngle, /Ascent, /Descent, /CapHeight and
/// /StemV from the cleartext portion. Missing entries default to 0.
pub fn parse_metrics(cleartext: &[u8]) -> Type1Metrics {
    let text = String::from_utf8_lossy(cleartext);
    let mut m = Type1Metrics::default();
    if let Some(b) = parse_num_array(&text, "/FontBBox") {
        m.font_bbox = b;
    }
    m.italic_angle = parse_num_after(&text, "/ItalicAngle").unwrap_or(0.0);
    m.ascent = parse_num_after(&text, "/Ascent").unwrap_or(0.0);
    m.descent = parse_num_after(&text, "/Descent").unwrap_or(0.0);
    m.cap_height = parse_num_after(&text, "/CapHeight").unwrap_or(0.0);
    m.stem_v = parse_num_after(&text, "/StemV").unwrap_or(0.0);
    m
}

/// First whitespace-delimited token after `key` (skipping an opening brace).
fn token_after<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let p = text.find(key)?;
    let rest = &text[p + key.len()..];
    let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '{' || c == '[');
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '}' || c == ']' || c == ';' || c == '[')
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

fn parse_num_after(text: &str, key: &str) -> Option<f64> {
    token_after(text, key)?.parse().ok()
}

fn parse_num_array(text: &str, key: &str) -> Option<[f64; 4]> {
    let p = text.find(key)?;
    let rest = &text[p + key.len()..];
    let open = rest.find(['{', '['])? + 1;
    let close = rest[open..].find(['}', ']'])? + open;
    let mut it = rest[open..close].split_whitespace();
    let mut out = [0.0f64; 4];
    for slot in &mut out {
        *slot = it.next()?.parse().ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pfb_roundtrip_lengths() {
        // container: ascii "abc", binary "xy", EOF
        let mut pfb = vec![0x80, 1, 3, 0, 0, 0];
        pfb.extend_from_slice(b"abc");
        pfb.extend_from_slice(&[0x80, 2, 2, 0, 0, 0]);
        pfb.extend_from_slice(b"xy");
        pfb.extend_from_slice(&[0x80, 3]);
        let p = parse_pfb(&pfb).unwrap();
        assert_eq!(&p.data, b"abcxy");
        assert_eq!(p.length1, 3);
        assert_eq!(p.length2, 2);
        assert_eq!(p.length3, 0);
    }

    #[test]
    fn metrics_from_cleartext() {
        let clear = b"/FontMatrix [0.001 0 0 0.001 0 0 ] readonly def\n\
                      /FontBBox {-180 -293 1340 1014 } readonly def\n\
                      /ItalicAngle 0 def /Ascent 694 def /Descent -194 def\n\
                      /CapHeight 683 def /StemV 90 def\n";
        let m = parse_metrics(clear);
        assert_eq!(m.font_bbox, [-180.0, -293.0, 1340.0, 1014.0]);
        assert_eq!(m.ascent, 694.0);
        assert_eq!(m.descent, -194.0);
        assert_eq!(m.stem_v, 90.0);
    }
}

/// Descriptor metrics derived from TFM when the PFB cleartext lacks them:
/// (ascent, descent, cap_height, stem_v) in 1/1000 font units. Ascent is
/// the tallest character, descent the deepest (negated), CapHeight the
/// height of `H`, StemV a quarter of `I`'s width (clamped to >= 30).
pub fn tfm_descriptor(font: &crate::tfm::Font) -> (f64, f64, f64, f64) {
    if font.at_size == 0 {
        return (0.0, 0.0, 0.0, 90.0);
    }
    let scale = |v: i32| -> f64 { v as f64 * 1000.0 / font.at_size as f64 };
    let mut max_h = 0i32;
    let mut max_d = 0i32;
    for c in 0..=255u8 {
        max_h = max_h.max(font.char_height(c));
        max_d = max_d.max(font.char_depth(c));
    }
    let cap = font.char_height(b'H');
    let stem = (scale(font.char_width(b'I')) / 4.0).max(30.0);
    (
        scale(max_h),
        -scale(max_d),
        if cap > 0 { scale(cap) } else { 0.0 },
        stem,
    )
}

/// Glyph names declared by the font's own `/Encoding 256 array` in the
/// cleartext (the `dup <code> /<Name> put` sequence). Returns None when
/// the cleartext declares no encoding, in which case the viewing
/// application falls back to the font's built-in one.
pub fn builtin_encoding(cleartext: &[u8]) -> Option<Vec<String>> {
    let text = String::from_utf8_lossy(cleartext);
    let p = text.find("/Encoding")?;
    let body = &text[p..text.len().min(p + 16384)];
    let mut names: Vec<(usize, String)> = Vec::new();
    let mut from = 0usize;
    while let Some(dup) = body[from..].find("dup") {
        let at = from + dup + 3;
        let rest = &body[at..];
        let num_end = rest
            .find(|c: char| !c.is_ascii_digit() && c != ' ')
            .unwrap_or(rest.len());
        let code: usize = rest[..num_end].trim().parse().ok()?;
        let after = &rest[num_end..];
        let slash = after.find('/')?;
        let name_end = after[slash + 1..]
            .find(|c: char| c.is_whitespace())
            .map(|e| slash + 1 + e)
            .unwrap_or(after.len());
        if code < 256 {
            names.push((code, after[slash + 1..name_end].to_string()));
        }
        from = at + num_end + name_end;
    }
    if names.is_empty() {
        return None;
    }
    let mut out = vec![String::new(); 256];
    for (code, name) in names {
        out[code] = name;
    }
    Some(out)
}
