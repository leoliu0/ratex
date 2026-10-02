//! The `node` and `node.direct` libraries (lnodelib.c). The natives below
//! work on integer node handles (the `node.direct` semantics); `lua_node.lua`
//! builds the public tables from them: `node.direct` is the natives under
//! their LuaTeX names, `node` wraps them for userdata nodes.

use std::rc::Rc;

use tex_lua::{Lua, LuaApi, LuaTable, UdValue, UserDataTrait, Value, Variadic};

use crate::engine::Engine;
use crate::lua_bridge::with_engine;
use crate::lua_node::*;
use crate::lua_node_conv::sl;
use crate::tfm::Font;

/// The userdata form of a node: LuaTeX's `luatex.node` objects.
#[derive(Clone)]
pub struct NodeUd {
    pub h: u32,
}

pub(crate) fn glyph_dimensions(fonts: &[Rc<Font>], font: i32, c: i32) -> (i32, i32, i32) {
    let Some(f) = fonts.get(font as usize) else { return (0, 0, 0) };
    if let Some(ci) = f.lua_char(c as u32) {
        return (ci.width, ci.height, ci.depth);
    }
    if f.lua.is_none() && (0..256).contains(&c) {
        let c = c as u8;
        return (f.char_width(c), f.char_height(c), f.char_depth(c));
    }
    (0, 0, 0)
}

/// The value of a field as a Lua value; node values are handles (direct) or
/// userdata.
fn to_ud(v: Val, direct: bool) -> UdValue {
    match v {
        Val::Nil => UdValue::Nil,
        Val::Int(i) => UdValue::Integer(i),
        Val::Num(n) => UdValue::Number(n),
        Val::Node(h) => {
            if direct {
                UdValue::Integer(i64::from(h))
            } else {
                UdValue::from_userdata(NodeUd { h })
            }
        }
        Val::Bytes(b) => UdValue::Bytes(b),
        Val::Str(s) => UdValue::Str(s.to_string()),
        Val::Toks => UdValue::Nil,
    }
}

fn node_ud(h: u32) -> UdValue {
    if h == 0 { UdValue::Nil } else { UdValue::from_userdata(NodeUd { h }) }
}

impl Engine {
    fn lua_node_field(&self, n: u32, name: &str) -> Val {
        let s = &self.lua_nodes;
        if !s.valid(n) {
            return Val::Nil;
        }
        let fonts = &self.eqtb.fonts;
        let v = s.get_field(n, name, &|f, c| glyph_dimensions(fonts, f, c));
        if matches!(v, Val::Toks) {
            return Val::Bytes(self.lua_node_tokens(n));
        }
        v
    }

    /// The tokens of a mark / write node as text.
    fn lua_node_tokens(&self, n: u32) -> Vec<u8> {
        match &self.lua_nodes.node(n).ext {
            Some(e) => self.tokens_to_bytes(&e.toks),
            None => Vec::new(),
        }
    }
}

/// `n.<name>` for a userdata node.
fn ud_get(n: u32, key: &str) -> Option<UdValue> {
    with_engine(|e| {
        let v = e.lua_node_field(n, key);
        Some(to_ud(v, false))
    })
    .ok()
    .flatten()
}

