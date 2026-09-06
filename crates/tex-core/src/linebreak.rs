//! Knuth-Plass paragraph breaking — a port of tex.web's `line_break`
//! (feasible breakpoints, per-(line,fitness) champions, two-pass + emergency
//! pass with artificial-demerits rescue), producing a vbox of line boxes.

use crate::boxes::{Glue, Node, NodeList};
use crate::engine::Engine;
use crate::fonts::FontResolver;
use crate::prim::{DimParam, GlueParam, IntParam};
use crate::scaled::{badness, EJECT_PENALTY, INF_BAD, INF_PENALTY};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// fitness classes with tex.web's numbering (adj-demerits fires when the
/// distance between consecutive classes exceeds 1)
const VERY_LOOSE: usize = 0;
const LOOSE: usize = 1;
const DECENT: usize = 2;
const TIGHT: usize = 3;

/// break type of an active node: tex's `unhyphenated` / `hyphenated`
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BreakType {
    Unhyphenated,
    Hyphenated,
}

struct ActiveNode {
    /// index of the breakpoint node in the augmented list (list.len() = the
    /// virtual end-of-paragraph break)
    pos: usize,
    btype: BreakType,
    line: i32,
    fitness: usize,
    demerits: i64,
    /// badness of the line ENDING at this break (debug/diagnostics)
    badness_dbg: i32,
    /// width state for lines STARTING at this break (tex's break_width)
    start_w: i64,
    start_st: [i64; 4],
    start_sh: [i64; 4],
    prev: Option<Rc<ActiveNode>>,
}

pub struct ParaParams {
    pub hsize: i32,
    pub left_skip: Glue,
    pub right_skip: Glue,
    pub pretolerance: i32,
    pub tolerance: i32,
    pub line_penalty: i32,
    pub hyphen_penalty: i32,
    pub ex_hyphen_penalty: i32,
    pub double_hyphen_demerits: i32,
    pub final_hyphen_demerits: i32,
    pub adj_demerits: i32,
    pub looseness: i32,
    pub emergency_stretch: i32,
    pub par_shape: Vec<(i32, i32)>,
    pub line_skip_limit: i32,
    pub line_skip: Glue,
    pub baseline_skip: Glue,
    pub par_indent: i32,
    pub inter_line_penalty: i32,
    pub club_penalty: i32,
    pub broken_penalty: i32,
}

impl Engine {
    pub fn para_params(&self) -> ParaParams {
        let e = &self.eqtb;
        ParaParams {
            hsize: e.dim_params[DimParam::HSize.idx() as usize],
            left_skip: e.glue_params[GlueParam::LeftSkip.idx() as usize].clone(),
            right_skip: e.glue_params[GlueParam::RightSkip.idx() as usize].clone(),
            pretolerance: e.int_params[IntParam::Pretolerance.idx() as usize],
            tolerance: e.int_params[IntParam::Tolerance.idx() as usize],
            line_penalty: e.int_params[IntParam::LinePenalty.idx() as usize],
            hyphen_penalty: e.int_params[IntParam::HyphenPenalty.idx() as usize],
            ex_hyphen_penalty: e.int_params[IntParam::ExHyphenPenalty.idx() as usize],
            double_hyphen_demerits: e.int_params[IntParam::DoubleHyphenDemerits.idx() as usize],
            final_hyphen_demerits: e.int_params[IntParam::FinalHyphenDemerits.idx() as usize],
            adj_demerits: e.int_params[IntParam::AdjDemerits.idx() as usize],
            looseness: e.int_params[IntParam::Looseness.idx() as usize],
            emergency_stretch: e.dim_params[DimParam::EmergencyStretch.idx() as usize],
            par_shape: self.par_shape.clone(),
            line_skip_limit: e.dim_params[DimParam::LineSkipLimit.idx() as usize],
            line_skip: e.glue_params[GlueParam::LineSkip.idx() as usize].clone(),
            baseline_skip: e.glue_params[GlueParam::BaselineSkip.idx() as usize].clone(),
            par_indent: e.dim_params[DimParam::ParIndent.idx() as usize],
            inter_line_penalty: e.int_params[IntParam::InterLinePenalty.idx() as usize],
            club_penalty: e.int_params[IntParam::ClubPenalty.idx() as usize],
            broken_penalty: e.int_params[IntParam::BrokenPenalty.idx() as usize],
        }
    }

