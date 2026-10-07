//! Lua-visible math noads: ids 16-27 of the node library (`style`, `choice`,
//! `noad`, `radical`, `fraction`, `accent`, `fence`, `math_char`, `sub_box`,
//! `sub_mlist`, `math_text_char`, `delim`) and their conversion from and to
//! the engine's flat math lists (the noad variants of `boxes::Node`).
//!
//! The engine keeps `\left...\right` groups flat (a pair of `DelimBox`
//! markers) and writes the scripts of a noad into a wrapping `Scripts`
//! node; LuaTeX has a fence noad inside a `sub_mlist` of an inner noad and
//! scripts on every noad. [`Engine::import_math_list`] regroups, and
//! [`Engine::export_math_node`] splices back.

use crate::boxes::{AccentSpec, Delim, FenceOpts, MathDiagnosticOrigin, Node, NodeList};
use crate::engine::Engine;
use crate::lua_callbacks::{Cb, CbArg, CbRet};
use crate::lua_node::*;
use crate::lua_node_conv::LangCtx;
use crate::math::{gstyle_of, math_style_of, GStyle, CL_BIN, CL_CLOSE, CL_INNER, CL_OP, CL_ORD, CL_OPEN, CL_PUNCT, CL_REL};

/// luatex `noad` subtypes beyond the eight classes
const SUB_UNDER: u16 = 10;
const SUB_OVER: u16 = 11;
const SUB_VCENTER: u16 = 12;

/// luatex `fence` subtypes
const FENCE_LEFT: u16 = 1;
const FENCE_MIDDLE: u16 = 2;
const FENCE_RIGHT: u16 = 3;
const FENCE_NO: u16 = 4;

/// An engine node that is a noad (or a math style/choice node) for Lua.
pub(crate) fn is_math_node(n: &Node) -> bool {
    matches!(
        n,
        Node::MathChar { .. }
            | Node::Scripts { .. }
            | Node::OpLimits { .. }
            | Node::Frac { .. }
            | Node::Radical { .. }
            | Node::Accent { .. }
            | Node::DelimBox { .. }
            | Node::Style(_, _)
            | Node::Choice
            | Node::Overline { .. }
            | Node::VCenter { .. }
    )
}

/// noad subtype (op subtype aside) of an engine math class
fn lua_subtype_of_class(class: u8) -> u16 {
    match class {
        CL_OP => 1,
        CL_BIN => 4,
        CL_REL => 5,
        CL_OPEN => 6,
        CL_CLOSE => 7,
        CL_PUNCT => 8,
        CL_INNER => 9,
        _ => 0,
    }
}

/// the engine class of a noad subtype, and the limits request of an
/// operator (`c` of the fam-255 marker: 0 normal, 1 limits, 2 no limits)
fn class_of_lua_subtype(sub: u16) -> (u8, u8) {
    match sub {
        1 => (CL_OP, 0),
        2 => (CL_OP, 1),
        3 => (CL_OP, 2),
        4 => (CL_BIN, 0),
        5 => (CL_REL, 0),
        6 => (CL_OPEN, 0),
        7 => (CL_CLOSE, 0),
        8 => (CL_PUNCT, 0),
        9 => (CL_INNER, 0),
        _ => (CL_ORD, 0),
    }
}

/// the matching close marker of an open `\left`/`\middle` marker at `list[i]`
/// (possibly wrapped in the scripts of the group)
fn close_marker_at(list: &[Node], i: usize) -> Option<usize> {
    let mut depth = 1usize;
    for (j, n) in list.iter().enumerate().skip(i + 1) {
        match n {
            Node::DelimBox { size: 0, .. } => depth += 1,
            Node::DelimBox { size: 1, .. } => depth -= 1,
            Node::Scripts { nucleus, .. } if matches!(nucleus.as_slice(), [Node::DelimBox { size: 1, .. }]) => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return Some(j);
        }
    }
    None
}

const STYLE_NAMES: [&str; 8] = [
    "display",
    "crampeddisplay",
    "text",
    "crampedtext",
    "script",
    "crampedscript",
    "scriptscript",
    "crampedscriptscript",
];

