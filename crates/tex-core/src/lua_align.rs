//! LuaTeX's `fin_row`: a finished alignment row passes the text passes and
//! `hpack_filter` (group `fin_row`) as an unset hlist and is handed to
//! `append_to_vlist_filter` with the location `alignment` (`align.c`,
//! `packaging.c`). A `\valign` row is a `vpack_filter` (group `fin_row`,
//! maximum depth `\maxdepth`) and joins the horizontal list directly. Lua
//! sees the row as luatex builds it: the initial tabskip glue, then one unset
//! node per cell followed by the tabskip glue after the cell's last column.
//!
//! What Lua leaves behind is followed: the nodes `append_to_vlist_filter`
//! returns, with the row (the node it was handed) wherever it ended up, are
//! what the alignment's list holds; `fin_align` (`finish_halign`) sets the
//! unset cells of that row from the final column widths and leaves every
//! other node alone.

use crate::align::{Cell, RowBody, RowCtl, RowEntry, RowItem, RowRepl, UnsetCell};
use crate::boxes::{Attr, Node, NodeList};
use crate::engine::{Engine, EngineKind};
use crate::lua_callbacks::{Cb, CbArg, CbRet};
use crate::lua_node::{HLIST, UNSET, VLIST};
use crate::lua_node_conv::sl;
use crate::lua_node_pack::{engine_order, lua_order_of};
use crate::prim::DimParam;

impl Engine {
    /// Whether any callback `fin_row` runs is registered.
    fn lua_fin_row_wanted(&self) -> bool {
        if self.engine_kind != EngineKind::LuaTeX {
            return false;
        }
        if self.align_is_valign {
            return self.cb_defined(Cb::VpackFilter);
        }
        [Cb::Hyphenate, Cb::Ligaturing, Cb::Kerning, Cb::HpackFilter, Cb::AppendToVlistFilter]
            .into_iter()
            .any(|cb| self.cb_defined(cb))
    }

    /// The row `row` of the current alignment is complete: run luatex's
    /// `fin_row` callbacks on it. The result tells the alignment which nodes
    /// Lua left for the row, whether `append_to_vlist_filter` took over
    /// `append_to_vlist` (no interline glue), and which `prev_depth` it asked
    /// for.
    pub(crate) fn lua_fin_row(&mut self, row: &[Cell]) -> RowCtl {
        if !self.lua_fin_row_wanted() {
            return RowCtl::default();
        }
        let valign = self.align_is_valign;
        let mut image: NodeList = Vec::with_capacity(2 * row.len() + 1);
        image.push(Node::Glue(self.align_col_tabskip_start(), Attr::NONE));
        let mut cells: Vec<&Cell> = Vec::with_capacity(row.len());
        for (c, cell) in row.iter().enumerate() {
            let Some(packed) = &cell.packed else { continue };
            image.push(packed.clone());
            image.push(Node::Glue(self.align_col_tabskip(c + usize::from(cell.span)), Attr::NONE));
            cells.push(cell);
        }
        let (head, tail) = self.lua_list_with_head(image);
        // `fin_col` made every cell an unset node: its subtype is the span
        // count and the glue totals sit in the order/sign fields
        let mut cells = cells.into_iter();
        let mut p = self.lua_nodes.next(head);
        while p != 0 {
            if matches!(self.lua_nodes.node(p).id, HLIST | VLIST) {
                if let Some(cell) = cells.next() {
                    self.lua_mark_unset(p, cell);
                }
            }
            p = self.lua_nodes.next(p);
        }
        if !valign {
            self.lua_text_passes_on(head, tail);
        }
        let mut first = self.lua_nodes.next(head);
        if first != 0 {
            self.lua_nodes.node_mut(head).next = 0;
            self.lua_nodes.node_mut(first).prev = 0;
        }
        self.lua_nodes.flush_node(head);
        let max_depth = self.eqtb.dim_params[DimParam::MaxDepth.idx() as usize];
        if valign {
            if first != 0 {
                first = self.lua_pack_filter_list(
                    Cb::VpackFilter,
                    "vpack filter",
                    "fin_row",
                    0,
                    false,
                    Some(max_depth),
                    None,
                    first,
                );
            }
        } else if first != 0 && self.cb_defined(Cb::HpackFilter) {
            first = self.lua_pack_filter_list(Cb::HpackFilter, "hpack filter", "fin_row", 0, false, None, None, first);
        }
        let (b, _) = if valign {
            self.lua_pack_list_md(first, 0, true, false, max_depth)
        } else {
            self.lua_pack_list(first, 0, true, true)
        };
        if b == 0 {
            return RowCtl::default();
        }
        let mut ctl = RowCtl::default();
        // what the alignment's list receives: the row, or what the callback
        // made of it
        let mut result = b;
        if !valign && self.cb_defined(Cb::AppendToVlistFilter) {
            let depth = self.lua_align_running_depth();
            let args = vec![CbArg::Node(b), CbArg::str("alignment"), CbArg::Int(i64::from(depth)), CbArg::Bool(false)];
            if let Some(rets) = self.lua_cb_call(Cb::AppendToVlistFilter, "append to vlist", args) {
                ctl.handled = true;
                result = match rets.first() {
                    Some(CbRet::Node(h)) => *h,
                    Some(CbRet::Nil) | None => 0,
                    Some(_) => {
                        self.warning_at("(append to vlist): error: node or nil expected", None);
                        0
                    }
                };
                ctl.depth = match rets.get(1) {
                    Some(CbRet::Int(d)) => Some(*d as i32),
                    Some(CbRet::Num(d)) => Some(d.round() as i32),
                    _ => None,
                };
            }
        }
        let mut items = Vec::new();
        let mut row_found = false;
        let mut n = result;
        while self.lua_nodes.valid(n) {
            let next = self.lua_nodes.next(n);
            if n == b {
                row_found = true;
                let body = self.lua_row_body(b, valign);
                items.push(RowItem::Row(body));
            } else {
                items.extend(self.lua_take_node(n).into_iter().map(RowItem::Node));
            }
            n = next;
        }
        if !row_found {
            self.lua_nodes.flush_node(b);
        }
        ctl.repl = Some(Box::new(RowRepl { items }));
        ctl
    }

