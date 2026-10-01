//! pdfTeX resource names (`/F<n>`, `/Fm<n>`, `/Im<n>`), object numbers of
//! the query primitives, expandability of the convert commands and the
//! Info/trailer entries, checked against TeX Live 2026 `pdftex -ini`.

use tex_core::Engine;

fn engine(source: &str) -> Engine {
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    let source = format!(
        r"\catcode`\{{=1 \catcode`\}}=2 \catcode`\#=6
{source}"
    );
    engine
        .input
        .push_file("names.tex".into(), source.into_bytes());
    engine.run();
    assert_eq!(engine.error_count, 0, "{}", engine.term);
    engine
}

fn pdf_of(engine: &mut Engine) -> lopdf::Document {
    let bytes = tex_core::driver::finish_pdf(engine, false).expect("PDF output");
    lopdf::Document::load_mem(&bytes).expect("valid PDF")
}

fn resource_names(pdf: &lopdf::Document, page: usize, kind: &[u8]) -> Vec<String> {
    let id = *pdf.get_pages().get(&(page as u32)).unwrap();
    let page = pdf.get_dictionary(id).unwrap();
    let resources = match page.get(b"Resources").unwrap() {
        lopdf::Object::Reference(r) => pdf.get_dictionary(*r).unwrap(),
        object => object.as_dict().unwrap(),
    };
    match resources.get(kind) {
        Ok(object) => {
            let dict = object
                .as_dict()
                .ok()
                .or_else(|| pdf.get_dictionary(object.as_reference().unwrap()).ok())
                .unwrap();
            let mut names: Vec<String> =
                dict.iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).collect();
            names.sort();
            names
        }
        Err(_) => Vec::new(),
    }
}

/// A font resource is named after the font that owns the dictionary, so
/// sizes of one TFM share `/F<n>` and `Tf` switches only the size.
#[test]
fn sizes_of_one_tfm_share_the_owner_font_resource() {
    let mut e = engine(
        r"\font\a=cmr10 \font\b=cmr10 at 12pt \font\c=cmbx10
\message{[\pdffontname\a/\pdffontname\b/\pdffontname\c/\pdffontobjnum\a/\pdffontobjnum\c]}
\shipout\hbox{\a A\b B\c C\a D}
\end",
    );
    assert!(e.term.contains("[1/1/3/1/2]"), "{}", e.term);
    let content = String::from_utf8_lossy(&e.pdf_doc.pages[0].content).into_owned();
    assert!(content.contains("/F1 9.9626 Tf"), "{content}");
    assert!(content.contains("/F1 11.9552 Tf"), "{content}");
    assert!(content.contains("/F3 9.9626 Tf"), "{content}");
    let pdf = pdf_of(&mut e);
    assert_eq!(resource_names(&pdf, 1, b"Font"), ["F1", "F3"]);
}

/// Forms are named by their creation count, not their object number, and a
/// form is written only when a shipped page paints it.
#[test]
fn forms_are_named_by_count_and_shipped_when_painted() {
    let mut e = engine(
        r"\font\a=cmr10
\setbox0\hbox{\a x}\setbox1\hbox{\a y}
\pdfxform0 \edef\first{\the\pdflastxform}
\pdfxform1 \edef\second{\the\pdflastxform}
\message{[\first/\pdfxformname\first/\second/\pdfxformname\second]}
\shipout\hbox{\pdfrefxform\second}
\end",
    );
    assert!(e.term.contains("[1/1/2/2]"), "{}", e.term);
    let content = String::from_utf8_lossy(&e.pdf_doc.pages[0].content).into_owned();
    assert!(content.contains("/Fm2 Do"), "{content}");
    let pdf = pdf_of(&mut e);
    assert_eq!(resource_names(&pdf, 1, b"XObject"), ["Fm2"]);
    // the unpainted first form never reaches the file
    let forms = pdf
        .objects
        .values()
        .filter(|o| {
            o.as_stream().is_ok_and(|s| {
                s.dict.get(b"Subtype").and_then(lopdf::Object::as_name).ok() == Some(b"Form")
            })
        })
        .count();
    assert_eq!(forms, 1);
}

/// `\endinput` is expandable: inside `\edef` it vanishes and still ends the
/// file after the current line.
#[test]
fn endinput_expands_to_nothing_inside_edef() {
    let e = engine(
        "\\edef\\x{a\\endinput b}\\message{X=\\x}\n\\message{NOT-READ}\n\\end\n",
    );
    assert!(e.term.contains("X=ab"), "{}", e.term);
    assert!(!e.term.contains("NOT-READ"), "{}", e.term);
}

