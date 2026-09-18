//! Tokenizer: raw token production from input sources with TeX's N/M/S
//! scanning states, ^^ notation, comments, control-sequence scanning.
//!
//! Line model: each file source holds the raw bytes; the current line is
//! materialized in `line_buf` (without its newline). "End of line" is
//! `line_peek() == None`. State: 0 = new line (N), 1 = mid line (M),
//! 2 = skip spaces (S).

use crate::engine::{Engine, PhysicalTokenSource};
use crate::input::{physical_line_bounds, Source, EOF_MARKER, PAR_END};
use crate::token::*;

impl Engine {
    /// Fetch the next raw token from the input stack (no expansion).
    pub fn get_next_raw(&mut self) -> Token {
        self.diagnostic_synthetic_source = None;
        self.diagnostic_physical_source = None;
        loop {
            if self.input.stack.is_empty() {
                return EOF_MARKER;
            }
            let si = self.input.stack.len() - 1;
            match self.input.stack[si] {
                Source::TokList { .. } | Source::MacroFrame(_) => {
                    if let Some(t) = self.toklist_next(si) {
                        self.diagnostic_token_from_file = false;
                        return t;
                    }
                    continue; // list popped; retry
                }
                Source::File { .. } => {
                    // Set the origin before tokenization because an invalid
                    // character reports from inside `file_next_token`.
                    self.diagnostic_token_from_file = true;
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
            Source::TokList { trace_depth, .. } => *trace_depth as usize,
            Source::MacroFrame(frame) => frame.trace_depth as usize,
            _ => unreachable!(),
        };
        if !self.align_macro_arg && self.diagnostic_trace_hold == 0 {
            self.diagnostic_macro_trace.truncate(trace_depth);
        }
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

    /// One token from the file source at `si`; None if the file is exhausted
    /// (in which case the source is popped by the caller via get_next_raw? no:
    /// file_finished pops here).
    fn file_next_token(&mut self, si: usize) -> Option<Token> {
        let r = self.file_next_token_inner(si);
        r
    }

    fn file_line_peek(&self, si: usize) -> Option<u8> {
        match self.input.stack.get(si) {
            Some(Source::File {
                line_buf: Some(buf),
                line_pos,
                ..
            }) => buf.get(*line_pos).copied(),
            _ => None,
        }
    }

    fn file_line_advance(&mut self, si: usize) {
        if let Some(Source::File { line_pos, .. }) = self.input.stack.get_mut(si) {
            *line_pos += 1;
        }
    }

    fn file_line_clear(&mut self, si: usize) {
        if let Some(Source::File {
            line_buf,
            line_pos,
            line_reload,
            ..
        }) = self.input.stack.get_mut(si)
        {
            *line_buf = None;
            *line_pos = 0;
            *line_reload = true;
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

    fn file_next_token_inner(&mut self, si: usize) -> Option<Token> {
        loop {
            let (done, _at_eof) = match &self.input.stack[si] {
                Source::File { done, at_eof, .. } => (*done, *at_eof),
                _ => unreachable!(),
            };
            if done {
                // e-TeX semantics: \everyeof fires every time scanning
                // reaches EOF of an input file or pseudo-file (expl3 \file_get
                // and \tl_set_rescan rely on this to supply closing delimiters).
                self.input.finish_file(si);
                let eof_toks = (*self.eqtb.tok_params
                    [crate::prim::ToksParam::EveryEOF.idx() as usize])
                    .clone();
                if !eof_toks.is_empty() {
                    self.push_tokens(eof_toks);
                }
                return None;
            }
            // honor \endinput: stop at end of the current line
            if self.file_line_is_none(si) {
                let ending = match &self.input.stack.get(si) {
                    Some(crate::input::Source::File { ending, .. }) => *ending,
                    _ => false,
                };
                if ending {
                    let d = match &mut self.input.stack[si] {
                        crate::input::Source::File { done, .. } => done,
                        _ => unreachable!(),
                    };
                    *d = true;
                    continue;
                }
            }
            // ensure a line buffer
            if self.file_line_is_none(si) {
                if !self.file_load_line(si) {
                    let d = match &mut self.input.stack[si] {
                        Source::File { done, .. } => done,
                        _ => unreachable!(),
                    };
                    *d = true;
                    continue;
                }
            }
            let st = match &self.input.stack[si] {
                Source::File { state, .. } => *state,
                _ => unreachable!(),
            };
            match st {
                0 => {
                    // new line state (tex.web 343): skip leading spaces and
                    // ignored chars; if nothing remains, the virtual endline
                    // char decides — cat 5 yields \par, anything else (e.g.
                    // \nfss@catcodes sets ^^M to 9 for .fd files) yields no
                    // token. Spaces-only lines count as blank.
                    let mut any = false;
                    while let Some(b) = self.file_line_peek(si) {
                        let cat = self.eqtb.cat[b as usize];
                        if cat == CAT_SPACE || cat == CAT_IGNORED {
                            self.file_line_advance(si);
                            continue;
                        }
                        any = true;
                        break;
                    }
                    if !any && self.file_line_peek(si).is_none() {
                        let el =
                            self.eqtb.int_params[crate::prim::IntParam::EndLineChar.idx() as usize];
                        self.file_line_clear(si);
                        if !(0..=255).contains(&el) {
                            continue;
                        }
                        let cat = self.eqtb.cat[el as usize];
                        match cat {
                            CAT_EOL => return Some(PAR_END),
                            CAT_SPACE | CAT_COMMENT | CAT_IGNORED => continue,
                            CAT_INVALID => {
                                self.invalid_character_error(si, el as u8, usize::MAX);
                                if self.stopped_on_error {
                                    return Some(EOF_MARKER);
                                }
                                continue;
                            }
                            CAT_ACTIVE => return Some(Token::char(CAT_ACTIVE, el as u32)),
                            _ => return Some(Token::char(cat, el as u32)),
                        }
                    }
                    let b = self.file_line_peek(si).unwrap();
                    let token_start = self.file_line_pos(si);
                    self.file_line_advance(si);
                    if let Some(t) = self.tokenize_char(b, si) {
                        // ^^ notation is translated before TeX applies the
                        // N/M/S state machine. A translated space at the start
                        // of a line is therefore ignored, just like a literal
                        // leading space.
                        if t.is_char() && t.cc() == CAT_SPACE {
                            continue;
                        }
                        if let Some(Source::File { state, .. }) = self.input.stack.get_mut(si) {
                            if *state == 0 {
                                *state = 1;
                            }
                        }
                        self.record_physical_token(si, token_start, t);
                        return Some(t);
                    }
                    // Comments reset the state themselves. Ignored characters,
                    // including ^^ translations, leave new-line state intact.
                    // comment or otherwise consumed rest of line: loop
                }
                1 => {
                    let b = match self.file_line_peek(si) {
                        Some(b) => b,
                        None => {
                            // end of line: endline char token (usually space)
                            let el = self.eqtb.int_params
                                [crate::prim::IntParam::EndLineChar.idx() as usize];
                            let s = match &mut self.input.stack[si] {
                                Source::File { state, .. } => state,
                                _ => unreachable!(),
                            };
                            *s = 0;
                            self.file_line_clear(si);
                            if el < 0 {
                                continue;
                            }
                            if el < 0 || el > 255 {
                                // tex.web: endlinechar outside 0..255 injects
                                // space_token (which \endlinechar=-1 makes
                                // impossible: TeX uses the value directly as a
                                // token; negative => no token at all)
                                continue;
                            }
                            let cat = self.eqtb.cat[el as usize];

                            if cat == CAT_EOL {
                                return Some(Token::space());
                            }
                            if cat == CAT_IGNORED {
                                continue;
                            }
                            if cat == CAT_INVALID {
                                self.invalid_character_error(si, el as u8, usize::MAX);
                                if self.stopped_on_error {
                                    return Some(EOF_MARKER);
                                }
                                continue;
                            }
                            // tex.web §347: every spacer token has character code 32.
                            // \\ProvidesFile sets \\catcode\\endlinechar=10; that
                            // spacer must \\ifx-equal \\@sptoken (chr 32) so
                            // \\@ifnextchar sees the optional '[' on the next line.
                            if cat == CAT_SPACE {
                                return Some(Token::space());
                            }
                            return Some(Token::char(cat, el as u32));
                        }
                    };
                    let token_start = self.file_line_pos(si);
                    self.file_line_advance(si);
                    if let Some(t) = self.tokenize_char(b, si) {
                        if t.is_char() && t.cc() == CAT_SPACE {
                            // mid_line + spacer: state <- skip_blanks
                            let s2 = match &mut self.input.stack[si] {
                                Source::File { state, .. } => state,
                                _ => unreachable!(),
                            };
                            *s2 = 2;
                        }
                        self.record_physical_token(si, token_start, t);
                        return Some(t);
                    }
                }
                2 => {
                    // skip spaces
                    let mut skipped = false;
                    while let Some(b) = self.file_line_peek(si) {
                        let cat = self.eqtb.cat[b as usize];
                        if cat == CAT_SPACE {
                            self.file_line_advance(si);
                            skipped = true;
                            continue;
                        }
                        break;
                    }
                    let _ = skipped;
                    let b = match self.file_line_peek(si) {
                        Some(b) => b,
                        None => {
                            // tex.web skip_blanks at line end: state <- new_line,
                            // so the NEXT line is re-examined by the new-line logic
                            // (an empty next line must still yield PAR_END — it
                            // never did from here, swallowing blank-line \par
                            // after every scan that ended in optional-space state)
                            let s = match &mut self.input.stack[si] {
                                Source::File { state, .. } => state,
                                _ => unreachable!(),
                            };
                            *s = 0;
                            self.file_line_clear(si);
                            continue;
                        }
                    };
                    let token_start = self.file_line_pos(si);
                    self.file_line_advance(si);
                    if let Some(t) = self.tokenize_char(b, si) {
                        // A ^^-translated space is still skipped in state S.
                        if t.is_char() && t.cc() == CAT_SPACE {
                            continue;
                        }
                        // A control word (including one produced by ^^
                        // translation) leaves TeX in skip-blanks state.
                        // Ordinary characters leave it in mid-line state;
                        // control symbols already selected that state in
                        // tokenize_char.
                        if !t.is_cs() {
                            if let Some(Source::File { state, .. }) = self.input.stack.get_mut(si) {
                                if *state == 2 {
                                    *state = 1;
                                }
                            }
                        }
                        self.record_physical_token(si, token_start, t);
                        return Some(t);
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    /// Load next line into the buffer; false at EOF.
    fn file_load_line(&mut self, si: usize) -> bool {
        let (chunk, start) = match &mut self.input.stack[si] {
            Source::File {
                name: _,
                data,
                pos,
                line_no,
                ..
            } => {
                if *pos >= data.len() {
                    return false;
                }
                let start = *pos;
                let (end, next) = physical_line_bounds(data, start);
                let mut content_end = end;
                // tex.web §31 input_ln: Trailing blanks are removed from the line;
                // thus, either last=first or buffer[last-1]<>" ".
                while content_end > start && data[content_end - 1] == b' ' {
                    content_end -= 1;
                }
                let line = data[start..content_end].to_vec();
                *pos = next;
                *line_no += 1;
                (line, start)
            }
            _ => return false,
        };

        if let Some(Source::File {
            line_buf,
            line_pos,
            line_reload,
            line_start,
            ..
        }) = self.input.stack.get_mut(si)
        {
            *line_buf = Some(chunk);
            *line_pos = 0;
            *line_reload = false;
            *line_start = start;
        }
        true
    }

    /// Tokenize a consumed character. None = rest of line consumed (comment);
    /// caller should re-loop.
    fn tokenize_char(&mut self, b: u8, si: usize) -> Option<Token> {
        let cat = self.eqtb.cat[b as usize];

        match cat {
            CAT_COMMENT => {
                self.file_line_clear(si);
                let s = match &mut self.input.stack[si] {
                    Source::File { state, .. } => state,
                    _ => unreachable!(),
                };
                // A comment discards this line, not the next line's
                // paragraph boundary.
                *s = 0;
                None
            }
            CAT_ESCAPE => {
                // scan control-sequence name
                let mut name: Vec<u8> = Vec::new();
                let mut end_state = 1u8;
                loop {
                    let nb = match self.file_line_peek(si) {
                        Some(b) => b,
                        None => {
                            if name.is_empty() {
                                // TeX appends endlinechar before scanning an
                                // escape: a trailing backslash names that
                                // character, not the null control sequence.
                                let el = self.eqtb.int_params
                                    [crate::prim::IntParam::EndLineChar.idx() as usize];
                                if (0..=255).contains(&el) {
                                    name.push(el as u8);
                                }
                                self.file_line_clear(si);
                                end_state = 0;
                            }
                            // control word complete at line end: KEEP the
                            // exhausted buffer so the next line is re-examined
                            // by the new-line logic — clearing here pre-loaded
                            // the following line in skip_blanks state and
                            // swallowed its blank-line \par
                            break;
                        }
                    };
                    if name.is_empty() {
                        // first char: ^^ notation applies; decides word vs symbol
                        self.file_line_advance(si);
                        let c = self.expand_sup(nb, si);
                        let ccat = self.eqtb.cat[c as usize];
                        if ccat == CAT_LETTER {
                            name.push(c);
                            // control word: following blanks are skipped (tex.web
                            // state <- skip_blanks after a letter-class cs)
                            end_state = 2;
                            continue;
                        }
                        // single-char control symbol (or active char via ^^)
                        name.push(c);
                        if c == b' ' {
                            end_state = 2;
                        }
                        break;
                    }
                    // continuation: raw bytes, letters only (tex.web); the
                    // terminator byte stays in the buffer for the next token
                    let ccat = self.eqtb.cat[nb as usize];
                    if ccat == CAT_LETTER {
                        self.file_line_advance(si);
                        name.push(nb);
                        continue;
                    }
                    break;
                }
                let s = match &mut self.input.stack[si] {
                    Source::File { state, .. } => state,
                    _ => unreachable!(),
                };
                *s = end_state;
                let id = self.cs.intern(&name);
                Some(Token::from_cs(id))
            }
            CAT_SUPER => {
                let before = self.file_line_pos(si);
                let c = self.expand_sup(b, si);
                if self.file_line_pos(si) == before {
                    // A lone superscript character is an ordinary catcode-7
                    // token. Re-dispatch only when ^^ notation consumed input.
                    Some(Token::char(CAT_SUPER, b as u32))
                } else if self.eqtb.cat[c as usize] == CAT_INVALID {
                    self.invalid_character_error(si, c, before.saturating_sub(1));
                    if self.stopped_on_error {
                        Some(EOF_MARKER)
                    } else {
                        None
                    }
                } else {
                    self.tokenize_char(c, si)
                }
            }
            CAT_ACTIVE => Some(Token::char(13, b as u32)),

            CAT_SPACE => {
                // tex.web §347: every spacer token has character code 32
                Some(Token::space())
            }
            CAT_IGNORED => None,
            CAT_INVALID => {
                let column = self.file_line_pos(si).saturating_sub(1);
                self.invalid_character_error(si, b, column);
                if self.stopped_on_error {
                    Some(EOF_MARKER)
                } else {
                    None
                }
            }
            _ => Some(Token::char(cat, b as u32)),
        }
    }

    fn invalid_character_error(&mut self, si: usize, byte: u8, byte_column: usize) {
        // Synthetic tokenizers (notably `\\read`) can retain the physical
        // command that supplied their bytes.  Prefer that call site over a
        // misleading `<read>:1:1` location.
        let source = self
            .diagnostic_source_override
            .clone()
            .or_else(|| self.input.source_context_at(si, byte_column));
        let showing = if byte.is_ascii_graphic() || byte == b' ' {
            format!(
                " (byte 0x{byte:02X}, '{}')",
                char::from(byte).escape_default()
            )
        } else {
            format!(" (byte 0x{byte:02X})")
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
            Some(self.active_cs_id(token.chr() as u8))
        } else {
            None
        };
        self.diagnostic_physical_source = Some(PhysicalTokenSource {
            token,
            semantic_cs,
            source_index: si,
            line,
            byte_column,
            span,
        });
    }

    /// ^^-notation expansion with repetition (tex.web §377): the result
    /// re-enters the loop if its catcode is superscript again. Hex pairs use
    /// lowercase digits only (verified against pdfTeX: `^^4A` -> 't'+'A').
    fn expand_sup(&mut self, first: u8, si: usize) -> u8 {
        let mut c = first;
        loop {
            if self.eqtb.cat[c as usize] != CAT_SUPER {
                return c;
            }
            let b2 = match self.file_line_peek(si) {
                Some(b) => b,
                None => return c,
            };
            if b2 != c {
                return c;
            }
            self.file_line_advance(si); // consume second matching char
            let b3 = match self.file_line_peek(si) {
                Some(b) => b,
                None => return c,
            };
            self.file_line_advance(si);
            let is_hex = |x: u8| matches!(x, b'0'..=b'9' | b'a'..=b'f');
            if is_hex(b3) {
                let b4 = match self.file_line_peek(si) {
                    Some(b) => b,
                    None => {
                        c = b3 ^ 64;
                        continue;
                    }
                };
                if is_hex(b4) {
                    self.file_line_advance(si);
                    let hv = |x: u8| -> u8 {
                        if x <= b'9' {
                            x - b'0'
                        } else {
                            x - b'a' + 10
                        }
                    };
                    c = hv(b3) * 16 + hv(b4);
                    continue;
                }
            }
            c = b3 ^ 64;
        }
    }
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
        engine
            .input
            .push_file("sub.tex".into(), b"A".to_vec());

        assert_eq!(engine.get_next_raw(), Token::letter(b'A'));
        assert_eq!(engine.get_next_raw(), Token::space());
        assert_eq!(engine.get_next_raw(), Token::letter(b'Z'));
        assert_eq!(engine.get_next_raw(), crate::input::EOF_MARKER);
    }
}
