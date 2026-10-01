use tex_core::engine::Engine;
use tex_core::prim::DimParam;

#[test]
fn test_load_default_fmt_blob() {
    let fmt_bytes = include_bytes!("../../tex-cli/assets/default.fmt.zst");
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

#[test]
fn test_preamble_format_fast_boot() {
    let mut eng1 = Engine::new(false);
    let fmt_bytes = include_bytes!("../../tex-cli/assets/default.fmt.zst");
    tex_core::format::load_format_bytes_into(fmt_bytes, &mut eng1).unwrap();
    eng1.input.push_file(
        "p.tex".to_string(),
        br"\documentclass{article}\usepackage{amsmath}".to_vec(),
    );
    eng1.run();
    assert_eq!(tex_core::format::check_dumpable(&eng1), Ok(()));

    let tmp = std::env::temp_dir().join(format!("preamble_fast_boot_{}.fmt", std::process::id()));
    let fmt_path = tmp.as_path();
    tex_core::format::save_format(&eng1, fmt_path).expect("save preamble");

    let mut eng2 = Engine::new(false);
    tex_core::format::load_format_into(fmt_path, &mut eng2).expect("load preamble");
    // a real job always has a name; `\jobname.aux` must not be the dotfile `.aux`
    eng2.job_name = "b".to_string();
    eng2.out_dir = std::env::temp_dir().to_string_lossy().into_owned();
    eng2.input.push_file(
        "b.tex".to_string(),
        br"\begin{document} Math: $\begin{pmatrix} a & b \\ c & d \end{pmatrix}$ \end{document}"
            .to_vec(),
    );
    eng2.run();
    let _ = std::fs::remove_file(fmt_path);
    assert_eq!(eng2.error_count, 0, "{}", eng2.term);
    assert_eq!(eng2.pdf_doc.pages.len(), 1);
}

/// Primitives added for pdfTeX/e-TeX parity work through the shipped format:
/// `px` follows `\pdfpxdimen` (initially 1bp), `\eTeXrevision` expands, and
/// the fake count-register stand-ins for unimplemented pdfTeX parameters
/// stay undefined so packages take their "primitive absent" branch rather
/// than a defined-but-inert one. The dimensions and revision match TeX Live
/// 2026 `pdflatex` (where `\pdfdecimaldigits` is a real primitive).
#[test]
fn shipped_format_has_pdftex_px_unit_and_etex_revision() {
    let mut eng = Engine::new(false);
    let fmt_bytes = include_bytes!("../../tex-cli/assets/default.fmt.zst");
    tex_core::format::load_format_bytes_into(fmt_bytes, &mut eng).unwrap();
    tex_core::driver::finalize_format_load(&mut eng);
    eng.job_name = "px".to_string();
    eng.out_dir = std::env::temp_dir().to_string_lossy().into_owned();
    eng.input.push_file(
        "px.tex".to_string(),
        br"\documentclass{article}\begin{document}
\dimen0=2px \pdfpxdimen=2pt \dimen2=3px
\typeout{[\the\dimen0|\the\dimen2|\eTeXrevision|\ifdefined\pdfdecimaldigits D\else U\fi]}
\end{document}"
            .to_vec(),
    );
    eng.run();
    assert!(eng.term.contains("[2.0075pt|6.0pt|.6|U]"), "{}", eng.term);
}
