//! Engine side of the LuaTeX-only primitives: `\Umathcode` and friends, the
//! `\Umath` parameters, the math style/character commands, hyphenation data
//! (`\hjcode`, `\prehyphenchar`, ...) and the direction commands.
//!
//! Sources: LuaTeX 1.24 `texmath.c` (`scan_mathchar`, `do_scan_extdef_del_code`,
//! `set_math_char`), `mathcodes.c`, `maincontrol.c` (`prefixed_command`,
//! `set_math_param_cmd`) and `scanning.c` (`scan_something_internal`).

use crate::boxes::{Glue, Node};
use crate::engine::{Engine, EngineKind, Mode};
use crate::eqtb::Equiv;
use crate::prim::{GlueParam, IntParam, Prim};
use crate::token::{CsId, Token};
use crate::uprim::{mp, UPrim, UMATH_NAMES};

/// The three notations `scan_mathchar` and `do_scan_extdef_del_code` read.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MathExt {
    /// `\mathcode`, `\delcode`: `"TFCC`
    Tex,
    /// `\Umathcode`, `\Udelcode`: three (two) integers
    U,
    /// `\Umathcodenum`, `\Udelcodenum`: one integer
    UNum,
}

/// A value `scan_something_internal` returns for a LuaTeX-only primitive.
pub(crate) enum UInternal {
    Int(i32),
    Dimen(i32),
    /// `\Umath` spacing parameter (mu glue)
    Glue(Glue),
}

impl UInternal {
    /// The value as `scan_int` coerces it (a glue to its width).
    pub(crate) fn as_int(&self) -> i32 {
        match self {
            UInternal::Int(n) | UInternal::Dimen(n) => *n,
            UInternal::Glue(g) => g.width,
        }
    }
}

/// `mathchar_from_integer(value, umath_mathcode)`: `(class, family, char)`.
pub(crate) fn decode_umath_num(value: i32) -> (u32, u32, u32) {
    let family = (value / 0x20_0000) & 0x7FF;
    ((family % 8) as u32, (family / 8) as u32, (value & 0x1F_FFFF) as u32)
}

/// `(class + 8 * family) * 0x200000 + character` as LuaTeX's 32 bit int.
pub(crate) fn encode_umath_num(class: i32, family: i32, character: i32) -> i32 {
    (class + 8 * family).wrapping_mul(0x20_0000).wrapping_add(character)
}

/// LuaTeX direction names (`dir_TLT` .. `dir_RTT`).
pub(crate) const DIRECTION_NAMES: [&str; 4] = ["TLT", "TRT", "LTL", "RTT"];

impl Engine {
    fn lua_engine(&self) -> bool {
        self.engine_kind == EngineKind::LuaTeX
    }

    /// `scan_char_num` of LuaTeX: 0 through 0x10FFFF.
    pub(crate) fn scan_char_num_lua(&mut self) -> i32 {
        let value = self.scan_int();
        if (0..=0x10FFFF).contains(&value) {
            value
        } else {
            self.error(&format!("Bad character code ({value})"));
            0
        }
    }

    /// texmath.c `scan_mathchar`: `(class, family, character)`.
    pub(crate) fn scan_mathchar_lua(&mut self, ext: MathExt) -> (i32, i32, i32) {
        match ext {
            MathExt::Tex => {
                let value = self.scan_int();
                if value > 0x8000 {
                    // needed for LaTeX: fall back to \Umathcodenum
                    self.mathchar_from_num(value)
                } else {
                    let mut value = value;
                    if value < 0 {
                        self.error(&format!("Bad mathchar ({value})"));
                        value = 0;
                    }
                    (value / 0x1000, (value % 0x1000) / 0x100, value % 0x100)
                }
            }
            MathExt::U => {
                let class = self.scan_int();
                let family = self.scan_int();
                let character = self.scan_char_num_lua();
                if !(0..=7).contains(&class) || family > 255 {
                    self.error("Invalid math code");
                    (0, 0, 0)
                } else {
                    (class, family, character)
                }
            }
            MathExt::UNum => {
                let value = self.scan_int();
                self.mathchar_from_num(value)
            }
        }
    }

    fn mathchar_from_num(&mut self, value: i32) -> (i32, i32, i32) {
        let (class, family, character) = decode_umath_num(value);
        if character > 0x10FFFF {
            self.error("Invalid math code");
            (0, 0, 0)
        } else {
            (class as i32, family as i32, character as i32)
        }
    }

