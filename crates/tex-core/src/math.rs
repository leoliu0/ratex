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
    /// None=default (large operator in display styles gets limits; tex.web
    /// make_op: `(subtype(q)=normal) and (cur_style<text_style)`).
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
                if let Node::DelimBox { size: 1, small, large } = n {
                    let code = delim_code_of(*small, *large);
                    match lr_stack.pop() {
                        Some((lopen, buf)) => {
                            let body = self.mlist_to_hlist(&buf, style);
                            let (_, bh, bd) = hlist_dims(&body, &self.eqtb);
                            let needed = self.lr_delimiter_size(bh, bd, style);
                            let mut assembled: NodeList = self.var_delimiter(lopen, needed, style);
                            assembled.extend(body);
                            assembled.extend(self.var_delimiter(code, needed, style));
                            style = after_lr_style(style);
                            match lr_stack.last_mut() {
                                // nested boundary: splice into the enclosing buffer
                                Some((_, pbuf)) => pbuf.extend(assembled),
                                None => {
                                    self.emit_atom(&mut out, &mut prev, Some(CL_INNER), assembled, style);
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
                if let Node::Style(s) = n {
                    style = gstyle_of(*s);
                    i += 1;
                    continue;
                }
                let nodes = self.convert_atom(n, style);
                // inter-atom mu glue comes first (tex.web second pass)
                self.insert_spacing(&mut out, prev, Some(cls), style);
                out.extend(nodes);
                // line-break penalty after a bin/rel atom (tex.web §766)
                if matches!(cls, CL_BIN | CL_REL)
                    && i + 1 < list.len()
                    && !matches!(list[i + 1], Node::Penalty(_))
                {
                    let p = if cls == CL_BIN {
                        self.eqtb.int_params[IntParam::BinOpPenalty.idx() as usize]
                    } else {
                        self.eqtb.int_params[IntParam::RelPenalty.idx() as usize]
                    };
                    out.push(Node::Penalty(p));
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
            let body = self.mlist_to_hlist(&buf, style);
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
        let mu = self.mu_unit();
        // plain TeX: \thinmuskip=3mu, \medmuskip=4mu plus 2mu minus 4mu,
        // \thickmuskip=5mu plus 5mu
        let g = match kind {
            1 | 2 => Glue { width: mu * 3, stretch: 0, shrink: 0, stretch_order: 0, shrink_order: 0 },
            3 => Glue { width: mu * 4, stretch: mu * 2, shrink: mu * 4, stretch_order: 0, shrink_order: 0 },
            _ => Glue { width: mu * 5, stretch: mu * 5, shrink: 0, stretch_order: 0, shrink_order: 0 },
        };
        out.push(Node::Glue(g));
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
                self.op_char_box(*fam, *c, style, false)
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
                let nodes = self.mlist_to_hlist(op, style);
                (hpack(nodes, None, HBOX, &self.eqtb).node, 0)
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
            [Node::MathChar { fam, c, class }] if *class != CL_OP => {
                let Some((fid, f)) = self.fam_font(style, *fam) else {
                    return vec![];
                };
                if !f.exists_char(*c) {
                    return vec![];
                }
                delta = f.char_italic(*c);
                let mut core: NodeList = vec![Node::Char { c: *c, font: fid }];
                if sub.is_none() && delta != 0 {
                    core.push(Node::Kern(delta));
                    delta = 0;
                }
                nuc = hpack(core, None, HBOX, &self.eqtb).node;
            }
            // non-limits big operator: axis-centered box, italic handled above
            [Node::MathChar { fam, c, class }] if *class == CL_OP => {
                let (b, d) = self.op_char_box(*fam, *c, style, sub.is_some());
                delta = d;
                nuc = b;
                let t = if style < 4 { 2 } else { 4 };
                let (zh, zd) = box_dims_shifted(&nuc);
                shift_up = zh - self.fparam(t, 2, 18);
                shift_down = zd + self.fparam(t, 2, 19);
            }
            // boxed nucleus: initial shifts from its (shift-adjusted) dims
            _ => {
                let nodes = self.mlist_to_hlist(nucleus, style);
                nuc = hpack(nodes, None, HBOX, &self.eqtb).node;
                let t = if style < 4 { 2 } else { 4 };
                let (zh, zd) = box_dims_shifted(&nuc);
                shift_up = zh - self.fparam(t, 2, 18);
                shift_down = zd + self.fparam(t, 2, 19);
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
            let mut nodes = self.mlist_to_hlist(s, sup_style(style));
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
            let mut nodes = self.mlist_to_hlist(s, sub_style(style));
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
        let (_, oh, od) = box_dims(&op_box);
        let sup_box = above.map(|s| hpack(self.mlist_to_hlist(s, sup_style(style)), None, HBOX, &self.eqtb).node);
        let sub_box = below.map(|s| hpack(self.mlist_to_hlist(s, sub_style(style)), None, HBOX, &self.eqtb).node);
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
        let r = if thickness < 0 {
            self.default_rule_thickness(style)
        } else {
            thickness
        };
        let num_box = hpack(self.mlist_to_hlist(num, num_style(style)), None, HBOX, &self.eqtb).node;
        let den_box = hpack(self.mlist_to_hlist(den, den_style(style)), None, HBOX, &self.eqtb).node;
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
            out.extend(self.var_delimiter(pair_to_code(Some(l)), dd_size, style));
        }
        out.push(packed);
        if let Some(rr) = right {
            out.extend(self.var_delimiter(pair_to_code(Some(rr)), dd_size, style));
        }
        out
    }

    /// tex.web make_radical (§752-753): the body is boxed in the cramped
    /// style; the surd is chosen for h+d+clr+rt; excess surd depth widens the
    /// clearance by half; the surd baseline drops to -(h+clr) and the
    /// overbar box = [kern(surd_h), rule(surd_h), kern(clr), body].
    fn make_radical(&self, body: &[Node], thickness: i32, style: GStyle) -> NodeList {
        let (delim_code, r_explicit) = unpack_radical(thickness);
        let body_nodes = self.mlist_to_hlist(body, style | 1);
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
            let body_nodes = self.mlist_to_hlist(body, style | 1);
            return vec![hpack(body_nodes, None, HBOX, &self.eqtb).node];
        };
        // skew: kern from the accent font's \skewchar to the accent character
        let sk = self.skew_char_of(afid, &af);
        let s = if sk >= 0 { self.char_kern(afid, &af, sk as u8, ac) } else { 0 };
        let body_nodes = self.mlist_to_hlist(body, style | 1);
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
            "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\$=3 \\catcode`\\^=7 \\catcode`\\_=8\n",
        );
        for c in b'a'..=b'z' {
            s.push_str(&format!("\\mathcode`{}={}\n", c as char, 0x7100 + c as i32));
        }
        for c in b'0'..=b'9' {
            s.push_str(&format!("\\mathcode`{}={}\n", c as char, 0x7000 + c as i32));
        }
        s.push_str(concat!(
            "\\mathcode`\\==\"303D \\mathcode`\\+=\"202B \\mathcode`\\-=\"2200\n",
            "\\mathcode`\\(=\"4028 \\mathcode`\\)=\"5029\n",
            "\\delcode`\\(=\"028300 \\delcode`\\)=\"029301 \\scriptspace=0.5pt\n",
            "\\font\\tenrm=cmr10 \\font\\teni=cmmi10 \\font\\tensy=cmsy10 \\font\\tenex=cmex10\n",
            "\\font\\sevenrm=cmr7 \\font\\seveni=cmmi7 \\font\\sevensy=cmsy7\n",
            "\\font\\fiverm=cmr5 \\font\\fivei=cmmi5 \\font\\fivesy=cmsy5\n",
            "\\textfont0=\\tenrm \\scriptfont0=\\sevenrm \\scriptscriptfont0=\\fiverm\n",
            "\\textfont1=\\teni \\scriptfont1=\\seveni \\scriptscriptfont1=\\fivei\n",
            "\\textfont2=\\tensy \\scriptfont2=\\sevensy \\scriptscriptfont2=\\fivesy\n",
            "\\textfont3=\\tenex \\scriptfont3=\\tenex \\scriptscriptfont3=\\tenex\n",
            "\\mathchardef\\sumG=\"1350 \\mathchardef\\intG=\"1352\n",
            "\\def\\sqrtG#1{\\radical\"270370 {#1}}\n\\def\\sqrtS#1{\\radical \"270370 {#1}}\n\\def\\sqrtB#1{{\\radical\"270370 #1}}\n\\def\\tmpA#1{#1}\n",
            "\\def\\fracG#1#2{{#1\\over #2}}\n",
            "\\def\\barG#1{\\mathaccent \"7016 {#1}}\n",
        ));
        s
    }

    /// run a complete document (preamble + body) in one engine pass: the
    /// job ends when the first pushed file is exhausted, so setup and body
    /// must share a file
    fn run_doc(body: &str) -> Engine {
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
        while let Node::Box { list, .. } = &n {
            if list.len() == 1 {
                if let Node::Box { .. } = &list[0] {
                    n = list[0].clone();
                    continue;
                }
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
        let (_, vh, _, _, vshift) = box_of(&list[1]);
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
        approx(vh, su + 3.01389, "vbox h = num1 + h(a)");
        approx(vd, sd, "vbox d = denom1");
        // rule top at axis + half(r): kern1 = (su - h(a)) - (axis + r/2)
        if let (Node::Kern(k1), Node::Kern(k2)) = (&vlist[1], &vlist[3]) {
            let r = RT * 10.0;
            let axis = AXIS * 10.0;
            approx(*k1, su - 0.0 - (axis + r / 2.0), "display kern1");
            approx(*k2, (axis - r / 2.0) - (4.8611 - sd), "display kern2");
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
        approx(-ssh, -7.20276, "surd shift -(h(x)+clr)");
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
        approx(h, 8.04175, "sum total height");
        approx(d, 3.00005, "sum total depth");
        assert_eq!(list.len(), 2, "op box + scripts vbox: {:?}", list);
        // op axis-centered: shift = -(7.50006)
        approx(-box_shift(&list[0]), 7.50006, "op axis shift");
        let (vlist, _, _, _, vshift) = box_of(&list[1]);
        approx(vshift, 3.00005, "scripts vbox shift");
        assert_eq!(vlist.len(), 3, "sup n, kern, sub i=1: {:?}", vlist);
        if let Node::Kern(k) = &vlist[1] {
            approx(*k, 3.39598, "scripts stack kern");
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
        approx(vh, 5.0 + BIG5 * 10.0 + 3.01389 + su + 10.00012, "limits vbox h");
        approx(vd, 0.55547 + BIG5 * 10.0 + 4.63193 + sd, "limits vbox d");
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
        approx(box_shift(&vlist[0]), 0.63542, "accent skew shift");
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
        assert_eq!(chars, 2, "y and z present: {:?}", sup);
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
        let mut e = math_engine();
        let full = "\\hbox{$(a\\overwithdelims()b)$}\n".to_string();
        e.input.push_file("m.tex".to_string(), full.into_bytes());
        e.run();
        assert_eq!(e.error_count, 0, "{}", e.term);
        let b = e
            .page_list
            .iter()
            .find(|n| matches!(n, Node::Box { kind: HBOX, .. }))
            .cloned()
            .expect("hbox");
        let (list, _, _, _, _) = box_of(&b);
        assert!(list.len() >= 3, "delim + frac + delim: {:?}", list);
        // both delimiters are cmex parens (big variants), axis-centered
        let (_, _, ph1, pd1, sh1) = box_of(&list[0]);
        assert!(ph1 + pd1 >= su(DELIM2_MIN), "delimiter covers delim2");
        approx(-sh1, AXIS * 10.0 + 2.8, "axis centered");
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
        // fraction is the last atom: inner class vbox with a rule
        let last = &list[list.len() - 1];
        match last {
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
        // 'T' is 0.611112em wide at 10pt: 6.11112pt; others differ
        approx(w, 6.11112, "text branch chosen (T)");
        let _ = list;
        // display picks the first branch (D)
        let bd = display_math("\\mathchoice{D}{T}{S}{SS}");
        let (_, wd, _, _, _) = box_of(&bd);
        approx(wd, 7.77779, "display branch chosen (D)"); // 'D' in cmr10
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
}
