// Basic library (_G global functions)
// Implements: print, type, assert, error, tonumber, tostring,
// select, ipairs, pairs, next, pcall, xpcall, getmetatable, setmetatable,
// rawget, rawset, rawlen, rawequal, collectgarbage, dofile, loadfile, load
pub mod parse_number;
mod require;

use crate::gc::{GcKind, MAJORMINOR, MINORMAJOR, MINORMUL, PAUSE, STEPMUL, STEPSIZE};
use crate::gc::{code_param, decode_param};
use crate::lib_registry::LibraryModule;
use crate::lua_value::{
    LuaValue, LuaValueKind, UpvalueStore, lua_value_to_udvalue, udvalue_to_lua_value,
};
use crate::lua_vm::{LuaError, LuaResult, LuaState, get_metatable};
use crate::stdlib::basic::parse_number::parse_lua_number;
use crate::stdlib::lauxlib;
use require::lua_require;

pub fn create_basic_lib() -> LibraryModule {
    crate::lib_module!("_G", {
        "print" => lua_print,
        "type" => lua_type,
        "assert" => lua_assert,
        "error" => lua_error,
        "tonumber" => lua_tonumber,
        "tostring" => lua_tostring,
        "select" => lua_select,
        "ipairs" => lua_ipairs,
        "pairs" => lua_pairs,
        "next" => lua_next,
        "pcall" => lua_pcall,
        "xpcall" => lua_xpcall,
        "getmetatable" => lua_getmetatable,
        "setmetatable" => lua_setmetatable,
        "rawget" => lua_rawget,
        "rawset" => lua_rawset,
        "rawlen" => lua_rawlen,
        "rawequal" => lua_rawequal,
        "collectgarbage" => lua_collectgarbage,
        "require" => lua_require,
        "load" => lua_load,
        "loadfile" => lua_loadfile,
        "dofile" => lua_dofile,
        "warn" => lua_warn,
    })
    .with_value("_VERSION", |vm| {
        let version = vm.version;
        vm.create_string_owned(format!("{}", version))
    })
}

/// print(...) - Print values to stdout
fn lua_print(l: &mut LuaState) -> LuaResult<usize> {
    let arg_count = l.arg_count();
    // Lua 5.3 converts through the global `tostring`; Lua 5.4+ uses luaL_tolstring.
    let tostring = if l.global_state().language() == crate::LuaLanguageLevel::Lua53 {
        Some(l.get_global_value("tostring")?.unwrap_or_default())
    } else {
        None
    };
    let mut output = Vec::new();
    for index in 1..=arg_count {
        let arg = l.get_arg(index).unwrap_or_default();
        if index > 1 {
            output.push(b'\t');
        }
        let text = match tostring {
            Some(tostring) => {
                let value = l.call(tostring, vec![arg])?.first().copied().unwrap_or_default();
                match lauxlib::to_lstr(l, &value) {
                    Some(text) => text,
                    None => {
                        return Err(lauxlib::lual_error(l, "'tostring' must return a string to 'print'"));
                    }
                }
            }
            None => lauxlib::tolstring(l, &arg)?,
        };
        output.extend_from_slice(&text);
    }
    output.push(b'\n');
    std::io::Write::write_all(&mut std::io::stdout().lock(), &output)
        .map_err(|error| l.error(error.to_string()))?;
    Ok(0)
}

/// type(v) - Return the type of a value as a string
fn lua_type(l: &mut LuaState) -> LuaResult<usize> {
    let value = match l.get_arg(1) {
        Some(v) => v,
        None => {
            return Err(l.error("bad argument #1 to 'type' (value expected)".to_string()));
        }
    };

    let cs = &l.global_state_mut().const_strings;
    let result = match value.kind() {
        LuaValueKind::Nil => cs.str_nil,
        LuaValueKind::Boolean => cs.str_boolean,
        LuaValueKind::Integer | LuaValueKind::Float => cs.str_number,
        LuaValueKind::String => cs.str_string,
        LuaValueKind::Table => cs.str_table,
        LuaValueKind::Function
        | LuaValueKind::CFunction
        | LuaValueKind::CClosure
        | LuaValueKind::RClosure => cs.str_function,
        LuaValueKind::Userdata => cs.str_userdata,
        LuaValueKind::Thread => cs.str_thread,
    };

    l.push_value(result)?;
    Ok(1)
}

/// assert(v [, message]) - Raise error if v is false or nil
fn lua_assert(l: &mut LuaState) -> LuaResult<usize> {
    let condition = lauxlib::check_any(l, 1)?;
    if condition.is_truthy() {
        return Ok(l.arg_count());
    }
    // Like luaB_assert: the message (default "assertion failed!") goes
    // through 'error' with level 1.
    let message = match l.get_arg(2) {
        Some(message) => message,
        None => l.create_string("assertion failed!")?,
    };
    if let Some(text) = message.as_bytes() {
        let mut full = lauxlib::lual_where(l, 1).into_bytes();
        full.extend_from_slice(text);
        let error = l.create_bytes(&full)?;
        return Err(l.error_with_object(error));
    }
    Err(l.error_with_object(message))
}

/// error(message [, level]) - Raise an error
fn lua_error(l: &mut LuaState) -> LuaResult<usize> {
    let level = lauxlib::opt_integer(l, 2, 1)?;
    let arg = l.get_arg(1).unwrap_or_default();
    if level > 0
        && let Some(message) = arg.as_bytes()
    {
        let mut text = lauxlib::lual_where(l, level as usize).into_bytes();
        text.extend_from_slice(message);
        let error = l.create_bytes(&text)?;
        return Err(l.error_with_object(error));
    }
    Err(l.error_with_object(arg))
}

/// `l_str2int` with an explicit base (tonumber(s, base)).
fn str_to_int_base(text: &[u8], base: u32) -> Option<i64> {
    let is_space = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let mut rest = &text[text.iter().take_while(|b| is_space(b)).count()..];
    let negative = rest.first() == Some(&b'-');
    if negative || rest.first() == Some(&b'+') {
        rest = &rest[1..];
    }
    let digits = rest.iter().take_while(|b| b.is_ascii_alphanumeric()).count();
    if digits == 0 || rest[digits..].iter().any(|b| !is_space(b)) {
        return None;
    }
    let mut n: u64 = 0;
    for &b in &rest[..digits] {
        let digit = char::from(b).to_digit(36)?;
        if digit >= base {
            return None;
        }
        n = n.wrapping_mul(u64::from(base)).wrapping_add(u64::from(digit));
    }
    Some(if negative { 0u64.wrapping_sub(n) } else { n } as i64)
}

