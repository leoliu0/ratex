//! The node-dependent part of LuaTeX's `tex` library (ltexlib.c): box
//! registers as nodes, the node lists of the page builder and the semantic
//! nest, `tex.linebreak`, `tex.shipout`, `tex.splitbox` and the main
//! control entry points.
//!
//! The engine keeps its lists as `Vec<Node>` while Lua sees linked nodes of
//! [`crate::lua_node`]. Where LuaTeX hands Lua a pointer into the live list
//! (`tex.box[n]`, `tex.nest[i].head`, `tex.lists.page_head`, ...) the list
//! is imported once and *linked* to its place in the engine: Lua edits
//! the imported nodes and [`Engine::lua_sync_links`] writes a changed list
//! back when the Lua call ends (and before TeX runs again), so that
//! `tex.getbox(n)` followed by in-place edits behaves as it does in LuaTeX.

use tex_lua::{Lua, LuaApi, LuaString, LuaTable, UdValue, UserDataTrait, Variadic};

use crate::boxes::{self, Glue, Node};
use crate::engine::{Engine, Mode};
use crate::eqtb::Equiv;
use crate::lua_bridge::{bytes_of, with_engine};
use crate::lua_node::{HLIST, TEMP, VLIST};
use crate::lua_node_lib::NodeUd;
use crate::prim::{DimParam, GlueParam, IntParam};

macro_rules! reg {
    ($lua:expr, $tbl:expr, $name:literal, $f:expr) => {
        $tbl.set($name, $lua.create_function($f).map_err(|e| format!("{}: {e:?}", $name))?)
            .map_err(|e| format!("{}: {e:?}", $name))?
    };
}

/// What a link of Lua nodes stands for in the engine.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Target {
    /// box register
    Box(u16),
    /// the list of the current semantic nest level
    Cur,
    /// the list of an enclosing nest level
    Saved(usize),
    /// `contrib_head`: the contribution list of the page builder
    Contrib,
    /// `page_head`: the material already on the current page
    PageHead,
}

/// A list imported for Lua whose edits go back to the engine.
#[derive(Debug)]
pub(crate) struct Link {
    pub target: Target,
    /// A box register: the box node. Lists: the `temp` head node whose
    /// `next` is the first node (what `tex.nest[i].head` is in LuaTeX).
    pub head: u32,
    /// `tex.nest[i].tail = n`: the last node of the list (0: the end of
    /// the chain).
    pub tail: u32,
    /// Fingerprint of the nodes at the last synchronisation.
    pub fp: u64,
}

/// Lists LuaTeX has but the engine does not keep as lists: what Lua stored
/// with `tex.setlist` stays there (`temp_head`, `hold_head`, ...).
const SCRATCH_LISTS: [&str; 8] = [
    "page_ins_head",
    "temp_head",
    "hold_head",
    "adjust_head",
    "pre_adjust_head",
    "align_head",
    "page_discards_head",
    "split_discards_head",
];

fn luatex_mode(m: Mode) -> i64 {
    match m {
        Mode::Vertical => 1,
        Mode::InternalVertical => -1,
        Mode::Horizontal => 134,
        Mode::RestrictedHorizontal => -134,
        Mode::DisplayMath => 267,
        Mode::Math => -267,
    }
}

fn engine_mode(v: i64) -> Option<Mode> {
    Some(match v {
        1 => Mode::Vertical,
        -1 => Mode::InternalVertical,
        134 => Mode::Horizontal,
        -134 => Mode::RestrictedHorizontal,
        267 => Mode::DisplayMath,
        -267 => Mode::Math,
        _ => return None,
    })
}

impl Engine {
    // ------------------------------------------------------------ lists

    fn lua_level_mode(&self, j: usize) -> Mode {
        if j >= self.saved_lists.len() { self.mode } else { self.saved_lists[j].0 }
    }

    /// The engine place of nest level `j`.
    fn lua_level_target(&self, j: usize) -> Target {
        if self.lua_level_mode(j) == Mode::Vertical {
            Target::Contrib
        } else if j >= self.saved_lists.len() {
            Target::Cur
        } else {
            Target::Saved(j)
        }
    }

    fn lua_target_list(&self, t: Target) -> Vec<Node> {
        let processed = self.page_processed.min(self.page_list.len());
        match t {
            Target::Box(k) => self.eqtb.boxed.get(usize::from(k)).cloned().flatten().into_iter().collect(),
            Target::Cur => self.cur_list.clone(),
            Target::Saved(j) => self.saved_lists.get(j).map(|f| f.1.clone()).unwrap_or_default(),
            Target::Contrib => self.page_list[processed..].to_vec(),
            Target::PageHead => self.page_list[..processed].to_vec(),
        }
    }

