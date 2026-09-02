//! Page builder (TeX82 `build_page`): contributes material from the main
//! vertical list (`page_list`) to the current page, tracks `page_so_far`
//! accounting (heights, running depth, stretch/shrink by fil order), finds
//! page breakpoints (penalties and glue after boxes) with insert penalties,
//! and fires the `\output` routine at good breaks with the page in `\box255`
//! and each active insert class split/placed into `\box N`.
//!
//! `\outputpenalty` is a real int param (exported by the output-routine
//! entry below). `\floatingpenalty` has no dedicated storage: the insert
//! node's `cost` field stands in for it.

use std::collections::BTreeMap;

use crate::boxes::{vpack, Glue, Node, NodeList, VBOX};
use crate::engine::Engine;
use crate::prim::{DimParam, GlueParam, IntParam, ToksParam};
use crate::scaled::{badness, AWFUL_BAD, EJECT_PENALTY, INF_BAD, INF_PENALTY};

/// sentinel token pushed after the `\output` token list; ends the routine.
/// Must live in `0xFFFF_xxxx` so `get_token` does not treat it as `\\noexpand`
/// (`NOEXP_FLAG = 0xC000_0000` .. `0xFFFF_0000`).
pub const OUT_END_TOKEN: crate::token::Token = crate::token::Token(0xFFFF_FFFD);
/// Sentinel ending the `<write>` emission toklist; get_token returns it raw
/// so do_write's emission loop stops instead of leaking into the outer stream.
pub const WRITE_END_TOKEN: crate::token::Token = crate::token::Token(0xFFFF_FFFB);

/// tex.web `deplorable`: cost of a break at awful badness
const DEPLORABLE: i32 = 100_000;
/// `\prevdepth` sentinel meaning "nothing contributed yet on this page"
const DEPTH_NONE: i32 = -1000 * 65536;
/// mark classes are small; guard the per-class mark vectors anyway
const MAX_MARK_CLASS: usize = 65536;

/// natural vertical extents (width, height, depth) of a vlist; boxes carry
/// their own dimensions, so this stays local to the page builder
fn vlist_extents(list: &[Node]) -> (i64, i64, i64) {
    let (mut w, mut h, mut d) = (0i64, 0i64, 0i64);
    for n in list {
        match n {
            Node::Box { w: bw, h: bh, d: bd, .. }
            | Node::Rule { width: bw, height: bh, depth: bd } => {
                h += d + *bh as i64;
                d = *bd as i64;
                if *bw as i64 > w {
                    w = *bw as i64;
                }
            }
            Node::Ins { height: ih, depth: id, box_node, .. } => {
                h += d + *ih as i64;
                d = *id as i64;
                if let Node::Box { w: bw, .. } = &**box_node {
                    if *bw as i64 > w {
                        w = *bw as i64;
                    }
                }
            }
            Node::Glue(g) => {
                h += d + g.width as i64;
                d = g.width as i64;
            }
            Node::Kern(k) | Node::ExplicitKern(k) => {
                h += d + *k as i64;
                d = *k as i64;
            }
            Node::Adj(a) => {
                h += d + *a as i64;
                d = *a as i64;
            }
            _ => {}
        }
    }
    (w, h, d)
}

/// a remembered page-breakpoint candidate
#[derive(Clone, Copy)]
struct BreakSpot {
    /// number of leading `page_list` nodes that belong to the page
    cut: usize,
    /// penalty at the break (0 for glue breaks)
    penalty: i32,
    /// total cost: badness + penalty + insert penalties
    cost: i32,
}

impl BreakSpot {
    /// a breakpoint carried over from an earlier `build_page` call; its real
    /// cost was strictly positive (non-positive fires immediately), so using
    /// `deplorable` only weakens it against fresh candidates
    fn carried(cut: usize, penalty: i32) -> BreakSpot {
        BreakSpot { cut, penalty, cost: DEPLORABLE }
    }
}

