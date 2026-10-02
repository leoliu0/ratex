//! Conversion between the engine's node lists (`Vec<Node>`) and the Lua node
//! store (`lua_node`): the list is imported when LuaTeX would hand it to
//! Lua and exported when Lua gives it back. Engine nodes that Lua cannot see
//! into (PDF page objects, language whatsits, ...) travel as opaque `temp`
//! nodes and come back untouched.

use crate::boxes::{
    self, ColorStackCmd, DiscNode, Glue, LeaderBody, LuaGlyph, Node, NodeList, WhatIt, BOX_LR_DLIST,
};
use crate::engine::Engine;
use crate::lua_node::*;
use crate::prim::IntParam;
use crate::tfm::FontId;

/// Slot numbers (positions in the `node.fields` list of the type); the
/// last slots are free for engine data that has no Lua field.
pub(crate) mod sl {
    // hlist, vlist
    pub const B_WIDTH: usize = 0;
    pub const B_DEPTH: usize = 1;
    pub const B_HEIGHT: usize = 2;
    pub const B_DIR: usize = 3;
    pub const B_SHIFT: usize = 4;
    pub const B_ORDER: usize = 5;
    pub const B_SIGN: usize = 6;
    pub const B_HEAD: usize = 8;
    // unset: `shrink` shares the shift slot
    pub const U_STRETCH: usize = 7;
    pub const U_HEAD: usize = 9;
    /// engine `lr` and `kind`
    pub const B_LR: usize = 10;
    pub const B_KIND: usize = 11;
    // glue
    pub const G_LEADER: usize = 0;
    pub const G_WIDTH: usize = 1;
    pub const G_STRETCH: usize = 2;
    pub const G_SHRINK: usize = 3;
    pub const G_SORDER: usize = 4;
    pub const G_HORDER: usize = 5;
    pub const G_ZERO: usize = 10;
    // rule
    pub const R_WIDTH: usize = 0;
    pub const R_DEPTH: usize = 1;
    pub const R_HEIGHT: usize = 2;
    pub const R_DIR: usize = 3;
    pub const R_INDEX: usize = 4;
    pub const R_TRANSFORM: usize = 7;
    // glyph
    pub const C_CHAR: usize = 0;
    pub const C_FONT: usize = 1;
    pub const C_LANG: usize = 2;
    pub const C_LEFT: usize = 3;
    pub const C_RIGHT: usize = 4;
    pub const C_UCHYPH: usize = 5;
    pub const C_COMP: usize = 6;
    pub const C_XOFF: usize = 7;
    pub const C_YOFF: usize = 8;
    pub const C_EXPAN: usize = 12;
    pub const C_DATA: usize = 13;
    // disc
    pub const D_PRE: usize = 0;
    pub const D_POST: usize = 1;
    pub const D_REPLACE: usize = 2;
    pub const D_PENALTY: usize = 3;
    /// engine data: bit 0/1/2 when the first node of the pre/post/replace
    /// list has no `prev` pointer (luatex `set_disc_field`), so a kern
    /// `add_kern_before` puts in front of it is lost
    pub const D_NOALINK: usize = 4;
    // ins
    pub const I_COST: usize = 0;
    pub const I_DEPTH: usize = 1;
    pub const I_HEIGHT: usize = 2;
    pub const I_SPEC: usize = 3;
    pub const I_HEAD: usize = 4;
    pub const I_ENGINE_DEPTH: usize = 10;
    // margin kern
    pub const M_WIDTH: usize = 0;
    pub const M_GLYPH: usize = 1;
}

/// The hyphenation state a glyph created by the engine would have.
#[derive(Clone, Copy)]
pub(crate) struct LangCtx {
    pub(crate) lang: u16,
    pub(crate) left: u8,
    pub(crate) right: u8,
    pub(crate) uchyph: u8,
}

fn glue_subtype_to_lua(s: u8) -> u16 {
    if s == boxes::glue_subtype::NONSCRIPT {
        COND_MATH_GLUE
    } else if s >= boxes::glue_subtype::THIN_MU_SKIP {
        u16::from(s) + 1
    } else {
        u16::from(s)
    }
}

fn glue_subtype_from_lua(s: u16) -> u8 {
    match s {
        COND_MATH_GLUE => boxes::glue_subtype::NONSCRIPT,
        17..=19 => (s - 1) as u8,
        0..=15 => s as u8,
        _ => 0,
    }
}

fn rule_to_lua(v: i32) -> i32 {
    if v == crate::build::RULE_FILL { NULL_FLAG } else { v }
}

fn rule_from_lua(v: i32) -> i32 {
    if v == NULL_FLAG { crate::build::RULE_FILL } else { v }
}

fn literal_mode_to_lua(origin: u8) -> i32 {
    match origin {
        1 => 2,
        2 => 1,
        o => i32::from(o),
    }
}

fn literal_mode_from_lua(mode: i32) -> u8 {
    match mode {
        2 => 1,
        1 => 2,
        m => m.clamp(0, 255) as u8,
    }
}

impl Engine {
    pub(crate) fn lang_ctx(&self) -> LangCtx {
        let l = self.current_language();
        LangCtx {
            lang: u16::from(l.lang),
            left: l.lhm,
            right: l.rhm,
            uchyph: u8::from(self.eqtb.int_params[IntParam::UcHyph.idx() as usize] > 0),
        }
    }

    /// The Lua attribute list (0 for none) standing for engine attribute
    /// list `a`.
    pub(crate) fn lua_attr_handle(&mut self, a: boxes::Attr) -> u32 {
        if a == boxes::Attr::NONE {
            0
        } else {
            self.lua_nodes.cached_attr_list(a.0, self.eqtb.attr_lists.pairs(a))
        }
    }

