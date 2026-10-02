//! The semantic nest as `show_activities` (tex.web §218) prints it. Ratex
//! keeps some of tex.web's nest levels elsewhere: the rows and the row level
//! of an alignment live in the alignment's tables, and the levels of math
//! (`{...}` groups, script arguments, denominators) are entries of
//! `math_lists`. This module puts them back.

use super::items::{
    empty_noad, empty_noad_kind, scripts_allowed, set_op_subtype, view_list, Field,
    FracItem, Item, NoadKind, Unset,
};
use crate::align::{AlignView, Cell, NOALIGN_SPAN};
use crate::boxes::{glue_subtype, glue_sums, Glue, Node};
use crate::engine::{Engine, Mode};
use crate::prim::{DimParam, GlueParam};
use crate::show_state::ScanKind;

pub(super) struct NestLevel<'a> {
    pub(super) mode: Mode,
    pub(super) line: i32,
    pub(super) list: &'a [Node],
    /// display items that precede `list` (an alignment's rows)
    pub(super) prefix: Vec<Item<'a>>,
    /// the complete list of a math level
    pub(super) math: Option<Vec<Item<'a>>>,
    /// a math level's `incompleat_noad`
    pub(super) incompleat: Option<Item<'a>>,
    pub(super) prev_depth: i32,
    pub(super) space_factor: i32,
    pub(super) prev_graf: i32,
    /// index of the frame this level comes from (the top level has index
    /// `saved_lists.len()`); none for levels made up for the display
    pub(super) frame: Option<usize>,
}

/// The visible nodes of `list` as display items.
pub(super) fn plain_items(list: &[Node]) -> Vec<Item<'_>> {
    list.iter()
        .filter(|n| !super::invisible(n))
        .map(Item::Node)
        .collect()
}

fn is_math(mode: Mode) -> bool {
    matches!(mode, Mode::Math | Mode::DisplayMath)
}

struct MathLevel<'a> {
    mode: Mode,
    line: i32,
    items: Vec<Item<'a>>,
    incompleat: Option<Item<'a>>,
}

