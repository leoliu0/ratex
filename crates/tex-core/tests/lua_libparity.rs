//! Lua environment parity with TeX Live 2026's `luatex` (expected values
//! printed by `luatex` itself).

use tex_core::engine_lua::LuaEngine;
use tex_lua::LuaApi;

fn eval(code: &str) -> String {
    let mut engine = LuaEngine::new().expect("lua engine");
    engine.lua.load(code).eval::<String>().unwrap_or_else(|e| panic!("{}", engine.lua.get_error_message(e).message))
}

#[test]
fn math_random_is_libc_random() {
    // luatex: math.randomseed(42); print(math.random(), math.random(10), math.random(5,8))
    let out = eval(
        "math.randomseed(42) local a, b, c = math.random(), math.random(10), math.random(5, 8) \
         return string.format('%.14g %d %d', a, b, c)",
    );
    assert_eq!(out, "0.32996420748532 7 6");
}

#[test]
fn environment_matches_luatex() {
    // luatex: debug has only traceback, setmetatable checks its argument,
    // _G has no _ENV or helper globals, LUATEXCOREVERSION is a float.
    let out = eval(
        "local d = {} for k in pairs(debug) do d[#d + 1] = k end \
         local ok, e = pcall(setmetatable, 1, {}) \
         return table.concat(d, ',') .. '|' .. tostring(rawget(_G, '_ENV')) .. '|' .. tostring(rawget(_G, '__ratex_callback')) \
           .. '|' .. math.type(LUATEXCOREVERSION) .. '|' .. e .. '|' .. tostring(lpeg.utfR)",
    );
    assert_eq!(
        out,
        "traceback|nil|nil|float|bad argument #1 to 'setmetatable' (table expected, got number)|nil"
    );
}

#[test]
fn mplib_solve_path_and_lifecycle() {
    // luatex: solve_path fills the knot tables in place and returns true;
    // finish on a fresh instance returns only the banner log; later calls give nil.
    let out = eval(
        "local mp = mplib.new{} \
         local p = {{x_coord=0,y_coord=0},{x_coord=10,y_coord=5},{x_coord=20,y_coord=0}} \
         local ok = mp:solve_path(p, false) \
         local r = mp:finish() \
         return tostring(ok) .. '|' .. p[1].left_type .. '|' .. p[2].right_type .. '|' .. p[3].right_type \
           .. '|' .. r.log .. '|' .. r.status .. '|' .. tostring(mp:finish()) .. '|' .. mplib.version()",
    );
    assert_eq!(
        out,
        "true|endpoint|explicit|endpoint|This is MetaPost, Version 3.00  14 NOV 2023 22:13|0|nil|3.00"
    );
}
