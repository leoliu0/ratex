//! The LuaTeX node model (`texnodes.c`, `lnodelib.c`): a store of doubly
//! linked nodes addressed by integer handles. `node.direct` hands the handles
//! to Lua as integers; userdata nodes wrap the same handles. Engine lists
//! (`Vec<Node>`) are converted to and from the store at the points where LuaTeX
//! lets Lua see node lists (callbacks, `tex.box`, ...); see `lua_node_conv`.
//!
//! A node has an id, a subtype, `next`/`prev` links and an attribute list
//! handle; the remaining fields live in fixed slots whose meaning follows the
//! field list of its type in [`crate::lua_node_tables`] (the order of
//! `node.fields`). Floating point `glue_set` lives in [`LNode::fl`]. Strings
//! and engine-only payloads hang off [`LNode::ext`].

use crate::lua_node_tables::{TypeInfo, K, TYPES, WHATSITS};

pub const HLIST: u8 = 0;
pub const VLIST: u8 = 1;
pub const RULE: u8 = 2;
pub const INS: u8 = 3;
pub const MARK: u8 = 4;
pub const ADJUST: u8 = 5;
pub const BOUNDARY: u8 = 6;
pub const DISC: u8 = 7;
pub const WHATSIT: u8 = 8;
pub const LOCAL_PAR: u8 = 9;
pub const DIR: u8 = 10;
pub const MATH: u8 = 11;
pub const GLUE: u8 = 12;
pub const KERN: u8 = 13;
pub const PENALTY: u8 = 14;
pub const UNSET: u8 = 15;
pub const STYLE: u8 = 16;
pub const CHOICE: u8 = 17;
pub const NOAD: u8 = 18;
pub const RADICAL: u8 = 19;
pub const FRACTION: u8 = 20;
pub const ACCENT: u8 = 21;
pub const FENCE: u8 = 22;
pub const MATH_CHAR: u8 = 23;
pub const SUB_BOX: u8 = 24;
pub const SUB_MLIST: u8 = 25;
pub const MATH_TEXT_CHAR: u8 = 26;
pub const DELIM: u8 = 27;
pub const MARGIN_KERN: u8 = 28;
pub const GLYPH: u8 = 29;
pub const PAGE_INSERT: u8 = 33;
pub const SPLIT_INSERT: u8 = 34;
pub const ATTRIBUTE: u8 = 38;
pub const GLUE_SPEC: u8 = 39;
pub const ATTRIBUTE_LIST: u8 = 40;
pub const TEMP: u8 = 41;
/// id of a released store slot.
const FREE_ID: u8 = 255;

/// whatsit subtypes
pub mod ws {
    pub const OPEN: u16 = 0;
    pub const WRITE: u16 = 1;
    pub const CLOSE: u16 = 2;
    pub const SPECIAL: u16 = 3;
    pub const LATE_SPECIAL: u16 = 4;
    pub const SAVE_POS: u16 = 7;
    pub const LATE_LUA: u16 = 8;
    pub const USER_DEFINED: u16 = 9;
    pub const PDF_LITERAL: u16 = 16;
    pub const PDF_LATE_LITERAL: u16 = 17;
    pub const PDF_REFOBJ: u16 = 18;
    pub const PDF_ANNOT: u16 = 19;
    pub const PDF_START_LINK: u16 = 20;
    pub const PDF_END_LINK: u16 = 21;
    pub const PDF_DEST: u16 = 22;
    pub const PDF_ACTION: u16 = 23;
    pub const PDF_THREAD: u16 = 24;
    pub const PDF_START_THREAD: u16 = 25;
    pub const PDF_END_THREAD: u16 = 26;
    pub const PDF_COLORSTACK: u16 = 29;
    pub const PDF_SETMATRIX: u16 = 30;
    pub const PDF_SAVE: u16 = 31;
    pub const PDF_RESTORE: u16 = 32;
    pub const PDF_LINK_STATE: u16 = 33;
}

/// glue subtypes beyond the `\skip` parameters
pub const COND_MATH_GLUE: u16 = 98;
pub const MU_GLUE: u16 = 99;
pub const A_LEADERS: u16 = 100;
pub const C_LEADERS: u16 = 101;
pub const X_LEADERS: u16 = 102;
pub const G_LEADERS: u16 = 103;

/// glyph subtype bits
pub const GLYPH_CHARACTER: u16 = 1;
pub const GLYPH_LIGATURE: u16 = 2;
pub const GLYPH_GHOST: u16 = 4;
pub const GLYPH_LEFT: u16 = 8;
pub const GLYPH_RIGHT: u16 = 16;

/// kern subtypes
pub const FONT_KERN: u16 = 0;
pub const EXPLICIT_KERN: u16 = 1;
pub const ACCENT_KERN: u16 = 2;
pub const ITALIC_KERN: u16 = 3;

/// LuaTeX's value of an attribute that is not set.
pub const UNUSED_ATTRIBUTE: i32 = -0x7FFF_FFFF;
/// LuaTeX `null_flag`: running rule dimension.
pub const NULL_FLAG: i32 = -0x4000_0000;

