//! Knuth-Plass paragraph breaking — a port of tex.web's `line_break`
//! (feasible breakpoints, per-(line,fitness) champions, two-pass + emergency
//! pass with artificial-demerits rescue), producing a vbox of line boxes.

use crate::boxes::{Glue, Node, NodeList};
use crate::engine::Engine;
use crate::fonts::FontResolver;
use crate::prim::{DimParam, GlueParam, IntParam};
use crate::scaled::{badness, EJECT_PENALTY, INF_BAD, INF_PENALTY};
use crate::tfm::FontId;
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
    start_w: i64,
    start_st: [i64; 4],
    start_sh: [i64; 4],
    start_fst: i64,
    start_fsh: i64,
    left_prot: i32,
    prev: Option<Rc<ActiveNode>>,
}

fn char_protrusion_width(
    eqtb: &crate::eqtb::Eqtb,
    protrude_chars: i32,
    f: FontId,
    c: u8,
    left: bool,
) -> i32 {
    if protrude_chars <= 0 {
        return 0;
    }
    let code = match eqtb.expand.get(f as usize) {
        Some(ex) => {
            if left {
                ex.lp_code(c)
            } else {
                ex.rp_code(c)
            }
        }
        None => 0,
    };
    if code == 0 {
        return 0;
    }
    let base = if protrude_chars > 1 {
        eqtb.fonts
            .get(f as usize)
            .map(|font| font.quad())
            .unwrap_or(0)
    } else {
        let fonts = crate::boxes::eqtb_fonts(eqtb);
        fonts.char_width(f, c)
    };
    if base == 0 {
        return 0;
    }
    crate::tfm::round_xn_over_d(base, code, 1000)
}

fn find_protchar_left(slice: &[Node], eqtb: &crate::eqtb::Eqtb, protrude_chars: i32) -> i32 {
    if protrude_chars < 2 {
        return 0;
    }
    for n in slice {
        match n {
            Node::Char { font, c } | Node::Ligature { font, c, .. } => {
                return char_protrusion_width(eqtb, protrude_chars, *font, *c, true);
            }
            Node::Glue(_)
            | Node::Penalty(_)
            | Node::Kern(_)
            | Node::ExplicitKern(_)
            | Node::Whatsit(_) => {}
            Node::Box {
                w: 0,
                h: 0,
                d: 0,
                list,
                ..
            } if list.is_empty() => {}
            _ => return 0,
        }
    }
    0
}

