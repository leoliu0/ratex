//! LuaTeX's text passes over node lists: hyphenation (`texlang.c`
//! `hnj_hyphenation`), ligaturing and kerning (`luafont.c`
//! `handle_ligaturing` / `handle_kerning` / `new_ligkern`). LuaTeX's main
//! control only appends glyph nodes; these passes run when a paragraph is
//! broken into lines and when an hbox is packed, and they are what
//! `node.hyphenate`, `node.ligaturing` and `node.kerning` run. The passes
//! work on the node store: lists have a `temp` head node, as in LuaTeX.
//!
//! Glyphs of TFM fonts (the engine's `Node::Char`) are left alone: the main
//! loop of the engine has already built their ligatures and kerns.

use std::rc::Rc;

use crate::boxes::NodeList;
use crate::engine::{Engine, EngineKind};
use crate::lua_callbacks::{Cb, CbArg};
use crate::lua_font::{LuaFont, LuaLig};
use crate::lua_node::*;
use crate::lua_node_conv::sl;
use crate::prim::IntParam;

/// luatex `MAX_WORD_LEN`.
const MAX_WORD_LEN: usize = 64;

/// luatex `automatic_disc` etc.
const DISC_AUTOMATIC: u16 = 2;
const DISC_SYLLABLE: u16 = 3;
const DISC_INIT: u16 = 4;
const DISC_SELECT: u16 = 5;

impl Engine {
    // ----------------------------------------------------------- accessors

    #[inline]
    fn lk_glyph(&self, n: u32) -> bool {
        n != 0 && self.lua_nodes.id(n) == GLYPH
    }

    #[inline]
    fn lk_ch(&self, n: u32) -> i32 {
        self.lua_nodes.node(n).f[sl::C_CHAR]
    }

    #[inline]
    fn lk_fnt(&self, n: u32) -> i32 {
        self.lua_nodes.node(n).f[sl::C_FONT]
    }

    #[inline]
    fn lk_sub(&self, n: u32) -> u16 {
        self.lua_nodes.node(n).subtype
    }

    #[inline]
    fn lk_ghost(&self, n: u32) -> bool {
        self.lk_sub(n) & GLYPH_GHOST != 0
    }

    #[inline]
    fn lk_right_ghost(&self, n: u32) -> bool {
        self.lk_ghost(n) && self.lk_sub(n) & GLYPH_RIGHT != 0
    }

    /// `is_simple_character`: a plain character glyph of a Lua font.
    fn lk_simple(&self, n: u32) -> bool {
        self.lk_glyph(n)
            && self.lk_sub(n) & GLYPH_CHARACTER != 0
            && self.lk_sub(n) & (GLYPH_LIGATURE | GLYPH_GHOST) == 0
            && self.lk_font(self.lk_fnt(n)).is_some()
    }

    fn lk_font(&self, f: i32) -> Option<Rc<LuaFont>> {
        usize::try_from(f).ok().and_then(|f| self.eqtb.fonts.get(f)).and_then(|f| f.lua.clone())
    }

    fn lk_lang(&self, n: u32) -> i32 {
        self.lua_nodes.node(n).f[sl::C_LANG]
    }

    /// The attribute list of the current `\attribute` registers.
    fn lk_current_attr(&mut self) -> u32 {
        let regs = self.lua_attribute_registers();
        self.lua_nodes.current_attr_list(&regs)
    }

    /// `raw_glyph_node` plus the current attributes (what `new_glyph` does):
    /// no language data.
    fn lk_new_glyph(&mut self, font: i32, ch: i32) -> u32 {
        let attr = self.lk_current_attr();
        let n = self.lua_nodes.new_node(GLYPH, 0, attr);
        let f = &mut self.lua_nodes.node_mut(n).f;
        f[sl::C_CHAR] = ch;
        f[sl::C_FONT] = font;
        f[sl::C_LANG] = 0;
        f[sl::C_LEFT] = 0;
        f[sl::C_RIGHT] = 0;
        f[sl::C_UCHYPH] = 0;
        n
    }

    fn lk_uncouple(&mut self, n: u32) {
        let nd = self.lua_nodes.node_mut(n);
        nd.next = 0;
        nd.prev = 0;
    }

    /// `try_couple_nodes`: `b` may be null.
    fn lk_try_couple(&mut self, a: u32, b: u32) {
        if a == 0 {
            return;
        }
        if b == 0 {
            self.lua_nodes.node_mut(a).next = 0;
        } else {
            self.lua_nodes.couple(a, b);
        }
    }

    fn lk_copy_attr(&mut self, from: u32, to: u32) {
        let attr = self.lua_nodes.node(from).attr;
        self.lua_nodes.reassign_attr(to, attr);
    }

    // ----------------------------------------------------------- ligatures

    fn lk_test_ligature(&self, left: u32, right: u32) -> Option<LuaLig> {
        if !self.lk_glyph(left) || self.lk_fnt(left) != self.lk_fnt(right) {
            return None;
        }
        if self.lk_ghost(left) || self.lk_ghost(right) {
            return None;
        }
        self.lk_font(self.lk_fnt(left))?.lig(self.lk_ch(left), self.lk_ch(right))
    }

    /// `try_ligature`: `*frst` is the left glyph, `fwd` the glyph after it.
    fn lk_try_ligature(&mut self, frst: &mut u32, fwd: u32) -> bool {
        let mut cur = *frst;
        let Some(lig) = self.lk_test_ligature(cur, fwd) else {
            return false;
        };
        let mut move_after = (lig.op & 0x0C) >> 2;
        let keep_right = lig.op & 0x01 != 0;
        let keep_left = lig.op & 0x02 != 0;
        let newgl = self.lk_new_glyph(self.lk_fnt(cur), lig.replacement as i32);
        let mut sub = GLYPH_LIGATURE;
        if self.lk_ch(cur) < 0 {
            sub |= GLYPH_LEFT;
        }
        if self.lk_ch(fwd) < 0 {
            sub |= GLYPH_RIGHT;
        }
        self.lua_nodes.node_mut(newgl).subtype = sub;
        if self.lk_ch(cur) < 0 {
            if self.lk_ch(fwd) >= 0 {
                self.lk_copy_attr(fwd, newgl);
            }
        } else {
            self.lk_copy_attr(cur, newgl);
        }
        // left side
        if keep_left {
            let new_first = self.lua_nodes.copy_node(cur);
            self.lua_nodes.node_mut(newgl).f[sl::C_COMP] = new_first as i32;
            self.lua_nodes.couple(cur, newgl);
            if move_after > 0 {
                move_after -= 1;
                cur = newgl;
            }
        } else {
            let prev = self.lua_nodes.prev(cur);
            self.lk_uncouple(cur);
            self.lua_nodes.node_mut(newgl).f[sl::C_COMP] = cur as i32;
            self.lua_nodes.couple(prev, newgl);
            cur = newgl;
        }
        // right side
        let comp = self.lua_nodes.node(newgl).f[sl::C_COMP] as u32;
        if keep_right {
            let new_second = self.lua_nodes.copy_node(fwd);
            self.lua_nodes.couple(comp, new_second);
            self.lua_nodes.couple(newgl, fwd);
            if move_after > 0 {
                cur = fwd;
            }
        } else {
            let next = self.lua_nodes.next(fwd);
            self.lk_uncouple(fwd);
            self.lua_nodes.couple(comp, fwd);
            if next != 0 {
                self.lua_nodes.couple(newgl, next);
            }
        }
        *frst = cur;
        true
    }

