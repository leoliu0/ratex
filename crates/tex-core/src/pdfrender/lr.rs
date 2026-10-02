//! etex.ch TeXXeT ship-out: `hlist_out` with the LR stack, reversal of
//! right-to-left segments (`reverse`) and edge nodes. Used for every hlist
//! once the job has created text-direction material; without it ship-out
//! takes `hlist_nodes`' plain left-to-right path.

use super::{GlueState, RenderCtx};
use crate::boxes::{
    math_end_lr, math_end_lr_type, math_lr_dir, Glue, Node, NodeList, BOX_LR_DLIST,
    BOX_LR_REVERSED, LR_KIND_MIN,
};

/// One item of an hlist being shipped: an original node, a node rewritten
/// by `reverse`, or an etex.ch edge node (new direction, width, edge_dist).
enum Item<'n> {
    Ref(&'n Node),
    Own(Node),
    Edge(u8, i64, i64),
}

impl Item<'_> {
    fn node(&self) -> Option<&Node> {
        match self {
            Item::Ref(n) => Some(n),
            Item::Own(n) => Some(n),
            Item::Edge(..) => None,
        }
    }

    /// width and kind of a math node
    fn math(&self) -> Option<(i64, u8)> {
        match self.node() {
            Some(&Node::MathKern(w, k, _)) if k != 0 => Some((w as i64, k)),
            _ => None,
        }
    }
}

/// How `reverse` treats a node.
enum Class {
    /// character-like: advances by its width, never dropped
    Char(i64),
    /// box or rule
    Width(i64),
    Kern(i64),
    Glue(Glue),
    Leaders(Glue),
    Math(i64, u8),
    /// `othercases goto next_p`: kept without width
    Other,
}

/// etex.ch "Handle a glue node for mixed direction text": only glue of the
/// box's active order is frozen.
fn active_glue(g: &Glue, sign: u8, order: u8) -> bool {
    (sign == 1 && g.stretch_order == order) || (sign == 2 && g.shrink_order == order)
}

impl<'a> RenderCtx<'a> {
    /// etex.ch `hlist_out` for a box whose width/box_lr are in
    /// `box_w_sp`/`box_lr`: x is `cur_h` on entry (the right edge when
    /// entered right-to-left).
    pub(super) fn hlist_nodes_lr(
        &mut self,
        list: &NodeList,
        x: i64,
        y: i64,
        sign: u8,
        order: u8,
        set: f64,
    ) {
        let box_lr = self.box_lr;
        let box_w = self.box_w_sp;
        let mut cur_h = x;
        let mut gs = GlueState::default();
        let mut items: Vec<Item> = Vec::with_capacity(list.len() + 1);
        for n in list {
            match n {
                // the text of an unbroken discretionary is ordinary material
                Node::Disc(dc) => items.extend(dc.no_break.iter().map(Item::Ref)),
                n => items.push(Item::Ref(n)),
            }
        }
        let mut stack: Vec<u8> = Vec::new();
        // "Initialize hlist_out for mixed direction typesetting"
        let mut dlist_rtl = false;
        if box_lr == BOX_LR_DLIST && self.cur_dir == 1 {
            self.cur_dir = 0;
            cur_h -= box_w;
            dlist_rtl = true;
        }
        if self.cur_dir == 1 && box_lr != BOX_LR_REVERSED {
            // "Reverse the complete hlist"
            let save_h = cur_h;
            cur_h = 0;
            let rev = self.lr_reverse(items, None, &mut cur_h, &mut gs, &mut stack, sign, order, set);
            items = Vec::with_capacity(rev.len() + 1);
            items.push(Item::Own(Node::Kern(-cur_h as i32, crate::boxes::Attr::NONE)));
            items.extend(rev);
            cur_h = save_h;
        }
        self.left_edge_sp = cur_h;
        let mut i = 0;
        while i < items.len() {
            if let Item::Edge(dir, width, dist) = items[i] {
                cur_h += width;
                self.left_edge_sp = cur_h + dist;
                self.cur_dir = dir;
                i += 1;
                continue;
            }
            if let Some((w, kind)) = items[i].math() {
                // "Adjust the LR stack for the hlist_out routine"
                if math_end_lr(kind) {
                    if stack.last() == Some(&math_end_lr_type(kind)) {
                        stack.pop();
                    } else if kind > LR_KIND_MIN {
                        self.lr_problems += 1;
                    }
                } else {
                    stack.push(math_end_lr_type(kind));
                    if math_lr_dir(kind) != self.cur_dir {
                        // "Reverse an hlist segment and goto reswitch"
                        let save_h = cur_h;
                        let rest = items.split_off(i + 1);
                        items.pop();
                        self.cur_dir = 1 - self.cur_dir;
                        let dir = self.cur_dir;
                        cur_h = cur_h - self.left_edge_sp + w;
                        let tail = Item::Edge(1 - dir, 0, 0);
                        let rev = self.lr_reverse(
                            rest,
                            Some(tail),
                            &mut cur_h,
                            &mut gs,
                            &mut stack,
                            sign,
                            order,
                            set,
                        );
                        items.push(Item::Edge(dir, w, cur_h));
                        items.extend(rev);
                        self.cur_dir = 1 - self.cur_dir;
                        cur_h = save_h;
                        continue;
                    }
                }
                cur_h += w;
                i += 1;
                continue;
            }
            if let Some(n) = items[i].node() {
                cur_h = self.hlist_node_out(n, cur_h, y, &mut gs, sign, order, set);
            }
            i += 1;
        }
        // "Check for LR anomalies at the end of hlist_out"
        while let Some(kind) = stack.pop() {
            if kind > LR_KIND_MIN {
                self.lr_problems += 10000;
            }
        }
        if dlist_rtl {
            self.cur_dir = 1;
        }
    }