impl UserDataTrait for NodeUd {
    fn type_name(&self) -> &'static str {
        "luatex.node"
    }

    fn get_field(&self, key: &str) -> Option<UdValue> {
        ud_get(self.h, key)
    }

    fn get_int_field(&self, key: i64) -> Option<UdValue> {
        with_engine(|e| {
            let s = &e.lua_nodes;
            if !s.valid(self.h) {
                return UdValue::Nil;
            }
            let id = s.id(self.h);
            let sub = s.subtype(self.h);
            if !has_attr_type(id, sub) {
                return UdValue::Nil;
            }
            let v = s.has_attribute(self.h, key as i32, UNUSED_ATTRIBUTE);
            if v > UNUSED_ATTRIBUTE { UdValue::Integer(i64::from(v)) } else { UdValue::Nil }
        })
        .ok()
    }

    fn set_field(&mut self, key: &str, value: UdValue) -> Option<Result<(), String>> {
        let v = match value {
            UdValue::Nil => SetVal::Nil,
            UdValue::Integer(i) => SetVal::Int(i),
            UdValue::Number(n) => SetVal::Num(n),
            UdValue::Boolean(_) => SetVal::Other,
            UdValue::Str(s) => SetVal::Bytes(s.into_bytes()),
            UdValue::Bytes(b) => SetVal::Bytes(b),
            UdValue::Handle(h) => SetVal::Node(h as u32),
            _ => SetVal::Other,
        };
        let h = self.h;
        Some(match with_engine(|e| e.lua_set_node_field(h, key, v, false)) {
            Ok(r) => r,
            Err(e) => Err(e),
        })
    }

    fn set_int_field(&mut self, key: i64, value: UdValue) -> Option<Result<(), String>> {
        let val = match value {
            UdValue::Integer(i) => i,
            UdValue::Number(n) => n as i64,
            _ => 0,
        };
        let h = self.h;
        with_engine(|e| {
            if val as i32 == UNUSED_ATTRIBUTE {
                e.lua_nodes.unset_attribute(h, key as i32, UNUSED_ATTRIBUTE);
            } else {
                e.lua_nodes.set_attribute(h, key as i32, val as i32);
            }
        })
        .ok();
        Some(Ok(()))
    }

    fn lua_tostring(&self) -> Option<String> {
        Some(with_engine(|e| e.lua_nodes_tostring(self.h, "node")).unwrap_or_default())
    }

    fn lua_eq(&self, other: &dyn UserDataTrait) -> Option<bool> {
        other.as_any().downcast_ref::<NodeUd>().map(|o| o.h == self.h)
    }

    fn handle_id(&self) -> Option<i64> {
        Some(i64::from(self.h))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl Engine {
    /// `setfield`; `direct` selects the integer form of node and attribute
    /// values.
    fn lua_set_node_field(&mut self, n: u32, name: &str, v: SetVal, _direct: bool) -> Result<(), String> {
        if !self.lua_nodes.valid(n) {
            return Ok(());
        }
        // `mark` and `write` take token lists
        let (id, sub) = (self.lua_nodes.id(n), self.lua_nodes.subtype(n));
        if (id == MARK && name == "mark") || (id == WHATSIT && sub == ws::WRITE && matches!(name, "data" | "value")) {
            if let SetVal::Bytes(b) = &v {
                let toks = self.lua_text_tokens(b);
                self.lua_nodes.node_mut(n).ext.get_or_insert_with(Default::default).toks = toks;
            }
            return Ok(());
        }
        self.lua_nodes.set_field(n, name, v)
    }

    /// A character-string as a token list (catcode 12, spaces 10).
    fn lua_text_tokens(&self, text: &[u8]) -> Vec<crate::token::Token> {
        text.iter().map(|&b| if b == b' ' { crate::token::Token::space() } else { crate::token::Token::other(b) }).collect()
    }

    pub(crate) fn lua_nodes_tostring(&self, n: u32, tag: &str) -> String {
        let s = &self.lua_nodes;
        if !s.valid(n) {
            return String::new();
        }
        let node = s.node(n);
        let a = if node.prev != 0 && node.id != ATTRIBUTE { format!("{:>6}", node.prev) } else { "   nil".to_string() };
        let v = if node.next != 0 { format!("{:>6}", node.next) } else { "   nil".to_string() };
        format!(
            "<{} {} < {:>6} > {} : {} {}>",
            tag,
            a,
            n,
            v,
            type_info(node.id).map_or("unknown", |t| t.name),
            node.subtype
        )
    }
}

macro_rules! nat {
    ($lua:expr, $tbl:expr, $name:literal, $f:expr) => {
        $tbl.set($name, $lua.create_function($f).map_err(|e| format!("{}: {e:?}", $name))?)
            .map_err(|e| format!("{}: {e:?}", $name))?
    };
}

type Ret = Result<Variadic<UdValue>, String>;

fn ret(vals: Vec<UdValue>) -> Ret {
    Ok(Variadic(vals))
}

fn opt_node(h: u32) -> UdValue {
    if h == 0 { UdValue::Nil } else { UdValue::Integer(i64::from(h)) }
}

pub(crate) fn handle32(v: Option<i64>) -> u32 {
    v.unwrap_or(0) as u32
}

fn rnd(v: Option<f64>) -> i64 {
    v.map_or(0, |n| (n + 0.5).floor() as i64)
}

/// a value usable as `lua_tointeger` argument
pub(crate) fn value_int(v: &Value) -> i64 {
    if let Some(i) = v.as_integer() {
        i
    } else if let Some(n) = v.as_number() {
        if n.fract() == 0.0 { n as i64 } else { 0 }
    } else if let Some(s) = v.as_string() {
        s.trim().parse::<i64>().unwrap_or(0)
    } else {
        0
    }
}

pub(crate) fn value_num(v: &Value) -> f64 {
    if let Some(n) = v.as_number() {
        n
    } else if let Some(i) = v.as_integer() {
        i as f64
    } else if let Some(s) = v.as_string() {
        s.trim().parse::<f64>().unwrap_or(0.0)
    } else {
        0.0
    }
}

fn is_num(v: Option<&Value>) -> bool {
    v.is_some_and(|v| v.as_integer().is_some() || v.as_number().is_some())
}

fn vround(v: &Value) -> i64 {
    (value_num(v) + 0.5).floor() as i64
}

pub(crate) fn value_bytes(v: &Value) -> Option<Vec<u8>> {
    v.as_string_handle().map(|s| s.to_bytes())
}

fn is_truthy(v: Option<&Value>) -> bool {
    v.is_some_and(|v| !v.is_nil() && v.as_boolean() != Some(false))
}

fn dir_string(d: i32) -> UdValue {
    if (0..4).contains(&d) { UdValue::Str(DIR_NAMES[d as usize].to_string()) } else { UdValue::Nil }
}

/// Register the natives; returns the table the Lua side consumes.
pub(crate) fn install(lua: &mut Lua) -> Result<LuaTable, String> {
    let n: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;

    // ---- identity, links ----
    nat!(lua, n, "getid", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            e.lua_nodes.valid(h).then(|| i64::from(e.lua_nodes.id(h)))
        })
    });
    nat!(lua, n, "getsubtype", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            e.lua_nodes.valid(h).then(|| i64::from(e.lua_nodes.subtype(h)))
        })
    });
    nat!(lua, n, "setsubtype", |h: Option<i64>, v: Option<i64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if let (true, Some(v)) = (e.lua_nodes.valid(h), v) {
                e.lua_nodes.node_mut(h).subtype = v as u16;
            }
        })
    });
    nat!(lua, n, "getnext", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return None;
            }
            let x = e.lua_nodes.next(h);
            (x != 0).then_some(i64::from(x))
        })
    });
    nat!(lua, n, "getprev", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return None;
            }
            let x = e.lua_nodes.prev(h);
            (x != 0).then_some(i64::from(x))
        })
    });
    nat!(lua, n, "getboth", |h: Option<i64>| -> Result<(Option<i64>, Option<i64>), String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return (None, None);
            }
            let (p, x) = (e.lua_nodes.prev(h), e.lua_nodes.next(h));
            ((p != 0).then_some(i64::from(p)), (x != 0).then_some(i64::from(x)))
        })
    });
    nat!(lua, n, "setnext", |h: Option<i64>, v: Option<f64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if e.lua_nodes.valid(h) {
                e.lua_nodes.node_mut(h).next = v.map_or(0, |v| v as u32);
            }
        })
    });
    nat!(lua, n, "setprev", |h: Option<i64>, v: Option<f64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if e.lua_nodes.valid(h) {
                e.lua_nodes.node_mut(h).prev = v.map_or(0, |v| v as u32);
            }
        })
    });
    nat!(lua, n, "setboth", |h: Option<i64>, p: Option<f64>, x: Option<f64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if e.lua_nodes.valid(h) {
                let nd = e.lua_nodes.node_mut(h);
                nd.prev = p.map_or(0, |v| v as u32);
                nd.next = x.map_or(0, |v| v as u32);
            }
        })
    });
    nat!(lua, n, "setlink", |args: Variadic<Value>| -> Result<Option<i64>, String> {
        let vals: Vec<Option<u32>> = args
            .iter()
            .map(|v| if is_num(Some(v)) { Some(value_int(v) as u32) } else { None })
            .collect();
        with_engine(|e| {
            let s = &mut e.lua_nodes;
            let (mut head, mut tail) = (0u32, 0u32);
            for (i, v) in vals.iter().enumerate() {
                match v {
                    Some(c) => {
                        let c = *c;
                        if c != tail {
                            if tail != 0 {
                                s.node_mut(tail).next = c;
                                s.node_mut(c).prev = tail;
                            } else if i > 0 {
                                s.node_mut(c).prev = 0;
                            }
                            tail = c;
                            if head == 0 {
                                head = tail;
                            }
                        }
                    }
                    None => {
                        if tail != 0 {
                            s.node_mut(tail).next = 0;
                        }
                    }
                }
            }
            (head != 0).then_some(i64::from(head))
        })
    });
    nat!(lua, n, "setsplit", |l: Option<i64>, r: Option<i64>| -> Result<(), String> {
        with_engine(|e| {
            let (l, r) = (handle32(l), handle32(r));
            if l == 0 || r == 0 {
                return;
            }
            let s = &mut e.lua_nodes;
            if l != r {
                let ln = s.next(l);
                if ln != 0 {
                    s.node_mut(ln).prev = 0;
                }
                let rp = s.prev(r);
                if rp != 0 {
                    s.node_mut(rp).next = 0;
                }
            }
            s.node_mut(l).next = 0;
            s.node_mut(r).prev = 0;
        })
    });

    // ---- creation, release, copies ----
    nat!(lua, n, "new", |id: Value, sub: Option<Value>| -> Result<i64, String> {
        let idv: Result<u8, String> = if let Some(i) = id.as_integer() {
            if type_info(i as u8).is_some() && (0..=255).contains(&i) {
                Ok(i as u8)
            } else {
                Err("invalid node id for creating new node".to_string())
            }
        } else if let Some(b) = value_bytes(&id) {
            type_by_name(&b).map(|t| t.id).ok_or_else(|| "invalid node id for creating new node".to_string())
        } else {
            Err("invalid node id for creating new node".to_string())
        };
        let idv = idv?;
        let subv: Result<u16, String> = if idv == WHATSIT {
            let r = match &sub {
                Some(s) => {
                    if let Some(i) = s.as_integer() {
                        whatsit_info(i as u16).filter(|_| (0..=255).contains(&i)).map(|w| u16::from(w.id))
                    } else {
                        value_bytes(s).and_then(|b| whatsit_by_name(&b))
                    }
                }
                None => None,
            };
            r.ok_or_else(|| "creating a whatsit requires the subtype number as a second argument".to_string())
        } else {
            match &sub {
                Some(s) if s.as_integer().is_some() => Ok(value_int(s) as u16),
                Some(s) => match value_bytes(s) {
                    Some(b) => {
                        let t = type_info(idv);
                        Ok(t.and_then(|t| t.subtypes.iter().find(|(_, n)| n.as_bytes() == b.as_slice()).map(|(i, _)| *i)).unwrap_or(0))
                    }
                    None => Ok(0),
                },
                None => Ok(0),
            }
        };
        let subv = subv?;
        with_engine(|e| i64::from(e.lua_new_node(idv, subv)))
    });
    nat!(lua, n, "free", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return None;
            }
            let nx = e.lua_nodes.next(h);
            e.lua_flush_node(h);
            (nx != 0).then_some(i64::from(nx))
        })
    });
    nat!(lua, n, "flush_node", |h: Option<i64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if e.lua_nodes.valid(h) {
                e.lua_flush_node(h);
            }
        })
    });
    nat!(lua, n, "flush_list", |h: Option<i64>| -> Result<(), String> {
        with_engine(|e| {
            let mut h = handle32(h);
            while e.lua_nodes.valid(h) {
                let nx = e.lua_nodes.next(h);
                e.lua_flush_node(h);
                h = nx;
            }
        })
    });
    nat!(lua, n, "copy", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return None;
            }
            let c = e.lua_copy_node(h);
            Some(i64::from(c))
        })
    });
    nat!(lua, n, "copy_list", |h: Option<i64>, stop: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let (h, stop) = (handle32(h), handle32(stop));
            if !e.lua_nodes.valid(h) {
                return None;
            }
            let c = e.lua_copy_range(h, stop);
            (c != 0).then_some(i64::from(c))
        })
    });
    nat!(lua, n, "remove", |head: Option<i64>, cur: Option<i64>| -> Result<(Option<i64>, Option<i64>), String> {
        with_engine(|e| {
            let (head, cur) = (handle32(head), handle32(cur));
            if head != 0 && !e.lua_nodes.valid(head) {
                return Ok((None, None));
            }
            let (h, c) = e.lua_nodes.remove(head, cur, true)?;
            Ok(((h != 0).then_some(i64::from(h)), (c != 0).then_some(i64::from(c))))
        })?
    });
    nat!(lua, n, "insert_before", |head: Option<i64>, cur: Option<i64>, node: Option<i64>| -> Result<(Option<i64>, Option<i64>), String> {
        with_engine(|e| {
            let (head, cur, nn) = (handle32(head), handle32(cur), handle32(node));
            if nn == 0 {
                return Ok((Some(i64::from(head)).filter(|h| *h != 0), Some(i64::from(cur)).filter(|c| *c != 0)));
            }
            let (h, n) = e.lua_nodes.insert_before(head, cur, nn)?;
            Ok((Some(i64::from(h)), Some(i64::from(n))))
        })?
    });
    nat!(lua, n, "insert_after", |head: Option<i64>, cur: Option<i64>, node: Option<i64>| -> Result<(Option<i64>, Option<i64>), String> {
        with_engine(|e| {
            let (head, cur, nn) = (handle32(head), handle32(cur), handle32(node));
            if nn == 0 {
                return (Some(i64::from(head)).filter(|h| *h != 0), Some(i64::from(cur)).filter(|c| *c != 0));
            }
            let (h, n) = e.lua_nodes.insert_after(head, cur, nn);
            (Some(i64::from(h)), Some(i64::from(n)))
        })
    });
    nat!(lua, n, "slide", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return None;
            }
            Some(i64::from(e.lua_nodes.slide(h)))
        })
    });
    nat!(lua, n, "tail", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return None;
            }
            Some(i64::from(e.lua_nodes.tail_of(h)))
        })
    });
    nat!(lua, n, "end_of_math", |h: Option<i64>| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return Variadic(vec![]);
            }
            let m = e.lua_nodes.end_of_math(h);
            Variadic(if m != 0 { vec![UdValue::Integer(i64::from(m))] } else { vec![] })
        })
    });
    nat!(lua, n, "length", |h: Option<i64>, stop: Option<i64>| -> Result<i64, String> {
        with_engine(|e| {
            let (h, stop) = (handle32(h), handle32(stop));
            if !e.lua_nodes.valid(h) {
                return 0;
            }
            e.lua_nodes.count(-1, h, stop)
        })
    });
    nat!(lua, n, "count", |id: Option<i64>, h: Option<i64>, stop: Option<i64>| -> Result<i64, String> {
        with_engine(|e| {
            let (h, stop) = (handle32(h), handle32(stop));
            if !e.lua_nodes.valid(h) {
                return 0;
            }
            e.lua_nodes.count(id.unwrap_or(0), h, stop)
        })
    });

    // ---- fields ----
    nat!(lua, n, "getfield", |h: Option<i64>, key: Value| -> Result<UdValue, String> {
        let hh = handle32(h);
        if let Some(i) = key.as_integer() {
            return with_engine(|e| {
                let s = &e.lua_nodes;
                if !s.valid(hh) || !has_attr_type(s.id(hh), s.subtype(hh)) {
                    return UdValue::Nil;
                }
                let v = s.has_attribute(hh, i as i32, UNUSED_ATTRIBUTE);
                if v > UNUSED_ATTRIBUTE { UdValue::Integer(i64::from(v)) } else { UdValue::Nil }
            });
        }
        let Some(name) = key.as_string() else { return Ok(UdValue::Nil) };
        with_engine(|e| {
            // node 0 is the zero glue spec of LuaTeX's memory
            if hh == 0 && name == "id" {
                return UdValue::Integer(i64::from(GLUE_SPEC));
            }
            if name == "subtype" && e.lua_nodes.valid(hh) && e.lua_nodes.id(hh) == GLUE_SPEC {
                return UdValue::Integer(0);
            }
            to_ud(e.lua_node_field(hh, &name), true)
        })
    });
    nat!(lua, n, "getfield_ud", |v: Option<Value>, key: Value| -> Result<UdValue, String> {
        let hh = v.as_ref().map_or(0, node_of);
        if let Some(i) = key.as_integer() {
            return with_engine(|e| {
                let s = &e.lua_nodes;
                if !s.valid(hh) || !has_attr_type(s.id(hh), s.subtype(hh)) {
                    return UdValue::Nil;
                }
                let v = s.has_attribute(hh, i as i32, UNUSED_ATTRIBUTE);
                if v > UNUSED_ATTRIBUTE { UdValue::Integer(i64::from(v)) } else { UdValue::Nil }
            });
        }
        let Some(name) = key.as_string() else { return Ok(UdValue::Nil) };
        with_engine(|e| to_ud(e.lua_node_field(hh, &name), false))
    });
    nat!(lua, n, "setfield", |args: Variadic<Value>| -> Result<(), String> {
        set_field_native(&args, true)
    });
    nat!(lua, n, "setfield_ud", |args: Variadic<Value>| -> Result<(), String> {
        set_field_native(&args, false)
    });
    nat!(lua, n, "has_field", |h: Option<i64>, key: Option<String>| -> Result<bool, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return false;
            }
            let Some(k) = key else { return false };
            let (id, sub) = (e.lua_nodes.id(h), e.lua_nodes.subtype(h));
            match k.as_str() {
                "next" | "id" => true,
                "subtype" => id != ATTRIBUTE && id != GLUE_SPEC && id != ATTRIBUTE_LIST,
                "attr" => has_attr_type(id, sub),
                "prev" => id != ATTRIBUTE && id != GLUE_SPEC && id != ATTRIBUTE_LIST,
                other => {
                    let k = if other == "list" { "head" } else { other };
                    fields_of(id, sub).iter().any(|(f, _)| *f == k)
                }
            }
        })
    });

    // ---- attributes ----
    nat!(lua, n, "has_attribute", |h: Option<i64>, id: Option<i64>, val: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return None;
            }
            let v = e.lua_nodes.has_attribute(h, id.unwrap_or(0) as i32, val.map_or(UNUSED_ATTRIBUTE, |v| v as i32));
            (v > UNUSED_ATTRIBUTE).then_some(i64::from(v))
        })
    });
    nat!(lua, n, "get_attribute", |h: Option<i64>, id: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) || !has_attr_type(e.lua_nodes.id(h), e.lua_nodes.subtype(h)) {
                return None;
            }
            let i = id.unwrap_or(0) as i32;
            let mut p = e.lua_nodes.attr_first(e.lua_nodes.node(h).attr);
            while p != 0 {
                let a = e.lua_nodes.node(p);
                if a.f[0] == i {
                    return (a.f[1] != UNUSED_ATTRIBUTE).then_some(i64::from(a.f[1]));
                } else if a.f[0] > i {
                    return None;
                }
                p = a.next;
            }
            None
        })
    });
    nat!(lua, n, "find_attribute", |h: Option<i64>, id: Option<i64>| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| {
            let mut c = handle32(h);
            let i = id.unwrap_or(0) as i32;
            while e.lua_nodes.valid(c) {
                if has_attr_type(e.lua_nodes.id(c), e.lua_nodes.subtype(c)) {
                    let mut p = e.lua_nodes.attr_first(e.lua_nodes.node(c).attr);
                    while p != 0 {
                        let a = e.lua_nodes.node(p);
                        if a.f[0] == i {
                            if a.f[1] != UNUSED_ATTRIBUTE {
                                return Variadic(vec![UdValue::Integer(i64::from(a.f[1])), UdValue::Integer(i64::from(c))]);
                            }
                            break;
                        } else if a.f[0] > i {
                            break;
                        }
                        p = a.next;
                    }
                }
                c = e.lua_nodes.next(c);
            }
            Variadic(vec![])
        })
    });
    nat!(lua, n, "set_attribute", |args: Variadic<Value>| -> Result<(), String> {
        let h = args.first().map_or(0, |v| value_int(v) as u32);
        if h == 0 {
            return Ok(());
        }
        if args.len() != 3 {
            return Err("incorrect number of arguments".to_string());
        }
        let (i, val) = (value_int(&args[1]) as i32, value_int(&args[2]) as i32);
        with_engine(|e| {
            if !e.lua_nodes.valid(h) {
                return;
            }
            if val == UNUSED_ATTRIBUTE {
                e.lua_nodes.unset_attribute(h, i, val);
            } else {
                e.lua_nodes.set_attribute(h, i, val);
            }
        })
    });
    nat!(lua, n, "unset_attribute", |h: Option<i64>, id: f64, val: Option<f64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return None;
            }
            let r = e.lua_nodes.unset_attribute(h, id as i32, val.map_or(UNUSED_ATTRIBUTE, |v| v as i32));
            (r > UNUSED_ATTRIBUTE).then_some(i64::from(r))
        })
    });
    nat!(lua, n, "current_attr", || -> Result<Option<i64>, String> {
        with_engine(|e| {
            let regs = e.lua_attribute_registers();
            let l = e.lua_nodes.current_attr_list(&regs);
            (l != 0).then_some(i64::from(l))
        })
    });
    nat!(lua, n, "getattributelist", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) || !has_attr_type(e.lua_nodes.id(h), e.lua_nodes.subtype(h)) {
                return None;
            }
            let a = e.lua_nodes.node(h).attr;
            (a != 0).then_some(i64::from(a))
        })
    });
    nat!(lua, n, "setattributelist", |h: Option<i64>, v: Option<Value>| -> Result<(), String> {
        let kind = match &v {
            Some(v) if v.as_integer().is_some() => Some(value_int(v) as u32),
            Some(v) if v.as_boolean() == Some(true) => Some(u32::MAX),
            _ => None,
        };
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) || !has_attr_type(e.lua_nodes.id(h), e.lua_nodes.subtype(h)) {
                return;
            }
            let new = match kind {
                Some(u32::MAX) => {
                    let regs = e.lua_attribute_registers();
                    e.lua_nodes.current_attr_list(&regs)
                }
                Some(a) if e.lua_nodes.valid(a) => {
                    if e.lua_nodes.id(a) == ATTRIBUTE_LIST {
                        a
                    } else if has_attr_type(e.lua_nodes.id(a), e.lua_nodes.subtype(a)) {
                        e.lua_nodes.node(a).attr
                    } else {
                        0
                    }
                }
                _ => 0,
            };
            e.lua_nodes.reassign_attr(h, new);
        })
    });

    // ---- scalar accessors ----
    install_accessors(lua, &n)?;
    install_lists(lua, &n)?;
    Ok(n)
}

