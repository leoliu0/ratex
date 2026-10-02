//! LuaTeX font expansion and character protrusion for glyph nodes
//! (packaging.c `char_stretch`, `kern_stretch`, `do_subst_font`, `char_pw`,
//! `hpack(.., cal_expand_ratio)`; linebreak.c `find_protchar_left/right`).
//!
//! LuaTeX keeps the font of an expanded glyph and records the expansion in
//! the node (`ex_glyph`, millionths) instead of switching to an expanded
//! copy of the font as pdfTeX does. The expansion limits (`step`, `stretch`,
//! `shrink`) and the per-character `expansion_factor`, `left_protruding` and
//! `right_protruding` live with the Lua font.

use crate::boxes::{Attr, Node, NodeList, PackResult, WhatIt};
use crate::eqtb::Eqtb;
use crate::tfm::round_xn_over_d;
use crate::tfm::FontId;

/// A glyph node as the protrusion and expansion code sees it.
#[derive(Clone, Copy, Debug)]
pub struct GlyphRef {
    pub font: FontId,
    pub c: u32,
}

/// The glyph a node stands for (luatex `is_char_node`).
#[inline]
pub fn glyph_ref(n: &Node) -> Option<GlyphRef> {
    match n {
        Node::Char { c, font, .. } | Node::Ligature { c, font, .. } => Some(GlyphRef { font: *font, c: u32::from(*c) }),
        Node::LuaGlyph(g) => Some(GlyphRef { font: g.font, c: g.c }),
        _ => None,
    }
}

/// luatex `ext_xn_over_d`: `x * n / d` rounded in floating point.
pub fn ext_xn_over_d(x: i64, n: i64, d: i64) -> i32 {
    let mut r = (x as f64 * n as f64) / d as f64;
    if r > f64::EPSILON {
        r += 0.5;
    } else {
        r -= 0.5;
    }
    r as i32
}

/// luatex `divide_scaled_n`.
fn divide_scaled_n(sd: f64, md: f64, n: f64) -> i32 {
    let dd = sd / md * n;
    if dd > 0.0 {
        (dd + 0.5).floor() as i32
    } else if dd < 0.0 {
        -((-dd + 0.5).floor() as i32)
    } else {
        0
    }
}

/// The width of a glyph that is `ex` millionths expanded (luatex
/// `pack_width`).
#[inline]
pub fn expanded_width(w: i32, ex: i32) -> i32 {
    if ex != 0 {
        ext_xn_over_d(i64::from(w), 1_000_000 + i64::from(ex), 1_000_000)
    } else {
        w
    }
}

/// `(step, stretch, shrink)` expansion limits of font `f`.
#[inline]
pub fn limits(eqtb: &Eqtb, f: FontId) -> (i32, i32, i32) {
    match eqtb.fonts.get(usize::from(f)).and_then(|font| font.lua.as_ref()) {
        Some(lf) => (lf.step, lf.stretch, lf.shrink),
        None => eqtb.expand.get(usize::from(f)).map_or((0, 0, 0), |x| (x.lua_step, x.lua_stretch, x.lua_shrink)),
    }
}

/// linebreak.c `check_expand_pars`' first test: has the font a step and a
/// stretch or shrink limit?
#[inline]
pub fn expandable(eqtb: &Eqtb, f: FontId) -> bool {
    let (step, st, sh) = limits(eqtb, f);
    step != 0 && (st != 0 || sh != 0)
}

/// `get_ef_code`.
pub fn ef_code(eqtb: &Eqtb, f: FontId, c: u32) -> i32 {
    let Some(font) = eqtb.fonts.get(usize::from(f)) else { return 0 };
    // luatex's null charinfo of a character the font lacks has no expansion
    if font.lua.is_some() {
        return font.lua_char(c).map_or(0, |ci| ci.expansion_factor);
    }
    match u8::try_from(c) {
        Ok(c) if font.char_present(c) => eqtb.expand.get(usize::from(f)).map_or(1000, |x| x.ef_code(c)),
        _ => 0,
    }
}

