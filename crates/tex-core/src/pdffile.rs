//! Serialize a PdfDoc into PDF bytes: xref, catalog, page tree, content
//! streams (flate), Type 1 font embedding with encodings and widths, link
//! annotations, named destinations, and outlines.

use crate::pdf_fonts::{parse_metrics, parse_type1};
use crate::pdfout::{Annot, EmbedFont, PdfDoc};
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::Write;

fn flate(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::fast());
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// Build an EmbedFont from a font's PFB bytes / encoding vector / TFM widths.
/// A missing PFB yields a font-dict-only entry (viewer substitutes by
/// BaseFont name); a missing encoding vector uses the font's built-in one.
pub fn make_embed_font(
    base_font: String,
    pfb: Option<&[u8]>,
    encoding: Option<&[String]>,
    first_char: u8,
    last_char: u8,
    widths_1000: Vec<i32>,
) -> EmbedFont {
    let (font_file, length1, length2, length3, metrics) = match pfb {
        Some(bytes) => {
            let p = parse_type1(bytes);
            let m = parse_metrics(&p.data[..p.length1.min(p.data.len())]);
            (p.data, p.length1, p.length2, p.length3, m)
        }
        None => (Vec::new(), 0, 0, 0, parse_metrics(b"")),
    };
    // no external encoding: adopt the font's own /Encoding array (if
    // the cleartext declares one) so extractors see accurate glyph names
    let encoding_diff = match encoding {
        Some(e) => Some(e.to_vec()),
        None => pfb.and_then(|bytes| {
            let prog = parse_type1(bytes);
            let end = prog.length1.min(prog.data.len());
            crate::pdf_fonts::builtin_encoding(&prog.data[..end])
        }),
    };
    // /ToUnicode: resolve every encoded slot through the glyph list,
    // keeping only non-identity mappings (ASCII slots extract natively)
    let to_unicode = encoding_diff
        .iter()
        .flat_map(|d| d.iter().enumerate())
        .filter_map(|(slot, g)| {
            if g.is_empty() || slot > 255 {
                return None;
            }
            let uni = crate::pdf_fonts::glyph_to_unicode(g)?;
            if slot < 0x80 && uni.len() == 1 && uni.as_bytes()[0] == slot as u8 {
                return None;
            }
            Some((slot as u8, uni))
        })
        .collect();
    EmbedFont {
        obj_font: 0,
        base_font,
        font_file,
        length1,
        length2,
        length3,
        encoding_diff,
        first_char,
        last_char,
        widths: widths_1000,
        font_matrix_scale: 1.0,
        font_bbox: metrics.font_bbox,
        italic_angle: metrics.italic_angle,
        ascent: metrics.ascent,
        descent: metrics.descent,
        cap_height: metrics.cap_height,
        stem_v: metrics.stem_v,
        flags: 4,
        to_unicode,
    }
}

// ---------------------------------------------------------------- serializer

struct PdfBuilder {
    objs: Vec<Option<Vec<u8>>>, // index n-1 holds object n
}

impl PdfBuilder {
    fn new() -> Self {
        PdfBuilder { objs: Vec::new() }
    }

    fn alloc(&mut self) -> usize {
        self.objs.push(None);
        self.objs.len()
    }

    fn set(&mut self, num: usize, body: String) {
        self.objs[num - 1] = Some(body.into_bytes());
    }

    fn set_stream(&mut self, num: usize, dict_extra: &str, data: &[u8], compress: bool) {
        let data = if compress { flate(data) } else { data.to_vec() };
        let filter = if compress { " /Filter /FlateDecode" } else { "" };
        let mut body = format!(
            "<< /Length {}{} {}>>\nstream\n",
            data.len(),
            filter,
            dict_extra
        )
        .into_bytes();
        body.extend_from_slice(&data);
        body.extend_from_slice(b"\nendstream\nendobj\n");
        self.objs[num - 1] = Some(body);
    }
}

/// Compact f64 formatting for PDF numbers: no trailing zero runs.
fn num(v: f64) -> String {
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// Escape a PDF literal string body.
fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a PDF name (identifier).
fn escape_pdf_name(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '#' | '(' | ')' | '<' | '>' | '[' | ']' | '/' | '%' => format!("#{:02x}", c as u8),
            _ => c.to_string(),
        })
        .collect()
}

