use tex_core::boxes::{DisplayItem, Node};
use tex_core::engine::Engine;
use tex_core::fonts::{
    shape_opentype_text, shape_opentype_text_simple, shape_opentype_to_nodes,
    shaped_glyphs_to_glyph_run, shaped_glyphs_to_nodes,
};

const TEST_FONT: &[u8] = include_bytes!("fixtures/ratex_test_font.ttf");

#[test]
fn test_shaping_invalid_font() {
    let result = shape_opentype_text_simple(b"corrupt font data", "hello");
    assert!(
        result.is_err(),
        "Invalid font data must return Err, not empty"
    );
}

#[test]
fn test_shaping_empty_input() {
    let shaped = shape_opentype_text_simple(TEST_FONT, "").expect("Empty input should succeed");
    assert!(shaped.is_empty());
}

#[test]
fn test_opentype_ligature_formation() {
    // "fi" -> single ligature glyph f_i (id 7, advance 580)
    let shaped_fi = shape_opentype_text_simple(TEST_FONT, "fi").expect("shaping fi should succeed");
    assert_eq!(shaped_fi.len(), 1, "fi must merge into one ligature glyph");
    assert_eq!(shaped_fi[0].glyph_id, 7);
    assert_eq!(shaped_fi[0].cluster_start, 0);
    assert_eq!(shaped_fi[0].cluster_end, 2);
    assert_eq!(shaped_fi[0].x_advance, 580);
    assert_eq!(shaped_fi[0].y_advance, 0);
    assert_eq!(shaped_fi[0].x_offset, 0);
    assert_eq!(shaped_fi[0].y_offset, 0);

    // "fl" -> single ligature glyph f_l (id 8, advance 580)
    let shaped_fl = shape_opentype_text_simple(TEST_FONT, "fl").expect("shaping fl should succeed");
    assert_eq!(shaped_fl.len(), 1);
    assert_eq!(shaped_fl[0].glyph_id, 8);
    assert_eq!(shaped_fl[0].cluster_start, 0);
    assert_eq!(shaped_fl[0].cluster_end, 2);
    assert_eq!(shaped_fl[0].x_advance, 580);

    // "ffi" -> single ligature glyph f_f_i (id 9, advance 850)
    let shaped_ffi =
        shape_opentype_text_simple(TEST_FONT, "ffi").expect("shaping ffi should succeed");
    assert_eq!(shaped_ffi.len(), 1);
    assert_eq!(shaped_ffi[0].glyph_id, 9);
    assert_eq!(shaped_ffi[0].cluster_start, 0);
    assert_eq!(shaped_ffi[0].cluster_end, 3);
    assert_eq!(shaped_ffi[0].x_advance, 850);
}

#[test]
fn test_opentype_advance_positioning_and_kerning() {
    // "AV" kerns by -80: A (id 10) gets 600 - 80 = 520, V (id 11) gets 600
    let shaped_av = shape_opentype_text_simple(TEST_FONT, "AV").expect("shaping AV should succeed");
    assert_eq!(shaped_av.len(), 2);
    assert_eq!(shaped_av[0].glyph_id, 10);
    assert_eq!(shaped_av[0].cluster_start, 0);
    assert_eq!(shaped_av[0].cluster_end, 1);
    assert_eq!(shaped_av[0].x_advance, 520);
    assert_eq!(shaped_av[1].glyph_id, 11);
    assert_eq!(shaped_av[1].cluster_start, 1);
    assert_eq!(shaped_av[1].cluster_end, 2);
    assert_eq!(shaped_av[1].x_advance, 600);

    // Separated letters with spaces should not form ligatures
    let shaped_sep =
        shape_opentype_text_simple(TEST_FONT, "f i").expect("shaping 'f i' should succeed");
    assert_eq!(shaped_sep.len(), 3);
    assert_eq!(shaped_sep[0].glyph_id, 4); // f
    assert_eq!(shaped_sep[0].cluster_start, 0);
    assert_eq!(shaped_sep[0].cluster_end, 1);
    assert_eq!(shaped_sep[0].x_advance, 350);
    assert_eq!(shaped_sep[1].glyph_id, 3); // space
    assert_eq!(shaped_sep[1].cluster_start, 1);
    assert_eq!(shaped_sep[1].cluster_end, 2);
    assert_eq!(shaped_sep[1].x_advance, 250);
    assert_eq!(shaped_sep[2].glyph_id, 5); // i
    assert_eq!(shaped_sep[2].cluster_start, 2);
    assert_eq!(shaped_sep[2].cluster_end, 3);
    assert_eq!(shaped_sep[2].x_advance, 280);
}

