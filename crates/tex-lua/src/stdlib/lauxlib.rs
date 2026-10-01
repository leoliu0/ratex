// TEMPORARY stand-in for LuaFront's lauxlib.rs (same API); replaced at merge.
use crate::lua_value::LuaValue;
use crate::lua_vm::{LuaError, LuaResult, LuaState};
use crate::stdlib::debug;

pub(crate) fn lual_error(l: &mut LuaState, msg: impl AsRef<str>) -> LuaError {
    l.error(msg.as_ref().to_string())
}

pub(crate) fn argerror(l: &mut LuaState, narg: usize, extramsg: &str) -> LuaError {
    debug::argerror(l, narg, extramsg)
}

pub(crate) fn typeerror(l: &mut LuaState, narg: usize, expected: &str) -> LuaError {
    match l.get_arg(narg) {
        Some(v) => debug::arg_typeerror(l, narg, expected, &v),
        None => debug::arg_typeerror_novalue(l, narg, expected),
    }
}

pub(crate) fn tointeger(v: &LuaValue) -> Option<i64> {
    if let Some(s) = v.as_str() {
        return crate::stdlib::basic::parse_number::parse_lua_number(s).as_integer();
    }
    if v.is_number() { v.as_integer() } else { None }
}

pub(crate) fn tonumber(v: &LuaValue) -> Option<f64> {
    if let Some(s) = v.as_str() {
        return crate::stdlib::basic::parse_number::parse_lua_number(s).as_number();
    }
    v.as_number()
}

pub(crate) fn check_integer(l: &mut LuaState, narg: usize) -> LuaResult<i64> {
    let v = l.get_arg(narg).unwrap_or_default();
    if let Some(i) = tointeger(&v) {
        return Ok(i);
    }
    if tonumber(&v).is_some() {
        return Err(argerror(l, narg, "number has no integer representation"));
    }
    Err(typeerror(l, narg, "number"))
}

pub(crate) fn opt_integer(l: &mut LuaState, narg: usize, def: i64) -> LuaResult<i64> {
    match l.get_arg(narg) {
        Some(v) if !v.is_nil() => check_integer(l, narg),
        _ => Ok(def),
    }
}

pub(crate) fn check_number(l: &mut LuaState, narg: usize) -> LuaResult<f64> {
    let v = l.get_arg(narg).unwrap_or_default();
    tonumber(&v).ok_or_else(|| typeerror(l, narg, "number"))
}

pub(crate) fn opt_number(l: &mut LuaState, narg: usize, def: f64) -> LuaResult<f64> {
    match l.get_arg(narg) {
        Some(v) if !v.is_nil() => check_number(l, narg),
        _ => Ok(def),
    }
}

pub(crate) struct LStr(LuaValue);

impl std::ops::Deref for LStr {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.0.as_bytes().unwrap_or(&[])
    }
}

pub(crate) fn check_lstring(l: &mut LuaState, narg: usize) -> LuaResult<LStr> {
    let v = l.get_arg(narg).unwrap_or_default();
    if v.is_string() {
        return Ok(LStr(v));
    }
    if let Some(i) = v.as_integer_strict() {
        return Ok(LStr(l.create_string(&i.to_string())?));
    }
    if let Some(f) = v.as_number() {
        return Ok(LStr(l.create_string(&crate::stdlib::basic::lua_float_to_string(f))?));
    }
    Err(typeerror(l, narg, "string"))
}

pub(crate) fn opt_lstring(l: &mut LuaState, narg: usize) -> LuaResult<Option<LStr>> {
    match l.get_arg(narg) {
        Some(v) if !v.is_nil() => check_lstring(l, narg).map(Some),
        _ => Ok(None),
    }
}

pub(crate) fn check_any(l: &mut LuaState, narg: usize) -> LuaResult<LuaValue> {
    l.get_arg(narg).ok_or_else(|| argerror(l, narg, "value expected"))
}

pub(crate) fn check_table(l: &mut LuaState, narg: usize) -> LuaResult<LuaValue> {
    match l.get_arg(narg) {
        Some(v) if v.is_table() => Ok(v),
        _ => Err(typeerror(l, narg, "table")),
    }
}

pub(crate) fn check_option(
    l: &mut LuaState,
    narg: usize,
    def: Option<&str>,
    opts: &[&str],
) -> LuaResult<usize> {
    let name = match (l.get_arg(narg).filter(|v| !v.is_nil()), def) {
        (None, Some(def)) => def.as_bytes().to_vec(),
        _ => check_lstring(l, narg)?.to_vec(),
    };
    if let Some(i) = opts.iter().position(|o| o.as_bytes() == name.as_slice()) {
        return Ok(i);
    }
    let msg = format!("invalid option '{}'", String::from_utf8_lossy(&name));
    Err(argerror(l, narg, &msg))
}
