//! UTF-8 library: a port of lutf8lib.c. Lua 5.3 (LuaTeX) decodes at most
//! four bytes up to U+10FFFF and accepts surrogates; Lua 5.5 decodes up to six
//! bytes (2^31 - 1) and rejects surrogates and values above U+10FFFF unless
//! `lax` is true.

use crate::LuaLanguageLevel;
use crate::lib_registry::LibraryModule;
use crate::lua_value::LuaValue;
use crate::lua_vm::LuaResult;
use crate::lua_vm::LuaState;
use crate::stdlib::lauxlib::{argerror, check_integer, check_lstring, lual_error, opt_integer};

pub fn create_utf8_lib() -> LibraryModule {
    let mut module = crate::lib_module!("utf8", {
        "len" => utf8_len,
        "char" => utf8_char,
        "codes" => utf8_codes,
        "codepoint" => utf8_codepoint,
        "offset" => utf8_offset,
    });

    // Add charpattern constant - the byte-level pattern matching a single UTF-8 character.
    module = module.with_value("charpattern", |vm| {
        let max_lead = if vm.language() == crate::LuaLanguageLevel::Lua53 {
            0xF4
        } else {
            0xFD
        };
        vm.create_binary(vec![
            b'[', 0x00, b'-', 0x7F, 0xC2, b'-', max_lead, b']', b'[', 0x80, b'-', 0xBF, b']', b'*',
        ])
    });

    module
}

const MAXUNICODE: u32 = 0x10FFFF;
const MAXUTF: u32 = 0x7FFFFFFF;
const MSG_INVALID: &str = "invalid UTF-8 code";

/// C: u_posrelat.
#[inline]
fn u_posrelat(pos: i64, len: usize) -> i64 {
    if pos >= 0 {
        pos
    } else if pos.unsigned_abs() > len as u64 {
        0
    } else {
        len as i64 + pos + 1
    }
}

/// The byte at `i`, or 0 past the end (C strings end with '\0').
#[inline]
fn byte_at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

#[inline]
fn iscont(s: &[u8], i: usize) -> bool {
    byte_at(s, i) & 0xC0 == 0x80
}

fn is_lua53(l: &LuaState) -> bool {
    l.global_state().language() == LuaLanguageLevel::Lua53
}

/// C: utf8_decode. Decodes the sequence at `s[i..]`; returns the code point
/// and the index just past it.
fn utf8_decode(s: &[u8], i: usize, lua53: bool, strict: bool) -> Option<(u32, usize)> {
    let mut c = byte_at(s, i) as u32;
    if c < 0x80 {
        return Some((c, i + 1));
    }
    let mut res: u32 = 0;
    let mut count = 0usize;
    while c & 0x40 != 0 {
        count += 1;
        let cc = byte_at(s, i + count) as u32;
        if cc & 0xC0 != 0x80 {
            return None;
        }
        res = (res << 6) | (cc & 0x3F);
        c <<= 1;
    }
    if lua53 {
        // limits = {0xFF, 0x7F, 0x7FF, 0xFFFF}; surrogates are accepted
        const LIMITS: [u32; 4] = [0xFF, 0x7F, 0x7FF, 0xFFFF];
        if count > 3 {
            return None;
        }
        res |= (c & 0x7F) << (count * 5);
        if res > MAXUNICODE || res <= LIMITS[count] {
            return None;
        }
    } else {
        const LIMITS: [u32; 6] = [u32::MAX, 0x80, 0x800, 0x10000, 0x200000, 0x4000000];
        if count > 5 {
            return None;
        }
        res |= ((c & 0x7F) as u64).wrapping_shl(count as u32 * 5) as u32;
        if res > MAXUTF || res < LIMITS[count] {
            return None;
        }
        if strict && (res > MAXUNICODE || (0xD800..=0xDFFF).contains(&res)) {
            return None;
        }
    }
    Some((res, i + count + 1))
}

/// C: luaO_utf8esc.
fn push_utf8(out: &mut Vec<u8>, mut x: u32) {
    if x < 0x80 {
        out.push(x as u8);
        return;
    }
    let mut buf = [0u8; 8];
    let mut n = 1;
    let mut mfb: u32 = 0x3f;
    loop {
        buf[8 - n] = 0x80 | (x & 0x3f) as u8;
        n += 1;
        x >>= 6;
        mfb >>= 1;
        if x <= mfb {
            break;
        }
    }
    buf[8 - n] = ((!mfb << 1) | x) as u8;
    out.extend_from_slice(&buf[8 - n..]);
}