impl Engine {
    /// mlist.c `run_mlist_to_hlist`: the `mlist_to_hlist` callback converts
    /// the formula when Lua defined one (it gets the noad list, the style
    /// name and the penalties flag, and returns the hlist), otherwise the
    /// built-in conversion runs.
    pub(crate) fn run_mlist_to_hlist(&mut self, list: &[Node], style: GStyle, penalties: bool) -> NodeList {
        if self.engine_kind != crate::engine::EngineKind::LuaTeX || list.is_empty() || !self.cb_defined(Cb::MlistToHlist) {
            return self.mlist_to_hlist_pen(list, style, penalties);
        }
        self.finalize_math_parameters();
        let first = self.lua_nodes_from_engine(list.to_vec()) as u32;
        let args = vec![CbArg::Node(first), CbArg::str(STYLE_NAMES[usize::from(style & 7)]), CbArg::Bool(penalties)];
        match self.lua_cb_call(Cb::MlistToHlist, "mlist to hlist", args).as_deref().and_then(|r| r.first()) {
            Some(CbRet::Node(h)) => self.lua_nodes_to_engine(i64::from(*h)),
            _ => Vec::new(),
        }
    }
}

impl Engine {
    /// Import an engine math list; returns the head and tail handles.
    pub(crate) fn import_math_list(&mut self, list: &[Node], ctx: &mut LangCtx, fenced: bool) -> (u32, u32) {
        let mut head = 0u32;
        let mut tail = 0u32;
        let mut i = 0;
        while i < list.len() {
            let n = match &list[i] {
                Node::DelimBox { size: 0, small, large, fence, .. } if !(fenced && i == 0) => {
                    // an open `\left`: the whole group is an inner noad
                    let end = close_marker_at(list, i).unwrap_or(list.len() - 1);
                    let group = &list[i..=end];
                    let (sup, sub, group_owned);
                    let group: &[Node] = match group.last() {
                        Some(Node::Scripts { nucleus, sup: s, sub: b, .. }) if matches!(nucleus.as_slice(), [Node::DelimBox { size: 1, .. }]) => {
                            let mut g = group[..group.len() - 1].to_vec();
                            g.push(nucleus[0].clone());
                            sup = s.clone();
                            sub = b.clone();
                            group_owned = g;
                            &group_owned
                        }
                        _ => {
                            sup = None;
                            sub = None;
                            group
                        }
                    };
                    let _ = (small, large, fence);
                    let (h, _) = self.import_math_list(group, ctx, true);
                    let noad = self.lua_new_node(NOAD, 9);
                    let inner = self.lua_new_node(SUB_MLIST, 0);
                    self.lua_nodes.node_mut(inner).f[0] = h as i32;
                    let sup_n = sup.map_or(0, |l| self.import_math_field(&l, ctx));
                    let sub_n = sub.map_or(0, |l| self.import_math_field(&l, ctx));
                    let f = &mut self.lua_nodes.node_mut(noad).f;
                    f[0] = inner as i32;
                    f[1] = sub_n as i32;
                    f[2] = sup_n as i32;
                    i = end;
                    noad
                }
                Node::Choice => {
                    let c = self.lua_new_node(CHOICE, 0);
                    for k in 0..4 {
                        if let Some(Node::ChoiceAlt { body, .. }) = list.get(i + 1 + k) {
                            let (h, _) = self.import_math_list(body, ctx, false);
                            self.lua_nodes.node_mut(c).f[k] = h as i32;
                        }
                    }
                    i += 4;
                    c
                }
                other => self.import_math_node(other, ctx),
            };
            i += 1;
            if n == 0 {
                continue;
            }
            if head == 0 {
                head = n;
            } else {
                self.lua_nodes.couple(tail, n);
            }
            tail = n;
        }
        (head, tail)
    }

    /// A noad field (`nucleus`, `sub`, `sup`, `num`, `degree`...) holding the
    /// list `l`: a `math_char` for a lone character, else a `sub_mlist`.
    fn import_math_field(&mut self, l: &[Node], ctx: &mut LangCtx) -> u32 {
        if let [Node::MathChar { fam, c, .. }] = l {
            if *fam != 255 {
                return self.import_math_char(*fam, *c);
            }
        }
        self.import_math_sub_mlist(l, ctx)
    }

