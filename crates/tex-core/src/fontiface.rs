//! Engine implementation of FontResolver plus small dispatch shims and
//! ligature step types.

use crate::engine::Engine;
use crate::fonts::FontResolver;
use crate::prim::Prim;
use crate::tfm::FontId;

pub struct LigKernStep {
    pub is_kern: bool,
    pub kern_amount: i32,
    pub lig_char: u8,
    pub keep_left: bool,
    pub keep_right: bool,
    pub iterate: bool,
}

/// A shaped glyph produced by HarfBuzz/rustybuzz OpenType shaping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShapedGlyph {
    /// OpenType glyph ID in the font.
    pub glyph_id: u32,
    /// Byte/character cluster start index in the source text.
    pub cluster: u32,
    /// Byte start offset of this glyph's cluster in the source text.
    pub cluster_start: u32,
    /// Byte end offset of this glyph's cluster in the source text.
    pub cluster_end: u32,
    /// Horizontal advance in font design units.
    pub x_advance: i32,
    /// Vertical advance in font design units.
    pub y_advance: i32,
    /// Horizontal glyph offset relative to pen position in font design units.
    pub x_offset: i32,
    /// Vertical glyph offset relative to pen position in font design units.
    pub y_offset: i32,
    /// True if HarfBuzz marked this glyph as unsafe to break.
    pub unsafe_to_break: bool,
}

