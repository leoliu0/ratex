//! Main control: dispatch of unexpandable tokens; assignments (def/let/
//! registers/parameters); box and list building; paragraph triggers.

use crate::eqtb::{Equiv, LevelType, Macro};
use crate::engine::{Engine, Mode, ScannerStatus};
use crate::boxes::{Glue, Node};
use crate::expand::PAR_REF_FLAG;
use crate::prim::*;
use crate::token::{CsId, Token};
use std::rc::Rc;

impl Engine {
    pub fn run(&mut self) {
        self.main_loop();
    }

    pub fn main_loop(&mut self) {
        while !self.end_occurred {
            let t = self.get_token();
            if t == crate::input::EOF_MARKER {
                break;
            }
            self.dispatch(t);
        }
    }

    pub fn dispatch(&mut self, t: Token) {
        let ln = self.input.current_file_line();
        let fnm = self.input.current_file_name();

        if t == crate::input::EOF_MARKER {
            return;
        }
        if t == crate::page::OUT_END_TOKEN {
            self.finish_output();
            return;
        }
        if t.is_cs() && (t.cs_id() as usize) >= self.cs.len() {
            return;
        }

        if t.is_cs() {
            let id = t.cs_id();
            if let Some(Equiv::FontRef(f)) = self.eqtb.resolve(id).cloned() {
                if crate::debug_flag("FONTWATCH") {
                    let nm = self
                        .eqtb
                        .fonts
                        .get(f as usize)
                        .map(|x| x.tfm_name.clone())
                        .unwrap_or_default();
                    if nm.starts_with("cmsy") || nm.starts_with("cmmi") || nm.starts_with("cmex") {
                        let src = match self.input.stack.last() {
                            Some(crate::input::Source::TokList { name, .. }) => name.clone(),
                            Some(crate::input::Source::File { name, .. }) => format!("F:{}", name),
                            None => String::new(),
                        };
                        eprintln!(
                            "FONTWATCH \\{} -> {} id={} line={} file={} src={} mode={:?}",
                            String::from_utf8_lossy(self.cs.name(id)),
                            nm,
                            f,
                            ln,
                            fnm,
                            src,
                            self.mode
                        );
                    }
                }
                // tex.web set_font: a group-scoped assignment that also
                // consumes any \global prefix
                let g = self.global_flag;
                self.eqtb.define_cur_font(f, g);
                self.clear_prefixes();
                self.space_factor = 1000;
                return;
            }
            // register alias assignment target (\countdef'd cs etc.)
            match self.eqtb.resolve(id) {
                Some(Equiv::CountReg(_))
                | Some(Equiv::DimenReg(_))
                | Some(Equiv::SkipReg(_))
                | Some(Equiv::MuSkipReg(_))
                | Some(Equiv::ToksReg(_)) => {
                    self.cs_assign(id);
                    self.trigger_after_assignment();
                    return;
                }
                _ => {}
            }
            // assignment prefixes
            if let Some(Equiv::Prim(p)) = self.eqtb.resolve(id).cloned() {
                match self.try_assignment(p, id) {
                    true => {
                        if !matches!(p, Prim::Global | Prim::Long | Prim::Outer | Prim::Protected | Prim::AfterAssignment) {
                            self.trigger_after_assignment();
                        }
                        return;
                    }
                    false => {}
                }
                if self.global_flag || self.long_flag || self.outer_flag || self.protected_flag {
                    // These assignments are executed by main_dispatch but
                    // still honor \global/\long/... prefixes (tex.web §407:
                    // prefixes persist until the assignment consumes them).
                    // Clearing here dropped \global\font inside NFSS
                    // \define@newfont groups, so \endgroup reverted font
                    // CSes to \relax.
                    if !matches!(
                        p,
                        Prim::Font
                            | Prim::TextFont
                            | Prim::ScriptFont
                            | Prim::ScriptScriptFont
                            | Prim::CatCode
                            | Prim::MathCode
                            | Prim::DelCode
                            | Prim::LcCodeP
                            | Prim::SfCodeP
                            | Prim::UcCodeP
                    ) {
                        self.clear_prefixes();
                    }
                }
                self.main_dispatch(p, id);
            } else {
                // non-primitive cs used as value: usually error
                match self.eqtb.resolve(id).cloned() {
                    None => {
                        let name_bytes = self.cs.name(id).to_vec();
                        if name_bytes == b"@@italiccorr" || name_bytes == b"/" {
                            self.eqtb.assign(id, Equiv::Prim(Prim::Relax), true);
                            return;
                        }
                        // tex.web \S358 tokenizes escape+space as a plain
                        // spacer token, never a cs. Formats dumped before
                        // that fix carry `\ ` as an (undefined) cs [0x20];
                        // dispatching it must yield the space it would have
                        // been: push back a real space token and re-dispatch.
                        if name_bytes == [0x20] {
                            self.pushed.push(Token::char(10, 0x20));
                            return;
                        }
                        if name_bytes.contains(&b'_') {
                            eprintln!("UNDEF-DISPATCH \\{} line={} file={}", String::from_utf8_lossy(&name_bytes), self.input.current_file_line(), self.input.current_file_name());
                        }
                        // l3 variant wrappers reference \exp_args:N<spec>
                        // expanders lazily (\exp_not:c{exp_args:NNcc}); a
                        // boot that has not generated the spec yet would see
                        // the expander as undefined and pass c-args through
                        // as raw character groups (expl3 quark/variant
                        // generation dies). Synthesize the standard expander
                        // for N[nc]* specs on first use: \expanded{ \exp_not:N
                        // #1 (\cs:w #j \cs_end: for c) ({#j} for n) ... }.
                        let name: Vec<u8> = self.cs.name(id).to_vec();
                        if name.starts_with(b"exp_args:N")
                            && name.len() > 10
                            && name[10..].iter().all(|c| matches!(c, b'N' | b'n' | b'c'))
                        {
                            let spec = &name[10..];
                            let k = spec.len() as u8;
                            let mut body: Vec<Token> = Vec::new();
                            if let (Some(&ex), Some(&noex), Some(&csw), Some(&cse)) = (
                                self.cs.prim_ids.get("expanded"),
                                self.cs.prim_ids.get("noexpand"),
                                self.cs.prim_ids.get("csname"),
                                self.cs.prim_ids.get("endcsname"),
                            ) {
                                body.push(Token::from_cs(ex));
                                body.push(Token::char(1, b'{' as u32));
                                for (i, letter) in spec.iter().enumerate() {
                                    let pref = PAR_REF_FLAG | (i as u32 + 1);
                                    match letter {
                                        b'N' => body.push(Token::from_cs(noex)),
                                        b'n' => body.push(Token::char(1, b'{' as u32)),
                                        b'c' => body.push(Token::from_cs(csw)),
                                        _ => {}
                                    }
                                    body.push(Token(pref));
                                    match letter {
                                        b'N' => {}
                                        b'n' => body.push(Token::char(2, b'}' as u32)),
                                        b'c' => body.push(Token::from_cs(cse)),
                                        _ => {}
                                    }
                                }
                                body.push(Token::char(2, b'}' as u32));
                                let m = crate::eqtb::Macro {
                                    num_params: k,
                                    params: vec![Vec::new(); k as usize],
                                    prefix: Vec::new(),
                                    body,
                                    long: false,
                                    outer: false,
                                    protected: false,
                                };
                                self.eqtb.assign(id, Equiv::Macro(std::rc::Rc::new(m)), true);
                                if let Some(Equiv::Macro(m)) = self.eqtb.resolve(id).cloned() {
                                    self.expand_macro(id, &m);
                                }
                                return;
                            }
                        }
                        let name = String::from_utf8_lossy(self.cs.name(id)).into_owned();
                        if name == "::x"
                            || name == "q__cs_nil"
                            || name.contains("msg_expandable_error")
                            || name == "exp_args:Noooo"
                        {
                            let st: Vec<String> = self
                                .input
                                .stack
                                .iter()
                                .rev()
                                .take(8)
                                .map(|src| match src {
                                    crate::input::Source::TokList {
                                        name, pos, toks, ..
                                    } => format!("T:{} {}/{}", name, pos, toks.len()),
                                    crate::input::Source::File { name, line_no, .. } => {
                                        format!(
                                            "F:{}#{}",
                                            name.split('/').last().unwrap_or(name),
                                            line_no
                                        )
                                    }
                                })
                                .collect();
                            eprintln!(
                                "UNDEF \\{} L{} file={} macros={:?} ifs={} level={} stack=[{}]",
                                name,
                                self.input.current_file_line(),
                                self.input.current_file_name(),
                                self.last_macros,
                                self.if_stack.len(),
                                self.eqtb.cur_level,
                                st.join(" << ")
                            );
                        }
                        if std::env::var("UNDEFTRACE").is_ok() {
                            eprintln!(
                                "UNDEFX name={:?} bytes={:02x?} line={} file={}",
                                name,
                                name_bytes,
                                self.input.current_file_line(),
                                self.input.current_file_name()
                            );
                        }
                        self.error(&format!("Undefined control sequence \\{}", name));
                    }
                    Some(Equiv::Macro(_)) => {
                        if crate::debug_flag("DEFWATCH") {
                            eprintln!("DEFWATCH-SILENT-DISPATCH \\{} L{}", String::from_utf8_lossy(self.cs.name(id)), self.input.current_file_line());
                        }
                        // get_token already declined to expand this (\noexpand
                        // freeze, \protected in an edef scan, self-quark stop
                        // marker). Knuth treats frozen dont_expand as \relax.
                        // Re-expanding here loops: self-quark terminators
                        // (\q__tl_recursion_tail) never stop expl3 maps.
                    }
                    Some(Equiv::CharDef(v)) => {
                        self.char_token(v as u8, false);
                    }

                    Some(Equiv::MathCharDef(v)) if self.mode.is_m() => {
                        self.append_mathchar(v as u16);
                    }
                    Some(Equiv::CharTok(v)) => {
                        self.dispatch(Token(v));
                    }
                    other => {
                        if crate::debug_flag("IFTRACE") {
                            let name = String::from_utf8_lossy(self.cs.name(id)).into_owned();
                            eprintln!("SILENT-DISPATCH cs=\\{} equiv={:?}", name, other.map(|e| e.kind_name()));
                        }
                    }
                }
            }
        } else {
            if self.global_flag || self.long_flag || self.outer_flag || self.protected_flag {
                self.clear_prefixes();
            }
            let cc = t.cc();
            let c = t.chr() as u8;
            match cc {
                1 => self.begin_group(true),
                2 => self.end_group(),
                3 => {
                    // math shift
                    if self.mode.is_m() {
                        self.exit_math();
                    } else if self.mode.is_v() {
                        let next = self.get_token();
                        if next.is_char() && next.cc() == 3 {
                            self.enter_math(true);
                        } else {
                            if next != crate::input::EOF_MARKER {
                                self.pushed.push(next);
                            }
                            // tex.web §1090: single math_shift in vertical mode starts a
                            // paragraph; the math_shift is put back on input so it
                            // executes AFTER \everypar.
                            self.pushed.push(t);
                            self.start_paragraph(true);
                        }
                    } else {
                        self.enter_math(false);
                    }
                }
                4 => self.align_tab(),
                10 => self.hspace_token(),
                13 => self.active_char(c),
                11 | 12 => self.char_token(c, cc == 11),
                5 | 7 | 8 => {
                    if cc == 7 {
                        self.super_token(c);
                    } else if cc == 8 {
                        self.sub_token(c);
                    } else {
                        // endline in odd places: treat as space
                        self.hspace_token();
                    }
                }
                0 | 6 | 9 | 14 | 15 => {
                    // escape/param/ignored/comment/invalid should not appear raw
                }
                _ => {}
            }
        }
    }