    /// A `sub_mlist` node holding `l` (the numerator and denominator of a
    /// fraction are never collapsed to a `math_char`).
    fn import_math_sub_mlist(&mut self, l: &[Node], ctx: &mut LangCtx) -> u32 {
        let n = self.lua_new_node(SUB_MLIST, 0);
        let (h, _) = self.import_math_list(l, ctx, false);
        self.lua_nodes.node_mut(n).f[0] = h as i32;
        n
    }

    fn import_math_char(&mut self, fam: u8, c: u32) -> u32 {
        let n = self.lua_new_node(MATH_CHAR, 0);
        let f = &mut self.lua_nodes.node_mut(n).f;
        f[0] = i32::from(fam);
        f[1] = c as i32;
        n
    }

    fn import_delim(&mut self, d: &Delim) -> u32 {
        let n = self.lua_new_node(DELIM, 0);
        let f = &mut self.lua_nodes.node_mut(n).f;
        f[0] = i32::from(d.small_fam);
        f[1] = d.small_char as i32;
        f[2] = i32::from(d.large_fam);
        f[3] = d.large_char as i32;
        n
    }

    /// The nucleus field of a noad whose body is `l` (`sub_box` for a lone box).
    fn import_math_nucleus(&mut self, l: &[Node], ctx: &mut LangCtx) -> u32 {
        match l {
            [] => 0,
            [b @ Node::Box { .. }] => {
                let bx = self.import_math_single(b, ctx);
                let n = self.lua_new_node(SUB_BOX, 0);
                self.lua_nodes.node_mut(n).f[0] = bx as i32;
                n
            }
            _ => self.import_math_field(l, ctx),
        }
    }

    fn import_math_single(&mut self, n: &Node, ctx: &mut LangCtx) -> u32 {
        let (h, _) = self.import_list(std::slice::from_ref(n), ctx);
        h
    }