/// tonumber(e [, base]) - Convert to number
fn lua_tonumber(l: &mut LuaState) -> LuaResult<usize> {
    let has_base = l.get_arg(2).is_some_and(|base| !base.is_nil());
    let result = if !has_base {
        match l.get_arg(1) {
            Some(value) if value.is_number() => value,
            Some(value) => match value.as_bytes() {
                Some(text) => std::str::from_utf8(text)
                    .map(parse_lua_number)
                    .unwrap_or_default(),
                None => LuaValue::nil(),
            },
            // LuaTeX's Lua 5.3 answers nil; Lua 5.5 requires an argument.
            None if l.global_state().language() == crate::LuaLanguageLevel::Lua53 => LuaValue::nil(),
            None => return Err(lauxlib::argerror(l, 1, "value expected")),
        }
    } else {
        let base = lauxlib::check_integer(l, 2)?;
        let text = match l.get_arg(1) {
            Some(value) if value.is_string() => value,
            _ => return Err(lauxlib::typeerror(l, 1, "string")),
        };
        if !(2..=36).contains(&base) {
            return Err(lauxlib::argerror(l, 2, "base out of range"));
        }
        match str_to_int_base(text.as_bytes().unwrap_or_default(), base as u32) {
            Some(n) => LuaValue::integer(n),
            None => LuaValue::nil(),
        }
    };
    l.push_value(result)?;
    Ok(1)
}

/// tostring(v) - Convert to string
fn lua_tostring(l: &mut LuaState) -> LuaResult<usize> {
    let value = lauxlib::check_any(l, 1)?;
    let result = match lauxlib::tolstring(l, &value)? {
        lauxlib::LStr::Value(result) => result,
        lauxlib::LStr::Number(text) => l.create_bytes(text.as_bytes())?,
    };
    l.push_value(result)?;
    Ok(1)
}

/// select(index, ...) - Return subset of arguments
/// select(index, ...) - Return subset of arguments
/// OPTIMIZED: Use ensure_stack_capacity + push_value_unchecked, cache base
fn lua_select(l: &mut LuaState) -> LuaResult<usize> {
    let index_arg = l
        .get_arg(1)
        .ok_or_else(|| l.error("bad argument #1 to 'select' (value expected)".to_string()))?;

    // Total args after index
    let total_args = l.arg_count();
    let vararg_count = if total_args > 0 { total_args - 1 } else { 0 };

    // FAST PATH: Check for "#"
    if let Some(s) = index_arg.as_str() {
        if s == "#" {
            let result = LuaValue::integer(vararg_count as i64);
            l.push_value(result)?;
            return Ok(1);
        }
        return Err(l.error("bad argument #1 to 'select' (number expected)".to_string()));
    }

    let index = index_arg
        .as_integer()
        .ok_or_else(|| l.error("bad argument #1 to 'select' (number expected)".to_string()))?;

    if index == 0 {
        return Err(l.error("bad argument #1 to 'select' (index out of range)".to_string()));
    }

    // Calculate start position (1-based to 0-based)
    let start_idx = if index > 0 {
        (index - 1) as usize
    } else {
        let abs_idx = (-index) as usize;
        if abs_idx > vararg_count {
            return Err(l.error("bad argument #1 to 'select' (index out of range)".to_string()));
        }
        vararg_count - abs_idx
    };

    if start_idx >= vararg_count {
        return Ok(0);
    }

    let result_count = vararg_count - start_idx;

    // Ensure stack has room for all results at once, then use unchecked push
    l.ensure_stack_capacity(result_count)?;

    // Cache base to avoid repeated frame lookups
    let frame = l.current_frame().expect("select requires active frame");
    let base = frame.base;
    let top = frame.top as usize;

    // first_arg_idx is 1-based: arg 2 + start_idx → stack offset = base + 1 + start_idx
    let stack_start = base + 1 + start_idx;

    for i in 0..result_count {
        let stack_idx = stack_start + i;
        let val = if stack_idx < top && stack_idx < l.stack.len() {
            l.stack[stack_idx]
        } else {
            LuaValue::nil()
        };
        l.push_value(val)?;
    }

    Ok(result_count)
}

/// ipairs(t) - Return iterator for array part of table
/// Like C Lua 5.5: accepts any value with metatable, uses lua_geti (triggers __index)
fn lua_ipairs(l: &mut LuaState) -> LuaResult<usize> {
    let table_val = l
        .get_arg(1)
        .ok_or_else(|| l.error("bad argument #1 to 'ipairs' (value expected)".to_string()))?;

    // Return iterator function, table, and 0 (3 values)
    // Note: C Lua 5.5 does NOT require arg1 to be a table (luaL_checkany)
    let iter_func = LuaValue::cfunction(ipairs_next);
    l.push_value(iter_func)?;
    l.push_value(table_val)?;
    l.push_value(LuaValue::integer(0))?;
    Ok(3)
}

/// Iterator function for ipairs — like C Lua's ipairsaux.
#[inline]
fn ipairs_next(l: &mut LuaState) -> LuaResult<usize> {
    let table_val = l.get_arg(1).unwrap_or_default();
    let index_val = l.get_arg(2).unwrap_or_default();

    let index = match index_val.as_integer() {
        Some(i) => i,
        None => return Err(l.error("ipairs iterator: invalid index".to_string())),
    };
    let next_index = index.wrapping_add(1);

    // Single table dereference — cache it for both has_metatable + raw_geti.
    let value = if let Some(table) = table_val.as_table_mut() {
        if !table.has_metatable() {
            // Hot path: no metatable → direct array access
            table.raw_geti(next_index).unwrap_or(LuaValue::nil())
        } else {
            // Cold: metatable present → full __index dispatch
            l.table_geti(&table_val, next_index)?
        }
    } else {
        l.table_geti(&table_val, next_index)?
    };

    if !value.is_nil() {
        l.push_value(LuaValue::integer(next_index))?;
        l.push_value(value)?;
        Ok(2)
    } else {
        // Return nil to signal end of iteration
        l.push_value(LuaValue::nil())?;
        Ok(1)
    }
}

