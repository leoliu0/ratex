use tex_core::engine::Engine;
use tex_core::eqtb::Equiv;

#[test]
fn onlypreamble_defined_after_expl3() {
    let mut eng = Engine::new(true);
    eng.init_primitives();
    let _ = eng.hyphen_trie.load_hyphen_file(std::path::Path::new(
        "/usr/share/texmf-dist/tex/generic/hyphen/hyphen.tex",
    ));
    eng.add_nullfont();
    eng.input_file("latex.ltx");

    let mut step = 0u64;
    while !eng.end_occurred {
        let t = eng.get_token();
        if t == tex_core::input::EOF_MARKER {
            break;
        }
        step += 1;
        let fnm = eng.input.current_file_name();
        let ln = eng.input.current_file_line();
        if step % 50_000 == 0 {
            eprintln!("step {} at {}:{} stack_depth={}", step, fnm, ln, eng.input.stack.len());
        }
        if fnm.ends_with("latex.ltx") && ln == 1227 && t.is_cs() {
            let nm = eng.cs.name(t.cs_id());
            if nm == b"@onlypreamble" {
                let eq = eng
                    .eqtb
                    .resolve(t.cs_id())
                    .map(|e| e.kind_name())
                    .unwrap_or("none");
                eprintln!(
                    "HIT \\@onlypreamble at L{} eq={} level={} step={}",
                    ln, eq, eng.eqtb.cur_level, step
                );
            }
        }
        eng.dispatch(t);
        if fnm.ends_with("latex.ltx") && ln >= 1230 {
            eprintln!("BREAK at {}:{}", fnm, ln);
            break;
        }
        if step > 20_000_000 {
            panic!("too many steps, still at {}:{}", fnm, ln);
        }
    }
    let id = eng.cs.lookup(b"@onlypreamble").expect("cs interned");
    match eng.eqtb.resolve(id) {
        Some(Equiv::Macro(_)) => {}
        other => panic!(
            "\\@onlypreamble not a macro after L1230: {:?} errors={} term={}",
            other.map(|e| e.kind_name()),
            eng.error_count,
            eng.term
        ),
    }
    eprintln!(
        "ok: \\@onlypreamble defined, level={} errors={} step={}",
        eng.eqtb.cur_level, eng.error_count, step
    );
}
