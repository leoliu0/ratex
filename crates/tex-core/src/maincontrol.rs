//! main_dispatch: the big match over primitives executed in main control.

use crate::boxes::Node;
use crate::engine::Engine;
use crate::engine::{Mode, ScannerStatus};
use crate::eqtb::{Equiv, LevelType, SaveItem};
use crate::prim::*;
use crate::token::{CsId, Token};
use std::fmt::Write;

const MAX_INSPECTION_BYTES: usize = 8 * 1024;
const MAX_INSPECTION_FRAMES: usize = 32;
const MAX_SAFE_SHOWBOX_DEPTH: usize = 16;
const MAX_SAFE_SHOWBOX_BREADTH: usize = 128;
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
                        self.push_token(Token::from_cs(id));
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
                    self.error("\\nonscript is only valid in math mode");
                }
            }
            MSkip => {
                let g = self.scan_glue(true);
                if self.mode.is_m() {
                    self.append_mlist_node(Node::MuGlue(g));
                } else {
                    self.error(
                        "\\mskip is only valid in math mode; use \\hskip or \\vskip for text spacing",
                    );
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
                    self.error("\\mkern is only valid in math mode; use \\kern for text spacing");
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
            HRule => {
                if self.mode == Mode::Horizontal {
                    self.push_token(Token::from_cs(id));
                    self.push_token(Token::from_cs(self.ids.par));
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
            Discretionary => {
                if self.mode.is_v() {
                    // Start the paragraph before adding replacement text,
                    // and let everypar run before scanning the arguments.
                    self.push_token(Token::from_cs(id));
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
                let g = self.take_assignment_prefixes("\\catcode");
                let (c, character_source) = self.scan_int_with_source();
                self.scan_optional_equals();
                let (v, value_source) = self.scan_int_with_source();
                if !(0..=255).contains(&c) {
                    self.error_at(
                        &format!(
                            "Character code {c} is out of range for \\catcode; expected 0 through 255; assignment ignored"
                        ),
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
                    self.eqtb.assign_cat(c as u8, v as u8, g);
                }
            }
            MathCode => {
                let g = self.take_assignment_prefixes("\\mathcode");
                let (c, character_source) = self.scan_int_with_source();
                self.scan_optional_equals();
                let (v, value_source) = self.scan_int_with_source();
                if !(0..=255).contains(&c) {
                    self.error_at(
                        &format!(
                            "Character code {c} is out of range for \\mathcode; expected 0 through 255; assignment ignored"
                        ),
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
                    self.eqtb.assign_math_code(c as u8, v as u16, g);
                }
            }
            DelCode => {
                let g = self.take_assignment_prefixes("\\delcode");
                let (c, character_source) = self.scan_int_with_source();
                self.scan_optional_equals();
                let (v, value_source) = self.scan_int_with_source();
                if !(0..=255).contains(&c) {
                    self.error_at(
                        &format!(
                            "Character code {c} is out of range for \\delcode; expected 0 through 255; assignment ignored"
                        ),
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
                    self.eqtb.assign_del_code(c as u8, v, g);
                }
            }
            LcCodeP => {
                let g = self.take_assignment_prefixes("\\lccode");
                let (c, character_source) = self.scan_int_with_source();
                self.scan_optional_equals();
                let (v, value_source) = self.scan_int_with_source();
                if !(0..=255).contains(&c) {
                    self.error_at(
                        &format!(
                            "Character code {c} is out of range for \\lccode; expected 0 through 255; assignment ignored"
                        ),
                        character_source,
                    );
                } else if !(0..=255).contains(&v) {
                    self.error_at(
                        &format!(
                            "Lowercase code {v} is out of range; expected 0 through 255; assignment ignored"
                        ),
                        value_source,
                    );
                } else {
                    self.eqtb.assign_lc_code(c as u8, v as u8, g);
                }
            }
            SfCodeP => {
                let g = self.take_assignment_prefixes("\\sfcode");
                let (c, character_source) = self.scan_int_with_source();
                self.scan_optional_equals();
                let (v, value_source) = self.scan_int_with_source();
                if !(0..=255).contains(&c) {
                    self.error_at(
                        &format!(
                            "Character code {c} is out of range for \\sfcode; expected 0 through 255; assignment ignored"
                        ),
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
                    self.eqtb.assign_sf_code(c as u8, v as u16, g);
                }
            }
            UcCodeP => {
                let g = self.take_assignment_prefixes("\\uccode");
                let (c, character_source) = self.scan_int_with_source();
                self.scan_optional_equals();
                let (v, value_source) = self.scan_int_with_source();
                if !(0..=255).contains(&c) {
                    self.error_at(
                        &format!(
                            "Character code {c} is out of range for \\uccode; expected 0 through 255; assignment ignored"
                        ),
                        character_source,
                    );
                } else if !(0..=255).contains(&v) {
                    self.error_at(
                        &format!(
                            "Uppercase code {v} is out of range; expected 0 through 255; assignment ignored"
                        ),
                        value_source,
                    );
                } else {
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
                    self.error("\\dump is only available while building a format; rerun with -ini");
                } else {
                    match crate::format::check_dumpable(self) {
                        Ok(()) => {
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
                    self.par_primitive();
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
                    let detail = self.show_box_description(idx);
                    self.report_inspection("\\showbox", detail, source);
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
                let detail = self.show_lists_description();
                self.report_inspection("\\showlists", detail, source);
            }
            ShowGroups => {
                let source = self.current_token_source_mark();
                let detail = self.show_groups_description();
                self.report_inspection("\\showgroups", detail, source);
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
                let detail = self.show_ifs_description();
                self.report_inspection("\\showifs", detail, source);
            }
            Char => {
                let (value, source) = self.scan_int_with_source();
                let character = if (0..=255).contains(&value) {
                    value as u8
                } else {
                    self.error_at(
                        &format!(
                            "Character code {value} is out of range for \\char; expected 0 through 255 and used 0"
                        ),
                        source.clone(),
                    );
                    0
                };
                let previous = std::mem::replace(&mut self.diagnostic_source_override, source);
                self.char_token(character, false);
                self.diagnostic_source_override = previous;
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
                    let correction = match self.cur_list.last() {
                        Some(Node::Char { c, font } | Node::Ligature { c, font, .. }) => self
                            .eqtb
                            .fonts
                            .get(*font as usize)
                            .map_or(0, |font| font.char_italic(*c)),
                        _ => 0,
                    };
                    if correction != 0 {
                        self.cur_list.push(Node::ExplicitKern(correction));
                    }
                }
                Mode::Math | Mode::DisplayMath => self.append_mlist_node(Node::MathKern(0, 0)),
            },
            // math
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
                } else {
                    self.error("You can't use `\\mathchar' here");
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
                } else {
                    self.error("You can't use `\\mathaccent' here");
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
                let command_source = self.current_token_source_mark();
                let v = self.scan_delimiter_code("\\delimiter");
                if self.mode.is_m() {
                    // tex.web §21942-§21944 mmode+delim_num:
                    // set_math_char(cur_val div @'10000)
                    let mc = (v >> 12) as u16;
                    let class = ((mc >> 12) & 0x7) as u8;
                    let fam = ((mc >> 8) & 0xF) as u8;
                    let c = (mc & 0xFF) as u8;
                    let origin = self.math_diagnostic_origin_at(command_source);
                    self.append_mlist_node(Node::MathChar {
                        fam,
                        c,
                        class,
                        origin,
                    });
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
            }
            Left => {
                if self.mode.is_m() {
                    let command_source = self.current_token_source_mark();
                    let v = self.scan_delim_int();
                    self.push_math_group_at(v, command_source);
                } else {
                    self.error("Missing $ inserted (\\left)");
                }
            }
            Right => {
                if self.mode.is_m() {
                    let command_source = self.current_token_source_mark();
                    let v = self.scan_delim_int();
                    self.right_delim = Some(v);
                    // ends the \left...\right group
                    self.pop_math_group_delimited_at(v, command_source);
                } else {
                    self.error("Missing $ inserted (\\right)");
                }
            }
            Middle => {
                if self.mode.is_m() {
                    let command_source = self.current_token_source_mark();
                    let v = self.scan_delim_int();
                    let origin = self.math_diagnostic_origin_at(command_source);
                    let (sf, sc, lf, lc) = crate::math::delim_code_parts_pub(v);
                    self.append_mlist_node(Node::DelimBox {
                        small: (sf, sc),
                        large: (lf, lc),
                        size: 3,
                        origin,
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
            HAlign => {
                if self.mode == Mode::Horizontal {
                    self.push_token(Token::from_cs(id));
                    self.push_token(Token::from_cs(self.ids.par));
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
                }));
            }
            PdfSave => {
                let source = self.current_token_source_mark();
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfSave { source }));
            }
            PdfRestore => {
                let source = self.current_token_source_mark();
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfRestore { source }));
            }
            PdfSetMatrix => {
                let source = self.current_token_source_mark();
                let matrix = self.scan_pdf_string();
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::PdfSetMatrix {
                    matrix,
                    source,
                }));
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
                        self.error(
                            "Missing \\pdfcolorstack operation; expected push, pop, set, current, or default",
                        );
                    }
                }
            }
            PdfColorStackPrim => {}
            PdfSavePos => {
                self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::SavePos { obj: 0 }));
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
            PdfXImageBBox => {
                let _ = self.scan_pdf_ximage_bbox();
            }
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
                }));
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
                    self.push_token(t);
                    self.error("Missing { inserted for \\pdffontattr");
                }
            }
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
                self.random_seed = self.scan_int();
            }
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
                for b in b"2" {
                    self.push_token(crate::token::Token::char(12, *b as u32));
                }
            }
            XeTeXRevision => {
                for b in b".9995" {
                    self.push_token(crate::token::Token::char(12, *b as u32));
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
                self.error(&format!(
                    "Command \\{name} is recognized but not implemented by this engine"
                ));
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
        let acc = self.scan_character_code("\\accent");
        let f_acc = self.eqtb.cur_font_val;
        let Some(af) = self.eqtb.fonts.get(f_acc as usize) else {
            return; // nullfont: nothing happens (tex.web new_character fails)
        };
        if !af.char_present(acc) {
            // char_warning: no accent glyph — drop the accent; the base
            // character stays in the stream and typesets normally.
            let accent_source = self
                .current_token_source_mark()
                .map(|mark| mark.to_context());
            self.font_has_character_or_warn(f_acc, acc, accent_source);
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
            Some(self.scan_character_code("\\char"))
        } else {
            self.push_token(t);
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
            .map(|f| f.char_present(bc))
            .unwrap_or(false);
        if !exists {
            let source = self
                .current_token_source_mark()
                .map(|mark| mark.to_context());
            self.font_has_character_or_warn(f_base, bc, source);
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
            self.push_token(t);
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
            self.push_token(t);
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
        let box_reg = self.scan_reg_num();
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
        let origin = self.current_token_source_mark();
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
        let Some(path) = self.resolve_input_path(&file) else {
            self.error_at(
                &format!("Image file `{file}` was not found"),
                origin.as_ref().map(crate::input::SourceMark::to_context),
            );
            return;
        };
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
        let obj = self.alloc_pdf_obj();
        let mut image_pages = 1;
        let ((nat_w, nat_h), img_bbox) = if bytes.starts_with(b"%PDF-") {
            match crate::pdf_images::import_pdf_page(
                &bytes,
                page,
                page_box,
                obj,
                &mut self.pdf_next_obj,
            ) {
                Ok((w, h, bbox, objects, total_pages)) => {
                    image_pages = total_pages as i32;
                    self.pdf_doc.objects.extend(
                        objects
                            .into_iter()
                            .map(|image| (image.obj_num, image.bytes)),
                    );
                    let sp_per_bp = 72.27 / 72.0 * 65536.0;
                    (
                        (
                            (w * sp_per_bp).round() as i32,
                            (h * sp_per_bp).round() as i32,
                        ),
                        [
                            (bbox[0] * sp_per_bp).round() as i32,
                            (bbox[1] * sp_per_bp).round() as i32,
                            (bbox[2] * sp_per_bp).round() as i32,
                            (bbox[3] * sp_per_bp).round() as i32,
                        ],
                    )
                }
                Err(error) => {
                    self.error_at(
                        &format!("Cannot include PDF `{file}`: {error}"),
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    return;
                }
            }
        } else if let Some(jpeg) = crate::pdf_images::jpeg_info(&bytes) {
            let w_sp = (jpeg.width as f64 / jpeg.dpi_x * 72.27 * 65536.0).round() as i32;
            let h_sp = (jpeg.height as f64 / jpeg.dpi_y * 72.27 * 65536.0).round() as i32;
            ((w_sp, h_sp), [0, 0, w_sp, h_sp])
        } else if let Some((w, h, rx, ry)) = read_png_dims(&bytes) {
            let w_sp = (w as f64 / rx as f64 * 72.27 * 65536.0).round() as i32;
            let h_sp = (h as f64 / ry as f64 * 72.27 * 65536.0).round() as i32;
            ((w_sp, h_sp), [0, 0, w_sp, h_sp])
        } else if let Some(svg) = crate::pdf_svg::parse_svg_dims(&bytes) {
            let w_sp = (svg.width as f64 / svg.dpi * 72.27 * 65536.0).round() as i32;
            let h_sp = (svg.height as f64 / svg.dpi * 72.27 * 65536.0).round() as i32;
            ((w_sp, h_sp), [0, 0, w_sp, h_sp])
        } else {
            self.error_at(
                &format!(
                    "Unsupported or invalid image `{file}` (expected PDF, JPEG, or PNG); valid SVG is also accepted"
                ),
                origin.as_ref().map(crate::input::SourceMark::to_context),
            );
            return;
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
        self.pdf_last_ximage_pages = image_pages;
        self.pdf_images.insert(
            obj,
            crate::engine::PdfImageInfo {
                path: path.to_string_lossy().into_owned(),
                used: false,
                embedded: bytes.starts_with(b"%PDF-"),
                width: w,
                height: h,
                depth: d,
                bbox: img_bbox,
            },
        );
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

    fn show_box_description(&self, register: u16) -> String {
        let mut out = InspectionText::new();
        match self.eqtb.boxed[register as usize].as_ref() {
            Some(node) => {
                out.push(format_args!("\\box{register}:\n"));
                let depth = self.eqtb.int_params[IntParam::ShowBoxDepth.idx() as usize]
                    .max(0)
                    .min(MAX_SAFE_SHOWBOX_DEPTH as i32) as usize;
                let breadth = self.eqtb.int_params[IntParam::ShowBoxBreadth.idx() as usize]
                    .max(0)
                    .min(MAX_SAFE_SHOWBOX_BREADTH as i32) as usize;
                self.show_node_into(node, 0, depth, breadth, &mut out);
            }
            None => out.push(format_args!("\\box{register} is void")),
        }
        out.finish()
    }

    fn show_lists_description(&self) -> String {
        let mut out = InspectionText::new();
        let depth = self.eqtb.int_params[IntParam::ShowBoxDepth.idx() as usize]
            .max(0)
            .min(MAX_SAFE_SHOWBOX_DEPTH as i32) as usize;
        let breadth = self.eqtb.int_params[IntParam::ShowBoxBreadth.idx() as usize]
            .max(0)
            .min(MAX_SAFE_SHOWBOX_BREADTH as i32) as usize;
        out.push(format_args!(
            "mode: {:?}; page list: {} nodes; current list: {} nodes\n",
            self.mode,
            self.page_list.len(),
            self.cur_list.len()
        ));
        self.show_node_list_into("page list", &self.page_list, 0, depth, breadth, &mut out);
        self.show_node_list_into("current list", &self.cur_list, 0, depth, breadth, &mut out);
        out.finish()
    }

    fn show_groups_description(&self) -> String {
        let total = self
            .eqtb
            .save_stack
            .iter()
            .filter(|item| matches!(item, SaveItem::Level(_, _)))
            .count();
        let mut out = InspectionText::new();
        if total == 0 {
            out.push(format_args!(
                "no groups are open; current level is the bottom level"
            ));
            return out.finish();
        }
        out.push(format_args!("{total} group(s) open, innermost first:\n"));
        for item in self
            .eqtb
            .save_stack
            .iter()
            .rev()
            .filter(|item| matches!(item, SaveItem::Level(_, _)))
            .take(MAX_INSPECTION_FRAMES)
        {
            let SaveItem::Level(level, kind) = item else {
                continue;
            };
            out.push(format_args!("  level {level}: {}", group_kind_name(*kind)));
            if let Some((_, mark)) = self
                .diagnostic_group_openings
                .iter()
                .rev()
                .find(|(opening_level, _)| opening_level == level)
            {
                let context = mark.to_context();
                out.push(format_args!(
                    " (opened at {}:{}:{})",
                    context.name, context.line, context.column
                ));
            }
            out.push(format_args!("\n"));
        }
        if total > MAX_INSPECTION_FRAMES {
            out.push(format_args!(
                "  … {} outer group(s) omitted\n",
                total - MAX_INSPECTION_FRAMES
            ));
        }
        out.push(format_args!("  bottom level"));
        out.finish()
    }

    fn show_ifs_description(&self) -> String {
        let total = self.if_stack.len();
        let mut out = InspectionText::new();
        if total == 0 {
            out.push(format_args!("no conditionals are open"));
            return out.finish();
        }
        out.push(format_args!(
            "{total} conditional(s) open, innermost first:\n"
        ));
        for (index, state) in self
            .if_stack
            .iter()
            .rev()
            .take(MAX_INSPECTION_FRAMES)
            .enumerate()
        {
            let command = if (state.loc_cs as usize) < self.cs.len() {
                self.display_cs(state.loc_cs)
            } else {
                "\\if?".to_string()
            };
            let status = if state.if_case >= 0 {
                format!("case {} remaining", state.if_case)
            } else if state.accepting {
                "taking current branch".to_string()
            } else if state.matched {
                "a previous branch matched".to_string()
            } else {
                "skipping current branch".to_string()
            };
            out.push(format_args!("  {}. {command}: {status}", index + 1));
            if let Some(mark) = &state.loc {
                let context = mark.to_context();
                out.push(format_args!(
                    " (opened at {}:{}:{})",
                    context.name, context.line, context.column
                ));
            } else if !state.loc_file.is_empty() && state.loc_line != 0 {
                out.push(format_args!(
                    " (opened at {}:{})",
                    state.loc_file, state.loc_line
                ));
            }
            out.push(format_args!("\n"));
        }
        if total > MAX_INSPECTION_FRAMES {
            out.push(format_args!(
                "  … {} outer conditional(s) omitted",
                total - MAX_INSPECTION_FRAMES
            ));
        }
        out.finish()
    }

    fn show_node_list_into(
        &self,
        label: &str,
        nodes: &[Node],
        depth: usize,
        max_depth: usize,
        breadth: usize,
        out: &mut InspectionText,
    ) {
        out.push(format_args!("{label}:\n"));
        for node in nodes.iter().take(breadth) {
            self.show_node_into(node, depth, max_depth, breadth, out);
            if out.truncated {
                return;
            }
        }
        if nodes.len() > breadth {
            out.push(format_args!(
                "{}… {} node(s) omitted by \\showboxbreadth\n",
                "  ".repeat(depth),
                nodes.len() - breadth
            ));
        }
    }

    pub(crate) fn show_node_into(
        &self,
        node: &Node,
        depth: usize,
        max_depth: usize,
        breadth: usize,
        out: &mut InspectionText,
    ) {
        if out.truncated {
            return;
        }
        let indent = "  ".repeat(depth.min(MAX_SAFE_SHOWBOX_DEPTH));
        match node {
            Node::Box {
                kind,
                w,
                h,
                d,
                shift,
                list,
                ..
            } => {
                let kind = match kind {
                    0 => "hbox",
                    1 => "vbox",
                    2 => "vtop",
                    _ => "vcenter",
                };
                out.push(format_args!(
                    "{indent}{kind}: width {}, height {}, depth {}, shift {}; {} child node(s)\n",
                    self.scaled_to_string(*w),
                    self.scaled_to_string(*h),
                    self.scaled_to_string(*d),
                    self.scaled_to_string(*shift),
                    list.len()
                ));
                if depth >= max_depth {
                    if !list.is_empty() {
                        out.push(format_args!(
                            "{indent}  … children hidden by \\showboxdepth\n"
                        ));
                    }
                    return;
                }
                for child in list.iter().take(breadth) {
                    self.show_node_into(child, depth + 1, max_depth, breadth, out);
                }
                if list.len() > breadth {
                    out.push(format_args!(
                        "{indent}  … {} child node(s) omitted by \\showboxbreadth\n",
                        list.len() - breadth
                    ));
                }
            }
            Node::Char { c, font } => out.push(format_args!(
                "{indent}character {:?} (byte {c}, font {font})\n",
                char::from(*c)
            )),
            Node::Ligature {
                c, font, n_letters, ..
            } => out.push(format_args!(
                "{indent}ligature {:?} (byte {c}, font {font}, {n_letters} source character(s))\n",
                char::from(*c)
            )),
            Node::Glue(glue) => {
                out.push(format_args!("{indent}glue {}\n", self.glue_to_string(glue)))
            }
            Node::MuGlue(glue) => out.push(format_args!(
                "{indent}math glue {}\n",
                self.mu_glue_to_string(glue)
            )),
            Node::Kern(value) => out.push(format_args!(
                "{indent}kern {}\n",
                self.scaled_to_string(*value)
            )),
            Node::ExplicitKern(value) => out.push(format_args!(
                "{indent}explicit kern {}\n",
                self.scaled_to_string(*value)
            )),
            Node::MarginKern { side, width, c, .. } => out.push(format_args!(
                "{indent}{} margin kern {} for {:?}\n",
                if *side == 0 { "left" } else { "right" },
                self.scaled_to_string(*width),
                char::from(*c)
            )),
            Node::Penalty(value) => out.push(format_args!("{indent}penalty {value}\n")),
            Node::Rule {
                width,
                height,
                depth,
            } => out.push(format_args!(
                "{indent}rule: width {}, height {}, depth {}\n",
                self.scaled_to_string(*width),
                self.scaled_to_string(*height),
                self.scaled_to_string(*depth)
            )),
            Node::Leaders { kind, glue, .. } => out.push(format_args!(
                "{indent}leaders kind {kind} with glue {}\n",
                self.glue_to_string(glue)
            )),
            Node::Disc(disc) => out.push(format_args!(
                "{indent}discretionary: {} pre-break, {} post-break, {} no-break node(s)\n",
                disc.pre_break.len(),
                disc.post_break.len(),
                disc.no_break.len()
            )),
            Node::Mark { class, tokens } => out.push(format_args!(
                "{indent}mark class {class}: {}\n",
                self.diagnostic_tokens_to_string(tokens, 256)
            )),
            Node::Ins {
                num,
                height,
                depth: insert_depth,
                cost,
                box_node,
                ..
            } => {
                out.push(format_args!(
                    "{indent}insert {num}: height {}, depth {}, cost {cost}\n",
                    self.scaled_to_string(*height),
                    self.scaled_to_string(*insert_depth)
                ));
                if depth < max_depth {
                    self.show_node_into(box_node, depth + 1, max_depth, breadth, out);
                }
            }
            Node::Adj(value) => out.push(format_args!("{indent}adjustment {value}\n")),
            Node::Whatsit(whatsit) => out.push(format_args!(
                "{indent}whatsit: {}\n",
                whatsit_kind_name(whatsit)
            )),
            Node::Style(style) => out.push(format_args!("{indent}math style {style:?}\n")),
            Node::NonScript => out.push(format_args!("{indent}nonscript\n")),
            Node::Choice => out.push(format_args!("{indent}math choice\n")),
            Node::ChoiceAlt { body } => out.push(format_args!(
                "{indent}math choice alternative: {} node(s)\n",
                body.len()
            )),
            Node::MathChar { fam, c, class, .. } => out.push(format_args!(
                "{indent}math character {c} (family {fam}, class {class})\n"
            )),
            Node::Frac { num, den, .. } => out.push(format_args!(
                "{indent}fraction: {} numerator, {} denominator node(s)\n",
                num.len(),
                den.len()
            )),
            Node::Radical { body, .. } => out.push(format_args!(
                "{indent}radical: {} body node(s)\n",
                body.len()
            )),
            Node::Scripts {
                nucleus, sup, sub, ..
            } => out.push(format_args!(
                "{indent}scripts: {} nucleus, {} superscript, {} subscript node(s)\n",
                nucleus.len(),
                sup.as_ref().map_or(0, Vec::len),
                sub.as_ref().map_or(0, Vec::len)
            )),
            Node::DelimBox {
                small, large, size, ..
            } => out.push(format_args!(
                "{indent}delimiter box: small {small:?}, large {large:?}, size {size}\n"
            )),
            Node::OpLimits { op, above, below } => out.push(format_args!(
                "{indent}operator limits: {} operator, {} above, {} below node(s)\n",
                op.len(),
                above.as_ref().map_or(0, Vec::len),
                below.as_ref().map_or(0, Vec::len)
            )),
            Node::MathKern(value, _) => out.push(format_args!(
                "{indent}math kern {}\n",
                self.scaled_to_string(*value)
            )),
            Node::Accent { fam, c, body, .. } => out.push(format_args!(
                "{indent}math accent {c} (family {fam}); {} body node(s)\n",
                body.len()
            )),
            Node::Overline { body, under } => out.push(format_args!(
                "{indent}{}: {} body node(s)\n",
                if *under { "underline" } else { "overline" },
                body.len()
            )),
            Node::VCenter { box_node } => {
                out.push(format_args!("{indent}vcenter\n"));
                if depth < max_depth {
                    self.show_node_into(box_node, depth + 1, max_depth, breadth, out);
                }
            }
            Node::InsDisc => out.push(format_args!("{indent}insertion discretionary\n")),
            Node::Empty => out.push(format_args!("{indent}empty node\n")),
            Node::VAdjust(nodes) => out.push(format_args!(
                "{indent}vertical adjustment: {} node(s)\n",
                nodes.len()
            )),
        }
    }
}

fn group_kind_name(kind: LevelType) -> &'static str {
    match kind {
        LevelType::Group => "group",
        LevelType::Simple => "brace group",
        LevelType::SemiSimple => "\\begingroup group",
        LevelType::Box => "box group",
        LevelType::MacroCall => "macro-call group",
        LevelType::NoLine => "no-line group",
        LevelType::Balanced => "balanced-text group",
        LevelType::MathShift => "math shift group",
        LevelType::MathLeft => "math left group",
        LevelType::MathGroup => "math group",
    }
}

fn whatsit_kind_name(whatsit: &crate::boxes::WhatIt) -> &'static str {
    use crate::boxes::WhatIt;
    match whatsit {
        WhatIt::PdfLiteral { .. } => "PDF literal",
        WhatIt::PdfColorPush(_) => "PDF color push",
        WhatIt::PdfColorPop => "PDF color pop",
        WhatIt::PdfColorSet(_) => "PDF color set",
        WhatIt::PdfRefXImage { .. } => "PDF image reference",
        WhatIt::PdfRefXForm { .. } => "PDF form reference",
        WhatIt::PdfSave { .. } => "PDF save",
        WhatIt::PdfRestore { .. } => "PDF restore",
        WhatIt::PdfSetMatrix { .. } => "PDF matrix",
        WhatIt::Write { .. } => "deferred write",
        WhatIt::OpenOut { .. } => "deferred openout",
        WhatIt::CloseOut { .. } => "deferred closeout",
        WhatIt::PdfDest { .. } => "PDF destination",
        WhatIt::PdfAnnot { .. } => "PDF annotation",
        WhatIt::PdfStartLink { .. } => "PDF link start",
        WhatIt::PdfEndLink => "PDF link end",
        WhatIt::Special(_) => "special",
        WhatIt::SavePos { .. } => "position save",
        WhatIt::User(_) => "user whatsit",
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
        assert!(engine.diagnostics[0].message.contains("1 group(s) open"));
        assert!(engine.diagnostics[0].message.contains("show.tex:1:"));
        assert!(engine.diagnostics[1].message.contains("tokens: A \\relax"));
        assert!(engine.diagnostics[2]
            .message
            .contains("1 conditional(s) open"));
        assert!(engine.diagnostics[2].message.contains("\\iftrue"));
        assert!(engine.diagnostics[2]
            .message
            .contains("taking current branch"));
    }

    #[test]
    fn showbox_output_obeys_a_hard_breadth_bound() {
        let mut engine = Engine::new(false);
        engine.eqtb.int_params[IntParam::ShowBoxBreadth.idx() as usize] = i32::MAX;
        engine.eqtb.int_params[IntParam::ShowBoxDepth.idx() as usize] = i32::MAX;
        engine.eqtb.boxed[0] = Some(Node::Box {
            kind: crate::boxes::HBOX,
            w: 10 * 65_536,
            h: 2 * 65_536,
            d: 0,
            shift: 0,
            list: vec![Node::Penalty(50); 10_000],
            glue_sign: 0,
            glue_order: 0,
            glue_set: 0.0,
            font: None,
        });

        let shown = engine.show_box_description(0);

        assert!(shown.contains("hbox: width 10.0pt, height 2.0pt"));
        assert!(shown.contains("9872 child node(s) omitted by \\showboxbreadth"));
        assert!(shown.len() <= MAX_INSPECTION_BYTES);
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