/// Number of field slots of a node.
pub const NF: usize = 14;

/// Engine-only payload of a node.
#[derive(Clone, Default, Debug)]
pub struct Ext {
    /// Strings of whatsit fields, indexed by the position of the field among
    /// the string fields of its subtype.
    pub strs: Vec<Vec<u8>>,
    /// A node of the engine that has no Lua-visible structure of its own.
    pub opaque: Option<Box<crate::boxes::Node>>,
    /// Tokens of a `mark` (or write) node.
    pub toks: Vec<crate::token::Token>,
}

#[derive(Clone, Debug)]
pub struct LNode {
    pub id: u8,
    pub subtype: u16,
    pub next: u32,
    pub prev: u32,
    pub attr: u32,
    pub f: [i32; NF],
    /// `glue_set`
    pub fl: f64,
    pub ext: Option<Box<Ext>>,
}

impl LNode {
    fn blank(id: u8, subtype: u16) -> Self {
        LNode { id, subtype, next: 0, prev: 0, attr: 0, f: [0; NF], fl: 0.0, ext: None }
    }
}

/// The value of a node field as handed to Lua.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Nil,
    Int(i64),
    Num(f64),
    Node(u32),
    Bytes(Vec<u8>),
    Str(&'static str),
    Toks,
}

/// A value assigned to a node field.
#[derive(Clone, Debug)]
pub enum SetVal {
    Nil,
    Int(i64),
    Num(f64),
    Node(u32),
    Bytes(Vec<u8>),
    Other,
}

impl SetVal {
    /// `lua_tointeger`: numbers (floats with an integral value), 0 otherwise.
    pub fn to_int(&self) -> i64 {
        match self {
            SetVal::Int(i) => *i,
            SetVal::Num(n) if n.fract() == 0.0 => *n as i64,
            SetVal::Bytes(b) => std::str::from_utf8(b)
                .ok()
                .and_then(|s| s.trim().parse::<i64>().ok())
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// `lua_roundnumber`: nearest integer.
    pub fn to_round(&self) -> i64 {
        match self {
            SetVal::Num(n) => (*n + 0.5).floor() as i64,
            _ => self.to_int(),
        }
    }

    pub fn to_num(&self) -> f64 {
        match self {
            SetVal::Int(i) => *i as f64,
            SetVal::Num(n) => *n,
            SetVal::Bytes(b) => std::str::from_utf8(b)
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(0.0),
            _ => 0.0,
        }
    }

    pub fn to_node(&self) -> u32 {
        match self {
            SetVal::Node(n) => *n,
            _ => 0,
        }
    }

    pub fn is_number(&self) -> bool {
        matches!(self, SetVal::Int(_) | SetVal::Num(_))
    }
}

pub fn type_info(id: u8) -> Option<&'static TypeInfo> {
    TYPES.get(usize::from(id)).filter(|t| t.id == id)
}

pub fn type_by_name(name: &[u8]) -> Option<&'static TypeInfo> {
    TYPES.iter().find(|t| t.name.as_bytes() == name)
}

pub fn whatsit_by_name(name: &[u8]) -> Option<u16> {
    WHATSITS.iter().find(|w| w.name.as_bytes() == name).map(|w| u16::from(w.id))
}

pub fn whatsit_info(sub: u16) -> Option<&'static crate::lua_node_tables::WhatsitInfo> {
    WHATSITS.iter().find(|w| u16::from(w.id) == sub)
}

/// `nodetype_has_attributes`
pub fn has_attr_type(id: u8, subtype: u16) -> bool {
    match id {
        WHATSIT => !matches!(subtype, ws::PDF_ACTION | 27 | 28),
        UNSET => false,
        HLIST..=GLYPH => true,
        _ => false,
    }
}

pub fn has_subtype_type(id: u8) -> bool {
    !matches!(id, LOCAL_PAR | GLUE_SPEC) && id <= TEMP
}

/// The fields of a node of type `id`/`subtype`, in `node.fields` order
/// (without id, subtype and attr).
pub fn fields_of(id: u8, subtype: u16) -> &'static [(&'static str, K)] {
    if id == WHATSIT {
        return whatsit_info(subtype).map_or(&[], |w| w.fields);
    }
    type_info(id).map_or(&[], |t| t.fields)
}

/// Direction names of `dir` fields.
pub const DIR_NAMES: [&str; 4] = ["TLT", "TRT", "LTL", "RTT"];

pub fn math_style_name(sub: u16) -> Option<&'static str> {
    Some(match sub {
        0 => "display",
        1 => "crampeddisplay",
        2 => "text",
        3 => "crampedtext",
        4 => "script",
        5 => "crampedscript",
        6 => "scriptscript",
        7 => "crampedscriptscript",
        _ => return None,
    })
}

pub fn math_style_from_name(name: &[u8]) -> Option<u16> {
    (0..8u16).find(|&i| math_style_name(i).is_some_and(|n| n.as_bytes() == name))
}

/// A resolved field: slot index and kind.
#[derive(Clone, Copy, Debug)]
pub struct Fld {
    pub slot: usize,
    pub kind: K,
}