    /// The engine attribute list of Lua attribute list `h`.
    pub(crate) fn engine_attr_of_list(&mut self, h: u32) -> boxes::Attr {
        if h == 0 {
            return boxes::Attr::NONE;
        }
        if let Some(a) = self.lua_nodes.engine_attr_of(h) {
            return boxes::Attr(a);
        }
        let pairs = self.lua_nodes.attr_pairs(h);
        self.eqtb.attr_lists.intern(&pairs)
    }

    /// The Lua attribute list of the current `\attribute` values.
    pub(crate) fn lua_current_attr_handle(&mut self) -> u32 {
        let a = self.eqtb.cur_attr;
        self.lua_attr_handle(a)
    }

    /// A new node of type `id` with the current attribute list (or, while
    /// an engine node is imported, the list of that node).
    pub(crate) fn lua_new_node(&mut self, id: u8, subtype: u16) -> u32 {
        let attr = match self.lua_nodes.import_attr {
            Some(h) => h,
            None if has_attr_type(id, subtype) => self.lua_current_attr_handle(),
            None => 0,
        };
        self.lua_nodes.new_node(id, subtype, attr)
    }

    /// Import a list of engine nodes; returns the head handle (0 when
    /// empty).
    pub(crate) fn lua_nodes_from_engine(&mut self, list: NodeList) -> i64 {
        let mut ctx = self.lang_ctx();
        let (head, _) = self.import_list(&list, &mut ctx);
        i64::from(head)
    }

    pub(crate) fn import_list(&mut self, list: &[Node], ctx: &mut LangCtx) -> (u32, u32) {
        if list.iter().any(crate::lua_math_conv::is_math_node) {
            return self.import_math_list(list, ctx, false);
        }
        let mut head = 0u32;
        let mut tail = 0u32;
        let mut i = 0;
        // language whatsits: LuaTeX has none (the language lives in the
        // glyphs); they ride along with the node that follows them
        let mut pending: Vec<Node> = Vec::new();
        while i < list.len() {
            let node = &list[i];
            i += 1;
            if let Node::Whatsit(w @ (WhatIt::Language { .. } | WhatIt::SyncPoint { .. }), _) = node {
                if let WhatIt::Language { lang, lhm, rhm } = w {
                    ctx.lang = u16::from(*lang);
                    ctx.left = *lhm;
                    ctx.right = *rhm;
                }
                pending.push(node.clone());
                continue;
            }
            let n = self.import_node(node, ctx);
            if n == 0 {
                continue;
            }
            if !pending.is_empty() {
                let ext = self.lua_nodes.node_mut(n).ext.get_or_insert_with(Default::default);
                ext.pre = std::mem::take(&mut pending);
            }
            if let Node::Disc(dc) = node {
                // the nodes following the discretionary that replace_count
                // counts duplicate its replace text
                i += dc.replace_count;
            }
            if head == 0 {
                head = n;
            } else {
                self.lua_nodes.couple(tail, n);
            }
            tail = n;
        }
        if !pending.is_empty() {
            if tail != 0 {
                let ext = self.lua_nodes.node_mut(tail).ext.get_or_insert_with(Default::default);
                ext.post = pending;
            } else {
                // nothing but language whatsits: keep them as an opaque node
                let first = pending.remove(0);
                let n = self.import_opaque(&first);
                self.lua_nodes.node_mut(n).ext.get_or_insert_with(Default::default).post = pending;
                head = n;
                tail = n;
            }
        }
        (head, tail)
    }

    fn import_sub(&mut self, list: &[Node], ctx: &mut LangCtx) -> u32 {
        self.import_list(list, ctx).0
    }

    fn import_glyph_node(&mut self, c: u32, font: FontId, ctx: &LangCtx, subtype: u16) -> u32 {
        let n = self.lua_new_node(GLYPH, subtype);
        let f = &mut self.lua_nodes.node_mut(n).f;
        f[sl::C_CHAR] = c as i32;
        f[sl::C_FONT] = i32::from(font);
        f[sl::C_LANG] = i32::from(ctx.lang);
        f[sl::C_LEFT] = i32::from(ctx.left);
        f[sl::C_RIGHT] = i32::from(ctx.right);
        f[sl::C_UCHYPH] = i32::from(ctx.uchyph);
        f[GLYPH_LANG_DATA] = make_lang_data(f[sl::C_UCHYPH], f[sl::C_LANG], f[sl::C_LEFT], f[sl::C_RIGHT]);
        n
    }

    pub(crate) fn import_glue(&mut self, g: &Glue, subtype: u16) -> u32 {
        let n = self.lua_new_node(GLUE, subtype);
        let node = self.lua_nodes.node_mut(n);
        node.f[sl::G_WIDTH] = g.width;
        node.f[sl::G_STRETCH] = g.stretch;
        node.f[sl::G_SHRINK] = g.shrink;
        node.f[sl::G_SORDER] = crate::lua_node_pack::lua_order_of(g.stretch_order);
        node.f[sl::G_HORDER] = crate::lua_node_pack::lua_order_of(g.shrink_order);
        node.f[sl::G_ZERO] = i32::from(g.is_zero_glue());
        n
    }

    /// A temp node standing for an engine node Lua cannot look into.
    fn import_opaque(&mut self, node: &Node) -> u32 {
        let n = self.lua_nodes.new_node(TEMP, 0, 0);
        self.lua_nodes.node_mut(n).ext = Some(Box::new(Ext { opaque: Some(Box::new(node.clone())), ..Ext::default() }));
        n
    }

