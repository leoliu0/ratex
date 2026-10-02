//! LuaTeX callbacks as the engine sees them (`lcallbacklib.c`, `luanode.c`):
//! which callbacks Lua has registered, how a callback is called with its
//! argument list, and the node-list filters that hand engine lists to Lua
//! and take what Lua returns.
//!
//! A callback that is not registered costs one byte load: the engine keeps
//! `callback_set` ([`Engine::lua_cb`]) and Lua's `callback.register` updates
//! it through `B.callback_set`. `0` is "not defined", a positive value "a
//! function is registered" and `-1` "registered as `false`" (the built-in
//! behaviour is switched off where luatex honours that).

use tex_lua::{LuaApi, UdValue, Value, Variadic};

use crate::boxes::{Node, NodeList};
use crate::engine::Engine;
use crate::lua_node_lib::NodeUd;

macro_rules! callbacks {
    ($($variant:ident = $name:literal,)*) => {
        /// The callbacks of `callback.list()`, in luatex's order (Lua's
        /// id is the index plus one).
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u8)]
        #[allow(dead_code)]
        pub(crate) enum Cb { $($variant,)* }

        pub(crate) const CALLBACK_NAMES: &[&str] = &[$($name,)*];
    };
}

callbacks! {
    FindWriteFile = "find_write_file",
    FindOutputFile = "find_output_file",
    FindImageFile = "find_image_file",
    FindFormatFile = "find_format_file",
    FindReadFile = "find_read_file",
    OpenReadFile = "open_read_file",
    FindVfFile = "find_vf_file",
    ReadVfFile = "read_vf_file",
    FindDataFile = "find_data_file",
    ReadDataFile = "read_data_file",
    FindFontFile = "find_font_file",
    ReadFontFile = "read_font_file",
    FindMapFile = "find_map_file",
    ReadMapFile = "read_map_file",
    FindEncFile = "find_enc_file",
    ReadEncFile = "read_enc_file",
    FindType1File = "find_type1_file",
    ReadType1File = "read_type1_file",
    FindTruetypeFile = "find_truetype_file",
    ReadTruetypeFile = "read_truetype_file",
    FindOpentypeFile = "find_opentype_file",
    ReadOpentypeFile = "read_opentype_file",
    FindCidmapFile = "find_cidmap_file",
    ReadCidmapFile = "read_cidmap_file",
    FindPkFile = "find_pk_file",
    ReadPkFile = "read_pk_file",
    ShowErrorHook = "show_error_hook",
    ProcessInputBuffer = "process_input_buffer",
    ProcessOutputBuffer = "process_output_buffer",
    ProcessJobname = "process_jobname",
    StartPageNumber = "start_page_number",
    StopPageNumber = "stop_page_number",
    StartRun = "start_run",
    StopRun = "stop_run",
    DefineFont = "define_font",
    PreOutputFilter = "pre_output_filter",
    BuildpageFilter = "buildpage_filter",
    HpackFilter = "hpack_filter",
    VpackFilter = "vpack_filter",
    GlyphNotFound = "glyph_not_found",
    GlyphInfo = "glyph_info",
    Hyphenate = "hyphenate",
    Ligaturing = "ligaturing",
    Kerning = "kerning",
    PreLinebreakFilter = "pre_linebreak_filter",
    LinebreakFilter = "linebreak_filter",
    PostLinebreakFilter = "post_linebreak_filter",
    AppendToVlistFilter = "append_to_vlist_filter",
    MlistToHlist = "mlist_to_hlist",
    FinishPdffile = "finish_pdffile",
    FinishPdfpage = "finish_pdfpage",
    PreDump = "pre_dump",
    StartFile = "start_file",
    StopFile = "stop_file",
    ShowErrorMessage = "show_error_message",
    ShowLuaErrorHook = "show_lua_error_hook",
    ShowIgnoredErrorMessage = "show_ignored_error_message",
    ShowWarningMessage = "show_warning_message",
    HpackQuality = "hpack_quality",
    VpackQuality = "vpack_quality",
    ProcessRule = "process_rule",
    InsertLocalPar = "insert_local_par",
    ContributeFilter = "contribute_filter",
    // luatex runs `call_edit(file, line)` only when `E` is typed at the
    // `?` prompt of an error in error_stop_mode (errors.c error()). The
    // engine never reads the terminal, so no error can reach that answer:
    // the callback can be registered and is never run.
    CallEdit = "call_edit",
    BuildPageInsert = "build_page_insert",
    GlyphStreamProvider = "glyph_stream_provider",
    FontDescriptorObjnumProvider = "font_descriptor_objnum_provider",
    FinishSynctex = "finish_synctex",
    WrapupRun = "wrapup_run",
    NewGraf = "new_graf",
    PageOrderIndex = "page_order_index",
    MakeExtensible = "make_extensible",
    ProcessPdfImageContent = "process_pdf_image_content",
    ProvideCharprocData = "provide_charproc_data",
    InputLevelString = "input_level_string",
}

/// Number of callbacks.
pub(crate) const N_CALLBACKS: usize = CALLBACK_NAMES.len();

/// luatex group names (`lua_push_group_code`), indexed by group code.
pub(crate) const GROUP_NAMES: [&str; 18] = [
    "", "simple", "hbox", "adjusted_hbox", "vbox", "vtop", "align", "no_align", "output", "math", "disc", "insert",
    "vcenter", "math_choice", "semi_simple", "math_shift", "math_left", "local_box",
];