fn set_field_native(args: &[Value], direct: bool) -> Result<(), String> {
    let h = args.first().map_or(0, |v| if direct { value_int(v) as u32 } else { node_of(v) });
    let Some(key) = args.get(1) else { return Ok(()) };
    if let Some(i) = key.as_integer() {
        if args.len() != 3 {
            return Err("incorrect number of arguments".to_string());
        }
        let val = value_int(&args[2]) as i32;
        return with_engine(|e| {
            if !e.lua_nodes.valid(h) {
                return;
            }
            if val == UNUSED_ATTRIBUTE {
                e.lua_nodes.unset_attribute(h, i as i32, val);
            } else {
                e.lua_nodes.set_attribute(h, i as i32, val);
            }
        });
    }
    let Some(name) = key.as_string() else { return Ok(()) };
    let v = match args.get(2) {
        None => SetVal::Nil,
        Some(v) => {
            if let Some(i) = v.as_integer() {
                SetVal::Int(i)
            } else if let Some(n) = v.as_number() {
                SetVal::Num(n)
            } else if let Some(b) = value_bytes(v) {
                SetVal::Bytes(b)
            } else if v.is_nil() {
                SetVal::Nil
            } else if !direct && node_of(v) != 0 {
                SetVal::Node(node_of(v))
            } else {
                SetVal::Other
            }
        }
    };
    with_engine(|e| e.lua_set_node_field(h, &name, v, direct))?
}

/// The handle of a userdata node value (0 otherwise).
fn node_of(v: &Value) -> u32 {
    v.as_userdata::<NodeUd>().and_then(|u| u.borrow().ok().map(|b| b.h)).unwrap_or(0)
}

