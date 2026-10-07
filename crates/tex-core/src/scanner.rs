//! Tokenizer: raw token production from input sources with TeX's N/M/S
//! scanning states, ^^ notation, comments, control-sequence scanning.
//!
//! Line model: each file source holds the raw bytes; the current line is
//! materialized in `line_buf` like TeX's `buffer`: without its newline and
//! trailing spaces, followed by the `\endlinechar` current when it was read.
//! The line ends when `line_pos` reaches its length. State: 0 = new line
//! (N), 1 = mid line (M), 2 = skip blanks (S).

use crate::engine::{Engine, EngineKind, PhysicalTokenSource};
use crate::input::{physical_line_bounds, Source, EOF_MARKER, PAR_END};
use crate::token::*;

impl Engine {
    /// Fetch the next raw token from the input stack (no expansion).
    pub fn get_next_raw(&mut self) -> Token {
        if self.diagnostic_sources_live {
            self.clear_diagnostic_sources();
        }
        loop {
            if self.input.stack.is_empty() {
                return EOF_MARKER;
            }
            let si = self.input.stack.len() - 1;
            match self.input.stack[si] {
                Source::TokList { .. } | Source::MacroFrame(_) => {
                    if let Some(t) = self.toklist_next(si) {
                        return t;
                    }
                    continue; // list popped; retry
                }
                Source::File { .. } => {
                    if !self.align_macro_arg && self.diagnostic_trace_hold == 0 {
                        self.diagnostic_macro_trace.clear();
                        self.diagnostic_macro_trace_truncated = false;
                        self.diagnostic_macro_call_site = None;
                        self.diagnostic_macro_call_span = 1;
                    }
                    match self.file_next_token(si) {
                        Some(t) => return t,
                        None => continue, // file popped; retry
                    }
                }
            }
        }
    }

    pub(crate) fn toklist_next(&mut self, si: usize) -> Option<Token> {
        let trace_depth = match &self.input.stack[si] {
            Source::TokList { trace_depth, .. } => *trace_depth,
            Source::MacroFrame(frame) => frame.trace_depth,
            _ => unreachable!(),
        };
        self.unwind_macro_trace(trace_depth);
        match &mut self.input.stack[si] {
            Source::TokList { toks, pos, .. } => {
                if *pos < toks.len() {
                    let t = toks[*pos];
                    *pos += 1;
                    // Keep the exhausted list until the next fetch. The token
                    // currently being processed still belongs to this source;
                    // retaining it preserves the macro call chain for errors
                    // in a tail-position replacement token.
                    return Some(t);
                }
            }
            Source::MacroFrame(frame) => {
                if let Some(t) = frame.next_token() {
                    return Some(t);
                }
            }
            _ => unreachable!(),
        }
        if si + 1 == self.input.stack.len() {
            self.input.stack.pop();
        } else {
            self.input.stack.remove(si);
        }
        None
    }

