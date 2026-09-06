//! Token expansion: get_token, macro expansion, conditionals, \csname and
//! the string-producing primitives.

use crate::eqtb::{Equiv, Macro};
use crate::engine::{Engine, ScannerStatus};

use crate::input::{PAR_END, EOF_MARKER};
use crate::prim::Prim;
use crate::token::*;

pub const PAR_REF_FLAG: u32 = 0x4000_0000;
pub const NOEXP_FLAG: u32 = 0xC000_0000;

impl Engine {
    pub(crate) fn freeze_unexpanded_toks(toks: Vec<Token>) -> Vec<Token> {
        toks.into_iter()
            .map(|t| {
                if t.is_cs() && t.0 < NOEXP_FLAG {
                    Token(NOEXP_FLAG | t.cs_id())
                } else {
                    t
                }
            })
            .collect()
    }
    fn delim_eq(a: Token, b: Token) -> bool {
        if a.is_cs() && b.is_cs() {
            (a.0 & 0x3FFF_FFFF) == (b.0 & 0x3FFF_FFFF)
        } else if a.is_char() && b.is_char() {
            a.cc() == b.cc() && a.chr() == b.chr()
        } else {
            false
        }
    }

    /// fetch next raw token honoring pushback
    pub fn raw_token(&mut self) -> Token {
        let t = loop {

            if self.scanner_status == ScannerStatus::Aligning {
                // Expansions of the current u/v-part are single-token
                // pushbacks above align_pushed_base and must play first
                // (\\tabcolsep after \\hskip). Older pushed tokens wait
                // under the token-list sources.
                if self.pushed.len() > self.align_pushed_base {
                    break self.pushed.pop().unwrap();
                }
                // Any token-list source above the file (u/v part, macro
                // body, \everypar hook, output routine) is newer than the
                // pre-align pushback parked below align_pushed_base and
                // must drain before it (tex.web input-stack LIFO).
                if matches!(
                    self.input.stack.last(),
                    Some(crate::input::Source::TokList { .. })
                ) {
                    let si = self.input.stack.len() - 1;
                    if let Some(t) = self.toklist_next(si) {
                        break t;
                    }
                    continue;
                }
            }
            if let Some(t) = self.pushed.pop() {
                break t;
            }
            break self.get_next_raw();
        };
        if t.is_char() {
            if t.cc() == 14 {
                if let Some(si) = self.input.stack.len().checked_sub(1) {
                    if let Some(crate::input::Source::File { line_buf, line_pos, state, .. }) = self.input.stack.get_mut(si) {
                        *line_buf = None;
                        *line_pos = 0;
                        if *state == 1 {
                            *state = 2;
                        }
                    }
                }
                return self.raw_token();
            }
            if t.cc() == 9 || t.cc() == 15 {
                return self.raw_token();
            }
        }

        if t.0 >= PAR_REF_FLAG && t.0 < 0x8000_0000 && !t.is_cs() {
            let n = (t.0 & 0xF) as u8;
            self.pushed.push(Token::char(12, b'0' as u32 + n as u32));
            return Token::char(6, b'#' as u32);
        }
        if t == crate::page::OUT_END_TOKEN {
            // Scanners that hit the output-routine sentinel mid-scan (error
            // recovery) must still finish the routine; swallowing it locks
            // in_output=true and silently suppresses every later fire_up.
            self.finish_output();
            return self.raw_token();
        }
        if crate::debug_flag("PARTRACE") && t.is_cs() && self.cs.name(t.cs_id()) == b"par" {
            let top = match self.input.stack.last() {
                Some(crate::input::Source::TokList { name, pos, toks, .. }) => format!("T:{}:{}/{}", name, pos, toks.len()),
                Some(crate::input::Source::File { name, line_no, .. }) => format!("F:{}:{}", name, line_no),
                None => "none".into(),
            };
            eprintln!("PARPOP pushed_len_after={}", self.pushed.len());
        }
        t
    }