/// Shape UTF-8 text with explicit face index, variation coordinates, OpenType features,
/// script, and language tags.
///
/// Returns a sequence of shaped glyphs with real glyph IDs, cluster ranges,
/// advances, and offsets. Returns Err if font parsing fails or on missing glyphs.
pub fn shape_opentype_text(
    font_bytes: &[u8],
    face_index: u32,
    variations: &[(ttf_parser::Tag, f32)],
    script: Option<rustybuzz::Script>,
    language: Option<rustybuzz::Language>,
    features: &[rustybuzz::Feature],
    text: &str,
) -> Result<Vec<ShapedGlyph>, String> {
    if font_bytes.is_empty() {
        return Err("Font bytes are empty".to_string());
    }
    let mut face = rustybuzz::Face::from_slice(font_bytes, face_index)
        .ok_or_else(|| "Failed to parse OpenType face from font bytes".to_string())?;

    if !variations.is_empty() {
        let vars: Vec<rustybuzz::Variation> = variations
            .iter()
            .map(|&(tag, value)| rustybuzz::Variation { tag, value })
            .collect();
        face.set_variations(&vars);
    }

    if text.is_empty() {
        return Ok(Vec::new());
    }

    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    if let Some(s) = script {
        buffer.set_script(s);
    }
    if let Some(l) = language {
        buffer.set_language(l);
    }

    let glyph_buffer = rustybuzz::shape(&face, features, buffer);
    let infos = glyph_buffer.glyph_infos();
    let positions = glyph_buffer.glyph_positions();

    let mut shaped_glyphs = Vec::with_capacity(infos.len());
    for (i, (info, pos)) in infos.iter().zip(positions.iter()).enumerate() {
        let cluster_start = info.cluster;
        let next_cluster = (i + 1..infos.len())
            .find(|&j| infos[j].cluster != info.cluster)
            .map(|j| infos[j].cluster);
        let cluster_end = if let Some(nc) = next_cluster {
            nc
        } else {
            text.len() as u32
        };
        let cluster_end = cluster_end.max(if (cluster_start as usize) < text.len() {
            let ch_len = text[cluster_start as usize..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
            cluster_start + ch_len as u32
        } else {
            cluster_start
        });

        // Check for GID 0 on visible printable text
        if info.glyph_id == 0 {
            let start = cluster_start as usize;
            let end = (cluster_end as usize).min(text.len());
            let char_slice = if start < end { &text[start..end] } else { "" };
            let is_ignorable = char_slice
                .chars()
                .all(|c| crate::native_layout::is_default_ignorable(c) || c.is_whitespace());
            if !is_ignorable {
                let first_char = char_slice.chars().next().unwrap_or('?');
                return Err(format!(
                    "Missing glyph in font for character '{}' (U+{:04X})",
                    first_char, first_char as u32
                ));
            }
        }

        shaped_glyphs.push(ShapedGlyph {
            glyph_id: info.glyph_id,
            cluster: cluster_start,
            cluster_start,
            cluster_end,
            x_advance: pos.x_advance,
            y_advance: pos.y_advance,
            x_offset: pos.x_offset,
            y_offset: pos.y_offset,
            unsafe_to_break: info.unsafe_to_break(),
        });
    }

    Ok(shaped_glyphs)
}

/// Shape UTF-8 text with explicit face index, variation coordinates, OpenType features,
/// script, and language tags.
pub fn shape_opentype_text_full(
    font_bytes: &[u8],
    face_index: u32,
    variations: &[(ttf_parser::Tag, f32)],
    script: Option<rustybuzz::Script>,
    language: Option<rustybuzz::Language>,
    features: &[rustybuzz::Feature],
    text: &str,
) -> Result<Vec<ShapedGlyph>, String> {
    shape_opentype_text(
        font_bytes, face_index, variations, script, language, features, text,
    )
}

/// Convenience wrapper for shaping UTF-8 text with default face index 0 and no variations/features.
pub fn shape_opentype_text_simple(
    font_bytes: &[u8],
    text: &str,
) -> Result<Vec<ShapedGlyph>, String> {
    shape_opentype_text(font_bytes, 0, &[], None, None, &[], text)
}

/// Lower a sequence of shaped glyphs into layout `Node::NativeGlyphRun`.
pub fn shaped_glyphs_to_nodes(font_id: FontId, glyphs: &[ShapedGlyph]) -> Vec<crate::boxes::Node> {
    shaped_glyphs_to_nodes_scaled(font_id, glyphs, "", 655360, 1000)
}

/// Lower a sequence of shaped glyphs into layout `Node::NativeGlyphRun` with specified scaling.
pub fn shaped_glyphs_to_nodes_scaled(
    font_id: FontId,
    glyphs: &[ShapedGlyph],
    text: &str,
    at_size: i32,
    upem: u16,
) -> Vec<crate::boxes::Node> {
    if glyphs.is_empty() {
        return Vec::new();
    }
    let upem = upem.max(1) as i64;
    let scale_to_sp = |val: i32| -> i32 { (val as i64 * at_size as i64 / upem) as i32 };
    let native_glyphs: Vec<crate::native_layout::NativeGlyph> = glyphs
        .iter()
        .map(|g| crate::native_layout::NativeGlyph {
            glyph_id: g.glyph_id as u16,
            cluster_start: g.cluster_start,
            cluster_end: g.cluster_end,
            x_advance: scale_to_sp(g.x_advance),
            y_advance: scale_to_sp(g.y_advance),
            x_offset: scale_to_sp(g.x_offset),
            y_offset: scale_to_sp(g.y_offset),
        })
        .collect();
    let total = native_glyphs.len();
    let total_w: i32 = native_glyphs.iter().map(|g| g.x_advance).sum();
    let run = std::rc::Rc::new(crate::native_layout::NativeRun {
        font: font_id,
        text: std::rc::Rc::from(text),
        glyphs: native_glyphs,
    });
    vec![crate::boxes::Node::NativeGlyphRun {
        run,
        start: 0,
        end: total,
        width: total_w,
        height: (at_size * 8) / 10,
        depth: (at_size * 2) / 10,
    }]
}

/// Lower a sequence of shaped glyphs into a `DisplayItem::NativeGlyphRun`.
pub fn shaped_glyphs_to_glyph_run(
    font_id: FontId,
    glyphs: &[ShapedGlyph],
    x_bp: f64,
    y_bp: f64,
) -> crate::boxes::DisplayItem {
    shaped_glyphs_to_native_display_item(font_id, glyphs, "", 655360, 1000, x_bp, y_bp)
}

/// Lower a sequence of shaped glyphs into a `DisplayItem::NativeGlyphRun` with specified scaling.
pub fn shaped_glyphs_to_native_display_item(
    font_id: FontId,
    glyphs: &[ShapedGlyph],
    text: &str,
    at_size: i32,
    upem: u16,
    x_bp: f64,
    y_bp: f64,
) -> crate::boxes::DisplayItem {
    let upem = upem.max(1) as i64;
    let scale_to_sp = |val: i32| -> i32 { (val as i64 * at_size as i64 / upem) as i32 };
    let native_glyphs: Vec<crate::native_layout::NativeGlyph> = glyphs
        .iter()
        .map(|g| crate::native_layout::NativeGlyph {
            glyph_id: g.glyph_id as u16,
            cluster_start: g.cluster_start,
            cluster_end: g.cluster_end,
            x_advance: scale_to_sp(g.x_advance),
            y_advance: scale_to_sp(g.y_advance),
            x_offset: scale_to_sp(g.x_offset),
            y_offset: scale_to_sp(g.y_offset),
        })
        .collect();
    let total = native_glyphs.len();
    let run = std::rc::Rc::new(crate::native_layout::NativeRun {
        font: font_id,
        text: std::rc::Rc::from(text),
        glyphs: native_glyphs,
    });
    crate::boxes::DisplayItem::NativeGlyphRun {
        run,
        start: 0,
        end: total,
        x_bp,
        y_bp,
        tag: None,
        span: None,
        source_file_id: 0,
        source_line: 0,
    }
}

/// Lower shaped OpenType text directly into layout nodes.
pub fn shape_opentype_to_nodes(
    font_bytes: &[u8],
    font_id: FontId,
    text: &str,
) -> Result<Vec<crate::boxes::Node>, String> {
    shape_opentype_to_nodes_full(font_bytes, 0, &[], None, None, &[], font_id, text, 655360)
}

/// Lower shaped OpenType text with explicit parameters directly into layout nodes.
pub fn shape_opentype_to_nodes_full(
    font_bytes: &[u8],
    face_index: u32,
    variations: &[(ttf_parser::Tag, f32)],
    script: Option<rustybuzz::Script>,
    language: Option<rustybuzz::Language>,
    features: &[rustybuzz::Feature],
    font_id: FontId,
    text: &str,
    at_size: i32,
) -> Result<Vec<crate::boxes::Node>, String> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let face = ttf_parser::Face::parse(font_bytes, face_index)
        .map_err(|e| format!("Failed to parse font: {e:?}"))?;
    let upem = face.units_per_em();
    let glyphs = shape_opentype_text(
        font_bytes, face_index, variations, script, language, features, text,
    )?;
    let scale_to_sp = |val: i32| -> i32 { (val as i64 * at_size as i64 / upem as i64) as i32 };
    let native_glyphs: Vec<crate::native_layout::NativeGlyph> = glyphs
        .iter()
        .map(|g| crate::native_layout::NativeGlyph {
            glyph_id: g.glyph_id as u16,
            cluster_start: g.cluster_start,
            cluster_end: g.cluster_end,
            x_advance: scale_to_sp(g.x_advance),
            y_advance: scale_to_sp(g.y_advance),
            x_offset: scale_to_sp(g.x_offset),
            y_offset: scale_to_sp(g.y_offset),
        })
        .collect();
    let (w, h, d) =
        crate::native_layout::calculate_slice_dims(&native_glyphs, &face, at_size, upem as i64);
    let total = native_glyphs.len();
    let run = std::rc::Rc::new(crate::native_layout::NativeRun {
        font: font_id,
        text: std::rc::Rc::from(text),
        glyphs: native_glyphs,
    });
    Ok(vec![crate::boxes::Node::NativeGlyphRun {
        run,
        start: 0,
        end: total,
        width: w,
        height: h,
        depth: d,
    }])
}

