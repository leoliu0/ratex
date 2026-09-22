use tex_core::engine::{Engine, EngineKind};

fn boot_lua() -> Engine {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.eqtb.cat[b'{' as usize] = 1;
    e.eqtb.cat[b'}' as usize] = 2;
    e.eqtb.cat[b'#' as usize] = 6;
    e.eqtb.cat[b' ' as usize] = 10;
    e.eqtb.cat[b'\n' as usize] = 5;
    e.eqtb.cat[b'\r' as usize] = 5;
    e
}
fn run_tex(e: &mut Engine, src: &str) {
    e.input
        .push_file("t.tex".to_string(), src.as_bytes().to_vec());
    e.run();
}

#[test]
fn directlua_prints_text_into_document() {
    let mut e = boot_lua();
    run_tex(
        &mut e,
        r#"
\message{MSG=\directlua{tex.print("Hello from Lua!")}}
\end
"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    assert!(e.term.contains("MSG=Hello from Lua!"), "term: {}", e.term);
}
#[test]
fn directlua_mutates_count_register() {
    let mut e = boot_lua();
    run_tex(
        &mut e,
        r#"
\count1=10
\directlua{tex.count[1] = tex.count[1] + 32}
\message{COUNT=\the\count1}
\end
"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    assert!(e.term.contains("COUNT=42"), "term: {}", e.term);
}

#[test]
fn directlua_works_inside_edef() {
    let mut e = boot_lua();
    run_tex(
        &mut e,
        r#"
\edef\result{\directlua{tex.print(string.upper("ratex_lua"))}}
\message{RESULT=\result}
\end
"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    assert!(e.term.contains("RESULT=RATEX_LUA"), "term: {}", e.term);
}

#[test]
fn directlua_computes_sp() {
    let mut e = boot_lua();
    run_tex(
        &mut e,
        r#"
\edef\result{\directlua{tex.print(tostring(tex.sp("10pt")))}}
\message{SP=\result}
\end
"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    assert!(e.term.contains("SP=655360"), "term: {}", e.term);
}

#[test]
fn test_lua_metatable_index() {
    use tex_lua::LuaApi;
    let mut lua = tex_lua::Lua::new_lua53(tex_lua::SafeOption::default());
    lua.open_stdlib(tex_lua::Stdlib::All).unwrap();
    let res: String = lua.eval(r#"
        local t = setmetatable({}, { __index = function(tbl, k) return "key_" .. k end })
        return t[1]
    "#).unwrap();
    assert_eq!(res, "key_1");
}

#[test]
fn test_rust_metatable_index() {
    use tex_lua::LuaApi;
    let mut lua = tex_lua::Lua::new_lua53(tex_lua::SafeOption::default());
    lua.open_stdlib(tex_lua::Stdlib::All).unwrap();
    let count_tbl = lua.create_table().unwrap();
    let fn_idx = lua.create_function(|_tbl: tex_lua::LuaValue, k: i64| -> tex_lua::LuaResult<i64> {
        Ok(k * 10)
    }).unwrap();
    let count_meta = lua.create_table().unwrap();
    count_meta.set("__index", fn_idx).unwrap();
    count_tbl.set_metatable(Some(&count_meta)).unwrap();
    assert!(count_tbl.has_metatable(), "count_tbl must have metatable");
    lua.set_global("mycount", count_tbl.clone()).unwrap();
    let g: tex_lua::LuaTable = lua.get_global("mycount").unwrap().unwrap();
    assert!(g.has_metatable(), "global mycount must have metatable");
    let vals: Vec<tex_lua::LuaValue> = lua.eval_multi("return mycount[1]").unwrap();
    assert_eq!(vals.len(), 1);
    assert_eq!(vals[0].as_integer(), Some(10));
}

#[test]
fn directlua_node_interface_reports_types_and_ids() {
    let mut e = boot_lua();
    run_tex(
        &mut e,
        r#"
\edef\res{\directlua{
    local glyph_id = node.id("glyph")
    local glyph_type = node.type(glyph_id)
    tex.print(tostring(glyph_id) .. ":" .. glyph_type)
}}
\message{NODE=\res}
\end
"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    assert!(e.term.contains("NODE=29:glyph"), "term: {}", e.term);
}

#[test]
fn directlua_callback_interface_registers_and_finds() {
    let mut e = boot_lua();
    run_tex(
        &mut e,
        r#"
\directlua{
    callback.register("test_cb", function(x) return x end)
    local found = callback.find("test_cb")
    assert(type(found) == "function")
    tex.print("CALLBACK_OK")
}
\message{DONE}
\end
"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    assert!(e.term.contains("DONE"), "term: {}", e.term);
}

#[test]
fn directlua_fontloader_and_shaping_modules_work() {
    let mut e = boot_lua();
    run_tex(
        &mut e,
        r#"
\directlua{
    local info = fontloader.info("lmroman10-regular.otf")
    assert(type(info) == "table")
    assert(type(info.fontname) == "string")
    local f = fontloader.open("lmroman10-regular.otf")
    assert(type(f) == "table")
    assert(f.units_per_em == 1000)
    local buf = luaharfbuzz.Buffer.new()
    buf:add_utf8("TeX")
    local glyphs = buf:get_glyph_infos_and_positions()
    assert(glyphs[3] ~= nil)
    font.define(1, { name = "testfont", size = 655360 })
    local loaded = font.getfont(1)
    assert(loaded and loaded.name == "testfont")
    tex.print("FONTS_OK")
}
\message{FONTS_STATUS}
\end
"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    assert!(e.term.contains("FONTS_STATUS"), "term: {}", e.term);
}

#[test]
fn directlua_runtime_modules_work() {
    let mut e = boot_lua();
    run_tex(
        &mut e,
        r#"
\directlua{
    local hash = md5.sumhexa("hello")
    assert(hash == "5d41402abc4b2a76b9719d911017c592")
    local types = img.types()
    assert(types[1] == "png")
    local obj = pdf.immediateobj(1)
    assert(obj == 1)
    local l = lang.new(1)
    assert(l.id == 1)
    local cur = lfs.currentdir()
    assert(type(cur) == "string")
    tex.print("RUNTIME_MODULES_OK")
}
\message{MODULES_STATUS}
\end
"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    assert!(e.term.contains("MODULES_STATUS"), "term: {}", e.term);
}

#[test]
fn directlua_mplib_module_works() {
    let mut e = boot_lua();
    run_tex(
        &mut e,
        r#"
\directlua{
    assert(mplib.version() == "3.00")
    local mp = mplib.new()
    assert(mp)
    local res = mp:execute("beginfig(1); draw (0,0)--(100,100); endfig;")
    assert(res.status == 0)
    assert(type(res.fig) == "table")
    assert(res.fig[1])
    local f = res.fig[1]
    assert(f:charcode() == 1)
    local ps = f:postscript()
    assert(string.find(ps, "Adobe", 1, true))
    local svg = f:svg()
    assert(string.find(svg, "svg", 1, true))
    mp:finish()
    tex.print("MPLIB_OK")
}
\message{MPLIB_STATUS}
\end
"#,
    );
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    assert!(e.term.contains("MPLIB_STATUS"), "term: {}", e.term);
}
