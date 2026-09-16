use tex_core::engine::Engine;

#[test]
fn compatibility_inputs_stay_in_memory() {
    let old_paths = [
        "tex-language-dat-guard.dat",
        "tex-hyphen-cfg-guard.cfg",
        "tex-fontspec-stub.sty",
        "tex-unicode-math-stub.sty",
        "tex-luacode-stub.sty",
        "tex-luatextra-stub.sty",
        "tex-luaotfload-stub.sty",
    ]
    .map(|name| std::env::temp_dir().join(name));
    for path in &old_paths {
        let _ = std::fs::remove_file(path);
    }

    let mut engine = Engine::new(false);
    engine.init_primitives();
    assert!(engine.input_file("fontspec.sty"));
    assert_eq!(engine.input.current_file_name(), "<compat:fontspec.sty>");
    for path in old_paths {
        assert!(
            !path.exists(),
            "compatibility input unexpectedly created {}",
            path.display()
        );
    }
}

#[test]
fn job_state_prefers_output_then_primary_source_and_never_the_invocation_directory() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "tex-state-resolution-{}-{nonce}",
        std::process::id()
    ));
    let source = root.join("source");
    let output = root.join("output");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(source.join("main.aux"), "source").unwrap();

    let mut engine = Engine::new(false);
    engine.main_dir = Some(source.clone());
    engine.out_dir = format!("{}/", output.display());
    engine.job_name = "main".to_string();
    assert_eq!(
        engine.resolve_input_path("main.aux").unwrap(),
        source.join("main.aux")
    );

    std::fs::write(output.join("main.aux"), "output").unwrap();
    assert_eq!(
        engine.resolve_input_path("main.aux").unwrap(),
        output.join("main.aux")
    );
    std::fs::remove_file(output.join("main.aux")).unwrap();
    std::fs::remove_file(source.join("main.aux")).unwrap();
    engine.allow_missing_main_aux = true;
    assert!(engine.resolve_input_path("main.aux").is_none());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn absolute_state_inputs_keep_normal_tex_input_semantics() {
    let root =
        std::env::temp_dir().join(format!("tex-absolute-state-input-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let aux = root.join("required.aux");
    let bbl = root.join("references.bbl");
    std::fs::write(&aux, "auxiliary").unwrap();
    std::fs::write(&bbl, "bibliography").unwrap();

    let mut engine = Engine::new(false);
    engine.job_name = "main".to_string();
    engine.allow_missing_main_aux = true;
    assert_eq!(
        engine.resolve_input_path(aux.to_str().unwrap()).unwrap(),
        aux
    );
    assert_eq!(
        engine.resolve_input_path(bbl.to_str().unwrap()).unwrap(),
        bbl
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn empty_aux_fallback_is_limited_to_latex_main_job_state() {
    let root = std::env::temp_dir().join(format!("tex-empty-aux-scope-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();

    let mut latex = Engine::new(false);
    latex.init_primitives();
    latex.main_dir = Some(root.clone());
    latex.aux_dir = Some(root.join("managed-state"));
    latex.job_name = "main".to_string();
    latex.allow_missing_main_aux = true;
    std::fs::write(root.join("main.aux"), "stale source state").unwrap();
    assert!(latex.input_file("main.aux"));
    assert_eq!(latex.input.current_file_name(), "<empty-aux:main.aux>");
    std::fs::remove_file(root.join("main.aux")).unwrap();

    let mut deliberate = Engine::new(false);
    deliberate.init_primitives();
    deliberate.main_dir = Some(root.clone());
    deliberate.job_name = "main".to_string();
    deliberate.allow_missing_main_aux = true;
    assert!(!deliberate.input_file("required.aux"));
    assert!(deliberate
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("required.aux")));

    let mut plain = Engine::new(false);
    plain.init_primitives();
    plain.main_dir = Some(root.clone());
    plain.job_name = "main".to_string();
    assert!(!plain.input_file("main.aux"));

    let _ = std::fs::remove_dir_all(root);
}
