//! List building: characters, glue, kerns, penalties, rules, box groups,
//! paragraphs, page-builder hook.

use crate::boxes::{self, Glue, Node, NodeList};
use crate::eqtb::LevelType;
use crate::engine::{Engine, Mode};
use crate::fontiface::LigKernStep;
use crate::prim::{DimParam, GlueParam, IntParam, Prim};
use crate::scaled::{mult, ONE};
use crate::token::{CsId, Token};

pub const RULE_FILL: i32 = i32::MIN; // sentinel: rule dimension from context

// pending \leaders object kinds (a/c/x), one entry per open leader box
// group; completed in end_box. Engine is single-threaded (Rc state), so a
// thread_local avoids touching the Engine struct.
thread_local! {
    static LEADER_KINDS: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Engine {
    pub fn font_resolver(&self) -> &dyn crate::fonts::FontResolver {
        self
    }

    // ---------- characters & spaces ----------

    pub fn hspace_token(&mut self) {
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                let g = self.interword_glue();
                self.cur_list.push(Node::Glue(g));
                self.space_factor = 1000;
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
        // tex.web: interword glue comes from fontdimen 2/3/4 of the CURRENT
        // font as seen through \fontdimen assignments — eqtb.font_params is
        // the overlay that \fontdimen writes (control.rs FontDimen); the raw
        // TFM params on the font object must not shadow it (real tex's
        // dominant stretch for newtx@12pt is the ADJUSTED 2.39996pt, not the
        // raw 2.39758pt).
        let fp = self.eqtb.font_params.get(f as usize);
        let fd = |i: usize| -> Option<i32> { fp.and_then(|v| v.get(i).copied()) };
        let mut g = if let Some(font) = self.eqtb.fonts.get(f as usize) {
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
        // tex.web app_space (§1057): \spaceskip (sf<2000) and \xspaceskip
        // (sf>=2000) are taken as-is; only the font-space fallback has its
        // stretch and shrink multiplied by sf/1000, with \extraspace added
        // to the width when sf>=2000
        let ss = self.eqtb.glue_params[GlueParam::SpaceSkip.idx() as usize].clone();
        let ss_nz = ss.width != 0 || ss.stretch != 0 || ss.shrink != 0;
        let xs = self.eqtb.glue_params[GlueParam::XSpaceSkip.idx() as usize].clone();
        let xs_nz = xs.width != 0 || xs.stretch != 0 || xs.shrink != 0;
        let sf = self.space_factor.max(1) as i64;
        if ss_nz && sf < 2000 {
            return ss;
        }
        if xs_nz {
            return xs;
        }
        if sf >= 2000 {
            if let Some(font) = self.eqtb.fonts.get(f as usize) {
                g.width += font.extra_space();
            }
        }
        g.stretch = ((g.stretch as i64) * sf / 1000) as i32;
        // tex.web app_space: shrink is scaled by 1000/sf (inverse of stretch)
        g.shrink = ((g.shrink as i64) * 1000 / sf) as i32;
        g
    }

    /// tex.web ex_space (control space `\ `): append_normal_space — plain
    /// interword glue of the current font (or \spaceskip as-is when set),
    /// WITHOUT space-factor scaling of stretch/shrink and without
    /// \fontdimen7 extra space. \spacefactor is left unchanged.
    pub fn ex_space(&mut self) {
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
                    self.cur_list.push(Node::Glue(g));
                }
            }
            Mode::Vertical | Mode::InternalVertical => {}
        }
    }

    pub fn char_token(&mut self, c: u8, is_letter: bool) {

        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                self.append_char(c);
                self.space_factor = self.space_factor_of(c);
            }
            Mode::Vertical | Mode::InternalVertical => {
                // tex.web §1091: back_input the letter, new_graf(true).
                // LaTeX \\everypar (\\g__para_standard_everypar_tl) runs
                // \\tex_par:D to cancel that dummy paragraph; the letter
                // must not already be on the list or it becomes its own para.
                let cc = if is_letter { 11 } else { 12 };
                self.pushed.push(Token::char(cc, c as u32));
                self.start_paragraph(true);
            }
            Mode::Math | Mode::DisplayMath => {
                // `_`/`^` cat 13 still have to subscript/superscript. The
                // active `\_` body is other-`_`, whose mathcode 0x8000 would
                // re-expand forever.
                if c == b'_' {
                    self.sub_token(c);
                    return;
                }
                if c == b'^' {
                    self.super_token(c);
                    return;
                }
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
            if main_s > 0 { main_s } else { self.space_factor }
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
            Some(crate::eqtb::Equiv::Macro(m)) => self.expand_macro(id, &m),
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
            Prim::HFilNeg => Glue { width: 0, stretch: -ONE, shrink: 0, stretch_order: 1, shrink_order: 0 },
            Prim::HSS => Glue { width: 0, stretch: ONE, shrink: ONE, stretch_order: 1, shrink_order: 1 },
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
            Prim::VFilNeg => Glue { width: 0, stretch: -ONE, shrink: 0, stretch_order: 1, shrink_order: 0 },
            Prim::VSS => Glue { width: 0, stretch: ONE, shrink: ONE, stretch_order: 1, shrink_order: 1 },
            _ => Glue::zero(),
        }
    }

    pub fn append_h_glue(&mut self, g: Glue, leader: bool) {
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

    /// appends to the current vertical list; at outer level the page builder
    /// runs only for box-like appends — tex.web triggers build_page on box
    /// appends / paragraph ends, NOT on \vskip/\penalty, so glue and penalty
    /// nodes stay visible to \lastskip/\lastpenalty until the next box
    /// (LaTeX's \addpenalty/\@xaddvskip compensation dances depend on this)
    pub fn vlist_append(&mut self, n: Node) {
        if self.mode == Mode::Vertical {
            let trigger = matches!(
                n,
                Node::Box { .. } | Node::Rule { .. } | Node::Ins { .. } | Node::Penalty(_)
            );
            self.page_list.push(n);
            if trigger {
                self.build_page();
            }
        } else {
            self.cur_list.push(n);
        }
    }

    pub fn append_whatsit(&mut self, n: Node) {
        match self.mode {
            Mode::Vertical | Mode::InternalVertical => self.vlist_append(n),
            _ => self.cur_list.push(n),
        }
    }

    // ---------- characters with ligatures & kerns ----------

    pub fn append_char(&mut self, c: u8) {
        let f = self.eqtb.cur_font_val;
        if f == 0 {
            // real TeX nullfont: chars are silently dropped (no error)
            return;
        }
        // tex.web main_loop wrapup: an empty discretionary follows every
        // input char equal to the font's \hyphenchar, giving a legal line
        // break after the hyphen at \exhyphenpenalty (pre_break=null →
        // try_break(ex_hyphen_penalty, hyphenated) in line_break)
        let hc = self.eqtb.hyphen_char.get(f as usize).copied().unwrap_or(-1);
        let explicit_hyphen = (0..=255).contains(&hc) && c as i32 == hc;
        self.append_char_lig(c, f);
        if explicit_hyphen {
            self.cur_list.push(Node::Disc(crate::boxes::DiscNode {
                pre_break: Vec::new(),
                post_break: Vec::new(),
                no_break: Vec::new(),
                replace_count: 0,
            }));
        }
    }

    fn append_char_lig(&mut self, c: u8, f: u16) {
        // ligature & kern with previous char (either Char or an already-formed Ligature)
        let prev: Option<(u8, [u8; 3], u8)> = match self.cur_list.last() {
            Some(Node::Char { c: pc, font: pf }) if *pf == f => Some((*pc, [0; 3], 0)),
            Some(Node::Ligature { c: lc, font: pf, letters, n_letters, .. }) if *pf == f => {
                Some((*lc, *letters, *n_letters))
            }
            _ => None,
        };
        if let Some((pc, pletters, pn)) = prev {
            if let Some(step) = self.find_lig_kern(f, pc, c) {
                if step.is_kern {
                    self.cur_list.push(Node::Kern(step.kern_amount));
                    self.cur_list.push(Node::Char { c, font: f });
                    return;
                } else {
                    // ligature: replace previous char/ligature, recording the
                    // component letters (fi + l -> ffi keeps [f, i, l])
                    let _ = self.cur_list.pop();
                    let lc = step.lig_char;
                    let dims = self.char_dims(f, lc);
                    let mut letters = [0u8; 3];
                    let base: &[u8] = if pn > 0 { &pletters[..pn as usize] } else { &[pc] };
                    let n = (base.len() + 1).min(3);
                    letters[..base.len().min(3)].copy_from_slice(&base[..base.len().min(3)]);
                    if base.len() < 3 {
                        letters[base.len()] = c;
                    }
                    self.cur_list.push(Node::Ligature { c: lc, font: f, lig_width: dims.0, lig_height: dims.1, lig_depth: dims.2, letters, n_letters: n as u8 });
                    if step.keep_right {
                        // re-add the new char after lig (iterate)
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
                    return Some(LigKernStep { is_kern: true, kern_amount: amt, lig_char: 0, keep_left: false, keep_right: false, iterate: false });
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
            if width == RULE_FILL {
                width = self.eqtb.dim_params[DimParam::HSize.idx() as usize];
            }
            self.prev_depth = -1000 * 65536;
            self.vlist_append(Node::Rule { width, height, depth });
            return;
        }
        if width == RULE_FILL {
            width = crate::scaled::ONE * 2 / 5;
        }
        let node = Node::Rule { width, height, depth };
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => self.cur_list.push(node),
            Mode::Math | Mode::DisplayMath => self.append_mlist_node(node),
            _ => self.error("\\vrule outside horizontal mode"),
        }
    }

    // ---------- boxes ----------

    fn token_is_left_brace(&self, t: Token) -> bool {
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
        if crate::debug_flag("IFTRACE") {
            let src = match self.input.stack.last() {
                Some(crate::input::Source::TokList { name, .. }) => name.clone(),
                Some(crate::input::Source::File { name, .. }) => format!("F:{}", name),
                None => String::new(),
            };
            eprintln!("BEGIN-BOX kind={} mode={:?} line={} src={} prim={:?} cur={:?} kinds={:?} ring=[{}] pushed_top={:?}", kind, self.mode, self.input.current_file_line(), src, self.cur_prim,
                { let t = self.cur_tok; if t.is_cs() { String::from_utf8_lossy(self.cs.name(t.cs_id())).into_owned() } else { format!("{:#x}", t.0) } },
                self.box_kinds,
                self.tok_ring.iter().rev().take(14).map(|(v, _ln)| { let t = Token(*v); if t.is_cs() { String::from_utf8_lossy(self.cs.name(t.cs_id())).into_owned() } else if t.is_char() { format!("cc{}:{:?}", t.cc(), t.chr() as u8 as char) } else { format!("{:#x}", t.0) } }).collect::<Vec<_>>().join(" "),
                self.pushed.last().map(|t| if t.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) } else { format!("{:#x}", t.0) }).unwrap_or_default());
        }
        // scan "to"/"spread" target (tex.web scan_spec uses scan_keyword
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
                format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id())))
            } else {
                format!("cc{}:{:#x}", t.cc(), t.chr())
            };
            self.error(&format!("Missing {{ inserted (got {})", got));
            self.pushed.push(t);
            self.pushed.push(Token::char(1, b'{' as u32));
        }
        let shift = match self.pending_box_shift.take() {
            Some((d, _)) => d,
            None => 0,
        };
        if crate::debug_flag("IFTRACE") { eprintln!("PUSH-BG437"); }
        // save the outer list context: mode, current list, prev_depth, space_factor
        self.saved_lists.push((self.mode, std::mem::take(&mut self.cur_list), self.prev_depth, self.space_factor));
        self.eqtb.push_level(LevelType::Box);

        self.box_targets.push(target);
        self.box_shifts.push(shift);
        self.box_kinds.push(kind);
        match kind {
            0 => self.mode = Mode::RestrictedHorizontal,
            1 | 2 | 9 => {
                self.mode = Mode::InternalVertical;
                self.prev_depth = -1000 * 65536;
            }
            _ => self.mode = Mode::InternalVertical,
        }
    }

    /// called on the matching `}` for a box group or plain group
    pub fn end_box(&mut self) {
        if crate::debug_flag("IFTRACE") {
            eprintln!("ENDBOX kinds={} saved={} targets={} pars={}", self.box_kinds.len(), self.saved_lists.len(), self.box_targets.len(), self.par_saves);
        }
        if self.box_kinds.is_empty() {
            eprintln!("TMBOX ring=[{}] pushed={:?}",
                self.tok_ring.iter().rev().take(24).map(|(v, ln)| format!("{v:#x}@{ln}")).collect::<Vec<_>>().join(" "),
                self.pushed.iter().rev().take(8).map(|x| format!("{:#x}", x.0)).collect::<Vec<_>>());
            self.error("Too many }'s");
            return;
        }
        let kind = self.box_kinds.pop().unwrap_or(0);
        // tex.web end_gracefully: closing a vertical box group while a
        // paragraph is running inside it forces the \par first, so the
        // packed lines join the vbox instead of being vpack-discarded
        if matches!(kind, 1 | 2 | 3 | 8 | 9) && self.mode == Mode::Horizontal {
            self.par_primitive();
        }
        let target = self.box_targets.pop().flatten();
        let shift = self.box_shifts.pop().unwrap_or(0);
        let inner = std::mem::replace(&mut self.cur_list, Vec::new());
        let (outer_mode, outer_list, pd, sf) = self.saved_lists.pop().unwrap_or((
            // group desync (e.g. runaway end): stay in the current context
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
        ));
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
            } else {
                self.cur_list.push(Node::VAdjust(inner));
            }
            return;
        }
        // tex.web package(): vboxes are packed against \boxmaxdepth
        let max_depth = self.eqtb.dim_params[DimParam::BoxMaxDepth.idx() as usize];
        let pack = |list: Vec<Node>, target: Option<(i32, bool)>, k: u8| -> boxes::PackResult {
            let (dim, spread) = match target {
                Some((d, sp)) => (Some(d), sp),
                None => (None, false),
            };
            match k {
                0 | 6 => boxes::hpack_add(list, dim, spread, boxes::HBOX, &self.eqtb),
                1 | 5 | 8 => boxes::vpack_add_md(list, dim, spread, boxes::VBOX, &self.eqtb, max_depth),
                2 => boxes::vtop_md(list, dim, spread, &self.eqtb, max_depth),
                3 => {
                    // \vcenter: natural packing, recentered via shift
                    let mut r = boxes::vpack_add_md(list, dim, spread, boxes::VBOX, &self.eqtb, i32::MAX);
                    if let Node::Box { h: h0, d: d0, shift, .. } = &mut r.node {
                        *shift = (*h0 - *d0) / 2;
                    }
                    r
                }
                _ => boxes::hpack_add(list, None, false, boxes::HBOX, &self.eqtb),
            }
        };
        let res = pack(inner, target, kind);
        self.last_badness = res.badness;
        match kind {
            0 | 1 | 2 | 8 => self.report_pack_warnings(&res),
            _ => {}
        }
        let mut node = res.node;
        if let Node::Box { shift: s, .. } = &mut node {
            *s += shift;
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
        // \insert group: wrap the packed vbox into an insert node
        if kind == 8 {
            if let Some(num) = self.insert_nums.pop() {
                let (h, d) = match &node {
                    Node::Box { h, d, .. } => (*h, *d),
                    _ => (0, 0),
                };
                let cost = std::mem::replace(
                    &mut self.eqtb.int_params[IntParam::InsertPenalties.idx() as usize],
                    0,
                );
                node = Node::Ins { num, height: h, depth: d, cost, box_node: Box::new(node) };
            }
        }
        // a leader-object box completes a \leaders group
        if matches!(kind, 0..=3) {
            if let Some(lk) = self.pop_leader_kind() {
                if self.setbox_target.is_some() && self.setbox_depth == self.box_kinds.len() {
                    self.unpark_setbox();
                    self.error("A <box> was supposed to be here");
                }
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
        // target: an inner \\hbox inside the content must append instead
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
        self.append_box_node(Some(node));
    }

    pub fn append_box_node(&mut self, b: Option<Node>) {
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
                            let bs = self.eqtb.glue_params[crate::prim::GlueParam::BaselineSkip.idx() as usize].clone();
                            let ls = self.eqtb.glue_params[crate::prim::GlueParam::LineSkip.idx() as usize].clone();
                            let lsl = self.eqtb.dim_params[crate::prim::DimParam::LineSkipLimit.idx() as usize];
                            let diff = bs.width as i64 - self.prev_depth as i64 - *h as i64;
                            let glue = if diff < lsl as i64 {
                                 ls
                             } else {
                                 Glue { width: diff as i32, ..bs }
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
            // tex.web §1110: \\unhbox/\\unvbox voids the register globally
            let old = self.eqtb.boxed.get(n as usize).cloned().flatten();
            self.eqtb.assign_box(n, None, true);
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
                if !want_v && self.mode.is_v() {
                    self.start_paragraph(false);
                }
                if want_v && self.mode.is_h() {
                    self.error("Incompatible list can't be unboxed");
                    return;
                }

                for item in list {
                    if self.mode.is_v() {
                        self.vlist_append(item);
                    } else {
                        self.cur_list.push(item);
                    }
                }
            }
            _ => self.error("Incompatible list can't be unboxed"),
        }
    }


    fn pop_leader_kind(&self) -> Option<u8> {
        LEADER_KINDS.with(|s| s.borrow_mut().pop())
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

    /// append a leader node. tex.web scan_box/box_end leader context.
    pub fn begin_leaders(&mut self, kind: u8) {
        use boxes::LeaderBody;
        self.skip_spaces_relax();
        let t = self.get_token();
        match self.box_prim_or_name(t) {
            Some(Prim::HBox) => {
                LEADER_KINDS.with(|s| s.borrow_mut().push(kind));
                self.begin_box(0);
            }
            Some(Prim::VBox) => {
                LEADER_KINDS.with(|s| s.borrow_mut().push(kind));
                self.begin_box(1);
            }
            Some(Prim::VTop) => {
                LEADER_KINDS.with(|s| s.borrow_mut().push(kind));
                self.begin_box(2);
            }
            Some(Prim::VCenter) => {
                LEADER_KINDS.with(|s| s.borrow_mut().push(kind));
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
                self.finish_leaders(kind, LeaderBody::Rule { width: w, height: h, depth: d });
            }
            Some(Prim::VRule) => {
                let (w, h, d) = self.scan_rule_dims(false);
                self.finish_leaders(kind, LeaderBody::Rule { width: w, height: h, depth: d });
            }
            _ => {
                self.pushed.push(t);
                self.error("A <box> was supposed to be here");
            }
        }
    }

    /// scan the glue spec that follows a leader object and append the
    /// leader node ("Leaders not followed by proper glue" otherwise).
    fn finish_leaders(&mut self, kind: u8, body: boxes::LeaderBody) {
        use crate::prim::Prim;
        self.skip_spaces_relax();
        let t = self.get_token();
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
            Some(Prim::HSkip) | Some(Prim::HFil) | Some(Prim::HFill) | Some(Prim::HFilL)
                | Some(Prim::HFilNeg) | Some(Prim::HSS)
        );
        let is_v_glue = matches!(
            p,
            Some(Prim::VSkip) | Some(Prim::VFil) | Some(Prim::VFill) | Some(Prim::VFilL)
                | Some(Prim::VFilNeg) | Some(Prim::VSS)
        );
        let want_h = !self.mode.is_v();
        let glue = if want_h && is_h_glue {
            self.scan_hskip_kind(p.unwrap())
        } else if !want_h && is_v_glue {
            self.scan_vskip_kind(p.unwrap())
        } else {
            self.pushed.push(t);
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
    pub fn report_pack_warnings(&mut self, res: &boxes::PackResult) {
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
        let at = if self.in_output {
            "has occurred while \\output is active".to_string()
        } else {
            format!("detected at line {}", self.input.current_file_line())
        };
        let mut msg: Option<String> = None;
        if x > 0 && res.order == 0 {
            // underfull / loose (includes badness 10000 when nothing stretches)
            if res.badness > bad_param {
                let kw = if res.badness > 100 { "Underfull" } else { "Loose" };
                msg = Some(format!("{} {} (badness {}) {}", kw, obj, res.badness, at));
            }
        } else if x < 0 && res.order == 0 {
            if res.badness == 1_000_000 {
                let excess = -x - res.shrink[0];
                if excess > fuzz as i64 || bad_param < 100 {
                    msg = Some(format!(
                        "Overfull {} ({}pt {}) {}",
                        obj,
                        print_scaled(excess),
                        too,
                        at
                    ));
                }
            } else if res.badness > bad_param {
                msg = Some(format!("Tight {} (badness {}) {}", obj, res.badness, at));
            }
        }
        if let Some(m) = msg {
            self.diagnostic(&m);
        }
    }

    /// diagnostics go to the log; terminal only when \tracingonline>0
    fn diagnostic(&mut self, msg: &str) {
        self.log.push_str(msg);
        self.log.push('\n');
        if self.eqtb.int_params[IntParam::TracingOnline.idx() as usize] > 0 {
            self.term.push_str(msg);
            self.term.push('\n');
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
        let idx = self.scan_reg_num();
        self.scan_optional_equals();
        let v = self.scan_dimen(false, false);
        if let Some(Node::Box { w, h, d, .. }) =
            self.eqtb.boxed.get_mut(idx as usize).and_then(|o| o.as_mut())
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
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => self.cur_list.push(n),
            Mode::Vertical | Mode::InternalVertical => self.vlist_append(n),
            Mode::Math | Mode::DisplayMath => self.append_mlist_node(n),
        }
    }

    // ---------- moves (raise/lower/moveleft/moveright) ----------

    pub fn box_move(&mut self, d: i32, negate: bool, horizontal: bool) {
        let d = if negate { -d } else { d };
        if horizontal {
            // \moveleft/\moveright: shift is the horizontal offset of a box
            // appended in vertical mode (tex.web: shift := box_context)
            self.pending_box_shift = Some((d, true));
            self.skip_spaces_relax();
            let t = self.get_token();
            if self.box_prim_or_name(t).is_some() || (t.is_cs() && self.cs.name(t.cs_id()) == b"usebox") {
                self.pushed.push(t);
                self.scan_box_after_move();
            } else {
                self.pushed.push(t);
                self.pending_box_shift = None;
                self.error("A <box> was supposed to be here");
            }
        } else {
            self.pending_box_shift = Some((d, false));
            self.skip_spaces_relax();
            let t = self.get_token();
            if self.box_prim_or_name(t).is_some() || (t.is_cs() && self.cs.name(t.cs_id()) == b"usebox") {
                self.pushed.push(t);
                self.scan_box_after_move();
            } else {
                self.pushed.push(t);
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
            self.pushed.push(t);
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
                    self.pushed.push(t);
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
    /// tex.web tail-of-current-list: in outer vmode the page builder has
    /// consumed page_list[..page_processed], so \lastskip/\lastpenalty/
    /// \lastkern/\lastbox/\unskip/\unkern/\unpenalty/\lastnodetype must see
    /// only the UNCONSUMED contributions (real TeX moves consumed nodes off
    /// the vlist; reading consumed ones made \lastskip report stale glue and
    /// broke LaTeX's \addvspace `\ifdim\lastskip=\z@` branching).
    fn current_tail(&self) -> Option<&Node> {
        match self.mode {
            Mode::Vertical => self
                .page_list
                .get(self.page_processed.max(0) as usize..)
                .and_then(|s| s.last()),
            _ => self.cur_list.last(),
        }
    }

    fn take_current_tail(&mut self) -> Option<Node> {
        match self.mode {
            Mode::Vertical => {
                if self.page_list.len() > self.page_processed.max(0) as usize {
                    self.page_list.pop()
                } else {
                    None
                }
            }
            _ => self.cur_list.pop(),
        }
    }

    pub fn last_node_type_value(&self) -> i32 {
        match self.current_tail() {
            None => -1,
            Some(Node::Char { .. }) => 0,
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
            | Some(Node::OpLimits { .. })
            | Some(Node::Accent { .. })
            | Some(Node::MathKern(..)) => 10,
            Some(Node::Glue(_)) | Some(Node::Leaders { .. }) => 11,
            Some(Node::Kern(_)) | Some(Node::ExplicitKern(_)) => 12,
            Some(Node::Penalty(_)) => 13,
            Some(Node::InsDisc) | Some(Node::Empty) => 14,
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
        match self.current_tail() {
            Some(Node::Kern(k) | Node::ExplicitKern(k)) => *k,
            _ => 0,
        }
    }

    pub fn last_penalty_value(&mut self) -> i32 {
        match self.current_tail() {
            Some(Node::Penalty(p)) => *p,
            _ => 0,
        }
    }

    pub fn last_skip_value(&mut self) -> Glue {
        match self.current_tail() {
            Some(Node::Glue(g)) => g.clone(),
            Some(Node::Leaders { glue, .. }) => glue.clone(),
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
        // \vsplit<n> to <dimen>: the top part becomes the box result, the
        // remainder is written back to the source register (tex.web @958)
        let (top, n, rest) = self.scan_vsplit();
        if let Some(rest) = rest {
            self.stash_vsplit_remainder(n, rest);
        }
        self.append_box_node(top);
    }

    /// scan `\vsplit<n> to <dimen>`: split box n at the target height and
    /// return (top part, source register, remainder). The source register is
    /// emptied by the scan; the remainder must be re-stored via
    /// `stash_vsplit_remainder`.
    pub fn scan_vsplit(&mut self) -> (Option<Node>, u16, Option<NodeList>) {
        let n = self.scan_reg_num();
        self.scan_keyword(b"to");
        let target = self.scan_dimen(false, false);
        match self.eqtb.boxed[n as usize].take() {
            Some(b) => {
                self.vsplat_remainder = None;
                let top = self.vsplit_box(b, target);
                (top, n, self.vsplat_remainder.take())
            }
            // splitting a void box: void result, empty remainder vbox
            None => (None, n, Some(Vec::new())),
        }
    }

    /// re-pack a `\vsplit` remainder into the source box register (tex.web
    /// stores `vpackage(rest, 0, additional, \boxmaxdepth)` in \box n)
    pub fn stash_vsplit_remainder(&mut self, n: u16, rest: NodeList) {
        // tex.web @977: an empty remainder leaves the source box void,
        // not an empty vbox (`\\ifvoid` must be true).
        if rest.is_empty() {
            self.eqtb.assign_box(n, None, true);
            return;
        }
        let md = self.eqtb.dim_params[DimParam::BoxMaxDepth.idx() as usize];
        let r = boxes::vpack_add_md(rest, None, false, boxes::VBOX, &self.eqtb, md);
        self.eqtb.assign_box(n, Some(r.node), true);
    }

    pub fn scan_keyword(&mut self, kw: &[u8]) -> bool {
        self.skip_spaces_relax();
        let mut collected: Vec<Token> = Vec::new();
        for &expected in kw {
            let t = self.get_token();
            collected.push(t);
            if !t.is_char() || (t.chr() as u8).to_ascii_lowercase() != expected.to_ascii_lowercase() {
                self.push_tokens(collected);
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
        self.saved_lists.push((self.mode, std::mem::take(&mut self.cur_list), self.prev_depth, self.space_factor));
        self.eqtb.push_level(LevelType::Box);

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
            self.pushed.push(t);
            self.pushed.push(Token::char(1, b'{' as u32));
        }
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
            Mode::Vertical | Mode::InternalVertical => self.vlist_append(Node::Mark { class, tokens: toks }),
            _ => self.cur_list.push(Node::Mark { class, tokens: toks }),
        }
    }

    pub fn do_shipout(&mut self) {
        // \shipout<box spec>
        self.skip_spaces_relax();
        let t = self.get_token();
        if !t.is_cs() {
            self.error("\\shipout expects a box");
            self.pushed.push(t);
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
                self.pushed.push(t);
                self.error("\\shipout expects a box");
            }
        }
    }

    // ---------- paragraphs ----------
    pub fn par_primitive(&mut self) {
        if crate::debug_flag("IFTRACE") {
            eprintln!(
                "PAR mode={:?} line={} file={} macros={:?} stack={} pushed={:?}",
                self.mode,
                self.input.current_file_line(),
                self.input.current_file_name(),
                self.last_macros.iter().rev().take(6).collect::<Vec<_>>(),
                self.input.stack.iter().rev().take(4).map(|src| match src {
                    crate::input::Source::TokList { name, pos, toks, .. } => format!("T:{} {}/{}", name, pos, toks.len()),
                    crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name.split('/').last().unwrap_or(name), line_no),
                }).collect::<Vec<_>>().join(" << "),
                self.pushed.iter().rev().take(4).map(|t| if t.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) } else { format!("{:#x}", t.0) }).collect::<Vec<_>>()
            );
        }
        match self.mode {
            Mode::Horizontal => self.end_paragraph(),
            Mode::Vertical | Mode::InternalVertical => {
                // \par right after a display: tex.web resume_after_display
                // already restored hmode, so pending resume state dies here
                self.resume_after_display = false;
            }
            Mode::Math | Mode::DisplayMath => {
                self.error("Missing $ inserted (\\par in math)");
            }
            Mode::RestrictedHorizontal => {
                self.error("Missing } inserted (\\par in restricted hmode)");
            }
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


    pub fn start_paragraph(&mut self, indent: bool) {
        if crate::debug_flag("IFTRACE") {
            let src = match self.input.stack.last() {
                Some(crate::input::Source::TokList { name, .. }) => name.clone(),
                Some(crate::input::Source::File { name, .. }) => format!("F:{}", name),
                None => String::new(),
            };
            let trig = self.pushed.last().map(|t| {
                if t.is_cs() { format!("cs=\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) } else { format!("pushed=0x{:x}", t.0) }
            }).unwrap_or_default();
            eprintln!("START-PAR mode={:?} indent={} line={} src={} {}", self.mode, indent, self.input.current_file_line(), src, trig);
        }
        match self.mode {
            Mode::Horizontal => {
                if indent {
                    let pi = self.eqtb.dim_params[DimParam::ParIndent.idx() as usize];
                    let r = boxes::hpack(Vec::new(), Some(pi), boxes::HBOX, &self.eqtb);
                    self.cur_list.push(r.node);
                }
                self.run_everypar();
            }
            Mode::Vertical => {
                // tex.web resume_after_display (§1194): when text follows a
                // display the new hlist is pushed directly — no \parskip,
                // no \parindent box, no \everypar
                let resume = std::mem::take(&mut self.resume_after_display);
                if !resume {
                    // tex.web new_graf: in outer vmode \parskip glue is appended
                    // unconditionally (the page builder discards glue sitting at
                    // the top of a fresh page); tex.web does NOT run build_page
                    // at paragraph start, so the skip stays visible to
                    // \lastskip until the paragraph ends
                    let ps = self.eqtb.glue_params[GlueParam::ParSkip.idx() as usize].clone();
                    self.page_list.push(Node::Glue(ps));
                }
                // begin paragraph: switch from page_list to hlist
                // (tex.web: a paragraph is not a group; no eqtb level)
                let page = std::mem::take(&mut self.page_list);
                self.saved_lists.push((Mode::Vertical, Vec::new(), self.prev_depth, self.space_factor));
                self.par_saves += 1;
                self.par_page_lists.push(page);
                self.mode = Mode::Horizontal;
                self.cur_list = Vec::new();
                self.space_factor = 1000;
                self.prev_graf = 0;
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
                // like vertical but inside a box; tex.web new_graf adds
                // \parskip here only when the vertical list is nonempty
                // (no skip at the start of an empty \vbox list)
                let resume = std::mem::take(&mut self.resume_after_display);
                if !resume && !self.cur_list.is_empty() {
                    let ps = self.eqtb.glue_params[GlueParam::ParSkip.idx() as usize].clone();
                    self.cur_list.push(Node::Glue(ps));
                }
                let page = std::mem::take(&mut self.cur_list);
                self.saved_lists.push((Mode::InternalVertical, Vec::new(), self.prev_depth, self.space_factor));
                self.par_page_lists.push(page);
                self.mode = Mode::Horizontal;
                self.cur_list = Vec::new();
                self.space_factor = 1000;
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

    fn run_everypar(&mut self) {
        let toks = (*self.eqtb.tok_params[crate::prim::ToksParam::EveryPar.idx() as usize]).clone();
        if !toks.is_empty() {
        // tex.web just_paragraph: \everypar begins its own token list on
        // top of the input stack (push_tokens = begin_token_list), so the
        // hook preempts any in-flight macro remainder and nests cleanly
        // around fire_up's <after-output> parking instead of being
        // flattened into it.
        self.push_tokens(toks);
        }
    }

    pub fn end_paragraph(&mut self) {
        let has_content = self.cur_list.iter().any(|n| match n {
            Node::Char { .. } | Node::Disc(_) | Node::Ligature { .. } => true,
            Node::Rule { width, height, .. } => *width > 0 || *height > 0,
            Node::Box { w, h, d, list, .. } => *w > 0 || *h > 0 || *d > 0 || !list.is_empty(),
            _ => false,
        });
        if !has_content {
            self.cur_list.clear();
            // tex.web: \parshape/\looseness/\hangafter/\hangindent are reset
            // only in normal_paragraph (§1079) after a real line break — an
            // ABANDONED (empty) paragraph leaves them intact. LaTeX's list
            // machinery starts-and-abandons an empty paragraph on every
            // \item; wiping here destroyed \list's \parshape before the
            // first real list paragraph broke.
            let (saved_mode, saved_list, pd, sf) = self.saved_lists.pop().unwrap_or((Mode::Vertical, Vec::new(), self.prev_depth, self.space_factor));
            self.prev_depth = pd;
            self.space_factor = sf;
            self.mode = saved_mode;
            if let Some(outer) = self.par_page_lists.pop() {
                if saved_mode == Mode::Vertical {
                    self.page_list = outer;
                } else {
                    self.cur_list = outer;
                }
            } else {
                self.cur_list = saved_list;
            }
            return;
        }
        let pfs = self.eqtb.glue_params[GlueParam::ParFillSkip.idx() as usize].clone();
        self.cur_list.push(Node::Penalty(10000));
        self.cur_list.push(Node::Glue(pfs));
        let content = std::mem::take(&mut self.cur_list);
        // tex.web §21764/§21181: the widow penalty before the final line is
        // \displaywidowpenalty when a display interrupted the paragraph
        let fw = self.next_par_widow.take().unwrap_or_else(|| {
            self.eqtb.int_params[crate::prim::IntParam::WidowPenalty.idx() as usize]
        });
        let lines = self.break_paragraph(content, fw);
        // tex.web §1079 normal_paragraph: reset paragraph-local parameters —
        // all four resets are LOCAL eq_defines, so a group-wrapped \par (the
        // `{\@@par}` LaTeX lists install via \@setpar) rolls them back at
        // \egroup and the shape survives across \items
        self.assign_par_shape(Vec::new(), false);
        self.eqtb.assign_int_param(crate::prim::IntParam::Looseness, 0, false);
        self.eqtb.assign_int_param(crate::prim::IntParam::HangAfter, 1, false);
        self.eqtb.assign_dim_param(crate::prim::DimParam::HangIndent, 0, false);
        // restore vertical context
        let (saved_mode, _, pd, sf) = self.saved_lists.pop().unwrap_or((Mode::Vertical, Vec::new(), self.prev_depth, self.space_factor));
        self.prev_depth = pd;
        self.space_factor = sf;
        // interline glue construction happens in page builder; append lines vbox
        let mut lines_opt = Some(lines);
        match (saved_mode, self.par_page_lists.pop()) {
            (Mode::Vertical, Some(mut page)) => {
                // paragraph was at outer level: tex.web contributes the line
                // boxes (and migrated \vadjust material) directly to the page
                // builder. Packing them into one opaque vbox would make the
                // page unable to break inside the paragraph and would bury
                // \vadjust float markers (\end@float's hmode path).
                let lines = match lines_opt.take().unwrap() {
                    Node::Box { list, .. } => list,
                    other => vec![other],
                };
                page.extend(lines);
                self.page_list = page;
                self.mode = Mode::Vertical;
                self.cur_list = Vec::new();
                self.build_page();
            }
            (Mode::InternalVertical, Some(inner)) => {
                // paragraph started inside a \vbox/\vtop: resume that list.
                // The page builder never sees this list, so the zero-width
                // interline placeholders build_lines left between line boxes
                // must be filled HERE with real baselineskip glue (tex.web
                // append_to_vlist semantics) — captions/parbox paragraphs
                // otherwise pack at line height with no leading
                let mut bx = lines_opt.take().unwrap();
                if let Node::Box { list, .. } = &mut bx {
                    let taken = std::mem::take(list);
                    *list = self.fill_line_interline(taken);
                }
                self.cur_list = inner;
                self.mode = Mode::InternalVertical;
                // append with interline glue against the previous inner node
                self.append_box_node(Some(bx));
            }
            (_, outer) => {
                if let Some(mut inner) = outer {
                    inner.push(lines_opt.take().unwrap());
                    self.cur_list = inner;
                } else {
                    self.cur_list.push(lines_opt.take().unwrap());
                }
                self.mode = saved_mode;
            }
        }
    }

    /// replace build_lines' zero interline placeholders with real
    /// baselineskip/lineskip glue (tex.web append_to_vlist §17438-17440):
    /// used when a paragraph's lines land in an internal vlist, which the
    /// page builder never processes
    fn fill_line_interline(&self, list: NodeList) -> NodeList {
        const IGNORE: i32 = -1000 * 65536;
        let bs = self.eqtb.glue_params[GlueParam::BaselineSkip.idx() as usize].clone();
        let ls = self.eqtb.glue_params[GlueParam::LineSkip.idx() as usize].clone();
        let lsl = self.eqtb.dim_params[DimParam::LineSkipLimit.idx() as usize];
        let mut out: NodeList = Vec::with_capacity(list.len());
        let mut prev_depth = IGNORE;
        // hold a pending placeholder until we know whether a box follows
        let mut held_placeholder = false;
        for n in list.into_iter() {
            match &n {
                Node::Glue(g) if g.width == 0 && g.stretch == 0 && g.shrink == 0 => {
                    held_placeholder = true;
                    continue;
                }
                Node::Box { h, d, .. } | Node::Rule { height: h, depth: d, .. } => {
                    let (h, d) = (*h, *d);
                    if prev_depth > IGNORE {
                        let b = bs.width as i64 - prev_depth as i64 - h as i64;
                        let glue =
                            if b < lsl as i64 { ls.clone() } else { Glue { width: b as i32, ..bs.clone() } };
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
        out
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