    /// main entry: hlist (paragraph content, already terminated by
    /// \penalty10000 + \parfillskip by end_paragraph) -> vbox of line boxes.
    /// Appends \leftskip at the front (tex-exact: no trailing rightskip node
    /// and no final penalty — the last break is virtual), hyphenates, then
    /// runs the Knuth-Plass passes. `final_widow_penalty` is the penalty
    /// before the paragraph's last line: \widowpenalty normally,
    /// \displaywidowpenalty when a display follows (tex.web line_break's
    /// only argument, §16054).
    pub fn break_paragraph(&mut self, hlist: NodeList, final_widow_penalty: i32) -> Node {
        if crate::debug_flag("SHAPE") {
            eprintln!("SHAPE par_shape={:?} leftskip={:.2}", self.par_shape, self.eqtb.glue_params[GlueParam::LeftSkip.idx() as usize].width as f64/65536.0);
        }
        if crate::debug_flag("PARADUMP") {
            let mut s = String::new();
            for node in &hlist {
                match node {
                    Node::Char { c, .. } => s.push(*c as char),
                    Node::Ligature { c, .. } => s.push_str(&format!("L{:02x}", c)),
                    Node::Glue(g) => {
                        let base = (g.width as f64)/65536.0;
                        if (base - 3.0).abs() < 0.01 { s.push(' '); } else { s.push_str(&format!("G{:.3},{:.3},{:.3}", base, g.stretch as f64/65536.0, g.shrink as f64/65536.0)); }
                    }
                    Node::Kern(k) | Node::ExplicitKern(k) => s.push_str(&format!("k{:.2}", *k as f64/65536.0)),
                    Node::Penalty(p) => s.push_str(&format!("p{}", p)),
                    Node::Box { w, .. } => s.push_str(&format!("B{:.2}", *w as f64/65536.0)),
                    Node::Whatsit(_) => s.push('|'),
                    Node::Disc(_) => s.push('-'),
                    Node::Rule { .. } => s.push('R'),
                    _ => s.push('?'),
                }
            }
            eprintln!("PARADUMP: {}", s);
        }
        let params = self.para_params();
        if crate::debug_flag("BSTRACE") {
            let probe: String = hlist.iter().filter_map(|n| match n {
                Node::Char { c, .. } => Some(*c as char),
                Node::Ligature { c, .. } => Some(*c as char),
                _ => None,
            }).collect();
            if probe.len() > 20 {
                eprintln!(
                    "BSTRACE baselineskip={:.2}pt font={} lineskip={:.2}",
                    params.baseline_skip.width as f64 / 65536.0,
                    self.eqtb.cur_font_val,
                    params.line_skip.width as f64 / 65536.0
                );
            }
        }
        let params = self.para_params();
        let mut list: NodeList = Vec::with_capacity(hlist.len() + 1);
        list.push(Node::Glue(params.left_skip.clone()));
        list.extend(hlist);

        // widths contributed by \leftskip+\rightskip to every line (tex's
        // "background"); excluded from the measured content width
        let bg_w = (params.left_skip.width + params.right_skip.width) as i64;
        let mut bg_st = [0i64; 4];
        let mut bg_sh = [0i64; 4];
        bg_st[params.left_skip.stretch_order as usize] += params.left_skip.stretch as i64;
        bg_st[params.right_skip.stretch_order as usize] += params.right_skip.stretch as i64;
        bg_sh[params.left_skip.shrink_order as usize] += params.left_skip.shrink as i64;
        bg_sh[params.right_skip.shrink_order as usize] += params.right_skip.shrink as i64;

        let hyphen_set = self.hyphenate_list(&mut list);
        // 0: pretolerance, no pattern-hyphen breaks
        // 1: tolerance, hyphen breaks, final_pass iff no emergency stretch
        // 2: tolerance + emergency stretch, final_pass (cannot fail)
        let mut threshold = if params.pretolerance >= 0 { params.pretolerance } else { params.tolerance };
        let mut second_pass = params.pretolerance < 0 || std::env::var("LB2").is_ok();
        let mut final_pass = params.pretolerance < 0 && params.emergency_stretch <= 0;
        let mut extra_stretch = 0i32;
        let mut best: Option<Rc<ActiveNode>> = None;
        let mut final_ran = false;
        loop {
            if threshold > INF_BAD {
                threshold = INF_BAD;
            }
            match self.try_break(&list, &params, &hyphen_set, threshold, second_pass, final_pass, extra_stretch, bg_w, bg_st, bg_sh) {
                Some(end) => {
                    best = Some(end);
                    break;
                }
                None => {
                    // belt and braces: the artificial-demerits rescue makes
                    // the final pass unfailing; this guard only bounds a
                    // hypothetical bug into the single-line fallback
                    if final_ran {
                        break;
                    }
                    if !second_pass {
                        threshold = params.tolerance;
                        second_pass = true;
                        final_pass = params.emergency_stretch <= 0;
                    } else {
                        extra_stretch = params.emergency_stretch;
                        final_pass = true;
                        final_ran = true;
                    }
                }
            }
        }
        let Some(end) = best else {
            let mut inner: NodeList = Vec::with_capacity(list.len() + 2);
            inner.push(Node::Glue(params.left_skip.clone()));
            inner.extend(list.into_iter().skip(1));
            inner.push(Node::Glue(params.right_skip.clone()));
            let line = crate::boxes::hpack(inner, None, crate::boxes::HBOX, &self.eqtb).node;
            return crate::boxes::vpack(vec![line], None, crate::boxes::VBOX, &self.eqtb).node;
        };
        if std::env::var("LBTRACE").is_ok() {
            let npen = list.iter().filter(|n| matches!(n, Node::Penalty(_))).count();
            let mut chain = Vec::new();
            let mut cur = Some(end.clone());
            while let Some(b) = cur {
                chain.push(b.badness_dbg);
                cur = b.prev.clone();
            }
            chain.reverse();
            eprintln!(
                "LB-DONE pretol={} tol={} emg={} second={} final={} xstretch={} nodes={} pens={} wp={} bs={:?}",
                params.pretolerance, params.tolerance, params.emergency_stretch,
                second_pass, final_pass, extra_stretch, list.len(), npen,
                params.line_penalty, &chain[1..]
            );
        }
        self.build_lines(list, &params, end, final_pass, final_widow_penalty)
    }