    fn import_whatsit(&mut self, w: &WhatIt) -> u32 {
        let whatsit = |this: &mut Self, sub: u16| this.lua_new_node(WHATSIT, sub);
        match w {
            WhatIt::Boundary { kind, value } => {
                let n = self.lua_new_node(BOUNDARY, u16::from(*kind));
                self.lua_nodes.node_mut(n).f[0] = *value;
                n
            }
            WhatIt::PdfLiteral { origin, data } => {
                let n = whatsit(self, ws::PDF_LITERAL);
                let node = self.lua_nodes.node_mut(n);
                node.f[0] = literal_mode_to_lua(*origin);
                node.ext.get_or_insert_with(Default::default).strs = vec![data.clone().into_bytes()];
                n
            }
            WhatIt::PdfColorStack { stack, cmd, data } => {
                let n = whatsit(self, ws::PDF_COLORSTACK);
                let node = self.lua_nodes.node_mut(n);
                node.f[0] = *stack;
                node.f[1] = match cmd {
                    ColorStackCmd::Set => 0,
                    ColorStackCmd::Push => 1,
                    ColorStackCmd::Pop => 2,
                    ColorStackCmd::Current => 3,
                };
                node.ext.get_or_insert_with(Default::default).strs = vec![data.clone().into_bytes()];
                n
            }
            WhatIt::PdfSave { .. } => whatsit(self, ws::PDF_SAVE),
            WhatIt::PdfRestore { .. } => whatsit(self, ws::PDF_RESTORE),
            WhatIt::PdfSetMatrix { matrix, .. } => {
                let n = whatsit(self, ws::PDF_SETMATRIX);
                self.lua_nodes.node_mut(n).ext.get_or_insert_with(Default::default).strs = vec![matrix.clone().into_bytes()];
                n
            }
            WhatIt::Special(data) => {
                let n = whatsit(self, ws::SPECIAL);
                self.lua_nodes.node_mut(n).ext.get_or_insert_with(Default::default).strs = vec![data.clone().into_bytes()];
                n
            }
            WhatIt::SavePos { obj } => {
                let n = whatsit(self, ws::SAVE_POS);
                self.lua_nodes.node_mut(n).f[10] = *obj;
                n
            }
            WhatIt::Write { stream, .. } => {
                let n = whatsit(self, ws::WRITE);
                self.lua_nodes.node_mut(n).f[0] = i32::from(*stream);
                self.keep_opaque(n, &Node::Whatsit(w.clone(), crate::boxes::Attr::NONE));
                n
            }
            WhatIt::OpenOut { stream, names, .. } => {
                let n = whatsit(self, ws::OPEN);
                self.lua_nodes.node_mut(n).f[0] = i32::from(*stream);
                self.lua_nodes.node_mut(n).ext.get_or_insert_with(Default::default).strs =
                    vec![names.0.clone().into_bytes(), Vec::new(), Vec::new()];
                self.keep_opaque(n, &Node::Whatsit(w.clone(), crate::boxes::Attr::NONE));
                n
            }
            WhatIt::CloseOut { stream, .. } => {
                let n = whatsit(self, ws::CLOSE);
                self.lua_nodes.node_mut(n).f[0] = i32::from(*stream);
                self.keep_opaque(n, &Node::Whatsit(w.clone(), crate::boxes::Attr::NONE));
                n
            }
            WhatIt::LateLua { code, func } => {
                let n = whatsit(self, ws::LATE_LUA);
                let node = self.lua_nodes.node_mut(n);
                node.f[0] = *func;
                node.ext.get_or_insert_with(Default::default).strs = vec![code.clone(), Vec::new(), Vec::new()];
                n
            }
            other => self.import_opaque(&Node::Whatsit(other.clone(), crate::boxes::Attr::NONE)),
        }
    }

    fn keep_opaque(&mut self, n: u32, node: &Node) {
        self.lua_nodes.node_mut(n).ext.get_or_insert_with(Default::default).opaque = Some(Box::new(node.clone()));
    }

    fn import_box(&mut self, node: &Node, ctx: &mut LangCtx) -> u32 {
        let Node::Box { kind, w, h, d, shift, list, glue_sign, glue_order, glue_set, lr, dir, subtype, .. } = node else {
            unreachable!()
        };
        let id = if *kind == boxes::HBOX { HLIST } else { VLIST };
        let n = self.lua_new_node(id, 0);
        let head = self.import_sub(list, ctx);
        let nd = self.lua_nodes.node_mut(n);
        nd.f[sl::B_WIDTH] = *w;
        nd.f[sl::B_HEIGHT] = *h;
        nd.f[sl::B_DEPTH] = *d;
        nd.f[sl::B_ORDER] = crate::lua_node_pack::lua_order_of(*glue_order);
        nd.f[sl::B_DIR] = if *dir == boxes::BOX_DIR_UNSET { -1 } else { i32::from(*dir) };
        nd.f[sl::B_SHIFT] = *shift;
        nd.f[sl::B_SIGN] = i32::from(*glue_sign);
        nd.fl = *glue_set;
        nd.f[sl::B_HEAD] = head as i32;
        nd.f[sl::B_LR] = i32::from(*lr);
        nd.f[sl::B_KIND] = i32::from(*kind);
        nd.subtype = u16::from(*subtype);
        n
    }

    fn import_node(&mut self, node: &Node, ctx: &mut LangCtx) -> u32 {
        let a = self.lua_attr_handle(node.attr());
        let saved = self.lua_nodes.import_attr.replace(a);
        let n = self.import_node_inner(node, ctx);
        self.lua_nodes.import_attr = saved;
        n
    }