    fn lua_set_target_list(&mut self, t: Target, list: Vec<Node>) {
        let processed = self.page_processed.min(self.page_list.len());
        match t {
            Target::Box(k) => {
                let node = list.into_iter().next();
                self.eqtb.replace_box_value(k, node);
            }
            Target::Cur => self.cur_list = list,
            Target::Saved(j) => {
                if let Some(f) = self.saved_lists.get_mut(j) {
                    f.1 = list;
                }
            }
            Target::Contrib => {
                self.page_list.truncate(processed);
                self.page_list.extend(list);
            }
            Target::PageHead => {
                let rest = self.page_list.split_off(processed);
                self.page_processed = list.len();
                self.page_list = list;
                self.page_list.extend(rest);
            }
        }
    }

    fn lua_link_index(&self, t: Target) -> Option<usize> {
        self.lua_tex.links.iter().position(|l| l.target == t && self.lua_nodes.valid(l.head))
    }

    /// The head of the linked list of `t`, importing it on first use.
    pub(crate) fn lua_list_link(&mut self, t: Target) -> u32 {
        if let Some(i) = self.lua_link_index(t) {
            return self.lua_tex.links[i].head;
        }
        self.lua_tex.links.retain(|l| l.target != t);
        let list = self.lua_target_list(t);
        let first = self.lua_nodes_from_engine(list) as u32;
        self.lua_attach_list(t, first)
    }

    /// Make the nodes at `first` the list of `t` (without exporting them).
    fn lua_attach_list(&mut self, t: Target, first: u32) -> u32 {
        self.lua_tex.links.retain(|l| l.target != t);
        let head = self.lua_new_node(TEMP, 0);
        self.lua_nodes.node_mut(head).next = first;
        let fp = self.lua_nodes.fingerprint(first, true);
        self.lua_tex.links.push(Link { target: t, head, tail: 0, fp });
        head
    }

    /// `n` and the nodes after it as a list at `t`; the engine takes a copy.
    fn lua_assign_list(&mut self, t: Target, first: u32) {
        let copy = self.lua_nodes.copy_list(first);
        let list = self.lua_nodes_to_engine(i64::from(copy));
        self.lua_set_target_list(t, list);
        self.lua_attach_list(t, first);
    }

    /// The last node of the linked list `head` (the head when empty).
    fn lua_link_tail(&self, i: usize) -> u32 {
        let l = &self.lua_tex.links[i];
        if l.tail != 0 && self.lua_nodes.valid(l.tail) {
            return l.tail;
        }
        self.lua_nodes.tail_of(l.head)
    }

    // ------------------------------------------------------------ boxes

    /// `tex.getbox`: the box of register `k` as a node (0: void). The node
    /// stays tied to the register.
    pub(crate) fn lua_box_handle(&mut self, k: u16) -> u32 {
        let t = Target::Box(k);
        if let Some(i) = self.lua_link_index(t) {
            return self.lua_tex.links[i].head;
        }
        self.lua_tex.links.retain(|l| l.target != t);
        let Some(b) = self.eqtb.boxed.get(usize::from(k)).cloned().flatten() else {
            return 0;
        };
        let h = self.lua_nodes_from_engine(vec![b]) as u32;
        if h != 0 {
            let fp = self.lua_nodes.fingerprint(h, false);
            self.lua_tex.links.push(Link { target: t, head: h, tail: 0, fp });
        }
        h
    }

    /// `tex.setbox`: node `h` (0: void) becomes the box of register `k`.
    pub(crate) fn lua_set_box(&mut self, k: u16, h: u32, global: bool) {
        let t = Target::Box(k);
        self.lua_tex.links.retain(|l| l.target != t);
        let global = self.lua_tex_global(global);
        if h == 0 || !self.lua_nodes.valid(h) {
            self.eqtb.assign_box(k, None, global);
            return;
        }
        let node = self.lua_export_copy(h);
        self.eqtb.assign_box(k, node, global);
        let fp = self.lua_nodes.fingerprint(h, false);
        self.lua_tex.links.push(Link { target: t, head: h, tail: 0, fp });
    }

    /// The engine form of node `h` alone (a copy; `h` stays).
    fn lua_export_copy(&mut self, h: u32) -> Option<Node> {
        let c = self.lua_nodes.copy_node(h);
        self.lua_nodes_to_engine(i64::from(c)).into_iter().next()
    }

