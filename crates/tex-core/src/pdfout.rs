use crate::engine::Engine;
use crate::boxes::{Node, WhatIt};
use crate::prim::Prim;
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
    pub annots: Vec<Annot>,
    /// (doc font index, resource number) — resolved by `embed_used_fonts`
    pub fonts: Vec<(usize, u16)>,
    /// named destinations anchored on this page
    pub dests: Vec<Dest>,
    /// raw dict body contributed by \pdfpageattr (copied at shipout)
    pub attr_extra: Vec<u8>,
}

impl PdfPage {
    pub fn new(width: i32, height: i32) -> Self {
        PdfPage {
            content: Vec::new(),
            width,
            height,
            annots: Vec::new(),
            fonts: Vec::new(),
            dests: Vec::new(),
            attr_extra: Vec::new(),
        }
    }
}

pub struct PdfDoc {
    pub pages: Vec<PdfPage>,
    /// raw /Info dict body contributed by \pdfinfo
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
    pub fonts: Vec<EmbedFont>,
}

/// A Type 1 font prepared for embedding.
pub struct EmbedFont {
    pub obj_font: i32,
    pub base_font: String,
    /// full Type 1 program (cleartext + encrypted + trailer)
    pub font_file: Vec<u8>,
    pub length1: usize,
    pub length2: usize,
    pub length3: usize,
    /// glyph names by slot (None = the font's built-in encoding)
    pub encoding_diff: Option<Vec<String>>,
    pub first_char: u8,
    pub last_char: u8,
    /// widths in 1/1000 font units, for first_char..=last_char
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
    /// /ToUnicode mappings: (code, Unicode string). Non-identity mappings
    /// only; empty when no glyph names are known.
    pub to_unicode: Vec<(u8, String)>,
}

impl PdfDoc {
    pub fn new() -> Self {
        PdfDoc {
            pages: Vec::new(),
            info: Vec::new(),
            catalog_extra: Vec::new(),
            names_extra: Vec::new(),
            pages_attr: Vec::new(),
            outlines: Vec::new(),
            fonts: Vec::new(),
        }
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
            self.pushed.push(t.clone());
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
                self.pushed.push(t.clone());
            }
            true
        } else {
            for t in toks.iter().rev() {
                self.pushed.push(t.clone());
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
            self.pushed.push(t);
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
                    self.pushed.push(tt.clone());
                }
                return DestParam::Null;
            }
            for tt in toks.iter().rev() {
                self.pushed.push(tt.clone());
            }
            return DestParam::End;
        }
        self.pushed.push(t);
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
        self.append_whatsit(Node::Whatsit(WhatIt::PdfStartLink { attr, uri, name: dest }));
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
                let node = Node::Whatsit(WhatIt::PdfDest { name, kind, params: vals });
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
        self.pdf_doc.catalog_extra.extend_from_slice(body.as_bytes());
        if self.peek_letters() == "use" {
            self.take_keyword(b"use");
            let extra = self.scan_pdf_string();
            self.pdf_doc.catalog_extra.extend_from_slice(extra.as_bytes());
        }
    }

    /// \pdfnames {<dict entries>}: appended to the catalog /Names dict.
    pub fn do_pdfnames(&mut self) {
        let body = self.scan_pdf_string();
        self.pdf_doc.names_extra.extend_from_slice(body.as_bytes());
    }

    /// \pdfpageattr {<dict body>}: replaces the per-page attribute body.
    pub fn do_pdfpageattr(&mut self) {
        let body = self.scan_pdf_string();
        self.pdf_page_attr = body;
    }

    /// \pdfpagesattr {<dict body>}: replaces the page-tree attribute body.
    pub fn do_pdfpagesattr(&mut self) {
        let body = self.scan_pdf_string();
        self.pdf_pages_attr = body.clone();
        self.pdf_doc.pages_attr = body.into_bytes();
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
        let d = Dest { name: "a".into(), x: 1.0, y: 2.0, kind: 0, zoom: None };
        assert_eq!((d.name.as_str(), d.kind), ("a", 0));
    }
}
