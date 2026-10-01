//! `node.hpack`, `node.vpack`, `node.dimensions`, `node.rangedimensions`
//! (packaging.c `hpack`, `vpackage`, `natural_sizes`) over the node store.
//!
//! Packing reuses the engine's own `hpack`/`vpack` as a calculator on a copy
//! of the list (so the badness reports match), then wraps the original
//! nodes, which keep their identity, in a new box node.

use tex_lua::{UdValue, Value, Variadic};

use crate::boxes::{self, Node};
use crate::engine::Engine;
use crate::lua_bridge::with_engine;
use crate::lua_node::*;
use crate::lua_node_conv::sl;

const MAX_DIMEN: i32 = 0x3FFF_FFFF;

fn vint(v: &Value) -> i64 {
    crate::lua_node_lib::value_int(v)
}

fn lua_order(o: u8) -> i32 {
    if o == 0 { 0 } else { i32::from(o) + 1 }
}

pub(crate) fn engine_order(o: i32) -> u8 {
    match o {
        i32::MIN..=0 => 0,
        1 => 1,
        o => (o - 1).min(3) as u8,
    }
}

pub(crate) fn lua_order_of(o: u8) -> i32 {
    lua_order(o)
}

/// glyph_height / glyph_depth of luatex: the character's extent shifted by
/// the vertical offset.
fn glyph_whd(e: &Engine, n: u32) -> (i32, i32, i32) {
    let f = e.lua_nodes.node(n).f;
    let (w, h, d) = crate::lua_node_lib::glyph_dimensions(&e.eqtb.fonts, f[sl::C_FONT], f[sl::C_CHAR]);
    let y = f[sl::C_YOFF];
    ((w), (h + y).max(0), (d - y).max(0))
}

impl Engine {
    /// `natural_sizes`: (width, height, depth) of the nodes from `p` up to
    /// (excluding) `pp`.
    pub(crate) fn lua_natural_sizes(&self, p: u32, pp: u32, g_mult: f64, g_sign: i32, g_order: i32) -> (i32, i32, i32) {
        let s = &self.lua_nodes;
        let (mut wd, mut ht, mut dp) = (0i64, 0i64, 0i64);
        let (mut gp, mut gm) = (0i64, 0i64);
        let mut p = p;
        while p != pp && p != 0 {
            let n = s.node(p);
            match n.id {
                GLYPH => {
                    let (w, h, d) = glyph_whd(self, p);
                    wd += i64::from(w);
                    ht = ht.max(i64::from(h));
                    dp = dp.max(i64::from(d));
                }
                HLIST | VLIST => {
                    let shift = i64::from(n.f[sl::B_SHIFT]);
                    wd += i64::from(n.f[sl::B_WIDTH]);
                    ht = ht.max(i64::from(n.f[sl::B_HEIGHT]) - shift);
                    dp = dp.max(i64::from(n.f[sl::B_DEPTH]) + shift);
                }
                RULE | UNSET => {
                    wd += i64::from(n.f[0]);
                    ht = ht.max(i64::from(n.f[2]));
                    dp = dp.max(i64::from(n.f[1]));
                }
                MATH | GLUE => {
                    let (base, zero) = if n.id == MATH {
                        (1, n.f[1] == 0 && n.f[2] == 0 && n.f[3] == 0)
                    } else {
                        (1, false)
                    };
                    if n.id == MATH && zero {
                        wd += i64::from(n.f[0]);
                    } else {
                        wd += i64::from(n.f[base]);
                        if g_sign != 0 {
                            if g_sign == 1 {
                                if n.f[base + 3] == g_order {
                                    gp += i64::from(n.f[base + 1]);
                                }
                            } else if n.f[base + 4] == g_order {
                                gm += i64::from(n.f[base + 2]);
                            }
                        }
                        if n.id == GLUE && n.subtype >= A_LEADERS {
                            let g = n.f[0] as u32;
                            if s.valid(g) {
                                let gn = s.node(g);
                                ht = ht.max(i64::from(gn.f[2]));
                                dp = dp.max(i64::from(gn.f[1]));
                            }
                        }
                    }
                }
                MARGIN_KERN => wd += i64::from(n.f[0]),
                KERN => wd += i64::from(n.f[0]) + i64::from(n.f[1]),
                DISC => {
                    let (w, h, d) = self.lua_natural_sizes(n.f[2] as u32, 0, g_mult, g_sign, g_order);
                    wd += i64::from(w);
                    ht = ht.max(i64::from(h));
                    dp = dp.max(i64::from(d));
                }
                _ => {}
            }
            p = n.next;
        }
        if g_sign != 0 {
            let adj = (g_mult * if g_sign == 1 { gp as f64 } else { gm as f64 }).round() as i64;
            if g_sign == 1 {
                wd += adj;
            } else {
                wd -= adj;
            }
        }
        (wd as i32, ht as i32, dp as i32)
    }

    /// A copy of the list at `head` in engine form.
    fn lua_clone_to_engine(&mut self, head: u32) -> Vec<Node> {
        let c = self.lua_nodes.copy_list(head);
        self.lua_nodes_to_engine(i64::from(c))
    }

