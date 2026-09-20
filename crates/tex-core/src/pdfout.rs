use crate::boxes::{Node, WhatIt};
use crate::engine::Engine;
pub use crate::pdffile::PdfEncryptConfig;
use crate::token::Token;

// PDF document model: pages, annotations, destinations, embedded fonts.
// Serialization lives in `pdffile`; page rendering in `pdfrender`.
/// A link (or generic) annotation attached to a page.
#[derive(Clone, Debug)]
pub struct Annot {
    /// [x0 y0 x1 y1] in PDF user space (y grows upward), in bp
    pub rect: [f64; 4],
    /// external URL: serialized as /A <</S /URI /URI (...)>>
    pub uri: Option<String>,
    /// internal named destination: serialized as /Dest (name)
    pub dest: Option<String>,
    /// extra key/value dict body from \pdfstartlink user{...}
    pub attr: String,
    /// explicit /Subtype value; `None` falls back to /Link (the annots
    /// produced by \pdfstartlink and plain \pdfannot)
    pub subtype: Option<String>,
}

/// A named destination anchored at a page point.
#[derive(Clone, Debug)]
pub struct Dest {
    pub name: String,
    /// anchor / explicit position in bp, bottom-origin
    pub x: f64,
    pub y: f64,
    /// PDF dest type: 0 /XYZ, 1 /Fit, 2 /FitH, 3 /FitV, 4 /FitB,
    /// 5 /FitBH, 6 /FitBV, 7 /FitR
    pub kind: u8,
    /// /XYZ zoom factor (None = null)
    pub zoom: Option<f64>,
}

/// A compiled PDF page awaiting serialization.
pub struct PdfPage {
    pub content: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub width_bp: f64,
    pub height_bp: f64,
    pub annots: Vec<Annot>,
    /// (doc font index, resource number) — resolved by `embed_used_fonts`
    pub fonts: Vec<(usize, u16)>,
    /// named destinations anchored on this page
    pub dests: Vec<Dest>,
    /// raw dict body contributed by \pdfpageattr (copied at shipout)
    pub attr_extra: Vec<u8>,
    /// raw dict entries contributed by \pdfpageresources (copied at shipout)
    pub resources_extra: Vec<u8>,
    pub display_list: Option<crate::boxes::DisplayList>,
}

impl PdfPage {
    pub fn new(width: i32, height: i32) -> Self {
        PdfPage {
            content: Vec::new(),
            width,
            height,
            width_bp: width as f64,
            height_bp: height as f64,
            annots: Vec::new(),
            fonts: Vec::new(),
            dests: Vec::new(),
            attr_extra: Vec::new(),
            resources_extra: Vec::new(),
            display_list: None,
        }
    }
}

