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

#[derive(crate::LuaUserData)]
struct ScopedProbe {
    pub count: i64,
}

#[crate::lua_methods]
impl ScopedProbe {
    pub fn get(&self) -> i64 {
        self.count
    }
}

#[test]
fn arithmetic_on_expired_userdata_raises_instead_of_keeping_stale_register() {
    use crate::{Lua, LuaApi};

    let mut lua = Lua::new(SafeOption::default());
    lua.open_stdlib(Stdlib::All).unwrap();
    let mut probe = ScopedProbe { count: 1 };
    lua.scope(|scope| {
        let borrowed = scope.create_userdata_ref(&mut probe)?;
        scope.globals().set("borrowed", &borrowed)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(probe.count, 1);

    let ok: bool = lua
        .load("local r = 'stale'; return (pcall(function() r = borrowed + 1 end))")
        .eval()
        .unwrap();
    assert!(!ok, "arithmetic on an expired userdata reference must raise");
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
