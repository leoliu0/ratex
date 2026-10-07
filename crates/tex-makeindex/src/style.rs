//! Index style (`.ist`) files: defaults and the style-file scanner.

/// Every keyword makeindex understands, with TeX Live's defaults.
#[derive(Clone, Debug)]
pub struct Style {
    pub keyword: Vec<u8>,
    pub arg_open: u8,
    pub arg_close: u8,
    pub range_open: u8,
    pub range_close: u8,
    pub level: u8,
    pub actual: u8,
    pub encap: u8,
    pub quote: u8,
    pub escape: u8,
    pub page_compositor: Vec<u8>,
    pub preamble: Vec<u8>,
    pub postamble: Vec<u8>,
    pub setpage_prefix: Vec<u8>,
    pub setpage_suffix: Vec<u8>,
    pub group_skip: Vec<u8>,
    pub headings_flag: i32,
    pub heading_prefix: Vec<u8>,
    pub heading_suffix: Vec<u8>,
    pub symhead_positive: Vec<u8>,
    pub symhead_negative: Vec<u8>,
    pub numhead_positive: Vec<u8>,
    pub numhead_negative: Vec<u8>,
    pub item_0: Vec<u8>,
    pub item_1: Vec<u8>,
    pub item_2: Vec<u8>,
    pub item_01: Vec<u8>,
    pub item_x1: Vec<u8>,
    pub item_12: Vec<u8>,
    pub item_x2: Vec<u8>,
    pub delim_0: Vec<u8>,
    pub delim_1: Vec<u8>,
    pub delim_2: Vec<u8>,
    pub delim_n: Vec<u8>,
    pub delim_r: Vec<u8>,
    pub delim_t: Vec<u8>,
    pub encap_prefix: Vec<u8>,
    pub encap_infix: Vec<u8>,
    pub encap_suffix: Vec<u8>,
    pub suffix_2p: Vec<u8>,
    pub suffix_3p: Vec<u8>,
    pub suffix_mp: Vec<u8>,
    pub line_max: usize,
    pub indent_space: Vec<u8>,
    pub indent_length: usize,
    pub page_precedence: Vec<u8>,
}

impl Default for Style {
    fn default() -> Self {
        let s = |text: &str| text.as_bytes().to_vec();
        Style {
            keyword: s("\\indexentry"),
            arg_open: b'{',
            arg_close: b'}',
            range_open: b'(',
            range_close: b')',
            level: b'!',
            actual: b'@',
            encap: b'|',
            quote: b'"',
            escape: b'\\',
            page_compositor: s("-"),
            preamble: s("\\begin{theindex}\n"),
            postamble: s("\n\n\\end{theindex}\n"),
            setpage_prefix: s("\n  \\setcounter{page}{"),
            setpage_suffix: s("}\n"),
            group_skip: s("\n\n  \\indexspace\n"),
            headings_flag: 0,
            heading_prefix: Vec::new(),
            heading_suffix: Vec::new(),
            symhead_positive: s("Symbols"),
            symhead_negative: s("symbols"),
            numhead_positive: s("Numbers"),
            numhead_negative: s("numbers"),
            item_0: s("\n  \\item "),
            item_1: s("\n    \\subitem "),
            item_2: s("\n      \\subsubitem "),
            item_01: s("\n    \\subitem "),
            item_x1: s("\n    \\subitem "),
            item_12: s("\n      \\subsubitem "),
            item_x2: s("\n      \\subsubitem "),
            delim_0: s(", "),
            delim_1: s(", "),
            delim_2: s(", "),
            delim_n: s(", "),
            delim_r: s("--"),
            delim_t: Vec::new(),
            encap_prefix: s("\\"),
            encap_infix: s("{"),
            encap_suffix: s("}"),
            suffix_2p: Vec::new(),
            suffix_3p: Vec::new(),
            suffix_mp: Vec::new(),
            line_max: 72,
            indent_space: s("\t\t"),
            indent_length: 16,
            page_precedence: s("rnaRA"),
        }
    }
}

/// A diagnostic from scanning a style file, formatted as in the transcript.
pub struct StyleScan {
    /// Number of attributes that were set.
    pub redefined: usize,
    /// Number of specifiers that were rejected.
    pub ignored: usize,
    /// Messages, already formatted (`** Input style error ...`).
    pub messages: Vec<String>,
}