    fn import_node_inner(&mut self, node: &Node, ctx: &mut LangCtx) -> u32 {
        match node {
            Node::Char { c, font, .. } => self.import_glyph_node(u32::from(*c), *font, ctx, 0),
            Node::LuaGlyph(g) => {
                let n = self.lua_new_node(GLYPH, u16::from(g.subtype));
                let comps = if g.components.is_empty() {
                    0
                } else {
                    let mut c = *ctx;
                    self.import_sub(&g.components, &mut c)
                };
                let f = &mut self.lua_nodes.node_mut(n).f;
                f[sl::C_CHAR] = g.c as i32;
                f[sl::C_FONT] = i32::from(g.font);
                f[sl::C_LANG] = i32::from(g.lang);
                f[sl::C_LEFT] = i32::from(g.left);
                f[sl::C_RIGHT] = i32::from(g.right);
                f[sl::C_UCHYPH] = i32::from(g.uchyph);
                f[sl::C_COMP] = comps as i32;
                f[sl::C_XOFF] = g.xoffset;
                f[sl::C_YOFF] = g.yoffset;
                f[sl::C_EXPAN] = g.expansion_factor;
                f[sl::C_DATA] = g.data;
                n
            }
            Node::Ligature { c, font, letters, n_letters, subtype, .. } => {
                let mut sub = GLYPH_LIGATURE;
                if subtype & 2 != 0 {
                    sub |= GLYPH_LEFT;
                }
                if subtype & 1 != 0 {
                    sub |= GLYPH_RIGHT;
                }
                let n = self.import_glyph_node(u32::from(*c), *font, ctx, sub);
                let mut head = 0;
                let mut tail = 0;
                for &l in &letters[..usize::from(*n_letters).min(3)] {
                    let g = self.import_glyph_node(u32::from(l), *font, ctx, GLYPH_CHARACTER);
                    if head == 0 {
                        head = g;
                    } else {
                        self.lua_nodes.couple(tail, g);
                    }
                    tail = g;
                }
                self.lua_nodes.node_mut(n).f[sl::C_COMP] = head as i32;
                n
            }
            Node::Glue(g, _) => self.import_glue(g, glue_subtype_to_lua(g.subtype)),
            Node::Leaders { glue, kind, body, .. } => {
                let n = self.import_glue(glue, A_LEADERS + u16::from(*kind));
                let leader = match body {
                    LeaderBody::Rule { width, height, depth, subtype } => {
                        let r = self.lua_new_node(RULE, u16::from(*subtype));
                        let f = &mut self.lua_nodes.node_mut(r).f;
                        f[sl::R_WIDTH] = rule_to_lua(*width);
                        f[sl::R_HEIGHT] = rule_to_lua(*height);
                        f[sl::R_DEPTH] = rule_to_lua(*depth);
                        r
                    }
                    LeaderBody::Box(b) => self.import_node(b, ctx),
                };
                self.lua_nodes.node_mut(n).f[sl::G_LEADER] = leader as i32;
                n
            }
            Node::Kern(k, _) => {
                let n = self.lua_new_node(KERN, FONT_KERN);
                self.lua_nodes.node_mut(n).f[0] = *k;
                n
            }
            Node::ItalicKern(k, _) => {
                let n = self.lua_new_node(KERN, ITALIC_KERN);
                self.lua_nodes.node_mut(n).f[0] = *k;
                n
            }
            Node::ExplicitKern(k, _) => {
                let n = self.lua_new_node(KERN, EXPLICIT_KERN);
                self.lua_nodes.node_mut(n).f[0] = *k;
                n
            }
            Node::AccentKern(k, _) => {
                let n = self.lua_new_node(KERN, ACCENT_KERN);
                self.lua_nodes.node_mut(n).f[0] = *k;
                n
            }
            Node::ExKern { width, ex, .. } => {
                let n = self.lua_new_node(KERN, FONT_KERN);
                let f = &mut self.lua_nodes.node_mut(n).f;
                f[0] = *width;
                f[1] = *ex;
                n
            }
            Node::MarginKern { side, width, c, font, ex, .. } => {
                let n = self.lua_new_node(MARGIN_KERN, u16::from(*side));
                let g = self.import_glyph_node(*c, *font, ctx, GLYPH_CHARACTER);
                self.lua_nodes.node_mut(g).f[sl::C_EXPAN] = *ex;
                let f = &mut self.lua_nodes.node_mut(n).f;
                f[sl::M_WIDTH] = *width;
                f[sl::M_GLYPH] = g as i32;
                n
            }
            Node::Penalty(p, _) => {
                let n = self.lua_new_node(PENALTY, 0);
                self.lua_nodes.node_mut(n).f[0] = *p;
                n
            }
            Node::Rule { width, height, depth, subtype, index, .. } => {
                let n = self.lua_new_node(RULE, u16::from(*subtype));
                let f = &mut self.lua_nodes.node_mut(n).f;
                f[sl::R_WIDTH] = rule_to_lua(*width);
                f[sl::R_HEIGHT] = rule_to_lua(*height);
                f[sl::R_DEPTH] = rule_to_lua(*depth);
                f[sl::R_INDEX] = *index;
                n
            }
            Node::Disc(dc) => {
                let n = self.lua_new_node(DISC, u16::from(dc.subtype));
                let mut c = *ctx;
                let pre = self.import_sub(&dc.pre_break, &mut c);
                let post = self.import_sub(&dc.post_break, &mut c);
                let rep = self.import_sub(&dc.no_break, &mut c);
                let penalty = if dc.penalty != boxes::DISC_PENALTY_TEX {
                    dc.penalty
                } else if dc.pre_break.is_empty() {
                    self.eqtb.int_params[IntParam::ExHyphenPenalty.idx() as usize]
                } else {
                    self.eqtb.int_params[IntParam::HyphenPenalty.idx() as usize]
                };
                let f = &mut self.lua_nodes.node_mut(n).f;
                f[sl::D_PRE] = pre as i32;
                f[sl::D_POST] = post as i32;
                f[sl::D_REPLACE] = rep as i32;
                f[sl::D_PENALTY] = penalty;
                n
            }
            Node::Box { .. } => self.import_box(node, ctx),
            Node::Mark { class, tokens, .. } => {
                let n = self.lua_new_node(MARK, 0);
                let nd = self.lua_nodes.node_mut(n);
                nd.f[0] = *class;
                nd.ext = Some(Box::new(Ext { toks: tokens.clone(), ..Ext::default() }));
                n
            }
            Node::Ins { num, height, depth, cost, split_top_skip, split_max_depth, box_node, .. } => {
                let n = self.lua_new_node(INS, *num);
                let spec = {
                    let s = self.lua_nodes.new_node(GLUE_SPEC, 0, 0);
                    let f = &mut self.lua_nodes.node_mut(s).f;
                    f[0] = split_top_skip.width;
                    f[1] = split_top_skip.stretch;
                    f[2] = split_top_skip.shrink;
                    f[3] = crate::lua_node_pack::lua_order_of(split_top_skip.stretch_order);
                    f[4] = crate::lua_node_pack::lua_order_of(split_top_skip.shrink_order);
                    s
                };
                let head = match &**box_node {
                    Node::Box { list, .. } => self.import_sub(list, ctx),
                    _ => 0,
                };
                let mut template = (**box_node).clone();
                if let Node::Box { list, .. } = &mut template {
                    list.clear();
                }
                let nd = self.lua_nodes.node_mut(n);
                nd.f[sl::I_COST] = *cost;
                nd.f[sl::I_DEPTH] = *split_max_depth;
                nd.f[sl::I_HEIGHT] = *height;
                nd.f[sl::I_SPEC] = spec as i32;
                nd.f[sl::I_HEAD] = head as i32;
                nd.f[sl::I_ENGINE_DEPTH] = *depth;
                nd.ext = Some(Box::new(Ext { opaque: Some(Box::new(template)), ..Ext::default() }));
                n
            }
            Node::VAdjust(list, _) => {
                let n = self.lua_new_node(ADJUST, 0);
                let head = self.import_sub(list, ctx);
                self.lua_nodes.node_mut(n).f[0] = head as i32;
                n
            }
            Node::MathKern(k, kind @ (boxes::MATH_ON | boxes::MATH_OFF), _) => {
                let n = self.lua_new_node(MATH, u16::from(*kind - 1));
                let nd = self.lua_nodes.node_mut(n);
                nd.f[0] = *k;
                nd.f[1] = *k;
                n
            }
            Node::Whatsit(w, _) => {
                if let WhatIt::Language { lang, lhm, rhm } = w {
                    ctx.lang = u16::from(*lang);
                    ctx.left = *lhm;
                    ctx.right = *rhm;
                }
                match w {
                    WhatIt::LocalPar(lp) => {
                        let n = self.lua_new_node(LOCAL_PAR, 0);
                        let left = self.import_sub(&lp.left, ctx);
                        let right = self.import_sub(&lp.right, ctx);
                        let f = &mut self.lua_nodes.node_mut(n).f;
                        f[0] = lp.pen_inter;
                        f[1] = lp.pen_broken;
                        f[2] = i32::from(lp.dir);
                        f[3] = left as i32;
                        f[4] = lp.left_width;
                        f[5] = right as i32;
                        f[6] = lp.right_width;
                        n
                    }
                    WhatIt::PdfRefXImage { obj, w: bw, h: bh, d: bd, .. } | WhatIt::PdfRefXForm { obj, w: bw, h: bh, d: bd } => {
                        let image = matches!(w, WhatIt::PdfRefXImage { .. });
                        let index = if image { self.lua_image_index(*obj) } else { *obj };
                        let n = self.lua_new_node(RULE, if image { 2 } else { 1 });
                        let f = &mut self.lua_nodes.node_mut(n).f;
                        f[sl::R_WIDTH] = *bw;
                        f[sl::R_HEIGHT] = *bh;
                        f[sl::R_DEPTH] = *bd;
                        f[sl::R_INDEX] = index;
                        if let WhatIt::PdfRefXImage { transform, .. } = w {
                            f[sl::R_TRANSFORM] = i32::from(*transform);
                        }
                        n
                    }
                    _ => self.import_whatsit(w),
                }
            }
            other => self.import_opaque(other),
        }
    }