/// Find field `name` of a node. Handles the aliases `lua_nodelib_fast_getfield`
/// accepts besides the names of `node.fields`.
pub fn resolve(id: u8, subtype: u16, name: &str) -> Option<Fld> {
    let fields = fields_of(id, subtype);
    let find = |n: &str| fields.iter().position(|(f, _)| *f == n).map(|slot| Fld { slot, kind: fields[slot].1 });
    if let Some(f) = find(name) {
        return Some(f);
    }
    let alias = match (id, name) {
        (HLIST | VLIST | INS | ADJUST | UNSET | SUB_BOX | SUB_MLIST, "list") => "head",
        (HLIST | VLIST | RULE | UNSET, "direction") => "dir",
        (ACCENT, "top_accent") => "accent",
        (ACCENT, "accent") => "accent",
        (WHATSIT, "cmd") => return None,
        (WHATSIT, "command") if subtype == ws::PDF_COLORSTACK => "cmd",
        (WHATSIT, "token" | "string") if matches!(subtype, ws::PDF_LITERAL | ws::PDF_LATE_LITERAL | ws::LATE_LUA) => "data",
        (WHATSIT, "value") if matches!(subtype, ws::SPECIAL | ws::LATE_SPECIAL | ws::WRITE) => "data",
        _ => return None,
    };
    find(alias)
}

#[derive(Default)]
pub struct NodeStore {
    pub nodes: Vec<LNode>,
    free: Vec<u32>,
    pub live: usize,
    /// cache of the attribute list built from the `\attribute` registers:
    /// (sorted pairs, list handle holding one reference)
    attr_cache: Option<(Vec<(i32, i32)>, u32)>,
    /// `node.fix_node_lists`
    pub fix_node_lists: bool,
    /// the Lua table `node.get_properties_table()` returns
    pub props: Option<tex_lua::LuaTable>,
    /// `node.set_properties_mode`: (enabled, auto-clean on free)
    pub props_mode: (bool, bool),
}

impl NodeStore {
    pub fn new() -> Self {
        let mut s = NodeStore::default();
        s.nodes.push(LNode::blank(FREE_ID, 0));
        s
    }

    #[inline]
    pub fn valid(&self, n: u32) -> bool {
        n != 0 && self.nodes.get(n as usize).is_some_and(|x| x.id != FREE_ID)
    }

    #[inline]
    pub fn node(&self, n: u32) -> &LNode {
        &self.nodes[n as usize]
    }

    #[inline]
    pub fn node_mut(&mut self, n: u32) -> &mut LNode {
        &mut self.nodes[n as usize]
    }

    #[inline]
    pub fn id(&self, n: u32) -> u8 {
        self.nodes[n as usize].id
    }

    #[inline]
    pub fn subtype(&self, n: u32) -> u16 {
        self.nodes[n as usize].subtype
    }

    #[inline]
    pub fn next(&self, n: u32) -> u32 {
        if n == 0 { 0 } else { self.nodes[n as usize].next }
    }

    #[inline]
    pub fn prev(&self, n: u32) -> u32 {
        if n == 0 { 0 } else { self.nodes[n as usize].prev }
    }

    fn alloc(&mut self, id: u8, subtype: u16) -> u32 {
        self.live += 1;
        if let Some(h) = self.free.pop() {
            self.nodes[h as usize] = LNode::blank(id, subtype);
            h
        } else {
            self.nodes.push(LNode::blank(id, subtype));
            (self.nodes.len() - 1) as u32
        }
    }

    fn release(&mut self, n: u32) {
        let slot = &mut self.nodes[n as usize];
        *slot = LNode::blank(FREE_ID, 0);
        self.free.push(n);
        self.live -= 1;
    }

    /// `new_node`: a node with LuaTeX's defaults; it takes `attr` (the list
    /// of the current `\attribute` values, 0 for none) when the type has
    /// attributes. The caller adds the reference count via
    /// [`Self::assign_attr_ref`].
    pub fn new_node(&mut self, id: u8, subtype: u16, attr: u32) -> u32 {
        let n = self.alloc(id, subtype);
        {
            let node = &mut self.nodes[n as usize];
            match id {
                GLYPH => {
                    // init_lang_data: lang 0, left/right hyphenmin 1
                    node.f[2] = 0;
                    node.f[3] = 1;
                    node.f[4] = 1;
                    node.f[GLYPH_LANG_DATA] = 256 + 1;
                }
                HLIST | VLIST => node.f[3] = -1,
                RULE => {
                    node.f[0] = NULL_FLAG;
                    node.f[1] = NULL_FLAG;
                    node.f[2] = NULL_FLAG;
                    node.f[3] = -1;
                }
                UNSET => node.f[0] = NULL_FLAG,
                FRACTION => node.f[6] = -1,
                WHATSIT => {
                    let ext = Ext {
                        strs: match subtype {
                            ws::OPEN => vec![Vec::new(); 3],
                            ws::WRITE | ws::SPECIAL | ws::LATE_SPECIAL | ws::PDF_ANNOT | ws::PDF_SETMATRIX => vec![Vec::new()],
                            ws::PDF_LITERAL | ws::PDF_LATE_LITERAL => vec![Vec::new()],
                            ws::LATE_LUA => vec![Vec::new(); 3],
                            ws::PDF_START_LINK => vec![Vec::new()],
                            ws::PDF_COLORSTACK => vec![Vec::new()],
                            ws::PDF_DEST => vec![Vec::new()],
                            ws::PDF_THREAD | ws::PDF_START_THREAD => vec![Vec::new(); 2],
                            ws::PDF_ACTION => vec![Vec::new(); 4],
                            _ => Vec::new(),
                        },
                        ..Ext::default()
                    };
                    if !ext.strs.is_empty() {
                        node.ext = Some(Box::new(ext));
                    }
                }
                _ => {}
            }
            if id == DIR {
                node.f[0] = 0;
            }
        }
        if attr != 0 && has_attr_type(id, subtype) {
            self.assign_attr_ref(n, attr);
        }
        n
    }

