//! Main control: dispatch of unexpandable tokens; assignments (def/let/
//! registers/parameters); box and list building; paragraph triggers.

use crate::engine::{Engine, Mode, ScannerStatus};
use crate::eqtb::{Equiv, LevelType, Macro};
use crate::expand::PAR_REF_FLAG;
use crate::prim::*;
use crate::token::{CsId, Token};
use std::rc::Rc;

// Current l3kernel uses font dimension 65536 as the backing store for its
// largest integer arrays while building the LaTeX format.
const MAX_FONT_DIMENS: i32 = 65_536;
const MAX_PAR_SHAPE_ENTRIES: i32 = 65_535;

impl Engine {
    /// Reject line-breaking parameter values whose signed meaning cannot be
    /// represented safely. This is centralized so direct assignments and
    /// TeX's arithmetic commands recover in the same way.
    pub(crate) fn recover_linebreak_int_parameter(
        &mut self,
        parameter: IntParam,
        value: i32,
        source: Option<crate::input::SourceContext>,
    ) -> i32 {
        if parameter == IntParam::HangAfter && value == i32::MIN {
            self.error_at(
                "\\hangafter value -2147483648 has an unrepresentable magnitude; used -2147483647",
                source,
            );
            return -i32::MAX;
        }
        value
    }

    pub fn run(&mut self) {
        self.main_loop();
    }