    fn lk_nest_append(&mut self, head: u32, n: u32) {
        let t = self.lua_nodes.tail_of(head);
        self.lua_nodes.couple(t, n);
    }

    fn lk_nest_prepend(&mut self, head: u32, n: u32) {
        let first = self.lua_nodes.next(head);
        self.lua_nodes.couple(head, n);
        if first != 0 {
            self.lua_nodes.couple(n, first);
        }
    }

    fn lk_nest_prepend_list(&mut self, head: u32, list: u32) {
        let first = self.lua_nodes.next(head);
        self.lua_nodes.couple(head, list);
        if first != 0 {
            let tail = self.lua_nodes.tail_of(list);
            self.lua_nodes.couple(tail, first);
        }
    }

    fn lk_handle_lig_nest(&mut self, root: u32, mut cur: u32) {
        if cur == 0 {
            return;
        }
        let _ = root;
        while self.lua_nodes.next(cur) != 0 {
            let fwd = self.lua_nodes.next(cur);
            if self.lk_glyph(cur) && self.lk_glyph(fwd) && self.lk_fnt(cur) == self.lk_fnt(fwd) && self.lk_try_ligature(&mut cur, fwd)
            {
                continue;
            }
            cur = self.lua_nodes.next(cur);
        }
    }

    /// The nest heads of the three lists of discretionary `d` (created by
    /// [`Self::lk_open_discs`]).
    fn lk_pre(&self, d: u32) -> u32 {
        self.lua_nodes.node(d).f[sl::D_PRE] as u32
    }

    fn lk_post(&self, d: u32) -> u32 {
        self.lua_nodes.node(d).f[sl::D_POST] as u32
    }

    fn lk_rep(&self, d: u32) -> u32 {
        self.lua_nodes.node(d).f[sl::D_REPLACE] as u32
    }

    fn lk_first(&self, nest: u32) -> u32 {
        self.lua_nodes.next(nest)
    }

    /// Give the sub lists of the discretionaries of the top level list at
    /// `head` a `temp` head node (LuaTeX's nest heads) so that the passes
    /// below can edit their first nodes in place.
    fn lk_open_discs(&mut self, head: u32) {
        let mut p = self.lua_nodes.next(head);
        while p != 0 {
            if self.lua_nodes.id(p) == DISC {
                for slot in [sl::D_PRE, sl::D_POST, sl::D_REPLACE] {
                    let first = self.lua_nodes.node(p).f[slot] as u32;
                    let nest = self.lua_nodes.new_node(TEMP, 0, 0);
                    if first != 0 {
                        self.lua_nodes.couple(nest, first);
                    }
                    self.lua_nodes.node_mut(p).f[slot] = nest as i32;
                }
            }
            p = self.lua_nodes.next(p);
        }
    }

    fn lk_close_discs(&mut self, head: u32) {
        let mut p = self.lua_nodes.next(head);
        while p != 0 {
            if self.lua_nodes.id(p) == DISC {
                for slot in [sl::D_PRE, sl::D_POST, sl::D_REPLACE] {
                    let nest = self.lua_nodes.node(p).f[slot] as u32;
                    if nest == 0 {
                        continue;
                    }
                    let first = self.lua_nodes.next(nest);
                    if first != 0 {
                        self.lua_nodes.node_mut(first).prev = 0;
                    }
                    self.lua_nodes.node_mut(nest).next = 0;
                    self.lua_nodes.flush_node(nest);
                    self.lua_nodes.node_mut(p).f[slot] = first as i32;
                }
            }
            p = self.lua_nodes.next(p);
        }
    }

