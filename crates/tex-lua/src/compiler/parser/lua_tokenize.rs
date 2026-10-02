use crate::compiler::parser::{
    lua_token_data::LuaTokenData, lua_token_kind::LuaTokenKind, reader::Reader,
};

use super::lua_language_level::LuaLanguageLevel;
use super::tokenize_config::TokensizeConfig;

pub struct LuaTokenize<'a> {
    reader: Reader<'a>,
    lexer_config: TokensizeConfig,
    error: Option<String>,
    line: usize,
}

impl<'a> LuaTokenize<'a> {
    pub fn new(reader: Reader<'a>, lexer_config: TokensizeConfig) -> Self {
        LuaTokenize {
            reader,
            lexer_config,
            error: None,
            line: 1,
        }
    }

    pub fn next_token_data(&mut self) -> Result<LuaTokenData, String> {
        loop {
            if self.reader.is_eof() {
                return Ok(LuaTokenData::with_line(
                    LuaTokenKind::TkEof,
                    self.reader.current_range(),
                    self.line,
                ));
            }

            let kind = self.lex();
            if let Some(err) = &self.error {
                return Err(err.clone());
            }

            if matches!(
                kind,
                LuaTokenKind::TkShortComment
                    | LuaTokenKind::TkLongComment
                    | LuaTokenKind::TkEndOfLine
                    | LuaTokenKind::TkWhitespace
            ) {
                continue;
            }

            return Ok(LuaTokenData::with_line(
                kind,
                self.reader.current_range(),
                self.line,
            ));
        }
    }

    fn name_to_kind(&self, name: &str) -> LuaTokenKind {
        match name {
            "and" => LuaTokenKind::TkAnd,
            "break" => LuaTokenKind::TkBreak,
            "do" => LuaTokenKind::TkDo,
            "else" => LuaTokenKind::TkElse,
            "elseif" => LuaTokenKind::TkElseIf,
            "end" => LuaTokenKind::TkEnd,
            "false" => LuaTokenKind::TkFalse,
            "for" => LuaTokenKind::TkFor,
            "function" => LuaTokenKind::TkFunction,
            "goto" => LuaTokenKind::TkGoto,
            "if" => LuaTokenKind::TkIf,
            "in" => LuaTokenKind::TkIn,
            "local" => LuaTokenKind::TkLocal,
            "nil" => LuaTokenKind::TkNil,
            "not" => LuaTokenKind::TkNot,
            "or" => LuaTokenKind::TkOr,
            "repeat" => LuaTokenKind::TkRepeat,
            "return" => LuaTokenKind::TkReturn,
            "then" => LuaTokenKind::TkThen,
            "true" => LuaTokenKind::TkTrue,
            "until" => LuaTokenKind::TkUntil,
            "while" => LuaTokenKind::TkWhile,
            _ => LuaTokenKind::TkName,
        }
    }