pub(crate) const GROUP_BOTTOM: &str = GROUP_NAMES[0];
pub(crate) const GROUP_HBOX: &str = GROUP_NAMES[2];
pub(crate) const GROUP_ADJUSTED_HBOX: &str = GROUP_NAMES[3];
pub(crate) const GROUP_VBOX: &str = GROUP_NAMES[4];
pub(crate) const GROUP_VTOP: &str = GROUP_NAMES[5];
pub(crate) const GROUP_ALIGN: &str = GROUP_NAMES[6];
pub(crate) const GROUP_OUTPUT: &str = GROUP_NAMES[8];
pub(crate) const GROUP_INSERT: &str = GROUP_NAMES[11];
pub(crate) const GROUP_MATH_SHIFT: &str = GROUP_NAMES[15];
pub(crate) const GROUP_LOCAL_BOX: &str = GROUP_NAMES[17];

/// An argument of a callback call.
#[derive(Clone, Debug)]
pub(crate) enum CbArg {
    Nil,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(Vec<u8>),
    /// a node handle, handed over as a `node` userdata
    Node(u32),
}

impl CbArg {
    pub(crate) fn str(s: &str) -> CbArg {
        CbArg::Str(s.as_bytes().to_vec())
    }
}

/// A value a callback returned.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CbRet {
    Nil,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(Vec<u8>),
    Node(u32),
    /// any other Lua type (its name)
    Other(String),
}

impl CbRet {
    pub(crate) fn from_value(v: &Value) -> CbRet {
        if v.is_nil() {
            CbRet::Nil
        } else if let Some(b) = v.as_boolean() {
            CbRet::Bool(b)
        } else if let Some(h) = v.as_userdata::<NodeUd>().and_then(|u| u.borrow().ok().map(|b| b.h)) {
            CbRet::Node(h)
        } else if let Some(i) = v.as_integer() {
            CbRet::Int(i)
        } else if let Some(n) = v.as_number() {
            CbRet::Num(n)
        } else if let Some(s) = crate::lua_node_lib::value_bytes(v) {
            CbRet::Str(s)
        } else {
            CbRet::Other(v.type_name().to_string())
        }
    }

    /// `lua_type` name, for luatex's "callback should return ..." messages.
    pub(crate) fn type_name(&self) -> &str {
        match self {
            CbRet::Nil => "nil",
            CbRet::Bool(_) => "boolean",
            CbRet::Int(_) | CbRet::Num(_) => "number",
            CbRet::Str(_) => "string",
            CbRet::Node(_) => "userdata",
            CbRet::Other(t) => t,
        }
    }
}

/// The outcome of a call: `None` when no function is registered.
pub(crate) type CbResult = Result<Option<Vec<CbRet>>, String>;

impl crate::engine_lua::LuaEngine {
    /// Call callback `cb` with `args` and collect everything it returns.
    pub(crate) fn call_callback_values(&mut self, cb: Cb, args: Vec<CbArg>) -> CbResult {
        let f = self.lua_callback_fn(CALLBACK_NAMES[cb as usize])?;
        let Some(f) = f else {
            return Ok(None);
        };
        let args: Vec<UdValue> = args
            .into_iter()
            .map(|a| match a {
                CbArg::Nil => UdValue::Nil,
                CbArg::Bool(b) => UdValue::Boolean(b),
                CbArg::Int(i) => UdValue::Integer(i),
                CbArg::Num(n) => UdValue::Number(n),
                CbArg::Str(s) => UdValue::Bytes(s),
                CbArg::Node(0) => UdValue::Nil,
                CbArg::Node(h) => UdValue::from_userdata(NodeUd { h }),
            })
            .collect();
        let rets: Variadic<Value> = f
            .call(Variadic(args))
            .map_err(|e| self.lua.get_error_message(e).message().to_string())?;
        Ok(Some(rets.iter().map(CbRet::from_value).collect()))
    }
}

impl Engine {
    /// `callback_defined(cb)`: `0`, a positive id, or `-1` (registered as
    /// `false`).
    #[inline(always)]
    pub(crate) fn cb_state(&self, cb: Cb) -> i8 {
        self.lua_cb[cb as usize]
    }

    /// `callback_defined(cb) > 0`: a function is registered.
    #[inline(always)]
    pub(crate) fn cb_defined(&self, cb: Cb) -> bool {
        self.lua_cb[cb as usize] > 0
    }

    /// Run callback `cb`; a Lua error is reported like luatex does
    /// (`warning  (what): error: ...` and an error) and yields `None`.
    pub(crate) fn lua_cb_call(&mut self, cb: Cb, what: &str, args: Vec<CbArg>) -> Option<Vec<CbRet>> {
        match self.lua_run(|lua| lua.call_callback_values(cb, args)) {
            Ok(rets) => rets,
            Err(err) => {
                self.lua_callback_failed(what, &err);
                None
            }
        }
    }

    pub(crate) fn lua_callback_failed(&mut self, what: &str, err: &str) {
        self.lua_warning(what, &format!("error: {err}"));
        self.lua_error("", err);
    }

    /// luatex `lua_node_filter_s`: a callback that only gets a string.
    pub(crate) fn lua_node_filter_s(&mut self, cb: Cb, info: &str) {
        if !self.cb_defined(cb) {
            return;
        }
        let _ = self.lua_cb_call(cb, "node filter", vec![CbArg::str(info)]);
    }

