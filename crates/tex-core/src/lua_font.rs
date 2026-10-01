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
    pub area: Option<Vec<u8>>,
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

/// luatex `literal_mode` values used by virtual-font `pdf` commands.
pub const PDF_SET_ORIGIN: u8 = 0;
pub const PDF_DIRECT_PAGE: u8 = 1;
pub const PDF_DIRECT_ALWAYS: u8 = 2;
pub const PDF_DIRECT_TEXT: u8 = 3;
pub const PDF_DIRECT_FONT: u8 = 4;
pub const PDF_DIRECT_RAW: u8 = 5;
pub const PDF_SCAN_SPECIAL: u8 = 6;

/// Engine-side bookkeeping of Lua font ids.
#[derive(Default)]
pub struct LuaFontState {
    /// Deleted font ids (luatex reuses the first free slot).
    pub holes: Vec<FontId>,
    /// luatex `font_touched`: the font identifier was scanned by TeX.
    pub touched: crate::FxHashSet<FontId>,
    /// luatex `font_used`: a character of the font reached the PDF.
    pub used: crate::FxHashSet<FontId>,
    /// The control sequence naming fonts without a `\font` identifier.
    pub anonymous_cs: Option<crate::token::CsId>,
    /// Glyph indices found through the character map of the font program.
    pub cmap_cache: crate::FxHashMap<(FontId, u32), u16>,
}

impl Engine {
    /// luatex `is_valid_font`.
    pub(crate) fn lua_font_valid(&self, id: i64) -> bool {
        id >= 0
            && (id as usize) < self.eqtb.fonts.len()
            && !self.lua_fonts.holes.contains(&(id as FontId))
    }

    /// luatex `max_font_id`.
    pub(crate) fn lua_font_max(&self) -> i64 {
        self.eqtb.fonts.len() as i64 - 1
    }

    fn lua_placeholder_font() -> crate::tfm::Font {
        crate::tfm::Font {
            name: String::new(),
            tfm_name: String::new(),
            at_size: 0,
            dsize: 0,
            chars: Vec::new(),
            bc: 1,
            ec: 0,
            lig_kern: Vec::new(),
            kerns: Vec::new(),
            ext: Vec::new(),
            params: vec![0; 7],
            hyphen_char: b'-' as i32,
            skew_char: -1,
            bchar: None,
            type1_path: None,
            enc_name: None,
            map_fontname: None,
            encoding: None,
            lua: Some(Rc::new(LuaFont::default())),
        }
    }

    /// The id luatex `new_font` would allocate next.
    pub(crate) fn lua_next_font_id(&self) -> FontId {
        self.lua_fonts
            .holes
            .iter()
            .copied()
            .min()
            .unwrap_or(self.eqtb.fonts.len() as FontId)
    }

    /// luatex `new_font`: allocate a blank font slot.
    pub(crate) fn lua_new_font(&mut self) -> FontId {
        let cs = match self.lua_fonts.anonymous_cs {
            Some(cs) => cs,
            None => {
                let cs = self.cs.intern(b"FONT");
                self.lua_fonts.anonymous_cs = Some(cs);
                cs
            }
        };
        if let Some(&hole) = self.lua_fonts.holes.iter().min() {
            self.lua_fonts.holes.retain(|&h| h != hole);
            self.lua_reset_font_slot(hole, Self::lua_placeholder_font(), cs);
            return hole;
        }
        self.push_engine_font(Rc::new(Self::lua_placeholder_font()), cs)
    }

    fn lua_reset_font_slot(&mut self, f: FontId, font: crate::tfm::Font, cs: crate::token::CsId) {
        let i = usize::from(f);
        let params = font.params.clone();
        self.eqtb.font_param_levels[i] = vec![1; params.len()];
        self.eqtb.font_params[i] = params;
        self.eqtb.hyphen_char[i] = font.hyphen_char;
        self.eqtb.hyphen_char_levels[i] = 1;
        self.eqtb.skew_char[i] = font.skew_char;
        self.eqtb.skew_char_levels[i] = 1;
        self.eqtb.font_cs[i] = cs;
        self.eqtb.expand[i] = Default::default();
        self.eqtb.fonts[i] = Rc::new(font);
    }

