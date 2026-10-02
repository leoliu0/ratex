//! Port of the lauxlib.c helpers used by the standard libraries: argument
//! checks with Lua's error messages, `luaL_where` positions, `luaL_tolstring`.

use crate::compiler::format_source;
use crate::lua_value::{LuaValue, LuaValueKind};
use crate::lua_vm::{LuaError, LuaResult, LuaState, TmKind, get_metamethod_event};
use crate::stdlib::basic::parse_number::parse_lua_number;
use crate::stdlib::debug::{current_func_name_with_kind, find_global_func_name, objtypename};
use crate::stdlib::numfmt::{NumBuf, tostring_float};

/// A string argument: a Lua string, or a number converted as `lua_tolstring`
/// would (without writing the conversion back to the stack), or the bytes of
/// a `__tostring` result (owned: that result is not on the stack).
pub(crate) enum LStr {
    Value(LuaValue),
    Number(NumBuf),
    Bytes(Vec<u8>),
}

impl std::ops::Deref for LStr {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        match self {
            LStr::Value(value) => value.as_bytes().unwrap_or_default(),
            LStr::Number(buf) => buf.as_bytes(),
            LStr::Bytes(bytes) => bytes,
        }
    }
}

impl LStr {
    /// The string as a Lua value: the string itself, or a new string for a
    /// converted number (`luaL_checkstring` converts a number in place). The
    /// result is unrooted.
    pub(crate) fn into_value(self, l: &mut LuaState) -> LuaResult<LuaValue> {
        match self {
            LStr::Value(value) => Ok(value),
            LStr::Number(buf) => l.create_bytes(buf.as_bytes()),
            LStr::Bytes(bytes) => l.create_bytes(&bytes),
        }
    }
}

/// `lua_tolstring` of a number value.
pub(crate) fn number_to_lstr(l: &LuaState, value: &LuaValue) -> Option<LStr> {
    if let Some(i) = value.as_integer_strict() {
        let mut buf = NumBuf::new();
        let _ = std::fmt::Write::write_str(&mut buf, itoa::Buffer::new().format(i));
        Some(LStr::Number(buf))
    } else {
        let n = value.as_float()?;
        Some(LStr::Number(tostring_float(n, l.global_state().language())))
    }
}

/// `lua_tolstring`: strings and numbers.
#[inline]
pub(crate) fn to_lstr(l: &LuaState, value: &LuaValue) -> Option<LStr> {
    if value.is_string() {
        Some(LStr::Value(*value))
    } else {
        number_to_lstr(l, value)
    }
}

/// `lua_tointegerx`: integers, floats with an exact integer value and
/// strings convertible to such numbers.
pub(crate) fn tointeger(value: &LuaValue) -> Option<i64> {
    if let Some(i) = value.as_integer() {
        return Some(i);
    }
    let text = value.as_bytes()?;
    parse_lua_number(std::str::from_utf8(text).ok()?).as_integer()
}

/// `lua_tonumberx`: numbers and strings convertible to numbers.
pub(crate) fn tonumber(value: &LuaValue) -> Option<f64> {
    if let Some(n) = value.as_float() {
        return Some(n);
    }
    let text = value.as_bytes()?;
    parse_lua_number(std::str::from_utf8(text).ok()?).as_float()
}

/// `luaL_where(L, level)`: `"chunk:line: "` of the function at `level`
/// (1 = the caller of the running C function), or `""` for C functions.
pub(crate) fn lual_where(l: &LuaState, level: usize) -> String {
    let depth = l.call_depth();
    if level == 0 || level >= depth {
        return String::new();
    }
    let ci = l.get_call_info(depth - 1 - level);
    if !ci.is_lua() || ci.chunk_ptr.is_null() {
        return String::new();
    }
    let chunk = unsafe { &*ci.chunk_ptr };
    let line = if ci.pc > 0 {
        chunk.line_info.get(ci.pc as usize - 1).copied().unwrap_or(0)
    } else {
        chunk.line_info.first().copied().unwrap_or(0)
    };
    if line == 0 {
        return String::new();
    }
    let source = chunk.source_name.as_deref().unwrap_or("?");
    format!("{}:{}: ", format_source(source), line)
}