    fn lex(&mut self) -> LuaTokenKind {
        self.reader.reset_buff();

        match self.reader.current_char() {
            '\n' | '\r' => self.lex_new_line(),
            ' ' | '\t' => self.lex_white_space(),
            '-' => {
                self.reader.bump();

                if self.reader.current_char() != '-' {
                    return LuaTokenKind::TkMinus;
                }

                self.reader.bump();
                if self.reader.current_char() == '[' {
                    self.reader.bump();
                    let sep = self.skip_sep();
                    if self.reader.current_char() == '[' {
                        self.reader.bump();
                        self.lex_long_string(sep, false);
                        return LuaTokenKind::TkLongComment;
                    }
                }

                self.reader.eat_while(|ch| ch != '\n' && ch != '\r');
                LuaTokenKind::TkShortComment
            }
            '[' => {
                self.reader.bump();
                let sep = self.skip_sep();
                if sep == 0 && self.reader.current_char() != '[' {
                    return LuaTokenKind::TkLeftBracket;
                }
                if self.reader.current_char() != '[' {
                    if self.is_lua53() {
                        self.error(|| format!("invalid long string delimiter near '[{}'", "=".repeat(sep)));
                    } else {
                        self.error(|| "invalid long string delimiter".to_string());
                    }
                    return LuaTokenKind::TkLongString;
                }

                self.reader.bump();
                self.lex_long_string(sep, true)
            }
            '=' => {
                self.reader.bump();
                if self.reader.current_char() != '=' {
                    return LuaTokenKind::TkAssign;
                }
                self.reader.bump();
                LuaTokenKind::TkEq
            }
            '<' => {
                self.reader.bump();
                match self.reader.current_char() {
                    '=' => {
                        self.reader.bump();
                        LuaTokenKind::TkLe
                    }
                    '<' => {
                        if !self.lexer_config.support_integer_operation() {
                            self.error(|| "bitwise operation is not supported".to_string());
                        }

                        self.reader.bump();
                        LuaTokenKind::TkShl
                    }
                    _ => LuaTokenKind::TkLt,
                }
            }
            '>' => {
                self.reader.bump();
                match self.reader.current_char() {
                    '=' => {
                        self.reader.bump();
                        LuaTokenKind::TkGe
                    }
                    '>' => {
                        if !self.lexer_config.support_integer_operation() {
                            self.error(|| "bitwise operation is not supported".to_string());
                        }

                        self.reader.bump();
                        LuaTokenKind::TkShr
                    }
                    _ => LuaTokenKind::TkGt,
                }
            }
            '~' => {
                self.reader.bump();
                if self.reader.current_char() != '=' {
                    if !self.lexer_config.support_integer_operation() {
                        self.error(|| "bitwise operation is not supported".to_string());
                    }
                    return LuaTokenKind::TkBitXor;
                }
                self.reader.bump();
                LuaTokenKind::TkNe
            }
            ':' => {
                self.reader.bump();
                if self.reader.current_char() != ':' {
                    return LuaTokenKind::TkColon;
                }
                self.reader.bump();
                LuaTokenKind::TkDbColon
            }
            '"' | '\'' => {
                let quote = self.reader.current_char();
                self.reader.bump();
                if self.is_lua53() {
                    self.lex_string53(quote)
                } else {
                    self.lex_string(quote)
                }
            }
            '`' => {
                if self.lexer_config.language_level == crate::LuaLanguageLevel::Lua53 {
                    self.reader.bump();
                    self.error(|| "unexpected symbol near '`'".to_string());
                    LuaTokenKind::TkString
                } else {
                    self.reader.bump();
                    self.lex_string('`')
                }
            }
            '.' => {
                if self.reader.next_char().is_ascii_digit() {
                    return if self.is_lua53() { self.lex_number53() } else { self.lex_number() };
                }

                self.reader.bump();
                if self.reader.current_char() != '.' {
                    return LuaTokenKind::TkDot;
                }
                self.reader.bump();
                if self.reader.current_char() != '.' {
                    return LuaTokenKind::TkConcat;
                }
                self.reader.bump();
                LuaTokenKind::TkDots
            }
            '0'..='9' => {
                if self.is_lua53() {
                    self.lex_number53()
                } else {
                    self.lex_number()
                }
            }
            '/' => {
                self.reader.bump();
                let current_char = self.reader.current_char();
                match current_char {
                    _ if current_char != '/' => LuaTokenKind::TkDiv,
                    _ => {
                        if !self.lexer_config.support_integer_operation() {
                            self.error(|| "integer division is not supported".to_string());
                        }

                        self.reader.bump();
                        LuaTokenKind::TkIDiv
                    }
                }
            }
            '*' => {
                self.reader.bump();
                LuaTokenKind::TkMul
            }
            '+' => {
                self.reader.bump();
                LuaTokenKind::TkPlus
            }
            '%' => {
                self.reader.bump();
                LuaTokenKind::TkMod
            }
            '^' => {
                self.reader.bump();
                LuaTokenKind::TkPow
            }
            '#' => {
                self.reader.bump();
                LuaTokenKind::TkLen
            }
            '&' => {
                self.reader.bump();
                if !self.lexer_config.support_integer_operation() {
                    self.error(|| "bitwise operation is not supported".to_string());
                }
                LuaTokenKind::TkBitAnd
            }
            '|' => {
                self.reader.bump();
                if !self.lexer_config.support_integer_operation() {
                    self.error(|| "bitwise operation is not supported".to_string());
                }
                LuaTokenKind::TkBitOr
            }
            '(' => {
                self.reader.bump();
                LuaTokenKind::TkLeftParen
            }
            ')' => {
                self.reader.bump();
                LuaTokenKind::TkRightParen
            }
            '{' => {
                self.reader.bump();
                LuaTokenKind::TkLeftBrace
            }
            '}' => {
                self.reader.bump();
                LuaTokenKind::TkRightBrace
            }
            ']' => {
                self.reader.bump();
                LuaTokenKind::TkRightBracket
            }
            ';' => {
                self.reader.bump();
                LuaTokenKind::TkSemicolon
            }
            ',' => {
                self.reader.bump();
                LuaTokenKind::TkComma
            }
            _ if self.reader.is_eof() => LuaTokenKind::TkEof,
            ch if is_name_start(ch, names_take_high_bytes(self.lexer_config.language_level)) => {
                let high = names_take_high_bytes(self.lexer_config.language_level);
                self.reader.bump();
                self.reader.eat_while(|ch| is_name_continue(ch, high));
                let name = self.reader.current_text();
                self.name_to_kind(name)
            }
            _ => {
                self.reader.bump();
                LuaTokenKind::TkUnknown
            }
        }
    }