    /// One noad of an engine math list as Lua nodes (a `fenced` list starts
    /// with its left fence and ends with the right one).
    fn import_math_node(&mut self, node: &Node, ctx: &mut LangCtx) -> u32 {
        match node {
            Node::Style(s, _) => {
                let g = gstyle_of(*s);
                let n = self.lua_new_node(STYLE, u16::from(g));
                self.lua_nodes.node_mut(n).f[0] = i32::from(g);
                n
            }
            Node::MathChar { fam: 255, .. } => 0,
            Node::MathChar { fam, c, class, .. } => self.import_noad_with(*class, 0, &[node.clone()], None, None, ctx, Some((*fam, *c))),
            Node::Scripts { nucleus, sup, sub, .. } => self.import_scripts(nucleus, sup.as_deref(), sub.as_deref(), ctx),
            Node::OpLimits { op, above, below, .. } => {
                let (marker, payload): (u8, &[Node]) = match op.first() {
                    Some(Node::MathChar { fam: 255, c, class: CL_OP, .. }) => (*c as u8, &op[1..]),
                    _ => (0, op.as_slice()),
                };
                let sub = match marker {
                    1 => 2,
                    2 => 3,
                    _ => 1,
                };
                self.import_noad_with(CL_OP, sub, payload, above.as_deref(), below.as_deref(), ctx, None)
            }
            Node::Frac { num, den, thickness, left, right, middle, options, fam, .. } => {
                let n = self.lua_new_node(FRACTION, 0);
                let nu = self.import_math_sub_mlist(num, ctx);
                let de = self.import_math_sub_mlist(den, ctx);
                // a `\withdelims` fraction keeps its delimiter nodes even when
                // they are null delimiters
                use crate::boxes::noad_option as no;
                let l = if no::has(*options, no::FRAC_LEFT_DELIM) || !left.is_null() { self.import_delim(left) } else { 0 };
                let r = if no::has(*options, no::FRAC_RIGHT_DELIM) || !right.is_null() { self.import_delim(right) } else { 0 };
                let m = if middle.is_null() { 0 } else { self.import_delim(middle) };
                let f = &mut self.lua_nodes.node_mut(n).f;
                f[0] = *thickness;
                f[1] = nu as i32;
                f[2] = de as i32;
                f[3] = l as i32;
                f[4] = r as i32;
                f[5] = m as i32;
                f[6] = if *fam == crate::boxes::NO_FAM { -1 } else { i32::from(*fam) };
                f[7] = i32::from(*options & !crate::boxes::noad_option::FRAC_DELIMS);
                n
            }
            Node::Radical { body, delim, subtype, width, options, degree, .. } => {
                let n = self.lua_new_node(RADICAL, u16::from(*subtype));
                let nucleus = self.import_math_nucleus(body, ctx);
                let left = self.import_delim(delim);
                let deg = degree.as_ref().map_or(0, |d| self.import_math_field(d, ctx));
                let f = &mut self.lua_nodes.node_mut(n).f;
                f[0] = nucleus as i32;
                f[3] = left as i32;
                f[4] = deg as i32;
                f[5] = *width;
                f[6] = i32::from(*options);
                n
            }
            Node::Accent { spec, body, .. } => self.import_accent(spec, body, None, None, ctx),
            Node::DelimBox { small, large, size, fence, .. } => {
                let d = Delim { small_fam: small.0, small_char: small.1, large_fam: large.0, large_char: large.1 };
                match *size {
                    0 | 1 | 3 | 4 => {
                        let subtype = match *size {
                            0 => FENCE_LEFT,
                            3 => FENCE_MIDDLE,
                            1 => FENCE_RIGHT,
                            _ => FENCE_NO,
                        };
                        self.import_fence(subtype, &d, fence)
                    }
                    _ => {
                        // a plain delimiter atom: an ordinary noad
                        let ch = self.import_math_char(small.0, small.1);
                        let noad = self.lua_new_node(NOAD, 0);
                        self.lua_nodes.node_mut(noad).f[0] = ch as i32;
                        noad
                    }
                }
            }
            Node::Overline { body, under, fam, .. } => {
                let inner = self.import_math_field(body, ctx);
                let noad = self.lua_new_node(NOAD, if *under { SUB_UNDER } else { SUB_OVER });
                let f = &mut self.lua_nodes.node_mut(noad).f;
                f[0] = inner as i32;
                f[3] = if *fam == crate::boxes::NO_FAM { -1 } else { i32::from(*fam) };
                noad
            }
            Node::VCenter { box_node } => {
                let bx = self.import_math_single(box_node, ctx);
                let sb = self.lua_new_node(SUB_BOX, 0);
                self.lua_nodes.node_mut(sb).f[0] = bx as i32;
                let noad = self.lua_new_node(NOAD, SUB_VCENTER);
                self.lua_nodes.node_mut(noad).f[0] = sb as i32;
                noad
            }
            Node::Box { .. } => {
                let bx = self.import_math_single(node, ctx);
                let sb = self.lua_new_node(SUB_BOX, 0);
                self.lua_nodes.node_mut(sb).f[0] = bx as i32;
                let noad = self.lua_new_node(NOAD, 0);
                self.lua_nodes.node_mut(noad).f[0] = sb as i32;
                noad
            }
            other => self.import_math_single(other, ctx),
        }
    }

    fn import_fence(&mut self, subtype: u16, d: &Delim, opts: &FenceOpts) -> u32 {
        let delim = self.import_delim(d);
        let n = self.lua_new_node(FENCE, subtype);
        let f = &mut self.lua_nodes.node_mut(n).f;
        f[0] = delim as i32;
        f[2] = opts.height;
        f[3] = opts.depth;
        f[4] = i32::from(opts.options);
        f[5] = opts.class;
        n
    }