/// pdftex.web `pdf_print_info` and the trailer: Producer first, then
/// Creator and Trapped, `PTEX.Fullbanner` unless suppressed (underscore
/// spelling on request) and /ID, the MD5 of `\pdftrailerid` when given.
#[test]
fn info_dictionary_and_trailer_follow_pdftex() {
    let mut e = engine(r"\shipout\hbox{}\end");
    let pdf = pdf_of(&mut e);
    let info = pdf.trailer.get(b"Info").unwrap().as_reference().unwrap();
    let info = pdf.get_dictionary(info).unwrap();
    let text = |key: &[u8]| String::from_utf8_lossy(info.get(key).unwrap().as_str().unwrap()).into_owned();
    assert_eq!(text(b"Producer"), "pdfTeX-1.40.29");
    assert_eq!(text(b"Creator"), "TeX");
    assert!(text(b"PTEX.Fullbanner").starts_with("This is pdfTeX, Version 3.141592653-2.6-1.40.29"));
    assert_eq!(info.get(b"Trapped").unwrap().as_name().unwrap(), b"False");
    assert!(pdf.trailer.get(b"ID").is_ok());

    let mut e = engine(r"\pdfptexuseunderscore=1 \pdftrailerid{abc}\shipout\hbox{}\end");
    let pdf = pdf_of(&mut e);
    let info = pdf.trailer.get(b"Info").unwrap().as_reference().unwrap();
    let info = pdf.get_dictionary(info).unwrap();
    assert!(info.get(b"PTEX_Fullbanner").is_ok() && info.get(b"PTEX.Fullbanner").is_err());
    let id = pdf.trailer.get(b"ID").unwrap().as_array().unwrap()[0].as_str().unwrap().to_vec();
    assert_eq!(id, md5::compute(b"abc").0);

    let mut e = engine(r"\pdfsuppressptexinfo=1 \pdftrailerid{}\shipout\hbox{}\end");
    let pdf = pdf_of(&mut e);
    let info = pdf.trailer.get(b"Info").unwrap().as_reference().unwrap();
    let info = pdf.get_dictionary(info).unwrap();
    assert!(info.get(b"PTEX.Fullbanner").is_err() && info.get(b"PTEX_Fullbanner").is_err());
    assert!(pdf.trailer.get(b"ID").is_err(), "empty \\pdftrailerid writes no /ID");
}

/// `\pdfuniqueresname` appends the same six-character job tag to font, form
/// and image resource names in resources and content alike.
#[test]
fn unique_resource_names_carry_the_job_tag() {
    let mut e = engine(
        r"\pdfuniqueresname=1 \font\a=cmr10 \setbox0\hbox{\a x}
\pdfxform0
\shipout\hbox{\a A\pdfrefxform\pdflastxform}
\end",
    );
    let content = String::from_utf8_lossy(&e.pdf_doc.pages[0].content).into_owned();
    let tag = content.split("/F1").nth(1).map(|rest| rest[..6].to_string()).unwrap();
    assert!(content.contains(&format!("/Fm1{tag} Do")), "{content}");
    let pdf = pdf_of(&mut e);
    assert_eq!(resource_names(&pdf, 1, b"Font"), [format!("F1{tag}")]);
    assert_eq!(resource_names(&pdf, 1, b"XObject"), [format!("Fm1{tag}")]);
}

/// pdftex.web `pdf_ship_out` writes a form from the bottom of its box
/// (`cur_page_height` is height + depth): the /BBox is [0 0 w h+d] and the
/// box's baseline sits at y = depth. `out_form` lowers the placement by the
/// same depth, in an hlist and a vlist alike, so the form paints exactly
/// where the box would (page and form content from `pdftex -ini` for this
/// input).
#[test]
fn form_placement_includes_its_depth() {
    let mut e = engine(
        r"\pdfcompresslevel=0
\setbox2\hbox{\vrule width 5pt height 3pt depth 4pt}\pdfxform2
\shipout\hbox{\raise7pt\hbox{\pdfrefxform\pdflastxform}}
\end",
    );
    let content = String::from_utf8_lossy(&e.pdf_doc.pages[0].content).into_owned();
    assert_eq!(content, "q\n1 0 0 1 72 74.989 cm\n/Fm1 Do\nQ\n");
    let pdf = pdf_of(&mut e);
    let form = pdf
        .objects
        .values()
        .filter_map(|object| object.as_stream().ok())
        .find(|stream| stream.dict.get(b"Subtype").and_then(lopdf::Object::as_name).ok() == Some(&b"Form"[..]))
        .expect("form XObject");
    let bbox: Vec<f64> = form
        .dict
        .get(b"BBox")
        .and_then(lopdf::Object::as_array)
        .unwrap()
        .iter()
        .map(|value| match value {
            lopdf::Object::Integer(i) => *i as f64,
            lopdf::Object::Real(r) => f64::from(*r),
            other => panic!("non-numeric /BBox entry {other:?}"),
        })
        .collect();
    let expected = [0.0, 0.0, 4.981, 6.974];
    assert!(bbox.iter().zip(expected).all(|(got, want)| (got - want).abs() < 0.001), "{bbox:?}");
    let drawn = String::from_utf8_lossy(&form.decompressed_content().unwrap()).into_owned();
    assert!(drawn.contains("0 0 4.981 6.974 re f"), "{drawn}");
}