/// Lower shaped OpenType text directly into a `DisplayItem::NativeGlyphRun`.
pub fn shape_opentype_to_glyph_run(
    font_bytes: &[u8],
    font_id: FontId,
    text: &str,
    x_bp: f64,
    y_bp: f64,
) -> Result<crate::boxes::DisplayItem, String> {
    shape_opentype_to_glyph_run_full(
        font_bytes,
        0,
        &[],
        None,
        None,
        &[],
        font_id,
        text,
        655360,
        x_bp,
        y_bp,
    )
}

/// Lower shaped OpenType text with explicit parameters directly into a `DisplayItem::NativeGlyphRun`.
pub fn shape_opentype_to_glyph_run_full(
    font_bytes: &[u8],
    face_index: u32,
    variations: &[(ttf_parser::Tag, f32)],
    script: Option<rustybuzz::Script>,
    language: Option<rustybuzz::Language>,
    features: &[rustybuzz::Feature],
    font_id: FontId,
    text: &str,
    at_size: i32,
    x_bp: f64,
    y_bp: f64,
) -> Result<crate::boxes::DisplayItem, String> {
    let face = ttf_parser::Face::parse(font_bytes, face_index)
        .map_err(|e| format!("Failed to parse font: {e:?}"))?;
    let upem = face.units_per_em();
    let glyphs = shape_opentype_text(
        font_bytes, face_index, variations, script, language, features, text,
    )?;
    Ok(shaped_glyphs_to_native_display_item(
        font_id, &glyphs, text, at_size, upem, x_bp, y_bp,
    ))
}
impl FontResolver for Engine {
    fn char_width(&self, f: FontId, c: u8) -> i32 {
        self.eqtb
            .fonts
            .get(f as usize)
            .map(|f| f.char_width(c))
            .unwrap_or(0)
    }
    fn char_height(&self, f: FontId, c: u8) -> i32 {
        self.eqtb
            .fonts
            .get(f as usize)
            .map(|f| f.char_height(c))
            .unwrap_or(0)
    }
    fn char_depth(&self, f: FontId, c: u8) -> i32 {
        self.eqtb
            .fonts
            .get(f as usize)
            .map(|f| f.char_depth(c))
            .unwrap_or(0)
    }
    fn char_italic(&self, f: FontId, c: u8) -> i32 {
        self.eqtb
            .fonts
            .get(f as usize)
            .map(|f| f.char_italic(c))
            .unwrap_or(0)
    }
}