    /// One token from the file source at `si`; None once the file has been
    /// finished and popped.
    ///
    /// This follows tex.web §343-§357 on a line buffer that, like TeX's,
    /// ends with the `\endlinechar` that was current when the line was read.
    fn file_next_token(&mut self, si: usize) -> Option<Token> {
        loop {
            let Source::File {
                done,
                ending,
                line_buf,
                line_pos,
                state,
                cat_regime,
                lua_lines,
                lua_reader,
                name,
                ..
            } = &self.input.stack[si]
            else {
                unreachable!()
            };
            if *done {
                // e-TeX semantics: \everyeof fires every time scanning
                // reaches EOF of an input file or pseudo-file (expl3 \file_get
                // and \tl_set_rescan rely on this to supply closing delimiters).
                // LuaTeX's `tex.print` input ends with force_eof instead.
                let lua = lua_lines.is_some();
                let reader = *lua_reader;
                let real_file = !name.starts_with('<') || name.starts_with("<embedded:");
                // The files whose start printed `(name`.
                let announced = real_file || name.starts_with("<compat:");
                if matches!(
                    self.input.stack.get(si),
                    Some(Source::File { tracked: true, .. })
                ) {
                    self.finish_tracked_file();
                }
                self.input.finish_file(si);
                if lua {
                    return None;
                }
                let mut reported = false;
                if self.engine_kind == EngineKind::LuaTeX {
                    // textoken.c force_eof: `stop_file`, then the reader's close
                    if real_file || reader != 0 {
                        reported = self.lua_report_stop_file(crate::lua_cb_files::filetype::TEX);
                    }
                    if reader != 0 {
                        self.lua_reader_close(reader);
                    }
                }
                // tex.web §362: TeX shows that the file has been read. Editors
                // follow the open files by these parentheses.
                if announced && !reported {
                    self.tex_print_str(true, true, ")");
                }
                let eof_toks = (*self.eqtb.tok_params
                    [crate::prim::ToksParam::EveryEOF.idx() as usize])
                    .clone();
                if !eof_toks.is_empty() {
                    self.push_tokens_named(eof_toks, "<everyeof>");
                }
                return None;
            }
            let Some(buf) = line_buf else {
                let ending = *ending;
                // luatex next_line: a printed token object is read next
                // (it is backed up and the pseudo file is resumed after it).
                if !ending {
                    if let Some(token) = self.take_lua_token(si) {
                        return Some(token);
                    }
                }
                // \endinput takes effect once the current line is finished.
                if ending || !self.file_load_line(si) {
                    self.set_file_done(si);
                }
                continue;
            };
            let start = *line_pos;
            let state = *state;
            let regime = *cat_regime;
            let Some((mut character, mut width)) = self.decode_scalar(buf, start) else {
                // tex.web §360: an exhausted line moves to the next one in
                // state new_line.
                self.file_line_clear(si);
                continue;
            };
            // luatex str2uni/do_buffer_to_unichar: an invalid UTF-8
            // sequence reads as U+FFFD and skips utf8_size(0xFFFD) bytes.
            let invalid =
                self.engine_kind == EngineKind::LuaTeX && width == 1 && character >= 0x80;
            if invalid {
                character = 0xFFFD;
                width = 3.min(buf.len() - start);
            }
            self.file_line_advance_by(si, width);
            if invalid {
                self.error("String contains an invalid utf-8 sequence");
            }
            if let Some(token) = self.tokenize_char(character, si, state, start, regime) {
                if token != PAR_END && !self.file_line_is_none(si) {
                    self.record_physical_token(si, start, token);
                }
                return Some(token);
            }
        }
    }

    fn set_file_done(&mut self, si: usize) {
        if let Source::File { done, .. } = &mut self.input.stack[si] {
            *done = true;
        }
    }

    fn take_lua_token(&mut self, si: usize) -> Option<Token> {
        let Some(Source::File { lua_lines: Some(lines), .. }) = self.input.stack.get_mut(si) else {
            return None;
        };
        lines.lines.front()?.token?;
        lines.lines.pop_front()?.token
    }

    fn set_file_state(&mut self, si: usize, value: u8) {
        if let Source::File { state, .. } = &mut self.input.stack[si] {
            *state = value;
        }
    }

    #[inline]
    fn decode_scalar(&self, buf: &[u8], index: usize) -> Option<(u32, usize)> {
        decode_scalar(buf, index, self.engine_kind != EngineKind::PdfTeX)
    }

    fn file_line_advance_by(&mut self, si: usize, width: usize) {
        if let Some(Source::File { line_pos, .. }) = self.input.stack.get_mut(si) {
            *line_pos += width;
        }
    }

    /// Discard the rest of the current line (`loc:=limit+1`). The buffer is
    /// kept for reuse by the next line.
    fn file_line_clear(&mut self, si: usize) {
        if let Some(Source::File {
            line_buf, line_pos, ..
        }) = self.input.stack.get_mut(si)
        {
            if let Some(buf) = line_buf.take() {
                self.spare_line_buf = buf;
            }
            *line_pos = 0;
        }
    }

    fn file_line_is_none(&self, si: usize) -> bool {
        match self.input.stack.get(si) {
            Some(Source::File { line_buf, .. }) => line_buf.is_none(),
            _ => true,
        }
    }

