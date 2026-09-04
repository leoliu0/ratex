//! Node lists, glue, boxes, and packing algorithms (hpack/vpack).

use crate::scaled::{self, ONE};
use crate::fonts::FontResolver;
use crate::tfm::FontId;

pub struct EqtbFonts<'a>(pub &'a crate::eqtb::Eqtb);

impl<'a> crate::fonts::FontResolver for EqtbFonts<'a> {
    fn char_width(&self, f: FontId, c: u8) -> i32 { self.0.fonts.get(f as usize).map(|x| x.char_width(c)).unwrap_or(0) }
    fn char_height(&self, f: FontId, c: u8) -> i32 { self.0.fonts.get(f as usize).map(|x| x.char_height(c)).unwrap_or(0) }
    fn char_depth(&self, f: FontId, c: u8) -> i32 { self.0.fonts.get(f as usize).map(|x| x.char_depth(c)).unwrap_or(0) }
    fn char_italic(&self, f: FontId, c: u8) -> i32 { self.0.fonts.get(f as usize).map(|x| x.char_italic(c)).unwrap_or(0) }
}

pub fn eqtb_fonts<'a>(eqtb: &'a crate::eqtb::Eqtb) -> EqtbFonts<'a> {
    EqtbFonts(eqtb)
}

pub const GLUE_FIL: u8 = 1;
pub const GLUE_FILL: u8 = 2;
pub const GLUE_FILLL: u8 = 3;

pub const HBOX: u8 = 0;
pub const VBOX: u8 = 1;
pub const VTOP: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MathStyle {
    Display,
    Text,
    Script,
    ScriptScript,
}

#[derive(Clone, Debug)]
pub struct Glue {
    pub width: i32,
    pub stretch: i32,
    pub shrink: i32,
    pub stretch_order: u8,
    pub shrink_order: u8,
}

impl Glue {
    pub fn zero() -> Glue {
        Glue { width: 0, stretch: 0, shrink: 0, stretch_order: 0, shrink_order: 0 }
    }
    pub fn new(w: i32) -> Glue {
        Glue { width: w, stretch: 0, shrink: 0, stretch_order: 0, shrink_order: 0 }
    }
    pub fn fil(order: u8, w: i32) -> Glue {
        Glue { width: w, stretch: ONE, shrink: 0, stretch_order: order, shrink_order: 0 }
    }
}

#[derive(Clone, Debug)]
pub enum WhatIt {
    PdfLiteral { origin: u8, data: String },
    PdfColorPush(String),
    PdfColorPop,
    PdfSave,
    PdfRestore,
    Write { stream: u16, tokens: Vec<crate::token::Token> },
    PdfDest { name: String, kind: u8, params: [i32; 4] },
    PdfAnnot { attr: String, wd: i32, ht: i32, dp: i32 },
    PdfStartLink { attr: String, uri: Option<String>, name: Option<String> },
    PdfEndLink,
    Special(String),
    SavePos { obj: i32 },
    User(i32),
}

/// leader kinds (tex.web subtypes a_leaders/c_leaders/x_leaders)
pub const LEADERS_A: u8 = 0;
pub const LEADERS_C: u8 = 1;
pub const LEADERS_X: u8 = 2;

/// the repeated object of a leader node: a rule or a packed box
#[derive(Clone, Debug)]
pub enum LeaderBody {
    Rule { width: i32, height: i32, depth: i32 },
    Box(Box<Node>),
}

/// (width, height, depth) of a leader body
pub fn leader_dims(body: &LeaderBody) -> (i32, i32, i32) {
    match body {
        LeaderBody::Rule { width, height, depth } => (*width, *height, *depth),
        LeaderBody::Box(b) => match &**b {
            Node::Box { w, h, d, .. } => (*w, *h, *d),
            Node::Rule { width, height, depth } => (*width, *height, *depth),
            _ => (0, 0, 0),
        },
    }
}

#[derive(Clone, Debug)]
pub struct DiscNode {
    pub pre_break: NodeList,
    pub post_break: NodeList,
    pub no_break: NodeList,
    pub replace_count: usize,
}

