//! List building: characters, glue, kerns, penalties, rules, box groups,
//! paragraphs, page-builder hook.

use crate::boxes::{self, Glue, Node};
use crate::eqtb::LevelType;
use crate::engine::{Engine, Mode};
use crate::fontiface::LigKernStep;
use crate::prim::{DimParam, GlueParam, IntParam, Prim};
use crate::scaled::{mult, ONE};
use crate::token::Token;

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
        let f = self.cur_font;
        let mut g = if let Some(font) = self.eqtb.fonts.get(f as usize) {
            Glue { width: font.space(), stretch: font.space_stretch(), shrink: font.space_shrink(), stretch_order: 0, shrink_order: 0 }
        } else {
            Glue::zero()
        };
        let ss = self.eqtb.glue_params[GlueParam::SpaceSkip.idx() as usize].clone();
        if ss.width != 0 || ss.stretch != 0 || ss.shrink != 0 {
            g = ss.clone();
        } else {
            let xs = self.eqtb.glue_params[GlueParam::XSpaceSkip.idx() as usize].clone();
            if self.space_factor >= 2000 && (xs.width != 0 || xs.stretch != 0 || xs.shrink != 0) {
                g = xs.clone();
            }
        }
        let sf = self.space_factor;
        if sf >= 2000 {
            g.stretch = mult(g.stretch, sf / 1000);
        } else if sf < 1000 {
            g.shrink = mult(g.shrink, 1000 / sf.max(1));
        }
        g
    }

    pub fn char_token(&mut self, c: u8, _is_letter: bool) {
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                self.append_char(c);
                self.space_factor = self.space_factor_of(c);
            }
            Mode::Vertical | Mode::InternalVertical => {
                // TeX: horizontal material in vmode begins a paragraph
                self.start_paragraph(false);
                self.append_char(c);
                self.space_factor = self.space_factor_of(c);
            }
            Mode::Math | Mode::DisplayMath => {
                let mc = self.eqtb.math_code[c as usize];
                if mc & 0x8000 != 0 {
                    // active char in math: expand
                    let _ = mc;
                    // find active meaning: treat as \active char
                    self.active_char(c);
                } else {
                    self.append_mathchar(mc);
                }
            }
            _ => {
                self.error(&format!("You can't use character `{}' in vertical mode", c as char));
            }
        }
    }

    pub fn space_factor_of(&self, c: u8) -> i32 {
        let sf_code = self.eqtb.sf_code[c as usize] as i32;
        if sf_code == 0 {
            self.space_factor
        } else {
            sf_code
        }
    }

    pub fn active_char(&mut self, c: u8) {
        // look up cs named by the active char (TeX: active chars are cs-like)
        let id = match self.cs.lookup(&[c]) {
            Some(id) => id,
            None => {
                self.error(&format!("Undefined active character `{}'", c as char));
                return;
            }
        };
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
            Prim::HSS => Glue { width: 0, stretch: ONE, shrink: ONE, stretch_order: 1, shrink_order: 0 },
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
            Prim::VSS => Glue { width: 0, stretch: ONE, shrink: ONE, stretch_order: 1, shrink_order: 0 },
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

    /// appends to the current vertical list and runs the page builder at outer level
    pub fn vlist_append(&mut self, n: Node) {
        if self.mode == Mode::Vertical {
            self.page_list.push(n);
            self.build_page();
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
        let f = self.cur_font;
        if f == 0 {
            // real TeX nullfont: chars are silently dropped (no error)
            return;
        }
        // ligature & kern with previous char
        if let Some(Node::Char { c: pc, font: pf }) = self.cur_list.last() {
            if *pf == f {
                if let Some(step) = self.find_lig_kern(f, *pc, c) {
                    if step.is_kern {
                        self.cur_list.push(Node::Kern(step.kern_amount));
                        self.cur_list.push(Node::Char { c, font: f });
                        return;
                    } else {
                        // ligature: replace previous char
                        let last = self.cur_list.pop();
                        let _ = last;
                        let lc = step.lig_char;
                        let dims = self.char_dims(f, lc);
                        self.cur_list.push(Node::Ligature { c: lc, font: f, lig_width: dims.0, lig_height: dims.1, lig_depth: dims.2 });
                        if step.keep_right {
                            // re-add the new char after lig (iterate)
                            if step.iterate {
                                self.append_char(c);
                            } else {
                                let dims = self.char_dims(f, c);
                                self.cur_list.push(Node::Char { c, font: f });
                            }
                        } else if step.iterate {
                            self.append_char(c);
                        }
                        return;
                    }
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
            k += step.skip as usize;
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
            self.skip_spaces_relax();
            let t = self.get_token();
            let which = if t.is_cs() {
                match self.cs.name(t.cs_id()) {
                    b"width" => Some(0u8),
                    b"height" => Some(1),
                    b"depth" => Some(2),
                    _ => None,
                }
            } else {
                None
            };
            let which = match which {
                Some(w) => w,
                None => {
                    self.pushed.push(t);
                    break;
                }
            };
            self.scan_optional_equals();
            let d = self.scan_dimen(false, false);
            match which {
                0 => width = d,
                1 => height = d,
                _ => depth = d,
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

    /// \hbox to 10pt{...} etc: scan spec, push group context
    pub fn begin_box(&mut self, kind: u8) {
        // scan "to"/"spread" target: tex.web scan_spec uses scan_keyword
        // (character keywords), not control sequences
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
        let t = self.get_token();
        if !(t.is_char() && t.cc() == 1) {
            self.error("Missing { inserted");
            self.pushed.push(t);
            self.pushed.push(Token::char(1, b'{' as u32));
        }
        let shift = match self.pending_box_shift.take() {
            Some((d, _)) => d,
            None => 0,
        };
        if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) { eprintln!("PUSH-BG437"); }
        // save the outer list context: mode, current list, prev_depth, space_factor
        self.saved_lists.push((self.mode, std::mem::take(&mut self.cur_list), self.prev_depth, self.space_factor));
        self.eqtb.push_level(LevelType::Group);
        self.box_targets.push(target);
        self.box_shifts.push(shift);
        self.box_kinds.push(kind);
        match kind {
            0 => self.mode = Mode::RestrictedHorizontal,
            1 | 2 => {
                self.mode = Mode::InternalVertical;
                self.prev_depth = -1000 * 65536;
            }
            _ => self.mode = Mode::InternalVertical,
        }
    }

    /// called on the matching `}` for a box group or plain group
    pub fn end_box(&mut self) {
        if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) {
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
        if matches!(kind, 1 | 2 | 3 | 8) && self.mode == Mode::Horizontal {
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
        self.eqtb.pop_level();
        self.prev_depth = pd;
        self.space_factor = sf;
        self.mode = outer_mode;
        // tex.web math_group: `{`/`}` in math mode are pure grouping — the
        // math list keeps accumulating; no box is packaged or appended.
        if matches!(kind, 5 | 6) && outer_mode.is_m() {
            self.cur_list = outer_list;
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
                if self.setbox_target.take().is_some() {
                    self.error("A <box> was supposed to be here");
                }
                let body = boxes::LeaderBody::Box(Box::new(node));
                self.finish_leaders(lk, body);
                return;
            }
        }
        // store or append
        if self.shipout_pending {
            // \shipout<hbox|vbox|...>: the completed box IS the page; clear
            // the box-255 target do_shipout parked so it cannot leak
            self.setbox_target = None;
            self.shipout_pending = false;
            self.ship_box(Some(node));
            return;
        }
        if let Some(idx) = self.setbox_target.take() {
            self.eqtb.assign_box(idx, Some(node), self.global_flag);
            self.global_flag = false;
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
                Mode::Vertical | Mode::InternalVertical => {
                    self.vlist_append(node);
                }
                Mode::Math | Mode::DisplayMath => {
                    self.append_mlist_node(node);
                }
            },
        }
    }

    fn pop_leader_kind(&self) -> Option<u8> {
        LEADER_KINDS.with(|s| s.borrow_mut().pop())
    }

    // ---------- leaders ----------

    /// \leaders/\cleaders/\xleaders: scan the leader object (a box through
    /// the normal group machinery, or a rule), then the mandatory glue
    /// (\hskip-family in horizontal modes, \vskip-family in vertical), and
    /// append a leader node. tex.web scan_box/box_end leader context.
    pub fn begin_leaders(&mut self, kind: u8) {
        use boxes::LeaderBody;
        self.skip_spaces_relax();
        let t = self.get_token();
        if !t.is_cs() {
            self.pushed.push(t);
            self.error("A <box> was supposed to be here");
            return;
        }
        let name = self.cs.name(t.cs_id()).to_vec();
        match name.as_slice() {
            b"hbox" => {
                LEADER_KINDS.with(|s| s.borrow_mut().push(kind));
                self.begin_box(0);
            }
            b"vbox" => {
                LEADER_KINDS.with(|s| s.borrow_mut().push(kind));
                self.begin_box(1);
            }
            b"vtop" => {
                LEADER_KINDS.with(|s| s.borrow_mut().push(kind));
                self.begin_box(2);
            }
            b"vcenter" => {
                LEADER_KINDS.with(|s| s.borrow_mut().push(kind));
                self.begin_box(3);
            }
            b"box" => {
                let idx = self.scan_reg_num();
                let b = self.eqtb.boxed[idx as usize].take();
                if let Some(b) = b {
                    self.finish_leaders(kind, LeaderBody::Box(Box::new(b)));
                }
            }
            b"copy" => {
                let idx = self.scan_reg_num();
                let b = self.eqtb.boxed[idx as usize].clone();
                if let Some(b) = b {
                    self.finish_leaders(kind, LeaderBody::Box(Box::new(b)));
                }
            }
            b"lastbox" => {
                if let Some(b) = self.take_last_box() {
                    self.finish_leaders(kind, LeaderBody::Box(Box::new(b)));
                }
            }
            b"hrule" => {
                let (w, h, d) = self.scan_rule_dims(true);
                self.finish_leaders(kind, LeaderBody::Rule { width: w, height: h, depth: d });
            }
            b"vrule" => {
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
            if t.is_cs()
                && matches!(
                    self.cs.name(t.cs_id()),
                    b"hbox" | b"vbox" | b"vtop" | b"vcenter" | b"box" | b"copy" | b"lastbox"
                )
            {
                self.pushed.push(t);
                self.scan_box_after_move();
            } else {
                self.pushed.push(t);
                self.pending_box_shift = None;
                self.error("A <box> was supposed to be here");
            }
        } else {
            // \raise/\lower: shift is the vertical offset of a box in hmode
            self.pending_box_shift = Some((d, false));
            self.skip_spaces_relax();
            let t = self.get_token();
            if t.is_cs()
                && matches!(
                    self.cs.name(t.cs_id()),
                    b"hbox" | b"vbox" | b"vtop" | b"vcenter" | b"box" | b"copy" | b"lastbox"
                )
            {
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
        match self.cs.name(t.cs_id()) {
            b"hbox" => self.begin_box(0),
            b"vbox" => self.begin_box(1),
            b"vtop" => self.begin_box(2),
            b"box" => {
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
            b"copy" => {
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
            b"vcenter" => self.begin_box(3),
            b"lastbox" => {
                let b = self.take_last_box();
                self.append_box_node(b);
            }
            b"usebox" => {
                let idx = self.scan_reg_num();
                let b = self.eqtb.boxed[idx as usize].take();
                self.append_box_node(b);
            }
            _ => {
                self.pushed.push(t);
                self.error("Missing box after move/raise");
            }
        }
    }

    pub fn take_last_box(&mut self) -> Option<Node> {
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                // remove trailing box node
                if let Some(Node::Box { .. }) = self.cur_list.last() {
                    self.cur_list.pop()
                } else {
                    None
                }
            }
            Mode::Vertical | Mode::InternalVertical => {
                if let Some(Node::Box { .. }) = self.page_list.last() {
                    self.page_list.pop()
                } else if let Some(Node::Box { .. }) = self.cur_list.last() {
                    self.cur_list.pop()
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn last_kern_value(&mut self) -> i32 {
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                if let Some(Node::Kern(k)) = self.cur_list.last() {
                    *k
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    pub fn last_penalty_value(&mut self) -> i32 {
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                if let Some(Node::Penalty(p)) = self.cur_list.last() {
                    *p
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    pub fn last_skip_value(&mut self) -> Glue {
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                if let Some(Node::Glue(g)) = self.cur_list.last() {
                    g.clone()
                } else {
                    Glue::zero()
                }
            }
            _ => Glue::zero(),
        }
    }

    pub fn un_skip(&mut self) {
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                if let Some(Node::Glue(_) | Node::Leaders { .. }) = self.cur_list.last() {
                    self.cur_list.pop();
                } else {
                    self.error("No glue to remove");
                }
            }
            Mode::Vertical | Mode::InternalVertical => {
                let list = if self.mode == Mode::Vertical { &mut self.page_list } else { &mut self.cur_list };
                if let Some(Node::Glue(_) | Node::Leaders { .. }) = list.last() {
                    list.pop();
                } else {
                    self.error("No glue to remove");
                }
            }
            _ => {}
        }
    }

    pub fn un_kern(&mut self) {
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                if let Some(Node::Kern(_)) = self.cur_list.last() {
                    self.cur_list.pop();
                } else {
                    self.error("No kern to remove");
                }
            }
            _ => {}
        }
    }

    pub fn un_penalty(&mut self) {
        match self.mode {
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                if let Some(Node::Penalty(_)) = self.cur_list.last() {
                    self.cur_list.pop();
                } else {
                    self.error("No penalty to remove");
                }
            }
            Mode::Vertical | Mode::InternalVertical => {
                let list = if self.mode == Mode::Vertical { &mut self.page_list } else { &mut self.cur_list };
                if let Some(Node::Penalty(_)) = list.last() {
                    list.pop();
                } else {
                    self.error("No penalty to remove");
                }
            }
            _ => {}
        }
    }

    pub fn do_vsplit(&mut self) {
        // \vsplit<n> to <dimen>
        let n = self.scan_reg_num();
        self.scan_keyword(b"to");
        let target = self.scan_dimen(false, false);
        let boxn = self.eqtb.boxed[n as usize].take();
        let result = boxn.map(|b| self.vsplit_box(b, target));
        self.append_box_node(result.flatten());
    }

    pub fn scan_keyword(&mut self, kw: &[u8]) -> bool {
        self.skip_spaces_relax();
        let mut collected: Vec<Token> = Vec::new();
        for &expected in kw {
            let t = self.get_token();
            collected.push(t);
            if !t.is_char() || t.chr() != expected as u32 {
                collected.reverse();
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
        self.eqtb.push_level(LevelType::Group);
        self.box_targets.push(target);
        self.box_shifts.push(0);
        self.box_kinds.push(8);
        self.insert_nums.push(n);
        self.mode = Mode::InternalVertical;
        self.prev_depth = -1000 * 65536;
        // consume the group's opening `{` (tex.web scan_left_brace)
        self.skip_spaces_relax();
        let t = self.get_token();
        if !(t.is_char() && t.cc() == 1) {
            self.error("Missing { inserted");
            self.pushed.push(t);
            self.pushed.push(Token::char(1, b'{' as u32));
        }
    }

    pub fn append_vadjust(&mut self, toks: Vec<Token>) {
        let _ = toks;
        // scan braced vlist content; simplify: treat as box content pushed later
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
        match self.cs.name(t.cs_id()) {
            b"hbox" | b"vbox" | b"vtop" | b"vcenter" => {
                self.setbox_target = Some(255);
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
                let b = self.eqtb.boxed[idx as usize].take();
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
        match self.mode {
            Mode::Horizontal => self.end_paragraph(),
            Mode::Vertical | Mode::InternalVertical => {}
            Mode::Math | Mode::DisplayMath => {
                self.error("Missing $ inserted (\\par in math)");
            }
            Mode::RestrictedHorizontal => {
                self.error("Missing } inserted (\\par in restricted hmode)");
            }
        }
    }

    pub fn start_paragraph(&mut self, indent: bool) {
        if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) { eprintln!("START-PAR mode={:?} indent={} line={}", self.mode, indent, self.input.current_file_line()); }
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
                if indent {
                    let pi = self.eqtb.dim_params[DimParam::ParIndent.idx() as usize];
                                        let r = boxes::hpack(Vec::new(), Some(pi), boxes::HBOX, &self.eqtb);
                    self.cur_list.push(r.node);
                }
                self.run_everypar();
            }
            Mode::InternalVertical => {
                // like vertical but inside a box
                let page = std::mem::take(&mut self.cur_list);
                self.saved_lists.push((Mode::InternalVertical, Vec::new(), self.prev_depth, self.space_factor));
                self.par_page_lists.push(page);
                self.mode = Mode::Horizontal;
                self.cur_list = Vec::new();
                self.space_factor = 1000;
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
            self.input.push_toks(toks, "<everypar>");
        }
    }

    pub fn end_paragraph(&mut self) {
        // append parfillskip
        let pfs = self.eqtb.glue_params[GlueParam::ParFillSkip.idx() as usize].clone();
        self.cur_list.push(Node::Penalty(10000));
        self.cur_list.push(Node::Glue(pfs));
        let content = std::mem::take(&mut self.cur_list);
        let lines = self.break_paragraph(content);
        // restore vertical context
        let (saved_mode, _, pd, sf) = self.saved_lists.pop().unwrap_or((Mode::Vertical, Vec::new(), self.prev_depth, self.space_factor));
        self.prev_depth = pd;
        self.space_factor = sf;
        // interline glue construction happens in page builder; append lines vbox
        let vbox = lines;
        match (saved_mode, self.par_page_lists.pop()) {
            (Mode::Vertical, Some(mut page)) => {
                // paragraph was at outer level
                page.push(vbox);
                self.page_list = page;
                self.mode = Mode::Vertical;
                self.cur_list = Vec::new();
                self.build_page();
            }
            (Mode::InternalVertical, Some(mut inner)) => {
                // paragraph started inside a \vbox/\vtop: resume that list
                inner.push(vbox);
                self.cur_list = inner;
                self.mode = Mode::InternalVertical;
            }
            (_, outer) => {
                if let Some(mut inner) = outer {
                    inner.push(vbox);
                    self.cur_list = inner;
                } else {
                    self.cur_list.push(vbox);
                }
                self.mode = saved_mode;
            }
        }
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

