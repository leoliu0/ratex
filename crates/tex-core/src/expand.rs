//! Token expansion: get_token, macro expansion, conditionals, \csname and
//! the string-producing primitives.

use crate::engine::{Engine, ScannerStatus};
use crate::eqtb::{Equiv, Macro};

use crate::input::{EOF_MARKER, PAR_END};
use crate::prim::Prim;
use crate::token::*;

pub const PAR_REF_FLAG: u32 = 0x4000_0000;
pub const NOEXP_FLAG: u32 = 0xC000_0000;
const UNEXPANDED_PARAMETER_FLAG: u32 = 0x1000_0000;
const UNEXPANDED_CS_FLAG: u32 = 0xE000_0000;

fn balanced_end_scalar(
    tokens: &[Token],
    long: bool,
    partoken: CsId,
    mut depth: i32,
) -> Option<usize> {
    for (index, &t) in tokens.iter().enumerate() {
        if (NOEXP_FLAG..0xFFFF_0000).contains(&t.0)
            || (UNEXPANDED_PARAMETER_FLAG..0x2000_0000).contains(&t.0)
        {
            return None;
        }
        match t.0 >> 24 {
            1 => depth += 1,
            2 => {
                depth -= 1;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ if !long && (t == PAR_END || (t.is_cs() && t.cs_id() == partoken)) => return None,
            _ => {}
        }
    }
    None
}

fn balanced_end(tokens: &[Token], long: bool, partoken: CsId) -> Option<usize> {
    #[cfg(target_arch = "x86_64")]
    if tokens.len() >= 16 && std::is_x86_feature_detected!("avx2") {
        // SAFETY: the processor supports AVX2; the callee bounds every load.
        return unsafe { balanced_end_avx2(tokens, long, partoken) };
    }
    balanced_end_scalar(tokens, long, partoken, 1)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn balanced_end_avx2(tokens: &[Token], long: bool, partoken: CsId) -> Option<usize> {
    use std::arch::x86_64::*;
    let mut position = 0;
    let mut depth = 1;
    while tokens.len() - position >= 8 {
        // Token is repr(transparent) over u32 and eight initialized elements
        // remain. Unaligned loads support every token-slice starting offset.
        let values = _mm256_loadu_si256(tokens.as_ptr().add(position).cast());
        let top = _mm256_srli_epi32::<24>(values);
        let opens = _mm256_cmpeq_epi32(top, _mm256_set1_epi32(1));
        let closes = _mm256_cmpeq_epi32(top, _mm256_set1_epi32(2));
        let mut prefix = _mm256_sub_epi32(closes, opens);
        // Inclusive prefix sums within each 128-bit half, then carry the
        // low half's total into the high half.
        prefix = _mm256_add_epi32(prefix, _mm256_slli_si256::<4>(prefix));
        prefix = _mm256_add_epi32(prefix, _mm256_slli_si256::<8>(prefix));
        let carry = _mm256_extract_epi32::<3>(prefix);
        prefix = _mm256_add_epi32(
            prefix,
            _mm256_setr_epi32(0, 0, 0, 0, carry, carry, carry, carry),
        );
        let depths = _mm256_add_epi32(prefix, _mm256_set1_epi32(depth));
        let ends = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(
            depths,
            _mm256_setzero_si256(),
        ))) as u32;
        let frozen = _mm256_and_si256(
            _mm256_cmpgt_epi32(values, _mm256_set1_epi32(0xbfff_ffffu32 as i32)),
            _mm256_cmpgt_epi32(_mm256_set1_epi32(0xffff_0000u32 as i32), values),
        );
        let parameter = _mm256_cmpeq_epi32(_mm256_srli_epi32::<28>(values), _mm256_set1_epi32(1));
        let mut guards = _mm256_or_si256(frozen, parameter);
        if !long {
            let paragraph = _mm256_or_si256(
                _mm256_cmpeq_epi32(values, _mm256_set1_epi32(Token::from_cs(partoken).0 as i32)),
                _mm256_cmpeq_epi32(values, _mm256_set1_epi32(PAR_END.0 as i32)),
            );
            guards = _mm256_or_si256(guards, paragraph);
        }
        let guards = _mm256_movemask_ps(_mm256_castsi256_ps(guards)) as u32;
        if ends | guards != 0 {
            let first = (ends | guards).trailing_zeros();
            return (guards & (1 << first) == 0).then_some(position + first as usize + 1);
        }
        depth += _mm256_extract_epi32::<7>(prefix);
        position += 8;
    }
    balanced_end_scalar(&tokens[position..], long, partoken, depth).map(|end| position + end)
}

#[cfg(test)]
mod balanced_scan_tests {
    use super::*;

    #[test]
    fn vector_scan_matches_scalar_at_guards_braces_and_slice_boundaries() {
        let alphabet = [
            Token::letter(b'x'),
            Token::char(10, 32),
            Token::from_cs(42),
            Token::char(1, 123),
            Token::char(2, 125),
            Token::from_cs(7),
            Token(NOEXP_FLAG | 42),
            Token(UNEXPANDED_CS_FLAG | 42),
            Token(UNEXPANDED_PARAMETER_FLAG | Token::char(6, 35).0),
            PAR_END,
            EOF_MARKER,
        ];
        let mut seed = 1u64;
        for length in 0..192 {
            for offset in 0..8 {
                let mut tokens = vec![Token::letter(b'x'); length + offset];
                for t in &mut tokens[offset..] {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    if seed >> 60 < 5 {
                        *t = alphabet[(seed >> 32) as usize % alphabet.len()];
                    }
                }
                for long in [false, true] {
                    let slice = &tokens[offset..];
                    let expected = balanced_end_scalar(slice, long, 7, 1);
                    assert_eq!(
                        balanced_end(slice, long, 7),
                        expected,
                        "length={length} offset={offset} long={long}"
                    );
                    #[cfg(target_arch = "x86_64")]
                    if std::is_x86_feature_detected!("avx2") {
                        assert_eq!(unsafe { balanced_end_avx2(slice, long, 7) }, expected);
                    }
                }
            }
        }
        // Prefix sums must carry nesting across the 128-bit lane boundary.
        let mut deep = vec![Token::char(1, 123); 64];
        deep.extend(vec![Token::char(2, 125); 65]);
        deep.push(Token(NOEXP_FLAG | 42));
        assert_eq!(balanced_end(&deep, true, 7), Some(129));
        deep[127] = Token(NOEXP_FLAG | 42);
        assert_eq!(balanced_end(&deep, true, 7), None);
    }
}

impl Engine {
    pub(crate) fn freeze_unexpanded_toks(&mut self, mut toks: Vec<Token>) -> Vec<Token> {
        for t in &mut toks {
            let id = if t.is_cs() && t.0 < NOEXP_FLAG {
                Some(t.cs_id())
            } else if t.is_char() && t.cc() == 13 {
                Some(self.active_cs_id(t.chr()))
            } else {
                None
            };
            if let Some(id) = id {
                *t = Token(UNEXPANDED_CS_FLAG | id);
            } else if t.is_char() && t.cc() == 6 {
                t.0 |= UNEXPANDED_PARAMETER_FLAG;
            }
        }
        toks
    }
    fn delim_eq(&self, a: Token, b: Token) -> bool {
        if a.is_cs() && b.is_cs() {
            (a.0 & 0x3FFF_FFFF) == (b.0 & 0x3FFF_FFFF)
        } else if a.is_char() && b.is_char() {
            a.cc() == b.cc() && a.chr() == b.chr()
        } else if a.is_cs() && b.is_char() && b.cc() == 13 {
            self.active_cs_lookup(b.chr()) == Some(a.cs_id())
        } else if b.is_cs() && a.is_char() && a.cc() == 13 {
            self.active_cs_lookup(a.chr()) == Some(b.cs_id())
        } else {
            false
        }
    }

    /// fetch next raw token honoring pushback
    #[inline(always)]
    pub fn raw_token(&mut self) -> Token {
        // tex.web @7335/@7492: a brace fetched from a real input source
        // adjusts the alignment brace depth. Tokens returned from the
        // pushback stack were counted at their original fetch (tex.web
        // compensates at back_input, @7028; skipping the pushed path is the
        // equivalent here), and <align-peek> replays re-serve an already
        let t = 'fetch: {
            if self.scanner_status != ScannerStatus::Aligning {
                if let Some(t) = self.pushed.pop() {
                    let keep_synthetic = t.is_cs()
                        && self
                            .diagnostic_synthetic_source
                            .as_ref()
                            .is_some_and(|(token, _, _)| *token == t.cs_id());
                    if !keep_synthetic {
                        self.diagnostic_synthetic_source = None;
                    }
                    let keep_physical =
                        self.diagnostic_physical_source
                            .as_ref()
                            .is_some_and(|source| {
                                source.token == t
                                    || (t.is_cs() && source.semantic_cs == Some(t.cs_id()))
                            });
                    if !keep_physical {
                        self.diagnostic_physical_source = None;
                    }
                    break 'fetch t;
                }
            } else if self.pushed.len() > self.align_pushed_base {
                let t = self.pushed.pop().unwrap();
                if !t.is_cs()
                    || self
                        .diagnostic_synthetic_source
                        .as_ref()
                        .is_some_and(|(token, _, _)| *token != t.cs_id())
                {
                    self.diagnostic_synthetic_source = None;
                }
                if !self
                    .diagnostic_physical_source
                    .as_ref()
                    .is_some_and(|source| {
                        source.token == t || (t.is_cs() && source.semantic_cs == Some(t.cs_id()))
                    })
                {
                    self.diagnostic_physical_source = None;
                }
                break 'fetch t;
            }
            loop {
                let trace_depth = match self.input.stack.last() {
                    Some(crate::input::Source::TokList { trace_depth, .. }) => {
                        *trace_depth as usize
                    }
                    Some(crate::input::Source::MacroFrame(frame)) => frame.trace_depth as usize,
                    _ => break,
                };
                if !self.align_macro_arg && self.diagnostic_trace_hold == 0 {
                    self.diagnostic_macro_trace.truncate(trace_depth);
                }
                match self.input.stack.last_mut() {
                    Some(crate::input::Source::TokList { toks, pos, .. }) => {
                        let s = &toks[..];
                        if *pos < s.len() {
                            let tok = s[*pos];
                            *pos += 1;
                            self.diagnostic_token_from_file = false;
                            self.diagnostic_synthetic_source = None;
                            self.diagnostic_physical_source = None;
                            break 'fetch tok;
                        }
                    }
                    Some(crate::input::Source::MacroFrame(frame)) => {
                        if let Some(tok) = frame.next_token() {
                            self.diagnostic_token_from_file = false;
                            self.diagnostic_synthetic_source = None;
                            self.diagnostic_physical_source = None;
                            break 'fetch tok;
                        }
                    }
                    _ => unreachable!(),
                }
                if let Some(src) = self.input.stack.pop() {
                    if let crate::input::Source::TokList { toks, name, .. } = src {
                        if name == crate::align::U_PART_SRC {
                            self.align_u_template_finished();
                        }
                        if let crate::input::TokTokens::Vec(mut v) = toks {
                            v.clear();
                            if self.token_vec_pool.len() < 512 {
                                self.token_vec_pool.push(v);
                            }
                        }
                    }
                }
            }
            self.get_next_raw()
        };
        // tex.web's frozen \endtemplate is an outer control sequence. A
        // delimited macro argument cannot consume it after a top-level `&`
        // has ended the cell; report the runaway argument instead of letting
        // the scan continue into the next cell. This engine represents the
        // frozen marker with the close stream's \crcr sentinel.
        if self.align_macro_arg
            && self.align_state & crate::align::PH_CLOSE != 0
            && t.is_cs()
            && matches!(
                self.eqtb.resolve(t.cs_id()),
                Some(crate::eqtb::Equiv::Prim(
                    crate::prim::Prim::Cr | crate::prim::Prim::CrCr
                ))
            )
        {
            self.error("Forbidden control sequence found while scanning a macro argument");
        }
        if t.0 < 0x8000_0000 && !t.is_cs() {
            let cc = (t.0 >> 24) as u8;
            if cc == 1 {
                self.align_brace_depth = self.align_brace_depth.saturating_add(1);
            } else if cc == 2 {
                self.align_brace_depth = self.align_brace_depth.saturating_sub(1);
            }
            if cc == 14 || cc == 9 || cc == 15 {
                if cc == 14 {
                    if let Some(si) = self.input.stack.len().checked_sub(1) {
                        if let Some(crate::input::Source::File {
                            line_buf,
                            line_pos,
                            state,
                            ..
                        }) = self.input.stack.get_mut(si)
                        {
                            *line_buf = None;
                            *line_pos = 0;
                            if *state == 1 {
                                *state = 2;
                            }
                        }
                    }
                }
                return self.raw_token();
            }
        }
        if self.align_state != crate::align::PH_IDLE
            && !self.in_expanded_scan
            && self.align_intercept_raw_token(t)
        {
            return self.raw_token();
        }
        let t = if t == PAR_END {
            Token::from_cs(self.partoken_id())
        } else {
            t
        };

        if t.0 >= PAR_REF_FLAG && t.0 < 0x8000_0000 && !t.is_cs() {
            let n = (t.0 & 0xF) as u8;
            self.push_token(Token::char(12, b'0' as u32 + n as u32));
            return Token::char(6, b'#' as u32);
        }