    fn file_line_pos(&self, si: usize) -> usize {
        match self.input.stack.get(si) {
            Some(Source::File { line_pos, .. }) => *line_pos,
            _ => 0,
        }
    }

    /// Load next line into the buffer; false at EOF. tex.web §362: trailing
    /// spaces are removed and `\endlinechar`, if it is in 0..=255, is
    /// appended; scanning restarts in state new_line.
    fn file_load_line(&mut self, si: usize) -> bool {
        let end_line_char =
            self.eqtb.int_params[crate::prim::IntParam::EndLineChar.idx() as usize];
        let unicode = self.engine_kind != EngineKind::PdfTeX;
        let mut buf = std::mem::take(&mut self.spare_line_buf);
        if let Some(Source::File {
            lua_lines: Some(lines),
            ..
        }) = self.input.stack.get_mut(si)
        {
            let Some(line) = lines.lines.pop_front() else {
                self.spare_line_buf = buf;
                return false;
            };
            let last = lines.lines.is_empty();
            return self.load_lua_line(si, buf, line, last, end_line_char);
        }
        if let Some(Source::File { lua_reader, .. }) = self.input.stack.get(si) {
            let reader = *lua_reader;
            if reader != 0 {
                self.spare_line_buf = buf;
                return self.load_reader_line(si, reader, end_line_char, unicode);
            }
        }
        if self.engine_kind == EngineKind::XeTeX {
            return self.file_load_line_xetex(si, buf, end_line_char);
        }
        let Source::File {
            data,
            pos,
            line_no,
            line_start,
            state,
            name,
            ..
        } = &mut self.input.stack[si]
        else {
            return false;
        };
        if *pos >= data.len() {
            self.spare_line_buf = buf;
            return false;
        }
        let start = *pos;
        let (end, next) = physical_line_bounds(data, start);
        // luatex `\scantextokens` ends without an end-of-line character
        // (which is why luacode compares its lines with such a token list).
        let last_text_line = next >= data.len() && name.as_str() == "<scantextokens>";
        let mut content_end = end;
        while content_end > start && data[content_end - 1] == b' ' {
            content_end -= 1;
        }
        buf.clear();
        buf.extend_from_slice(&data[start..content_end]);
        *line_start = start;
        *pos = next;
        *line_no += 1;
        *state = 0;
        // luatex process_input_buffer: sees the line without \endlinechar
        if self.engine_kind == EngineKind::LuaTeX {
            self.lua_process_input_line(&mut buf);
        }
        let before = buf.len();
        if let Some(character) = u8::try_from(end_line_char).ok().filter(|_| !last_text_line) {
            if unicode && !character.is_ascii() {
                let mut encoded = [0u8; 4];
                buf.extend_from_slice(char::from(character).encode_utf8(&mut encoded).as_bytes());
            } else {
                buf.push(character);
            }
        }
        let Source::File {
            line_buf,
            line_end_len,
            line_pos,
            ..
        } = &mut self.input.stack[si]
        else {
            return false;
        };
        *line_end_len = (buf.len() - before) as u8;
        *line_buf = Some(buf);
        *line_pos = 0;
        true
    }

    /// A line of a file read through an `open_read_file` object: the line
    /// the reader returns passes `process_input_buffer` and receives the
    /// end-of-line character like a line of a real file.
    fn load_reader_line(&mut self, si: usize, reader: u32, end_line_char: i32, unicode: bool) -> bool {
        let Some(mut buf) = self.lua_reader_line(reader) else {
            return false;
        };
        self.lua_process_input_line(&mut buf);
        let before = buf.len();
        if let Ok(character) = u8::try_from(end_line_char) {
            if unicode && !character.is_ascii() {
                buf.extend_from_slice(char::from(character).encode_utf8(&mut [0u8; 4]).as_bytes());
            } else {
                buf.push(character);
            }
        }
        let Some(Source::File { line_buf, line_end_len, line_pos, line_no, state, .. }) = self.input.stack.get_mut(si) else {
            return false;
        };
        *line_end_len = (buf.len() - before) as u8;
        *line_buf = Some(buf);
        *line_pos = 0;
        *line_no += 1;
        *state = 0;
        true
    }

