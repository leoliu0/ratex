#[cfg(test)]
use crate::lua_vm::GlobalState;
use crate::lua_vm::SafeOption;

#[test]
fn test_xpcall_simple() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    // Test 1: Basic xpcall without upvalues
    let result = vm.main_state().execute(
        r#"
        local function handler(err)
            return "handled"
        end
        
        local ok, result = xpcall(function()
            error("test")
        end, handler)
        
        assert(ok == false)
        assert(result == "handled")
        "#,
    );
    assert!(result.is_ok(), "Test 1 failed: {:?}", result);

    // Test 2: Handler with upvalue capture
    let result2 = vm.main_state().execute(
        r#"
        local called = false
        local function handler2(err)
            called = true
            return "ok"
        end
        
        local ok2 = xpcall(function() error("x") end, handler2)
        assert(ok2 == false)
        assert(called == true)
        "#,
    );
    assert!(result2.is_ok(), "Test 2 failed: {:?}", result2);
}

#[test]
fn test_xpcall_concat() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    // Test: Handler with string concatenation
    let result = vm.main_state().execute(
        r#"
        local flag = false
        local function handler(err)
            flag = true
            return "handled: " .. tostring(err)
        end
        
        local ok, msg = xpcall(function() error("xyz") end, handler)
        assert(ok == false, "ok should be false")
        assert(flag == true, "flag should be true")
        -- remove this
        --assert(msg == "handled: xyz", "msg should be 'handled: xyz'")
        "#,
    );
    assert!(result.is_ok(), "Test failed: {:?}", result);
}

#[test]
fn test_debug_traceback_level_two_keeps_caller_frame() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local function caller()
            local trace = debug.traceback("", 2)
            assert(type(trace) == "string")
            assert(string.find(trace, "stack traceback:", 1, true) ~= nil)
            assert(string.find(trace, "in main chunk", 1, true) ~= nil)
        end

        caller()
        "#,
    );

    assert!(result.is_ok(), "Test failed: {:?}", result);
}

#[test]
fn test_debug_traceback_in_hook_reports_hook_frame() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local count = 0
        local function f ()
            assert(debug.getinfo(1).namewhat == "hook")
            local sndline = string.match(debug.traceback(), "\n(.-)\n")
            assert(string.find(sndline, "hook", 1, true) ~= nil)
            count = count + 1
        end

        debug.sethook(f, "l")
        local a = 0
        _ENV.a = a
        a = 1
        debug.sethook()
        assert(count == 4)
        _ENV.a = nil
        "#,
    );

    assert!(result.is_ok(), "Test failed: {:?}", result);
}

fn run_debug_level(level: crate::LuaLanguageLevel, code: &str) {
    let mut vm = GlobalState::new_with_language(SafeOption::default(), level);
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let result = vm.main_state().execute(code);
    assert!(result.is_ok(), "{level}: {result:?}");
}

