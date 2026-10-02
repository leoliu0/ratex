// Math library
// Implements: abs, acos, asin, atan, ceil, cos, deg, exp, floor, fmod,
// log, max, min, modf, rad, random, randomseed, sin, sqrt, tan, tointeger,
// type, ult, pi, huge, maxinteger, mininteger

use crate::lib_registry::LibraryModule;
use crate::lua_value::{LuaValue, LuaValueKind};
use crate::lua_vm::LuaResult;
use crate::lua_vm::LuaState;
use crate::lua_vm::LuaRng;
use crate::stdlib::lauxlib;
use crate::platform_time;

pub fn create_math_lib() -> LibraryModule {
    let mut module = crate::lib_module!("math", {
        "abs" => math_abs,
        "acos" => math_acos,
        "asin" => math_asin,
        "atan" => math_atan,
        "atan2" => math_atan,
        "ceil" => math_ceil,
        "cos" => math_cos,
        "cosh" => math_cosh,
        "deg" => math_deg,
        "exp" => math_exp,
        "floor" => math_floor,
        "fmod" => math_fmod,
        "frexp" => math_frexp,
        "ldexp" => math_ldexp,
        "log" => math_log,
        "log10" => math_log10,
        "max" => math_max,
        "min" => math_min,
        "modf" => math_modf,
        "pow" => math_pow,
        "rad" => math_rad,
        "random" => math_random,
        "randomseed" => math_randomseed,
        "sin" => math_sin,
        "sinh" => math_sinh,
        "sqrt" => math_sqrt,
        "tan" => math_tan,
        "tanh" => math_tanh,
        "tointeger" => math_tointeger,
        "type" => math_type,
        "ult" => math_ult,
    });

    // Add constants using with_value
    module = module.with_value("pi", |_vm| Ok(LuaValue::float(std::f64::consts::PI)));
    module = module.with_value("huge", |_vm| Ok(LuaValue::float(f64::INFINITY)));
    module = module.with_value("maxinteger", |_vm| Ok(LuaValue::integer(i64::MAX)));
    module = module.with_value("mininteger", |_vm| Ok(LuaValue::integer(i64::MIN)));

    module
}

fn push_float(l: &mut LuaState, n: f64) -> LuaResult<usize> {
    l.push_value(LuaValue::float(n))?;
    Ok(1)
}

/// `pushnumint`: an integer if the float has an exact integer value.
fn push_num_int(l: &mut LuaState, n: f64) -> LuaResult<usize> {
    let value = match LuaValue::float(n).as_integer() {
        Some(i) => LuaValue::integer(i),
        None => LuaValue::float(n),
    };
    l.push_value(value)?;
    Ok(1)
}

fn unary(l: &mut LuaState, f: fn(f64) -> f64) -> LuaResult<usize> {
    let x = lauxlib::check_number(l, 1)?;
    push_float(l, f(x))
}

fn math_cosh(l: &mut LuaState) -> LuaResult<usize> {
    unary(l, f64::cosh)
}

fn math_sinh(l: &mut LuaState) -> LuaResult<usize> {
    unary(l, f64::sinh)
}

fn math_tanh(l: &mut LuaState) -> LuaResult<usize> {
    unary(l, f64::tanh)
}

fn math_log10(l: &mut LuaState) -> LuaResult<usize> {
    unary(l, f64::log10)
}

fn math_acos(l: &mut LuaState) -> LuaResult<usize> {
    unary(l, f64::acos)
}

fn math_asin(l: &mut LuaState) -> LuaResult<usize> {
    unary(l, f64::asin)
}

fn math_cos(l: &mut LuaState) -> LuaResult<usize> {
    unary(l, f64::cos)
}

fn math_deg(l: &mut LuaState) -> LuaResult<usize> {
    unary(l, |x| x * (180.0 / std::f64::consts::PI))
}

fn math_rad(l: &mut LuaState) -> LuaResult<usize> {
    unary(l, |x| x * (std::f64::consts::PI / 180.0))
}

fn math_exp(l: &mut LuaState) -> LuaResult<usize> {
    unary(l, f64::exp)
}

fn math_sin(l: &mut LuaState) -> LuaResult<usize> {
    unary(l, f64::sin)
}

fn math_sqrt(l: &mut LuaState) -> LuaResult<usize> {
    unary(l, f64::sqrt)
}

fn math_tan(l: &mut LuaState) -> LuaResult<usize> {
    unary(l, f64::tan)
}

fn math_pow(l: &mut LuaState) -> LuaResult<usize> {
    let base = lauxlib::check_number(l, 1)?;
    let exponent = lauxlib::check_number(l, 2)?;
    push_float(l, base.powf(exponent))
}

fn math_abs(l: &mut LuaState) -> LuaResult<usize> {
    match l.get_arg(1) {
        Some(value) if value.ttisinteger() => {
            l.push_value(LuaValue::integer(value.ivalue().wrapping_abs()))?;
            Ok(1)
        }
        _ => unary(l, f64::abs),
    }
}

fn math_atan(l: &mut LuaState) -> LuaResult<usize> {
    let y = lauxlib::check_number(l, 1)?;
    let x = lauxlib::opt_number(l, 2, 1.0)?;
    push_float(l, y.atan2(x))
}