    /// `handle_lig_word`: ligature the word starting at `cur`; returns the
    /// last node it looked at.
    fn lk_handle_lig_word(&mut self, mut cur: u32) -> u32 {
        let mut right = 0u32;
        let mut last = 0u32;
        if self.lua_nodes.id(cur) == BOUNDARY {
            let prev = self.lua_nodes.prev(cur);
            let fwd = self.lua_nodes.next(cur);
            self.lua_nodes.flush_node(cur);
            if fwd == 0 {
                self.lua_nodes.node_mut(prev).next = 0;
                return prev;
            }
            self.lua_nodes.couple(prev, fwd);
            if !self.lk_glyph(fwd) {
                return prev;
            }
            cur = fwd;
        } else if self.lk_font(self.lk_fnt(cur)).is_some_and(|f| f.has_left_boundary()) {
            let prev = self.lua_nodes.prev(cur);
            let p = self.lk_new_glyph(self.lk_fnt(cur), crate::lua_font::LEFT_BOUNDARY);
            self.lua_nodes.couple(prev, p);
            self.lua_nodes.couple(p, cur);
            cur = p;
        }
        if self.lk_font(self.lk_fnt(cur)).is_some_and(|f| f.has_right_boundary()) {
            right = self.lk_new_glyph(self.lk_fnt(cur), crate::lua_font::RIGHT_BOUNDARY);
        }
        loop {
            if self.lk_glyph(cur) {
                let fwd = self.lua_nodes.next(cur);
                if fwd == 0 {
                    // the last character of a paragraph
                    if right == 0 {
                        break;
                    }
                    self.lua_nodes.couple(cur, right);
                    right = 0;
                    continue;
                }
                if self.lk_glyph(fwd) {
                    if self.lk_fnt(cur) != self.lk_fnt(fwd) {
                        break;
                    }
                    if self.lk_try_ligature(&mut cur, fwd) {
                        continue;
                    }
                } else if self.lua_nodes.id(fwd) == DISC {
                    let pre = self.lk_first(self.lk_pre(fwd));
                    let nob = self.lk_first(self.lk_rep(fwd));
                    // a{b?}{?}{?} and a+b=>B: {B?}{?}{a?}; a{?}{?}{b?}: {a?}{?}{B?}
                    if (pre != 0 && self.lk_glyph(pre) && self.lk_test_ligature(cur, pre).is_some())
                        || (nob != 0 && self.lk_glyph(nob) && self.lk_test_ligature(cur, nob).is_some())
                    {
                        let prev = self.lua_nodes.prev(cur);
                        self.lk_uncouple(cur);
                        self.lua_nodes.couple(prev, fwd);
                        let rep = self.lk_rep(fwd);
                        self.lk_nest_prepend(rep, cur);
                        let copy = self.lua_nodes.copy_node(cur);
                        let pre_nest = self.lk_pre(fwd);
                        self.lk_nest_prepend(pre_nest, copy);
                        cur = prev;
                    }
                    // a{?}{?}{}b and a+b=>B: {a?}{?b}{B}
                    let next = self.lua_nodes.next(fwd);
                    if nob == 0
                        && next != 0
                        && self.lk_glyph(next)
                        && self.lk_glyph(cur)
                        && self.lk_test_ligature(cur, next).is_some()
                    {
                        let prev = self.lua_nodes.prev(cur);
                        self.lk_uncouple(cur);
                        self.lua_nodes.couple(prev, fwd);
                        let rep = self.lk_rep(fwd);
                        self.lua_nodes.couple(rep, cur);
                        let copy = self.lua_nodes.copy_node(cur);
                        let pre_nest = self.lk_pre(fwd);
                        self.lk_nest_prepend(pre_nest, copy);
                        let tail = self.lua_nodes.next(next);
                        self.lk_uncouple(next);
                        self.lk_try_couple(fwd, tail);
                        self.lua_nodes.couple(cur, next);
                        let post_nest = self.lk_post(fwd);
                        let copy = self.lua_nodes.copy_node(next);
                        self.lk_nest_append(post_nest, copy);
                        cur = prev;
                    }
                    let pre_nest = self.lk_pre(fwd);
                    let first = self.lk_first(pre_nest);
                    self.lk_handle_lig_nest(pre_nest, first);
                } else if self.lua_nodes.id(fwd) == BOUNDARY {
                    let next = self.lua_nodes.next(fwd);
                    self.lk_try_couple(cur, next);
                    self.lua_nodes.flush_node(fwd);
                    if right != 0 {
                        self.lua_nodes.flush_node(right);
                    }
                    break;
                } else {
                    // something unknown
                    if right == 0 {
                        break;
                    }
                    self.lua_nodes.couple(cur, right);
                    self.lua_nodes.couple(right, fwd);
                    right = 0;
                    continue;
                }
            } else if self.lua_nodes.id(cur) == DISC {
                if self.lk_first(self.lk_rep(cur)) != 0 || self.lk_first(self.lk_post(cur)) != 0 {
                    let mut prev = 0u32;
                    if self.lk_sub(cur) == DISC_SELECT {
                        prev = self.lua_nodes.prev(cur);
                        if self.lk_first(self.lk_post(cur)) != 0 {
                            let n = self.lk_post(prev);
                            let first = self.lk_first(self.lk_post(prev));
                            self.lk_handle_lig_nest(n, first);
                        }
                        if self.lk_first(self.lk_rep(cur)) != 0 {
                            let n = self.lk_rep(prev);
                            let first = self.lk_first(self.lk_rep(prev));
                            self.lk_handle_lig_nest(n, first);
                        }
                    }
                    if self.lk_first(self.lk_post(cur)) != 0 {
                        let n = self.lk_post(cur);
                        let first = self.lk_first(n);
                        self.lk_handle_lig_nest(n, first);
                    }
                    if self.lk_first(self.lk_rep(cur)) != 0 {
                        let n = self.lk_rep(cur);
                        let first = self.lk_first(n);
                        self.lk_handle_lig_nest(n, first);
                    }
                    let mut fwd;
                    loop {
                        fwd = self.lua_nodes.next(cur);
                        if fwd == 0 || !self.lk_glyph(fwd) {
                            break;
                        }
                        if self.lk_sub(cur) != DISC_SELECT {
                            let nob = self.lk_tail(self.lk_rep(cur));
                            let pst = self.lk_tail(self.lk_post(cur));
                            if (nob == 0 || self.lk_test_ligature(nob, fwd).is_none())
                                && (pst == 0 || self.lk_test_ligature(pst, fwd).is_none())
                            {
                                break;
                            }
                            let copy = self.lua_nodes.copy_node(fwd);
                            let n = self.lk_rep(cur);
                            self.lk_nest_append(n, copy);
                            self.lk_handle_lig_nest(n, nob);
                        } else {
                            let mut dobreak = false;
                            let nob = self.lk_tail(self.lk_rep(prev));
                            let pst = self.lk_tail(self.lk_post(prev));
                            if (nob == 0 || self.lk_test_ligature(nob, fwd).is_none())
                                && (pst == 0 || self.lk_test_ligature(pst, fwd).is_none())
                            {
                                dobreak = true;
                            }
                            if !dobreak {
                                let copy = self.lua_nodes.copy_node(fwd);
                                let n = self.lk_rep(prev);
                                self.lk_nest_append(n, copy);
                                self.lk_handle_lig_nest(n, nob);
                                let copy = self.lua_nodes.copy_node(fwd);
                                let n = self.lk_post(prev);
                                self.lk_nest_append(n, copy);
                                self.lk_handle_lig_nest(n, pst);
                            }
                            dobreak = false;
                            let nob = self.lk_tail(self.lk_rep(cur));
                            let pst = self.lk_tail(self.lk_post(cur));
                            if (nob == 0 || self.lk_test_ligature(nob, fwd).is_none())
                                && (pst == 0 || self.lk_test_ligature(pst, fwd).is_none())
                            {
                                dobreak = true;
                            }
                            if !dobreak {
                                let copy = self.lua_nodes.copy_node(fwd);
                                let n = self.lk_rep(cur);
                                self.lk_nest_append(n, copy);
                                self.lk_handle_lig_nest(n, nob);
                            }
                            if dobreak {
                                break;
                            }
                        }
                        let next = self.lua_nodes.next(fwd);
                        self.lk_uncouple(fwd);
                        self.lk_try_couple(cur, next);
                        let pst = self.lk_tail(self.lk_post(cur));
                        let n = self.lk_post(cur);
                        self.lk_nest_append(n, fwd);
                        self.lk_handle_lig_nest(n, pst);
                    }
                    if fwd != 0 && self.lua_nodes.id(fwd) == DISC {
                        let next = self.lua_nodes.next(fwd);
                        let tp = self.lk_tail(self.lk_post(cur));
                        let tn = self.lk_tail(self.lk_rep(cur));
                        if self.lk_first(self.lk_rep(fwd)) == 0
                            && self.lk_first(self.lk_post(fwd)) == 0
                            && next != 0
                            && self.lk_glyph(next)
                            && ((tp != 0 && self.lk_test_ligature(tp, next).is_some())
                                || (tn != 0 && self.lk_test_ligature(tn, next).is_some()))
                        {
                            // optionally building an init_disc followed by a select_disc
                            let last1 = self.lua_nodes.next(next);
                            self.lk_uncouple(next);
                            self.lk_try_couple(fwd, last1);
                            let mode = 0; // \discretionaryligaturemode
                            if mode == 1 {
                                let tail = self.lk_tail(self.lk_rep(cur));
                                let copy = self.lua_nodes.copy_node(next);
                                let n = self.lk_rep(cur);
                                self.lk_nest_append(n, copy);
                                self.lk_handle_lig_nest(n, tail);
                                let tail = self.lk_tail(self.lk_post(cur));
                                let n = self.lk_post(cur);
                                self.lk_nest_append(n, next);
                                self.lk_handle_lig_nest(n, tail);
                                let a = self.lua_nodes.prev(fwd);
                                let b = self.lua_nodes.next(fwd);
                                self.lk_try_couple(a, b);
                                self.lua_nodes.flush_node(fwd);
                            } else if mode == 2 {
                                let copy = self.lua_nodes.copy_node(next);
                                let n = self.lk_post(fwd);
                                self.lk_nest_append(n, copy);
                                if self.lk_first(self.lk_rep(cur)) != 0 {
                                    let list = self.lua_nodes.copy_list(self.lk_first(self.lk_rep(cur)));
                                    let n = self.lk_rep(fwd);
                                    self.lk_nest_prepend_list(n, list);
                                    let tail = self.lk_tail(n);
                                    self.lk_nest_append(n, next);
                                    self.lk_handle_lig_nest(n, tail);
                                    let list = self.lua_nodes.copy_list(self.lk_first(self.lk_rep(cur)));
                                    let n = self.lk_pre(fwd);
                                    self.lk_nest_prepend_list(n, list);
                                }
                                let a = self.lua_nodes.prev(cur);
                                let b = self.lua_nodes.next(cur);
                                self.lk_try_couple(a, b);
                                self.lua_nodes.flush_node(cur);
                                cur = fwd;
                            } else {
                                let copy = self.lua_nodes.copy_node(next);
                                let n = self.lk_post(fwd);
                                self.lk_nest_append(n, copy);
                                if self.lk_first(self.lk_rep(cur)) != 0 {
                                    let c = self.lua_nodes.copy_node(self.lk_first(self.lk_pre(fwd)));
                                    let n = self.lk_rep(fwd);
                                    self.lk_nest_prepend(n, c);
                                }
                                if self.lk_first(self.lk_post(cur)) != 0 {
                                    let list = self.lua_nodes.copy_list(self.lk_first(self.lk_post(cur)));
                                    let n = self.lk_pre(fwd);
                                    self.lk_nest_prepend_list(n, list);
                                }
                                if self.lk_first(self.lk_rep(cur)) != 0 {
                                    let list = self.lua_nodes.copy_list(self.lk_first(self.lk_rep(cur)));
                                    let n = self.lk_rep(fwd);
                                    self.lk_nest_prepend_list(n, list);
                                }
                                let tail = self.lk_tail(self.lk_rep(cur));
                                let copy = self.lua_nodes.copy_node(next);
                                let n = self.lk_rep(cur);
                                self.lk_nest_append(n, copy);
                                self.lk_handle_lig_nest(n, tail);
                                let tail = self.lk_tail(self.lk_post(cur));
                                let n = self.lk_post(cur);
                                self.lk_nest_append(n, next);
                                self.lk_handle_lig_nest(n, tail);
                                self.lua_nodes.node_mut(cur).subtype = DISC_INIT;
                                self.lua_nodes.node_mut(fwd).subtype = DISC_SELECT;
                            }
                        }
                    }
                }
            } else {
                // neither glyph nor disc
                return last;
            }
            last = cur;
            cur = self.lua_nodes.next(cur);
            if cur == 0 {
                return last;
            }
        }
        cur
    }