    fn lex_new_line(&mut self) -> LuaTokenKind {
        match self.reader.current_char() {
            // support \n or \n\r
            '\n' => {
                self.reader.bump();
                if self.reader.current_char() == '\r' {
                    self.reader.bump();
                }
            }
            // support \r or \r\n
            '\r' => {
                self.reader.bump();
                if self.reader.current_char() == '\n' {
                    self.reader.bump();
                }
            }
            _ => {}
        }
        self.line += 1;

        LuaTokenKind::TkEndOfLine
    }

    fn lex_white_space(&mut self) -> LuaTokenKind {
        self.reader
            .eat_while(|ch| ch == ' ' || ch == '\t' || ch == '\x0B' || ch == '\x0C');
        LuaTokenKind::TkWhitespace
    }

    fn skip_sep(&mut self) -> usize {
        self.reader.eat_when('=')
    }

    fn lex_string(&mut self, quote: char) -> LuaTokenKind {
        while !self.reader.is_eof() {
            let ch = self.reader.current_char();
            if ch == quote || ch == '\n' || ch == '\r' {
                break;
            }

            if ch != '\\' {
                self.reader.bump();
                continue;
            }

            self.reader.bump();
            match self.reader.current_char() {
                'z' => {
                    self.reader.bump();
                    // Skip whitespace after \z, tracking line numbers
                    while !self.reader.is_eof() {
                        let c = self.reader.current_char();
                        if c == ' ' || c == '\t' || c == '\x0B' || c == '\x0C' {
                            self.reader.bump();
                        } else if c == '\r' || c == '\n' {
                            self.lex_new_line();
                        } else {
                            break;
                        }
                    }
                }
                'x' => {
                    // Hexadecimal escape: \xHH
                    self.reader.bump(); // skip 'x'
                    // Need exactly 2 hex digits
                    let ch1 = self.reader.current_char();
                    if !ch1.is_ascii_hexdigit() {
                        // Build error message with context
                        let mut ctx = String::from("\\x");
                        if ch1 != '\0' && ch1 != '\n' && ch1 != '\r' {
                            ctx.push(ch1);
                        }
                        self.error(|| format!("hexadecimal digit expected near '{}'", ctx));
                        return LuaTokenKind::TkString;
                    }
                    self.reader.bump();

                    let ch2 = self.reader.current_char();
                    if !ch2.is_ascii_hexdigit() {
                        // Build error message with context
                        let mut ctx = String::from("\\x");
                        ctx.push(ch1);
                        if ch2 != '\0' && ch2 != '\n' && ch2 != '\r' {
                            ctx.push(ch2);
                        }
                        self.error(|| format!("hexadecimal digit expected near '{}'", ctx));
                        return LuaTokenKind::TkString;
                    }
                    self.reader.bump();
                }
                'u' => {
                    // Unicode escape: \u{XXX}
                    self.reader.bump(); // skip 'u'
                    if self.reader.current_char() != '{' {
                        // Missing '{' after \u
                        // Get context: extract chars before current position
                        // current_text() includes from token start (the quote) to after 'u'
                        let text_before = self.reader.current_text();
                        // Skip opening quote and get last few chars
                        let context_start = if text_before.len() > 6 {
                            // Get last 5 chars (will include `abc\u`)
                            &text_before[text_before.len() - 5..]
                        } else if text_before.len() > 1 {
                            &text_before[1..] // Skip opening quote
                        } else {
                            ""
                        };
                        let mut ctx = String::from(context_start);
                        let next_ch = self.reader.current_char();
                        if next_ch != '\0' && next_ch != '\n' && next_ch != '\r' {
                            ctx.push(next_ch);
                        }
                        self.error(|| format!("missing '{{' in unicode escape near '{}'", ctx));
                        return LuaTokenKind::TkString;
                    }
                    self.reader.bump(); // skip '{'

                    // Collect hex digits
                    let mut hex_digits = String::new();
                    while self.reader.current_char() != '}' {
                        let ch = self.reader.current_char();
                        if ch == '\0' || ch == '\n' || ch == '\r' {
                            // Unfinished escape
                            let text_before = self.reader.current_text();
                            let context_start = if text_before.len() > 11 {
                                &text_before[text_before.len() - 10..]
                            } else if text_before.len() > 1 {
                                &text_before[1..]
                            } else {
                                ""
                            };
                            let ctx = String::from(context_start);
                            self.error(|| format!("unfinished unicode escape near '{}'", ctx));
                            return LuaTokenKind::TkString;
                        }
                        if !ch.is_ascii_hexdigit() {
                            // Non-hex character
                            let text_before = self.reader.current_text();
                            let context_start = if text_before.len() > 11 {
                                &text_before[text_before.len() - 10..]
                            } else if text_before.len() > 1 {
                                &text_before[1..]
                            } else {
                                ""
                            };
                            let mut ctx = String::from(context_start);
                            ctx.push(ch);
                            self.error(|| {
                                format!(
                                    "hexadecimal digit expected in unicode escape near '{}'",
                                    ctx
                                )
                            });
                            return LuaTokenKind::TkString;
                        }
                        hex_digits.push(ch);
                        self.reader.bump();
                    }

                    if hex_digits.is_empty() {
                        let text_before = self.reader.current_text();
                        let context_start = if text_before.len() > 11 {
                            &text_before[text_before.len() - 10..]
                        } else if text_before.len() > 1 {
                            &text_before[1..]
                        } else {
                            ""
                        };
                        let mut ctx = String::from(context_start);
                        let next_ch = self.reader.current_char();
                        if next_ch != '\0' && next_ch != '\n' && next_ch != '\r' {
                            ctx.push(next_ch);
                        }
                        self.error(|| {
                            format!(
                                "hexadecimal digit expected in unicode escape near '{}'",
                                ctx
                            )
                        });
                        return LuaTokenKind::TkString;
                    }

                    let max_value = if self.lexer_config.language_level == LuaLanguageLevel::Lua53 {
                        0x10FFFF
                    } else {
                        0x7FFFFFFF
                    };
                    match u32::from_str_radix(&hex_digits, 16) {
                        Ok(val) if val > max_value => {
                            // Value too large, include context in error
                            let text_before = self.reader.current_text();
                            let context_start = if text_before.len() > 16 {
                                &text_before[text_before.len() - 15..]
                            } else if text_before.len() > 1 {
                                &text_before[1..]
                            } else {
                                ""
                            };
                            // Don't include the closing } - the error is about the value
                            // being too large, which is detected before we accept the }
                            let ctx = String::from(context_start);
                            self.error(|| format!("UTF-8 value too large near '{}'", ctx));
                            return LuaTokenKind::TkString;
                        }
                        Err(_) => {
                            // Parse error means value too large for u32
                            let text_before = self.reader.current_text();
                            let context_start = if text_before.len() > 16 {
                                &text_before[text_before.len() - 15..]
                            } else if text_before.len() > 1 {
                                &text_before[1..]
                            } else {
                                ""
                            };
                            let ctx = String::from(context_start);
                            self.error(|| format!("UTF-8 value too large near '{}'", ctx));
                            return LuaTokenKind::TkString;
                        }
                        Ok(_) => {
                            // Valid value, skip '}'
                            self.reader.bump();
                        }
                    }
                }
                '\r' | '\n' => {
                    self.lex_new_line();
                }
                '0'..='9' => {
                    // Decimal escape: \DDD (up to 3 digits, max 255)
                    let start_ch = self.reader.current_char();
                    let mut digits = String::new();
                    digits.push(start_ch);
                    self.reader.bump();

                    let mut count = 1;
                    while count < 3 && self.reader.current_char().is_ascii_digit() {
                        digits.push(self.reader.current_char());
                        self.reader.bump();
                        count += 1;
                    }

                    // Validate range (0-255)
                    if let Ok(val) = digits.parse::<u16>()
                        && val > 255
                    {
                        // Include next char in error context if it's not special
                        let mut ctx = format!("\\{}", digits);
                        let next_ch = self.reader.current_char();
                        if next_ch != '\0' && next_ch != '\n' && next_ch != '\r' {
                            ctx.push(next_ch);
                        }
                        self.error(|| format!("decimal escape too large near '{}'", ctx));
                        return LuaTokenKind::TkString;
                    }
                }
                'a' | 'b' | 'f' | 'n' | 'r' | 't' | 'v' | '\\' | '\'' | '\"' => {
                    // Valid single-character escapes
                    self.reader.bump();
                }
                _ => {
                    // Invalid escape sequence
                    let ch = self.reader.current_char();
                    self.error(|| format!("invalid escape sequence near '\\{}'", ch));
                    return LuaTokenKind::TkString;
                }
            }
        }

        if self.reader.current_char() != quote {
            self.error(|| "unfinished string near <eof>".to_string());
            return LuaTokenKind::TkString;
        }

        self.reader.bump();
        LuaTokenKind::TkString
    }

