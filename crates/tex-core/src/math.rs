//! Math mode: math list building and conversion of math lists to horizontal
//! lists — TeX82 Appendix G (`mlist_to_hlist`, `make_op`, `make_fraction`,
//! `make_radical`, `make_scripts`, `make_accent`, `make_left_right`).
//!
//! Styles use the tex.web encoding in one byte (`GStyle`): even = D/T/S/SS,
//! odd = the cramped variants: 0=D 1=D' 2=T 3=T' 4=S 5=S' 6=SS 7=SS'.
//!
//! Delimiter sizing follows `var_delimiter` (small char, then the large char
//! and its "next larger" TAG_LIST chain, then TAG_EXT extensible stacking).
//!
//! `\left...\right` groups are stored in the enclosing math list as raw nodes
//! bracketed by `DelimBox{size:0}` (open boundary) and `DelimBox{size:1}`
//! (close boundary) markers; conversion buffers between them, measures the
//! body, and picks delimiter variants of matching height (`make_left_right`).
//! `DelimBox{size:2}` is a plain delimiter atom (Ord).

use crate::boxes::{hlist_dims, hpack, vpack, Glue, MathStyle, Node, NodeList, HBOX, VBOX};
use crate::engine::{Engine, Mode};
use crate::eqtb::Equiv;
use crate::prim::{DimParam, GlueParam, IntParam, Prim};
use crate::scaled::ONE;
use crate::tfm::{Font, FontId, TAG_EXT, TAG_LIG, TAG_LIST};
use crate::token::Token;

// ---------- style ladder (tex.web §689) ----------

pub type GStyle = u8;

#[inline]
pub fn gstyle_of(m: MathStyle) -> GStyle {
    match m {
        MathStyle::Display => 0,
        MathStyle::Text => 2,
        MathStyle::Script => 4,
        MathStyle::ScriptScript => 6,
    }
}
/// tex.web `half(x)`: round x/2, .5 up (odd positives toward +inf, odd
/// negatives toward -inf exactly as `(x+1) div 2` in Pascal)
#[inline]
fn half_sp(x: i64) -> i64 {
    if x % 2 == 0 {
        x / 2
    } else {
        (x + 1) / 2
    }
}

/// font table index (text/script/scriptscript) for a style: D,T -> 0; S -> 1; SS -> 2
#[inline]
fn font_size(g: GStyle) -> usize {
    match g {
        0 | 1 | 2 | 3 => 0,
        4 | 5 => 1,
        _ => 2,
    }
}

/// denominator style: next level up (tex.web §738:
/// `num_style = #+2-2*(# div 6)`, `denom_style = 2*(# div 2)+cramped+2-2*(# div 6)`)
#[inline]
fn num_style(g: GStyle) -> GStyle {
    if g < 6 {
        g + 2
    } else {
        g
    }
}

#[inline]
fn den_style(g: GStyle) -> GStyle {
    2 * (g / 2) + 3 - 2 * (g / 6)
}

/// superscript style
#[inline]
fn sup_style(g: GStyle) -> GStyle {
    match g >> 1 {
        0 | 1 => 4 + (g & 1),
        _ => 6 + (g & 1),
    }
}

/// subscript style: always cramped at the next level
#[inline]
fn sub_style(g: GStyle) -> GStyle {
    match g >> 1 {
        0 | 1 => 5,
        _ => 7,
    }
}

/// style after a `\left...\right` group: D becomes T, everything else becomes
/// the cramped variant of itself (tex.web §817)
#[inline]
fn after_lr_style(g: GStyle) -> GStyle {
    if g == 0 {
        2
    } else {
        g | 1
    }
}

// ---------- atom classes ----------

pub const CL_ORD: u8 = 0;
pub const CL_OP: u8 = 1;
pub const CL_BIN: u8 = 2;
pub const CL_REL: u8 = 3;
pub const CL_OPEN: u8 = 4;
pub const CL_CLOSE: u8 = 5;
pub const CL_PUNCT: u8 = 6;
pub const CL_INNER: u8 = 7;

/// Finish a brace-delimited math field as tex.web §1198 does.
///
/// Braces around one scriptless Ord noad disappear; every other field stays
/// an Ord whose nucleus is a sub-mlist. Keeping the singleton as a character
/// is essential because `make_scripts` then uses its italic correction and
/// skips the box-nucleus drop calculations.
pub(crate) fn finish_math_group(mut inner: NodeList) -> Node {
    if inner.len() == 1
        && matches!(
            inner.first(),
            Some(Node::MathChar {
                class: CL_ORD | 7,
                ..
            })
        )
    {
        return inner.pop().unwrap();
    }
    let mut nucleus = Vec::with_capacity(inner.len() + 1);
    nucleus.push(Node::MathChar {
        fam: 255,
        c: 0,
        class: CL_ORD,
    });
    nucleus.extend(inner);
    Node::Scripts {
        nucleus,
        sup: None,
        sub: None,
    }
}

/// tex.web §760 "magic" spacing string, rows = left class, cols = right class
/// (ord op bin rel open close punct inner). Digits: 0 = none, 1 = thin in
/// D/T only, 2 = thin, 3 = medium in D/T only, 4 = thick in D/T only,
/// 9 = impossible (the bin is demoted to ord before lookup).
const SPACING: [[u8; 8]; 8] = [
    //         ord op bin rel open close punct inner
    /* ord  */ [0, 2, 3, 4, 0, 0, 0, 1],
    /* op   */ [2, 2, 9, 4, 0, 0, 0, 1],
    /* bin  */ [3, 3, 9, 9, 3, 9, 9, 3],
    /* rel  */ [4, 4, 9, 0, 4, 0, 0, 4],
    /* open */ [0, 0, 9, 0, 0, 0, 0, 0],
    /* close*/ [0, 2, 3, 4, 0, 0, 0, 1],
    /* punct*/ [1, 1, 9, 1, 1, 1, 1, 1],
    /* inner*/ [1, 2, 3, 4, 1, 0, 1, 1],
];

#[inline]
fn is_bin_forbidden_left(c: u8) -> bool {
    matches!(c, CL_BIN | CL_OP | CL_REL | CL_OPEN | CL_PUNCT)
}

#[inline]
fn is_bin_forbidden_right(c: u8) -> bool {
    matches!(c, CL_REL | CL_CLOSE | CL_PUNCT)
}

// ---------- Radical.thickness encoding ----------
//
// The `Frac` node keeps plain semantics: thickness < 0 = default rule,
// 0 = atop (no rule), > 0 = explicit. `Radical` has no field for the 27-bit
// `\radical` delimiter code, so `do_radical` packs it into `thickness` with
// the sentinel below (both are created and consumed only inside this file).
// thickness <= -1  =>  delimiter code = -1 - thickness (0 => no surd),
//                      rule thickness = default.
// thickness > 0    =>  explicit rule thickness, no delimiter code stored.

#[inline]
fn pack_radical_delim(code: i32) -> i32 {
    -1 - (code & 0x0FFF_FFFF)
}

#[inline]
fn unpack_radical(t: i32) -> (i32, i32) {
    if t <= -1 {
        (-1 - t, -1)
    } else {
        (0, t)
    }
}

#[inline]
fn delim_code_parts(code: i32) -> (u8, u8, u8, u8) {
    // 27-bit tex.web delimiter: class(3) @24, small fam(4) @20, small
    // char(8) @12, large fam(4) @8, large char(8) @0 — the small fam mask
    // must be 4 bits; 0xFF leaks the class into the family (e.g. `\{` =
    // "426630A decoded to fam 0x42=66 → glyph 'f' instead of fam 2's brace)
    (
        ((code >> 20) & 0xF) as u8,
        ((code >> 12) & 0xFF) as u8,
        ((code >> 8) & 0xF) as u8,
        (code & 0xFF) as u8,
    )
}

/// public view for maincontrol's standalone \delimiter arm
pub fn delim_code_parts_pub(code: i32) -> (u8, u8, u8, u8) {
    delim_code_parts(code)
}

/// Record a limit_switch subtype on an operator noad's nucleus. The fam-255
/// CL_OP marker char carries tex.web's noad subtype (0=normal, 1=limits,
/// 2=no_limits). This helper runs only for an EXPLICIT directive, whose
/// st must survive on an `OpLimits` node, where a missing marker otherwise
/// reads as `normal` (`no_limits` is only the implicit state of a plain
/// `Scripts` node — see `append_script`). A bare-char op nucleus therefore
/// gains a marker for any recorded subtype; `normal` on a grouped nucleus
/// keeps its existing marker. (A marker is never stripped down to nothing:
/// a grouped mathop keeps its marker + content so build_op_box boxes it as
/// a group, not a char nucleus.)
fn set_limits_subtype(op: &mut NodeList, st: u8) {
    if matches!(
        op.first(),
        Some(Node::MathChar {
            fam: 255,
            class: CL_OP,
            ..
        })
    ) {
        if let Some(Node::MathChar { c, .. }) = op.first_mut() {
            *c = st;
        }
    } else if matches!(op.first(), Some(Node::MathChar { class: CL_OP, .. })) {
        op.insert(
            0,
            Node::MathChar {
                fam: 255,
                c: st,
                class: CL_OP,
            },
        );
    }
}

/// math_limits request code (maincontrol: 0=\nolimits 1=\limits
/// 2=\displaylimits) to tex.web op_noad subtype.
#[inline]
fn limits_req_to_subtype(v: u8) -> u8 {
    match v {
        0 => 2,
        1 => 1,
        _ => 0,
    }
}

#[inline]
fn pair_to_code(p: Option<(u8, u8)>) -> i32 {
    match p {
        Some((f, c)) if c != 0 => {
            ((f as i32) << 20) | ((c as i32) << 12) | ((f as i32) << 8) | (c as i32)
        }
        _ => 0,
    }
}

#[inline]
fn delim_marker(code: i32, size: u8) -> Node {
    let (sf, sc, lf, lc) = delim_code_parts(code);
    Node::DelimBox {
        small: (sf, sc),
        large: (lf, lc),
        size,
    }
}

// =====================================================================

impl Engine {
    // ---------- mode entry / exit ----------

    pub fn enter_math(&mut self, _display: bool) {
        let mut display = _display;
        if !display && self.mode == Mode::Horizontal {
            // tex.web §1134: a second math_shift promotes to display math.
            // The peek must be RAW: get_token processes \if conditionals
            // (tex.web get_next), so `$\ifmmode...` would evaluate \ifmmode
            // BEFORE mode=Math is set (getting false) and the skipped
            // branch's tokens would be consumed here
            let t = self.raw_token();
            if t.is_char() && t.cc() == 3 {
                display = true;
            } else if t != crate::input::EOF_MARKER {
                self.pushed.push(t);
            }
        }
        if self.mode == Mode::DisplayMath {
            return;
        }
        if display {
            // tex.web §1185: $$ in vertical mode starts a new paragraph —
            // the \parindent box becomes the interrupted paragraph, whose
            // break supplies \predisplaysize and prev_depth = 0. Without
            // this the display inherited the sentinel pds (always-short
            // skip selection) and a stale prev_depth.
            if self.mode == Mode::Vertical {
                self.start_paragraph(true);
            }
            if self.mode == Mode::Horizontal {
                // yet) gives \predisplaysize = -max_dimen; otherwise the
                // interrupted paragraph is broken and its final line is
                // measured — before the end-of-paragraph reset clears
                // \parshape/\hangindent state.
                let was_empty = self.cur_list.is_empty();
                // tex.web §21764: the interrupted paragraph's final widow
                // penalty is \displaywidowpenalty, not \widowpenalty
                if !was_empty {
                    self.next_par_widow =
                        Some(self.eqtb.int_params[IntParam::DisplayWidowPenalty.idx() as usize]);
                }
                let shape = self.par_shape.clone();
                let hang = (
                    self.eqtb.dim_params[DimParam::HangIndent.idx() as usize] as i64,
                    self.eqtb.int_params[IntParam::HangAfter.idx() as usize] as i64,
                );
                self.in_display_init = true;
                self.par_primitive();

                self.in_display_init = false;
                let prev_graf = self.prev_graf() as i64;
                let hsize = self.eqtb.dim_params[DimParam::HSize.idx() as usize] as i64;
                // §1184: display width/indent from \parshape (1-based entry
                // prev_graf+2, clamped to n) or \hangindent, else \hsize/0
                let (l, s) = if !shape.is_empty() {
                    let n = shape.len() as i64;
                    let k = (prev_graf + 2).min(n);
                    let e = shape[(k - 1) as usize];
                    (e.1 as i64, e.0 as i64) // engine stores (indent, width)
                } else if hang.0 != 0
                    && ((hang.1 >= 0 && prev_graf + 2 > hang.1) || (prev_graf + 1 < -hang.1))
                {
                    (hsize - hang.0.abs(), if hang.0 > 0 { hang.0 } else { 0 })
                } else {
                    (hsize, 0)
                };
                self.pre_display_l = l;
                self.pre_display_s = s;
                self.pre_display_size = if was_empty {
                    -0x3FFF_FFFF
                } else {
                    // tex.web §1181/§1148: \predisplaysize is measured on
                    // just_box — the interrupted paragraph's final line,
                    // captured at break time. Searching the contribution
                    // list is unreliable: build_page may have consumed the
                    // lines already (yielding the always-short-skip bug).
                    match self.last_par_line.take() {
                        Some(line) => self.pre_display_size_of(&line),
                        None => -0x3FFF_FFFF,
                    }
                };
            } else {
                // display entered from vertical mode: tex.web §1185 starts a
                // new paragraph whose zero-depth \parindent box is appended
                // first (append_to_vlist: prev_depth := box depth = 0), so
                // the interline glue above the display = baselineskip − h.
                self.prev_depth = 0;
                self.pre_display_size = -0x3FFF_FFFF;
                self.pre_display_l = self.eqtb.dim_params[DimParam::HSize.idx() as usize] as i64;
                self.pre_display_s = 0;
            }
            // tex.web push_math: the display math group level (exit_math /
            // \\endgroup pop it; dropping this push leaves one pop too many

            self.push_group_level(crate::eqtb::LevelType::Group);
            self.eqtb.assign_dim_param(
                crate::prim::DimParam::DisplayWidth,
                self.pre_display_l as i32,
                false,
            );
            self.eqtb.assign_dim_param(
                crate::prim::DimParam::DisplayIndent,
                self.pre_display_s as i32,
                false,
            );
            self.eqtb.assign_dim_param(
                crate::prim::DimParam::PreDisplaySize,
                self.pre_display_size as i32,
                false,
            );

            // tex.web push_math: eq_word_define(cur_fam_code,-1) — \\fam is
            // -1 inside every math group, restored at group end
            self.eqtb
                .assign_int_param(crate::prim::IntParam::CurFam, -1, false);
            let outer_mode = self.mode;
            self.saved_lists.push((
                outer_mode,
                std::mem::take(&mut self.cur_list),
                self.prev_depth,
                self.space_factor,
                self.prev_graf,
            ));
            self.mode = Mode::DisplayMath;
            self.math_style_stack.push(MathStyle::Display);
            // tex.web init_math: \everydisplay enters the input stack before
            // build_page; an output routine fired below must preempt it.
            let toks = (*self.eqtb.tok_params[crate::prim::ToksParam::EveryDisplay.idx() as usize])
                .clone();
            if !toks.is_empty() {
                self.push_tokens(toks);
            }
            // tex.web §1145: the interrupted paragraph reaches the outer page
            // builder before the display material exists. This online ordering
            // can fire a page that would otherwise absorb the later display.
            if outer_mode == Mode::Vertical {
                self.build_page();
            }
            // TeX's page/contribution list is global, not part of the semantic
            // nest pushed for display math. Keep `page_list` live while the
            // display runs so an output routine fired above can consume and
            // rebuild its remainder. `par_page_lists` carries only the marker
            // needed by the shared resume-after-display path.
            self.par_page_lists.push(Vec::new());
        } else {
            self.saved_lists.push((
                self.mode,
                std::mem::take(&mut self.cur_list),
                self.prev_depth,
                self.space_factor,
                self.prev_graf,
            ));
            self.push_group_level(crate::eqtb::LevelType::Group);
            // tex.web push_math: eq_word_define(cur_fam_code,-1)
            self.eqtb
                .assign_int_param(crate::prim::IntParam::CurFam, -1, false);
            self.mode = Mode::Math;
            self.math_style_stack.push(MathStyle::Text);
        }
        self.math_lists.push(crate::boxes::NodeList::new());
        self.right_delim = None;
        self.math_limits = None;
        // tex.web: \everymath only for inline math (displays ran
        // \everydisplay above)
        if self.mode != Mode::DisplayMath {
            self.run_everymath();
        }
    }

    /// tex.web start_eq_no (§21741): \eqno/\leqno in display math parks the
    /// current mlist as the formula; the tag collects into a fresh list
    /// until the closing display shift
    pub fn start_eq_no(&mut self, leqno: bool) {
        self.flush_math_limits();
        let formula = self.math_lists.pop().unwrap_or_default();
        self.pending_display_formula = Some(formula);
        self.math_lists.push(crate::boxes::NodeList::new());
        self.eqno_leqno = Some(leqno);
    }

    fn run_everymath(&mut self) {
        let toks =
            (*self.eqtb.tok_params[crate::prim::ToksParam::EveryMath.idx() as usize]).clone();
        if !toks.is_empty() {
            self.push_tokens(toks);
        }
    }

    pub fn exit_math(&mut self) {
        let was_display = self.mode == Mode::DisplayMath;
        if was_display {
            let t = self.get_token();
            if !(t.is_char() && t.cc() == 3) && t != crate::input::EOF_MARKER {
                self.pushed.push(t);
            }
        }
        // a directive at the very end of the formula (`$\sum_0^1\limits$`)
        // still switches the tail op noad before conversion
        self.flush_math_limits();
        let mlist = self.math_lists.pop().unwrap_or_default();
        // tex.web after_math reads the display registers BEFORE unsave:
        // assignments made inside the display (setspace's \everydisplay
        // scales the display skips group-locally) must still apply
        let disp_regs = if was_display {
            let g = |p: crate::prim::GlueParam| self.eqtb.glue_params[p.idx() as usize].clone();
            let i = |p: crate::prim::IntParam| self.eqtb.int_params[p.idx() as usize];
            let ads = g(crate::prim::GlueParam::AboveDisplaySkip);

            Some((
                ads,
                g(crate::prim::GlueParam::BelowDisplaySkip),
                g(crate::prim::GlueParam::AboveDisplayShortSkip),
                g(crate::prim::GlueParam::BelowDisplayShortSkip),
                i(crate::prim::IntParam::PreDisplayPenalty),
                i(crate::prim::IntParam::PostDisplayPenalty),
            ))
        } else {
            None
        };
        let is_outer_horiz = self
            .saved_lists
            .last()
            .map(|(m, ..)| *m == Mode::Horizontal)
            .unwrap_or(false);
        let inline_hlist = if !was_display {
            Some(self.mlist_to_hlist_pen(&mlist, 2, is_outer_horiz))
        } else {
            None
        };
        self.pop_group();
        let (outer_mode, outer_list, pd, sf, pg) = self.saved_lists.pop().unwrap_or((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
        ));
        self.prev_graf = pg;
        self.math_style_stack.pop();
        // leak guards: state set inside math must not escape it
        self.right_delim = None;
        while self
            .math_group_marks
            .last()
            .is_some_and(|(depth, _)| *depth > self.math_lists.len())
        {
            self.math_group_marks.pop();
        }
        self.mode = outer_mode;
        self.prev_depth = pd;
        self.space_factor = sf;
        self.cur_list = outer_list;
        if was_display {
            // tex.web after_math: with \eqno/\leqno the popped list is the
            // TAG; the formula was parked by start_eq_no
            let (formula, tag) = if let Some(leqno) = self.eqno_leqno.take() {
                (
                    self.pending_display_formula.take().unwrap_or_default(),
                    Some((mlist, leqno)),
                )
            } else {
                (mlist, None)
            };
            self.finish_display_math(formula, tag, disp_regs.unwrap(), outer_mode);
            return;
        }
        let hlist = inline_hlist.unwrap();
        let ms = self.eqtb.dim_params[DimParam::MathSurround.idx() as usize];
        match self.mode {
            // tex.web §22461 (finish math in text): the converted nodes are
            // SPLICED into the current hlist between math-on/math-off nodes
            // carrying \mathsurround — justification stretches into the
            // formula and lines may break inside it (never inside a box)
            Mode::Horizontal => {
                self.cur_list.push(Node::MathKern(ms, 1));
                self.cur_list.extend(hlist);
                self.cur_list.push(Node::MathKern(ms, 2));
                self.space_factor = 1000;
            }
            Mode::Vertical | Mode::InternalVertical => {
                let hbox = hpack(hlist, None, HBOX, &self.eqtb).node;
                self.vlist_append(hbox);
            }
            _ => {
                self.cur_list.push(Node::MathKern(ms, 1));
                self.cur_list.extend(hlist);
                self.cur_list.push(Node::MathKern(ms, 2));
            }
        }
    }