    pub fn main_loop(&mut self) {
        // Command-line selection is authoritative when execution starts; the
        // readable e-TeX parameter must begin with that same numeric value.
        self.set_interaction_mode(self.interaction_mode);
        if self.capacity_exceeded() {
            return;
        }
        while !self.end_occurred {
            self.main_steps += 1;
            if self.main_steps & 0x7FF == 0 && self.capacity_exceeded() {
                return;
            }
            let t = self.get_token();
            // have filled the save stack on the preceding iteration. Report
            // either logical limit here, while the relevant source token is
            // still current and before an overflow id can be dispatched.
            if self.structural_capacity_exceeded() {
                return;
            }
            if t == crate::input::EOF_MARKER {
                break;
            }
            self.dispatch(t);
            self.apply_pending_interaction_mode();
            if self.structural_capacity_exceeded() {
                return;
            }
        }
    }
    pub fn dispatch(&mut self, t: Token) {
        if self.output_pending {
            self.output_pending = false;
            let saved = std::mem::replace(&mut self.prev_depth, -1000 * 65536);
            let saved_pg = std::mem::replace(&mut self.prev_graf, 0);
            // tex.web fire_up (§28633-28637) `push_nest; mode:=-vmode`: the
            // output routine runs in INTERNAL vertical mode on a fresh list,
            // NOT in outer vertical mode. Mode::Vertical here would let the
            // page builder keep contributing routine-side material to the
            // contribution list and fire nested routines mid-routine.
            // The interrupted nest's cur_list/space_factor park in
            // output_nest; finish_output splices the routine's own cur_list
            // ahead of the contribution remainder, then restores mode +
            // parked list (pop_nest).
            let saved_mode = std::mem::replace(&mut self.mode, Mode::InternalVertical);
            let parked_list = std::mem::take(&mut self.cur_list);
            let parked_sf = std::mem::replace(&mut self.space_factor, 1000);
            self.output_nest = Some((parked_list, parked_sf));
            // <Resume the page builder> §28652-28661 splices the routine's
            // list AFTER the held-over insertions (which fire_up moved to the
            // head of the contribution list, count = \insertpenalties) and
            // BEFORE the remainder: the cursor starts at that index.
            let n_carry = self.eqtb.int_params
                [crate::prim::IntParam::InsertPenalties.idx() as usize]
                .max(0) as usize;
            self.output_tail = Some((n_carry, saved, saved_pg, saved_mode));
        }
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
        // tex.web main-loop wrapup: leaving the character/ligature loop in
        // unrestricted horizontal mode settles a trailing explicit hyphen
        // into a null discretionary. Character commands defer that decision
        // to append_char_lig, after ligature/kern lookup.
        if matches!(self.mode, Mode::Horizontal | Mode::RestrictedHorizontal) {
            let continues_character = if t.is_cs() {
                match self.eqtb.resolve(t.cs_id()) {
                    Some(Equiv::CharDef(_) | Equiv::Prim(Prim::Char)) => true,
                    Some(Equiv::CharTok(raw)) => matches!(Token(*raw).cc(), 11 | 12),
                    _ => false,
                }
            } else {
                matches!(t.cc(), 11 | 12)
            };
            if !continues_character {
                let font = self.eqtb.cur_font_val;
                if font != 0 {
                    if self.mode == Mode::Horizontal {
                        self.flush_hyphen_disc(font);
                    }
                    self.flush_right_boundary_kern(font);
                }
            }
        }

        if t.is_cs() {
            let id = t.cs_id();

            if let Some(Equiv::FontRef(f)) = self.eqtb.resolve(id).cloned() {
                // tex.web set_font: a group-scoped assignment that also
                // consumes any \global prefix
                let g = self.take_global();
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
                if p == Prim::Relax {
                    return;
                }
                match self.try_assignment(p, id) {
                    true => {
                        if !matches!(
                            p,
                            Prim::Global
                                | Prim::Long
                                | Prim::Outer
                                | Prim::Protected
                                | Prim::AfterAssignment
                        ) {
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
                            | Prim::Letterspacefont
                            | Prim::PdfFontExpand
                            | Prim::PdfNoLigatures
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
                        // Control space `\ ` (ex_space in tex.web): plain
                        // interword glue, no space-factor scaling. Formats
                        // dumped before the primitive was registered carry
                        // `\ ` as an (undefined) cs [0x20]; dispatch it to
                        // the same handler.
                        if name_bytes == [0x20] {
                            match self.mode {
                                Mode::Vertical | Mode::InternalVertical => {
                                    self.push_token(Token::from_cs(id));
                                    self.start_paragraph(true);
                                }
                                _ => self.ex_space(),
                            }
                            return;
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
                                let has_param_refs = k > 0
                                    && body.iter().any(|t| t.0 >= 0x4000_0000 && t.0 < 0x8000_0000);
                                let m = crate::eqtb::Macro {
                                    replacement: Default::default(),
                                    num_params: k,
                                    has_param_refs,
                                    params: vec![Vec::new(); k as usize],
                                    prefix: Vec::new(),
                                    body: body.into(),
                                    long: false,
                                    outer: false,
                                    protected: false,
                                };
                                self.eqtb
                                    .assign(id, Equiv::Macro(std::rc::Rc::new(m)), true);
                                if let Some(Equiv::Macro(m)) = self.eqtb.resolve(id).cloned() {
                                    self.expand_macro(id, &m, id);
                                }
                                return;
                            }
                        }

                        let source = self
                            .current_token_source_mark()
                            .map(|mark| mark.to_context());
                        let message = format!("Undefined control sequence {}", self.display_cs(id));
                        self.error_at(&message, source);
                    }
                    Some(Equiv::Macro(_)) => {

                        // get_token already declined to expand this (\noexpand
                        // freeze, \protected in an edef scan, self-quark stop
                        // marker). Knuth treats frozen dont_expand as \relax.
                        // Re-expanding here loops: self-quark terminators
                        // (\q__tl_recursion_tail) never stop expl3 maps.
                    }
                    Some(Equiv::CharDef(v)) => {
                        self.reject_assignment_prefixes(&format!("\\char\"{v:X}"));
                        self.char_token(v as u8, false);
                    }

                    // tex.web math_given (\S1177) recovers a text-mode
                    // \mathchardef by opening math ("Missing $ inserted").
                    // Our exit_math replays converted tokens back into the
                    // input, which re-feeds the \mathchardef token and loops
                    // forever. Appending the MathChar node to the current
                    // list renders the glyph directly (visually equivalent
                    // for the \fnsymbol/\ast cases) without the replay.
                    Some(Equiv::MathCharDef(v)) => {
                        self.reject_assignment_prefixes(&format!("\\mathchar\"{v:X}"));
                        self.append_mathchar(v as u16);
                    }
                    Some(Equiv::CharTok(v)) => {
                        self.dispatch(Token(v));
                    }
                    _ => {}
                }
            }
        } else {
            if self.global_flag || self.long_flag || self.outer_flag || self.protected_flag {
                self.clear_prefixes();
            }
            let cc = t.cc();
            let c = t.chr() as u8;
            match cc {
                1 => {
                    // tex.web: a `{` in math mode opens a subformula whose
                    // mlist boundary limits \over's numerator; the engine
                    // keeps one flat list per math level and records the
                    // boundary as a position mark
                    if self.mode.is_m() {
                        let saved_mode = self.mode;
                        self.math_group_marks.push((
                            self.math_lists.len(),
                            self.math_lists.last().map(|l| l.len()).unwrap_or(0),
                            saved_mode,
                        ));
                        // tex.web §1197 / §21691 push_math: a subformula group in
                        // math mode enters -mmode (inner math mode, so \ifinner is true).
                        self.mode = Mode::Math;
                    }
                    self.begin_group(true);
                }
                2 => {
                    if self.mode.is_m() {
                        if let Some((depth, start_mark, saved_mode)) = self.math_group_marks.pop() {
                            self.mode = saved_mode;
                            if depth == self.math_lists.len() {
                                if let Some(l) = self.math_lists.last_mut() {
                                    if start_mark <= l.len() {
                                        let inner = l.split_off(start_mark);
                                        l.push(crate::math::finish_math_group(inner));
                                    }
                                }
                            }
                        }
                    }
                    self.end_group();
                }
                3 => {
                    // math shift
                    if self.mode.is_m() {
                        self.exit_math();
                    } else if self.mode.is_v() {
                        // tex.web §1090: math_shift in vertical mode starts a paragraph;
                        // the math_shift is put back on input so it executes AFTER \everypar.
                        self.push_token(t);
                        self.start_paragraph(true);
                    } else {
                        self.enter_math(false);
                    }
                }
                4 => self.error("Misplaced alignment tab character &"),
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

    /// Consume prefixes for an assignment that permits `\global`. Definition
    /// modifiers are diagnosed but do not prevent the assignment, matching
    /// TeX's recovery after an illegal prefix.
    pub(crate) fn take_assignment_prefixes(&mut self, command: &str) -> bool {
        let invalid_definition_prefix = self.long_flag || self.outer_flag || self.protected_flag;
        let source = invalid_definition_prefix
            .then(|| self.current_token_source_mark())
            .flatten();
        let global = self.take_global();
        self.clear_prefixes();
        if invalid_definition_prefix {
            self.error_at(
                &format!("You can't use `\\long' or `\\outer' or `\\protected' with `{command}'."),
                source.map(|mark| mark.to_context()),
            );
        }
        global
    }

    /// Reject every pending prefix before a command that is not an
    /// assignment. The command still executes after the diagnostic.
    fn reject_assignment_prefixes(&mut self, command: &str) {
        if !(self.global_flag || self.long_flag || self.outer_flag || self.protected_flag) {
            return;
        }
        let source = self.current_token_source_mark();
        self.clear_prefixes();
        self.error_at(
            &format!("You can't use a prefix with `{command}'."),
            source.map(|mark| mark.to_context()),
        );
    }

    /// Handle assignment-prefix primitives; returns true if consumed.
    pub fn try_assignment(&mut self, p: Prim, id: CsId) -> bool {
        use Prim::*;
        match p {
            PartokenName => {
                let global = self.take_assignment_prefixes("\\partokenname");
                self.skip_spaces_relax();
                let token = self.raw_token();
                if token.is_cs() {
                    self.eqtb.assign_int_param(
                        IntParam::PartokenNameCs,
                        token.cs_id() as i32,
                        global,
                    );
                } else {
                    self.error("Missing control sequence for \\partokenname");
                }
                true
            }
            AfterAssignment => {
                self.reject_assignment_prefixes("\\afterassignment");
                let tok = self.raw_token();
                self.after_assignment = Some(tok);
                true
            }
            AfterGroup => {
                self.reject_assignment_prefixes("\\aftergroup");
                let tok = self.raw_token();
                self.eqtb.push_save(crate::eqtb::SaveItem::AfterGroup(tok));
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
                let g = self.take_global();
                self.eqtb.assign_count(idx, v, g);
                self.clear_prefixes();
                true
            }
            Dimen => {
                let idx = self.scan_reg_num();
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                let g = self.take_global();
                self.eqtb.assign_dimen(idx, v, g);
                self.clear_prefixes();
                true
            }
            Skip => {
                let idx = self.scan_reg_num();
                self.scan_optional_equals();
                let v = self.scan_glue(false);
                let g = self.take_global();
                self.eqtb.assign_skip(idx, v, g);
                self.clear_prefixes();
                true
            }
            MuSkip => {
                let idx = self.scan_reg_num();
                self.scan_optional_equals();
                let v = self.scan_glue(true);
                let g = self.take_global();
                self.eqtb.assign_muskip(idx, v, g);
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
                let g = self.take_global();
                self.eqtb.assign_toks_reg(idx, toks, g);
                self.clear_prefixes();
                true
            }
            Box => {
                // \box<n> in value position handled in scan paths; in main
                // position it's an error unless followed by use
                self.reject_assignment_prefixes("\\box");
                let idx = self.scan_reg_num();

                let b = self.eqtb.take_box(idx);
                self.append_box_node(b);
                true
            }
            Copy => {
                self.reject_assignment_prefixes("\\copy");
                let idx = self.scan_reg_num();

                let b = self.eqtb.boxed.get(idx as usize).cloned().flatten();

                self.append_box_node(b);
                true
            }
            CountDef => {
                self.do_def_register(|_engine, idx| Equiv::CountReg(idx));
                true
            }
            DimenDef => {
                self.do_def_register(|_engine, idx| Equiv::DimenReg(idx));
                true
            }
            SkipDef => {
                self.do_def_register(|_engine, idx| Equiv::SkipReg(idx));
                true
            }
            MuSkipDef => {
                self.do_def_register(|_engine, idx| Equiv::MuSkipReg(idx));
                true
            }
            ToksDef => {
                self.do_def_register(|_engine, idx| Equiv::ToksReg(idx));
                true
            }
            CharDef => {
                let t = self.scan_definable_cs();
                self.scan_optional_equals();
                let (v, value_source) = self.scan_int_with_source();
                let g = self.take_global();
                if !(0..=255).contains(&v) {
                    self.error_at(
                        &format!(
                            "Character code {v} is out of range for \\chardef; expected 0 through 255 and used 0"
                        ),
                        value_source,
                    );
                    self.eqtb.assign(t, Equiv::CharDef(0), g);
                } else {
                    self.eqtb.assign(t, Equiv::CharDef(v as u32), g);
                }
                self.clear_prefixes();
                true
            }
            MathCharDef => {
                let t = self.scan_definable_cs();
                self.scan_optional_equals();
                let (v, value_source) = self.scan_int_with_source();
                let g = self.take_global();
                if !(0..=32767).contains(&v) {
                    self.error_at(
                        &format!(
                            "Math code {v} is out of range for \\mathchardef; expected 0 through 32767 and used 0"
                        ),
                        value_source,
                    );
                    self.eqtb.assign(t, Equiv::MathCharDef(0), g);
                } else {
                    self.eqtb.assign(t, Equiv::MathCharDef(v as u16), g);
                }
                self.clear_prefixes();
                true
            }
            FontDimen => {
                // Font parameters are always global, but the command must
                // still consume all pending assignment prefixes on errors.
                let _ = self.take_assignment_prefixes("\\fontdimen");
                let idx = self.scan_int();
                let f = self.scan_font_id();
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                if idx <= 0 {
                    self.error("Font dimension number must be positive");
                } else if idx > MAX_FONT_DIMENS {
                    self.error(&format!(
                        "TeX capacity exceeded, sorry [font dimensions={idx}; maximum={MAX_FONT_DIMENS}]"
                    ));
                } else {
                    // tex.web: \fontdimen/\hyphenchar/\skewchar are always global
                    self.eqtb.assign_font_param(f, idx as usize - 1, v, true);
                }
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
            InterLinePenalties | ClubPenalties | WidowPenalties | DisplayWidowPenalties => {
                let command = match p {
                    InterLinePenalties => "\\interlinepenalties",
                    ClubPenalties => "\\clubpenalties",
                    WidowPenalties => "\\widowpenalties",
                    _ => "\\displaywidowpenalties",
                };
                let global = self.take_assignment_prefixes(command);
                self.scan_optional_equals();
                let n = self.scan_int();
                if n <= 0 {
                    self.assign_penalty_shape(p, Vec::new(), global);
                } else if n > MAX_PAR_SHAPE_ENTRIES {
                    self.fatal_error(&format!(
                        "TeX capacity exceeded, sorry [penalty array entries={n}; maximum={MAX_PAR_SHAPE_ENTRIES}]"
                    ));
                } else {
                    let mut values = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        values.push(self.scan_int());
                    }
                    self.assign_penalty_shape(p, values, global);
                }
                self.clear_prefixes();
                true
            }
            ParShape => {
                // tex.web §1070: \parshape n i1 w1 ... in wn; n<=0 clears.
                self.scan_optional_equals();
                let n = self.scan_int();
                let g = self.take_global();
                if n <= 0 {
                    self.assign_par_shape(Vec::new(), g);
                } else if n > MAX_PAR_SHAPE_ENTRIES {
                    self.fatal_error(&format!(
                        "TeX capacity exceeded, sorry [parshape entries={n}; maximum={MAX_PAR_SHAPE_ENTRIES}]"
                    ));
                    // Consuming billions of following dimensions just to
                    // recover would itself be a denial of service.
                } else {
                    let mut shape = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        let indent = self.scan_dimen(false, false);
                        let width = self.scan_dimen(false, false);
                        shape.push((indent, width));
                    }
                    self.assign_par_shape(shape, g);
                }
                self.clear_prefixes();
                true
            }
            IntP(ip) => {
                if let Some(mode) = match ip {
                    crate::prim::IntParam::ErrorStopMode => {
                        Some(crate::engine::InteractionMode::ErrorStop)
                    }
                    crate::prim::IntParam::ScrollMode => {
                        Some(crate::engine::InteractionMode::Scroll)
                    }
                    crate::prim::IntParam::NonStopMode => {
                        Some(crate::engine::InteractionMode::Nonstop)
                    }
                    crate::prim::IntParam::BatchMode => Some(crate::engine::InteractionMode::Batch),
                    _ => None,
                } {
                    self.set_interaction_mode(mode);
                    self.clear_prefixes();
                    return true;
                }
                self.scan_optional_equals();
                // tex.web §17440: \fam is an ordinary eq_word_define —
                // level-tracked so \mathrm/\operator@font groups restore
                // cur_fam at \egroup (a direct write leaks fam 0 into the
                // following subscripts, turning math italic upright)
                let capture_value_source = matches!(
                    ip,
                    IntParam::InteractionMode | IntParam::HangAfter
                );
                let (v, value_source) = if capture_value_source {
                    self.scan_int_with_source()
                } else {
                    (self.scan_int(), None)
                };
                let g = self.take_global();
                if ip == crate::prim::IntParam::InteractionMode && !(0..=3).contains(&v) {
                    self.error_at(
                        &format!(
                            "Bad interaction mode ({v}); expected 0 (batch), 1 (nonstop), 2 (scroll), or 3 (error stop); mode left unchanged"
                        ),
                        value_source.clone(),
                    );
                    self.clear_prefixes();
                    return true;
                }
                let v = self.recover_linebreak_int_parameter(ip, v, value_source);
                if ip == crate::prim::IntParam::PrevGraf {
                    *self.prev_graf_mut() = v;
                }
                if ip == crate::prim::IntParam::DeadCycles {
                    self.dead_cycles = v;
                }
                if ip == crate::prim::IntParam::SpaceFactor {
                    self.space_factor = v;
                }
                self.eqtb.assign_int_param(ip, v, g);
                self.clear_prefixes();
                true
            }
            DimP(dp) => {
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                let g = self.take_global();
                if dp == DimParam::PrevDepth {
                    self.prev_depth = v;
                } else {
                    if dp == DimParam::PageGoal {
                        self.page_goal = v as i64;
                        self.page_goal_set = true;
                    } else if dp == DimParam::VSize && !self.page_box_seen {
                        self.page_goal = if v <= 0 { 0x3FFF_FFFF } else { v as i64 };
                    }
                    self.eqtb.assign_dim_param(dp, v, g);
                }
                self.clear_prefixes();
                true
            }
            GlueP(gp) => {
                self.scan_optional_equals();
                let v = self.scan_glue(gp.is_mu());
                let g = self.take_global();
                self.eqtb.assign_glue_param(gp, v, g);
                self.clear_prefixes();
                true
            }
            ToksP(tp) => {
                self.scan_optional_equals();
                let toks = Rc::new(self.scan_token_list());

                let g = self.take_global();
                self.eqtb.assign_toks_param(tp, toks, g);
                self.clear_prefixes();
                true
            }
            p @ (EfCode | LpCode | RpCode | TagCode | KnBsCode | StBsCode | ShBsCode | KnBcCode
            | KnAcCode) => {
                let f = self.scan_font_id();
                let command = match p {
                    EfCode => "\\efcode",
                    LpCode => "\\lpcode",
                    RpCode => "\\rpcode",
                    TagCode => "\\tagcode",
                    KnBsCode => "\\knbscode",
                    StBsCode => "\\stbscode",
                    ShBsCode => "\\shbscode",
                    KnBcCode => "\\knbccode",
                    KnAcCode => "\\knaccode",
                    _ => unreachable!(),
                };
                let c = self.scan_character_code(command);
                self.scan_optional_equals();
                let v = self.scan_int();
                if p == TagCode {
                    self.set_tag_code(f, c, v);
                } else {
                    let f_idx = f as usize;
                    if f_idx >= self.eqtb.expand.len() {
                        self.eqtb.expand.resize_with(f_idx + 1, Default::default);
                    }
                    match p {
                        EfCode => self.eqtb.expand[f_idx].set_ef_code(c, v),
                        LpCode => self.eqtb.expand[f_idx].set_lp_code(c, v),
                        RpCode => self.eqtb.expand[f_idx].set_rp_code(c, v),
                        KnBsCode => self.eqtb.expand[f_idx].set_kn_bs_code(c, v),
                        StBsCode => self.eqtb.expand[f_idx].set_st_bs_code(c, v),
                        ShBsCode => self.eqtb.expand[f_idx].set_sh_bs_code(c, v),
                        KnBcCode => self.eqtb.expand[f_idx].set_kn_bc_code(c, v),
                        KnAcCode => self.eqtb.expand[f_idx].set_kn_ac_code(c, v),
                        _ => {}
                    }
                }
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
                let capture_value_source = matches!(
                    ip,
                    IntParam::HangAfter
                );
                let (v, value_source) = if capture_value_source {
                    self.scan_int_with_source()
                } else {
                    (self.scan_int(), None)
                };
                let v = self.recover_linebreak_int_parameter(ip, v, value_source);
                let g = self.take_global();
                if ip == crate::prim::IntParam::PrevGraf {
                    *self.prev_graf_mut() = v;
                }
                if ip == crate::prim::IntParam::DeadCycles {
                    self.dead_cycles = v;
                }
                if ip == crate::prim::IntParam::SpaceFactor {
                    self.space_factor = v;
                }
                self.eqtb.assign_int_param(ip, v, g);
                true
            }
            Some(Equiv::Prim(Prim::DimP(dp))) => {
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                let g = self.take_global();
                if dp == DimParam::PrevDepth {
                    self.prev_depth = v;
                } else {
                    if dp == DimParam::PageGoal {
                        self.page_goal = v as i64;
                        self.page_goal_set = true;
                    } else if dp == DimParam::VSize && !self.page_box_seen {
                        self.page_goal = if v <= 0 { 0x3FFF_FFFF } else { v as i64 };
                    }
                    self.eqtb.assign_dim_param(dp, v, g);
                }
                self.clear_prefixes();
                true
            }
            Some(Equiv::Prim(Prim::GlueP(gp))) => {
                self.scan_optional_equals();
                let v = self.scan_glue(gp.is_mu());
                let g = self.take_global();
                self.eqtb.assign_glue_param(gp, v, g);
                self.clear_prefixes();
                true
            }
            Some(Equiv::Prim(Prim::ToksP(tp))) => {
                self.scan_optional_equals();
                let toks = Rc::new(self.scan_token_list());
                let g = self.take_global();
                self.eqtb.assign_toks_param(tp, toks, g);
                self.clear_prefixes();
                true
            }
            Some(Equiv::CountReg(i)) => {
                self.scan_optional_equals();
                let v = self.scan_int();
                let g = self.take_global();
                self.eqtb.assign_count(i, v, g);
                self.clear_prefixes();
                true
            }
            Some(Equiv::DimenReg(i)) => {
                self.scan_optional_equals();
                let v = self.scan_dimen(false, false);
                let g = self.take_global();
                self.eqtb.assign_dimen(i, v, g);
                self.clear_prefixes();
                true
            }
            Some(Equiv::SkipReg(i)) => {
                self.scan_optional_equals();
                let v = self.scan_glue(false);
                let g = self.take_global();
                self.eqtb.assign_skip(i, v, g);
                self.clear_prefixes();
                true
            }
            Some(Equiv::MuSkipReg(i)) => {
                self.scan_optional_equals();
                let v = self.scan_glue(true);
                let g = self.take_global();
                self.eqtb.assign_muskip(i, v, g);
                self.clear_prefixes();
                true
            }
            Some(Equiv::ToksReg(i)) => {
                self.scan_optional_equals();
                let toks = Rc::new(self.scan_token_list());
                let g = self.take_global();
                self.eqtb.assign_toks_reg(i, toks, g);
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
        let g = self.take_global();
        self.eqtb.assign(t, e, g);
        self.clear_prefixes();
    }
    /// scan a cs for definitions (non-expanding, like tex.web's get_token)
    pub fn scan_definable_cs(&mut self) -> CsId {
        self.skip_raw_spaces();
        let t = self.raw_token();

        if t.is_char() && t.cc() == 13 {
            self.definable_cs_recovery_count = 0;
            let aid = self.active_cs_id(t.chr() as u8);

            return aid;
        }
        if !t.is_cs() {
            let n = self.definable_cs_recovery_count;
            self.definable_cs_recovery_count = n.saturating_add(1);
            self.error("Missing control sequence inserted");
            if n >= 8 {
                self.pushed.clear();
                while matches!(
                    self.input.stack.last(),
                    Some(crate::input::Source::TokList { .. })
                ) {
                    self.input.stack.pop();
                }
                self.definable_cs_recovery_count = 0;
            }
            return self.cs.intern(b"inaccessible");
        }
        self.definable_cs_recovery_count = 0;
        t.cs_id()
    }

    fn do_def_register(&mut self, mk: impl Fn(&mut Self, u16) -> Equiv) {
        let t = self.scan_definable_cs();

        self.scan_optional_equals();
        let idx = self.scan_reg_num();
        let e = mk(self, idx);
        let g = self.take_global();
        self.eqtb.assign(t, e, g);
        self.clear_prefixes();
    }
    /// \def/\gdef/\edef/\xdef
    fn do_def(&mut self, p: Prim, _id: CsId) {
        let definition_start = self.current_token_source_mark();
        let preserve_trace = !self.diagnostic_macro_trace.is_empty();
        if preserve_trace {
            self.diagnostic_trace_hold = self.diagnostic_trace_hold.saturating_add(1);
        }
        self.global_flag |= matches!(p, Prim::GDef | Prim::XDef);
        let global = self.take_global();
        let expanded = p == Prim::EDef || p == Prim::XDef;
        let target = self.scan_definable_cs();
        let mut params: Vec<Vec<Token>> = Vec::new();
        let mut num_params = 0u8;
        let mut hash_brace: Option<Token> = None; // tex.web §473 #{ append
        let mut parameter_tokens = 0usize;
        self.def_prefix.clear();
        loop {
            // tex.web scans the parameter text with get_token (NON-expanding)
            let t = self.raw_token();
            if t == crate::input::EOF_MARKER {
                self.fatal_error_at(
                    "File ended in definition",
                    definition_start.as_ref().map(|mark| mark.to_context()),
                );
                if preserve_trace {
                    self.diagnostic_trace_hold -= 1;
                }
                return;
            }
            if t.is_char() && t.cc() == 1 {
                self.push_token(t);
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
                    self.error_at(
                        &format!(
                            "Parameters must be numbered consecutively in the definition of {}; expected #{} but found #{}",
                            self.display_cs(target),
                            num_params.min(9),
                            n
                        ),
                        definition_start.as_ref().map(|mark| mark.to_context()),
                    );
                    num_params = num_params.min(9);
                }
                continue;
            }
            if self.is_macro_param(t) {
                // #n or #{
                let mut t2 = self.raw_token();
                while t2.is_char() && (t2.cc() == 10 || t2.cc() == 0) {
                    t2 = self.raw_token();
                }
                if self.is_macro_param(t2) {
                    if !self.scanned_token_list_has_room(
                        parameter_tokens,
                        1,
                        "macro parameter text size",
                        definition_start.as_ref(),
                    ) {
                        if preserve_trace {
                            self.diagnostic_trace_hold -= 1;
                        }
                        return;
                    }
                    parameter_tokens += 1;
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
                    if !self.scanned_token_list_has_room(
                        parameter_tokens,
                        1,
                        "macro parameter text size",
                        definition_start.as_ref(),
                    ) {
                        if preserve_trace {
                            self.diagnostic_trace_hold -= 1;
                        }
                        return;
                    }
                    parameter_tokens += 1;
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
                let parameter_source = self.current_token_source_mark();
                num_params += 1;
                params.push(Vec::new());
                let expected = u32::from(b'0') + u32::from(num_params.min(9));
                if num_params > 9 || n != expected {
                    let found = char::from_u32(n).unwrap_or('�');
                    self.error_at(
                        &format!(
                            "Parameters must be numbered consecutively in the definition of {}; expected #{} but found #{}",
                            self.display_cs(target),
                            num_params.min(9),
                            found
                        ),
                        parameter_source.as_ref().map(|mark| mark.to_context()),
                    );
                    num_params = num_params.min(9);
                }
                continue;
            }
            if !self.scanned_token_list_has_room(
                parameter_tokens,
                1,
                "macro parameter text size",
                definition_start.as_ref(),
            ) {
                if preserve_trace {
                    self.diagnostic_trace_hold -= 1;
                }
                return;
            }
            parameter_tokens += 1;
            match params.last_mut() {
                Some(last) => last.push(t),
                None => self.def_prefix.push(t),
            }
        }
        let mut body = self.collect_def_body(
            target,
            num_params,
            expanded,
            hash_brace.is_some(),
            definition_start.clone(),
        );
        if preserve_trace {
            self.diagnostic_trace_hold -= 1;
        }
        if self.stopped_on_error {
            return;
        }
        if let Some(b) = hash_brace {
            if !self.scanned_token_list_has_room(
                body.len(),
                1,
                "macro definition size",
                definition_start.as_ref(),
            ) {
                return;
            }
            body.push(b);
        }

        self.finish_def(target, num_params, params, body, global);
    }

    fn finish_def(
        &mut self,
        target: CsId,
        num_params: u8,
        params: Vec<Vec<Token>>,
        body: Vec<Token>,
        global: bool,
    ) {
        let has_param_refs =
            num_params > 0 && body.iter().any(|t| t.0 >= 0x4000_0000 && t.0 < 0x8000_0000);
        let m = Macro {
            replacement: Default::default(),
            num_params,
            has_param_refs,
            params,
            body: body.into(),
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
    fn store_unexpanded_in_edef(
        &mut self,
        out: &mut Vec<Token>,
        toks: &[Token],
        origin: Option<&crate::input::SourceMark>,
    ) -> bool {
        let remaining = crate::input::MAX_TOKEN_LIST_TOKENS.saturating_sub(out.len());
        let mut additional = 0usize;
        for t in toks {
            let width = if t.0 >= PAR_REF_FLAG && t.0 < 0x8000_0000 {
                2
            } else {
                1
            };
            additional = additional.saturating_add(width);
            if additional > remaining {
                break;
            }
        }
        if !self.scanned_token_list_has_room(out.len(), additional, "macro definition size", origin)
        {
            return false;
        }
        for &t in toks {
            if t.is_cs() {
                out.push(Token::from_cs(t.cs_id()));
            } else if t.0 >= PAR_REF_FLAG && t.0 < 0x8000_0000 {
                let n = (t.0 & 0xF) as u8;
                out.push(Token::char(6, b'#' as u32));
                out.push(Token::char(12, b'0' as u32 + n as u32));
            } else {
                out.push(Token::unfreeze(t));
            }
        }
        true
    }

    /// collect macro body until the closing brace at depth 0
    fn collect_def_body(
        &mut self,
        target: CsId,
        num_params: u8,
        expanded: bool,
        brace_consumed: bool,
        definition_start: Option<crate::input::SourceMark>,
    ) -> Vec<Token> {
        let prev_expanded_scan = self.in_expanded_scan;
        if expanded {
            self.in_expanded_scan = true;
        }
        let mut out = Vec::new();
        let mut depth = 1i32;
        if !brace_consumed {
            self.skip_spaces_relax();
            let t = self.get_token();
            if !(t.is_char() && t.cc() == 1) {
                let got = if t.is_cs() {
                    self.display_cs(t.cs_id())
                } else {
                    format!("cc{}:{}", t.cc(), t.chr())
                };
                self.error(&format!("Missing {{ inserted (def body, got {})", got));
                self.in_expanded_scan = prev_expanded_scan;
                return out;
            }
        }
        loop {
            let t = if expanded {
                let raw = self.raw_token();
                if raw.is_cs() && raw.0 < crate::expand::NOEXP_FLAG {
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
                            if let Some(Equiv::ToksReg(i)) = self.eqtb.resolve(nxt.cs_id()).cloned()
                            {
                                let toks = Rc::clone(&self.eqtb.toks[i as usize]);
                                if !self.store_unexpanded_in_edef(
                                    &mut out,
                                    &toks,
                                    definition_start.as_ref(),
                                ) {
                                    self.in_expanded_scan = prev_expanded_scan;
                                    return out;
                                }
                                continue;
                            }
                        }
                        self.push_token(nxt);
                        let toks = self.scan_general_text();
                        if self.stopped_on_error
                            || !self.store_unexpanded_in_edef(
                                &mut out,
                                &toks,
                                definition_start.as_ref(),
                            )
                        {
                            self.in_expanded_scan = prev_expanded_scan;
                            return out;
                        }
                        continue;
                    }
                }
                self.get_token_from(raw)
            } else {
                self.raw_token()
            };
            let from_unexp = expanded && self.unexp_protect > 0;
            if from_unexp {
                self.unexp_protect -= 1;
            }
            if t == crate::input::EOF_MARKER {
                self.fatal_error_at(
                    &format!(
                        "File ended while scanning the definition of {}; add the missing }}",
                        self.display_cs(target)
                    ),
                    definition_start.as_ref().map(|mark| mark.to_context()),
                );
                self.in_expanded_scan = prev_expanded_scan;
                return out;
            }
            if expanded && self.cur_prim == Some(Prim::UnExpanded) {
                self.skip_spaces_relax();
                let nxt = self.raw_token();
                if nxt.is_cs() {
                    if let Some(Equiv::ToksReg(i)) = self.eqtb.resolve(nxt.cs_id()).cloned() {
                        let toks = Rc::clone(&self.eqtb.toks[i as usize]);
                        if !self.store_unexpanded_in_edef(
                            &mut out,
                            &toks,
                            definition_start.as_ref(),
                        ) {
                            self.in_expanded_scan = prev_expanded_scan;
                            return out;
                        }
                        continue;
                    }
                }
                self.push_token(nxt);
                let toks = self.scan_general_text();
                if self.stopped_on_error
                    || !self.store_unexpanded_in_edef(&mut out, &toks, definition_start.as_ref())
                {
                    self.in_expanded_scan = prev_expanded_scan;
                    return out;
                }
                continue;
            }
            if self.is_macro_param(t) {
                if !self.scanned_token_list_has_room(
                    out.len(),
                    1,
                    "macro definition size",
                    definition_start.as_ref(),
                ) {
                    self.in_expanded_scan = prev_expanded_scan;
                    return out;
                }
                // `\unexpanded{#1}` inside `\edef\foo#1#2{...}` must store
                // a literal hash, not parameter 1 of `\foo`.
                // Unexpanded control-sequence parameters (e.g. `\@sharp` from
                // `\the\toks` in `revtex4-2` tabular preambles) must also store
                // the literal token rather than demanding a parameter digit.
                if from_unexp || self.unexpanded_parameter || self.no_expand_tok == Some(t) {
                    out.push(if t.is_cs() { t } else { Token::char(6, b'#' as u32) });
                    continue;
                }
                let t2 = self.raw_token();
                let parameter_source = self.current_token_source_mark();
                if t2 == crate::input::EOF_MARKER {
                    self.fatal_error_at(
                        &format!(
                            "File ended after # in the definition of {}; add a parameter number, another #, and the missing }}",
                            self.display_cs(target)
                        ),
                        definition_start.as_ref().map(|mark| mark.to_context()),
                    );
                    self.in_expanded_scan = prev_expanded_scan;
                    return out;
                }
                if self.is_macro_param(t2) {
                    out.push(Token::char(6, b'#' as u32));
                    continue;
                }
                if t2.is_char() && (b'1'..=b'9').contains(&(t2.chr() as u8)) {
                    let parameter = (t2.chr() & 0xF) as u8;
                    if parameter <= num_params {
                        out.push(Token(PAR_REF_FLAG | u32::from(parameter)));
                    } else {
                        let declared = if num_params == 0 {
                            "this macro declares no parameters".to_string()
                        } else {
                            format!("only #1 through #{num_params} are declared")
                        };
                        self.error_at(
                            &format!(
                                "Illegal parameter reference #{} in the definition of {}; {}",
                                parameter,
                                self.display_cs(target),
                                declared
                            ),
                            parameter_source.as_ref().map(|mark| mark.to_context()),
                        );
                        out.push(Token::other(b'?'));
                    }
                    continue;
                }
                let found = if t2.is_char() {
                    char::from_u32(t2.chr()).unwrap_or('?').to_string()
                } else {
                    self.display_cs(t2.cs_id())
                };
                self.error_at(
                    &format!(
                        "Illegal parameter reference #{} in the definition of {}; use ## for a literal #",
                        found,
                        self.display_cs(target)
                    ),
                    parameter_source.as_ref().map(|mark| mark.to_context()),
                );
                out.push(Token::other(b'?'));
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
            }
            if !self.scanned_token_list_has_room(
                out.len(),
                1,
                "macro definition size",
                definition_start.as_ref(),
            ) {
                self.in_expanded_scan = prev_expanded_scan;
                return out;
            }
            let t = self.unfreeze_input_token(t);
            out.push(t);
        }
    }

    /// \let (and \futurelet)
    fn do_let(&mut self, future: bool) {
        let global = self.take_global();
        let target = self.scan_definable_cs();
        if future {
            let tb = self.raw_token();
            let tc = self.raw_token();
            if tc.is_cs() {
                self.copy_meaning(target, tc.cs_id(), global);
            } else if tc.is_char() && tc.cc() == 13 {
                let id = self.active_cs_id(tc.chr() as u8);
                self.copy_meaning(target, id, global);
            } else {
                self.eqtb.assign(target, Equiv::CharTok(tc.0), global);
            }
            if tc.is_cs() && self.cs.name(tc.cs_id()) == b"end" {
                self.push_tokens_named(vec![tc], "futurelet-end");
                self.push_token(tb);
            } else {
                self.push_token(tc);
                self.push_token(tb);
            }
        } else {
            self.skip_raw_spaces();
            let eq = self.raw_token();
            if eq.is_char() && eq.chr() == b'=' as u32 && eq.cc() == 12 {
                let sp = self.raw_token();
                if !(sp.is_char() && sp.cc() == 10) {
                    self.push_token(sp);
                }
            } else {
                self.push_token(eq);
            }
            let t = self.raw_token();
            if t.is_cs() {
                self.copy_meaning(target, t.cs_id(), global);
            } else if t.is_char() && t.cc() == 13 {
                let id = self.active_cs_id(t.chr() as u8);
                self.copy_meaning(target, id, global);
            } else if t.0 >= crate::expand::PAR_REF_FLAG
                && t.0 < 0xFFFF_0000
                && t.0 != crate::input::PAR_END.0
            {
                self.error("Missing control sequence after \\let");
            } else {
                self.eqtb.assign(target, Equiv::CharTok(t.0), global);
            }
        }
        self.clear_prefixes();
    }

    /// copy meaning from src cs to dst cs (TeX \let semantics)
    fn copy_meaning(&mut self, dst: CsId, src: CsId, global: bool) {
        match self.eqtb.resolve(src).cloned() {
            None => {
                let name = self.cs.name(src);
                if name == b"/" || name == b"@@italiccorr" {
                    self.eqtb.assign(dst, Equiv::Prim(Prim::Relax), global);
                } else {
                    self.eqtb.undefine(dst, global);
                }
            }
            Some(eq) => self.eqtb.assign(dst, eq, global),
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
                        if !matches!(
                            p,
                            Prim::Global
                                | Prim::Long
                                | Prim::Outer
                                | Prim::Protected
                                | Prim::AfterAssignment
                        ) {
                            self.trigger_after_assignment();
                        }
                    } else {
                        self.main_dispatch(p, id);
                    }
                    return;
                }
                Some(Equiv::Macro(m)) => {
                    self.expand_macro(id, &m, id);
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
                Some(Equiv::Prim(Prim::ToksP(p))) => {
                    return (*self.eqtb.tok_params[p.idx() as usize]).clone()
                }
                Some(Equiv::Prim(Prim::Toks)) => {
                    let i = self.scan_reg_num();
                    return (*self.eqtb.toks[i as usize]).clone();
                }
                Some(Equiv::Prim(Prim::CsName)) => {
                    let id = self.scan_csname_explicit();
                    return vec![Token::from_cs(id)];
                }
                _ => {}
            }
        }
        if t.is_char() && t.cc() == 1 {
            return self.scan_balanced_raw(true).to_vec();
        }
        self.error("Missing { inserted (token list)");
        self.push_token(t);
        Vec::new()
    }

    pub fn scan_csname_explicit(&mut self) -> CsId {
        const MAX_CONTROL_SEQUENCE_NAME_BYTES: usize = 2000;
        let mut origin = (self.input.current_file_line() != 0)
            .then(|| self.current_token_source_mark())
            .flatten();
        let mut name: Vec<u8> = Vec::new();
        loop {
            let t = self.get_token();
            if origin.is_none() {
                origin = self
                    .current_token_source_mark()
                    .or_else(|| self.input.current_source_mark());
            }
            if t == crate::input::EOF_MARKER {
                break;
            }
            if t.is_cs() {
                break;
            }
            if name.len() == MAX_CONTROL_SEQUENCE_NAME_BYTES {
                self.fatal_error_at(
                    "TeX capacity exceeded, sorry [control sequence name exceeds 2000 bytes]",
                    origin.as_ref().map(crate::input::SourceMark::to_context),
                );
                return self.cs.lookup(b"relax").unwrap_or(0);
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
        self.push_group_level(LevelType::Simple);
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
            Some(LevelType::Group | LevelType::MathGroup) => {
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
            Some(LevelType::MathLeft) => {
                self.error("Extra }, or forgotten \\right");
            }
            Some(LevelType::MathShift) => {
                self.error("Extra }, or forgotten $");
            }
            _ => self.error("Too many }'s"),
        }
    }

    /// tex.web semi_simple_group: \\begingroup/\\endgroup save-stack only
    pub fn begin_semi_simple(&mut self) {
        self.ss_trace.push(format!(
            "{}:{}",
            self.input
                .current_file_name()
                .split('/')
                .last()
                .unwrap_or("?"),
            self.input.current_file_line()
        ));
        self.push_group_level(crate::eqtb::LevelType::SemiSimple);
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
                    self.error("Too many \\endgroups");
                } else {
                    // mismatch: still pop to keep the save stack moving
                    let _ = self.pop_group();
                }
            }
        }
    }
}

#[cfg(test)]
mod definable_cs_recovery_tests {
    use super::*;
    use crate::engine::InteractionMode;

