// String library (port of lstrlib.c)
// Implements: byte, char, dump, find, format, gmatch, gsub, len, lower,
// match, pack, packsize, rep, reverse, sub, unpack, upper
mod pack;
mod pattern;
mod string_format;

use crate::LuaLanguageLevel;
use crate::lib_registry::LibraryModule;
use crate::lua_value::{LuaValue, chunk_serializer, chunk53};
use crate::lua_vm::lua_limits::MAX_STRING_SIZE;
use crate::lua_vm::{LuaResult, LuaState};
use crate::stdlib::lauxlib::{self, LStr};
use pattern::{Cap, MatchState, PatternError};

pub fn create_string_lib() -> LibraryModule {
    crate::lib_module!("string", {
        "byte" => string_byte,
        "char" => string_char,
        "dump" => string_dump,
        "find" => string_find,
        "format" => string_format::string_format,
        "gmatch" => string_gmatch,
        "gsub" => string_gsub,
        "len" => string_len,
        "lower" => string_lower,
        "match" => string_match,
        "pack" => pack::string_pack,
        "packsize" => pack::string_packsize,
        "rep" => string_rep,
        "reverse" => string_reverse,
        "sub" => string_sub,
        "unpack" => pack::string_unpack,
        "upper" => string_upper,
    })
}

/// `posrelat` (start positions): negative counts from the end; the result
/// is clamped to `1..` (Lua 5.4's `posrelatI` behaves the same way).
#[inline]
fn start_pos(pos: i64, len: usize) -> i64 {
    if pos > 0 {
        pos
    } else if pos == 0 || pos.unsigned_abs() > len as u64 {
        1
    } else {
        len as i64 + pos + 1
    }
}

/// `getendpos`: end positions clamped to `0..=len`.
#[inline]
fn end_pos(pos: i64, len: usize) -> i64 {
    if pos > len as i64 {
        len as i64
    } else if pos >= 0 {
        pos
    } else if pos.unsigned_abs() > len as u64 {
        0
    } else {
        len as i64 + pos + 1
    }
}

fn push_bytes(l: &mut LuaState, bytes: &[u8]) -> LuaResult<()> {
    let value = l.create_bytes(bytes)?;
    l.push_value(value)
}

fn push_buffer(l: &mut LuaState, bytes: Vec<u8>) -> LuaResult<()> {
    let value = l.create_binary(bytes)?;
    l.push_value(value)
}

/// string.byte(s [, i [, j]])
fn string_byte(l: &mut LuaState) -> LuaResult<usize> {
    let s = lauxlib::check_lstring(l, 1)?;
    let pi = lauxlib::opt_integer(l, 2, 1)?;
    let pose = end_pos(lauxlib::opt_integer(l, 3, pi)?, s.len());
    let posi = start_pos(pi, s.len());
    if posi > pose {
        return Ok(0);
    }
    let n = (pose - posi + 1) as usize;
    if n >= i32::MAX as usize || !l.check_stack(n) {
        return Err(lauxlib::lual_error(l, "string slice too long"));
    }
    l.ensure_stack_capacity(n)?;
    for &byte in &s[posi as usize - 1..pose as usize] {
        l.push_value(LuaValue::integer(i64::from(byte)))?;
    }
    Ok(n)
}

/// string.char(...)
fn string_char(l: &mut LuaState) -> LuaResult<usize> {
    let n = l.arg_count();
    let mut bytes = Vec::with_capacity(n);
    for i in 1..=n {
        let c = lauxlib::check_integer(l, i)?;
        if !(0..=255).contains(&c) {
            return Err(lauxlib::argerror(l, i, "value out of range"));
        }
        bytes.push(c as u8);
    }
    push_bytes(l, &bytes)?;
    Ok(1)
}

/// string.dump(function [, strip])
fn string_dump(l: &mut LuaState) -> LuaResult<usize> {
    let function = l.get_arg(1).unwrap_or_default();
    if !function.is_function() {
        return Err(lauxlib::typeerror(l, 1, "function"));
    }
    let strip = l.get_arg(2).is_some_and(|v| v.is_truthy());
    let Some(function) = function.as_lua_function() else {
        return Err(lauxlib::lual_error(l, "unable to dump given function"));
    };
    let chunk = function.chunk();
    let dumped = if l.global_state().language() == LuaLanguageLevel::Lua53 {
        chunk53::dump(chunk, strip)
    } else {
        chunk_serializer::serialize_chunk_with_pool(chunk, strip)
    };
    match dumped {
        Ok(bytes) => {
            push_buffer(l, bytes)?;
            Ok(1)
        }
        Err(_) => Err(lauxlib::lual_error(l, "unable to dump given function")),
    }
}

