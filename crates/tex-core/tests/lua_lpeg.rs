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

// LPeg 1.1 additions, expected values from `lua5.1` with lpeg 1.1.0.
#[test]
fn lpeg_1_1_accumulator_and_utf_ranges() {
    let out = run_collecting(
        r#"
        local P, C, Cc, Ct = lpeg.P, lpeg.C, lpeg.Cc, lpeg.Ct
        local cat = function(acc, ...) return acc .. "|" .. table.concat({...}, ",") end
        print(lpeg.match(C(1) * (C(1) % cat)^0, "abcd"))
        print(lpeg.match(Cc("x") * ((C(1) * C(1)) % cat)^0, "abcd"))
        print(lpeg.match(Cc("x") * Cc("y") * (C(1) % cat), "ab"))
        print(pcall(lpeg.match, C(1) % cat, "abcd"))
        print(select(2, pcall(lpeg.match, lpeg.Cs(Cc("x") * (C(1) % cat)), "ab")))
        local u = lpeg.utfR
        print(lpeg.match(u(0x80, 0x10ffff), "\195\169"), lpeg.match(u(0x100, 0x10ffff), "\195\169"))
        print(lpeg.match(u(0, 0x7f)^0, "abc\195"), lpeg.match(u(0, 0x10ffff)^0, "a\195\169\226\130\172\240\159\152\128"))
        print(lpeg.match(u(0, 0x10ffff), "\237\160\128"), lpeg.match(u(0, 0x10ffff), "\192\128"), lpeg.match(u(0, 0x10ffff), "\244\144\128\128"), lpeg.match(u(0, 0x10ffff), "\255"))
        print(lpeg.match(u(0x800, 0xffff)^1, "\226\130\172\226\130"))
        print(pcall(u, 5, 3))
        print(pcall(u, 0, 0x110000))
        "#,
        "@acc.lua",
    );
    let lines: Vec<&str> = out.lines().collect();
    let expected = [
        "a|b|c|d",
        "x|a,b|c,d",
        "x\ty|a",
        "false\tno previous value for accumulator capture",
        "invalid context for an accumulator capture",
        "3\tnil",
        "4\t11",
        "4\tnil\tnil\tnil",
        "4",
    ];
    assert_eq!(&lines[..expected.len()], &expected);
    assert!(lines[expected.len()].starts_with("false\t") && lines[expected.len()].ends_with("(empty range)"));
    assert!(lines[expected.len() + 1].ends_with("(invalid code point)"));
}
