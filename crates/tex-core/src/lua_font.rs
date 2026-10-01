//! Lua-defined fonts (luatex `font.define`, `luafont.c` / `texfont.c`): the
//! font data model shared by the `font` library, the `\font` machinery, the
//! node-level ligature/kerning code and the PDF writer.
//!
//! A Lua font is an ordinary engine font ([`crate::tfm::Font`]) whose `lua`
//! field is set. Unlike a TFM it is indexed by Unicode scalar values (any
//! `u32`), carries per-character kern and ligature tables keyed by the next
//! character, and may describe a virtual font, a math font or an OpenType
//! font whose glyphs are addressed by glyph index.

use std::rc::Rc;

use crate::engine::Engine;
use crate::tfm::FontId;
use crate::FxHashMap;

/// luatex `left_boundarychar`.
pub const LEFT_BOUNDARY: i32 = -1;
/// luatex `right_boundarychar`.
pub const RIGHT_BOUNDARY: i32 = -2;

/// Font `type` field (`font_type_strings`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontType {
    #[default]
    Unknown,
    Virtual,
    Real,
}

/// Font `format` field (`font_format_strings`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontFormat {
    #[default]
    Unknown,
    Type1,
    Type3,
    TrueType,
    OpenType,
}

/// Font `embedding` field (`font_embedding_strings`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontEmbedding {
    #[default]
    Unknown,
    No,
    Subset,
    Full,
}

/// Font `writingmode` / `identity` fields (`unknown`, `horizontal`,
/// `vertical`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontMode {
    #[default]
    Unknown,
    Horizontal,
    Vertical,
}

/// A ligature: `op` is luatex's `lig_type` (the TFM op byte, 0..=11) and
/// `replacement` the ligature character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LuaLig {
    pub op: u8,
    pub replacement: u32,
}

/// One entry of a math `horiz_variants` / `vert_variants` list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MathVariant {
    pub glyph: i32,
    pub extender: i32,
    pub start: i32,
    pub end: i32,
    pub advance: i32,
}

/// Extensible recipe of a character (`extensible = {top,bot,mid,rep}`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extensible {
    pub top: i32,
    pub bot: i32,
    pub mid: i32,
    pub rep: i32,
}

/// The four math kern corners, `(height, kern)` pairs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MathKerns {
    pub top_right: Vec<(i32, i32)>,
    pub top_left: Vec<(i32, i32)>,
    pub bottom_right: Vec<(i32, i32)>,
    pub bottom_left: Vec<(i32, i32)>,
}

/// One virtual-font command of `characters[c].commands`.
#[derive(Clone, Debug)]
pub enum VfCommand {
    /// Select a local font (already resolved to an engine font id).
    Font(FontId),
    Push,
    Pop,
    Char(u32),
    /// `{"rule", height, width}`.
    Rule(i32, i32),
    Right(i32),
    Down(i32),
    /// `{"pdf", mode, literal}`.
    Pdf { mode: u8, data: Vec<u8> },
    /// `{"pdf", "mode", mode}`.
    PdfMode(u8),
    Special(Vec<u8>),
    /// `{"lua", function}`: the function is kept in the Lua state.
    Lua(tex_lua::Value),
    /// `{"node", n}` / `{"image", img}`: Lua object kept in the Lua state.
    Node(tex_lua::Value),
    Image(tex_lua::Value),
    Nop,
    Scale(f32),
}

/// Character data of a Lua font (luatex `charinfo`). Dimensions are in sp.
#[derive(Clone, Debug)]
pub struct LuaCharInfo {
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    pub italic: i32,
    pub vert_italic: i32,
    /// `i32::MIN` when absent.
    pub top_accent: i32,
    /// `i32::MIN` when absent.
    pub bot_accent: i32,
    pub expansion_factor: i32,
    pub left_protruding: i32,
    pub right_protruding: i32,
    /// Glyph index in the font program (the `index` field).
    pub index: u32,
    pub name: Option<Vec<u8>>,
    /// ToUnicode value as UTF-16BE hex digits.
    pub tounicode: Option<Vec<u8>>,
    pub used: bool,
    /// Next larger variant (math `next`).
    pub next: Option<u32>,
    pub extensible: Option<Extensible>,
    pub hor_variants: Vec<MathVariant>,
    pub vert_variants: Vec<MathVariant>,
    pub math_kerns: MathKerns,
    /// Kern to the following character (key: character, or
    /// [`RIGHT_BOUNDARY`]).
    pub kerns: FxHashMap<i32, i32>,
    /// Ligature with the following character.
    pub ligatures: FxHashMap<i32, LuaLig>,
    pub commands: Option<Vec<VfCommand>>,
}

