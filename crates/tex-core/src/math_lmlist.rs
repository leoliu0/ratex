//! LuaTeX `mlist_to_hlist` (luatex `mlist.c`) over ratex's flat math lists.
//!
//! The raw list ratex builds is decoded into noad slots (nucleus kind,
//! class, scripts), nested `\left...\right` groups become one inner noad
//! whose sub-list starts and ends with fence noads, and the two passes of
//! mlist.c then run on the slots. Only the LuaTeX engine comes through
//! here; the other engines keep the TeX82 conversion of `math.rs`.

use crate::boxes::{hlist_dims, Glue, MathDiagnosticOrigin, MathStyle, Node, NodeList, HBOX, VBOX};
use crate::engine::Engine;
use crate::eqtb::UNDEFINED_MATH_PARAMETER;
use crate::math::{
    sup_style, sub_style, GStyle, CL_BIN, CL_CLOSE, CL_INNER, CL_OPEN, CL_OP, CL_ORD, CL_PUNCT,
    CL_REL,
};
use crate::math_otf::*;
use crate::prim::{DimParam, IntParam};
use crate::tfm::FontId;
use crate::uprim::mp::*;

const OP_NORMAL: u8 = 0;
const OP_LIMITS: u8 = 1;
const OP_NOLIMITS: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Special {
    None,
    Over,
    Under,
    VCenter,
}

#[derive(Clone, Debug)]
enum Nuc {
    None,
    Char { fam: u8, c: u32, origin: MathDiagnosticOrigin },
    Mlist(NodeList),
    /// a `\left...\right` list: starts and ends with its fence markers
    Fenced(NodeList),
    Box(Node),
}

#[derive(Clone, Debug)]
struct Noad {
    class: u8,
    special: Special,
    opsub: u8,
    nuc: Nuc,
    sup: Option<NodeList>,
    sub: Option<NodeList>,
    text_char: bool,
}

impl Noad {
    fn new(class: u8, nuc: Nuc) -> Noad {
        Noad { class, special: Special::None, opsub: OP_NORMAL, nuc, sup: None, sub: None, text_char: false }
    }
    fn has_scripts(&self) -> bool {
        self.sup.is_some() || self.sub.is_some()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Debug)]
enum Item {
    Noad(Noad),
    /// a fraction/radical/accent converted by `convert_atom`; `inner`
    /// spacing class for fractions
    Raw { node: Node, frac: bool },
    Fence { side: Side, delim: Option<(u8, u32, u8, u32)> },
    Style(GStyle),
    NonScript,
    Other(Node),
}

#[derive(Clone, Debug)]
struct Slot {
    item: Item,
    hlist: NodeList,
}

impl Slot {
    fn new(item: Item) -> Slot {
        Slot { item, hlist: Vec::new() }
    }
}

#[inline]
fn size_of_style(g: GStyle) -> usize {
    match g {
        0..=3 => 0,
        4 | 5 => 1,
        _ => 2,
    }
}

#[inline]
fn cramped(g: GStyle) -> GStyle {
    g | 1
}

fn is_char_node(n: &Node) -> bool {
    matches!(n, Node::Char { .. } | Node::LuaGlyph(_))
}

fn italic_kern(k: i32) -> Node {
    Node::ItalicKern(k)
}

impl Engine {
    // ================= decoding the raw list =================

