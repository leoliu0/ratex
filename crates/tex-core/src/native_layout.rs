//! XeTeX native-word layout: shaping (`measure_native_node` of XeTeX_ext.c
//! with `layoutChars` of XeTeXLayoutInterface.cpp), glyph metrics and the
//! shared glyph/run records carried by native word and glyph nodes.

use crate::native_font::{d2fix, NativeFont, ReqEngine};
use crate::tfm::FontId;
use std::rc::Rc;

/// A shaped glyph. For XeTeX native words `x_offset` is 0 and `x_advance`
/// is the distance to the next glyph origin (`locations[i+1].x -
/// locations[i].x`, the last one reaching the word width), `y_offset` the
/// raise (XeTeX's `-locations[i].y`); cluster fields are byte offsets into
/// the run text.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeGlyph {
    pub glyph_id: u16,
    pub cluster_start: u32,
    pub cluster_end: u32,
    pub x_advance: i32,
    pub y_advance: i32,
    pub x_offset: i32,
    pub y_offset: i32,
}

/// A shared sequence of shaped glyphs with associated source text and font.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeRun {
    pub font: FontId,
    pub text: Rc<str>,
    pub glyphs: Vec<NativeGlyph>,
    /// `native_word_node_AT`: emit ActualText for this word
    /// (`\XeTeXgenerateactualtext`).
    pub actual_text: bool,
}

/// State of the character run the main loop is collecting (`native_text`).
#[derive(Default, Debug)]
pub struct NativeTextState {
    /// UTF-16 text of the native word being collected, `main_f` its font.
    pub buffer: Vec<u16>,
    pub font: Option<FontId>,
    /// First hyphen offset (`main_h`), 0 when none yet.
    pub first_hyph: usize,
    /// `is_hyph` of the last collected character
    pub last_is_hyph: bool,
    /// Inside a run of characters (`prev_class` is meaningful).
    pub in_run: bool,
    /// `prev_class` / `space_class` of the inter-character token machinery.
    pub prev_class: u16,
    pub space_class: u16,
    /// The character put back by an `\XeTeXinterchartoks` insertion.
    pub backed_up_char: Option<u32>,
    /// Physical source for the buffered run, captured before shipout.
    pub source: Option<(u32, u32)>,
    /// \noboundary before a character: suppress its left boundary
    /// ligature/kern (tex.web `cancel_boundary`)
    pub suppress_left_boundary: bool,
    /// font of the open TFM character chain (tex.web main loop): ligatures
    /// and kerns only form between characters of one uninterrupted chain
    pub lig_chain: Option<FontId>,
}

impl NativeTextState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// Check if a character has strong Right-To-Left bidirectional directionality.
pub fn char_bidi_is_rtl(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0590..=0x05FF // Hebrew
        | 0x0600..=0x06FF // Arabic
        | 0x0700..=0x074F // Syriac
        | 0x0750..=0x077F // Arabic Supplement
        | 0x0780..=0x07BF // Thaana
        | 0x07C0..=0x07FF // NKo
        | 0x0800..=0x083F // Samaritan
        | 0x0840..=0x085F // Mandaic
        | 0x08A0..=0x08FF // Arabic Extended-A
        | 0xFB1D..=0xFB4F // Hebrew Presentation Forms
        | 0xFB50..=0xFDFF // Arabic Presentation Forms-A
        | 0xFE70..=0xFEFF // Arabic Presentation Forms-B
        | 0x1EE00..=0x1EEFF // Arabic Mathematical Alphabetic Symbols
    )
}

