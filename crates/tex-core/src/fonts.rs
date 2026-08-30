//! Font resolver trait: maps (FontId, char) to metrics, used by layout code
//! decoupled from the engine's font table.

use crate::tfm::FontId;

pub trait FontResolver {
    fn char_width(&self, f: FontId, c: u8) -> i32;
    fn char_height(&self, f: FontId, c: u8) -> i32;
    fn char_depth(&self, f: FontId, c: u8) -> i32;
    fn char_italic(&self, f: FontId, c: u8) -> i32;
}