/// `get_lp_code` / `get_rp_code`.
pub fn pw_code(eqtb: &Eqtb, f: FontId, c: u32, left: bool) -> i32 {
    let Some(font) = eqtb.fonts.get(usize::from(f)) else { return 0 };
    if font.lua.is_some() {
        return font.lua_char(c).map_or(0, |ci| if left { ci.left_protruding } else { ci.right_protruding });
    }
    match u8::try_from(c) {
        Ok(c) if font.char_present(c) => {
            eqtb.expand.get(usize::from(f)).map_or(0, |x| if left { x.lp_code(c) } else { x.rp_code(c) })
        }
        _ => 0,
    }
}

/// luatex `quad(f)`: parameter 6, zero when the font has none.
fn quad(eqtb: &Eqtb, f: FontId) -> i32 {
    eqtb.font_params.get(usize::from(f)).and_then(|p| p.get(5).copied()).unwrap_or(0)
}

/// `char_pw`: how far the glyph protrudes into the margin.
pub fn char_pw(eqtb: &Eqtb, g: GlyphRef, left: bool) -> i32 {
    let code = pw_code(eqtb, g.font, g.c, left);
    if code == 0 {
        return 0;
    }
    round_xn_over_d(quad(eqtb, g.font), code, 1000)
}

fn char_width(eqtb: &Eqtb, f: FontId, c: u32) -> i32 {
    let Some(font) = eqtb.fonts.get(usize::from(f)) else { return 0 };
    if font.lua.is_some() {
        return font.lua_char(c).map_or(0, |ci| ci.width);
    }
    match u8::try_from(c) {
        Ok(c) if font.char_present(c) => font.char_width(c),
        _ => 0,
    }
}

/// `calc_char_width(f, c, ex)`.
fn calc_char_width(eqtb: &Eqtb, f: FontId, c: u32, ex: i32) -> i32 {
    let w = char_width(eqtb, f, c);
    if ex != 0 {
        round_xn_over_d(w, 1000 + ex, 1000)
    } else {
        w
    }
}

/// packaging.c `char_stretch`.
pub fn char_stretch(eqtb: &Eqtb, g: GlyphRef) -> i32 {
    let (_, m, _) = limits(eqtb, g.font);
    if m > 0 {
        let ef = ef_code(eqtb, g.font, g.c);
        if ef > 0 {
            let dw = calc_char_width(eqtb, g.font, g.c, m) - char_width(eqtb, g.font, g.c);
            if dw > 0 {
                return round_xn_over_d(dw, ef, 1000);
            }
        }
    }
    0
}

/// packaging.c `char_shrink`.
pub fn char_shrink(eqtb: &Eqtb, g: GlyphRef) -> i32 {
    let (_, _, m) = limits(eqtb, g.font);
    if m > 0 {
        let ef = ef_code(eqtb, g.font, g.c);
        if ef > 0 {
            let dw = char_width(eqtb, g.font, g.c) - calc_char_width(eqtb, g.font, g.c, -m);
            if dw > 0 {
                return round_xn_over_d(dw, ef, 1000);
            }
        }
    }
    0
}

/// packaging.c `kern_stretch` for a font kern of width `w` between glyphs
/// `l` and `r`.
pub fn kern_stretch(eqtb: &Eqtb, w: i32, l: GlyphRef, r: GlyphRef) -> i32 {
    if w == 0 {
        return 0;
    }
    let m = (limits(eqtb, l.font).1 + limits(eqtb, r.font).1) / 2;
    if m == 0 {
        return 0;
    }
    let d = round_xn_over_d(w, 1000 + m, 1000);
    let e = (ef_code(eqtb, l.font, l.c) + ef_code(eqtb, r.font, r.c)) / 2;
    if e == 1000 {
        d - w
    } else {
        round_xn_over_d(d - w, e, 1000)
    }
}