enum Value {
    Text(Vec<u8>),
    Char(u8),
    Number(i32),
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    line: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.bytes.get(self.pos).copied()?;
        self.pos += 1;
        if byte == b'\n' {
            self.line += 1;
        }
        Some(byte)
    }

    fn skip_blanks_and_comments(&mut self) {
        while let Some(byte) = self.peek() {
            if byte == b'%' {
                while let Some(next) = self.peek() {
                    if next == b'\n' {
                        break;
                    }
                    self.bump();
                }
            } else if byte.is_ascii_whitespace() {
                self.bump();
            } else {
                break;
            }
        }
    }
}

impl Style {
    /// Applies a style file to `self`, as makeindex's `scan_style`.
    pub fn scan(&mut self, bytes: &[u8], file_name: &str) -> StyleScan {
        let mut scan = StyleScan { redefined: 0, ignored: 0, messages: Vec::new() };
        let mut cursor = Cursor { bytes, pos: 0, line: 1 };
        loop {
            cursor.skip_blanks_and_comments();
            if cursor.peek().is_none() {
                break;
            }
            let mut specifier = Vec::new();
            while let Some(byte) = cursor.peek() {
                if byte.is_ascii_whitespace() || matches!(byte, b'"' | b'\'' | b'%') {
                    break;
                }
                specifier.push(byte);
                cursor.bump();
            }
            let name = String::from_utf8_lossy(&specifier).into_owned();
            cursor.skip_blanks_and_comments();
            let at_line = cursor.line;
            let value = match cursor.peek() {
                None => break,
                Some(b'"') => {
                    cursor.bump();
                    let mut text = Vec::new();
                    let mut closed = false;
                    while let Some(byte) = cursor.bump() {
                        match byte {
                            b'"' => {
                                closed = true;
                                break;
                            }
                            b'\\' => match cursor.bump() {
                                Some(b'n') => text.push(b'\n'),
                                Some(b't') => text.push(b'\t'),
                                Some(other) => text.push(other),
                                None => break,
                            },
                            other => text.push(other),
                        }
                    }
                    if !closed {
                        scan.messages.push(style_error(
                            file_name,
                            at_line,
                            "No closing delimiter.",
                        ));
                        scan.ignored += 1;
                        break;
                    }
                    Value::Text(text)
                }
                Some(b'\'') => {
                    cursor.bump();
                    let first = cursor.bump();
                    let byte = match first {
                        Some(b'\\') => cursor.bump(),
                        other => other,
                    };
                    let closing = cursor.bump();
                    match (byte, closing) {
                        (Some(byte), Some(b'\'')) => Value::Char(byte),
                        _ => {
                            scan.messages.push(style_error(
                                file_name,
                                at_line.saturating_sub(1),
                                "No closing delimiter or too many letters.",
                            ));
                            scan.ignored += 1;
                            // The scanner resumes after the offending letters.
                            while let Some(next) = cursor.peek() {
                                if next.is_ascii_whitespace() {
                                    break;
                                }
                                cursor.bump();
                            }
                            continue;
                        }
                    }
                }
                Some(byte) if byte.is_ascii_digit() || byte == b'-' => {
                    let mut digits = String::new();
                    digits.push(cursor.bump().map(char::from).unwrap_or('0'));
                    while let Some(next) = cursor.peek() {
                        if !next.is_ascii_digit() {
                            break;
                        }
                        digits.push(char::from(next));
                        cursor.bump();
                    }
                    Value::Number(digits.parse().unwrap_or(0))
                }
                Some(_) => {
                    scan.messages.push(style_error(file_name, at_line, "No opening delimiter."));
                    scan.ignored += 1;
                    while let Some(next) = cursor.peek() {
                        if next.is_ascii_whitespace() {
                            break;
                        }
                        cursor.bump();
                    }
                    continue;
                }
            };
            match self.set(&name, value) {
                Ok(()) => scan.redefined += 1,
                Err(message) => {
                    scan.messages.push(style_error(file_name, at_line, &message));
                    scan.ignored += 1;
                }
            }
        }
        scan
    }

