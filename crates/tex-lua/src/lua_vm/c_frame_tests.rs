//! Stack discipline of C frames seen through the C API: metamethods called
//! from C, and continuations of `lua_yieldk` / `lua_callk` / `lua_pcallk`.
//! Expected values come from a C program linked against Lua 5.3.6.

#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::{CStr, c_char, c_int};
use std::ptr;

use crate::c_api::*;

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn lua_len(state: *mut lua_State, index: c_int);
    fn lua_setglobal(state: *mut lua_State, name: *const c_char);
    fn lua_yieldk(
        state: *mut lua_State,
        nresults: c_int,
        context: lua_KContext,
        continuation: lua_KFunction,
    ) -> c_int;
    fn lua_callk(
        state: *mut lua_State,
        nargs: c_int,
        nresults: c_int,
        context: lua_KContext,
        continuation: lua_KFunction,
    );
    fn lua_pcallk(
        state: *mut lua_State,
        nargs: c_int,
        nresults: c_int,
        error_function: c_int,
        context: lua_KContext,
        continuation: lua_KFunction,
    ) -> c_int;
}

/// Run `source` in a fresh C-API state with `functions` registered as globals.
fn run(functions: &[(&CStr, unsafe extern "C" fn(*mut lua_State) -> c_int)], source: &CStr) {
    unsafe {
        let state = luaL_newstate();
        luaL_openlibs(state);
        for (name, function) in functions {
            lua_pushcclosure(state, Some(*function), 0);
            lua_setglobal(state, name.as_ptr());
        }
        assert_eq!(luaL_loadstring(state, source.as_ptr()), LUA_OK);
        let status = lua_pcallk(state, 0, 0, 0, 0, None);
        let message = if status == LUA_OK {
            None
        } else {
            let text = lua_tolstring(state, -1, ptr::null_mut());
            Some((!text.is_null()).then(|| CStr::from_ptr(text).to_string_lossy().into_owned()))
        };
        lua_close(state);
        assert_eq!(message, None);
    }
}

unsafe extern "C" fn len_after_pushes(state: *mut lua_State) -> c_int {
    lua_pushinteger(state, 42);
    lua_pushinteger(state, 0);
    lua_len(state, 1);
    lua_gettop(state)
}

#[test]
fn metamethod_called_from_c_keeps_the_values_the_c_function_pushed() {
    run(
        &[(c"len_after_pushes", len_after_pushes)],
        c"local t = setmetatable({}, {__len = function() return 7 end})
          local n = select('#', len_after_pushes(t))
          local a, b, c, d = len_after_pushes(t)
          assert(n == 4 and a == t and b == 42 and c == 0 and d == 7, tostring(a))
          -- a metamethod with a larger frame than the C function's arguments
          t = setmetatable({}, {__len = function()
              local a, b, c, d, e, f, g, h = 1, 2, 3, 4, 5, 6, 7, 8
              return a + b + c + d + e + f + g + h
          end})
          a, b, c, d = len_after_pushes(t)
          assert(a == t and b == 42 and c == 0 and d == 36, tostring(a))",
    );
}

unsafe extern "C" fn push_stack_and_status(
    state: *mut lua_State,
    status: c_int,
    _context: lua_KContext,
) -> c_int {
    lua_pushinteger(state, status as lua_Integer);
    lua_gettop(state)
}

unsafe extern "C" fn yield_with_continuation(state: *mut lua_State) -> c_int {
    lua_pushstring(state, c"kept".as_ptr());
    lua_pushinteger(state, 99);
    lua_yieldk(state, 1, 0, Some(push_stack_and_status))
}

#[test]
fn yieldk_continuation_sees_its_arguments_and_the_resume_values() {
    run(
        &[(c"yield_with_continuation", yield_with_continuation)],
        c"local co = coroutine.wrap(yield_with_continuation)
          assert(select('#', co('a', 'b')) == 1)
          local r = table.pack(co('c', 'd'))
          assert(table.concat(r, ',', 1, r.n) == 'a,b,kept,c,d,1', table.concat(r, ',', 1, r.n))",
    );
}

unsafe extern "C" fn callk_one_result(state: *mut lua_State) -> c_int {
    lua_pushstring(state, c"mark".as_ptr());
    lua_pushvalue(state, 1);
    lua_callk(state, 0, 1, 0, Some(push_stack_and_status));
    push_stack_and_status(state, LUA_OK, 0)
}

unsafe extern "C" fn pcallk_two_results(state: *mut lua_State) -> c_int {
    lua_pushstring(state, c"mark".as_ptr());
    lua_pushvalue(state, 1);
    let status = lua_pcallk(state, 0, 2, 0, 0, Some(push_stack_and_status));
    push_stack_and_status(state, status, 0)
}

#[test]
fn callk_continuation_adjusts_the_callee_results_after_a_yield() {
    run(
        &[(c"callk_one_result", callk_one_result), (c"pcallk_two_results", pcallk_two_results)],
        c"local function f() coroutine.yield('y') return 'v', 'r1', 'r2' end
          local function show(...) return table.concat(table.pack(...), ',', 2, select('#', ...)) end
          for _, case in ipairs{{callk_one_result, 'mark,v,1'}, {pcallk_two_results, 'mark,v,r1,1'}} do
              local co = coroutine.wrap(case[1])
              assert(co(f) == 'y')
              local got = show(co())
              assert(got == case[2], got)
          end
          local co = coroutine.wrap(pcallk_two_results)
          assert(co(function() coroutine.yield('y') error('boom', 0) end) == 'y')
          local got = show(co())
          assert(got == 'mark,boom,2', got)",
    );
}

#[test]
fn pcallk_reports_a_failing_message_handler_as_errerr() {
    unsafe {
        let state = luaL_newstate();
        luaL_openlibs(state);
        let source = c"return function(m) error('h') end, function() error('x') end";
        assert_eq!(luaL_loadstring(state, source.as_ptr()), LUA_OK);
        assert_eq!(lua_pcallk(state, 0, 2, 0, 0, None), LUA_OK);
        let status = lua_pcallk(state, 0, 0, 1, 0, None);
        let message = CStr::from_ptr(lua_tolstring(state, -1, ptr::null_mut()));
        assert_eq!((status, message.to_bytes()), (LUA_ERRERR, &b"error in error handling"[..]));
        assert_eq!(lua_gettop(state), 2);
        lua_close(state);
    }
}
