//! Native OpenType/TrueType layout, shaping, and line-breaking integration.
//!
//! Provides `NativeGlyph`, `NativeRun`, and `NativeTextState` for real Unicode/NFSS
//! text typesetting, UTF-8 cluster tracking, CJK punctuation-aware line breaking,
//! and CJK/Latin spacing.

use crate::boxes::{Glue, Node};
use crate::engine::Engine;
use crate::tfm::FontId;
use std::rc::Rc;
use unicode_segmentation::UnicodeSegmentation;
/// A shaped glyph with scaled TeX sp metrics and original source UTF-8 cluster offsets.
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
}

/// Private buffer state owned by `Engine::native_text`.
#[derive(Default, Debug)]
pub struct NativeTextState {
    /// FontId the currently buffered text is targeted for
    pub current_font: Option<FontId>,
    /// Accumulated source text
    pub buffer: String,
    /// Tracks if last appended character was CJK (for CJK/Latin spacing)
    pub last_was_cjk: Option<bool>,
    /// Tracks if last appended character was RTL
    pub last_was_rtl: Option<bool>,
    /// \noboundary flag: suppress next left boundary ligature/kern
    pub suppress_left_boundary: bool,
    /// \noboundary flag: suppress right boundary ligature/kern
    pub suppress_right_boundary: bool,
    /// \noboundary flag: do not form ligature with previous character
    pub no_lig_prev: bool,
}

impl NativeTextState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn clear(&mut self) {
        self.current_font = None;
        self.buffer.clear();
        self.last_was_cjk = None;
        self.last_was_rtl = None;
        self.suppress_left_boundary = false;
        self.suppress_right_boundary = false;
        self.no_lig_prev = false;
    }
}

/// Check if a character belongs to a CJK script (Unified Ideographs, Kana, Hangul, Bopomofo, Radicals).
pub fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2E80..=0x2FD5 // CJK Radicals / Kangxi Radicals
        | 0x2FF0..=0x2FFF // Ideographic Description Characters
        | 0x3000..=0x303F // CJK Symbols and Punctuation
        | 0x3040..=0x309F // Hiragana
        | 0x30A0..=0x30FF // Katakana
        | 0x3100..=0x312F // Bopomofo
        | 0x3130..=0x318F // Hangul Compatibility Jamo
        | 0x3190..=0x319F // Kanbun
        | 0x31A0..=0x31BF // Bopomofo Extended
        | 0x31C0..=0x31EF // CJK Strokes
        | 0x31F0..=0x31FF // Katakana Phonetic Extensions
        | 0x3200..=0x32FF // Enclosed CJK Letters and Months
        | 0x3300..=0x33FF // CJK Compatibility
        | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0xFE30..=0xFE4F // CJK Compatibility Forms
        | 0xFE50..=0xFE6F // Small Form Variants
        | 0xFF00..=0xFFEF // Halfwidth and Fullwidth Forms
        | 0xAC00..=0xD7AF // Hangul Syllables
        | 0x1100..=0x11FF // Hangul Jamo
        | 0xA960..=0xA97F // Hangul Jamo Extended-A
        | 0xD7B0..=0xD7FF // Hangul Jamo Extended-B
        | 0x20000..=0x2CEAF // CJK Unified Ideographs Extensions B-E
        | 0x2CEB0..=0x2EBEF // CJK Unified Ideographs Extension F
        | 0x2F800..=0x2FA1F // CJK Compatibility Ideographs Supplement
        | 0x30000..=0x323AF // CJK Unified Ideographs Extensions G-H
    )
}

/// Check if a character is CJK punctuation.
pub fn is_cjk_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '、' | '，'
            | '。'
            | '．'
            | '・'
            | '：'
            | '；'
            | '？'
            | '！'
            | '「'
            | '」'
            | '『'
            | '』'
            | '（'
            | '）'
            | '〔'
            | '〕'
            | '【'
            | '】'
            | '《'
            | '》'
            | '〈'
            | '〉'
            | '〖'
            | '〗'
            | '〘'
            | '〙'
            | '〚'
            | '〛'
            | '～'
            | '—'
            | '…'
            | '‥'
            | '“'
            | '”'
            | '‘'
            | '’'
            | '｀'
    )
}