pub struct PdfDoc {
    compression_worker: Option<crate::pdfcompress::Worker>,
    page_compression: Vec<Option<crate::pdfcompress::Pending>>,
    pub pages: Vec<PdfPage>,
    /// raw user objects from \pdfobj
    pub objects: Vec<(i32, Vec<u8>)>,
    /// Reserved font dictionaries for forms, with the same font-index
    /// remapping as pages. Each form has its own resource namespace.
    pub form_fonts: Vec<(i32, Vec<(usize, u16)>)>,
    pub info: Vec<u8>,
    /// raw dict body contributed by \pdfcatalog
    pub catalog_extra: Vec<u8>,
    /// raw dict entries contributed by \pdfnames (merged into /Names)
    pub names_extra: Vec<u8>,
    /// raw dict body contributed by \pdfpagesattr (written to the page tree)
    pub pages_attr: Vec<u8>,
    /// (title, dest name, count) from \pdfoutline
    pub outlines: Vec<(String, String, i32)>,
    /// loaded embedded fonts
    /// open action: (1-based page number, view name) from \pdfcatalog's
    /// keyword form `openaction goto page <n> {view}` (hyperref PDF@SetupDoc)
    pub open_action: Option<(i32, String)>,
    /// loaded embedded fonts
    pub fonts: Vec<EmbedFont>,
    /// Character codes used by each engine font before page/form font ids
    /// are remapped to entries in `fonts`. This lets the serializer retain
    /// only the required Type 1 glyph programs.
    pub font_chars: std::collections::BTreeMap<usize, [u64; 4]>,
    pub encrypt: Option<PdfEncryptConfig>,
    pub pdfa: bool,
    pub minor_version: Option<i32>,
    pub native_bindings: std::collections::BTreeMap<usize, Vec<NativeBindingInfo>>,
    pub legacy_bindings: std::collections::BTreeMap<usize, Vec<LegacyBindingInfo>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EmbedFontSubtype {
    Type1,
    TrueType,
    Cff,
}

#[derive(Clone, Debug)]
pub struct NativeBindingInfo {
    pub code_map: std::collections::HashMap<u16, smallvec::SmallVec<[usize; 1]>>,
    pub entries: Vec<(u16, u16, String)>,
    pub next_code: u32,
}
#[derive(Clone, Debug)]
pub struct LegacyBindingInfo {
    pub code_map: std::collections::HashMap<u8, smallvec::SmallVec<[usize; 1]>>,
    pub entries: Vec<(u8, u8, String)>,
    pub next_code: u16,
}

/// Keep the engine font in the low bits and its native code-space shard above it.
pub(crate) fn font_resource_key(font_id: u16, binding: usize) -> usize {
    (binding << u16::BITS) | usize::from(font_id)
}

/// A font prepared for embedding (Type 1 or OpenType/TrueType/CFF).
pub struct EmbedFont {
    pub obj_font: i32,
    pub base_font: String,
    /// full Type 1 program or raw font data
    pub font_file: std::rc::Rc<Vec<u8>>,
    pub length1: usize,
    pub length2: usize,
    pub length3: usize,
    pub is_truetype: bool,
    pub subtype: EmbedFontSubtype,
    pub face_index: u32,
    pub variations: Vec<(ttf_parser::Tag, f32)>,
    pub allow_subsetting: bool,
    pub content_hash: [u8; 16],
    pub units_per_em: u16,
    /// glyph names by slot (None = the font's built-in encoding)
    pub encoding_diff: Option<Vec<String>>,
    pub first_char: u8,
    pub last_char: u8,
    /// widths in 1/10000 font units, for first_char..=last_char
    pub widths: Vec<i32>,
    pub font_matrix_scale: f64,
    /// FontDescriptor metrics (1/1000 font units, degrees for the angle)
    pub font_bbox: [f64; 4],
    pub italic_angle: f64,
    pub ascent: f64,
    pub descent: f64,
    pub cap_height: f64,
    pub stem_v: f64,
    pub flags: i32,
    /// /ToUnicode mappings: (code, Unicode string).
    pub to_unicode: Vec<(u8, String)>,
    /// Character codes actually painted with this font.
    pub used_chars: [u64; 4],
    /// CID font indicators and mappings
    pub is_cid: bool,
    pub is_native: bool,
    pub legacy_cids: Vec<(u8, u16, String)>,
    pub native_cids: Vec<(u16, u16, String)>,
    pub used_gids: std::collections::BTreeSet<u16>,
    pub to_unicode_2byte: Vec<(u16, String)>,
}

impl PdfDoc {
    pub fn new() -> Self {
        PdfDoc {
            compression_worker: None,
            page_compression: Vec::new(),
            pages: Vec::new(),
            objects: Vec::new(),
            form_fonts: Vec::new(),
            info: Vec::new(),
            catalog_extra: Vec::new(),
            names_extra: Vec::new(),
            pages_attr: Vec::new(),
            outlines: Vec::new(),
            open_action: None,
            fonts: Vec::new(),
            font_chars: std::collections::BTreeMap::new(),
            encrypt: None,
            pdfa: false,
            minor_version: None,
            native_bindings: std::collections::BTreeMap::new(),
            legacy_bindings: std::collections::BTreeMap::new(),
        }
    }

    pub fn push_page(&mut self, page: PdfPage) {
        if self.pages.len() == 1 && !crate::debug_flag("TEX_PDF_SERIAL") {
            self.compression_worker = crate::pdfcompress::Worker::new();
        }
        self.page_compression.resize_with(self.pages.len(), || None);
        self.page_compression.push(
            self.compression_worker
                .as_ref()
                .and_then(|worker| worker.submit(&page.content)),
        );
        self.pages.push(page);
    }

    pub(crate) fn compressed_page(&self, index: usize) -> Option<&[u8]> {
        self.page_compression
            .get(index)?
            .as_ref()?
            .get(&self.pages.get(index)?.content)
    }