    fn finish_display_math(
        &mut self,
        formula: NodeList,
        tag: Option<(NodeList, bool)>,
        regs: (
            crate::boxes::Glue,
            crate::boxes::Glue,
            crate::boxes::Glue,
            crate::boxes::Glue,
            i32,
            i32,
        ),
        outer_mode: Mode,
    ) {
        if let Some(mut page) = self.par_page_lists.pop() {
            if outer_mode == Mode::Vertical {
                // Recover the live global contribution list after any output
                // routine that ran while the display was being scanned.
                page = std::mem::take(&mut self.page_list);
            }
            if let Some(mut rows) = self.display_halign.take() {
                // tex.web §16078 (finish_display_math with an alignment in
                // |temp_head|): the alignment rows — already carrying their
                // interline glue, including the glue before the first row
                // computed from the outer vlist's prev_depth (seeded in
                // finish_halign) — are spliced raw into the display vlist
                // between \predisplaypenalty/\abovedisplayskip and
                // \postdisplaypenalty/\belowdisplayskip. §16078 always uses
                // the NORMAL display skips; the short pair is a
                // formula-display optimization (§22578) and never applies
                // to alignment displays. No centering/tag dance: amsmath
                // typesets tags inside the rows themselves.
                let (ads, bds, _, _, pre, post) = regs;
                page.push(Node::Penalty(pre));
                page.push(Node::Glue(ads));
                // prev_depth after the display = the align's last row depth
                // (tex.web append_to_vlist at the align level; exit_math's
                // save-pop clobbers the value finish_halign computed)
                if let Some(Node::Box { d, .. }) =
                    rows.iter().rev().find(|n| matches!(n, Node::Box { .. }))
                {
                    self.prev_depth = *d;
                }
                if self.pre_display_s != 0 {
                    for r in &mut rows {
                        if let Node::Box { shift, .. } = r {
                            *shift += self.pre_display_s as i32;
                        }
                    }
                }
                page.extend(rows);
                page.push(Node::Penalty(post));
                page.push(Node::Glue(bds));
                if outer_mode == Mode::Vertical {
                    self.page_list = page;
                    let outer = std::mem::take(&mut self.page_list);
                    self.saved_lists.push((
                        Mode::Vertical,
                        Vec::new(),
                        self.prev_depth,
                        self.space_factor,
                        self.prev_graf,
                    ));
                    self.par_page_lists.push(outer);
                    self.mode = Mode::Horizontal;
                    self.cur_list = Vec::new();
                    self.space_factor = 1000;
                } else {
                    self.cur_list.extend(page);
                    let outer = std::mem::take(&mut self.cur_list);
                    self.saved_lists.push((
                        outer_mode,
                        Vec::new(),
                        self.prev_depth,
                        self.space_factor,
                        self.prev_graf,
                    ));
                    self.par_page_lists.push(outer);
                    self.mode = Mode::Horizontal;
                    self.cur_list = Vec::new();
                    self.space_factor = 1000;
                }
                self.resume_after_display();
                return;
            }
            // tex.web §22504 (finish displayed math): z = \displaywidth,
            // s = \displayindent, b = the formula at natural width
            let z = self.pre_display_l;
            let s = self.pre_display_s;
            let fh = self.mlist_to_hlist_pen(&formula, 0, false);
            // tex.web §22507: the display's hpack runs with adjust_tail
            // non-null (§22507 `adjust_tail:=adjust_head`), so §12956-12957
            // strips every ins/mark/adjust node out of the formula hlist;
            // §22611 splices the migrated material into the vlist right
            // after the display box. Without this, a \footnote inside
            // \begin{equation} never reaches the page builder and LaTeX's
            // \@makecol silently drops it (document2609.05162 p42).
            let (mut r0, migrated) = crate::boxes::hpack_migrate(fh, None, HBOX, &self.eqtb);
            let first_is_glue = matches!(
                &r0.node,
                Node::Box { list, .. } if matches!(list.first(), Some(Node::Glue(_)))
            );
            let mut w = self.box_w(&r0.node) as i64;
            // the tag (text style, natural width); e = its width, e=0 means
            // "on a line by itself" (or absent)
            let mut a: Option<Node> = None;
            let mut leqno = false;
            let mut e = 0i64;
            let mut q = 0i64;
            if let Some((tl, lq)) = tag {
                leqno = lq;
                let th = self.mlist_to_hlist_pen(&tl, 2, false);
                let ab = hpack(th, None, HBOX, &self.eqtb).node;
                e = self.box_w(&ab) as i64;
                // q = e + math_quad(text_size): quad of the fam-2 symbols font
                let mq = self
                    .fam_font(0, 2)
                    .map(|(_, f)| f.quad() as i64)
                    .unwrap_or(0);
                q = e + mq;
                a = Some(ab);
            }
            // §22537 squeeze: if the formula + tag overflow the line, re-pack
            // the formula to z-q when shrink allows; tex.web tests
            // `w-total_shrink[normal]+q<=z or total_shrink[fil/fill/filll]<>0`
            // — shrink totals, NOT stretch (a stretched \hfill does not let
            // an over-wide formula fit)
            if w + q > z {
                let can_squeeze = e != 0
                    && (w - r0.shrink[0] + q <= z
                        || r0.shrink[1] != 0
                        || r0.shrink[2] != 0
                        || r0.shrink[3] != 0);
                if can_squeeze {
                    if let Node::Box { list, .. } = r0.node {
                        r0 = hpack(list, Some((z - q) as i32), HBOX, &self.eqtb);
                    }
                } else {
                    e = 0;
                    q = 0;
                    if w > z {
                        if let Node::Box { list, .. } = r0.node {
                            r0 = hpack(list, Some(z as i32), HBOX, &self.eqtb);
                        }
                    }
                }
                w = self.box_w(&r0.node) as i64;
            }
            // §22560: centering displacement; too close to the tag -> center
            // in the remaining space (or honor leading user glue)
            let mut d = half_sp(z - w);
            if e > 0 && d < 2 * e {
                d = half_sp(z - w - e);
                if first_is_glue {
                    d = 0;
                }
            }
            // tex.web §22563: normal skips iff `(d+s<=pre_display_size) or l`
            // (l = \leqno). The sentinel -max_dimen (head=tail: `\noindent$$`
            // or a display following a display, §1148) therefore selects the
            // SHORT skips — raw comparison reproduces this, exactly as the
            // oracle shows (t6: consecutive $$ get \abovedisplayshortskip).
            let is_short = !leqno && (s + d > self.pre_display_size);

            let (above, below) = if is_short {
                (regs.2.clone(), regs.3.clone())
            } else {
                (regs.0.clone(), regs.1.clone())
            };
            let bs =
                self.eqtb.glue_params[crate::prim::GlueParam::BaselineSkip.idx() as usize].clone();
            let lsk =
                self.eqtb.glue_params[crate::prim::GlueParam::LineSkip.idx() as usize].clone();
            let lsl =
                self.eqtb.dim_params[crate::prim::DimParam::LineSkipLimit.idx() as usize] as i64;
            // tex.web interline glue for a vlist box append (used for the
            // display line and for own-line tag boxes below)
            // tex.web append_to_vlist: new_skip_param(baseline_skip_code)
            // COPIES the parameter glue spec (stretch/shrink/orders and all)
            // and only adjusts the width; below the limit it appends
            // new_param_glue(line_skip_code) untouched. Glue is Copy, so
            // clone the spec and overwrite `width`.
            let ilg = |prev_depth: i32, h: i64| -> Option<crate::boxes::Glue> {
                if prev_depth <= -1000 * 65536 {
                    return None;
                }
                let d = bs.width as i64 - prev_depth as i64 - h;
                if d < lsl {
                    return Some(lsk);
                }
                let mut g = bs;
                g.width = d as i32;
                Some(g)
            };
            let pre = regs.4;
            let post = regs.5;
            // §22601 `if g2>0`: TeX tests the glue-parameter CODE, so a
            // user-set \belowdisplayskip=0pt still appends a (zero) glue
            // node — only the "tag on its own line" case clears g2.
            let mut g2 = Some(below);
            page.push(Node::Penalty(pre));
            if leqno && e == 0 {
                // \leqno with the tag on its own line ABOVE the formula:
                // tex.web append_to_vlist gives the tag box ordinary interline
                // glue from prev_depth, then prev_depth := tag depth.
                if let Some(mut ab) = a.take() {
                    let (th, td) = match &mut ab {
                        Node::Box { shift, h, d, .. } => {
                            *shift = s as i32;
                            (*h as i64, *d as i64)
                        }
                        _ => (0, 0),
                    };
                    if let Some(g) = ilg(self.prev_depth, th) {
                        page.push(Node::Glue(g));
                    }
                    page.push(ab);
                    self.prev_depth = td as i32;
                    page.push(Node::Penalty(crate::scaled::INF_PENALTY));
                }
            } else {
                page.push(Node::Glue(above));
            }
            // the display line itself (§22592): with a tag, b becomes
            // [formula, kern z-w-e-d, tag] (or reversed for \leqno)
            let mut line = r0.node;
            if e != 0 {
                let ab = a.take().unwrap();
                let kern = Node::ExplicitKern((z - w - e - d) as i32);
                let (seq, nd) = if leqno {
                    (vec![ab, kern, line], 0i64)
                } else {
                    (vec![line, kern, ab], d)
                };
                d = nd;
                line = hpack(seq, None, HBOX, &self.eqtb).node;
            }
            if let Node::Box { shift, .. } = &mut line {
                *shift = (s + d) as i32;
            }
            // tex.web append_to_vlist: the display box joins the vlist with
            // ordinary interline glue (from the previous box's depth,
            // ignoring the display skips). Oracle shows
            // \glue(\baselineskip) between \abovedisplayskip and the
            // display box; omitting it tightens every display by ~4pt.
            let (lh, ld) = match &line {
                Node::Box { h, d, .. } => (*h as i64, *d as i64),
                _ => (0, 0),
            };
            if let Some(g) = ilg(self.prev_depth, lh) {
                page.push(Node::Glue(g));
            }
            page.push(line);
            self.prev_depth = ld as i32;
            // §22598: a right tag on its own line follows the display, flush
            // right, after an infinite penalty; the below-skip is suppressed
            if e == 0 && !leqno {
                if let Some(mut ab) = a.take() {
                    let aw = self.box_w(&ab) as i64;
                    page.push(Node::Penalty(crate::scaled::INF_PENALTY));
                    let (th, td) = match &mut ab {
                        Node::Box { shift, h, d, .. } => {
                            *shift = (s + z - aw) as i32;
                            (*h as i64, *d as i64)
                        }
                        _ => (0, 0),
                    };
                    // tex.web §22598 appends the tag box via append_to_vlist:
                    // ordinary interline glue from the display's depth first,
                    // then prev_depth := tag box's depth.
                    if let Some(g) = ilg(self.prev_depth, th) {
                        page.push(Node::Glue(g));
                    }
                    page.push(ab);
                    self.prev_depth = td as i32;
                    g2 = None;
                }
            }
            // tex.web §22611-22613: `if t<>adjust_head then
            // {migrating material comes after equation number}
            // link(tail):=link(adjust_head); tail:=t' — the ins/mark/adjust
            // material migrated out of the formula hlist joins the vlist
            // after the display box (and after an own-line tag), ahead of
            // \postdisplaypenalty. No interline glue, no prev_depth change:
            // these nodes are appended raw to the tail.
            page.extend(migrated);
            page.push(Node::Penalty(post));
            if let Some(g) = g2 {
                page.push(Node::Glue(g));
            }
            if outer_mode == Mode::Vertical {
                self.page_list = page;
                let outer = std::mem::take(&mut self.page_list);
                self.saved_lists.push((
                    Mode::Vertical,
                    Vec::new(),
                    self.prev_depth,
                    self.space_factor,
                    self.prev_graf,
                ));
                self.par_page_lists.push(outer);
                self.mode = Mode::Horizontal;
                self.cur_list = Vec::new();
                self.space_factor = 1000;
            } else {
                self.cur_list.extend(page);
                let outer = std::mem::take(&mut self.cur_list);
                self.saved_lists.push((
                    outer_mode,
                    Vec::new(),
                    self.prev_depth,
                    self.space_factor,
                    self.prev_graf,
                ));
                self.par_page_lists.push(outer);
                self.mode = Mode::Horizontal;
                self.cur_list = Vec::new();
                self.space_factor = 1000;
            }
            // tex.web finish_display does NOT run build_page: the display
            // nodes stay on the vlist (visible to \lastskip — LaTeX's
            // theorem \addpenalty/\@xaddvskip dances read them) until the
            // next box append or paragraph end
        }
        // tex.web resume_after_display (§1194): any text following the
        // display resumes hmode directly — start_paragraph must not treat
        // it as a new paragraph (\parskip/\parindent/\everypar skipped)
        self.resume_after_display();
    }

    /// tex.web §1181 ("Calculate the natural width, w"): \predisplaysize is
    /// `shift + 2em(cur font)` plus the natural width of the final line up
    /// to and including its last *visible* node (chars, ligs, hlists/vlists,
    /// rules, leaders). Kerns, math-nodes (mathsurround markers), margin
    /// kerns and pdf-ref whatsits accumulate width without being visible
    /// (§1184: `kern_node,math_node: d:=width(p)`; pdftex:
    /// `margin_kern_node`, `pdf_refximage/refxform` widths). A glue whose
    /// stretch or shrink order matches the line's *active* order (and is
    /// nonzero) voids the remainder → max_dimen: TeX reads only *natural*
    /// widths here — §1186 warns that glue_set rounding is
    /// system-dependent and "must not infiltrate parameters like
    /// |pre_display_size|" — which is exactly why the active-order glue
    /// voids instead of being scaled.
    fn pre_display_size_of(&self, line: &Node) -> i64 {
        const MAX_DIM: i64 = 0x3FFF_FFFF;
        let Node::Box {
            list,
            shift,
            glue_sign,
            glue_order,
            ..
        } = line
        else {
            return -MAX_DIM;
        };
        let quad = self
            .eqtb
            .fonts
            .get(self.eqtb.cur_font_val as usize)
            .map(|f| f.quad() as i64)
            .unwrap_or(0);
        let mut v = *shift as i64;
        let mut w: i64 = -MAX_DIM;
        for node in list {
            let (d, visible): (i64, bool) = match node {
                Node::Char { c, font } => (
                    self.eqtb
                        .fonts
                        .get(*font as usize)
                        .map(|f| f.char_width(*c) as i64)
                        .unwrap_or(0),
                    true,
                ),
                Node::Ligature { lig_width, .. } => (*lig_width as i64, true),
                // §1184: hlist_node,vlist_node,rule_node: d:=width(p); goto found
                Node::Box { w, .. } => (*w as i64, true),
                Node::Rule { width, .. } => (*width as i64, true),
                // §1184: kern_node,math_node: d:=width(p) — invisible width
                Node::Kern(k) | Node::ExplicitKern(k) => (*k as i64, false),
                Node::MathKern(k, _) => (*k as i64, false),
                // pdftex §30662: margin_kern_node: d:=width(p)
                Node::MarginKern { width, .. } => (*width as i64, false),
                // pdftex §36000: whatsit width only for pdf_ref{image,form}
                Node::Whatsit(crate::boxes::WhatIt::PdfRefXImage { w, .. })
                | Node::Whatsit(crate::boxes::WhatIt::PdfRefXForm { w, .. }) => (*w as i64, false),
                Node::Glue(g) => {
                    if (*glue_sign == 1 && g.stretch_order as u8 == *glue_order && g.stretch != 0)
                        || (*glue_sign == 2 && g.shrink_order as u8 == *glue_order && g.shrink != 0)
                    {
                        v = MAX_DIM;
                    }
                    (g.width as i64, false)
                }
                Node::Leaders { glue, .. } => {
                    // §1192: leaders take the same active-glue test, then
                    // `goto found` (visible)
                    if (*glue_sign == 1
                        && glue.stretch_order as u8 == *glue_order
                        && glue.stretch != 0)
                        || (*glue_sign == 2
                            && glue.shrink_order as u8 == *glue_order
                            && glue.shrink != 0)
                    {
                        v = MAX_DIM;
                    }
                    (glue.width as i64, true)
                }
                _ => (0, false),
            };
            if visible {
                if v >= MAX_DIM {
                    w = MAX_DIM;
                } else {
                    v += d;
                    w = v + 2 * quad;
                }
            } else if v < MAX_DIM {
                v += d;
            }
        }
        w
    }

    pub fn append_mlist_node(&mut self, n: Node) {
        // tex.web math_limit_switch applies IMMEDIATELY to the op noad at
        // the mlist tail (`\sum_0^1\limits`); append_script consumed its own
        // request earlier. A pending switch with a non-operator tail is
        // dropped like TeX's misplaced switch.
        self.flush_math_limits();
        if let Some(l) = self.math_lists.last_mut() {
            l.push(n);
        } else {
            self.error("Math node outside math mode");
        }
    }

    pub fn append_mathchar(&mut self, mc: u16) {
        let class = (mc >> 12) as u8;
        let mut fam = ((mc >> 8) & 0xF) as u8;
        let c = (mc & 0xFF) as u8;
        // tex.web §17440: a class-7 (varfam) char takes the current \fam
        // when it is in range — \mathrm/\operator@font work through this
        // (\fam0 makes `ln` in \ln come out upright, not math italic)
        if class == 7 {
            let cur = self.eqtb.int_params[crate::prim::IntParam::CurFam.idx() as usize];
            if (0..16).contains(&cur) {
                fam = cur as u8;
            }
        }
        self.append_mlist_node(Node::MathChar { fam, c, class });
    }

    pub fn style_font(&self, fam: u8) -> u16 {
        let g = gstyle_of(self.cur_math_style());
        self.eqtb.style_fonts[font_size(g)][fam as usize]
    }

    pub fn cur_math_style(&self) -> MathStyle {
        self.math_style_stack
            .last()
            .copied()
            .unwrap_or(MathStyle::Text)
    }

    pub fn push_math_group(&mut self, left: i32) {
        // tex.web math_limit_switch fires at scan time: a directive before
        // `\left` belongs to the noad OUTSIDE the new group list
        self.flush_math_limits();
        self.saved_lists.push((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
        ));
        self.push_group_level(crate::eqtb::LevelType::Group);
        self.math_lists.push(vec![delim_marker(left, 0)]);
    }

    /// `\right` end of a `\left...\right` group: the inner *raw* math list is
    /// spliced into the enclosing math list, bracketed by boundary markers.
    /// Conversion (including delimiter sizing) happens in one pass later.
    pub fn pop_math_group_delimited(&mut self, right_delim: i32) {
        self.flush_math_limits();
        let inner = self.math_lists.pop().unwrap_or_default();
        self.pop_group();
        let (outer_mode, outer_list, pd, sf, pg) = self.saved_lists.pop().unwrap_or((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
        ));
        self.prev_graf = pg;
        self.mode = outer_mode;
        self.prev_depth = pd;
        self.space_factor = sf;
        self.cur_list = outer_list;
        match self.math_lists.last_mut() {
            Some(l) => {
                l.extend(inner);
                l.push(delim_marker(right_delim, 1));
            }
            None => {
                self.error("Missing $ inserted (\\right)");
            }
        }
    }

    // ---------- scripts, accents, radicals, fractions ----------