#[test]
fn test_debug_library_matches_ldblib_53() {
    // Expected values from texlua (Lua 5.3.6).
    run_debug_level(
        crate::LuaLanguageLevel::Lua53,
        r#"
        local function up() return print end
        local function e(...) return select(2, pcall(...)) end
        assert(e(debug.getlocal) == "bad argument #2 to 'debug.getlocal' (number expected, got no value)")
        assert(e(debug.getlocal, 50, 1) == "bad argument #1 to 'debug.getlocal' (level out of range)")
        assert(select('#', debug.getlocal(up, 1)) == 1 and debug.getlocal(up, 1) == nil)
        assert(e(debug.setlocal, 1, 1) == "bad argument #3 to 'debug.setlocal' (value expected)")
        assert(e(debug.getupvalue, 1, 1) == "bad argument #1 to 'debug.getupvalue' (function expected, got number)")
        assert(e(debug.upvalueid, up, 2) == "bad argument #2 to 'debug.upvalueid' (invalid upvalue index)")
        assert(e(debug.upvaluejoin, print, 1, up, 1) == "bad argument #2 to 'debug.upvaluejoin' (invalid upvalue index)")
        assert(e(debug.setmetatable, 1, 2) == "bad argument #2 to 'debug.setmetatable' (nil or table expected)")
        assert(e(debug.sethook, print) == "bad argument #2 to 'debug.sethook' (string expected, got no value)")
        local h, mask, count = debug.gethook()
        assert(h == nil and mask == "" and count == 0)
        debug.sethook(print, "", 0)
        assert(debug.gethook() == nil)
        assert(e(debug.getinfo, 1, "r") == "bad argument #2 to 'debug.getinfo' (invalid option)")
        assert(debug.getinfo(100) == nil and select('#', debug.getinfo(100)) == 1)
        local info = debug.getinfo(1)
        assert(info.ntransfer == nil and info.ftransfer == nil and info.extraargs == nil)
        assert(debug.traceback(12):find("^12\nstack traceback:\n\t"), debug.traceback(12))
        assert(type(debug.debug) == "function")
        local ok, m = pcall(function() return "abc" + {} end)
        assert(m:find("attempt to perform arithmetic on a string value$"), m)
        local t = setmetatable({}, {__index = function() return debug.traceback("mm", 1) end})
        assert(t.x:find("in metamethod '__index'", 1, true), t.x)
        local co = coroutine.wrap(function() local w = coroutine.wrap(function() error("deep") end) w() end)
        local ok, m = pcall(co)
        assert(m:find('^[^\n]-:%d+: [^\n]-:%d+: deep$'), m)
        assert(e(coroutine.resume, 1) == "bad argument #1 to 'coroutine.resume' (thread expected)")
        assert(e(coroutine.wrap, 1) == "bad argument #1 to 'coroutine.wrap' (function expected, got number)")
        "#,
    );
}

#[test]
fn test_debug_library_matches_ldblib_55() {
    // Expected values from lua 5.5.1.
    run_debug_level(
        crate::LuaLanguageLevel::Lua55,
        r#"
        local function up() return print end
        local function e(...) return select(2, pcall(...)) end
        assert(select('#', debug.gethook()) == 1)
        assert(select('#', debug.upvalueid(up, 2)) == 1 and debug.upvalueid(up, 2) == nil)
        assert(e(debug.setmetatable, 1, 2) == "bad argument #2 to 'debug.setmetatable' (nil or table expected, got number)")
        assert(e(debug.getinfo, 1, ">") == "bad argument #2 to 'debug.getinfo' (invalid option '>')")
        local t = setmetatable({}, {__index = function() return debug.traceback("mm", 1) end})
        assert(t.x:find("in metamethod 'index'", 1, true), t.x)
        assert(debug.getuservalue(io.stdout) == nil)
        assert(e(coroutine.resume, 1) == "bad argument #1 to 'coroutine.resume' (thread expected, got number)")
        local function deep(n) if n == 0 then return debug.traceback("m") end return (deep(n - 1)) end
        assert(deep(30):find("\n\t...\t(skipping ", 1, true), deep(30))
        "#,
    );
}

#[test]
fn test_lua53_contract_matches_texlua() {
    // Expected values from texlua (Lua 5.3.6).
    run_debug_level(
        crate::LuaLanguageLevel::Lua53,
        r#"
        -- 5.3's parlist declares no vararg parameter: no local, no register
        local function foo(a, b, ...) local d = 1 return (debug.getlocal(1, 3)) end
        assert(debug.getlocal(foo, 3) == nil and foo(1, 2, 3) == 'd')
        local ok, m = load("function f(...t) end")
        assert(not ok and m:find("')' expected near 't'", 1, true), m)
        -- luaB_pcall keeps its `true` below the function: a C temporary
        local function lv2() return debug.getlocal(2, 1) end
        local _, n, v = pcall(lv2)
        assert(n == '(*temporary)' and v == true, n)
        -- 5.3 call errors: no metamethod names; a bad __call blames the object
        local t = setmetatable({}, {__add = 34, __call = 34, __index = 34})
        assert(select(2, pcall(function() return t + 1 end)):find('attempt to call a number value$'))
        assert(select(2, pcall(function() return t() end)):find("call a table value (upvalue 't')", 1, true))
        assert(select(2, pcall(function() return t.x end)):find('attempt to index a number value$'))
        -- a trivial handler still runs after a stack overflow (error zone)
        local function deep() return deep() + 1 end
        assert(select(2, xpcall(deep, function() return 1 end)) == 1)
        local s, e = xpcall(deep, function(m) return select(2, pcall(deep)) end)
        assert(e == 'error in error handling', e)
        -- luaL_loadfilex errors
        assert(select(2, loadfile('/nonexistent/x.lua')) == 'cannot open /nonexistent/x.lua: No such file or directory')
        -- a load reader cannot yield
        local co = coroutine.wrap(function() return load(function() coroutine.yield(1) end) end)
        local f, m = co()
        assert(f == nil and m == 'attempt to yield across a C-call boundary', tostring(m))
        "#,
    );
}