    #[inline]
    pub fn record_font_char(&mut self, font: usize, character: u8) {
        let words = self.font_chars.entry(font).or_insert([0; 4]);
        words[character as usize / 64] |= 1_u64 << (character as usize % 64);
    }

    pub fn get_or_alloc_native_code(
        &mut self,
        font_id: usize,
        glyph_id: u16,
        text: &str,
    ) -> (usize, u16) {
        let bindings = self.native_bindings.entry(font_id).or_default();
        for (index, binding) in bindings.iter().enumerate() {
            if let Some(indices) = binding.code_map.get(&glyph_id) {
                for &entry in indices {
                    if binding.entries[entry].2 == text {
                        return (index, binding.entries[entry].0);
                    }
                }
            }
        }
        if bindings
            .last()
            .is_none_or(|binding| binding.next_code > u16::MAX as u32)
        {
            bindings.push(NativeBindingInfo {
                code_map: std::collections::HashMap::new(),
                entries: Vec::new(),
                next_code: 1,
            });
        }
        let binding_index = bindings.len() - 1;
        let binding = &mut bindings[binding_index];
        let code = binding.next_code as u16;
        binding.next_code += 1;
        binding
            .code_map
            .entry(glyph_id)
            .or_default()
            .push(binding.entries.len());
        binding.entries.push((code, glyph_id, text.to_owned()));
        (binding_index, code)
    }
    pub fn get_or_alloc_legacy_code(
        &mut self,
        font_id: usize,
        base_char: u8,
        text: &str,
    ) -> (usize, u8) {
        let bindings = self.legacy_bindings.entry(font_id).or_default();
        for (index, binding) in bindings.iter().enumerate() {
            if let Some(indices) = binding.code_map.get(&base_char) {
                for &entry in indices {
                    if binding.entries[entry].2 == text {
                        return (index, binding.entries[entry].0);
                    }
                }
            }
        }
        if bindings
            .last()
            .is_none_or(|binding| binding.next_code > u8::MAX as u16)
        {
            bindings.push(LegacyBindingInfo {
                code_map: std::collections::HashMap::new(),
                entries: Vec::new(),
                next_code: 1,
            });
        }
        let binding_index = bindings.len() - 1;
        let binding = &mut bindings[binding_index];
        let code = binding.next_code as u8;
        binding.next_code += 1;
        binding
            .code_map
            .entry(base_char)
            .or_default()
            .push(binding.entries.len());
        binding.entries.push((code, base_char, text.to_owned()));
        (binding_index, code)
    }
}

impl Default for PdfDoc {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------- primitive handling

/// Sentinel \pdfdest coordinate: "use the current position" (pdfTeX uses
/// the value -32768 for this).
pub const PDF_POS_CURRENT: i32 = -32768;

/// Result of scanning an optional \pdfdest positional parameter.
enum DestParam {
    Number,
    Null,
    End,
}
/// Destination type keywords, case-insensitive per pdfTeX's scan_keyword.
fn dest_kind(kw: &[u8]) -> Option<u8> {
    Some(match kw {
        b"xyz" => 0,
        b"fit" => 1,
        b"fith" => 2,
        b"fitv" => 3,
        b"fitb" => 4,
        b"fitbh" => 5,
        b"fitbv" => 6,
        b"fitr" => 7,
        _ => return None,
    })
}

/// Number of optional positional parameters a dest type takes
/// (xyz: left top zoom; fith/fitbh: top; fitv/fitbv: left; fitr: 4).
fn dest_param_count(kind: u8) -> usize {
    match kind {
        0 => 3,
        2 | 5 | 3 | 6 => 1,
        7 => 4,
        _ => 0,
    }
}

/// Extract the /URI (...) action target from a raw \pdfstartlink user{...}
/// spec; returns None when the spec carries no URI action.
pub fn extract_uri(spec: &str) -> Option<String> {
    let b = spec.as_bytes();
    let mut i = 0;
    while i + 4 <= b.len() {
        if &b[i..i + 4] == b"/URI" && b.get(i + 4).is_none_or(|c| !c.is_ascii_alphabetic()) {
            let mut j = i + 4;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'(' {
                j += 1;
                let mut out = Vec::new();
                let mut depth = 1;
                while j < b.len() {
                    match b[j] {
                        b'\\' if j + 1 < b.len() => {
                            out.push(b[j + 1]);
                            j += 2;
                        }
                        b'(' => {
                            depth += 1;
                            out.push(b'(');
                            j += 1;
                        }
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                return String::from_utf8_lossy(&out).into_owned().into();
                            }
                            out.push(b')');
                            j += 1;
                        }
                        c => {
                            out.push(c);
                            j += 1;
                        }
                    }
                }
            }
        }
        i += 1;
    }
    None
}

