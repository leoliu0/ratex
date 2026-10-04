//! LPeg behaviour, cross-checked against TeX Live 2026's LuaTeX (lpeg 1.0.1):
//! `fixtures/lpeg_cases.lua` is run by both, and `lpeg_cases.expected` is
//! the output of `texlua lpeg_cases.lua` in the fixtures directory.

use tex_core::engine_lua::LuaEngine;
use tex_lua::LuaApi;

/// Run Lua source with a `print` that collects its lines.
fn run_collecting(source: &str, chunk: &str) -> String {
    let mut engine = LuaEngine::new().expect("lua engine");
    engine
        .lua
        .execute(
            r##"
            __lines = {}
            print = function(...)
              local t = {}
              for i = 1, select("#", ...) do t[#t + 1] = tostring((select(i, ...))) end
              __lines[#__lines + 1] = table.concat(t, "\t")
            end
            "##,
        )
        .expect("prelude");
    if let Err(e) = engine.lua.load(source).set_name(chunk).exec() {
        panic!("{}", engine.lua.get_error_message(e).message);
    }
    engine.lua.eval::<String>("return table.concat(__lines, '\\n')").expect("output")
}

#[test]
fn lpeg_matches_luatex_on_the_fixture_corpus() {
    let script = include_str!("fixtures/lpeg_cases.lua");
    let expected = include_str!("fixtures/lpeg_cases.expected");
    let actual = run_collecting(script, "@lpeg_cases.lua");
    let (actual, expected): (Vec<&str>, Vec<&str>) = (actual.lines().collect(), expected.lines().collect());
    for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(a, e, "line {} differs", i + 1);
    }
    assert_eq!(actual.len(), expected.len(), "number of output lines");
}

#[test]
fn unreachable_patterns_and_their_captured_closures_are_collected() {
    run_collecting(
        r#"
        local lpeg = require("lpeg")
        local weak = setmetatable({}, {__mode = "v"})
        local function make(i)
          local pattern
          local callback = function() return pattern end
          pattern = lpeg.Cmt(lpeg.P(string.rep("x", 256)), callback)
          assert(lpeg.match(pattern, "") == nil)
          weak[2 * i - 1], weak[2 * i] = pattern, callback
        end
        for i = 1, 20 do make(i) end
        collectgarbage("collect")
        collectgarbage("collect")
        for i = 1, 40 do assert(weak[i] == nil, "unreachable capture cycle retained") end
        "#,
        "@lpeg_capture_cycles.lua",
    );
}

#[test]
fn live_patterns_preserve_captured_objects_across_callback_collection() {
    run_collecting(
        r#"
        local lpeg = require("lpeg")
        local weak = setmetatable({}, {__mode = "v"})
        local function make()
          local object = {answer = 42}
          local lookup = {x = object}
          local callback = function(_, position, captured)
            collectgarbage("collect")
            assert(captured == object)
            return position, captured
          end
          weak[1], weak[2], weak[3] = object, lookup, callback
          return lpeg.Cmt(lpeg.C(lpeg.P("x")) / lookup, callback)
        end
        local pattern = make()
        collectgarbage("collect")
        local captured = lpeg.match(pattern, "x")
        assert(captured.answer == 42)
        assert(captured == weak[1] and weak[2] and weak[3])
        captured, pattern = nil, nil
        collectgarbage("collect")
        collectgarbage("collect")
        for i = 1, 3 do assert(weak[i] == nil, "released pattern kept capture alive") end
        "#,
        "@lpeg_live_capture_edges.lua",
    );
}

#[test]
fn cloned_and_merged_patterns_trace_shared_capture_tables() {
    run_collecting(
        r#"
        local lpeg = require("lpeg")
        local weak = setmetatable({}, {__mode = "v"})
        local function make()
          local callback = function(_, position)
            collectgarbage("collect")
            return position, "shared"
          end
          local object = {answer = 99}
          local base = lpeg.Cmt(lpeg.P("x"), callback)
          weak[1], weak[2] = callback, object
          return base * base, lpeg.Cg(base), base * lpeg.Cc(object)
        end
        local joined, wrapped, merged = make()
        collectgarbage("collect")
        local a, b = lpeg.match(joined, "xx")
        assert(a == "shared" and b == "shared")
        assert(lpeg.match(wrapped, "x") == "shared")
        local text, object = lpeg.match(merged, "x")
        assert(text == "shared" and object.answer == 99)
        joined, wrapped, merged, object = nil, nil, nil, nil
        collectgarbage("collect")
        collectgarbage("collect")
        assert(weak[1] == nil and weak[2] == nil, "shared captures retained")
        "#,
        "@lpeg_shared_capture_edges.lua",
    );
}