    /// The unset node `fin_col` makes of the packed cell `cell` (`p`).
    fn lua_mark_unset(&mut self, p: u32, cell: &Cell) {
        let Some(Node::Box { list, .. }) = &cell.packed else { return };
        let (stretch, shrink) = crate::boxes::glue_sums(list);
        let top = |v: &[i64; 4]| (0..4).rev().find(|&o| v[o] != 0).unwrap_or(0);
        let (so, ho) = (top(&stretch), top(&shrink));
        let nd = self.lua_nodes.node_mut(p);
        nd.id = UNSET;
        nd.subtype = cell.span;
        nd.f[sl::B_ORDER] = lua_order_of(so as u8);
        nd.f[sl::B_SIGN] = lua_order_of(ho as u8);
        // `glue_shrink` shares the shift slot (luatex's `glue_shrink =
        // shift_amount`); the list moves one slot further
        nd.f[sl::B_SHIFT] = shrink[ho] as i32;
        nd.f[sl::U_STRETCH] = stretch[so] as i32;
        nd.f[sl::U_HEAD] = nd.f[sl::B_HEAD];
        nd.f[sl::B_HEAD] = 0;
    }

    /// The engine form of the single node `n`, detached from its neighbours.
    fn lua_take_node(&mut self, n: u32) -> NodeList {
        let next = self.lua_nodes.next(n);
        let prev = self.lua_nodes.prev(n);
        self.lua_nodes.node_mut(n).next = 0;
        self.lua_nodes.node_mut(n).prev = 0;
        if next != 0 {
            self.lua_nodes.node_mut(next).prev = 0;
        }
        if prev != 0 {
            self.lua_nodes.node_mut(prev).next = 0;
        }
        self.lua_nodes_to_engine(i64::from(n))
    }