    fn engine() -> Engine {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.set_interaction_mode(InteractionMode::Nonstop);
        engine
    }

    #[test]
    fn malformed_definition_recovery_is_isolated_between_engines() {
        let mut first = engine();
        first
            .input
            .push_toks(vec![Token::letter(b'x'); 8], "first definitions");
        for _ in 0..8 {
            first.scan_definable_cs();
        }
        assert_eq!(first.definable_cs_recovery_count, 8);

        let mut second = engine();
        second.input.push_toks(
            vec![Token::letter(b'a'), Token::letter(b'b')],
            "second definitions",
        );
        second.scan_definable_cs();

        assert_eq!(second.definable_cs_recovery_count, 1);
        assert_eq!(second.input.stack.len(), 1);
        assert_eq!(second.raw_token(), Token::letter(b'b'));
    }

    #[test]
    fn ninth_bad_target_discards_only_the_current_engine_recovery_list() {
        let mut engine = engine();
        engine
            .input
            .push_toks(vec![Token::letter(b'x'); 10], "bad definitions");

        for _ in 0..8 {
            engine.scan_definable_cs();
        }
        assert_eq!(engine.definable_cs_recovery_count, 8);
        assert_eq!(engine.input.stack.len(), 1);

        engine.scan_definable_cs();
        assert_eq!(engine.definable_cs_recovery_count, 0);
        assert!(engine.input.stack.is_empty());

        engine
            .input
            .push_toks(vec![Token::letter(b'y'); 2], "later definitions");
        engine.scan_definable_cs();
        assert_eq!(engine.definable_cs_recovery_count, 1);
        assert_eq!(engine.input.stack.len(), 1);
    }