    // ---------------------------------------------------------------- links

    pub fn couple(&mut self, a: u32, b: u32) {
        if a != 0 {
            self.nodes[a as usize].next = b;
        }
        if b != 0 {
            self.nodes[b as usize].prev = a;
        }
    }

    pub fn tail_of(&self, mut n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        while self.nodes[n as usize].next != 0 {
            n = self.nodes[n as usize].next;
        }
        n
    }

    pub fn list_len(&self, mut n: u32) -> usize {
        let mut c = 0;
        while n != 0 {
            c += 1;
            n = self.nodes[n as usize].next;
        }
        c
    }

    // ------------------------------------------------------------ attributes

    fn attr_ref(&self, list: u32) -> i32 {
        self.nodes[list as usize].f[0]
    }

    /// The first attribute node of attribute list `list`.
    pub fn attr_first(&self, list: u32) -> u32 {
        if list == 0 { 0 } else { self.nodes[list as usize].next }
    }

    pub fn assign_attr_ref(&mut self, n: u32, list: u32) {
        if list != 0 {
            self.nodes[list as usize].f[0] += 1;
        }
        self.nodes[n as usize].attr = list;
    }

    /// `delete_attribute_ref`
    pub fn delete_attr_ref(&mut self, list: u32) {
        if list == 0 || self.nodes[list as usize].id != ATTRIBUTE_LIST {
            return;
        }
        let l = &mut self.nodes[list as usize];
        l.f[0] -= 1;
        if l.f[0] <= 0 {
            if self.attr_cache.as_ref().is_some_and(|(_, h)| *h == list) {
                self.attr_cache = None;
            }
            let mut p = list;
            while p != 0 {
                let nx = self.nodes[p as usize].next;
                self.release(p);
                p = nx;
            }
        }
    }

    /// `reassign_attribute`
    pub fn reassign_attr(&mut self, n: u32, new: u32) {
        let old = self.nodes[n as usize].attr;
        if old == new {
            return;
        }
        if new != 0 {
            self.nodes[new as usize].f[0] += 1;
        }
        self.nodes[n as usize].attr = new;
        if old != 0 {
            self.delete_attr_ref(old);
        }
    }

    fn new_attribute_node(&mut self, number: i32, value: i32) -> u32 {
        let a = self.alloc(ATTRIBUTE, 0);
        self.nodes[a as usize].f[0] = number;
        self.nodes[a as usize].f[1] = value;
        a
    }

    /// An attribute list holding `pairs` (sorted by number) with reference
    /// count 0.
    pub fn build_attr_list(&mut self, pairs: &[(i32, i32)]) -> u32 {
        let head = self.alloc(ATTRIBUTE_LIST, 0);
        let mut p = head;
        for &(k, v) in pairs {
            let a = self.new_attribute_node(k, v);
            self.nodes[p as usize].next = a;
            p = a;
        }
        head
    }

