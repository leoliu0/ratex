//! List building: characters, glue, kerns, penalties, rules, box groups,
//! paragraphs, page-builder hook.

use crate::boxes::{self, Glue, Node, NodeList};
use crate::engine::{Engine, Mode};
use crate::eqtb::LevelType;
use crate::tfm::Font;
use crate::prim::{DimParam, GlueParam, IntParam, Prim};
use crate::scaled::ONE;
use crate::token::{CsId, Token};

pub const RULE_FILL: i32 = i32::MIN; // sentinel: rule dimension from context
/// box_kinds marker for a \discretionary part group (tex.web disc_group)
const DISC_GROUP_KIND: u8 = 10;

/// a matching lig/kern program instruction (tex.web §545)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LigKernOp {
    Kern(i32),
    /// ligature: the op byte (`=:`=0, `=:|`=1, `|=:`=2, `|=:|`=3, `=:|>`=5,
    /// `|=:>`=6, `|=:|>`=7, `|=:|>>`=11) and the ligature character
    Lig { op: u8, ch: u8 },
}

/// tex.web §1039/§909: the instruction of `cur_l`'s lig/kern program (None:
/// the font's left boundary program) whose next char is `cur_r`.
pub(crate) fn lig_kern_step(font: &Font, cur_l: Option<u8>, cur_r: u8) -> Option<LigKernOp> {
    let prog = &font.lig_kern;
    let mut k = match cur_l {
        None => bchar_label(font)?,
        Some(c) => {
            let ci = font.chars.get(c as usize).filter(|_| font.exists_char(c))?;
            if ci.tag != crate::tfm::TAG_LIG {
                return None;
            }
            let k = ci.remainder as usize;
            let first = prog.get(k)?;
            // lig_kern_restart: a first instruction with skip_byte > 128
            // points to the real program
            if first.skip > 128 {
                256 * first.op as usize + first.rem as usize
            } else {
                k
            }
        }
    };
    loop {
        let q = prog.get(k)?;
        if q.next_char == cur_r && q.skip <= 128 {
            return Some(if q.op >= 128 {
                let idx = 256 * (q.op as usize - 128) + q.rem as usize;
                LigKernOp::Kern(font.kerns.get(idx).copied().unwrap_or(0))
            } else {
                LigKernOp::Lig { op: q.op, ch: q.rem }
            });
        }
        if q.skip >= 128 {
            return None;
        }
        k += q.skip as usize + 1;
    }
}

/// tex.web §573/§576 bchar_label: when the LAST lig/kern instruction has
/// skip_byte 255, the left boundary program starts at 256*op+rem.
pub(crate) fn bchar_label(font: &Font) -> Option<usize> {
    let last = font.lig_kern.last()?;
    let k = 256 * last.op as usize + last.rem as usize;
    (last.skip == 255 && k < font.lig_kern.len()).then_some(k)
}

/// tex.web font_false_bchar: the right boundary char when no character of
/// that code exists (an input character equal to it forms nothing)
fn false_bchar(font: &Font) -> Option<u8> {
    font.bchar.filter(|&b| !font.char_present(b))
}

/// an entry of tex.web's lig_stack (§1034): a character to the right of
/// the cursor — the input character (`is_char`) or a pseudo-ligature
/// whose `orig` is the input character it replaced (`lig_ptr`)
#[derive(Clone, Copy)]
struct LigItem {
    ch: u8,
    is_char: bool,
    orig: Option<u8>,
}

/// lig_stack holding at most the input character without allocating;
/// only `|=:|` insertions push pseudo-ligatures above it
#[derive(Default)]
struct LigStack {
    bottom: Option<LigItem>,
    above: Vec<LigItem>,
}

impl LigStack {
    fn one(item: LigItem) -> Self {
        Self {
            bottom: Some(item),
            above: Vec::new(),
        }
    }
    fn is_empty(&self) -> bool {
        self.bottom.is_none()
    }
    fn last(&self) -> Option<&LigItem> {
        self.above.last().or(self.bottom.as_ref())
    }
    fn last_mut(&mut self) -> Option<&mut LigItem> {
        match self.above.last_mut() {
            Some(top) => Some(top),
            None => self.bottom.as_mut(),
        }
    }
    fn push(&mut self, item: LigItem) {
        if self.bottom.is_none() {
            self.bottom = Some(item);
        } else {
            self.above.push(item);
        }
    }
    fn pop(&mut self) -> Option<LigItem> {
        self.above.pop().or_else(|| self.bottom.take())
    }
}

/// the main loop's cursor state (tex.web cur_l, ligature_present,
/// lft_hit, rt_hit); `cur_l` None is the left boundary
struct LigCursor {
    cur_l: Option<u8>,
    lig_present: bool,
    lft_pending: bool,
    rt_hit: bool,
}