    fn set(&mut self, name: &str, value: Value) -> Result<(), String> {
        macro_rules! text {
            ($field:ident) => {
                match value {
                    Value::Text(text) => {
                        self.$field = text;
                        Ok(())
                    }
                    _ => Err(format!("Unknown specifier {name}.")),
                }
            };
        }
        macro_rules! character {
            ($field:ident) => {
                match value {
                    Value::Char(byte) => {
                        self.$field = byte;
                        Ok(())
                    }
                    _ => Err(format!("Unknown specifier {name}.")),
                }
            };
        }
        match name {
            "keyword" => text!(keyword),
            "preamble" => text!(preamble),
            "postamble" => text!(postamble),
            "group_skip" => text!(group_skip),
            "heading_prefix" => text!(heading_prefix),
            "heading_suffix" => text!(heading_suffix),
            "symhead_positive" => text!(symhead_positive),
            "symhead_negative" => text!(symhead_negative),
            "numhead_positive" => text!(numhead_positive),
            "numhead_negative" => text!(numhead_negative),
            "setpage_prefix" => text!(setpage_prefix),
            "setpage_suffix" => text!(setpage_suffix),
            "item_0" => text!(item_0),
            "item_1" => text!(item_1),
            "item_2" => text!(item_2),
            "item_01" => text!(item_01),
            "item_12" => text!(item_12),
            "item_x1" => text!(item_x1),
            "item_x2" => text!(item_x2),
            "encap_prefix" => text!(encap_prefix),
            "encap_infix" => text!(encap_infix),
            "encap_suffix" => text!(encap_suffix),
            "delim_0" => text!(delim_0),
            "delim_1" => text!(delim_1),
            "delim_2" => text!(delim_2),
            "delim_n" => text!(delim_n),
            "delim_r" => text!(delim_r),
            "delim_t" => text!(delim_t),
            "suffix_2p" => text!(suffix_2p),
            "suffix_3p" => text!(suffix_3p),
            "suffix_mp" => text!(suffix_mp),
            "indent_space" => text!(indent_space),
            "page_compositor" => text!(page_compositor),
            "arg_open" => character!(arg_open),
            "arg_close" => character!(arg_close),
            "range_open" => character!(range_open),
            "range_close" => character!(range_close),
            "level" => character!(level),
            "actual" => character!(actual),
            "encap" => character!(encap),
            "headings_flag" => match value {
                Value::Number(number) => {
                    self.headings_flag = number;
                    Ok(())
                }
                _ => Err(format!("Unknown specifier {name}.")),
            },
            "line_max" => match value {
                Value::Number(number) if number > 0 => {
                    self.line_max = number as usize;
                    Ok(())
                }
                Value::Number(number) => {
                    Err(format!("line_max must be positive (got {number})"))
                }
                _ => Err(format!("Unknown specifier {name}.")),
            },
            "indent_length" => match value {
                Value::Number(number) if number >= 0 => {
                    self.indent_length = number as usize;
                    Ok(())
                }
                Value::Number(number) => {
                    Err(format!("indent_length must be nonnegative (got {number})"))
                }
                _ => Err(format!("Unknown specifier {name}.")),
            },
            "quote" => match value {
                Value::Char(byte) if byte == self.escape => Err(format!(
                    "Quote and escape symbols must be distinct (both `{}' now).",
                    char::from(byte)
                )),
                Value::Char(byte) => {
                    self.quote = byte;
                    Ok(())
                }
                _ => Err(format!("Unknown specifier {name}.")),
            },
            "escape" => match value {
                Value::Char(byte) if byte == self.quote => Err(format!(
                    "Quote and escape symbols must be distinct (both `{}' now).",
                    char::from(byte)
                )),
                Value::Char(byte) => {
                    self.escape = byte;
                    Ok(())
                }
                _ => Err(format!("Unknown specifier {name}.")),
            },
            "page_precedence" => match value {
                Value::Text(text) => self.set_page_precedence(text),
                _ => Err(format!("Unknown specifier {name}.")),
            },
            _ => Err(format!("Unknown specifier {name}.")),
        }
    }

    fn set_page_precedence(&mut self, text: Vec<u8>) -> Result<(), String> {
        if text.len() > 5 {
            return Err("Page precedence specification string too long.".to_string());
        }
        let mut seen = [false; 5];
        for &byte in &text {
            let Some(index) = b"rnaRA".iter().position(|&kind| kind == byte) else {
                return Err("Unknow type `".to_string() + &char::from(byte).to_string()
                    + "' in page precedence specification.");
            };
            if seen[index] {
                return Err(format!(
                    "Multiple instances of type `{}' in page precedence specification `{}'.",
                    char::from(byte),
                    String::from_utf8_lossy(&text)
                ));
            }
            seen[index] = true;
        }
        self.page_precedence = text;
        Ok(())
    }
}

fn style_error(file: &str, line: usize, message: &str) -> String {
    format!("** Input style error (file = {file}, line = {line}):\n   -- {message}\n")
}