    fn lex_long_string(&mut self, sep: usize, is_string: bool) -> LuaTokenKind {
        let start_line = self.line;
        let mut end = false;
        while !self.reader.is_eof() {
            match self.reader.current_char() {
                ']' => {
                    self.reader.bump();
                    let count = self.reader.eat_when('=');
                    if count == sep && self.reader.current_char() == ']' {
                        self.reader.bump();
                        end = true;
                        break;
                    }
                }
                '\n' | '\r' => {
                    self.lex_new_line();
                }
                _ => {
                    self.reader.bump();
                }
            }
        }

        if !end {
            if self.is_lua53() {
                let what = if is_string { "string" } else { "comment" };
                self.error(|| format!("unfinished long {what} (starting at line {start_line}) near <eof>"));
            } else {
                self.error(|| "unfinished long string or comment near <eof>".to_string());
            }
        }

        LuaTokenKind::TkLongString
    }

    fn is_lua53(&self) -> bool {
        self.lexer_config.language_level == LuaLanguageLevel::Lua53
    }

    /// llex.c `esccheck`: on failure the current character joins the buffer
    /// and the error quotes the buffer.
    fn esccheck53(&mut self, buf: &mut String, ok: bool, msg: &str) -> bool {
        if !ok {
            if !self.reader.is_eof() {
                buf.push(self.reader.current_char());
                self.reader.bump();
            }
            let text = buf.clone();
            self.error(|| format!("{msg} near '{text}'"));
        }
        ok
    }

