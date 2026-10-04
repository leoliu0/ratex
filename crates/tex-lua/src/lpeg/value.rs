//! The values LPeg handles while matching: ktable entries, extra match
//! arguments, capture results. Lua strings are byte strings.

use std::rc::Rc;

use crate::{LuaValue, Value};

/// Match-time objects use rooted handles. Published pattern ktables instead
/// contain private VM edges traced by their owning userdata.
#[derive(Clone)]
pub enum V {
    Nil,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(Rc<[u8]>),
    Obj(Value),
    ObjRaw(LuaValue),
}

impl V {
    pub fn str(bytes: &[u8]) -> V {
        V::Str(Rc::from(bytes))
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, V::Nil)
    }

    /// `lua_toboolean`.
    pub fn truthy(&self) -> bool {
        !matches!(self, V::Nil | V::Bool(false))
    }

    /// `lua_isstring`: strings and numbers.
    pub fn is_stringy(&self) -> bool {
        matches!(self, V::Str(_) | V::Int(_) | V::Num(_))
    }

    /// `lua_typename(lua_type(v))`.
    pub fn type_name(&self) -> &'static str {
        match self {
            V::Nil => "nil",
            V::Bool(_) => "boolean",
            V::Int(_) | V::Num(_) => "number",
            V::Str(_) => "string",
            V::Obj(v) => v.type_name(),
            V::ObjRaw(v) => v.type_name(),
        }
    }

    /// `lua_tolstring`: the bytes of a string or of a number converted like
    /// Lua 5.3 does.
    pub fn to_bytes(&self) -> Option<std::borrow::Cow<'_, [u8]>> {
        match self {
            V::Str(s) => Some(std::borrow::Cow::Borrowed(s)),
            V::Int(i) => Some(std::borrow::Cow::Owned(i.to_string().into_bytes())),
            V::Num(n) => Some(std::borrow::Cow::Owned(number_to_string(*n).into_bytes())),
            _ => None,
        }
    }

    /// `lua_tointeger` for numbers and numeric strings (0 when not an exact
    /// integer, as LPeg's callers need).
    pub fn to_integer(&self) -> Option<i64> {
        match self {
            V::Int(i) => Some(*i),
            V::Num(n) => float_to_integer(*n),
            V::Str(s) => {
                let text = std::str::from_utf8(s).ok()?.trim();
                if let Ok(i) = text.parse::<i64>() {
                    Some(i)
                } else {
                    float_to_integer(text.parse::<f64>().ok()?)
                }
            }
            _ => None,
        }
    }

    /// `lua_rawequal` / `lua_compare(LUA_OPEQ)` for the value kinds LPeg
    /// compares (group names are strings).
    pub fn equals(&self, other: &V) -> bool {
        match (self, other) {
            (V::Nil, V::Nil) => true,
            (V::Bool(a), V::Bool(b)) => a == b,
            (V::Int(a), V::Int(b)) => a == b,
            (V::Num(a), V::Num(b)) => a == b,
            (V::Int(a), V::Num(b)) | (V::Num(b), V::Int(a)) => (*a as f64) == *b,
            (V::Str(a), V::Str(b)) => a == b,
            (V::Obj(a), V::Obj(b)) => a.to_pointer() == b.to_pointer(),
            (V::ObjRaw(a), V::ObjRaw(b)) => a.raw_ptr_repr() == b.raw_ptr_repr(),
            (V::Obj(a), V::ObjRaw(b)) | (V::ObjRaw(b), V::Obj(a)) => {
                a.to_value().raw_ptr_repr() == b.raw_ptr_repr()
            }
            _ => false,
        }
    }
}

fn float_to_integer(n: f64) -> Option<i64> {
    if n.is_finite() && n.floor() == n && n >= -9.223_372_036_854_775_808e18 && n < 9.223_372_036_854_775_808e18 {
        Some(n as i64)
    } else {
        None
    }
}

/// Lua 5.3 `tostring` of a float (`%.14g`, plus `.0` when it looks like an
/// integer).
pub fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return if n.is_sign_negative() { "-nan" } else { "nan" }.to_string();
    }
    if n.is_infinite() {
        return if n < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    let mut text = format_g14(n);
    if !text.bytes().any(|b| matches!(b, b'.' | b'e' | b'n' | b'i')) {
        text.push_str(".0");
    }
    text
}

/// C's `%.14g`.
fn format_g14(n: f64) -> String {
    if n == 0.0 {
        return if n.is_sign_negative() { "-0" } else { "0" }.to_string();
    }
    const PRECISION: i32 = 14;
    let scientific = format!("{:.*e}", (PRECISION - 1) as usize, n);
    let (mantissa, exponent) = scientific.split_once('e').expect("exponent");
    let exponent: i32 = exponent.parse().expect("exponent value");
    if exponent < -4 || exponent >= PRECISION {
        let mantissa = trim_fraction(mantissa);
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exponent.abs())
    } else {
        let decimals = (PRECISION - 1 - exponent).max(0) as usize;
        trim_fraction(&format!("{n:.decimals$}")).to_string()
    }
}

fn trim_fraction(text: &str) -> &str {
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        text
    }
}