    /// luatex `delete_font`.
    pub(crate) fn lua_delete_font(&mut self, f: FontId) {
        let i = usize::from(f);
        if i == 0 || i >= self.eqtb.fonts.len() {
            return;
        }
        self.lua_fonts.touched.remove(&f);
        self.lua_fonts.used.remove(&f);
        if i + 1 == self.eqtb.fonts.len() {
            self.eqtb.fonts.pop();
            self.eqtb.font_params.pop();
            self.eqtb.font_param_levels.pop();
            self.eqtb.hyphen_char.pop();
            self.eqtb.hyphen_char_levels.pop();
            self.eqtb.skew_char.pop();
            self.eqtb.skew_char_levels.pop();
            self.eqtb.font_cs.pop();
            self.eqtb.expand.pop();
            // trailing holes disappear with it
            while let Some(last) = self.eqtb.fonts.len().checked_sub(1) {
                let last = last as FontId;
                if self.lua_fonts.holes.contains(&last) {
                    self.lua_fonts.holes.retain(|&h| h != last);
                    self.lua_delete_font_tail();
                } else {
                    break;
                }
            }
        } else if !self.lua_fonts.holes.contains(&f) {
            let cs = self.eqtb.font_cs[i];
            self.lua_reset_font_slot(f, Self::lua_placeholder_font(), cs);
            self.lua_fonts.holes.push(f);
        }
    }

    fn lua_delete_font_tail(&mut self) {
        self.eqtb.fonts.pop();
        self.eqtb.font_params.pop();
        self.eqtb.font_param_levels.pop();
        self.eqtb.hyphen_char.pop();
        self.eqtb.hyphen_char_levels.pop();
        self.eqtb.skew_char.pop();
        self.eqtb.skew_char_levels.pop();
        self.eqtb.font_cs.pop();
        self.eqtb.expand.pop();
    }

    /// Store a font read by `font_from_lua` in slot `f`.
    pub(crate) fn lua_install_font(&mut self, f: FontId, parsed: crate::lua_font_lib::ParsedFont) {
        let crate::lua_font_lib::ParsedFont { lua, params, hyphen_char, skew_char, warnings } = parsed;
        for message in warnings {
            self.warning_at(&format!("luatex warning (font): {message}"), None);
        }
        let name = String::from_utf8_lossy(&lua.name).into_owned();
        if lua.used {
            self.lua_fonts.used.insert(f);
        }
        let mut font = crate::tfm::Font {
            name: name.clone(),
            tfm_name: name.clone(),
            at_size: lua.size,
            dsize: lua.designsize,
            chars: Vec::new(),
            bc: 1,
            ec: 0,
            lig_kern: Vec::new(),
            kerns: Vec::new(),
            ext: Vec::new(),
            params,
            hyphen_char,
            skew_char,
            bchar: None,
            type1_path: None,
            enc_name: None,
            map_fontname: lua.psname.as_ref().map(|p| String::from_utf8_lossy(p).into_owned()),
            encoding: None,
            lua: None,
        };
        // The one-byte view of the font: metrics for `\fontcharwd`, the
        // `/Widths` of one-byte PDF fonts and the TeX character range.
        let (mut low, mut high) = (256usize, 0usize);
        for (&code, ci) in lua.chars.iter() {
            if code < 256 {
                low = low.min(code as usize);
                high = high.max(code as usize);
                if font.chars.len() <= code as usize {
                    font.chars.resize(code as usize + 1, crate::tfm::CharInfo { width: 0, height: 0, depth: 0, italic: 0, tag: 0, remainder: 0 });
                }
                font.chars[code as usize] = crate::tfm::CharInfo {
                    width: ci.width,
                    height: ci.height,
                    depth: ci.depth,
                    italic: ci.italic,
                    tag: 0,
                    remainder: 0,
                };
            }
        }
        if low <= high {
            font.bc = low as u8;
            font.ec = high as u8;
        }
        if lua.pdf_kind() == LuaPdfKind::Legacy {
            match &lua.filename {
                None => {
                    self.font_loader.apply_map_entry(&mut font, &name);
                }
                Some(file) => {
                    font.type1_path = Some(String::from_utf8_lossy(file).into_owned());
                    let mut names = vec![String::new(); 256];
                    for (&code, ci) in lua.chars.iter() {
                        if let (true, Some(glyph)) = (code < 256, &ci.name) {
                            names[code as usize] = String::from_utf8_lossy(glyph).into_owned();
                        }
                    }
                    if names.iter().any(|n| !n.is_empty()) {
                        font.encoding = Some(names.into());
                    }
                }
            }
        }
        font.lua = Some(Rc::new(lua));
        let cs = self.eqtb.font_cs[usize::from(f)];
        self.lua_reset_font_slot(f, font, cs);
    }