    /// A simple noad of `class` with the nucleus `body` (or the math char
    /// `ch`) and scripts.
    #[allow(clippy::too_many_arguments)]
    fn import_noad_with(
        &mut self,
        class: u8,
        op_sub: u16,
        body: &[Node],
        sup: Option<&[Node]>,
        sub: Option<&[Node]>,
        ctx: &mut LangCtx,
        ch: Option<(u8, u32)>,
    ) -> u32 {
        let nucleus = match ch {
            Some((fam, c)) => self.import_math_char(fam, c),
            None => self.import_math_nucleus(body, ctx),
        };
        let subtype = if class == CL_OP && op_sub == 0 { 1 } else if class == CL_OP { op_sub } else { lua_subtype_of_class(class) };
        let sup_n = sup.map_or(0, |l| self.import_math_field(l, ctx));
        let sub_n = sub.map_or(0, |l| self.import_math_field(l, ctx));
        let n = self.lua_new_node(NOAD, subtype);
        let f = &mut self.lua_nodes.node_mut(n).f;
        f[0] = nucleus as i32;
        f[1] = sub_n as i32;
        f[2] = sup_n as i32;
        n
    }

    fn import_accent(&mut self, spec: &AccentSpec, body: &[Node], sup: Option<&[Node]>, sub: Option<&[Node]>, ctx: &mut LangCtx) -> u32 {
        let nucleus = self.import_math_nucleus(body, ctx);
        let sup_n = sup.map_or(0, |l| self.import_math_field(l, ctx));
        let sub_n = sub.map_or(0, |l| self.import_math_field(l, ctx));
        let top = spec.top.map_or(0, |(fam, c)| self.import_math_char(fam, c));
        let bottom = spec.bottom.map_or(0, |(fam, c)| self.import_math_char(fam, c));
        let overlay = spec.overlay.map_or(0, |(fam, c)| self.import_math_char(fam, c));
        let n = self.lua_new_node(ACCENT, u16::from(spec.subtype));
        let f = &mut self.lua_nodes.node_mut(n).f;
        f[0] = nucleus as i32;
        f[1] = sub_n as i32;
        f[2] = sup_n as i32;
        f[4] = bottom as i32;
        f[5] = top as i32;
        f[6] = overlay as i32;
        f[7] = spec.fraction;
        n
    }

    fn import_scripts(&mut self, nucleus: &[Node], sup: Option<&[Node]>, sub: Option<&[Node]>, ctx: &mut LangCtx) -> u32 {
        match nucleus {
            [Node::Accent { spec, body, .. }] => self.import_accent(spec, body, sup, sub, ctx),
            [Node::Radical { body, delim, subtype, width, options, degree, .. }] => {
                let n = self.import_math_node(
                    &Node::Radical {
                        body: body.clone(),
                        delim: *delim,
                        subtype: *subtype,
                        width: *width,
                        options: *options,
                        degree: degree.clone(),
                        origin: MathDiagnosticOrigin::default(), attr: crate::boxes::Attr::NONE,
                    },
                    ctx,
                );
                let sup_n = sup.map_or(0, |l| self.import_math_field(l, ctx));
                let sub_n = sub.map_or(0, |l| self.import_math_field(l, ctx));
                let f = &mut self.lua_nodes.node_mut(n).f;
                f[1] = sub_n as i32;
                f[2] = sup_n as i32;
                n
            }
            [Node::MathChar { fam: 255, c, class, .. }, rest @ ..] => {
                let op_sub = if *class == CL_OP {
                    match *c {
                        1 => 2,
                        2 => 3,
                        _ => 1,
                    }
                } else {
                    0
                };
                self.import_noad_with(*class, op_sub, rest, sup, sub, ctx, None)
            }
            // a bare operator character that carries scripts has them beside
            // it (`\nolimits`); a normal operator is an `OpLimits` node
            [Node::MathChar { fam, c, class, .. }] => {
                let op_sub = if *class == CL_OP { 3 } else { 0 };
                self.import_noad_with(*class, op_sub, &[], sup, sub, ctx, Some((*fam, *c)))
            }
            [Node::VCenter { box_node }] => {
                let bx = self.import_math_single(box_node, ctx);
                let sb = self.lua_new_node(SUB_BOX, 0);
                self.lua_nodes.node_mut(sb).f[0] = bx as i32;
                let sup_n = sup.map_or(0, |l| self.import_math_field(l, ctx));
                let sub_n = sub.map_or(0, |l| self.import_math_field(l, ctx));
                let noad = self.lua_new_node(NOAD, SUB_VCENTER);
                let f = &mut self.lua_nodes.node_mut(noad).f;
                f[0] = sb as i32;
                f[1] = sub_n as i32;
                f[2] = sup_n as i32;
                noad
            }
            [Node::Overline { body, under, fam, .. }] => {
                let inner = self.import_math_field(body, ctx);
                let sup_n = sup.map_or(0, |l| self.import_math_field(l, ctx));
                let sub_n = sub.map_or(0, |l| self.import_math_field(l, ctx));
                let noad = self.lua_new_node(NOAD, if *under { SUB_UNDER } else { SUB_OVER });
                let f = &mut self.lua_nodes.node_mut(noad).f;
                f[3] = if *fam == crate::boxes::NO_FAM { -1 } else { i32::from(*fam) };
                f[0] = inner as i32;
                f[1] = sub_n as i32;
                f[2] = sup_n as i32;
                noad
            }
            [Node::DelimBox { size: 1, .. }] => {
                // the close marker of a group that carries scripts: the
                // enclosing group import attaches them
                self.import_math_node(&nucleus[0], ctx)
            }
            _ => self.import_noad_with(CL_ORD, 0, nucleus, sup, sub, ctx, None),
        }
    }

