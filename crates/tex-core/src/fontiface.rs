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
    /// Byte/character cluster index in the source text.
    pub cluster: u32,
    /// Horizontal advance in font design units.
    pub x_advance: i32,
    /// Vertical advance in font design units.
    pub y_advance: i32,
    /// Horizontal glyph offset relative to pen position in font design units.
    pub x_offset: i32,
    /// Vertical glyph offset relative to pen position in font design units.
    pub y_offset: i32,
}

/// Shape UTF-8 text using rustybuzz (HarfBuzz) and OpenType font bytes.
///
/// Returns a sequence of shaped glyphs with glyph IDs, cluster indices,
/// advances, and offsets. If font parsing fails or font bytes are invalid,
/// returns an empty vector.
pub fn shape_opentype_text(font_bytes: &[u8], text: &str) -> Vec<ShapedGlyph> {
    let face = match rustybuzz::Face::from_slice(font_bytes, 0) {
        Some(face) => face,
        None => return Vec::new(),
    };
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    let glyph_buffer = rustybuzz::shape(&face, &[], buffer);
    let infos = glyph_buffer.glyph_infos();
    let positions = glyph_buffer.glyph_positions();
    infos
        .iter()
        .zip(positions.iter())
        .map(|(info, pos)| ShapedGlyph {
            glyph_id: info.glyph_id,
            cluster: info.cluster,
            x_advance: pos.x_advance,
            y_advance: pos.y_advance,
            x_offset: pos.x_offset,
            y_offset: pos.y_offset,
        })
        .collect()
}

/// Lower a sequence of shaped glyphs into layout `Node`s (`Node::Kern` for non-zero
/// horizontal offsets, followed by `Node::Char`).
pub fn shaped_glyphs_to_nodes(font_id: FontId, glyphs: &[ShapedGlyph]) -> Vec<crate::boxes::Node> {
    let mut nodes = Vec::with_capacity(glyphs.len() * 2);
    for g in glyphs {
        if g.x_offset != 0 {
            nodes.push(crate::boxes::Node::Kern(g.x_offset));
        }
        nodes.push(crate::boxes::Node::Char {
            c: g.glyph_id as u8,
            font: font_id,
        });
    }
    nodes
}