#[derive(Clone, Debug)]
pub enum Node {
    Char { c: u8, font: FontId },
    /// `letters` = the component letters that formed the glyph (hyphenation
    /// needs them: a break point may fall inside the ligature)
    Ligature { c: u8, font: FontId, lig_width: i32, lig_height: i32, lig_depth: i32, letters: [u8; 3], n_letters: u8 },
    Glue(Glue),
    Kern(i32),
    ExplicitKern(i32),
    Penalty(i32),
    Rule { width: i32, height: i32, depth: i32 },
    Leaders { glue: Glue, kind: u8, body: LeaderBody },
    Disc(DiscNode),
    Box {
        kind: u8,
        w: i32,
        h: i32,
        d: i32,
        shift: i32,
        list: NodeList,
        glue_sign: u8,
        glue_order: u8,
        glue_set: f64,
        font: Option<FontId>,
    },
    Mark { class: i32, tokens: Vec<crate::token::Token> },
    Ins { num: u16, height: i32, depth: i32, cost: i32, box_node: Box<Node> },
    Adj(i32),
    Whatsit(WhatIt),
    // math nodes (converted to boxes before shipping):
    Style(MathStyle),
    Choice,
    ChoiceAlt { body: NodeList },
    MathChar { fam: u8, c: u8, class: u8 },
    Frac { num: NodeList, den: NodeList, thickness: i32, left: Option<i32>, right: Option<i32> },
    Radical { body: NodeList, left_delim: Option<(u8, u8)>, thickness: i32 },
    Scripts { nucleus: NodeList, sup: Option<NodeList>, sub: Option<NodeList> },
    DelimBox { small: (u8, u8), large: (u8, u8), size: u8 },
    OpLimits { op: NodeList, above: Option<NodeList>, below: Option<NodeList> },
    MathKern(i32, u8),
    Accent { accent: (u8, FontId), body: NodeList, skew: i32 },
    InsDisc,
    Empty,
    VAdjust(NodeList),
}

pub type NodeList = Vec<Node>;

/// dimensions of a single node in a horizontal list
fn single_dims(n: &Node, eqtb: &crate::eqtb::Eqtb) -> (i32, i32, i32) {
    match n {
        Node::Char { c, font } => (
            eqtb_fonts(eqtb).char_width(*font, *c),
            eqtb_fonts(eqtb).char_height(*font, *c),
            eqtb_fonts(eqtb).char_depth(*font, *c),
        ),
        Node::Ligature { lig_width, lig_height, lig_depth, .. } => {
            (*lig_width, *lig_height, *lig_depth)
        }
        Node::Glue(g) => (g.width, 0, 0),
        Node::Kern(k) | Node::ExplicitKern(k) => (*k, 0, 0),
        Node::Penalty(_) => (0, 0, 0),
        Node::Rule { width, height, depth } => (*width, *height, *depth),
        Node::Box { w, h, d, shift, .. } => (*w, (*h - *shift).max(0), (*d + *shift).max(0)),
        Node::Mark { .. } | Node::Style(_) | Node::Adj(_) | Node::Choice | Node::ChoiceAlt { .. } => (0, 0, 0),
        Node::Scripts { nucleus, .. } => hlist_dims(nucleus, eqtb),
        Node::Frac { num, den, .. } => {
            let (wn, hn, _) = hlist_dims(num, eqtb);
            let (wd, hd, _) = hlist_dims(den, eqtb);
            (wn.max(wd), hn + hd, 0)
        }
        Node::Radical { body, .. } => hlist_dims(body, eqtb),
        Node::OpLimits { op, .. } => hlist_dims(op, eqtb),
        Node::DelimBox { .. } | Node::Accent { .. } | Node::Whatsit(_) | Node::Ins { .. }
        | Node::Disc(_) | Node::MathChar { .. } => (0, 0, 0),
        Node::VAdjust(_) | Node::InsDisc | Node::Empty | Node::MathKern(_, _) => (0, 0, 0),
        Node::Leaders { glue, body, .. } => {
            let (_, bh, bd) = leader_dims(body);
            (glue.width, bh, bd)
        }
    }
}