    // ---------- Lua nodes -> engine ----------

    /// The engine list of a noad field: `math_char` (a lone character),
    /// `sub_mlist`, `sub_box`, or nothing.
    fn export_math_field(&mut self, h: i32) -> NodeList {
        let h = h as u32;
        if !self.lua_nodes.valid(h) {
            return Vec::new();
        }
        let (id, f) = (self.lua_nodes.id(h), self.lua_nodes.node(h).f);
        let out = match id {
            MATH_CHAR | MATH_TEXT_CHAR => vec![Node::MathChar {
                fam: f[0].clamp(0, 255) as u8,
                c: f[1] as u32,
                class: CL_ORD,
                origin: MathDiagnosticOrigin::default(), attr: crate::boxes::Attr::NONE,
            }],
            SUB_MLIST => {
                let l = self.export_sub(f[0]);
                l
            }
            SUB_BOX => self.export_sub(f[0]),
            _ => Vec::new(),
        };
        self.lua_nodes.flush_node(h);
        out
    }

    fn export_delim_opt(&mut self, h: i32) -> Option<Delim> {
        let h = h as u32;
        if !self.lua_nodes.valid(h) {
            return None;
        }
        let f = self.lua_nodes.node(h).f;
        self.lua_nodes.flush_node(h);
        Some(Delim { small_fam: f[0] as u8, small_char: f[1] as u32, large_fam: f[2] as u8, large_char: f[3] as u32 })
    }

    fn export_accent_char(&mut self, h: i32) -> Option<(u8, u32)> {
        let h = h as u32;
        if !self.lua_nodes.valid(h) {
            return None;
        }
        let f = self.lua_nodes.node(h).f;
        self.lua_nodes.flush_node(h);
        Some((f[0] as u8, f[1] as u32))
    }

    /// Wrap `nucleus` with the scripts that `sup`/`sub` lists describe.
    fn with_scripts(nucleus: NodeList, sup: NodeList, sub: NodeList, has_sup: bool, has_sub: bool) -> Node {
        Node::Scripts {
            nucleus,
            sup: has_sup.then_some(sup),
            sub: has_sub.then_some(sub),
            options: 0, attr: crate::boxes::Attr::NONE,
        }
    }

