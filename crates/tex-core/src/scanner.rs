//! Tokenizer: raw token production from input sources with TeX's N/M/S
//! scanning states, ^^ notation, comments, control-sequence scanning.
//!
//! Line model: each file source holds the raw bytes; the current line is
//! materialized in `line_buf` (without its newline). "End of line" is
//! `line_peek() == None`. State: 0 = new line (N), 1 = mid line (M),
//! 2 = skip spaces (S).

use crate::engine::Engine;
use crate::input::{PAR_END, Source, EOF_MARKER};
use crate::token::*;

impl Engine {
    /// Fetch the next raw token from the input stack (no expansion).
    pub fn get_next_raw(&mut self) -> Token {
        loop {
            if self.input.stack.is_empty() {
                return EOF_MARKER;
            }
            let si = self.input.stack.len() - 1;
            match self.input.stack[si] {
                Source::TokList { .. } => {
                    if let Some(t) = self.toklist_next(si) {
                        return t;
                    }
                    continue; // list popped; retry
                }
                Source::File { .. } => match self.file_next_token(si) {
                    Some(t) => return t,
                    None => continue, // file popped; retry
                },
            }
        }
    }

    pub(crate) fn toklist_next(&mut self, si: usize) -> Option<Token> {

        let s = match &mut self.input.stack[si] {
            Source::TokList { toks, pos, params, param_idx, param_pos, in_param, name, .. } => {
                loop {
                    if *in_param {
                        let p = &params[*param_idx];
                        if *param_pos < p.len() {
                            let t = p[*param_pos];
                            *param_pos += 1;
                            if crate::debug_flag("SPTRACE") && t == crate::token::Token::space() {
                                eprintln!("SPPOP2 list={} idx={}", name, param_idx);
                            }
                            return Some(t);
                        }
                        *in_param = false;
                        *param_idx += 1;
                        continue;
                    }
                    if *pos < toks.len() {
                        let t = toks[*pos];
                        *pos += 1;
                        // tex.web end_token_list: when the last token of the
                        // list is consumed the source is retired IMMEDIATELY
                        // (before the read token expands). Lazy popping made
                        // tail-recursive macro loops (l3 \ior_map) accumulate
                        // exhausted replay sources below the live recursion
                        // until the 50k input-stack guard fired.
                        let last = *pos == toks.len();
                        if crate::debug_flag("SPTRACE") && t == crate::token::Token::space() {
                            eprintln!("SPPOP list={} pos={}", name, pos);
                        }
                        if t.0 >= 0x4000_0000 && t.0 < 0x8000_0000 {
                            let n = (t.0 & 0x3FFF_FFFF) as usize;
                            if n >= 1 && n <= params.len() {
                                *param_idx = n - 1;
                                *param_pos = 0;
                                *in_param = true;
                                if crate::debug_flag("SUBTRACE") {
                                    eprintln!("SUB-enter {} idx={} arglen={}", name, n, params[n-1].len());
                                }
                                continue;
                            } else if crate::debug_flag("SUBTRACE") {
                                eprintln!("SUB-OOR {} n={} params={}", name, n, params.len());
                            }
                        }
                        if crate::debug_flag("SUBTRACE") {
                            let nm = if t.is_cs() { String::from_utf8_lossy(self.cs.name(t.cs_id())).into_owned() } else { format!("cc{}", t.cc()) };
                            eprintln!("TOK {} pos={} tok={}", name, pos, nm);
                        }
                        if last {
                            self.input.stack.remove(si);
                        }
                        return Some(t);
                    }
                    break;
                }
            }
            _ => unreachable!(),
        };
        let _ = s;
        self.input.stack.remove(si);
        None
    }