/// natural dimensions of a horizontal list. Returns (width, height, max_depth)
pub fn hlist_dims(list: &[Node], eqtb: &crate::eqtb::Eqtb) -> (i32, i32, i32) {
    let mut w = 0i64;
    let mut h = 0i64;
    let mut d = 0i64;
    let mut i = 0usize;
    while i < list.len() {
        let n = &list[i];
        if let Node::Disc(dc) = n {
            for n2 in &dc.no_break {
                let (w2, h2, d2) = single_dims(n2, eqtb);
                w += w2 as i64;
                h = h.max(h2 as i64);
                d = d.max(d2 as i64);
            }
            i += 1 + dc.replace_count;
            continue;
        }
        if matches!(n, Node::Choice) {
            i += 1;
            continue;
        }
        let (w2, h2, d2) = single_dims(n, eqtb);
        w += w2 as i64;
        h = h.max(h2 as i64);
        d = d.max(d2 as i64);
        i += 1;
    }
    (w as i32, h as i32, d as i32)
}

/// natural dimensions of a vertical list: (width, height, depth), per
/// tex.web vpackage §13178: glue/kern reset the running depth, box width
/// competes as width+shift, and penalty/mark/ins/whatsit items do nothing.
pub fn vlist_dims(list: &[Node], eqtb: &crate::eqtb::Eqtb) -> (i32, i32, i32) {
    let _ = eqtb;
    let (mut x, mut d, mut w) = (0i64, 0i64, 0i64);
    for n in list {
        match n {
            Node::Box { w: bw, h: bh, d: bd, shift, .. } => {
                x += d + *bh as i64;
                d = *bd as i64;
                w = w.max(*bw as i64 + *shift as i64);
            }
            Node::Rule { height, depth, width } => {
                x += d + *height as i64;
                d = *depth as i64;
                w = w.max(*width as i64);
            }
            Node::Glue(g) => {
                x += d + g.width as i64;
                d = 0;
            }
            Node::Kern(k) | Node::ExplicitKern(k) => {
                x += d + *k as i64;
                d = 0;
            }
            Node::Leaders { body, .. } => {
                x += d;
                d = 0;
                let (lw, _, _) = leader_dims(body);
                w = w.max(lw as i64);
            }
            // penalty, mark, ins, whatsit, math-only nodes: do_nothing
            _ => {}
        }
    }
    (w as i32, x as i32, d as i32)
}


/// badness(t, s) exactly as tex.web §2337: r ≈ 297·t/s, badness = r³/2¹⁸
/// rounded, capped at INF_BAD. (scaled::badness uses the exact cube; real
/// TeX uses this approximation, so packers must too for byte compatibility.)
pub fn tex_badness(t: i32, s: i32) -> i32 {
    if t == 0 {
        return 0;
    }
    if s <= 0 {
        return scaled::INF_BAD;
    }
    let ti = t as i64;
    let si = s as i64;
    let r: i64 = if t <= 7230584 {
        ti * 297 / si
    } else if s >= 1663497 {
        ti / (si / 297)
    } else {
        ti
    };
    if r > 1290 {
        scaled::INF_BAD
    } else {
        ((r * r * r + 262144) / 262144) as i32
    }
}

/// choose glue sign/order/ratio per tex.web hpack §649: check the highest
/// nonzero stretch (or shrink) order first — filll, fill, fil, normal.
/// If that total is 0 the sign reverts to normal with ratio 0.
pub fn compute_glue_set(
    target: i64,
    natural: i64,
    stretch: [i64; 4],
    shrink: [i64; 4],
) -> (u8, u8, f64) {
    let x = target - natural;
    if x == 0 {
        return (0, 0, 0.0);
    }
    let pick = |v: &[i64; 4]| -> usize {
        if v[3] != 0 { 3 } else if v[2] != 0 { 2 } else if v[1] != 0 { 1 } else { 0 }
    };
    if x > 0 {
        let o = pick(&stretch);
        if stretch[o] != 0 {
            (1, o as u8, x as f64 / stretch[o] as f64)
        } else {
            (0, 0, 0.0)
        }
    } else {
        let o = pick(&shrink);
        if shrink[o] != 0 {
            (2, o as u8, (-x) as f64 / shrink[o] as f64)
        } else {
            (0, 0, 0.0)
        }
    }
}