    /// luatex `find_font_id`: load a font through `\font` machinery without
    /// binding a control sequence (local fonts of virtual fonts).
    pub(crate) fn lua_find_font_id(&mut self, name: &[u8], size: i32) -> FontId {
        let f = self.lua_new_font();
        match self.lua_do_define_font(f, name, size) {
            Some(id) => id,
            None => 0,
        }
    }

    /// luatex `do_define_font` for the blank slot `f`: the `define_font`
    /// callback when registered, the TFM loader otherwise. Returns the font
    /// id to use (an existing font when the callback returned a number) or
    /// `None`; the slot is released on failure.
    pub(crate) fn lua_do_define_font(&mut self, f: FontId, name: &[u8], s: i32) -> Option<FontId> {
        let has_callback = self.lua.as_mut().is_some_and(|lua| lua.has_callback("define_font"));
        if has_callback {
            let name_owned = name.to_vec();
            let result = self.lua_run(|lua| lua.call_define_font(&name_owned, s, f));
            let value = match result {
                Ok(v) => v,
                Err(err) => {
                    self.error(&format!("LuaTeX error {err}"));
                    None
                }
            };
            return match value {
                Some(v) => {
                    if let Some(t) = v.as_table() {
                        match crate::lua_font_lib::font_from_lua(self, f, &t) {
                            Ok(mut parsed) => {
                                let cache = crate::lua_font_lib::cache_allowed(&t);
                                // luatex `do_define_font`: `do_vf` + natural direction
                                if parsed.lua.ftype != FontType::Virtual {
                                    if parsed.lua.ftype == FontType::Unknown {
                                        parsed.lua.ftype = FontType::Real;
                                    }
                                    parsed.lua.direction = -1;
                                }
                                self.lua_install_font(f, parsed);
                                if cache {
                                    self.lua_cache_font_table(f, &t);
                                }
                                Some(f)
                            }
                            Err(msg) => {
                                self.error(&format!("LuaTeX error {msg}"));
                                self.lua_delete_font(f);
                                None
                            }
                        }
                    } else if let Some(n) = v.as_number().map(|n| (n + 0.5).floor() as i64) {
                        self.lua_delete_font(f);
                        if self.lua_font_valid(n) && n > 0 {
                            Some(n as FontId)
                        } else {
                            None
                        }
                    } else {
                        self.lua_delete_font(f);
                        None
                    }
                }
                None => {
                    self.lua_delete_font(f);
                    None
                }
            };
        }
        let base = String::from_utf8_lossy(name).into_owned();
        let loaded = if s >= 0 || s == -1000 {
            self.font_loader.load_tfm(&base, if s >= 0 { s } else { 0 })
        } else {
            let dsize = self.font_loader.load_tfm(&base, 0).map(|font| font.dsize);
            dsize.and_then(|d| self.font_loader.load_tfm(&base, crate::scaled::xn_over_d(d, -s, 1000)))
        };
        let Some(font) = loaded else {
            self.lua_delete_font(f);
            return None;
        };
        // The slot was only a reservation: TFM fonts go through the
        // classic registration (virtual font bases, expansion tables).
        self.lua_delete_font(f);
        let cs = self.lua_fonts.anonymous_cs.unwrap_or(0);
        Some(self.push_engine_font(font, cs))
    }