    /// `^` / `_`: scan the following group-or-token and attach it to the last
    /// atom of the current math list (tex.web "scripts on the tail noad").
    pub fn append_script(&mut self, sup: bool, _c: u8) {
        let limits_req = self.math_limits.take();
        let group = self.scan_math_group_or_token();
        let popped = if let Some(l) = self.math_lists.last_mut() {
            if l.iter()
                .rev()
                .take(4)
                .all(|n| matches!(n, Node::ChoiceAlt { .. }))
                && l.len() >= 5
                && matches!(l[l.len() - 5], Node::Choice)
            {
                let mut choice_nodes = Vec::new();
                for _ in 0..5 {
                    choice_nodes.push(l.pop().unwrap());
                }
                choice_nodes.reverse();
                Some(Node::Scripts {
                    nucleus: choice_nodes,
                    sup: None,
                    sub: None,
                })
            } else {
                l.pop()
            }
        } else {
            None
        };

        let top = match popped {
            Some(node) => node,
            None => Node::Scripts {
                nucleus: Vec::new(),
                sup: None,
                sub: None,
            },
        };
        match top {
            Node::Scripts {
                mut nucleus,
                sup: s,
                sub: x,
            } => {
                // a `\mathop{...}` group atom is stored as a null-MathChar-
                // prefixed Scripts node. tex.web keeps the limits subtype ON
                // THE NOAD, so a later second script must see the first
                // script's \limits/\nolimits request: encode the subtype in
                // the (invisible) fam255 prefix char's c field — 0=normal,
                // 1=limits, 2=no_limits (tex.web subtypes).
                let head_is_op =
                    matches!(nucleus.first(), Some(Node::MathChar { class: CL_OP, .. }));
                // A standard operator that is already a Scripts node was
                // deliberately given side scripts when its first script was
                // attached (in display style this is how \nolimits survives).
                // Keep that decision for the second script. Grouped \mathop
                // atoms carry TeX's noad subtype in their fam-255 marker.
                let mut sub_type = match nucleus.first() {
                    // c encodes tex.web subtype: 0=normal, 1=limits, 2=no_limits
                    Some(Node::MathChar {
                        fam: 255,
                        c,
                        class: CL_OP,
                    }) => *c,
                    Some(Node::MathChar { class: CL_OP, .. }) => 2,
                    _ => 0,
                };
                // A limits directive between the two scripts changes the
                // subtype of the same noad; it overrides the stored choice.
                match limits_req {
                    Some(0) => sub_type = 2,
                    Some(1) => sub_type = 1,
                    Some(2) => sub_type = 0,
                    _ => {}
                }
                if let Some(Node::MathChar {
                    fam: 255,
                    c,
                    class: CL_OP,
                }) = nucleus.get_mut(0)
                {
                    *c = sub_type;
                }
                // tex.web make_op: a forced `limits` subtype is honored in
                // EVERY style; `normal` defers to conversion time where the
                // actual (possibly nested) style is known; `no_limits` pins
                // side scripts. A bare-char op nucleus has no marker yet —
                // a forced \limits attaches one so convert_atom can see it
                // (stripping it would turn the grouped mathop payload into
                // an ambiguous char nucleus).
                let use_limits_node = head_is_op && sub_type != 2;
                if use_limits_node {
                    if sub_type == 1
                        && !matches!(nucleus.first(), Some(Node::MathChar { fam: 255, .. }))
                    {
                        nucleus.insert(
                            0,
                            Node::MathChar {
                                fam: 255,
                                c: 1,
                                class: CL_OP,
                            },
                        );
                    }
                    let (na, nb) = if sup {
                        (Some(group), x)
                    } else {
                        (s, Some(group))
                    };
                    self.append_mlist_node(Node::OpLimits {
                        op: nucleus,
                        above: na,
                        below: nb,
                    });
                } else {
                    let (ns, nx) = if sup {
                        (Some(group), x)
                    } else {
                        (s, Some(group))
                    };
                    self.append_mlist_node(Node::Scripts {
                        nucleus,
                        sup: ns,
                        sub: nx,
                    });
                }
            }
            Node::OpLimits {
                mut op,
                above,
                below,
            } => {
                // tex.web math_limit_switch writes subtype(tail) whenever the
                // tail noad is an op_noad — including between the noad's two
                // scripts. Re-record the request on the noad's marker.
                if let Some(v) = limits_req {
                    set_limits_subtype(&mut op, limits_req_to_subtype(v));
                }
                let (na, nb) = if sup {
                    (Some(group), below)
                } else {
                    (above, Some(group))
                };
                self.append_mlist_node(Node::OpLimits {
                    op,
                    above: na,
                    below: nb,
                });
            }
            atom => {
                let is_op = matches!(atom, Node::MathChar { class: CL_OP, .. });
                // tex.web math_limit_switch: only an op_noad tail takes the
                // switch; a misplaced \limits on an ord atom is ignored.
                let req = limits_req.filter(|_| is_op);
                // A default op (or explicit \displaylimits) stays `normal`:
                // OpLimits defers the choice to convert_atom's actual-style
                // test, so nested styles select correctly.
                let use_limits_node = match req {
                    Some(0) => false,
                    Some(1) | Some(2) | None => is_op,
                    _ => false,
                };
                let (sup_g, sub_g) = if sup {
                    (Some(group), None)
                } else {
                    (None, Some(group))
                };
                if use_limits_node {
                    let mut op = vec![atom];
                    if req == Some(1) {
                        op.insert(
                            0,
                            Node::MathChar {
                                fam: 255,
                                c: 1,
                                class: CL_OP,
                            },
                        );
                    }
                    self.append_mlist_node(Node::OpLimits {
                        op,
                        above: sup_g,
                        below: sub_g,
                    });
                } else {
                    self.append_mlist_node(Node::Scripts {
                        nucleus: vec![atom],
                        sup: sup_g,
                        sub: sub_g,
                    });
                }
            }
        }
    }

    /// tex.web `math_limit_switch` at scan time: `subtype(tail) := request`
    /// whenever the mlist tail is an op noad. A directive that is NOT
    /// followed by a script (`\sum_0^1\limits`, `\sum\limits x`, or a
    /// trailing `$\sum_0^1\limits$`) still rewrites the noad that precedes
    /// it; a switch on a non-operator tail is dropped like TeX's misplaced
    /// `\limits` (which errors — error parity is the engine's concern).
    ///
    /// The op noad's subtype lives on its fam-255 CL_OP nucleus marker
    /// (see `set_limits_subtype`), so a Scripts node whose scripts must
    /// now be forced is rewritten into an `OpLimits` node: that is exactly
    /// the shape `append_script` builds for a first forced script.
    fn flush_math_limits(&mut self) {
        let Some(req) = self.math_limits.take() else {
            return;
        };
        let st = limits_req_to_subtype(req);
        let Some(l) = self.math_lists.last_mut() else {
            return;
        };
        let Some(tail) = l.last_mut() else { return };
        match std::mem::replace(tail, Node::Empty) {
            Node::Scripts {
                mut nucleus,
                sup,
                sub,
            } => {
                let head_op = matches!(nucleus.first(), Some(Node::MathChar { class: CL_OP, .. }));
                if !head_op {
                    *tail = Node::Scripts { nucleus, sup, sub };
                    return;
                }
                if st == 2 {
                    // `no_limits` is the natural state of a Scripts node;
                    // a grouped op keeps its marker in sync, but inserting
                    // one on a bare char would route make_scripts through
                    // the boxed-group arm and lose the char nucleus.
                    if let Some(Node::MathChar {
                        fam: 255,
                        c,
                        class: CL_OP,
                    }) = nucleus.first_mut()
                    {
                        *c = 2;
                    }
                    *tail = Node::Scripts { nucleus, sup, sub };
                    return;
                }
                set_limits_subtype(&mut nucleus, st);
                if sup.is_some() || sub.is_some() {
                    *tail = Node::OpLimits {
                        op: nucleus,
                        above: sup,
                        below: sub,
                    };
                } else {
                    *tail = Node::Scripts { nucleus, sup, sub };
                }
            }
            Node::OpLimits {
                mut op,
                above,
                below,
            } => {
                set_limits_subtype(&mut op, st);
                *tail = Node::OpLimits { op, above, below };
            }
            // (removed stale duplicate comment)
            // a bare op char with no scripts: wrap it in the op-noad shape
            // so the recorded subtype survives until conversion (TeX sets
            // subtype(tail) at scan time; a following script then finds the
            // noad already pinned/forced instead of re-deriving defaults).
            Node::MathChar {
                fam,
                c,
                class: CL_OP,
            } => {
                let mut op = vec![Node::MathChar {
                    fam,
                    c,
                    class: CL_OP,
                }];
                set_limits_subtype(&mut op, st);
                *tail = Node::OpLimits {
                    op,
                    above: None,
                    below: None,
                };
            }
            other => *tail = other,
        }
    }

    /// scan a `{...}` group, a single control sequence, or a single character
    /// into a RAW math list (tex.web's "scan a math-list group").
    pub fn scan_math_group_or_token(&mut self) -> NodeList {
        // a directive pending here (`\sum\limits\mathop{xy}`) belongs to the
        // tail of the CURRENT list, before any temp/group list is pushed
        self.flush_math_limits();
        self.skip_spaces_relax();
        let t = self.get_token();
        if t == crate::input::EOF_MARKER {
            self.error("Missing { inserted in math mode");
            return Vec::new();
        }
        if t.is_char() && t.cc() == 1 {
            return self.scan_math_group_braced();
        }
        // single token: run it into a temporary math list
        self.math_lists.push(Vec::new());
        self.run_math_token(t);
        self.flush_math_limits();
        self.math_lists.pop().unwrap_or_default()
    }

    /// Execute tokens up to the matching `}` as a nested math list.
    fn scan_math_group_braced(&mut self) -> NodeList {
        self.math_lists.push(Vec::new());
        self.push_group_level(crate::eqtb::LevelType::Group);
        let my_level = self.eqtb.cur_level;
        loop {
            let t = self.get_token();

            if t.is_char() && t.cc() == 2 {
                if self.eqtb.cur_level == my_level {
                    // no nested level is open: this `}` closes OUR group.
                    // Raw save_stack.len() is NOT a nesting test — \aftergroup
                    // items and \let/\def assignments pushed inside the group
                    // inflate it, and dispatching our own closer here would
                    // let end_group reroute to end_box (box_kinds is non-empty
                    // inside a tabular cell), packing an outer box and
                    // desyncing the whole alignment (Misplaced &, \cr cascade).
                    self.pop_group();
                    break;
                }
                let nested = self.eqtb.cur_group_type();
                match nested {
                    // braces in math are pure grouping (tex.web math_group):
                    // pop directly, never through end_group's box reroute
                    Some(crate::eqtb::LevelType::Group) => {
                        self.pop_group();
                    }
                    _ => self.dispatch(t),
                }
                continue;
            }
            if t == crate::input::EOF_MARKER {
                self.error("Missing } in math group");
                self.pop_group();
                break;
            }
            if t.is_char() && t.cc() == 1 {
                if self.mode.is_m() {
                    // tex.web math_group: braces in math are pure grouping —
                    // the sublist stays RAW and is boxed at CONVERSION time
                    // with the style in force then (an eagerly packed group
                    // in `^{\mathrm{V}}' would print at the enclosing size).
                    // tex.web §1198 removes braces around one scriptless Ord
                    // noad; other groups remain raw until conversion so they
                    // acquire the style in force at their use site.
                    let inner = self.scan_math_group_braced();
                    self.append_mlist_node(finish_math_group(inner));
                } else {
                    self.begin_group(true);
                }
                continue;
            }
            self.run_math_token(t);
        }
        self.flush_math_limits();
        self.math_lists.pop().unwrap_or_default()
    }

    pub fn do_math_accent(&mut self, mc: u16) {
        let group = self.scan_math_group_or_token();
        let fam = ((mc >> 8) & 0xF) as u8;
        let c = (mc & 0xFF) as u8;
        self.append_mlist_node(Node::Accent {
            fam,
            c,
            body: group,
        });
    }

    pub fn do_radical(&mut self, delim: i32) {
        let group = self.scan_math_group_or_token();
        self.append_mlist_node(Node::Radical {
            body: group,
            left_delim: None,
            thickness: pack_radical_delim(delim),
        });
    }
    pub fn do_math_class(&mut self, class: u8) {
        let field = self.scan_math_group_or_token();
        let node = if field.is_empty() {
            Node::MathChar {
                fam: 255,
                c: 0,
                class,
            }
        } else if field.len() == 1 {
            match field.into_iter().next().unwrap() {
                Node::MathChar { fam, c, .. } => Node::MathChar { fam, c, class },
                other => {
                    let nuc = vec![
                        Node::MathChar {
                            fam: 255,
                            c: 0,
                            class,
                        },
                        other,
                    ];
                    Node::Scripts {
                        nucleus: nuc,
                        sup: None,
                        sub: None,
                    }
                }
            }
        } else {
            // multi-node group atom. tex.web: `\mathop{...}` (and the other
            // math_comp prims) tail_append a FRESH noad whose type is the
            // class and whose subtype is `normal` — scripts then take the
            // make_op promotion rule `(subtype=normal) and (cur_style<
            // text_style)`. Raw brace groups reach here through
            // scan_math_group_braced's fam255 CL_ORD marker instead and are
            // never re-classed. The fam255 CL_OP prefix carries the subtype
            // (c=0 normal; append_script/flush_math_limits overwrite it on
            // a genuine `\limits`/`\nolimits`/`\displaylimits`).
            let mut nuc: NodeList = vec![Node::MathChar {
                fam: 255,
                c: 0,
                class,
            }];
            nuc.extend(field);
            Node::Scripts {
                nucleus: nuc,
                sup: None,
                sub: None,
            }
        };
        self.append_mlist_node(node);
    }

    /// Knuth \\overline / \\underline: scan a math field, pack it, and
    /// put a default-rule bar above (or below) with 3 default_rule_thickness
    /// clearance (tex.web make_over / make_under).
    pub fn do_overline(&mut self, under: bool) {
        let group = self.scan_math_group_or_token();
        // tex.web make_over uses cramped_style; make_under keeps cur_style.
        let g = gstyle_of(self.cur_math_style()) | u8::from(!under);
        let body = self.mlist_to_hlist_pen(&group, g, self.mode == Mode::Horizontal);
        let packed = hpack(body, None, HBOX, &self.eqtb).node;
        let (w, body_h, _) = box_dims(&packed);
        let rt = self.default_rule_thickness(g);
        let kern = 3 * rt;
        let rule = Node::Rule {
            width: w,
            height: rt,
            depth: 0,
        };
        let mut vlist = Vec::new();
        if under {
            vlist.push(packed);
            vlist.push(Node::Kern(kern));
            vlist.push(rule);
        } else {
            // tex.web overbar: an extra rule-thickness kern above the rule.
            vlist.push(Node::Kern(rt));
            vlist.push(rule);
            vlist.push(Node::Kern(kern));
            vlist.push(packed);
        }
        let mut vb = vpack(vlist, None, VBOX, &self.eqtb).node;
        if under {
            // tex.web make_under keeps the nucleus baseline and puts the
            // complete rule stack below it, including one bottom clearance.
            if let Node::Box { h, d, .. } = &mut vb {
                let extent = *h as i64 + *d as i64 + rt as i64;
                *h = body_h;
                *d = (extent - body_h as i64) as i32;
            }
        }
        self.append_mlist_node(vb);
    }

    /// tex.web scan_delimiter: a character token with a `\delcode` uses it;
    /// `\delimiter` scans its 27-bit code; anything else backs up and scans
    /// an integer.
    pub fn scan_delim_int(&mut self) -> i32 {
        self.skip_spaces_relax();
        let t = self.get_token();
        if t == crate::input::EOF_MARKER {
            self.error("Missing delimiter (. inserted)");
            return 0;
        }
        if t.is_char() {
            let c = t.chr() as u8;
            // tex.web §240: period is the null delimiter (code 0)
            if c == b'.' {
                return 0;
            }
            let d = self.eqtb.del_code[c as usize];
            if d >= 0 {
                return d;
            }
            self.error("Missing delimiter (. inserted)");
            return 0;
        }
        if let Some(Equiv::Prim(Prim::Delimiter)) = self.eqtb.resolve(t.cs_id()).cloned() {
            let v = self.scan_int();
            if v < 0 || v >= 0x8000000 {
                self.error("Invalid delimiter code");
                return 0;
            }
            return v;
        }
        // tex.web: back_input, then scan a 27-bit integer constant
        self.pushed.push(t);
        let v = self.scan_int();
        if v < 0 || v >= 0x8000000 {
            self.error("Invalid delimiter code");
            return 0;
        }
        v
    }

    pub fn do_fraction(&mut self, p: Prim) {
        // numerator = everything accumulated in the current math list so far.
        // The list slot itself MUST stay: the Frac node is appended to it (at
        // the enclosing group/top level), so popping it here would strand the
        // `\right`/`$` that closes the enclosing group.
        // tex.web math_fraction: the numerator is the current mlist's
        // content SINCE THE INNERMOST `{` (subformula boundary), not the
        // whole level — `b^W=\frac{A}{B}` keeps `b^W=` outside the fraction
        self.flush_math_limits();
        let cur_depth = self.math_lists.len();
        let num = match self.math_lists.last_mut() {
            Some(l) => {
                let m = self
                    .math_group_marks
                    .iter()
                    .rev()
                    .find(|(d, _)| *d == cur_depth)
                    .map(|(_, mark)| *mark);
                match m {
                    Some(m) if m <= l.len() => l.split_off(m),
                    _ => std::mem::take(l),
                }
            }
            None => Vec::new(),
        };
        // denominator is scanned into a temporary list on top
        self.math_lists.push(Vec::new());
        // tex.web: the lexically-following arguments (\above's dimen, the
        // withdelims delimiter pair) are scanned immediately...
        let mut thickness = -1i32; // default rule thickness
        let mut ld = 0i32;
        let mut rd = 0i32;
        match p {
            Prim::Above => {
                thickness = self.scan_dimen(false, false);
            }
            Prim::Atop => {
                thickness = 0;
            }
            Prim::OverWithDelims => {
                ld = self.scan_delim_int();
                rd = self.scan_delim_int();
            }
            Prim::AtopWithDelims => {
                thickness = 0;
                ld = self.scan_delim_int();
                rd = self.scan_delim_int();
            }
            Prim::AboveWithDelims => {
                ld = self.scan_delim_int();
                rd = self.scan_delim_int();
                thickness = self.scan_dimen(false, false);
            }
            _ => {}
        }
        // ...and the denominator is the REST of the current math group (up
        // to the closing brace / end of formula), which stays unconsumed
        let den = self.scan_math_rest_of_group();
        let left = if ld > 0 { Some(ld) } else { None };
        let right = if rd > 0 { Some(rd) } else { None };
        self.append_mlist_node(Node::Frac {
            num,
            den,
            thickness,
            left,
            right,
        });
    }

    /// run tokens into the current math list until the enclosing group ends
    /// (a `}`, a `$`, or EOF, which is pushed back for the outer machinery)
    fn scan_math_rest_of_group(&mut self) -> NodeList {
        let start_level = self.eqtb.cur_level;
        loop {
            let t = self.get_token();
            if t == crate::input::EOF_MARKER {
                break;
            }
            if t.is_char() {
                if t.cc() == 2 && self.eqtb.cur_level <= start_level {
                    self.pushed.push(t);
                    break;
                }
                if t.cc() == 3 && self.eqtb.cur_level <= start_level {
                    self.pushed.push(t);
                    break;
                }
            }
            // tex.web: a display's \eqno/\leqno also closes the fraction's
            // denominator — the tag is a separate sublist, not formula tail
            if t.is_cs()
                && self.eqtb.cur_level <= start_level
                && matches!(
                    self.eqtb.resolve(t.cs_id()),
                    Some(crate::eqtb::Equiv::Prim(crate::prim::Prim::EqNo))
                        | Some(crate::eqtb::Equiv::Prim(crate::prim::Prim::LeqNo))
                )
            {
                self.pushed.push(t);
                break;
            }
            self.run_math_token(t);
        }
        self.flush_math_limits();
        self.math_lists.pop().unwrap_or_default()
    }

    pub fn style_quad(&self, style: MathStyle) -> i32 {
        let g = gstyle_of(style);
        self.math_quad(g)
    }

    // ---------- font/parameter access ----------

    fn fam_font(&self, g: GStyle, fam: u8) -> Option<(FontId, std::rc::Rc<Font>)> {
        let fid = self.eqtb.style_fonts[font_size(g)][fam as usize];
        if fid == 0 {
            return None;
        }
        self.eqtb.fonts.get(fid as usize).cloned().map(|f| (fid, f))
    }

    /// font parameter `i` (1-based) of family `fam` at style `g`
    fn fparam(&self, g: GStyle, fam: u8, i: usize) -> i32 {
        match self.fam_font(g, fam) {
            Some((fid, f)) => self
                .eqtb
                .font_params
                .get(fid as usize)
                .and_then(|v| v.get(i - 1).copied())
                .unwrap_or_else(|| f.param(i)),
            None => 0,
        }
    }
    /// font parameter `i` (1-based) of family `fam` at a specific font size
    /// index (0 = text, 1 = script, 2 = scriptscript)
    fn fparam_idx(&self, size_idx: usize, fam: u8, i: usize) -> i32 {
        let fid = self.eqtb.style_fonts[size_idx][fam as usize];
        if fid == 0 {
            return 0;
        }
        self.eqtb
            .font_params
            .get(fid as usize)
            .and_then(|v| v.get(i - 1).copied())
            .or_else(|| self.eqtb.fonts.get(fid as usize).map(|f| f.param(i)))
            .unwrap_or(0)
    }

    /// (font id, font) of family `fam` at a specific font size index
    fn fam_font_idx(&self, size_idx: usize, fam: u8) -> Option<(FontId, std::rc::Rc<Font>)> {
        let fid = self.eqtb.style_fonts[size_idx][fam as usize];
        if fid == 0 {
            return None;
        }
        self.eqtb.fonts.get(fid as usize).cloned().map(|f| (fid, f))
    }

    fn math_quad(&self, g: GStyle) -> i32 {
        let q = self.fparam(g, 2, 6);
        if q != 0 {
            q
        } else {
            ONE
        }
    }

    fn math_x_height(&self, g: GStyle) -> i32 {
        let x = self.fparam(g, 2, 5);
        if x != 0 {
            x
        } else {
            ONE * 45 / 100
        }
    }

    /// default rule thickness: fontdimen 8 of family 3 (the extension font)
    /// at the current size (tex.web `default_rule_thickness`)
    fn default_rule_thickness(&self, g: GStyle) -> i32 {
        let r = self.fparam(g, 3, 8);
        if r != 0 {
            r
        } else {
            ONE / 4
        }
    }

    fn axis_height(&self, g: GStyle) -> i32 {
        let a = self.fparam(g, 2, 22);
        if a != 0 {
            a
        } else {
            ONE * 25 / 100
        }
    }

    /// \mathchoice{D}{T}{S}{SS}: scan the four style groups immediately and
    /// attach them as ChoiceAlt bodies of a Choice atom; mlist_to_hlist picks
    /// the branch matching the current style.
    pub fn begin_mathchoice(&mut self) {
        self.append_mlist_node(Node::Choice);
        for _ in 0..4 {
            let body = self.scan_math_group_or_token();
            self.append_mlist_node(Node::ChoiceAlt { body });
        }
    }

    /// 1mu = quad of family 2 at the current math size / 18 (tex.web §767).
    fn mu_unit(&self, style: GStyle) -> i32 {
        self.math_quad(style) / 18
    }

    fn skew_char_of(&self, fid: FontId, _f: &Font) -> i32 {
        self.eqtb.skew_char.get(fid as usize).copied().unwrap_or(-1)
    }

    // ---------- the conversion itself ----------

