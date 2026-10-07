//! The iterator functions of `node.traverse*` and `node.direct.traverse*`
//! (lnodelib.c `nodelib_aux_next*`, `nodelib_direct_aux_next*`), which
//! `lua_node.lua` hands to the generic `for`.
//!
//! As in LuaTeX, an iterator called with a nil control value starts at its
//! state (the head) and otherwise at the node after the control value, read
//! when the step runs: a loop body may relink or replace the node it is at.
//! The first node from there that the iterator accepts is returned with its
//! data; past the end a single nil is returned.

use tex_lua::{Lua, LuaApi, LuaTable, ToInteger, UdValue};

use crate::lua_bridge::with_engine;
use crate::lua_node::*;
use crate::lua_node_conv::sl;
use crate::lua_node_lib::NodeUd;

/// Node ids below this get a prebuilt `traverse_id` iterator each; other
/// filter values use one that takes the id as an argument.
const PREBUILT_IDS: u8 = 64;

macro_rules! nat {
    ($lua:expr, $tbl:expr, $name:expr, $f:expr) => {
        $tbl.set($name, $lua.create_function($f).map_err(|e| format!("{}: {e:?}", $name))?)
            .map_err(|e| format!("{}: {e:?}", $name))?
    };
}

/// The node a step stops at: from `first` (no control value) or from the
/// node after `cur`, the first node `keep` accepts; 0 past the end. A node
/// Lua has freed ends the walk.
#[inline]
fn step(s: &NodeStore, first: u32, cur: Option<u32>, keep: impl Fn(&LNode) -> bool) -> u32 {
    let mut t = match cur {
        None => first,
        Some(c) if s.valid(c) => s.node(c).next,
        Some(_) => 0,
    };
    while s.valid(t) {
        let nd = s.node(t);
        if keep(nd) {
            return t;
        }
        t = nd.next;
    }
    0
}

fn any(_: &LNode) -> bool {
    true
}

/// `traverse_char`: glyphs not yet processed (subtype below 256)
fn char_glyph(nd: &LNode) -> bool {
    nd.id == GLYPH && nd.subtype < 256
}

fn glyph(nd: &LNode) -> bool {
    nd.id == GLYPH
}

fn list(nd: &LNode) -> bool {
    nd.id == HLIST || nd.id == VLIST
}

fn handle(v: ToInteger) -> u32 {
    v.0 as u32
}

fn ud_handle(v: &NodeUd) -> u32 {
    v.h
}

fn int(v: impl Into<i64>) -> i64 {
    v.into()
}

fn ud(h: u32) -> UdValue {
    UdValue::from_userdata(NodeUd { h })
}

fn ud_or_nil(h: u32) -> UdValue {
    if h == 0 { UdValue::Nil } else { ud(h) }
}

fn list_head(nd: &LNode) -> u32 {
    nd.f[sl::B_HEAD] as u32
}

