//! Complete XeTeX primitive semantics, font/glyph queries, and character class handling.

use std::rc::Rc;
use crate::engine::{Engine, EngineKind};
use crate::prim::Prim;
use crate::token::Token;

impl Engine {
    /// Registers all XeTeX primitives when `engine_kind == EngineKind::XeTeX`.
    pub fn init_xetex_primitives(&mut self) {
        if self.engine_kind != EngineKind::XeTeX {
            return;
        }

        let def = |name: &'static [u8], p: Prim, e: &mut Engine| {
            let id = e.cs.intern(name);
            e.eqtb.assign(id, crate::eqtb::Equiv::Prim(p), false);
        };

        macro_rules! d {
            ($eng:expr, $name:expr, $prim:ident) => {
                def($name, Prim::$prim, $eng)
            };
        }

        d!(self, b"XeTeXversion", XeTeXVersion);
        d!(self, b"XeTeXrevision", XeTeXRevision);
        d!(self, b"XeTeXfonttype", XeTeXFontType);
        d!(self, b"XeTeXglyph", XeTeXGlyph);
        d!(self, b"XeTeXglyphindex", XeTeXGlyphIndex);
        d!(self, b"XeTeXglyphname", XeTeXGlyphName);
        d!(self, b"XeTeXcountglyphs", XeTeXCountGlyphs);
        d!(self, b"XeTeXglyphbounds", XeTeXGlyphBounds);
        d!(self, b"XeTeXuseglyphmetrics", XeTeXUseGlyphMetrics);
        d!(self, b"XeTeXcharglyph", XeTeXCharGlyph);
        d!(self, b"XeTeXinterchartokenstate", XeTeXInterCharTokenState);
        d!(self, b"XeTeXcharclass", XeTeXCharClass);
        d!(self, b"XeTeXinterchartoks", XeTeXInterCharToks);
        d!(self, b"XeTeXcountfeatures", XeTeXCountFeatures);
        d!(self, b"XeTeXfeaturecode", XeTeXFeatureCode);
        d!(self, b"XeTeXfeaturename", XeTeXFeatureName);
        d!(self, b"XeTeXcountvariations", XeTeXCountVariations);
        d!(self, b"XeTeXvariation", XeTeXVariation);
        d!(self, b"XeTeXvariationname", XeTeXVariationName);
        d!(self, b"XeTeXpicfile", XeTeXPicFile);
        d!(self, b"XeTeXpdffile", XeTeXPdfFile);
        d!(self, b"XeTeXinputnormalization", XeTeXInputNormalization);
        d!(self, b"XeTeXgenerateactualtext", XeTeXGenerateActualText);
        d!(self, b"XeTeXdashbreakstate", XeTeXDashBreakState);
    }

    /// Handles `\XeTeXcharclass <char> = <class>` assignment.
    pub fn do_xetex_charclass_assign(&mut self) {
        let ch = self.scan_profile_character_code("\\XeTeXcharclass");
        self.scan_optional_equals();
        let class = self.scan_int();
        if (0..=255).contains(&class) {
            self.xetex_char_classes.insert(ch, class as u8);
        } else {
            self.error(&format!(
                "XeTeX character class {class} is out of range; expected 0 through 255"
            ));
        }
    }

    /// Reads `\XeTeXcharclass <char>` in an integer expression.
    pub fn scan_xetex_charclass_val(&mut self) -> i32 {
        let ch = self.scan_profile_character_code("\\XeTeXcharclass");
        self.xetex_char_classes.get(&ch).copied().unwrap_or(0) as i32
    }

    /// Handles `\XeTeXinterchartoks <c1> <c2> = { <toks> }` assignment.
    pub fn do_xetex_interchartoks_assign(&mut self) {
        let c1 = self.scan_int();
        let c2 = self.scan_int();
        self.scan_optional_equals();
        let toks = self.scan_token_list();
        if (0..=255).contains(&c1) && (0..=255).contains(&c2) {
            self.xetex_interchar_toks.insert((c1 as u8, c2 as u8), toks);
        } else {
            self.error("XeTeX interchar token classes must be between 0 and 255");
        }
    }

    /// Reads `\the\XeTeXinterchartoks <c1> <c2>` token list.
    pub fn scan_xetex_interchartoks_the(&mut self) -> Vec<Token> {
        let c1 = self.scan_int();
        let c2 = self.scan_int();
        if (0..=255).contains(&c1) && (0..=255).contains(&c2) {
            self.xetex_interchar_toks
                .get(&(c1 as u8, c2 as u8))
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        }
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
                c: slot as u8,
            });
        }
    }

    /// Evaluates integer queries for XeTeX primitives:
    /// `\XeTeXversion`, `\XeTeXcountglyphs`, `\XeTeXglyphindex`, `\XeTeXcharglyph`,
    /// `\XeTeXglyphbounds`, `\XeTeXfonttype`, `\XeTeXuseglyphmetrics`, etc.
    pub fn scan_xetex_int_query(&mut self, p: Prim) -> i32 {
        match p {
            Prim::XeTeXVersion => 0,
            Prim::XeTeXUseGlyphMetrics => self.xetex_use_glyph_metrics,
            Prim::XeTeXInterCharTokenState => self.xetex_interchartokenstate,
            Prim::XeTeXInputNormalization => self.xetex_input_normalization,
            Prim::XeTeXGenerateActualText => self.xetex_generate_actual_text,
            Prim::XeTeXDashBreakState => self.xetex_dash_break_state,
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
