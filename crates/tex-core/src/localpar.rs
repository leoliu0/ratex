//! LuaTeX `local_par` nodes and the primitives that make them
//! (`\localleftbox`, `\localrightbox`, `\localinterlinepenalty`,
//! `\localbrokenpenalty`), plus the per-group counters
//! (`\nolocalwhatsits`, `\nolocaldirs`) that decide which groups append
//! a node or a cancelling dir node when they end (maincontrol.c
//! `fixup_directions`).

use crate::boxes::{LocalPar, LocalParMode, Node, NodeList, WhatIt};
use crate::engine::{Engine, EngineKind, Mode};
use crate::eqtb::{group_code, GroupMeta, LevelType, LOCAL_LEFT_BOX, LOCAL_RIGHT_BOX};
use crate::lua_callbacks::{Cb, CbArg, GROUP_LOCAL_BOX};
use crate::prim::IntParam;


impl Engine {
    fn int_param(&self, p: IntParam) -> i32 {
        self.eqtb.int_params[p.idx() as usize]
    }

    /// texnodes.c `make_local_par_node` without the callback: the node
    /// carries the penalties, the current local boxes (copies) and the
    /// paragraph direction of the moment.
    fn local_par_data(&self) -> LocalPar {
        let boxed = |idx: u16| match &self.eqtb.boxed[usize::from(idx)] {
            Some(b @ Node::Box { w, .. }) => (vec![b.clone()], *w),
            _ => (Vec::new(), 0),
        };
        let (left, left_width) = boxed(LOCAL_LEFT_BOX);
        let (right, right_width) = boxed(LOCAL_RIGHT_BOX);
        LocalPar {
            pen_inter: self.int_param(IntParam::LocalInterLinePenalty),
            pen_broken: self.int_param(IntParam::LocalBrokenPenalty),
            dir: (self.int_param(IntParam::ParDirection) & 3) as u8,
            left,
            left_width,
            right,
            right_width,
        }
    }

    /// texnodes.c `make_local_par_node(mode)`: the node, after the
    /// `insert_local_par` callback has had a look at (and may have changed) it.
    pub(crate) fn make_local_par(&mut self, mode: LocalParMode) -> Node {
        let mut node = Node::Whatsit(WhatIt::LocalPar(Box::new(self.local_par_data())), self.eqtb.cur_attr);
        if self.cb_defined(Cb::InsertLocalPar) {
            let head = self.lua_nodes_from_engine(vec![node.clone()]) as u32;
            let _ = self.lua_cb_call(
                Cb::InsertLocalPar,
                "insert_local_par",
                vec![CbArg::Node(head), CbArg::str(mode.name())],
            );
            let mut back = self.lua_nodes_to_engine(i64::from(head));
            if matches!(back.first(), Some(Node::Whatsit(WhatIt::LocalPar(_), _))) {
                node = back.remove(0);
            }
        }
        node
    }

    /// `tail_append(make_local_par_node(mode))`
    pub(crate) fn append_local_par(&mut self, mode: LocalParMode) {
        let node = self.make_local_par(mode);
        self.cur_list.push(node);
    }

    /// `\localinterlinepenalty`/`\localbrokenpenalty` was assigned in
    /// horizontal mode: the new values reach `line_break` through a
    /// `local_par` node, and the group knows it appended one.
    #[inline(always)]
    pub(crate) fn local_penalty_assigned(&mut self, ip: IntParam) {
        if matches!(ip, IntParam::LocalInterLinePenalty | IntParam::LocalBrokenPenalty) {
            self.local_penalty_assigned_lua();
        }
    }

    #[cold]
    fn local_penalty_assigned_lua(&mut self) {
        if self.engine_kind == EngineKind::LuaTeX && self.mode.is_h() {
            self.append_local_par(LocalParMode::Penalty);
            self.count_local_whatsit();
        }
    }

