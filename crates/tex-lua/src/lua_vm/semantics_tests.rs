//! Regression tests for VM semantics, checked against the reference
//! interpreters (`lua` 5.5 and LuaTeX's `texlua` 5.3).

use crate::{GlobalState, LuaLanguageLevel, SafeOption, Stdlib};

/// Run `source` under `language`; panic with the Lua error message on failure.
fn run(language: LuaLanguageLevel, source: &str) {
    let mut vm = GlobalState::new_with_language(SafeOption::default(), language);
    vm.open_stdlib(Stdlib::All).unwrap();
    let state = vm.main_state();
    if let Err(error) = state.execute(source) {
        let full = state.get_full_error(error);
        panic!("{:?}: {}", language, full.message());
    }
}

fn run_both(source: &str) {
    run(LuaLanguageLevel::Lua53, source);
    run(LuaLanguageLevel::Lua55, source);
}

#[test]
fn deep_non_tail_recursion_is_bounded_by_stack_size_not_frame_count() {
    run_both(
        r#"
        local function depth(n) if n == 0 then return 0 end return 1 + depth(n - 1) end
        assert(depth(150000) == 150000)
        local ok, msg = pcall(depth, 1e7)
        assert(not ok and msg:find("stack overflow", 1, true), msg)
        assert(depth(10) == 10)  -- the state stays usable after the overflow
        "#,
    );
}

