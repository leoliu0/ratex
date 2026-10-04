//! XeTeX font and glyph queries: `\XeTeXfonttype`, `\XeTeXcountglyphs`,
//! `\XeTeXcharglyph`, `\XeTeXglyphindex`, `\XeTeXglyphbounds`,
//! `\XeTeXfirstfontchar`, the `\XeTeXOT...` OpenType-layout queries, the
//! deprecated variation queries, the AAT/Graphite feature queries, and the
//! `\fontchar..`/`\iffontchar` behaviour for native fonts.
//!
//! References: xetex.web "Cases for fetching an integer value" (the
//! `XeTeX_*_code` cases), XeTeX_ext.c (`otfontget*`, `getnativechar*`,
//! `getglyphbounds`, `mapchartoglyph`, `getfontcharrange`) and
//! XeTeXLayoutInterface.cpp (`countScripts`, `getIndLanguage`, ...).
//! TeXres has no AAT renderer and no Graphite engine: `is_aat_font` is
//! never true, and a Graphite font reports no Graphite features.

use crate::engine::Engine;
use crate::native_font::{d2fix, NativeFont, ReqEngine};
use crate::prim::Prim;
use crate::tfm::FontId;
use std::rc::Rc;

/// The XeTeX query primitives that have no `Prim` variant of their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum XeQuery {
    VariationMin,
    VariationMax,
    VariationDefault,
    FindVariationByName,
    FindFeatureByName,
    IsExclusiveFeature,
    CountSelectors,
    SelectorCode,
    FindSelectorByName,
    IsDefaultSelector,
    SelectorName,
    OTCountScripts,
    OTCountLanguages,
    OTCountFeatures,
    OTScriptTag,
    OTLanguageTag,
    OTFeatureTag,
    FirstFontChar,
    LastFontChar,
}

impl XeQuery {
    pub const ALL: [XeQuery; 19] = [
        XeQuery::VariationMin,
        XeQuery::VariationMax,
        XeQuery::VariationDefault,
        XeQuery::FindVariationByName,
        XeQuery::FindFeatureByName,
        XeQuery::IsExclusiveFeature,
        XeQuery::CountSelectors,
        XeQuery::SelectorCode,
        XeQuery::FindSelectorByName,
        XeQuery::IsDefaultSelector,
        XeQuery::SelectorName,
        XeQuery::OTCountScripts,
        XeQuery::OTCountLanguages,
        XeQuery::OTCountFeatures,
        XeQuery::OTScriptTag,
        XeQuery::OTLanguageTag,
        XeQuery::OTFeatureTag,
        XeQuery::FirstFontChar,
        XeQuery::LastFontChar,
    ];

    pub fn idx(self) -> u16 {
        Self::ALL.iter().position(|&q| q == self).unwrap() as u16
    }

    pub fn from_idx(i: u16) -> Option<XeQuery> {
        Self::ALL.get(i as usize).copied()
    }

    /// The primitive's name.
    pub fn name(self) -> &'static [u8] {
        match self {
            XeQuery::VariationMin => b"XeTeXvariationmin",
            XeQuery::VariationMax => b"XeTeXvariationmax",
            XeQuery::VariationDefault => b"XeTeXvariationdefault",
            XeQuery::FindVariationByName => b"XeTeXfindvariationbyname",
            XeQuery::FindFeatureByName => b"XeTeXfindfeaturebyname",
            XeQuery::IsExclusiveFeature => b"XeTeXisexclusivefeature",
            XeQuery::CountSelectors => b"XeTeXcountselectors",
            XeQuery::SelectorCode => b"XeTeXselectorcode",
            XeQuery::FindSelectorByName => b"XeTeXfindselectorbyname",
            XeQuery::IsDefaultSelector => b"XeTeXisdefaultselector",
            XeQuery::SelectorName => b"XeTeXselectorname",
            XeQuery::OTCountScripts => b"XeTeXOTcountscripts",
            XeQuery::OTCountLanguages => b"XeTeXOTcountlanguages",
            XeQuery::OTCountFeatures => b"XeTeXOTcountfeatures",
            XeQuery::OTScriptTag => b"XeTeXOTscripttag",
            XeQuery::OTLanguageTag => b"XeTeXOTlanguagetag",
            XeQuery::OTFeatureTag => b"XeTeXOTfeaturetag",
            XeQuery::FirstFontChar => b"XeTeXfirstfontchar",
            XeQuery::LastFontChar => b"XeTeXlastfontchar",
        }
    }
}