/// Font-expansion contribution of one discretionary list. The predecessor
/// state is explicit because pre-break and no-break material are measured
/// against the same source-list boundary.
fn list_font_expansion<F>(
    eqtb: &crate::eqtb::Eqtb,
    nodes: &[Node],
    mut prev: Option<(FontId, u8)>,
    trailing: Option<&Node>,
    record_expansion: &mut F,
) -> (i64, i64, Option<(FontId, u8)>)
where
    F: FnMut(FontId),
{
    let mut stretch = 0i64;
    let mut shrink = 0i64;
    for (i, node) in nodes.iter().enumerate() {
        match node {
            Node::Char { c, font } | Node::Ligature { c, font, .. } => {
                record_expansion(*font);
                stretch += crate::boxes::char_stretch(eqtb, *font, *c) as i64;
                shrink += crate::boxes::char_shrink(eqtb, *font, *c) as i64;
                prev = Some((*font, *c));
            }
            Node::Kern(k) => {
                let next = if i + 1 < nodes.len() {
                    nodes.get(i + 1)
                } else {
                    trailing
                };
                if let (
                    Some((font, left)),
                    Some(Node::Char { c: right, .. } | Node::Ligature { c: right, .. }),
                ) = (prev, next)
                {
                    stretch += crate::boxes::kern_stretch(eqtb, font, left, *right, *k) as i64;
                    shrink += crate::boxes::kern_shrink(eqtb, font, left, *right, *k) as i64;
                }
            }
            _ => {}
        }
    }
    (stretch, shrink, prev)
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
    /// \hangindent (tex.web §25092–25148): nonzero switches line
    /// width/indent based on signed \hangafter
    pub hang_indent: i32,
    pub hang_after: i32,
    pub line_skip_limit: i32,
    pub line_skip: Glue,
    pub baseline_skip: Glue,
    pub par_indent: i32,
    pub inter_line_penalty: i32,
    pub club_penalty: i32,
    pub broken_penalty: i32,
    /// Snapshots of \interlinepenalties, \clubpenalties, \widowpenalties,
    /// and \displaywidowpenalties, in that order.
    pub penalty_shapes: [Rc<[i32]>; 4],
    /// tex.web §16014: lines already put into the vertical list for this
    /// paragraph at the enclosing semantic level (zero unless the
    /// paragraph is being continued after a displayed formula, where
    /// resume_after_display adds three per display, §22509). Line
    /// numbering — and with it \parshape/\hangindent lookup and the
    /// easy-line class merge — starts at prev_graf+1 (§17015, §17253).
    pub prev_graf: i32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParagraphLayoutRecord {
    pub lines: usize,
    pub demerits: i64,
    pub pass: u8,
    pub emergency_stretch: i32,
}

#[inline]
fn penalty_shape_at(shape: &[i32], index: usize, fallback: i32) -> i32 {
    if shape.is_empty() {
        fallback
    } else {
        shape[index.saturating_sub(1).min(shape.len() - 1)]
    }
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
            hang_indent: e.dim_params[DimParam::HangIndent.idx() as usize],
            hang_after: e.int_params[IntParam::HangAfter.idx() as usize],
            line_skip_limit: e.dim_params[DimParam::LineSkipLimit.idx() as usize],
            line_skip: e.glue_params[GlueParam::LineSkip.idx() as usize].clone(),
            baseline_skip: e.glue_params[GlueParam::BaselineSkip.idx() as usize].clone(),
            par_indent: e.dim_params[DimParam::ParIndent.idx() as usize],
            inter_line_penalty: e.int_params[IntParam::InterLinePenalty.idx() as usize],
            club_penalty: e.int_params[IntParam::ClubPenalty.idx() as usize],
            broken_penalty: e.int_params[IntParam::BrokenPenalty.idx() as usize],
            penalty_shapes: self.penalty_shapes.clone(),
            prev_graf: self.prev_graf().max(0),
        }
    }

    /// main entry: hlist (paragraph content, already terminated by
    /// \penalty10000 + \parfillskip by end_paragraph) -> vbox of line boxes.
    /// Appends \leftskip at the front (tex-exact: no trailing rightskip node
    /// and no final penalty — the last break is virtual), hyphenates, then
    /// runs the Knuth-Plass passes. `final_widow_penalty` is the scalar
    /// fallback before the paragraph's last line; `display_widow` selects
    /// the matching plural penalty array.
    pub fn break_paragraph(
        &mut self,
        hlist: NodeList,
        final_widow_penalty: i32,
        display_widow: bool,
    ) -> Node {
        let (node, record) =
            self.break_paragraph_with_record(hlist, final_widow_penalty, display_widow);
        if self.eqtb.int_params[crate::prim::IntParam::TracingParagraphs.idx() as usize] > 0 {
            let pass_name = match record.pass {
                0 => "@firstpass",
                1 => "@secondpass",
                _ => "@emergencypass",
            };
            self.append_log(&format!(
                "\n{} lines={} demerits={}\n",
                pass_name, record.lines, record.demerits
            ));
        }
        self.last_paragraph_layout = Some(record);
        node
    }

    pub fn break_paragraph_with_record(
        &mut self,
        hlist: NodeList,
        final_widow_penalty: i32,
        display_widow: bool,
    ) -> (Node, ParagraphLayoutRecord) {
        let params = self.para_params();
        let mut list = hlist;

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
        let mut threshold = if params.pretolerance >= 0 {
            params.pretolerance
        } else {
            params.tolerance
        };
        let mut second_pass = params.pretolerance < 0;
        let mut final_pass = params.pretolerance < 0 && params.emergency_stretch <= 0;
        let mut extra_stretch = 0i32;
        let mut best: Option<Rc<ActiveNode>> = None;
        let mut final_ran = false;
        loop {
            if threshold > INF_BAD {
                threshold = INF_BAD;
            }
            match self.try_break(
                &list,
                &params,
                &hyphen_set,
                threshold,
                second_pass,
                final_pass,
                extra_stretch,
                bg_w,
                bg_st,
                bg_sh,
            ) {
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
                        second_pass = true;
                        threshold = params.tolerance;
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
            let mut post_adj: NodeList = Vec::new();
            for n in list.into_iter() {
                if let Node::VAdjust(items) = n {
                    post_adj.extend(items);
                } else {
                    inner.push(n);
                }
            }
            inner.push(Node::Glue(params.right_skip.clone()));
            let line = crate::boxes::hpack(inner, None, crate::boxes::HBOX, &self.eqtb).node;
            let mut vlines = vec![line];
            vlines.extend(post_adj);
            let node = crate::boxes::vpack(vlines, None, crate::boxes::VBOX, &self.eqtb).node;
            let record = ParagraphLayoutRecord {
                lines: 1,
                demerits: 0,
                pass: 2,
                emergency_stretch: extra_stretch,
            };
            self.last_paragraph_layout = Some(record.clone());
            return (node, record);
        };
        let mut cur = Some(end.clone());
        let mut total_lines: usize = 0;
        while let Some(b) = cur {
            cur = b.prev.clone();
            total_lines += 1;
        }
        let total_lines = total_lines.saturating_sub(1);
        let pass_num = if !second_pass {
            0
        } else if !final_ran {
            1
        } else {
            2
        };
        let record = ParagraphLayoutRecord {
            lines: total_lines,
            demerits: end.demerits,
            pass: pass_num,
            emergency_stretch: extra_stretch,
        };
        let node = self.build_lines(
            list,
            &params,
            end,
            final_pass,
            final_widow_penalty,
            display_widow,
        );
        self.last_paragraph_layout = Some(record.clone());
        (node, record)
    }

    /// insert discretionary hyphens into words; returns the indices of the
    /// inserted disc nodes (pattern-inserted, as opposed to explicit `\-`)
    fn hyphenate_list(&mut self, list: &mut NodeList) -> HashSet<usize> {
        let mut inserted = HashSet::new();
        if self.hyphen_trie.is_empty() {
            return inserted;
        }
        // tex.web §18261/§18149: cur_lang := language (<=0 or >255 maps to 0).
        // If cur_lang has no patterns (such as language 2, \l@nohyphenation),
        // TeX returns immediately without hyphenating.
        let lang = self.eqtb.int_params[IntParam::Language.idx() as usize];
        let cur_lang = if lang <= 0 || lang > 255 { 0 } else { lang };
        if cur_lang != 0 {
            return inserted;
        }
        // Formats and embedders can construct an Eqtb without going through
        // tex.web §21112 norm_min: \lefthyphenmin and \righthyphenmin are clamped to 1..=63.
        let lh = self.eqtb.int_params[IntParam::LeftHyphenMin.idx() as usize].clamp(1, 63) as usize;
        let rh = self.eqtb.int_params[IntParam::RightHyphenMin.idx() as usize].clamp(1, 63) as usize;
        let minimum_letters = lh.saturating_add(rh);
        // TeX considers at most 63 letters while hyphenating. A larger
        // minimum sum therefore disables automatic hyphenation.
        if minimum_letters > 63 {
            return inserted;
        }
        let mut word: Vec<u8> = Vec::new();
        // per letter: (node index, component slot) — slot 0 for Char, slot j
        // for the j-th letter inside a ligature node
        let mut word_positions: Vec<(usize, u8)> = Vec::new();
        // tex.web §26160-26224: `hf`, the font of the word's first letter,
        // owns the hyphen character — NOT the font current at paragraph end
        // (a paragraph ending in \texttt/math still hyphenates roman words)
        let mut word_font: u16 = 0;
        let mut prev_ok = false;
        let mut can_start_word = false;
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
            let mut node_font: u16 = 0;
            match &list[i] {
                Node::Char { c, font } => {
                    let lc = self.eqtb.lc_code.get(*c as usize).copied().unwrap_or(0);
                    if lc != 0 {
                        node_letters.push(lc);
                        node_font = *font;
                    }
                }
                Node::Ligature {
                    letters,
                    n_letters,
                    font,
                    ..
                } => {
                    let mut ok = *n_letters > 0;
                    let mut lcs = Vec::with_capacity(*n_letters as usize);
                    for j in 0..*n_letters as usize {
                        let lc = self
                            .eqtb
                            .lc_code
                            .get(letters[j] as usize)
                            .copied()
                            .unwrap_or(0);
                        if lc == 0 {
                            ok = false;
                            break;
                        }
                        lcs.push(lc);
                    }
                    if ok {
                        node_letters = lcs;
                        node_font = *font;
                    }
                }
                _ => {}
            }
            if !node_letters.is_empty() {
                if word.is_empty() {
                    word_font = node_font;
                    word_positions.clear();
                    // tex.web §894: only glue starts the lookahead for a
                    // hyphenatable word; an initial indent box does not.
                    prev_ok = can_start_word;
                    can_start_word = false;
                } else if node_font != word_font {
                    // tex.web §26117-26118: a character whose font differs
                    // from hf is treated as a nonletter — close the word
                    // (hyphenating it under word_font) and start a fresh
                    // word at this node with the new font
                    self.flush_hyphen_word(
                        list,
                        i,
                        &word,
                        &word_positions,
                        prev_ok,
                        lh,
                        rh,
                        word_font,
                        &mut edits,
                    );
                    word.clear();
                    word_positions.clear();
                    word_font = node_font;
                    prev_ok = false; // no glue before this node
                }
                for (j, lc) in node_letters.iter().enumerate() {
                    word.push(*lc);
                    word_positions.push((i, j as u8));
                }
            } else if !word.is_empty() {
                self.flush_hyphen_word(
                    list,
                    i,
                    &word,
                    &word_positions,
                    prev_ok,
                    lh,
                    rh,
                    word_font,
                    &mut edits,
                );
                word.clear();
            }
            if word.is_empty() {
                match &list[i] {
                    Node::Glue(_) => can_start_word = true,
                    Node::Char { .. } | Node::Ligature { .. } | Node::Whatsit(_) => {}
                    _ => can_start_word = false,
                }
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

    /// hyphenate one completed word (tex.web `hyphenate`): the hyphen
    /// character comes from the word's own font `wf` (§26222-26224); a font
    /// without a usable hyphenchar simply does not hyphenate.
    fn flush_hyphen_word(
        &self,
        list: &[Node],
        end: usize,
        word: &[u8],
        word_positions: &[(usize, u8)],
        prev_ok: bool,
        lh: usize,
        rh: usize,
        wf: u16,
        edits: &mut Vec<(usize, Node)>,
    ) {
        let hyphen_c = self
            .eqtb
            .hyphen_char
            .get(wf as usize)
            .copied()
            .unwrap_or(-1);
        if hyphen_c < 0 || !(0..=255).contains(&hyphen_c) {
            return; // tex done1: goto without hyphenating
        }
        let hyphen_c = hyphen_c as u8;
        // tex.web compound-word rule: a word terminated by the font's
        // hyphen char (an explicit `-` in the text) gets NO internal
        // points — "market-to-book" breaks only at its explicit hyphens
        let closed_by_hyphen = matches!(
            &list[end],
            Node::Char { c, .. } if *c == hyphen_c
        ) || matches!(&list[end], Node::Disc(_));
        if closed_by_hyphen || !prev_ok || word.len() < lh.saturating_add(rh) {
            return;
        }
        let points = self.hyphen_trie.hyphenate(word, lh, rh);
        let mut disc_at_node: Option<usize> = None;
        for &k in &points {
            if k == 0 || k >= word_positions.len() {
                continue;
            }
            // point k = break before letter k
            let (mut pos, slot) = word_positions[k];
            if disc_at_node == Some(pos) {
                continue; // one disc per node
            }
            let disc = match &list[pos] {
                Node::Ligature {
                    letters,
                    n_letters,
                    font,
                    ..
                } if slot > 0 => {
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
                    // A break replaces the kern to the next letter with
                    // the kern to the hyphen (tex.web reconstitute).
                    let (left_pos, _) = word_positions[k - 1];
                    let (left, font) = match &list[left_pos] {
                        Node::Char { c, font } | Node::Ligature { c, font, .. } => (*c, *font),
                        _ => unreachable!("hyphenation position is a letter"),
                    };
                    let kern = crate::boxes::get_kern(&self.eqtb, font, left, hyphen_c);
                    let mut pre_break = Vec::with_capacity(if kern == 0 { 1 } else { 2 });
                    if kern != 0 {
                        pre_break.push(Node::Kern(kern));
                    }
                    pre_break.push(Node::Char { c: hyphen_c, font });
                    let mut no_break = Vec::new();
                    if pos > 0 && matches!(list[pos - 1], Node::Kern(_)) {
                        pos -= 1;
                        no_break.push(list[pos].clone());
                    }
                    disc_at_node = Some(word_positions[k].0);
                    Node::Disc(crate::boxes::DiscNode {
                        pre_break,
                        post_break: Vec::new(),
                        replace_count: no_break.len(),
                        no_break,
                    })
                }
            };
            edits.push((pos, disc));
        }
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
        let mut cum_w = vec![0i64; n + 1];
        let mut cum_st = vec![[0i64; 4]; n + 1];
        let mut cum_sh = vec![[0i64; 4]; n + 1];
        let mut cum_fst = vec![0i64; n + 1];
        let mut cum_fsh = vec![0i64; n + 1];
        let mut disc_pre_fst = vec![0i64; n];
        let mut disc_pre_fsh = vec![0i64; n];
        let mut stretch_steps = 0i64;
        let mut shrink_steps = 0i64;
        {
            let fonts = crate::boxes::eqtb_fonts(&self.eqtb);
            let pdf_adjust =
                self.eqtb.int_params[crate::prim::IntParam::PdfAdjustSpacing.idx() as usize];
            let mut record_expansion = |font: u16| {
                if pdf_adjust >= 2 && (stretch_steps == 0 || shrink_steps == 0) {
                    let ex = &self.eqtb.expand[font as usize];
                    if ex.step > 0 {
                        if stretch_steps == 0 && ex.stretch != 0 {
                            stretch_steps =
                                (self.eqtb.expand[ex.stretch as usize].ratio / ex.step) as i64;
                        }
                        if shrink_steps == 0 && ex.shrink != 0 {
                            shrink_steps =
                                (-self.eqtb.expand[ex.shrink as usize].ratio / ex.step) as i64;
                        }
                    }
                }
            };
            let mut i = 0usize;
            let mut prev_exp_char: Option<(FontId, u8)> = None;
            while i < n {
                let (w, st, sh, fst, fsh) = match &list[i] {
                    Node::Char { c, font } => {
                        record_expansion(*font);
                        prev_exp_char = Some((*font, *c));
                        let fst = if pdf_adjust >= 2 {
                            crate::boxes::char_stretch(&self.eqtb, *font, *c) as i64
                        } else {
                            0
                        };
                        let fsh = if pdf_adjust >= 2 {
                            crate::boxes::char_shrink(&self.eqtb, *font, *c) as i64
                        } else {
                            0
                        };
                        (fonts.char_width(*font, *c) as i64, [0; 4], [0; 4], fst, fsh)
                    }
                    Node::Ligature {
                        c, font, lig_width, ..
                    } => {
                        record_expansion(*font);
                        prev_exp_char = Some((*font, *c));
                        let fst = if pdf_adjust >= 2 {
                            crate::boxes::char_stretch(&self.eqtb, *font, *c) as i64
                        } else {
                            0
                        };
                        let fsh = if pdf_adjust >= 2 {
                            crate::boxes::char_shrink(&self.eqtb, *font, *c) as i64
                        } else {
                            0
                        };
                        (*lig_width as i64, [0; 4], [0; 4], fst, fsh)
                    }
                    Node::Glue(g) => {
                        let mut st = [0i64; 4];
                        let mut sh = [0i64; 4];
                        st[g.stretch_order as usize] = g.stretch as i64;
                        sh[g.shrink_order as usize] = g.shrink as i64;
                        (g.width as i64, st, sh, 0, 0)
                    }
                    Node::Kern(k) => {
                        let next = match list.get(i + 1) {
                            Some(Node::Char { c, font } | Node::Ligature { c, font, .. }) => {
                                Some((*font, *c))
                            }
                            _ => None,
                        };
                        let (fst, fsh) = if pdf_adjust >= 2 {
                            match (prev_exp_char, next) {
                                (Some((font, left)), Some((_, right))) => (
                                    crate::boxes::kern_stretch(&self.eqtb, font, left, right, *k)
                                        as i64,
                                    crate::boxes::kern_shrink(&self.eqtb, font, left, right, *k)
                                        as i64,
                                ),
                                _ => (0, 0),
                            }
                        } else {
                            (0, 0)
                        };
                        (*k as i64, [0; 4], [0; 4], fst, fsh)
                    }
                    Node::ExplicitKern(k) => (*k as i64, [0; 4], [0; 4], 0, 0),
                    Node::Disc(dc) => {
                        let mut fst = 0i64;
                        let mut fsh = 0i64;
                        if pdf_adjust >= 2 {
                            let (pre_fst, pre_fsh, _) = list_font_expansion(
                                &self.eqtb,
                                &dc.pre_break,
                                prev_exp_char,
                                None,
                                &mut record_expansion,
                            );
                            disc_pre_fst[i] = pre_fst;
                            disc_pre_fsh[i] = pre_fsh;
                            let trailing = list.get(i + 1 + dc.replace_count);
                            let (no_fst, no_fsh, after_no) = list_font_expansion(
                                &self.eqtb,
                                &dc.no_break,
                                prev_exp_char,
                                trailing,
                                &mut record_expansion,
                            );
                            fst = no_fst;
                            fsh = no_fsh;
                            prev_exp_char = after_no;
                        }
                        (
                            disc_list_width(&self.eqtb, &dc.no_break),
                            [0; 4],
                            [0; 4],
                            fst,
                            fsh,
                        )
                    }
                    Node::Box { w, .. } => (*w as i64, [0; 4], [0; 4], 0, 0),
                    Node::Rule { width: w, .. } => (*w as i64, [0; 4], [0; 4], 0, 0),
                    _ => (0, [0; 4], [0; 4], 0, 0),
                };
                cum_w[i + 1] = cum_w[i] + w;
                for k in 0..4 {
                    cum_st[i + 1][k] = cum_st[i][k] + st[k];
                    cum_sh[i + 1][k] = cum_sh[i][k] + sh[k];
                }
                cum_fst[i + 1] = cum_fst[i] + fst;
                cum_fsh[i + 1] = cum_fsh[i] + fsh;
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
                        cum_fst[i + 1] = cum_fst[i];
                        cum_fsh[i + 1] = cum_fsh[i];
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

        let protrude_chars =
            self.eqtb.int_params[crate::prim::IntParam::PdfProtrudeChars.idx() as usize];
        let start_left_prot = find_protchar_left(list, &self.eqtb, protrude_chars);
        let start = Rc::new(ActiveNode {
            pos: 0,
            btype: BreakType::Unhyphenated,
            // tex.web §17015: line_number(initial active) = prev_graf+1;
            // the engine's 0-based `line` (completed lines) starts at
            // prev_graf so a fragment resumed after a display keeps the
            // paragraph's absolute \parshape/\hangindent line numbering
            line: params.prev_graf,
            fitness: DECENT,
            demerits: 0,
            start_w: 0,
            start_st: [0; 4],
            start_sh: [0; 4],
            start_fst: 0,
            start_fsh: 0,
            left_prot: start_left_prot,
            prev: None,
        });
        let mut actives: Vec<Rc<ActiveNode>> = vec![start];
        // tex.web §25121–25133: easy_line is last_special_line (looseness
        // disables the line-class merge), which for parshape is the number
        // of specified lines minus one and for hanging indentation is
        // abs(hang_after); with neither special shape it is 0.
        let (last_special_line, ..) = line_shape(params);
        let easy_line = if params.looseness != 0 {
            i32::MAX
        } else {
            last_special_line
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
                    let pre_fst = if $is_disc { disc_pre_fst[cand] } else { 0 };
                    let pre_fsh = if $is_disc { disc_pre_fsh[cand] } else { 0 };
                    let font_st = (cum_fst[cand] - a.start_fst + pre_fst).max(0);
                    let font_sh = (cum_fsh[cand] - a.start_fsh + pre_fsh).max(0);
                    dst[0] += extra_stretch as i64; // emergency-pass background
                    let target = line_metrics(params, a.line + 1).1 as i64;
                    let right_prot = if protrude_chars > 0 && cand < n {
                        if $is_disc {
                            if let Some(Node::Disc(dc)) = list.get(cand) {
                                dc.pre_break
                                    .iter()
                                    .rev()
                                    .chain(list[..cand].iter().rev())
                                    .find_map(|n| match n {
                                        Node::Char { font, c } | Node::Ligature { font, c, .. } => {
                                            Some(char_protrusion_width(
                                                &self.eqtb,
                                                protrude_chars,
                                                *font,
                                                *c,
                                                false,
                                            ))
                                        }
                                        _ => None,
                                    })
                                    .unwrap_or(0)
                            } else {
                                0
                            }
                        } else if cand > 0 {
                            list[..cand]
                                .iter()
                                .rev()
                                .find_map(|n| match n {
                                    Node::Char { font, c } | Node::Ligature { font, c, .. } => {
                                        Some(char_protrusion_width(
                                            &self.eqtb,
                                            protrude_chars,
                                            *font,
                                            *c,
                                            false,
                                        ))
                                    }
                                    _ => None,
                                })
                                .unwrap_or(0)
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    let mut shortfall = target - width + (a.left_prot + right_prot) as i64;
                    // pdftex.web: retain half an expansion step when the
                    // available font adjustment exceeds the shortfall.
                    if shortfall > 0 && font_st > 0 {
                        shortfall = if font_st > shortfall {
                            (font_st / stretch_steps) / 2
                        } else {
                            shortfall - font_st
                        };
                    } else if shortfall < 0 && font_sh > 0 {
                        shortfall = if font_sh > -shortfall {
                            -(font_sh / shrink_steps) / 2
                        } else {
                            shortfall + font_sh
                        };
                    }
                    let (b, fit) = if shortfall == 0 {
                        (0, DECENT)
                    } else if shortfall > 0 {
                        // stretching
                        if dst[1] > 0 || dst[2] > 0 || dst[3] > 0 {
                            (0, DECENT) // infinite stretch
                        } else {
                            let bb = badness(shortfall as i32, dst[0] as i32);
                            let fit = if bb > 99 {
                                VERY_LOOSE
                            } else if bb > 12 {
                                LOOSE
                            } else {
                                DECENT
                            };
                            (bb, fit)
                        }
                    } else {
                        // shortfall < 0 (shrinking)
                        let needed = -shortfall;
                        if needed > dsh[0] {
                            // cannot shrink enough: hopeless
                            (INF_BAD + 1, TIGHT)
                        } else {
                            let bb = badness(needed as i32, dsh[0] as i32);
                            let fit = if bb > 12 { TIGHT } else { DECENT };
                            (bb, fit)
                        }
                    };

                    if b <= threshold {
                        let d = a.demerits
                            + demerits(params, b, penalty)
                            + fitness_demerits(params, &a, btype, fit, cand == n);
                        // line_number(r) >= easy_line (the l == easy_line
                        // class is never flushed separately, so it joins the
                        // merged class). Rust's `line` is 0-based
                        // (line_number = line + 1), hence a.line + 1 >=
                        // easy_line. With easy_line = 0 (no parshape/hang/
                        // looseness) EVERY predecessor shares one class per
                        // fitness, so equal-demerit chains of different line
                        // counts compete and the last-scanned (longest,
                        // newest-inserted) wins under §25307's <= replace.
                        let line_class = if a.line + 1 >= easy_line {
                            easy_line + 1
                        } else {
                            a.line
                        };
                        let key = (line_class, fit);
                        match champions.get(&key) {
                            // tex.web §25307: replace when d <=
                            // minimal_demerits. Rust scans this class in
                            // active-list order, so an equal candidate must
                            // replace the current champion.
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
                // materialize champion active nodes.
                // tex.web §24805–§24833: each line-number class is flushed
                // separately, and a fitness-class champion is inserted only
                // if its total demerits are <= minimum_demerits +
                // |adj_demerits| (clamped to awful_bad-1), where
                // minimum_demerits is the best total within THAT class
                // flush. Champions that could never win the final scan
                // because an adjacent-fitness jump would cost too much are
                // dropped here.
                const AWFUL_BAD: i64 = (1 << 30) - 1;
                let adj = params.adj_demerits as i64;
                let mut group_min: HashMap<i32, i64> = HashMap::new();
                for (key, (d, _)) in &champions {
                    let e = group_min.entry(key.0).or_insert(AWFUL_BAD);
                    if *d < *e {
                        *e = *d;
                    }
                }
                let mut keys: Vec<_> = champions
                    .iter()
                    .filter(|((cls, _), (d, _))| {
                        let m = group_min[cls];
                        let cutoff = if adj.abs() >= AWFUL_BAD - m {
                            AWFUL_BAD - 1
                        } else {
                            m + adj.abs()
                        };
                        *d <= cutoff
                    })
                    .map(|(k, _)| *k)
                    .collect();
                keys.sort();
                let mut new_nodes: Vec<((i32, usize), Rc<ActiveNode>)> =
                    Vec::with_capacity(keys.len());
                for key in keys {
                    let (d, prev) = &champions[&key];
                    let (start_w, start_st, start_sh, start_fst, start_fsh) = start_state(
                        list,
                        &after_prune,
                        &self.eqtb,
                        cand,
                        $is_disc,
                        &cum_w,
                        &cum_st,
                        &cum_sh,
                        &cum_fst,
                        &cum_fsh,
                    );
                    let left_prot = if protrude_chars >= 2 {
                        let start_idx = after_prune(cand);
                        list.get(start_idx..)
                            .map(|slice| find_protchar_left(slice, &self.eqtb, protrude_chars))
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    new_nodes.push((
                        key,
                        Rc::new(ActiveNode {
                            pos: cand,
                            btype,
                            line: prev.line + 1,
                            fitness: key.1,
                            demerits: *d,
                            start_w,
                            start_st,
                            start_sh,
                            start_fst,
                            start_fsh,
                            left_prot,
                            prev: Some(prev.clone()),
                        }),
                    ));
                }
                if forced {
                    actives = new_nodes.into_iter().map(|(_, n)| n).collect();
                } else {
                    // tex.web §24805–§24815/§25067: the merged class
                    // (line_number >= easy_line) flushes once at
                    // last_active and appends in creation order; a
                    // non-merged class flushes at the boundary into the
                    // next class and its nodes are inserted at the head
                    // of that class block (newest-first within a class).
                    // With the default easy_line = 0 every node is merged,
                    // so try_break scans oldest-first, §25307's <= replace
                    // keeps the newest equal champion, and the §25729
                    // strict-< final scan keeps the first (oldest) minimum.
                    let merged_key = easy_line.saturating_add(1);
                    let mut insert_at = 0usize;
                    for (key, node) in new_nodes {
                        if key.0 == merged_key {
                            actives.push(node);
                        } else {
                            while insert_at < actives.len() && actives[insert_at].line < node.line {
                                insert_at += 1;
                            }
                            actives.insert(insert_at, node);
                            insert_at += 1;
                        }
                    }
                }
                if actives.is_empty() {

                    return None; // pass failed: active list drained
                }
                if cand == n {

                    let mut opt: Option<&Rc<ActiveNode>> = None;
                    for a in &actives {
                        match opt {
                            None => opt = Some(a),
                            // tex.web §25729–25734: the scan follows the active
                            // list (ascending line_number, newest-created first
                            // within a class) and replaces only on strictly fewer
                            // demerits, so the first minimum wins.
                            Some(b) if a.demerits < b.demerits => opt = Some(a),
                            _ => {}
                        }
                    }
                    let opt = match opt {
                        Some(o) => o,
                        None => return None,
                    };
                    if params.looseness == 0 {
                        return Some(opt.clone());
                    }
                    // tex.web §25737–§25756: re-scan the active list for a
                    // node whose line_diff = line_number(r) - best_line lies
                    // between 0 and the requested looseness (so for
                    // looseness = -1 only a one-line-shorter chain wins);
                    // ties at the same line_diff go to the first
                    // strictly-fewest-demerits node in list order.
                    let best_line = opt.line;
                    let mut best_bet = opt;
                    let mut actual_looseness = 0i32;
                    let mut fewest = best_bet.demerits;
                    for a in &actives {
                        let line_diff = a.line - best_line;
                        if (line_diff < actual_looseness && params.looseness <= line_diff)
                            || (line_diff > actual_looseness && params.looseness >= line_diff)
                        {
                            best_bet = a;
                            actual_looseness = line_diff;
                            fewest = a.demerits;
                        } else if line_diff == actual_looseness && a.demerits < fewest {
                            best_bet = a;
                            fewest = a.demerits;
                        }
                    }
                    // tex.web §25724: the pass succeeds only when the
                    // requested looseness was achieved, or on the final
                    // pass (best-effort). Otherwise the pass fails and
                    // line_break moves on (hyphenating pass, emergency
                    // pass) — savetrees' global \looseness=-1 relies on
                    // this to reach the hyphenated 4-line footnote.
                    if actual_looseness == params.looseness || final_pass {
                        return Some(best_bet.clone());
                    }
                    return None;
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
                            Node::Glue(_)
                                | Node::Penalty(_)
                                | Node::ExplicitKern(_)
                                | Node::MathKern(..)
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
                        let pen = if !dc.pre_break.is_empty() {
                            params.hyphen_penalty
                        } else {
                            params.ex_hyphen_penalty
                        };
                        let endw = cum_w[i] + disc_list_width(&self.eqtb, &dc.pre_break);
                        consider!(i, true, pen, BreakType::Hyphenated, false, endw);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        // virtual final break at end of paragraph
        consider!(
            n,
            false,
            EJECT_PENALTY,
            BreakType::Hyphenated,
            true,
            cum_w[n]
        );
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
        display_widow: bool,
    ) -> Node {
        let mut chain = Vec::new();
        let mut cur = Some(end);
        while let Some(b) = cur {
            chain.push(b.clone());
            cur = b.prev.clone();
        }
        chain.reverse();

        let hfuzz = self.eqtb.dim_params[DimParam::Hfuzz.idx() as usize] as i64;
        let mut lines: NodeList = Vec::new();
        let mut i = 0usize;
        let mut pending_post: Option<crate::boxes::DiscNode> = None;
        let mut dead_until = 0usize; // nodes in [i, dead_until) are dead
                                     // chain[0] is the synthetic paragraph start (pos 0, line 0)
        let total_lines = chain.len() - 1;
        for (li, bp) in chain.iter().skip(1).enumerate() {
            let j = bp.pos.min(list.len());
            let mut seg: NodeList = Vec::new();
            let mut nat_w = 0i64;
            if let Some(dc) = pending_post.take() {
                for nn in dc.post_break {
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
                if let Node::VAdjust(items) = &mut list[i] {
                    // tex.web §866 post_line_break: adjustment material joins the
                    // vertical list right after the line box containing it
                    post_adj.append(items);
                    i += 1;
                    continue;
                }
                if matches!(
                    &list[i],
                    Node::Ins { .. } | Node::Mark { .. } | Node::Adj(_)
                ) {
                    // tex.web §866 post_line_break: ins, mark, and adjust nodes
                    // migrate from the line's hlist to the vertical list right
                    // after the line box containing them
                    post_adj.push(std::mem::replace(&mut list[i], Node::Kern(0)));
                    i += 1;
                    continue;
                }
                let skip = match &list[i] {
                    // an unbroken disc renders its no_break text and swallows
                    // the replaced nodes (ligature splits)
                    Node::Disc(dc) => dc.replace_count,
                    _ => 0,
                };
                let node = std::mem::replace(&mut list[i], Node::Kern(0));
                push_dims(&self.eqtb, node, &mut seg, &mut nat_w);
                i += 1 + skip;
            }
            let last = bp.pos >= list.len();
            let mut break_disc: Option<crate::boxes::DiscNode> = None;
            let mut broke_at_disc = false;
            if !last {
                match &mut list[j] {
                    Node::Disc(dc) => {
                        broke_at_disc = true;
                        // line ends with the pre-break text
                        let mut dc = std::mem::replace(
                            dc,
                            crate::boxes::DiscNode {
                                pre_break: Vec::new(),
                                post_break: Vec::new(),
                                no_break: Vec::new(),
                                replace_count: 0,
                            },
                        );
                        for nn in std::mem::take(&mut dc.pre_break) {
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
                            i = j + 1;
                            dead_until = j + 1 + dc.replace_count;
                            break_disc = Some(dc);
                        }
                    }
                    Node::Glue(_)
                    | Node::Penalty(_)
                    | Node::ExplicitKern(_)
                    | Node::MathKern(..) => {
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
            let protrude_chars =
                self.eqtb.int_params[crate::prim::IntParam::PdfProtrudeChars.idx() as usize];
            if protrude_chars > 0 {
                let left_cand = seg.iter().find_map(|n| match n {
                    Node::Char { font, c } | Node::Ligature { font, c, .. } => Some((*font, *c)),
                    Node::Glue(_)
                    | Node::Penalty(_)
                    | Node::Kern(_)
                    | Node::ExplicitKern(_)
                    | Node::Whatsit(_) => None,
                    Node::Box {
                        w: 0,
                        h: 0,
                        d: 0,
                        list,
                        ..
                    } if list.is_empty() => None,
                    _ => Some((0, 0)),
                });
                if let Some((f, c)) = left_cand {
                    if c != 0 {
                        let pw = char_protrusion_width(&self.eqtb, protrude_chars, f, c, true);
                        if pw != 0 {
                            seg.insert(
                                0,
                                Node::MarginKern {
                                    side: 0,
                                    width: -pw,
                                    font: f,
                                    c,
                                },
                            );
                        }
                    }
                }
                if let Some((f, c)) = seg.iter().rev().find_map(|n| match n {
                    Node::Char { font, c } | Node::Ligature { font, c, .. } => Some((*font, *c)),
                    _ => None,
                }) {
                    let pw = char_protrusion_width(&self.eqtb, protrude_chars, f, c, false);
                    if pw != 0 {
                        seg.push(Node::MarginKern {
                            side: 1,
                            width: -pw,
                            font: f,
                            c,
                        });
                    }
                }
            }
            let mut inner: NodeList = Vec::new();
            inner.push(Node::Glue(params.left_skip.clone()));
            inner.extend(seg);
            inner.push(Node::Glue(params.right_skip.clone()));
            let mut r = crate::boxes::hpack_expand(self, inner, target, crate::boxes::HBOX);
            // tex.web §17436: the parshape indent is the line box's
            // shift_amount, never an in-line kern (a kern would overshoot
            // the packed width, which already excludes the indent).
            if indent != 0 {
                if let Node::Box { shift, .. } = &mut r.node {
                    *shift = indent;
                }
            }
            let excess = -r.delta - r.shrink[0];
            if final_pass && -r.delta > r.shrink[0] && excess > hfuzz {
                let msg = format!(
                    "Overfull \\hbox ({:.3}pt too wide) in paragraph ending here",
                    excess as f64 / 65536.0
                );
                let source = self
                    .current_token_source_mark()
                    .map(|mark| mark.to_context());
                self.pack_warning_at(&msg, source);
            }
            // interline glue placeholder (page builder owns real baseline
            // spacing between line boxes)
            if !lines.is_empty() {
                lines.push(Node::Glue(Glue::zero()));
            }
            lines.push(r.node);
            if !post_adj.is_empty() {
                lines.push(Node::VAdjust(post_adj));
            }
            // tex.web §17438: interline penalty after every line but the
            // last — interlinepenalty, plus clubpenalty after line 1, plus
            // the (display)widow penalty before the last line, plus
            // brokenpenalty when the line ended at a discretionary.
            if li + 1 != total_lines {
                let line_no = params.prev_graf.max(0) as usize + li + 1;
                let mut pen = penalty_shape_at(
                    &params.penalty_shapes[0],
                    line_no,
                    params.inter_line_penalty,
                );
                if !params.penalty_shapes[1].is_empty() {
                    pen += penalty_shape_at(&params.penalty_shapes[1], li + 1, 0);
                } else if li == 0 {
                    pen += params.club_penalty;
                }
                let remaining = total_lines - li - 1;
                let widow_shape = if display_widow { 3 } else { 2 };
                if !params.penalty_shapes[widow_shape].is_empty() {
                    pen += penalty_shape_at(&params.penalty_shapes[widow_shape], remaining, 0);
                } else if remaining == 1 {
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

        crate::boxes::vpack(lines, None, crate::boxes::VBOX, &self.eqtb).node
    }
    pub fn vsplit_box(&mut self, b: Node, target: i32) -> Option<Node> {
        let Node::Box { mut list, .. } = b else {
            return Some(b);
        };
        let smd = self.eqtb.dim_params[crate::prim::DimParam::SplitMaxDepth.idx() as usize] as i64;
        let target64 = target as i64;
        let mut t = 0i64;
        let mut d = 0i64;
        let mut stretch = [0i64; 4];
        let mut shrink = 0i64;
        let mut best_cost = 1073741823i64;
        let mut best_split = 0;
        let mut prev_non_discardable = false;

        // tex.web vert_break: the end-of-list penalty is evaluated only if
        // scanning reaches it, never after an earlier forced/overfull break.
        for i in 0..=list.len() {
            let penalty = match list.get(i) {
                None => Some(-10000),
                Some(Node::Penalty(p)) => Some(*p),
                Some(Node::Glue(_) | Node::Leaders { .. }) if prev_non_discardable => Some(0),
                Some(Node::Kern(_) | Node::ExplicitKern(_))
                    if matches!(list.get(i + 1), Some(Node::Glue(_) | Node::Leaders { .. })) =>
                {
                    Some(0)
                }
                _ => None,
            };
            if let Some(p) = penalty.filter(|p| *p < 10000) {
                // The target is height, not height plus the trailing depth.
                let badness = if t < target64 {
                    if stretch[1..].iter().any(|s| *s != 0) {
                        0
                    } else {
                        crate::scaled::badness(
                            (target64 - t).min(i32::MAX as i64) as i32,
                            stretch[0].min(i32::MAX as i64) as i32,
                        ) as i64
                    }
                } else if t - target64 > shrink {
                    1073741823
                } else {
                    crate::scaled::badness(
                        (t - target64).min(i32::MAX as i64) as i32,
                        shrink.min(i32::MAX as i64) as i32,
                    ) as i64
                };
                let cost = if badness == 1073741823 {
                    badness
                } else if p <= -10000 {
                    p as i64
                } else if badness < 10000 {
                    badness + p as i64
                } else {
                    100000
                };
                if cost <= best_cost {
                    best_cost = cost;
                    best_split = i;
                }
                if cost == 1073741823 || p <= -10000 {
                    break;
                }
            }
            match &mut list[i] {
                Node::Box { h, d: depth, .. }
                | Node::Rule {
                    height: h, depth, ..
                } => {
                    t += d + *h as i64;
                    d = *depth as i64;
                }
                Node::Glue(g) | Node::Leaders { glue: g, .. } => {
                    stretch[(g.stretch_order as usize).min(3)] += g.stretch as i64;
                    shrink += g.shrink as i64;
                    if g.shrink_order != 0 && g.shrink != 0 {
                        if self.eqtb.int_params[IntParam::IgnorePrimitiveError.idx() as usize] & 1
                            != 0
                        {
                            self.append_log(
                                "\nignored: Infinite glue shrinkage found in box being split\n",
                            );
                        } else {
                            self.error("Infinite glue shrinkage found in box being split");
                        }
                        g.shrink_order = 0;
                    }
                    t += d + g.width as i64;
                    d = 0;
                }
                Node::Kern(k) | Node::ExplicitKern(k) => {
                    t += d + *k as i64;
                    d = 0;
                }
                _ => {}
            }
            // A negative splitmaxdepth applies even after glue and kerns.
            if d > smd {
                t += d - smd;
                d = smd;
            }
            prev_non_discardable = matches!(
                list[i],
                Node::Box { .. }
                    | Node::Rule { .. }
                    | Node::Ins { .. }
                    | Node::Mark { .. }
                    | Node::Whatsit(_)
                    | Node::Adj(_)
            );
        }
        let mut rest = list.split_off(best_split);
        let top = list;
        // prune_page_top keeps marks/whatsits/inserts while removing
        // discardable nodes before the first box, then inserts splittopskip.
        let mut seen_box = false;
        rest.retain(|n| {
            if seen_box {
                return true;
            }
            match n {
                Node::Box { .. } | Node::Rule { .. } => {
                    seen_box = true;
                    true
                }
                Node::Glue(_)
                | Node::Leaders { .. }
                | Node::Penalty(_)
                | Node::Kern(_)
                | Node::ExplicitKern(_) => false,
                _ => true,
            }
        });
        if let Some((i, height)) = rest.iter().enumerate().find_map(|(i, n)| match n {
            Node::Box { h, .. } | Node::Rule { height: h, .. } => Some((i, *h)),
            _ => None,
        }) {
            let mut skip =
                self.eqtb.glue_params[crate::prim::GlueParam::SplitTopSkip.idx() as usize];
            skip.width = (skip.width - height).max(0);
            rest.insert(i, Node::Glue(skip));
        }
        let mut seen = std::collections::HashSet::new();
        for node in &top {
            if let Node::Mark { class, tokens } = node {
                let class = *class as usize;
                if class < crate::eqtb::NUM_REGISTERS {
                    for marks in &mut self.marks[3..5] {
                        if marks.len() <= class {
                            marks.resize(class + 1, Vec::new());
                        }
                    }
                    if seen.insert(class) {
                        self.marks[3][class] = tokens.clone();
                    }
                    self.marks[4][class] = tokens.clone();
                }
            }
        }
        let r = crate::boxes::vpack_add_md(
            top,
            Some(target),
            false,
            crate::boxes::VBOX,
            &self.eqtb,
            smd as i32,
        );
        self.last_badness = r.badness;
        self.vsplat_remainder = Some(rest);
        Some(r.node)
    }
}
/// nodes tex removes at the start of the next line after a non-disc break
fn is_prunable(n: &Node) -> bool {
    matches!(
        n,
        Node::Glue(_) | Node::Penalty(_) | Node::ExplicitKern(_) | Node::MathKern(..)
    )
}

fn push_dims(eqtb: &crate::eqtb::Eqtb, n: Node, seg: &mut NodeList, w: &mut i64) {
    if let Node::Disc(mut dc) = n {
        *w += disc_list_width(eqtb, &dc.no_break);
        // The source replacement nodes have already been swallowed by the
        // caller. Keep the discretionary for faithful box structure, but do
        // not make later dimension scans skip the next live line node too.
        dc.replace_count = 0;
        seg.push(Node::Disc(dc));
        return;
    }
    let fonts = crate::boxes::eqtb_fonts(eqtb);
    let wd = match &n {
        Node::Char { c, font } => fonts.char_width(*font, *c),
        Node::Ligature { lig_width, .. } => *lig_width,
        Node::Glue(g) => g.width,
        Node::Kern(k) | Node::ExplicitKern(k) => *k,
        Node::Box { w: bw, .. } => *bw,
        Node::Rule { width, .. } => *width,
        _ => 0,
    };
    *w += wd as i64;
    seg.push(n);
}

fn disc_list_width(eqtb: &crate::eqtb::Eqtb, l: &[Node]) -> i64 {
    let fonts = crate::boxes::eqtb_fonts(eqtb);
    l.iter()
        .map(|nn| match nn {
            Node::Char { c, font } => fonts.char_width(*font, *c) as i64,
            Node::Ligature { lig_width, .. } => *lig_width as i64,
            Node::Kern(k) | Node::ExplicitKern(k) => *k as i64,
            Node::Box { w, .. } | Node::Rule { width: w, .. } => *w as i64,
            _ => 0,
        })
        .sum()
}

/// tex.web §25108–25148: the line-shape parameters computed once per
/// paragraph. Returns `(last_special_line, first_indent, first_width,
/// second_indent, second_width)`; lines `<= last_special_line` (when
/// nonzero) use the first pair, later lines the second. With `\parshape`
/// the first pair is per-line from the shape list (handled by
/// `line_metrics`); the returned first_* values are unused there.
fn line_shape(params: &ParaParams) -> (i32, i32, i32, i32, i32) {
    if !params.par_shape.is_empty() {
        // §25128–25131: last_special_line = n-1; the n-th shape entry's
        // (indent, width) serves all later lines.
        let last = params.par_shape.len() as i32 - 1;
        let (si, sw) = params.par_shape[last as usize];
        return (last, 0, params.hsize, si, sw);
    }
    if params.hang_indent == 0 {
        // §25123–25126
        return (0, 0, params.hsize, 0, params.hsize);
    }
    // §25136–25147
    let last = params.hang_after.saturating_abs();
    let hi = params.hang_indent.abs();
    let ind = if params.hang_indent >= 0 {
        params.hang_indent
    } else {
        0
    };
    if params.hang_after < 0 {
        (last, ind, params.hsize - hi, 0, params.hsize)
    } else {
        (last, 0, params.hsize, ind, params.hsize - hi)
    }
}

/// (left indent, width) for 1-based line number `line`
fn line_metrics(params: &ParaParams, line: i32) -> (i32, i32) {
    let (last_special_line, first_indent, first_width, second_indent, second_width) =
        line_shape(params);
    if line > last_special_line {
        return (second_indent, second_width);
    }
    if !params.par_shape.is_empty() {
        let idx = (line - 1).max(0) as usize;
        return params.par_shape[idx.min(params.par_shape.len() - 1)];
    }
    (first_indent, first_width)
}

/// d = (line_penalty + b)^2 + penalty term (tex.web §859)
fn demerits(params: &ParaParams, b: i32, pi: i32) -> i64 {
    let d = params.line_penalty as i64 + b as i64;
    let mut d = if d.abs() >= 10000 {
        100_000_000i64
    } else {
        d * d
    };
    if pi != 0 {
        if pi > 0 {
            d += (pi as i64) * (pi as i64);
        } else if pi > EJECT_PENALTY {
            d -= (pi as i64) * (pi as i64);
        }
    }
    d
}

/// extra demerits: double-hyphen / final-hyphen, and adjacent fitness
fn fitness_demerits(
    params: &ParaParams,
    a: &ActiveNode,
    btype: BreakType,
    fit: usize,
    at_end: bool,
) -> i64 {
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
    cum_fst: &[i64],
    cum_fsh: &[i64],
) -> (i64, [i64; 4], [i64; 4], i64, i64) {
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
                (cum_w[f], cum_st[f], cum_sh[f], cum_fst[f], cum_fsh[f])
            } else {
                // startsum = C[a] + no_break - post_break (dead nodes are 0)
                let sw = cum_w[cand] - post_w + disc_list_width(eqtb, &dc.no_break);
                (sw, cum_st[cand], cum_sh[cand], cum_fst[cand], cum_fsh[cand])
            }
        } else {
            (
                cum_w[cand],
                cum_st[cand],
                cum_sh[cand],
                cum_fst[cand],
                cum_fsh[cand],
            )
        }
    } else {
        let f = after_prune(cand);
        (cum_w[f], cum_st[f], cum_sh[f], cum_fst[f], cum_fsh[f])
    }
}

#[cfg(test)]
mod plural_penalty_tests {
    use super::*;
    use crate::boxes::GLUE_FIL;

    #[test]
    fn plural_penalties_are_applied_by_line_and_remaining_line() {
        let mut engine = Engine::new(true);
        engine.eqtb.dim_params[DimParam::HSize.idx() as usize] = 65_536;
        engine.eqtb.int_params[IntParam::Pretolerance.idx() as usize] = 10_000;
        engine.eqtb.int_params[IntParam::Tolerance.idx() as usize] = 10_000;
        engine.eqtb.int_params[IntParam::LinePenalty.idx() as usize] = 0;
        engine.penalty_shapes[0] = Rc::from(vec![10, 20, 30]);
        engine.penalty_shapes[1] = Rc::from(vec![100, 200, 300]);
        engine.penalty_shapes[2] = Rc::from(vec![1_000, 2_000, 3_000]);

        let rule = || Node::Rule {
            width: 65_536,
            height: 0,
            depth: 0,
        };
        let list = vec![
            rule(),
            Node::Glue(Glue::zero()),
            rule(),
            Node::Glue(Glue::zero()),
            rule(),
            Node::Penalty(10_000),
            Node::Glue(Glue::fil(GLUE_FIL, 0)),
        ];
        let Node::Box { list, .. } = engine.break_paragraph(list, 0, false) else {
            panic!("paragraph breaker did not return a vbox");
        };
        let penalties: Vec<i32> = list
            .iter()
            .filter_map(|node| match node {
                Node::Penalty(value) => Some(*value),
                _ => None,
            })
            .collect();

        assert_eq!(penalties, vec![2_110, 1_220]);
    }
    #[test]
    fn paragraph_layout_record_tracks_lines_and_pass() {
        let mut engine = Engine::new(true);
        engine.eqtb.dim_params[DimParam::HSize.idx() as usize] = 65_536;
        engine.eqtb.int_params[IntParam::Pretolerance.idx() as usize] = 10_000;
        engine.eqtb.int_params[IntParam::Tolerance.idx() as usize] = 10_000;
        engine.eqtb.int_params[IntParam::LinePenalty.idx() as usize] = 10;

        let rule = || Node::Rule {
            width: 65_536,
            height: 0,
            depth: 0,
        };
        let list = vec![
            rule(),
            Node::Glue(Glue::zero()),
            rule(),
            Node::Penalty(10_000),
            Node::Glue(Glue::fil(GLUE_FIL, 0)),
        ];
        let (_node, record) = engine.break_paragraph_with_record(list, 0, false);
        assert_eq!(record.lines, 2);
        assert_eq!(record.pass, 0); // pretolerance succeeded
        assert!(record.demerits > 0);
        assert_eq!(engine.last_paragraph_layout.as_ref(), Some(&record));
    }
}