    /// The Lua form of `list`: a `temp` head node whose `next` is the list
    /// (LuaTeX's `temp_head`), and the last node of the list (the head when
    /// the list is empty).
    pub(crate) fn lua_list_with_head(&mut self, list: NodeList) -> (u32, u32) {
        let head = self.lua_nodes.new_node(crate::lua_node::TEMP, 0, 0);
        if list.is_empty() {
            return (head, head);
        }
        let first = self.lua_nodes_from_engine(list) as u32;
        self.lua_nodes.couple(head, first);
        let tail = self.lua_nodes.tail_of(first);
        (head, tail)
    }

    /// Take the list behind a `temp` head back to the engine and free the
    /// head.
    pub(crate) fn lua_list_from_head(&mut self, head: u32) -> NodeList {
        let first = self.lua_nodes.next(head);
        if first != 0 {
            self.lua_nodes.node_mut(head).next = 0;
            self.lua_nodes.node_mut(first).prev = 0;
        }
        self.lua_nodes.flush_node(head);
        self.lua_nodes_to_engine(i64::from(first))
    }

    /// luanode.c `lua_node_filter` (`pre_linebreak_filter`,
    /// `post_linebreak_filter`): the list goes to the callback with the
    /// group name; `true` keeps it, `false` discards it, a node replaces it
    /// (nil, too).
    pub(crate) fn lua_node_filter(&mut self, cb: Cb, group: &str, list: NodeList) -> NodeList {
        if list.is_empty() || !self.cb_defined(cb) {
            return list;
        }
        let first = self.lua_nodes_from_engine(list) as u32;
        let rets = self.lua_cb_call(cb, "node filter", vec![CbArg::Node(first), CbArg::str(group)]);
        match rets.as_deref().and_then(|r| r.first()) {
            None | Some(CbRet::Bool(true)) => self.lua_nodes_to_engine(i64::from(first)),
            Some(CbRet::Bool(false)) => {
                self.lua_nodes.flush_list(first);
                Vec::new()
            }
            Some(CbRet::Node(h)) => self.lua_nodes_to_engine(i64::from(*h)),
            Some(CbRet::Nil) => Vec::new(),
            Some(other) => {
                let msg = format!("bad argument #1 (node expected, got {})", other.type_name());
                self.lua_callback_failed("node filter", &msg);
                self.lua_nodes_to_engine(i64::from(first))
            }
        }
    }

    /// luanode.c `lua_hpack_filter` / `lua_vpack_filter`: the list is handed
    /// over with the group name, size, pack type (`exactly` or `additional`)
    /// and, for vertical lists, the maximum depth. A `false` result empties
    /// the list, nil does too, a node list replaces it.
    pub(crate) fn lua_pack_filter(
        &mut self,
        cb: Cb,
        what: &str,
        group: &str,
        size: i32,
        exactly: bool,
        max_depth: Option<i32>,
        dir: Option<&str>,
        list: NodeList,
    ) -> NodeList {
        if list.is_empty() || !self.cb_defined(cb) {
            return list;
        }
        let first = self.lua_nodes_from_engine(list) as u32;
        let first = self.lua_pack_filter_list(cb, what, group, size, exactly, max_depth, dir, first);
        self.lua_nodes_to_engine(i64::from(first))
    }

    /// `lua_pack_filter` on a list that already lives in Lua (`first`, never
    /// 0): the first node of what the callback leaves (0 when it returned
    /// `false` or nil, which empties the list).
    pub(crate) fn lua_pack_filter_list(
        &mut self,
        cb: Cb,
        what: &str,
        group: &str,
        size: i32,
        exactly: bool,
        max_depth: Option<i32>,
        dir: Option<&str>,
        first: u32,
    ) -> u32 {
        let mut args = vec![
            CbArg::Node(first),
            CbArg::str(group),
            CbArg::Int(i64::from(size)),
            CbArg::str(if exactly { "exactly" } else { "additional" }),
        ];
        if let Some(d) = max_depth {
            args.push(CbArg::Int(i64::from(d)));
        }
        args.push(dir.map_or(CbArg::Nil, CbArg::str));
        args.push(CbArg::Nil);
        let rets = self.lua_cb_call(cb, what, args);
        match rets.as_deref().and_then(|r| r.first()) {
            None | Some(CbRet::Bool(true)) => first,
            Some(CbRet::Bool(false)) | Some(CbRet::Nil) => {
                if matches!(rets.as_deref().and_then(|r| r.first()), Some(CbRet::Bool(false))) {
                    self.lua_nodes.flush_list(first);
                }
                0
            }
            Some(CbRet::Node(h)) => *h,
            Some(other) => {
                let msg = format!("bad argument #1 (node expected, got {})", other.type_name());
                self.lua_callback_failed(what, &msg);
                first
            }
        }
    }

    /// `lua_hpack_filter` for the group `group`.
    pub(crate) fn lua_hpack_filter(&mut self, group: &str, size: i32, exactly: bool, list: NodeList) -> NodeList {
        self.lua_pack_filter(Cb::HpackFilter, "hpack filter", group, size, exactly, None, Some("TLT"), list)
    }

    /// `lua_vpack_filter`; the output box goes to `pre_output_filter`.
    pub(crate) fn lua_vpack_filter(
        &mut self,
        group: &str,
        size: i32,
        exactly: bool,
        max_depth: i32,
        list: NodeList,
    ) -> NodeList {
        let cb = if group == GROUP_OUTPUT { Cb::PreOutputFilter } else { Cb::VpackFilter };
        self.lua_pack_filter(cb, "vpack filter", group, size, exactly, Some(max_depth), Some("TLT"), list)
    }

