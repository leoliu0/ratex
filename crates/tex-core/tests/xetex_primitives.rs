use tex_core::engine::{Engine, EngineKind, InteractionMode};

fn boot_xetex() -> Engine {
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng
}

#[test]
fn test_xetex_version_and_revision() {
    let mut eng = boot_xetex();
    let src = r#"
\catcode`\{=1 \catcode`\}=2
\ifnum\XeTeXversion=0 \message{VERSION_OK}\else\message{VERSION_BAD}\fi
\edef\rev{\XeTeXrevision}
\message{REV=\rev}
\end
"#;
    eng.input.push_file("test.tex".into(), src.as_bytes().to_vec());
    eng.run();

    assert_eq!(eng.error_count, 0, "errors: {:?}, term: {}", eng.diagnostics, eng.term);
    assert!(eng.term.contains("VERSION_OK"), "term: {}", eng.term);
    assert!(eng.term.contains("REV=.999998"), "term: {}", eng.term);
}

#[test]
fn test_xetex_charclass_and_interchartoks() {
    let mut eng = boot_xetex();
    let src = r#"
\catcode`\{=1 \catcode`\}=2
\XeTeXcharclass 65 = 1
\XeTeXcharclass 66 = 2
\XeTeXinterchartoks 1 2 = {XYZ}
\edef\c{\the\XeTeXcharclass 65}
\message{CLASS=\c}
\edef\toks{\the\XeTeXinterchartoks 1 2}
\message{TOKS=\toks}
\XeTeXinterchartokenstate = 1
\message{STATE=\the\XeTeXinterchartokenstate}
\end
"#;
    eng.input.push_file("test.tex".into(), src.as_bytes().to_vec());
    eng.run();

    assert_eq!(eng.error_count, 0, "errors: {:?}, term: {}", eng.diagnostics, eng.term);
    assert!(eng.term.contains("CLASS=1"), "term: {}", eng.term);
    assert!(eng.term.contains("TOKS=XYZ"), "term: {}", eng.term);
    assert!(eng.term.contains("STATE=1"), "term: {}", eng.term);
}

#[test]
fn test_xetex_font_queries_on_nullfont() {
    let mut eng = boot_xetex();
    let src = r#"
\catcode`\{=1 \catcode`\}=2
\message{FONTTYPE=\the\XeTeXfonttype\nullfont}
\message{GLYPHS=\the\XeTeXcountglyphs\nullfont}
\XeTeXuseglyphmetrics = 1
\message{USEMETRICS=\the\XeTeXuseglyphmetrics}
\end
"#;
    eng.input.push_file("test.tex".into(), src.as_bytes().to_vec());
    eng.run();

    assert_eq!(eng.error_count, 0, "errors: {:?}, term: {}", eng.diagnostics, eng.term);
    assert!(eng.term.contains("FONTTYPE=0"), "term: {}", eng.term);
    assert!(eng.term.contains("GLYPHS=256"), "term: {}", eng.term);
    assert!(eng.term.contains("USEMETRICS=1"), "term: {}", eng.term);
}

#[test]
fn test_xetex_interchartoks_execution() {
    let mut eng = boot_xetex();
    let src = r#"
\catcode`\{=1 \catcode`\}=2
\XeTeXcharclass 65 = 1
\XeTeXcharclass 66 = 2
\XeTeXinterchartoks 1 2 = {\message{INTERCHAR_INSERTED}}
\XeTeXinterchartokenstate = 1
AB
\end
"#;
    eng.input.push_file("test.tex".into(), src.as_bytes().to_vec());
    eng.run();

    assert_eq!(eng.error_count, 0, "errors: {:?}, term: {}", eng.diagnostics, eng.term);
    assert!(eng.term.contains("INTERCHAR_INSERTED"), "term: {}", eng.term);
}

#[test]
fn test_bidi_and_script_detection() {
    use tex_core::native_layout::{char_bidi_is_rtl, detect_script};

    assert!(char_bidi_is_rtl('א'));
    assert!(char_bidi_is_rtl('م'));
    assert!(!char_bidi_is_rtl('A'));
    assert!(!char_bidi_is_rtl('文'));

    assert_eq!(detect_script("שלום"), Some(rustybuzz::script::HEBREW));
    assert_eq!(detect_script("مرحبا"), Some(rustybuzz::script::ARABIC));
    assert_eq!(detect_script("こんにちは"), Some(rustybuzz::script::HIRAGANA));
    assert_eq!(detect_script("Hello"), Some(rustybuzz::script::LATIN));
    assert_eq!(detect_script("世界"), Some(rustybuzz::script::HAN));
}

#[test]
fn test_sfnt_table_detection_and_aat_graphite() {
    use tex_core::font_program::sfnt_has_table;

    let mut dummy_sfnt = vec![0x00, 0x01, 0x00, 0x00]; // sfnt_version
    dummy_sfnt.extend_from_slice(&2u16.to_be_bytes()); // num_tables = 2
    dummy_sfnt.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // search_range, etc.
    // Table 1: GSUB
    dummy_sfnt.extend_from_slice(b"GSUB");
    dummy_sfnt.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 44, 0, 0, 0, 10]);
    // Table 2: Silf
    dummy_sfnt.extend_from_slice(b"Silf");
    dummy_sfnt.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 54, 0, 0, 0, 20]);

    assert!(sfnt_has_table(&dummy_sfnt, b"GSUB"));
    assert!(sfnt_has_table(&dummy_sfnt, b"Silf"));
    assert!(!sfnt_has_table(&dummy_sfnt, b"morx"));
}

#[test]
fn test_unicode_math_scalars_compile_without_error() {
    let mut eng = boot_xetex();
    let src = r#"
\catcode`\{=1 \catcode`\}=2 \catcode`\$=3
$α + x = y$
\message{UNICODE_MATH_OK}
\end
"#;
    eng.input.push_file("test.tex".into(), src.as_bytes().to_vec());
    eng.run();

    assert_eq!(eng.error_count, 0, "errors: {:?}, term: {}", eng.diagnostics, eng.term);
    assert!(eng.term.contains("UNICODE_MATH_OK"), "term: {}", eng.term);
}

#[test]
fn test_xetex_specials_emission() {
    let mut eng = boot_xetex();
    let src = r#"
\catcode`\{=1 \catcode`\}=2
\shipout\vbox{
\special{color push rgb 1 0 0}
\special{x:scale 1.5 1.5}
\special{pdf:literal 0.5 w}
\special{color pop}
\hrule
}
\end
"#;
    eng.input.push_file("test.tex".into(), src.as_bytes().to_vec());
    eng.run();

    assert_eq!(eng.error_count, 0, "errors: {:?}, term: {}", eng.diagnostics, eng.term);
    assert!(!eng.pdf_doc.pages.is_empty(), "PDF page must be produced");
    let page_content = String::from_utf8_lossy(&eng.pdf_doc.pages[0].content);
    assert!(page_content.contains("1 0 0 rg 1 0 0 RG"), "content: {page_content}");
    assert!(page_content.contains("1.5000 0 0 1.5000 0 0 cm"), "content: {page_content}");
    assert!(page_content.contains("0.5 w"), "content: {page_content}");
    assert!(page_content.contains("0 g 0 G"), "content: {page_content}");
}