    /// texmath.c `do_scan_extdef_del_code`: `(class, small family, small
    /// char, large family, large char)`.
    pub(crate) fn scan_delcode_lua(&mut self, ext: MathExt, with_class: bool) -> (i32, i32, i32, i32, i32) {
        match ext {
            MathExt::Tex => {
                let mut value = self.scan_int();
                let mut class = 0;
                if with_class {
                    class = value / 0x100_0000;
                    value &= 0xFF_FFFF;
                }
                if value > 0xFF_FFFF {
                    self.error("Invalid delimiter code");
                    value = 0;
                }
                (class, value / 0x10_0000, (value % 0x10_0000) / 0x1000, (value & 0xFFF) / 0x100, value % 0x100)
            }
            MathExt::U => {
                let class = if with_class { self.scan_int() } else { 0 };
                let mut family = self.scan_int();
                let mut character = self.scan_char_num_lua();
                if !(0..=255).contains(&family) {
                    self.error("Invalid delimiter code");
                    family = 0;
                    character = 0;
                }
                (class, family, character, 0, 0)
            }
            MathExt::UNum => {
                let value = self.scan_int();
                let mut family = value / 0x20_0000;
                let mut character = value & 0x1F_FFFF;
                if !(0..=255).contains(&family) || character > 0x10FFFF {
                    self.error("Invalid delimiter code");
                    family = 0;
                    character = 0;
                }
                (0, family, character, 0, 0)
            }
        }
    }

    /// `\mathcode`/`\Umathcode`/`\Umathcodenum` assignment.
    pub(crate) fn assign_lua_math_code_command(&mut self, ext: MathExt, name: &str) {
        let global = self.take_assignment_prefixes(name);
        let character = self.scan_char_num_lua();
        self.scan_optional_equals();
        let (class, family, slot) = self.scan_mathchar_lua(ext);
        self.eqtb.assign_lua_math_code(character as u32, class, family, slot, global);
    }

    /// `\delcode`/`\Udelcode`/`\Udelcodenum` assignment.
    pub(crate) fn assign_lua_del_code_command(&mut self, ext: MathExt, name: &str) {
        let global = self.take_assignment_prefixes(name);
        let character = self.scan_char_num_lua();
        self.scan_optional_equals();
        let (_, small_family, small_char, large_family, large_char) = self.scan_delcode_lua(ext, false);
        self.eqtb.assign_lua_del_code(character as u32, small_family, small_char, large_family, large_char, global);
    }

    /// `\Umath<param><style>` as `scan_something_internal` reads it.
    fn umath_internal(&mut self, id: u32, style: u8) -> UInternal {
        if id == mp::MATH_PARAM_RADICAL_DEGREE_RAISE {
            UInternal::Int(self.eqtb.math_param(id, style))
        } else if id < mp::MATH_PARAM_FIRST_MU_GLUE {
            UInternal::Dimen(self.eqtb.math_param(id, style))
        } else {
            UInternal::Glue(self.math_glue_value(id, style))
        }
    }

    /// The glue an unset spacing parameter reads as is zero; one that
    /// refers to `\thinmuskip` and friends reads that parameter.
    pub(crate) fn math_glue_value(&self, id: u32, style: u8) -> Glue {
        match self.eqtb.math_glue_param(id, style) {
            Some([0, w, st, sh, sto, sho]) => {
                Glue::spec(w, st, sto as u8, sh, sho as u8)
            }
            Some([kind, ..]) => {
                let param = match kind {
                    1 => GlueParam::ThinMuSkip,
                    2 => GlueParam::MedMuSkip,
                    _ => GlueParam::ThickMuSkip,
                };
                self.eqtb.glue_params[param.idx() as usize].eqtb_value()
            }
            None => Glue::zero(),
        }
    }

