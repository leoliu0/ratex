//! The Lua node library (`node`, `node.direct`) against LuaTeX 1.24: the
//! expected outputs in `tests/lua_node/*.out` were produced by `luatex --ini`
//! running the same `*.lua` files (see `tests/lua_node/README`-free generator
//! comment in each test).

use tex_core::engine::{Engine, EngineKind};

fn boot_lua() -> Engine {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.eqtb.cat[b'{' as usize] = 1;
    e.eqtb.cat[b'}' as usize] = 2;
    e.eqtb.cat[b'#' as usize] = 6;
    e.eqtb.cat[b'^' as usize] = 7;
    e.eqtb.cat[b' ' as usize] = 10;
    e.eqtb.cat[b'\n' as usize] = 5;
    e.eqtb.cat[b'\r' as usize] = 5;
    e
}

const PRELUDE: &str = include_str!("lua_node/prelude.lua");

/// Run a Lua chunk with `\directlua`; the lines it passed to `P(...)`
/// (tab-joined `tostring`s) are returned. Same prelude as the LuaTeX probes.
pub fn run_lua(code: &str) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("lua_node_{}_{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out.txt");
    let path = dir.join("p.lua");
    std::fs::write(&path, format!("OUTFILE=[[{}]]\n{PRELUDE}{code}\nPEND()\n", out.display())).unwrap();
    let mut e = boot_lua();
    let src = format!("\\directlua{{dofile(\"{}\")}}\n\\end\n", path.display());
    e.input.push_file("t.tex".to_string(), src.into_bytes());
    e.run();
    let mut result: Vec<String> = std::fs::read_to_string(&out)
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    if e.error_count > 0 {
        // the first line of the first error message
        let text = format!("{:?}", e.diagnostics);
        let msg = text.split("message: \"").nth(1).and_then(|m| m.split("\", original").next()).unwrap_or("?");
        let first = msg.split("\\n").next().unwrap_or("");
        if first.starts_with("LuaTeX error") {
            result.push(format!("! {first}"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[test]
fn probe_from_env() {
    let Ok(file) = std::env::var("LUA_PROBE") else { return };
    let code = std::fs::read_to_string(file).unwrap();
    for l in run_lua(&code) {
        println!("{l}");
    }
}
