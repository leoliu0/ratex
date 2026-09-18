//! Regression test: PDF driver primitives — pdfTeX dimension parameters
//! (assignment/\ifdim/\the/\divide), \pdfcolorstack action keywords, and
//! object-number tracking (\pdfobj/\pdfximage/\pdfxform + \pdflast*).
use tex_core::engine::Engine;

const SRC: &str = r#"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\$=3 \catcode`\^=7 \catcode`\_=11 \catcode`\@=11
\immediate\openout15=smoke.out
\pdfinfoomitdate=1
\pdftrailerid{}
\pdfsuppressptexinfo=15
\chardef\pdfstack=\pdfcolorstackinit direct{0 g}
\ifnum\pdfstack=0 \immediate\write15{STACKINIT-OK}\else\immediate\write15{STACKINIT-BAD}\fi
\pdfhorigin=1in
\pdfpagewidth=200pt
\pdfpageheight 300pt
\ifdim\pdfpagewidth=200pt \immediate\write15{PW-OK}\else\immediate\write15{PW-BAD}\fi
\divide\pdfpagewidth by 2
\ifdim\pdfpagewidth=100pt \immediate\write15{DIV-OK}\else\immediate\write15{DIV-BAD}\fi
\ifdim\pdfpageheight>299pt \immediate\write15{IFDIM-OK}\else\immediate\write15{IFDIM-BAD}\fi
\immediate\write15{ORIGIN=\the\pdfhorigin}
\pdflinkmargin=5pt
\immediate\write15{LM=\the\pdflinkmargin}
\pdfcolorstack 0 push {1 0 0 rg}
\pdfcolorstack0 pop\relax
\pdfcolorstack0 set {0 g 0 G}
\immediate\write15{CS-OK}
\pdfmapline{zzprobe ZZProbe <cmr10.pfb}
\pdfobj {<< /Type /Test >>}
\immediate\write15{LASTOBJ=\the\pdflastobj}
\pdfrefobj\pdflastobj
\pdfximage{fig.png}
\immediate\write15{LASTIMG=\the\pdflastximage}
\pdfrefximage\pdflastximage
\pdfxform0
\immediate\write15{LASTXFORM=\the\pdflastxform}
\pdfobjcompresslevel=9
\immediate\write15{OCL=\the\pdfobjcompresslevel}
\immediate\write15{OCLDEF=\number\pdfobjcompresslevel}
\pdfinfo{/Producer (document override)}
\immediate\closeout15
\end
"#;

fn write_one_pixel_png(path: &std::path::Path) {
    std::fs::write(
        path,
        [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00,
            0x00, 0xb5, 0x1c, 0x0c, 0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78,
            0xda, 0x63, 0x64, 0xf8, 0x0f, 0x00, 0x01, 0x05, 0x01, 0x01, 0x27, 0x18, 0xe3, 0x66,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ],
    )
    .unwrap();
}

#[test]
fn pdf_driver_primitives_smoke() {
    let dir_buf = std::env::temp_dir().join(format!("pdf_smoke_{}", std::process::id()));
    std::fs::create_dir_all(&dir_buf).unwrap();
    let dir = dir_buf.to_string_lossy().replace('\\', "/");
    let output = dir_buf.join("smoke.out");
    let _ = std::fs::remove_file(&output);
    let image = dir_buf.join("fig.png");
    write_one_pixel_png(&image);
    let image_str = image.to_string_lossy().replace('\\', "/");
    let source = SRC.replace("fig.png", &image_str);
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.out_dir = format!("{dir}/");
    e.input
        .push_file("smoke.tex".to_string(), source.into_bytes());
    e.run();
    let out = std::fs::read_to_string(&output).expect("smoke.out produced");
    let norm: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    for expect in [
        "PW-OK",
        "DIV-OK",
        "IFDIM-OK",
        "CS-OK",
        "STACKINIT-OK",
        "ORIGIN=72.26999pt",
        "LM=5.0pt",
        "OCL=9",
        "OCLDEF=9",
    ] {
        assert!(
            norm.contains(expect),
            "missing {expect} in: {norm}\nTERM: {}",
            e.term
        );
    }
    assert!(!norm.contains("BAD"), "BAD marker in: {norm}");
    assert_eq!(e.error_count, 0, "{}", e.term);
    assert!(e.font_loader.map.contains_key("zzprobe"));
    let pdf_bytes = tex_core::pdffile::write_pdf(&e.pdf_doc);
    let pdf = lopdf::Document::load_mem(&pdf_bytes).expect("valid PDF");
    let info = pdf
        .trailer
        .get(b"Info")
        .and_then(lopdf::Object::as_reference)
        .and_then(|id| pdf.get_dictionary(id))
        .expect("PDF info dictionary");
    assert_eq!(
        info.get(b"Producer")
            .and_then(lopdf::Object::as_str)
            .expect("Producer"),
        b"document override"
    );
    std::fs::remove_file(image).unwrap();
}