    /// LuaTeX `package()` for the box kinds `\hbox` (0), `\vbox` (1) and
    /// `\vtop` (2): the text passes and `hpack_filter` for horizontal boxes,
    /// `vpack_filter` for vertical ones. `target` is the `to`/`spread`
    /// specification. An empty list is packed as it is.
    pub(crate) fn lua_pack_inner(
        &mut self,
        kind: u8,
        inner: NodeList,
        target: Option<(i32, bool)>,
        box_max_depth: i32,
        adjusted: bool,
    ) -> NodeList {
        if inner.is_empty() {
            return inner;
        }
        let (size, exactly) = match target {
            Some((d, false)) => (d, true),
            Some((d, true)) => (d, false),
            None => (0, false),
        };
        if kind == 0 {
            let list = self.lua_text_passes(inner);
            let group = if adjusted { GROUP_ADJUSTED_HBOX } else { GROUP_HBOX };
            self.lua_hpack_filter(group, size, exactly, list)
        } else {
            let group = if kind == 2 { GROUP_VTOP } else { GROUP_VBOX };
            self.lua_vpack_filter(group, size, exactly, box_max_depth, inner)
        }
    }
}

impl Engine {
    /// luatex `line_break_context`: the group the paragraph ends in
    /// (`math_shift` when a display interrupts it).
    fn lua_line_break_group(&self) -> &'static str {
        if self.in_display_init { GROUP_MATH_SHIFT } else { GROUP_NAMES[usize::from(self.lua_par_group)] }
    }
}
impl Engine {
    /// The paragraph list as Lua sees it in `pre_linebreak_filter`: the
    /// final penalty typed `linepenalty`. The head is returned; the
    /// paragraph's last node before `\parfillskip` is the penalty.
    fn lua_paragraph_to_lua(&mut self, content: NodeList) -> u32 {
        let first = self.lua_nodes_from_engine(content) as u32;
        if first != 0 {
            let tail = self.lua_nodes.tail_of(first);
            let before = self.lua_nodes.prev(tail);
            if before != 0 && self.lua_nodes.id(before) == crate::lua_node::PENALTY {
                self.lua_nodes.node_mut(before).subtype = 2;
            }
        }
        first
    }

    /// luatex `lua_node_filter(pre_linebreak_filter_callback, ...)` on the
    /// paragraph `content`.
    pub(crate) fn lua_pre_linebreak(&mut self, content: NodeList) -> NodeList {
        if content.is_empty() || !self.cb_defined(Cb::PreLinebreakFilter) {
            return content;
        }
        let group = self.lua_line_break_group();
        let first = self.lua_paragraph_to_lua(content);
        let rets = self.lua_cb_call(Cb::PreLinebreakFilter, "node filter", vec![CbArg::Node(first), CbArg::str(group)]);
        match rets.as_deref().and_then(|r| r.first()) {
            None | Some(CbRet::Bool(true)) => self.lua_nodes_to_engine(i64::from(first)),
            Some(CbRet::Bool(false)) => {
                self.lua_nodes.flush_list(first);
                Vec::new()
            }
            Some(CbRet::Node(h)) => self.lua_nodes_to_engine(i64::from(*h)),
            Some(CbRet::Nil) => Vec::new(),
            Some(other) => {
                let msg = format!("bad argument #1 (node expected, got {})", other.type_name());
                self.lua_callback_failed("node filter", &msg);
                self.lua_nodes_to_engine(i64::from(first))
            }
        }
    }

    /// luatex `lua_linebreak_callback`: Lua breaks the paragraph itself.
    /// `Ok(lines)` when the callback returned a node list (the lines),
    /// `Err(content)` when the built-in line breaker has to run.
    pub(crate) fn lua_linebreak_filter(&mut self, content: NodeList, display: bool) -> Result<NodeList, NodeList> {
        if content.is_empty() || !self.cb_defined(Cb::LinebreakFilter) {
            return Err(content);
        }
        let first = self.lua_paragraph_to_lua(content);
        let rets = self.lua_cb_call(Cb::LinebreakFilter, "linebreak", vec![CbArg::Node(first), CbArg::Bool(display)]);
        match rets.as_deref().and_then(|r| r.first()) {
            Some(CbRet::Node(h)) => Ok(self.lua_nodes_to_engine(i64::from(*h))),
            _ => Err(self.lua_nodes_to_engine(i64::from(first))),
        }
    }

    /// luatex `lua_node_filter(post_linebreak_filter_callback, ...)` on the
    /// lines of a paragraph.
    pub(crate) fn lua_post_linebreak(&mut self, lines: NodeList) -> NodeList {
        if !self.cb_defined(Cb::PostLinebreakFilter) {
            return lines;
        }
        let group = self.lua_line_break_group();
        self.lua_node_filter(Cb::PostLinebreakFilter, group, lines)
    }
}

/// One `hpack_quality` event of a paragraph line, held back until the line
/// is appended to the vertical list: luatex packs and appends line by line,
/// so the callbacks of line `k` come between those of lines `k - 1` and
/// `k + 1`.
pub(crate) struct DeferredQuality {
    ordinal: usize,
    what: String,
    value: i32,
    snapshot: Node,
    begin: i32,
    line: i32,
}

/// State of the lines of the paragraph `build_lines` is making.
#[derive(Default)]
pub(crate) struct ParLineState {
    /// `hpack_quality` calls are held back
    pub(crate) defer: bool,
    /// `end_paragraph` appends the lines itself and fires the held calls
    pub(crate) hold: bool,
    /// index of the line being packed
    pub(crate) ordinal: usize,
    pub(crate) quality: Vec<DeferredQuality>,
}