    /// Pack the list at `head` like `hpack`/`vpackage`; returns the new box
    /// and the badness.
    pub(crate) fn lua_pack_list(&mut self, head: u32, size: i32, additional: bool, horizontal: bool) -> (u32, i32) {
        let list = self.lua_clone_to_engine(head);
        let orig_len = list.len();
        let res = if horizontal {
            boxes::hpack_add(list, Some(size), additional, boxes::HBOX, &self.eqtb)
        } else {
            boxes::vpack_add_md(list, Some(size), additional, boxes::VBOX, &self.eqtb, MAX_DIMEN)
        };
        self.report_pack_warnings(&res);
        let Node::Box { w, h, d, glue_sign, glue_order, glue_set, list, .. } = &res.node else {
            return (0, 0);
        };
        let b = self.lua_new_node(if horizontal { HLIST } else { VLIST }, 0);
        // an overfull rule the engine appended
        let mut head = head;
        if list.len() > orig_len {
            if let Some(extra) = list.last().cloned() {
                let mut ctx = Vec::new();
                ctx.push(extra);
                let rh = self.lua_nodes_from_engine(ctx) as u32;
                if head == 0 {
                    head = rh;
                } else {
                    let t = self.lua_nodes.tail_of(head);
                    self.lua_nodes.couple(t, rh);
                }
            }
        }
        let nd = self.lua_nodes.node_mut(b);
        nd.f[sl::B_WIDTH] = *w;
        nd.f[sl::B_HEIGHT] = *h;
        nd.f[sl::B_DEPTH] = *d;
        nd.f[sl::B_DIR] = 0;
        nd.f[sl::B_SIGN] = i32::from(*glue_sign);
        nd.f[sl::B_ORDER] = lua_order(*glue_order);
        nd.fl = f64::from(*glue_set as f32);
        nd.f[sl::B_HEAD] = head as i32;
        (b, res.badness)
    }
}

/// `node.direct.hpack(n, w, mode, dir)` / `vpack`.
pub(crate) fn lua_pack(args: &[Value], horizontal: bool) -> Result<Variadic<UdValue>, String> {
    let head = args.first().map_or(0, |v| vint(v) as u32);
    let mut w = 0i64;
    let mut mode = 1i64;
    if args.len() > 1 {
        w = (crate::lua_node_lib::value_num(&args[1]) + 0.5).floor() as i64;
        if args.len() > 2 {
            if let Some(s) = crate::lua_node_lib::value_bytes(&args[2]) {
                mode = match s.as_slice() {
                    b"additional" => 1,
                    b"exactly" => 0,
                    b"cal_expand_ratio" if horizontal => 2,
                    b"subst_ex_font" if horizontal => 3,
                    _ => return Err("3rd argument should be either additional or exactly".to_string()),
                };
            } else if args[2].as_integer().is_some() {
                mode = vint(&args[2]);
            }
        }
    }
    let mut dir = -1i32;
    if let Some(a) = args.get(3) {
        if let Some(i) = a.as_integer() {
            if !(0..4).contains(&i) {
                return Err(format!("Invalid direction value {i}"));
            }
            dir = i as i32;
        } else if let Some(b) = crate::lua_node_lib::value_bytes(a) {
            let text = String::from_utf8_lossy(&b).into_owned();
            match DIR_NAMES.iter().position(|d| *d == text) {
                Some(d) => dir = d as i32,
                None => return Err(format!("Bad direction specifier {text}")),
            }
        }
    }
    with_engine(|e| {
        let (b, bad) = e.lua_pack_list(head, w as i32, mode != 0, horizontal);
        if dir >= 0 && b != 0 {
            e.lua_nodes.node_mut(b).f[sl::B_DIR] = dir;
        }
        Variadic(vec![UdValue::Integer(i64::from(b)), UdValue::Integer(i64::from(bad))])
    })
}

/// `dimensions` and `rangedimensions` (direct handles).
pub(crate) fn lua_dimensions(args: &[Value], range: bool) -> Result<Variadic<UdValue>, String> {
    if range {
        if args.len() < 2 {
            return Err("missing argument to 'rangedimensions' (2 or more direct nodes expected)".to_string());
        }
        let parent = vint(&args[0]) as u32;
        let first = vint(&args[1]) as u32;
        let last = args.get(2).map_or(0, |v| vint(v) as u32);
        return with_engine(|e| {
            if !e.lua_nodes.valid(parent) {
                return Variadic(vec![]);
            }
            let p = e.lua_nodes.node(parent);
            let (g, sign, order) = (p.fl, p.f[sl::B_SIGN], p.f[sl::B_ORDER]);
            let (w, h, d) = e.lua_natural_sizes(first, last, g, sign, order);
            Variadic(vec![UdValue::Integer(i64::from(w)), UdValue::Integer(i64::from(h)), UdValue::Integer(i64::from(d))])
        });
    }
    if args.is_empty() {
        return Err("missing argument to 'dimensions' (direct node expected)".to_string());
    }
    let (mut mult, mut sign, mut order) = (1.0f64, 0i32, 0i32);
    let mut i = 0;
    if args.len() > 3 {
        i = 3;
        mult = crate::lua_node_lib::value_num(&args[0]);
        sign = vint(&args[1]) as i32;
        order = vint(&args[2]) as i32;
    }
    let n = vint(&args[i]) as u32;
    let mut stop = 0u32;
    if let Some(a) = args.get(i + 1) {
        if !a.is_nil() && crate::lua_node_lib::value_bytes(a).is_none() {
            stop = vint(a) as u32;
        }
    }
    with_engine(|e| {
        let (w, h, d) = e.lua_natural_sizes(n, stop, mult, sign, order);
        Variadic(vec![UdValue::Integer(i64::from(w)), UdValue::Integer(i64::from(h)), UdValue::Integer(i64::from(d))])
    })
}