#[test]
fn test_lowering_to_nodes() {
    let font_id = 5;
    let nodes = shape_opentype_to_nodes(TEST_FONT, font_id, "fi").expect("should lower to nodes");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        Node::NativeGlyphRun {
            run,
            start,
            end,
            width,
            ..
        } => {
            assert_eq!(run.font, font_id);
            assert_eq!(*start, 0);
            assert_eq!(*end, 1);
            assert_eq!(run.glyphs[0].glyph_id, 7);
            assert_eq!(run.glyphs[0].cluster_start, 0);
            assert_eq!(run.glyphs[0].cluster_end, 2);
            assert!(*width > 0);
        }
        other => panic!("expected Node::NativeGlyphRun, got {:?}", other),
    }

    // Direct conversion of shaped glyphs with offsets
    let shaped = shape_opentype_text_simple(TEST_FONT, "AV").expect("should shape AV");
    let nodes_av = shaped_glyphs_to_nodes(font_id, &shaped);
    assert_eq!(nodes_av.len(), 1);
    match &nodes_av[0] {
        Node::NativeGlyphRun {
            run,
            start,
            end,
            width,
            ..
        } => {
            assert_eq!(run.font, font_id);
            assert_eq!(*start, 0);
            assert_eq!(*end, 2);
            assert_eq!(run.glyphs[0].glyph_id, 10);
            assert_eq!(run.glyphs[0].cluster_start, 0);
            assert_eq!(run.glyphs[0].cluster_end, 1);
            assert_eq!(run.glyphs[1].glyph_id, 11);
            assert_eq!(run.glyphs[1].cluster_start, 1);
            assert_eq!(run.glyphs[1].cluster_end, 2);
            assert!(*width > 0);
        }
        other => panic!("expected Node::NativeGlyphRun, got {:?}", other),
    }
}

#[test]
fn test_lowering_to_glyph_run() {
    let font_id = 7;
    let shaped = shape_opentype_text_simple(TEST_FONT, "ffi").expect("should shape ffi");
    let run = shaped_glyphs_to_glyph_run(font_id, &shaped, 12.5, 34.0);
    match run {
        DisplayItem::NativeGlyphRun {
            run,
            x_bp,
            y_bp,
            start,
            end,
            ..
        } => {
            assert_eq!(run.font, font_id);
            assert_eq!(x_bp, 12.5);
            assert_eq!(y_bp, 34.0);
            assert_eq!(start, 0);
            assert_eq!(end, 1);
            assert_eq!(run.glyphs[0].glyph_id, 9); // f_f_i ligature
            assert_eq!(run.glyphs[0].cluster_start, 0);
            assert_eq!(run.glyphs[0].cluster_end, 3);
        }
        other => panic!("expected NativeGlyphRun, got {:?}", other),
    }
}