    /// Remember `t` as the table `font.getfont(f)` returns.
    pub(crate) fn lua_cache_font_table(&mut self, f: FontId, t: &tex_lua::LuaTable) {
        if let Some(lua) = self.lua.as_mut() {
            lua.set_font_cache(f, t);
        }
    }
}

impl crate::engine_lua::LuaEngine {
    /// Call the `define_font` callback with `(name, size, id)`; `None` when
    /// it returned nothing/false.
    pub(crate) fn call_define_font(
        &mut self,
        name: &[u8],
        size: i32,
        id: FontId,
    ) -> Result<Option<tex_lua::Value>, String> {
        use tex_lua::{LuaApi, LuaBytes, LuaFunction, Value};
        let f: Option<LuaFunction> = self
            .lua
            .load("return __ratex_callback(...)")
            .call("define_font")
            .map_err(|e| self.lua.get_error_message(e).message().to_string())?;
        let Some(f) = f else {
            return Ok(None);
        };
        let result: Option<Value> = f
            .call((LuaBytes(name.to_vec()), i64::from(size), i64::from(id)))
            .map_err(|e| self.lua.get_error_message(e).message().to_string())?;
        Ok(result.filter(|v| !v.is_nil() && v.as_boolean() != Some(false)))
    }

    /// Store `t` in the hidden cache consulted by `font.getfont`.
    pub(crate) fn set_font_cache(&mut self, f: FontId, t: &tex_lua::LuaTable) {
        use tex_lua::LuaApi;
        if let Ok(Some(cache)) = self.lua.get_global::<tex_lua::LuaTable>("__ratex_font_cache") {
            let _ = cache.raw_seti(i64::from(f), t);
        }
    }
}

impl Engine {
    /// luatex `set_font_touched` (a font identifier was scanned by TeX).
    #[inline]
    pub(crate) fn lua_touch_font(&mut self, f: FontId) {
        if self.engine_kind == crate::engine::EngineKind::LuaTeX {
            self.lua_fonts.touched.insert(f);
        }
    }

    /// Mutable access to the Lua part of font `f` (copy-on-write: only fonts
    /// shared with other holders are copied).
    pub(crate) fn lua_font_mut(&mut self, f: FontId) -> Option<&mut LuaFont> {
        let font = Rc::make_mut(self.eqtb.fonts.get_mut(usize::from(f))?);
        Some(Rc::make_mut(font.lua.as_mut()?))
    }