/// `is_aat_font` / `is_ot_font` / `is_gr_font` of xetex.web.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Tfm,
    Ot,
    Gr,
}

fn same_tag(a: ttf_parser::Tag, t: u32) -> bool {
    a.0 == t
}

/// Script tags of the font as `getLargerScriptListTable` returns them.
/// XeTeX reads the GSUB list for both tables (its GPOS call passes
/// `HB_OT_TAG_GSUB`), so the result is always the GSUB script list.
fn script_tags(face: &ttf_parser::Face<'_>) -> Vec<u32> {
    match face.tables().gsub {
        Some(t) => t.scripts.into_iter().map(|s| s.tag.0).collect(),
        None => Vec::new(),
    }
}

/// Language tags of script `script_index` in one table (no default).
fn lang_tags(table: Option<ttf_parser::opentype_layout::LayoutTable<'_>>, script_index: usize) -> Vec<u32> {
    let Some(t) = table else { return Vec::new() };
    let Some(s) = t.scripts.get(script_index as u16) else { return Vec::new() };
    s.languages.into_iter().map(|l| l.tag.0).collect()
}

/// Feature tags of (script, language) in `table`, `hb_ot_layout_language_get_feature_tags`.
fn feature_tags(
    table: Option<ttf_parser::opentype_layout::LayoutTable<'_>>,
    script: u32,
    language: u32,
) -> Vec<u32> {
    let Some(t) = table else { return Vec::new() };
    let Some(s) = t.scripts.into_iter().find(|s| same_tag(s.tag, script)) else {
        return Vec::new();
    };
    let ls = if language == 0 {
        s.default_language
    } else {
        match s.languages.into_iter().find(|l| same_tag(l.tag, language)) {
            Some(l) => Some(l),
            None => return Vec::new(),
        }
    };
    let Some(ls) = ls else { return Vec::new() };
    ls.feature_indices
        .into_iter()
        .filter_map(|fi| t.features.get(fi).map(|f| f.tag.0))
        .collect()
}

impl NativeFont {
    /// `XeTeXFontInst::getFirstCharCode` / `getLastCharCode`.
    pub fn char_range(&self, first: bool) -> i32 {
        let Some(face) = self.program.shape_face() else { return 0 };
        let Some(cmap) = face.tables().cmap else { return 0 };
        // FreeType keeps one Unicode charmap, preferring the full repertoire.
        let mut chosen = None;
        for st in cmap.subtables {
            if !st.is_unicode() {
                continue;
            }
            let full = matches!(
                (st.platform_id, st.encoding_id),
                (ttf_parser::PlatformId::Windows, 10) | (ttf_parser::PlatformId::Unicode, 4 | 6)
            );
            if chosen.is_none() || full {
                chosen = Some(st);
                if full {
                    break;
                }
            }
        }
        let Some(st) = chosen else { return 0 };
        let (mut lo, mut hi) = (u32::MAX, 0u32);
        st.codepoints(|c| {
            if st.glyph_index(c).is_some_and(|g| g.0 != 0) {
                lo = lo.min(c);
                hi = hi.max(c);
            }
        });
        if lo == u32::MAX {
            0
        } else if first {
            lo as i32
        } else {
            hi as i32
        }
    }
}

impl Engine {
    fn xe_native(&self, f: FontId) -> Option<Rc<NativeFont>> {
        self.eqtb.fonts.get(f as usize).and_then(|x| x.native.clone())
    }

    fn xe_kind(&self, f: FontId) -> Kind {
        match self.xe_native(f) {
            None => Kind::Tfm,
            Some(n) if n.req_engine == ReqEngine::Graphite && n.program.has_graphite => Kind::Gr,
            Some(_) => Kind::Ot,
        }
    }

    /// `not_aat_font_error` and friends: `what` is the tail of the message.
    fn xe_font_error(&mut self, cmd: &str, f: FontId, what: &str) {
        let name = self
            .eqtb
            .fonts
            .get(f as usize)
            .map(|x| x.tfm_name.clone())
            .unwrap_or_default();
        self.error(&format!("Cannot use \\{cmd} with {name}; {what}"));
    }

    fn xe_not_aat(&mut self, cmd: &str, f: FontId) {
        self.xe_font_error(cmd, f, "not an AAT font");
    }
    fn xe_not_aat_gr(&mut self, cmd: &str, f: FontId) {
        self.xe_font_error(cmd, f, "not an AAT or Graphite font");
    }
    fn xe_not_ot(&mut self, cmd: &str, f: FontId) {
        self.xe_font_error(cmd, f, "not an OpenType Layout font");
    }
    fn xe_not_native(&mut self, cmd: &str, f: FontId) {
        self.xe_font_error(cmd, f, "not a native platform font");
    }

