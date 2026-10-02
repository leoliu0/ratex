//! XeTeX engine identity (primitive table, parameters), character classes
//! and inter-character tokens, and the native-font/glyph queries.
//!
//! `init_xetex_primitives` turns the common table into exactly the table
//! TeX Live's `xetex -ini -etex` has: the `pdf*` and Ratex-only names are
//! removed, XeTeX's own names are added, and the utility primitives that
//! XeTeX shares with pdfTeX get their XeTeX spelling. Each slice registers
//! the names it implements in its own `register_xetex_*_primitives`.

use std::rc::Rc;
use crate::engine::{Engine, EngineKind};
use crate::eqtb::Equiv;
use crate::prim::{GlueParam, IntParam, Prim};
use crate::token::Token;

/// Utility primitives XeTeX takes from pdfTeX, with pdfTeX's syntax and
/// expansion: (XeTeX name, name of the shared implementation).
static XETEX_ALIASES: &[(&[u8], &[u8])] = &[
    (b"strcmp", b"pdfstrcmp"),
    (b"mdfivesum", b"pdfmdfivesum"),
    (b"filemoddate", b"pdffilemoddate"),
    (b"filedump", b"pdffiledump"),
    (b"creationdate", b"pdfcreationdate"),
    (b"elapsedtime", b"pdfelapsedtime"),
    (b"resettimer", b"pdfresettimer"),
    (b"uniformdeviate", b"pdfuniformdeviate"),
    (b"normaldeviate", b"pdfnormaldeviate"),
    (b"randomseed", b"pdfrandomseed"),
    (b"setrandomseed", b"pdfsetrandomseed"),
    (b"shellescape", b"pdfshellescape"),
    (b"primitive", b"pdfprimitive"),
    (b"ifprimitive", b"ifpdfprimitive"),
];

/// XeTeX's (and TeX Live's) integer parameters that are plain eqtb integers.
static XETEX_INT_PARAMS: &[(&[u8], IntParam)] = &[
    (b"XeTeXlinebreakpenalty", IntParam::XeTeXLinebreakPenalty),
    (b"XeTeXprotrudechars", IntParam::XeTeXProtrudeChars),
    (b"XeTeXupwardsmode", IntParam::XeTeXUpwardsMode),
    (b"XeTeXuseglyphmetrics", IntParam::XeTeXUseGlyphMetrics),
    (b"XeTeXinterchartokenstate", IntParam::XeTeXInterCharTokenState),
    (b"XeTeXdashbreakstate", IntParam::XeTeXDashBreakState),
    (b"XeTeXinputnormalization", IntParam::XeTeXInputNormalization),
    (b"XeTeXtracingfonts", IntParam::XeTeXTracingFonts),
    (b"XeTeXinterwordspaceshaping", IntParam::XeTeXInterwordSpaceShaping),
    (b"XeTeXgenerateactualtext", IntParam::XeTeXGenerateActualText),
    (b"XeTeXhyphenatablelength", IntParam::XeTeXHyphenatableLength),
    (b"tracingstacklevels", IntParam::TracingStackLevels),
    (b"showstream", IntParam::ShowStream),
    (b"suppressfontnotfounderror", IntParam::SuppressFontNotFoundError),
];

impl Engine {
    /// Install the XeTeX primitive table (a no-op for other engines).
    pub fn init_xetex_primitives(&mut self) {
        if self.engine_kind != EngineKind::XeTeX {
            return;
        }
        self.register_xetex_core_primitives();
        self.register_xetex_text_primitives();
        self.register_xetex_math_primitives();
        self.register_xetex_driver_primitives();
        self.alias_xetex_primitives();
        self.prune_to_xetex_primitives();
        self.initialize_xetex_parameters();
        self.rebuild_primitive_table();
    }

    /// Define `name` as `p`, printing it by that name from now on.
    fn xetex_define(&mut self, name: &'static [u8], p: Prim) {
        let id = self.cs.intern(name);
        self.primitive_names.insert(p.code(), name);
        self.eqtb.assign(id, Equiv::Prim(p), true);
    }