fn install_accessors(lua: &mut Lua, n: &LuaTable) -> Result<(), String> {
    macro_rules! get_set {
        ($get:literal, $set:literal, |$s:ident, $h:ident| $getter:expr, |$s2:ident, $h2:ident, $v:ident| $setter:expr) => {
            nat!(lua, n, $get, |h: Option<i64>| -> Result<Option<i64>, String> {
                with_engine(|e| {
                    let $h = handle32(h);
                    let $s = &e.lua_nodes;
                    if !$s.valid($h) {
                        return None;
                    }
                    $getter
                })
            });
            nat!(lua, n, $set, |h: Option<i64>, v: Option<f64>| -> Result<(), String> {
                with_engine(|e| {
                    let $h2 = handle32(h);
                    let $s2 = &mut e.lua_nodes;
                    if !$s2.valid($h2) {
                        return;
                    }
                    let $v: Option<f64> = v;
                    $setter
                })
            });
        };
    }

    get_set!("getchar", "setchar",
        |s, h| match s.id(h) {
            GLYPH | MATH_CHAR | MATH_TEXT_CHAR => Some(i64::from(s.node(h).f[if s.id(h) == GLYPH { 0 } else { 1 }])),
            DELIM => Some(i64::from(s.node(h).f[1])),
            _ => None,
        },
        |s, h, v| {
            let Some(v) = v else { return };
            let id = s.id(h);
            let slot = match id {
                GLYPH => 0,
                MATH_CHAR | MATH_TEXT_CHAR | DELIM => 1,
                _ => return,
            };
            s.node_mut(h).f[slot] = v as i32;
        });
    get_set!("getlang", "setlang",
        |s, h| (s.id(h) == GLYPH).then(|| i64::from(s.node(h).f[2])),
        |s, h, v| if let (GLYPH, Some(v)) = (s.id(h), v) { s.node_mut(h).f[2] = v as i32 });
    get_set!("getexpansion", "setexpansion",
        |s, h| match s.id(h) {
            GLYPH => Some(i64::from(s.node(h).f[12])),
            KERN => Some(i64::from(s.node(h).f[1])),
            _ => None,
        },
        |s, h, v| {
            let e = v.map_or(0, |v| v as i32);
            match s.id(h) {
                GLYPH => s.node_mut(h).f[12] = e,
                KERN => s.node_mut(h).f[1] = e,
                _ => {}
            }
        });
    get_set!("getpenalty", "setpenalty",
        |s, h| match s.id(h) {
            PENALTY => Some(i64::from(s.node(h).f[0])),
            DISC => Some(i64::from(s.node(h).f[3])),
            _ => None,
        },
        |s, h, v| match s.id(h) {
            PENALTY => s.node_mut(h).f[0] = v.map_or(0, |v| v as i32),
            DISC => {
                if let Some(v) = v {
                    s.node_mut(h).f[3] = v as i32;
                }
            }
            _ => {}
        });
    get_set!("getnucleus", "setnucleus",
        |s, h| matches!(s.id(h), NOAD | ACCENT | RADICAL).then(|| i64::from(s.node(h).f[0])).filter(|x| *x != 0),
        |s, h, v| if matches!(s.id(h), NOAD | ACCENT | RADICAL) { s.node_mut(h).f[0] = v.map_or(0, |v| v as i32) });
    get_set!("getsub", "setsub",
        |s, h| matches!(s.id(h), NOAD | ACCENT | RADICAL).then(|| i64::from(s.node(h).f[1])).filter(|x| *x != 0),
        |s, h, v| if matches!(s.id(h), NOAD | ACCENT | RADICAL) { s.node_mut(h).f[1] = v.map_or(0, |v| v as i32) });
    get_set!("getsup", "setsup",
        |s, h| matches!(s.id(h), NOAD | ACCENT | RADICAL).then(|| i64::from(s.node(h).f[2])).filter(|x| *x != 0),
        |s, h, v| if matches!(s.id(h), NOAD | ACCENT | RADICAL) { s.node_mut(h).f[2] = v.map_or(0, |v| v as i32) });
    get_set!("getshift", "setshift",
        |s, h| matches!(s.id(h), HLIST | VLIST).then(|| i64::from(s.node(h).f[4])),
        |s, h, v| if matches!(s.id(h), HLIST | VLIST) { s.node_mut(h).f[4] = v.map_or(0, |v| (v + 0.5).floor() as i32) });
    get_set!("getleader", "setleader",
        |s, h| (s.id(h) == GLUE).then(|| i64::from(s.node(h).f[0])).filter(|x| *x != 0),
        |s, h, v| if s.id(h) == GLUE { s.node_mut(h).f[0] = v.map_or(0, |v| v as i32) });
    get_set!("getcomponents", "setcomponents",
        |s, h| (s.id(h) == GLYPH).then(|| i64::from(s.node(h).f[6])).filter(|x| *x != 0),
        |s, h, v| if s.id(h) == GLYPH { s.node_mut(h).f[6] = v.map_or(0, |v| v as i32) });

    // font and family
    nat!(lua, n, "getfont", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return None;
            }
            let node = e.lua_nodes.node(h);
            match node.id {
                GLYPH => Some(i64::from(node.f[1])),
                MATH_CHAR | MATH_TEXT_CHAR => Some(e.lua_family_font(node.f[0], 0)),
                DELIM => Some(e.lua_family_font(node.f[0], 0)),
                _ => None,
            }
        })
    });
    nat!(lua, n, "setfont", |h: Option<i64>, f: Option<i64>, c: Option<i64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if e.lua_nodes.valid(h) && e.lua_nodes.id(h) == GLYPH {
                let nd = e.lua_nodes.node_mut(h);
                nd.f[1] = f.unwrap_or(0) as i32;
                if let Some(c) = c {
                    nd.f[0] = c as i32;
                }
            }
        })
    });
    nat!(lua, n, "getfam", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return None;
            }
            let node = e.lua_nodes.node(h);
            match node.id {
                MATH_CHAR | MATH_TEXT_CHAR => Some(i64::from(node.f[0])),
                DELIM => Some(i64::from(node.f[0])),
                FRACTION => Some(i64::from(node.f[6])),
                NOAD => Some(i64::from(node.f[3])),
                _ => None,
            }
        })
    });
    nat!(lua, n, "setfam", |h: Option<i64>, v: Option<i64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            let Some(v) = v else { return };
            if !e.lua_nodes.valid(h) {
                return;
            }
            let nd = e.lua_nodes.node_mut(h);
            match nd.id {
                MATH_CHAR | MATH_TEXT_CHAR | DELIM => nd.f[0] = v as i32,
                FRACTION => nd.f[6] = v as i32,
                NOAD => nd.f[3] = v as i32,
                _ => {}
            }
        })
    });

    // kern
    nat!(lua, n, "getkern", |h: Option<i64>, exp: Option<Value>| -> Result<Variadic<UdValue>, String> {
        // the C code tests `lua_toboolean(L, 2)` after pushing its result, so
        // without a second argument it sees the pushed number: always true
        let want_exp = exp.as_ref().map_or(true, |v| is_truthy(Some(v)));
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return Variadic(vec![UdValue::Nil]);
            }
            let node = e.lua_nodes.node(h);
            match node.id {
                KERN => {
                    let mut v = vec![UdValue::Number(f64::from(node.f[0]))];
                    if want_exp {
                        v.push(UdValue::Integer(i64::from(node.f[1])));
                    }
                    Variadic(v)
                }
                MARGIN_KERN => Variadic(vec![UdValue::Integer(i64::from(node.f[0]))]),
                MATH => Variadic(vec![UdValue::Integer(i64::from(node.f[0]))]),
                _ => Variadic(vec![UdValue::Nil]),
            }
        })
    });
    nat!(lua, n, "setkern", |h: Option<i64>, v: Option<f64>, sub: Option<i64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return;
            }
            let nd = e.lua_nodes.node_mut(h);
            match nd.id {
                KERN | MARGIN_KERN => {
                    nd.f[0] = rnd(v) as i32;
                    if let Some(s) = sub {
                        nd.subtype = s as u16;
                    }
                }
                MATH => nd.f[0] = rnd(v) as i32,
                _ => {}
            }
        })
    });

    // width / height / depth
    nat!(lua, n, "getwidth", |h: Option<i64>, exp: Option<Value>| -> Result<Variadic<UdValue>, String> {
        let want_exp = exp.as_ref().map_or(true, |v| is_truthy(Some(v)));
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return Variadic(vec![UdValue::Nil]);
            }
            let node = e.lua_nodes.node(h);
            match node.id {
                HLIST | VLIST | RULE | UNSET => Variadic(vec![UdValue::Integer(i64::from(node.f[0]))]),
                GLYPH => {
                    let (w, _, _) = glyph_dimensions(&e.eqtb.fonts, node.f[1], node.f[0]);
                    let mut v = vec![UdValue::Number(f64::from(w))];
                    if want_exp {
                        v.push(UdValue::Integer(i64::from(node.f[12])));
                    }
                    Variadic(v)
                }
                GLUE => Variadic(vec![UdValue::Integer(i64::from(node.f[1]))]),
                MATH => Variadic(vec![UdValue::Integer(i64::from(node.f[1]))]),
                GLUE_SPEC => Variadic(vec![UdValue::Integer(i64::from(node.f[0]))]),
                INS => Variadic(vec![UdValue::Integer(i64::from(node.f[INS_GLUE]))]),
                KERN => {
                    let mut v = vec![UdValue::Integer(i64::from(node.f[0]))];
                    if want_exp {
                        v.push(UdValue::Integer(i64::from(node.f[1])));
                    }
                    Variadic(v)
                }
                MARGIN_KERN => Variadic(vec![UdValue::Integer(i64::from(node.f[0]))]),
                _ => Variadic(vec![UdValue::Nil]),
            }
        })
    });
    nat!(lua, n, "setwidth", |h: Option<i64>, v: Option<f64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return;
            }
            let nd = e.lua_nodes.node_mut(h);
            let slot = match nd.id {
                HLIST | VLIST | RULE | UNSET | GLUE_SPEC | MARGIN_KERN | FRACTION => 0,
                INS => INS_GLUE,
                GLUE | MATH => 1,
                KERN => 0,
                RADICAL => 5,
                _ => return,
            };
            let slot = match nd.id {
                FRACTION => 0,
                _ => slot,
            };
            nd.f[slot] = rnd(v) as i32;
        })
    });
    nat!(lua, n, "getheight", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return None;
            }
            let node = e.lua_nodes.node(h);
            match node.id {
                HLIST | VLIST | RULE | UNSET => Some(i64::from(node.f[2])),
                INS => Some(i64::from(node.f[2])),
                GLYPH => Some(i64::from(glyph_dimensions(&e.eqtb.fonts, node.f[1], node.f[0]).1)),
                FENCE => Some(i64::from(node.f[2])),
                _ => None,
            }
        })
    });
    nat!(lua, n, "setheight", |h: Option<i64>, v: Option<f64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return;
            }
            let nd = e.lua_nodes.node_mut(h);
            match nd.id {
                HLIST | VLIST | RULE | UNSET => nd.f[2] = rnd(v) as i32,
                FENCE => nd.f[2] = rnd(v) as i32,
                _ => {}
            }
        })
    });
    nat!(lua, n, "getdepth", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return None;
            }
            let node = e.lua_nodes.node(h);
            match node.id {
                HLIST | VLIST | RULE | UNSET => Some(i64::from(node.f[1])),
                INS => Some(i64::from(node.f[1])),
                GLYPH => Some(i64::from(glyph_dimensions(&e.eqtb.fonts, node.f[1], node.f[0]).2)),
                FENCE => Some(i64::from(node.f[3])),
                _ => None,
            }
        })
    });
    nat!(lua, n, "setdepth", |h: Option<i64>, v: Option<f64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return;
            }
            let nd = e.lua_nodes.node_mut(h);
            match nd.id {
                HLIST | VLIST | RULE | UNSET => nd.f[1] = rnd(v) as i32,
                FENCE => nd.f[3] = rnd(v) as i32,
                _ => {}
            }
        })
    });
    nat!(lua, n, "getwhd", |h: Option<i64>, exp: Option<Value>| -> Result<Variadic<UdValue>, String> {
        let want_exp = is_truthy(exp.as_ref());
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return Variadic(vec![]);
            }
            let s = &e.lua_nodes;
            let t = s.node(h);
            let whd = |x: &LNode| {
                vec![
                    UdValue::Integer(i64::from(x.f[0])),
                    UdValue::Integer(i64::from(x.f[2])),
                    UdValue::Integer(i64::from(x.f[1])),
                ]
            };
            match t.id {
                HLIST | VLIST | RULE | UNSET => Variadic(whd(t)),
                GLYPH => {
                    let (w, ht, d) = glyph_dimensions(&e.eqtb.fonts, t.f[1], t.f[0]);
                    let mut v = vec![
                        UdValue::Integer(i64::from(w)),
                        UdValue::Integer(i64::from(ht)),
                        UdValue::Integer(i64::from(d)),
                    ];
                    if want_exp {
                        v.push(UdValue::Integer(i64::from(t.f[12])));
                    }
                    Variadic(v)
                }
                GLUE => {
                    let l = t.f[0] as u32;
                    if s.valid(l) && matches!(s.id(l), HLIST | VLIST | RULE) {
                        Variadic(whd(s.node(l)))
                    } else {
                        Variadic(vec![])
                    }
                }
                _ => Variadic(vec![]),
            }
        })
    });
    nat!(lua, n, "setwhd", |args: Variadic<Value>| -> Result<(), String> {
        let h = args.first().map_or(0, |v| value_int(v) as u32);
        let top = args.len();
        with_engine(|e| {
            let s = &mut e.lua_nodes;
            if !s.valid(h) {
                return;
            }
            let mut target = h;
            if s.id(target) == GLUE {
                target = s.node(target).f[0] as u32;
                if !s.valid(target) {
                    return;
                }
            }
            if !matches!(s.id(target), HLIST | VLIST | RULE | UNSET) {
                return;
            }
            for (i, slot) in [(1usize, 0usize), (2, 2), (3, 1)] {
                if top > i && is_num(args.get(i)) {
                    s.node_mut(target).f[slot] = vround(&args[i]) as i32;
                }
            }
        })
    });

    // glue
    nat!(lua, n, "getglue", |h: Option<i64>| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return Variadic(vec![]);
            }
            let t = e.lua_nodes.node(h);
            match t.id {
                // an insert's glue fields are not backed by data (zeros)
                INS => Variadic((INS_GLUE..INS_GLUE + 5).map(|i| UdValue::Integer(i64::from(t.f[i]))).collect()),
                GLUE | MATH => Variadic((1..=5).map(|i| UdValue::Integer(i64::from(t.f[i]))).collect()),
                GLUE_SPEC => Variadic((0..5).map(|i| UdValue::Integer(i64::from(t.f[i]))).collect()),
                HLIST | VLIST => {
                    Variadic(vec![
                        UdValue::Number(t.fl),
                        UdValue::Integer(i64::from(t.f[5])),
                        UdValue::Integer(i64::from(t.f[6])),
                    ])
                }
                _ => Variadic(vec![]),
            }
        })
    });
    nat!(lua, n, "setglue", |args: Variadic<Value>| -> Result<Variadic<UdValue>, String> {
        let h = args.first().map_or(0, |v| value_int(v) as u32);
        let top = args.len();
        with_engine(|e| {
            let s = &mut e.lua_nodes;
            if !s.valid(h) {
                return Variadic(vec![]);
            }
            let t = s.id(h);
            let num = |i: usize| top > i && is_num(args.get(i));
            match t {
                GLUE | GLUE_SPEC | MATH => {
                    let base = if t == GLUE_SPEC { 0 } else { 1 };
                    let nd = s.node_mut(h);
                    nd.f[base] = if num(1) { vround(&args[1]) as i32 } else { 0 };
                    nd.f[base + 1] = if num(2) { vround(&args[2]) as i32 } else { 0 };
                    nd.f[base + 2] = if num(3) { vround(&args[3]) as i32 } else { 0 };
                    nd.f[base + 3] = if num(4) { value_int(&args[4]) as i32 } else { 0 };
                    nd.f[base + 4] = if num(5) { value_int(&args[5]) as i32 } else { 0 };
                    Variadic(vec![])
                }
                HLIST | VLIST => {
                    let nd = s.node_mut(h);
                    nd.fl = if num(1) { value_num(&args[1]) } else { 0.0 };
                    nd.f[5] = if num(2) { value_int(&args[2]) as i32 } else { 0 };
                    nd.f[6] = if num(3) { value_int(&args[3]) as i32 } else { 0 };
                    Variadic(vec![])
                }
                _ => Variadic(vec![]),
            }
        })
    });
    nat!(lua, n, "is_zero_glue", |h: Option<i64>| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return Variadic(vec![]);
            }
            let t = e.lua_nodes.node(h);
            match t.id {
                GLUE | MATH => Variadic(vec![UdValue::Boolean(t.f[1] == 0 && t.f[2] == 0 && t.f[3] == 0)]),
                GLUE_SPEC => Variadic(vec![UdValue::Boolean(t.f[0] == 0 && t.f[1] == 0 && t.f[2] == 0)]),
                INS => Variadic(vec![UdValue::Boolean(t.f[INS_GLUE] == 0 && t.f[INS_GLUE + 1] == 0 && t.f[INS_GLUE + 2] == 0)]),
                HLIST | VLIST => Variadic(vec![UdValue::Boolean(t.fl == 0.0 && t.f[5] == 0 && t.f[6] == 0)]),
                _ => Variadic(vec![]),
            }
        })
    });
    nat!(lua, n, "effective_glue", |glue: Option<i64>, parent: Option<i64>, round: Option<Value>| -> Result<Option<UdValue>, String> {
        let round = is_truthy(round.as_ref());
        with_engine(|e| {
            let (g, p) = (handle32(glue), handle32(parent));
            if !e.lua_nodes.valid(g) || e.lua_nodes.id(g) != GLUE {
                return None;
            }
            let pv = e.lua_nodes.valid(p) && matches!(e.lua_nodes.id(p), HLIST | VLIST);
            if !pv {
                return Some(UdValue::Integer(i64::from(e.lua_nodes.node(g).f[1])));
            }
            let w = e.lua_nodes.effective_glue(g, p)?;
            Some(if round { UdValue::Integer(w.round() as i64) } else { UdValue::Number(w) })
        })
    });

    // dir
    nat!(lua, n, "getdir", |h: Option<i64>| -> Result<UdValue, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return UdValue::Nil;
            }
            let t = e.lua_nodes.node(h);
            match t.id {
                DIR => {
                    if (0..4).contains(&t.f[0]) {
                        UdValue::Str(format!("{}{}", if t.subtype == 1 { '-' } else { '+' }, DIR_NAMES[t.f[0] as usize]))
                    } else {
                        UdValue::Nil
                    }
                }
                HLIST | VLIST | RULE => dir_string(t.f[3]),
                LOCAL_PAR => dir_string(t.f[2]),
                _ => UdValue::Nil,
            }
        })
    });
    nat!(lua, n, "getdirection", |h: Option<i64>| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return Variadic(vec![UdValue::Nil]);
            }
            let t = e.lua_nodes.node(h);
            let d = |x: i32| if x < 0 { UdValue::Nil } else { UdValue::Integer(i64::from(x)) };
            match t.id {
                DIR => Variadic(vec![d(t.f[0]), UdValue::Boolean(t.subtype != 0)]),
                HLIST | VLIST | RULE => Variadic(vec![d(t.f[3])]),
                LOCAL_PAR => Variadic(vec![d(t.f[2])]),
                _ => Variadic(vec![UdValue::Nil]),
            }
        })
    });
    nat!(lua, n, "setdir", |h: Option<i64>, v: Option<String>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return Ok(());
            }
            let id = e.lua_nodes.id(h);
            let Some(v) = v else { return Err("Direction specifiers have to be strings".to_string()) };
            match id {
                DIR => e.lua_nodes.set_field(h, "dir", SetVal::Bytes(v.into_bytes())),
                HLIST | VLIST | RULE => e.lua_nodes.set_field(h, "dir", SetVal::Bytes(v.into_bytes())),
                LOCAL_PAR => e.lua_nodes.set_field(h, "dir", SetVal::Bytes(v.into_bytes())),
                _ => Ok(()),
            }
        })?
    });
    nat!(lua, n, "setdirection", |h: Option<i64>, v: Option<Value>, cancel: Option<Value>| -> Result<(), String> {
        let cancel = cancel.as_ref().and_then(|c| c.as_boolean());
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return Ok(());
            }
            if !matches!(e.lua_nodes.id(h), DIR | HLIST | VLIST | RULE | LOCAL_PAR) {
                return Ok(());
            }
            let Some(v) = v.as_ref().and_then(|v| v.as_integer()) else {
                return Err("Direction specifiers have to be numbers".to_string());
            };
            if !(0..4).contains(&v) {
                return Err(format!("Invalid direction value {v}"));
            }
            let nd = e.lua_nodes.node_mut(h);
            match nd.id {
                DIR => {
                    nd.f[0] = v as i32;
                    if let Some(c) = cancel {
                        nd.subtype = u16::from(c);
                    }
                }
                HLIST | VLIST | RULE => nd.f[3] = v as i32,
                LOCAL_PAR => nd.f[2] = v as i32,
                _ => {}
            }
            Ok(())
        })?
    });

    // offsets
    nat!(lua, n, "getoffsets", |h: Option<i64>| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return Variadic(vec![]);
            }
            let t = e.lua_nodes.node(h);
            match t.id {
                GLYPH => Variadic(vec![UdValue::Integer(i64::from(t.f[7])), UdValue::Integer(i64::from(t.f[8]))]),
                RULE => Variadic(vec![UdValue::Integer(i64::from(t.f[5])), UdValue::Integer(i64::from(t.f[6]))]),
                _ => Variadic(vec![]),
            }
        })
    });
    nat!(lua, n, "setoffsets", |args: Variadic<Value>| -> Result<(), String> {
        let h = args.first().map_or(0, |v| value_int(v) as u32);
        with_engine(|e| {
            let s = &mut e.lua_nodes;
            if !s.valid(h) {
                return;
            }
            let base = match s.id(h) {
                GLYPH => 7,
                RULE => 5,
                _ => return,
            };
            for i in 0..2 {
                if is_num(args.get(1 + i)) {
                    s.node_mut(h).f[base + i] = vround(&args[1 + i]) as i32;
                }
            }
        })
    });

    // data
    nat!(lua, n, "getdata", |h: Option<i64>| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return Variadic(vec![UdValue::Nil]);
            }
            let t = e.lua_nodes.node(h);
            match t.id {
                GLYPH => Variadic(vec![UdValue::Integer(i64::from(t.f[13]))]),
                BOUNDARY => Variadic(vec![UdValue::Integer(i64::from(t.f[0]))]),
                WHATSIT => match t.subtype {
                    ws::USER_DEFINED => Variadic(vec![to_ud(e.lua_nodes.get_field(h, "value", &|_, _| (0, 0, 0)), true)]),
                    ws::PDF_LITERAL | ws::PDF_LATE_LITERAL => Variadic(vec![
                        UdValue::Bytes(e.lua_nodes.ext_str(h, "data")),
                        UdValue::Integer(i64::from(t.f[0])),
                    ]),
                    ws::LATE_LUA | ws::PDF_SETMATRIX | ws::SPECIAL | ws::LATE_SPECIAL => {
                        Variadic(vec![UdValue::Bytes(e.lua_nodes.ext_str(h, "data"))])
                    }
                    ws::WRITE => Variadic(vec![UdValue::Bytes(e.lua_node_tokens(h))]),
                    _ => Variadic(vec![UdValue::Nil]),
                },
                _ => Variadic(vec![UdValue::Nil]),
            }
        })
    });
    nat!(lua, n, "setdata", |h: Option<i64>, v: Option<Value>| -> Result<(), String> {
        let (iv, bv) = match &v {
            Some(v) => (vround(v), value_bytes(v)),
            None => (0, None),
        };
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return;
            }
            let (id, sub) = (e.lua_nodes.id(h), e.lua_nodes.subtype(h));
            match id {
                GLYPH => e.lua_nodes.node_mut(h).f[13] = iv as i32,
                BOUNDARY => e.lua_nodes.node_mut(h).f[0] = iv as i32,
                WHATSIT => {
                    let _ = e.lua_set_node_field(
                        h,
                        if sub == ws::USER_DEFINED { "value" } else { "data" },
                        match (sub, bv) {
                            (ws::USER_DEFINED, Some(b)) => SetVal::Bytes(b),
                            (ws::USER_DEFINED, None) => SetVal::Int(iv),
                            (_, Some(b)) => SetVal::Bytes(b),
                            (_, None) => SetVal::Nil,
                        },
                        true,
                    );
                }
                _ => {}
            }
        })
    });
    Ok(())
}