    /// public entry: convert a raw math list produced at `style`
    pub fn math_to_hlist(&self, list: &[Node], style: MathStyle) -> NodeList {
        self.mlist_to_hlist_pen(list, gstyle_of(style), false)
    }

    /// classify a raw node as a spacing atom; None = not an atom
    fn atom_class(&self, n: &Node) -> Option<u8> {
        match n {
            // mathcode class 7 = variable: spaced as ord (tex.web §759)
            Node::MathChar { fam: 255, .. } => None,
            Node::MathChar { class, .. } => Some(if *class == 7 { CL_ORD } else { *class }),
            Node::Scripts { nucleus, .. } => Some(match nucleus.first() {
                // fam255 prefix carries the atom's class (op groups etc.);
                // a plain char nucleus with class 7 is varfam -> Ord.
                Some(Node::MathChar {
                    fam: 255, class, ..
                }) => *class,
                Some(Node::MathChar { class, .. }) => {
                    if *class == 7 {
                        CL_ORD
                    } else {
                        *class
                    }
                }
                _ => CL_ORD,
            }),
            Node::OpLimits { .. } => Some(CL_OP),
            // tex.web pass 2 (§14983 case): fraction_noad keeps the default
            // t=ord_noad — fractions take Ord spacing, NOT Inner (Inner is
            // for \mathinner atoms only)
            Node::Frac { .. } => Some(CL_ORD),
            Node::Radical { .. } => Some(CL_ORD),
            Node::Accent { .. } => Some(CL_ORD),
            Node::DelimBox { size, .. } => Some(match size {
                0 => CL_OPEN,
                1 => CL_CLOSE,
                _ => CL_ORD,
            }),
            Node::Box { .. } | Node::VCenter { .. } => Some(CL_ORD),
            Node::Choice => Some(CL_ORD),
            _ => None,
        }
    }

    fn math_noad_char(n: &Node) -> Option<(u8, u8)> {
        match n {
            Node::MathChar { fam, c, .. } if *fam != 255 => Some((*fam, *c)),
            Node::Scripts { nucleus, .. } if nucleus.len() == 1 => match &nucleus[0] {
                Node::MathChar { fam, c, .. } if *fam != 255 => Some((*fam, *c)),
                _ => None,
            },
            _ => None,
        }
    }

    fn set_math_noad_char(n: &mut Node, c: u8) {
        match n {
            Node::MathChar { c: current, .. } => *current = c,
            Node::Scripts { nucleus, .. } => {
                if let Some(Node::MathChar { c: current, .. }) = nucleus.first_mut() {
                    *current = c;
                }
            }
            _ => {}
        }
    }

    /// TeX's `make_ord`: combine adjacent ordinary math characters through
    /// the current math font's lig/kern program before atom conversion.
    ///
    /// The boolean side table represents TeX's `math_text_char` state. It
    /// suppresses italic correction only when the resulting character comes
    /// from a text font (`fontdimen2 != 0`).
    fn prepare_math_ligatures(&self, list: &[Node], start: GStyle) -> (NodeList, Vec<bool>) {
        let mut nodes = list.to_vec();
        let mut math_text = vec![false; nodes.len()];
        let mut style = start;
        let mut left_right_depth = 0usize;
        let mut i = 0usize;

        while i < nodes.len() {
            if let Node::DelimBox { size, .. } = &nodes[i] {
                match *size {
                    0 => left_right_depth += 1,
                    1 if left_right_depth > 0 => left_right_depth -= 1,
                    _ => {}
                }
                i += 1;
                continue;
            }
            // A \left...\right body is converted recursively. Leaving it raw
            // here prevents its ligature program from running twice.
            if left_right_depth > 0 {
                i += 1;
                continue;
            }
            if let Node::Style(s) = &nodes[i] {
                style = gstyle_of(*s);
                i += 1;
                continue;
            }
            let q_has_scripts = matches!(
                &nodes[i],
                Node::Scripts { sup: Some(_), .. } | Node::Scripts { sub: Some(_), .. }
            );
            if math_text[i] || q_has_scripts || self.atom_class(&nodes[i]) != Some(CL_ORD) {
                i += 1;
                continue;
            }

            let mut restarts = 0usize;
            loop {
                restarts += 1;
                if restarts > 256 || i + 1 >= nodes.len() {
                    break;
                }
                let Some(p_class) = self.atom_class(&nodes[i + 1]) else {
                    break;
                };
                if p_class > CL_PUNCT {
                    break;
                }
                let Some((q_fam, q_char)) = Self::math_noad_char(&nodes[i]) else {
                    break;
                };
                let Some((p_fam, p_char)) = Self::math_noad_char(&nodes[i + 1]) else {
                    break;
                };
                if q_fam != p_fam || q_fam as usize >= 16 {
                    break;
                }

                // tex.web §14865: this happens before testing whether the
                // font actually supplies a ligature or kern instruction.
                math_text[i] = true;
                let Some((_, font)) = self.fam_font_idx(font_size(style), q_fam) else {
                    break;
                };
                let Some(ci) = font.chars.get(q_char as usize) else {
                    break;
                };
                if ci.tag != TAG_LIG {
                    break;
                }

                let mut k = ci.remainder as usize;
                let Some(first) = font.lig_kern.get(k) else {
                    break;
                };
                if first.skip > 128 {
                    k = ((first.op as usize) << 8) | first.rem as usize;
                }
                let mut action: Option<(Option<i32>, u8, u8)> = None;
                for _ in 0..512 {
                    let Some(step) = font.lig_kern.get(k) else {
                        break;
                    };
                    if step.next_char == p_char && step.skip <= 128 {
                        if step.op >= 128 {
                            let ki = ((step.op as usize - 128) << 8) | step.rem as usize;
                            if let Some(kern) = font.kerns.get(ki) {
                                action = Some((Some(*kern), 0, 0));
                            }
                        } else {
                            action = Some((None, step.op, step.rem));
                        }
                        break;
                    }
                    if step.skip >= 128 {
                        break;
                    }
                    k += step.skip as usize + 1;
                }
                let Some((kern, op, replacement)) = action else {
                    break;
                };
                if let Some(kern) = kern {
                    nodes.insert(i + 1, Node::Kern(kern));
                    math_text.insert(i + 1, false);
                    break;
                }

                match op {
                    1 | 5 => Self::set_math_noad_char(&mut nodes[i], replacement),
                    2 | 6 => Self::set_math_noad_char(&mut nodes[i + 1], replacement),
                    3 | 7 | 11 => {
                        nodes.insert(
                            i + 1,
                            Node::MathChar {
                                fam: q_fam,
                                c: replacement,
                                class: CL_ORD,
                            },
                        );
                        math_text.insert(i + 1, op == 11);
                    }
                    _ => {
                        let p = nodes.remove(i + 1);
                        math_text.remove(i + 1);
                        Self::set_math_noad_char(&mut nodes[i], replacement);
                        if let Node::Scripts { sup, sub, .. } = p {
                            let nucleus = match nodes[i].clone() {
                                Node::Scripts { nucleus, .. } => nucleus,
                                q => vec![q],
                            };
                            nodes[i] = Node::Scripts { nucleus, sup, sub };
                        }
                    }
                }
                if op > 3 {
                    break;
                }
                math_text[i] = false;
            }
            i += 1;
        }
        (nodes, math_text)
    }

    /// `pen`: insert \binoppenalty/\relpenalty breakpoints after Bin/Rel
    /// atoms (tex.web pass 2, mlist_penalties = mode>0 i.e. inline text math
    /// only — never in displays or \hbox)
    fn mlist_to_hlist_pen(&self, list: &[Node], start: GStyle, pen: bool) -> NodeList {
        let saved_pen = self.math_penalties.replace(pen);
        let out = self.mlist_to_hlist_inner(list, start);
        self.math_penalties.set(saved_pen);
        out
    }

