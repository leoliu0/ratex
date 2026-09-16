use tex_core::engine::{Engine, InteractionMode};

#[test]
fn useobjnum_rejects_invalid_unreserved_and_duplicate_numbers_without_emitting_objects() {
    let lines = [
        r"\pdfobj reserveobjnum",
        r"\count0=\pdflastobj",
        r"\pdfobj useobjnum 0 {<< /InvalidZero true >>}",
        r"\pdfobj useobjnum -1 {<< /InvalidNegative true >>}",
        r"\pdfobj useobjnum 999 {<< /InvalidUnreserved true >>}",
        r"\pdfobj useobjnum \count0 {<< /ValidReserved true /Self \the\pdflastobj >>}",
        r"\pdfobj useobjnum \count0 {<< /InvalidDuplicate true >>}",
        r"\pdfobj {<< /ValidFresh true /Self \the\pdflastobj >>}",
        r"\shipout\hbox{\vrule width1pt height1pt}",
        r"\end",
    ];
    let mut engine = Engine::new(false);
    engine.init_primitives();
    engine.add_nullfont();
    engine.set_interaction_mode(InteractionMode::Nonstop);
    engine.input.push_file(
        "pdfobj-errors.tex".to_string(),
        lines.join("\n").into_bytes(),
    );

    engine.run();

    assert_eq!(engine.error_count, 4, "{}", engine.diagnostic_output);
    assert_eq!(engine.diagnostics.len(), 4, "{}", engine.diagnostic_output);
    assert_eq!(
        engine.diagnostics[0].message,
        "PDF object number 0 is invalid for \\pdfobj useobjnum; use the positive number returned by \\pdfobj reserveobjnum; object omitted"
    );
    assert_eq!(
        engine.diagnostics[1].message,
        "PDF object number -1 is invalid for \\pdfobj useobjnum; use the positive number returned by \\pdfobj reserveobjnum; object omitted"
    );
    assert_eq!(
        engine.diagnostics[2].message,
        "PDF object number 999 was not reserved by \\pdfobj reserveobjnum; reserve an object first and pass its \\pdflastobj value; object omitted"
    );
    assert_eq!(
        engine.diagnostics[3].message,
        "PDF object number 5 has already been defined; each reserved object number can be used only once; object omitted"
    );

    for (diagnostic, line_index, operand) in [
        (&engine.diagnostics[0], 2, "0"),
        (&engine.diagnostics[1], 3, "-1"),
        (&engine.diagnostics[2], 4, "999"),
        (&engine.diagnostics[3], 6, "\\count0"),
    ] {
        let source = diagnostic.primary.as_ref().expect("missing operand source");
        assert_eq!(source.name, "pdfobj-errors.tex");
        assert_eq!(source.line, line_index as u32 + 1);
        assert_eq!(source.column, lines[line_index].find(operand).unwrap() + 1);
        assert!(diagnostic
            .help
            .as_deref()
            .is_some_and(|help| help.contains("define that number exactly once")));
    }

    assert_eq!(engine.pdf_doc.objects.len(), 2);
    assert_eq!(engine.pdf_doc.objects[0].0, 5);
    assert_eq!(engine.pdf_doc.objects[1].0, 6);
    assert_eq!(engine.pdf_last_obj, 6);
    let objects = engine
        .pdf_doc
        .objects
        .iter()
        .flat_map(|(_, body)| body.iter().copied())
        .collect::<Vec<_>>();
    let objects = String::from_utf8_lossy(&objects);
    assert!(objects.contains("ValidReserved"), "{objects}");
    assert!(objects.contains("ValidFresh"), "{objects}");
    assert!(objects.contains("/Self 5"), "{objects}");
    assert!(objects.contains("/Self 6"), "{objects}");
    assert!(!objects.contains("InvalidZero"), "{objects}");
    assert!(!objects.contains("InvalidNegative"), "{objects}");
    assert!(!objects.contains("InvalidUnreserved"), "{objects}");
    assert!(!objects.contains("InvalidDuplicate"), "{objects}");

    let pdf = tex_core::pdffile::write_pdf(&engine.pdf_doc);
    lopdf::Document::load_mem(&pdf).expect("invalid useobjnum input must not corrupt the PDF");
}

#[test]
fn pdf_xref_with_leading_newline_is_repaired_and_loads_successfully() {
    let pdf_path = "/home/leo/dd/tex/output/corpus-high-risk-95/0803.0966/figure-gamma_rules.pdf";
    if let Ok(bytes) = std::fs::read(pdf_path) {
        let mut next_obj = 100;
        let res = tex_core::pdf_images::import_pdf_page(&bytes, 1, b"MediaBox", 1, &mut next_obj);
        assert!(res.is_ok(), "failed to import PDF page: {:?}", res.err());
    }
}

#[test]
fn pdfximagebbox_queries_bounding_box_coordinates_correctly() {
    let pdf_path = "/home/leo/dd/tex/output/corpus-high-risk-95/0803.0966/figure-gamma_rules.pdf";
    if !std::path::Path::new(pdf_path).exists() {
        return;
    }
    let lines = [
        format!(r"\pdfximage{{{pdf_path}}}"),
        r"\dimen0=\pdfximagebbox\pdflastximage 1".to_string(),
        r"\dimen1=\pdfximagebbox\pdflastximage 2".to_string(),
        r"\dimen2=\pdfximagebbox\pdflastximage 3".to_string(),
        r"\dimen3=\pdfximagebbox\pdflastximage 4".to_string(),
        r"\shipout\hbox{\the\dimen0 \the\dimen1 \the\dimen2 \the\dimen3}".to_string(),
        r"\end".to_string(),
    ];
    let mut engine = Engine::new(false);
    engine.init_primitives();
    engine.add_nullfont();
    engine.set_interaction_mode(InteractionMode::Nonstop);
    engine.input.push_file(
        "test-bbox.tex".to_string(),
        lines.join("\n").into_bytes(),
    );
    engine.run();
    assert_eq!(engine.error_count, 0, "{}", engine.diagnostic_output);
    assert_eq!(engine.eqtb.dimen[0], 0);
    assert_eq!(engine.eqtb.dimen[1], 0);
    assert_eq!(engine.eqtb.dimen[2], 28417720);
    assert_eq!(engine.eqtb.dimen[3], 28417720);
}