impl Engine {
    /// `fam_fnt(fam, size)`: the font of family `fam` in the text size
    /// (`size` = 0), script (256) or scriptscript (512) size.
    pub(crate) fn lua_family_font(&self, fam: i32, size: i32) -> i64 {
        let sz = (size / 256).clamp(0, 2) as usize;
        self.eqtb.style_fonts[sz][fam.clamp(0, 255) as usize] as i64
    }

    pub(crate) fn lua_flush_node(&mut self, n: u32) {
        if self.lua_nodes.props_mode.1 {
            self.lua_clear_property(n);
        }
        self.lua_nodes.flush_node(n);
    }

    pub(crate) fn lua_copy_node(&mut self, n: u32) -> u32 {
        let c = self.lua_nodes.copy_node(n);
        if self.lua_nodes.props_mode.0 {
            self.lua_copy_property(n, c);
        }
        c
    }

    pub(crate) fn lua_copy_range(&mut self, p: u32, stop: u32) -> u32 {
        let mut head = 0;
        let mut q = 0;
        let mut p = p;
        while p != stop && p != 0 {
            let s = self.lua_copy_node(p);
            if head == 0 {
                head = s;
            } else {
                self.lua_nodes.couple(q, s);
            }
            q = s;
            p = self.lua_nodes.next(p);
        }
        head
    }