    /// insert discretionary hyphens into words; returns the indices of the
    /// inserted disc nodes (pattern-inserted, as opposed to explicit `\-`)
    fn hyphenate_list(&mut self, list: &mut NodeList) -> HashSet<usize> {
        let mut inserted = HashSet::new();
        if self.hyphen_trie.is_empty() {
            return inserted;
        }
        let f = self.eqtb.cur_font_val;
        if self.eqtb.fonts.get(f as usize).is_none() {
            return inserted;
        }
        let hyphen_c = self.eqtb.hyphen_char.get(f as usize).copied().unwrap_or(-1);
        if hyphen_c < 0 || !(0..=255).contains(&hyphen_c) {
            return inserted;
        }
        let hyphen_c = hyphen_c as u8;
        let lh = (self.eqtb.int_params[IntParam::LeftHyphenMin.idx() as usize] as usize).max(1);
        let rh = (self.eqtb.int_params[IntParam::RightHyphenMin.idx() as usize] as usize).max(1);
        let mut word: Vec<u8> = Vec::new();
        // per letter: (node index, component slot) — slot 0 for Char, slot j
        // for the j-th letter inside a ligature node
        let mut word_positions: Vec<(usize, u8)> = Vec::new();
        let mut prev_ok = false; // word preceded by glue/box/rule/penalty/...?
        // (insert position, disc); a disc whose no_break/replace_count cover
        // a ligature splits that ligature at the break point
        let mut edits: Vec<(usize, Node)> = Vec::new();
        let n0 = list.len();
        for i in 0..n0 {
            // tex.web hyphenate (§920ff): implicit font kerns inside a word
            // are transparent — they neither join the letter list nor close
            // the word; explicit kerns (and everything else non-letter) do
            if let Node::Kern(_) = &list[i] {
                continue;
            }
            // letters contributed by this node: a Char is one letter; a
            // ligature expands into its component letters (tex.web §937)
            let mut node_letters: Vec<u8> = Vec::new();
            match &list[i] {
                Node::Char { c, .. } => {
                    let lc = self.eqtb.lc_code.get(*c as usize).copied().unwrap_or(0);
                    if lc != 0 {
                        node_letters.push(lc);
                    }
                }
                Node::Ligature { letters, n_letters, .. } => {
                    let mut ok = *n_letters > 0;
                    let mut lcs = Vec::with_capacity(*n_letters as usize);
                    for j in 0..*n_letters as usize {
                        let lc = self.eqtb.lc_code.get(letters[j] as usize).copied().unwrap_or(0);
                        if lc == 0 {
                            ok = false;
                            break;
                        }
                        lcs.push(lc);
                    }
                    if ok {
                        node_letters = lcs;
                    }
                }
                _ => {}
            }
            if !node_letters.is_empty() {
                if word.is_empty() {
                    word_positions.clear();
                    // tex.web: hyphenation abandoned unless the word is
                    // preceded by glue, box, rule, penalty, ins, mark, whatsit
                    prev_ok = i > 0
                        && matches!(
                            list[i - 1],
                            Node::Glue(_) | Node::Box { .. } | Node::Rule { .. } | Node::Penalty(_) | Node::Ins { .. } | Node::Mark { .. } | Node::Whatsit(_)
                        );
                }
                for (j, lc) in node_letters.iter().enumerate() {
                    word.push(*lc);
                    word_positions.push((i, j as u8));
                }
            } else if !word.is_empty() {
                // tex.web compound-word rule: a word terminated by the
                // font's hyphen char (an explicit `-` in the text) gets NO
                // internal points — "market-to-book" breaks only at its
                // explicit hyphens, never at "mar-ket"
                let closed_by_hyphen = matches!(
                    &list[i],
                    Node::Char { c, .. } if *c == hyphen_c
                ) || matches!(&list[i], Node::Disc(_));
                if !closed_by_hyphen && prev_ok && word.len() >= lh + rh {
                    let points = self.hyphen_trie.hyphenate(&word, lh, rh);
                    let mut disc_at_node: Option<usize> = None;
                    for &k in &points {
                        // point k = break before letter k
                        let (pos, slot) = word_positions[k];
                        if disc_at_node == Some(pos) {
                            continue; // one disc per node
                        }
                        let disc = match &list[pos] {
                            Node::Ligature { letters, n_letters, font, .. } if slot > 0 => {
                                // break inside a ligature: the disc replaces
                                // the ligature node; pre = leading letters +
                                // hyphen, post = trailing letters, no_break =
                                // the intact ligature
                                let font = *font;
                                let j = slot as usize;
                                let mut pre_break: NodeList = letters[..j]
                                    .iter()
                                    .map(|&c| Node::Char { c, font })
                                    .collect();
                                pre_break.push(Node::Char { c: hyphen_c, font });
                                let post_break: NodeList = letters[j..*n_letters as usize]
                                    .iter()
                                    .map(|&c| Node::Char { c, font })
                                    .collect();
                                disc_at_node = Some(pos);
                                Node::Disc(crate::boxes::DiscNode {
                                    pre_break,
                                    post_break,
                                    no_break: vec![list[pos].clone()],
                                    replace_count: 1,
                                })
                            }
                            _ => {
                                disc_at_node = Some(pos);
                                Node::Disc(crate::boxes::DiscNode {
                                    pre_break: vec![Node::Char { c: hyphen_c, font: f }],
                                    post_break: Vec::new(),
                                    no_break: Vec::new(),
                                    replace_count: 0,
                                })
                            }
                        };
                        edits.push((pos, disc));
                    }
                }
                word.clear();
            }
        }
        edits.sort_by(|a, b| a.0.cmp(&b.0));
        for (offset, (pos, node)) in edits.into_iter().enumerate() {
            let actual_pos = pos + offset;
            list.insert(actual_pos, node);
            inserted.insert(actual_pos);
        }
        inserted
    }

