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
    e.input.push_file("t.tex".to_string(), src.as_bytes().to_vec());
    e.run();
}

#[test]
fn zz_probe() {
    let Ok(path) = std::env::var("PROBE") else { return };
    let mut e = boot_lua();
    let pre = std::env::var("PROBE_PRE").unwrap_or_default();
    run_tex(&mut e, &format!("{pre}\\directlua{{dofile(\"{path}\")}}\n\\end\n"));
    println!("@@BEGIN\n{}\n--LOG--\n{}\n--DIAG--\n{:?}\n@@END", e.term, e.log, e.diagnostics);
}