impl Engine {
    /// Peek at the run of letter tokens at the input (pdfTeX-style
    /// keyword: plain letters scanned case-insensitively, e.g. the
    /// `attr`/`goto`/`name`/`XYZ` of hyperref's calls). The input is
    /// always restored.
    fn peek_letters(&mut self) -> String {
        self.skip_spaces_relax();
        let mut letters: Vec<u8> = Vec::new();
        let mut toks: Vec<Token> = Vec::new();
        loop {
            let t = self.get_token();
            if t.is_char() && t.cc() == 11 {
                letters.push(t.chr() as u8);
                toks.push(t);
            } else {
                toks.push(t);
                break;
            }
        }
        for t in toks.iter().rev() {
            self.push_token(t.clone());
        }
        String::from_utf8_lossy(&letters).to_ascii_lowercase()
    }

    /// Consume the keyword `name` (a run of letters, case-insensitive,
    /// spaces skipped). On mismatch the input is fully restored.
    fn take_keyword(&mut self, name: &[u8]) -> bool {
        self.skip_spaces_relax();
        let mut letters: Vec<u8> = Vec::new();
        let mut toks: Vec<Token> = Vec::new();
        loop {
            let t = self.get_token();
            if t.is_char() && t.cc() == 11 {
                letters.push(t.chr() as u8);
                toks.push(t);
            } else {
                toks.push(t);
                break;
            }
        }
        if letters.to_ascii_lowercase().as_slice() == name {
            // keyword consumed: only the terminator token goes back
            if let Some(t) = toks.last() {
                self.push_token(t.clone());
            }
            true
        } else {
            for t in toks.iter().rev() {
                self.push_token(t.clone());
            }
            false
        }
    }

    /// Next optional \pdfdest parameter: a number, the keyword `null`
    /// (keep the current position), or end of the parameter list.
    fn next_dest_param(&mut self) -> DestParam {
        self.skip_spaces_relax();
        let t = self.get_token();
        if t.is_char() && matches!(t.chr() as u8, b'0'..=b'9' | b'+' | b'-') {
            self.push_token(t);
            return DestParam::Number;
        }
        if t.is_char() && t.cc() == 11 {
            let mut letters = vec![t.chr() as u8];
            let mut toks = vec![t];
            loop {
                let tt = self.get_token();
                if tt.is_char() && tt.cc() == 11 {
                    letters.push(tt.chr() as u8);
                    toks.push(tt);
                } else {
                    toks.push(tt);
                    break;
                }
            }
            if letters.to_ascii_lowercase().as_slice() == b"null" {
                if let Some(tt) = toks.last() {
                    self.push_token(tt.clone());
                }
                return DestParam::Null;
            }
            for tt in toks.iter().rev() {
                self.push_token(tt.clone());
            }
            return DestParam::End;
        }
        self.push_token(t);
        DestParam::End
    }

    /// \pdfstartlink [attr{..}|width..|height..|depth..] goto name{..}|user{..}
    /// hyperref forms: `attr{..} goto name{..}\relax`, `attr{..} user{..}\relax`,
    /// `user{..}\relax`.
    pub fn do_pdfstartlink(&mut self) {
        let mut attr = String::new();
        let mut uri: Option<String> = None;
        let mut dest: Option<String> = None;
        loop {
            match self.peek_letters().as_str() {
                "width" | "height" | "depth" => {
                    let kw = self.peek_letters();
                    self.take_keyword(kw.as_bytes());
                    self.scan_optional_equals();
                    let _ = self.scan_dimen(false, false);
                }
                "attr" => {
                    self.take_keyword(b"attr");
                    self.scan_optional_equals();
                    attr = self.scan_pdf_string();
                }
                "user" | "file" => {
                    let kw = self.peek_letters();
                    self.take_keyword(kw.as_bytes());
                    self.scan_optional_equals();
                    let spec = self.scan_pdf_string();
                    match extract_uri(&spec) {
                        Some(u) => uri = Some(u),
                        None => attr.push_str(&spec),
                    }
                    break;
                }
                "goto" => {
                    self.take_keyword(b"goto");
                    match self.peek_letters().as_str() {
                        "name" | "fld" => {
                            let sub = self.peek_letters();
                            self.take_keyword(sub.as_bytes());
                            dest = Some(self.scan_pdf_string());
                        }
                        "num" => {
                            self.take_keyword(b"num");
                            let _ = self.scan_int();
                        }
                        "page" => {
                            self.take_keyword(b"page");
                            let _ = self.scan_int();
                        }
                        _ => {}
                    }
                    break;
                }
                "thread" => {
                    self.take_keyword(b"thread");
                    break;
                }
                _ => break,
            }
        }
        self.append_whatsit(Node::Whatsit(WhatIt::PdfStartLink {
            attr,
            uri,
            name: dest,
        }));
    }

