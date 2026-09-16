use tex_core::engine::{Engine, InteractionMode};
use tex_core::prim::IntParam;

fn run(source: &str, ini: bool) -> Engine {
    let mut engine = Engine::new(ini);
    engine.init_primitives();
    engine.add_nullfont();
    engine.set_interaction_mode(InteractionMode::Nonstop);
    engine.input.push_file(
        "linebreak-diagnostics.tex".to_string(),
        source.as_bytes().to_vec(),
    );
    engine.run();
    engine
}

#[test]
fn negative_and_oversized_hyphen_minima_are_safe_and_keep_tex_semantics() {
    let assignment = r"\lefthyphenmin=-1 \righthyphenmin=64";
    let source = format!(
        "\\catcode`\\{{=1 \\catcode`\\}}=2\n\\patterns{{a1a}}\n\\font\\ten=cmr10 \\ten\n\\lefthyphenmin=0 \\message{{ZERO-MIN=\\the\\lefthyphenmin}}\n{assignment}\n\\hsize=1pt banana banana\\par\n\\end"
    );

    let engine = run(&source, true);

    assert_eq!(engine.error_count, 0, "{}", engine.diagnostic_output);
    assert!(engine.term.contains("ZERO-MIN=0"), "{}", engine.term);
    assert_eq!(
        engine.eqtb.int_params[IntParam::LeftHyphenMin.idx() as usize],
        -1
    );
    assert_eq!(
        engine.eqtb.int_params[IntParam::RightHyphenMin.idx() as usize],
        64
    );
    assert!(engine.explicit_end_seen, "{}", engine.diagnostic_output);
}

#[test]
fn minimum_hangafter_is_located_and_recovered_before_line_breaking() {
    let assignment =
        r"\hangafter=\count0 \hangindent=1pt \message{RECOVERED-HANGAFTER=\the\hangafter}";
    let source = format!(
        "\\font\\ten=cmr10 \\ten\n\\count0=-2147483647 \\advance\\count0 by -1\n{assignment}\ntext\\par\n\\end"
    );

    let engine = run(&source, false);

    assert_eq!(engine.error_count, 1, "{}", engine.diagnostic_output);
    assert!(engine.term.contains("RECOVERED-HANGAFTER=-2147483647"));
    assert_eq!(
        engine.diagnostics[0].message,
        "\\hangafter value -2147483648 has an unrepresentable magnitude; used -2147483647"
    );
    let primary = engine.diagnostics[0]
        .primary
        .as_ref()
        .expect("operand location");
    assert_eq!(primary.name, "linebreak-diagnostics.tex");
    assert_eq!(primary.line, 3);
    assert_eq!(primary.column, assignment.find("\\count0").unwrap() + 1);
    assert!(engine.diagnostics[0]
        .help
        .as_deref()
        .is_some_and(|help| help.contains("-2147483647 through 2147483647")));
    assert!(engine.explicit_end_seen, "{}", engine.diagnostic_output);
}