    /// Fetch the value of a LuaTeX-only internal quantity
    /// (`scan_something_internal`); `None` when `p` is not one.
    pub(crate) fn uprim_internal(&mut self, p: Prim) -> Option<UInternal> {
        match p {
            Prim::UMath(id) => {
                let style = self.scan_math_style();
                Some(self.umath_internal(u32::from(id), style))
            }
            Prim::U(u) => match u {
                UPrim::UMathCode | UPrim::UMathCodeNum => {
                    let c = self.scan_char_num_lua();
                    Some(UInternal::Int(self.eqtb.lua_math_code_num(c as u32)))
                }
                UPrim::UDelCode | UPrim::UDelCodeNum => {
                    let c = self.scan_char_num_lua();
                    Some(UInternal::Int(self.eqtb.lua_del_code_num(c as u32)))
                }
                UPrim::HjCode => {
                    let c = self.scan_char_num_lua();
                    Some(UInternal::Int(self.hj_code(c)))
                }
                UPrim::HyphenationMin
                | UPrim::PreHyphenChar
                | UPrim::PostHyphenChar
                | UPrim::PreExHyphenChar
                | UPrim::PostExHyphenChar => Some(UInternal::Int(self.language_param(u))),
                UPrim::BoxDirection => {
                    let n = self.scan_reg_num();
                    Some(UInternal::Int(self.box_direction(n)))
                }
                UPrim::TextDir | UPrim::ParDir | UPrim::BodyDir | UPrim::PageDir | UPrim::MathDir | UPrim::LineDir => {
                    Some(UInternal::Int(self.direction_param(u)))
                }
                UPrim::BoxDir => {
                    let n = self.scan_reg_num();
                    Some(UInternal::Int(self.box_direction(n)))
                }
                _ => None,
            },
            _ => None,
        }
    }

    // ---- hyphenation data (`hyph_data_cmd`, texlang.c) ----

    /// The language `\prehyphenchar` and friends apply to.
    fn current_language_id(&self) -> u8 {
        let v = self.eqtb.int_params[IntParam::Language.idx() as usize];
        if (1..=255).contains(&v) {
            v as u8
        } else {
            0
        }
    }

    fn language_param(&mut self, u: UPrim) -> i32 {
        let id = self.current_language_id();
        let p = self.lua_tex.lang.get(&id).copied().unwrap_or_default();
        match u {
            UPrim::PreHyphenChar => p.pre_hyphen.unwrap_or(i32::from(b'-')),
            UPrim::PostHyphenChar => p.post_hyphen,
            UPrim::PreExHyphenChar => p.pre_exhyphen,
            UPrim::PostExHyphenChar => p.post_exhyphen,
            _ => p.hyphenation_min,
        }
    }

    fn set_language_param(&mut self, u: UPrim, value: i32) {
        let id = self.current_language_id();
        let p = self.lua_tex.lang.entry(id).or_default();
        match u {
            UPrim::PreHyphenChar => p.pre_hyphen = Some(value),
            UPrim::PostHyphenChar => p.post_hyphen = value,
            UPrim::PreExHyphenChar => p.pre_exhyphen = value,
            UPrim::PostExHyphenChar => p.post_exhyphen = value,
            _ => p.hyphenation_min = value,
        }
    }

    /// `\hjcode` of the current language (texlang.c `get_hj_code`).
    fn hj_code(&self, c: i32) -> i32 {
        self.hj_code_of(self.current_language_id(), c)
    }

    fn set_hj_code(&mut self, c: i32, value: i32) {
        let id = self.current_language_id();
        if !self.set_hj_code_of(id, c, value) {
            self.error("Invalid hjcode: the character or the code is out of range");
        }
    }

    // ---- directions ----

    fn direction_param(&self, u: UPrim) -> i32 {
        let p = match u {
            UPrim::ParDir => IntParam::ParDirection,
            UPrim::BodyDir => IntParam::BodyDirection,
            UPrim::PageDir => IntParam::PageDirection,
            UPrim::MathDir => IntParam::MathDirection,
            // line_direction reads as the text direction
            _ => IntParam::TextDirection,
        };
        self.eqtb.int_params[p.idx() as usize]
    }

    fn box_direction(&self, n: u16) -> i32 {
        match self.eqtb.boxed.get(n as usize) {
            Some(Some(Node::Box { dir, .. })) => i32::from(*dir),
            _ => 0,
        }
    }

    /// directions.c `scan_direction`: a direction parameter or keyword.
    pub(crate) fn scan_direction(&mut self) -> i32 {
        let t = self.get_x_raw();
        if t.is_cs() {
            if let Some(Equiv::Prim(Prim::U(
                u @ (UPrim::TextDir | UPrim::ParDir | UPrim::BodyDir | UPrim::PageDir | UPrim::MathDir | UPrim::LineDir),
            ))) = self.eqtb.resolve(t.cs_id()).cloned()
            {
                return self.direction_param(u);
            }
        }
        self.push_token(t);
        let value = if self.scan_keyword(b"tlt") {
            0
        } else if self.scan_keyword(b"trt") {
            1
        } else if self.scan_keyword(b"ltl") {
            2
        } else if self.scan_keyword(b"rtt") {
            3
        } else {
            self.error("Bad direction");
            0
        };
        let t = self.get_x_raw();
        if !t.is_space() {
            self.push_token(t);
        }
        value
    }