    /// The checksum word of a TFM file.
    pub(crate) fn tfm_checksum(&mut self, name: &str) -> Option<u32> {
        let stem = name.strip_suffix(".tfm").unwrap_or(name);
        let data = self
            .font_loader
            .read_dependency(stem, tex_kpse::Format::Tfm)
            .or_else(|| self.font_loader.read_dependency(name, tex_kpse::Format::Tfm))?;
        let b = data.get(24..28)?;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// `font.read_tfm`: luatex `read_tfm_info` as a [`LuaFont`] plus the
    /// `\fontdimen` list.
    pub(crate) fn lua_read_tfm(&mut self, name: &[u8], size: i32) -> Option<(LuaFont, Vec<i32>)> {
        let given = String::from_utf8_lossy(name).into_owned();
        let base = {
            let b = given.rsplit('/').next().unwrap_or(&given);
            b.strip_suffix(".tfm").or_else(|| b.strip_suffix(".ofm")).unwrap_or(b).to_string()
        };
        let load = |loader: &mut crate::fontload::FontLoader, n: &str, at: i32| loader.load_tfm(n, at);
        let mut try_name = given.as_str();
        let mut loaded = if size >= 0 || size == -1000 {
            load(&mut self.font_loader, try_name, size.max(0))
        } else {
            None
        };
        if loaded.is_none() && base != given {
            try_name = &base;
            loaded = if size >= 0 || size == -1000 {
                load(&mut self.font_loader, try_name, size.max(0))
            } else {
                None
            };
        }
        if size < -1000 || (size < 0 && size != -1000) {
            let d = load(&mut self.font_loader, &base, 0)?.dsize;
            loaded = load(&mut self.font_loader, &base, crate::scaled::xn_over_d(d, -size, 1000));
        }
        let font = loaded?;
        let mut lf = crate::lua_font_lib::lua_font_from_tfm(&font, base.as_bytes());
        lf.checksum = self.tfm_checksum(&font.tfm_name).unwrap_or(0);
        let mut params = font.params.clone();
        if params.len() < 7 {
            params.resize(7, 0);
        }
        Some((lf, params))
    }

    /// The bytes of the virtual font `name`.
    pub(crate) fn lua_read_vf_bytes(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let given = String::from_utf8_lossy(name).into_owned();
        let stem = given.strip_suffix(".vf").unwrap_or(&given).to_string();
        self.font_loader.read_dependency(&stem, tex_kpse::Format::Vf)
    }

    /// `font.settounicode`.
    pub(crate) fn lua_set_tounicode(&mut self, f: FontId, code: i64, value: Option<Vec<u8>>) {
        let Ok(code) = u32::try_from(code) else { return };
        if let Some(lf) = self.lua_font_mut(f) {
            if let Some(ci) = lf.chars.get_mut(&code) {
                ci.tounicode = value;
                lf.tounicode = 1;
            }
        }
    }

    /// `font.setexpansion`: luatex `set_expand_params`.
    pub(crate) fn lua_set_expansion(&mut self, f: FontId, stretch: i32, shrink: i32, step: i32) {
        if let Some(lf) = self.lua_font_mut(f) {
            lf.stretch = stretch;
            lf.shrink = shrink;
            lf.step = step;
        }
    }
}

/// C's `%g` for a size in points.
fn format_g(v: f64) -> String {
    let s = format!("{v:.5}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s.is_empty() { "0".to_string() } else { s }
}

impl Engine {
    /// luatex `tex_def_font` after the font name was scanned: the optional
    /// `at`/`scaled` clause, `read_font_info` (through the `define_font`
    /// callback when registered) and binding of the identifier. Unlike
    /// tex.web a font is never shared between two `\font` commands.
    pub(crate) fn lua_font_definition(
        &mut self,
        name: &str,
        cs: crate::token::CsId,
        declaration_source: Option<crate::input::SourceContext>,
        global: bool,
    ) {
        let mut s = -1000i32;
        if self.scan_keyword(b"at") {
            s = self.scan_dimen(false, false);
            if s <= 0 || s >= 0o1_000_000_000 {
                self.error(&format!(
                    "Improper `at' size ({}pt), replaced by 10pt",
                    format_g(f64::from(s) / 65536.0)
                ));
                s = 10 * 65536;
            }
        } else if self.scan_keyword(b"scaled") {
            let n = self.scan_int();
            s = -n;
            if n <= 0 || n > 32768 {
                self.error(&format!("Illegal magnification has been changed to 1000 ({n})"));
                s = -1000;
            }
        }
        let slot = self.lua_new_font();
        match self.lua_do_define_font(slot, name.as_bytes(), s) {
            Some(id) => {
                self.eqtb.font_cs[usize::from(id)] = cs;
                self.eqtb.assign(cs, crate::eqtb::Equiv::FontRef(id), global);
            }
            None => {
                let extra = "metric data not found or bad";
                let cs_name = String::from_utf8_lossy(self.cs.name(cs)).into_owned();
                let message = if s >= 0 {
                    format!("Font \\{cs_name}={name} at {}pt not loadable: {extra}", format_g(f64::from(s) / 65536.0))
                } else if s != -1000 {
                    format!("Font \\{cs_name}={name} scaled {} not loadable: {extra}", -s)
                } else {
                    format!("Font \\{cs_name}={name} not loadable: {extra}")
                };
                self.error_at(&message, declaration_source);
            }
        }
    }
}

/// How glyphs of a Lua font reach the PDF.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LuaPdfKind {
    /// Two-byte CID fonts: OpenType and TrueType programs.
    Cid,
    /// One-byte fonts: Type 1 programs and TFM-named fonts through the map.
    Legacy,
}

impl LuaFont {
    /// luatex decides on `format` (and, lacking one, the font file).
    pub fn pdf_kind(&self) -> LuaPdfKind {
        let sfnt_file = self.filename.as_ref().is_some_and(|name| {
            let name = String::from_utf8_lossy(name).to_ascii_lowercase();
            matches!(name.rsplit('.').next(), Some("otf" | "ttf" | "ttc" | "otc" | "dfont"))
        });
        match self.format {
            FontFormat::OpenType | FontFormat::TrueType => LuaPdfKind::Cid,
            FontFormat::Unknown if sfnt_file => LuaPdfKind::Cid,
            _ => LuaPdfKind::Legacy,
        }
    }
}

impl crate::fontload::FontLoader {
    /// The font program named by a Lua font's `filename`.
    pub(crate) fn lua_font_program(&mut self, lf: &LuaFont) -> Result<Rc<crate::font_program::FontProgram>, String> {
        let Some(filename) = &lf.filename else {
            return Err(format!("Lua font `{}` has no font file", String::from_utf8_lossy(&lf.name)));
        };
        let name = String::from_utf8_lossy(filename).into_owned();
        let data = self
            .read_program_bytes(&name)
            .or_else(|| {
                let base = name.rsplit('/').next().unwrap_or(&name).to_string();
                (base != name).then(|| self.read_program_bytes(&base)).flatten()
            })
            .ok_or_else(|| format!("Font program file `{name}` of font `{}` not found", String::from_utf8_lossy(&lf.name)))?;
        let face_index = u32::try_from(lf.subfont.max(0)).unwrap_or(0);
        self.load_program(data, face_index, Vec::new())
    }
}

impl Engine {
    /// The font program of Lua font `fid` (None for other fonts).
    pub(crate) fn lua_font_program(&mut self, fid: FontId) -> Option<Rc<crate::font_program::FontProgram>> {
        let font = self.eqtb.fonts.get(usize::from(fid))?.clone();
        let lf = font.lua.as_ref()?;
        self.font_loader.lua_font_program(lf).ok()
    }

    /// The glyph index drawn for `c`: the character's `index`, or the
    /// program's character map when the table gave none.
    pub(crate) fn lua_glyph_index(&mut self, fid: FontId, lf: &LuaFont, c: u32) -> Result<u16, String> {
        let index = lf.chars.get(&c).map_or(0, |ci| ci.index);
        if index != 0 {
            return u16::try_from(index)
                .map_err(|_| format!("Glyph index {index} of character {c:#x} is out of range in font {}", String::from_utf8_lossy(&lf.name)));
        }
        if let Some(&gid) = self.lua_fonts.cmap_cache.get(&(fid, c)) {
            return Ok(gid);
        }
        let program = self.font_loader.lua_font_program(lf)?;
        let gid = char::from_u32(c)
            .and_then(|ch| program.face().ok().and_then(|face| face.glyph_index(ch)))
            .map_or(0, |g| g.0);
        self.lua_fonts.cmap_cache.insert((fid, c), gid);
        Ok(gid)
    }
}