    /// The last node of a nest (0 when it is empty).
    fn lk_tail(&self, nest: u32) -> u32 {
        let first = self.lua_nodes.next(nest);
        self.lua_nodes.tail_of(first)
    }

    /// `handle_ligaturing`: the new tail. `head` is a dummy.
    pub(crate) fn lk_handle_ligaturing(&mut self, head: u32, tail: u32) -> u32 {
        if self.lua_nodes.next(head) == 0 {
            return tail;
        }
        let mut save_tail = 0;
        if tail != 0 {
            save_tail = self.lua_nodes.next(tail);
            self.lua_nodes.node_mut(tail).next = 0;
        }
        self.lk_open_discs(head);
        let mut prev = head;
        let mut cur = self.lua_nodes.next(prev);
        while cur != 0 {
            if self.lk_glyph(cur) || self.lua_nodes.id(cur) == BOUNDARY {
                cur = self.lk_handle_lig_word(cur);
                if cur == 0 {
                    break;
                }
            }
            prev = cur;
            cur = self.lua_nodes.next(cur);
        }
        self.lk_close_discs(head);
        if prev == 0 {
            prev = tail;
        }
        if tail != 0 {
            self.lk_try_couple(prev, save_tail);
        }
        prev
    }

    // -------------------------------------------------------------- kerning