/// PDF text string: ASCII as literal, otherwise UTF-16BE hex.
fn pdf_text_string(s: &str) -> String {
    if s.is_ascii() {
        return format!("({})", escape_string(s));
    }
    let mut hex = String::from("FEFF");
    for u in s.encode_utf16() {
        hex += &format!("{:04X}", u);
    }
    format!("<{}>", hex)
}

pub fn write_pdf(doc: &PdfDoc) -> Vec<u8> {
    let mut b = PdfBuilder::new();
    let catalog_obj = b.alloc(); // 1
    let pages_obj = b.alloc(); // 2

    // Collect named destinations first (first definition wins, sorted),
    // so the names object is only allocated when needed.
    let mut named: Vec<(String, usize, f64, f64, u8, Option<f64>)> = Vec::new();
    for (pi, page) in doc.pages.iter().enumerate() {
        for dest in &page.dests {
            if !named.iter().any(|(n, ..)| n == &dest.name) {
                named.push((dest.name.clone(), pi, dest.x, dest.y, dest.kind, dest.zoom));
            }
        }
    }
    named.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let names_obj = if named.is_empty() && doc.names_extra.is_empty() {
        0
    } else {
        b.alloc()
    };

    // Font objects: font dict, descriptor, optional font file and
    // optional /ToUnicode CMap.
    struct FontObjs {
        font: usize,
        desc: usize,
        file: Option<usize>,
        tounicode: Option<usize>,
    }
    let font_objs: Vec<FontObjs> = doc
        .fonts
        .iter()
        .map(|f| {
            let font = b.alloc();
            let desc = b.alloc();
            let file = if f.font_file.is_empty() { None } else { Some(b.alloc()) };
            let tounicode = if f.to_unicode.is_empty() { None } else { Some(b.alloc()) };
            FontObjs { font, desc, file, tounicode }
        })
        .collect();

    // Page objects: content stream, page dict, one object per annotation.
    let page_objs: Vec<(usize, usize, Vec<usize>)> = doc
        .pages
        .iter()
        .map(|p| {
            let content = b.alloc();
            let page = b.alloc();
            let annots = (0..p.annots.len()).map(|_| b.alloc()).collect();
            (content, page, annots)
        })
        .collect();

    // Outlines: root + one entry per \pdfoutline (flat, linked as siblings).
    let outlines: Option<(usize, Vec<usize>)> = if doc.outlines.is_empty() {
        None
    } else {
        let root = b.alloc();
        let entries = (0..doc.outlines.len()).map(|_| b.alloc()).collect();
        Some((root, entries))
    };

    let info_obj = b.alloc();

    // ---- emit fonts
    for (f, fo) in doc.fonts.iter().zip(&font_objs) {
        let first = f.first_char as i32;
        let last = f.last_char as i32;
        let n = (last - first + 1).max(1) as usize;
        let mut widths = String::from("[ ");
        for k in 0..n {
            widths.push_str(&num(f.widths.get(k).copied().unwrap_or(0) as f64));
            widths.push(' ');
        }
        widths.push(']');
        let mut enc = String::new();
        if let Some(diffs) = &f.encoding_diff {
            enc.push_str(" /Encoding << /Type /Encoding /Differences [ ");
            for (slot, g) in diffs.iter().enumerate() {
                if !g.is_empty() {
                    enc.push_str(&format!("{} /{} ", slot, escape_pdf_name(g)));
                }
            }
            enc.push_str(" ] >>");
        }
        let tounicode_ref = match fo.tounicode {
            Some(o) => format!(" /ToUnicode {} 0 R", o),
            None => String::new(),
        };
        b.set(
            fo.font,
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /{} /FirstChar {} /LastChar {} /Widths {} /FontDescriptor {} 0 R{}{} >>",
                escape_pdf_name(&f.base_font), first, last, widths, fo.desc, enc, tounicode_ref
            ),
        );
        let mut desc = format!(
            "<< /Type /FontDescriptor /FontName /{} /Flags {} /FontBBox [{} {} {} {}] /ItalicAngle {} /Ascent {} /Descent {} /CapHeight {} /StemV {}",
            escape_pdf_name(&f.base_font),
            f.flags,
            num(f.font_bbox[0]), num(f.font_bbox[1]), num(f.font_bbox[2]), num(f.font_bbox[3]),
            num(f.italic_angle), num(f.ascent), num(f.descent), num(f.cap_height), num(f.stem_v),
        );
        if let Some(file) = fo.file {
            desc.push_str(&format!(" /FontFile {} 0 R", file));
        }
        desc.push_str(" >>");
        b.set(fo.desc, desc);
        if let Some(file) = fo.file {
            b.set_stream(
                file,
                &format!(
                    "/Length1 {} /Length2 {} /Length3 {}",
                    f.length1, f.length2, f.length3
                ),
                &f.font_file,
                false,
            );
        }
        if let Some(to) = fo.tounicode {
            b.set_stream(to, "", to_unicode_cmap(&f.to_unicode).as_bytes(), true);
        }
    }

    // ---- emit pages
    for (i, page) in doc.pages.iter().enumerate() {
        let (content_obj, page_obj, annot_objs) = &page_objs[i];
        b.set_stream(*content_obj, "", &page.content, true);
        let mut fonts_res = String::new();
        for (fidx, fnum) in &page.fonts {
            if let Some(fo) = font_objs.get(*fidx) {
                fonts_res.push_str(&format!("/F{} {} 0 R ", fnum, fo.font));
            }
        }
        let mut annots_res = String::new();
        for (a, aobj) in page.annots.iter().zip(annot_objs) {
            emit_annot(&mut b, *aobj, a);
            annots_res.push_str(&format!("{} 0 R ", aobj));
        }
        let annots = if annots_res.is_empty() {
            String::new()
        } else {
            format!(" /Annots [ {}]", annots_res)
        };
        let page_attr = String::from_utf8_lossy(&page.attr_extra);
        b.set(
            *page_obj,
            format!(
                "<< /Type /Page /Parent {} 0 R /MediaBox [0 0 {} {}] /Contents {} 0 R /Resources << /Font << {} >> /ProcSet [/PDF /Text] >>{}{} >>",
                pages_obj,
                page.width,
                page.height,
                content_obj,
                fonts_res,
                annots,
                if page_attr.trim().is_empty() {
                    String::new()
                } else {
                    format!(" {}", page_attr)
                }
            ),
        );
    }

    // ---- emit the page tree node
    let kids: Vec<String> = page_objs
        .iter()
        .map(|(_, p, _)| format!("{} 0 R", p))
        .collect();
    let pages_attr = String::from_utf8_lossy(&doc.pages_attr);
    b.set(
        pages_obj,
        format!(
            "<< /Type /Pages /Count {} /Kids [ {} ]{} >>",
            page_objs.len(),
            kids.join(" "),
            if pages_attr.trim().is_empty() {
                String::new()
            } else {
                format!(" {}", pages_attr)
            }
        ),
    );

    // ---- emit named destination tree
    if names_obj != 0 {
        let mut body = String::from("<< ");
        if !named.is_empty() {
            body.push_str("/Names [ ");
            for (name, pi, x, y, kind, zoom) in &named {
                let page_ref = page_objs[*pi].1;
                let term = match kind {
                    1 => "/Fit".to_string(),
                    2 => format!("/FitH {}", num(*y)),
                    3 => format!("/FitV {}", num(*x)),
                    4 => "/FitB".to_string(),
                    5 => format!("/FitBH {}", num(*y)),
                    6 => format!("/FitBV {}", num(*x)),
                    7 => format!("/FitR {} {} {} {}", num(*x), num(*y), num(*x), num(*y)),
                    _ => format!(
                        "/XYZ {} {} {}",
                        num(*x),
                        num(*y),
                        zoom.map(|z| num(z)).unwrap_or_else(|| "null".to_string())
                    ),
                };
                body.push_str(&format!("({}) [{} 0 R {}] ", escape_string(name), page_ref, term));
            }
            body.push_str(" ] ");
        }
        // \pdfnames entries merge into the same /Names dictionary
        body.push_str(&String::from_utf8_lossy(&doc.names_extra));
        body.push_str(" >>");
        b.set(names_obj, body);
    }

    // ---- emit outlines
    if let Some((root, entries)) = &outlines {
        let n = entries.len();
        for (k, ((title, dest, count), eobj)) in
            doc.outlines.iter().zip(entries).enumerate()
        {
            let mut body = format!(
                "<< /Title {} /Parent {} 0 R",
                pdf_text_string(title),
                root
            );
            if k > 0 {
                body.push_str(&format!(" /Prev {} 0 R", entries[k - 1]));
            }
            if k + 1 < n {
                body.push_str(&format!(" /Next {} 0 R", entries[k + 1]));
            }
            if !dest.is_empty() {
                body.push_str(&format!(" /Dest ({})", escape_string(dest)));
            }
            if *count != 0 {
                body.push_str(&format!(" /Count {}", count));
            }
            body.push_str(" >>");
            b.set(*eobj, body);
        }
        b.set(
            *root,
            format!(
                "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {} >>",
                entries[0], entries[n - 1], n
            ),
        );
    }

    // ---- emit info
    b.set(
        info_obj,
        format!(
            "<< /Producer (tex-rs) /Creator (tex-rs) {} >>",
            String::from_utf8_lossy(&doc.info)
        ),
    );

    // ---- emit catalog
    let mut cat = format!("<< /Type /Catalog /Pages {} 0 R", pages_obj);
    if names_obj != 0 {
        cat.push_str(&format!(" /Names << /D {} 0 R >>", names_obj));
    }
    if let Some((page, view)) = &doc.open_action {
        if let Some(&(content, page_obj, _)) = page_objs.get((*page).checked_sub(1).unwrap_or(0) as usize) {
            let _ = content;
            let view_s = view.trim();
            let view_pdf = if !view_s.is_empty()
                && view_s.chars().all(|c| c.is_ascii_alphanumeric())
            {
                format!("/{}", view_s)
            } else {
                view_s.to_string()
            };
            cat.push_str(&format!(" /OpenAction [{} 0 R {}]", page_obj, view_pdf));
        }
    }
    if let Some((root, _)) = &outlines {
        cat.push_str(&format!(" /Outlines {} 0 R", root));
    }
    let extra = String::from_utf8_lossy(&doc.catalog_extra);
    if !extra.trim().is_empty() {
        cat.push(' ');
        cat.push_str(&extra);
    }
    cat.push_str(" >>");
    b.set(catalog_obj, cat);

    // ---- assemble file
    let mut buf = b"%PDF-1.5\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = vec![0usize; b.objs.len() + 1];
    for (i, obj) in b.objs.iter().enumerate() {
        let Some(bytes) = obj else { continue };
        offsets[i + 1] = buf.len();
        buf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        buf.extend_from_slice(bytes);
        buf.extend_from_slice(b"\nendobj\n");
    }
    let xref_pos = buf.len();
    let n = b.objs.len();
    buf.extend_from_slice(format!("xref\n0 {}\n", n + 1).as_bytes());
    buf.extend_from_slice(b"0000000000 65535 f \n");
    for off in &offsets[1..] {
        buf.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
    }
    buf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root {} 0 R /Info {} 0 R >>\nstartxref\n{}\n%%EOF\n",
            n + 1,
            catalog_obj,
            info_obj,
            xref_pos
        )
        .as_bytes(),
    );
    buf
}

