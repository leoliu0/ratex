//! Knuth-Plass paragraph breaking — a port of tex.web's `line_break`
//! (feasible breakpoints, per-(line,fitness) champions, two-pass + emergency
//! pass with artificial-demerits rescue), producing a vbox of line boxes.

use crate::boxes::{Glue, Node, NodeList, WhatIt};
use crate::engine::Engine;
use crate::language::LangState;
use crate::fonts::FontResolver;
use crate::prim::{DimParam, GlueParam, IntParam};
use crate::scaled::{badness, EJECT_PENALTY, INF_BAD, INF_PENALTY};
use crate::tfm::FontId;
use std::collections::HashMap;

mod xetex_hyph;
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
    /// left margin protrusion less the width of the left local box
    left_prot: i32,
    prev: Option<Rc<ActiveNode>>,
    pub ratio: i32,
    /// e-TeX `active_short` / `active_glue` (\lastlinefit data)
    short: i64,
    glue: i64,
}

/// `ActiveNode::lp` of a breakpoint no `local_par` node precedes
const NO_LOCAL_PAR: u32 = u32::MAX;

/// e-TeX \lastlinefit setup (etex.ch <Check for special treatment of last
/// line of paragraph>): the infinite stretch of \parfillskip by order.
#[derive(Clone, Copy)]
struct LastLineFit {
    fill_width: [i64; 3],
    fit: i64,
}

/// etex.ch `fract(x,n,d,max_answer)`: floor(xn/d+1/2) with the sign of
/// the operands, `None` on overflow (arith_error).
fn fract(x: i64, n: i64, d: i64, max_answer: i64) -> Option<i64> {
    if d == 0 {
        return None;
    }
    let negative = (x < 0) ^ (n < 0) ^ (d < 0);
    let (x, n, d) = (
        i128::from(x).abs(),
        i128::from(n).abs(),
        i128::from(d).abs(),
    );
    let q = (x * n + d / 2) / d;
    if q > i128::from(max_answer) {
        return None;
    }
    let q = q as i64;
    Some(if negative { -q } else { q })
}

