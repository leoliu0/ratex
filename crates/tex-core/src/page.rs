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
use crate::engine::{Engine, Mode};
use crate::prim::{DimParam, IntParam, ToksParam};
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

/// tex.web precedes_break (§3109): node types a page glue may legally break
/// after — type < math_node, i.e. box/rule/insert/mark/adjust/whatsit
fn precedes_break(n: &Node) -> bool {
    matches!(
        n,
        Node::Box { .. }
            | Node::Rule { .. }
            | Node::NativeGlyphRun { .. }
            | Node::Ins { .. }
            | Node::Mark { .. }
            | Node::Adj(_)
            | Node::Whatsit(_)
    )
}

/// tex.web `x_over_n` (§2274): Pascal `div` on the magnitude, sign
/// restored — truncation toward zero; division by zero yields 0
fn x_over_n(x: i64, n: i64) -> i64 {
    if n == 0 {
        0
    } else {
        x / n
    }
}

/// tex.web `prune_page_top` (§18869): after a split, leading discardables
/// (glue/kern/penalty) are dropped, whatsits/marks/insertions stay, and a
/// `split_top_skip` glue shrunk by the first box's height opens the list.
/// Scanning stops at the first box/rule; everything after it is untouched.
fn prune_page_top_list(list: NodeList, topskip: &Glue) -> NodeList {
    let mut out: NodeList = Vec::new();
    let mut it = list.into_iter();
    loop {
        match it.next() {
            None => return out,
            Some(n @ Node::Mark { .. })
            | Some(n @ Node::Ins { .. })
            | Some(n @ Node::Whatsit(_)) => out.push(n),
            Some(n) => {
                let h = match &n {
                    Node::Box { h, .. }
                    | Node::Rule { height: h, .. }
                    | Node::NativeGlyphRun { height: h, .. } => *h as i64,
                    Node::Glue(_) | Node::Kern(_) | Node::ExplicitKern(_) | Node::Penalty(_) => {
                        continue
                    }
                    _ => continue,
                };
                let pad = (topskip.width as i64 - h).max(0) as i32;
                out.push(Node::Glue(Glue::new(pad)));
                out.push(n);
                out.extend(it);
                return out;
            }
        }
    }
}

/// tex.web `vert_break` (§18914) on a `Node::Ins` inner vlist: the optimum
/// place to cut so the top part packs to natural height `w` with maximum
/// depth `d`. Returns `(best_cut, best_height_plus_depth)`; `best_cut=None`
/// stands for `q=null` (the artificial end-of-list forced break, i.e. the
/// whole list is the top part). `best_cut=Some(i)` cuts before `list[i]`.
fn page_vert_break(list: &[Node], w: i64, d: i64) -> (Option<usize>, i64) {
    // active_height[1..6]: cur, finite stretch, fil, fill, filll, shrink
    let mut cur: i64 = 0;
    let mut act = [0i64; 5];
    let mut prev_dp: i64 = 0;
    let mut least_cost: i64 = AWFUL_BAD as i64;
    let mut best: Option<usize> = None;
    let mut best_bhpd: i64 = 0;
    // prev_p := p initially: an opening glue is not a legal breakpoint
    let mut prev_breakable = false;
    let n = list.len();
    let mut i = 0usize;
    loop {
        let c32 = |v: i64| v.clamp(0, i32::MAX as i64) as i32;
        let pi: i64 = if i >= n {
            EJECT_PENALTY as i64
        } else {
            match &list[i] {
                Node::Box { h, d: bd, .. }
                | Node::NativeGlyphRun {
                    height: h,
                    depth: bd,
                    ..
                }
                | Node::Rule {
                    height: h,
                    depth: bd,
                    ..
                } => {
                    cur += prev_dp + *h as i64;
                    prev_dp = *bd as i64;
                    if prev_dp > d {
                        cur += prev_dp - d;
                        prev_dp = d;
                    }
                    prev_breakable = true;
                    i += 1;
                    continue;
                }
                Node::Glue(g) => {
                    if !prev_breakable {
                        cur += prev_dp + g.width as i64;
                        prev_dp = 0;
                        let so = (g.stretch_order as usize).min(3);
                        act[so] += g.stretch as i64;
                        act[4] += g.shrink as i64;
                        if prev_dp > d {
                            cur += prev_dp - d;
                            prev_dp = d;
                        }
                        prev_breakable = false;
                        i += 1;
                        continue;
                    }
                    0
                }
                Node::Kern(k) | Node::ExplicitKern(k) => {
                    // tex.web @18973: a kern is a breakpoint only when the
                    // FOLLOWING node is glue; at the list end it counts as a
                    // penalty node type, which is not glue
                    let followed = matches!(list.get(i + 1), Some(Node::Glue(_)));
                    if !followed {
                        cur += prev_dp + *k as i64;
                        prev_dp = 0;
                        if prev_dp > d {
                            cur += prev_dp - d;
                            prev_dp = d;
                        }
                        prev_breakable = false;
                        i += 1;
                        continue;
                    }
                    0
                }
                Node::Penalty(p) => *p as i64,
                Node::Mark { .. } | Node::Ins { .. } => {
                    if prev_dp > d {
                        cur += prev_dp - d;
                        prev_dp = d;
                    }
                    prev_breakable = true;
                    i += 1;
                    continue;
                }
                _ => {
                    if prev_dp > d {
                        cur += prev_dp - d;
                        prev_dp = d;
                    }
                    prev_breakable = precedes_break(&list[i]);
                    i += 1;
                    continue;
                }
            }
        };
        // champion check (tex.web @18985): only penalties < inf_penalty
        if pi < INF_PENALTY as i64 {
            let mut b: i64 = if cur < w {
                if act[1] != 0 || act[2] != 0 || act[3] != 0 {
                    0
                } else {
                    badness(c32(w - cur), c32(act[0])) as i64
                }
            } else if cur - w > act[4] {
                AWFUL_BAD as i64
            } else {
                badness(c32(cur - w), c32(act[4])) as i64
            };
            if b < AWFUL_BAD as i64 {
                b = if pi <= EJECT_PENALTY as i64 {
                    pi
                } else if b < INF_BAD as i64 {
                    b + pi
                } else {
                    DEPLORABLE as i64
                };
            }
            if b <= least_cost {
                best = if i >= n { None } else { Some(i) };
                least_cost = b;
                best_bhpd = cur + prev_dp;
            }
            if b == AWFUL_BAD as i64 || pi <= EJECT_PENALTY as i64 {
                return (best, best_bhpd);
            }
        }
        // glue/kern candidates fold their own size in after the check
        // (the break discards the glue); penalty candidates fall through
        // to the depth clamp only
        if i < n {
            match &list[i] {
                Node::Glue(g) => {
                    cur += prev_dp + g.width as i64;
                    prev_dp = 0;
                    let so = (g.stretch_order as usize).min(3);
                    act[so] += g.stretch as i64;
                    act[4] += g.shrink as i64;
                }
                Node::Kern(k) | Node::ExplicitKern(k) => {
                    cur += prev_dp + *k as i64;
                    prev_dp = 0;
                }
                _ => {}
            }
            if prev_dp > d {
                cur += prev_dp - d;
                prev_dp = d;
            }
            prev_breakable = precedes_break(&list[i]);
        }
        i += 1;
        if i > n {
            return (best, best_bhpd);
        }
    }
}

