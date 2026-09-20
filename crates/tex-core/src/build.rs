//! List building: characters, glue, kerns, penalties, rules, box groups,
//! paragraphs, page-builder hook.

use crate::boxes::{self, Glue, Node, NodeList};
use crate::engine::{Engine, Mode};
use crate::eqtb::LevelType;
use crate::fontiface::LigKernStep;
use crate::prim::{DimParam, GlueParam, IntParam, Prim};
use crate::scaled::ONE;
use crate::token::{CsId, Token};

pub const RULE_FILL: i32 = i32::MIN; // sentinel: rule dimension from context

impl Engine {
    pub fn font_resolver(&self) -> &dyn crate::fonts::FontResolver {
        self
    }

    // ---------- characters & spaces ----------

    pub fn hspace_token(&mut self) {
        self.flush_native_text();
        let f = self.eqtb.cur_font_val;
        if f != 0 && matches!(self.mode, Mode::Horizontal | Mode::RestrictedHorizontal) {
            if self.mode == Mode::Horizontal {
                self.flush_hyphen_disc(f);
            }
            self.flush_right_boundary_kern(f);
        }
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                let g = self.interword_glue();

                self.cur_list.push(Node::Glue(g));
            }
            Mode::Vertical | Mode::InternalVertical => {
                // spaces are ignored in vertical mode
            }
            Mode::Math | Mode::DisplayMath => {
                // space tokens ignored in math
            }
        }
    }

    pub fn interword_glue(&mut self) -> Glue {
        let f = self.eqtb.cur_font_val;

        // tex.web app_space (§1057): a nonzero \xspaceskip is used unchanged
        // only for sentence spaces. Otherwise copy \spaceskip when set, or
        // the current font's space glue, then apply the space factor to that
        // copy. Font dimensions come through the mutable \fontdimen overlay.
        let sf = self.space_factor.max(1) as i64;
        let xs = &self.eqtb.glue_params[GlueParam::XSpaceSkip.idx() as usize];
        if sf >= 2000 && (xs.width != 0 || xs.stretch != 0 || xs.shrink != 0) {
            return xs.clone();
        }

        let fp = self.eqtb.font_params.get(f as usize);
        let fd = |i: usize| -> Option<i32> { fp.and_then(|v| v.get(i).copied()) };
        let ss = &self.eqtb.glue_params[GlueParam::SpaceSkip.idx() as usize];
        let mut g = if ss.width != 0 || ss.stretch != 0 || ss.shrink != 0 {
            ss.clone()
        } else if let Some(font) = self.eqtb.fonts.get(f as usize) {
            Glue {
                width: fd(1).unwrap_or_else(|| font.space()),
                stretch: fd(2).unwrap_or_else(|| font.space_stretch()),
                shrink: fd(3).unwrap_or_else(|| font.space_shrink()),
                stretch_order: 0,
                shrink_order: 0,
            }
        } else {
            Glue::zero()
        };
        if sf >= 2000 {
            if let Some(font) = self.eqtb.fonts.get(f as usize) {
                g.width += fd(6).unwrap_or_else(|| font.extra_space());
            }
        }
        g.stretch = crate::scaled::xn_over_d(g.stretch, sf as i32, 1000);
        g.shrink = crate::scaled::xn_over_d(g.shrink, 1000, sf as i32);
        g
    }

    /// tex.web ex_space (control space `\ `): append_normal_space — plain
    /// interword glue of the current font (or \spaceskip as-is when set),
    /// WITHOUT space-factor scaling of stretch/shrink and without
    /// \fontdimen7 extra space. \spacefactor is left unchanged.
    pub fn ex_space(&mut self) {
        self.flush_native_text();
        let f = self.eqtb.cur_font_val;
        if f != 0 && matches!(self.mode, Mode::Horizontal | Mode::RestrictedHorizontal) {
            if self.mode == Mode::Horizontal {
                self.flush_hyphen_disc(f);
            }
            self.flush_right_boundary_kern(f);
        }
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                let ss = self.eqtb.glue_params[GlueParam::SpaceSkip.idx() as usize].clone();
                let g = if ss.width != 0 || ss.stretch != 0 || ss.shrink != 0 {
                    ss
                } else {
                    let f = self.eqtb.cur_font_val;
                    let fp = self.eqtb.font_params.get(f as usize);
                    let fd = |i: usize| -> Option<i32> { fp.and_then(|v| v.get(i).copied()) };
                    match self.eqtb.fonts.get(f as usize) {
                        Some(font) => Glue {
                            width: fd(1).unwrap_or_else(|| font.space()),
                            stretch: fd(2).unwrap_or_else(|| font.space_stretch()),
                            shrink: fd(3).unwrap_or_else(|| font.space_shrink()),
                            stretch_order: 0,
                            shrink_order: 0,
                        },
                        None => Glue::zero(),
                    }
                };
                self.cur_list.push(Node::Glue(g));
            }
            Mode::Math | Mode::DisplayMath => {
                // tex.web mmode+ex_space: goto append_normal_space — a plain
                // space glue lands on the math list.
                let f = self.eqtb.cur_font_val;
                if let Some(font) = self.eqtb.fonts.get(f as usize) {
                    let g = Glue {
                        width: font.space(),
                        stretch: font.space_stretch(),
                        shrink: font.space_shrink(),
                        stretch_order: 0,
                        shrink_order: 0,
                    };
                    self.append_mlist_node(Node::Glue(g));
                }
            }
            Mode::Vertical | Mode::InternalVertical => {}
        }
    }

    pub fn char_token(&mut self, c: u8, is_letter: bool) {
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                if !self.append_native_char(c as u32) {
                    self.append_char(c);
                }
                self.space_factor = self.space_factor_of(c);
            }
            Mode::Vertical | Mode::InternalVertical => {
                // LaTeX \\everypar (\\g__para_standard_everypar_tl) runs
                // \\tex_par:D to cancel that dummy paragraph; the letter
                // must not already be on the list or it becomes its own para.
                let cc = if is_letter { 11 } else { 12 };

                self.push_token(Token::char(cc, c as u32));
                self.start_paragraph(true);
            }
            Mode::Math | Mode::DisplayMath => {
                let mc = self.eqtb.math_code[c as usize];
                if mc & 0x8000 != 0 {
                    self.active_char(c);
                } else {
                    self.append_mathchar(mc);
                }
            }
        }
    }

    /// tex.web adjust_space_factor (§20124): sf_code=0 leaves the factor
    /// unchanged; 1000 forces 1000; below 1000 (e.g. 999 uppercase) sets
    /// directly; above 1000 (sentence punctuation) sets the code — but never
    /// crosses 1000 upward: after an uppercase letter (sf=999) a period
    /// yields sf=1000, i.e. NO sentence boost after capitals.
    pub fn space_factor_of(&self, c: u8) -> i32 {
        let main_s = self.eqtb.sf_code[c as usize] as i32;
        if main_s == 1000 {
            1000
        } else if main_s < 1000 {
            if main_s > 0 {
                main_s
            } else {
                self.space_factor
            }
        } else if self.space_factor < 1000 {
            1000
        } else {
            main_s
        }
    }

    pub fn active_char(&mut self, c: u8) {
        // tex.web: an active character is a control sequence whose entry
        // lives in the active region — look it up there, not in the hash
        // (where the control symbol of the same character lives).
        let id = self.active_cs_id(c);
        match self.eqtb.get(id).cloned() {
            Some(crate::eqtb::Equiv::Macro(m)) => self.expand_macro(id, &m, id),
            Some(crate::eqtb::Equiv::Prim(p)) => {
                if self.is_expandable(p) {
                    let _ = self.expand_prim_dispatch(p, id);
                } else {
                    self.dispatch_cs(p, id);
                }
            }
            None => {
                self.error(&format!("Undefined active character `{}'", c as char));
            }
            _ => {}
        }
    }

    fn expand_prim_dispatch(&mut self, p: Prim, id: crate::token::CsId) -> Option<Token> {
        // reuse expand_prim via a small shim
        let saved = std::mem::take(&mut self.pushed);
        self.pushed = saved;
        // public wrapper:
        self.expand_prim_pub(p, id)
    }

    pub fn super_token(&mut self, c: u8) {
        if self.mode.is_m() {
            self.append_script(true, c);
        } else {
            self.error("Missing $ inserted (superscript)");
        }
    }

    pub fn sub_token(&mut self, c: u8) {
        if self.mode.is_m() {
            self.append_script(false, c);
        } else {
            self.error("Missing $ inserted (subscript)");
        }
    }

    // ---------- glue / kern / penalty appends ----------

    pub fn scan_hskip_kind(&mut self, p: Prim) -> Glue {
        match p {
            Prim::HSkip => {
                self.scan_optional_equals();
                self.scan_glue(false)
            }
            // tex.web: \hfil = 0pt plus 1fil; \hss = 0pt plus 1fil minus 1fil
            Prim::HFil => Glue::fil(1, 0),
            Prim::HFill => Glue::fil(2, 0),
            Prim::HFilL => Glue::fil(3, 0),
            Prim::HFilNeg => Glue {
                width: 0,
                stretch: -ONE,
                shrink: 0,
                stretch_order: 1,
                shrink_order: 0,
            },
            Prim::HSS => Glue {
                width: 0,
                stretch: ONE,
                shrink: ONE,
                stretch_order: 1,
                shrink_order: 1,
            },
            _ => Glue::zero(),
        }
    }

    pub fn scan_vskip_kind(&mut self, p: Prim) -> Glue {
        match p {
            Prim::VSkip => {
                self.scan_optional_equals();
                self.scan_glue(false)
            }
            // \vfil/\vfill/\vss mirror the horizontal definitions
            Prim::VFil => Glue::fil(1, 0),
            Prim::VFill => Glue::fil(2, 0),
            Prim::VFilL => Glue::fil(3, 0),
            Prim::VFilNeg => Glue {
                width: 0,
                stretch: -ONE,
                shrink: 0,
                stretch_order: 1,
                shrink_order: 0,
            },
            Prim::VSS => Glue {
                width: 0,
                stretch: ONE,
                shrink: ONE,
                stretch_order: 1,
                shrink_order: 1,
            },
            _ => Glue::zero(),
        }
    }

    pub fn append_h_glue(&mut self, g: Glue, leader: bool) {
        self.flush_native_text();
        let _ = leader;
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                self.cur_list.push(Node::Glue(g));
                self.space_factor = 1000;
            }
            Mode::Vertical | Mode::InternalVertical => {
                self.start_paragraph(false);
                self.cur_list.push(Node::Glue(g));
            }
            Mode::Math | Mode::DisplayMath => {
                self.append_mlist_node(Node::Glue(g));
            }
        }
    }

    pub fn append_v_glue(&mut self, g: Glue) {
        match self.mode {
            Mode::Vertical | Mode::InternalVertical => {
                self.vlist_append(Node::Glue(g));
            }
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                self.error("You can't use \\vskip in horizontal mode");
            }
            Mode::Math | Mode::DisplayMath => {
                self.append_mlist_node(Node::Glue(g));
            }
        }
    }

    pub fn append_h_kern(&mut self, d: i32) {
        self.flush_native_text();
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                self.cur_list.push(Node::Kern(d));
            }
            _ => self.error("Bad \\kern context"),
        }
    }

    pub fn append_v_kern(&mut self, d: i32) {
        match self.mode {
            Mode::Vertical | Mode::InternalVertical => {
                self.vlist_append(Node::Kern(d));
            }
            _ => self.error("Bad \\kern context"),
        }
    }

    pub fn append_h_penalty(&mut self, n: i32) {
        self.flush_native_text();
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                self.cur_list.push(Node::Penalty(n));
            }
            _ => self.error("You can't use \\penalty in this mode"),
        }
    }

    pub fn append_v_penalty(&mut self, n: i32) {
        match self.mode {
            Mode::Vertical | Mode::InternalVertical => {
                self.vlist_append(Node::Penalty(n));
            }
            Mode::Math | Mode::DisplayMath => {
                self.append_mlist_node(Node::Penalty(n));
            }
            _ => self.error("You can't use \\penalty in this mode"),
        }
    }

    /// Append a node to the current vertical list. In outer vertical mode
    /// this is the contribution list (`page_list`). During an output routine
    /// tex.web runs the routine in INTERNAL vertical mode on a fresh list
    /// (`push_nest; mode:=-vmode`, §28635), which Rust keeps in `cur_list`;
    /// <Resume the page builder> (§28652-28661) splices that list into the
    /// contribution list at finish_output, so appends must not touch
    /// `page_list` while `output_tail` is active.
    pub fn page_append(&mut self, n: Node) {
        if self.mode == Mode::Vertical {
            self.page_list.push(n);
        } else {
            self.cur_list.push(n);
        }
    }
    /// appends to the current vertical list; at outer level the page builder
    /// runs only for box-like appends — tex.web triggers build_page on box
    /// appends / paragraph ends, NOT on \vskip/\penalty, so glue and penalty
    /// nodes stay visible to \lastskip/\lastpenalty until the next box
    /// (LaTeX's \addpenalty/\@xaddvskip compensation dances depend on this)
    pub fn vlist_append(&mut self, n: Node) {
        self.vlist_append_il(n, true);
    }
    pub fn vlist_append_il(&mut self, n: Node, interline: bool) {
        if self.mode == Mode::Vertical {
            // tex.web append_to_vlist (§21374): interline glue is
            // materialized AT APPEND TIME against prev_depth with the
            // CURRENT \baselineskip — deferring it to the page builder
            // reads whatever font-size state is active when the page
            // fires (e.g. post-\endgroup 1.5-spacing into a singlespaced
            // bibliography)
            // tex.web §19460: in vmode, build_page is triggered by boxes,
            // rules, insertions, and penalties (append_penalty §21251).
            // Glue and kern (append_glue §20596, append_kern §20615) simply
            // tail_append to the contribution list WITHOUT triggering build_page,
            // allowing subsequent macros (\addvspace, \@xaddvskip, \delete_last)
            // to inspect \lastskip or adjust the contribution list before the
            // next box contributes.
            let trigger = matches!(
                n,
                Node::Box { .. } | Node::Rule { .. } | Node::Ins { .. } | Node::Penalty(_)
            );
            match &n {
                Node::Box { h, d, .. } => {
                    if interline {
                        const IGNORE: i32 = -1000 * 65536;
                        if self.prev_depth > IGNORE {
                            let bs = self.eqtb.glue_params[GlueParam::BaselineSkip.idx() as usize]
                                .clone();
                            let ls =
                                self.eqtb.glue_params[GlueParam::LineSkip.idx() as usize].clone();
                            let lsl = self.eqtb.dim_params[DimParam::LineSkipLimit.idx() as usize];
                            let b = bs.width as i64 - self.prev_depth as i64 - *h as i64;
                            let glue = if b < lsl as i64 {
                                ls
                            } else {
                                Glue {
                                    width: b as i32,
                                    ..bs
                                }
                            };

                            // tex.web §21376: append_to_vlist materializes the
                            // interline glue UNCONDITIONALLY — a 0pt glue is
                            // still a node on the page list, and build_page
                            // treats a glue after a non-discardable node as a
                            // legal breakpoint (suppressing zero glue here
                            self.page_append(Node::Glue(glue));
                        }
                    }
                    self.prev_depth = *d;
                }
                Node::Rule { .. } => {
                    // tex.web §1067/§1068: hrule in vmode does NOT get interline
                    // glue, and sets prev_depth := ignore_depth
                    const IGNORE: i32 = -1000 * 65536;
                    self.prev_depth = IGNORE;
                }
                _ => {}
            }
            self.page_append(n);
            if trigger {
                self.build_page();
            }
        } else {
            self.cur_list.push(n);
        }
    }

    pub fn append_whatsit(&mut self, n: Node) {
        self.flush_native_text();
        match self.mode {
            Mode::Vertical | Mode::InternalVertical => self.vlist_append(n),
            Mode::Math | Mode::DisplayMath => self.append_mlist_node(n),
            _ => self.cur_list.push(n),
        }
    }
    // ---------- characters with ligatures & kerns ----------

    /// Return whether a font contains a character and, when requested by
    /// `\\tracinglostchars`, emit the same event as a located warning.  A
    /// missing glyph is recoverable: TeX omits it rather than failing the job.
    pub(crate) fn font_has_character_or_warn(
        &mut self,
        font_id: u16,
        character: u8,
        source: Option<crate::input::SourceContext>,
    ) -> bool {
        let Some(font) = self.eqtb.fonts.get(font_id as usize) else {
            return false;
        };
        if font.char_present(character) {
            return true;
        }
        if self.eqtb.int_params[crate::prim::IntParam::TracingLostChars.idx() as usize] > 0 {
            let font_name = if font.tfm_name.is_empty() {
                format!("font {font_id}")
            } else {
                font.tfm_name.clone()
            };
            self.warning_at(
                &format!(
                    "Character code {character} (0x{character:02X}) is not available in font `{font_name}`; character omitted"
                ),
                source,
            );
        }
        false
    }

    pub fn append_char(&mut self, c: u8) {
        let f = self.eqtb.cur_font_val;
        let present = self
            .eqtb
            .fonts
            .get(f as usize)
            .is_some_and(|font| font.char_present(c));
        if !present {
            // Materializing a source excerpt scans and decodes the physical
            // line. Keep that work on the exceptional missing-glyph path;
            // ordinary text can contain millions of characters.
            let source =
                (self.eqtb.int_params[crate::prim::IntParam::TracingLostChars.idx() as usize] > 0)
                    .then(|| {
                        self.current_token_source_mark()
                            .map(|mark| mark.to_context())
                    })
                    .flatten();
            let _ = self.font_has_character_or_warn(f, c, source);
            return;
        }
        // tex.web main_loop wrapup: a null discretionary rides AFTER an
        // output hyphen char — but only once the next token is known not to
        // ligature with it ("--" forms the en-dash first). flush points:
        // here (next char), glue/space appends, and end_paragraph.
        let tail_charish = matches!(
            self.cur_list.last(),
            Some(Node::Char { font: pf, .. } | Node::Ligature { font: pf, .. }) if *pf == f
        );
        if !tail_charish {
            self.flush_hyphen_disc(f);
        }
        self.append_char_lig(c, f);
    }

    /// tex.web wrapup: when the last output char is the font's hyphen char
    /// (possibly inside a just-formed ligature like the en-dash), a null
    /// discretionary follows it — the legal break after an explicit hyphen.
    /// The disc is appended only when the hyphen settles (next token does
    /// not extend the ligature chain).
    fn tail_ends_hyphen(&self, f: u16) -> bool {
        let hc = self.eqtb.hyphen_char.get(f as usize).copied().unwrap_or(-1);
        if !(0..=255).contains(&hc) {
            return false;
        }
        match self.cur_list.last() {
            Some(Node::Char { c: pc, font: pf }) => *pf == f && *pc as i32 == hc,
            Some(Node::Ligature {
                font: pf,
                letters,
                n_letters,
                ..
            }) => *pf == f && *n_letters > 0 && letters[*n_letters as usize - 1] as i32 == hc,
            _ => false,
        }
    }

    pub(crate) fn flush_hyphen_disc(&mut self, f: u16) {
        if self.tail_ends_hyphen(f) {
            self.cur_list.push(Node::Disc(crate::boxes::DiscNode {
                pre_break: Vec::new(),
                post_break: Vec::new(),
                no_break: Vec::new(),
                replace_count: 0,
            }));
        }
    }

    /// Implement \noboundary primitive logic:
    /// skips current right boundary and suppresses next implicit left boundary / starts new ligature chain.
    pub fn no_boundary(&mut self) {
        self.flush_native_text();
        self.native_text.suppress_right_boundary = true;
        self.native_text.suppress_left_boundary = true;
        self.native_text.no_lig_prev = true;
    }

    pub(crate) fn find_left_boundary_step(&self, f: u16, next: u8) -> Option<LigKernStep> {
        let font = self.eqtb.fonts.get(f as usize)?;
        let first = font.lig_kern.first()?;
        if first.skip != 255 {
            return None;
        }
        let mut k = 256 * (first.op as usize) + (first.rem as usize);
        if k >= font.lig_kern.len() {
            return None;
        }
        let mut jumps = 0;
        loop {
            if k >= font.lig_kern.len() || jumps > 128 {
                return None;
            }
            let step = &font.lig_kern[k];
            if step.next_char == next {
                if step.op >= 128 {
                    let idx = ((step.op as usize) - 128) * 256 + step.rem as usize;
                    let amt = font.kerns.get(idx).copied().unwrap_or(0);
                    return Some(LigKernStep {
                        is_kern: true,
                        kern_amount: amt,
                        lig_char: 0,
                        keep_left: false,
                        keep_right: false,
                        iterate: false,
                    });
                } else {
                    let lig = step.rem;
                    let keep_left = (step.op & 1) != 0;
                    let keep_right = (step.op & 2) != 0;
                    let iterate = (step.op & 4) != 0;
                    return Some(LigKernStep {
                        is_kern: false,
                        kern_amount: 0,
                        lig_char: lig,
                        keep_left,
                        keep_right,
                        iterate,
                    });
                }
            }
            if step.stop {
                return None;
            }
            k += (step.skip as usize) + 1;
            jumps += 1;
        }
    }

    /// tex.web §20237–§20238: when leaving the character loop, TeX checks
    /// if the last character/ligature has a lig/kern step with the font's
    /// right boundary character (font_bchar), and if so appends the kern or ligature.
    pub(crate) fn flush_right_boundary_kern(&mut self, f: u16) {
        if self.native_text.suppress_right_boundary {
            self.native_text.suppress_right_boundary = false;
            self.native_text.suppress_left_boundary = false;
            self.native_text.no_lig_prev = false;
            return;
        }
        let Some(font) = self.eqtb.fonts.get(f as usize) else {
            return;
        };
        let Some(bchar) = font.bchar else {
            return;
        };
        let last_char = match self.cur_list.last() {
            Some(Node::Char { c, font: pf }) if *pf == f => Some(*c),
            Some(Node::Ligature { c, font: pf, .. }) if *pf == f => Some(*c),
            _ => None,
        };
        if let Some(c) = last_char {
            if let Some(step) = self.find_lig_kern(f, c, bchar) {
                if step.is_kern {
                    if step.kern_amount != 0 {
                        self.cur_list.push(Node::Kern(step.kern_amount));
                    }
                } else {
                    let _ = self.cur_list.pop();
                    let lc = step.lig_char;
                    self.cur_list.push(Node::Char { c: lc, font: f });
                    if let Some(step2) = self.find_lig_kern(f, lc, bchar) {
                        if step2.is_kern && step2.kern_amount != 0 {
                            self.cur_list.push(Node::Kern(step2.kern_amount));
                        }
                    }
                }
            }
        }
    }
    fn append_char_lig(&mut self, c: u8, f: u16) {
        let suppress_lb = self.native_text.suppress_left_boundary;
        self.native_text.suppress_left_boundary = false;
        let no_lig = self.native_text.no_lig_prev;
        self.native_text.no_lig_prev = false;

        if no_lig {
            self.flush_hyphen_disc(f);
            self.cur_list.push(Node::Char { c, font: f });
            return;
        }

        // ligature & kern with previous char (either Char or an already-formed Ligature)
        let prev: Option<(u8, [u8; 3], u8)> = match self.cur_list.last() {
            Some(Node::Char { c: pc, font: pf }) if *pf == f => Some((*pc, [0; 3], 0)),
            Some(Node::Ligature {
                c: lc,
                font: pf,
                letters,
                n_letters,
                ..
            }) if *pf == f => Some((*lc, *letters, *n_letters)),
            _ => None,
        };

        if prev.is_none() && !suppress_lb {
            if let Some(step) = self.find_left_boundary_step(f, c) {
                if step.is_kern {
                    if step.kern_amount != 0 {
                        self.cur_list.push(Node::Kern(step.kern_amount));
                    }
                } else {
                    let lc = step.lig_char;
                    if step.keep_right {
                        self.cur_list.push(Node::Char { c: lc, font: f });
                        self.append_char_lig(c, f);
                    } else if step.iterate {
                        self.append_char_lig(lc, f);
                    } else {
                        self.cur_list.push(Node::Char { c: lc, font: f });
                    }
                    return;
                }
            }
        }

        if let Some((pc, pletters, pn)) = prev {
            if let Some(step) = self.find_lig_kern(f, pc, c) {
                if step.is_kern {
                    if self.tail_ends_hyphen(f) {
                        self.flush_hyphen_disc(f);
                    }
                    self.cur_list.push(Node::Kern(step.kern_amount));
                    self.cur_list.push(Node::Char { c, font: f });
                    return;
                } else {
                    // ligature: replace previous char/ligature, recording the
                    // component letters (fi + l -> ffi keeps [f, i, l])
                    let ends_hyphen = self.tail_ends_hyphen(f);
                    let _ = self.cur_list.pop();
                    let lc = step.lig_char;
                    let dims = self.char_dims(f, lc);
                    let mut letters = [0u8; 3];
                    let base: &[u8] = if pn > 0 {
                        &pletters[..pn as usize]
                    } else {
                        &[pc]
                    };
                    let n = (base.len() + 1).min(3);
                    letters[..base.len().min(3)].copy_from_slice(&base[..base.len().min(3)]);
                    if base.len() < 3 {
                        letters[base.len()] = c;
                    }
                    self.cur_list.push(Node::Ligature {
                        c: lc,
                        font: f,
                        lig_width: dims.0,
                        lig_height: dims.1,
                        lig_depth: dims.2,
                        letters,
                        n_letters: n as u8,
                    });
                    let _ = ends_hyphen;
                    if step.keep_right {
                        if step.iterate {
                            self.append_char_lig(c, f);
                        } else {
                            self.cur_list.push(Node::Char { c, font: f });
                        }
                    } else if step.iterate {
                        self.append_char_lig(c, f);
                    }
                    return;
                }
            }
        }
        self.flush_hyphen_disc(f);
        self.cur_list.push(Node::Char { c, font: f });
    }

    pub fn char_dims(&self, f: u16, c: u8) -> (i32, i32, i32) {
        match self.eqtb.fonts.get(f as usize) {
            Some(font) => (font.char_width(c), font.char_height(c), font.char_depth(c)),
            None => (0, 0, 0),
        }
    }

    /// find the lig/kern step for (prev, next); None if none
    fn find_lig_kern(&self, f: u16, prev: u8, next: u8) -> Option<LigKernStep> {
        let font = self.eqtb.fonts.get(f as usize)?;
        if !font.exists_char(prev) {
            return None;
        }
        let ci = &font.chars[prev as usize];
        if ci.tag != crate::tfm::TAG_LIG {
            return None;
        }
        let mut k = ci.remainder as usize;
        let mut jumps = 0;
        // tex.web §10618: if the very first instruction of a character's
        // lig/kern program has skip_byte > 128, the program actually begins
        // at 256*op_byte + rem_byte (large-program indirection).
        if let Some(first) = font.lig_kern.get(k) {
            if first.skip > 128 {
                k = 256 * first.op as usize + first.rem as usize;
            }
        }
        loop {
            if k >= font.lig_kern.len() || jumps > 128 {
                return None;
            }
            let step = &font.lig_kern[k];
            if step.next_char == 0xFF && false {
                // boundary char handling omitted
            }
            if step.next_char == next {
                if step.op >= 128 {
                    let idx = ((step.op as usize) - 128) * 256 + step.rem as usize;
                    let amt = font.kerns.get(idx).copied().unwrap_or(0);
                    return Some(LigKernStep {
                        is_kern: true,
                        kern_amount: amt,
                        lig_char: 0,
                        keep_left: false,
                        keep_right: false,
                        iterate: false,
                    });
                } else {
                    return Some(LigKernStep {
                        is_kern: false,
                        kern_amount: 0,
                        lig_char: step.rem,
                        keep_left: step.op & 2 != 0,
                        keep_right: step.op & 1 != 0,
                        iterate: step.op & 4 != 0,
                    });
                }
            }
            if step.stop {
                return None;
            }
            k += 1 + (step.skip as usize);
            jumps += 1;
        }
    }

    // ---------- rules ----------

    /// scan `width|height|depth <dimen>` keywords (tex.web scan_rule_spec).
    /// Null dimensions stay as RULE_FILL sentinels exactly like TeX: they
    /// contribute nothing to packing (max/highly-negative) and are resolved
    /// only at shipout. \hrule defaults height 0.4pt depth 0, width fills
    /// to \hsize; \vrule defaults width 0.4pt, height/depth to context.
    pub fn scan_rule_dims(&mut self, horizontal: bool) -> (i32, i32, i32) {
        let (mut width, mut height, mut depth) = (RULE_FILL, RULE_FILL, RULE_FILL);
        if horizontal {
            height = crate::scaled::ONE * 2 / 5;
            depth = 0;
        } else {
            width = crate::scaled::ONE * 2 / 5;
        }
        loop {
            if self.scan_keyword(b"width") {
                self.scan_optional_equals();
                width = self.scan_dimen(false, false);
            } else if self.scan_keyword(b"height") {
                self.scan_optional_equals();
                height = self.scan_dimen(false, false);
            } else if self.scan_keyword(b"depth") {
                self.scan_optional_equals();
                depth = self.scan_dimen(false, false);
            } else {
                break;
            }
        }
        (width, height, depth)
    }

    pub fn make_rule(&mut self, horizontal: bool) {
        let (mut width, height, depth) = self.scan_rule_dims(horizontal);
        if horizontal {
            // tex.web: an \hrule with null width keeps a RUNNING width in the
            // node; vpackage resolves it to the enclosing box's width
            // (§13468). Baking \hsize in here made \noalign{\hrule} blocks
            // (booktabs) blow the alignment up to full text width.
            self.prev_depth = -1000 * 65536;
            self.vlist_append(Node::Rule {
                width,
                height,
                depth,
            });
            return;
        }
        if width == RULE_FILL {
            width = crate::scaled::ONE * 2 / 5;
        }
        let node = Node::Rule {
            width,
            height,
            depth,
        };
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                if horizontal && self.mode == Mode::Horizontal {
                    self.end_paragraph();
                    self.vlist_append(node);
                } else {
                    self.cur_list.push(node);
                }
            }
            Mode::Vertical => {
                if horizontal {
                    self.vlist_append(node);
                } else {
                    self.start_paragraph(true);
                    self.cur_list.push(node);
                }
            }
            Mode::InternalVertical => {
                if horizontal {
                    self.vlist_append(node);
                } else {
                    self.cur_list.push(node);
                }
            }
            Mode::Math | Mode::DisplayMath => self.append_mlist_node(node),
        }
    }

    // ---------- boxes ----------

    pub(crate) fn token_is_left_brace(&self, t: Token) -> bool {
        if t.is_char() && t.cc() == 1 {
            return true;
        }
        if t.is_cs() {
            match self.eqtb.resolve(t.cs_id()) {
                Some(crate::eqtb::Equiv::Prim(Prim::BGroup)) => return true,
                Some(crate::eqtb::Equiv::CharTok(v)) if Token(*v).cc() == 1 => return true,
                _ => {}
            }
        }
        false
    }

    /// \hbox to 10pt{...} etc: scan spec, push group context
    pub fn begin_box(&mut self, kind: u8) {
        self.flush_native_text();
        // (character keywords), not control sequences)
        let mut target: Option<(i32, bool)> = None; // (dim, is_spread)
        if self.scan_keyword(b"to") {
            let d = self.scan_dimen(false, false);
            target = Some((d, false));
        } else if self.scan_keyword(b"spread") {
            let d = self.scan_dimen(false, false);
            target = Some((d, true));
        }
        // consume the opening `{` (tex.web scan_left_brace): the box group
        // pushed below is the group; a dispatch-level plain group would
        // desynchronize box_kinds from braces.
        self.skip_spaces_relax();
        let t = self.get_x_raw();
        if !self.token_is_left_brace(t) {
            let got = if t.is_cs() {
                self.display_cs(t.cs_id())
            } else {
                format!("cc{}:{:#x}", t.cc(), t.chr())
            };
            self.error(&format!("Missing {{ inserted (got {})", got));
            self.push_token(t);
            self.push_token(Token::char(1, b'{' as u32));
        }
        let shift = match self.pending_box_shift.take() {
            Some((d, _)) => d,
            None => 0,
        };

        // save the outer list context: mode, current list, prev_depth, space_factor
        self.saved_lists.push((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
        ));
        self.prev_graf = 0;
        self.push_group_level(LevelType::Box);

        self.box_targets.push(target);
        self.box_shifts.push(shift);
        self.box_kinds.push(kind);
        match kind {
            0 => {
                self.mode = Mode::RestrictedHorizontal;
                let toks = (*self.eqtb.tok_params
                    [crate::prim::ToksParam::EveryHBox.idx() as usize])
                    .clone();
                if !toks.is_empty() {
                    self.push_tokens_named(toks, "<everyhbox>");
                }
            }
            // tex.web §1083 (@21025) & §1105 (@22114): \vbox (1), \vtop (2), and \vcenter (3)
            // all push an internal vertical mode level, set prev_depth to ignore_depth,
            // and expand \everyvbox.
            1 | 2 | 3 => {
                self.normal_paragraph();
                self.mode = Mode::InternalVertical;
                self.prev_depth = -1000 * 65536;
                let toks = (*self.eqtb.tok_params
                    [crate::prim::ToksParam::EveryVBox.idx() as usize])
                    .clone();
                if !toks.is_empty() {
                    self.push_tokens_named(toks, "<everyvbox>");
                }
            }
            9 => {
                self.mode = Mode::InternalVertical;
                self.prev_depth = -1000 * 65536;
            }
            _ => self.mode = Mode::InternalVertical,
        }
    }

    /// called on the matching `}` for a box group or plain group
    pub fn end_box(&mut self) {
        self.flush_native_text();
        if self.box_kinds.is_empty() {
            self.error("Too many }'s");
            return;
        }
        let box_level = self.eqtb.cur_level;
        let pack_warning_source = self
            .diagnostic_group_openings
            .iter()
            .rev()
            .find(|(level, _)| *level == box_level)
            .map(|(_, source)| source.clone());
        let kind = self.box_kinds.pop().unwrap_or(0);
        // packed lines join the vbox instead of being vpack-discarded

        if matches!(kind, 1 | 2 | 3 | 8 | 9) && self.mode == Mode::Horizontal {
            self.par_primitive();
        }
        let target = self.box_targets.pop().flatten();
        let shift = self.box_shifts.pop().unwrap_or(0);
        let inner = std::mem::replace(&mut self.cur_list, Vec::new());
        let (outer_mode, outer_list, pd, sf, pg) = self.saved_lists.pop().unwrap_or((
            // group desync (e.g. runaway end): stay in the current context
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
        ));
        self.prev_graf = pg;
        // tex.web @21193-21194 (insert_group): `q:=split_top_skip;
        // d:=split_max_depth; f:=floating_penalty; unsave` — the insert's
        // float cost AND its split metadata are the LOCAL values captured
        // before unsave restores the outer ones. Reading them after
        // pop_group would miss settings made inside the group (LaTeX's
        // `\@footnotetext` sets `\floatingpenalty` to `\@MM` there; a
        // class's `\insert\n` can locally re-set `\splittopskip`/
        // `\splitmaxdepth` before closing).
        let (ins_float_cost, ins_split_top_skip, ins_split_max_depth) = if kind == 8 {
            (
                self.eqtb.int_params[IntParam::FloatingPenalty.idx() as usize],
                self.eqtb.glue_params[GlueParam::SplitTopSkip.idx() as usize],
                self.eqtb.dim_params[DimParam::SplitMaxDepth.idx() as usize],
            )
        } else {
            (0, Glue::zero(), 0)
        };
        // `\boxmaxdepth` is scoped to the vbox group and package() uses the
        // value that is still current before unsave (tex.web §1102).
        let box_max_depth = self.eqtb.dim_params[DimParam::BoxMaxDepth.idx() as usize];
        self.pop_group();
        self.prev_depth = pd;
        self.space_factor = sf;
        self.mode = outer_mode;
        // tex.web math_group: `{`/`}` in math mode are pure grouping — the
        // math list keeps accumulating; no box is packaged or appended.
        if matches!(kind, 5 | 6) && outer_mode.is_m() {
            self.cur_list = outer_list;
            return;
        }
        // \vadjust (kind 9): no packing — capture the material as an
        // adjustment attached to the enclosing hlist; the line breaker
        // migrates it into the vertical list after the line containing it.
        if kind == 9 {
            self.cur_list = outer_list;
            if outer_mode.is_v() {
                self.cur_list.extend(inner);
            } else if outer_mode.is_m() {
                self.append_mlist_node(Node::VAdjust(inner));
            } else {
                self.cur_list.push(Node::VAdjust(inner));
            }
            return;
        }
        // tex.web package(): vboxes are packed against the value of
        let pack = |list: Vec<Node>, target: Option<(i32, bool)>, k: u8| -> boxes::PackResult {
            let (dim, spread) = match target {
                Some((d, sp)) => (Some(d), sp),
                None => (None, false),
            };
            match k {
                0 | 6 => boxes::hpack_add(list, dim, spread, boxes::HBOX, &self.eqtb),
                1 | 5 => {
                    boxes::vpack_add_md(list, dim, spread, boxes::VBOX, &self.eqtb, box_max_depth)
                }
                2 => boxes::vtop_md(list, dim, spread, &self.eqtb, box_max_depth),
                // \vcenter and \insert pack with vpack == vpackage(l:=max_dimen):
                // tex.web @21196 `p:=vpack(link(head),natural)` — \boxmaxdepth
                // never applies to an insertion vbox
                3 | 8 => boxes::vpack_add_md(list, dim, spread, boxes::VBOX, &self.eqtb, i32::MAX),
                _ => boxes::hpack_add(list, None, false, boxes::HBOX, &self.eqtb),
            }
        };

        let res = pack(inner, target, kind);
        self.last_badness = res.badness;
        match kind {
            0 | 1 | 2 | 8 => self.report_pack_warnings_at(&res, pack_warning_source),
            _ => {}
        }
        let mut node = res.node;
        if let Node::Box { shift: s, .. } = &mut node {
            *s += shift;
        }
        if kind == 3 {
            node = Node::VCenter {
                box_node: Box::new(node),
            };
        }
        if kind == 7 {
            self.cur_list = outer_list;
            self.finish_halign();
            return;
        }
        // plain groups in vmode: restore the page list
        if kind == 5 {
            if let Some(mut page) = self.par_page_lists.pop() {
                page.push(node);
                self.page_list = page;
                self.cur_list = outer_list;
                self.mode = Mode::Vertical;
                self.build_page();
                return;
            }
        }
        self.cur_list = outer_list;
        // \insert group: wrap the packed vbox into an insert node.
        // tex.web @21196-21201: `float_cost(tail):=f` where f was the
        // floating_penalty read before unsave — never \insertpenalties
        // (that register is the page builder's running total; clobbering
        // it to 0 here loses penalties accumulated earlier on the page).
        if kind == 8 {
            if let Some(num) = self.insert_nums.pop() {
                let (h, d) = match &node {
                    Node::Box { h, d, .. } => (*h, *d),
                    _ => (0, 0),
                };
                node = Node::Ins {
                    num,
                    height: h,
                    depth: d,
                    cost: ins_float_cost,
                    split_top_skip: ins_split_top_skip,
                    split_max_depth: ins_split_max_depth,
                    box_node: Box::new(node),
                };
            }
        }
        // a leader-object box completes a \leaders group
        if matches!(kind, 0..=3) {
            let is_leader_match = self
                .leader_stack
                .last()
                .is_some_and(|&(_, depth)| depth == self.box_kinds.len());
            if is_leader_match {
                let (lk, _) = self.leader_stack.pop().unwrap();
                let body = boxes::LeaderBody::Box(Box::new(node));
                self.finish_leaders(lk, body);
                return;
            }
        }
        // only the \\shipout box itself ships (tex.web box_context);
        // inner \\hbox/\\vbox inside the page must append
        if self.shipout_pending && self.shipout_depth == self.box_kinds.len() {
            self.unpark_setbox();
            self.shipout_pending = false;
            self.shipout_depth = usize::MAX;
            self.ship_box(Some(node));
            return;
        }
        // only the group do_setbox (or do_shipout) opened consumes the
        // target: an inner \hbox inside the content must append instead
        if self.setbox_target.is_some() && self.setbox_depth == self.box_kinds.len() {
            let idx = self.setbox_target.take().unwrap();
            let g = self.setbox_global;
            self.unpark_setbox();
            self.eqtb.assign_box(idx, Some(node), g);
            return;
        }
        if let Some((d, is_hmove)) = self.pending_box_shift.take() {
            if let Node::Box { shift, .. } = &mut node {
                *shift += d;
            }
            let _ = is_hmove;
        }
        // Unlike a completed \hbox/\vbox, an insertion node does not reset
        // space_factor (tex.web insert_group appends it directly).
        if kind == 8 && matches!(self.mode, Mode::Horizontal | Mode::RestrictedHorizontal) {
            self.cur_list.push(node);
        } else {
            self.append_box_node(Some(node));
        }
    }

    pub fn append_box_node(&mut self, b: Option<Node>) {
        self.flush_native_text();
        match b {
            None => {}
            Some(node) => match self.mode {
                Mode::Horizontal | Mode::RestrictedHorizontal => {
                    self.cur_list.push(node);
                    self.space_factor = 1000;
                }
                Mode::Vertical => {
                    self.vlist_append(node);
                }
                Mode::InternalVertical => {
                    if let Node::Box { h, d, .. } = &node {
                        const IGNORE_DEPTH: i32 = -1000 * 65536;
                        if self.prev_depth > IGNORE_DEPTH {
                            let bs = self.eqtb.glue_params
                                [crate::prim::GlueParam::BaselineSkip.idx() as usize]
                                .clone();
                            let ls = self.eqtb.glue_params
                                [crate::prim::GlueParam::LineSkip.idx() as usize]
                                .clone();
                            let lsl = self.eqtb.dim_params
                                [crate::prim::DimParam::LineSkipLimit.idx() as usize];
                            let diff = bs.width as i64 - self.prev_depth as i64 - *h as i64;

                            let glue = if diff < lsl as i64 {
                                ls
                            } else {
                                Glue {
                                    width: diff as i32,
                                    ..bs
                                }
                            };
                            if glue.width != 0 || glue.stretch != 0 || glue.shrink != 0 {
                                self.cur_list.push(Node::Glue(glue));
                            }
                        }
                        self.prev_depth = *d;
                    }
                    self.cur_list.push(node);
                }
                Mode::Math | Mode::DisplayMath => {
                    self.append_mlist_node(node);
                }
            },
        }
    }

    /// \\unhbox/\\unvbox/\\unhcopy/\\unvcopy: splice a box register's list
    /// into the current list. Copy variants leave the register intact.
    pub fn do_unbox(&mut self, want_v: bool, copy: bool) {
        let n = self.scan_reg_num();
        let node = if copy {
            self.eqtb.boxed.get(n as usize).cloned().flatten()
        } else {
            // tex.web's box(n):=null consumes the value without changing its
            // eqtb level. A locally assigned box can therefore restore the
            // saved outer value when the current group closes.
            let old = self.eqtb.take_box(n);
            self.global_flag = false;
            old
        };
        let Some(node) = node else { return };
        match node {
            crate::boxes::Node::Box { kind, list, .. } => {
                let is_v = kind != crate::boxes::HBOX;
                if is_v != want_v {
                    self.error("Incompatible list can't be unboxed");
                    return;
                }
                // vertical-mode \unhbox/\unhcopy already started a paragraph
                // at dispatch (tex.web §21105); unpackage only runs in hmode
                if want_v && self.mode.is_h() {
                    self.error("Incompatible list can't be unboxed");
                    return;
                }

                // tex.web unpackage (§21327-21331): the splice is pure link
                // surgery (`link(tail):=list_ptr(p)` then advance `tail`) —
                // append_to_vlist never runs, so NO interline glue is
                // recomputed AND \prevdepth keeps its pre-splice value.
                let is_vmode = self.mode == Mode::Vertical;
                for item in list {
                    if is_vmode {
                        self.page_append(item);
                    } else {
                        self.cur_list.push(item);
                    }
                }
            }
            _ => self.error("Incompatible list can't be unboxed"),
        }
    }

    fn box_prim_or_name(&self, t: Token) -> Option<Prim> {
        if !t.is_cs() {
            return None;
        }
        if let Some(crate::eqtb::Equiv::Prim(p)) = self.eqtb.resolve(t.cs_id()) {
            if matches!(
                p,
                Prim::HBox
                    | Prim::VBox
                    | Prim::VTop
                    | Prim::VCenter
                    | Prim::Box
                    | Prim::Copy
                    | Prim::LastBox
                    | Prim::HRule
                    | Prim::VRule
            ) {
                return Some(*p);
            }
        }
        match self.cs.name(t.cs_id()) {
            b"hbox" => Some(Prim::HBox),
            b"vbox" => Some(Prim::VBox),
            b"vtop" => Some(Prim::VTop),
            b"vcenter" => Some(Prim::VCenter),
            b"box" => Some(Prim::Box),
            b"copy" => Some(Prim::Copy),
            b"lastbox" => Some(Prim::LastBox),
            b"hrule" => Some(Prim::HRule),
            b"vrule" => Some(Prim::VRule),
            _ => None,
        }
    }

    fn get_x_token_skip_spaces_relax(&mut self) -> Token {
        loop {
            let t = self.get_x_raw();
            if t == crate::input::EOF_MARKER {
                return t;
            }
            if t.is_char() && (t.cc() == 10 || t.cc() == 9) {
                continue;
            }
            if t.is_cs() {
                if let Some(crate::eqtb::Equiv::Prim(crate::prim::Prim::Relax)) =
                    self.eqtb.resolve(t.cs_id())
                {
                    continue;
                }
            }
            return t;
        }
    }

    /// append a leader node. tex.web scan_box/box_end leader context.
    pub fn begin_leaders(&mut self, kind: u8) {
        use boxes::LeaderBody;
        let depth = self.box_kinds.len();
        let t = self.get_x_token_skip_spaces_relax();
        match self.box_prim_or_name(t) {
            Some(Prim::HBox) => {
                self.leader_stack.push((kind, depth));
                self.begin_box(0);
            }
            Some(Prim::VBox) => {
                self.leader_stack.push((kind, depth));
                self.begin_box(1);
            }
            Some(Prim::VTop) => {
                self.leader_stack.push((kind, depth));
                self.begin_box(2);
            }
            Some(Prim::VCenter) => {
                self.leader_stack.push((kind, depth));
                self.begin_box(3);
            }
            Some(Prim::Box) => {
                let idx = self.scan_reg_num();
                let b = self.eqtb.boxed[idx as usize].take();
                if let Some(b) = b {
                    self.finish_leaders(kind, LeaderBody::Box(Box::new(b)));
                }
            }
            Some(Prim::Copy) => {
                let idx = self.scan_reg_num();
                let b = self.eqtb.boxed[idx as usize].clone();
                if let Some(b) = b {
                    self.finish_leaders(kind, LeaderBody::Box(Box::new(b)));
                }
            }
            Some(Prim::LastBox) => {
                if let Some(b) = self.take_last_box() {
                    self.finish_leaders(kind, LeaderBody::Box(Box::new(b)));
                }
            }
            Some(Prim::HRule) => {
                let (w, h, d) = self.scan_rule_dims(true);
                self.finish_leaders(
                    kind,
                    LeaderBody::Rule {
                        width: w,
                        height: h,
                        depth: d,
                    },
                );
            }
            Some(Prim::VRule) => {
                let (w, h, d) = self.scan_rule_dims(false);
                self.finish_leaders(
                    kind,
                    LeaderBody::Rule {
                        width: w,
                        height: h,
                        depth: d,
                    },
                );
            }
            _ => {
                self.push_token(t);
                self.error("A <box> was supposed to be here");
            }
        }
    }

    /// scan the glue spec that follows a leader object and append the
    /// leader node ("Leaders not followed by proper glue" otherwise).
    fn finish_leaders(&mut self, kind: u8, body: boxes::LeaderBody) {
        use crate::prim::Prim;
        let t = self.get_x_token_skip_spaces_relax();
        let p = if t.is_cs() {
            match self.eqtb.resolve(t.cs_id()).cloned() {
                Some(crate::eqtb::Equiv::Prim(p)) => Some(p),
                _ => None,
            }
        } else {
            None
        };
        let is_h_glue = matches!(
            p,
            Some(Prim::HSkip)
                | Some(Prim::HFil)
                | Some(Prim::HFill)
                | Some(Prim::HFilL)
                | Some(Prim::HFilNeg)
                | Some(Prim::HSS)
        );
        let is_v_glue = matches!(
            p,
            Some(Prim::VSkip)
                | Some(Prim::VFil)
                | Some(Prim::VFill)
                | Some(Prim::VFilL)
                | Some(Prim::VFilNeg)
                | Some(Prim::VSS)
        );
        let is_m_glue = matches!(p, Some(Prim::MSkip));
        let glue = if (self.mode.is_h() || self.mode.is_m()) && is_h_glue {
            self.scan_hskip_kind(p.unwrap())
        } else if self.mode.is_v() && is_v_glue {
            self.scan_vskip_kind(p.unwrap())
        } else if self.mode.is_m() && is_m_glue {
            self.scan_glue(true)
        } else {
            self.push_token(t);
            self.error("Leaders not followed by proper glue");
            return;
        };
        let node = Node::Leaders { glue, kind, body };
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                self.cur_list.push(node);
                self.space_factor = 1000;
            }
            Mode::Vertical | Mode::InternalVertical => self.vlist_append(node),
            Mode::Math | Mode::DisplayMath => self.append_mlist_node(node),
        }
    }

    // ---------- box diagnostics ----------

    /// tex.web hpack/vpackage common_ending: Overfull / Underfull / Loose /
    /// Tight reports for \hbox & friends, gated by \hfuzz|\vfuzz and
    /// \hbadness|\vbadness exactly as in tex.web §653-§663.
    /// Report packing quality at the scanner's current position. Kept as the
    /// public one-argument entry point for library callers.
    pub fn report_pack_warnings(&mut self, res: &boxes::PackResult) {
        let source = self.current_token_source_mark();
        self.report_pack_warnings_at(res, source);
    }

    fn report_pack_warnings_at(
        &mut self,
        res: &boxes::PackResult,
        source: Option<crate::input::SourceMark>,
    ) {
        self.last_pack = Some(res.record());
        let (hbox, nonempty) = match &res.node {
            Node::Box { kind, list, .. } => (*kind == boxes::HBOX, !list.is_empty()),
            _ => return,
        };
        if !nonempty {
            return;
        }
        let x = res.delta;
        let (bad_param, fuzz): (i32, i32) = if hbox {
            (
                self.eqtb.int_params[IntParam::HBadness.idx() as usize],
                self.eqtb.dim_params[DimParam::Hfuzz.idx() as usize],
            )
        } else {
            (
                self.eqtb.int_params[IntParam::VBadness.idx() as usize],
                self.eqtb.dim_params[DimParam::Vfuzz.idx() as usize],
            )
        };
        let obj = if hbox { "\\hbox" } else { "\\vbox" };
        let too = if hbox { "too wide" } else { "too high" };
        let context = if self.in_output {
            " while \\output is active"
        } else {
            ""
        };
        let mut msg: Option<String> = None;
        if x > 0 && res.order == 0 {
            // underfull / loose (includes badness 10000 when nothing stretches)
            if res.badness > bad_param {
                let kw = if res.badness > 100 {
                    "Underfull"
                } else {
                    "Loose"
                };
                msg = Some(format!("{kw} {obj} (badness {}){context}", res.badness));
            }
        } else if x < 0 && res.order == 0 {
            if -x > res.shrink[0] {
                let excess = -x - res.shrink[0];
                if excess > fuzz as i64 || bad_param < 100 {
                    msg = Some(format!(
                        "Overfull {obj} ({}pt {too}){context}",
                        print_scaled(excess),
                    ));
                }
            } else if res.badness > bad_param {
                msg = Some(format!("Tight {obj} (badness {}){context}", res.badness));
            }
        }
        if let Some(m) = msg {
            self.pack_warning_at(&m, source.map(|mark| mark.to_context()));
        }
    }

    // ---------- \wd / \ht / \dp ----------

    /// value of \wd<n> / \ht<n> / \dp<n>: which 0=width 1=height 2=depth.
    /// Void or non-box registers read as 0.
    pub fn box_reg_dimen(&self, idx: u16, which: u8) -> i32 {
        match self.eqtb.boxed.get(idx as usize).and_then(|b| b.as_ref()) {
            Some(Node::Box { w, h, d, .. }) => match which {
                0 => *w,
                1 => *h,
                _ => *d,
            },
            _ => 0,
        }
    }

    /// \wd<n>=<dimen> assignment; vanishes silently on void registers
    /// (matches tex.web set_box_dimen behavior for void boxes).
    pub fn do_box_dimen_assign(&mut self, which: u8) {
        // Box dimensions are direct node mutations, but they still consume
        // every assignment prefix even when the target register is void.
        let command = match which {
            0 => "\\wd",
            1 => "\\ht",
            _ => "\\dp",
        };
        let _ = self.take_assignment_prefixes(command);
        let idx = self.scan_reg_num();
        self.scan_optional_equals();
        let v = self.scan_dimen(false, false);
        if let Some(Node::Box { w, h, d, .. }) = self
            .eqtb
            .boxed
            .get_mut(idx as usize)
            .and_then(|o| o.as_mut())
        {
            match which {
                0 => *w = v,
                1 => *h = v,
                _ => *d = v,
            }
        }
    }
    pub fn append_take_node_opt(&mut self, n: Option<Node>) {
        if let Some(n) = n {
            self.append_take_node(n);
        }
    }

    pub fn append_take_node(&mut self, n: Node) {
        self.flush_native_text();
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => self.cur_list.push(n),
            Mode::Vertical | Mode::InternalVertical => self.vlist_append(n),
            Mode::Math | Mode::DisplayMath => self.append_mlist_node(n),
        }
    }

    pub fn box_move(&mut self, d: i32, negate: bool, horizontal: bool) {
        let d = if negate { -d } else { d };
        if horizontal {
            // \moveleft/\moveright: shift is the horizontal offset of a box
            // appended in vertical mode (tex.web: shift := box_context)
            self.pending_box_shift = Some((d, true));
            self.skip_spaces_relax();
            let t = self.get_token();
            if self.box_prim_or_name(t).is_some()
                || (t.is_cs() && self.cs.name(t.cs_id()) == b"usebox")
            {
                self.push_token(t);
                self.scan_box_after_move();
            } else {
                self.push_token(t);
                self.pending_box_shift = None;
                self.error("A <box> was supposed to be here");
            }
        } else {
            self.pending_box_shift = Some((d, false));
            self.skip_spaces_relax();
            let t = self.get_token();
            if self.box_prim_or_name(t).is_some()
                || (t.is_cs() && self.cs.name(t.cs_id()) == b"usebox")
            {
                self.push_token(t);
                self.scan_box_after_move();
            } else {
                self.push_token(t);
                self.pending_box_shift = None;
                self.error("A <box> was supposed to be here");
            }
        }
    }
    pub fn scan_box_after_move(&mut self) {
        // like \box primitive handling: <box spec> = \box<n> | \hbox.. | \vtop.. | \vbox..
        self.skip_spaces_relax();
        let t = self.get_token();
        if !t.is_cs() {
            self.push_token(t);
            return;
        }
        match self.box_prim_or_name(t) {
            Some(Prim::HBox) => self.begin_box(0),
            Some(Prim::VBox) => self.begin_box(1),
            Some(Prim::VTop) => self.begin_box(2),
            Some(Prim::Box) => {
                let idx = self.scan_reg_num();
                let b = self.eqtb.boxed[idx as usize].take();
                let b = b.map(|mut n| {
                    if let Node::Box { shift, .. } = &mut n {
                        if let Some((d, _)) = self.pending_box_shift {
                            *shift = d;
                        }
                    }
                    n
                });
                self.pending_box_shift = None;
                self.append_box_node(b);
            }
            Some(Prim::Copy) => {
                let idx = self.scan_reg_num();
                let b = self.eqtb.boxed[idx as usize].clone();
                let b = b.map(|mut n| {
                    if let Node::Box { shift, .. } = &mut n {
                        if let Some((d, _)) = self.pending_box_shift {
                            *shift = d;
                        }
                    }
                    n
                });
                self.pending_box_shift = None;
                self.append_box_node(b);
            }
            Some(Prim::VCenter) => self.begin_box(3),
            Some(Prim::LastBox) => {
                let b = self.take_last_box();
                self.append_box_node(b);
            }
            _ => {
                if t.is_cs() && self.cs.name(t.cs_id()) == b"usebox" {
                    let idx = self.scan_reg_num();
                    let b = self.eqtb.boxed[idx as usize].take();
                    self.append_box_node(b);
                } else {
                    self.push_token(t);
                    self.error("Missing box after move/raise");
                }
            }
        }
    }

    fn current_nodes(&self) -> &[Node] {
        if self.mode == Mode::Vertical {
            &self.page_list
        } else {
            &self.cur_list
        }
    }

    fn current_nodes_mut(&mut self) -> &mut Vec<Node> {
        if self.mode == Mode::Vertical {
            &mut self.page_list
        } else {
            &mut self.cur_list
        }
    }

    /// e-TeX `\\lastnodetype`: -1 if the current list is empty, else the
    /// type of `tail` (etex.web; TeX Live e-TeX manual).
    /// tex.web `tail` of the current list. In outer vmode the page builder
    /// has consumed page_list[..page_processed], so \lastskip/\lastpenalty/
    /// \lastkern/\lastbox/\unskip/\unkern/\unpenalty/\lastnodetype must see
    /// only the UNCONSUMED contributions (real TeX moves consumed nodes off
    /// the vlist; reading consumed ones made \lastskip report stale glue and
    /// broke LaTeX's \addvspace `\ifdim\lastskip=\z@` branching).
    fn current_tail(&self) -> Option<&Node> {
        match self.mode {
            Mode::Vertical => {
                if let Some((c, ..)) = self.output_tail {
                    if c > 0 && c <= self.page_list.len() {
                        return self.page_list.get(c - 1);
                    }
                    return None;
                }
                // outer vmode: the page builder has already moved
                // `page_list[..page_processed]` onto the current page, so
                // the contribution-list tail is the last UNCONSUMED node;
                // an empty suffix means an empty list (tail == head)
                let start = self.page_processed.min(self.page_list.len());
                self.page_list[start..].last()
            }
            _ => self.cur_list.last(),
        }
    }

    fn take_current_tail(&mut self) -> Option<Node> {
        match self.mode {
            Mode::Vertical => {
                if let Some((c, saved, pg, m)) = self.output_tail {
                    if c > 0 && c <= self.page_list.len() {
                        let node = self.page_list.remove(c - 1);
                        self.output_tail = Some((c - 1, saved, pg, m));
                        return Some(node);
                    }
                    return None;
                }
                // only the unconsumed suffix is removable; popping a
                // consumed page node steals shipped material and shifts
                // every stored breakpoint index behind it
                let start = self.page_processed.min(self.page_list.len());
                if start >= self.page_list.len() {
                    return None;
                }
                self.page_list.pop()
            }
            _ => self.cur_list.pop(),
        }
    }

    pub fn last_node_type_value(&self) -> i32 {
        match self.current_tail() {
            None if self.mode == Mode::Vertical => self.last_page_node_type,
            None => -1,
            Some(Node::Char { .. }) | Some(Node::NativeGlyphRun { .. }) => 0,
            Some(Node::Box { kind, .. }) if *kind == boxes::HBOX => 1,
            Some(Node::Box { .. }) => 2,
            Some(Node::Rule { .. }) => 3,
            Some(Node::Ins { .. }) => 4,
            Some(Node::Mark { .. }) => 5,
            Some(Node::Adj(_)) | Some(Node::VAdjust(_)) => 6,
            Some(Node::Ligature { .. }) => 7,
            Some(Node::Disc(_)) => 8,
            Some(Node::Whatsit(_)) => 9,
            Some(Node::Style(_))
            | Some(Node::Choice)
            | Some(Node::ChoiceAlt { .. })
            | Some(Node::MathChar { .. })
            | Some(Node::Frac { .. })
            | Some(Node::Radical { .. })
            | Some(Node::Scripts { .. })
            | Some(Node::DelimBox { .. })
            | Some(Node::Accent { .. })
            | Some(Node::Overline { .. })
            | Some(Node::MathKern(..)) => 10,
            Some(Node::Glue(_)) | Some(Node::Leaders { .. }) => 11,
            Some(Node::Kern(_)) | Some(Node::ExplicitKern(_)) | Some(Node::MarginKern { .. }) => 12,
            Some(Node::Penalty(_)) => 13,
            Some(Node::InsDisc) | Some(Node::Empty) => 14,
            Some(Node::NonScript) | Some(Node::MuGlue(_)) => 11,
            Some(Node::VCenter { .. }) => 10,
            _ => -1,
        }
    }
    pub fn margin_kern_width(&self, n: u16, left: bool) -> i32 {
        let Some(Some(Node::Box { list, .. })) = self.eqtb.boxed.get(n as usize) else {
            return 0;
        };
        let nodes: Vec<&Node> = if left {
            list.iter().collect()
        } else {
            list.iter().rev().collect()
        };
        for node in nodes {
            match node {
                Node::MarginKern { width, .. } => return *width,
                Node::Glue(_) | Node::Kern(_) | Node::ExplicitKern(_) | Node::Penalty(_) => {
                    continue
                }
                _ => break,
            }
        }
        0
    }

    pub fn do_discretionary(&mut self) {
        let f = self.eqtb.cur_font_val;
        let toks_to_nodes = |toks: Vec<Token>| -> Vec<Node> {
            toks.into_iter()
                .filter_map(|t| {
                    if t.is_char() {
                        Some(Node::Char {
                            c: t.chr() as u8,
                            font: f,
                        })
                    } else {
                        None
                    }
                })
                .collect()
        };
        let pre = toks_to_nodes(self.scan_general_text());
        let post = toks_to_nodes(self.scan_general_text());
        let rep = toks_to_nodes(self.scan_general_text());
        let d = Node::Disc(crate::boxes::DiscNode {
            pre_break: pre,
            post_break: post,
            no_break: rep,
            // Explicit replacements live here, not in following source nodes.
            replace_count: 0,
        });
        self.cur_list.push(d);
    }

    pub fn take_last_box(&mut self) -> Option<Node> {
        if let Some(Node::Box { .. }) = self.current_tail() {
            self.take_current_tail()
        } else {
            None
        }
    }

    pub fn last_kern_value(&mut self) -> i32 {
        match self.current_tail() {
            Some(Node::Kern(k) | Node::ExplicitKern(k)) => *k,
            None if self.mode == Mode::Vertical => self.last_page_kern,
            _ => 0,
        }
    }

    pub fn last_penalty_value(&mut self) -> i32 {
        match self.current_tail() {
            Some(Node::Penalty(p)) => *p,
            None if self.mode == Mode::Vertical => self.last_page_penalty,
            _ => 0,
        }
    }

    pub fn last_skip_value(&mut self) -> Glue {
        match self.current_tail() {
            Some(Node::Glue(g)) => g.clone(),
            Some(Node::Leaders { glue, .. }) => glue.clone(),
            None if self.mode == Mode::Vertical => {
                self.last_page_glue.clone().unwrap_or_else(Glue::zero)
            }
            _ => Glue::zero(),
        }
    }

    /// tex.web §1105: pop a trailing glue (or leader) node; no-op otherwise.
    pub fn un_skip(&mut self) {
        if matches!(
            self.current_tail(),
            Some(Node::Glue(_) | Node::Leaders { .. })
        ) {
            self.take_current_tail();
        }
    }

    /// tex.web §1110: pop a trailing kern; no-op otherwise.
    pub fn un_kern(&mut self) {
        if matches!(
            self.current_tail(),
            Some(Node::Kern(_) | Node::ExplicitKern(_))
        ) {
            self.take_current_tail();
        }
    }

    /// tex.web §1110: pop a trailing penalty; no-op otherwise.
    pub fn un_penalty(&mut self) {
        if matches!(self.current_tail(), Some(Node::Penalty(_))) {
            self.take_current_tail();
        }
    }

    pub fn do_vsplit(&mut self) {
        let _ = self.take_global();
        let (top, n, rest) = self.scan_vsplit();
        if let Some(rest) = rest {
            self.stash_vsplit_remainder(n, rest);
        }
        self.append_box_node(top);
    }

    /// Scan `\vsplit<n> to <dimen>` and return the top part, source register,
    /// and unpacked remainder. The caller replaces the source register with
    /// the re-packed remainder via `stash_vsplit_remainder`.
    pub fn scan_vsplit(&mut self) -> (Option<Node>, u16, Option<NodeList>) {
        self.marks[3].clear();
        self.marks[4].clear();
        let n = self.scan_reg_num();
        self.scan_keyword(b"to");
        let target = self.scan_dimen(false, false);
        let bx = self.eqtb.boxed.get(n as usize).cloned().flatten();
        match bx {
            Some(b) => {
                self.vsplat_remainder = None;
                let top = self.vsplit_box(b, target);
                (top, n, self.vsplat_remainder.take())
            }
            None => (None, n, Some(Vec::new())),
        }
    }

    /// Re-pack a `\vsplit` remainder into its source register. Like
    /// `box(n):=...` in TeX, this mutates the value at the register's existing
    /// assignment level: an outer register stays consumed across a group,
    /// while a locally assigned register still restores its saved outer box.
    pub fn stash_vsplit_remainder(&mut self, n: u16, rest: NodeList) {
        let value = if rest.is_empty() {
            None
        } else {
            let md = self.eqtb.dim_params[DimParam::BoxMaxDepth.idx() as usize];
            let r = boxes::vpack_add_md(rest, None, false, boxes::VBOX, &self.eqtb, md);
            Some(r.node)
        };
        self.eqtb.replace_box_value(n, value);
    }

    pub fn scan_keyword(&mut self, kw: &[u8]) -> bool {
        self.skip_spaces_relax();
        let mut collected: Vec<Token> = Vec::new();
        for &expected in kw {
            let t = self.get_token();
            collected.push(t);
            if !t.is_char() || (t.chr() as u8).to_ascii_lowercase() != expected.to_ascii_lowercase()
            {
                for t in collected.into_iter().rev() {
                    self.push_token(t);
                }
                return false;
            }
        }
        true
    }

    pub fn do_insert(&mut self) {
        // \insert<n> [to <dimen>] {vlist content} — tex.web insert_group:
        // the material is packaged as a vbox (to the target when given),
        // then wrapped into an insert node at group end (kind 8 in end_box).
        let n = self.scan_reg_num();
        let mut target: Option<(i32, bool)> = None;
        if self.scan_keyword(b"to") {
            let d = self.scan_dimen(false, false);
            target = Some((d, false));
        }
        self.saved_lists.push((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
        ));
        self.prev_graf = 0;
        self.push_group_level(LevelType::Box);

        self.box_targets.push(target);
        self.box_shifts.push(0);
        self.box_kinds.push(8);
        self.insert_nums.push(n);
        self.mode = Mode::InternalVertical;
        self.prev_depth = -1000 * 65536;
        // consume the group's opening `{` (tex.web scan_left_brace)
        self.skip_spaces_relax();
        let t = self.get_x_raw();
        if !self.token_is_left_brace(t) {
            self.error("Missing { inserted");
            self.push_token(t);
            self.push_token(Token::char(1, b'{' as u32));
        }
        self.normal_paragraph();
    }

    /// tex.web \vadjust: the braced material is typeset in internal vertical
    /// mode and NOT packed — the resulting vlist migrates into the enclosing
    /// vertical list right after the line containing the adjustment (tex.web
    /// post_line_break). Modeled as a box group (kind 9) that end_box
    /// captures. [pre] is treated as post (latex.ltx never uses pre).
    pub fn append_vadjust(&mut self) {
        let _pre = self.scan_keyword(b"pre");
        self.begin_box(9);
    }

    pub fn append_mark(&mut self, class: i32, toks: Vec<Token>) {
        match self.mode {
            Mode::Vertical | Mode::InternalVertical => self.vlist_append(Node::Mark {
                class,
                tokens: toks,
            }),
            Mode::Math | Mode::DisplayMath => self.append_mlist_node(Node::Mark {
                class,
                tokens: toks,
            }),
            _ => self.cur_list.push(Node::Mark {
                class,
                tokens: toks,
            }),
        }
    }

    pub fn do_shipout(&mut self) {
        // \shipout<box spec>
        self.skip_spaces_relax();
        let t = self.get_token();
        if !t.is_cs() {
            self.error("\\shipout expects a box");
            self.push_token(t);
            return;
        }
        if t.is_cs() {
            if let Some(crate::eqtb::Equiv::Prim(prim)) = self.eqtb.resolve(t.cs_id()).cloned() {
                match prim {
                    crate::prim::Prim::HBox
                    | crate::prim::Prim::VBox
                    | crate::prim::Prim::VTop
                    | crate::prim::Prim::VCenter => {
                        self.park_setbox(255);
                        self.shipout_depth = self.box_kinds.len();
                        let kind = match prim {
                            crate::prim::Prim::HBox => 0,
                            crate::prim::Prim::VBox => 1,
                            crate::prim::Prim::VTop => 2,
                            _ => 3,
                        };
                        self.begin_box(kind);
                        self.shipout_pending = true;
                        return;
                    }
                    crate::prim::Prim::Box => {
                        let idx = self.scan_reg_num();
                        let b = self.eqtb.boxed[idx as usize].take();
                        self.ship_box(b);
                        return;
                    }
                    crate::prim::Prim::Copy => {
                        let idx = self.scan_reg_num();
                        let b = self.eqtb.boxed.get(idx as usize).cloned().flatten();
                        self.ship_box(b);
                        return;
                    }
                    _ => {}
                }
            }
        }
        match self.cs.name(t.cs_id()) {
            b"hbox" | b"vbox" | b"vtop" | b"vcenter" => {
                self.park_setbox(255);
                self.shipout_depth = self.box_kinds.len();
                let kind = match self.cs.name(t.cs_id()) {
                    b"hbox" => 0,
                    b"vbox" => 1,
                    b"vtop" => 2,
                    _ => 3,
                };
                self.begin_box(kind);
                self.shipout_pending = true;
            }
            b"box" => {
                let idx = self.scan_reg_num();
                let b = self.eqtb.boxed.get(idx as usize).cloned().flatten();
                self.ship_box(b);
            }
            _ => {
                self.push_token(t);
                self.error("\\shipout expects a box");
            }
        }
    }

    // ---------- paragraphs ----------
    pub fn par_primitive(&mut self) {
        // tex.web: between alignment rows (\cr .. next u part) a \par token
        // (e.g. from a blank line before \hline) must not disturb the align
        // state — the interrow phase is idle and the par is a no-op.
        if self.mode != Mode::Horizontal
            && self.scanner_status == crate::engine::ScannerStatus::Aligning
            && self.align_phase() == crate::align::PH_IDLE
        {
            return;
        }
        match self.mode {
            Mode::Horizontal => self.end_paragraph(),
            Mode::Vertical => {
                self.resume_after_display = false;
                self.build_page();
            }
            Mode::InternalVertical => {
                self.resume_after_display = false;
            }
            Mode::Math | Mode::DisplayMath => {
                self.error("Missing $ inserted (\\par in math)");
            }
            // tex.web §21179 end_graf: `if mode = hmode` — in restricted hmode (-hmode),
            // \par does not end a paragraph; it is a no-op.
            Mode::RestrictedHorizontal => {}
        }
    }
    /// tex.web: an assignment is global if \global prefixed it OR
    /// \globaldefs>0 forces global; \globaldefs<0 forces local even after
    /// \global. Consumes the pending \global flag (call exactly once per
    /// assignment).
    pub fn take_global(&mut self) -> bool {
        let gd = self.eqtb.int_params[crate::prim::IntParam::GlobalDefs.idx() as usize];
        let g = if gd > 0 {
            true
        } else if gd < 0 {
            false
        } else {
            self.global_flag
        };
        self.global_flag = false;
        g
    }
    /// tex.web active_base: active characters resolve through a dedicated
    /// eqtb region, NOT the hash — control symbol `\~` and active `~` are
    /// distinct entries (fontenc's \DeclareTextAccent{\~} retargets the
    /// former and must never clobber the kernel tie in the latter). We
    /// model the separate region with collision-proof placeholder names
    /// in the shared cs table.
    pub fn active_cs_name(c: u8) -> [u8; 7] {
        [0xFF, 0x00, b'A', b'C', b'T', 0x00, c]
    }

    pub fn active_cs_id(&mut self, c: u8) -> CsId {
        let name = Self::active_cs_name(c);
        match self.cs.lookup(&name) {
            Some(id) => id,
            None => self.cs.intern(&name),
        }
    }

    pub fn active_cs_lookup(&self, c: u8) -> Option<CsId> {
        self.cs.lookup(&Self::active_cs_name(c))
    }

    /// tex.web §1079: paragraph-shape controls are reset locally when a
    /// paragraph ends or an internal vertical-list context begins.
    fn normal_paragraph(&mut self) {
        self.assign_par_shape(Vec::new(), false);
        self.eqtb
            .assign_int_param(crate::prim::IntParam::Looseness, 0, false);
        self.eqtb
            .assign_int_param(crate::prim::IntParam::HangAfter, 1, false);
        self.eqtb
            .assign_dim_param(crate::prim::DimParam::HangIndent, 0, false);
    }

    pub fn start_paragraph(&mut self, indent: bool) {
        self.flush_native_text();
        match self.mode {
            Mode::Horizontal => {
                if indent {
                    let pi = self.eqtb.dim_params[DimParam::ParIndent.idx() as usize];
                    let r = boxes::hpack(Vec::new(), Some(pi), boxes::HBOX, &self.eqtb);
                    self.cur_list.push(r.node);
                }
            }
            Mode::Vertical => {
                self.par_interrupted = false;
                // tex.web resume_after_display (§1194): when text follows a
                // display the new hlist is pushed directly — no \parskip,
                // no \parindent box, no \everypar
                let resume = std::mem::take(&mut self.resume_after_display);
                if !resume {
                    // tex.web new_graf (§21128): in outer vmode \parskip glue is
                    // appended unconditionally; if nest_ptr=1, build_page puts
                    // \parskip glue on the current page and evaluates legal breaks.
                    let ps = self.eqtb.glue_params[GlueParam::ParSkip.idx() as usize].clone();
                    self.page_list.push(Node::Glue(ps));
                    self.build_page();
                }
                // (tex.web: a paragraph is not a group; no eqtb level)
                // In outer vmode, the global contribution list (page_list)
                // remains live across the paragraph so that any output
                // routine fired during the paragraph sees and contributes to it.
                self.saved_lists.push((
                    Mode::Vertical,
                    Vec::new(),
                    self.prev_depth,
                    self.space_factor,
                    self.prev_graf,
                ));
                self.par_saves += 1;
                self.par_page_lists.push(Vec::new());
                self.mode = Mode::Horizontal;
                self.cur_list = Vec::new();
                self.space_factor = 1000;
                // tex.web new_graf zeroes the level's prev_graf; a display
                // RESUMPTION (resume_after_display) does not — the count
                // already sitting in the field (fragment lines + 3 per
                // display) is what §17015/§17253 continue numbering from
                if !resume {
                    *self.prev_graf_mut() = 0;
                }
                if resume {
                    return;
                }
                if indent {
                    let pi = self.eqtb.dim_params[DimParam::ParIndent.idx() as usize];
                    let r = boxes::hpack(Vec::new(), Some(pi), boxes::HBOX, &self.eqtb);
                    self.cur_list.push(r.node);
                }
                self.run_everypar();
            }
            Mode::InternalVertical => {
                self.par_interrupted = false;
                // like vertical but inside a box; tex.web new_graf adds
                // \parskip here only when the vertical list is nonempty
                // (no skip at the start of an empty \vbox list)
                let resume = std::mem::take(&mut self.resume_after_display);
                if !resume && !self.cur_list.is_empty() {
                    let ps = self.eqtb.glue_params[GlueParam::ParSkip.idx() as usize].clone();
                    self.cur_list.push(Node::Glue(ps));
                }
                let page = std::mem::take(&mut self.cur_list);
                self.saved_lists.push((
                    Mode::InternalVertical,
                    Vec::new(),
                    self.prev_depth,
                    self.space_factor,
                    self.prev_graf,
                ));
                self.par_page_lists.push(page);
                self.mode = Mode::Horizontal;
                self.cur_list = Vec::new();
                self.space_factor = 1000;
                if !resume {
                    *self.prev_graf_mut() = 0;
                }
                if resume {
                    return;
                }
                if indent {
                    let pi = self.eqtb.dim_params[DimParam::ParIndent.idx() as usize];
                    let r = boxes::hpack(Vec::new(), Some(pi), boxes::HBOX, &self.eqtb);
                    self.cur_list.push(r.node);
                }
                self.run_everypar();
            }
            _ => {}
        }
    }

    /// \prevgraf belongs to the enclosing vertical nest, even in an hbox or math.
    pub(crate) fn prev_graf(&self) -> i32 {
        if self.mode.is_v() {
            self.prev_graf
        } else {
            self.saved_lists
                .iter()
                .rev()
                .find(|(mode, ..)| mode.is_v())
                .map(|(_, _, _, _, pg)| *pg)
                .unwrap_or(self.prev_graf)
        }
    }

    pub(crate) fn prev_graf_mut(&mut self) -> &mut i32 {
        if self.mode.is_v() {
            &mut self.prev_graf
        } else {
            self.saved_lists
                .iter_mut()
                .rev()
                .find(|(mode, ..)| mode.is_v())
                .map(|(_, _, _, _, pg)| pg)
                .unwrap_or(&mut self.prev_graf)
        }
    }

    /// Charge the display's three lines before resuming paragraph line numbering.
    pub(crate) fn resume_after_display(&mut self) {
        *self.prev_graf_mut() += 3;
    }

    fn run_everypar(&mut self) {
        let toks = (*self.eqtb.tok_params[crate::prim::ToksParam::EveryPar.idx() as usize]).clone();
        if !toks.is_empty() {
            self.push_tokens(toks);
        }
    }

    pub fn end_paragraph(&mut self) {
        self.flush_native_text();
        // a displayed equation counts as three lines of the interrupted
        // paragraph, and §17015/§17253 make the resumed fragment's line
        // numbering (and so \parshape/\hangindent lookup and \prevgraf)
        // continue from the enclosing level's accumulated count. The old
        // reset destroyed that: a wrapfigure-interrupted paragraph set the
        // fragment's prev_graf back to 1, re-running the shape's narrow
        // lines after every display and never retiring the counter.
        // the +3-per-display accounting itself lives at the
        // resume_after_display sites (math.rs, tex.web §22509); here the
        // flag is only consumed/cleared
        self.resume_after_display = false;

        // tex.web end_graf ignores a paragraph only when the hlist is
        // STRUCTURALLY empty (`if head=tail then pop_nest`). A list holding
        // only a `\vadjust` — LaTeX's `\end@float` in-text path plants
        // `\vadjust{\penalty-\@Miv \vbox{}\penalty\@floatpenalty}` with no
        // further text — is NOT empty: real TeX breaks the (zero-width)
        // line and post_line_break migrates the adjustment into the
        // vertical list. Treating it as an abandoned paragraph destroyed
        // the float markers, stranding the box in `\@currlist` and ending
        // in `Float(s) lost` at the next clearpage.
        let has_content = !self.cur_list.is_empty();

        if !has_content {
            self.cur_list.clear();
            // tex.web: \parshape/\looseness/\hangafter/\hangindent are reset
            // only in normal_paragraph (§1079) after a real line break — an
            // ABANDONED (empty) paragraph leaves them intact. LaTeX's list
            // machinery starts-and-abandons an empty paragraph on every
            // \item; wiping here destroyed \list's \parshape before the
            // first real list paragraph broke.
            let (saved_mode, saved_list, pd, sf, pg) = self.saved_lists.pop().unwrap_or((
                Mode::Vertical,
                Vec::new(),
                self.prev_depth,
                self.space_factor,
                self.prev_graf,
            ));
            self.prev_graf = pg;
            self.prev_depth = pd;
            self.space_factor = sf;
            self.mode = saved_mode;
            if let Some(outer) = self.par_page_lists.pop() {
                if saved_mode != Mode::Vertical {
                    self.cur_list = outer;
                }
            } else {
                self.cur_list = saved_list;
            }
            return;
        }

        let fnt = self.eqtb.cur_font_val;
        if fnt != 0 {
            self.flush_hyphen_disc(fnt);
            self.flush_right_boundary_kern(fnt);
        }
        let pfs = self.eqtb.glue_params[GlueParam::ParFillSkip.idx() as usize].clone();
        // tex.web §16074: a trailing glue node is REPLACED by the infinite
        // penalty ("removing a space if it was there, since spaces usually
        // precede blank lines"); otherwise the penalty is appended. Without
        // this, a space after \end{tabular} survives into the final line and
        // forces a phantom second line in float/tabular paragraphs.
        match self.cur_list.last_mut() {
            Some(slot @ Node::Glue(_)) => *slot = Node::Penalty(10000),
            _ => self.cur_list.push(Node::Penalty(10000)),
        }
        self.cur_list.push(Node::Glue(pfs));
        let content = std::mem::take(&mut self.cur_list);
        // tex.web §21764/§21181: the widow penalty before the final line is
        // \displaywidowpenalty when a display interrupted the paragraph
        let display_widow = self.next_par_widow.is_some();
        let fw = self.next_par_widow.take().unwrap_or_else(|| {
            self.eqtb.int_params[crate::prim::IntParam::WidowPenalty.idx() as usize]
        });

        let lines = self.break_paragraph(content, fw, display_widow);
        let line_count = match &lines {
            Node::Box { list, .. } => list
                .iter()
                .filter(|m| matches!(m, Node::Box { kind, .. } if *kind == crate::boxes::HBOX))
                .count(),
            _ => 1,
        };
        // tex.web §17264: prev_graf := best_line - 1 — the ABSOLUTE count
        // of paragraph lines now in the vlist (this break added
        // line_count to whatever resume_after_display had accumulated)
        *self.prev_graf_mut() += line_count as i32;
        // A soft page break that SHIPPED this paragraph's lines interrupts
        // it: the resumed content has no complete line yet, so just_box
        // must stay empty until the next real break refreshes it.
        self.last_par_line = match &lines {
            Node::Box { list, .. } => list
                .iter()
                .rev()
                .find(|n| matches!(n, Node::Box { kind, .. } if *kind == crate::boxes::HBOX))
                .cloned(),
            _ => None,
        };

        // tex.web §1079 normal_paragraph: reset paragraph-local parameters —
        // all four resets are LOCAL eq_defines, so a group-wrapped \par (the
        // `{\@@par}` LaTeX lists install via \@setpar) rolls them back at
        // \egroup and the shape survives across \items
        if !self.in_display_init {
            self.normal_paragraph();
        }
        // restore vertical context
        let (saved_mode, _, pd, _sf, pg) = self.saved_lists.pop().unwrap_or((
            Mode::Vertical,
            Vec::new(),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
        ));
        self.prev_graf = pg;
        self.prev_depth = pd;

        let mut lines_opt = Some(lines);
        match (saved_mode, self.par_page_lists.pop()) {
            (Mode::Vertical, _) => {
                // paragraph was at outer level: tex.web contributes the line
                // boxes (and migrated \vadjust material) directly to the page
                // builder. Packing them into one opaque vbox would make the
                // page unable to break inside the paragraph and would bury
                // \vadjust float markers (\end@float's hmode path).
                let lines = match lines_opt.take().unwrap() {
                    Node::Box { list, .. } => list,
                    other => vec![other],
                };
                // tex.web append_to_vlist: materialize interline glue NOW
                // with the \baselineskip in force at paragraph end — the
                // page builder's lazy interline would read post-group state
                let (filled, last_d) = self.fill_line_interline(self.prev_depth, lines);
                self.page_list.extend(filled);
                self.mode = Mode::Vertical;
                self.cur_list = Vec::new();
                self.prev_depth = last_d;

                if !self.in_display_init {
                    let pages_before = self.pdf_doc.pages.len();
                    self.build_page();
                    // tex.web: a page shipped inside this paragraph interrupts
                    // it — the resumed content has no complete line yet, so
                    // just_box must not carry a stale clone into init_math.
                    if self.pdf_doc.pages.len() > pages_before {
                        self.par_interrupted = true;
                    }
                }
            }
            (Mode::InternalVertical, Some(inner)) => {
                // paragraph started inside a \vbox/\vtop: tex.web appends the
                // line boxes to that vlist FLAT (append_to_vlist), threading
                // interline glue from the surrounding prev_depth — packing
                // them into one wrapper vbox buries the leading and feeds the
                // wrapper's full height into the next interline computation
                // (a \vspace-separated heading then collapses to \lineskip).
                let bx = lines_opt.take().unwrap();
                let taken = match bx {
                    Node::Box { list, .. } => list,
                    other => vec![other],
                };
                let (filled, last_d) = self.fill_line_interline(self.prev_depth, taken);
                self.cur_list = inner;
                self.mode = Mode::InternalVertical;
                self.cur_list.extend(filled);
                self.prev_depth = last_d;
            }
            (_, outer) => {
                let node = lines_opt.take().unwrap();
                // tex.web: after line_break, prev_depth = the final line's
                // depth — the lines wrapper box carries exactly that depth,
                // so thread it instead of restoring the pre-paragraph value
                // (which left the display interline glue computed against a
                // stale prev_depth, misplacing every amsmath display).
                if let Node::Box { d, .. } = &node {
                    self.prev_depth = *d;
                }
                if let Some(mut inner) = outer {
                    inner.push(node);
                    self.cur_list = inner;
                } else {
                    self.cur_list.push(node);
                }
                self.mode = saved_mode;
            }
        }
    }

    /// replace build_lines' zero interline placeholders with real
    /// baselineskip/lineskip glue (tex.web append_to_vlist §17438-17440):
    /// used when a paragraph's lines land in an internal vlist, which the
    /// page builder never processes
    fn fill_line_interline(&self, outer_prev_depth: i32, list: NodeList) -> (NodeList, i32) {
        const IGNORE: i32 = -1000 * 65536;
        let bs = self.eqtb.glue_params[GlueParam::BaselineSkip.idx() as usize].clone();
        let ls = self.eqtb.glue_params[GlueParam::LineSkip.idx() as usize].clone();
        let lsl = self.eqtb.dim_params[DimParam::LineSkipLimit.idx() as usize];
        let mut out: NodeList = Vec::with_capacity(list.len());
        let mut prev_depth = outer_prev_depth;
        // hold a pending placeholder until we know whether a box follows
        let mut held_placeholder = false;
        for n in list.into_iter() {
            match n {
                Node::Glue(ref g) if g.width == 0 && g.stretch == 0 && g.shrink == 0 => {
                    held_placeholder = true;
                    continue;
                }
                Node::VAdjust(items) => {
                    // tex.web §17415-17416: adjustment material joins the
                    // vertical list raw without interline glue and without
                    // altering prev_depth
                    out.extend(items);
                }
                Node::Rule { .. } => {
                    held_placeholder = false;
                    prev_depth = IGNORE;
                    out.push(n);
                }
                Node::Box { h, d, .. } => {
                    let (h, d) = (h, d);
                    if prev_depth > IGNORE {
                        let b = bs.width as i64 - prev_depth as i64 - h as i64;
                        let glue = if b < lsl as i64 {
                            ls.clone()
                        } else {
                            Glue {
                                width: b as i32,
                                ..bs.clone()
                            }
                        };
                        if glue.width != 0 || glue.stretch != 0 || glue.shrink != 0 {
                            out.push(Node::Glue(glue));
                        }
                    }
                    prev_depth = d;
                    held_placeholder = false;
                    out.push(n);
                }
                _ => {
                    if held_placeholder {
                        // a placeholder not followed by a box: keep it
                        // (defensive; build_lines only emits them pre-box)
                        out.push(Node::Glue(Glue::zero()));
                        held_placeholder = false;
                    }
                    out.push(n);
                }
            }
        }
        if held_placeholder {
            out.push(Node::Glue(Glue::zero()));
        }
        (out, prev_depth)
    }
}