    // ---- character-level helpers (also used by \fontchar.. and \iffontchar) ----

    /// `\iffontchar` for a native font; `None` for a TFM font.
    pub(crate) fn native_char_present(&mut self, f: FontId, c: u32) -> Option<bool> {
        let nf = self.xe_native(f)?;
        Some(c <= 0x10ffff && nf.map_char(c) != 0)
    }

    /// `\fontcharwd/ht/dp/ic` for a native font (`getnativechar*`).
    pub(crate) fn native_char_dimensions(&mut self, f: FontId, c: u32) -> Option<(i32, i32, i32, i32)> {
        let nf = self.xe_native(f)?;
        let gid = nf.map_char(c);
        let wd = d2fix(nf.engine_glyph_width(gid) as f64);
        let (h, d) = nf.glyph_height_depth(gid);
        let (mut height, mut depth) = (d2fix(h as f64), d2fix(d as f64));
        // snap to the known zones if within 4% of the em size
        let params = &self.eqtb.font_params[f as usize];
        let p = |i: usize| params.get(i - 1).copied().unwrap_or(0);
        let fuzz = p(6) / 25;
        let snap = |zone: &mut i32, value: i32| {
            if (*zone - value).abs() < fuzz {
                *zone = value;
            }
        };
        snap(&mut depth, 0);
        snap(&mut height, 0);
        snap(&mut height, p(5));
        snap(&mut height, p(8));
        let (_lsb, rsb) = nf.glyph_sidebearings(gid);
        let rsb = d2fix(rsb as f64);
        let ic = if rsb < 0 { nf.letter_space - rsb } else { nf.letter_space };
        Some((wd, height, depth, ic))
    }

    /// A native word as a node list (for callers that want a list).
    pub fn shape_native_slice(&self, font: FontId, text: &str) -> Result<Vec<crate::boxes::Node>, String> {
        if !self.is_native_font(font) {
            return Err(format!("Font {font} is not a registered native font"));
        }
        Ok(vec![self.xetex_native_word(font, text)])
    }

    // ---- the queries ----

    /// Integer (and dimension) values of the XeTeX font queries that have
    /// their own `Prim` variant.
    pub fn scan_xetex_int_query(&mut self, p: Prim) -> i32 {
        match p {
            Prim::XeTeXVersion => 0,
            Prim::XeTeXPdfPageCount => self.xetex_pdf_page_count(),
            Prim::XeTeXFontType => {
                let n = self.scan_font_id();
                match self.xe_kind(n) {
                    Kind::Tfm => 0,
                    Kind::Ot => 2,
                    Kind::Gr => 3,
                }
            }
            Prim::XeTeXCountGlyphs => {
                let n = self.scan_font_id();
                self.xe_native(n)
                    .and_then(|nf| nf.program.shape_face())
                    .map_or(0, |face| face.number_of_glyphs() as i32)
            }
            Prim::XeTeXCountFeatures => {
                // Graphite features only; TeXres has no Graphite engine.
                let _ = self.scan_font_id();
                0
            }
            Prim::XeTeXFeatureCode => {
                let n = self.scan_font_id();
                match self.xe_kind(n) {
                    Kind::Gr => {
                        let _ = self.scan_int();
                        0
                    }
                    _ => {
                        self.xe_not_aat_gr("XeTeXfeaturecode", n);
                        -1
                    }
                }
            }
            Prim::XeTeXCountVariations | Prim::XeTeXVariation => {
                // deprecated: always 0
                let _ = self.scan_font_id();
                0
            }
            Prim::XeTeXCharGlyph => {
                let cf = self.eqtb.cur_font_val;
                match self.xe_native(cf) {
                    Some(nf) => {
                        let n = self.scan_int();
                        if n < 0 {
                            0
                        } else {
                            i32::from(nf.map_char(n as u32))
                        }
                    }
                    None => {
                        self.xe_not_native("XeTeXcharglyph", cf);
                        0
                    }
                }
            }
            Prim::XeTeXGlyphIndex => {
                let cf = self.eqtb.cur_font_val;
                match self.xe_native(cf) {
                    Some(nf) => {
                        let name = self.scan_file_name();
                        nf.program
                            .shape_face()
                            .and_then(|face| face.glyph_index_by_name(&name))
                            .map_or(0, |g| i32::from(g.0))
                    }
                    None => {
                        self.xe_not_native("XeTeXglyphindex", cf);
                        0
                    }
                }
            }
            Prim::XeTeXGlyphBounds => {
                let cf = self.eqtb.cur_font_val;
                match self.xe_native(cf) {
                    Some(nf) => {
                        let n = self.scan_int();
                        if !(1..=4).contains(&n) {
                            self.error(&format!(
                                "\\XeTeXglyphbounds requires an edge index from 1 to 4;\nI don't know anything about edge {n}"
                            ));
                            0
                        } else {
                            let gid = self.scan_int().clamp(0, 0xffff) as u16;
                            let (a, b) = if n & 1 == 1 {
                                nf.glyph_sidebearings(gid)
                            } else {
                                nf.glyph_height_depth(gid)
                            };
                            d2fix(if n <= 2 { a } else { b } as f64)
                        }
                    }
                    None => {
                        self.xe_not_native("XeTeXglyphbounds", cf);
                        0
                    }
                }
            }
            _ => 0,
        }
    }