/// utf8.len(s [, i [, j [, lax]]])
fn utf8_len(l: &mut LuaState) -> LuaResult<usize> {
    let s = check_lstring(l, 1)?;
    let len = s.len();
    let mut posi = u_posrelat(opt_integer(l, 2, 1)?, len);
    let mut posj = u_posrelat(opt_integer(l, 3, -1)?, len);
    let lua53 = is_lua53(l);
    let lax = !lua53 && l.get_arg(4).is_some_and(|v| v.is_truthy());
    let (initial, last) = if lua53 {
        ("initial position out of string", "final position out of string")
    } else {
        ("initial position out of bounds", "final position out of bounds")
    };
    if posi < 1 || {
        posi -= 1;
        posi
    } > len as i64
    {
        return Err(argerror(l, 2, initial));
    }
    posj -= 1;
    if posj >= len as i64 {
        return Err(argerror(l, 3, last));
    }
    let mut n: i64 = 0;
    while posi <= posj {
        match utf8_decode(&s, posi as usize, lua53, !lax) {
            Some((_, next)) => {
                posi = next as i64;
                n += 1;
            }
            None => {
                l.push_value(LuaValue::nil())?;
                l.push_value(LuaValue::integer(posi + 1))?;
                return Ok(2);
            }
        }
    }
    l.push_value(LuaValue::integer(n))?;
    Ok(1)
}

/// utf8.char(...)
fn utf8_char(l: &mut LuaState) -> LuaResult<usize> {
    let max = if is_lua53(l) { MAXUNICODE } else { MAXUTF };
    let mut out = Vec::new();
    for arg in 1..=l.arg_count() {
        let code = check_integer(l, arg)?;
        // 5.5 checks `(lua_Unsigned)code <= MAXUTF`, 5.3 `0 <= code <= MAXUNICODE`
        if code < 0 || code > max as i64 {
            return Err(argerror(l, arg, "value out of range"));
        }
        push_utf8(&mut out, code as u32);
    }
    let value = l.create_bytes(&out)?;
    l.push_value(value)?;
    Ok(1)
}

/// utf8.codepoint(s [, i [, j [, lax]]])
fn utf8_codepoint(l: &mut LuaState) -> LuaResult<usize> {
    let s = check_lstring(l, 1)?;
    let len = s.len();
    let posi = u_posrelat(opt_integer(l, 2, 1)?, len);
    let pose = u_posrelat(opt_integer(l, 3, posi)?, len);
    let lua53 = is_lua53(l);
    let lax = !lua53 && l.get_arg(4).is_some_and(|v| v.is_truthy());
    let range = if lua53 { "out of range" } else { "out of bounds" };
    if posi < 1 {
        return Err(argerror(l, 2, range));
    }
    if pose > len as i64 {
        return Err(argerror(l, 3, range));
    }
    if posi > pose {
        return Ok(0);
    }
    if pose - posi >= i32::MAX as i64 {
        return Err(lual_error(l, "string slice too long"));
    }
    let end = pose as usize;
    let mut pos = (posi - 1) as usize;
    let mut n = 0;
    while pos < end {
        let Some((code, next)) = utf8_decode(&s, pos, lua53, !lax) else {
            return Err(lual_error(l, MSG_INVALID));
        };
        l.push_value(LuaValue::integer(code as i64))?;
        n += 1;
        pos = next;
    }
    Ok(n)
}