impl Engine {
    pub fn font_resolver(&self) -> &dyn crate::fonts::FontResolver {
        self
    }

    // ---------- characters & spaces ----------

    pub fn hspace_token(&mut self) {
        self.flush_native_text();
        self.end_char_chain();
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
        self.end_char_chain();
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
                self.space_factor = self.space_factor_of(u32::from(c));
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
                    self.active_char(u32::from(c));
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
    pub fn space_factor_of(&self, c: u32) -> i32 {
        let main_s = i32::from(self.eqtb.space_factor_code(c));
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

    pub fn active_char(&mut self, c: u32) {
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
            Some(crate::eqtb::Equiv::LuaCall { slot, .. }) => self.call_lua_function(slot as i32),
            None => {
                let shown = char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER);
                self.error(&format!("Undefined active character `{shown}'"));
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
            // tex.web §1036 main_loop_move+2: a missing character leaves
            // the main loop (goto big_switch) without meeting the right
            // boundary; the next character starts a fresh chain
            if self.native_text.lig_chain == Some(f) {
                self.native_text.lig_chain = None;
                self.lig_kern_loop(f, LigStack::default(), None);
            }
            return;
        }
        // Record the source when text enters the paragraph, not during
        // shipout: by then the input scanner is usually at \end{document}.
        // A marker at each word gives inverse search a local hit without
        // interrupting ligatures within a word.
        let tail_charish = matches!(
            self.cur_list.last(),
            Some(Node::Char { font: pf, .. } | Node::Ligature { font: pf, .. }) if *pf == f
        );
        if !tail_charish {
            if self.synctex_active() {
                if let Some((path, line)) = self.input.current_file_position() {
                    if !path.is_empty() && line > 0 {
                        let file_id = self.synctex.get_or_register_file(path);
                        self.cur_list.push(Node::Whatsit(crate::boxes::WhatIt::SyncPoint {
                            file_id,
                            line,
                        }));
                    }
                }
            }
        }
        self.append_char_lig(c, f);
    }

    /// tex.web wrapup (§1035): when the last character consumed is the
    /// font's hyphen char (possibly inside a just-formed ligature like the
    /// en-dash), a null discretionary follows it — the legal break after an
    /// explicit hyphen — but only in unrestricted horizontal mode.
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

    /// \noboundary: inside a character chain it ends the chain without the
    /// right boundary (tex.web `bchar:=non_char`); a character that follows
    /// starts without the left boundary (`cancel_boundary`). Any other
    /// command in between clears that again (see `end_char_chain`).
    pub fn no_boundary(&mut self) {
        self.flush_native_text();
        if let Some(f) = self.native_text.lig_chain.take() {
            self.lig_kern_loop(f, LigStack::default(), None);
        }
        self.native_text.suppress_left_boundary = true;
    }

    /// tex.web main loop when the character chain ends (any command other
    /// than a character, `\char` or `\noboundary`): the cursor meets the
    /// font's right boundary character (§1038 `cur_r:=bchar`).
    pub(crate) fn end_char_chain(&mut self) {
        self.native_text.suppress_left_boundary = false;
        let Some(f) = self.native_text.lig_chain.take() else {
            return;
        };
        let bchar = self.eqtb.fonts.get(f as usize).and_then(|font| font.bchar);
        self.lig_kern_loop(f, LigStack::default(), bchar);
    }

    /// tex.web main loop (§1034-1040) for character `c` of font `f`: the
    /// cursor sits after the chain's last character (the tail), or after
    /// the left boundary when `c` starts a chain.
    fn append_char_lig(&mut self, c: u8, f: u16) {
        let suppress_lb = std::mem::take(&mut self.native_text.suppress_left_boundary);
        let chained = self.native_text.lig_chain == Some(f)
            && matches!(
                self.cur_list.last(),
                Some(Node::Char { font: pf, .. } | Node::Ligature { font: pf, .. }) if *pf == f
            );
        self.native_text.lig_chain = Some(f);
        let item = LigItem {
            ch: c,
            is_char: true,
            orig: None,
        };
        if chained {
            // §1038 main_loop_lookahead+1: a character equal to a
            // nonexistent boundary char forms nothing
            let false_bchar = self.eqtb.fonts.get(f as usize).and_then(|font| false_bchar(font));
            let cur_r = if false_bchar == Some(c) { None } else { Some(c) };
            self.lig_kern_loop(f, LigStack::one(item), cur_r);
            return;
        }
        // §1034: begin with the cursor after the left boundary, unless
        // \noboundary cancelled it or the font has no boundary program
        let has_lb = self
            .eqtb
            .fonts
            .get(f as usize)
            .is_some_and(|font| bchar_label(font).is_some());
        if has_lb && !suppress_lb {
            let mut cur = LigCursor {
                cur_l: None,
                lig_present: false,
                lft_pending: false,
                rt_hit: false,
            };
            self.lig_kern_run(f, &mut cur, LigStack::one(item), Some(c), None);
        } else {
            self.cur_list.push(Node::Char { c, font: f });
        }
    }

    /// Run the main loop with the cursor after the chain's tail (cur_l),
    /// `stack` the characters to its right and `cur_r` the next character
    /// (None: non_char). An empty stack means the chain is ending and the
    /// right boundary is `cur_r`.
    fn lig_kern_loop(&mut self, f: u16, stack: LigStack, cur_r: Option<u8>) {
        let (cur_l, lig_present) = match self.cur_list.last() {
            Some(Node::Char { c, font }) if *font == f => (*c, false),
            Some(Node::Ligature { c, font, .. }) if *font == f => (*c, true),
            _ => return,
        };
        let mut cur = LigCursor {
            cur_l: Some(cur_l),
            lig_present,
            lft_pending: false,
            rt_hit: false,
        };
        let bchar = if stack.is_empty() {
            cur_r
        } else {
            self.eqtb.fonts.get(f as usize).and_then(|font| font.bchar)
        };
        self.lig_kern_run(f, &mut cur, stack, cur_r, bchar);
    }

    /// tex.web §1036-1040 main_lig_loop, materializing as it goes: the
    /// tail of `cur_list` is cur_l (an open ligature node while
    /// `ligature_present`). Returns when the cursor reaches the stack's
    /// last input character (main_loop_lookahead) or the boundary.
    fn lig_kern_run(
        &mut self,
        f: u16,
        cur: &mut LigCursor,
        mut stack: LigStack,
        mut cur_r: Option<u8>,
        mut bchar: Option<u8>,
    ) {
        let Some(font) = self.eqtb.fonts.get(f as usize).cloned() else {
            return;
        };
        loop {
            let step = cur_r.and_then(|r| lig_kern_step(&font, cur.cur_l, r));
            let wrap_then_move = match step {
                None => true,
                Some(LigKernOp::Kern(w)) => {
                    self.lig_wrapup(f, cur, stack.is_empty(), true);
                    self.cur_list.push(Node::Kern(w));
                    false
                }
                Some(LigKernOp::Lig { op, ch }) => {
                    if cur.cur_l.is_none() {
                        cur.lft_pending = true;
                    } else if stack.is_empty() {
                        cur.rt_hit = true;
                    }
                    match op {
                        // =:| and =:|> — the left character becomes the ligature
                        1 | 5 => self.lig_set_left(f, cur, ch, None),
                        // |=: and |=:> — the right character becomes the ligature
                        2 | 6 => {
                            cur_r = Some(ch);
                            match stack.last_mut() {
                                None => {
                                    stack.push(LigItem {
                                        ch,
                                        is_char: false,
                                        orig: None,
                                    });
                                    bchar = None;
                                }
                                Some(top) if top.is_char => {
                                    *top = LigItem {
                                        ch,
                                        is_char: false,
                                        orig: Some(top.ch),
                                    }
                                }
                                Some(top) => top.ch = ch,
                            }
                        }
                        // |=:| — insert the ligature between them
                        3 => {
                            cur_r = Some(ch);
                            stack.push(LigItem {
                                ch,
                                is_char: false,
                                orig: None,
                            });
                        }
                        // |=:|> and |=:|>> — pass over the left character
                        7 | 11 => {
                            self.lig_wrapup(f, cur, stack.is_empty(), false);
                            cur.cur_l = None;
                            cur.lig_present = false;
                            self.lig_set_left(f, cur, ch, None);
                        }
                        // =: — both characters become the ligature
                        _ => {
                            if stack.is_empty() {
                                self.lig_set_left(f, cur, ch, None);
                                self.lig_wrapup(f, cur, true, true);
                                return;
                            }
                            let top = stack.pop().unwrap();
                            if top.is_char {
                                // main_loop_move+2: the character joins the
                                // ligature's components; look ahead
                                self.lig_set_left(f, cur, ch, Some(top.ch));
                                return;
                            }
                            self.lig_set_left(f, cur, ch, top.orig);
                            match stack.last() {
                                Some(next) => cur_r = Some(next.ch),
                                None if top.orig.is_some() => return,
                                None => cur_r = bchar,
                            }
                            continue;
                        }
                    }
                    if op > 4 && op != 7 {
                        true
                    } else {
                        continue;
                    }
                }
            };
            if wrap_then_move {
                self.lig_wrapup(f, cur, stack.is_empty(), true);
            }
            // main_loop_move: the cursor passes cur_l
            let Some(top) = stack.pop() else {
                return;
            };
            if top.is_char {
                self.cur_list.push(Node::Char { c: top.ch, font: f });
                return;
            }
            // main_loop_move_lig: a pseudo-ligature becomes cur_l
            cur.cur_l = None;
            cur.lig_present = false;
            self.lig_set_left(f, cur, top.ch, top.orig);
            match stack.last() {
                Some(next) => cur_r = Some(next.ch),
                None if top.orig.is_some() => return,
                None => cur_r = bchar,
            }
        }
    }

    /// cur_l := `ch` with ligature_present: the open ligature at the tail
    /// takes over cur_l's components (none at the left boundary), plus
    /// `consumed` when a character is absorbed.
    fn lig_set_left(&mut self, f: u16, cur: &mut LigCursor, ch: u8, consumed: Option<u8>) {
        let mut letters = [0u8; 3];
        let mut n = 0usize;
        let mut subtype = 0u8;
        if cur.cur_l.is_some() {
            match self.cur_list.pop() {
                Some(Node::Char { c, .. }) => {
                    letters[0] = c;
                    n = 1;
                }
                Some(Node::Ligature {
                    letters: l,
                    n_letters,
                    subtype: s,
                    ..
                }) => {
                    letters = l;
                    n = n_letters as usize;
                    subtype = s;
                }
                Some(other) => self.cur_list.push(other),
                None => {}
            }
        }
        if let Some(c) = consumed {
            if n < 3 {
                letters[n] = c;
                n += 1;
            }
        }
        // pack_lig: a ligature formed with the left boundary gets subtype 2
        if std::mem::take(&mut cur.lft_pending) {
            subtype = 2;
        }
        let (lig_width, lig_height, lig_depth) = self.char_dims(f, ch);
        self.cur_list.push(Node::Ligature {
            c: ch,
            font: f,
            lig_width,
            lig_height,
            lig_depth,
            letters,
            n_letters: n as u8,
            subtype,
        });
        cur.cur_l = Some(ch);
        cur.lig_present = true;
    }

    /// tex.web wrapup (§1035): close cur_l's ligature (marking a right
    /// boundary hit) and settle a trailing hyphen into a null discretionary.
    fn lig_wrapup(&mut self, f: u16, cur: &mut LigCursor, stack_empty: bool, rt: bool) {
        if cur.cur_l.is_none() {
            return;
        }
        if cur.lig_present {
            if rt && cur.rt_hit && stack_empty {
                if let Some(Node::Ligature { subtype, .. }) = self.cur_list.last_mut() {
                    *subtype += 1;
                }
                cur.rt_hit = false;
            }
            cur.lig_present = false;
        }
        if self.mode == Mode::Horizontal && self.tail_ends_hyphen(f) {
            self.cur_list.push(Node::Disc(crate::boxes::DiscNode {
                pre_break: Vec::new(),
                post_break: Vec::new(),
                no_break: Vec::new(),
                replace_count: 0,
            }));
        }
    }

    pub fn char_dims(&self, f: u16, c: u8) -> (i32, i32, i32) {
        match self.eqtb.fonts.get(f as usize) {
            Some(font) => (font.char_width(c), font.char_height(c), font.char_depth(c)),
            None => (0, 0, 0),
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
        if kind == DISC_GROUP_KIND {
            self.cur_list = outer_list;
            self.build_discretionary(shift, inner, outer_mode);
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

        if kind == 7 {
            self.cur_list = outer_list;
            self.finish_halign();
            return;
        }
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
                            // tex.web append_to_vlist: the glue node is
                            // appended even when it is zero (a legal
                            // \vsplit breakpoint)
                            self.cur_list.push(Node::Glue(glue));
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

    pub(crate) fn report_pack_warnings_at(
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
            self.pack_warning_at(&m, source.as_ref().map(|mark| mark.to_context()));
        }
        // etex.ch hpack exit: "Report LR problems" after the glue report
        if res.lr_problems > 0 {
            self.report_lr_problems(res.lr_problems, source.map(|mark| mark.to_context()));
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
            // etex.ch find_effective_tail: a trailing \endM is transparent
            _ => match self.cur_list.as_slice() {
                [.., Node::MathKern(_, boxes::END_M)] => {
                    self.cur_list.len().checked_sub(2).map(|i| &self.cur_list[i])
                }
                list => list.last(),
            },
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
            _ => {
                let n = self.cur_list.len();
                if n >= 2 && matches!(self.cur_list[n - 1], Node::MathKern(_, boxes::END_M)) {
                    // etex.ch fetch_effective_tail: take the node before the
                    // \endM and drop a \beginM\endM pair it leaves empty
                    let tx = self.cur_list.remove(n - 2);
                    if n >= 3 && matches!(self.cur_list[n - 3], Node::MathKern(_, boxes::BEGIN_M)) {
                        self.cur_list.truncate(n - 3);
                    }
                    Some(tx)
                } else {
                    self.cur_list.pop()
                }
            }
        }
    }

    pub fn last_node_type_value(&self) -> i32 {
        if self.write_mode_zero {
            return -1;
        }
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

    /// tex.web §1117 append_discretionary: a disc node joins the list and
    /// its three parts are typeset as restricted-horizontal groups.
    pub fn do_discretionary(&mut self) {
        self.flush_native_text();
        self.cur_list.push(Node::Disc(crate::boxes::DiscNode {
            pre_break: Vec::new(),
            post_break: Vec::new(),
            no_break: Vec::new(),
            replace_count: 0,
        }));
        self.begin_disc_part(0);
    }

    /// `new_save_level(disc_group); scan_left_brace; push_nest;
    /// mode:=-hmode; space_factor:=1000` for part `part` (0 pre-break,
    /// 1 post-break, 2 no-break); the part index rides in box_shifts.
    fn begin_disc_part(&mut self, part: i32) {
        self.skip_spaces_relax();
        let t = self.get_x_raw();
        if !self.token_is_left_brace(t) {
            self.error("Missing { inserted");
            self.push_token(t);
        }
        self.saved_lists.push((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
        ));
        self.push_group_level(LevelType::Box);
        self.box_targets.push(None);
        self.box_shifts.push(part);
        self.box_kinds.push(DISC_GROUP_KIND);
        self.mode = Mode::RestrictedHorizontal;
        self.space_factor = 1000;
    }

    /// tex.web §1119-1121 build_discretionary: keep only characters,
    /// ligatures, boxes, rules and kerns ("Improper discretionary list"
    /// flushes the rest), store the part in the disc node at the tail and
    /// open the next part; a nonempty no-break part is illegal in math.
    fn build_discretionary(&mut self, part: i32, mut list: NodeList, outer_mode: Mode) {
        // ratex's SyncTeX points are invisible bookkeeping, not list items
        list.retain(|n| !matches!(n, Node::Whatsit(crate::boxes::WhatIt::SyncPoint { .. })));
        if let Some(bad) = list.iter().position(|n| {
            !matches!(
                n,
                Node::Char { .. }
                    | Node::Ligature { .. }
                    | Node::NativeGlyphRun { .. }
                    | Node::Box { .. }
                    | Node::Rule { .. }
                    | Node::Kern(_)
                    | Node::ExplicitKern(_)
            )
        }) {
            self.error("Improper discretionary list");
            list.truncate(bad);
        }
        if part == 2 && outer_mode.is_m() && !list.is_empty() {
            self.error("Illegal math \\discretionary");
            list.clear();
        }
        let Some(Node::Disc(dc)) = self.cur_list.last_mut() else {
            return;
        };
        match part {
            0 => dc.pre_break = list,
            1 => dc.post_break = list,
            _ => dc.no_break = list,
        }
        if part < 2 {
            self.begin_disc_part(part + 1);
        } else if outer_mode.is_m() {
            if let Some(disc) = self.cur_list.pop() {
                self.append_mlist_node(disc);
            }
        }
    }

    pub fn take_last_box(&mut self) -> Option<Node> {
        if let Some(Node::Box { .. }) = self.current_tail() {
            self.take_current_tail()
        } else {
            None
        }
    }

    pub fn last_kern_value(&mut self) -> i32 {
        if self.write_mode_zero {
            return 0;
        }
        match self.current_tail() {
            Some(Node::Kern(k) | Node::ExplicitKern(k)) => *k,
            None if self.mode == Mode::Vertical => self.last_page_kern,
            _ => 0,
        }
    }

    pub fn last_penalty_value(&mut self) -> i32 {
        if self.write_mode_zero {
            return 0;
        }
        match self.current_tail() {
            Some(Node::Penalty(p)) => *p,
            None if self.mode == Mode::Vertical => self.last_page_penalty,
            _ => 0,
        }
    }

    pub fn last_skip_value(&mut self) -> Glue {
        if self.write_mode_zero {
            return Glue::zero();
        }
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
            let matches = t.is_char()
                && u8::try_from(t.chr()).is_ok_and(|actual| actual.eq_ignore_ascii_case(&expected));
            if !matches {
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
    pub fn active_cs_name(c: u32) -> Vec<u8> {
        debug_assert!(char::from_u32(c).is_some(), "invalid active character");
        let mut name = vec![0xFF, 0x00, b'A', b'C', b'T'];
        if let Ok(byte) = u8::try_from(c) {
            name.extend_from_slice(&[0x00, byte]);
        } else {
            name.push(0x01);
            name.extend_from_slice(&c.to_be_bytes());
        }
        name
    }

    pub fn active_cs_scalar(name: &[u8]) -> Option<u32> {
        if name.len() == 7 && name[..6] == [0xFF, 0x00, b'A', b'C', b'T', 0x00] {
            return Some(u32::from(name[6]));
        }
        if name.len() == 10 && name[..6] == [0xFF, 0x00, b'A', b'C', b'T', 0x01] {
            let scalar = u32::from_be_bytes(name[6..10].try_into().ok()?);
            return char::from_u32(scalar).map(u32::from);
        }
        None
    }

    /// Recover the physical bytes represented by an active-character control
    /// sequence. Byte-engine active characters retain their original byte;
    /// Unicode active characters use their scalar's UTF-8 encoding.
    pub(crate) fn active_cs_source_bytes(name: &[u8]) -> Option<([u8; 4], usize)> {
        if name.len() == 7 && name[..6] == [0xFF, 0x00, b'A', b'C', b'T', 0x00] {
            let mut bytes = [0; 4];
            bytes[0] = name[6];
            return Some((bytes, 1));
        }
        if name.len() == 10 && name[..6] == [0xFF, 0x00, b'A', b'C', b'T', 0x01] {
            let scalar = u32::from_be_bytes(name[6..10].try_into().ok()?);
            let character = char::from_u32(scalar)?;
            let mut bytes = [0; 4];
            let len = character.encode_utf8(&mut bytes).len();
            return Some((bytes, len));
        }
        None
    }

    pub fn active_cs_id(&mut self, c: u32) -> CsId {
        if let Some(id) = self.cs.cached_active(c) {
            return id;
        }
        let name = Self::active_cs_name(c);
        let id = match self.cs.lookup(&name) {
            Some(id) => id,
            None => self.cs.intern(&name),
        };
        self.cs.cache_active(c, id);
        id
    }

    pub fn active_cs_lookup(&self, c: u32) -> Option<CsId> {
        self.cs
            .cached_active(c)
            .or_else(|| self.cs.lookup(&Self::active_cs_name(c)))
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
                    // tex.web new_graf (§1091): in outer vmode \parskip glue
                    // is appended unconditionally
                    let ps = self.eqtb.glue_params[GlueParam::ParSkip.idx() as usize].clone();
                    self.page_list.push(Node::Glue(ps));
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
                // §1091: `if nest_ptr=1 then build_page` comes last, so an
                // output routine it fires is read BEFORE the \everypar
                // tokens (which then run in the paragraph, not the routine)
                self.build_page();
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

    /// \prevgraf belongs to the enclosing vertical nest, even in an hbox or
    /// math; it reads 0 while a `\write` expands (tex.web §422, mode 0).
    pub(crate) fn prev_graf(&self) -> i32 {
        if self.write_mode_zero {
            0
        } else if self.mode.is_v() {
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

        // tex.web §1096 end_graf ignores a paragraph only when the hlist is
        // STRUCTURALLY empty (`if head=tail then pop_nest`). Anything else
        // makes lines: glue alone (`\noindent\hskip5pt\par`) becomes one
        // empty line, and a list holding only a `\vadjust` — LaTeX's
        // `\end@float` in-text path plants `\vadjust{\penalty-\@Miv
        // \vbox{}\penalty\@floatpenalty}` with no further text — breaks into
        // a zero-width line whose adjustment post_line_break migrates into
        // the vertical list (treating it as abandoned stranded the float
        // box in `\@currlist`: `Float(s) lost`).
        // etex.ch end_graf: `if LR_save<>null then flush_list(LR_save)` —
        // init_math's line_break keeps it for the display and the resumption
        let lr_key = self.saved_lists.len();
        if self.cur_list.is_empty() {
            if !self.in_display_init {
                self.lr_save_take(lr_key);
            }
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

        self.end_char_chain();
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
        if !self.in_display_init {
            self.lr_save_take(lr_key);
        }
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

    /// Widths measured with TeX Live 2026 pdflatex (cmr10): ligatures and
    /// kerns form only inside one uninterrupted character chain; `{}`,
    /// `\relax` or an assignment end it, macro expansion does not.
    #[test]
    fn ligatures_and_kerns_stop_at_non_character_commands() {
        let cases = [
            (r"bc", "10.2778pt"),
            (r"b\relax c", "10.00002pt"),
            (r"b{}c", "10.00002pt"),
            (r"b\x c", "10.2778pt"),
            (r"f{}i", "5.83336pt"),
            (r"fi", "5.55557pt"),
            (r"b\count255=1 c", "10.00002pt"),
            (r"A\relax V", "15.00003pt"),
            (r"b\noboundary c", "10.00002pt"),
            (r"b\char`c", "10.2778pt"),
        ];
        for (text, wd) in cases {
            let mut engine = Engine::new(true);
            run_in(
                &mut engine,
                &format!(
                    "\\font\\cmr=cmr10 \\cmr \\def\\x{{}}\\setbox1\\hbox{{{text}}}\
                     \\ifdim\\wd1={wd}\\else\\errmessage{{\\the\\wd1}}\\fi"
                ),
            );
            assert_eq!(engine.error_count, 0, "{text}:\n{}", engine.diagnostic_output);
        }
    }

    /// tex.web make_accent: a \chardef'd base character (LaTeX's \i) is
    /// accented, and do_assignments runs font selections first.
    #[test]
    fn accent_takes_chardef_base_after_assignments() {
        let mut engine = Engine::new(true);
        run_in(
            &mut engine,
            "\\font\\cmr=cmr10 \\font\\big=cmr10 at 20pt \\cmr \\chardef\\i=16 \
             \\setbox1\\hbox{\\accent19 \\i}\
             \\ifdim\\wd1=2.77779pt\\else\\errmessage{a \\the\\wd1}\\fi\
             \\setbox1\\hbox{\\accent19 \\big\\relax\\i}\
             \\ifdim\\wd1=5.55557pt\\else\\errmessage{b \\the\\wd1}\\fi",
        );
        assert_eq!(engine.error_count, 0, "{}", engine.diagnostic_output);
    }

    /// TeX Live 2026: tex.web §108 badness rounds r³/2¹⁸ to nearest
    /// (+2¹⁷), and math-on/off nodes are \mathsurround wide, taking the
    /// value current inside the formula.
    #[test]
    fn pack_badness_and_math_surround_match_tex_live() {
        let mut engine = Engine::new(true);
        run_in(
            &mut engine,
            "\\catcode`\\$=3 \\hbadness=10000 \
             \\setbox1\\hbox to 5pt{\\hskip0pt plus 10pt}\
             \\ifnum\\badness=12 \\else\\errmessage{badness \\the\\badness}\\fi\
             \\mathsurround=2pt \\setbox1\\hbox{$\\kern1pt$}\
             \\ifdim\\wd1=5pt\\else\\errmessage{a \\the\\wd1}\\fi\
             \\setbox1\\hbox{$\\mathsurround=3pt \\kern1pt$}\
             \\ifdim\\wd1=7pt\\else\\errmessage{b \\the\\wd1}\\fi",
        );
        assert_eq!(engine.error_count, 0, "{}", engine.diagnostic_output);
    }

    /// tex.web §1117-1121: \discretionary parts are typeset (so \char and
    /// \hyphenchar work, as in LaTeX's \-), and anything but characters,
    /// boxes, rules and kerns is an "Improper discretionary list".
    #[test]
    fn discretionary_parts_are_typeset_lists() {
        let mut engine = Engine::new(true);
        run_in(
            &mut engine,
            "\\font\\cmr=cmr10 \\cmr \\hyphenchar\\cmr=45 \
             \\setbox1\\hbox{a\\discretionary{\\char\\hyphenchar\\font}{}{x}b}\
             \\ifdim\\wd1=15.83339pt\\else\\errmessage{wd \\the\\wd1}\\fi",
        );
        assert_eq!(engine.error_count, 0, "{}", engine.diagnostic_output);
        let Some(crate::boxes::Node::Box { list, .. }) = engine.eqtb.boxed[1].as_ref() else {
            panic!("box 1");
        };
        let disc = list
            .iter()
            .find_map(|n| match n {
                crate::boxes::Node::Disc(dc) => Some(dc),
                _ => None,
            })
            .expect("disc node");
        assert!(matches!(disc.pre_break[..], [crate::boxes::Node::Char { c: 45, .. }]));
        assert!(disc.post_break.is_empty());
        assert!(matches!(disc.no_break[..], [crate::boxes::Node::Char { c: b'x', .. }]));

        let mut engine = Engine::new(true);
        run_in(
            &mut engine,
            "\\setbox1\\hbox{\\discretionary{\\hskip1pt}{}{}}\
             \\ifdim\\wd1=0pt\\else\\errmessage{wd \\the\\wd1}\\fi",
        );
        assert_eq!(engine.error_count, 1, "{}", engine.diagnostic_output);
        assert!(engine.diagnostic_output.contains("Improper discretionary list"));
    }

    /// tex.web §1096 end_graf: only a structurally empty list (`head=tail`)
    /// is ignored; TeX Live 2026 sets one empty line for each of these.
    #[test]
    fn paragraph_of_only_glue_kern_or_penalty_makes_a_line() {
        for (text, wd, lines) in [
            (r"\noindent", "0pt", 0),
            (r"\noindent\hskip5pt", "100pt", 1),
            (r"\noindent\penalty0", "100pt", 1),
            (r"\noindent\kern2pt", "100pt", 1),
            (r"\noindent\vadjust{}", "100pt", 1),
        ] {
            let mut engine = Engine::new(true);
            run_in(
                &mut engine,
                &format!(
                    "\\hsize=100pt \\parfillskip=0pt plus 1fil \
                     \\setbox1\\vbox{{{text}\\par\\xdef\\pg{{\\the\\prevgraf}}}}\
                     \\ifdim\\wd1={wd}\\else\\errmessage{{wd \\the\\wd1}}\\fi\
                     \\ifnum\\pg={lines} \\else\\errmessage{{prevgraf \\pg}}\\fi"
                ),
            );
            assert_eq!(engine.error_count, 0, "{text}:\n{}", engine.diagnostic_output);
        }
    }

    /// tex.web §1034-1040 boundary ligatures and every ligature op, on
    /// tests/fixtures/bndlig.tfm (pltotf of bndlig.pl: left boundary
    /// program, right boundary char `Z` that is not a character, widths
    /// a=1 b=2 c=4 d=8 x=16 y=32pt). Widths measured with TeX Live 2026
    /// pdftex -ini.
    #[test]
    fn boundary_ligatures_and_ligature_ops_match_tex_live() {
        let tfm = include_bytes!("../tests/fixtures/bndlig.tfm");
        let font = crate::tfm::parse_tfm(tfm, "bndlig", 0).expect("bndlig.tfm");
        for (text, wd) in [
            ("a", "22pt"),
            ("b", "31.99998pt"),
            ("c", "21.99997pt"),
            ("d", "31.99998pt"),
            ("ab", "39.99997pt"),
            ("ba", "40.99998pt"),
            ("ac", "35.99998pt"),
            ("ca", "59.99995pt"),
            ("dd", "39.99998pt"),
            ("bab", "58.99995pt"),
            ("cab", "77.99992pt"),
            (r"\noboundary b", "17.99998pt"),
            (r"b\noboundary", "31.99998pt"),
            (r"a\noboundary", "17pt"),
            ("aZ", "17pt"),
            ("xc", "21.99997pt"),
        ] {
            let mut engine = Engine::new(true);
            engine.init_primitives();
            engine.add_nullfont();
            let cs = engine.cs.intern(b"bnd");
            let id = engine.push_engine_font(std::rc::Rc::new(font.clone()), cs);
            engine.eqtb.assign(cs, crate::eqtb::Equiv::FontRef(id), true);
            let input = format!(
                "\\catcode`\\{{=1 \\catcode`\\}}=2 \\bnd \\setbox1\\hbox{{{text}}}\
                 \\ifdim\\wd1={wd}\\else\\errmessage{{wd \\the\\wd1}}\\fi\n"
            );
            engine.input.push_file("bndlig.tex".into(), input.into_bytes());
            engine.run();
            assert_eq!(engine.error_count, 0, "{text}:\n{}", engine.diagnostic_output);
        }
    }

    /// tex.web §668/§633: a running \hrule width stays running through
    /// vpack and takes the enclosing box's width at shipout. TeX Live 2026
    /// draws a 99.626bp (100pt) rule after \unvcopy into a 100pt-wide box
    /// and a 9.963bp one after \wd0=10pt.
    #[test]
    fn running_rule_width_resolves_at_shipout() {
        let mut engine = Engine::new(true);
        run_in(
            &mut engine,
            "\\setbox0\\vbox{\\hrule}\\setbox1\\vbox{\\unvcopy0 \\hbox to100pt{}}\
             \\ifdim\\wd0=0pt\\else\\errmessage{wd0 \\the\\wd0}\\fi\\shipout\\box1 \
             \\setbox0\\vbox{\\hrule height 2pt}\\wd0=10pt \\shipout\\box0 \\end",
        );
        assert_eq!(engine.error_count, 0, "{}", engine.diagnostic_output);
        let widths: Vec<f64> = engine
            .pdf_doc
            .pages
            .iter()
            .flat_map(|page| page.display_list.as_ref().expect("display list").items.iter())
            .filter_map(|item| match item {
                crate::boxes::DisplayItem::Rule { width_bp, .. } => Some(*width_bp),
                _ => None,
            })
            .collect();
        assert_eq!(widths.len(), 2, "{widths:?}");
        assert!((widths[0] - 99.626).abs() < 0.001, "{widths:?}");
        assert!((widths[1] - 9.963).abs() < 0.001, "{widths:?}");
    }

    /// tex.web §1091 new_graf: build_page runs after \everypar is queued,
    /// so an output routine fired by the new paragraph's \parskip runs
    /// BEFORE the \everypar tokens. TeX Live 2026 records `EEOE`.
    #[test]
    fn output_fired_by_new_paragraph_precedes_everypar() {
        let mut engine = Engine::new(true);
        run_in(
            &mut engine,
            "\\font\\cmr=cmr10 \\cmr \\hsize=100pt \\vsize=20pt \\baselineskip=12pt \
             \\parindent=0pt \\parfillskip=0pt plus 1fil \\maxdepth=2pt \\topskip=10pt \
             \\def\\seq{} \\everypar{\\xdef\\seq{\\seq E}}\
             \\output{\\xdef\\seq{\\seq O}\\setbox0\\box255 \\deadcycles=0 }\
             a\\par b\\par c\\par \\def\\want{EEOE}\
             \\ifx\\seq\\want\\else\\errmessage{order \\seq}\\fi",
        );
        assert_eq!(engine.error_count, 0, "{}", engine.diagnostic_output);
    }
}