/// running accounting for the current page (tex.web's cur_page_* globals)
struct PageState {
    /// natural height of contributed material ("page so far")
    total: i64,
    /// "recent contributions": depth folded forward into height
    depth: i64,
    /// stretch by glue order (0 = pt, 1 = fil, 2 = fill, 3 = filll)
    stretch: [i64; 4],
    /// shrink by glue order
    shrink: [i64; 4],
    /// running `\insertpenalties`
    insert_penalties: i64,
    /// scaled insert height used per class on this page
    ins_used: Vec<(u16, i64)>,
    /// a box or rule has been contributed (breaks need one before them)
    box_seen: bool,
    best: Option<BreakSpot>,
    /// a fire condition was met this call
    fire: bool,
    /// `page_list` prefix already folded into the accounting
    processed: usize,
    /// page started (a box or insert landed): stops top-of-page discards
    goal_set: bool,
}

impl PageState {
    fn new() -> PageState {
        PageState {
            total: 0,
            depth: 0,
            stretch: [0; 4],
            shrink: [0; 4],
            insert_penalties: 0,
            ins_used: Vec::new(),
            box_seen: false,
            best: None,
            fire: false,
            processed: 0,
            goal_set: false,
        }
    }

    /// accumulate `amount` for insert class `num`, returning the new total
    fn ins_bump(&mut self, num: u16, amount: i64) -> i64 {
        for e in self.ins_used.iter_mut() {
            if e.0 == num {
                e.1 += amount;
                return e.1;
            }
        }
        self.ins_used.push((num, amount));
        amount
    }
}

impl Engine {
    /// `\vsize` read live, so mid-page changes are honored at the next break.
    /// An explicit `\pagegoal` assignment (output routine / package) takes
    /// precedence; in iniTeX or when the fallback `\vsize <= 0`, the goal is
    /// `max_dimen`.
    fn page_goal(&self) -> i64 {
        let pg = self.eqtb.dim_params[DimParam::PageGoal.idx() as usize] as i64;
        if pg != 0 {
            return pg;
        }
        self.vsize_goal()
    }

    /// goal derived from `\vsize` alone (max_dimen when `\vsize <= 0`);
    /// tex.web re-derives `page_goal := \vsize` at each page start
    fn vsize_goal(&self) -> i64 {
        let vs = self.eqtb.dim_params[DimParam::VSize.idx() as usize] as i64;
        if vs <= 0 {
            0x3FFF_FFFF // max_dimen: 16383.99998 pt
        } else {
            vs
        }
    }
    fn max_depth(&self) -> i64 {
        self.eqtb.dim_params[DimParam::MaxDepth.idx() as usize] as i64
    }

    /// mirror the running page accounting into the `\pagetotal` family of
    /// dimension registers (tex.web keeps cur_page_* and these registers in
    /// one storage; here the builder state is the authority)
    fn sync_page_dims(&mut self, st: &PageState) {
        let c32 = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        self.eqtb.dim_params[DimParam::PageTotal.idx() as usize] = c32(st.total);
        self.eqtb.dim_params[DimParam::PageDepth.idx() as usize] = c32(st.depth);
        for (o, p) in [
            (0, DimParam::PageStretch),
            (1, DimParam::PageFilStretch),
            (2, DimParam::PageFillStretch),
            (3, DimParam::PageFilllStretch),
        ] {
            self.eqtb.dim_params[p.idx() as usize] = c32(st.stretch[o]);
        }
        self.eqtb.dim_params[DimParam::PageShrink.idx() as usize] = c32(st.shrink[0]);
        // `\pagegoal` is read-only in tex.web: publish the goal the builder
        // acts on so `\the\pagegoal` reports live state
        self.eqtb.dim_params[DimParam::PageGoal.idx() as usize] = c32(self.page_goal());
    }

    /// scaled insert height: `h * count(n) / 1000` ("magnification");
    /// `count(n) <= 0` contributes nothing to the page
    fn ins_scaled_height(&self, num: u16, raw_h: i64) -> i64 {
        let cnt = self.eqtb.count[num as usize] as i64;
        if cnt == 1000 {
            raw_h
        } else if cnt <= 0 {
            0
        } else {
            (raw_h * cnt + 500) / 1000
        }
    }

