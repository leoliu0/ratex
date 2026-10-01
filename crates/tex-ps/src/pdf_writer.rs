//! Assembles the single-page PDF for a converted EPS figure.

use std::fmt::Write;

use crate::fonts::{FontRes, BASE14_NAMES};
use crate::graphics::put_num;
use crate::images::ImageRes;
use crate::types::EpsBoundingBox;

fn put_name(out: &mut String, name: &[u8]) {
    out.push('/');
    for &b in name {
        if (0x21..=0x7E).contains(&b) && !b"()<>[]{}/%#".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "#{b:02X}");
        }
    }
}

fn font_object(res: &FontRes) -> String {
    let mut s = String::from("<< /Type /Font /Subtype /Type1 /BaseFont /");
    s.push_str(BASE14_NAMES[usize::from(res.base)]);
    s.push_str(" /Encoding << /Type /Encoding /Differences [");
    let mut next = None;
    for (code, glyph) in res.codes.iter().enumerate() {
        let Some(glyph) = glyph else { continue };
        if next != Some(code) {
            let _ = write!(s, " {code}");
        }
        s.push(' ');
        put_name(&mut s, glyph);
        next = Some(code + 1);
    }
    s.push_str(" ] >> >>");
    s
}

fn image_dict(img: &ImageRes) -> String {
    let mut s = format!(
        "<< /Type /XObject /Subtype /Image /Width {} /Height {} /BitsPerComponent {}",
        img.width, img.height, img.bpc
    );
    match &img.space {
        Some(space) => {
            s.push_str(" /ColorSpace ");
            s.push_str(space);
        }
        None => s.push_str(" /ImageMask true"),
    }
    if let Some(decode) = &img.decode {
        let mut nums = Vec::new();
        for (k, &d) in decode.iter().enumerate() {
            if k > 0 {
                nums.push(b' ');
            }
            put_num(&mut nums, d);
        }
        let _ = write!(s, " /Decode [{}]", String::from_utf8_lossy(&nums));
    }
    if img.interpolate {
        s.push_str(" /Interpolate true");
    }
    let _ = write!(s, " /Filter /{} /Length {} >>", img.filter, img.data.len());
    s
}

/// A standalone PDF 1.4 document holding one page with the figure.
pub(crate) fn generate_pdf(
    bbox: &EpsBoundingBox,
    content: &[u8],
    fonts: &[FontRes],
    images: &[ImageRes],
) -> Vec<u8> {
    let mut pdf = Vec::with_capacity(content.len() + images.iter().map(|i| i.data.len()).sum::<usize>() + 1024);
    pdf.extend_from_slice(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");
    let mut offsets = Vec::new();
    let mut object = |pdf: &mut Vec<u8>, body: &[u8], stream: Option<&[u8]>| {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        pdf.extend_from_slice(body);
        if let Some(data) = stream {
            pdf.extend_from_slice(b"\nstream\n");
            pdf.extend_from_slice(data);
            pdf.extend_from_slice(b"\nendstream");
        }
        pdf.extend_from_slice(b"\nendobj\n");
    };

    let first_font = 5;
    let first_image = first_font + fonts.len();
    let mut resources = String::from("<<");
    if !fonts.is_empty() {
        resources.push_str(" /Font <<");
        for k in 0..fonts.len() {
            let _ = write!(resources, " /F{} {} 0 R", k + 1, first_font + k);
        }
        resources.push_str(" >>");
    }
    if !images.is_empty() {
        resources.push_str(" /XObject <<");
        for k in 0..images.len() {
            let _ = write!(resources, " /Im{} {} 0 R", k + 1, first_image + k);
        }
        resources.push_str(" >>");
    }
    resources.push_str(" >>");
    let mut media = Vec::new();
    for (k, v) in [bbox.llx, bbox.lly, bbox.urx, bbox.ury].into_iter().enumerate() {
        if k > 0 {
            media.push(b' ');
        }
        put_num(&mut media, v);
    }

    object(&mut pdf, b"<< /Type /Catalog /Pages 2 0 R >>", None);
    object(&mut pdf, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>", None);
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [{}] /Contents 4 0 R /Resources {resources} >>",
        String::from_utf8_lossy(&media)
    );
    object(&mut pdf, page.as_bytes(), None);
    object(&mut pdf, format!("<< /Length {} >>", content.len()).as_bytes(), Some(content));
    for res in fonts {
        object(&mut pdf, font_object(res).as_bytes(), None);
    }
    for img in images {
        object(&mut pdf, image_dict(img).as_bytes(), Some(&img.data));
    }

    let xref = pdf.len();
    let count = offsets.len() + 1;
    pdf.extend_from_slice(format!("xref\n0 {count}\n0000000000 65535 f \n").as_bytes());
    for off in &offsets {
        pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    pdf
}
