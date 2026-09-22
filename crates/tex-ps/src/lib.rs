//! tex-ps: Pure-Rust PostScript and EPS interpreter and PDF renderer.

pub mod interp;
pub mod lexer;
pub mod pdf_writer;
pub mod types;

pub use interp::{PsError, PsInterpreter};
pub use lexer::{extract_bounding_box, extract_ps_payload};
pub use pdf_writer::{generate_form_xobject, generate_pdf};
pub use types::{EpsBoundingBox, GraphicsState, Matrix, PathOp, PsColor, PsValue};

/// Output of an EPS to PDF conversion.
#[derive(Clone, Debug)]
pub struct EpsPdfOutput {
    pub pdf_bytes: Vec<u8>,
    pub content_stream: Vec<u8>,
    pub bbox: EpsBoundingBox,
}

/// Converts raw EPS bytes into a valid PDF document and content stream.
pub fn eps_to_pdf(eps_bytes: &[u8]) -> Result<EpsPdfOutput, PsError> {
    let payload = extract_ps_payload(eps_bytes);
    let bbox = extract_bounding_box(eps_bytes).unwrap_or(EpsBoundingBox {
        llx: 0.0,
        lly: 0.0,
        urx: 100.0,
        ury: 100.0,
    });

    let mut interp = PsInterpreter::new(bbox);
    interp.execute(payload)?;

    let content_stream = interp.pdf_stream;
    let pdf_bytes = generate_pdf(&bbox, &content_stream);

    Ok(EpsPdfOutput {
        pdf_bytes,
        content_stream,
        bbox,
    })
}