    /// Export the list starting at `head`, consuming (freeing) the nodes.
    pub(crate) fn lua_nodes_to_engine(&mut self, head: i64) -> NodeList {
        let head = u32::try_from(head).unwrap_or(0);
        let mut out = Vec::new();
        self.export_list(head, &mut out);
        out
    }

    fn export_list(&mut self, head: u32, out: &mut NodeList) {
        let mut n = head;
        while self.lua_nodes.valid(n) {
            let next = self.lua_nodes.next(n);
            let (pre, post) = match self.lua_nodes.node_mut(n).ext.as_mut() {
                Some(e) => (std::mem::take(&mut e.pre), std::mem::take(&mut e.post)),
                None => (Vec::new(), Vec::new()),
            };
            out.extend(pre);
            self.export_node(n, out);
            out.extend(post);
            self.lua_nodes.flush_node(n);
            n = next;
        }
    }

    pub(crate) fn export_sub(&mut self, head: i32) -> NodeList {
        let mut v = Vec::new();
        self.export_list(head as u32, &mut v);
        v
    }

    fn export_glue_params(&self, n: u32, subtype: u8) -> Glue {
        let f = &self.lua_nodes.node(n).f;
        let zero_glue = f[sl::G_ZERO] != 0 && f[sl::G_WIDTH] == 0 && f[sl::G_STRETCH] == 0 && f[sl::G_SHRINK] == 0;
        Glue {
            width: f[sl::G_WIDTH],
            stretch: f[sl::G_STRETCH],
            shrink: f[sl::G_SHRINK],
            stretch_order: crate::lua_node_pack::engine_order(f[sl::G_SORDER]),
            shrink_order: crate::lua_node_pack::engine_order(f[sl::G_HORDER]),
            subtype,
            spec: if zero_glue { Glue::ZERO_SPEC } else { Glue::NO_SPEC },
        }
    }

