//! PDF document and Form XObject generation from PostScript content streams.

use std::fmt::Write;
use crate::types::EpsBoundingBox;

/// Generates a valid standalone PDF 1.4 document containing the rendered EPS figure.
pub fn generate_pdf(bbox: &EpsBoundingBox, content_stream: &[u8]) -> Vec<u8> {
    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");

    let mut offsets = Vec::new();

    // 1 0 obj: Catalog
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    // 2 0 obj: Pages
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");

    // 3 0 obj: Page
    offsets.push(pdf.len());
    let mut page_dict = String::new();
    let _ = write!(
        page_dict,
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [{:.4} {:.4} {:.4} {:.4}] /Contents 4 0 R /Resources << >> >>\nendobj\n",
        bbox.llx, bbox.lly, bbox.urx, bbox.ury
    );
    pdf.extend_from_slice(page_dict.as_bytes());

    // 4 0 obj: Contents Stream
    offsets.push(pdf.len());
    let mut stream_header = String::new();
    let _ = write!(
        stream_header,
        "4 0 obj\n<< /Length {} >>\nstream\n",
        content_stream.len()
    );
    pdf.extend_from_slice(stream_header.as_bytes());
    pdf.extend_from_slice(content_stream);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");

    // Cross-reference table
    let xref_offset = pdf.len();
    pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for off in offsets {
        let mut row = String::new();
        let _ = write!(row, "{:010} 00000 n \n", off);
        pdf.extend_from_slice(row.as_bytes());
    }

    // Trailer
    let mut trailer = String::new();
    let _ = write!(
        trailer,
        "trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
        xref_offset
    );
    pdf.extend_from_slice(trailer.as_bytes());

    pdf
}

/// Generates a PDF Form XObject dictionary and stream for in-engine embedding.
pub fn generate_form_xobject(bbox: &EpsBoundingBox, content_stream: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut header = String::new();
    let _ = write!(
        header,
        "<< /Type /XObject /Subtype /Form /BBox [{:.4} {:.4} {:.4} {:.4}] /Length {} /Resources << >> >>\nstream\n",
        bbox.llx, bbox.lly, bbox.urx, bbox.ury, content_stream.len()
    );
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(content_stream);
    out.extend_from_slice(b"\nendstream");
    out
}