    fn lk_has_kern(&self, f: i32, c: i32) -> bool {
        self.lk_font(f)
            .and_then(|font| font.char_info_or_boundary(c).map(|ci| !ci.kerns.is_empty()))
            .unwrap_or(false)
    }

    fn lk_raw_kern(&self, f: i32, l: i32, r: i32) -> i32 {
        self.lk_font(f).and_then(|font| font.kern(l, r)).unwrap_or(0)
    }

    fn lk_new_kern(&mut self, k: i32) -> u32 {
        let attr = self.lk_current_attr();
        let n = self.lua_nodes.new_node(KERN, FONT_KERN, attr);
        self.lua_nodes.node_mut(n).f[0] = k;
        n
    }

    fn lk_add_kern_before(&mut self, left: u32, right: u32) {
        if !self.lk_right_ghost(right) && self.lk_fnt(left) == self.lk_fnt(right) && self.lk_has_kern(self.lk_fnt(left), self.lk_ch(left)) {
            let k = self.lk_raw_kern(self.lk_fnt(left), self.lk_ch(left), self.lk_ch(right));
            if k != 0 {
                let kern = self.lk_new_kern(k);
                let prev = self.lua_nodes.prev(right);
                self.lua_nodes.couple(prev, kern);
                self.lua_nodes.couple(kern, right);
                self.lk_copy_attr(left, kern);
            }
        }
    }

    fn lk_add_kern_after(&mut self, left: u32, right: u32, aft: u32) {
        if !self.lk_right_ghost(right) && self.lk_fnt(left) == self.lk_fnt(right) && self.lk_has_kern(self.lk_fnt(left), self.lk_ch(left)) {
            let k = self.lk_raw_kern(self.lk_fnt(left), self.lk_ch(left), self.lk_ch(right));
            if k != 0 {
                let kern = self.lk_new_kern(k);
                let next = self.lua_nodes.next(aft);
                self.lua_nodes.couple(aft, kern);
                self.lk_try_couple(kern, next);
                self.lk_copy_attr(aft, kern);
            }
        }
    }

    fn lk_do_kerning(&mut self, root: u32, init_left: u32, init_right: u32) {
        let mut cur = self.lua_nodes.next(root);
        let mut left = 0u32;
        if cur == 0 {
            if init_left != 0 && init_right != 0 {
                self.lk_add_kern_after(init_left, init_right, root);
            }
            return;
        }
        if self.lk_glyph(cur) {
            self.lk_set_is_glyph(cur);
            if init_left != 0 {
                self.lk_add_kern_before(init_left, cur);
            }
            left = cur;
        }
        loop {
            cur = self.lua_nodes.next(cur);
            if cur == 0 {
                break;
            }
            if self.lk_glyph(cur) {
                self.lk_set_is_glyph(cur);
                if left != 0 {
                    self.lk_add_kern_before(left, cur);
                    if self.lk_ch(left) < 0 || self.lk_ghost(left) {
                        let prev = self.lua_nodes.prev(left);
                        self.lua_nodes.couple(prev, cur);
                        self.lua_nodes.flush_node(left);
                    }
                }
                left = cur;
            } else {
                if self.lua_nodes.id(cur) == DISC {
                    let nx = self.lua_nodes.next(cur);
                    let right = if self.lk_glyph(nx) { nx } else { 0 };
                    let pre = self.lk_pre(cur);
                    self.lk_do_kerning(pre, left, 0);
                    let post = self.lk_post(cur);
                    self.lk_do_kerning(post, 0, right);
                    let rep = self.lk_rep(cur);
                    self.lk_do_kerning(rep, left, right);
                }
                if left != 0 {
                    if self.lk_ch(left) < 0 || self.lk_ghost(left) {
                        let prev = self.lua_nodes.prev(left);
                        self.lua_nodes.couple(prev, cur);
                        self.lua_nodes.flush_node(left);
                    }
                    left = 0;
                }
            }
        }
        if left != 0 {
            if init_right != 0 {
                self.lk_add_kern_after(left, init_right, left);
            }
            if self.lk_ch(left) < 0 || self.lk_ghost(left) {
                let prev = self.lua_nodes.prev(left);
                let next = self.lua_nodes.next(left);
                if next != 0 {
                    self.lua_nodes.couple(prev, next);
                } else if prev != root {
                    self.lua_nodes.node_mut(prev).next = 0;
                } else {
                    self.lua_nodes.node_mut(root).next = 0;
                }
                self.lua_nodes.flush_node(left);
            }
        }
    }

    fn lk_set_is_glyph(&mut self, n: u32) {
        self.lua_nodes.node_mut(n).subtype &= !GLYPH_CHARACTER;
    }

    /// `handle_kerning`: the new tail. `head` is a dummy.
    pub(crate) fn lk_handle_kerning(&mut self, head: u32, tail: u32) -> u32 {
        let mut save_link = 0;
        if tail != 0 {
            save_link = self.lua_nodes.next(tail);
            self.lua_nodes.node_mut(tail).next = 0;
        }
        self.lk_open_discs(head);
        self.lk_do_kerning(head, 0, 0);
        self.lk_close_discs(head);
        if tail == 0 {
            return 0;
        }
        let first = self.lua_nodes.next(head);
        let new_tail = if first == 0 { head } else { self.lua_nodes.tail_of(first) };
        if save_link != 0 && self.lua_nodes.valid(save_link) {
            self.lk_try_couple(new_tail, save_link);
        }
        new_tail
    }

    // ------------------------------------------------------- new_ligkern

