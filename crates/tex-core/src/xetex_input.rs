//! XeTeX input: `\XeTeXinputencoding`, `\XeTeXdefaultencoding`,
//! `\XeTeXinputnormalization` and the decoding of file lines
//! (XeTeX_ext.c `u_open_in`, `get_uni_c`, `input_line`).
//!
//! A file's bytes are kept as they are; every line is decoded when TeX
//! reads it, so an encoding change takes effect from the next line.
//! Decoded lines are UTF-8 in the scanner's buffer.

use encoding_rs::Encoding;
use unicode_normalization::UnicodeNormalization;

use crate::engine::{Engine, EngineKind};
use crate::input::{physical_line_bounds, Source};
use crate::prim::IntParam;

/// How the bytes of one input file become characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enc {
    /// Not decided yet: the first line read resolves it from
    /// `\XeTeXdefaultencoding` (`auto` sniffs a byte-order mark).
    Default,
    Utf8,
    Utf16Be,
    Utf16Le,
    /// `bytes`: every byte is the character of that code.
    Raw,
    /// Any other encoding name (XeTeX asks ICU, TeXres asks `encoding_rs`).
    Icu(&'static Encoding),
}

/// The value of `\XeTeXdefaultencoding`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncSpec {
    Auto,
    Mode(Enc),
}

/// XeTeX_ext.c `getencodingmodeandinfo`: the built-in names, then any
/// converter name. `Err` means the name is unknown (XeTeX then reads raw
/// bytes).
pub(crate) fn parse_encoding_name(name: &str) -> Result<EncSpec, ()> {
    let lower = name.to_ascii_lowercase();
    Ok(match lower.as_str() {
        "auto" => EncSpec::Auto,
        "utf8" => EncSpec::Mode(Enc::Utf8),
        "utf16" => EncSpec::Mode(if cfg!(target_endian = "big") { Enc::Utf16Be } else { Enc::Utf16Le }),
        "utf16be" => EncSpec::Mode(Enc::Utf16Be),
        "utf16le" => EncSpec::Mode(Enc::Utf16Le),
        // ICU's ISO-8859-1 is the identity on bytes, unlike the WHATWG
        // "latin1" (windows-1252)
        "bytes" | "latin1" | "latin-1" | "iso-8859-1" | "iso8859-1" | "iso_8859-1" | "iso88591"
        | "l1" | "cp819" | "ibm819" | "ibm-819" => EncSpec::Mode(Enc::Raw),
        _ => match Encoding::for_label(name.as_bytes()) {
            Some(encoding) => EncSpec::Mode(Enc::Icu(encoding)),
            None => return Err(()),
        },
    })
}

/// `u_open_in` with mode `auto`: the form of the file, from its first bytes,
/// and how many bytes the byte-order mark takes.
fn sniff(data: &[u8]) -> (Enc, usize) {
    let b1 = data.first().copied();
    let b2 = data.get(1).copied();
    match (b1, b2) {
        (Some(0xFE), Some(0xFF)) => (Enc::Utf16Be, 2),
        (Some(0xFF), Some(0xFE)) => (Enc::Utf16Le, 2),
        (Some(0), Some(b)) if b != 0 => (Enc::Utf16Be, 0),
        (Some(b), Some(0)) if b != 0 => (Enc::Utf16Le, 0),
        (Some(0xEF), Some(0xBB)) if data.get(2) == Some(&0xBF) => (Enc::Utf8, 3),
        _ => (Enc::Utf8, 0),
    }
}

fn push_scalar(out: &mut Vec<u8>, scalar: u32) {
    let c = char::from_u32(scalar).unwrap_or('\u{FFFD}');
    let mut tmp = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
}

/// XeTeX_ext.c `get_uni_c` for UTF-8: the bytes of one line as scalars.
/// A malformed sequence is one U+FFFD; the byte that broke it is read
/// again. Returns whether anything was replaced.
fn decode_utf8(raw: &[u8], out: &mut Vec<u8>) -> bool {
    if let Ok(valid) = std::str::from_utf8(raw) {
        out.extend_from_slice(valid.as_bytes());
        return false;
    }
    const OFFSETS: [u32; 4] = [0, 0x3080, 0xE2080, 0x3C82080];
    let mut bad = false;
    let mut i = 0;
    while i < raw.len() {
        let first = raw[i];
        i += 1;
        let extra = match first {
            0..=0xBF => 0,
            0xC0..=0xDF => 1,
            0xE0..=0xEF => 2,
            0xF0..=0xF7 => 3,
            _ => {
                push_scalar(out, 0xFFFD);
                bad = true;
                continue;
            }
        };
        let mut value = u32::from(first);
        let mut complete = true;
        for _ in 0..extra {
            match raw.get(i) {
                Some(&c) if (0x80..0xC0).contains(&c) => {
                    value = (value << 6).wrapping_add(u32::from(c));
                    i += 1;
                }
                _ => {
                    complete = false;
                    break;
                }
            }
        }
        if !complete {
            push_scalar(out, 0xFFFD);
            bad = true;
            continue;
        }
        let value = value.wrapping_sub(OFFSETS[extra]);
        if value > 0x10FFFF {
            push_scalar(out, 0xFFFD);
            bad = true;
        } else {
            push_scalar(out, value);
        }
    }
    bad
}