/// packaging.c `kern_shrink`.
pub fn kern_shrink(eqtb: &Eqtb, w: i32, l: GlyphRef, r: GlyphRef) -> i32 {
    if w == 0 {
        return 0;
    }
    let m = (limits(eqtb, l.font).2 + limits(eqtb, r.font).2) / 2;
    if m == 0 {
        return 0;
    }
    let d = round_xn_over_d(w, 1000 - m, 1000);
    let e = (ef_code(eqtb, l.font, l.c) + ef_code(eqtb, r.font, r.c)) / 2;
    if e == 1000 {
        w - d
    } else {
        round_xn_over_d(w - d, e, 1000)
    }
}

/// texfont.c `fix_expand_value`: the multiple of the font's step nearest to
/// `e`, within its limits.
fn fix_expand_value(eqtb: &Eqtb, f: FontId, e: i32) -> i32 {
    if e == 0 {
        return 0;
    }
    let (step, st, sh) = limits(eqtb, f);
    let (mut e, neg, max_expand) = if e < 0 { (-e, true, sh) } else { (e, false, st) };
    if e > max_expand {
        e = max_expand;
    } else if step > 0 && e % step > 0 {
        e = step * round_xn_over_d(e, 1, step);
    }
    if neg {
        -e
    } else {
        e
    }
}

/// packaging.c `do_subst_font` for one glyph: the new `expansion_factor`
/// (the old one stays when the font can neither stretch nor shrink).
pub fn subst_font(eqtb: &Eqtb, g: GlyphRef, ex_ratio: i32, cur: i32) -> i32 {
    let ef = ef_code(eqtb, g.font, g.c);
    if ef == 0 {
        return cur;
    }
    let (_, st, sh) = limits(eqtb, g.font);
    let ratio_ef = i64::from(ex_ratio) * i64::from(ef);
    if st > 0 && ex_ratio > 0 {
        fix_expand_value(eqtb, g.font, ext_xn_over_d(ratio_ef, i64::from(st), 1_000_000)) * 1000
    } else if sh > 0 && ex_ratio < 0 {
        fix_expand_value(eqtb, g.font, ext_xn_over_d(ratio_ef, i64::from(sh), 1_000_000)) * 1000
    } else {
        cur
    }
}

// ---------------------------------------------------------------------
// protrusion: skipping over what cannot protrude
// ---------------------------------------------------------------------

fn zero_glue(g: &crate::boxes::Glue) -> bool {
    g.width == 0 && g.stretch == 0 && g.shrink == 0
}

/// linebreak.h `cp_skipable` (a glyph is never skipable).
fn cp_skipable(n: &Node) -> bool {
    match n {
        Node::Glue(g, _) => zero_glue(g),
        Node::Penalty(..) => true,
        Node::Disc(d) => d.pre_break.is_empty() && d.post_break.is_empty() && d.no_break.is_empty(),
        Node::Kern(..) | Node::ExKern { .. } => true,
        Node::ExplicitKern(k, _) | Node::AccentKern(k, _) | Node::ItalicKern(k, _) | Node::SpaceAdjKern(k, _) => *k == 0,
        Node::Rule { width, height, depth, .. } => *width == 0 && *height == 0 && *depth == 0,
        Node::MathKern(_, kind, _) => *kind >= crate::boxes::MATH_ON,
        Node::Box { kind, w, h, d, list, .. } => *kind == crate::boxes::HBOX && list.is_empty() && *w == 0 && *h == 0 && *d == 0,
        Node::Ins { .. } | Node::Mark { .. } | Node::Adj(..) | Node::VAdjust(..) | Node::PreAdjust(..) => true,
        Node::Whatsit(..) => true,
        _ => false,
    }
}

/// texnodes.h `non_discardable`.
fn non_discardable(n: &Node) -> bool {
    !matches!(
        n,
        Node::Glue(..)
            | Node::Leaders { .. }
            | Node::Kern(..)
            | Node::ExKern { .. }
            | Node::ExplicitKern(..)
            | Node::AccentKern(..)
            | Node::ItalicKern(..) | Node::SpaceAdjKern(..)
            | Node::MathKern(..)
            | Node::Penalty(..)
    )
}

fn is_empty_zero_hbox(n: &Node) -> bool {
    matches!(n, Node::Box { kind, w: 0, h: 0, d: 0, list, .. } if *kind == crate::boxes::HBOX && list.is_empty())
}

