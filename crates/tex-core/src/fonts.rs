//! Font resolver trait: maps (FontId, char) to metrics, used by layout code
//! decoupled from the engine's font table.

use crate::tfm::FontId;

pub trait FontResolver {
    fn char_width(&self, f: FontId, c: u8) -> i32;
    fn char_height(&self, f: FontId, c: u8) -> i32;
    fn char_depth(&self, f: FontId, c: u8) -> i32;
    fn char_italic(&self, f: FontId, c: u8) -> i32;
}

pub use crate::fontload::TrackedFont;

pub use crate::fontiface::{
    shape_opentype_text, shape_opentype_to_glyph_run, shape_opentype_to_nodes,
    shaped_glyphs_to_glyph_run, shaped_glyphs_to_nodes, ShapedGlyph,
};
