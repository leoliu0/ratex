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
/// Capacity reserved for the argument buffer of a macro call.
const MIN_ARG_BUFFER: usize = 32;

/// True when a token list token may be moved into balanced text as is: it
/// is a brace, an ordinary character or a plain control sequence. Every
/// other token (expansion guards, parameter references, sentinels, ignored
/// characters and, in a short argument, \par) needs `raw_token`'s handling.
#[inline(always)]
fn plain_balanced_token(t: Token, long: bool, partoken: Token) -> bool {
    let top = t.0 >> 24;
    if top < 0x80 {
        top <= 13 && top != 9
    } else {
        top < 0xC0 && (long || t != partoken)
    }
}

fn balanced_prefix_scalar(
    tokens: &[Token],
    long: bool,
    partoken: Token,
    mut depth: i32,
) -> (usize, i32) {
    for (index, &t) in tokens.iter().enumerate() {
        if !plain_balanced_token(t, long, partoken) {
            return (index, depth);
        }
        match t.0 >> 24 {
            1 => depth += 1,
            2 => {
                depth -= 1;
                if depth == 0 {
                    return (index + 1, 0);
                }
            }
            _ => {}
        }
    }
    (tokens.len(), depth)
}

/// The longest prefix of `tokens` that balanced text at brace depth `depth`
/// (> 0) can take without per-token handling, and the depth after it. A
/// depth of zero means the prefix ends with the closing brace.
fn balanced_prefix(tokens: &[Token], long: bool, partoken: CsId, depth: i32) -> (usize, i32) {
    let partoken = Token::from_cs(partoken);
    #[cfg(target_arch = "x86_64")]
    if tokens.len() >= 16 && std::is_x86_feature_detected!("avx2") {
        // SAFETY: the processor supports AVX2; the callee bounds every load.
        return unsafe { balanced_prefix_avx2(tokens, long, partoken, depth) };
    }
    balanced_prefix_scalar(tokens, long, partoken, depth)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn balanced_prefix_avx2(
    tokens: &[Token],
    long: bool,
    partoken: Token,
    mut depth: i32,
) -> (usize, i32) {
    use std::arch::x86_64::*;
    let mut position = 0;
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
        // The negation of `plain_balanced_token`, on the top byte.
        let ignored = _mm256_cmpeq_epi32(top, _mm256_set1_epi32(9));
        let flagged_char = _mm256_and_si256(
            _mm256_cmpgt_epi32(top, _mm256_set1_epi32(13)),
            _mm256_cmpgt_epi32(_mm256_set1_epi32(0x80), top),
        );
        let marked = _mm256_cmpgt_epi32(top, _mm256_set1_epi32(0xBF));
        let mut guards = _mm256_or_si256(_mm256_or_si256(ignored, flagged_char), marked);
        if !long {
            guards = _mm256_or_si256(
                guards,
                _mm256_cmpeq_epi32(values, _mm256_set1_epi32(partoken.0 as i32)),
            );
        }
        let guards = _mm256_movemask_ps(_mm256_castsi256_ps(guards)) as u32;
        if ends | guards != 0 {
            break;
        }
        depth += _mm256_extract_epi32::<7>(prefix);
        position += 8;
    }
    let (length, depth) = balanced_prefix_scalar(&tokens[position..], long, partoken, depth);
    (position + length, depth)
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
            Token(PAR_REF_FLAG | 1),
            Token::char(9, 0),
            Token::char(14, 37),
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
                    for depth in [1, 3] {
                        let slice = &tokens[offset..];
                        let expected =
                            balanced_prefix_scalar(slice, long, Token::from_cs(7), depth);
                        assert_eq!(
                            balanced_prefix(slice, long, 7, depth),
                            expected,
                            "length={length} offset={offset} long={long} depth={depth}"
                        );
                    }
                }
            }
        }
        // Prefix sums must carry nesting across the 128-bit lane boundary.
        let mut deep = vec![Token::char(1, 123); 64];
        deep.extend(vec![Token::char(2, 125); 65]);
        deep.push(Token(NOEXP_FLAG | 42));
        assert_eq!(balanced_prefix(&deep, true, 7, 1), (129, 0));
        deep[127] = Token(NOEXP_FLAG | 42);
        assert_eq!(balanced_prefix(&deep, true, 7, 1), (127, 2));
        // Only a short argument stops at \par.
        let par = [Token::letter(b'a'), Token::from_cs(7), Token::char(2, 125)];
        assert_eq!(balanced_prefix(&par, false, 7, 1), (1, 1));
        assert_eq!(balanced_prefix(&par, true, 7, 1), (3, 0));
    }
}

/// Which plain tokens (see `Engine::is_plain_raw_token`) `raw_token`'s fast
/// path must leave to `raw_token_general` because of a running alignment.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AlignFilter {
    /// No alignment delimiter can be intercepted.
    None,
    /// A row delimiter would end the current cell: only tokens that are
    /// no row delimiter pass.
    Delimiters,
    /// Every token needs the general path.
    All,
}

impl AlignFilter {
    #[inline(always)]
    fn passes(self, t: Token, eqtb: &crate::eqtb::Eqtb) -> bool {
        Engine::is_plain_raw_token(t)
            && match self {
                AlignFilter::None => true,
                AlignFilter::Delimiters => Self::no_row_delimiter(t, eqtb),
                AlignFilter::All => false,
            }
    }

    #[inline(always)]
    fn no_row_delimiter(t: Token, eqtb: &crate::eqtb::Eqtb) -> bool {
        crate::align::row_delimiter(t, eqtb).is_none()
    }
}

/// A macro call ended before its arguments were complete (tex.web §396 and
/// §398 abort the call); diagnostics have already been issued.
struct ArgAbort;

/// Why balanced text ended without its closing brace.
enum Unbalanced {
    /// A forbidden paragraph token, not yet consumed.
    Paragraph(Token),
    /// An \outer macro inside a macro argument, not yet consumed.
    Outer(Token),
    /// A fatal error has already been reported.
    Fatal,
    /// The input ended inside a macro argument.
    Eof,
}