    /// `\Umath<param><style>=<value>` (`set_math_param_cmd`).
    pub(crate) fn set_math_param_command(&mut self, id: u8) {
        let id = u32::from(id);
        let name = format!("\\{}", String::from_utf8_lossy(UMATH_NAMES[id as usize]));
        let global = self.take_assignment_prefixes(&name);
        let style = self.scan_math_style();
        self.scan_optional_equals();
        if id < mp::MATH_PARAM_FIRST_MU_GLUE {
            let value = if id == mp::MATH_PARAM_RADICAL_DEGREE_RAISE {
                self.scan_int()
            } else {
                self.scan_dimen(false, false)
            };
            self.eqtb.assign_math_param(id, style, value, global);
        } else {
            // a bare `\thinmuskip` (`\medmuskip`, `\thickmuskip`) refers to
            // the parameter instead of copying its current value
            let t = self.get_x_raw();
            let kind = if t.is_cs() {
                match self.eqtb.resolve(t.cs_id()) {
                    Some(Equiv::Prim(Prim::GlueP(GlueParam::ThinMuSkip))) => 1,
                    Some(Equiv::Prim(Prim::GlueP(GlueParam::MedMuSkip))) => 2,
                    Some(Equiv::Prim(Prim::GlueP(GlueParam::ThickMuSkip))) => 3,
                    _ => 0,
                }
            } else {
                0
            };
            let value = if kind != 0 {
                [kind, 0, 0, 0, 0, 0]
            } else {
                self.push_token(t);
                let g = self.scan_glue(true);
                [0, g.width, g.stretch, g.shrink, i32::from(g.stretch_order), i32::from(g.shrink_order)]
            };
            self.eqtb.assign_math_glue_param(id, style, value, global);
        }
    }

    /// Assignment commands among the LuaTeX-only primitives; `false` when
    /// `u` is not an assignment.
    pub(crate) fn uprim_assign(&mut self, u: UPrim, _id: CsId) -> bool {
        match u {
            UPrim::UMathCode => self.assign_lua_math_code_command(MathExt::U, "\\Umathcode"),
            UPrim::UMathCodeNum => self.assign_lua_math_code_command(MathExt::UNum, "\\Umathcodenum"),
            UPrim::UDelCode => self.assign_lua_del_code_command(MathExt::U, "\\Udelcode"),
            UPrim::UDelCodeNum => self.assign_lua_del_code_command(MathExt::UNum, "\\Udelcodenum"),
            UPrim::UMathCharDef | UPrim::UMathCharNumDef => {
                let (ext, name) = if u == UPrim::UMathCharDef {
                    (MathExt::U, "\\Umathchardef")
                } else {
                    (MathExt::UNum, "\\Umathcharnumdef")
                };
                let t = self.scan_definable_cs();
                self.scan_optional_equals();
                let (class, family, character) = self.scan_mathchar_lua(ext);
                let global = self.take_assignment_prefixes(name);
                self.eqtb.assign(t, Equiv::UMathCharDef(encode_umath_num(class, family, character)), global);
            }
            UPrim::HjCode => {
                self.take_assignment_prefixes("\\hjcode");
                let c = self.scan_char_num_lua();
                self.scan_optional_equals();
                let v = self.scan_int();
                self.set_hj_code(c, v);
            }
            UPrim::HyphenationMin
            | UPrim::PreHyphenChar
            | UPrim::PostHyphenChar
            | UPrim::PreExHyphenChar
            | UPrim::PostExHyphenChar => {
                self.take_assignment_prefixes("\\hyphenationmin");
                self.scan_optional_equals();
                let v = self.scan_int();
                self.set_language_param(u, v);
            }
            UPrim::SetFontId => {
                let global = self.take_assignment_prefixes("\\setfontid");
                let n = self.scan_int();
                if usize::try_from(n).is_ok_and(|n| n < self.eqtb.fonts.len()) {
                    self.eqtb.define_cur_font(n as u16, global);
                }
            }
            UPrim::TextDir | UPrim::ParDir | UPrim::BodyDir | UPrim::PageDir | UPrim::MathDir | UPrim::LineDir => {
                let global = self.take_assignment_prefixes("\\textdir");
                let value = self.scan_direction();
                self.assign_direction(u, value, global);
            }
            _ => return false,
        }
        true
    }

