// ============================================================
// String-to-number coercion in arithmetic.
//
// Lua 5.5 (lstrlib.c `arith`/`tonum`/`trymt`): the string metatable carries
// __add.. metamethods that convert numeric strings with `lua_stringtonumber`,
// keeping the integer/float subtype of the numeral ("10"+1 == 11).
//
// Lua 5.3 (lvm.c): strings have no arithmetic metamethods; the VM converts
// both operands with `tonumber` and computes in floats ("10"+1 == 11.0).
// ============================================================

use crate::{
    LuaResult, LuaState, LuaValue,
    lua_vm::{
        TmKind, execute,
        execute::arith::{lua_fmod, lua_fmod53, lua_idiv, lua_imod, luai_numpow, luai_numpow53},
        execute::helper::{error_div_by_zero, error_mod_by_zero},
    },
    stdlib::basic::parse_number::parse_lua_number,
};

/// Matches C Lua's `tonum()` in lstrlib.c: numbers pass through, strings are
/// converted with `lua_stringtonumber` (decimal, hex, exponents, surrounding
/// whitespace), keeping the integer/float subtype of the numeral.
pub fn string_arith_tonum(v: &LuaValue) -> Option<LuaValue> {
    if v.is_number() {
        return Some(*v);
    }
    let result = parse_lua_number(v.as_str()?);
    (!result.is_nil()).then_some(result)
}

/// Lua 5.3 `tonumber` (luaV_tonumber_): a number or numeric string as a float.
pub(crate) fn tonumber53(v: &LuaValue) -> Option<f64> {
    string_arith_tonum(v)?.as_float()
}

/// Float arithmetic for an arithmetic event (Lua 5.5 `luai_num*` macros).
pub(crate) fn arith_float(tm: TmKind, a: f64, b: f64) -> f64 {
    match tm {
        TmKind::Add => a + b,
        TmKind::Sub => a - b,
        TmKind::Mul => a * b,
        TmKind::Div => a / b,
        TmKind::Mod => lua_fmod(a, b),
        TmKind::Pow => luai_numpow(a, b),
        TmKind::IDiv => (a / b).floor(),
        TmKind::Unm => -a,
        _ => unreachable!("not an arithmetic event: {:?}", tm),
    }
}

/// Lua 5.3 float arithmetic: as `arith_float`, but with 5.3's '^' (plain
/// `pow`) and '%' (`m*b < 0` correction).
pub(crate) fn arith_float53(tm: TmKind, a: f64, b: f64) -> f64 {
    match tm {
        TmKind::Pow => luai_numpow53(a, b),
        TmKind::Mod => lua_fmod53(a, b),
        _ => arith_float(tm, a, b),
    }
}

/// `luaO_arith` on two numbers: integer arithmetic when both are integers
/// (except '/' and '^'), float arithmetic otherwise.
fn arith_numbers(l: &mut LuaState, tm: TmKind, a: LuaValue, b: LuaValue) -> LuaResult<LuaValue> {
    if a.ttisinteger() && b.ttisinteger() && !matches!(tm, TmKind::Div | TmKind::Pow) {
        let (x, y) = (a.ivalue(), b.ivalue());
        let result = match tm {
            TmKind::Add => x.wrapping_add(y),
            TmKind::Sub => x.wrapping_sub(y),
            TmKind::Mul => x.wrapping_mul(y),
            TmKind::Mod if y == 0 => return Err(error_mod_by_zero(l)),
            TmKind::Mod => lua_imod(x, y),
            TmKind::IDiv if y == 0 => return Err(error_div_by_zero(l)),
            TmKind::IDiv => lua_idiv(x, y),
            TmKind::Unm => x.wrapping_neg(),
            _ => unreachable!("not an arithmetic event: {:?}", tm),
        };
        return Ok(LuaValue::integer(result));
    }
    // Both operands are numbers here, so `as_float` cannot fail.
    let fa = a.as_float().unwrap_or_default();
    let fb = b.as_float().unwrap_or_default();
    Ok(LuaValue::float(arith_float(tm, fa, fb)))
}

/// Lua 5.5 string arithmetic metamethod (lstrlib.c `arith` + `trymt`):
/// - if both operands convert to numbers, do the arithmetic;
/// - otherwise, if the second operand is a string, raise an error;
/// - otherwise, call the second operand's metamethod for this event.
fn string_arith(l: &mut LuaState, tm: TmKind) -> LuaResult<usize> {
    let v1 = l.get_arg(1).unwrap_or_default();
    let v2 = l.get_arg(2).unwrap_or_default();

    if let (Some(a), Some(b)) = (string_arith_tonum(&v1), string_arith_tonum(&v2)) {
        let result = arith_numbers(l, tm, a, b)?;
        l.push_value(result)?;
        return Ok(1);
    }

    if !v2.is_string()
        && let Some(mt) = execute::get_metatable(l, &v2)
    {
        let tm_key = l.global_state_mut().const_strings.get_tm_value(tm);
        if let Some(mm) = mt.as_table().and_then(|t| t.raw_get(&tm_key)) {
            let results = l.call_function(mm, vec![v1, v2])?;
            l.push_value(results.into_iter().next().unwrap_or_default())?;
            return Ok(1);
        }
    }

    // "__add" -> "add", as C Lua's `mtname + 2`.
    let op_name = &tm.name()[2..];
    Err(crate::stdlib::lauxlib::lual_error(
        l,
        format!(
            "attempt to {} a '{}' with a '{}'",
            op_name,
            v1.type_name(),
            v2.type_name()
        ),
    ))
}

pub fn string_arith_add(l: &mut LuaState) -> LuaResult<usize> {
    string_arith(l, TmKind::Add)
}

pub fn string_arith_sub(l: &mut LuaState) -> LuaResult<usize> {
    string_arith(l, TmKind::Sub)
}

pub fn string_arith_mul(l: &mut LuaState) -> LuaResult<usize> {
    string_arith(l, TmKind::Mul)
}

pub fn string_arith_mod(l: &mut LuaState) -> LuaResult<usize> {
    string_arith(l, TmKind::Mod)
}

pub fn string_arith_pow(l: &mut LuaState) -> LuaResult<usize> {
    string_arith(l, TmKind::Pow)
}

pub fn string_arith_div(l: &mut LuaState) -> LuaResult<usize> {
    string_arith(l, TmKind::Div)
}

pub fn string_arith_idiv(l: &mut LuaState) -> LuaResult<usize> {
    string_arith(l, TmKind::IDiv)
}

pub fn string_arith_unm(l: &mut LuaState) -> LuaResult<usize> {
    string_arith(l, TmKind::Unm)
}