fn round_with(l: &mut LuaState, f: fn(f64) -> f64) -> LuaResult<usize> {
    if let Some(value) = l.get_arg(1).filter(|v| v.ttisinteger()) {
        l.push_value(value)?;
        return Ok(1);
    }
    let x = lauxlib::check_number(l, 1)?;
    push_num_int(l, f(x))
}

fn math_ceil(l: &mut LuaState) -> LuaResult<usize> {
    round_with(l, f64::ceil)
}

fn math_floor(l: &mut LuaState) -> LuaResult<usize> {
    round_with(l, f64::floor)
}

fn math_fmod(l: &mut LuaState) -> LuaResult<usize> {
    let a = l.get_arg(1).unwrap_or_default();
    let b = l.get_arg(2).unwrap_or_default();
    if a.ttisinteger() && b.ttisinteger() {
        let d = b.ivalue();
        let result = match d {
            0 => return Err(lauxlib::argerror(l, 2, "zero")),
            // Avoid overflow of mininteger % -1.
            -1 => 0,
            _ => a.ivalue() % d,
        };
        l.push_value(LuaValue::integer(result))?;
        return Ok(1);
    }
    let x = lauxlib::check_number(l, 1)?;
    let y = lauxlib::check_number(l, 2)?;
    push_float(l, x % y)
}

fn math_log(l: &mut LuaState) -> LuaResult<usize> {
    let x = lauxlib::check_number(l, 1)?;
    let result = if l.get_arg(2).is_none_or(|v| v.is_nil()) {
        x.ln()
    } else {
        match lauxlib::check_number(l, 2)? {
            2.0 => x.log2(),
            10.0 => x.log10(),
            base => x.ln() / base.ln(),
        }
    };
    push_float(l, result)
}

/// math.max / math.min: `lua_compare` over all arguments (any comparable
/// values, metamethods included).
fn extremum(l: &mut LuaState, max: bool) -> LuaResult<usize> {
    let n = l.arg_count();
    if n == 0 {
        return Err(lauxlib::argerror(l, 1, "value expected"));
    }
    let mut best = l.get_arg(1).unwrap_or_default();
    for i in 2..=n {
        let value = l.get_arg(i).unwrap_or_default();
        let better = if max { l.obj_lt(&best, &value)? } else { l.obj_lt(&value, &best)? };
        if better {
            best = value;
        }
    }
    l.push_value(best)?;
    Ok(1)
}

fn math_max(l: &mut LuaState) -> LuaResult<usize> {
    extremum(l, true)
}

fn math_min(l: &mut LuaState) -> LuaResult<usize> {
    extremum(l, false)
}

fn math_modf(l: &mut LuaState) -> LuaResult<usize> {
    if let Some(value) = l.get_arg(1).filter(|v| v.ttisinteger()) {
        l.push_value(value)?;
        l.push_value(LuaValue::float(0.0))?;
        return Ok(2);
    }
    let n = lauxlib::check_number(l, 1)?;
    let ip = if n < 0.0 { n.ceil() } else { n.floor() };
    push_num_int(l, ip)?;
    push_float(l, if n == ip { 0.0 } else { n - ip })?;
    Ok(2)
}

/// Lua 5.3 `math.random` (lmathlib.c over POSIX `random()`).
fn math_random53(l: &mut LuaState) -> LuaResult<usize> {
    let rv = l.global_state_mut().libc_rng.next_rand();
    let r = rv as f64 * (1.0 / (2147483647.0 + 1.0));
    let (low, up) = match l.arg_count() {
        0 => return push_float(l, r),
        1 => (1, lauxlib::check_integer(l, 1)?),
        2 => (lauxlib::check_integer(l, 1)?, lauxlib::check_integer(l, 2)?),
        _ => return Err(lauxlib::lual_error(l, "wrong number of arguments")),
    };
    if low > up {
        return Err(lauxlib::argerror(l, 1, "interval is empty"));
    }
    if !(low >= 0 || up <= i64::MAX + low) {
        return Err(lauxlib::argerror(l, 1, "interval too large"));
    }
    let r = r * ((up as f64 - low as f64) + 1.0);
    l.push_value(LuaValue::integer((r as i64).wrapping_add(low)))?;
    Ok(1)
}

fn math_random(l: &mut LuaState) -> LuaResult<usize> {
    let lua53 = l.global_state().language() == crate::LuaLanguageLevel::Lua53;
    if lua53 {
        return math_random53(l);
    }
    let rv = l.global_state_mut().rng.next_rand();
    let (low, up) = match l.arg_count() {
        0 => return push_float(l, LuaRng::to_float(rv)),
        1 => {
            let up = lauxlib::check_integer(l, 1)?;
            if up == 0 && !lua53 {
                l.push_value(LuaValue::integer(rv as i64))?;
                return Ok(1);
            }
            (1, up)
        }
        2 => (lauxlib::check_integer(l, 1)?, lauxlib::check_integer(l, 2)?),
        _ => return Err(lauxlib::lual_error(l, "wrong number of arguments")),
    };
    if low > up {
        return Err(lauxlib::argerror(l, 1, "interval is empty"));
    }
    let n = (up as u64).wrapping_sub(low as u64);
    let offset = project(l, rv, n);
    l.push_value(LuaValue::integer((low as u64).wrapping_add(offset) as i64))?;
    Ok(1)
}