/// The absorbing scans of tex.web §338-§339 (scanner_status defining,
/// absorbing, aligning): what an \outer control sequence interrupts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum OuterScan {
    /// A macro definition body (\def, \edef, ...).
    Definition,
    /// General text (\message, \toks, \write, \expanded, ...).
    Text,
    /// An alignment preamble.
    Preamble,
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
    ///
    /// Out of line so that its many callers share one copy. The common case
    /// (no alignment entry being scanned) is served by `raw_token_fast`,
    /// which never calls another function and therefore runs without
    /// setting up a stack frame; inside an alignment entry the next stop is
    /// `raw_token_aligning`, and everything else is a tail call to
    /// `raw_token_general`.
    #[inline(never)]
    pub fn raw_token(&mut self) -> Token {
        if !self.diagnostic_sources_live {
            if self.align_state != crate::align::PH_IDLE {
                return self.raw_token_aligning();
            }
            if let Some(t) = self.raw_token_fast(AlignFilter::None) {
                return t;
            }
        }
        self.raw_token_general()
    }

    /// Inside an alignment entry, tokens that cannot end the cell take the
    /// fast path too, without the frame of `raw_token_general`.
    #[inline(never)]
    fn raw_token_aligning(&mut self) -> Token {
        let filter = self.align_raw_filter();
        if filter != AlignFilter::All {
            if let Some(t) = self.raw_token_fast(filter) {
                return t;
            }
        }
        self.raw_token_general()
    }

    /// The next token when it is a pushed-back token or a token of a token
    /// list or macro replacement that `raw_token` returns exactly as stored
    /// and that `filter` lets through, and no Lua callback needs
    /// bookkeeping. Otherwise nothing is consumed and `raw_token_general`
    /// has to fetch it. The caller checks that no diagnostic source
    /// position is live.
    #[inline(always)]
    fn raw_token_fast(&mut self, filter: AlignFilter) -> Option<Token> {
        let token = if let Some(&t) = self.pushed.last() {
            // While an alignment is scanned, pushback below
            // `align_pushed_base` predates the template and waits (see
            // `raw_token_general`); pushback above it is read as usual.
            if !filter.passes(t, &self.eqtb)
                || (self.scanner_status == ScannerStatus::Aligning
                    && self.pushed.len() <= self.align_pushed_base)
                || self.lua_cb[crate::lua_callbacks::Cb::ShowErrorHook as usize] > 0
            {
                return None;
            }
            self.pushed.pop();
            self.pushed_read = t;
            t
        } else {
            let eqtb = &self.eqtb;
            let (t, depth) = match self.input.stack.last_mut() {
                Some(crate::input::Source::TokList {
                    toks,
                    pos,
                    trace_depth,
                    ..
                }) => match toks.get(*pos) {
                    Some(&t) if filter.passes(t, eqtb) => {
                        *pos += 1;
                        (t, *trace_depth)
                    }
                    _ => return None,
                },
                Some(crate::input::Source::MacroFrame(frame)) => match frame.peek_token_advancing() {
                    Some(t) if filter.passes(t, eqtb) => {
                        frame.skip(1);
                        (t, frame.trace_depth)
                    }
                    _ => return None,
                },
                _ => return None,
            };
            self.unwind_macro_trace(depth);
            t
        };
        match token.0 >> 24 {
            1 => self.align_brace_depth = self.align_brace_depth.saturating_add(1),
            2 => self.align_brace_depth = self.align_brace_depth.saturating_sub(1),
            _ => {}
        }
        Some(token)
    }

    /// Inside an alignment: which plain tokens `raw_token_general` would
    /// return as stored, that is, which cannot end the current cell (see
    /// `align_intercept_raw_token`) and are not the frozen end of a cell
    /// met by a macro argument scan.
    fn align_raw_filter(&self) -> AlignFilter {
        if self.align_macro_arg && self.align_state & crate::align::PH_CLOSE != 0 {
            AlignFilter::All
        } else if self.align_delimiter_live() {
            AlignFilter::Delimiters
        } else {
            AlignFilter::None
        }
    }

    /// True when a row delimiter fetched now would end the current cell
    /// (tex.web §342: the v template is inserted only when a `&` or `\cr`
    /// arrives at align_state = 0 while the entry is being scanned).
    #[inline]
    pub(crate) fn align_delimiter_live(&self) -> bool {
        self.align_state != crate::align::PH_IDLE
            && !self.in_expanded_scan
            && self.scanner_status == ScannerStatus::Aligning
            && self.align_phase() == crate::align::PH_CONTENT
            && self.align_state & crate::align::PH_CLOSE == 0
            && !self.align_delimiter_hidden()
    }

    /// Tokens `raw_token` returns exactly as they are stored: control
    /// sequences and sentinels other than `\par` markers, and characters
    /// that are neither ignored, comments, invalid nor parameter references.
    #[inline(always)]
    fn is_plain_raw_token(t: Token) -> bool {
        if t.0 >= 0x8000_0000 {
            t != PAR_END
        } else {
            t.0 < PAR_REF_FLAG && !matches!(t.0 >> 24, 9 | 14 | 15)
        }
    }

    #[inline(never)]
    fn raw_token_general(&mut self) -> Token {
        // tex.web @7335/@7492: a brace fetched from a real input source
        // adjusts the alignment brace depth. Tokens returned from the
        // pushback stack were counted at their original fetch (tex.web
        // compensates at back_input, @7028; skipping the pushed path is the
        // equivalent here). While an alignment is scanned, pushback below
        // `align_pushed_base` predates the template and waits.
        let t = 'fetch: {
            if !self.pushed.is_empty()
                && (self.scanner_status != ScannerStatus::Aligning
                    || self.pushed.len() > self.align_pushed_base)
            {
                if let Some(t) = self.pushed.pop() {
                    self.pushed_read = t;
                    self.retain_diagnostic_sources_for(t);
                    if self.lua_cb[crate::lua_callbacks::Cb::ShowErrorHook as usize] > 0 {
                        self.recent_pushed = Some((t, self.input.signature()));
                    }
                    break 'fetch t;
                }
            }
            loop {
                match self.input.stack.last_mut() {
                    Some(crate::input::Source::TokList {
                        toks,
                        pos,
                        trace_depth,
                        ..
                    }) => {
                        let depth = *trace_depth;
                        if let Some(&tok) = toks.get(*pos) {
                            *pos += 1;
                            self.note_token_list_fetch(depth);
                            break 'fetch tok;
                        }
                        self.unwind_macro_trace(depth);
                    }
                    Some(crate::input::Source::MacroFrame(frame)) => {
                        let depth = frame.trace_depth;
                        if let Some(tok) = frame.next_token() {
                            self.note_token_list_fetch(depth);
                            break 'fetch tok;
                        }
                        self.unwind_macro_trace(depth);
                    }
                    _ => break 'fetch self.get_next_raw(),
                }
                self.end_token_list();
            }
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
        if t.0 < 0x8000_0000 {
            match t.0 >> 24 {
                1 => self.align_brace_depth = self.align_brace_depth.saturating_add(1),
                2 => self.align_brace_depth = self.align_brace_depth.saturating_sub(1),
                // Ignored, comment and invalid characters never form tokens
                // in TeX; drop any that a token list was built with.
                9 | 14 | 15 => return self.raw_token(),
                _ => {}
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

    /// A token from a token list has no physical spelling of its own, and the
    /// macro trace returns to the list's ancestry. In the steady state (no
    /// recorded source, trace no deeper than the list) this only compares.
    #[inline(always)]
    fn note_token_list_fetch(&mut self, depth: u8) {
        if self.diagnostic_sources_live {
            self.clear_diagnostic_sources();
        }
        self.unwind_macro_trace(depth);
    }

    /// Drop macro-trace entries that do not belong to a token list with
    /// `depth` ancestry entries, unless a macro argument or a held
    /// definition is being scanned.
    #[inline(always)]
    pub(crate) fn unwind_macro_trace(&mut self, depth: u8) {
        if self.diagnostic_macro_trace.len() > usize::from(depth)
            && !self.align_macro_arg
            && self.diagnostic_trace_hold == 0
        {
            self.diagnostic_macro_trace.truncate(usize::from(depth));
        }
    }

    #[inline(never)]
    pub(crate) fn clear_diagnostic_sources(&mut self) {
        self.diagnostic_synthetic_source = None;
        self.diagnostic_physical_source = None;
        self.diagnostic_sources_live = false;
    }

    /// A pushed-back token keeps the source locations recorded for it, and
    /// only for it.
    #[inline]
    fn retain_diagnostic_sources_for(&mut self, t: Token) {
        if !self.diagnostic_sources_live {
            return;
        }
        if self
            .diagnostic_synthetic_source
            .as_ref()
            .is_some_and(|(token, _, _)| !t.is_cs() || *token != t.cs_id())
        {
            self.diagnostic_synthetic_source = None;
        }
        if self.diagnostic_physical_source.is_some_and(|source| {
            source.token != t && !(t.is_cs() && source.semantic_cs == Some(t.cs_id()))
        }) {
            self.diagnostic_physical_source = None;
        }
    }

    /// Pop the exhausted token list on top of the input stack (tex.web
    /// end_token_list), recycling its buffer.
    #[inline(never)]
    fn end_token_list(&mut self) {
        match self.input.stack.pop() {
            Some(crate::input::Source::TokList { toks, name, .. }) => {
                if name == crate::align::U_PART_SRC {
                    self.align_u_template_finished();
                }
                if let crate::input::TokTokens::Vec(v) = toks {
                    self.recycle_token_vec(v);
                }
            }
            Some(crate::input::Source::MacroFrame(frame)) => {
                self.recycle_token_vec(frame.into_arg_buffer());
            }
            _ => {}
        }
    }

    #[inline]
    pub(crate) fn recycle_token_vec(&mut self, mut v: Vec<Token>) {
        if self.token_vec_pool.len() < 512 && v.capacity() != 0 {
            v.clear();
            self.token_vec_pool.push(v);
        }
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
            if first.0 >= NOEXP_FLAG && first.0 < UNEXPANDED_CS_FLAG {
                // tex.web §358: a \noexpand-marked token means \relax.
                let tok = Token::from_cs(first.0 & 0x3FFF_FFFF);
                self.no_expand_tok = Some(tok);
                self.cur_tok = tok;
                self.cur_cs = Some(tok.cs_id());
                self.cur_prim = Some(Prim::Relax);
                return tok;
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
                    if m.outer
                        && self.outer_scan.is_some()
                        && self.eqtb.int_params[crate::prim::IntParam::SuppressOuterError.idx() as usize] == 0
                    {
                        return self.forbidden_outer(t);
                    }
                    // edef/write/expanded list. Nested \\romannumeral (f-expansion)
                    // clears in_expanded_scan and must expand \\exp_end_continue_f:w.
                    if m.protected && self.in_expanded_scan && self.csname_depth == 0 {
                        self.set_cur_cs_known(Token::from_cs(id), None);
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
                        self.set_cur_cs_known(t, None);
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
                        self.show_operand_ending_delimiter(p);
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
                                    // tex.web §358: the marked token means \relax.
                                    let tok = Token::from_cs(tok.0 & 0x3FFF_FFFF);
                                    self.set_cur_cs(tok);
                                    self.no_expand_tok = Some(tok);
                                    self.cur_prim = Some(Prim::Relax);
                                    return tok;
                                }
                                if !tok.is_cs() {
                                    if tok.is_char() {
                                        self.set_cur_char(tok);
                                    }
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
                    } else if p == Prim::PdfPrimitiveExec && !self.in_expanded_scan {
                        first = self.pdf_primitive_target();
                        continue;
                    } else {
                        self.set_cur_cs_known(t, Some(p));
                        return t;
                    }
                }
                Some(Equiv::LuaCall { slot, protected: false }) => {
                    self.call_lua_function(slot as i32);
                    first = self.raw_token();
                    continue;
                }
                None => {
                    self.undefined_cs_error(t);
                    first = self.raw_token();
                }
                _ => {
                    self.set_cur_cs_known(t, None);
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

    /// tex.web macro_call "conserve stack space": drop finished lists before
    /// a new one is pushed so tail calls do not deepen the input stack.
    fn pop_exhausted_token_lists(&mut self) {
        while let Some(source) = self.input.stack.last() {
            let exhausted = match source {
                crate::input::Source::TokList { toks, pos, .. } => *pos >= toks.len(),
                crate::input::Source::MacroFrame(frame) => frame.is_exhausted(),
                crate::input::Source::File { .. } => false,
            };
            if !exhausted {
                break;
            }
            self.end_token_list();
        }
    }

    /// Make room for one inserted input source with `len` tokens. Pending
    /// pushback is older than the new source, so it moves into its own list
    /// underneath (also used for Lua's pseudo-file output).
    pub(crate) fn prepare_token_list_push(&mut self, len: usize) -> bool {
        if !self.ensure_token_list_room(len) {
            return false;
        }
        self.pop_exhausted_token_lists();
        let cut = if self.scanner_status == ScannerStatus::Aligning {
            self.align_pushed_base.min(self.pushed.len())
        } else {
            0
        };
        let pending = self.pushed.len() - cut;
        if !self.ensure_token_list_room(pending)
            || !self.ensure_input_stack_room(1 + usize::from(pending > 0))
        {
            return false;
        }
        if pending > 0 {
            let mut rest = self.token_vec_pool.pop().unwrap_or_default();
            rest.extend(self.pushed[cut..].iter().rev());
            self.pushed.truncate(cut);
            self.input.push_toks(rest, "<pushback>");
        }
        true
    }

    pub fn push_tokens_rc(&mut self, toks: std::rc::Rc<[Token]>, owner: CsId) {
        self.try_push_tokens_rc(toks, owner);
    }

    fn try_push_tokens_rc(&mut self, toks: std::rc::Rc<[Token]>, owner: CsId) -> bool {
        if toks.is_empty() {
            return true;
        }
        if !self.prepare_token_list_push(toks.len()) {
            return false;
        }
        let depth = self.trace_depth();
        self.input.push_toks_owned(toks, "<macro>", Some(owner), depth);
        true
    }

    fn try_push_macro_frame(&mut self, frame: crate::input::MacroFrame) -> bool {
        if !self.prepare_token_list_push(0) {
            self.recycle_token_vec(frame.into_arg_buffer());
            return false;
        }
        self.input
            .stack
            .push(crate::input::Source::MacroFrame(frame));
        true
    }

    #[inline]
    fn trace_depth(&self) -> u8 {
        self.diagnostic_macro_trace.len().min(u8::MAX as usize) as u8
    }

    fn begin_token_list(
        &mut self,
        toks: Vec<Token>,
        exp_not: bool,
        name: &'static str,
        owner: Option<CsId>,
    ) -> bool {
        if toks.is_empty() {
            self.recycle_token_vec(toks);
            return true;
        }
        if self.engine_kind == crate::engine::EngineKind::XeTeX
            && self.eqtb.int_params[crate::prim::IntParam::TracingMacros.idx() as usize] > 1
        {
            self.trace_token_list(name, &toks);
        }
        if !self.prepare_token_list_push(toks.len()) {
            return false;
        }
        let toks = if exp_not {
            self.freeze_unexpanded_toks(toks)
        } else {
            toks
        };
        let depth = self.trace_depth();
        self.input.push_toks_owned(toks, name, owner, depth);
        true
    }
    /// tex.web §370: expanding an undefined control sequence is an error;
    /// TeX then forgets the token and reads on.
    #[cold]
    #[inline(never)]
    fn undefined_cs_error(&mut self, t: Token) {
        self.set_cur_cs(t);
        let message = format!("Undefined control sequence {}", self.display_cs(t.cs_id()));
        self.error(&message);
    }
    fn set_cur_cs(&mut self, t: Token) {
        let prim = match self.eqtb.resolve(t.cs_id()) {
            Some(Equiv::Prim(p)) => Some(*p),
            _ => None,
        };
        self.set_cur_cs_known(t, prim);
    }

    /// `set_cur_cs` for a token whose meaning was just resolved: `prim` is
    /// its primitive, if any.
    #[inline(always)]
    fn set_cur_cs_known(&mut self, t: Token, prim: Option<Prim>) {
        self.diagnostic_source_cs = Some(t.cs_id());
        self.cur_tok = t;
        self.cur_cs = Some(t.cs_id());
        self.cur_prim = prim;
    }

    fn set_cur_char(&mut self, t: Token) {
        self.diagnostic_source_cs = None;
        self.cur_tok = t;
        self.cur_cs = None;
        self.cur_prim = None;
    }

    /// pdftex.web `prim_eqtb`: the hidden control sequence that keeps the
    /// INITEX meaning of primitive `name`, created on first use.
    pub(crate) fn primitive_cs(&mut self, name: &[u8]) -> Option<CsId> {
        let p = *self.primitive_table.get(name)?;
        if let Some(id) = self.cs.frozen_lookup(name) {
            return Some(id);
        }
        let id = self.cs.push_frozen(name, true);
        self.eqtb.assign(id, Equiv::Prim(p), true);
        Some(id)
    }

    /// pdftex.web <Reset cur_tok for unexpandable primitives>: after the
    /// frozen `\pdfprimitive` marker, the next token stands for its
    /// primitive meaning (frozen `\relax` if its name is no primitive).
    pub(crate) fn pdf_primitive_target(&mut self) -> Token {
        let t = self.raw_token();
        let name = if t.is_cs() {
            self.cs.name(t.cs_id()).to_vec()
        } else {
            Vec::new()
        };
        let id = match self.primitive_cs(&name) {
            Some(id) => id,
            None => self.primitive_cs(b"relax").expect("relax is a primitive"),
        };
        Token::from_cs(id)
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
                        Some(Equiv::LuaCall { protected, .. }) => !protected,
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
                            if m.outer
                        && self.outer_scan.is_some()
                        && self.eqtb.int_params[crate::prim::IntParam::SuppressOuterError.idx() as usize] == 0
                    {
                                return self.forbidden_outer(t);
                            }
                            if m.protected && self.in_expanded_scan && self.csname_depth == 0 {
                                self.set_cur_cs_known(t, None);
                                return t;
                            }
                            if self.freeze_gts_in_edef(id) {
                                self.set_cur_cs(t);
                                return t;
                            }
                            if m.num_params == 0 && m.prefix.is_empty() && !self.xetex_macro_trace() {
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
                                self.set_cur_cs_known(t, None);
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
                                self.show_operand_ending_delimiter(p);
                                let tok = Token::from_cs(id);
                                self.set_cur_cs(tok);
                                return tok;
                            }
                            if self.is_expandable(p) {
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
                            } else if p == Prim::PdfPrimitiveExec && !self.in_expanded_scan {
                                t = self.pdf_primitive_target();
                                continue 'resolve;
                            } else {
                                self.set_cur_cs_known(t, Some(p));
                                return t;
                            }
                        }
                        Some(&Equiv::LuaCall { slot, protected: false }) => {
                            self.call_lua_function(slot as i32);
                            break 'expand;
                        }
                        None => {
                            self.undefined_cs_error(t);
                            break 'expand;
                        }
                        _ => {
                            self.set_cur_cs_known(t, None);
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

    #[inline(always)]
    pub fn is_expandable(&self, p: Prim) -> bool {
        EXPANDABLE_PRIMS.contains(p)
            && match p {
                Prim::U(u) => u.is_expandable(),
                Prim::XeTeXQuery(q) => q == crate::xetex_query::XeQuery::SelectorName,
                _ => true,
            }
    }

    /// Execute an expandable primitive; None = keep expanding,
    /// Some(t) = t is the resulting current token.
    pub fn expand_prim(&mut self, p: Prim, id: CsId) -> Option<Token> {
        // tex.web expand: cur_cs is the expanding control sequence, which a
        // general-text scan names in its errors (warning_index).
        self.cur_cs = Some(id);
        if Self::is_if_test(p)
            && self.eqtb.int_params[crate::prim::IntParam::TracingIfs as usize] > 0
        {
            self.show_if_start(p);
        }
        self.expand_prim_inner(p, id)
    }

    /// `\ifcase`.
    #[inline(never)]
    fn expand_if_case(&mut self, p: Prim, id: CsId) -> Option<Token> {
        // The case frame must exist while its numeric operand expands:
        // nested conditionals can remain open until after the first digit.
        let save = self.push_if(id, p, false);
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
        None
    }

    /// The conditionals with operands that scanning can expand: `\ifodd`,
    /// `\ifnum`, `\ifdim`, the box tests, `\iffontchar`, `\ifeof` and
    /// pdfTeX's `\ifpdfabsnum`/`\ifpdfabsdim`.
    #[inline(never)]
    fn expand_operand_if(&mut self, p: Prim, id: CsId) -> Option<Token> {
        // The outer conditional must exist before operand expansion:
        // an operand can leave a nested conditional open.
        let unless = std::mem::take(&mut self.unless_next);
        let save = self.push_if(id, p, unless);
        let previous = self.pending_if_depth.replace(self.if_stack.len());
        let value = match p {
            Prim::IfOdd => self.scan_int() % 2 != 0,
            Prim::IfNum | Prim::IfDim | Prim::IfPdfAbsNum | Prim::IfPdfAbsDim => {
                let numeric = matches!(p, Prim::IfNum | Prim::IfPdfAbsNum);
                // pdfTeX \ifpdfabsnum/\ifpdfabsdim compare magnitudes
                let absolute = matches!(p, Prim::IfPdfAbsNum | Prim::IfPdfAbsDim);
                let operand = |e: &mut Self| {
                    let v = if numeric {
                        e.scan_int()
                    } else {
                        e.scan_dimen(false, false)
                    };
                    if absolute {
                        v.wrapping_abs()
                    } else {
                        v
                    }
                };
                let a = operand(self);
                let rel = self.scan_relational();
                let b = operand(self);
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
                let lua_font = self.eqtb.fonts.get(f as usize).is_some_and(|font| font.lua_font().is_some());
                let c = if lua_font || self.is_native_font(f) {
                    self.scan_unicode_character_code("\\iffontchar")
                } else {
                    self.scan_character_code("\\iffontchar") as u32
                };
                if lua_font {
                    self.eqtb.fonts.get(f as usize).is_some_and(|font| font.lua_char_exists(c))
                } else {
                    self.native_char_present(f, c).unwrap_or_else(|| {
                        u8::try_from(c).ok().is_some_and(|byte| {
                            self.eqtb
                                .fonts
                                .get(f as usize)
                                .is_some_and(|font| font.char_present(byte))
                        })
                    })
                }
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
    }

    #[inline(always)]
    fn expand_prim_inner(&mut self, p: Prim, id: CsId) -> Option<Token> {
        use Prim::*;
        match p {
            ExpandAfter => self.expand_after(),
            IfCase => self.expand_if_case(p, id),
            IfOdd | IfNum | IfDim | IfVoid | IfFontChar | IfPdfAbsNum | IfPdfAbsDim | IfHBox
            | IfVBox | IfEOF => self.expand_operand_if(p, id),
            NoExpand => self.expand_noexpand(),
            EndCsName => {
                // extra \endcsname outside \csname: TeX errors then continues
                None
            }
            CsName | BeginCsName => self.expand_csname(p, id),
            LastNamedCs => self.expand_lastnamedcs(),
            The => {
                self.the_scan();
                None
            }
            Prim::String => self.expand_string(),
            Prim::Meaning => self.expand_meaning(),
            Number => self.expand_number(),
            RomanNumeral => self.expand_romannumeral(),
            Detokenize => self.expand_detokenize(),
            Expanded => self.expand_expanded(),
            UnExpanded => self.expand_unexpanded(),
            Unless => self.expand_unless(),
            IfTrue => self.do_if(true, id, p),
            IfFalse => self.do_if(false, id, p),
            IfChar => self.expand_if_char(p, id),
            IfCat => self.expand_if_cat(p, id),
            // tex.web §501 with §1370's `mode=0`: no mode test holds while a
            // `\write` text expands.
            IfVMode => self.do_if(!self.write_mode_zero && self.mode.is_v(), id, p),
            IfHMode => self.do_if(!self.write_mode_zero && self.mode.is_h(), id, p),
            IfMMode => self.do_if(!self.write_mode_zero && self.mode.is_m(), id, p),
            IfInner => {
                let ok = !self.write_mode_zero && self.mode.is_inner();
                self.do_if(ok, id, p)
            }
            IfDef => self.expand_if_def(p, id),
            IfInCsName => self.do_if(self.csname_depth > 0, id, p),
            IfCSName => self.expand_if_csname(p, id),
            IfX => self.expand_if_x(p, id),
            Or => self.expand_or(id),
            Else => self.expand_else(id),
            ElIf | ElIfX => self.expand_elif(id),
            Fi => self.expand_fi(id),
            _ => self.expand_prim_extended(p, id),
        }
    }

    /// `\expandafter`: expand the token after the next one.
    #[inline(never)]
    fn expand_after(&mut self) -> Option<Token> {
        use Prim::*;
        let t1 = self.raw_token_outer();
        let t2 = self.raw_token_outer();
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
                        } else if m.num_params == 0 && m.prefix.is_empty() && !self.xetex_macro_trace() {
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
                    &Equiv::LuaCall { slot, protected: false } => {
                        self.call_lua_function(slot as i32);
                    }
                    _ => {
                        self.push_token(t2);
                    }
                }
            } else {
                // tex.web §368: expand the undefined token (§370).
                self.undefined_cs_error(Token::from_cs(id2));
            }
        } else {
            self.push_token(t2);
        }
        self.push_token(t1);
        None
    }

    /// `\noexpand`.
    #[inline(never)]
    fn expand_noexpand(&mut self) -> Option<Token> {
        let t = self.raw_token_normal();
        let id = if t.is_cs() {
            Some(t.cs_id())
        } else if t.is_char() && t.cc() == 13 {
            Some(self.active_cs_id(t.chr()))
        } else {
            None
        };
        if let Some(id) = id {
            // tex.web §367: \noexpand marks every control sequence,
            // active characters included, with frozen_dont_expand.
            // An undefined one then reads as \relax (§358) instead
            // of raising "Undefined control sequence" when expanded.
            let needs_freeze = match self.eqtb.resolve(id) {
                None | Some(Equiv::Macro(_)) => true,
                Some(Equiv::Prim(p2)) => self.is_expandable(*p2),
                Some(Equiv::LuaCall { protected, .. }) => !protected,
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

    /// `\csname` (and LuaTeX `\begincsname`).
    #[inline(never)]
    fn expand_csname(&mut self, p: Prim, id: CsId) -> Option<Token> {
        use Prim::*;
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
            if self.take_csname_run(&mut name) {
                continue;
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
                // tex.web §372: the name ends at the first
                // unexpandable control sequence. Anything but
                // \endcsname, a \noexpand-marked token (which means
                // \relax) included, is an error and is read again.
                if self.cur_prim == Some(Prim::EndCsName) {
                    break;
                }
                self.push_token(t);
                self.error_at(
                    "Missing \\endcsname inserted",
                    csname_origin
                        .as_ref()
                        .map(crate::input::SourceMark::to_context),
                );
                break;
            }

            if t.is_char() && t.cc() == 9 {
                continue;
            }
            t.append_character_bytes(&mut name);
        }
        self.csname_depth = self.csname_depth.saturating_sub(1);
        let id = self.cs.intern(&name);
        if p == BeginCsName && self.eqtb.get(id).is_none() {
            // LuaTeX `\begincsname`: an undefined name expands to
            // nothing and stays undefined.
            return None;
        }
        self.last_named_cs = Some(id);
        if self.eqtb.get(id).is_none() {
            // tex.web §372: a new name means \relax (locally).
            let relax = self.cs.lookup(b"relax").unwrap();
            let r = self.eqtb.get(relax).cloned();
            if let Some(e) = r {
                self.eqtb.assign(id, e, false);
            }
        }
        if let Some(mark) = csname_origin {
            self.diagnostic_synthetic_source = Some((id, mark, csname_span.max(1)));
            self.diagnostic_sources_live = true;
        }
        Some(Token::from_cs(id))
    }

    /// LuaTeX `\lastnamedcs`.
    #[inline(never)]
    fn expand_lastnamedcs(&mut self) -> Option<Token> {
        let id = self
            .last_named_cs
            .unwrap_or_else(|| self.cs.lookup(b"relax").unwrap());
        Some(Token::from_cs(id))
    }

    /// `\string`.
    #[inline(never)]
    fn expand_string(&mut self) -> Option<Token> {
        let t = self.raw_token_normal();
        let esc = self.eqtb.int_params[crate::prim::IntParam::EscapeChar.idx() as usize];
        if t.is_cs() && self.exp_cs_name_string(t.cs_id(), esc) {
            return None;
        }
        let mut bytes: Vec<u8> = Vec::new();
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

    /// `\meaning`.
    #[inline(never)]
    fn expand_meaning(&mut self) -> Option<Token> {
        let t = self.raw_token_normal();
        let text = self.meaning_of(t);
        self.exp_string(&crate::tex_bytes::text_to_bytes(&text));
        None
    }

    /// `\number`.
    #[inline(never)]
    fn expand_number(&mut self) -> Option<Token> {
        let n = self.scan_int();
        self.exp_int(i64::from(n));
        None
    }

    /// `\romannumeral`.
    #[inline(never)]
    fn expand_romannumeral(&mut self) -> Option<Token> {
        let n = self.scan_int();
        if n > 0 {
            self.exp_string(roman(n).as_bytes());
        }
        None
    }

    /// e-TeX `\detokenize`.
    #[inline(never)]
    fn expand_detokenize(&mut self) -> Option<Token> {
        let toks = self.scan_general_text();
        let bytes = self.tokens_to_bytes(&toks);
        // The pooled buffer serves the string's token list next.
        self.recycle_token_vec(toks);
        self.exp_string(&bytes);
        None
    }

    /// `\expanded`.
    #[inline(never)]
    fn expand_expanded(&mut self) -> Option<Token> {
        let r = self.scan_general_text_expanded();

        self.push_tokens(r);
        None
    }

    /// e-TeX `\unexpanded`.
    #[inline(never)]
    fn expand_unexpanded(&mut self) -> Option<Token> {
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
                    Self::strip_outer_braces(&mut toks, 0);
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

    /// e-TeX `\unless`.
    #[inline(never)]
    fn expand_unless(&mut self) -> Option<Token> {
        use Prim::*;
        // etex.ch expand: \unless reads the next token unexpanded; only
        // a conditional other than \ifcase may follow, the flag is then
        // consumed by do_if.
        let t = self.raw_token();
        let target = if t.is_cs() {
            match self.eqtb.resolve(t.cs_id()) {
                Some(Equiv::Prim(p)) => Some(*p),
                _ => None,
            }
        } else {
            None
        };
        match target {
            // tex.web `goto reswitch` with the unless flag: the
            // conditional is expanded in this very step (so an
            // `\expandafter` over `\unless` sees its result).
            Some(p) if Self::is_if_test(p) && p != IfCase => {
                self.unless_next = true;
                self.expand_prim(p, t.cs_id())
            }
            _ => {
                self.push_token(t);
                let meaning = self.meaning_of(t);
                let name = meaning.split(':').next().unwrap_or("");
                self.error(&format!("You can't use `\\unless' before `{name}'"));
                None
            }
        }
    }

    /// `\if`.
    #[inline(never)]
    fn expand_if_char(&mut self, p: Prim, id: CsId) -> Option<Token> {
        // tex.web 498: push this \\if *before* get_x_token so a nested
        // true conditional from the test sits on top (babel
        // `\\if T\\ifeof1F\\fi T`).
        let unless = std::mem::take(&mut self.unless_next);
        let save = self.push_if(id, p, unless);
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

    /// `\ifcat`.
    #[inline(never)]
    fn expand_if_cat(&mut self, p: Prim, id: CsId) -> Option<Token> {
        let unless = std::mem::take(&mut self.unless_next);
        let save = self.push_if(id, p, unless);
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

    /// e-TeX `\ifdefined`.
    #[inline(never)]
    fn expand_if_def(&mut self, p: Prim, id: CsId) -> Option<Token> {
        let t = self.raw_token_normal();
        let def = if t.is_cs() {
            self.eqtb.resolve(t.cs_id()).is_some()
        } else if t.is_char() && t.cc() == 13 {
            let id = self.active_cs_id(t.chr());
            self.eqtb.resolve(id).is_some()
        } else {
            false
        };
        self.do_if(def, id, p)
    }

    /// e-TeX `\ifcsname`.
    #[inline(never)]
    fn expand_if_csname(&mut self, p: Prim, id: CsId) -> Option<Token> {
        // e-TeX \\ifcsname: get_x_token until \\endcsname; true iff
        // the name is already in the hash (even if \\relax). Must not
        // intern on a miss — that would poison \\ifcsname.
        // Take \\unless now so name collection cannot flip nested
        // \\ifx; restore before do_if so \\unless\\ifcsname inverts.
        let unless = std::mem::take(&mut self.unless_next);
        self.csname_depth += 1;
        let mut name: Vec<u8> = Vec::new();
        let mut aborted = false;
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
                        if self.eqtb.int_params[crate::prim::IntParam::SuppressIfCsnameError.idx() as usize] != 0 {
                            // conditional.c test_for_cs: skip to the
                            // \endcsname, the test fails
                            aborted = true;
                            loop {
                                let t = self.get_x_raw();
                                if t == EOF_MARKER {
                                    self.push_token(t);
                                    break;
                                }
                                if t.is_cs()
                                    && matches!(
                                        self.eqtb.resolve(t.cs_id()),
                                        Some(Equiv::Prim(crate::prim::Prim::EndCsName))
                                    )
                                {
                                    break;
                                }
                            }
                            break;
                        }
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
        let def = if aborted {
            self.last_named_cs = None;
            false
        } else if let Some(id) = self.cs.lookup(&name) {
            self.last_named_cs = Some(id);
            self.eqtb.resolve(id).is_some()
        } else {
            false
        };
        self.unless_next = unless;
        self.do_if(def, id, p);
        None
    }

    /// `\ifx`.
    #[inline(never)]
    fn expand_if_x(&mut self, p: Prim, id: CsId) -> Option<Token> {
        // tex.web if_x: operands see expandable PRIMS (\\csname...)
        // but never macros (\\ifx\\foo x is false for \\def\\foo{x}).
        let a = self.raw_token_normal();
        let b = self.raw_token_normal();

        let eq = self.ifx_equal(a, b);
        self.do_if(eq, id, p)
    }

    /// `\or`.
    #[inline(never)]
    fn expand_or(&mut self, id: CsId) -> Option<Token> {
        self.trace_if_delimiter(Prim::Or);
        if self.if_stack.last().is_some_and(|st| st.evaluating) {
            self.insert_relax(id);
            return None;
        }
        // encountered while accepting: skip to \fi or next \or
        if let Some(st) = self.if_stack.last_mut() {
            if st.matched {
                st.if_case = -1; // skip mode
                self.skip_to_fi();
                return None;
            }
        }
        self.error("Extra \\or");
        None
    }

    /// `\else`.
    #[inline(never)]
    fn expand_else(&mut self, id: CsId) -> Option<Token> {
        self.trace_if_delimiter(Prim::Else);
        if self.if_stack.last().is_some_and(|st| st.evaluating) {
            self.insert_relax(id);
            return None;
        }
        match self.if_stack.last_mut() {
            Some(st) if st.matched => self.skip_to_fi(),
            Some(st) => {
                st.accepting = true;
                st.matched = true;
                st.in_else = true;
            }
            None => self.error("Extra \\else"),
        }
        None
    }

    /// The unsupported `\elseif` forms.
    #[inline(never)]
    fn expand_elif(&mut self, id: CsId) -> Option<Token> {
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

    /// `\fi`.
    #[inline(never)]
    fn expand_fi(&mut self, id: CsId) -> Option<Token> {
        self.trace_if_delimiter(Prim::Fi);
        if self.if_stack.last().is_some_and(|st| st.evaluating) {
            self.insert_relax(id);
            return None;
        }
        if !self.pop_cond() {
            self.error("Extra \\fi");
        }
        None
    }
    /// The expandable primitives `expand_prim_inner` leaves to a separate
    /// function: they are rare, and their bodies would otherwise bloat the
    /// code of the conditionals and `\expandafter` that run all the time.
    #[inline(never)]
    fn expand_prim_extended(&mut self, p: Prim, id: CsId) -> Option<Token> {
        use Prim::*;
        match p {
            ScanTokens => {
                // \scantokens{...}: stringify and rescan (e-TeX pseudo_start;
                // the string is split into lines at \newlinechar)
                let toks = self.scan_general_text();
                let mut text = self.tokens_to_bytes(&toks);
                let newline = self.eqtb.int_params[crate::prim::IntParam::NewLineChar.idx() as usize];
                if let Ok(newline) = u8::try_from(newline) {
                    if newline != b'\n' {
                        for byte in text.iter_mut().filter(|byte| **byte == newline) {
                            *byte = b'\n';
                        }
                    }
                }
                if self.ensure_input_stack_room(1) {
                    self.input.push_file("<scantokens>".to_string(), text);
                    self.mark_scan_tokens_file();
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
                let code = self.tokens_to_lua_text(&toks);
                if let Err(err) = self.execute_directlua(code.as_bytes()) {
                    self.lua_error("LuaTeX error: ", &err);
                }
                None
            }
            LuaFunction => {
                let slot = self.scan_int();
                self.call_lua_function(slot);
                None
            }
            LuaBytecode => {
                let slot = self.scan_int();
                self.call_lua_bytecode(slot);
                None
            }
            Input => {
                self.do_input();
                None
            }
            EndInput => {
                self.do_endinput();
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
                let mut text = self.font_display_name(f);
                // xetex.web: the name of a native font is quoted
                if self.is_native_font(f) {
                    let name = self.eqtb.fonts[f as usize].tfm_name.clone();
                    let q = if name.contains('"') { '\'' } else { '"' };
                    text = text.replacen(&name, &format!("{q}{name}{q}"), 1);
                }
                self.exp_string(text.as_bytes());
                None
            }
            Prim::JobName => {
                // LuaTeX does not quote the job name itself; its
                // process_jobname callback may (textoken.c print_job_name).
                let text = if self.engine_kind == crate::engine::EngineKind::LuaTeX {
                    let name = self.job_name.clone();
                    self.run_lua_string_callback("process_jobname", &name)
                        .unwrap_or(name)
                } else {
                    self.quoted_job_name()
                };
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
            Prim::PdfPageRef | Prim::PdfFontName | Prim::PdfFontObjNum | Prim::PdfXFormName => {
                let value = match p {
                    Prim::PdfPageRef => self.pdf_page_ref(),
                    Prim::PdfFontName => self.pdf_font_name(),
                    Prim::PdfFontObjNum => self.pdf_font_objnum(),
                    _ => self.pdf_xform_name(),
                };
                if let Some(value) = value {
                    self.exp_string(value.to_string().as_bytes());
                }
                None
            }
            Prim::PdfXImageBBox => {
                if let Some(value) = self.pdf_ximage_bbox() {
                    let text = self.scaled_to_string(value);
                    self.exp_string(text.as_bytes());
                }
                None
            }
            Prim::PdfBanner => {
                self.exp_string(crate::pdftex::PDFTEX_BANNER.as_bytes());
                None
            }
            Prim::LeftMarginKern | Prim::RightMarginKern => {
                // pdftex §11371: skip discardables and the structural
                // \leftskip/\rightskip glue, then report the margin kern's
                // width (0 if the line box has none).
                let left = p == LeftMarginKern;
                let n = self.scan_reg_num();
                let width = if self.engine_kind == crate::engine::EngineKind::LuaTeX {
                    self.lua_margin_kern_width(n, left)
                } else {
                    self.margin_kern_width(n, left)
                };
                let s = width.map_or_else(|| "0pt".to_string(), |w| self.scaled_to_string(w));
                self.exp_string(s.as_bytes());
                None
            }
            IfPdfPrimitive => {
                // pdftex.web if_pdfprimitive_code: the next token (read
                // without expansion) still has the primitive meaning that
                // its name had in INITEX.
                let save = self.scanner_status;
                self.scanner_status = ScannerStatus::Normal;
                let t = self.raw_token();
                self.scanner_status = save;
                let b = t.is_cs()
                    && match (
                        self.primitive_table.get(self.cs.name(t.cs_id())),
                        self.eqtb.resolve(t.cs_id()),
                    ) {
                        (Some(p), Some(Equiv::Prim(q))) => p == q,
                        _ => false,
                    };
                self.do_if(b, id, p)
            }
            PdfPrimitive => {
                // pdftex.web <Implement \pdfprimitive>
                let save = self.scanner_status;
                self.scanner_status = ScannerStatus::Normal;
                let t = self.raw_token();
                self.scanner_status = save;
                if !t.is_cs() {
                    self.missing_primitive_name(t);
                    return None;
                }
                let name = self.cs.name(t.cs_id()).to_vec();
                let Some(hidden) = self.primitive_cs(&name) else {
                    self.missing_primitive_name(t);
                    return None;
                };
                match self.eqtb.get(hidden) {
                    Some(Equiv::Prim(p)) if self.is_expandable(*p) => {
                        Some(Token::from_cs(hidden))
                    }
                    _ => {
                        // the name survives a round trip through a file
                        self.push_token(t);
                        self.push_token(Token::from_cs(self.ids.frozen_primitive));
                        None
                    }
                }
            }
            PdfInsertHt => {
                // pdftex.web pdf_insert_ht_code: height(r) of the page
                // insertion record for class n, else 0pt
                let n = self.scan_reg_num();
                // (pdfTeX prints a missing class as `0pt`, not `0.0pt`)
                let text = match self.page_insertions.iter().find(|s| s.num == n) {
                    Some(s) => self.scaled_to_string(s.height_raw as i32),
                    None => "0pt".to_string(),
                };
                self.exp_string(text.as_bytes());
                None
            }
            PdfTexRevision => {
                self.exp_string(b"29");
                None
            }
            EtxRevision => {
                // LuaTeX implements e-TeX 2.2; pdfTeX 1.40 e-TeX 2.6.
                self.exp_string(if self.engine_kind == crate::engine::EngineKind::LuaTeX { b".2" } else { b".6" });
                None
            }
            XeTeXUchar => {
                let c = self.scan_usv_num();
                let token = if c == 32 { Token::space() } else { Token::unicode_char(12, c) };
                self.push_token(token);
                None
            }
            UcharCat if self.engine_kind == crate::engine::EngineKind::XeTeX => {
                let c = self.scan_usv_num();
                let value = self.scan_int();
                // xetex.web `illegal_Ucharcat_catcode`
                let cat = if (1..=13).contains(&value) && value != 5 && value != 9 {
                    value as u8
                } else {
                    self.error(&format!(
                        "Invalid code ({value}), should be in the ranges 1..4, 6..8, 10..13"
                    ));
                    12
                };
                let token = if cat == 13 {
                    let id = self.cs.intern(&Engine::active_cs_name(c));
                    Token::from_cs(id)
                } else {
                    Token::unicode_char(cat, c)
                };
                self.push_token(token);
                None
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
                        source.map(|mark| mark.to_context()),
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
                let loaded_files_before_lookup = self.loaded_files.len();
                let font_files_before_lookup = self.font_loader.dependency_files.len();
                let found = self.find_input_file(&name);
                // Resolving an input normally records a content dependency.
                // This primitive observes only metadata, so keep its cheaper,
                // size-specific dependency unless another operation consumes
                // the same file independently.
                self.loaded_files.truncate(loaded_files_before_lookup);
                let sz = match found {
                    Some(crate::io::FoundInputFile::Path(path)) => {
                        self.font_loader
                            .discard_file_dependency_since(font_files_before_lookup, &path);
                        tex_kpse::fs::metadata(&path)
                            .ok()
                            .filter(|metadata| metadata.is_file())
                            .map(|metadata| {
                                let size = metadata.len();
                                // Like a read (`record_loaded_bytes`), the size
                                // of a file this run wrote is its own output.
                                if !self.written_before(&path) {
                                    self.loaded_file_sizes.push((path, size));
                                }
                                size
                            })
                    }
                    Some(crate::io::FoundInputFile::Bytes(data)) => Some(data.len() as u64),
                    None => None,
                };
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
                    let data = match self.find_input_file(&name) {
                        Some(crate::io::FoundInputFile::Path(path)) => {
                            let Ok(data) = tex_kpse::fs::read(&path) else {
                                return None;
                            };
                            self.record_loaded_bytes(&path, &data);
                            self.loaded_files.push(path);
                            data
                        }
                        Some(crate::io::FoundInputFile::Bytes(data)) => data,
                        None => return None,
                    };
                    md5::compute(&data)
                } else {
                    md5::compute(&bytes)
                };
                self.exp_string(format!("{digest:X}").as_bytes());
                None
            }
            PdfFileModDate => {
                let name = {
                    let t = self.scan_general_text_expanded();
                    self.tokens_to_string(&t)
                };
                let date = match self.find_input_file(&name) {
                    Some(crate::io::FoundInputFile::Path(path)) => {
                        let date = disk_file_mod_date(&path);
                        // The result cache revalidates the date it reports.
                        // A file this run wrote has the time of the run.
                        match &date {
                            Some(date) if !self.written_before(&path) => {
                                self.loaded_file_mod_dates.push((path, date.clone()))
                            }
                            _ => self.font_loader.dependency_tracking_complete = false,
                        }
                        date
                    }
                    Some(crate::io::FoundInputFile::Bytes(_)) => Some(pdf_file_mod_date(None)),
                    None => None,
                };
                if let Some(date) = date {
                    self.exp_string(date.as_bytes());
                }
                None
            }
            PdfCreationDate => {
                // pdfTeX fixes the date when the job starts.
                let date = self
                    .pdf_creation_date
                    .get_or_insert_with(pdf_creation_date)
                    .clone();
                self.exp_string(date.as_bytes());
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
                let bytes = match self.find_input_file(&name) {
                    Some(crate::io::FoundInputFile::Path(path)) => {
                        // Only the requested range affects this expansion.
                        // Reading the whole input here makes a one-byte dump
                        // allocate the size of an arbitrarily large file.
                        // Until the dependency cache can represent byte-range
                        // reads, fail closed rather than publishing a cache
                        // entry sampled after the pass.
                        self.font_loader.dependency_tracking_complete = false;
                        let Ok(mut input) = tex_kpse::fs::File::open(&path) else {
                            return None;
                        };
                        if input.seek(SeekFrom::Start(offset as u64)).is_err() {
                            return None;
                        }
                        let mut data = Vec::new();
                        // Read one byte beyond the largest representable hex
                        // token list. This distinguishes an oversized result
                        // without allocating the full user-supplied length,
                        // while a huge request near EOF can still return a
                        // small legal tail.
                        let read_limit = (length as u64).min(max_dump_bytes as u64 + 1);
                        if input.take(read_limit).read_to_end(&mut data).is_err() {
                            return None;
                        }
                        self.loaded_files.push(path);
                        data
                    }
                    Some(crate::io::FoundInputFile::Bytes(data)) => {
                        let start = (offset as usize).min(data.len());
                        let end = start.saturating_add(length as usize).min(data.len());
                        data[start..end].to_vec()
                    }
                    None => return None,
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
            PdfColorStackInit => {
                let stack = self.pdf_colorstack_init();
                self.exp_string(stack.to_string().as_bytes());
                None
            }
            PdfUniformDeviate => {
                let x = self.scan_int();
                let value = self.rng.unif_rand(x);
                self.exp_string(value.to_string().as_bytes());
                None
            }
            PdfNormalDeviate => {
                let value = self.rng.norm_rand();
                self.exp_string(value.to_string().as_bytes());
                None
            }
            PdfEscapeString | PdfEscapeName | PdfEscapeHex => {
                let toks = self.scan_general_text_expanded();
                let bytes = self.token_list_bytes(&toks);
                self.exp_string(&crate::io::pdf_escape(p, &bytes));
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
            Prim::XeTeXQuery(crate::xetex_query::XeQuery::SelectorName) => {
                self.expand_xetex_selector_name();
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
            PdfVariable => {
                self.expand_pdf_variable();
                None
            }
            PdfFeedback => {
                self.expand_pdf_feedback();
                None
            }
            DviVariable => {
                self.lua_warning("dvi backend", "unexpected use of \\dvivariable");
                None
            }
            DviFeedback => {
                self.expand_dvi_feedback();
                None
            }
            EtxVersionString => {
                self.exp_string(b"2.2");
                None
            }
            CsString => {
                self.expand_csstring(id);
                None
            }
            FormatName => {
                self.expand_format_name();
                None
            }
            LuaEscapeString => {
                self.expand_lua_escape_string();
                None
            }
            U(u) => self.uprim_expand(u),
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
        self.cs_input_token(id)
    }

    fn unfreeze_unexpanded_token(&self, t: Token) -> Token {
        if t.0 >= UNEXPANDED_CS_FLAG && t.0 < 0xFFFF_0000 {
            self.cs_input_token(t.0 & 0x1FFF_FFFF)
        } else {
            t.unfreeze()
        }
    }

    /// The input token for control sequence `id`: active characters are
    /// stored as character tokens.
    #[inline]
    fn cs_input_token(&self, id: CsId) -> Token {
        if self.cs.is_active(id) {
            if let Some(scalar) = Self::active_cs_scalar(self.cs.name(id)) {
                return Token::char(CAT_ACTIVE, scalar);
            }
        }
        Token::from_cs(id)
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
        let t = self.raw_token_outer();
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
    /// e-TeX's `if_*_code` of a conditional primitive (cur_if).
    pub(crate) fn if_code(p: Prim) -> u8 {
        match p {
            Prim::IfChar => 0,
            Prim::IfCat => 1,
            Prim::IfNum => 2,
            Prim::IfDim => 3,
            Prim::IfOdd => 4,
            Prim::IfVMode => 5,
            Prim::IfHMode => 6,
            Prim::IfMMode => 7,
            Prim::IfInner => 8,
            Prim::IfVoid => 9,
            Prim::IfHBox => 10,
            Prim::IfVBox => 11,
            Prim::IfX => 12,
            Prim::IfEOF => 13,
            Prim::IfTrue => 14,
            Prim::IfFalse => 15,
            Prim::IfCase => 16,
            Prim::IfDef => 17,
            Prim::IfCSName => 18,
            Prim::IfFontChar => 19,
            Prim::IfInCsName => 20,
            Prim::IfPdfPrimitive => 21,
            Prim::IfPdfAbsNum => 22,
            Prim::IfPdfAbsDim => 23,
            _ => 0,
        }
    }

    /// tex.web expand §510 for a `\fi`, `\else` or `\or` that meets a
    /// conditional still evaluating its operand: TeX shows it, then inserts
    /// `\relax` and reads the delimiter again when it skips or selects a
    /// branch (where it is shown a second time). TeXres ends the operand with
    /// the delimiter itself, so the first showing happens here, once.
    pub(crate) fn show_operand_ending_delimiter(&mut self, p: Prim) {
        if matches!(p, Prim::Fi | Prim::Else | Prim::Or)
            && self.eqtb.int_params[crate::prim::IntParam::TracingIfs as usize] > 0
        {
            if let Some(state) = self.if_stack.last_mut() {
                if state.delimiter_shown {
                    return;
                }
                state.delimiter_shown = true;
            }
            self.show_if_delimiter(p);
        }
    }

    /// tex.web "Push the condition stack" (the conditional's operands are
    /// scanned with it already on top). `p` is the conditional being
    /// expanded (e-TeX `cur_if := cur_chr`).
    fn push_if(&mut self, id: CsId, p: Prim, unless: bool) -> usize {
        let loc = self.current_token_source_mark();
        // The file name is kept only when `loc` lies in another file.
        let (loc_file, loc_line) = match self.input.top_file_origin() {
            Some((origin, line)) if loc.as_ref().is_some_and(|mark| mark.in_origin(origin)) => {
                (None, line)
            }
            _ => {
                let (name, line) = self.input.current_file_location();
                (Some(name), line)
            }
        };
        self.if_stack.push(crate::engine::IfState {
            accepting: false,
            matched: false,
            if_case: -1,
            evaluating: true,
            delimiter_shown: false,
            kind: Self::if_code(p),
            unless,
            in_else: false,
            loc_file,
            loc_line,
            loc_cs: id,
            loc,
        });
        self.if_stack.len() - 1
    }

    /// tex.web "Pop the condition stack"; false when it is empty.
    #[inline]
    fn pop_cond(&mut self) -> bool {
        let depth = self.if_stack.len();
        if depth == 0 {
            return false;
        }
        // e-TeX: a conditional that began in another file than the one it
        // ends in is recorded (and reported) by if_warning, which acts only
        // when the innermost tracked file began at this depth.
        if self.file_nests.last().is_some_and(|nest| nest.if_depth == depth) {
            self.if_warning();
        }
        self.if_stack.pop();
        true
    }

    #[inline]
    fn finish_if(&mut self, save: usize, b: bool) -> Option<Token> {
        if let Some(st) = self.if_stack.get_mut(save) {
            st.evaluating = false;
            if b {
                st.accepting = true;
                st.matched = true;
            }
        }
        if !b {
            self.skip_to_else_or_fi(save);
        }
        None
    }

    #[inline(never)]
    fn do_if(&mut self, mut b: bool, id: CsId, p: Prim) -> Option<Token> {
        let unless = std::mem::take(&mut self.unless_next);
        if unless {
            b = !b;
        }
        let save = self.push_if(id, p, unless);
        self.finish_if(save, b)
    }

    /// `show_if_delimiter` with its `\tracingifs` test inlined: the
    /// delimiters run all the time, the trace almost never.
    #[inline(always)]
    fn trace_if_delimiter(&mut self, delimiter: Prim) {
        if self.eqtb.int_params[crate::prim::IntParam::TracingIfs as usize] > 0 {
            self.show_if_delimiter(delimiter);
        }
    }

    #[inline(always)]
    fn is_if_test(p: Prim) -> bool {
        IF_TESTS.contains(p)
    }

    /// tex.web §494: unexpanded skip to next \fi/\else/\or at local depth 0.
    fn pass_text(&mut self) -> Option<Prim> {
        let save_scanner = self.scanner_status;
        self.scanner_status = ScannerStatus::Skipping;
        let skip_line = self.input.current_file_line();
        let mut l = 0i32;
        let res = 'skip: loop {
            if let Some(delimiter) = self.pass_text_run(&mut l) {
                break 'skip Some(delimiter);
            }
            let t = self.raw_token();
            if t == EOF_MARKER {
                self.incomplete_conditional(EOF_MARKER, skip_line);
                continue;
            }
            // A \noexpand-guarded token means \relax here (tex.web §358).
            // Active characters carry meanings like control sequences.
            let id = if t.is_cs() {
                if t.0 >= NOEXP_FLAG {
                    continue;
                }
                t.cs_id()
            } else if t.is_char() && t.cc() == CAT_ACTIVE {
                match self.active_cs_lookup(t.chr()) {
                    Some(id) => id,
                    None => continue,
                }
            } else {
                continue;
            };
            let is = match self.eqtb.resolve(id) {
                Some(Equiv::Prim(p)) => *p,
                Some(Equiv::Macro(_)) if self.eqtb.has_outer_macros() && self.eqtb.is_outer_cs(id) => {
                    self.incomplete_conditional(t, skip_line);
                    continue;
                }
                _ => continue,
            };
            // \unless is not a test itself; the conditional after it is.
            if Self::is_if_test(is) {
                l += 1;
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
        if res.is_some() && self.eqtb.int_params[crate::prim::IntParam::TracingIfs as usize] > 0 {
            if let Some(delimiter) = res {
                self.show_if_delimiter(delimiter);
            }
        }
        self.scanner_status = save_scanner;
        res
    }

    /// Continue the decimal constant `value` with the digits (other
    /// characters `0`-`9`) at the front of the current token list, taking
    /// them as `scan_int`'s expanding fetches would, and with the space
    /// that ends the constant when `space_ends` (`scan_int`; `scan_dimen`
    /// reads on). Stops before a digit that would overflow and before any
    /// other token. Returns true when it consumed that space (the constant
    /// is complete).
    pub(crate) fn take_decimal_run(&mut self, value: &mut i64, space_ends: bool) -> bool {
        let Some((segment, trace_depth)) = self.token_list_front() else {
            return false;
        };
        let mut v = *value;
        let mut length = 0;
        let mut last = None;
        let mut space = false;
        for &t in segment {
            let digit = t.0.wrapping_sub(Token::other(b'0').0);
            if digit < 10 {
                let next = v * 10 + i64::from(digit);
                if next > 0x7FFF_FFFF {
                    break;
                }
                v = next;
            } else if space_ends && t.0 >> 24 == u32::from(crate::token::CAT_SPACE) {
                space = true;
            } else {
                break;
            }
            length += 1;
            last = Some(t);
            if space {
                break;
            }
        }
        let Some(last) = last else {
            return false;
        };
        self.consume_token_list_front(length, trace_depth);
        self.unexpanded_parameter = false;
        self.set_cur_char(last);
        *value = v;
        space
    }

    /// Skip the tokens at the front of the current token list that
    /// `pass_text` would pass over one by one, counting nested conditionals
    /// in `level`. Returns the \fi, \else or \or that ends the text at
    /// level zero; stops early before any token that needs `raw_token` or
    /// an active character's meaning.
    fn pass_text_run(&mut self, level: &mut i32) -> Option<Prim> {
        if !self.pushed.is_empty() || self.align_macro_arg {
            return None;
        }
        let (segment, counts_braces, trace_depth) = match self.input.stack.last() {
            Some(crate::input::Source::TokList {
                toks,
                pos,
                name,
                trace_depth,
                ..
            }) => (&toks[*pos..], *name != crate::align::PEEK_SRC, *trace_depth),
            Some(crate::input::Source::MacroFrame(frame)) => {
                (frame.segment(), true, frame.trace_depth)
            }
            _ => return None,
        };
        let mut braces = 0i32;
        let mut found = None;
        let mut length = 0;
        for &t in segment {
            let top = t.0 >> 24;
            if top < 0x80 {
                if top == u32::from(CAT_ACTIVE) {
                    break;
                }
                braces += i32::from(top == 1) - i32::from(top == 2);
            } else if t.0 < NOEXP_FLAG {
                // The \outer flag lives in the eqtb entry: no macro load.
                match self.eqtb.resolve(t.cs_id()) {
                    Some(Equiv::Prim(p)) if Self::is_if_test(*p) => *level += 1,
                    Some(Equiv::Prim(p)) if IF_DELIMITERS.contains(*p) => {
                        if *level == 0 {
                            found = Some(*p);
                            length += 1;
                            break;
                        }
                        if *p == Prim::Fi {
                            *level -= 1;
                        }
                    }
                    Some(Equiv::Macro(_))
                        if self.eqtb.has_outer_macros() && self.eqtb.is_outer_cs(t.cs_id()) =>
                    {
                        break
                    }
                    _ => {}
                }
            } else if t.0 >= 0xFFFF_0000 {
                // \par and end-of-input markers.
                break;
            }
            // A \noexpand-guarded token means \relax here (tex.web §358).
            length += 1;
        }
        if length == 0 {
            return None;
        }
        self.consume_token_list_front(length, trace_depth);
        if counts_braces {
            self.align_brace_depth = self.align_brace_depth.saturating_add(braces);
        }
        found
    }

    /// tex.web §336: an \outer macro, or the end of the input, ends skipped
    /// conditional text. TeX inserts \fi before it and reads it again
    /// afterwards; `skip_line` is where the skipping began.
    fn incomplete_conditional(&mut self, outer: Token, skip_line: u32) {
        let opener = self.if_stack.last().map_or(0, |state| state.loc_cs);
        let opener = if (opener as usize) < self.cs.len() && opener != 0 {
            self.display_cs(opener).trim_end().to_string()
        } else {
            "\\if".to_string()
        };
        self.push_token(outer);
        if let Some(fi) = self.cs.lookup(b"fi") {
            self.push_token(Token::from_cs(fi));
        }
        if outer == EOF_MARKER {
            if self.eof_reported {
                return;
            }
            self.eof_reported = true;
        }
        self.error(&format!(
            "Incomplete {opener}; all text was ignored after line {skip_line}"
        ));
    }

    /// tex.web §500: after a false test, keep skipping until the closer
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
                        self.pop_cond();
                        return;
                    }
                    Prim::Else | Prim::ElIf | Prim::ElIfX => {
                        if let Some(st) = self.if_stack.get_mut(save) {
                            st.accepting = true;
                            st.matched = true;
                            st.in_else = true;
                        }
                        return;
                    }
                    // TeX reports the stray \or and keeps skipping.
                    _ => self.error("Extra \\or"),
                }
            } else if chr == Prim::Fi {
                self.pop_cond();
            }
        }
    }

    /// Skip to the branch delimiter belonging to `target`. Numeric operands
    /// can leave expanded nested conditionals open above an \ifcase frame.
    fn skip_branch(&mut self, if_case: bool, target: usize) {
        loop {
            let Some(delimiter) = self.pass_text() else {
                return;
            };
            if self.if_stack.len() > target + 1 {
                // This belongs to a conditional opened while scanning the
                // target's numeric operand, not to the target itself.
                if delimiter == Prim::Fi {
                    self.pop_cond();
                }
                continue;
            }
            match delimiter {
                Prim::Fi => {
                    self.pop_cond();
                    return;
                }
                Prim::Or => {
                    if if_case {
                        let Some(st) = self.if_stack.last_mut() else {
                            return;
                        };
                        if st.matched {
                            self.skip_to_fi();
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
                Prim::ElIf | Prim::ElIfX => {
                    self.skip_branch(false, target);
                    return;
                }
                Prim::Else => {
                    if let Some(st) = self.if_stack.last_mut() {
                        if st.matched {
                            self.skip_to_fi();
                        } else {
                            st.accepting = true;
                            st.matched = true;
                            st.in_else = true;
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
    }

    /// Skip the rest of a conditional whose branch was already taken, up to
    /// and including its \fi (tex.web §510).
    fn skip_to_fi(&mut self) {
        while let Some(delimiter) = self.pass_text() {
            if delimiter == Prim::Fi {
                self.pop_cond();
                return;
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

        if self.xetex_macro_trace() {
            self.current_macro = id;
            let show_args = self.trace_macro_call(invocation, m);
            if m.num_params == 0 && m.prefix.is_empty() {
                self.enter_macro_diagnostic(id, invocation);
                self.push_tokens_rc(std::rc::Rc::clone(&m.body), id);
                return;
            }
            self.enter_macro_diagnostic(id, invocation);
            let call_site = self.diagnostic_macro_call_site.clone();
            self.expand_macro_with_args(id, m, call_site.as_ref(), Some(show_args));
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
        self.expand_macro_with_args(id, m, call_site.as_ref(), None);
    }
    fn expand_macro_with_args(
        &mut self,
        id: CsId,
        m: &Macro,
        origin: Option<&crate::input::SourceMark>,
        trace: Option<bool>,
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
                    // tex.web §398: the mismatching token is consumed.
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
        let selector = if trace.is_some() {
            None
        } else if m.body.is_empty() {
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
        let mut buffer = self.token_vec_pool.pop().unwrap_or_default();
        // Recycled buffers converge on a size that holds typical arguments
        // without regrowing token by token.
        buffer.reserve(MIN_ARG_BUFFER);
        let mut args = crate::input::MacroArgs::with_buffer(buffer);
        for (i, delim) in m.params.iter().enumerate().take(m.num_params as usize) {
            let keep = selector.map_or(true, |selected| selected == i + 1);
            let start = args.open_start();
            let scanned = if delim.is_empty() {
                self.scan_undelimited_arg(id, m.long, keep, args.buffer(), origin)
            } else {
                self.scan_delimited(id, delim, m.long, args.buffer(), origin)
            };
            if scanned.is_err() {
                self.recycle_token_vec(args.into_buffer());
                return;
            }
            if !keep {
                args.buffer().truncate(start);
            }
            args.finish_arg();
            if trace == Some(true) {
                let shown = args.get(i).unwrap_or_default().to_vec();
                self.trace_macro_arg(i + 1, &shown);
            }
        }
        // A `show_error_hook` shows the macro level and its `<argument>`
        // like TeX, so the shortcut that drops the macro level is not taken.
        if selector.is_some_and(|index| index <= m.num_params as usize)
            && !self.cb_defined(crate::lua_callbacks::Cb::ShowErrorHook)
        {
            // Only the selected argument was retained in the buffer.
            let mut selected = args.into_buffer();
            if selected.len() == 1 {
                self.push_token(selected[0]);
                selected.clear();
            }
            if selected.is_empty() {
                self.recycle_token_vec(selected);
            } else {
                self.push_macro_tokens(selected, id);
            }
            return;
        }
        if !m.has_param_refs {
            self.recycle_token_vec(args.into_buffer());
            self.push_tokens_rc(std::rc::Rc::clone(&m.body), id);
            return;
        }
        let references = m.ensure_replacement_plan();
        let Some(length) =
            m.replacement_length(&references, &args, crate::input::MAX_TOKEN_LIST_TOKENS)
        else {
            self.recycle_token_vec(args.into_buffer());
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
            self.recycle_token_vec(args.into_buffer());
            return;
        }
        let frame = crate::input::MacroFrame::new(
            std::rc::Rc::clone(&m.body),
            references,
            args,
            Some(id),
            self.trace_depth(),
        );
        self.try_push_macro_frame(frame);
    }

    /// tex.web §393: an undelimited argument is the next nonblank token or
    /// balanced group.
    fn scan_undelimited_arg(
        &mut self,
        id: CsId,
        long: bool,
        collect: bool,
        out: &mut Vec<Token>,
        origin: Option<&crate::input::SourceMark>,
    ) -> Result<(), ArgAbort> {
        let saved_align_macro_arg = self.align_macro_arg;
        self.align_macro_arg = true;
        self.skip_raw_spaces();
        self.align_macro_arg = saved_align_macro_arg;
        let raw = self.macro_arg_token();
        let stored = self.unfreeze_input_token(raw);
        let t = self.unfreeze_unexpanded_token(stored);
        if t == EOF_MARKER {
            return Err(self.abort_file_ended(id, origin));
        }
        if self.is_partoken(t) && !long && !self.suppress_long_error() {
            return Err(self.abort_paragraph(id, stored, origin));
        }
        if self.is_outer_token(raw) {
            return Err(self.abort_outer(id, stored, origin));
        }
        if t.is_char() && t.cc() == 2 {
            return Err(self.abort_extra_brace(id, stored, origin));
        }
        if t.is_char() && t.cc() == 1 {
            return self.scan_macro_balanced_arg(id, long, collect, out, origin);
        }
        out.push(stored);
        Ok(())
    }

    /// LuaTeX `\primitive` with something that names no primitive (expand.c):
    /// the token is read again after the error, which
    /// `\suppressprimitiveerror` silences (the token is then gone).
    fn missing_primitive_name(&mut self, t: Token) {
        if self.engine_kind != crate::engine::EngineKind::LuaTeX
            || self.eqtb.int_params[crate::prim::IntParam::SuppressPrimitiveError.idx() as usize] != 0
        {
            return;
        }
        self.push_token(t);
        self.error("Missing primitive name");
    }

    /// LuaTeX `\suppresslongerror`: a `\par` in the argument of a non-long
    /// macro is an ordinary token.
    #[inline]
    fn suppress_long_error(&self) -> bool {
        self.eqtb.int_params[crate::prim::IntParam::SuppressLongError.idx() as usize] != 0
    }

    /// tex.web §396: a forbidden \par ends the call; TeX reads it again.
    fn abort_paragraph(
        &mut self,
        id: CsId,
        par: Token,
        origin: Option<&crate::input::SourceMark>,
    ) -> ArgAbort {
        self.push_token(par);
        self.error_at(
            &format!("Paragraph ended before {} was complete", self.display_cs(id)),
            origin.map(crate::input::SourceMark::to_context),
        );
        ArgAbort
    }

    /// tex.web §395: the brace is read again after an inserted \par, which
    /// ends the call even for a \long macro.
    fn abort_extra_brace(
        &mut self,
        id: CsId,
        brace: Token,
        origin: Option<&crate::input::SourceMark>,
    ) -> ArgAbort {
        self.push_token(brace);
        self.error_at(
            &format!("Argument of {} has an extra }}", self.display_cs(id)),
            origin.map(crate::input::SourceMark::to_context),
        );
        let par = Token::from_cs(self.partoken_id());
        self.abort_paragraph(id, par, origin)
    }

    /// tex.web §336-§339: an \outer macro cannot occur in an argument. It is
    /// read again after the inserted \par has silently ended the call.
    fn abort_outer(
        &mut self,
        id: CsId,
        outer: Token,
        origin: Option<&crate::input::SourceMark>,
    ) -> ArgAbort {
        self.push_token(outer);
        self.error_at(
            &format!(
                "Forbidden control sequence found while scanning use of {}",
                self.display_cs(id)
            ),
            origin.map(crate::input::SourceMark::to_context),
        );
        ArgAbort
    }

    /// tex.web §336-§339: the input ended inside an argument. The inserted
    /// \par silently ends the call; the end of the input is read again.
    #[cold]
    #[inline(never)]
    fn abort_file_ended(
        &mut self,
        id: CsId,
        origin: Option<&crate::input::SourceMark>,
    ) -> ArgAbort {
        self.push_token(EOF_MARKER);
        if !self.eof_reported {
            self.eof_reported = true;
            if self.eqtb.int_params[crate::prim::IntParam::SuppressOuterError.idx() as usize] == 0 {
                self.error_at(
                    &format!("File ended while scanning use of {}", self.display_cs(id)),
                    origin.map(crate::input::SourceMark::to_context),
                );
            }
        }
        ArgAbort
    }

    /// tex.web §336-§339 for an \outer macro met by a delimited argument:
    /// the macro is read again after the call, which sees a space in its
    /// place and then the inserted \par.
    #[cold]
    #[inline(never)]
    fn report_outer_in_argument(
        &mut self,
        id: CsId,
        outer: Token,
        origin: Option<&crate::input::SourceMark>,
    ) {
        let stored = self.unfreeze_input_token(outer);
        self.push_token(stored);
        let par = Token::from_cs(self.partoken_id());
        self.push_token(par);
        self.error_at(
            &format!(
                "Forbidden control sequence found while scanning use of {}",
                self.display_cs(id)
            ),
            origin.map(crate::input::SourceMark::to_context),
        );
    }

    /// True for an \outer macro token (see `is_outer_macro_token`) and for
    /// the end-of-\write and end-of-output sentinels, which stand for TeX's
    /// frozen outer `\endwrite`.
    #[inline(always)]
    fn is_outer_token(&self, t: Token) -> bool {
        if t.0 >= crate::page::WRITE_END_TOKEN.0 {
            t != EOF_MARKER && t != PAR_END
        } else {
            self.is_outer_macro_token(t)
        }
    }

    /// True for a control sequence or active character whose meaning is an
    /// \outer macro. Takes the token as fetched: tokens guarded by
    /// \noexpand are exempt (tex.web §358). LuaTeX's
    /// \suppressoutererror makes `check_outer_validity` return at once.
    #[inline(always)]
    pub(crate) fn is_outer_macro_token(&self, t: Token) -> bool {
        if !self.eqtb.has_outer_macros()
            || self.eqtb.int_params[crate::prim::IntParam::SuppressOuterError.idx() as usize] != 0
        {
            return false;
        }
        if t.is_cs() {
            t.0 < NOEXP_FLAG && self.is_outer_cs(t.cs_id())
        } else {
            t.is_char() && t.cc() == CAT_ACTIVE && self.is_outer_active(t.chr())
        }
    }

    #[inline(always)]
    fn is_outer_cs(&self, id: CsId) -> bool {
        self.eqtb.is_outer_cs(id)
    }

    #[inline(never)]
    fn is_outer_active(&self, c: u32) -> bool {
        self.active_cs_lookup(c).is_some_and(|id| self.is_outer_cs(id))
    }

    /// Run `scan` as the absorbing scan `kind` of `owner` (tex.web's
    /// scanner_status and warning_index), restoring the enclosing one.
    pub(crate) fn with_outer_scan<R>(
        &mut self,
        kind: OuterScan,
        owner: Option<CsId>,
        scan: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let saved = self.outer_scan.replace((kind, owner));
        let result = scan(self);
        self.outer_scan = saved;
        result
    }

    /// tex.web get_token under an absorbing scan: an \outer macro is
    /// reported by `forbidden_outer` and replaced by a space.
    #[inline(always)]
    pub(crate) fn raw_token_outer(&mut self) -> Token {
        let t = self.raw_token();
        if self.outer_scan.is_some() && self.is_outer_macro_token(t) {
            self.forbidden_outer(t)
        } else {
            t
        }
    }

    /// A token read the way tex.web does with scanner_status:=normal
    /// (\noexpand, \string, \meaning, \ifx, \ifdefined): an \outer macro is
    /// no problem, and the end of the input stops the job there without a
    /// report of its own.
    #[inline]
    fn raw_token_normal(&mut self) -> Token {
        let t = self.raw_token();
        if t == EOF_MARKER {
            self.eof_reported = true;
        }
        t
    }

    /// The error line of tex.web §338: `head` is `Forbidden control
    /// sequence found` or `File ended`.
    fn outer_scan_message(&self, head: &str) -> String {
        let (kind, owner) = self.outer_scan.unwrap_or((OuterScan::Text, None));
        let what = match kind {
            OuterScan::Definition => "definition",
            OuterScan::Text => "text",
            OuterScan::Preamble => "preamble",
        };
        match owner {
            Some(id) => format!("{head} while scanning {what} of {}", self.display_cs(id)),
            None => format!("{head} while scanning {what}"),
        }
    }

    /// tex.web §336-§339 check_outer_validity during an absorbing scan: the
    /// \outer token `outer` is reported and backed up behind the inserted
    /// `}` (`\cr}` for a preamble), and TeX reads a space in its place,
    /// which is returned as the current token.
    #[cold]
    #[inline(never)]
    pub(crate) fn forbidden_outer(&mut self, outer: Token) -> Token {
        let message = self.outer_scan_message("Forbidden control sequence found");
        self.error(&message);
        self.push_token(outer);
        self.insert_outer_recovery();
        let space = Token::space();
        self.set_cur_char(space);
        space
    }

    /// tex.web §339: the tokens that end the interrupted absorbing scan.
    fn insert_outer_recovery(&mut self) {
        self.push_token(Token::char(CAT_EGROUP, u32::from(b'}')));
        if self.outer_scan.is_some_and(|(kind, _)| kind == OuterScan::Preamble) {
            let cr = self.crcr_token();
            self.push_token(cr);
            self.align_brace_depth = -1_000_000;
        }
    }

    /// tex.web §336-§339 for the end of the input during an absorbing scan.
    /// TeX inserts the `}` (`\cr}`) that ends the scan, and turns to the
    /// terminal afterwards, where it stops; here the caller ends the scan
    /// and the end of the input is read again.
    #[cold]
    #[inline(never)]
    pub(crate) fn outer_scan_file_ended(&mut self, origin: Option<&crate::input::SourceMark>) {
        if !self.eof_reported {
            self.eof_reported = true;
            if self.eqtb.int_params[crate::prim::IntParam::SuppressOuterError.idx() as usize] == 0 {
                let message = self.outer_scan_message("File ended");
                self.error_at(&message, origin.map(crate::input::SourceMark::to_context));
            }
        }
        self.push_token(EOF_MARKER);
    }

    /// tex.web `expand` on a token that was just read: a macro, expandable
    /// primitive or undefined control sequence is expanded one step (even a
    /// \protected macro) and true is returned; any other token is left
    /// alone and false is returned.
    pub(crate) fn expand_token_once(&mut self, t: Token) -> bool {
        let invocation = if t.is_cs() && t.0 < NOEXP_FLAG {
            t.cs_id()
        } else if t.is_char() && t.cc() == CAT_ACTIVE {
            self.active_cs_id(t.chr())
        } else {
            return false;
        };
        let mut id = invocation;
        for _ in 0..1024 {
            match self.eqtb.get(id) {
                Some(Equiv::Alias(next)) => id = *next,
                _ => break,
            }
        }
        match self.eqtb.get(id).cloned() {
            Some(Equiv::Macro(m)) => {
                self.expand_macro(id, &m, invocation);
                true
            }
            Some(Equiv::Prim(p)) if self.is_expandable(p) => {
                if let Some(tok) = self.expand_prim(p, id) {
                    self.push_token(tok);
                }
                true
            }
            None => {
                self.undefined_cs_error(Token::from_cs(id));
                true
            }
            _ => false,
        }
    }

    pub fn skip_raw_spaces(&mut self) {
        loop {
            if let Some((segment, trace_depth)) = self.token_list_front() {
                let spaces = segment.iter().take_while(|t| t.0 >> 24 == 10).count();
                let rest = segment.len() - spaces;
                if spaces > 0 {
                    self.consume_token_list_front(spaces, trace_depth);
                }
                if rest > 0 {
                    return;
                }
            }
            let t = self.raw_token();
            if (t.0 >> 24) != 10 {
                self.push_token(t);
                return;
            }
        }
    }

    /// Scan a balanced group (the opening brace was already consumed).
    /// Returns tokens without the outer braces.
    pub fn scan_balanced_raw(&mut self, long: bool) -> Vec<Token> {
        let origin = self.current_token_source_mark();
        let mut out = Vec::new();
        if let Err(Unbalanced::Paragraph(par)) =
            self.scan_balanced_raw_collect(long, false, true, &mut out, origin.as_ref())
        {
            self.push_token(par);
            self.error_at(
                "Runaway argument / missing }",
                origin.as_ref().map(crate::input::SourceMark::to_context),
            );
        }
        out
    }

    fn scan_macro_balanced_arg(
        &mut self,
        id: CsId,
        long: bool,
        collect: bool,
        out: &mut Vec<Token>,
        origin: Option<&crate::input::SourceMark>,
    ) -> Result<(), ArgAbort> {
        self.diagnostic_trace_hold = self.diagnostic_trace_hold.saturating_add(1);
        let result = self.scan_balanced_raw_collect(long, true, collect, out, origin);
        self.diagnostic_trace_hold -= 1;
        match result {
            Ok(()) => Ok(()),
            Err(Unbalanced::Paragraph(par)) => Err(self.abort_paragraph(id, par, origin)),
            Err(Unbalanced::Outer(token)) => Err(self.abort_outer(id, token, origin)),
            Err(Unbalanced::Eof) => Err(self.abort_file_ended(id, origin)),
            Err(Unbalanced::Fatal) => Err(ArgAbort),
        }
    }

    /// Append the balanced text after an already consumed `{` to `out`
    /// (when `collect`), without the closing brace. A macro argument
    /// rejects \outer macros; general text reports them (`forbidden_outer`).
    fn scan_balanced_raw_collect(
        &mut self,
        long: bool,
        macro_arg: bool,
        collect: bool,
        out: &mut Vec<Token>,
        origin: Option<&crate::input::SourceMark>,
    ) -> Result<(), Unbalanced> {
        let mut depth = 1i32;
        let mut scanned = 0;
        loop {
            if self.pushed.is_empty()
                && (self.align_state == crate::align::PH_IDLE
                    || (!self.align_macro_arg && self.align_brace_depth >= depth))
            {
                if let Some(closed) =
                    self.take_balanced_run(long, collect, out, &mut depth, &mut scanned)
                {
                    if closed {
                        return Ok(());
                    }
                    continue;
                }
            }
            let raw = self.raw_token();
            let mut stored = self.unfreeze_input_token(raw);
            let t = self.unfreeze_unexpanded_token(stored);
            if t == EOF_MARKER {
                if macro_arg {
                    return Err(Unbalanced::Eof);
                }
                if self.outer_scan.is_some() {
                    // The `}` that tex.web inserts closes every open group of
                    // the text here.
                    self.outer_scan_file_ended(origin);
                    return Ok(());
                }
                self.fatal_error_at(
                    "Runaway argument / missing }",
                    origin.map(crate::input::SourceMark::to_context),
                );
                return Err(Unbalanced::Fatal);
            }
            if t.is_char() && matches!(t.cc(), 1 | 2) {
                if t.cc() == 1 {
                    depth += 1;
                } else {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
            } else if !long && !self.suppress_long_error() && self.is_partoken(t) {
                return Err(Unbalanced::Paragraph(stored));
            } else if macro_arg {
                if self.is_outer_token(raw) {
                    return Err(Unbalanced::Outer(stored));
                }
            } else if self.outer_scan.is_some() && self.is_outer_macro_token(raw) {
                stored = self.forbidden_outer(raw);
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
                return Err(Unbalanced::Fatal);
            }
            scanned += 1;
            if collect {
                out.push(stored);
            }
        }
    }

    /// Move the run of plain tokens at the front of the current token list
    /// into balanced text at brace depth `depth` in one step, as the
    /// token-by-token loop of `scan_balanced_raw_collect` would. Returns
    /// whether the run closed the text, or `None` when no token qualifies
    /// (input from a file, or a token that needs `raw_token`).
    fn take_balanced_run(
        &mut self,
        long: bool,
        collect: bool,
        out: &mut Vec<Token>,
        depth: &mut i32,
        scanned: &mut usize,
    ) -> Option<bool> {
        let partoken = self.partoken_id();
        let (segment, trace_depth) = self.token_list_front()?;
        let (mut length, mut after) = balanced_prefix(segment, long, partoken, *depth);
        // An \outer token needs the token-by-token path.
        if self.eqtb.has_outer_macros() {
            if let Some(outer) = segment[..length].iter().position(|&t| self.is_outer_token(t)) {
                (length, after) = balanced_prefix(&segment[..outer], long, partoken, *depth);
            }
        }
        let mut content = length - usize::from(after == 0);
        // The token-by-token loop reports an overlong text before its first
        // excess token.
        let room = crate::input::MAX_TOKEN_LIST_TOKENS - *scanned;
        if content > room {
            (length, after) = balanced_prefix(&segment[..room], long, partoken, *depth);
            content = length;
        }
        if length == 0 {
            return None;
        }
        if collect {
            out.extend_from_slice(&segment[..content]);
        }
        self.consume_token_list_front(length, trace_depth);
        // raw_token() counts every brace it fetches in align_brace_depth.
        self.align_brace_depth = self.align_brace_depth.saturating_add(after - *depth);
        *depth = after;
        *scanned += content;
        Some(after == 0)
    }

    /// Move the run of tokens at the front of the current token list that a
    /// \def body stores as they are (no macro parameter, \outer macro or
    /// token needing `raw_token`; for an `expanded` body only characters that
    /// do not expand) into `out` at brace depth `depth`, as the token loop of
    /// `collect_def_body` would. Returns whether the run ended with the
    /// body's closing brace, or `None` when no token qualifies.
    pub(crate) fn take_def_body_run(
        &mut self,
        out: &mut Vec<Token>,
        depth: &mut i32,
        expanded: bool,
    ) -> Option<bool> {
        if !self.pushed.is_empty()
            || (expanded && self.unexp_protect > 0)
            || !(self.align_state == crate::align::PH_IDLE
                || (!self.align_macro_arg && self.align_brace_depth >= *depth))
        {
            return None;
        }
        let (segment, trace_depth) = self.token_list_front()?;
        let outer = self.eqtb.has_outer_macros();
        // The token loop reports an overlong body before its first excess token.
        let room = crate::input::MAX_TOKEN_LIST_TOKENS.saturating_sub(out.len());
        let mut after = *depth;
        let mut length = 0;
        for &t in segment {
            if !plain_balanced_token(t, true, t)
                || t.0 >> 24 == 6
                || (expanded && (t.is_cs() || t.0 >> 24 == 13))
                || (t.is_cs()
                    && (self.cs.is_active(t.cs_id())
                        || self.is_macro_param(t)
                        || (outer && self.is_outer_macro_token(t))))
            {
                break;
            }
            let top = t.0 >> 24;
            if top == 2 && after == 1 {
                after = 0;
                length += 1;
                break;
            }
            if length == room {
                break;
            }
            match top {
                1 => after += 1,
                2 => after -= 1,
                _ => {}
            }
            length += 1;
        }
        if length == 0 {
            return None;
        }
        out.extend_from_slice(&segment[..length - usize::from(after == 0)]);
        let last = segment[length - 1];
        self.consume_token_list_front(length, trace_depth);
        if expanded {
            // get_token_from() leaves the last character current.
            self.no_expand_tok = None;
            self.unexpanded_parameter = false;
            self.set_cur_char(last);
        }
        self.align_brace_depth = self.align_brace_depth.saturating_add(after - *depth);
        *depth = after;
        Some(after == 0)
    }

    /// Append the run of character tokens at the front of the current token
    /// list that `\csname` takes one by one through `get_x_raw` (characters
    /// that are neither active, braces, alignment tabs nor ignored) to
    /// `name`, leaving the current token as the last of them would. Stops
    /// once the name exceeds the length `\csname` checks before each token.
    fn take_csname_run(&mut self, name: &mut Vec<u8>) -> bool {
        let Some((segment, trace_depth)) = self.token_list_front() else {
            return false;
        };
        let mut last = None;
        let mut length = 0;
        for &t in segment {
            if !matches!(t.0 >> 24, 0 | 3 | 5..=8 | 10..=12) || name.len() > 2000 {
                break;
            }
            if t.is_unicode_char() {
                t.append_character_bytes(name);
            } else {
                name.push(t.chr() as u8);
            }
            last = Some(t);
            length += 1;
        }
        let Some(last) = last else {
            return false;
        };
        self.consume_token_list_front(length, trace_depth);
        self.unexpanded_parameter = false;
        self.set_cur_char(last);
        true
    }

    /// The undelivered tokens of the current token list (or of the current
    /// segment of a macro replacement) and its macro-trace depth, when no
    /// token is pushed back and the input comes from a list.
    #[inline]
    fn token_list_front(&self) -> Option<(&[Token], u8)> {
        if !self.pushed.is_empty() {
            return None;
        }
        match self.input.stack.last() {
            Some(crate::input::Source::TokList {
                toks,
                pos,
                trace_depth,
                ..
            }) => Some((&toks[*pos..], *trace_depth)),
            Some(crate::input::Source::MacroFrame(frame)) => {
                Some((frame.segment(), frame.trace_depth))
            }
            _ => None,
        }
    }

    /// Consume `count` tokens of `token_list_front()` as `raw_token` would,
    /// apart from per-token category handling.
    #[inline]
    fn consume_token_list_front(&mut self, count: usize, trace_depth: u8) {
        match self.input.stack.last_mut() {
            Some(crate::input::Source::TokList { pos, .. }) => *pos += count,
            Some(crate::input::Source::MacroFrame(frame)) => frame.skip(count),
            _ => unreachable!(),
        }
        self.note_token_list_fetch(trace_depth);
    }

    /// tex.web §392-§397: scan an argument ended by the token list `delim`,
    /// appending it to `out`.
    fn scan_delimited(
        &mut self,
        id: CsId,
        delim: &[Token],
        long: bool,
        out: &mut Vec<Token>,
        origin: Option<&crate::input::SourceMark>,
    ) -> Result<(), ArgAbort> {
        let start = out.len();
        let mut matched = smallvec::SmallVec::<[Token; 8]>::new();
        let mut outer_abort = false;
        loop {
            if matched.is_empty() {
                let room = crate::input::MAX_TOKEN_LIST_TOKENS.saturating_sub(out.len() - start);
                if self.take_delimited_run(delim[0], long, out, room) {
                    continue;
                }
            }
            let mut raw = self.macro_arg_token();
            if self.is_outer_token(raw) {
                // tex.web §336-§339: the \outer token is read again after
                // the call; the call sees a space, then the inserted \par.
                self.report_outer_in_argument(id, raw, origin);
                raw = Token::space();
                outer_abort = true;
            }
            let stored = self.unfreeze_input_token(raw);
            let t = self.unfreeze_unexpanded_token(stored);
            if self.align_state & crate::align::PH_CLOSE != 0 && t == self.crcr_token() {
                // The frozen end of the alignment cell is \outer.
                return Err(self.abort_outer(id, t, origin));
            }
            if t == EOF_MARKER {
                return Err(self.abort_file_ended(id, origin));
            }
            if matched.is_empty() && !self.delim_eq(stored.unfreeze(), delim[0]) {
                // The common case: the token cannot start the delimiter.
                if !self.scanned_token_list_has_room(out.len() - start, 1, "macro parameter size", origin)
                {
                    return Err(ArgAbort);
                }
                out.push(stored);
            } else {
                matched.push(stored);
                // Keep the longest suffix of the recent tokens that is still
                // a prefix of the delimiter; the rest belongs to the argument.
                while !matched
                    .iter()
                    .enumerate()
                    .all(|(i, token)| self.delim_eq(token.unfreeze(), delim[i]))
                {
                    let rm = matched.remove(0);
                    if !self.scanned_token_list_has_room(
                        out.len() - start,
                        1,
                        "macro parameter size",
                        origin,
                    ) {
                        return Err(ArgAbort);
                    }
                    out.push(rm);
                }
                if matched.len() == delim.len() {
                    if let Some(last) = delim.last() {
                        if last.is_char() && last.cc() == 1 {
                            // tex.web §392 / @8063: When the parameter delimiter
                            // ends with `#{`, both the delimiter match and the
                            // subsequent macro body scan see a left brace. Only
                            // one should affect align_state, so TeX decrements
                            // align_state here.
                            self.align_brace_depth = self.align_brace_depth.saturating_sub(1);
                        }
                    }
                    Self::strip_outer_braces(out, start);
                    return Ok(());
                }
                if !matched.is_empty() {
                    continue;
                }
            }
            // tex.web §392 matches the delimiter before §396 rejects an
            // illegal paragraph. A non-long #1\par parameter may therefore
            // use the paragraph token as its terminator.
            if (!long && !self.suppress_long_error() || outer_abort) && self.is_partoken(t) {
                out.pop();
                if outer_abort {
                    // tex.web §396: the call was ended by an \outer macro,
                    // which has been reported already.
                    return Err(ArgAbort);
                }
                return Err(self.abort_paragraph(id, stored, origin));
            }
            if t.is_char() && t.cc() == 2 {
                out.pop();
                return Err(self.abort_extra_brace(id, stored, origin));
            }
            if t.is_char() && t.cc() == 1 {
                // Delimiters cannot contain an unmatched opening brace other
                // than the single-token #{ case, which returned above. The
                // brace itself was already stored.
                let before = out.len();
                self.scan_macro_balanced_arg(id, long, true, out, origin)?;
                let inner = out.len() - before;
                if !self.scanned_token_list_has_room(
                    before - start,
                    inner.saturating_add(1),
                    "macro parameter size",
                    origin,
                ) {
                    return Err(ArgAbort);
                }
                out.push(Token::char(2, b'}' as u32));
            }
        }
    }

    /// Move the run of tokens at the front of the current token list that
    /// can neither start the delimiter (whose first token is `first`) nor
    /// end the argument into `out` (at most `room` tokens), as the
    /// token-by-token loop of `scan_delimited` would. False when no token
    /// qualifies.
    fn take_delimited_run(
        &mut self,
        first: Token,
        long: bool,
        out: &mut Vec<Token>,
        room: usize,
    ) -> bool {
        // Inside an alignment, raw_token() may end the cell at a row
        // delimiter, and the frozen end of a cell aborts the argument.
        let align_live = self.align_state != crate::align::PH_IDLE && {
            if self.align_state & crate::align::PH_CLOSE != 0 {
                return false;
            }
            self.align_delimiter_live()
        };
        let partoken = Token::from_cs(self.partoken_id());
        let outer = self.eqtb.has_outer_macros();
        let Some((segment, trace_depth)) = self.token_list_front() else {
            return false;
        };
        let limit = segment.len().min(room);
        let mut length = 0;
        while length < limit {
            let t = segment[length];
            if !plain_balanced_token(t, long, partoken)
                || matches!(t.0 >> 24, 1 | 2)
                // unfreeze_input_token() turns these into active characters.
                || (t.is_cs() && self.cs.is_active(t.cs_id()))
                || self.delim_eq(t, first)
                || (outer && self.is_outer_macro_token(t))
                || (align_live && crate::align::row_delimiter(t, &self.eqtb).is_some())
            {
                break;
            }
            length += 1;
        }
        if length == 0 {
            return false;
        }
        out.extend_from_slice(&segment[..length]);
        self.consume_token_list_front(length, trace_depth);
        true
    }

    /// tex.web §400: a delimited argument that is exactly one group loses
    /// its outer braces. `start` is where the argument begins in `tokens`.
    fn strip_outer_braces(tokens: &mut Vec<Token>, start: usize) {
        let arg = &tokens[start..];
        if arg.len() < 2 || !arg[0].is_left_brace() {
            return;
        }
        let mut depth = 0i32;
        for (i, t) in arg.iter().enumerate() {
            if t.is_left_brace() {
                depth += 1;
            } else if t.is_right_brace() {
                depth -= 1;
                if depth == 0 && i != arg.len() - 1 {
                    return;
                }
            }
        }
        if depth == 0 {
            tokens.pop();
            tokens.remove(start);
        }
    }

    // ---------- helpers ----------

    /// put a byte string into the expansion stream; spaces become cat-10
    /// spacer tokens (tex.web str_toks), everything else cat-12
    /// (chronologically on top: newer than any earlier pushback). The
    /// Unicode engines decode UTF-8 (xetex.web and luatex textoken.c
    /// `str_toks`), so `\detokenize{–}` is one character token there.
    pub fn exp_string(&mut self, bytes: &[u8]) {
        self.exp_string_inner(bytes);
    }

    /// `\string` of the control sequence `id` (escape character `esc`)
    /// built straight into a pooled token list. False, with nothing done,
    /// for the names that need `exp_string`'s general path: active
    /// characters and, in the Unicode engines, non-ASCII text.
    fn exp_cs_name_string(&mut self, id: CsId, esc: i32) -> bool {
        let name = self.cs.name(id);
        let escape = u8::try_from(esc).ok();
        if self.engine_kind != crate::engine::EngineKind::PdfTeX
            && !(name.is_ascii() && escape.is_none_or(|e| e.is_ascii()))
        {
            return false;
        }
        if Self::active_cs_source_bytes(name).is_some() {
            return false;
        }
        let mut toks = self.token_vec_pool.pop().unwrap_or_default();
        let string_token = |b: u8| if b == b' ' { Token::space() } else { Token::other(b) };
        if let Some(e) = escape {
            toks.push(string_token(e));
        }
        toks.extend(self.cs.name(id).iter().map(|&b| string_token(b)));
        self.push_tokens_named(toks, "<inserted>");
        true
    }

    /// `exp_string` of the decimal digits of `n` (tex.web print_int),
    /// formatted without a heap string.
    pub(crate) fn exp_int(&mut self, n: i64) {
        let mut buf = [0u8; 20];
        let mut at = buf.len();
        let mut m = n.unsigned_abs();
        loop {
            at -= 1;
            buf[at] = b'0' + (m % 10) as u8;
            m /= 10;
            if m == 0 {
                break;
            }
        }
        if n < 0 {
            at -= 1;
            buf[at] = b'-';
        }
        self.exp_string_inner(&buf[at..]);
    }

    #[inline(always)]
    fn exp_string_inner(&mut self, bytes: &[u8]) {
        if self.engine_kind != crate::engine::EngineKind::PdfTeX && !bytes.is_ascii() {
            self.exp_string_scalars(bytes);
            return;
        }
        if bytes.is_empty() {
            return;
        }
        // A recycled buffer: the list is popped (and its buffer returned to
        // the pool) as soon as it is read, so these lists never allocate.
        let mut toks = self.token_vec_pool.pop().unwrap_or_default();
        toks.extend(bytes.iter().map(|&b| {
            if b == b' ' {
                Token::space()
            } else {
                Token::other(b)
            }
        }));
        self.push_tokens_named(toks, "<inserted>");
    }

    /// xetex.web / luatex `str_toks`: UTF-8 text becomes one token per scalar
    /// value (a byte outside any UTF-8 sequence stands for the character of
    /// that code).
    #[inline(never)]
    fn exp_string_scalars(&mut self, bytes: &[u8]) {
        let mut toks: Vec<Token> = Vec::with_capacity(bytes.len());
        let mut rest = bytes;
        while !rest.is_empty() {
            let (good, tail) = match std::str::from_utf8(rest) {
                Ok(s) => (s, &rest[rest.len()..]),
                Err(e) => {
                    let (good, tail) = rest.split_at(e.valid_up_to());
                    (std::str::from_utf8(good).unwrap_or(""), tail)
                }
            };
            for c in good.chars() {
                toks.push(match c {
                    ' ' => Token::space(),
                    c if (c as u32) < 128 => Token::other(c as u8),
                    c => Token::unicode_char(12, c as u32),
                });
            }
            let Some((&stray, tail)) = tail.split_first() else { break };
            toks.push(Token::unicode_char(12, u32::from(stray)));
            rest = tail;
        }
        self.push_tokens_named(toks, "<inserted>");
    }

    pub fn tokens_to_string(&self, toks: &[Token]) -> String {
        String::from_utf8_lossy(&self.tokens_to_bytes(toks)).into_owned()
    }

    /// luatex `tokenlist_to_cstring(p, inhibit_par = 1, ..)`: the text that
    /// `\directlua`, `\latelua`, `\patterns` and `\hyphenation` pass on.
    /// Same as `tokens_to_string`, except that `\par` tokens (including
    /// those from blank lines) are left out.
    pub(crate) fn tokens_to_lua_text(&self, toks: &[Token]) -> String {
        let kept: Vec<Token> = toks.iter().copied().filter(|t| !self.is_partoken(*t)).collect();
        self.tokens_to_string(&kept)
    }

    /// `tokens_to_string` that keeps every byte (see `tex_bytes`).
    pub(crate) fn tokens_to_text(&self, toks: &[Token]) -> String {
        crate::tex_bytes::bytes_to_text(&self.tokens_to_bytes(toks))
    }

    pub(crate) fn tokens_to_bytes(&self, toks: &[Token]) -> Vec<u8> {
        let esc = self.eqtb.int_params[crate::prim::IntParam::EscapeChar.idx() as usize];
        self.tokens_to_bytes_esc(toks, esc)
    }

    /// `show_token_list` text with an explicit escape character.
    pub(crate) fn tokens_to_bytes_esc(&self, toks: &[Token], esc: i32) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
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
                // is followed by a space. The Unicode engines store a
                // single-character name as one UTF-8 scalar.
                let trailing_space = match name {
                    [] => false,
                    [c] => self.eqtb.cat[*c as usize] == crate::token::CAT_LETTER,
                    _ if self.engine_kind != crate::engine::EngineKind::PdfTeX && name.len() <= 4 => {
                        match std::str::from_utf8(name).ok().and_then(|s| {
                            let mut chars = s.chars();
                            let first = chars.next()?;
                            chars.next().is_none().then_some(u32::from(first))
                        }) {
                            Some(c) => self.eqtb.cat_code(c) == crate::token::CAT_LETTER,
                            None => true,
                        }
                    }
                    _ => true,
                };
                if trailing_space {
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
        match (self.eqtb.resolve(a.cs_id()), self.eqtb.resolve(b.cs_id())) {
            (None, None) => true,
            (Some(Equiv::Macro(x)), Some(Equiv::Macro(y))) => {
                std::rc::Rc::ptr_eq(x, y)
                    || (x.num_params == y.num_params
                        && x.params == y.params
                        && x.body == y.body
                        && x.long == y.long
                        && x.outer == y.outer
                        && x.protected == y.protected)
            }
            (Some(Equiv::Prim(x)), Some(Equiv::Prim(y))) => x == y,
            (Some(Equiv::CharTok(v)), Some(Equiv::CharTok(w))) => v == w,
            (Some(x), Some(y)) => match (x, y) {
                (Equiv::CharDef(v1), Equiv::CharDef(v2)) => v1 == v2,
                (Equiv::MathCharDef(v1), Equiv::MathCharDef(v2)) => v1 == v2,
                (Equiv::UMathCharDef(v1), Equiv::UMathCharDef(v2)) => v1 == v2,
                (Equiv::FontRef(v1), Equiv::FontRef(v2)) => v1 == v2,
                (Equiv::CountReg(v1), Equiv::CountReg(v2)) => v1 == v2,
                (Equiv::AttributeReg(v1), Equiv::AttributeReg(v2)) => v1 == v2,
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
        self.flush_trace_events();
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

/// pdfTeX's `\pdfcreationdate` (web2c `initstarttime`): SOURCE_DATE_EPOCH
/// (or the sandbox epoch) selects that instant in UTC, written with `Z`;
/// otherwise the local time is used.
pub(crate) fn pdf_creation_date() -> String {
    let epoch = tex_kpse::fs::epoch()
        .and_then(|epoch| i64::try_from(epoch).ok())
        .or_else(|| {
            std::env::var("SOURCE_DATE_EPOCH")
                .ok()
                .and_then(|value| value.trim().parse().ok())
        });
    match epoch {
        Some(epoch) => crate::clock::pdf_date(epoch, true),
        None => crate::clock::pdf_date(crate::clock::now(), false),
    }
}

/// pdfTeX's `\pdffilemoddate` (web2c `getfilemoddate`): the file's mtime in
/// local time, or in UTC when FORCE_SOURCE_DATE=1 pins TeX's clock to
/// SOURCE_DATE_EPOCH (and in the sandbox, whose clock is its epoch). Inputs
/// without a file-system timestamp (the embedded package archive and
/// built-in compatibility inputs) report the Unix epoch in UTC,
/// `D:19700101000000Z`, so date comparisons treat them as older than any
/// user file and the answer never varies with the host.
fn pdf_file_mod_date(modified: Option<i64>) -> String {
    match modified {
        Some(epoch) => crate::clock::pdf_date(
            epoch,
            tex_kpse::fs::epoch().is_some() || crate::clock::forced_source_date_epoch().is_some(),
        ),
        None => crate::clock::pdf_date(0, true),
    }
}

/// `\pdffilemoddate` of a file on disk; `None` unless it is a regular file
/// with a modification time. The result cache revalidates its records of
/// the primitive with this.
pub fn disk_file_mod_date(path: &std::path::Path) -> Option<String> {
    let modified = tex_kpse::fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())?
        .modified()
        .ok()?;
    Some(pdf_file_mod_date(Some(crate::clock::system_time_epoch(modified))))
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

/// The expandable primitives (`Engine::is_expandable`). `U` and
/// `XeTeXQuery` are members for some payloads only; the caller tests those.
static EXPANDABLE_PRIMS: crate::prim::PrimSet = {
    use Prim::*;
    crate::prim::PrimSet::of(&[
        ExpandAfter,
        NoExpand,
        CsName,
        LastNamedCs,
        The,
        String,
        Meaning,
        Number,
        RomanNumeral,
        Detokenize,
        ScanTokens,
        DirectLua,
        LuaFunction,
        LuaBytecode,
        Input,
        EndInput,
        Expanded,
        UnExpanded,
        JobName,
        FontName,
        FontIdPrim,
        IfChar,
        IfCat,
        IfOdd,
        IfNum,
        IfDim,
        IfVoid,
        IfHBox,
        IfVBox,
        IfHMode,
        IfVMode,
        IfInner,
        IfMMode,
        IfTrue,
        IfFalse,
        IfEOF,
        IfDef,
        IfCSName,
        IfInCsName,
        IfX,
        IfFontChar,
        IfPdfAbsNum,
        IfPdfAbsDim,
        IfPdfPrimitive,
        PdfPrimitive,
        PdfInsertHt,
        IfCase,
        Or,
        Else,
        ElIf,
        ElIfX,
        Fi,
        Unless,
        PdfFileSize,
        PdfMdFiveSum,
        PdfFileModDate,
        PdfCreationDate,
        PdfFileDump,
        PdfStrCmp,
        PdfUniformDeviate,
        PdfNormalDeviate,
        PdfEscapeString,
        PdfEscapeName,
        PdfEscapeHex,
        PdfUnescapeHex,
        PdfTexRevision,
        EtxRevision,
        PdfColorStackInit,
        PdfBanner,
        PdfFontSize,
        PdfPageRef,
        PdfFontName,
        PdfFontObjNum,
        PdfXFormName,
        PdfXImageBBox,
        LeftMarginKern,
        RightMarginKern,
        UcharCat,
        XeTeXUchar,
        FileSize,
        PdfMatch,
        PdfLastMatch,
        TopMark,
        FirstMark,
        BotMark,
        SplitFirstMark,
        SplitBotMark,
        TopMarksClass,
        FirstMarksClass,
        BotMarksClass,
        SplitFirstMarksClass,
        SplitBotMarksClass,
        XeTeXRevision,
        XeTeXGlyphName,
        XeTeXFeatureName,
        XeTeXVariationName,
        XeTeXQuery(crate::xetex_query::XeQuery::SelectorName),
        LuaTeXRevision,
        LuaTeXBanner,
        PdfVariable,
        PdfFeedback,
        DviVariable,
        DviFeedback,
        EtxVersionString,
        CsString,
        BeginCsName,
        FormatName,
        LuaEscapeString,
        U(crate::uprim::UPrim::UChar),
    ])
};

/// The conditionals that open an `\if...\fi` (`Engine::is_if_test`).
static IF_TESTS: crate::prim::PrimSet = {
    use Prim::*;
    crate::prim::PrimSet::of(&[
        IfChar,
        IfCat,
        IfOdd,
        IfNum,
        IfDim,
        IfVoid,
        IfHBox,
        IfVBox,
        IfHMode,
        IfVMode,
        IfInner,
        IfMMode,
        IfTrue,
        IfFalse,
        IfEOF,
        IfDef,
        IfCSName,
        IfInCsName,
        IfX,
        IfCase,
        IfFontChar,
        IfPdfAbsNum,
        IfPdfAbsDim,
        IfPdfPrimitive,
    ])
};

/// `\fi`, `\else`, `\or` and the unsupported `\elseif` forms: the
/// delimiters `pass_text` stops at.
static IF_DELIMITERS: crate::prim::PrimSet = {
    use Prim::*;
    crate::prim::PrimSet::of(&[Fi, Else, Or, ElIf, ElIfX])
};

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