    /// Write the lists Lua changed back to the engine. `last` ends all links
    /// (the outermost Lua call ended, or TeX is about to run).
    pub(crate) fn lua_sync_links(&mut self, last: bool) {
        if self.lua_tex.links.is_empty() {
            return;
        }
        let mut links = std::mem::take(&mut self.lua_tex.links);
        links.retain(|l| self.lua_nodes.valid(l.head));
        for l in &mut links {
            match l.target {
                Target::Box(k) => {
                    let fp = self.lua_nodes.fingerprint(l.head, false);
                    if fp != l.fp {
                        if matches!(self.lua_nodes.id(l.head), HLIST | VLIST) {
                            let node = self.lua_export_copy(l.head);
                            self.eqtb.replace_box_value(k, node);
                        }
                        l.fp = fp;
                    }
                }
                t => {
                    let first = self.lua_nodes.next(l.head);
                    let fp = self.lua_nodes.fingerprint(first, true) ^ u64::from(l.tail);
                    if fp != l.fp {
                        let end = if l.tail != 0 && self.lua_nodes.valid(l.tail) { self.lua_nodes.next(l.tail) } else { 0 };
                        let copy = self.lua_nodes.copy_range(first, end);
                        let list = self.lua_nodes_to_engine(i64::from(copy));
                        self.lua_set_target_list(t, list);
                        l.fp = fp;
                    }
                }
            }
        }
        if !last {
            self.lua_tex.links = links;
        }
    }

    // ----------------------------------------------------------- shipout

    pub(crate) fn lua_shipout(&mut self, k: u16) {
        self.lua_sync_links(true);
        let b = self.eqtb.boxed.get_mut(usize::from(k)).and_then(Option::take);
        self.ship_box(b);
    }

    /// `tex.splitbox(k, h, mode)`: `vsplit` of box register `k`; the
    /// remainder stays in the register. 0 when nothing was split off.
    pub(crate) fn lua_splitbox(&mut self, k: u16, h: i32, exactly: bool) -> u32 {
        self.lua_sync_links(true);
        for m in &mut self.marks[3..5] {
            m.clear();
        }
        let Some(b) = self.eqtb.boxed.get(usize::from(k)).cloned().flatten() else {
            return 0;
        };
        if !matches!(&b, Node::Box { kind, .. } if *kind != boxes::HBOX) {
            self.error("\\vsplit needs a \\vbox");
            return 0;
        }
        let b = self.eqtb.take_box(k).unwrap_or(b);
        self.vsplat_remainder = None;
        let top = self.vsplit_box(b, h);
        if let Some(rest) = self.vsplat_remainder.take() {
            self.stash_vsplit_remainder(k, rest);
        }
        let Some(mut top) = top else { return 0 };
        if !exactly {
            if let Node::Box { list, .. } = top {
                let md = self.eqtb.dim_params[DimParam::BoxMaxDepth.idx() as usize];
                top = boxes::vpack_add_md(list, None, false, boxes::VBOX, &self.eqtb, md).node;
            }
        }
        self.lua_nodes_from_engine(vec![top]) as u32
    }

    // --------------------------------------------------------- linebreak