    /// \pdfdest name{<name>} <type> [<params>] — <type> is one of
    /// xyz fit fith fitv fitb fitbh fitbv fitr (case-insensitive), each
    /// optionally followed by integers (sp; `null` keeps the anchor) and
    /// ended by `\relax` or the next non-parameter token.
    pub fn do_pdfdest(&mut self) {
        match self.peek_letters().as_str() {
            "name" | "fld" => {
                let kw = self.peek_letters();
                self.take_keyword(kw.as_bytes());
                let name = self.scan_pdf_string();
                let type_kw = self.peek_letters();
                let kind = dest_kind(type_kw.as_bytes()).unwrap_or(0);
                if !type_kw.is_empty() {
                    self.take_keyword(type_kw.as_bytes());
                }
                let mut vals = [PDF_POS_CURRENT; 4];
                for slot in vals.iter_mut().take(dest_param_count(kind)) {
                    match self.next_dest_param() {
                        DestParam::Number => *slot = self.scan_int(),
                        DestParam::Null => {}
                        DestParam::End => break,
                    }
                }
                let node = Node::Whatsit(WhatIt::PdfDest {
                    name,
                    kind,
                    params: vals,
                });
                match self.mode {
                    crate::engine::Mode::Vertical | crate::engine::Mode::InternalVertical => {
                        self.vlist_append(node)
                    }
                    _ => self.cur_list.push(node),
                }
            }
            "num" => {
                self.take_keyword(b"num");
                let _ = self.scan_int();
            }
            _ => {}
        }
    }

    /// \pdfannot [width <d>|height <d>|depth <d>] {<dict body>}: a generic
    /// annotation over the given box at the current point.
    pub fn do_pdfannot(&mut self) {
        let mut wd = 0;
        let mut ht = 0;
        let mut dp = 0;
        loop {
            match self.peek_letters().as_str() {
                "width" => {
                    self.take_keyword(b"width");
                    self.scan_optional_equals();
                    wd = self.scan_dimen(false, false);
                }
                "height" => {
                    self.take_keyword(b"height");
                    self.scan_optional_equals();
                    ht = self.scan_dimen(false, false);
                }
                "depth" => {
                    self.take_keyword(b"depth");
                    self.scan_optional_equals();
                    dp = self.scan_dimen(false, false);
                }
                _ => break,
            }
        }
        let attr = self.scan_pdf_string();
        self.append_whatsit(Node::Whatsit(WhatIt::PdfAnnot { attr, wd, ht, dp }));
    }

    /// \pdfoutline [goto name{..}|user{..}] [count <n>] {<title>}
    pub fn do_pdfoutline(&mut self) {
        let mut dest = String::new();
        match self.peek_letters().as_str() {
            "goto" => {
                self.take_keyword(b"goto");
                match self.peek_letters().as_str() {
                    "name" | "fld" => {
                        let sub = self.peek_letters();
                        self.take_keyword(sub.as_bytes());
                        dest = self.scan_pdf_string();
                    }
                    "num" => {
                        self.take_keyword(b"num");
                        let _ = self.scan_int();
                    }
                    _ => {}
                }
            }
            "user" => {
                self.take_keyword(b"user");
                let _ = self.scan_pdf_string();
            }
            _ => {}
        }
        let mut count = 0i32;
        if self.peek_letters() == "count" {
            self.take_keyword(b"count");
            count = self.scan_int();
        }
        let title = self.scan_pdf_string();
        self.pdf_outlines.push((title, dest, count));
        // a trailing \pdfoutline after the last shipout must survive:
        // render_page only mirrors at page build time
        self.pdf_doc.outlines = self.pdf_outlines.clone();
    }

