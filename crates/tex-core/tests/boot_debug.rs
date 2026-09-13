use tex_core::engine::Engine;
use tex_core::prim::DimParam;

#[test]
fn test_load_default_fmt_blob() {
    let fmt_bytes = include_bytes!("../../tex-cli/assets/default.fmt");
    let mut eng = Engine::new(false);
    tex_core::format::load_format_bytes_into(fmt_bytes, &mut eng)
        .expect("load_format_bytes_into default.fmt");
    assert_eq!(eng.eqtb.cat[b'd' as usize], 11);
    assert_ne!(eng.eqtb.cat[b'@' as usize], 0);
    let doc_id = eng.cs.lookup(b"document").expect("\\document defined");
    assert!(matches!(
        eng.eqtb.resolve(doc_id),
        Some(tex_core::eqtb::Equiv::Macro(_))
    ));
    assert_eq!(
        eng.eqtb.dim_params[DimParam::PdfPageWidth.idx() as usize],
        39_158_276
    );
    assert_eq!(
        eng.eqtb.dim_params[DimParam::PdfPageHeight.idx() as usize],
        55_380_990
    );
}

#[test]
fn initex_pdf_page_dimensions_start_unset() {
    let mut eng = Engine::new(true);
    eng.init_primitives();
    assert_eq!(
        eng.eqtb.dim_params[DimParam::PdfPageWidth.idx() as usize],
        0
    );
    assert_eq!(
        eng.eqtb.dim_params[DimParam::PdfPageHeight.idx() as usize],
        0
    );
}