/// Lua 5.5 `project`: `ran` masked to the smallest Mersenne number covering `n`,
/// drawing again while it exceeds `n` (unbiased).
fn project(l: &mut LuaState, mut ran: u64, n: u64) -> u64 {
    let mut lim = n;
    let mut sh = 1;
    while lim & lim.wrapping_add(1) != 0 {
        lim |= lim >> sh;
        sh *= 2;
    }
    loop {
        ran &= lim;
        if ran <= n {
            return ran;
        }
        ran = l.global_state_mut().rng.next_rand();
    }
}

fn math_randomseed(l: &mut LuaState) -> LuaResult<usize> {
    if l.global_state().language() == crate::LuaLanguageLevel::Lua53 {
        // l_srand((unsigned int)(lua_Integer)luaL_checknumber(L, 1)); l_rand().
        let seed = lauxlib::check_number(l, 1)?;
        let mut rng = crate::lua_vm::LibcRandom::from_seed(seed as i64 as u32);
        rng.next_rand();
        l.global_state_mut().libc_rng = rng;
        return Ok(0);
    }
    let (n1, n2) = if l.arg_count() == 0 {
        // A "random" seed, mixed with the current state in case it is not that random.
        (platform_time::unix_nanos() as i64, l.global_state_mut().rng.next_rand() as i64)
    } else {
        let n1 = lauxlib::check_integer(l, 1)?;
        (n1, lauxlib::opt_integer(l, 2, 0)?)
    };
    l.global_state_mut().rng = LuaRng::from_seed(n1, n2);
    l.push_value(LuaValue::integer(n1))?;
    l.push_value(LuaValue::integer(n2))?;
    Ok(2)
}

fn math_tointeger(l: &mut LuaState) -> LuaResult<usize> {
    let value = lauxlib::check_any(l, 1)?;
    let result = match lauxlib::tointeger(&value) {
        Some(i) => LuaValue::integer(i),
        None => LuaValue::nil(),
    };
    l.push_value(result)?;
    Ok(1)
}

fn math_type(l: &mut LuaState) -> LuaResult<usize> {
    let value = lauxlib::check_any(l, 1)?;
    let strings = &l.global_state().const_strings;
    let result = match value.kind() {
        LuaValueKind::Integer => strings.str_integer,
        LuaValueKind::Float => strings.str_float,
        _ => LuaValue::nil(),
    };
    l.push_value(result)?;
    Ok(1)
}

fn math_ult(l: &mut LuaState) -> LuaResult<usize> {
    let m = lauxlib::check_integer(l, 1)?;
    let n = lauxlib::check_integer(l, 2)?;
    l.push_value(LuaValue::boolean((m as u64) < (n as u64)))?;
    Ok(1)
}

/// math.frexp(x) -> m, e such that x = m * 2^e, 0.5 <= |m| < 1
fn math_frexp(l: &mut LuaState) -> LuaResult<usize> {
    let x = lauxlib::check_number(l, 1)?;
    if x == 0.0 || !x.is_finite() {
        push_float(l, x)?;
        l.push_value(LuaValue::integer(0))?;
        return Ok(2);
    }
    // Normalize subnormals first, then read the exponent field.
    let (scaled, shift) = if x.abs() < f64::MIN_POSITIVE { (x * 2f64.powi(64), -64) } else { (x, 0) };
    let bits = scaled.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i64 - 1022 + shift;
    let mantissa = f64::from_bits((bits & !(0x7ff << 52)) | (1022 << 52));
    push_float(l, mantissa)?;
    l.push_value(LuaValue::integer(exp))?;
    Ok(2)
}

/// `scalbn(x, n)` = x * 2^n with a single rounding (port of musl's scalbn).
pub(crate) fn scalbn(mut x: f64, n: i64) -> f64 {
    let two_1023 = f64::from_bits(0x7fe0_0000_0000_0000);
    let two_minus_969 = f64::from_bits(0x0010_0000_0000_0000) * f64::from_bits(0x4340_0000_0000_0000);
    let mut n = n.clamp(-3000, 3000) as i32;
    if n > 1023 {
        x *= two_1023;
        n -= 1023;
        if n > 1023 {
            x *= two_1023;
            n = (n - 1023).min(1023);
        }
    } else if n < -1022 {
        x *= two_minus_969;
        n += 1022 - 53;
        if n < -1022 {
            x *= two_minus_969;
            n = (n + 1022 - 53).max(-1022);
        }
    }
    x * f64::from_bits(((0x3ff + n) as u64) << 52)
}

/// math.ldexp(m, e) -> m * 2^e
fn math_ldexp(l: &mut LuaState) -> LuaResult<usize> {
    let m = lauxlib::check_number(l, 1)?;
    let e = lauxlib::check_integer(l, 2)?;
    push_float(l, scalbn(m, e))
}