    /// one Knuth-Plass pass; returns the final breakpoint chain on success
    #[allow(clippy::too_many_arguments)]
    fn try_break(
        &self,
        list: &[Node],
        params: &ParaParams,
        hyphen_set: &HashSet<usize>,
        threshold: i32,
        hyph_enabled: bool,
        final_pass: bool,
        extra_stretch: i32,
        bg_w: i64,
        bg_st: [i64; 4],
        bg_sh: [i64; 4],
    ) -> Option<Rc<ActiveNode>> {
        let n = list.len();
        // cumulative measurements; discs contribute their no_break text and
        // make the replace_count following nodes dead (zero contribution)
        let mut cum_w = vec![0i64; n + 1];
        let mut cum_st = vec![[0i64; 4]; n + 1];
        let mut cum_sh = vec![[0i64; 4]; n + 1];
        {
            let fonts = crate::boxes::eqtb_fonts(&self.eqtb);
            let mut i = 0usize;
            while i < n {
                let (w, st, sh) = match &list[i] {
                    Node::Char { c, font } => (fonts.char_width(*font, *c) as i64, [0; 4], [0; 4]),
                    Node::Ligature { lig_width, .. } => (*lig_width as i64, [0; 4], [0; 4]),
                    Node::Glue(g) => {
                        let mut st = [0i64; 4];
                        let mut sh = [0i64; 4];
                        st[g.stretch_order as usize] = g.stretch as i64;
                        sh[g.shrink_order as usize] = g.shrink as i64;
                        (g.width as i64, st, sh)
                    }
                    Node::Kern(k) | Node::ExplicitKern(k) => (*k as i64, [0; 4], [0; 4]),
                    Node::Disc(dc) => {
                        // replacements contain no glue (tex.web assumption)
                        (disc_list_width(&self.eqtb, &dc.no_break), [0; 4], [0; 4])
                    }
                    Node::Box { w, .. } => (*w as i64, [0; 4], [0; 4]),
                    Node::Rule { width: w, .. } => (*w as i64, [0; 4], [0; 4]),
                    _ => (0, [0; 4], [0; 4]),
                };
                cum_w[i + 1] = cum_w[i] + w;
                for k in 0..4 {
                    cum_st[i + 1][k] = cum_st[i][k] + st[k];
                    cum_sh[i + 1][k] = cum_sh[i][k] + sh[k];
                }
                // nodes a disc replaces are dead: zero contribution, carry
                // the cumulative sums forward unchanged
                if let Node::Disc(dc) = &list[i] {
                    for _ in 0..dc.replace_count {
                        i += 1;
                        cum_w[i + 1] = cum_w[i];
                        for k in 0..4 {
                            cum_st[i + 1][k] = cum_st[i][k];
                            cum_sh[i + 1][k] = cum_sh[i][k];
                        }
                    }
                }
                i += 1;
            }
        }

        // first index > j whose node survives at the start of the next line
        // (tex's post_line_break prune: glue, penalty, explicit kern, math)
        let after_prune = |j: usize| -> usize {
            let mut f = j + 1;
            while f < n && is_prunable(&list[f]) {
                f += 1;
            }
            f.min(n)
        };

        if crate::debug_flag("KPW") {
            let fonts = crate::boxes::eqtb_fonts(&self.eqtb);
            for (i, node) in list.iter().enumerate() {
                let d = match node {
                    Node::Char { c, font } => format!("{}:ch'{}' w={:.1}", i, *c as u8 as char, fonts.char_width(*font, *c) as f64/65536.0),
                    Node::Glue(g) => format!("{}:G {:.1}+{:.1}-{:.1}", i, g.width as f64/65536.0, g.stretch as f64/65536.0, g.shrink as f64/65536.0),
                    Node::Box { w, list, .. } => format!("{}:B w={:.1} n={}", i, *w as f64/65536.0, list.len()),
                    Node::Kern(k) | Node::ExplicitKern(k) => format!("{}:K{:.1}", i, *k as f64/65536.0),
                    Node::Penalty(p) => format!("{}:P{}", i, p),
                    Node::Disc(_) => format!("{}:DISC", i),
                    Node::Ligature { c, lig_width, .. } => format!("{}:lig'{}' w={:.1}", i, *c as char, *lig_width as f64/65536.0),
                    other => format!("{}:?{:?}", i, std::mem::discriminant(other)),
                };
                eprintln!("KPW {}", d);
            }
        }
        let start = Rc::new(ActiveNode {
            badness_dbg: 0,
            pos: 0,
            btype: BreakType::Unhyphenated,
            line: 0,
            fitness: DECENT,
            demerits: 0,
            start_w: 0,
            start_st: [0; 4],
            start_sh: [0; 4],
            prev: None,
        });
        let kptrace = crate::debug_flag("KPTRACE");
        let mut kptext: String = String::new();
        let mut kpchars: Vec<usize> = Vec::with_capacity(n + 1); // chars before node i
        if kptrace {
            for node in list.iter() {
                kpchars.push(kptext.len());
                match node {
                    Node::Char { c, .. } => kptext.push(*c as char),
                    Node::Ligature { c, .. } => kptext.push(*c as char),
                    Node::Glue(_) => kptext.push(' '),
                    Node::Disc(_) => kptext.push('-'),
                    Node::Kern(_) | Node::ExplicitKern(_) => {}
                    _ => kptext.push('`'),
                }
            }
            kpchars.push(kptext.len());
            let kb = (0..=kptext.len().min(70)).rev().find(|&b| kptext.is_char_boundary(b)).unwrap_or(0);
            eprintln!("KPPAR: {}", &kptext[..kb]);
        }
        let mut actives: Vec<Rc<ActiveNode>> = vec![start];
        let easy_line = if params.looseness != 0 {
            i32::MAX
        } else if !params.par_shape.is_empty() {
            (params.par_shape.len() - 1) as i32
        } else {
            0
        };

        // evaluate one candidate breakpoint; `cand` == n is the virtual
        // end-of-paragraph break (tex: try_break at cur_p = null)
        macro_rules! consider {
            ($cand:expr, $is_disc:expr, $penalty:expr, $btype:expr, $forced:expr, $endw:expr) => {{
                let cand: usize = $cand;
                let forced: bool = $forced;
                let btype: BreakType = $btype;
                let penalty: i32 = $penalty;
                let endw: i64 = $endw;
                // per-(line,fitness) champions for this candidate
                let mut champions: HashMap<(i32, usize), (i64, Rc<ActiveNode>)> = HashMap::new();
                let mut idx = 0usize;
                while idx < actives.len() {
                    let a = actives[idx].clone();
                    let is_only = actives.len() == 1;
                    let width = endw - a.start_w + bg_w;
                    let mut dst = [0i64; 4];
                    let mut dsh = [0i64; 4];
                    for k in 0..4 {
                        dst[k] = cum_st[cand][k] - a.start_st[k] + bg_st[k];
                        dsh[k] = cum_sh[cand][k] - a.start_sh[k] + bg_sh[k];
                    }
                    dst[0] += extra_stretch as i64; // emergency-pass background
                    let target = line_metrics(params, a.line + 1).1 as i64;
                    let shortfall = target - width;
                    let (b, fit) = if shortfall == 0 {
                        (0, DECENT)
                    } else if shortfall > 0 {
                        // stretching
                        if dst[1] > 0 || dst[2] > 0 || dst[3] > 0 {
                            (0, DECENT) // infinite stretch
                        } else {
                            let bb = badness(shortfall as i32, dst[0] as i32);
                            let fit = if bb > 99 { VERY_LOOSE } else if bb > 12 { LOOSE } else { DECENT };
                            (bb, fit)
                        }

                    } else if -shortfall > dsh[0] {
                        // cannot shrink enough: hopeless
                        (INF_BAD + 1, TIGHT)
                    } else {
                        let bb = badness((-shortfall) as i32, dsh[0] as i32);
                        let fit = if bb > 12 { TIGHT } else { DECENT };
                        (bb, fit)
                    };
                    if kptrace {
                        eprintln!("KP-EVAL cand={} from=({},@{}) sf={:.2}pt b={} fit={} pen={}", cand, a.line, a.pos, shortfall as f64 / 65536.0, b, fit, penalty);
                    }
                    if b <= threshold {
                        let d = a.demerits
                            + demerits(params, b, penalty)
                            + fitness_demerits(params, &a, btype, fit, cand == n);
                        let line_class = if a.line > easy_line { easy_line + 1 } else { a.line };
                        let key = (line_class, fit);
                        match champions.get(&key) {
                            Some((best_d, _)) if *best_d < d => {}
                            _ => {
                                champions.insert(key, (d, a.clone()));
                            }
                        }
                    }
                    let hopeless = b > INF_BAD;
                    if hopeless || forced {
                        if final_pass && champions.is_empty() && is_only {
                            champions.insert((a.line + 1, DECENT), (a.demerits, a.clone()));
                        }
                        actives.remove(idx);
                        continue;
                    }
                    idx += 1;
                }
                // materialize champion active nodes
                let mut keys: Vec<_> = champions.keys().copied().collect();
                keys.sort();
                let mut new_nodes: Vec<Rc<ActiveNode>> = Vec::with_capacity(keys.len());
                for key in keys {
                    let (d, prev) = &champions[&key];
                    let (start_w, start_st, start_sh) =
                        start_state(list, &after_prune, &self.eqtb, cand, $is_disc, &cum_w, &cum_st, &cum_sh);
                    new_nodes.push(Rc::new(ActiveNode {
                        pos: cand,
                        btype,
                        line: prev.line + 1,
                        fitness: key.1,
                        demerits: *d,
                        badness_dbg: 0,
                        start_w,
                        start_st,
                        start_sh,
                        prev: Some(prev.clone()),
                    }));
                }
                if kptrace {
                    for node in &new_nodes {
                        let at = kpchars.get(node.pos).copied().unwrap_or(0);
                        let at = (0..=at.min(kptext.len())).rev().find(|&b| kptext.is_char_boundary(b)).unwrap_or(0);
                        let ctx = &kptext[..at];
                        eprintln!(
                            "KP @@c{}: line {}.{} t={} -> @@c{} | ...{}",
                            node.pos,
                            node.line,
                            node.fitness,
                            node.demerits,
                            node.prev.as_ref().map(|p| p.pos).unwrap_or(0),
                            &ctx[(0..=ctx.len()).rev().find(|&b| b <= ctx.len().saturating_sub(28) && ctx.is_char_boundary(b)).unwrap_or(0)..]
                        );
                    }
                }
                if forced {
                    actives = new_nodes;
                } else {
                    actives.extend(new_nodes);
                }
                if actives.is_empty() {
                    return None; // pass failed: active list drained
                }
                if cand == n {
                    return actives.iter().min_by_key(|a| a.demerits).map(|a| a.clone());
                }
            }};
        }

        // tex.web §16964: auto_breaking is false between math-on/math-off
        // (glue inside a formula is never a breakpoint); outside math, glue
        // breaks only when not preceded by glue/penalty/explicit-kern/math
        let mut i = 0usize;
        let mut auto_breaking = true;
        while i < n {
            match &list[i] {
                Node::MathKern(_, kind) => {
                    auto_breaking = *kind != 1;
                    // tex.web §17079: math_node does kern_break — a math
                    // node followed by glue is a legal breakpoint (the glue
                    // is discarded at the break)
                    if i + 1 < n && matches!(list[i + 1], Node::Glue(_)) {
                        consider!(i, false, 0, BreakType::Unhyphenated, false, cum_w[i]);
                    }
                }
                Node::Penalty(p) => {
                    if *p < INF_PENALTY {
                        let forced = *p <= EJECT_PENALTY;
                        consider!(i, false, *p, BreakType::Unhyphenated, forced, cum_w[i]);
                    }
                }
                Node::Glue(_) => {
                    let legal = auto_breaking
                        && i > 0
                        && !matches!(
                            list[i - 1],
                            Node::Glue(_) | Node::Penalty(_) | Node::ExplicitKern(_) | Node::MathKern(..)
                        );
                    if legal {
                        consider!(i, false, 0, BreakType::Unhyphenated, false, cum_w[i]);
                    }
                }
                Node::ExplicitKern(k) => {
                    // tex's kern_break: explicit kern followed by glue; the
                    // kern itself is zeroed at the line end
                    if auto_breaking && i + 1 < n && matches!(list[i + 1], Node::Glue(_)) {
                        consider!(i, false, 0, BreakType::Unhyphenated, false, cum_w[i]);
                        let _ = k;
                    }
                }
                Node::Disc(dc) => {
                    if hyph_enabled || !hyphen_set.contains(&i) {
                        let endw = cum_w[i] + disc_list_width(&self.eqtb, &dc.pre_break);
                        consider!(i, true, params.ex_hyphen_penalty, BreakType::Hyphenated, false, endw);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        // virtual final break at end of paragraph
        consider!(n, false, EJECT_PENALTY, BreakType::Hyphenated, true, cum_w[n]);
        unreachable!("consider! at cand == n always returns")
    }

    /// materialize the line boxes along the chosen breakpoint chain
    fn build_lines(
        &mut self,
        mut list: NodeList,
        params: &ParaParams,
        end: Rc<ActiveNode>,
        final_pass: bool,
        final_widow_penalty: i32,
    ) -> Node {
        let mut chain = Vec::new();
        let mut cur = Some(end);
        while let Some(b) = cur {
            chain.push(b.clone());
            cur = b.prev.clone();
        }
        chain.reverse();
        let hfuzz = self.eqtb.dim_params[DimParam::Hfuzz.idx() as usize] as i64;
        let overfull_rule = self.eqtb.dim_params[DimParam::OverfullRule.idx() as usize];
        let mut lines: NodeList = Vec::new();
        let mut i = 1usize;
        let mut pending_post: Option<crate::boxes::DiscNode> = None;
        let mut dead_until = 0usize; // nodes in [i, dead_until) are dead
        // chain[0] is the synthetic paragraph start (pos 0, line 0)
        let total_lines = chain.len() - 1;
        for (li, bp) in chain.iter().skip(1).enumerate() {
            let j = bp.pos.min(list.len());
            let mut seg: NodeList = Vec::new();
            let mut nat_w = 0i64;
            if let Some(dc) = pending_post.take() {
                for nn in &dc.post_break {
                    push_dims(&self.eqtb, nn, &mut seg, &mut nat_w);
                }
            }
            // gather live nodes strictly before the breakpoint node
            let mut post_adj: NodeList = Vec::new();
            while i < j {
                if i < dead_until {
                    i += 1;
                    continue;
                }
                if let Node::VAdjust(items) = &list[i] {
                    // tex.web post_line_break: adjustment material joins the
                    // vertical list right after the line box containing it
                    post_adj.extend(items.clone());
                    i += 1;
                    continue;
                }
                let skip = match &list[i] {
                    // an unbroken disc renders its no_break text and swallows
                    // the replaced nodes (ligature splits)
                    Node::Disc(dc) => dc.replace_count,
                    _ => 0,
                };
                push_dims(&self.eqtb, &list[i], &mut seg, &mut nat_w);
                i += 1 + skip;
            }
            let last = bp.pos >= list.len();
            let mut break_disc: Option<crate::boxes::DiscNode> = None;
            let mut broke_at_disc = false;
            if !last {
                match &list[j] {
                    Node::Disc(dc) => {
                        broke_at_disc = true;
                        // line ends with the pre-break text
                        for nn in &dc.pre_break {
                            push_dims(&self.eqtb, nn, &mut seg, &mut nat_w);
                        }
                        if dc.post_break.is_empty() {
                            // post_line_break prunes the next line start
                            i = j + 1 + dc.replace_count;
                            while i < list.len() && is_prunable(&list[i]) {
                                i += 1;
                            }
                            dead_until = 0;
                        } else {
                            break_disc = Some(dc.clone());
                            i = j + 1;
                            dead_until = j + 1 + dc.replace_count;
                        }
                    }
                    Node::Glue(_) | Node::Penalty(_) | Node::ExplicitKern(_) => {
                        // break node dropped (glue becomes \rightskip at
                        // packing; explicit-kern break is zeroed by tex);
                        // prune discardables at the start of the next line
                        i = j + 1;
                        while i < list.len() && is_prunable(&list[i]) {
                            i += 1;
                        }
                        dead_until = 0;
                    }
                    _ => {
                        i = j + 1;
                    }
                }
            }
            let (indent, target) = line_metrics(params, bp.line);
            let mut inner: NodeList = Vec::new();
            inner.push(Node::Glue(params.left_skip.clone()));
            inner.extend(seg);
            inner.push(Node::Glue(params.right_skip.clone()));
            let mut r = crate::boxes::hpack(inner, Some(target), crate::boxes::HBOX, &self.eqtb);
            // tex.web §17436: the parshape indent is the line box's
            // shift_amount, never an in-line kern (a kern would overshoot
            // the packed width, which already excludes the indent).
            if indent != 0 {
                if let Node::Box { shift, .. } = &mut r.node {
                    *shift = indent;
                }
            }
            let overfull = nat_w + params.left_skip.width as i64 + params.right_skip.width as i64 - target as i64;
            if final_pass && overfull > hfuzz {
                let msg = format!(
                    "Overfull \\hbox ({:.3}pt too wide) in paragraph at line {} [{}]\n",
                    overfull as f64 / 65536.0,
                    self.input.current_file_line(),
                    self.input.current_file_name()
                );
                self.log.push_str(&msg);
                self.term.push_str(&msg);
                if overfull_rule > 0 {
                    if let Node::Box { list: rl, .. } = &mut r.node {
                        rl.push(Node::Rule { width: overfull_rule, height: 0x10000, depth: 0 });
                    }
                }
            }
            // interline glue placeholder (page builder owns real baseline
            // spacing between line boxes)
            if !lines.is_empty() {
                lines.push(Node::Glue(Glue::zero()));
            }
            lines.push(r.node);
            if !post_adj.is_empty() {
                lines.extend(post_adj);
            }
            // tex.web §17438: interline penalty after every line but the
            // last — interlinepenalty, plus clubpenalty after line 1, plus
            // the (display)widow penalty before the last line, plus
            // brokenpenalty when the line ended at a discretionary.
            if li + 1 != total_lines {
                let mut pen = params.inter_line_penalty;
                if li == 0 {
                    pen += params.club_penalty;
                }
                if li + 2 == total_lines {
                    pen += final_widow_penalty;
                }
                if broke_at_disc {
                    pen += params.broken_penalty;
                }
                if pen != 0 {
                    lines.push(Node::Penalty(pen));
                }
            }
            if let Some(dc) = break_disc {
                pending_post = Some(dc);
            }
        }
        if crate::debug_flag("LINEDUMP") {
            let mut s = String::new();
            for n in &lines {
                match n {
                    Node::Box { list, .. } => {
                        s.push('[');
                        for m in list.iter().take(8) {
                            match m {
                                Node::Char { c, .. } => s.push(*c as char),
                                Node::Ligature { c, .. } => s.push(*c as char),
                                Node::Glue(_) => s.push(' '),
                                _ => s.push('?'),
                            }
                        }
                        s.push(']');
                    }
                    Node::Glue(g) => s.push_str(&format!(" G{:.2}+{:.2}-{:.2}", g.width as f64/65536.0, g.stretch as f64/65536.0, g.shrink as f64/65536.0)),
                    Node::Penalty(p) => s.push_str(&format!(" p{}", p)),
                    Node::Kern(k) | Node::ExplicitKern(k) => s.push_str(&format!(" k{:.2}", *k as f64/65536.0)),
                    _ => s.push_str(" ?"),
                }
            }
            eprintln!("LINEDUMP: {}", s);
        }
        crate::boxes::vpack(lines, None, crate::boxes::VBOX, &self.eqtb).node
    }

    pub fn vsplit_box(&mut self, b: Node, target: i32) -> Option<Node> {
        let Node::Box { list, .. } = b else { return Some(b) };
        // tex.web: \\vsplit to 0pt is used by LaTeX \\@doclearpage to peel
        // marks. A positive topskip glue at the start must not become the
        // split result (that \\unvbox's 10pt onto the next page).
        if target <= 0 {
            self.vsplat_remainder = Some(list);
            let r = crate::boxes::vpack(Vec::new(), None, crate::boxes::VBOX, &self.eqtb);
            return Some(r.node);
        }
        let (mut h, mut d) = (0i64, 0i64);
        let mut split_at = list.len();
        for (i, n) in list.iter().enumerate() {
            match n {
                Node::Box { h: bh, d: bd, shift, .. } => {
                    let (bh, bd) = (*bh as i64, *bd as i64);
                    if h > 0 && h + d + bh > target as i64 && split_at == list.len() {
                        split_at = i;
                        break;
                    }
                    h += d + bh - *shift as i64;
                    d = bd + *shift as i64;
                }
                Node::Glue(g) => {
                    let w = g.width as i64;
                    if h + d + w > target as i64 && split_at == list.len() && h > 0 {
                        split_at = i;
                        break;
                    }
                    d += w;
                }
                Node::Kern(k) => {
                    let w = *k as i64;
                    if h + d + w > target as i64 && split_at == list.len() && h > 0 {
                        split_at = i;
                        break;
                    }
                    d += w;
                }
                _ => {}
            }
        }
        let top: NodeList = list[..split_at].to_vec();
        let mut rest = list[split_at..].to_vec();
        while let Some(Node::Glue(_)) = rest.first() {
            rest.remove(0);
        }
        let _ = d;
        self.eqtb.dimen[0] = 0; // splitbotmark etc simplified
        let r = crate::boxes::vpack(top, None, crate::boxes::VBOX, &self.eqtb);
        self.vsplat_remainder = Some(rest);
        Some(r.node)
    }
}

/// nodes tex removes at the start of the next line after a non-disc break
fn is_prunable(n: &Node) -> bool {
    matches!(n, Node::Glue(_) | Node::Penalty(_) | Node::ExplicitKern(_) | Node::MathKern(..))
}

fn push_dims(eqtb: &crate::eqtb::Eqtb, n: &Node, seg: &mut NodeList, w: &mut i64) {
    if let Node::Disc(dc) = n {
        *w += disc_list_width(eqtb, &dc.no_break);
        seg.push(n.clone());
        return;
    }
    let fonts = crate::boxes::eqtb_fonts(eqtb);
    let wd = match n {
        Node::Char { c, font } => fonts.char_width(*font, *c),
        Node::Ligature { lig_width, .. } => *lig_width,
        Node::Glue(g) => g.width,
        Node::Kern(k) | Node::ExplicitKern(k) => *k,
        Node::Box { w: bw, .. } => *bw,
        Node::Rule { width, .. } => *width,
        _ => 0,
    };
    *w += wd as i64;
    seg.push(n.clone());
}

fn disc_list_width(eqtb: &crate::eqtb::Eqtb, l: &[Node]) -> i64 {
    let fonts = crate::boxes::eqtb_fonts(eqtb);
    l.iter().map(|nn| match nn {
        Node::Char { c, font } => fonts.char_width(*font, *c) as i64,
        Node::Ligature { lig_width, .. } => *lig_width as i64,
        Node::Kern(k) | Node::ExplicitKern(k) => *k as i64,
        Node::Box { w, .. } | Node::Rule { width: w, .. } => *w as i64,
        _ => 0,
    }).sum()
}

/// (left indent, width) for 1-based line number `line`
fn line_metrics(params: &ParaParams, line: i32) -> (i32, i32) {
    if params.par_shape.is_empty() {
        return (0, params.hsize);
    }
    let idx = ((line - 1).max(0) as usize).min(params.par_shape.len() - 1);
    params.par_shape[idx]
}

/// d = (line_penalty + b)^2 + penalty term (tex.web @<Compute the demerits@>)
fn demerits(params: &ParaParams, b: i32, pi: i32) -> i64 {
    // tex.web §1141: b>inf_bad or pi=eject_penalty => inf_demerits;
    // otherwise d = (line_penalty + b)^2 + pi^2 — pi^2 is ADDED for
    // negative penalties too (a discretionary's -50 costs +2500 demerits).
    // (The earlier version subtracted pi^2 for pi<0 and never produced
    // inf_demerits, biasing the optimum toward penalty/hyphen breaks and
    // changing raggedness in \sloppy paragraphs — the ai_patent
    // 108-vs-110 page divergence.)
    const INF_DEMERITS: i64 = (INF_BAD as i64) * (INF_BAD as i64);
    if b > INF_BAD || pi == EJECT_PENALTY {
        return INF_DEMERITS;
    }
    let d = params.line_penalty as i64 + b as i64;
    d * d + pi as i64 * pi as i64
}

/// extra demerits: double-hyphen / final-hyphen, and adjacent fitness
fn fitness_demerits(params: &ParaParams, a: &ActiveNode, btype: BreakType, fit: usize, at_end: bool) -> i64 {
    let mut d = 0i64;
    if btype == BreakType::Hyphenated && a.btype == BreakType::Hyphenated {
        if !at_end {
            d += params.double_hyphen_demerits as i64;
        } else {
            d += params.final_hyphen_demerits as i64;
        }
    }
    if (fit as i32 - a.fitness as i32).abs() > 1 {
        d += params.adj_demerits as i64;
    }
    d
}

/// stored ("startsum") width state for a new active node at breakpoint `cand`
#[allow(clippy::too_many_arguments)]
fn start_state(
    list: &[Node],
    after_prune: &dyn Fn(usize) -> usize,
    eqtb: &crate::eqtb::Eqtb,
    cand: usize,
    is_disc: bool,
    cum_w: &[i64],
    cum_st: &[[i64; 4]],
    cum_sh: &[[i64; 4]],
) -> (i64, [i64; 4], [i64; 4]) {
    let n = list.len();
    if is_disc && cand < n {
        if let Node::Disc(dc) = &list[cand] {
            let post_w = disc_list_width(eqtb, &dc.post_break);
            if dc.post_break.is_empty() {
                // next line also prunes discardables after the break
                // dead zone may extend past the list end
                let mut f = (cand + 1 + dc.replace_count).min(n);
                while f < n && is_prunable(&list[f]) {
                    f += 1;
                }
                (cum_w[f], cum_st[f], cum_sh[f])
            } else {
                // startsum = C[a] + no_break - post_break (dead nodes are 0)
                let sw = cum_w[cand] - post_w + disc_list_width(eqtb, &dc.no_break);
                (sw, cum_st[cand], cum_sh[cand])
            }
        } else {
            (cum_w[cand], cum_st[cand], cum_sh[cand])
        }
    } else {
        let f = after_prune(cand);
        (cum_w[f], cum_st[f], cum_sh[f])
    }
}
