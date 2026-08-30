//! PDF document model: pages, annotations, destinations, embedded fonts.
//! Serialization lives in `pdffile`; page rendering in `pdfrender`.

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
}

/// A compiled PDF page awaiting serialization.
pub struct PdfPage {
    pub content: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub annots: Vec<Annot>,
    /// (doc font index, resource number) — resolved by `embed_used_fonts`
    pub fonts: Vec<(usize, u16)>,
    /// named destinations anchored on this page: (name, x_bp, y_bp, kind)
    /// kind: 0 = /XYZ, 1 = /FitBH
    pub dests: Vec<(String, f64, f64, u8)>,
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
        }
    }
}

pub struct PdfDoc {
    pub pages: Vec<PdfPage>,
    /// raw /Info dict body contributed by \pdfinfo
    pub info: Vec<u8>,
    /// raw dict body contributed by \pdfcatalog
    pub catalog_extra: Vec<u8>,
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
}

impl PdfDoc {
    pub fn new() -> Self {
        PdfDoc {
            pages: Vec::new(),
            info: Vec::new(),
            catalog_extra: Vec::new(),
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