    /// luatex textoken.c next_line for a `tex.print` line: full lines lose
    /// trailing spaces and restart in state new_line; partial (`sprint`)
    /// lines keep both and the scanner state. The end-of-line character is
    /// appended only to full lines that are not the last queued line and
    /// are not read with "string" catcodes.
    fn load_lua_line(
        &mut self,
        si: usize,
        mut buf: Vec<u8>,
        line: crate::engine_lua::LuaLine,
        last: bool,
        end_line_char: i32,
    ) -> bool {
        let unicode = self.engine_kind != EngineKind::PdfTeX;
        buf.clear();
        buf.extend_from_slice(&line.text);
        if !line.partial {
            while buf.last() == Some(&b' ') {
                buf.pop();
            }
        }
        let before = buf.len();
        if !(last || line.partial || line.cattable == crate::engine_lua::NO_CAT_TABLE) {
            if let Ok(character) = u8::try_from(end_line_char) {
                if unicode && !character.is_ascii() {
                    buf.extend_from_slice(char::from(character).encode_utf8(&mut [0u8; 4]).as_bytes());
                } else {
                    buf.push(character);
                }
            }
        }
        let Source::File {
            line_buf,
            line_end_len,
            line_pos,
            line_no,
            state,
            cat_regime,
            ..
        } = &mut self.input.stack[si]
        else {
            return false;
        };
        *line_end_len = (buf.len() - before) as u8;
        *line_buf = Some(buf);
        *line_pos = 0;
        *line_no += 1;
        if !line.partial {
            *state = 0;
        }
        *cat_regime = line.cattable;
        true
    }

    /// luatex textoken.c `do_get_cat_code` for the current line's regime.
    #[inline]
    fn regime_cat_code(&self, regime: i32, character: u32) -> u8 {
        if regime == crate::engine_lua::DEFAULT_CAT_TABLE {
            self.eqtb.cat_code(character)
        } else {
            self.lua_line_cat_code(regime, character)
        }
    }

    fn source_character_token(&self, cat: u8, character: u32) -> Token {
        if self.engine_kind != EngineKind::PdfTeX && character > 127 {
            Token::unicode_char(cat, character)
        } else {
            Token::char(cat, character)
        }
    }

    /// tex.web §347: act on a consumed character in scanner `state`. None
    /// means the character produced no token; the caller keeps scanning.
    fn tokenize_char(
        &mut self,
        mut character: u32,
        si: usize,
        state: u8,
        start: usize,
        regime: i32,
    ) -> Option<Token> {
        loop {
            let cat = self.regime_cat_code(regime, character);
            let token = match cat {
                CAT_ESCAPE => return Some(self.scan_control_sequence(si)),
                CAT_IGNORED => return None,
                CAT_SPACE => {
                    if state != 1 {
                        return None;
                    }
                    self.set_file_state(si, 2);
                    // tex.web §348: every spacer token has character code 32.
                    return Some(Token::space());
                }
                CAT_EOL => {
                    // `loc:=limit+1`: the rest of the line is discarded.
                    self.file_line_clear(si);
                    return match state {
                        0 => Some(PAR_END),
                        1 => Some(Token::space()),
                        _ => None,
                    };
                }
                CAT_COMMENT => {
                    self.file_line_clear(si);
                    return None;
                }
                CAT_INVALID => {
                    self.invalid_character_error(si, character, start);
                    return self.stopped_on_error.then_some(EOF_MARKER);
                }
                CAT_SUPER => {
                    if let Some(expanded) = self.expand_sup(si, character) {
                        // `goto reswitch` with the translated character.
                        character = expanded;
                        continue;
                    }
                    self.source_character_token(CAT_SUPER, character)
                }
                _ => self.source_character_token(cat, character),
            };
            self.set_file_state(si, 1);
            return Some(token);
        }
    }