    /// Names whose implementation lives in this file: identity, the
    /// parameters, character classes and the input-encoding commands.
    fn register_xetex_core_primitives(&mut self) {
        self.xetex_define(b"XeTeXversion", Prim::XeTeXVersion);
        self.xetex_define(b"XeTeXrevision", Prim::XeTeXRevision);
        self.xetex_define(b"XeTeXcharclass", Prim::XeTeXCharClass);
        self.xetex_define(b"XeTeXinterchartoks", Prim::XeTeXInterCharToks);
        self.xetex_define(b"XeTeXinputencoding", Prim::XeTeXInputEncoding);
        self.xetex_define(b"XeTeXdefaultencoding", Prim::XeTeXDefaultEncoding);
        self.xetex_define(b"XeTeXlinebreaklocale", Prim::XeTeXLinebreakLocale);
        self.xetex_define(b"Uchar", Prim::XeTeXUchar);
        for &(name, param) in XETEX_INT_PARAMS {
            self.xetex_define(name, Prim::IntP(param));
        }
        self.xetex_define(b"XeTeXlinebreakskip", Prim::GlueP(GlueParam::XeTeXLinebreakSkip));
    }

    /// Font, glyph and feature queries, native words (slice XeText).
    fn register_xetex_text_primitives(&mut self) {
        self.xetex_define(b"XeTeXfonttype", Prim::XeTeXFontType);
        self.xetex_define(b"XeTeXglyph", Prim::XeTeXGlyph);
        self.xetex_define(b"XeTeXglyphindex", Prim::XeTeXGlyphIndex);
        self.xetex_define(b"XeTeXglyphname", Prim::XeTeXGlyphName);
        self.xetex_define(b"XeTeXcountglyphs", Prim::XeTeXCountGlyphs);
        self.xetex_define(b"XeTeXglyphbounds", Prim::XeTeXGlyphBounds);
        self.xetex_define(b"XeTeXcharglyph", Prim::XeTeXCharGlyph);
        self.xetex_define(b"XeTeXcountfeatures", Prim::XeTeXCountFeatures);
        self.xetex_define(b"XeTeXfeaturecode", Prim::XeTeXFeatureCode);
        self.xetex_define(b"XeTeXfeaturename", Prim::XeTeXFeatureName);
        self.xetex_define(b"XeTeXcountvariations", Prim::XeTeXCountVariations);
        self.xetex_define(b"XeTeXvariation", Prim::XeTeXVariation);
        self.xetex_define(b"XeTeXvariationname", Prim::XeTeXVariationName);
    }

    /// Unicode math primitives (slice XeMath).
    fn register_xetex_math_primitives(&mut self) {
        for name in [
            &b"Umathcode"[..], b"Umathcodenum", b"Udelcode", b"Udelcodenum", b"Umathchardef",
            b"Umathcharnumdef", b"Umathchar", b"Umathcharnum", b"Umathaccent", b"Udelimiter",
            b"Uradical",
        ] {
            let &(name, u) = crate::uprim::UPRIMS.iter().find(|(n, _)| *n == name).unwrap();
            self.xetex_define(name, Prim::U(u));
        }
    }

    /// Images and PDF queries (slice XeDriver).
    fn register_xetex_driver_primitives(&mut self) {
        self.xetex_define(b"XeTeXpicfile", Prim::XeTeXPicFile);
        self.xetex_define(b"XeTeXpdffile", Prim::XeTeXPdfFile);
    }

    /// Give the shared utility primitives their XeTeX names.
    fn alias_xetex_primitives(&mut self) {
        for &(name, shared) in XETEX_ALIASES {
            let Some(source) = self.cs.lookup(shared) else { continue };
            let Some(equiv) = self.eqtb.get(source).cloned() else { continue };
            if let Equiv::Prim(p) = equiv {
                self.primitive_names.insert(p.code(), name);
            }
            let id = self.cs.intern(name);
            self.eqtb.assign(id, equiv, true);
        }
    }

