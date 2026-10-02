//! Byte-exact 8-bit text: TeX's `xprn` printability table, `^^` notation
//! and the lossless text encoding used to carry arbitrary TeX bytes through
//! `String`-based message plumbing into the transcript.
//!
//! TeX prints characters one byte at a time; which bytes appear verbatim and
//! which as `^^` notation depends on the translation table (`xprn`, tex.web
//! §49) that web2c fills from a TCX file and dumps into the format. Bytes
//! that are not valid UTF-8 cannot live in a `String`, so message text uses a
//! reversible encoding: a raw byte 0x80..=0xFF that is not part of a valid
//! UTF-8 sequence (or that would collide with the escape range) is stored as
//! the private-use character `U+F700 + byte`. [`text_to_bytes`] undoes it at
//! the files and streams that receive the transcript; everything that is
//! valid UTF-8 passes unchanged.

use std::borrow::Cow;

/// `xprn[k]`: whether byte `k` prints as itself (tex.web §49).
pub type Xprn = [bool; 256];

const RAW_BASE: u32 = 0xF700;
const RAW_FIRST: u32 = RAW_BASE + 0x80;
const RAW_LAST: u32 = RAW_BASE + 0xFF;

/// tex.web's own table: only 32..=126 print as themselves.
pub fn default_xprn() -> Xprn {
    let mut table = [false; 256];
    for entry in &mut table[32..127] {
        *entry = true;
    }
    table
}

/// TeX Live's `cp227.tcx`, the translation file every format except plain
/// TeX is built with: 128..=255 and HT, LF, VT print as themselves.
pub fn cp227_xprn() -> Xprn {
    let mut table = default_xprn();
    for entry in &mut table[128..] {
        *entry = true;
    }
    table[9] = true;
    table[10] = true;
    table[11] = true;
    table
}

/// XeTeX's printable character codes (observed with TeX Live 2026
/// `xetex -ini`): 32..126 and 160..255 print as themselves (as the Unicode
/// scalar of that value), the C0 and C1 controls use `^^` notation.
pub fn xetex_xprn() -> Xprn {
    let mut table = default_xprn();
    for entry in &mut table[160..] {
        *entry = true;
    }
    table[9] = true;
    table[10] = true;
    table
}

/// `-8bit`: every byte prints as itself.
pub fn eight_bit_xprn() -> Xprn {
    [true; 256]
}

/// tex.web §49: the `^^` spelling of a byte that does not print as itself.
fn push_caret(out: &mut Vec<u8>, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.extend_from_slice(b"^^");
    match byte {
        0..=63 => out.push(byte + 64),
        64..=127 => out.push(byte - 64),
        _ => out.extend_from_slice(&[HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 15)]]),
    }
}

/// Append `bytes` as TeX prints them: a byte `k` verbatim when `xprn[k]`,
/// else in `^^` notation.
pub fn push_printable(xprn: &Xprn, out: &mut Vec<u8>, bytes: &[u8]) {
    for &byte in bytes {
        if xprn[usize::from(byte)] {
            out.push(byte);
        } else {
            push_caret(out, byte);
        }
    }
}

fn is_escape_char(c: char) -> bool {
    (RAW_FIRST..=RAW_LAST).contains(&(c as u32))
}

fn push_raw(out: &mut String, byte: u8) {
    out.push(char::from_u32(RAW_BASE + u32::from(byte)).expect("private-use scalar"));
}

/// Whether `s` might contain an escaped raw byte: U+F780..U+F7FF encode as
/// EF 9E xx / EF 9F xx.
fn may_hold_escapes(s: &str) -> bool {
    s.as_bytes()
        .windows(2)
        .any(|pair| pair[0] == 0xEF && (pair[1] == 0x9E || pair[1] == 0x9F))
}

/// The text form of TeX bytes: valid UTF-8 unchanged, every other byte as an
/// escape character. Inverse of [`text_to_bytes`].
pub fn bytes_to_text(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        if !may_hold_escapes(text) {
            return text.to_owned();
        }
    }
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            if is_escape_char(c) {
                let mut buf = [0u8; 4];
                for &byte in c.encode_utf8(&mut buf).as_bytes() {
                    push_raw(&mut out, byte);
                }
            } else {
                out.push(c);
            }
        }
        for &byte in chunk.invalid() {
            push_raw(&mut out, byte);
        }
    }
    out
}

/// The bytes TeX wrote for `text` (inverse of [`bytes_to_text`]).
pub fn text_to_bytes(text: &str) -> Cow<'_, [u8]> {
    if !may_hold_escapes(text) {
        return Cow::Borrowed(text.as_bytes());
    }
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        if is_escape_char(c) {
            out.push((c as u32 - RAW_BASE) as u8);
        } else {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    Cow::Owned(out)
}

/// `text` for display on a UTF-8 stream: escaped bytes become U+FFFD, as
/// `String::from_utf8_lossy` would have produced.
pub fn text_to_display(text: &str) -> Cow<'_, str> {
    if !may_hold_escapes(text) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .map(|c| if is_escape_char(c) { char::REPLACEMENT_CHARACTER } else { c })
            .collect(),
    )
}

