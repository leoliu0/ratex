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
