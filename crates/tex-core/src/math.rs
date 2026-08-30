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
use crate::prim::{DimParam, IntParam, Prim};
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

/// font table index (text/script/scriptscript) for a style: D,T -> 0; S -> 1; SS -> 2
#[inline]
fn font_size(g: GStyle) -> usize {
    match g {
        0 | 1 | 2 | 3 => 0,
        4 | 5 => 1,
        _ => 2,
    }
}

/// numerator style: cramped at the same level (tex.web §738)
#[inline]
fn num_style(g: GStyle) -> GStyle {
    g | 1
}

/// denominator style: next level up (tex.web §738)
#[inline]
fn den_style(g: GStyle) -> GStyle {
    (g & !1) + 2
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

/// tex.web §761 "magic" spacing table. Rows: left class, cols: right class.
/// Values: 0 none, 1 thin, 2 med, 3 thick, 4 = forbidden (the bin is demoted
/// to ord before the table is consulted, so these cells cannot survive).
const SPACING: [[u8; 8]; 8] = [
    //         ord op bin rel open close punct inner
    /* ord  */ [0, 1, 2, 3, 0, 0, 0, 2],
    /* op   */ [1, 1, 4, 3, 0, 0, 0, 2],
    /* bin  */ [2, 2, 4, 4, 2, 4, 4, 2],
    /* rel  */ [3, 3, 4, 0, 3, 0, 0, 3],
    /* open */ [0, 1, 4, 3, 0, 0, 0, 2],
    /* close*/ [0, 1, 4, 3, 0, 0, 0, 2],
    /* punct*/ [0, 1, 4, 3, 0, 0, 0, 2],
    /* inner*/ [2, 2, 4, 3, 2, 0, 2, 2],
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
    (
        ((code >> 20) & 0xFF) as u8,
        ((code >> 12) & 0xFF) as u8,
        ((code >> 8) & 0xF) as u8,
        (code & 0xFF) as u8,
    )
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
        if self.mode == Mode::DisplayMath {
            return;
        }
        let display = self.mode.is_v();
        if display {
            self.eqtb.push_level(crate::eqtb::LevelType::Group);
            let page = std::mem::take(&mut self.page_list);
            self.saved_lists.push((
                self.mode,
                std::mem::take(&mut self.cur_list),
                self.prev_depth,
                self.space_factor,
            ));
            self.par_page_lists.push(page);
            self.mode = Mode::DisplayMath;
        } else {
            self.saved_lists.push((
                self.mode,
                std::mem::take(&mut self.cur_list),
                self.prev_depth,
                self.space_factor,
            ));
            self.eqtb.push_level(crate::eqtb::LevelType::Group);
            self.mode = Mode::Math;
        }
        self.math_lists.push(crate::boxes::NodeList::new());
        self.left_delim = None;
        self.right_delim = None;
        self.math_limits = None;
        self.run_everymath();
    }

    fn run_everymath(&mut self) {
        let toks = (*self.eqtb.tok_params[crate::prim::ToksParam::EveryMath.idx() as usize]).clone();
        if !toks.is_empty() {
            self.input.push_toks(toks, "<everymath>");
        }
    }

    pub fn exit_math(&mut self) {
        let mlist = self.math_lists.pop().unwrap_or_default();
        let was_display = self.mode == Mode::DisplayMath;
        self.eqtb.pop_level();
        let (outer_mode, outer_list, pd, sf) = self.saved_lists.pop().unwrap_or((self.mode, std::mem::take(&mut self.cur_list), self.prev_depth, self.space_factor));
        // leak guards: state set inside math must not escape it
        self.left_delim = None;
        self.right_delim = None;
        self.math_limits = None;
        let g: GStyle = if was_display { 0 } else { 2 };
        let hlist = self.mlist_to_hlist(&mlist, g);
        let packed = hpack(hlist, None, HBOX, &self.eqtb);
        let hbox = packed.node;
        self.mode = outer_mode;
        self.prev_depth = pd;
        self.space_factor = sf;
        self.cur_list = outer_list;
        match self.mode {
            Mode::Horizontal => {
                self.cur_list.push(hbox);
                self.space_factor = 1000;
            }
            Mode::Vertical | Mode::InternalVertical => {
                if was_display {
                    self.finish_display_math(hbox);
                } else {
                    self.vlist_append(hbox);
                }
            }
            _ => {
                self.cur_list.push(hbox);
            }
        }
        if was_display {
            if let Some(page) = self.par_page_lists.pop() {
                let _ = page;
            }
        }
    }

    fn finish_display_math(&mut self, hbox: Node) {
        let above = self.eqtb.glue_params[crate::prim::GlueParam::AboveDisplaySkip.idx() as usize].clone();
        let below = self.eqtb.glue_params[crate::prim::GlueParam::BelowDisplaySkip.idx() as usize].clone();
        if let Some(page) = self.par_page_lists.pop() {
            let mut page = page;
            page.push(Node::Glue(above));
            page.push(Node::Penalty(-10000));
            page.push(hbox);
            page.push(Node::Glue(below));
            self.page_list = page;
            self.mode = Mode::Vertical;
            self.cur_list = Vec::new();
            self.build_page();
        }
    }

    pub fn append_mlist_node(&mut self, n: Node) {
        if let Some(l) = self.math_lists.last_mut() {
            l.push(n);
        } else {
            self.error("Math node outside math mode");
        }
    }

    pub fn append_mathchar(&mut self, mc: u16) {
        let class = (mc >> 12) as u8;
        let fam = ((mc >> 8) & 0xF) as u8;
        let c = (mc & 0xFF) as u8;
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
        self.eqtb.pop_level();
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
        let group = self.scan_math_group_or_token();
        let limits_req = self.math_limits.take();
        let popped = self.math_lists.last_mut().and_then(|l| l.pop());
        let Some(top) = popped else {
            self.error("Missing { inserted for subscript");
            return;
        };
        match top {
            Node::Scripts { nucleus, sup: s, sub: x } => {
                let (ns, nx) = if sup { (Some(group), x) } else { (s, Some(group)) };
                self.append_mlist_node(Node::Scripts { nucleus, sup: ns, sub: nx });
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
    /// None=default (large operator in D/T styles gets limits).
    fn script_wants_limits(&self, req: Option<u8>, atom: &Node) -> bool {
        let is_op = match atom {
            Node::MathChar { class, .. } => *class == CL_OP,
            Node::DelimBox { size: 2, .. } => true,
            _ => false,
        };
        match req {
            Some(0) => false,
            Some(1) => true,
            Some(2) => {
                let g = gstyle_of(self.cur_math_style());
                g < 4
            }
            _ => {
                let g = gstyle_of(self.cur_math_style());
                is_op && g < 4
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
        let mut depth: usize = 1;
        loop {
            let t = self.get_token();
            if t == crate::input::EOF_MARKER {
                self.error("Missing } in math group");
                break;
            }
            if t.is_char() && t.cc() == 2 {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                // nested close inside a nested group cannot happen here
                continue;
            }
            if t.is_char() && t.cc() == 1 {
                // nested braced group: an Ord atom holding the packed contents
                let inner = self.scan_math_group_braced();
                let g = gstyle_of(self.cur_math_style());
                let nodes = self.mlist_to_hlist(&inner, g);
                let boxed = hpack(nodes, None, HBOX, &self.eqtb).node;
                self.append_mlist_node(boxed);
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
            let d = self.eqtb.del_code[c as usize];
            if d >= 0 {
                return d;
            }
            self.error("Missing delimiter (. inserted)");
            return 0;
        }
        if let Some(Equiv::Prim(Prim::Delimiter)) = self.eqtb.resolve(t.cs_id()).cloned() {
            let v = self.scan_int();
            if v < 0 || v >= 0x4000000 {
                self.error("Invalid delimiter code");
                return 0;
            }
            return v;
        }
        // tex.web: back_input, then scan a 27-bit integer constant
        self.pushed.push(t);
        let v = self.scan_int();
        if v < 0 || v >= 0x4000000 {
            self.error("Invalid delimiter code");
            return 0;
        }
        v
    }

    pub fn do_fraction(&mut self, p: Prim) {
        // numerator = everything accumulated in the current math list so far
        let num = self.math_lists.pop().unwrap_or_default();
        self.math_lists.push(Vec::new());
        // tex.web: the denominator is the rest of the mlist; here it is the
        // next group-or-token (matches \frac{..}{..} usage).
        let den = self.scan_math_group_or_token();
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
        let left = if ld > 0 {
            Some((((ld >> 20) & 0xFF) as u8, ((ld >> 12) & 0xFF) as u8))
        } else {
            None
        };
        let right = if rd > 0 {
            Some((((rd >> 20) & 0xFF) as u8, ((rd >> 12) & 0xFF) as u8))
        } else {
            None
        };
        self.append_mlist_node(Node::Frac {
            num,
            den,
            thickness,
            left,
            right,
        });
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

    fn skew_char_of(&self, fid: FontId, f: &Font) -> i32 {
        let v = self.eqtb.skew_char.get(fid as usize).copied().unwrap_or(-1);
        if v >= 0 {
            v
        } else {
            f.skew_char
        }
    }

    // ---------- the conversion itself ----------

    /// public entry: convert a raw math list produced at `style`
    pub fn math_to_hlist(&self, list: &[Node], style: MathStyle) -> NodeList {
        self.mlist_to_hlist(list, gstyle_of(style))
    }

    /// classify a raw node as a spacing atom; None = not an atom
    fn atom_class(&self, n: &Node) -> Option<u8> {
        match n {
            // mathcode class 7 = variable: spaced as ord (tex.web §759)
            Node::MathChar { class, .. } => Some(if *class == 7 { CL_ORD } else { *class }),
            Node::Scripts { nucleus, .. } => Some(match nucleus.first() {
                Some(Node::MathChar { class, .. }) => if *class == 7 { CL_ORD } else { *class },
                _ => CL_ORD,
            }),
            Node::OpLimits { .. } => Some(CL_OP),
            Node::Frac { .. } => Some(CL_INNER),
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

    fn mlist_to_hlist(&self, list: &[Node], start: GStyle) -> NodeList {
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
                    let nodes = self.mlist_to_hlist(chosen, style);
                    self.emit_atom(&mut out, &mut prev, Some(CL_ORD), nodes);
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
                if let Node::DelimBox { size: 1, small, large } = n {
                    let code = delim_code_of(*small, *large);
                    match lr_stack.pop() {
                        Some((lopen, buf)) => {
                            let body = self.mlist_to_hlist(&buf, style);
                            let (bw, bh, bd) = hlist_dims(&body, &self.eqtb);
                            let mut needed = bh + bd;
                            let floor = if style < 4 {
                                self.fparam(style, 2, 20)
                            } else {
                                self.fparam(style, 2, 21)
                            };
                            if floor > needed {
                                needed = floor;
                            }
                            let mut assembled: NodeList = self.var_delimiter(lopen, needed, style);
                            assembled.extend(body);
                            assembled.extend(self.var_delimiter(code, needed, style));
                            style = after_lr_style(style);
                            match lr_stack.last_mut() {
                                // nested boundary: splice into the enclosing buffer
                                Some((_, pbuf)) => pbuf.extend(assembled),
                                None => {
                                    self.emit_atom(&mut out, &mut prev, Some(CL_INNER), assembled);
                                }
                            }
                        }
                        None => {
                            // stray close: a normal close delimiter
                            self.emit_atom(&mut out, &mut prev, Some(CL_CLOSE), self.var_delimiter(code, 0, style));
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
                if let Node::Style(s) = n {
                    style = gstyle_of(*s);
                    i += 1;
                    continue;
                }
                // spacing before this atom
                self.insert_spacing(&mut out, prev, Some(cls));
                // line-break penalties tied to the left atom (tex.web §766)
                if prev == Some(CL_BIN) {
                    let p = self.eqtb.int_params[IntParam::BinOpPenalty.idx() as usize];
                    out.push(Node::Penalty(p));
                } else if prev == Some(CL_REL) {
                    let p = self.eqtb.int_params[IntParam::RelPenalty.idx() as usize];
                    out.push(Node::Penalty(p));
                }
                let nodes = self.convert_atom(n, style);
                out.extend(nodes);
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
        // unclosed \left: flush buffers innermost-first
        while let Some((lopen, buf)) = lr_stack.pop() {
            let body = self.mlist_to_hlist(&buf, style);
            let (bw, bh, bd) = hlist_dims(&body, &self.eqtb);
            out.extend(self.var_delimiter(lopen, bh + bd, style));
            out.extend(body);
        }
        out
    }

    fn emit_atom(&self, out: &mut NodeList, prev: &mut Option<u8>, cls: Option<u8>, nodes: NodeList) {
        self.insert_spacing(out, *prev, cls);
        out.extend(nodes);
        *prev = cls;
    }

    fn insert_spacing(&self, out: &mut NodeList, prev: Option<u8>, cur: Option<u8>) {
        let (Some(a), Some(b)) = (prev, cur) else {
            return;
        };
        let kind = SPACING[(a.min(7)) as usize][(b.min(7)) as usize];
        let mu = self.mu_unit();
        let w = match kind {
            1 => mu * 3,  // thinmuskip = 3mu
            2 => mu * 4,  // medmuskip = 4mu
            _ => mu * 5,  // thickmuskip = 5mu
        };
        if w > 0 {
            out.push(Node::Kern(w));
        }
    }

    fn run_math_token(&mut self, t: Token) {
        if t.is_cs() {
            match self.eqtb.resolve(t.cs_id()).cloned() {
                Some(Equiv::Prim(Prim::Delimiter)) => {
                    let v = self.scan_int();
                    if v < 0 || v >= 0x4000000 {
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
            Node::MathChar { fam, c, .. } => {
                let mut out: NodeList = Vec::new();
                if let Some((_, f)) = self.fam_font(style, *fam) {
                    if f.exists_char(*c) {
                        out.push(Node::Char { c: *c, font: self.eqtb.style_fonts[font_size(style)][*fam as usize] });
                    }
                }
                out
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

    fn make_scripts(&self, nucleus: &[Node], sup: Option<&[Node]>, sub: Option<&[Node]>, style: GStyle) -> NodeList {
        let nuc_nodes = self.mlist_to_hlist(nucleus, style);
        let nuc = hpack(nuc_nodes, None, HBOX, &self.eqtb).node;
        let (nw, nh, nd) = box_dims(&nuc);
        let x = self.math_x_height(style);
        let rt = self.default_rule_thickness(style);
        // italic correction of a single-character nucleus (tex.web §745)
        let delta = match nucleus {
            [Node::MathChar { fam, c, .. }] => match self.fam_font(style, *fam) {
                Some((_, f)) if f.exists_char(*c) => f.char_italic(*c),
                _ => 0,
            },
            _ => 0,
        };
        let cramped = style & 1 == 1;
        let scripted = style >= 4;
        let sup1 = self.fparam(style, 2, 13);
        let sup2 = self.fparam(style, 2, 14);
        let sup3 = self.fparam(style, 2, 15);
        let sub1 = self.fparam(style, 2, 16);
        let sub2 = self.fparam(style, 2, 17);
        let sup_drop: i32 = if scripted { x } else { 0 };
        let sub_drop: i32 = if scripted { x } else { 0 };

        let mut out: NodeList = vec![nuc];
        // italic-correction kerns (approximation of the §746 corner rules)
        if delta != 0 {
            let k = match (sup, sub) {
                (Some(_), Some(_)) => delta / 2,
                _ => delta,
            };
            if k != 0 {
                out.push(Node::Kern(k));
            }
        }
        let ss = self.eqtb.dim_params[DimParam::ScriptSpace.idx() as usize];
        if ss != 0 {
            out.push(Node::Kern(ss));
        }
        let mut shift_up = 0i32;
        let mut shift_down = 0i32;
        let mut sup_box: Option<Node> = None;
        let mut sub_box: Option<Node> = None;
        if let Some(s) = sup {
            let nodes = self.mlist_to_hlist(s, sup_style(style));
            let b = hpack(nodes, None, HBOX, &self.eqtb).node;
            let (_, bh, bd) = box_dims(&b);
            let mut su = nh - sup_drop;
            let sbase = if scripted {
                if cramped {
                    sup2
                } else {
                    sup3
                }
            } else if cramped {
                sup2
            } else {
                sup1
            };
            if su < sbase {
                su = sbase;
            }
            if su < bh + x * 4 / 5 {
                su = bh + x * 4 / 5;
            }
            shift_up = su;
            sup_box = Some(b);
            let _ = bd;
        }
        if let Some(s) = sub {
            let nodes = self.mlist_to_hlist(s, sub_style(style));
            let b = hpack(nodes, None, HBOX, &self.eqtb).node;
            let (_, bh, _) = box_dims(&b);
            let mut sd = nd + sub_drop;
            let sbase = if scripted { sub2 } else { sub1 };
            if sd < sbase {
                sd = sbase;
            }
            if sd < bh + x * 4 / 5 {
                sd = bh + x * 4 / 5;
            }
            shift_down = sd;
            sub_box = Some(b);
        }
        // clearance between stacked super/sub scripts (tex.web §746)
        if let (Some(bx), Some(by)) = (&sup_box, &sub_box) {
            let (_, bh, bd) = box_dims(bx);
            let (_, ch, _) = box_dims(by);
            let z = rt * 4;
            let gap = (shift_up - bd) - (ch - shift_down);
            if gap < z {
                let need = z - gap;
                shift_up += (need + 1) / 2;
                shift_down += need / 2;
                let _ = bh;
            }
        }
        match (sup_box, sub_box) {
            (Some(mut bs), Some(mut bb)) => {
                // both scripts: stacked in a vbox, baseline = sub's baseline
                // (tex.web §746; oracle-verified kern and shift)
                let (_, sd_, _) = box_dims(&bs);
                let (ch_, _, _) = box_dims(&bb);
                let k = (shift_up - sd_) + (shift_down - ch_);
                let mut vlist: NodeList = vec![bs];
                if k > 0 {
                    vlist.push(Node::Kern(k));
                }
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
        let _ = (nw, delta);
        out
    }

    // ---------- make_op with limits (tex.web §744) ----------

    fn make_op_limits(&self, op: &[Node], above: Option<&[Node]>, below: Option<&[Node]>, style: GStyle, _force: bool) -> NodeList {
        let op_nodes = self.mlist_to_hlist(op, style);
        let mut op_box = hpack(op_nodes, None, HBOX, &self.eqtb).node;
        let big_op = match op {
            [Node::MathChar { class, .. }] => *class == CL_OP,
            [Node::DelimBox { size: 2, .. }] => true,
            _ => false,
        };
        if big_op {
            // tex.web §744 (oracle-verified): ink center on the math axis
            let (_, oh, od) = box_dims(&op_box);
            if let Node::Box { shift: sh, .. } = &mut op_box {
                *sh = (oh - od) / 2 - self.axis_height(style);
            }
        }
        if above.is_none() && below.is_none() {
            return vec![op_box];
        }
        let (_, oh, od) = box_dims(&op_box);
        let x = self.math_x_height(style);
        let mut w = self.box_w(&op_box);
        let sup_nodes = above.map(|s| self.mlist_to_hlist(s, sup_style(style)));
        let sub_nodes = below.map(|s| self.mlist_to_hlist(s, sub_style(style)));
        let sup_box = sup_nodes.map(|n| hpack(n, None, HBOX, &self.eqtb).node);
        let sub_box = sub_nodes.map(|n| hpack(n, None, HBOX, &self.eqtb).node);
        let mut su = 0i32;
        let mut sd = 0i32;
        if let Some(b) = &sup_box {
            let (_, bh, bd) = box_dims(b);
            w = w.max(bh.max(bd).max(self.box_w(b)));
            if big_op {
                su = oh - x / 2 + bd;
            } else {
                let sup1 = self.fparam(style, 2, 13);
                su = oh - if style >= 4 { x } else { 0 };
                if su < sup1 {
                    su = sup1;
                }
                if su < bd + x * 4 / 5 {
                    su = bd + x * 4 / 5;
                }
            }
        }
        if let Some(b) = &sub_box {
            let (_, bh, bd) = box_dims(b);
            w = w.max(bh.max(bd).max(self.box_w(b)));
            if big_op {
                sd = od + x / 2 + bh;
            } else {
                let sub1 = self.fparam(style, 2, 16);
                sd = od;
                if sd < sub1 {
                    sd = sub1;
                }
                if sd < bh + x * 4 / 5 {
                    sd = bh + x * 4 / 5;
                }
            }
        }
        // clearance when both scripts are present (corner kerns omitted)
        if let (Some(bx), Some(by)) = (&sup_box, &sub_box) {
            let (_, _, bxd) = box_dims(bx);
            let (_, bxh, _) = box_dims(by);
            let gap = (su - bxd) - (bxh - sd);
            if gap < 0 {
                su += (-gap + 1) / 2;
                sd += (-gap) / 2;
            }
        }
        let mut vlist: NodeList = Vec::new();
        if let Some(b) = &sup_box {
            let (_, _, bd) = box_dims(b);
            let centered = self.center_to_w(b.clone(), w);
            vlist.push(centered);
            let k = su - oh - bd;
            if k != 0 {
                vlist.push(Node::Kern(k));
            }
        }
        let op_centered = if self.box_w(&op_box) == w {
            op_box.clone()
        } else {
            self.center_to_w(op_box.clone(), w)
        };
        vlist.push(op_centered);
        if let Some(b) = &sub_box {
            let (_, bh, _) = box_dims(b);
            let k = sd - bh - od;
            if k != 0 {
                vlist.push(Node::Kern(k));
            }
            let centered = self.center_to_w(b.clone(), w);
            vlist.push(centered);
        }
        let packed = vpack(vlist, None, VBOX, &self.eqtb).node;
        vec![packed]
    }

    fn center_to_w(&self, b: Node, w: i32) -> Node {
        if self.box_w(&b) == w {
            return b;
        }
        hpack(
            vec![Node::Glue(Glue::fil(0, ONE)), b, Node::Glue(Glue::fil(0, ONE))],
            Some(w),
            HBOX,
            &self.eqtb,
        )
        .node
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

    fn make_fraction(&self, num: &[Node], den: &[Node], thickness: i32, left: Option<(u8, u8)>, right: Option<(u8, u8)>, style: GStyle) -> NodeList {
        let num_nodes = self.mlist_to_hlist(num, num_style(style));
        let den_nodes = self.mlist_to_hlist(den, den_style(style));
        let num_box = hpack(num_nodes, None, HBOX, &self.eqtb).node;
        let den_box = hpack(den_nodes, None, HBOX, &self.eqtb).node;
        let (_, nh, nd) = box_dims(&num_box);
        let (_, dh, dd) = box_dims(&den_box);
        let w = self.box_w(&num_box).max(self.box_w(&den_box));
        let num_c = self.center_to_w(num_box, w);
        let den_c = self.center_to_w(den_box, w);

        let r = if thickness < 0 {
            self.default_rule_thickness(style)
        } else {
            thickness
        };
        // axis positioning
        let mut su = if style < 4 {
            self.fparam(style, 2, 8) // num1
        } else {
            self.fparam(style, 2, 9) // num2
        };
        let mut sd = if style < 4 {
            self.fparam(style, 2, 11) // denom1
        } else {
            self.fparam(style, 2, 12) // denom2
        };
        if su < nd {
            su = nd;
        }
        if sd < dh {
            sd = dh;
        }
        let axis = self.axis_height(style);
        let mut vlist: NodeList;
        let mut shift: i32;
        if r == 0 {
            // \atop: no rule; equalize the gaps around the axis
            let a = su - nd;
            let b = sd - dh;
            let z = a - b;
            if z > 0 {
                su -= z / 2;
                sd += z - z / 2;
            } else if z < 0 {
                sd -= (-z) / 2;
                su += -z - ((-z) / 2);
            }
            let k = (su - nd) + (sd - dh);
            vlist = vec![num_c, Node::Kern(k), den_c];
            shift = k + dh - su - axis;
        } else {
            // clearance so num/den do not touch the rule
            let a = su - nd;
            let b = sd - dh;
            if a < r {
                su += r - a;
            }
            if b < r {
                sd += r - b;
            }
            let k1 = su - nd - r / 2;
            let k2 = sd - dh - r / 2;
            vlist = vec![
                num_c,
                Node::Kern(k1),
                Node::Rule {
                    width: w,
                    height: r,
                    depth: 0,
                },
                Node::Kern(k2),
                den_c,
            ];
            shift = k2 + r / 2 - axis;
            if k1 < 0 {
                // degenerate: pull the rule up instead of overlapping the numerator
                vlist[1] = Node::Kern(0);
                shift += k1;
            }
        }
        let mut packed = vpack(vlist, None, VBOX, &self.eqtb).node;
        if let Node::Box { shift: s, .. } = &mut packed {
            *s = shift;
        }
        // delimiter pairs (withdelims): sized to the finished fraction
        let lcode = pair_to_code(left);
        let rcode = pair_to_code(right);
        let (bw, bh, bd) = box_dims(&packed);
        let mut out: NodeList = Vec::new();
        if lcode > 0 {
            out.extend(self.var_delimiter(lcode, bh + bd, style));
        }
        out.push(packed);
        if rcode > 0 {
            out.extend(self.var_delimiter(rcode, bh + bd, style));
        }
        let _ = (bw, nh, dd);
        out
    }

    // ---------- make_radical (tex.web §752-753) ----------

    fn make_radical(&self, body: &[Node], thickness_enc: i32, style: GStyle) -> NodeList {
        let (code, explicit_r) = unpack_radical(thickness_enc);
        let body_nodes = self.mlist_to_hlist(body, style);
        let body_box = hpack(body_nodes, None, HBOX, &self.eqtb).node;
        let (_, bh, bd) = box_dims(&body_box);
        let rt = if explicit_r >= 0 {
            explicit_r
        } else {
            self.default_rule_thickness(style)
        };
        let x = self.math_x_height(style);
        let clearance = if bh < x { x / 4 } else { rt / 3 };
        // inner vbox: bar over the body, baseline = body baseline
        let vlist: NodeList = vec![
            Node::Rule {
                width: self.box_w(&body_box),
                height: rt,
                depth: 0,
            },
            Node::Kern(clearance),
            body_box,
        ];
        let inner = vpack(vlist, None, VBOX, &self.eqtb).node;
        if code <= 0 {
            return vec![inner];
        }
        // surd covers from the baseline up to (and past) the bar top
        let needed = bh + clearance + rt;
        let surd_box = match self.var_delimiter(code, needed, style).into_iter().next() {
            Some(b @ Node::Box { .. }) => b,
            Some(other) => hpack(vec![other], None, HBOX, &self.eqtb).node,
            None => return vec![inner],
        };
        let (_, sh, _) = box_dims(&surd_box);
        let mut sb = surd_box;
        if let Node::Box { shift, .. } = &mut sb {
            *shift = -(bh + clearance);
        }
        let sw = self.box_w(&sb);
        let inset = sw / 8;
        let outer = hpack(vec![sb, Node::Kern(-(sw - inset)), inner], None, HBOX, &self.eqtb).node;
        let _ = sh;
        vec![outer]
    }

    // ---------- make_accent (tex.web §747-753) ----------

    fn make_accent(&self, accent: (u8, FontId), body: &[Node], style: GStyle) -> NodeList {
        let body_nodes = self.mlist_to_hlist(body, style);
        let body_box = hpack(body_nodes, None, HBOX, &self.eqtb).node;
        let (bw, bh, _) = box_dims(&body_box);
        let (ac, afid) = accent;
        let Some(af) = self.eqtb.fonts.get(afid as usize).cloned() else {
            return vec![body_box];
        };
        let aw = af.char_width(ac);
        let ax = {
            let v = self.eqtb.font_params.get(afid as usize).and_then(|v| v.get(4)).copied();
            v.unwrap_or_else(|| af.x_height())
        };
        // skew: kern between the body char and the skewchar of its font
        let mut skew = 0i32;
        if let [Node::MathChar { fam, c, .. }] = body {
            if let Some((bfid, bf)) = self.fam_font(style, *fam) {
                if bf.exists_char(*c) {
                    let sk = self.skew_char_of(bfid, &bf);
                    skew = self.char_kern(bfid, &bf, *c, sk as u8);
                }
            }
        }
        let acc_box = hpack(vec![Node::Char { c: ac, font: afid }], None, HBOX, &self.eqtb).node;
        let mut acc_box = acc_box;
        let vshift = (bh - ax).max(0);
        if let Node::Box { shift, .. } = &mut acc_box {
            *shift = vshift;
        }
        let dx = (bw - aw) / 2 + skew;
        let out = hpack(vec![acc_box, Node::Kern(dx - aw), body_box], None, HBOX, &self.eqtb).node;
        vec![out]
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

    /// Choose a delimiter tall enough for `needed` (height+depth). Returns the
    /// nodes to place in an hlist: a shifted box (single char or extensible
    /// stack), or a null-delimiter kern.
    fn var_delimiter(&self, code: i32, needed: i32, style: GStyle) -> NodeList {
        if code <= 0 {
            let nd = self.eqtb.dim_params[DimParam::NullDelimiterSpace.idx() as usize];
            return vec![Node::Kern(nd)];
        }
        let (sf, sc, lf, lc) = delim_code_parts(code);
        let shortfall = self.eqtb.dim_params[DimParam::DelimiterShortfall.idx() as usize];
        let mut factor = self.eqtb.int_params[IntParam::DelimiterFactor.idx() as usize] as i64;
        if factor <= 0 {
            factor = 901;
        }
        let mut best: Option<(u8, u8, i32)> = None;
        let mut fam = sf;
        let mut c = sc;
        let mut moved_to_large = false;
        let mut steps = 0usize;
        loop {
            steps += 1;
            if steps > 256 {
                break;
            }
            if c != 0 {
                if let Some((_, f)) = self.fam_font(style, fam) {
                    if f.exists_char(c) {
                        let a = f.char_height(c) + f.char_depth(c);
                        if a >= needed + shortfall || (a as i64 * 1000) >= (needed as i64) * factor {
                            return self.delim_char_nodes(fam, c, style);
                        }
                        if best.map(|(_, _, ba)| a > ba).unwrap_or(true) {
                            best = Some((fam, c, a));
                        }
                        let ci = &f.chars[c as usize];
                        if ci.tag == TAG_LIST {
                            let next = ci.remainder;
                            if next != c {
                                c = next;
                                continue;
                            }
                        } else if ci.tag == TAG_EXT {
                            return self.ext_delimiter(fam, c, needed, style);
                        }
                    }
                }
            }
            if !moved_to_large && lc != 0 {
                fam = lf;
                c = lc;
                moved_to_large = true;
                continue;
            }
            break;
        }
        match best {
            Some((f, ch, _)) => self.delim_char_nodes(f, ch, style),
            None => {
                let nd = self.eqtb.dim_params[DimParam::NullDelimiterSpace.idx() as usize];
                vec![Node::Kern(nd)]
            }
        }
    }

    fn delim_char_nodes(&self, fam: u8, c: u8, style: GStyle) -> NodeList {
        let fid = self.eqtb.style_fonts[font_size(style)][fam as usize];
        let mut b = hpack(vec![Node::Char { c, font: fid }], None, HBOX, &self.eqtb).node;
        // tex.web §723/§817: center the delimiter's ink on the math axis
        if let Node::Box { h: hh, d: dd, shift: sh, .. } = &mut b {
            *sh = (*hh - *dd) / 2 - self.axis_height(style);
        }
        vec![b]
    }

    /// make_extensible: stack top / rep / mid / rep / bottom glyphs so that
    /// the total extent covers `needed`, centered on the math axis.
    fn ext_delimiter(&self, fam: u8, c: u8, needed: i32, style: GStyle) -> NodeList {
        let Some((fid, f)) = self.fam_font(style, fam) else {
            return self.delim_char_nodes(fam, c, style);
        };
        let Some(ci) = f.chars.get(c as usize) else {
            return self.delim_char_nodes(fam, c, style);
        };
        let Some(rec) = f.ext.get(ci.remainder as usize) else {
            return self.delim_char_nodes(fam, c, style);
        };
        let glyph = |ch: u8| -> Option<(Node, i32, i32)> {
            if ch == 0 || !f.exists_char(ch) {
                return None;
            }
            let n = hpack(vec![Node::Char { c: ch, font: fid }], None, HBOX, &self.eqtb).node;
            let (w, h, d) = box_dims(&n);
            let _ = w;
            Some((n, h, d))
        };
        let top = glyph(rec.top);
        let mid = glyph(rec.mid);
        let bot = glyph(rec.bot);
        let rep = glyph(rec.rep);
        let mut vlist: NodeList = Vec::new();
        let mut total = 0i64;
        if let Some((n, h, d)) = &top {
            vlist.push(n.clone());
            total += (*h + *d) as i64;
        }
        let rep_size = rep.as_ref().map(|(_, h, d)| (*h + *d) as i64).unwrap_or(0);
        let mid_size = mid.as_ref().map(|(_, h, d)| (*h + *d) as i64).unwrap_or(0);
        let base = total + mid_size + bot.as_ref().map(|(_, h, d)| (*h + *d) as i64).unwrap_or(0);
        let mut n_reps = 0i64;
        if rep_size > 0 && (needed as i64) > base {
            n_reps = ((needed as i64 - base) / rep_size).max(0);
        }
        let (na, nb) = if mid.is_some() {
            ((n_reps + 1) / 2, n_reps / 2)
        } else {
            (n_reps, 0)
        };
        if let Some((n, _, _)) = &rep {
            for _ in 0..na {
                vlist.push(n.clone());
            }
        }
        if let Some((n, _, _)) = &mid {
            vlist.push(n.clone());
        }
        if let Some((n, _, _)) = &rep {
            for _ in 0..nb {
                vlist.push(n.clone());
            }
        }
        if let Some((n, _, _)) = &bot {
            vlist.push(n.clone());
        }
        if vlist.is_empty() {
            return vec![Node::Kern(0)];
        }
        let packed = vpack(vlist, None, VBOX, &self.eqtb).node;
        let (_, h, d) = box_dims(&packed);
        let total_h = h + d;
        let axis = self.axis_height(style);
        let extra = (needed - total_h).max(0);
        let h_eff = axis + extra / 2;
        let d_eff = (total_h + extra - h_eff).max(0);
        let last_depth = d; // bottom glyph's depth = box depth
        let shift = (total_h - last_depth) - h_eff;
        let mut out = packed;
        if let Node::Box { h: hh, d: dd, shift: sh, .. } = &mut out {
            *hh = h_eff;
            *dd = d_eff;
            *sh = shift;
        }
        vec![out]
    }
}

// ---------- small local helpers ----------

#[inline]
fn delim_code_of(small: (u8, u8), large: (u8, u8)) -> i32 {
    ((small.0 as i32) << 20) | ((small.1 as i32) << 12) | ((large.0 as i32) << 8) | (large.1 as i32)
}

#[inline]
fn box_dims(n: &Node) -> (i32, i32, i32) {
    match n {
        Node::Box { w, h, d, .. } => (*w, *h, *d),
        Node::Char { .. } | Node::Rule { .. } | Node::Kern(_) | Node::Glue(_) => (0, 0, 0),
        _ => (0, 0, 0),
    }
}


/// append `box` to `out` with the given shift applied
fn set_shift(out: &mut NodeList, mut b: Node, shift: i32) {
    if let Node::Box { shift: s, .. } = &mut b {
        *s = shift;
    }
    out.push(b);
}