    fn lua_clear_property(&mut self, n: u32) {
        if let Some(t) = &self.lua_nodes.props {
            let _ = t.raw_set(i64::from(n), ());
        }
    }

    fn lua_copy_property(&mut self, from: u32, to: u32) {
        if let Some(t) = &self.lua_nodes.props {
            if let Ok(v) = t.raw_get::<Value>(i64::from(from)) {
                if !v.is_nil() {
                    let _ = t.raw_set(i64::from(to), v);
                }
            }
        }
    }
}

fn install_lists(lua: &mut Lua, n: &LuaTable) -> Result<(), String> {
    // lists and discretionaries
    nat!(lua, n, "getlist", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return None;
            }
            let slot = match e.lua_nodes.id(h) {
                HLIST | VLIST => sl::B_HEAD,
                SUB_BOX | SUB_MLIST | ADJUST => 0,
                INS => sl::I_HEAD,
                _ => return None,
            };
            let head = e.lua_nodes.node(h).f[slot] as u32;
            if head == 0 {
                return None;
            }
            e.lua_nodes.node_mut(head).prev = 0;
            Some(i64::from(head))
        })
    });
    nat!(lua, n, "setlist", |h: Option<i64>, v: Option<f64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return;
            }
            let slot = match e.lua_nodes.id(h) {
                HLIST | VLIST => sl::B_HEAD,
                SUB_BOX | SUB_MLIST | ADJUST => 0,
                INS => sl::I_HEAD,
                _ => return,
            };
            e.lua_nodes.node_mut(h).f[slot] = v.map_or(0, |v| v as i32);
        })
    });
    nat!(lua, n, "getdisc", |h: Option<i64>, tails: Option<Value>| -> Result<Variadic<UdValue>, String> {
        let tails = is_truthy(tails.as_ref());
        with_engine(|e| {
            let h = handle32(h);
            let s = &mut e.lua_nodes;
            if !s.valid(h) || s.id(h) != DISC {
                return Variadic(vec![]);
            }
            let f = s.node(h).f;
            let mut out = Vec::new();
            for slot in 0..3 {
                let head = f[slot] as u32;
                if head != 0 {
                    s.node_mut(head).prev = 0;
                }
                out.push(opt_node(head));
            }
            if tails {
                for slot in 0..3 {
                    let head = f[slot] as u32;
                    out.push(opt_node(s.tail_of(head)));
                }
            }
            Variadic(out)
        })
    });
    nat!(lua, n, "setdisc", |args: Variadic<Value>| -> Result<(), String> {
        let h = args.first().map_or(0, |v| value_int(v) as u32);
        let top = args.len();
        with_engine(|e| {
            let s = &mut e.lua_nodes;
            if !s.valid(h) || s.id(h) != DISC {
                return;
            }
            let get = |i: usize| if top > i { value_int(&args[i]) as i32 } else { 0 };
            let nd = s.node_mut(h);
            nd.f[0] = get(1);
            nd.f[1] = get(2);
            nd.f[2] = get(3);
            if top > 4 {
                nd.subtype = value_int(&args[4]) as u16;
            }
            if top > 5 {
                nd.f[3] = value_int(&args[5]) as i32;
            }
        })
    });
    nat!(lua, n, "flatten_discretionaries", |h: Option<i64>| -> Result<(Option<i64>, i64), String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return (None, 0);
            }
            let (head, c) = e.lua_nodes.flatten_discretionaries(h);
            ((head != 0).then_some(i64::from(head)), c)
        })
    });
    nat!(lua, n, "first_glyph", |h: Option<i64>, t: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let (h, t) = (handle32(h), handle32(t));
            if !e.lua_nodes.valid(h) {
                return None;
            }
            let g = e.lua_nodes.first_glyph(h, t);
            (g != 0).then_some(i64::from(g))
        })
    });
    nat!(lua, n, "has_glyph", |h: Option<i64>| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let g = e.lua_nodes.has_glyph(handle32(h));
            (g != 0).then_some(i64::from(g))
        })
    });
    nat!(lua, n, "is_char", |h: Option<i64>, font: Option<Value>| -> Result<Variadic<UdValue>, String> {
        let font_arg = font.as_ref().filter(|v| is_num(Some(v))).map(value_int);
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return Variadic(vec![UdValue::Nil, UdValue::Integer(0)]);
            }
            let t = e.lua_nodes.node(h);
            if t.id != GLYPH {
                return Variadic(vec![UdValue::Nil, UdValue::Integer(i64::from(t.id))]);
            }
            if t.subtype & 0xFF00 != 0 {
                return Variadic(vec![UdValue::Boolean(false)]);
            }
            match font_arg {
                Some(f) => {
                    if f != 0 && f == i64::from(t.f[1]) {
                        Variadic(vec![UdValue::Integer(i64::from(t.f[0]))])
                    } else {
                        Variadic(vec![UdValue::Boolean(false)])
                    }
                }
                None => Variadic(vec![UdValue::Integer(i64::from(t.f[0]))]),
            }
        })
    });
    nat!(lua, n, "is_glyph", |h: Option<i64>| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if !e.lua_nodes.valid(h) {
                return Variadic(vec![UdValue::Boolean(false), UdValue::Integer(0)]);
            }
            let t = e.lua_nodes.node(h);
            if t.id != GLYPH {
                Variadic(vec![UdValue::Boolean(false), UdValue::Integer(i64::from(t.id))])
            } else {
                Variadic(vec![UdValue::Integer(i64::from(t.f[0])), UdValue::Integer(i64::from(t.f[1]))])
            }
        })
    });
    nat!(lua, n, "uses_font", |h: Option<i64>, f: Option<i64>| -> Result<bool, String> {
        with_engine(|e| {
            let h = handle32(h);
            e.lua_nodes.valid(h) && e.lua_nodes.uses_font(h, f.unwrap_or(0))
        })
    });
    nat!(lua, n, "protect_glyph", |h: Option<i64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if e.lua_nodes.valid(h) {
                e.lua_nodes.protect(h, true);
            }
        })
    });
    nat!(lua, n, "unprotect_glyph", |h: Option<i64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if e.lua_nodes.valid(h) {
                e.lua_nodes.protect(h, false);
            }
        })
    });
    nat!(lua, n, "protect_glyphs", |h: Option<i64>, t: Option<i64>| -> Result<(), String> {
        with_engine(|e| {
            let (mut head, tail) = (handle32(h), handle32(t));
            while e.lua_nodes.valid(head) {
                e.lua_nodes.protect(head, true);
                if head == tail {
                    break;
                }
                head = e.lua_nodes.next(head);
            }
        })
    });
    nat!(lua, n, "unprotect_glyphs", |h: Option<i64>, t: Option<i64>| -> Result<(), String> {
        with_engine(|e| {
            let (mut head, tail) = (handle32(h), handle32(t));
            while e.lua_nodes.valid(head) {
                e.lua_nodes.protect(head, false);
                if head == tail {
                    break;
                }
                head = e.lua_nodes.next(head);
            }
        })
    });
    nat!(lua, n, "protrusion_skippable", |h: Option<i64>| -> Result<Option<bool>, String> {
        with_engine(|e| {
            let h = handle32(h);
            e.lua_nodes.valid(h).then(|| e.lua_nodes.cp_skippable(h))
        })
    });
    nat!(lua, n, "check_discretionary", |_h: Option<i64>| -> Result<(), String> { Ok(()) });
    nat!(lua, n, "check_discretionaries", |_h: Option<i64>| -> Result<(), String> { Ok(()) });
    nat!(lua, n, "usedlist", || -> Result<i64, String> {
        with_engine(|e| {
            // copies of all nodes in use, chained (texnodes.c list_node_mem_usage)
            let ids: Vec<u32> = (1..e.lua_nodes.nodes.len() as u32).filter(|&i| e.lua_nodes.valid(i)).collect();
            let mut head = 0;
            let mut prev = 0;
            for i in ids {
                let c = e.lua_copy_node(i);
                if prev == 0 {
                    head = c;
                } else {
                    e.lua_nodes.couple(prev, c);
                }
                prev = c;
            }
            i64::from(head)
        })
    });
    nat!(lua, n, "tostring", |h: Option<i64>| -> Result<Option<String>, String> {
        with_engine(|e| {
            let h = handle32(h);
            (e.lua_nodes.valid(h)).then(|| e.lua_nodes_tostring(h, "direct"))
        })
    });
    nat!(lua, n, "tostring_node", |h: Option<i64>| -> Result<Option<String>, String> {
        with_engine(|e| {
            let h = handle32(h);
            (e.lua_nodes.valid(h)).then(|| e.lua_nodes_tostring(h, "node"))
        })
    });
    nat!(lua, n, "todirect_ud", |v: Option<Value>| -> Result<Option<i64>, String> {
        Ok(v.and_then(|v| {
            let h = node_of(&v);
            (h != 0).then_some(i64::from(h))
        }))
    });
    nat!(lua, n, "is_node_ud", |v: Option<Value>| -> Result<Option<i64>, String> {
        Ok(v.and_then(|v| {
            let h = node_of(&v);
            (h != 0).then_some(i64::from(h))
        }))
    });
    nat!(lua, n, "is_protected", |h: Option<i64>| -> Result<bool, String> {
        with_engine(|e| {
            let h = handle32(h);
            e.lua_nodes.valid(h) && e.lua_nodes.node(h).subtype & 0xFF00 != 0
        })
    });
    nat!(lua, n, "tonode", |h: Option<i64>| -> Result<UdValue, String> { Ok(node_ud(handle32(h))) });
    nat!(lua, n, "fix_node_lists", |v: Option<Value>| -> Result<(), String> {
        let b = v.as_ref().and_then(|v| v.as_boolean()).or_else(|| v.as_ref().and_then(|v| v.as_integer()).map(|i| i != 0));
        with_engine(|e| {
            if let Some(b) = b {
                e.lua_nodes.fix_node_lists = b;
            }
        })
    });
    nat!(lua, n, "set_properties_mode", |a: Option<Value>, b: Option<Value>| -> Result<(), String> {
        let (a, b) = (
            a.and_then(|v| v.as_boolean()),
            b.and_then(|v| v.as_boolean()),
        );
        with_engine(|e| {
            if let Some(a) = a {
                e.lua_nodes.props_mode.0 = a;
            }
            if let Some(b) = b {
                e.lua_nodes.props_mode.1 = b;
            }
        })
    });
    nat!(lua, n, "set_properties_table", |t: LuaTable| -> Result<(), String> {
        with_engine(|e| e.lua_nodes.props = Some(t))
    });
    nat!(lua, n, "family_font", |fam: i64, size: Option<i64>| -> Result<i64, String> {
        with_engine(|e| e.lua_family_font(fam as i32, size.unwrap_or(0) as i32))
    });
    nat!(lua, n, "last_node", || -> Result<i64, String> {
        with_engine(|e| match e.cur_list.pop() {
            Some(node) => e.lua_nodes_from_engine(vec![node]),
            None => 0,
        })
    });
    nat!(lua, n, "write", |h: Option<i64>| -> Result<(), String> {
        with_engine(|e| {
            let nodes = e.lua_nodes_to_engine(i64::from(handle32(h)));
            e.cur_list.extend(nodes);
        })
    });
    nat!(lua, n, "prepend_prevdepth", |h: Option<i64>, prev: Option<i64>, ud: bool| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| e.lua_prepend_prevdepth(handle32(h), prev.unwrap_or(0) as i32, ud))
    });
    nat!(lua, n, "set_synctex_fields", |h: Option<i64>, tag: Option<i64>, line: Option<i64>| -> Result<(), String> {
        with_engine(|e| {
            let h = handle32(h);
            if e.lua_nodes.valid(h) && has_synctex(e.lua_nodes.id(h)) {
                let f = &mut e.lua_nodes.node_mut(h).f;
                if let Some(t) = tag.filter(|t| *t != 0) {
                    f[NF - 2] = t as i32;
                }
                if let Some(l) = line.filter(|l| *l != 0) {
                    f[NF - 1] = l as i32;
                }
            }
        })
    });
    nat!(lua, n, "get_synctex_fields", |h: Option<i64>| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| {
            let h = handle32(h);
            if e.lua_nodes.valid(h) && has_synctex(e.lua_nodes.id(h)) {
                let f = &e.lua_nodes.node(h).f;
                Variadic(vec![UdValue::Integer(i64::from(f[NF - 2])), UdValue::Integer(i64::from(f[NF - 1]))])
            } else {
                Variadic(vec![])
            }
        })
    });
    nat!(lua, n, "ligaturing", |h: Option<i64>, t: Option<i64>| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| e.lua_ligkern_native(LigKern::Ligaturing, h, t))
    });
    nat!(lua, n, "kerning", |h: Option<i64>, t: Option<i64>| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| e.lua_ligkern_native(LigKern::Kerning, h, t))
    });
    nat!(lua, n, "hyphenating", |h: Option<i64>, t: Option<i64>, ud: bool| -> Result<Variadic<UdValue>, String> {
        with_engine(|e| {
            let h = handle32(h);
            // lang_tex_direct_hyphenating always walks to the tail; the
            // userdata form honours a given one
            let t = match handle32(t) {
                t if ud && t != 0 => t,
                _ => e.lua_nodes.tail_of(h),
            };
            e.lk_hyphenation(h, t);
            Variadic(vec![
                UdValue::Integer(i64::from(h)),
                UdValue::Integer(i64::from(t)),
                UdValue::Boolean(true),
            ])
        })
    });
    nat!(lua, n, "mlist_to_hlist", |h: Option<i64>, style: Option<Value>, pen: Option<Value>| -> Result<Option<i64>, String> {
        // assign_math_style: numbers as they are, names by the math style list
        let style = match &style {
            Some(v) if v.as_integer().is_some() => value_int(v) as u8,
            Some(v) => match value_bytes(v) {
                Some(b) => math_style_from_name(&b).map_or(2, |s| s as u8),
                None => 0,
            },
            None => 0,
        };
        let pen = is_truthy(pen.as_ref());
        with_engine(|e| {
            let list = e.lua_nodes_to_engine(i64::from(handle32(h)));
            let out = e.lua_mlist_to_hlist(&list, style, pen);
            let head = e.lua_nodes_from_engine(out);
            (head != 0).then_some(head)
        })
    });
    // direct.getbox / direct.setbox: a box register as a node. The node is
    // tied to the register (see `lua_texnodes`): edits reach the register
    // when the Lua call ends.
    nat!(lua, n, "getbox", |k: Value| -> Result<Option<i64>, String> {
        let k = box_register(&k, "getbox")?;
        with_engine(|e| {
            let h = e.lua_box_handle(k);
            (h != 0).then_some(i64::from(h))
        })
    });
    nat!(lua, n, "setbox", |args: Variadic<Value>| -> Result<(), String> {
        let top = args.len();
        if top < 2 {
            return Err("argument must be a string or a number".to_string());
        }
        let global = top == 3 && value_bytes(&args[0]).as_deref() == Some(b"global");
        let value = &args[top - 1];
        let k = box_register(&args[top - 2], "setbox")?;
        if let Some(b) = value.as_boolean() {
            if b {
                return Ok(());
            }
            return with_engine(|e| e.lua_set_box(k, 0, global));
        }
        if value.is_nil() {
            return with_engine(|e| e.lua_set_box(k, 0, global));
        }
        let h = handle32(value.as_integer());
        with_engine(|e| {
            if h != 0 && e.lua_nodes.valid(h) && !matches!(e.lua_nodes.id(h), HLIST | VLIST) {
                let id = e.lua_nodes.id(h);
                let name = type_info(id).map_or("unknown", |t| t.name);
                return Err(format!("setbox: incompatible node type ({name})\n"));
            }
            e.lua_set_box(k, h, global);
            Ok(())
        })?
    });
    nat!(lua, n, "wrap_hpack", |args: Variadic<Value>| -> Result<Variadic<UdValue>, String> {
        crate::lua_node_pack::lua_pack(&args, true)
    });
    nat!(lua, n, "wrap_vpack", |args: Variadic<Value>| -> Result<Variadic<UdValue>, String> {
        crate::lua_node_pack::lua_pack(&args, false)
    });
    nat!(lua, n, "dimensions", |args: Variadic<Value>| -> Result<Variadic<UdValue>, String> {
        crate::lua_node_pack::lua_dimensions(&args, false)
    });
    nat!(lua, n, "rangedimensions", |args: Variadic<Value>| -> Result<Variadic<UdValue>, String> {
        crate::lua_node_pack::lua_dimensions(&args, true)
    });
    Ok(())
}