/// pairs(t) - Return iterator for all key-value pairs.
/// Lua 5.3 returns exactly three values; Lua 5.5 adds a close value.
fn lua_pairs(l: &mut LuaState) -> LuaResult<usize> {
    let val = l.get_arg(1).ok_or_else(|| {
        l.error("bad argument #1 to 'pairs' (table or userdata expected)".to_string())
    })?;

    // Check for __pairs metamethod first (like C Lua 5.5)
    if val.is_table() || val.is_userdata() {
        let pairs_key = l.global_state_mut().const_strings.tm_pairs;
        if let Some(mt) = get_metatable(l, &val)
            && let Some(mt_table) = mt.as_table()
            && let Some(mm) = mt_table.raw_get(&pairs_key)
        {
            // Use stack-based call for yield support (equivalent to
            // C Lua's lua_insert + lua_callk pattern).
            // Place __pairs at call_base and the table arg right after it,
            // so that finish_c_frame(CIST_YCALL) moves results correctly
            // on yield-resume (body_results_start == call_base).
            let call_base = l.current_frame().unwrap().base;
            l.stack_set(call_base, mm)?;
            l.stack_set(call_base + 1, val)?;
            l.set_top(call_base + 2)?;

            let num_results = l.call_stack_based(call_base, 1)?;

            let wanted = if l.global_state().language() == crate::LuaLanguageLevel::Lua53 {
                3
            } else {
                4
            };
            if num_results < wanted {
                for _ in num_results..wanted {
                    l.push_value(LuaValue::nil())?;
                }
            } else if num_results > wanted {
                l.set_top(call_base + wanted)?;
            }
            return Ok(wanted);
        }
    }

    let lua53 = l.global_state().language() == crate::LuaLanguageLevel::Lua53;
    if val.is_table() {
        let next_func = LuaValue::cfunction(lua_next);
        l.push_value(next_func)?;
        l.push_value(val)?;
        l.push_value(LuaValue::nil())?;
        if lua53 {
            Ok(3)
        } else {
            l.push_value(LuaValue::nil())?;
            Ok(4)
        }
    } else if val.is_userdata() {
        let next_func = LuaValue::cfunction(lua_userdata_next);
        l.push_value(next_func)?;
        l.push_value(val)?;
        l.push_value(LuaValue::nil())?;
        if lua53 {
            Ok(3)
        } else {
            l.push_value(LuaValue::nil())?;
            Ok(4)
        }
    } else {
        Err(l.error("bad argument #1 to 'pairs' (table or userdata expected)".to_string()))
    }
}

/// Iterator function for userdata pairs().
/// Delegates to `UserDataTrait::lua_next(control)` — a stateless Rust iterator.
/// Returns (next_control, value) or nil when exhausted.
fn lua_userdata_next(l: &mut LuaState) -> LuaResult<usize> {
    let ud_val = l.get_arg(1).unwrap_or_default();
    let key_val = l.get_arg(2).unwrap_or_default();

    let ud = ud_val.as_userdata_mut().ok_or_else(|| {
        l.error("bad argument #1 to userdata iterator (userdata expected)".to_string())
    })?;

    // Convert the Lua control variable to UdValue for the trait call
    let control = lua_value_to_udvalue(&key_val);

    let trait_obj = ud.get_trait()?;

    match trait_obj.lua_next(&control) {
        Some((next_control, value)) => {
            let k = udvalue_to_lua_value(l, next_control)?;
            let v = udvalue_to_lua_value(l, value)?;
            l.push_value(k)?;
            l.push_value(v)?;
            Ok(2)
        }
        None => {
            // Iteration exhausted
            l.push_value(LuaValue::nil())?;
            Ok(1)
        }
    }
}

/// next(table [, index]) - Return next key-value pair
/// Port of Lua 5.5's luaB_next using luaH_next
fn lua_next(l: &mut LuaState) -> LuaResult<usize> {
    // arg 1 is the table (required), arg 2 is the key (optional, defaults to nil)
    let table_val = l.get_arg(1).unwrap_or_default();
    let index_val = l.get_arg(2).unwrap_or_default();

    let result = {
        let table = table_val
            .as_table()
            .ok_or_else(|| l.error("bad argument #1 to 'next' (table expected)".to_string()))?;
        table
            .next(&index_val)
            .map_err(|_| l.error("invalid key to 'next'".to_string()))?
    };

    if let Some((k, v)) = result {
        l.push_value(k)?;
        l.push_value(v)?;
        Ok(2)
    } else {
        l.push_value(LuaValue::nil())?;
        Ok(1)
    }
}

/// pcall(f [, arg1, ...]) - Protected call
fn lua_pcall(l: &mut LuaState) -> LuaResult<usize> {
    // Arguments are already on stack from the call:
    // stack: [pcall_func, target_func, arg1, arg2, ...]
    // We need: [target_func, arg1, arg2, ...] and call it

    let arg_count = l.arg_count();
    if arg_count < 1 {
        return Err(l.error("bad argument #1 to 'pcall' (value expected)".to_string()));
    }

    // Get current frame info
    let base = l
        .current_frame()
        .map(|f| f.base)
        .ok_or(LuaError::RuntimeError)?;

    // func is at base+0, args are at base+1..base+arg_count-1
    // We want to call func with arg_count-1 arguments
    let func_idx = base;
    let call_arg_count = arg_count - 1;

    // Call using stack-based API (no Vec allocation!)
    let (success, result_count) = l.pcall_stack_based(func_idx, call_arg_count)?;

    // Results at stack[func_idx..func_idx+result_count], top = func_idx + result_count.
    // Need to return [bool, result1, result2, ...] — shift results right by 1.
    // Push a nil to ensure stack capacity for the extra boolean slot.
    l.push_value(LuaValue::nil())?;

    // In-place shift: move results right by 1, insert boolean at func_idx.
    // Zero allocation, single O(n) copy.
    {
        let stack = l.stack_mut();
        for i in (0..result_count).rev() {
            stack[func_idx + 1 + i] = stack[func_idx + i];
        }
        stack[func_idx] = LuaValue::boolean(success);
    }

    Ok(result_count + 1)
}