    /// The row box `b` as `fin_align` sees it: its dimensions after Lua's
    /// edits and its list, the unset cells with the glue totals Lua left.
    fn lua_row_body(&mut self, b: u32, valign: bool) -> RowBody {
        let f = self.lua_nodes.node(b).f;
        self.lua_nodes.node_mut(b).f[sl::B_HEAD] = 0;
        self.lua_nodes.flush_node(b);
        let mut list = Vec::new();
        let mut n = f[sl::B_HEAD] as u32;
        while self.lua_nodes.valid(n) {
            let next = self.lua_nodes.next(n);
            if self.lua_nodes.id(n) == UNSET {
                let nd = self.lua_nodes.node_mut(n);
                let cell = (
                    nd.subtype,
                    engine_order(nd.f[sl::B_ORDER]),
                    i64::from(nd.f[sl::U_STRETCH]),
                    engine_order(nd.f[sl::B_SIGN]),
                    i64::from(nd.f[sl::B_SHIFT]),
                );
                // back to a box for the export, which sets it at its natural size
                nd.id = if valign { VLIST } else { HLIST };
                nd.subtype = 0;
                nd.f[sl::B_HEAD] = nd.f[sl::U_HEAD];
                nd.f[sl::U_HEAD] = 0;
                nd.f[sl::B_ORDER] = 0;
                nd.f[sl::B_SIGN] = 0;
                nd.f[sl::B_SHIFT] = 0;
                nd.f[sl::U_STRETCH] = 0;
                let node = self.lua_take_node(n).into_iter().next();
                if let Some(node) = node {
                    list.push(RowEntry::Unset(UnsetCell {
                        node,
                        span: cell.0,
                        stretch_order: cell.1,
                        stretch: cell.2,
                        shrink_order: cell.3,
                        shrink: cell.4,
                    }));
                }
            } else {
                list.extend(self.lua_take_node(n).into_iter().map(RowEntry::Node));
            }
            n = next;
        }
        RowBody { w: f[sl::B_WIDTH], h: f[sl::B_HEIGHT], d: f[sl::B_DEPTH], list }
    }

    /// `prev_depth` at the start of a `\\noalign` when Lua shaped the rows:
    /// the depth `append_to_vlist_filter` last asked for, else the depth of
    /// the row Lua left (`None` when TeX's own rule applies).
    pub(crate) fn lua_align_noalign_depth(&self) -> Option<i32> {
        if self.engine_kind != EngineKind::LuaTeX || self.align_is_valign {
            return None;
        }
        let handled = |r: &Vec<Cell>| r.first().is_some_and(|c| c.ctl.handled);
        if self.align_rows.iter().any(handled) {
            return Some(self.lua_align_running_depth());
        }
        let last = self.align_rows.last()?;
        let repl = last.first()?.ctl.repl.as_deref()?;
        repl.items.iter().find_map(|item| match item {
            RowItem::Row(body) => Some(body.d),
            RowItem::Node(_) => None,
        })
    }

    /// `prev_depth` as the alignment's vertical list has it after the rows
    /// finished so far: the last depth a callback chose, the depth a
    /// `\noalign` left, else the one of the enclosing list.
    fn lua_align_running_depth(&self) -> i32 {
        for row in self.align_rows.iter().rev() {
            let Some(first) = row.first() else { continue };
            if row.len() == 1 && first.span == crate::align::NOALIGN_SPAN {
                if let Some(Node::Box { shift, .. }) = &first.packed {
                    return *shift;
                }
            } else if let Some(d) = first.ctl.depth {
                return d;
            }
        }
        self.prev_depth
    }

    /// luatex `fin_align` for `\valign`: the preamble (tabskip glue and one
    /// unset node per column, `height` the column width) goes to
    /// `vpack_filter` with the group `preamble`.
    pub(crate) fn lua_preamble_filter(&mut self, preamble: NodeList, size: i32, exactly: bool) -> NodeList {
        if preamble.is_empty() || !self.cb_defined(Cb::VpackFilter) {
            return preamble;
        }
        let mut first = self.lua_nodes_from_engine(preamble) as u32;
        let mut p = first;
        while p != 0 {
            if self.lua_nodes.node(p).id == VLIST {
                self.lua_nodes.node_mut(p).id = UNSET;
            }
            p = self.lua_nodes.next(p);
        }
        let max_depth = self.eqtb.dim_params[DimParam::MaxDepth.idx() as usize];
        first = self.lua_pack_filter_list(Cb::VpackFilter, "vpack filter", "preamble", size, exactly, Some(max_depth), None, first);
        let mut p = first;
        while p != 0 {
            if self.lua_nodes.node(p).id == UNSET {
                self.lua_nodes.node_mut(p).id = VLIST;
            }
            p = self.lua_nodes.next(p);
        }
        self.lua_nodes_to_engine(i64::from(first))
    }
}