    /// `\textdir`/`\pardir`/... after the value is scanned
    /// (maincontrol.c `prefixed_command`, `assign_dir_cmd`).
    pub(crate) fn assign_direction(&mut self, u: UPrim, value: i32, global: bool) {
        let value = if (0..=3).contains(&value) { value } else { 0 };
        let param = match u {
            UPrim::ParDir => IntParam::ParDirection,
            UPrim::BodyDir => IntParam::BodyDirection,
            UPrim::PageDir => IntParam::PageDirection,
            UPrim::MathDir => IntParam::MathDirection,
            _ => IntParam::TextDirection,
        };
        if !matches!(u, UPrim::TextDir | UPrim::LineDir) {
            self.eqtb.assign_int_param(param, value, global);
            return;
        }
        // maincontrol.c `assign_dir_cmd`, text_direction_code: end the
        // direction in force (when a \textdir of this group is), begin the
        // new one, and count the change for the group's end
        let level = self.eqtb.cur_level;
        let attr = self.eqtb.cur_attr;
        if self.mode.is_h() {
            let current = self.eqtb.int_params[IntParam::TextDirection.idx() as usize] as u8;
            if self.eqtb.int_params[IntParam::NoLocalDirs.idx() as usize] > 0 {
                let cancel = Node::Whatsit(crate::boxes::WhatIt::Dir { dir: current, cancel: true, level: 0 }, attr);
                // \linedir goes before a trailing glue so the glue stays
                // outside the direction it ends
                let before_glue = u == UPrim::LineDir && matches!(self.cur_list.last(), Some(Node::Glue(..)));
                if before_glue {
                    let at = self.cur_list.len() - 1;
                    self.cur_list.insert(at, cancel);
                } else {
                    self.cur_list.push(cancel);
                }
            }
        }
        match self.text_dirs.last_mut() {
            Some(top) if top.0 == level => top.1 = value as u8,
            _ => self.text_dirs.push((level, value as u8)),
        }
        if self.mode.is_h() {
            self.cur_list.push(Node::Whatsit(crate::boxes::WhatIt::Dir { dir: value as u8, cancel: false, level }, attr));
        }
        self.eqtb.assign_int_param(IntParam::TextDirection, value, global);
        let counted = self.eqtb.int_params[IntParam::NoLocalDirs.idx() as usize] + 1;
        self.eqtb.assign_int_param(IntParam::NoLocalDirs, counted, false);
    }

    /// `\hrule`, `\vrule` and luatex's `\nohrule`/`\novrule` (`subtype`
    /// [`RULE_EMPTY`](crate::boxes::RULE_EMPTY)) in main control.
    pub(crate) fn rule_command(&mut self, id: CsId, horizontal: bool, subtype: u8) {
        if horizontal {
            if self.mode == Mode::Horizontal {
                self.push_token(Token::from_cs(id));
                self.push_token(Token::from_cs(self.ids.par));
                return;
            }
            if self.mode == Mode::RestrictedHorizontal {
                // tex.web head_for_vmode: only leaders may hold a rule in
                // restricted horizontal mode
                self.error("You can't use `\\hrule' here except with leaders");
                return;
            }
        } else if self.mode.is_v() {
            self.push_token(Token::from_cs(id));
            self.start_paragraph(true);
            return;
        }
        self.make_rule(horizontal, subtype);
    }