impl Engine {
    /// Hold back the `hpack_quality` call of the line being packed.
    pub(crate) fn lua_defer_pack_quality(&mut self, what: &str, value: i32, snapshot: Node, begin: i32, line: i32) {
        let ordinal = self.lua_par_lines.ordinal;
        self.lua_par_lines.quality.push(DeferredQuality { ordinal, what: what.to_string(), value, snapshot, begin, line });
    }

    /// Run the held `hpack_quality` calls of line `ordinal`; the rules they
    /// return are appended to the line box `b`.
    pub(crate) fn lua_fire_pack_quality(&mut self, ordinal: usize, b: &mut Node) {
        if self.lua_par_lines.quality.is_empty() {
            return;
        }
        let (mine, rest): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.lua_par_lines.quality).into_iter().partition(|q| q.ordinal == ordinal);
        self.lua_par_lines.quality = rest;
        for q in mine {
            if let Some(rules) = self.lua_pack_quality(true, &q.what, q.value, &q.snapshot, q.begin, q.line) {
                if let Node::Box { list, .. } = b {
                    list.extend(rules);
                }
            }
        }
    }

    /// Fire the held `hpack_quality` calls that no line append consumed
    /// (their rules are dropped with the box they belong to).
    pub(crate) fn lua_flush_pack_quality(&mut self) {
        let held = std::mem::take(&mut self.lua_par_lines.quality);
        for q in held {
            let _ = self.lua_pack_quality(true, &q.what, q.value, &q.snapshot, q.begin, q.line);
        }
    }

    /// luatex `checked_break_filter`: `contribute_filter(info)` while the
    /// lines of a paragraph are appended.
    pub(crate) fn lua_contribute_filter(&mut self, info: &str) {
        if self.cb_defined(Cb::ContributeFilter) {
            self.lua_node_filter_s(Cb::ContributeFilter, info);
        }
    }

    /// luatex `build_page_insert(n, i)`: the number of the `\skip` register
    /// used for the first insertion of class `n` on a page; `i` is the
    /// position of the class among the page's insertion classes.
    pub(crate) fn lua_build_page_insert(&mut self, n: u16, i: usize) -> u16 {
        if self.engine_kind != crate::engine::EngineKind::LuaTeX || !self.cb_defined(Cb::BuildPageInsert) {
            return n;
        }
        let rets = self.lua_cb_call(Cb::BuildPageInsert, "build_page_insert", vec![CbArg::Int(i64::from(n)), CbArg::Int(i as i64)]);
        match rets.as_deref().and_then(|r| r.first()) {
            Some(CbRet::Int(v)) => u16::try_from(*v).unwrap_or(n),
            Some(CbRet::Num(v)) => u16::try_from(*v as i64).unwrap_or(n),
            Some(other) => {
                eprintln!("callback should return a number, not: {}", other.type_name());
                n
            }
            None => n,
        }
    }
}

/// The `info` argument of `buildpage_filter` (luatex `lua_key_index`).
pub(crate) mod page_info {
    pub const VMODE_PAR: &str = "vmode_par";
    pub const HMODE_PAR: &str = "hmode_par";
    pub const INSERT: &str = "insert";
    pub const BOX: &str = "box";
    pub const NEW_GRAF: &str = "new_graf";
    pub const PENALTY: &str = "penalty";
    pub const BEFORE_DISPLAY: &str = "before_display";
    pub const AFTER_DISPLAY: &str = "after_display";
    pub const ALIGNMENT: &str = "alignment";
    pub const AFTER_OUTPUT: &str = "after_output";
    pub const END: &str = "end";
}

impl Engine {
    /// luatex `checked_page_filter` (`checked`: not while the output routine
    /// runs) and `normal_page_filter`: `buildpage_filter(info)` just before
    /// the page builder is entered.
    #[inline]
    pub(crate) fn lua_page_filter(&mut self, info: &str, checked: bool) {
        if self.lua_cb[Cb::BuildpageFilter as usize] <= 0 || (checked && self.output_depth > 0) {
            return;
        }
        self.lua_node_filter_s(Cb::BuildpageFilter, info);
    }

    /// luatex `new_graf`: `new_graf(mode, indented)` may change the
    /// indentation (`nil` counts as `false`, any other type is ignored).
    pub(crate) fn lua_new_graf(&mut self, indented: bool) -> bool {
        if !self.cb_defined(Cb::NewGraf) {
            return indented;
        }
        let mode = if self.mode == crate::engine::Mode::Vertical { 1 } else { -1 };
        let rets = self.lua_cb_call(Cb::NewGraf, "new_graf", vec![CbArg::Int(mode), CbArg::Bool(indented)]);
        match rets.as_deref().and_then(|r| r.first()) {
            Some(CbRet::Bool(b)) => *b,
            Some(CbRet::Nil) => false,
            _ => indented,
        }
    }