/// xpcall(f, msgh [, arg1, ...]) - Protected call with error handler
fn lua_xpcall(l: &mut LuaState) -> LuaResult<usize> {
    // xpcall(f, msgh, arg1, arg2, ...)
    // Stack layout from call:
    //   xpcall's C frame has: base+0=f, base+1=msgh, base+2..=args
    let arg_count = l.arg_count();
    if arg_count < 2 {
        return Err(l.error("bad argument #2 to 'xpcall' (value expected)".to_string()));
    }

    let base = l
        .current_frame()
        .map(|f| f.base)
        .ok_or(LuaError::RuntimeError)?;

    // Like luaB_xpcall, leave the frame's own slots alone (a traceback then
    // names this frame 'xpcall'): the handler stays at base+1 and a copy of f
    // followed by the arguments goes at base+2.. (finish_c_frame and the
    // coroutine recovery path rely on this layout).
    let call_arg_count = arg_count - 2;
    let func_idx = base + 2;
    let handler_idx = base + 1;
    l.set_top(func_idx + 1 + call_arg_count)?;
    {
        let stack = l.stack_mut();
        for i in (0..call_arg_count).rev() {
            stack[func_idx + 1 + i] = stack[func_idx + i];
        }
        stack[func_idx] = stack[base];
    }

    // Mark current (xpcall's) C frame with CIST_XPCALL
    // so finish_c_frame knows to apply the error handler on error recovery.
    {
        use crate::lua_vm::call_info::call_status::CIST_XPCALL;
        let frame_idx = l.call_depth() - 1;
        let ci = l.get_call_info_mut(frame_idx);
        ci.call_status |= CIST_XPCALL;
    }

    // Call using xpcall_stack_based which calls handler BEFORE unwinding frames
    let (success, result_count) = l.xpcall_stack_based(func_idx, call_arg_count, handler_idx)?;

    if success {
        // Results at func_idx..func_idx+result_count; true goes just below.
        l.stack_mut()[func_idx - 1] = LuaValue::boolean(true);
        Ok(result_count + 1)
    } else {
        // Error — handler already called by xpcall_stack_based, result is at func_idx
        let transformed_error = l.stack_get(func_idx).unwrap_or_default();
        l.push_value(LuaValue::boolean(false))?;
        l.push_value(transformed_error)?;
        Ok(2)
    }
}

/// getmetatable(object) - Get metatable
fn lua_getmetatable(l: &mut LuaState) -> LuaResult<usize> {
    let value = l
        .get_arg(1)
        .ok_or_else(|| l.error("bad argument #1 to 'getmetatable' (value expected)".to_string()))?;

    let mt = get_metatable(l, &value);
    match mt {
        Some(mt_val) => {
            // Check for __metatable field - if present, return that instead
            if let Some(table) = mt_val.as_table() {
                let key = l.create_string("__metatable")?;
                if let Some(mm) = table.raw_get(&key)
                    && !mm.is_nil()
                {
                    l.push_value(mm)?;
                    return Ok(1);
                }
            }
            l.push_value(mt_val)?;
        }
        None => {
            l.push_value(LuaValue::nil())?;
        }
    }
    Ok(1)
}

/// setmetatable(table, metatable) - Set metatable
fn lua_setmetatable(l: &mut LuaState) -> LuaResult<usize> {
    let table = l
        .get_arg(1)
        .ok_or_else(|| l.error("bad argument #1 to 'setmetatable' (value expected)".to_string()))?;
    let metatable = l
        .get_arg(2)
        .ok_or_else(|| l.error("bad argument #2 to 'setmetatable' (value expected)".to_string()))?;

    if let Some(table_ref) = table.as_table_mut() {
        // Check for __metatable protection on existing metatable
        if let Some(existing_mt) = table_ref.get_metatable()
            && let Some(mt_table) = existing_mt.as_table()
        {
            let key = l.create_string("__metatable")?;
            let has_protection = mt_table.raw_get(&key).is_some_and(|v| !v.is_nil());
            if has_protection {
                return Err(l.error("cannot change a protected metatable".to_string()));
            }
        }

        match metatable.kind() {
            LuaValueKind::Nil => {
                table_ref.set_metatable(None);
            }
            LuaValueKind::Table => {
                table_ref.set_metatable(Some(metatable));
            }
            _ => {
                return Err(
                    l.error("setmetatable() second argument must be a table or nil".to_string())
                );
            }
        }

        // GC write barrier: if the table is BLACK and the new metatable is WHITE,
        // the GC must be notified. barrier_back turns the table back to GRAY
        // so it gets re-traversed and the metatable gets properly marked.
        // Without this, the metatable can be swept (freed) while the table still
        // references it → dangling pointer → heap corruption.
        if let Some(gc_ptr) = table.as_gc_ptr() {
            l.gc_barrier_back(gc_ptr);
        }
    }

    // Lua 5.5: luaC_checkfinalizer - register object if __gc is present
    l.global_state_mut().gc.check_finalizer(&table);
    // Return the original table
    l.push_value(table)?;
    Ok(1)
}

/// rawget(table, index) - Get without metamethods
fn lua_rawget(l: &mut LuaState) -> LuaResult<usize> {
    let table = l
        .get_arg(1)
        .ok_or_else(|| l.error("bad argument #1 to 'rawget' (value expected)".to_string()))?;
    let key = l
        .get_arg(2)
        .ok_or_else(|| l.error("bad argument #2 to 'rawget' (value expected)".to_string()))?;

    let value = table
        .as_table()
        .map(|table_ref| table_ref.raw_get(&key).unwrap_or(LuaValue::nil()));

    if let Some(v) = value {
        l.push_value(v)?;
        return Ok(1);
    }
    Err(l.error("bad argument #1 to 'rawget' (table expected)".to_string()))
}

/// rawset(table, index, value) - Set without metamethods
fn lua_rawset(l: &mut LuaState) -> LuaResult<usize> {
    let table = l
        .get_arg(1)
        .ok_or_else(|| l.error("bad argument #1 to 'rawset' (value expected)".to_string()))?;
    let key = l
        .get_arg(2)
        .ok_or_else(|| l.error("bad argument #2 to 'rawset' (value expected)".to_string()))?;
    let value = l
        .get_arg(3)
        .ok_or_else(|| l.error("bad argument #3 to 'rawset' (value expected)".to_string()))?;

    if table.is_table() {
        // Check for NaN key
        if key.is_float()
            && let Some(f) = key.as_number()
            && f.is_nan()
        {
            return Err(l.error("table index is NaN".to_string()));
        }
        l.raw_set(&table, key, value);
        l.push_value(table)?;
        return Ok(1);
    }
    Err(l.error("bad argument #1 to 'rawset' (table expected)".to_string()))
}