/// `\protrusionboundary` value of a node.
fn protrusion_boundary(n: &Node) -> Option<i32> {
    match n {
        Node::Whatsit(WhatIt::Boundary { kind, value }, _) if *kind == crate::boxes::BOUNDARY_PROTRUSION => Some(*value),
        _ => None,
    }
}

/// What a protrusion search ended on.
enum Walk {
    Found(GlyphRef, Attr),
    /// a node that stops the search without being a glyph
    Stop,
    /// everything was skipable
    Exhausted,
}

/// A non-empty horizontal box the searches descend into.
fn inner_hlist(n: &Node) -> Option<&NodeList> {
    match n {
        Node::Box { kind, list, .. } if *kind == crate::boxes::HBOX && !list.is_empty() => Some(list),
        _ => None,
    }
}

#[inline]
fn is_dead(dead: &[bool], i: usize) -> bool {
    dead.get(i).copied().unwrap_or(false)
}

/// The leftmost non-skipable node of `nodes[start..]` (the walk of
/// `find_protchar_left` once its prelude is over).
fn left_walk(nodes: &[Node], start: usize, dead: &[bool]) -> Walk {
    let mut i = start;
    while i < nodes.len() {
        if is_dead(dead, i) {
            i += 1;
            continue;
        }
        let n = &nodes[i];
        if let Some(g) = glyph_ref(n) {
            return Walk::Found(g, n.attr());
        }
        if let Some(list) = inner_hlist(n) {
            match left_walk(list, 0, &[]) {
                Walk::Exhausted => {}
                found => return found,
            }
            i += 1;
            continue;
        }
        if !cp_skipable(n) {
            return Walk::Stop;
        }
        if matches!(protrusion_boundary(n), Some(1 | 3)) && i + 1 < nodes.len() {
            // skip the node after the boundary
            i += 1;
        }
        i += 1;
    }
    Walk::Exhausted
}

/// The rightmost non-skipable node of `nodes[lo..=r]`.
fn right_walk(nodes: &[Node], lo: usize, r: usize, dead: &[bool]) -> Walk {
    let mut i = r as isize;
    while i >= lo as isize {
        let iu = i as usize;
        if is_dead(dead, iu) {
            i -= 1;
            continue;
        }
        let n = &nodes[iu];
        if let Some(g) = glyph_ref(n) {
            return Walk::Found(g, n.attr());
        }
        if let Some(list) = inner_hlist(n) {
            match right_walk(list, 0, list.len() - 1, &[]) {
                Walk::Exhausted => {}
                found => return found,
            }
            i -= 1;
            continue;
        }
        if !cp_skipable(n) {
            return Walk::Stop;
        }
        if matches!(protrusion_boundary(n), Some(2 | 3)) && iu > lo {
            // skip the node before the boundary
            i -= 1;
        }
        i -= 1;
    }
    Walk::Exhausted
}

/// linebreak.c `find_protchar_left(l, d)` for the list starting at
/// `nodes[start]`: the glyph that may protrude into the left margin.
pub fn find_protchar_left(nodes: &[Node], start: usize, d: bool, dead: &[bool]) -> Option<(GlyphRef, Attr)> {
    let mut i = start;
    if i >= nodes.len() {
        return None;
    }
    let mut done = false;
    while i + 1 < nodes.len() && is_empty_zero_hbox(&nodes[i]) {
        i += 1;
        done = true;
    }
    if !done && d {
        while i + 1 < nodes.len() && !(glyph_ref(&nodes[i]).is_some() || non_discardable(&nodes[i])) {
            i += 1;
        }
    }
    match left_walk(nodes, i, dead) {
        Walk::Found(g, a) => Some((g, a)),
        _ => None,
    }
}

/// linebreak.c `find_protchar_right(l, r)`: the glyph that may protrude into
/// the right margin, searching back from `nodes[r]` to `nodes[lo]`.
pub fn find_protchar_right(nodes: &[Node], lo: usize, r: usize, dead: &[bool]) -> Option<(GlyphRef, Attr)> {
    if r >= nodes.len() || r < lo {
        return None;
    }
    match right_walk(nodes, lo, r, dead) {
        Walk::Found(g, a) => Some((g, a)),
        _ => None,
    }
}