pub struct PackResult {
    pub node: Node,
    /// tex.web `last_badness`: 0 unless the pack was under/tight/overfull
    pub badness: i32,
    /// requested − natural along the packing axis ("x" in tex.web hpack)
    pub delta: i64,
    pub stretch: [i64; 4],
    pub shrink: [i64; 4],
    pub sign: u8,
    pub order: u8,
}

fn glue_sums(list: &[Node]) -> ([i64; 4], [i64; 4]) {
    let mut stretch = [0i64; 4];
    let mut shrink = [0i64; 4];
    for n in list {
        let g = match n {
            Node::Glue(g) => g,
            Node::Leaders { glue: g, .. } => g,
            _ => continue,
        };
        stretch[g.stretch_order as usize] += g.stretch as i64;
        shrink[g.shrink_order as usize] += g.shrink as i64;
    }
    (stretch, shrink)
}

/// shared tail of hpack/vpackage: glue decision, capped shrink, last_badness,
/// and the overfull-rule marker for overfull hboxes (tex.web §656-659).
fn finish_glue(
    mut list: NodeList,
    target: i64,
    natural: i64,
    stretch: [i64; 4],
    shrink: [i64; 4],
    horizontal: bool,
    eqtb: &crate::eqtb::Eqtb,
) -> (NodeList, u8, u8, f64, i32, i64) {
    use crate::prim::DimParam;
    let x = target - natural;
    let nonempty = !list.is_empty();
    let (sign, order, mut set) = compute_glue_set(target, natural, stretch, shrink);
    let mut bad = 0i32;
    if x > 0 {
        // stretching: badness only meaningful in the normal order
        if order == 0 && nonempty {
            bad = tex_badness(x as i32, stretch[0] as i32);
        }
    } else if x < 0 {
        if order == 0 {
            if (shrink[0] as i64) < -x {
                // overfull: clamp the glue ratio to exactly 1
                set = 1.0;
                if nonempty {
                    bad = 1_000_000;
                    let excess = -x - shrink[0];
                    let fuzz = eqtb.dim_params[DimParam::Hfuzz.idx() as usize] as i64;
                    let rule_w = eqtb.dim_params[DimParam::OverfullRule.idx() as usize] as i64;
                    if horizontal && rule_w > 0 && excess > fuzz {
                        list.push(Node::Rule { width: rule_w as i32, height: 0, depth: 0 });
                    }
                }
            } else if nonempty {
                bad = tex_badness((-x) as i32, shrink[0] as i32);
            }
        }
    }
    (list, sign, order, set, bad, x)
}

/// \hbox packing; `additional` selects tex.web's m=additional (\hbox spread).
pub fn hpack_add(
    list: NodeList,
    w: Option<i32>,
    additional: bool,
    kind: u8,
    eqtb: &crate::eqtb::Eqtb,
) -> PackResult {
    let (nat_w, h, d) = hlist_dims(&list, eqtb);
    let nat = nat_w as i64;
    let (stretch, shrink) = glue_sums(&list);
    let mut target = w.map(|v| v as i64).unwrap_or(nat);
    if additional {
        target = nat + target;
    }
    let (list, sign, order, set, bad, delta) =
        finish_glue(list, target, nat, stretch, shrink, true, eqtb);
    PackResult {
        node: Node::Box {
            kind,
            w: target as i32,
            h,
            d,
            shift: 0,
            list,
            glue_sign: sign,
            glue_order: order,
            glue_set: set,
            font: None,
        },
        badness: bad,
        delta,
        stretch,
        shrink,
        sign,
        order,
    }
}

/// \hbox packing with an exact (or natural) target
pub fn hpack(list: NodeList, w: Option<i32>, kind: u8, eqtb: &crate::eqtb::Eqtb) -> PackResult {
    hpack_add(list, w, false, kind, eqtb)
}

