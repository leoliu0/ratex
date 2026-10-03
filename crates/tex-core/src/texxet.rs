//! e-TeX TeXXeT (etex.ch "mixed direction typesetting"): the LR stack
//! logic shared by hpack, post_line_break, init_math and after_math.
//! ship_out's reversal lives with the PDF renderer (`pdfrender::lr`).
//!
//! Text-direction nodes are math nodes (`Node::MathKern` kinds, see
//! `boxes::MATH_ON`); an LR stack holds the end-node kinds still open,
//! bottom first, so an empty stack is etex.ch's `before` sentinel.

use crate::boxes::{
    math_end_lr, math_end_lr_type, math_lr_dir, Glue, Node, NodeList, BEGIN_L, BEGIN_M,
    BOX_LR_DLIST, END_L, END_M, HBOX, LR_KIND_MIN,
};
use crate::engine::Engine;
use crate::prim::{GlueParam, IntParam};

const MAX_DIMEN: i64 = 0x3FFF_FFFF;

/// etex.ch hpack: "Adjust the LR stack for the hpack routine" for every
/// math node, then "Check for LR anomalies at the end of hpack". An
/// unmatched end node becomes an explicit kern of its width; every node
/// still open gets its end node appended. Returns `LR_problems`
/// (10000 × missing + extra).
pub(crate) fn hpack_lr_check(list: &mut NodeList) -> i32 {
    let mut stack: Vec<u8> = Vec::new();
    let mut problems = 0;
    for n in list.iter_mut() {
        let Node::MathKern(w, kind, attr) = *n else {
            continue;
        };
        if kind == 0 {
            continue;
        }
        if math_end_lr(kind) {
            if stack.last() == Some(&math_end_lr_type(kind)) {
                stack.pop();
            } else {
                problems += 1;
                *n = Node::ExplicitKern(w, attr);
            }
        } else {
            stack.push(math_end_lr_type(kind));
        }
    }
    while let Some(kind) = stack.pop() {
        list.push(Node::MathKern(0, kind, crate::boxes::Attr::NONE));
        problems += 10000;
    }
    problems
}

/// etex.ch "Adjust the LR stack for the post_line_break routine": an end
/// node pops its own entry (and is ignored otherwise), a begin node pushes.
pub(crate) fn lr_adjust(stack: &mut Vec<u8>, kind: u8) {
    if math_end_lr(kind) {
        if stack.last() == Some(&math_end_lr_type(kind)) {
            stack.pop();
        }
    } else {
        stack.push(math_end_lr_type(kind));
    }
}

/// One item of init_math's working copy of the last line: a node or an
/// etex.ch edge node (direction, width).
enum WItem<'a> {
    Ref(&'a Node),
    Own(Node),
    Edge(u8, i32),
}

impl WItem<'_> {
    fn node(&self) -> Option<&Node> {
        match self {
            WItem::Ref(n) => Some(n),
            WItem::Own(n) => Some(n),
            WItem::Edge(..) => None,
        }
    }

    /// width and kind of a math node
    fn math(&self) -> Option<(i32, u8)> {
        match self.node() {
            Some(&Node::MathKern(w, k, _)) if k != 0 => Some((w, k)),
            _ => None,
        }
    }
}

impl Engine {
    /// etex.ch "Report LR problems": `\endL or \endR problem (m missing,
    /// e extra)`, reported like hpack's other box diagnostics.
    pub(crate) fn report_lr_problems(
        &mut self,
        problems: i32,
        source: Option<crate::input::SourceContext>,
    ) {
        let msg = format!(
            "\\endL or \\endR problem ({} missing, {} extra)",
            problems / 10000,
            problems % 10000
        );
        self.pack_warning_at(&msg, source);
    }

    /// `LR_save` of the vertical level whose paragraph runs at nest depth
    /// `key` (`saved_lists.len()` inside the paragraph).
    pub(crate) fn lr_save_peek(&self, key: usize) -> Option<&[u8]> {
        self.lr_save
            .last()
            .filter(|(k, _)| *k == key)
            .map(|(_, s)| s.as_slice())
    }