        t
    }
    #[inline]
    fn macro_arg_token(&mut self) -> Token {
        let saved = self.align_macro_arg;
        self.align_macro_arg = true;
        let t = self.raw_token();
        self.align_macro_arg = saved;
        t
    }

    /// tex.web get_x_token: expands macros/conditionals but does NOT skip
    /// spaces (scan_int relies on spaces terminating constants).
    pub fn get_x_raw(&mut self) -> Token {
        self.unexpanded_parameter = false;
        let t = self.raw_token();
        self.get_x_raw_from(t)
    }

    /// Expansion step for an already-fetched raw token (the alignment
    /// interrow peek pre-filters PAR_END/\\par before expansion).
    pub fn get_x_raw_from(&mut self, first: Token) -> Token {
        let mut first = first;
        loop {
            if first.0 >= UNEXPANDED_CS_FLAG && first.0 < 0xFFFF_0000 {
                let tok = self.unfreeze_unexpanded_token(first);
                if tok.is_cs() {
                    self.set_cur_cs(tok);
                } else {
                    self.set_cur_char(tok);
                }
                return tok;
            }
            if first.0 >= UNEXPANDED_PARAMETER_FLAG && first.0 < 0x2000_0000 {
                self.unexpanded_parameter = true;
                return first.unfreeze();
            }
            if first.0 >= crate::page::WRITE_END_TOKEN.0 && first != PAR_END {
                self.cur_prim = None;
                return first;
            }
            // The PAR_END sentinel (0xFFFF_FFFE) sits in the cs-token encoding
            // space; without conversion get_x_raw treats it as an unnameable cs
            // (blank-line \par between alignment rows -> garbage "cs" starts a
            // phantom row). Convert exactly like get_token_inner does.
            let t = if first == PAR_END {
                Token::from_cs(self.partoken_id())
            } else {
                first
            };
            let mut id = if t.is_cs() {
                self.diagnostic_source_cs = Some(t.cs_id());
                t.cs_id()
            } else if t.is_char() && t.cc() == 13 {
                let id = self.active_cs_id(t.chr());
                self.diagnostic_source_cs = Some(id);
                id
            } else {
                self.diagnostic_source_cs = None;
                self.set_cur_char(t);
                return t;
            };
            let t = Token::from_cs(id);

            let mut equiv = self.eqtb.get(id);
            if let Some(Equiv::Alias(mut next)) = equiv {
                while let Some(Equiv::Alias(n)) = self.eqtb.get(next) {
                    next = *n;
                }
                id = next;
                equiv = self.eqtb.get(id);
            }
            match equiv.cloned() {
                Some(Equiv::Macro(m)) => {
                    // edef/write/expanded list. Nested \\romannumeral (f-expansion)
                    // clears in_expanded_scan and must expand \\exp_end_continue_f:w.
                    if m.protected && self.in_expanded_scan && self.csname_depth == 0 {
                        self.set_cur_cs(Token::from_cs(id));
                        return Token::from_cs(id);
                    }
                    if self.freeze_gts_in_edef(id) {
                        self.set_cur_cs(Token::from_cs(id));
                        return Token::from_cs(id);
                    }
                    if self.is_self_quark(id, &m) {
                        self.set_cur_cs(Token::from_cs(id));
                        return Token::from_cs(id);
                    }
                    self.expand_macro(id, &m, t.cs_id());
                    first = self.raw_token();
                    continue;
                }

                Some(Equiv::CharTok(v)) => {
                    let tok = Token(v);
                    if tok.is_space() {
                        self.cur_tok = tok;
                        self.cur_cs = None;
                        self.cur_prim = None;
                        return tok;
                    } else {
                        self.set_cur_cs(t);
                        return t;
                    }
                }
                Some(Equiv::Prim(p)) => {
                    // While scanning a conditional's numeric operand, expansion
                    // may run nested conditionals. The delimiter belonging to
                    // the pending outer test must terminate the number instead
                    // of executing against its not-yet-initialized frame.
                    if matches!(
                        p,
                        Prim::Else | Prim::Or | Prim::Fi | Prim::ElIf | Prim::ElIfX
                    ) && self
                        .pending_if_depth
                        .is_some_and(|depth| self.if_stack.len() <= depth)
                    {
                        self.set_cur_cs(t);
                        return t;
                    }
                    if self.is_expandable(p) {
                        match self.expand_prim(p, id) {
                            Some(tok) => {
                                if tok.0 >= UNEXPANDED_CS_FLAG && tok.0 < 0xFFFF_0000 {
                                    let tok = self.unfreeze_unexpanded_token(tok);
                                    if tok.is_cs() {
                                        self.set_cur_cs(tok);
                                    } else {
                                        self.set_cur_char(tok);
                                    }
                                    return tok;
                                }
                                if tok.0 >= NOEXP_FLAG && tok.0 < UNEXPANDED_CS_FLAG {
                                    let tok = Token::from_cs(tok.0 & 0x3FFF_FFFF);
                                    self.set_cur_cs(tok);
                                    return tok;
                                }
                                if !tok.is_cs() {
                                    return tok;
                                }
                                self.push_token(tok);
                                first = self.raw_token();
                                continue;
                            }
                            None => {
                                first = self.raw_token();
                                continue;
                            }
                        }
                    } else {
                        self.set_cur_cs(t);
                        return t;
                    }
                }
                _ => {
                    self.set_cur_cs(t);
                    return t;
                }
            }
        }
    }

    /// tex.web begin_token_list semantics: the list becomes its own
    /// Source::TokList on TOP of the input stack, so it nests properly
    /// with macro bodies, the \everypar hook and the output routine
    /// (fire_up's <after-output> parking below the OR can no longer
    /// capture a replay that belongs inside a hook or macro).
    ///
    /// `pushed` now holds only genuine single-token back_input (scan
    /// lookahead). Those tokens are OLDER than the list being begun, and
    /// tex.web reads the newest list first, so a pending pushback is
    /// flushed into its own source UNDER the new list. During alignment
    /// scanning the tail below `align_pushed_base` predates the u/v-part
    /// source and keeps waiting under raw_token's Aligning gate.
    pub fn push_tokens(&mut self, toks: Vec<Token>) {
        self.begin_token_list(toks, false, "<replay>", None);
    }
    /// push_tokens variant matching tex.web \\unexpanded: each control
    /// sequence carries the one-shot \\noexpand flag so a later x/f-scan
    /// stores it without expanding; char tokens are unaffected.
    pub fn push_tokens_exp_not(&mut self, toks: Vec<Token>) {
        self.begin_token_list(toks, true, "<replay>", None);
    }
    /// named replay source: trace readability only.
    pub fn push_tokens_named(&mut self, toks: Vec<Token>, name: &'static str) {
        self.try_push_tokens_named(toks, name);
    }
    pub(crate) fn try_push_tokens_named(&mut self, toks: Vec<Token>, name: &'static str) -> bool {
        self.begin_token_list(toks, false, name, None)
    }
    fn push_macro_tokens(&mut self, toks: Vec<Token>, owner: CsId) {
        self.begin_token_list(toks, false, "<macro>", Some(owner));
    }
    #[inline]
    pub(crate) fn ensure_input_stack_room(&mut self, needed: usize) -> bool {
        if self.input.stack.len().saturating_add(needed) <= crate::input::MAX_INPUT_STACK {
            return true;
        }
        self.fatal_error(&format!(
            "TeX capacity exceeded, sorry [input stack size={}]",
            crate::input::MAX_INPUT_STACK
        ));
        false
    }

    fn ensure_token_list_room(&mut self, len: usize) -> bool {
        if len <= crate::input::MAX_TOKEN_LIST_TOKENS {
            return true;
        }
        self.fatal_error(&format!(
            "TeX capacity exceeded, sorry [token list size={}]",
            crate::input::MAX_TOKEN_LIST_TOKENS
        ));
        false
    }

    fn pop_exhausted_token_lists(&mut self) {
        while let Some(src) = self.input.stack.last() {
            match src {
                crate::input::Source::TokList {
                    toks, pos, name, ..
                } => {
                    if *pos >= toks.len() {
                        if *name == crate::align::U_PART_SRC {
                            self.align_u_template_finished();
                        }
                        self.input.stack.pop();
                    } else {
                        break;
                    }
                }
                crate::input::Source::MacroFrame(frame) => {
                    if frame.is_exhausted() {
                        self.input.stack.pop();
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
    }

    pub fn push_tokens_rc(&mut self, toks: std::rc::Rc<[Token]>, owner: CsId) {
        self.try_push_tokens_rc(toks, owner);
    }

    fn try_push_tokens_rc(&mut self, toks: std::rc::Rc<[Token]>, owner: CsId) -> bool {
        if toks.is_empty() {
            return true;
        }
        if !self.ensure_token_list_room(toks.len()) {
            return false;
        }
        self.pop_exhausted_token_lists();
        let cut = if self.scanner_status == ScannerStatus::Aligning {
            self.align_pushed_base.min(self.pushed.len())
        } else {
            0
        };
        if !self.ensure_token_list_room(self.pushed.len().saturating_sub(cut)) {
            return false;
        }
        if !self.ensure_input_stack_room(1 + usize::from(self.pushed.len() > cut)) {
            return false;
        }
        if self.pushed.len() > cut {
            let mut rest = self.pushed.split_off(cut);
            rest.reverse();
            self.input.push_toks(rest, "<pushback>");
        }
        self.input.push_toks_owned(
            toks,
            "<macro>",
            Some(owner),
            self.diagnostic_macro_trace.len().min(u8::MAX as usize) as u8,
        );
        true
    }
    fn try_push_macro_frame(&mut self, frame: crate::input::MacroFrame) -> bool {
        self.pop_exhausted_token_lists();
        let cut = if self.scanner_status == ScannerStatus::Aligning {
            self.align_pushed_base.min(self.pushed.len())
        } else {
            0
        };
        if !self.ensure_token_list_room(self.pushed.len().saturating_sub(cut)) {
            return false;
        }
        if !self.ensure_input_stack_room(1 + usize::from(self.pushed.len() > cut)) {
            return false;
        }
        if self.pushed.len() > cut {
            let mut rest = self.pushed.split_off(cut);
            rest.reverse();
            self.input.push_toks(rest, "<pushback>");
        }
        self.input
            .stack
            .push(crate::input::Source::MacroFrame(frame));
        true
    }

    fn begin_token_list(
        &mut self,
        toks: Vec<Token>,
        exp_not: bool,
        name: &'static str,
        owner: Option<CsId>,
    ) -> bool {
        if toks.is_empty() {
            return true;
        }
        if !self.ensure_token_list_room(toks.len()) {
            return false;
        }
        self.pop_exhausted_token_lists();
        let cut = if self.scanner_status == ScannerStatus::Aligning {
            self.align_pushed_base.min(self.pushed.len())
        } else {
            0
        };
        if !self.ensure_token_list_room(self.pushed.len().saturating_sub(cut)) {
            return false;
        }
        if !self.ensure_input_stack_room(1 + usize::from(self.pushed.len() > cut)) {
            return false;
        }
        if self.pushed.len() > cut {
            let mut rest = self.pushed.split_off(cut);
            rest.reverse();
            self.input.push_toks(rest, "<pushback>");
        }
        let toks = if exp_not {
            self.freeze_unexpanded_toks(toks)
        } else {
            toks
        };
        self.input.push_toks_owned(
            toks,
            name,
            owner,
            self.diagnostic_macro_trace.len().min(u8::MAX as usize) as u8,
        );
        true
    }
    fn set_cur_cs(&mut self, t: Token) {
        self.diagnostic_source_cs = Some(t.cs_id());
        self.cur_tok = t;
        self.cur_cs = Some(t.cs_id());
        let prim = match self.eqtb.resolve(t.cs_id()) {
            Some(Equiv::Prim(p)) => Some(*p),
            _ => None,
        };
        self.cur_prim = prim;
    }

    fn set_cur_char(&mut self, t: Token) {
        self.diagnostic_source_cs = None;
        self.cur_tok = t;
        self.cur_cs = None;
        self.cur_prim = None;
    }

    /// Get the next token, expanding macros and expandable primitives.
    /// Sets cur_tok/cur_cs/cur_prim.
    pub fn get_token(&mut self) -> Token {
        self.no_expand_tok = None;
        self.unexpanded_parameter = false;
        let first = self.raw_token();
        self.get_token_inner(first)
    }

    /// Expand a token already fetched by a scanner without parking and
    /// fetching it again through the input stack.
    pub(crate) fn get_token_from(&mut self, first: Token) -> Token {
        self.no_expand_tok = None;
        self.unexpanded_parameter = false;
        self.get_token_inner(first)
    }

    fn get_token_inner(&mut self, mut t: Token) -> Token {
        'resolve: loop {
            'expand: {
                if t.0 >= UNEXPANDED_CS_FLAG && t.0 < 0xFFFF_0000 {
                    t = self.unfreeze_unexpanded_token(t);
                    if t.is_cs() {
                        self.no_expand_tok = Some(t);
                        self.cur_cs = Some(t.cs_id());
                        self.cur_prim = Some(Prim::Relax);
                    } else {
                        self.set_cur_char(t);
                    }
                    return t;
                }
                if t.0 >= UNEXPANDED_PARAMETER_FLAG && t.0 < 0x2000_0000 {
                    t = t.unfreeze();
                    self.unexpanded_parameter = true;
                    self.set_cur_char(t);
                    return t;
                }
                if t.0 < 0x8000_0000 && t.cc() != 13 {
                    self.set_cur_char(t);
                    return t;
                }
                if t.0 >= NOEXP_FLAG && t.0 < 0xFFFF_0000 {
                    let cs = t.0 & 0x3FFF_FFFF;
                    let tok = Token::from_cs(cs);
                    if self.eqtb.get(cs).is_none() {
                        // Lazily synthesize l3 exp_args:N<spec> expanders.
                        self.synth_exp_args_if_match(cs);
                    }
                    self.no_expand_tok = Some(tok);
                    self.cur_tok = tok;
                    self.cur_cs = Some(cs);
                    self.cur_prim = Some(Prim::Relax);
                    return tok;
                }

                if t.0 >= crate::page::WRITE_END_TOKEN.0 {
                    if t == EOF_MARKER {
                        self.end_occurred = true;
                        return EOF_MARKER;
                    }
                    if t == crate::page::OUT_END_TOKEN {
                        return t;
                    }
                    if t == crate::page::WRITE_END_TOKEN {
                        return t;
                    }
                    if t == PAR_END {
                        t = Token::from_cs(self.partoken_id());
                    }
                }

                t = if t.is_char() && t.cc() == 13 {
                    let id = self.active_cs_id(t.chr());
                    self.diagnostic_source_cs = Some(id);
                    Token::from_cs(id)
                } else {
                    self.diagnostic_source_cs = t.is_cs().then(|| t.cs_id());
                    t
                };

                if t.is_cs() {
                    let mut id = t.cs_id();
                    if let Some(Equiv::Alias(mut next)) = self.eqtb.get(id) {
                        while let Some(Equiv::Alias(n)) = self.eqtb.get(next) {
                            next = *n;
                        }
                        id = next;
                    }
                    let is_expansion = match self.eqtb.get(id) {
                        Some(Equiv::Macro(_)) => true,
                        Some(Equiv::Prim(p)) => self.is_expandable(*p),
                        _ => false,
                    };
                    if is_expansion {
                        self.expansion_steps = self.expansion_steps.saturating_add(1);
                        if self.expansion_limit > 0 && self.expansion_steps > self.expansion_limit {
                            self.set_cur_cs(t);
                            self.fatal_error(&format!(
                            "TeX capacity exceeded [expansion steps={}]; runaway expansion near {}",
                            self.expansion_limit,
                            self.display_cs(id)
                        ));
                            return EOF_MARKER;
                        }
                    }
                    let equiv = self.eqtb.get(id);
                    match equiv {
                        Some(Equiv::Macro(m)) => {
                            if m.protected && self.in_expanded_scan && self.csname_depth == 0 {
                                self.set_cur_cs(t);
                                return t;
                            }
                            if self.freeze_gts_in_edef(id) {
                                self.set_cur_cs(t);
                                return t;
                            }
                            if m.num_params == 0 && m.prefix.is_empty() {
                                let body = std::rc::Rc::clone(&m.body);
                                self.enter_macro_diagnostic(id, t.cs_id());
                                if body.is_empty() {
                                    break 'expand;
                                }
                                if body.len() == 1 {
                                    let t_only = body[0];
                                    if t_only == Token::from_cs(id) {
                                        self.set_cur_cs(t);
                                        return t;
                                    }
                                    if (0x8000_0000..NOEXP_FLAG).contains(&t_only.0)
                                        && !self.align_macro_arg
                                    {
                                        t = t_only;
                                        continue 'resolve;
                                    }
                                    if t_only.0 < 0x8000_0000
                                        && t_only.cc() != 13
                                        && !matches!(t_only.cc(), 1 | 2 | 4)
                                    {
                                        self.cur_tok = t_only;
                                        self.cur_cs = None;
                                        self.cur_prim = None;
                                        return t_only;
                                    }
                                }
                                if !self.try_push_tokens_rc(body, id) {
                                    return EOF_MARKER;
                                }
                                break 'expand;
                            }
                            let m = m.clone();
                            self.expand_macro(id, &m, t.cs_id());
                            break 'expand;
                        }
                        Some(Equiv::CharTok(v)) => {
                            // tex.web scan_toks: a CS \let-equal to `}` is not
                            // expandable and is stored as the CS. Substituting
                            // the char would close \expanded/\edef early
                            // (\c_group_end_token inside \cs_new_protected:Npe).
                            if self.in_expanded_scan {
                                self.set_cur_cs(t);
                                return t;
                            }
                            let tok = Token(*v);
                            if self.align_state != crate::align::PH_IDLE
                                && tok.is_char()
                                && tok.cc() == 4
                                && self.align_intercept_raw_token(tok)
                            {
                                t = self.raw_token();
                                continue 'resolve;
                            }
                            self.cur_tok = tok;
                            self.cur_cs = None;
                            self.cur_prim = None;
                            return tok;
                        }
                        Some(Equiv::Prim(p_ref)) => {
                            let p = *p_ref;

                            // NOTE: no early-return for UnExpanded inside
                            // e-scans. Returning the bare token leaked an
                            // unexpanded-marker into \expanded results, which
                            // then re-froze the NEXT token singly and broke the
                            // \unexpanded\expanded{{...}} callback idiom
                            // (keyval_parse / l3keys). The normal arm below
                            // (scan group + push exp_not) is correct in every
                            // context, csname included.
                            if matches!(
                                p,
                                Prim::Else | Prim::Or | Prim::Fi | Prim::ElIf | Prim::ElIfX
                            ) && self
                                .pending_if_depth
                                .is_some_and(|depth| self.if_stack.len() <= depth)
                            {
                                let tok = Token::from_cs(id);
                                self.set_cur_cs(tok);
                                return tok;
                            }
                            if self.is_expandable(p) {
                                // A \csname-created exp_args:N<spec> may carry the
                                // relax default from an earlier pass; repair it.
                                if p == Prim::Relax
                                    && self.cs.name(id) != b"relax"
                                    && self.name_is_synth_exp_args(id)
                                {
                                    self.synth_exp_args_if_match(id);
                                    break 'expand;
                                }

                                match self.expand_prim(p, id) {
                                    Some(tok) => {
                                        if tok.0 >= UNEXPANDED_CS_FLAG && tok.0 < 0xFFFF_0000 {
                                            let tok = self.unfreeze_unexpanded_token(tok);
                                            self.no_expand_tok = tok.is_cs().then_some(tok);
                                            if tok.is_cs() {
                                                self.set_cur_cs(tok);
                                                self.cur_prim = Some(Prim::Relax);
                                            } else {
                                                self.set_cur_char(tok);
                                            }
                                            return tok;
                                        }
                                        if tok.0 >= NOEXP_FLAG && tok.0 < UNEXPANDED_CS_FLAG {
                                            let cs = tok.0 & 0x3FFF_FFFF;
                                            let tok = Token::from_cs(cs);
                                            self.no_expand_tok = Some(tok);
                                            self.cur_tok = tok;
                                            self.cur_cs = Some(cs);
                                            self.cur_prim = Some(Prim::Relax);
                                            return tok;
                                        }
                                        if !tok.is_cs() {
                                            self.set_cur_char(tok);
                                            return tok;
                                        }
                                        self.push_token(tok);
                                        break 'expand;
                                    }
                                    None => break 'expand,
                                }
                            } else {
                                self.set_cur_cs(t);
                                return t;
                            }
                        }
                        _ => {
                            if self.synth_exp_args_if_match(id) {
                                break 'expand;
                            }
                            self.set_cur_cs(t);
                            return t;
                        }
                    }
                } else {
                    self.set_cur_char(t);
                    return t;
                }
            }
            t = self.raw_token();
        }
    }

    /// true when the cs name is exp_args:N followed only by l3 arg letters.
    pub fn name_is_synth_exp_args(&self, id: CsId) -> bool {
        let name = self.cs.name(id);
        if name.len() < 11 || name[0] != b'e' {
            return false;
        }
        name.starts_with(b"exp_args:N")
            && name[10..].iter().all(|c| {
                matches!(
                    c,
                    b'N' | b'n' | b'c' | b'o' | b'f' | b'e' | b'V' | b'v' | b'x'
                )
            })
    }

    /// l3 variant wrappers reference \exp_args:N<spec> expanders lazily
    /// (\exp_not:c{exp_args:NNcc}); a boot that has not generated the spec
    /// yet would see the expander as undefined (then relax-poisoned via
    /// \csname) and pass c-args through as raw character groups, killing
    /// expl3 quark/variant generation. Synthesize the standard expander on
    /// first use as the same \::-chain body real l3 builds:
    /// \exp_args:NNxn -> \::N \::x \::n \:::  (params grabbed from stream).
    /// Returns true when the name matched and the macro was assigned.
    pub fn synth_exp_args_if_match(&mut self, id: CsId) -> bool {
        if !self.name_is_synth_exp_args(id) {
            return false;
        }
        let name = self.cs.name(id);
        let spec = &name[10..];
        let mut body: Vec<Token> = Vec::new();
        for &letter in spec {
            let helper = [b':', b':', letter];
            match self.cs.lookup(&helper) {
                Some(h) => body.push(Token::from_cs(h)),
                None => return false,
            }
        }
        match self.cs.lookup(b":::") {
            Some(t) => body.push(Token::from_cs(t)),
            None => return false,
        }
        let m = crate::eqtb::Macro {
            replacement: Default::default(),
            num_params: 0,
            has_param_refs: false,
            params: Vec::new(),
            prefix: Vec::new(),
            body: body.into(),
            long: true,
            outer: false,
            protected: false,
        };
        self.eqtb
            .assign(id, Equiv::Macro(std::rc::Rc::new(m)), true);
        true
    }

    #[allow(dead_code)]
    fn synth_legacy_removed(&mut self, _id: CsId) -> bool {
        false
    }

    #[allow(dead_code)]
    fn old_body(&mut self) {
        let spec: Vec<u8> = Vec::new();
        let k = spec.len();
        let _ = k;
        let (ex, noex, csw, cse) = (
            self.cs.lookup(b"expanded"),
            self.cs.lookup(b"noexpand"),
            self.cs.lookup(b"csname"),
            self.cs.lookup(b"endcsname"),
        );
        let _ = (ex, noex, csw, cse);
        let mut body: Vec<Token> = Vec::new();
        body.push(Token::char(1, b'{' as u32));
        for (i, letter) in spec.iter().enumerate() {
            let pref = crate::expand::PAR_REF_FLAG | (i as u32 + 1);
            match letter {
                b'N' => {}
                b'n' => {}
                _ => {}
            }
            body.push(Token(pref));
        }
    }

    #[inline(always)]
    pub fn is_expandable(&self, p: Prim) -> bool {
        use Prim::*;
        matches!(
            p,
            ExpandAfter
                | NoExpand
                | CsName
                | LastNamedCs
                | The
                | String
                | Meaning
                | Number
                | RomanNumeral
                | Detokenize
                | ScanTokens
                | DirectLua
                | Input
                | Expanded
                | UnExpanded
                | JobName
                | FontName
                | FontIdPrim
                | IfChar
                | IfCat
                | IfOdd
                | IfNum
                | IfDim
                | IfVoid
                | IfHBox
                | IfVBox
                | IfHMode
                | IfVMode
                | IfInner
                | IfMMode
                | IfTrue
                | IfFalse
                | IfEOF
                | IfDef
                | IfCSName
                | IfInCsName
                | IfX
                | IfFontChar
                | IfCase
                | Or
                | Else
                | ElIf
                | ElIfX
                | Fi
                | Unless
                | PdfFileSize
                | PdfMdFiveSum
                | PdfFileModDate
                | PdfCreationDate
                | PdfFileDump
                | PdfStrCmp
                | PdfElapsedTime
                | PdfUniformDeviate
                | PdfNormalDeviate
                | PdfEscapeString
                | PdfEscapeName
                | PdfEscapeHex
                | PdfUnescapeHex
                | PdfTexRevision
                | PdfColorStackInit
                | PdfBanner
                | PdfFontSize
                | LeftMarginKern
                | RightMarginKern
                | UcharCat
                | RatexUnicodeVersion
                | RatexNativeTextMode
                | RatexUtfEight
                | FileSize
                | PdfMatch
                | PdfLastMatch
                | TopMark
                | FirstMark
                | BotMark
                | SplitFirstMark
                | SplitBotMark
                | TopMarksClass
                | FirstMarksClass
                | BotMarksClass
                | SplitFirstMarksClass
                | SplitBotMarksClass
                | Prim::XeTeXRevision
                | Prim::XeTeXGlyphName
                | Prim::XeTeXFeatureName
                | Prim::XeTeXVariationName
                | Prim::LuaTeXRevision
                | Prim::LuaTeXBanner
        )
    }

    /// Execute an expandable primitive; None = keep expanding,
    /// Some(t) = t is the resulting current token.
    pub fn expand_prim(&mut self, p: Prim, id: CsId) -> Option<Token> {
        if p == Prim::IfCase {
            // The case frame must exist while its numeric operand expands:
            // nested conditionals can remain open until after the first digit.
            let save = self.push_if(id);
            let previous = self.pending_if_depth.replace(self.if_stack.len());
            let n = self.scan_int();
            self.pending_if_depth = previous;
            if let Some(st) = self.if_stack.get_mut(save) {
                st.evaluating = false;
                st.accepting = n == 0;
                st.matched = n == 0;
                st.if_case = n;
            }
            if n != 0 {
                self.skip_branch(true, save);
            }
            return None;
        }
        if matches!(
            p,
            Prim::IfOdd
                | Prim::IfNum
                | Prim::IfDim
                | Prim::IfVoid
                | Prim::IfFontChar
                | Prim::IfHBox
                | Prim::IfVBox
                | Prim::IfEOF
        ) {
            // The outer conditional must exist before operand expansion:
            // an operand can leave a nested conditional open.
            let unless = std::mem::take(&mut self.unless_next);
            let save = self.push_if(id);
            let previous = self.pending_if_depth.replace(self.if_stack.len());
            let value = match p {
                Prim::IfOdd => self.scan_int() % 2 != 0,
                Prim::IfNum | Prim::IfDim => {
                    let a = if p == Prim::IfNum {
                        self.scan_int()
                    } else {
                        self.scan_dimen(false, false)
                    };
                    let rel = self.scan_relational();
                    let b = if p == Prim::IfNum {
                        self.scan_int()
                    } else {
                        self.scan_dimen(false, false)
                    };
                    compare(a, rel, b)
                }
                Prim::IfVoid | Prim::IfHBox | Prim::IfVBox => {
                    let n = self.scan_reg_num() as usize;
                    match p {
                        Prim::IfVoid => self.eqtb.boxed[n].is_none(),
                        Prim::IfHBox => matches!(
                            &self.eqtb.boxed[n],
                            Some(crate::boxes::Node::Box { kind: 0, .. })
                        ),
                        _ => matches!(
                            &self.eqtb.boxed[n],
                            Some(crate::boxes::Node::Box { kind: 1 | 2, .. })
                        ),
                    }
                }
                Prim::IfFontChar => {
                    let f = self.scan_font_id();
                    let c = if self.font_loader.native_fonts.contains_key(&f) {
                        self.scan_unicode_character_code("\\iffontchar")
                    } else {
                        self.scan_character_code("\\iffontchar") as u32
                    };
                    self.native_char_present(f, c).unwrap_or_else(|| {
                        u8::try_from(c).ok().is_some_and(|byte| {
                            self.eqtb
                                .fonts
                                .get(f as usize)
                                .is_some_and(|font| font.exists_char(byte))
                        })
                    })
                }
                Prim::IfEOF => {
                    let n = self.scan_int();
                    self.read_eof
                        .get(n.max(0) as usize)
                        .copied()
                        .unwrap_or(true)
                }
                _ => unreachable!(),
            };
            self.pending_if_depth = previous;
            self.finish_if(save, value ^ unless)
        } else {
            self.expand_prim_inner(p, id)
        }
    }

    fn expand_prim_inner(&mut self, p: Prim, id: CsId) -> Option<Token> {
        use Prim::*;
        match p {
            ExpandAfter => {
                let t1 = self.raw_token();
                let t2 = self.raw_token();
                if t2.0 >= NOEXP_FLAG && t2.0 < 0xFFFF_0000 {
                    self.push_token(t2);
                } else if t2.is_cs() || (t2.is_char() && t2.cc() == 13) {
                    let mut id2 = if t2.is_cs() {
                        t2.cs_id()
                    } else {
                        self.active_cs_id(t2.chr())
                    };
                    let invocation = id2;
                    for _ in 0..1024 {
                        match self.eqtb.get(id2) {
                            Some(Equiv::Alias(next)) => id2 = *next,
                            _ => break,
                        }
                    }
                    if let Some(eq) = self.eqtb.get(id2) {
                        match eq {
                            // tex.web: \expandafter expands even \protected macros
                            Equiv::Macro(m) => {
                                if self.freeze_gts_in_edef(id2) {
                                    self.push_token(t2);
                                } else if m.num_params == 0 && m.prefix.is_empty() {
                                    let body = std::rc::Rc::clone(&m.body);
                                    self.enter_macro_diagnostic(id2, invocation);
                                    self.push_tokens_rc(body, id2);
                                } else {
                                    let m = m.clone();
                                    self.expand_macro(id2, &m, invocation);
                                }
                            }
                            Equiv::Prim(The) => {
                                let expanded = self.in_expanded_scan;
                                self.in_expanded_scan = false;
                                self.the_scan();
                                self.in_expanded_scan = expanded;
                            }
                            Equiv::Prim(p2) if self.is_expandable(*p2) => {
                                let p2 = *p2;
                                if let Some(tt) = self.expand_prim(p2, id2) {
                                    self.push_token(tt);
                                }
                            }
                            _ => {
                                self.push_token(t2);
                            }
                        }
                    } else {
                        self.push_token(t2);
                    }
                } else {
                    self.push_token(t2);
                }
                self.push_token(t1);
                None
            }

            NoExpand => {
                let t = self.raw_token();
                let id = if t.is_cs() {
                    Some(t.cs_id())
                } else if t.is_char() && t.cc() == 13 {
                    Some(self.active_cs_id(t.chr()))
                } else {
                    None
                };
                if let Some(id) = id {
                    // tex.web \noexpand: the one-shot no-expansion flag
                    // applies to any expandable control sequence, including
                    // active-character control sequences.
                    let needs_freeze = match self.eqtb.resolve(id) {
                        Some(Equiv::Macro(_)) => true,
                        Some(Equiv::Prim(p2)) => self.is_expandable(*p2),
                        _ => false,
                    };
                    if needs_freeze {
                        Some(Token(NOEXP_FLAG | id))
                    } else {
                        Some(t)
                    }
                } else {
                    Some(t)
                }
            }
            EndCsName => {
                // extra \endcsname outside \csname: TeX errors then continues
                None
            }
            CsName => {
                let csname_origin = self.current_token_source_mark();
                let csname_span = if self.diagnostic_macro_trace.is_empty() {
                    self.diagnostic_cs_source_width(self.diagnostic_source_cs.unwrap_or(id))
                } else {
                    self.diagnostic_macro_call_span
                };
                self.csname_depth += 1;
                let mut name: Vec<u8> = Vec::with_capacity(32);
                // A leftover e-TeX \unless flag must not flip \ifx inside \csname
                self.unless_next = false;
                loop {
                    if name.len() > 2000 {
                        self.csname_depth = self.csname_depth.saturating_sub(1);
                        self.fatal_error_at(
                            "TeX capacity exceeded [control sequence name exceeds 2000 bytes]",
                            csname_origin
                                .as_ref()
                                .map(crate::input::SourceMark::to_context),
                        );
                        return None;
                    }
                    let t = self.get_x_raw();
                    if t == EOF_MARKER {
                        self.csname_depth = self.csname_depth.saturating_sub(1);
                        self.fatal_error_at(
                            "File ended while scanning \\csname; missing \\endcsname",
                            csname_origin
                                .as_ref()
                                .map(crate::input::SourceMark::to_context),
                        );
                        return None;
                    }
                    if t.is_cs() {
                        let id = t.cs_id();
                        let is_end = matches!(
                            self.eqtb.resolve(id),
                            Some(Equiv::Prim(crate::prim::Prim::EndCsName))
                        );
                        if is_end {
                            break;
                        }
                        // get_x_raw strips \\noexpand and returns the frozen CS.
                        // Expand leftover expandable prims (\\expanded) so
                        // \\csname\\noexpand\\expanded{...}\\endcsname works.
                        // Do NOT re-expand macros — that steals following args.
                        match self.eqtb.resolve(id).cloned() {
                            Some(Equiv::Prim(p)) if self.is_expandable(p) => {
                                match self.expand_prim(p, id) {
                                    Some(tok) if tok.is_char() => {
                                        tok.append_character_bytes(&mut name);
                                    }
                                    Some(tok) => self.push_token(tok),
                                    None => {}
                                }
                                continue;
                            }
                            _ => {
                                self.push_token(t);
                                self.error_at(
                                    "Missing \\endcsname inserted",
                                    csname_origin
                                        .as_ref()
                                        .map(crate::input::SourceMark::to_context),
                                );
                                break;
                            }
                        }
                    }

                    if t.is_char() && t.cc() == 9 {
                        continue;
                    }
                    t.append_character_bytes(&mut name);
                }
                self.csname_depth = self.csname_depth.saturating_sub(1);
                let id = self.cs.intern(&name);
                self.last_named_cs = Some(id);
                if self.eqtb.get(id).is_none() {
                    // l3 variant wrappers reference \exp_args:N<spec>
                    // expanders lazily (\exp_not:c{exp_args:NNcc}). If the
                    // boot has not generated that spec yet, the plain relax
                    // default would poison the name (cs_if_free then reports
                    // it as defined and the synthesis never runs). Synthesize
                    // the real expander on creation instead.
                    let relax = self.cs.lookup(b"relax").unwrap();
                    let r = self.eqtb.get(relax).cloned();
                    if let Some(e) = r {
                        self.eqtb.assign(id, e, false);
                    }
                }
                if let Some(mark) = csname_origin {
                    self.diagnostic_synthetic_source = Some((id, mark, csname_span.max(1)));
                }
                Some(Token::from_cs(id))
            }

            LastNamedCs => {
                let id = self
                    .last_named_cs
                    .unwrap_or_else(|| self.cs.lookup(b"relax").unwrap());
                Some(Token::from_cs(id))
            }
            The => {
                self.the_scan();
                None
            }
            Prim::String => {
                let t = self.raw_token();
                let mut bytes: Vec<u8> = Vec::new();
                let esc = self.eqtb.int_params[crate::prim::IntParam::EscapeChar.idx() as usize];
                if t.is_cs() {
                    let name = self.cs.name(t.cs_id());
                    if let Some((source_bytes, len)) = Self::active_cs_source_bytes(name) {
                        bytes.extend_from_slice(&source_bytes[..len]);
                    } else {
                        if esc >= 0 && esc <= 255 {
                            bytes.push(esc as u8);
                        }
                        bytes.extend_from_slice(name);
                    }
                } else {
                    t.append_character_bytes(&mut bytes);
                }
                self.exp_string(&bytes);
                None
            }
            Prim::Meaning => {
                let t = self.raw_token();
                let text = self.meaning_of(t);
                self.exp_string(text.as_bytes());
                None
            }
            Number => {
                let n = self.scan_int();
                let s = n.to_string();
                self.exp_string(s.as_bytes());
                None
            }
            RomanNumeral => {
                let n = self.scan_int();
                let s = if n <= 0 {
                    ::std::string::String::new()
                } else {
                    roman(n)
                };
                self.exp_string(s.as_bytes());
                None
            }
            Detokenize => {
                let toks = self.scan_general_text();
                let text = self.tokens_to_string(&toks);
                self.exp_string(text.as_bytes());
                None
            }
            Expanded => {
                let r = self.scan_general_text_expanded();

                self.push_tokens(r);
                None
            }
            UnExpanded => {
                self.skip_spaces_relax();
                let t = self.get_x_raw();

                if t.is_cs() {
                    if let Some(Equiv::ToksReg(i)) = self.eqtb.resolve(t.cs_id()).cloned() {
                        let toks = (*self.eqtb.toks[i as usize]).clone();
                        self.push_tokens_exp_not(toks);
                        return None;
                    }
                    // e-TeX pair `\unexpanded\expanded{{X}}`: run the
                    // \expanded to completion, deliver its result frozen.
                    // l3's \__kernel_exp_not:w = \tex_unexpanded:D, so
                    // keyval/tl machinery leans on this exact idiom.
                    if let Some(Equiv::Prim(p)) = self.eqtb.resolve(t.cs_id()).cloned() {
                        if p == Prim::Expanded {
                            let mut toks = self.scan_general_text_expanded();
                            Self::strip_outer_braces(&mut toks);
                            if self.in_expanded_scan {
                                self.push_tokens_exp_not(toks);
                            } else {
                                self.push_tokens(toks);
                            }

                            return None;
                        }
                    }
                }
                self.push_token(t);
                let toks = self.scan_general_text();
                if self.in_expanded_scan {
                    self.push_tokens_exp_not(toks);
                } else {
                    self.push_tokens(toks);
                }
                None
            }
            ScanTokens => {
                // \scantokens{...}: stringify and rescan
                let toks = self.scan_general_text();
                let text = self.tokens_to_string(&toks);
                if self.ensure_input_stack_room(1) {
                    self.input
                        .push_file("<scantokens>".to_string(), text.into_bytes());
                }
                None
            }
            DirectLua => {
                self.skip_spaces_relax();
                let t = self.get_token();
                if t.is_char() && t.chr() == u32::from(b'[') {
                    let _ = self.scan_int();
                    let close = self.get_token();
                    if !(close.is_char() && close.chr() == u32::from(b']')) {
                        self.push_token(close);
                    }
                } else {
                    self.push_token(t);
                }
                let toks = self.scan_general_text_expanded();
                let code = self.tokens_to_string(&toks);
                if let Err(err) = self.execute_directlua(&code) {
                    self.error(&format!("LuaTeX error: {err}"));
                }
                None
            }
            Input => {
                self.do_input();
                None
            }
            NumExpr => {
                let v = self.scan_expr_num();
                self.exp_string(v.to_string().as_bytes());
                None
            }
            DimExpr => {
                let v = self.scan_expr_dim();
                let text = self.scaled_to_string(v);
                self.exp_string(text.as_bytes());
                None
            }
            GlueExpr => {
                let g = self.scan_expr_glue(false);
                let text = self.glue_to_string(&g);
                self.exp_string(text.as_bytes());
                None
            }
            MuExpr => {
                let g = self.scan_expr_glue(true);
                let text = self.mu_glue_to_string(&g);
                self.exp_string(text.as_bytes());
                None
            }
            Prim::FontName => {
                let f = self.scan_font_id();
                let text = self.font_display_name(f);
                self.exp_string(text.as_bytes());
                None
            }
            Prim::JobName => {
                let text = self.job_name.clone();
                self.exp_string(text.as_bytes());
                None
            }
            Prim::FontIdPrim => {
                let f = self.scan_font_id();
                let text = f.to_string();
                self.exp_string(text.as_bytes());
                None
            }
            Prim::PdfFontSize => {
                let f = self.scan_font_id();
                let size = self
                    .eqtb
                    .fonts
                    .get(f as usize)
                    .map(|font| font.at_size)
                    .unwrap_or(0);
                let text = self.scaled_to_string(size);
                self.exp_string(text.as_bytes());
                None
            }
            Prim::PdfBanner => {
                self.exp_string(b"This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux) kpathsea version 6.4.2");
                None
            }
            Prim::LeftMarginKern | Prim::RightMarginKern => {
                // pdftex §11371: skip discardables and the structural
                // \leftskip/\rightskip glue, then report the margin kern's
                // width (0 if the line box has none).
                let left = p == LeftMarginKern;
                let n = self.scan_reg_num();
                let width = self.margin_kern_width(n, left);
                let s = self.scaled_to_string(width);
                self.exp_string(s.as_bytes());
                None
            }
            Unless => {
                // e-TeX \\unless prefixes the next conditional; the flag is
                // consumed by do_if. Do not restore: a save/restore around
                // expand_prim_of_next re-armed the flag after do_if cleared
                // it and made \\ifx inside \\csname keep a leftover `=`.
                self.unless_next = true;
                self.expand_prim_of_next()
            }
            IfTrue => self.do_if(true, id),
            IfFalse => self.do_if(false, id),
            IfChar => {
                // tex.web 498: push this \\if *before* get_x_token so a nested
                // true conditional from the test sits on top (babel
                // `\\if T\\ifeof1F\\fi T`).
                let unless = std::mem::take(&mut self.unless_next);
                let save = self.push_if(id);
                let a = self.character_test_operand();
                let b = self.character_test_operand();
                // tex.web: CS tokens have character code 256, so two
                // control sequences always compare equal for \\if.
                let mut eq = if a.is_cs() && b.is_cs() {
                    true
                } else if !a.is_cs() && !b.is_cs() {
                    a.chr() == b.chr()
                } else {
                    false
                };
                if unless {
                    eq = !eq;
                }
                self.finish_if(save, eq)
            }
            IfCat => {
                let unless = std::mem::take(&mut self.unless_next);
                let save = self.push_if(id);
                let a = self.character_test_operand();
                let b = self.character_test_operand();
                // tex.web: CS tokens have category 16.
                let mut eq = if a.is_cs() && b.is_cs() {
                    true
                } else if !a.is_cs() && !b.is_cs() {
                    a.cc() == b.cc()
                } else {
                    false
                };
                if unless {
                    eq = !eq;
                }
                self.finish_if(save, eq)
            }
            IfVMode => self.do_if(self.mode.is_v(), id),
            IfHMode => self.do_if(self.mode.is_h(), id),
            IfMMode => self.do_if(self.mode.is_m(), id),
            IfInner => {
                let ok = matches!(
                    self.mode,
                    crate::engine::Mode::InternalVertical
                        | crate::engine::Mode::RestrictedHorizontal
                        | crate::engine::Mode::Math
                );
                self.do_if(ok, id)
            }
            IfDef => {
                let t = self.raw_token();
                let def = if t.is_cs() {
                    self.eqtb.resolve(t.cs_id()).is_some()
                } else if t.is_char() && t.cc() == 13 {
                    let id = self.active_cs_id(t.chr());
                    self.eqtb.resolve(id).is_some()
                } else {
                    false
                };
                self.do_if(def, id)
            }
            IfInCsName => self.do_if(self.csname_depth > 0, id),

            IfCSName => {
                // e-TeX \\ifcsname: get_x_token until \\endcsname; true iff
                // the name is already in the hash (even if \\relax). Must not
                // intern on a miss — that would poison \\ifcsname.
                // Take \\unless now so name collection cannot flip nested
                // \\ifx; restore before do_if so \\unless\\ifcsname inverts.
                let unless = std::mem::take(&mut self.unless_next);
                self.csname_depth += 1;
                let mut name: Vec<u8> = Vec::new();
                loop {
                    let t = self.get_x_raw();
                    if t == EOF_MARKER {
                        self.error("Missing \\endcsname inserted");
                        break;
                    }
                    if t.is_cs() {
                        let id = t.cs_id();
                        let is_end = matches!(
                            self.eqtb.resolve(id),
                            Some(Equiv::Prim(crate::prim::Prim::EndCsName))
                        );
                        if is_end {
                            break;
                        }
                        match self.eqtb.resolve(id).cloned() {
                            Some(Equiv::Prim(p)) if self.is_expandable(p) => {
                                match self.expand_prim(p, id) {
                                    Some(tok) if tok.is_char() => {
                                        tok.append_character_bytes(&mut name)
                                    }
                                    Some(tok) => self.push_token(tok),
                                    None => {}
                                }
                                continue;
                            }
                            _ => {
                                self.push_token(t);
                                self.error("Missing \\endcsname inserted");
                                break;
                            }
                        }
                    }
                    if t.is_char() && t.cc() == 9 {
                        continue;
                    }
                    t.append_character_bytes(&mut name);
                }
                self.csname_depth = self.csname_depth.saturating_sub(1);
                let def = if let Some(id) = self.cs.lookup(&name) {
                    self.last_named_cs = Some(id);
                    self.eqtb.resolve(id).is_some()
                } else {
                    false
                };
                self.unless_next = unless;
                self.do_if(def, id);
                None
            }
            IfX => {
                // tex.web if_x: operands see expandable PRIMS (\\csname...)
                // but never macros (\\ifx\\foo x is false for \\def\\foo{x}).
                let a = self.raw_token();
                let b = self.raw_token();

                let eq = self.ifx_equal(a, b);
                self.do_if(eq, id)
            }
            Or => {
                if self.if_stack.last().is_some_and(|st| st.evaluating) {
                    self.insert_relax(id);
                    return None;
                }
                // encountered while accepting: skip to \fi or next \or
                if let Some(st) = self.if_stack.last_mut() {
                    if st.matched {
                        st.if_case = -1; // skip mode
                        self.skip_case_skip();
                        return None;
                    }
                }
                self.error("Extra \\or");
                None
            }
            Else => {
                if self.if_stack.last().is_some_and(|st| st.evaluating) {
                    self.insert_relax(id);
                    return None;
                }
                let st = self.if_stack.last().cloned();
                match st {
                    Some(s) => {
                        if s.matched {
                            self.skip_to_fi();
                        } else {
                            let st2 = self.if_stack.last_mut().unwrap();
                            st2.accepting = true;
                            st2.matched = true;
                        }
                    }
                    None => self.error("Extra \\else"),
                }
                None
            }
            ElIf | ElIfX => {
                if self.if_stack.last().is_some_and(|st| st.evaluating) {
                    self.insert_relax(id);
                    return None;
                }
                // TeX has no \elseif; treat like \else that never accepts
                let st = self.if_stack.last().cloned();
                match st {
                    Some(s) => {
                        if s.matched {
                            self.skip_to_fi();
                        } else {
                            self.error("\\elseif not supported");
                        }
                    }
                    None => self.error("Extra \\elseif"),
                }
                None
            }
            Fi => {
                if self.if_stack.last().is_some_and(|st| st.evaluating) {
                    self.insert_relax(id);
                    return None;
                }
                if self.if_stack.pop().is_none() {
                    self.error("Extra \\fi");
                }
                None
            }
            PdfTexRevision => {
                self.exp_string(b"29");
                None
            }
            RatexUnicodeVersion => {
                self.exp_string(b"1");
                None
            }
            RatexNativeTextMode => {
                self.exp_string(if self.native_text_active() && !self.mode.is_m() {
                    b"1"
                } else {
                    b"0"
                });
                None
            }
            RatexUtfEight => {
                let tokens = self.scan_general_text();
                let mut bytes = [0u8; 4];
                let mut valid = (2..=4).contains(&tokens.len());
                for (token, byte) in tokens.iter().zip(bytes.iter_mut()) {
                    let value = if token.is_cs() {
                        let name = self.cs.name(token.cs_id());
                        (name.len() == 1).then(|| name[0] as u32)
                    } else {
                        Some(token.chr())
                    };
                    match value.and_then(|value| u8::try_from(value).ok()) {
                        Some(value) => *byte = value,
                        None => valid = false,
                    }
                }
                let scalar = if valid {
                    std::str::from_utf8(&bytes[..tokens.len()])
                        .ok()
                        .and_then(|text| {
                            let mut chars = text.chars();
                            let scalar = chars.next()?;
                            chars.next().is_none().then_some(scalar)
                        })
                } else {
                    None
                };
                if let Some(scalar) = scalar {
                    Some(Token::unicode_char(12, scalar as u32))
                } else {
                    self.error("Invalid UTF-8 sequence in native text");
                    None
                }
            }
            UcharCat => {
                let c = self.scan_unicode_character_code("\\Ucharcat");
                let (category, source) = self.scan_int_with_source();
                let cat = if (0..=15).contains(&category) {
                    category as u8
                } else {
                    self.error_at(
                        &format!(
                            "Category code {category} is out of range for \\Ucharcat; expected 0 through 15 and used category 12"
                        ),
                        source,
                    );
                    12
                };
                self.push_token(Token::unicode_char(cat, c));
                None
            }
            PdfFileSize | FileSize => {
                let name = {
                    let t = self.scan_general_text_expanded();
                    self.tokens_to_string(&t)
                };
                let name = name.trim();
                let loaded_files_before_lookup = self.loaded_files.len();
                let font_files_before_lookup = self.font_loader.dependency_files.len();
                let resolved = self
                    .resolve_input_path(name)
                    .or_else(|| self.font_loader.kpse.find_any(name));
                // Resolving an input normally records a content dependency.
                // This primitive observes only metadata, so keep its cheaper,
                // size-specific dependency unless another operation consumes
                // the same file independently.
                self.loaded_files.truncate(loaded_files_before_lookup);
                if let Some(path) = resolved.as_deref() {
                    self.font_loader
                        .discard_file_dependency_since(font_files_before_lookup, path);
                }
                let sz = resolved
                    .and_then(|path| {
                        let metadata = tex_kpse::fs::metadata(&path).ok()?;
                        if !metadata.is_file() {
                            return None;
                        }
                        let size = metadata.len();
                        self.loaded_file_sizes.push((path, size));
                        Some(size)
                    })
                    .or_else(|| {
                        tex_kpse::get_embedded_tex_input(name)
                            .map(|(_resolved_name, data)| data.len() as u64)
                    });
                if let Some(n) = sz {
                    self.exp_string(n.to_string().as_bytes());
                }
                None
            }
            TopMark | FirstMark | BotMark | SplitFirstMark | SplitBotMark | TopMarksClass
            | FirstMarksClass | BotMarksClass | SplitFirstMarksClass | SplitBotMarksClass => {
                let which = match p {
                    TopMark | TopMarksClass => 0,
                    FirstMark | FirstMarksClass => 1,
                    BotMark | BotMarksClass => 2,
                    SplitFirstMark | SplitFirstMarksClass => 3,
                    _ => 4,
                };
                let class = if matches!(
                    p,
                    TopMarksClass
                        | FirstMarksClass
                        | BotMarksClass
                        | SplitFirstMarksClass
                        | SplitBotMarksClass
                ) {
                    self.scan_int()
                } else {
                    0
                };
                self.push_mark_tokens_class(which, class);
                None
            }
            PdfMatch => {
                let origin = self.current_token_source_mark();
                let icase = self.scan_keyword(b"icase");
                let count = if self.scan_keyword(b"subcount") {
                    self.scan_int()
                } else {
                    10
                };
                let count = if count < 0 { 10 } else { count as usize };
                let pattern = {
                    let t = self.scan_general_text_expanded();
                    self.tokens_to_string(&t)
                };
                let subject = {
                    let t = self.scan_general_text_expanded();
                    self.tokens_to_string(&t).into_bytes()
                };
                match posix_regex::PosixRegexBuilder::new(pattern.as_bytes())
                    .with_default_classes()
                    .extended(true)
                    .compile()
                {
                    Ok(regex) => {
                        let mut matches = regex.case_insensitive(icase).matches(&subject, Some(1));
                        let found = !matches.is_empty();
                        self.pdf_match_ranges.clear();
                        if let Some(captures) = matches.pop() {
                            self.pdf_match_ranges
                                .extend(captures.iter().take(count).copied());
                        }
                        self.pdf_match_subject = subject;
                        self.exp_string(if found { b"1" } else { b"0" });
                    }
                    Err(error) => {
                        // pdfTeX preserves previous captures when compilation fails.
                        self.warning_at(
                            &format!(
                                "Invalid regular expression in \\pdfmatch: {}",
                                posix_regex_error_detail(&error)
                            ),
                            origin.map(|mark| mark.to_context()),
                        );
                        self.exp_string(b"-1");
                    }
                }
                None
            }
            PdfLastMatch => {
                let index = self.scan_int();
                let capture = usize::try_from(index)
                    .ok()
                    .and_then(|i| self.pdf_match_ranges.get(i).copied().flatten());
                let mut text = Vec::new();
                if let Some((start, end)) = capture {
                    text.extend_from_slice(start.to_string().as_bytes());
                    text.extend_from_slice(b"->");
                    text.extend_from_slice(&self.pdf_match_subject[start..end]);
                } else {
                    text.extend_from_slice(b"-1->");
                }
                self.exp_string(&text);
                None
            }
            PdfStrCmp => {
                let a = {
                    let t = self.scan_general_text_expanded();
                    self.tokens_to_string(&t)
                };
                let b = {
                    let t = self.scan_general_text_expanded();
                    self.tokens_to_string(&t)
                };
                let n = match a.cmp(&b) {
                    std::cmp::Ordering::Less => "-1",
                    std::cmp::Ordering::Equal => "0",
                    std::cmp::Ordering::Greater => "1",
                };
                self.exp_string(n.as_bytes());
                None
            }
            PdfMdFiveSum => {
                let file = self.scan_keyword(b"file");
                let toks = self.scan_general_text_expanded();
                let bytes = self.tokens_to_bytes(&toks);
                let digest = if file {
                    let name = std::string::String::from_utf8_lossy(&bytes);
                    let data = if let Some(path) = self
                        .resolve_input_path(name.trim())
                        .or_else(|| self.font_loader.kpse.find_any(name.trim()))
                    {
                        let Ok(data) = tex_kpse::fs::read(&path) else {
                            return None;
                        };
                        self.record_loaded_bytes(&path, &data);
                        self.loaded_files.push(path);
                        data
                    } else {
                        let clean = std::path::Path::new(name.trim())
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(name.trim());
                        let Some(pkg_data) = tex_kpse::get_embedded_package(clean) else {
                            return None;
                        };
                        pkg_data
                    };
                    md5::compute(&data)
                } else {
                    md5::compute(&bytes)
                };
                self.exp_string(format!("{digest:X}").as_bytes());
                None
            }
            PdfFileModDate => {
                let _ = self.scan_general_text_expanded();
                self.exp_string(b"D:20260101000000Z");
                None
            }
            PdfCreationDate => {
                // Keep metadata-dependent package logic deterministic, matching
                // the stable timestamp used by the file-date compatibility shim.
                self.exp_string(b"D:20260101000000Z");
                None
            }
            PdfFileDump => {
                use std::io::{Read, Seek, SeekFrom};

                let max_dump_bytes = crate::input::MAX_TOKEN_LIST_TOKENS / 2;
                let offset = if self.scan_keyword(b"offset") {
                    self.scan_int()
                } else {
                    0
                };
                let length = if self.scan_keyword(b"length") {
                    self.scan_int()
                } else {
                    0
                };
                let name = self.scan_pdf_string();
                if offset < 0 || length < 0 {
                    self.error("Negative offset or length in \\pdffiledump");
                    return None;
                }
                let bytes = if let Some(path) = self
                    .resolve_input_path(name.trim())
                    .or_else(|| self.font_loader.kpse.find_any(name.trim()))
                {
                    // Only the requested range affects this expansion. Reading
                    // the whole input here makes a one-byte dump allocate the
                    // size of an arbitrarily large file. Until the dependency
                    // cache can represent byte-range reads, fail closed rather
                    // than publishing a cache entry sampled after the pass.
                    self.font_loader.dependency_tracking_complete = false;
                    let Ok(mut input) = tex_kpse::fs::File::open(&path) else {
                        return None;
                    };
                    if input.seek(SeekFrom::Start(offset as u64)).is_err() {
                        return None;
                    }
                    let mut data = Vec::new();
                    // Read one byte beyond the largest representable hex token
                    // list. This distinguishes an oversized result without
                    // allocating the full user-supplied length, while a huge
                    // request near EOF can still return a small legal tail.
                    let read_limit = (length as u64).min(max_dump_bytes as u64 + 1);
                    if input.take(read_limit).read_to_end(&mut data).is_err() {
                        return None;
                    }
                    self.loaded_files.push(path);
                    data
                } else {
                    let clean = std::path::Path::new(name.trim())
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(name.trim());
                    let Some(pkg_data) = tex_kpse::get_embedded_package(clean) else {
                        return None;
                    };
                    let start = (offset as usize).min(pkg_data.len());
                    let end = start.saturating_add(length as usize).min(pkg_data.len());
                    pkg_data[start..end].to_vec()
                };
                if !self.ensure_token_list_room(bytes.len().saturating_mul(2)) {
                    return None;
                }
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                let mut hex = Vec::with_capacity(bytes.len() * 2);
                for byte in bytes {
                    hex.push(HEX[(byte >> 4) as usize]);
                    hex.push(HEX[(byte & 15) as usize]);
                }
                self.exp_string(&hex);
                None
            }
            PdfElapsedTime => {
                self.exp_string(b"0");
                None
            }
            PdfColorStackInit => {
                let _ = self.scan_keyword(b"page");
                let _ = self.scan_keyword(b"direct");
                let _ = self.scan_general_text_expanded();
                self.exp_string(b"0");
                None
            }
            PdfUniformDeviate | PdfNormalDeviate => {
                let _ = self.scan_int();
                self.exp_string(b"0");
                None
            }
            PdfEscapeString | PdfEscapeName => {
                let s = {
                    let t = self.scan_general_text_expanded();
                    self.tokens_to_string(&t)
                };
                self.exp_string(s.as_bytes());
                None
            }
            PdfEscapeHex => {
                let toks = self.scan_general_text_expanded();
                let s = self.tokens_to_string(&toks);
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                let mut encoded = Vec::with_capacity(s.len() * 2);
                for b in s.bytes() {
                    encoded.push(HEX[(b >> 4) as usize]);
                    encoded.push(HEX[(b & 0x0f) as usize]);
                }
                self.exp_string(&encoded);
                None
            }
            PdfUnescapeHex => {
                let toks = self.scan_general_text_expanded();
                let s = self.tokens_to_string(&toks);
                let mut decoded = Vec::with_capacity(s.len() / 2);
                let mut high = None;
                for b in s.bytes().filter(|b| !b.is_ascii_whitespace()) {
                    let Some(nibble) = (b as char).to_digit(16).map(|n| n as u8) else {
                        continue;
                    };
                    if let Some(h) = high.take() {
                        decoded.push((h << 4) | nibble);
                    } else {
                        high = Some(nibble);
                    }
                }
                if let Some(h) = high {
                    decoded.push(h << 4);
                }
                self.exp_string(&decoded);
                None
            }
            Prim::XeTeXRevision | Prim::XeTeXGlyphName | Prim::XeTeXFeatureName | Prim::XeTeXVariationName => {
                self.expand_xetex_query(p);
                None
            }
            Prim::LuaTeXRevision => {
                self.exp_string(b"0");
                None
            }
            Prim::LuaTeXBanner => {
                self.exp_string(b"This is LuaTeX, Version 1.24.0");
                None
            }
            _ => None,
        }
    }
    /// Remove an already-consumed `\noexpand` marker before TeX compares or
    /// stores the token as macro input. `\unexpanded` has a distinct marker
    /// because it must survive any intervening macro-argument scanners.
    pub(crate) fn unfreeze_input_token(&self, t: Token) -> Token {
        if t.0 >= UNEXPANDED_CS_FLAG && t.0 < 0xFFFF_0000 {
            return t;
        }
        let id = if t.0 >= NOEXP_FLAG && t.0 < UNEXPANDED_CS_FLAG {
            t.0 & 0x3FFF_FFFF
        } else if t.is_cs() {
            t.cs_id()
        } else {
            return t;
        };
        let name = self.cs.name(id);
        if let Some(scalar) = Self::active_cs_scalar(name) {
            Token::char(13, scalar)
        } else {
            Token::from_cs(id)
        }
    }

    fn unfreeze_unexpanded_token(&self, t: Token) -> Token {
        if t.0 >= UNEXPANDED_CS_FLAG && t.0 < 0xFFFF_0000 {
            let id = t.0 & 0x1FFF_FFFF;
            let name = self.cs.name(id);
            if let Some(scalar) = Self::active_cs_scalar(name) {
                Token::char(13, scalar)
            } else {
                Token::from_cs(id)
            }
        } else {
            t.unfreeze()
        }
    }

    /// Return the character/category identity used by `\if` and `\ifcat`.
    ///
    /// `\noexpand` marks an expandable control sequence with `NOEXP_FLAG`.
    /// For an active character TeX still exposes the original character
    /// token to these tests, not a category-16 control sequence.
    fn character_test_operand(&mut self) -> Token {
        let t = self.get_token();
        let mut t = if self.no_expand_tok == Some(t) {
            self.unfreeze_input_token(Token(NOEXP_FLAG | t.cs_id()))
        } else {
            self.unfreeze_input_token(t)
        };
        if t.is_cs() {
            if let Some(Equiv::CharTok(value)) = self.eqtb.resolve(t.cs_id()) {
                t = Token(*value);
            }
        }
        t
    }

    /// dispatch the token after \unless (must be a conditional)
    fn expand_prim_of_next(&mut self) -> Option<Token> {
        let t = self.raw_token();
        if !t.is_cs() {
            self.error("Missing \\if after \\unless");
            return Some(t);
        }
        let id = t.cs_id();
        match self.eqtb.resolve(id) {
            Some(Equiv::Prim(p)) if self.is_expandable(*p) => self.expand_prim(*p, id),
            _ => {
                self.error("Missing \\if after \\unless");
                Some(t)
            }
        }
    }
    pub(crate) fn insert_relax(&mut self, cur_cs: CsId) {
        let relax_id = self.cs.lookup(b"relax").unwrap();
        self.push_token(Token::from_cs(cur_cs));
        self.push_token(Token::from_cs(relax_id));
    }
    fn push_if(&mut self, id: CsId) -> usize {
        let loc = self.current_token_source_mark();
        self.if_stack.push(crate::engine::IfState {
            accepting: false,
            matched: false,
            if_case: -1,
            evaluating: true,
            loc_file: self.input.current_file_name(),
            loc_line: self.input.current_file_line(),
            loc_cs: id,
            loc,
        });
        self.if_stack.len() - 1
    }

    fn finish_if(&mut self, save: usize, b: bool) -> Option<Token> {
        if let Some(st) = self.if_stack.get_mut(save) {
            st.evaluating = false;
        }
        if b {
            if let Some(st) = self.if_stack.get_mut(save) {
                st.accepting = true;
                st.matched = true;
            }
        } else {
            self.skip_to_else_or_fi(save);
        }
        None
    }

    fn do_if(&mut self, mut b: bool, id: CsId) -> Option<Token> {
        if self.unless_next {
            b = !b;
            self.unless_next = false;
        }
        let save = self.push_if(id);
        self.finish_if(save, b)
    }

    /// Fetch an \\ifx operand: expandable prims (\\csname, \\expandafter)
    /// expand; macros and everything else stay raw (tex.web if_x).
    fn ifx_operand(&mut self) -> Token {
        loop {
            let t = self.raw_token();
            if t.is_cs() {
                if let Some(Equiv::Prim(p)) = self.eqtb.resolve(t.cs_id()).cloned() {
                    if self.is_expandable(p) {
                        match self.expand_prim(p, t.cs_id()) {
                            Some(tok) => {
                                self.push_token(tok);
                                continue;
                            }
                            None => continue,
                        }
                    }
                }
            }
            return t;
        }
    }

    fn is_if_test(p: Prim) -> bool {
        matches!(
            p,
            Prim::IfChar
                | Prim::IfCat
                | Prim::IfOdd
                | Prim::IfNum
                | Prim::IfDim
                | Prim::IfVoid
                | Prim::IfHBox
                | Prim::IfVBox
                | Prim::IfHMode
                | Prim::IfVMode
                | Prim::IfInner
                | Prim::IfMMode
                | Prim::IfTrue
                | Prim::IfFalse
                | Prim::IfEOF
                | Prim::IfDef
                | Prim::IfCSName
                | Prim::IfInCsName
                | Prim::IfX
                | Prim::IfCase
                | Prim::IfFontChar
        )
    }

    /// tex.web §494: unexpanded skip to next \fi/\else/\or at local depth 0.
    fn pass_text(&mut self) -> Option<Prim> {
        let save_scanner = self.scanner_status;
        self.scanner_status = ScannerStatus::Skipping;
        let mut l = 0i32;
        let res = 'skip: loop {
            if self.pushed.is_empty() {
                if let Some(crate::input::Source::TokList {
                    toks, pos, name, ..
                }) = self.input.stack.last_mut()
                {
                    let is_peek = *name == crate::align::PEEK_SRC;
                    let s = &toks[..];
                    while *pos < s.len() && s[*pos].0 < 0x8000_0000 {
                        let tok = s[*pos];
                        if !is_peek && !tok.is_cs() {
                            let cc = (tok.0 >> 24) as u8;
                            if cc == 1 {
                                self.align_brace_depth = self.align_brace_depth.saturating_add(1);
                            } else if cc == 2 {
                                self.align_brace_depth = self.align_brace_depth.saturating_sub(1);
                            }
                        }
                        *pos += 1;
                    }
                    if *pos == s.len() {
                        if let Some(crate::input::Source::TokList {
                            toks: crate::input::TokTokens::Vec(mut v),
                            ..
                        }) = self.input.stack.pop()
                        {
                            v.clear();
                            if self.token_vec_pool.len() < 512 {
                                self.token_vec_pool.push(v);
                            }
                        }
                        continue;
                    }
                }
            }
            let t = self.raw_token();
            if t == EOF_MARKER {
                self.fatal_conditional_eof();
                break 'skip None;
            }
            if !t.is_cs() {
                continue;
            }
            let is = match self.eqtb.resolve(t.cs_id()) {
                Some(Equiv::Prim(p)) => *p,
                _ => continue,
            };
            if Self::is_if_test(is) {
                l += 1;
            } else if is == Prim::Unless {
                self.skip_count_unless_target(&mut l);
            } else if matches!(
                is,
                Prim::Fi | Prim::Else | Prim::Or | Prim::ElIf | Prim::ElIfX
            ) {
                if l == 0 {
                    break 'skip Some(is);
                }
                if is == Prim::Fi {
                    l -= 1;
                }
            }
        };
        self.scanner_status = save_scanner;
        res
    }

    /// tex.web 498: after a false test, keep skipping until the closer
    /// belongs to *this* \\if. A nested still-open true \\if from the test
    /// (`\\if T\\iftrue F\\fi T`) has its \\fi popped here; skip continues.
    fn skip_to_else_or_fi(&mut self, save: usize) {
        loop {
            let Some(chr) = self.pass_text() else {
                return;
            };
            if self.if_stack.len().saturating_sub(1) == save {
                match chr {
                    Prim::Fi => {
                        self.if_stack.pop();
                        return;
                    }
                    Prim::Else | Prim::ElIf | Prim::ElIfX => {
                        if let Some(st) = self.if_stack.get_mut(save) {
                            st.accepting = true;
                            st.matched = true;
                        }
                        return;
                    }
                    Prim::Or => {
                        self.error("Extra \\or");
                        return;
                    }
                    _ => return,
                }
            } else if chr == Prim::Fi {
                self.if_stack.pop();
            }
        }
    }

    /// e-TeX `\\unless` during skip: the following *conditional* is one opener
    /// together with `\\unless`. If the next token is not a conditional (the
    /// expl3 `\\expandafter\\unless\\fi\\ifx` pattern while skipping), leave it.
    fn skip_count_unless_target(&mut self, depth: &mut i32) {
        let t2 = self.raw_token();
        if t2 == EOF_MARKER || t2 == crate::input::PAR_END {
            return;
        }
        if t2.is_cs() {
            if let Some(Equiv::Prim(p)) = self.eqtb.resolve(t2.cs_id()) {
                use Prim::*;
                if matches!(p, Unless) {
                    self.skip_count_unless_target(depth);
                    return;
                }
                if matches!(
                    p,
                    IfChar
                        | IfCat
                        | IfOdd
                        | IfNum
                        | IfDim
                        | IfVoid
                        | IfHBox
                        | IfVBox
                        | IfHMode
                        | IfVMode
                        | IfInner
                        | IfMMode
                        | IfTrue
                        | IfFalse
                        | IfEOF
                        | IfDef
                        | IfCSName
                        | IfInCsName
                        | IfX
                        | IfCase
                        | IfFontChar
                ) {
                    *depth += 1;
                    return;
                }
            }
        }
        self.push_token(t2);
    }

    /// Skip to the branch delimiter belonging to `target`. Numeric operands
    /// can leave expanded nested conditionals open above an \ifcase frame.
    fn skip_branch(&mut self, if_case: bool, target: usize) {
        let save = self.scanner_status;
        self.scanner_status = ScannerStatus::Skipping;
        self.skip_branch_inner(if_case, target);
        self.scanner_status = save;
    }

    fn skip_branch_inner(&mut self, if_case: bool, target: usize) {
        let mut depth = 0i32;
        loop {
            let t = self.raw_token();
            if !t.is_cs() {
                continue;
            }
            let is = match self.eqtb.resolve(t.cs_id()) {
                Some(Equiv::Prim(p)) => *p,
                _ => continue,
            };
            if Self::is_if_test(is) {
                depth += 1;
            } else {
                match is {
                    Prim::Unless => self.skip_count_unless_target(&mut depth),
                    Prim::Fi => {
                        if depth > 0 {
                            depth -= 1;
                        } else if self.if_stack.len() > target + 1 {
                            // This closes a conditional opened while scanning the
                            // target's numeric operand, not the target itself.
                            self.if_stack.pop();
                        } else {
                            self.if_stack.pop();
                            return;
                        }
                    }
                    Prim::Or => {
                        if depth == 0 && self.if_stack.len() == target + 1 {
                            if if_case {
                                let st = self.if_stack.last_mut().unwrap();
                                if st.matched {
                                    self.skip_case_skip();
                                    return;
                                }
                                st.if_case -= 1;
                                if st.if_case == 0 {
                                    st.accepting = true;
                                    st.matched = true;
                                    return;
                                }
                            }
                        }
                    }
                    Prim::ElIf | Prim::ElIfX => {
                        if depth == 0 && self.if_stack.len() == target + 1 {
                            self.skip_branch(false, target);
                            return;
                        }
                    }
                    Prim::Else => {
                        if depth == 0 && self.if_stack.len() == target + 1 {
                            let st = self.if_stack.last().cloned();
                            match st {
                                Some(s) => {
                                    if s.matched {
                                        self.skip_to_fi();
                                    } else {
                                        let st2 = self.if_stack.last_mut().unwrap();
                                        st2.accepting = true;
                                        st2.matched = true;
                                    }
                                }
                                None => {}
                            }
                            return;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn fatal_conditional_eof(&mut self) {
        let count = self.if_stack.len();
        let openings = self
            .if_stack
            .iter()
            .rev()
            .take(3)
            .map(|state| {
                let file = state.loc_file.rsplit('/').next().unwrap_or("?");
                let mut end = file.len().min(256);
                while end > 0 && !file.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}:{}", &file[..end], state.loc_line)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let source = self
            .if_stack
            .last()
            .and_then(|state| state.loc.as_ref().map(crate::input::SourceMark::to_context));
        self.fatal_error_at(
            &format!(
                "File ended while scanning conditional ({count} open; innermost at {openings})"
            ),
            source,
        );
    }

    /// skip until \fi (used after \else when branch matched)
    fn skip_to_fi(&mut self) {
        let save = self.scanner_status;
        self.scanner_status = ScannerStatus::Skipping;
        self.skip_to_fi_inner();
        self.scanner_status = save;
    }

    fn skip_to_fi_inner(&mut self) {
        let mut depth = 0i32;
        loop {
            let t = self.raw_token();
            if t == EOF_MARKER {
                return;
            }
            if !t.is_cs() {
                continue;
            }
            match self.eqtb.resolve(t.cs_id()) {
                Some(Equiv::Prim(p)) => {
                    if Self::is_if_test(*p) {
                        depth += 1;
                    } else if *p == Prim::Unless {
                        self.skip_count_unless_target(&mut depth);
                    } else if *p == Prim::Fi {
                        if depth == 0 {
                            self.if_stack.pop();
                            return;
                        }
                        depth -= 1;
                    }
                }
                _ => {}
            }
        }
    }

    /// skip to \fi within an ifcase that already matched
    fn skip_case_skip(&mut self) {
        let save = self.scanner_status;
        self.scanner_status = ScannerStatus::Skipping;
        self.skip_case_skip_inner();
        self.scanner_status = save;
    }

    fn skip_case_skip_inner(&mut self) {
        let mut depth = 0i32;
        loop {
            let t = self.raw_token();
            if t == EOF_MARKER {
                return;
            }
            if !t.is_cs() {
                continue;
            }
            match self.eqtb.resolve(t.cs_id()) {
                Some(Equiv::Prim(p)) => {
                    if Self::is_if_test(*p) {
                        depth += 1;
                    } else if *p == Prim::Unless {
                        self.skip_count_unless_target(&mut depth);
                    } else if *p == Prim::Fi {
                        if depth == 0 {
                            self.if_stack.pop();
                            return;
                        }
                        depth -= 1;
                    }
                }
                _ => {}
            }
        }
    }

    fn freeze_gts_in_edef(&self, id: CsId) -> bool {
        self.in_expanded_scan
            && self.csname_depth == 0
            && matches!(
                self.cs.name(id),
                b"GTS@RemoveLeft"
                    | b"GTS@TestLeftEnd"
                    | b"GTS@TestLeft"
                    | b"GetTitleStringNonExpand"
                    | b"GetTitleString"
            )
    }

    fn is_self_quark(&self, id: CsId, m: &Macro) -> bool {
        if m.num_params != 0 || m.body.len() != 1 {
            return false;
        }
        let b = m.body[0];
        if b.is_cs() {
            return (b.0 & 0x3FFF_FFFF) == (id & 0x3FFF_FFFF);
        }
        // An active character is interned under its collision-proof scalar
        // identity, not under the source spelling of the control symbol.
        b.is_char()
            && b.cc() == CAT_ACTIVE
            && Self::active_cs_scalar(self.cs.name(id)) == Some(b.chr())
    }
    pub fn expand_macro(&mut self, id: CsId, m: &Macro, invocation: CsId) {
        if self.is_self_quark(id, m) {
            return;
        }
        if m.body.is_empty()
            && (self.cs.name(id) == b"f@encoding" || self.cs.name(id) == b"cf@encoding")
        {
            let ot1_body = vec![
                Token::char(12, b'O' as u32),
                Token::char(12, b'T' as u32),
                Token::char(12, b'1' as u32),
            ];
            self.push_tokens(ot1_body);
            return;
        }

        // Numeric scanners and expandafter also enter here. Parameterless
        // macros need no argument buffers or substituted replacement list.
        if m.num_params == 0 && m.prefix.is_empty() {
            self.current_macro = id;
            self.push_tokens_rc(std::rc::Rc::clone(&m.body), id);
            return;
        }

        self.current_macro = id;
        self.enter_macro_diagnostic(id, invocation);
        let call_site = self.diagnostic_macro_call_site.clone();
        self.expand_macro_with_args(id, m, call_site.as_ref());
    }
    fn expand_macro_with_args(
        &mut self,
        id: CsId,
        m: &Macro,
        origin: Option<&crate::input::SourceMark>,
    ) {
        if !m.prefix.is_empty() {
            // tex.web: tokens before the first # must match the next
            // input tokens exactly. scan_delimited would skip ahead and
            // steal a later \fi: (expl3 \__tl_if_head_is_group_fi_false:w).
            for p in &m.prefix {
                let raw = self.macro_arg_token();
                let stored = self.unfreeze_input_token(raw);
                let t = self.unfreeze_unexpanded_token(stored);
                if t == EOF_MARKER {
                    self.fatal_error_at(
                        "File ended while matching macro prefix",
                        origin.map(crate::input::SourceMark::to_context),
                    );
                    return;
                }
                if !self.delim_eq(t, *p) {
                    self.push_token(stored);
                    self.error_at(
                        &format!(
                            "Use of {} doesn't match its definition",
                            self.display_cs(id)
                        ),
                        origin.map(crate::input::SourceMark::to_context),
                    );
                    return;
                }
            }
        }
        // Selectors and discarders only retain one (or no) argument. The
        // other arguments still undergo ordinary TeX scanning and validation.
        let selector = if m.body.is_empty() {
            Some(0)
        } else if m.has_param_refs
            && m.body.len() == 1
            && (PAR_REF_FLAG + 1..=PAR_REF_FLAG + u32::from(m.num_params)).contains(&m.body[0].0)
            && (m.body[0].0 & 0x3FFF_FFFF) as usize <= m.params.len()
        {
            Some((m.body[0].0 & 0x3FFF_FFFF) as usize)
        } else {
            None
        };
        let mut selected = smallvec::SmallVec::<[Token; 16]>::new();
        let mut args: smallvec::SmallVec<[smallvec::SmallVec<[Token; 16]>; 9]> =
            smallvec::SmallVec::new();
        for (i, delim) in m.params.iter().enumerate() {
            if i as u32 + 1 > m.num_params as u32 {
                break;
            }
            let keep = selector.map_or(true, |selected| selected == i + 1);
            let mut arg = smallvec::SmallVec::new();
            if delim.is_empty() {
                let saved_align_macro_arg = self.align_macro_arg;
                self.align_macro_arg = true;
                self.skip_raw_spaces();
                self.align_macro_arg = saved_align_macro_arg;
                let raw = self.macro_arg_token();
                let stored = self.unfreeze_input_token(raw);
                let t = self.unfreeze_unexpanded_token(stored);
                if self.is_partoken(t) && !m.long {
                    self.error_at(
                        &format!(
                            "Paragraph ended before {} was complete",
                            self.display_cs(id)
                        ),
                        origin.map(crate::input::SourceMark::to_context),
                    );
                    return;
                }
                if t.is_char() && t.cc() == 2 {
                    self.push_token(stored);
                    self.error_at(
                        &format!("Argument of {} has an extra }}", self.display_cs(id)),
                        origin.map(crate::input::SourceMark::to_context),
                    );
                } else if t.is_char() && t.cc() == 1 {
                    arg = self.scan_macro_balanced_arg(m.long, keep, origin);
                } else {
                    if keep {
                        arg.push(stored);
                    }
                }
            } else {
                arg = smallvec::SmallVec::from_vec(self.scan_delimited(delim, m.long, origin));
            }
            if selector.is_none() {
                args.push(arg);
            } else if keep {
                selected = arg;
            }
        }
        if let Some(index) = selector {
            // Invalid hand-built parameter references follow the generic
            // replacement path; valid TeX definitions have 1..=num_params.
            if index <= m.num_params as usize {
                match selected.len() {
                    0 => {}
                    1 => self.push_token(selected[0]),
                    _ if selected.spilled() => self.push_macro_tokens(selected.into_vec(), id),
                    _ => {
                        let mut body = self.token_vec_pool.pop().unwrap_or_default();
                        body.clear();
                        body.extend_from_slice(&selected);
                        self.push_macro_tokens(body, id);
                    }
                }
                return;
            }
        }
        if m.has_param_refs {
            // A selector/identity macro can hand an already allocated
            // argument to the input stack instead of copying it again.
            if let [token] = &m.body[..] {
                if (PAR_REF_FLAG..0x8000_0000).contains(&token.0) {
                    let index = (token.0 & 0x3FFF_FFFF).wrapping_sub(1) as usize;
                    if let Some(arg) = args.get_mut(index) {
                        if arg.spilled() {
                            let body = std::mem::take(arg).into_vec();
                            self.push_macro_tokens(body, id);
                            return;
                        }
                    }
                }
            }
            let Some(length) = m.replacement_length(&args, crate::input::MAX_TOKEN_LIST_TOKENS)
            else {
                self.fatal_error_at(
                    &format!(
                        "TeX capacity exceeded, sorry [macro expansion size={}]",
                        crate::input::MAX_TOKEN_LIST_TOKENS
                    ),
                    origin.map(crate::input::SourceMark::to_context),
                );
                return;
            };
            if length == 0 {
                return;
            }
            let references = m.ensure_replacement_plan();
            self.try_push_macro_frame(crate::input::MacroFrame {
                body: std::rc::Rc::clone(&m.body),
                args,
                references,
                ref_idx: 0,
                body_pos: 0,
                arg_pos: 0,
                name: "<macro>",
                owner: Some(id),
                trace_depth: self.diagnostic_macro_trace.len().min(255) as u8,
                delivered_brace_balance: 0,
            });
        } else {
            self.push_tokens_rc(std::rc::Rc::clone(&m.body), id);
        }
    }
    pub fn skip_raw_spaces(&mut self) {
        if self.pushed.is_empty() {
            if let Some(crate::input::Source::TokList { toks, pos, .. }) =
                self.input.stack.last_mut()
            {
                let s = &toks[..];
                while *pos < s.len() && (s[*pos].0 >> 24) == 10 {
                    *pos += 1;
                }
                if *pos == s.len() {
                    if let Some(crate::input::Source::TokList {
                        toks: crate::input::TokTokens::Vec(mut v),
                        ..
                    }) = self.input.stack.pop()
                    {
                        v.clear();
                        self.token_vec_pool.push(v);
                    }
                } else {
                    return;
                }
            }
        }
        loop {
            let t = self.raw_token();
            if (t.0 >> 24) == 10 {
                continue;
            }
            self.push_token(t);
            return;
        }
    }

    /// Scan a balanced group (the opening brace was already consumed).
    /// Returns tokens without the outer braces.
    pub fn scan_balanced_raw(&mut self, long: bool) -> smallvec::SmallVec<[Token; 16]> {
        let origin = self.current_token_source_mark();
        self.scan_balanced_raw_collect(long, true, origin.as_ref())
    }

    fn scan_macro_balanced_arg(
        &mut self,
        long: bool,
        collect: bool,
        origin: Option<&crate::input::SourceMark>,
    ) -> smallvec::SmallVec<[Token; 16]> {
        self.diagnostic_trace_hold = self.diagnostic_trace_hold.saturating_add(1);
        let result = self.scan_balanced_raw_collect(long, collect, origin);
        self.diagnostic_trace_hold -= 1;
        result
    }

    fn scan_balanced_raw_collect(
        &mut self,
        long: bool,
        collect: bool,
        origin: Option<&crate::input::SourceMark>,
    ) -> smallvec::SmallVec<[Token; 16]> {
        let partoken_id = self.partoken_id();
        if self.pushed.is_empty() {
            if let Some(crate::input::Source::TokList { toks, pos, .. }) =
                self.input.stack.last_mut()
            {
                let s = &toks[..];
                {
                    let start = *pos;
                    if let Some(end) = balanced_end(&s[start..], long, partoken_id) {
                        let p = start + end;
                        *pos = p;
                        // The opening brace of this balanced group was fetched by raw_token()
                        // and incremented align_brace_depth; the fast path consumes the matching
                        // closing brace directly from the slice, so decrement align_brace_depth.
                        self.align_brace_depth = self.align_brace_depth.saturating_sub(1);
                        if !collect {
                            return smallvec::SmallVec::new();
                        }
                        let slice = &s[start..p - 1];
                        return smallvec::SmallVec::from_slice(slice);
                    }
                }
            }
        }

        let mut depth = 1i32;
        let mut out = smallvec::SmallVec::<[Token; 16]>::new();
        let mut scanned = 0;
        loop {
            let raw = self.raw_token();
            let stored = self.unfreeze_input_token(raw);
            let t = self.unfreeze_unexpanded_token(stored);
            if t == EOF_MARKER {
                self.fatal_error_at(
                    "Runaway argument / missing }",
                    origin.map(crate::input::SourceMark::to_context),
                );
                return out;
            }
            if self.is_partoken(t) && !long {
                self.error_at(
                    "Runaway argument / missing }",
                    origin.map(crate::input::SourceMark::to_context),
                );
                return out;
            }
            if t.is_char() {
                let cc = t.cc();
                if cc == 1 {
                    depth += 1;
                } else if cc == 2 {
                    depth -= 1;
                    if depth == 0 {
                        return out;
                    }
                }
            }
            // Check after recognizing the outer closing brace: a list of
            // exactly MAX_TOKEN_LIST_TOKENS tokens remains legal, while a
            // further content token never enters the accumulator.
            if scanned == crate::input::MAX_TOKEN_LIST_TOKENS {
                self.fatal_error_at(
                    &format!(
                        "TeX capacity exceeded, sorry [balanced text size={}]",
                        crate::input::MAX_TOKEN_LIST_TOKENS
                    ),
                    origin.map(crate::input::SourceMark::to_context),
                );
                return out;
            }
            scanned += 1;
            if collect {
                out.push(stored);
            }
        }
    }

    /// tex.web scan_toks delimited search: expand one non-matching
    /// unprotected macro / expandable primitive in place. Returns true
    /// when the token was expanded (loop must continue); false when the
    /// token should be appended to the argument verbatim.
    fn expand_if_expansive(&mut self, t: Token) -> bool {
        if !t.is_cs() {
            return false;
        }
        let mut id = t.cs_id();
        for _ in 0..64 {
            match self.eqtb.get(id) {
                Some(Equiv::Alias(next)) => id = *next,
                _ => break,
            }
        }
        match self.eqtb.get(id).cloned() {
            Some(Equiv::Macro(m)) => {
                if m.protected || self.freeze_gts_in_edef(id) {
                    self.set_cur_cs(t);
                    return false;
                }
                self.expand_macro(id, &m, t.cs_id());
                true
            }
            Some(Equiv::Prim(p)) => {
                if !self.is_expandable(p) {
                    self.set_cur_cs(t);
                    return false;
                }
                match self.expand_prim(p, id) {
                    Some(tok) => {
                        if tok.0 >= NOEXP_FLAG && tok.0 < UNEXPANDED_CS_FLAG {
                            self.push_token(Token::from_cs(tok.0 & 0x3FFF_FFFF));
                        } else {
                            self.push_token(tok);
                        }
                    }
                    None => {}
                }
                true
            }
            _ => {
                self.set_cur_cs(t);
                false
            }
        }
    }

    fn scan_delimited(
        &mut self,
        delim: &[Token],
        long: bool,
        origin: Option<&crate::input::SourceMark>,
    ) -> Vec<Token> {
        let mut arg: Vec<Token> = Vec::with_capacity(8);
        let mut matched = smallvec::SmallVec::<[Token; 8]>::new();
        loop {
            let raw = self.macro_arg_token();
            let stored = self.unfreeze_input_token(raw);
            let raw = self.unfreeze_unexpanded_token(stored);
            if self.align_state & crate::align::PH_CLOSE != 0 && raw == self.crcr_token() {
                self.error(&format!(
                    "Forbidden control sequence found while scanning use of {}",
                    self.display_cs(self.current_macro)
                ));
                self.push_token(raw);
                return arg;
            }
            if raw == EOF_MARKER {
                self.fatal_error_at(
                    &format!(
                        "Runaway argument of {} (delim={})",
                        self.display_cs(self.current_macro),
                        self.diagnostic_tokens_to_string(delim, 512)
                    ),
                    origin.map(crate::input::SourceMark::to_context),
                );
                return arg;
            }
            // The file scanner uses an internal sentinel for a blank line.
            // Macro parameter matching sees the real \par token.
            let t = if raw == PAR_END {
                Token::from_cs(self.partoken_id())
            } else {
                raw
            };
            let stored = if raw == PAR_END { t } else { stored };
            matched.push(stored);
            while !matched
                .iter()
                .enumerate()
                .all(|(i, token)| self.delim_eq(token.unfreeze(), delim[i]))
            {
                let rm = matched.remove(0);
                if !self.scanned_token_list_has_room(arg.len(), 1, "macro parameter size", origin) {
                    return arg;
                }
                arg.push(rm);
            }
            if matched.len() == delim.len() {
                if let Some(last) = delim.last() {
                    if last.is_char() && last.cc() == 1 {
                        // tex.web §392 / @8063: When the parameter delimiter ends
                        // with `#{`, both the delimiter match and the subsequent
                        // macro body scan see a left brace. Only one should affect
                        // align_state, so TeX decrements align_state here.
                        self.align_brace_depth = self.align_brace_depth.saturating_sub(1);
                    }
                }
                Self::strip_outer_braces(&mut arg);
                return arg;
            }

            // tex.web §392 matches the delimiter before §396 rejects an
            // illegal paragraph. A non-long #1\par parameter may therefore
            // use the paragraph token as its terminator.
            if self.is_partoken(t) && !long {
                self.error_at(
                    &format!(
                        "Paragraph ended before {} was complete (delim={}) collected={}",
                        self.display_cs(self.current_macro),
                        self.diagnostic_tokens_to_string(delim, 512),
                        self.diagnostic_tokens_to_string(&arg, 1024)
                    ),
                    origin.map(crate::input::SourceMark::to_context),
                );
                return arg;
            }

            if t.is_char() && t.cc() == 1 {
                // Delimiters cannot contain an unmatched opening brace other
                // than the single-token #{ case, which returned above.
                let inner = self.scan_macro_balanced_arg(long, true, origin);
                if self.stopped_on_error {
                    return arg;
                }
                if !self.scanned_token_list_has_room(
                    arg.len() + matched.len(),
                    inner.len().saturating_add(1),
                    "macro parameter size",
                    origin,
                ) {
                    return arg;
                }
                arg.extend(matched.drain(..));
                arg.extend(inner);
                arg.push(Token::char(2, b'}' as u32));
            }
        }
    }
    fn strip_outer_braces(arg: &mut Vec<Token>) {
        if arg.len() >= 2 && arg[0].is_char() && arg[0].cc() == 1 {
            let mut depth = 0i32;
            let mut ok = true;
            for (i, x) in arg.iter().enumerate() {
                if x.is_char() && x.cc() == 1 {
                    depth += 1;
                } else if x.is_char() && x.cc() == 2 {
                    depth -= 1;
                    if depth == 0 && i != arg.len() - 1 {
                        ok = false;
                        break;
                    }
                }
            }
            if ok && depth == 0 {
                arg.remove(arg.len() - 1);
                arg.remove(0);
            }
        }
    }

    // ---------- helpers ----------

    /// put a byte string into the expansion stream; spaces become cat-10
    /// spacer tokens (tex.web str_toks), everything else cat-12
    /// (chronologically on top: newer than any earlier pushback)
    pub fn exp_string(&mut self, bytes: &[u8]) {
        let toks: Vec<Token> = bytes
            .iter()
            .map(|&b| {
                if b == b' ' {
                    Token::space()
                } else {
                    Token::other(b)
                }
            })
            .collect();
        self.push_tokens(toks);
    }

    pub fn tokens_to_string(&self, toks: &[Token]) -> String {
        String::from_utf8_lossy(&self.tokens_to_bytes(toks)).into_owned()
    }

    pub(crate) fn tokens_to_bytes(&self, toks: &[Token]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let esc = self.eqtb.int_params[crate::prim::IntParam::EscapeChar.idx() as usize];
        for t in toks {
            if t.0 >= PAR_REF_FLAG && t.0 < 0xFFFF_0000 && !t.is_cs() {
                out.push(b'#');
                out.push(b'0' + (t.0 & 0xF) as u8);
                continue;
            }
            if t.is_char() && t.cc() == 6 && t.chr() == 0x23 {
                // tex.web show_token_list: a mac_param token prints as ##.
                out.push(b'#');
                out.push(b'#');
                continue;
            }
            if t.is_cs() {
                let name = self.cs.name(t.cs_id());
                if let Some((bytes, len)) = Self::active_cs_source_bytes(name) {
                    out.extend_from_slice(&bytes[..len]);
                    continue;
                }
                if esc >= 0 && esc <= 255 {
                    out.push(esc as u8);
                }
                out.extend_from_slice(name);
                // tex.web print_cs / show_token_list (§5605): control word
                // (name length > 1 or single character with letter catcode)
                // is followed by a space.
                if name.len() > 1
                    || name
                        .first()
                        .is_some_and(|&c| self.eqtb.cat[c as usize] == crate::token::CAT_LETTER)
                {
                    out.push(b' ');
                }
            } else {
                t.append_character_bytes(&mut out);
            }
        }
        out
    }

    pub fn ifx_equal(&self, a: Token, b: Token) -> bool {
        self.ifx_equal_inner(a, b)
    }
    fn ifx_equal_inner(&self, mut a: Token, mut b: Token) -> bool {
        if a.is_char() && a.cc() == 13 {
            if let Some(id) = self.active_cs_lookup(a.chr()) {
                a = Token::from_cs(id);
            }
        }
        if b.is_char() && b.cc() == 13 {
            if let Some(id) = self.active_cs_lookup(b.chr()) {
                b = Token::from_cs(id);
            }
        }
        let a_fr = a.0 >= NOEXP_FLAG && a.0 < 0xFFFF_0000;
        let b_fr = b.0 >= NOEXP_FLAG && b.0 < 0xFFFF_0000;
        if a_fr || b_fr {
            if a_fr && b_fr {
                return true;
            }
            let other = if a_fr { b } else { a };
            if !other.is_cs() {
                return false;
            }
            return matches!(
                self.eqtb.resolve(other.cs_id()),
                Some(Equiv::Prim(Prim::Relax))
            );
        }
        if a == b {
            return true;
        }
        if !a.is_cs() && !b.is_cs() {
            return a == b;
        }
        if a.is_cs() && !b.is_cs() {
            if let Some(Equiv::CharTok(v)) = self.eqtb.resolve(a.cs_id()) {
                return Token(*v) == b;
            }
            return false;
        }
        if !a.is_cs() && b.is_cs() {
            if let Some(Equiv::CharTok(w)) = self.eqtb.resolve(b.cs_id()) {
                return a == Token(*w);
            }
            return false;
        }
        let ma = self.eqtb.resolve(a.cs_id()).cloned();
        let mb = self.eqtb.resolve(b.cs_id()).cloned();
        match (ma, mb) {
            (None, None) => true,
            (Some(Equiv::Macro(x)), Some(Equiv::Macro(y))) => {
                x.num_params == y.num_params
                    && x.params == y.params
                    && x.body == y.body
                    && x.long == y.long
                    && x.outer == y.outer
                    && x.protected == y.protected
            }
            (Some(Equiv::Prim(x)), Some(Equiv::Prim(y))) => x == y,
            (Some(Equiv::CharTok(v)), Some(Equiv::CharTok(w))) => v == w,
            (Some(x), Some(y)) => match (&x, &y) {
                (Equiv::CharDef(v1), Equiv::CharDef(v2)) => v1 == v2,
                (Equiv::MathCharDef(v1), Equiv::MathCharDef(v2)) => v1 == v2,
                (Equiv::FontRef(v1), Equiv::FontRef(v2)) => v1 == v2,
                (Equiv::CountReg(v1), Equiv::CountReg(v2)) => v1 == v2,
                (Equiv::DimenReg(v1), Equiv::DimenReg(v2)) => v1 == v2,
                (Equiv::SkipReg(v1), Equiv::SkipReg(v2)) => v1 == v2,
                (Equiv::ToksReg(v1), Equiv::ToksReg(v2)) => v1 == v2,
                (Equiv::BoxReg(v1), Equiv::BoxReg(v2)) => v1 == v2,
                (Equiv::MuSkipReg(v1), Equiv::MuSkipReg(v2)) => v1 == v2,
                _ => false,
            },
            _ => false,
        }
    }

    /// tex.web print_nl: begin a fresh terminal line before `s` unless the
    /// terminal is already at the start of a line. Without the reset, error
    /// text glues to unterminated "(file" output and log comparison breaks.
    pub fn term_print_nl(&mut self, s: &str) {
        if !self.term.is_empty() && !self.term.ends_with('\n') {
            self.append_term("\n");
        }
        self.append_term(s);
        if !self.log.is_empty() && !self.log.ends_with('\n') {
            self.append_log("\n");
        }
        self.append_log(s);
    }
}

fn posix_regex_error_detail(error: &posix_regex::compile::Error) -> String {
    use posix_regex::compile::Error;
    match error {
        Error::EOF => "unexpected end of pattern".to_string(),
        Error::EmptyRepetition => "repetition range is empty".to_string(),
        Error::Expected(expected, found) => match found {
            Some(found) => format!(
                "expected {}, found {}",
                regex_byte(*expected),
                regex_byte(*found)
            ),
            None => format!("expected {}, found end of pattern", regex_byte(*expected)),
        },
        Error::IllegalRange => "character range has its endpoints in the wrong order".to_string(),
        Error::IntegerOverflow => "repetition count is too large".to_string(),
        Error::InvalidBackRef(reference) => {
            format!("back-reference \\{reference} does not name an earlier capture")
        }
        Error::LeadingRepetition => "a repetition operator has no preceding expression".to_string(),
        Error::UnclosedRepetition => "repetition range is missing its closing `}`".to_string(),
        Error::UnexpectedToken(token) => format!("unexpected token {}", regex_byte(*token)),
        Error::UnknownClass(class) => format!(
            "unknown character class `[:{}:]`",
            String::from_utf8_lossy(class)
        ),
        Error::UnknownCollation => "unknown collating element".to_string(),
    }
}

fn regex_byte(byte: u8) -> String {
    if byte.is_ascii_graphic() || byte == b' ' {
        format!("`{}`", char::from(byte).escape_default())
    } else {
        format!("byte 0x{byte:02X}")
    }
}

fn roman(mut n: i32) -> String {
    let table: &[(&str, i32)] = &[
        ("m", 1000),
        ("cm", 900),
        ("d", 500),
        ("cd", 400),
        ("c", 100),
        ("xc", 90),
        ("l", 50),
        ("xl", 40),
        ("x", 10),
        ("ix", 9),
        ("v", 5),
        ("iv", 4),
        ("i", 1),
    ];
    let mut out = String::new();
    for (s, v) in table {
        while n >= *v {
            out.push_str(s);
            n -= v;
        }
    }
    out
}

fn compare(a: i32, rel: u8, b: i32) -> bool {
    match rel {
        b'<' => a < b,
        b'=' => a == b,
        _ => a > b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_letter_control_sequence_followed_by_space_in_tokens_to_string() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        let d = engine.cs.intern(b"D");
        let toks = vec![Token::from_cs(d), Token::letter(b'y')];
        let s = engine.tokens_to_string(&toks);
        assert_eq!(s, "\\D y");
    }
}