    fn count_local_whatsit(&mut self) {
        let n = self.int_param(IntParam::NoLocalWhatsits) + 1;
        self.eqtb.assign_int_param(IntParam::NoLocalWhatsits, n, false);
    }

    /// `run_left_brace`/`run_begin_group`: the new group has appended
    /// neither node yet.
    #[inline(always)]
    pub(crate) fn reset_local_counters(&mut self) {
        if self.engine_kind == EngineKind::LuaTeX {
            self.eqtb.assign_int_param(IntParam::NoLocalWhatsits, 0, false);
            self.eqtb.assign_int_param(IntParam::NoLocalDirs, 0, false);
        }
    }

    /// packaging.c `scan_full_spec`: a new box group starts counting its own
    /// dir changes, and its `text_dir_ptr` is a fresh list holding the
    /// direction of the enclosing mode (restored by [`Self::end_box_dirs`]).
    #[inline(always)]
    pub(crate) fn begin_box_dirs(&mut self) {
        if self.engine_kind == EngineKind::LuaTeX {
            self.begin_box_dirs_lua();
        }
    }

    fn begin_box_dirs_lua(&mut self) {
        self.eqtb.assign_int_param(IntParam::NoLocalDirs, 0, false);
        let param = match self.mode {
            Mode::Vertical | Mode::InternalVertical => IntParam::BodyDirection,
            Mode::Horizontal | Mode::RestrictedHorizontal => IntParam::TextDirection,
            _ => IntParam::MathDirection,
        };
        let dir = (self.int_param(param) & 3) as u8;
        // created before the box group opens: one level below the group
        let level = self.eqtb.cur_level - 1;
        let outer = std::mem::replace(&mut self.text_dirs, vec![(level, dir)]);
        self.text_dir_saves.push(outer);
    }

    /// packaging.c `package`: "adjust back |text_dir_ptr| for |scan_spec|"
    #[inline(always)]
    pub(crate) fn end_box_dirs(&mut self) {
        if self.engine_kind == EngineKind::LuaTeX {
            if let Some(outer) = self.text_dir_saves.pop() {
                self.text_dirs = outer;
            }
        }
    }

    /// maincontrol.c `new_graf`: the `local_par` node, the `\parindent`
    /// box and the dir nodes of the directions still in force.
    pub(crate) fn lua_paragraph_start(&mut self, indent: bool) {
        let node = self.make_local_par(LocalParMode::NewGraf);
        self.cur_list.push(node);
        let par_dir = (self.int_param(IntParam::ParDirection) & 3) as u8;
        // the dir nodes of the directions still in force follow the
        // local_par node, outermost first, and precede the \parindent box
        let attr = self.eqtb.cur_attr;
        for (i, &(_, dir)) in self.text_dirs.iter().enumerate() {
            if i != 0 || dir != par_dir {
                self.cur_list.push(Node::Whatsit(WhatIt::Dir { dir, cancel: false, level: 0 }, attr));
            }
        }
        if indent {
            let pi = self.eqtb.dim_params[crate::prim::DimParam::ParIndent.idx() as usize];
            let mut r = crate::boxes::hpack(Vec::new(), Some(pi), crate::boxes::HBOX, &self.eqtb).node;
            if let Node::Box { dir, subtype, .. } = &mut r {
                *dir = par_dir;
                *subtype = crate::boxes::list_subtype::INDENT;
            }
            self.cur_list.push(r);
        }
    }

    /// maincontrol.c `fixup_directions`: close a simple or semi-simple
    /// group. In horizontal mode the dir change of the group is cancelled
    /// and a `local_par` node restores the paragraph-local state.
    #[inline(always)]
    pub(crate) fn pop_group_fixup(&mut self) -> LevelType {
        if self.engine_kind != EngineKind::LuaTeX {
            return self.pop_group();
        }
        self.pop_group_fixup_lua()
    }