/// rawlen(v) - Length without metamethods
fn lua_rawlen(l: &mut LuaState) -> LuaResult<usize> {
    let value = l
        .get_arg(1)
        .ok_or_else(|| l.error("bad argument #1 to 'rawlen' (value expected)".to_string()))?;

    let len = match value.kind() {
        LuaValueKind::Table => {
            if let Some(table) = value.as_table() {
                table.len() as i64
            } else {
                return Err(
                    l.error("bad argument #1 to 'rawlen' (table or string expected)".to_string())
                );
            }
        }
        LuaValueKind::String => {
            if let Some(s) = value.as_str() {
                s.len() as i64
            } else {
                return Err(
                    l.error("bad argument #1 to 'rawlen' (table or string expected)".to_string())
                );
            }
        }
        _ => {
            return Err(
                l.error("bad argument #1 to 'rawlen' (table or string expected)".to_string())
            );
        }
    };

    l.push_value(LuaValue::integer(len))?;
    Ok(1)
}

/// rawequal(v1, v2) - Equality without metamethods
fn lua_rawequal(l: &mut LuaState) -> LuaResult<usize> {
    let v1 = l.get_arg(1).unwrap_or_default();
    let v2 = l.get_arg(2).unwrap_or_default();

    let result = v1 == v2;
    l.push_value(LuaValue::boolean(result))?;
    Ok(1)
}

/// collectgarbage([opt [, arg, arg2]]) - Garbage collector control
/// Lua 5.5 version with full parameter support including the new 'param' option
fn lua_collectgarbage(l: &mut LuaState) -> LuaResult<usize> {
    // Check if GC is internally stopped (like Lua 5.5's gcstp & (GCSTPGC | GCSTPCLS))
    // GCSTPGC means GC is currently running (prevents reentrancy)
    // From lapi.c line 1174: if (g->gcstp & (GCSTPGC | GCSTPCLS)) return -1;
    if l.global_state_mut().gc.gc_stopem {
        // Return nil (false) to indicate GC is currently running
        // In Lua 5.5, lua_gc returns -1, which is not returned to Lua code
        // The Lua manual says collectgarbage returns false if it cannot run
        l.push_value(LuaValue::nil())?;
        return Ok(1);
    }

    let arg1 = l.get_arg(1);

    let opt = match &arg1 {
        Some(v) if v.is_nil() => "collect".to_string(),
        Some(v) => match v.as_str() {
            Some(s) => s.to_string(),
            None => return Err(crate::stdlib::debug::arg_typeerror(l, 1, "string", v)),
        },
        None => "collect".to_string(),
    };
    let language = l.global_state_mut().language();
    if language == crate::LuaLanguageLevel::Lua53
        && matches!(opt.as_str(), "generational" | "incremental" | "param")
    {
        return Err(l.error(format!(
            "bad argument #1 to 'collectgarbage' (invalid option '{}')",
            opt
        )));
    }

    match opt.as_str() {
        "collect" => {
            l.collect_garbage()?;
            l.push_value(LuaValue::integer(0))?;
            Ok(1)
        }
        "count" => {
            let gc = &l.global_state_mut().gc;
            let real_bytes = gc.total_bytes - gc.gc_debt; // gettotalbytes
            let kb = real_bytes.max(0) as f64 / 1024.0;
            l.push_value(LuaValue::number(kb))?;
            Ok(1)
        }
        "stop" => {
            // LUA_GCSTOP: Stop collector (like Lua's gcstp = GCSTPUSR)
            l.global_state_mut().gc.gc_stopped = true;
            l.push_value(LuaValue::integer(0))?;
            Ok(1)
        }
        "restart" => {
            // LUA_GCRESTART: Restart collector
            // From lapi.c: luaE_setdebt(g, 0); g->gcstp = 0;
            // Exactly like Lua 5.5: debt=0 will trigger GC on next check
            l.global_state_mut().gc.gc_stopped = false;
            l.global_state_mut().gc.set_debt(0);
            l.push_value(LuaValue::integer(0))?;
            Ok(1)
        }
        "step" => {
            // LUA_GCSTEP: Single step with optional size argument (in bytes)
            //
            // From lapi.c (Lua 5.5): lines 1202-1214
            // ```c
            // case LUA_GCSTEP: {
            //   lu_byte oldstp = g->gcstp;
            //   l_mem n = cast(l_mem, va_arg(argp, size_t));
            //   int work = 0;
            //   g->gcstp = 0;
            //   if (n <= 0)
            //     n = g->GCdebt;
            //   luaE_setdebt(g, g->GCdebt - n);
            //   luaC_condGC(L, (void)0, work = 1);
            //   if (work && g->gcstate == GCSpause)
            //     res = 1;
            //   g->gcstp = oldstp;
            //   break;
            // }
            // ```
            let requested = l
                .get_arg(2)
                .and_then(|value| value.as_integer())
                .unwrap_or(0)
                .clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            let gc = &mut l.global_state_mut().gc as *mut crate::gc::GC;
            let completed = unsafe {
                (*gc).api_step(
                    l,
                    if language == crate::LuaLanguageLevel::Lua53 {
                        requested
                    } else {
                        // Lua 5.5's public step argument is bytes.
                        requested.saturating_add(1023) / 1024
                    },
                )
            };
            if let Some(message) = l.global_state_handle().take_finalizer_error() {
                return Err(l.error(message));
            }
            l.push_value(LuaValue::boolean(completed))?;

            Ok(1)
        }
        "setpause" => {
            let new_value = l
                .get_arg(2)
                .and_then(|value| value.as_integer())
                .unwrap_or(0);
            let old_value = {
                let vm = l.global_state_mut();
                let old = decode_param(vm.gc.gc_params[PAUSE]);
                vm.gc.gc_params[PAUSE] = code_param(new_value.max(0) as u32);
                old
            };
            l.push_value(LuaValue::integer(i64::from(old_value)))?;
            Ok(1)
        }
        "setstepmul" => {
            let new_value = l
                .get_arg(2)
                .and_then(|value| value.as_integer())
                .unwrap_or(0);
            let old_value = {
                let vm = l.global_state_mut();
                let old = decode_param(vm.gc.gc_params[STEPMUL]);
                vm.gc.gc_params[STEPMUL] = code_param(new_value.max(0) as u32);
                old
            };
            l.push_value(LuaValue::integer(i64::from(old_value)))?;
            Ok(1)
        }
        "isrunning" => {
            // LUA_GCISRUNNING: Check if collector is running
            // GC is running if not stopped by user
            let is_running = !l.global_state_mut().gc.gc_stopped;
            l.push_value(LuaValue::boolean(is_running))?;
            Ok(1)
        }
        "generational" => {
            // LUA_GCGEN: Switch to generational mode (like luaC_changemode)
            let old_mode = match l.global_state_mut().gc.gc_kind {
                GcKind::Inc => "incremental",
                GcKind::GenMinor => "generational",
                GcKind::GenMajor => "generational",
            };

            l.change_gc_mode(GcKind::GenMinor);

            let mode_value = l.create_string(old_mode)?;
            l.push_value(mode_value)?;
            Ok(1)
        }
        "incremental" => {
            // LUA_GCINC: Switch to incremental mode (like luaC_changemode)
            let old_mode = match l.global_state_mut().gc.gc_kind {
                GcKind::Inc => "incremental",
                GcKind::GenMinor => "generational",
                GcKind::GenMajor => "generational",
            };

            l.change_gc_mode(GcKind::Inc);

            let mode_value = l.create_string(old_mode)?;
            l.push_value(mode_value)?;
            Ok(1)
        }
        "param" => {
            // LUA_GCPARAM: Get/set GC parameters (NEW in Lua 5.5!)
            let arg2 = l.get_arg(2);
            let arg3 = l.get_arg(3);

            // Get parameter name string
            let param_name = if let Some(v) = arg2 {
                v.as_str().map(|s| s.to_string())
            } else {
                None
            };

            if param_name.is_none() {
                return Err(l.error("collectgarbage 'param': parameter name expected".to_string()));
            }

            let param_name = param_name.unwrap();

            // Map parameter name to index
            let param_idx = match param_name.as_str() {
                "minormul" => Some(MINORMUL),     // 0: LUA_GCPMINORMUL
                "majorminor" => Some(MAJORMINOR), // 1: LUA_GCPMAJORMINOR
                "minormajor" => Some(MINORMAJOR), // 2: LUA_GCPMINORMAJOR
                "pause" => Some(PAUSE),           // 3: LUA_GCPPAUSE
                "stepmul" => Some(STEPMUL),       // 4: LUA_GCPSTEPMUL
                "stepsize" => Some(STEPSIZE),     // 5: LUA_GCPSTEPSIZE
                _ => None,
            };

            if param_idx.is_none() {
                return Err(l.error(format!(
                    "collectgarbage 'param': invalid parameter name '{}'",
                    param_name
                )));
            }

            let param_idx = param_idx.unwrap();

            // Get old value and potentially set new value
            let old_value = {
                let vm = l.global_state_mut();
                // Decode the compressed parameter to get actual percentage
                let old = decode_param(vm.gc.gc_params[param_idx]);

                // Set new value if provided
                if let Some(new_val) = arg3
                    && let Some(new_int) = new_val.as_integer()
                {
                    // Encode the new value using Lua 5.5's compressed format
                    vm.gc.gc_params[param_idx] = code_param(new_int as u32);
                }

                old
            };

            // Return old value
            l.push_value(LuaValue::integer(old_value as i64))?;
            Ok(1)
        }
        _ => Err(l.error(format!("collectgarbage: invalid option '{}'", opt))),
    }
}