    /// luatex `lua_appendtovlist_callback`: `append_to_vlist_filter(box,
    /// location, prev_depth, mirrored)` returns the nodes to append and the
    /// new `prev_depth`. `Err(box)` hands the box back when no function is
    /// registered (the built-in interline glue applies).
    pub(crate) fn lua_append_to_vlist(
        &mut self,
        b: Node,
        location: &str,
        prev_depth: i32,
    ) -> Result<(NodeList, Option<i32>), Node> {
        if !self.cb_defined(Cb::AppendToVlistFilter) {
            return Err(b);
        }
        let first = self.lua_nodes_from_engine(vec![b]) as u32;
        let args = vec![
            CbArg::Node(first),
            CbArg::str(location),
            CbArg::Int(i64::from(prev_depth)),
            CbArg::Bool(false),
        ];
        let Some(rets) = self.lua_cb_call(Cb::AppendToVlistFilter, "append to vlist", args) else {
            let mut back = self.lua_nodes_to_engine(i64::from(first));
            return Err(back.remove(0));
        };
        let list = match rets.first() {
            Some(CbRet::Node(h)) => self.lua_nodes_to_engine(i64::from(*h)),
            Some(CbRet::Nil) | None => Vec::new(),
            Some(_) => {
                self.lua_warning("append to vlist", "error: node or nil expected");
                Vec::new()
            }
        };
        let depth = match rets.get(1) {
            Some(CbRet::Int(d)) => Some(*d as i32),
            Some(CbRet::Num(d)) => Some(d.round() as i32),
            _ => None,
        };
        Ok((list, depth))
    }

    /// luatex `process_input_buffer`: `line` (no end of line character) may be
    /// replaced by a string the callback returns; the result loses trailing
    /// spaces.
    pub(crate) fn lua_process_input_line(&mut self, line: &mut Vec<u8>) {
        if !self.cb_defined(Cb::ProcessInputBuffer) {
            return;
        }
        let rets = self.lua_cb_call(Cb::ProcessInputBuffer, "process_input_buffer", vec![CbArg::Str(std::mem::take(line))]);
        if let Some(CbRet::Str(s)) = rets.and_then(|r| r.into_iter().next()) {
            *line = s;
            while line.last() == Some(&b' ') {
                line.pop();
            }
        }
    }