    /// `new_ligkern`: ligaturing, then kerning, each through its callback
    /// when one is registered. Returns the new tail.
    pub(crate) fn lk_new_ligkern(&mut self, head: u32, mut tail: u32) -> u32 {
        if head == 0 {
            return 0;
        }
        if self.lua_nodes.next(head) == 0 {
            return tail;
        }
        let state = self.cb_state(Cb::Ligaturing);
        if state > 0 {
            let mut save_tail = 0;
            if tail != 0 {
                save_tail = self.lua_nodes.next(tail);
                self.lua_nodes.node_mut(tail).next = 0;
            }
            let _ = self.lua_cb_call(Cb::Ligaturing, "ligkern", vec![CbArg::Node(head), CbArg::Node(tail)]);
            tail = self.lua_nodes.tail_of(head);
            if save_tail != 0 {
                self.lk_try_couple(tail, save_tail);
            }
            tail = self.lua_nodes.tail_of(head);
        } else if state == 0 {
            tail = self.lk_handle_ligaturing(head, tail);
        }
        let state = self.cb_state(Cb::Kerning);
        if state > 0 {
            let _ = self.lua_cb_call(Cb::Kerning, "ligkern", vec![CbArg::Node(head), CbArg::Node(tail)]);
            tail = self.lua_nodes.tail_of(head);
        } else if state == 0 {
            let nest1 = self.lua_nodes.new_node(TEMP, 1, 0);
            let cur = self.lua_nodes.next(head);
            let aft = if tail != 0 { self.lua_nodes.next(tail) } else { 0 };
            self.lua_nodes.couple(nest1, cur);
            if tail != 0 {
                self.lua_nodes.node_mut(tail).next = 0;
            }
            self.lk_open_discs(nest1);
            self.lk_do_kerning(nest1, 0, 0);
            self.lk_close_discs(nest1);
            let first = self.lua_nodes.next(nest1);
            self.lua_nodes.couple(head, first);
            tail = if first == 0 { head } else { self.lua_nodes.tail_of(first) };
            if aft != 0 {
                self.lk_try_couple(tail, aft);
            }
            self.lua_nodes.node_mut(nest1).next = 0;
            self.lua_nodes.flush_node(nest1);
        }
        tail
    }

    // -------------------------------------------------------- hyphenation

    fn lk_hj_code(&self, lang: i32, c: i32) -> i32 {
        let _ = lang;
        if c < 0 {
            0
        } else if c < 256 {
            i32::from(self.eqtb.lc_code[c as usize])
        } else {
            match char::from_u32(c as u32) {
                Some(ch) if ch.is_alphabetic() => {
                    let mut lower = ch.to_lowercase();
                    match (lower.next(), lower.next()) {
                        (Some(l), None) => l as i32,
                        _ => c,
                    }
                }
                _ => 0,
            }
        }
    }

    /// `insert_discretionary(t, pre, post, replace, penalty)`: a new
    /// discretionary after `t`, or replacing `t` when `replace == t`.
    fn lk_insert_discretionary(&mut self, t: u32, pre: u32, post: u32, replace: u32, penalty: i32) -> u32 {
        let attr_node = t;
        let d = self.lua_nodes.new_node(DISC, DISC_SYLLABLE, 0);
        self.lua_nodes.node_mut(d).f[sl::D_PENALTY] = penalty;
        let mut replace = replace;
        if t == replace {
            let next = self.lua_nodes.next(t);
            self.lk_try_couple(d, next);
            let prev = self.lua_nodes.prev(t);
            self.lk_try_couple(prev, d);
            self.lua_nodes.node_mut(t).prev = 0;
            self.lua_nodes.node_mut(t).next = 0;
            replace = t;
        } else {
            let next = self.lua_nodes.next(t);
            self.lk_try_couple(d, next);
            self.lua_nodes.couple(t, d);
        }
        let font = if replace != 0 { self.lk_fnt(replace) } else { i32::from(self.eqtb.cur_font_val) };
        let mut g = pre;
        while g != 0 {
            if self.lk_fnt(g) == 0 {
                self.lua_nodes.node_mut(g).f[sl::C_FONT] = font;
            }
            self.lk_copy_attr(attr_node, g);
            g = self.lua_nodes.next(g);
        }
        let mut g = post;
        while g != 0 {
            if self.lk_fnt(g) == 0 {
                self.lua_nodes.node_mut(g).f[sl::C_FONT] = font;
            }
            self.lk_copy_attr(attr_node, g);
            g = self.lua_nodes.next(g);
        }
        let mut g = replace;
        while g != 0 {
            self.lk_copy_attr(attr_node, g);
            g = self.lua_nodes.next(g);
        }
        self.lk_copy_attr(attr_node, d);
        for (slot, list) in [(sl::D_PRE, pre), (sl::D_POST, post), (sl::D_REPLACE, replace)] {
            if list != 0 {
                self.lua_nodes.node_mut(list).prev = 0;
            }
            self.lua_nodes.node_mut(d).f[slot] = list as i32;
        }
        d
    }

    /// `insert_syllable_discretionary(t)`: a hyphenation point after `t`.
    fn lk_insert_syllable_disc(&mut self, t: u32) -> u32 {
        let d = self.lua_nodes.new_node(DISC, DISC_SYLLABLE, 0);
        let penalty = self.eqtb.int_params[IntParam::HyphenPenalty.idx() as usize];
        self.lua_nodes.node_mut(d).f[sl::D_PENALTY] = penalty;
        let next = self.lua_nodes.next(t);
        self.lk_try_couple(d, next);
        self.lua_nodes.couple(t, d);
        self.lk_copy_attr(t, d);
        let lang = self.lk_lang(t);
        let pre_char = self.lk_pre_hyphen_char(lang);
        let post_char = self.lk_post_hyphen_char(lang);
        if pre_char > 0 {
            let g = self.lua_nodes.new_node(GLYPH, GLYPH_CHARACTER, 0);
            let tf = self.lua_nodes.node(t).f;
            let f = &mut self.lua_nodes.node_mut(g).f;
            f[sl::C_CHAR] = pre_char;
            f[sl::C_FONT] = tf[sl::C_FONT];
            f[sl::C_LANG] = tf[sl::C_LANG];
            f[sl::C_LEFT] = tf[sl::C_LEFT];
            f[sl::C_RIGHT] = tf[sl::C_RIGHT];
            f[sl::C_UCHYPH] = tf[sl::C_UCHYPH];
            self.lk_copy_attr(t, g);
            self.lua_nodes.node_mut(d).f[sl::D_PRE] = g as i32;
        }
        if post_char > 0 {
            let t2 = self.lua_nodes.next(d);
            if t2 != 0 {
                let g = self.lua_nodes.new_node(GLYPH, GLYPH_CHARACTER, 0);
                let tf = self.lua_nodes.node(t2).f;
                let f = &mut self.lua_nodes.node_mut(g).f;
                f[sl::C_CHAR] = post_char;
                f[sl::C_FONT] = tf[sl::C_FONT];
                f[sl::C_LANG] = tf[sl::C_LANG];
                f[sl::C_LEFT] = tf[sl::C_LEFT];
                f[sl::C_RIGHT] = tf[sl::C_RIGHT];
                f[sl::C_UCHYPH] = tf[sl::C_UCHYPH];
                self.lk_copy_attr(t2, g);
                self.lua_nodes.node_mut(d).f[sl::D_POST] = g as i32;
            }
        }
        d
    }

