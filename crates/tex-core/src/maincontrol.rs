//! main_dispatch: the big match over primitives executed in main control.

use crate::engine::Engine;
use crate::eqtb::{Equiv, Macro};
use crate::engine::{Mode, ScannerStatus};
use crate::boxes::{Glue, Node};
use crate::prim::*;
use crate::token::{CsId, Token};
use std::rc::Rc;

impl Engine {
    pub fn main_dispatch(&mut self, p: Prim, id: CsId) {
        use Prim::*;
        match p {
            Relax | EndCsName => {}
            BeginGroup => self.begin_semi_simple(),
            EndGroup => self.end_semi_simple(),
            BGroup => self.begin_group(false),
            EGroup => self.end_group(),

            Par => self.par_primitive(),
            Indent => self.start_paragraph(true),
            NoIndent => self.start_paragraph(false),
            HSkip | HFil | HFill | HFilL | HFilNeg | HSS => {
                let g = self.scan_hskip_kind(p);
                self.append_h_glue(g, false);
            }
            VSkip | VFil | VFill | VFilL | VFilNeg | VSS => {
                let g = self.scan_vskip_kind(p);
                self.append_v_glue(g);
            }
            MSkip => {
                let g = self.scan_glue(true);
                if self.mode.is_m() {
                    self.append_mlist_node(Node::Glue(g));
                } else {
                    self.error("There's no \\mskip here");
                }
            }
            Kern => {
                let d = self.scan_dimen(false, false);
                if self.mode.is_m() {
                    self.append_mlist_node(Node::Kern(d));
                } else if self.mode.is_v() {
                    self.append_v_kern(d);
                } else {
                    self.append_h_kern(d);
                }
            }
            MKern => {
                let d = self.scan_dimen(true, false);
                if self.mode.is_m() {
                    self.append_mlist_node(Node::MathKern(d, 0));
                } else {
                    self.error("There's no \\mkern here");
                }
            }
            HMove => {
                let d = self.scan_dimen(false, false);
                let neg = id_cs_is(self, id, b"moveleft");
                self.box_move(d, neg, true);
            }
            VMove => {
                let d = self.scan_dimen(false, false);
                let neg = id_cs_is(self, id, b"raise");
                self.box_move(d, neg, false);
            }
            HBox | VBox | VTop | VCenter => {
                let kind = match p {
                    HBox => 0u8,
                    VBox => 1,
                    VTop => 2,
                    _ => 3,
                };
                self.begin_box(kind);
            }
            HRule | VRule => self.make_rule(p == HRule),
            Leaders | CLeaders | XLeaders => {
                self.begin_leaders(match p { Leaders => 0, CLeaders => 1, _ => 2 });
            }
            Penalty => {
                let n = self.scan_int();
                if self.mode.is_v() {
                    self.append_v_penalty(n);
                } else if self.mode.is_m() {
                    self.append_mlist_node(Node::Penalty(n));
                } else {
                    self.append_h_penalty(n);
                }
            }
            Penalties => {
                // \penalties{...}: append each int as penalty
                let toks = self.scan_general_text();
                let _ = toks;
            }
            UnSkip => self.un_skip(),
            IgnoreSpaces => self.ignore_spaces(),
            UnKern => self.un_kern(),
            UnPenalty => self.un_penalty(),
            UnHBox => self.do_unbox(false, false),
            UnVBox => self.do_unbox(true, false),
            UnHCopy => self.do_unbox(false, true),
            UnVCopy => self.do_unbox(true, true),
            LastBox => {

                let b = self.take_last_box();
                self.append_take_node_opt(b);
            }
            LastKern => {
                let v = self.last_kern_value();
                self.append_take_node(Node::Kern(v));
            }
            LastPenalty => {
                let v = self.last_penalty_value();
                self.append_take_node(Node::Penalty(v));
            }
            LastSkip => {
                let g = self.last_skip_value();
                self.append_take_node(Node::Glue(g));
            }
            VSplit => self.do_vsplit(),
            Insert => self.do_insert(),
            VAdjust => {
                let toks = self.scan_general_text();
                self.append_vadjust(toks);
            }
            MarkPrim => {
                let _n = 0i32; // class 0 (e-TeX classes later)
                let toks = self.scan_general_text();
                self.append_mark(0, toks);
            }
            TopMark | FirstMark | BotMark | SplitFirstMark | SplitBotMark => {
                // only valid in \the
                self.error("You can't use this mark primitive here");
            }
            ShipOut => {
                // \shipout<box>: scan box spec
                self.do_shipout();
            }
            Font => self.do_font(),
            FontDimen | HyphenChar | SkewChar => {} // handled in try_assignment
            CatCode => {
                let c = self.scan_char_num();
                self.scan_optional_equals();
                let v = self.scan_int();
                if (0..=255).contains(&c) && (0..=15).contains(&v) {
                    self.eqtb.assign_cat(c as u8, v as u8, self.global_flag);
                    self.global_flag = false;
                }

            }
            MathCode => {
                let c = self.scan_char_num();
                self.scan_optional_equals();
                let v = self.scan_int();
                if (0..=255).contains(&c) {
                    self.eqtb.assign_math_code(c as u8, v as u16, self.global_flag);
                    self.global_flag = false;
                }
            }
            DelCode => {
                let c = self.scan_char_num();
                self.scan_optional_equals();
                let v = self.scan_int();
                if (0..=255).contains(&c) {
                    self.eqtb.assign_del_code(c as u8, v, self.global_flag);
                    self.global_flag = false;
                }
            }
            LcCodeP => {
                let c = self.scan_char_num();
                self.scan_optional_equals();
                let v = self.scan_int();
                if (0..=255).contains(&c) && (0..=32767).contains(&v) {
                    self.eqtb.assign_lc_code(c as u8, v as u8, self.global_flag);
                    self.global_flag = false;
                }
            }
            SfCodeP => {
                let c = self.scan_char_num();
                self.scan_optional_equals();
                let v = self.scan_int();
                if (0..=255).contains(&c) && (0..=32767).contains(&v) {
                    self.eqtb.assign_sf_code(c as u8, v as u16, self.global_flag);
                    self.global_flag = false;
                }
            }
            UcCodeP => {
                let c = self.scan_char_num();
                self.scan_optional_equals();
                let v = self.scan_int();
                if (0..=255).contains(&c) && (0..=32767).contains(&v) {
                    self.eqtb.assign_uc_code(c as u8, v as u8, self.global_flag);
                    self.global_flag = false;
                }
            }
            Lowercase | Uppercase => {
                let up = p == Uppercase;
                let mut toks = self.scan_general_text();
                self.shift_case(&mut toks, up);
                // tex.web §1289 back_list: shifted tokens must be read
                // *before* the rest of the current macro (on `pushed`).
                // input.push_toks would sit under `pushed`, so the {true}
                // {false} of utf8.def's \cdp@elt ran before \InputIfFileExists.
                self.push_tokens(toks);
            }
            Input => self.do_input(),
            EndInput => self.do_endinput(),
            Patterns | Hyphenation => self.do_hyphenation_words(p == Patterns),
            ScanTokens => {
                let _ = self.expand_prim(ScanTokens, id);
            }
            Dump => {
                if !self.ini_mode {
                    // tex.web:1308/1336: \dump exists only while format-building.
                    self.error("cannot \\dump outside -ini mode");
                } else {
                    match crate::format::check_dumpable(self) {
                        Ok(()) => {
                            self.format_done = true;
                            self.end_occurred = true;
                        }
                        Err(e) => self.error(&format!("cannot \\dump ({})", e)),
                    }
                }
            }
            End => {
                // tex.web its_all_over: \par if in hmode, then eject the last page
                if self.mode.is_h() && !self.mode.is_inner() {
                    self.par_primitive();
                }
                if self.mode.is_v() && !self.page_list.is_empty() {
                    let n = self.page_list.len();
                    self.eject_page(n);
                    return;
                }
                self.end_occurred = true;
            }
            Immediate => {
                let t = self.get_token();
                if t.is_cs() {
                    let id2 = t.cs_id();
                    if let Some(Equiv::Prim(p2)) = self.eqtb.resolve(id2).cloned() {
                        match p2 {
                            Prim::Write => self.do_write(),
                            Prim::Special => self.do_special(),
                            Prim::OpenOut => self.do_openout(),
                            Prim::CloseOut => self.do_closeout(),
                            _ => self.main_dispatch(p2, id2),
                        }
                    }
                }
            }
            OpenOut => self.do_openout(),
            CloseOut => self.do_closeout(),
            Write => self.do_write(),
            Special => self.do_special(),
            Message => self.do_message(false),
            ErrMessage => self.do_message(true),
            DirectLua => self.do_directlua(),
            OpenIn => self.do_openin(),
            CloseIn => self.do_closein(),
            Read => self.do_read(false),
            ReadLine => self.do_read(true),
            JobName => {
                // \jobname in text position: error; normally used via \the
                self.error("\\jobname used as a command");
            }
            Show => self.do_show(),
            ShowBox => {
                let idx = self.scan_reg_num();
                let msg = match &self.eqtb.boxed[idx as usize] {
                    Some(b) => self.box_to_string(b),
                    None => "\\box is void".to_string(),
                };
                self.term.push_str(&msg);
                self.term.push('\n');
            }
            ShowThe => {
                let _ = self.get_token();
                self.term.push_str(&self.pending_the_string.take().unwrap_or_default());
            }
            ShowLists => {
                self.term.push_str(&format!("mode: {:?}\n", self.mode));
            }
            ShowGroups | ShowTokens | ShowIfs => {}
            Char => {
                let c = self.scan_int();
                self.char_token(c as u8, false);
            }
            // math
            MathChar => {
                let v = self.scan_int();
                if self.mode.is_m() {
                    self.append_mathchar(v as u16);
                } else {
                    self.error("You can't use `\\mathchar' here");
                }
            }
            MathAccent => {
                let v = self.scan_int();
                if self.mode.is_m() {
                    self.do_math_accent(v as u16);
                } else {
                    self.error("You can't use `\\mathaccent' here");
                }
            }
            Radical => {
                let v = self.scan_int();
                if self.mode.is_m() {
                    self.do_radical(v);
                } else {
                    self.error("You can't use `\\radical' here");
                }
            }
            Overline => {
                if self.mode.is_m() {
                    self.do_overline(false);
                } else {
                    self.error("You can't use `\\overline' here");
                }
            }
            Underline => {
                if self.mode.is_m() {
                    self.do_overline(true);
                } else {
                    self.error("You can't use `\\underline' here");
                }
            }
            Delimiter => {
                let v = self.scan_int();
                if self.mode.is_m() {
                    self.last_delim = Some(v);
                }
            }
            Above | Over | Atop | OverWithDelims | AtopWithDelims | AboveWithDelims => {
                if self.mode.is_m() {
                    self.do_fraction(p);
                } else {
                    self.error("You can't use a fraction here");
                }
            }
            TextFont | ScriptFont | ScriptScriptFont => {
                let style = match p {
                    TextFont => 0u8,
                    ScriptFont => 1,
                    _ => 2,
                };
                // tex.web any_math_fonts: <fam number> <filler> = <filler> <font id>
                let fam = self.scan_int();
                self.scan_optional_equals();
                let f = self.scan_font_id();
                if (0..=255).contains(&fam) {
                    self.eqtb.assign_style_font(style, fam as u16, f, self.global_flag);
                    self.global_flag = false;
                }
            }
            Left => {
                if self.mode.is_m() {
                    let v = self.scan_delim_int();
                    self.push_math_group();
                    self.left_delim = Some(v);
                } else {
                    self.error("Missing $ inserted (\\left)");
                }
            }
            Right => {
                if self.mode.is_m() {
                    let v = self.scan_delim_int();
                    self.right_delim = Some(v);
                    // ends the \left...\right group
                    self.pop_math_group_delimited(v);
                } else {
                    self.error("Missing $ inserted (\\right)");
                }
            }
            Middle => {
                if self.mode.is_m() {
                    let v = self.scan_delim_int();
                    self.append_mlist_node(Node::DelimBox { small: (0, 0), large: ((v >> 20) as u8, (v >> 12) as u8), size: 1 });
                }
            }
            NoLimits | Limits | DisplayLimits => {
                if self.mode.is_m() {
                    self.math_limits = match p {
                        NoLimits => Some(0u8),
                        Limits => Some(1),
                        _ => Some(2),
                    };
                } else {
                    self.error("You can't use \\limits here");
                }
            }
            MathChoice => {
                if self.mode.is_m() {
                    // tex.web scans the four style groups immediately; each
                    // body is scanned as a math list and attached as ChoiceAlt
                    self.begin_mathchoice();
                } else {
                    self.error("You can't use \\mathchoice here");
                }
            }
            DisplayStyle | TextStyle | ScriptStyle | ScriptScriptStyle => {
                if self.mode.is_m() {
                    let style = match p {
                        DisplayStyle => crate::boxes::MathStyle::Display,
                        TextStyle => crate::boxes::MathStyle::Text,
                        ScriptStyle => crate::boxes::MathStyle::Script,
                        _ => crate::boxes::MathStyle::ScriptScript,
                    };
                    self.append_mlist_node(Node::Style(style));
                } else {
                    self.error("Missing $ inserted");
                }
            }
            MathOrd | MathOp | MathBin | MathRel | MathOpen | MathClose | MathPunct | MathInner => {
                if !self.mode.is_m() {
                    self.error("Missing $ inserted");
                } else {
                    let class = match p {
                        MathOrd => crate::math::CL_ORD,
                        MathOp => crate::math::CL_OP,
                        MathBin => crate::math::CL_BIN,
                        MathRel => crate::math::CL_REL,
                        MathOpen => crate::math::CL_OPEN,
                        MathClose => crate::math::CL_CLOSE,
                        MathPunct => crate::math::CL_PUNCT,
                        _ => crate::math::CL_INNER,
                    };
                    self.do_math_class(class);
                }
            }
            Span => {
                if self.align_state > 0 || self.scanner_status == ScannerStatus::Aligning {
                    self.align_span();
                } else {
                    self.error("\\span outside alignment");
                }
            }
            Cr | CrCr => {
                self.align_cr();
            }
            Omit => self.align_omit(),
            NoAlign => self.align_noalign(),
            HAlign => self.begin_halign(),
            VAlign => {
                self.error("\\valign not implemented");
                self.end_occurred = true;
            }
            // pdfTeX primitives
            PdfLiteral => {
                let origin = self.scan_pdf_origin();
                let data = self.scan_pdf_string();
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfLiteral { origin, data }));
            }
            PdfSave => self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfSave)),
            PdfRestore => self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfRestore)),
            PdfSetMatrix => {
                let _ = self.scan_pdf_string();
            }
            PdfStartLink => self.do_pdfstartlink(),
            PdfEndLink => {
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfEndLink));
            }
            PdfDest => self.do_pdfdest(),
            PdfOutline => self.do_pdfoutline(),
            PdfInfo => {
                let data = self.scan_pdf_string();
                self.pdf_doc.info.extend_from_slice(data.as_bytes());
            }
            PdfCatalog => self.do_pdfcatalog(),
            PdfNames => self.do_pdfnames(),
            PdfAnnot => self.do_pdfannot(),
            PdfPageAttr => self.do_pdfpageattr(),
            PdfPagesAttr => self.do_pdfpagesattr(),
            PdfColorStackInit => {
                let _ = self.scan_pdf_string();
            }
            PdfColorStack => {
                let _stack = self.scan_int();
                // action keyword, unbraced (pdfTeX: push|pop|set|current,
                // plus `default`; hyperref writes e.g. `\pdfcolorstack0 pop\relax`)
                let op = if self.scan_keyword(b"push") {
                    "push"
                } else if self.scan_keyword(b"pop") {
                    "pop"
                } else if self.scan_keyword(b"set") {
                    "set"
                } else if self.scan_keyword(b"current") {
                    "current"
                } else if self.scan_keyword(b"default") {
                    "default"
                } else {
                    ""
                };
                match op {
                    "push" | "set" => {
                        let color = self.scan_pdf_string();
                        self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfColorPush(color)));
                    }
                    "pop" => {
                        self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfColorPop));
                    }
                    // `current`/`default` need no material here
                    "current" | "default" => {}
                    _ => {
                        self.error("Missing colorstack operation (push/pop/set/current)");
                    }
                }
            }
            PdfColorStackPrim => {}
            PdfSavePos => {
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::SavePos { obj: 0 }));
            }
            PdfLastXPos | PdfLastYPos => {
                self.error("position primitive needs \\the");
            }
            PdfObj => self.do_pdfobj(),
            PdfXForm => self.do_pdfxform(),
            PdfGlyphToUnicode => {
                // pdftex: \pdfglyphtounicode <glyph name> <unicode value> —
                // TWO arguments. The generic one-arg consumer left the
                // second in the stream, leaking hex like "221500B7".
                for _ in 0..2 {
                    self.skip_spaces_relax();
                    let t = self.get_token();
                    if t.is_char() && t.cc() == 1 {
                        self.scan_balanced_raw(true);
                    } else if !(t.is_char() && t.cc() == 10) {
                        // unbraced single-token arg; spaces between args skip
                    }
                }
            }
            PdfXImage => self.do_pdfximage(),
            // object references take an object number (typically
            // `\pdfrefximage\pdflastximage`), so scan it as an integer
            PdfRefObj | PdfRefXForm | PdfRefXImage => {
                let _ = self.scan_int();
            }
            PdfFontAttr | PdfCompressorLevel
            | PdfUncompress | PdfTolerance | PdfPageBox | PdfThread | PdfStartThread
            | PdfEndThread | PdfResetTimer => {
                // consume the argument syntactically: most take balanced text
                self.skip_spaces_relax();
                let t = self.get_token();
                if t.is_char() && t.cc() == 1 {
                    self.scan_balanced_raw(true);
                } else if !t.is_cs() {
                    // maybe a number: push it back, digit-dropping corrupts
                    // output (e.g. \pdfobjcompresslevel=\z@ style use)
                    self.pushed.push(t);
                } else {
                    self.pushed.push(t);
                }
            }
            // e-TeX expression primitives are expandable; in a main (non-scan)
            // position they produce their digit string into the stream
            NumExpr | DimExpr | GlueExpr | MuExpr => {
                let _ = self.expand_prim(p, id);
            }
            // tex.web: conditionals are executed from the main loop, not
            // expanded by get_token (they must be storeable by \edef etc)
            IfChar | IfCat | IfOdd | IfNum | IfDim | IfVoid | IfHBox | IfVBox
            | IfHMode | IfVMode | IfInner | IfMMode | IfTrue | IfFalse
            | IfEOF | IfDef | IfCSName | IfInCsName | IfX | IfCase | Or | Else | ElIf

            | ElIfX | Fi | Unless => {
                let _ = self.expand_prim(p, id);
            }
            _ => {
                if self.is_expandable(p) {
                    let _ = self.expand_prim(p, id);
                    return;
                }
                let name = ::std::string::String::from_utf8_lossy(self.cs.name(id)).into_owned();
                let cmax = self.cs.intern(b"c_max_int");
                eprintln!("UNIMPL \\{} L{} macros={:?} prim={:?} c_max_int={:?} global={}", name, self.input.current_file_line(), self.last_macros, p, self.eqtb.resolve(cmax).map(|e| e.kind_name()), self.global_flag);
                self.error(&format!("Primitive not implemented: \\{}", name));
            }
        }
    }

    /// tex.web ignore_spaces: get_x_token, discard cat-10, put back the rest.
    fn ignore_spaces(&mut self) {
        loop {
            let t = self.get_token();
            if t == crate::input::EOF_MARKER {
                return;
            }
            if t.is_char() && t.cc() == 10 {
                continue;
            }
            self.pushed.push(t);
            return;
        }
    }

    /// LuaTeX `\\directlua{...}`: consume the group and ignore it.
    fn do_directlua(&mut self) {
        self.skip_spaces_relax();
        let t = self.get_token();
        if t.is_char() && t.cc() == 1 {
            let _ = self.scan_balanced_raw(true);
        } else {
            self.pushed.push(t);
        }
    }

    // ---------- \pdfobj / \pdfxform / \pdfximage: object-number allocation

    /// Reserve the next PDF object number and report it via \pdflastobj.
    /// (Serialization renumbers objects at write time; these reservations
    /// keep `\pdflastobj` self-consistent for `\pdfrefobj`.)
    fn alloc_pdf_obj(&mut self) -> i32 {
        let n = self.pdf_next_obj;
        self.pdf_next_obj += 1;
        n
    }

    /// \pdfobj [attr{..}] [reserveobjnum|useobjnum <n>] [stream [attr{..}]
    /// {data}] {general text}: reserve an object number, consume the body.
    pub fn do_pdfobj(&mut self) {
        let mut reserved: Option<i32> = None;
        let mut reserve_only = false;
        loop {
            if self.scan_keyword(b"attr") {
                let _ = self.scan_pdf_string();
            } else if self.scan_keyword(b"stream") {
                if self.scan_keyword(b"attr") {
                    let _ = self.scan_pdf_string();
                }
                let _ = self.scan_pdf_string();
            } else if self.scan_keyword(b"file") {
                let _ = self.scan_pdf_string();
            } else if self.scan_keyword(b"useobjnum") {
                reserved = Some(self.scan_int());
                reserve_only = true;
            } else if self.scan_keyword(b"reserveobjnum") {
                reserve_only = true;
            } else {
                break;
            }
        }
        if !reserve_only {
            let _ = self.scan_pdf_string();
        }
        self.pdf_last_obj = reserved.unwrap_or_else(|| self.alloc_pdf_obj());
    }

    /// \pdfxform [attr{..}] [resources{..}] <box register number>: freeze a
    /// box register into an XForm XObject; \pdflastxform reports the number.
    pub fn do_pdfxform(&mut self) {
        loop {
            if self.scan_keyword(b"attr") {
                let _ = self.scan_pdf_string();
            } else if self.scan_keyword(b"resources") {
                let _ = self.scan_pdf_string();
            } else {
                break;
            }
        }
        let _ = self.scan_int();
        self.pdf_last_xform = self.alloc_pdf_obj();
    }

    /// \pdfximage [attr{..}] [page <n>] [interpolate|nointerpolate]
    /// [<box spec>] {<file>}: reserve an image XObject number;
    /// \pdflastximage reports it.
    pub fn do_pdfximage(&mut self) {
        loop {
            if self.scan_keyword(b"attr") {
                let _ = self.scan_pdf_string();
            } else if self.scan_keyword(b"page") {
                let _ = self.scan_int();
            } else if self.scan_keyword(b"interpolate") {
            } else if self.scan_keyword(b"nointerpolate") {
            } else {
                break;
            }
        }
        let _ = self.scan_pdf_string();
        self.pdf_last_ximage = self.alloc_pdf_obj();
    }
}

fn id_cs_is(e: &Engine, id: CsId, name: &[u8]) -> bool {
    e.cs.lookup(name) == Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;

    #[test]
    fn test_char_and_chardef() {
        let mut e = Engine::new(false);
        e.init_primitives();
        e.add_nullfont();
        let src = "\\font\\tenrm=cmr10 \\tenrm \\hbox{\\char65 \\char`A \\chardef\\x=66 \\x}\n";
        e.input.push_file("test.tex".to_string(), src.as_bytes().to_vec());
        e.run();
        let b = e.page_list.iter().find(|n| matches!(n, Node::Box { .. })).expect("hbox on page");
        if let Node::Box { list, .. } = b {
            let chars: Vec<u8> = list.iter().filter_map(|n| match n {
                Node::Char { c, .. } => Some(*c),
                _ => None,
            }).collect();
            assert_eq!(chars, vec![65, 65, 66]);
        }
    }
}