    /// The pairs of attribute list `list`, including unset values.
    pub fn attr_pairs(&self, list: u32) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        let mut p = self.attr_first(list);
        while p != 0 {
            let a = &self.nodes[p as usize];
            out.push((a.f[0], a.f[1]));
            p = a.next;
        }
        out
    }

    /// `current_attribute_list`: a list (kept alive by the cache) for the
    /// attribute registers `regs` (sorted, only set ones), 0 when none is set.
    pub fn current_attr_list(&mut self, regs: &[(i32, i32)]) -> u32 {
        if regs.is_empty() {
            if let Some((_, h)) = self.attr_cache.take() {
                self.delete_attr_ref(h);
            }
            return 0;
        }
        if let Some((pairs, h)) = &self.attr_cache {
            if pairs.as_slice() == regs && self.valid(*h) {
                return *h;
            }
        }
        if let Some((_, h)) = self.attr_cache.take() {
            self.delete_attr_ref(h);
        }
        let h = self.build_attr_list(regs);
        self.nodes[h as usize].f[0] = 1; // the cache's reference
        self.attr_cache = Some((regs.to_vec(), h));
        h
    }

    fn copy_attr_list(&mut self, list: u32) -> u32 {
        let pairs = self.attr_pairs(list);
        self.build_attr_list(&pairs)
    }

    /// `set_attribute`
    pub fn set_attribute(&mut self, n: u32, i: i32, val: i32) {
        let (id, sub) = (self.nodes[n as usize].id, self.nodes[n as usize].subtype);
        if !has_attr_type(id, sub) {
            return;
        }
        let mut list = self.nodes[n as usize].attr;
        if list == 0 {
            let l = self.build_attr_list(&[(i, val)]);
            self.nodes[l as usize].f[0] = 1;
            self.nodes[n as usize].attr = l;
            return;
        }
        // unchanged?
        let mut p = self.nodes[list as usize].next;
        while p != 0 {
            let a = &self.nodes[p as usize];
            if a.f[0] == i && a.f[1] == val {
                return;
            }
            if a.f[0] >= i {
                break;
            }
            p = a.next;
        }
        let shared = self.attr_ref(list) > 1 || self.attr_cache.as_ref().is_some_and(|(_, h)| *h == list);
        if shared {
            let copy = self.copy_attr_list(list);
            self.nodes[copy as usize].f[0] = 1;
            self.delete_attr_ref(list);
            self.nodes[n as usize].attr = copy;
            list = copy;
        }
        let mut prev = list;
        loop {
            let nx = self.nodes[prev as usize].next;
            if nx != 0 && self.nodes[nx as usize].f[0] < i {
                prev = nx;
            } else {
                break;
            }
        }
        let nx = self.nodes[prev as usize].next;
        if nx != 0 && self.nodes[nx as usize].f[0] == i {
            self.nodes[nx as usize].f[1] = val;
        } else {
            let r = self.new_attribute_node(i, val);
            self.nodes[r as usize].next = nx;
            self.nodes[prev as usize].next = r;
        }
    }

    /// `unset_attribute`: the old value or [`UNUSED_ATTRIBUTE`].
    pub fn unset_attribute(&mut self, n: u32, i: i32, val: i32) -> i32 {
        let (id, sub) = (self.nodes[n as usize].id, self.nodes[n as usize].subtype);
        if !has_attr_type(id, sub) {
            return 0;
        }
        let list = self.nodes[n as usize].attr;
        if list == 0 {
            return UNUSED_ATTRIBUTE;
        }
        let mut p = self.nodes[list as usize].next;
        let mut j = 0;
        while p != 0 {
            let t = self.nodes[p as usize].f[0];
            if t > i {
                return UNUSED_ATTRIBUTE;
            }
            if t == i {
                break;
            }
            j += 1;
            p = self.nodes[p as usize].next;
        }
        if p == 0 {
            return UNUSED_ATTRIBUTE;
        }
        let mut list = list;
        if self.attr_ref(list) > 1 || self.attr_cache.as_ref().is_some_and(|(_, h)| *h == list) {
            let copy = self.copy_attr_list(list);
            self.nodes[copy as usize].f[0] = 1;
            self.delete_attr_ref(list);
            self.nodes[n as usize].attr = copy;
            list = copy;
        }
        let mut q = self.nodes[list as usize].next;
        for _ in 0..j {
            q = self.nodes[q as usize].next;
        }
        let t = self.nodes[q as usize].f[1];
        if val == UNUSED_ATTRIBUTE || t == val {
            self.nodes[q as usize].f[1] = UNUSED_ATTRIBUTE;
        }
        t
    }

    /// `has_attribute`
    pub fn has_attribute(&self, n: u32, i: i32, val: i32) -> i32 {
        let (id, sub) = (self.nodes[n as usize].id, self.nodes[n as usize].subtype);
        if !has_attr_type(id, sub) {
            return UNUSED_ATTRIBUTE;
        }
        let mut p = self.attr_first(self.nodes[n as usize].attr);
        while p != 0 {
            let a = &self.nodes[p as usize];
            if a.f[0] == i {
                let ret = a.f[1];
                return if val == UNUSED_ATTRIBUTE || val == ret { ret } else { UNUSED_ATTRIBUTE };
            } else if a.f[0] > i {
                return UNUSED_ATTRIBUTE;
            }
            p = a.next;
        }
        UNUSED_ATTRIBUTE
    }

    // ------------------------------------------------------ copy and release

    /// Handles of the child lists / nodes owned by `n` (slots of kind `N`).
    fn child_slots(&self, n: u32) -> impl Iterator<Item = usize> + '_ {
        let node = &self.nodes[n as usize];
        fields_of(node.id, node.subtype)
            .iter()
            .enumerate()
            .filter(|(_, (_, k))| *k == K::N)
            .map(|(i, _)| i)
    }

    /// `copy_node`: a copy of `n` alone (links cleared), with deep copies of
    /// its sub lists.
    pub fn copy_node(&mut self, n: u32) -> u32 {
        let src = self.nodes[n as usize].clone();
        let r = self.alloc(src.id, src.subtype);
        let mut copy = src;
        copy.next = 0;
        copy.prev = 0;
        let attr = copy.attr;
        let slots: Vec<usize> = fields_of(copy.id, copy.subtype)
            .iter()
            .enumerate()
            .filter(|(_, (_, k))| *k == K::N)
            .map(|(i, _)| i)
            .collect();
        self.nodes[r as usize] = copy;
        if attr != 0 {
            self.nodes[attr as usize].f[0] += 1;
        }
        // user defined whatsits with node values copy them as well
        for i in slots {
            let child = self.nodes[r as usize].f[i] as u32;
            if child != 0 {
                let c = self.copy_list(child);
                self.nodes[r as usize].f[i] = c as i32;
            }
        }
        if self.nodes[r as usize].id == WHATSIT
            && self.nodes[r as usize].subtype == ws::USER_DEFINED
            && self.nodes[r as usize].f[1] == i32::from(b'n')
        {
            let v = self.nodes[r as usize].f[2] as u32;
            if v != 0 {
                let c = self.copy_list(v);
                self.nodes[r as usize].f[2] = c as i32;
            }
        }
        r
    }

    /// `copy_node_list`
    pub fn copy_list(&mut self, mut p: u32) -> u32 {
        self.copy_range(p, 0)
    }

    /// Copy the nodes `p` up to (excluding) `end`.
    pub fn copy_range(&mut self, mut p: u32, end: u32) -> u32 {
        let mut head = 0;
        let mut q = 0;
        while p != end && p != 0 {
            let s = self.copy_node(p);
            if head == 0 {
                head = s;
            } else {
                self.couple(q, s);
            }
            q = s;
            p = self.nodes[p as usize].next;
        }
        head
    }

    /// `flush_node`: release `n` and what it owns.
    pub fn flush_node(&mut self, n: u32) {
        if !self.valid(n) {
            return;
        }
        let slots: Vec<usize> = self.child_slots(n).collect();
        let attr = self.nodes[n as usize].attr;
        for i in slots {
            let child = self.nodes[n as usize].f[i] as u32;
            if child != 0 {
                self.flush_list(child);
            }
        }
        if self.nodes[n as usize].id == WHATSIT
            && self.nodes[n as usize].subtype == ws::USER_DEFINED
            && self.nodes[n as usize].f[1] == i32::from(b'n')
        {
            let v = self.nodes[n as usize].f[2] as u32;
            if v != 0 {
                self.flush_list(v);
            }
        }
        if attr != 0 && has_attr_type(self.nodes[n as usize].id, self.nodes[n as usize].subtype) {
            self.delete_attr_ref(attr);
        }
        self.release(n);
    }

    /// `flush_node_list`
    pub fn flush_list(&mut self, mut n: u32) {
        while self.valid(n) {
            let nx = self.nodes[n as usize].next;
            self.flush_node(n);
            n = nx;
        }
    }

    // ------------------------------------------------- generic field access

    /// `n.<name>` as LuaTeX answers it. `dims` supplies glyph
    /// width/height/depth for (font, char).
    pub fn get_field(&self, n: u32, name: &str, dims: &dyn Fn(i32, i32) -> (i32, i32, i32)) -> Val {
        let node = &self.nodes[n as usize];
        let (id, sub) = (node.id, node.subtype);
        match name {
            "id" => return Val::Int(i64::from(id)),
            "next" => return node_val(node.next),
            "prev" => return node_val(node.prev),
            "attr" => {
                return if has_attr_type(id, sub) { node_val(node.attr) } else { Val::Nil };
            }
            "subtype" => {
                return if (id == WHATSIT || has_subtype_type(id)) && !matches!(id, 30..=32 | 35..=37 | TEMP) {
                    Val::Int(i64::from(sub))
                } else {
                    Val::Nil
                };
            }
            _ => {}
        }
        // node types without Lua-visible fields (`lua_nodelib_fast_getfield`
        // falls through to nil for them)
        if matches!(id, 30..=32 | 35..=37 | TEMP) {
            return Val::Nil;
        }
        if id == UNSET && name == "span" {
            return Val::Nil;
        }
        if id == GLYPH {
            match name {
                "width" | "height" | "depth" => {
                    let (w, h, d) = dims(node.f[1], node.f[0]);
                    return Val::Int(i64::from(match name {
                        "width" => w,
                        "height" => h,
                        _ => d,
                    }));
                }
                _ => {}
            }
        }
        if id == STYLE && name == "style" {
            return math_style_name(sub).map_or(Val::Nil, Val::Str);
        }
        let Some(f) = resolve(id, sub, name) else { return Val::Nil };
        let fv = node.f[f.slot];
        match f.kind {
            K::I => {
                if id == WHATSIT
                    && ((sub == ws::PDF_COLORSTACK && name == "cmd")
                        || (sub == ws::PDF_LINK_STATE && name == "value")
                        || (sub == ws::LATE_LUA && name == "reg" && fv == 0))
                {
                    return Val::Nil;
                }
                Val::Int(i64::from(fv))
            }
            K::F => Val::Num(node.fl),
            K::N => node_val(fv as u32),
            K::D => {
                if (0..4).contains(&fv) {
                    if id == DIR {
                        Val::Bytes(
                            format!("{}{}", if sub == 1 { '-' } else { '+' }, DIR_NAMES[fv as usize])
                                .into_bytes(),
                        )
                    } else {
                        Val::Str(DIR_NAMES[fv as usize])
                    }
                } else {
                    Val::Nil
                }
            }
            K::S => {
                if id == MARK {
                    return Val::Toks;
                }
                let text = self.ext_str(n, name);
                if text.is_empty() && id == WHATSIT {
                    // fields LuaTeX keeps as numbers until a string is assigned
                    match (sub, name) {
                        (ws::PDF_DEST, "dest_id") | (ws::PDF_ACTION, "action_id") => return Val::Int(0),
                        (ws::PDF_THREAD | ws::PDF_START_THREAD, "thread_id") => return Val::Int(0),
                        (ws::PDF_ACTION, "struct_id") => return Val::Nil,
                        // a new literal reads back as "data" (an unset reference)
                        (ws::PDF_LITERAL | ws::PDF_LATE_LITERAL, "data") => return Val::Bytes(b"data".to_vec()),
                        _ => {}
                    }
                }
                Val::Bytes(text)
            }
            K::V => self.user_value(n),
        }
    }

    /// Index of the string field `name` among the string fields of `n`.
    fn str_index(&self, n: u32, name: &str) -> Option<usize> {
        let node = &self.nodes[n as usize];
        let target = resolve(node.id, node.subtype, name)?;
        let mut idx = 0;
        for (i, (_, k)) in fields_of(node.id, node.subtype).iter().enumerate() {
            if *k == K::S {
                if i == target.slot {
                    return Some(idx);
                }
                idx += 1;
            }
        }
        None
    }

    pub fn ext_str(&self, n: u32, name: &str) -> Vec<u8> {
        match (self.str_index(n, name), self.nodes[n as usize].ext.as_ref()) {
            (Some(i), Some(e)) => e.strs.get(i).cloned().unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn set_ext_str(&mut self, n: u32, name: &str, v: Vec<u8>) {
        let Some(i) = self.str_index(n, name) else { return };
        let ext = self.nodes[n as usize].ext.get_or_insert_with(Default::default);
        if ext.strs.len() <= i {
            ext.strs.resize(i + 1, Vec::new());
        }
        ext.strs[i] = v;
    }

    /// `value` of a user defined whatsit
    fn user_value(&self, n: u32) -> Val {
        let node = &self.nodes[n as usize];
        match u8::try_from(node.f[1]).map(char::from) {
            Ok('a') | Ok('n') => node_val(node.f[2] as u32),
            Ok('s') => Val::Bytes(node.ext.as_ref().and_then(|e| e.strs.first().cloned()).unwrap_or_default()),
            Ok('t') => Val::Toks,
            Ok('l') => Val::Nil,
            _ => Val::Int(i64::from(node.f[2])),
        }
    }

    /// `n.<name> = v`; errors name the field like LuaTeX does.
    pub fn set_field(&mut self, n: u32, name: &str, v: SetVal) -> Result<(), String> {
        let (id, sub) = (self.nodes[n as usize].id, self.nodes[n as usize].subtype);
        match name {
            "next" => {
                let x = v.to_node();
                if x != 0 && self.valid(x) && self.nodes[x as usize].id == GLUE_SPEC {
                    return Err("You can't assign a glue_spec node to a next field\n".to_string());
                }
                self.nodes[n as usize].next = x;
                return Ok(());
            }
            "prev" => {
                let x = v.to_node();
                if x != 0 && self.valid(x) && self.nodes[x as usize].id == GLUE_SPEC {
                    return Err("You can't assign a glue_spec node to a prev field\n".to_string());
                }
                self.nodes[n as usize].prev = x;
                return Ok(());
            }
            "attr" => {
                if has_attr_type(id, sub) {
                    self.reassign_attr(n, v.to_node());
                }
                return Ok(());
            }
            "subtype" => {
                if id == PENALTY {
                    return Ok(());
                }
                if id == WHATSIT || has_subtype_type(id) {
                    self.nodes[n as usize].subtype = v.to_int() as u16;
                    return Ok(());
                }
            }
            _ => {}
        }
        let cant = || {
            format!(
                "You cannot set field {} in a node of type {}",
                name,
                type_info(id).map_or("unknown", |t| t.name)
            )
        };
        if id == GLYPH && matches!(name, "width" | "height" | "depth") {
            return Ok(());
        }
        if id == STYLE {
            // `style` takes a name (unknown names mean text) or a number;
            // other names are silently ignored
            if name == "style" {
                self.nodes[n as usize].subtype = match &v {
                    SetVal::Bytes(b) => math_style_from_name(b).unwrap_or(2),
                    other => other.to_int() as u16,
                };
            }
            return Ok(());
        }
        if (id == INS && name == "spec") || (id == UNSET && name == "span") {
            return Err(cant());
        }
        if id == GLYPH && matches!(name, "lang" | "left" | "right" | "uchyph") {
            let f = &mut self.nodes[n as usize].f;
            let (lang, left, right, uchyph) = split_lang_data(f[GLYPH_LANG_DATA]);
            let x = v.to_int() as i32;
            let ld = match name {
                "lang" => make_lang_data(uchyph, x, left, right),
                "left" => make_lang_data(uchyph, lang, x, right),
                "right" => make_lang_data(uchyph, lang, left, x),
                _ => make_lang_data(x, lang, left, right),
            };
            let (lang, left, right, uchyph) = split_lang_data(ld);
            f[GLYPH_LANG_DATA] = ld;
            f[2] = lang;
            f[3] = left;
            f[4] = right;
            f[5] = uchyph;
            return Ok(());
        }
        let Some(f) = resolve(id, sub, name) else { return Err(cant()) };
        match f.kind {
            K::I => {
                let mut val = if is_dim_field(id, name) { v.to_round() } else { v.to_int() };
                if is_quarterword_field(id, name) {
                    val &= 0xFFFF;
                }
                self.nodes[n as usize].f[f.slot] = val as i32;
            }
            K::F => self.nodes[n as usize].fl = v.to_num(),
            K::N => {
                let x = v.to_node();
                self.nodes[n as usize].f[f.slot] = x as i32;
            }
            K::D => {
                let val = match &v {
                    SetVal::Bytes(b) => {
                        let text = String::from_utf8_lossy(b).into_owned();
                        let (minus, t) = match text.strip_prefix('+') {
                            Some(t) => (false, t),
                            None => match text.strip_prefix('-') {
                                Some(t) => (true, t),
                                None => (false, text.as_str()),
                            },
                        };
                        let Some(d) = DIR_NAMES.iter().position(|d| *d == t) else {
                            return Err(format!("Bad direction specifier {text}"));
                        };
                        if id == DIR {
                            self.nodes[n as usize].subtype = u16::from(minus);
                        }
                        d as i64
                    }
                    _ => return Err("Direction specifiers have to be strings".to_string()),
                };
                self.nodes[n as usize].f[f.slot] = val as i32;
            }
            K::S => {
                if id == MARK {
                    if let SetVal::Bytes(b) = v {
                        self.set_ext_str(n, name, b);
                    }
                } else if let SetVal::Bytes(b) = v {
                    self.set_ext_str(n, name, b);
                } else {
                    self.set_ext_str(n, name, Vec::new());
                }
            }
            K::V => match u8::try_from(self.nodes[n as usize].f[1]).map(char::from) {
                Ok('a') | Ok('n') => self.nodes[n as usize].f[2] = v.to_node() as i32,
                Ok('s') => {
                    let ext = self.nodes[n as usize].ext.get_or_insert_with(Default::default);
                    if ext.strs.is_empty() {
                        ext.strs.push(Vec::new());
                    }
                    if let SetVal::Bytes(b) = v {
                        ext.strs[0] = b;
                    }
                }
                _ => self.nodes[n as usize].f[2] = v.to_int() as i32,
            },
        }
        Ok(())
    }
}

#[inline]
fn node_val(h: u32) -> Val {
    if h == 0 { Val::Nil } else { Val::Node(h) }
}

/// texnodes.h `make_lang_data` (int arithmetic, wrapping like the C code
/// compiled for a two's complement target)
pub fn make_lang_data(uchyph: i32, lang: i32, left: i32, right: i32) -> i32 {
    let clamp = |v: i32| if v > 0 && v < 256 { v } else { 255 };
    (if uchyph > 0 { i32::MIN } else { 0 })
        .wrapping_add(lang.wrapping_shl(16))
        .wrapping_add(clamp(left) << 8)
        .wrapping_add(clamp(right))
}

/// (lang, left, right, uchyph) of a packed `lang_data`.
pub fn split_lang_data(ld: i32) -> (i32, i32, i32, i32) {
    let lang = (((ld as u32 & 0x7FFF_0000) as i32).wrapping_shl(1)) >> 17;
    (lang, (ld >> 8) & 0xFF, ld & 0xFF, ((ld as u32) >> 31) as i32)
}

/// Slot of a glyph holding the packed `lang_data` (the separate fields are
/// views of it).
pub const GLYPH_LANG_DATA: usize = 9;

/// fields LuaTeX stores in a 16 bit quarterword
fn is_quarterword_field(id: u8, name: &str) -> bool {
    match id {
        HLIST | VLIST | UNSET => matches!(name, "glue_order" | "glue_sign"),
        GLUE | MATH => matches!(name, "stretch_order" | "shrink_order"),
        DISC => name == "penalty",
        _ => false,
    }
}

/// fields set with `lua_roundnumber` (dimensions) rather than `lua_tointeger`
fn is_dim_field(id: u8, name: &str) -> bool {
    match name {
        "width" | "height" | "depth" | "shift" | "stretch" | "shrink" | "kern" | "xoffset" | "yoffset"
        | "expansion_factor" | "left" | "right" | "surround" | "box_left_width" | "box_right_width"
        | "italic" => true,
        "fraction" => id == ACCENT,
        _ => false,
    }
}