/// tex.web print_scaled: `s` in sp printed as `<int>.<5 digits>`, the first
/// fraction digit rounded half up via the +5 trick.
pub fn print_scaled(v: i64) -> String {
    let neg = v < 0;
    let s = v.abs();
    let int_part = s / 65536;
    let mut frac = 10 * (s % 65536) + 5;
    let mut digits = [b'0'; 5];
    for d in digits.iter_mut() {
        *d = b'0' + (frac / 65536) as u8;
        frac = 10 * (frac % 65536);
    }
    format!(
        "{}{}.{}",
        if neg { "-" } else { "" },
        int_part,
        String::from_utf8_lossy(&digits)
    )
}

#[cfg(test)]
mod structural_state_tests {
    use crate::engine::Engine;
    use crate::prim::IntParam;
    use crate::tfm::CharInfo;

    fn run_in(engine: &mut Engine, src: &str) {
        engine.init_primitives();
        engine.add_nullfont();
        let tick = char::from(96);
        let input =
            format!("\\catcode{tick}\\{{=1 \\catcode{tick}\\}}=2 \\catcode{tick}\\#=6 {src}\n");
        engine
            .input
            .push_file("structural-state.tex".into(), input.into_bytes());
        engine.run();
    }

    #[test]
    fn unfinished_leader_box_cannot_affect_a_later_engine() {
        let mut engine = Box::new(Engine::new(true));
        let address = (&*engine) as *const Engine;
        run_in(&mut engine, r"\hbox{\leaders\hbox{");
        engine.finish_job_diagnostics();
        assert!(engine.stopped_on_error);
        assert_eq!(engine.leader_stack.len(), 1);

        // Replace the engine without changing its allocation. This makes the
        // address-reuse regression deterministic rather than allocator-dependent.
        *engine = Engine::new(true);
        assert_eq!((&*engine) as *const Engine, address);

        // The inner ordinary box closes at the same box-stack depth as the
        // abandoned leader object above. A process-global stack would treat
        // it as that earlier engine's leader body and consume the following
        // closing brace while looking for glue.
        run_in(&mut engine, r"\hbox{\hbox{ok}}\end");
        assert_eq!(
            engine.error_count, 0,
            "later engine inherited leader state:\n{}",
            engine.diagnostic_output
        );
        assert!(engine.leader_stack.is_empty());
    }