    /// llex.c `read_string` of Lua 5.3 (the opening quote is consumed). `buf`
    /// mirrors the lexer buffer, which the error messages quote.
    fn lex_string53(&mut self, quote: char) -> LuaTokenKind {
        let mut buf = String::new();
        buf.push(quote);
        loop {
            if self.reader.is_eof() {
                self.error(|| "unfinished string near <eof>".to_string());
                return LuaTokenKind::TkString;
            }
            let c = self.reader.current_char();
            if c == quote {
                break;
            }
            match c {
                '\n' | '\r' => {
                    let text = buf.clone();
                    self.error(|| format!("unfinished string near '{text}'"));
                    return LuaTokenKind::TkString;
                }
                '\\' => {
                    buf.push('\\');
                    self.reader.bump();
                    if self.reader.is_eof() {
                        continue;
                    }
                    let e = self.reader.current_char();
                    let simple = match e {
                        'a' => Some('\x07'),
                        'b' => Some('\x08'),
                        'f' => Some('\x0c'),
                        'n' => Some('\n'),
                        'r' => Some('\r'),
                        't' => Some('\t'),
                        'v' => Some('\x0b'),
                        '\\' | '"' | '\'' => Some(e),
                        _ => None,
                    };
                    if let Some(value) = simple {
                        self.reader.bump();
                        buf.pop();
                        buf.push(value);
                        continue;
                    }
                    match e {
                        'x' => {
                            let mut value = 0u32;
                            for _ in 0..2 {
                                buf.push(self.reader.current_char());
                                self.reader.bump();
                                let d = self.reader.current_char();
                                let ok = !self.reader.is_eof() && d.is_ascii_hexdigit();
                                if !self.esccheck53(&mut buf, ok, "hexadecimal digit expected") {
                                    return LuaTokenKind::TkString;
                                }
                                value = value * 16 + d.to_digit(16).unwrap_or(0);
                            }
                            self.reader.bump();
                            for _ in 0..3 {
                                buf.pop();
                            }
                            buf.push(char::from(value as u8));
                        }
                        'u' => {
                            buf.push('u');
                            self.reader.bump();
                            let brace = self.reader.current_char() == '{';
                            if !self.esccheck53(&mut buf, brace, "missing '{'") {
                                return LuaTokenKind::TkString;
                            }
                            buf.push('{');
                            self.reader.bump();
                            let first = self.reader.current_char();
                            let ok = !self.reader.is_eof() && first.is_ascii_hexdigit();
                            if !self.esccheck53(&mut buf, ok, "hexadecimal digit expected") {
                                return LuaTokenKind::TkString;
                            }
                            let mut value = u64::from(first.to_digit(16).unwrap_or(0));
                            let mut removed = 4;
                            loop {
                                buf.push(self.reader.current_char());
                                self.reader.bump();
                                let d = self.reader.current_char();
                                if self.reader.is_eof() || !d.is_ascii_hexdigit() {
                                    break;
                                }
                                removed += 1;
                                value = (value << 4) + u64::from(d.to_digit(16).unwrap_or(0));
                                if !self.esccheck53(&mut buf, value <= 0x10FFFF, "UTF-8 value too large") {
                                    return LuaTokenKind::TkString;
                                }
                            }
                            let close = self.reader.current_char() == '}';
                            if !self.esccheck53(&mut buf, close, "missing '}'") {
                                return LuaTokenKind::TkString;
                            }
                            for _ in 0..removed {
                                buf.pop();
                            }
                            buf.push(char::from_u32(value as u32).unwrap_or('\u{fffd}'));
                        }
                        '\n' | '\r' => {
                            self.lex_new_line();
                            buf.pop();
                            buf.push('\n');
                        }
                        'z' => {
                            buf.pop();
                            self.reader.bump();
                            while matches!(self.reader.current_char(), ' ' | '\t' | '\x0b' | '\x0c' | '\n' | '\r') {
                                if matches!(self.reader.current_char(), '\n' | '\r') {
                                    self.lex_new_line();
                                } else {
                                    self.reader.bump();
                                }
                            }
                        }
                        _ => {
                            if !self.esccheck53(&mut buf, e.is_ascii_digit(), "invalid escape sequence") {
                                return LuaTokenKind::TkString;
                            }
                            let mut value = 0u32;
                            let mut digits = 0;
                            while digits < 3 && self.reader.current_char().is_ascii_digit() {
                                value = value * 10 + self.reader.current_char().to_digit(10).unwrap_or(0);
                                buf.push(self.reader.current_char());
                                self.reader.bump();
                                digits += 1;
                            }
                            if !self.esccheck53(&mut buf, value <= 255, "decimal escape too large") {
                                return LuaTokenKind::TkString;
                            }
                            for _ in 0..digits + 1 {
                                buf.pop();
                            }
                            buf.push(char::from(value as u8));
                        }
                    }
                }
                _ => {
                    buf.push(c);
                    self.reader.bump();
                }
            }
        }
        self.reader.bump();
        LuaTokenKind::TkString
    }