/// The file system path whose name is the bytes TeX wrote for `text`.
pub fn text_to_path(text: &str) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        std::path::PathBuf::from(std::ffi::OsString::from_vec(text_to_bytes(text).into_owned()))
    }
    #[cfg(not(unix))]
    {
        std::path::PathBuf::from(text_to_display(text).into_owned())
    }
}

/// TeX's count of the bytes `text` prints (`file_offset` accounting).
pub fn printed_len(text: &str) -> usize {
    if !may_hold_escapes(text) {
        return text.len();
    }
    text.chars()
        .map(|c| if is_escape_char(c) { 1 } else { c.len_utf8() })
        .sum()
}

/// A parsed TCX file (web2c `readtcxfile`): `xord`/`xchr` translate between
/// the external and the internal character code and `xprn` says which
/// internal codes print as themselves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tcx {
    pub xord: [u8; 256],
    pub xchr: [u8; 256],
    pub xprn: Xprn,
}

impl Tcx {
    /// The compiled-in tables: identity translation, tex.web printability.
    pub fn identity() -> Self {
        let mut identity = [0u8; 256];
        for (k, entry) in identity.iter_mut().enumerate() {
            *entry = k as u8;
        }
        Tcx { xord: identity, xchr: identity, xprn: default_xprn() }
    }

    pub fn is_identity(&self) -> bool {
        (0..256).all(|k| self.xord[k] == k as u8 && self.xchr[k] == k as u8)
    }

    /// TeX Live's own `cp227.tcx`, `cp8bit.tcx` and `empty.tcx`, which
    /// only change printability, available without a TeX tree.
    pub fn builtin(name: &str) -> Option<Self> {
        let mut tcx = Tcx::identity();
        match name {
            "cp227.tcx" => tcx.xprn = cp227_xprn(),
            "cp8bit.tcx" => tcx.xprn[128..].fill(true),
            "empty.tcx" => {}
            _ => return None,
        }
        Some(tcx)
    }

    /// Apply the lines of a TCX file on top of the compiled-in tables. Each
    /// line is `from [to [printable]]`; `%` starts a comment. Returns the
    /// offending line on a malformed entry, as web2c does.
    pub fn parse(text: &str) -> Result<Self, String> {
        fn number(word: &str) -> Option<u32> {
            let (digits, radix) = if let Some(hex) =
                word.strip_prefix("0x").or_else(|| word.strip_prefix("0X"))
            {
                (hex, 16)
            } else if word.len() > 1 && word.starts_with('0') {
                (&word[1..], 8)
            } else {
                (word, 10)
            };
            u32::from_str_radix(digits, radix).ok()
        }
        let mut tcx = Tcx::identity();
        for line in text.lines() {
            let content = line.split('%').next().unwrap_or("");
            let mut words = content.split_whitespace();
            let Some(first) = words.next() else { continue };
            let bad = || format!("bad TCX line `{}'", line.trim());
            let from = number(first).filter(|&v| v < 256).ok_or_else(bad)?;
            let to = match words.next() {
                Some(word) => number(word).filter(|&v| v < 256).ok_or_else(bad)?,
                None => from,
            };
            let printable = match words.next() {
                Some(word) => number(word).filter(|&v| v < 2).ok_or_else(bad)? == 1,
                None => true,
            };
            tcx.xord[from as usize] = to as u8;
            tcx.xchr[to as usize] = from as u8;
            tcx.xprn[to as usize] = printable;
        }
        Ok(tcx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_bytes_round_trip_through_text() {
        let samples: [&[u8]; 4] = [
            b"caf\xe9 \xff\x80",
            "caf\u{e9}".as_bytes(),
            b"\xef\x9e\x80 and \xef\x9f\xbf",
            b"mixed \xc3\xa9\xe9\xc3",
        ];
        for bytes in samples {
            let text = bytes_to_text(bytes);
            assert_eq!(text_to_bytes(&text).as_ref(), bytes);
        }
        assert_eq!(bytes_to_text("caf\u{e9}".as_bytes()), "caf\u{e9}");
    }

    #[test]
    fn caret_notation_follows_tex_web_49() {
        let mut out = Vec::new();
        push_printable(&default_xprn(), &mut out, b"\x01\t\x7f\x80\xffz@");
        assert_eq!(out, b"^^A^^I^^?^^80^^ffz@");
        let mut out = Vec::new();
        push_printable(&cp227_xprn(), &mut out, b"\x01\t\x0c\x7f\x80");
        assert_eq!(out, b"^^A\t^^L^^?\x80");
    }

    #[test]
    fn tcx_lines_set_printability_and_translation() {
        let tcx = Tcx::parse("0x80 0x80 % printable\n0x41 0x42 0\n\n0141\n").unwrap();
        assert!(tcx.xprn[0x80]);
        assert!(!tcx.xprn[0x42]);
        assert_eq!((tcx.xord[0x41], tcx.xchr[0x42]), (0x42, 0x41));
        assert!(tcx.xprn[0o141]);
        assert!(Tcx::parse("0x100\n").is_err());
    }
}