/// \vbox/\vtop packing with explicit max depth (tex.web vpackage's `l`):
/// \vbox passes box_max_depth, \vsplit split_max_depth, the page builder
/// page_max_depth. Excess depth moves into the natural height *before*
/// glue setting.
pub fn vpack_add_md(
    list: NodeList,
    h: Option<i32>,
    additional: bool,
    kind: u8,
    eqtb: &crate::eqtb::Eqtb,
    max_depth: i32,
) -> PackResult {
    let (w, nat_h, nat_d) = vlist_dims(&list, eqtb);
    // tex.web vpackage §13468: rules with running (null) width take the
    // packed box's width — e.g. \hrule inside a tabular \noalign block must
    // span the alignment's natural width, not \hsize.
    let mut list = list;
    for n in list.iter_mut() {
        if let Node::Rule { width, .. } = n {
            if *width == crate::build::RULE_FILL {
                *width = w;
            }
        }
    }
    let (mut x, mut d) = (nat_h as i64, nat_d as i64);
    let (stretch, shrink) = glue_sums(&list);
    if d > max_depth as i64 {
        x += d - max_depth as i64;
        d = max_depth as i64;
    }
    let mut target = h.map(|v| v as i64).unwrap_or(x);
    if additional {
        target = x + target;
    }
    let (list, sign, order, set, bad, delta) =
        finish_glue(list, target, x, stretch, shrink, false, eqtb);
    PackResult {
        node: Node::Box {
            kind,
            w,
            h: target as i32,
            d: d as i32,
            shift: 0,
            list,
            glue_sign: sign,
            glue_order: order,
            glue_set: set,
            font: None,
        },
        badness: bad,
        delta,
        stretch,
        shrink,
        sign,
        order,
    }
}

/// vpack with no depth clamp (tex.web's max_dimen `l`)
pub fn vpack(list: NodeList, h: Option<i32>, kind: u8, eqtb: &crate::eqtb::Eqtb) -> PackResult {
    vpack_add_md(list, h, false, kind, eqtb, i32::MAX)
}

/// \vtop: vpackage to the target, then readjust per tex.web package() §1074:
/// height becomes the *raw* height of the first box/rule item (0 if the list
/// is empty or starts with anything else), and the depth absorbs the rest.
pub fn vtop_md(
    list: NodeList,
    h: Option<i32>,
    additional: bool,
    eqtb: &crate::eqtb::Eqtb,
    max_depth: i32,
) -> PackResult {
    let mut res = vpack_add_md(list, h, additional, VTOP, eqtb, max_depth);
    if let Node::Box { h: hh, d: dd, list, .. } = &mut res.node {
        let first_h = list.iter().find_map(|n| match n {
            Node::Box { h: bh, d: bd, .. } if *bh > 0 || *bd > 0 => Some(*bh),
            Node::Rule { height, depth, .. } if *height > 0 || *depth > 0 => Some(*height),
            _ => None,
        })
        .unwrap_or(0);
        *dd = *dd - first_h + *hh;
        *hh = first_h;
    }
    res
}

/// \vtop with box_max_depth (the tex.web package() default)
pub fn vtop(list: NodeList, h: Option<i32>, eqtb: &crate::eqtb::Eqtb) -> PackResult {
    use crate::prim::DimParam;
    let md = eqtb.dim_params[DimParam::BoxMaxDepth.idx() as usize];
    vtop_md(list, h, false, eqtb, md)
}

/// leader replication positions along one axis, tex.web hlist_out/vlist_out
/// §12456/§12625: returns (positions of each copy, lx spacing, final cur).
/// `left_edge` is the containing box's content origin; inputs/outputs in the
/// caller's units (sp at shipout) with tex.web's ±10 rounding compensation.
pub fn leader_layout(
    kind: u8,
    leader_wd: i64,
    total_w: i64,
    left_edge: i64,
    mut cur: i64,
) -> (Vec<i64>, i64, i64) {
    let mut lx = 0i64;
    let mut out = Vec::new();
    if leader_wd <= 0 || total_w <= 0 {
        return (out, lx, cur);
    }
    let rule_wd = total_w + 10; // compensate for floating-point rounding
    let edge = cur + rule_wd;
    if kind == LEADERS_A {
        let save = cur;
        cur = left_edge + leader_wd * ((cur - left_edge) / leader_wd);
        if cur < save {
            cur += leader_wd;
        }
    } else {
        let lq = rule_wd / leader_wd;
        let lr = rule_wd % leader_wd;
        if kind == LEADERS_C {
            cur += lr / 2;
        } else {
            lx = lr / (lq + 1);
            cur += (lr - (lq - 1) * lx) / 2;
        }
    }
    while cur + leader_wd <= edge {
        out.push(cur);
        cur += leader_wd + lx;
    }
    (out, lx, edge - 10)
}