/// a remembered page-breakpoint candidate
#[derive(Clone, Copy, Debug)]
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
        BreakSpot {
            cut,
            penalty,
            cost: DEPLORABLE,
        }
    }
}

/// tex.web page insertion node (`page_ins_node_size` @19193): one per class
/// appearing on the current page. `height_raw` mirrors |height(r)| — the RAW
/// (unscaled) height-plus-depth of `\box n` at class creation plus every
/// part placed since; `split_up` is |type(r)| = split_up; the `_ord` fields
/// identify `Node::Ins` contributions by their ordinal position in this
/// page's contribution order (tex.web node pointers |last_ins_ptr|,
/// |broken_ins|, |best_ins_ptr|; |broken_ptr| becomes `split_at`, the
/// vert_break best-place index inside that node's own vlist).
#[derive(Clone, Copy, Debug)]
pub struct PageInsState {
    /// insertion class (`subtype(r)`)
    pub num: u16,
    /// raw h+d already accounted for the class (tex.web `height(r)`)
    pub height_raw: i64,
    /// tex.web `type(r) = split_up`
    pub split_up: bool,
    /// ordinal of the node whose material overflowed (`broken_ins`)
    pub broken_ord: Option<usize>,
    /// vert_break best place inside `broken_ord`'s vlist (None = `null`)
    pub split_at: Option<usize>,
    /// tex.web `last_ins_ptr(r)`
    pub last_ord: Option<usize>,
    /// tex.web `best_ins_ptr(r)`: snapshotted from `last_ord` at the
    /// champion page break (§19565-19570), restored with the page state
    pub best_ord: Option<usize>,
}

impl PageInsState {
    fn new(num: u16, height_raw: i64) -> PageInsState {
        PageInsState {
            num,
            height_raw,
            split_up: false,
            broken_ord: None,
            split_at: None,
            last_ord: None,
            best_ord: None,
        }
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
    /// page insertion chain for this page, ascending by class (tex.web
    /// `page_ins_head`; ordered by class number per the §19602 insert loop)
    ins: Vec<PageInsState>,
    /// number of `Node::Ins` contributions scanned so far on this page:
    /// the ordinal source for the `_ord` fields above
    ins_ord: usize,
    /// a box or rule has been contributed (breaks need one before them)
    box_seen: bool,
    /// tex.web precedes_break(page_tail): the last contributed node is
    /// non-discardable (box/rule/ins/mark/adjust/whatsit) — a glue break is
    /// legal only right after such a node
    prev_breakable: bool,
    /// the node just processed is a legal break candidate (tex.web fires
    /// only at breakpoints: a box crossing the goal never fires)
    cur_legal: bool,
    best: Option<BreakSpot>,
    /// a fire condition was met this call
    fire: bool,
    /// `page_list` prefix already folded into the accounting
    processed: usize,
    /// page dimensions frozen after the first box or insertion
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
            ins: Vec::new(),
            ins_ord: 0,
            box_seen: false,
            prev_breakable: false,
            cur_legal: false,
            best: None,
            fire: false,
            processed: 0,
            goal_set: false,
        }
    }
}

impl Engine {
    /// The builder goal is latched from \vsize when a page starts; an
    /// unstarted page reports max_dimen. During \output, dim_param_value
    /// reads the completed page's register snapshot instead of this new
    /// page's counters.
    fn page_goal(&self) -> i64 {
        if !self.page_goal_set {
            return 0x3FFF_FFFF;
        }
        self.page_goal
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
        // Publish the builder goal; preserve this snapshot during \output.
        let goal = self.page_goal();
        self.eqtb.dim_params[DimParam::PageGoal.idx() as usize] = c32(goal);
    }
    /// tex.web §19610: the page-room weight of a raw insertion height —
    /// `x_over_n(raw,1000)*count(n)` (truncating), raw itself when count=1000
    fn ins_scaled_height(&self, num: u16, raw_h: i64) -> i64 {
        let cnt = self.eqtb.count[num as usize] as i64;
        if cnt == 1000 {
            raw_h
        } else {
            x_over_n(raw_h, 1000) * cnt
        }
    }

