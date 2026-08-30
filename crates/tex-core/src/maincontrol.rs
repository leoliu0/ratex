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
            Relax => {}
            BeginGroup => self.begin_group(false),
            EndGroup => self.end_group(),
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
            UnKern => self.un_kern(),
            UnPenalty => self.un_penalty(),
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
                self.input.push_toks(toks, "<case>");
            }
            Input => self.do_input(),
            EndInput => self.do_endinput(),
            ScanTokens => {
                let _ = self.expand_prim(ScanTokens, id);
            }
            Dump => {
                self.format_done = true;
                self.end_occurred = true;
            }
            End => {
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
                if (0..=16).contains(&fam) {
                    self.eqtb.assign_style_font(style, fam as u8, f, self.global_flag);
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
            PdfStartLink => {
                let attr = self.scan_link_attr();
                let (uri, name) = self.scan_link_dest();
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfStartLink { attr, uri, name }));
            }
            PdfEndLink => {
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfEndLink));
            }
            PdfDest => self.do_pdfdest(),
            PdfOutline => self.do_pdfoutline(),
            PdfInfo => {
                let data = self.scan_pdf_string();
                self.pdf_doc.info.extend_from_slice(data.as_bytes());
            }
            PdfCatalog => {
                let data = self.scan_pdf_string();
                self.pdf_doc.catalog_extra.extend_from_slice(data.as_bytes());
            }
            PdfAnnot => {
                let _ = self.scan_general_text();
            }
            PdfColorStackInit => {
                let _ = self.scan_pdf_string();
            }
            PdfColorStack => {
                let _stack = self.scan_int();
                let op = self.scan_pdf_string();
                if op == "push" || op == "set" {
                    let color = self.scan_pdf_string();
                    self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfColorPush(color)));
                } else if op == "pop" {
                    self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfColorPop));
                }
            }
            PdfColorStackPrim => {}
            PdfSavePos => {
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::SavePos { obj: 0 }));
            }
            PdfLastXPos | PdfLastYPos => {
                self.error("position primitive needs \\the");
            }
            PdfMapFile => self.do_pdfmapfile(),
            PdfMapLine => self.do_pdfmapline(),
            PdfGlyphToUnicode | PdfFontAttr | PdfPagesAttr | PdfCompressorLevel | PdfObj
            | PdfRefObj | PdfUncompress | PdfTolerance | PdfXForm | PdfXImage
            | PdfRefXForm | PdfRefXImage | PdfPageBox | PdfThread | PdfStartThread
            | PdfEndThread | PdfLinkMargin | PdfDestMargin | PdfThreadMargin | PdfResetTimer => {
                // consume the argument syntactically: most take balanced text
                self.skip_spaces_relax();
                let t = self.get_token();
                if t.is_char() && t.cc() == 1 {
                    self.scan_balanced_raw();
                } else if !t.is_cs() {
                    // maybe a number
                } else {
                    self.pushed.push(t);
                }
            }
            PdfHOrigin | PdfVOrigin => {
                let v = self.scan_dimen(false, false);
                self.pdf_horigin = v;
                let _ = v;
            }
            PdfPageWidth => {
                let v = self.scan_dimen(false, false);
                self.pdf_page_width = Some(v);
            }
            PdfPageHeight => {
                let v = self.scan_dimen(false, false);
                self.pdf_page_height = Some(v);
            }
            _ => {
                if self.is_expandable(p) {
                    let _ = self.expand_prim(p, id);
                    return;
                }
                if p == EndCsName {
                    self.error("Extra \\endcsname");
                    return;
                }
                let name = ::std::string::String::from_utf8_lossy(self.cs.name(id)).into_owned();
                let cmax = self.cs.intern(b"c_max_int");
                eprintln!("UNIMPL \\{} L{} macros={:?} prim={:?} c_max_int={:?} global={}", name, self.input.current_file_line(), self.last_macros, p, self.eqtb.resolve(cmax).map(|e| e.kind_name()), self.global_flag);
                self.error(&format!("Primitive not implemented: \\{}", name));
            }
        }
    }
}

fn id_cs_is(e: &Engine, id: CsId, name: &[u8]) -> bool {
    e.cs.lookup(name) == Some(id)
}
