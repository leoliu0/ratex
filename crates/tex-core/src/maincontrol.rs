//! main_dispatch: the big match over primitives executed in main control.

use crate::boxes::Node;
use crate::engine::Engine;
use crate::engine::{Mode, ScannerStatus};
use crate::eqtb::Equiv;
use crate::prim::*;
use crate::token::{CsId, Token};

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
                if self.mode.is_v() {
                    self.pushed.push(Token::from_cs(id));
                    self.start_paragraph(true);
                } else {
                    let g = self.scan_hskip_kind(p);
                    self.append_h_glue(g, false);
                }
            }
            VSkip | VFil | VFill | VFilL | VFilNeg | VSS => {
                match self.mode {
                    // tex.web head_for_vmode: in horizontal mode, back-input
                    // the skip and end the paragraph before scanning its
                    // operand. fire_up may schedule an output routine, so the
                    // token must already be parked beneath it.
                    Mode::Horizontal => {
                        self.pushed.push(Token::from_cs(id));
                        self.par_primitive();
                    }
                    // tex.web insert_dollar_sign: a vertical skip cannot
                    // execute in math mode. Recover as if a closing math shift
                    // had been inserted, then reprocess the untouched skip.
                    Mode::Math | Mode::DisplayMath => {
                        self.pushed.push(Token::from_cs(id));
                        self.error("Missing $ inserted.");
                        self.exit_math();
                    }
                    _ => {
                        let g = self.scan_vskip_kind(p);
                        self.append_v_glue(g);
                    }
                }
            }
            NonScript => {
                if self.mode.is_m() {
                    self.append_mlist_node(Node::NonScript);
                } else {
                    self.error("There's no \\nonscript here");
                }
            }
            MSkip => {
                let g = self.scan_glue(true);
                if self.mode.is_m() {
                    self.append_mlist_node(Node::MuGlue(g));
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
            HRule => self.make_rule(true),
            VRule => {
                if self.mode.is_v() {
                    self.pushed.push(Token::from_cs(id));
                    self.start_paragraph(true);
                } else {
                    self.make_rule(false);
                }
            }
            Leaders | CLeaders | XLeaders => {
                self.begin_leaders(match p {
                    Leaders => 0,
                    CLeaders => 1,
                    _ => 2,
                });
            }
            Discretionary => {
                if self.mode.is_v() {
                    // Start the paragraph before adding replacement text,
                    // and let everypar run before scanning the arguments.
                    self.pushed.push(Token::from_cs(id));
                    self.start_paragraph(true);
                } else {
                    self.do_discretionary();
                }
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
            Box => {
                let idx = self.scan_reg_num();
                let b = self.eqtb.take_box(idx);
                self.append_box_node(b);
            }
            Copy => {
                let idx = self.scan_reg_num();

                let b = self.eqtb.boxed[idx as usize].clone();
                self.append_box_node(b);
            }
            UnHBox => {
                if self.mode.is_v() {
                    // tex.web §21105: vmode+un_hbox back-inputs the token and
                    // starts an indented paragraph FIRST (new_graf(true)); the
                    // re-dispatched hmode unpackage then silently returns on a
                    // void box. \leavevmode = \unhbox\voidb@x depends on both.
                    self.pushed.push(Token::from_cs(id));
                    self.start_paragraph(true);
                } else {
                    self.do_unbox(false, false);
                }
            }
            UnVBox => {
                if self.mode == Mode::Horizontal {
                    // tex.web head_for_vmode: finish even an empty paragraph,
                    // then reprocess the vertical unbox in vertical mode.
                    self.pushed.push(Token::from_cs(id));
                    self.par_primitive();
                } else {
                    self.do_unbox(true, false);
                }
            }
            UnHCopy => {
                if self.mode.is_v() {
                    // same tex.web §21105 case as UnHBox (same cmd code)
                    self.pushed.push(Token::from_cs(id));
                    self.start_paragraph(true);
                } else {
                    self.do_unbox(false, true);
                }
            }
            UnVCopy => {
                if self.mode == Mode::Horizontal {
                    self.pushed.push(Token::from_cs(id));
                    self.par_primitive();
                } else {
                    self.do_unbox(true, true);
                }
            }
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
            VAdjust => self.append_vadjust(),
            MarkPrim | MarksClass => {
                let class = if p == MarksClass {
                    self.scan_reg_num() as i32
                } else {
                    0
                };
                // TeX's scan_toks(false, true): mark text is expanded once
                // when inserted, then the resulting token list is retained.
                let toks = self.scan_general_text_expanded();
                self.append_mark(class, toks);
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
                    let g = self.take_global();
                    self.eqtb.assign_cat(c as u8, v as u8, g);
                }
            }
            MathCode => {
                let c = self.scan_char_num();
                self.scan_optional_equals();
                let v = self.scan_int();
                if (0..=255).contains(&c) {
                    let g = self.take_global();

                    self.eqtb.assign_math_code(c as u8, v as u16, g);
                }
            }
            DelCode => {
                let c = self.scan_char_num();
                self.scan_optional_equals();
                let v = self.scan_int();
                if (0..=255).contains(&c) {
                    let g = self.take_global();
                    self.eqtb.assign_del_code(c as u8, v, g);
                }
            }
            LcCodeP => {
                let c = self.scan_char_num();
                self.scan_optional_equals();
                let v = self.scan_int();
                if (0..=255).contains(&c) && (0..=32767).contains(&v) {
                    let g = self.take_global();
                    self.eqtb.assign_lc_code(c as u8, v as u8, g);
                }
            }
            SfCodeP => {
                let c = self.scan_char_num();
                self.scan_optional_equals();
                let v = self.scan_int();
                if (0..=255).contains(&c) && (0..=32767).contains(&v) {
                    let g = self.take_global();
                    self.eqtb.assign_sf_code(c as u8, v as u16, g);
                }
            }
            UcCodeP => {
                let c = self.scan_char_num();
                self.scan_optional_equals();
                let v = self.scan_int();
                if (0..=255).contains(&c) && (0..=32767).contains(&v) {
                    let g = self.take_global();
                    self.eqtb.assign_uc_code(c as u8, v as u8, g);
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
                if self.mode.is_h() && !self.mode.is_inner() {
                    // Finish the paragraph, then execute this same \end in
                    // vertical mode. Dropping it made ordinary `text\end`
                    // indistinguishable from an illegal raw EOF.
                    self.pushed.push(Token::from_cs(id));
                    self.par_primitive();
                    return;
                }
                if self.mode.is_v() {
                    self.pushed.push(Token::from_cs(id));
                    self.build_page();
                    if self.in_output {
                        return;
                    }
                    self.pushed.pop();
                }
                // tex.web §19775 `its_all_over`: both the current page and
                // contribution list must be empty. Any residual node keeps
                // the job alive. The final retry is forced through the page
                // builder with TeX's null box, \vfill, and awful penalty;
                // calling fire_up directly loses those nodes and changes how
                // LaTeX's output routine recognizes an empty final column.
                if self.mode.is_v() && !self.page_list.is_empty() {
                    self.pushed.push(Token::from_cs(id));
                    let hsize = self.eqtb.dim_params[DimParam::HSize.idx() as usize];
                    self.page_append(Node::Box {
                        kind: crate::boxes::HBOX,
                        w: hsize,
                        h: 0,
                        d: 0,
                        shift: 0,
                        list: Vec::new(),
                        glue_sign: 0,
                        glue_order: 0,
                        glue_set: 0.0,
                        font: None,
                    });
                    self.page_append(Node::Glue(crate::boxes::Glue::fil(
                        crate::boxes::GLUE_FILL,
                        0,
                    )));
                    self.page_append(Node::Penalty(-0x4000_0000));
                    self.build_page();
                    return;
                }
                self.explicit_end_seen = true;
                self.end_occurred = true;
            }
            Immediate => {
                let t = self.get_token();
                if t.is_cs() {
                    let id2 = t.cs_id();
                    if let Some(Equiv::Prim(p2)) = self.eqtb.resolve(id2).cloned() {
                        match p2 {
                            Prim::Write => self.do_write(true),
                            Prim::Special => self.do_special(),
                            Prim::OpenOut => self.do_openout(true),
                            Prim::CloseOut => self.do_closeout(true),
                            _ => self.main_dispatch(p2, id2),
                        }
                    }
                }
            }
            OpenOut => self.do_openout(false),
            CloseOut => self.do_closeout(false),
            Write => self.do_write(false),
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
                self.log.push_str(&msg);
                self.log.push('\n');
            }
            ShowThe => {
                let _ = self.get_token();
                self.term
                    .push_str(&self.pending_the_string.take().unwrap_or_default());
            }
            ShowLists => {
                let mut s = format!(
                    "mode: {:?}\npage_list: {} nodes\ncur_list: {} nodes\n",
                    self.mode,
                    self.page_list.len(),
                    self.cur_list.len()
                );
                for (i, n) in self.page_list.iter().enumerate() {
                    match n {
                        Node::Box { kind, w, h, d, .. } => s.push_str(&format!(
                            "  {}: BOX k={} w={:.1} h={:.1} d={:.1}\n",
                            i,
                            kind,
                            *w as f64 / 65536.0,
                            *h as f64 / 65536.0,
                            *d as f64 / 65536.0
                        )),
                        Node::Glue(g) => s.push_str(&format!(
                            "  {}: GLUE w={:.2} st={:.2} sh={:.2}\n",
                            i,
                            g.width as f64 / 65536.0,
                            g.stretch as f64 / 65536.0,
                            g.shrink as f64 / 65536.0
                        )),
                        Node::Penalty(p) => s.push_str(&format!("  {}: PENALTY p={}\n", i, p)),
                        Node::Kern(k) | Node::ExplicitKern(k) => {
                            s.push_str(&format!("  {}: KERN k={:.2}\n", i, *k as f64 / 65536.0))
                        }
                        other => s.push_str(&format!(
                            "  {}: OTHER {:?}\n",
                            i,
                            std::mem::discriminant(other)
                        )),
                    }
                }
                self.term.push_str(&s);
            }
            ShowGroups | ShowTokens | ShowIfs => {}
            Char => {
                let c = self.scan_int();
                self.char_token(c as u8, false);
            }
            Accent => {
                match self.mode {
                    Mode::Vertical | Mode::InternalVertical => {
                        // tex.web §21103: vmode+accent → back_input and start
                        // a paragraph; the accent is re-processed in hmode.
                        self.pushed.push(Token::from_cs(id));
                        self.start_paragraph(true);
                    }
                    Mode::Horizontal | Mode::RestrictedHorizontal => self.do_accent(),
                    // mmode+accent is unmatched in tex.web's main_control
                    // switch: silently ignored.
                    _ => {}
                }
            }
            ExSpace => {
                match self.mode {
                    Mode::Vertical | Mode::InternalVertical => {
                        // tex.web vmode+ex_space: back_input, new_graf(true)
                        self.pushed.push(Token::from_cs(id));
                        self.start_paragraph(true);
                    }
                    _ => self.ex_space(),
                }
            }
            ItalicCorrection => match self.mode {
                Mode::Vertical | Mode::InternalVertical => {
                    self.error("You can't use `\\/' in vertical mode");
                }
                Mode::Horizontal | Mode::RestrictedHorizontal => {
                    let correction = match self.cur_list.last() {
                        Some(Node::Char { c, font } | Node::Ligature { c, font, .. }) => self
                            .eqtb
                            .fonts
                            .get(*font as usize)
                            .map_or(0, |font| font.char_italic(*c)),
                        _ => 0,
                    };
                    self.cur_list.push(Node::ExplicitKern(correction));
                }
                Mode::Math | Mode::DisplayMath => self.append_mlist_node(Node::MathKern(0, 0)),
            },
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
            EqNo | LeqNo => {
                // tex.web §21734: mmode+eq_no is legal only in display math
                if self.mode == Mode::DisplayMath {
                    self.start_eq_no(matches!(p, Prim::LeqNo));
                } else {
                    self.error("You can't use \\eqno here");
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
                    // tex.web: a standalone \delimiter is a delimiter atom —
                    // typeset via var_delimiter at mlist conversion
                    // (\lbrace/\rbrace in newtx land here; parking the code
                    // in last_delim silently dropped the glyph)
                    if v < 0 || v >= 0x8000000 {
                        self.error("Invalid delimiter code");
                    } else {
                        let (sf, sc, lf, lc) = crate::math::delim_code_parts_pub(v);
                        self.append_mlist_node(Node::DelimBox {
                            small: (sf, sc),
                            large: (lf, lc),
                            size: 2,
                        });
                    }
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
                    let g = self.take_global();
                    self.eqtb.assign_style_font(style, fam as u16, f, g);
                }
            }
            Left => {
                if self.mode.is_m() {
                    let v = self.scan_delim_int();
                    self.push_math_group(v);
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
                    self.append_mlist_node(Node::DelimBox {
                        small: (0, 0),
                        large: ((v >> 20) as u8, (v >> 12) as u8),
                        size: 1,
                    });
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
                    if let Some(s) = self.math_style_stack.last_mut() {
                        *s = style;
                    }
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
                self.fatal_error("\\valign not implemented");
            }
            // pdfTeX primitives
            PdfLiteral => {
                let origin = self.scan_pdf_origin();
                let data = self.scan_pdf_string();
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfLiteral {
                    origin,
                    data,
                }));
            }
            PdfSave => self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfSave)),
            PdfRestore => self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfRestore)),
            PdfSetMatrix => {
                let matrix = self.scan_pdf_string();
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfSetMatrix(matrix)));
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
            PdfPageResources => {
                self.scan_optional_equals();
                let toks = self.scan_token_list();
                self.pdf_page_resources = self.write_tokens_to_string(&toks).into_bytes();
                self.pdf_page_resources_toks = toks;
            }
            PdfColorStackInit => {
                let _ = self.scan_keyword(b"page");
                let _ = self.scan_keyword(b"direct");
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
                    "push" => {
                        let color = self.scan_pdf_string();
                        self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfColorPush(
                            color,
                        )));
                    }
                    "set" => {
                        let color = self.scan_pdf_string();
                        self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfColorSet(
                            color,
                        )));
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
            PdfMapFile => self.do_pdfmapfile(),
            PdfMapLine => self.do_pdfmapline(),
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
            PdfRefObj => {
                let _ = self.scan_int();
            }
            PdfRefXForm => {
                let id = self.scan_int();
                let (w, h, d) = self.pdf_xforms.get(&id).copied().unwrap_or((0, 0, 0));
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfRefXForm {
                    obj: id,
                    w,
                    h,
                    d,
                }));
            }
            PdfRefXImage => {
                let id = self.scan_int();
                let (w, h, d) = self
                    .pdf_images
                    .get(&id)
                    .map(|info| (info.width, info.height, info.depth))
                    .unwrap_or((0, 0, 0));
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfRefXImage {
                    obj: id,
                    w,
                    h,
                    d,
                }));
            }
            PdfFontAttr => {
                // pdfTeX syntax is \pdffontattr <font identifier> {<attribute>}.
                // Consuming only the font identifier leaks the dictionary text
                // into the document as ordinary characters.
                let _ = self.scan_font_id();
                self.skip_spaces_relax();
                let t = self.get_token();
                if t.is_char() && t.cc() == 1 {
                    self.scan_balanced_raw(true);
                } else {
                    self.pushed.push(t);
                    self.error("Missing { inserted for \\pdffontattr");
                }
            }
            PdfFontExpand => {
                self.do_pdffontexpand();
            }
            PdfNoLigatures => {
                let f = self.scan_font_id();
                if f != 0 {
                    self.set_no_ligatures(f);
                }
            }
            LetterspaceFont => self.do_letterspacefont(),
            PdfUncompress | PdfTolerance | PdfPageBox | PdfThread | PdfStartThread
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
            // XeTeX identity probes: \XeTeXversion is an \the-like integer
            // quantity; \XeTeXrevision expands to its decimal revision
            // string. hyperref/iftex probe these to select driver code.
            XeTeXVersion => {
                for b in b"2" {
                    self.pushed.push(crate::token::Token::char(12, *b as u32));
                }
            }
            XeTeXRevision => {
                for b in b".9995" {
                    self.pushed.push(crate::token::Token::char(12, *b as u32));
                }
            }
            // tex.web: conditionals are executed from the main loop, not
            // expanded by get_token (they must be storeable by \edef etc)
            IfChar | IfCat | IfOdd | IfNum | IfDim | IfVoid | IfHBox | IfVBox | IfHMode
            | IfVMode | IfInner | IfMMode | IfTrue | IfFalse | IfEOF | IfDef | IfCSName
            | IfInCsName | IfX | IfCase | Or | Else | ElIf | ElIfX | Fi | Unless => {
                let _ = self.expand_prim(p, id);
            }
            _ => {
                if self.is_expandable(p) {
                    let _ = self.expand_prim(p, id);
                    return;
                }
                let name = ::std::string::String::from_utf8_lossy(self.cs.name(id)).into_owned();
                self.error(&format!("Primitive not implemented: \\{}", name));
            }
        }
    }

    /// tex.web §1267-1275 make_accent: `\accent <number 0-255> <filler>
    /// <char>`. Typesets the next character with an accent character taken
    /// from slot <number> of the current font, stacked above the base char
    /// and centered with two kerns, raised so the accent sits at the font's
    /// x-height. The emitted list is
    ///   kern(delta) [accent char] kern(-a-delta) base_char
    /// so the sequence is exactly as wide as the base character.
    fn do_accent(&mut self) {
        let n = self.scan_char_num();
        if !(0..=255).contains(&n) {
            self.error(&format!(
                "Invalid code ({}), should be in the range 0..255",
                n
            ));
            return;
        }
        let acc = n as u8;
        let f_acc = self.eqtb.cur_font_val;
        let Some(af) = self.eqtb.fonts.get(f_acc as usize) else {
            return; // nullfont: nothing happens (tex.web new_character fails)
        };
        if !af.exists_char(acc) {
            // char_warning: no accent glyph — drop the accent; the base
            // character stays in the stream and typesets normally.
            self.term.push_str(&format!(
                "Missing character: There is no {} in font {}!\n",
                n, f_acc
            ));
            return;
        }
        let a = af.char_width(acc);
        let x = af.x_height();
        let s = f64::from(af.param(1)) / 65536.0; // accent font slant
                                                  // do_assignments: get_x_token skips blank spaces/\relax AND expands,
                                                  // so `\accent 127 \i` typesets the dotless ı that \i expands to
                                                  // (tex.web §1269 make_accent's `do_assignments;`).
        self.skip_spaces_relax();
        let t = self.get_x_raw();
        let base: Option<u8> = if t.is_char() && (t.cc() == 11 || t.cc() == 12) {
            Some(t.chr() as u8)
        } else if t.is_cs() && matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(Prim::Char)))
        {
            let v = self.scan_char_num();
            (0..=255).contains(&v).then_some(v as u8)
        } else {
            self.pushed.push(t);
            None
        };
        let Some(bc) = base else {
            // no usable base character: append the accent alone
            self.cur_list.push(Node::Char {
                c: acc,
                font: f_acc,
            });
            self.space_factor = 1000;
            return;
        };
        let f_base = self.eqtb.cur_font_val;
        let exists = self
            .eqtb
            .fonts
            .get(f_base as usize)
            .map(|f| f.exists_char(bc))
            .unwrap_or(false);
        if !exists {
            self.term.push_str(&format!(
                "Missing character: There is no {} in font {}!\n",
                bc, f_base
            ));
            self.cur_list.push(Node::Char {
                c: acc,
                font: f_acc,
            });
            self.space_factor = 1000;
            return;
        }
        let (w, h, _) = self.char_dims(f_base, bc);
        let t_sl = self
            .eqtb
            .fonts
            .get(f_base as usize)
            .map(|f| f64::from(f.param(1)) / 65536.0)
            .unwrap_or(0.0);
        // If the base height differs from the x-height, the accent char is
        // packed into a box shifted by x-h (tex.web §1274).
        let accent_part: Node = if h != x {
            let mut b = crate::boxes::hpack(
                vec![Node::Char {
                    c: acc,
                    font: f_acc,
                }],
                None,
                crate::boxes::HBOX,
                &self.eqtb,
            )
            .node;
            if let Node::Box { shift, .. } = &mut b {
                *shift = x - h;
            }
            b
        } else {
            Node::Char {
                c: acc,
                font: f_acc,
            }
        };
        let delta = ((w - a) as f64 / 2.0 + h as f64 * t_sl - x as f64 * s).round() as i32;
        self.cur_list.push(Node::ExplicitKern(delta));
        self.cur_list.push(accent_part);
        self.cur_list.push(Node::ExplicitKern(-a - delta));
        self.cur_list.push(Node::Char {
            c: bc,
            font: f_base,
        });
        self.space_factor = 1000;
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

    /// Parse one PDF object body, optionally as a stream or binary file.
    pub fn do_pdfobj(&mut self) {
        let mut reserved = None;
        let mut stream = false;
        let mut file = false;
        let mut attr = String::new();
        loop {
            if self.scan_keyword(b"reserveobjnum") {
                self.pdf_last_obj = self.alloc_pdf_obj();
                return;
            } else if self.scan_keyword(b"useobjnum") {
                reserved = Some(self.scan_int());
            } else if self.scan_keyword(b"stream") {
                stream = true;
            } else if self.scan_keyword(b"attr") {
                attr = self.scan_pdf_string();
            } else if self.scan_keyword(b"file") {
                file = true;
            } else {
                break;
            }
        }
        let obj = reserved.unwrap_or_else(|| self.alloc_pdf_obj());
        self.pdf_last_obj = obj;
        let toks = self.scan_general_text_expanded();
        let mut body = self.tokens_to_bytes(&toks);
        if file {
            let name = String::from_utf8_lossy(&body);
            let path = self
                .resolve_input_path(name.trim())
                .or_else(|| self.font_loader.kpse.find_any(name.trim()));
            match path.and_then(|path| std::fs::read(path).ok()) {
                Some(bytes) => body = bytes,
                None => {
                    self.fatal_error(&format!("Cannot open PDF object file `{name}'"));
                    return;
                }
            }
        }
        if stream {
            let mut object =
                format!("<< {} /Length {} >>\nstream\n", attr.trim(), body.len()).into_bytes();
            object.extend_from_slice(&body);
            object.extend_from_slice(b"\nendstream");
            body = object;
        }
        self.pdf_doc.objects.push((obj, body));
    }

    /// \pdfxform [attr{..}] [resources{..}] <box register number>: freeze a
    /// box register into an XForm XObject; \pdflastxform reports the number.
    pub fn do_pdfxform(&mut self) {
        let mut attr = String::new();
        let mut resources = String::new();
        loop {
            if self.scan_keyword(b"attr") {
                attr = self.scan_pdf_string();
            } else if self.scan_keyword(b"resources") {
                resources = self.scan_pdf_string();
            } else {
                break;
            }
        }
        let box_reg = self.scan_int();
        let obj = self.alloc_pdf_obj();
        self.pdf_last_xform = obj;
        let b = self.eqtb.boxed.get(box_reg as usize).cloned().flatten();
        let (w, h, d) = match &b {
            Some(Node::Box { w, h, d, .. }) => (*w, *h, *d),
            _ => (0, 0, 0),
        };
        self.pdf_xforms.insert(obj, (w, h, d));
        let w_bp = crate::pdfrender::sp_to_bp(w as i64);
        let h_bp = crate::pdfrender::sp_to_bp(h as i64);
        let d_bp = crate::pdfrender::sp_to_bp(d as i64);
        let attr_str = if attr.trim().is_empty() {
            String::new()
        } else {
            format!(" {}", attr.trim())
        };
        let (content, fonts) = match &b {
            Some(node) => self.render_form_box(node, w, h, d),
            None => (Vec::new(), Vec::new()),
        };
        let font_object = self.alloc_pdf_obj();
        self.pdf_doc.objects.push((font_object, b"<< >>".to_vec()));
        self.pdf_doc.form_fonts.push((font_object, fonts));
        let mut xobj_entries = Vec::new();
        for (obj_num, bytes) in &self.pdf_doc.objects {
            if bytes.starts_with(b"<< /Type /XObject /Subtype /Form") {
                xobj_entries.push(format!("/Fm{} {} 0 R", obj_num, obj_num));
            }
        }
        for obj_num in self.pdf_images.keys() {
            xobj_entries.push(format!("/Im{} {} 0 R", obj_num, obj_num));
        }
        let xobj_res = if xobj_entries.is_empty() || resources.contains("/XObject") {
            String::new()
        } else {
            format!(" /XObject << {} >>", xobj_entries.join(" "))
        };
        let res_str = format!(
            " /Resources << /Font {font_object} 0 R {} /ProcSet [/PDF /Text]{} >>",
            resources.trim(),
            xobj_res
        );
        let (data, filter) = if !content.is_empty() {
            (crate::pdffile::flate(&content), " /Filter /FlateDecode")
        } else {
            (Vec::new(), "")
        };
        let mut body = format!(
            "<< /Type /XObject /Subtype /Form /FormType 1 /BBox [0 {:.4} {:.4} {:.4}] /Matrix [1 0 0 1 0 0]{}{} /Length {}{} >>\nstream\n",
-d_bp, w_bp, h_bp, attr_str, res_str, data.len(), filter
        )
        .into_bytes();
        body.extend_from_slice(&data);
        body.extend_from_slice(b"\nendstream");
        self.pdf_doc.objects.push((obj, body));
    }

    /// \pdfximage [attr{..}] [page <n>] [interpolate|nointerpolate]
    /// [<box spec>] {<file>}: reserve an image XObject number;
    /// \pdflastximage reports it.
    pub fn do_pdfximage(&mut self) {
        let mut scan_w: Option<i32> = None;
        let mut scan_h: Option<i32> = None;
        let mut scan_d: Option<i32> = None;
        let mut page = 1;
        let mut page_box: &[u8] = b"CropBox";
        loop {
            if self.scan_keyword(b"width") {
                scan_w = Some(self.scan_dimen(false, false));
            } else if self.scan_keyword(b"height") {
                scan_h = Some(self.scan_dimen(false, false));
            } else if self.scan_keyword(b"depth") {
                scan_d = Some(self.scan_dimen(false, false));
            } else if self.scan_keyword(b"attr") {
                let _ = self.scan_pdf_string();
            } else if self.scan_keyword(b"page") {
                page = self.scan_int().max(1) as u32;
            } else if self.scan_keyword(b"interpolate") {
            } else if self.scan_keyword(b"nointerpolate") {
            } else if self.scan_keyword(b"cropbox") {
                page_box = b"CropBox";
            } else if self.scan_keyword(b"mediabox") {
                page_box = b"MediaBox";
            } else if self.scan_keyword(b"bleedbox") {
                page_box = b"BleedBox";
            } else if self.scan_keyword(b"trimbox") {
                page_box = b"TrimBox";
            } else if self.scan_keyword(b"artbox") {
                page_box = b"ArtBox";
            } else {
                break;
            }
        }
        let file = self.scan_pdf_string();
        let path = self.resolve_input_path(&file);
        let bytes = match path.as_ref().and_then(|p| std::fs::read(p).ok()) {
            Some(bytes) => bytes,
            None => {
                self.error(&format!("Cannot read image {file}"));
                return;
            }
        };
        if let Some(path) = &path {
            self.loaded_files.push(path.clone());
        }
        let obj = self.alloc_pdf_obj();
        let (nat_w, nat_h) = if bytes.starts_with(b"%PDF-") {
            match crate::pdf_images::import_pdf_page(
                &bytes,
                page,
                page_box,
                obj,
                &mut self.pdf_next_obj,
            ) {
                Ok((w, h, objects)) => {
                    self.pdf_doc.objects.extend(
                        objects
                            .into_iter()
                            .map(|image| (image.obj_num, image.bytes)),
                    );
                    let sp_per_bp = 72.27 / 72.0 * 65536.0;
                    (
                        (w * sp_per_bp).round() as i32,
                        (h * sp_per_bp).round() as i32,
                    )
                }
                Err(error) => {
                    self.error(&format!("Cannot include PDF {file}: {error}"));
                    return;
                }
            }
        } else if let Some(jpeg) = crate::pdf_images::jpeg_info(&bytes) {
            (
                (jpeg.width as f64 / jpeg.dpi_x * 72.27 * 65536.0).round() as i32,
                (jpeg.height as f64 / jpeg.dpi_y * 72.27 * 65536.0).round() as i32,
            )
        } else {
            let b = &bytes;
            match read_png_dims(b) {
                Some((w, h, rx, ry)) => {
                    let w_sp = (w as f64 / rx as f64 * 72.27 * 65536.0).round() as i32;
                    let h_sp = (h as f64 / ry as f64 * 72.27 * 65536.0).round() as i32;
                    (w_sp, h_sp)
                }
                None => {
                    self.error(&format!(
                        "Unsupported or invalid image `{file}` (expected PDF, JPEG, or PNG)"
                    ));
                    return;
                }
            }
        };
        let d = scan_d.unwrap_or(0);
        let (w, h) = match (scan_w, scan_h) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) if nat_w != 0 => {
                (w, ((nat_h as i64 * w as i64) / nat_w as i64) as i32 - d)
            }
            (None, Some(h)) if nat_h != 0 => (
                ((nat_w as i64 * (h as i64 + d as i64)) / nat_h as i64) as i32,
                h,
            ),
            (w, h) => (w.unwrap_or(nat_w), h.unwrap_or(nat_h - d)),
        };
        self.pdf_last_ximage = obj;
        self.pdf_images.insert(
            obj,
            crate::engine::PdfImageInfo {
                path: path
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or(file),
                used: false,
                embedded: bytes.starts_with(b"%PDF-"),
                width: w,
                height: h,
                depth: d,
            },
        );
    }
}