    /// LuaTeX-only primitives that main control executes (neither
    /// assignments nor expandable).
    pub(crate) fn uprim_command(&mut self, u: UPrim, id: CsId) {
        match u {
            UPrim::BoxDir | UPrim::BoxDirection => {
                let n = self.scan_reg_num();
                self.scan_optional_equals();
                let value = if u == UPrim::BoxDir {
                    self.scan_direction()
                } else {
                    let v = self.scan_int();
                    if (0..=3).contains(&v) { v } else { 0 }
                };
                if let Some(Some(Node::Box { dir, .. })) = self.eqtb.boxed.get_mut(n as usize) {
                    *dir = value as u8;
                }
            }
            UPrim::UMathChar => self.math_char_num_command(MathExt::U, id),
            UPrim::UMathCharNum => self.math_char_num_command(MathExt::UNum, id),
            UPrim::UDelimiter => {
                if self.mode.is_v() {
                    self.push_token(Token::from_cs(id));
                    self.start_paragraph(true);
                } else if !self.insert_dollar_unless_math(id) {
                    let source = self.current_token_source_mark();
                    let (class, family, character, _, _) = self.scan_delcode_lua(MathExt::U, true);
                    self.set_math_char_lua(class as u32, family as u32, character as u32, 0, source);
                }
            }
            UPrim::URadical => {
                if !self.lua_insert_dollar(id) {
                    let source = self.current_token_source_mark();
                    self.math_radical_lua(1, source);
                }
            }
            UPrim::UMathAccent => {
                if !self.lua_insert_dollar(id) {
                    let source = self.current_token_source_mark();
                    self.math_ac_lua(1, false, source);
                }
            }
            UPrim::CrampedDisplayStyle
            | UPrim::CrampedTextStyle
            | UPrim::CrampedScriptStyle
            | UPrim::CrampedScriptScriptStyle => {
                if !self.insert_dollar_unless_math(id) {
                    let style = match u {
                        UPrim::CrampedDisplayStyle => crate::boxes::MathStyle::CrampedDisplay,
                        UPrim::CrampedTextStyle => crate::boxes::MathStyle::CrampedText,
                        UPrim::CrampedScriptStyle => crate::boxes::MathStyle::CrampedScript,
                        _ => crate::boxes::MathStyle::CrampedScriptScript,
                    };
                    if let Some(s) = self.math_style_stack.last_mut() {
                        *s = style;
                    }
                    self.append_mlist_node(Node::Style(style, self.eqtb.cur_attr));
                }
            }
            UPrim::USubscript | UPrim::USuperscript => {
                if !self.insert_dollar_unless_math(id) {
                    self.append_script(u == UPrim::USuperscript, 0);
                }
            }
            UPrim::UNoSubscript | UPrim::UNoSuperscript => {
                if !self.insert_dollar_unless_math(id) {
                    self.append_script_opt(u == UPrim::UNoSuperscript, 0, true);
                }
            }
            UPrim::LateLua => {
                let toks = self.scan_general_text_expanded();
                let code = self.tokens_to_lua_text(&toks).into_bytes();
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::LateLua { code, func: 0 }, self.eqtb.cur_attr));
            }
            UPrim::LateLuaFunction => {
                let n = self.scan_int();
                if n <= 0 {
                    self.error("LuaTeX error (lateluafunction: invalid number)");
                } else {
                    self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::LateLua { code: Vec::new(), func: n }, self.eqtb.cur_attr));
                }
            }
            UPrim::ClearMarks => {
                let class = self.scan_int();
                let node = Node::Mark { class, tokens: Vec::new(), attr: self.eqtb.cur_attr };
                match self.mode {
                    Mode::Vertical | Mode::InternalVertical => self.vlist_append(node),
                    Mode::Math | Mode::DisplayMath => self.append_mlist_node(node),
                    _ => self.cur_list.push(node),
                }
            }
            UPrim::AutomaticDiscretionary => {
                if self.mode.is_v() {
                    // as for \-: the paragraph starts before the discretionary
                    self.push_token(Token::from_cs(id));
                    self.start_paragraph(true);
                } else {
                    self.append_hyphen_discretionary(true);
                }
            }
            UPrim::EndLocalControl => self.end_local_control(),
            UPrim::GLeaders => self.begin_leaders(crate::boxes::LEADERS_G),
            UPrim::NoHRule => self.rule_command(id, true, crate::boxes::RULE_EMPTY),
            UPrim::NoVRule => self.rule_command(id, false, crate::boxes::RULE_EMPTY),
            UPrim::LeftGhost | UPrim::RightGhost => self.char_ghost(id, u == UPrim::RightGhost),
            UPrim::LocalLeftBox | UPrim::LocalRightBox => self.append_local_box(u == UPrim::LocalRightBox),
            UPrim::URoot
            | UPrim::UUnderDelimiter
            | UPrim::UOverDelimiter
            | UPrim::UDelimiterUnder
            | UPrim::UDelimiterOver
            | UPrim::UHExtensible => {
                if !self.lua_insert_dollar(id) {
                    let source = self.current_token_source_mark();
                    let chr = match u {
                        UPrim::URoot => 2,
                        UPrim::UUnderDelimiter => 3,
                        UPrim::UOverDelimiter => 4,
                        UPrim::UDelimiterUnder => 5,
                        UPrim::UDelimiterOver => 6,
                        _ => 7,
                    };
                    self.math_radical_lua(chr, source);
                }
            }
            UPrim::UVExtensible => {
                if !self.lua_insert_dollar(id) {
                    let source = self.current_token_source_mark();
                    self.math_vextensible(source);
                }
            }
            UPrim::USkewed | UPrim::USkewedWithDelims => {
                if !self.lua_insert_dollar(id) {
                    self.do_fraction_kind(crate::math::FracKind::Skewed, u == UPrim::USkewedWithDelims);
                }
            }
            UPrim::UStartDisplayMath => self.math_shift_cs(0, id),
            UPrim::UStopDisplayMath => self.math_shift_cs(1, id),
            UPrim::MathOption => self.error("LuaTeX error (mathoption: obsolete command)"),
            _ => {
                let name = String::from_utf8_lossy(self.cs.name(id)).into_owned();
                self.error(&format!("Command \\{name} is recognized but not implemented by this engine"));
            }
        }
    }

    /// texmath.c `set_math_char`: append the noad of a math code. A class 8
    /// (active) code runs the active character `active` instead.
    pub(crate) fn set_math_char_lua(
        &mut self,
        class: u32,
        family: u32,
        character: u32,
        active: u32,
        source: Option<crate::input::SourceMark>,
    ) {
        if class == 8 {
            self.active_char(active);
            return;
        }
        let in_range = |v: i32| (0..=255).contains(&v);
        let cur_fam = self.eqtb.int_params[IntParam::CurFam.idx() as usize];
        let var_fam = self.eqtb.int_params[IntParam::VariableFam.idx() as usize];
        let mut fam = family;
        let node_class;
        if class == 7 {
            if in_range(cur_fam) {
                fam = cur_fam as u32;
            }
            node_class = crate::math::CL_ORD;
        } else if family as i32 == var_fam && in_range(var_fam) {
            if in_range(cur_fam) {
                fam = cur_fam as u32;
            }
            node_class = crate::math::CL_ORD;
        } else {
            node_class = class as u8;
        }
        let origin = self.math_diagnostic_origin_at(source);
        self.append_mlist_node(Node::MathChar { fam: fam as u8, c: character, class: node_class, origin, attr: self.eqtb.cur_attr });
    }

    /// texmath.c `math_char_in_text`: a math character outside math is a
    /// character of the family's text font.
    fn math_char_in_text(&mut self, class: u32, family: u32, character: u32, active: u32) {
        if class == 8 {
            self.active_char(active);
            return;
        }
        let font = self.eqtb.style_fonts[0][(family as usize) & 0xFF];
        let saved = std::mem::replace(&mut self.eqtb.cur_font_val, font);
        self.unicode_char_token(character, false);
        self.eqtb.cur_font_val = saved;
    }

    /// `\mathchar`, `\Umathchar` and `\Umathcharnum`.
    pub(crate) fn math_char_num_command(&mut self, ext: MathExt, id: CsId) {
        if self.mode.is_v() {
            // maincontrol.c run_non_math_math: start a paragraph, then run
            // the command again
            self.push_token(Token::from_cs(id));
            self.start_paragraph(true);
            return;
        }
        let source = self.current_token_source_mark();
        let (class, family, character) = self.scan_mathchar_lua(ext);
        let active = match ext {
            MathExt::Tex => 0,
            MathExt::U => 1,
            MathExt::UNum => 2,
        };
        if self.mode.is_m() {
            self.set_math_char_lua(class as u32, family as u32, character as u32, active, source);
        } else {
            self.math_char_in_text(class as u32, family as u32, character as u32, active);
        }
    }

    /// A control sequence defined by `\mathchardef` (`value` is the plain
    /// 15 bit code) or `\Umathchardef` (`value` is the packed integer).
    pub(crate) fn math_given_command(&mut self, value: i32, umath: bool, id: CsId) {
        if self.mode.is_v() {
            self.push_token(Token::from_cs(id));
            self.start_paragraph(true);
            return;
        }
        let (class, family, character) = if umath {
            decode_umath_num(value)
        } else {
            ((value / 0x1000) as u32, ((value % 0x1000) / 0x100) as u32, (value % 0x100) as u32)
        };
        let source = self.current_token_source_mark();
        if self.mode.is_m() {
            self.set_math_char_lua(class, family, character, 0, source);
        } else {
            self.math_char_in_text(class, family, character, 0);
        }
    }
}