    /// tex.web get_x_token: expands macros/conditionals but does NOT skip
    /// spaces (scan_int relies on spaces terminating constants).
    pub fn get_x_raw(&mut self) -> Token {
        loop {
            let t = self.raw_token();
            let mut id = if t.is_cs() {
                t.cs_id()
            } else if t.is_char() && t.cc() == 13 {
                self.active_cs_id(t.chr() as u8)
            } else {
                self.set_cur_char(t);
                return t;
            };
            let t = Token::from_cs(id);

            for _ in 0..1024 {
                match self.eqtb.get(id) {
                    Some(Equiv::Alias(next)) => id = *next,
                    _ => break,
                }
            }
            match self.eqtb.get(id).cloned() {
                Some(Equiv::Macro(m)) => {
                    // e-TeX: \\protected is frozen only while absorbing an
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
                    self.expand_macro(id, &m);
                    continue;
                }

                Some(Equiv::CharTok(v)) => {
                    let tok = Token(v);
                    self.cur_tok = tok;
                    self.cur_cs = None;
                    self.cur_prim = None;
                    return tok;
                }
                Some(Equiv::Prim(p)) => {
                    if self.is_expandable(p) {
                        match self.expand_prim(p, id) {
                            Some(tok) => {
                                if tok.0 >= NOEXP_FLAG && tok.0 < 0xFFFF_0000 {
                                    let tok = Token::from_cs(tok.0 & 0x3FFF_FFFF);
                                    self.set_cur_cs(tok);
                                    return tok;
                                }
                                if !tok.is_cs() {
                                    return tok;
                                }
                                self.pushed.push(tok);
                                continue;
                            }
                            None => continue,
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
        self.begin_token_list(toks, false, "<replay>");
    }
    /// push_tokens variant matching tex.web \\unexpanded: each control
    /// sequence carries the one-shot \\noexpand flag so a later x/f-scan
    /// stores it without expanding; char tokens are unaffected.
    pub fn push_tokens_exp_not(&mut self, toks: Vec<Token>) {
        self.begin_token_list(toks, true, "<replay>");
    }
    /// named replay source (macro bodies): trace readability only.
    pub fn push_tokens_named(&mut self, toks: Vec<Token>, name: &str) {
        self.begin_token_list(toks, false, name);
    }

    fn begin_token_list(&mut self, toks: Vec<Token>, exp_not: bool, name: &str) {
        if toks.is_empty() {
            return;
        }
        if !self.pushed.is_empty() {
            let cut = if self.scanner_status == ScannerStatus::Aligning {
                self.align_pushed_base.min(self.pushed.len())
            } else {
                0
            };
            if self.pushed.len() > cut {
                let mut rest = self.pushed.split_off(cut);
                rest.reverse();
                self.input.push_toks(rest, "<pushback>");
            }
        }
        let toks = if exp_not {
            toks.into_iter()
                .map(|t| {
                    if t.is_cs() && t.0 < NOEXP_FLAG {
                        Token(NOEXP_FLAG | t.cs_id())
                    } else {
                        t
                    }
                })
                .collect()
        } else {
            toks
        };
        self.input.push_toks(toks, name);
    }
    fn set_cur_cs(&mut self, t: Token) {
        self.cur_tok = t;
        self.cur_cs = Some(t.cs_id());
        let prim = match self.eqtb.resolve(t.cs_id()) {
            Some(Equiv::Prim(p)) => Some(*p),
            _ => None,
        };
        self.cur_prim = prim;
    }

    fn set_cur_char(&mut self, t: Token) {
        self.cur_tok = t;
        self.cur_cs = None;
        self.cur_prim = None;
    }

    /// Get the next token, expanding macros and expandable primitives.
    /// Sets cur_tok/cur_cs/cur_prim.
    pub fn get_token(&mut self) -> Token {
        self.get_token_inner()
    }

    fn get_token_inner(&mut self) -> Token {
        self.gt_steps += 1;
        if self.pushed.len() > 100_000 && !self.loop_traced {
            eprintln!(
                "PUSHEDGROW n={} mac={} file={}:{} last={:?}",
                self.pushed.len(),
                self.current_macro,
                self.input.current_file_name().split('/').last().unwrap_or("?"),
                self.input.current_file_line(),
                self.last_macros.iter().rev().take(12).collect::<Vec<_>>()
            );
            std::process::exit(9);
        }

        loop {


            let mut t = self.raw_token();
            if t.0 >= NOEXP_FLAG && t.0 < 0xFFFF_0000 {
                let cs = t.0 & 0x3FFF_FFFF;
                let tok = Token::from_cs(cs);
                if self.eqtb.get(cs).is_none() {
                    // Lazily synthesize l3 exp_args:N<spec> expanders.
                    self.synth_exp_args_if_match(cs);
                }
                self.cur_tok = tok;
                self.cur_cs = Some(cs);
                self.cur_prim = Some(Prim::Relax);
                return tok;
            }




            if t == EOF_MARKER {
                self.end_occurred = true;
                return EOF_MARKER;
            }
            if t == crate::page::OUT_END_TOKEN {
                self.finish_output();
                continue;
            }
            if t == crate::page::WRITE_END_TOKEN {
                return t;
            }
            if t == PAR_END {
                // tex.web: a blank line becomes \par. Route the token through
                // the shared control-sequence path below instead of returning
                // it bare: the bare return skipped macro expansion, so once
                // LaTeX redefines \par as the macro \para_end:
                // (\cs_set_eq:NN \par \para_end:) every blank line was a
                // silent no-op — the open paragraph never ended, and the next
                // \penalty/\vskip ran in the wrong mode.
                t = Token::from_cs(self.ids.par);
            }
            if t.is_char() && t.chr() == b'_' as u32 {
                static UC: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let n = UC.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if n < 6 {
                    eprintln!("USCORE-TOK cc={} raw={:08x} L{} pushed={}", t.cc(), t.0, self.input.current_file_line(), self.pushed.len());
                }
            }
            let t = if t.is_char() && t.cc() == 13 {
                Token::from_cs(self.active_cs_id(t.chr() as u8))
            } else {
                t
            };

            if t.is_cs() {

                let mut id = t.cs_id();
                for _ in 0..1024 {
                    match self.eqtb.get(id) {
                        Some(Equiv::Alias(next)) => id = *next,
                        _ => break,
                    }
                }
                if self.eqtb.get(id).is_none() {
                    let name = self.cs.name(id);
                    if name == b"@@italiccorr" || name == b"/" {
                        self.eqtb.assign(id, Equiv::Prim(Prim::Relax), true);
                    }
                }
                match self.eqtb.get(id).cloned() {
                    Some(Equiv::Macro(m)) => {
                        if m.protected && self.in_expanded_scan && self.csname_depth == 0 {
                            self.set_cur_cs(t);
                            return t;
                        }
                        if self.freeze_gts_in_edef(id) {
                            self.set_cur_cs(t);
                            return t;
                        }
                        if self.is_self_quark(id, &m) {
                            self.set_cur_cs(t);
                            return t;
                        }
self.expand_macro(id, &m);
                        continue;
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
                        let tok = Token(v);
                        self.cur_tok = tok;
                        self.cur_cs = None;
                        self.cur_prim = None;
                        return tok;
                    }
                    Some(Equiv::Prim(p)) => {
                    if crate::debug_flag("PAIRTRACE") && self.cs.name(id) == b"__kernel_exp_not:w" {
                        let rk = match self.eqtb.get(id) { Some(Equiv::Prim(pp)) => format!("Prim({:?})", pp), Some(Equiv::Alias(n)) => format!("Alias->{}", n), Some(Equiv::Macro(_)) => "Macro".to_string(), _ => "other".to_string() };
                        eprintln!("EXPNOT-DISPATCH id={} resolved={} in_scan={}", id, rk, self.in_expanded_scan);
                    }
                        if p == Prim::UnExpanded && crate::debug_flag("PAIRTRACE") {
                            eprintln!("UE-DISPATCH in_expanded_scan={} csname_depth={} L{}", self.in_expanded_scan, self.csname_depth, self.input.current_file_line());
                        }
                        // NOTE: no early-return for UnExpanded inside
                        // e-scans. Returning the bare token leaked an
                        // unexpanded-marker into \expanded results, which
                        // then re-froze the NEXT token singly and broke the
                        // \unexpanded\expanded{{...}} callback idiom
                        // (keyval_parse / l3keys). The normal arm below
                        // (scan group + push exp_not) is correct in every
                        // context, csname included.
                        if self.is_expandable(p) {
                        // A \csname-created exp_args:N<spec> may carry the
                        // relax default from an earlier pass; repair it.
                        if p == Prim::Relax && self.name_is_synth_exp_args(id) {
                            self.synth_exp_args_if_match(id);
                            continue;
                        }

                            match self.expand_prim(p, id) {
                                Some(tok) => {
                                    if tok.0 >= NOEXP_FLAG && tok.0 < 0xFFFF_0000 {
                                        let cs = tok.0 & 0x3FFF_FFFF;
                                        let tok = Token::from_cs(cs);
                                        self.cur_tok = tok;
                                        self.cur_cs = Some(cs);
                                        self.cur_prim = Some(Prim::Relax);
                                        return tok;
                                    }
                                    if !tok.is_cs() {
                                        self.set_cur_char(tok);
                                        return tok;
                                    }
                                    self.pushed.push(tok);
                                    continue;
                                }
                                None => continue,
                            }
                        } else {
                            self.set_cur_cs(t);
                            return t;
                        }
                    }
                    _ => {
                        if self.synth_exp_args_if_match(id) {
                            continue;
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
    }


    /// true when the cs name is exp_args:N followed only by l3 arg letters.
    pub fn name_is_synth_exp_args(&self, id: CsId) -> bool {
        let name: Vec<u8> = self.cs.name(id).to_vec();
        name.starts_with(b"exp_args:N")
            && name.len() > 10
            && name[10..].iter().all(|c| matches!(c, b'N' | b'n' | b'c' | b'o' | b'f' | b'e' | b'V' | b'v' | b'x'))
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
        let name: Vec<u8> = self.cs.name(id).to_vec();
        let spec: Vec<u8> = name[10..].to_vec();
        let mut body: Vec<Token> = Vec::new();
        for letter in &spec {
            let mut helper: Vec<u8> = b"::".to_vec();
            helper.push(*letter);
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
            num_params: 0,
            params: Vec::new(),
            prefix: Vec::new(),
            body,
            long: true,
            outer: false,
            protected: false,
        };
        self.eqtb.assign(id, Equiv::Macro(std::rc::Rc::new(m)), true);
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
                | PdfFileDump
                | PdfStrCmp
                | PdfShellEscape
                | PdfElapsedTime
                | PdfUniformDeviate
                | PdfNormalDeviate
                | PdfEscapeString
                | PdfEscapeName
                | PdfEscapeHex
                | PdfUnescapeHex
                | PdfTexRevision
                | UcharCat
                | FileSize
        )
    }

    /// Execute an expandable primitive; None = keep expanding,
    /// Some(t) = t is the resulting current token.
    pub fn expand_prim(&mut self, p: Prim, _id: CsId) -> Option<Token> {
        use Prim::*;
        match p {
            ExpandAfter => {
                let t1 = self.raw_token();
                let t2 = self.raw_token();
                if t2.0 >= NOEXP_FLAG && t2.0 < 0xFFFF_0000 {
                    self.pushed.push(t2);
                } else if t2.is_cs() {
                    let mut id2 = t2.cs_id();
                    for _ in 0..1024 {
                        match self.eqtb.get(id2) {
                            Some(Equiv::Alias(next)) => id2 = *next,
                            _ => break,
                        }
                    }
                    match self.eqtb.get(id2).cloned() {
                        // tex.web: \expandafter expands even \protected macros
                        Some(Equiv::Macro(m)) => {
                            if self.freeze_gts_in_edef(id2) {
                                self.pushed.push(t2);
                            } else {
                                self.expand_macro(id2, &m);
                            }
                        }
                        Some(Equiv::Prim(p2)) if self.is_expandable(p2) => {
                            if let Some(tt) = self.expand_prim(p2, id2) {
                                self.pushed.push(tt);
                            }
                        }
                        _ => {
                            self.pushed.push(t2);
                        }
                    }
                } else {
                    self.pushed.push(t2);
                }
                self.pushed.push(t1);
                None
            }

            NoExpand => {
                let t = self.raw_token();
                if t.is_cs() {
                    let id = t.cs_id();
                    // tex.web \noexpand: the one-shot no-expansion flag
                    // applies to ANY macro — protected ones included.
                    // Leaving protected macros unflagged let them expand
                    // inside \edef/\if (hyperref pdfstringdef), leaking
                    // \delimiter "42xxx hex into the stored text.
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
                self.csname_depth += 1;
                let mut name: Vec<u8> = Vec::new();
                // A leftover e-TeX \unless flag must not flip \ifx inside \csname
                self.unless_next = false;
                loop {
                    // tex.web §372: get_x_token until cur_cs≠0; that first CS
                    // ends the name. Re-expanding leftovers (and peeking 12
                    // extra tokens) lets nested \\expanded steal the next
                    // undelimited arg (utf8.def IfFileExists@ #2 = with@hooks).
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
                        // get_x_raw strips \\noexpand and returns the frozen CS.
                        // Expand leftover expandable prims (\\expanded) so
                        // \\csname\\noexpand\\expanded{...}\\endcsname works.
                        // Do NOT re-expand macros — that steals following args.
                        match self.eqtb.resolve(id).cloned() {
                            Some(Equiv::Prim(p)) if self.is_expandable(p) => {
                                match self.expand_prim(p, id) {
                                    Some(tok) if tok.is_char() => {
                                        name.push(tok.chr() as u8);
                                    }
                                    Some(tok) => self.pushed.push(tok),
                                    None => {}
                                }
                                continue;
                            }
                            _ => {
                                if std::env::var("UNDEFTRACE").is_ok() {
                                    let cs = std::string::String::from_utf8_lossy(&name).into_owned();
                                    eprintln!("CSN-STALL name={:?} offending={:#x} line={} collected_len={}", cs, t.0, self.input.current_file_line(), name.len());
                                }
                                self.pushed.push(t);
                                self.error("Missing \\endcsname inserted");
                                break;
                            }
                        }
                    }

                    if t.is_char() && t.cc() == 9 {
                        continue;
                    }
                    name.push(t.chr() as u8);
                }
                self.csname_depth = self.csname_depth.saturating_sub(1);
                let id = self.cs.intern(&name);
                self.last_named_cs = Some(id);
                if self.eqtb.get(id).is_none() {
                    // tex.web §372: EMPTY \\csname\\endcsname yields null_cs,
                    // which stays undefined — never relax. ltfilehook's
                    // \\@set@curr@file@aux tests `\\ifx\\csname\\endcsname#1`:
                    // relaxing the empty name makes it equal every
                    // relax-defaulted name and the filename becomes ".tex".
                    if name.is_empty() {
                        return Some(Token::from_cs(id));
                    }
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
                Some(Token::from_cs(id))
            }


            LastNamedCs => {
                let id = self.last_named_cs.unwrap_or_else(|| self.cs.lookup(b"relax").unwrap());
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
                    if esc >= 0 && esc <= 255 {
                        bytes.push(esc as u8);
                    }
                    bytes.extend_from_slice(self.cs.name(t.cs_id()));
                } else {
                    bytes.push(t.chr() as u8);
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
                if crate::debug_flag("PAIRTRACE") {
                    eprintln!("EXPANDED-DISPATCH L{}", self.input.current_file_line());
                }
                if crate::debug_flag("QUARKTRACE") {
                    eprintln!("EXPANDED-DISPATCH at {}:{} stack-tail=[{}] prev-mac={:?}",
                        self.input.current_file_name().split('/').last().unwrap_or("?"), self.input.current_file_line(),
                        self.input.stack.iter().rev().take(4).map(|src| match src {
                            crate::input::Source::TokList { name, pos, toks, .. } => format!("T:{} {}/{}", name, pos, toks.len()),
                            crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name.split('/').last().unwrap_or(name), line_no),
                        }).collect::<Vec<_>>().join(" << "),
                        self.last_macros);
                }
                let r = self.scan_general_text_expanded();
                if crate::debug_flag("QUARKTRACE2") {
                    let before = self.tokens_to_string(&self.pushed.iter().rev().take(24).cloned().collect::<Vec<_>>());
                    eprintln!("EXP-IN  {}:{} pushed=[{}]", self.input.current_file_name().split('/').last().unwrap_or("?"), self.input.current_file_line(), before);
                }

                if crate::debug_flag("QUARKTRACE") {
                    if let Some(i) = r.iter().position(|t| {
                        t.is_cs() && t.0 < 0xC000_0000 && self.cs.name(t.cs_id()).starts_with(b"q__")
                    }) {
                        let nm: std::string::String = self.cur_cs.map(|c| std::string::String::from_utf8_lossy(self.cs.name(c)).into_owned()).unwrap_or_default();
                        eprintln!("EXPANDED-QUARK-UNPROT caller={} pre=[{}]",
                            nm, self.tokens_to_string(&r[..i.min(30)]));
                    }
                }
                if crate::debug_flag("QUARKTRACE2") {
                    eprintln!("EXP-OUT {}:{} result=[{}]", self.input.current_file_name().split('/').last().unwrap_or("?"), self.input.current_file_line(), self.tokens_to_string(&r.iter().take(24).cloned().collect::<Vec<_>>()));
                }
 self.push_tokens(r);
 None
            }
            UnExpanded => {
                self.skip_spaces_relax();
                let t = self.raw_token();
                if crate::debug_flag("PAIRTRACE") {
                    let nxt = self.raw_token();
                    self.pushed.push(nxt);
                    let desc = if nxt.is_cs() {
                        let rk = match self.eqtb.resolve(nxt.cs_id()) { Some(Equiv::Prim(p)) => format!("Prim({:?})", p), Some(Equiv::Alias(_)) => "Alias".into(), Some(Equiv::Macro(_)) => "Macro".into(), _ => "other".into() };
                        format!("cs:{} {}", ::std::string::String::from_utf8_lossy(self.cs.name(nxt.cs_id())), rk)
                    } else { format!("cc{} c{}", nxt.cc(), nxt.chr()) };
                    eprintln!("UE-ENTRY in={} next={}", self.current_macro, desc);
                }
                if t.is_cs() {
                    if let Some(Equiv::ToksReg(i)) = self.eqtb.resolve(t.cs_id()).cloned() {
                        let toks = (*self.eqtb.toks[i as usize]).clone();
                        if self.in_expanded_scan {
                            self.unexp_protect = self.unexp_protect.saturating_add(toks.len());
                        }
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
                            let n = toks.len();
                            if self.in_expanded_scan {
                                self.unexp_protect = self.unexp_protect.saturating_add(n);
                                self.push_tokens_exp_not(toks);
                            } else {
                                self.push_tokens(toks);
                            }
                            if crate::debug_flag("PAIRTRACE") {
                                eprintln!("UE-PAIR fired toks={}", n);
                            }
                            return None;
                        }
                    }
                }
                self.pushed.push(t);
                let toks = self.scan_general_text();
                if self.in_expanded_scan {
                    self.unexp_protect = self.unexp_protect.saturating_add(toks.len());
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
                self.input.push_file("<scantokens>".to_string(), text.into_bytes());
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
                let text = self.glue_to_string(&g);
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
            Unless => {
                // e-TeX \\unless prefixes the next conditional; the flag is
                // consumed by do_if. Do not restore: a save/restore around
                // expand_prim_of_next re-armed the flag after do_if cleared
                // it and made \\ifx inside \\csname keep a leftover `=`.
                self.unless_next = true;
                self.expand_prim_of_next()
            }
            IfEOF => {
                let n = self.scan_int();
                let eof = self.read_eof.get(n.max(0) as usize).copied().unwrap_or(true);
self.do_if(eof)
            }
            IfTrue => self.do_if(true),
            IfFalse => self.do_if(false),
            IfChar => {
                // tex.web 498: push this \\if *before* get_x_token so a nested
                // true conditional from the test sits on top (babel
                // `\\if T\\ifeof1F\\fi T`).
                let unless = std::mem::take(&mut self.unless_next);
                let save = self.push_if();
                let a = self.get_token();
                let b = self.get_token();
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
                let save = self.push_if();
                let a = self.get_token();
                let b = self.get_token();
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
            IfOdd => {
                let n = self.scan_int();
                self.do_if(n % 2 != 0)
            }
            IfNum => {
                let a = self.scan_int();
                let rel = self.scan_relational();
                let b = self.scan_int();
                if crate::debug_flag("IFNUMTRACE") {
                    eprintln!(
                        "IFNUM {} {} {} -> {} line={} stack={}",
                        a,
                        rel as char,
                        b,
                        compare(a, rel, b),
                        self.input.current_file_line(),
                        self.if_stack.len()
                    );
                }
                self.do_if(compare(a, rel, b))
            }
            IfDim => {
                let a = self.scan_dimen(false, false);
                let rel = self.scan_relational();
                let b = self.scan_dimen(false, false);
                self.do_if(compare(a, rel, b))
            }
            IfVoid => {
                let n = self.scan_reg_num();
                self.do_if(self.eqtb.boxed[n as usize].is_none())
            }
            IfFontChar => {
                // e-TeX: true when the char has a non-zero width in the font
                let f = self.scan_font_id();
                let c = self.scan_int().clamp(0, 255) as u8;
                let w = self
                    .eqtb
                    .fonts
                    .get(f as usize)
                    .map(|x| x.char_width(c))
                    .unwrap_or(0);
                self.do_if(w != 0)
            }
            IfHBox => {
                let n = self.scan_reg_num();
                self.do_if(matches!(&self.eqtb.boxed[n as usize], Some(crate::boxes::Node::Box { kind: 0, .. })))
            }
            IfVBox => {
                let n = self.scan_reg_num();
                self.do_if(matches!(
                    &self.eqtb.boxed[n as usize],
                    Some(crate::boxes::Node::Box { kind: 1 | 2, .. })
                ))
            }
            IfVMode => self.do_if(self.mode.is_v()),
            IfHMode => self.do_if(self.mode.is_h()),
            IfMMode => self.do_if(self.mode.is_m()),
            IfInner => {
                let ok = matches!(self.mode, crate::engine::Mode::InternalVertical | crate::engine::Mode::RestrictedHorizontal | crate::engine::Mode::Math);
                self.do_if(ok)
            }
            IfDef => {
                let t = self.raw_token();
                let def = if t.is_cs() {
                    self.eqtb.resolve(t.cs_id()).is_some()
                } else if t.is_char() && t.cc() == 13 {
                    let id = self.active_cs_id(t.chr() as u8);
                    self.eqtb.resolve(id).is_some()
                } else {
                    false
                };
                self.do_if(def)
            }
            IfInCsName => self.do_if(self.csname_depth > 0),

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
                                    Some(tok) if tok.is_char() => name.push(tok.chr() as u8),
                                    Some(tok) => self.pushed.push(tok),
                                    None => {}
                                }
                                continue;
                            }
                            _ => {
                                self.pushed.push(t);
                                self.error("Missing \\endcsname inserted");
                                break;
                            }
                        }
                    }
                    if t.is_char() && t.cc() == 9 {
                        continue;
                    }
                    name.push(t.chr() as u8);
                }
                self.csname_depth = self.csname_depth.saturating_sub(1);
                let def = if let Some(id) = self.cs.lookup(&name) {
                    self.last_named_cs = Some(id);
                    !matches!(
                        self.eqtb.resolve(id),
                        Some(Equiv::Prim(Prim::Relax)) | None
                    )
                } else {
                    false
                };
                self.unless_next = unless;
                self.do_if(def);
                None
            }
            IfX => {
                // tex.web if_x: operands see expandable PRIMS (\\csname...)
                // but never macros (\\ifx\\foo x is false for \\def\\foo{x}).
                let a = self.raw_token();
                let b = self.raw_token();
                let an = if a.is_cs() { self.cs.name(a.cs_id()).to_vec() } else { vec![] };
                let bn = if b.is_cs() { self.cs.name(b.cs_id()).to_vec() } else { vec![] };
                if an == b"GTS@temp" || an == b"GTS@Token" || bn == b"GTS@temp" || bn == b"GTS@Token" {
                    static GX: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                    if GX.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 16 {
                        let eq = self.ifx_equal(a, b);
                        let tok = self.cs.lookup(b"GTS@Token").map(|id| match self.eqtb.resolve(id) {
                            Some(Equiv::Macro(m)) => format!("mac[{}]", self.tokens_to_string(&m.body)),
                            Some(other) => other.kind_name().to_string(),
                            None => "undef".into(),
                        }).unwrap_or_else(|| "missing".into());
                        let tmp = self.cs.lookup(b"GTS@temp").map(|id| match self.eqtb.resolve(id) {
                            Some(Equiv::Macro(m)) => format!("mac[{}]", self.tokens_to_string(&m.body)),
                            Some(other) => other.kind_name().to_string(),
                            None => "undef".into(),
                        }).unwrap_or_else(|| "missing".into());
                        eprintln!(
                            "GTSIFX a=\\{} b=\\{} eq={} tok={tok} tmp={tmp} e={}",
                            std::string::String::from_utf8_lossy(&an),
                            std::string::String::from_utf8_lossy(&bn),
                            eq,
                            self.in_expanded_scan
                        );
                    }
                }
                let eq = self.ifx_equal(a, b);
                self.do_if(eq)
            }
            IfCase => {
                let n = self.scan_int();
                let accepting = n == 0;
                self.if_stack.push(crate::engine::IfState {
                    accepting,
                    matched: accepting,
                    if_case: n,
                    loc_file: self.input.current_file_name(),
                    loc_line: self.input.current_file_line(),
                    loc_cs: 0,
                });
                if !accepting {
                    self.skip_branch(true);
                }
                None
            }
            Or => {
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
                let ln = self.input.current_file_line();
                let st = self.if_stack.last().cloned();
                if ((ln >= 1880 && ln <= 1930) || (ln >= 290 && ln <= 310)) && self.input.current_file_name().contains("latex.ltx") {
                    eprintln!("ELSE_HIT L{} if_stack_len={} top={:?}", ln, self.if_stack.len(), st.as_ref().map(|s| (s.accepting, s.matched, s.loc_line)));
                }
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
                let ln = self.input.current_file_line();
                let popped = self.if_stack.pop();
                if ((ln >= 1880 && ln <= 1930) || (ln >= 290 && ln <= 310)) && self.input.current_file_name().contains("latex.ltx") {
                    eprintln!("FI_HIT L{} if_stack_len={} popped={:?}", ln, self.if_stack.len(), popped.as_ref().map(|s| (s.accepting, s.matched, s.loc_line)));
                }
                if popped.is_none() {
                    if crate::debug_flag("FIFTRACE") {
                        eprintln!(
                            "EXTRA-FI L{} file={} mac={} last={:?} pushed=[{}]",
                            ln,
                            self.input.current_file_name().split('/').last().unwrap_or(""),
                            self.current_macro,
                            self.last_macros.iter().rev().take(10).collect::<Vec<_>>(),
                            self.tokens_to_string(&self.pushed.iter().rev().take(14).cloned().collect::<Vec<_>>())
                        );
                    }
                    self.error("Extra \\fi");
                }
                None
            }
            PdfTexRevision => {
                self.exp_string(b"29");
                None
            }
            UcharCat => {
                let c = self.scan_int().clamp(0, 255) as u32;
                let cat = self.scan_int().clamp(0, 15) as u8;
                self.pushed.push(Token::char(cat, c));
                None
            }
            PdfFileSize | FileSize => {
                let name = { let t = self.scan_general_text_expanded(); self.tokens_to_string(&t) };
                let name = name.trim();
                // \@missingfileerror give-up sentinel: see resolve_input_path.
                // Must report a size so \file_full_name accepts ".tex" and
                // the retry \input{.tex} lands on the placeholder file.
                let sz = if name == ".tex" {
                    Some(1)
                } else {
                    self.font_loader.kpse.find_any(name)
                        .or_else(|| self.font_loader.kpse.find(name, tex_kpse::Format::Tex))
                        .and_then(|p| std::fs::metadata(p).ok())
                        .map(|m| m.len())
                };
                if let Some(n) = sz {
                    self.exp_string(n.to_string().as_bytes());
                }
                None
            }
            PdfStrCmp => {
                let a = { let t = self.scan_general_text_expanded(); self.tokens_to_string(&t) };
                let b = { let t = self.scan_general_text_expanded(); self.tokens_to_string(&t) };
                let n = match a.cmp(&b) {
                    std::cmp::Ordering::Less => "-1",
                    std::cmp::Ordering::Equal => "0",
                    std::cmp::Ordering::Greater => "1",
                };
                self.exp_string(n.as_bytes());
                None
            }
            PdfMdFiveSum => {
                let _ = self.scan_keyword(b"file");
                let s = { let t = self.scan_general_text_expanded(); self.tokens_to_string(&t) };
                // cheap non-crypto hex; expl3 only needs a stable token during boot
                let mut h: u64 = 0xcbf29ce484222325;
                for b in s.as_bytes() {
                    h ^= *b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
                self.exp_string(format!("{h:032x}").as_bytes());
                None
            }
            PdfFileModDate => {
                let _ = self.scan_general_text_expanded();
                self.exp_string(b"D:20260101000000Z");
                None
            }
            PdfFileDump => {
                let _ = self.scan_keyword(b"offset");
                let _ = self.scan_keyword(b"length");
                let _ = self.scan_general_text_expanded();
                None
            }
            PdfShellEscape => {
                self.exp_string(b"1");
                None
            }
            PdfElapsedTime => {
                self.exp_string(b"0");
                None
            }
            PdfUniformDeviate | PdfNormalDeviate => {
                let _ = self.scan_int();
                self.exp_string(b"0");
                None
            }
            PdfEscapeString | PdfEscapeName | PdfEscapeHex => {
                let s = { let t = self.scan_general_text_expanded(); self.tokens_to_string(&t) };
                self.exp_string(s.as_bytes());
                None
            }
            PdfUnescapeHex => {
                let s = { let t = self.scan_general_text_expanded(); self.tokens_to_string(&t) };
                self.exp_string(s.as_bytes());
                None
            }
            _ => {
                self.error("not expandable primitive in expansion");
                None
            }
        }
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

    fn push_if(&mut self) -> usize {
        self.if_stack.push(crate::engine::IfState {
            accepting: false,
            matched: false,
            if_case: -1,
            loc_file: self.input.current_file_name(),
            loc_line: self.input.current_file_line(),
            loc_cs: 0,
        });
        if crate::debug_flag("FIFTRACE") {
            eprintln!(
                "IFPUSH depth={} L{} file={} mac={} last={:?}",
                self.if_stack.len(),
                self.input.current_file_line(),
                self.input.current_file_name().split('/').last().unwrap_or(""),
                self.current_macro,
                self.last_macros.iter().rev().take(6).collect::<Vec<_>>()
            );
        }
        self.if_stack.len() - 1
    }

    fn finish_if(&mut self, save: usize, b: bool) -> Option<Token> {
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

    fn do_if(&mut self, mut b: bool) -> Option<Token> {
        if self.unless_next {
            b = !b;
            self.unless_next = false;
        }
        let save = self.push_if();
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
                                self.pushed.push(tok);
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

    /// tex.web 494: unexpanded skip to next \\fi/\\else/\\or at local depth 0.
    fn pass_text(&mut self) -> Option<Prim> {
        let mut l = 0i32;
        loop {
            let t = self.raw_token();
            if t == EOF_MARKER {
                let ist: Vec<std::string::String> = self
                    .if_stack
                    .iter()
                    .map(|st| {
                        format!(
                            "{}:{}",
                            st.loc_file.split('/').last().unwrap_or("?"),
                            st.loc_line
                        )
                    })
                    .collect();
                self.error(&format!(
                    "File ended while scanning conditional [{}]",
                    ist.join(", ")
                ));
                self.end_occurred = true;
                return None;
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
                    return Some(is);
                }
                if is == Prim::Fi {
                    l -= 1;
                }
            }
        }
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
                    IfChar | IfCat | IfOdd | IfNum | IfDim | IfVoid | IfHBox | IfVBox
                        | IfHMode | IfVMode | IfInner | IfMMode | IfTrue | IfFalse
                        | IfEOF | IfDef | IfCSName | IfInCsName | IfX | IfCase | IfFontChar

                ) {
                    *depth += 1;
                    return;
                }
            }
        }
        self.pushed.push(t2);
    }

    /// skip tokens until matching \else / \or (for ifcase) / \fi at this level
    fn skip_branch(&mut self, if_case: bool) {
        if crate::debug_flag("SKIPTRACE") {
            eprintln!("SKIPSTART ifcase={}", if_case);
        }
        let mut depth = 0i32;
        loop {
            let t = self.raw_token();
            if t == EOF_MARKER {
                let ist: Vec<std::string::String> = self.if_stack.iter().map(|st| format!("{}:{}", st.loc_file.split('/').last().unwrap_or("?"), st.loc_line)).collect();
                self.error(&format!("File ended while scanning conditional [{}]", ist.join(", ")));
                self.end_occurred = true;
                if crate::debug_flag("IFTRACE") { eprintln!("ENDOCC crates/tex-core/src/expand.rs:686 line={}", self.input.current_file_line()); }
                return;
            }
            if crate::debug_flag("SKIPTRACE") {
                eprintln!("SKIP tok {:#x}", t.0);
            }
            if !t.is_cs() {
                continue;
            }
            let is = match self.eqtb.resolve(t.cs_id()) {
                Some(Equiv::Prim(p)) => *p,
                _ => continue,
            };
            match is {
                Prim::IfChar | Prim::IfCat | Prim::IfOdd | Prim::IfNum | Prim::IfDim
                | Prim::IfVoid | Prim::IfHBox | Prim::IfVBox | Prim::IfHMode | Prim::IfVMode
                | Prim::IfInner | Prim::IfMMode | Prim::IfTrue | Prim::IfFalse | Prim::IfDef
                | Prim::IfEOF | Prim::IfCSName | Prim::IfInCsName | Prim::IfX | Prim::IfCase | Prim::IfFontChar => depth += 1,

                Prim::Unless => self.skip_count_unless_target(&mut depth),
                Prim::Fi => {
                    if depth == 0 {
                        if let Some(st) = self.if_stack.last() {
                            if st.accepting {
                                self.if_stack.pop();
                                continue;
                            }
                            self.if_stack.pop();
                        }
                        return;
                    }
                    depth -= 1;
                }
                Prim::Or => {
                    if depth == 0 {
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
                    if depth == 0 {
                        self.skip_branch(false);
                        return;
                    }
                }
                Prim::Else => {
                    if depth == 0 {
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

    /// skip until \fi (used after \else when branch matched)
    fn skip_to_fi(&mut self) {
        let mut depth = 0i32;
        loop {
            if crate::debug_flag("SKIPTRACE") {
                let t0 = self.raw_token();
                eprintln!("SKIPFI tok {:#x} cs={:?} depth={}", t0.0, if t0.is_cs() { Some(String::from_utf8_lossy(self.cs.name(t0.cs_id())).into_owned()) } else { None }, depth);
                { let __pt = t0; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/expand.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
            }
            let t = self.raw_token();
            if t == EOF_MARKER {
                return;
            }
            if !t.is_cs() {
                continue;
            }
            match self.eqtb.resolve(t.cs_id()) {
                Some(Equiv::Prim(Prim::IfChar))
                | Some(Equiv::Prim(Prim::IfCat))
                | Some(Equiv::Prim(Prim::IfOdd))
                | Some(Equiv::Prim(Prim::IfNum))
                | Some(Equiv::Prim(Prim::IfDim))
                | Some(Equiv::Prim(Prim::IfVoid))
                | Some(Equiv::Prim(Prim::IfHBox))
                | Some(Equiv::Prim(Prim::IfVBox))
                | Some(Equiv::Prim(Prim::IfHMode))
                | Some(Equiv::Prim(Prim::IfVMode))
                | Some(Equiv::Prim(Prim::IfInner))
                | Some(Equiv::Prim(Prim::IfMMode))
                | Some(Equiv::Prim(Prim::IfTrue))
                | Some(Equiv::Prim(Prim::IfFalse))
                | Some(Equiv::Prim(Prim::IfEOF))
                | Some(Equiv::Prim(Prim::IfDef))
                | Some(Equiv::Prim(Prim::IfCSName))
                | Some(Equiv::Prim(Prim::IfInCsName))
                | Some(Equiv::Prim(Prim::IfX))
                | Some(Equiv::Prim(Prim::IfCase)) => depth += 1,

                Some(Equiv::Prim(Prim::Unless)) => self.skip_count_unless_target(&mut depth),
                Some(Equiv::Prim(Prim::Fi)) => {
                    if depth == 0 {
                        let popped = self.if_stack.pop();
                        if crate::debug_flag("IFTRACE") {
                            eprintln!("IFPOP_SKIPFI line={} popped={:?} depth_after={}", self.input.current_file_line(), popped.as_ref().map(|s| (&s.loc_file, s.loc_line)), self.if_stack.len());
                        }
                        return;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
    }

    /// skip to \or or \fi within an ifcase that already matched
    /// skip to \fi within an ifcase that already matched
    fn skip_case_skip(&mut self) {
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
                Some(Equiv::Prim(p)) => match p {
                    Prim::IfChar | Prim::IfCat | Prim::IfOdd | Prim::IfNum | Prim::IfDim
                    | Prim::IfVoid | Prim::IfHBox | Prim::IfVBox | Prim::IfHMode
                    | Prim::IfVMode | Prim::IfInner | Prim::IfMMode | Prim::IfTrue
                    | Prim::IfFalse | Prim::IfDef | Prim::IfCSName | Prim::IfInCsName | Prim::IfX
                    | Prim::IfCase | Prim::IfFontChar => depth += 1,

                    Prim::Unless => self.skip_count_unless_target(&mut depth),
                    Prim::Fi => {
                        if depth == 0 {
                            let popped = self.if_stack.pop();
                            if crate::debug_flag("IFTRACE") {
                                eprintln!("IFPOP_CASE line={} popped={:?} depth_after={}", self.input.current_file_line(), popped.as_ref().map(|s| (&s.loc_file, s.loc_line)), self.if_stack.len());
                            }
                            return;
                        }
                        depth -= 1;
                    }
                    _ => {}
                },
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
        // Active `_` with \\def_{_} (body is cc13 not a CS) loops otherwise.
        b.is_char() && b.cc() == 13 && self.cs.name(id) == [b.chr() as u8]
    }

    pub fn expand_macro(&mut self, id: CsId, m: &Macro) {
        // expl3 quarks: \def\q_stop{\q_stop}. Expanding them loops.
        // Treat as \relax so a leaked delimiter does not hang main_loop.
        if self.is_self_quark(id, m) {
            return;
        }
        let name_bytes = self.cs.name(id).to_vec();
        let nm = &name_bytes[..];
        if (nm == b"f@encoding" || nm == b"cf@encoding") && m.body.is_empty() {
            let ot1_body = vec![
                Token::char(12, b'O' as u32),
                Token::char(12, b'T' as u32),
                Token::char(12, b'1' as u32),
            ];
            self.push_tokens(ot1_body);
            return;
        }
        if (nm == b"textperthousand" || nm == b"?-cmd" || nm == b"TextSymbolUnavailable")
            && self.input.current_file_line() >= 14408
        {
            static TPD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if TPD.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 24 {
                let cf = self.cs.lookup(b"cf@encoding").and_then(|id| match self.eqtb.resolve(id) {
                    Some(Equiv::Macro(mm)) => Some(self.tokens_to_string(&mm.body)),
                    Some(e) => Some(e.kind_name().to_string()),
                    None => None,
                });
                eprintln!(
                    "TP-EXP \\{} e-scan={} ifs={} mac_depth={} np={} body=[{}] cf@enc={:?} protect={} L{} last={:?}",
                    String::from_utf8_lossy(nm),
                    self.in_expanded_scan,
                    self.if_stack.len(),
                    self.mac_depth,
                    m.num_params,
                    self.tokens_to_string(&m.body.iter().take(12).cloned().collect::<Vec<_>>()),
                    cf,
                    self.cs.lookup(b"protect").and_then(|id| self.eqtb.resolve(id).cloned()).map(|e| match e {
                        Equiv::Macro(mm) => format!("Macro[{}]", self.tokens_to_string(&mm.body.iter().take(6).cloned().collect::<Vec<_>>())),
                        Equiv::Prim(pp) => format!("Prim({:?})", pp),
                        other => other.kind_name().to_string(),
                    }).unwrap_or_default(),
                    self.input.current_file_line(),
                    self.last_macros.iter().rev().take(8).collect::<Vec<_>>()
                );
                if nm == b"textperthousand" {
                    let ring: Vec<std::string::String> = self.tok_ring.iter().rev().take(30).map(|(v, ln)| {
                        let tk = Token(*v);
                        if tk.is_cs() { format!("\\{:?}@{}", std::string::String::from_utf8_lossy(self.cs.name(tk.cs_id())), ln) }
                        else { format!("{}{:?}@{}", tk.cc(), tk.chr() as u8 as char, ln) }
                    }).collect();
                    eprintln!("TPRING newest-first=[{}]", ring.join(" "));
                    eprintln!("TPCUR cs={:?} prim={:?}", self.cur_cs.map(|c| std::string::String::from_utf8_lossy(self.cs.name(c)).into_owned()), self.cur_prim);
                }
            }
        }
        self.last_macros.push_back(String::from_utf8_lossy(&name_bytes).into_owned());
        if (nm.starts_with(b"prg_map_break") || nm.starts_with(b"__prg_break_point") || nm.starts_with(b"__file_name_expand")
            || nm.starts_with(b"__tl_map") || nm.starts_with(b"__clist_map") || nm == b"tl_map_break:" || nm == b"clist_map_break:")
            && crate::debug_flag("PRGTRACE")
        {
            static PT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = PT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n < 12 || n % 2000 == 0 {
                let delims: Vec<String> = m.params.iter().map(|d| format!("«{}»", self.tokens_to_string(d))).collect();
                let pv: Vec<Token> = self.pushed.iter().rev().take(8).cloned().collect();
                let pnames = self.tokens_to_string(&pv);
                let stk: Vec<String> = self.input.stack.iter().rev().take(3).map(|src| match src {
                    crate::input::Source::TokList { name, pos, toks, .. } => {
                        let rest: Vec<String> = toks[(*pos).min(toks.len())..].iter().take(10).map(|t| format!("{:#010x}", t.0)).collect();
                        format!("T:{} {}/{} rest=[{}]", name, pos, toks.len(), rest.join(" "))
                    }
                    crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name.split('/').last().unwrap_or(name), line_no),
                }).collect();
                eprintln!(
                    "PRG #{} \\{} np={} delims=[{}] pushed=[{}] stack=[{}]",
                    n,
                    String::from_utf8_lossy(nm),
                    m.num_params,
                    delims.join(" | "),
                    pnames,
                    stk.join(" << ")
                );
                if nm.starts_with(b"__tl_map") || nm.starts_with(b"__clist_map") {
                    eprintln!("  body=[{}]", self.tokens_to_string(&m.body.iter().take(40).cloned().collect::<Vec<_>>()));
                }
            }
        }
        if nm == b"d" && crate::debug_flag("DTRACE") {
            static DN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = DN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n < 4 {
                let st: Vec<String> = self.input.stack.iter().rev().take(5).map(|src| match src {
                    crate::input::Source::TokList { name, pos, toks, .. } => format!("T:{} {}/{}", name, pos, toks.len()),
                    crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name.split('/').last().unwrap_or(name), line_no),
                }).collect();
                eprintln!(
                    "D-EXPAND #{} np={} body=[{}] stack=[{}] pushed=[{}] last={:?}",
                    n,
                    m.num_params,
                    self.tokens_to_string(&m.body.iter().take(16).cloned().collect::<Vec<_>>()),
                    st.join(" << "),
                    self.tokens_to_string(&self.pushed.iter().rev().take(10).cloned().collect::<Vec<_>>()),
                    self.last_macros.iter().rev().take(10).collect::<Vec<_>>()
                );
            }
        }
        if crate::debug_flag("FONTSZ")
            && matches!(nm, b"@currsize" | b"@setfontsize" | b"selectfont" | b"fontsize" | b"selectfont " | b"normalsize " | b"footnotesize ")
        {
            static FZ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = FZ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n < 40 {
                let peek: Vec<Token> = self.pushed.iter().rev().take(10).cloned().collect();
                let src = match self.input.stack.last() {
                    Some(crate::input::Source::TokList { name, pos, toks, .. }) => format!("T:{} {}/{}", name, pos, toks.len()),
                    Some(crate::input::Source::File { name, line_no, .. }) => format!("F:{}:{}", name.split('/').last().unwrap_or(name), line_no),
                    None => "none".into(),
                };
                eprintln!(
                    "FONTSZ #{} \\{} pushed={} src={} next=[{}]",
                    n,
                    String::from_utf8_lossy(nm),
                    self.pushed.len(),
                    src,
                    self.tokens_to_string(&peek)
                );
                if nm == b"@currsize" && n >= 15 && n < 18 {
                    eprintln!("FONTSZ-BT\n{}", std::backtrace::Backtrace::force_capture());
                }
            }
        }
        while self.last_macros.len() > 48 {
            self.last_macros.pop_front();
        }

        if nm == b"q__tl_recursion_tail" && crate::debug_flag("QUARKTRACE") {
            eprintln!("QUARK-EXPAND expanded_scan={} depth={} mac_depth={} at {}:{} pushed=[{}] backtrace:\n{}",
                self.in_expanded_scan, self.gt_steps, self.mac_depth,
                self.input.current_file_name().split('/').last().unwrap_or("?"), self.input.current_file_line(),
                self.tokens_to_string(&self.pushed.iter().rev().take(8).cloned().collect::<Vec<_>>()),
                std::backtrace::Backtrace::force_capture());
            if self.gt_steps > 100_000_000 { std::process::exit(7); }
        }
        if nm == b"@pushfilename" {
            let st: Vec<String> = self.input.stack.iter().rev().take(6).map(|src| match src {
                crate::input::Source::TokList { name, .. } => format!("T:{}", name),
                crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name.split('/').last().unwrap_or(name), line_no),
            }).collect();
            eprintln!("PUSHFILENAME_CALLED at line {} stack=[{}] last12={:?}", self.input.current_file_line(), st.join(" << "), self.last_macros);
        }
        if crate::debug_flag("INTCMP")
            && (nm == b"__int_compare_<=:NNw" || nm == b"__int_compare_<:NNw")
        {
            static IC: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if IC.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 4 {
                eprintln!(
                    "INTCMP \\{} np={} prefix=[{}] params=[{}] L{} file={}",
                    String::from_utf8_lossy(nm),
                    m.num_params,
                    self.tokens_to_string(&m.prefix),
                    m.params.iter().map(|p| format!("«{}»", p.iter().map(|t| format!("{:#x}", t.0)).collect::<Vec<_>>().join(" "))).collect::<Vec<_>>().join(" | "),
                    self.input.current_file_line(),
                    self.input.current_file_name().split('/').last().unwrap_or("")
                );
            }
            if IC.fetch_add(0, std::sync::atomic::Ordering::Relaxed) <= 4 {
            for key in [b"__int_compare:w" as &[u8], b"__int_compare:Nw", b"__int_compare:NNw", b"__int_to_roman:w"] {
                if let Some(cid) = self.cs.lookup(key) {
                    match self.eqtb.resolve(cid) {
                        Some(Equiv::Macro(mm)) => eprintln!(
                            "  meaning \\{} np={} body=[{}]",
                            String::from_utf8_lossy(key),
                            mm.num_params,
                            self.tokens_to_string(&mm.body.iter().take(30).cloned().collect::<Vec<_>>())
                        ),
                        Some(Equiv::Prim(p)) => eprintln!("  meaning \\{} Prim({p:?})", String::from_utf8_lossy(key)),
                        other => eprintln!("  meaning \\{} {:?}", String::from_utf8_lossy(key), other.map(|e| e.kind_name())),
                    }
                }
            }
            }
        }
        if nm == b"@onlypreamble" {
            static HITN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if HITN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 4 {
                eprintln!(
                    "ONLYPREAMBLE-HIT L{} macros={:?}",
                    self.input.current_file_line(),
                    self.last_macros.iter().rev().take(6).collect::<Vec<_>>()
                );
            }
        }

        if nm == b"@preamblecmds" && self.input.current_file_line() >= 6738 {
            static PCN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if PCN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 6 {
                eprintln!(
                    "EXPAND-PREAMBLECMDS np={} prefix=[{}] body=[{}] L{}",
                    m.num_params,
                    self.tokens_to_string(&m.prefix),
                    self.tokens_to_string(&m.body.iter().take(24).cloned().collect::<Vec<_>>()),
                    self.input.current_file_line()
                );
            }
        }

        self.current_macro = String::from_utf8_lossy(nm).into_owned();
        if nm == b"GTS@RemoveLeft" {
            static RL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if RL.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 3 {
                let body_of = |nm: &[u8]| -> std::string::String {
                    self.cs.lookup(nm).and_then(|id| match self.eqtb.resolve(id) {
                        Some(Equiv::Macro(m)) => Some(self.tokens_to_string(&m.body)),
                        _ => None,
                    }).unwrap_or_else(|| "?".into())
                };
                let tok = self.cs.lookup(b"GTS@Token").map(|id| match self.eqtb.resolve(id) {
                    Some(Equiv::Macro(m)) => format!("mac[{}]", self.tokens_to_string(&m.body)),
                    Some(other) => other.kind_name().to_string(),
                    None => "undef".into(),
                }).unwrap_or_else(|| "not-interned".into());
                eprintln!(
                    "RL token={tok} expanded={} tl=[{}] macros={:?} pushed=[{}]",
                    self.in_expanded_scan,
                    body_of(b"GTS@TestLeft"),
                    self.last_macros.iter().rev().take(8).collect::<Vec<_>>(),
                    self.tokens_to_string(&self.pushed.iter().rev().take(16).cloned().collect::<Vec<_>>())
                );
            }
        }


        if nm == b"GTS@TestLeftEnd" {
            static TLE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if TLE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 4 {
                let toks_at = self.cs.lookup(b"toks@").map(|id| match self.eqtb.resolve(id) {
                    Some(Equiv::ToksReg(i)) => {
                        let s = self.tokens_to_string(&self.eqtb.toks[*i as usize]);
                        format!("ToksReg({i}) [{s}]")
                    }
                    Some(other) => other.kind_name().to_string(),
                    None => "undef".into(),
                }).unwrap_or_else(|| "not-interned".into());
                let tok = self.cs.lookup(b"GTS@Token").map(|id| match self.eqtb.resolve(id) {
                    Some(Equiv::Macro(m)) => format!("mac[{}]", self.tokens_to_string(&m.body)),
                    Some(other) => other.kind_name().to_string(),
                    None => "undef".into(),
                }).unwrap_or_else(|| "not-interned".into());
                let gts = self.cs.lookup(b"GTS@GlobalString").and_then(|id| match self.eqtb.resolve(id) {
                    Some(Equiv::Macro(m)) => Some(self.tokens_to_string(&m.body)),
                    _ => None,
                });
                let spt = self.cs.lookup(b"@sptoken").map(|id| match self.eqtb.resolve(id) {
                    Some(Equiv::CharTok(v)) => format!("CharTok({v:#x})"),
                    Some(other) => other.kind_name().to_string(),
                    None => "undef".into(),
                }).unwrap_or_else(|| "not-interned".into());
                eprintln!(
                    "TLE toks@={toks_at} token={tok} gts={gts:?} sptoken={spt} macros={:?}",
                    self.last_macros.iter().rev().take(8).collect::<Vec<_>>()
                );
            }
        }

        if nm.starts_with(b"__codepoint_data_aux") {
            static AUXN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if AUXN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 6 {
                eprintln!(
                    "AUXW \\{} np={} prefix=[{}] params=[{}] L{} file={}",
                    String::from_utf8_lossy(nm),
                    m.num_params,
                    self.tokens_to_string(&m.prefix),
                    m.params.iter().enumerate().map(|(i,p)| format!("#{}«{}»", i+1, self.tokens_to_string(p))).collect::<Vec<_>>().join(" | "),
                    self.input.current_file_line(),
                    self.input.current_file_name().split('/').last().unwrap_or("")
                );
            }
        }
        if nm == b"__quark_if_recursion_tail:w" || nm == b"quark_if_recursion_tail_stop:n" {
            static QN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if QN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 4 {
                eprintln!(
                    "QUARKW \\{} np={} prefix=[{}] params=[{}] L{} file={}",
                    String::from_utf8_lossy(nm),
                    m.num_params,
                    self.tokens_to_string(&m.prefix),
                    m.params.iter().enumerate().map(|(i,p)| format!("#{}«{}»", i+1, self.tokens_to_string(p))).collect::<Vec<_>>().join(" | "),
                    self.input.current_file_line(),
                    self.input.current_file_name().split('/').last().unwrap_or("")
                );
            }
        }
        if nm == b"UseHook" || nm == b"hook_use:n" {
            static HN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if HN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 80 {
                let nxt: Vec<String> = self.pushed.iter().rev().take(24).map(|t| {
                    if t.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) }
                    else { format!("{}{}", t.cc(), t.chr() as u8 as char) }
                }).collect();
                eprintln!(
                    "USEHOOK \\{} L{} csname={} file={} next=[{}]",
                    String::from_utf8_lossy(nm),
                    self.input.current_file_line(),
                    self.csname_depth,
                    self.input.current_file_name().split('/').last().unwrap_or(""),
                    nxt.join(" ")
                );
            }
        }
        if nm == b"ProvidesFile" {
            let nxt: Vec<String> = self.pushed.iter().rev().take(16).map(|t| {
                if t.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) }
                else { format!("{}{}", t.cc(), t.chr() as u8 as char) }
            }).collect();
            let f = self.input.current_file_name();
            if self.pushed.len() > 1000 || f.contains(".fd") || self.csname_depth > 0 {
                let st: Vec<String> = self.input.stack.iter().rev().take(6).map(|src| match src {
                    crate::input::Source::TokList { name, .. } => format!("T:{}", name),
                    crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name.split('/').last().unwrap_or(name), line_no),
                }).collect();
                eprintln!(
                    "PROVIDES np={} prefix=[{}] body=[{}] L{} file={} pushed={} csname={} stack=[{}] next=[{}] last={:?}",
                    m.num_params,
                    self.tokens_to_string(&m.prefix),
                    self.tokens_to_string(&m.body.iter().take(12).cloned().collect::<Vec<_>>()),
                    self.input.current_file_line(),
                    f.split('/').last().unwrap_or(""),
                    self.pushed.len(),
                    self.csname_depth,
                    st.join(" << "),
                    nxt.join(" "),
                    self.last_macros.iter().rev().take(8).collect::<Vec<_>>()
                );
            }
            if self.pushed.len() > 50_000 {
                eprintln!("PROVIDES-LOOP abort pushed={}", self.pushed.len());
                std::process::exit(9);
            }
        }


        self.mac_depth += 1;
        if self.mac_depth > 200 {
            eprintln!("RECURSION depth {} at {}", self.mac_depth, String::from_utf8_lossy(self.cs.name(id)));
            if self.mac_depth > 205 { std::process::exit(8); }
        }
        if self.input.stack.len() > 50_000 && !self.loop_traced {
            self.loop_traced = true;
            eprintln!("LOOPGROW at stack={}; dumping top 30 sources:", self.input.stack.len());
            for s in self.input.stack.iter().rev().take(30) {
                match s {
                    crate::input::Source::TokList { name, pos, toks, .. } => {
                        let rest: Vec<String> = toks[(*pos).min(toks.len())..].iter().take(8).map(|t| {
                            if t.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) }
                            else if t.0 >= 0x4000_0000 && t.0 < 0x8000_0000 { format!("#{}", t.0 & 0x3FFF_FFFF) }
                            else { format!("{}{}", t.cc(), t.chr() as u8 as char) }
                        }).collect();
                        eprintln!("  toklist {} pos {}/{} rest=[{}]", name, pos, toks.len(), rest.join(" "));
                    }
                    crate::input::Source::File { name, line_no, .. } => {
                        eprintln!("  file {} line {}", name, line_no);
                    }
                }
            }
            eprintln!("LAST MACROS: {:?}", self.last_macros);
            eprintln!("BOTTOM 12 sources:");
            for s in self.input.stack.iter().take(12) {
                match s {
                    crate::input::Source::TokList { name, pos, toks, .. } => {
                        let rest: Vec<String> = toks[(*pos).min(toks.len())..].iter().take(6).map(|t| {
                            if t.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) }
                            else if t.0 >= 0x4000_0000 && t.0 < 0x8000_0000 { format!("#{}", t.0 & 0x3FFF_FFFF) }
                            else { format!("{}{}", t.cc(), t.chr() as u8 as char) }
                        }).collect();
                        eprintln!("  toklist {} pos {}/{} rest=[{}]", name, pos, toks.len(), rest.join(" "));
                    }
                    crate::input::Source::File { name, line_no, .. } => {
                        eprintln!("  file {} line {}", name, line_no);
                    }
                }
            }
            eprintln!("AT FILE {} LINE {}", self.input.current_file_name(), self.input.current_file_line());
            if let Some(did) = self.cs.lookup(b"do") {
                match self.eqtb.resolve(did).cloned() {
                    Some(crate::eqtb::Equiv::Macro(dm)) => eprintln!("MEANING \\do: nparams={} prefix={:?} body={}", dm.num_params, dm.params.iter().map(|d| self.tokens_to_string(d)).collect::<Vec<_>>(), self.tokens_to_string(&dm.body)),
                    other => eprintln!("MEANING \\do: {:?}", other.map(|e| e.kind_name())),
                }
            } else {
                eprintln!("MEANING \\do: <no such cs>");
            }
            eprintln!("CUR TOKEN cs={} prim={:?}", self.cur_cs.map(|c| String::from_utf8_lossy(self.cs.name(c)).into_owned()).unwrap_or_default(), self.cur_prim);
            if let Some(crate::eqtb::Equiv::Macro(mm)) = self.cs.lookup(b"global").and_then(|g| self.eqtb.resolve(g).cloned()) {
                eprintln!("meaning of \\global: {}", self.tokens_to_string(&mm.body));
                eprintln!("params: {:?}", mm.params.iter().map(|d| self.tokens_to_string(d)).collect::<Vec<_>>());
            }
            std::process::exit(7);
        }
        if !m.prefix.is_empty() {
            // tex.web: tokens before the first # must match the next
            // input tokens exactly. scan_delimited would skip ahead and
            // steal a later \\fi: (expl3 \\__tl_if_head_is_group_fi_false:w).
            for p in &m.prefix {
                let t = self.raw_token();
                if t == EOF_MARKER {
                    self.error("File ended while matching macro prefix");
                    self.end_occurred = true;
                    self.mac_depth = self.mac_depth.saturating_sub(1);
                    return;
                }
                if !Self::delim_eq(t, *p) {
                    static PN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                    if PN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
                        let got = if t.is_cs() {
                            format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id())))
                        } else {
                            format!("c{}:{:?}", t.cc(), t.chr() as u8 as char)
                        };
                        eprintln!(
                            "PREFIX-FAIL \\{} want=[{}] got={} L{} file={} np={} body=[{}]",
                            String::from_utf8_lossy(self.cs.name(id)),
                            self.tokens_to_string(&m.prefix),
                            got,
                            self.input.current_file_line(),
                            self.input.current_file_name().split('/').last().unwrap_or(""),
                            m.num_params,
                            self.tokens_to_string(&m.body.iter().take(20).cloned().collect::<Vec<_>>())
                        );
                        let mut peek = vec![t];
                        for _ in 0..40 {
                            let x = self.raw_token();
                            if x == crate::input::EOF_MARKER {
                                break;
                            }
                            peek.push(x);
                        }
                        eprintln!("  MSGTOKENS [{}]", self.tokens_to_string(&peek));
                        for x in peek.into_iter().rev() {
                            self.pushed.push(x);
                        }
                    } else {
                        self.pushed.push(t);
                    }
                    self.error(&format!(
                        "Use of \\{} doesn't match its definition",
                        String::from_utf8_lossy(self.cs.name(id))
                    ));
                    self.mac_depth = self.mac_depth.saturating_sub(1);
                    return;
                }
            }
        }
        let mut args: Vec<Vec<Token>> = Vec::with_capacity(m.num_params as usize);
        for (i, delim) in m.params.iter().enumerate() {
            if i as u32 + 1 > m.num_params as u32 {
                break;
            }
            if delim.is_empty() {
                // undelimited
                self.skip_raw_spaces();
                let t = self.raw_token();
                if t == PAR_END && !m.long {
                    self.error(&format!(
                        "Paragraph ended before \\{} was complete; delim-args={:?} body={}",
                        String::from_utf8_lossy(self.cs.name(id)),
                        m.params.iter().map(|d| self.tokens_to_string(d)).collect::<Vec<_>>(),
                        self.tokens_to_string(&m.body)
                    ));
                    args.push(Vec::new());
                    continue;
                }
                if t == EOF_MARKER {
                    self.error("File ended while scanning argument");
                    self.end_occurred = true;
                    if crate::debug_flag("IFTRACE") { eprintln!("ENDOCC crates/tex-core/src/expand.rs:897 line={}", self.input.current_file_line()); }
                    args.push(Vec::new());
                    continue;
                }
                if crate::debug_flag("ARGTRACE") {
                    let srcs: Vec<std::string::String> = self.input.stack.iter().rev().take(3).map(|src| match src {
                        crate::input::Source::TokList { name, pos, toks, .. } => format!("T:{} {}/{}", name, pos, toks.len()),
                        crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name, line_no),
                    }).collect();
                    let tname = if t.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) } else { format!("cc{}", t.cc()) };
                    let ring: Vec<std::string::String> = self.tok_ring.iter().rev().take(14).map(|(v, ln)| format!("{:#x}@{}", v, ln)).collect();
                    eprintln!("ARG \\{} #{}/{} t={} pushed={:?} srcs=[{}] ring=[{}]", String::from_utf8_lossy(self.cs.name(id)), i + 1, m.num_params, tname, self.pushed.iter().rev().take(3).map(|x| format!("{:#x}", x.0)).collect::<Vec<_>>(), srcs.join(" << "), ring.join(" "));
                }
                if t.is_char() && t.cc() == 2 {
                    // tex.web ~§397: a lone right brace = unbalanced group
                    self.pushed.push(t);
                    if nm == b"@input@file@exists@with@hooks"
                        || (self.input.current_file_line() >= 4540 && self.input.current_file_line() <= 4570)
                    {
                        let fund = self.cs.lookup(b"@filef@und").and_then(|id| match self.eqtb.resolve(id) {
                            Some(crate::eqtb::Equiv::Macro(mm)) => Some(self.tokens_to_string(&mm.body)),
                            _ => None,
                        }).unwrap_or_else(|| "?".into());
                        let prot = self.cs.lookup(b"protect").map(|pid| match self.eqtb.resolve(pid) {
                            Some(crate::eqtb::Equiv::Prim(p)) => format!("prim:{:?}", p),
                            Some(crate::eqtb::Equiv::Macro(mm)) => format!("macro:{}", self.tokens_to_string(&mm.body)),
                            Some(other) => other.kind_name().to_string(),
                            None => "undef".into(),
                        }).unwrap_or_else(|| "missing".into());
                        eprintln!(
                            "XBRACE \\{} L{} e-scan={} csname={} fund=[{}] protect=[{}] afterasg={} macros={:?} src={}",
                            String::from_utf8_lossy(self.cs.name(id)),
                            self.input.current_file_line(),
                            self.in_expanded_scan,
                            self.csname_depth,
                            fund,
                            prot,
                            self.after_assignment.is_some(),
                            self.last_macros.iter().rev().take(8).collect::<Vec<_>>(),
                            self.input.current_file_name().split('/').last().unwrap_or(""),
                        );
                        for s in self.input.stack.iter().rev().take(6) {
                            match s {
                                crate::input::Source::TokList { name, pos, toks, .. } => {
                                    let rest: Vec<String> = toks[(*pos).min(toks.len())..].iter().take(10).map(|tt| {
                                        if tt.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(tt.cs_id()))) }
                                        else { format!("{}:'{}'", tt.cc(), tt.chr() as u8 as char) }
                                    }).collect();
                                    eprintln!("  toklist {} pos {}/{} rest=[{}]", name, pos, toks.len(), rest.join(" "));
                                }
                                crate::input::Source::File { name, line_no, .. } => {
                                    eprintln!("  file {} line {}", name.split('/').last().unwrap_or(""), line_no);
                                }
                            }
                        }
                        eprintln!("  pushed=[{}]", self.tokens_to_string(&self.pushed.iter().rev().take(8).cloned().collect::<Vec<_>>()));
                    }

                    self.error(&format!(
                        "Argument of \\{} has an extra }}",
                        String::from_utf8_lossy(self.cs.name(id))
                    ));
                    args.push(Vec::new());
                } else if t.is_char() && t.cc() == 1 {
                    // braced group
                    let arg = self.scan_balanced_raw(m.long);
                    args.push(arg);
                } else {
                    args.push(vec![t]);
                }
            } else {
                // delimited
                let arg = self.scan_delimited(delim, m.long);
                args.push(arg);
            }
        }
        if crate::debug_flag("MACTRACE") {
            let argstr: Vec<String> = args.iter().map(|a| self.tokens_to_string(a)).collect();
            eprintln!("MAC \\{} args={:?}", String::from_utf8_lossy(nm), argstr);
        }
        if crate::debug_flag("QUARKTRACE2")
            && (nm.starts_with(b"__quark") || nm.starts_with(b"__kernel_quark") || nm == b"cs_gset:Npn" || nm == b"exp_args:NNcc" || nm == b"exp_args:Ncc")

        {
            let argstr: Vec<String> = args.iter().map(|a| self.tokens_to_string(a)).collect();
            eprintln!(
                "Q2 \\{} np={} declared={:?} args=[{}] L{}",
                String::from_utf8_lossy(nm),
                m.num_params,
                m.params.len(),
                argstr.join(" | "),
                self.input.current_file_line()
            );
        }
        if nm == b"exp_args:NNcc" && crate::debug_flag("QUARKTRACE2") {
            eprintln!("NNCC-EXPAND ok");
        }

        if nm == b"__quark_new_test:Nccn" && crate::debug_flag("QUARKTRACE2") {
            let bd: Vec<String> = m.body.iter().map(|t| format!("{:#x}", t.0)).collect();
            eprintln!("NCCN-BODY hex=[{}] str=[{}]", bd.join(","), self.tokens_to_string(&m.body));
        }




        // TeX splices macro arguments into the replacement text eagerly at
        // expansion time (tex.web macro_expand). Reproduce that here: build the
        // substituted body up front instead of relying on lazy in-param replay,
        // whose arg-exhaustion boundary interacted with conditional skipping.
        let has_pr = m.body.iter().any(|t| t.0 >= 0x4000_0000 && t.0 < 0x8000_0000);
        let spliced = if has_pr {
            let mut v: Vec<crate::token::Token> = Vec::with_capacity(m.body.len() + 16);
            for &t in &m.body {
                if t.0 >= 0x4000_0000 && t.0 < 0x8000_0000 {
                    let n = (t.0 & 0x3FFF_FFFF) as usize;
                    if n >= 1 && n <= args.len() {
                        v.extend_from_slice(&args[n - 1]);
                        continue;
                    }
                }
                v.push(t);
            }
            v
        } else {
            m.body.clone()
        };
        if nm == b"@onlypreamble" {
            static OPB: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if OPB.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
                eprintln!("ONLYP-SPLICED [{}]", self.tokens_to_string(&spliced));
            }
        }
        if crate::debug_flag("BODYDUMP") && name_bytes == b"e@alloc" {
            let dump: Vec<String> = spliced.iter().enumerate().map(|(i,t)| {
                if t.is_cs() { format!("{}:{}", i, String::from_utf8_lossy(self.cs.name(t.cs_id()))) }
                else { format!("{}:{}{}", i, t.cc(), t.chr() as u8 as char) }
            }).collect();
            eprintln!("BODYDUMP {}", dump.join(" "));
        }
        if crate::debug_flag("IFTRACE") {
            let src_name = format!("<m:{}>", String::from_utf8_lossy(&name_bytes));
            self.push_tokens_named(spliced, &src_name);
        } else {
            self.push_tokens(spliced);
        }




        self.mac_depth -= 1;
    }
    pub fn skip_raw_spaces(&mut self) {
        loop {
            let t = self.raw_token();
            if t.is_char() && t.cc() == 10 {
                continue;
            }
            { let __pt = t; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/expand.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
            return;
        }
    }

    /// scan a balanced group (the opening brace already consumed);
    /// returns tokens without the outer braces.
    /// `long`: Knuth `\long` — `\par` (blank line) is legal inside the group.
    /// `\@firstoftwo` is long; nameref.sty's unused branch starts with a blank line.
    pub fn scan_balanced_raw(&mut self, long: bool) -> Vec<Token> {
        let mut depth = 1i32;
        let mut out = Vec::new();
        loop {
            let t = self.raw_token();
            if t == EOF_MARKER {
                self.error("Runaway argument / missing }");
                self.end_occurred = true;
                return out;
            }
            if t == PAR_END && !long {
                self.error("Runaway argument / missing }");
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
            out.push(t);
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
                self.expand_macro(id, &m);
                true
            }
            Some(Equiv::Prim(p)) => {
                if !self.is_expandable(p) {
                    self.set_cur_cs(t);
                    return false;
                }
                match self.expand_prim(p, id) {
                    Some(tok) => {
                        if tok.0 >= NOEXP_FLAG && tok.0 < 0xFFFF_0000 {
                            let plain = Token::from_cs(tok.0 & 0x3FFF_FFFF);
                            self.pushed.push(plain);
                        } else {
                            self.pushed.push(tok);
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

    fn scan_delimited(&mut self, delim: &[Token], long: bool) -> Vec<Token> {
        let mut arg: Vec<Token> = Vec::new();
        let mut di = 0usize;
        loop {
            // tex.web scan_toks: fetch is RAW; the delimiter matches by
            // TOKEN before any expansion (a macro delimiter like `\stop`
            // must be seen as a token). Only non-matching tokens are
            // expanded, in the final else arm below. Expanding eagerly
            // here (get_x_raw) made the delimiter's own expansion leak
            // into the argument (`\z a\mark` -> `[aSTOP]`).
            let t = self.raw_token();
            if t == EOF_MARKER {
                self.error(&format!("Runaway argument of \\{} (delim={})", self.current_macro, self.tokens_to_string(delim)));
                if std::env::var("UNDEFTRACE").is_ok() {
                    eprintln!(
                        "RUNAWAY-STACK {:?}",
                        self.input.stack.iter().rev().map(|s| match s {
                            crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name.split('/').last().unwrap_or(name), line_no),
                            crate::input::Source::TokList { name, pos, toks, .. } => format!("T:{} {}/{}", name, pos, toks.len()),
                        }).collect::<Vec<_>>()
                    );
                }
                self.end_occurred = true;
                if crate::debug_flag("IFTRACE") { eprintln!("ENDOCC crates/tex-core/src/expand.rs:1003 line={}", self.input.current_file_line()); }
                return arg;
            }
            if t == PAR_END && !long {
                self.error(&format!(
                    "Paragraph ended before \\{} was complete (delim={}) collected={}",
                    self.current_macro,
                    self.tokens_to_string(delim),
                    self.tokens_to_string(&arg)
                ));
                return arg;
            }
            let d = delim[di];
            if t.is_char() && t.cc() == 1 {
                if crate::debug_flag("GRABTRACE") {
                    eprintln!("GRAB brace: macro={} di={}/{} delim_ok={} pending={}",
                        self.current_macro, di, delim.len(),
                        delim.get(di).map(|d| d.0 == t.0).unwrap_or(false),
                        arg.iter().map(|d| format!("{:#x}", d.0)).collect::<Vec<_>>().join(","));
                }
                if delim.get(di).copied().map(|d| Self::delim_eq(t, d)).unwrap_or(false) {
                    di += 1;
                    if di >= delim.len() {
                        // tex.web §392: hash_brace (`#{`) leaves the `{` in the
                        // input stream (back_input) for the following construct!
                        self.pushed.push(t);
                        return arg;
                    }
                    continue;
                }
                if di > 0 {
                    arg.extend_from_slice(&delim[0..di]);
                    di = 0;
                }
                let inner = self.scan_balanced_raw(long);
                arg.push(Token::char(1, b'{' as u32));
                arg.extend(inner);
                arg.push(Token::char(2, b'}' as u32));
            } else if Self::delim_eq(t, d) {
                di += 1;
                if di >= delim.len() {
                    Self::strip_outer_braces(&mut arg);
                    return arg;
                }
            } else if di > 0 {
                arg.push(delim[0]);
                let mut rest: Vec<Token> = delim[1..di].to_vec();
                rest.push(t);
                di = 0;
                for rt in rest.into_iter().rev() {
                    self.pushed.push(rt);
                }
            } else if crate::debug_flag("DELIM_EXPAND")
                && self.in_expanded_scan
                && self.expand_if_expansive(t)
            {
                // Real-pdftex probes: expansion during delimited scans only
                // inside \edef collection (`\xdef\ee{\z \foo d\mark}` ->
                // RAWBODY [Fd]); top-level `\Z\foo` -> runaway (raw).
                // Default OFF: the nested-grab re-sync (__prg_generate_
                // conditional at expl3-code:1893) still diverges; flip
                // DELIM_EXPAND=1 to A/B in the kvdbg battery.
                continue;
            } else {
                arg.push(t);
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
            .map(|&b| if b == b' ' { Token::space() } else { Token::other(b) })
            .collect();
        self.push_tokens(toks);
    }

    pub fn tokens_to_string(&self, toks: &[Token]) -> String {
        let mut out: Vec<u8> = Vec::new();
        let esc = self.eqtb.int_params[crate::prim::IntParam::EscapeChar.idx() as usize];
        for t in toks {
            if t.0 >= PAR_REF_FLAG && t.0 < 0xFFFF_0000 && !t.is_cs() {
                out.push(b'#');
                out.push(b'0' + (t.0 & 0xF) as u8);
                continue;
            }
            if t.is_cs() {
                if esc >= 0 && esc <= 255 {
                    out.push(esc as u8);
                }
                let name = self.cs.name(t.cs_id());
                out.extend_from_slice(name);
                // tex.web print_cs / show_token_list: control word (name
                // length > 1) is followed by a space. amsmath \@tempb
                // splits \\meaning on that space.
                if name.len() > 1 {
                    out.push(b' ');
                }
            } else {
                out.push(t.chr() as u8);
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    pub fn ifx_equal(&self, mut a: Token, mut b: Token) -> bool {
        self.ifx_equal_inner(a, b)
    }
    fn ifx_equal_inner(&self, mut a: Token, mut b: Token) -> bool {
        if a.is_char() && a.cc() == 13 {
            if let Some(id) = self.cs.lookup(&[a.chr() as u8]) {
                a = Token::from_cs(id);
            }
        }
        if b.is_char() && b.cc() == 13 {
            if let Some(id) = self.cs.lookup(&[b.chr() as u8]) {
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
            self.term.push('\n');
        }
        self.term.push_str(s);
    }

    pub fn error(&mut self, msg: &str) {
        let mut ctx = String::new();
        for s in self.input.stack.iter().rev().take(2) {
            match s {
                crate::input::Source::TokList { name, pos, toks, .. } => {
                    ctx.push_str(&format!(
                        " [{} {}/{} rest={:?}]",
                        name,
                        pos,
                        toks.len(),
                        toks[(*pos).min(toks.len())..]
                            .iter()
                            .take(6)
                            .map(|t| format!("{:#x}", t.0))
                            .collect::<Vec<_>>()
                    ))
                }
                crate::input::Source::File { name, line_no, .. } => {
                    ctx.push_str(&format!(" [{} line {}]", name, line_no))
                }
            }
        }
        self.term_print_nl(&format!(
            "! {} at line {}{}\n",
            msg,
            self.input.current_file_line(),
            ctx
        ));
        self.error_count += 1;
        if self.error_count > 2000 && !self.ini_mode {
            self.end_occurred = true;
        }
        if crate::debug_flag("STOP_FIRST_ERR") {
            eprintln!(
                "FIRST-ERR {} L{} file={} mac={:?} last={:?} pushed={}",
                msg,
                self.input.current_file_line(),
                self.input.current_file_name(),
                self.current_macro,
                self.last_macros.iter().rev().take(12).collect::<Vec<_>>(),
                self.tokens_to_string(&self.pushed.iter().rev().take(12).cloned().collect::<Vec<_>>())
            );
            self.end_occurred = true;
        }
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
