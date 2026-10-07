//! XeTeX engine identity (primitive table, parameters), character classes
//! and inter-character tokens, and the native-font/glyph queries.
//!
//! `init_xetex_primitives` turns the common table into exactly the table
//! TeX Live's `xetex -ini -etex` has: the `pdf*` and TeXres-only names are
//! removed, XeTeX's own names are added, and the utility primitives that
//! XeTeX shares with pdfTeX get their XeTeX spelling. Each slice registers
//! the names it implements in its own `register_xetex_*_primitives`.

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
        for q in crate::xetex_query::XeQuery::ALL {
            self.xetex_define(q.name(), Prim::XeTeXQuery(q));
        }
    }

    /// Unicode math primitives (slice XeMath): xetex.web §27875-27895,
    /// §6785-6890. Both the `\U...` name and the `\XeTeXmath*` alias are
    /// defined; `\meaning` shows the `\U...` name.
    fn register_xetex_math_primitives(&mut self) {
        for &(name, alias, x) in crate::xemath_prims::XEMATH_NAMES {
            for n in [name, alias] {
                let id = self.cs.intern(n);
                self.eqtb.assign(id, crate::eqtb::Equiv::Prim(Prim::XeMath(x)), true);
            }
            self.primitive_names.insert(Prim::XeMath(x).code(), name);
        }
        self.eqtb.xe_math = true;
    }

    /// Images and PDF queries (slice XeDriver).
    fn register_xetex_driver_primitives(&mut self) {
        self.xetex_define(b"XeTeXpicfile", Prim::XeTeXPicFile);
        self.xetex_define(b"XeTeXpdffile", Prim::XeTeXPdfFile);
        self.xetex_define(b"XeTeXpdfpagecount", Prim::XeTeXPdfPageCount);
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

    /// `XeTeX_upwards` (`\XeTeXupwardsmode>0`).
    #[inline]
    pub(crate) fn xe_upwards(&self) -> bool {
        self.eqtb.int_params[IntParam::XeTeXUpwardsMode.idx() as usize] > 0
    }

    /// The extents `append_to_vlist` measures a box by: the distance of its
    /// far edge from the previous baseline and the depth the next box starts
    /// from, `(height, depth)` normally and `(depth, height)` while stacking
    /// upwards.
    #[inline]
    pub(crate) fn interline_extents<T>(&self, height: T, depth: T) -> (T, T) {
        if self.xe_upwards() {
            (depth, height)
        } else {
            (height, depth)
        }
    }

    /// `\XeTeXinterchartoks <class> <class> = <general text>`.
    pub fn do_xetex_interchartoks_assign(&mut self, owner: crate::token::CsId) {
        let global = self.take_assignment_prefixes("\\XeTeXinterchartoks");
        let c1 = self.scan_char_class();
        let c2 = self.scan_char_class();
        self.scan_optional_equals();
        let toks = self.scan_toks_value(Some(owner));
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

    /// `scan_glyph_number`: `/name`, case-insensitive `U<char>`, or a
    /// numeric glyph ID. Unicode operands use XeTeX's 16-bit `scan_char_num`.
    fn scan_xetex_protrusion_glyph(&mut self, font: &crate::native_font::NativeFont) -> i32 {
        if self.scan_keyword(b"/") {
            let name = self.scan_file_name();
            font.program
                .shape_face()
                .and_then(|face| face.glyph_index_by_name(&name))
                .map_or(0, |glyph| i32::from(glyph.0))
        } else if self.scan_keyword(b"u") {
            let mut character = self.scan_int();
            if !(0..=65535).contains(&character) {
                self.error(&format!("Bad character code ({character})"));
                character = 0;
            }
            i32::from(font.map_char(character as u32))
        } else {
            self.scan_int()
        }
    }

    /// Native-font protrusion queries share the assignment's glyph grammar
    /// for both integer scanning and `\the`.
    pub(crate) fn xetex_native_font_code(&mut self, f: u16, p: Prim) -> Option<i32> {
        if self.engine_kind != EngineKind::XeTeX || !matches!(p, Prim::LpCode | Prim::RpCode) {
            return None;
        }
        let font = self.eqtb.fonts.get(f as usize)?.native.clone()?;
        let glyph = self.scan_xetex_protrusion_glyph(&font);
        let side = usize::from(p == Prim::RpCode);
        let value = font.protrusion_codes.borrow().get(&glyph).map_or(0, |codes| codes[side]);
        Some(value)
    }

    /// XeTeX font-code assignments are global and do not clamp their values.
    pub(crate) fn xetex_native_font_code_assign(&mut self, f: u16, p: Prim) -> bool {
        if self.engine_kind != EngineKind::XeTeX || !matches!(p, Prim::LpCode | Prim::RpCode) {
            return false;
        }
        let Some(font) = self.eqtb.fonts.get(f as usize).and_then(|font| font.native.clone()) else {
            return false;
        };
        let glyph = self.scan_xetex_protrusion_glyph(&font);
        self.scan_optional_equals();
        let value = self.scan_int();
        let side = usize::from(p == Prim::RpCode);
        let mut codes = font.protrusion_codes.borrow_mut();
        if value != 0 {
            codes.entry(glyph).or_insert([0; 2])[side] = value;
        } else if let Some(entry) = codes.get_mut(&glyph) {
            entry[side] = 0;
            if *entry == [0; 2] {
                codes.remove(&glyph);
            }
        }
        true
    }

    /// `\XeTeXglyph <glyph number>` (xetex.web "Implement \XeTeXglyph").
    pub fn do_xetex_glyph(&mut self, id: crate::token::CsId) {
        use crate::engine::Mode;
        match self.mode {
            Mode::Vertical | Mode::InternalVertical => {
                self.push_token(Token::from_cs(id));
                self.start_paragraph(true);
            }
            m if m.is_m() => self.report_illegal_case(id),
            _ => {
                let font = self.eqtb.cur_font_val;
                if self.is_native_font(font) {
                    let mut n = self.scan_int();
                    if !(0..=65535).contains(&n) {
                        self.error(&format!("Bad glyph number ({n})"));
                        n = 0;
                    }
                    let node = self.xetex_glyph_node(font, n as u16);
                    self.cur_list.push(node);
                } else {
                    let name = self.eqtb.fonts.get(font as usize).map(|f| f.tfm_name.clone()).unwrap_or_default();
                    self.error(&format!("Cannot use \\XeTeXglyph with {name}; not a native platform font"));
                }
            }
        }
    }
}