/// `luaL_error`: raise `msg` prefixed with the caller's position.
pub(crate) fn lual_error(l: &mut LuaState, msg: impl AsRef<str>) -> LuaError {
    let position = lual_where(l, 1);
    l.error(format!("{}{}", position, msg.as_ref()))
}

/// `lual_error` for a message of bytes (it may quote a name that is not UTF-8).
pub(crate) fn lual_error_bytes(l: &mut LuaState, msg: &[u8]) -> LuaError {
    let mut full = lual_where(l, 1).into_bytes();
    full.extend_from_slice(msg);
    l.error_bytes(full)
}

/// `luaL_argerror`: `"bad argument #narg to 'name' (extramsg)"`.
pub(crate) fn argerror(l: &mut LuaState, narg: usize, extramsg: &str) -> LuaError {
    argerror_at(l, narg as i64, extramsg)
}

/// `luaL_argerror` for an argument index that may be negative (a stack
/// index relative to the top, as LuaTeX's `mplib.new` reports).
pub(crate) fn argerror_at(l: &mut LuaState, narg: i64, extramsg: &str) -> LuaError {
    let mut narg = narg;
    let name = match current_func_name_with_kind(l) {
        Some((kind, name)) => {
            if kind == "method" {
                narg -= 1;
                if narg == 0 {
                    let mut msg = b"calling '".to_vec();
                    msg.extend_from_slice(&name);
                    msg.extend_from_slice(format!("' on bad self ({extramsg})").as_bytes());
                    return lual_error_bytes(l, &msg);
                }
            }
            name
        }
        None => {
            let function = l.get_frame_func(l.call_depth().wrapping_sub(1));
            function
                .as_ref()
                .and_then(|f| find_global_func_name(l, f))
                .map_or_else(|| b"?".to_vec(), String::into_bytes)
        }
    };
    let mut msg = format!("bad argument #{narg} to '").into_bytes();
    msg.extend_from_slice(&name);
    msg.extend_from_slice(format!("' ({extramsg})").as_bytes());
    lual_error_bytes(l, &msg)
}

/// `luaL_typeerror`: `"<expected> expected, got <type>"` for argument `narg`.
pub(crate) fn typeerror(l: &mut LuaState, narg: usize, expected: &str) -> LuaError {
    let actual = match l.get_arg(narg) {
        None => "no value".to_string(),
        Some(value) => match crate::stdlib::debug::metatable_name(l, &value) {
            Some(name) => name,
            None if value.ttislightuserdata() => "light userdata".to_string(),
            None => value.type_name().to_string(),
        },
    };
    argerror(l, narg, &format!("{expected} expected, got {actual}"))
}

/// Error for an argument that is neither a number nor convertible.
fn number_error(l: &mut LuaState, narg: usize) -> LuaError {
    let convertible = l.get_arg(narg).as_ref().and_then(tonumber).is_some();
    if convertible {
        argerror(l, narg, "number has no integer representation")
    } else {
        typeerror(l, narg, "number")
    }
}

/// `luaL_checkinteger`.
#[inline]
pub(crate) fn check_integer(l: &mut LuaState, narg: usize) -> LuaResult<i64> {
    match l.get_arg(narg) {
        Some(value) if value.ttisinteger() => Ok(value.ivalue()),
        Some(value) => tointeger(&value).ok_or_else(|| number_error(l, narg)),
        None => Err(typeerror(l, narg, "number")),
    }
}

/// `luaL_optinteger`.
#[inline]
pub(crate) fn opt_integer(l: &mut LuaState, narg: usize, default: i64) -> LuaResult<i64> {
    match l.get_arg(narg) {
        None => Ok(default),
        Some(value) if value.is_nil() => Ok(default),
        Some(_) => check_integer(l, narg),
    }
}