/// `left_pw(l1)` of `try_break` for the line that starts at the break node
/// `list[start]`.
pub fn break_left_pw(eqtb: &Eqtb, list: &[Node], start: usize, dead: &[bool]) -> i32 {
    if let Some(Node::Disc(dc)) = list.get(start) {
        if !dc.post_break.is_empty() {
            return glyph_ref(&dc.post_break[0]).map_or(0, |g| char_pw(eqtb, g, true));
        }
    }
    find_protchar_left(list, start, true, dead).map_or(0, |(g, _)| char_pw(eqtb, g, true))
}

/// `right_pw(o)` of `try_break` for a line from the break node `list[lo]`
/// to the break at `cand` (`is_disc`: `list[cand]` is a discretionary).
pub fn break_right_pw(eqtb: &Eqtb, list: &[Node], lo: usize, cand: usize, is_disc: bool, dead: &[bool]) -> i32 {
    if is_disc {
        if let Some(Node::Disc(dc)) = list.get(cand) {
            if let Some(last) = dc.pre_break.last() {
                return glyph_ref(last).map_or(0, |g| char_pw(eqtb, g, false));
            }
        }
    }
    if cand == 0 || cand > list.len() {
        return 0;
    }
    find_protchar_right(list, lo, cand - 1, dead).map_or(0, |(g, _)| char_pw(eqtb, g, false))
}

/// Marks the nodes a discretionary replaces (they follow it in the
/// paragraph list but are not part of it).
pub fn dead_mask(list: &[Node]) -> Vec<bool> {
    let mut dead = vec![false; list.len()];
    let mut i = 0;
    while i < list.len() {
        if let Node::Disc(dc) = &list[i] {
            for k in 0..dc.replace_count {
                if let Some(slot) = dead.get_mut(i + 1 + k) {
                    *slot = true;
                }
            }
            i += 1 + dc.replace_count;
        } else {
            i += 1;
        }
    }
    dead
}

// ---------------------------------------------------------------------
// hpack with font expansion
// ---------------------------------------------------------------------

/// The font kern `list[i]` (width `w`) sits between two glyph nodes: its
/// neighbours.
fn kern_neighbours(list: &[Node], i: usize) -> Option<(GlyphRef, GlyphRef)> {
    let l = glyph_ref(list.get(i.checked_sub(1)?)?)?;
    let r = glyph_ref(list.get(i + 1)?)?;
    Some((l, r))
}

/// Add the expansion room of `list` to `stretch`/`shrink` (packaging.c
/// `hpack` with `m = cal_expand_ratio`). `kerns` is false when
/// `\adjustspacing` is above 2.
fn collect(eqtb: &Eqtb, list: &[Node], kerns: bool, stretch: &mut i64, shrink: &mut i64) {
    for (i, n) in list.iter().enumerate() {
        match n {
            Node::Kern(w, _) if kerns => {
                if let Some((l, r)) = kern_neighbours(list, i) {
                    *stretch += i64::from(kern_stretch(eqtb, *w, l, r));
                    *shrink += i64::from(kern_shrink(eqtb, *w, l, r));
                }
            }
            Node::Disc(dc) => collect(eqtb, &dc.no_break, kerns, stretch, shrink),
            n => {
                if let Some(g) = glyph_ref(n) {
                    *stretch += i64::from(char_stretch(eqtb, g));
                    *shrink += i64::from(char_shrink(eqtb, g));
                }
            }
        }
    }
}