/// string.len(s)
fn string_len(l: &mut LuaState) -> LuaResult<usize> {
    let len = lauxlib::check_lstring(l, 1)?.len();
    l.push_value(LuaValue::integer(len as i64))?;
    Ok(1)
}

fn map_bytes(l: &mut LuaState, map: fn(&mut [u8])) -> LuaResult<usize> {
    let s = lauxlib::check_lstring(l, 1)?;
    let mut bytes = s.to_vec();
    map(&mut bytes);
    push_buffer(l, bytes)?;
    Ok(1)
}

/// string.lower(s): C-locale `tolower` on each byte.
fn string_lower(l: &mut LuaState) -> LuaResult<usize> {
    map_bytes(l, <[u8]>::make_ascii_lowercase)
}

/// string.upper(s): C-locale `toupper` on each byte.
fn string_upper(l: &mut LuaState) -> LuaResult<usize> {
    map_bytes(l, <[u8]>::make_ascii_uppercase)
}

/// string.reverse(s)
fn string_reverse(l: &mut LuaState) -> LuaResult<usize> {
    map_bytes(l, <[u8]>::reverse)
}

/// string.rep(s, n [, sep])
fn string_rep(l: &mut LuaState) -> LuaResult<usize> {
    let s = lauxlib::check_lstring(l, 1)?;
    let n = lauxlib::check_integer(l, 2)?;
    let sep = lauxlib::opt_lstring(l, 3)?;
    let sep: &[u8] = sep.as_deref().unwrap_or_default();
    if n <= 0 || s.len() + sep.len() == 0 {
        push_bytes(l, b"")?;
        return Ok(1);
    }
    let total = (s.len() as u64 + sep.len() as u64)
        .checked_mul(n as u64)
        .map(|t| t - sep.len() as u64);
    if total.is_none_or(|t| t > MAX_STRING_SIZE as u64) {
        return Err(lauxlib::lual_error(l, "resulting string too large"));
    }
    let mut result = Vec::with_capacity(total.unwrap_or_default() as usize);
    for i in 0..n {
        if i > 0 {
            result.extend_from_slice(sep);
        }
        result.extend_from_slice(&s);
    }
    push_buffer(l, result)?;
    Ok(1)
}

/// string.sub(s, i [, j])
fn string_sub(l: &mut LuaState) -> LuaResult<usize> {
    let s = lauxlib::check_lstring(l, 1)?;
    let start = start_pos(lauxlib::check_integer(l, 2)?, s.len());
    let end = end_pos(lauxlib::opt_integer(l, 3, -1)?, s.len());
    if start > end {
        push_bytes(l, b"")?;
    } else if start == 1 && end as usize == s.len() && let LStr::Value(value) = s {
        l.push_value(value)?;
    } else {
        let (start, end) = (start as usize - 1, end as usize);
        if end == start + 1
            && let Some(value) = l.global_state().object_allocator.get_byte_string(s[start])
        {
            l.push_value(value)?;
        } else {
            push_bytes(l, &s[start..end])?;
        }
    }
    Ok(1)
}

/// Raise a pattern error as `luaL_error` does.
fn pattern_error(l: &mut LuaState, message: PatternError) -> crate::LuaError {
    lauxlib::lual_error(l, message)
}

/// Push capture `i` of the match `s..e` (`push_onecapture`).
fn push_capture(l: &mut LuaState, ms: &MatchState, i: usize, s: usize, e: usize) -> LuaResult<()> {
    match ms.get_capture(i, s, e) {
        Ok(Cap::Str(start, end)) => push_bytes(l, &ms.src[start..end]),
        Ok(Cap::Pos(position)) => l.push_value(LuaValue::integer(position as i64)),
        Err(message) => Err(pattern_error(l, message)),
    }
}

/// Push all captures (`push_captures`); the whole match if `whole` and the
/// pattern has none.
fn push_captures(l: &mut LuaState, ms: &MatchState, s: usize, e: usize, whole: bool) -> LuaResult<usize> {
    let n = ms.capture_count(whole);
    if !l.check_stack(n) {
        return Err(lauxlib::lual_error(l, "too many captures"));
    }
    for i in 0..n {
        push_capture(l, ms, i, s, e)?;
    }
    Ok(n)
}

/// The values of all captures of the match `s..e` (the whole match if the
/// pattern has none), as arguments for a replacement function.
fn capture_values(l: &mut LuaState, ms: &MatchState, s: usize, e: usize) -> LuaResult<Vec<LuaValue>> {
    let n = ms.capture_count(true);
    let mut values = Vec::with_capacity(n);
    for i in 0..n {
        values.push(match ms.get_capture(i, s, e) {
            Ok(Cap::Str(start, end)) => l.create_bytes(&ms.src[start..end])?,
            Ok(Cap::Pos(position)) => LuaValue::integer(position as i64),
            Err(message) => return Err(pattern_error(l, message)),
        });
    }
    Ok(values)
}