/// Install the global `node` table: the natives of [`install`] under the
/// public names of `lua_node.lua`.
pub(crate) fn install_library(lua: &mut Lua) -> Result<(), String> {
    let natives = install(lua)?;
    let data: LuaTable = lua
        .load(include_str!("lua_node_data.lua"))
        .set_name("=[ratex node data]")
        .eval()
        .map_err(|e| format!("node data: {}", lua.get_error_message(e).message()))?;
    lua.set_global("__ratex_nodelib", natives).map_err(|e| format!("{e:?}"))?;
    lua.set_global("__ratex_nodedata", data).map_err(|e| format!("{e:?}"))?;
    let node: LuaTable = lua
        .load(include_str!("lua_node.lua"))
        .set_name("=[ratex node]")
        .eval()
        .map_err(|e| format!("node library: {}", lua.get_error_message(e).message()))?;
    lua.set_global("node", node).map_err(|e| format!("{e:?}"))
}

/// Node types that carry synctex tag and line.
fn has_synctex(id: u8) -> bool {
    matches!(id, GLYPH | GLUE | KERN | HLIST | VLIST | UNSET | RULE | MATH)
}

impl Engine {
    /// `node.prepend_prevdepth` (`ud`) / `node.direct.prepend_prevdepth`:
    /// the interline glue that goes in front of box `n`. The direct form
    /// keeps LuaTeX's inverted box test.
    fn lua_prepend_prevdepth(&mut self, n: u32, prev: i32, ud: bool) -> Variadic<UdValue> {
        use crate::prim::{DimParam, GlueParam};
        if !self.lua_nodes.valid(n) {
            return Variadic(vec![UdValue::Nil]);
        }
        let id = self.lua_nodes.id(n);
        let is_box = id == HLIST || id == VLIST;
        if is_box != ud {
            return Variadic(vec![UdValue::Nil]);
        }
        let f = self.lua_nodes.node(n).f;
        let mirrored = id == HLIST && matches!(f[sl::B_DIR], 1 | 3);
        let (height, depth) = if is_box { (f[sl::B_HEIGHT], f[sl::B_DEPTH]) } else { (0, 0) };
        let result = if prev > self.ignore_depth() {
            let bs = self.eqtb.glue_params[GlueParam::BaselineSkip.idx() as usize];
            let ls = self.eqtb.glue_params[GlueParam::LineSkip.idx() as usize];
            let limit = self.eqtb.dim_params[DimParam::LineSkipLimit.idx() as usize];
            let d = if mirrored { bs.width - prev - depth } else { bs.width - prev - height };
            let p = if d < limit {
                self.import_glue(&ls, 1)
            } else {
                let g = crate::boxes::Glue { width: d, ..bs.fresh() };
                self.import_glue(&g, 2)
            };
            self.lua_nodes.node_mut(p).next = n;
            self.lua_nodes.node_mut(n).prev = p;
            p
        } else {
            n
        };
        let new_prev = if mirrored { height } else { depth };
        let first = if ud { node_ud(result) } else { UdValue::Integer(i64::from(result)) };
        Variadic(vec![first, UdValue::Integer(i64::from(new_prev))])
    }
}