    pub fn clear_prefixes(&mut self) {
        self.global_flag = false;
        self.long_flag = false;
        self.outer_flag = false;
        self.protected_flag = false;
    }

    /// Handle assignment-prefix primitives; returns true if consumed.
    pub fn try_assignment(&mut self, p: Prim, id: CsId) -> bool {
        use Prim::*;
        match p {
            AfterAssignment => {
                let tok = self.raw_token();
                self.after_assignment = Some(tok);
                true
            }
            AfterGroup => {
                let tok = self.raw_token();
                self.eqtb.save_stack.push(crate::eqtb::SaveItem::AfterGroup(tok));
                true
            }
            Global => {
                self.global_flag = true;
                true
            }
            Def | GDef | EDef | XDef => {
                self.do_def(p, id);
                true
            }
            Let => {
                self.do_let(false);
                true
            }
            FutureLet => {
                self.do_let(true);
                true
            }
            Long => {
                self.long_flag = true;
                true
            }
            Outer => {
                self.outer_flag = true;
                true
            }
            Protected => {
                self.protected_flag = true;
                true
            }
            Advance => {
                self.do_advance();
                true
            }
            Multiply => {
                self.do_arith(1);
                true
            }
            Divide => {
                self.do_arith(2);
                true
            }
            SetBox => {
                self.do_setbox();
                true
            }
            Count => {
                let idx = self.scan_reg_num();
                self.scan_optional_equals();
                let v = self.scan_int();
                self.eqtb.assign_count(idx, v, self.global_flag);
                self.clear_prefixes();
                true
            }
            Dimen => {
                let idx = self.scan_reg_num();
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                self.eqtb.assign_dimen(idx, v, self.global_flag);
                self.clear_prefixes();
                true
            }
            Skip => {
                let idx = self.scan_reg_num();
                self.scan_optional_equals();
                let v = self.scan_glue(false);
                self.eqtb.assign_skip(idx, v, self.global_flag);
                self.clear_prefixes();
                true
            }
            MuSkip => {
                let idx = self.scan_reg_num();
                self.scan_optional_equals();
                let v = self.scan_glue(true);
                self.eqtb.assign_muskip(idx, v, self.global_flag);
                self.clear_prefixes();
                true
            }
            Wd | Ht | Dp => {
                self.do_box_dimen_assign(match p {
                    Prim::Wd => 0,
                    Prim::Ht => 1,
                    _ => 2,
                });
                true
            }
            Toks => {
                let idx = self.scan_reg_num();
                self.scan_optional_equals();
                let toks = Rc::new(self.scan_token_list());
                self.eqtb.assign_toks_reg(idx, toks, self.global_flag);
                self.clear_prefixes();
                true
            }
            Box => {
                // \box<n> in value position handled in scan paths; in main
                // position it's an error unless followed by use
                let idx = self.scan_reg_num();
                let b = self.eqtb.boxed.get(idx as usize).cloned().flatten();
                self.eqtb.assign_box(idx, None, true);
                self.append_box_node(b);
                true
            }
            Copy => {
                let idx = self.scan_reg_num();
                let b = self.eqtb.boxed.get(idx as usize).cloned().flatten();
                self.append_box_node(b);
                true
            }
            CountDef => {
                if crate::debug_flag("DEFTRACE") { eprintln!("COUNTDEF call"); }
                self.do_def_register(|e, idx| Equiv::CountReg(idx)); true
            }
            DimenDef => {
                if crate::debug_flag("DEFTRACE") { eprintln!("DIMENDEF call"); }
                self.do_def_register(|e, idx| Equiv::DimenReg(idx)); true
            }
            SkipDef => { self.do_def_register(|e, idx| Equiv::SkipReg(idx)); true }
            MuSkipDef => { self.do_def_register(|e, idx| Equiv::MuSkipReg(idx)); true }
            ToksDef => { self.do_def_register(|e, idx| Equiv::ToksReg(idx)); true }
            CharDef => {
                // tex.web: \chardef is global
                let t = self.scan_definable_cs();
                if crate::debug_flag("DEFWATCH")
                    && self.cs.name(t) == b"def" {
                    eprintln!("DEFWATCH: \\chardef targeting \\def at line {}", self.input.current_file_line());
                }
                self.scan_optional_equals();
                let v = self.scan_int();
                self.eqtb.assign(t, Equiv::CharDef(v.max(0) as u32), true);
                self.clear_prefixes();
                true
            }
            MathCharDef => {
                // tex.web: \mathchardef is global
                let t = self.scan_definable_cs();
                self.scan_optional_equals();
                let v = self.scan_int();
                self.eqtb.assign(t, Equiv::MathCharDef(v as u16), true);
                self.clear_prefixes();
                true
            }
            FontDimen => {
                let idx = self.scan_int();
                let f = self.scan_font_id();
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                if idx > 0 {
                    // tex.web: \fontdimen/\hyphenchar/\skewchar are always global
                    self.eqtb.assign_font_param(f, idx as usize - 1, v, true);
                }
                self.clear_prefixes();
                true
            }
            HyphenChar => {
                let f = self.scan_font_id();
                self.scan_optional_equals();
                let v = self.scan_int();
                self.eqtb.assign_hyphen_char(f, v, true);
                self.clear_prefixes();
                true
            }
            SkewChar => {
                let f = self.scan_font_id();
                self.scan_optional_equals();
                let v = self.scan_int();
                self.eqtb.assign_skew_char(f, v, true);
                self.clear_prefixes();
                true
            }
            ParShape => {
                // tex.web §1070: \parshape n i1 w1 ... in wn; n<=0 clears.
                self.scan_optional_equals();
                let n = self.scan_int();
                if n <= 0 {
                    self.par_shape.clear();
                } else {
                    let mut shape = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        let indent = self.scan_dimen(false, false);
                        let width = self.scan_dimen(false, false);
                        shape.push((indent, width));
                    }
                    self.par_shape = shape;
                }
                self.clear_prefixes();
                true
            }
            IntP(ip) => {
                if matches!(ip, crate::prim::IntParam::ErrorStopMode | crate::prim::IntParam::ScrollMode | crate::prim::IntParam::NonStopMode | crate::prim::IntParam::BatchMode) {
                    self.clear_prefixes();
                    return true;
                }
                self.scan_optional_equals();
                match ip {
                    IntParam::CurFam => {
                        let v = self.scan_int();
                        self.eqtb.int_params[ip.idx() as usize] = v;
                    }
                    _ => {
                        let v = self.scan_int();
                        self.eqtb.assign_int_param(ip, v, self.global_flag);
                    }
                }
                self.clear_prefixes();
                true
            }
            DimP(dp) => {
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                if dp == DimParam::PrevDepth {
                    self.prev_depth = v;
                } else {
                    self.eqtb.assign_dim_param(dp, v, self.global_flag);
                }
                self.clear_prefixes();
                true
            }
            GlueP(gp) => {
                self.scan_optional_equals();
                let v = self.scan_glue(gp.is_mu());
                self.eqtb.assign_glue_param(gp, v, self.global_flag);
                self.clear_prefixes();
                true
            }
            ToksP(tp) => {
                self.scan_optional_equals();
                let toks = Rc::new(self.scan_token_list());
                if tp == crate::prim::ToksParam::EveryEOF && std::env::var("UNDEFTRACE").is_ok() {
                    eprintln!("EVERYEOF-SET len={} line={} file={}", toks.len(), self.input.current_file_line(), self.input.current_file_name());
                }
                self.eqtb.assign_toks_param(tp, toks, self.global_flag);
                self.clear_prefixes();
                true
            }
            _ => false,
        }
    }

    /// cs-bound value/parameter assignment: \foo=... where foo is a register
    /// alias or a parameter primitive
    fn cs_assign(&mut self, id: CsId) -> bool {
        match self.eqtb.resolve(id).cloned() {
            Some(Equiv::Prim(Prim::IntP(ip))) => {
                self.scan_optional_equals();
                let v = self.scan_int();
                self.eqtb.assign_int_param(ip, v, self.global_flag);
                self.clear_prefixes();
                true
            }
            Some(Equiv::Prim(Prim::DimP(dp))) => {
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                if dp == DimParam::PrevDepth {
                    self.prev_depth = v;
                } else {
                    self.eqtb.assign_dim_param(dp, v, self.global_flag);
                }
                self.clear_prefixes();
                true
            }
            Some(Equiv::Prim(Prim::GlueP(gp))) => {
                self.scan_optional_equals();
                let v = self.scan_glue(gp.is_mu());
                self.eqtb.assign_glue_param(gp, v, self.global_flag);
                self.clear_prefixes();
                true
            }
            Some(Equiv::Prim(Prim::ToksP(tp))) => {
                self.scan_optional_equals();
                let toks = Rc::new(self.scan_token_list());
                self.eqtb.assign_toks_param(tp, toks, self.global_flag);
                self.clear_prefixes();
                true
            }
            Some(Equiv::CountReg(i)) => {
                self.scan_optional_equals();
                let v = self.scan_int();
                self.eqtb.assign_count(i, v, self.global_flag);
                self.clear_prefixes();
                true
            }
            Some(Equiv::DimenReg(i)) => {
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                self.eqtb.assign_dimen(i, v, self.global_flag);
                self.clear_prefixes();
                true
            }
            Some(Equiv::SkipReg(i)) => {
                self.scan_optional_equals();
                let v = self.scan_glue(false);
                self.eqtb.assign_skip(i, v, self.global_flag);
                self.clear_prefixes();
                true
            }
            Some(Equiv::MuSkipReg(i)) => {
                self.scan_optional_equals();
                let v = self.scan_glue(true);
                self.eqtb.assign_muskip(i, v, self.global_flag);
                self.clear_prefixes();
                true
            }
            Some(Equiv::ToksReg(i)) => {
                self.scan_optional_equals();
                let toks = Rc::new(self.scan_token_list());
                self.eqtb.assign_toks_reg(i, toks, self.global_flag);
                self.clear_prefixes();
                true
            }
            _ => false,
        }
    }

    pub fn scan_char_num(&mut self) -> i32 {
        self.scan_int()
    }

    pub fn define_value(&mut self, e: Equiv) {
        let t = self.scan_definable_cs();
        self.eqtb.assign(t, e, self.global_flag);
        self.clear_prefixes();
    }

    /// scan a cs for definitions (non-expanding, like tex.web's get_token)
    pub fn scan_definable_cs(&mut self) -> CsId {
        static NONCS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        self.skip_raw_spaces();
        let t = self.raw_token();
        if crate::debug_flag("DEFTRACE") {
            eprintln!("DEFINABLE target={:#x} cs={:?}", t.0, if t.is_cs() { String::from_utf8_lossy(self.cs.name(t.cs_id())).to_string() } else { String::new() });
        }
        if crate::debug_flag("DEFWATCH") {
            eprintln!("DEFSCAN target={:#x} cs={:?} pushed_top={:?} line={}", t.0, if t.is_cs() { String::from_utf8_lossy(self.cs.name(t.cs_id())).to_string() } else { "-".to_string() }, self.pushed.last().map(|x| format!("{:#x}", x.0)), self.input.current_file_line());
        }
        if t.is_char() && t.cc() == 13 {
            NONCS.store(0, std::sync::atomic::Ordering::Relaxed);
            let aid = self.active_cs_id(t.chr() as u8);
            if crate::debug_flag("DEFWATCH") {
                eprintln!("DEFSCAN-ACTIVE chr={} aid={} line={}", t.chr() as u8 as char, aid, self.input.current_file_line());
            }
            return aid;
        }
        if !t.is_cs() {
            let n = NONCS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n < 3 {
                let ln = self.input.current_file_line();
                let fnm = self.input.current_file_name();
                eprintln!(
                    "DEF-NONCS tok={:#x} cc={} chr={} L{} file={} pushed=[{}]",
                    t.0,
                    if t.0 < 0x8000_0000 { t.cc() } else { 99 },
                    t.chr(),
                    ln,
                    fnm.split('/').last().unwrap_or(""),
                    self.tokens_to_string(&self.pushed.iter().rev().take(4).cloned().collect::<Vec<_>>())
                );
            }
            self.error("Missing control sequence inserted");
            if n >= 8 {
                self.pushed.clear();
                while matches!(self.input.stack.last(), Some(crate::input::Source::TokList { .. })) {
                    self.input.stack.pop();
                }
                NONCS.store(0, std::sync::atomic::Ordering::Relaxed);
            }
            return self.cs.intern(b"inaccessible");
        }
        NONCS.store(0, std::sync::atomic::Ordering::Relaxed);
        t.cs_id()

    }

    fn do_def_register(&mut self, mk: impl Fn(&mut Self, u16) -> Equiv) {
        // tex.web: \countdef/\dimendef/\skipdef/\toksdef assignments are always \global
        let t = self.scan_definable_cs();

        if crate::debug_flag("DEFWATCH")
            && self.cs.name(t) == b"def" {
            let ring: Vec<std::string::String> = self.tok_ring.iter().rev().take(30).map(|(v, ln)| format!("{:#x}@{}", v, ln)).collect();
            eprintln!("DEFWATCH: register-def targeting \\def at line {} ring=[{}]", self.input.current_file_line(), ring.join(" "));
        }
        self.scan_optional_equals();
        let idx = self.scan_reg_num();
        let e = mk(self, idx);
        self.eqtb.assign(t, e, true);
        self.clear_prefixes();
    }

    /// \def/\gdef/\edef/\xdef
    fn do_def(&mut self, p: Prim, _id: CsId) {
        if self.input.current_file_line() >= 6738 && self.input.current_file_line() <= 6742 {
            eprintln!(
                "DODEF-ENTER p={:?} L{} file={} e-scan={} cat#={} cat?={} pushed=[{}]",
                p,
                self.input.current_file_line(),
                self.input.current_file_name().split('/').last().unwrap_or(""),
                self.in_expanded_scan,
                self.eqtb.cat[b'#' as usize],
                self.eqtb.cat[b'?' as usize],
                self.tokens_to_string(&self.pushed.iter().rev().take(8).cloned().collect::<Vec<_>>())
            );
        }

        let global = self.global_flag || p == Prim::GDef || p == Prim::XDef;
        let expanded = p == Prim::EDef || p == Prim::XDef;
        let target = self.scan_definable_cs();
        // parameter text
        let mut params: Vec<Vec<Token>> = Vec::new();
        let mut num_params = 0u8;
        let mut hash_brace: Option<Token> = None; // tex.web §473 #{ append
        self.def_prefix.clear();
        loop {
            // tex.web scans the parameter text with get_token (NON-expanding)
            let t = self.raw_token();
            if t == crate::input::EOF_MARKER {
                self.error("File ended in definition");
                self.end_occurred = true;
                return;
            }
            if t.is_char() && t.cc() == 1 {
                self.pushed.push(t);
                break;
            }
            // A parameter reference spliced from an outer macro body arrives
            // as a PAR_REF token (cc field 0), not a cat-6 char token; tex.web
            // treats these as ordinary #n parameters when a \def's param text
            // is itself built by expansion (expl3 conditional wrappers).
            if t.0 >= crate::expand::PAR_REF_FLAG && t.0 < 0x8000_0000 {
                let n = (t.0 & 0x3FFF_FFFF) as u32;
                num_params += 1;
                params.push(Vec::new());
                if n != num_params as u32 || num_params > 9 {
                    self.error("Illegal parameter number");
                }
                continue;
            }
            if t.is_char() && t.cc() == 6 {
                // #n or #{
                let mut t2 = self.raw_token();
                while t2.is_char() && (t2.cc() == 10 || t2.cc() == 0) {
                    t2 = self.raw_token();
                }
                if t2.is_char() && t2.cc() == 6 {
                    if let Some(last) = params.last_mut() {
                        last.push(Token::char(6, b'#' as u32));
                    } else {
                        self.def_prefix.push(Token::char(6, b'#' as u32));
                    }
                    continue;
                }
                if t2.is_char() && t2.cc() == 1 {
                    // tex.web §476 (hash_brace): `#{` — the brace becomes the
                    // last parameter's delimiter; param text ends here WITHOUT
                    // consuming the body-opening brace (collect_def_body starts
                    // already inside the group, unbalance=1) and a copy of the
                    // brace is appended to the END of the body (§473) so the
                    // group the grab consumed is reopened at call time.
                    if let Some(last) = params.last_mut() {
                        last.push(Token::char(1, b'{' as u32));
                    } else {
                        num_params += 1;
                        params.push(vec![Token::char(1, b'{' as u32)]);
                    }
                    hash_brace = Some(t2);
                    break;
                }
                let n = t2.chr();
                num_params += 1;
                params.push(Vec::new());
                if n == b'0' as u32 || num_params > 9 {
                    self.error("Illegal parameter number");
                    num_params = 9;
                }
                continue;
            }
            match params.last_mut() {
                Some(last) => last.push(t),
                None => self.def_prefix.push(t),
            }
        }
        let body = self.collect_def_body(target, expanded, hash_brace.is_some());
        if self.cs.name(target) == b"GTS@Token" {
            eprintln!("AFTER-TOKEN-EDEF e-scan={} body=[{}]", self.in_expanded_scan, self.tokens_to_string(&body));
        }
        if crate::debug_flag("QUARKTRACE")
            && (String::from_utf8_lossy(self.cs.name(target)).starts_with("q__")
                || String::from_utf8_lossy(self.cs.name(target)).contains("recursion_tail"))
        {
            eprintln!("QUIRKDEF \\{} body=[{}] self={}", String::from_utf8_lossy(self.cs.name(target)),
                self.tokens_to_string(&body),
                body.iter().any(|t| t.is_cs() && self.cs.name(t.cs_id()) == self.cs.name(target)));
        }
        if crate::debug_flag("DEFTOOL") {
            eprintln!("FINISH {} np={} prefix=[{}] params=[{}] body=[{}]",
                String::from_utf8_lossy(self.cs.name(target)), num_params,
                self.tokens_to_string(&self.def_prefix),
                params.iter().map(|p| format!("{{{}}}", self.tokens_to_string(p))).collect::<Vec<_>>().join(","),
                self.tokens_to_string(&body));
        }
        {
            let ln = self.input.current_file_line();
            if (6730..=6750).contains(&ln) {
                let swallowed = body.iter().any(|t| t.is_cs() && self.cs.name(t.cs_id()) == b"@onlypreamble");
                eprintln!(
                    "DEF-DONE p={:?} \\{} np={} body_len={} swallowed={} e-scan={} L{} file={} body=[{}]",
                    p,
                    String::from_utf8_lossy(self.cs.name(target)),
                    num_params,
                    body.len(),
                    swallowed,
                    self.in_expanded_scan,
                    ln,
                    self.input.current_file_name().split('/').last().unwrap_or(""),
                    self.tokens_to_string(&body.iter().take(20).cloned().collect::<Vec<_>>())
                );
            }
        }

        self.finish_def(target, num_params, params, body, global);
    }

    fn finish_def(&mut self, target: CsId, num_params: u8, params: Vec<Vec<Token>>, body: Vec<Token>, global: bool) {
        let m = Macro {
            num_params,
            params,
            body,
            prefix: std::mem::take(&mut self.def_prefix),
            long: self.long_flag,
            outer: self.outer_flag,
            protected: self.protected_flag,
        };
        self.long_flag = false;
        self.outer_flag = false;
        self.protected_flag = false;
        self.eqtb.assign(target, Equiv::Macro(Rc::new(m)), global);
        self.clear_prefixes();
    }

    /// e-TeX: `\unexpanded` tokens copied into `\edef`/`\xdef` keep literal
    /// `#n` (Knuth doubles hashes from `\the`/`\unexpanded`). Leaving them as
    /// PAR_REF binds `#1`/`#2` to the outer definition — that is what broke
    /// `\__hook_tmp:w` (nameref `\AddToHookWithArguments`).
    fn store_unexpanded_in_edef(out: &mut Vec<Token>, toks: impl IntoIterator<Item = Token>) {
        for t in toks {
            let t = Token::unfreeze(t);
            if t.0 >= PAR_REF_FLAG && t.0 < 0x8000_0000 && !t.is_cs() {
                let n = (t.0 & 0xF) as u8;
                out.push(Token::char(6, b'#' as u32));
                out.push(Token::char(12, b'0' as u32 + n as u32));
            } else {
                out.push(t);
            }
        }
    }

    /// collect macro body until the closing brace at depth 0
    fn collect_def_body(&mut self, target: CsId, expanded: bool, brace_consumed: bool) -> Vec<Token> {
        let prev_expanded_scan = self.in_expanded_scan;
        if expanded {
            let fnm = self.input.current_file_name();
            if self.last_macros.iter().any(|m| m == "GTS@RemoveLeft" || m == "GTS@TestLeftEnd" || m == "GetTitleStringNonExpand") {
                static ED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                if ED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 16 {
                    eprintln!(
                        "EDEF \\{} L{} file={} macros={:?}",
                        std::string::String::from_utf8_lossy(self.cs.name(target)),
                        self.input.current_file_line(),
                        fnm.split('/').last().unwrap_or(""),
                        self.last_macros.iter().rev().take(8).collect::<Vec<_>>()
                    );
                }
            }
            if fnm.contains("latex.ltx")
                && (6680..=6760).contains(&self.input.current_file_line())
            {
                eprintln!(
                    "E-SCAN-ON edef \\{} L{} brace_consumed={} prev_e={}",
                    std::string::String::from_utf8_lossy(self.cs.name(target)),
                    self.input.current_file_line(),
                    brace_consumed,
                    prev_expanded_scan
                );
            }
            self.in_expanded_scan = true;
        }
        let mut out = Vec::new();
        let dump_tlifin = self.cs.name(target).starts_with(b"tl_if_in:nn");
        let mut dump_n = 0u32;
        if dump_tlifin {
            eprintln!("CDB-START \\{} expanded={} brace_consumed={}", String::from_utf8_lossy(self.cs.name(target)), expanded, brace_consumed);
            for key in [b"exp_not:n" as &[u8], b"unexpanded", b"tex_unexpanded:D", b"exp_not:N", b"use:e", b"tex_expanded:D", b"expanded"] {
                let desc = match self.cs.lookup(key).and_then(|id| self.eqtb.resolve(id).cloned()) {
                    Some(Equiv::Prim(p)) => format!("Prim({p:?})"),
                    Some(Equiv::Macro(m)) => format!("Macro(np={})", m.num_params),
                    Some(other) => other.kind_name().to_string(),
                    None => "undefined".into(),
                };
                eprintln!("CDB-MEAN \\{} = {}", String::from_utf8_lossy(key), desc);
            }
        }
        let mut depth = 1i32;
        if !brace_consumed {
            self.skip_spaces_relax();
            let t = self.get_token();
            if !(t.is_char() && t.cc() == 1) {
                let got = if t.is_cs() {
                    format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id())))
                } else {
                    format!("cc{}:{}", t.cc(), t.chr())
                };
                self.error(&format!("Missing {{ inserted (def body, got {})", got));
                self.in_expanded_scan = prev_expanded_scan;
                return out;
            }
        }
        loop {
            if expanded {
                let raw = self.raw_token();
                if raw.is_cs() {
                    let mut id = raw.cs_id();
                    for _ in 0..1024 {
                        match self.eqtb.get(id) {
                            Some(Equiv::Alias(next)) => id = *next,
                            _ => break,
                        }
                    }
                    if let Some(Equiv::Prim(Prim::UnExpanded)) = self.eqtb.get(id) {
                        self.skip_spaces_relax();
                        let nxt = self.raw_token();
                        if nxt.is_cs() {
                            if let Some(Equiv::ToksReg(i)) = self.eqtb.resolve(nxt.cs_id()).cloned() {
                                let toks = (*self.eqtb.toks[i as usize]).clone();
                                Self::store_unexpanded_in_edef(&mut out, toks);

                                continue;
                            }
                        }
                        self.pushed.push(nxt);
                        let toks = self.scan_general_text();
                        Self::store_unexpanded_in_edef(&mut out, toks);

                        continue;
                    }
                }
                self.pushed.push(raw);
            }
            let t = if expanded { self.get_token() } else { self.raw_token() };
            let from_unexp = expanded && self.unexp_protect > 0;
            if from_unexp {
                self.unexp_protect -= 1;
            }
            if dump_tlifin && dump_n < 25 {
                dump_n += 1;
                let desc = if t.is_cs() {
                    format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id())))
                } else if t.0 >= PAR_REF_FLAG && t.0 < 0x8000_0000 {
                    format!("PARREF{}", t.0 & 0xF)
                } else {
                    format!("cc{}:{:#x}", t.cc(), t.chr())
                };
                eprintln!("CDB t={:#x} {} d={}", t.0, desc, depth);
            }
            if t == crate::input::EOF_MARKER {
                self.error(&format!("File ended while scanning macro body for \\{} depth={}", String::from_utf8_lossy(self.cs.name(target)), depth));
                self.end_occurred = true;
                self.in_expanded_scan = prev_expanded_scan;
                return out;
            }
            if expanded && self.cur_prim == Some(Prim::UnExpanded) {
                self.skip_spaces_relax();
                let nxt = self.raw_token();
                if nxt.is_cs() {
                    if let Some(Equiv::ToksReg(i)) = self.eqtb.resolve(nxt.cs_id()).cloned() {
                        Self::store_unexpanded_in_edef(&mut out, (*self.eqtb.toks[i as usize]).clone());

                        continue;
                    }
                }
                self.pushed.push(nxt);
                Self::store_unexpanded_in_edef(&mut out, self.scan_general_text());

                continue;
            }
            if t.is_char() {
                let cc = t.cc();
                if cc == 1 {
                    depth += 1;
                } else if cc == 2 {
                    depth -= 1;
                    if depth == 0 {
                        self.in_expanded_scan = prev_expanded_scan;
                        return out;
                    }
                }
                if cc == 6 {
                    // `\unexpanded{#1}` inside `\edef\foo#1#2{...}` must store
                    // a literal hash, not parameter 1 of `\foo`.
                    if from_unexp {
                        out.push(Token::char(6, b'#' as u32));
                        continue;
                    }
                    let t2 = self.raw_token();
                    if dump_tlifin {
                        eprintln!("CDB-HASH t2={:#x} cc={} chr={:#x}", t2.0, if t2.is_char() { t2.cc() } else { 99 }, t2.chr());
                    }
                    if t2.is_char() && t2.cc() == 6 {
                        out.push(Token::char(6, b'#' as u32));
                        continue;
                    }
                    if t2.is_char() && (b'1'..=b'9').contains(&(t2.chr() as u8)) {
                        out.push(Token(PAR_REF_FLAG | (t2.chr() & 0xF)));
                        continue;
                    }
                    if t2.is_char() && t2.chr() == b'0' as u32 {
                        self.error("Illegal parameter number in definition");
                        out.push(Token(PAR_REF_FLAG | 1));
                        continue;
                    }
                    out.push(t);
                    self.pushed.push(t2);
                    continue;
                }
            }
            out.push(t);


        }
    }

    /// \let (and \futurelet)
    fn do_let(&mut self, future: bool) {
        let target = self.scan_definable_cs();
        let nm = std::string::String::from_utf8_lossy(self.cs.name(target)).to_string();
        if nm.contains("bar_bool") || nm.contains("backend_header_bool") || nm.contains("cmd_log_bool") {
            eprintln!("DO_LET_BOOL target={} future={} global={} level={} line={}", nm, future, self.global_flag, self.eqtb.cur_level, self.input.current_file_line());
        }
        if future {
            let tb = self.raw_token();
            let tc = self.raw_token();
            if tc.is_cs() {
                self.copy_meaning(target, tc.cs_id());
            } else if tc.is_char() && tc.cc() == 13 {
                let id = self.active_cs_id(tc.chr() as u8);
                self.copy_meaning(target, id);
            } else {
                self.eqtb.assign(target, Equiv::CharTok(tc.0), self.global_flag);
            }
            if tc.is_cs() && self.cs.name(tc.cs_id()) == b"end" {
                self.input.push_toks(vec![tc], "futurelet-end");
                self.pushed.push(tb);
            } else {
                self.pushed.push(tc);
                self.pushed.push(tb);
            }





        } else {
            self.skip_raw_spaces();
            let eq = self.raw_token();
            if eq.is_char() && eq.chr() == b'=' as u32 && eq.cc() == 12 {
                let sp = self.raw_token();
                if !(sp.is_char() && sp.cc() == 10) {
                    self.pushed.push(sp);
                }
            } else {
                self.pushed.push(eq);
            }
            let t = self.raw_token();
            if nm.contains("bar_bool") || nm.contains("backend_header_bool") || nm.contains("cmd_log_bool") {
                let tn = if t.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) } else { format!("cc{}:{}", t.cc(), t.chr()) };
                eprintln!("DO_LET_SRC target={} src={} global={}", nm, tn, self.global_flag);
            }
            if t.is_cs() {
                self.copy_meaning(target, t.cs_id());
            } else if t.is_char() && t.cc() == 13 {
                let id = self.active_cs_id(t.chr() as u8);
                self.copy_meaning(target, id);
            } else if t.0 >= crate::expand::PAR_REF_FLAG && t.0 < 0xFFFF_0000 && t.0 != crate::input::PAR_END.0 {
                self.error("Missing control sequence after \\let");
            } else {
                self.eqtb.assign(target, Equiv::CharTok(t.0), self.global_flag);
            }
        }
        self.clear_prefixes();
    }


    /// copy meaning from src cs to dst cs (TeX \let semantics)
    fn copy_meaning(&mut self, dst: CsId, src: CsId) {
        match self.eqtb.resolve(src).cloned() {
            None => {
                let name = self.cs.name(src);
                if name == b"/" || name == b"@@italiccorr" {
                    self.eqtb.assign(dst, Equiv::Prim(Prim::Relax), self.global_flag);
                } else {
                    self.eqtb.undefine(dst, self.global_flag);
                }
            }
            Some(eq) => self.eqtb.assign(dst, eq, self.global_flag),
        }
    }

    fn after_prefix(&mut self) {
        self.skip_spaces_relax();
        let t = self.get_token();
        if t.is_cs() {
            let id = t.cs_id();
            match self.eqtb.resolve(id).cloned() {
                Some(Equiv::Prim(p)) => {
                    if self.try_assignment(p, id) {
                        if !matches!(p, Prim::Global | Prim::Long | Prim::Outer | Prim::Protected | Prim::AfterAssignment) {
                            self.trigger_after_assignment();
                        }
                    } else {
                        self.main_dispatch(p, id);
                    }
                    return;
                }
                Some(Equiv::Macro(m)) => {
                    self.expand_macro(id, &m);
                    return;
                }
                _ => {}
            }
        }
        self.error("Missing \\def or similar after prefix");
    }

    /// scan a token list (\toks assignments): balanced text or \csname...
    /// tex.web scan_toks (xpand=false): expand until `{` / register, then
    /// copy the group RAW. Expanding inside the group made `\toks@\expandafter
    /// \expandafter\expandafter{\expandafter\GTS@Car\GTS@GlobalString...\GTS@Nil}`
    /// run `\GTS@Car` at top level and hang looking for `\GTS@Nil`.
    pub fn scan_token_list(&mut self) -> Vec<Token> {
        self.skip_spaces_relax();
        let t = self.get_token();
        if t.is_cs() {
            match self.eqtb.resolve(t.cs_id()).cloned() {
                Some(Equiv::ToksReg(i)) => return (*self.eqtb.toks[i as usize]).clone(),
                Some(Equiv::Prim(Prim::ToksP(p))) => return (*self.eqtb.tok_params[p.idx() as usize]).clone(),
                Some(Equiv::Prim(Prim::CsName)) => {
                    let id = self.scan_csname_explicit();
                    return vec![Token::from_cs(id)];
                }
                _ => {}
            }
        }
        if t.is_char() && t.cc() == 1 {
            return self.scan_balanced_raw(true);
        }
        self.error("Missing { inserted (token list)");
        self.pushed.push(t);
        Vec::new()
    }

    pub fn scan_csname_explicit(&mut self) -> CsId {
        let mut name: Vec<u8> = Vec::new();
        loop {
            let t = self.get_token();
            if t == crate::input::EOF_MARKER {
                break;
            }
            if t.is_cs() {
                break;
            }
            name.push(t.chr() as u8);
        }
        let id = self.cs.intern(&name);
        self.last_named_cs = Some(id);
        if self.eqtb.get(id).is_none() {
            let relax = self.cs.lookup(b"relax").unwrap();
            if let Some(r) = self.eqtb.get(relax).cloned() {
                self.eqtb.assign(id, r, true);
            }
        }
        id
    }

    // ---------- grouping ----------

    /// `{` / `\bgroup`: tex.web simple_group — save-stack only.
    /// Box constructors (`\hbox`/`\vbox`/...) consume their own `{` via begin_box.
    pub fn begin_group(&mut self, _brace: bool) {
        self.eqtb.push_level(LevelType::Simple);
    }

    pub fn end_group(&mut self) {
        match self.eqtb.cur_group_type() {
            Some(LevelType::Box) => self.end_box(),
            Some(LevelType::Simple) => {
                if self.eqtb.save_stack.is_empty() {
                    self.error("Too many }'s");
                    return;
                }
                let _ = self.pop_group();
                // tex.web 1136-1140: the `}` closing a \noalign body group
                // ends the no-align. Depth returned to the watermark set by
                // align_noalign means the body's brace group just closed.
                if self.scanner_status == ScannerStatus::Aligning
                    && self.align_in_noalign
                    && self.eqtb.save_stack.len() == self.align_noalign_save_base
                {
                    self.align_finish_noalign_now();
                }
            }
            Some(LevelType::Group) => {
                // math/legacy groups: pack if a box context is open
                if !self.box_kinds.is_empty() {
                    self.end_box();
                } else if self.eqtb.save_stack.is_empty() {
                    self.error("Too many }'s");
                } else {
                    let _ = self.pop_group();
                }
            }
            Some(LevelType::SemiSimple) => {
                self.error("Extra }, or forgotten \\endgroup");
            }
            _ => self.error("Too many }'s"),
        }
    }

    /// tex.web semi_simple_group: \\begingroup/\\endgroup save-stack only
    pub fn begin_semi_simple(&mut self) {
        self.ss_trace.push(format!(
            "{}:{}",
            self.input.current_file_name().split('/').last().unwrap_or("?"),
            self.input.current_file_line()
        ));
        self.eqtb.push_level(crate::eqtb::LevelType::SemiSimple);
    }
    pub fn end_semi_simple(&mut self) {
        match self.eqtb.cur_group_type() {
            Some(crate::eqtb::LevelType::SemiSimple) => {
                self.ss_trace.pop();
                let _ = self.pop_group();
            }
            Some(crate::eqtb::LevelType::Simple) => {
                self.error("Extra \\endgroup, or missing }");
            }
            _ => {
                if self.eqtb.save_stack.is_empty() {
                    if crate::debug_flag("EGTRACE") {
                        let srcs: Vec<String> = self.input.stack.iter().rev().take(5).map(|src| match src {
                            crate::input::Source::TokList { name, pos, toks, .. } => format!("T:{} {}/{}", name, pos, toks.len()),
                            crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name, line_no),
                        }).collect();
                        eprintln!("EG-TOOMANY srcs=[{}] macs={:?}", srcs.join(" << "), self.last_macros.iter().rev().take(8).collect::<Vec<_>>());
                    }
                    self.error("Too many \\endgroups");
                } else {
                    // mismatch: still pop to keep the save stack moving
                    let _ = self.pop_group();
                }
            }
        }
    }



}