/// The glyph node a character or ligature node of a TFM font becomes once
/// it is expanded: LuaTeX keeps one glyph node type whose `ex_glyph` holds
/// the expansion (`ctx` is the hyphenation state the importer to Lua gives
/// such a node as well).
fn expanded_glyph(n: &Node, ex: i32, ctx: crate::lua_node_conv::LangCtx) -> Option<Node> {
    use crate::lua_node::{GLYPH_CHARACTER, GLYPH_LEFT, GLYPH_LIGATURE, GLYPH_RIGHT};
    let glyph = |c: u32, font: FontId, subtype: u16, components: NodeList, attr: Attr| {
        Node::LuaGlyph(Box::new(crate::boxes::LuaGlyph {
            c,
            font,
            lang: ctx.lang,
            left: ctx.left,
            right: ctx.right,
            uchyph: ctx.uchyph,
            xoffset: 0,
            yoffset: 0,
            expansion_factor: ex,
            data: 0,
            subtype: subtype as u8,
            components,
            attr,
        }))
    };
    match n {
        Node::Char { c, font, attr } => Some(glyph(u32::from(*c), *font, GLYPH_CHARACTER, Vec::new(), *attr)),
        Node::Ligature { c, font, letters, n_letters, subtype, attr, .. } => {
            let mut sub = GLYPH_LIGATURE;
            if subtype & 2 != 0 {
                sub |= GLYPH_LEFT;
            }
            if subtype & 1 != 0 {
                sub |= GLYPH_RIGHT;
            }
            let components = letters[..usize::from(*n_letters).min(3)]
                .iter()
                .map(|&l| Node::Char { c: l, font: *font, attr: *attr })
                .collect();
            Some(glyph(u32::from(*c), *font, sub, components, *attr))
        }
        _ => None,
    }
}

/// `do_subst_font` for one node of a list: a glyph takes its new
/// `expansion_factor`, a character of a TFM font becomes a glyph node
/// carrying it.
fn subst_glyph(eqtb: &Eqtb, node: &mut Node, ratio: i32, ctx: crate::lua_node_conv::LangCtx) {
    let Some(gr) = glyph_ref(node) else { return };
    if let Node::LuaGlyph(g) = node {
        g.expansion_factor = subst_font(eqtb, gr, ratio, g.expansion_factor);
        return;
    }
    let ex = subst_font(eqtb, gr, ratio, 0);
    if ex != 0 {
        if let Some(g) = expanded_glyph(node, ex, ctx) {
            *node = g;
        }
    }
}

/// `do_subst_font` over the glyphs of a list (packaging.c `hpack` with
/// `m = subst_ex_font`); `kerns` as in [`collect`].
fn substitute(eqtb: &Eqtb, list: &mut NodeList, ratio: i32, kerns: bool, ctx: crate::lua_node_conv::LangCtx) {
    for i in 0..list.len() {
        match &mut list[i] {
            Node::Disc(dc) => {
                for part in [&mut dc.pre_break, &mut dc.post_break, &mut dc.no_break] {
                    for n in part.iter_mut() {
                        subst_glyph(eqtb, n, ratio, ctx);
                    }
                }
                substitute_kerns(eqtb, &mut dc.no_break, ratio, kerns);
            }
            Node::MarginKern { width, side, font, c, ex, .. } => {
                let gr = GlyphRef { font: *font, c: *c };
                *ex = subst_font(eqtb, gr, ratio, *ex);
                *width = -char_pw(eqtb, gr, *side == 0);
            }
            n => subst_glyph(eqtb, n, ratio, ctx),
        }
    }
    substitute_kerns(eqtb, list, ratio, kerns);
}

/// The `ex_kern` half of `subst_ex_font`: stretch or shrink the font kerns.
fn substitute_kerns(eqtb: &Eqtb, list: &mut NodeList, ratio: i32, kerns: bool) {
    if !kerns {
        return;
    }
    for i in 0..list.len() {
        let Node::Kern(w, attr) = list[i] else { continue };
        let Some((l, r)) = kern_neighbours(list, i) else { continue };
        let k = if ratio > 0 {
            kern_stretch(eqtb, w, l, r)
        } else if ratio < 0 {
            kern_shrink(eqtb, w, l, r)
        } else {
            0
        };
        if k != 0 {
            list[i] = Node::ExKern { width: w, ex: k, attr };
        }
    }
}