impl Engine {
    fn exp_number(&mut self, text: String) {
        self.exp_string(text.as_bytes());
    }

    /// Expandable LuaTeX-only primitives (`convert_cmd`, `if_test_cmd`,
    /// `input_cmd`).
    pub(crate) fn uprim_expand(&mut self, u: UPrim) -> Option<Token> {
        match u {
            UPrim::UChar => {
                let c = self.scan_char_num_lua();
                let t = if c == 32 { Token::space() } else { Token::unicode_char(12, c as u32) };
                self.push_token(t);
            }
            UPrim::UMathCharClass | UPrim::UMathCharFam | UPrim::UMathCharSlot => {
                let c = self.scan_int();
                // the tree is indexed by the low 21 bits; a default code
                // keeps the full number as its character
                let key = (c as u32) & 0x1F_FFFF;
                let (class, family, mut slot) = self.eqtb.lua_math_code(key);
                if (class, family, slot) == (0, 0, key) && key > 255 {
                    slot = c as u32;
                }
                let v = match u {
                    UPrim::UMathCharClass => class as i32,
                    UPrim::UMathCharFam => family as i32,
                    _ => slot as i32,
                };
                self.exp_number(v.to_string());
            }
            UPrim::MathStyleValue => {
                let v = if self.mode.is_m() { i32::from(self.current_style_number()) } else { -1 };
                self.exp_number(v.to_string());
            }
            UPrim::IfCondition => {}
            UPrim::ScanTextokens => {
                let toks = self.scan_general_text();
                let text = self.tokens_to_bytes(&toks);
                if self.ensure_input_stack_room(1) {
                    self.input.push_file("<scantextokens>".to_string(), text);
                }
            }
            UPrim::ImmediateAssignment | UPrim::ImmediateAssigned => {
                // textoken.c: skip spaces and \relax, then run assignments
                let mut t;
                loop {
                    t = self.get_x_raw();
                    if !(t.is_space() || self.is_relax_token(t)) {
                        break;
                    }
                }
                if u == UPrim::ImmediateAssignment {
                    self.immediate_assignment(t);
                } else if t.is_char() && t.cc() == 1 {
                    loop {
                        let mut t;
                        loop {
                            t = self.get_x_raw();
                            if !(t.is_space() || self.is_relax_token(t)) {
                                break;
                            }
                        }
                        if t.is_char() && t.cc() == 2 {
                            break;
                        }
                        self.immediate_assignment(t);
                    }
                } else {
                    self.push_token(t);
                }
            }
            _ => {}
        }
        None
    }