    /// Undefine every primitive TeX Live's XeTeX does not have.
    fn prune_to_xetex_primitives(&mut self) {
        let keep: crate::FxHashSet<&[u8]> = crate::xetex_names::XETEX_PRIMITIVE_NAMES
            .iter()
            .chain(crate::xetex_names::XETEX_SYMBOL_NAMES)
            .copied()
            .collect();
        let frozen = self.ids.frozen_primitive;
        for id in self.cs.all_ids() {
            if id != frozen
                && matches!(self.eqtb.get(id), Some(Equiv::Prim(_)))
                && !keep.contains(self.cs.name(id))
            {
                self.eqtb.undefine(id, true);
            }
        }
    }

    /// INITEX values of XeTeX's parameters that are not zero.
    fn initialize_xetex_parameters(&mut self) {
        let ints = &mut self.eqtb.int_params;
        ints[IntParam::XeTeXHyphenatableLength.idx() as usize] = 63;
        ints[IntParam::ShowStream.idx() as usize] = -1;
    }

    /// `\primitive` / `\ifprimitive` look names up in this table: the
    /// primitives XeTeX defines at start-up.
    fn rebuild_primitive_table(&mut self) {
        let frozen = self.ids.frozen_primitive;
        let mut table = crate::FxHashMap::default();
        for id in self.cs.all_ids() {
            if let Some(Equiv::Prim(p)) = self.eqtb.get(id) {
                if id != frozen {
                    table.insert(self.cs.name(id).to_vec().into_boxed_slice(), *p);
                }
            }
        }
        self.primitive_table = table;
    }

    /// xetex.web `scan_usv_num`: a Unicode scalar value (0..=0x10FFFF).
    pub(crate) fn scan_usv_num(&mut self) -> u32 {
        let value = self.scan_int();
        if (0..=0x10FFFF).contains(&value) {
            value as u32
        } else {
            self.error(&format!("Bad character code ({value})"));
            0
        }
    }

    /// xetex.web `scan_char_class` / `scan_char_class_not_ignored`; both
    /// accept `char_class_limit` itself, as xetex.web does.
    pub(crate) fn scan_char_class(&mut self) -> u16 {
        let value = self.scan_int();
        if (0..=crate::eqtb::CHAR_CLASS_LIMIT as i32).contains(&value) {
            value as u16
        } else {
            self.error(&format!("Bad character class ({value})"));
            0
        }
    }

    /// `\XeTeXcharclass <usv> = <class>`.
    pub fn do_xetex_charclass_assign(&mut self) {
        let global = self.take_assignment_prefixes("\\XeTeXcharclass");
        let character = self.scan_usv_num();
        self.scan_optional_equals();
        let class = self.scan_char_class();
        self.eqtb.assign_char_class(character, class, global);
    }

    /// `\XeTeXcharclass <usv>` as a number.
    pub fn scan_xetex_charclass_val(&mut self) -> i32 {
        let character = self.scan_usv_num();
        i32::from(self.eqtb.char_class(character))
    }

    /// `\XeTeXinterchartoks <class> <class> = <general text>`.
    pub fn do_xetex_interchartoks_assign(&mut self, owner: crate::token::CsId) {
        let global = self.take_assignment_prefixes("\\XeTeXinterchartoks");
        let c1 = self.scan_char_class();
        let c2 = self.scan_char_class();
        self.scan_optional_equals();
        let toks = Rc::new(self.scan_token_list_of(Some(owner)));
        self.eqtb.assign_inter_char_toks(c1, c2, toks, global);
    }

    /// `\XeTeXinterchartoks <class> <class>` as a token list (`\the`, or
    /// the right-hand side of a token assignment).
    pub fn scan_xetex_interchartoks_the(&mut self) -> Vec<Token> {
        let c1 = self.scan_char_class();
        let c2 = self.scan_char_class();
        self.eqtb
            .inter_char_toks(c1, c2)
            .map(|toks| toks.as_ref().clone())
            .unwrap_or_default()
    }