    /// llex.c `read_numeral` of Lua 5.3: take every hexadecimal digit, `.` and
    /// exponent sign, then require the whole text to be one numeral.
    fn lex_number53(&mut self) -> LuaTokenKind {
        let first = self.reader.current_char();
        let mut text = String::new();
        text.push(first);
        self.reader.bump();
        let mut expo = ['e', 'E'];
        if first == '0' && matches!(self.reader.current_char(), 'x' | 'X') {
            text.push(self.reader.current_char());
            self.reader.bump();
            expo = ['p', 'P'];
        }
        loop {
            let c = self.reader.current_char();
            if expo.contains(&c) {
                text.push(c);
                self.reader.bump();
                if matches!(self.reader.current_char(), '+' | '-') {
                    text.push(self.reader.current_char());
                    self.reader.bump();
                }
            }
            let c = self.reader.current_char();
            if c.is_ascii_hexdigit() || c == '.' {
                text.push(c);
                self.reader.bump();
            } else {
                break;
            }
        }
        if !numeral53_is_valid(&text) {
            self.error(|| format!("malformed number near '{text}'"));
            return LuaTokenKind::TkFloat;
        }
        let hex = text.starts_with("0x") || text.starts_with("0X");
        let integer = if hex {
            text[2..].bytes().all(|b| b.is_ascii_hexdigit())
        } else {
            text.bytes().all(|b| b.is_ascii_digit())
        };
        if integer {
            LuaTokenKind::TkInt
        } else {
            LuaTokenKind::TkFloat
        }
    }