    /// luatex `write_out`: `process_output_buffer(text)` for a line that goes
    /// to a `\write` file.
    pub(crate) fn lua_process_output_line(&mut self, raw: &[u8]) -> Option<Vec<u8>> {
        if !self.cb_defined(Cb::ProcessOutputBuffer) {
            return None;
        }
        let rets = self.lua_cb_call(Cb::ProcessOutputBuffer, "process_output_buffer", vec![CbArg::Str(raw.to_vec())]);
        match rets.and_then(|r| r.into_iter().next()) {
            Some(CbRet::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// luatex `lua_glyph_not_found_callback`: a glyph whose character the font
    /// lacks reaches the output. Without a callback `char_warning` applies.
    pub(crate) fn lua_glyph_not_found(&mut self, font: crate::tfm::FontId, c: u32) {
        if self.cb_defined(Cb::GlyphNotFound) {
            let _ = self.lua_cb_call(Cb::GlyphNotFound, "glyph not found", vec![CbArg::Int(i64::from(font)), CbArg::Int(i64::from(c))]);
            return;
        }
        if self.eqtb.int_params[crate::prim::IntParam::TracingLostChars.idx() as usize] <= 0 {
            return;
        }
        let name = self.eqtb.fonts.get(usize::from(font)).map_or_else(String::new, |f| f.tfm_name.clone());
        self.warning_at(
            &format!("Missing character: There is no {} (U+{c:04X}) in font {name}!", char::from_u32(c).unwrap_or('?')),
            None,
        );
    }

    /// luatex `hpack_quality` / `vpack_quality`: the box was badly packed
    /// (`what` is underfull, loose, tight or overfull, `value` the badness or
    /// the overshoot). Returns what the callback produced: a rule for an
    /// hbox, which hpack appends to the box; `None` when no function is
    /// registered (the message is printed).
    pub(crate) fn lua_pack_quality(&mut self, hbox: bool, what: &str, value: i32, bx: &Node, line_start: i32, line: i32) -> Option<NodeList> {
        let cb = if hbox { Cb::HpackQuality } else { Cb::VpackQuality };
        if !self.cb_defined(cb) {
            return None;
        }
        let first = self.lua_nodes_from_engine(vec![bx.clone()]) as u32;
        let args = vec![
            CbArg::str(what),
            CbArg::Int(i64::from(value)),
            CbArg::Node(first),
            CbArg::Int(i64::from(line_start.abs())),
            CbArg::Int(i64::from(line)),
        ];
        let rets = self.lua_cb_call(cb, if hbox { "hpack quality" } else { "vpack quality" }, args);
        self.lua_nodes.flush_list(first);
        let rule = match rets.as_deref().and_then(|r| r.first()) {
            Some(CbRet::Node(h)) if hbox => self.lua_nodes_to_engine(i64::from(*h)),
            _ => Vec::new(),
        };
        Some(rule)
    }

    /// luatex `start_page_number` / `stop_page_number` (`->`): whether a
    /// function replaces the built-in page number printing.
    pub(crate) fn lua_page_number_callback(&mut self, cb: Cb) -> bool {
        if !self.cb_defined(cb) {
            return false;
        }
        let _ = self.lua_cb_call(cb, "page number", Vec::new());
        true
    }

    /// luatex `finish_pdfpage(is_page)` after a page or form is written.
    pub(crate) fn lua_finish_pdfpage(&mut self, page: bool) {
        if self.cb_defined(Cb::FinishPdfpage) {
            let _ = self.lua_cb_call(Cb::FinishPdfpage, "finish pdfpage", vec![CbArg::Bool(page)]);
        }
    }

    /// luatex `finish_pdffile`, `stop_run` and `wrapup_run` (`->`): run when
    /// the output is closed.
    pub(crate) fn lua_simple_callback(&mut self, cb: Cb) {
        if self.cb_defined(cb) {
            let what = CALLBACK_NAMES[cb as usize];
            let _ = self.lua_cb_call(cb, what, Vec::new());
        }
    }

    /// luatex `--lua=FILE`: run the Lua initialization script before the job
    /// starts (callbacks such as `start_run` can only be registered here).
    pub fn run_lua_init_file(&mut self, path: &str) -> Result<(), String> {
        let code = std::fs::read(path).map_err(|e| format!("cannot read lua initialization file {path}: {e}"))?;
        let name = format!("@{path}");
        self.lua_run(|lua| lua.execute(&code, &name))
    }

    /// luatex `print_banner`: with a `start_run` callback (even one
    /// registered as `false`) the callback, not the built-in banner, goes to
    /// the terminal. Returns whether the built-in banner is replaced.
    pub fn lua_start_run(&mut self) -> bool {
        match self.cb_state(Cb::StartRun) {
            0 => false,
            s => {
                if s > 0 {
                    let _ = self.lua_cb_call(Cb::StartRun, "start_run", Vec::new());
                }
                true
            }
        }
    }

    /// luatex `page_order_index` (`"d->d"`): the location the page just
    /// written takes in the page tree, recorded for page number
    /// `total_pages`. A result that is not a number is reported and counts
    /// as 0.
    pub(crate) fn lua_page_order_index(&mut self, total_pages: usize) {
        if !self.cb_defined(Cb::PageOrderIndex) {
            return;
        }
        let rets = self.lua_cb_call(Cb::PageOrderIndex, "page order index", vec![CbArg::Int(total_pages as i64)]);
        let location = match rets.as_deref().map(|r| r.first()) {
            Some(Some(CbRet::Int(n))) => *n as i32,
            Some(Some(CbRet::Num(n))) => *n as i32,
            Some(other) => {
                eprintln!("callback should return a number, not: {}", other.map_or("nil", CbRet::type_name));
                0
            }
            None => 0,
        };
        if location != 0 {
            let pages = self.pdf_doc.pages.len();
            self.pdf_doc.page_order.resize(pages, 0);
            self.pdf_doc.page_order[pages - 1] = location;
        }
    }

    /// luatex `normal_warning(tag, text)`: with a `show_warning_message`
    /// callback the callback (reading `status.lastwarningtag` and
    /// `status.lastwarningstring`) replaces the printed warning.
    pub(crate) fn lua_warning(&mut self, tag: &str, text: &str) {
        self.lua_warning_in(None, tag, text);
    }

    /// `normal_warning` while `cur_file_name` is `file`: the warning names
    /// the file, as map file warnings do.
    pub(crate) fn lua_warning_in(&mut self, file: Option<&str>, tag: &str, text: &str) {
        if self.engine_kind != crate::engine::EngineKind::LuaTeX {
            self.warning_at(&format!("({tag}): {text}"), None);
            return;
        }
        if self.cb_defined(Cb::ShowWarningMessage) {
            self.lua_msgs.last_warning_tag = Some(tag.to_string());
            self.lua_msgs.last_warning = Some(text.to_string());
            let _ = self.lua_cb_call(Cb::ShowWarningMessage, "show_warning_message", Vec::new());
            return;
        }
        // print_ln, "warning ", " (file F)", " (tag)", ": text", print_ln
        let file = file.map_or_else(String::new, |f| format!(" (file {f})"));
        let line = format!("warning {file} ({tag}): {text}");
        self.flush_diagnostic_repeats();
        for target in [Target::Term, Target::Log] {
            self.lua_print_line(target, &line);
        }
        self.record_warning(&format!("({tag}): {text}"));
    }

    /// luatex `print_ln(); tprint(line); print_ln()` to the terminal or the log.
    fn lua_print_line(&mut self, target: Target, line: &str) {
        let text = format!("\n{line}\n");
        match target {
            Target::Term => self.append_term(&text),
            Target::Log => self.append_log(&text),
        }
    }

    /// luatex `print_ignored_err(s)`: an error `\ignoreprimitiveerror` turns
    /// into a note. With a `show_ignored_error_message` callback the text
    /// waits in the pending string until the next `error()` (`flush_err`)
    /// runs the callback; without one it goes to the log and `status` keeps
    /// the message.
    pub(crate) fn lua_ignored_error(&mut self, msg: &str) {
        if self.engine_kind == crate::engine::EngineKind::LuaTeX && self.cb_defined(Cb::ShowIgnoredErrorMessage) {
            self.lua_msgs.pending_ignored.get_or_insert_with(String::new).push_str(&format!("ignored: {msg}"));
            return;
        }
        if self.engine_kind == crate::engine::EngineKind::LuaTeX {
            self.lua_msgs.last_error = Some(msg.to_string());
        }
        if self.file_offset > 0 {
            self.append_log("\n");
        }
        self.append_log(&format!("ignored: {msg}"));
    }

    /// luatex `normal_warning("lua", ...)` / `luatex_error`: an error raised
    /// by Lua code. `status.lastluaerrorstring` keeps `err`; a
    /// `show_lua_error_hook` callback prints it instead of TeX.
    pub(crate) fn lua_error(&mut self, prefix: &str, err: &str) {
        if self.engine_kind == crate::engine::EngineKind::LuaTeX {
            self.lua_msgs.last_lua_error = Some(err.strip_prefix("Lua error: ").unwrap_or(err).to_string());
            self.lua_msgs.lua_error_pending = true;
        }
        self.error(&format!("{prefix}{err}"));
        self.lua_msgs.lua_error_pending = false;
    }

    /// luatex `error()` with the message and context callbacks: the
    /// callbacks take over the corresponding parts of `d`, which is what
    /// `Engine::error` displays. `msg` is the raw error message.
    pub(crate) fn lua_error_hooks(&mut self, msg: &str, d: &mut crate::diagnostics::Diagnostic) {
        use crate::diagnostics::{HIDE_CONTEXT, HIDE_MESSAGE};
        if self.engine_kind != crate::engine::EngineKind::LuaTeX || self.lua_msgs.in_hook {
            return;
        }
        let lua_error = self.lua_msgs.lua_error_pending;
        let message_hook = if lua_error { Cb::ShowLuaErrorHook } else { Cb::ShowErrorMessage };
        let context_hook = self.cb_defined(Cb::ShowErrorHook);
        if !lua_error && !self.cb_defined(message_hook) {
            // print_err: `status.lasterrorstring` is what print_err was given
            let text = if self.diagnostic_use_err_help { "" } else { print_err_text(msg) };
            self.lua_msgs.last_error = Some(text.to_string());
        }
        // flush_err: a pending ignored error is shown first; a
        // show_error_message callback, which owns the pending string, takes it
        let mut pending = self.lua_msgs.pending_ignored.take().unwrap_or_default();
        if !pending.is_empty() && !self.cb_defined(Cb::ShowErrorMessage) {
            self.lua_msgs.last_error = Some(std::mem::take(&mut pending));
            self.lua_msgs.in_hook = true;
            if self.cb_defined(Cb::ShowIgnoredErrorMessage) {
                let _ = self.lua_cb_call(Cb::ShowIgnoredErrorMessage, "show_ignored_error_message", Vec::new());
            }
            self.lua_msgs.in_hook = false;
        }
        if !self.cb_defined(message_hook) && !context_hook {
            return;
        }
        self.lua_msgs.in_hook = true;
        if self.cb_defined(message_hook) {
            if !lua_error {
                let text = msg.strip_prefix("Undefined control sequence ").map_or(msg, |_| "Undefined control sequence");
                self.lua_msgs.last_error = Some(format!("{pending}! {text}"));
            }
            let _ = self.lua_cb_call(message_hook, CALLBACK_NAMES[message_hook as usize], Vec::new());
            d.hidden |= HIDE_MESSAGE;
        }
        if context_hook {
            self.lua_msgs.last_error_context = Some(self.show_context_string(inserted_text(msg)));
            let _ = self.lua_cb_call(Cb::ShowErrorHook, "show_error_hook", Vec::new());
            d.hidden |= HIDE_CONTEXT;
        }
        self.lua_msgs.in_hook = false;
    }
}

/// What the callbacks of the messages left behind for `status`
/// (`last_error`, `last_error_context`, `last_lua_error`, `last_warning_tag`,
/// `last_warning_str` of `errors.c`).
#[derive(Default)]
pub(crate) struct LuaMessages {
    pub last_error: Option<String>,
    pub last_error_context: Option<String>,
    pub last_lua_error: Option<String>,
    pub last_warning_tag: Option<String>,
    pub last_warning: Option<String>,
    /// an error raised by Lua code is being reported
    pub lua_error_pending: bool,
    /// text `print_ignored_err` left for a `show_ignored_error_message` callback
    pub pending_ignored: Option<String>,
    /// a message callback is running (errors in it are not hooked again)
    pub in_hook: bool,
}

/// What tex.web's `print_err` is given for an error message: the rest of
/// the message (a control sequence, a command, ` (n)` of `int_error`) is
/// printed after it.
fn print_err_text(msg: &str) -> &str {
    const OPEN: [&str; 11] = [
        "Undefined control sequence",
        "Paragraph ended before ",
        "Argument of ",
        "Use of ",
        "Misplaced ",
        "You can't use `",
        "Missing = inserted for ",
        "Illegal parameter number in definition of ",
        "Extra \\else",
        "Extra \\fi",
        "Extra \\or",
    ];
    if msg == "Misplaced \\noalign" || msg == "Misplaced \\omit" {
        return msg;
    }
    if msg.starts_with("Extra \\endgroup") {
        return "Extra ";
    }
    if msg.starts_with("Missing ") && ["} inserted", "\\endgroup inserted", "\\right. inserted"].iter().any(|t| msg.ends_with(t)) {
        return "Missing ";
    }
    for open in OPEN {
        if msg.starts_with(open) {
            return if open.starts_with("Extra") { "Extra " } else { open };
        }
    }
    // `int_error`: the message is followed by ` (n)`
    if let Some(head) = msg.strip_suffix(')') {
        if let Some((text, number)) = head.rsplit_once(" (") {
            if number.strip_prefix('-').unwrap_or(number).bytes().all(|b| b.is_ascii_digit()) && !number.is_empty() {
                return text;
            }
        }
    }
    msg
}

#[derive(Clone, Copy)]
enum Target {
    Term,
    Log,
}

/// How TeX's `ins_error` inserted text for this error message.
fn inserted_text(msg: &str) -> crate::show_context::Insertion {
    use crate::show_context::Insertion;
    if msg.starts_with("Missing $ inserted") {
        Insertion::Dollar
    } else if ["Missing } inserted", "Missing \\endgroup inserted", "Missing \\right. inserted", "Forbidden control sequence found"]
        .iter()
        .any(|m| msg.starts_with(m))
    {
        Insertion::TopPushed
    } else if msg.starts_with("Argument of ") && msg.ends_with(" has an extra }") {
        Insertion::Par
    } else {
        Insertion::None
    }
}