/// luatex `hpack(p, w, cal_expand_ratio)`: measure how far the line's
/// glyphs and font kerns can stretch or shrink, derive the expansion ratio
/// from the room left after the glue, then pack again with the glyphs
/// expanded.
pub fn hpack_expand(eng: &mut crate::engine::Engine, mut list: NodeList, w: i32, kind: u8) -> PackResult {
    let adjust = eng.eqtb.int_params[crate::prim::IntParam::PdfAdjustSpacing.idx() as usize];
    let kerns = adjust <= 2;
    let (nat_w, _, _) = crate::boxes::hlist_dims(&list, &eng.eqtb);
    let (stretch, shrink) = crate::boxes::glue_sums(&list);
    let x = i64::from(w) - i64::from(nat_w);
    let mut ratio = 0;
    if x != 0 {
        let (inf, positive) = if x > 0 { (stretch[1..].iter().any(|&s| s != 0), true) } else { (shrink[1..].iter().any(|&s| s != 0), false) };
        if !inf {
            let (mut fs, mut fh) = (0i64, 0i64);
            collect(&eng.eqtb, &list, kerns, &mut fs, &mut fh);
            let room = if positive { fs } else { fh };
            if room > 0 {
                ratio = divide_scaled_n(x as f64, room as f64, 1000.0).clamp(-1000, 1000);
            }
        }
    }
    // the marginal glyph of a margin kern takes part in both passes
    for n in list.iter_mut() {
        if let Node::MarginKern { font, c, ex, .. } = n {
            let gr = GlyphRef { font: *font, c: *c };
            *ex = subst_font(&eng.eqtb, gr, 1000, *ex);
            *ex = subst_font(&eng.eqtb, gr, -1000, *ex);
        }
    }
    if ratio != 0 {
        let ctx = eng.lang_ctx();
        substitute(&eng.eqtb, &mut list, ratio, kerns, ctx);
    }
    crate::boxes::hpack(list, Some(w), kind, &eng.eqtb)
}

// ---------------------------------------------------------------------
// \efcode, \lpcode, \rpcode of Lua fonts
// ---------------------------------------------------------------------

impl crate::engine::Engine {
    /// The font code `p` (`\efcode`, `\lpcode` or `\rpcode`) of a font in
    /// LuaTeX: scans the character (`scan_char_num`) and reads its record.
    /// `None`, before anything is scanned, for other engines and
    /// primitives.
    pub(crate) fn lua_font_code(&mut self, f: FontId, p: crate::prim::Prim) -> Option<i32> {
        use crate::prim::Prim;
        if !matches!(p, Prim::EfCode | Prim::LpCode | Prim::RpCode) || !self.luatex_font_codes() {
            return None;
        }
        let c = self.scan_char_num_lua() as u32;
        Some(match p {
            Prim::EfCode => ef_code(&self.eqtb, f, c),
            Prim::LpCode => pw_code(&self.eqtb, f, c, true),
            _ => pw_code(&self.eqtb, f, c, false),
        })
    }

    /// Assign `\efcode`, `\lpcode` or `\rpcode` in LuaTeX (texfont.c
    /// `set_ef_code`: only characters the font has are changed, the value
    /// is stored as it is). Returns false, before anything is scanned, for
    /// other engines and primitives.
    pub(crate) fn lua_font_code_assign(&mut self, f: FontId, p: crate::prim::Prim) -> bool {
        use crate::prim::Prim;
        if !matches!(p, Prim::EfCode | Prim::LpCode | Prim::RpCode) || !self.luatex_font_codes() {
            return false;
        }
        let c = self.scan_char_num_lua() as u32;
        self.scan_optional_equals();
        let v = self.scan_int();
        let lua = self.eqtb.fonts.get(usize::from(f)).is_some_and(|font| font.lua.is_some());
        if lua {
            if let Some(ci) = self.lua_font_mut(f).and_then(|lf| lf.chars.get_mut(&c)) {
                match p {
                    Prim::EfCode => ci.expansion_factor = v,
                    Prim::LpCode => ci.left_protruding = v,
                    _ => ci.right_protruding = v,
                }
            }
        } else if let Some(c) = u8::try_from(c).ok().filter(|&c| {
            self.eqtb.fonts.get(usize::from(f)).is_some_and(|font| font.char_present(c))
        }) {
            if usize::from(f) >= self.eqtb.expand.len() {
                self.eqtb.expand.resize_with(usize::from(f) + 1, Default::default);
            }
            let x = &mut self.eqtb.expand[usize::from(f)];
            let (table, default) = match p {
                Prim::EfCode => (&mut x.ef, 1000),
                Prim::LpCode => (&mut x.lp, 0),
                _ => (&mut x.rp, 0),
            };
            table.get_or_insert_with(|| std::rc::Rc::new(std::cell::RefCell::new([default; 256]))).borrow_mut()[usize::from(c)] = v;
        }
        true
    }

