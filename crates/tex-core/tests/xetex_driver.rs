//! xdvipdfmx special semantics on an XeTeX `Engine`. The expected values were
//! produced by TeX Live 2026 `xetex` + `xdvipdfmx` (20260113) on the same
//! source; only the parts independent of how the page origin is folded into
//! the content stream are compared (colour operators in order, the page size,
//! literal ordering).

use tex_core::engine::{Engine, EngineKind, InteractionMode};

const SRC: &str = r#"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\pdfpagewidth=200pt \pdfpageheight=100pt
\shipout\vbox{\hrule width 10pt height 5pt
\special{color push rgb 1 0 0}\hrule width 10pt height 5pt
\special{color push cmyk 0 1 1 0}\hrule width 10pt height 5pt\special{color pop}
\hrule width 10pt height 5pt\special{color pop}\hrule width 10pt height 5pt
\special{pdf:bcolor [0.5]}\hrule width 10pt height 5pt\special{pdf:ecolor}
\special{pdf:btrans rotate 90}\hrule width 10pt height 5pt\special{pdf:etrans}
\special{x:gsave}\special{x:scale 2 3}\hrule width 10pt height 5pt\special{x:grestore}
\special{pdf:literal 0 0 m 1 1 l S}\special{pdf:literal direct 2 0 m}
}
\end
"#;

fn page_content() -> (String, i64, i64) {
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng.input.push_file("test.tex".into(), SRC.as_bytes().to_vec());
    eng.run();
    assert_eq!(eng.error_count, 0, "errors: {:?}, term: {}", eng.diagnostics, eng.term);
    let page = &eng.pdf_doc.pages[0];
    (String::from_utf8_lossy(&page.content).split_whitespace().collect::<Vec<_>>().join(" "), page.width_sp, page.height_sp)
}

fn color_ops(content: &str) -> Vec<String> {
    let toks: Vec<&str> = content.split_whitespace().collect();
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let n = match *t {
            "g" | "G" => 1,
            "rg" | "RG" => 3,
            "k" | "K" => 4,
            _ => continue,
        };
        out.push(toks[i - n..=i].join(" "));
    }
    out
}

#[test]
fn color_stack_nesting_follows_xdvipdfmx() {
    let (content, w, h) = page_content();
    assert_eq!((w, h), (200 * 65536, 100 * 65536));
    let ops = color_ops(&content);
    // the colour operators after the initial reset, in TL's order
    let expected = [
        "1 0 0 RG", "1 0 0 rg", "0 1 1 0 K", "0 1 1 0 k", "1 0 0 RG", "1 0 0 rg", "0 G", "0 g", "0.5 G", "0.5 g", "0 G",
        "0 g",
    ];
    let mut at = 0;
    for want in expected {
        match ops[at..].iter().position(|o| o == want) {
            Some(i) => at += i + 1,
            None => panic!("missing {want} after position {at} in {ops:?}"),
        }
    }
}

#[test]
fn literals_stay_in_order_between_graphics_state_changes() {
    let (content, _, _) = page_content();
    let a = content.find("0 0 m 1 1 l S").expect("page literal");
    let b = content.find("2 0 m").expect("direct literal");
    assert!(a < b);
    assert_eq!(content.matches('q').count() >= content.matches('Q').count(), true);
}

#[test]
fn docinfo_overrides_creator_and_metadata_matches_xdvipdfmx() {
    let src = r#"\catcode`\{=1 \catcode`\}=2
\shipout\vbox{\special{pdf:docinfo<</Title(T)/Creator(LaTeX with hyperref)>>}\hrule width 10pt height 5pt}
\end
"#;
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng.input.push_file("test.tex".into(), src.as_bytes().to_vec());
    eng.run();
    let pdf = tex_core::driver::finish_pdf(&mut eng, false).expect("pdf");
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.starts_with("%PDF-1.7"), "{}", &text[..20]);
    assert!(text.contains("/Title (T)"), "{text}");
    assert!(text.contains("/Creator (LaTeX with hyperref)"));
    assert!(text.contains("/Producer (xdvipdfmx \\(20260113\\))"));
    assert!(!text.contains("/Trapped") && !text.contains("PTEX"));
}

fn ship(src: &str) -> (Engine, String) {
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng.input.push_file("test.tex".into(), src.as_bytes().to_vec());
    eng.run();
    assert_eq!(eng.error_count, 0, "errors: {:?}, term: {}", eng.diagnostics, eng.term);
    let content = String::from_utf8_lossy(&eng.pdf_doc.pages[0].content).split_whitespace().collect::<Vec<_>>().join(" ");
    (eng, content)
}