/// Register the iterators on the natives table `n`: `trav_next`,
/// `trav_char`, `trav_glyph`, `trav_list`, `trav_id` (id, state, control)
/// and the table `trav_ids` of per-id iterators, and the same with a `_ud`
/// suffix for userdata nodes.
pub(crate) fn install(lua: &mut Lua, n: &LuaTable) -> Result<(), String> {
    // ---- node.direct ----
    nat!(lua, n, "trav_next", |s: ToInteger, c: Option<ToInteger>| -> Result<Option<(i64, i64, i64)>, String> {
        with_engine(|e| {
            let st = &e.lua_nodes;
            let t = step(st, handle(s), c.map(handle), any);
            (t != 0).then(|| (int(t), int(st.id(t)), int(st.subtype(t))))
        })
    });
    nat!(lua, n, "trav_char", |s: ToInteger, c: Option<ToInteger>| -> Result<Option<(i64, i64, i64)>, String> {
        with_engine(|e| {
            let st = &e.lua_nodes;
            let t = step(st, handle(s), c.map(handle), char_glyph);
            (t != 0).then(|| (int(t), int(st.node(t).f[0]), int(st.node(t).f[1])))
        })
    });
    nat!(lua, n, "trav_glyph", |s: ToInteger, c: Option<ToInteger>| -> Result<Option<(i64, i64, i64)>, String> {
        with_engine(|e| {
            let st = &e.lua_nodes;
            let t = step(st, handle(s), c.map(handle), glyph);
            (t != 0).then(|| (int(t), int(st.node(t).f[0]), int(st.node(t).f[1])))
        })
    });
    nat!(lua, n, "trav_list", |s: ToInteger, c: Option<ToInteger>| -> Result<Option<(i64, i64, i64, Option<i64>)>, String> {
        with_engine(|e| {
            let st = &e.lua_nodes;
            let t = step(st, handle(s), c.map(handle), list);
            (t != 0).then(|| {
                let nd = st.node(t);
                let head = list_head(nd);
                (int(t), int(nd.id), int(nd.subtype), (head != 0).then_some(int(head)))
            })
        })
    });
    nat!(lua, n, "trav_id", |id: ToInteger, s: ToInteger, c: Option<ToInteger>| -> Result<Option<(i64, i64)>, String> {
        // the C int of lua_tointeger: no node id matches outside 0..=255
        let id = id.0 as i32;
        with_engine(|e| {
            let st = &e.lua_nodes;
            let t = step(st, handle(s), c.map(handle), |nd| i32::from(nd.id) == id);
            (t != 0).then(|| (int(t), int(st.subtype(t))))
        })
    });
    let ids = lua.create_table().map_err(|e| format!("{e:?}"))?;
    for id in 0..PREBUILT_IDS {
        nat!(lua, ids, i64::from(id), move |s: ToInteger, c: Option<ToInteger>| -> Result<Option<(i64, i64)>, String> {
            with_engine(|e| {
                let st = &e.lua_nodes;
                let t = step(st, handle(s), c.map(handle), |nd| nd.id == id);
                (t != 0).then(|| (int(t), int(st.subtype(t))))
            })
        });
    }
    n.set("trav_ids", ids).map_err(|e| format!("trav_ids: {e:?}"))?;

    // ---- node (userdata) ----
    nat!(lua, n, "trav_next_ud", |s: NodeUd, c: Option<NodeUd>| -> Result<Option<(UdValue, i64, i64)>, String> {
        with_engine(|e| {
            let st = &e.lua_nodes;
            let t = step(st, ud_handle(&s), c.as_ref().map(ud_handle), any);
            (t != 0).then(|| (ud(t), int(st.id(t)), int(st.subtype(t))))
        })
    });
    nat!(lua, n, "trav_char_ud", |s: NodeUd, c: Option<NodeUd>| -> Result<Option<(UdValue, i64, i64)>, String> {
        with_engine(|e| {
            let st = &e.lua_nodes;
            let t = step(st, ud_handle(&s), c.as_ref().map(ud_handle), char_glyph);
            (t != 0).then(|| (ud(t), int(st.node(t).f[0]), int(st.node(t).f[1])))
        })
    });
    nat!(lua, n, "trav_glyph_ud", |s: NodeUd, c: Option<NodeUd>| -> Result<Option<(UdValue, i64, i64)>, String> {
        with_engine(|e| {
            let st = &e.lua_nodes;
            let t = step(st, ud_handle(&s), c.as_ref().map(ud_handle), glyph);
            (t != 0).then(|| (ud(t), int(st.node(t).f[0]), int(st.node(t).f[1])))
        })
    });
    nat!(lua, n, "trav_list_ud", |s: NodeUd, c: Option<NodeUd>| -> Result<Option<(UdValue, i64, i64, UdValue)>, String> {
        with_engine(|e| {
            let st = &e.lua_nodes;
            let t = step(st, ud_handle(&s), c.as_ref().map(ud_handle), list);
            (t != 0).then(|| {
                let nd = st.node(t);
                (ud(t), int(nd.id), int(nd.subtype), ud_or_nil(list_head(nd)))
            })
        })
    });
    nat!(lua, n, "trav_id_ud", |id: ToInteger, s: NodeUd, c: Option<NodeUd>| -> Result<Option<(UdValue, i64)>, String> {
        let id = id.0 as i32;
        with_engine(|e| {
            let st = &e.lua_nodes;
            let t = step(st, ud_handle(&s), c.as_ref().map(ud_handle), |nd| i32::from(nd.id) == id);
            (t != 0).then(|| (ud(t), int(st.subtype(t))))
        })
    });
    let ids = lua.create_table().map_err(|e| format!("{e:?}"))?;
    for id in 0..PREBUILT_IDS {
        nat!(lua, ids, i64::from(id), move |s: NodeUd, c: Option<NodeUd>| -> Result<Option<(UdValue, i64)>, String> {
            with_engine(|e| {
                let st = &e.lua_nodes;
                let t = step(st, ud_handle(&s), c.as_ref().map(ud_handle), |nd| nd.id == id);
                (t != 0).then(|| (ud(t), int(st.subtype(t))))
            })
        });
    }
    n.set("trav_ids_ud", ids).map_err(|e| format!("trav_ids_ud: {e:?}"))?;
    Ok(())
}