    fn lk_pre_hyphen_char(&self, lang: i32) -> i32 {
        u8::try_from(lang)
            .ok()
            .and_then(|l| self.lua_tex.lang.get(&l))
            .and_then(|p| p.pre_hyphen)
            .unwrap_or(i32::from(b'-'))
    }

    fn lk_post_hyphen_char(&self, lang: i32) -> i32 {
        u8::try_from(lang).ok().and_then(|l| self.lua_tex.lang.get(&l)).map_or(0, |p| p.post_hyphen)
    }

    fn lk_insert_character(&mut self, c: i32) -> u32 {
        let attr = self.lk_current_attr();
        let p = self.lua_nodes.new_node(GLYPH, GLYPH_CHARACTER, attr);
        self.lua_nodes.node_mut(p).f[sl::C_CHAR] = c;
        p
    }

    /// `compound_word_break`: the explicit hyphen `t` becomes an automatic
    /// discretionary.
    fn lk_compound_word_break(&mut self, t: u32) -> u32 {
        let ex = self.eqtb.int_params[IntParam::ExHyphenChar.idx() as usize];
        let pre = self.lk_insert_character(ex);
        let penalty = self.eqtb.int_params[IntParam::ExHyphenPenalty.idx() as usize];
        let disc = self.lk_insert_discretionary(t, pre, 0, t, penalty);
        self.lua_nodes.node_mut(disc).subtype = DISC_AUTOMATIC;
        disc
    }

    fn lk_find_next_wordstart(&mut self, mut r: u32, first_language: i32, strict_bound: i32) -> u32 {
        let ex = self.eqtb.int_params[IntParam::ExHyphenChar.idx() as usize];
        let mut start_ok = true;
        let mut mathlevel = 1;
        while r != 0 {
            match self.lua_nodes.id(r) {
                BOUNDARY => {
                    if self.lk_sub(r) == 3 {
                        start_ok = true;
                    }
                }
                HLIST | VLIST | RULE | DIR | WHATSIT => {
                    if strict_bound == 1 || strict_bound == 3 {
                        start_ok = false;
                    }
                }
                GLUE => start_ok = true,
                MATH => {
                    while mathlevel > 0 {
                        r = self.lua_nodes.next(r);
                        if r == 0 {
                            return r;
                        }
                        if self.lua_nodes.id(r) == MATH {
                            if self.lk_sub(r) == 0 {
                                mathlevel += 1;
                            } else {
                                mathlevel -= 1;
                            }
                        }
                    }
                }
                GLYPH => {
                    if self.lk_simple(r) {
                        let chr = self.lk_ch(r);
                        if chr == ex {
                            let mut t = self.lua_nodes.next(r);
                            if self.lk_glyph(t) && self.lk_ch(t) != ex {
                                // automatic hyphen mode 0: no word yet and the next
                                // character is not a hyphen
                                r = self.lk_compound_word_break(r);
                            } else {
                                while self.lk_glyph(t) && self.lk_ch(t) == ex {
                                    r = t;
                                    t = self.lua_nodes.next(r);
                                }
                                if t == 0 {
                                    return 0;
                                }
                            }
                            start_ok = false;
                        } else {
                            let l = self.lk_hj_code(self.lk_lang(r), chr);
                            if start_ok && self.lk_lang(r) >= first_language && l > 0 {
                                if self.lua_nodes.node(r).f[sl::C_UCHYPH] != 0 || l == chr || l <= 32 {
                                    return r;
                                } else {
                                    start_ok = false;
                                }
                            }
                        }
                    }
                }
                _ => start_ok = false,
            }
            r = self.lua_nodes.next(r);
        }
        r
    }

    fn lk_valid_wordend(&self, s: u32, strict_bound: i32) -> bool {
        let mut r = s;
        let clang = self.lk_lang(s);
        if r == 0 {
            return true;
        }
        while r != 0
            && ((self.lk_glyph(r) && self.lk_simple(r) && clang == self.lk_lang(r))
                || (self.lua_nodes.id(r) == KERN && self.lk_sub(r) == FONT_KERN))
        {
            r = self.lua_nodes.next(r);
        }
        if r == 0 {
            return true;
        }
        let id = self.lua_nodes.id(r);
        let sub = self.lk_sub(r);
        if (id == GLYPH && self.lk_simple(r) && clang != self.lk_lang(r))
            || id == GLUE
            || id == PENALTY
            || (id == KERN && matches!(sub, EXPLICIT_KERN | ITALIC_KERN | ACCENT_KERN))
            || (matches!(id, HLIST | VLIST | RULE | DIR | WHATSIT | INS | ADJUST) && !(strict_bound == 2 || strict_bound == 3))
            || id == BOUNDARY
        {
            return true;
        }
        false
    }