    /// Append the engine form of node `n` (not its successors) to `out`.
    /// Lists below `n` are consumed.
    fn export_node(&mut self, n: u32, out: &mut NodeList) {
        let (id, attr) = (self.lua_nodes.id(n), self.lua_nodes.node(n).attr);
        let before = out.len();
        self.export_node_inner(n, out);
        if attr != 0 && id != TEMP {
            let a = self.engine_attr_of_list(attr);
            for node in &mut out[before..] {
                node.set_attr(a);
            }
        }
    }

    fn export_node_inner(&mut self, n: u32, out: &mut NodeList) {
        let (id, sub) = (self.lua_nodes.id(n), self.lua_nodes.subtype(n));
        let f = self.lua_nodes.node(n).f;
        match id {
            GLYPH => {
                let g = self.export_glyph(n);
                out.push(g);
            }
            GLUE => {
                if sub >= A_LEADERS {
                    let leader = f[sl::G_LEADER] as u32;
                    let body = if self.lua_nodes.valid(leader) {
                        let lid = self.lua_nodes.id(leader);
                        match lid {
                            RULE => {
                                let lf = self.lua_nodes.node(leader).f;
                                Some(LeaderBody::Rule {
                                    width: rule_from_lua(lf[sl::R_WIDTH]),
                                    height: rule_from_lua(lf[sl::R_HEIGHT]),
                                    depth: rule_from_lua(lf[sl::R_DEPTH]),
                                    subtype: self.lua_nodes.subtype(leader) as u8,
                                })
                            }
                            HLIST | VLIST => {
                                let mut v = Vec::new();
                                self.export_node(leader, &mut v);
                                v.pop().map(|b| LeaderBody::Box(Box::new(b)))
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };
                    // the leader is consumed here; keep it out of the later release
                    self.lua_nodes.node_mut(n).f[sl::G_LEADER] = 0;
                    let glue = self.export_glue_params(n, 0);
                    match body {
                        Some(body) => {
                            let kind = match sub {
                                A_LEADERS => boxes::LEADERS_A,
                                C_LEADERS => boxes::LEADERS_C,
                                X_LEADERS => boxes::LEADERS_X,
                                _ => boxes::LEADERS_G,
                            };
                            out.push(Node::Leaders { glue, kind, body, attr: crate::boxes::Attr::NONE });
                        }
                        None => out.push(Node::Glue(glue, crate::boxes::Attr::NONE)),
                    }
                    self.lua_nodes.flush_node(leader);
                } else {
                    let g = self.export_glue_params(n, glue_subtype_from_lua(sub));
                    out.push(Node::Glue(g, crate::boxes::Attr::NONE));
                }
            }
            KERN => {
                let k = f[0];
                out.push(match sub {
                    EXPLICIT_KERN => Node::ExplicitKern(k, crate::boxes::Attr::NONE),
                    ITALIC_KERN => Node::ItalicKern(k, crate::boxes::Attr::NONE),
                    ACCENT_KERN => Node::AccentKern(k, crate::boxes::Attr::NONE),
                    _ if f[1] != 0 => Node::ExKern { width: k, ex: f[1], attr: crate::boxes::Attr::NONE },
                    _ => Node::Kern(k, crate::boxes::Attr::NONE),
                });
            }
            MARGIN_KERN => {
                let g = f[sl::M_GLYPH] as u32;
                let (c, font, ex) = if self.lua_nodes.valid(g) {
                    let gf = self.lua_nodes.node(g).f;
                    (gf[sl::C_CHAR] as u32, gf[sl::C_FONT] as FontId, gf[sl::C_EXPAN])
                } else {
                    (0, 0, 0)
                };
                out.push(Node::MarginKern { side: sub as u8, width: f[sl::M_WIDTH], c, font, ex, attr: crate::boxes::Attr::NONE });
            }
            PENALTY => out.push(Node::Penalty(f[0], crate::boxes::Attr::NONE)),
            RULE => {
                let (w, h, d) = (f[sl::R_WIDTH], f[sl::R_HEIGHT], f[sl::R_DEPTH]);
                match sub {
                    1 | 2 => {
                        let wh = WhatIt::PdfRefXImage {
                            obj: self.lua_image_obj(f[sl::R_INDEX]),
                            w,
                            h,
                            d,
                            transform: (f[sl::R_TRANSFORM] & 7) as u8,
                        };
                        out.push(Node::Whatsit(if sub == 2 {
                            wh
                        } else {
                            WhatIt::PdfRefXForm { obj: f[sl::R_INDEX], w, h, d }
                        }, crate::boxes::Attr::NONE));
                    }
                    _ => out.push(Node::Rule {
                        width: rule_from_lua(w),
                        height: rule_from_lua(h),
                        depth: rule_from_lua(d),
                        subtype: sub as u8,
                        index: f[sl::R_INDEX],
                        attr: crate::boxes::Attr::NONE,
                    }),
                }
            }
            DISC => {
                let pre_break = self.export_sub(f[sl::D_PRE]);
                let post_break = self.export_sub(f[sl::D_POST]);
                let no_break = self.export_sub(f[sl::D_REPLACE]);
                out.push(Node::Disc(DiscNode {
                    pre_break,
                    post_break,
                    no_break,
                    replace_count: 0,
                    subtype: sub as u8,
                    penalty: f[sl::D_PENALTY], attr: crate::boxes::Attr::NONE,
                }));
                let nd = self.lua_nodes.node_mut(n);
                nd.f[sl::D_PRE] = 0;
                nd.f[sl::D_POST] = 0;
                nd.f[sl::D_REPLACE] = 0;
            }
            HLIST | VLIST | UNSET => {
                // an unset node keeps its list one slot further (`node.fields`)
                let head_slot = if id == UNSET { sl::U_HEAD } else { sl::B_HEAD };
                let list = self.export_sub(f[head_slot]);
                self.lua_nodes.node_mut(n).f[head_slot] = 0;
                let nd = self.lua_nodes.node(n);
                let kind = if id == VLIST {
                    if f[sl::B_KIND] == i32::from(boxes::VTOP) { boxes::VTOP } else { boxes::VBOX }
                } else {
                    boxes::HBOX
                };
                out.push(Node::Box {
                    kind,
                    w: f[sl::B_WIDTH],
                    list,
                    h: f[sl::B_HEIGHT],
                    d: f[sl::B_DEPTH],
                    // an unset node keeps its shrink total in the slot
                    shift: if id == UNSET { 0 } else { f[sl::B_SHIFT] },
                    glue_order: crate::lua_node_pack::engine_order(f[sl::B_ORDER]),
                    glue_sign: f[sl::B_SIGN] as u8,
                    glue_set: nd.fl,
                    lr: if nd.subtype == 6 { BOX_LR_DLIST } else { f[sl::B_LR] as u8 },
                    dir: f[sl::B_DIR] as u8, attr: crate::boxes::Attr::NONE, subtype: nd.subtype as u8,
                });
            }
            MARK => {
                let toks = self.lua_nodes.node(n).ext.as_ref().map(|e| e.toks.clone()).unwrap_or_default();
                out.push(Node::Mark { class: f[0], tokens: toks, attr: crate::boxes::Attr::NONE });
            }
            INS => {
                let list = self.export_sub(f[sl::I_HEAD]);
                self.lua_nodes.node_mut(n).f[sl::I_HEAD] = 0;
                let spec = f[sl::I_SPEC] as u32;
                let skip = if self.lua_nodes.valid(spec) {
                    let sf = self.lua_nodes.node(spec).f;
                    Glue {
                        width: sf[0],
                        stretch: sf[1],
                        shrink: sf[2],
                        stretch_order: crate::lua_node_pack::engine_order(sf[3]),
                        shrink_order: crate::lua_node_pack::engine_order(sf[4]),
                        ..Glue::default()
                    }
                } else {
                    Glue::default()
                };
                let mut box_node = self
                    .lua_nodes
                    .node(n)
                    .ext
                    .as_ref()
                    .and_then(|e| e.opaque.as_deref().cloned())
                    .unwrap_or(Node::Box {
                        kind: boxes::VBOX,
                        w: 0,
                        h: 0,
                        d: 0,
                        shift: 0,
                        list: Vec::new(),
                        glue_sign: 0,
                        glue_order: 0,
                        glue_set: 0.0,
                        lr: 0,
                        dir: 0, attr: crate::boxes::Attr::NONE, subtype: 0,
                    });
                if let Node::Box { list: l, .. } = &mut box_node {
                    *l = list;
                }
                out.push(Node::Ins {
                    num: sub,
                    height: f[sl::I_HEIGHT],
                    depth: f[sl::I_ENGINE_DEPTH],
                    cost: f[sl::I_COST],
                    split_top_skip: skip,
                    split_max_depth: f[sl::I_DEPTH],
                    box_node: Box::new(box_node), attr: crate::boxes::Attr::NONE,
                });
            }
            ADJUST => {
                let list = self.export_sub(f[0]);
                self.lua_nodes.node_mut(n).f[0] = 0;
                out.push(Node::VAdjust(list, crate::boxes::Attr::NONE));
            }
            MATH => {
                out.push(Node::MathKern(f[0], (sub as u8).min(1) + 1, crate::boxes::Attr::NONE));
            }
            BOUNDARY => out.push(Node::Whatsit(WhatIt::Boundary { kind: sub as u8, value: f[0] }, crate::boxes::Attr::NONE)),
            LOCAL_PAR => {
                let left = self.export_sub(f[3]);
                let right = self.export_sub(f[5]);
                let nd = self.lua_nodes.node_mut(n);
                nd.f[3] = 0;
                nd.f[5] = 0;
                out.push(Node::Whatsit(
                    WhatIt::LocalPar(Box::new(crate::boxes::LocalPar {
                        pen_inter: f[0],
                        pen_broken: f[1],
                        dir: (f[2] & 3) as u8,
                        left,
                        left_width: f[4],
                        right,
                        right_width: f[6],
                    })),
                    crate::boxes::Attr::NONE,
                ));
            }
            WHATSIT => self.export_whatsit(n, out),
            TEMP => {
                if let Some(o) = self.lua_nodes.node(n).ext.as_ref().and_then(|e| e.opaque.as_deref().cloned()) {
                    out.push(o);
                }
            }
            STYLE | CHOICE | NOAD | RADICAL | FRACTION | ACCENT | FENCE | MATH_CHAR | MATH_TEXT_CHAR | SUB_BOX | SUB_MLIST => {
                self.export_math_node(n, out)
            }
            _ => {
                // a node the engine has no use for (glue_spec, attribute, ...)
                // is dropped
            }
        }
    }

    fn export_glyph(&mut self, n: u32) -> Node {
        let nd = self.lua_nodes.node(n);
        let f = nd.f;
        let sub = nd.subtype;
        let font = f[sl::C_FONT] as FontId;
        let c = f[sl::C_CHAR] as u32;
        let comp_head = f[sl::C_COMP] as u32;
        let lua_font = self.eqtb.fonts.get(usize::from(font)).is_some_and(|ft| ft.lua.is_some());
        let plain = !lua_font
            && c < 256
            && f[sl::C_XOFF] == 0
            && f[sl::C_YOFF] == 0
            && f[sl::C_EXPAN] == 0
            && f[sl::C_DATA] == 0;
        if plain && comp_head == 0 && sub & GLYPH_LIGATURE == 0 {
            return Node::Char { c: c as u8, font, attr: crate::boxes::Attr::NONE };
        }
        if plain && comp_head != 0 && self.lua_nodes.list_len(comp_head) <= 3 {
            let mut letters = [0u8; 3];
            let mut cnt = 0usize;
            let mut ok = true;
            let mut p = comp_head;
            while p != 0 {
                let pn = self.lua_nodes.node(p);
                if pn.id != GLYPH || pn.f[sl::C_CHAR] > 255 || pn.f[sl::C_COMP] != 0 {
                    ok = false;
                    break;
                }
                letters[cnt] = pn.f[sl::C_CHAR] as u8;
                cnt += 1;
                p = pn.next;
            }
            if ok {
                self.lua_nodes.flush_list(comp_head);
                self.lua_nodes.node_mut(n).f[sl::C_COMP] = 0;
                let fonts = crate::boxes::eqtb_fonts(&self.eqtb);
                use crate::fonts::FontResolver;
                let lig_width = fonts.char_width(font, c as u8);
                let lig_height = fonts.char_height(font, c as u8);
                let lig_depth = fonts.char_depth(font, c as u8);
                let mut st = 0u8;
                if sub & GLYPH_LEFT != 0 {
                    st |= 2;
                }
                if sub & GLYPH_RIGHT != 0 {
                    st |= 1;
                }
                return Node::Ligature {
                    c: c as u8,
                    font,
                    lig_width,
                    lig_height,
                    lig_depth,
                    letters,
                    n_letters: cnt as u8,
                    subtype: st, attr: crate::boxes::Attr::NONE,
                };
            }
        }
        let mut components = Vec::new();
        if comp_head != 0 {
            self.export_list(comp_head, &mut components);
            self.lua_nodes.node_mut(n).f[sl::C_COMP] = 0;
        }
        let nd = self.lua_nodes.node(n);
        let f = nd.f;
        Node::LuaGlyph(Box::new(LuaGlyph {
            c,
            font,
            lang: f[sl::C_LANG] as u16,
            left: f[sl::C_LEFT] as u8,
            right: f[sl::C_RIGHT] as u8,
            uchyph: f[sl::C_UCHYPH] as u8,
            xoffset: f[sl::C_XOFF],
            yoffset: f[sl::C_YOFF],
            expansion_factor: f[sl::C_EXPAN],
            data: f[sl::C_DATA],
            subtype: sub as u8,
            components, attr: crate::boxes::Attr::NONE,
        }))
    }

    fn export_whatsit(&mut self, n: u32, out: &mut NodeList) {
        let sub = self.lua_nodes.subtype(n);
        let f = self.lua_nodes.node(n).f;
        let opaque = self.lua_nodes.node(n).ext.as_ref().and_then(|e| e.opaque.as_deref().cloned());
        let s = |this: &Self, i: usize| -> String {
            this.lua_nodes
                .node(n)
                .ext
                .as_ref()
                .and_then(|e| e.strs.get(i))
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default()
        };
        match sub {
            ws::PDF_LITERAL | ws::PDF_LATE_LITERAL => out.push(Node::Whatsit(WhatIt::PdfLiteral {
                origin: literal_mode_from_lua(f[0]),
                data: s(self, 0),
            }, crate::boxes::Attr::NONE)),
            ws::PDF_COLORSTACK => out.push(Node::Whatsit(WhatIt::PdfColorStack {
                stack: f[0],
                cmd: match f[1] {
                    1 => ColorStackCmd::Push,
                    2 => ColorStackCmd::Pop,
                    3 => ColorStackCmd::Current,
                    _ => ColorStackCmd::Set,
                },
                data: s(self, 0),
            }, crate::boxes::Attr::NONE)),
            ws::PDF_SAVE => out.push(Node::Whatsit(WhatIt::PdfSave { source: None }, crate::boxes::Attr::NONE)),
            ws::PDF_RESTORE => out.push(Node::Whatsit(WhatIt::PdfRestore { source: None }, crate::boxes::Attr::NONE)),
            ws::PDF_SETMATRIX => out.push(Node::Whatsit(WhatIt::PdfSetMatrix { matrix: s(self, 0), source: None }, crate::boxes::Attr::NONE)),
            ws::SPECIAL | ws::LATE_SPECIAL => out.push(Node::Whatsit(WhatIt::Special(s(self, 0)), crate::boxes::Attr::NONE)),
            ws::LATE_LUA => {
                let code = self.lua_nodes.node(n).ext.as_ref().and_then(|e| e.strs.first().cloned()).unwrap_or_default();
                out.push(Node::Whatsit(WhatIt::LateLua { code, func: f[0].max(0) }, crate::boxes::Attr::NONE));
            }
            ws::SAVE_POS => out.push(Node::Whatsit(WhatIt::SavePos { obj: f[10] }, crate::boxes::Attr::NONE)),
            ws::WRITE | ws::CLOSE | ws::OPEN => {
                if let Some(Node::Whatsit(mut w, _)) = opaque {
                    match &mut w {
                        WhatIt::Write { stream, .. } | WhatIt::CloseOut { stream, .. } | WhatIt::OpenOut { stream, .. } => {
                            *stream = f[0] as u16;
                        }
                        _ => {}
                    }
                    out.push(Node::Whatsit(w, crate::boxes::Attr::NONE));
                }
            }
            _ => {
                // whatsits the engine cannot execute are dropped
            }
        }
    }
}

#[allow(dead_code)]
fn _unused(_: &Option<std::marker::PhantomData<LuaGlyph>>) {}