/// load(chunk [, chunkname [, mode [, env]]]) - Load a chunk
fn lua_load(l: &mut LuaState) -> LuaResult<usize> {
    use crate::lua_value::{chunk_serializer, chunk53};

    let chunk_val = l
        .get_arg(1)
        .ok_or_else(|| l.error("bad argument #1 to 'load' (value expected)".to_string()))?;

    // Save all arguments before potentially calling reader function
    // because calling the reader will modify the stack
    let chunkname_arg = l.get_arg(2).and_then(|v| v.as_str().map(|s| s.to_string()));
    let mode_arg = l.get_arg(3).and_then(|v| v.as_str().map(|s| s.to_string()));
    let env_arg = l.get_arg(4);

    // Check if chunk is callable (function, cfunction, or table with __call metamethod)
    let is_reader = chunk_val.is_function() || chunk_val.is_table();

    let text_source = if !is_reader
        && chunk_val
            .as_bytes()
            .is_some_and(|bytes| bytes.first() != Some(&0x1B))
    {
        chunk_val.as_str()
    } else {
        None
    };

    // Get the chunk string or binary data
    let (code_bytes, is_binary) = if text_source.is_some() {
        (Vec::new(), false)
    } else if is_reader {
        // chunk is a reader function - call it repeatedly to get source
        let mut accumulated = Vec::new();
        let mut is_binary = false;
        let mut first_chunk = true;

        loop {
            // Call the reader function
            l.push_value(chunk_val)?;

            let func_idx = l.get_top() - 1;
            let call_result = l.pcall_stack_based(func_idx, 0);

            let result = match call_result {
                Ok((true, result_count)) => {
                    if result_count > 0 {
                        l.stack_get(func_idx).unwrap_or_default()
                    } else {
                        LuaValue::nil()
                    }
                }
                Ok((false, _)) => {
                    // Error occurred in reader function
                    let error_val = l.stack_get(func_idx).unwrap_or_default();
                    l.set_top(func_idx)?;

                    // Return nil + error message
                    l.push_value(LuaValue::nil())?;
                    let err_msg =
                        l.create_string(&format!("error in reader function: {}", error_val))?;
                    l.push_value(err_msg)?;
                    return Ok(2);
                }
                Err(e) => {
                    // Fatal error, propagate it
                    return Err(e);
                }
            };

            // nil or empty string means end of input
            if result.is_nil() {
                l.set_top(func_idx)?;
                break;
            }

            // Get raw bytes from either textual or binary Lua strings.
            let bytes_opt = result.as_bytes();

            if let Some(bytes) = bytes_opt {
                if bytes.is_empty() {
                    l.set_top(func_idx)?;
                    break;
                }

                // Check if first byte is binary marker (0x1B for Lua bytecode)
                if first_chunk && !bytes.is_empty() && bytes[0] == 0x1B {
                    is_binary = true;
                }

                // IMPORTANT: Copy the bytes BEFORE calling set_top
                // because set_top may allow GC to run
                accumulated.extend_from_slice(bytes);
                first_chunk = false;

                // Clean up stack
                l.set_top(func_idx)?;
            } else {
                // Reader function returned non-string value (not nil)
                // Return nil + error message like Lua does
                l.set_top(func_idx)?;
                l.push_value(LuaValue::nil())?;
                let err_msg = l.create_string("reader function must return a string")?;
                l.push_value(err_msg)?;
                return Ok(2);
            }
        }

        (accumulated, is_binary)
    } else if let Some(bytes) = chunk_val.as_bytes() {
        let is_binary = bytes.first() == Some(&0x1B);
        (bytes.to_vec(), is_binary)
    } else {
        return Err(l.error("bad argument #1 to 'load' (function or string expected)".to_string()));
    };

    // Optional chunk name for error messages
    // If not provided and chunk is text, use the source code itself (or a prefix)
    let chunkname = if let Some(name) = chunkname_arg {
        name
    } else if let Some(source) = text_source {
        source.to_string()
    } else if !is_binary {
        // For text chunks, use the source code as chunk name (like Lua 5.5)
        // This allows the source to be preserved in the bytecode
        String::from_utf8_lossy(&code_bytes).into_owned()
    } else {
        "=(load)".to_string()
    };

    // Optional mode ("b", "t", or "bt")
    let mode = mode_arg.unwrap_or_else(|| "bt".to_string());

    // Validate mode string - must contain only 'b' and/or 't'
    if mode.is_empty() || mode.chars().any(|c| c != 'b' && c != 't') {
        return Err(crate::stdlib::debug::argerror(l, 3, "invalid mode"));
    }

    // Check if mode allows this chunk type
    if is_binary {
        // Binary chunk - mode must allow binary ("b" or "bt")
        if !mode.contains('b') {
            l.push_value(LuaValue::nil())?;
            let err_msg = l.create_string("attempt to load a binary chunk (mode is 'text')")?;
            l.push_value(err_msg)?;
            return Ok(2);
        }
        if !l.allow_load_bytecode() {
            l.push_value(LuaValue::nil())?;
            let err_msg =
                l.create_string("attempt to load a binary chunk (bytecode loading is disabled)")?;
            l.push_value(err_msg)?;
            return Ok(2);
        }
    } else {
        // Text chunk - mode must allow text ("t" or "bt")
        if !mode.contains('t') {
            l.push_value(LuaValue::nil())?;
            let err_msg = l.create_string("attempt to load a text chunk (mode is 'binary')")?;
            l.push_value(err_msg)?;
            return Ok(2);
        }
    }

    // Optional environment table
    let env = env_arg;

    let chunk_result = if is_binary {
        let vm = l.global_state_mut();
        let result = if code_bytes.len() < 5 || code_bytes.get(4) == Some(&0x53) {
            if vm.language() != crate::LuaLanguageLevel::Lua53 {
                Err("standard Lua 5.3 chunks require a Lua 5.3 state".to_string())
            } else {
                chunk53::load(&code_bytes, vm)
            }
        } else if vm.language() == crate::LuaLanguageLevel::Lua53 {
            Err("binary chunk is not standard Lua 5.3".to_string())
        } else {
            chunk_serializer::deserialize_chunk_with_strings_vm(&code_bytes, vm)
        };
        result.map_err(|error| format!("binary load error: {error}"))
    } else if let Some(source) = text_source {
        l.compile_chunk_with_name(source, &chunkname)
            .map_err(|e| l.get_error_msg(e))
    } else if l.global_state().language() == crate::LuaLanguageLevel::Lua53 {
        l.global_state_mut()
            .compile_bytes_with_name(&code_bytes, &chunkname)
            .map_err(|message| message)
    } else {
        let code_str = match String::from_utf8(code_bytes) {
            Ok(source) => source,
            Err(_) => return Err(l.error("source is not valid UTF-8".to_string())),
        };
        l.compile_chunk_with_name(&code_str, &chunkname)
            .map_err(|error| l.get_error_msg(error))
    };

    match chunk_result {
        Ok(chunk) => {
            // Create upvalues for the function
            // According to Lua documentation:
            // - If chunk has upvalues, the first one should be _ENV (global environment)
            // - Other upvalues are initialized to nil
            let upvalue_count = chunk.upvalue_count;
            let mut upvalues = Vec::with_capacity(upvalue_count);

            for i in 0..upvalue_count {
                if i == 0 {
                    // First upvalue is _ENV
                    let env_upvalue_id = if let Some(env) = env {
                        l.create_upvalue_closed(env)?
                    } else {
                        let global = l.global_state_mut().global;
                        l.create_upvalue_closed(global)?
                    };
                    upvalues.push(env_upvalue_id);
                } else {
                    // Other upvalues are initialized to nil
                    let nil_upvalue = l.create_upvalue_closed(LuaValue::nil())?;
                    upvalues.push(nil_upvalue);
                }
            }

            let func = l
                .global_state_mut()
                .create_loaded_function(chunk, UpvalueStore::from_vec(upvalues))?;
            l.push_value(func)?;
            Ok(1)
        }
        Err(e) => {
            // Return nil and error message
            let err_msg = l.create_string(&e)?;
            l.push_value(LuaValue::nil())?;
            l.push_value(err_msg)?;
            Ok(2)
        }
    }
}