/// utf8.offset(s, n [, i])
fn utf8_offset(l: &mut LuaState) -> LuaResult<usize> {
    let s = check_lstring(l, 1)?;
    let len = s.len();
    let mut n = check_integer(l, 2)?;
    let default = if n >= 0 { 1 } else { len as i64 + 1 };
    let mut posi = u_posrelat(opt_integer(l, 3, default)?, len);
    let lua53 = is_lua53(l);
    if posi < 1 || {
        posi -= 1;
        posi
    } > len as i64
    {
        let detail = if lua53 { "position out of range" } else { "position out of bounds" };
        return Err(argerror(l, 3, detail));
    }
    if n == 0 {
        // find beginning of current byte sequence
        while posi > 0 && iscont(&s, posi as usize) {
            posi -= 1;
        }
    } else {
        if iscont(&s, posi as usize) {
            return Err(lual_error(l, "initial position is a continuation byte"));
        }
        if n < 0 {
            while n < 0 && posi > 0 {
                // find beginning of previous character
                loop {
                    posi -= 1;
                    if !(posi > 0 && iscont(&s, posi as usize)) {
                        break;
                    }
                }
                n += 1;
            }
        } else {
            n -= 1; // do not move for 1st character
            while n > 0 && posi < len as i64 {
                // find beginning of next character (cannot pass the final '\0')
                loop {
                    posi += 1;
                    if !iscont(&s, posi as usize) {
                        break;
                    }
                }
                n -= 1;
            }
        }
    }
    if n != 0 {
        l.push_value(LuaValue::nil())?;
        return Ok(1);
    }
    l.push_value(LuaValue::integer(posi + 1))?;
    if lua53 {
        return Ok(1);
    }
    // Lua 5.5 also returns the final position of the character
    let mut last = posi as usize;
    if byte_at(&s, last) & 0x80 != 0 {
        if iscont(&s, last) {
            return Err(lual_error(l, "initial position is a continuation byte"));
        }
        while iscont(&s, last + 1) {
            last += 1;
        }
    }
    l.push_value(LuaValue::integer(last as i64 + 1))?;
    Ok(2)
}

/// utf8.codes(s [, lax]): returns a stateless iterator, `s` and 0.
fn utf8_codes(l: &mut LuaState) -> LuaResult<usize> {
    let lua53 = is_lua53(l);
    let lax = !lua53 && l.get_arg(2).is_some_and(|v| v.is_truthy());
    let s = check_lstring(l, 1)?;
    if !lua53 && iscont(&s, 0) {
        return Err(argerror(l, 1, MSG_INVALID));
    }
    // luaL_checkstring converts a number argument in place
    let subject = s.into_value(l)?;
    let iterator: fn(&mut LuaState) -> LuaResult<usize> = match (lua53, lax) {
        (true, _) => iter_aux53,
        (false, false) => iter_aux_strict,
        (false, true) => iter_aux_lax,
    };
    l.push_value(LuaValue::cfunction(iterator))?;
    l.push_value(subject)?;
    l.push_value(LuaValue::integer(0))?;
    Ok(3)
}

/// Lua 5.3 iter_aux: the control value is the previous 1-based position.
fn iter_aux53(l: &mut LuaState) -> LuaResult<usize> {
    let s = check_lstring(l, 1)?;
    let len = s.len() as i64;
    let mut n = l.get_arg(2).as_ref().and_then(crate::stdlib::lauxlib::tointeger).unwrap_or(0).wrapping_sub(1);
    if n < 0 {
        n = 0; // first iteration
    } else if n < len {
        n += 1; // skip current byte and its continuations
        while iscont(&s, n as usize) {
            n += 1;
        }
    }
    if n >= len {
        return Ok(0);
    }
    match utf8_decode(&s, n as usize, true, true) {
        Some((code, next)) if !iscont(&s, next) => {
            l.push_value(LuaValue::integer(n + 1))?;
            l.push_value(LuaValue::integer(code as i64))?;
            Ok(2)
        }
        _ => Err(lual_error(l, MSG_INVALID)),
    }
}

/// Lua 5.5 iter_aux.
fn iter_aux55(l: &mut LuaState, strict: bool) -> LuaResult<usize> {
    let s = check_lstring(l, 1)?;
    let len = s.len() as u64;
    let mut n = l.get_arg(2).as_ref().and_then(crate::stdlib::lauxlib::tointeger).unwrap_or(0) as u64;
    if n < len {
        while iscont(&s, n as usize) {
            n += 1; // go to next character
        }
    }
    if n >= len {
        return Ok(0); // (also handles an original negative 'n')
    }
    match utf8_decode(&s, n as usize, false, strict) {
        Some((code, next)) if !iscont(&s, next) => {
            l.push_value(LuaValue::integer(n as i64 + 1))?;
            l.push_value(LuaValue::integer(code as i64))?;
            Ok(2)
        }
        _ => Err(lual_error(l, MSG_INVALID)),
    }
}

fn iter_aux_strict(l: &mut LuaState) -> LuaResult<usize> {
    iter_aux55(l, true)
}

fn iter_aux_lax(l: &mut LuaState) -> LuaResult<usize> {
    iter_aux55(l, false)
}