    fn lex_number(&mut self) -> LuaTokenKind {
        enum NumberState {
            Int,
            Float,
            Hex,
            HexFloat,
            WithExpo,
            Bin,
        }

        let mut state = NumberState::Int;
        let first = self.reader.current_char();
        self.reader.bump();
        match first {
            '0' if matches!(self.reader.current_char(), 'X' | 'x') => {
                self.reader.bump();
                state = NumberState::Hex;
            }
            '0' if matches!(self.reader.current_char(), 'B' | 'b')
                && self.lexer_config.support_binary_integer() =>
            {
                self.reader.bump();
                state = NumberState::Bin;
            }
            '.' => {
                state = NumberState::Float;
            }
            _ => {}
        }

        while !self.reader.is_eof() {
            let ch = self.reader.current_char();
            let continue_ = match state {
                NumberState::Int => match ch {
                    '0'..='9' => true,
                    '.' => {
                        state = NumberState::Float;
                        true
                    }
                    _ if matches!(self.reader.current_char(), 'e' | 'E') => {
                        if matches!(self.reader.next_char(), '+' | '-') {
                            self.reader.bump();
                        }
                        state = NumberState::WithExpo;
                        true
                    }
                    _ => false,
                },
                NumberState::Float => match ch {
                    '0'..='9' => true,
                    _ if matches!(self.reader.current_char(), 'e' | 'E') => {
                        if matches!(self.reader.next_char(), '+' | '-') {
                            self.reader.bump();
                        }
                        state = NumberState::WithExpo;
                        true
                    }
                    _ => false,
                },
                NumberState::Hex => match ch {
                    '0'..='9' | 'a'..='f' | 'A'..='F' => true,
                    '.' => {
                        state = NumberState::HexFloat;
                        true
                    }
                    _ if matches!(self.reader.current_char(), 'P' | 'p') => {
                        if matches!(self.reader.next_char(), '+' | '-') {
                            self.reader.bump();
                        }
                        state = NumberState::WithExpo;
                        true
                    }
                    _ => false,
                },
                NumberState::HexFloat => match ch {
                    '0'..='9' | 'a'..='f' | 'A'..='F' => true,
                    _ if matches!(self.reader.current_char(), 'P' | 'p') => {
                        if matches!(self.reader.next_char(), '+' | '-') {
                            self.reader.bump();
                        }
                        state = NumberState::WithExpo;
                        true
                    }
                    _ => false,
                },
                NumberState::WithExpo => ch.is_ascii_digit(),
                NumberState::Bin => matches!(ch, '0' | '1'),
            };

            if continue_ {
                self.reader.bump();
            } else {
                break;
            }
        }

        if self.lexer_config.support_complex_number() && self.reader.current_char() == 'i' {
            self.reader.bump();
            return LuaTokenKind::TkComplex;
        }

        if self.lexer_config.support_ll_integer()
            && matches!(
                state,
                NumberState::Int | NumberState::Hex | NumberState::Bin
            )
        {
            self.reader
                .eat_while(|ch| matches!(ch, 'u' | 'U' | 'l' | 'L'));
            return LuaTokenKind::TkInt;
        }

        if self.reader.current_char().is_ascii_alphabetic() {
            let ch = self.reader.current_char();
            self.error(|| format!("malformed number near '%{ch}'", ch = ch));
        }

        match state {
            NumberState::Int | NumberState::Hex => LuaTokenKind::TkInt,
            _ => LuaTokenKind::TkFloat,
        }
    }