    /// `tex.linebreak(head, params)`: break the paragraph `head` with the
    /// values of `p` standing in for the TeX parameters. Returns the lines,
    /// the demerits, the actual looseness, `prevdepth` and `prevgraf`.
    pub(crate) fn lua_linebreak(&mut self, head: u32, p: &LuaTable) -> Result<(u32, i64, i64, i64, i64), String> {
        let int = |name: &str| p.raw_get::<Option<i64>>(name).ok().flatten();
        let ints = [
            ("pretolerance", IntParam::Pretolerance),
            ("tracingparagraphs", IntParam::TracingParagraphs),
            ("tolerance", IntParam::Tolerance),
            ("looseness", IntParam::Looseness),
            ("adjustspacing", IntParam::PdfAdjustSpacing),
            ("adjdemerits", IntParam::AdjDemerits),
            ("protrudechars", IntParam::PdfProtrudeChars),
            ("linepenalty", IntParam::LinePenalty),
            ("lastlinefit", IntParam::LastLineFit),
            ("doublehyphendemerits", IntParam::DoubleHyphenDemerits),
            ("finalhyphendemerits", IntParam::FinalHyphenDemerits),
            ("hangafter", IntParam::HangAfter),
            ("interlinepenalty", IntParam::InterLinePenalty),
            ("widowpenalty", IntParam::WidowPenalty),
            ("clubpenalty", IntParam::ClubPenalty),
            ("brokenpenalty", IntParam::BrokenPenalty),
        ];
        let dims = [
            ("emergencystretch", DimParam::EmergencyStretch),
            ("hangindent", DimParam::HangIndent),
            ("hsize", DimParam::HSize),
        ];
        let glues = [("leftskip", GlueParam::LeftSkip), ("rightskip", GlueParam::RightSkip)];
        let saved_ints: Vec<(IntParam, i32)> =
            ints.iter().map(|(_, ip)| (*ip, self.eqtb.int_params[ip.idx() as usize])).collect();
        let saved_dims: Vec<(DimParam, i32)> =
            dims.iter().map(|(_, dp)| (*dp, self.eqtb.dim_params[dp.idx() as usize])).collect();
        let saved_glues: Vec<(GlueParam, Glue)> =
            glues.iter().map(|(_, gp)| (*gp, self.eqtb.glue_params[gp.idx() as usize].clone())).collect();
        let saved_shape = self.par_shape.clone();
        let saved_shapes = self.penalty_shapes.clone();
        let saved_mode = self.mode;
        let saved_pg = self.prev_graf;
        let saved_pd = self.prev_depth;

        for (name, ip) in ints {
            if let Some(v) = int(name) {
                self.eqtb.int_params[ip.idx() as usize] = v as i32;
            }
        }
        for (name, dp) in dims {
            if let Some(v) = int(name) {
                self.eqtb.dim_params[dp.idx() as usize] = v as i32;
            }
        }
        for (name, gp) in glues {
            if let Ok(Some(t)) = p.raw_get::<Option<LuaTable>>(name) {
                let v = |i: i64| t.raw_geti::<Option<i64>>(i).ok().flatten().unwrap_or(0) as i32;
                let g = Glue::spec(v(1), v(2), v(4).clamp(0, 3) as u8, v(3), v(5).clamp(0, 3) as u8).eqtb_value();
                self.eqtb.glue_params[gp.idx() as usize] = g;
            }
        }
        if let Ok(Some(t)) = p.raw_get::<Option<LuaTable>>("parshape") {
            let n = t.raw_len().unwrap_or(0);
            self.par_shape = (1..=n as i64)
                .step_by(2)
                .map(|i| {
                    let v = |i: i64| t.raw_geti::<Option<i64>>(i).ok().flatten().unwrap_or(0) as i32;
                    (v(i), v(i + 1))
                })
                .collect();
        }
        for (i, name) in ["interlinepenalties", "clubpenalties", "widowpenalties"].into_iter().enumerate() {
            if let Ok(Some(t)) = p.raw_get::<Option<LuaTable>>(name) {
                let n = t.raw_len().unwrap_or(0);
                let v: Vec<i32> =
                    (1..=n as i64).map(|i| t.raw_geti::<Option<i64>>(i).ok().flatten().unwrap_or(0) as i32).collect();
                self.penalty_shapes[i] = std::rc::Rc::from(v);
            }
        }

        // a new nest level: internal vertical mode, nothing on it yet
        self.mode = Mode::InternalVertical;
        self.prev_graf = 0;
        self.prev_depth = self.ignore_depth();
        let widow = self.eqtb.int_params[IntParam::WidowPenalty.idx() as usize];
        let list = self.lua_nodes_to_engine(i64::from(head));
        let probe = (self.eqtb.int_params[IntParam::Looseness.idx() as usize] != 0).then(|| list.clone());
        // the engine breaks a list that ends in its \parfillskip glue
        let (lines, record) = self.break_paragraph_with_record(list, widow, false);

        let (mut depth, mut graf) = (self.ignore_depth(), 0i64);
        let mut out = Vec::new();
        if let Node::Box { list, .. } = lines {
            for n in &list {
                if let Node::Box { kind, d, .. } = n {
                    if *kind == boxes::HBOX {
                        graf += 1;
                        depth = *d;
                    }
                }
            }
            out = list;
        }
        let first = self.lua_nodes_from_engine(out) as u32;

        let looseness = self.eqtb.int_params[IntParam::Looseness.idx() as usize];
        let actual = match probe {
            Some(list) if looseness != 0 => self.lua_actual_looseness(list, widow, record.lines),
            _ => 0,
        };

        for (ip, v) in saved_ints {
            self.eqtb.int_params[ip.idx() as usize] = v;
        }
        for (dp, v) in saved_dims {
            self.eqtb.dim_params[dp.idx() as usize] = v;
        }
        for (gp, g) in saved_glues {
            self.eqtb.glue_params[gp.idx() as usize] = g;
        }
        self.par_shape = saved_shape;
        self.penalty_shapes = saved_shapes;
        self.mode = saved_mode;
        self.prev_graf = saved_pg;
        self.prev_depth = saved_pd;
        Ok((first, record.demerits, actual, i64::from(depth), graf))
    }