    fn is_relax_token(&self, t: Token) -> bool {
        t.is_cs() && matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(Prim::Relax)))
    }

    /// Run `t` if it starts an assignment, otherwise put it back.
    fn immediate_assignment(&mut self, t: Token) {
        if t.is_cs() {
            let id = t.cs_id();
            if let Some(Equiv::Prim(p)) = self.eqtb.resolve(id).cloned() {
                if self.try_assignment(p, id) {
                    return;
                }
            } else if matches!(
                self.eqtb.resolve(id),
                Some(
                    Equiv::CountReg(_)
                        | Equiv::DimenReg(_)
                        | Equiv::SkipReg(_)
                        | Equiv::MuSkipReg(_)
                        | Equiv::ToksReg(_)
                        | Equiv::AttributeReg(_)
                )
            ) {
                self.cs_assign(id);
                return;
            }
        }
        self.push_token(t);
    }

    /// LuaTeX `print_math_style` value inside math (without cramped
    /// information: the engine tracks only the four base styles at scan time).
    pub(crate) fn current_style_number(&self) -> u8 {
        crate::math::gstyle_of(self.cur_math_style())
    }
}

impl Engine {
    /// maincontrol.c `non_math(..., insert_dollar_sign)`: outside math the
    /// command is read again after an inserted `$`. True when that happened.
    fn insert_dollar_unless_math(&mut self, id: CsId) -> bool {
        if self.mode.is_m() {
            return false;
        }
        self.push_token(Token::from_cs(id));
        self.push_token(Token::char(3, u32::from(b'$')));
        self.error("Missing $ inserted");
        true
    }
}