    fn error<F, R>(&mut self, msg: F)
    where
        F: FnOnce() -> R,
        R: AsRef<str>,
    {
        self.error = Some(format!("{}: {}", self.line, msg().as_ref()));
    }
}

/// Whether bytes >= 0x80 are letters in names. LuaTeX builds Lua 5.3 with
/// `LUA_UCID`, where every byte >= 0x80 is a letter (so any UTF-8 text, and
/// any Latin-1 byte, can be part of a name); stock Lua 5.5 has no such option.
fn names_take_high_bytes(level: LuaLanguageLevel) -> bool {
    level == LuaLanguageLevel::Lua53
}

fn is_name_start(ch: char, high: bool) -> bool {
    ch.is_ascii_alphabetic() || ch == '_' || (high && !ch.is_ascii())
}

fn is_name_continue(ch: char, high: bool) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || (high && !ch.is_ascii())
}

/// Whether `text` is one Lua 5.3 numeral (`luaO_str2num`): decimal digits or
/// `0x` hexadecimal digits with an optional fraction and a `e`/`p` exponent.
fn numeral53_is_valid(text: &str) -> bool {
    let b = text.as_bytes();
    let hex = b.len() >= 2 && b[0] == b'0' && matches!(b[1], b'x' | b'X');
    let mut i = if hex { 2 } else { 0 };
    let digit = |c: u8| if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() };
    let mut digits = 0;
    while i < b.len() && digit(b[i]) {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && digit(b[i]) {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    let expo: [u8; 2] = if hex { [b'p', b'P'] } else { [b'e', b'E'] };
    if i < b.len() && expo.contains(&b[i]) {
        i += 1;
        if i < b.len() && matches!(b[i], b'+' | b'-') {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == b.len()
}