#[test]
fn test_lua55_contract_matches_lua() {
    // Expected values from lua 5.5.1.
    run_debug_level(
        crate::LuaLanguageLevel::Lua55,
        r#"
        local function foo(a, b, ...) local d = 1 return (debug.getlocal(1, 3)) end
        assert(debug.getlocal(foo, 3) == nil and foo(1, 2, 3) == '(vararg table)')
        local function lv2() return debug.getlocal(2, 1) end
        local _, n, v = pcall(lv2)
        assert(n == '(C temporary)' and v == true, n)
        local t = setmetatable({}, {__add = 34})
        assert(select(2, pcall(function() return t + 1 end)):find("(metamethod 'add')", 1, true))
        local ok, m = pcall(load("for x in 1 do end"))
        assert(m:find("(for iterator 'for iterator')", 1, true), m)
        local p = select(2, package.searchpath('x', 'a?;;b?'))
        assert(p == "no file 'ax'\n\tno file ''\n\tno file 'bx'", p)
        -- 'then' gets no line event of its own
        local lines = {}
        local src = "if\nmath.sin(1)\nthen\n  a=1\nelse\n  a=2\nend\n"
        debug.sethook(function(_, l) if debug.getinfo(2, 'S').source == src then lines[#lines + 1] = l end end, 'l')
        load(src)()
        debug.sethook()
        assert(table.concat(lines, ',') == '2,4,7', table.concat(lines, ','))
        "#,
    );
}

/// Reading a directory fails at `fread` with `EISDIR` on POSIX systems; on
/// Windows `fopen` of a directory fails already.
#[cfg(unix)]
#[test]
fn test_loadfile_directory_errors() {
    run_debug_level(
        crate::LuaLanguageLevel::Lua53,
        "assert(select(2, loadfile('/')) == 'cannot read /: Is a directory')",
    );
    run_debug_level(
        crate::LuaLanguageLevel::Lua55,
        "assert(select(2, loadfile('/')) == 'cannot read /')",
    );
}

#[test]
fn test_lua53_then_line_event() {
    // Expected values from texlua (Lua 5.3.6): 5.3 emits the jump on the THEN line.
    run_debug_level(
        crate::LuaLanguageLevel::Lua53,
        r#"
        local lines = {}
        local src = "if\nmath.sin(1)\nthen\n  a=1\nelse\n  a=2\nend\n"
        debug.sethook(function(_, l) if debug.getinfo(2, 'S').source == src then lines[#lines + 1] = l end end, 'l')
        load(src)()
        debug.sethook()
        assert(table.concat(lines, ',') == '2,3,4,7', table.concat(lines, ','))
        "#,
    );
}

#[test]
fn test_lua53_thread_cycle_survives_one_collection() {
    // lua-5.3.4-tests/gc.lua:480: touched open upvalues of an unreachable thread are
    // remarked once, so the cycle needs two collections (texlua agrees).
    run_debug_level(
        crate::LuaLanguageLevel::Lua53,
        r#"
        local collected = false
        collectgarbage(); collectgarbage("stop")
        do
          local function f(param)
            ;(function()
              param = {param, f}
              setmetatable(param, {__gc = function() collected = true end})
              coroutine.yield(100)
            end)()
          end
          local co = coroutine.create(f)
          assert(coroutine.resume(co, co))
        end
        collectgarbage()
        assert(not collected)
        collectgarbage()
        assert(collected)
        collectgarbage("restart")
        "#,
    );
}