#[test]
fn page_content_starts_at_the_dvi_origin_and_background_precedes_it() {
    let (_, content) = ship(
        r#"\catcode`\{=1 \catcode`\}=2
\pdfpagewidth=612bp \pdfpageheight=792bp
\shipout\vbox{\special{background rgb 1 1 0.94}\hrule width 10pt height 5pt}
\end
"#,
    );
    // xdvipdfmx: background stream first, then q + origin cm
    assert!(
        content.starts_with("q 1 1 0.94 rg q n 0 0 612 792 re f Q Q q 1 0 0 1 72 720 cm 0 G 0 g"),
        "{content}"
    );
    assert!(content.trim_end().ends_with('Q'));
}

const GOTO_SRC: &str = r#"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\pdfpagewidth=200pt \pdfpageheight=100pt
\shipout\vbox{CONFIG\hbox{\special{pdf:dest (alpha) [@thispage /XYZ @xpos @ypos null]}\special{pdf:dest (unused) [@thispage /Fit]}\special{pdf:dest (beta) [@thispage /Fit]}\vrule width 5pt height 5pt}
\hbox{\special{pdf:bann << /Type /Annot /Subtype /Link /Border [0 0 1] /Dest (beta) >>}\vrule width 5pt height 5pt\special{pdf:eann}\special{pdf:bann << /Type /Annot /Subtype /Link /A << /S /GoTo /D (alpha) >> >>}\vrule width 5pt height 5pt\special{pdf:eann}}
\special{pdf:outline 1 << /Title (T) /A << /S /GoTo /D (beta) >> >>}}
\end
"#;

fn goto_pdf(config: &str) -> String {
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng.input.push_file("test.tex".into(), GOTO_SRC.replace("CONFIG", config).into_bytes());
    eng.run();
    assert_eq!(eng.error_count, 0, "errors: {:?}, term: {}", eng.diagnostics, eng.term);
    let pdf = tex_core::driver::finish_pdf(&mut eng, false).expect("pdf");
    // no blanks at all: only the names and their order matter
    String::from_utf8_lossy(&pdf).chars().filter(|c| c.is_ascii_graphic()).collect()
}

/// xdvipdfmx (`-C 0`, the default) renames the destinations links and
/// bookmarks use to 0, 1, ... in order of first use and drops the others.
/// The expectation is the PDF TeX Live 2026 `xdvipdfmx` writes for this input.
#[test]
fn used_destinations_get_short_names_and_unused_ones_go() {
    let text = goto_pdf("");
    assert!(text.contains("/Dest(0)"), "{text}");
    assert!(text.contains("/D(1)"), "{text}");
    assert!(text.contains("/D(0)"), "{text}");
    assert!(text.contains("/Names[(0)[") && text.contains("(1)["), "{text}");
    for gone in ["(alpha)", "(beta)", "(unused)"] {
        assert!(!text.contains(gone), "{gone} in {text}");
    }
}

/// hyperref asks for `dvipdfmx:config C 0x0010`: every destination stays,
/// under its own name.
#[test]
fn config_c_flag_keeps_all_destinations() {
    let text = goto_pdf(r"\special{dvipdfmx:config C 0x0010}");
    assert!(text.contains("/Dest(beta)"), "{text}");
    assert!(text.contains("/D(alpha)"), "{text}");
    for kept in ["(alpha)", "(beta)", "(unused)"] {
        assert!(text.contains(kept), "{kept} missing in {text}");
    }
}

/// The complete content stream TL's xetex + xdvipdfmx write for rules, colour
/// specials, transformations and literals (same sources as the colour test).
#[test]
fn content_stream_equals_xdvipdfmx() {
    let (content, _, _) = page_content();
    assert_eq!(
        content,
        "q 1 0 0 1 72 27.626 cm 0 G 0 g q 4.9813 w 0 -2.491 m 9.963 -2.491 l S Q 1 0 0 RG 1 0 0 rg q 4.9813 w 0 -7.472 m 9.963 -7.472 l S Q 0 1 1 0 K 0 1 1 0 k q 4.9813 w 0 -12.453 m 9.963 -12.453 l S Q 1 0 0 RG 1 0 0 rg q 4.9813 w 0 -17.435 m 9.963 -17.435 l S Q 0 G 0 g q 4.9813 w 0 -22.416 m 9.963 -22.416 l S Q 0.5 G 0.5 g q 4.9813 w 0 -27.397 m 9.963 -27.397 l S Q 0 G 0 g q 0 1 -1 0 -29.888 -29.888 cm q 4.9813 w 0 -32.379 m 9.963 -32.379 l S Q Q 0 G 0 g q 2 0 0 3 0 69.738 cm q 4.9813 w 0 -37.36 m 9.963 -37.36 l S Q Q 0 G 0 g 1 0 0 1 0 -39.851 cm 0 0 m 1 1 l S 1 0 0 1 0 39.851 cm 2 0 m Q"
    );
}