/// `luaL_checknumber`.
#[inline]
pub(crate) fn check_number(l: &mut LuaState, narg: usize) -> LuaResult<f64> {
    match l.get_arg(narg).as_ref().and_then(tonumber) {
        Some(n) => Ok(n),
        None => Err(typeerror(l, narg, "number")),
    }
}

/// `luaL_optnumber`.
pub(crate) fn opt_number(l: &mut LuaState, narg: usize, default: f64) -> LuaResult<f64> {
    match l.get_arg(narg) {
        None => Ok(default),
        Some(value) if value.is_nil() => Ok(default),
        Some(_) => check_number(l, narg),
    }
}

/// `luaL_checklstring`: strings, or numbers converted to strings.
#[inline]
pub(crate) fn check_lstring(l: &mut LuaState, narg: usize) -> LuaResult<LStr> {
    match l.get_arg(narg) {
        Some(value) if value.is_string() => Ok(LStr::Value(value)),
        Some(value) => number_to_lstr(l, &value).ok_or_else(|| typeerror(l, narg, "string")),
        None => Err(typeerror(l, narg, "string")),
    }
}

/// `luaL_optlstring` without a default: `None` for none/nil.
pub(crate) fn opt_lstring(l: &mut LuaState, narg: usize) -> LuaResult<Option<LStr>> {
    match l.get_arg(narg) {
        None => Ok(None),
        Some(value) if value.is_nil() => Ok(None),
        Some(_) => check_lstring(l, narg).map(Some),
    }
}

/// `luaL_checkany`.
pub(crate) fn check_any(l: &mut LuaState, narg: usize) -> LuaResult<LuaValue> {
    l.get_arg(narg)
        .ok_or_else(|| argerror(l, narg, "value expected"))
}

/// `luaL_checkoption`: index of the argument in `options`.
pub(crate) fn check_option(
    l: &mut LuaState,
    narg: usize,
    default: Option<&str>,
    options: &[&str],
) -> LuaResult<usize> {
    let name = match (l.get_arg(narg).filter(|v| !v.is_nil()), default) {
        (None, Some(default)) => default.as_bytes().to_vec(),
        _ => check_lstring(l, narg)?.to_vec(),
    };
    if let Some(index) = options.iter().position(|option| option.as_bytes() == name) {
        return Ok(index);
    }
    let name = String::from_utf8_lossy(&name).into_owned();
    Err(argerror(l, narg, &format!("invalid option '{name}'")))
}

/// `luaL_tolstring`: `__tostring`, then `__name`, then the default format.
pub(crate) fn tolstring(l: &mut LuaState, value: &LuaValue) -> LuaResult<LStr> {
    if let Some(metamethod) = get_metamethod_event(l, value, TmKind::ToString) {
        let result = crate::lua_vm::execute::call_tm_res1(l, metamethod, *value)?;
        // The result lies above the stack top now; the caller keeps the
        // bytes, not the unrooted string.
        return match to_lstr(l, &result) {
            Some(text) => Ok(LStr::Bytes(text.to_vec())),
            None => Err(lual_error(l, "'__tostring' must return a string")),
        };
    }
    if let Some(text) = to_lstr(l, value) {
        return Ok(text);
    }
    let text = match value.kind() {
        LuaValueKind::Nil => return Ok(LStr::Value(l.global_state().const_strings.str_nil)),
        LuaValueKind::Boolean => {
            let strings = &l.global_state().const_strings;
            let text = if value.as_boolean() == Some(true) { strings.str_true } else { strings.str_false };
            return Ok(LStr::Value(text));
        }
        _ => {
            if value.ttisfulluserdata()
                && let Some(ud) = value.as_userdata_mut()
                && let Some(text) = ud.get_trait().lua_tostring()
            {
                text
            } else {
                let type_name = objtypename(l, value);
                if type_name == value.type_name() {
                    format!("{value}")
                } else {
                    format!("{}: 0x{:x}", type_name, value.raw_ptr_repr() as usize)
                }
            }
        }
    };
    Ok(LStr::Value(l.create_string(&text)?))
}