#[test]
fn lua55_string_arithmetic_keeps_numeral_subtype() {
    run(
        LuaLanguageLevel::Lua55,
        r#"
        assert(math.type("10" + 1) == "integer" and "10" + 1 == 11)
        assert(math.type("10.0" + 1) == "float")
        assert(math.type(5.0 + "1") == "float")
        assert(math.type("1e1" // 1) == "float")
        assert(math.type(-"2") == "integer" and math.type(-"2.0") == "float")
        assert(math.type(2.0 * "3") == "float" and 2.0 * "3" == 6)
        local ok, msg = pcall(function() return "5" % 0 end)
        assert(not ok and msg:find("attempt to perform 'n%%0'"), msg)
        ok, msg = pcall(function() return "5" // 0 end)
        assert(not ok and msg:find("attempt to divide by zero"), msg)
        assert("5" % 0.0 ~= "5" % 0.0)  -- float modulo by zero is NaN
        ok, msg = pcall(function() return -"abc" end)
        assert(not ok and msg:find("attempt to unm a 'string' with a 'string'", 1, true), msg)
        ok, msg = pcall(function() return "abc" + 1 end)
        assert(not ok and msg:find("^.-:%d+: attempt to add a 'string' with a 'number'"), msg)
        "#,
    );
}

#[test]
fn lua53_string_arithmetic_is_float_and_bypasses_metatables() {
    run(
        LuaLanguageLevel::Lua53,
        r#"
        assert(getmetatable("").__add == nil and getmetatable("").__unm == nil)
        assert(math.type("10" + 1) == "float" and "10" + 1 == 11)
        assert(math.type(-"2") == "float" and -"2" == -2)
        assert(math.type("7" % "2") == "float" and math.type("10" // "3") == "float")
        assert("5" // 0 == math.huge)
        assert(math.type("-9223372036854775808" // "-1") == "float")
        assert(math.type("3" | 0) == "integer")
        local mt = {__add = function() return "mt" end}
        assert("10" + setmetatable({}, mt) == "mt")
        local ok, msg = pcall(function() return "abc" + 1 end)
        assert(not ok and msg:find("^.-:%d+: attempt to perform arithmetic on a string value"), msg)
        "#,
    );
}

#[test]
fn table_assignment_rejects_nan_and_nil_keys_after_newindex() {
    run_both(
        r#"
        local ok, msg = pcall(function() local t, k = {}, 0/0 t[k] = 1 end)
        assert(not ok and msg:find("table index is NaN"), msg)
        assert(next({}) == nil)
        ok, msg = pcall(function() local t, k = {}, nil t[k] = 1 end)
        assert(not ok and msg:find("table index is nil"), msg)
        local seen = {}
        local t = setmetatable({}, {__newindex = function(_, k, v) seen[#seen + 1] = tostring(k) end})
        t[nil] = 1
        t[0/0] = 2
        assert(#seen == 2 and seen[1] == "nil", table.concat(seen, ","))
        "#,
    );
}

#[test]
fn lua53_integer_for_loop_skips_when_start_is_past_limit() {
    run(
        LuaLanguageLevel::Lua53,
        r#"
        local function count(a, b, c)
          local n = 0
          for _ = a, b, c do n = n + 1 if n > 10 then break end end
          return n
        end
        local maxi, mini = math.maxinteger, math.mininteger
        assert(count(maxi, 1, 1) == 0)
        assert(count(1, -1, maxi) == 0)
        assert(count(mini, -1, -1) == 0)
        assert(count(-1, 1, mini) == 0)
        assert(count(1, 3, 1) == 3 and count(3, 1, -1) == 3)
        assert(count(1, -1e300, 1) == 0 and count(1, 1e300, -1) == 0)
        assert(count(1, 2.5, 1) == 2 and count(3, 1.5, -1) == 2)
        "#,
    );
}

#[test]
fn power_uses_libm_pow() {
    run_both(
        r#"
        local ten, e = 10, 23
        assert(ten ^ e == 1.0000000000000001e23)
        local three = 3
        assert(three ^ 40 == 12157665459056928801.0)
        assert(three ^ 255 == 4.633615079238158e121)
        assert(2 ^ 0.5 == math.sqrt(2))
        "#,
    );
}

#[test]
fn bitwise_error_names_the_operand_before_the_reason() {
    run_both(
        r#"
        local a, b = 1, 1.5
        local ok, msg = pcall(function() return a & b end)
        assert(not ok and msg:find("number (upvalue 'b') has no integer representation", 1, true), msg)
        "#,
    );
}

#[test]
fn gc_keeps_open_upvalues_of_threads_reached_late_in_atomic() {
    // Generational minor collections reach the main thread only through
    // `grayagain`, after `remark_upvalues`; the coroutine must not be treated
    // as dead there (its open upvalue was closed early, losing `x = {123}`).
    run(
        LuaLanguageLevel::Lua55,
        r#"
        collectgarbage("generational")
        local co = coroutine.create(function ()
          local x = nil
          local f = function () return x[1] end
          x = coroutine.yield(f)
          coroutine.yield()
        end)
        local _, f = coroutine.resume(co)
        collectgarbage("step")
        coroutine.resume(co, {123})
        co = nil
        collectgarbage("step")
        assert(f() == 123)
        collectgarbage("incremental")
        "#,
    );
}

#[test]
fn gc_collects_values_held_only_by_dead_threads_open_upvalues() {
    // Lua 5.5 semantics (lua-5.5.0-tests gc.lua); 5.3 keeps them one more cycle.
    run(
        LuaLanguageLevel::Lua55,
        r#"
        local collected = false
        collectgarbage(); collectgarbage("stop")
        do
          local co = coroutine.create(function (param)
            ;(function ()
              param = setmetatable({}, {__gc = function () collected = true end})
              coroutine.yield(100)
            end)()
          end)
          assert(coroutine.resume(co, 1))
        end
        collectgarbage()
        assert(collected)
        collectgarbage("restart")
        "#,
    );
}

#[test]
fn lua53_float_modulo_uses_the_5_3_sign_correction() {
    // 5.3 corrects fmod's result only when m*b < 0, which is false when the
    // product underflows; 5.5 compares signs.
    run(
        LuaLanguageLevel::Lua53,
        r#"
        local a, b = 1, -1e-300
        local m = a % b
        assert(m > 0 and m == math.fmod(a, b))
        assert(5.5 % -2 == -0.5 and -5.5 % 2 == 0.5)
        "#,
    );
    run(
        LuaLanguageLevel::Lua55,
        r#"
        local a, b = 1, -1e-300
        assert(a % b < 0)
        "#,
    );
}

#[test]
fn lua53_arithmetic_error_blames_the_non_numeric_operand() {
    run(
        LuaLanguageLevel::Lua53,
        r#"
        local ok, msg = pcall(load("aaa = '2'; b = nil; x = aaa * b"))
        assert(not ok and msg:find("global 'b'", 1, true), msg)
        "#,
    );
}

#[test]
fn lua55_nil_error_object_becomes_no_error_object_after_the_handler() {
    run(
        LuaLanguageLevel::Lua55,
        r#"
        local ok, e = pcall(error); assert(not ok and e == "<no error object>", e)
        ok, e = xpcall(error, function(m) return type(m) end); assert(e == "nil")
        ok, e = xpcall(error, function(m) return m end); assert(e == "<no error object>")
        ok, e = coroutine.resume(coroutine.create(function() error() end))
        assert(e == "<no error object>")
        local seen
        ok, e = pcall(function()
            local x <close> = setmetatable({}, {__close = function(_, err) seen = err end})
            error()
        end)
        assert(seen == "<no error object>" and e == "<no error object>", tostring(seen))
        "#,
    );
    run(LuaLanguageLevel::Lua53, "local ok, e = pcall(error); assert(not ok and e == nil)");
}

#[test]
fn close_errors_reach_the_message_handler_before_unwinding() {
    run(
        LuaLanguageLevel::Lua55,
        r#"
        local function close(f) return setmetatable({}, {__close = f}) end
        -- the handler sees the frames of the failing __close
        local function foo() local x <close> = close(function() error("@x") end) end
        local ok, msg = xpcall(foo, debug.traceback)
        assert(msg:find("^[^\n]*@x") and msg:find("in metamethod"), msg)
        -- while unwinding, each close gets the handled error and its own
        -- error goes through the handler again
        local log = {}
        local function bar()
            local a <close> = close(function(_, e) log[#log + 1] = "a:" .. e end)
            local b <close> = close(function(_, e) log[#log + 1] = "b:" .. e; error("@b", 0) end)
            error("orig", 0)
        end
        ok, msg = xpcall(bar, function(m) return "H:" .. m end)
        assert(msg == "H:@b" and table.concat(log, ",") == "b:H:orig,a:H:@b", table.concat(log, ","))
        -- an error in a __close on normal exit stops there; pcall closes the rest
        log = {}
        local function baz()
            local a <close> = close(function(_, e) log[#log + 1] = "a:" .. tostring(e) end)
            local b <close> = close(function(_, e)
                log[#log + 1] = "b:" .. tostring(e)
                error("@b", 0)
            end)
        end
        ok, msg = pcall(baz)
        assert(msg == "@b" and table.concat(log, ",") == "b:nil,a:@b", table.concat(log, ","))
        "#,
    );
}

#[test]
fn c_stack_overflow_names_the_calling_line() {
    // 200 nested metamethod calls need more native stack than a test thread
    // has in debug builds.
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(|| {
            run_both(
                r#"
                local mt = {}
                mt.__index = function(t, k) if k == 0 then return 0 end return t[k - 1] + 1 end
                local t = setmetatable({}, mt)
                local ok, msg = pcall(function() return t[100000] end)
                assert(not ok and msg:find(":%d+: C stack overflow$"), msg)
                "#,
            )
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn concat_error_names_the_operand_that_is_not_a_string() {
    run_both(
        r#"
        local function msg(f) local ok, m = pcall(f) assert(not ok) return m end
        local m = msg(function() local a = "x" local b = {} return a .. b end)
        assert(m:find("table value (local 'b')", 1, true), m)
        m = msg(function() local a = "x" local t = {} return a .. t.f .. "y" end)
        assert(m:find("nil value (field 'f')", 1, true), m)
        m = msg(function() local t = {} return "a" .. t.x .. "b" end)
        assert(m:find("nil value (field 'x')", 1, true), m)
        m = msg(function() return "abc" .. {} end)
        assert(m:find("concatenate a table value$"), m)
        "#,
    );
}

#[test]
fn xpcall_keeps_its_frame_and_survives_yields() {
    run_both(
        r#"
        local _, tb = xpcall(function() error("x") end, debug.traceback)
        assert(tb:find("'xpcall'") and not tb:find("traceback'"), tb)
        local co = coroutine.wrap(function()
            return xpcall(function(a) local b = coroutine.yield(a) return b, "z" end,
                debug.traceback, "y")
        end)
        assert(co() == "y")
        local ok, b, z = co("w")
        assert(ok == true and b == "w" and z == "z")
        co = coroutine.wrap(function()
            return xpcall(function() coroutine.yield(1) error("e", 0) end,
                function(m) return "H" .. m end)
        end)
        co()
        local ok2, m = co()
        assert(ok2 == false and m == "He", tostring(m))
        "#,
    );
}

#[test]
fn resuming_after_a_yield_fires_the_return_hook_of_yield() {
    run_both(
        r#"
        local co = coroutine.create(function()
            coroutine.yield(10)
            return 20
        end)
        local trace = {}
        debug.sethook(co, function(e) trace[#trace + 1] = e end, "clr")
        repeat until not coroutine.resume(co)
        local got = table.concat(trace, " ")
        assert(got == "call line call return line return", got)
        "#,
    );
}

#[test]
fn functions_called_from_library_code_cannot_yield() {
    run_both(
        r#"
        local co = coroutine.wrap(function()
            local inside
            string.gsub("a", ".", function() inside = coroutine.isyieldable() end)
            return inside
        end)
        assert(co() == false)
        "#,
    );
}

#[test]
fn loadfile_names_the_chunk_by_the_path_as_given() {
    let dir = std::env::temp_dir();
    let file = dir.join(format!("tex_lua_chunkname_{}.lua", std::process::id()));
    std::fs::write(&file, "error('boom')").unwrap();
    // A path with a "." component differs from its canonical form.
    let given = format!("{}/./{}", dir.display(), file.file_name().unwrap().to_string_lossy());
    let source = format!(
        r#"
        local path = {given:?}
        local f = assert(loadfile(path))
        assert(debug.getinfo(f, "S").source == "@" .. path, debug.getinfo(f, "S").source)
        local ok, msg = pcall(f)
        assert(msg == path .. ":1: boom", msg)
        "#
    );
    run_both(&source);
    std::fs::remove_file(&file).unwrap();
}

#[test]
fn lua55_names_the_finalizer_itself_as_the_gc_metamethod() {
    // No frame sits between the finalizer and the frame that triggered the collection.
    run(
        LuaLanguageLevel::Lua55,
        r#"
        local info
        setmetatable({}, {__gc = function ()
          info = {debug.getinfo(1, "n"), debug.getinfo(2, "n")}
        end})
        collectgarbage()
        assert(info[1].namewhat == "metamethod" and info[1].name == "__gc", info[1].name)
        assert(info[2].namewhat == "global" and info[2].name == "collectgarbage", info[2].name)
        info = nil
        local function loop()
          setmetatable({}, {__gc = function ()
            info = {debug.getinfo(1, "n"), debug.getinfo(2, "n")}
          end})
          repeat local t = {} until info
        end
        loop()
        assert(info[1].namewhat == "metamethod" and info[1].name == "__gc", info[1].name)
        assert(info[2].namewhat == "local" and info[2].name == "loop", info[2].name)
        "#,
    );
}

#[test]
fn lua55_finalizer_errors_become_warnings() {
    // luaE_warnerror: "error in __gc (<message>)", subject to the warning switch.
    run(
        LuaLanguageLevel::Lua55,
        r#"
        warn("@store")
        setmetatable({}, {__gc = function () error("boom") end})
        collectgarbage()
        assert(_WARN and _WARN:find("^error in __gc %(.*boom%)$"), _WARN)
        "#,
    );
}

#[test]
fn lua53_names_the_frame_running_a_finalizer_as_the_gc_metamethod() {
    run(
        LuaLanguageLevel::Lua53,
        r#"
        local info
        setmetatable({}, {__gc = function ()
          info = {debug.getinfo(1, "n"), debug.getinfo(2, "nS")}
        end})
        collectgarbage()
        assert(info[1].namewhat == "" and info[1].name == nil, info[1].name)
        assert(info[2].what == "C" and info[2].namewhat == "metamethod" and info[2].name == "__gc")
        info = nil
        local function loop()
          setmetatable({}, {__gc = function ()
            info = {debug.getinfo(1, "n"), debug.getinfo(2, "nS")}
          end})
          repeat local t = {} until info
        end
        loop()
        assert(info[2].what == "Lua" and info[2].namewhat == "metamethod" and info[2].name == "__gc")
        "#,
    );
}

#[test]
fn sources_need_not_be_valid_utf8() {
    run_both(
        r#"
        local f = assert(load("return '\255\128', 1 -- \200"))
        local a, b = f()
        assert(a == "\255\128" and b == 1)
        local g, msg = load("x = 1 \200")
        -- LuaTeX's texlua takes bytes above 127 for identifier characters
        assert(_VERSION == "Lua 5.3" or not g and msg:find("near '<\\200>'"), msg)
        local name = os.tmpname()
        local fh = assert(io.open(name, "wb"))
        fh:write("return '\255', ...")
        fh:close()
        local ok, v = pcall(dofile, name)
        os.remove(name)
        assert(ok and v == "\255", v)
        "#,
    );
}

#[test]
fn lua55_string_identity_follows_c_lua() {
    run(
        LuaLanguageLevel::Lua55,
        r#"
        local function addr(s) return string.format("%p", s) end
        -- equal long literals of a chunk are one string; concatenation makes a new one
        local s1 = "01234567890123456789012345678901234567890123456789"
        local function f() return "01234567890123456789012345678901234567890123456789" end
        assert(addr(s1) == addr(f()))
        local sd = "0123456789" .. "0123456789012345678901234567890123456789"
        assert(sd == s1 and addr(sd) ~= addr(s1))
        -- gsub returns its subject when nothing was replaced
        local s = string.rep("a", 100)
        assert(addr(s) == addr((string.gsub(s, "b", "c"))))
        assert(addr(s) == addr((string.gsub(s, ".", {x = "y"}))))
        assert(addr(s) == addr((string.gsub(s, ".", function () return false end))))
        assert(addr(s) ~= addr((string.gsub(s, "a", "a"))))
        local ok, msg = pcall(string.format, "%" .. string.rep("0", 30) .. "d", 1)
        assert(not ok and msg:find("invalid format (too long)", 1, true), msg)
        "#,
    );
}

#[test]
fn to_be_closed_files_are_closed() {
    run(
        LuaLanguageLevel::Lua55,
        r#"
        local name = os.tmpname()
        local F
        do
          local f <close> = assert(io.open(name, "w"))
          F = f
        end
        assert(io.type(F) == "closed file" and tostring(F) == "file (closed)", tostring(F))
        local ok = pcall(function ()
          local f <close> = assert(io.open(name, "w"))
          F = f
          error("x")
        end)
        assert(not ok and io.type(F) == "closed file")
        os.remove(name)
        "#,
    );
}

#[test]
fn error_messages_follow_the_language_level() {
    run_both(
        r#"
        local lua53 = _VERSION == "Lua 5.3"
        for _, f in ipairs{math.max, math.min} do
          local ok, msg = pcall(f)
          assert(not ok and msg:find("(value expected)", 1, true), msg)
        end
        local x
        local lud = debug.upvalueid(function () return x end, 1)
        local ok, msg = pcall(debug.setuservalue, lud, {})
        assert(not ok and msg:find("userdata expected, got light userdata", 1, true), msg)
        -- 5.4+ see the labels of enclosing blocks
        local f, msg = load("::l1:: do ::l1:: end")
        if lua53 then
          assert(f, msg)
        else
          assert(not f and msg:find("label 'l1' already defined on line 1", 1, true), msg)
        end
        -- a method name past the RK range: 5.3 still used OP_SELF
        local t = {}
        for i = 1, 300 do t[i] = "aaa = x" .. i end
        local _, msg = pcall(load(table.concat(t, "; ") .. "; local t = {}; t:bbb()"))
        assert(msg:find(lua53 and "(method 'bbb')" or "(field 'bbb')", 1, true), msg)
        -- 5.4+ attribute a call to the line of its arguments
        local _, msg = pcall(load("local a = {x = 13}\na\n.\nx\n(\n23\n)"))
        assert(msg:find(lua53 and "]:2:" or "]:5:", 1, true), msg)
        local _, msg = pcall(load("local a = {}\na\n:\nx\n'str'"))
        assert(msg:find(lua53 and "]:2:" or "]:5:", 1, true), msg)
        "#,
    );
}

#[test]
fn lua55_random_numbers_match_c_lua() {
    run(
        LuaLanguageLevel::Lua55,
        r##"
        math.randomseed(1007, 0)
        assert(math.random() == (0x7a7040a5a323c9d6 >> 11) * 2.0^-53)
        math.randomseed(42)
        local t = {}
        for i = 1, 12 do t[#t + 1] = math.random(1, 10) end
        for i = 1, 4 do t[#t + 1] = math.random(1000) end
        t[#t + 1] = math.random(-5, 2^40)
        t[#t + 1] = math.random(math.mininteger, math.maxinteger)
        local got = table.concat(t, " ")
        assert(got == "6 2 6 6 7 2 9 3 8 8 1 1 831 872 225 126 860715225255 -6231897288346500571", got)
        assert(select("#", math.randomseed()) == 2)
        assert(not pcall(math.randomseed, nil))
        "##,
    );
}