    /// The line difference TeX reports as `actual_looseness`: the lines of
    /// the chosen solution minus those of the optimum without `\looseness`.
    fn lua_actual_looseness(&mut self, list: Vec<Node>, widow: i32, lines: usize) -> i64 {
        let li = IntParam::Looseness.idx() as usize;
        let ti = IntParam::TracingParagraphs.idx() as usize;
        let (saved_l, saved_t) = (self.eqtb.int_params[li], self.eqtb.int_params[ti]);
        let saved_layout = self.last_paragraph_layout.clone();
        self.eqtb.int_params[li] = 0;
        self.eqtb.int_params[ti] = 0;
        let (_, base) = self.break_paragraph_with_record(list, widow, false);
        self.eqtb.int_params[li] = saved_l;
        self.eqtb.int_params[ti] = saved_t;
        self.last_paragraph_layout = saved_layout;
        lines as i64 - base.lines as i64
    }

    // ------------------------------------------------------ main control

    /// `tex.show_context`: the line of input being read, as TeX shows it.
    pub(crate) fn lua_show_context(&mut self) {
        let Some(ctx) = self.input.current_source_context() else { return };
        let split = ctx.display_column.saturating_sub(1).min(ctx.text.len());
        let (done, rest) = ctx.text.split_at(ctx.text.floor_char_boundary(split));
        let head = format!("l.{} {}", ctx.line, done);
        let indent = " ".repeat(head.chars().count());
        self.lua_texio_print(0, true, head.as_bytes());
        self.lua_texio_print(0, true, format!("{indent}{rest}").as_bytes());
    }

    /// `token.scan_list` (`local_scan_box`): scan a box specification
    /// (`\hbox{...}`, `\box n`, `\copy n`), running TeX until the box is
    /// complete.
    pub(crate) fn lua_scan_list(&mut self) -> Option<Node> {
        use crate::prim::Prim;
        let saved = self.save_scanner();
        self.skip_spaces_relax();
        let t = self.get_token();
        let mut result = None;
        let prim = if t.is_cs() {
            match self.eqtb.resolve(t.cs_id()) {
                Some(Equiv::Prim(p)) => Some(p.box_spec()),
                _ => None,
            }
        } else {
            None
        };
        match prim {
            Some(p @ (Prim::HBox | Prim::VBox | Prim::VTop | Prim::VCenter)) => {
                let kind = match p {
                    Prim::HBox => 0,
                    Prim::VBox => 1,
                    Prim::VTop => 2,
                    _ => 3,
                };
                self.lua_tex.scan_depth = Some(self.box_kinds.len());
                self.begin_box(kind);
                while !self.end_occurred && self.lua_tex.scan_result.is_none() {
                    let t = self.get_token();
                    if t == crate::input::EOF_MARKER {
                        break;
                    }
                    self.dispatch(t);
                }
                self.lua_tex.scan_depth = None;
                result = self.lua_tex.scan_result.take();
            }
            Some(Prim::Box) => {
                let idx = self.scan_reg_num();
                result = self.eqtb.take_box(idx);
            }
            Some(Prim::Copy) => {
                let idx = self.scan_reg_num();
                result = self.eqtb.boxed.get(usize::from(idx)).cloned().flatten();
            }
            _ => {
                self.push_token(t);
                self.error("A <box> was supposed to be here");
            }
        }
        self.restore_scanner(saved);
        result
    }

    /// `\saveboxresource` for Lua (`pdf_create_obj(obj_type_xform)`): an
    /// XObject form of `node`; returns its object number.
    fn lua_save_box_resource(&mut self, node: Node, attr: String, resources: String, immediate: bool, margin: i32) -> i32 {
        let size = match &node {
            Node::Box { w, h, d, .. } => (*w, *h, *d),
            _ => (0, 0, 0),
        };
        let obj = self.alloc_pdf_obj();
        self.pdf_last_xform = obj;
        self.pdf_xform_count += 1;
        self.pdf_doc.form_names.insert(obj, self.pdf_xform_count);
        self.pdf_xforms.insert(obj, size);
        self.pdf_pending_forms
            .insert(obj, crate::engine::PendingForm { node: Some(node), size, attr, resources });
        self.lua_tex.xform_margin.insert(obj, margin);
        if immediate {
            self.ship_pdf_form(obj);
        }
        obj
    }
}

// ------------------------------------------------------------ nest userdata

/// `tex.getnest(n)`: a view of one semantic nest level (`luatex.nest`).
pub(crate) struct NestUd {
    pub level: usize,
}

fn opt_node(h: u32) -> UdValue {
    if h == 0 { UdValue::Nil } else { UdValue::from_userdata(NodeUd { h }) }
}

