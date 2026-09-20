use tex_runtime::{Session, Status};

fn session(source: &str) -> Session {
    let mut session = Session::new();
    session.set_epoch(Some(1_700_000_000)).unwrap();
    session.add_file("main.tex", source.as_bytes()).unwrap();
    session
}

fn native_text(pdf: &mut lopdf::Document) -> String {
    // lopdf 0.44 prioritizes /Encoding over /ToUnicode and does not handle
    // stream Encoding CMaps. For these extraction-only assertions, decode
    // the original source codes through their original /ToUnicode maps.
    // The unmodified PDFs' glyph addressing is checked by Poppler/pdf.js.
    for object in pdf.objects.values_mut() {
        if let Ok(font) = object.as_dict_mut() {
            if font.has_type(b"Font")
                && font
                    .get(b"Subtype")
                    .and_then(lopdf::Object::as_name)
                    .is_ok_and(|subtype| subtype == b"Type0")
            {
                assert!(font.has(b"ToUnicode"));
                font.remove(b"Encoding");
            }
        }
    }
    pdf.extract_text(&[1]).unwrap()
}

const HELLO: &str = r"\documentclass{article}\begin{document}Hello from libtex.\end{document}";

#[test]
fn bundled_latex_and_nested_inputs_produce_a_real_pdf() {
    let mut s = session(
        r"\documentclass{article}\usepackage{amsmath}\begin{document}\input{parts/body}\end{document}",
    );
    s.add_file(
        "parts/body.tex",
        br"Hello $x^2$.\section{Section}\label{sec:a}Section~\ref{sec:a}.",
    )
    .unwrap();
    let result = s.compile("main.tex");
    assert_eq!(
        result.status,
        Status::Success,
        "{}\n{}",
        result.diagnostics,
        result.log
    );
    assert!((2..=5).contains(&result.passes));
    assert!(result.pdf.starts_with(b"%PDF-"));
    let pdf = lopdf::Document::load_mem(&result.pdf).unwrap();
    assert_eq!(pdf.get_pages().len(), 1);
    assert!(result.files.contains_key("main.aux"));
    assert!(result.files.contains_key("main.log"));
    assert_eq!(result.files["main.pdf"], result.pdf);
    let text = pdf.extract_text(&[1]).unwrap();
    assert!(text.contains("Hello"), "{text}");
}

#[test]
fn automatic_bibtex_uses_the_production_engine() {
    let mut s = session(
        r"\documentclass{article}\begin{document}Citation~\cite{paper}.\bibliographystyle{plain}\bibliography{refs}\end{document}",
    );
    s.add_file("refs.bib", br"@article{paper, author={Ada Lovelace}, title={Library Test}, journal={Testing}, year={2024}}").unwrap();
    let r = s.compile("main.tex");
    assert_eq!(r.status, Status::Success, "{}\n{}", r.diagnostics, r.log);
    assert!(r.bibtex_runs >= 1);
    assert!(String::from_utf8_lossy(&r.files["main.bbl"]).contains("Lovelace"));
    assert!(!String::from_utf8_lossy(&r.files["main.aux"]).contains("undefined"));
}

#[test]
fn errors_and_repeated_sessions_do_not_reuse_stale_outputs() {
    let mut s = session(HELLO);
    let first = s.compile("main.tex");
    assert_eq!(first.status, Status::Success, "{}", first.diagnostics);
    let original = first.pdf.clone();
    s.add_file(
        "main.tex",
        br"\documentclass{article}\begin{document}\input{missing-file}\end{document}",
    )
    .unwrap();
    let failed = s.compile("main.tex");
    assert_eq!(failed.status, Status::CompilationError);
    assert!(failed.pdf.is_empty());
    assert!(!failed.files.contains_key("main.pdf"));
    assert!(failed.diagnostics.contains("missing-file"));
    assert_eq!(first.pdf, original);
    s.add_file("main.tex", HELLO.as_bytes()).unwrap();
    let recovered = s.compile("main.tex");
    assert_eq!(
        recovered.status,
        Status::Success,
        "{}",
        recovered.diagnostics
    );
    assert_eq!(recovered.pdf, original);
    s.remove_file("main.tex").unwrap();
    assert_eq!(s.compile("main.tex").status, Status::InvalidInput);
    assert!(s.add_file("../outside.tex", b"").is_err());
    assert!(s.add_file("/etc/passwd", b"").is_err());
}

