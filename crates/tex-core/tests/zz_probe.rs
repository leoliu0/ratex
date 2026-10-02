use tex_core::engine::{Engine, EngineKind, InteractionMode};
#[test]
fn probe() {
    let Ok(path) = std::env::var("PROBE") else { return };
    let body = std::fs::read_to_string(&path).unwrap();
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.set_interaction_mode(InteractionMode::Nonstop);
    e.input.push_file("t.tex".to_string(), body.into_bytes());
    e.run();
    std::fs::write(format!("{path}.rx"), &e.term).unwrap();
}