    fn mlist_to_hlist_inner(&self, list: &[Node], start: GStyle) -> NodeList {
        let (list, math_text_chars) = self.prepare_math_ligatures(list, start);
        // pass 1: classify atoms and demote binary operators that cannot be
        // binary in context (tex.web §760)
        let classes: Vec<Option<u8>> = list.iter().map(|n| self.atom_class(n)).collect();
        let mut eff: Vec<Option<u8>> = classes.clone();
        for i in 0..list.len() {
            if classes[i] == Some(CL_BIN) {
                let mut left = None;
                let mut right = None;
                for j in (0..i).rev() {
                    if classes[j].is_some() {
                        left = classes[j];
                        break;
                    }
                }
                for j in (i + 1)..list.len() {
                    if classes[j].is_some() {
                        right = classes[j];
                        break;
                    }
                }
                let demote = match left {
                    None => true,
                    Some(c) => is_bin_forbidden_left(c),
                } || match right {
                    None => true,
                    Some(c) => is_bin_forbidden_right(c),
                };
                if demote {
                    eff[i] = Some(CL_ORD);
                }
            }
        }

        // pass 2: convert, inserting mu glue (and line-break penalties) between
        // adjacent atoms per the spacing table
        let mut out: NodeList = Vec::new();
        let mut prev: Option<u8> = None;
        let mut style: GStyle = start;
        // \left...\right buffering stack: (left delim code, buffered raw nodes)
        let mut lr_stack: Vec<(i32, NodeList)> = Vec::new();
        let mut i = 0usize;
        while i < list.len() {
            let n = &list[i];
            if let Node::Style(s) = n {
                if let Some((_, buf)) = lr_stack.last_mut() {
                    buf.push(n.clone());
                } else {
                    style = gstyle_of(*s);
                }
                i += 1;
                continue;
            }
            if let Some(cls) = eff[i] {
                // \mathchoice: consume the ChoiceAlt bodies that follow
                if matches!(n, Node::Choice) {
                    let mut bodies: Vec<&NodeList> = Vec::new();
                    while i + 1 < list.len() {
                        if let Node::ChoiceAlt { body } = &list[i + 1] {
                            bodies.push(body);
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    if bodies.is_empty() {
                        i += 1;
                        continue;
                    }
                    let idx = ((style >> 1) as usize).min(bodies.len() - 1);
                    let chosen = bodies[idx];
                    let nodes = self.mlist_to_hlist_pen(chosen, style, self.math_penalties.get());
                    self.emit_atom(&mut out, &mut prev, Some(CL_ORD), nodes, style);
                    i += 1;
                    continue;
                }
                // boundary markers
                if let Node::DelimBox {
                    size: 0,
                    small,
                    large,
                } = n
                {
                    let code = delim_code_of(*small, *large);
                    lr_stack.push((code, Vec::new()));
                    i += 1;
                    continue;
                }
                // close marker — possibly wrapped by `^`/`_` (append_script
                // pops the tail marker and re-wraps it): the scripts belong on
                // the assembled group box (tex.web make_left_right tail).
                let close = match n {
                    Node::DelimBox {
                        size: 1,
                        small,
                        large,
                    } => Some((*small, *large, None, None)),
                    Node::Scripts { nucleus, sup, sub } => match nucleus.as_slice() {
                        [Node::DelimBox {
                            size: 1,
                            small,
                            large,
                        }] => Some((*small, *large, Some((sup.as_deref(), sub.as_deref())), None)),
                        _ => None,
                    },
                    Node::OpLimits { op, above, below } => match op.as_slice() {
                        [Node::DelimBox {
                            size: 1,
                            small,
                            large,
                        }] => Some((
                            *small,
                            *large,
                            None,
                            Some((above.as_deref(), below.as_deref())),
                        )),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some((small, large, scripts, limits)) = close {
                    let code = delim_code_of(small, large);
                    match lr_stack.pop() {
                        Some((lopen, buf)) => {
                            let body =
                                self.mlist_to_hlist_pen(&buf, style, self.math_penalties.get());
                            let (_, bh, bd) = hlist_dims(&body, &self.eqtb);
                            let needed = self.lr_delimiter_size(bh, bd, style);
                            let mut assembled: NodeList = self.var_delimiter(lopen, needed, style);
                            assembled.extend(body);
                            assembled.extend(self.var_delimiter(code, needed, style));
                            let gb = hpack(assembled, None, HBOX, &self.eqtb).node;
                            let tail: NodeList = match (scripts, limits) {
                                (Some((sup, sub)), _) => self.make_scripts(&[gb], sup, sub, style),
                                (None, Some((above, below))) => {
                                    self.make_op_limits(&[gb], above, below, style, true)
                                }
                                _ => vec![gb],
                            };
                            match lr_stack.last_mut() {
                                Some((_, pbuf)) => pbuf.extend(tail),
                                None => {
                                    self.emit_atom(&mut out, &mut prev, Some(CL_INNER), tail, style)
                                }
                            }
                        }
                        None => {
                            // stray close: a normal close delimiter
                            self.emit_atom(
                                &mut out,
                                &mut prev,
                                Some(CL_CLOSE),
                                self.var_delimiter(code, 0, style),
                                style,
                            );
                        }
                    }
                    i += 1;
                    continue;
                }
                // buffered \left...\right content (middles etc. keep going in)
                if let Some((_, buf)) = lr_stack.last_mut() {
                    buf.push(n.clone());
                    i += 1;
                    continue;
                }
                let nodes = self.convert_atom(n, style, math_text_chars[i]);
                // inter-atom mu glue comes first (tex.web second pass)
                self.insert_spacing(&mut out, prev, Some(cls), style);
                out.extend(nodes);
                // tex.web pass 2: after a Bin/Rel noad in inline text math,
                // a \binoppenalty/\relpenalty breakpoint follows (skipped when
                // the next noad is a penalty, a relation, or absent)
                if self.math_penalties.get() {
                    let pval = match cls {
                        CL_BIN => Some(self.eqtb.int_params[IntParam::BinOpPenalty.idx() as usize]),
                        CL_REL => Some(self.eqtb.int_params[IntParam::RelPenalty.idx() as usize]),
                        _ => None,
                    };
                    if let Some(pv) = pval {
                        let suppress = match list.get(i + 1) {
                            None => true,
                            Some(Node::Penalty(_)) => true,
                            Some(nn) => self.atom_class(nn) == Some(CL_REL),
                        };
                        if !suppress {
                            out.push(Node::Penalty(pv));
                        }
                    }
                }
                prev = Some(cls);
                i += 1;
                continue;
            }
            // Internal group/class markers carry no material into the hlist.
            if matches!(n, Node::MathChar { fam: 255, .. }) {
                i += 1;
                continue;
            }
            // non-atoms: convert mu-denominated material at the current
            // style. \nonscript removes the following glue/kern outside
            // text styles (tex.web mlist_to_hlist pass 1).
            if matches!(n, Node::NonScript) {
                if style >= 4
                    && matches!(
                        list.get(i + 1),
                        Some(
                            Node::Glue(_)
                                | Node::MuGlue(_)
                                | Node::Kern(_)
                                | Node::ExplicitKern(_)
                                | Node::MathKern(_, _)
                        )
                    )
                {
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            let converted = match n {
                Node::MuGlue(g) => {
                    let mu = self.mu_unit(style) as i64;
                    let conv = |v: i32| (v as i64 * mu / 65536) as i32;
                    Node::Glue(Glue {
                        width: conv(g.width),
                        stretch: conv(g.stretch),
                        shrink: conv(g.shrink),
                        stretch_order: g.stretch_order,
                        shrink_order: g.shrink_order,
                    })
                }
                Node::MathKern(k, 0) => {
                    Node::Kern((*k as i64 * self.mu_unit(style) as i64 / 65536) as i32)
                }
                _ => n.clone(),
            };
            if let Some((_, buf)) = lr_stack.last_mut() {
                buf.push(converted);
            } else if !matches!(n, Node::ChoiceAlt { .. }) {
                out.push(converted);
            }
            i += 1;
        }
        while let Some((lopen, buf)) = lr_stack.pop() {
            let body = self.mlist_to_hlist_pen(&buf, style, self.math_penalties.get());
            let (_, bh, bd) = hlist_dims(&body, &self.eqtb);
            let needed = self.lr_delimiter_size(bh, bd, style);
            out.extend(self.var_delimiter(lopen, needed, style));
            out.extend(body);
        }
        out
    }

    fn emit_atom(
        &self,
        out: &mut NodeList,
        prev: &mut Option<u8>,
        cls: Option<u8>,
        nodes: NodeList,
        style: GStyle,
    ) {
        self.insert_spacing(out, *prev, cls, style);
        out.extend(nodes);
        *prev = cls;
    }

    /// size request for the two delimiters of a `\left..\right` group
    /// (tex.web `make_left_right`): twice the larger axis distance, via
    /// `\delimiterfactor` and `\delimitershortfall`.
    fn lr_delimiter_size(&self, bh: i32, bd: i32, style: GStyle) -> i32 {
        let axis = self.axis_height(style);
        let d2 = bd + axis;
        let mut d1 = bh + bd - d2;
        if d2 > d1 {
            d1 = d2;
        }
        let mut factor = self.eqtb.int_params[IntParam::DelimiterFactor.idx() as usize] as i64;
        if factor <= 0 {
            factor = 901;
        }
        let shortfall = self.eqtb.dim_params[DimParam::DelimiterShortfall.idx() as usize] as i64;
        let mut delta = (d1 as i64 / 500) * factor;
        let delta2 = 2 * d1 as i64 - shortfall;
        if delta < delta2 {
            delta = delta2;
        }
        delta.max(0) as i32
    }

    fn insert_spacing(&self, out: &mut NodeList, prev: Option<u8>, cur: Option<u8>, style: GStyle) {
        let (Some(a), Some(b)) = (prev, cur) else {
            return;
        };
        let kind = SPACING[(a.min(7)) as usize][(b.min(7)) as usize];
        if kind == 0 || kind >= 9 {
            return;
        }
        // conditional classes (1/3/4) get no space in script/scriptscript
        if matches!(kind, 1 | 3 | 4) && style >= 4 {
            return;
        }
        // tex.web §716 (math_glue): the muskip parameters hold mu-denominated
        // glue; convert with the current style's mu so `\medmuskip=...`
        // assignments by packages actually take effect.
        let mu = self.mu_unit(style);
        let conv = |sp_mu: i32| ((sp_mu as i64) * (mu as i64) / 65536) as i32;
        let src = match kind {
            1 | 2 => GlueParam::ThinMuSkip,
            3 => GlueParam::MedMuSkip,
            _ => GlueParam::ThickMuSkip,
        };
        let p = &self.eqtb.glue_params[src.idx() as usize];
        let g = Glue {
            width: conv(p.width),
            stretch: conv(p.stretch),
            shrink: conv(p.shrink),
            stretch_order: p.stretch_order,
            shrink_order: p.shrink_order,
        };
        out.push(Node::Glue(g));
    }

    fn run_math_token(&mut self, t: Token) {
        if t.is_cs() {
            match self.eqtb.resolve(t.cs_id()).cloned() {
                Some(Equiv::Prim(Prim::Delimiter)) => {
                    let v = self.scan_int();
                    if v < 0 || v >= 0x8000000 {
                        self.error("Invalid delimiter code");
                        return;
                    }
                    let (sf, sc, lf, lc) = delim_code_parts(v);
                    self.append_mlist_node(Node::DelimBox {
                        small: (sf, sc),
                        large: (lf, lc),
                        size: 2,
                    });
                    return;
                }
                Some(Equiv::Prim(Prim::Middle)) => {
                    let v = self.scan_delim_int();
                    let (sf, sc, lf, lc) = delim_code_parts(v);
                    self.append_mlist_node(Node::DelimBox {
                        small: (sf, sc),
                        large: (lf, lc),
                        size: 2,
                    });
                    return;
                }
                // \mathchardef'd control sequences never reach main_dispatch
                // (control.rs has no arm for the equiv), so materialize them here
                Some(Equiv::MathCharDef(v)) if self.mode.is_m() => {
                    self.append_mathchar(v);
                    return;
                }
                _ => {}
            }
        }
        self.dispatch(t);
    }

    fn convert_atom(&self, n: &Node, style: GStyle, math_text_char: bool) -> NodeList {
        match n {
            Node::MathChar { fam, c, class } => {
                if *fam == 255 {
                    vec![]
                } else if *class == CL_OP {
                    let (b, _) = self.op_char_box(*fam, *c, style, false);
                    vec![b]
                } else {
                    // tex.web fetch(A): the char node is created from the fam
                    // font unconditionally — a nullfont/missing-char fam gives
                    // a zero-width glyph, never a dropped atom (which would
                    // silently lose the math and its spacing).
                    let fid = self.eqtb.style_fonts[font_size(style)][*fam as usize];
                    let mut out = vec![Node::Char { c: *c, font: fid }];
                    // tex.web §14865: `math_text_char` suppresses italic
                    // correction only in a text font. A final/standalone
                    // math char retains its correction even in that font.
                    if let Some(f) = self.eqtb.fonts.get(fid as usize) {
                        let ic = f.char_italic(*c);
                        if ic != 0 && !(math_text_char && f.space() != 0) {
                            out.push(Node::Kern(ic));
                        }
                    }
                    out
                }
            }
            Node::Scripts { nucleus, sup, sub } => {
                // TeX make_math_accent: scripts on a single-character accent
                // attach to the character, not the taller accent box. Ordinary
                // groups containing a lone accent preserve that noad identity.
                if sup.is_some() || sub.is_some() {
                    if let Some((accent, body)) = accent_noad_of(nucleus) {
                        if matches!(body, [Node::MathChar { fam, .. }] if *fam != 255) {
                            return self.make_accent(
                                accent,
                                body,
                                style,
                                sup.as_deref(),
                                sub.as_deref(),
                            );
                        }
                    }
                }
                self.make_scripts(nucleus, sup.as_deref(), sub.as_deref(), style)
            }
            Node::OpLimits { op, above, below } => {
                // fam-255 CL_OP marker = tex.web noad subtype (append_script):
                // 1=limits forces the above/below construction in every
                // style, 2=no_limits pins side scripts even in display,
                // 0=normal defers to make_op's promotion test on the ACTUAL
                // conversion style (`(subtype=normal) and (cur_style<
                // text_style)`), so a display-parsed sum in a text-style
                // denominator gets side scripts.
                let sub_type = match op.first() {
                    Some(Node::MathChar {
                        fam: 255,
                        c,
                        class: CL_OP,
                    }) => Some(*c),
                    _ => None,
                };
                let payload = if sub_type.is_some() {
                    &op[1..]
                } else {
                    op.as_slice()
                };
                let want_limits = match sub_type {
                    Some(1) => true,
                    Some(2) => false,
                    _ => style < 2,
                };
                if want_limits {
                    self.make_op_limits(payload, above.as_deref(), below.as_deref(), style, true)
                } else {
                    self.make_scripts(payload, above.as_deref(), below.as_deref(), style)
                }
            }
            Node::Frac {
                num,
                den,
                thickness,
                left,
                right,
            } => self.make_fraction(num, den, *thickness, *left, *right, style),
            Node::Radical {
                body, thickness, ..
            } => self.make_radical(body, *thickness, style),
            Node::Accent { fam, c, body } => self.make_accent((*fam, *c), body, style, None, None),
            Node::DelimBox { small, large, .. } => {
                // plain delimiter atom (size 2); 0/1 only reach here as strays
                let code = delim_code_of(*small, *large);
                let mut out = self.var_delimiter(code, 0, style);
                if out.is_empty() {
                    out.push(Node::Kern(0));
                }
                out
            }
            Node::VCenter { box_node } => {
                let mut b = (**box_node).clone();
                let (h, d) = match &b {
                    Node::Box { h, d, .. } => (*h as i64, *d as i64),
                    _ => (0, 0),
                };
                let delta = h + d;
                let axis = self.axis_height(style) as i64;
                let new_h = axis + half_i(delta as i32) as i64;
                let new_d = delta - new_h;
                if let Node::Box { h, d, .. } = &mut b {
                    *h = new_h as i32;
                    *d = new_d as i32;
                }
                vec![b]
            }
            Node::Box { .. } => vec![n.clone()],
            other => vec![other.clone()],
        }
    }

    // ---------- make_scripts (tex.web §745-746) ----------

    /// box a single math-char operator (tex.web `make_op`): display-style
    /// "next larger" chain, width including the italic correction (removed
    /// again when a subscript tucks under), ink centered on the math axis.
    /// Returns the box and the italic correction.
    fn op_char_box(&self, fam: u8, c: u8, style: GStyle, sub_present: bool) -> (Node, i32) {
        let Some((fid, f)) = self.fam_font(style, fam) else {
            return (Node::Kern(0), 0);
        };
        let mut c = c;
        if style < 2 {
            if let Some(ci) = f.chars.get(c as usize) {
                if ci.tag == TAG_LIST {
                    let next = ci.remainder;
                    if next != c && f.exists_char(next) {
                        c = next;
                    }
                }
            }
        }
        let delta = if f.exists_char(c) {
            f.char_italic(c)
        } else {
            0
        };
        let mut b = hpack(vec![Node::Char { c, font: fid }], None, HBOX, &self.eqtb).node;
        if let Node::Box { w, h, d, shift, .. } = &mut b {
            *w += delta;
            *shift = (*h - *d) / 2 - self.axis_height(style);
        }
        if sub_present {
            if let Node::Box { w, .. } = &mut b {
                *w -= delta;
            }
        }
        (b, delta)
    }

    /// operator box from an arbitrary atom (char op, delimiter op, or other)
    fn build_op_box(&self, op: &[Node], style: GStyle) -> (Node, i32) {
        match op {
            [Node::MathChar { fam, c, class }] if *class == CL_OP => {
                if *fam == 255 {
                    (hpack(Vec::new(), None, HBOX, &self.eqtb).node, 0)
                } else {
                    self.op_char_box(*fam, *c, style, false)
                }
            }
            [Node::DelimBox {
                small,
                large,
                size: 2,
            }] => {
                let code = delim_code_of(*small, *large);
                let mut out = self.var_delimiter(code, 0, style);
                if out.len() == 1 {
                    (out.pop().unwrap(), 0)
                } else {
                    (hpack(out, None, HBOX, &self.eqtb).node, 0)
                }
            }
            _ => {
                // box nucleus (`\mathop{...} group`): tex.web make_op only
                // fetches a CHARACTER nucleus and axis-centers that one; a
                // sub_mlist/sub_box nucleus is boxed with shift 0
                let nodes = self.mlist_to_hlist_pen(op, style, self.math_penalties.get());
                let b = hpack(nodes, None, HBOX, &self.eqtb).node;
                (b, 0)
            }
        }
    }

    /// tex.web `clean_box`, including its singleton-character optimization:
    /// discard the sole trailing kern (the character's italic correction).
    fn clean_math_box(&self, list: &[Node], style: GStyle) -> Node {
        let mut nodes = self.mlist_to_hlist_pen(list, style, self.math_penalties.get());
        if matches!(nodes.as_slice(), [Node::Char { .. }, Node::Kern(_)]) {
            nodes.pop();
        }
        if matches!(nodes.as_slice(), [Node::Box { shift: 0, .. }]) {
            return nodes.pop().expect("single clean math box");
        }
        hpack(nodes, None, HBOX, &self.eqtb).node
    }

    fn make_scripts(
        &self,
        nucleus: &[Node],
        sup: Option<&[Node]>,
        sub: Option<&[Node]>,
        style: GStyle,
    ) -> NodeList {
        let ss = self.eqtb.dim_params[DimParam::ScriptSpace.idx() as usize];
        let x = self.math_x_height(style);
        let rt = self.default_rule_thickness(style);
        let mut delta = 0i32;
        let nuc: Node;
        let mut shift_up = 0i32;
        let mut shift_down = 0i32;
        match nucleus {
            // A character nucleus keeps its italic correction as a trailing
            // kern unless a subscript is present; then it offsets the sup.
            [Node::MathChar { fam, c, class }] if *class != CL_OP => {
                if *fam == 255 {
                    // fam255 prefix marker: the nucleus is the REST of the
                    // list (a group). tex.web treats a brace-group nucleus as
                    // an Ord atom with delta = 0 — never take the first
                    // character's italic correction.
                    nuc = hpack(Vec::new(), None, HBOX, &self.eqtb).node;
                } else {
                    let Some((fid, f)) = self.fam_font(style, *fam) else {
                        return vec![];
                    };
                    if !f.exists_char(*c) {
                        return vec![];
                    }
                    // tex.web §14865 (@<Determine the char list...@>): a
                    // no-subscript nucleus keeps its italic correction as a
                    // TRAILING kern in the character list (width grows, delta
                    // resets to 0); with a subscript the ic stays as `delta`
                    // and shifts the superscript right instead.
                    let ic = f.char_italic(*c);
                    let mut core: NodeList = vec![Node::Char { c: *c, font: fid }];
                    if sub.is_none() && ic != 0 {
                        core.push(Node::Kern(ic));
                        delta = 0;
                    } else {
                        delta = ic;
                    }
                    nuc = hpack(core, None, HBOX, &self.eqtb).node;
                }
            }
            // A standalone \delimiter is an ordinary math-character noad:
            // scripts use the character shifts, not sub-box drop parameters.
            [Node::DelimBox {
                small: (fam, c),
                size: 2,
                ..
            }] => {
                let Some((fid, f)) = self.fam_font(style, *fam) else {
                    return vec![];
                };
                if !f.exists_char(*c) {
                    return vec![];
                }
                let ic = f.char_italic(*c);
                let mut core = vec![Node::Char { c: *c, font: fid }];
                if sub.is_none() && ic != 0 {
                    core.push(Node::Kern(ic));
                } else {
                    delta = ic;
                }
                nuc = hpack(core, None, HBOX, &self.eqtb).node;
            }
            // non-limits big operator: axis-centered box, italic handled above
            [Node::MathChar { fam, c, class }] if *class == CL_OP => {
                if *fam == 255 {
                    nuc = hpack(Vec::new(), None, HBOX, &self.eqtb).node;
                } else {
                    let (b, d) = self.op_char_box(*fam, *c, style, sub.is_some());
                    delta = d;
                    nuc = b;
                }
                // tex.web §746: an operator nucleus is already boxed, so
                // sup_drop/sub_drop come from subsidiary size `t`.
                let drop_size = font_size(sup_style(style));
                let (zh, zd) = box_dims_shifted(&nuc);
                shift_up = zh - self.fparam_idx(drop_size, 2, 18);
                shift_down = zd + self.fparam_idx(drop_size, 2, 19);
            }
            // boxed nucleus: initial shifts from its (shift-adjusted) dims
            _ => {
                nuc = self.clean_math_box(nucleus, style);
                let drop_size = font_size(sup_style(style));
                let (zh, zd) = box_dims_shifted(&nuc);
                shift_up = zh - self.fparam_idx(drop_size, 2, 18);
                shift_down = zd + self.fparam_idx(drop_size, 2, 19);
            }
        }
        let mut out: NodeList = vec![nuc];
        let sup1 = self.fparam(style, 2, 13);
        let sup2 = self.fparam(style, 2, 14);
        let sup3 = self.fparam(style, 2, 15);
        let sub1 = self.fparam(style, 2, 16);
        let sub2 = self.fparam(style, 2, 17);
        let mut sup_box: Option<Node> = None;
        let mut sub_box: Option<Node> = None;
        if let Some(s) = sup {
            let mut nodes = self.mlist_to_hlist_pen(s, sup_style(style), self.math_penalties.get());
            if ss != 0 {
                nodes.push(Node::Kern(ss)); // \scriptspace widens the box
            }
            let b = hpack(nodes, None, HBOX, &self.eqtb).node;
            let (_, _, bd) = box_dims(&b);
            let mut clr = if style & 1 == 1 {
                sup3
            } else if style < 2 {
                sup1
            } else {
                sup2
            };
            if shift_up < clr {
                shift_up = clr;
            }
            clr = bd + x / 4;
            if shift_up < clr {
                shift_up = clr;
            }
            sup_box = Some(b);
        }
        if let Some(s) = sub {
            let mut nodes = self.mlist_to_hlist_pen(s, sub_style(style), self.math_penalties.get());
            if ss != 0 {
                nodes.push(Node::Kern(ss));
            }
            let b = hpack(nodes, None, HBOX, &self.eqtb).node;
            let (_, bh, _) = box_dims(&b);
            if sup_box.is_none() {
                if shift_down < sub1 {
                    shift_down = sub1;
                }
                let clr = bh - (x * 4) / 5;
                if shift_down < clr {
                    shift_down = clr;
                }
            }
            sub_box = Some(b);
        }
        if let (Some(bx), Some(by)) = (&sup_box, &sub_box) {
            // both scripts: sub2 floor, then the 4*rule-thickness clearance
            let (_, _, sup_d) = box_dims(bx);
            let (sub_w_, sub_h, _) = box_dims(by);
            let _ = sub_w_;
            if shift_down < sub2 {
                shift_down = sub2;
            }
            let mut clr = rt * 4 - ((shift_up - sup_d) - (sub_h - shift_down));
            if clr > 0 {
                shift_down += clr;
                clr = (x * 4) / 5 - (shift_up - sup_d);
                if clr > 0 {
                    shift_up += clr;
                    shift_down -= clr;
                }
            }
        }

        match (sup_box, sub_box) {
            (Some(bs), Some(bb)) => {
                let (_, _, sup_d) = box_dims(&bs);
                let (_, sub_h, _) = box_dims(&bb);
                let k = (shift_up - sup_d) - (sub_h - shift_down);
                let mut sbs = bs;
                if let Node::Box { shift, .. } = &mut sbs {
                    *shift = delta; // superscript offset (tex.web §746)
                }
                let mut vlist: NodeList = vec![sbs];
                vlist.push(Node::Kern(k));
                vlist.push(bb);
                let mut v = vpack(vlist, None, VBOX, &self.eqtb).node;
                if let Node::Box { shift, .. } = &mut v {
                    *shift = shift_down;
                }
                out.push(v);
            }
            (Some(mut b), None) => {
                if let Node::Box { shift, .. } = &mut b {
                    *shift = -shift_up;
                }
                out.push(b);
            }
            (None, Some(mut b)) => {
                if let Node::Box { shift, .. } = &mut b {
                    *shift = shift_down;
                }
                out.push(b);
            }
            (None, None) => {}
        }
        out
    }

    // ---------- make_op with limits (tex.web §744) ----------

    fn make_op_limits(
        &self,
        op: &[Node],
        above: Option<&[Node]>,
        below: Option<&[Node]>,
        style: GStyle,
        _force: bool,
    ) -> NodeList {
        let (op_box, delta) = self.build_op_box(op, style);
        if above.is_none() && below.is_none() {
            return vec![op_box];
        }
        let sp1 = self.fparam(style, 3, 9);
        let sp2 = self.fparam(style, 3, 10);
        let sp3 = self.fparam(style, 3, 11);
        let sp4 = self.fparam(style, 3, 12);
        let sp5 = self.fparam(style, 3, 13);
        let (oh, od) = box_dims_shifted(&op_box);
        let sup_box = above.map(|s| {
            hpack(
                self.mlist_to_hlist_pen(s, sup_style(style), self.math_penalties.get()),
                None,
                HBOX,
                &self.eqtb,
            )
            .node
        });
        let sub_box = below.map(|s| {
            hpack(
                self.mlist_to_hlist_pen(s, sub_style(style), self.math_penalties.get()),
                None,
                HBOX,
                &self.eqtb,
            )
            .node
        });
        let mut w = self.box_w(&op_box);
        if let Some(b) = &sup_box {
            w = w.max(self.box_w(b));
        }
        if let Some(b) = &sub_box {
            w = w.max(self.box_w(b));
        }
        let mut vlist: NodeList = Vec::new();
        let mut su = 0i32;
        let mut sd = 0i32;
        if let Some(b) = &sup_box {
            let (_, _, bd) = box_dims(b);
            su = sp3 - bd;
            if su < sp1 {
                su = sp1;
            }
            vlist.push(Node::Kern(sp5));
            let mut bc = self.center_to_w(b.clone(), w);
            if let Node::Box { shift, .. } = &mut bc {
                *shift = half_i(delta); // limits skewed by half the italic
            }
            vlist.push(bc);
            vlist.push(Node::Kern(su));
        }
        let op_centered = self.center_to_w(op_box, w);
        vlist.push(op_centered);
        if let Some(b) = &sub_box {
            let (_, bh, _) = box_dims(b);
            sd = sp4 - bh;
            if sd < sp2 {
                sd = sp2;
            }
            vlist.push(Node::Kern(sd));
            let mut bc = self.center_to_w(b.clone(), w);
            if let Node::Box { shift, .. } = &mut bc {
                *shift = -half_i(delta);
            }
            vlist.push(bc);
            vlist.push(Node::Kern(sp5));
        }
        let mut packed = vpack(vlist, None, VBOX, &self.eqtb).node;
        // declared dims (tex.web): the op's baseline is the box baseline
        let mut h = oh;
        let mut d = od;
        if let Some(b) = &sup_box {
            let (_, bh, bd) = box_dims(b);
            h += sp5 + bh + bd + su;
        }
        if let Some(b) = &sub_box {
            let (_, bh, bd) = box_dims(b);
            d += sp5 + bh + bd + sd;
        }
        if let Node::Box {
            w: bw,
            h: hh,
            d: dd,
            shift,
            ..
        } = &mut packed
        {
            *bw = w;
            *hh = h;
            *dd = d;
            *shift = 0;
        }
        vec![packed]
    }

    /// tex.web rebox: pad the narrower num/den box to the common width with
    /// ss_glue (`0pt plus 1fil minus 1fil`) on each side, exactly like
    /// rebox's `new_glue(ss_glue)` wrapping and exact-width hpack.
    fn center_to_w(&self, b: Node, w: i32) -> Node {
        if self.box_w(&b) == w {
            return b;
        }
        let ss = || {
            Node::Glue(Glue {
                width: 0,
                stretch: ONE,
                shrink: ONE,
                stretch_order: crate::boxes::GLUE_FIL,
                shrink_order: crate::boxes::GLUE_FIL,
            })
        };
        hpack(vec![ss(), b, ss()], Some(w), HBOX, &self.eqtb).node
    }
    fn box_w(&self, n: &Node) -> i32 {
        match n {
            Node::Box { w, .. } => *w,
            Node::Char { c, font } => self
                .eqtb
                .fonts
                .get(*font as usize)
                .map(|f| f.char_width(*c))
                .unwrap_or(0),
            Node::Rule { width, .. } => *width,
            Node::Kern(k) => *k,
            Node::Glue(g) => g.width,
            _ => 0,
        }
    }

    // ---------- make_fraction (tex.web §741-742) ----------

    fn make_fraction(
        &self,
        num: &[Node],
        den: &[Node],
        thickness: i32,
        left: Option<i32>,
        right: Option<i32>,
        style: GStyle,
    ) -> NodeList {
        let r = if thickness < 0 {
            self.default_rule_thickness(style)
        } else {
            thickness
        };
        let num_box = hpack(
            self.mlist_to_hlist_pen(num, num_style(style), self.math_penalties.get()),
            None,
            HBOX,
            &self.eqtb,
        )
        .node;
        let den_box = hpack(
            self.mlist_to_hlist_pen(den, den_style(style), self.math_penalties.get()),
            None,
            HBOX,
            &self.eqtb,
        )
        .node;
        let w = self.box_w(&num_box).max(self.box_w(&den_box));
        let num_c = self.center_to_w(num_box, w);
        let den_c = self.center_to_w(den_box, w);
        let (_, nh, nd) = box_dims(&num_c);
        let (_, dh, dd) = box_dims(&den_c);
        let axis = self.axis_height(style);
        let display = style < 2;
        let mut su = if display {
            self.fparam(style, 2, 8) // num1
        } else if r != 0 {
            self.fparam(style, 2, 9) // num2
        } else {
            self.fparam(style, 2, 10) // num3
        };
        let mut sd = if display {
            self.fparam(style, 2, 11) // denom1
        } else {
            self.fparam(style, 2, 12) // denom2
        };
        let vlist: NodeList;
        if r == 0 {
            // \atop: symmetric minimum clearance around the numerator/denominator
            let rt = self.default_rule_thickness(style);
            let clr = if display { rt * 7 } else { rt * 3 };
            let delta = half_i(clr - ((su - nd) - (dh - sd)));
            if delta > 0 {
                su += delta;
                sd += delta;
            }
            vlist = vec![num_c, Node::Kern((su - nd) - (dh - sd)), den_c];
        } else {
            // fraction rule: clearances measured from the axis
            let dr = r / 2;
            let rt = self.default_rule_thickness(style);
            let clr = if display { 3 * rt } else { rt };
            let d1 = clr - ((su - nd) - (axis + dr));
            if d1 > 0 {
                su += d1;
            }
            let d2 = clr - ((axis - dr) - (dh - sd));
            if d2 > 0 {
                sd += d2;
            }
            vlist = vec![
                num_c,
                Node::Kern((su - nd) - (axis + dr)),
                Node::Rule {
                    width: w,
                    height: r,
                    depth: 0,
                },
                Node::Kern((axis - dr) - (dh - sd)),
                den_c,
            ];
        }
        let mut packed = vpack(vlist, None, VBOX, &self.eqtb).node;
        // tex.web §742: height = shift_up + height(numerator), depth =
        // depth(denominator) + shift_down; the baseline is the numerator's
        if let Node::Box {
            w: bw, h, d, shift, ..
        } = &mut packed
        {
            *bw = w;
            *h = su + nh;
            *d = dd + sd;
            *shift = 0;
        }
        // \overwithdelims etc.: both delimiters sized to delim1/delim2
        let dd_size = if display {
            self.fparam(style, 2, 20)
        } else {
            self.fparam(style, 2, 21)
        };
        let mut out: NodeList = Vec::new();
        if let Some(l) = left {
            out.extend(self.var_delimiter(l, dd_size, style));
        } else {
            out.push(self.null_delimiter_box(style));
        }
        out.push(packed);
        if let Some(rr) = right {
            out.extend(self.var_delimiter(rr, dd_size, style));
        } else {
            out.push(self.null_delimiter_box(style));
        }
        out
    }

    /// tex.web make_radical (§752-753): the body is boxed in the cramped
    /// style; the surd is chosen for h+d+clr+rt; excess surd depth widens the
    /// clearance by half; the surd baseline drops to -(h+clr) and the
    /// overbar box = [kern(surd_h), rule(surd_h), kern(clr), body].
    fn make_radical(&self, body: &[Node], thickness: i32, style: GStyle) -> NodeList {
        let (delim_code, r_explicit) = unpack_radical(thickness);
        let body_nodes = self.mlist_to_hlist_pen(body, style | 1, self.math_penalties.get());
        let x = hpack(body_nodes, None, HBOX, &self.eqtb).node;
        let (xw, xh, xd) = box_dims(&x);
        let rt = if r_explicit >= 0 {
            r_explicit
        } else {
            self.default_rule_thickness(style)
        };
        let x_h = self.math_x_height(style);
        let mut clr = if style < 2 {
            rt + (x_h / 4).abs()
        } else {
            rt + rt / 4
        };
        let target_size = xh + xd + clr + rt;
        let mut d_nodes = self.var_delimiter(delim_code, target_size, style);
        let mut d_box = if d_nodes.len() == 1 {
            d_nodes.pop().unwrap()
        } else {
            hpack(d_nodes, None, HBOX, &self.eqtb).node
        };
        let (_, dh, dd) = box_dims(&d_box);
        let delta = dd - (xh + xd + clr);
        if delta > 0 {
            clr += half_i(delta);
        }
        // overbar(b, k=clr, t=surd height): [kern(t), rule(t), kern(clr), body]
        let vlist = vec![
            Node::Kern(dh),
            Node::Rule {
                width: xw,
                height: dh,
                depth: 0,
            },
            Node::Kern(clr),
            x,
        ];
        let v = vpack(vlist, None, VBOX, &self.eqtb).node;
        if let Node::Box { shift, .. } = &mut d_box {
            *shift = -(xh + clr);
        }
        let mut out = NodeList::new();
        out.push(d_box);
        out.push(v);
        out
    }

    fn make_accent(
        &self,
        accent: (u8, u8),
        body: &[Node],
        style: GStyle,
        sup: Option<&[Node]>,
        sub: Option<&[Node]>,
    ) -> NodeList {
        let (afam, mut ac) = accent;
        let has_scripts = sup.is_some() || sub.is_some();
        let Some((afid, af)) = self.fam_font(style, afam) else {
            // TeX keeps the nucleus (and any scripts) when the accent font is
            // unavailable.
            if has_scripts {
                return self.make_scripts(body, sup, sub, style);
            }
            let body_nodes = self.mlist_to_hlist_pen(body, style | 1, self.math_penalties.get());
            return vec![hpack(body_nodes, None, HBOX, &self.eqtb).node];
        };
        // skew: kern from the nucleus font's character to its \skewchar
        let s = match body {
            [Node::MathChar { fam, c, .. }] => {
                if let Some((fid, f)) = self.fam_font(style | 1, *fam) {
                    let sk = self.skew_char_of(fid, &f);
                    if sk >= 0 && sk <= 255 {
                        self.char_kern(fid, &f, *c, sk as u8)
                    } else {
                        0
                    }
                } else {
                    0
                }
            }
            [Node::Char { c, font }] => {
                if let Some(f) = self.eqtb.fonts.get(*font as usize) {
                    let sk = self.skew_char_of(*font, f);
                    if sk >= 0 && sk <= 255 {
                        self.char_kern(*font, f, *c, sk as u8)
                    } else {
                        0
                    }
                } else {
                    0
                }
            }
            _ => 0,
        };
        let body_nodes = self.mlist_to_hlist_pen(body, style | 1, self.math_penalties.get());
        let body_box = hpack(body_nodes, None, HBOX, &self.eqtb).node;
        let (bw, mut bh, _bd) = box_dims(&body_box);
        let mut aw = af.char_width(ac);
        // TeX chooses the largest next-larger accent that fits the nucleus,
        // before scripts widen it. Bound traversal like var_delimiter.
        for _ in 0..256 {
            let Some(ci) = af.chars.get(ac as usize) else {
                break;
            };
            if ci.tag != TAG_LIST {
                break;
            }
            let next = ci.remainder;
            if next == ac || !af.exists_char(next) {
                break;
            }
            let next_width = af.char_width(next);
            if next_width > bw {
                break;
            }
            ac = next;
            aw = next_width;
        }
        let axh = {
            let v = af.param(5);
            if v != 0 {
                v
            } else {
                af.x_height()
            }
        };
        let mut delta = if bh < axh { bh } else { axh };
        // TeX's script swap increases the overlap by the scripted box's added
        // height. Horizontal accent placement keeps the original nucleus width.
        let mut xw = bw;
        let x_box = if has_scripts && matches!(body, [Node::MathChar { fam, .. }] if *fam != 255) {
            let inner = self.make_scripts(body, sup, sub, style);
            let x = hpack(inner, None, HBOX, &self.eqtb).node;
            let (w2, h2, _) = box_dims(&x);
            delta += h2 - bh;
            bh = h2;
            xw = w2;
            x
        } else {
            body_box
        };
        let mut acc_box = hpack(
            vec![Node::Char { c: ac, font: afid }],
            None,
            HBOX,
            &self.eqtb,
        )
        .node;
        if let Node::Box { w, shift, .. } = &mut acc_box {
            *w = 0; // accent width does not affect the box width
            *shift = s + half_i(bw - aw); // horizontal shift inside the vlist
        }
        let mut v = vpack(
            vec![acc_box, Node::Kern(-delta), x_box],
            None,
            VBOX,
            &self.eqtb,
        )
        .node;
        if let Node::Box { w, .. } = &mut v {
            *w = xw; // tex.web: width(y):=width(x) after the vpack
        }
        let (_, vh, _) = box_dims(&v);
        if vh < bh {
            if let Node::Box { list, h, .. } = &mut v {
                list.insert(0, Node::Kern(bh - vh));
                *h = bh;
            }
        }
        vec![v]
    }

    fn char_kern(&self, fid: FontId, f: &Font, c: u8, to: u8) -> i32 {
        // follow the lig/kern program for c looking for a kern to `to`
        if !f.exists_char(c) || !f.exists_char(to) {
            return 0;
        }
        let ci = match f.chars.get(c as usize) {
            Some(ci) if ci.tag == crate::tfm::TAG_LIG => ci,
            _ => return 0,
        };
        let mut k = ci.remainder as usize;
        // tex.web §10618 restart: first instruction with skip_byte > 128
        // redirects the program start to 256*op_byte + rem_byte.
        if let Some(first) = f.lig_kern.get(k) {
            if first.skip > 128 {
                k = 256 * first.op as usize + first.rem as usize;
            }
        }
        for _ in 0..(f.lig_kern.len() + 1) {
            let Some(step) = f.lig_kern.get(k) else {
                return 0;
            };
            if step.next_char == to {
                let op = step.op as usize;
                if op >= 128 {
                    let idx = (op - 128) * 256 + step.rem as usize;
                    return f.kerns.get(idx).copied().unwrap_or(0);
                }
                return 0;
            }
            if step.stop {
                return 0;
            }
            k += 1 + step.skip as usize;
            if k >= f.lig_kern.len() {
                return 0;
            }
        }
        let _ = fid;
        0
    }

    // ---------- var_delimiter (tex.web §716-723) ----------

    /// null delimiter: an empty box of width \nulldelimiterspace whose ink
    /// baseline is centered on the math axis (tex.web var_delimiter tail).
    fn null_delimiter_box(&self, style: GStyle) -> Node {
        let nd = self.eqtb.dim_params[DimParam::NullDelimiterSpace.idx() as usize];
        let mut b = hpack(Vec::new(), Some(nd), HBOX, &self.eqtb).node;
        if let Node::Box { shift, .. } = &mut b {
            *shift = -self.axis_height(style);
        }
        b
    }

    /// box a single delimiter character: width includes the italic
    /// correction, ink centered on the math axis (tex.web char_box + tail).
    fn delim_char_box(&self, size_idx: usize, fam: u8, c: u8, style: GStyle) -> Node {
        let fid = self.eqtb.style_fonts[size_idx][fam as usize];
        let it = self
            .eqtb
            .fonts
            .get(fid as usize)
            .map(|f| f.char_italic(c))
            .unwrap_or(0);
        let mut b = hpack(vec![Node::Char { c, font: fid }], None, HBOX, &self.eqtb).node;
        if let Node::Box { w, h, d, shift, .. } = &mut b {
            *w += it;
            *shift = (*h - *d) / 2 - self.axis_height(style);
        }
        b
    }

    /// Choose the smallest delimiter variant whose height+depth reaches `v`
    /// (tex.web var_delimiter: no shortfall/factor here — those live in
    /// make_left_right). Search: small char (following `next larger` chains),
    /// then the large char, each at the current size and smaller.
    fn var_delimiter(&self, code: i32, v: i32, style: GStyle) -> NodeList {
        if code <= 0 {
            return vec![self.null_delimiter_box(style)];
        }
        let (sf, sc, lf, lc) = delim_code_parts(code);
        let cur_size = font_size(style);
        let mut best: Option<(usize, u8, u8)> = None;
        let mut found: Option<(usize, u8, u8)> = None;
        let mut w = 0i32;
        'parts: for (fam, first) in [(sf as u8, sc as u8), (lf as u8, lc as u8)] {
            if fam == 0 && first == 0 {
                continue;
            }
            for sz in (0..=cur_size).rev() {
                // sz IS the style_fonts size index (tex.web walks `fam +
                // cur_size` down to `fam`): fam_font() would push it
                // through font_size() and map script sizes back onto the
                // text/script fonts (style 2/4), not the size sz font.
                let Some((_, f)) = self.fam_font_idx(sz, fam) else {
                    continue;
                };
                let mut c = first;
                let mut steps = 0usize;
                loop {
                    steps += 1;
                    if steps > 256 {
                        break;
                    }
                    let Some(ci) = f.chars.get(c as usize) else {
                        break;
                    };
                    if !f.exists_char(c) {
                        break;
                    }
                    if ci.tag == TAG_EXT {
                        found = Some((sz, fam, c));
                        break 'parts;
                    }
                    let u = f.char_height(c) + f.char_depth(c);
                    if u > w {
                        best = Some((sz, fam, c));
                        w = u;
                        if u >= v {
                            found = Some((sz, fam, c));
                            break 'parts;
                        }
                    }
                    if ci.tag == TAG_LIST {
                        let next = ci.remainder;
                        if next == c {
                            break;
                        }
                        c = next;
                        continue;
                    }
                    break;
                }
            }
        }
        match found.or(best) {
            Some((sz, fam, c)) => {
                let f = self
                    .eqtb
                    .fonts
                    .get(self.eqtb.style_fonts[sz][fam as usize] as usize)
                    .cloned();
                let ext_rec = f.as_ref().and_then(|ff| {
                    ff.chars
                        .get(c as usize)
                        .filter(|ci| ci.tag == TAG_EXT)
                        .and_then(|ci| ff.ext.get(ci.remainder as usize).cloned())
                });
                match ext_rec {
                    Some(rec) => vec![self.make_extensible(sz, fam, rec, v, style)],
                    None => vec![self.delim_char_box(sz, fam, c, style)],
                }
            }
            None => vec![self.null_delimiter_box(style)],
        }
    }

    /// tex.web make_extensible: stack top / n x rep / mid / n x rep / bottom
    /// with n grown until the total extent reaches `v` (in pairs when a mid
    /// part exists). Height = top part's height, depth = w - height.
    fn make_extensible(
        &self,
        size_idx: usize,
        fam: u8,
        rec: crate::tfm::ExtRecipe,
        v: i32,
        style: GStyle,
    ) -> Node {
        // `size_idx` is the style_fonts size index chosen by var_delimiter
        // (fam + size, tex.web); fam_font() would re-derive it from a style
        // code and collapse script sizes onto the text font.
        let (fid, f) = match self.fam_font_idx(size_idx, fam) {
            Some(v2) => v2,
            None => return self.null_delimiter_box(style),
        };
        let glyph = |ch: u8| -> Option<(Node, i32, i32)> {
            if ch == 0 || !f.exists_char(ch) {
                return None;
            }
            let n = hpack(
                vec![Node::Char { c: ch, font: fid }],
                None,
                HBOX,
                &self.eqtb,
            )
            .node;
            let (w, h, d) = box_dims(&n);
            Some((n, w, h + d))
        };
        let top = glyph(rec.top);
        let mid = glyph(rec.mid);
        let bot = glyph(rec.bot);
        let rep = glyph(rec.rep);
        let (rep_u, rep_w) = match &rep {
            Some((_, rw, u)) => (*u, *rw),
            None => (0, 0),
        };
        // minimum extent without repetitions; width comes from the rep part
        let mut wacc = 0i64;
        for g in [&top, &mid, &bot] {
            if let Some((_, _, u)) = g {
                wacc += *u as i64;
            }
        }
        let has_mid = mid.is_some();
        let mut n: i64 = 0;
        if rep_u > 0 {
            while wacc < v as i64 {
                wacc += rep_u as i64;
                n += 1;
                if has_mid {
                    wacc += rep_u as i64;
                }
            }
        }
        let mut vlist: NodeList = Vec::new();
        if let Some((node, _, _)) = top {
            vlist.push(node);
        }
        if let Some((node, _, _)) = &rep {
            for _ in 0..n {
                vlist.push(node.clone());
            }
        }
        if let Some((node, _, _)) = mid {
            vlist.push(node);
            if let Some((node, _, _)) = &rep {
                for _ in 0..n {
                    vlist.push(node.clone());
                }
            }
        }
        if let Some((node, _, _)) = bot {
            vlist.push(node);
        }
        // height = top-most present part's height; depth fills to w
        let h_top = vlist.first().map(|b| box_dims(b).1).unwrap_or(0);
        let total = wacc as i32;
        let mut b = vpack(vlist, None, VBOX, &self.eqtb).node;
        if let Node::Box { w, h, d, shift, .. } = &mut b {
            *w = rep_w;
            *h = h_top;
            *d = (total - h_top).max(0);
            *shift = (*h - *d) / 2 - self.axis_height(style);
        }
        b
    }
}

// ---------- small local helpers ----------

#[inline]
fn delim_code_of(small: (u8, u8), large: (u8, u8)) -> i32 {
    ((small.0 as i32) << 20) | ((small.1 as i32) << 12) | ((large.0 as i32) << 8) | (large.1 as i32)
}

#[inline]
fn half_i(x: i32) -> i32 {
    (x + 1) / 2
}

/// (width, height, depth) of a box
#[inline]
fn box_dims(n: &Node) -> (i32, i32, i32) {
    match n {
        Node::Box { w, h, d, .. } => (*w, *h, *d),
        _ => (0, 0, 0),
    }
}

/// (height, depth) of a box with the hpack shift adjustment applied
/// (tex.web hpack: h - shift, d + shift)
#[inline]
fn box_dims_shifted(n: &Node) -> (i32, i32) {
    match n {
        Node::Box { h, d, shift, .. } => ((*h - *shift).max(0), (*d + *shift).max(0)),
        _ => (0, 0),
    }
}

/// Preserve TeX's ordinary-group unwrap for a lone accent noad.
/// Explicit non-ordinary class markers must not be unwrapped.
fn accent_noad_of(nucleus: &[Node]) -> Option<((u8, u8), &[Node])> {
    let accent = match nucleus {
        [a @ Node::Accent { .. }] => a,
        [Node::MathChar {
            fam: 255,
            class: CL_ORD,
            ..
        }, a @ Node::Accent { .. }] => a,
        _ => return None,
    };
    match accent {
        Node::Accent { fam, c, body } => Some(((*fam, *c), body)),
        _ => None,
    }
}

// =====================================================================
// Tests: layout is checked against dims captured from real TeX
// (`tex \showbox` oracles on the same CM fonts at 10/7/5 pt), with the
// plain TeX family setup fam0=cmr, fam1=cmmi, fam2=cmsy, fam3=cmex.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;

    fn su(pt: f64) -> i32 {
        (pt * 65536.0).round() as i32
    }

    /// plain-TeX math preamble: families, mathcodes, delcodes, macros
    pub(crate) fn math_preamble() -> String {
        let mut s = String::from(
            "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\$=3 \\catcode`\\^=7 \\catcode`\\_=8 \\catcode`\\#=6\n",
        );
        for c in b'a'..=b'z' {
            s.push_str(&format!("\\mathcode`{}={}\n", c as char, 0x7100 + c as i32));
        }
        for c in b'A'..=b'Z' {
            s.push_str(&format!("\\mathcode`{}={}\n", c as char, 0x7100 + c as i32));
        }
        for c in b'0'..=b'9' {
            s.push_str(&format!("\\mathcode`{}={}\n", c as char, 0x7000 + c as i32));
        }
        s.push_str(concat!(
            "\\mathcode`\\==\"303D \\mathcode`\\+=\"202B \\mathcode`\\-=\"2200\n",
            "\\mathcode`\\(=\"4028 \\mathcode`\\)=\"5029\n",
            "\\delcode`\\(=\"028300 \\delcode`\\)=\"029301 \\scriptspace=0.5pt \\nulldelimiterspace=1.2pt\n",
            "\\font\\tenrm=cmr10 \\font\\teni=cmmi10 \\font\\tensy=cmsy10 \\font\\tenex=cmex10\n",
            "\\font\\sevenrm=cmr7 \\font\\seveni=cmmi7 \\font\\sevensy=cmsy7\n",
            "\\font\\fiverm=cmr5 \\font\\fivei=cmmi5 \\font\\fivesy=cmsy5\n",
            "\\textfont0=\\tenrm \\scriptfont0=\\sevenrm \\scriptscriptfont0=\\fiverm\n",
            "\\textfont1=\\teni \\scriptfont1=\\seveni \\scriptscriptfont1=\\fivei\n",
            "\\textfont2=\\tensy \\scriptfont2=\\sevensy \\scriptscriptfont2=\\fivesy\n",
            "\\textfont3=\\tenex \\scriptfont3=\\tenex \\scriptscriptfont3=\\tenex\n",
            "\\mathchardef\\sumG=\"1350 \\mathchardef\\intG=\"1352\n",
            "\\mathchardef\\dagger=\"2279 \\mathchardef\\lambda=\"0115 \\mathchardef\\theta=\"0112\n",
            "\\def\\sqrtG#1{\\radical\"270370 {#1}}\n\\def\\sqrtS#1{\\radical \"270370 {#1}}\n\\def\\sqrtB#1{{\\radical\"270370 #1}}\n\\def\\tmpA#1{#1}\n",
            "\\def\\fracG#1#2{{\\mathchoice{{#1\\over #2}}{{#1\\over #2}}{{#1\\over #2}}{{#1\\over #2}}}}\n",
            "\\def\\barG#1{\\mathaccent \"7016 {#1}}\n",
        ));
        s
    }

    /// run a complete document (preamble + body) in one engine pass: the
    /// job ends when the first pushed file is exhausted, so setup and body
    /// must share a file
    pub(crate) fn run_doc(body: &str) -> Engine {
        let mut e = Engine::new(true);
        e.init_primitives();
        e.add_nullfont();
        let full = format!("{}{}\n", math_preamble(), body);
        e.input
            .push_file("mathtest.tex".to_string(), full.into_bytes());
        e.run();
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        e
    }

    /// the preamble source on its own (kept for setup-only checks)
    fn math_engine() -> Engine {
        run_doc("")
    }

    /// run `$...$` inside an \hbox (text style) and return the packed hbox
    fn text_math(src: &str) -> Node {
        let e = run_doc(&format!("\\hbox{{${}$}}", src));
        page_or_box255(&e)
    }

    /// page-list hbox, falling back to \box255 registration. An inline
    /// `\hbox{$..$}` wraps the math box one level down — unwrap it.
    fn page_or_box255(e: &Engine) -> Node {
        let all_boxes: Vec<Node> = e
            .page_list
            .iter()
            .chain(e.par_page_lists.iter().flatten())
            .filter(|n| matches!(n, Node::Box { kind, .. } if *kind == HBOX))
            .cloned()
            .collect();
        let mut n = if let Some(n) = all_boxes.into_iter().last() {
            n
        } else if let Some(Some(b)) = e.eqtb.boxed.get(255).cloned() {
            b
        } else {
            panic!(
                "no math hbox: mode={:?} page_list={:?} cur_list={:?} errors={} term={}",
                e.mode, e.page_list, e.cur_list, e.error_count, e.term
            );
        };
        while let Node::Box { kind, list, .. } = &n {
            if *kind == HBOX && list.len() == 1 {
                if let Node::Box { kind: ikind, .. } = &list[0] {
                    if *ikind == HBOX {
                        n = list[0].clone();
                        continue;
                    }
                }
            }
            // inline math now splices into the enclosing hlist (tex.web
            // §22461): the \hbox{$..$} wrapper contains [MathKern, atoms..,
            // MathKern]; repack minus the on/off markers so tests see the
            // formula box
            if *kind == HBOX
                && matches!(list.first(), Some(Node::MathKern(_, 1)))
                && matches!(list.last(), Some(Node::MathKern(_, 2)))
            {
                let inner: NodeList = list[1..list.len() - 1].to_vec();
                return hpack(inner, None, HBOX, &e.eqtb).node;
            }
            break;
        }
        n
    }

    /// run $$...$$ (display style) and return the packed hbox
    fn display_math(src: &str) -> Node {
        let e = run_doc(&format!("$${}$$", src));
        page_or_box255(&e)
    }

    fn box_of(n: &Node) -> (&NodeList, i32, i32, i32, i32) {
        match n {
            Node::Box {
                list,
                w,
                h,
                d,
                shift,
                ..
            } => (list, *w, *h, *d, *shift),
            other => panic!("expected box, got {:?}", other),
        }
    }

    fn box_shift(n: &Node) -> i32 {
        match n {
            Node::Box { shift, .. } => *shift,
            other => panic!("expected box, got {:?}", other),
        }
    }

    fn approx(got: i32, want: f64, what: &str) {
        assert!(
            (got as f64 / 65536.0 - want).abs() < 0.002,
            "{}: got {}pt want {}pt",
            what,
            got as f64 / 65536.0,
            want
        );
    }

    // ---- plain font constants (cmsy10 / cmex10 / cmr7 / cmmi7) ----
    const SUP1: f64 = 0.412892; // cmsy10 fontdimen 13
    const SUP2: f64 = 0.362892; // fontdimen 14
    const SUB1: f64 = 0.15; // fontdimen 16
    const SUB2: f64 = 0.247217; // fontdimen 17
    const NUM1: f64 = 0.676508; // fontdimen 8
    const NUM2: f64 = 0.393732; // fontdimen 9
    const NUM3: f64 = 0.443731; // fontdimen 10
    const DEN1: f64 = 0.685951; // fontdimen 11
    const DEN2: f64 = 0.344841; // fontdimen 12
    const AXIS: f64 = 0.25; // fontdimen 22
    const RT: f64 = 0.039999; // cmex10 fontdimen 8 (rule thickness)
    const BIG1: f64 = 0.111112; // cmex10 fontdimen 9
    const BIG2: f64 = 0.166667; // fontdimen 10
    const BIG3: f64 = 0.2; // fontdimen 11
    const BIG4: f64 = 0.6; // fontdimen 12
    const BIG5: f64 = 0.1; // fontdimen 13

    /// oracle: `\hbox{$x^2$}` — sup sits at sup2 (uncramped text style),
    /// scriptspace widens the script box (TeXbook Appendix G, tex.web §745)
    #[test]
    fn superscript_placement_x2() {
        let b = text_math("x^2");
        let (list, w, h, _, _) = box_of(&b);
        approx(h, 8.14003, "x^2 height");
        approx(w, 10.2014, "x^2 width");
        assert_eq!(list.len(), 2, "nucleus + script box: {:?}", list);
        // script box raised by sup2, width includes \scriptspace
        approx(-box_shift(&list[1]), SUP2 * 10.0, "sup shift");
        let (_, sw, _, _, _) = box_of(&list[1]);
        // '2' in cmr7 is 0.291672em wide = 3.98613pt at 7pt? width = char + 0.5pt
        approx(sw, 3.98613 + 0.5, "sup box width");
    }

    /// oracle: `\hbox{$x_i^2$}` — stacked scripts in a vbox with the
    /// 4*rule-thickness clearance adjustment, shifted by shift_down
    #[test]
    fn both_scripts_stack_x_i_2() {
        let b = text_math("x_i^2");
        let (list, _, h, d, _) = box_of(&b);
        approx(h, 8.14003, "x_i^2 height");
        approx(d, 2.60292, "x_i^2 depth");
        let (_, _, vh, _, vshift) = box_of(&list[1]);
        assert!(matches!(list[1], Node::Box { kind: VBOX, .. }));
        approx(vshift, 2.60292, "vbox shift = shift_down");
        approx(vh, 10.74295, "vbox natural height");
        let (vlist, _, _, _, _) = box_of(&list[1]);
        assert_eq!(vlist.len(), 3, "sup, kern, sub: {:?}", vlist);
        approx(-box_shift(&vlist[0]), 0.0, "sup shift = delta(italic x)=0");
        if let Node::Kern(k) = &vlist[1] {
            approx(*k, 1.59991, "stack kern");
        } else {
            panic!("expected kern, got {:?}", vlist[1]);
        }
        // depth check: sub2 floor + clearance moved the sub down
        let su = SUP2 * 10.0;
        let mut sd = SUB2 * 10.0;
        let clr = 4.0 * RT * 10.0 - ((su - 0.0) - (4.63193 - sd));
        if clr > 0.0 {
            sd += clr;
        }
        approx(vshift, sd, "shift matches tex.web clearance rule");
    }

    /// subscript alone: floor at sub1 (0.15em of fam2 text)
    #[test]
    fn subscript_alone_floor() {
        let b = text_math("x_i");
        let (list, _, _, _, _) = box_of(&b);
        assert_eq!(list.len(), 2);
        approx(box_shift(&list[1]), SUB1 * 10.0, "sub shift = sub1");
    }

    /// oracle: `\hbox{$a\over b$}` — text-style fraction: num2/denom2, rule
    /// centered on the axis, num/den reboxed to a common width
    #[test]
    fn fraction_text_a_over_b() {
        let b = text_math("a\\over b");
        let (list, w, h, d, _) = box_of(&b);
        approx(w, 6.73764, "frac width (nulldelimspace pair)");
        approx(h, 6.9512, "frac height = num2 + h(a)");
        approx(d, 3.44841, "frac depth = denom2");
        // [null delim box, vbox, null delim box]
        assert_eq!(list.len(), 3, "{:?}", list);
        let (_, ndw, _, _, nds) = box_of(&list[0]);
        approx(ndw, 1.2, "nulldelimiterspace");
        approx(-nds, AXIS * 10.0, "null delim axis shift");
        let (vlist, vw, vh, vd, vs) = box_of(&list[1]);
        approx(vs, 0.0, "fraction vbox unshifted");
        approx(vh, NUM2 * 10.0 + 3.01389, "vbox h = shift_up + h(num)");
        approx(vd, DEN2 * 10.0, "vbox d = shift_down + d(den)");
        assert_eq!(vlist.len(), 5, "num, kern, rule, kern, den: {:?}", vlist);
        approx(vw, 4.33765, "common width");
        if let (Node::Kern(k1), Node::Rule { height, width, .. }, Node::Kern(k2)) =
            (&vlist[1], &vlist[2], &vlist[3])
        {
            approx(*k1, 1.23732, "num->rule kern");
            approx(*k2, 0.88731, "rule->den kern");
            approx(*height, RT * 10.0, "rule thickness");
            approx(*width, 4.33765, "rule width = box width");
        } else {
            panic!("fraction middle: {:?}", vlist);
        }
        let _ = h;
        let _ = d;
    }

    /// display-style fraction: num1/denom1 and axis-centered rule
    #[test]
    fn fraction_display_uses_num1_denom1() {
        let b = display_math("a\\over b");
        let (list, _, h, d, _) = box_of(&b);
        let (vlist, _, vh, vd, _) = box_of(&list[list.len() - 2]);
        let su = NUM1 * 10.0;
        let sd = DEN1 * 10.0;
        approx(vh, su + 4.30554, "vbox h = num1 + h(a)");
        approx(vd, sd, "vbox d = denom1");
        // rule top at axis + half(r): kern1 = (su - h(a)) - (axis + r/2)
        if let (Node::Kern(k1), Node::Kern(k2)) = (&vlist[1], &vlist[3]) {
            let r = RT * 10.0;
            let axis = AXIS * 10.0;
            approx(*k1, su - 0.0 - (axis + r / 2.0), "display kern1");
            approx(*k2, (axis - r / 2.0) - (6.94444 - sd), "display kern2");
        } else {
            panic!("expected kerns: {:?}", vlist);
        }
        let _ = (h, d);
    }

    /// text-style \atop: num3 (cramped numerator shift), no rule, no axis
    /// equalization; oracle box for `$p\atop q$`
    #[test]
    fn atop_text_uses_num3() {
        let b = text_math("p\\atop q");
        let (list, _, _, _, _) = box_of(&b);
        let (vlist, _, vh, vd, _) = box_of(&list[1]);
        approx(vh, NUM3 * 10.0 + 3.01389, "vbox h = num3 + h(p)");
        approx(vd, 1.3611 + DEN2 * 10.0, "vbox d = d(p) + denom2");
        assert_eq!(vlist.len(), 3, "no rule for atop: {:?}", vlist);
        if let Node::Kern(k) = &vlist[1] {
            approx(*k, 3.51073, "atop stack kern");
        } else {
            panic!("expected kern: {:?}", vlist);
        }
    }

    /// oracle: `\hbox{$\sqrt{x}$}` — cmex surd shifted to -(h+clr), bar box
    /// = [kern(surd_h), rule(surd_h), kern(clr), body]
    #[test]
    fn radical_sqrt_x() {
        let b = text_math("\\sqrtG{x}");
        let (list, w, h, d, _) = box_of(&b);
        approx(w, 14.04863, "sqrt width = surd + body");
        approx(h, 8.00272, "sqrt height");
        approx(d, 2.39725, "sqrt depth");
        assert_eq!(list.len(), 2, "surd + bar vbox: {:?}", list);
        let (_, sw, _, sd, ssh) = box_of(&list[0]);
        approx(sw, 8.33336, "surd width");
        approx(ssh, -7.20276, "surd shift -(h(x)+clr)");
        // depth of surd glyph: 9.6pt; shift moved ink up
        approx(sd, 9.6, "surd glyph depth");
        let (vlist, _, vh, _, _) = box_of(&list[1]);
        approx(vh, 8.00272, "bar vbox height = t + t + clr + h(x)");
        assert_eq!(vlist.len(), 4, "kern, rule, kern, body: {:?}", vlist);
        if let (Node::Kern(t), Node::Rule { height, .. }, Node::Kern(clr)) =
            (&vlist[0], &vlist[1], &vlist[2])
        {
            approx(*t, 0.39998, "top kern = surd height");
            approx(*height, 0.39998, "rule = surd height");
            approx(*clr, 2.89722, "clearance with half-excess");
        } else {
            panic!("bar vbox: {:?}", vlist);
        }
    }

    #[test]
    fn math_units_scale_with_style_and_nonscript() {
        let (_, mw, _, _, _) = box_of(&text_math("a\\mkern5mu b"));
        let (_, sw, _, _, _) = box_of(&text_math("a\\mskip5mu b"));
        approx(mw, 12.35526, "text-style mkern uses 10pt/18 mu");
        approx(sw, 12.35526, "text-style mskip uses 10pt/18 mu");

        let (_, script_w, _, _, _) = box_of(&text_math("a_{a\\mkern5mu b}"));
        approx(script_w, 15.91643, "script-style mkern uses 7pt/18 mu");

        let (_, nonscript_w, _, _, _) = box_of(&text_math("a_{a\\nonscript\\mskip5mu b}"));
        approx(nonscript_w, 13.6402, "nonscript suppresses following mskip");
    }

    #[test]
    fn overline_and_underline_keep_nucleus_baseline() {
        let over = display_math("\\overline{x_i}");
        let (_, ow, oh, od, _) = box_of(&over);
        approx(ow, 9.04456, "overline width");
        approx(oh, 6.30544, "overline includes top rule clearance");
        approx(od, 1.49998, "overline keeps nucleus depth");

        let under = display_math("\\underline{x_i}");
        let (_, uw, uh, ud, _) = box_of(&under);
        approx(uw, 9.04456, "underline width");
        approx(uh, 4.30554, "underline keeps nucleus height");
        approx(ud, 3.49988, "underline rule stack is below baseline");

        let under_sup = display_math("\\underline{x^i}");
        let (_, _, ush, usd, _) = box_of(&under_sup);
        approx(ush, 8.76085, "underline uses uncramped nucleus style");
        approx(usd, 1.9999, "underline superscript rule stack depth");
    }

    /// oracle: `\hbox{$\sum_{i=1}^n$}` — side scripts in text style (no
    /// limits), op axis-centered, scripts in a shifted vbox
    #[test]
    fn sum_text_style_side_scripts() {
        let b = text_math("\\sumG_{i=1}^n");
        let (list, _, h, d, _) = box_of(&b);
        approx(h, 8.04175, "sum total height");
        approx(d, 3.00005, "sum total depth");
        assert_eq!(list.len(), 2, "op box + scripts vbox: {:?}", list);
        approx(-box_shift(&list[0]), 7.50006, "op axis shift");
        let (vlist, _, _, _, vshift) = box_of(&list[1]);
        approx(vshift, 3.00005, "scripts vbox shift");
        if let Node::Kern(k) = &vlist[1] {
            approx(*k, 3.39598, "scripts stack kern");
        } else {
            panic!("expected kern: {:?}", vlist);
        }
    }

    #[test]
    fn nolimits_survives_attaching_the_second_script() {
        let lower_first = text_math("\\displaystyle\\sumG\\nolimits_a^b");
        let upper_first = text_math("\\displaystyle\\sumG\\nolimits^b_a");
        let (_, w1, h1, d1, _) = box_of(&lower_first);
        let (_, w2, h2, d2, _) = box_of(&upper_first);
        assert_eq!((w1, h1, d1), (w2, h2, d2));
        let limits = text_math("\\displaystyle\\sumG\\limits_a^b");
        let (_, _, height, depth, _) = box_of(&limits);
        assert!(height + depth > h1 + d1);
    }

    /// \int with both scripts: the superscript is offset right by the italic
    /// correction of the op (tex.web make_scripts shift_amount(x) := delta)
    #[test]
    fn int_sup_italic_offset() {
        let b = text_math("\\intG_a^b");
        let (list, _, _, _, _) = box_of(&b);
        let (vlist, _, _, _, _) = box_of(&list[1]);
        // sup 'b' box shifted right by half? no — by the full italic (0.19444em)
        approx(box_shift(&vlist[0]), 1.94444, "sup shifted by italic");
    }

    /// A second script must not erase the \nolimits subtype stored on the
    /// operator noad. Oracle: `\hbox{$\displaystyle\int\nolimits_0^1$}`.
    #[test]
    fn int_display_nolimits_keeps_side_scripts() {
        let b = display_math("\\intG\\nolimits_0^1");
        let (list, w, h, d, _) = box_of(&b);
        assert_eq!(list.len(), 2, "operator plus side-script box: {:?}", list);
        approx(w, 14.48615, "integral with side scripts width");
        approx(h, 15.65013, "integral with side scripts height");
        approx(d, 9.11122, "integral with side scripts depth");
    }

    /// A plain delimiter atom is Ord, not an operator; its subscript stays
    /// beside it instead of becoming a displayed lower limit.
    #[test]
    fn delimiter_subscript_is_not_operator_limits() {
        let b = display_math("\\delimiter\"026B30D _H");
        let (list, w, h, d, _) = box_of(&b);
        assert_eq!(list.len(), 2, "delimiter plus side-script box: {:?}", list);
        approx(w, 12.58475, "delimiter subscript width");
        approx(h, 7.5, "delimiter subscript height");
        approx(d, 2.5, "delimiter subscript depth");
    }

    /// display-style \sum gets limits above/below (big op spacings from
    /// cmex fontdimens 9..13), sup/sub skewed by half the italic (delta=0
    /// for \sum), all centered on the common width
    #[test]
    fn sum_display_limits_box() {
        let b = display_math("\\sumG_{i=1}^{n}");
        let (list, _, _, _, _) = box_of(&b);
        let (vlist, vw, vh, vd, _) = box_of(&list[0]);
        assert!(
            matches!(list[0], Node::Box { kind: VBOX, .. }),
            "{:?}",
            list
        );
        let xh5 = 0.430555 * 5.0; // x-height at scriptscript size (5pt)
        approx(vh, 16.51395, "limits vbox h");
        approx(vd, 12.79869, "limits vbox d");
        let _ = vw;
        // vbox: [kern sp5, sup, kern su, op, kern sd, sub, kern sp5]
        assert_eq!(vlist.len(), 7, "{:?}", vlist);
        let _ = xh5;
    }

    /// Operator limits are selected when the surrounding style is converted,
    /// not when the math list is parsed. A denominator is text style even
    /// when its enclosing fraction was parsed in display style.
    #[test]
    fn sum_in_display_denominator_uses_side_scripts() {
        let b = display_math("{1\\over\\sumG_{i=1}^{n}}");
        let (_, w, h, d, _) = box_of(&b);
        approx(w, 26.40991, "fraction width");
        approx(h, 13.20952, "fraction height");
        approx(d, 9.94173, "fraction depth");
    }

    /// A directive after BOTH scripts still switches the noad: tex.web
    /// `math_limit_switch` writes `subtype(tail)` at scan time, so a sum
    /// parsed in text style gains displayed limits from a trailing
    /// `\limits`. oracle: `\hbox{$\textstyle\sumG_0^1\limits$}`
    #[test]
    fn trailing_limits_forces_above_below_in_text_style() {
        let b = text_math("\\sumG_0^1\\limits");
        let (list, w, h, d, _) = box_of(&b);
        assert!(
            matches!(list[0], Node::Box { kind: VBOX, .. }),
            "limits vbox: {:?}",
            list
        );
        approx(w, 10.55559, "forced limits width");
        approx(h, 15.01115, "forced limits height");
        approx(d, 9.67783, "forced limits depth");
    }

    /// `\nolimits` pins the noad for the FIRST script, but the trailing
    /// `\limits` rewrites the same noad's subtype — the pin is gone.
    /// oracle: `\hbox{$\displaystyle\intG\nolimits_x\limits$}`
    #[test]
    fn trailing_limits_after_first_script_rewrites_the_noad() {
        let b = display_math("\\intG\\nolimits_x\\limits");
        let (list, w, h, d, _) = box_of(&b);
        assert!(
            matches!(list[0], Node::Box { kind: VBOX, .. }),
            "limits vbox: {:?}",
            list
        );
        approx(w, 10.00002, "integral limits width");
        approx(h, 13.61122, "integral limits height");
        approx(d, 15.61124, "integral limits depth");
    }

    /// Same shape with `\displaylimits`: releases the nolimits pin back to
    /// `normal`, which display style promotes to limits.
    /// oracle: `\hbox{$\displaystyle\intG\nolimits_x\displaylimits$}`
    #[test]
    fn trailing_displaylimits_after_first_script_promotes() {
        let b = display_math("\\intG\\nolimits_x\\displaylimits");
        let (list, w, h, d, _) = box_of(&b);
        assert!(
            matches!(list[0], Node::Box { kind: VBOX, .. }),
            "limits vbox: {:?}",
            list
        );
        approx(w, 10.00002, "integral limits width");
        approx(h, 13.61122, "integral limits height");
        approx(d, 15.61124, "integral limits depth");
    }

    /// A `\nolimits` BETWEEN the two scripts pins the noad for the second
    /// script too (the subtype persists on the operator noad).
    /// oracle: `\hbox{$\displaystyle\sumG_a\nolimits^b$}`
    #[test]
    fn mid_directive_nolimits_survives_second_script() {
        let b = display_math("\\sumG_a\\nolimits^b");
        let (list, w, h, d, _) = box_of(&b);
        assert_eq!(list.len(), 2, "op box plus side-script vbox: {:?}", list);
        approx(w, 19.28212, "sum side-scripts width");
        approx(h, 12.88896, "sum side-scripts height");
        approx(d, 6.00005, "sum side-scripts depth");
    }

    /// Explicit `\displaylimits` records subtype=normal (marker 0), which
    /// the actual conversion style promotes in display.
    /// oracle: `\hbox{$\displaystyle\sumG\displaylimits_a$}`
    #[test]
    fn explicit_displaylimits_promotes_in_display_style() {
        let b = display_math("\\sumG\\displaylimits_a");
        let (list, w, h, d, _) = box_of(&b);
        assert!(
            matches!(list[0], Node::Box { kind: VBOX, .. }),
            "limits vbox: {:?}",
            list
        );
        approx(w, 14.44447, "displaylimits width");
        approx(h, 10.50006, "displaylimits height");
        approx(d, 12.50006, "displaylimits depth");
    }

    /// `\mathop{...}\nolimits` keeps the GROUP nucleus (boxed mlist with
    /// its italic kern), not a collapsed char nucleus.
    /// oracle: `\hbox{$\displaystyle\mathop{xy}\nolimits_a$}`
    #[test]
    fn grouped_mathop_nolimits_keeps_group_nucleus() {
        let b = display_math("\\mathop{xy}\\nolimits_a");
        let (list, w, h, d, _) = box_of(&b);
        assert_eq!(list.len(), 2, "group box plus side-script box: {:?}", list);
        approx(w, 15.81451, "mathop nolimits width");
        approx(h, 4.30554, "mathop nolimits height");
        approx(d, 2.44443, "mathop nolimits depth");
    }

    /// Forced `\limits` on a grouped mathop builds the above/below box
    /// even in text style, centered at the group width.
    /// oracle: `\hbox{$\textstyle\mathop{xy}\limits_a$}`
    #[test]
    fn grouped_mathop_limits_forces_above_below_in_text_style() {
        let b = text_math("\\mathop{xy}\\limits_a");
        let (list, w, h, d, _) = box_of(&b);
        assert!(
            matches!(list[0], Node::Box { kind: VBOX, .. }),
            "limits vbox: {:?}",
            list
        );
        approx(w, 10.97687, "forced mathop limits width");
        approx(h, 4.30554, "forced mathop limits height");
        approx(d, 8.94444, "forced mathop limits depth");
    }

    /// Delimiter lookup indexes `style_fonts[sz]` directly (tex.web
    /// `fam_fnt(fam+cur_size)`): a script-size delimiter takes the SCRIPT
    /// font, not the text font. Total width includes scriptspace.
    /// oracle: `\hbox{$x^{\delimiter"0028300}$}`
    #[test]
    fn script_delimiter_uses_script_font() {
        let b = text_math("x^{\\delimiter\"0028300}");
        let (_, w, h, d, _) = box_of(&b);
        approx(w, 9.34029, "script paren total width");
        approx(h, 8.87892, "script paren total height");
        approx(d, 0.0, "script paren total depth");
    }

    /// Same at scriptscript size, including the scriptspace in total width.
    /// oracle: `\hbox{$x^{\scriptscriptstyle\delimiter"0028300}$}`
    #[test]
    fn scriptscript_delimiter_uses_scriptscript_font() {
        let b = text_math("x^{\\scriptscriptstyle\\delimiter\"0028300}");
        let (_, w, h, d, _) = box_of(&b);
        approx(w, 8.92363, "scriptscript paren total width");
        approx(h, 7.37892, "scriptscript paren total height");
        approx(d, 0.0, "scriptscript paren total depth");
    }

    /// oracle: `\hbox{$\left({a\over b}\right)$}` — delimiters from cmex
    /// sized by delimiterfactor/shortfall, ink centered on the axis
    #[test]
    fn left_right_group_paren_big() {
        let b = text_math("\\left({a\\over b}\\right)");
        let (outer, _, h, d, _) = box_of(&b);
        approx(h, 8.50005, "group height");
        approx(d, 3.50006, "group depth");
        let list = match outer.first() {
            Some(Node::Box { list: inner, .. }) => inner,
            _ => outer,
        };
        assert_eq!(list.len(), 3, "open, inner, close: {:?}", list);
        let (_, pw, ph, pd, psh) = box_of(&list[0]);
        approx(ph, 0.39998, "parenleftbig height");
        approx(pd, 11.60013, "parenleftbig depth");
        approx(-psh, 8.10007, "paren axis shift");
        let _ = pw;
    }

    /// oracle: `\hbox{$\bar{x}$}` — accent vbox: zero-width accent shifted
    /// by skew + half(body-accent), kern -h(body), body; width = body width
    #[test]
    fn accent_bar_x() {
        let b = text_math("\\barG{x}");
        let (list, w, h, _, _) = box_of(&b);
        assert_eq!(list.len(), 1, "single accent vbox: {:?}", list);
        approx(h, 5.67776, "accent box height");
        approx(w, 5.71527, "width = body width");
        let (vlist, _, _, _, _) = box_of(&list[0]);
        assert_eq!(vlist.len(), 3, "accent, kern, body: {:?}", vlist);
        approx(box_shift(&vlist[0]), 0.35764, "accent skew shift");
        if let Node::Kern(k) = &vlist[1] {
            approx(*k, -4.30554, "kern = -h(x)");
        } else {
            panic!("expected kern: {:?}", vlist);
        }
        let (_, aw, _, _, _) = box_of(&vlist[0]);
        assert_eq!(aw, 0, "accent width forced to 0");
    }

    /// tex.web make_math_accent @<Swap the subscript and superscript into
    /// box x@>: scripts on an accented single-char nucleus attach to the
    /// character, not the accent glyph. Oracle: real pdftex
    /// `\hbox{$\hat\beta_p^{\rm PR}$}` (\hat = \mathaccent"705E, \beta =
    /// \mathchar"010C) — 9.58334+3.83327x17.86461; without the swap the
    /// superscript clears the taller accent box instead of the nucleus and
    /// the total inflates to 11.89449+3.83327x17.86461.
    #[test]
    fn accent_scripts_swap() {
        let b = text_math("\\mathaccent\"705E\\mathchar\"010C_p^{\\fam0 PR}");
        let (_, w, h, d, _) = box_of(&b);
        approx(h, 9.58334, "accent+scripts box height");
        approx(d, 3.83327, "accent+scripts box depth");
        approx(w, 17.86461, "accent+scripts box width");
    }

    #[test]
    fn wide_accent_selects_fitting_variant() {
        let b = text_math("\\mathaccent\"0365{xyz}");
        let (_, w, h, d, _) = box_of(&b);
        approx(h, 7.5, "wide tilde height");
        approx(d, 1.94444, "wide tilde depth");
        approx(w, 16.06717, "wide tilde width");
    }

    #[test]
    fn roman_math_run_kerns_and_keeps_final_italic_correction() {
        let b = text_math("{\\fam0 cov}");
        let (_, w, _, _, _) = box_of(&b);
        approx(w, 14.58334, "roman math run width");
    }

    #[test]
    fn roman_math_run_forms_font_ligatures() {
        let b = text_math("{\\fam0 ffi}");
        let (_, w, _, _, _) = box_of(&b);
        approx(w, 8.33336, "roman ffi ligature width");
    }

    /// tex.web §1198 removes braces around a lone scriptless Ord noad.
    /// The resulting character keeps its italic correction when bare and
    /// uses that correction to offset a superscript when a subscript exists.
    #[test]
    fn singleton_ord_math_group_keeps_character_script_semantics() {
        let bare = text_math("{\\fam2 T}");
        let (_, w, h, d, _) = box_of(&bare);
        approx(w, 7.98811, "bare grouped calligraphic T width");
        approx(h, 6.83331, "bare grouped calligraphic T height");
        approx(d, 0.0, "bare grouped calligraphic T depth");

        let scripted = text_math("{\\fam2 T}^2_C");
        let (_, w, h, d, _) = box_of(&scripted);
        approx(w, 12.47424, "grouped T with both scripts width");
        approx(h, 8.14003, "grouped T with both scripts height");
        approx(d, 2.75433, "grouped T with both scripts depth");
    }

    /// A math accent stores a family and character, not the font selected
    /// while its field is scanned. Conversion inside `_...` must therefore
    /// use the cramped script font. Oracle: pdfTeX with the test preamble.
    #[test]
    fn accent_font_is_selected_at_conversion_style() {
        let b = text_math("{\\fam2 T}_{\\barG C}");
        let (_, w, h, d, _) = box_of(&b);
        approx(w, 12.17242, "grouped T accented subscript width");
        approx(h, 6.83331, "grouped T accented subscript height");
        approx(d, 2.34445, "grouped T accented subscript depth");
    }

    /// oracle: `\hbox{$x+a$}` — medmuskip glue (4mu plus 2mu minus 4mu) on
    /// both sides of the bin
    #[test]
    fn bin_spacing_medmuskip() {
        let b = text_math("x+a");
        let (list, _, _, _, _) = box_of(&b);
        assert_eq!(list.len(), 5, "x, glue, +, glue, a: {:?}", list);
        for i in [1usize, 3] {
            if let Node::Glue(g) = &list[i] {
                approx(g.width, 4.0 * (10.0 / 18.0), "medmuskip width");
                approx(g.stretch, 2.0 * (10.0 / 18.0), "medmuskip stretch");
                approx(g.shrink, 4.0 * (10.0 / 18.0), "medmuskip shrink");
            } else {
                panic!("expected glue at {}: {:?}", i, list[i]);
            }
        }
    }

    /// script styles drop conditional mu spacing (tex.web digits 1/3/4):
    /// `x+a` inside a superscript gets NO medmuskip
    #[test]
    fn script_style_drops_bin_spacing() {
        let b = text_math("x^{y+z}");
        let (list, _, _, _, _) = box_of(&b);
        let (sup, _, _, _, _) = box_of(&list[1]);
        // sup content: y + kern(?? no: [char y, glue? none, char z])
        let glues = sup.iter().filter(|n| matches!(n, Node::Glue(_))).count();
        assert_eq!(glues, 0, "no mu glue in script style: {:?}", sup);
        let chars = sup
            .iter()
            .filter(|n| matches!(n, Node::Char { .. }))
            .count();
        assert_eq!(chars, 3, "y, +, z present: {:?}", sup);
    }

    /// bin demotion: a bin at the start of a formula is classed ord (no
    /// medmuskip around it)
    #[test]
    fn leading_bin_demoted_to_ord() {
        let b = text_math("+x");
        let (list, _, _, _, _) = box_of(&b);
        assert_eq!(list.len(), 2, "no spacing around demoted bin: {:?}", list);
    }

    /// \overwithdelims: delimiters sized to delim1 (display) / delim2
    #[test]
    fn overwithdelims_sizes_delimiters() {
        let b = text_math("a\\overwithdelims()b");
        let (list, _, _, _, _) = box_of(&b);
        assert!(list.len() >= 3, "delim + frac + delim: {:?}", list);
        // both delimiters are cmex parens (big variants), axis-centered
        let (_, _, ph1, pd1, sh1) = box_of(&list[0]);
        assert!(ph1 + pd1 >= su(DELIM2_MIN), "delimiter covers delim2");
        approx(-sh1, AXIS * 10.0 + 5.6, "axis centered");
    }

    const DELIM2_MIN: f64 = 0.49; // cmsy10 fontdimen 21 (delim2) in em

    /// radical inside fraction inside \left..\right — the full nesting path
    /// (theory.tex eq:bdag shape: b^\dagger = \lambda/(\lambda+\theta))
    #[test]
    fn nested_formula_layout() {
        let b = text_math("b^\\dagger =\\fracG{\\lambda}{\\lambda+\\theta}");
        let (list, w, _, _, _) = box_of(&b);
        // b, sup, glue(thick), =, glue(thick), fraction with parens? no \left here:
        // b + supbox + thick + rel(=) + thick + [frac: \lambda over \lambda+\theta]
        assert!(list.len() >= 5, "{:?}", list);
        assert!(w > 0);
        // fraction is the last atom: enclosed in inner class box with nulldelimiters
        let last = &list[list.len() - 1];
        fn find_frac_vbox(n: &Node) -> Option<&Node> {
            match n {
                Node::Box {
                    kind: VBOX, list, ..
                } if list.len() == 5 => Some(n),
                Node::Box { list, .. } => list.iter().find_map(find_frac_vbox),
                _ => None,
            }
        }
        let frac_box = find_frac_vbox(last).expect("expected fraction vbox");
        if let Node::Box {
            kind: VBOX,
            list: vl,
            ..
        } = frac_box
        {
            assert_eq!(vl.len(), 5, "num, kern, rule, kern, den: {:?}", vl);
        }
    }

    /// \mathchoice selects the branch for the current style
    #[test]
    fn mathchoice_branch_selection() {
        // text style picks the second branch (T)
        let b = text_math("\\mathchoice{D}{T}{S}{SS}");
        let (list, w, _, _, _) = box_of(&b);
        // cmmi10 'T': wd 0.584376 + ic 0.13889 = 7.232666pt (tex.web §14865:
        // char nucleus without subscript takes its italic correction)
        approx(w, 7.232666, "text branch chosen (T)");
        let _ = list;
        // display picks the first branch (D); cmmi10 'D': 0.827917+0.027779
        let bd = display_math("\\mathchoice{D}{T}{S}{SS}");
        let (_, wd, _, _, _) = box_of(&bd);
        approx(wd, 8.55696, "display branch chosen (D)");
    }

    /// \mathaccent exists and the accent char is present: sanity on setup
    #[test]
    fn script_space_and_params_present() {
        let e = math_engine();
        assert!(e.eqtb.dim_params[DimParam::ScriptSpace.idx() as usize] > 0);
        assert!(e.eqtb.int_params[IntParam::DelimiterFactor.idx() as usize] > 0);
        // fam2 axis height present at all three sizes
        assert!(e.eqtb.style_fonts[0][2] != 0);
        assert!(e.eqtb.style_fonts[1][2] != 0);
        assert!(e.eqtb.style_fonts[2][2] != 0);
    }

    #[test]
    fn math_class_primitives_spacing() {
        // \mathopen{}x\mathbin{+}y\mathrel{=}z\mathclose{}
        let b = text_math("\\mathopen{}x\\mathbin{+}y\\mathrel{=}z\\mathclose{}");
        let (list, _, _, _, _) = box_of(&b);
        // verify that glue nodes (medmuskip, thickmuskip) were inserted
        let glues: Vec<_> = list
            .iter()
            .filter_map(|n| match n {
                Node::Glue(g) => Some(g.width),
                _ => None,
            })
            .collect();
        assert!(!glues.is_empty(), "inter-atom glue present");
    }

    #[test]
    fn math_style_primitives() {
        let b = text_math("{\\displaystyle x}+\\textstyle y+\\scriptstyle z+\\scriptscriptstyle w");
        let (list, _, _, _, _) = box_of(&b);
        assert!(!list.is_empty());
    }

    #[test]
    fn math_all_twelve_primitives() {
        let b = text_math("\\mathord{a}\\mathop{b}\\mathpunct{,}\\mathinner{c}");
        let (list, _, _, _, _) = box_of(&b);
        assert!(!list.is_empty());
    }

    // ---- pre_display_size_of / display skip selection (tex.web §1181) ----

    /// Hand-build the final line (just_box) of an interrupted paragraph and
    /// measure \predisplaysize directly. cmr10 must be the current font so
    /// the `+2 quad` term is well-defined.
    fn pds_engine() -> (Engine, FontId) {
        let mut e = run_doc("");
        let cmr = e
            .eqtb
            .fonts
            .iter()
            .position(|f| f.tfm_name == "cmr10")
            .expect("cmr10 loaded") as FontId;
        e.eqtb.cur_font_val = cmr;
        (e, cmr)
    }

    fn line_box(list: Vec<Node>, sign: u8, order: u8, set: f64) -> Node {
        Node::Box {
            kind: HBOX,
            w: su(240.0),
            h: su(6.94444),
            d: 0,
            shift: 0,
            list,
            glue_sign: sign,
            glue_order: order,
            glue_set: set,
            font: None,
        }
    }

    #[test]
    fn pds_counts_boxes_rules_kerns() {
        // §1184: hlist/rule are *visible* (w resets at each); kerns and
        // inactive glue accumulate without being visible.
        let (e, cmr) = pds_engine();
        let quad = e.eqtb.fonts[cmr as usize].quad() as i64;
        let wa = e.eqtb.fonts[cmr as usize].char_width(b'A') as i64;
        let line = line_box(
            vec![
                Node::Box {
                    kind: HBOX,
                    w: su(10.0),
                    h: 0,
                    d: 0,
                    shift: 0,
                    list: vec![],
                    glue_sign: 0,
                    glue_order: 0,
                    glue_set: 0.0,
                    font: None,
                },
                Node::Glue(Glue {
                    width: su(3.33333),
                    stretch: su(1.66666),
                    shrink: su(1.11111),
                    stretch_order: 0,
                    shrink_order: 0,
                }),
                Node::Rule {
                    width: su(1.0),
                    height: su(0.4),
                    depth: 0,
                },
                Node::Kern(su(0.5)),
                Node::Char { c: b'A', font: cmr },
            ],
            0,
            0,
            0.0,
        );
        // last visible node is the 'A': 10 + 3.33333 + 1 + 0.5 + w(A) + 2quad
        let want =
            su(10.0) as i64 + su(3.33333) as i64 + su(1.0) as i64 + su(0.5) as i64 + wa + 2 * quad;
        assert_eq!(e.pre_display_size_of(&line), want);
    }

    #[test]
    fn pds_voided_by_normal_order_active_glue() {
        // §21802: glue_order(just_box)=stretch_order(q) with stretch<>0 sets
        // v:=max_dimen *even at order 0* (normal); the next visible node then
        // yields w = max_dimen. The old code required order > 0 and instead
        // scaled the glue by glue_set — system-dependent rounding TeX's
        // §1186 comment explicitly forbids.
        let (e, cmr) = pds_engine();
        let mk = |sign: u8, g: Glue| {
            line_box(
                vec![
                    Node::Char { c: b'A', font: cmr },
                    Node::Glue(g),
                    Node::Char { c: b'B', font: cmr },
                ],
                sign,
                0,
                0.5,
            )
        };
        let stretched = mk(
            1,
            Glue {
                width: su(3.33333),
                stretch: su(1.66666),
                shrink: 0,
                stretch_order: 0,
                shrink_order: 0,
            },
        );
        assert_eq!(e.pre_display_size_of(&stretched), 0x3FFF_FFFF);
        let shrunk = mk(
            2,
            Glue {
                width: su(3.33333),
                stretch: 0,
                shrink: su(1.11111),
                stretch_order: 0,
                shrink_order: 0,
            },
        );
        assert_eq!(e.pre_display_size_of(&shrunk), 0x3FFF_FFFF);
        // a *fil* line (order 2) leaves normal-order glue untouched: natural
        // widths accumulate and the trailing 'B' is visible
        let quad = e.eqtb.fonts[cmr as usize].quad() as i64;
        let wa = e.eqtb.fonts[cmr as usize].char_width(b'A') as i64;
        let wb = e.eqtb.fonts[cmr as usize].char_width(b'B') as i64;
        let fil_line = line_box(
            vec![
                Node::Char { c: b'A', font: cmr },
                Node::Glue(Glue {
                    width: su(3.33333),
                    stretch: su(1.66666),
                    shrink: 0,
                    stretch_order: 0,
                    shrink_order: 0,
                }),
                Node::Char { c: b'B', font: cmr },
            ],
            1,
            2,
            0.5,
        );
        assert_eq!(
            e.pre_display_size_of(&fil_line),
            wa + su(3.33333) as i64 + wb + 2 * quad
        );
    }

    #[test]
    fn pds_leaders_are_visible() {
        // §1192: leaders take the active-glue test, then goto found with the
        // glue spec's *natural* width (never inflated by glue_set).
        let (e, cmr) = pds_engine();
        let quad = e.eqtb.fonts[cmr as usize].quad() as i64;
        let line = line_box(
            vec![
                Node::Rule {
                    width: su(1.0),
                    height: 0,
                    depth: 0,
                },
                Node::Leaders {
                    glue: Glue {
                        width: su(2.0),
                        stretch: su(1.0),
                        shrink: 0,
                        stretch_order: 2,
                        shrink_order: 0,
                    },
                    kind: 0,
                    body: crate::boxes::LeaderBody::Rule {
                        width: su(2.0),
                        height: 0,
                        depth: 0,
                    },
                },
            ],
            1,
            2,
            0.5,
        );
        // line stretches at order 2 (fil) and the leader's stretch_order is
        // 2 → active: v := max_dimen, then `goto found` → w = max_dimen
        assert_eq!(e.pre_display_size_of(&line), 0x3FFF_FFFF);
        // with the line inactive (sign 0) the leader is visible at natural width
        let calm = line_box(
            vec![
                Node::Rule {
                    width: su(1.0),
                    height: 0,
                    depth: 0,
                },
                Node::Leaders {
                    glue: Glue {
                        width: su(2.0),
                        stretch: su(1.0),
                        shrink: 0,
                        stretch_order: 2,
                        shrink_order: 0,
                    },
                    kind: 0,
                    body: crate::boxes::LeaderBody::Rule {
                        width: su(2.0),
                        height: 0,
                        depth: 0,
                    },
                },
            ],
            0,
            0,
            0.0,
        );
        assert_eq!(
            e.pre_display_size_of(&calm),
            su(1.0) as i64 + su(2.0) as i64 + 2 * quad
        );
    }

    /// Recursively search every node list the engine may still hold after
    /// a run (page list, parked paragraph lists, current list) for a glue
    /// node of exactly width `w` sp.
    fn has_glue_width(e: &Engine, w: i32) -> bool {
        fn walk(ns: &[Node], w: i32) -> bool {
            ns.iter().any(|n| match n {
                Node::Glue(g) => g.width == w,
                Node::Box { list, .. } => walk(list, w),
                _ => false,
            })
        }
        walk(&e.page_list, w) || e.par_page_lists.iter().any(|p| walk(p, w)) || walk(&e.cur_list, w)
    }

    /// Oracle t13/t20 (pdftex -ini \input plain): `\parindent=20pt $$a=1$$`
    /// measures \predisplaysize = 20pt(box) + 2·quad(10pt) = 40pt; the old
    /// code treated the parindent hlist as invisible and returned the
    /// -max_dimen sentinel. d+s = 108.19pt > 40pt → short skips.
    #[test]
    fn pds_vmode_entry_measures_parindent_box() {
        let e = run_doc(
            "\\hsize=240pt \\vsize=60in \\parindent=20pt \
             \\abovedisplayskip=30pt \\abovedisplayshortskip=1pt \
             \\belowdisplayskip=30pt \\belowdisplayshortskip=2pt \
             \\tenrm $$a=1$$\n",
        );
        // quad = 10.00003pt (tfm design rounding) → pds = 40.00006pt, not exact
        approx(e.pre_display_size as i32, 40.0, "pds = parindent + 2 quad");
        assert!(
            has_glue_width(&e, su(1.0)),
            "\\abovedisplayshortskip glue missing (pds={})",
            e.pre_display_size
        );
        assert!(
            !has_glue_width(&e, su(30.0)),
            "normal display skip chosen despite d+s > pds"
        );
    }

    /// Oracle t24: `\noindent\hbox to 235pt{}$$a=1$$` in a 240pt hsize. The
    /// final line's last visible node is the 235pt box, so
    /// \predisplaysize = 235pt + 2·quad = 255pt ≥ d+s = 108.19pt → TeX picks
    /// the NORMAL skips. The old code dropped the box entirely (w stayed at
    /// the sentinel) and wrongly picked the short pair.
    #[test]
    fn pds_box_final_line_selects_normal_skips() {
        let e = run_doc(
            "\\hsize=240pt \\vsize=60in \\parindent=0pt \
             \\abovedisplayskip=30pt \\abovedisplayshortskip=1pt \
             \\belowdisplayskip=30pt \\belowdisplayshortskip=2pt \
             \\noindent\\hbox to 235pt{}\\tenrm$$a=1$$\n",
        );
        approx(e.pre_display_size as i32, 255.0, "pds = 235pt box + 2 quad");
        assert!(has_glue_width(&e, su(30.0)), "normal display skip missing");
        assert!(
            !has_glue_width(&e, su(1.0)),
            "short skip chosen although pds = 255pt (old invisible-box bug)"
        );
    }
}