    /// \pdfcatalog {<dict body>} [use {<dict body>}]
    pub fn do_pdfcatalog(&mut self) {
        let body = self.scan_pdf_string();
        self.pdf_doc
            .catalog_extra
            .extend_from_slice(body.as_bytes());
        if self.peek_letters() == "use" {
            self.take_keyword(b"use");
            let extra = self.scan_pdf_string();
            self.pdf_doc
                .catalog_extra
                .extend_from_slice(extra.as_bytes());
        }
        // pdfTeX keyword form: `openaction goto page <n> {<view>}` —
        // hyperref's \PDF@SetupDoc emits it right after the dict body.
        // Unparsed, it leaked into the typeset stream.
        if self.peek_letters() == "openaction" {
            self.take_keyword(b"openaction");
            if self.peek_letters() == "goto" {
                self.take_keyword(b"goto");
                if self.peek_letters() == "page" {
                    self.take_keyword(b"page");
                    let page = self.scan_int();
                    let view = self.scan_pdf_string();
                    self.pdf_doc.open_action = Some((page, view));
                }
            }
        }
    }

    /// \pdfnames {<dict entries>}: appended to the catalog /Names dict.
    pub fn do_pdfnames(&mut self) {
        let body = self.scan_pdf_string();
        self.pdf_doc.names_extra.extend_from_slice(body.as_bytes());
    }

    /// \pdfpageattr {<dict body>}: replaces the per-page attribute body.
    pub fn do_pdfpageattr(&mut self) {
        self.scan_optional_equals();
        let toks = self.scan_token_list();
        self.pdf_page_attr = self.write_tokens_to_string(&toks);
        self.pdf_page_attr_toks = toks;
    }

    /// \pdfpagesattr {<dict body>}: replaces the page-tree attribute body.
    pub fn do_pdfpagesattr(&mut self) {
        self.scan_optional_equals();
        let toks = self.scan_token_list();
        self.pdf_pages_attr = self.write_tokens_to_string(&toks);
        self.pdf_pages_attr_toks = toks;
        self.pdf_doc.pages_attr = self.pdf_pages_attr.clone().into_bytes();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dest_kinds_map_case_insensitively() {
        // case-folding happens in the scanner (peek_letters lowercases)
        assert_eq!(dest_kind(b"xyz"), Some(0));
        assert_eq!(dest_kind(b"XYZ"), None);
        assert_eq!(dest_kind(b"fit"), Some(1));
        assert_eq!(dest_kind(b"fitbh"), Some(5));
        assert_eq!(dest_kind(b"fitbv"), Some(6));
        assert_eq!(dest_kind(b"fitr"), Some(7));
        assert_eq!(dest_kind(b"nonsense"), None);
        assert_eq!(dest_param_count(0), 3);
        assert_eq!(dest_param_count(5), 1);
        assert_eq!(dest_param_count(7), 4);
        assert_eq!(dest_param_count(1), 0);
    }

    #[test]
    fn uri_extraction_from_user_spec() {
        assert_eq!(
            extract_uri("/Subtype/Link/A<</S/URI/URI(https://example.org)>>"),
            Some("https://example.org".to_string())
        );
        // parens and backslashes inside the string survive PDF escaping
        assert_eq!(
            extract_uri("/A<</S/URI/URI(a\\(b\\)c)>>"),
            Some("a(b)c".to_string())
        );
        // no URI action: None, raw spec keeps flowing through as attr
        assert_eq!(extract_uri("/Subtype/Link/A<</S/Named/N/NextPage>>"), None);
    }

    #[test]
    fn dest_doc_model_defaults() {
        let doc = PdfDoc::new();
        assert!(doc.names_extra.is_empty());
        assert!(doc.pages_attr.is_empty());
        let page = PdfPage::new(612, 792);
        assert!(page.dests.is_empty());
        assert!(page.attr_extra.is_empty());
        let d = Dest {
            name: "a".into(),
            x: 1.0,
            y: 2.0,
            kind: 0,
            zoom: None,
        };
        assert_eq!((d.name.as_str(), d.kind), ("a", 0));
    }
}