/// etex.ch <Perform computations for last line and goto found>: badness,
/// fitness class and adjustment `g` of the paragraph's last line after
/// active node `a`, or `None` (not_found) to use the ordinary rules.
fn last_line_badness(
    llf: LastLineFit,
    a: &ActiveNode,
    shortfall: i64,
    dst: &[i64; 4],
    dsh: &[i64; 4],
) -> Option<(i32, usize, i64)> {
    const MAX_DIMEN: i64 = 0x3FFF_FFFF;
    if a.short == 0 || a.glue <= 0 {
        return None;
    }
    if dst[1..] != llf.fill_width {
        return None;
    }
    let g = if a.short > 0 { dst[0] } else { dsh[0] };
    if g <= 0 {
        return None;
    }
    let g = fract(g, a.short, a.glue, MAX_DIMEN).and_then(|g| {
        if llf.fit < 1000 {
            fract(g, llf.fit, 1000, MAX_DIMEN)
        } else {
            Some(g)
        }
    });
    let mut g = g.unwrap_or(if a.short > 0 { MAX_DIMEN } else { -MAX_DIMEN });
    if g > 0 {
        if g > shortfall {
            g = shortfall;
        }
        if g > 7_230_584 && dst[0] < 1_663_497 {
            return Some((INF_BAD, VERY_LOOSE, g));
        }
        let b = badness(g as i32, dst[0] as i32);
        let fit = if b > 99 {
            VERY_LOOSE
        } else if b > 12 {
            LOOSE
        } else {
            DECENT
        };
        Some((b, fit, g))
    } else if g < 0 {
        if -g > dsh[0] {
            g = -dsh[0];
        }
        let b = badness(-g as i32, dsh[0] as i32);
        Some((b, if b > 12 { TIGHT } else { DECENT }, g))
    } else {
        None
    }
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
            Node::Char { font, c, .. } | Node::Ligature { font, c, .. } => {
                return char_protrusion_width(eqtb, protrude_chars, *font, *c, true);
            }
            Node::Glue(_, _)
            | Node::Penalty(_, _)
            | Node::Kern(_, _)
            | Node::ExplicitKern(_, _)
            // pdftex cp_skipable: only a zero-width accent kern is skipped
            | Node::AccentKern(0, _) | Node::ItalicKern(0, _)
            | Node::Whatsit(_, _) => {}
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
    lua_kerns: bool,
    lua_mode: bool,
    record_expansion: &mut F,
) -> (i64, i64, Option<(FontId, u8)>)
where
    F: FnMut(FontId),
{
    let mut stretch = 0i64;
    let mut shrink = 0i64;
    for (i, node) in nodes.iter().enumerate() {
        match node {
            // luatex: every character is a glyph node, TFM fonts included
            Node::Char { .. } | Node::Ligature { .. } | Node::LuaGlyph(_) if lua_mode => {
                if let Some(gr) = crate::luaexp::glyph_ref(node).filter(|gr| crate::luaexp::expandable(eqtb, gr.font)) {
                    record_expansion(gr.font);
                    stretch += crate::luaexp::char_stretch(eqtb, gr) as i64;
                    shrink += crate::luaexp::char_shrink(eqtb, gr) as i64;
                }
            }
            Node::Char { c, font, .. } | Node::Ligature { c, font, .. } => {
                record_expansion(*font);
                stretch += crate::boxes::char_stretch(eqtb, *font, *c) as i64;
                shrink += crate::boxes::char_shrink(eqtb, *font, *c) as i64;
                prev = Some((*font, *c));
            }
            Node::LuaGlyph(g) if crate::luaexp::expandable(eqtb, g.font) => {
                record_expansion(g.font);
                let gr = crate::luaexp::GlyphRef { font: g.font, c: g.c };
                stretch += crate::luaexp::char_stretch(eqtb, gr) as i64;
                shrink += crate::luaexp::char_shrink(eqtb, gr) as i64;
            }
            Node::Kern(k, _) => {
                let next = if i + 1 < nodes.len() {
                    nodes.get(i + 1)
                } else {
                    trailing
                };
                if lua_kerns && lua_mode {
                    let left = i.checked_sub(1).and_then(|j| crate::luaexp::glyph_ref(&nodes[j]));
                    if let (Some(lr), Some(rr)) = (left, next.and_then(crate::luaexp::glyph_ref)) {
                        if crate::luaexp::expandable(eqtb, lr.font) {
                            stretch += crate::luaexp::kern_stretch(eqtb, *k, lr, rr) as i64;
                            shrink += crate::luaexp::kern_shrink(eqtb, *k, lr, rr) as i64;
                        }
                    }
                }
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
    /// luatex `swap_hang_indent`: \shapemode 1 and 3 (or their negatives)
    /// mirror \hangindent
    pub(crate) fn swap_hang_indent(&self, indent: i32) -> i32 {
        match self.eqtb.int_params[IntParam::ShapeMode.idx() as usize] {
            1 | 3 | -1 | -3 => indent.wrapping_neg(),
            _ => indent,
        }
    }

    /// luatex `swap_parshape_indent`: \shapemode 2 and 3 (or their
    /// negatives) mirror each \parshape line
    pub(crate) fn swap_parshape_indent(&self, indent: i32, width: i32) -> i32 {
        match self.eqtb.int_params[IntParam::ShapeMode.idx() as usize] {
            2 | 3 | -2 | -3 => self.eqtb.dim_params[DimParam::HSize.idx() as usize]
                .wrapping_sub(width)
                .wrapping_sub(indent),
            _ => indent,
        }
    }

    pub fn para_params(&self) -> ParaParams {
        let e = &self.eqtb;
        ParaParams {
            hsize: e.dim_params[DimParam::HSize.idx() as usize],
            left_skip: e.glue_params[GlueParam::LeftSkip.idx() as usize]
                .param(crate::boxes::glue_subtype::LEFT_SKIP),
            right_skip: e.glue_params[GlueParam::RightSkip.idx() as usize]
                .param(crate::boxes::glue_subtype::RIGHT_SKIP),
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
            par_shape: if self.engine_kind == crate::engine::EngineKind::LuaTeX {
                self.par_shape.iter().map(|&(i, w)| (self.swap_parshape_indent(i, w), w)).collect()
            } else {
                self.par_shape.clone()
            },
            hang_indent: if self.engine_kind == crate::engine::EngineKind::LuaTeX {
                self.swap_hang_indent(e.dim_params[DimParam::HangIndent.idx() as usize])
            } else {
                e.dim_params[DimParam::HangIndent.idx() as usize]
            },
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
        // etex.ch <Check for special treatment of last line of paragraph>:
        // \parfillskip (the list's last node) must stretch infinitely and
        // \leftskip+\rightskip finitely
        let last_line_fit = {
            let fit = self.eqtb.int_params[IntParam::LastLineFit.idx() as usize];
            match list.last() {
                Some(Node::Glue(q, _))
                    if fit > 0
                        && q.stretch > 0
                        && q.stretch_order > 0
                        && bg_st[1..] == [0, 0, 0] =>
                {
                    let mut fill_width = [0i64; 3];
                    fill_width[q.stretch_order as usize - 1] = q.stretch as i64;
                    Some(LastLineFit {
                        fill_width,
                        fit: fit as i64,
                    })
                }
                _ => None,
            }
        };

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
        // tex.web §863/§866: words are hyphenated only by the second pass
        // (at the glue before each word), so a paragraph the first pass
        // sets keeps its original list
        if second_pass {
            self.hyphenate_list(&mut list);
        }
        let mut extra_stretch = 0i32;
        let mut best: Option<Rc<ActiveNode>> = None;
        let mut final_ran = false;
        let lua = self.engine_kind == crate::engine::EngineKind::LuaTeX;
        loop {
            if threshold > INF_BAD {
                threshold = INF_BAD;
            }
            // the LuaTeX code is a separate instantiation: pdfTeX's pass is
            // the plain Knuth-Plass loop
            let pass = if lua {
                self.try_break::<true>(&list, &params, threshold, final_pass, second_pass, extra_stretch, bg_w, bg_st, bg_sh, last_line_fit)
            } else {
                self.try_break::<false>(&list, &params, threshold, final_pass, second_pass, extra_stretch, bg_w, bg_st, bg_sh, last_line_fit)
            };
            match pass {
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
                        self.hyphenate_list(&mut list);
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
            // tex.web §887: \leftskip glue only when it is not zero_glue
            if !params.left_skip.is_zero_glue() {
                inner.push(Node::Glue(params.left_skip, self.eqtb.cur_attr));
            }
            let mut pre_adj: NodeList = Vec::new();
            let mut post_adj: NodeList = Vec::new();
            for n in list.into_iter() {
                match n {
                    Node::VAdjust(items, _) => post_adj.extend(items),
                    Node::PreAdjust(items, _) => pre_adj.extend(items),
                    n => inner.push(n),
                }
            }
            inner.push(Node::Glue(params.right_skip.clone(), self.eqtb.cur_attr));
            let line = crate::boxes::hpack(inner, None, crate::boxes::HBOX, &self.eqtb).node;
            let mut vlines = pre_adj;
            vlines.push(line);
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
        // etex.ch <Adjust the final line of the paragraph>
        if last_line_fit.is_some() && end.short != 0 {
            if let Some(Node::Glue(q, _)) = list.last_mut() {
                q.width += (end.short - end.glue) as i32;
                q.stretch = 0;
            }
        }
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
        let node = if lua {
            self.build_lines::<true>(list, &params, end, final_widow_penalty, display_widow)
        } else {
            self.build_lines::<false>(list, &params, end, final_widow_penalty, display_widow)
        };
        self.last_paragraph_layout = Some(record.clone());
        (node, record)
    }

    /// tex.web §891-§918 (second pass, "Try to hyphenate the following
    /// word"): after every glue node outside math, find the word, insert
    /// its discretionary hyphens and reconstitute ligatures and kerns
    /// around them. Native-font words follow `hyphenate_native_words`.
    /// The language starts as new_graf recorded it for the paragraph and
    /// follows the `\setlanguage` whatsits (`adv_past`).
    fn hyphenate_list(&mut self, list: &mut NodeList) {
        if self.engine_kind == crate::engine::EngineKind::XeTeX {
            self.xetex_hyphenate_list(list);
            return;
        }
        let start = self.paragraph_language();
        let mut lang = start;
        // (first replaced index, end index, replacement)
        let mut edits: Vec<(usize, usize, NodeList)> = Vec::new();
        let mut auto_breaking = true;
        let mut i = 0;
        while i < list.len() {
            match &list[i] {
                // §866: math-off re-enables automatic breaking (etex.ch:
                // only math nodes below L_code, i.e. not \beginL..\endR)
                Node::MathKern(_, kind @ 1..=4, _) => auto_breaking = crate::boxes::math_end_lr(*kind),
                Node::Whatsit(WhatIt::Language { lang: l, lhm, rhm }, _) => {
                    lang = LangState {
                        lang: *l,
                        lhm: *lhm,
                        rhm: *rhm,
                    };
                }
                Node::Glue(_, _) | Node::Leaders { .. } if auto_breaking => {
                    if let Some(edit) = self.hyphenate_word_after(list, i, &mut lang) {
                        i = edit.1;
                        edits.push(edit);
                        continue;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        for (start, end, nodes) in edits.into_iter().rev() {
            list.splice(start..end, nodes);
        }
        if list.iter().any(|n| matches!(n, Node::NativeGlyphRun { .. })) {
            if let Some(ctx) = self.hyph_ctx(start) {
                self.hyphenate_native_words(list, &ctx);
            }
        }
    }

    /// tex.web §891 init_cur_lang/init_l_hyf/init_r_hyf: the values
    /// new_graf stored for the paragraph being broken (the current
    /// parameters outside a paragraph).
    fn paragraph_language(&self) -> LangState {
        match self.par_langs.last() {
            Some(p) => p.start,
            None => self.current_language(),
        }
    }

    /// Hyphenation inputs for one language state; `None` when the
    /// language cannot hyphenate (no patterns, or the minima exceed 63).
    fn hyph_ctx(&self, st: LangState) -> Option<HyphCtx<'_>> {
        if st.lang == 255 {
            return None;
        }
        let trie = self.trie_for_language(st.lang).filter(|t| !t.is_empty())?;
        // TeX considers at most 63 letters while hyphenating. A larger
        // minimum sum therefore disables automatic hyphenation.
        let (lh, rh) = (usize::from(st.lhm), usize::from(st.rhm));
        if lh + rh > 63 {
            return None;
        }
        Some(HyphCtx {
            trie,
            codes: self.hyphen_codes.get(&st.lang).map(Box::as_ref),
            lc_code: &self.eqtb.lc_code,
            lh,
            rh,
            uc_hyph: self.eqtb.int_params[IntParam::UcHyph.idx() as usize] > 0,
            min_len: self.lang_hyphenation_min(st.lang),
            pre_hyphen: self.lua_tex.lang.get(&st.lang).and_then(|p| p.pre_hyphen),
        })
    }

    /// tex.web §894-§903 for the word after the glue at `g`: returns the
    /// replacement for `list[start..end]` when hyphens were found.
    fn hyphenate_word_after(
        &self,
        list: &[Node],
        g: usize,
        lang: &mut LangState,
    ) -> Option<(usize, usize, NodeList)> {
        let mut ctx = self.hyph_ctx(*lang);
        // §896: skip to node ha, the one just before the first letter
        let mut ha = g;
        let mut s = g + 1;
        let hf = loop {
            let (c, f) = match list.get(s)? {
                Node::Char { c, font, .. } => (*c, *font),
                Node::Ligature {
                    letters,
                    n_letters,
                    font,
                    ..
                } if *n_letters > 0 => (letters[0], *font),
                // §1363 adv_past in the pre-hyphenation loop
                Node::Whatsit(WhatIt::Language { lang: l, lhm, rhm }, _) => {
                    *lang = LangState {
                        lang: *l,
                        lhm: *lhm,
                        rhm: *rhm,
                    };
                    ctx = self.hyph_ctx(*lang);
                    ha = s;
                    s += 1;
                    continue;
                }
                // etex.ch: text-direction math nodes are skipped like kerns
                Node::Ligature { .. } | Node::Kern(_, _) | Node::Whatsit(_, _) => {
                    ha = s;
                    s += 1;
                    continue;
                }
                Node::MathKern(_, kind, _) if *kind >= crate::boxes::LR_KIND_MIN => {
                    ha = s;
                    s += 1;
                    continue;
                }
                _ => return None,
            };
            let lc = ctx.as_ref()?.lc(c);
            if lc != 0 {
                if lc == c || ctx.as_ref()?.uc_hyph {
                    break f;
                }
                return None;
            }
            ha = s;
            s += 1;
        };
        let ctx = ctx?;
        let ctx = &ctx;
        let hyf_char = ctx.pre_hyphen.unwrap_or_else(|| self.eqtb.hyphen_char.get(hf as usize).copied().unwrap_or(-1));
        let hyf_char = u8::try_from(hyf_char).ok()?;
        let font = self.eqtb.fonts.get(hf as usize)?.clone();
        // §897-898: the letters hu[1..=hn] (hc lowercased) of nodes ..=hb
        let mut hu = [NON_CHAR; 66];
        let mut hc = [0u8; 66];
        let mut hn = 0usize;
        let mut hb = s;
        let first_attr = list[s].attr();
        let mut hyf_bchar: Option<u8> = None;
        'word: loop {
            match list.get(s) {
                Some(Node::Char { c, font: f, .. }) => {
                    if *f != hf {
                        break;
                    }
                    hyf_bchar = Some(*c);
                    let lc = ctx.lc(*c);
                    if lc == 0 || hn == 63 {
                        break;
                    }
                    hb = s;
                    hn += 1;
                    hu[hn] = *c as u16;
                    hc[hn] = lc;
                    hyf_bchar = None;
                }
                Some(Node::Ligature {
                    font: f,
                    letters,
                    n_letters,
                    subtype,
                    ..
                }) => {
                    if *f != hf {
                        break;
                    }
                    let mut j = hn;
                    if *n_letters > 0 {
                        hyf_bchar = Some(letters[0]);
                    }
                    for &c in &letters[..*n_letters as usize] {
                        let lc = ctx.lc(c);
                        if lc == 0 || j == 63 {
                            break 'word;
                        }
                        j += 1;
                        hu[j] = c as u16;
                        hc[j] = lc;
                    }
                    hb = s;
                    hn = j;
                    hyf_bchar = if subtype & 1 != 0 { font.bchar } else { None };
                }
                Some(Node::Kern(_, _)) => {
                    hb = s;
                    hyf_bchar = font.bchar;
                }
                _ => break,
            }
            s += 1;
        }
        // §899: the nodes after hb must permit hyphenation
        if hn < ctx.lh + ctx.rh || hn < ctx.min_len {
            return None;
        }
        loop {
            match list.get(s) {
                Some(Node::Char { .. } | Node::Ligature { .. } | Node::Kern(_, _)) => s += 1,
                None
                | Some(
                    Node::ExplicitKern(_, _)
                    | Node::AccentKern(_, _) | Node::ItalicKern(_, _)
                    | Node::Whatsit(_, _)
                    | Node::Glue(_, _)
                    | Node::Leaders { .. }
                    | Node::Penalty(_, _)
                    | Node::Ins { .. }
                    | Node::VAdjust(_, _)
                    | Node::PreAdjust(_, _)
                    | Node::Mark { .. },
                ) => break,
                // etex.ch: `math_node: if subtype(s)>=L_code then goto done4`
                Some(Node::MathKern(_, kind, _)) if *kind >= crate::boxes::LR_KIND_MIN => break,
                _ => return None,
            }
        }
        // §923: hyphen positions; hyf[j] odd = a hyphen after letter j
        let mut hyf = [0u8; 65];
        for k in ctx.trie.hyphenate(&hc[1..=hn], ctx.lh, ctx.rh) {
            if (ctx.lh..=hn - ctx.rh).contains(&k) {
                hyf[k] = 1;
            }
        }
        if !hyf.contains(&1) {
            return None;
        }
        Some(Self::replace_hyphenated_word(
            list,
            ha,
            hb,
            hf,
            &font,
            hu.to_vec(),
            hyf.to_vec(),
            hn,
            hyf_bchar,
            hyf_char,
            first_attr,
        ))
    }

    /// tex.web §903: nodes `ha+1..=hb` of `list` (the word hu[1..=hn]) become
    /// the reconstituted word with its discretionaries. hu[0] is the
    /// punctuation character or ligature before the word (reconstituted
    /// along with it) or the left boundary.
    #[allow(clippy::too_many_arguments)]
    fn replace_hyphenated_word(
        list: &[Node],
        ha: usize,
        hb: usize,
        hf: FontId,
        font: &crate::tfm::Font,
        hu: Vec<u16>,
        hyf: Vec<u8>,
        hn: usize,
        hyf_bchar: Option<u8>,
        hyf_char: u8,
        first_attr: crate::boxes::Attr,
    ) -> (usize, usize, NodeList) {
        let mut rc = Reconstitute {
            font,
            hf,
            hu,
            hyf,
            init_list: [0; 3],
            init_len: 0,
            init_lig: false,
            init_lft: false,
            hyphen_passed: 0,
            hold: Vec::new(),
            attr: first_attr,
        };
        let (start, j0) = match &list[ha] {
            Node::Char { c, font: f, .. } if *f == hf => {
                rc.init_list[0] = *c;
                rc.init_len = 1;
                rc.hu[0] = *c as u16;
                (ha, 0)
            }
            Node::Ligature {
                c,
                font: f,
                letters,
                n_letters,
                subtype,
                ..
            } if *f == hf => {
                rc.init_list = *letters;
                rc.init_len = *n_letters as usize;
                rc.init_lig = true;
                rc.init_lft = *subtype > 1;
                rc.hu[0] = *c as u16;
                if rc.init_len == 0 && rc.init_lft {
                    rc.hu[0] = NON_CHAR;
                    rc.init_lig = false;
                }
                (ha, 0)
            }
            // found2: another font's character keeps its place; the word
            // starts at the left boundary
            Node::Char { .. } | Node::Ligature { .. } => (ha + 1, 0),
            _ => match &list[ha + 1] {
                Node::Ligature { subtype, .. } if *subtype > 1 => (ha + 1, 0),
                _ => (ha + 1, 1),
            },
        };
        let nodes = rc.hyphenated_word(j0, hn, hyf_bchar, hyf_char);
        (start, hb + 1, nodes)
    }

    /// XeTeX native-font words: a hyphen point splits the glyph run, with
    /// pre/post texts reshaped.
    fn hyphenate_native_words(&self, list: &mut NodeList, ctx: &HyphCtx) {
        let mut word: Vec<u8> = Vec::new();
        // per letter: (node index, byte slot inside the run)
        let mut word_positions: Vec<(usize, u8)> = Vec::new();
        // tex.web §26160-26224: `hf`, the font of the word's first letter,
        // owns the hyphen character — NOT the font current at paragraph end
        let mut word_font: u16 = 0;
        let mut prev_ok = false;
        let mut can_start_word = false;
        let mut edits: Vec<(usize, Node)> = Vec::new();
        for i in 0..list.len() {
            let mut node_letters: Vec<u8> = Vec::new();
            let mut node_font: u16 = 0;
            if let Node::NativeGlyphRun {
                run, start, end, ..
            } = &list[i]
            {
                let mut is_ascii_letters = true;
                let mut letters = Vec::new();
                for g in &run.glyphs[*start..*end] {
                    let text_slice = &run.text[g.cluster_start as usize..g.cluster_end as usize];
                    for b in text_slice.bytes() {
                        let lc = if b.is_ascii_alphabetic() { ctx.lc(b) } else { 0 };
                        if lc == 0 {
                            is_ascii_letters = false;
                            break;
                        }
                        letters.push(lc);
                    }
                    if !is_ascii_letters {
                        break;
                    }
                }
                if is_ascii_letters && !letters.is_empty() {
                    node_letters = letters;
                    node_font = run.font;
                }
            }
            if !node_letters.is_empty() {
                if word.is_empty() {
                    word_font = node_font;
                    word_positions.clear();
                    // tex.web §894: only glue starts the lookahead for a
                    // hyphenatable word
                    prev_ok = can_start_word;
                    can_start_word = false;
                } else if node_font != word_font {
                    self.flush_native_word(
                        list,
                        i,
                        &word,
                        &word_positions,
                        prev_ok,
                        ctx,
                        word_font,
                        &mut edits,
                    );
                    word.clear();
                    word_positions.clear();
                    word_font = node_font;
                    prev_ok = false;
                }
                for (j, lc) in node_letters.iter().enumerate() {
                    word.push(*lc);
                    word_positions.push((i, j as u8));
                }
            } else if !word.is_empty() {
                self.flush_native_word(
                    list,
                    i,
                    &word,
                    &word_positions,
                    prev_ok,
                    ctx,
                    word_font,
                    &mut edits,
                );
                word.clear();
            }
            if word.is_empty() {
                match &list[i] {
                    Node::Glue(_, _) => can_start_word = true,
                    Node::Char { .. } | Node::Ligature { .. } | Node::Whatsit(_, _) => {}
                    _ => can_start_word = false,
                }
            }
        }
        edits.sort_by(|a, b| a.0.cmp(&b.0));
        for (offset, (pos, node)) in edits.into_iter().enumerate() {
            list.insert(pos + offset, node);
        }
    }

    /// hyphenate one completed native-font word: the hyphen character
    /// comes from the word's own font `wf`
    #[allow(clippy::too_many_arguments)]
    fn flush_native_word(
        &self,
        list: &[Node],
        end: usize,
        word: &[u8],
        word_positions: &[(usize, u8)],
        prev_ok: bool,
        ctx: &HyphCtx,
        wf: u16,
        edits: &mut Vec<(usize, Node)>,
    ) {
        let Some(hyphen_c) = self
            .eqtb
            .hyphen_char
            .get(wf as usize)
            .and_then(|&h| u8::try_from(h).ok())
        else {
            return;
        };
        // a word closed by an explicit hyphen gets no internal points
        let closed_by_hyphen = matches!(&list[end], Node::Char { c, .. } if *c == hyphen_c)
            || matches!(&list[end], Node::Disc(_));
        if closed_by_hyphen || !prev_ok || word.len() < ctx.lh + ctx.rh || word.len() < ctx.min_len {
            return;
        }
        let hyphen_str = (hyphen_c as char).to_string();
        let mut disc_at_node: Option<usize> = None;
        for k in ctx.trie.hyphenate(word, ctx.lh, ctx.rh) {
            if k == 0 || k >= word_positions.len() {
                continue;
            }
            // point k = break before letter k
            let (pos, slot) = word_positions[k];
            if disc_at_node == Some(pos) {
                continue; // one disc per node
            }
            let Node::NativeGlyphRun {
                run, start, end, ..
            } = &list[pos]
            else {
                continue;
            };
            let disc = if slot > 0 {
                let slice_start_byte = run.glyphs[*start].cluster_start as usize;
                let slice_end_byte = run.glyphs[*end - 1].cluster_end as usize;
                let split_byte = slice_start_byte + slot as usize;
                if split_byte > slice_end_byte {
                    continue;
                }
                let pre_str = format!("{}{hyphen_str}", &run.text[slice_start_byte..split_byte]);
                let post_str = &run.text[split_byte..slice_end_byte];
                let (Ok(pre_break), Ok(post_break)) = (
                    self.shape_native_slice(run.font, &pre_str),
                    self.shape_native_slice(run.font, post_str),
                ) else {
                    continue;
                };
                crate::boxes::DiscNode::new(pre_break, post_break, vec![list[pos].clone()], 1)
            } else {
                crate::boxes::DiscNode::new(
                    self.shape_native_slice(run.font, &hyphen_str).unwrap_or_default(),
                    Vec::new(),
                    Vec::new(),
                    0,
                )
            };
            disc_at_node = Some(pos);
            edits.push((pos, Node::Disc(disc)));
        }
    }

    /// one Knuth-Plass pass; returns the final breakpoint chain on success
    #[allow(clippy::too_many_arguments)]
    fn try_break<const LUA: bool>(
        &self,
        list: &[Node],
        params: &ParaParams,
        threshold: i32,
        final_pass: bool,
        second_pass: bool,
        extra_stretch: i32,
        bg_w: i64,
        bg_st: [i64; 4],
        bg_sh: [i64; 4],
        last_line_fit: Option<LastLineFit>,
    ) -> Option<Rc<ActiveNode>> {
        let n = list.len();
        let pdf_adjust =
            self.eqtb.int_params[crate::prim::IntParam::PdfAdjustSpacing.idx() as usize];
        // luatex: the nodes a discretionary replaces are not part of the list.
        // line_break runs try_break::<true> exactly in LuaTeX mode, so the
        // pdfTeX instantiation folds every LuaTeX branch away.
        let lua_mode = LUA;
        let lb_dead_mask = if lua_mode { crate::luaexp::dead_mask(list) } else { Vec::new() };
        let lb_dead = |i: usize| lb_dead_mask.get(i).copied().unwrap_or(false);
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

            let mut record_expansion = |font: u16| {
                if pdf_adjust >= 2 && (stretch_steps == 0 || shrink_steps == 0) {
                    let (lua_step, lua_stretch, lua_shrink) = crate::luaexp::limits(&self.eqtb, font);
                    if lua_step > 0 {
                        if stretch_steps == 0 && lua_stretch > 0 {
                            stretch_steps = i64::from(lua_stretch / lua_step);
                        }
                        if shrink_steps == 0 && lua_shrink > 0 {
                            shrink_steps = i64::from(lua_shrink / lua_step);
                        }
                    }
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
                    Node::Char { .. } | Node::Ligature { .. } | Node::LuaGlyph(_) if lua_mode => {
                        prev_exp_char = None;
                        let (mut fst, mut fsh) = (0i64, 0i64);
                        let gr = crate::luaexp::glyph_ref(&list[i]);
                        if let Some(gr) = gr.filter(|gr| pdf_adjust >= 2 && crate::luaexp::expandable(&self.eqtb, gr.font)) {
                            record_expansion(gr.font);
                            fst = i64::from(crate::luaexp::char_stretch(&self.eqtb, gr));
                            fsh = i64::from(crate::luaexp::char_shrink(&self.eqtb, gr));
                        }
                        let wd = match &list[i] {
                            Node::Char { c, font, .. } => fonts.char_width(*font, *c),
                            Node::Ligature { lig_width, .. } => *lig_width,
                            Node::LuaGlyph(g) => crate::boxes::lua_glyph_dims(&self.eqtb, g).0,
                            _ => 0,
                        };
                        (i64::from(wd), [0; 4], [0; 4], fst, fsh)
                    }
                    Node::Char { c, font, .. } => {
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
                    Node::LuaGlyph(g) => {
                        prev_exp_char = None;
                        let (mut fst, mut fsh) = (0i64, 0i64);
                        if pdf_adjust >= 2 && crate::luaexp::expandable(&self.eqtb, g.font) {
                            record_expansion(g.font);
                            let gr = crate::luaexp::GlyphRef { font: g.font, c: g.c };
                            fst = i64::from(crate::luaexp::char_stretch(&self.eqtb, gr));
                            fsh = i64::from(crate::luaexp::char_shrink(&self.eqtb, gr));
                        }
                        (crate::boxes::lua_glyph_dims(&self.eqtb, g).0 as i64, [0; 4], [0; 4], fst, fsh)
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
                    Node::Glue(g, _) => {
                        let mut st = [0i64; 4];
                        let mut sh = [0i64; 4];
                        st[g.stretch_order as usize] = g.stretch as i64;
                        sh[g.shrink_order as usize] = g.shrink as i64;
                        (g.width as i64, st, sh, 0, 0)
                    }
                    Node::Kern(k, _) => {
                        let next = match list.get(i + 1) {
                            Some(Node::Char { c, font, .. } | Node::Ligature { c, font, .. }) => {
                                Some((*font, *c))
                            }
                            _ => None,
                        };
                        let lua_kern = if pdf_adjust == 2 && i > 0 && !lb_dead(i - 1) {
                            match (crate::luaexp::glyph_ref(&list[i - 1]), list.get(i + 1).and_then(crate::luaexp::glyph_ref)) {
                                (Some(lr), Some(rr)) if lua_mode && crate::luaexp::expandable(&self.eqtb, lr.font) => {
                                    Some((
                                        i64::from(crate::luaexp::kern_stretch(&self.eqtb, *k, lr, rr)),
                                        i64::from(crate::luaexp::kern_shrink(&self.eqtb, *k, lr, rr)),
                                    ))
                                }
                                _ => None,
                            }
                        } else {
                            None
                        };
                        let (fst, fsh) = if let Some(lua_kern) = lua_kern {
                            lua_kern
                        } else if pdf_adjust >= 2 {
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
                    Node::ExplicitKern(k, _) | Node::AccentKern(k, _) | Node::ItalicKern(k, _) => {
                        (*k as i64, [0; 4], [0; 4], 0, 0)
                    }
                    Node::ExKern { width, ex, .. } => ((*width + *ex) as i64, [0; 4], [0; 4], 0, 0),
                    Node::Disc(dc) => {
                        let mut fst = 0i64;
                        let mut fsh = 0i64;
                        if pdf_adjust >= 2 {
                            let (pre_fst, pre_fsh, _) = list_font_expansion(
                                &self.eqtb,
                                &dc.pre_break,
                                prev_exp_char,
                                None,
                                pdf_adjust == 2,
                                lua_mode,
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
                                pdf_adjust == 2,
                                lua_mode,
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
                    Node::NativeGlyphRun { width, .. } => (*width as i64, [0; 4], [0; 4], 0, 0),
                    // math-on/off nodes carry \mathsurround
                    Node::MathKern(k, 1.., _) => (*k as i64, [0; 4], [0; 4], 0, 0),
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
            while f < n && is_prunable::<LUA>(&list[f]) {
                f += 1;
            }
            f.min(n)
        };

        let protrude_chars =
            self.eqtb.int_params[crate::prim::IntParam::PdfProtrudeChars.idx() as usize];
        let start_left_prot = if lua_mode {
            if protrude_chars > 1 {
                crate::luaexp::break_left_pw(&self.eqtb, list, 0, &lb_dead_mask)
            } else {
                0
            }
        } else {
            find_protchar_left(list, &self.eqtb, protrude_chars)
        };
        // luatex `local_par` nodes: the state in force at each candidate
        // (`internal_left_box_width` etc. when `try_break` is called)
        let lua_lp = LUA
            && list.iter().any(|n| matches!(n, Node::Whatsit(WhatIt::LocalPar(_), _)));
        let mut last_lp: Vec<u32> = Vec::new();
        if lua_lp {
            last_lp.reserve(n + 1);
            let mut cur = NO_LOCAL_PAR;
            for (i, nd) in list.iter().enumerate() {
                last_lp.push(cur);
                if matches!(nd, Node::Whatsit(WhatIt::LocalPar(_), _)) {
                    cur = i as u32;
                }
            }
            last_lp.push(cur);
        }
        let init_lp = if lua_lp && matches!(list.first(), Some(Node::Whatsit(WhatIt::LocalPar(_), _))) {
            0
        } else {
            NO_LOCAL_PAR
        };
        // (left box width, right box width) of a local_par state
        let lp_widths = |lp: u32| -> (i64, i64) {
            match list.get(lp as usize) {
                Some(Node::Whatsit(WhatIt::LocalPar(p), _)) => (i64::from(p.left_width), i64::from(p.right_width)),
                _ => (0, 0),
            }
        };
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
            // (net of the left local box, which eats into the line too)
            left_prot: start_left_prot - if lua_lp { lp_widths(init_lp).0 as i32 } else { 0 },
            prev: None,
            ratio: 0,
            short: 0,
            glue: 0,
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
                let cand_lp = if lua_lp { last_lp[cand] } else { NO_LOCAL_PAR };
                // luatex: the right local box eats into every line ending here
                let (cand_left_w, cand_right_w) = if lua_lp { lp_widths(cand_lp) } else { (0, 0) };
                let bg_w_cand = bg_w + cand_right_w;
                let mut champions: HashMap<(i32, usize), (i64, Rc<ActiveNode>, i32, i64, i64)> =
                    HashMap::new();
                let mut idx = 0usize;
                while idx < actives.len() {
                    let a = actives[idx].clone();
                    let is_only = actives.len() == 1;
                    let width = endw - a.start_w + bg_w_cand;
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
                    let right_prot = if lua_mode {
                        if protrude_chars > 1 && cand < n {
                            crate::luaexp::break_right_pw(&self.eqtb, list, a.pos, cand, $is_disc, &lb_dead_mask)
                        } else {
                            0
                        }
                    } else if protrude_chars > 0 && cand < n {
                        if $is_disc {
                            if let Some(Node::Disc(dc)) = list.get(cand) {
                                dc.pre_break
                                    .iter()
                                    .rev()
                                    .chain(list[..cand].iter().rev())
                                    .find_map(|n| match n {
                                        Node::Char { font, c, .. } | Node::Ligature { font, c, .. } => {
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
                                    Node::Char { font, c, .. } | Node::Ligature { font, c, .. } => {
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
                    let cur_ratio = if pdf_adjust >= 2 && shortfall > 0 && font_st > 0 {
                        let raw_ratio = (crate::boxes::divide_scaled(shortfall, font_st, 3).0)
                            .clamp(0, 1000) as i32;
                        let line_st_steps = {
                            let mut steps = 0i64;
                            for node in &list[a.pos..cand.min(n)] {
                                if let Node::Char { font, .. } | Node::Ligature { font, .. } = node
                                {
                                    let ex = &self.eqtb.expand[*font as usize];
                                    if ex.step > 0 && ex.stretch != 0 {
                                        let r = self.eqtb.expand[ex.stretch as usize].ratio;
                                        if r > 0 {
                                            steps = (r / ex.step) as i64;
                                            break;
                                        }
                                    }
                                }
                            }
                            if steps == 0 {
                                stretch_steps
                            } else {
                                steps
                            }
                        };
                        shortfall = if font_st > shortfall {
                            if line_st_steps > 0 {
                                (font_st / line_st_steps) / 2
                            } else {
                                0
                            }
                        } else {
                            shortfall - font_st
                        };
                        raw_ratio
                    } else if pdf_adjust >= 2 && shortfall < 0 && font_sh > 0 {
                        let raw_ratio = (crate::boxes::divide_scaled(shortfall, font_sh, 3).0)
                            .clamp(-1000, 0) as i32;
                        let line_sh_steps = {
                            let mut steps = 0i64;
                            for node in &list[a.pos..cand.min(n)] {
                                if let Node::Char { font, .. } | Node::Ligature { font, .. } = node
                                {
                                    let ex = &self.eqtb.expand[*font as usize];
                                    if ex.step > 0 && ex.shrink != 0 {
                                        let r = -self.eqtb.expand[ex.shrink as usize].ratio;
                                        if r > 0 {
                                            steps = (r / ex.step) as i64;
                                            break;
                                        }
                                    }
                                }
                            }
                            if steps == 0 {
                                shrink_steps
                            } else {
                                steps
                            }
                        };
                        shortfall = if font_sh > -shortfall {
                            if line_sh_steps > 0 {
                                -(font_sh / line_sh_steps) / 2
                            } else {
                                0
                            }
                        } else {
                            shortfall + font_sh
                        };
                        raw_ratio
                    } else {
                        0
                    };
                    // etex.ch: with \lastlinefit, `found` keeps the
                    // last-line adjustment `g`; otherwise it is the
                    // line's finite stretch or shrink
                    let mut llf_found: Option<i64> = None;
                    let (b, fit) = if shortfall == 0 {
                        (0, DECENT)
                    } else if shortfall > 0 {
                        // stretching
                        if dst[1] != 0 || dst[2] != 0 || dst[3] != 0 {
                            let mut r = (0, DECENT); // infinite stretch
                            if let Some(llf) = last_line_fit {
                                match last_line_badness(llf, &a, shortfall, &dst, &dsh)
                                    .filter(|_| cand == n)
                                {
                                    Some((bb, fit, g)) => {
                                        llf_found = Some(g);
                                        r = (bb, fit);
                                    }
                                    None => shortfall = 0,
                                }
                            }
                            r
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
                    // etex.ch <Adjust the additional data for last line>
                    let llf_g = match llf_found {
                        Some(g) => g,
                        None if last_line_fit.is_some() => {
                            if cand == n {
                                shortfall = 0;
                            }
                            if shortfall > 0 {
                                dst[0]
                            } else if shortfall < 0 {
                                dsh[0]
                            } else {
                                0
                            }
                        }
                        None => 0,
                    };
                    let llf_short = if last_line_fit.is_some() { shortfall } else { 0 };

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
                            Some((best_d, ..)) if *best_d < d => {}
                            _ => {
                                champions
                                    .insert(key, (d, a.clone(), cur_ratio, llf_short, llf_g));
                            }
                        }
                    }
                    let hopeless = b > INF_BAD;
                    if hopeless || forced {
                        if final_pass && champions.is_empty() && is_only {
                            champions.insert(
                                (a.line + 1, DECENT),
                                (a.demerits, a.clone(), 0, llf_short, llf_g),
                            );
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
                for (key, (d, ..)) in &champions {
                    let e = group_min.entry(key.0).or_insert(AWFUL_BAD);
                    if *d < *e {
                        *e = *d;
                    }
                }
                let mut keys: Vec<_> = champions
                    .iter()
                    .filter(|((cls, _), (d, ..))| {
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
                    let (d, prev, ratio, short, glue) = &champions[&key];
                    let (start_w, start_st, start_sh, start_fst, start_fsh) = start_state::<LUA>(
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
                    let left_prot = if lua_mode {
                        if protrude_chars > 1 {
                            crate::luaexp::break_left_pw(&self.eqtb, list, cand, &lb_dead_mask)
                        } else {
                            0
                        }
                    } else if protrude_chars >= 2 {
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
                            left_prot: left_prot - cand_left_w as i32,
                            prev: Some(prev.clone()),
                            ratio: *ratio,
                            short: *short,
                            glue: *glue,
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
        let break_after_dir = LUA && self.eqtb.int_params[IntParam::BreakAfterDirMode.idx() as usize] == 1;
        let mut i = 0usize;
        let mut auto_breaking = true;
        while i < n {
            match &list[i] {
                Node::MathKern(_, kind, _) => {
                    // etex.ch: `if subtype(cur_p)<L_code then
                    // auto_breaking:=odd(subtype(cur_p))` — text-direction
                    // nodes leave it alone
                    if *kind < crate::boxes::LR_KIND_MIN {
                        auto_breaking = crate::boxes::math_end_lr(*kind);
                    }
                    // tex.web §866: math_node does kern_break after setting
                    // auto_breaking, so only a math node followed by glue
                    // (outside a formula) is a legal breakpoint
                    if auto_breaking && i + 1 < n && matches!(list[i + 1], Node::Glue(_, _)) {
                        consider!(i, false, 0, BreakType::Unhyphenated, false, cum_w[i]);
                    }
                }
                Node::Penalty(p, _) => {
                    if *p < INF_PENALTY {
                        let forced = *p <= EJECT_PENALTY;

                        consider!(i, false, *p, BreakType::Unhyphenated, forced, cum_w[i]);
                    }
                }
                Node::Glue(_, _) => {
                    let legal = auto_breaking
                        && i > 0
                        && !matches!(
                            list[i - 1],
                            Node::Glue(_, _)
                                | Node::Penalty(_, _)
                                | Node::ExplicitKern(_, _)
                                | Node::MathKern(..)
                        )
                        // luatex precedes_break: local_par and dir nodes
                        // are no break context (a dir node is with
                        // \breakafterdirmode=1)
                        && (!LUA
                            || match &list[i - 1] {
                                Node::Whatsit(WhatIt::LocalPar(_), _) => false,
                                Node::Whatsit(WhatIt::Dir { .. }, _) => break_after_dir,
                                _ => true,
                            });
                    if legal {
                        consider!(i, false, 0, BreakType::Unhyphenated, false, cum_w[i]);
                    }
                }
                Node::ExplicitKern(k, _) => {
                    // tex's kern_break: explicit kern followed by glue; the
                    // kern itself is zeroed at the line end
                    if auto_breaking && i + 1 < n && matches!(list[i + 1], Node::Glue(_, _)) {
                        consider!(i, false, 0, BreakType::Unhyphenated, false, cum_w[i]);
                        let _ = k;
                    }
                }
                Node::Disc(dc) => {
                    let pen = if dc.penalty != crate::boxes::DISC_PENALTY_TEX {
                        dc.penalty
                    } else if !dc.pre_break.is_empty() {
                        params.hyphen_penalty
                    } else {
                        params.ex_hyphen_penalty
                    };
                    let endw = cum_w[i] + disc_list_width(&self.eqtb, &dc.pre_break);
                    // luatex: syllable discretionaries (subtype > automatic)
                    // only break in the second pass
                    if second_pass || dc.subtype <= 2 {
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
    fn build_lines<const LUA: bool>(
        &mut self,
        mut list: NodeList,
        params: &ParaParams,
        end: Rc<ActiveNode>,
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
        // luatex `local_par` state of every break of the chain: the last
        // local_par node before it (the paragraph's first node for the
        // initial break) - what `line_break` keeps in `internal_*`
        let mut chain_lp: Vec<Option<crate::boxes::LocalPar>> = Vec::new();
        if LUA {
            let lps: Vec<usize> = list
                .iter()
                .enumerate()
                .filter(|(_, n)| matches!(n, Node::Whatsit(WhatIt::LocalPar(_), _)))
                .map(|(i, _)| i)
                .collect();
            for (k, b) in chain.iter().enumerate() {
                let at = if k == 0 {
                    lps.first().copied().filter(|&i| i == 0)
                } else {
                    lps.iter().rev().copied().find(|&i| i < b.pos)
                };
                chain_lp.push(match at.map(|i| &list[i]) {
                    Some(Node::Whatsit(WhatIt::LocalPar(p), _)) => Some((**p).clone()),
                    _ => None,
                });
            }
        }

        // etex.ch post_line_break: `LR_ptr:=LR_save` ... `LR_save:=LR_ptr`;
        // with TeXXeT every line reopens (closes) the text-direction and
        // \beginM/\endM segments still open at its start (end)
        let texxet = self.eqtb.int_params[IntParam::TeXXeTEnabled.idx() as usize] > 0;
        let lr_key = self.saved_lists.len();
        let mut lr: Vec<u8> = self.lr_save_take(lr_key);
        let mut lines: NodeList = Vec::new();
        // luatex post_line_break `dir_ptr`: the text directions still open
        // at the end of a line are closed there and reopened on the next
        let lua_dirs = LUA;
        let mut dir_stack: Vec<u8> = Vec::new();
        let mut i = 0usize;
        let mut pending_post: Option<crate::boxes::DiscNode> = None;
        let mut dead_until = 0usize; // nodes in [i, dead_until) are dead
                                     // chain[0] is the synthetic paragraph start (pos 0, line 0)
        let total_lines = chain.len() - 1;
        // luatex packs and appends line by line: hold the hpack_quality
        // calls back until `fill_line_interline` reaches the line
        self.lua_par_lines.defer = self.engine_kind == crate::engine::EngineKind::LuaTeX
            && self.cb_defined(crate::lua_callbacks::Cb::HpackQuality);
        self.lua_par_lines.quality.clear();
        for (li, bp) in chain.iter().skip(1).enumerate() {
            self.lua_par_lines.ordinal = li;
            let j = bp.pos.min(list.len());
            let mut seg: NodeList = Vec::new();
            let mut nat_w = 0i64;
            for &d in &dir_stack {
                seg.push(Node::Whatsit(WhatIt::Dir { dir: d, cancel: false, level: 0 }, crate::boxes::Attr::NONE));
            }
            // "Insert LR nodes at the beginning of the current line"
            if texxet && !lr.is_empty() {
                seg.extend(lr.iter().map(|k| Node::MathKern(0, k - 1, self.eqtb.cur_attr)));
                self.texxet_nodes = true;
            }
            if let Some(dc) = pending_post.take() {
                for nn in dc.post_break {
                    push_dims(&self.eqtb, nn, &mut seg, &mut nat_w);
                }
            }
            // gather live nodes strictly before the breakpoint node
            let mut post_adj: NodeList = Vec::new();
            let mut pre_adj: NodeList = Vec::new();
            while i < j {
                if i < dead_until {
                    i += 1;
                    continue;
                }
                if let Node::VAdjust(items, _) = &mut list[i] {
                    // tex.web §866 post_line_break: adjustment material joins the
                    // vertical list right after the line box containing it
                    post_adj.append(items);
                    i += 1;
                    continue;
                }
                if let Node::PreAdjust(items, _) = &mut list[i] {
                    // pdftex.web: `\vadjust pre` material goes in front of the line
                    pre_adj.append(items);
                    i += 1;
                    continue;
                }
                if matches!(
                    &list[i],
                    Node::Ins { .. } | Node::Mark { .. } | Node::Adj(_, _)
                ) {
                    // tex.web §866 post_line_break: ins, mark, and adjust nodes
                    // migrate from the line's hlist to the vertical list right
                    // after the line box containing them
                    post_adj.push(std::mem::replace(&mut list[i], Node::Kern(0, crate::boxes::Attr::NONE)));
                    i += 1;
                    continue;
                }
                let skip = match &list[i] {
                    // an unbroken disc renders its no_break text and swallows
                    // the replaced nodes (ligature splits)
                    Node::Disc(dc) => dc.replace_count,
                    _ => 0,
                };
                let node = std::mem::replace(&mut list[i], Node::Kern(0, crate::boxes::Attr::NONE));
                if let (true, Node::Whatsit(WhatIt::Dir { dir, cancel, .. }, _)) = (lua_dirs, &node) {
                    if !cancel {
                        dir_stack.push(*dir);
                    } else if dir_stack.last() == Some(dir) {
                        dir_stack.pop();
                    }
                }
                if let (true, Node::MathKern(_, kind @ 1.., _)) = (texxet, &node) {
                    crate::texxet::lr_adjust(&mut lr, *kind);
                }
                push_dims(&self.eqtb, node, &mut seg, &mut nat_w);
                i += 1 + skip;
            }
            let last = bp.pos >= list.len();
            let mut break_disc: Option<crate::boxes::DiscNode> = None;
            let mut broke_at_disc = false;
            // tex.web §881: a math-node break stays on the line with width 0
            // (etex.ch adjusts the LR stack there); math nodes pruned from
            // the next line's start adjust it only after this line's LR end
            // nodes are inserted
            let mut break_math: Option<Node> = None;
            let mut pruned_lr: Vec<u8> = Vec::new();
            let mut break_glue_attr: Option<crate::boxes::Attr> = None;
            if !last {
                match &mut list[j] {
                    Node::Disc(dc) => {
                        broke_at_disc = true;
                        // line ends with the pre-break text
                        let mut dc = std::mem::replace(
                            dc,
                            crate::boxes::DiscNode::new(Vec::new(), Vec::new(), Vec::new(), 0),
                        );
                        // tex.web §882: the (now empty) disc node stays in the
                        // line, followed by the transplanted pre-break list
                        // (luatex puts the transplanted pre-break list in
                        // front of the node)
                        let lua_order = self.engine_kind == crate::engine::EngineKind::LuaTeX;
                        if !lua_order {
                            seg.push(Node::Disc(crate::boxes::DiscNode::new(Vec::new(), Vec::new(), Vec::new(), 0)));
                        }
                        for nn in std::mem::take(&mut dc.pre_break) {
                            push_dims(&self.eqtb, nn, &mut seg, &mut nat_w);
                        }
                        if lua_order {
                            seg.push(Node::Disc(crate::boxes::DiscNode::new(Vec::new(), Vec::new(), Vec::new(), 0)));
                        }
                        if dc.post_break.is_empty() {
                            // post_line_break prunes the next line start
                            i = j + 1 + dc.replace_count;
                            while i < list.len() && is_prunable::<LUA>(&list[i]) {
                                if let Node::MathKern(_, kind @ 1.., _) = list[i] {
                                    pruned_lr.push(kind);
                                }
                                i += 1;
                            }
                            dead_until = 0;
                        } else {
                            i = j + 1;
                            dead_until = j + 1 + dc.replace_count;
                            break_disc = Some(dc);
                        }
                    }
                    Node::Glue(_, _)
                    | Node::Penalty(_, _)
                    | Node::ExplicitKern(_, _)
                    | Node::MathKern(..) => {
                        // glue becomes \rightskip at packing; tex.web §881
                        // keeps a penalty node as is and a kern or math node
                        // with width 0 (a math node also adjusts the LR
                        // stack); discardables at the start of the next line
                        // are pruned
                        if matches!(list[j], Node::Glue(..)) {
                            // luatex post_line_break turns the glue node itself
                            // into the \rightskip glue: it keeps its attributes
                            break_glue_attr = Some(list[j].attr());
                        }
                        match &list[j] {
                            Node::Penalty(p, a) => break_math = Some(Node::Penalty(*p, *a)),
                            Node::ExplicitKern(_, a) => break_math = Some(Node::ExplicitKern(0, *a)),
                            Node::MathKern(_, kind @ 1.., a) => {
                                if texxet {
                                    crate::texxet::lr_adjust(&mut lr, *kind);
                                }
                                break_math = Some(Node::MathKern(0, *kind, *a));
                            }
                            _ => {}
                        }
                        i = j + 1;
                        while i < list.len() && is_prunable::<LUA>(&list[i]) {
                            if let Node::MathKern(_, kind @ 1.., _) = list[i] {
                                pruned_lr.push(kind);
                            }
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
            // luatex: the left box of the break before and the right box of
            // the break after the line (copies, as the line is packed)
            let (left_box, right_box) = if lua_dirs {
                (
                    chain_lp[li].as_ref().filter(|p| !p.left.is_empty()).map(|p| p.left.clone()),
                    chain_lp[li + 1].as_ref().filter(|p| !p.right.is_empty()).map(|p| p.right.clone()),
                )
            } else {
                (None, None)
            };
            if let Some(lb) = left_box {
                // after the empty \parindent box of the first line
                let at = if li == 0 && matches!(seg.get(1), Some(Node::Box { list, .. }) if list.is_empty()) {
                    2
                } else {
                    0
                };
                seg.splice(at..at, lb);
            }
            let protrude_chars =
                self.eqtb.int_params[crate::prim::IntParam::PdfProtrudeChars.idx() as usize];
            if protrude_chars > 0 && self.engine_kind == crate::engine::EngineKind::LuaTeX {
                // luatex post_line_break: the right margin kern goes before
                // the break glue (before \parfillskip on the last line), the
                // left one in front of the first node of the line
                let ins = if (last && matches!(seg.last(), Some(Node::Glue(..)))) || broke_at_disc {
                    seg.len() - 1
                } else {
                    seg.len()
                };
                if ins > 0 {
                    if let Some((g, attr)) = crate::luaexp::find_protchar_right(&seg, 0, ins - 1, &[]) {
                        let pw = crate::luaexp::char_pw(&self.eqtb, g, false);
                        if pw != 0 {
                            seg.insert(ins, Node::MarginKern { side: 1, width: -pw, c: g.c, font: g.font, ex: 0, attr });
                        }
                    }
                }
                if let Some((g, _)) = crate::luaexp::find_protchar_left(&seg, 0, false, &[]) {
                    let pw = crate::luaexp::char_pw(&self.eqtb, g, true);
                    if pw != 0 {
                        let attr = seg[0].attr();
                        seg.insert(0, Node::MarginKern { side: 0, width: -pw, c: g.c, font: g.font, ex: 0, attr });
                    }
                }
            } else if protrude_chars > 0 {
                let left_cand = seg.iter().find_map(|n| match n {
                    Node::Char { font, c, attr } | Node::Ligature { font, c, attr, .. } => Some((*font, *c, *attr)),
                    Node::Glue(_, _)
                    | Node::Penalty(_, _)
                    | Node::Kern(_, _)
                    | Node::ExplicitKern(_, _)
                    | Node::AccentKern(0, _) | Node::ItalicKern(0, _)
                    | Node::Whatsit(_, _) => None,
                    // pdftex cp_skipable: zero-width math nodes; only the
                    // TeXXeT \beginM..\endR kinds are skipped here
                    Node::MathKern(0, crate::boxes::BEGIN_M.., _) => None,
                    Node::Box {
                        w: 0,
                        h: 0,
                        d: 0,
                        list,
                        ..
                    } if list.is_empty() => None,
                    _ => Some((0, 0, crate::boxes::Attr::NONE)),
                });
                if let Some((f, c, lattr)) = left_cand {
                    if c != 0 {
                        let pw = char_protrusion_width(&self.eqtb, protrude_chars, f, c, true);
                        if pw != 0 {
                            seg.insert(
                                0,
                                Node::MarginKern {
                                    side: 0,
                                    width: -pw,
                                    font: f,
                                    c: u32::from(c),
                                    ex: 0, attr: lattr,
                                },
                            );
                        }
                    }
                }
                if let Some((f, c, rattr)) = seg.iter().rev().filter(|_| right_box.is_none()).find_map(|n| match n {
                    Node::Char { font, c, attr } | Node::Ligature { font, c, attr, .. } => Some((*font, *c, *attr)),
                    _ => None,
                }) {
                    let pw = char_protrusion_width(&self.eqtb, protrude_chars, f, c, false);
                    if pw != 0 {
                        seg.push(Node::MarginKern {
                            side: 1,
                            width: -pw,
                            font: f,
                            c: u32::from(c),
                            ex: 0, attr: rattr,
                        });
                    }
                }
            }
            seg.extend(break_math);
            if lua_dirs {
                let mut line_end: NodeList = Vec::new();
                let attr = seg.last().map_or(self.eqtb.cur_attr, Node::attr);
                line_end.extend(
                    dir_stack
                        .iter()
                        .rev()
                        .map(|&d| Node::Whatsit(WhatIt::Dir { dir: d, cancel: true, level: 0 }, attr)),
                );
                line_end.extend(right_box.into_iter().flatten());
                if !line_end.is_empty() {
                    // before the break glue (\parfillskip on the last line)
                    let at = if last && matches!(seg.last(), Some(Node::Glue(..))) { seg.len() - 1 } else { seg.len() };
                    seg.splice(at..at, line_end);
                }
            }
            // "Insert LR nodes at the end of the current line"
            if texxet && !lr.is_empty() {
                seg.extend(lr.iter().rev().map(|&k| Node::MathKern(0, k, self.eqtb.cur_attr)));
                self.texxet_nodes = true;
            }
            if texxet {
                for kind in pruned_lr {
                    crate::texxet::lr_adjust(&mut lr, kind);
                }
            }
            let mut inner: NodeList = Vec::with_capacity(seg.len() + 2);
            let skip_attr = |n: Option<&Node>, dflt: crate::boxes::Attr| n.map_or(dflt, Node::attr);
            let left_attr = skip_attr(seg.first(), self.eqtb.cur_attr);
            let right_attr = break_glue_attr.unwrap_or_else(|| skip_attr(seg.last(), self.eqtb.cur_attr));
            // tex.web §887: \leftskip glue only when it is not zero_glue
            if !params.left_skip.is_zero_glue() {
                inner.push(Node::Glue(params.left_skip, left_attr));
            }
            inner.extend(seg);
            inner.push(Node::Glue(params.right_skip, right_attr));
            let mut r = crate::boxes::hpack_expand(self, inner, target, crate::boxes::HBOX);
            // tex.web §17436: the parshape indent is the line box's
            // shift_amount, never an in-line kern (a kern would overshoot
            // the packed width, which already excludes the indent).
            // tex.web §889: hpack (with its under/overfull report, which
            // names the paragraph's lines) runs before the line's
            // shift_amount is set
            let source = self.current_token_source_mark();
            let begin_line = self.mode_line();
            let saved_begin = std::mem::replace(&mut self.pack_begin_line, begin_line);
            self.report_pack_warnings_at(&mut r, source);
            self.pack_begin_line = saved_begin;
            if let Node::Box { shift, subtype, .. } = &mut r.node {
                *subtype = crate::boxes::list_subtype::LINE;
                if indent != 0 {
                    *shift = indent;
                }
            }
            // pdftex.web <Append the new box to the current vertical list>:
            // \pdfeachlineheight/depth, then \pdffirstlineheight and
            // \pdflastlinedepth, each unless it equals \pdfignoreddimen
            {
                let dim = |p: DimParam| self.eqtb.dim_params[p.idx() as usize];
                let ignored = dim(DimParam::PdfIgnoredDimen);
                let set = |v: i32| (v != ignored).then_some(v);
                let each_h = set(dim(DimParam::PdfEachLineHeight));
                let each_d = set(dim(DimParam::PdfEachLineDepth));
                let first_h = set(dim(DimParam::PdfFirstLineHeight)).filter(|_| li == 0);
                let last_d = set(dim(DimParam::PdfLastLineDepth)).filter(|_| li + 1 == total_lines);
                if let Node::Box { h, d, .. } = &mut r.node {
                    if let Some(v) = first_h.or(each_h) {
                        *h = v;
                    }
                    if let Some(v) = last_d.or(each_d) {
                        *d = v;
                    }
                }
            }
            // interline glue placeholder (page builder owns real baseline
            // spacing between line boxes)
            if !lines.is_empty() {
                lines.push(Node::Glue(Glue::zero(), self.eqtb.cur_attr));
            }
            // pdftex.web <Append the new box to the current vertical list>:
            // the `\vadjust pre` material precedes the box (and its interline
            // glue, which is measured against the previous line)
            if !pre_adj.is_empty() {
                lines.push(Node::VAdjust(pre_adj, self.eqtb.cur_attr));
            }
            lines.push(r.node);
            if !post_adj.is_empty() {
                lines.push(Node::VAdjust(post_adj, self.eqtb.cur_attr));
            }
            // tex.web §17438: interline penalty after every line but the
            // last — interlinepenalty, plus clubpenalty after line 1, plus
            // the (display)widow penalty before the last line, plus
            // brokenpenalty when the line ended at a discretionary.
            if li + 1 != total_lines {
                let line_no = params.prev_graf.max(0) as usize + li + 1;
                // luatex: \localinterlinepenalty, when set, replaces
                // \interlinepenalty
                let local = if lua_dirs { chain_lp[li + 1].as_ref() } else { None };
                let inter = match local {
                    Some(p) if p.pen_inter != 0 => p.pen_inter,
                    _ => params.inter_line_penalty,
                };
                let mut pen = penalty_shape_at(&params.penalty_shapes[0], line_no, inter);
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
                    pen += match local {
                        Some(p) if p.pen_broken != 0 => p.pen_broken,
                        _ => params.broken_penalty,
                    };
                }
                if pen != 0 {
                    lines.push(Node::Penalty(pen, self.eqtb.cur_attr));
                }
            }
            if let Some(dc) = break_disc {
                pending_post = Some(dc);
            }
        }
        self.lr_save_store(lr_key, lr);
        self.lua_par_lines.defer = false;
        if !self.lua_par_lines.hold {
            self.lua_flush_pack_quality();
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
                Some(Node::Penalty(p, _)) => Some(*p),
                Some(Node::Glue(_, _) | Node::Leaders { .. }) if prev_non_discardable => Some(0),
                Some(Node::Kern(_, _) | Node::ExplicitKern(_, _) | Node::AccentKern(_, _) | Node::ItalicKern(_, _))
                    if matches!(list.get(i + 1), Some(Node::Glue(_, _) | Node::Leaders { .. })) =>
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
                Node::Glue(g, _) | Node::Leaders { glue: g, .. } => {
                    stretch[(g.stretch_order as usize).min(3)] += g.stretch as i64;
                    shrink += g.shrink as i64;
                    if g.shrink_order != 0 && g.shrink != 0 {
                        if self.eqtb.int_params[IntParam::IgnorePrimitiveError.idx() as usize] & 1
                            != 0
                        {
                            self.lua_ignored_error("Infinite glue shrinkage found in box being split");
                        } else {
                            self.error("Infinite glue shrinkage found in box being split");
                        }
                        g.shrink_order = 0;
                    }
                    t += d + g.width as i64;
                    d = 0;
                }
                Node::Kern(k, _) | Node::ExplicitKern(k, _) | Node::AccentKern(k, _) | Node::ItalicKern(k, _) => {
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
                    | Node::Whatsit(_, _)
                    | Node::Adj(_, _)
            );
        }
        let mut rest = list.split_off(best_split);
        let top = list;
        // prune_page_top keeps marks/whatsits/inserts while removing
        // discardable nodes before the first box, then inserts splittopskip.
        let mut seen_box = false;
        let mut snaps = 0;
        rest.retain(|n| {
            if seen_box {
                return true;
            }
            match n {
                Node::Box { .. } | Node::Rule { .. } => {
                    seen_box = true;
                    true
                }
                Node::Glue(_, _)
                | Node::Leaders { .. }
                | Node::Penalty(_, _)
                | Node::Kern(_, _)
                | Node::ExplicitKern(_, _)
                | Node::AccentKern(_, _) | Node::ItalicKern(_, _) => false,
                Node::Whatsit(
                    crate::boxes::WhatIt::PdfSnapY(_) | crate::boxes::WhatIt::PdfSnapYComp(_),
                _) => {
                    snaps += 1;
                    false
                }
                _ => true,
            }
        });
        for _ in 0..snaps {
            self.report_discarded_snap();
        }
        if let Some((i, height)) = rest.iter().enumerate().find_map(|(i, n)| match n {
            Node::Box { h, .. } | Node::Rule { height: h, .. } => Some((i, *h)),
            _ => None,
        }) {
            // tex.web §968 new_skip_param(split_top_skip_code)
            let mut skip = self.eqtb.glue_params
                [crate::prim::GlueParam::SplitTopSkip.idx() as usize]
                .fresh();
            skip.subtype = crate::boxes::glue_subtype::SPLIT_TOP_SKIP;
            skip.width = (skip.width - height).max(0);
            rest.insert(i, Node::Glue(skip, self.eqtb.cur_attr));
        }
        let mut seen = std::collections::HashSet::new();
        for node in &top {
            if let Node::Mark { class, tokens, .. } = node {
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
        // luatex vsplit: `filtered_vpackage(q, h, exactly, split_max_depth,
        // split_off_group)` runs `vpack_filter` and reports the packing
        let lua = self.engine_kind == crate::engine::EngineKind::LuaTeX;
        let top = if lua {
            self.lua_pack_filter(
                crate::lua_callbacks::Cb::VpackFilter,
                "vpack filter",
                "split_off",
                target,
                true,
                Some(smd as i32),
                Some("TLT"),
                top,
            )
        } else {
            top
        };
        let mut r = crate::boxes::vpack_add_md(
            top,
            Some(target),
            false,
            crate::boxes::VBOX,
            &self.eqtb,
            smd as i32,
        );
        if lua {
            self.report_pack_warnings(&mut r);
        }
        self.last_badness = r.badness;
        self.vsplat_remainder = Some(rest);
        Some(r.node)
    }
}

/// hu value of an implicit boundary (tex.web non_char)
const NON_CHAR: u16 = 256;

/// hyphenation inputs shared by the words of one paragraph
struct HyphCtx<'a> {
    trie: &'a crate::hyphen::Trie,
    /// the language's saved \lccode table (eTeX \savinghyphcodes)
    codes: Option<&'a [u8; 256]>,
    lc_code: &'a [u8],
    lh: usize,
    rh: usize,
    uc_hyph: bool,
    /// `lang.hyphenationmin`: words shorter than this stay whole
    min_len: usize,
    /// `lang.prehyphenchar` when a Lua program set it for the language
    pre_hyphen: Option<i32>,
}

impl HyphCtx<'_> {
    fn lc(&self, c: u8) -> u8 {
        match self.codes {
            Some(codes) => codes[c as usize],
            None => self.lc_code.get(c as usize).copied().unwrap_or(0),
        }
    }
}

fn hu_char(v: u16) -> Option<u8> {
    u8::try_from(v).ok()
}

/// tex.web §905-§918: the word hu[1..=hn] of font `hf` (hu[0] the
/// character or ligature before it, or NON_CHAR for the left boundary),
/// its hyphen positions `hyf`, and the translation `hold` that
/// `reconstitute` produces one cut prefix at a time
struct Reconstitute<'a> {
    font: &'a crate::tfm::Font,
    hf: FontId,
    hu: Vec<u16>,
    hyf: Vec<u8>,
    /// components of hu[0] (`init_list`), a ligature if `init_lig`
    init_list: [u8; 3],
    init_len: usize,
    init_lig: bool,
    init_lft: bool,
    hyphen_passed: usize,
    hold: NodeList,
    /// the attribute list of the word's first letter, which every node the
    /// reconstitution makes carries
    attr: crate::boxes::Attr,
}

impl Reconstitute<'_> {
    fn char_node(&self, c: u8) -> Node {
        Node::Char { c, font: self.hf, attr: self.attr }
    }

    /// set_cur_r: (cur_r, cur_rh) for the cursor after position `j`
    fn cur_r_at(&self, j: usize, n: usize, bchar: Option<u8>, hchar: Option<u8>) -> (Option<u8>, Option<u8>) {
        let cur_r = if j < n { hu_char(self.hu[j + 1]) } else { bchar };
        let cur_rh = if self.hyf[j] % 2 == 1 { hchar } else { None };
        (cur_r, cur_rh)
    }

    /// wrap_lig: the characters after `cur_q` become ligature `c`
    fn pack_lig(&mut self, cur_q: usize, c: u8, subtype: u8) {
        let mut letters = [0u8; 3];
        let mut n = 0;
        for node in self.hold.drain(cur_q..) {
            if let Node::Char { c, .. } = node {
                if n < 3 {
                    letters[n] = c;
                    n += 1;
                }
            }
        }
        self.hold.push(Node::Ligature {
            c,
            font: self.hf,
            lig_width: self.font.char_width(c),
            lig_height: self.font.char_height(c),
            lig_depth: self.font.char_depth(c),
            letters,
            n_letters: n as u8,
            subtype, attr: self.attr,
        });
    }

    /// §906 reconstitute(j, n, bchar, hchar): translate the cut prefix of
    /// hu[j..=n] into `hold`; returns its last index and sets
    /// `hyphen_passed` to the first hyphen position it ran across
    fn run(&mut self, mut j: usize, n: usize, mut bchar: Option<u8>, mut hchar: Option<u8>) -> usize {
        use crate::build::{lig_kern_step, LigKernOp};
        self.hyphen_passed = 0;
        self.hold.clear();
        let mut w = 0;
        // §908
        let mut cur_l = hu_char(self.hu[j]);
        let mut cur_q = 0;
        let mut lig_present = false;
        let mut lft_hit = false;
        let mut rt_hit = false;
        if j == 0 {
            lig_present = self.init_lig;
            if lig_present {
                lft_hit = self.init_lft;
            }
            for k in 0..self.init_len {
                let node = self.char_node(self.init_list[k]);
                self.hold.push(node);
            }
        } else if let Some(c) = cur_l {
            let node = self.char_node(c);
            self.hold.push(node);
        }
        // lig_stack: (character, lig_ptr) with the top last
        let mut stack: Vec<(u8, Option<u8>)> = Vec::new();
        let (mut cur_r, mut cur_rh) = self.cur_r_at(j, n, bchar, hchar);
        loop {
            // §909: a lig/kern with the hyphen, then with cur_r
            let mut done = true;
            if let Some(h) = cur_rh.take() {
                if lig_kern_step(self.font, cur_l, h).is_some() {
                    self.hyphen_passed = j;
                    hchar = None;
                }
            }
            if let Some(step) = cur_r.and_then(|r| lig_kern_step(self.font, cur_l, r)) {
                if hchar.is_some() && self.hyf[j] % 2 == 1 {
                    self.hyphen_passed = j;
                    hchar = None;
                }
                match step {
                    LigKernOp::Kern(k) => w = k,
                    // §911
                    LigKernOp::Lig { op, ch } => {
                        if cur_l.is_none() {
                            lft_hit = true;
                        }
                        if j == n && stack.is_empty() {
                            rt_hit = true;
                        }
                        done = op > 4 && op != 7;
                        match op {
                            1 | 5 => {
                                cur_l = Some(ch);
                                lig_present = true;
                            }
                            2 | 6 => {
                                cur_r = Some(ch);
                                if let Some(top) = stack.last_mut() {
                                    top.0 = ch;
                                } else if j == n {
                                    stack.push((ch, None));
                                    bchar = None;
                                } else {
                                    stack.push((ch, hu_char(self.hu[j + 1])));
                                }
                            }
                            3 => {
                                cur_r = Some(ch);
                                stack.push((ch, None));
                            }
                            7 | 11 => {
                                if lig_present {
                                    let subtype = if std::mem::take(&mut lft_hit) { 2 } else { 0 };
                                    self.pack_lig(cur_q, cur_l.unwrap_or(0), subtype);
                                    lig_present = false;
                                }
                                cur_q = self.hold.len();
                                cur_l = Some(ch);
                                lig_present = true;
                            }
                            _ => {
                                cur_l = Some(ch);
                                lig_present = true;
                                if let Some((_, orig)) = stack.pop() {
                                    if let Some(o) = orig {
                                        let node = self.char_node(o);
                                        self.hold.push(node);
                                        j += 1;
                                    }
                                    match stack.last() {
                                        Some(&(c, _)) => cur_r = Some(c),
                                        None => (cur_r, cur_rh) = self.cur_r_at(j, n, bchar, hchar),
                                    }
                                } else if j == n {
                                    done = true;
                                } else {
                                    let node = self.char_node(cur_r.unwrap_or(0));
                                    self.hold.push(node);
                                    j += 1;
                                    (cur_r, cur_rh) = self.cur_r_at(j, n, bchar, hchar);
                                }
                            }
                        }
                    }
                }
            }
            if !done {
                continue;
            }
            // §910: append the ligature and/or kern
            if lig_present {
                let mut subtype = if std::mem::take(&mut lft_hit) { 2 } else { 0 };
                if rt_hit && stack.is_empty() {
                    subtype += 1;
                    rt_hit = false;
                }
                self.pack_lig(cur_q, cur_l.unwrap_or(0), subtype);
                lig_present = false;
            }
            if w != 0 {
                self.hold.push(Node::Kern(w, self.attr));
                w = 0;
            }
            let Some((c, orig)) = stack.pop() else {
                return j;
            };
            cur_q = self.hold.len();
            cur_l = Some(c);
            lig_present = true;
            if let Some(o) = orig {
                let node = self.char_node(o);
                self.hold.push(node);
                j += 1;
            }
            match stack.last() {
                Some(&(c, _)) => cur_r = Some(c),
                None => (cur_r, cur_rh) = self.cur_r_at(j, n, bchar, hchar),
            }
        }
    }

    /// §913-§918: the reconstituted word hu[j..=hn] with its discretionary
    /// hyphens
    fn hyphenated_word(&mut self, mut j: usize, hn: usize, bchar: Option<u8>, hyf_char: u8) -> NodeList {
        let mut out = NodeList::new();
        let has_hyphen = self.font.char_present(hyf_char);
        let font_bchar = self.font.bchar;
        let left_boundary = crate::build::bchar_label(self.font).is_some();
        loop {
            let mut l = j;
            j = self.run(j, hn, bchar, Some(hyf_char)) + 1;
            if self.hyphen_passed == 0 {
                out.append(&mut self.hold);
                if self.hyf[j - 1] % 2 == 1 {
                    l = j;
                    self.hyphen_passed = j - 1;
                }
            }
            while self.hyphen_passed > 0 {
                // §914: the translation so far is the no-break text
                let mut major = std::mem::take(&mut self.hold);
                let mut i = self.hyphen_passed;
                self.hyf[i] = 0;
                // §915: hu[l..=i] and a hyphen into pre_break
                let mut pre_break = NodeList::new();
                let mut c = 0;
                if has_hyphen {
                    i += 1;
                    c = self.hu[i];
                    self.hu[i] = hyf_char as u16;
                }
                while l <= i {
                    l = self.run(l, i, font_bchar, None) + 1;
                    pre_break.append(&mut self.hold);
                }
                if has_hyphen {
                    self.hu[i] = c;
                    l = i;
                    i -= 1;
                }
                let _ = i;
                // §916: hu[i+1..] into post_break until both branches
                // reach the same position
                let mut post_break = NodeList::new();
                let mut c_loc = 0;
                if left_boundary {
                    l -= 1;
                    c = self.hu[l];
                    c_loc = l;
                    self.hu[l] = NON_CHAR;
                }
                while l < j {
                    loop {
                        l = self.run(l, hn, bchar, None) + 1;
                        if c_loc > 0 {
                            self.hu[c_loc] = c;
                            c_loc = 0;
                        }
                        post_break.append(&mut self.hold);
                        if l >= j {
                            break;
                        }
                    }
                    // §917
                    while l > j {
                        j = self.run(j, hn, bchar, None) + 1;
                        major.append(&mut self.hold);
                    }
                }
                // §918: a discretionary may replace at most 127 nodes
                if major.len() <= 127 {
                    out.push(Node::Disc(
                        crate::boxes::DiscNode::new(pre_break, post_break, major.clone(), major.len())
                            .with_attr(self.attr),
                    ));
                }
                out.append(&mut major);
                self.hyphen_passed = j - 1;
                self.hold.clear();
                if self.hyf[j - 1] % 2 == 0 {
                    break;
                }
            }
            if j > hn {
                return out;
            }
        }
    }
}
/// nodes tex removes at the start of the next line after a non-disc break
fn is_prunable<const LUA: bool>(n: &Node) -> bool {
    matches!(
        n,
        Node::Glue(_, _) | Node::Penalty(_, _) | Node::ExplicitKern(_, _) | Node::MathKern(..)
    ) || (LUA && matches!(n, Node::Whatsit(WhatIt::LocalPar(_), _))) // luatex post_line_break: "weird, in the middle somewhere"
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
        Node::Char { c, font, .. } => fonts.char_width(*font, *c),
        Node::LuaGlyph(g) => crate::boxes::lua_glyph_dims(eqtb, g).0,
        Node::Ligature { lig_width, .. } => *lig_width,
        Node::Glue(g, _) => g.width,
        Node::Kern(k, _) | Node::ExplicitKern(k, _) | Node::AccentKern(k, _) | Node::ItalicKern(k, _) => *k,
        Node::ExKern { width, ex, .. } => *width + *ex,
        Node::Box { w: bw, .. } => *bw,
        Node::Rule { width, .. } => *width,
        Node::NativeGlyphRun { width, .. } => *width,
        Node::MathKern(k, 1.., _) => *k,
        _ => 0,
    };
    *w += wd as i64;
    seg.push(n);
}

fn disc_list_width(eqtb: &crate::eqtb::Eqtb, l: &[Node]) -> i64 {
    let fonts = crate::boxes::eqtb_fonts(eqtb);
    l.iter()
        .map(|nn| match nn {
            Node::Char { c, font, .. } => fonts.char_width(*font, *c) as i64,
            Node::LuaGlyph(g) => crate::boxes::lua_glyph_dims(eqtb, g).0 as i64,
            Node::Ligature { lig_width, .. } => *lig_width as i64,
            Node::Kern(k, _) | Node::ExplicitKern(k, _) | Node::AccentKern(k, _) | Node::ItalicKern(k, _) => *k as i64,
            Node::Box { w, .. } | Node::Rule { width: w, .. } => *w as i64,
            Node::NativeGlyphRun { width, .. } => *width as i64,
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
fn start_state<const LUA: bool>(
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
                while f < n && is_prunable::<LUA>(&list[f]) {
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
            depth: 0, subtype: crate::boxes::RULE_NORMAL, index: 0, attr: crate::boxes::Attr::NONE,
        };
        let list = vec![
            rule(),
            Node::Glue(Glue::zero(), crate::boxes::Attr::NONE),
            rule(),
            Node::Glue(Glue::zero(), crate::boxes::Attr::NONE),
            rule(),
            Node::Penalty(10_000, crate::boxes::Attr::NONE),
            Node::Glue(Glue::fil(GLUE_FIL, 0), crate::boxes::Attr::NONE),
        ];
        let Node::Box { list, .. } = engine.break_paragraph(list, 0, false) else {
            panic!("paragraph breaker did not return a vbox");
        };
        let penalties: Vec<i32> = list
            .iter()
            .filter_map(|node| match node {
                Node::Penalty(value, _) => Some(*value),
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
            depth: 0, subtype: crate::boxes::RULE_NORMAL, index: 0, attr: crate::boxes::Attr::NONE,
        };
        let list = vec![
            rule(),
            Node::Glue(Glue::zero(), crate::boxes::Attr::NONE),
            rule(),
            Node::Penalty(10_000, crate::boxes::Attr::NONE),
            Node::Glue(Glue::fil(GLUE_FIL, 0), crate::boxes::Attr::NONE),
        ];
        let (_node, record) = engine.break_paragraph_with_record(list, 0, false);
        assert_eq!(record.lines, 2);
        assert_eq!(record.pass, 0); // pretolerance succeeded
        assert!(record.demerits > 0);
        assert_eq!(engine.last_paragraph_layout.as_ref(), Some(&record));
    }
    #[test]
    fn saved_language_codes_drive_runtime_hyphenation() {
        let mut engine = Engine::new(true);
        engine.eqtb.int_params[IntParam::Language.idx() as usize] = 7;
        engine.eqtb.int_params[IntParam::UcHyph.idx() as usize] = 1;
        engine.eqtb.int_params[IntParam::LeftHyphenMin.idx() as usize] = 1;
        engine.eqtb.int_params[IntParam::RightHyphenMin.idx() as usize] = 1;
        engine.add_nullfont();
        engine.eqtb.lc_code[b'X' as usize] = b'x';
        engine.eqtb.lc_code[b'b' as usize] = b'b';
        engine.trie_for_language_mut(7).add_pattern_bytes(b"a1b");

        let mut codes = Box::new([0; 256]);
        codes[b'X' as usize] = b'a';
        codes[b'b' as usize] = b'b';
        engine.hyphen_codes.insert(7, codes);

        let word = || {
            vec![
                Node::Glue(Glue::zero(), crate::boxes::Attr::NONE),
                Node::Char { font: 0, c: b'X', attr: crate::boxes::Attr::NONE },
                Node::Char { font: 0, c: b'b', attr: crate::boxes::Attr::NONE },
                Node::Penalty(10_000, crate::boxes::Attr::NONE),
            ]
        };
        let mut with_saved_codes = word();
        engine.hyphenate_list(&mut with_saved_codes);
        assert!(with_saved_codes
            .iter()
            .any(|node| matches!(node, Node::Disc(_))));

        engine.hyphen_codes.remove(&7);
        let mut with_current_codes = word();
        engine.hyphenate_list(&mut with_current_codes);
        assert!(!with_current_codes
            .iter()
            .any(|node| matches!(node, Node::Disc(_))));
    }

    /// tex.web §897: with \uchyph<=0 a word whose first letter is not its
    /// own lc_code (a capital) is left unhyphenated.
    #[test]
    fn uchyph_zero_skips_capitalized_words() {
        let mut engine = Engine::new(true);
        engine.eqtb.int_params[IntParam::Language.idx() as usize] = 0;
        engine.eqtb.int_params[IntParam::LeftHyphenMin.idx() as usize] = 1;
        engine.eqtb.int_params[IntParam::RightHyphenMin.idx() as usize] = 1;
        engine.add_nullfont();
        engine.eqtb.lc_code[b'A' as usize] = b'a';
        engine.eqtb.lc_code[b'a' as usize] = b'a';
        engine.eqtb.lc_code[b'b' as usize] = b'b';
        engine.trie_for_language_mut(0).add_pattern_bytes(b"a1b");
        let word = |first: u8| {
            vec![
                Node::Glue(Glue::zero(), crate::boxes::Attr::NONE),
                Node::Char { font: 0, c: first, attr: crate::boxes::Attr::NONE },
                Node::Char { font: 0, c: b'b', attr: crate::boxes::Attr::NONE },
                Node::Penalty(10_000, crate::boxes::Attr::NONE),
            ]
        };
        let hyphenated = |engine: &mut Engine, first: u8| {
            let mut list = word(first);
            engine.hyphenate_list(&mut list);
            list.iter().any(|node| matches!(node, Node::Disc(_)))
        };
        engine.eqtb.int_params[IntParam::UcHyph.idx() as usize] = 0;
        assert!(hyphenated(&mut engine, b'a'));
        assert!(!hyphenated(&mut engine, b'A'));
        engine.eqtb.int_params[IntParam::UcHyph.idx() as usize] = 1;
        assert!(hyphenated(&mut engine, b'A'));
    }

    /// tex.web §903-§918, measured with TeX Live 2026 pdftex -ini (cmr10,
    /// \patterns{f1f f1l}): the natural widths of each line, last first.
    /// Ligatures re-form around the hyphen ("of-" / "fice" with the fi
    /// ligature, "baf-" / "fling"); explicit discretionaries end the word;
    /// only the second pass reconstitutes, so `shelf{}ful` gains the ff
    /// ligature there but keeps its split f's when the first pass succeeds.
    #[test]
    fn hyphenation_reconstitutes_ligatures_like_tex_live() {
        let cases = [
            (
                r"\hsize=1pt \pretolerance=-1 \hskip0pt office baffling",
                "18.88895pt 16.94449pt 14.44446pt 11.38892pt 0pt",
            ),
            (
                r"\hsize=1pt \pretolerance=-1 \hskip0pt
                  ef\discretionary{-}{}{}fi\discretionary{-}{}{}cient difficult",
                "22.22227pt 14.72226pt 20.83336pt 8.8889pt 10.83334pt 0pt",
            ),
            (r"\hsize=1pt \pretolerance=-1 \hskip0pt shelf{}ful", "11.38893pt 23.11115pt 0pt"),
            (r"\hsize=100pt \pretolerance=-1 \hskip0pt shelf{}ful baffling", "66.44461pt"),
            (r"\hsize=100pt \pretolerance=10000 \hskip0pt shelf{}ful baffling", "66.7224pt"),
        ];
        for (text, widths) in cases {
            let checks: String = widths.split(' ').map(|w| format!(r"\check{w} ")).collect();
            let mut engine = Engine::new(true);
            engine.init_primitives();
            engine.add_nullfont();
            let src = format!(
                "\\catcode`\\{{=1 \\catcode`\\}}=2 \\catcode`\\#=6 \
                 \\font\\cmr=cmr10 \\cmr \\hyphenchar\\cmr=45 \\lefthyphenmin=1 \\righthyphenmin=1 \
                 \\parindent=0pt \\overfullrule=0pt \\hbadness=10000 \\tolerance=10000 \
                 \\parfillskip=0pt plus 1fil \\patterns{{f1f f1l}}\
                 \\def\\check#1 {{\\setbox2\\lastbox \\setbox3\\hbox{{\\unhcopy2}}\
                 \\ifdim\\wd3=#1\\else\\errmessage{{got \\the\\wd3}}\\fi\\unskip\\unpenalty}}\
                 \\setbox1\\vbox{{{text}\\par {checks}\
                 \\setbox2\\lastbox \\ifvoid2 \\else\\errmessage{{extra line}}\\fi}}\n"
            );
            engine.input.push_file("hyph.tex".into(), src.into_bytes());
            engine.run();
            assert_eq!(engine.error_count, 0, "{text}:\n{}", engine.diagnostic_output);
        }
    }
}
