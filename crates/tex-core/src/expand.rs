//! Token expansion: get_token, macro expansion, conditionals, \csname and
//! the string-producing primitives.

use crate::eqtb::{Equiv, Macro};
use crate::engine::Engine;
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

    /// fetch next raw token honoring pushback
    pub fn raw_token(&mut self) -> Token {
        self.rt_steps += 1;
        if self.rt_steps % 1_000_000 == 0 {
            eprintln!("RT {}M raw calls, file {} line {}, cs={} prim={:?} stack={}", self.rt_steps / 1_000_000, self.input.current_file_name(), self.input.current_file_line(),
                self.cur_cs.map(|c| String::from_utf8_lossy(self.cs.name(c)).into_owned()).unwrap_or_default(), self.cur_prim, self.input.stack.len());
        }
        if let Some(t) = self.pushed.pop() {
            if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) {
                self.tok_ring.push_back((t.0, self.input.current_file_line()));
                while self.tok_ring.len() > 30 {
                    self.tok_ring.pop_front();
                }
            }
            return t;
        }
        let t = self.get_next_raw();
        if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) {
            self.tok_ring.push_back((t.0, self.input.current_file_line()));
            while self.tok_ring.len() > 30 {
                self.tok_ring.pop_front();
            }
        }
        t
    }

    /// tex.web get_x_token: expands macros/conditionals but does NOT skip
    /// spaces (scan_int relies on spaces terminating constants).
    pub fn get_x_raw(&mut self) -> Token {
        loop {
            let t = self.raw_token();
            if !t.is_cs() {
                return t;
            }
            let mut id = t.cs_id();
            for _ in 0..1024 {
                match self.eqtb.get(id) {
                    Some(Equiv::Alias(next)) => id = *next,
                    _ => break,
                }
            }
            match self.eqtb.get(id).cloned() {
                Some(Equiv::Macro(m)) => {
                    if m.protected && self.expand_protected == 0 {
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
                                if tok.is_cs() {
                                    if let Some(Equiv::Prim(p2)) = self.eqtb.resolve(tok.cs_id()) {
                                        if self.is_expandable(*p2) {
                                            self.pushed.push(tok);
                                            continue;
                                        }
                                    }
                                    if let Some(Equiv::Macro(m)) = self.eqtb.resolve(tok.cs_id()) {
                                        if !m.protected {
                                            self.pushed.push(tok);
                                            continue;
                                        }
                                    }
                                }
                                self.cur_tok = tok;
                                self.cur_cs = if tok.is_cs() { Some(tok.cs_id()) } else { None };
                                self.cur_prim = None;
                                return tok;
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

    /// push tokens back so they are seen before any further input
    pub fn push_tokens(&mut self, toks: Vec<Token>) {
        for t in toks.into_iter().rev() {
            { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/expand.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
        }
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
        let t = self.get_token_inner();
        if std::env::var("GBT").map(|v|v=="1").unwrap_or(false) && t.is_cs() && self.cs.name(t.cs_id()) == b"gdef" && self.input.current_file_line() > 2000 {
            eprintln!("GBT at {}:{}:\n{}", self.input.current_file_name(), self.input.current_file_line(), std::backtrace::Backtrace::force_capture());
            std::process::exit(9);
        }
        if std::env::var("RETTRACE").map(|v|v=="1").unwrap_or(false) {
            let nm = if t.is_cs() { std::string::String::from_utf8_lossy(self.cs.name(t.cs_id())).into_owned() } else { format!("cc{}:{}", t.cc(), t.chr()) };
            let src = match self.input.stack.last() { Some(crate::input::Source::TokList { name, pos, .. }) => format!("T:{}:{}", name, pos), Some(crate::input::Source::File { line_no, .. }) => format!("F:{}", line_no), None => "-".into() };
            eprintln!("RET {} @{}", nm, src);
        }
        t
    }

    fn get_token_inner(&mut self) -> Token {
        self.gt_steps += 1;
        if self.gt_steps % 1_000_000 == 0 {
            eprintln!("GT {}M calls, file {} line {}", self.gt_steps / 1_000_000, self.input.current_file_name(), self.input.current_file_line());
        }
        if self.gt_steps > 300_000_000 {
            eprintln!("GT: giving up");
            std::process::exit(9);
        }
        {
            let line = self.input.current_file_line();
            let file = self.input.current_file_name();
            if line == 2802 && file.contains("expl3-code") {
                static DUMPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if !DUMPED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("CAT _={} :={} escape={}", self.eqtb.cat[b'_' as usize], self.eqtb.cat[b':' as usize], self.eqtb.int_params[crate::prim::IntParam::EscapeChar.idx() as usize]);
                    for name in [
                        &b"cs_end:"[..],
                        &b"cs:w"[..],
                        &b"exp_not:c"[..],
                        &b"exp_not:N"[..],
                        &b"cs_new_protected:Npe"[..],
                        &b"__cs_generate_variant:N"[..],
                        &b"exp_args:Nc"[..],
                        &b"::c"[..],
                        &b"cs_gset_protected:Npe"[..],
                        &b"cs_new_protected:cpe"[..],
                        &b"__cs_tmp:w"[..],
                    ] {
                        let id = self.cs.intern(name);
                        let m = self.meaning_of(Token::from_cs(id));
                        eprintln!("MEAN L{} {} = {}", line, String::from_utf8_lossy(name), m);
                    }
                }
            }
        }
        loop {
            let t = self.raw_token();
            if t == crate::page::OUT_END_TOKEN {
                self.finish_output();
                continue;
            }
            if t == crate::align::CELL_END_TOKEN {
                self.finish_cell_typeset();
                continue;
            }
            // protected-once token from \noexpand
            if t.0 >= NOEXP_FLAG && t.0 < 0xFFFF_0000 {
                let cs = t.0 & 0x3FFF_FFFF;
                let tok = Token::from_cs(cs);
                self.set_cur_cs(tok);
                return tok;
            }
            if t == EOF_MARKER {
                self.end_occurred = true;
                if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) { eprintln!("ENDOCC crates/tex-core/src/expand.rs:155 line={}", self.input.current_file_line()); }
                return EOF_MARKER;
            }
            if t == PAR_END {
                // blank line: \par control sequence
                let tok = Token::from_cs(self.ids.par);
                self.set_cur_cs(tok);
                return tok;
            }
            if t.is_cs() {
                let mut id = t.cs_id();
                // follow \let alias chains to the real meaning
                for _ in 0..1024 {
                    match self.eqtb.get(id) {
                        Some(Equiv::Alias(next)) => id = *next,
                        _ => break,
                    }
                }
                match self.eqtb.get(id).cloned() {
                    Some(Equiv::Macro(m)) => {
                        if m.protected && self.expand_protected == 0 {
                            self.set_cur_cs(t);
                            return t;
                        }
                        self.expand_macro(id, &m);
                        continue;
                    }
                    Some(Equiv::CharTok(v)) => {
                        // \let\bgroup={ : the cs stands for the char token
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
                                    if tok.is_cs() {
                                        match self.eqtb.resolve(tok.cs_id()).cloned() {
                                            Some(Equiv::Prim(p2)) if self.is_expandable(p2) => {
                                                self.pushed.push(tok);
                                                continue;
                                            }
                                            Some(Equiv::Macro(m)) if !m.protected => {
                                                self.pushed.push(tok);
                                                continue;
                                            }
                                            _ => {}
                                        }
                                    }
                                    self.cur_tok = tok;
                                    self.cur_cs = if tok.is_cs() { Some(tok.cs_id()) } else { None };
                                    self.cur_prim = None;
                                    return tok;
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
            } else {
                self.set_cur_char(t);
                return t;
            }
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
                | Expanded
                | UnExpanded
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
                | IfX
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
                // tex.web-style: <a> fetched raw, <b> fetched with the expanding
                // get_token (full expansion), then <b> pushed back and <a>
                // returned. NOTE: a tex.web-exact "expand <b> exactly once with
                // raw fetches" variant was tried (2026-08-30) and broke
                // \@onlypreamble's \expandafter\gdef cascade differently
                // (runaway gdef body collection) — see NOTES-BootDump.md.
                let t1 = self.raw_token();
                let pre = self.pushed.last().map(|x| format!("{:#x}", x.0)).unwrap_or_default();
                // tex.web §368: <b> expands EXACTLY one level — a macro is
                // expanded once and its first replacement token taken raw;
                // an expandable primitive executes once. Full get_token
                // recursion loops forever on self-appending chains
                // (\@preamblecmds: \ea\gdef\ea\@preamblecmds\ea{\@preamblecmds...}).
                let t2 = self.raw_token();
                // tex.web §507: a left brace as second token descends into
                // the group and expands its first token once
                // (\exp_args:No = \ea#1\ea{#2} relies on this).
                let descend = t2.is_char() && t2.cc() == 1;
                let t3 = if descend { self.raw_token() } else { t2 };
                if t3.is_cs() {
                    let mut id2 = t3.cs_id();
                    for _ in 0..1024 {
                        match self.eqtb.get(id2) {
                            Some(Equiv::Alias(next)) => id2 = *next,
                            _ => break,
                        }
                    }
                    match self.eqtb.get(id2).cloned() {
                        Some(Equiv::Macro(m)) if !m.protected => {
                            self.expand_macro(id2, &m);
                        }
                        Some(Equiv::Prim(p2)) if self.is_expandable(p2) => {
                            if let Some(tt) = self.expand_prim(p2, id2) {
                                self.pushed.push(tt);
                            }
                        }
                        _ => {
                            self.pushed.push(t3);
                        }
                    }
                } else {
                    self.pushed.push(t3);
                }
                if descend {
                    // push the brace AFTER the expansion so pop order is
                    // t1, {, expansion..., rest of group
                    self.pushed.push(t2);
                }
                if std::env::var("DEFWATCH").map(|v|v=="1").unwrap_or(false) {
                    let n0 = self.pushed.last().map(|x| format!("{:#x}", x.0)).unwrap_or_default();
                    eprintln!("XFAFTER pre_pushed_top={} post_pushed_top={}", pre, n0);
                }
                if std::env::var("DEFTRACE").map(|v|v=="1").unwrap_or(false) {
                    let n1 = if t1.is_cs() { std::string::String::from_utf8_lossy(self.cs.name(t1.cs_id())).into_owned() } else { format!("cc{}", t1.cc()) };
                    let n2 = if t2.is_cs() { std::string::String::from_utf8_lossy(self.cs.name(t2.cs_id())).into_owned() } else { format!("cc{}", t2.cc()) };
                    eprintln!("EXPANDAFTER t1={}({:#x}) t2={}({:#x})", n1, t1.0, n2, t2.0);
                }
                Some(t1)
            }
            NoExpand => {
                let t = self.raw_token();
                if t.is_cs() {
                    Some(Token(NOEXP_FLAG | t.cs_id()))
                } else {
                    Some(t)
                }
            }
            CsName => {
                let mut name: Vec<u8> = Vec::new();
                let dbg = self.input.current_file_line() == 2802
                    && self.input.current_file_name().contains("expl3-code");
                if dbg {
                    eprintln!("CSNAME-START macros={:?}", self.last_macros);
                }
                loop {
                    let t = self.get_token();
                    if t == EOF_MARKER {
                        break;
                    }
                    if dbg {
                        if t.is_cs() {
                            eprintln!("CSNAME-TOK cs=\\{} {:#x}", std::string::String::from_utf8_lossy(self.cs.name(t.cs_id())), t.0);
                        } else {
                            eprintln!("CSNAME-TOK cc={} chr={} {:#x}", t.cc(), t.chr(), t.0);
                        }
                    }
                    if t.is_cs() {
                        let is_end = matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(crate::prim::Prim::EndCsName)));
                        if is_end {
                            break;
                        }
                        let nm = std::string::String::from_utf8_lossy(self.cs.name(t.cs_id())).to_string();
                        let ek = match self.eqtb.resolve(t.cs_id()) { Some(e) => match e { Equiv::Macro(_) => "M", Equiv::Alias(_) => "A", Equiv::Prim(_) => "P", Equiv::CharTok(_) => "C", _ => "?" }.to_string(), None => "U".to_string() };
                        self.error(&format!("Missing \\endcsname inserted (collected={:?}, hit=\\{} prim={:?} at {}:{} macros={:?})", std::string::String::from_utf8_lossy(&name), nm, ek, self.input.current_file_name(), self.input.current_file_line(), self.last_macros));
                        break;
                    }
                    name.push(t.chr() as u8);
                }
                let id = self.cs.intern(&name);
                self.last_named_cs = Some(id);
                if self.eqtb.get(id).is_none() {
                    // undefined: become \relax
                    let relax = self.cs.lookup(b"relax").unwrap();
                    let r = self.eqtb.get(relax).cloned();
                    if let Some(e) = r {
                        self.eqtb.assign(id, e, true);
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
                    if n < 0 {
                        self.error("Roman numeral out of range");
                    }
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
                let toks = self.scan_general_text_expanded();
                if self.input.current_file_line() >= 1880 && self.input.current_file_line() < 2500 {
                    let s = self.tokens_to_string(&toks);
                    eprintln!("EXPANDED L{} n={} {}", self.input.current_file_line(), toks.len(), &s[..s.len().min(500)]);
                }
                self.push_tokens(toks);
                None
            }
            UnExpanded => {
                self.skip_spaces_relax();
                let t = self.raw_token();
                if t.is_cs() {
                    if let Some(Equiv::ToksReg(i)) = self.eqtb.resolve(t.cs_id()).cloned() {
                        let toks = (*self.eqtb.toks[i as usize]).clone();
                        self.push_tokens(Self::freeze_unexpanded_toks(toks));
                        return None;
                    }
                }
                self.pushed.push(t);
                let toks = self.scan_general_text();
                self.push_tokens(Self::freeze_unexpanded_toks(toks));
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
                let g = self.scan_expr_glue();
                let text = self.glue_to_string(&g);
                self.exp_string(text.as_bytes());
                None
            }
            MuExpr => {
                let g = self.scan_expr_glue();
                let text = self.glue_to_string(&g);
                self.exp_string(text.as_bytes());
                None
            }
            Prim::FontName => {
                let t = self.raw_token();
                let mut text = std::string::String::new();
                if t.is_cs() {
                    if let Some(Equiv::FontRef(f)) = self.eqtb.resolve(t.cs_id()).cloned() {
                        text = self.font_display_name(f);
                    }
                }
                self.exp_string(text.as_bytes());
                None
            }
            Prim::FontIdPrim => {
                let t = self.raw_token();
                let mut text = std::string::String::new();
                if t.is_cs() {
                    if let Some(Equiv::FontRef(f)) = self.eqtb.resolve(t.cs_id()).cloned() {
                        text = f.to_string();
                    }
                }
                self.exp_string(text.as_bytes());
                None
            }
            Unless => {
                let saved = self.unless_next;
                self.unless_next = true;
                let r = self.expand_prim_of_next();
                self.unless_next = saved;
                r
            }
            IfEOF => {
                let n = self.scan_int();
                let eof = self.read_eof.get(n.max(0) as usize).copied().unwrap_or(true);
self.do_if(eof)
            }
            IfTrue => self.do_if(true),
            IfFalse => self.do_if(false),
            IfChar => {
                let a = self.get_token();
                let b = self.get_token();
                let eq = if a.is_cs() && b.is_cs() {
                    true
                } else if !a.is_cs() && !b.is_cs() {
                    a.chr() == b.chr()
                } else {
                    false
                };
                self.do_if(eq)
            }
            IfCat => {
                let a = self.get_token();
                let b = self.get_token();
                let cat = |t: Token, e: &Self| -> u8 {
                    if t.is_cs() {
                        16
                    } else {
                        t.cc()
                    }
                };
                let eq = cat(a, self) == cat(b, self);
                self.do_if(eq)
            }
            IfOdd => {
                let n = self.scan_int();
                self.do_if(n % 2 != 0)
            }
            IfNum => {
                let a = self.scan_int();
                let rel = self.scan_relational();
                let b = self.scan_int();
                if std::env::var("IFNUMTRACE").map(|v|v=="1").unwrap_or(false) {
                    eprintln!("IFNUM {} {} {} -> {} line={} stack={}", a, rel as char, b, compare(a, rel, b), self.input.current_file_line(), self.if_stack.len());
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
                let def = t.is_cs() && self.eqtb.get(t.cs_id()).is_some();
                self.do_if(def)
            }
            IfCSName => {
                let mut name: Vec<u8> = Vec::new();
                loop {
                    let t = self.get_token();
                    if t == EOF_MARKER {
                        break;
                    }
                    if t.is_cs() {
                        let is_end = matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(crate::prim::Prim::EndCsName)));
                        if !is_end {
                            self.error("Missing \\endcsname inserted");
                        }
                        break;
                    }
                    name.push(t.chr() as u8);
                }
                let id = self.cs.intern(&name);
                self.last_named_cs = Some(id);
                let def = self.eqtb.get(id).is_some();
                self.do_if(def)
            }
            IfX => {
                let a = self.raw_token();
                let b = self.raw_token();
                let eq = self.ifx_equal(a, b);
                if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) {
                    let nm = |t: Token, e: &Self| -> ::std::string::String {
                        if t.is_cs() { ::std::string::String::from_utf8_lossy(e.cs.name(t.cs_id())).into_owned() }
                        else { ::std::format!("char {:#x}", t.0) }
                    };
                    eprintln!("IFX a={} b={} eq={}", nm(a, self), nm(b, self), eq);
                }
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
                if self.if_stack.is_empty() && std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) {
                    eprintln!("FIREACH t={:#x} csname={} line={} buf={:?}", self.cur_tok.0, self.input.current_file_line(), self.current_line_text(), if self.cur_tok.is_cs() { ::std::string::String::from_utf8_lossy(self.cs.name(self.cur_tok.cs_id())).into_owned() } else { ::std::string::String::new() });
                }
                if self.if_stack.pop().is_none() {
                    eprintln!(
                        "EXTRAFI line={} file={} macros={:?} ring={:?}",
                        self.input.current_file_line(),
                        self.input.current_file_name(),
                        self.last_macros,
                        self.tok_ring
                    );
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
                let n = self.font_loader.kpse.find_any(name)
                    .or_else(|| self.font_loader.kpse.find(name, tex_kpse::Format::Tex))
                    .and_then(|p| std::fs::metadata(p).ok())
                    .map(|m| m.len())
                    .unwrap_or(0);
                self.exp_string(n.to_string().as_bytes());
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

    fn do_if(&mut self, mut b: bool) -> Option<Token> {
        if self.unless_next {
            b = !b;
            self.unless_next = false;
        }
        if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) {
            let src = match self.input.stack.last() { Some(crate::input::Source::TokList { name, pos, toks, .. }) => format!("T:{} {}/{}", name, pos, toks.len()), _ => "F".into() };
            eprintln!("IFPUSH accepting={} line={} depth->{} src={} tok={:#x}", b, self.input.current_file_line(), self.if_stack.len()+1, src, self.cur_tok.0);
        }
        self.if_stack.push(crate::engine::IfState {
            accepting: b,
            matched: b,
            if_case: -1,
            loc_file: self.input.current_file_name(),
            loc_line: self.input.current_file_line(),
            loc_cs: 0,
        });
        if !b {
            self.skip_branch(false);
        }
        None
    }


    /// tex.web: \unless modifies the *following* conditional; during skipping the
    /// pair counts as ONE opener (otherwise branch-skips over `\unless\ifnum...`
    /// double-count and overshoot to the wrong \fi).
    fn skip_count_unless_target(&mut self, depth: &mut i32) {
        loop {
            let t2 = self.raw_token();
            if t2 == EOF_MARKER || t2 == crate::input::PAR_END {
                return;
            }
            if !t2.is_cs() {
                continue;
            }
            if let Some(Equiv::Prim(p)) = self.eqtb.resolve(t2.cs_id()) {
                use Prim::*;
                if matches!(
                    p,
                    IfChar | IfCat | IfOdd | IfNum | IfDim | IfVoid | IfHBox | IfVBox
                        | IfHMode | IfVMode | IfInner | IfMMode | IfTrue | IfFalse
                        | IfEOF | IfDef | IfCSName | IfX | IfCase | Unless
                ) {
                    *depth += 1;
                }
            }
            return;
        }
    }

    /// skip tokens until matching \else / \or (for ifcase) / \fi at this level
    fn skip_branch(&mut self, if_case: bool) {
        if std::env::var("SKIPTRACE").map(|v| v == "1").unwrap_or(false) {
            eprintln!("SKIPSTART ifcase={}", if_case);
        }
        let mut depth = 0i32;
        loop {
            let t = self.raw_token();
            if t == EOF_MARKER {
                let ist: Vec<std::string::String> = self.if_stack.iter().map(|st| format!("{}:{}", st.loc_file.split('/').last().unwrap_or("?"), st.loc_line)).collect();
                self.error(&format!("File ended while scanning conditional [{}]", ist.join(", ")));
                self.end_occurred = true;
                if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) { eprintln!("ENDOCC crates/tex-core/src/expand.rs:686 line={}", self.input.current_file_line()); }
                return;
            }
            if std::env::var("SKIPTRACE").map(|v| v == "1").unwrap_or(false) {
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
                | Prim::IfCSName | Prim::IfX | Prim::IfCase => depth += 1,
                Prim::Unless => self.skip_count_unless_target(&mut depth),
                Prim::Fi => {
                    if depth == 0 {
                        self.if_stack.pop();
                        return;
                    }
                    depth -= 1;
                }
                Prim::Or => {
                    if depth == 0 {
                        // handled by caller for ifcase
                        if if_case {
                            let st = self.if_stack.last_mut().unwrap();
                            if st.matched {
                                self.skip_case_skip();
                                return;
                            }
                            st.if_case -= 1;
                            let acc = st.if_case == 0;
                            st.accepting = acc;
                            st.matched = acc || st.matched;
                            if !acc {
                                self.skip_case_skip();
                            }
                            return;
                        }
                        { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/expand.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                        return;
                    }
                }
                Prim::ElIf | Prim::ElIfX => {
                    if depth == 0 {
                        // LaTeX rarely uses \elseif; treat as skip
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
            if std::env::var("SKIPTRACE").map(|v| v == "1").unwrap_or(false) {
                let t0 = self.raw_token();
                eprintln!("SKIPFI tok {:#x} cs={:?} depth={}", t0.0, if t0.is_cs() { Some(String::from_utf8_lossy(self.cs.name(t0.cs_id())).into_owned()) } else { None }, depth);
                { let __pt = t0; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/expand.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
                | Some(Equiv::Prim(Prim::IfX))
                | Some(Equiv::Prim(Prim::IfCase)) => depth += 1,
                Some(Equiv::Prim(Prim::Unless)) => self.skip_count_unless_target(&mut depth),
                Some(Equiv::Prim(Prim::Fi)) => {
                    if depth == 0 {
                        self.if_stack.pop();
                        return;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
    }

    /// skip to \or or \fi within an ifcase that already matched
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
                    | Prim::IfFalse | Prim::IfDef | Prim::IfCSName | Prim::IfX
                    | Prim::IfCase => depth += 1,
                    Prim::Unless => self.skip_count_unless_target(&mut depth),
                    Prim::Fi => {
                        if depth == 0 {
                            self.if_stack.pop();
                            return;
                        }
                        depth -= 1;
                    }
                    Prim::Or => {
                        if depth == 0 {
                            return;
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    // ---------- macro expansion ----------

    pub fn expand_macro(&mut self, id: CsId, m: &Macro) {
        if std::env::var("OFWOTRACE").map(|v|v=="1").unwrap_or(false)
            && self.cs.name(id) == b"@onefilewithoptions" {
            let st: std::string::String = self.input.stack.iter().rev().take(4).map(|src| match src {
                crate::input::Source::TokList { name, pos, toks, .. } => format!("T:{} {}/{}", name, pos, toks.len()),
                crate::input::Source::File { name, line_no, .. } => format!("F:{}", line_no),
            }).collect::<Vec<_>>().join(" << ");
            eprintln!("OFWO-CALLED stack=[{}]", st);
        }
        self.current_macro = String::from_utf8_lossy(self.cs.name(id)).into_owned();
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
            // parameter text before the first #: match and discard
            self.scan_delimited(&m.prefix, m.long);
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
                    if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) { eprintln!("ENDOCC crates/tex-core/src/expand.rs:897 line={}", self.input.current_file_line()); }
                    args.push(Vec::new());
                    continue;
                }
                if std::env::var("ARGTRACE").map(|v|v=="1").unwrap_or(false) {
                    let srcs: Vec<std::string::String> = self.input.stack.iter().rev().take(3).map(|src| match src {
                        crate::input::Source::TokList { name, pos, toks, .. } => format!("T:{} {}/{}", name, pos, toks.len()),
                        crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name, line_no),
                    }).collect();
                    let tname = if t.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) } else { format!("cc{}", t.cc()) };
                    let ring: Vec<std::string::String> = self.tok_ring.iter().rev().take(14).map(|(v, ln)| format!("{:#x}@{}", v, ln)).collect();
                    eprintln!("ARG \\{} #{}/{} t={} pushed={:?} srcs=[{}] ring=[{}]", String::from_utf8_lossy(self.cs.name(id)), i + 1, m.num_params, tname, self.pushed.iter().rev().take(3).map(|x| format!("{:#x}", x.0)).collect::<Vec<_>>(), srcs.join(" << "), ring.join(" "));
                }
                if t.is_char() && t.cc() == 1 {
                    // braced group
                    let arg = self.scan_balanced_raw();
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
        let name_bytes = self.cs.name(id).to_vec();
        let nm = &name_bytes;
        self.last_macros.push_back(String::from_utf8_lossy(&name_bytes).into_owned());
        if nm == b"@missingfileerror" {
            eprintln!("MFE ARGS: {} STACK: {}", args.iter().map(|a| format!("[{}]", self.tokens_to_string(a))).collect::<Vec<_>>().join(" "),
                self.input.stack.iter().rev().take(3).map(|src| match src {
                    crate::input::Source::TokList { name, .. } => format!("T:{}", name),
                    crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name, line_no),
                }).collect::<Vec<_>>().join(" << "));
        }
        while self.last_macros.len() > 12 {
            self.last_macros.pop_front();
        }
        if nm == b"__prg_generate_conditional:nnNNNnnn"
            || nm == b"__prg_generate_conditional_parm:NNNpnn"
            || nm == b"prg_gset_conditional:Npnn"
            || nm == b"cs_split_function:N"
            || nm == b"__prg_generate_conditional:NNnnnnNw"
            || nm == b"__prg_generate_p_form:wNNnnnnN"
            || nm == b"__prg_generate_T_form:wNNnnnnN"
            || nm == b"__prg_generate_F_form:wNNnnnnN"
            || nm == b"__prg_generate_TF_form:wNNnnnnN"
            || nm == b"__prg_generate_conditional_test:w"
            || nm == b"__prg_generate_conditional_fast:nw"
            || nm == b"__cs_tmp:w"
            || nm == b"exp_args:Nc"
            || nm == b"cs_new_protected:cpx"
            || nm == b"cs_new_protected:cpe"
            || nm == b"cs_new_protected:Npe"
        {
            let line = self.input.current_file_line();
            if line >= 2260 && line <= 2980 {
                eprintln!(
                    "GEN \\{} L{} args={}",
                    String::from_utf8_lossy(nm),
                    line,
                    args.iter().map(|a| format!("[{}]", self.tokens_to_string(a))).collect::<Vec<_>>().join(" ")
                );
            }
        }
        if nm == b"__quark_new_test_aux:nnNNnnnn"
            || nm == b"__quark_new_test_aux:Nn"
            || nm == b"__quark_new_test:NNNn"
            || nm == b"__quark_new_test:Nccn"
            || nm == b"cs_split_function:N"
            || nm == b"__quark_module_name:N"
            || nm == b"__quark_module_name:w"
            || nm == b"__kernel_quark_new_test:N"
            || nm == b"exp_last_unbraced:Nf"
            || nm == b"__quark_new_test_nN:Nnnn"
            || nm == b"cs_to_str:N"
        {
            eprintln!(
                "Q \\{} L{} args={}",
                String::from_utf8_lossy(nm),
                self.input.current_file_line(),
                args.iter().map(|a| format!("[{}]", self.tokens_to_string(a))).collect::<Vec<_>>().join(" ")
            );
        }
        if nm == b"__quark_new_test_aux:Nn" && args.len() >= 2 {
            let dump: Vec<String> = args[1].iter().map(|t| {
                if t.is_cs() {
                    format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id())))
                } else {
                    format!("{}:{}", t.cc(), t.chr() as u8 as char)
                }
            }).collect();
            eprintln!("QCAT #2={}", dump.join(" "));
        }
        if nm.starts_with(b"msg_error") || nm.starts_with(b"__kernel_msg") || nm.starts_with(b"__msg_") {
            if std::env::var("MSGTRACE").map(|v|v=="1").unwrap_or(false) {
                eprintln!(
                    "MSGEXEC \\{} L{} args={}",
                    String::from_utf8_lossy(nm),
                    self.input.current_file_line(),
                    args.iter().map(|a| format!("[{}]", self.tokens_to_string(a))).collect::<Vec<_>>().join(" ")
                );
            }
        }
        if std::env::var("IFINTRACE").map(|v|v=="1").unwrap_or(false)
            && (nm.starts_with(b"tl_if_in") || nm.starts_with(b"str_if_in") || nm == b"__tl_tmp:w") {
            eprintln!(
                "IFIN \\{} L{} args={}",
                String::from_utf8_lossy(nm),
                self.input.current_file_line(),
                args.iter().map(|a| format!("[{}]", self.tokens_to_string(a))).collect::<Vec<_>>().join(" ")
            );
        }
        let dbg_mod = matches!(
            &nm[..],
            b"@IncludeInRelease" | b"kernel@ifnextchar" | b"@ifnch" | b"@xifnch"
                | b"@execifchar" | b"IncludeInRelease" | b"EndIncludeInRelease"
                | b"@gobble@IncludeInRelease" | b"@onlypreamble"
        );
        if dbg_mod {
            eprintln!(
                "MOD {} L{} args={}",
                String::from_utf8_lossy(&name_bytes), self.input.current_file_line(),
                args.iter().map(|a| self.tokens_to_string(a)).collect::<Vec<_>>().join(" | ")
            );
        }
        if nm == b"@IncludeInRele@se" {
            let st: Vec<String> = self.input.stack.iter().rev().take(4).map(|src| match src {
                crate::input::Source::TokList { name, .. } => format!("T:{}", name),
                crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name, line_no),
            }).collect();
            eprintln!("IIR STACK: {} MACROS: {:?} ARGS0: {}", st.join(" << "), self.last_macros, self.tokens_to_string(&args[0]));
        }
        if std::env::var("MACTRACE").map(|v| v == "1").unwrap_or(false) {
            let argstr: Vec<String> = args.iter().map(|a| self.tokens_to_string(a)).collect();
            eprintln!("MAC \\{} args={:?}", String::from_utf8_lossy(nm), argstr);
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
        if std::env::var("BODYDUMP").map(|v|v=="1").unwrap_or(false) && name_bytes == b"e@alloc" {
            let dump: Vec<String> = spliced.iter().enumerate().map(|(i,t)| {
                if t.is_cs() { format!("{}:{}", i, String::from_utf8_lossy(self.cs.name(t.cs_id()))) }
                else { format!("{}:{}{}", i, t.cc(), t.chr() as u8 as char) }
            }).collect();
            eprintln!("BODYDUMP {}", dump.join(" "));
        }
        self.push_tokens(spliced);
        self.mac_depth -= 1;
    }
    pub fn skip_raw_spaces(&mut self) {
        loop {
            let t = self.raw_token();
            if t.is_char() && t.cc() == 10 {
                continue;
            }
            { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/expand.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
            return;
        }
    }

    /// scan a balanced group (the opening brace already consumed);
    /// returns tokens without the outer braces
    pub fn scan_balanced_raw(&mut self) -> Vec<Token> {
        let mut depth = 1i32;
        let mut out = Vec::new();
        loop {
            let t = self.raw_token();
            if std::env::var("SBTRACE").map(|v|v=="1").unwrap_or(false) {
                let tn = if t.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) } else { format!("cc{}:{:#x}", t.cc(), t.chr()) };
                eprintln!("SB depth={} t={} file={}:{}", depth, tn, self.input.current_file_name(), self.input.current_file_line());
            }
            if t == EOF_MARKER || t == PAR_END {
                self.error("Runaway argument / missing }");
                if t == EOF_MARKER {
                    self.end_occurred = true;
                }
                return out;
            }
            if t.is_char() {
                let cc = t.cc();
                if cc == 1 {
                    depth += 1;
                } else if cc == 2 {
                    depth -= 1;
                    if depth == 0 {
                        if std::env::var("SBTRACE").map(|v|v=="1").unwrap_or(false) && out.iter().any(|x| x.0 == 0x80001482) {
                            eprintln!("SBOUT n={} [{}]", out.len(), out.iter().map(|t| format!("{:#x}", t.0)).collect::<Vec<_>>().join(","));
                        }
                        return out;
                    }
                }
            }
            out.push(t);
        }
    }


    /// scan until the delimiter token sequence matches
    fn scan_delimited(&mut self, delim: &[Token], long: bool) -> Vec<Token> {
        if std::env::var("DELIMTRACE").map(|v|v=="1").unwrap_or(false) {
            eprintln!("DELIMSCAN macro={} delim=[{}] long={}", self.current_macro,
                delim.iter().map(|d| format!("{:#x}", d.0)).collect::<Vec<_>>().join(","), long);
        }
        let mut arg: Vec<Token> = Vec::new();
        let mut di = 0usize;
        loop {
            let t = self.raw_token();
            if std::env::var("DELIMSTEP").map(|v|v=="1").unwrap_or(false) {
                let tn = if t.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) } else { format!("cc{}:{:#x}", t.cc(), t.chr()) };
                eprintln!("DSTEP macro={} di={} t={} ({:#x})", self.current_macro, di, tn, t.0);
            }
            if t == EOF_MARKER {
                if std::env::var("DELIMTRACE").map(|v|v=="1").unwrap_or(false) {
                    eprintln!("DELIMRUN macro={} delim=[{}] collected={}", self.current_macro,
                        delim.iter().map(|d| format!("{:#x}", d.0)).collect::<Vec<_>>().join(","),
                        arg.iter().map(|d| format!("{:#x}", d.0)).collect::<Vec<_>>().join(","));
                }
                self.error("Runaway argument");
                self.end_occurred = true;
                if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) { eprintln!("ENDOCC crates/tex-core/src/expand.rs:1003 line={}", self.input.current_file_line()); }
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
                if d.is_char() && d.cc() == 1 {
                    // #{ convention: { delimits the last param and is reread
                    di += 1;
                    if di >= delim.len() {
                        self.pushed.push(t);
                        return arg;
                    }
                    continue;
                }
                let inner = self.scan_balanced_raw();
                arg.push(Token::char(1, b'{' as u32));
                arg.extend(inner);
                arg.push(Token::char(2, b'}' as u32));
            } else if t == d {
                di += 1;
                if di >= delim.len() {
                    // strip outer braces if whole arg was braced
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
            } else {
                arg.push(t);
            }
        }
    }

    // ---------- helpers ----------

    /// put a byte string as cat-12 char tokens into the expansion stream
    /// (chronologically on top: newer than any earlier pushback)
    pub fn exp_string(&mut self, bytes: &[u8]) {
        let toks: Vec<Token> = bytes.iter().map(|&b| Token::other(b)).collect();
        self.push_tokens(toks);
    }

    pub fn tokens_to_string(&self, toks: &[Token]) -> String {
        let mut out: Vec<u8> = Vec::new();
        let esc = self.eqtb.int_params[crate::prim::IntParam::EscapeChar.idx() as usize];
        for t in toks {
            if t.0 >= PAR_REF_FLAG && t.0 < 0xFFFF_0000 && !t.is_cs() {
                // param reference token: render as #n
                out.push(b'#');
                out.push(b'0' + (t.0 & 0xF) as u8);
                continue;
            }
            if t.is_cs() {
                if esc >= 0 && esc <= 255 {
                    out.push(esc as u8);
                }
                out.extend_from_slice(self.cs.name(t.cs_id()));
                let n = self.cs.name(t.cs_id());
                if n.len() > 1 {
                    out.push(b' ');
                }
            } else {
                out.push(t.chr() as u8);
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    pub fn ifx_equal(&self, a: Token, b: Token) -> bool {
        if a.is_cs() != b.is_cs() {
            return false;
        }
        if !a.is_cs() {
            return a == b;
        }
        let ma = self.eqtb.resolve(a.cs_id()).cloned();
        let mb = self.eqtb.resolve(b.cs_id()).cloned();
        match (ma, mb) {
            (None, None) => true,
            (Some(Equiv::Macro(x)), Some(Equiv::Macro(y))) => {
                x.num_params == y.num_params
                    && x.prefix == y.prefix
                    && x.params == y.params
                    && x.body == y.body
                    && x.long == y.long
                    && x.outer == y.outer
                    && x.protected == y.protected
            }
            (Some(Equiv::Prim(x)), Some(Equiv::Prim(y))) => x == y,
            (Some(Equiv::CharTok(v)), Some(Equiv::CharTok(w))) => v == w,
            (Some(Equiv::CharTok(v)), None) if !b.is_cs() => Token(v) == b,
            (None, Some(Equiv::CharTok(w))) if !a.is_cs() => Token(w) == a,
            (Some(x), Some(y)) => {
                // same variant+value approximates TeX's entry identity
                match (&x, &y) {
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
                }
            }
            _ => false,
        }
    }

    pub fn error(&mut self, msg: &str) {
        let mut ctx = String::new();
        for s in self.input.stack.iter().rev().take(2) {
            match s {
                crate::input::Source::TokList { name, pos, toks, .. } => {
                    ctx.push_str(&format!(" [{} {}/{} rest={:?}]", name, pos, toks.len(),
                        toks[(*pos).min(toks.len())..].iter().take(6).map(|t| format!("{:#x}", t.0)).collect::<Vec<_>>()))
                }
                crate::input::Source::File { name, line_no, .. } => {
                    ctx.push_str(&format!(" [{} line {}]", name, line_no))
                }
            }
        }
        self.term.push_str(&format!(
            "! {} at line {}{}\n",
            msg,
            self.input.current_file_line(),
            ctx
        ));
        self.error_count += 1;
        if self.error_count > 10000 {
            self.end_occurred = true;
            if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) { eprintln!("ENDOCC crates/tex-core/src/expand.rs:1152 line={}", self.input.current_file_line()); }
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