#[derive(Clone, Copy)]
enum LigKern {
    Ligaturing,
    Kerning,
}

impl Engine {
    /// `font_tex_direct_ligaturing` / `font_tex_direct_kerning`: run the
    /// pass on the range `h`..`t` (the whole list from `h` when `t` is
    /// absent) behind a temporary head.
    fn lua_ligkern_native(&mut self, kind: LigKern, h: Option<i64>, t: Option<i64>) -> Variadic<UdValue> {
        let h = handle32(h);
        if !self.lua_nodes.valid(h) {
            return Variadic(vec![UdValue::Nil, UdValue::Boolean(false)]);
        }
        let t = handle32(t);
        let tmp = self.lua_new_node(TEMP, 1);
        let p = self.lua_nodes.prev(h);
        self.lua_nodes.couple(tmp, h);
        let t = match kind {
            LigKern::Ligaturing => self.lk_handle_ligaturing(tmp, t),
            LigKern::Kerning => self.lk_handle_kerning(tmp, t),
        };
        let first = self.lua_nodes.next(tmp);
        if p != 0 {
            self.lua_nodes.node_mut(p).next = first;
        }
        if first != 0 {
            self.lua_nodes.node_mut(first).prev = p;
        }
        self.lua_nodes.node_mut(tmp).next = 0;
        self.lua_nodes.flush_node(tmp);
        Variadic(vec![
            UdValue::Integer(i64::from(first)),
            if t == 0 { UdValue::Nil } else { UdValue::Integer(i64::from(t)) },
            UdValue::Boolean(true),
        ])
    }
}

/// `direct_get_box_id` + `direct_check_index_range`: a box register number.
fn box_register(v: &Value, what: &str) -> Result<u16, String> {
    let k = if let Some(k) = v.as_integer() {
        k
    } else if let Some(name) = value_bytes(v) {
        with_engine(|e| {
            let id = e.cs.lookup(&name)?;
            match e.eqtb.resolve(id)? {
                crate::eqtb::Equiv::BoxReg(i) => Some(i64::from(*i)),
                crate::eqtb::Equiv::CharDef(c) => Some(i64::from(*c)),
                crate::eqtb::Equiv::MathCharDef(c) => Some(i64::from(*c)),
                _ => None,
            }
        })?
        .unwrap_or(-1)
    } else {
        return Err("argument must be a string or a number".to_string());
    };
    u16::try_from(k).map_err(|_| format!("incorrect index in {what}"))
}
