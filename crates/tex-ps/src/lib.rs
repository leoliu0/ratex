//! tex-ps: pure-Rust PostScript interpreter that converts EPS figures to PDF.

mod base14;
mod files;
mod fonts;
mod graphics;
mod images;
mod interp;
mod lexer;
mod pdf_writer;
mod types;

pub use interp::PsError;
pub use lexer::{extract_bounding_box, extract_ps_payload};
pub use types::EpsBoundingBox;

/// Output of an EPS to PDF conversion.
#[derive(Clone, Debug)]
pub struct EpsPdfOutput {
    /// A standalone one-page PDF whose MediaBox is the EPS bounding box.
    pub pdf_bytes: Vec<u8>,
    /// The page's content stream (device space = EPS default user space).
    pub content_stream: Vec<u8>,
    pub bbox: EpsBoundingBox,
}

/// Converts raw EPS bytes (optionally with a DOS binary header) into PDF.
pub fn eps_to_pdf(eps_bytes: &[u8]) -> Result<EpsPdfOutput, PsError> {
    let payload = extract_ps_payload(eps_bytes);
    let bbox = extract_bounding_box(payload).unwrap_or(EpsBoundingBox {
        llx: 0.0,
        lly: 0.0,
        urx: 612.0,
        ury: 792.0,
    });

    let mut interp = interp::Interp::new(bbox);
    interp.run_program(payload)?;
    graphics::finish(&mut interp);

    let pdf_bytes = pdf_writer::generate_pdf(&bbox, &interp.out.content, &interp.out.fonts, &interp.out.images);
    Ok(EpsPdfOutput { pdf_bytes, content_stream: interp.out.content, bbox })
}