/// Detects the predominant script of a text run for HarfBuzz shaping.
pub fn detect_script(text: &str) -> Option<rustybuzz::Script> {
    for ch in text.chars() {
        match ch as u32 {
            0x0590..=0x05FF | 0xFB1D..=0xFB4F => return Some(rustybuzz::script::HEBREW),
            0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => {
                return Some(rustybuzz::script::ARABIC);
            }
            0x0700..=0x074F => return Some(rustybuzz::script::SYRIAC),
            0x0780..=0x07BF => return Some(rustybuzz::script::THAANA),
            0x0900..=0x097F => return Some(rustybuzz::script::DEVANAGARI),
            0x0980..=0x09FF => return Some(rustybuzz::script::BENGALI),
            0x0A00..=0x0A7F => return Some(rustybuzz::script::GURMUKHI),
            0x0A80..=0x0AFF => return Some(rustybuzz::script::GUJARATI),
            0x0B00..=0x0B7F => return Some(rustybuzz::script::ORIYA),
            0x0B80..=0x0BFF => return Some(rustybuzz::script::TAMIL),
            0x0C00..=0x0C7F => return Some(rustybuzz::script::TELUGU),
            0x0C80..=0x0CFF => return Some(rustybuzz::script::KANNADA),
            0x0D00..=0x0D7F => return Some(rustybuzz::script::MALAYALAM),
            0x0D80..=0x0DFF => return Some(rustybuzz::script::SINHALA),
            0x0E00..=0x0E7F => return Some(rustybuzz::script::THAI),
            0x0E80..=0x0EFF => return Some(rustybuzz::script::LAO),
            0x0F00..=0x0FFF => return Some(rustybuzz::script::TIBETAN),
            0x1000..=0x109F => return Some(rustybuzz::script::MYANMAR),
            0x10A0..=0x10FF => return Some(rustybuzz::script::GEORGIAN),
            0x1100..=0x11FF | 0xAC00..=0xD7AF => return Some(rustybuzz::script::HANGUL),
            0x3040..=0x309F => return Some(rustybuzz::script::HIRAGANA),
            0x30A0..=0x30FF => return Some(rustybuzz::script::KATAKANA),
            0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2CEAF => return Some(rustybuzz::script::HAN),
            0x0370..=0x03FF | 0x1F00..=0x1FFF => return Some(rustybuzz::script::GREEK),
            0x0400..=0x04FF | 0x0500..=0x052F => return Some(rustybuzz::script::CYRILLIC),
            0x0041..=0x007A | 0x00C0..=0x024F => return Some(rustybuzz::script::LATIN),
            _ => {}
        }
    }
    None
}

/// Backward compatibility stub: Ratex now natively supports bidirectional text layout.
pub fn is_unsupported_bidi(_ch: char) -> bool {
    false
}

/// Check for default-ignorable characters that may have GID 0 without error.
pub fn is_default_ignorable(ch: char) -> bool {
    matches!(
        ch as u32,
        0x00AD // Soft Hyphen
        | 0x200B // Zero Width Space
        | 0x200C // Zero Width Non-Joiner
        | 0x200D // Zero Width Joiner
        | 0x200E // Left-to-Right Mark
        | 0x200F // Right-to-Left Mark
        | 0x202A..=0x202E // Bidi embeddings/overrides
        | 0x2060 // Word Joiner
        | 0x2066..=0x2069 // Bidi isolates
        | 0xFEFF // Zero Width No-Break Space (BOM)
    )
}

/// Glyph bounding box in points (`GlyphBBox` of XeTeX).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlyphBBox {
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
}

impl NativeFont {
    fn face(&self) -> Option<Rc<crate::font_program::ShapeFace>> {
        self.program.shape_face()
    }

    /// `XeTeXFontInst::mapCharToGlyph` (0 when absent).
    pub fn map_char(&self, c: u32) -> u16 {
        if c > 0x10ffff || (0xd800..=0xdfff).contains(&c) {
            return 0;
        }
        let Some(face) = self.face() else { return 0 };
        char::from_u32(c)
            .and_then(|ch| face.glyph_index(ch))
            .map_or(0, |g| g.0)
    }

    /// Advance of a glyph in points, `XeTeXFontInst::getGlyphWidth`.
    pub fn glyph_width(&self, gid: u16) -> f32 {
        let Some(face) = self.face() else { return 0.0 };
        let adv = face
            .glyph_hor_advance(ttf_parser::GlyphId(gid))
            .map_or(0.0, |a| a as f32);
        self.units_to_points(adv)
    }