    /// One token from the file source at `si`; None if the file is exhausted
    /// (in which case the source is popped by the caller via get_next_raw? no:
    /// file_finished pops here).
    fn file_next_token(&mut self, si: usize) -> Option<Token> {
        let r = self.file_next_token_inner(si);
        if crate::debug_flag("FILETRACE") {
            let ln = match self.input.stack.get(si) { Some(Source::File { line_no, .. }) => *line_no, _ => 0 };
            let nm = match r { Some(t) if t.is_cs() => format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))), Some(t) => format!("cc{}:{}", t.cc(), t.chr()), None => "POP".into() };
            eprintln!("FTOK si={} line={} -> {}", si, ln, nm);
        }
        r
    }

    fn file_line_peek(&self, si: usize) -> Option<u8> {
        match self.input.stack.get(si) {
            Some(Source::File { line_buf: Some(buf), line_pos, .. }) => buf.get(*line_pos).copied(),
            _ => None,
        }
    }

    fn file_line_advance(&mut self, si: usize) {
        if let Some(Source::File { line_pos, .. }) = self.input.stack.get_mut(si) {
            *line_pos += 1;
        }
    }

    fn file_line_clear(&mut self, si: usize) {
        if let Some(Source::File { line_buf, line_pos, line_reload, .. }) = self.input.stack.get_mut(si) {
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
            let (done, at_eof) = match &self.input.stack[si] {
                Source::File { done, at_eof, .. } => (*done, *at_eof),
                _ => unreachable!(),
            };
            if done {
                if crate::debug_flag("SPTRACE") {
                    let nm = match &self.input.stack.get(si) { Some(crate::input::Source::File { name, .. }) => name.clone(), _ => "?".into() };
                    let ev = self.eqtb.tok_params[crate::prim::ToksParam::EveryEOF.idx() as usize].len();
                    eprintln!("FILE-POP si={} name={} everyeof={}", si, nm, ev);
                }
                // l3's rescan protocol (\tl_set_rescan) relies on this to
                // terminate its delimited scans with the marker.
                let is_scantokens = match &self.input.stack.get(si) {
                    Some(crate::input::Source::File { name, .. }) => name == "<scantokens>",
                    _ => false,
                };
                // e-TeX semantics: \everyeof fires EVERY time scanning
                // crosses the pseudo-file end (the l3 single-rescan chain
                // re-enters deliberately); no one-shot guard.
                if is_scantokens {
                    let eof_toks = (*self.eqtb.tok_params[crate::prim::ToksParam::EveryEOF.idx() as usize]).clone();
                    if !eof_toks.is_empty() {
                        self.push_tokens(eof_toks);
                        return None;
                    }
                }
                self.input.stack.remove(si);
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
                        let el = self.eqtb.int_params[crate::prim::IntParam::EndLineChar.idx() as usize];
                        let eol = el < 0
                            || el > 255
                            || self.eqtb.cat[el as usize] == CAT_EOL;
                        self.file_line_clear(si);
                        if eol {
                            return Some(PAR_END);
                        }
                        continue;
                    }
                    let b = self.file_line_peek(si).unwrap();
                    self.file_line_advance(si);
                    let s = match &mut self.input.stack[si] {
                        Source::File { state, .. } => state,
                        _ => unreachable!(),
                    };
                    *s = 1;
                    if let Some(t) = self.tokenize_char(b, si) {
                        return Some(t);
                    }
                    // comment or otherwise consumed rest of line: loop
                }
                1 => {
                    let b = match self.file_line_peek(si) {
                        Some(b) => b,
                        None => {
                            // end of line: endline char token (usually space)
                            let el = self.eqtb.int_params[crate::prim::IntParam::EndLineChar.idx() as usize];
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
                            if crate::debug_flag("DEFTRACE") {
                                eprintln!("EOL state->0");
                            }
                            if cat == CAT_EOL {
                                return Some(Token::space());
                            }
                            if cat == CAT_IGNORED || cat == CAT_INVALID {
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
                    self.file_line_advance(si);
                    let cat_b = self.eqtb.cat[b as usize];
                    if let Some(t) = self.tokenize_char(b, si) {
                        if cat_b == CAT_SPACE {
                            // mid_line + spacer: state <- skip_blanks
                            let s2 = match &mut self.input.stack[si] {
                                Source::File { state, .. } => state,
                                _ => unreachable!(),
                            };
                            *s2 = 2;
                        }
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
                            // line end in skip-spaces: no space token
                            self.file_line_clear(si);
                            continue;
                        }
                    };
                    self.file_line_advance(si);
                    let s = match &mut self.input.stack[si] {
                        Source::File { state, .. } => state,
                        _ => unreachable!(),
                    };
                    *s = 1;
                    if let Some(t) = self.tokenize_char(b, si) {
                        return Some(t);
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    /// Load next line into the buffer; false at EOF.
    fn file_load_line(&mut self, si: usize) -> bool {
        let chunk = match &mut self.input.stack[si] {
            Source::File { name, data, pos, line_no, .. } => {
                if *pos >= data.len() {
                    return false;
                }
                let rest = &data[*pos..];
                let nl = rest.iter().position(|&b| b == b'\n').map(|i| i + 1).unwrap_or(rest.len());
                let mut line = rest[..nl].to_vec();
                *pos += nl;
                *line_no += 1;
                if name.ends_with("latex.ltx") && *line_no % 1000 == 0 {
                    eprintln!("PROGRESS: {} line {}", name.split('/').last().unwrap_or(name), *line_no);
                }
                if line.last() == Some(&b'\n') {
                    line.pop();
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                }
                line
            }
            _ => return false,
        };




        if let Some(Source::File { line_buf, line_pos, line_reload, .. }) = self.input.stack.get_mut(si) {
            *line_buf = Some(chunk);
            *line_pos = 0;
            *line_reload = false;
        }
        true
    }

    /// Tokenize a consumed character. None = rest of line consumed (comment);
    /// caller should re-loop.
    fn tokenize_char(&mut self, b: u8, si: usize) -> Option<Token> {
        let cat = self.eqtb.cat[b as usize];
        if crate::debug_flag("TOKTRACE") {
            eprintln!("TOK b={:?} ({}) cat={}", b as char, b, cat);
        }
        match cat {
            CAT_COMMENT => {
                self.file_line_clear(si);
                let s = match &mut self.input.stack[si] {
                    Source::File { state, .. } => state,
                    _ => unreachable!(),
                };
                if *s == 1 {
                    *s = 2;
                }
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
                            // escape at line end: empty cs; endline consumed
                            self.file_line_clear(si);
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
                let c = self.expand_sup(b, si);
                let ccat = self.eqtb.cat[c as usize];
                if ccat == CAT_ACTIVE {
                    Some(Token::char(13, c as u32))
                } else if ccat == CAT_SPACE {
                    Some(Token::space())
                } else {
                    Some(Token::char(ccat, c as u32))
                }
            }
            CAT_ACTIVE => Some(Token::char(13, b as u32)),

            CAT_SPACE => {
                // tex.web §347: every spacer token has character code 32
                Some(Token::space())
            }
            CAT_IGNORED | CAT_INVALID => None,
            _ => Some(Token::char(cat, b as u32)),
        }
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
                        if x <= b'9' { x - b'0' } else { x - b'a' + 10 }
                    };
                    c = hv(b3) * 16 + hv(b4);
                    continue;
                }
            }
            c = b3 ^ 64;
        }
    }
}