fn read_png_dims(bytes: &[u8]) -> Option<(u32, u32, u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    let mut rx = 72u32;
    let mut ry = 72u32;
    let mut i = 8usize;
    while i + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[i..i + 4].try_into().ok()?) as usize;
        let chunk_type = &bytes[i + 4..i + 8];
        if chunk_type == b"pHYs" && i + 8 + len <= bytes.len() && len >= 9 {
            let ppu_x = u32::from_be_bytes(bytes[i + 8..i + 12].try_into().ok()?);
            let ppu_y = u32::from_be_bytes(bytes[i + 12..i + 16].try_into().ok()?);
            let unit = bytes[i + 16];
            if unit == 1 && ppu_x > 0 && ppu_y > 0 {
                rx = ((ppu_x as f64 * 0.0254).round() as u32).max(1);
                ry = ((ppu_y as f64 * 0.0254).round() as u32).max(1);
            }
        }
        i += 12 + len;
    }
    Some((w, h, rx, ry))
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
        e.input
            .push_file("test.tex".to_string(), src.as_bytes().to_vec());
        e.run();
        let b = e
            .page_list
            .iter()
            .find(|n| matches!(n, Node::Box { .. }))
            .expect("hbox on page");
        if let Node::Box { list, .. } = b {
            let chars: Vec<u8> = list
                .iter()
                .filter_map(|n| match n {
                    Node::Char { c, .. } => Some(*c),
                    _ => None,
                })
                .collect();
            assert_eq!(chars, vec![65, 65, 66]);
        }
    }
}