    /// Appends a glyph node for `\XeTeXglyph <slot>`.
    pub fn do_xetex_glyph(&mut self) {
        let slot = self.scan_int();
        let font_id = self.eqtb.cur_font_val;

        if let Some(native_font) = self.font_loader.native_fonts.get(&font_id) {
            if let Ok(face) = native_font.program.face() {
                let glyph_id = slot.max(0) as u16;
                let at_size = self.eqtb.fonts.get(font_id as usize).map_or(655360, |f| f.at_size);
                let upem = face.units_per_em() as f64;
                let scale = at_size as f64 / upem;

                let advance = face
                    .glyph_hor_advance(ttf_parser::GlyphId(glyph_id))
                    .map_or(0, |adv| (adv as f64 * scale).round() as i32);

                let native_glyph = crate::native_layout::NativeGlyph {
                    glyph_id,
                    cluster_start: 0,
                    cluster_end: 0,
                    x_advance: advance,
                    y_advance: 0,
                    x_offset: 0,
                    y_offset: 0,
                };

                let native_run = crate::native_layout::NativeRun {
                    font: font_id,
                    text: Rc::from(""),
                    glyphs: vec![native_glyph],
                };

                let height = (face.ascender() as f64 * scale).round() as i32;
                let depth = (-face.descender() as f64 * scale).round() as i32;

                self.cur_list.push(crate::boxes::Node::NativeGlyphRun {
                    run: Rc::new(native_run),
                    start: 0,
                    end: 1,
                    width: advance,
                    height,
                    depth,
                });
                return;
            }
        }

        // TFM fallback: append Char node if slot fits in u8
        if (0..=255).contains(&slot) {
            self.cur_list.push(crate::boxes::Node::Char {
                font: font_id,
                c: slot as u8, attr: crate::boxes::Attr::NONE,
            });
        }
    }

    /// Evaluates integer queries for XeTeX primitives:
    /// `\XeTeXversion`, `\XeTeXcountglyphs`, `\XeTeXglyphindex`, `\XeTeXcharglyph`,
    /// `\XeTeXglyphbounds`, `\XeTeXfonttype`, `\XeTeXuseglyphmetrics`, etc.
    pub fn scan_xetex_int_query(&mut self, p: Prim) -> i32 {
        match p {
            Prim::XeTeXVersion => 0,
            Prim::XeTeXFontType => {
                let font_id = self.scan_font_id();
                if let Some(native_font) = self.font_loader.native_fonts.get(&font_id) {
                    if native_font.program.has_graphite {
                        3 // Graphite
                    } else if native_font.program.has_opentype {
                        2 // OpenType
                    } else if native_font.program.has_aat {
                        1 // AAT
                    } else {
                        2 // Default native font
                    }
                } else {
                    0 // TFM
                }
            }
            Prim::XeTeXCountGlyphs => {
                let font_id = self.scan_font_id();
                if let Some(native_font) = self.font_loader.native_fonts.get(&font_id) {
                    if let Ok(face) = native_font.program.face() {
                        return face.number_of_glyphs() as i32;
                    }
                }
                256
            }
            Prim::XeTeXGlyphIndex => {
                let font_id = self.scan_font_id();
                let name = self.scan_general_text_to_string();
                if let Some(native_font) = self.font_loader.native_fonts.get(&font_id) {
                    if let Ok(face) = native_font.program.face() {
                        if let Some(gid) = face.glyph_index_by_name(&name) {
                            return gid.0 as i32;
                        }
                    }
                }
                0
            }
            Prim::XeTeXCharGlyph => {
                let font_id = self.scan_font_id();
                let ch = self.scan_profile_character_code("\\XeTeXcharglyph");
                if let Some(native_font) = self.font_loader.native_fonts.get(&font_id) {
                    if let Ok(face) = native_font.program.face() {
                        if let Some(c) = char::from_u32(ch) {
                            if let Some(gid) = face.glyph_index(c) {
                                return gid.0 as i32;
                            }
                        }
                    }
                }
                ch as i32
            }
            Prim::XeTeXGlyphBounds => {
                let edge = self.scan_int(); // 1=left, 2=bottom, 3=right, 4=top
                let font_id = self.scan_font_id();
                let slot = self.scan_int() as u16;

                if let Some(native_font) = self.font_loader.native_fonts.get(&font_id) {
                    if let Ok(face) = native_font.program.face() {
                        if let Some(bbox) = face.glyph_bounding_box(ttf_parser::GlyphId(slot)) {
                            let at_size = self.eqtb.fonts.get(font_id as usize).map_or(655360, |f| f.at_size);
                            let upem = face.units_per_em() as f64;
                            let scale = at_size as f64 / upem;
                            return match edge {
                                1 => (bbox.x_min as f64 * scale).round() as i32,
                                2 => (bbox.y_min as f64 * scale).round() as i32,
                                3 => (bbox.x_max as f64 * scale).round() as i32,
                                4 => (bbox.y_max as f64 * scale).round() as i32,
                                _ => 0,
                            };
                        }
                    }
                }
                0
            }
            Prim::XeTeXCountFeatures => {
                let _font_id = self.scan_font_id();
                0
            }
            Prim::XeTeXFeatureCode => {
                let _font_id = self.scan_font_id();
                let _idx = self.scan_int();
                0
            }
            Prim::XeTeXCountVariations => {
                let font_id = self.scan_font_id();
                if let Some(native_font) = self.font_loader.native_fonts.get(&font_id) {
                    if let Ok(face) = native_font.program.face() {
                        return face.variation_axes().len() as i32;
                    }
                }
                0
            }
            Prim::XeTeXVariation => {
                let _font_id = self.scan_font_id();
                let _idx = self.scan_int();
                0
            }
            _ => 0,
        }
    }