    /// `hnj_hyphenation(head, tail)`: insert the discretionaries of the
    /// hyphenatable words and compound words of the list after `head`.
    pub(crate) fn lk_hyphenation(&mut self, head: u32, tail: u32) {
        if tail == 0 {
            return;
        }
        let first_language = self.eqtb.int_params[IntParam::FirstValidLanguage.idx() as usize];
        let strict_bound = 0;
        let ex = self.eqtb.int_params[IntParam::ExHyphenChar.idx() as usize];
        let mut r = head;
        while r != 0 && !self.lk_simple(r) {
            r = self.lua_nodes.next(r);
        }
        r = self.lk_find_next_wordstart(r, first_language, strict_bound);
        if r == 0 {
            return;
        }
        let save_tail1 = self.lua_nodes.next(tail);
        let s = self.lua_nodes.new_node(PENALTY, 0, 0);
        self.lua_nodes.couple(tail, s);
        let mut word: Vec<u32> = Vec::new();
        let mut letters: Vec<u32> = Vec::new();
        let mut explicit_hyphen = false;
        while r != 0 {
            let wordstart = r;
            let mut end_word = r;
            let mut hyf_font = self.lk_fnt(wordstart);
            if self.eqtb.hyphen_char.get(hyf_font as usize).copied().unwrap_or(-1) < 0 {
                hyf_font = 0;
            }
            let clang = self.lk_lang(wordstart);
            let mut lhmin = self.lua_nodes.node(wordstart).f[sl::C_LEFT];
            let mut rhmin = self.lua_nodes.node(wordstart).f[sl::C_RIGHT];
            let mut wordlen = 0i32;
            word.clear();
            letters.clear();
            let mut too_long = false;
            while r != 0 && self.lk_glyph(r) && self.lk_simple(r) && clang == self.lk_lang(r) {
                let ch = self.lk_ch(r);
                let mut lchar;
                if clang >= first_language {
                    lchar = self.lk_hj_code(clang, ch);
                    if lchar <= 0 {
                        if ch == ex && ex != 0 {
                            lchar = ex;
                        } else {
                            break;
                        }
                    }
                } else if ch == ex && ex != 0 {
                    lchar = ex;
                } else {
                    break;
                }
                if ch == ex {
                    explicit_hyphen = true;
                    break;
                }
                wordlen += 1;
                if wordlen as usize >= MAX_WORD_LEN {
                    while r != 0 && self.lk_glyph(r) {
                        r = self.lua_nodes.next(r);
                    }
                    too_long = true;
                    break;
                }
                if lchar <= 32 {
                    if lchar == 32 {
                        lchar = 0;
                    }
                    if wordlen <= lhmin {
                        lhmin = lhmin - lchar + 1;
                        if lhmin < 0 {
                            lhmin = 1;
                        }
                    }
                    if wordlen >= rhmin {
                        rhmin = rhmin - lchar + 1;
                        if rhmin < 0 {
                            rhmin = 1;
                        }
                    }
                    lchar = ch;
                }
                word.push(lchar as u32);
                letters.push(r);
                end_word = r;
                r = self.lua_nodes.next(r);
            }
            if too_long {
                // PICKUP
            } else if explicit_hyphen {
                // we are not at the start, so we only need to look ahead
                let mut t = self.lua_nodes.next(r);
                if self.lk_glyph(t) && self.lk_simple(t) && self.lk_ch(t) != ex {
                    // we have a word already but the next character may not be a hyphen too
                    r = self.lk_compound_word_break(r);
                } else {
                    while self.lk_glyph(t) && self.lk_simple(t) && self.lk_ch(t) == ex {
                        r = t;
                        t = self.lua_nodes.next(r);
                    }
                    if t == 0 {
                        r = 0;
                    }
                }
            } else if self.lk_valid_wordend(r, strict_bound)
                && clang >= first_language
                && wordlen >= lhmin + rhmin
                && hyf_font != 0
            {
                let positions = self.lk_word_points(clang, &word, lhmin, rhmin);
                for k in positions {
                    // the discretionary after the k-th letter
                    if let Some(&t) = letters.get(k - 1) {
                        if k < letters.len() {
                            let _ = end_word;
                            self.lk_insert_syllable_disc(t);
                        }
                    }
                }
            }
            // PICKUP
            explicit_hyphen = false;
            if r == 0 {
                break;
            }
            r = self.lk_find_next_wordstart(r, first_language, strict_bound);
        }
        // remove the sentinel penalty
        let sentinel = self.lua_nodes.next(tail);
        self.lua_nodes.flush_node(sentinel);
        self.lk_try_couple(tail, save_tail1);
    }

    /// The letters after which a hyphen may be inserted: exceptions as they
    /// are, patterns within the left and right minima.
    fn lk_word_points(&self, lang: i32, word: &[u32], lhmin: i32, rhmin: i32) -> Vec<usize> {
        let Some(trie) = u8::try_from(lang).ok().and_then(|l| self.trie_for_language(l)) else {
            return Vec::new();
        };
        if let Some(points) = trie.exception_points(word) {
            return points.into_iter().filter(|&k| k > 0 && k < word.len()).collect();
        }
        let gaps = trie.gap_values(word);
        let n = word.len() as i32;
        let mut out = Vec::new();
        let mut i = lhmin.max(1);
        while i <= n - rhmin.max(1) {
            if gaps.get(i as usize).is_some_and(|g| g & 1 == 1) {
                out.push(i as usize);
            }
            i += 1;
        }
        out
    }

    /// `new_hyphenation(head, tail)`: through the `hyphenate` callback, or
    /// the built-in pass when none is registered.
    pub(crate) fn lk_new_hyphenation(&mut self, head: u32, tail: u32) {
        if head == 0 || self.lua_nodes.next(head) == 0 {
            return;
        }
        let state = self.cb_state(Cb::Hyphenate);
        if state > 0 {
            let _ = self.lua_cb_call(Cb::Hyphenate, "hyphenation", vec![CbArg::Node(head), CbArg::Node(tail)]);
        } else if state == 0 {
            self.lk_hyphenation(head, tail);
        }
    }

    // ------------------------------------------------- engine integration

    /// Whether the text passes have anything to do for `list`: a Lua font
    /// glyph is present, or Lua has registered a callback of the passes.
    fn lk_wanted(&self, list: &[crate::boxes::Node]) -> bool {
        if self.engine_kind != EngineKind::LuaTeX {
            return false;
        }
        if self.cb_state(Cb::Hyphenate) > 0 || self.cb_state(Cb::Ligaturing) > 0 || self.cb_state(Cb::Kerning) > 0 {
            return true;
        }
        list.iter().any(|n| matches!(n, crate::boxes::Node::LuaGlyph(_)))
    }

    /// The LuaTeX text passes on a list of the engine (a paragraph or the
    /// contents of an hbox), as `line_break` and `filtered_hpack` run them.
    pub(crate) fn lua_text_passes(&mut self, list: NodeList) -> NodeList {
        if !self.lk_wanted(&list) {
            return list;
        }
        let (head, tail) = self.lua_list_with_head(list);
        self.lua_text_passes_on(head, tail);
        self.lua_list_from_head(head)
    }

    /// The passes on a list that lives in Lua behind the `temp` node `head`
    /// whose last node is `tail`.
    pub(crate) fn lua_text_passes_on(&mut self, head: u32, tail: u32) {
        self.lk_new_hyphenation(head, tail);
        let tail = self.lua_nodes.tail_of(head);
        self.lk_new_ligkern(head, tail);
    }
}
