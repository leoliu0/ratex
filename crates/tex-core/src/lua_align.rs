//! LuaTeX's `fin_row`: a finished `\halign` row passes the text passes and
//! `hpack_filter` (group `fin_row`) as an unset hlist and is handed to
//! `append_to_vlist_filter` with the location `alignment` (`align.c`,
//! `packaging.c`). Lua sees the row as luatex builds it: the initial tabskip
//! glue, then one unset node per cell followed by the tabskip glue after the
//! cell's last column.

use crate::align::{Cell, RowCtl};
use crate::boxes::{Attr, Node, NodeList};
use crate::engine::{Engine, EngineKind};
use crate::lua_callbacks::{Cb, CbArg, CbRet};
use crate::lua_node::{HLIST, UNSET};

impl Engine {
    /// Whether any callback `fin_row` runs is registered.
    fn lua_fin_row_wanted(&self) -> bool {
        self.engine_kind == EngineKind::LuaTeX
            && [Cb::Hyphenate, Cb::Ligaturing, Cb::Kerning, Cb::HpackFilter, Cb::AppendToVlistFilter]
                .into_iter()
                .any(|cb| self.cb_defined(cb))
    }

    /// The row `row` of the current `\halign` is complete: run luatex's
    /// `fin_row` callbacks on it. The result tells the alignment whether the
    /// callback took over `append_to_vlist` (no interline glue), dropped the
    /// row, and which `prev_depth` it asked for. A callback that changes the
    /// row's nodes is not followed: the row stays as TeX built it.
    pub(crate) fn lua_fin_row(&mut self, row: &[Cell]) -> RowCtl {
        if !self.lua_fin_row_wanted() {
            return RowCtl::default();
        }
        let mut image: NodeList = Vec::with_capacity(2 * row.len() + 1);
        image.push(Node::Glue(self.align_col_tabskip_start(), Attr::NONE));
        for (c, cell) in row.iter().enumerate() {
            let Some(packed) = &cell.packed else { continue };
            image.push(packed.clone());
            image.push(Node::Glue(self.align_col_tabskip(c + usize::from(cell.span)), Attr::NONE));
        }
        let (head, tail) = self.lua_list_with_head(image);
        let mut p = self.lua_nodes.next(head);
        while p != 0 {
            if self.lua_nodes.node(p).id == HLIST {
                self.lua_nodes.node_mut(p).id = UNSET;
            }
            p = self.lua_nodes.next(p);
        }
        self.lua_text_passes_on(head, tail);
        let mut first = self.lua_nodes.next(head);
        if first != 0 {
            self.lua_nodes.node_mut(head).next = 0;
            self.lua_nodes.node_mut(first).prev = 0;
        }
        self.lua_nodes.flush_node(head);
        if first != 0 && self.cb_defined(Cb::HpackFilter) {
            first = self.lua_pack_filter_list(Cb::HpackFilter, "hpack filter", "fin_row", 0, false, None, None, first);
        }
        let (b, _) = self.lua_pack_list(first, 0, true, true);
        if b == 0 {
            return RowCtl::default();
        }
        let mut ctl = RowCtl::default();
        if self.cb_defined(Cb::AppendToVlistFilter) {
            let depth = self.lua_align_running_depth();
            let args = vec![CbArg::Node(b), CbArg::str("alignment"), CbArg::Int(i64::from(depth)), CbArg::Bool(false)];
            if let Some(rets) = self.lua_cb_call(Cb::AppendToVlistFilter, "append to vlist", args) {
                ctl.handled = true;
                ctl.keep = match rets.first() {
                    Some(CbRet::Node(_)) => true,
                    Some(CbRet::Nil) | None => false,
                    Some(_) => {
                        self.warning_at("(append to vlist): error: node or nil expected", None);
                        false
                    }
                };
                ctl.depth = match rets.get(1) {
                    Some(CbRet::Int(d)) => Some(*d as i32),
                    Some(CbRet::Num(d)) => Some(d.round() as i32),
                    _ => None,
                };
            }
        }
        self.lua_nodes.flush_list(b);
        ctl
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
}