    /// Integer values of the `XeQuery` primitives.
    pub fn scan_xetex_query(&mut self, q: XeQuery) -> i32 {
        let name = std::str::from_utf8(q.name()).unwrap_or("");
        match q {
            XeQuery::VariationMin | XeQuery::VariationMax | XeQuery::VariationDefault => {
                let _ = self.scan_font_id();
                0
            }
            XeQuery::FindVariationByName => {
                let n = self.scan_font_id();
                self.xe_not_aat(name, n);
                -1
            }
            XeQuery::IsExclusiveFeature | XeQuery::CountSelectors => {
                let n = self.scan_font_id();
                if self.xe_kind(n) == Kind::Gr {
                    let _ = self.scan_int();
                    if q == XeQuery::IsExclusiveFeature { 1 } else { 0 }
                } else {
                    self.xe_not_aat_gr(name, n);
                    -1
                }
            }
            XeQuery::SelectorCode | XeQuery::IsDefaultSelector => {
                let n = self.scan_font_id();
                if self.xe_kind(n) == Kind::Gr {
                    let _ = self.scan_int();
                    let _ = self.scan_int();
                    0
                } else {
                    self.xe_not_aat_gr(name, n);
                    -1
                }
            }
            XeQuery::FindFeatureByName => {
                let n = self.scan_font_id();
                if self.xe_kind(n) == Kind::Gr {
                    let _ = self.scan_file_name();
                    -1
                } else {
                    self.xe_not_aat_gr(name, n);
                    -1
                }
            }
            XeQuery::FindSelectorByName => {
                let n = self.scan_font_id();
                if self.xe_kind(n) == Kind::Gr {
                    let _ = self.scan_int();
                    let _ = self.scan_file_name();
                    -1
                } else {
                    self.xe_not_aat_gr(name, n);
                    -1
                }
            }
            XeQuery::OTCountScripts => {
                let n = self.scan_font_id();
                match (self.xe_kind(n), self.xe_native(n)) {
                    (Kind::Ot, Some(nf)) => nf
                        .program
                        .shape_face()
                        .map_or(0, |face| script_tags(&face).len() as i32),
                    _ => 0,
                }
            }
            XeQuery::OTCountLanguages | XeQuery::OTScriptTag => {
                let n = self.scan_font_id();
                match (self.xe_kind(n), self.xe_native(n)) {
                    (Kind::Ot, Some(nf)) => {
                        let param = self.scan_int();
                        let Some(face) = nf.program.shape_face() else { return 0 };
                        let scripts = script_tags(&face);
                        if q == XeQuery::OTScriptTag {
                            usize::try_from(param)
                                .ok()
                                .and_then(|i| scripts.get(i).copied())
                                .unwrap_or(0) as i32
                        } else {
                            let t = face.tables();
                            let mut count = 0;
                            if let Some(i) = scripts.iter().position(|&s| s == param as u32) {
                                count += lang_tags(t.gsub, i).len();
                                count += lang_tags(t.gpos, i).len();
                            }
                            count as i32
                        }
                    }
                    _ => {
                        self.xe_not_ot(name, n);
                        -1
                    }
                }
            }
            XeQuery::OTCountFeatures | XeQuery::OTLanguageTag => {
                let n = self.scan_font_id();
                match (self.xe_kind(n), self.xe_native(n)) {
                    (Kind::Ot, Some(nf)) => {
                        let k = self.scan_int();
                        let v = self.scan_int();
                        let Some(face) = nf.program.shape_face() else { return 0 };
                        let t = face.tables();
                        if q == XeQuery::OTCountFeatures {
                            let mut count = 0;
                            for table in [t.gsub, t.gpos] {
                                count += feature_tags(table, k as u32, v as u32).len();
                            }
                            count as i32
                        } else {
                            // getIndLanguage: GSUB language list, then the GPOS
                            // list of the *same script index*; the index is
                            // compared against each list unadjusted.
                            let scripts = script_tags(&face);
                            let mut rval = 0u32;
                            for (i, &s) in scripts.iter().enumerate() {
                                if s != k as u32 {
                                    continue;
                                }
                                let index = v as usize;
                                let sub = lang_tags(t.gsub, i);
                                if index < sub.len() {
                                    rval = sub[index];
                                    break;
                                }
                                let pos = lang_tags(t.gpos, i);
                                if index < pos.len() {
                                    rval = pos[index];
                                    break;
                                }
                            }
                            rval as i32
                        }
                    }
                    _ => {
                        self.xe_not_ot(name, n);
                        -1
                    }
                }
            }
            XeQuery::OTFeatureTag => {
                let n = self.scan_font_id();
                match (self.xe_kind(n), self.xe_native(n)) {
                    (Kind::Ot, Some(nf)) => {
                        let (k, kk, index) = (self.scan_int(), self.scan_int(), self.scan_int());
                        let Some(face) = nf.program.shape_face() else { return 0 };
                        let t = face.tables();
                        let mut index = index as i64;
                        for table in [t.gsub, t.gpos] {
                            let tags = feature_tags(table, k as u32, kk as u32);
                            let found = !tags.is_empty() || {
                                // a found script/language with zero features
                                // still consumes nothing
                                false
                            };
                            let _ = found;
                            if index >= 0 && (index as usize) < tags.len() {
                                return tags[index as usize] as i32;
                            }
                            index -= tags.len() as i64;
                        }
                        0
                    }
                    _ => {
                        self.xe_not_ot(name, n);
                        -1
                    }
                }
            }
            XeQuery::FirstFontChar | XeQuery::LastFontChar => {
                let n = self.scan_font_id();
                let first = q == XeQuery::FirstFontChar;
                match self.xe_native(n) {
                    Some(nf) => nf.char_range(first),
                    None => self
                        .eqtb
                        .fonts
                        .get(n as usize)
                        .map_or(0, |f| if first { i32::from(f.bc) } else { i32::from(f.ec) }),
                }
            }
            XeQuery::SelectorName => 0,
        }
    }