/// `str_find_aux`: string.find and string.match.
fn str_find_aux(l: &mut LuaState, find: bool) -> LuaResult<usize> {
    let s = lauxlib::check_lstring(l, 1)?;
    let p = lauxlib::check_lstring(l, 2)?;
    let init = lauxlib::opt_integer(l, 3, 1)?;
    let init = if init >= 0 {
        init.max(1)
    } else if init.unsigned_abs() > s.len() as u64 {
        1
    } else {
        s.len() as i64 + init + 1
    };
    if init > s.len() as i64 + 1 {
        l.push_value(LuaValue::nil())?;
        return Ok(1);
    }
    let init = init as usize - 1;
    if find && (l.get_arg(4).is_some_and(|v| v.is_truthy()) || pattern::is_plain(&p)) {
        if let Some(found) = pattern::find_plain(&s[init..], &p) {
            let start = init + found;
            l.push_value(LuaValue::integer(start as i64 + 1))?;
            l.push_value(LuaValue::integer((start + p.len()) as i64))?;
            return Ok(2);
        }
    } else {
        let (anchor, p) = match p.split_first() {
            Some((b'^', rest)) => (true, rest),
            _ => (false, &p[..]),
        };
        let mut ms = MatchState::new(&s, p);
        let first = if anchor { None } else { pattern::first_literal(p) };
        let mut s1 = init;
        loop {
            if let Some(first) = first {
                match s[s1..].iter().position(|&b| b == first) {
                    Some(offset) => s1 += offset,
                    None => break,
                }
            }
            ms.reprep();
            match ms.do_match(s1, 0) {
                Ok(Some(end)) => {
                    return if find {
                        l.push_value(LuaValue::integer(s1 as i64 + 1))?;
                        l.push_value(LuaValue::integer(end as i64))?;
                        Ok(push_captures(l, &ms, 0, 0, false)? + 2)
                    } else {
                        push_captures(l, &ms, s1, end, true)
                    };
                }
                Ok(None) => {}
                Err(message) => return Err(pattern_error(l, message)),
            }
            if anchor || s1 >= s.len() {
                break;
            }
            s1 += 1;
        }
    }
    l.push_value(LuaValue::nil())?;
    Ok(1)
}

/// string.find(s, pattern [, init [, plain]])
fn string_find(l: &mut LuaState) -> LuaResult<usize> {
    str_find_aux(l, true)
}

/// string.match(s, pattern [, init])
fn string_match(l: &mut LuaState) -> LuaResult<usize> {
    str_find_aux(l, false)
}

/// Append the replacement for the match `s..e` (`add_s` / `add_value`);
/// false when the original text is kept.
fn add_value(
    l: &mut LuaState,
    ms: &MatchState,
    out: &mut Vec<u8>,
    s: usize,
    e: usize,
    repl: &LuaValue,
    repl_text: Option<&[u8]>,
) -> LuaResult<()> {
    if let Some(news) = repl_text {
        let mut i = 0;
        while i < news.len() {
            let c = news[i];
            i += 1;
            if c != b'%' {
                out.push(c);
                continue;
            }
            match news.get(i).copied() {
                Some(b'0') => out.extend_from_slice(&ms.src[s..e]),
                Some(d) if d.is_ascii_digit() => match ms.get_capture(usize::from(d - b'1'), s, e) {
                    Ok(Cap::Str(start, end)) => out.extend_from_slice(&ms.src[start..end]),
                    Ok(Cap::Pos(position)) => {
                        out.extend_from_slice(itoa::Buffer::new().format(position).as_bytes())
                    }
                    Err(message) => return Err(pattern_error(l, message)),
                },
                Some(b'%') => out.push(b'%'),
                _ => {
                    return Err(lauxlib::lual_error(l, "invalid use of '%' in replacement string"));
                }
            }
            i += 1;
        }
        return Ok(());
    }
    let result = if repl.is_table() {
        let key = match ms.get_capture(0, s, e) {
            Ok(Cap::Str(start, end)) => l.create_bytes(&ms.src[start..end])?,
            Ok(Cap::Pos(position)) => LuaValue::integer(position as i64),
            Err(message) => return Err(pattern_error(l, message)),
        };
        l.table_get(repl, &key)?.unwrap_or_default()
    } else {
        let args = capture_values(l, ms, s, e)?;
        l.call(*repl, args)?.first().copied().unwrap_or_default()
    };
    if !result.is_truthy() {
        out.extend_from_slice(&ms.src[s..e]);
    } else if let Some(text) = lauxlib::to_lstr(l, &result) {
        out.extend_from_slice(&text);
    } else {
        let message = format!("invalid replacement value (a {})", result.type_name());
        return Err(lauxlib::lual_error(l, message));
    }
    Ok(())
}