    /// The engine form of the noad-like node `n` (not its successors).
    pub(crate) fn export_math_node(&mut self, n: u32, out: &mut NodeList) {
        let (id, sub) = (self.lua_nodes.id(n), self.lua_nodes.subtype(n));
        let f = self.lua_nodes.node(n).f;
        let origin = MathDiagnosticOrigin::default;
        match id {
            STYLE => {
                let g = f[0].clamp(0, 7) as u8;
                out.push(Node::Style(math_style_of(g), crate::boxes::Attr::NONE));
            }
            CHOICE => {
                out.push(Node::Choice);
                for k in 0..4 {
                    let body = self.export_sub(f[k]);
                    out.push(Node::ChoiceAlt { body, attr: crate::boxes::Attr::NONE });
                }
            }
            NOAD => {
                let has_sup = self.lua_nodes.valid(f[2] as u32);
                let has_sub = self.lua_nodes.valid(f[1] as u32);
                let nuc_id = if self.lua_nodes.valid(f[0] as u32) { self.lua_nodes.id(f[0] as u32) } else { 0 };
                let nuc_f = if nuc_id != 0 { self.lua_nodes.node(f[0] as u32).f } else { [0; NF] };
                let sup = self.export_math_field(f[2]);
                let subs = self.export_math_field(f[1]);
                match sub {
                    SUB_UNDER | SUB_OVER => {
                        let body = self.export_math_field(f[0]);
                        let fam = if (0..255).contains(&f[3]) { f[3] as u8 } else { crate::boxes::NO_FAM };
                        let node = Node::Overline { body, under: sub == SUB_UNDER, fam, attr: crate::boxes::Attr::NONE };
                        if has_sup || has_sub {
                            out.push(Self::with_scripts(vec![node], sup, subs, has_sup, has_sub));
                        } else {
                            out.push(node);
                        }
                    }
                    SUB_VCENTER => {
                        let mut body = self.export_math_field(f[0]);
                        let node = Node::VCenter {
                            box_node: Box::new(if body.is_empty() { Node::Empty } else { body.remove(0) }),
                        };
                        if has_sup || has_sub {
                            out.push(Self::with_scripts(vec![node], sup, subs, has_sup, has_sub));
                        } else {
                            out.push(node);
                        }
                    }
                    _ => {
                        let (class, limits) = class_of_lua_subtype(sub);
                        let body = self.export_math_field(f[0]);
                        let is_char = nuc_id == MATH_CHAR || nuc_id == MATH_TEXT_CHAR;
                        let is_box = nuc_id == SUB_BOX;
                        let _ = nuc_f;
                        if class == CL_OP && (limits != 0 || has_sup || has_sub) {
                            let mut op = vec![Node::MathChar { fam: 255, c: u32::from(limits), class: CL_OP, origin: origin(), attr: crate::boxes::Attr::NONE }];
                            let mut body = body;
                            if let (true, Some(Node::MathChar { class: c, .. })) = (is_char, body.first_mut()) {
                                *c = CL_OP;
                            }
                            op.append(&mut body);
                            out.push(Node::OpLimits { op, above: has_sup.then_some(sup), below: has_sub.then_some(subs), attr: crate::boxes::Attr::NONE });
                        } else if is_char {
                            let mut body = body;
                            if let Some(Node::MathChar { class: c, .. }) = body.first_mut() {
                                *c = class;
                            }
                            if has_sup || has_sub {
                                out.push(Self::with_scripts(body, sup, subs, has_sup, has_sub));
                            } else {
                                out.extend(body);
                            }
                        } else if is_box && class == CL_ORD && !(has_sup || has_sub) {
                            out.extend(body);
                        } else if class == CL_INNER
                            && matches!(body.first(), Some(Node::DelimBox { size: 0, .. }))
                            && matches!(body.last(), Some(Node::DelimBox { size: 1, .. }))
                        {
                            // a `\left...\right` group: the markers go back
                            // into the list, the scripts onto the close marker
                            let mut body = body;
                            if has_sup || has_sub {
                                let close = body.pop().unwrap();
                                out.extend(body);
                                out.push(Self::with_scripts(vec![close], sup, subs, has_sup, has_sub));
                            } else {
                                out.extend(body);
                            }
                        } else {
                            let nucleus = if is_box && class == CL_ORD {
                                body
                            } else if body.is_empty() && class == CL_ORD {
                                body
                            } else {
                                let mut g = vec![Node::MathChar { fam: 255, c: 0, class, origin: origin(), attr: crate::boxes::Attr::NONE }];
                                g.extend(body);
                                g
                            };
                            out.push(Self::with_scripts(nucleus, sup, subs, has_sup, has_sub));
                        }
                    }
                }
            }
            RADICAL => {
                let has_sup = self.lua_nodes.valid(f[2] as u32);
                let has_sub = self.lua_nodes.valid(f[1] as u32);
                let sup = self.export_math_field(f[2]);
                let subs = self.export_math_field(f[1]);
                let body = self.export_math_field(f[0]);
                let delim = self.export_delim_opt(f[3]).unwrap_or_default();
                let degree = if self.lua_nodes.valid(f[4] as u32) { Some(self.export_math_field(f[4])) } else { None };
                let node = Node::Radical {
                    body,
                    delim,
                    subtype: sub as u8,
                    width: f[5],
                    options: f[6] as u16,
                    degree,
                    origin: origin(), attr: crate::boxes::Attr::NONE,
                };
                if has_sup || has_sub {
                    out.push(Self::with_scripts(vec![node], sup, subs, has_sup, has_sub));
                } else {
                    out.push(node);
                }
            }
            ACCENT => {
                let has_sup = self.lua_nodes.valid(f[2] as u32);
                let has_sub = self.lua_nodes.valid(f[1] as u32);
                let sup = self.export_math_field(f[2]);
                let subs = self.export_math_field(f[1]);
                let body = self.export_math_field(f[0]);
                let accent = self.export_accent_char(f[3]);
                let bottom = self.export_accent_char(f[4]);
                let top = self.export_accent_char(f[5]).or(accent);
                let overlay = self.export_accent_char(f[6]);
                let node = Node::Accent {
                    spec: AccentSpec { top, bottom, overlay, subtype: sub as u8, fraction: f[7] },
                    body,
                    origin: origin(), attr: crate::boxes::Attr::NONE,
                };
                if has_sup || has_sub {
                    out.push(Self::with_scripts(vec![node], sup, subs, has_sup, has_sub));
                } else {
                    out.push(node);
                }
            }
            FRACTION => {
                let num = self.export_math_field(f[1]);
                let den = self.export_math_field(f[2]);
                let left = self.export_delim_opt(f[3]);
                let right = self.export_delim_opt(f[4]);
                let middle = self.export_delim_opt(f[5]);
                out.push(Node::Frac {
                    num,
                    den,
                    thickness: f[0],
                    left: left.unwrap_or_default(),
                    right: right.unwrap_or_default(),
                    middle: middle.unwrap_or_default(),
                    options: f[7] as u16
                        | if f[3] != 0 { crate::boxes::noad_option::FRAC_LEFT_DELIM } else { 0 }
                        | if f[4] != 0 { crate::boxes::noad_option::FRAC_RIGHT_DELIM } else { 0 },
                    fam: if (0..255).contains(&f[6]) { f[6] as u8 } else { crate::boxes::NO_FAM },
                    origin: origin(), attr: crate::boxes::Attr::NONE,
                });
            }
            FENCE => {
                let d = self.export_delim_opt(f[0]).unwrap_or_default();
                let size = match sub {
                    FENCE_LEFT => 0,
                    FENCE_MIDDLE => 3,
                    FENCE_RIGHT => 1,
                    _ => 4,
                };
                out.push(Node::DelimBox {
                    small: (d.small_fam, d.small_char),
                    large: (d.large_fam, d.large_char),
                    size,
                    fence: FenceOpts { height: f[2], depth: f[3], class: f[5], options: f[4] as u16 },
                    origin: origin(), attr: crate::boxes::Attr::NONE,
                });
            }
            MATH_CHAR | MATH_TEXT_CHAR => out.push(Node::MathChar {
                fam: f[0].clamp(0, 255) as u8,
                c: f[1] as u32,
                class: CL_ORD,
                origin: origin(), attr: crate::boxes::Attr::NONE,
            }),
            SUB_MLIST | SUB_BOX => {
                let list = self.export_sub(f[0]);
                out.extend(list);
            }
            _ => {}
        }
    }
}
