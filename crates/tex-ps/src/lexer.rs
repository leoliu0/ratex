//! PostScript / EPS lexer and binary header / DSC comment parser.

use crate::types::{EpsBoundingBox, PsValue};

/// Unwraps binary DOS EPS wrapper if present.
pub fn extract_ps_payload(input: &[u8]) -> &[u8] {
    if input.len() >= 30 && input.starts_with(&[0xC5, 0xD0, 0xD3, 0xC6]) {
        let ps_offset = u32::from_le_bytes([input[4], input[5], input[6], input[7]]) as usize;
        let ps_length = u32::from_le_bytes([input[8], input[9], input[10], input[11]]) as usize;
        if ps_offset < input.len() && ps_offset + ps_length <= input.len() {
            return &input[ps_offset..ps_offset + ps_length];
        }
    }
    input
}

/// Parses `%%BoundingBox:` and `%%HiResBoundingBox:` from EPS comments.
pub fn extract_bounding_box(input: &[u8]) -> Option<EpsBoundingBox> {
    let text = String::from_utf8_lossy(input);
    let mut regular_bbox = None;
    let mut hires_bbox = None;

    for line in text.lines().take(500) {
        let trimmed = line.trim();
        if trimmed.starts_with("%%HiResBoundingBox:") {
            let rest = trimmed.strip_prefix("%%HiResBoundingBox:").unwrap().trim();
            if rest != "(atend)" {
                if let Some(bb) = parse_bbox_numbers(rest) {
                    hires_bbox = Some(bb);
                }
            }
        } else if trimmed.starts_with("%%BoundingBox:") {
            let rest = trimmed.strip_prefix("%%BoundingBox:").unwrap().trim();
            if rest != "(atend)" {
                if let Some(bb) = parse_bbox_numbers(rest) {
                    regular_bbox = Some(bb);
                }
            }
        }
    }

    hires_bbox.or(regular_bbox)
}

fn parse_bbox_numbers(s: &str) -> Option<EpsBoundingBox> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() >= 4 {
        let llx = parts[0].parse::<f64>().ok()?;
        let lly = parts[1].parse::<f64>().ok()?;
        let urx = parts[2].parse::<f64>().ok()?;
        let ury = parts[3].parse::<f64>().ok()?;
        Some(EpsBoundingBox { llx, lly, urx, ury })
    } else {
        None
    }
}

/// Lexical tokens produced by the PostScript tokenizer.
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Value(PsValue),
    LBracket,  // [
    RBracket,  // ]
    LBrace,    // {
    RBrace,    // }
    LDict,     // <<
    RDict,     // >>
}