/// string.gsub(s, pattern, repl [, n])
fn string_gsub(l: &mut LuaState) -> LuaResult<usize> {
    let src = lauxlib::check_lstring(l, 1)?;
    let p = lauxlib::check_lstring(l, 2)?;
    let repl = l.get_arg(3).unwrap_or_default();
    let repl_text = if repl.is_string() || repl.is_number() {
        Some(lauxlib::check_lstring(l, 3)?)
    } else if repl.is_table() || repl.is_function() {
        None
    } else if l.global_state().language() == LuaLanguageLevel::Lua53 {
        return Err(lauxlib::argerror(l, 3, "string/function/table expected"));
    } else {
        return Err(lauxlib::typeerror(l, 3, "string/function/table"));
    };
    let max_s = lauxlib::opt_integer(l, 4, src.len() as i64 + 1)?;
    let (anchor, p) = match p.split_first() {
        Some((b'^', rest)) => (true, rest),
        _ => (false, &p[..]),
    };
    let mut ms = MatchState::new(&src, p);
    let mut out: Vec<u8> = Vec::with_capacity(src.len());
    let mut s = 0;
    let mut lastmatch = usize::MAX;
    let mut n: i64 = 0;
    let first = if anchor { None } else { pattern::first_literal(p) };
    while n < max_s {
        if let Some(first) = first {
            // No match can start before the next occurrence of the literal.
            let skip = src[s..].iter().position(|&b| b == first).unwrap_or(src.len() - s);
            out.extend_from_slice(&src[s..s + skip]);
            s += skip;
        }
        ms.reprep();
        let end = match ms.do_match(s, 0) {
            Ok(end) => end,
            Err(message) => return Err(pattern_error(l, message)),
        };
        match end {
            Some(e) if e != lastmatch => {
                n += 1;
                add_value(l, &ms, &mut out, s, e, &repl, repl_text.as_deref())?;
                s = e;
                lastmatch = e;
            }
            _ if s < src.len() => {
                out.push(src[s]);
                s += 1;
            }
            _ => break,
        }
        if anchor {
            break;
        }
    }
    if n == 0 && let LStr::Value(original) = src {
        l.push_value(original)?;
    } else {
        out.extend_from_slice(&src[s..]);
        push_buffer(l, out)?;
    }
    l.push_value(LuaValue::integer(n))?;
    Ok(2)
}

/// string.gmatch(s, pattern [, init]); `init` exists from Lua 5.4 on. A
/// leading '^' is not an anchor here.
fn string_gmatch(l: &mut LuaState) -> LuaResult<usize> {
    let s = lauxlib::check_lstring(l, 1)?;
    let p = lauxlib::check_lstring(l, 2)?;
    let init = if l.global_state().language() == LuaLanguageLevel::Lua53 {
        0
    } else {
        let init = start_pos(lauxlib::opt_integer(l, 3, 1)?, s.len()) - 1;
        (init as usize).min(s.len() + 1)
    };
    let s = match s {
        LStr::Value(value) => value,
        LStr::Number(text) => l.create_bytes(text.as_bytes())?,
    };
    let p = match p {
        LStr::Value(value) => value,
        LStr::Number(text) => l.create_bytes(text.as_bytes())?,
    };
    // Upvalues: subject, pattern, next start, end of the last match (-1: none).
    let closure = l.global_state_mut().create_c_closure(
        gmatch_aux,
        vec![s, p, LuaValue::integer(init as i64), LuaValue::integer(-1)],
    )?;
    l.push_value(closure)?;
    Ok(1)
}

/// Iterator of string.gmatch (`gmatch_aux`).
fn gmatch_aux(l: &mut LuaState) -> LuaResult<usize> {
    let function = l.get_frame_func(l.call_depth().wrapping_sub(1)).unwrap_or_default();
    let Some(closure) = function.as_cclosure() else {
        return Ok(0);
    };
    let upvalues = closure.upvalues();
    let (s, p) = (upvalues[0], upvalues[1]);
    let start = upvalues[2].as_integer().unwrap_or(0) as usize;
    let lastmatch = upvalues[3].as_integer().unwrap_or(-1);
    let src = s.as_bytes().unwrap_or_default();
    let mut ms = MatchState::new(src, p.as_bytes().unwrap_or_default());
    for s1 in start..=src.len() {
        ms.reprep();
        match ms.do_match(s1, 0) {
            Ok(Some(e)) if e as i64 != lastmatch => {
                if let Some(closure) = function.as_cclosure_mut() {
                    let upvalues = closure.upvalues_mut();
                    upvalues[2] = LuaValue::integer(e as i64);
                    upvalues[3] = LuaValue::integer(e as i64);
                }
                return push_captures(l, &ms, s1, e, true);
            }
            Ok(_) => {}
            Err(message) => return Err(pattern_error(l, message)),
        }
    }
    Ok(0)
}
