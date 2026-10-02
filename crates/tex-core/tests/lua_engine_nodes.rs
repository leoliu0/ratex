//! A Lua-defined font typeset through the real main loop: glyph nodes,
//! ligature/kern passes at `\hbox` end and paragraph line breaking.
//! Expected values come from `luatex --ini` on the same input:
//! `\font\f=tst at 10pt \f \setbox0\hbox{AV ffi fi ab}` gives
//! `\hbox(9.15527+1.52588)x57.52563` with `A`, `kern-1.2207`, `V`, glue,
//! `ffi` and `fi` ligatures, glue, `a`, `b`.

use tex_core::engine::{Engine, EngineKind};

const FONT: &str = r#"
local function ch(w, extra)
  local t = {width = w, height = 600000, depth = 100000}
  if extra then for k, v in pairs(extra) do t[k] = v end end
  return t
end
local chars = {
  [65] = ch(500000, {kerns = {[86] = -80000}}),
  [86] = ch(500000), [97] = ch(450000), [98] = ch(450000),
  [102] = ch(300000, {ligatures = {[102] = {type = 0, char = 0xFB00}, [105] = {type = 0, char = 0xFB01}}}),
  [105] = ch(250000),
  [0xFB00] = ch(600000, {ligatures = {[105] = {type = 0, char = 0xFB03}}}),
  [0xFB01] = ch(550000), [0xFB03] = ch(800000), [45] = ch(330000),
}
CAPTURE = {}
callback.register("hpack_filter", function(head, group)
  CAPTURE[#CAPTURE + 1] = tostring((node.dimensions(head)))
  for n in node.traverse(head) do
    CAPTURE[#CAPTURE + 1] = node.type(n.id) .. "/" .. n.subtype .. (n.id == 29 and (" " .. n.char) or "")
  end
  return true
end)
callback.register("define_font", function(name, size, id)
  return {name = name, size = size, designsize = 655360, type = "real", format = "type1",
    parameters = {slant = 0, space = 200000, space_stretch = 100000, space_shrink = 60000, x_height = 300000, quad = 655360, extra_space = 0},
    characters = chars, hyphenchar = 45}
end)
"#;

fn run(tex_body: &str, report: &str) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("lua_engine_nodes_{}_{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out.txt");
    let lua = dir.join("font.lua");
    std::fs::write(&lua, FONT).unwrap();
    let _ = &out;
    let rep = dir.join("report.lua");
    std::fs::write(&rep, format!("local o = {{write = function(_, ...) texio.write(\"@@\" .. table.concat({{...}})) end}}\n{report}\ntexio.write_nl(\"@@END\")\n")).unwrap();
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    for (c, cc) in [(b'{', 1u8), (b'}', 2), (b'#', 6)] {
        e.eqtb.cat[c as usize] = cc;
    }
    let src = format!(
        "\\directlua{{dofile(\"{}\")}}\\font\\f=tst at 10pt \\f \\hsize 100pt \\parindent 0pt \\tolerance 10000 {tex_body}\\directlua{{dofile(\"{}\")}}\\end\n",
        lua.display().to_string().replace('\\', "/"),
        rep.display().to_string().replace('\\', "/")
    );
    e.input.push_file("t.tex".to_string(), src.into_bytes());
    e.run();
    let text = e.term.clone();
    let lines = text.lines().filter_map(|l| l.strip_prefix("@@")).take_while(|l| *l != "END").map(str::to_string).collect();
    let _ = std::fs::remove_dir_all(&dir);
    lines
}

#[test]
fn hbox_of_lua_font_ligates_and_kerns_like_luatex() {
    let lines = run(
        "\\setbox0\\hbox{AV ffi fi ab}",
        r#"for _, l in ipairs(CAPTURE) do o:write(l, "\n") end"#,
    );
    assert_eq!(lines[0], "3770000"); // 57.52563pt
    let rest: Vec<&str> = lines[1..].iter().map(String::as_str).collect();
    assert_eq!(
        rest,
        ["glyph/0 65", "kern/0", "glyph/0 86", "glue/13", "glyph/2 64259", "glue/13", "glyph/2 64257", "glue/13", "glyph/0 97", "glyph/0 98"]
    );
}