impl Engine {
    /// public wrapper for expansion dispatch from active chars
    pub fn expand_prim_pub(
        &mut self,
        p: Prim,
        id: crate::token::CsId,
    ) -> Option<crate::token::Token> {
        self.expand_prim(p, id)
    }

    /// dispatch a primitive from non-main-loop contexts
    pub fn dispatch_cs(&mut self, p: Prim, id: crate::token::CsId) {
        if !self.try_assignment(p, id) {
            self.main_dispatch(p, id);
        }
    }

    /// Shape text using the font's OpenType data if available, lowering shaped glyphs
    /// into layout `Node`s (`Node::Char` with `Node::Kern` for positioning offsets).
    /// If no OpenType font data is found or shaping yields no glyphs, falls back
    /// to emitting individual `Node::Char` nodes for each character.
    /// Shape text using the font's OpenType data if available, lowering shaped glyphs
    /// into layout `Node`s. Returns Err on missing glyphs or if font is not native.
    pub fn shape_text(
        &self,
        font_id: FontId,
        text: &str,
    ) -> Result<Vec<crate::boxes::Node>, String> {
        let native_font = self
            .font_loader
            .native_fonts
            .get(&font_id)
            .cloned()
            .ok_or_else(|| format!("Font {font_id} is not a registered native font"))?;
        self.shape_native_run_nodes(font_id, &native_font, text)
    }

    /// Shape text using explicitly provided font bytes, lowering into layout nodes.
    pub fn shape_text_with_bytes(
        &self,
        font_bytes: &[u8],
        font_id: FontId,
        text: &str,
    ) -> Result<Vec<crate::boxes::Node>, String> {
        let at_size = self
            .eqtb
            .fonts
            .get(font_id as usize)
            .map(|f| f.at_size)
            .unwrap_or(655360);
        shape_opentype_to_nodes_full(font_bytes, 0, &[], None, None, &[], font_id, text, at_size)
    }