pub fn tokenize_ps(input: &[u8]) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let len = input.len();
    let mut i = 0;

    while i < len {
        let b = input[i];

        if b.is_ascii_whitespace() || b == 0 {
            i += 1;
            continue;
        }

        if b == b'%' {
            // Comment
            while i < len && input[i] != b'\n' && input[i] != b'\r' {
                i += 1;
            }
            continue;
        }

        if b == b'[' {
            tokens.push(Token::LBracket);
            i += 1;
            continue;
        }
        if b == b']' {
            tokens.push(Token::RBracket);
            i += 1;
            continue;
        }
        if b == b'{' {
            tokens.push(Token::LBrace);
            i += 1;
            continue;
        }
        if b == b'}' {
            tokens.push(Token::RBrace);
            i += 1;
            continue;
        }
        if b == b'<' {
            if i + 1 < len && input[i + 1] == b'<' {
                tokens.push(Token::LDict);
                i += 2;
                continue;
            }
            // Hex string <...>
            i += 1;
            let mut hex_bytes = Vec::new();
            while i < len && input[i] != b'>' {
                if !input[i].is_ascii_whitespace() {
                    hex_bytes.push(input[i]);
                }
                i += 1;
            }
            if i < len && input[i] == b'>' {
                i += 1;
            }
            let mut decoded = Vec::new();
            let mut j = 0;
            while j < hex_bytes.len() {
                let high = hex_val(hex_bytes[j]);
                let low = if j + 1 < hex_bytes.len() {
                    hex_val(hex_bytes[j + 1])
                } else {
                    0
                };
                decoded.push((high << 4) | low);
                j += 2;
            }
            tokens.push(Token::Value(PsValue::String(decoded)));
            continue;
        }
        if b == b'>' {
            if i + 1 < len && input[i + 1] == b'>' {
                tokens.push(Token::RDict);
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        if b == b'(' {
            // PostScript string (balanced parens and escapes)
            i += 1;
            let mut s = Vec::new();
            let mut depth = 1;
            while i < len && depth > 0 {
                let cb = input[i];
                if cb == b'\\' {
                    i += 1;
                    if i >= len {
                        break;
                    }
                    match input[i] {
                        b'n' => s.push(b'\n'),
                        b'r' => s.push(b'\r'),
                        b't' => s.push(b'\t'),
                        b'b' => s.push(0x08),
                        b'f' => s.push(0x0C),
                        b'\\' => s.push(b'\\'),
                        b'(' => s.push(b'('),
                        b')' => s.push(b')'),
                        d if d.is_ascii_digit() => {
                            // Octal escape \ooo
                            let mut oct = (d - b'0') as u8;
                            if i + 1 < len && input[i + 1].is_ascii_digit() {
                                i += 1;
                                oct = (oct << 3) | (input[i] - b'0');
                                if i + 1 < len && input[i + 1].is_ascii_digit() {
                                    i += 1;
                                    oct = (oct << 3) | (input[i] - b'0');
                                }
                            }
                            s.push(oct);
                        }
                        other => s.push(other),
                    }
                    i += 1;
                } else if cb == b'(' {
                    depth += 1;
                    s.push(cb);
                    i += 1;
                } else if cb == b')' {
                    depth -= 1;
                    if depth > 0 {
                        s.push(cb);
                    }
                    i += 1;
                } else {
                    s.push(cb);
                    i += 1;
                }
            }
            tokens.push(Token::Value(PsValue::String(s)));
            continue;
        }

        if b == b'/' {
            i += 1;
            // Check for immediate evaluation //name
            if i < len && input[i] == b'/' {
                i += 1;
            }
            let start = i;
            while i < len && !is_delimiter(input[i]) {
                i += 1;
            }
            let name = String::from_utf8_lossy(&input[start..i]).into_owned();
            tokens.push(Token::Value(PsValue::LiteralName(name)));
            continue;
        }

        // Token word: number, boolean, or executable name
        let start = i;
        while i < len && !is_delimiter(input[i]) {
            i += 1;
        }
        let word = String::from_utf8_lossy(&input[start..i]);

        if word == "true" {
            tokens.push(Token::Value(PsValue::Boolean(true)));
        } else if word == "false" {
            tokens.push(Token::Value(PsValue::Boolean(false)));
        } else if word == "null" {
            tokens.push(Token::Value(PsValue::Null));
        } else if let Some(val) = parse_number(&word) {
            tokens.push(Token::Value(val));
        } else {
            tokens.push(Token::Value(PsValue::ExecutableName(word.into_owned())));
        }
    }

    Ok(tokens)
}

fn parse_number(s: &str) -> Option<PsValue> {
    if let Some(pos) = s.find('#') {
        // Radix number: base#number
        let base: u32 = s[..pos].parse().ok()?;
        if (2..=36).contains(&base) {
            let n = i64::from_str_radix(&s[pos + 1..], base).ok()?;
            return Some(PsValue::Integer(n));
        }
    }

    if let Ok(i) = s.parse::<i64>() {
        return Some(PsValue::Integer(i));
    }
    if let Ok(f) = s.parse::<f64>() {
        return Some(PsValue::Real(f));
    }
    None
}

#[inline]
fn is_delimiter(b: u8) -> bool {
    b.is_ascii_whitespace()
        || b == 0
        || b == b'('
        || b == b')'
        || b == b'<'
        || b == b'>'
        || b == b'['
        || b == b']'
        || b == b'{'
        || b == b'}'
        || b == b'/'
        || b == b'%'
}

#[inline]
fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}
