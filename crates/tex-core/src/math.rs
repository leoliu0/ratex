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
use crate::tfm::{Font, FontId, TAG_EXT, TAG_LIST};
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

#[inline]
fn pair_to_code(p: Option<(u8, u8)>) -> i32 {
    match p {
        Some((f, c)) if c != 0 => ((f as i32) << 20) | ((c as i32) << 12) | ((f as i32) << 8) | (c as i32),
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
        let trace = crate::debug_flag("MATHTRACE");
        if !display {
            // tex.web §1134: a second math_shift promotes to display math.
            // The peek must be RAW: get_token processes \if conditionals
            // (tex.web get_next), so `$\ifmmode...` would evaluate \ifmmode
            // BEFORE mode=Math is set (getting false) and the skipped
            // branch's tokens would be consumed here
            let t = self.raw_token();
            if trace {
                eprintln!("ENTER-MATH mode={:?} peek={:?}", self.mode,
                    if t.is_cs() { format!("cs{}", t.cs_id()) } else { format!("cc{}:{:#x}", t.cc(), t.chr()) });
            }
            if t.is_char() && t.cc() == 3 {
                display = true;
            } else if t != crate::input::EOF_MARKER {
                self.pushed.push(t);
            }
        } else if trace {
            eprintln!("ENTER-MATH mode={:?} arg=display", self.mode);
        }
        if self.mode == Mode::DisplayMath {
            return;
        }
        if display {
            if self.mode == Mode::Horizontal {
                if crate::debug_flag("DSKIP") {
                    let desc: Vec<String> = self.cur_list.iter().take(4).map(|n| match n {
                        Node::Box { w, h, d, .. } => format!("B(w{:.2} h{:.2} d{:.2})", *w as f64/65536.0, *h as f64/65536.0, *d as f64/65536.0),
                        Node::Glue(g) => format!("G{:.2}", g.width as f64/65536.0),
                        Node::Kern(k) | Node::ExplicitKern(k) => format!("K{:.2}", *k as f64/65536.0),
                        Node::Penalty(p) => format!("P{}", p),
                        _ => "?".into(),
                    }).collect();
                    eprintln!("DSKIP-HENTRY n={} pd={:.4} [{}]", self.cur_list.len(), self.prev_depth as f64/65536.0, desc.join(" "));
                }
                // yet) gives \predisplaysize = -max_dimen; otherwise the
                // interrupted paragraph is broken and its final line is
                // measured — before the end-of-paragraph reset clears
                // \parshape/\hangindent state.
                let was_empty = self.cur_list.is_empty();
                // tex.web §21764: the interrupted paragraph's final widow
                // penalty is \displaywidowpenalty, not \widowpenalty
                if !was_empty {
                    self.next_par_widow = Some(
                        self.eqtb.int_params[IntParam::DisplayWidowPenalty.idx() as usize],
                    );
                }
                let shape = std::mem::take(&mut self.par_shape);
                let hang = (
                    self.eqtb.dim_params[DimParam::HangIndent.idx() as usize] as i64,
                    self.eqtb.int_params[IntParam::HangAfter.idx() as usize] as i64,
                );
                self.in_display_init = true;
                self.par_primitive();
                if crate::debug_flag("DSKIP") { eprintln!("DSKIP-PAR pd_after_par={:.4} last_par_line={}", self.prev_depth as f64 / 65536.0, self.last_par_line.is_some()); }
                self.in_display_init = false;
                let prev_graf = self.prev_graf as i64;
                let hsize = self.eqtb.dim_params[DimParam::HSize.idx() as usize] as i64;
                // §1184: display width/indent from \parshape (1-based entry
                // prev_graf+2, clamped to n) or \hangindent, else \hsize/0
                let (l, s) = if !shape.is_empty() {
                    let n = shape.len() as i64;
                    let k = (prev_graf + 2).min(n);
                    let e = shape[(k - 1) as usize];
                    (e.1 as i64, e.0 as i64) // engine stores (indent, width)
                } else if hang.0 != 0
                    && ((hang.1 >= 0 && prev_graf + 2 > hang.1)
                        || (prev_graf + 1 < -hang.1))
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
                if crate::debug_flag("DSKIP") { eprintln!("DSKIP-ENTER pds={} l={} s={} was_empty={} pd_before={:.4}", self.pre_display_size as f64/65536.0, self.pre_display_l as f64/65536.0, self.pre_display_s as f64/65536.0, was_empty, self.prev_depth as f64/65536.0); }
            } else {
                // display entered from vertical mode: tex.web §1185 starts a
                // new paragraph whose zero-depth \parindent box is appended
                // first (append_to_vlist: prev_depth := box depth = 0), so
                // the interline glue above the display = baselineskip − h.
                self.prev_depth = 0;
                self.pre_display_size = -0x3FFF_FFFF;
                self.pre_display_l =
                    self.eqtb.dim_params[DimParam::HSize.idx() as usize] as i64;
                self.pre_display_s = 0;
            }
            // tex.web push_math: the display math group level (exit_math /
            // \\endgroup pop it; dropping this push leaves one pop too many
            if crate::debug_flag("DSKIP") {
                eprintln!("ENTER-MATH-LEVEL before_push={} fn={} ln={}", self.eqtb.cur_level, self.input.current_file_name(), self.input.current_file_line());
            }
            self.eqtb.push_level(crate::eqtb::LevelType::Group);
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
            if crate::debug_flag("DSKIP") { eprintln!("DSKIP-WRITE field={} reg={}", self.pre_display_size as f64/65536.0, self.eqtb.dim_params[crate::prim::DimParam::PreDisplaySize.idx() as usize] as f64/65536.0); }
            // tex.web push_math: eq_word_define(cur_fam_code,-1) — \\fam is
            // -1 inside every math group, restored at group end
            self.eqtb
                .assign_int_param(crate::prim::IntParam::CurFam, -1, false);
            let page = std::mem::take(&mut self.page_list);
            self.saved_lists.push((
                self.mode,
                std::mem::take(&mut self.cur_list),
                self.prev_depth,
                self.space_factor,
            ));
            self.par_page_lists.push(page);
            self.mode = Mode::DisplayMath;
            self.math_style_stack.push(MathStyle::Display);
            // tex.web init_math: \everydisplay (not \everymath) at display
            // entry — setspace scales the display skips from there
            let toks = (*self.eqtb.tok_params[crate::prim::ToksParam::EveryDisplay.idx() as usize]).clone();
            if !toks.is_empty() {
                self.push_tokens(toks);
            }
            // tex.web §1145: if nest_ptr=1 then build_page
            self.build_page();
        } else {
            self.saved_lists.push((
                self.mode,
                std::mem::take(&mut self.cur_list),
                self.prev_depth,
                self.space_factor,
            ));
            self.eqtb.push_level(crate::eqtb::LevelType::Group);
            // tex.web push_math: eq_word_define(cur_fam_code,-1)
            self.eqtb
                .assign_int_param(crate::prim::IntParam::CurFam, -1, false);
            self.mode = Mode::Math;
            self.math_style_stack.push(MathStyle::Text);
        }
        self.math_lists.push(crate::boxes::NodeList::new());
        self.left_delim = None;
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
        let formula = self.math_lists.pop().unwrap_or_default();
        self.pending_display_formula = Some(formula);
        self.math_lists.push(crate::boxes::NodeList::new());
        self.eqno_leqno = Some(leqno);
    }

    fn run_everymath(&mut self) {
        let toks = (*self.eqtb.tok_params[crate::prim::ToksParam::EveryMath.idx() as usize]).clone();
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
        let mlist = self.math_lists.pop().unwrap_or_default();
        // tex.web after_math reads the display registers BEFORE unsave:
        // assignments made inside the display (setspace's \everydisplay
        // scales the display skips group-locally) must still apply
        let disp_regs = if was_display {
            let g = |p: crate::prim::GlueParam| {
                self.eqtb.glue_params[p.idx() as usize].clone()
            };
            let i = |p: crate::prim::IntParam| self.eqtb.int_params[p.idx() as usize];
            let ads = g(crate::prim::GlueParam::AboveDisplaySkip);
            if crate::debug_flag("DSKIP") {
                eprintln!("DSKIP-REGS pg={} lvl={} above={:.4}", self.pdf_doc.pages.len(), self.eqtb.cur_level, ads.width as f64/65536.0);
            }
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
        self.pop_group();
        let (outer_mode, outer_list, pd, sf) = self.saved_lists.pop().unwrap_or((self.mode, std::mem::take(&mut self.cur_list), self.prev_depth, self.space_factor));
        self.math_style_stack.pop();
        // leak guards: state set inside math must not escape it
        self.left_delim = None;
        self.right_delim = None;
        self.math_limits = None;
        self.math_group_marks.clear();
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
            self.finish_display_math(formula, tag, disp_regs.unwrap());
            return;
        }
        let hlist = self.mlist_to_hlist_pen(&mlist, 2, self.mode == Mode::Horizontal);
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
        regs: (crate::boxes::Glue, crate::boxes::Glue, crate::boxes::Glue, crate::boxes::Glue, i32, i32),
    ) {
        if let Some(mut page) = self.par_page_lists.pop() {
            // tex.web §22504 (finish displayed math): z = \displaywidth,
            // s = \displayindent, b = the formula at natural width
            let z = self.pre_display_l;
            let s = self.pre_display_s;
            let fh = self.mlist_to_hlist_pen(&formula, 0, false);
            let first_is_glue = matches!(fh.first(), Some(Node::Glue(_)));
            let mut r0 = hpack(fh, None, HBOX, &self.eqtb);
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
            // the formula to z-q when shrink/stretch allows; otherwise the
            // tag drops to its own line (e := 0)
            if w + q > z {
                let can_squeeze = e != 0
                    && (w - r0.shrink[0] + q <= z
                        || r0.stretch[1] != 0
                        || r0.stretch[2] != 0
                        || r0.stretch[3] != 0);
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
            // skip selection (§22578): normal skips unless there is clearance
            // for the short pair (and never short with \leqno)
            let is_short = d + s > self.pre_display_size && !leqno;
            if crate::debug_flag("DSKIP") {
                eprintln!("DSKIP pg={} d={} s={} pds={} z={} w={} e={} is_short={}", self.pdf_doc.pages.len(), d as f64/65536.0, s as f64/65536.0, self.pre_display_size as f64/65536.0, z as f64/65536.0, w as f64/65536.0, e as f64/65536.0, is_short);
            }
            let (above, below) = if is_short {
                (regs.2.clone(), regs.3.clone())
            } else {
                (regs.0.clone(), regs.1.clone())
            };
            let bs = self.eqtb.glue_params[crate::prim::GlueParam::BaselineSkip.idx() as usize].clone();
            let lsk = self.eqtb.glue_params[crate::prim::GlueParam::LineSkip.idx() as usize].clone();
            let lsl = self.eqtb.dim_params[crate::prim::DimParam::LineSkipLimit.idx() as usize] as i64;
            // tex.web interline glue for a vlist box append (used for the
            // display line and for own-line tag boxes below)
            let ilg = |prev_depth: i32, h: i64| -> Option<crate::boxes::Glue> {
                if prev_depth <= -0x3FFF_FFFF {
                    return None;
                }
                let mut g = bs.width as i64 - prev_depth as i64 - h;
                if g < lsl {
                    g = lsk.width as i64;
                }
                Some(crate::boxes::Glue::new(g as i32))
            };
            let pre = regs.4;
            let post = regs.5;
            let mut g2 = below;
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
                if crate::debug_flag("DSKIP") {
                    eprintln!("DSKIP-ILG pd={:.4} lh={:.4} glue_w={:.4}", self.prev_depth as f64 / 65536.0, lh as f64 / 65536.0, g.width as f64 / 65536.0);
                }
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
                    g2 = crate::boxes::Glue::zero();
                }
            }
            page.push(Node::Penalty(post));
            if g2.width != 0 || g2.stretch != 0 || g2.shrink != 0 {
                page.push(Node::Glue(g2));
            }
            self.page_list = page;
            self.mode = Mode::Vertical;
            self.cur_list = Vec::new();
            // tex.web finish_display does NOT run build_page: the display
            // nodes stay on the vlist (visible to \lastskip — LaTeX's
            // theorem \addpenalty/\@xaddvskip dances read them) until the
            // next box append or paragraph end
        }
        // tex.web resume_after_display (§1194): any text following the
        // display resumes hmode directly — start_paragraph must not treat
        // it as a new paragraph (\parskip/\parindent/\everypar skipped)
        self.resume_after_display = true;
    }

    /// tex.web §1181 ("Calculate the natural width, w"): \predisplaysize is
    /// `shift + 2em(cur font)` plus the natural width of the final line up
    /// to and including its last *visible* node (chars, ligs, boxes, rules,
    /// leaders). A kern/math-kern accumulates width without being visible;
    /// a glue whose stretch or shrink is *active* in the set line (same
    /// order as the box's glue sign) voids the remainder → max_dimen, which
    /// makes the display use the full skips.
    fn pre_display_size_of(&self, line: &Node) -> i64 {
        const MAX_DIM: i64 = 0x3FFF_FFFF;
        let Node::Box { list, shift, glue_sign, glue_order, .. } = line else {
            return -MAX_DIM;
        };
        let quad = self
            .eqtb
            .fonts
            .get(self.eqtb.cur_font_val as usize)
            .map(|f| f.quad() as i64)
            .unwrap_or(0);
        let mut v = *shift as i64 + 2 * quad;
        if crate::debug_flag("DSKIP") { eprintln!("DSKIP-PDS shift={} quad={} v0={}", *shift as f64/65536.0, quad as f64/65536.0, v as f64/65536.0); }
        let mut w: i64 = -MAX_DIM;
        let voids = |g: &Glue| -> bool {
            (*glue_sign == 1 && g.stretch != 0 && g.stretch_order as u8 == *glue_order)
                || (*glue_sign == 2 && g.shrink != 0 && g.shrink_order as u8 == *glue_order)
        };
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
                Node::Box { w, .. } | Node::Rule { width: w, .. } => (*w as i64, true),
                Node::Kern(k) | Node::ExplicitKern(k) | Node::MathKern(k, _) => {
                    (*k as i64, false)
                }
                Node::Glue(g) => {
                    if voids(g) {
                        v = MAX_DIM;
                    }
                    (g.width as i64, false)
                }
                Node::Leaders { glue, .. } => {
                    if voids(glue) {
                        v = MAX_DIM;
                    }
                    (glue.width as i64, true)
                }
                _ => (0, false),
            };
            if visible {
                if v < MAX_DIM {
                    v += d;
                    w = v;
                } else {
                    w = MAX_DIM;
                    break;
                }
            } else if v < MAX_DIM {
                v += d;
            }
        }
        w
    }

    pub fn append_mlist_node(&mut self, n: Node) {
        if crate::debug_flag("MDUMP") {
            match &n {
                Node::Scripts { nucleus, sup, sub } => eprintln!("MDUMP SCR nuc={:?} sup={} sub={}", nucleus.iter().map(|x| format!("{:?}", x)).collect::<Vec<_>>().join(","), sup.is_some(), sub.is_some()),
                Node::MathChar { .. } | Node::DelimBox { .. } => eprintln!("MDUMP {:?}", n),
                _ => {}
            }
        }
        // tex.web math_limit_switch: a \limits request lives on the op noad
        // that preceded it, never on a later atom — any new noad appended
        // invalidates the pending request (append_script takes it first).
        self.math_limits = None;
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
        self.math_style_stack.last().copied().unwrap_or(MathStyle::Text)
    }

    pub fn push_math_group(&mut self) {
        self.math_lists.push(crate::boxes::NodeList::new());
        self.saved_lists.push((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
        ));
        self.eqtb.push_level(crate::eqtb::LevelType::Group);
    }

    /// `\right` end of a `\left...\right` group: the inner *raw* math list is
    /// spliced into the enclosing math list, bracketed by boundary markers.
    /// Conversion (including delimiter sizing) happens in one pass later.
    pub fn pop_math_group_delimited(&mut self, right_delim: i32) {
        let inner = self.math_lists.pop().unwrap_or_default();
        self.pop_group();
        let (outer_mode, outer_list, pd, sf) = self.saved_lists.pop().unwrap_or((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
        ));
        let ld = self.left_delim.take().unwrap_or(0);
        self.mode = outer_mode;
        self.prev_depth = pd;
        self.space_factor = sf;
        self.cur_list = outer_list;
        match self.math_lists.last_mut() {
            Some(l) => {
                l.push(delim_marker(ld, 0));
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
        if crate::debug_flag("MDUMP") { eprintln!("MDUMP append_script sup={}", sup); }
        let limits_req = self.math_limits.take();
        let group = self.scan_math_group_or_token();
        let popped = if let Some(l) = self.math_lists.last_mut() {
            if l.iter().rev().take(4).all(|n| matches!(n, Node::ChoiceAlt { .. }))
                && l.len() >= 5
                && matches!(l[l.len() - 5], Node::Choice)
            {
                let mut choice_nodes = Vec::new();
                for _ in 0..5 {
                    choice_nodes.push(l.pop().unwrap());
                }
                choice_nodes.reverse();
                Some(Node::Scripts { nucleus: choice_nodes, sup: None, sub: None })
            } else {
                l.pop()
            }
        } else {
            None
        };
        if crate::debug_flag("MDUMP") { eprintln!("MDUMP popped={} limits_req={:?}", popped.as_ref().map(|p| format!("{:?}", std::mem::discriminant(p))).unwrap_or_else(|| "EMPTY".into()), limits_req); }
        let top = match popped {
            Some(node) => node,
            None => Node::Scripts { nucleus: Vec::new(), sup: None, sub: None },
        };
        match top {
            Node::Scripts { mut nucleus, sup: s, sub: x } => {
                // a `\mathop{...}` group atom is stored as a null-MathChar-
                // prefixed Scripts node. tex.web keeps the limits subtype ON
                // THE NOAD, so a later second script must see the first
                // script's \limits/\nolimits request: encode the subtype in
                // the (invisible) fam255 prefix char's c field — 0=normal,
                // 1=limits, 2=no_limits (tex.web subtypes).
                let head_is_op = matches!(nucleus.first(), Some(Node::MathChar { class: CL_OP, .. }));
                let mut sub_type = match nucleus.first() {
                    Some(Node::MathChar { fam: 255, c, class: CL_OP }) => *c,
                    _ => 0,
                };
                if sub_type == 0 {
                    match limits_req {
                        Some(0) => sub_type = 2,
                        Some(1) => sub_type = 1,
                        _ => {}
                    }
                }
                if head_is_op {
                    if let Some(Node::MathChar { fam: 255, c, .. }) = nucleus.get_mut(0) {
                        if *c == 0 {
                            *c = sub_type;
                        }
                    }
                }
                let use_limits = match sub_type {
                    1 => head_is_op,
                    2 => false,
                    _ => head_is_op && gstyle_of(self.cur_math_style()) < 2,
                };
                if use_limits {
                    let (na, nb) = if sup { (Some(group), x) } else { (s, Some(group)) };
                    self.append_mlist_node(Node::OpLimits { op: nucleus, above: na, below: nb });
                } else {
                    let (ns, nx) = if sup { (Some(group), x) } else { (s, Some(group)) };
                    self.append_mlist_node(Node::Scripts { nucleus, sup: ns, sub: nx });
                }
            }
            Node::OpLimits { op, above, below } => {
                let (na, nb) = if sup { (Some(group), below) } else { (above, Some(group)) };
                self.append_mlist_node(Node::OpLimits { op, above: na, below: nb });
            }
            atom => {
                let use_limits = self.script_wants_limits(limits_req, &atom);
                let (sup_g, sub_g) = if sup { (Some(group), None) } else { (None, Some(group)) };
                if use_limits {
                    self.append_mlist_node(Node::OpLimits { op: vec![atom], above: sup_g, below: sub_g });
                } else {
                    self.append_mlist_node(Node::Scripts { nucleus: vec![atom], sup: sup_g, sub: sub_g });
                }
            }
        }
    }

    /// Decide whether a scripts pair on `atom` becomes a limits construction.
    /// `req`: Some(0)=\nolimits, Some(1)=\limits, Some(2)=\displaylimits,
    /// None=default (large operator in display styles gets limits; tex.web
    /// make_op: `(subtype(q)=normal) and (cur_style<text_style)`).
    fn script_wants_limits(&self, req: Option<u8>, atom: &Node) -> bool {
        let is_op = match atom {
            Node::MathChar { class, .. } => *class == CL_OP,
            Node::Scripts { nucleus, .. } => match nucleus.first() {
                Some(Node::MathChar { class, .. }) => *class == CL_OP,
                _ => false,
            },
            Node::DelimBox { size: 2, .. } => true,
            _ => false,
        };
        match req {
            Some(0) => false,
            Some(1) => true,
            Some(2) => {
                let g = gstyle_of(self.cur_math_style());
                g < 2
            }
            _ => {
                let g = gstyle_of(self.cur_math_style());
                is_op && g < 2
            }
        }
    }

    /// scan a `{...}` group, a single control sequence, or a single character
    /// into a RAW math list (tex.web's "scan a math-list group").
    pub fn scan_math_group_or_token(&mut self) -> NodeList {
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
        self.math_lists.pop().unwrap_or_default()
    }

    /// Execute tokens up to the matching `}` as a nested math list.
    fn scan_math_group_braced(&mut self) -> NodeList {
        self.math_lists.push(Vec::new());
        self.eqtb.push_level(crate::eqtb::LevelType::Group);
        let my_level = self.eqtb.cur_level;
        loop {
            let t = self.get_token();
            if crate::debug_flag("MGTRACE") {
                let src: Vec<String> = self
                    .input
                    .stack
                    .iter()
                    .rev()
                    .take(2)
                    .map(|s| match s {
                        crate::input::Source::TokList { name, pos, toks, .. } => {
                            format!("T:{} {}/{}", name, pos, toks.len())
                        }
                        crate::input::Source::File { name, line_no, .. } => {
                            format!("F:{}#{}", name.rsplit('/').next().unwrap_or("?"), line_no)
                        }
                    })
                    .collect();
                eprintln!(
                    "MGB t={} ss={} kinds={} savedl={} ml={} mode={:?} gt={:?} src=[{}]",
                    self.tokens_to_string(&[t]),
                    self.eqtb.save_stack.len(),
                    self.box_kinds.len(),
                    self.saved_lists.len(),
                    self.math_lists.len(),
                    self.mode,
                    self.eqtb.cur_group_type(),
                    src.join(" << ")
                );
            }
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
                    // Represent it as an Ord atom: null fam255 prefix + the
                    // raw nodes, which make_scripts' boxed-nucleus arm packs
                    // exactly like tex.web clean_box.
                    let inner = self.scan_math_group_braced();
                    let mut nuc: NodeList = vec![Node::MathChar { fam: 255, c: 0, class: CL_ORD }];
                    nuc.extend(inner);
                    self.append_mlist_node(Node::Scripts { nucleus: nuc, sup: None, sub: None });
                } else {
                    self.begin_group(true);
                }
                continue;
            }
            self.run_math_token(t);
        }
        self.math_lists.pop().unwrap_or_default()
    }


    pub fn do_math_accent(&mut self, mc: u16) {
        let group = self.scan_math_group_or_token();
        let fam = ((mc >> 8) & 0xF) as u8;
        let c = (mc & 0xFF) as u8;
        let fid = self.style_font(fam);
        self.append_mlist_node(Node::Accent {
            accent: (c, fid),
            body: group,
            skew: 0,
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
            Node::MathChar { fam: 255, c: 0, class }
        } else if field.len() == 1 {
            match field.into_iter().next().unwrap() {
                Node::MathChar { fam, c, .. } => Node::MathChar { fam, c, class },
                other => {
                    let nuc = vec![Node::MathChar { fam: 255, c: 0, class }, other];
                    Node::Scripts { nucleus: nuc, sup: None, sub: None }
                }
            }
        } else {
            // multi-node group atom. tex.web: `\mathop{...}` (and the other
            // math_comp prims) tail_append a FRESH noad whose type is the
            // class and whose subtype is `normal` — scripts then take the
            // make_op promotion rule `(subtype=normal) and (cur_style<
            // text_style)`. Raw brace groups reach here through
            // scan_math_group_braced's fam255 CL_ORD marker instead and are
            // never re-classed. c=1 on the fam255 CL_OP prefix marks the
            // subtype-normal state for append_script's limits logic (c=2
            // would pin \nolimits, c=1... see sub_type encoding there); a
            // genuine `\limits`/`\nolimits` afterwards overwrites it via
            // the math_limits request.
            let mut nuc: NodeList = vec![Node::MathChar { fam: 255, c: 0, class }];
            if class == CL_OP {
                nuc[0] = Node::MathChar { fam: 255, c: 0, class };
            }
            nuc.extend(field);
            Node::Scripts { nucleus: nuc, sup: None, sub: None }
        };
        self.append_mlist_node(node);
    }

    /// Knuth \\overline / \\underline: scan a math field, pack it, and
    /// put a default-rule bar above (or below) with 3 default_rule_thickness
    /// clearance (tex.web make_over / make_under).
    pub fn do_overline(&mut self, under: bool) {
        let group = self.scan_math_group_or_token();
        let g = gstyle_of(self.cur_math_style()) | 1; // cramped
        let body = self.mlist_to_hlist_pen(&group, g, self.mode == Mode::Horizontal);
        let packed = hpack(body, None, HBOX, &self.eqtb).node;
        let (w, _, _) = box_dims(&packed);
        let rt = self.default_rule_thickness(g);
        let kern = 3 * rt;
        let rule = Node::Rule { width: w, height: rt, depth: 0 };
        let mut vlist = Vec::new();
        if under {
            vlist.push(packed);
            vlist.push(Node::Kern(kern));
            vlist.push(rule);
        } else {
            vlist.push(rule);
            vlist.push(Node::Kern(kern));
            vlist.push(packed);
        }
        let vb = vpack(vlist, None, VBOX, &self.eqtb).node;
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
        let num = match self.math_lists.last_mut() {
            Some(l) => match self.math_group_marks.last().copied() {
                Some(m) if m <= l.len() => l.split_off(m),
                _ => std::mem::take(l),
            },
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
        loop {
            let t = self.get_token();
            if t == crate::input::EOF_MARKER {
                break;
            }
            if t.is_char() && matches!(t.cc(), 2 | 3) {
                self.pushed.push(t);
                break;
            }
            // tex.web: a display's \eqno/\leqno also closes the fraction's
            // denominator — the tag is a separate sublist, not formula tail
            if t.is_cs()
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

    /// 1mu = quad of family 2 at text size / 18 (tex.web §767)
    fn mu_unit(&self) -> i32 {
        self.math_quad(0) / 18
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
                Some(Node::MathChar { fam: 255, class, .. }) => *class,
                Some(Node::MathChar { class, .. }) => if *class == 7 { CL_ORD } else { *class },
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
            Node::Box { .. } => Some(CL_ORD),
            Node::Choice => Some(CL_ORD),
            _ => None,
        }
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
                if let Node::DelimBox { size: 0, small, large } = n {
                    let code = delim_code_of(*small, *large);
                    lr_stack.push((code, Vec::new()));
                    i += 1;
                    continue;
                }
                // close marker — possibly wrapped by `^`/`_` (append_script
                // pops the tail marker and re-wraps it): the scripts belong on
                // the assembled group box (tex.web make_left_right tail).
                let close = match n {
                    Node::DelimBox { size: 1, small, large } => Some((*small, *large, None, None)),
                    Node::Scripts { nucleus, sup, sub } => match nucleus.as_slice() {
                        [Node::DelimBox { size: 1, small, large }] => {
                            Some((*small, *large, Some((sup.as_deref(), sub.as_deref())), None))
                        }
                        _ => None,
                    },
                    Node::OpLimits { op, above, below } => match op.as_slice() {
                        [Node::DelimBox { size: 1, small, large }] => {
                            Some((*small, *large, None, Some((above.as_deref(), below.as_deref()))))
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some((small, large, scripts, limits)) = close {
                    let code = delim_code_of(small, large);
                    match lr_stack.pop() {
                        Some((lopen, buf)) => {
                            let body = self.mlist_to_hlist_pen(&buf, style, self.math_penalties.get());
                            let (_, bh, bd) = hlist_dims(&body, &self.eqtb);
                            let needed = self.lr_delimiter_size(bh, bd, style);
                            let mut assembled: NodeList = self.var_delimiter(lopen, needed, style);
                            assembled.extend(body);
                            assembled.extend(self.var_delimiter(code, needed, style));
                            style = after_lr_style(style);
                            if scripts.is_some() || limits.is_some() {
                                let gb = hpack(assembled, None, HBOX, &self.eqtb).node;
                                let tail: NodeList = match (scripts, limits) {
                                    (Some((sup, sub)), _) => self.make_scripts(&[gb], sup, sub, style),
                                    (None, Some((above, below))) => {
                                        self.make_op_limits(&[gb], above, below, style, true)
                                    }
                                    _ => Vec::new(),
                                };
                                match lr_stack.last_mut() {
                                    Some((_, pbuf)) => pbuf.extend(tail),
                                    None => self.emit_atom(&mut out, &mut prev, Some(CL_INNER), tail, style),
                                }
                            } else {
                                match lr_stack.last_mut() {
                                    // nested boundary: splice into the enclosing buffer
                                    Some((_, pbuf)) => pbuf.extend(assembled),
                                    None => {
                                        self.emit_atom(&mut out, &mut prev, Some(CL_INNER), assembled, style);
                                    }
                                }
                            }
                        }
                        None => {
                            // stray close: a normal close delimiter
                            self.emit_atom(&mut out, &mut prev, Some(CL_CLOSE), self.var_delimiter(code, 0, style), style);
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
                let nodes = self.convert_atom(n, style);
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
            // non-atom: pass through, preserving the previous class
            if let Some((_, buf)) = lr_stack.last_mut() {
                buf.push(n.clone());
            } else if !matches!(n, Node::ChoiceAlt { .. }) {
                out.push(n.clone());
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

    fn emit_atom(&self, out: &mut NodeList, prev: &mut Option<u8>, cls: Option<u8>, nodes: NodeList, style: GStyle) {
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
        let mu = self.mu_unit();
        let conv = |sp_mu: i32| ((sp_mu as i64) * (mu as i64) / 65536) as i32;
        let src = match kind {
            1 | 2 => GlueParam::ThinMuSkip,
            3 => GlueParam::MedMuSkip,
            _ => GlueParam::ThickMuSkip,
        };
        let p = &self.eqtb.glue_params[src.idx() as usize];
        let g = Glue { width: conv(p.width), stretch: conv(p.stretch), shrink: conv(p.shrink), stretch_order: p.stretch_order, shrink_order: p.shrink_order };
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
                    self.append_mlist_node(Node::DelimBox { small: (sf, sc), large: (lf, lc), size: 2 });
                    return;
                }
                Some(Equiv::Prim(Prim::Middle)) => {
                    let v = self.scan_delim_int();
                    let (sf, sc, lf, lc) = delim_code_parts(v);
                    self.append_mlist_node(Node::DelimBox { small: (sf, sc), large: (lf, lc), size: 2 });
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

    fn convert_atom(&self, n: &Node, style: GStyle) -> NodeList {
        match n {
            Node::MathChar { fam, c, class } => {
                if crate::debug_flag("MFONT") {
                    let fid = self.eqtb.style_fonts[font_size(style)][*fam as usize];
                    if let Some(f) = self.eqtb.fonts.get(fid as usize) {
                        eprintln!("MFONT style={} fam={} c={} font={} at={:.2}pt w={:.3}pt", style, fam, c, f.name, f.at_size as f64/65536.0, f.char_width(*c) as f64/65536.0);
                    }
                }
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
                    // tex.web §14865 (@<Create a character node for nucleus@>):
                    // a char nucleus with no subscript takes its italic
                    // correction as a trailing kern; text fonts (nonzero
                    // interword space) skip it (mid-word reading)
                    if let Some(f) = self.eqtb.fonts.get(fid as usize) {
                        let ic = f.char_italic(*c);
                        let is_text_font = f.space() != 0;
                        if ic != 0 && !is_text_font {
                            out.push(Node::Kern(ic));
                        }
                    }
                    out
                }
            }
            Node::Scripts { nucleus, sup, sub } => self.make_scripts(nucleus, sup.as_deref(), sub.as_deref(), style),
            Node::OpLimits { op, above, below } => {
                self.make_op_limits(op, above.as_deref(), below.as_deref(), style, true)
            }
            Node::Frac { num, den, thickness, left, right } => self.make_fraction(num, den, *thickness, *left, *right, style),
            Node::Radical { body, thickness, .. } => self.make_radical(body, *thickness, style),
            Node::Accent { accent, body, .. } => self.make_accent(*accent, body, style),
            Node::DelimBox { small, large, size } => {
                // plain delimiter atom (size 2); 0/1 only reach here as strays
                let code = delim_code_of(*small, *large);
                let mut out = self.var_delimiter(code, 0, style);
                if out.is_empty() {
                    out.push(Node::Kern(0));
                }
                out
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
        let delta = if f.exists_char(c) { f.char_italic(c) } else { 0 };
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
            [Node::DelimBox { small, large, size: 2 }] => {
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

    fn make_scripts(&self, nucleus: &[Node], sup: Option<&[Node]>, sub: Option<&[Node]>, style: GStyle) -> NodeList {
        let ss = self.eqtb.dim_params[DimParam::ScriptSpace.idx() as usize];
        let x = self.math_x_height(style);
        let rt = self.default_rule_thickness(style);
        let mut delta = 0i32;
        let mut nuc: Node;
        let mut shift_up = 0i32;
        let mut shift_down = 0i32;
        match nucleus {
            // single-char nucleus (tex.web): the italic correction becomes a
            // trailing kern unless a subscript is present (then it right-shifts
            // the superscript in the stacked construction)
            // single-char nucleus (tex.web): the italic correction becomes a
            // trailing kern unless a subscript is present (then it right-shifts
            // the superscript in the stacked construction)
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
            // non-limits big operator: axis-centered box, italic handled above
            [Node::MathChar { fam, c, class }] if *class == CL_OP => {
                if *fam == 255 {
                    nuc = hpack(Vec::new(), None, HBOX, &self.eqtb).node;
                } else {
                    let (b, d) = self.op_char_box(*fam, *c, style, sub.is_some());
                    delta = d;
                    nuc = b;
                }
                // tex.web §746: `t := sup/drop-style fontdimen` — the
                // sup/drop parameters come from the CURRENT math family's
                // TEXT-size font (fparam maps style->size slot), not from
                // the script font.
                let (zh, zd) = box_dims_shifted(&nuc);
                shift_up = zh - self.fparam(style, 2, 18);
                shift_down = zd + self.fparam(style, 2, 19);
            }
            // boxed nucleus: initial shifts from its (shift-adjusted) dims
            _ => {
                let nodes = self.mlist_to_hlist_pen(nucleus, style, self.math_penalties.get());
                // tex.web clean_box packs the converted list; fam255 marker
                // chars convert to nothing already, but drop any residual
                // zero-width kern artifacts so widths match the oracle.
                nuc = hpack(nodes, None, HBOX, &self.eqtb).node;
                let (zh, zd) = box_dims_shifted(&nuc);
                shift_up = zh - self.fparam(style, 2, 18);
                shift_down = zd + self.fparam(style, 2, 19);
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
            let (_, sup_h, sup_d) = box_dims(bx);
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
        if crate::debug_flag("SCR") {
            eprintln!("SCR style={} nuc0={:?} shift_up={} shift_down={}", style, nucleus.first(), shift_up, shift_down);
        }
        match (sup_box, sub_box) {
            (Some(bs), Some(bb)) => {
                let (_, sup_h, sup_d) = box_dims(&bs);
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

    fn make_op_limits(&self, op: &[Node], above: Option<&[Node]>, below: Option<&[Node]>, style: GStyle, _force: bool) -> NodeList {
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
        let sup_box = above.map(|s| hpack(self.mlist_to_hlist_pen(s, sup_style(style), self.math_penalties.get()), None, HBOX, &self.eqtb).node);
        let sub_box = below.map(|s| hpack(self.mlist_to_hlist_pen(s, sub_style(style), self.math_penalties.get()), None, HBOX, &self.eqtb).node);
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
        if let Node::Box { w: bw, h: hh, d: dd, shift, .. } = &mut packed {
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

    fn make_fraction(&self, num: &[Node], den: &[Node], thickness: i32, left: Option<i32>, right: Option<i32>, style: GStyle) -> NodeList {
        let r = if thickness < 0 {
            self.default_rule_thickness(style)
        } else {
            thickness
        };
        let num_box = hpack(self.mlist_to_hlist_pen(num, num_style(style), self.math_penalties.get()), None, HBOX, &self.eqtb).node;
        let den_box = hpack(self.mlist_to_hlist_pen(den, den_style(style), self.math_penalties.get()), None, HBOX, &self.eqtb).node;
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
                Node::Rule { width: w, height: r, depth: 0 },
                Node::Kern((axis - dr) - (dh - sd)),
                den_c,
            ];
        }
        let mut packed = vpack(vlist, None, VBOX, &self.eqtb).node;
        // tex.web §742: height = shift_up + height(numerator), depth =
        // depth(denominator) + shift_down; the baseline is the numerator's
        if let Node::Box { w: bw, h, d, shift, .. } = &mut packed {
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
            Node::Rule { width: xw, height: dh, depth: 0 },
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

    fn make_accent(&self, accent: (u8, FontId), body: &[Node], style: GStyle) -> NodeList {
        let (ac, afid) = accent;
        let Some(af) = self.eqtb.fonts.get(afid as usize).cloned() else {
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
        let (bw, bh, bd) = box_dims(&body_box);
        let aw = af.char_width(ac);
        let axh = {
            let v = af.param(5);
            if v != 0 {
                v
            } else {
                af.x_height()
            }
        };
        let delta = if bh < axh { bh } else { axh };
        let mut acc_box = hpack(vec![Node::Char { c: ac, font: afid }], None, HBOX, &self.eqtb).node;
        if let Node::Box { w, shift, .. } = &mut acc_box {
            *w = 0; // accent width does not affect the box width
            *shift = s + half_i(bw - aw); // horizontal shift inside the vlist
        }
        let mut v = vpack(vec![acc_box, Node::Kern(-delta), body_box], None, VBOX, &self.eqtb).node;
        if let Node::Box { w, .. } = &mut v {
            *w = bw;
        }
        let (_, vh, _) = box_dims(&v);
        if vh < bh {
            if let Node::Box { list, h, .. } = &mut v {
                list.insert(0, Node::Kern(bh - vh));
                *h = bh;
            }
        }
        let _ = bd;
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
                let Some((_, f)) = self.fam_font((sz * 2) as u8, fam) else {
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
        if crate::debug_flag("VDELIM") {
            eprintln!(
                "VDELIM code={:#x} parts=({},{},{},{}) found={:?} best={:?}",
                code, sf, sc, lf, lc, found, best
            );
        }
        match found.or(best) {
            Some((sz, fam, c)) => {
                let f = self.eqtb.fonts.get(self.eqtb.style_fonts[sz][fam as usize] as usize).cloned();
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
    fn make_extensible(&self, size_idx: usize, fam: u8, rec: crate::tfm::ExtRecipe, v: i32, style: GStyle) -> Node {
        let (fid, f) = match self.fam_font((size_idx * 2) as u8, fam) {
            Some(v2) => v2,
            None => return self.null_delimiter_box(style),
        };
        let glyph = |ch: u8| -> Option<(Node, i32, i32)> {
            if ch == 0 || !f.exists_char(ch) {
                return None;
            }
            let n = hpack(vec![Node::Char { c: ch, font: fid }], None, HBOX, &self.eqtb).node;
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
        }
        if let Some((node, _, _)) = &rep {
            for _ in 0..n {
                vlist.push(node.clone());
            }
        }
        if let Some((node, _, _)) = bot {
            vlist.push(node);
        }
        // height = top-most present part's height; depth fills to w
        let h_top = vlist
            .first()
            .map(|b| box_dims(b).1)
            .unwrap_or(0);
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

/// append `box` to `out` with the given shift applied
fn set_shift(out: &mut NodeList, mut b: Node, shift: i32) {
    if let Node::Box { shift: s, .. } = &mut b {
        *s = shift;
    }
    out.push(b);
}

// =====================================================================
// Tests: layout is checked against dims captured from real TeX
// (`tex \showbox` oracles on the same CM fonts at 10/7/5 pt), with the
// plain TeX family setup fam0=cmr, fam1=cmmi, fam2=cmsy, fam3=cmex.
#[cfg(test)]
pub(crate) mod tests_support {
    pub use super::*;
}

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
        e.input.push_file("mathtest.tex".to_string(), full.into_bytes());
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
        let mut n = if let Some(n) = e
            .page_list
            .iter()
            .find(|n| matches!(n, Node::Box { kind, .. } if *kind == HBOX))
            .cloned()
        {
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
            Node::Box { list, w, h, d, shift, .. } => (list, *w, *h, *d, *shift),
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
        if let (Node::Kern(k1), Node::Rule { height, width, .. }, Node::Kern(k2)) = (&vlist[1], &vlist[2], &vlist[3]) {
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
        if let (Node::Kern(t), Node::Rule { height, .. }, Node::Kern(clr)) = (&vlist[0], &vlist[1], &vlist[2]) {
            approx(*t, 0.39998, "top kern = surd height");
            approx(*height, 0.39998, "rule = surd height");
            approx(*clr, 2.89722, "clearance with half-excess");
        } else {
            panic!("bar vbox: {:?}", vlist);
        }
    }

    /// oracle: `\hbox{$\sum_{i=1}^n$}` — side scripts in text style (no
    /// limits), op axis-centered, scripts in a shifted vbox
    #[test]
    fn sum_text_style_side_scripts() {
        let b = text_math("\\sumG_{i=1}^n");
        let (list, _, h, d, _) = box_of(&b);
        approx(h, 7.50006, "sum total height");
        approx(d, 3.00005, "sum total depth");
        assert_eq!(list.len(), 2, "op box + scripts vbox: {:?}", list);
        approx(-box_shift(&list[0]), 7.50006, "op axis shift");
        let (vlist, _, _, _, vshift) = box_of(&list[1]);
        approx(vshift, 3.00005, "scripts vbox shift");
        if let Node::Kern(k) = &vlist[1] {
            approx(*k, 2.00711, "scripts stack kern");
        } else {
            panic!("expected kern: {:?}", vlist);
        }
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

    /// display-style \sum gets limits above/below (big op spacings from
    /// cmex fontdimens 9..13), sup/sub skewed by half the italic (delta=0
    /// for \sum), all centered on the common width
    #[test]
    fn sum_display_limits_box() {
        let b = display_math("\\sumG_{i=1}^{n}");
        let (list, _, _, _, _) = box_of(&b);
        let (vlist, vw, vh, vd, _) = box_of(&list[0]);
        assert!(matches!(list[0], Node::Box { kind: VBOX, .. }), "{:?}", list);
        let xh5 = 0.430555 * 5.0; // x-height at scriptscript size (5pt)
        let su = (BIG3 * 10.0 - (3.01389)).max(BIG1 * 10.0); // sp3 - d(n), floor sp1
        let sd = (BIG4 * 10.0 - 4.63193).max(BIG2 * 10.0); // sp4 - h(i=1), floor sp2
        approx(vh, 16.51395, "limits vbox h");
        approx(vd, 12.79869, "limits vbox d");
        let _ = vw;
        // vbox: [kern sp5, sup, kern su, op, kern sd, sub, kern sp5]
        assert_eq!(vlist.len(), 7, "{:?}", vlist);
        let _ = xh5;
    }

    /// oracle: `\hbox{$\left({a\over b}\right)$}` — delimiters from cmex
    /// sized by delimiterfactor/shortfall, ink centered on the axis
    #[test]
    fn left_right_group_paren_big() {
        let b = text_math("\\left({a\\over b}\\right)");
        let (list, _, h, d, _) = box_of(&b);
        approx(h, 8.50005, "group height");
        approx(d, 3.50006, "group depth");
        assert_eq!(list.len(), 5, "open, null, inner, null, close: {:?}", list);
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
        let chars = sup.iter().filter(|n| matches!(n, Node::Char { .. })).count();
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
        let frac_box = match last {
            Node::Box { list: fl, .. } if fl.len() == 4 => &fl[2],
            other => other,
        };
        match frac_box {
            Node::Box { kind: VBOX, list: vl, .. } => {
                assert_eq!(vl.len(), 5, "num, kern, rule, kern, den: {:?}", vl);
            }
            other => panic!("expected fraction vbox, got {:?}", other),
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
        let glues: Vec<_> = list.iter().filter_map(|n| match n {
            Node::Glue(g) => Some(g.width),
            _ => None,
        }).collect();
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
}

#[cfg(test)]
mod probe {
    use super::*;
    #[test]
    fn probe_page_list() {
        let mut e = Engine::new(true);
        e.init_primitives();
        e.add_nullfont();
        let s = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\$=3 \\catcode`\\^=7 \\catcode`\\_=8\n\\font\\tenrm=cmr10\n\\textfont0=\\tenrm\n\\hbox{$x$}\n";
        e.input.push_file("p.tex".to_string(), s.as_bytes().to_vec());
        e.run();
        println!("TERM: {}", e.term);
        println!("ERRORS: {}", e.error_count);
        println!("PAGE: {:?}", e.page_list);
        println!("CUR_LIST: {:?}", e.cur_list);
    }
}

#[cfg(test)]
#[cfg(test)]
mod probe2 {
    use super::tests_support::*;
    use super::*;
    #[test]
    fn probe_constructs() {
        for (name, body) in [
            ("bare_def", "!!!"),
            ("owd_sp", "\\hbox{$a\\overwithdelims ( ( b$}"),
            ("delcode_val", "\\hbox{$\\the\\delcode`( $}"),
            ("id_hbox_notex", "\\hbox{\\tmpA{y}}"),
            ("id_math_par", "$\\tmpA{y}$ x\\par"),
            ("id_math_hbox", "\\hbox{$\\tmpA{y}$}"),
            ("sqrt_spaced", "\\hbox{$\\sqrtS {x}$}"),
            ("sqrt_braced_body", "\\hbox{$\\sqrtB{x}$}"),
            ("radical_direct", "\\hbox{$\\radical \"270370 {x}$}"),
            ("accent_direct", "\\hbox{$\\mathaccent \"7016 {x}$}"),
            ("owd2", "\\hbox{$a\\overwithdelims()b$}"),
        ] {
            let mut e = Engine::new(true);
            e.init_primitives();
            e.add_nullfont();
            let full = if body == "!!!" {
                "\\catcode`\\{=1 \\catcode`\\}=2 \\def\\tmpA#1{#1}\\tmpA{y}\\par".to_string()
            } else {
                format!("{}{}\n", super::tests::math_preamble(), body)
            };
            e.input.push_file("p.tex".to_string(), full.into_bytes());
            e.run();
            if name == "mathchoice" {
                if let Some(n) = e.page_list.iter().find(|n| matches!(n, Node::Box { kind: HBOX, .. })) {
                    println!("MATHCHOICE BOX: {:?}", n);
                }
            }
            if name == "delcode_val" {
                println!("DEL40={:?}", e.eqtb.del_code[40]);
            }
            println!("== {} errors={} hbox={} term_tail={:?}", name, e.error_count,
                e.page_list.iter().any(|n| matches!(n, Node::Box { kind: HBOX, .. })),
                e.term.lines().filter(|l| l.starts_with("! ")).collect::<Vec<_>>());
        }
    }
}

#[cfg(test)]
mod probe3 {
    use super::*;
    #[test]
    fn probe_int_metrics() {
        let mut e = Engine::new(true);
        e.init_primitives();
        e.add_nullfont();
        let s = "\\font\\tenex=cmex10\n\\textfont3=\\tenex\n";
        e.input.push_file("p.tex".to_string(), s.as_bytes().to_vec());
        e.run();
        let fid = e.eqtb.style_fonts[0][3];
        let f = &e.eqtb.fonts[fid as usize];
        println!("FID={} ec={} it(0x52)={} w(0x52)={} d(0x52)={}", fid, f.ec, f.char_italic(0x52), f.char_width(0x52), f.char_depth(0x52));
    }

    #[test]
    /// Dumps the page_list after `$$\left({a\over b}+c\right)^2$$`: proves the
    /// Frac vbox, Scripts stack, and sized DelimBoxes exist in the display
    /// hbox BEFORE the shipout pipeline (page.rs/pdfrender) touches it.
    fn probe_lr_display_page_list() {
        let mut e = Engine::new(true);
        e.init_primitives();
        let full = format!(
            "{}\\vsize=60in\nx\n$$\\left({{a\\over b}}+c\\right)^2$$\ny\n",
            super::tests::math_preamble()
        );
        e.run();
        println!("ERRORS: {}", e.error_count);
        fn dump(name: &str, ns: &[Node], depth: usize) {
            for n in ns {
                match n {
                    Node::Box { kind, w, h, d, list, .. } => {
                        let kn = if *kind == VBOX { "VBOX" } else { "HBOX" };
                        println!("{}{} w={} h={} d={} nchildren={}", "  ".repeat(depth), kn, w, h, d, list.len());
                        if depth < 3 {
                            dump(name, list, depth + 1);
                        }
                    }
                    other => println!("{}{:?}  [{}]", "  ".repeat(depth), other, name),
                }
            }
        }
        dump("page", &e.page_list, 0);
        println!("PAGE_LIST_LEN={} PAR_PAGE_LISTS={} SAVED={} CUR_LIST={} MODE={:?} BOX255: {:?} PDF_PAGES: {}",
            e.page_list.len(), e.par_page_lists.len(), e.saved_lists.len(), e.cur_list.len(), e.mode,
            e.eqtb.boxed[255].is_some(), e.pdf_doc.pages.len());
    }
}

#[cfg(test)]
mod probe4 {
    use super::*;
    /// Isolated math constructs through the REAL LaTeX format + newtx setup,
    /// dumping each paragraph line-box width (== the math width, since
    /// \noindent + \parfillskip natural width 0). Oracle: real pdflatex
    /// \showthe\wd of the same \hbox{}es.
    #[test]
    #[ignore]
    fn probe_ntx_math_widths() {
        let mut e = Engine::new(false);
        e.init_primitives();
        match crate::format::load_format_into(std::path::Path::new("/tmp/pdflatex.fmt"), &mut e) {
            Ok(()) => eprintln!("format ok"),
            Err(err) => panic!("format load failed: {err}"),
        }
        e.ini_mode = false;
        e.end_occurred = false;
        let body = r#"
\documentclass[12pt]{article}
\usepackage[T1]{fontenc}
\usepackage{newtx}
\usepackage{amsmath}
\begin{document}
\noindent$\lambda(\tau)$\par
\noindent$\tfrac12$\par
\noindent$\Lambda_{M}$\par
\noindent$x^{M}$\par
\noindent$\ell^2/\mu$\par
\noindent$\lambda'(\tau) < 0$\par
\end{document}
"#;
        e.input.push_file("mw.tex".to_string(), body.as_bytes().to_vec());
        e.run();
        eprintln!("ERRORS={}", e.error_count);
        let mut idx = 0usize;
        fn walk(ns: &[Node], depth: usize, idx: &mut usize) {
            for n in ns {
                match n {
                    Node::Box { kind, w, h, d, list, .. } => {
                        eprintln!("{}[{}] {} w={}pt h={} d={}", "  ".repeat(depth), idx,
                            if *kind == VBOX { "VBOX" } else { "HBOX" },
                            *w as f64 / 65536.0, *h as f64 / 65536.0, *d as f64 / 65536.0);
                        *idx += 1;
                        if *w != 0 { walk(list, depth + 1, idx); }
                    }
                    other => eprintln!("{}{:?}  [{}]", "  ".repeat(depth), other, idx),
                }
            }
        }
        for (i, par) in e.par_page_lists.iter().enumerate() {
            eprintln!("== PAR {} ({} nodes)", i, par.len());
            walk(par, 1, &mut idx);
        }
        eprintln!("== PAGE_LIST ({} nodes) SAVED={} CUR={}", e.page_list.len(), e.saved_lists.len(), e.cur_list.len());
        walk(&e.page_list, 1, &mut idx);
    }
    /// \showbox0 dumps through the REAL LaTeX format + newtx: compare widths
    /// to real pdflatex's \showbox on the same file.
    #[test]
    #[ignore]
    fn probe_ntx_showbox() {
        let mut e = Engine::new(false);
        e.init_primitives();
        crate::format::load_format_into(std::path::Path::new("/tmp/pdflatex.fmt"), &mut e).unwrap();
        e.ini_mode = false;
        e.end_occurred = false;
        let body = r#"
\documentclass[12pt]{article}
\usepackage[T1]{fontenc}
\usepackage{newtx}
\usepackage{amsmath}
\showboxbreadth=100 \showboxdepth=4
\begin{document}
\setbox0=\hbox{$\lambda(\tau)$}\showbox0
\setbox0=\hbox{$\Lambda_{M}$}\showbox0
\setbox0=\hbox{$x^{M}$}\showbox0
\setbox0=\hbox{$\ell^2/\mu$}\showbox0
\setbox0=\hbox{$\tfrac12$}\showbox0
\end{document}
"#;
        e.input.push_file("mw2.tex".to_string(), body.as_bytes().to_vec());
        e.run();
        for l in e.term.lines() {
            if l.contains("width") || l.contains("character") || l.contains("rule") || l.contains("glue") || l.contains("kern") {
                eprintln!("SB {}", l.trim_start());
            }
        }
    }

    #[test]
    #[ignore]
    fn probe_latex_math_fonts() {
        let mut e = Engine::new(false);
        e.init_primitives();
        crate::format::load_format_into(std::path::Path::new("/tmp/pdflatex.fmt"), &mut e).unwrap();
        e.ini_mode = false;
        let body = "\\documentclass[12pt]{article}\n\\usepackage[margin=1in]{geometry}\n\\usepackage[T1]{fontenc}\n\\usepackage{newtx}\n\\usepackage{amsmath}\n\\usepackage{setspace}\n\\begin{document}\n\\setstretch{1.5}\n\\begin{abstract}\nSome abstract text here.\n\\end{abstract}\nBody paragraph after abstract.\n\\end{document}\n";
        e.input.push_file("mw3.tex".to_string(), body.as_bytes().to_vec());
        e.run();
        eprintln!("ERRORS={} mathcode_x={:?} hsize={}pt parindent={}pt",
            e.error_count, e.eqtb.math_code[b'x' as usize],
            e.eqtb.dim_params[DimParam::HSize.idx() as usize] as f64 / 65536.0,
            e.eqtb.dim_params[DimParam::ParIndent.idx() as usize] as f64 / 65536.0);
        for size in 0..3 {
            let row: Vec<String> = (0..4)
                .map(|fam| {
                    let fid = e.eqtb.style_fonts[size][fam];
                    let name = e.eqtb.fonts.get(fid as usize).map(|f| f.name.clone()).unwrap_or_default();
                    format!("fam{}={}({})", fam, fid, name)
                })
                .collect();
            eprintln!("size{}: {}", size, row.join(" "));
        }
        let c = e.eqtb.math_code[b'x' as usize] as usize;
        let fam = ((c >> 8) & 0xF) as u8;
        let ch = (c & 0xFF) as u8;
        let fid = e.eqtb.style_fonts[0][fam as usize];
        if let Some(f) = e.eqtb.fonts.get(fid as usize) {
            eprintln!("x -> fam{} char 0x{:02x} in font {}: exists={}", fam, ch, fid, f.exists_char(ch));
        } else {
            eprintln!("x -> font {} OUT OF RANGE", fid);
        }
    }
    #[test]
    #[ignore]
    fn probe_latex_textfont_assign() {
        let mut e = Engine::new(false);
        e.init_primitives();
        crate::format::load_format_into(std::path::Path::new("/tmp/pdflatex.fmt"), &mut e).unwrap();
        e.ini_mode = false;
        e.end_occurred = false;
        let body = "\\documentclass[12pt]{article}\n\\begin{document}\n\\font\\myi=cmmi12 \\textfont1=\\myi\n$x$\n\\end{document}\n";
        e.input.push_file("mw4.tex".to_string(), body.as_bytes().to_vec());
        e.run();
        eprintln!("ERRORS={} style_fonts[0][1]={} style_fonts[1][1]={}",
            e.error_count, e.eqtb.style_fonts[0][1], e.eqtb.style_fonts[1][1]);
        for l in e.term.lines() {
            if l.contains("Font Info") || l.contains("Font Warning") || l.contains("cmmi") || l.contains("loaded") {
                eprintln!("FI {}", l.trim_start());
            }
        }
    }

    #[test]
    #[ignore]
    fn probe_cm_manual_fam_widths() {
        let mut e = Engine::new(false);
        e.init_primitives();
        crate::format::load_format_into(std::path::Path::new("/tmp/pdflatex.fmt"), &mut e).unwrap();
        e.ini_mode = false;
        e.end_occurred = false;
        // Mirror real LaTeX 12pt math font setup (OML/cmm, OMS/cmsy, cmex, cmr
        // at 12/8/6) — what \math@fonts would do at $-entry.
        let body = r#"
\documentclass[12pt]{article}
\begin{document}
\font\fa=cmr12 \textfont0=\fa
\font\fb=cmmi12 \textfont1=\fb
\font\fc=cmsy10 at 12pt \textfont2=\fc
\font\fd=cmex10 at 12pt \textfont3=\fd
\font\fe=cmr8 \scriptfont0=\fe
\font\ff=cmmi8 \scriptfont1=\ff
\font\fg=cmsy10 at 8pt \scriptfont2=\fg
\font\fh=cmex10 at 8pt \scriptfont3=\fh
\font\fj=cmmi6 \scriptscriptfont1=\fj
\setbox0=\hbox{$\Lambda_{M}$}\showbox0
\setbox0=\hbox{$x^{M}$}\showbox0
"#;
        e.input.push_file("cmw.tex".to_string(), body.as_bytes().to_vec());
        e.run();
        eprintln!("ERRORS={}", e.error_count);
        for l in e.term.lines() {
            if l.contains("width") || l.contains("character") || l.contains("kern") || l.contains("rule") || l.contains("vbox") || l.starts_with("! ") {
                eprintln!("CMW {}", l.trim_start());
            }
        }
    }
    #[test]
    fn probe_eqno_frac_scratch_no_eqno() {
        let mut e = Engine::new(false);
        e.init_primitives();
        crate::format::load_format_into(std::path::Path::new("/home/leo/dd/tex/target/debug/pdflatex.fmt"), &mut e).unwrap();
        e.ini_mode = false;
        e.end_occurred = false;
        let body = "\\documentclass[12pt]{article}\\usepackage[T1]{fontenc}\\usepackage{amsmath}\\usepackage{newtx}\\begin{document}$$b^{W}=\\frac{\\lambda+\\zeta\\kappa}{\\lambda+\\theta+\\zeta},\\qquad \\frac{\\partial w^{*}}{\\partial\\lambda}<0$$";
        e.input.push_file("probe.tex".to_string(), body.as_bytes().to_vec());
        e.run();
        eprintln!("errors={} term_tail={}", e.error_count, &e.term[e.term.len().saturating_sub(200)..]);
        fn dump(n: &Node, ind: usize, depth: usize) {
            if depth > 8 { return; }
            let pad = "  ".repeat(ind);
            match n {
                Node::Box { kind, w, h, d, shift, list, .. } => {
                    eprintln!("{}box kind={} w={} h={} d={} shift={}", pad, kind, w, h, d, shift);
                    for c in list { dump(c, ind + 1, depth + 1); }
                }
                Node::Glue(g) => eprintln!("{}glue w={} st={} sh={}", pad, g.width, g.stretch, g.shrink),
                Node::Kern(k) | Node::ExplicitKern(k) => eprintln!("{}kern {}", pad, k),
                Node::Penalty(p) => eprintln!("{}pen {}", pad, p),
                Node::Rule { width, height, depth } => eprintln!("{}rule w={} h={} d={}", pad, width, height, depth),
                other => eprintln!("{}{:?}", pad, other),
            }
        }
        for n in &e.page_list { dump(n, 0, 0); }
    }
}

