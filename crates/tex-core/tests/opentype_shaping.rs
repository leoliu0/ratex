use tex_core::boxes::{DisplayItem, Node};
use tex_core::engine::Engine;
use tex_core::fonts::{
    shape_opentype_text, shape_opentype_to_nodes,
    shaped_glyphs_to_glyph_run, shaped_glyphs_to_nodes,
};

const TEST_FONT: &[u8] = include_bytes!("fixtures/ratex_test_font.ttf");

#[test]
fn test_shaping_invalid_font() {
    let shaped = shape_opentype_text(b"corrupt font data", "hello");
    assert!(shaped.is_empty());
}

#[test]
fn test_shaping_empty_input() {
    let shaped = shape_opentype_text(TEST_FONT, "");
    assert!(shaped.is_empty());
}

#[test]
fn test_opentype_ligature_formation() {
    // "fi" -> single ligature glyph f_i (id 7, advance 580)
    let shaped_fi = shape_opentype_text(TEST_FONT, "fi");
    assert_eq!(shaped_fi.len(), 1, "fi must merge into one ligature glyph");
    assert_eq!(shaped_fi[0].glyph_id, 7);
    assert_eq!(shaped_fi[0].cluster, 0);
    assert_eq!(shaped_fi[0].x_advance, 580);
    assert_eq!(shaped_fi[0].y_advance, 0);
    assert_eq!(shaped_fi[0].x_offset, 0);
    assert_eq!(shaped_fi[0].y_offset, 0);

    // "fl" -> single ligature glyph f_l (id 8, advance 580)
    let shaped_fl = shape_opentype_text(TEST_FONT, "fl");
    assert_eq!(shaped_fl.len(), 1);
    assert_eq!(shaped_fl[0].glyph_id, 8);
    assert_eq!(shaped_fl[0].cluster, 0);
    assert_eq!(shaped_fl[0].x_advance, 580);

    // "ffi" -> single ligature glyph f_f_i (id 9, advance 850)
    let shaped_ffi = shape_opentype_text(TEST_FONT, "ffi");
    assert_eq!(shaped_ffi.len(), 1);
    assert_eq!(shaped_ffi[0].glyph_id, 9);
    assert_eq!(shaped_ffi[0].cluster, 0);
    assert_eq!(shaped_ffi[0].x_advance, 850);
}

#[test]
fn test_opentype_advance_positioning_and_kerning() {
    // "AV" kerns by -80: A (id 10) gets 600 - 80 = 520, V (id 11) gets 600
    let shaped_av = shape_opentype_text(TEST_FONT, "AV");
    assert_eq!(shaped_av.len(), 2);
    assert_eq!(shaped_av[0].glyph_id, 10);
    assert_eq!(shaped_av[0].cluster, 0);
    assert_eq!(shaped_av[0].x_advance, 520);
    assert_eq!(shaped_av[1].glyph_id, 11);
    assert_eq!(shaped_av[1].cluster, 1);
    assert_eq!(shaped_av[1].x_advance, 600);

    // Separated letters with spaces should not form ligatures
    let shaped_sep = shape_opentype_text(TEST_FONT, "f i");
    assert_eq!(shaped_sep.len(), 3);
    assert_eq!(shaped_sep[0].glyph_id, 4); // f
    assert_eq!(shaped_sep[0].x_advance, 350);
    assert_eq!(shaped_sep[1].glyph_id, 3); // space
    assert_eq!(shaped_sep[1].x_advance, 250);
    assert_eq!(shaped_sep[2].glyph_id, 5); // i
    assert_eq!(shaped_sep[2].x_advance, 280);
}

#[test]
fn test_lowering_to_nodes() {
    let font_id = 5;
    let nodes = shape_opentype_to_nodes(TEST_FONT, font_id, "fi");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        Node::Char { c, font } => {
            assert_eq!(*c, 7);
            assert_eq!(*font, font_id);
        }
        other => panic!("expected Node::Char, got {:?}", other),
    }

    // Direct conversion of shaped glyphs with offsets
    let shaped = shape_opentype_text(TEST_FONT, "AV");
    let nodes_av = shaped_glyphs_to_nodes(font_id, &shaped);
    assert_eq!(nodes_av.len(), 2);
    match (&nodes_av[0], &nodes_av[1]) {
        (Node::Char { c: c1, font: f1 }, Node::Char { c: c2, font: f2 }) => {
            assert_eq!(*c1, 10);
            assert_eq!(*f1, font_id);
            assert_eq!(*c2, 11);
            assert_eq!(*f2, font_id);
        }
        other => panic!("expected two Node::Char nodes, got {:?}", other),
    }
}

#[test]
fn test_lowering_to_glyph_run() {
    let font_id = 7;
    let shaped = shape_opentype_text(TEST_FONT, "ffi");
    let run = shaped_glyphs_to_glyph_run(font_id, &shaped, 12.5, 34.0);
    match run {
        DisplayItem::GlyphRun {
            font,
            x_bp,
            y_bp,
            glyphs,
            ..
        } => {
            assert_eq!(font, font_id);
            assert_eq!(x_bp, 12.5);
            assert_eq!(y_bp, 34.0);
            assert_eq!(glyphs, vec![9]); // f_f_i ligature
        }
        other => panic!("expected GlyphRun, got {:?}", other),
    }
}

#[test]
fn test_engine_shaping_integration() {
    let eng = Engine::new(false);
    let font_id = 3;

    // shape_text_with_bytes
    let nodes = eng.shape_text_with_bytes(TEST_FONT, font_id, "fl");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        Node::Char { c, font } => {
            assert_eq!(*c, 8); // f_l
            assert_eq!(*font, font_id);
        }
        other => panic!("expected Node::Char, got {:?}", other),
    }

    // shape_text_to_glyph_run_with_bytes
    let run = eng.shape_text_to_glyph_run_with_bytes(TEST_FONT, font_id, "AV", 10.0, 20.0);
    match run {
        DisplayItem::GlyphRun {
            font,
            x_bp,
            y_bp,
            glyphs,
            ..
        } => {
            assert_eq!(font, font_id);
            assert_eq!(x_bp, 10.0);
            assert_eq!(y_bp, 20.0);
            assert_eq!(glyphs, vec![10, 11]);
        }
        other => panic!("expected GlyphRun, got {:?}", other),
    }

    // fallback when font data not found: returns raw character bytes
    let fallback_nodes = eng.shape_text(999, "hello");
    assert_eq!(fallback_nodes.len(), 5);
    for (node, expected_b) in fallback_nodes.iter().zip(b"hello".iter()) {
        match node {
            Node::Char { c, font } => {
                assert_eq!(*c, *expected_b);
                assert_eq!(*font, 999);
            }
            other => panic!("expected Node::Char, got {:?}", other),
        }
    }
}