#[test]
fn file_stream_preserves_binary_bytes_and_following_input() {
    let path = std::env::temp_dir().join(format!(
        "rustex-pdf-stream-{}-{:?}.bin",
        std::process::id(),
        std::thread::current().id()
    ));
    let payload = b"\0\xff\x80abc\n";
    std::fs::write(&path, payload).unwrap();
    let source = r#"\catcode`\{=1 \catcode`\}=2
\pdfobj reserveobjnum \count0=\pdflastobj
\immediate\pdfobj useobjnum\count0 stream attr{/Probe /Binary} file{PAYLOAD}
\count1=73
\shipout\hbox{\vrule width1pt height1pt}
\end"#
        .replace("PAYLOAD", &path.to_string_lossy().replace('\\', "/"));
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.input.push_file("stream.tex".into(), source.into_bytes());
    e.run();
    std::fs::remove_file(path).unwrap();
    assert_eq!(e.error_count, 0, "{}", e.term);
    assert_eq!(e.eqtb.count[1], 73);
    let bytes = tex_core::pdffile::write_pdf(&e.pdf_doc);
    let pdf = lopdf::Document::load_mem(&bytes).unwrap();
    let stream = pdf
        .get_object((e.eqtb.count[0] as u32, 0))
        .unwrap()
        .as_stream()
        .unwrap();
    assert_eq!(
        stream.dict.get(b"Probe").unwrap().as_name().unwrap(),
        b"Binary"
    );
    assert_eq!(stream.content, payload);
}

#[test]
fn pdfrestore_keeps_following_image_in_the_restored_coordinate_system() {
    let dir = std::env::temp_dir().join(format!(
        "rustex-pdf-restore-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let image = dir.join("pixel.png");
    write_one_pixel_png(&image);
    let source = r#"\catcode`\{=1 \catcode`\}=2
\pdfpagewidth=100pt \pdfpageheight=100pt
\pdfhorigin=0pt \pdfvorigin=0pt
\pdfximage width 10pt {IMAGE}
\setbox0=\hbox{\pdfsave\pdfsetmatrix{2 0 0 2}\pdfrefximage\pdflastximage\pdfrestore\kern10pt\pdfrefximage\pdflastximage}
\shipout\box0
\end"#
        .replace("IMAGE", &image.to_string_lossy().replace('\\', "/"));
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.input.push_file("restore.tex".into(), source.into_bytes());
    e.run();
    assert_eq!(e.error_count, 0, "{}", e.term);

    let bytes = tex_core::pdffile::write_pdf(&e.pdf_doc);
    let pdf = lopdf::Document::load_mem(&bytes).unwrap();
    let page = *pdf.get_pages().values().next().expect("one output page");
    let stream = String::from_utf8(pdf.get_page_content(page)).unwrap();
    let image_ops: Vec<&str> = stream
        .lines()
        .filter(|line| line.trim_end().ends_with(" Do"))
        .collect();
    assert_eq!(image_ops.len(), 2, "{stream}");
    assert!(
        image_ops[0].starts_with("9.963 0 0 9.963 0 0 cm "),
        "{stream}"
    );
    assert!(
        image_ops[1].starts_with("9.963 0 0 9.963 9.962 0 cm "),
        "{stream}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn display_list_captures_rules_and_glyphs() {
    let source = r#"\catcode`\{=1 \catcode`\}=2
\pdfpagewidth=100pt \pdfpageheight=100pt
\pdfhorigin=0pt \pdfvorigin=0pt
\setbox0=\hbox{\hrule width 50pt height 5pt depth 0pt}
\shipout\box0
\end"#;
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.input.push_file("display_list.tex".into(), source.as_bytes().to_vec());
    e.run();
    assert_eq!(e.error_count, 0, "{}", e.term);
    assert_eq!(e.pdf_doc.pages.len(), 1);
    let page = &e.pdf_doc.pages[0];
    let dl = page.display_list.as_ref().expect("display list should be present");
    assert!(!dl.is_empty());
    let has_rule = dl.items.iter().any(|item| matches!(item, tex_core::boxes::DisplayItem::Rule { width_bp, .. } if (*width_bp - 49.8).abs() < 1.0));
    assert!(has_rule, "display list should capture the 50pt rule: {:?}", dl.items);
}

#[test]
fn tagged_pdf_emits_markinfo_and_struct_tree_root() {
    let source = r#"\catcode`\{=1 \catcode`\}=2
\pdfpagewidth=100pt \pdfpageheight=100pt
\pdfhorigin=0pt \pdfvorigin=0pt
\setbox0=\hbox{\hrule width 50pt height 5pt depth 0pt}
\shipout\box0
\end"#;
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.input.push_file("tagged.tex".into(), source.as_bytes().to_vec());
    e.run();
    assert_eq!(e.error_count, 0, "{}", e.term);
    assert_eq!(e.pdf_doc.pages.len(), 1);
    let page = &mut e.pdf_doc.pages[0];
    let dl = page.display_list.as_mut().expect("display list should be present");
    dl.push(tex_core::boxes::DisplayItem::GlyphRun {
        font: 0,
        x_bp: 10.0,
        y_bp: 10.0,
        glyphs: vec![b'H', b'i'],
        tag: Some(tex_core::boxes::StructureTag::Paragraph),
        span: Some(tex_core::boxes::SpanId(42)),
    });
    let bytes = tex_core::pdffile::write_pdf(&e.pdf_doc);
    let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");
    let catalog = pdf
        .trailer
        .get(b"Root")
        .and_then(lopdf::Object::as_reference)
        .and_then(|id| pdf.get_dictionary(id))
        .expect("PDF catalog dictionary");
    assert!(catalog.has(b"MarkInfo"), "Catalog must have /MarkInfo: {:?}", catalog);
    assert!(catalog.has(b"StructTreeRoot"), "Catalog must have /StructTreeRoot: {:?}", catalog);
}