    /// etex.ch `reverse`: move `src` onto a new list in reverse order up to
    /// the end node closing the current segment (whose edge `t` then gets
    /// the end node's width and the edge distance, followed by the rest),
    /// or the whole of `src` when `t` is None. Freezes active glue, turns
    /// outer math nodes into kerns and flips inner ones, manufactures missing
    /// end nodes, and advances `cur_h` by the reversed material.
    #[allow(clippy::too_many_arguments)]
    fn lr_reverse<'n>(
        &mut self,
        src: Vec<Item<'n>>,
        mut t: Option<Item<'n>>,
        cur_h: &mut i64,
        gs: &mut GlueState,
        stack: &mut Vec<u8>,
        sign: u8,
        order: u8,
        set: f64,
    ) -> Vec<Item<'n>> {
        let has_t = t.is_some();
        let mut l: Vec<Item<'n>> = Vec::with_capacity(src.len() + 1);
        let mut rest: Vec<Item<'n>> = Vec::new();
        let (mut m, mut n) = (0u32, 0u32);
        let mut pending = src.into_iter();
        'done: loop {
            while let Some(p) = pending.next() {
                let class = match p.node() {
                    None => Class::Other,
                    Some(node) => match node {
                        Node::Char { c, font, .. } => Class::Char(self.font_char_advance_sp(*font, *c)),
                        Node::LuaGlyph(g) => Class::Char(i64::from(crate::boxes::lua_glyph_dims(&self.eng.eqtb, g).0)),
                        Node::Ligature {
                            font, lig_width, ..
                        } => Class::Char(self.font_lig_advance_sp(*font, *lig_width)),
                        Node::NativeGlyphRun { width, .. } => Class::Char(*width as i64),
                        Node::Box { w, .. } => Class::Width(*w as i64),
                        Node::Rule { width, .. } => Class::Width(*width as i64),
                        Node::Kern(k, _) | Node::ExplicitKern(k, _) | Node::AccentKern(k, _) | Node::ItalicKern(k, _) => {
                            Class::Kern(*k as i64)
                        }
                        Node::Glue(g, _) => Class::Glue(*g),
                        Node::Leaders { glue, .. } => Class::Leaders(*glue),
                        &Node::MathKern(w, k, _) if k != 0 => Class::Math(w as i64, k),
                        _ => Class::Other,
                    },
                };
                let (item, rule_wd, is_kern) = match class {
                    Class::Char(w) => {
                        *cur_h += w;
                        l.push(p);
                        continue;
                    }
                    Class::Width(w) => (p, w, false),
                    Class::Kern(w) => (p, w, true),
                    Class::Glue(g) => {
                        let w = gs.advance(&g, sign, order, set);
                        if active_glue(&g, sign, order) {
                            (Item::Own(Node::Kern(w as i32, crate::boxes::Attr::NONE)), w, true)
                        } else {
                            (p, w, false)
                        }
                    }
                    Class::Leaders(g) => {
                        let w = gs.advance(&g, sign, order, set);
                        let item = match p.node() {
                            Some(Node::Leaders { kind, body, .. }) if active_glue(&g, sign, order) => {
                                // a rigid spec whose orders never match
                                Item::Own(Node::Leaders {
                                    glue: Glue::spec(w as i32, 0, 4, 0, 4),
                                    kind: *kind,
                                    body: body.clone(), attr: crate::boxes::Attr::NONE,
                                })
                            }
                            _ => p,
                        };
                        (item, w, false)
                    }
                    Class::Math(w, kind) => {
                        // "Cases of reverse": inner math nodes flip, outer
                        // ones become kerns
                        if math_end_lr(kind) {
                            if stack.last() != Some(&math_end_lr_type(kind)) {
                                self.lr_problems += 1;
                                (Item::Own(Node::Kern(w as i32, crate::boxes::Attr::NONE)), w, true)
                            } else {
                                stack.pop();
                                if n > 0 {
                                    n -= 1;
                                    (Item::Own(Node::MathKern(w as i32, kind - 1, crate::boxes::Attr::NONE)), w, false)
                                } else if m > 0 {
                                    m -= 1;
                                    (Item::Own(Node::Kern(w as i32, crate::boxes::Attr::NONE)), w, true)
                                } else {
                                    // "Finish the reversed hlist segment"
                                    if let Some(Item::Edge(_, tw, td)) = t.as_mut() {
                                        *tw = w;
                                        *td = -*cur_h - w;
                                    }
                                    rest.extend(pending);
                                    break 'done;
                                }
                            }
                        } else {
                            stack.push(math_end_lr_type(kind));
                            if n > 0 || math_lr_dir(kind) != self.cur_dir {
                                n += 1;
                                (Item::Own(Node::MathKern(w as i32, kind + 1, crate::boxes::Attr::NONE)), w, false)
                            } else {
                                m += 1;
                                (Item::Own(Node::Kern(w as i32, crate::boxes::Attr::NONE)), w, true)
                            }
                        }
                    }
                    Class::Other => (p, 0, false),
                };
                *cur_h += rule_wd;
                // a zero kern, or the kern that would end the whole list, goes
                if is_kern && (rule_wd == 0 || (l.is_empty() && !has_t)) {
                    continue;
                }
                l.push(item);
            }
            if !has_t && m == 0 && n == 0 {
                break;
            }
            // manufacture one missing math node
            let Some(&top) = stack.last() else {
                break;
            };
            self.lr_problems += 10000;
            pending = vec![Item::Own(Node::MathKern(0, top, crate::boxes::Attr::NONE))].into_iter();
        }
        l.reverse();
        l.extend(t);
        l.extend(rest);
        l
    }
}