    #[test]
    fn valid_target_and_job_reset_clear_the_recovery_streak() {
        let mut engine = engine();
        let target = engine.cs.intern(b"target");
        engine.input.push_toks(
            vec![Token::letter(b'x'), Token::from_cs(target)],
            "definitions",
        );

        engine.scan_definable_cs();
        assert_eq!(engine.definable_cs_recovery_count, 1);
        assert_eq!(engine.scan_definable_cs(), target);
        assert_eq!(engine.definable_cs_recovery_count, 0);

        engine
            .input
            .push_toks(vec![Token::letter(b'y')], "next definition");
        engine.scan_definable_cs();
        assert_eq!(engine.definable_cs_recovery_count, 1);
        engine.reset_job_diagnostics();
        assert_eq!(engine.definable_cs_recovery_count, 0);
    }

    #[test]
    fn let_macro_parameter_recognized_in_definition_parameters_and_body() {
        // tex.web §470 / §477: a control sequence \let to a catcode-6 character
        // token (like pb-diagram's `\let\@tempa=##`) acts as a macro parameter
        // token in both parameter text and replacement text.
        let mut eng = engine();
        let hash_tok = Token::char(6, b'#' as u32);
        let myhash = eng.cs.intern(b"myhash");
        eng.eqtb.assign(myhash, crate::eqtb::Equiv::CharTok(hash_tok.0), false);
        let mymacro = eng.cs.intern(b"mymacro");
        eng.input.push_toks(
            vec![
                Token::from_cs(mymacro),
                Token::other(b'['),
                Token::from_cs(myhash),
                Token::other(b'1'),
                Token::other(b']'),
                Token::char(1, b'{' as u32),
                Token::from_cs(myhash),
                Token::other(b'1'),
                Token::char(2, b'}' as u32),
            ],
            "def-stream",
        );
        eng.do_def(crate::prim::Prim::Def, 0);
        assert_eq!(eng.error_count, 0);
        if let Some(crate::eqtb::Equiv::Macro(m)) = eng.eqtb.get(mymacro) {
            assert_eq!(m.num_params, 1);
            assert!(m.has_param_refs);
        } else {
            panic!("mymacro was not defined as a Macro");
        }
    }

    #[test]
    fn toks_register_copy_from_another_toks_register() {
        let mut eng = Engine::new(false);
        eng.init_primitives();
        eng.add_nullfont();
        eng.set_interaction_mode(crate::engine::InteractionMode::Nonstop);
        eng.input.push_file(
            "test-toks.tex".to_string(),
            b"\\toks1={hello world}\\toks0=\\toks1\\end".to_vec(),
        );
        eng.run();
        assert_eq!(eng.error_count, 0, "{}", eng.diagnostic_output);
        assert_eq!(eng.eqtb.toks[0], eng.eqtb.toks[1]);
        assert_eq!(eng.eqtb.toks[0].len(), 11);
    }
}