    /// Expands the string queries (`convert` primitives): `\XeTeXrevision`,
    /// `\XeTeXglyphname`, `\XeTeXfeaturename`, `\XeTeXvariationname`,
    /// `\XeTeXselectorname`.
    pub fn expand_xetex_query(&mut self, p: Prim) {
        match p {
            Prim::XeTeXRevision => self.exp_string(b".999998"),
            Prim::XeTeXGlyphName => {
                let n = self.scan_font_id();
                match self.xe_native(n) {
                    Some(nf) => {
                        let gid = self.scan_int();
                        let name = nf
                            .program
                            .shape_face()
                            .zip(u16::try_from(gid).ok())
                            .and_then(|(face, g)| face.glyph_name(ttf_parser::GlyphId(g)).map(str::to_owned));
                        if let Some(name) = name {
                            self.exp_string(name.as_bytes());
                        }
                    }
                    None => self.xe_not_native("XeTeXglyphname", n),
                }
            }
            Prim::XeTeXFeatureName => {
                let n = self.scan_font_id();
                if self.xe_kind(n) == Kind::Gr {
                    let _ = self.scan_int();
                } else {
                    self.xe_not_aat_gr("XeTeXfeaturename", n);
                }
            }
            Prim::XeTeXVariationName => {
                let n = self.scan_font_id();
                self.xe_not_aat("XeTeXvariationname", n);
            }
            _ => {}
        }
    }

    /// `\XeTeXselectorname <font> <int> <int>`.
    pub fn expand_xetex_selector_name(&mut self) {
        let n = self.scan_font_id();
        if self.xe_kind(n) == Kind::Gr {
            let _ = self.scan_int();
            let _ = self.scan_int();
        } else {
            self.xe_not_aat_gr("XeTeXselectorname", n);
        }
    }
}
