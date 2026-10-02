//! Node attribute lists on engine nodes. Expected values come from
//! `luatex --ini` on the same input (cmr10, `\attribute` regions, boxes made
//! after the group ends take the list current when `\hbox` started).

use tex_core::engine::{Engine, EngineKind};

fn run(body: &str) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("lua_attrs_{}_{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&dir).unwrap();
    let lua = dir.join("d.lua");
    std::fs::write(
        &lua,
        r#"callback.register("hpack_filter", function(h)
  local t = {}
  for n in node.traverse(h) do
    local a = {}
    for k = 1, 9 do local v = node.has_attribute(n, k) if v then a[#a+1] = k.."="..v end end
    t[#t+1] = node.type(n.id)..(n.id == 29 and (":"..n.char) or "").."{"..table.concat(a, ",").."}"
  end
  texio.write_nl("@@"..table.concat(t, " "))
  return true
end)
function dumpbox(b)
  local n = node.direct.tonode(node.direct.getbox(b))
  local a = {}
  for k = 1, 9 do local v = node.has_attribute(n, k) if v then a[#a+1] = k.."="..v end end
  texio.write_nl("@@box{"..table.concat(a, ",").."}")
end"#,
    )
    .unwrap();
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    for (c, cc) in [(b'{', 1u8), (b'}', 2), (b'#', 6)] {
        e.eqtb.cat[c as usize] = cc;
    }
    let src = format!(
        "\\directlua{{tex.enableprimitives('',tex.extraprimitives())}}\\directlua{{dofile(\"{}\")}}\\font\\f=cmr10 \\f {body}\\end\n",
        lua.display()
    );
    e.input.push_file("t.tex".to_string(), src.into_bytes());
    e.run();
    let _ = std::fs::remove_dir_all(&dir);
    e.term.lines().filter_map(|l| l.strip_prefix("@@")).map(str::to_string).collect()
}

#[test]
fn nodes_carry_the_attributes_current_at_creation_and_boxes_those_at_their_start() {
    let lines = run(
        "\\attribute9=3 \\setbox0\\hbox{\\global\\attribute2=7 a}\
         \\setbox1\\hbox{\\attribute4=4 a}\
         \\setbox2\\hbox{{\\attribute6=6 a}}\
         \\directlua{dumpbox(0) dumpbox(1) dumpbox(2)}",
    );
    // luatex: the first box was started before attribute 2 became global
    assert_eq!(
        lines,
        [
            "glyph:97{2=7,9=3}",
            "glyph:97{2=7,4=4,9=3}",
            "glyph:97{2=7,6=6,9=3}",
            "box{9=3}",
            "box{2=7,9=3}",
            "box{2=7,9=3}",
        ]
    );
}

#[test]
fn box_attr_keyword_and_copy_keep_attributes() {
    let lines = run("\\attribute1=1 \\setbox0\\hbox attr 5=2 {a}\\setbox1\\hbox{\\copy0 b}\\directlua{dumpbox(0) dumpbox(1)}");
    assert_eq!(lines.last().map(String::as_str), Some("box{1=1}"));
    assert!(lines.contains(&"box{1=1,5=2}".to_string()), "{lines:?}");
}