/// Lower a sequence of shaped glyphs into a `DisplayItem::GlyphRun`.
pub fn shaped_glyphs_to_glyph_run(
    font_id: FontId,
    glyphs: &[ShapedGlyph],
    x_bp: f64,
    y_bp: f64,
) -> crate::boxes::DisplayItem {
    crate::boxes::DisplayItem::GlyphRun {
        font: font_id,
        x_bp,
        y_bp,
        glyphs: glyphs.iter().map(|g| g.glyph_id as u8).collect(),
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
) -> Vec<crate::boxes::Node> {
    let shaped = shape_opentype_text(font_bytes, text);
    if !shaped.is_empty() {
        shaped_glyphs_to_nodes(font_id, &shaped)
    } else {
        text.bytes()
            .map(|b| crate::boxes::Node::Char {
                c: b,
                font: font_id,
            })
            .collect()
    }
}

/// Lower shaped OpenType text directly into a `DisplayItem::GlyphRun`.
pub fn shape_opentype_to_glyph_run(
    font_bytes: &[u8],
    font_id: FontId,
    text: &str,
    x_bp: f64,
    y_bp: f64,
) -> Option<crate::boxes::DisplayItem> {
    let shaped = shape_opentype_text(font_bytes, text);
    if !shaped.is_empty() {
        Some(shaped_glyphs_to_glyph_run(font_id, &shaped, x_bp, y_bp))
    } else {
        None
    }
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
    pub fn shape_text(&self, font_id: FontId, text: &str) -> Vec<crate::boxes::Node> {
        let font = self.eqtb.fonts.get(font_id as usize);
        let font_bytes = font.and_then(|f| {
            f.type1_path
                .as_ref()
                .and_then(|p| {
                    self.font_loader
                        .kpse
                        .read(p, tex_kpse::Format::Truetype)
                        .or_else(|| self.font_loader.kpse.read(p, tex_kpse::Format::Otf))
                        .or_else(|| self.font_loader.kpse.read(p, tex_kpse::Format::Type1))
                        .or_else(|| tex_kpse::fs::read(p).ok())
                })
                .or_else(|| {
                    f.map_fontname.as_ref().and_then(|m| {
                        self.font_loader
                            .kpse
                            .read(m, tex_kpse::Format::Truetype)
                            .or_else(|| self.font_loader.kpse.read(m, tex_kpse::Format::Otf))
                            .or_else(|| self.font_loader.kpse.read(m, tex_kpse::Format::Type1))
                            .or_else(|| tex_kpse::fs::read(m).ok())
                    })
                })
        });

        if let Some(bytes) = font_bytes {
            let shaped = shape_opentype_text(&bytes, text);
            if !shaped.is_empty() {
                return shaped_glyphs_to_nodes(font_id, &shaped);
            }
        }

        text.bytes()
            .map(|b| crate::boxes::Node::Char {
                c: b,
                font: font_id,
            })
            .collect()
    }

    /// Shape text using explicitly provided font bytes, lowering into layout nodes.
    pub fn shape_text_with_bytes(
        &self,
        font_bytes: &[u8],
        font_id: FontId,
        text: &str,
    ) -> Vec<crate::boxes::Node> {
        shape_opentype_to_nodes(font_bytes, font_id, text)
    }

    /// Shape text using the font's OpenType data if available, lowering into a `DisplayItem::GlyphRun`.
    pub fn shape_text_to_glyph_run(
        &self,
        font_id: FontId,
        text: &str,
        x_bp: f64,
        y_bp: f64,
    ) -> Option<crate::boxes::DisplayItem> {
        let font = self.eqtb.fonts.get(font_id as usize);
        let font_bytes = font.and_then(|f| {
            f.type1_path
                .as_ref()
                .and_then(|p| {
                    self.font_loader
                        .kpse
                        .read(p, tex_kpse::Format::Truetype)
                        .or_else(|| self.font_loader.kpse.read(p, tex_kpse::Format::Otf))
                        .or_else(|| self.font_loader.kpse.read(p, tex_kpse::Format::Type1))
                        .or_else(|| tex_kpse::fs::read(p).ok())
                })
                .or_else(|| {
                    f.map_fontname.as_ref().and_then(|m| {
                        self.font_loader
                            .kpse
                            .read(m, tex_kpse::Format::Truetype)
                            .or_else(|| self.font_loader.kpse.read(m, tex_kpse::Format::Otf))
                            .or_else(|| self.font_loader.kpse.read(m, tex_kpse::Format::Type1))
                            .or_else(|| tex_kpse::fs::read(m).ok())
                    })
                })
        });

        if let Some(bytes) = font_bytes {
            shape_opentype_to_glyph_run(&bytes, font_id, text, x_bp, y_bp)
        } else {
            None
        }
    }

    /// Shape text using explicitly provided font bytes, lowering into a `DisplayItem::GlyphRun`.
    pub fn shape_text_to_glyph_run_with_bytes(
        &self,
        font_bytes: &[u8],
        font_id: FontId,
        text: &str,
        x_bp: f64,
        y_bp: f64,
    ) -> crate::boxes::DisplayItem {
        shape_opentype_to_glyph_run(font_bytes, font_id, text, x_bp, y_bp).unwrap_or_else(|| {
            crate::boxes::DisplayItem::GlyphRun {
                font: font_id,
                x_bp,
                y_bp,
                glyphs: text.bytes().collect(),
                tag: None,
                span: None,
                source_file_id: 0,
                source_line: 0,
            }
        })
}
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FONT: &[u8] = include_bytes!("../tests/fixtures/ratex_test_font.ttf");

    #[test]
    fn test_shape_invalid_font_bytes() {
        let shaped = shape_opentype_text(b"not a valid font", "hello");
        assert!(shaped.is_empty());
    }

    #[test]
    fn test_shape_empty_text() {
        let shaped = shape_opentype_text(TEST_FONT, "");
        assert!(shaped.is_empty());
    }

    #[test]
    fn test_harfbuzz_rustybuzz_ligatures() {
        // "fi" should be merged into a single ligature glyph `f_i` (glyph id 7)
        let shaped_fi = shape_opentype_text(TEST_FONT, "fi");
        assert_eq!(shaped_fi.len(), 1, "fi should form a single ligature glyph");
        assert_eq!(shaped_fi[0].glyph_id, 7);
        assert_eq!(shaped_fi[0].cluster, 0);
        assert_eq!(shaped_fi[0].x_advance, 580);
        assert_eq!(shaped_fi[0].y_advance, 0);

        // "fl" should form `f_l` (glyph id 8)
        let shaped_fl = shape_opentype_text(TEST_FONT, "fl");
        assert_eq!(shaped_fl.len(), 1, "fl should form a single ligature glyph");
        assert_eq!(shaped_fl[0].glyph_id, 8);
        assert_eq!(shaped_fl[0].cluster, 0);
        assert_eq!(shaped_fl[0].x_advance, 580);

        // "ffi" should form `f_f_i` (glyph id 9)
        let shaped_ffi = shape_opentype_text(TEST_FONT, "ffi");
        assert_eq!(shaped_ffi.len(), 1, "ffi should form a single ligature glyph");
        assert_eq!(shaped_ffi[0].glyph_id, 9);
        assert_eq!(shaped_ffi[0].cluster, 0);
        assert_eq!(shaped_ffi[0].x_advance, 850);
    }

    #[test]
    fn test_harfbuzz_rustybuzz_advance_positioning_and_kerning() {
        // "AV" has a kern of -80 in the test font: 'A' advance is 600 - 80 = 520, 'V' is 600.
        let shaped_av = shape_opentype_text(TEST_FONT, "AV");
        assert_eq!(shaped_av.len(), 2);
        assert_eq!(shaped_av[0].glyph_id, 10); // 'A'
        assert_eq!(shaped_av[0].cluster, 0);
        assert_eq!(shaped_av[0].x_advance, 520, "advance of A should be kerned by -80");

        assert_eq!(shaped_av[1].glyph_id, 11); // 'V'
        assert_eq!(shaped_av[1].cluster, 1);
        assert_eq!(shaped_av[1].x_advance, 600);

        // "f i" with space should not form a ligature: 3 separate glyphs
        let shaped_sep = shape_opentype_text(TEST_FONT, "f i");
        assert_eq!(shaped_sep.len(), 3);
        assert_eq!(shaped_sep[0].glyph_id, 4); // 'f'
        assert_eq!(shaped_sep[0].x_advance, 350);
        assert_eq!(shaped_sep[1].glyph_id, 3); // ' '
        assert_eq!(shaped_sep[1].x_advance, 250);
        assert_eq!(shaped_sep[2].glyph_id, 5); // 'i'
        assert_eq!(shaped_sep[2].x_advance, 280);
    }

    #[test]
    fn test_shape_to_nodes_and_glyph_run() {
        let font_id = 42;
        let nodes = shape_opentype_to_nodes(TEST_FONT, font_id, "fi");
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            crate::boxes::Node::Char { c, font } => {
                assert_eq!(*c, 7);
                assert_eq!(*font, font_id);
            }
            other => panic!("expected Node::Char, got {:?}", other),
        }

        let run = shape_opentype_to_glyph_run(TEST_FONT, font_id, "fi", 10.0, 20.0)
            .expect("glyph run should be produced");
        match run {
            crate::boxes::DisplayItem::GlyphRun {
                font,
                x_bp,
                y_bp,
                glyphs,
                ..
            } => {
                assert_eq!(font, font_id);
                assert_eq!(x_bp, 10.0);
                assert_eq!(y_bp, 20.0);
                assert_eq!(glyphs, vec![7]);
            }
            other => panic!("expected GlyphRun, got {:?}", other),
        }
    }

    #[test]
    fn test_engine_shape_helpers() {
        let eng = Engine::new(false);
        let font_id = 1;
        let nodes = eng.shape_text_with_bytes(TEST_FONT, font_id, "AV");
        assert_eq!(nodes.len(), 2);
        match (&nodes[0], &nodes[1]) {
            (
                crate::boxes::Node::Char { c: c1, font: f1 },
                crate::boxes::Node::Char { c: c2, font: f2 },
            ) => {
                assert_eq!(*c1, 10);
                assert_eq!(*f1, font_id);
                assert_eq!(*c2, 11);
                assert_eq!(*f2, font_id);
            }
            other => panic!("expected two Node::Char nodes, got {:?}", other),
        }

        let run = eng.shape_text_to_glyph_run_with_bytes(TEST_FONT, font_id, "ffi", 5.0, 15.0);
        match run {
            crate::boxes::DisplayItem::GlyphRun {
                font,
                x_bp,
                y_bp,
                glyphs,
                ..
            } => {
                assert_eq!(font, font_id);
                assert_eq!(x_bp, 5.0);
                assert_eq!(y_bp, 15.0);
                assert_eq!(glyphs, vec![9]); // f_f_i ligature
            }
            other => panic!("expected GlyphRun, got {:?}", other),
        }
    }

    #[test]
    fn test_system_font_shaping_if_available() {
        let path = "/usr/share/fonts/liberation/LiberationSerif-Regular.ttf";
        if let Ok(bytes) = std::fs::read(path) {
            let shaped = shape_opentype_text(&bytes, "office");
            // Liberation Serif has standard ffi ligature, so "office" becomes "o", "ffi", "c", "e" (4 glyphs)
            // or shapes all glyphs with valid advances and cluster indices
            assert!(!shaped.is_empty());
            assert!(shaped.iter().all(|g| g.x_advance > 0));
            let cluster_monotonic = shaped.windows(2).all(|w| w[0].cluster <= w[1].cluster);
            assert!(cluster_monotonic, "cluster indices should be monotonic");
        }
    }
}