    pub fn build_page(&mut self) {
        // page_list may have been swapped/restored by group or paragraph
        // handling (par_page_lists), which does not carry page_processed:
        // clamp the restored index instead of trusting it
        let mut st = PageState {
            processed: self.page_processed.min(self.page_list.len()),
            goal_set: self.page_goal_set,
            insert_penalties: self.eqtb.int_params[IntParam::InsertPenalties.idx() as usize] as i64,
            ..PageState::new()
        };
        st.total = self.page_total;
        st.depth = self.page_depth;
        if let Some(cut) = self.page_best_break {
            st.best = Some(BreakSpot::carried(cut, self.page_break_penalty));
        }
        // a box before the pending region legalizes breaks; tex.web keeps
        // this in page_contents (box_there), we re-derive it from the prefix
        st.box_seen = self.page_list[..st.processed]
            .iter()
            .any(|n| matches!(n, Node::Box { .. } | Node::Rule { .. }));
        // seed per-class insert usage from the already-processed prefix
        for node in &self.page_list[..st.processed] {
            if let Node::Ins { num, height, .. } = node {
                st.ins_bump(*num, self.ins_scaled_height(*num, *height as i64));
            }
        }

        while st.processed < self.page_list.len() {
            let idx = st.processed;
            let mut advance = true;
            match self.page_list[idx].clone() {
                Node::Glue(g) => {
                    if st.goal_set {
                        self.contribute_glue(&mut st, &g);
                        // legal break at glue when a box precedes
                        if st.box_seen {
                            self.try_page_break(&mut st, idx, 0);
                        }
                    }
                    // else: discarded at the top of a fresh page
                }
                Node::Kern(k) | Node::ExplicitKern(k) => {
                    if st.goal_set {
                        self.contribute_gap(&mut st, k as i64);
                    }
                }
                Node::Penalty(p) => {
                    // legal break at penalties < inf_penalty when a box precedes
                    if p < INF_PENALTY && st.goal_set && st.box_seen {
                        self.try_page_break(&mut st, idx + 1, p);
                    }
                }
                Node::Box { h, d, .. } | Node::Rule { height: h, depth: d, .. } => {
                    // tex.web contributes every box, including 0x0 ones
                    // (`\box_there`); LaTeX's float/clearpage machinery
                    // plants empty `\vbox{}` markers purely so the following
                    // forced `\penalty -1000x` is a legal break that re-fires
                    // the output routine. Mark the page breakable without
                    // contributing height (unlike tex.web we add no topskip
                    // pad, keeping page accounting unchanged).
                    if h == 0 && d == 0 {
                        st.goal_set = true;
                        st.box_seen = true;
                    } else if !st.goal_set {
                        // first box on a fresh page: `\topskip` glue before it
                        st.goal_set = true;
                        self.page_prev_depth = DEPTH_NONE;
                        let ts = self.eqtb.dim_params[DimParam::TopSkip.idx() as usize];
                        let pad = (ts as i64 - h as i64).max(0);
                        if pad > 0 {
                            self.page_list.insert(idx, Node::Glue(Glue::new(pad as i32)));
                            advance = false; // reprocess at the inserted glue
                        }
                    } else if self.page_prev_depth > DEPTH_NONE {
                        // interline glue between boxes
                        let bs =
                            self.eqtb.glue_params[GlueParam::BaselineSkip.idx() as usize].clone();
                        let ls = self.eqtb.glue_params[GlueParam::LineSkip.idx() as usize].clone();
                        let lsl = self.eqtb.dim_params[DimParam::LineSkipLimit.idx() as usize];
                        let b = bs.width as i64 - self.page_prev_depth as i64 - h as i64;
                        let glue = if b < lsl as i64 { ls } else { Glue { width: b as i32, ..bs } };
                        self.page_prev_depth = DEPTH_NONE;
                        if glue.width != 0 || glue.stretch != 0 || glue.shrink != 0 {
                            self.page_list.insert(idx, Node::Glue(glue));
                            advance = false; // reprocess at the inserted glue
                        }
                    }
                    if advance && (h != 0 || d != 0) {
                        self.contribute_box(&mut st, h, d);
                        self.page_prev_depth = d;
                    }
                }
                Node::Ins { num, height, depth, cost, .. } => {
                    self.contribute_ins(&mut st, num, height, depth, cost);
                }
                Node::Mark { class, tokens } => {
                    if st.goal_set {
                        let c = class.max(0) as usize;
                        if c < MAX_MARK_CLASS {
                            for m in self.marks.iter_mut() {
                                if m.len() <= c {
                                    m.resize(c + 1, Vec::new());
                                }
                            }
                            if self.marks[2][c].is_empty() {
                                self.marks[1][c] = tokens.clone();
                            }
                            self.marks[2][c] = tokens;
                        }
                    }
                    // else: discarded at the top of a fresh page
                }
                Node::Adj(a) => {
                    if st.goal_set {
                        self.contribute_gap(&mut st, a as i64);
                    }
                }
                _ => {}
            }
            if advance {
                st.processed += 1;
                if self.ready_to_fire(&st) {
                    st.fire = true;
                    break;
                }
            }
        }

        // persist the running state
        self.page_total = st.total;
        self.page_depth = st.depth;
        self.page_processed = st.processed;
        self.page_best_break = st.best.map(|b| b.cut);
        self.page_break_penalty = st.best.map(|b| b.penalty).unwrap_or(0);
        self.page_goal_set = st.goal_set;
        self.eqtb.int_params[IntParam::InsertPenalties.idx() as usize] =
            st.insert_penalties.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        self.sync_page_dims(&st);
        // fire only when a fire condition was met; otherwise the best break
        // stays remembered and the page keeps accumulating (tex.web)
        if st.fire {
            if let Some(spot) = st.best {
                let penalty = spot.penalty;
                self.fire_up(spot.cut, penalty);
            }
        }
    }