/// Rules inside (nested) `pdf:bcontent` are positioned relative to the
/// translated origin (`dvi_set_compensation`); the stream is TL's.
#[test]
fn bcontent_compensates_rule_positions() {
    let (_, content) = ship(
        r#"\catcode`\{=1 \catcode`\}=2
\pdfpagewidth=200pt \pdfpageheight=100pt
\shipout\vbox{\hbox{\kern20pt\special{pdf:bcontent}\special{pdf:literal 0 0 m 5 5 l S}\raise3pt\hbox{\vrule width 10pt height 2pt}\special{pdf:bcontent}\kern4pt\vrule width 7pt height 6pt\special{pdf:econtent}\special{pdf:econtent}\vrule width 3pt height 6pt}}
\end
"#,
    );
    assert_eq!(
        content,
        "q 1 0 0 1 72 27.626 cm 0 G 0 g q 1 0 0 1 19.925 -5.978 cm 0 0 m 5 5 l S q 1.9925 w 0 3.985 m 9.963 3.985 l S Q q 1 0 0 1 9.963 0 cm q 3.985 0 6.974 5.978 re f Q Q 0 G 0 g Q 0 G 0 g q 2.9888 w 42.341 -5.978 m 42.341 0 l S Q Q"
    );
}

/// TL xetex + xdvipdfmx: a bare picture restores XeTeX's cached DVI
/// coordinates, while the reader remains at the picture. A nested hbox's
/// push/pop discards its extra displacement but keeps the outer one.
#[test]
fn bare_pictures_preserve_dvi_displacement_until_the_containing_box_ends() {
    let image = format!("{}/tests/fixtures/xetex_pic/tmp-1.png", env!("CARGO_MANIFEST_DIR"));
    let (_, content) = ship(&format!(
        r"\catcode`\{{=1 \catcode`\}}=2
\font\a=cmr10 \a
\pdfpagewidth=200pt \pdfpageheight=120pt
\shipout\hbox{{A\kern10pt\XeTeXpicfile {image} width12pt B\hbox{{C\kern5pt\XeTeXpicfile {image} width10pt D}}E\XeTeXpicfile {image} width5pt }}
\end"
    ));
    let operations = lopdf::content::Content::decode(content.as_bytes()).unwrap().operations;
    let mut translation = [0.0f32; 2];
    let mut images = Vec::new();
    for op in operations {
        if op.operator == "cm" {
            translation = [op.operands[4].as_float().unwrap(), op.operands[5].as_float().unwrap()];
        } else if op.operator == "Do" {
            images.push(translation);
        }
    }
    assert_eq!(images, [[17.435, -9.0], [58.586, -9.0], [82.939, -9.0]]);
}

/// TL's link right edge is 205.202bp, not 205.201bp: xdvipdfmx rounds
/// raw TFM fixwords with sqxfw; TeX's stored widths truncate. The 46sp
/// kern puts the difference across PDF's 0.001bp rounding boundary.
#[test]
fn tfm_annotation_metrics_use_the_driver_rounding() {
    let (mut eng, _) = ship(
        r"\catcode`\{=1 \catcode`\}=2
\pdfpagewidth=300pt \pdfpageheight=120pt
\font\a=cmr10 at13.37pt \a
\shipout\hbox{\kern46sp\special{pdf:bann << /Subtype /Link /A << /S /URI /URI (https://example.org) >> >>}MMMMMMMMMMgj\special{pdf:eann}}
\end"
    );
    let bytes = tex_core::driver::finish_pdf(&mut eng, false).unwrap();
    let pdf = lopdf::Document::load_mem(&bytes).unwrap();
    let page = pdf.get_dictionary(pdf.get_pages()[&1]).unwrap();
    let (_, annotations) = pdf.dereference(page.get(b"Annots").unwrap()).unwrap();
    let (_, annotation) = pdf.dereference(&annotations.as_array().unwrap()[0]).unwrap();
    let rect: Vec<f32> = annotation.as_dict().unwrap().get(b"Rect").unwrap().as_array().unwrap()
        .iter().map(|value| value.as_float().unwrap()).collect();
    assert_eq!(rect, [72.001, 35.860, 205.202, 47.552]);
}

#[test]
fn mapline_replaces_the_default_program_and_rejects_pfa() {
    use std::path::Path;
    let pfb = tex_kpse::get_embedded_package("cmr10.pfb").unwrap();
    let pfa = tex_core::pdf_fonts::parse_type1(&pfb).data;
    let fs = tex_kpse::fs::MemoryFs::new(Path::new("/project"), 0).unwrap();
    fs.insert(Path::new("probe.pfa"), pfa).unwrap();
    let _scope = fs.enter();
    let (mut eng, _) = ship(
        r"\catcode`\{=1 \catcode`\}=2
\font\a=cmr10 at10pt \a
\shipout\hbox{\special{pdf:mapline cmr10 CMR10 <probe.pfa}ABC}
\end"
    );
    assert!(tex_core::driver::finish_pdf(&mut eng, false).is_err());
}