    /// Expands string/name queries for XeTeX:
    /// `\XeTeXrevision`, `\XeTeXglyphname`, `\XeTeXfeaturename`, `\XeTeXvariationname`.
    pub fn expand_xetex_query(&mut self, p: Prim) {
        match p {
            Prim::XeTeXRevision => {
                for b in b".999998".iter().rev() {
                    self.push_token(Token::char(12, *b as u32));
                }
            }
            Prim::XeTeXGlyphName => {
                let font_id = self.scan_font_id();
                let slot = self.scan_int() as u16;
                let mut name_str = String::new();
                if let Some(native_font) = self.font_loader.native_fonts.get(&font_id) {
                    if let Ok(face) = native_font.program.face() {
                        if let Some(name) = face.glyph_name(ttf_parser::GlyphId(slot)) {
                            name_str.push_str(name);
                        }
                    }
                }
                for b in name_str.bytes().rev() {
                    self.push_token(Token::char(12, b as u32));
                }
            }
            Prim::XeTeXFeatureName => {
                let _font_id = self.scan_font_id();
                let _code = self.scan_int();
            }
            Prim::XeTeXVariationName => {
                let _font_id = self.scan_font_id();
                let _idx = self.scan_int();
            }
            _ => {}
        }
    }

    /// Reads general text or string in braces or quotes into a String.
    fn scan_general_text_to_string(&mut self) -> String {
        self.skip_spaces_relax();
        let tok = self.get_x_raw();
        if tok.is_char() && (tok.chr() == b'"' as u32 || tok.chr() == b'{' as u32) {
            let close_chr = if tok.chr() == b'"' as u32 { b'"' as u32 } else { b'}' as u32 };
            let mut s = String::new();
            loop {
                let t = self.get_x_raw();
                if t == crate::input::EOF_MARKER {
                    break;
                }
                if t.is_char() && t.chr() == close_chr {
                    break;
                }
                if t.is_char() {
                    if let Some(c) = char::from_u32(t.chr()) {
                        s.push(c);
                    }
                }
            }
            s
        } else {
            String::new()
        }
    }
}
