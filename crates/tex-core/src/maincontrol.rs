//! main_dispatch: the big match over primitives executed in main control.

use crate::boxes::Node;
use crate::engine::Engine;
use crate::engine::{Mode, ScannerStatus};
use crate::eqtb::Equiv;
use crate::prim::*;
use crate::token::{CsId, Token};
use std::fmt::Write;

const MAX_INSPECTION_BYTES: usize = 8 * 1024;
const MAX_INSPECTION_FRAMES: usize = 32;
const INSPECTION_TRUNCATED: &str = "\n… inspection output truncated";

pub(crate) struct InspectionText {
    pub(crate) text: String,
    pub(crate) truncated: bool,
}

impl InspectionText {
    pub(crate) fn new() -> Self {
        Self {
            text: String::with_capacity(512),
            truncated: false,
        }
    }

    pub(crate) fn push(&mut self, args: std::fmt::Arguments<'_>) {
        if self.truncated {
            return;
        }
        let content_limit = MAX_INSPECTION_BYTES - INSPECTION_TRUNCATED.len();
        let _ = self.text.write_fmt(args);
        if self.text.len() > content_limit {
            let mut end = content_limit;
            while end > 0 && !self.text.is_char_boundary(end) {
                end -= 1;
            }
            self.text.truncate(end);
            self.truncated = true;
        }
    }

    pub(crate) fn finish(mut self) -> String {
        if self.truncated {
            self.text.push_str(INSPECTION_TRUNCATED);
        }
        self.text
    }
}

