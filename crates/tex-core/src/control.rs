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

    fn main_loop(&mut self) {
        while !self.end_occurred {
            let t = self.get_token();
            if std::env::var("PCTRACE").map(|v|v=="1").unwrap_or(false) && t.is_cs() && self.cs.name(t.cs_id()) == b"gdef" {
                eprintln!("MAINLOOP-GOT-GDEF line={}", self.input.current_file_line());
            }
            if t == crate::input::EOF_MARKER {
                break;
            }
            self.dispatch(t);
        }
    }

    pub fn dispatch(&mut self, t: Token) {
        if std::env::var("PCTRACE").map(|v|v=="1").unwrap_or(false) && t.is_cs() && self.cs.name(t.cs_id()) == b"gdef" {
            eprintln!("DISPATCH-GDEF line={} pushed={:?}", self.input.current_file_line(), self.pushed.iter().rev().take(3).map(|x| if x.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(x.cs_id()))) } else { format!("{}{}", x.cc(), x.chr() as u8 as char) }).collect::<Vec<_>>());
        }
        if t.is_cs() {
            let id = t.cs_id();
            // font selector
            if let Some(Equiv::FontRef(f)) = self.eqtb.resolve(id).cloned() {
                self.cur_font = f;
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
                    return;
                }
                _ => {}
            }
            // assignment prefixes
            if let Some(Equiv::Prim(p)) = self.eqtb.resolve(id).cloned() {
                match self.try_assignment(p, id) {
                    true => return,
                    false => {}
                }
                self.main_dispatch(p, id);
            } else {
                // non-primitive cs used as value: usually error
                match self.eqtb.resolve(id).cloned() {
                    None => {
                        let name = String::from_utf8_lossy(self.cs.name(id)).into_owned();
                        if name == "::x" || name == "q__cs_nil" {
                            eprintln!("UNDEF \\{} L{} macros={:?}", name, self.input.current_file_line(), self.last_macros);
                        }
                        self.error(&format!("Undefined control sequence \\{}", name));
                    }
                    Some(Equiv::Macro(m)) => {
                        // e-TeX: protected macros are not expanded by get_x_token
                        // but the execute processor still calls them.
                        self.expand_macro(id, &m);
                    }
                    Some(Equiv::MathCharDef(v)) if self.mode.is_m() => {
                        self.append_mathchar(v as u16);
                    }
                    other => {
                        if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) {
                            let name = String::from_utf8_lossy(self.cs.name(id)).into_owned();
                            eprintln!("SILENT-DISPATCH cs=\\{} equiv={:?}", name, other.map(|e| e.kind_name()));
                        }
                    }
                }
            }
        } else {
            let cc = t.cc();
            let c = t.chr() as u8;
            match cc {
                1 => self.begin_group(true),
                2 => self.end_group(),
                3 => {
                    // math shift
                    if self.mode.is_m() {
                        self.exit_math();
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

    /// Handle assignment-prefix primitives; returns true if consumed.
    pub fn try_assignment(&mut self, p: Prim, id: CsId) -> bool {
        use Prim::*;
        match p {
            Global => {
                self.global_flag = true;
                let t = self.get_token();
                if t.is_cs() {
                    let id2 = t.cs_id();
                    if self.cs_assign(id2) {
                        true
                    } else {
                        match self.eqtb.resolve(id2).cloned() {
                            Some(Equiv::Prim(p2)) => {
                                if !self.try_assignment(p2, id2) {
                                    self.main_dispatch(p2, id2);
                                }
                                self.global_flag = false;
                                true
                            }
                            Some(Equiv::Macro(m)) => {
                                self.expand_macro(id2, &m);
                                true
                            }
                            _ => {
                                self.global_flag = false;
                                self.pushed.push(t);
                                true
                            }
                        }
                    }
                } else {
                    self.global_flag = false;
                    self.pushed.push(t);
                    true
                }
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
                self.after_prefix();
                true
            }
            Outer => {
                self.outer_flag = true;
                self.after_prefix();
                true
            }
            Protected => {
                self.protected_flag = true;
                self.after_prefix();
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
                self.global_flag = false;
                true
            }
            Dimen => {
                let idx = self.scan_reg_num();
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                self.eqtb.assign_dimen(idx, v, self.global_flag);
                self.global_flag = false;
                true
            }
            Skip => {
                let idx = self.scan_reg_num();
                self.scan_optional_equals();
                let v = self.scan_glue(false);
                self.eqtb.assign_skip(idx, v, self.global_flag);
                self.global_flag = false;
                true
            }
            MuSkip => {
                let idx = self.scan_reg_num();
                self.scan_optional_equals();
                let v = self.scan_glue(true);
                self.eqtb.assign_muskip(idx, v, self.global_flag);
                self.global_flag = false;
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
                self.global_flag = false;
                true
            }
            Box => {
                // \box<n> in value position handled in scan paths; in main
                // position it's an error unless followed by use
                let idx = self.scan_reg_num();
                let b = self.eqtb.boxed[idx as usize].take();
                self.append_box_node(b);
                true
            }
            CountDef => {
                if std::env::var("DEFTRACE").map(|v|v=="1").unwrap_or(false) { eprintln!("COUNTDEF call"); }
                self.do_def_register(|e, idx| Equiv::CountReg(idx)); true
            }
            DimenDef => {
                if std::env::var("DEFTRACE").map(|v|v=="1").unwrap_or(false) { eprintln!("DIMENDEF call"); }
                self.do_def_register(|e, idx| Equiv::DimenReg(idx)); true
            }
            SkipDef => { self.do_def_register(|e, idx| Equiv::SkipReg(idx)); true }
            MuSkipDef => { self.do_def_register(|e, idx| Equiv::MuSkipReg(idx)); true }
            ToksDef => { self.do_def_register(|e, idx| Equiv::ToksReg(idx)); true }
            CharDef => {
                // tex.web: \chardef is global
                let t = self.scan_definable_cs();
                if std::env::var("DEFWATCH").map(|v|v=="1").unwrap_or(false)
                    && self.cs.name(t) == b"def" {
                    eprintln!("DEFWATCH: \\chardef targeting \\def at line {}", self.input.current_file_line());
                }
                self.scan_optional_equals();
                let v = self.scan_int();
                self.eqtb.assign(t, Equiv::CharDef(v.max(0) as u32), true);
                self.global_flag = false;
                true
            }
            MathCharDef => {
                // tex.web: \mathchardef is global
                let t = self.scan_definable_cs();
                self.scan_optional_equals();
                let v = self.scan_int();
                self.eqtb.assign(t, Equiv::MathCharDef(v as u16), true);
                self.global_flag = false;
                true
            }
            FontDimen => {
                let idx = self.scan_int();
                let f = self.scan_font_id();
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                if idx > 0 {
                    self.eqtb.assign_font_param(f, idx as usize - 1, v, self.global_flag);
                }
                self.global_flag = false;
                true
            }
            HyphenChar => {
                let f = self.scan_font_id();
                self.scan_optional_equals();
                let v = self.scan_int();
                self.eqtb.assign_hyphen_char(f, v, self.global_flag);
                self.global_flag = false;
                true
            }
            SkewChar => {
                let f = self.scan_font_id();
                self.scan_optional_equals();
                let v = self.scan_int();
                self.eqtb.assign_skew_char(f, v, self.global_flag);
                self.global_flag = false;
                true
            }
            IntP(ip) => {
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
                self.global_flag = false;
                true
            }
            DimP(dp) => {
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                self.eqtb.assign_dim_param(dp, v, self.global_flag);
                self.global_flag = false;
                true
            }
            GlueP(gp) => {
                self.scan_optional_equals();
                let v = self.scan_glue(false);
                self.eqtb.assign_glue_param(gp, v, self.global_flag);
                self.global_flag = false;
                true
            }
            ToksP(tp) => {
                self.scan_optional_equals();
                let toks = Rc::new(self.scan_token_list());
                self.eqtb.assign_toks_param(tp, toks, self.global_flag);
                self.global_flag = false;
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
                self.global_flag = false;
                true
            }
            Some(Equiv::Prim(Prim::DimP(dp))) => {
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                self.eqtb.assign_dim_param(dp, v, self.global_flag);
                self.global_flag = false;
                true
            }
            Some(Equiv::Prim(Prim::GlueP(gp))) => {
                self.scan_optional_equals();
                let v = self.scan_glue(false);
                self.eqtb.assign_glue_param(gp, v, self.global_flag);
                self.global_flag = false;
                true
            }
            Some(Equiv::Prim(Prim::ToksP(tp))) => {
                self.scan_optional_equals();
                let toks = Rc::new(self.scan_token_list());
                self.eqtb.assign_toks_param(tp, toks, self.global_flag);
                self.global_flag = false;
                true
            }
            Some(Equiv::CountReg(i)) => {
                self.scan_optional_equals();
                let v = self.scan_int();
                self.eqtb.assign_count(i, v, self.global_flag);
                self.global_flag = false;
                true
            }
            Some(Equiv::DimenReg(i)) => {
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                self.eqtb.assign_dimen(i, v, self.global_flag);
                self.global_flag = false;
                true
            }
            Some(Equiv::SkipReg(i)) => {
                self.scan_optional_equals();
                let v = self.scan_glue(false);
                self.eqtb.assign_skip(i, v, self.global_flag);
                self.global_flag = false;
                true
            }
            Some(Equiv::MuSkipReg(i)) => {
                self.scan_optional_equals();
                let v = self.scan_glue(true);
                self.eqtb.assign_muskip(i, v, self.global_flag);
                self.global_flag = false;
                true
            }
            Some(Equiv::ToksReg(i)) => {
                self.scan_optional_equals();
                let toks = Rc::new(self.scan_token_list());
                self.eqtb.assign_toks_reg(i, toks, self.global_flag);
                self.global_flag = false;
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
        self.global_flag = false;
    }

    /// scan a cs for definitions (non-expanding, like tex.web's get_token)
    pub fn scan_definable_cs(&mut self) -> CsId {
        self.skip_raw_spaces();
        let t = self.raw_token();
        if std::env::var("DEFTRACE").map(|v|v=="1").unwrap_or(false) {
            eprintln!("DEFINABLE target={:#x} cs={:?}", t.0, if t.is_cs() { String::from_utf8_lossy(self.cs.name(t.cs_id())).to_string() } else { String::new() });
        }
        if std::env::var("DEFWATCH").map(|v|v=="1").unwrap_or(false) {
            eprintln!("DEFSCAN target={:#x} cs={:?} pushed_top={:?} line={}", t.0, if t.is_cs() { String::from_utf8_lossy(self.cs.name(t.cs_id())).to_string() } else { "-".to_string() }, self.pushed.last().map(|x| format!("{:#x}", x.0)), self.input.current_file_line());
        }
        if !t.is_cs() {
            self.error("Missing control sequence inserted");
            return self.cs.intern(b"");
        }
        t.cs_id()
    }

    fn do_def_register(&mut self, mk: impl Fn(&mut Self, u16) -> Equiv) {
        // tex.web: \countdef/\dimendef/\skipdef/\toksdef assignments are always \global
        let t = self.scan_definable_cs();
        if std::env::var("DEFWATCH").map(|v|v=="1").unwrap_or(false)
            && self.cs.name(t) == b"def" {
            let ring: Vec<std::string::String> = self.tok_ring.iter().rev().take(30).map(|(v, ln)| format!("{:#x}@{}", v, ln)).collect();
            eprintln!("DEFWATCH: register-def targeting \\def at line {} ring=[{}]", self.input.current_file_line(), ring.join(" "));
        }
        self.scan_optional_equals();
        let idx = self.scan_reg_num();
        let e = mk(self, idx);
        self.eqtb.assign(t, e, true);
        self.global_flag = false;
    }

    /// \def/\gdef/\edef/\xdef
    fn do_def(&mut self, p: Prim, _id: CsId) {
        let global = self.global_flag || p == Prim::GDef || p == Prim::XDef;
        let expanded = p == Prim::EDef || p == Prim::XDef;
        let target = self.scan_definable_cs();
        if std::env::var("DBT").map(|v|v=="1").unwrap_or(false) && self.cs.name(target) == b"ifx" {
            let st: Vec<String> = self.input.stack.iter().rev().take(4).map(|src| match src {
                crate::input::Source::TokList { name, pos, toks, .. } => {
                    let rest: Vec<String> = toks[(*pos).min(toks.len())..].iter().take(8).map(|t| {
                        if t.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) }
                        else { format!("{}{}", t.cc(), t.chr() as u8 as char) }
                    }).collect();
                    format!("T:{} {}/{} [{}]", name, pos, toks.len(), rest.join(" "))
                }
                crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name, line_no),
            }).collect();
            eprintln!("DBT do_def(ifx) stack=[{}]", st.join(" << "));
            // no exit
        }
        if std::env::var("XTRACE").map(|v|v=="1").unwrap_or(false) && expanded {
            eprintln!("XDEF-ENTER target={} line={} buf={:?} pushed={:?}", String::from_utf8_lossy(self.cs.name(target)), self.input.current_file_line(), self.current_line_text(), self.pushed.iter().rev().take(5).map(|x| if x.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(x.cs_id()))) } else { format!("{}{}", x.cc(), x.chr() as u8 as char) }).collect::<Vec<_>>());
        }
        if std::env::var("PCTRACE").map(|v|v=="1").unwrap_or(false) && self.cs.name(target) == b"@preamblecmds" {
            eprintln!("PCDEF-ENTER line={} pushed={:?}", self.input.current_file_line(), self.pushed.iter().rev().take(4).map(|x| if x.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(x.cs_id()))) } else { format!("{}{}", x.cc(), x.chr() as u8 as char) }).collect::<Vec<_>>());
        }
        // parameter text
        let mut params: Vec<Vec<Token>> = Vec::new();
        let mut num_params = 0u8;
        self.def_prefix.clear();
        loop {
            // tex.web scans the parameter text with get_token (NON-expanding):
            // defined macros must NOT expand inside a \def param/delimiter text
            let t = self.raw_token();
            if t == crate::input::EOF_MARKER {
                self.error("File ended in definition");
                self.end_occurred = true;
                if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) { eprintln!("ENDOCC crates/tex-core/src/control.rs:418 line={}", self.input.current_file_line()); }
                return;
            }
            if t.is_char() && t.cc() == 1 {
                // body starts; push back the brace
                { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/control.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                break;
            }
            if t.is_cs() && self.cur_prim == Some(Prim::Def) {
                // \def#1#{...} style: delimiter is left brace
            }
            if t.is_char() && t.cc() == 6 {
                // #n or #{
                let t2 = self.raw_token();
                if t2.is_char() && t2.cc() == 6 {
                    // ## -> literal #
                    if let Some(last) = params.last_mut() {
                        last.push(Token::char(6, b'#' as u32));
                    }
                    continue;
                }
                if t2.is_char() && t2.cc() == 1 {
                    // #{ : the left brace is the delimiter of the last param
                    num_params = params.len() as u8;
                    if num_params > 0 {
                        params[num_params as usize - 1] = vec![Token::char(1, b'{' as u32)];
                    }
                    // body starts at the brace (already consumed)
                    let body = self.collect_def_body(expanded, true);
                    self.finish_def(target, num_params, params, body, global);
                    return;
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
            // delimiter text for previous param (or prefix before any #)
            match params.last_mut() {
                Some(last) => last.push(t),
                None => self.def_prefix.push(t),
            }
        }
        let body = self.collect_def_body(expanded, false);
        self.finish_def(target, num_params, params, body, global);
    }

    fn finish_def(&mut self, target: CsId, num_params: u8, params: Vec<Vec<Token>>, body: Vec<Token>, global: bool) {
        if std::env::var("PCTRACE").map(|v|v=="1").unwrap_or(false) && self.cs.name(target) == b"@preamblecmds" {
            eprintln!("PCDEF line={} body={}", self.input.current_file_line(), self.tokens_to_string(&body));
        }
        if std::env::var("DEFDUMP").map(|v|v=="1").unwrap_or(false) {
            let nm = String::from_utf8_lossy(self.cs.name(target)).into_owned();
            if nm.contains("clist_if_wrap") || nm.contains("tl_trim_spaces_aux") || nm.contains("clist_wrap_item") || nm.contains("clist_sanitize") || nm.contains("clist_trim_next") {
                eprintln!("DEFDUMP {} params={} delim={} body_hex={}", nm, num_params,
                    params.iter().map(|d| d.iter().map(|t| format!("{:#x}", t.0)).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join(" | "),
                    body.iter().map(|t| format!("{:#x}", t.0)).collect::<Vec<_>>().join(","));
            }
        }
        if std::env::var("DEFTRACE").map(|v|v=="1").unwrap_or(false) {
            let nm = String::from_utf8_lossy(self.cs.name(target)).into_owned();
            if std::env::var("DEFALL").map(|v|v=="1").unwrap_or(false) || nm == "e@alloc" || nm == "e@ch@ck" {
                eprintln!("DEF {} nparams={} params={:?} body={}", nm, num_params,
                    params.iter().map(|d| self.tokens_to_string(d)).collect::<Vec<_>>(),
                    self.tokens_to_string(&body));
            }
        }
        let m = Macro { num_params, params, body, prefix: std::mem::take(&mut self.def_prefix), long: self.long_flag, outer: self.outer_flag, protected: self.protected_flag };
        self.long_flag = false;
        self.outer_flag = false;
        self.protected_flag = false;
        self.eqtb.assign(target, Equiv::Macro(Rc::new(m)), global);
        self.global_flag = false;
    }

    /// collect macro body until the closing brace at depth 0
    fn collect_def_body(&mut self, expanded: bool, brace_consumed: bool) -> Vec<Token> {
        let mut out = Vec::new();
        let mut depth = 1i32;
        if !brace_consumed {
            // expect {
            self.skip_spaces_relax();
            let t = self.get_token();
            if !(t.is_char() && t.cc() == 1) {
                let got = if t.is_cs() {
                    format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id())))
                } else {
                    format!("cc{}:{}", t.cc(), t.chr())
                };
                self.error(&format!("Missing {{ inserted (def body, got {})", got));
                return out;
            }
        }
        let mut pending_param: Option<u32> = None;
        let dbg_edef = self.input.current_file_line() == 2802
            && self.input.current_file_name().contains("expl3-code");
        loop {
            let t = if expanded { self.get_token() } else { self.raw_token() };
            if std::env::var("CDBTRACE").map(|v|v=="1").unwrap_or(false) && self.input.current_file_name().contains("expl3-code") && self.input.current_file_line() >= 8394 && self.input.current_file_line() <= 8412 {
                eprintln!("CDB line={} t={:#x}", self.input.current_file_line(), t.0);
            }
            if dbg_edef && expanded {
                eprintln!("COLLECT got {:#018x} cs={}", t.0, if t.is_cs() { String::from_utf8_lossy(self.cs.name(t.cs_id())).into_owned() } else { format!("cc{} chr={}", t.cc(), t.chr()) });
            }
            if t == crate::input::EOF_MARKER {
                self.error("File ended while scanning macro body");
                self.end_occurred = true;
                if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) { eprintln!("ENDOCC crates/tex-core/src/control.rs:500 line={}", self.input.current_file_line()); }
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
                if cc == 6 {
                    // param reference or ##
                    let t2 = self.raw_token();
                    if t2.is_char() && t2.cc() == 6 {
                        // ## -> single literal # token (cat 6, chr '#')
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
                    { let __pt = t2; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/control.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                    continue;
                }
            }
            if let Some(_p) = pending_param {
                pending_param = None;
            }
            if dbg_edef {
                eprintln!("EDEF tok {:#x} cs={}", t.0, if t.is_cs() { String::from_utf8_lossy(self.cs.name(t.cs_id())).into_owned() } else { String::new() });
            }
            out.push(t);
        }
    }

    /// \let (and \futurelet)
    fn do_let(&mut self, future: bool) {
        let target = self.scan_definable_cs();
        if future {
            // \futurelet A B C: A := meaning(C); then B and C execute.
            // target (A) already scanned; grab exactly B and C.
            let tb = self.raw_token();
            let tc = self.raw_token();
            if tc.is_cs() {
                self.copy_meaning(target, tc.cs_id());
            } else {
                // char tokens have eqtb slots; \futurelet may peek chars
                if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) {
                    eprintln!("FUTLET target={:?} peek={:#x}", String::from_utf8_lossy(self.cs.name(target)), tc.0);
                }
                self.eqtb.assign(target, Equiv::CharTok(tc.0), self.global_flag);
            }
            // tex.web back_inputs tc THEN tb on a single input stack, so tb's
            // expansion lands ABOVE tc and plays first. Our two-level model
            // (pushed > input.stack) breaks that if both go to `pushed`:
            // expanding tb pushes its body to input.stack, then tc would pop
            // ahead of the body. Park tc on the input stack (below whatever
            // tb expands to) and keep tb on the high-priority pushed stack.
            self.input.push_toks(vec![tc], "<futurelet>");
            self.pushed.push(tb);
        } else {
            self.skip_raw_spaces();
            // tex.web §1221: optional `=`, then at most ONE spacer, then the
            // value token (which may itself be a space — expl3 `\let\x=~`).
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
            if t.is_cs() {
                self.copy_meaning(target, t.cs_id());
            } else if t.0 >= crate::expand::PAR_REF_FLAG && t.0 < 0xFFFF_0000 && t.0 != crate::input::PAR_END.0 {
                self.error("Missing control sequence after \\let");
            } else {
                self.eqtb.assign(target, Equiv::CharTok(t.0), self.global_flag);
            }
        }
        self.global_flag = false;
    }

    /// copy meaning from src cs to dst cs (TeX \let semantics)
    fn copy_meaning(&mut self, dst: CsId, src: CsId) {
        match self.eqtb.resolve(src).cloned() {
            None => self.eqtb.undefine(dst, self.global_flag),
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
                    if !self.try_assignment(p, id) {
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
    pub fn scan_token_list(&mut self) -> Vec<Token> {
        self.skip_spaces_relax();
        let t = self.get_token();
        if t.is_cs() {
            match self.eqtb.resolve(t.cs_id()).cloned() {
                Some(Equiv::ToksReg(i)) => return (*self.eqtb.toks[i as usize]).clone(),
                Some(Equiv::Prim(Prim::ToksP(p))) => return (*self.eqtb.tok_params[p.idx() as usize]).clone(),
                Some(Equiv::Prim(Prim::CsName)) => {
                    // \toks0=\csname...\endcsname
                    let id = self.scan_csname_explicit();
                    return vec![Token::from_cs(id)];
                }
                _ => {}
            }
        }
        if !(t.is_char() && t.cc() == 1) {
            // tex.web scan_toks recovery: a non-brace, non-register leading token
            // (e.g. \expandafter in `\toks-assign\expandafter{...}`) is stored and
            // the following balanced group is collected into the list
            let mut out = vec![t];
            let mut depth = 0i32;
        loop {
            let t2 = self.get_token();
            if t2 == crate::input::EOF_MARKER {
                self.error("File ended while scanning token list (recovery loop, first-stored was the token above)");
                self.end_occurred = true;
                return out;
            }
            if t2.is_char() {
                let cc = t2.cc();
                if cc == 1 {
                    depth += 1;
                    if depth == 1 { continue; } // inserted brace not stored
                } else if cc == 2 {
                    depth -= 1;
                    if depth == 0 { return out; }
                }
            }
            out.push(t2);
        }
        }
        // normal path: leading { already in t
        { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/control.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
        let t = self.get_token();
        if !(t.is_char() && t.cc() == 1) {
            self.error("Missing { for token list");
            return Vec::new();
        }
        self.scan_balanced_raw()
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

    /// `{`, \bgroup, \begingroup: start a list group packed into a box on close
    pub fn begin_group(&mut self, _brace: bool) {
        if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) {
            eprintln!("BEGINGROUP-ctl kinds->{} saved->{}", self.box_kinds.len()+1, self.saved_lists.len()+1);
        }
        self.saved_lists.push((self.mode, std::mem::take(&mut self.cur_list), self.prev_depth, self.space_factor));
        self.eqtb.push_level(LevelType::Group);
        self.box_targets.push(None);
        self.box_shifts.push(0);
        // kind 4 = plain group: pack by outer mode
        let kind = if self.mode.is_v() { 5 } else { 6 };
        self.box_kinds.push(kind);
        match self.mode {
            Mode::Vertical => {
                // the outer vlist continues after the group; swap in fresh list
                let page = std::mem::take(&mut self.page_list);
                self.par_page_lists.push(page);
                self.mode = Mode::InternalVertical;
                self.prev_depth = -1000 * 65536;
            }
            Mode::InternalVertical => {
                self.cur_list = Vec::new();
                self.prev_depth = -1000 * 65536;
            }
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                self.cur_list = Vec::new();
            }
            Mode::Math | Mode::DisplayMath => {
                self.cur_list = Vec::new();
            }
        }
    }

    pub fn end_group(&mut self) {
        self.end_box();
    }
}