/// Line-start forbidden characters in CJK (Kinsoku Shori line-start prohibition).
pub fn is_line_start_forbidden(ch: char) -> bool {
    matches!(
        ch,
        ')' | ']'
            | '}'
            | '）'
            | '］'
            | '｝'
            | '、'
            | '，'
            | '。'
            | '．'
            | '！'
            | '？'
            | '：'
            | '；'
            | '”'
            | '’'
            | '»'
            | '›'
            | '」'
            | '』'
            | '〕'
            | '〉'
            | '》'
            | '】'
            | '〗'
            | '〙'
            | '〛'
            | '〜'
            | '…'
            | '‥'
            | 'ー'
            | '々'
            | 'ゝ'
            | 'ヽ'
            | 'ゞ'
            | 'ヾ'
    )
}

/// Line-end forbidden characters in CJK (Kinsoku Shori line-end prohibition).
pub fn is_line_end_forbidden(ch: char) -> bool {
    matches!(
        ch,
        '(' | '['
            | '{'
            | '（'
            | '［'
            | '｛'
            | '“'
            | '‘'
            | '«'
            | '‹'
            | '「'
            | '『'
            | '〔'
            | '〈'
            | '《'
            | '【'
            | '〖'
            | '〘'
            | '〚'
    )
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

/// Replace classic TeX ligature patterns with Unicode characters while building a byte offset map.
///
/// Maps byte ranges in the transformed text back to `(start, end)` byte ranges in the original text.
fn apply_tex_ligatures(source: &str) -> (String, Vec<(u32, u32)>) {
    let mut out = String::with_capacity(source.len());
    let mut mapping = Vec::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i..].starts_with(b"---") {
            let start = i as u32;
            let end = (i + 3) as u32;
            let ch = '—'; // U+2014 em-dash
            let prev_len = out.len();
            out.push(ch);
            for _ in prev_len..out.len() {
                mapping.push((start, end));
            }
            i += 3;
        } else if bytes[i..].starts_with(b"--") {
            let start = i as u32;
            let end = (i + 2) as u32;
            let ch = '–'; // U+2013 en-dash
            let prev_len = out.len();
            out.push(ch);
            for _ in prev_len..out.len() {
                mapping.push((start, end));
            }
            i += 2;
        } else if bytes[i..].starts_with(b"''") {
            let start = i as u32;
            let end = (i + 2) as u32;
            let ch = '”'; // U+201D right double quotation mark
            let prev_len = out.len();
            out.push(ch);
            for _ in prev_len..out.len() {
                mapping.push((start, end));
            }
            i += 2;
        } else if bytes[i..].starts_with(b"``") {
            let start = i as u32;
            let end = (i + 2) as u32;
            let ch = '“'; // U+201C left double quotation mark
            let prev_len = out.len();
            out.push(ch);
            for _ in prev_len..out.len() {
                mapping.push((start, end));
            }
            i += 2;
        } else if bytes[i..].starts_with(b"!`") {
            let start = i as u32;
            let end = (i + 2) as u32;
            let ch = '¡'; // U+00A1 inverted exclamation mark
            let prev_len = out.len();
            out.push(ch);
            for _ in prev_len..out.len() {
                mapping.push((start, end));
            }
            i += 2;
        } else if bytes[i..].starts_with(b"?`") {
            let start = i as u32;
            let end = (i + 2) as u32;
            let ch = '¿'; // U+00BF inverted question mark
            let prev_len = out.len();
            out.push(ch);
            for _ in prev_len..out.len() {
                mapping.push((start, end));
            }
            i += 2;
        } else if bytes[i] == b'\'' {
            let start = i as u32;
            let end = (i + 1) as u32;
            let ch = '’'; // U+2019 right single quotation mark
            let prev_len = out.len();
            out.push(ch);
            for _ in prev_len..out.len() {
                mapping.push((start, end));
            }
            i += 1;
        } else if bytes[i] == b'`' {
            let start = i as u32;
            let end = (i + 1) as u32;
            let ch = '‘'; // U+2018 left single quotation mark
            let prev_len = out.len();
            out.push(ch);
            for _ in prev_len..out.len() {
                mapping.push((start, end));
            }
            i += 1;
        } else {
            let start = i as u32;
            let ch = source[i..].chars().next().unwrap();
            let char_len = ch.len_utf8();
            let end = (i + char_len) as u32;
            let prev_len = out.len();
            out.push(ch);
            for _ in prev_len..out.len() {
                mapping.push((start, end));
            }
            i += char_len;
        }
    }

    (out, mapping)
}