    /// Whether the font codes follow luatex (a character record of the font
    /// holds them) rather than pdfTeX.
    fn luatex_font_codes(&self) -> bool {
        self.engine_kind == crate::engine::EngineKind::LuaTeX
    }
}

impl crate::engine::Engine {
    /// texfont.c `read_expand_font` (`\expandglyphsinfont`) for a Lua font:
    /// the limits are kept with the font, no expanded copies are made.
    pub(crate) fn lua_read_expand_font(&mut self, f: FontId) {
        if f == 0 {
            self.lua_res_error(None, "font expansion", "invalid font identifier");
            return;
        }
        self.scan_optional_equals();
        let mut stretch = self.scan_int().clamp(0, 1000);
        let mut shrink = self.scan_int().clamp(0, 500);
        let step = self.scan_int().clamp(0, 100);
        if step == 0 {
            self.lua_res_error(None, "font expansion", "invalid step");
            return;
        }
        stretch -= stretch % step;
        shrink -= shrink % step;
        if stretch == 0 && shrink == 0 {
            self.lua_res_error(None, "font expansion", "invalid limit(s)");
            return;
        }
        if self.scan_keyword(b"autoexpand") {
            self.lua_warning("font expansion", "autoexpand not supported");
            self.scan_optional_space();
        }
        let (cur_step, cur_stretch, cur_shrink) = limits(&self.eqtb, f);
        if cur_step != 0 {
            if cur_step != step {
                self.lua_res_error(None, "font expansion", "font has been expanded with different expansion step");
            } else if (cur_stretch == 0 && stretch != 0) || (cur_stretch > 0 && cur_stretch != stretch) {
                self.lua_res_error(None, "font expansion", "font has been expanded with different stretch limit");
            } else if (cur_shrink == 0 && shrink != 0) || (cur_shrink > 0 && cur_shrink != shrink) {
                self.lua_res_error(None, "font expansion", "font has been expanded with different shrink limit");
            }
        } else {
            let used = self.lua_fonts.used.contains(&f) || self.pdf_doc.font_chars.contains_key(&usize::from(f));
            if used {
                self.lua_warning("font expansion", "font should be expanded before its first use");
            }
            self.lua_set_expansion(f, stretch, shrink, step);
        }
    }
}

impl crate::engine::Engine {
    /// textoken.c `left_margin_kern_code` / `right_margin_kern_code`: the
    /// width of the margin kern at the end of the hbox in register `n`.
    pub(crate) fn lua_margin_kern_width(&mut self, n: u16, left: bool) -> Option<i32> {
        let Some(Some(Node::Box { kind, list, .. })) = self.eqtb.boxed.get(usize::from(n)) else {
            self.error("marginkern: a non-empty hbox expected");
            return None;
        };
        if *kind != crate::boxes::HBOX {
            self.error("marginkern: a non-empty hbox expected");
            return None;
        }
        let side = |n: &Node| match n {
            Node::MarginKern { width, side, .. } if (*side == 0) == left => Some(*width),
            _ => None,
        };
        if left {
            list.iter().find(|n| !matches!(n, Node::Glue(..))).and_then(side)
        } else {
            let mut rest = list.iter().rev().skip_while(|n| matches!(n, Node::Glue(..)));
            let p = rest.next()?;
            match p {
                Node::Disc(_) => rest.next().and_then(side),
                p => side(p),
            }
        }
    }
}