    /// Shape text using the font's OpenType data if available, lowering into a `DisplayItem::NativeGlyphRun`.
    pub fn shape_text_to_glyph_run(
        &self,
        font_id: FontId,
        text: &str,
        x_bp: f64,
        y_bp: f64,
    ) -> Result<crate::boxes::DisplayItem, String> {
        let native_font = self
            .font_loader
            .native_fonts
            .get(&font_id)
            .cloned()
            .ok_or_else(|| format!("Font {font_id} is not a registered native font"))?;
        let at_size = self
            .eqtb
            .fonts
            .get(font_id as usize)
            .map(|f| f.at_size)
            .unwrap_or(655360);
        let upem = native_font.program.units_per_em;
        let glyphs = shape_opentype_text(
            &native_font.program.data,
            native_font.program.face_index,
            &native_font.program.variations,
            native_font.script,
            native_font.language.clone(),
            &native_font.features,
            text,
        )?;
        Ok(shaped_glyphs_to_native_display_item(
            font_id, &glyphs, text, at_size, upem, x_bp, y_bp,
        ))
    }

    /// Shape text using explicitly provided font bytes, lowering into a `DisplayItem::NativeGlyphRun`.
    pub fn shape_text_to_glyph_run_with_bytes(
        &self,
        font_bytes: &[u8],
        font_id: FontId,
        text: &str,
        x_bp: f64,
        y_bp: f64,
    ) -> Result<crate::boxes::DisplayItem, String> {
        let at_size = self
            .eqtb
            .fonts
            .get(font_id as usize)
            .map(|f| f.at_size)
            .unwrap_or(655360);
        shape_opentype_to_glyph_run_full(
            font_bytes,
            0,
            &[],
            None,
            None,
            &[],
            font_id,
            text,
            at_size,
            x_bp,
            y_bp,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FONT: &[u8] = include_bytes!("../tests/fixtures/ratex_test_font.ttf");

    #[test]
    fn test_shape_invalid_font_bytes() {
        let res = shape_opentype_text_simple(b"not a valid font", "hello");
        assert!(res.is_err(), "Invalid font bytes must return Err");
    }

    #[test]
    fn test_shape_empty_text() {
        let shaped = shape_opentype_text_simple(TEST_FONT, "").expect("Empty text should succeed");
        assert!(shaped.is_empty());
    }

    #[test]
    fn test_harfbuzz_rustybuzz_ligatures() {
        // "fi" should be merged into a single ligature glyph `f_i` (glyph id 7)
        let shaped_fi =
            shape_opentype_text_simple(TEST_FONT, "fi").expect("shaping fi should succeed");
        assert_eq!(shaped_fi.len(), 1, "fi should form a single ligature glyph");
        assert_eq!(shaped_fi[0].glyph_id, 7);
        assert_eq!(shaped_fi[0].cluster_start, 0);
        assert_eq!(shaped_fi[0].cluster_end, 2);
        assert_eq!(shaped_fi[0].x_advance, 580);
        assert_eq!(shaped_fi[0].y_advance, 0);

        // "fl" should form `f_l` (glyph id 8)
        let shaped_fl =
            shape_opentype_text_simple(TEST_FONT, "fl").expect("shaping fl should succeed");
        assert_eq!(shaped_fl.len(), 1, "fl should form a single ligature glyph");
        assert_eq!(shaped_fl[0].glyph_id, 8);
        assert_eq!(shaped_fl[0].cluster_start, 0);
        assert_eq!(shaped_fl[0].cluster_end, 2);
        assert_eq!(shaped_fl[0].x_advance, 580);

        // "ffi" should form `f_f_i` (glyph id 9)
        let shaped_ffi =
            shape_opentype_text_simple(TEST_FONT, "ffi").expect("shaping ffi should succeed");
        assert_eq!(
            shaped_ffi.len(),
            1,
            "ffi should form a single ligature glyph"
        );
        assert_eq!(shaped_ffi[0].glyph_id, 9);
        assert_eq!(shaped_ffi[0].cluster_start, 0);
        assert_eq!(shaped_ffi[0].cluster_end, 3);
        assert_eq!(shaped_ffi[0].x_advance, 850);
    }

    #[test]
    fn test_harfbuzz_rustybuzz_advance_positioning_and_kerning() {
        // "AV" has a kern of -80 in the test font: 'A' advance is 600 - 80 = 520, 'V' is 600.
        let shaped_av =
            shape_opentype_text_simple(TEST_FONT, "AV").expect("shaping AV should succeed");
        assert_eq!(shaped_av.len(), 2);
        assert_eq!(shaped_av[0].glyph_id, 10); // 'A'
        assert_eq!(shaped_av[0].cluster_start, 0);
        assert_eq!(shaped_av[0].cluster_end, 1);
        assert_eq!(
            shaped_av[0].x_advance, 520,
            "advance of A should be kerned by -80"
        );

        assert_eq!(shaped_av[1].glyph_id, 11); // 'V'
        assert_eq!(shaped_av[1].cluster_start, 1);
        assert_eq!(shaped_av[1].cluster_end, 2);
        assert_eq!(shaped_av[1].x_advance, 600);

        // "f i" with space should not form a ligature: 3 separate glyphs
        let shaped_sep =
            shape_opentype_text_simple(TEST_FONT, "f i").expect("shaping 'f i' should succeed");
        assert_eq!(shaped_sep.len(), 3);
        assert_eq!(shaped_sep[0].glyph_id, 4); // 'f'
        assert_eq!(shaped_sep[0].cluster_start, 0);
        assert_eq!(shaped_sep[0].cluster_end, 1);
        assert_eq!(shaped_sep[0].x_advance, 350);
        assert_eq!(shaped_sep[1].glyph_id, 3); // ' '
        assert_eq!(shaped_sep[1].cluster_start, 1);
        assert_eq!(shaped_sep[1].cluster_end, 2);
        assert_eq!(shaped_sep[1].x_advance, 250);
        assert_eq!(shaped_sep[2].glyph_id, 5); // 'i'
        assert_eq!(shaped_sep[2].cluster_start, 2);
        assert_eq!(shaped_sep[2].cluster_end, 3);
        assert_eq!(shaped_sep[2].x_advance, 280);
    }

    #[test]
    fn test_shape_to_nodes_and_glyph_run() {
        let font_id = 42;
        let nodes =
            shape_opentype_to_nodes(TEST_FONT, font_id, "fi").expect("should lower to nodes");
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            crate::boxes::Node::NativeGlyphRun {
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

        let run = shape_opentype_to_glyph_run(TEST_FONT, font_id, "fi", 10.0, 20.0)
            .expect("glyph run should be produced");
        match run {
            crate::boxes::DisplayItem::NativeGlyphRun {
                run,
                start,
                end,
                x_bp,
                y_bp,
                ..
            } => {
                assert_eq!(run.font, font_id);
                assert_eq!(x_bp, 10.0);
                assert_eq!(y_bp, 20.0);
                assert_eq!(start, 0);
                assert_eq!(end, 1);
                assert_eq!(run.glyphs[0].glyph_id, 7);
                assert_eq!(run.glyphs[0].cluster_start, 0);
                assert_eq!(run.glyphs[0].cluster_end, 2);
            }
            other => panic!("expected NativeGlyphRun, got {:?}", other),
        }
    }

    #[test]
    fn test_engine_shape_helpers() {
        let eng = Engine::new(false);
        let font_id = 1;
        let nodes = eng
            .shape_text_with_bytes(TEST_FONT, font_id, "AV")
            .expect("shape_text_with_bytes should succeed");
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            crate::boxes::Node::NativeGlyphRun {
                run, start, end, ..
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
            }
            other => panic!("expected Node::NativeGlyphRun, got {:?}", other),
        }

        let run = eng
            .shape_text_to_glyph_run_with_bytes(TEST_FONT, font_id, "ffi", 5.0, 15.0)
            .expect("shape_text_to_glyph_run_with_bytes should succeed");
        match run {
            crate::boxes::DisplayItem::NativeGlyphRun {
                run,
                x_bp,
                y_bp,
                start,
                end,
                ..
            } => {
                assert_eq!(run.font, font_id);
                assert_eq!(x_bp, 5.0);
                assert_eq!(y_bp, 15.0);
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
    fn test_missing_glyph_error() {
        // U+1234 is not present in TEST_FONT
        let res = shape_opentype_text_simple(TEST_FONT, "\u{1234}");
        assert!(res.is_err(), "Missing glyph must return Err");
        let err = res.unwrap_err();
        assert!(err.contains("Missing glyph in font"), "Error was: {err}");
    }
}
