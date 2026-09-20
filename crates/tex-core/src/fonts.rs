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
    shape_opentype_text, shape_opentype_text_full, shape_opentype_text_simple,
    shape_opentype_to_glyph_run, shape_opentype_to_glyph_run_full, shape_opentype_to_nodes,
    shape_opentype_to_nodes_full, shaped_glyphs_to_glyph_run, shaped_glyphs_to_native_display_item,
    shaped_glyphs_to_nodes, shaped_glyphs_to_nodes_scaled, ShapedGlyph,
};
pub use crate::native_layout::{calculate_slice_dims, NativeGlyph, NativeRun, NativeTextState};