    pub(crate) fn lr_save_take(&mut self, key: usize) -> Vec<u8> {
        // entries of deeper levels belong to nests that are gone
        self.lr_save.retain(|(k, _)| *k <= key);
        match self.lr_save.last() {
            Some((k, _)) if *k == key => self.lr_save.pop().map(|(_, s)| s).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    pub(crate) fn lr_save_store(&mut self, key: usize, stack: Vec<u8>) {
        self.lr_save.retain(|(k, _)| *k < key);
        if !stack.is_empty() {
            self.lr_save.push((key, stack));
        }
    }

    /// etex.ch "Set the value of x to the text direction before the
    /// display" for the paragraph at nest depth `key`.
    pub(crate) fn display_direction_before(&self, key: usize) -> i32 {
        match self.lr_save_peek(key).and_then(|s| s.last()) {
            None => 0,
            Some(&top) if math_lr_dir(top) == 1 => -1,
            Some(_) => 1,
        }
    }

    /// etex.ch "Let j be the prototype box for the display": the final
    /// line's width, shift and glue setting around the current
    /// \leftskip/\rightskip (kerns when those are zero).
    pub(crate) fn display_prototype_box(&self, just_box: &Node) -> Option<Node> {
        let Node::Box {
            w,
            shift,
            glue_sign,
            glue_order,
            glue_set,
            ..
        } = just_box
        else {
            return None;
        };
        let skip = |p: GlueParam| -> Node {
            let g = self.eqtb.glue_params[p.idx() as usize];
            if is_zero_glue(&g) {
                Node::Kern(0, self.eqtb.cur_attr)
            } else {
                Node::Glue(g, self.eqtb.cur_attr)
            }
        };
        Some(Node::Box {
            kind: HBOX,
            w: *w,
            h: 0,
            d: 0,
            shift: *shift,
            list: vec![skip(GlueParam::LeftSkip), skip(GlueParam::RightSkip)],
            glue_sign: *glue_sign,
            glue_order: *glue_order,
            glue_set: *glue_set,
            lr: 0,
            dir: 0, attr: self.eqtb.cur_attr, subtype: 0,
        })
    }

    /// etex.ch init_math "Calculate the natural width, w" of the final line
    /// `just_box` with the text direction `x` before the display: R-text
    /// lines are measured mirrored, reflected segments are reversed
    /// (`just_reverse`), and LR anomalies void the size (max_dimen).
    pub(crate) fn display_line_size(&self, just_box: &Node, x: i32, gap: i64) -> i64 {
        let Node::Box {
            w: box_w,
            list,
            shift,
            glue_sign,
            glue_order,
            ..
        } = just_box
        else {
            return -MAX_DIMEN;
        };
        let texxet = self.eqtb.int_params[IntParam::TeXXeTEnabled.idx() as usize] > 0;
        let mut v = *shift as i64;
        let mut cur_dir = 0u8;
        let mut items: Vec<WItem> = if x >= 0 {
            list.iter().map(WItem::Ref).collect()
        } else {
            v = -v - *box_w as i64;
            cur_dir = 1;
            let mut l = Vec::with_capacity(list.len() + 2);
            l.push(WItem::Own(Node::MathKern(0, BEGIN_L, crate::boxes::Attr::NONE)));
            l.extend(list.iter().filter(|n| just_copied(n)).map(WItem::Ref));
            l.push(WItem::Own(Node::MathKern(0, END_L, crate::boxes::Attr::NONE)));
            l
        };
        v += gap;
        let mut stack: Vec<u8> = Vec::new();
        let mut lr_problems = 0;
        let mut w = -MAX_DIMEN;
        let active = |g: &Glue| {
            (*glue_sign == 1 && g.stretch_order == *glue_order && g.stretch != 0)
                || (*glue_sign == 2 && g.shrink_order == *glue_order && g.shrink != 0)
        };
        let mut i = 0;
        'scan: while i < items.len() {
            let math = items[i].math();
            let (d, visible) = if let Some((width, kind)) = math {
                if texxet {
                    // "Adjust the LR stack for the init_math routine"
                    if math_end_lr(kind) {
                        if stack.last() == Some(&math_end_lr_type(kind)) {
                            stack.pop();
                        } else if kind >= LR_KIND_MIN {
                            w = MAX_DIMEN;
                            break 'scan;
                        }
                    } else {
                        stack.push(math_end_lr_type(kind));
                        if math_lr_dir(kind) != cur_dir {
                            // just_reverse, then continue at temp_head
                            let rest = items.split_off(i + 1);
                            let (rev, dir) =
                                just_reverse(rest, cur_dir, &mut stack, &mut lr_problems);
                            cur_dir = dir;
                            if v < MAX_DIMEN {
                                v += width as i64;
                            }
                            items = rev;
                            i = 0;
                            continue 'scan;
                        }
                    }
                } else if kind >= LR_KIND_MIN {
                    w = MAX_DIMEN;
                    break 'scan;
                }
                (width as i64, false)
            } else {
                match items[i].node() {
                    None => match items[i] {
                        WItem::Edge(dir, width) => {
                            cur_dir = dir;
                            (width as i64, false)
                        }
                        _ => (0, false),
                    },
                    Some(n) => match n {
                        Node::Char { c, font, .. } => (
                            self.eqtb
                                .fonts
                                .get(*font as usize)
                                .map_or(0, |f| f.char_width(*c) as i64),
                            true,
                        ),
                        Node::LuaGlyph(g) => (crate::boxes::lua_glyph_dims(&self.eqtb, g).0 as i64, true),
                        Node::Ligature { lig_width, .. } => (*lig_width as i64, true),
                        Node::Box { w, .. } => (*w as i64, true),
                        // xetex.web §1146: native_word/glyph whatsits are visible
                        Node::NativeGlyphRun { width, .. } => (*width as i64, true),
                        Node::Rule { width, .. } => (*width as i64, true),
                        Node::Kern(k, _) | Node::ExplicitKern(k, _) | Node::AccentKern(k, _) | Node::ItalicKern(k, _) | Node::SpaceAdjKern(k, _) => {
                            (*k as i64, false)
                        }
                        Node::MarginKern { width, .. } => (*width as i64, false),
                        Node::ExKern { width, ex, .. } => ((*width + *ex) as i64, false),
                        Node::Whatsit(crate::boxes::WhatIt::PdfRefXImage { w, .. }, _)
                        | Node::Whatsit(crate::boxes::WhatIt::PdfRefXForm { w, .. }, _) => {
                            (*w as i64, false)
                        }
                        // xetex.web: pic_node/pdf_node are visible whatsits
                        Node::Whatsit(crate::boxes::WhatIt::XePic { w, .. }, _) => (*w as i64, true),
                        Node::Glue(g, _) => {
                            if active(g) {
                                v = MAX_DIMEN;
                            }
                            (g.width as i64, false)
                        }
                        Node::Leaders { glue, .. } => {
                            if active(glue) {
                                v = MAX_DIMEN;
                            }
                            (glue.width as i64, true)
                        }
                        _ => (0, false),
                    },
                }
            };
            if visible {
                if v < MAX_DIMEN {
                    v += d;
                    w = v;
                } else {
                    w = MAX_DIMEN;
                    break;
                }
            } else if v < MAX_DIMEN {
                v += d;
            }
            i += 1;
        }
        // "Finish the natural width computation"
        if texxet && lr_problems != 0 {
            w = MAX_DIMEN;
        }
        w
    }

    /// etex.ch app_display(j, b, d): place the display box `b` at
    /// displacement `d` within the display line (`z` = \displaywidth,
    /// `s` = \displayindent, `x` = \predisplaydirection). Without a text
    /// direction this is TeX's `shift_amount(b):=s+d`; otherwise the line
    /// is built from the prototype box `j` (or kerns) and bracketed by
    /// \beginM/\endM, mirrored for right-to-left text.
    pub(crate) fn app_display(
        &mut self,
        j: Option<&Node>,
        mut b: Node,
        mut d: i64,
        z: i64,
        s: i64,
        x: i32,
    ) -> Node {
        if x == 0 {
            if let Node::Box { shift, .. } = &mut b {
                *shift = (s + d) as i32;
            }
            return b;
        }
        self.texxet_nodes = true;
        let (pw, ph, pd) = match &b {
            Node::Box { w, h, d, .. } => (*w as i64, *h, *d),
            _ => (0, 0, 0),
        };
        // "Set up the hlist for the display line"
        let mut e;
        if x > 0 {
            e = z - d - pw;
        } else {
            e = d;
            d = z - e - pw;
        }
        let mut s = s;
        let mut line = j.cloned();
        if let Some(Node::Box {
            h, d: bd, shift, w, ..
        }) = &mut line
        {
            *h = ph;
            *bd = pd;
            s -= *shift as i64;
            d += s;
            e += *w as i64 - z - s;
        }
        let mut hlist: NodeList = match b {
            Node::Box { lr, .. } if lr == BOX_LR_DLIST => vec![b],
            Node::Box { list, .. } => {
                let mut list = list;
                if x < 0 {
                    list.reverse();
                }
                list
            }
            other => vec![other],
        };
        // "Package the display line"
        let (left, right) = match &mut line {
            Some(Node::Box { list, .. }) if list.len() == 2 => {
                let r = list.pop().unwrap();
                (list.pop().unwrap(), r)
            }
            _ => (Node::Kern(0, self.eqtb.cur_attr), Node::Kern(0, self.eqtb.cur_attr)),
        };
        let mut out: NodeList = Vec::with_capacity(hlist.len() + 6);
        match left {
            Node::Glue(g, _) => {
                out.push(Node::Glue(g, self.eqtb.cur_attr));
                out.push(Node::MathKern(0, BEGIN_M, self.eqtb.cur_attr));
                out.push(Node::Glue(cancel_glue(&g, d), self.eqtb.cur_attr));
            }
            _ => {
                out.push(Node::MathKern(0, BEGIN_M, self.eqtb.cur_attr));
                out.push(Node::Kern(d as i32, self.eqtb.cur_attr));
            }
        }
        out.append(&mut hlist);
        match right {
            Node::Glue(g, _) => {
                out.push(Node::Glue(cancel_glue(&g, e), self.eqtb.cur_attr));
                out.push(Node::MathKern(0, END_M, self.eqtb.cur_attr));
                out.push(Node::Glue(g, self.eqtb.cur_attr));
            }
            _ => {
                out.push(Node::Kern(e as i32, self.eqtb.cur_attr));
                out.push(Node::MathKern(0, END_M, self.eqtb.cur_attr));
            }
        }
        match line {
            Some(Node::Box {
                kind,
                w,
                h,
                d,
                shift,
                glue_sign,
                glue_order,
                glue_set,
                lr,
                dir,
                attr,
                ..
            }) => Node::Box {
                kind,
                w,
                h,
                d,
                shift,
                list: out,
                glue_sign,
                glue_order,
                glue_set,
                lr,
                dir,
                attr, subtype: 0,
            },
            _ => {
                let mut packed = crate::boxes::hpack(out, None, HBOX, &self.eqtb).node;
                if let Node::Box { shift, .. } = &mut packed {
                    *shift = s as i32;
                }
                packed
            }
        }
    }
}

/// etex.ch `cancel_glue`: a glue that, together with `g`, amounts to a
/// kern of `amount`.
fn cancel_glue(g: &Glue, amount: i64) -> Glue {
    Glue::spec(
        (amount - g.width as i64) as i32,
        -g.stretch,
        g.stretch_order,
        -g.shrink,
        g.shrink_order,
    )
}

fn is_zero_glue(g: &Glue) -> bool {
    g.width == 0 && g.stretch == 0 && g.shrink == 0
}

/// etex.ch `just_copy` keeps only the nodes init_math measures.
fn just_copied(n: &Node) -> bool {
    matches!(
        n,
        Node::Char { .. }
            | Node::LuaGlyph(_)
            | Node::Ligature { .. }
            | Node::NativeGlyphRun { .. }
            | Node::Box { .. }
            | Node::Rule { .. }
            | Node::Kern(_, _)
            | Node::ExKern { .. }
            | Node::ExplicitKern(_, _)
            | Node::AccentKern(_, _) | Node::ItalicKern(_, _) | Node::SpaceAdjKern(_, _)
            | Node::MathKern(..)
            | Node::Glue(_, _)
            | Node::Leaders { .. }
            | Node::Whatsit(_, _)
    )
}

/// etex.ch `just_reverse`: reverse the line remainder `rest` up to the end
/// node closing the segment just opened (whose end kind is on top of
/// `stack`), in direction `reflected`; the returned list ends with an edge
/// restoring `cur_dir`, followed by the unreversed remainder. Returns the
/// new list and the direction now current (the reflected one).
fn just_reverse<'a>(
    rest: Vec<WItem<'a>>,
    cur_dir: u8,
    stack: &mut Vec<u8>,
    lr_problems: &mut i32,
) -> (Vec<WItem<'a>>, u8) {
    let reflected = 1 - cur_dir;
    let mut edge_width = 0;
    let (mut m, mut n) = (0u32, 0u32);
    let mut consumed: Vec<WItem<'a>> = Vec::with_capacity(rest.len() + 1);
    let mut iter = rest.into_iter();
    let mut tail: Vec<WItem<'a>> = Vec::new();
    while let Some(item) = iter.next() {
        let math = item.math();
        let Some((width, kind)) = math else {
            consumed.push(item);
            continue;
        };
        // "Adjust the LR stack for the just_reverse routine"
        let node = if math_end_lr(kind) {
            if stack.last() != Some(&math_end_lr_type(kind)) {
                *lr_problems += 1;
                Node::Kern(width, crate::boxes::Attr::NONE)
            } else {
                stack.pop();
                if n > 0 {
                    n -= 1;
                    Node::MathKern(width, kind - 1, crate::boxes::Attr::NONE)
                } else if m > 0 {
                    m -= 1;
                    Node::Kern(width, crate::boxes::Attr::NONE)
                } else {
                    // found: the segment's own end node
                    edge_width = width;
                    tail.extend(iter);
                    break;
                }
            }
        } else {
            stack.push(math_end_lr_type(kind));
            if n > 0 || math_lr_dir(kind) != reflected {
                n += 1;
                Node::MathKern(width, kind + 1, crate::boxes::Attr::NONE)
            } else {
                m += 1;
                Node::Kern(width, crate::boxes::Attr::NONE)
            }
        };
        consumed.push(WItem::Own(node));
    }
    consumed.reverse();
    consumed.push(WItem::Edge(cur_dir, edge_width));
    consumed.extend(tail);
    (consumed, reflected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boxes::{BEGIN_R, END_R};

    #[test]
    fn hpack_check_closes_open_segments_and_neutralizes_stray_ends() {
        let mut list = vec![
            Node::MathKern(0, END_L, crate::boxes::Attr::NONE),
            Node::MathKern(0, BEGIN_R, crate::boxes::Attr::NONE),
            Node::MathKern(0, BEGIN_L, crate::boxes::Attr::NONE),
        ];
        assert_eq!(hpack_lr_check(&mut list), 20001);
        assert!(matches!(list[0], Node::ExplicitKern(0, _)));
        // innermost first: \endL then \endR
        assert!(matches!(list[3], Node::MathKern(0, END_L, _)));
        assert!(matches!(list[4], Node::MathKern(0, END_R, _)));
    }
}