    fn lm_decode(&mut self, list: &[Node], fenced: bool) -> Vec<Slot> {
        let mut out: Vec<Slot> = Vec::with_capacity(list.len());
        let mut i = 0;
        while i < list.len() {
            let n = &list[i];
            match n {
                Node::Style(s) => out.push(Slot::new(Item::Style(crate::math::gstyle_of(*s)))),
                Node::NonScript => out.push(Slot::new(Item::NonScript)),
                Node::DelimBox { small, large, size, .. } => {
                    let d = Some((small.0, u32::from(small.1), large.0, u32::from(large.1)));
                    match *size {
                        0 if fenced && i == 0 => out.push(Slot::new(Item::Fence { side: Side::Left, delim: d })),
                        0 => {
                            let mut depth = 1usize;
                            let mut j = i + 1;
                            let mut found = None;
                            while j < list.len() {
                                match &list[j] {
                                    Node::DelimBox { size: 0, .. } => depth += 1,
                                    other if close_marker(other).is_some() => {
                                        depth -= 1;
                                        if depth == 0 {
                                            found = Some(j);
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                                j += 1;
                            }
                            let end = found.unwrap_or(list.len() - 1);
                            let mut sub: NodeList = list[i..=end].to_vec();
                            let mut noad = Noad::new(CL_INNER, Nuc::None);
                            if found.is_some() {
                                if let Some((marker, sup, below)) = close_marker(&list[end]) {
                                    *sub.last_mut().unwrap() = marker.clone();
                                    noad.sup = sup;
                                    noad.sub = below;
                                }
                            }
                            noad.nuc = Nuc::Fenced(sub);
                            out.push(Slot::new(Item::Noad(noad)));
                            i = end;
                        }
                        1 if fenced && i + 1 == list.len() => {
                            out.push(Slot::new(Item::Fence { side: Side::Right, delim: d }))
                        }
                        1 => {
                            // stray right delimiter: a close noad
                            out.push(Slot::new(Item::Noad(Noad::new(
                                CL_CLOSE,
                                Nuc::Char { fam: small.0, c: u32::from(small.1), origin: MathDiagnosticOrigin::default() },
                            ))));
                        }
                        3 if fenced => out.push(Slot::new(Item::Fence { side: Side::Middle, delim: d })),
                        3 => {}
                        _ => out.push(Slot::new(Item::Noad(Noad::new(
                            CL_ORD,
                            Nuc::Char { fam: small.0, c: u32::from(small.1), origin: MathDiagnosticOrigin::default() },
                        )))),
                    }
                }
                Node::MathChar { fam: 255, .. } => {}
                Node::MathChar { fam, c, class, origin } => {
                    let mut noad = Noad::new(*class, Nuc::Char { fam: *fam, c: *c, origin: origin.clone() });
                    noad.opsub = OP_NORMAL;
                    out.push(Slot::new(Item::Noad(noad)));
                }
                Node::Scripts { nucleus, sup, sub } => {
                    out.push(Slot::new(self.lm_decode_scripts(nucleus, sup, sub, false)));
                }
                Node::OpLimits { op, above, below } => {
                    out.push(Slot::new(self.lm_decode_oplimits(op, above, below)));
                }
                Node::Frac { .. } => out.push(Slot::new(Item::Raw { node: n.clone(), frac: true })),
                Node::Radical { .. } | Node::Accent { .. } => {
                    out.push(Slot::new(Item::Raw { node: n.clone(), frac: false }))
                }
                Node::Box { .. } => out.push(Slot::new(Item::Noad(Noad::new(CL_ORD, Nuc::Box(n.clone()))))),
                Node::VCenter { box_node } => {
                    let mut noad = Noad::new(CL_ORD, Nuc::Box((**box_node).clone()));
                    noad.special = Special::VCenter;
                    out.push(Slot::new(Item::Noad(noad)));
                }
                Node::Overline { body, under, .. } => {
                    let mut noad = Noad::new(CL_ORD, Nuc::Mlist(body.clone()));
                    noad.special = if *under { Special::Under } else { Special::Over };
                    out.push(Slot::new(Item::Noad(noad)));
                }
                Node::Empty | Node::InsDisc => {}
                other => out.push(Slot::new(Item::Other(other.clone()))),
            }
            i += 1;
        }
        out
    }

    fn lm_decode_oplimits(&mut self, op: &[Node], above: &Option<NodeList>, below: &Option<NodeList>) -> Item {
        let mut opsub = OP_NORMAL;
        let payload: &[Node] = match op.first() {
            Some(Node::MathChar { fam: 255, c, class: CL_OP, .. }) => {
                opsub = *c as u8;
                &op[1..]
            }
            _ => op,
        };
        let nuc = match payload {
            [Node::MathChar { fam, c, class: CL_OP, origin }] if *fam != 255 => {
                Nuc::Char { fam: *fam, c: *c, origin: origin.clone() }
            }
            [Node::DelimBox { small, size: 2, .. }] => Nuc::Char {
                fam: small.0,
                c: u32::from(small.1),
                origin: MathDiagnosticOrigin::default(),
            },
            _ => Nuc::Mlist(payload.to_vec()),
        };
        let mut noad = Noad::new(CL_OP, nuc);
        noad.opsub = opsub;
        noad.sup = above.clone();
        noad.sub = below.clone();
        Item::Noad(noad)
    }

    fn lm_decode_scripts(
        &mut self,
        nucleus: &[Node],
        sup: &Option<NodeList>,
        sub: &Option<NodeList>,
        _top: bool,
    ) -> Item {
        // an accent over a single character keeps its identity (scripts are
        // swapped into the accentee)
        if sup.is_some() || sub.is_some() {
            if let [Node::Accent { body, .. }] = nucleus {
                if matches!(body.as_slice(), [Node::MathChar { fam, .. }] if *fam != 255) {
                    return Item::Raw {
                        node: Node::Scripts { nucleus: nucleus.to_vec(), sup: sup.clone(), sub: sub.clone() },
                        frac: false,
                    };
                }
            }
        }
        let mut noad = match nucleus {
            [] => Noad::new(CL_ORD, Nuc::None),
            [Node::MathChar { fam: 255, c, class, .. }, rest @ ..] => {
                let mut n = Noad::new(*class, Nuc::Mlist(rest.to_vec()));
                if *class == CL_OP {
                    n.opsub = *c as u8;
                }
                n
            }
            [Node::MathChar { fam, c, class, origin }] => {
                let mut n = Noad::new(*class, Nuc::Char { fam: *fam, c: *c, origin: origin.clone() });
                if *class == CL_OP {
                    n.opsub = OP_NOLIMITS;
                }
                n
            }
            [Node::DelimBox { small, size: 2, .. }] => Noad::new(
                CL_ORD,
                Nuc::Char { fam: small.0, c: u32::from(small.1), origin: MathDiagnosticOrigin::default() },
            ),
            [Node::Box { .. }] => Noad::new(CL_ORD, Nuc::Box(nucleus[0].clone())),
            [Node::VCenter { box_node }] => {
                let mut n = Noad::new(CL_ORD, Nuc::Box((**box_node).clone()));
                n.special = Special::VCenter;
                n
            }
            [Node::Overline { body, under, .. }] => {
                let mut n = Noad::new(CL_ORD, Nuc::Mlist(body.clone()));
                n.special = if *under { Special::Under } else { Special::Over };
                n
            }
            other => Noad::new(CL_ORD, Nuc::Mlist(other.to_vec())),
        };
        noad.sup = sup.clone();
        noad.sub = sub.clone();
        Item::Noad(noad)
    }

    // ================= entry =================

    /// `run_mlist_to_hlist` / `mlist_to_hlist` for the LuaTeX engine.
    pub(crate) fn lm_mlist_to_hlist(&mut self, list: &[Node], style: GStyle, penalties: bool) -> NodeList {
        self.finalize_math_parameters();
        self.lm_convert(list, style, penalties, false)
    }

    fn lm_convert(&mut self, list: &[Node], start: GStyle, penalties: bool, fenced: bool) -> NodeList {
        let spliced = crate::math::splice_choices_pub(list, start);
        let list = spliced.as_deref().unwrap_or(list);
        let mut slots = self.lm_decode(list, fenced);
        let penalties = penalties || self.eqtb.int_params[IntParam::MathPenaltiesMode.idx() as usize] != 0;
        let (max_hl, max_d) = self.lm_pass1(&mut slots, start);
        self.lm_pass2(slots, start, penalties, max_hl, max_d)
    }

    // ================= pass 1 =================

    fn cur_mu_of(&mut self, g: GStyle) -> i32 {
        self.math_quad_style(g) / 18
    }

    fn lm_pass1(&mut self, slots: &mut Vec<Slot>, start: GStyle) -> (i32, i32) {
        #[derive(Clone, Copy, PartialEq)]
        enum R {
            Simple(u8),
            Fence { left: bool },
        }
        let style = start;
        let mut cur_style = start;
        let mut r: Option<usize> = None;
        let mut r_kind = R::Simple(CL_OP);
        let mut max_hl = 0i32;
        let mut max_d = 0i32;
        let mut cur_mu = self.math_quad_size(size_of_style(cur_style)) / 18;
        let mut i = 0usize;
        while i < slots.len() {
            let item = slots[i].item.clone();
            match item {
                Item::Style(s) => {
                    cur_style = s;
                    cur_mu = self.cur_mu_of(cur_style);
                }
                Item::NonScript => {
                    if size_of_style(cur_style) != 0 {
                        if let Some(Slot { item: Item::Other(Node::Glue(_) | Node::Kern(_) | Node::ExplicitKern(_)), .. }) =
                            slots.get(i + 1)
                        {
                            slots.remove(i + 1);
                        } else if let Some(Slot { item: Item::Other(Node::MuGlue(_) | Node::MathKern(_, 0)), .. }) =
                            slots.get(i + 1)
                        {
                            slots.remove(i + 1);
                        }
                    }
                }
                Item::Other(node) => {
                    match &node {
                        Node::Rule { height, depth, .. } => {
                            max_hl = max_hl.max(*height);
                            max_d = max_d.max(*depth);
                        }
                        Node::MuGlue(g) => {
                            let ng = self.lm_math_glue(g, cur_mu);
                            slots[i].item = Item::Other(Node::Glue(ng));
                        }
                        Node::MathKern(k, 0) => {
                            let k2 = mu_mult_kern(*k, cur_mu);
                            slots[i].item = Item::Other(italic_kern(k2));
                        }
                        _ => {}
                    }
                }
                Item::Fence { side, .. } => {
                    if side != Side::Left {
                        if let (Some(ri), R::Simple(CL_BIN)) = (r, r_kind) {
                            if let Item::Noad(rn) = &mut slots[ri].item {
                                rn.class = CL_ORD;
                            }
                        }
                    }
                    r = Some(i);
                    r_kind = R::Fence { left: true };
                    cur_style = style;
                    cur_mu = self.math_quad_size(size_of_style(cur_style)) / 18;
                }
                Item::Raw { node, frac } => {
                    let nodes = self.convert_atom(&node, cur_style, false);
                    let hl = if matches!(nodes.as_slice(), [Node::Box { .. }]) {
                        nodes
                    } else {
                        vec![hpack_nat(self, nodes)]
                    };
                    let (_, h, d) = hlist_dims(&hl, &self.eqtb);
                    max_hl = max_hl.max(h);
                    max_d = max_d.max(d);
                    slots[i].hlist = hl;
                    r = Some(i);
                    r_kind = R::Simple(if frac { CL_INNER } else { CL_ORD });
                }
                Item::Noad(_) => {
                    // RESWITCH loop
                    loop {
                        let Item::Noad(q) = &slots[i].item else { unreachable!() };
                        let class = q.class;
                        let special = q.special;
                        if special == Special::None && class == CL_BIN {
                            let demote = match r_kind {
                                R::Simple(c) => matches!(c, CL_BIN | CL_OP | CL_REL | CL_OPEN | CL_PUNCT),
                                R::Fence { left } => left,
                            };
                            if demote {
                                if let Item::Noad(q) = &mut slots[i].item {
                                    q.class = CL_ORD;
                                }
                                continue;
                            }
                        }
                        break;
                    }
                    let (class, special) = match &slots[i].item {
                        Item::Noad(q) => (q.class, q.special),
                        _ => unreachable!(),
                    };
                    let mut delta = 0;
                    let mut done_hlist = false;
                    match special {
                        Special::Over => self.lm_make_over(slots, i, cur_style),
                        Special::Under => self.lm_make_under(slots, i, cur_style),
                        Special::VCenter => self.lm_make_vcenter(slots, i, cur_style),
                        Special::None => match class {
                            CL_REL | CL_CLOSE | CL_PUNCT => {
                                if let (Some(ri), R::Simple(CL_BIN)) = (r, r_kind) {
                                    if let Item::Noad(rn) = &mut slots[ri].item {
                                        rn.class = CL_ORD;
                                    }
                                }
                            }
                            CL_OP => {
                                delta = self.lm_make_op(slots, i, cur_style);
                                if let Item::Noad(q) = &slots[i].item {
                                    done_hlist = q.opsub != OP_NORMAL;
                                }
                            }
                            CL_ORD => self.lm_make_ord(slots, i, cur_style),
                            _ => {}
                        },
                    }
                    if !done_hlist {
                        let (p, d2) = self.lm_check_nucleus(slots, i, cur_style);
                        if let Some(d2) = d2 {
                            delta = d2;
                        }
                        let has_scripts = matches!(&slots[i].item, Item::Noad(q) if q.has_scripts());
                        if !has_scripts {
                            slots[i].hlist = p;
                        } else {
                            self.lm_make_scripts(slots, i, p, delta, cur_style, 0, 0);
                        }
                    }
                    let (_, h, d) = hlist_dims(&slots[i].hlist, &self.eqtb);
                    max_hl = max_hl.max(h);
                    max_d = max_d.max(d);
                    r = Some(i);
                    let c = match &slots[i].item {
                        Item::Noad(q) => q.class,
                        _ => CL_ORD,
                    };
                    r_kind = R::Simple(c);
                }
            }
            i += 1;
        }
        if let (Some(ri), R::Simple(CL_BIN)) = (r, r_kind) {
            if let Item::Noad(rn) = &mut slots[ri].item {
                rn.class = CL_ORD;
            }
        }
        (max_hl, max_d)
    }

    /// `math_glue`
    fn lm_math_glue(&self, g: &Glue, m: i32) -> Glue {
        let mut n = m / 65536;
        let mut f = m % 65536;
        if f < 0 {
            n -= 1;
            f += 65536;
        }
        let mul = |a: i32| -> i32 {
            let v = i64::from(n) * i64::from(a) + i64::from(xn_over_d_signed(a, f, 65536));
            v.clamp(-0x3FFF_FFFF, 0x3FFF_FFFF) as i32
        };
        Glue::spec(
            mul(g.width),
            if g.stretch_order == 0 { mul(g.stretch) } else { g.stretch },
            g.stretch_order,
            if g.shrink_order == 0 { mul(g.shrink) } else { g.shrink },
            g.shrink_order,
        )
    }

    // ================= fetch / nucleus =================

    /// luatex `fetch`: the font and character of a math char at size `size`.
    fn lm_fetch(&mut self, fam: u8, c: u32, size: usize) -> (FontId, u32) {
        let f = self.fam_fnt(u32::from(fam), size);
        self.lm_cur_f = f;
        if f == 0 {
            let name = ["textfont", "scriptfont", "scriptscriptfont"][size.min(2)];
            self.error(&format!("\\{name}{fam} is undefined (character {c})"));
        } else if !self.mc_exists(f, c) {
            self.lua_glyph_not_found(f, c);
        }
        (f, c)
    }

    /// `check_nucleus_complexity`: the translated nucleus and the italic
    /// correction offset (`Some` only for character nuclei).
    fn lm_check_nucleus(&mut self, slots: &mut Vec<Slot>, i: usize, cur_style: GStyle) -> (NodeList, Option<i32>) {
        let Item::Noad(q) = &slots[i].item else {
            return (Vec::new(), None);
        };
        let size = size_of_style(cur_style);
        let has_scripts = q.has_scripts();
        let text_char = q.text_char;
        match q.nuc.clone() {
            Nuc::Char { fam, c, .. } => {
                let (f, c) = self.lm_fetch(fam, c, size);
                if !self.mc_exists(f, c) {
                    return (Vec::new(), Some(0));
                }
                let mut delta = self.mc_metrics(f, c).italic;
                let mut p = vec![self.glyph_node(f, c)];
                if self.assume_new_math(f) {
                } else if text_char && self.font_space_of(f) != 0 {
                    delta = 0;
                }
                if !has_scripts && delta != 0 {
                    p.push(italic_kern(delta));
                    delta = 0;
                }
                (p, Some(delta))
            }
            Nuc::Box(b) => (vec![b], None),
            Nuc::Mlist(l) => {
                let nodes = self.lm_convert(&l, cur_style, false, false);
                (vec![hpack_nat(self, nodes)], None)
            }
            Nuc::Fenced(l) => {
                let nodes = self.lm_convert(&l, cur_style, false, true);
                (vec![hpack_nat(self, nodes)], None)
            }
            Nuc::None => (Vec::new(), None),
        }
    }

    // ================= clean_box / rebox =================

    /// `clean_box` of a script/numerator list (a lone math char becomes an
    /// ord noad).
    pub(crate) fn lm_clean_list(&mut self, list: &[Node], s: GStyle) -> Node {
        let tmp;
        let list = match list {
            [Node::MathChar { fam, c, origin, class }] if *fam != 255 && *class != CL_ORD => {
                tmp = [Node::MathChar { fam: *fam, c: *c, class: CL_ORD, origin: origin.clone() }];
                &tmp[..]
            }
            _ => list,
        };
        let nodes = self.lm_convert(list, s, false, false);
        self.lm_finish_clean(nodes)
    }

    fn lm_finish_clean(&mut self, nodes: NodeList) -> Node {
        let mut x = if matches!(nodes.as_slice(), [Node::Box { shift: 0, .. }]) {
            nodes.into_iter().next().unwrap()
        } else {
            hpack_nat(self, nodes)
        };
        if let Node::Box { list, .. } = &mut x {
            if list.len() == 2 && is_char_node(&list[0]) && matches!(list[1], Node::Kern(_) | Node::ItalicKern(_)) {
                list.pop();
            }
        }
        x
    }

    fn lm_clean_nuc(&mut self, nuc: &Nuc, s: GStyle) -> Node {
        match nuc {
            Nuc::Char { fam, c, origin } => {
                let l = [Node::MathChar { fam: *fam, c: *c, class: CL_ORD, origin: origin.clone() }];
                let nodes = self.lm_convert(&l, s, false, false);
                self.lm_finish_clean(nodes)
            }
            Nuc::Box(b) => {
                let nodes = vec![b.clone()];
                self.lm_finish_clean(nodes)
            }
            Nuc::Mlist(l) => {
                let nodes = self.lm_convert(l, s, false, false);
                self.lm_finish_clean(nodes)
            }
            Nuc::Fenced(l) => {
                let nodes = self.lm_convert(l, s, false, true);
                self.lm_finish_clean(nodes)
            }
            Nuc::None => {
                let x = null_box(HBOX);
                x
            }
        }
    }

    /// `rebox`
    pub(crate) fn lm_rebox(&mut self, b: Node, w: i32) -> Node {
        let (bw, _, _) = box_whd(&b);
        let nonempty = matches!(&b, Node::Box { list, .. } if !list.is_empty());
        if bw != w && nonempty {
            let mut b = b;
            if matches!(&b, Node::Box { kind, .. } if *kind == VBOX) {
                b = hpack_nat(self, vec![b]);
            }
            let (bw, _, _) = box_whd(&b);
            let Node::Box { mut list, .. } = b else { unreachable!() };
            if list.len() == 1 && is_char_node(&list[0]) {
                let (f, c) = glyph_fc(&list[0]);
                let v = self.mc_metrics(f, c).width;
                if v != bw {
                    list.push(Node::Kern(bw - v));
                }
            }
            let ss = || {
                Node::Glue(Glue::spec(0, 65536, crate::boxes::GLUE_FIL, 65536, crate::boxes::GLUE_FIL))
            };
            let mut l2 = vec![ss()];
            l2.extend(list);
            l2.push(ss());
            crate::boxes::hpack(l2, Some(w), HBOX, &self.eqtb).node
        } else {
            let mut b = b;
            if let Node::Box { w: bw, .. } = &mut b {
                *bw = w;
            }
            b
        }
    }

    // ================= over / under / vcenter =================

    fn lm_rule(t: i32) -> Node {
        Node::Rule { width: crate::build::RULE_FILL, height: t, depth: 0 }
    }

    fn lm_nuc_of(&self, slots: &[Slot], i: usize) -> Nuc {
        match &slots[i].item {
            Item::Noad(q) => q.nuc.clone(),
            _ => Nuc::None,
        }
    }

    fn lm_set_nuc_box(slots: &mut [Slot], i: usize, b: Node) {
        if let Item::Noad(q) = &mut slots[i].item {
            q.nuc = Nuc::Box(b);
        }
    }

    fn lm_make_over(&mut self, slots: &mut Vec<Slot>, i: usize, cur_style: GStyle) {
        let thickness = self.mparam_err(MATH_PARAM_OVERBAR_RULE, cur_style);
        let nuc = self.lm_nuc_of(slots, i);
        let b = self.lm_clean_nuc(&nuc, cramped(cur_style));
        let vgap = self.mparam_err(MATH_PARAM_OVERBAR_VGAP, cur_style);
        let kern = self.mparam_err(MATH_PARAM_OVERBAR_KERN, cur_style);
        // overbar(b, k = vgap, t, ht = kern)
        let v = vpack_nat(self, vec![Node::Kern(kern), Self::lm_rule(thickness), Node::Kern(vgap), b]);
        Self::lm_set_nuc_box(slots, i, v);
    }

    fn lm_make_under(&mut self, slots: &mut Vec<Slot>, i: usize, cur_style: GStyle) {
        let thickness = self.mparam_err(MATH_PARAM_UNDERBAR_RULE, cur_style);
        let nuc = self.lm_nuc_of(slots, i);
        let x = self.lm_clean_nuc(&nuc, cur_style);
        let vgap = self.mparam_err(MATH_PARAM_UNDERBAR_VGAP, cur_style);
        let (_, xh, _) = box_whd(&x);
        let mut y = vpack_nat(self, vec![x, Node::Kern(vgap), Self::lm_rule(thickness)]);
        let kern = self.mparam_err(MATH_PARAM_UNDERBAR_KERN, cur_style);
        let (_, yh, yd) = box_whd(&y);
        let delta = yh + yd + kern;
        if let Node::Box { h, d, .. } = &mut y {
            *h = xh;
            *d = delta - xh;
        }
        Self::lm_set_nuc_box(slots, i, y);
    }

    fn lm_make_vcenter(&mut self, slots: &mut Vec<Slot>, i: usize, cur_style: GStyle) {
        let size = size_of_style(cur_style);
        let axis = self.math_axis_size(size);
        if let Item::Noad(q) = &mut slots[i].item {
            if let Nuc::Box(Node::Box { kind, h, d, .. }) = &mut q.nuc {
                if *kind == VBOX {
                    let delta = *h + *d;
                    *h = axis + half(delta);
                    *d = delta - *h;
                }
            }
        }
    }

    // ================= make_ord =================

    fn lm_make_ord(&mut self, slots: &mut Vec<Slot>, i: usize, cur_style: GStyle) {
        let size = size_of_style(cur_style);
        loop {
            let (fam, _) = match &slots[i].item {
                Item::Noad(q) if !q.has_scripts() => match &q.nuc {
                    Nuc::Char { fam, c, .. } => (*fam, *c),
                    _ => return,
                },
                _ => return,
            };
            let pc = match slots.get(i + 1) {
                Some(Slot { item: Item::Noad(p), .. })
                    if p.special == Special::None && p.class <= CL_PUNCT =>
                {
                    match &p.nuc {
                        Nuc::Char { fam: pf, c, .. } if *pf == fam => *c,
                        _ => return,
                    }
                }
                _ => return,
            };
            let (a_fam, a_c) = match &mut slots[i].item {
                Item::Noad(q) => {
                    q.text_char = true;
                    match &q.nuc {
                        Nuc::Char { fam, c, .. } => (*fam, *c),
                        _ => return,
                    }
                }
                _ => return,
            };
            let (cur_f, a) = self.lm_fetch(a_fam, a_c, size);
            let kl = self.mc_kern_lig(cur_f, a, pc);
            let no_ligs = self.eqtb.int_params[IntParam::NoLigs.idx() as usize];
            let no_kerns = self.eqtb.int_params[IntParam::NoKerns.idx() as usize];
            if no_ligs == 0 {
                if let Some((ltype, repl)) = kl.lig {
                    match ltype {
                        1 | 5 => set_nuc_char(&mut slots[i], repl),
                        2 | 6 => set_nuc_char(&mut slots[i + 1], repl),
                        3 | 7 | 11 => {
                            let origin = match &slots[i].item {
                                Item::Noad(q) => match &q.nuc {
                                    Nuc::Char { origin, .. } => origin.clone(),
                                    _ => MathDiagnosticOrigin::default(),
                                },
                                _ => MathDiagnosticOrigin::default(),
                            };
                            let mut r = Noad::new(CL_ORD, Nuc::Char { fam, c: repl, origin });
                            r.text_char = ltype >= 11;
                            slots.insert(i + 1, Slot::new(Item::Noad(r)));
                        }
                        _ => {
                            let p = slots.remove(i + 1);
                            set_nuc_char(&mut slots[i], repl);
                            if let (Item::Noad(q), Item::Noad(p)) = (&mut slots[i].item, p.item) {
                                q.sub = p.sub;
                                q.sup = p.sup;
                            }
                        }
                    }
                    if ltype > 3 {
                        return;
                    }
                    if let Item::Noad(q) = &mut slots[i].item {
                        q.text_char = false;
                    }
                    continue;
                }
            }
            if no_kerns == 0 {
                if let Some(k) = kl.kern {
                    if k != 0 {
                        slots.insert(i + 1, Slot::new(Item::Other(Node::Kern(k))));
                        return;
                    }
                }
            }
            return;
        }
    }

    // ================= make_op =================

    /// luatex `make_op`; returns the italic correction offset.
    fn lm_make_op(&mut self, slots: &mut Vec<Slot>, i: usize, cur_style: GStyle) -> i32 {
        let size = size_of_style(cur_style);
        let mut delta = 0;
        let (mut opsub, nuc, has_sub, has_sup) = match &slots[i].item {
            Item::Noad(q) => (q.opsub, q.nuc.clone(), q.sub.is_some(), q.sup.is_some()),
            _ => return 0,
        };
        if opsub == OP_NORMAL && cur_style < 2 {
            opsub = OP_LIMITS;
            if let Item::Noad(q) = &mut slots[i].item {
                q.opsub = OP_LIMITS;
            }
        }
        let mut cur_f = self.lm_cur_f;
        if let Nuc::Char { fam, c, origin } = nuc.clone() {
            let (f, mut cc) = self.lm_fetch(fam, c, size);
            cur_f = f;
            let mut x;
            let mut axis_shift = false;
            if cur_style < 2 {
                let ok_size = self.mparam(MATH_PARAM_OPERATOR_SIZE, cur_style);
                if ok_size != UNDEFINED_MATH_PARAMETER {
                    let (xb, info) = self.do_delimiter(Some((fam, cc, 0, 0)), 0, ok_size, false, cur_style, true, 0);
                    x = xb;
                    delta = info.delta;
                    if delta != 0 && has_sub && opsub != OP_LIMITS {
                        if let Node::Box { w, .. } = &mut x {
                            *w -= delta;
                        }
                    }
                } else {
                    let m = self.mc_metrics(f, cc);
                    let ok = m.height + m.depth + 1;
                    while let CharTag::List(next) = self.mc_tag(f, cc) {
                        let mm = self.mc_metrics(f, cc);
                        if mm.height + mm.depth >= ok {
                            break;
                        }
                        if !self.mc_exists(f, next) {
                            break;
                        }
                        cc = next;
                    }
                    delta = self.mc_metrics(f, cc).italic;
                    let tmp = Nuc::Char { fam, c: cc, origin: origin.clone() };
                    x = self.lm_clean_nuc(&tmp, cur_style);
                    if delta != 0 && has_sub && opsub != OP_LIMITS {
                        if let Node::Box { w, .. } = &mut x {
                            *w -= delta;
                        }
                    }
                    axis_shift = true;
                }
            } else {
                delta = self.mc_metrics(f, cc).italic;
                x = self.lm_clean_nuc(&nuc, cur_style);
                if delta != 0 && has_sub && opsub != OP_LIMITS {
                    if let Node::Box { w, .. } = &mut x {
                        *w -= delta;
                    }
                }
                axis_shift = true;
            }
            if axis_shift {
                let (_, h, d) = box_whd(&x);
                let axis = self.math_axis_size(size);
                set_shift(&mut x, half(h - d) - axis);
            }
            Self::lm_set_nuc_box(slots, i, x);
        }
        let opentype = self.assume_new_math(cur_f);
        if opsub == OP_NOLIMITS {
            let (mut p, d2) = self.lm_check_nucleus(slots, i, cur_style);
            if let Some(d2) = d2 {
                delta = d2;
            }
            if opentype {
                // width(p) -= delta: p is a single box here
                if let Some(Node::Box { w, .. }) = p.first_mut() {
                    *w -= delta;
                }
            }
            if !has_sub && !has_sup {
                slots[i].hlist = p;
            } else {
                self.lm_make_scripts(slots, i, p, delta, cur_style, 0, 0);
            }
        } else if opsub == OP_LIMITS {
            let (sup, sub) = match &slots[i].item {
                Item::Noad(q) => (q.sup.clone(), q.sub.clone()),
                _ => (None, None),
            };
            let nuc = self.lm_nuc_of(slots, i);
            let x0 = self.lm_clean_list_opt(sup.as_deref(), sup_style(cur_style));
            let y0 = self.lm_clean_nuc(&nuc, cur_style);
            let z0 = self.lm_clean_list_opt(sub.as_deref(), sub_style(cur_style));
            let mut width = box_whd(&y0).0;
            width = width.max(box_whd(&x0).0).max(box_whd(&z0).0);
            let mut x = self.lm_rebox(x0, width);
            let y = self.lm_rebox(y0, width);
            let mut z = self.lm_rebox(z0, width);
            set_shift(&mut x, half(delta));
            set_shift(&mut z, -half(delta));
            let (_, yh, yd) = box_whd(&y);
            let mut vh = yh;
            let mut vd = yd;
            let mut list: NodeList = Vec::new();
            if sup.is_none() {
                list.push(y);
            } else {
                let (_, xh, xd) = box_whd(&x);
                let bgap = self.mparam_err(MATH_PARAM_LIMIT_ABOVE_BGAP, cur_style);
                let vgap = self.mparam_err(MATH_PARAM_LIMIT_ABOVE_VGAP, cur_style);
                let kern = self.mparam_err(MATH_PARAM_LIMIT_ABOVE_KERN, cur_style);
                let mut shift_up = bgap - xd;
                if shift_up < vgap {
                    shift_up = vgap;
                }
                list.push(Node::Kern(kern));
                list.push(x);
                list.push(Node::Kern(shift_up));
                list.push(y);
                vh = vh + kern + xh + xd + shift_up;
            }
            if sub.is_some() {
                let (_, zh, zd) = box_whd(&z);
                let bgap = self.mparam_err(MATH_PARAM_LIMIT_BELOW_BGAP, cur_style);
                let vgap = self.mparam_err(MATH_PARAM_LIMIT_BELOW_VGAP, cur_style);
                let kern = self.mparam_err(MATH_PARAM_LIMIT_BELOW_KERN, cur_style);
                let mut shift_down = bgap - zh;
                if shift_down < vgap {
                    shift_down = vgap;
                }
                list.push(Node::Kern(shift_down));
                list.push(z);
                list.push(Node::Kern(kern));
                vd = vd + kern + zh + zd + shift_down;
            }
            let mut v = vpack_nat(self, list);
            if let Node::Box { w, h, d, .. } = &mut v {
                *w = width;
                *h = vh;
                *d = vd;
            }
            slots[i].hlist = vec![v];
            if let Item::Noad(q) = &mut slots[i].item {
                q.sup = None;
                q.sub = None;
            }
        }
        delta
    }

    fn lm_clean_list_opt(&mut self, l: Option<&[Node]>, s: GStyle) -> Node {
        match l {
            Some(l) => self.lm_clean_list(l, s),
            None => null_box(HBOX),
        }
    }

    // ================= make_scripts =================

    #[allow(clippy::too_many_arguments)]
    fn lm_make_scripts(
        &mut self,
        slots: &mut Vec<Slot>,
        i: usize,
        mut p: NodeList,
        it: i32,
        cur_style: GStyle,
        supshift: i32,
        subshift: i32,
    ) {
        let size = size_of_style(cur_style);
        let (sup, sub, nuc_is_char) = match &slots[i].item {
            Item::Noad(q) => (q.sup.clone(), q.sub.clone(), matches!(q.nuc, Nuc::Char { .. })),
            _ => return,
        };
        let mut shift_up;
        let mut shift_down;
        let mut delta1 = it;
        if nuc_is_char && sub.is_none() && delta1 != 0 {
            p.push(italic_kern(delta1));
            delta1 = 0;
        }
        let p_is_char = p.first().is_some_and(is_char_node);
        if p_is_char {
            shift_up = 0;
            shift_down = 0;
        } else {
            let (_, zh, zd) = hlist_dims(&p, &self.eqtb);
            let drop_up = self.mparam_err(MATH_PARAM_SUP_SHIFT_DROP, cur_style);
            let drop_down = self.mparam_err(MATH_PARAM_SUB_SHIFT_DROP, cur_style);
            shift_up = zh - drop_up;
            shift_down = zd + drop_down;
        }
        let mut sub_fc: Option<(FontId, u32)> = None;
        let mut sup_fc: Option<(FontId, u32)> = None;
        if p_is_char {
            sub_fc = self.lm_analyze_script(sub.as_deref(), size);
            sup_fc = self.lm_analyze_script(sup.as_deref(), size);
        }
        let (pf, pc) = p.first().map(glyph_fc_opt).unwrap_or((0, 0));
        let mode = self.eqtb.int_params[IntParam::MathScriptsMode.idx() as usize];
        let space_after = self.mparam_err(MATH_PARAM_SPACE_AFTER_SCRIPT, cur_style);
        let x: Node;
        let mut kern_after: Vec<Node> = Vec::new();
        if sup.is_none() {
            let mut xb = self.lm_clean_list(sub.as_deref().unwrap_or(&[]), sub_style(cur_style));
            if let Node::Box { w, .. } = &mut xb {
                *w += space_after;
            }
            let sdown = self.mparam_err(MATH_PARAM_SUB_SHIFT_DOWN, cur_style);
            let ssdown = self.mparam_err(MATH_PARAM_SUB_SUP_SHIFT_DOWN, cur_style);
            match mode {
                1 | 5 => shift_down = sdown,
                2 | 3 => shift_down = ssdown,
                4 => shift_down = sdown + half(ssdown - sdown),
                _ => {
                    if shift_down < sdown {
                        shift_down = sdown;
                    }
                    let top_max = self.mparam_err(MATH_PARAM_SUB_TOP_MAX, cur_style);
                    let clr = box_whd(&xb).1 - top_max;
                    if shift_down < clr {
                        shift_down = clr;
                    }
                }
            }
            set_shift(&mut xb, shift_down);
            let mut d2 = subshift;
            if let Some((sf, sc)) = sub_fc {
                if let Some(k) = self.find_math_kern(pf, pc, sf, sc, false, shift_down) {
                    d2 = k + subshift;
                }
            }
            if d2 != 0 {
                kern_after.push(Node::Kern(d2));
            }
            x = xb;
        } else {
            let mut xb = self.lm_clean_list(sup.as_deref().unwrap(), sup_style(cur_style));
            if let Node::Box { w, .. } = &mut xb {
                *w += space_after;
            }
            let sup_up = self.mparam_err(MATH_PARAM_SUP_SHIFT_UP, cur_style);
            let sdown = self.mparam_err(MATH_PARAM_SUB_SHIFT_DOWN, cur_style);
            let ssdown = self.mparam_err(MATH_PARAM_SUB_SUP_SHIFT_DOWN, cur_style);
            match mode {
                1 | 2 => shift_up = sup_up,
                3 | 5 => shift_up = sup_up + ssdown - sdown,
                4 => shift_up = sup_up + half(ssdown - sdown),
                _ => {
                    if shift_up < sup_up {
                        shift_up = sup_up;
                    }
                    let bmin = self.mparam_err(MATH_PARAM_SUP_BOTTOM_MIN, cur_style);
                    let clr = box_whd(&xb).2 + bmin;
                    if shift_up < clr {
                        shift_up = clr;
                    }
                }
            }
            if sub.is_none() {
                set_shift(&mut xb, -shift_up);
                let mut clr = supshift;
                if let Some((sf, sc)) = sup_fc {
                    if let Some(k) = self.find_math_kern(pf, pc, sf, sc, true, shift_up) {
                        clr = k + supshift;
                    }
                }
                if clr != 0 {
                    kern_after.push(Node::Kern(clr));
                }
                x = xb;
            } else {
                let mut yb = self.lm_clean_list(sub.as_deref().unwrap(), sub_style(cur_style));
                if let Node::Box { w, .. } = &mut yb {
                    *w += space_after;
                }
                match mode {
                    1 | 5 => shift_down = sdown,
                    2 | 3 => shift_down = ssdown,
                    4 => shift_down = sdown + half(ssdown - sdown),
                    _ => {
                        if shift_down < ssdown {
                            shift_down = ssdown;
                        }
                        let vgap = self.mparam_err(MATH_PARAM_SUBSUP_VGAP, cur_style);
                        let (_, yh, _) = box_whd(&yb);
                        let (_, _, xd) = box_whd(&xb);
                        let mut clr = vgap - ((shift_up - xd) - (yh - shift_down));
                        if clr > 0 {
                            shift_down += clr;
                            let maxb = self.mparam_err(MATH_PARAM_SUP_SUB_BOTTOM_MAX, cur_style);
                            clr = maxb - (shift_up - xd);
                            if clr > 0 {
                                shift_up += clr;
                                shift_down -= clr;
                            }
                        }
                    }
                }
                let mut d2 = subshift;
                let mut have_d2 = true;
                if let Some((sf, sc)) = sub_fc {
                    if let Some(k) = self.find_math_kern(pf, pc, sf, sc, false, shift_down) {
                        d2 = k + subshift;
                    }
                }
                if d2 != 0 {
                    kern_after.push(Node::Kern(d2));
                }
                let _ = &mut have_d2;
                let mut clr = None;
                if let Some((sf, sc)) = sup_fc {
                    clr = self.find_math_kern(pf, pc, sf, sc, true, shift_up);
                }
                let d2b = d2 - supshift;
                let shift_x = match clr {
                    Some(c) => c + delta1 - d2b,
                    None => delta1 - d2b,
                };
                set_shift(&mut xb, shift_x);
                let (_, yh, _) = box_whd(&yb);
                let (_, _, xd) = box_whd(&xb);
                let gap = (shift_up - xd) - (yh - shift_down);
                let mut v = vpack_nat(self, vec![xb, Node::Kern(gap), yb]);
                set_shift(&mut v, shift_down);
                x = v;
            }
        }
        let mut hl = p;
        hl.extend(kern_after);
        hl.push(x);
        slots[i].hlist = hl;
        if let Item::Noad(q) = &mut slots[i].item {
            q.sup = None;
            q.sub = None;
        }
    }

    /// `analyze_script`: the character the italic/kern logic looks at.
    fn lm_analyze_script(&mut self, su: Option<&[Node]>, size: usize) -> Option<(FontId, u32)> {
        let su = su?;
        let size = if size < 2 { size + 1 } else { size };
        let char_mode = self.eqtb.int_params[IntParam::MathScriptCharMode.idx() as usize];
        let box_mode = self.eqtb.int_params[IntParam::MathScriptBoxMode.idx() as usize];
        match su {
            [Node::MathChar { fam, c, class: _, .. }] if *fam != 255 && char_mode > 0 => {
                let (f, c) = self.lm_fetch(*fam, *c, size);
                self.mc_exists(f, c).then_some((f, c))
            }
            _ if box_mode > 0 => {
                for n in su {
                    match n {
                        Node::Kern(_) | Node::ExplicitKern(_) | Node::Glue(_) | Node::MuGlue(_) | Node::MathKern(..) => {}
                        Node::MathChar { fam, c, .. } if *fam != 255 => {
                            let (f, c) = self.lm_fetch(*fam, *c, size);
                            return self.mc_exists(f, c).then_some((f, c));
                        }
                        Node::Scripts { nucleus, .. } => {
                            if let [Node::MathChar { fam, c, .. }] = nucleus.as_slice() {
                                if *fam != 255 {
                                    let (f, c) = self.lm_fetch(*fam, *c, size);
                                    return self.mc_exists(f, c).then_some((f, c));
                                }
                            }
                            return None;
                        }
                        _ => return None,
                    }
                }
                None
            }
            _ => None,
        }
    }

    // ================= pass 2 =================

    fn lm_spacing_glue(&mut self, l: u8, r: u8, style: GStyle, mmu: i32) -> Option<Node> {
        let id = MATH_PARAM_ORD_ORD_SPACING + u32::from(l.min(7)) * 8 + u32::from(r.min(7));
        let v = self.eqtb.math_glue_param(id, style)?;
        match v[0] {
            0 => {
                let g = Glue::spec(v[1], v[2], v[4] as u8, v[3], v[5] as u8);
                let mut ng = self.lm_math_glue(&g, mmu);
                ng.subtype = 0;
                Some(Node::Glue(ng))
            }
            k => {
                let (param, subtype) = match k {
                    1 => (crate::prim::GlueParam::ThinMuSkip, crate::boxes::glue_subtype::THIN_MU_SKIP),
                    2 => (crate::prim::GlueParam::MedMuSkip, crate::boxes::glue_subtype::MED_MU_SKIP),
                    _ => (crate::prim::GlueParam::ThickMuSkip, crate::boxes::glue_subtype::THICK_MU_SKIP),
                };
                let src = self.eqtb.glue_params[param.idx() as usize];
                let mut ng = self.lm_math_glue(&src, mmu);
                ng.subtype = subtype;
                Some(Node::Glue(ng))
            }
        }
    }

    fn lm_pass2(&mut self, slots: Vec<Slot>, start: GStyle, penalties: bool, max_hl: i32, max_d: i32) -> NodeList {
        let style = start;
        let mut cur_style = start;
        let mut cur_mu = self.math_quad_size(size_of_style(cur_style)) / 18;
        let mut out: NodeList = Vec::new();
        let mut r_type: Option<u8> = None;
        let bin_pen = self.eqtb.int_params[IntParam::BinOpPenalty.idx() as usize];
        let rel_pen = self.eqtb.int_params[IntParam::RelPenalty.idx() as usize];
        let pre_bin = self.eqtb.int_params[IntParam::PreBinOpPenalty.idx() as usize];
        let pre_rel = self.eqtb.int_params[IntParam::PreRelPenalty.idx() as usize];
        let n = slots.len();
        let delim_mode = self.eqtb.int_params[IntParam::MathDelimitersMode.idx() as usize];
        for idx in 0..n {
            let slot = &slots[idx];
            let mut t: u8 = CL_ORD;
            let mut pen = 10000;
            let mut prepen = 10000;
            match &slot.item {
                Item::Style(s) => {
                    cur_style = *s;
                    cur_mu = self.cur_mu_of(cur_style);
                    continue;
                }
                Item::NonScript => {
                    out.push(Node::Glue(Glue::zero()));
                    continue;
                }
                Item::Other(node) => {
                    out.push(node.clone());
                    continue;
                }
                Item::Noad(q) => {
                    t = q.class;
                    match q.class {
                        CL_BIN if q.special == Special::None => {
                            pen = bin_pen;
                            prepen = pre_bin;
                        }
                        CL_REL if q.special == Special::None => {
                            pen = rel_pen;
                            prepen = pre_rel;
                        }
                        _ => {}
                    }
                    if q.special != Special::None {
                        t = CL_ORD;
                    }
                }
                Item::Raw { frac, .. } => {
                    t = if *frac { CL_INNER } else { CL_ORD };
                }
                Item::Fence { .. } => {}
            }
            let mut new_hlist: NodeList = slot.hlist.clone();
            let mut fence_right = false;
            if let Item::Fence { side, delim } = &slot.item {
                let (hl, class) = self.lm_make_left_right(*side, *delim, style, max_d, max_hl);
                new_hlist = hl;
                t = class;
                fence_right = *side == Side::Right;
            }
            if let Some(rt) = r_type {
                let mut tt = t;
                if delim_mode & 0x04 != 0 && t == CL_INNER && false {
                    tt = CL_ORD;
                }
                if let Some(z) = self.lm_spacing_glue(rt, tt, cur_style, cur_mu) {
                    out.push(z);
                }
                if penalties && prepen < 10000 && !matches!(out.last(), Some(Node::Penalty(_))) {
                    out.push(Node::Penalty(prepen));
                }
            }
            out.extend(new_hlist);
            if penalties && idx + 1 < n && pen < 10000 {
                let next_is_pen_or_rel = match &slots[idx + 1].item {
                    Item::Other(Node::Penalty(_)) => true,
                    Item::Noad(nq) => nq.class == CL_REL && nq.special == Special::None,
                    _ => false,
                };
                if !next_is_pen_or_rel {
                    out.push(Node::Penalty(pen));
                }
            }
            if fence_right {
                t = CL_OPEN;
            }
            r_type = Some(t);
        }
        out
    }

    /// `make_left_right`: the delimiter box and the spacing class.
    fn lm_make_left_right(
        &mut self,
        side: Side,
        delim: Option<(u8, u32, u8, u32)>,
        style: GStyle,
        max_d: i32,
        max_h: i32,
    ) -> (NodeList, u8) {
        let size = size_of_style(style);
        let axis = true;
        let delta = self.lm_delimiter_height(max_d, max_h, axis, size);
        let same = match side {
            Side::Left => 1,
            Side::Middle => 2,
            Side::Right => 3,
        };
        let (tmp, _info) = self.do_delimiter(delim, size, delta, false, style, axis, same);
        let class = if side == Side::Left { CL_OPEN } else { CL_CLOSE };
        (vec![tmp], class)
    }

    /// `get_delimiter_height`
    fn lm_delimiter_height(&mut self, max_d: i32, max_h: i32, axis: bool, size: usize) -> i32 {
        let delta2 = if axis { max_d + self.math_axis_size(size) } else { max_d };
        let mut delta1 = max_h + max_d - delta2;
        if delta2 > delta1 {
            delta1 = delta2;
        }
        let factor = self.eqtb.int_params[IntParam::DelimiterFactor.idx() as usize];
        let shortfall = self.eqtb.dim_params[DimParam::DelimiterShortfall.idx() as usize];
        let delta = (i64::from(delta1) / 500) * i64::from(factor);
        let delta2 = i64::from(delta1) + i64::from(delta1) - i64::from(shortfall);
        delta.max(delta2).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    }
}

fn glyph_fc(n: &Node) -> (FontId, u32) {
    match n {
        Node::Char { c, font } => (*font, u32::from(*c)),
        Node::LuaGlyph(g) => (g.font, g.c),
        _ => (0, 0),
    }
}

fn glyph_fc_opt(n: &Node) -> (FontId, u32) {
    glyph_fc(n)
}

fn set_nuc_char(slot: &mut Slot, c: u32) {
    if let Item::Noad(q) = &mut slot.item {
        if let Nuc::Char { c: cc, .. } = &mut q.nuc {
            *cc = c;
        }
    }
}

fn mu_mult_kern(k: i32, m: i32) -> i32 {
    let mut n = m / 65536;
    let mut f = m % 65536;
    if f < 0 {
        n -= 1;
        f += 65536;
    }
    let v = i64::from(n) * i64::from(k) + i64::from(xn_over_d_signed(k, f, 65536));
    v.clamp(-0x3FFF_FFFF, 0x3FFF_FFFF) as i32
}

fn xn_over_d_signed(x: i32, n: i32, d: i32) -> i32 {
    let r = (i64::from(x).abs() * i64::from(n) / i64::from(d)) as i32;
    if x < 0 {
        -r
    } else {
        r
    }
}

/// A right-delimiter marker (possibly wrapped in scripts): the bare marker
/// and the wrapper's scripts.
fn close_marker(n: &Node) -> Option<(&Node, Option<NodeList>, Option<NodeList>)> {
    match n {
        Node::DelimBox { size: 1, .. } => Some((n, None, None)),
        Node::Scripts { nucleus, sup, sub } => match nucleus.as_slice() {
            [m @ Node::DelimBox { size: 1, .. }] => Some((m, sup.clone(), sub.clone())),
            _ => None,
        },
        Node::OpLimits { op, above, below } => match op.as_slice() {
            [m @ Node::DelimBox { size: 1, .. }] => Some((m, above.clone(), below.clone())),
            _ => None,
        },
        _ => None,
    }
}

#[allow(dead_code)]
fn _unused(_: MathStyle) {}