/// loadfile([filename [, mode [, env]]]) - Load a file as a chunk
fn lua_loadfile(l: &mut LuaState) -> LuaResult<usize> {
    let filename = l
        .get_arg(1)
        .ok_or_else(|| l.error("bad argument #1 to 'loadfile' (value expected)".to_string()))?;

    let filename_str = if let Some(s) = filename.as_str() {
        s.to_string()
    } else {
        return Err(l.error("bad argument #1 to 'loadfile' (string expected)".to_string()));
    };

    // Optional mode ("b", "t", or "bt")
    let mode = l
        .get_arg(2)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "bt".to_string());

    // Optional environment table
    let env_arg = l.get_arg(3);

    // Load from specified file as bytes (to handle both text and binary)
    let file_bytes = match std::fs::read(&filename_str) {
        Ok(b) => b,
        Err(e) => {
            let err_msg = l.create_string(&format!("cannot open {}: {}", filename_str, e))?;
            l.push_value(LuaValue::nil())?;
            l.push_value(err_msg)?;
            return Ok(2);
        }
    };

    // Determine content after skipping shebang/BOM for binary detection
    // For text files, the tokenizer handles shebang natively
    let mut skip_offset = 0;

    // Skip initial comment line (shebang) if present
    if file_bytes.first() == Some(&b'#') {
        if let Some(pos) = file_bytes.iter().position(|&b| b == b'\n') {
            skip_offset = pos + 1;
        } else {
            skip_offset = file_bytes.len();
        }
    }

    // Skip UTF-8 BOM if present (after potential shebang skip)
    if file_bytes[skip_offset..].starts_with(&[0xEF, 0xBB, 0xBF]) {
        skip_offset += 3;
    }

    // Check if it's a binary chunk (starts with 0x1B after shebang/BOM)
    let is_binary = file_bytes.get(skip_offset) == Some(&0x1B);

    // Validate mode
    if !mode.is_empty() && mode.chars().all(|c| c == 'b' || c == 't') {
        if is_binary && !mode.contains('b') {
            let err_msg = l.create_string("attempt to load a binary chunk (mode is 'text')")?;
            l.push_value(LuaValue::nil())?;
            l.push_value(err_msg)?;
            return Ok(2);
        }
        if !is_binary && !mode.contains('t') {
            let err_msg = l.create_string("attempt to load a text chunk (mode is 'binary')")?;
            l.push_value(LuaValue::nil())?;
            l.push_value(err_msg)?;
            return Ok(2);
        }
    }

    if is_binary && !l.allow_load_bytecode() {
        let err_msg =
            l.create_string("attempt to load a binary chunk (bytecode loading is disabled)")?;
        l.push_value(LuaValue::nil())?;
        l.push_value(err_msg)?;
        return Ok(2);
    }

    match l.load_proto_from_file(&filename_str) {
        Ok(proto) => {
            let upvalue_count = proto.as_ref().data.upvalue_count;
            let mut upvalues = Vec::with_capacity(upvalue_count);

            for i in 0..upvalue_count {
                if i == 0 {
                    let env_upvalue_id = if let Some(env) = env_arg {
                        l.create_upvalue_closed(env)?
                    } else {
                        let global = l.global_state_mut().global;
                        l.create_upvalue_closed(global)?
                    };
                    upvalues.push(env_upvalue_id);
                } else {
                    let nil_upvalue = l.create_upvalue_closed(LuaValue::nil())?;
                    upvalues.push(nil_upvalue);
                }
            }

            let func = l
                .global_state_mut()
                .create_function(proto, UpvalueStore::from_vec(upvalues))?;
            l.push_value(func)?;
            Ok(1)
        }
        Err(error) => {
            let message = l.get_error_message(error);
            let err_msg = l.create_string(&message)?;
            l.push_value(LuaValue::nil())?;
            l.push_value(err_msg)?;
            Ok(2)
        }
    }
}