    pub fn build_page(&mut self) {
        // tex.web §1026 / build_page entry: `if output_active then return`,
        // unconditional. finish_output is the only place that clears the
        // flag (immediately before its final build_page); inferring liveness
        // from the input stack is not canonical.
        if self.in_output {
            return;
        }
        // page_list may have been swapped/restored by group or paragraph
        // handling (par_page_lists), which does not carry page_processed:
        // clamp the restored index instead of trusting it
        // page_list may have been swapped out (display math / paragraph
        // capture): the clamped index is NOT a real new position — never
        // persist it, or the carried prefix would re-contribute later
        let clamped = self.page_processed > self.page_list.len();

        let mut st = PageState {
            processed: self.page_processed.min(self.page_list.len()),
            goal_set: self.page_goal_set,
            insert_penalties: self.eqtb.int_params[IntParam::InsertPenalties.idx() as usize] as i64,
            ..PageState::new()
        };
        st.total = self.page_total;
        st.depth = self.page_depth;
        // tex.web keeps page_stretch/page_shrink as running state across
        // contribution batches; the processed prefix is NOT rescanned, so the
        // accumulated values must persist on the engine between build_page
        // calls (otherwise page badness sees only the current batch's glue
        // and every candidate evaluates as underfull-deplorable)
        st.stretch = self.page_stretch;
        st.shrink = self.page_shrink;
        if let Some(cut) = self.page_best_break {
            let mut spot = BreakSpot::carried(cut, self.page_break_penalty);
            spot.cost = self.page_best_cost as i32;
            st.best = Some(spot);
        }
        // tex.web `page_contents` is persistent engine state, not a scan of
        // the contribution-list prefix: display math and paragraph capture
        // swap the list, and output operations mutate it, so re-deriving
        // `box_seen` here re-inserts \topskip pads for pages that already
        // have a box and shifts stored breakpoint indices.
        st.box_seen = self.page_box_seen;
        // canonical page-insertion chain: restored with the rest of the page
        // state instead of re-derived from the prefix (the prefix's class
        // accounting — height(r), split flags, best_ins_ptr snapshots — is
        // per-node state that replaying cannot reconstruct)
        st.ins = std::mem::take(&mut self.page_insertions);
        // ordinals count Ins nodes already scanned on this page; held carries
        // are re-inserted ahead of the processed prefix, so count Ins nodes
        // before the resume point
        st.ins_ord = self.page_list[..st.processed]
            .iter()
            .filter(|n| matches!(n, Node::Ins { .. }))
            .count();

        while st.processed < self.page_list.len() {
            let idx = st.processed;
            let mut advance = true;
            st.cur_legal = false;
            match &self.page_list[idx] {
                Node::Glue(g) => {
                    self.last_page_glue = Some(g.clone());
                    self.last_page_penalty = 0;
                    self.last_page_kern = 0;
                    self.last_page_node_type = 11;
                }
                Node::Penalty(p) => {
                    self.last_page_glue = None;
                    self.last_page_penalty = *p;
                    self.last_page_kern = 0;
                    self.last_page_node_type = 13;
                }
                Node::Kern(k) | Node::ExplicitKern(k) => {
                    self.last_page_glue = None;
                    self.last_page_penalty = 0;
                    self.last_page_kern = *k;
                    self.last_page_node_type = 12;
                }
                Node::Box { kind, .. } => {
                    self.last_page_glue = None;
                    self.last_page_penalty = 0;
                    self.last_page_kern = 0;
                    self.last_page_node_type = if *kind == crate::boxes::HBOX { 1 } else { 2 };
                }
                Node::Rule { .. } => {
                    self.last_page_glue = None;
                    self.last_page_penalty = 0;
                    self.last_page_kern = 0;
                    self.last_page_node_type = 3;
                }
                Node::NativeGlyphRun { .. } => {
                    self.last_page_glue = None;
                    self.last_page_penalty = 0;
                    self.last_page_kern = 0;
                    self.last_page_node_type = 0;
                }
                Node::Ins { .. } => {
                    self.last_page_glue = None;
                    self.last_page_penalty = 0;
                    self.last_page_kern = 0;
                    self.last_page_node_type = 4;
                }
                Node::Mark { .. } => {
                    self.last_page_glue = None;
                    self.last_page_penalty = 0;
                    self.last_page_kern = 0;
                    self.last_page_node_type = 5;
                }
                _ => {
                    self.last_page_glue = None;
                    self.last_page_penalty = 0;
                    self.last_page_kern = 0;
                    self.last_page_node_type = -1;
                }
            }

            match &self.page_list[idx] {
                Node::Glue(g) => {
                    let g = g.clone();
                    if st.box_seen {
                        // tex.web evaluates a glue breakpoint BEFORE the glue
                        // contributes: its page total excludes the glue (the
                        // glue is discarded at the break)
                        // tex.web §19495-§19497: a page glue is a legal breakpoint
                        // iff the IMMEDIATELY preceding page node (page_tail) is
                        // non-discardable (precedes_break). Subsequent glues never
                        // break.
                        let legal0 =
                            st.box_seen && idx > 0 && precedes_break(&self.page_list[idx - 1]);
                        if legal0 {
                            self.try_page_break(&mut st, idx, 0);
                        }
                        self.contribute_glue(&mut st, &g);
                        st.cur_legal = legal0;
                        // tex.web §19490: a page glue is a legal breakpoint
                        // iff the preceding page node is non-discardable
                        // (precedes_break). Zero-width interline-glue
                        // placeholders from build_lines are transparent here
                        // — the real gap is inserted at the next box.
                    } else {
                        // tex.web: while page_contents<box_there the node is
                        // recycled (`goto done1`), not merely ignored — it
                        // must leave page_list or it lands in \box255 and
                        // renders as phantom space at the top of the page
                        self.page_list.remove(idx);
                        if let Some(spot) = st.best.as_mut() {
                            if idx < spot.cut {
                                spot.cut -= 1;
                            }
                        }
                        advance = false;
                    }
                }
                Node::Kern(k) | Node::ExplicitKern(k) => {
                    let k = *k;
                    if st.box_seen {
                        self.contribute_gap(&mut st, k as i64);
                    } else {
                        self.page_list.remove(idx);
                        if let Some(spot) = st.best.as_mut() {
                            if idx < spot.cut {
                                spot.cut -= 1;
                            }
                        }
                        advance = false;
                    }
                }
                Node::Penalty(p) => {
                    let p = *p;
                    if st.box_seen {
                        // legal break at penalties < inf_penalty when a box precedes
                        if p < INF_PENALTY && st.box_seen {
                            st.cur_legal = true;
                            self.try_page_break(&mut st, idx + 1, p);
                        }
                    } else {
                        self.page_list.remove(idx);
                        if let Some(spot) = st.best.as_mut() {
                            if idx < spot.cut {
                                spot.cut -= 1;
                            }
                        }
                        advance = false;
                    }
                }
                Node::Box { h, d, .. }
                | Node::NativeGlyphRun {
                    height: h,
                    depth: d,
                    ..
                }
                | Node::Rule {
                    height: h,
                    depth: d,
                    ..
                } => {
                    let (h, d) = (*h, *d);
                    // tex.web contributes every box, including 0x0 ones
                    // (`\box_there`); LaTeX's float/clearpage machinery
                    // plants empty `\vbox{}` markers purely so the following
                    // forced `\penalty -1000x` is a legal break that re-fires
                    // the output routine.
                    if !st.box_seen {
                        // first box on a fresh page: `\topskip` glue before it
                        // — tex.web inserts it whatever the box height, so a
                        // leading 0x0 box pads a full `\topskip`
                        if !st.goal_set {
                            st.goal_set = true;
                            self.page_goal = self.vsize_goal();
                            self.page_goal_set = true;
                            self.page_prev_depth = DEPTH_NONE;
                        }
                        let ts =
                            self.eqtb.dim_params[crate::prim::DimParam::TopSkip.idx() as usize];
                        let pad = (ts as i64 - h as i64).max(0) as i32;
                        self.page_list.insert(idx, Node::Glue(Glue::new(pad)));
                        // tex.web §19509-§19516: \topskip is linked ahead of the box,
                        // and build_page jumps to `continue` to process \topskip through
                        // the normal glue_node path. If precedes_break(page_tail) is true
                        // (e.g. whatsits/marks from output routine sit at the page top),
                        if idx > 0 && self.page_list[..idx].iter().any(precedes_break) {
                            self.try_page_break(&mut st, idx, 0);
                        }
                        self.contribute_gap(&mut st, pad as i64);
                        st.processed += 1;
                        if let Some(spot) = st.best.as_mut() {
                            if idx < spot.cut {
                                spot.cut += 1;
                            }
                        }
                    } else if self.page_prev_depth > DEPTH_NONE {
                        self.page_prev_depth = DEPTH_NONE;
                    }
                    st.box_seen = true;
                    self.contribute_box(&mut st, h, d);
                    self.page_prev_depth = d;
                }
                Node::Ins { .. } => {
                    // canonical vert_break reads the insertion's own inner
                    // vlist; lift the node out of the list so the &mut self
                    // call can borrow it without a deep copy
                    let node = self.page_list.remove(idx);
                    let ord = st.ins_ord;
                    st.ins_ord += 1;
                    self.contribute_ins(&mut st, ord, &node);
                    self.page_list.insert(idx, node);
                }
                Node::Mark { .. } => {
                    // tex.web: mark nodes contribute without affecting page dimensions;
                    // first_mark and bot_mark are updated only at fire_up for the chosen break
                }
                Node::Adj(a) => {
                    let a = *a;
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
        self.page_stretch = st.stretch;
        self.page_shrink = st.shrink;
        if !clamped {
            self.page_processed = st.processed;
        }
        self.page_best_break = st.best.map(|b| b.cut);
        self.page_break_penalty = st.best.map(|b| b.penalty).unwrap_or(0);
        self.page_best_cost = st.best.map(|b| b.cost as i64).unwrap_or(0);
        self.page_goal_set = st.goal_set;
        self.page_box_seen = st.box_seen;
        self.eqtb.int_params[IntParam::InsertPenalties.idx() as usize] =
            st.insert_penalties.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        // canonical page-ins chain persists with the rest of the page state
        // (tex.web keeps page_ins_head live across contribution batches)
        self.page_insertions = st.ins.clone();
        self.sync_page_dims(&st);
        if st.fire {
            let (cut, penalty, pack_goal) = match st.best {
                Some(spot) => (spot.cut, spot.penalty, self.page_best_goal),
                None => (st.processed, 0, self.page_goal()),
            };
            self.fire_up(cut, penalty, pack_goal);
        }
    }

    /// fold a box contribution into the page accounting (tex.web @1047):
    /// depth beyond `\maxdepth` is charged to height
    fn contribute_box(&mut self, st: &mut PageState, h: i32, d: i32) {
        let md = self.max_depth();
        st.total += st.depth + h as i64;
        let d64 = d as i64;
        if d64 > md {
            st.total += d64 - md;
            st.depth = md;
        } else {
            st.depth = d64;
        }
        st.box_seen = true;
        st.goal_set = true;
        if !self.page_goal_set {
            self.page_goal = self.vsize_goal();
            self.page_goal_set = true;
        }
    }
    /// fold glue: width folds the running depth away, stretch/shrink accrue
    /// by order (tex.web @1042)
    fn contribute_glue(&mut self, st: &mut PageState, g: &Glue) {
        self.contribute_gap(st, g.width as i64);
        let so = (g.stretch_order as usize).min(3);
        let ho = (g.shrink_order as usize).min(3);
        st.stretch[so] += g.stretch as i64;
        st.shrink[ho] += g.shrink as i64;
    }

    /// fold a kern-like gap (kern, \vadjust amount)
    fn contribute_gap(&mut self, st: &mut PageState, w: i64) {
        st.total += st.depth + w;
        st.depth = 0;
    }

    /// fold an insert contribution — canonical `contrib_list` insertion
    /// case (tex.web §§19596-19626 + §§19663-19679). `ord` is the node's
    /// ordinal among this page's `Node::Ins` contributions (stands in for
    /// the node pointer `p`). The class state fits the insertion only when
    /// BOTH the remaining page room (`page_goal - page_total - page_depth
    /// + page_shrink`, count-scaled) and `\dimen n` allow; otherwise the
    /// insertion's own vlist is broken with `vert_break(ins_ptr(p), w,
    /// depth(p))` using its LOCAL `\splitmaxdepth`/`\splittopskip`, only
    /// the selected part is subtracted from `page_goal`, and the class
    /// flips to `split_up` (later same-class nodes charge float_cost).
    /// tex.web §19607: once split_up, every further node of the class
    /// charges `\floatingpenalty` and is held for the next page.
    fn contribute_ins(&mut self, st: &mut PageState, ord: usize, node: &Node) {
        let (num, raw_h, cost, split_max_depth, inner): (u16, i64, i32, i32, NodeList) = match node
        {
            Node::Ins {
                num,
                height,
                depth,
                cost,
                split_max_depth,
                box_node,
                ..
            } => {
                let inner: NodeList = match &**box_node {
                    Node::Box { list, .. } => list.clone(),
                    other => vec![other.clone()],
                };
                (
                    *num,
                    *height as i64 + *depth as i64,
                    *cost,
                    *split_max_depth,
                    inner,
                )
            }
            _ => return,
        };
        // tex.web §19597: freeze page specs (inserts_only) when the page
        // has no box yet — the goal latch is vsize
        st.goal_set = true;
        if !self.page_goal_set {
            self.page_goal = self.vsize_goal();
            self.page_goal_set = true;
        }
        // §19598-19604: locate the class in the ascending page-ins chain —
        // pos is the first entry >= num (canonical walks past entries <= n,
        // then tests subtype(r)=n at the stop node)
        let pos = st.ins.iter().take_while(|s| s.num < num).count();
        if st.ins.get(pos).map_or(true, |s| s.num != num) {
            // §19631-19658 <Create a page insertion node ... include the
            // glue correction for box n in the current page state>: only
            // the FIRST insert n of a page reads \box n and \skip n
            let mut existing: Option<&Node> = None;
            match self.eqtb.boxed.get(num as usize) {
                Some(Some(b @ Node::Box { kind, .. })) if *kind != crate::boxes::HBOX => {
                    existing = Some(b)
                }
                Some(Some(other)) => existing = Some(other),
                _ => {}
            }
            let height_raw = match existing {
                Some(Node::Box { h, d, .. }) => *h as i64 + *d as i64,
                Some(_) => 0,
                None => 0,
            };
            self.page_goal -= self.ins_scaled_height(num, height_raw);
            if let Some(sk) = self.eqtb.skip.get(num as usize) {
                let sk = *sk;
                self.page_goal -= sk.width as i64;
                let so = (sk.stretch_order as usize).min(3);
                let ho = (sk.shrink_order as usize).min(3);
                st.stretch[so] += sk.stretch as i64;
                st.shrink[ho] += sk.shrink as i64;
            }
            st.ins.insert(pos, PageInsState::new(num, height_raw));
        }
        let s = &mut st.ins[pos];
        // §19610: class already split on this page — charge float_cost, hold
        if s.split_up {
            st.insert_penalties += cost as i64;
            return;
        }
        s.last_ord = Some(ord);
        // §19612-19619: the fit test — page room AND \dimen n
        let cnt = self.eqtb.count[num as usize] as i64;
        let budget = self.eqtb.dimen[num as usize] as i64;
        let delta = self.page_goal - st.total - st.depth + st.shrink[0];
        let h = if cnt == 1000 {
            raw_h
        } else {
            x_over_n(raw_h, 1000) * cnt
        };
        if (h <= 0 || h <= delta) && raw_h + s.height_raw <= budget {
            self.page_goal -= h;
            s.height_raw += raw_h;
            return;
        }
        // §19663-19679 <Find the best way to split the insertion>:
        // w = raw room left on the page, capped by the class budget
        let mut w = if cnt <= 0 {
            0x3FFF_FFFF
        } else {
            let room = self.page_goal - st.total - st.depth;
            if cnt != 1000 {
                x_over_n(room, cnt) * 1000
            } else {
                room
            }
        };
        if w > budget - s.height_raw {
            w = budget - s.height_raw;
        }
        let (q, bhpd_raw) = page_vert_break(&inner, w, split_max_depth as i64);
        s.height_raw += bhpd_raw;
        let charge = if cnt != 1000 {
            x_over_n(bhpd_raw, 1000) * cnt
        } else {
            bhpd_raw
        };
        self.page_goal -= charge;
        s.split_up = true;
        s.broken_ord = Some(ord);
        s.split_at = q;
        // §19677-19678: the break penalty rides into \insertpenalties
        if let Some(i) = q {
            if let Some(Node::Penalty(p)) = inner.get(i) {
                st.insert_penalties += *p as i64;
            }
        } else {
            st.insert_penalties += EJECT_PENALTY as i64;
        }
    }

    /// remember a breakpoint candidate if its cost beats the current best,
    /// then fire if the page is done. tex.web evaluates firing ONLY at
    /// candidates: a page whose total crosses the goal between candidates
    /// keeps growing until the next one, which may carry a better cost
    /// case — firing mid-run there cut one line early vs real pdflatex)
    fn try_page_break(&mut self, st: &mut PageState, cut: usize, penalty: i32) {
        if penalty >= 10000 {
            return;
        }
        let (b, cost) = self.break_cost(st, penalty);
        // tex.web §19563: `if c<=least_page_cost` — least starts at
        let better = match st.best {
            None => true,
            Some(spot) => cost <= spot.cost,
        };
        if self.eqtb.int_params[IntParam::TracingPages.idx() as usize] > 0 {
            let msg = format!(
                "% t={:.5} plus {:.1} minus {:.1} g={:.5} b={} p={} c={}{}\n",
                st.total as f64 / 65536.0,
                st.stretch[0] as f64 / 65536.0,
                st.shrink[0] as f64 / 65536.0,
                self.page_goal() as f64 / 65536.0,
                if b == AWFUL_BAD as i64 {
                    "*".to_string()
                } else {
                    b.to_string()
                },
                penalty,
                if cost == AWFUL_BAD {
                    "*".to_string()
                } else {
                    cost.to_string()
                },
                if better { "#" } else { "" }
            );
            self.append_log(&msg);
        }
        if (b < AWFUL_BAD as i64 || st.best.is_none()) && better {
            st.best = Some(BreakSpot { cut, penalty, cost });
            // tex.web best_size snapshots page_goal at the selected break.
            // Later insertions may reduce the running page_goal further.
            self.page_best_goal = self.page_goal();
            // tex.web §19566-19570: best_ins_ptr(r) := last_ins_ptr(r) for
            // every class — fire_up packages exactly the material that had
            // been accounted when THIS break was the champion.
            for s in st.ins.iter_mut() {
                s.best_ord = s.last_ord;
            }
        }
        // tex.web §1004-1005: fire iff this candidate is awful (overfull
        // beyond shrink) or a forcing penalty; ship the best champion.
        if cost == AWFUL_BAD || penalty <= EJECT_PENALTY {
            st.fire = true;
        }
    }
    /// tex.web @1003/@1004: returns (badness, cost = badness + penalty +
    /// `\insertpenalties`) for breaking at the current candidate
    fn break_cost(&self, st: &PageState, penalty: i32) -> (i64, i32) {
        let goal = self.page_goal();
        let b: i64 = if st.total < goal {
            // tex.web §1003: if any infinite stretch exists (fil/fill/filll),
            // the badness is zero; calling badness on the magnitude of the
            // infinite stretch treats it as finite and assigns deplorable cost.
            if st.stretch[1] != 0 || st.stretch[2] != 0 || st.stretch[3] != 0 {
                0
            } else {
                let want = (goal - st.total).min(i32::MAX as i64 / 2) as i32;
                let s = st.stretch[0].min(i32::MAX as i64 / 2) as i32;
                badness(want, s) as i64
            }
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
        } else {
            // tex.web §1004: overfull -> c := b = awful_bad (this also drives
            // the fire: `if c = awful_bad or pi <= eject_penalty`)
            b
        };
        // tex.web: an insert-heavy page forces the issue
        let c = if st.insert_penalties >= 10000 {
            AWFUL_BAD as i64
        } else {
            c
        };
        (b, c.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
    }

    /// Fires once the page builder has selected a breakpoint.
    fn ready_to_fire(&self, st: &PageState) -> bool {
        st.fire
    }
    fn fire_up(&mut self, cut: usize, _penalty: i32, pack_goal: i64) {
        // is inserted before it, preserving \lastskip/\lastpenalty semantics.
        let (cut, penalty) = match cut.checked_sub(1).and_then(|i| self.page_list.get_mut(i)) {
            Some(Node::Penalty(p)) => {
                let penalty = *p;
                *p = INF_PENALTY;
                (cut - 1, penalty)
            }
            _ => (cut, INF_PENALTY),
        };
        self.eqtb.int_params[IntParam::OutputPenalty.idx() as usize] = penalty;

        // marks: `\topmark` becomes the old `\botmark`; per-page marks reset
        self.marks[0] = self.marks[2].clone();
        for m in self.marks.iter_mut().skip(1) {
            *m = Vec::new();
        }

        let items: NodeList = self.page_list.drain(..cut).collect();
        // tex.web §1012-§1016: update first_mark and bot_mark from the page material
        let mut seen_first = std::collections::BTreeSet::new();
        let mut seen_bot = std::collections::BTreeSet::new();
        for node in &items {
            if let Node::Mark { class, tokens } = node {
                let c = (*class).max(0) as usize;
                if c < MAX_MARK_CLASS {
                    for m in self.marks.iter_mut() {
                        if m.len() <= c {
                            m.resize(c + 1, Vec::new());
                        }
                    }
                    if !seen_first.contains(&c) {
                        seen_first.insert(c);
                        self.marks[1][c] = tokens.clone();
                    }
                    seen_bot.insert(c);
                    self.marks[2][c] = tokens.clone();
                }
            }
        }
        // tex.web §1012: if first_mark is null (no mark on page), first_mark := top_mark
        for c in 0..self.marks[0].len() {
            if !seen_first.contains(&c) {
                if !self.marks[0][c].is_empty() {
                    if self.marks[1].len() <= c {
                        self.marks[1].resize(c + 1, Vec::new());
                    }
                    self.marks[1][c] = self.marks[0][c].clone();
                }
            }
            if !seen_bot.contains(&c) {
                if !self.marks[0][c].is_empty() {
                    if self.marks[2].len() <= c {
                        self.marks[2].resize(c + 1, Vec::new());
                    }
                    self.marks[2][c] = self.marks[0][c].clone();
                }
            }
        }

        self.page_processed = self.page_processed.saturating_sub(cut);

        self.page_best_break = None;
        self.page_break_penalty = 0;
        self.page_best_cost = 0;
        self.page_best_goal = 0x3FFF_FFFF;
        self.last_page_glue = None;
        self.last_page_penalty = 0;
        self.last_page_kern = 0;
        self.last_page_node_type = -1;

        // canonical fire_up packaging (tex.web §§19827-19876): walk the page
        // material in contribution order; each class receives the material
        // of its insert nodes up to `best_ord` (the node snapshotted as
        // best_ins_ptr at the champion break). The class box is packed when
        // that node is reached; if the class flipped to `split_up` at the
        // same node, the node's own vlist is broken at `split_at`, pruned
        // with the node's LOCAL `\splittopskip`, and the remainder is held
        // over as an insert node. Nodes past the champion (or of classes
        // with no champion) are held whole. `\insertpenalties` counts held
        // nodes (§19752). tex.web §1014: if \holdinginserts > 0, inserts
        // stay on the page list (inside \box255) instead.
        let holding = self.eqtb.int_params[IntParam::HoldingInserts.idx() as usize] > 0;
        let mut states: BTreeMap<u16, PageInsState> =
            self.page_insertions.iter().map(|s| (s.num, *s)).collect();
        let mut page_mat: NodeList = Vec::new();
        // per-class queue: existing \box n content + appended insert material
        let mut queues: BTreeMap<u16, NodeList> = BTreeMap::new();
        let mut carried: NodeList = Vec::new();
        let mut ord = 0usize;
        for node in items {
            match node {
                ins @ Node::Ins { .. } if !holding => {
                    let (num, cost, topskip, splitmax, inner) = match &ins {
                        Node::Ins {
                            num,
                            cost,
                            split_top_skip,
                            split_max_depth,
                            box_node,
                            ..
                        } => {
                            let inner: NodeList = match &**box_node {
                                Node::Box { list, .. } => list.clone(),
                                other => vec![other.clone()],
                            };
                            (*num, *cost, *split_top_skip, *split_max_depth, inner)
                        }
                        _ => unreachable!(),
                    };
                    let cur_ord = ord;
                    ord += 1;
                    // §19871: best_ins_ptr(r)=null -> hold this node whole
                    let wrap = match states.get(&num) {
                        Some(s) => s.best_ord,
                        None => None,
                    };
                    if wrap != Some(cur_ord) {
                        // §19871-19877: best_ins_ptr(r)=null holds the node
                        // whole; ordinals before the champion append to the
                        // queue (equality wraps up below); ordinals after a
                        // consumed wrap-up hold whole
                        let held = match wrap {
                            None => true,
                            Some(o) => cur_ord > o,
                        };
                        if held {
                            carried.push(ins);
                        } else {
                            let queue = queues.entry(num).or_insert_with(|| {
                                // §19835-19843 <Prepare all the boxes... to
                                // act as queues>: existing \box n content
                                // opens the queue
                                match self.eqtb.boxed[num as usize].take() {
                                    Some(Node::Box { list, .. }) => list,
                                    Some(other) => vec![other],
                                    None => Vec::new(),
                                }
                            });
                            queue.extend(inner);
                        }
                        continue;
                    }
                    // §19874: best_ins_ptr(r)=p — append, then wrap up
                    let queue = queues.entry(num).or_insert_with(|| {
                        match self.eqtb.boxed[num as usize].take() {
                            Some(Node::Box { list, .. }) => list,
                            Some(other) => vec![other],
                            None => Vec::new(),
                        }
                    });
                    let base = queue.len();
                    queue.extend(inner);
                    // §19853 <Wrap up the box specified by node r, splitting
                    // node p if called for>
                    let s = states[&num];
                    let mut remainder: Option<NodeList> = None;
                    if s.split_up && s.broken_ord == Some(cur_ord) {
                        if let Some(i) = s.split_at {
                            // §19855-19856: cut at broken_ptr, prune the
                            // tail with this node's LOCAL \splittopskip
                            let pos = (base + i).min(queue.len());
                            let rest: NodeList = queue.drain(pos..).collect();
                            let pruned = prune_page_top_list(rest, &topskip);
                            if !pruned.is_empty() {
                                remainder = Some(pruned);
                            }
                        }
                    }
                    // §19862-19865: box n := vpack(temp_ptr, natural)
                    let q = std::mem::take(queue);
                    queues.remove(&num);
                    let r = vpack(q, None, VBOX, &self.eqtb);
                    self.eqtb.assign_box(num, Some(r.node), true);
                    if let Some(rest) = remainder {
                        // §19857-19860: height(p) := extents of the packed
                        // pruned remainder; node p itself carries it
                        let rr = vpack(rest, None, VBOX, &self.eqtb);
                        let (rh, rd) = match &rr.node {
                            Node::Box { h, d, .. } => (*h, *d),
                            _ => (0, 0),
                        };
                        carried.push(Node::Ins {
                            num,
                            height: rh,
                            depth: rd,
                            cost,
                            split_top_skip: topskip,
                            split_max_depth: splitmax,
                            box_node: Box::new(rr.node),
                        });
                    }
                    if let Some(s) = states.get_mut(&num) {
                        s.best_ord = None;
                    }
                }
                other => page_mat.push(other),
            }
        }
        // classes whose champion node never appeared in [0..cut) (defensive:
        // canonical packaging would have hit it): pack what was queued
        for (num, q) in queues {
            if !q.is_empty() {
                let r = vpack(q, None, VBOX, &self.eqtb);
                self.eqtb.assign_box(num, Some(r.node), true);
            }
        }
        // §19860 <Delete the page-insertion nodes>
        self.page_insertions.clear();
        // §19752/§19893: insert_penalties counts the held-over nodes
        self.eqtb.int_params[IntParam::InsertPenalties.idx() as usize] = carried.len() as i32;
        // §19817-19819 + §19904-19911/§19948-19953: held insertions go to
        // the HEAD of the contribution list, ahead of the remainder; the
        // next page contributes everything (holds + remainder) from a fresh
        // canonical state, so no processed prefix may survive
        let n_carry = carried.len();
        if n_carry > 0 {
            for (k, c) in carried.into_iter().enumerate() {
                self.page_list.insert(k, c);
            }
            self.page_processed = 0;
        } else if self.page_processed > 0
            && self.page_list[..self.page_processed]
                .iter()
                .any(|n| matches!(n, Node::Ins { .. }))
        {
            // insertions scanned past the champion break: canonical puts
            // every post-break node back on the contribution list
            // unprocessed
            self.page_processed = 0;
        }
        // tex.web page-top invariant: box255 opens with whatsits/marks, the
        // \topskip pad, and the first box — nothing else may sit between the
        // pad and that box. Discardable glue/kern/penalty nodes can re-enter
        // ahead of the first box when the output routine reinserts held
        // material (float fires) after the strip already ran; real TeX
        // discards top-of-page discardables, so drop all but the last glue
        // (the pad) here
        if let Some(fb) = page_mat
            .iter()
            .position(|n| matches!(n, Node::Box { .. } | Node::Rule { .. }))
        {
            let mut head: NodeList = Vec::new();
            let mut pad: Option<Node> = None;
            for n in page_mat.drain(..fb) {
                match n {
                    Node::Glue(_) | Node::Kern(_) | Node::ExplicitKern(_) => pad = Some(n),
                    Node::Penalty(_) => {}
                    other => head.push(other),
                }
            }
            head.extend(pad);
            head.append(&mut page_mat);
            page_mat = head;
        }

        // Reset next-page accounting without erasing the completed page's
        // goal, total, or glue registers seen by \output. Page depth is the
        // exception: TeX clears it in "Start a new current page".
        // \insertpenalties is NOT cleared here: tex.web §19752 leaves the
        // held-over count live for the output routine and the next page's
        // accumulation; it is zeroed at <Resume the page builder> (§19942)
        // in finish_output.
        self.page_total = 0;
        self.page_depth = 0;
        // tex.web "Start a new current page" clears depth before \output.
        self.eqtb.dim_params[DimParam::PageDepth.idx() as usize] = 0;
        self.page_stretch = [0; 4];
        self.page_shrink = [0; 4];
        self.page_prev_depth = DEPTH_NONE;
        self.page_goal = 0x3FFF_FFFF;
        self.page_goal_set = false;
        // <Start a new current page> (tex.web §19955): page_contents := empty
        self.page_box_seen = false;
        // \output, TeX rebuilds the next page from scratch at <Resume the page
        // builder> (routine list + remainder), so the fold (strip/pad/prefix
        // accounting) must not run — finish_output zeroes page_processed and
        // build_page contributes the spliced head + remainder nodes afresh.
        // tex.web §28435-28438: the gate tests dead_cycles BEFORE this
        // fire's increment (incr happens inside <Fire up>): the routine runs
        // while dead_cycles < maxdeadcycles; at the limit TeX explains the
        // loop and falls through to the default output (which ships box255,
        // and ship_out resets the counter).
        let maxdc = self.eqtb.int_params[IntParam::MaxDeadCycles.idx() as usize].max(0);
        let toks = (*self.eqtb.tok_params[ToksParam::Output.idx() as usize]).clone();
        let will_routine = !toks.is_empty() && self.dead_cycles < maxdc;
        if !will_routine {
            // tex.web fresh-page parity for the carried-over processed prefix:
            // strip stale leading discardables (old interline glue, break
            // glue, placeholders) ahead of the carried first box. tex.web
            // contributes whatsits and marks even at a page top (they must
            // ship), so they stay put while discardables around them are
            // recycled (§19490: page_contents < box_there)
            let mut i = 0;
            let mut removed_prefix = 0usize;
            // strip leading discardables of the whole remainder, prefix and
            // fresh tail alike: tex.web discards glue/kern/penalty as they
            // arrive at a fresh page top (§19490 page_contents < box_there),
            // and the prefix fold below forces goal_set=true before the tail
            // would otherwise recycle them
            while i < self.page_list.len() {
                match &self.page_list[i] {
                    Node::Glue(_) | Node::Kern(_) | Node::ExplicitKern(_) | Node::Penalty(_) => {
                        if i < self.page_processed - removed_prefix {
                            removed_prefix += 1;
                        }
                        self.page_list.remove(i);
                    }
                    Node::Whatsit(_) | Node::Mark { .. } => i += 1,
                    _ => break,
                }
            }
            self.page_processed -= removed_prefix;
            if self.page_processed > 0 {
                // the first box of the prefix gets the \topskip pad measured
                // against IT (tex.web inserts the pad right before that box,
                // after any page-top whatsits/marks)
                let fb = self.page_list[..self.page_processed]
                    .iter()
                    .position(|n| matches!(n, Node::Box { .. } | Node::Rule { .. }));
                if let Some(fb) = fb {
                    // the carried prefix contributes a box: page_contents
                    // reaches box_there through the fold (tex.web §19955
                    // reset + re-contribution)
                    self.page_box_seen = true;
                    let h = match &self.page_list[fb] {
                        Node::Box { h, .. } => *h,
                        Node::Rule { height: h, .. } => *h,
                        _ => unreachable!(),
                    };
                    let ts = self.eqtb.dim_params[DimParam::TopSkip.idx() as usize];
                    let pad = (ts as i64 - h as i64).max(0) as i32;
                    if pad > 0 {
                        self.page_list.insert(fb, Node::Glue(Glue::new(pad)));
                        self.page_processed += 1;
                    }
                    // fold the carried prefix into the fresh page's
                    // accounting (contribute_box/gap semantics)
                    let mut total = 0i64;
                    let mut depth = 0i64;
                    let mut prev_d = DEPTH_NONE;
                    let md64 = self.max_depth();
                    let mut stretch = [0i64; 4];
                    let mut shrink = [0i64; 4];
                    for node in &self.page_list[..self.page_processed] {
                        match node {
                            Node::Glue(g) => {
                                total += depth + g.width as i64;
                                depth = 0;
                                let so = (g.stretch_order as usize).min(3);
                                let ho = (g.shrink_order as usize).min(3);
                                stretch[so] += g.stretch as i64;
                                if so > 0 && g.width != 0 {
                                    stretch[so] += g.width as i64;
                                }
                                shrink[ho] += g.shrink as i64;
                                if ho > 0 && g.width != 0 {
                                    shrink[ho] += g.width as i64;
                                }
                            }
                            Node::Kern(k) | Node::ExplicitKern(k) => {
                                total += depth + *k as i64;
                                depth = 0;
                            }
                            Node::Box { h, d, .. }
                            | Node::Rule {
                                height: h,
                                depth: d,
                                ..
                            } => {
                                total += depth + *h as i64;
                                depth = (*d as i64).min(md64);
                                prev_d = *d;
                            }
                            _ => {}
                        }
                    }
                    self.page_stretch = stretch;
                    self.page_shrink = shrink;
                    self.page_total = total;
                    self.page_depth = depth;
                    self.page_prev_depth = prev_d;
                    self.page_goal_set = true;
                }
            }
        }

        let md = self.max_depth().min(i32::MAX as i64) as i32;
        // NOTE: tex.web §19905 says `vpack(q, natural)`, but real pdfTeX
        // packs \box255 to \pagegoal when the page has finite shrink/
        // stretch to absorb (oracle: BOX: 30.0pt = goal, not 25 = natural).
        // The exact-goal pack below matches the empirical contract that
        // LaTeX/ltxgrid rely on (\ht\box255 == \pagegoal at \output).
        let exact = (pack_goal < 0x3FFF_FFFF)
            .then_some(pack_goal.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
        let r = crate::boxes::vpack_add_md(page_mat, exact, false, VBOX, &self.eqtb, md);
        self.eqtb.assign_box(255, Some(r.node), true);

        // tex.web §28435-28439: with no routine (or once the dead-cycle
        // limit is reached, after explaining the loop) fall through to
        // <Perform the default output routine>: ship box255. ship_out
        // resets dead_cycles (§19147), so the counter never needs a
        // second reset here.
        if toks.is_empty() || self.dead_cycles >= maxdc {
            if !toks.is_empty() {
                self.error(&format!(
                    "Output loop---{} consecutive dead cycles",
                    self.dead_cycles
                ));
            }
            let b = self.eqtb.boxed[255].take();
            self.ship_box(b);
            return;
        }
        // <Fire up the user's output routine> §28633-28634:
        // output_active:=true; incr(dead_cycles)
        self.dead_cycles += 1;
        self.in_output = true;
        self.output_depth += 1;
        // tex.web push_nest: the routine runs on a fresh list; Rust defers the
        // cursor until the routine's first dispatch so that post-fire appends
        // inside the firing primitive (remaining chunk rows) stay contribution
        // material at the list tail.
        self.output_pending = true;
        // (output_group) — its local assignments (\@restorepar's \def\par,
        // \@specials, mark state) roll back at <endoutput> instead of
        // clobbering the enclosing list's eqtb state
        self.push_group_level(crate::eqtb::LevelType::Simple);
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
            if !self.try_push_tokens_named(rest, "<after-output>") {
                return;
            }
        }
        // LIFO input stack: continuation first, then the routine.

        if !self.try_push_tokens_named(vec![OUT_END_TOKEN], "<endoutput>") {
            return;
        }
        self.push_tokens_named(toks, "<output>");
    }

    // (tex.web §§19827-19876 insertion packaging lives inline in fire_up;
    // the old `place_insert`/`carry_insert` split-to-`\dimen`-only model is
    // superseded — canonical splits at contribute time against BOTH page
    // room and the class budget, and holds the broken node itself.)

    pub fn finish_output(&mut self) {
        // <Resume the page builder> (tex.web §28649-28663, exact order):
        // end_graf; unsave; output_active:=false; insert_penalties:=0;
        // <Ensure that box 255 is empty after output>; splice the routine's
        // own current list (internal vmode, §28635) into the contribution
        // list AFTER the held-over insertions and BEFORE the remainder;
        // pop_nest; build_page. Nothing the routine left on its list is
        // discarded — ltxgrid's dead@cycle relies on the re-inserted
        // material (and its trailing \penalty\outputpenalty) re-triggering
        // the routine for the moving pass.
        if self.mode == Mode::Horizontal {
            // end_graf: a routine that ends mid-paragraph closes it first so
            // the line lands on the routine's list before the splice
            self.end_paragraph();
        }
        let routine_list = std::mem::take(&mut self.cur_list);
        // The whole page rebuilds from scratch after the splice (head copy
        // included), so any carried-over processed prefix is void.
        self.page_processed = 0;
        self.page_box_seen = false;
        self.page_goal_set = false;
        self.page_total = 0;
        self.page_depth = 0;
        self.page_stretch = [0; 4];
        self.page_shrink = [0; 4];
        self.page_best_break = None;
        self.page_best_cost = 0;
        self.page_break_penalty = 0;
        self.sync_page_dims(&PageState::new());
        // close the save level opened at fire_up (tex.web output_group) BEFORE
        // inspecting box255: a non-global `\setbox255` inside the routine is
        // rolled back by unsave, and the rolled-back value is what TeX checks.
        self.pop_group();
        // tex.web §28650: insert_penalties := 0 at <Resume the page builder>
        self.eqtb.int_params[IntParam::InsertPenalties.idx() as usize] = 0;
        // tex.web <Ensure that box 255 is empty after output>: after unsave,
        // any surviving `\box255` material is reported and discarded.
        if self.eqtb.boxed[255].is_some() {
            self.error("Output routine didn't use all of \\box255");
            self.eqtb.boxed[255] = None;
        }
        self.in_output = false;
        self.output_depth = self.output_depth.saturating_sub(1);
        // §28652-28661: the routine's list goes AFTER the held-over
        // insertions (cursor index snapshotted at activation) and BEFORE
        // the contribution remainder; then pop_nest restores the
        // interrupted nest's mode, list, space factor, prevdepth, prevgraf.
        let (c, saved_pd, saved_pg, saved_mode) =
            self.output_tail
                .take()
                .unwrap_or((0, self.prev_depth, self.prev_graf, Mode::Vertical));
        let c = c.min(self.page_list.len());
        if !routine_list.is_empty() {
            for (k, n) in routine_list.into_iter().enumerate() {
                self.page_list.insert(c + k, n);
            }
        }
        self.prev_depth = saved_pd;
        self.prev_graf = saved_pg;
        self.mode = saved_mode;
        if let Some((parked_list, parked_sf)) = self.output_nest.take() {
            self.cur_list = parked_list;
            self.space_factor = parked_sf;
        }
        // §28663: pop_nest; build_page
        if self.output_depth == 0 {
            self.build_page();
        }
    }
    /// fire deferred \write/\openout/\closeout whatsits found anywhere in a
    /// shipped tree, in list order (tex.web `out_what` @1414)
    fn fire_page_writes(&mut self, n: &Node) {
        match n {
            Node::Whatsit(crate::boxes::WhatIt::Write {
                stream,
                tokens,
                source,
            }) => {
                let toks = tokens.clone();
                self.fire_write(*stream, &toks, source.as_ref());
            }
            Node::Whatsit(crate::boxes::WhatIt::OpenOut {
                stream,
                path,
                create_parent,
                source,
            }) => {
                let p = path.clone();
                self.exec_openout(*stream, &p, *create_parent, source.as_ref());
            }
            Node::Whatsit(crate::boxes::WhatIt::CloseOut { stream, source }) => {
                self.exec_closeout(*stream, source.as_ref());
            }
            Node::Box { list, .. } => {
                for m in list {
                    self.fire_page_writes(m);
                }
            }
            Node::Ins { box_node, .. } => self.fire_page_writes(box_node),
            Node::VAdjust(v) => {
                for m in v {
                    self.fire_page_writes(m);
                }
            }
            _ => {}
        }
    }

    /// \shipout received a box: emit a PDF page
    pub fn ship_box(&mut self, b: Option<Node>) {
        self.dead_cycles = 0;
        let Some(boxn) = b else { return };

        // tex.web §1395 / out_what: deferred \write/\openout/\closeout whatsits
        // fire during render_page in list order so \pdflastxpos/\pdflastypos
        // from earlier \pdfsavepos nodes are visible.

        if self.eqtb.int_params[IntParam::TracingPages.idx() as usize] > 0 {
            let page = self.eqtb.count[0] as i64 + 1;
            let msg = format!("[{}]", page);
            self.append_term(&msg);
            self.append_log(&msg);
        }
        if self.eqtb.int_params[IntParam::TracingOutput.idx() as usize] > 0 {
            let depth = self.eqtb.int_params[IntParam::ShowBoxDepth.idx() as usize].max(0) as usize;
            let breadth =
                self.eqtb.int_params[IntParam::ShowBoxBreadth.idx() as usize].max(0) as usize;
            let mut out = crate::maincontrol::InspectionText::new();
            out.push(format_args!("\nCompleted box being shipped out\n"));
            self.show_node_into(&boxn, 0, depth, breadth, &mut out);
            let desc = out.finish();
            self.append_log(&desc);
            self.append_term(&desc);
        }
        // page size
        let width = self.eqtb.dim_params[DimParam::PdfPageWidth.idx() as usize];
        let height = self.eqtb.dim_params[DimParam::PdfPageHeight.idx() as usize];
        let _ = (width, height);
        let page = self.render_page(&boxn);
        self.pdf_doc.push_page(page);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> Engine {
        let mut e = Engine::new(true);
        e.init_primitives();
        e.add_nullfont();
        let full = format!(
            "\\catcode`\\{{=1 \\catcode`\\}}=2 \\catcode`\\#=6 \\catcode`\\&=4 {}\n",
            src
        );
        e.input
            .push_file("page.tex".to_string(), full.as_bytes().to_vec());
        e.run();
        e
    }

    fn vbox_parts(n: &Node) -> (usize, i32) {
        match n {
            // count line boxes only: vlist_append inserts interline glue
            // between them since d8b1214c
            Node::Box { list, h, .. } => (
                list.iter()
                    .filter(|n| matches!(n, Node::Box { .. }))
                    .count(),
                *h,
            ),
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
    fn split_shrink_error_requires_selected_ignore_bit() {
        for (mask, errors) in [(0, 1), (1, 0), (2, 1)] {
            let e = run(&format!(
                "\\ignoreprimitiveerror={mask}\
                 \\setbox0=\\vbox{{\\hrule height1pt\\vskip0pt minus1fil\\hrule height1pt}}\
                 \\setbox1=\\vsplit0 to100pt"
            ));
            assert_eq!(e.error_count, errors, "ignore mask {mask}: {}", e.term);
        }
    }
}