impl Engine {
    pub fn main_dispatch(&mut self, p: Prim, id: CsId) {
        use Prim::*;
        if !self.mode.is_m() && Self::is_math_only(p) {
            self.insert_dollar_sign(Token::from_cs(id));
            return;
        }
        match p {
            Relax | EndCsName => {}
            BeginGroup => self.begin_semi_simple(),
            EndGroup => self.end_semi_simple(),
            BGroup => self.begin_group(false),
            EGroup => self.end_group(),
            NoBoundary => self.no_boundary(),

            Par => self.par_primitive(Token::from_cs(id)),
            Indent => self.start_paragraph(true),
            NoIndent => self.start_paragraph(false),
            // pdftex.web start_par chr 2: \indent in vertical mode, nothing
            // in horizontal and math mode
            QuitVMode => {
                if self.mode.is_v() {
                    self.start_paragraph(true);
                }
            }
            SetLanguage => self.set_language(id),
            PdfPrimitiveExec => {
                let t = self.pdf_primitive_target();
                self.push_token(t);
            }
            PdfRetval | ParShapeLength | ParShapeIndent | ParShapeDimen | GlueToMu
            | MuToGlue => self.report_illegal_case(id),
            HSkip | HFil | HFill | HFilL | HFilNeg | HSS => {
                if self.mode.is_v() {
                    self.push_token(Token::from_cs(id));
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
                        self.push_token(Token::from_cs(id));
                        self.push_token(Token::from_cs(self.ids.par));
                    }
                    // tex.web insert_dollar_sign: a vertical skip cannot
                    // execute in math mode. Recover as if a closing math shift
                    // had been inserted, then reprocess the untouched skip.
                    Mode::Math | Mode::DisplayMath => {
                        self.insert_dollar_sign(Token::from_cs(id));
                    }
                    // tex.web head_for_vmode: restricted horizontal mode
                    // cannot end a paragraph, so the group is closed
                    Mode::RestrictedHorizontal => self.off_save(Token::from_cs(id)),
                    _ => {
                        let g = self.scan_vskip_kind(p);
                        self.append_v_glue(g);
                    }
                }
            }
            NonScript => {
                if self.mode.is_m() {
                    self.append_mlist_node(Node::NonScript);
                }
            }
            MSkip => {
                let g = self.scan_glue(true);
                if self.mode.is_m() {
                    self.append_mlist_node(Node::MuGlue(g, self.eqtb.cur_attr));
                }
            }
            Kern => {
                // tex.web §1061 append_kern: subtype explicit
                let d = self.scan_dimen(false, false);
                if self.mode.is_m() {
                    self.append_mlist_node(Node::ExplicitKern(d, self.eqtb.cur_attr));
                } else if self.mode.is_v() {
                    self.append_v_kern(d);
                } else {
                    self.append_h_kern(d);
                }
            }
            MKern => {
                let d = self.scan_dimen(true, false);
                if self.mode.is_m() {
                    self.append_mlist_node(Node::MathKern(d, 0, self.eqtb.cur_attr));
                }
            }
            // etex.ch `hmode+valign` with cur_chr>0; vmode+valign starts a
            // paragraph (back_input; new_graf), mmode+valign is
            // insert_dollar_sign
            BeginL | EndL | BeginR | EndR => match self.mode {
                Mode::Vertical | Mode::InternalVertical => {
                    self.push_token(Token::from_cs(id));
                    self.start_paragraph(true);
                }
                Mode::Math | Mode::DisplayMath => {
                    self.insert_dollar_sign(Token::from_cs(id));
                }
                Mode::Horizontal | Mode::RestrictedHorizontal => {
                    if self.eqtb.int_params[IntParam::TeXXeTEnabled.idx() as usize] > 0 {
                        self.flush_native_text();
                        let kind = match p {
                            BeginL => crate::boxes::BEGIN_L,
                            EndL => crate::boxes::END_L,
                            BeginR => crate::boxes::BEGIN_R,
                            _ => crate::boxes::END_R,
                        };
                        self.cur_list.push(Node::MathKern(0, kind, self.eqtb.cur_attr));
                        self.texxet_nodes = true;
                    } else {
                        // etex.ch eTeX_enabled: "Sorry, this optional e-TeX
                        // feature has been disabled."
                        let name = self.prim_name(p);
                        self.error(&format!("Improper \\{name}"));
                    }
                }
            },
            HMove | HMoveLeft => {
                let d = self.scan_dimen(false, false);
                self.box_move(d, p == HMoveLeft, true);
            }
            VMove | VRaise => {
                let d = self.scan_dimen(false, false);
                self.box_move(d, p == VRaise, false);
            }
            HBox | VBox | VTop | VCenter | HPack | VPack | TPack => {
                let kind = match p.box_spec() {
                    HBox => 0u8,
                    VBox => 1,
                    VTop => 2,
                    _ => 3,
                };
                self.begin_box(kind);
            }
            PdfExtension => self.do_pdf_extension(),
            DviExtension => self.do_dvi_extension(),
            Deferred => self.do_deferred(),
            Boundary | WordBoundary | ProtrusionBoundary => self.append_boundary(p),
            ToksApp | ToksPre | EToksApp | EToksPre | GToksApp | GToksPre | XToksApp | XToksPre => {
                self.combine_the_toks(p)
            }
            HRule => {
                if self.mode == Mode::Horizontal {
                    self.push_token(Token::from_cs(id));
                    self.push_token(Token::from_cs(self.ids.par));
                    return;
                }
                if self.mode == Mode::RestrictedHorizontal {
                    // tex.web head_for_vmode: only leaders may hold a rule
                    // in restricted horizontal mode
                    self.error("You can't use `\\hrule' here except with leaders");
                    return;
                }
                self.make_rule(true);
            }
            VRule => {
                if self.mode.is_v() {
                    self.push_token(Token::from_cs(id));
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
            Discretionary | HyphenDisc => {
                if self.mode.is_v() {
                    // Start the paragraph before adding replacement text,
                    // and let everypar run before scanning the arguments.
                    self.push_token(Token::from_cs(id));
                    self.start_paragraph(true);
                } else if p == HyphenDisc {
                    self.append_hyphen_discretionary();
                } else {
                    self.do_discretionary();
                }
            }
            Penalty => {
                let n = self.scan_int();
                if self.mode.is_v() {
                    self.append_v_penalty(n);
                } else if self.mode.is_m() {
                    self.append_mlist_node(Node::Penalty(n, self.eqtb.cur_attr));
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
            PageDiscards | SplitDiscards => {}
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
                    self.push_token(Token::from_cs(id));
                    self.start_paragraph(true);
                } else {
                    self.do_unbox(false, false);
                }
            }
            UnVBox => {
                if self.mode == Mode::Horizontal {
                    // tex.web head_for_vmode: finish even an empty paragraph,
                    // then reprocess the vertical unbox in vertical mode.
                    self.push_token(Token::from_cs(id));
                    self.push_token(Token::from_cs(self.ids.par));
                } else if self.mode == Mode::RestrictedHorizontal {
                    // head_for_vmode closes an inner group before replaying
                    // the unbox; its register number remains unscanned.
                    self.off_save(Token::from_cs(id));
                } else {
                    self.do_unbox(true, false);
                }
            }
            UnHCopy => {
                if self.mode.is_v() {
                    // same tex.web §21105 case as UnHBox (same cmd code)
                    self.push_token(Token::from_cs(id));
                    self.start_paragraph(true);
                } else {
                    self.do_unbox(false, true);
                }
            }
            UnVCopy => {
                if self.mode == Mode::Horizontal {
                    self.push_token(Token::from_cs(id));
                    self.push_token(Token::from_cs(self.ids.par));
                } else if self.mode == Mode::RestrictedHorizontal {
                    self.off_save(Token::from_cs(id));
                } else {
                    self.do_unbox(true, true);
                }
            }
            LastBox => {
                let b = self.take_last_box();
                self.append_take_node_opt(b);
            }
            // tex.web 1045 any_mode(last_item): reading the last node is
            // not a command
            LastKern | LastPenalty | LastSkip => self.report_illegal_case(id),
            VSplit => self.do_vsplit(),
            Insert => self.do_insert(),
            // tex.web §1098: vmode+vadjust is a forbidden case
            VAdjust if self.mode.is_v() => self.report_illegal_case(id),
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
                let g = self.take_assignment_prefixes("\\catcode");
                let (c, character_source) = self.scan_int_with_source();
                self.scan_optional_equals();
                let (v, value_source) = self.scan_int_with_source();
                let character = self.valid_profile_character_code(c);
                if character.is_none() {
                    let max = if self.engine_kind == crate::engine::EngineKind::PdfTeX { 255 } else { 1114111 };
                    self.error_at(
                        &format!("Character code {c} is out of range for \\catcode; expected 0 through {max}; assignment ignored"),
                        character_source,
                    );
                } else if !(0..=15).contains(&v) {
                    self.error_at(
                        &format!(
                            "Category code {v} is out of range; expected 0 through 15; assignment ignored"
                        ),
                        value_source,
                    );
                } else {
                    self.eqtb.assign_cat_code(character.unwrap(), v as u8, g);
                }
            }
            MathCode if self.engine_kind == crate::engine::EngineKind::LuaTeX => {
                self.assign_lua_math_code_command(crate::uprims::MathExt::Tex, "\\mathcode");
            }
            DelCode if self.engine_kind == crate::engine::EngineKind::LuaTeX => {
                self.assign_lua_del_code_command(crate::uprims::MathExt::Tex, "\\delcode");
            }
            MathChar if self.engine_kind == crate::engine::EngineKind::LuaTeX => {
                self.math_char_num_command(crate::uprims::MathExt::Tex, id);
            }
            MathCode => {
                let g = self.take_assignment_prefixes("\\mathcode");
                let (c, character_source) = self.scan_int_with_source();
                self.scan_optional_equals();
                let (v, value_source) = self.scan_int_with_source();
                let character = self.valid_profile_character_code(c);
                if character.is_none() {
                    let max = if self.engine_kind == crate::engine::EngineKind::PdfTeX { 255 } else { 1114111 };
                    self.error_at(
                        &format!("Character code {c} is out of range for \\mathcode; expected 0 through {max}; assignment ignored"),
                        character_source,
                    );
                } else if !(0..=32768).contains(&v) {
                    self.error_at(
                        &format!(
                            "Math code {v} is out of range; expected 0 through 32768; assignment ignored"
                        ),
                        value_source,
                    );
                } else {
                    self.eqtb
                        .assign_math_code_for(character.unwrap(), v as u32, g);
                }
            }
            DelCode => {
                let g = self.take_assignment_prefixes("\\delcode");
                let (c, character_source) = self.scan_int_with_source();
                self.scan_optional_equals();
                let (v, value_source) = self.scan_int_with_source();
                let character = self.valid_profile_character_code(c);
                if character.is_none() {
                    let max = if self.engine_kind == crate::engine::EngineKind::PdfTeX { 255 } else { 1114111 };
                    self.error_at(
                        &format!("Character code {c} is out of range for \\delcode; expected 0 through {max}; assignment ignored"),
                        character_source,
                    );
                } else if !(-1..=0xFF_FFFF).contains(&v) {
                    self.error_at(
                        &format!(
                            "Delimiter code {v} is out of range; expected -1 through 16777215; assignment ignored"
                        ),
                        value_source,
                    );
                } else {
                    self.eqtb
                        .assign_delimiter_code_for(character.unwrap(), i64::from(v), g);
                }
            }
            LcCodeP | UcCodeP => {
                let uppercase = p == UcCodeP;
                let command = if uppercase { "\\uccode" } else { "\\lccode" };
                let global = self.take_assignment_prefixes(command);
                let (character, character_source) = self.scan_int_with_source();
                self.scan_optional_equals();
                let (value, value_source) = self.scan_int_with_source();
                let character = self.valid_profile_character_code(character);
                let value = self.valid_profile_character_code(value);
                if character.is_none() {
                    self.error_at(
                        &format!("Invalid character code for {command}; assignment ignored"),
                        character_source,
                    );
                } else if value.is_none() {
                    self.error_at(
                        &format!("Invalid case-mapping value for {command}; assignment ignored"),
                        value_source,
                    );
                } else {
                    self.eqtb.assign_case_code(
                        character.unwrap(),
                        value.unwrap(),
                        uppercase,
                        global,
                    );
                }
            }
            SfCodeP => {
                let g = self.take_assignment_prefixes("\\sfcode");
                let (c, character_source) = self.scan_int_with_source();
                self.scan_optional_equals();
                let (v, value_source) = self.scan_int_with_source();
                let character = self.valid_profile_character_code(c);
                if character.is_none() {
                    let max = if self.engine_kind == crate::engine::EngineKind::PdfTeX { 255 } else { 1114111 };
                    self.error_at(
                        &format!("Character code {c} is out of range for \\sfcode; expected 0 through {max}; assignment ignored"),
                        character_source,
                    );
                } else if !(0..=32767).contains(&v) {
                    self.error_at(
                        &format!(
                            "Space-factor code {v} is out of range; expected 0 through 32767; assignment ignored"
                        ),
                        value_source,
                    );
                } else {
                    self.eqtb
                        .assign_space_factor_code(character.unwrap(), v as u16, g);
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
            Patterns | Hyphenation => self.do_hyphenation_words(p == Patterns),
            ScanTokens => {
                let _ = self.expand_prim(ScanTokens, id);
            }
            // tex.web mmode+stop: insert_dollar_sign
            Dump | End if self.mode.is_m() => self.insert_dollar_sign(Token::from_cs(id)),
            Dump | End if self.mode == Mode::InternalVertical => {
                // tex.web `privileged`: \end and \dump belong to the
                // outer vertical mode
                self.report_illegal_case(id);
            }
            // tex.web head_for_vmode (hmode+stop): a restricted box cannot
            // end the job, so its group is closed first
            Dump | End if self.mode == Mode::RestrictedHorizontal => {
                self.off_save(Token::from_cs(id))
            }
            Dump if self.mode == Mode::Horizontal => {
                self.push_token(Token::from_cs(id));
                self.par_primitive(Token::from_cs(self.ids.par));
            }
            Dump => {
                if !self.ini_mode {
                    // tex.web:1308/1336: \dump exists only while format-building.
                    self.error("\\dump is only available while building a format; rerun with -ini");
                } else {
                    match crate::format::check_dumpable(self) {
                        Ok(()) => {
                            // tex.web §1328 `format_ident`
                            let int = |p: crate::prim::IntParam| self.int_param_value(p);
                            self.format_ident = format!(
                                " (preloaded format={} {}.{}.{})",
                                self.job_name,
                                int(crate::prim::IntParam::Year),
                                int(crate::prim::IntParam::Month),
                                int(crate::prim::IntParam::Day)
                            );
                            // dumpdata.c store_fmt_file: pre_dump runs first.
                            self.run_lua_callback("pre_dump");
                            self.format_done = true;
                            self.end_occurred = true;
                        }
                        Err(e) => self.error(&format!("Cannot dump the format: {e}")),
                    }
                }
            }
            End => {
                if self.mode.is_h() && !self.mode.is_inner() {
                    // Finish the paragraph, then execute this same \end in
                    // vertical mode. Dropping it made ordinary `text\end`
                    // indistinguishable from an illegal raw EOF.
                    self.push_token(Token::from_cs(id));
                    self.par_primitive(Token::from_cs(self.ids.par));
                    return;
                }
                if self.mode.is_v() {
                    self.push_token(Token::from_cs(id));
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
                    self.push_token(Token::from_cs(id));
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
                        lr: 0,
                        dir: 0, attr: self.eqtb.cur_attr,
                    });
                    self.page_append(Node::Glue(crate::boxes::Glue::fil(
                        crate::boxes::GLUE_FILL,
                        0,
                    ), self.eqtb.cur_attr));
                    self.page_append(Node::Penalty(-0x4000_0000, self.eqtb.cur_attr));
                    self.lua_page_filter(crate::lua_callbacks::page_info::END, false);
                    self.build_page();
                    return;
                }
                if self.engine_kind == crate::engine::EngineKind::LuaTeX {
                    self.lua_stop_open_files();
                }
                self.explicit_end_seen = true;
                self.end_occurred = true;
            }
            Immediate => {
                let t = self.get_x_raw();
                if t.is_cs() {
                    let id2 = t.cs_id();
                    if let Some(Equiv::Prim(p2)) = self.eqtb.resolve(id2).cloned() {
                        match p2 {
                            Prim::Write => {
                                self.do_write(true);
                                return;
                            }
                            Prim::Special => {
                                self.do_special();
                                return;
                            }
                            Prim::OpenOut => {
                                self.do_openout(true);
                                return;
                            }
                            Prim::CloseOut => {
                                self.do_closeout(true);
                                return;
                            }
                            // pdfTeX writes an \immediate form or image
                            // object at once
                            Prim::PdfXForm => {
                                self.do_pdfxform(true);
                                let form = self.pdf_last_xform;
                                self.write_form_procset(form);
                                return;
                            }
                            Prim::PdfXImage => {
                                let previous = self.pdf_last_ximage;
                                self.do_pdfximage();
                                if self.pdf_last_ximage != previous {
                                    let image = self.pdf_last_ximage;
                                    self.write_ximage(image);
                                }
                                return;
                            }
                            _ => {}
                        }
                    }
                }
                self.push_token(t);
            }
            OpenOut => self.do_openout(false),
            CloseOut => self.do_closeout(false),
            Write => self.do_write(false),
            Special => self.do_special(),
            Message => self.do_message(false),
            ErrMessage => self.do_message(true),
            DirectLua => {
                let _ = self.expand_prim(DirectLua, id);
            }
            LuaFunctionCall => {
                let slot = self.scan_int();
                self.call_lua_function(slot);
            }
            LuaBytecodeCall => {
                let slot = self.scan_int();
                self.call_lua_bytecode(slot);
            }
            LuaFunction | LuaBytecode => {
                let _ = self.expand_prim(p, id);
            }
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
                let source = self.current_token_source_mark();
                let Ok(operand_source) = self.prepare_inspection_operand(
                    "\\showbox",
                    "the register number to inspect",
                    source.as_ref(),
                ) else {
                    return;
                };
                let errors_before = self.error_count;
                let previous =
                    std::mem::replace(&mut self.diagnostic_source_override, operand_source);
                let idx = self.scan_reg_num();
                self.diagnostic_source_override = previous;
                if self.error_count == errors_before {
                    self.show_box_register(idx);
                    self.report_logged_inspection("\\showbox", source);
                }
            }
            ShowThe => {
                let source = self.current_token_source_mark();
                let Ok(operand_source) = self.prepare_inspection_operand(
                    "\\showthe",
                    "the quantity or control sequence to inspect",
                    source.as_ref(),
                ) else {
                    return;
                };
                let errors_before = self.error_count;
                self.pending_the_string = Some(std::string::String::new());
                let previous =
                    std::mem::replace(&mut self.diagnostic_source_override, operand_source);
                self.the_scan();
                self.diagnostic_source_override = previous;
                if self.error_count == errors_before {
                    let value = self.pending_the_string.take().unwrap_or_default();
                    self.report_inspection("\\showthe", format!("value: {value}"), source);
                } else {
                    self.pending_the_string = None;
                }
            }
            ShowLists => {
                let source = self.current_token_source_mark();
                // tex.web §1293: begin_diagnostic; show_activities
                let display = self.show_activities();
                self.emit_box_diagnostic(display);
                self.report_logged_inspection("\\showlists", source);
            }
            ShowGroups => {
                let source = self.current_token_source_mark();
                // e-TeX: begin_diagnostic; show_save_groups
                let display = self.show_save_groups();
                self.emit_box_diagnostic(display);
                self.report_logged_inspection("\\showgroups", source);
            }
            ShowTokens => {
                let source = self.current_token_source_mark();
                let Ok(operand_source) = self.prepare_inspection_operand(
                    "\\showtokens",
                    "a braced token list to inspect",
                    source.as_ref(),
                ) else {
                    return;
                };
                let errors_before = self.error_count;
                let previous =
                    std::mem::replace(&mut self.diagnostic_source_override, operand_source);
                let tokens = self.scan_general_text();
                self.diagnostic_source_override = previous;
                if self.error_count == errors_before {
                    let shown = self.diagnostic_tokens_to_string(&tokens, MAX_INSPECTION_BYTES / 2);
                    self.report_inspection("\\showtokens", format!("tokens: {shown}"), source);
                }
            }
            ShowIfs => {
                let source = self.current_token_source_mark();
                let display = self.show_ifs();
                self.emit_box_diagnostic(display);
                self.report_logged_inspection("\\showifs", source);
            }
            Char | RatexLiteralChar => {
                if p == RatexLiteralChar && self.mode.is_v() {
                    self.push_token(Token::from_cs(id));
                    self.start_paragraph(true);
                    return;
                }
                let (value, source) = self.scan_int_with_source();
                let maximum = if self.native_text_active() || self.engine_kind == crate::engine::EngineKind::LuaTeX {
                    0x10ffff
                } else {
                    255
                };
                let character = if (0..=maximum).contains(&value)
                    && char::from_u32(value as u32).is_some()
                {
                    value as u32
                } else {
                    self.error_at(
                        &format!("Character code {value} is out of range for \\char; expected 0 through {maximum} and used 0"),
                        source.clone(),
                    );
                    0
                };
                let previous = std::mem::replace(&mut self.diagnostic_source_override, source);
                if p != RatexLiteralChar
                    || self.mode.is_m()
                    || !self.append_native_literal_char(character)
                {
                    self.unicode_char_token(character, false);
                }
                self.diagnostic_source_override = previous;
            }
            RatexCjkText => {
                let plane = self.scan_pdf_string();
                let slot = self.scan_int();
                let text = if plane.is_empty() {
                    None
                } else {
                    let text = u32::from_str_radix(&plane, 16)
                        .ok()
                        .and_then(|plane| plane.checked_mul(256))
                        .filter(|_| (0..=255).contains(&slot))
                        .and_then(|plane| plane.checked_add(slot as u32))
                        .and_then(char::from_u32);
                    if text.is_none() {
                        self.error("Invalid CJK Unicode plane or character slot");
                    }
                    text
                };
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::CjkText(text), self.eqtb.cur_attr));
            }
            Accent => {
                match self.mode {
                    Mode::Vertical | Mode::InternalVertical => {
                        // tex.web §21103: vmode+accent → back_input and start
                        // a paragraph; the accent is re-processed in hmode.
                        self.push_token(Token::from_cs(id));
                        self.start_paragraph(true);
                    }
                    Mode::Horizontal | Mode::RestrictedHorizontal => self.do_accent(),
                    // mmode+accent is unmatched in tex.web's main_control
                    // switch: silently ignored.
                    Mode::Math | Mode::DisplayMath
                        if self.engine_kind == crate::engine::EngineKind::LuaTeX =>
                    {
                        let command_source = self.current_token_source_mark();
                        self.math_ac_lua(0, true, command_source);
                    }
                    _ => {}
                }
            }
            ExSpace => {
                match self.mode {
                    Mode::Vertical | Mode::InternalVertical => {
                        // tex.web vmode+ex_space: back_input, new_graf(true)
                        self.push_token(Token::from_cs(id));
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
                    // tex.web §1113: a kern follows a character or ligature
                    // even when its italic correction is zero
                    let correction = match self.cur_list.last() {
                        Some(Node::Char { c, font, .. } | Node::Ligature { c, font, .. }) => Some(
                            self.eqtb
                                .fonts
                                .get(*font as usize)
                                .map_or(0, |font| font.char_italic(*c)),
                        ),
                        Some(Node::LuaGlyph(g)) => {
                            Some(self.lua_char(g.font, g.c).map_or(0, |ci| ci.italic))
                        }
                        _ => None,
                    };
                    if let Some(correction) = correction {
                        self.cur_list.push(Node::ExplicitKern(correction, self.eqtb.cur_attr));
                    }
                }
                // tex.web §1112: `mmode+ital_corr: tail_append(new_kern(0))`
                Mode::Math | Mode::DisplayMath => self.append_mlist_node(Node::Kern(0, self.eqtb.cur_attr)),
            },
            // math
            MathChar if self.engine_kind == crate::engine::EngineKind::LuaTeX => {
                self.math_char_num_command(crate::uprims::MathExt::Tex, id)
            }
            MathChar => {
                let command_source = self.current_token_source_mark();
                let (value, source) = self.scan_int_with_source();
                let v = if (0..=32767).contains(&value) {
                    value as u16
                } else {
                    self.error_at(
                        &format!(
                            "Math character code {value} is out of range for \\mathchar; expected 0 through 32767 and used 0"
                        ),
                        source,
                    );
                    0
                };
                if self.mode.is_m() {
                    self.append_mathchar_at(v, command_source);
                }
            }
            MathAccent if self.engine_kind == crate::engine::EngineKind::LuaTeX => {
                if !self.lua_insert_dollar(id) {
                    let command_source = self.current_token_source_mark();
                    self.math_ac_lua(0, false, command_source);
                }
            }
            MathAccent => {
                let command_source = self.current_token_source_mark();
                let (value, source) = self.scan_int_with_source();
                let v = if (0..=32767).contains(&value) {
                    value as u16
                } else {
                    self.error_at(
                        &format!(
                            "Math character code {value} is out of range for \\mathaccent; expected 0 through 32767 and used 0"
                        ),
                        source,
                    );
                    0
                };
                if self.mode.is_m() {
                    self.do_math_accent_at(v, command_source);
                }
            }
            Radical if self.engine_kind == crate::engine::EngineKind::LuaTeX => {
                if !self.lua_insert_dollar(id) {
                    let command_source = self.current_token_source_mark();
                    self.math_radical_lua(0, command_source);
                }
            }
            Radical => {
                let command_source = self.current_token_source_mark();
                let (value, source) = self.scan_int_with_source();
                let v = if (0..0x0800_0000).contains(&value) {
                    value
                } else {
                    self.error_at(
                        &format!(
                            "Delimiter code {value} is out of range for \\radical; expected 0 through 134217727 and used 0"
                        ),
                        source,
                    );
                    0
                };
                if self.mode.is_m() {
                    self.do_radical_at(v, command_source);
                }
            }
            EqNo | LeqNo => {
                // tex.web §1140 mmode+eq_no: legal only in display math
                // (`privileged`), where an open inner group is closed first;
                // the tag itself is typeset in -mmode, where it is illegal
                if self.display_math_is_privileged() {
                    if self.eqtb.cur_group_code() == crate::eqtb::group_code::MATH_SHIFT {
                        self.start_eq_no(matches!(p, Prim::LeqNo));
                    } else {
                        self.off_save(Token::from_cs(id));
                    }
                } else if self.mode.is_m() {
                    let name = self.prim_name(p);
                    self.error(&format!("You can't use `\\{name}' in math mode"));
                } else {
                    self.error("You can't use \\eqno here");
                }
            }
            Overline => {
                if self.mode.is_m() {
                    self.do_overline(false);
                }
            }
            Underline => {
                if self.mode.is_m() {
                    self.do_overline(true);
                }
            }
            Delimiter => {
                let command_source = self.current_token_source_mark();
                if self.engine_kind == crate::engine::EngineKind::LuaTeX {
                    // texmath.c `scan_delimiter_as_mathchar`
                    if !self.lua_insert_dollar(id) {
                        let (class, family, character, _, _) =
                            self.scan_delcode_lua(crate::uprims::MathExt::Tex, true);
                        self.set_math_char_lua(
                            class as u32,
                            family as u32,
                            character as u32,
                            0,
                            command_source,
                        );
                    }
                    return;
                }
                let v = self.scan_delimiter_code("\\delimiter");
                if self.mode.is_m() {
                    // tex.web §1160 mmode+delim_num:
                    // set_math_char(cur_val div @'10000)
                    self.append_mathchar_at((v >> 12) as u16, command_source);
                }
            }
            Above | Over | Atop | OverWithDelims | AtopWithDelims | AboveWithDelims => {
                if self.mode.is_m() {
                    self.do_fraction(p);
                }
            }
            TextFont | ScriptFont | ScriptScriptFont => {
                // Consume prefixes for this assignment before any scanner can
                // fail; otherwise an invalid family/font leaks `\global` (or
                // definition-only prefixes) onto the following assignment.
                let command = match p {
                    TextFont => "\\textfont",
                    ScriptFont => "\\scriptfont",
                    _ => "\\scriptscriptfont",
                };
                let global = self.take_assignment_prefixes(command);
                let style = match p {
                    TextFont => 0u8,
                    ScriptFont => 1,
                    _ => 2,
                };
                // tex.web any_math_fonts: <fam number> <filler> = <filler> <font id>
                let fam = self.scan_math_family(command);
                self.scan_optional_equals();
                let f = self.scan_font_id();
                self.eqtb.assign_style_font(style, fam as u16, f, global);
                self.fixup_math_parameters(fam, usize::from(style), f, global);
            }
            Left | ULeft => {
                if self.mode.is_m() {
                    let command_source = self.current_token_source_mark();
                    let fence = if p == ULeft {
                        self.scan_fence_options()
                    } else {
                        crate::boxes::FenceOpts::NONE
                    };
                    let v = self.scan_delim(true);
                    self.push_math_group_at(v, fence, command_source);
                } else {
                    self.error("Missing $ inserted (\\left)");
                }
            }
            Right | URight => {
                if self.mode.is_m() && !self.mismatched_right_or_middle(Token::from_cs(id), false) {
                    let command_source = self.current_token_source_mark();
                    let fence = if p == URight {
                        self.scan_fence_options()
                    } else {
                        crate::boxes::FenceOpts::NONE
                    };
                    let v = self.scan_delim(true);
                    self.right_delim = Some(v);
                    // ends the \left...\right group
                    self.pop_math_group_delimited_at(v, fence, command_source);
                } else {
                    self.error("Missing $ inserted (\\right)");
                }
            }
            Middle | UMiddle => {
                if self.mode.is_m() && !self.mismatched_right_or_middle(Token::from_cs(id), true) {
                    let command_source = self.current_token_source_mark();
                    let fence = if p == UMiddle {
                        self.scan_fence_options()
                    } else {
                        crate::boxes::FenceOpts::NONE
                    };
                    let v = self.scan_delim(true);
                    let origin = self.math_diagnostic_origin_at(command_source);
                    self.append_mlist_node(Node::DelimBox {
                        small: (v.small_fam, v.small_char),
                        large: (v.large_fam, v.large_char),
                        size: 3,
                        fence,
                        origin, attr: self.eqtb.cur_attr,
                    });
                    self.restart_math_left_group();
                }
            }
            NoLimits | Limits | DisplayLimits => {
                if self.mode.is_m() {
                    self.math_limits = match p {
                        NoLimits => Some(0u8),
                        Limits => Some(1),
                        _ => Some(2),
                    };
                }
            }
            MathChoice => {
                if self.mode.is_m() {
                    // tex.web scans the four style groups immediately; each
                    // body is scanned as a math list and attached as ChoiceAlt
                    self.begin_mathchoice();
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
                    self.append_mlist_node(Node::Style(style, self.eqtb.cur_attr));
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
            HAlign => {
                if self.mode == Mode::Horizontal {
                    self.push_token(Token::from_cs(id));
                    self.push_token(Token::from_cs(self.ids.par));
                    return;
                }
                if self.mode == Mode::RestrictedHorizontal {
                    self.off_save(Token::from_cs(id));
                    return;
                }
                if self.mode.is_m() {
                    // tex.web mmode+halign: `privileged` (display math, not
                    // the tag of \eqno) and then the display's own group
                    if self.display_math_is_privileged() {
                        if self.eqtb.cur_group_code() == crate::eqtb::group_code::MATH_SHIFT {
                            self.begin_halign();
                        } else {
                            self.off_save(Token::from_cs(id));
                        }
                    } else {
                        self.error("You can't use `\\halign' in math mode");
                    }
                    return;
                }
                self.begin_halign();
            }
            VAlign => self.begin_valign(),
            // pdfTeX primitives
            PdfLiteral => {
                let origin = self.scan_pdf_origin();
                let data = self.scan_pdf_string();
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfLiteral {
                    origin,
                    data,
                }, self.eqtb.cur_attr));
            }
            PdfSave => {
                let source = self.current_token_source_mark();
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfSave { source }, self.eqtb.cur_attr));
            }
            PdfRestore => {
                let source = self.current_token_source_mark();
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfRestore { source }, self.eqtb.cur_attr));
            }
            PdfSetMatrix => {
                let source = self.current_token_source_mark();
                let matrix = self.scan_pdf_string();
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfSetMatrix {
                    matrix,
                    source,
                }, self.eqtb.cur_attr));
            }
            PdfStartLink => self.do_pdfstartlink(),
            PdfEndLink => {
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfEndLink, self.eqtb.cur_attr));
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
                let _ = self.pdf_colorstack_init();
            }
            PdfColorStack => {
                // pdfTeX "Implement \pdfcolorstack"
                let mut stack = self.scan_int();
                if stack as usize >= self.color_stacks.len() && stack >= 0 {
                    self.error(&format!(
                        "Unknown color stack number {stack}; allocate it with \\pdfcolorstackinit (using stack 0)"
                    ));
                    stack = 0;
                } else if stack < 0 {
                    self.error("Invalid negative color stack number (using stack 0)");
                    stack = 0;
                }
                use crate::boxes::ColorStackCmd;
                let cmd = if self.scan_keyword(b"set") {
                    Some(ColorStackCmd::Set)
                } else if self.scan_keyword(b"push") {
                    Some(ColorStackCmd::Push)
                } else if self.scan_keyword(b"pop") {
                    Some(ColorStackCmd::Pop)
                } else if self.scan_keyword(b"current") {
                    Some(ColorStackCmd::Current)
                } else {
                    None
                };
                match cmd {
                    Some(cmd) => {
                        let data = match cmd {
                            ColorStackCmd::Set | ColorStackCmd::Push => self.scan_pdf_string(),
                            ColorStackCmd::Pop | ColorStackCmd::Current => std::string::String::new(),
                        };
                        self.append_whatsit(Node::Whatsit(
                            crate::boxes::WhatIt::PdfColorStack { stack, cmd, data },
                        self.eqtb.cur_attr));
                    }
                    None => self.error(
                        "Color stack action is missing; expected set, push, pop, or current",
                    ),
                }
            }
            PdfColorStackPrim => {}
            PdfSavePos => {
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::SavePos { obj: 0 }, self.eqtb.cur_attr));
            }
            PdfLastXPos | PdfLastYPos => {
                let command = if p == PdfLastXPos {
                    "\\pdflastxpos"
                } else {
                    "\\pdflastypos"
                };
                self.error(&format!(
                    "{command} is a value; read it with \\the{command}"
                ));
            }
            PdfObj => self.do_pdfobj(),
            PdfXForm => self.do_pdfxform(false),
            PdfMapFile => self.do_pdfmapfile(),
            PdfMapLine => self.do_pdfmapline(),
            PdfGlyphToUnicode => self.do_pdfglyphtounicode(),
            PdfXImage => self.do_pdfximage(),
            PdfLastObj | PdfLastXForm | PdfLastXImage | PdfLastXImagePages | PdfLastLink
            | PdfLastAnnot => {}
            // object references take an object number (typically
            // `\pdfrefximage\pdflastximage`), so scan it as an integer
            PdfRefObj => {
                let _ = self.scan_int();
            }
            PdfRefXForm => {
                let (id, source) = self.scan_int_with_source();
                let Some((w, h, d)) = self.pdf_xforms.get(&id).copied() else {
                    self.error_at(
                        &format!(
                            "Undefined PDF form object {id} in \\pdfrefxform; reference omitted"
                        ),
                        source,
                    );
                    return;
                };
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfRefXForm {
                    obj: id,
                    w,
                    h,
                    d,
                }, self.eqtb.cur_attr));
            }
            PdfRefXImage => {
                let (id, source) = self.scan_int_with_source();
                let Some((w, h, d)) = self
                    .pdf_images
                    .get(&id)
                    .map(|info| (info.width, info.height, info.depth))
                else {
                    self.error_at(
                        &format!(
                            "Undefined PDF image object {id} in \\pdfrefximage; reference omitted"
                        ),
                        source,
                    );
                    return;
                };
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfRefXImage {
                    obj: id,
                    w,
                    h,
                    d,
                }, self.eqtb.cur_attr));
            }
            PdfFontAttr => self.do_pdffontattr(),
            PdfNoBuiltinToUnicode => self.do_pdfnobuiltintounicode(),
            PdfFontExpand => {
                self.do_pdffontexpand();
            }
            PdfNoLigatures => {
                // `\pdfnoligatures` changes its font in place, but it is still
                // the command that must consume pending assignment prefixes.
                self.take_global();
                self.clear_prefixes();
                let f = self.scan_font_id();
                if f != 0 {
                    self.set_no_ligatures(f);
                }
            }
            Letterspacefont => self.do_letterspacefont(),
            PdfSetRandomSeed => {
                // pdftex.web: negative seeds are silently made positive.
                let seed = self.scan_int().saturating_abs();
                self.rng = crate::random::Randoms::new(seed);
            }
            PdfResetTimer => self.timer_start = crate::clock::now_micros(),
            PdfTrailer => self.do_pdftrailer(),
            PdfTrailerId => self.do_pdftrailerid(),
            PdfIncludeChars => self.do_pdfincludechars(),
            PdfCopyFont => self.do_pdfcopyfont(),
            PdfSpaceFont => self.do_pdfspacefont(),
            PdfLastXImageColorDepth => {}
            PdfInterwordSpaceOn | PdfInterwordSpaceOff => self.append_whatsit(Node::Whatsit(
                crate::boxes::WhatIt::PdfInterwordSpace(p == PdfInterwordSpaceOn),
            self.eqtb.cur_attr)),
            PdfFakeSpace => {
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfFakeSpace, self.eqtb.cur_attr))
            }
            PdfRunningLinkOn | PdfRunningLinkOff => self.append_whatsit(Node::Whatsit(
                crate::boxes::WhatIt::PdfRunningLink(p == PdfRunningLinkOn),
            self.eqtb.cur_attr)),
            PdfSnapRefPoint => {
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfSnapRefPoint, self.eqtb.cur_attr))
            }
            PdfSnapY => {
                // pdftex.web new_snap_node: negative snap glue is an error
                let (glue, source) = {
                    let source = self.current_token_source_mark();
                    (self.scan_glue(false), source)
                };
                if glue.width < 0 {
                    self.fatal_error_at(
                        "pdfTeX error (ext1): negative snap glue",
                        source.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    return;
                }
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfSnapY(glue), self.eqtb.cur_attr));
            }
            PdfSnapYComp => {
                let ratio = self.scan_int().clamp(0, 1000);
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfSnapYComp(ratio), self.eqtb.cur_attr));
            }
            PdfUncompress | PdfTolerance | PdfThread | PdfStartThread
            | PdfEndThread => {
                // consume the argument syntactically: most take balanced text
                self.skip_spaces_relax();
                let t = self.get_token();
                if t.is_char() && t.cc() == 1 {
                    self.scan_balanced_raw(true);
                } else if !t.is_cs() {
                    // maybe a number: push it back, digit-dropping corrupts
                    // output (e.g. \pdfobjcompresslevel=\z@ style use)
                    self.push_token(t);
                } else {
                    self.push_token(t);
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
                for b in b"0" {
                    self.push_token(crate::token::Token::char(12, *b as u32));
                }
            }
            XeTeXRevision => {
                for b in b".999998" {
                    self.push_token(crate::token::Token::char(12, *b as u32));
                }
            }
            XeTeXGlyph => {
                self.do_xetex_glyph();
            }
            XeTeXCharClass => {
                self.do_xetex_charclass_assign();
            }
            XeTeXInterCharToks => {
                self.do_xetex_interchartoks_assign();
            }
            XeTeXUseGlyphMetrics => {
                self.scan_optional_equals();
                self.xetex_use_glyph_metrics = self.scan_int();
            }
            XeTeXInterCharTokenState => {
                self.scan_optional_equals();
                self.xetex_interchartokenstate = self.scan_int();
            }
            XeTeXInputNormalization => {
                self.scan_optional_equals();
                self.xetex_input_normalization = self.scan_int();
            }
            XeTeXGenerateActualText => {
                self.scan_optional_equals();
                self.xetex_generate_actual_text = self.scan_int();
            }
            XeTeXDashBreakState => {
                self.scan_optional_equals();
                self.xetex_dash_break_state = self.scan_int();
            }
            CatCodeTable => {
                let g = self.take_assignment_prefixes("\\catcodetable");
                self.scan_optional_equals();
                let table = self.scan_int();
                // luatex maincontrol.c assign_internal_value
                if self.eqtb.cat_table_valid(table) {
                    self.eqtb.assign_cat_table(table, g);
                } else {
                    self.error("Invalid \\catcode table");
                }
            }
            InitCatCodeTable | SaveCatCodeTable => {
                // luatex maincontrol.c run_normal: both are global and
                // never replace the current table.
                self.reject_assignment_prefixes(if p == InitCatCodeTable {
                    "\\initcatcodetable"
                } else {
                    "\\savecatcodetable"
                });
                let table = self.scan_int();
                if !(0..=crate::eqtb::MAX_CAT_TABLE).contains(&table)
                    || table == self.eqtb.cat_table
                {
                    self.error("Invalid \\catcode table");
                } else if p == InitCatCodeTable {
                    self.eqtb.init_cat_table(table);
                } else {
                    self.eqtb.save_cat_table(table);
                }
            }
            XeTeXPicFile | XeTeXPdfFile => {
                self.do_pdfximage();
            }
            Ustack => {
                if self.mode.is_m() {
                    self.do_ustack();
                } else {
                    // maincontrol.c non_math(math_choice_cmd, insert_dollar_sign)
                    self.push_token(Token::from_cs(id));
                    self.push_token(Token::char(3, u32::from(b'$')));
                    self.error("Missing $ inserted");
                }
            }
            Ustartmath => self.math_shift_cs(2, id),
            Ustopmath => self.math_shift_cs(3, id),
            U(u) if !u.is_expandable() => self.uprim_command(u, id),
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
                self.error(&format!(
                    "Command \\{name} is recognized but not implemented by this engine"
                ));
            }
        }
    }

    /// tex.web §1050 report_illegal_case (`you_cant`).
    pub(crate) fn report_illegal_case(&mut self, id: CsId) {
        self.report_illegal_case_in(id, self.mode);
    }

    /// [`Self::report_illegal_case`] for a given mode (luatex's
    /// `after_math` reports after it has left the formula).
    pub(crate) fn report_illegal_case_in(&mut self, id: CsId, mode: Mode) {
        let name = match self.eqtb.resolve(id) {
            Some(crate::eqtb::Equiv::Prim(p)) => self.prim_name(*p),
            _ => ::std::string::String::from_utf8_lossy(self.cs.name(id)).into_owned(),
        };
        let mode = mode.name();
        self.error(&format!("You can't use `\\{name}' in {mode}"));
    }

    /// tex.web §1267-1275 make_accent: `\accent <number 0-255> <filler>
    /// <char>`. Typesets the next character with an accent character taken
    /// from slot <number> of the current font, stacked above the base char
    /// and centered with two kerns, raised so the accent sits at the font's
    /// x-height. The emitted list is
    ///   kern(delta) [accent char] kern(-a-delta) base_char
    /// so the sequence is exactly as wide as the base character.
    fn do_accent(&mut self) {
        let lua_mode = self.engine_kind == crate::engine::EngineKind::LuaTeX;
        let acc: u32 = if lua_mode {
            self.scan_unicode_character_code("\\accent")
        } else {
            u32::from(self.scan_character_code("\\accent"))
        };
        let f_acc = self.eqtb.cur_font_val;
        if self.eqtb.fonts.get(f_acc as usize).is_none() {
            return; // nullfont: nothing happens (tex.web new_character fails)
        }
        let Some(accent_node) = self.new_glyph_node(f_acc, acc) else {
            // char_warning: no accent glyph — drop the accent; the base
            // character stays in the stream and typesets normally.
            let accent_source = self
                .current_token_source_mark()
                .map(|mark| mark.to_context());
            if let Ok(byte) = u8::try_from(acc) {
                self.font_has_character_or_warn(f_acc, byte, accent_source);
            }
            return;
        };
        let (a, _, _) = self.glyph_whd(f_acc, acc);
        let af = &self.eqtb.fonts[f_acc as usize];
        let x = af.x_height();
        let s = f64::from(af.param(1)) / 65536.0; // accent font slant
        // tex.web §1123 do_assignments: expand, skip blanks/\relax and
        // perform assignments (font selections included) until a
        // non-assignment command; `\accent 127 \i` then typesets the
        // dotless ı that \i stands for.
        let t = loop {
            self.skip_spaces_relax();
            let t = self.get_x_raw();
            if !t.is_cs() {
                break t;
            }
            let id = t.cs_id();
            match self.eqtb.resolve(id).cloned() {
                Some(Equiv::FontRef(font)) => {
                    let g = self.take_global();
                    self.eqtb.define_cur_font(font, g);
                    self.clear_prefixes();
                    self.trigger_after_assignment();
                }
                // \afterassignment, \aftergroup, \box and \copy are not
                // prefixed commands: they end do_assignments
                Some(Equiv::Prim(
                    Prim::AfterAssignment | Prim::AfterGroup | Prim::Box | Prim::Copy,
                )) => break t,
                // tex.web §1241: set_box_allowed is false inside do_assignments
                Some(Equiv::Prim(Prim::SetBox)) => {
                    let _ = self.take_assignment_prefixes("\\setbox");
                    self.scan_reg_num();
                    self.scan_optional_equals();
                    self.error("Improper \\setbox");
                    self.trigger_after_assignment();
                }
                Some(Equiv::Prim(p)) if p != Prim::Relax && self.try_assignment(p, id) => {
                    if !matches!(p, Prim::Global | Prim::Long | Prim::Outer | Prim::Protected) {
                        self.trigger_after_assignment();
                    }
                }
                // assignments that main control runs through main_dispatch
                // (\font, \catcode, \textfont, ...) and register aliases
                Some(Equiv::Prim(
                    p @ (Prim::Font
                    | Prim::CatCode
                    | Prim::MathCode
                    | Prim::DelCode
                    | Prim::LcCodeP
                    | Prim::UcCodeP
                    | Prim::SfCodeP
                    | Prim::TextFont
                    | Prim::ScriptFont
                    | Prim::ScriptScriptFont
                    | Prim::Hyphenation
                    | Prim::Patterns),
                )) => {
                    self.main_dispatch(p, id);
                    self.trigger_after_assignment();
                }
                Some(
                    Equiv::CountReg(_)
                    | Equiv::DimenReg(_)
                    | Equiv::SkipReg(_)
                    | Equiv::MuSkipReg(_)
                    | Equiv::ToksReg(_),
                ) => {
                    self.cs_assign(id);
                    self.trigger_after_assignment();
                }
                _ => break t,
            }
        };
        // §1124: a letter, other char, \chardef'd char or \char is the base
        let base: Option<u32> = if t.is_char() && (t.cc() == 11 || t.cc() == 12) {
            Some(t.chr()).filter(|c| lua_mode || *c < 256)
        } else if t.is_cs() {
            match self.eqtb.resolve(t.cs_id()).cloned() {
                Some(Equiv::Prim(Prim::Char)) => Some(if lua_mode {
                    self.scan_unicode_character_code("\\char")
                } else {
                    u32::from(self.scan_character_code("\\char"))
                }),
                Some(Equiv::CharDef(v)) => Some(v).filter(|c| lua_mode || *c < 256),
                Some(Equiv::CharTok(raw)) if matches!(Token(raw).cc(), 11 | 12) => {
                    Some(Token(raw).chr()).filter(|c| lua_mode || *c < 256)
                }
                _ => None,
            }
        } else {
            None
        };
        if base.is_none() {
            self.push_token(t);
        }
        let Some(bc) = base else {
            // no usable base character: append the accent alone
            self.cur_list.push(accent_node);
            self.space_factor = 1000;
            return;
        };
        let f_base = self.eqtb.cur_font_val;
        let Some(base_node) = self.new_glyph_node(f_base, bc) else {
            let source = self
                .current_token_source_mark()
                .map(|mark| mark.to_context());
            if let Ok(byte) = u8::try_from(bc) {
                self.font_has_character_or_warn(f_base, byte, source);
            }
            self.cur_list.push(accent_node);
            self.space_factor = 1000;
            return;
        };
        let (w, h, _) = self.glyph_whd(f_base, bc);
        let t_sl = self
            .eqtb
            .fonts
            .get(f_base as usize)
            .map(|f| f64::from(f.param(1)) / 65536.0)
            .unwrap_or(0.0);
        // If the base height differs from the x-height, the accent char is
        // packed into a box shifted by x-h (tex.web §1274).
        let accent_part: Node = if h != x {
            let mut b = crate::boxes::hpack(vec![accent_node], None, crate::boxes::HBOX, &self.eqtb).node;
            if let Node::Box { shift, .. } = &mut b {
                *shift = x - h;
            }
            b
        } else {
            accent_node
        };
        let delta = ((w - a) as f64 / 2.0 + h as f64 * t_sl - x as f64 * s).round() as i32;
        self.cur_list.push(Node::AccentKern(delta, self.eqtb.cur_attr));
        self.cur_list.push(accent_part);
        self.cur_list.push(Node::AccentKern(-a - delta, self.eqtb.cur_attr));
        self.cur_list.push(base_node);
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
            self.push_token(t);
            return;
        }
    }

    /// LuaTeX `\directlua [ <reg> ] { <lua-code> }`: execute Lua code and inject output.
    fn do_directlua(&mut self) {
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
        if let Err(err) = self.execute_directlua(code.as_bytes()) {
            self.error(&format!("LuaTeX error: {err}"));
        }
    }

    // ---------- \pdfobj / \pdfxform / \pdfximage: object-number allocation

    /// Reserve the next PDF object number. The writer places each reserved
    /// object (\pdfobj, forms, images, \pdfpageref pages, \pdffontobjnum
    /// fonts) at its number; other objects are numbered after them.
    pub(crate) fn alloc_pdf_obj(&mut self) -> i32 {
        let n = self.pdf_next_obj;
        self.pdf_next_obj += 1;
        n
    }

    /// Parse one PDF object body, optionally as a stream or binary file.
    pub fn do_pdfobj(&mut self) {
        let origin = self.current_token_source_mark();
        let mut requested_obj = None;
        let mut stream = false;
        let mut file = false;
        let mut attr = String::new();
        loop {
            if self.scan_keyword(b"reserveobjnum") {
                let obj = self.alloc_pdf_obj();
                self.pdf_reserved_objnums.insert(obj);
                self.pdf_last_obj = obj;
                return;
            } else if self.scan_keyword(b"useobjnum") {
                let errors_before = self.error_count;
                let (obj, source) = self.scan_int_with_source();
                requested_obj = Some((obj, source, self.error_count == errors_before));
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

        let mut omit_requested_obj = false;
        if let Some((obj, source, scanned)) = &requested_obj {
            omit_requested_obj = !scanned;
            let message = if *obj <= 0 {
                Some(format!(
                    "PDF object number {obj} is invalid for \\pdfobj useobjnum; use the positive number returned by \\pdfobj reserveobjnum; object omitted"
                ))
            } else if self.pdf_doc.objects.iter().any(|(used, _)| used == obj) {
                Some(format!(
                    "PDF object number {obj} has already been defined; each reserved object number can be used only once; object omitted"
                ))
            } else if !self.pdf_reserved_objnums.contains(obj) {
                Some(format!(
                    "PDF object number {obj} was not reserved by \\pdfobj reserveobjnum; reserve an object first and pass its \\pdflastobj value; object omitted"
                ))
            } else {
                None
            };
            if *scanned {
                if let Some(message) = message {
                    // pdftex.web: `pdf_retval := -1 {signal the problem}`
                    self.pdf_retval = -1;
                    self.error_at(&message, source.clone());
                    omit_requested_obj = true;
                }
            }
            if omit_requested_obj && self.stopped_on_error {
                return;
            }
        }

        // Allocate before expanding the body, matching pdfTeX: the object can
        // refer to its own freshly updated `\pdflastobj` value. Invalid
        // `useobjnum` requests deliberately leave that value unchanged.
        let obj = if omit_requested_obj {
            None
        } else if let Some((obj, _, _)) = requested_obj {
            self.pdf_reserved_objnums.remove(&obj);
            self.pdf_last_obj = obj;
            Some(obj)
        } else {
            let obj = self.alloc_pdf_obj();
            self.pdf_last_obj = obj;
            Some(obj)
        };
        let toks = self.scan_general_text_expanded();
        let mut body = self.tokens_to_bytes(&toks);
        let Some(obj) = obj else {
            return;
        };
        if file {
            let name = String::from_utf8_lossy(&body).trim().to_string();
            if let Some(bytes) = tex_kpse::get_embedded_package(&name) {
                body = bytes;
            } else {
                let Some(path) = self
                    .resolve_input_path(&name)
                    .or_else(|| self.font_loader.kpse.find_any(&name))
                else {
                    self.fatal_error_at(
                        &format!("PDF object file `{name}` was not found"),
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    return;
                };
                match tex_kpse::fs::read(&path) {
                    Ok(bytes) => {
                        self.record_loaded_bytes(&path, &bytes);
                        self.loaded_files.push(path);
                        body = bytes;
                    }
                    Err(error) => {
                        self.fatal_error_at(
                            &format!("Cannot read PDF object file `{}`: {error}", path.display()),
                            origin.as_ref().map(crate::input::SourceMark::to_context),
                        );
                        return;
                    }
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
    pub fn do_pdfxform(&mut self, immediate: bool) {
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
        let box_reg = self.scan_reg_num();
        let obj = self.alloc_pdf_obj();
        self.pdf_last_xform = obj;
        // \pdfxformname: forms are painted as `/Fm<n> Do`, n = pdf_xform_count
        self.pdf_xform_count += 1;
        self.pdf_doc.form_names.insert(obj, self.pdf_xform_count);
        let b = self.eqtb.boxed.get(box_reg as usize).cloned().flatten();
        let size = match &b {
            Some(Node::Box { w, h, d, .. }) => (*w, *h, *d),
            _ => (0, 0, 0),
        };
        self.pdf_xforms.insert(obj, size);
        self.pdf_pending_forms.insert(
            obj,
            crate::engine::PendingForm { node: b, size, attr, resources },
        );
        if immediate {
            self.ship_pdf_form(obj);
        }
    }

    /// pdftex.web `pdf_ship_out(obj_xform_box, false)`: write a declared
    /// `\pdfxform`. A form that was already shipped is left alone.
    pub(crate) fn ship_pdf_form(&mut self, obj: i32) {
        let Some(crate::engine::PendingForm { node: b, size: (w, h, d), attr, resources }) =
            self.pdf_pending_forms.remove(&obj)
        else {
            return;
        };
        let w_bp = crate::pdfrender::sp_to_bp(w as i64);
        let hd_bp = crate::pdfrender::sp_to_bp(h as i64 + d as i64);
        let attr_str = if attr.trim().is_empty() {
            String::new()
        } else {
            format!(" {}", attr.trim())
        };
        let (content, fonts, image_procset, xforms, ximages) = match &b {
            Some(node) => {
                let form = self.render_form_box(node, w, h, d);
                (form.content, form.fonts, form.image_procset, form.xforms, form.ximages)
            }
            None => (Vec::new(), Vec::new(), 0, Vec::new(), Vec::new()),
        };
        let text = !fonts.is_empty();
        let font_object = self.alloc_pdf_obj();
        self.pdf_doc.objects.push((font_object, b"<< >>".to_vec()));
        self.pdf_doc.form_fonts.push((font_object, fonts));
        // pdftex.web "Generate XObject resources": the forms, then the
        // images, this form painted
        let prefix = &self.pdf_doc.resname_prefix;
        let mut xobj_entries = Vec::new();
        for obj_num in &xforms {
            let name = self.pdf_doc.form_names.get(obj_num).copied().unwrap_or(*obj_num);
            xobj_entries.push(format!("/Fm{name}{prefix} {obj_num} 0 R"));
        }
        for obj_num in &ximages {
            let name = self.pdf_doc.image_names.get(obj_num).copied().unwrap_or(*obj_num);
            xobj_entries.push(format!("/Im{name}{prefix} {obj_num} 0 R"));
        }
        let xobj_res = if xobj_entries.is_empty() || resources.contains("/XObject") {
            String::new()
        } else {
            format!(" /XObject << {} >>", xobj_entries.join(" "))
        };
        let (data, filter) = if !content.is_empty() {
            (crate::pdffile::flate(&content), " /Filter /FlateDecode")
        } else {
            (Vec::new(), "")
        };
        let head = format!(
            "<< /Type /XObject /Subtype /Form /FormType 1 /BBox [0 0 {:.4} {:.4}] /Matrix [1 0 0 1 0 0]{} /Resources << /Font {font_object} 0 R {}",
            w_bp, hd_bp, attr_str, resources.trim()
        );
        // "Generate ProcSet if desired" goes here once the form is written
        self.pdf_form_procsets.insert(
            obj,
            crate::engine::FormProcset {
                offset: head.len(),
                text,
                images: image_procset,
            },
        );
        let mut body = format!(
            "{head}{} >> /Length {}{} >>\nstream\n",
            xobj_res,
            data.len(),
            filter
        )
        .into_bytes();
        body.extend_from_slice(&data);
        body.extend_from_slice(b"\nendstream");
        self.pdf_doc.objects.push((obj, body));
    }

    /// pdfTeX `\pdfcolorstackinit [page] [direct|page] {init}`
    /// (`newcolorstack`): allocate a color stack and return its number.
    pub fn pdf_colorstack_init(&mut self) -> i32 {
        let page_start = self.scan_keyword(b"page");
        let mode = if self.scan_keyword(b"direct") {
            crate::pdfrender::LITERAL_DIRECT_ALWAYS
        } else if self.scan_keyword(b"page") {
            crate::pdfrender::LITERAL_DIRECT_PAGE
        } else {
            crate::pdfrender::LITERAL_SET_ORIGIN
        };
        let init = self.scan_pdf_string();
        match self.color_stacks.new_stack(init, mode, page_start) {
            Some(stack) => stack,
            None => {
                self.error("Too many color stacks");
                0
            }
        }
    }

    /// pdfTeX `scan_image` (\pdfximage [<rule spec>] [attr {..}]
    /// [named {..} | page <n>] [colorspace <n>] [<box spec>] {<file>})
    /// with `read_image` and `scale_image`; \pdflastximage reports it.
    pub fn do_pdfximage(&mut self) {
        use crate::engine::{ImageKind, IMAGE_COLOR_B, IMAGE_COLOR_C, IMAGE_COLOR_I};
        use crate::pdf_images::{PDF_BOX_SPEC_ART, PDF_BOX_SPEC_BLEED, PDF_BOX_SPEC_CROP, PDF_BOX_SPEC_MEDIA, PDF_BOX_SPEC_TRIM};
        let origin = self.current_token_source_mark();
        // check_pdfversion: the first PDF object fixes the output parameters
        self.fix_pdf_output_params();
        let Some(fixed) = self.pdf_fixed else { return };
        let mut scan_w: Option<i32> = None;
        let mut scan_h: Option<i32> = None;
        let mut scan_d: Option<i32> = None;
        loop {
            if self.scan_keyword(b"width") {
                scan_w = Some(self.scan_dimen(false, false));
            } else if self.scan_keyword(b"height") {
                scan_h = Some(self.scan_dimen(false, false));
            } else if self.scan_keyword(b"depth") {
                scan_d = Some(self.scan_dimen(false, false));
            } else {
                break;
            }
        }
        let attr = self.scan_keyword(b"attr").then(|| self.scan_pdf_string());
        let mut named = None;
        let mut page = 1;
        if self.scan_keyword(b"named") {
            named = Some(self.scan_pdf_string());
        } else if self.scan_keyword(b"page") {
            page = self.scan_int();
        }
        let colorspace = if self.scan_keyword(b"colorspace") { self.scan_int() } else { 0 };
        let mut page_box = if self.scan_keyword(b"mediabox") {
            PDF_BOX_SPEC_MEDIA
        } else if self.scan_keyword(b"cropbox") {
            PDF_BOX_SPEC_CROP
        } else if self.scan_keyword(b"bleedbox") {
            PDF_BOX_SPEC_BLEED
        } else if self.scan_keyword(b"trimbox") {
            PDF_BOX_SPEC_TRIM
        } else if self.scan_keyword(b"artbox") {
            PDF_BOX_SPEC_ART
        } else {
            0
        };
        let int = |e: &Self, p: IntParam| e.eqtb.int_params[p.idx() as usize];
        if page_box == 0 {
            page_box = int(self, IntParam::PdfPagebox);
        }
        let file = self.scan_pdf_string();
        // The obsolete options warn once and hand their value to the
        // current parameter (`warn_pdfpagebox`).
        let always = int(self, IntParam::PdfOptionAlwaysUsePdfPagebox);
        if always != 0 {
            self.warning_at(
                "pdfTeX warning (PDF inclusion): Primitive \\pdfoptionalwaysusepdfpagebox is obsolete; use \\pdfpagebox instead.",
                origin.as_ref().map(crate::input::SourceMark::to_context),
            );
            self.eqtb.int_params[IntParam::PdfForcePagebox.idx() as usize] = always;
            self.eqtb.int_params[IntParam::PdfOptionAlwaysUsePdfPagebox.idx() as usize] = 0;
            self.pdf_warned_pagebox = true;
        }
        let option_level = int(self, IntParam::PdfOptionPdfInclusionErrorlevel);
        if option_level != 0 {
            self.warning_at(
                "pdfTeX warning (PDF inclusion): Primitive \\pdfoptionpdfinclusionerrorlevel is obsolete; use \\pdfinclusionerrorlevel instead.",
                origin.as_ref().map(crate::input::SourceMark::to_context),
            );
            self.eqtb.int_params[IntParam::PdfInclusionErrorlevel.idx() as usize] = option_level;
            self.eqtb.int_params[IntParam::PdfOptionPdfInclusionErrorlevel.idx() as usize] = 0;
        }
        let force = int(self, IntParam::PdfForcePagebox);
        if force > 0 {
            if !self.pdf_warned_pagebox {
                self.warning_at(
                    "pdfTeX warning (PDF inclusion): Primitive \\pdfforcepagebox is obsolete; use \\pdfpagebox instead.",
                    origin.as_ref().map(crate::input::SourceMark::to_context),
                );
                self.pdf_warned_pagebox = true;
            }
            page_box = force;
        }
        if page_box == 0 {
            page_box = PDF_BOX_SPEC_CROP;
        }

        let (path, bytes, bundled) = if let Some(bytes) = self.pdfe_memstreams.get(&file).cloned() {
            // registered by `pdfe.new(stream, length, id)`
            (std::path::PathBuf::from(&file), bytes, true)
        } else if let Some(path) = self.resolve_input_path(&file) {
            let bytes = match tex_kpse::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.error_at(
                        &format!("Cannot read image `{}`: {error}", path.display()),
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    return;
                }
            };
            self.record_loaded_bytes(&path, &bytes);
            self.loaded_files.push(path.clone());
            (path, bytes, false)
        } else if !std::path::Path::new(&file).is_absolute() {
            let Some(bytes) = tex_kpse::get_embedded_package(&file) else {
                self.error_at(
                    &format!("Image file `{file}` was not found"),
                    origin.as_ref().map(crate::input::SourceMark::to_context),
                );
                return;
            };
            (std::path::PathBuf::from(&file), bytes, true)
        } else {
            self.error_at(
                &format!("Image file `{file}` was not found"),
                origin.as_ref().map(crate::input::SourceMark::to_context),
            );
            return;
        };
        let file_name = self.kpse_found_name(&path, bundled);
        let is_eps = bytes.starts_with(b"%!PS")
            || bytes.starts_with(b"%!ps")
            || bytes.starts_with(b"%!")
            || bytes.starts_with(&[0xC5, 0xD0, 0xD3, 0xC6]);
        let obj = self.alloc_pdf_obj();
        // \pdfximage: `/Im<n>` takes n = pdf_ximage_count
        self.pdf_ximage_count += 1;
        self.pdf_doc.image_names.insert(obj, self.pdf_ximage_count);
        self.pdf_backend.last_ximage_colordepth = crate::pdftex::image_color_depth(&bytes);
        let mut info = crate::engine::PdfImageInfo {
            path: path.to_string_lossy().into_owned(),
            kind: ImageKind::Pdf,
            used: false,
            written: false,
            width: 0,
            height: 0,
            depth: 0,
            image_width: 0,
            image_height: 0,
            rotate: 0,
            orig_x: 0,
            orig_y: 0,
            color: 0,
            group_ref: 0,
            attr: attr.filter(|attr| !attr.is_empty()),
            colorspace,
            resource_bytes: None,
            pdf_form: None,
            bbox: [0; 4],
        };
        let (mut x_res, mut y_res) = (0i32, 0i32);
        let mut image_pages = 1;
        if bytes.starts_with(b"%PDF-") || is_eps {
            let converted;
            let pdf_bytes = if is_eps {
                match tex_ps::eps_to_pdf(&bytes) {
                    Ok(eps_out) => {
                        converted = eps_out.pdf_bytes;
                        &converted[..]
                    }
                    Err(error) => {
                        self.error_at(
                            &format!("Cannot parse PostScript/EPS `{file}`: {error}"),
                            origin.as_ref().map(crate::input::SourceMark::to_context),
                        );
                        return;
                    }
                }
            } else {
                &bytes[..]
            };
            let options = crate::pdf_images::PdfIncludeOptions {
                page: match &named {
                    Some(name) => crate::pdf_images::PdfPageSelector::Named(name.as_bytes()),
                    None => crate::pdf_images::PdfPageSelector::Number(page),
                },
                page_box,
                file_name: &file_name,
                suppress_ptex_info: int(self, IntParam::PdfSuppressPtexInfo),
                ptex_underscore: int(self, IntParam::PdfPtexUseUnderscore) != 0,
            };
            let included = {
                // pdftoepdf.cc find_add_document: one source per file
                let doc = &mut self.pdf_doc;
                let mut fonts = EngineFontLookup {
                    loader: &mut self.font_loader,
                    programs: &mut doc.imported_programs,
                    replace: !fixed.inclusion_copy_font,
                };
                let mut host = crate::pdf_images::PdfImportHost {
                    next_object: &mut self.pdf_next_obj,
                    base14_fonts: &mut doc.imported_base14_fonts,
                    fonts: &mut fonts,
                    imported_fonts: &mut doc.imported_fonts,
                    font_init_order: self.pdf_backend.initialized_fonts(),
                };
                let source = match doc.pdf_sources.entry(path.to_string_lossy().into_owned()) {
                    std::collections::hash_map::Entry::Occupied(entry) => Ok(entry.into_mut()),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        crate::pdf_images::PdfSource::open(pdf_bytes).map(|source| entry.insert(source))
                    }
                };
                source.and_then(|source| {
                    crate::pdf_images::include_pdf_page(source, &options, &mut host)
                })
            };
            let included = match included {
                Ok(included) => included,
                Err(error) => {
                    // pdftex_fail: fatal, no output file
                    self.fatal_error_at(
                        &format!("pdfTeX error (file {file_name}): {error}"),
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    return;
                }
            };
            // read_pdf_info: a newer PDF than the output is an error, a
            // warning or nothing, per \pdfinclusionerrorlevel.
            let wanted = f64::from((f64::from(fixed.major_version) + f64::from(fixed.minor_version) * 0.1) as f32);
            if included.version as f32 as f64 > wanted + 0.01 {
                let message = format!(
                    "PDF inclusion: found PDF version <{:.1}>, but at most version <{:.1}> allowed",
                    included.version, wanted
                );
                let level = int(self, IntParam::PdfInclusionErrorlevel);
                if level > 0 {
                    self.fatal_error_at(
                        &format!("pdfTeX error (file {file_name}): {message}"),
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    return;
                } else if level == 0 {
                    self.warning_at(
                        &format!("pdfTeX warning (file {file_name}): {message}"),
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                }
            }
            for warning in &included.warnings {
                self.warning_at(
                    &format!("pdfTeX warning (file {file_name}): {warning}"),
                    origin.as_ref().map(crate::input::SourceMark::to_context),
                );
            }
            // writeimg.c bp2int on pdfTeX's single-precision page box
            let bp2int = |bp: f32| (f64::from(bp) * (6_578_176.0 / 100.0)).round() as i32;
            info.image_width = bp2int(included.width);
            info.image_height = bp2int(included.height);
            info.orig_x = bp2int(included.orig_x);
            info.orig_y = bp2int(included.orig_y);
            info.rotate = included.rotate;
            info.group_ref = if included.form.has_group() { -1 } else { 0 };
            info.bbox = [
                info.orig_x,
                info.orig_y,
                info.orig_x + info.image_width,
                info.orig_y + info.image_height,
            ];
            image_pages = included.total_pages;
            self.pdf_doc.objects.extend(
                included
                    .objects
                    .into_iter()
                    .map(|object| (object.obj_num, object.bytes)),
            );
            info.pdf_form = Some(std::sync::Arc::new(included.form));
        } else if bytes.starts_with(&[0xff, 0xd8]) {
            let jpeg = match crate::pdf_images::jpeg_info(&bytes) {
                Ok(jpeg) => jpeg,
                Err(error) => {
                    self.fatal_error_at(
                        &format!("pdfTeX error (file {file_name}): {error}"),
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    return;
                }
            };
            if jpeg.progressive && fixed.major_version == 1 && fixed.minor_version <= 2 {
                self.fatal_error_at(
                    &format!("pdfTeX error (file {file_name}): cannot use progressive DCT with PDF-1.2"),
                    origin.as_ref().map(crate::input::SourceMark::to_context),
                );
                return;
            }
            info.kind = ImageKind::Jpeg;
            info.image_width = i32::from(jpeg.width);
            info.image_height = i32::from(jpeg.height);
            info.color = if jpeg.components == 1 { IMAGE_COLOR_B } else { IMAGE_COLOR_C };
            (x_res, y_res) = (jpeg.x_res, jpeg.y_res);
        } else if let Some(png) = crate::pdf_images::png_info(&bytes) {
            info.kind = ImageKind::Png;
            info.image_width = png.width as i32;
            info.image_height = png.height as i32;
            info.color = match png.color_type {
                3 => IMAGE_COLOR_C | IMAGE_COLOR_I,
                0 | 4 => IMAGE_COLOR_B,
                _ => IMAGE_COLOR_C,
            };
            (x_res, y_res) = (png.x_res, png.y_res);
            // read_png_info: an alpha channel needs a transparency page
            // group (PDF 1.4 and newer)
            if (fixed.major_version > 1 || fixed.minor_version >= 4)
                && matches!(png.color_type, 4 | 6)
            {
                if self.transparent_page_group == 0 {
                    self.transparent_page_group = self.alloc_pdf_obj();
                }
                if self.pdf_page_group_val == 0 {
                    self.pdf_page_group_val = self.transparent_page_group;
                }
                info.group_ref = self.pdf_page_group_val;
            }
        } else if let Some(svg) = crate::pdf_svg::parse_svg_dims(&bytes) {
            // SVG (a ratex extension) is rasterized at its own resolution.
            info.kind = ImageKind::Svg;
            info.image_width = svg.width as i32;
            info.image_height = svg.height as i32;
            info.color = IMAGE_COLOR_C;
            x_res = svg.dpi.round() as i32;
            y_res = x_res;
        } else {
            self.error_at(
                &format!(
                    "Unsupported or invalid image `{file}` (expected PDF, JPEG, or PNG); valid SVG is also accepted"
                ),
                origin.as_ref().map(crate::input::SourceMark::to_context),
            );
            return;
        }
        if bundled && info.kind != ImageKind::Pdf {
            info.resource_bytes = Some(std::sync::Arc::new(bytes));
        }
        if !self.scale_image(&mut info, (scan_w, scan_h, scan_d), (x_res, y_res), origin.as_ref()) {
            return;
        }
        self.pdf_last_ximage = obj;
        self.pdf_last_ximage_pages = image_pages;
        self.pdf_images.insert(obj, info);
    }

    /// pdfTeX `scale_image`: the natural size from the pixel size and
    /// resolution (\pdfimageresolution for images without one) or the PDF
    /// page box, then the `width`/`height`/`depth` overrides scaled to the
    /// image's aspect ratio.
    fn scale_image(
        &mut self,
        info: &mut crate::engine::PdfImageInfo,
        (scan_w, scan_h, scan_d): (Option<i32>, Option<i32>, Option<i32>),
        (mut xr, mut yr): (i32, i32),
        origin: Option<&crate::input::SourceMark>,
    ) -> bool {
        use crate::pdfrender::ext_xn_over_d;
        const ONE_INCH: f64 = 4_736_287.0;
        const ONE_HUNDRED_INCH: i64 = 473_628_672;
        let (x, y) = if info.rotate == 90 || info.rotate == 270 {
            std::mem::swap(&mut xr, &mut yr);
            (info.image_height, info.image_width)
        } else {
            (info.image_width, info.image_height)
        };
        if xr > 65535 || yr > 65535 {
            (xr, yr) = (0, 0);
            self.warning_at(
                "pdfTeX warning (ext1): too large image resolution ignored",
                origin.map(crate::input::SourceMark::to_context),
            );
        }
        if x <= 0 || y <= 0 || xr < 0 || yr < 0 {
            self.fatal_error_at(
                "pdfTeX error (ext1): invalid image dimensions",
                origin.map(crate::input::SourceMark::to_context),
            );
            return false;
        }
        if !(xr == 0 && yr == 0)
            && (f64::from(x) / ONE_INCH >= f64::from(xr) || f64::from(y) / ONE_INCH >= f64::from(yr))
        {
            (xr, yr) = (0, 0);
            self.warning_at(
                "pdfTeX warning (ext1): too small image resolution ignored",
                origin.map(crate::input::SourceMark::to_context),
            );
        }
        let (mut w, mut h) = (0, 0);
        if info.kind == crate::engine::ImageKind::Pdf {
            (w, h) = (x, y);
        } else {
            let default_res = self.eqtb.int_params[IntParam::PdfImageResolution.idx() as usize]
                .clamp(0, 65535);
            if default_res > 0 && (xr == 0 || yr == 0) {
                (xr, yr) = (default_res, default_res);
            }
            if scan_w.is_none() && scan_h.is_none() {
                if xr > 0 && yr > 0 {
                    w = ext_xn_over_d(ONE_HUNDRED_INCH, x as i64, 100 * xr as i64);
                    h = ext_xn_over_d(ONE_HUNDRED_INCH, y as i64, 100 * yr as i64);
                } else {
                    w = ext_xn_over_d(ONE_HUNDRED_INCH, x as i64, 7200);
                    h = ext_xn_over_d(ONE_HUNDRED_INCH, y as i64, 7200);
                }
            }
        }
        let (x, y) = (x as i64, y as i64);
        (info.width, info.height, info.depth) = match (scan_w, scan_h, scan_d) {
            (None, None, None) => (w, h, 0),
            // depth given: the natural height splits into height + depth
            (None, None, Some(d)) => (ext_xn_over_d(h as i64, x, y), h - d, d),
            (None, Some(ht), None) => (ext_xn_over_d(ht as i64, x, y), ht, 0),
            (None, Some(ht), Some(d)) => (ext_xn_over_d(ht as i64 + d as i64, x, y), ht, d),
            (Some(wd), None, None) => (wd, ext_xn_over_d(wd as i64, y, x), 0),
            (Some(wd), None, Some(d)) => (wd, ext_xn_over_d(wd as i64, y, x) - d, d),
            (Some(wd), Some(ht), None) => (wd, ht, 0),
            (Some(wd), Some(ht), Some(d)) => (wd, ht, d),
        };
        true
    }

    /// The name kpathsea returns for a found image: relative names get a
    /// leading `./`, files under the document directory are shown
    /// relative to it (pdfTeX writes this as /PTEX.FileName).
    fn kpse_found_name(&self, path: &std::path::Path, bundled: bool) -> String {
        if bundled {
            return path.to_string_lossy().into_owned();
        }
        let path = match &self.main_dir {
            Some(dir) if path.is_absolute() => path.strip_prefix(dir).unwrap_or(path),
            _ => path,
        };
        let name = path.to_string_lossy();
        if path.is_absolute() || name.starts_with("./") || name.starts_with("../") {
            name.into_owned()
        } else {
            format!("./{name}")
        }
    }
}

impl Engine {
    /// Pull just enough raw input to distinguish a genuinely missing
    /// inspection operand from an invalid one. The tokens are replayed through
    /// the command's normal scanner, while the first operand source is retained
    /// for any syntax diagnostic that scanner emits.
    fn prepare_inspection_operand(
        &mut self,
        command: &str,
        expected: &str,
        command_source: Option<&crate::input::SourceMark>,
    ) -> Result<Option<crate::input::SourceContext>, ()> {
        let mut prefix = Vec::new();
        loop {
            let token = self.raw_token();
            if token == crate::input::EOF_MARKER {
                self.fatal_error_at(
                    &format!("File ended after {command}; add {expected}"),
                    command_source.map(crate::input::SourceMark::to_context),
                );
                return Err(());
            }
            let is_space = token.is_char() && (token.cc() == 9 || token.cc() == 10);
            let operand_source = (!is_space)
                .then(|| {
                    self.current_token_source_mark()
                        .map(|mark| mark.to_context())
                })
                .flatten();
            prefix.push(token);
            if !is_space {
                self.push_tokens(prefix);
                return Ok(operand_source);
            }
        }
    }

    pub(crate) fn report_inspection(
        &mut self,
        command: &str,
        detail: String,
        source: Option<crate::input::SourceMark>,
    ) {
        let message = format!("Inspection requested by {command}\n{}", detail.trim_end());
        self.error_at(
            &message,
            source.as_ref().map(crate::input::SourceMark::to_context),
        );
    }

    /// A box display already went to the transcript (and to the terminal
    /// when `\tracingonline>0`); the structured report says where it is,
    /// like tex.web's `OK (see the transcript file)`.
    fn report_logged_inspection(
        &mut self,
        command: &str,
        source: Option<crate::input::SourceMark>,
    ) {
        let message = if self.diagnostic_to_term() {
            format!("Inspection requested by {command}")
        } else {
            format!("Inspection requested by {command} (see the transcript file)")
        };
        self.error_at(
            &message,
            source.as_ref().map(crate::input::SourceMark::to_context),
        );
    }
}


/// The font map and font programs an included PDF's fonts are looked up
/// in (`\pdfinclusioncopyfonts` = 0 replaces them by the map's programs).
struct EngineFontLookup<'a> {
    loader: &'a mut crate::fontload::FontLoader,
    /// Parsed replacement programs by file name (None: file not found).
    programs: &'a mut std::collections::HashMap<String, Option<std::rc::Rc<crate::pdffile::Type1Source>>>,
    replace: bool,
}

impl crate::pdf_images::FontLookup for EngineFontLookup<'_> {
    fn replacement(&mut self, ps_name: &str) -> Option<crate::pdf_images::FontReplacement> {
        if !self.replace {
            return None;
        }
        self.loader.ensure_map();
        let entry = self.loader.map.replacement_for(ps_name)?;
        let ff_name = entry.pfb.clone()?;
        if !self.programs.contains_key(&ff_name) {
            let program = self
                .loader
                .read_program_bytes(&ff_name)
                .map(|bytes| std::rc::Rc::new(crate::pdffile::Type1Source::new(&bytes)));
            self.programs.insert(ff_name.clone(), program);
        }
        // mapfile.c fm_valid_for_font_replacement: the font file must exist
        let program = self.programs[&ff_name].clone()?;
        Some(crate::pdf_images::FontReplacement {
            slant: entry.slant_millis(),
            extend: entry.extend_millis(),
            base_font: entry.fontname,
            subsettable: !entry.full_download,
            ff_name,
            program,
        })
    }

    fn type1_program(&mut self, file: &str) -> Option<Vec<u8>> {
        self.loader.read_type1_dependency(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Engine, InteractionMode};

    fn run_inspections(source: &str) -> Engine {
        let mut engine = Engine::new(false);
        engine.init_primitives();
        engine.set_interaction_mode(InteractionMode::Nonstop);
        engine
            .input
            .push_file("show.tex".to_string(), source.as_bytes().to_vec());
        engine.run();
        engine
    }

    #[test]
    fn show_and_showthe_report_the_target_and_value() {
        let engine =
            run_inspections("\\def\\foo#1{Hello #1}\\count0=42\\show\\foo\\showthe\\count0\\end");

        assert_eq!(engine.diagnostics.len(), 2, "{}", engine.diagnostic_output);
        assert!(engine.diagnostics[0].message.contains("\\foo = macro:#1"));
        assert!(engine.diagnostics[0].message.contains("Hello #1"));
        assert!(engine.diagnostics[1].message.contains("value: 42"));
        assert_eq!(engine.error_count, 2);
        assert!(engine.explicit_end_seen);
    }

    #[test]
    fn etex_inspection_commands_describe_tokens_groups_and_conditionals() {
        let engine = run_inspections(
            "\\begingroup\\iftrue\\showgroups\\showtokens{A \\relax}\\showifs\\fi\\endgroup\\end",
        );

        assert_eq!(engine.diagnostics.len(), 3, "{}", engine.diagnostic_output);
        // e-TeX show_save_groups / show_ifs transcripts
        assert!(
            engine.log.contains(
                "### semi simple group (level 1) entered at line 1 (\\begingroup)\n### bottom level"
            ),
            "{}",
            engine.log
        );
        assert!(engine.diagnostics[1].message.contains("tokens: A \\relax"));
        assert!(
            engine.log.contains("### level 1: \\iftrue entered on line 1"),
            "{}",
            engine.log
        );
    }

    #[test]
    fn showbox_prints_tex_box_display_with_depth_and_breadth_limits() {
        // Expected transcript text from `pdftex -ini -etex` (TeX Live 2026)
        let engine = run_inspections(concat!(
            "\\catcode`\\{=1 \\catcode`\\}=2 \\showboxdepth=1 \\showboxbreadth=3\n",
            "\\setbox0\\hbox to 10pt{\\vrule width 1pt\\kern2pt\\hskip 3pt plus 1fil minus 2pt",
            "\\penalty5 \\hbox{\\kern1pt}}\\showbox0\n",
            "\\setbox1\\vbox{\\hrule height 2pt\\vskip 1pt\\vbox to 5pt{}\\kern1pt}\\showbox1\n",
            "\\showboxdepth=-1 \\showbox1\n\\end\n",
        ));
        let log = &engine.log;
        for expected in [
            "> \\box0=\n\\hbox(0.0+0.0)x10.0, glue set 3.0fil\n.\\rule(*+*)x1.0\n.\\kern 2.0\n.\\glue 3.0 plus 1.0fil minus 2.0\n.etc.\n\n",
            "> \\box1=\n\\vbox(9.0+0.0)x0.0\n.\\rule(2.0+0.0)x*\n.\\glue 1.0\n.\\vbox(5.0+0.0)x0.0\n.etc.\n\n",
            "> \\box1= []\n\n",
        ] {
            assert!(log.contains(expected), "{log}");
        }
        assert_eq!(engine.error_count, 3);
    }

    #[test]
    fn font_mutation_commands_consume_global_prefixes_on_every_exit() {
        for command in [
            "\\textfont-1=\\nullfont",
            "\\pdfnoligatures\\nullfont",
            "\\pdffontexpand\\nullfont",
            "\\letterspacefont\\spaced\\nullfont",
        ] {
            let mut engine = Engine::new(false);
            engine.init_primitives();
            engine.add_nullfont();
            engine.set_interaction_mode(InteractionMode::Nonstop);
            let source = format!("\\count0=0{{\\global{command}\\count0=7}}\\end");
            engine
                .input
                .push_file("prefix-state.tex".to_string(), source.into_bytes());

            engine.run();

            assert_eq!(
                engine.eqtb.count[0], 0,
                "{command} leaked \\global onto the following assignment: {}",
                engine.diagnostic_output
            );
            assert!(!engine.global_flag, "{command}");
            assert!(!engine.long_flag, "{command}");
            assert!(!engine.outer_flag, "{command}");
            assert!(!engine.protected_flag, "{command}");
        }
    }

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