impl Engine {
    /// The semantic nest, bottom level first (tex.web `nest[0..nest_ptr]`).
    pub(super) fn nest_levels(&self) -> Vec<NestLevel<'_>> {
        let frames = &self.saved_lists;
        let output = self.output_tail.map(|tail| (self.output_nest_mark, tail));
        let line_of = |i: usize| -> i32 {
            match output {
                Some(((base, line), _)) if base == i => line,
                _ if i == 0 => 0,
                _ => frames[i - 1].5,
            }
        };
        let mut levels = Vec::with_capacity(frames.len() + 2);
        for i in 0..=frames.len() {
            if let Some(((base, _), (_, pd, pg, mode))) = output {
                if base == i {
                    let (list, sf) = self
                        .output_nest
                        .as_ref()
                        .map_or((&[][..], 1000), |(l, sf)| (l.as_slice(), *sf));
                    levels.push(NestLevel {
                        mode,
                        line: if i == 0 { 0 } else { frames[i - 1].5 },
                        list,
                        prefix: Vec::new(),
                        math: None,
                        incompleat: None,
                        prev_depth: pd,
                        space_factor: sf,
                        prev_graf: pg,
                        frame: None,
                    });
                }
            }
            let level = match frames.get(i) {
                Some((mode, list, pd, sf, pg, _)) => NestLevel {
                    mode: *mode,
                    line: line_of(i),
                    list,
                    prefix: Vec::new(),
                    math: None,
                    incompleat: None,
                    prev_depth: *pd,
                    space_factor: *sf,
                    prev_graf: *pg,
                    frame: Some(i),
                },
                None => NestLevel {
                    mode: self.mode,
                    line: line_of(i),
                    list: &self.cur_list,
                    prefix: Vec::new(),
                    math: None,
                    incompleat: None,
                    prev_depth: self.prev_depth,
                    space_factor: self.space_factor,
                    prev_graf: self.prev_graf,
                    frame: Some(i),
                },
            };
            levels.push(level);
        }
        self.insert_align_levels(&mut levels);
        self.expand_math_levels(levels)
    }

    /// The nest as `tex.nest` shows it: tex.web's levels, bottom first, as
    /// `(mode, mode_line, prev_depth, space_factor, prev_graf, frame)`. Levels
    /// made up for the display (output routine, alignment rows, math groups) have
    /// no `frame`; like `push_nest` they repeat the aux field of the level below.
    pub(crate) fn lua_nest_view(&self) -> Vec<(Mode, i32, i32, i32, i32, Option<usize>)> {
        let mut out = Vec::new();
        let (mut pd, mut sf) = (self.prev_depth, 1000);
        for lvl in self.nest_levels() {
            if lvl.frame.is_some() {
                pd = lvl.prev_depth;
                sf = lvl.space_factor;
            }
            out.push((lvl.mode, lvl.line, pd, sf, lvl.prev_graf, lvl.frame));
        }
        out
    }

    // ---------------------------------------------------------------
    // alignments (tex.web §768-§812)
    // ---------------------------------------------------------------

    fn insert_align_levels<'a>(&'a self, levels: &mut Vec<NestLevel<'a>>) {
        let n = self.show.aligns.len();
        let usable = (0..n)
            .take_while(|&d| self.align_view(d).is_some())
            .count();
        for depth in (0..usable).rev() {
            let a = &self.show.aligns[n - 1 - depth];
            let Some(v) = self.align_view(depth) else {
                continue;
            };
            let Some(idx) = levels.iter().position(|l| l.frame == Some(a.level)) else {
                continue;
            };
            let pd0 = levels[idx].prev_depth;
            let sf0 = if idx > 0 { levels[idx - 1].space_factor } else { 1000 };
            let (rows, pd) = self.align_row_items(&v, pd0);
            if v.in_noalign {
                // the \noalign material is the alignment level's own list
                if idx + 1 < levels.len() {
                    let mut lvl = levels.remove(idx + 1);
                    lvl.prefix = rows;
                    lvl.line = a.line;
                    lvl.frame = Some(a.level);
                    lvl.prev_graf = 0;
                    levels[idx] = lvl;
                }
                continue;
            }
            let lvl = &mut levels[idx];
            lvl.prefix = rows;
            lvl.line = a.line;
            lvl.prev_graf = 0;
            if v.is_valign {
                lvl.space_factor = sf0;
            } else {
                lvl.prev_depth = pd;
            }
            if idx + 1 < levels.len() {
                let row = NestLevel {
                    mode: if v.is_valign {
                        Mode::InternalVertical
                    } else {
                        Mode::RestrictedHorizontal
                    },
                    line: a.row_line,
                    list: &[],
                    prefix: self.align_row_cells(&v),
                    math: None,
                    incompleat: None,
                    prev_depth: 0,
                    space_factor: 0,
                    prev_graf: 0,
                    frame: None,
                };
                levels.insert(idx + 1, row);
            }
        }
    }

    /// The tabskip glue after grid column `col`.
    fn align_tab_after(&self, v: &AlignView<'_>, col: usize) -> Glue {
        let pre = v.preamble;
        let g = if pre.is_empty() {
            v.t0
        } else if col < pre.len() {
            pre[col].tabskip
        } else {
            match v.loop_start {
                Some(ls) if pre.len() > ls => pre[ls + (col - ls) % (pre.len() - ls)].tabskip,
                _ => v.t0,
            }
        };
        g.param(glue_subtype::TAB_SKIP)
    }

    /// tex.web fin_col §796: the unset box of one finished cell.
    fn align_unset_cell<'a>(&self, cell: &'a Cell) -> Option<Item<'a>> {
        let Node::Box { w, h, d, list, .. } = cell.packed.as_ref()? else {
            return None;
        };
        let (stretch, shrink) = glue_sums(list);
        let top = |v: &[i64; 4]| (0..4).rev().find(|&o| v[o] != 0).unwrap_or(0);
        let (so, ko) = (top(&stretch), top(&shrink));
        Some(Item::Unset(Box::new(Unset {
            h: *h,
            d: *d,
            w: *w,
            span: cell.span,
            stretch: (stretch[so] != 0).then_some((stretch[so] as i32, so as u8)),
            shrink: (shrink[ko] != 0).then_some((shrink[ko] as i32, ko as u8)),
            list: plain_items(list),
        })))
    }

    /// The list of the row level (tex.web init_row, fin_col): the tabskip
    /// glue in front of the first column, then every finished cell with the
    /// tabskip glue after its last column.
    fn align_row_cells<'a>(&self, v: &AlignView<'a>) -> Vec<Item<'a>> {
        let mut items = vec![Item::Glue(v.t0.param(glue_subtype::TAB_SKIP))];
        for (c, cell) in v.cur_row.iter().enumerate() {
            if let Some(unset) = self.align_unset_cell(cell) {
                items.push(unset);
                items.push(Item::Glue(self.align_tab_after(v, c + cell.span as usize)));
            }
        }
        items
    }

    /// The alignment level's list so far (rows set natural, with the
    /// interline glue `append_to_vlist` added) and its final `prev_depth`.
    fn align_row_items<'a>(&self, v: &AlignView<'a>, pd0: i32) -> (Vec<Item<'a>>, i32) {
        let valign = v.is_valign;
        let ignore = self.ignore_depth();
        let bs = self.eqtb.glue_params[GlueParam::BaselineSkip.idx() as usize];
        let ls = self.eqtb.glue_params[GlueParam::LineSkip.idx() as usize];
        let lsl = self.eqtb.dim_params[DimParam::LineSkipLimit.idx() as usize];
        let mut items: Vec<Item<'a>> = Vec::new();
        let mut pd = pd0;
        let mut prev = (!valign && pd0 > ignore).then_some(pd0);
        for (row, adj) in v.rows.iter().zip(v.row_adjust.iter()) {
            if row.len() == 1 && row[0].span == NOALIGN_SPAN {
                if let Some(Node::Box {
                    list,
                    shift: end_pd,
                    ..
                }) = row[0].packed.as_ref()
                {
                    if !valign {
                        prev = (*end_pd > ignore).then_some(*end_pd);
                        pd = *end_pd;
                    }
                    items.extend(plain_items(list));
                }
                continue;
            }
            let mut cells = vec![Item::Glue(v.t0.param(glue_subtype::TAB_SKIP))];
            let (mut ra, mut rb, mut width) = (0i32, 0i32, v.t0.width as i64);
            for (c, cell) in row.iter().enumerate() {
                let Some(Node::Box { w, h, d, .. }) = cell.packed.as_ref() else {
                    continue;
                };
                let tab = self.align_tab_after(v, c + cell.span as usize);
                if valign {
                    ra = ra.max(*w);
                    width += i64::from(*h);
                } else {
                    ra = ra.max(*h);
                    rb = rb.max(*d);
                    width += i64::from(*w);
                }
                width += i64::from(tab.width);
                if let Some(unset) = self.align_unset_cell(cell) {
                    cells.push(unset);
                }
                cells.push(Item::Glue(tab));
            }
            // fin_row: the `\vadjust pre` material precedes the row and its
            // interline glue, the other adjustments follow the row
            for n in adj {
                if let Node::PreAdjust(pre, _) = n {
                    items.extend(plain_items(pre));
                }
            }
            if !valign {
                if let Some(p) = prev {
                    let gap = i64::from(bs.width) - i64::from(p) - i64::from(ra);
                    items.push(Item::Glue(if gap < i64::from(lsl) {
                        ls.param(glue_subtype::LINE_SKIP)
                    } else {
                        Glue {
                            width: gap as i32,
                            subtype: glue_subtype::BASELINE_SKIP,
                            ..bs.fresh()
                        }
                    }));
                }
                prev = Some(rb);
                pd = rb;
            }
            items.push(Item::Unset(Box::new(Unset {
                h: if valign { width as i32 } else { ra },
                d: if valign { 0 } else { rb },
                w: if valign { ra } else { width as i32 },
                span: 0,
                stretch: None,
                shrink: None,
                list: cells,
            })));
            for n in adj {
                if !matches!(n, Node::PreAdjust(_, _)) {
                    items.extend(plain_items(std::slice::from_ref(n)));
                }
            }
        }
        (items, pd)
    }

    // ---------------------------------------------------------------
    // math (tex.web §1136-§1206)
    // ---------------------------------------------------------------

    /// The `{` groups opened in `math_lists[index]`: (list offset where the
    /// group's list starts, `mode_line`, mode before the group).
    fn brace_groups(&self, index: usize) -> Vec<(usize, i32, Mode)> {
        let marks = &self.math_group_marks;
        let lines = &self.show.brace_lines;
        let skip = marks.len().saturating_sub(lines.len());
        marks
            .iter()
            .enumerate()
            .filter(|(_, (depth, _, _))| *depth == index + 1)
            .map(|(i, (_, start, mode))| {
                let line = i
                    .checked_sub(skip)
                    .and_then(|k| lines.get(k))
                    .copied()
                    .unwrap_or(0);
                (*start, line, *mode)
            })
            .collect()
    }

    fn expand_math_levels<'a>(&'a self, levels: Vec<NestLevel<'a>>) -> Vec<NestLevel<'a>> {
        let lists = &self.math_lists;
        let scans = &self.show.math_scans;
        let scan_at = |j: usize| scans.iter().find(|s| s.index == j);
        let recorded = scans.iter().filter(|s| s.index < lists.len()).count();
        let nest_math = levels.iter().filter(|l| is_math(l.mode)).count();
        let consistent = lists.len() >= recorded && lists.len() - recorded == nest_math;
        // the mode of each math level; a `{` group in progress makes
        // `Engine::mode` read `Math` for the formula that holds it
        let mut true_mode: Vec<Option<Mode>> = Vec::with_capacity(levels.len());
        let mut entry = 0usize;
        for lvl in &levels {
            if is_math(lvl.mode) && consistent && entry < lists.len() {
                let m = self
                    .brace_groups(entry)
                    .first()
                    .map(|g| g.2)
                    .filter(|m| is_math(*m))
                    .unwrap_or(lvl.mode);
                true_mode.push(Some(m));
                entry += 1;
                while entry < lists.len() && scan_at(entry).is_some() {
                    entry += 1;
                }
            } else {
                true_mode.push(None);
            }
        }
        // the display level an `\eqno` belongs to is the innermost display
        let eqno_level = if self.pending_display_formula.is_some() {
            true_mode
                .iter()
                .rposition(|m| *m == Some(Mode::DisplayMath))
        } else {
            None
        };
        let last_math = true_mode.iter().rposition(Option::is_some);
        let mut out = Vec::with_capacity(levels.len() + lists.len());
        let mut next_entry = 0usize;
        for (li, lvl) in levels.into_iter().enumerate() {
            if !is_math(lvl.mode) || !consistent || next_entry >= lists.len() {
                out.push(lvl);
                continue;
            }
            let start = next_entry;
            next_entry += 1;
            while next_entry < lists.len() && scan_at(next_entry).is_some() {
                next_entry += 1;
            }
            let mut chain: Vec<MathLevel<'a>> = Vec::new();
            let mut base_line = lvl.line;
            let mut base_mode = true_mode[li].unwrap_or(lvl.mode);
            if eqno_level == Some(li) {
                if let Some(formula) = self.pending_display_formula.as_ref() {
                    // `\eqno` ends a pending fraction's denominator; tex.web's
                    // display level still holds it as its `incompleat_noad`
                    let level = match formula.as_slice() {
                        [Node::Frac {
                            num,
                            den,
                            thickness,
                            left,
                            right,
                            ..
                        }] => MathLevel {
                            mode: Mode::DisplayMath,
                            line: lvl.line,
                            items: view_list(den),
                            incompleat: Some(Item::Frac(Box::new(FracItem {
                                thickness: *thickness,
                                left: *left,
                                right: *right,
                                num: Field::List(view_list(num)),
                                den: Field::Empty,
                            }))),
                        },
                        _ => MathLevel {
                            mode: Mode::DisplayMath,
                            line: lvl.line,
                            items: view_list(formula),
                            incompleat: None,
                        },
                    };
                    chain.push(level);
                    base_mode = Mode::Math;
                    base_line = self.show.eqno_line;
                }
            }
            for e in start..next_entry {
                let scan = if e == start { None } else { scan_at(e) };
                let (mode0, line0) = match scan {
                    None => (base_mode, base_line),
                    Some(s) => (Mode::Math, s.line),
                };
                let groups = self.brace_groups(e);
                let list: &'a [Node] = &lists[e];
                let mut cuts: Vec<usize> = groups.iter().map(|g| g.0.min(list.len())).collect();
                cuts.insert(0, 0);
                cuts.push(list.len());
                let seg = |k: usize| -> &'a [Node] {
                    let (a, b) = (cuts[k], cuts[k + 1].max(cuts[k]));
                    &list[a..b]
                };
                let mut first = MathLevel {
                    mode: mode0,
                    line: line0,
                    items: view_list(seg(0)),
                    incompleat: None,
                };
                match scan.map(|s| &s.kind) {
                    Some(ScanKind::Denominator(p)) => {
                        if let Some(parent) = chain.pop() {
                            let mut numerator = parent.items;
                            numerator.extend(view_list(&p.num));
                            first.mode = parent.mode;
                            first.line = parent.line;
                            first.incompleat = Some(Item::Frac(Box::new(FracItem {
                                thickness: p.thickness,
                                left: p.left,
                                right: p.right,
                                num: Field::List(numerator),
                                den: Field::Empty,
                            })));
                        }
                    }
                    Some(kind) => {
                        if let Some(parent) = chain.last_mut() {
                            if let ScanKind::Script {
                                limits: Some(req), ..
                            } = kind
                            {
                                if let Some(tail) = parent.items.last_mut() {
                                    set_op_subtype(tail, *req);
                                }
                            }
                            if let Some(p) = pending_noad(kind, &parent.items) {
                                parent.items.push(p);
                            }
                        }
                    }
                    None => {}
                }
                chain.push(first);
                for (k, g) in groups.iter().enumerate() {
                    if let Some(parent) = chain.last_mut() {
                        parent.items.push(empty_noad(0));
                    }
                    chain.push(MathLevel {
                        mode: Mode::Math,
                        line: g.1,
                        items: view_list(seg(k + 1)),
                        incompleat: None,
                    });
                }
            }
            if last_math == Some(li) {
                // a `\limits` request not yet applied to the tail operator
                if let (Some(req), Some(tail)) = (
                    self.math_limits,
                    chain.last_mut().and_then(|l| l.items.last_mut()),
                ) {
                    set_op_subtype(tail, req);
                }
            }
            for m in chain {
                out.push(NestLevel {
                    mode: m.mode,
                    line: m.line,
                    list: &[],
                    prefix: Vec::new(),
                    math: Some(m.items),
                    incompleat: m.incompleat,
                    prev_depth: 0,
                    space_factor: 0,
                    prev_graf: 0,
                    frame: None,
                });
            }
        }
        out
    }
}

/// The noad tex.web has appended to the enclosing list before it scans the
/// field that `kind` opened.
fn pending_noad<'a>(kind: &ScanKind, parent: &[Item<'a>]) -> Option<Item<'a>> {
    match kind {
        ScanKind::Brace => Some(empty_noad(0)),
        ScanKind::Script { .. } => {
            if scripts_allowed(parent.last()) {
                None
            } else {
                Some(empty_noad(0))
            }
        }
        ScanKind::Accent(spec) => Some(empty_noad_kind(NoadKind::Accent(*spec))),
        ScanKind::Radical {
            delim,
            subtype,
            width,
            options,
        } => Some(empty_noad_kind(NoadKind::Radical {
            subtype: *subtype,
            delim: *delim,
            width: *width,
            options: *options,
        })),
        // the radical noad is already in the parent; its degree is what
        // this level is scanning
        ScanKind::Degree { .. } => None,
        ScanKind::Class(class) => Some(empty_noad(*class)),
        ScanKind::Over => Some(empty_noad_kind(NoadKind::Over)),
        ScanKind::Under => Some(empty_noad_kind(NoadKind::Under)),
        ScanKind::Choice | ScanKind::Denominator(_) => None,
    }
}