#[test]
fn nonconvergence_is_an_explicit_failure() {
    let s = session(
        r"\documentclass{article}
\newcount\passno
\InputIfFileExists{counter.tex}{}{\passno=0}
\advance\passno by 1
\newwrite\statefile
\immediate\openout\statefile=counter.tex
\immediate\write\statefile{\string\passno=\the\passno\relax}
\immediate\closeout\statefile
\begin{document}Pass \the\passno.\end{document}",
    );
    let r = s.compile("main.tex");
    assert_eq!(
        r.status,
        Status::NoConvergence,
        "{}\n{}",
        r.diagnostics,
        r.log
    );
    assert_eq!(r.passes, 5);
    assert!(r.pdf.is_empty());
}

#[test]
fn independent_native_threads_keep_their_projects_separate() {
    let handles: Vec<_> = ["Alpha", "Beta"]
        .into_iter()
        .map(|word| {
            std::thread::spawn(move || {
                let s = session(&format!(
                    r"\documentclass{{article}}\begin{{document}}{word}\end{{document}}"
                ));
                let r = s.compile("main.tex");
                assert_eq!(r.status, Status::Success, "{}", r.diagnostics);
                let pdf = lopdf::Document::load_mem(&r.pdf).unwrap();
                assert!(pdf.extract_text(&[1]).unwrap().contains(word));
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn user_images_and_type1_fonts_are_embedded_from_memory() {
    let mut s = session(
        r"\documentclass{article}\usepackage{graphicx}
\pdfmapline{customfont CMR10 <customfont.pfb}
\begin{document}\font\custom=customfont at 10pt\custom Custom font.
\includegraphics[width=10pt]{picture.png}\end{document}",
    );
    s.add_file(
        "customfont.tfm",
        &tex_kpse::get_embedded_package("cmr10.tfm").unwrap(),
    )
    .unwrap();
    s.add_file(
        "customfont.pfb",
        &tex_kpse::get_embedded_package("cmr10.pfb").unwrap(),
    )
    .unwrap();
    s.add_file(
        "picture.png",
        &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15,
            0, 1, 5, 1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ],
    )
    .unwrap();
    let r = s.compile("main.tex");
    assert_eq!(r.status, Status::Success, "{}\n{}", r.diagnostics, r.log);
    let pdf = lopdf::Document::load_mem(&r.pdf).unwrap();
    assert!(pdf.objects.values().any(|o| o.as_stream().is_ok_and(|s| s
        .dict
        .get(b"Subtype")
        .and_then(lopdf::Object::as_name)
        .ok()
        == Some(b"Image".as_slice()))));
    assert!(pdf
        .objects
        .values()
        .any(|o| o.as_dict().is_ok_and(|d| d.has(b"FontFile"))));
    assert!(pdf.extract_text(&[1]).unwrap().contains("Custom"));
}

#[test]
fn subdirectory_entry_and_local_package_override() {
    let mut s = Session::new();
    s.add_file("src/main.tex", br"\documentclass{article}\usepackage{amsmath}\begin{document}\localmarker\input{body}\end{document}").unwrap();
    s.add_file(
        "src/amsmath.sty",
        br"\ProvidesPackage{amsmath}\newcommand\localmarker{Local override.}",
    )
    .unwrap();
    s.add_file("src/body.tex", b"Nested entry.").unwrap();
    let r = s.compile("src/main.tex");
    assert_eq!(r.status, Status::Success, "{}\n{}", r.diagnostics, r.log);
    assert!(r.files.contains_key("src/main.pdf"));
    let pdf = lopdf::Document::load_mem(&r.pdf).unwrap();
    let text = pdf.extract_text(&[1]).unwrap();
    assert!(text.contains("Local override"), "{text}");
    assert!(text.contains("Nested entry"), "{text}");
}

#[test]
fn native_fontspec_selection_and_styles_in_memory_fs() {
    let mut s = session(
        r"\documentclass{article}
\usepackage{fontspec}
\setmainfont{Latin Modern Roman}
\setsansfont{Latin Modern Sans}
\setmonofont{Latin Modern Mono}
\begin{document}
\section{Native Fontspec Heading}
Regular Roman text. \textbf{Bold Roman glyphs.} \textit{Italic Roman shapes.}
{\sffamily Sans Serif text.}
{\ttfamily Monospace typewriter text.}
\end{document}",
    );
    let r = s.compile("main.tex");
    assert_eq!(r.status, Status::Success, "{}\n{}", r.diagnostics, r.log);
    assert!(r.pdf.starts_with(b"%PDF-"));
    let mut pdf = lopdf::Document::load_mem(&r.pdf).unwrap();
    assert_eq!(pdf.get_pages().len(), 1);
    assert!(
        pdf.objects.values().any(|o| o
            .as_dict()
            .is_ok_and(|d| { d.has(b"FontFile2") || d.has(b"FontFile3") })),
        "PDF must contain embedded OpenType FontFile2 or FontFile3 stream"
    );
    let text = native_text(&mut pdf);
    assert!(text.contains("Native Fontspec Heading"), "{text}");
    assert!(text.contains("Bold Roman glyphs"), "{text}");
    assert!(text.contains("Sans Serif text"), "{text}");
    assert!(text.contains("Monospace typewriter text"), "{text}");
}

#[test]
fn native_font_missing_or_bad_selection_returns_explicit_error() {
    let mut s = session(
        r"\documentclass{article}
\usepackage{fontspec}
\setmainfont{NonexistentPhantomFont12345}
\begin{document}
Should fail because font is missing.
\end{document}",
    );
    let r = s.compile("main.tex");
    assert_eq!(r.status, Status::CompilationError);
    assert!(r.pdf.is_empty());
    assert!(
        r.diagnostics.contains("NonexistentPhantomFont12345")
            || r.log.contains("NonexistentPhantomFont12345")
            || r.diagnostics.contains("font")
            || r.log.contains("font"),
        "diagnostics: {}\nlog: {}",
        r.diagnostics,
        r.log
    );
}

#[test]
fn native_font_project_local_override_and_isolation_between_sessions() {
    let font_bytes = tex_kpse::get_embedded_package("lmroman10-regular.otf")
        .expect("lmroman10-regular.otf should be embedded");

    // Session 1: supplies a local font file with Path=./
    let mut s1 = Session::new();
    s1.set_epoch(Some(1_700_000_000)).unwrap();
    s1.add_file("custom.otf", &font_bytes).unwrap();
    s1.add_file(
        "main.tex",
        br"\documentclass{article}
\usepackage{fontspec}
\setmainfont[Path=./]{custom.otf}
\begin{document}
Local font in MemoryFs session.
\end{document}",
    )
    .unwrap();
    let r1 = s1.compile("main.tex");
    assert_eq!(r1.status, Status::Success, "{}\n{}", r1.diagnostics, r1.log);
    assert!(r1.pdf.starts_with(b"%PDF-"));
    let mut pdf1 = lopdf::Document::load_mem(&r1.pdf).unwrap();
    let text1 = native_text(&mut pdf1);
    assert!(text1.contains("Local font in MemoryFs session"), "{text1}");

    // Session 2: completely independent, does not have custom.otf in memory
    let mut s2 = Session::new();
    s2.set_epoch(Some(1_700_000_000)).unwrap();
    s2.add_file(
        "main.tex",
        br"\documentclass{article}
\usepackage{fontspec}
\setmainfont[Path=./]{custom.otf}
\begin{document}
Should fail because custom.otf is not in this session.
\end{document}",
    )
    .unwrap();
    let r2 = s2.compile("main.tex");
    assert_eq!(r2.status, Status::CompilationError);
    assert!(r2.pdf.is_empty());
}