/// dofile([filename]) - Execute a file
fn lua_dofile(l: &mut LuaState) -> LuaResult<usize> {
    let arg1 = l.get_arg(1);

    // Get filename (nil/none means stdin, which we don't support yet)
    let filename_str = if let Some(v) = arg1 {
        if v.is_nil() {
            return Err(l.error("dofile: reading from stdin not yet implemented".to_string()));
        }
        if let Some(s) = v.as_str() {
            s.to_string()
        } else {
            return Err(l.error("bad argument #1 to 'dofile' (string expected)".to_string()));
        }
    } else {
        return Err(l.error("dofile: reading from stdin not yet implemented".to_string()));
    };

    let proto = l.load_proto_from_file(&filename_str)?;
    let global = l.global_state_mut().global;
    // Create function with _ENV upvalue (global table)
    let env_upvalue = l.create_upvalue_closed(global)?;
    let func = l
        .global_state_mut()
        .create_function(proto, UpvalueStore::from_single(env_upvalue))?;

    // Use call_stack_based which supports yields (equivalent to lua_callk in C Lua).
    // C Lua does lua_settop(L, 1) to keep only the filename, then pushes the chunk at slot 2.
    // After yield, dofilecont returns gettop-1 (skipping the filename).
    // Our finish_c_frame/CIST_YCALL uses pcall_func_pos+1 as result start,
    // so we must clear all args and place the chunk right after the C function slot.
    let func_pos = l
        .current_frame()
        .map(|f| f.base - f.func_offset as usize)
        .unwrap_or(0);
    let clear_to = func_pos + 1; // right after the dofile function slot
    l.set_top(clear_to)?;
    l.push_value(func)?;
    let func_idx = clear_to; // chunk is at this position
    let num_results = l.call_stack_based(func_idx, 0)?;

    Ok(num_results)
}

/// warn(msg1, ...) - Emit a warning (Lua 5.5 semantics)
///
/// Control messages: single argument starting with '@':
///   @on    - enable warnings (stderr mode)
///   @off   - disable warnings
///   @store - store warnings in _WARN global
///   @normal - restore normal stderr output
///   other  - ignored
///
/// Regular messages: concatenate all arguments; output according to state.
/// Warnings are OFF by default.
fn lua_warn(l: &mut LuaState) -> LuaResult<usize> {
    let args = l.get_args();

    // At least one argument required, all must be strings
    if args.is_empty() {
        return Err(
            l.error("bad argument #1 to 'warn' (string expected, got no value)".to_string())
        );
    }
    let mut parts: Vec<String> = Vec::with_capacity(args.len());
    for (i, arg) in args.iter().enumerate() {
        if let Some(s) = arg.as_str() {
            parts.push(s.to_string());
        } else {
            return Err(l.error(format!(
                "bad argument #{} to 'warn' (string expected, got {})",
                i + 1,
                arg.type_name()
            )));
        }
    }

    // Get current warn mode from registry ("off", "on", "store")
    let registry = l.global_state_mut().registry;
    let mode_key = l.create_string("_WARN_MODE")?;
    let current_mode = l
        .raw_get(&registry, &mode_key)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "off".to_string());

    // Check for control message: single argument starting with '@'
    if parts.len() == 1 && parts[0].starts_with('@') {
        let control = &parts[0][1..];
        match control {
            "on" => {
                let mode_val = l.create_string("on")?;
                l.raw_set(&registry, mode_key, mode_val);
            }
            "off" => {
                let mode_val = l.create_string("off")?;
                l.raw_set(&registry, mode_key, mode_val);
            }
            "store" => {
                let mode_val = l.create_string("store")?;
                l.raw_set(&registry, mode_key, mode_val);
            }
            "normal" => {
                let mode_val = l.create_string("on")?;
                l.raw_set(&registry, mode_key, mode_val);
            }
            _ => {
                // Unknown control message, ignored
            }
        }
        return Ok(0);
    }

    // Regular message: concatenate all parts (no separator)
    let message: String = parts.concat();

    match current_mode.as_str() {
        "on" => {
            eprintln!("Lua warning: {}", message);
        }
        "store" => {
            // Store in _WARN global
            let warn_val = l.create_string(&message)?;
            l.global_state_mut().set_global("_WARN", warn_val)?;
        }
        _ => {
            // "off" - do nothing
        }
    }

    Ok(0)
}