    /// fold a box contribution into the page accounting (tex.web @1047):
    /// depth beyond `\maxdepth` is charged to height
    fn contribute_box(&mut self, st: &mut PageState, h: i32, d: i32) {
        st.total += st.depth + h as i64;
        let md = self.max_depth();
        let d64 = d as i64;
        if d64 > md {
            st.total += d64 - md;
            st.depth = md;
        } else {
            st.depth = d64;
        }
        st.box_seen = true;
        st.goal_set = true;
    }

    /// fold glue: width folds the running depth away, stretch/shrink accrue
    /// by order (tex.web @1042)
    fn contribute_glue(&mut self, st: &mut PageState, g: &Glue) {
        self.contribute_gap(st, g.width as i64);
        let so = (g.stretch_order as usize).min(3);
        let ho = (g.shrink_order as usize).min(3);
        st.stretch[so] += g.stretch as i64;
        if so > 0 && g.width != 0 {
            st.stretch[so] += g.width as i64;
        }
        st.shrink[ho] += g.shrink as i64;
        if ho > 0 && g.width != 0 {
            st.shrink[ho] += g.width as i64;
        }
    }

    /// fold a kern-like gap (kern, \vadjust amount)
    fn contribute_gap(&mut self, st: &mut PageState, w: i64) {
        st.total += st.depth + w;
        st.depth = 0;
    }

    /// fold an insert contribution (tex.web @1052): the count-scaled height
    /// counts toward the page; exceeding `\dimen n` charges the insert's
    /// cost (standing in for `\floatingpenalty`) to `\insertpenalties`
    fn contribute_ins(&mut self, st: &mut PageState, num: u16, height: i32, depth: i32, cost: i32) {
        let scaled = self.ins_scaled_height(num, height as i64);
        st.total += st.depth + scaled;
        let md = self.max_depth();
        let d64 = depth as i64;
        if d64 > md {
            st.total += d64 - md;
            st.depth = md;
        } else {
            st.depth = d64;
        }
        st.goal_set = true;
        if height != 0 {
            let used = st.ins_bump(num, scaled);
            let budget = self.eqtb.dimen[num as usize] as i64;
            if self.eqtb.count[num as usize] > 0 && budget < used && cost > 0 {
                st.insert_penalties += cost as i64;
            }
        }
    }