    /// tex.web §354-§357: scan the name after an escape character. Expanded
    /// `^^` codes inside the name are reduced in the buffer first.
    fn scan_control_sequence(&mut self, si: usize) -> Token {
        loop {
            let Source::File {
                line_buf: Some(buf),
                line_pos,
                cat_regime,
                ..
            } = &self.input.stack[si]
            else {
                unreachable!()
            };
            let regime = *cat_regime;
            let loc = *line_pos;
            let Some((first, first_width)) = self.decode_scalar(buf, loc) else {
                // The escape character ended the buffer: the null control
                // sequence (the state is irrelevant; the line is finished).
                return Token::from_cs(self.cs.intern(b""));
            };
            let first_cat = self.regime_cat_code(regime, first);
            let mut k = loc + first_width;
            let mut end = loc + first_width;
            let reduce_at = if first_cat == CAT_LETTER && k < buf.len() {
                let (mut character, mut width, mut cat);
                loop {
                    (character, width) = self.decode_scalar(buf, k).expect("k is inside the buffer");
                    cat = self.regime_cat_code(regime, character);
                    k += width;
                    if cat != CAT_LETTER || k >= buf.len() {
                        break;
                    }
                }
                end = if cat == CAT_LETTER { k } else { k - width };
                (cat == CAT_SUPER).then_some((k, character))
            } else {
                (first_cat == CAT_SUPER).then_some((k, first))
            };
            if let Some((k, character)) = reduce_at {
                if self.reduce_expanded_code(si, k, character) {
                    continue;
                }
            }
            let state = if first_cat == CAT_LETTER || first_cat == CAT_SPACE {
                2
            } else {
                1
            };
            if let Source::File {
                line_pos,
                state: file_state,
                ..
            } = &mut self.input.stack[si]
            {
                *line_pos = end;
                *file_state = state;
            }
            let Source::File {
                line_buf: Some(buf),
                ..
            } = &self.input.stack[si]
            else {
                unreachable!()
            };
            let id = if self.engine_kind == EngineKind::PdfTeX {
                self.cs.intern(&buf[loc..end])
            } else {
                // Unicode engines spell names in UTF-8; a byte produced by
                // `^^` reduction stands for the scalar of the same value.
                let mut name = std::mem::take(&mut self.cs_name_scratch);
                name.clear();
                let mut index = loc;
                while let Some((character, width)) =
                    decode_scalar(&buf[..end], index, true)
                {
                    let character = char::from_u32(character).unwrap_or(char::REPLACEMENT_CHARACTER);
                    name.extend_from_slice(character.encode_utf8(&mut [0u8; 4]).as_bytes());
                    index += width;
                }
                let id = self.cs.intern(&name);
                self.cs_name_scratch = name;
                id
            };
            return Token::from_cs(id);
        }
    }

    /// tex.web §355: when `buffer[k-1]` is a superscript character that
    /// begins `^^` notation, replace the notation by the character it
    /// denotes and close the gap. Returns false if no expanded code is there.
    fn reduce_expanded_code(&mut self, si: usize, k: usize, sup: u32) -> bool {
        if self.engine_kind != EngineKind::PdfTeX {
            return self.reduce_unicode_expanded_code(si, k, sup);
        }
        let Some(Source::File {
            line_buf: Some(buf),
            ..
        }) = self.input.stack.get_mut(si)
        else {
            return false;
        };
        let Some((value, width)) = sup_notation(buf, k, sup) else {
            return false;
        };
        buf[k - 1] = value;
        buf.drain(k..k + width);
        true
    }

    /// `reduce_expanded_code` of the Unicode engines: the longer forms, and
    /// the denoted scalar spelled in UTF-8 so the line still decodes
    /// (luatex stores `^^ad` as U+00AD, not a raw byte).
    #[inline(never)]
    fn reduce_unicode_expanded_code(&mut self, si: usize, k: usize, sup: u32) -> bool {
        let kind = self.engine_kind;
        let Some(Source::File {
            line_buf: Some(buf),
            ..
        }) = self.input.stack.get_mut(si)
        else {
            return false;
        };
        let (notation, error) = unicode_sup_notation(buf, k, sup, kind);
        if let Some((value, width)) = notation {
            let character = char::from_u32(value).expect("unicode_sup_notation denotes scalars");
            let mut encoded = [0u8; 4];
            let bytes = character.encode_utf8(&mut encoded).as_bytes();
            buf.splice(k - 1..k + width, bytes.iter().copied());
        }
        if let Some(message) = error {
            self.error(message);
        }
        notation.is_some()
    }