    /// `XeTeXFontInst::getGlyphBounds` (cached per font).
    pub fn glyph_bbox(&self, gid: u16) -> GlyphBBox {
        if let Some(b) = self.bbox_cache.borrow().get(&gid) {
            return *b;
        }
        let mut b = GlyphBBox::default();
        if let Some(face) = self.face() {
            if let Some(r) = face.glyph_bounding_box(ttf_parser::GlyphId(gid)) {
                b.x_min = self.units_to_points(r.x_min as f32);
                b.y_min = self.units_to_points(r.y_min as f32);
                b.x_max = self.units_to_points(r.x_max as f32);
                b.y_max = self.units_to_points(r.y_max as f32);
            }
        }
        self.bbox_cache.borrow_mut().insert(gid, b);
        b
    }

    /// `getGlyphBounds(engine,..)`: x extents scaled by `extend`.
    pub fn engine_glyph_bbox(&self, gid: u16) -> GlyphBBox {
        let mut b = self.glyph_bbox(gid);
        if self.extend != 0.0 {
            b.x_min *= self.extend;
            b.x_max *= self.extend;
        }
        b
    }

    /// `getGlyphHeightDepth`
    pub fn glyph_height_depth(&self, gid: u16) -> (f32, f32) {
        let b = self.glyph_bbox(gid);
        (b.y_max, -b.y_min)
    }

    /// `getGlyphSidebearings` (with `extend`)
    pub fn glyph_sidebearings(&self, gid: u16) -> (f32, f32) {
        let w = self.glyph_width(gid);
        let b = self.glyph_bbox(gid);
        let (mut l, mut r) = (b.x_min, w - b.x_max);
        if self.extend != 0.0 {
            l *= self.extend;
            r *= self.extend;
        }
        (l, r)
    }

    /// `getGlyphItalCorr` (with `extend`)
    pub fn glyph_italic_correction(&self, gid: u16) -> f32 {
        let w = self.glyph_width(gid);
        let b = self.glyph_bbox(gid);
        let ic = if b.x_max > w { b.x_max - w } else { 0.0 };
        self.extend * ic
    }

    /// `getGlyphWidthFromEngine`
    pub fn engine_glyph_width(&self, gid: u16) -> f32 {
        self.extend * self.glyph_width(gid)
    }
}

struct RunGlyph {
    gid: u16,
    cluster: u32,
    x_adv: i32,
    y_adv: i32,
    x_off: i32,
    y_off: i32,
}

/// `layoutChars` for `text[range]` (byte range; clusters are byte offsets in `text`).
fn layout_chars(nf: &NativeFont, face: &crate::font_program::ShapeFace, text: &str, start: usize, end: usize, rtl: bool) -> Vec<RunGlyph> {
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    if start > 0 {
        buffer.set_pre_context(&text[..start]);
    }
    if end < text.len() {
        buffer.set_post_context(&text[end..]);
    }
    for (i, ch) in text[start..end].char_indices() {
        buffer.add(ch, (start + i) as u32);
    }
    buffer.set_direction(if nf.vertical {
        rustybuzz::Direction::TopToBottom
    } else if rtl {
        rustybuzz::Direction::RightToLeft
    } else {
        rustybuzz::Direction::LeftToRight
    });
    if let Some(script) = crate::native_font::ot_tag_to_script(nf.script) {
        buffer.set_script(script);
    }
    if let Some(lang) = nf.language.and_then(crate::native_font::ot_tag_to_language) {
        buffer.set_language(lang);
    }
    buffer.guess_segment_properties();
    let out = rustybuzz::shape(face, &nf.features, buffer);
    out.glyph_infos()
        .iter()
        .zip(out.glyph_positions())
        .map(|(i, p)| RunGlyph {
            gid: i.glyph_id as u16,
            cluster: i.cluster,
            x_adv: p.x_advance,
            y_adv: p.y_advance,
            x_off: p.x_offset,
            y_off: p.y_offset,
        })
        .collect()
}

/// Result of `measure_native_node`.
#[derive(Debug, Clone, Default)]
pub struct Measured {
    pub glyphs: Vec<NativeGlyph>,
    pub width: i32,
    pub height: i32,
    pub depth: i32,
}

