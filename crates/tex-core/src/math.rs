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

use crate::boxes::{
    hlist_dims, hpack, noad_option, vpack, AccentSpec, Delim, FenceOpts, Glue, MathDiagnosticOrigin,
    MathStyle,
    Node, NodeList, HBOX, VBOX,
};
use crate::engine::{Engine, Mode};
use crate::eqtb::Equiv;
use crate::prim::{DimParam, GlueParam, IntParam, Prim};
use crate::scaled::ONE;
use crate::tfm::{Font, FontId, TAG_EXT, TAG_LIG, TAG_LIST};
use crate::show_state::{PendingFrac, ScanKind};
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
        MathStyle::CrampedDisplay => 1,
        MathStyle::CrampedText => 3,
        MathStyle::CrampedScript => 5,
        MathStyle::CrampedScriptScript => 7,
    }
}

/// Inverse of [`gstyle_of`].
pub(crate) fn math_style_of(g: GStyle) -> MathStyle {
    match g {
        0 => MathStyle::Display,
        1 => MathStyle::CrampedDisplay,
        2 => MathStyle::Text,
        3 => MathStyle::CrampedText,
        4 => MathStyle::Script,
        5 => MathStyle::CrampedScript,
        6 => MathStyle::ScriptScript,
        _ => MathStyle::CrampedScriptScript,
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
pub(crate) fn font_size(g: GStyle) -> usize {
    match g {
        0 | 1 | 2 | 3 => 0,
        4 | 5 => 1,
        _ => 2,
    }
}

/// denominator style: next level up (tex.web §738:
/// `num_style = #+2-2*(# div 6)`, `denom_style = 2*(# div 2)+cramped+2-2*(# div 6)`)
#[inline]
pub(crate) fn num_style(g: GStyle) -> GStyle {
    if g < 6 {
        g + 2
    } else {
        g
    }
}

#[inline]
pub(crate) fn den_style(g: GStyle) -> GStyle {
    2 * (g / 2) + 3 - 2 * (g / 6)
}

/// superscript style
#[inline]
pub(crate) fn sup_style(g: GStyle) -> GStyle {
    match g >> 1 {
        0 | 1 => 4 + (g & 1),
        _ => 6 + (g & 1),
    }
}

/// subscript style: always cramped at the next level
#[inline]
pub(crate) fn sub_style(g: GStyle) -> GStyle {
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
pub(crate) fn finish_math_group(mut inner: NodeList, flatten: i32, attr: crate::boxes::Attr) -> Node {
    // luatex `close_math_group`: one scriptless simple noad is flattened
    // into its field when `\mathflattenmode` has the bit of its class
    // (ord 1, bin 2, rel 4, punct 8, inner 16); other engines use 1
    let bit = |class: u8| -> bool {
        match class {
            CL_ORD => flatten & 1 != 0,
            CL_BIN => flatten & 2 != 0,
            CL_REL => flatten & 4 != 0,
            CL_PUNCT => flatten & 8 != 0,
            CL_INNER => flatten & 16 != 0,
            _ => false,
        }
    };
    if inner.len() == 1 {
        match inner.first() {
            Some(Node::MathChar { fam, class, .. }) if *fam != 255 && bit(*class) => {
                let mut n = inner.pop().unwrap();
                if let Node::MathChar { class, .. } = &mut n {
                    *class = CL_ORD;
                }
                return n;
            }
            Some(Node::Scripts { nucleus, sup: None, sub: None, .. })
                if matches!(
                    nucleus.first(),
                    Some(Node::MathChar { fam: 255, class, .. }) if *class != CL_ORD && bit(*class)
                ) =>
            {
                let mut n = inner.pop().unwrap();
                if let Node::Scripts { nucleus, .. } = &mut n {
                    if let Some(Node::MathChar { class, c, .. }) = nucleus.first_mut() {
                        *class = CL_ORD;
                        *c = 0;
                    }
                }
                return n;
            }
            _ => {}
        }
    }
    let mut nucleus = Vec::with_capacity(inner.len() + 1);
    nucleus.push(Node::MathChar {
        fam: 255,
        c: 0,
        class: CL_ORD,
        origin: MathDiagnosticOrigin::default(), attr,
    });
    nucleus.extend(inner);
    Node::Scripts {
        nucleus,
        sup: None,
        sub: None,
        options: 0,
        attr,
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

// ---------- Frac thickness encoding ----------
//
// The `Frac` node keeps tex.web semantics: thickness = `DEFAULT_CODE` means
// the default rule, 0 = atop (no rule), anything else is explicit (an
// explicit negative `\above` thickness is legal).

/// tex.web `default_code`: "denotes default_rule_thickness"
pub(crate) const DEFAULT_CODE: i32 = 0x4000_0000;

/// texmath.c `math_fraction` codes: `\above` (0), `\over` (1), `\atop` (2)
/// and the LuaTeX-only `\Uskewed` (3); `withdelims` adds 4.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FracKind {
    Above,
    Over,
    Atop,
    Skewed,
}

/// The delimiter of a `DelimBox` marker.
#[inline]
fn delim_of(small: (u8, u32), large: (u8, u32)) -> Delim {
    Delim {
        small_fam: small.0,
        small_char: small.1,
        large_fam: large.0,
        large_char: large.1,
    }
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
            *c = u32::from(st);
        }
    } else if matches!(op.first(), Some(Node::MathChar { class: CL_OP, .. })) {
        op.insert(
            0,
            Node::MathChar {
                fam: 255,
                c: u32::from(st),
                class: CL_OP,
                origin: MathDiagnosticOrigin::default(), attr: crate::boxes::Attr::NONE,
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
fn delim_marker(d: Delim, size: u8, fence: FenceOpts, origin: MathDiagnosticOrigin, attr: crate::boxes::Attr) -> Node {
    Node::DelimBox {
        small: (d.small_fam, d.small_char),
        large: (d.large_fam, d.large_char),
        size,
        fence,
        origin, attr,
    }
}

/// Start of the mlist that a `\left`/`\middle` delimiter opened in a flat
/// math list: one past the innermost unmatched open (`size` 0) or middle
/// (`size` 3) marker; closed `\left...\right` groups spliced in earlier are
/// skipped. 0 when no such group is open.
fn open_lr_boundary(list: &[Node]) -> usize {
    let mut closed = 0usize;
    for (i, n) in list.iter().enumerate().rev() {
        if is_lr_close(n) {
            closed += 1;
        } else if let Node::DelimBox { size, .. } = n {
            match *size {
                0 if closed > 0 => closed -= 1,
                0 | 3 if closed == 0 => return i + 1,
                _ => {}
            }
        }
    }
    0
}

/// `\middle` delimiter marker (eTeX right noad with subtype middle)
#[inline]
fn is_middle(n: &Node) -> bool {
    matches!(n, Node::DelimBox { size: 3, .. })
}

/// `middle_delimiter_size` while a `\left...\right` body is being measured:
/// `\middle` delimiters produce nothing (tex.web §762 sizes them later)
const MIDDLE_UNSIZED: i32 = i32::MIN;

/// close marker of a `\left...\right` group, possibly wrapped by scripts or
/// limits (`append_script` re-wraps the tail marker)
fn is_lr_close(n: &Node) -> bool {
    match n {
        Node::DelimBox { size: 1, .. } => true,
        Node::Scripts { nucleus: op, .. } | Node::OpLimits { op, .. } => {
            matches!(op.as_slice(), [Node::DelimBox { size: 1, .. }])
        }
        _ => false,
    }
}


/// tex.web `scripts_allowed(#)`: the mlist node is a noad scripts can attach to
/// (everything but style, choice, glue, kern, penalty, rule, whatsit... nodes).
fn scripts_allowed(n: &Node) -> bool {
    matches!(
        n,
        Node::MathChar { .. }
            | Node::DelimBox { .. }
            | Node::Frac { .. }
            | Node::Radical { .. }
            | Node::Accent { .. }
            | Node::Overline { .. }
            | Node::VCenter { .. }
            | Node::Box { .. }
            | Node::Scripts { .. }
            | Node::OpLimits { .. }
            | Node::Choice
            | Node::ChoiceAlt { .. }
    )
}

/// `None` when the list holds no choice.
pub(crate) fn splice_choices_pub(list: &[Node], start: GStyle) -> Option<NodeList> {
    splice_choices(list, start)
}

/// tex.web §731: each `\mathchoice` is replaced in place by the mlist for
/// the style current at that point (pass 1 style: a `\middle` resets it to
/// the group's starting style, a `\left...\right` group keeps its style
/// changes local), so the chosen atoms take part in the surrounding
/// spacing. The choice node itself becomes a style node; a fam-255 marker
/// keeps that separating, output-free role (no cramped-style loss).
fn splice_choices(list: &[Node], start: GStyle) -> Option<NodeList> {
    if !list.iter().any(|n| matches!(n, Node::Choice)) {
        return None;
    }
    let mut nodes = list.to_vec();
    let mut style = start;
    let mut lr: Vec<GStyle> = Vec::new();
    let mut i = 0usize;
    while i < nodes.len() {
        match &nodes[i] {
            Node::Style(s, _) => style = gstyle_of(*s),
            Node::DelimBox { size: 0, .. } => lr.push(style),
            Node::DelimBox { size: 3, .. } => style = lr.last().copied().unwrap_or(start),
            Node::Choice => {
                let alts = nodes[i + 1..]
                    .iter()
                    .take_while(|n| matches!(n, Node::ChoiceAlt { .. }))
                    .count();
                let chosen = match nodes.get(i + 1 + ((style >> 1) as usize).min(alts.max(1) - 1)) {
                    Some(Node::ChoiceAlt { body, .. }) if alts > 0 => body.clone(),
                    _ => NodeList::new(),
                };
                let marker = Node::MathChar {
                    fam: 255,
                    c: 0,
                    class: CL_ORD,
                    origin: MathDiagnosticOrigin::default(), attr: crate::boxes::Attr::NONE,
                };
                nodes.splice(i..i + 1 + alts, std::iter::once(marker).chain(chosen));
            }
            n if is_lr_close(n) => {
                if let Some(s) = lr.pop() {
                    style = s;
                }
            }
            _ => {}
        }
        i += 1;
    }
    Some(nodes)
}

// =====================================================================

impl Engine {
    // ---------- mode entry / exit ----------

    pub fn enter_math(&mut self, display: bool) {
        self.enter_math_inner(display, true);
    }

    /// `\Ustartmath` / `\Ustartdisplaymath`: no second `$` is looked for.
    pub(crate) fn enter_math_cs(&mut self, display: bool) {
        self.enter_math_inner(display, false);
    }

    fn enter_math_inner(&mut self, _display: bool, peek_dollar: bool) {
        // marks alive until that outer formula also finishes.
        if self.math_lists.is_empty() && self.pending_display_formula.is_none() {
            self.math_diagnostic_sources.clear();
        }
        let math_entry_mark = self.current_known_token_source_mark();
        let mut display = _display;
        if !display && peek_dollar && self.mode == Mode::Horizontal {
            // tex.web §1134: a second math_shift promotes to display math.
            // The peek must be RAW: get_token processes \if conditionals
            // (tex.web get_next), so `$\ifmmode...` would evaluate \ifmmode
            // BEFORE mode=Math is set (getting false) and the skipped
            // branch's tokens would be consumed here
            let t = self.raw_token();
            if t.is_char() && t.cc() == 3 {
                display = true;
            } else if t != crate::input::EOF_MARKER {
                self.push_token(t);
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
            // etex.ch init_math: x = \predisplaydirection, j = LR_box
            let mut lr_direction = 0;
            let mut lr_box = None;
            if self.mode == Mode::Horizontal {
                // the interrupted paragraph's LR_save is keyed by its depth
                let lr_key = self.saved_lists.len();
                // yet) gives \predisplaysize = -max_dimen; otherwise the
                // interrupted paragraph is broken and its final line is
                // measured — before the end-of-paragraph reset clears
                // \parshape/\hangindent state.
                let was_empty = self.cur_list.is_empty()
                    || (self.engine_kind == crate::engine::EngineKind::LuaTeX
                        && matches!(self.cur_list.as_slice(), [Node::Whatsit(crate::boxes::WhatIt::LocalPar(_), _)]));
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
                self.par_primitive(Token::from_cs(self.ids.par));

                self.in_display_init = false;
                lr_direction = self.display_direction_before(lr_key);
                let prev_graf = self.prev_graf() as i64;
                let hsize = self.eqtb.dim_params[DimParam::HSize.idx() as usize] as i64;
                // §1184: display width/indent from \parshape (1-based entry
                // prev_graf+2, clamped to n) or \hangindent, else \hsize/0
                let (l, s) = if !shape.is_empty() {
                    let n = shape.len() as i64;
                    let k = (prev_graf + 2).min(n);
                    let e = shape[(k - 1) as usize];
                    // engine stores (indent, width)
                    (e.1 as i64, self.swap_parshape_indent(e.0, e.1) as i64)
                } else if hang.0 != 0
                    && ((hang.1 >= 0 && prev_graf + 2 > hang.1) || (prev_graf + 1 < -hang.1))
                {
                    let used = self.swap_hang_indent(hang.0 as i32) as i64;
                    (hsize - used.abs(), if used > 0 { used } else { 0 })
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
                        Some(line) => {
                            lr_box = self.display_prototype_box(&line);
                            self.pre_display_size_of(&line, lr_direction)
                        }
                        None => -0x3FFF_FFFF,
                    }
                };
            } else {
                // display entered from vertical mode: tex.web §1185 starts a
                // new paragraph whose zero-depth \parindent box is appended
                // first (append_to_vlist: prev_depth := box depth = 0), so
                // the interline glue above the display = baselineskip − h,
                // unless at page top where prev_depth <= ignore_depth.
                if self.prev_depth > self.ignore_depth() {
                    self.prev_depth = 0;
                }
                self.pre_display_size = -0x3FFF_FFFF;
                self.pre_display_l = self.eqtb.dim_params[DimParam::HSize.idx() as usize] as i64;
                self.pre_display_s = 0;
            }
            // tex.web push_math: the display math group level (exit_math /
            // \\endgroup pop it; dropping this push leaves one pop too many

            self.push_group_level_at(crate::eqtb::LevelType::MathShift, math_entry_mark.clone());
            self.display_lr_boxes.push(lr_box);
            // tex.web init_math order: push_math's \fam, then
            // \predisplaysize, \predisplaydirection, \displaywidth and
            // \displayindent (the order \tracingassigns shows)
            self.eqtb
                .assign_int_param(crate::prim::IntParam::CurFam, -1, false);
            self.eqtb.assign_dim_param(
                crate::prim::DimParam::PreDisplaySize,
                self.pre_display_size as i32,
                false,
            );
            self.eqtb
                .assign_int_param(IntParam::PreDisplayDirection, lr_direction, false);
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
            let outer_mode = self.mode;
            self.saved_lists.push((
                outer_mode,
                std::mem::take(&mut self.cur_list),
                self.prev_depth,
                self.space_factor,
                self.prev_graf,
                self.nest_line(),
            ));
            self.mode = Mode::DisplayMath;
            self.math_style_stack.push(MathStyle::Display);
            // tex.web init_math: \everydisplay enters the input stack before
            // build_page; an output routine fired below must preempt it.
            let toks = (*self.eqtb.tok_params[crate::prim::ToksParam::EveryDisplay.idx() as usize])
                .clone();
            if !toks.is_empty() {
                self.push_tokens_named(toks, "<everydisplay>");
            }
            // tex.web §1145: the interrupted paragraph reaches the outer page
            // builder before the display material exists. This online ordering
            // can fire a page that would otherwise absorb the later display.
            if outer_mode == Mode::Vertical {
                self.lua_page_filter(crate::lua_callbacks::page_info::BEFORE_DISPLAY, true);
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
                self.nest_line(),
            ));
            self.push_group_level_at(crate::eqtb::LevelType::MathShift, math_entry_mark);
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
    /// until the closing display shift. The tag is typeset in its own math
    /// shift group (`saved(0)` is 1 for \leqno), nested in the display's.
    pub fn start_eq_no(&mut self, leqno: bool) {
        self.flush_math_limits();
        self.push_group_level_coded(
            crate::eqtb::LevelType::MathShift,
            crate::eqtb::GroupMeta {
                spec: i32::from(leqno),
                ..crate::eqtb::GroupMeta::new(crate::eqtb::group_code::MATH_SHIFT)
            },
        );
        // tex.web <Go into ordinary math mode>: \fam is -1 in the tag's
        // group and \everymath runs
        self.eqtb
            .assign_int_param(crate::prim::IntParam::CurFam, -1, false);
        self.run_everymath();
        let formula = self.math_lists.pop().unwrap_or_default();
        self.pending_display_formula = Some(formula);
        self.math_lists.push(crate::boxes::NodeList::new());
        self.eqno_leqno = Some(leqno);
        // the tag is scanned in text style (luatex start_eq_no pushes text_style)
        self.math_style_stack.push(MathStyle::Text);
        self.show.eqno_line = self.nest_line();
    }

    fn run_everymath(&mut self) {
        let toks =
            (*self.eqtb.tok_params[crate::prim::ToksParam::EveryMath.idx() as usize]).clone();
        if !toks.is_empty() {
            self.push_tokens_named(toks, "<everymath>");
        }
    }

    /// tex.web §1195: pdfTeX typesets no formula unless families 2 and 3
    /// have at least 22 and 13 \fontdimen parameters in all three sizes.
    fn insufficient_math_fonts(&self) -> Option<&'static str> {
        if self.is_luamath() {
            return None;
        }
        if !matches!(
            self.engine_kind,
            crate::engine_mode::EngineKind::PdfTeX | crate::engine_mode::EngineKind::XeTeX
        ) {
            return None;
        }
        // font_params[f] is at least 7 in TeX (the null font has 7); a new
        // math font (xetex.web: OpenType MATH table) always suffices, other
        // native fonts have 8 parameters
        let params = |size: usize, fam: usize| {
            let fid = self.eqtb.style_fonts[size][fam];
            if self.engine_kind == crate::engine_mode::EngineKind::XeTeX {
                if self.xe_is_new_mathfont(fid) {
                    return usize::MAX;
                }
                if self.is_native_font(fid) {
                    return 8;
                }
            }
            self.eqtb.font_params.get(fid as usize).map_or(0, Vec::len).max(7)
        };
        if (0..3).any(|size| params(size, 2) < 22) {
            Some("Math formula deleted: Insufficient symbol fonts")
        } else if (0..3).any(|size| params(size, 3) < 13) {
            Some("Math formula deleted: Insufficient extension fonts")
        } else {
            None
        }
    }

    /// tex.web §1047 insert_dollar_sign: let the inserted shift go through
    /// ordinary dispatch; a nested group must close before the formula does.
    pub(crate) fn insert_dollar_sign(&mut self, token: Token) {
        self.push_token(token);
        self.error("Missing $ inserted");
        self.push_token(Token::char(3, b'$' as u32));
    }

    /// tex.web off_save: a math shift closes only a math-shift group.
    pub(crate) fn close_math_shift(&mut self, token: Token) {
        if self.eqtb.cur_group_type() == Some(crate::eqtb::LevelType::MathShift) {
            self.exit_math();
        } else {
            self.off_save(token);
        }
    }

    pub fn exit_math(&mut self) {
        self.exit_math_with(None);
    }

    /// texmath.c `after_math`; `closer` is the `\Ustartmath` family command
    /// (character, control sequence) that ended the formula instead of `$`.
    pub(crate) fn exit_math_with(&mut self, closer: Option<(u8, crate::token::CsId)>) {
        self.exit_math_core(closer, true);
    }

    /// texmath.c `finish_display_alignment`: the alignment of a display is
    /// over, so what follows it is checked at once. Assignments are done,
    /// then the next token must close the display (`$` and another `$`, or
    /// `\Ustopdisplaymath`); with `\suppressmathparerror` a `\par` there is
    /// skipped. Anything else is an error that eats the token. The display
    /// then ends without scanning for a closing `$$` again.
    pub(crate) fn finish_display_alignment(&mut self) {
        use crate::eqtb::Equiv;
        use crate::prim::Prim;
        use crate::uprim::UPrim;
        let mut t = self.do_assignments();
        loop {
            if t.is_char() && t.cc() == 3 {
                // check_second_math_shift
                let t2 = self.get_x_raw();
                if t2 != crate::input::EOF_MARKER && !(t2.is_char() && t2.cc() == 3) {
                    self.push_token(t2);
                    self.error("Display math should end with $$");
                }
            } else if self.eqtb.int_params[IntParam::SuppressMathParError.idx() as usize] != 0
                && t.is_cs()
                && matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(Prim::Par)))
            {
                t = self.get_x_raw();
                continue;
            } else {
                // check_display_math_end: `cur_chr` must be the style of
                // \Ustopdisplaymath (cramped display, 1)
                let closes = if t.is_cs() {
                    matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(Prim::U(UPrim::UStopDisplayMath))))
                } else {
                    t.chr() == 1
                };
                if !closes {
                    self.error("Display math should end with \\Ustopdisplaymath");
                }
            }
            break;
        }
        self.exit_math_core(None, false);
    }

    /// `check_end` is off when the display's closing was already checked.
    fn exit_math_core(&mut self, closer: Option<(u8, crate::token::CsId)>, check_end: bool) {
        if let Some((chr, id)) = closer {
            if chr == 0 || chr == 2 {
                // `\Ustartmath` inside math: luatex complains, then closes the
                // formula; the nest is already popped, so the mode is the
                // enclosing one
                let outer = self.saved_lists.last().map_or(self.mode, |l| l.0);
                self.report_illegal_case_in(id, outer);
            }
            if !matches!(self.mode, Mode::DisplayMath) && chr != 3 {
                // texmath.c check_inline_math_end
                self.error("Inline math should end with \\Ustopmath");
            }
        }
        let was_display = self.mode == Mode::DisplayMath;
        // a directive at the very end of the formula (`$\sum_0^1\limits$`)
        // still switches the tail op noad before conversion
        self.flush_math_limits();
        // tex.web after_math: the font check (flush_math, danger:=true)
        // precedes the second `$` of a display
        let mut danger = false;
        if let Some(message) = self.insufficient_math_fonts() {
            self.error(message);
            if let Some(list) = self.math_lists.last_mut() {
                list.clear();
            }
            danger = true;
        }
        if was_display {
            if check_end {
                match closer {
                    // tex.web §1197 <Check that another $ follows>: get_x_token; a
                    // non-math-shift token is an error and is read again (back_error)
                    None => {
                        let t = self.get_token();
                        if t != crate::input::EOF_MARKER && !(t.is_char() && t.cc() == 3) {
                            self.push_token(t);
                            self.error("Display math should end with $$");
                        }
                    }
                    // texmath.c check_display_math_end
                    Some((chr, _)) => {
                        if chr != 1 {
                            self.error("Display math should end with \\Ustopdisplaymath");
                        }
                    }
                }
            }
            // with \eqno the popped list is the tag; TeX checks again for the
            // formula itself after unsaving the tag's group
            if self.eqno_leqno.is_some() {
                // tex.web after_math: the tag's group ends before the
                // formula's fonts are checked again
                self.pop_group();
                danger = false;
                if let Some(message) = self.insufficient_math_fonts() {
                    self.error(message);
                    self.pending_display_formula = Some(NodeList::new());
                    danger = true;
                }
            }
        }
        let mlist = self.math_lists.pop().unwrap_or_default();
        // tex.web after_math reads the display registers BEFORE unsave:
        // assignments made inside the display (setspace's \everydisplay
        // scales the display skips group-locally) must still apply
        let disp_regs = if was_display {
            use crate::boxes::glue_subtype as gs;
            let g = |p: crate::prim::GlueParam| self.eqtb.glue_params[p.idx() as usize];
            let i = |p: crate::prim::IntParam| self.eqtb.int_params[p.idx() as usize];
            let ads = g(crate::prim::GlueParam::AboveDisplaySkip).param(gs::ABOVE_DISPLAY_SKIP);

            Some((
                ads,
                g(crate::prim::GlueParam::BelowDisplaySkip).param(gs::BELOW_DISPLAY_SKIP),
                g(crate::prim::GlueParam::AboveDisplayShortSkip)
                    .param(gs::ABOVE_DISPLAY_SHORT_SKIP),
                g(crate::prim::GlueParam::BelowDisplayShortSkip)
                    .param(gs::BELOW_DISPLAY_SHORT_SKIP),
                i(crate::prim::IntParam::PreDisplayPenalty),
                i(crate::prim::IntParam::PostDisplayPenalty),
                g(crate::prim::GlueParam::BaselineSkip),
                g(crate::prim::GlueParam::LineSkip),
                self.eqtb.dim_params[crate::prim::DimParam::LineSkipLimit.idx() as usize],
                i(crate::prim::IntParam::PreDisplayDirection),
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
            Some(self.run_mlist_to_hlist(&mlist, 2, is_outer_horiz))
        } else {
            None
        };
        // tex.web §1196: the math nodes take \mathsurround before unsave
        let ms = self.eqtb.dim_params[DimParam::MathSurround.idx() as usize];
        // luatex after_math builds everything before unsave_math: the
        // nodes carry the attributes in force inside the formula
        let formula_attr = self.eqtb.cur_attr;
        self.pop_group();
        let (outer_mode, outer_list, pd, sf, pg, _) = self.saved_lists.pop().unwrap_or((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
            0,
        ));
        self.prev_graf = pg;
        self.math_style_stack.pop();
        if self.eqno_leqno.is_some() {
            // the tag's text style
            self.math_style_stack.pop();
        }
        // leak guards: state set inside math must not escape it
        self.right_delim = None;
        while self
            .math_group_marks
            .last()
            .is_some_and(|(depth, _, _)| *depth > self.math_lists.len())
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
            let restored_attr = std::mem::replace(&mut self.eqtb.cur_attr, formula_attr);
            self.finish_display_math(formula, tag, danger, disp_regs.unwrap(), outer_mode);
            self.eqtb.cur_attr = restored_attr;
            // tex.web resume_after_display (§1200) ends with <Scan an
            // optional space>, after unsave has inserted any \aftergroup
            // tokens, then `if nest_ptr=1 then build_page`.
            self.scan_optional_space();
            if outer_mode == Mode::Vertical {
                self.lua_page_filter(crate::lua_callbacks::page_info::AFTER_DISPLAY, false);
                self.build_page();
            }
            return;
        }
        let hlist = inline_hlist.unwrap();
        match self.mode {
            // tex.web §22461 (finish math in text): the converted nodes are
            // SPLICED into the current hlist between math-on/math-off nodes
            // carrying \mathsurround — justification stretches into the
            // formula and lines may break inside it (never inside a box)
            Mode::Horizontal => {
                self.cur_list.push(Node::MathKern(ms, 1, formula_attr));
                self.cur_list.extend(hlist);
                self.cur_list.push(Node::MathKern(ms, 2, formula_attr));
                self.space_factor = 1000;
            }
            Mode::Vertical | Mode::InternalVertical => {
                let hbox = hpack(hlist, None, HBOX, &self.eqtb).node;
                self.vlist_append(hbox);
            }
            _ => {
                self.cur_list.push(Node::MathKern(ms, 1, formula_attr));
                self.cur_list.extend(hlist);
                self.cur_list.push(Node::MathKern(ms, 2, formula_attr));
            }
        }
    }

    fn finish_display_math(
        &mut self,
        formula: NodeList,
        tag: Option<(NodeList, bool)>,
        danger: bool,
        regs: (
            crate::boxes::Glue,
            crate::boxes::Glue,
            crate::boxes::Glue,
            crate::boxes::Glue,
            i32,
            i32,
            crate::boxes::Glue,
            crate::boxes::Glue,
            i32,
            i32,
        ),
        outer_mode: Mode,
    ) {
        // etex.ch "Retrieve the prototype box" (LR_box) and
        // \predisplaydirection (read before unsave)
        let lr_box = self.display_lr_boxes.pop().flatten();
        let x = regs.9;
        if let Some(mut page) = self.par_page_lists.pop() {
            if outer_mode == Mode::Vertical {
                // Recover the live global contribution list after any output
                // routine that ran while the display was being scanned.
                page = std::mem::take(&mut self.page_list);
            }
            if let Some((rows, final_pd)) = self.display_halign.take() {
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
                // typesets tags inside the rows themselves; fin_align already
                // shifted the rows by \displayindent (§800).
                let (ads, bds, _, _, pre, post, ..) = regs;
                page.push(Node::Penalty(pre, self.eqtb.cur_attr));
                if self.display_skip_applies(&ads) {
                    page.push(Node::Glue(ads, self.eqtb.cur_attr));
                }
                self.prev_depth = final_pd;
                page.extend(rows);
                page.push(Node::Penalty(post, self.eqtb.cur_attr));
                if self.display_skip_applies(&bds) {
                    page.push(Node::Glue(bds, self.eqtb.cur_attr));
                }
                if outer_mode == Mode::Vertical {
                    self.page_list = page;
                    self.saved_lists.push((
                        Mode::Vertical,
                        Vec::new(),
                        self.prev_depth,
                        self.space_factor,
                        self.prev_graf,
                        self.nest_line(),
                    ));
                    self.par_page_lists.push(Vec::new());
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
                        self.nest_line(),
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
            // tex.web after_math converts the equation number (when there is one)
            // before the formula itself
            let tag_hlist = tag.as_ref().map(|(tl, _)| self.run_mlist_to_hlist(tl, 2, false));
            let fh = self.run_mlist_to_hlist(&formula, 0, false);
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
                Node::Box { list, .. } if matches!(list.first(), Some(Node::Glue(_, _)))
            );
            let mut w = self.box_w(&r0.node) as i64;
            // the tag (text style, natural width); e = its width, e=0 means
            // "on a line by itself" (or absent)
            let mut a: Option<Node> = None;
            let mut leqno = false;
            let mut e = 0i64;
            let mut q = 0i64;
            if let Some((_, lq)) = tag {
                leqno = lq;
                let th = tag_hlist.unwrap_or_default();
                let mut ab = hpack(th, None, HBOX, &self.eqtb).node;
                if let Node::Box { lr, subtype, .. } = &mut ab {
                    *lr = crate::boxes::BOX_LR_DLIST;
                    *subtype = crate::boxes::list_subtype::EQUATION_NUMBER;
                }
                e = self.box_w(&ab) as i64;
                // q = e + math_quad(text_size): quad of the fam-2 symbols font
                let mq = if self.is_luamath() {
                    // luatex: round_xn_over_d(\matheqnogapstep, math quad, 1000)
                    let step = self.eqtb.int_params[IntParam::MathEqnoGapStep.idx() as usize];
                    let quad = self.math_quad_style(2);
                    i64::from(crate::tfm::round_xn_over_d(quad, step, 1000))
                } else {
                    self.fam_font(0, 2)
                        .map(|(_, f)| f.quad() as i64)
                        .unwrap_or(0)
                };
                q = e + mq;
                // tex.web §1199: `if (a=null) or danger then e:=0; q:=0`
                if danger {
                    e = 0;
                    q = 0;
                }
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
            if let Node::Box { lr, subtype, .. } = &mut r0.node {
                *lr = crate::boxes::BOX_LR_DLIST;
                *subtype = crate::boxes::list_subtype::EQUATION;
            }
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
            // etex.ch: `if pre_display_direction<0 then s:=-s-z`
            let s_clear = if x < 0 { -s - z } else { s };
            let is_short = if self.is_luamath()
                && self.eqtb.int_params[IntParam::MathEqDirMode.idx() as usize] > 0
            {
                // luatex \matheqdirmode: the tag side is judged against the
                // direction of the text
                let reversed = x < 0;
                let near = a.is_some() && ((!reversed && leqno) || (reversed && !leqno));
                !(s_clear + d <= self.pre_display_size || near)
            } else {
                !leqno && (s_clear + d > self.pre_display_size)
            };
            let (above, below) = if is_short {
                (regs.2.clone(), regs.3.clone())
            } else {
                (regs.0.clone(), regs.1.clone())
            };

            let bs = regs.6;
            let lsk = regs.7;
            let lsl = regs.8 as i64;
            // tex.web interline glue for a vlist box append (used for the
            // display line and for own-line tag boxes below)
            // tex.web append_to_vlist: new_skip_param(baseline_skip_code)
            // COPIES the parameter glue spec (stretch/shrink/orders and all)
            // and only adjusts the width; below the limit it appends
            // new_param_glue(line_skip_code) untouched. Glue is Copy, so
            let ignore_depth = self.ignore_depth();
            let ilg = |prev_depth: i32, h: i64| -> Option<crate::boxes::Glue> {
                if prev_depth <= ignore_depth {
                    return None;
                }
                let d = bs.width as i64 - prev_depth as i64 - h;
                if d < lsl {
                    return Some(lsk.param(crate::boxes::glue_subtype::LINE_SKIP));
                }
                Some(crate::boxes::Glue {
                    width: d as i32,
                    subtype: crate::boxes::glue_subtype::BASELINE_SKIP,
                    ..bs.fresh()
                })
            };
            let pre = regs.4;
            let post = regs.5;
            // §22601 `if g2>0`: TeX tests the glue-parameter CODE, so a
            // user-set \belowdisplayskip=0pt still appends a (zero) glue
            // node — only the "tag on its own line" case clears g2.
            let mut g2 = Some(below);
            page.push(Node::Penalty(pre, self.eqtb.cur_attr));
            if leqno && e == 0 {
                // \leqno with the tag on its own line ABOVE the formula:
                // tex.web append_to_vlist gives the tag box ordinary interline
                // glue from prev_depth, then prev_depth := tag depth.
                if let Some(ab) = a.take() {
                    let own_s = if self.is_luamath() { 0 } else { s };
                    let ab = self.app_display(lr_box.as_ref(), ab, 0, z, own_s, x);
                    let (th, td) = match &ab {
                        Node::Box { h, d, .. } => (*h as i64, *d as i64),
                        _ => (0, 0),
                    };
                    if let Some(g) = ilg(self.prev_depth, th) {
                        page.push(Node::Glue(g, self.eqtb.cur_attr));
                    }
                    page.push(ab);
                    self.prev_depth = td as i32;
                    page.push(Node::Penalty(crate::scaled::INF_PENALTY, self.eqtb.cur_attr));
                }
            } else if self.display_skip_applies(&above) {
                page.push(Node::Glue(above, self.eqtb.cur_attr));
            }
            // the display line itself (§22592): with a tag, b becomes
            // [formula, kern z-w-e-d, tag] (or reversed for \leqno)
            let mut line = r0.node;
            if e != 0 {
                let ab = a.take().unwrap();
                if self.is_luamath() {
                    // luatex finish_displayed_math: the line is
                    // [kern d] eq [kern] eqno (or eqno [kern] eq [kern]) and
                    // shifted by \displayindent only
                    let r = (z - w - e - d) as i32;
                    let seq = if leqno {
                        vec![ab, Node::Kern(r, self.eqtb.cur_attr), line, Node::Kern((i64::from(r) + e) as i32, crate::boxes::Attr::NONE)]
                    } else {
                        vec![Node::Kern(d as i32, self.eqtb.cur_attr), line, Node::Kern(r, crate::boxes::Attr::NONE), ab]
                    };
                    d = 0;
                    line = crate::math_otf::with_list_subtype(
                        hpack(seq, None, HBOX, &self.eqtb).node,
                        crate::boxes::list_subtype::EQUATION,
                    );
                } else {
                    let kern = Node::ExplicitKern((z - w - e - d) as i32, self.eqtb.cur_attr);
                    let (seq, nd) = if leqno {
                        (vec![ab, kern, line], 0i64)
                    } else {
                        (vec![line, kern, ab], d)
                    };
                    d = nd;
                    line = hpack(seq, None, HBOX, &self.eqtb).node;
                }
            }
            let line = self.app_display(lr_box.as_ref(), line, d, z, s, x);
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
                page.push(Node::Glue(g, self.eqtb.cur_attr));
            }
            page.push(line);
            self.prev_depth = ld as i32;
            // §22598: a right tag on its own line follows the display, flush
            // right, after an infinite penalty; the below-skip is suppressed
            if e == 0 && !leqno {
                if let Some(ab) = a.take() {
                    let aw = self.box_w(&ab) as i64;
                    page.push(Node::Penalty(crate::scaled::INF_PENALTY, self.eqtb.cur_attr));
                    let ab = self.app_display(lr_box.as_ref(), ab, z - aw, z, s, x);
                    let (th, td) = match &ab {
                        Node::Box { h, d, .. } => (*h as i64, *d as i64),
                        _ => (0, 0),
                    };
                    // tex.web §22598 appends the tag box via append_to_vlist:
                    // ordinary interline glue from the display's depth first,
                    // then prev_depth := tag box's depth.
                    if let Some(g) = ilg(self.prev_depth, th) {
                        page.push(Node::Glue(g, self.eqtb.cur_attr));
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
            page.push(Node::Penalty(post, self.eqtb.cur_attr));
            if let Some(g) = g2 {
                if self.display_skip_applies(&g) {
                    page.push(Node::Glue(g, self.eqtb.cur_attr));
                }
            }
            if outer_mode == Mode::Vertical {
                self.page_list = page;
                self.saved_lists.push((
                    Mode::Vertical,
                    Vec::new(),
                    self.prev_depth,
                    self.space_factor,
                    self.prev_graf,
                    self.nest_line(),
                ));
                self.par_page_lists.push(Vec::new());
                self.mode = Mode::Horizontal;
                self.cur_list = Vec::new();
                self.space_factor = 1000;
                self.begin_paragraph_language();
            } else {
                self.cur_list.extend(page);
                let outer = std::mem::take(&mut self.cur_list);
                self.saved_lists.push((
                    outer_mode,
                    Vec::new(),
                    self.prev_depth,
                    self.space_factor,
                    self.prev_graf,
                    self.nest_line(),
                ));
                self.par_page_lists.push(outer);
                self.mode = Mode::Horizontal;
                self.cur_list = Vec::new();
                self.space_factor = 1000;
                self.begin_paragraph_language();
            }
            // resume_after_display transfers these display nodes to the page
            // builder after consuming the optional following space (§1200).
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
    /// voids instead of being scaled. e-TeX (`display_line_size`) measures
    /// mirrored for right-to-left text before the display (`x < 0`) and
    /// reverses reflected TeXXeT segments.
    fn pre_display_size_of(&self, line: &Node, x: i32) -> i64 {
        let quad = self
            .eqtb
            .fonts
            .get(self.eqtb.cur_font_val as usize)
            .map(|f| f.quad() as i64)
            .unwrap_or(0);
        // luatex texmath.c: x_over_n(quad, 1000) * \predisplaygapfactor
        // (2000 by default) instead of 2em
        let gap = if self.engine_kind == crate::engine::EngineKind::LuaTeX {
            quad / 1000 * i64::from(self.eqtb.int_params[IntParam::PreDisplayGapFactor.idx() as usize])
        } else {
            2 * quad
        };
        self.display_line_size(line, x, gap)
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

    /// Allocate a stable, lightweight origin for a math atom. Source marks
    /// retain shared input bytes through `Rc` in an engine-side arena; the
    /// node itself stores only this arena index.
    pub(crate) fn math_diagnostic_origin(&mut self) -> MathDiagnosticOrigin {
        let source = self.current_token_source_mark();
        self.math_diagnostic_origin_at(source)
    }

    pub(crate) fn math_diagnostic_origin_at(
        &mut self,
        source: Option<crate::input::SourceMark>,
    ) -> MathDiagnosticOrigin {
        let source = (self.eqtb.int_params[IntParam::TracingLostChars.idx() as usize] > 0)
            .then_some(source)
            .flatten();
        let Some(id) = self
            .math_diagnostic_sources
            .len()
            .checked_add(1)
            .and_then(|id| u32::try_from(id).ok())
        else {
            return MathDiagnosticOrigin::default();
        };
        self.math_diagnostic_sources.push(source);
        MathDiagnosticOrigin { id }
    }

    /// Test the font selected by the eventual math style and report a lost
    /// character once for this atom.  Some Appendix G paths first measure an
    /// atom and then build it; the stable origin id prevents duplicate
    /// warnings without suppressing a distinct input occurrence.
    fn math_font_has_character_or_warn(
        &mut self,
        font_id: FontId,
        character: u8,
        origin: &MathDiagnosticOrigin,
    ) -> bool {
        let present = self
            .eqtb
            .fonts
            .get(font_id as usize)
            .is_some_and(|font| font.char_present(character));
        if present {
            return true;
        }
        if self.eqtb.int_params[IntParam::TracingLostChars.idx() as usize] <= 0 {
            return false;
        }
        if origin.id != 0
            && !self
                .reported_missing_math_atoms
                .insert((origin.id, font_id, character))
        {
            return false;
        }
        let font_name = self
            .eqtb
            .fonts
            .get(font_id as usize)
            .map(|font| {
                if font.tfm_name.is_empty() {
                    format!("font {font_id}")
                } else {
                    font.tfm_name.clone()
                }
            })
            .unwrap_or_else(|| format!("font {font_id} (not loaded)"));
        let source = usize::try_from(origin.id)
            .ok()
            .and_then(|id| id.checked_sub(1))
            .and_then(|index| self.math_diagnostic_sources.get(index))
            .and_then(Option::as_ref)
            .map(|source| source.to_context());
        self.warning_at(
            &format!(
                "Character code {character} (0x{character:02X}) is not available in font `{font_name}` selected for this math style; character omitted"
            ),
            source,
        );
        false
    }

    pub fn append_mathchar(&mut self, mc: u16) {
        let origin = self.math_diagnostic_origin();
        self.append_mathchar_with_origin(mc, origin);
    }

    pub(crate) fn append_mathchar_at(&mut self, mc: u16, source: Option<crate::input::SourceMark>) {
        let origin = self.math_diagnostic_origin_at(source);
        self.append_mathchar_with_origin(mc, origin);
    }

    fn append_mathchar_with_origin(&mut self, mc: u16, origin: MathDiagnosticOrigin) {
        if self.engine_kind == crate::engine::EngineKind::XeTeX {
            // xetex.web §26546: a tex.web mathchar in XeTeX's own layout
            self.xe_append_math_char(crate::xemath_prims::legacy_to_packed(i32::from(mc)), origin);
            return;
        }
        let mut class = (mc >> 12) as u8;
        let mut fam = ((mc >> 8) & 0xF) as u8;
        let c = (mc & 0xFF) as u8;
        // tex.web §1155 set_math_char: a class-7 (varfam) char takes the
        // current \fam when it is in range — \mathrm/\operator@font work
        // through this (\fam0 makes `ln` in \ln come out upright, not math
        // italic) — and becomes an ord noad. Class 7 on a MathChar node
        // therefore always means Inner (`\mathinner{x}`).
        if class == 7 {
            let cur = self.eqtb.int_params[crate::prim::IntParam::CurFam.idx() as usize];
            if (0..16).contains(&cur) {
                fam = cur as u8;
            }
            class = CL_ORD;
        }
        self.append_mlist_node(Node::MathChar {
            fam,
            c: u32::from(c),
            class,
            origin, attr: self.eqtb.cur_attr,
        });
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

    pub(crate) fn push_math_group_at(
        &mut self,
        left: Delim,
        fence: FenceOpts,
        source: Option<crate::input::SourceMark>,
    ) {
        // tex.web math_limit_switch fires at scan time: a directive before
        // `\left` belongs to the noad OUTSIDE the new group list
        self.flush_math_limits();
        self.saved_lists.push((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
            self.nest_line(),
        ));
        self.mode = Mode::Math;
        self.push_group_level(crate::eqtb::LevelType::MathLeft);
        let origin = self.math_diagnostic_origin_at(source);
        self.math_lists.push(vec![delim_marker(left, 0, fence, origin, self.eqtb.cur_attr)]);
    }

    /// tex.web §1192 "Try to recover from mismatched \right": `\right` or
    /// `\middle` outside a `\left` group. In the math shift group the
    /// delimiter is scanned and the command ignored; in any other group the
    /// command closes that group first (off_save). Returns whether the
    /// command was consumed here.
    pub(crate) fn mismatched_right_or_middle(&mut self, token: Token, middle: bool) -> bool {
        use crate::eqtb::group_code;
        match self.eqtb.cur_group_code() {
            group_code::MATH_LEFT => false,
            group_code::MATH_SHIFT => {
                self.scan_delim_int();
                self.error(if middle { "Extra \\middle" } else { "Extra \\right" });
                true
            }
            _ => {
                self.off_save(token);
                true
            }
        }
    }

    /// tex.web `privileged` for mmode commands (`\eqno`, `\halign`): the
    /// innermost math list is the display itself. A `{...}` group, a
    /// `\mathchoice` part, a `\left` group and the tag of `\eqno` each push
    /// a -mmode list in tex.web; Ratex keeps `Mode::DisplayMath` for them.
    pub(crate) fn display_math_is_privileged(&self) -> bool {
        use crate::eqtb::group_code;
        self.mode == Mode::DisplayMath
            && self.eqno_leqno.is_none()
            && !matches!(
                self.eqtb.cur_group_code(),
                group_code::MATH | group_code::MATH_CHOICE | group_code::MATH_LEFT
            )
    }

    /// tex.web math_left_right for `\middle`: the `\left` group ends (its
    /// local assignments are undone) and a new math left group begins at
    /// once; show_save_groups tells the two apart by `spec` 1.
    pub(crate) fn restart_math_left_group(&mut self) {
        self.pop_group();
        self.push_group_level_coded(
            crate::eqtb::LevelType::MathLeft,
            crate::eqtb::GroupMeta {
                spec: 1,
                ..crate::eqtb::GroupMeta::new(crate::eqtb::group_code::MATH_LEFT)
            },
        );
    }

    /// `\right` end of a `\left...\right` group: the inner *raw* math list is
    /// spliced into the enclosing math list, bracketed by boundary markers.
    /// Conversion (including delimiter sizing) happens in one pass later.
    pub(crate) fn pop_math_group_delimited_at(
        &mut self,
        right_delim: Delim,
        fence: FenceOpts,
        source: Option<crate::input::SourceMark>,
    ) {
        self.flush_math_limits();
        let inner = self.math_lists.pop().unwrap_or_default();
        self.pop_group();
        let (outer_mode, outer_list, pd, sf, pg, _) = self.saved_lists.pop().unwrap_or((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
            0,
        ));
        self.prev_graf = pg;
        self.mode = outer_mode;
        self.prev_depth = pd;
        self.space_factor = sf;
        self.cur_list = outer_list;
        let origin = self.math_diagnostic_origin_at(source);
        match self.math_lists.last_mut() {
            Some(l) => {
                l.extend(inner);
                l.push(delim_marker(right_delim, 1, fence, origin, self.eqtb.cur_attr));
            }
            None => {
                self.error("Missing $ inserted (\\right)");
            }
        }
    }

    // ---------- scripts, accents, radicals, fractions ----------

    /// `^` / `_`: scan the following group-or-token and attach it to the last
    /// atom of the current math list (tex.web "scripts on the tail noad").
    pub fn append_script(&mut self, sup: bool, c: u8) {
        self.append_script_opt(sup, c, false);
    }

    /// texmath.c `do_sub_sup(no)`: `no_script` marks the noad with
    /// `noad_option_no_sub_script`/`no_super_script` (`\Unosubscript`,
    /// `\Unosuperscript`): that script is typeset in the style of the noad.
    pub fn append_script_opt(&mut self, sup: bool, _c: u8, no_script: bool) {
        let limits_req = self.math_limits.take();
        if self.script_repeats(sup) {
            // tex.web sub_sup: a second script of the same kind goes on a fresh noad
            self.error(if sup {
                "Double superscript"
            } else {
                "Double subscript"
            });
            self.append_mlist_node(Node::Scripts {
                nucleus: Vec::new(),
                sup: None,
                sub: None,
                options: 0,
                attr: self.eqtb.cur_attr,
            });
        }
        self.show.scan_owner = Some(ScanKind::Script { sup, limits: limits_req });
        let group = self.scan_math_group_or_token();
        let cur_depth = self.math_lists.len();
        let group_boundary = self
            .math_group_marks
            .iter()
            .rev()
            .find(|(d, _, _)| *d == cur_depth)
            .map(|(_, mark, _)| *mark)
            .unwrap_or(0);
        let popped = if let Some(l) = self.math_lists.last_mut() {
            if l.len() <= group_boundary {
                None
            } else if l
                .iter()
                .rev()
                .take(4)
                .all(|n| matches!(n, Node::ChoiceAlt { .. }))
                && l.len().saturating_sub(group_boundary) >= 5
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
                    options: 0,
                    attr: self.eqtb.cur_attr,
                })
            } else {
                l.pop()
            }
        } else {
            None
        };

        let top = match popped {
            // tex.web `scripts_allowed(tail)`: a style, glue, kern, penalty... node
            // stays in the list and the script goes on a new empty noad
            Some(node) if !scripts_allowed(&node) => {
                self.append_mlist_node(node);
                Node::Scripts { nucleus: Vec::new(), sup: None, sub: None, options: 0, attr: crate::boxes::Attr::NONE }
            }
            Some(node) => node,
            None => Node::Scripts {
                nucleus: Vec::new(),
                sup: None,
                sub: None,
                options: 0,
                attr: self.eqtb.cur_attr,
            },
        };
        match top {
            Node::Scripts {
                mut nucleus,
                sup: s,
                sub: x,
                options: opts,
                .. } => {
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
                        ..
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
                    ..
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
                                origin: MathDiagnosticOrigin::default(), attr: self.eqtb.cur_attr,
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
                        below: nb, attr: self.eqtb.cur_attr,
                    });
                } else {
                    let (ns, nx) = if sup {
                        (Some(group), x)
                    } else {
                        (s, Some(group))
                    };
                    let no_bit = match (no_script, sup) {
                        (false, _) => 0,
                        (true, true) => crate::boxes::noad_option::NO_SUPER_SCRIPT,
                        (true, false) => crate::boxes::noad_option::NO_SUB_SCRIPT,
                    };
                    self.append_mlist_node(Node::Scripts {
                        nucleus,
                        sup: ns,
                        sub: nx,
                        options: opts | no_bit,
                        attr: self.eqtb.cur_attr,
                    });
                }
            }
            Node::OpLimits {
                mut op,
                above,
                below, .. } => {
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
                    below: nb, attr: self.eqtb.cur_attr,
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
                                origin: MathDiagnosticOrigin::default(), attr: self.eqtb.cur_attr,
                            },
                        );
                    }
                    self.append_mlist_node(Node::OpLimits {
                        op,
                        above: sup_g,
                        below: sub_g, attr: self.eqtb.cur_attr,
                    });
                } else {
                    self.append_mlist_node(Node::Scripts {
                        nucleus: vec![atom],
                        sup: sup_g,
                        sub: sub_g,
                        options: match (no_script, sup) {
                            (false, _) => 0,
                            (true, true) => crate::boxes::noad_option::NO_SUPER_SCRIPT,
                            (true, false) => crate::boxes::noad_option::NO_SUB_SCRIPT,
                        },
                        attr: self.eqtb.cur_attr,
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
                options,
                .. } => {
                let head_op = matches!(nucleus.first(), Some(Node::MathChar { class: CL_OP, .. }));
                if !head_op {
                    *tail = Node::Scripts { nucleus, sup, sub, options, attr: self.eqtb.cur_attr };
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
                        ..
                    }) = nucleus.first_mut()
                    {
                        *c = 2;
                    }
                    *tail = Node::Scripts { nucleus, sup, sub, options, attr: self.eqtb.cur_attr };
                    return;
                }
                set_limits_subtype(&mut nucleus, st);
                if sup.is_some() || sub.is_some() {
                    *tail = Node::OpLimits {
                        op: nucleus,
                        above: sup,
                        below: sub, attr: self.eqtb.cur_attr,
                    };
                } else {
                    *tail = Node::Scripts { nucleus, sup, sub, options, attr: self.eqtb.cur_attr };
                }
            }
            Node::OpLimits {
                mut op,
                above,
                below, .. } => {
                set_limits_subtype(&mut op, st);
                *tail = Node::OpLimits { op, above, below, attr: self.eqtb.cur_attr };
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
                origin, .. } => {
                let mut op = vec![Node::MathChar {
                    fam,
                    c,
                    class: CL_OP,
                    origin, attr: self.eqtb.cur_attr,
                }];
                set_limits_subtype(&mut op, st);
                *tail = Node::OpLimits {
                    op,
                    above: None,
                    below: None, attr: self.eqtb.cur_attr,
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
        let owner = self.show.scan_owner.take();
        self.flush_math_limits();
        self.skip_spaces_relax();
        let t = self.get_token();
        if t == crate::input::EOF_MARKER {
            self.error("Missing { inserted in math mode");
            return Vec::new();
        }
        if t.is_char() && t.cc() == 1 {
            return self.scan_math_group_braced(owner.unwrap_or(ScanKind::Brace));
        }
        // single token: run it into a temporary math list
        let xe_char = self.engine_kind == crate::engine::EngineKind::XeTeX
            && self.xe_scan_math_char_token(t);
        self.begin_math_scan(owner.unwrap_or(ScanKind::Brace));
        self.run_math_token(t);
        self.flush_math_limits();
        let mut field = self.end_math_scan();
        if xe_char {
            crate::xemath_prims::xe_char_field(&mut field);
        }
        field
    }

    /// Execute tokens up to the matching `}` as a nested math list.
    fn scan_math_group_braced(&mut self, kind: ScanKind) -> NodeList {
        let my_level = self.open_math_group(kind);
        self.scan_math_group_body(my_level)
    }

    /// Push the group of a braced subformula whose `{` is read. tex.web
    /// build_choices pushes math_choice_group (13), every other braced
    /// subformula is a math_group (9).
    fn open_math_group(&mut self, kind: ScanKind) -> u16 {
        let code = if matches!(kind, ScanKind::Choice) {
            crate::eqtb::group_code::MATH_CHOICE
        } else {
            crate::eqtb::group_code::MATH
        };
        self.begin_math_scan(kind);
        self.push_group_level_coded(
            crate::eqtb::LevelType::MathGroup,
            crate::eqtb::GroupMeta::new(code),
        );
        self.eqtb.cur_level
    }

    /// The tokens of the group opened at `my_level`, up to its `}`.
    fn scan_math_group_body(&mut self, my_level: u16) -> NodeList {
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
                    Some(crate::eqtb::LevelType::Group | crate::eqtb::LevelType::MathGroup) => {
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
                    let inner = self.scan_math_group_braced(ScanKind::Brace);
                    let flatten = self.math_flatten_mode();
                    self.append_mlist_node(finish_math_group(inner, flatten, self.eqtb.cur_attr));
                } else {
                    self.begin_group(true);
                }
                continue;
            }
            self.run_math_token(t);
        }
        self.flush_math_limits();
        self.end_math_scan()
    }

    pub(crate) fn do_math_accent_at(&mut self, mc: u16, source: Option<crate::input::SourceMark>) {
        if self.engine_kind == crate::engine::EngineKind::XeTeX {
            self.do_xe_math_accent(crate::xemath_prims::legacy_to_packed(i32::from(mc)), 0, source);
            return;
        }
        let origin = self.math_diagnostic_origin_at(source);
        let mut fam = ((mc >> 8) & 0xF) as u8;
        let c = (mc & 0xFF) as u8;
        // tex.web math_ac: a variable-family code takes \fam when it is in range
        let cur_fam = self.eqtb.int_params[IntParam::CurFam.idx() as usize];
        if mc >= 0x7000 && (0..16).contains(&cur_fam) {
            fam = cur_fam as u8;
        }
        let spec = AccentSpec {
            top: Some((fam, u32::from(c))),
            ..AccentSpec::default()
        };
        self.append_accent_noad(spec, origin);
    }

    /// Scan the nucleus of an accent noad (the accent characters are
    /// already scanned) and append the noad.
    pub(crate) fn append_accent_noad(&mut self, spec: AccentSpec, origin: MathDiagnosticOrigin) {
        self.show.scan_owner = Some(ScanKind::Accent(spec));
        let group = self.scan_math_group_or_token();
        self.append_mlist_node(Node::Accent {
            spec,
            body: group,
            origin, attr: self.eqtb.cur_attr,
        });
    }

    pub(crate) fn do_radical_at(&mut self, delim: Delim, source: Option<crate::input::SourceMark>) {
        let origin = self.math_diagnostic_origin_at(source);
        self.show.scan_owner = Some(ScanKind::Radical {
            delim,
            subtype: 0,
            width: 0,
            options: 0,
            degree: None,
        });
        let group = self.scan_math_group_or_token();
        self.append_mlist_node(Node::Radical {
            body: group,
            delim,
            subtype: 0,
            width: 0,
            options: 0,
            degree: None,
            origin, attr: self.eqtb.cur_attr,
        });
    }

    pub fn do_math_class(&mut self, class: u8) {
        self.show.scan_owner = Some(ScanKind::Class(class));
        let field = self.scan_math_group_or_token();
        let node = if field.is_empty() {
            Node::MathChar {
                fam: 255,
                c: 0,
                class,
                origin: MathDiagnosticOrigin::default(), attr: self.eqtb.cur_attr,
            }
        } else if field.len() == 1 {
            match field.into_iter().next().unwrap() {
                Node::MathChar {
                    fam,
                    c,
                    class: char_class,
                    origin, .. } if char_class == CL_ORD => Node::MathChar {
                    fam,
                    c,
                    class,
                    origin, attr: self.eqtb.cur_attr,
                },
                other => {
                    let nuc = vec![
                        Node::MathChar {
                            fam: 255,
                            c: 0,
                            class,
                            // a group around one non-ord noad (`\mathop{\sum}`): its
                            // nucleus is a sub-mlist, not the noad's own character
                            origin: MathDiagnosticOrigin { id: u32::MAX }, attr: self.eqtb.cur_attr,
                        },
                        other,
                    ];
                    Node::Scripts {
                        nucleus: nuc,
                        sup: None,
                        sub: None,
                        options: 0,
                        attr: self.eqtb.cur_attr,
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
                origin: MathDiagnosticOrigin::default(), attr: self.eqtb.cur_attr,
            }];
            nuc.extend(field);
            Node::Scripts {
                nucleus: nuc,
                sup: None,
                sub: None,
                options: 0,
                attr: self.eqtb.cur_attr,
            }
        };
        self.append_mlist_node(node);
    }

    /// Knuth \\overline / \\underline: scan a math field, pack it, and
    /// put a default-rule bar above (or below) with 3 default_rule_thickness
    /// clearance (tex.web make_over / make_under).
    pub fn do_overline(&mut self, under: bool) {
        self.show.scan_owner = Some(if under { ScanKind::Under } else { ScanKind::Over });
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
            depth: 0, subtype: crate::boxes::RULE_NORMAL, index: 0, attr: self.eqtb.cur_attr,
        };
        let mut vlist = Vec::new();
        if under {
            vlist.push(packed);
            vlist.push(Node::Kern(kern, self.eqtb.cur_attr));
            vlist.push(rule);
        } else {
            // tex.web overbar: an extra rule-thickness kern above the rule.
            vlist.push(Node::Kern(rt, self.eqtb.cur_attr));
            vlist.push(rule);
            vlist.push(Node::Kern(kern, self.eqtb.cur_attr));
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
        self.append_mlist_node(Node::Overline {
            body: group,
            under,
            fam: crate::boxes::NO_FAM,
            packed: Box::new(vb), attr: self.eqtb.cur_attr,
        });
    }

    /// tex.web scan_delimiter (§1160, r=false): after the next non-blank
    /// non-relax non-call token, a letter or other character uses its
    /// `\delcode` and `\delimiter` scans a 27-bit code; any other token (and
    /// a negative `\delcode`) is `Missing delimiter (. inserted)`, backed up
    /// so it is read again, and the null delimiter is used.
    pub fn scan_delim_int(&mut self) -> i32 {
        self.skip_spaces_relax();
        let t = self.get_token();
        let token_source = self.current_token_source_mark();
        let code = if t.is_char() && matches!(t.cc(), 11 | 12) {
            if self.is_luamath() {
                // luatex `\delcode` (get_del_code): -1 = undefined
                let (sf, sc, lf, lc) = self.eqtb.lua_del_code(t.chr());
                if sf < 0 {
                    -1
                } else {
                    (i64::from(sf & 0xF) << 20) | (i64::from(sc & 0xFF) << 12) | (i64::from(lf & 0xF) << 8) | i64::from(lc & 0xFF)
                }
            } else if self.engine_kind == crate::engine::EngineKind::XeTeX {
                // xetex.web §26749: `del_code(cur_chr)` for every USV
                i64::from(self.eqtb.xe_del_code(t.chr()))
            } else {
                self.eqtb.delimiter_code_for(t.chr())
            }
        } else if t.is_cs()
            && matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(Prim::Delimiter)))
        {
            return self.scan_delimiter_code("\\delimiter");
        } else if t.is_cs()
            && matches!(
                self.eqtb.resolve(t.cs_id()),
                Some(Equiv::Prim(Prim::XeMath(crate::xemath_prims::XeMath::Delimiter)))
            )
        {
            // `\Udelimiter <class> <fam> <usv>`: the class is discarded (§26751)
            self.scan_xe_math_class();
            return self.scan_xe_fam_usv_delcode();
        } else {
            -1
        };
        if let Ok(code) = i32::try_from(code) {
            if code >= 0 {
                return code;
            }
        }
        // back_error: the offending token is read again
        if t != crate::input::EOF_MARKER {
            self.push_token(t);
        }
        self.error_at(
            "Missing delimiter (. inserted)",
            token_source.map(|mark| mark.to_context()),
        );
        0
    }

    pub(crate) fn scan_delimiter_code(&mut self, command: &str) -> i32 {
        if self.engine_kind == crate::engine::EngineKind::XeTeX {
            return self.scan_xe_delimiter_int();
        }
        let (value, source) = self.scan_int_with_source();
        if (0..0x0800_0000).contains(&value) {
            value
        } else {
            self.error_at(
                &format!(
                    "Delimiter code {value} is out of range for {command}; expected 0 through 134217727 and used the null delimiter"
                ),
                source,
            );
            0
        }
    }

    pub fn do_fraction(&mut self, p: Prim) {
        let (kind, delimited) = match p {
            Prim::Above => (FracKind::Above, false),
            Prim::AboveWithDelims => (FracKind::Above, true),
            Prim::Over => (FracKind::Over, false),
            Prim::OverWithDelims => (FracKind::Over, true),
            Prim::Atop => (FracKind::Atop, false),
            _ => (FracKind::Atop, true),
        };
        self.do_fraction_kind(kind, delimited);
    }

    /// texmath.c `math_fraction`; the LuaTeX-only `\Uskewed` and
    /// `\Uskewedwithdelims` come in through [`FracKind::Skewed`].
    pub(crate) fn do_fraction_kind(&mut self, kind: FracKind, delimited: bool) {
        let origin = self.math_diagnostic_origin();
        let lua = self.engine_kind == crate::engine::EngineKind::LuaTeX;
        if self.fraction_is_ambiguous() {
            // tex.web §1181: the arguments are scanned, then the fraction is ignored
            if delimited {
                self.scan_delim(!lua);
                self.scan_delim(!lua);
            }
            if kind == FracKind::Above {
                self.scan_dimen(false, false);
            }
            self.error("Ambiguous; you need another { and }");
            return;
        }
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
                    .find(|(d, _, _)| *d == cur_depth)
                    .map(|(_, mark, _)| *mark);
                match m {
                    Some(m) if m <= l.len() => l.split_off(m),
                    // no brace at this depth: a `\left`/`\middle` group list
                    // starts its numerator after its own open delimiter
                    // (tex.web math_left_right starts a fresh mlist there)
                    _ => {
                        let start = open_lr_boundary(l);
                        l.split_off(start)
                    }
                }
            }
            None => Vec::new(),
        };
        // denominator is scanned into a temporary list on top
        self.begin_math_scan(ScanKind::Denominator(PendingFrac {
            num,
            thickness: DEFAULT_CODE,
            left: Delim::default(),
            right: Delim::default(),
        }));
        // tex.web: the lexically-following arguments (\above's dimen, the
        // withdelims delimiter pair) are scanned immediately...
        let mut thickness = DEFAULT_CODE;
        let mut options = 0u16;
        let mut middle = None;
        let mut ld = Delim::default();
        let mut rd = Delim::default();
        if kind == FracKind::Skewed {
            middle = Some(self.scan_delim(true));
        }
        if delimited {
            ld = self.scan_delim(true);
            rd = self.scan_delim(true);
        }
        match kind {
            FracKind::Above => {
                if lua {
                    // texmath.c math_fraction: `exact` and `norule`
                    loop {
                        if self.scan_keyword(b"exact") {
                            options |= noad_option::EXACT;
                        } else if self.scan_keyword(b"norule") {
                            options |= noad_option::NO_RULE;
                        } else {
                            break;
                        }
                    }
                }
                thickness = self.scan_dimen(false, false);
            }
            FracKind::Over => {}
            FracKind::Atop => thickness = 0,
            FracKind::Skewed => {
                loop {
                    if self.scan_keyword(b"exact") {
                        options |= noad_option::EXACT;
                    } else if self.scan_keyword(b"noaxis") {
                        options |= noad_option::NO_AXIS;
                    } else {
                        break;
                    }
                }
                thickness = 0;
            }
        }
        // ...and the denominator is the REST of the current math group (up
        // to the closing brace / end of formula), which stays unconsumed
        self.set_pending_fraction(thickness, ld, rd);
        let den = self.scan_math_rest_of_group();
        let num = self.take_pending_numerator();
        self.append_mlist_node(Node::Frac {
            num,
            den,
            thickness,
            left: ld,
            right: rd,
            middle: middle.unwrap_or_default(),
            options: if delimited && self.engine_kind == crate::engine::EngineKind::LuaTeX { options | noad_option::FRAC_DELIMS } else { options },
            fam: crate::boxes::NO_FAM,
            origin, attr: self.eqtb.cur_attr,
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
                    self.push_token(t);
                    break;
                }
                if t.cc() == 3 && self.eqtb.cur_level <= start_level {
                    self.push_token(t);
                    break;
                }
            }
            // tex.web: a display's \eqno/\leqno also closes the fraction's
            // denominator — the tag is a separate sublist, not formula tail.
            // `\right`/`\middle` at this level end the `\left` group's mlist
            // that holds the fraction (tex.web math_left_right).
            if t.is_cs()
                && self.eqtb.cur_level <= start_level
                && matches!(
                    self.eqtb.resolve(t.cs_id()),
                    Some(crate::eqtb::Equiv::Prim(crate::prim::Prim::EqNo))
                        | Some(crate::eqtb::Equiv::Prim(crate::prim::Prim::LeqNo))
                        | Some(crate::eqtb::Equiv::Prim(crate::prim::Prim::Right))
                        | Some(crate::eqtb::Equiv::Prim(crate::prim::Prim::Middle))
                )
            {
                self.push_token(t);
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
    pub(crate) fn fparam(&self, g: GStyle, fam: u8, i: usize) -> i32 {
        if self.engine_kind == crate::engine::EngineKind::XeTeX {
            if let Some(v) = self.xe_family_param(font_size(g), fam, i) {
                return v;
            }
        }
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
    pub(crate) fn fparam_idx(&self, size_idx: usize, fam: u8, i: usize) -> i32 {
        if self.engine_kind == crate::engine::EngineKind::XeTeX {
            if let Some(v) = self.xe_family_param(size_idx, fam, i) {
                return v;
            }
        }
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

    pub(crate) fn math_quad(&self, g: GStyle) -> i32 {
        let q = self.fparam(g, 2, 6);
        if q != 0 {
            q
        } else {
            ONE
        }
    }

    pub(crate) fn math_x_height(&self, g: GStyle) -> i32 {
        let x = self.fparam(g, 2, 5);
        if x != 0 {
            x
        } else {
            ONE * 45 / 100
        }
    }

    /// default rule thickness: fontdimen 8 of family 3 (the extension font)
    /// at the current size (tex.web `default_rule_thickness`)
    pub(crate) fn default_rule_thickness(&self, g: GStyle) -> i32 {
        let r = self.fparam(g, 3, 8);
        if r != 0 {
            r
        } else {
            ONE / 4
        }
    }

    pub(crate) fn axis_height(&self, g: GStyle) -> i32 {
        let a = self.fparam(g, 2, 22);
        if a != 0 {
            a
        } else {
            ONE * 25 / 100
        }
    }

    /// A `\Umath` parameter value a LuaTeX job has defined (luatex
    /// `get_math_param`); other engines and undefined parameters read the
    /// font parameters directly.
    fn umath_param(&self, param: u32, style: GStyle) -> Option<i32> {
        if self.engine_kind != crate::engine::EngineKind::LuaTeX {
            return None;
        }
        let value = self.eqtb.math_param(param, style);
        (value != crate::eqtb::UNDEFINED_MATH_PARAMETER).then_some(value)
    }

    /// \mathchoice{D}{T}{S}{SS}: scan the four style groups immediately and
    /// attach them as ChoiceAlt bodies of a Choice atom; mlist_to_hlist picks
    /// the branch matching the current style. tex.web build_choices opens
    /// each part with scan_left_brace, so a part that does not start with
    /// `{` reports "Missing { inserted" and still opens its group.
    pub fn begin_mathchoice(&mut self) {
        self.append_mlist_node(Node::Choice);
        for branch in 0..4u8 {
            self.flush_math_limits();
            // tex.web build_choices: push_math(math_choice_group) comes
            // before scan_left_brace, so the group is entered (and traced)
            // before the next token is read
            let my_level = self.open_math_group(ScanKind::Choice);
            self.skip_spaces_relax();
            let t = self.get_x_raw();
            if !self.token_is_left_brace(t) {
                self.push_token(t);
                self.error("Missing { inserted");
            }
            // each branch is scanned in its own style (`\mathstyle`)
            self.math_style_stack.push(math_style_of(branch * 2));
            let body = self.scan_math_group_body(my_level);
            self.math_style_stack.pop();
            self.append_mlist_node(Node::ChoiceAlt { body, attr: self.eqtb.cur_attr });
        }
    }

    /// LuaTeX `\Ustack {<mlist>}` (texmath.c `setup_math_style`): an Ord
    /// noad whose nucleus is the braced subformula. Unlike plain braces the
    /// group never reduces to a single character noad; luatex scans it in
    /// the numerator style, which only `\mathstyle` could observe.
    pub(crate) fn do_ustack(&mut self) {
        self.flush_math_limits();
        self.skip_spaces_relax();
        let t = self.get_token();
        if !(t.is_char() && t.cc() == 1) {
            self.push_token(t);
            self.error("Missing { inserted");
        }
        let g = gstyle_of(self.cur_math_style());
        self.math_style_stack.push(math_style_of(num_style(g)));
        let inner = self.scan_math_group_braced(ScanKind::Brace);
        self.math_style_stack.pop();
        let mut nucleus = Vec::with_capacity(inner.len() + 1);
        nucleus.push(Node::MathChar {
            fam: 255,
            c: 0,
            class: CL_ORD,
            origin: MathDiagnosticOrigin::default(), attr: self.eqtb.cur_attr,
        });
        nucleus.extend(inner);
        self.append_mlist_node(Node::Scripts { nucleus, sup: None, sub: None, options: 0, attr: self.eqtb.cur_attr });
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
    pub fn math_to_hlist(&mut self, list: &[Node], style: MathStyle) -> NodeList {
        self.mlist_to_hlist_pen(list, gstyle_of(style), false)
    }

    /// classify a raw node as a spacing atom; None = not an atom
    fn atom_class(&self, n: &Node) -> Option<u8> {
        match n {
            Node::MathChar { fam: 255, .. } => None,
            Node::MathChar { class, .. } => Some(*class),
            Node::Scripts { nucleus, .. } => Some(match nucleus.first() {
                // fam255 prefix carries the atom's class (op groups etc.)
                Some(Node::MathChar { class, .. }) => *class,
                _ => CL_ORD,
            }),
            Node::OpLimits { .. } => Some(CL_OP),
            // tex.web pass 2 (§14983 case): fraction_noad keeps the default
            // t=ord_noad — fractions take Ord spacing, NOT Inner (Inner is
            // for \mathinner atoms only)
            Node::Frac { .. } => Some(CL_ORD),
            Node::Radical { .. } => Some(CL_ORD),
            Node::Accent { .. } => Some(CL_ORD),
            // `\middle` is a right noad: close on its left side (the open
            // side is applied by the callers, see `is_middle`)
            Node::DelimBox { size, .. } => Some(match size {
                0 => CL_OPEN,
                1 | 3 => CL_CLOSE,
                _ => CL_ORD,
            }),
            Node::Box { .. } | Node::VCenter { .. } | Node::Overline { .. } => Some(CL_ORD),
            Node::Choice => Some(CL_ORD),
            _ => None,
        }
    }

    fn math_noad_char(n: &Node) -> Option<(u8, u32)> {
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
            Node::MathChar { c: current, .. } => *current = u32::from(c),
            Node::Scripts { nucleus, .. } => {
                if let Some(Node::MathChar { c: current, .. }) = nucleus.first_mut() {
                    *current = u32::from(c);
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
            if let Node::Style(s, _) = &nodes[i] {
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
                if q_fam != p_fam || (q_fam as usize >= 16 && self.engine_kind != crate::engine::EngineKind::XeTeX) {
                    break;
                }

                // tex.web §14865: this happens before testing whether the
                // font actually supplies a ligature or kern instruction.
                math_text[i] = true;
                let Some((_, font)) = self.fam_font_idx(font_size(style), q_fam) else {
                    break;
                };
                // characters above 255 have no lig/kern program
                let (Ok(q_char), Ok(p_char)) = (u8::try_from(q_char), u8::try_from(p_char)) else {
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
                // luatex mlist.c: \noligs / \nokerns switch the font's math
                // ligatures and kerns off
                if self.engine_kind == crate::engine::EngineKind::LuaTeX
                    && self.eqtb.int_params
                        [(if kern.is_some() { IntParam::NoKerns } else { IntParam::NoLigs }).idx() as usize]
                        != 0
                {
                    break;
                }
                if let Some(kern) = kern {
                    nodes.insert(i + 1, Node::Kern(kern, self.eqtb.cur_attr));
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
                                c: u32::from(replacement),
                                class: CL_ORD,
                                origin: MathDiagnosticOrigin::default(), attr: self.eqtb.cur_attr,
                            },
                        );
                        math_text.insert(i + 1, op == 11);
                    }
                    _ => {
                        let p = nodes.remove(i + 1);
                        math_text.remove(i + 1);
                        Self::set_math_noad_char(&mut nodes[i], replacement);
                        if let Node::Scripts { sup, sub, options, .. } = p {
                            let nucleus = match nodes[i].clone() {
                                Node::Scripts { nucleus, .. } => nucleus,
                                q => vec![q],
                            };
                            nodes[i] = Node::Scripts { nucleus, sup, sub, options, attr: self.eqtb.cur_attr };
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
    pub(crate) fn mlist_to_hlist_pen(&mut self, list: &[Node], start: GStyle, pen: bool) -> NodeList {
        self.mlist_to_hlist_full(list, start, pen, false)
    }

    /// `node.mlist_to_hlist` (Lua): convert the math nodes of `list`.
    pub(crate) fn lua_mlist_to_hlist(&mut self, list: &[Node], style: GStyle, pen: bool) -> NodeList {
        self.mlist_to_hlist_pen(list, style, pen)
    }

    /// `lr_body`: the list is the inside of a `\left...\right` group, so a
    /// close noad (the right delimiter) follows it for spacing purposes
    fn mlist_to_hlist_full(
        &mut self,
        list: &[Node],
        start: GStyle,
        pen: bool,
        lr_body: bool,
    ) -> NodeList {
        let outermost = self.math_diagnostic_depth == 0;
        if outermost {
            self.reported_missing_math_atoms.clear();
        }
        self.math_diagnostic_depth += 1;
        let saved_pen = self.math_penalties.replace(pen);
        let out = if self.is_luamath() {
            self.lm_mlist_to_hlist(list, start, pen)
        } else {
            self.mlist_to_hlist_inner(list, start, lr_body)
        };
        self.math_penalties.set(saved_pen);
        self.math_diagnostic_depth -= 1;
        out
    }

    fn mlist_to_hlist_inner(&mut self, list: &[Node], start: GStyle, lr_body: bool) -> NodeList {
        let spliced = splice_choices(list, start);
        let list = spliced.as_deref().unwrap_or(list);
        let (list, math_text_chars) = self.prepare_math_ligatures(list, start);
        // pass 1: classify atoms and demote binary operators that cannot be
        // binary in context (tex.web §760)
        let classes: Vec<Option<u8>> = list.iter().map(|n| self.atom_class(n)).collect();
        let mut eff: Vec<Option<u8>> = classes.clone();
        for i in 0..list.len() {
            if eff[i] == Some(CL_BIN) {
                let mut left = None;
                let mut right = None;
                for j in (0..i).rev() {
                    if eff[j].is_some() {
                        // tex.web §727: after a right/middle noad r_type
                        // becomes left_noad, which demotes a following bin
                        left = if is_middle(&list[j]) { Some(CL_OPEN) } else { eff[j] };
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
        // tex.web keeps one cur_style for both passes, except that pass 1
        // resets it to the list's starting style after a `\middle` (§727)
        // while pass 2 (spacing) only follows style nodes
        let mut style: GStyle = start;
        let mut sp_style: GStyle = start;
        // the open \left...\right group of this list: (left delimiter code and
        // origin, buffered raw nodes). A nested group stays raw in the buffer
        // (`lr_nest` counts its open markers) so the recursive conversion of
        // the body builds it as an Inner atom in the body's own style
        // (tex.web: the nested group is an inner_noad of the outer sub_mlist).
        let mut lr_open: Option<(Delim, MathDiagnosticOrigin, NodeList)> = None;
        let mut lr_nest = 0usize;
        let mut i = 0usize;
        while i < list.len() {
            let n = &list[i];
            if let Node::Style(s, _) = n {
                if let Some((_, _, buf)) = lr_open.as_mut() {
                    buf.push(n.clone());
                } else {
                    style = gstyle_of(*s);
                    sp_style = style;
                }
                i += 1;
                continue;
            }
            if let Some(cls) = eff[i] {
                // boundary markers
                if let Node::DelimBox {
                    size: 0,
                    small,
                    large,
                    origin,
                    ..
                } = n
                {
                    if let Some((_, _, buf)) = lr_open.as_mut() {
                        buf.push(n.clone());
                        lr_nest += 1;
                    } else {
                        lr_open = Some((delim_of(*small, *large), origin.clone(), Vec::new()));
                    }
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
                        origin,
                        ..
                    } => Some((*small, *large, None, None, origin)),
                    Node::Scripts { nucleus, sup, sub, .. } => match nucleus.as_slice() {
                        [Node::DelimBox {
                            size: 1,
                            small,
                            large,
                            origin,
                            ..
                        }] => Some((
                            *small,
                            *large,
                            Some((sup.as_deref(), sub.as_deref())),
                            None,
                            origin,
                        )),
                        _ => None,
                    },
                    Node::OpLimits { op, above, below, .. } => match op.as_slice() {
                        [Node::DelimBox {
                            size: 1,
                            small,
                            large,
                            origin,
                            ..
                        }] => Some((
                            *small,
                            *large,
                            None,
                            Some((above.as_deref(), below.as_deref())),
                            origin,
                        )),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some((small, large, scripts, limits, close_origin)) = close {
                    if lr_nest > 0 {
                        lr_nest -= 1;
                        if let Some((_, _, buf)) = lr_open.as_mut() {
                            buf.push(n.clone());
                        }
                        i += 1;
                        continue;
                    }
                    let close_delim = delim_of(small, large);
                    match lr_open.take() {
                        Some((lopen, open_origin, buf)) => {
                            // tex.web §762: max_h/max_d come from the inner
                            // noads only; `\middle` delimiters are sized
                            // afterwards and do not count
                            let saved_mid = self.middle_delimiter_size;
                            self.middle_delimiter_size = MIDDLE_UNSIZED;
                            let body_measure = self.mlist_to_hlist_full(&buf, style, false, true);
                            let (_, bh, bd) = hlist_dims(&body_measure, &self.eqtb);
                            let needed = self.lr_delimiter_size(bh, bd, style);
                            self.middle_delimiter_size = needed;
                            let body = self.mlist_to_hlist_full(&buf, style, false, true);
                            self.middle_delimiter_size = saved_mid;
                            let mut assembled: NodeList =
                                self.var_delimiter(lopen, needed, style, &open_origin);
                            assembled.extend(body);
                            assembled.extend(self.var_delimiter(close_delim, needed, style, close_origin));
                            let gb = hpack(assembled, None, HBOX, &self.eqtb).node;
                            let tail: NodeList = match (scripts, limits) {
                                (Some((sup, sub)), _) => self.make_scripts(&[gb], sup, sub, style),
                                (None, Some((above, below))) => {
                                    self.make_op_limits(&[gb], above, below, style, true)
                                }
                                _ => vec![gb],
                            };
                            self.emit_atom(&mut out, &mut prev, Some(CL_INNER), tail, sp_style);
                        }
                        None => {
                            // A stray close is an ordinary close delimiter.
                            // Build it before borrowing `self` for `emit_atom`.
                            let delimiter = self.var_delimiter(close_delim, 0, style, close_origin);
                            self.emit_atom(&mut out, &mut prev, Some(CL_CLOSE), delimiter, sp_style);
                        }
                    }
                    i += 1;
                    continue;
                }
                // buffered \left...\right content (middles etc. keep going in)
                if let Some((_, _, buf)) = lr_open.as_mut() {
                    buf.push(n.clone());
                    i += 1;
                    continue;
                }
                // pass 1 sizes a `\middle` in the list's starting style
                let conv_style = if is_middle(n) { start } else { style };
                // luatex mlist.c `reset_attributes(p, node_attr(q))`: what an
                // atom converts to carries the atom's attribute list
                let saved_attr = std::mem::replace(&mut self.eqtb.cur_attr, n.attr());
                let nodes = self.convert_atom(n, conv_style, math_text_chars[i]);
                if is_middle(n) {
                    style = start;
                }
                // inter-atom mu glue comes first (tex.web second pass)
                let after_penalty = matches!(out.last(), Some(Node::Penalty(_, _)));
                self.insert_spacing(&mut out, prev, Some(cls), sp_style);
                // luatex mlist.c: \prebinoppenalty / \prerelpenalty precede a
                // Bin / Rel noad that is not the first one
                if self.engine_kind == crate::engine::EngineKind::LuaTeX
                    && self.math_penalties.get()
                    && prev.is_some()
                    && !after_penalty
                {
                    let pre = match cls {
                        CL_BIN => self.eqtb.int_params[IntParam::PreBinOpPenalty.idx() as usize],
                        CL_REL => self.eqtb.int_params[IntParam::PreRelPenalty.idx() as usize],
                        _ => 10000,
                    };
                    if pre < 10000 {
                        out.push(Node::Penalty(pre, self.eqtb.cur_attr));
                    }
                }
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
                    // tex.web §767: only pen<inf_penalty is inserted
                    if let Some(pv) = pval.filter(|&pv| pv < 10000) {
                        let suppress = match list.get(i + 1) {
                            None => true,
                            Some(Node::Penalty(_, _)) => true,
                            Some(nn) => self.atom_class(nn) == Some(CL_REL),
                        };
                        if !suppress {
                            out.push(Node::Penalty(pv, self.eqtb.cur_attr));
                        }
                    }
                }
                self.eqtb.cur_attr = saved_attr;
                // tex.web §760: a \middle is spaced as a close noad before it
                // and as an open noad after it (`r_type:=open_noad`)
                prev = Some(if is_middle(n) { CL_OPEN } else { cls });
                i += 1;
                continue;
            }
            // A \left...\right body keeps its non-atoms raw: the recursive
            // conversion applies \nonscript and mu units in the body's style.
            if let Some((_, _, buf)) = lr_open.as_mut() {
                buf.push(n.clone());
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
                            Node::Glue(_, _)
                                | Node::MuGlue(_, _)
                                | Node::Kern(_, _)
                                | Node::ExplicitKern(_, _)
                                | Node::MathKern(_, _, _)
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
                Node::MuGlue(g, a) => {
                    let mu = self.mu_unit(style) as i64;
                    let conv = |v: i32| (v as i64 * mu / 65536) as i32;
                    Node::Glue(Glue::spec(
                        conv(g.width),
                        conv(g.stretch),
                        g.stretch_order,
                        conv(g.shrink),
                        g.shrink_order,
                    ), *a)
                }
                Node::MathKern(k, 0, a) => {
                    Node::Kern((*k as i64 * self.mu_unit(style) as i64 / 65536) as i32, *a)
                }
                _ => n.clone(),
            };
            out.push(converted);
            i += 1;
        }
        if let Some((lopen, origin, buf)) = lr_open {
            let body = self.mlist_to_hlist_pen(&buf, style, false);
            let (_, bh, bd) = hlist_dims(&body, &self.eqtb);
            let needed = self.lr_delimiter_size(bh, bd, style);
            out.extend(self.var_delimiter(lopen, needed, style, &origin));
            out.extend(body);
        }
        // the right delimiter of a `\left...\right` body is a close noad:
        // only Punct-Close spacing (a conditional thin space) is non-zero
        if lr_body {
            self.insert_spacing(&mut out, prev, Some(CL_CLOSE), sp_style);
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
        let factor = self.eqtb.int_params[IntParam::DelimiterFactor.idx() as usize] as i64;
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
        let (src, subtype) = match kind {
            1 | 2 => (GlueParam::ThinMuSkip, crate::boxes::glue_subtype::THIN_MU_SKIP),
            3 => (GlueParam::MedMuSkip, crate::boxes::glue_subtype::MED_MU_SKIP),
            _ => (GlueParam::ThickMuSkip, crate::boxes::glue_subtype::THICK_MU_SKIP),
        };
        let p = &self.eqtb.glue_params[src.idx() as usize];
        // tex.web §766: `subtype(z):=x+1`, the spacing parameter's symbol
        let g = Glue {
            subtype,
            ..Glue::spec(
                conv(p.width),
                conv(p.stretch),
                p.stretch_order,
                conv(p.shrink),
                p.shrink_order,
            )
        };
        out.push(Node::Glue(g, self.eqtb.cur_attr));
    }

    fn run_math_token(&mut self, t: Token) {
        if t.is_cs() {
            match self.eqtb.resolve(t.cs_id()).cloned() {
                // \mathchardef'd control sequences never reach main_dispatch
                // (control.rs has no arm for the equiv), so materialize them here
                Some(Equiv::MathCharDef(v)) if self.mode.is_m() && self.engine_kind == crate::engine::EngineKind::LuaTeX => {
                    self.math_given_command(i32::from(v), false, t.cs_id());
                    return;
                }
                Some(Equiv::UMathCharDef(v)) if self.mode.is_m() && self.engine_kind == crate::engine::EngineKind::XeTeX => {
                    let source = self.current_token_source_mark();
                    self.xe_set_math_char_at(i64::from(v as u32), v as u32, source);
                    return;
                }
                Some(Equiv::UMathCharDef(v)) if self.mode.is_m() => {
                    self.math_given_command(v, true, t.cs_id());
                    return;
                }
                Some(Equiv::MathCharDef(v)) if self.mode.is_m() => {
                    self.append_mathchar(v);
                    return;
                }
                _ => {}
            }
        }
        self.dispatch(t);
    }

    pub(crate) fn convert_atom(&mut self, n: &Node, style: GStyle, math_text_char: bool) -> NodeList {
        match n {
            Node::MathChar {
                fam,
                c,
                class,
                origin, .. } => {
                if *fam == 255 {
                    vec![]
                } else if let Some(nodes) = self.xe_convert_math_char(*fam, *c, *class, style, math_text_char, origin) {
                    nodes
                } else if *class == CL_OP {
                    let (b, _) = self.op_char_box(*fam, *c, style, false, origin);
                    vec![b]
                } else {
                    let fid = self.eqtb.style_fonts[font_size(style)][*fam as usize];
                    if self.is_native_font(fid) {
                        if let Some(ch) = char::from_u32(*c) {
                            if let Ok(nodes) = self.shape_native_slice(fid, &ch.to_string()) {
                                return nodes;
                            }
                        }
                    }
                    let byte = *c as u8;
                    if !self.math_font_has_character_or_warn(fid, byte, origin) {
                        return Vec::new();
                    }
                    let mut out = vec![Node::Char { c: byte, font: fid, attr: self.eqtb.cur_attr }];
                    if let Some(f) = self.eqtb.fonts.get(fid as usize) {
                        let ic = f.char_italic(byte);
                        if ic != 0 && !(math_text_char && f.space() != 0) {
                            out.push(Node::Kern(ic, self.eqtb.cur_attr));
                        }
                    }
                    out
                }
            }
            Node::Scripts { nucleus, sup, sub, .. } => {
                // TeX make_math_accent: scripts on a single-character accent
                // attach to the character, not the taller accent box. Ordinary
                // groups containing a lone accent preserve that noad identity.
                if self.engine_kind == crate::engine::EngineKind::LuaTeX && (sup.is_some() || sub.is_some()) {
                    if let Some((spec, body, _)) = accent_noad_of(nucleus) {
                        let (b, consumed) =
                            self.make_math_accent_lua(&spec, body, sup.as_deref(), sub.as_deref(), style, nucleus.first().map_or(crate::boxes::Attr::NONE, Node::attr));
                        if consumed {
                            return vec![b];
                        }
                        return self.make_scripts(&[b], sup.as_deref(), sub.as_deref(), style);
                    }
                }
                if sup.is_some() || sub.is_some() {
                    if let Some((spec, body, origin)) = accent_noad_of(nucleus) {
                        let accent = spec.top.unwrap_or((0, 0));
                        if matches!(body, [Node::MathChar { fam, .. }] if *fam != 255) {
                            return self.make_accent(
                                accent,
                                spec.subtype,
                                body,
                                style,
                                sup.as_deref(),
                                sub.as_deref(),
                                origin,
                            );
                        }
                    }
                }
                self.make_scripts(nucleus, sup.as_deref(), sub.as_deref(), style)
            }
            Node::OpLimits { op, above, below, .. } => {
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
                        ..
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
                middle,
                options,
                fam,
                origin,
                attr } => {
                if self.engine_kind == crate::engine::EngineKind::LuaTeX {
                    vec![self.make_fraction_lua(
                        num,
                        den,
                        *thickness,
                        noad_option::has(*options, noad_option::FRAC_LEFT_DELIM).then_some(left),
                        noad_option::has(*options, noad_option::FRAC_RIGHT_DELIM).then_some(right),
                        (!middle.is_null()).then_some(middle),
                        *options & !noad_option::FRAC_DELIMS,
                        if *fam == crate::boxes::NO_FAM { -1 } else { i32::from(*fam) },
                        style,
                        *attr,
                    )]
                } else {
                    let opt = |d: &Delim| (!d.is_null()).then_some(*d);
                    self.make_fraction(num, den, *thickness, (opt(left), opt(right)), style, origin)
                }
            }
            Node::Radical {
                body,
                delim,
                subtype,
                width,
                options,
                degree,
                origin,
                attr,
            } => {
                if self.engine_kind == crate::engine::EngineKind::LuaTeX {
                    vec![self.make_radical_lua(body, delim, *subtype, *width, *options, degree.as_deref(), style, *attr)]
                } else {
                    self.make_radical(body, *delim, style, origin)
                }
            }
            Node::Accent { spec, body, origin, attr } => {
                if self.engine_kind == crate::engine::EngineKind::LuaTeX {
                    vec![self.make_math_accent_lua(spec, body, None, None, style, *attr).0]
                } else {
                    let accent = spec.top.unwrap_or((0, 0));
                    self.make_accent(accent, spec.subtype, body, style, None, None, origin)
                }
            }
            Node::DelimBox {
                small,
                large,
                size,
                origin,
                ..
            } => {
                // plain delimiter atom (size 2); 3 is e-TeX \middle delimiter; 0/1 only reach here as strays
                let delim = delim_of(*small, *large);
                let target_size = if *size == 3 {
                    if self.middle_delimiter_size == MIDDLE_UNSIZED {
                        return Vec::new();
                    }
                    self.middle_delimiter_size
                } else {
                    0
                };
                let mut out = self.var_delimiter(delim, target_size, style, origin);
                if out.is_empty() {
                    out.push(Node::Kern(0, self.eqtb.cur_attr));
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
            Node::Overline { packed, .. } => vec![(**packed).clone()],
            other => vec![other.clone()],
        }
    }

    // ---------- make_scripts (tex.web §745-746) ----------

    /// box a single math-char operator (tex.web `make_op`): display-style
    /// "next larger" chain, width including the italic correction (removed
    /// again when a subscript tucks under), ink centered on the math axis.
    /// Returns the box and the italic correction.
    fn op_char_box(
        &mut self,
        fam: u8,
        c: u32,
        style: GStyle,
        sub_present: bool,
        origin: &MathDiagnosticOrigin,
    ) -> (Node, i32) {
        let fid = self.eqtb.style_fonts[font_size(style)][fam as usize];
        if self.engine_kind == crate::engine::EngineKind::XeTeX {
            self.xe_note_fetch(fid);
            if self.xe_ot(fid).is_some() {
                return self.xe_op_char_box(fid, c, style, sub_present, false);
            }
            if c > 255 {
                self.xe_missing_math_char(fid, c, origin);
                return (Node::Kern(0, self.eqtb.cur_attr), 0);
            }
        }
        let c = c as u8;
        if !self.math_font_has_character_or_warn(fid, c, origin) {
            return (Node::Kern(0, self.eqtb.cur_attr), 0);
        }
        let Some(f) = self.eqtb.fonts.get(fid as usize).cloned() else {
            return (Node::Kern(0, self.eqtb.cur_attr), 0);
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
        let delta = f.char_italic(c);
        let mut b = hpack(vec![Node::Char { c, font: fid, attr: self.eqtb.cur_attr }], None, HBOX, &self.eqtb).node;
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
    fn build_op_box(&mut self, op: &[Node], style: GStyle) -> (Node, i32) {
        match op {
            [Node::MathChar {
                fam,
                c,
                class,
                origin, .. }] if *class == CL_OP => {
                if *fam == 255 {
                    (hpack(Vec::new(), None, HBOX, &self.eqtb).node, 0)
                } else {
                    self.op_char_box(*fam, *c, style, false, origin)
                }
            }
            [Node::DelimBox {
                small,
                large,
                size: 2,
                origin,
                ..
            }] => {
                let delim = delim_of(*small, *large);
                let mut out = self.var_delimiter(delim, 0, style, origin);
                if out.len() == 1 {
                    (out.pop().unwrap(), 0)
                } else {
                    (hpack(out, None, HBOX, &self.eqtb).node, 0)
                }
            }
            _ => {
                // box nucleus (`\mathop{...} group`): tex.web make_op only
                // fetches a CHARACTER nucleus and axis-centers that one; a
                // sub_mlist/sub_box nucleus becomes `y := clean_box(nucleus)`
                // (a lone vcenter box keeps its negative depth)
                (self.clean_math_box(op, style), 0)
            }
        }
    }

    /// tex.web `clean_box` (§720): an already-clean single box is reused;
    /// otherwise the hlist is packed at natural width. "Simplify a trivial
    /// box" then unlinks a lone character's italic-correction kern AFTER
    /// packing, so the box keeps the corrected width.
    pub(crate) fn clean_math_box(&mut self, list: &[Node], style: GStyle) -> Node {
        let mut nodes = self.mlist_to_hlist_pen(list, style, false);
        let mut x = if matches!(nodes.as_slice(), [Node::Box { shift: 0, .. }]) {
            nodes.pop().expect("single clean math box")
        } else {
            hpack(nodes, None, HBOX, &self.eqtb).node
        };
        if let Node::Box { list, .. } = &mut x {
            if matches!(list.as_slice(), [Node::Char { .. }, Node::Kern(_, _)]) {
                list.pop();
            }
        }
        x
    }

    /// What a lone math character nucleus converts to carries that
    /// character's attribute list (luatex gives the glyph its nucleus'
    /// `node_attr`), not the scripted noad's.
    fn nucleus_attr(&self, nucleus: &[Node]) -> crate::boxes::Attr {
        match nucleus {
            [n @ (Node::MathChar { .. } | Node::DelimBox { .. })] => n.attr(),
            _ => self.eqtb.cur_attr,
        }
    }

    pub(crate) fn make_scripts(
        &mut self,
        nucleus: &[Node],
        sup: Option<&[Node]>,
        sub: Option<&[Node]>,
        style: GStyle,
    ) -> NodeList {
        let attr = self.nucleus_attr(nucleus);
        let saved = std::mem::replace(&mut self.eqtb.cur_attr, attr);
        let out = self.make_scripts_inner(nucleus, sup, sub, style);
        self.eqtb.cur_attr = saved;
        out
    }

    fn make_scripts_inner(
        &mut self,
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
        // XeTeX: a native-font character nucleus is a bare glyph node
        let mut xe_list: Option<NodeList> = None;
        let mut xe_glyph: Option<(FontId, u16)> = None;
        match nucleus {
            // A character nucleus keeps its italic correction as a trailing
            // kern unless a subscript is present; then it offsets the sup.
            [Node::MathChar {
                fam,
                c,
                class,
                origin, .. }] if *class != CL_OP => {
                if *fam == 255 {
                    // fam255 prefix marker: the nucleus is the REST of the
                    // list (a group). tex.web treats a brace-group nucleus as
                    // an Ord atom with delta = 0 — never take the first
                    // character's italic correction.
                    nuc = hpack(Vec::new(), None, HBOX, &self.eqtb).node;
                } else {
                    let fid = self.eqtb.style_fonts[font_size(style)][*fam as usize];
                    self.xe_note_fetch(fid);
                    if self.engine_kind == crate::engine::EngineKind::XeTeX && self.xe_ot(fid).is_some() {
                        // xetex.web "Create a character node": glyph node and
                        // italic correction; `p` stays the glyph for math kerning
                        let (list, d) = self.xe_native_char(fid, *c, false, sub.is_some());
                        delta = d;
                        xe_glyph = list.first().and_then(Self::xe_glyph_of);
                        xe_list = Some(list);
                        nuc = Node::Empty;
                    } else if self.engine_kind == crate::engine::EngineKind::XeTeX && *c > 255 {
                        self.xe_missing_math_char(fid, *c, origin);
                        nuc = hpack(Vec::new(), None, HBOX, &self.eqtb).node;
                    } else {
                        let byte = *c as u8;
                    if self.math_font_has_character_or_warn(fid, byte, origin) {
                        let f = self.eqtb.fonts[fid as usize].clone();
                        let ic = f.char_italic(byte);
                        let mut core: NodeList = vec![Node::Char { c: byte, font: fid, attr: self.eqtb.cur_attr }];
                        if sub.is_none() && ic != 0 {
                            core.push(Node::Kern(ic, self.eqtb.cur_attr));
                            delta = 0;
                        } else {
                            delta = ic;
                        }
                        nuc = hpack(core, None, HBOX, &self.eqtb).node;
                    } else {
                        nuc = hpack(Vec::new(), None, HBOX, &self.eqtb).node;
                    }
                }
            }
        }
            // A standalone \delimiter is an ordinary math-character noad:
            // scripts use the character shifts, not sub-box drop parameters.
            [Node::DelimBox {
                small: (fam, c),
                size: 2,
                origin,
                ..
            }] => {
                let cb = *c as u8;
                let fid = self.eqtb.style_fonts[font_size(style)][*fam as usize];
                if self.math_font_has_character_or_warn(fid, cb, origin) {
                    let f = self.eqtb.fonts[fid as usize].clone();
                    let ic = f.char_italic(cb);
                    let mut core = vec![Node::Char { c: cb, font: fid, attr: self.eqtb.cur_attr }];
                    if sub.is_none() && ic != 0 {
                        core.push(Node::Kern(ic, self.eqtb.cur_attr));
                    } else {
                        delta = ic;
                    }
                    nuc = hpack(core, None, HBOX, &self.eqtb).node;
                } else {
                    nuc = hpack(Vec::new(), None, HBOX, &self.eqtb).node;
                }
            }
            // non-limits big operator: axis-centered box, italic handled above
            [Node::MathChar {
                fam,
                c,
                class,
                origin, .. }] if *class == CL_OP => {
                if *fam == 255 {
                    nuc = hpack(Vec::new(), None, HBOX, &self.eqtb).node;
                } else {
                    let (b, d) = self.op_char_box(*fam, *c, style, sub.is_some(), origin);
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
        let mut out: NodeList = match xe_list {
            Some(list) => list,
            None => vec![nuc],
        };
        // xetex.web: `is_new_mathfont(cur_f)` for the current font of the last fetch
        let xe_ot = self.engine_kind == crate::engine::EngineKind::XeTeX && self.xe_cur_f_is_math();
        let xe_f0 = self.xe_math.cur_f.get();
        let mut xe_sub_kern = 0i32;
        let sup1 = self.fparam(style, 2, 13);
        let sup2 = self.fparam(style, 2, 14);
        let sup3 = self.fparam(style, 2, 15);
        let sub1 = self.fparam(style, 2, 16);
        let sub2 = self.fparam(style, 2, 17);
        let mut sup_box: Option<Node> = None;
        let mut sub_box: Option<Node> = None;
        // tex.web §757-§758: each script is a clean_box whose WIDTH grows by
        // \scriptspace (no kern is appended: a running rule in a vcenter
        // script takes the widened width)
        let widen = |mut b: Node| {
            if let Node::Box { w, .. } = &mut b {
                *w += ss;
            }
            b
        };
        if let Some(s) = sup {
            let b = widen(self.clean_math_box(s, sup_style(style)));
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
            clr = if xe_ot {
                bd + self.xe_const(xe_f0, crate::math_xetex::k::SUPERSCRIPT_BOTTOM_MIN)
            } else {
                bd + x / 4
            };
            if shift_up < clr {
                shift_up = clr;
            }
            sup_box = Some(b);
        }
        if let Some(s) = sub {
            let b = widen(self.clean_math_box(s, sub_style(style)));
            let (_, bh, _) = box_dims(&b);
            if sup_box.is_none() {
                if shift_down < sub1 {
                    shift_down = sub1;
                }
                let clr = if xe_ot {
                    bh - self.xe_const(xe_f0, crate::math_xetex::k::SUBSCRIPT_TOP_MAX)
                } else {
                    bh - (x * 4) / 5
                };
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
            let mut clr = if xe_ot {
                self.xe_const(xe_f0, crate::math_xetex::k::SUB_SUPERSCRIPT_GAP_MIN)
            } else {
                rt * 4
            } - ((shift_up - sup_d) - (sub_h - shift_down));
            if clr > 0 {
                shift_down += clr;
                clr = if xe_ot {
                    self.xe_const(xe_f0, crate::math_xetex::k::SUPERSCRIPT_BOTTOM_MAX_WITH_SUBSCRIPT)
                } else {
                    (x * 4) / 5
                } - (shift_up - sup_d);
                if clr > 0 {
                    shift_up += clr;
                    shift_down -= clr;
                }
            }
        }

        // xetex.web "Attach subscript/superscript OpenType math kerning"
        if xe_ot {
            if let Some((pf, pg)) = xe_glyph {
                if let Some(sb) = sub {
                    xe_sub_kern = self.xe_script_kern(pf, pg, sb, style, true, shift_down);
                } else if let Some(sp) = sup {
                    let k = self.xe_script_kern(pf, pg, sp, style, false, shift_up);
                    if k != 0 {
                        out.push(Node::Kern(k, self.eqtb.cur_attr));
                    }
                }
                if xe_sub_kern != 0 {
                    out.push(Node::Kern(xe_sub_kern, self.eqtb.cur_attr));
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
                    *shift = delta - xe_sub_kern; // superscript offset (tex.web §746)
                }
                let mut vlist: NodeList = vec![sbs];
                vlist.push(Node::Kern(k, self.eqtb.cur_attr));
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
        &mut self,
        op: &[Node],
        above: Option<&[Node]>,
        below: Option<&[Node]>,
        style: GStyle,
        force: bool,
    ) -> NodeList {
        let attr = self.nucleus_attr(op);
        let saved = std::mem::replace(&mut self.eqtb.cur_attr, attr);
        let out = self.make_op_limits_inner(op, above, below, style, force);
        self.eqtb.cur_attr = saved;
        out
    }

    fn make_op_limits_inner(
        &mut self,
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
        // tex.web §14712 `y := clean_box(nucleus(q), cur_style)`: when the
        // operator nucleus has been axis-centered (shift != 0), clean_box
        // (§14195) wraps it in an hpack so that its shift is absorbed into
        // height/depth and shift becomes 0 before placing it into the vbox.
        let op_box = if let Node::Box { shift, .. } = &op_box {
            if *shift != 0 {
                hpack(vec![op_box], None, HBOX, &self.eqtb).node
            } else {
                op_box
            }
        } else {
            op_box
        };
        let sp1 = self.fparam(style, 3, 9);
        let sp2 = self.fparam(style, 3, 10);
        let sp3 = self.fparam(style, 3, 11);
        let sp4 = self.fparam(style, 3, 12);
        let sp5 = self.fparam(style, 3, 13);
        // height(v):=height(y); depth(v):=depth(y) — y is unshifted here, and
        // a reused vcenter box may have negative depth
        let (_, oh, od) = box_dims(&op_box);
        // tex.web §749: x and z are clean_box results
        let sup_box = above.map(|s| self.clean_math_box(s, sup_style(style)));
        let sub_box = below.map(|s| self.clean_math_box(s, sub_style(style)));
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
            vlist.push(Node::Kern(sp5, self.eqtb.cur_attr));
            let mut bc = self.center_to_w(b.clone(), w);
            if let Node::Box { shift, .. } = &mut bc {
                *shift = half_i(delta); // limits skewed by half the italic
            }
            vlist.push(bc);
            vlist.push(Node::Kern(su, self.eqtb.cur_attr));
        }
        let op_centered = self.center_to_w(op_box, w);
        vlist.push(op_centered);
        if let Some(b) = &sub_box {
            let (_, bh, _) = box_dims(b);
            sd = sp4 - bh;
            if sd < sp2 {
                sd = sp2;
            }
            vlist.push(Node::Kern(sd, self.eqtb.cur_attr));
            let mut bc = self.center_to_w(b.clone(), w);
            if let Node::Box { shift, .. } = &mut bc {
                *shift = -half_i(delta);
            }
            vlist.push(bc);
            vlist.push(Node::Kern(sp5, self.eqtb.cur_attr));
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
        if self.engine_kind == crate::engine::EngineKind::XeTeX {
            return self.xe_rebox(b, w);
        }
        if self.box_w(&b) == w {
            return b;
        }
        let ss = || {
            Node::Glue(Glue::spec(
                0,
                ONE,
                crate::boxes::GLUE_FIL,
                ONE,
                crate::boxes::GLUE_FIL,
            ), self.eqtb.cur_attr)
        };
        hpack(vec![ss(), b, ss()], Some(w), HBOX, &self.eqtb).node
    }
    fn box_w(&self, n: &Node) -> i32 {
        match n {
            Node::Box { w, .. } => *w,
            Node::Char { c, font, .. } => self
                .eqtb
                .fonts
                .get(*font as usize)
                .map(|f| f.char_width(*c))
                .unwrap_or(0),
            Node::Rule { width, .. } => *width,
            Node::Kern(k, _) => *k,
            Node::Glue(g, _) => g.width,
            _ => 0,
        }
    }

    // ---------- make_fraction (tex.web §741-742) ----------

    fn make_fraction(
        &mut self,
        num: &[Node],
        den: &[Node],
        thickness: i32,
        delimiters: (Option<Delim>, Option<Delim>),
        style: GStyle,
        origin: &MathDiagnosticOrigin,
    ) -> NodeList {
        let (left, right) = delimiters;
        let r = if thickness == DEFAULT_CODE {
            self.default_rule_thickness(style)
        } else {
            thickness
        };
        // tex.web §743: x and z are clean_box results, so a lone character's
        // italic correction is dropped
        let num_box = self.clean_math_box(num, num_style(style));
        let den_box = self.clean_math_box(den, den_style(style));
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
        // xetex.web: `is_new_mathfont(cur_f)` after the numerator and denominator
        let xe_ot = self.engine_kind == crate::engine::EngineKind::XeTeX && self.xe_cur_f_is_math();
        let xe_f0 = self.xe_math.cur_f.get();
        let xe_mode = self.engine_kind == crate::engine::EngineKind::XeTeX;
        let vlist: NodeList;
        if r == 0 {
            // \atop: symmetric minimum clearance around the numerator/denominator
            // (luatex stack_num_up, stack_denom_down, stack_vgap)
            if let Some(v) = self.umath_param(crate::luatex::MATH_PARAM_STACK_NUM_UP, style) {
                su = v;
            }
            if let Some(v) = self.umath_param(crate::luatex::MATH_PARAM_STACK_DENOM_DOWN, style) {
                sd = v;
            }
            let rt = self.default_rule_thickness(style);
            let clr = if xe_ot {
                self.xe_const(
                    xe_f0,
                    if display {
                        crate::math_xetex::k::STACK_DISPLAY_STYLE_GAP_MIN
                    } else {
                        crate::math_xetex::k::STACK_GAP_MIN
                    },
                )
            } else {
                self.umath_param(crate::luatex::MATH_PARAM_STACK_VGAP, style)
                    .unwrap_or(if display { rt * 7 } else { rt * 3 })
            };
            let delta = half_i(clr - ((su - nd) - (dh - sd)));
            if delta > 0 {
                su += delta;
                sd += delta;
            }
            vlist = vec![num_c, Node::Kern((su - nd) - (dh - sd), self.eqtb.cur_attr), den_c];
        } else {
            // tex.web §746: the clearance is measured from the axis with the
            // fraction's OWN rule thickness (3x in display style)
            let dr = half_i(r);
            let (clr_n, clr_d) = if xe_ot {
                use crate::math_xetex::k;
                (
                    self.xe_const(
                        xe_f0,
                        if display { k::FRACTION_NUM_DISPLAY_STYLE_GAP_MIN } else { k::FRACTION_NUMERATOR_GAP_MIN },
                    ),
                    self.xe_const(
                        xe_f0,
                        if display { k::FRACTION_DENOM_DISPLAY_STYLE_GAP_MIN } else { k::FRACTION_DENOMINATOR_GAP_MIN },
                    ),
                )
            } else {
                let clr = if display { 3 * r } else { r };
                (clr, clr)
            };
            let d1 = clr_n - ((su - nd) - (axis + dr));
            if d1 > 0 {
                su += d1;
            }
            let d2 = clr_d - ((axis - dr) - (dh - sd));
            if d2 > 0 {
                sd += d2;
            }
            vlist = vec![
                num_c,
                Node::Kern((su - nd) - (axis + dr), self.eqtb.cur_attr),
                Node::Rule {
                    // xetex.web fraction_rule: a running-width rule
                    width: if xe_mode { crate::build::RULE_FILL } else { w },
                    height: r,
                    depth: 0, subtype: crate::boxes::RULE_NORMAL, index: 0, attr: self.eqtb.cur_attr,
                },
                Node::Kern((axis - dr) - (dh - sd), self.eqtb.cur_attr),
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
        let dd_size = match self.umath_param(crate::luatex::MATH_PARAM_FRACTION_DEL_SIZE, style) {
            Some(v) => v,
            None if display => self.fparam(style, 2, 20),
            None => self.fparam(style, 2, 21),
        };
        let mut out: NodeList = Vec::new();
        if let Some(l) = left {
            out.extend(self.var_delimiter(l, dd_size, style, origin));
        } else {
            out.push(self.null_delimiter_box(style));
        }
        out.push(packed);
        if let Some(rr) = right {
            out.extend(self.var_delimiter(rr, dd_size, style, origin));
        } else {
            out.push(self.null_delimiter_box(style));
        }
        if xe_mode {
            // new_hlist(q) := hpack(x, natural)
            return vec![hpack(out, None, HBOX, &self.eqtb).node];
        }
        out
    }

    /// tex.web make_radical (§752-753): the body is boxed in the cramped
    /// style; the surd is chosen for h+d+clr+rt; excess surd depth widens the
    /// clearance by half; the surd baseline drops to -(h+clr) and the
    /// overbar box = [kern(surd_h), rule(surd_h), kern(clr), body].
    fn make_radical(
        &mut self,
        body: &[Node],
        delim: Delim,
        style: GStyle,
        origin: &MathDiagnosticOrigin,
    ) -> NodeList {
        let body_nodes = self.mlist_to_hlist_pen(body, style | 1, self.math_penalties.get());
        let x = hpack(body_nodes, None, HBOX, &self.eqtb).node;
        let (xw, xh, xd) = box_dims(&x);
        // xetex.web make_radical: `f` is the small family font at this size
        let xe_f = self.xe_fam_fnt(style, delim.small_fam);
        let xe_ot = self.engine_kind == crate::engine::EngineKind::XeTeX && self.xe_is_new_mathfont(xe_f);
        let rt = if xe_ot {
            self.xe_const(xe_f, crate::math_xetex::k::RADICAL_RULE_THICKNESS)
        } else {
            self.default_rule_thickness(style)
        };
        let x_h = self.math_x_height(style);
        let mut clr = if xe_ot {
            self.xe_const(
                xe_f,
                if style < 2 {
                    crate::math_xetex::k::RADICAL_DISPLAY_STYLE_VERTICAL_GAP
                } else {
                    crate::math_xetex::k::RADICAL_VERTICAL_GAP
                },
            )
        } else if style < 2 {
            rt + (x_h / 4).abs()
        } else {
            rt + rt / 4
        };
        let target_size = xh + xd + clr + rt;
        let mut d_nodes = self.var_delimiter(delim, target_size, style, origin);
        let mut d_box = if d_nodes.len() == 1 {
            d_nodes.pop().unwrap()
        } else {
            hpack(d_nodes, None, HBOX, &self.eqtb).node
        };
        if xe_ot {
            // the radical sign is drawn as a rule-thin box hanging down
            if let Node::Box { h, d, .. } = &mut d_box {
                *d = *h + *d - rt;
                *h = rt;
            }
        }
        let (_, dh, dd) = box_dims(&d_box);
        let delta = dd - (xh + xd + clr);
        if delta > 0 {
            clr += half_i(delta);
        }
        // overbar(b, k=clr, t=surd height): [kern(t), rule(t), kern(clr), body]
        let xe_mode = self.engine_kind == crate::engine::EngineKind::XeTeX;
        let vlist = vec![
            Node::Kern(dh, self.eqtb.cur_attr),
            Node::Rule {
                width: if xe_mode { crate::build::RULE_FILL } else { xw },
                height: dh,
                depth: 0, subtype: crate::boxes::RULE_NORMAL, index: 0, attr: self.eqtb.cur_attr,
            },
            Node::Kern(clr, self.eqtb.cur_attr),
            x,
        ];
        let v = vpack(vlist, None, VBOX, &self.eqtb).node;
        if let Node::Box { shift, .. } = &mut d_box {
            *shift = -(xh + clr);
        }
        let mut out = NodeList::new();
        out.push(d_box);
        out.push(v);
        if xe_mode {
            // info(nucleus(q)) := hpack(y, natural)
            return vec![hpack(out, None, HBOX, &self.eqtb).node];
        }
        out
    }

    fn make_accent(
        &mut self,
        accent: (u8, u32),
        xe_subtype: u8,
        body: &[Node],
        style: GStyle,
        sup: Option<&[Node]>,
        sub: Option<&[Node]>,
        origin: &MathDiagnosticOrigin,
    ) -> NodeList {
        let (afam, ac32) = accent;
        let has_scripts = sup.is_some() || sub.is_some();
        let afid = self.eqtb.style_fonts[font_size(style)][afam as usize];
        if self.engine_kind == crate::engine::EngineKind::XeTeX {
            // xetex.web make_math_accent: `fetch(accent_chr(q))`
            self.xe_note_fetch(afid);
            if self.xe_ot(afid).is_some() {
                return self.xe_make_accent(afid, ac32, xe_subtype, body, style, sup, sub);
            }
            if ac32 > 255 {
                self.xe_missing_math_char(afid, ac32, origin);
                if has_scripts {
                    return self.make_scripts(body, sup, sub, style);
                }
                let body_nodes = self.mlist_to_hlist_pen(body, style | 1, self.math_penalties.get());
                return vec![hpack(body_nodes, None, HBOX, &self.eqtb).node];
            }
        }
        let mut ac = ac32 as u8;
        if !self.math_font_has_character_or_warn(afid, ac, origin) {
            // TeX keeps the nucleus (and any scripts) when the accent font is
            // unavailable.
            if has_scripts {
                return self.make_scripts(body, sup, sub, style);
            }
            let body_nodes = self.mlist_to_hlist_pen(body, style | 1, self.math_penalties.get());
            return vec![hpack(body_nodes, None, HBOX, &self.eqtb).node];
        }
        let af = self.eqtb.fonts[afid as usize].clone();
        // skew: kern from the nucleus font's character to its \skewchar
        let s = match body {
            [Node::MathChar { fam, c, .. }] => {
                if let Some((fid, f)) = self.fam_font(style | 1, *fam) {
                    let sk = self.skew_char_of(fid, &f);
                    if sk >= 0 && sk <= 255 {
                        self.char_kern(fid, &f, *c as u8, sk as u8)
                    } else {
                        0
                    }
                } else {
                    0
                }
            }
            [Node::Char { c, font, .. }] => {
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
        let body_box = self.clean_math_box(body, style | 1);
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
            if next == ac || !af.char_present(next) {
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
            vec![Node::Char { c: ac, font: afid, attr: self.eqtb.cur_attr }],
            None,
            HBOX,
            &self.eqtb,
        )
        .node;
        if let Node::Box { w, shift, .. } = &mut acc_box {
            *w = 0; // accent width does not affect the box width
            // tex.web §738: y = char_box(f,c), whose width includes the
            // accent's italic correction
            *shift = s + half_i(bw - (aw + af.char_italic(ac)));
        }
        let mut v = vpack(
            vec![acc_box, Node::Kern(-delta, self.eqtb.cur_attr), x_box],
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
                list.insert(0, Node::Kern(bh - vh, self.eqtb.cur_attr));
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
        let mut b = hpack(vec![Node::Char { c, font: fid, attr: self.eqtb.cur_attr }], None, HBOX, &self.eqtb).node;
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
    fn var_delimiter(
        &mut self,
        d: Delim,
        v: i32,
        style: GStyle,
        origin: &MathDiagnosticOrigin,
    ) -> NodeList {
        if d.is_null() {
            return vec![self.null_delimiter_box(style)];
        }
        let xetex = self.engine_kind == crate::engine::EngineKind::XeTeX;
        // tex.web delimiters are 8-bit; XeTeX keeps the full character code
        let byte_code = |c: u32| if xetex { c } else { u32::from(c as u8) };
        let (sf, sc, lf, lc) = (d.small_fam, byte_code(d.small_char), d.large_fam, byte_code(d.large_char));
        let cur_size = font_size(style);
        let mut best: Option<VarCand> = None;
        let mut found: Option<VarCand> = None;
        let mut first_missing: Option<(FontId, u8)> = None;
        let mut w = 0i32;
        'parts: for (fam, first) in [(sf, sc), (lf, lc)] {
            if fam == 0 && first == 0 {
                continue;
            }
            for sz in (0..=cur_size).rev() {
                // sz IS the style_fonts size index (tex.web walks `fam +
                // cur_size` down to `fam`): fam_font() would push it
                // through font_size() and map script sizes back onto the
                // text/script fonts (style 2/4), not the size sz font.
                let fid = self.eqtb.style_fonts[sz][fam as usize];
                if xetex {
                    if let Some(ot) = self.xe_ot(fid) {
                        // xetex.web: variants of the glyph, then its assembly
                        let x = ot.glyph_of_char(first);
                        w = 0;
                        best = Some(VarCand::Ot { fid, gid: x, parts: None });
                        let mut n = 0usize;
                        loop {
                            let (y, u) = ot.variant(x, n, false);
                            if u > w {
                                w = u;
                                best = Some(VarCand::Ot { fid, gid: y, parts: None });
                                if u >= v {
                                    found = best.clone();
                                    break 'parts;
                                }
                            }
                            n += 1;
                            if u < 0 || n > 4096 {
                                break;
                            }
                        }
                        if let Some(parts) = ot.assembly(x, false) {
                            found = Some(VarCand::Ot { fid, gid: x, parts: Some(parts) });
                            break 'parts;
                        }
                        continue;
                    }
                }
                let Some((_, f)) = self.fam_font_idx(sz, fam) else {
                    first_missing.get_or_insert((fid, first as u8));
                    continue;
                };
                if first > 255 {
                    first_missing.get_or_insert((fid, first as u8));
                    continue;
                }
                let first = first as u8;
                let mut c = first;
                let mut steps = 0usize;
                loop {
                    steps += 1;
                    if steps > 256 {
                        break;
                    }
                    if !f.char_present(c) {
                        first_missing.get_or_insert((fid, c));
                        break;
                    }
                    let Some(ci) = f.chars.get(c as usize) else {
                        break;
                    };
                    if ci.tag == TAG_EXT {
                        found = Some(VarCand::Tfm(sz, fam, c));
                        break 'parts;
                    }
                    let u = f.char_height(c) + f.char_depth(c);
                    if u > w {
                        best = Some(VarCand::Tfm(sz, fam, c));
                        w = u;
                        if u >= v {
                            found = best.clone();
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
            Some(VarCand::Tfm(sz, fam, c)) => {
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
                    Some(rec) => vec![self.make_extensible_tex82(sz, fam, rec, v, style)],
                    None => vec![self.delim_char_box(sz, fam, c, style)],
                }
            }
            Some(VarCand::Ot { fid, gid, parts }) => vec![self.xe_delimiter_box(fid, gid, parts, v, style)],
            None => {
                if let Some((font, character)) = first_missing {
                    let _ = self.math_font_has_character_or_warn(font, character, origin);
                }
                vec![self.null_delimiter_box(style)]
            }
        }
    }

    /// tex.web make_extensible: stack top / n x rep / mid / n x rep / bottom
    /// with n grown until the total extent reaches `v` (in pairs when a mid
    /// part exists). Height = top part's height, depth = w - height.
    fn make_extensible_tex82(
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
                vec![Node::Char { c: ch, font: fid, attr: self.eqtb.cur_attr }],
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

/// A delimiter variant candidate of `var_delimiter`
#[derive(Clone)]
enum VarCand {
    /// TFM character `c` of family `fam` at font size index `sz`
    Tfm(usize, u8, u8),
    /// glyph `gid` (or the assembly `parts` built around it) of a native font
    Ot { fid: FontId, gid: u16, parts: Option<Vec<crate::math_xetex::Part>> },
}

// ---------- small local helpers ----------

/// tex.web `half` on a scaled value (see `half_sp`)
#[inline]
pub(crate) fn half_i(x: i32) -> i32 {
    half_sp(i64::from(x)) as i32
}

/// (width, height, depth) of a box
#[inline]
pub(crate) fn box_dims(n: &Node) -> (i32, i32, i32) {
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
pub(crate) fn accent_noad_of(nucleus: &[Node]) -> Option<(AccentSpec, &[Node], &MathDiagnosticOrigin)> {
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
        Node::Accent { spec, body, origin, .. } => Some((*spec, body, origin)),
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
                && matches!(list.first(), Some(Node::MathKern(_, 1, _)))
                && matches!(list.last(), Some(Node::MathKern(_, 2, _)))
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
        if let Node::Kern(k, _) = &vlist[1] {
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
        if let (Node::Kern(k1, _), Node::Rule { height, width, .. }, Node::Kern(k2, _)) =
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
        if let (Node::Kern(k1, _), Node::Kern(k2, _)) = (&vlist[1], &vlist[3]) {
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
        if let Node::Kern(k, _) = &vlist[1] {
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
        if let (Node::Kern(t, _), Node::Rule { height, .. }, Node::Kern(clr, _)) =
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
        if let Node::Kern(k, _) = &vlist[1] {
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

    /// oracle (pdftex -ini, this preamble): a `\left...\right` group nested
    /// inside another is an Inner atom of the outer body (Ord-Inner gets
    /// \thinmuskip), and mu glue in a body follows the body's own style.
    #[test]
    fn nested_left_right_group_is_inner_atom() {
        let e = run_doc(concat!(
            "\\delcode`[=\"05B302 \\delcode`]=\"05D303 \\thinmuskip=3mu\n",
            "\\setbox0\\hbox{$\\left[a\\left(b\\right)\\right]$}\n",
            "\\setbox1\\hbox{$\\left.a\\left(b\\right)\\right.$}\n",
            "\\setbox2\\hbox{$\\left[\\left(b\\right)a\\right]$}\n",
            "\\setbox3\\hbox{$\\left(\\scriptstyle a\\mskip18mu b\\right)$}\n",
            "\\message{W=\\the\\wd0,\\the\\wd1,\\the\\wd2,\\the\\wd3}\n",
        ));
        assert!(
            e.term.contains("W=24.57755pt,21.42197pt,24.57755pt,23.82654pt"),
            "{}",
            e.term
        );
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
        if let Node::Kern(k, _) = &vlist[1] {
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
            if let Node::Glue(g, _) = &list[i] {
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
        let glues = sup.iter().filter(|n| matches!(n, Node::Glue(_, _))).count();
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
    /// tex.web §760: when a leading binary operator is demoted to ord, an
    /// immediately following binary operator sees an ord on its left and
    /// remains a binary operator with medmuskip spacing (e.g. `-\otimes R`).
    #[test]
    fn leading_bin_demoted_allows_following_bin() {
        let b = text_math("-+R");
        let (list, _, _, _, _) = box_of(&b);
        let glues = list.iter().filter(|n| matches!(n, Node::Glue(_, _))).count();
        assert_eq!(glues, 2, "medmuskip on both sides of plus: {:?}", list);
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
                Node::Glue(g, _) => Some(g.width),
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
            lr: 0,
            dir: 0, attr: crate::boxes::Attr::NONE, subtype: 0,
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
                    lr: 0,
                    dir: 0, attr: crate::boxes::Attr::NONE, subtype: 0,
                },
                Node::Glue(Glue::spec(su(3.33333), su(1.66666), 0, su(1.11111), 0), crate::boxes::Attr::NONE),
                Node::Rule {
                    width: su(1.0),
                    height: su(0.4),
                    depth: 0, subtype: crate::boxes::RULE_NORMAL, index: 0, attr: crate::boxes::Attr::NONE,
                },
                Node::Kern(su(0.5), crate::boxes::Attr::NONE),
                Node::Char { c: b'A', font: cmr, attr: crate::boxes::Attr::NONE },
            ],
            0,
            0,
            0.0,
        );
        // last visible node is the 'A': 10 + 3.33333 + 1 + 0.5 + w(A) + 2quad
        let want =
            su(10.0) as i64 + su(3.33333) as i64 + su(1.0) as i64 + su(0.5) as i64 + wa + 2 * quad;
        assert_eq!(e.pre_display_size_of(&line, 0), want);
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
                    Node::Char { c: b'A', font: cmr, attr: crate::boxes::Attr::NONE },
                    Node::Glue(g, crate::boxes::Attr::NONE),
                    Node::Char { c: b'B', font: cmr, attr: crate::boxes::Attr::NONE },
                ],
                sign,
                0,
                0.5,
            )
        };
        let stretched = mk(
            1,
            Glue::spec(su(3.33333), su(1.66666), 0, 0, 0),
        );
        assert_eq!(e.pre_display_size_of(&stretched, 0), 0x3FFF_FFFF);
        let shrunk = mk(
            2,
            Glue::spec(su(3.33333), 0, 0, su(1.11111), 0),
        );
        assert_eq!(e.pre_display_size_of(&shrunk, 0), 0x3FFF_FFFF);
        // a *fil* line (order 2) leaves normal-order glue untouched: natural
        // widths accumulate and the trailing 'B' is visible
        let quad = e.eqtb.fonts[cmr as usize].quad() as i64;
        let wa = e.eqtb.fonts[cmr as usize].char_width(b'A') as i64;
        let wb = e.eqtb.fonts[cmr as usize].char_width(b'B') as i64;
        let fil_line = line_box(
            vec![
                Node::Char { c: b'A', font: cmr, attr: crate::boxes::Attr::NONE },
                Node::Glue(Glue::spec(su(3.33333), su(1.66666), 0, 0, 0), crate::boxes::Attr::NONE),
                Node::Char { c: b'B', font: cmr, attr: crate::boxes::Attr::NONE },
            ],
            1,
            2,
            0.5,
        );
        assert_eq!(
            e.pre_display_size_of(&fil_line, 0),
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
                    depth: 0, subtype: crate::boxes::RULE_NORMAL, index: 0, attr: crate::boxes::Attr::NONE,
                },
                Node::Leaders {
                    glue: Glue::spec(su(2.0), su(1.0), 2, 0, 0),
                    kind: 0,
                    body: crate::boxes::LeaderBody::Rule {
                        subtype: crate::boxes::RULE_NORMAL,
                        width: su(2.0),
                        height: 0,
                        depth: 0,
                    }, attr: crate::boxes::Attr::NONE,
                },
            ],
            1,
            2,
            0.5,
        );
        // line stretches at order 2 (fil) and the leader's stretch_order is
        // 2 → active: v := max_dimen, then `goto found` → w = max_dimen
        assert_eq!(e.pre_display_size_of(&line, 0), 0x3FFF_FFFF);
        // with the line inactive (sign 0) the leader is visible at natural width
        let calm = line_box(
            vec![
                Node::Rule {
                    width: su(1.0),
                    height: 0,
                    depth: 0, subtype: crate::boxes::RULE_NORMAL, index: 0, attr: crate::boxes::Attr::NONE,
                },
                Node::Leaders {
                    glue: Glue::spec(su(2.0), su(1.0), 2, 0, 0),
                    kind: 0,
                    body: crate::boxes::LeaderBody::Rule {
                        subtype: crate::boxes::RULE_NORMAL,
                        width: su(2.0),
                        height: 0,
                        depth: 0,
                    }, attr: crate::boxes::Attr::NONE,
                },
            ],
            0,
            0,
            0.0,
        );
        assert_eq!(
            e.pre_display_size_of(&calm, 0),
            su(1.0) as i64 + su(2.0) as i64 + 2 * quad
        );
    }

    /// Recursively search every node list the engine may still hold after
    /// a run (page list, parked paragraph lists, current list) for a glue
    /// node of exactly width `w` sp.
    fn has_glue_width(e: &Engine, w: i32) -> bool {
        fn walk(ns: &[Node], w: i32) -> bool {
            ns.iter().any(|n| match n {
                Node::Glue(g, _) => g.width == w,
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

    #[test]
    fn display_math_preserves_interline_parameters_from_display_context() {
        let e = run_doc(
            "\\hsize=240pt \\vsize=60in \\parindent=0pt \
             \\abovedisplayskip=30pt \\abovedisplayshortskip=1pt \
             \\everydisplay{\\lineskip=7pt \\lineskiplimit=100pt} \
             First line$$\\vbox to 50pt{}$$\n",
        );
        assert!(
            has_glue_width(&e, su(7.0)),
            "interline glue from inside display was not preserved"
        );
    }

    #[test]
    fn math_subformula_group_enters_inner_mode() {
        // tex.web §1197 / §21691: a group inside display math enters -mmode,
        // so \ifinner is true while inside the braces and false outside.
        let e = run_doc(
            "$$\\message{OUTER-INNER=\\ifinner T\\else F\\fi} \
             {\\message{INNER-INNER=\\ifinner T\\else F\\fi}} \
             \\message{RESTORED-INNER=\\ifinner T\\else F\\fi}$$",
        );
        assert!(e.term.contains("OUTER-INNER=F"));
        assert!(e.term.contains("INNER-INNER=T"));
        assert!(e.term.contains("RESTORED-INNER=F"));
    }

    #[test]
    fn char_94_in_math_mode_appends_mathchar_not_superscript() {
        // \char 94 in math mode is a character token (hat / circumflex),
        // not a superscript mark (catcode 7).
        let e = run_doc("$\\char94\\relax$");
        assert_eq!(e.error_count, 0);
    }

    /// tex.web §103 print_scaled, so expectations are TeX Live's `\the`
    /// output verbatim
    fn tex_pt(v: i32) -> String {
        let mut out = String::new();
        let mut s = i64::from(v);
        if s < 0 {
            out.push('-');
            s = -s;
        }
        out.push_str(&(s / 65536).to_string());
        out.push('.');
        s = 10 * (s % 65536) + 5;
        let mut delta: i64 = 10;
        loop {
            if delta > 65536 {
                s += 0o100000 - 50000;
            }
            out.push(char::from(b'0' + (s / 65536) as u8));
            s = 10 * (s % 65536);
            delta *= 10;
            if s <= delta {
                break;
            }
        }
        out + "pt"
    }

    /// the TeX Live probe parameters (plain values) used for the expected
    /// dimensions below
    const TL_PARAMS: &str = "\\delimiterfactor=901 \\delimitershortfall=5pt \
        \\thinmuskip=3mu \\medmuskip=4mu plus 2mu minus 4mu \\thickmuskip=5mu plus 5mu \
        \\delcode`|=\"26A30C \\mathcode`,=\"613B ";

    fn tl_dims(src: &str) -> (Node, String) {
        let b = text_math(&format!("{TL_PARAMS}{src}"));
        let (_, w, h, d, _) = box_of(&b);
        let dims = format!("{} {} {}", tex_pt(w), tex_pt(h), tex_pt(d));
        (b, dims)
    }

    /// `\hbox{$...$}` dimensions measured with TeX Live 2026 pdflatex
    /// (`\wd`/`\ht`/`\dp`) on the same cmr/cmmi/cmsy/cmex families.
    #[test]
    fn math_layout_matches_tex_live() {
        let cases = [
            // §1185 math_left_right: \over inside \left...\right only takes
            // the group's own list as numerator
            (r"\left(a\over b\right)", "15.90436pt 8.50005pt 3.50006pt"),
            (r"\left(\left(x\right)^2 a\over b\right)", "33.36969pt 11.50008pt 6.50009pt"),
            // eTeX \middle: ends the fraction, close/open spacing
            (r"\left( a\over b \middle| c\right)", "23.56525pt 8.50006pt 3.50006pt"),
            // §727: pass 1 resets the style after \middle
            (r"\left( \scriptstyle a \middle| b \right)", "19.18489pt 7.5pt 2.5pt"),
            // §762: Punct before the right delimiter takes a thin space
            (r"\left( a, \right)", "17.5081pt 7.5pt 2.5pt"),
            // §746: clearance uses the fraction's own rule thickness
            (r"{a\above 2pt b}", "6.73764pt 8.51389pt 5.3611pt"),
            // §720 clean_box keeps the italic-corrected width
            (r"{f\over g}", "7.08408pt 9.32217pt 4.80951pt"),
            // class 7 on a noad is Inner; Inner-Ord gets a thin space
            (r"\mathinner{x}y", "12.6435pt 4.30554pt 1.94444pt"),
            // §1160 \delimiter goes through set_math_char: class 7 takes \fam
            ("\\fam0 \\delimiter\"7162362", "5.55557pt 6.94444pt 0.0pt"),
            // §749: a lone vcenter nucleus keeps its negative depth
            (
                r"\displaystyle\mathop{\vcenter{\hrule width 30pt height 3pt}}\limits^{a}_{b}",
                "30.0pt 10.01387pt 6.52776pt",
            ),
            // §731: the chosen list is spliced into the surrounding mlist
            (r"a\mathchoice{+}{+}{+}{+}b", "21.79968pt 6.94444pt 0.83333pt"),
        ];
        for (src, want) in cases {
            let (_, got) = tl_dims(src);
            assert_eq!(got, want, "{src}");
        }
    }

    #[test]
    fn above_keeps_explicit_negative_thickness() {
        // TeX Live: `{a\above -1pt b}` is 6.73764pt x 6.9512pt + 3.44841pt
        // and its fraction rule is \rule(-1.0+0.0)x4.33765 (invisible)
        let (b, got) = tl_dims(r"{a\above -1pt b}");
        assert_eq!(got, "6.73764pt 6.9512pt 3.44841pt");
        fn has_rule(list: &[Node], want: i32) -> bool {
            list.iter().any(|n| match n {
                Node::Rule { height, .. } => *height == want,
                Node::Box { list, .. } => has_rule(list, want),
                _ => false,
            })
        }
        let (list, ..) = box_of(&b);
        assert!(has_rule(list, -su(1.0)), "fraction rule keeps -1pt: {list:?}");
    }

    #[test]
    fn math_accent_shift_uses_char_box_width_with_italic() {
        // TeX Live \showbox of `\mathaccent"017E f`:
        // \vbox(9.78334+1.94444)x5.97226 / \hbox(7.14444+0.0)x0.0, shifted 1.38376
        let (b, got) = tl_dims("\\skewchar\\teni=127 \\mathaccent\"017E f");
        assert_eq!(got, "5.97226pt 9.78334pt 1.94444pt");
        let (list, ..) = box_of(&b);
        let (vlist, ..) = box_of(&list[0]);
        assert_eq!(tex_pt(box_shift(&vlist[0])), "1.38376pt");
    }

    #[test]
    fn infinite_math_penalties_are_not_inserted() {
        // TeX Live line box for `\binoppenalty=10000 \relpenalty=500
        // \noindent$a+b=c$\par`: only \penalty 500 (after `=`) appears
        // inside the formula (§767: pen<inf_penalty)
        let e = run_doc(
            "\\hsize=200pt \\binoppenalty=10000 \\relpenalty=500 \
             \\setbox1\\vbox{\\noindent$a+b=c$\\par}",
        );
        let vbox = e.eqtb.boxed[1].clone().expect("box1");
        let (lines, ..) = box_of(&vbox);
        let (line, ..) = box_of(&lines[0]);
        let on = line.iter().position(|n| matches!(n, Node::MathKern(_, 1, _))).unwrap();
        let off = line.iter().position(|n| matches!(n, Node::MathKern(_, 2, _))).unwrap();
        let pens: Vec<i32> = line[on..off]
            .iter()
            .filter_map(|n| match n {
                Node::Penalty(p, _) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(pens, vec![500]);
    }

    #[test]
    fn display_alignment_shifts_rows_and_noalign_rules_only() {
        // TeX Live \showbox of the vbox: the paragraph line, row `a`, the
        // \noalign rule (re-boxed as \hbox(0.4+0.0)x5.55557) and row `b` are
        // shifted 30.0; the \noalign{\hbox{N}} box is not (tex.web §800)
        let e = run_doc(
            "\\tenrm \\hsize=200pt \\parskip=0pt \\setbox1=\\vbox{\\hangindent=30pt \\hangafter=0 \
             \\noindent X $$\\halign{#\\cr a\\cr\\noalign{\\hbox{N}}\\noalign{\\hrule}b\\cr}$$\\par}",
        );
        let vbox = e.eqtb.boxed[1].clone().expect("box1");
        let (list, ..) = box_of(&vbox);
        let shifts: Vec<String> = list
            .iter()
            .filter(|n| matches!(n, Node::Box { .. }))
            .map(|n| tex_pt(box_shift(n)))
            .collect();
        assert_eq!(shifts, ["30.0pt", "30.0pt", "0.0pt", "30.0pt", "30.0pt"]);
        let rule_box = list.iter().filter(|n| matches!(n, Node::Box { .. })).nth(3).unwrap();
        let (inner, w, h, ..) = box_of(rule_box);
        assert!(matches!(inner.as_slice(), [Node::Rule { .. }]), "{inner:?}");
        assert_eq!((tex_pt(w), tex_pt(h)), ("5.55557pt".to_string(), "0.4pt".to_string()));
    }
}