    /// tex.web §352: a superscript character at `loc - 1` followed by the
    /// same character and a 7-bit code is `^^` notation. Consume it and
    /// return the character it denotes.
    fn expand_sup(&mut self, si: usize, sup: u32) -> Option<u32> {
        if self.engine_kind != EngineKind::PdfTeX {
            return self.expand_unicode_sup(si, sup);
        }
        let Some(Source::File {
            line_buf: Some(buf),
            line_pos,
            ..
        }) = self.input.stack.get_mut(si)
        else {
            return None;
        };
        let (value, width) = sup_notation(buf, *line_pos, sup)?;
        *line_pos += width;
        Some(u32::from(value))
    }

    /// `expand_sup` of the Unicode engines (longer forms included).
    #[inline(never)]
    fn expand_unicode_sup(&mut self, si: usize, sup: u32) -> Option<u32> {
        let Some(Source::File {
            line_buf: Some(buf),
            line_pos,
            ..
        }) = self.input.stack.get(si)
        else {
            return None;
        };
        let (notation, error) = unicode_sup_notation(buf, *line_pos, sup, self.engine_kind);
        // luatex reports a malformed long form before reading the rest
        if let Some(message) = error {
            self.error(message);
        }
        let (value, width) = notation?;
        if let Some(Source::File { line_pos, .. }) = self.input.stack.get_mut(si) {
            *line_pos += width;
        }
        Some(value)
    }

    fn invalid_character_error(&mut self, si: usize, character: u32, byte_column: usize) {
        // Synthetic tokenizers (notably `\\read`) can retain the physical
        // command that supplied their bytes. Prefer that call site over a
        // misleading `<read>:1:1` location.
        let source = self
            .diagnostic_source_override
            .clone()
            .or_else(|| self.input.source_context_at(si, byte_column));
        let showing = if let Ok(byte) = u8::try_from(character) {
            if byte.is_ascii_graphic() || byte == b' ' {
                format!(
                    " (byte 0x{byte:02X}, '{}')",
                    char::from(byte).escape_default()
                )
            } else {
                format!(" (byte 0x{byte:02X})")
            }
        } else if let Some(character) = char::from_u32(character) {
            format!(
                " (U+{:04X}, '{}')",
                character as u32,
                character.escape_default()
            )
        } else {
            format!(" (invalid scalar U+{character:X})")
        };
        self.error_at(
            &format!("Text line contains an invalid character{showing}"),
            source,
        );
    }

    fn record_physical_token(&mut self, si: usize, byte_column: usize, token: Token) {
        let Some(line) = self.input.source_line_at(si) else {
            return;
        };
        let span = self.file_line_pos(si).saturating_sub(byte_column).max(1);
        let semantic_cs = if token.is_cs() {
            Some(token.cs_id())
        } else if token.is_char() && token.cc() == CAT_ACTIVE {
            Some(self.active_cs_id(token.chr()))
        } else {
            None
        };
        self.diagnostic_sources_live = true;
        self.diagnostic_physical_source = Some(PhysicalTokenSource {
            token,
            semantic_cs,
            source_index: si,
            line,
            byte_column,
            span,
        });
    }
}

/// Decode one source character. pdfTeX reads bytes; Unicode engines decode
/// UTF-8 and fall back to the byte value for malformed sequences.
#[inline]
fn decode_scalar(buf: &[u8], index: usize, unicode: bool) -> Option<(u32, usize)> {
    let first = *buf.get(index)?;
    if !unicode || first.is_ascii() {
        return Some((u32::from(first), 1));
    }
    let width = match first {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return Some((u32::from(first), 1)),
    };
    let decoded = buf
        .get(index..index + width)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| text.chars().next());
    Some(decoded.map_or((u32::from(first), 1), |character| (character as u32, width)))
}