    /// remember a breakpoint candidate if its cost beats the current best
    fn try_page_break(&mut self, st: &mut PageState, cut: usize, penalty: i32) {
        let cost = self.break_cost(st, penalty);
        let better = match st.best {
            None => true,
            Some(b) => cost <= b.cost,
        };
        if better {
            st.best = Some(BreakSpot { cut, penalty, cost });
        }
    }

    /// tex.web @1003/@1004: badness of the break plus penalty plus
    /// `\insertpenalties`
    fn break_cost(&self, st: &PageState, penalty: i32) -> i32 {
        let goal = self.page_goal();
        let b: i64 = if st.total < goal {
            let want = (goal - st.total).min(i32::MAX as i64 / 2) as i32;
            let mut order = 0usize;
            for o in (1..4).rev() {
                if st.stretch[o] != 0 {
                    order = o;
                    break;
                }
            }
            let s = st.stretch[order].min(i32::MAX as i64 / 2) as i32;
            badness(want, s) as i64
        } else if st.total - goal > st.shrink[0] {
            AWFUL_BAD as i64
        } else {
            let s = st.shrink[0].min(i32::MAX as i64 / 2) as i32;
            let t = (st.total - goal).min(i32::MAX as i64 / 2) as i32;
            badness(t, s) as i64
        };
        let c: i64 = if b < AWFUL_BAD as i64 {
            if penalty <= EJECT_PENALTY {
                penalty as i64
            } else if b < INF_BAD as i64 {
                b + penalty as i64 + st.insert_penalties
            } else {
                DEPLORABLE as i64
            }
        } else if penalty <= EJECT_PENALTY {
            penalty as i64
        } else {
            DEPLORABLE as i64
        };
        c.clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    fn ready_to_fire(&self, st: &PageState) -> bool {
        if self.ini_mode {
            return false;
        }
        if let Some(spot) = st.best {
            if spot.penalty <= EJECT_PENALTY {
                return true;
            }
            if st.total >= self.page_goal() {
                return true;
            }
        }
        false
    }

    /// cut the page at `cut`, place/split inserts into `\box N`, pack the
    /// rest into `\box255`, and run `\output` (tex.web fire_up)
    fn fire_up(&mut self, cut: usize, penalty: i32) {
        if crate::debug_flag("PAGETRACE") {
            eprintln!(
                "FIRE_UP cut={} pen={} in_output={} pages={} line={}",
                cut,
                penalty,
                self.in_output,
                self.pdf_doc.pages.len(),
                self.input.current_file_line()
            );
        }
        if self.in_output {
            // Stale-lock recovery: error paths that strip the <output>/
            // <endoutput> toklists (scan_definable_cs clears, end-of-file)
            // bypass finish_output; without the routine on the stack the
            // lock can never release and every later page is silently lost.
            let alive = self.input.stack.iter().any(|s| {
                matches!(
                    s,
                    crate::input::Source::TokList { name, .. }
                        if name == "<output>" || name == "<endoutput>"
                )
            });
            if alive {
                return;
            }
            self.in_output = false;
            self.output_depth = 0;
        }
        if self.ini_mode {
            return;
        }
        self.eqtb.int_params[IntParam::OutputPenalty.idx() as usize] = penalty;
        self.dead_cycles += 1;
        // marks: `\topmark` becomes the old `\botmark`; per-page marks reset
        self.marks[0] = self.marks[2].clone();
        for m in self.marks.iter_mut().skip(1) {
            *m = Vec::new();
        }

        let items: NodeList = self.page_list.drain(..cut).collect();
        self.page_processed = self.page_processed.saturating_sub(cut);
        self.page_best_break = None;
        self.page_break_penalty = 0;

        // inserts leave the page material; each class goes into `\box N`
        let mut page_mat: NodeList = Vec::new();
        let mut inserts: BTreeMap<u16, Vec<Node>> = BTreeMap::new();
        for node in items {
            match node {
                Node::Ins { num, box_node, .. } => {
                    inserts.entry(num).or_default().push(*box_node);
                }
                other => page_mat.push(other),
            }
        }
        for (num, boxes) in inserts {
            self.place_insert(num, boxes);
        }

        // a new page begins: reset running accounting
        self.eqtb.int_params[IntParam::InsertPenalties.idx() as usize] = 0;
        self.page_total = 0;
        self.page_depth = 0;
        self.page_prev_depth = DEPTH_NONE;
        self.page_goal_set = false;
        // the `\pagetotal` family restarts with the page (tex.web @638)
        self.sync_page_dims(&PageState::new());
        // the new page's goal is a fresh `\vsize` snapshot (tex.web @638);
        // a mid-page `\pagegoal` pin does not survive the page break
        self.eqtb.dim_params[DimParam::PageGoal.idx() as usize] =
            self.vsize_goal().clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        let md = self.max_depth().min(i32::MAX as i64) as i32;
        let r = crate::boxes::vpack_add_md(page_mat, None, false, VBOX, &self.eqtb, md);
        self.eqtb.assign_box(255, Some(r.node), true);

        // dead-cycle limit: force shipout instead of looping the routine
        let maxdc = self.eqtb.int_params[IntParam::MaxDeadCycles.idx() as usize].max(0);
        if self.dead_cycles > maxdc {
            self.error(&format!(
                "Output loop---{} consecutive dead cycles",
                self.dead_cycles
            ));
            let b = self.eqtb.boxed[255].take();
            self.ship_box(b);
            self.dead_cycles = 0;
            return;
        }

        let toks = (*self.eqtb.tok_params[ToksParam::Output.idx() as usize]).clone();
        if toks.is_empty() {
            // TeXbook default output: \shipout\box255 (also in INITEX)
            let b = self.eqtb.boxed[255].take();
            self.ship_box(b);
            self.dead_cycles = 0;
            return;
        }
        self.in_output = true;
        self.output_depth += 1;
        // tex.web: the output routine preempts in-flight input. With
        // begin_token_list semantics, macro/hook replays live as nested
        // TokList sources BELOW the routine pushed here, so they resume
        // only after OUT_END ran finish_output (tex.web input-stack LIFO).
        // What still has to be parked is `pushed`: genuine single-token
        // back_input, which raw_token would otherwise drain BEFORE the
        // routine (e.g. `\enddocument` remainder after `\clearpage`).
        // Park it below the routine (same pattern as <after-input>).
        if !self.pushed.is_empty() {
            let mut rest = std::mem::take(&mut self.pushed);
            rest.reverse();
            self.input.push_toks(rest, "<after-output>");
        }
        // LIFO input stack: continuation first, then the routine.
        self.input.push_toks(vec![OUT_END_TOKEN], "<endoutput>");
        self.input.push_toks(toks, "<output>");
    }

    /// Eject the current page prefix (used by `\end`).
    pub fn eject_page(&mut self, cut: usize) {
        self.fire_up(cut, -0x4000_0000);
    }

    /// assemble class `num`'s material (leftover in `\box N` plus this
    /// page's insert boxes) into `\box N`, splitting to `\dimen N` when it
    /// exceeds the budget (tex.web @1046); the remainder is carried over as
    /// a fresh insert node at the front of the pending list
    fn place_insert(&mut self, num: u16, ins_boxes: Vec<Node>) {
        let mut list: NodeList = match self.eqtb.boxed[num as usize].take() {
            Some(Node::Box { list, .. }) => list,
            Some(other) => vec![other],
            None => Vec::new(),
        };
        for b in ins_boxes {
            match b {
                Node::Box { list: l, .. } => list.extend(l),
                other => list.push(other),
            }
        }
        if list.is_empty() {
            return;
        }
        let cnt = self.eqtb.count[num as usize];
        let budget = self.eqtb.dimen[num as usize] as i64;
        let (_, raw_h, _) = vlist_extents(&list);
        let scaled = self.ins_scaled_height(num, raw_h);
        if cnt <= 0 || scaled <= budget {
            // fits (or unregulated class): place whole
            let r = vpack(list, None, VBOX, &self.eqtb);
            self.eqtb.assign_box(num, Some(r.node), true);
            return;
        }
        // split honoring `\splittopskip` / `\splitmaxdepth`
        let topskip = self.eqtb.glue_params[GlueParam::SplitTopSkip.idx() as usize].clone();
        let splitmax = self.max_split_depth();
        let target = (budget.max(0) * 1000) / cnt.max(1) as i64;
        let (mut top, rest) = split_vlist(&list, target);
        if top.iter().any(|n| matches!(n, Node::Box { .. })) {
            // `\splittopskip` pad above the first kept box (tex.web @987)
            if let Some(Node::Box { h: fh, .. }) = top.first() {
                let pad = topskip.width as i64 - *fh as i64;
                if pad > 0 {
                    top.insert(0, Node::Glue(Glue::new(pad as i32)));
                }
            }
            // split marks come from the kept part
            for n in &top {
                if let Node::Mark { class, tokens } = n {
                    let c = (*class).max(0) as usize;
                    if c < MAX_MARK_CLASS {
                        for m in self.marks[3..5].iter_mut() {
                            if m.len() <= c {
                                m.resize(c + 1, Vec::new());
                            }
                        }
                        self.marks[3][c] = tokens.clone();
                        self.marks[4][c] = tokens.clone();
                    }
                }
            }
            let dm = splitmax.min(i32::MAX as i64) as i32;
            let r = crate::boxes::vpack_add_md(top, None, false, VBOX, &self.eqtb, dm);
            let node = r.node;
            self.eqtb.assign_box(num, Some(node), true);
        } else {
            // nothing fits: defer the whole insert to the next page
            self.carry_insert(num, list);
            return;
        }
        if !rest.is_empty() {
            self.carry_insert(num, rest);
        }
    }

    fn max_split_depth(&self) -> i64 {
        self.eqtb.dim_params[DimParam::SplitMaxDepth.idx() as usize] as i64
    }

    /// push leftover insert material back for the next page
    fn carry_insert(&mut self, num: u16, list: NodeList) {
        let (w, h, d) = vlist_extents(&list);
        let _ = w;
        let r = vpack(list, None, VBOX, &self.eqtb);
        let node = Node::Ins {
            num,
            height: h as i32,
            depth: d as i32,
            cost: 0,
            box_node: Box::new(r.node),
        };
        self.page_list.insert(0, node);
        self.page_processed += 1;
    }

    pub fn finish_output(&mut self) {
        self.in_output = false;
        self.output_depth = self.output_depth.saturating_sub(1);
        if self.output_depth == 0 {
            self.build_page();
        }
    }

    /// \shipout received a box: emit a PDF page
    pub fn ship_box(&mut self, b: Option<Node>) {
        self.dead_cycles = 0;
        let Some(boxn) = b else { return };
        if crate::debug_flag("PAGETRACE") {
            eprintln!(
                "SHIPOUT pages={} in_output={} line={}",
                self.pdf_doc.pages.len() + 1,
                self.in_output,
                self.input.current_file_line()
            );
        }
        if self.eqtb.int_params[IntParam::TracingPages.idx() as usize] > 0 {
            let page = self.eqtb.count[0] as i64 + 1;
            let msg = format!("[{}]", page);
            self.term.push_str(&msg);
            self.log.push_str(&msg);
        }
        // page size
        let width = self.eqtb.dim_params[DimParam::PdfPageWidth.idx() as usize];
        let height = self.eqtb.dim_params[DimParam::PdfPageHeight.idx() as usize];
        let _ = (width, height);
        let page = self.render_page(&boxn);
        self.pdf_doc.pages.push(page);
    }
}

/// tex.web `vsplit_page` (@973-987): find the best split point of a vertical
/// list within `target` of natural height. Breaks happen before a box, at
/// glue/kern following a box, and at `\penalty-10000`; the glue at a split
/// is discarded. Returns (kept top part, remainder).
fn split_vlist(list: &[Node], target: i64) -> (NodeList, NodeList) {
    let mut d: i64 = 0;
    let mut prev_box = false;
    let mut split_at: Option<usize> = None;
    for (i, node) in list.iter().enumerate() {
        match node {
            Node::Box { h, .. } | Node::Rule { height: h, .. } | Node::Ins { height: h, .. } => {
                if d + *h as i64 > target {
                    split_at = Some(i); // split before this box
                    break;
                }
                d += *h as i64;
                prev_box = true;
            }
            Node::Glue(g) => {
                let w = g.width as i64;
                if d + w > target && prev_box {
                    split_at = Some(i); // the break glue is discarded below
                    break;
                }
                d = w; // tex.web: glue flushes the running depth
                prev_box = false;
            }
            Node::Kern(k) | Node::ExplicitKern(k) => {
                let w = *k as i64;
                if d + w > target && prev_box {
                    split_at = Some(i);
                    break;
                }
                d = w;
                prev_box = false;
            }
            Node::Penalty(p) => {
                if *p <= EJECT_PENALTY {
                    split_at = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let (top, mut rest) = match split_at {
        None => (list.to_vec(), Vec::new()),
        Some(i) => {
            let at_glue = matches!(list[i], Node::Glue(_));
            let mut r = list[i..].to_vec();
            if at_glue {
                r.remove(0);
            }
            (list[..i].to_vec(), r)
        }
    };
    (top, rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim::DimParam as DP;

    fn run(src: &str) -> Engine {
        let mut e = Engine::new(true);
        e.init_primitives();
        e.add_nullfont();
        let full = format!(
            "\\catcode`\\{{=1 \\catcode`\\}}=2 \\catcode`\\#=6 \\catcode`\\&=4 {}\n",
            src
        );
        e.input.push_file("page.tex".to_string(), full.as_bytes().to_vec());
        e.run();
        e
    }

    fn vbox_parts(n: &Node) -> (usize, i32) {
        match n {
            Node::Box { list, h, .. } => (list.len(), *h),
            other => panic!("expected vbox, got {:?}", other),
        }
    }

    #[test]
    fn vsplit_retains_remainder_in_source_box() {
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\setbox0=\\vbox{\\hbox{a}\\hbox{b}\\hbox{c}\\hbox{d}\\hbox{e}}\n",
            "\\setbox1=\\vsplit0 to 20pt\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        let (n1, h1) = vbox_parts(e.eqtb.boxed[1].as_ref().expect("split part in \\box1"));
        let (n0, _) = vbox_parts(e.eqtb.boxed[0].as_ref().expect("remainder in \\box0"));
        assert!(n1 > 0, "split part holds boxes");
        assert!(n0 > 0, "remainder holds the rest, not empty");
        assert!(h1 > 0 && h1 <= 20 * 65536, "split part within 20pt: {}", h1);
        assert_eq!(n1 + n0, 5, "all five lines accounted for");
    }

    #[test]
    fn page_dims_track_builder_state() {
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\vsize 100pt \\hsize 200pt\n",
            "line one\n\nline two\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        // \pagegoal mirrors \vsize while the page accumulates
        assert_eq!(
            e.eqtb.dim_params[DP::PageGoal.idx() as usize] as i64,
            100 * 65536
        );
        let total = e.eqtb.dim_params[DP::PageTotal.idx() as usize] as i64;
        assert!(total > 0, "\\pagetotal accumulates, got {}", total);
        assert_eq!(total, e.page_total, "\\pagetotal == builder total");
        assert_eq!(
            e.eqtb.dim_params[DP::PageDepth.idx() as usize] as i64,
            e.page_depth,
            "\\pagedepth == builder depth"
        );
    }
}