impl Default for LuaCharInfo {
    fn default() -> Self {
        LuaCharInfo {
            width: 0,
            height: 0,
            depth: 0,
            italic: 0,
            vert_italic: 0,
            top_accent: i32::MIN,
            bot_accent: i32::MIN,
            expansion_factor: 1000,
            left_protruding: 0,
            right_protruding: 0,
            index: 0,
            name: None,
            tounicode: None,
            used: false,
            next: None,
            extensible: None,
            hor_variants: Vec::new(),
            vert_variants: Vec::new(),
            math_kerns: MathKerns::default(),
            kerns: FxHashMap::default(),
            ligatures: FxHashMap::default(),
            commands: None,
        }
    }
}

/// `cidinfo = { registry, ordering, supplement, version }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CidInfo {
    pub registry: Vec<u8>,
    pub ordering: Vec<u8>,
    pub supplement: i32,
    pub version: i32,
}

/// A font defined from Lua.
#[derive(Clone, Debug, Default)]
pub struct LuaFont {
    pub chars: FxHashMap<u32, LuaCharInfo>,
    pub left_boundary: Option<LuaCharInfo>,
    pub right_boundary: Option<LuaCharInfo>,
    pub bc: i32,
    pub ec: i32,
    pub name: Vec<u8>,
    pub area: Vec<u8>,
    pub filename: Option<Vec<u8>>,
    pub fullname: Option<Vec<u8>>,
    pub psname: Option<Vec<u8>>,
    pub encodingname: Option<Vec<u8>>,
    pub attributes: Option<Vec<u8>>,
    pub units_per_em: i32,
    pub designsize: i32,
    pub size: i32,
    pub checksum: u32,
    pub direction: i32,
    pub encodingbytes: u8,
    pub streamprovider: u8,
    pub oldmath: bool,
    pub nomath: bool,
    pub tounicode: u8,
    pub slant: i32,
    pub extend: i32,
    pub squeeze: i32,
    pub width: i32,
    pub mode: i32,
    pub used: bool,
    pub subfont: i32,
    pub ftype: FontType,
    pub format: FontFormat,
    pub writingmode: FontMode,
    pub identity: FontMode,
    pub embedding: FontEmbedding,
    pub cidinfo: Option<CidInfo>,
    /// `MathConstants` by luatex's 1-based math parameter index.
    pub math_params: Vec<i32>,
    /// Font expansion: `stretch`, `shrink`, `step` (per mille).
    pub stretch: i32,
    pub shrink: i32,
    pub step: i32,
}

impl LuaFont {
    /// The character record of `code` (a real character, not a boundary).
    #[inline]
    pub fn char_info(&self, code: u32) -> Option<&LuaCharInfo> {
        self.chars.get(&code)
    }

    /// The record of a real or boundary character; boundary characters are
    /// [`LEFT_BOUNDARY`] / [`RIGHT_BOUNDARY`].
    pub fn char_info_or_boundary(&self, code: i32) -> Option<&LuaCharInfo> {
        match code {
            LEFT_BOUNDARY => self.left_boundary.as_ref(),
            RIGHT_BOUNDARY => self.right_boundary.as_ref(),
            c if c >= 0 => self.chars.get(&(c as u32)),
            _ => None,
        }
    }

    #[inline]
    pub fn has_left_boundary(&self) -> bool {
        self.left_boundary.is_some()
    }

    #[inline]
    pub fn has_right_boundary(&self) -> bool {
        self.right_boundary.is_some()
    }

    /// luatex `raw_get_kern`: the kern between `left` and `right` (either may
    /// be a boundary character) when the font defines one.
    pub fn kern(&self, left: i32, right: i32) -> Option<i32> {
        self.char_info_or_boundary(left)?.kerns.get(&right).copied()
    }

    /// luatex `get_ligature`.
    pub fn lig(&self, left: i32, right: i32) -> Option<LuaLig> {
        self.char_info_or_boundary(left)?.ligatures.get(&right).copied()
    }

    /// luatex `quick_char_exists`.
    #[inline]
    pub fn char_exists(&self, code: u32) -> bool {
        self.chars.contains_key(&code)
    }
}

impl crate::tfm::Font {
    /// The Lua character record of `code` when this is a Lua font.
    #[inline]
    pub fn lua_char(&self, code: u32) -> Option<&LuaCharInfo> {
        self.lua.as_ref()?.char_info(code)
    }

    /// Whether this is a Lua font that defines `code`.
    #[inline]
    pub fn lua_char_exists(&self, code: u32) -> bool {
        self.lua.as_ref().is_some_and(|font| font.char_exists(code))
    }

    /// The Lua part of the font.
    #[inline]
    pub fn lua_font(&self) -> Option<&Rc<LuaFont>> {
        self.lua.as_ref()
    }
}

impl Engine {
    /// Character record of `code` in engine font `font` (Lua fonts only).
    #[inline]
    pub fn lua_char(&self, font: FontId, code: u32) -> Option<&LuaCharInfo> {
        self.eqtb.fonts.get(usize::from(font))?.lua_char(code)
    }
}