/// tex.web §352/§355: `buf[k-1]` holds superscript character `sup`. If
/// `buf[k..]` continues it as `^^` notation (the same character, then a
/// 7-bit code), return the denoted character and the number of bytes from
/// `k` that the notation spans. Two lowercase hex digits denote a code
/// (pdfTeX: `^^4A` is `t` followed by `A`); any other code is toggled by 64.
fn sup_notation(buf: &[u8], k: usize, sup: u32) -> Option<(u8, usize)> {
    let sup = u8::try_from(sup).ok().filter(u8::is_ascii)?;
    if buf.get(k) != Some(&sup) {
        return None;
    }
    let c = *buf.get(k + 1).filter(|c| c.is_ascii())?;
    let hex = |x: u8| match x {
        b'0'..=b'9' => Some(x - b'0'),
        b'a'..=b'f' => Some(x - b'a' + 10),
        _ => None,
    };
    if let (Some(high), Some(low)) = (hex(c), buf.get(k + 2).copied().and_then(hex)) {
        return Some((high * 16 + low, 3));
    }
    Some((c ^ 0x40, 2))
}

/// `sup_notation` of the Unicode engines, which read longer forms first.
/// LuaTeX (textoken.c `process_sup_mark`) takes `^^^^XXXX` and
/// `^^^^^^XXXXXX`; when the hex digits are missing it returns the error it
/// reports and falls back to the two-character form. XeTeX (xetex.web
/// §355) counts up to six superscript characters and reads as many hex
/// digits, falls back silently, and leaves a value beyond U+10FFFF
/// unexpanded. A surrogate code point is not a character here and is
/// treated like that value.
fn unicode_sup_notation(
    buf: &[u8],
    k: usize,
    sup: u32,
    kind: EngineKind,
) -> (Option<(u32, usize)>, Option<&'static str>) {
    let Some(sup) = u8::try_from(sup).ok().filter(u8::is_ascii) else {
        return (None, None);
    };
    if buf.get(k) != Some(&sup) {
        return (None, None);
    }
    let hex_value = |from: usize, digits: usize| {
        buf.get(from..from + digits)?.iter().try_fold(0u32, |value, &digit| {
            let nibble = match digit {
                b'0'..=b'9' => digit - b'0',
                b'a'..=b'f' => digit - b'a' + 10,
                _ => return None,
            };
            Some(value * 16 + u32::from(nibble))
        })
    };
    let mut error = None;
    match kind {
        EngineKind::LuaTeX if buf.get(k + 1) == Some(&sup) && buf.get(k + 2) == Some(&sup) => {
            let six = buf.get(k + 3) == Some(&sup) && buf.get(k + 4) == Some(&sup);
            let (from, digits) = if six { (k + 5, 6) } else { (k + 3, 4) };
            if from + digits > buf.len() {
                error = Some(if six {
                    "^^^^^^ needs six hex digits, end of input"
                } else {
                    "^^^^ needs four hex digits, end of input"
                });
            } else if let Some(value) = hex_value(from, digits) {
                if char::from_u32(value).is_some() {
                    return (Some((value, from + digits - k)), None);
                }
            } else {
                error = Some(if six {
                    "^^^^^^ needs six hex digits"
                } else {
                    "^^^^ needs four hex digits"
                });
            }
        }
        EngineKind::XeTeX => {
            let mut count = 2;
            while count < 6 && k + 2 * count - 2 < buf.len() && buf.get(k + count - 1) == Some(&sup) {
                count += 1;
            }
            if let Some(value) = hex_value(k + count - 1, count) {
                if char::from_u32(value).is_none() {
                    return (None, None);
                }
                return (Some((value, 2 * count - 1)), None);
            }
        }
        _ => {}
    }
    let notation = sup_notation(buf, k, u32::from(sup)).map(|(value, width)| (u32::from(value), width));
    (notation, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carriage_return_terminates_physical_lines() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine
            .input
            .push_file("legacy-cr.tex".into(), b"% comment\rA\r".to_vec());

        assert_eq!(engine.get_next_raw(), Token::letter(b'A'));
        let context = engine.input.current_source_mark().unwrap().to_context();
        assert_eq!(context.line, 2);
        assert_eq!(context.text, "A");
    }

    #[test]
    fn trailing_spaces_stripped_from_physical_line_before_endlinechar() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.eqtb.cat[13] = 12;
        engine.eqtb.int_params[crate::prim::IntParam::EndLineChar.idx() as usize] = 13;
        engine
            .input
            .push_file("trailing.tex".into(), b"X   \n".to_vec());

        assert_eq!(engine.get_next_raw(), Token::letter(b'X'));
        assert_eq!(engine.get_next_raw(), Token::char(12, 13));
        assert_eq!(engine.get_next_raw(), crate::input::EOF_MARKER);
    }

    #[test]
    fn everyeof_tokens_inserted_at_file_eof() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.eqtb.tok_params[crate::prim::ToksParam::EveryEOF.idx() as usize] =
            std::rc::Rc::new(vec![Token::letter(b'Z')]);
        engine.input.push_file("sub.tex".into(), b"A".to_vec());

        assert_eq!(engine.get_next_raw(), Token::letter(b'A'));
        assert_eq!(engine.get_next_raw(), Token::space());
        assert_eq!(engine.get_next_raw(), Token::letter(b'Z'));
        assert_eq!(engine.get_next_raw(), crate::input::EOF_MARKER);
    }
    #[test]
    fn unicode_profiles_decode_source_scalars_without_changing_pdftex_bytes() {
        let source = "é".as_bytes().to_vec();

        let mut unicode = Engine::new_with_kind(EngineKind::XeTeX, true);
        unicode.init_primitives();
        unicode
            .input
            .push_file("unicode.tex".into(), source.clone());
        assert_eq!(
            unicode.get_next_raw(),
            Token::unicode_char(CAT_OTHER, 'é' as u32)
        );

        let mut pdftex = Engine::new_with_kind(EngineKind::PdfTeX, true);
        pdftex.init_primitives();
        pdftex.input.push_file("bytes.tex".into(), source);
        assert_eq!(
            pdftex.get_next_raw(),
            Token::char(CAT_OTHER, u32::from("é".as_bytes()[0]))
        );
    }

    #[test]
    fn unicode_letters_and_active_characters_keep_scalar_identity() {
        let mut engine = Engine::new_with_kind(EngineKind::LuaTeX, true);
        engine.init_primitives();
        engine.eqtb.assign_cat_code('界' as u32, CAT_LETTER, true);
        engine.eqtb.assign_cat_code('🦀' as u32, CAT_ACTIVE, true);
        engine
            .input
            .push_file("unicode.tex".into(), "\\界 🦀".as_bytes().to_vec());

        let control_word = engine.get_next_raw();
        assert!(control_word.is_cs());
        assert_eq!(engine.cs.name(control_word.cs_id()), "界".as_bytes());
        assert_eq!(
            engine.get_next_raw(),
            Token::unicode_char(CAT_ACTIVE, '🦀' as u32)
        );
    }
    #[test]
    fn unicode_active_definition_source_keeps_brace_tokens() {
        let mut engine = Engine::new_with_kind(EngineKind::XeTeX, true);
        engine.init_primitives();
        engine.eqtb.assign_cat_code('🦀' as u32, CAT_ACTIVE, true);
        engine.eqtb.assign_cat(b'{', CAT_BGROUP, true);
        engine.eqtb.assign_cat(b'}', CAT_EGROUP, true);
        engine
            .input
            .push_file("unicode.tex".into(), "\\def🦀{OK}".as_bytes().to_vec());

        let definition = engine.get_next_raw();
        assert_eq!(engine.cs.name(definition.cs_id()), b"def");
        assert_eq!(
            engine.get_next_raw(),
            Token::unicode_char(CAT_ACTIVE, '🦀' as u32)
        );
        assert_eq!(engine.get_next_raw(), Token::char(CAT_BGROUP, b'{' as u32));
        assert_eq!(engine.get_next_raw(), Token::letter(b'O'));
        assert_eq!(engine.get_next_raw(), Token::letter(b'K'));
        assert_eq!(engine.get_next_raw(), Token::char(CAT_EGROUP, b'}' as u32));
    }
}
