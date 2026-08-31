//! Probe: run truncated expl3 (through the str module) and call
//! \exp_args_generate:n { oooo } directly to observe the invalid-exp-args path.
use tex_core::engine::Engine;

#[test]
fn probe_exp_args_generate() {
    let mut eng = Engine::new(true);
    eng.init_primitives();
    eng.add_nullfont();
    let data = std::fs::read("/tmp/texdbg/expl3-trunc.tex").expect("probe source");
    eng.input.push_file("expl3-trunc.tex".to_string(), data);
    let mut step = 0u64;
    while !eng.end_occurred {
        let t = eng.get_token();
        if t == tex_core::input::EOF_MARKER {
            break;
        }
        step += 1;
        eng.dispatch(t);
        if step > 20_000_000 {
            panic!("runaway");
        }
    }
    eprintln!("steps={} errors={} term_tail={:?}", step, eng.error_count, {
        let n = eng.term.len();
        &eng.term[n.saturating_sub(1500)..]
    });
}