/// Per-run glyph data in XeTeX's `locations` form.
struct Placed {
    gid: u16,
    cluster: u32,
    x: i32,
    y: i32,
    adv: i32,
}

/// `getGlyphPositions` + `getGlyphAdvances` of one laid-out run: returns the
/// per-glyph `(x, y)` positions in points, the advance in points and the
/// end position.
fn run_positions(nf: &NativeFont, glyphs: &[RunGlyph]) -> (Vec<(f32, f32)>, Vec<f32>, (f32, f32)) {
    let n = glyphs.len();
    let mut pos = Vec::with_capacity(n + 1);
    let mut adv = Vec::with_capacity(n);
    let (mut x, mut y) = (0f32, 0f32);
    if nf.vertical {
        for g in glyphs {
            pos.push((
                -nf.units_to_points(x + g.y_off as f32),
                nf.units_to_points(y - g.x_off as f32),
            ));
            adv.push(nf.units_to_points(g.y_adv as f32));
            x += g.y_adv as f32;
            y += g.x_adv as f32;
        }
        pos.push((-nf.units_to_points(x), nf.units_to_points(y)));
    } else {
        for g in glyphs {
            pos.push((
                nf.units_to_points(x + g.x_off as f32),
                -nf.units_to_points(y + g.y_off as f32),
            ));
            adv.push(nf.units_to_points(g.x_adv as f32));
            x += g.x_adv as f32;
            y += g.y_adv as f32;
        }
        pos.push((nf.units_to_points(x), -nf.units_to_points(y)));
    }
    if nf.extend != 1.0 || nf.slant != 0.0 {
        for p in pos.iter_mut() {
            p.0 = p.0 * nf.extend - p.1 * nf.slant;
        }
    }
    let end = pos[n];
    pos.truncate(n);
    (pos, adv, end)
}

/// `measure_native_node(node, use_glyph_metrics)` for `text` in font `nf`.
pub fn measure_native_word(nf: &NativeFont, text: &str, use_glyph_metrics: bool) -> Measured {
    let Some(face) = nf.face() else {
        return Measured { glyphs: Vec::new(), width: 0, height: nf.height_base, depth: nf.depth_base };
    };
    let mut placed: Vec<Placed> = Vec::new();
    let width_d: f64;
    // direction runs (ubidi): visual order list of (byte range, rtl)
    let runs: Vec<(usize, usize, bool)> = bidi_runs(text);
    if runs.len() == 1 {
        let (s, e, rtl) = runs[0];
        let g = layout_chars(nf, &face, text, s, e, rtl);
        let (pos, adv, end) = run_positions(nf, &g);
        for (i, rg) in g.iter().enumerate() {
            placed.push(Placed {
                gid: rg.gid,
                cluster: rg.cluster,
                x: d2fix(pos[i].0 as f64),
                y: d2fix(pos[i].1 as f64),
                adv: d2fix(adv[i] as f64),
            });
        }
        width_d = if g.is_empty() { 0.0 } else { end.0 as f64 };
    } else {
        let (mut x, mut y) = (0f64, 0f64);
        for (s, e, rtl) in runs {
            let g = layout_chars(nf, &face, text, s, e, rtl);
            let (pos, adv, end) = run_positions(nf, &g);
            for (i, rg) in g.iter().enumerate() {
                placed.push(Placed {
                    gid: rg.gid,
                    cluster: rg.cluster,
                    x: d2fix(pos[i].0 as f64 + x),
                    y: d2fix(pos[i].1 as f64 + y),
                    adv: d2fix(adv[i] as f64),
                });
            }
            x += end.0 as f64;
            y += end.1 as f64;
        }
        width_d = x;
    }
    let mut width = d2fix(width_d);
    if nf.letter_space != 0 && !placed.is_empty() {
        let unit = nf.letter_space;
        let mut delta = 0i32;
        for p in placed.iter_mut() {
            if p.adv == 0 && delta != 0 {
                delta -= unit;
            }
            p.x += delta;
            delta += unit;
        }
        if delta != 0 {
            delta -= unit;
            width += delta;
        }
    }
    let (height, depth) = if !use_glyph_metrics || placed.is_empty() {
        (nf.height_base, nf.depth_base)
    } else {
        let mut y_min = 65536.0f32;
        let mut y_max = -65536.0f32;
        for p in &placed {
            let y = (-(p.y as f64) / 65536.0) as f32;
            let b = nf.glyph_bbox(p.gid);
            let (ht, dp) = (b.y_max, -b.y_min);
            if y + ht > y_max {
                y_max = y + ht;
            }
            if y - dp < y_min {
                y_min = y - dp;
            }
        }
        (d2fix(y_max as f64), -d2fix(y_min as f64))
    };
    // clusters: end of each glyph's cluster in logical order
    let mut cl: Vec<u32> = placed.iter().map(|p| p.cluster).collect();
    cl.sort_unstable();
    cl.dedup();
    let n = placed.len();
    let glyphs = (0..n)
        .map(|i| {
            let c = placed[i].cluster;
            let ce = cl
                .iter()
                .find(|&&v| v > c)
                .copied()
                .unwrap_or(text.len() as u32);
            let next_x = if i + 1 < n { placed[i + 1].x } else { width };
            NativeGlyph {
                glyph_id: placed[i].gid,
                cluster_start: c,
                cluster_end: ce,
                x_advance: next_x - placed[i].x,
                y_advance: 0,
                x_offset: 0,
                y_offset: -placed[i].y,
            }
        })
        .collect();
    Measured { glyphs, width, height, depth }
}