impl Engine {
    /// True if native font layout is active for the current font or scoped CJK binding.
    pub fn native_text_active(&self) -> bool {
        if self.font_loader.native_fonts.is_empty() {
            return false;
        }
        if self
            .font_loader
            .native_fonts
            .contains_key(&self.eqtb.cur_font_val)
        {
            return true;
        }
        if let Some(cjk_fid) = self.current_cjk_native_font() {
            return self.font_loader.native_fonts.contains_key(&cjk_fid);
        }
        false
    }

    /// Retrieve the scoped CJK native font if `\ratex@cjkfont` is bound.
    pub fn current_cjk_native_font(&self) -> Option<FontId> {
        let cs = self.cs.lookup(b"ratex@cjkfont")?;
        match self.eqtb.resolve(cs) {
            Some(crate::eqtb::Equiv::FontRef(fid)) => {
                if self.font_loader.native_fonts.contains_key(fid) {
                    Some(*fid)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Append a character to the native layout pipeline.
    ///
    /// Routes CJK text/punctuation to `ratex@cjkfont` when active while preserving Latin
    /// font selections. Implements CJK/Latin spacing glue on script transitions.
    /// Returns `true` if handled, `false` to fall back to the classic byte path.
    pub fn append_native_char(&mut self, scalar: u32) -> bool {
        if !self.native_text_active() {
            return false;
        }

        let Some(ch) = char::from_u32(scalar) else {
            self.error(&format!("Invalid Unicode scalar value {scalar:#X}"));
            return true;
        };

        let ch_is_rtl = char_bidi_is_rtl(ch);
        if let Some(last_rtl) = self.native_text.last_was_rtl {
            if last_rtl != ch_is_rtl {
                self.flush_native_text();
            }
        }
        self.native_text.last_was_rtl = Some(ch_is_rtl);

        let ch_is_cjk = is_cjk(ch);
        let ch_is_punct = is_cjk_punctuation(ch);

        // Determine target font:
        // CJK text/punctuation uses ratex@cjkfont if bound, else current font.
        // Latin/other text uses current font.
        let target_font = if ch_is_cjk || ch_is_punct {
            self.current_cjk_native_font()
                .unwrap_or(self.eqtb.cur_font_val)
        } else {
            self.eqtb.cur_font_val
        };

        // If target font is not native, flush native buffer and let classic path handle it
        if !self.font_loader.native_fonts.contains_key(&target_font) {
            self.flush_native_text();
            if self.native_text.last_was_cjk == Some(true)
                && !ch_is_punct
                && self.current_cjk_native_font().is_some()
            {
                self.insert_cjk_latin_glue();
            }
            self.native_text.last_was_cjk = Some(false);
            return false;
        }

        // CJK / Latin spacing transition
        if let Some(last_cjk) = self.native_text.last_was_cjk {
            if last_cjk && !ch_is_cjk && !ch_is_punct && self.current_cjk_native_font().is_some() {
                // Transition CJK -> Latin: flush CJK and insert inter-script space
                self.flush_native_text();
                self.insert_cjk_latin_glue();
            } else if !last_cjk
                && ch_is_cjk
                && !ch_is_punct
                && self.current_cjk_native_font().is_some()
            {
                // Transition Latin -> CJK: flush Latin and insert inter-script space
                self.flush_native_text();
                self.insert_cjk_latin_glue();
            }
        }

        // If switching fonts within native layout, flush buffer first
        if self.native_text.current_font != Some(target_font) {
            self.flush_native_text();
            self.native_text.current_font = Some(target_font);
        }

        self.native_text.buffer.push(ch);
        self.native_text.last_was_cjk = Some(ch_is_cjk);
        true
    }

    /// Append a single literal character bypassing TeX ligature substitutions.
    ///
    /// Used by `\RatexLiteralChar` / TU `\textquotesingle`. Flushes pending text,
    /// selects the current/CJK native face, emits literal cmap glyph with real metrics,
    /// and returns `false` if target font is not native.
    pub fn append_native_literal_char(&mut self, scalar: u32) -> bool {
        if !self.native_text_active() {
            return false;
        }

        let Some(ch) = char::from_u32(scalar) else {
            self.error(&format!("Invalid Unicode scalar value {scalar:#X}"));
            return true;
        };

        self.flush_native_text();

        let ch_is_cjk = is_cjk(ch);
        let target_font = if ch_is_cjk || is_cjk_punctuation(ch) {
            self.current_cjk_native_font()
                .unwrap_or(self.eqtb.cur_font_val)
        } else {
            self.eqtb.cur_font_val
        };

        let Some(native_font) = self.font_loader.native_fonts.get(&target_font).cloned() else {
            return false;
        };

        // Shape single literal character bypassing TeX ligatures using the shared shaping path
        let mut literal_font = (*native_font).clone();
        literal_font.tex_ligatures = false;
        let char_str = ch.to_string();

        match self.shape_native_run(target_font, &literal_font, &char_str) {
            Ok((run, _)) => {
                let total = run.glyphs.len();
                let at_size = self
                    .eqtb
                    .fonts
                    .get(target_font as usize)
                    .map(|f| f.at_size)
                    .unwrap_or(655360);
                let upem = literal_font.program.units_per_em.max(1) as i64;
                let (w, h, d) = match literal_font.program.face() {
                    Ok(face) => calculate_slice_dims(&run.glyphs, &face, at_size, upem),
                    Err(_) => (0, 0, 0),
                };
                self.cur_list.push(Node::NativeGlyphRun {
                    run,
                    start: 0,
                    end: total,
                    width: w,
                    height: h,
                    depth: d,
                });
                self.native_text.last_was_cjk = Some(ch_is_cjk);
                true
            }
            Err(err) => {
                self.error(&err);
                true
            }
        }
    }

    /// Flush any buffered native text into shaped layout nodes.
    pub fn flush_native_text(&mut self) {
        if self.native_text.buffer.is_empty() {
            return;
        }

        let font_id = match self.native_text.current_font {
            Some(fid) => fid,
            None => {
                self.native_text.buffer.clear();
                return;
            }
        };

        let raw_text = std::mem::take(&mut self.native_text.buffer);
        let Some(native_font) = self.font_loader.native_fonts.get(&font_id).cloned() else {
            self.error(&format!("Font {} is not a registered native font", font_id));
            return;
        };

        match self.shape_native_run_nodes(font_id, &native_font, &raw_text) {
            Ok(nodes) => {
                self.cur_list.extend(nodes);
            }
            Err(err) => {
                self.error(&err);
            }
        }
    }

    /// Shared single shaping path: shapes raw text with rustybuzz using the native font's
    /// selected face, variations, script, language, and OpenType features.
    ///
    /// Preserves full original UTF-8 byte cluster ranges, handles default-ignorables deliberately,
    /// checks for missing glyphs (returning Err on visible GID 0), and returns the shared
    /// `Rc<NativeRun>` along with the set of glyph indices marked unsafe to break by HarfBuzz.
    pub fn shape_native_run(
        &self,
        font_id: FontId,
        native_font: &crate::native_font::NativeFont,
        raw_text: &str,
    ) -> Result<(Rc<NativeRun>, std::collections::HashSet<usize>), String> {
        if raw_text.is_empty() {
            let run = Rc::new(NativeRun {
                font: font_id,
                text: Rc::from(""),
                glyphs: Vec::new(),
            });
            return Ok((run, std::collections::HashSet::new()));
        }

        // Apply TeX ligatures if enabled while maintaining exact byte mapping to raw_text
        let (shaped_text, cluster_map) = if native_font.tex_ligatures {
            apply_tex_ligatures(raw_text)
        } else {
            let mut mapping = Vec::with_capacity(raw_text.len());
            let mut i = 0;
            while i < raw_text.len() {
                let start = i as u32;
                let ch = raw_text[i..].chars().next().unwrap();
                let char_len = ch.len_utf8();
                let end = (i + char_len) as u32;
                for _ in 0..char_len {
                    mapping.push((start, end));
                }
                i += char_len;
            }
            (raw_text.to_string(), mapping)
        };

        let mut buzz_face =
            rustybuzz::Face::from_slice(&native_font.program.data, native_font.program.face_index)
                .ok_or_else(|| "Failed to load rustybuzz face from program bytes".to_string())?;

        // Apply variation coordinates
        if !native_font.program.variations.is_empty() {
            let variations: Vec<rustybuzz::Variation> = native_font
                .program
                .variations
                .iter()
                .map(|&(tag, value)| rustybuzz::Variation { tag, value })
                .collect();
            buzz_face.set_variations(&variations);
        }

        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(&shaped_text);
        let is_rtl = raw_text.chars().any(char_bidi_is_rtl);
        let is_vertical = native_font.vertical;
        if is_vertical {
            buffer.set_direction(rustybuzz::Direction::TopToBottom);
        } else if is_rtl {
            buffer.set_direction(rustybuzz::Direction::RightToLeft);
        } else {
            buffer.set_direction(rustybuzz::Direction::LeftToRight);
        }

        if let Some(script) = native_font.script.or_else(|| detect_script(&shaped_text)) {
            buffer.set_script(script);
        }
        if let Some(language) = native_font.language.clone() {
            buffer.set_language(language);
        }

        let glyph_buffer = rustybuzz::shape(&buzz_face, &native_font.features, buffer);
        let infos = glyph_buffer.glyph_infos();
        let positions = glyph_buffer.glyph_positions();

        let at_size = self
            .eqtb
            .fonts
            .get(font_id as usize)
            .map(|f| f.at_size)
            .unwrap_or(655360);
        let upem = native_font.program.units_per_em.max(1) as i64;
        let scale_to_sp = |val: i32| -> i32 { (val as i64 * at_size as i64 / upem) as i32 };

        let mut native_glyphs = Vec::with_capacity(infos.len());
        let mut unsafe_breaks = std::collections::HashSet::new();

        for (i, (info, pos)) in infos.iter().zip(positions.iter()).enumerate() {
            let gid = info.glyph_id as u16;
            let cluster_idx = info.cluster as usize;

            let orig_start = if cluster_idx < cluster_map.len() {
                cluster_map[cluster_idx].0
            } else {
                raw_text.len() as u32
            };

            // Find next distinct cluster in infos to establish full cluster_end
            let next_cluster_idx = (i + 1..infos.len())
                .find(|&j| infos[j].cluster != info.cluster)
                .map(|j| infos[j].cluster as usize);

            let orig_end = if let Some(idx) = next_cluster_idx {
                if idx < cluster_map.len() {
                    cluster_map[idx].0
                } else {
                    raw_text.len() as u32
                }
            } else {
                raw_text.len() as u32
            };

            let orig_end = orig_end.max(if cluster_idx < cluster_map.len() {
                cluster_map[cluster_idx].1
            } else {
                orig_start
            });

            // Check for GID 0 on visible printable text
            if gid == 0 {
                let char_slice =
                    &raw_text[orig_start as usize..orig_end.min(raw_text.len() as u32) as usize];
                let is_ignorable = char_slice
                    .chars()
                    .all(|c| is_default_ignorable(c) || c.is_whitespace());
                if !is_ignorable {
                    let first_char = char_slice.chars().next().unwrap_or('?');
                    return Err(format!(
                        "Missing glyph in font for character '{}' (U+{:04X})",
                        first_char, first_char as u32
                    ));
                }
            }

            if info.unsafe_to_break() {
                unsafe_breaks.insert(i);
            }

            native_glyphs.push(NativeGlyph {
                glyph_id: gid,
                cluster_start: orig_start,
                cluster_end: orig_end,
                x_advance: scale_to_sp(pos.x_advance),
                y_advance: scale_to_sp(pos.y_advance),
                x_offset: scale_to_sp(pos.x_offset),
                y_offset: scale_to_sp(pos.y_offset),
            });
        }

        let run = Rc::new(NativeRun {
            font: font_id,
            text: Rc::from(raw_text),
            glyphs: native_glyphs,
        });

        Ok((run, unsafe_breaks))
    }

    /// Shape a native text string with rustybuzz and split into shared run slices at legal break points.
    pub fn shape_native_run_nodes(
        &self,
        font_id: FontId,
        native_font: &crate::native_font::NativeFont,
        raw_text: &str,
    ) -> Result<Vec<Node>, String> {
        if raw_text.is_empty() {
            return Ok(Vec::new());
        }

        let (run, unsafe_breaks) = self.shape_native_run(font_id, native_font, raw_text)?;
        let total_glyphs = run.glyphs.len();
        if total_glyphs == 0 {
            return Ok(Vec::new());
        }

        let face = native_font.program.face()?;
        let at_size = self
            .eqtb
            .fonts
            .get(font_id as usize)
            .map(|f| f.at_size)
            .unwrap_or(655360);
        let upem = native_font.program.units_per_em.max(1) as i64;

        // Collect grapheme boundaries in raw_text
        let grapheme_boundaries: std::collections::HashSet<usize> = raw_text
            .grapheme_indices(true)
            .map(|(idx, _)| idx)
            .collect();

        // Collect allowed break opportunities via unicode_linebreak
        let mut allowed_byte_breaks = std::collections::HashSet::new();
        for (offset, opp) in unicode_linebreak::linebreaks(raw_text) {
            if opp == unicode_linebreak::BreakOpportunity::Allowed
                || opp == unicode_linebreak::BreakOpportunity::Mandatory
            {
                allowed_byte_breaks.insert(offset);
            }
        }

        let mut nodes = Vec::new();
        let mut slice_start = 0;

        for i in 1..total_glyphs {
            let prev_start = run.glyphs[i - 1].cluster_start as usize;
            let curr_start = run.glyphs[i].cluster_start as usize;

            let prev_char = raw_text[prev_start..].chars().next().unwrap_or(' ');
            let curr_char = raw_text[curr_start..].chars().next().unwrap_or(' ');

            // Line break between glyphs i-1 and i is allowed ONLY if:
            // 1. Glyphs belong to different source clusters
            // 2. curr_start is at a grapheme boundary (never split combining marks)
            // 3. curr_start is an allowed Unicode linebreak opportunity
            // 4. HarfBuzz shaping did not mark glyph i as unsafe to break
            // 5. CJK punctuation rules are respected (no break after opening punct, no break before closing punct)
            let is_break_allowed = curr_start > prev_start
                && grapheme_boundaries.contains(&curr_start)
                && allowed_byte_breaks.contains(&curr_start)
                && !unsafe_breaks.contains(&i)
                && !is_line_end_forbidden(prev_char)
                && !is_line_start_forbidden(curr_char);

            if is_break_allowed {
                let (w, h, d) =
                    calculate_slice_dims(&run.glyphs[slice_start..i], &face, at_size, upem);
                nodes.push(Node::NativeGlyphRun {
                    run: Rc::clone(&run),
                    start: slice_start,
                    end: i,
                    width: w,
                    height: h,
                    depth: d,
                });
                if is_cjk(prev_char)
                    && is_cjk(curr_char)
                    && self.current_cjk_native_font() == Some(font_id)
                {
                    // xeCJK's default inter-character glue: 0pt plus 0.08 baselineskip.
                    let baseline = self.eqtb.glue_params
                        [crate::prim::GlueParam::BaselineSkip.idx() as usize]
                        .width;
                    nodes.push(Node::Glue(Glue {
                        width: 0,
                        stretch: (baseline as i64 * 8 / 100) as i32,
                        shrink: 0,
                        stretch_order: 0,
                        shrink_order: 0,
                    }));
                } else {
                    nodes.push(Node::Penalty(0));
                }
                slice_start = i;
            }
        }

        let (w, h, d) =
            calculate_slice_dims(&run.glyphs[slice_start..total_glyphs], &face, at_size, upem);
        nodes.push(Node::NativeGlyphRun {
            run,
            start: slice_start,
            end: total_glyphs,
            width: w,
            height: h,
            depth: d,
        });

        Ok(nodes)
    }

    /// Shape a single string slice into layout nodes for native discretionary/hyphenation fragments.
    pub fn shape_native_slice(&self, font_id: FontId, text: &str) -> Result<Vec<Node>, String> {
        let native_font = self
            .font_loader
            .native_fonts
            .get(&font_id)
            .ok_or_else(|| format!("Font {} is not a registered native font", font_id))?;

        let (run, _) = self.shape_native_run(font_id, native_font, text)?;
        let total_glyphs = run.glyphs.len();
        if total_glyphs == 0 {
            return Ok(Vec::new());
        }

        let face = native_font.program.face()?;
        let at_size = self
            .eqtb
            .fonts
            .get(font_id as usize)
            .map(|f| f.at_size)
            .unwrap_or(655360);
        let upem = native_font.program.units_per_em.max(1) as i64;
        let (w, h, d) = calculate_slice_dims(&run.glyphs, &face, at_size, upem);

        Ok(vec![Node::NativeGlyphRun {
            run,
            start: 0,
            end: total_glyphs,
            width: w,
            height: h,
            depth: d,
        }])
    }

    /// xeCJK uses ordinary Latin interword space, never in addition to explicit space.
    fn insert_cjk_latin_glue(&mut self) {
        if matches!(self.cur_list.last(), Some(Node::Glue(_))) {
            return;
        }
        let glue = self.interword_glue();
        self.cur_list.push(Node::Glue(glue));
    }

    /// Check whether a character is present in the specified font.
    pub fn native_char_present(&self, font: FontId, scalar: u32) -> Option<bool> {
        let native_font = self.font_loader.native_fonts.get(&font)?;
        let face = native_font.program.face().ok()?;
        let ch = char::from_u32(scalar)?;
        let gid = face.glyph_index(ch)?;
        Some(gid.0 != 0)
    }

    /// Return `(width, height, depth, italic)` for a character in the specified font (all in sp).
    pub fn native_char_dimensions(
        &self,
        font: FontId,
        scalar: u32,
    ) -> Option<(i32, i32, i32, i32)> {
        let native_font = self.font_loader.native_fonts.get(&font)?;
        let face = native_font.program.face().ok()?;
        let ch = char::from_u32(scalar)?;
        let gid = face.glyph_index(ch)?;
        if gid.0 == 0 {
            return None;
        }
        let tfm_font = self.eqtb.fonts.get(font as usize)?;
        let at_size = tfm_font.at_size;
        let upem = native_font.program.units_per_em as i64;
        let scale_to_sp = |val: i32| -> i32 { (val as i64 * at_size as i64 / upem) as i32 };

        let adv = scale_to_sp(face.glyph_hor_advance(gid).unwrap_or(0) as i32);
        let (ht, dp) = if let Some(rect) = face.glyph_bounding_box(gid) {
            let h = scale_to_sp(rect.y_max as i32).max(0);
            let d = scale_to_sp(-rect.y_min as i32).max(0);
            (h, d)
        } else {
            let h = scale_to_sp(face.ascender() as i32).max(0);
            let d = scale_to_sp(-face.descender() as i32).max(0);
            (h, d)
        };

        Some((adv, ht, dp, 0))
    }
}

pub fn calculate_slice_dims(
    glyphs: &[NativeGlyph],
    face: &ttf_parser::Face<'_>,
    at_size: i32,
    upem: i64,
) -> (i32, i32, i32) {
    let scale_to_sp = |val: i32| -> i32 { (val as i64 * at_size as i64 / upem) as i32 };

    let mut w = 0;
    let mut h = 0;
    let mut d = 0;

    for g in glyphs {
        w += g.x_advance;
        let gid = ttf_parser::GlyphId(g.glyph_id);
        if let Some(rect) = face.glyph_bounding_box(gid) {
            let gh = scale_to_sp(rect.y_max as i32) + g.y_offset;
            let gd = scale_to_sp(-rect.y_min as i32) - g.y_offset;
            h = h.max(gh.max(0));
            d = d.max(gd.max(0));
        } else {
            let asc = scale_to_sp(face.ascender() as i32);
            let desc = scale_to_sp(-face.descender() as i32);
            h = h.max(asc.max(0));
            d = d.max(desc.max(0));
        }
    }

    (w, h, d)
}