    /// luatex removes the `\textdir` entry of the group about to end from
    /// `text_dir_ptr` in `fixup_directions`, `fixup_directions_only` and
    /// when an output routine ends - not in `unsave`, so a box group leaves
    /// its entry behind unless `\fixupboxesmode` is on.
    pub(crate) fn pop_text_dir(&mut self) {
        let level = self.eqtb.cur_level;
        if self.engine_kind == EngineKind::LuaTeX
            && self.text_dirs.len() > 1
            && self.text_dirs.last().is_some_and(|&(l, _)| l == level)
        {
            self.text_dirs.pop();
        }
    }

    fn pop_group_fixup_lua(&mut self) -> LevelType {
        self.pop_text_dir();
        let whatsits = self.int_param(IntParam::NoLocalWhatsits);
        let dirs = self.int_param(IntParam::NoLocalDirs);
        let inner_dir = self.int_param(IntParam::TextDirection) as u8;
        let level = self.pop_group();
        if self.mode.is_h() {
            if dirs != 0 {
                let attr = self.eqtb.cur_attr;
                self.cur_list
                    .push(Node::Whatsit(WhatIt::Dir { dir: inner_dir, cancel: true, level: 0 }, attr));
            }
            if whatsits != 0 {
                self.append_local_par(LocalParMode::HmodePar);
            }
        }
        level
    }

    /// maincontrol.c `append_local_box`: `\localleftbox{...}` (`right` is
    /// false) or `\localrightbox{...}`.
    pub(crate) fn append_local_box(&mut self, right: bool) {
        self.flush_native_text();
        self.skip_spaces_relax();
        let t = self.get_x_raw();
        if !self.token_is_left_brace(t) {
            self.error("Missing { inserted");
            self.push_token(t);
        }
        self.saved_lists.push((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
            self.nest_line(),
        ));
        self.push_group_level_coded(LevelType::Box, GroupMeta::new(group_code::LOCAL_BOX));
        self.box_targets.push(None);
        self.box_shifts.push(i32::from(right));
        self.box_kinds.push(crate::build::LOCAL_BOX_KIND);
        self.mode = Mode::RestrictedHorizontal;
        self.space_factor = 1000;
    }

    /// maincontrol.c `build_local_box`: the group's material is packed
    /// into an hbox that becomes the (group-local) value of the local box.
    pub(crate) fn build_local_box(&mut self, right: bool, inner: NodeList, outer_mode: Mode) {
        let packed = if inner.is_empty() {
            None
        } else {
            let list = self.lua_text_passes(inner);
            let list = self.lua_hpack_filter(GROUP_LOCAL_BOX, 0, false, list);
            Some(crate::boxes::hpack(list, None, crate::boxes::HBOX, &self.eqtb).node)
        };
        let slot = if right { LOCAL_RIGHT_BOX } else { LOCAL_LEFT_BOX };
        self.eqtb.assign_local_box(slot, packed);
        if outer_mode.is_h() {
            self.append_local_par(LocalParMode::LocalBox);
        }
        self.count_local_whatsit();
    }

    /// `fixupboxesmode`: a box that changed the text direction ends it
    /// before it is packaged (maincontrol.c `fixup_directions_only`).
    #[inline(always)]
    pub(crate) fn fixup_box_directions(&mut self) {
        if self.int_param(IntParam::FixupBoxesMode) != 0 {
            self.pop_text_dir();
            if self.int_param(IntParam::NoLocalDirs) != 0 {
                let dir = self.int_param(IntParam::TextDirection) as u8;
                let attr = self.eqtb.cur_attr;
                self.cur_list.push(Node::Whatsit(WhatIt::Dir { dir, cancel: true, level: 0 }, attr));
            }
        }
    }

    /// Whether `list` holds nothing but `local_par` and dir nodes (the
    /// paragraph is then dropped, maincontrol.c `only_dirs`).
    pub(crate) fn only_par_nodes(list: &[Node]) -> bool {
        list.iter().all(|n| {
            matches!(n, Node::Whatsit(WhatIt::LocalPar(_) | WhatIt::Dir { .. }, _))
        })
    }

}