fn emit_annot(b: &mut PdfBuilder, obj: usize, a: &Annot) {
    let [x0, y0, x1, y1] = a.rect;
    // /Border comes from the annotation attributes when present (hyperref
    // passes pdfborder explicitly); duplicating the key makes qpdf flag
    // every link object
    let border = if a.attr.contains("/Border") { "" } else { " /Border [0 0 0]" };
    let mut body = format!(
        "<< /Type /Annot /Subtype {} /Rect [{} {} {} {}]{}",
        a.subtype.as_deref().unwrap_or("/Link"),
        num(x0),
        num(y0),
        num(x1),
        num(y1),
        border
    );
    if let Some(uri) = &a.uri {
        body.push_str(&format!(
            " /A << /S /URI /URI ({}) >>",
            escape_string(uri)
        ));
    }
    if let Some(dest) = &a.dest {
        body.push_str(&format!(" /Dest ({})", escape_string(dest)));
    }
    if !a.attr.is_empty() {
        body.push(' ');
        body.push_str(&a.attr);
    }
    body.push_str(" >>");
    b.set(obj, body);
}
/// Build a /ToUnicode CMap stream body from (code, Unicode string) pairs.
/// The stream is written flate-compressed; bfchar blocks are chunked at
/// 100 entries (PDF limit).
fn to_unicode_cmap(map: &[(u8, String)]) -> String {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n\
         /CMapType 2 def\n\
         1 begincodespacerange\n\
         <00> <FF>\n\
         endcodespacerange\n",
    );
    for chunk in map.chunks(100) {
        s.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (code, uni) in chunk {
            let mut hex = String::new();
            for u in uni.encode_utf16() {
                hex += &format!("{:04X}", u);
            }
            s.push_str(&format!("<{:02X}> <{}>\n", code, hex));
        }
        s.push_str("endbfchar\n");
    }
    s.push_str(
        "endcmap\n\
         CMapName currentdict /CMap defineresource pop\n\
         end\n\
         end",
    );
    s
}