    #[test]
    fn missing_text_glyphs_warn_for_nullfont_and_empty_tfm_slots() {
        let mut engine = Engine::new(false);
        engine.add_nullfont();
        assert_eq!(
            engine.eqtb.int_params[IntParam::TracingLostChars.idx() as usize],
            0
        );
        engine.eqtb.int_params[IntParam::TracingLostChars.idx() as usize] = 1;

        engine.append_char(b'A');
        assert_eq!(engine.diagnostics.len(), 1);
        assert!(engine.diagnostics[0].message.contains("font `nullfont`"));
        assert!(engine.cur_list.is_empty());

        let mut font_with_hole = (*engine.eqtb.fonts[0]).clone();
        font_with_hole.name = "holes".into();
        font_with_hole.tfm_name = "holes".into();
        font_with_hole.bc = 0;
        font_with_hole.ec = 2;
        font_with_hole.chars = vec![
            CharInfo {
                width: 0,
                height: 0,
                depth: 0,
                italic: 0,
                tag: 0,
                remainder: 0,
            };
            3
        ];
        engine.eqtb.fonts.push(std::rc::Rc::new(font_with_hole));
        engine.eqtb.cur_font_val = 1;

        engine.append_char(1);
        assert_eq!(engine.diagnostics.len(), 2);
        assert!(engine.diagnostics[1].message.contains("font `holes`"));
        assert!(engine.cur_list.is_empty());
    }
}