/// One UTF-16 line starting at `start`: the characters, and the offset of
/// the next line (a CR LF pair ends a line like a single line feed).
fn utf16_line(data: &[u8], start: usize, big_endian: bool, out: &mut Vec<u8>) -> usize {
    let unit = |at: usize| -> Option<u32> {
        let pair = data.get(at..at + 2)?;
        Some(u32::from(if big_endian {
            u16::from_be_bytes([pair[0], pair[1]])
        } else {
            u16::from_le_bytes([pair[0], pair[1]])
        }))
    };
    let mut i = start;
    while let Some(u) = unit(i) {
        i += 2;
        match u {
            0x0A => return i,
            0x0D => {
                if unit(i) == Some(0x0A) {
                    i += 2;
                }
                return i;
            }
            0xD800..=0xDBFF => match unit(i) {
                Some(low @ 0xDC00..=0xDFFF) => {
                    i += 2;
                    push_scalar(out, 0x10000 + (u - 0xD800) * 0x400 + (low - 0xDC00));
                }
                _ => push_scalar(out, 0xFFFD),
            },
            0xDC00..=0xDFFF => push_scalar(out, 0xFFFD),
            _ => push_scalar(out, u),
        }
    }
    data.len()
}

impl Engine {
    /// `file_load_line` for XeTeX: decode the next line of the file at
    /// `si` per its encoding, normalize it (`\XeTeXinputnormalization`)
    /// and append `\endlinechar`. False at end of file.
    #[inline(never)]
    pub(crate) fn file_load_line_xetex(&mut self, si: usize, mut buf: Vec<u8>, end_line_char: i32) -> bool {
        debug_assert_eq!(self.engine_kind, EngineKind::XeTeX);
        let normalization = self.eqtb.int_params[IntParam::XeTeXInputNormalization.idx() as usize];
        let default = self.xetex_default_encoding;
        let Source::File { name, data, pos, line_no, line_start, state, xetex_enc, .. } =
            &mut self.input.stack[si]
        else {
            self.spare_line_buf = buf;
            return false;
        };
        if *xetex_enc == Enc::Default {
            let real = !name.starts_with('<') || name.starts_with("<embedded:");
            *xetex_enc = if !real {
                Enc::Utf8
            } else {
                match default {
                    EncSpec::Mode(mode) => mode,
                    EncSpec::Auto => {
                        let (mode, skip) = sniff(&data[*pos..]);
                        *pos += skip;
                        mode
                    }
                }
            };
        }
        let start = *pos;
        if start >= data.len() {
            self.spare_line_buf = buf;
            return false;
        }
        let mut text: Vec<u8> = Vec::new();
        let mut invalid = false;
        let next = match *xetex_enc {
            Enc::Utf16Be | Enc::Utf16Le => {
                utf16_line(data, start, *xetex_enc == Enc::Utf16Be, &mut text)
            }
            enc => {
                let (end, next) = physical_line_bounds(data, start);
                let raw = &data[start..end];
                match enc {
                    Enc::Raw => raw.iter().for_each(|&b| push_scalar(&mut text, u32::from(b))),
                    Enc::Icu(encoding) => {
                        let (decoded, _) = encoding.decode_without_bom_handling(raw);
                        text.extend_from_slice(decoded.as_bytes());
                    }
                    _ => invalid = decode_utf8(raw, &mut text),
                }
                next
            }
        };
        *line_start = start;
        *pos = next;
        *line_no += 1;
        *state = 0;
        let line = *line_no;
        if matches!(normalization, 1 | 2) && !text.is_ascii() {
            if let Ok(s) = String::from_utf8(std::mem::take(&mut text)) {
                text = if normalization == 1 {
                    s.nfc().collect::<String>().into_bytes()
                } else {
                    s.nfd().collect::<String>().into_bytes()
                };
            }
        }
        while text.last() == Some(&b' ') {
            text.pop();
        }
        buf.clear();
        buf.extend_from_slice(&text);
        let before = buf.len();
        if let Ok(character) = u8::try_from(end_line_char) {
            let mut encoded = [0u8; 4];
            buf.extend_from_slice(char::from(character).encode_utf8(&mut encoded).as_bytes());
        }
        let Source::File { line_buf, line_end_len, line_pos, .. } = &mut self.input.stack[si] else {
            return false;
        };
        *line_end_len = (buf.len() - before) as u8;
        *line_buf = Some(buf);
        *line_pos = 0;
        if invalid {
            // xetex.web `bad_utf8_warning`
            let message = format!("Invalid UTF-8 byte or sequence at line {line} replaced by U+FFFD.");
            let term = self.diagnostic_to_term();
            self.print_nl_diagnostic(&message, term);
        }
        true
    }

    /// `\XeTeXinputencoding <name>` (the current file) and
    /// `\XeTeXdefaultencoding <name>` (files opened from now on).
    pub(crate) fn do_xetex_encoding_command(&mut self, current_file: bool) {
        let name = self.scan_file_name();
        let spec = match parse_encoding_name(&name) {
            Ok(spec) => spec,
            Err(()) => {
                let message = format!("Unknown encoding `{name}'; reading as raw bytes");
                let term = self.diagnostic_to_term();
                self.print_nl_diagnostic(&message, term);
                EncSpec::Mode(Enc::Raw)
            }
        };
        if !current_file {
            self.xetex_default_encoding = spec;
            return;
        }
        match spec {
            EncSpec::Auto => {
                self.error("Encoding mode `auto' is not valid for \\XeTeXinputencoding");
            }
            EncSpec::Mode(mode) => {
                if let Some(Source::File { xetex_enc, .. }) = self
                    .input
                    .stack
                    .iter_mut()
                    .rev()
                    .find(|source| matches!(source, Source::File { .. }))
                {
                    *xetex_enc = mode;
                }
            }
        }
    }
}