/// The direction runs of `text` in visual order (`ubidi_setPara` with a
/// left-to-right default).
fn bidi_runs(text: &str) -> Vec<(usize, usize, bool)> {
    if text.is_empty() || !text.chars().any(|c| (c as u32) >= 0x590 && is_strong_rtl_or_arabic_number(c)) {
        return vec![(0, text.len(), false)];
    }
    let info = unicode_bidi::BidiInfo::new(text, Some(unicode_bidi::Level::ltr()));
    let Some(para) = info.paragraphs.first() else {
        return vec![(0, text.len(), false)];
    };
    if info.levels.iter().all(|l| !l.is_rtl()) {
        return vec![(0, text.len(), false)];
    }
    if info.levels.iter().all(|l| l.is_rtl()) {
        return vec![(0, text.len(), true)];
    }
    let (levels, runs) = info.visual_runs(para, para.range.clone());
    runs.into_iter()
        .map(|r| (r.start, r.end, levels[r.start].is_rtl()))
        .collect()
}

fn is_strong_rtl_or_arabic_number(c: char) -> bool {
    use unicode_bidi::BidiClass::*;
    matches!(unicode_bidi::bidi_class(c), R | AL | AN | RLE | RLO | RLI)
}

/// Whether the font is laid out through the OT shaper (Ratex has no AAT or
/// Graphite renderer: those requests fall back to OpenType shaping).
pub fn uses_ot(nf: &NativeFont) -> bool {
    nf.req_engine != ReqEngine::Aat
}

/// Width, height and depth of a glyph slice (used by the generic shaping
/// entry points of `fontiface`).
pub fn calculate_slice_dims(
    glyphs: &[NativeGlyph],
    face: &ttf_parser::Face<'_>,
    at_size: i32,
    upem: i64,
) -> (i32, i32, i32) {
    let scale_to_sp = |val: i32| -> i32 { (val as i64 * at_size as i64 / upem) as i32 };
    let (mut w, mut h, mut d) = (0, 0, 0);
    for g in glyphs {
        w += g.x_advance;
        let gid = ttf_parser::GlyphId(g.glyph_id);
        if let Some(rect) = face.glyph_bounding_box(gid) {
            let gh = scale_to_sp(rect.y_max as i32) + g.y_offset;
            let gd = scale_to_sp(-rect.y_min as i32) - g.y_offset;
            h = h.max(gh.max(0));
            d = d.max(gd.max(0));
        } else {
            h = h.max(scale_to_sp(face.ascender() as i32).max(0));
            d = d.max(scale_to_sp(-face.descender() as i32).max(0));
        }
    }
    (w, h, d)
}