impl UserDataTrait for NestUd {
    fn type_name(&self) -> &'static str {
        "userdata"
    }

    fn get_field(&self, key: &str) -> Option<UdValue> {
        let j = self.level;
        let r = with_engine(|e| {
            let top = j >= e.saved_lists.len();
            Some(match key {
                "mode" => UdValue::Integer(luatex_mode(e.lua_level_mode(j))),
                "head" => {
                    let t = e.lua_level_target(j);
                    opt_node(e.lua_list_link(t))
                }
                "tail" => {
                    let t = e.lua_level_target(j);
                    e.lua_list_link(t);
                    let i = e.lua_link_index(t)?;
                    opt_node(e.lua_link_tail(i))
                }
                "prevgraf" => UdValue::Integer(i64::from(if top { e.prev_graf } else { e.saved_lists[j].4 })),
                "modeline" => UdValue::Integer(i64::from(if j == 0 {
                    0
                } else if top {
                    e.mode_line()
                } else {
                    e.saved_lists[j - 1].5
                })),
                "prevdepth" => UdValue::Integer(i64::from(if top { e.prev_depth } else { e.saved_lists[j].2 })),
                "spacefactor" => UdValue::Integer(i64::from(if top { e.space_factor } else { e.saved_lists[j].3 })),
                "mathdir" => UdValue::Boolean(false),
                "mathstyle" => UdValue::Integer(-1),
                "noad" | "delimptr" | "dirs" => UdValue::Nil,
                _ => UdValue::Nil,
            })
        });
        Some(r.ok().flatten().unwrap_or(UdValue::Nil))
    }

    fn set_field(&mut self, key: &str, value: UdValue) -> Option<Result<(), String>> {
        let j = self.level;
        let int = match &value {
            UdValue::Integer(i) => *i,
            UdValue::Number(n) => (*n + 0.5).floor() as i64,
            _ => 0,
        };
        let node = match &value {
            UdValue::Handle(h) => *h as u32,
            _ => 0,
        };
        let r = with_engine(|e| {
            let top = j >= e.saved_lists.len();
            match key {
                "mode" => {
                    if let Some(m) = engine_mode(int) {
                        if top {
                            e.mode = m;
                        } else {
                            e.saved_lists[j].0 = m;
                        }
                    }
                }
                "head" => {
                    if e.lua_nodes.valid(node) {
                        let t = e.lua_level_target(j);
                        e.lua_list_link(t);
                        if let Some(i) = e.lua_link_index(t) {
                            e.lua_tex.links[i].head = node;
                        }
                    }
                }
                "tail" => {
                    if e.lua_nodes.valid(node) {
                        let t = e.lua_level_target(j);
                        e.lua_list_link(t);
                        if let Some(i) = e.lua_link_index(t) {
                            e.lua_tex.links[i].tail = node;
                        }
                    }
                }
                "prevgraf" => {
                    if top {
                        e.prev_graf = int as i32;
                    } else {
                        e.saved_lists[j].4 = int as i32;
                    }
                }
                "modeline" => {
                    if j > 0 && !top {
                        e.saved_lists[j - 1].5 = int as i32;
                    }
                }
                "prevdepth" => {
                    if top {
                        e.prev_depth = int as i32;
                    } else {
                        e.saved_lists[j].2 = int as i32;
                    }
                }
                "spacefactor" => {
                    if top {
                        e.space_factor = int as i32;
                    } else {
                        e.saved_lists[j].3 = int as i32;
                    }
                }
                _ => {}
            }
        });
        r.ok();
        Some(Ok(()))
    }

    fn lua_tostring(&self) -> Option<String> {
        Some(format!("luatex.nest: {:#x}", 0x5000_0000usize + self.level))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

// ------------------------------------------------------------------ natives

fn box_register(k: i64, what: &str) -> Result<u16, String> {
    u16::try_from(k).map_err(|_| format!("incorrect index specification for tex.{what}()"))
}

pub(crate) fn install(lua: &mut Lua, t: &LuaTable) -> Result<(), String> {
    // a box register named by \chardef (what LuaTeX's get_box_id accepts)
    reg!(lua, t, "box_index", |name: LuaString| -> Result<Option<i64>, String> {
        let name = bytes_of(&name);
        with_engine(|e| {
            let id = e.cs.lookup(&name)?;
            match e.eqtb.resolve(id)? {
                Equiv::BoxReg(i) => Some(i64::from(*i)),
                Equiv::CharDef(v) => Some(i64::from(*v)),
                Equiv::MathCharDef(v) => Some(i64::from(*v)),
                _ => None,
            }
        })
    });
    reg!(lua, t, "box_get", |k: i64| -> Result<Option<i64>, String> {
        let k = box_register(k, "getbox")?;
        with_engine(|e| {
            let h = e.lua_box_handle(k);
            (h != 0).then_some(i64::from(h))
        })
    });
    reg!(lua, t, "box_set", |k: i64, h: Option<i64>, global: bool| -> Result<(), String> {
        let k = box_register(k, "setbox")?;
        with_engine(|e| e.lua_set_box(k, h.unwrap_or(0) as u32, global))
    });
    reg!(lua, t, "shipout", |k: i64| -> Result<(), String> {
        let k = box_register(k, "shipout")?;
        with_engine(|e| e.lua_shipout(k))
    });
    reg!(lua, t, "splitbox", |k: i64, h: i64, exactly: bool| -> Result<Option<i64>, String> {
        let k = box_register(k, "splitbox")?;
        with_engine(|e| {
            let n = e.lua_splitbox(k, h as i32, exactly);
            (n != 0).then_some(i64::from(n))
        })
    });

    // ---- lists ----
    reg!(lua, t, "list_get", |name: LuaString| -> Result<Option<i64>, String> {
        let name = bytes_of(&name);
        with_engine(|e| {
            let list_first = |e: &mut Engine, t: Target| {
                let head = e.lua_list_link(t);
                let first = e.lua_nodes.next(head);
                (first != 0).then_some(i64::from(first))
            };
            match name.as_slice() {
                b"page_head" => list_first(e, Target::PageHead),
                b"contrib_head" => list_first(e, Target::Contrib),
                b"best_page_break" => {
                    let idx = e.page_best_break?;
                    let processed = e.page_processed.min(e.page_list.len());
                    let (t, steps) = if idx < processed { (Target::PageHead, idx) } else { (Target::Contrib, idx - processed) };
                    let mut n = list_first(e, t)? as u32;
                    for _ in 0..steps {
                        n = e.lua_nodes.next(n);
                    }
                    (n != 0).then_some(i64::from(n))
                }
                b"least_page_cost" => Some(e.page_best_cost),
                b"best_size" => Some(if e.page_best_break.is_some() { e.page_best_goal } else { 0 }),
                other => {
                    let key = std::str::from_utf8(other).ok()?;
                    let h = e.lua_tex.scratch.iter().find(|(n, _)| n == key).map(|(_, h)| *h)?;
                    e.lua_nodes.valid(h).then_some(i64::from(h))
                }
            }
        })
    });
    reg!(lua, t, "list_set", |name: LuaString, v: Option<i64>| -> Result<(), String> {
        let name = bytes_of(&name);
        let v = v.unwrap_or(0);
        with_engine(|e| {
            let n = if e.lua_nodes.valid(v as u32) { v as u32 } else { 0 };
            match name.as_slice() {
                b"best_size" => e.page_best_goal = v,
                b"least_page_cost" => e.page_best_cost = v,
                b"best_page_break" => {
                    let processed = e.page_processed.min(e.page_list.len());
                    let find = |e: &mut Engine, t: Target, base: usize| -> Option<usize> {
                        let head = e.lua_list_link(t);
                        let mut c = e.lua_nodes.next(head);
                        let mut i = 0;
                        while c != 0 {
                            if c == n {
                                return Some(base + i);
                            }
                            c = e.lua_nodes.next(c);
                            i += 1;
                        }
                        None
                    };
                    e.page_best_break =
                        if n == 0 { None } else { find(e, Target::PageHead, 0).or_else(|| find(e, Target::Contrib, processed)) };
                }
                b"page_head" => e.lua_assign_list(Target::PageHead, n),
                b"contrib_head" => e.lua_assign_list(Target::Contrib, n),
                other => {
                    let Ok(key) = std::str::from_utf8(other) else { return };
                    if !SCRATCH_LISTS.contains(&key) {
                        return;
                    }
                    e.lua_tex.scratch.retain(|(k, _)| k != key);
                    if n != 0 {
                        e.lua_tex.scratch.push((key.to_string(), n));
                    }
                }
            }
        })
    });

    // ---- nest ----
    reg!(lua, t, "nest_ptr", || -> Result<i64, String> { with_engine(|e| e.saved_lists.len() as i64) });
    reg!(lua, t, "nest_get", |level: i64| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| {
            let ok = usize::try_from(level).is_ok_and(|l| l <= e.saved_lists.len());
            Variadic(vec![if ok { UdValue::from_userdata(NestUd { level: level as usize }) } else { UdValue::Nil }])
        })
    });

    // ---- main control ----
    reg!(lua, t, "run", || -> Result<(), String> {
        with_engine(|e| {
            e.lua_sync_links(true);
            e.main_loop();
        })
    });
    reg!(lua, t, "finish", || -> Result<(), String> {
        with_engine(|e| {
            e.lua_sync_links(true);
            e.explicit_end_seen = true;
            e.end_occurred = true;
        })
    });
    reg!(lua, t, "show_context", || -> Result<(), String> { with_engine(Engine::lua_show_context) });
    reg!(lua, t, "linebreak", |head: i64, params: LuaTable| -> Result<(i64, i64, i64, i64, i64), String> {
        with_engine(|e| {
            let (first, demerits, looseness, depth, graf) = e.lua_linebreak(head as u32, &params)?;
            Ok((i64::from(first), demerits, looseness, depth, graf))
        })?
    });

    // ---- token.scan_glue / token.scan_list ----
    reg!(lua, t, "scan_glue", |mu: bool| -> Result<(i64, i64, i64, i64, i64), String> {
        with_engine(|e| {
            let saved = e.save_scanner();
            let g = e.scan_glue(mu);
            e.restore_scanner(saved);
            (
                i64::from(g.width),
                i64::from(g.stretch),
                i64::from(g.shrink),
                i64::from(g.stretch_order),
                i64::from(g.shrink_order),
            )
        })
    });
    reg!(lua, t, "scan_list", || -> Result<Option<i64>, String> {
        with_engine(|e| {
            let b = e.lua_scan_list()?;
            let h = e.lua_nodes_from_engine(vec![b]);
            Some(h)
        })
        .map(|h| h.filter(|&h| h != 0))
    });

    // ---- tex.getmath / tex.setmath / tex.permitmathobsolete ----
    reg!(lua, t, "math_get", |i: i64, j: i64| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| {
            let (i, j) = (i as u32, j as u8);
            if i < crate::uprim::mp::MATH_PARAM_FIRST_MU_GLUE {
                return Variadic(vec![UdValue::Integer(i64::from(e.eqtb.math_param(i, j)))]);
            }
            if e.eqtb.math_glue_param(i, j).is_none() {
                return Variadic(vec![UdValue::Nil]);
            }
            let g = e.math_glue_value(i, j);
            Variadic(
                [g.width, g.stretch, g.shrink, i32::from(g.stretch_order), i32::from(g.shrink_order)]
                    .into_iter()
                    .map(|v| UdValue::Integer(i64::from(v)))
                    .collect(),
            )
        })
    });
    reg!(lua, t, "math_set", |i: i64, j: i64, v: i64, global: bool| -> Result<(), String> {
        with_engine(|e| e.eqtb.assign_math_param(i as u32, j as u8, v as i32, global))
    });
    reg!(lua, t, "math_set_glue", |i: i64, j: i64, w: i64, st: i64, sh: i64, so: i64, sho: i64, global: bool| -> Result<(), String> {
        with_engine(|e| {
            let value = [0, w as i32, st as i32, sh as i32, so as i32, sho as i32];
            e.eqtb.assign_math_glue_param(i as u32, j as u8, value, global)
        })
    });
    reg!(lua, t, "permit_math_obsolete", |on: bool| -> Result<(), String> {
        with_engine(|e| {
            e.lua_tex.permit_math_obsolete = on;
            e.warning_at(&format!("(math): obsolete commands are {}", if on { "permitted" } else { "blocked" }), None);
        })
    });

    // ---- box resources ----
    reg!(lua, t, "box_resource_save", |what: i64, is_node: bool, attr: Option<LuaString>, res: Option<LuaString>, immediate: bool, margin: Option<i64>| -> Result<i64, String> {
        let attr = attr.as_ref().map(bytes_of).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let res = res.as_ref().map(bytes_of).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        with_engine(|e| {
            let node = if is_node {
                let h = what as u32;
                if !matches!(e.lua_nodes.id(h), HLIST | VLIST) {
                    e.fatal_error("error:  (pdf backend): xforms can only be used with a box or [h|v]list");
                    return 0;
                }
                e.lua_export_copy(h)
            } else {
                let k = u16::try_from(what).unwrap_or(0);
                e.lua_sync_links(true);
                e.eqtb.boxed.get_mut(usize::from(k)).and_then(Option::take)
            };
            let Some(node) = node else {
                e.fatal_error("error:  (pdf backend): xforms cannot be used with a void box or empty [h|v]list");
                return 0;
            };
            let margin = margin.map_or_else(|| e.eqtb.dim_params[DimParam::PdfXFormMargin.idx() as usize], |m| m as i32);
            i64::from(e.lua_save_box_resource(node, attr, res, immediate, margin))
        })
    });
    reg!(lua, t, "box_resource_dimensions", |index: i64| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| {
            let Some((w, h, d)) = e.pdf_xforms.get(&(index as i32)).copied() else {
                return Variadic(vec![UdValue::Nil]);
            };
            let margin = e.lua_tex.xform_margin.get(&(index as i32)).copied().unwrap_or(0);
            Variadic([w, h, d, margin].into_iter().map(|v| UdValue::Integer(i64::from(v))).collect())
        })
    });
    reg!(lua, t, "box_resource_box", |index: i64| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let node = e.pdf_pending_forms.get(&(index as i32))?.node.clone()?;
            let h = e.lua_nodes_from_engine(vec![node]);
            (h != 0).then_some(h)
        })
    });
    Ok(())
}