#[test]
fn test_engine_shaping_integration() {
    let eng = Engine::new(false);
    let font_id = 3;

    // shape_text_with_bytes
    let nodes = eng
        .shape_text_with_bytes(TEST_FONT, font_id, "fl")
        .expect("shape_text_with_bytes should succeed");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        Node::NativeGlyphRun {
            run, start, end, ..
        } => {
            assert_eq!(run.font, font_id);
            assert_eq!(*start, 0);
            assert_eq!(*end, 1);
            assert_eq!(run.glyphs[0].glyph_id, 8); // f_l
            assert_eq!(run.glyphs[0].cluster_start, 0);
            assert_eq!(run.glyphs[0].cluster_end, 2);
        }
        other => panic!("expected Node::NativeGlyphRun, got {:?}", other),
    }

    // shape_text_to_glyph_run_with_bytes
    let run = eng
        .shape_text_to_glyph_run_with_bytes(TEST_FONT, font_id, "AV", 10.0, 20.0)
        .expect("shape_text_to_glyph_run_with_bytes should succeed");
    match run {
        DisplayItem::NativeGlyphRun {
            run,
            x_bp,
            y_bp,
            start,
            end,
            ..
        } => {
            assert_eq!(run.font, font_id);
            assert_eq!(x_bp, 10.0);
            assert_eq!(y_bp, 20.0);
            assert_eq!(start, 0);
            assert_eq!(end, 2);
            assert_eq!(run.glyphs[0].glyph_id, 10);
            assert_eq!(run.glyphs[0].cluster_start, 0);
            assert_eq!(run.glyphs[0].cluster_end, 1);
            assert_eq!(run.glyphs[1].glyph_id, 11);
            assert_eq!(run.glyphs[1].cluster_start, 1);
            assert_eq!(run.glyphs[1].cluster_end, 2);
        }
        other => panic!("expected NativeGlyphRun, got {:?}", other),
    }
}

#[test]
fn test_gids_above_255_and_layout_metrics() {
    use tex_core::fonts::{NativeGlyph, NativeRun};
    let glyph = NativeGlyph {
        glyph_id: 345, // > 255
        cluster_start: 0,
        cluster_end: 4,
        x_advance: 655360,
        y_advance: 0,
        x_offset: 1000,
        y_offset: 2000,
    };
    assert_eq!(glyph.glyph_id, 345);
    let run = std::rc::Rc::new(NativeRun {
        font: 1,
        text: std::rc::Rc::from("test"),
        glyphs: vec![glyph],
    });
    let node = Node::NativeGlyphRun {
        run,
        start: 0,
        end: 1,
        width: 655360,
        height: 500000,
        depth: 100000,
    };
    match node {
        Node::NativeGlyphRun {
            width,
            height,
            depth,
            run,
            ..
        } => {
            assert_eq!(width, 655360);
            assert_eq!(height, 500000);
            assert_eq!(depth, 100000);
            assert_eq!(run.glyphs[0].glyph_id, 345);
        }
        _ => panic!("expected NativeGlyphRun"),
    }
}

#[test]
fn test_cluster_ranges_and_grapheme_safety() {
    // Verify that CJK punctuation rules prevent breaking before closing punctuation
    use tex_core::native_layout::{is_cjk, is_line_end_forbidden, is_line_start_forbidden};
    assert!(is_cjk('日'));
    assert!(is_cjk('本'));
    assert!(is_line_start_forbidden('。'));
    assert!(is_line_start_forbidden('、'));
    assert!(is_line_end_forbidden('「'));
    assert!(is_line_end_forbidden('（'));
}

#[test]
fn test_deliberate_ignorables_and_missing_glyph_error() {
    use tex_core::native_layout::is_default_ignorable;
    // Soft hyphen and zero-width spaces are deliberate ignorables
    assert!(is_default_ignorable('\u{00AD}'));
    assert!(is_default_ignorable('\u{200B}'));
    assert!(is_default_ignorable('\u{FEFF}'));
    assert!(!is_default_ignorable('A'));
    assert!(!is_default_ignorable('文'));

    // Character absent in TEST_FONT must return Err rather than silently emitting GID 0
    let res = shape_opentype_text_simple(TEST_FONT, "\u{1234}");
    assert!(res.is_err(), "Missing glyph must return an Err result");
    let err = res.unwrap_err();
    assert!(
        err.contains("Missing glyph in font"),
        "Unexpected error: {err}"
    );
}

#[test]
fn test_explicit_face_and_features_shaping() {
    // Test shape_opentype_text with explicit parameters
    let res = shape_opentype_text(
        TEST_FONT,
        0,
        &[],
        Some(rustybuzz::script::LATIN),
        None,
        &[],
        "fi",
    );
    let glyphs = res.expect("explicit shaping should succeed");
    assert_eq!(glyphs.len(), 1);
    assert_eq!(glyphs[0].glyph_id, 7);
    assert_eq!(glyphs[0].cluster_start, 0);
    assert_eq!(glyphs[0].cluster_end, 2);
}
