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
\ifnum\pdfstack=1 \immediate\write15{STACKINIT-OK}\else\immediate\write15{STACKINIT-BAD}\fi
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
    let pdf_bytes = tex_core::pdffile::write_pdf(&e.pdf_doc).expect("valid embedded fonts");
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
    let _ = std::fs::remove_dir_all(&dir_buf);
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
    let bytes = tex_core::pdffile::write_pdf(&e.pdf_doc).expect("valid embedded fonts");
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

    let bytes = tex_core::pdffile::write_pdf(&e.pdf_doc).expect("valid embedded fonts");
    let pdf = lopdf::Document::load_mem(&bytes).unwrap();
    let page = *pdf.get_pages().values().next().expect("one output page");
    let stream = String::from_utf8(pdf.get_page_content(page)).unwrap();
    // pdfTeX out_image: the image cm (4 decimals of bp) precedes `/Im Do`
    let image_ops: Vec<&str> = stream
        .lines()
        .filter(|line| line.starts_with("9.9626 "))
        .collect();
    assert_eq!(
        image_ops,
        ["9.9626 0 0 9.9626 0 0 cm", "9.9626 0 0 9.9626 9.962 0 cm"],
        "{stream}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// pdfTeX color stacks outlive the page: a page-start stack re-emits its
/// current value at the top of the next page, a pop restores the pushed-over
/// value, and forms restart from the initial value. Content streams are the
/// ones `pdftex -ini` writes for the same input.
#[test]
fn color_stacks_carry_across_pages_like_pdftex() {
    let source = r#"\catcode`\{=1 \catcode`\}=2
\count1=\pdfcolorstackinit page direct{0 g}
\shipout\hbox{\pdfcolorstack0 push{1 0 0 rg}\pdfcolorstack\count1 set{0.5 g}}
\shipout\hbox{\pdfcolorstack0 pop\pdfcolorstack0 current}
\setbox2\hbox{\pdfcolorstack0 current}\pdfxform2
\shipout\hbox{\pdfrefxform\pdflastxform}
\end"#;
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.input.push_file("colorstack.tex".into(), source.as_bytes().to_vec());
    e.run();
    assert_eq!(e.error_count, 0, "{}", e.term);
    assert_eq!(e.eqtb.count[1], 1);
    let pages: Vec<String> = e
        .pdf_doc
        .pages
        .iter()
        .map(|p| String::from_utf8(p.content.clone()).unwrap())
        .collect();
    assert_eq!(
        pages,
        [
            "0 g\n1 0 0 rg\n0.5 g\n",
            "1 0 0 rg\n0.5 g\n0 g 0 G\n0 g 0 G\n",
            "0.5 g\nq\n1 0 0 1 72 72 cm\n/Fm1 Do\nQ\n"
        ]
    );
    let bytes = tex_core::pdffile::write_pdf(&e.pdf_doc).expect("valid PDF");
    let pdf = lopdf::Document::load_mem(&bytes).unwrap();
    let form = pdf
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .find(|s| s.dict.get(b"Subtype").and_then(|v| v.as_name()).ok() == Some(b"Form"))
        .expect("form xobject");
    assert_eq!(form.decompressed_content().unwrap_or(form.content.clone()), b"0 g 0 G\n");
}
#[test]
fn display_list_captures_rules_and_glyphs() {
    let source = r#"\catcode`\{=1 \catcode`\}=2
\pdfpagewidth=100pt \pdfpageheight=100pt
\pdfhorigin=0pt \pdfvorigin=0pt
\setbox0=\hbox{\vrule width 50pt height 5pt depth 0pt}
\shipout\box0
\end"#;
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.input
        .push_file("display_list.tex".into(), source.as_bytes().to_vec());
    e.run();
    assert_eq!(e.error_count, 0, "{}", e.term);
    assert_eq!(e.pdf_doc.pages.len(), 1);
    let page = &e.pdf_doc.pages[0];
    let dl = page
        .display_list
        .as_ref()
        .expect("display list should be present");
    assert!(!dl.is_empty());
    let has_rule = dl.items.iter().any(|item| matches!(item, tex_core::boxes::DisplayItem::Rule { width_bp, .. } if (*width_bp - 49.8).abs() < 1.0));
    assert!(
        has_rule,
        "display list should capture the 50pt rule: {:?}",
        dl.items
    );
}

#[test]
fn tagged_pdf_emits_markinfo_and_struct_tree_root() {
    let source = r#"\catcode`\{=1 \catcode`\}=2
\pdfpagewidth=100pt \pdfpageheight=100pt
\pdfhorigin=0pt \pdfvorigin=0pt
\setbox0=\hbox{\vrule width 50pt height 5pt depth 0pt}
\shipout\box0
\end"#;
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.input
        .push_file("tagged.tex".into(), source.as_bytes().to_vec());
    e.run();
    assert_eq!(e.error_count, 0, "{}", e.term);
    assert_eq!(e.pdf_doc.pages.len(), 1);
    let page = &mut e.pdf_doc.pages[0];
    let dl = page
        .display_list
        .as_mut()
        .expect("display list should be present");
    dl.push(tex_core::boxes::DisplayItem::GlyphRun {
        font: 0,
        x_bp: 10.0,
        y_bp: 10.0,
        glyphs: vec![b'H', b'i'],
        tag: Some(tex_core::boxes::StructureTag::Paragraph),
        span: Some(tex_core::boxes::SpanId(42)),
    });
    let bytes = tex_core::pdffile::write_pdf(&e.pdf_doc).expect("valid embedded fonts");
    let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");
    let catalog = pdf
        .trailer
        .get(b"Root")
        .and_then(lopdf::Object::as_reference)
        .and_then(|id| pdf.get_dictionary(id))
        .expect("PDF catalog dictionary");
    assert!(
        catalog.has(b"MarkInfo"),
        "Catalog must have /MarkInfo: {:?}",
        catalog
    );
    assert!(
        catalog.has(b"StructTreeRoot"),
        "Catalog must have /StructTreeRoot: {:?}",
        catalog
    );
}

/// pdfTeX opens the SyncTeX file at a shipout while `\synctex` is nonzero:
/// `\synctex=1` in the document enables it without a command-line option,
/// and `\synctex=0` before the first page leaves no SyncTeX output.
#[test]
fn synctex_parameter_controls_recording_like_pdftex() {
    for (setting, recorded) in [(1, true), (0, false)] {
        let source = format!(
            "\\catcode`\\{{=1 \\catcode`\\}}=2 \\synctex={setting}
\\pdfpagewidth=100pt \\pdfpageheight=100pt
\\pdfhorigin=0pt \\pdfvorigin=0pt
\\setbox0=\\hbox{{\\vrule width 50pt height 5pt depth 0pt}}
\\shipout\\box0
\\end"
        );
        let mut e = Engine::new(true);
        e.init_primitives();
        e.add_nullfont();
        e.input
            .push_file("synctex_doc.tex".into(), source.into_bytes());
        e.run();
        assert_eq!(e.error_count, 0, "{}", e.term);
        assert_eq!(e.pdf_doc.pages.len(), 1);
        assert_eq!(e.synctex.is_open(), recorded, "\\synctex={setting}");
    }
}
#[test]
fn encrypted_pdf_emits_encrypt_dict_and_trailer_id() {
    let source = r#"\catcode`\{=1 \catcode`\}=2
\pdfpagewidth=100pt \pdfpageheight=100pt
\pdfhorigin=0pt \pdfvorigin=0pt
\setbox0=\hbox{\vrule width 50pt height 5pt depth 0pt}
\shipout\box0
\end"#;
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.input
        .push_file("enc.tex".into(), source.as_bytes().to_vec());
    e.run();
    assert_eq!(e.error_count, 0, "{}", e.term);
    assert_eq!(e.pdf_doc.pages.len(), 1);

    let fixed_id = [
        0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
        0x88,
    ];
    let mut enc_cfg = tex_core::pdffile::PdfEncryptConfig::new("user_secret", "owner_secret");
    enc_cfg.permissions = -4;
    enc_cfg.file_id = Some(fixed_id);
    e.pdf_doc.encrypt = Some(enc_cfg);

    let pdf_bytes = tex_core::pdffile::write_pdf(&e.pdf_doc).expect("valid embedded fonts");
    let pdf = lopdf::Document::load_mem(&pdf_bytes).expect("valid PDF");

    // Verify trailer has /Encrypt and /ID
    assert!(
        pdf.trailer.has(b"Encrypt"),
        "Trailer must contain /Encrypt entry: {:?}",
        pdf.trailer
    );
    assert!(
        pdf.trailer.has(b"ID"),
        "Trailer must contain /ID entry: {:?}",
        pdf.trailer
    );

    let encrypt_ref = pdf
        .trailer
        .get(b"Encrypt")
        .and_then(lopdf::Object::as_reference)
        .expect("Encrypt must be an indirect reference");
    let encrypt_dict = pdf.get_dictionary(encrypt_ref).expect("Encrypt dictionary");

    assert_eq!(
        encrypt_dict
            .get(b"Filter")
            .and_then(lopdf::Object::as_name)
            .unwrap(),
        b"Standard"
    );
    assert_eq!(
        encrypt_dict
            .get(b"V")
            .and_then(lopdf::Object::as_i64)
            .unwrap(),
        2
    );
    assert_eq!(
        encrypt_dict
            .get(b"R")
            .and_then(lopdf::Object::as_i64)
            .unwrap(),
        3
    );
    assert_eq!(
        encrypt_dict
            .get(b"Length")
            .and_then(lopdf::Object::as_i64)
            .unwrap(),
        128
    );
    assert_eq!(
        encrypt_dict
            .get(b"P")
            .and_then(lopdf::Object::as_i64)
            .unwrap(),
        -4
    );

    let o_obj = encrypt_dict.get(b"O").expect("/O entry must be present");
    let u_obj = encrypt_dict.get(b"U").expect("/U entry must be present");

    let o_bytes = o_obj.as_str().expect("/O string");
    let u_bytes = u_obj.as_str().expect("/U string");
    assert_eq!(o_bytes.len(), 32, "/O must be 32 bytes");
    assert_eq!(u_bytes.len(), 32, "/U must be 32 bytes");

    // Check cryptographic correctness
    let expected_o = tex_core::pdffile::compute_o_hash(b"user_secret", b"owner_secret");
    assert_eq!(o_bytes, &expected_o[..]);
    let expected_key =
        tex_core::pdffile::compute_file_encryption_key(b"user_secret", &expected_o, -4, &fixed_id);
    let expected_u = tex_core::pdffile::compute_u_hash(&expected_key, &fixed_id);
    assert_eq!(u_bytes, &expected_u[..]);
}

#[test]
fn pdfa_emits_output_intents_with_srgb() {
    let source = r#"\catcode`\{=1 \catcode`\}=2
\pdfpagewidth=100pt \pdfpageheight=100pt
\pdfhorigin=0pt \pdfvorigin=0pt
\pdfminorversion=4
\pdfcatalog{/GTS_PDFA1 (PDF/A-1b)}
\setbox0=\hbox{\vrule width 50pt height 5pt depth 0pt}
\shipout\box0
\end"#;
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.input
        .push_file("pdfa.tex".into(), source.as_bytes().to_vec());
    e.run();
    assert_eq!(e.error_count, 0, "{}", e.term);
    assert_eq!(e.pdf_doc.pages.len(), 1);

    let pdf_bytes = tex_core::pdffile::write_pdf(&e.pdf_doc).expect("valid embedded fonts");
    let pdf = lopdf::Document::load_mem(&pdf_bytes).expect("valid PDF");
    let catalog = pdf
        .trailer
        .get(b"Root")
        .and_then(lopdf::Object::as_reference)
        .and_then(|id| pdf.get_dictionary(id))
        .expect("PDF catalog dictionary");

    assert!(
        catalog.has(b"OutputIntents"),
        "Catalog must contain /OutputIntents: {:?}",
        catalog
    );
    let intents_arr = catalog
        .get(b"OutputIntents")
        .and_then(lopdf::Object::as_array)
        .expect("/OutputIntents must be an array");
    assert!(
        !intents_arr.is_empty(),
        "/OutputIntents array must not be empty"
    );

    let intent_dict = match &intents_arr[0] {
        lopdf::Object::Dictionary(d) => d,
        lopdf::Object::Reference(r) => pdf.get_dictionary(*r).expect("OutputIntent dict"),
        other => panic!("Unexpected object in /OutputIntents: {:?}", other),
    };

    assert_eq!(
        intent_dict
            .get(b"Type")
            .and_then(lopdf::Object::as_name)
            .unwrap(),
        b"OutputIntent"
    );
    assert_eq!(
        intent_dict
            .get(b"S")
            .and_then(lopdf::Object::as_name)
            .unwrap(),
        b"GTS_PDFA1"
    );
    let cid = intent_dict
        .get(b"OutputConditionIdentifier")
        .expect("OutputConditionIdentifier");
    assert_eq!(cid.as_str().unwrap(), b"sRGB");

    // Verify metadata validation function passes
    assert!(tex_core::pdffile::validate_pdfa_metadata(&e.pdf_doc).is_ok());
    let cat_str = format!("{:?}", catalog);
    assert!(cat_str.contains("OutputIntents"));
}

#[test]
fn mapped_truetype_preserves_used_outlines_and_extraction() {
    use std::path::Path;
    let ttf = include_bytes!("fixtures/ratex_test_font.ttf");
    let fs = tex_kpse::fs::MemoryFs::new(Path::new("/project"), 903).unwrap();
    fs.insert(Path::new("mapped.ttf"), ttf.to_vec()).unwrap();
    let mut names = vec!["/.notdef"; 256];
    names[b'A' as usize] = "/A";
    fs.insert(
        Path::new("mapped.enc"),
        format!("/MappedEncoding [ {} ] def", names.join(" ")).into_bytes(),
    )
    .unwrap();
    let _scope = fs.enter();
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.input.push_file(
        "mapped.tex".into(),
        br#"\catcode`\{=1 \catcode`\}=2
\pdfmapline{cmr10 RatexTestFont <mapped.enc <mapped.ttf}
\font\mapped=cmr10 at 10pt
\shipout\hbox{\mapped A}
\end"#
            .to_vec(),
    );
    engine.run();
    assert_eq!(engine.error_count, 0, "{}", engine.term);
    let raw_key = engine.pdf_doc.pages[0].fonts[0].0;
    let font_id = raw_key as u16;
    assert_eq!(
        raw_key,
        tex_core::pdfout::FontBinding::RAW.resource_key(font_id)
    );
    let (semantic_binding, semantic_code) =
        engine
            .pdf_doc
            .get_or_alloc_legacy_code(font_id as usize, b'A', "Alpha");
    let semantic_key = semantic_binding.resource_key(font_id);
    assert_ne!(raw_key, semantic_key);
    let semantic_resource = engine.pdf_doc.pages[0]
        .fonts
        .iter()
        .map(|(_, resource)| *resource)
        .max()
        .unwrap_or(0)
        + 1;
    let page = &mut engine.pdf_doc.pages[0];
    page.fonts.push((semantic_key, semantic_resource));
    page.content.extend_from_slice(
        format!("\nBT /F{semantic_resource} 10 Tf 100 100 Td <{semantic_code:02X}> Tj ET\n")
            .as_bytes(),
    );
    let bytes = tex_core::driver::finish_pdf(&mut engine, false).expect("mapped font finalization");
    let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");
    let parent = pdf
        .objects
        .values()
        .filter_map(|object| object.as_dict().ok())
        .find(|dict| dict.get(b"Subtype").and_then(lopdf::Object::as_name).ok() == Some(b"Type0"))
        .expect("composite font");
    let encoding = parent.get(b"Encoding").unwrap().as_reference().unwrap();
    let encoding = pdf
        .get_object(encoding)
        .unwrap()
        .as_stream()
        .unwrap()
        .decompressed_content()
        .unwrap();
    let encoding = String::from_utf8(encoding).unwrap();
    let gid: u16 = encoding
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next()? == "<41>")
                .then(|| fields.next()?.parse().ok())
                .flatten()
        })
        .expect("A must resolve to a subset glyph");
    assert_ne!(gid, 0, "a used letter cannot become .notdef");
    let descendant = parent.get(b"DescendantFonts").unwrap().as_array().unwrap()[0]
        .as_reference()
        .unwrap();
    let descendant = pdf.get_dictionary(descendant).unwrap();
    let descriptor = descendant
        .get(b"FontDescriptor")
        .unwrap()
        .as_reference()
        .unwrap();
    let descriptor = pdf.get_dictionary(descriptor).unwrap();
    let program = descriptor
        .get(b"FontFile2")
        .unwrap()
        .as_reference()
        .unwrap();
    let program = pdf
        .get_object(program)
        .unwrap()
        .as_stream()
        .unwrap()
        .decompressed_content()
        .unwrap();
    let embedded = ttf_parser::Face::parse(&program, 0).expect("standalone TrueType subset");
    let original = ttf_parser::Face::parse(ttf, 0).unwrap();
    let original_gid = original.glyph_index('A').unwrap();
    assert!(embedded.number_of_glyphs() < original.number_of_glyphs());
    assert_eq!(
        embedded.glyph_bounding_box(ttf_parser::GlyphId(gid)),
        original.glyph_bounding_box(original_gid)
    );
    assert_eq!(
        embedded.glyph_hor_advance(ttf_parser::GlyphId(gid)),
        original.glyph_hor_advance(original_gid)
    );
    assert_eq!(pdf.extract_text(&[1]).unwrap().trim(), "A");
    let semantic_mapping = format!("<{semantic_code:02X}> <0041006C007000680061>").into_bytes();
    assert!(
        pdf.objects
            .values()
            .filter_map(|object| object.as_stream().ok())
            .filter_map(|stream| stream.decompressed_content().ok())
            .any(|content| content
                .windows(semantic_mapping.len())
                .any(|window| window == semantic_mapping)),
        "semantic remap must retain its own ToUnicode mapping"
    );
    let composite_fonts = pdf
        .objects
        .values()
        .filter_map(|object| object.as_dict().ok())
        .filter(|dict| dict.get(b"Subtype").and_then(lopdf::Object::as_name).ok() == Some(b"Type0"))
        .count();
    assert_eq!(
        composite_fonts, 2,
        "raw and semantic code spaces need distinct font dictionaries"
    );

    engine.pdf_doc.fonts[0].used_gids.insert(u16::MAX);
    assert!(
        tex_core::pdffile::write_pdf(&engine.pdf_doc).is_err(),
        "invalid glyphs must abort serialization, not fall back to mismapped full-font bytes"
    );
}

#[test]
fn bundled_jpeg_embeds_without_disk_and_project_image_takes_precedence() {
    use std::path::Path;

    fn compile_image() -> lopdf::Document {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.add_nullfont();
        engine.input.push_file(
            "image.tex".into(),
            br#"\catcode`\{=1 \catcode`\}=2
\pdfximage{thumbnails/cas-email.jpeg}
\shipout\hbox{\pdfrefximage\pdflastximage}
\end"#
                .to_vec(),
        );
        engine.run();
        assert_eq!(engine.error_count, 0, "{}", engine.term);
        let bytes = tex_core::driver::finish_pdf(&mut engine, false).unwrap();
        lopdf::Document::load_mem(&bytes).unwrap()
    }

    let fs = tex_kpse::fs::MemoryFs::new(Path::new("/project"), 0).unwrap();
    let _scope = fs.enter();
    let pdf = compile_image();
    let image = pdf
        .objects
        .values()
        .filter_map(|object| object.as_stream().ok())
        .find(|stream| {
            stream
                .dict
                .get(b"Subtype")
                .and_then(lopdf::Object::as_name)
                .ok()
                == Some(b"Image")
        })
        .expect("Bundled JPEG must be included in the PDF");
    assert_eq!(
        image.dict.get(b"Filter").unwrap().as_name().unwrap(),
        b"DCTDecode"
    );
    assert_eq!(
        image.content,
        tex_kpse::get_embedded_package("cas-email.jpeg").unwrap()
    );

    // A project-provided image shadows the bundled resource. Its actual
    // format is sniffed from the bytes, independently of the extension.
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 1, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[255, 0, 0]).unwrap();
    }
    fs.insert(Path::new("thumbnails/cas-email.jpeg"), png)
        .unwrap();
    let pdf = compile_image();
    let image = pdf
        .objects
        .values()
        .filter_map(|object| object.as_stream().ok())
        .find(|stream| {
            stream
                .dict
                .get(b"Subtype")
                .and_then(lopdf::Object::as_name)
                .ok()
                == Some(b"Image")
        })
        .unwrap();
    assert_eq!(image.dict.get(b"Width").unwrap().as_i64().unwrap(), 1);
    assert_eq!(image.dict.get(b"Height").unwrap().as_i64().unwrap(), 1);
    assert_eq!(
        image.dict.get(b"Filter").unwrap().as_name().unwrap(),
        b"FlateDecode"
    );
}

#[test]
fn eps_image_includes_natively_without_external_converter() {
    let dir = std::env::temp_dir().join(format!("eps_smoke_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let eps_file = dir.join("test_box.eps");
    std::fs::write(
        &eps_file,
        b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 120 80\n%%EndComments\n0.2 0.4 0.8 setrgbcolor\nnewpath\n10 10 moveto\n110 10 lineto\n110 70 lineto\n10 70 lineto\nclosepath\nfill\nshowpage\n%%EOF\n",
    )
    .unwrap();

    let tex_source = format!(
        "\\catcode`\\{{=1 \\catcode`\\}}=2 \\pdfximage{{{}}}\\immediate\\pdfximage{{{}}}\\noindent\\pdfrefximage\\pdflastximage\\end\n",
        eps_file.to_string_lossy().replace('\\', "/"),
        eps_file.to_string_lossy().replace('\\', "/")
    );

    let mut eng = Engine::new(true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.out_dir = format!("{}/", dir.display());
    eng.input
        .push_file("doc.tex".to_string(), tex_source.into_bytes());
    eng.run();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(eng.error_count, 0, "errors: {:?}, term: {}", eng.diagnostics, eng.term);
    assert!(!eng.pdf_doc.objects.is_empty(), "PDF objects must be produced");
    let mut found_form = false;
    for (_, bytes) in &eng.pdf_doc.objects {
        let s = String::from_utf8_lossy(bytes);
        if s.contains("/Subtype /Form") {
            found_form = true;
            break;
        }
    }
    assert!(found_form, "Form XObject for EPS must be embedded");
}
#[test]
fn cjk_latin_mixed_script_raw_binding_isolation() {
    let dir_buf = std::env::temp_dir().join(format!("cjk_mix_{}", std::process::id()));
    std::fs::create_dir_all(&dir_buf).unwrap();
    let dir = dir_buf.to_string_lossy().replace('\\', "/");

    let tex_source = "\\catcode`\\{=1 \\catcode`\\}=2\n\\font\\tenrm=cmr10\n\\tenrm\n\\texrescjktext{4E}1\\relax\nA\n\\texrescjktext{}0\\relax\nB\n\\end\n";

    let mut eng = Engine::new(true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.out_dir = format!("{}/", dir);
    eng.input
        .push_file("doc.tex".to_string(), tex_source.as_bytes().to_vec());
    eng.run();
    let _ = std::fs::remove_dir_all(&dir_buf);

    assert_eq!(eng.error_count, 0, "errors: {:?}", eng.diagnostics);
    let bindings = eng.pdf_doc.legacy_bindings.get(&1);
    assert!(
        bindings.is_none() || bindings.unwrap().is_empty(),
        "CMR10 must not acquire legacy remapped bindings when mixed with CJK text whatsit"
    );
}

fn named_destination_page(pdf: &lopdf::Document, name: &[u8]) -> lopdf::ObjectId {
    let catalog = pdf.catalog().expect("PDF catalog");
    let (_, names) = pdf
        .dereference(catalog.get(b"Names").expect("catalog /Names"))
        .expect("resolve catalog /Names");
    let names = names.as_dict().expect("/Names dictionary");
    let (_, destinations) = pdf
        .dereference(names.get(b"Dests").expect("catalog /Names /Dests"))
        .expect("resolve destination name tree");
    let entries = destinations
        .as_dict()
        .and_then(|tree| tree.get(b"Names"))
        .and_then(lopdf::Object::as_array)
        .expect("destination name-tree entries");

    for pair in entries.chunks_exact(2) {
        if pair[0].as_str().ok() != Some(name) {
            continue;
        }
        let (_, value) = pdf.dereference(&pair[1]).expect("resolve named destination");
        return match value {
            lopdf::Object::Array(destination) => destination[0]
                .as_reference()
                .expect("destination page reference"),
            lopdf::Object::Dictionary(dictionary) => {
                let (_, destination) = pdf
                    .dereference(dictionary.get(b"D").expect("destination /D"))
                    .expect("resolve destination /D");
                destination
                    .as_array()
                    .expect("destination array")[0]
                    .as_reference()
                    .expect("destination page reference")
            }
            other => panic!("unexpected named destination value: {other:?}"),
        };
    }
    panic!(
        "named destination not found: {}",
        String::from_utf8_lossy(name)
    );
}

fn goto_link_rect(
    pdf: &lopdf::Document,
    page: lopdf::ObjectId,
    destination: &[u8],
) -> [f64; 4] {
    let page = pdf.get_dictionary(page).expect("page dictionary");
    let (_, annotations) = pdf
        .dereference(page.get(b"Annots").expect("page annotations"))
        .expect("resolve page annotations");
    for annotation in annotations.as_array().expect("annotation array") {
        let (_, annotation) = pdf.dereference(annotation).expect("resolve annotation");
        let annotation = annotation.as_dict().expect("annotation dictionary");
        let Ok(action) = annotation.get(b"A") else {
            continue;
        };
        let (_, action) = pdf.dereference(action).expect("resolve link action");
        let action = action.as_dict().expect("link action dictionary");
        if action.get(b"S").and_then(lopdf::Object::as_name).ok() != Some(b"GoTo")
            || action.get(b"D").and_then(lopdf::Object::as_str).ok() != Some(destination)
        {
            continue;
        }
        let rect = annotation
            .get(b"Rect")
            .and_then(lopdf::Object::as_array)
            .expect("link rectangle");
        return std::array::from_fn(|index| {
            rect[index].as_float().expect("link rectangle number") as f64
        });
    }
    panic!(
        "GoTo annotation not found for {}",
        String::from_utf8_lossy(destination)
    );
}

fn assert_clickable_text_rect(rect: [f64; 4]) {
    let [x0, y0, x1, y1] = rect;
    assert!(
        [x0, y0, x1, y1].iter().all(|coordinate| coordinate.is_finite()),
        "link rectangle must be finite: {rect:?}"
    );
    assert!(
        x0 >= 0.0 && y0 >= 0.0 && x1 <= 612.0 && y1 <= 792.0,
        "link rectangle must stay on the page: {rect:?}"
    );
    assert!(
        (5.0..100.0).contains(&(x1 - x0)),
        "link rectangle must cover only its text width: {rect:?}"
    );
    assert!(
        (5.0..30.0).contains(&(y1 - y0)),
        "link rectangle must cover one text line: {rect:?}"
    );
}

/// Compile `source` with the embedded LaTeX format twice in a fresh temp
/// directory, so state written on the first pass (.aux, hyperref's .out)
/// is read back, and return the finished second-pass engine.
fn compile_latex_twice(job: &str, source: &'static [u8]) -> Engine {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("texres-{job}-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let run_pass = || {
        let mut engine = Engine::new(false);
        tex_core::format::load_format_bytes_into(
            include_bytes!("../../tex-cli/assets/default.fmt.zst"),
            &mut engine,
        )
        .expect("load embedded LaTeX format");
        tex_core::driver::finalize_format_load(&mut engine);
        tex_core::driver::prepare_latex_job(&mut engine);
        engine.set_interaction_mode(tex_core::engine::InteractionMode::Nonstop);
        engine.halt_on_error = true;
        engine.allow_missing_main_aux = true;
        engine.job_name = job.to_string();
        engine.main_dir = Some(dir.clone());
        engine.aux_dir = Some(dir.clone());
        engine.out_dir = format!("{}/", dir.display());
        engine
            .input
            .push_file(format!("{job}.tex"), source.to_vec());
        tex_core::driver::insert_everyjob(&mut engine);
        engine.run();
        for stream in &mut engine.write_streams {
            stream.take();
        }
        assert_eq!(
            engine.error_count, 0,
            "{job} compilation failed:\n{}",
            engine.term
        );
        engine
    };

    drop(run_pass());
    let engine = run_pass();
    std::fs::remove_dir_all(dir).unwrap();
    engine
}

#[test]
fn hyperref_internal_links_resolve_across_pages_with_clickable_rectangles() {
    const SOURCE: &[u8] = br"\documentclass{article}
\usepackage{hyperref}
\begin{document}
\tableofcontents
\section{Anchor}\label{sec:anchor}
Visible text on anchor page.
\newpage
\section{Links}
Jump to \hyperref[sec:anchor]{Anchor}.
\end{document}
";

    let mut engine = compile_latex_twice("hyperlinks", SOURCE);
    assert_eq!(engine.pdf_doc.pages.len(), 2, "expected cross-page fixture");
    let bytes = tex_core::driver::finish_pdf(&mut engine, false).expect("finish PDF");
    let pdf = lopdf::Document::load_mem(&bytes).expect("hyperref output must be a valid PDF");
    let pages = pdf.get_pages();
    let first_page = pages[&1];
    let second_page = pages[&2];

    assert_eq!(
        named_destination_page(&pdf, b"section.1"),
        first_page,
        "section.1 destination must resolve to page 1"
    );
    assert_eq!(
        named_destination_page(&pdf, b"section.2"),
        second_page,
        "section.2 destination must resolve to page 2"
    );

    // The contents entry navigates forward, and the explicit \hyperref
    // navigates back. Both rectangles must be line-sized rather than spanning
    // most of the page and intercepting unrelated clicks.
    assert_clickable_text_rect(goto_link_rect(&pdf, first_page, b"section.2"));
    assert_clickable_text_rect(goto_link_rect(&pdf, second_page, b"section.1"));
}

#[test]
fn referenced_but_undefined_names_fall_back_to_the_first_page_like_pdftex() {
    // setspace loaded after hyperref replaces hyperref's footnote text, so the
    // footnote marks link to Hfootnote.N names that are never defined. pdfTeX
    // warns and points each at the first page; a dangling name is unclickable.
    const SOURCE: &[u8] = br"\documentclass{article}
\usepackage{hyperref}
\usepackage{setspace}
\begin{document}
First page.
\newpage
Text\footnote{A note.}
\end{document}
";

    let mut engine = compile_latex_twice("missingdest", SOURCE);
    engine.finish_job_diagnostics();
    assert!(
        engine.diagnostics.iter().any(|d| d.message
            == "name{Hfootnote.1} has been referenced but does not exist, replaced by a fixed one"),
        "missing destination must be reported:\n{}",
        engine.diagnostic_output
    );
    let bytes = tex_core::driver::finish_pdf(&mut engine, false).expect("finish PDF");
    let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");
    let pages = pdf.get_pages();
    goto_link_rect(&pdf, pages[&2], b"Hfootnote.1");
    assert_eq!(
        named_destination_page(&pdf, b"Hfootnote.1"),
        pages[&1],
        "the stand-in destination must target the first page"
    );
}

#[derive(Debug, PartialEq)]
struct Bookmark {
    title: String,
    count: Option<i64>,
    dest: String,
    children: Vec<Bookmark>,
}

impl Bookmark {
    fn new(title: &str, count: Option<i64>, dest: &str, children: Vec<Bookmark>) -> Self {
        Bookmark {
            title: title.to_string(),
            count,
            dest: dest.to_string(),
            children,
        }
    }
}

/// Children of an outline node, checking the /Parent, /Prev, /Next and
/// /First, /Last links that viewers walk.
fn outline_children(
    pdf: &lopdf::Document,
    parent: lopdf::ObjectId,
    node: &lopdf::Dictionary,
) -> Vec<Bookmark> {
    let reference = |dict: &lopdf::Dictionary, key: &[u8]| {
        dict.get(key)
            .ok()
            .map(|object| object.as_reference().expect("outline link reference"))
    };
    let mut items = Vec::new();
    let mut prev = None;
    let mut next = reference(node, b"First");
    while let Some(id) = next {
        let item = pdf.get_dictionary(id).expect("outline item");
        assert_eq!(
            reference(item, b"Parent"),
            Some(parent),
            "/Parent of {id:?}"
        );
        assert_eq!(reference(item, b"Prev"), prev, "/Prev of {id:?}");
        // a named target, either as /Dest or through a /GoTo action
        let dest = match item.get(b"A") {
            Ok(action) => {
                let (_, action) = pdf.dereference(action).expect("resolve outline action");
                let action = action.as_dict().expect("outline action dictionary");
                assert_eq!(action.get(b"S").unwrap().as_name().unwrap(), b"GoTo");
                action.get(b"D").expect("GoTo /D")
            }
            Err(_) => item.get(b"Dest").expect("outline /A or /Dest"),
        };
        items.push(Bookmark {
            title: lopdf::decode_text_string(item.get(b"Title").expect("outline title"))
                .expect("decodable outline title"),
            count: item.get(b"Count").ok().map(|c| c.as_i64().unwrap()),
            dest: String::from_utf8_lossy(dest.as_str().expect("named target")).into_owned(),
            children: outline_children(pdf, id, item),
        });
        prev = Some(id);
        next = reference(item, b"Next");
    }
    assert_eq!(reference(node, b"Last"), prev, "/Last of {parent:?}");
    items
}

#[test]
fn hyperref_bookmarks_nest_by_level_with_decoded_titles_and_counts() {
    // Level 2 opens sections and closes subsections; the subsubsection
    // lands on the next page, so the outline spans shipouts.
    const SOURCE: &[u8] = "\\documentclass{article}
\\usepackage[bookmarksopen,bookmarksopenlevel=2]{hyperref}
\\begin{document}
\\section{Section 1}
\\section{Section (2)}
\\subsection{Section 2.1}
\\newpage
\\subsubsection{Section 2.1.1}
\\section{Ünïcødé}
\\end{document}
"
    .as_bytes();

    let mut engine = compile_latex_twice("bookmarks", SOURCE);
    assert_eq!(engine.pdf_doc.pages.len(), 2, "expected cross-page fixture");
    let bytes = tex_core::driver::finish_pdf(&mut engine, false).expect("finish PDF");
    let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");
    let root_id = pdf
        .catalog()
        .unwrap()
        .get(b"Outlines")
        .unwrap()
        .as_reference()
        .unwrap();
    let root = pdf.get_dictionary(root_id).unwrap();

    // pdfTeX: open items count visible descendants, closed ones are
    // negated, the root counts all visible items.
    assert_eq!(
        outline_children(&pdf, root_id, root),
        [
            Bookmark::new("Section 1", None, "section.1", vec![]),
            Bookmark::new(
                "Section (2)",
                Some(1),
                "section.2",
                vec![Bookmark::new(
                    "Section 2.1",
                    Some(-1),
                    "subsection.2.1",
                    vec![Bookmark::new(
                        "Section 2.1.1",
                        None,
                        "subsubsection.2.1.1",
                        vec![]
                    )]
                )]
            ),
            Bookmark::new("Ünïcødé", None, "section.3", vec![]),
        ]
    );
    assert_eq!(root.get(b"Count").unwrap().as_i64().unwrap(), 4);
    assert_eq!(
        named_destination_page(&pdf, b"subsubsection.2.1.1"),
        pdf.get_pages()[&2]
    );
}

/// pdfTeX `\pdfnobuiltintounicode` suppresses the generated /ToUnicode of
/// one font and `\pdffontattr` appends entries to its dictionary; both are
/// how cmap.sty and ctex install their own CMaps without duplicate keys.
#[test]
fn font_attr_and_nobuiltin_tounicode_shape_font_dictionaries() {
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.input.push_file(
        "attrs.tex".into(),
        br#"\catcode`\{=1 \catcode`\}=2
\pdfmapline{=cmr10 CMR10 <cmr10.pfb}
\pdfmapline{=cmr12 CMR12 <cmr12.pfb}
\font\plain=cmr12
\pdfgentounicode=1 \pdfglyphtounicode{fi}{0066 0069}
\font\flagged=cmr10
\font\shared=cmr10 at 12pt
\pdfnobuiltintounicode\flagged
\def\attr{/TeXresProbe 7}
\pdffontattr\flagged{\attr}
\shipout\hbox{\plain\char12 \flagged\char12 \shared\char12}
\end"#
            .to_vec(),
    );
    engine.run();
    assert_eq!(engine.error_count, 0, "{}", engine.term);
    let bytes = tex_core::driver::finish_pdf(&mut engine, false).expect("PDF finalization");
    let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");
    let fonts: Vec<_> = pdf
        .objects
        .values()
        .filter_map(|object| object.as_dict().ok())
        .filter(|dict| dict.get(b"Type").and_then(lopdf::Object::as_name).ok() == Some(b"Font"))
        .collect();
    let summary = |dict: &&lopdf::Dictionary| {
        let base = String::from_utf8_lossy(dict.get(b"BaseFont").unwrap().as_name().unwrap());
        let base = base.split('+').last().unwrap().to_string();
        // writefont.c: /Widths is an indirect array object
        let widths = pdf.dereference(dict.get(b"Widths").unwrap()).unwrap().1;
        let width = widths.as_array().unwrap()[0].as_float().unwrap();
        (
            base,
            width.round() as i64,
            dict.has(b"ToUnicode"),
            dict.get(b"TeXresProbe").and_then(lopdf::Object::as_i64).ok(),
        )
    };
    let mut fonts: Vec<_> = fonts.iter().map(summary).collect();
    fonts.sort();
    // pdfTeX writes one font dictionary per TFM, owned by the first shipped
    // font of it: both cmr10 sizes share the flagged font's dictionary
    // (attribute, no CMap, design-size widths); cmr12 keeps its own CMap.
    assert_eq!(
        fonts,
        [
            ("CMR10".to_string(), 556, false, Some(7)),
            ("CMR12".to_string(), 544, true, None),
        ]
    );
}

/// tex.web §625: shipped glue advances by the change in the ROUNDED
/// running stretch total, so a box's glue ends exactly at its width; per-glue
/// rounding drifted by several sp (`\pdflastxpos` disagreed with pdfTeX).
#[test]
fn shipped_glue_rounds_cumulatively() {
    let dir_buf = std::env::temp_dir().join(format!("pdf_glue_{}", std::process::id()));
    std::fs::create_dir_all(&dir_buf).unwrap();
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.out_dir = format!("{}/", dir_buf.to_string_lossy().replace('\\', "/"));
    engine.input.push_file(
        "glue.tex".into(),
        br#"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\pdfhorigin=0pt \pdfvorigin=0pt
\immediate\openout3=glue.out
\dimen0=100.00007pt
\def\g{\hskip 1pt plus 1sp }
\shipout\hbox to\dimen0{\g\g\g\g\g\g\g\pdfsavepos\write3{X=\the\pdflastxpos}}
\immediate\write3{W=\number\dimen0}
\end"#
            .to_vec(),
    );
    engine.run();
    assert_eq!(engine.error_count, 0, "{}", engine.term);
    let out = std::fs::read_to_string(dir_buf.join("glue.out")).expect("glue.out");
    let value = |key: &str| {
        out.lines()
            .find_map(|line| line.strip_prefix(key))
            .unwrap_or_else(|| panic!("{key} missing in {out}"))
            .trim()
            .to_string()
    };
    assert_eq!(value("X="), value("W="));
    let _ = std::fs::remove_dir_all(&dir_buf);
}

/// writefont.c: the built-in /ToUnicode CMap exists only when
/// \pdfgentounicode > 0 at the end of the job, and (tounicode.c) then maps
/// every encoded code, ASCII identities included.
#[test]
fn pdfgentounicode_gates_full_tounicode_cmaps() {
    for gen in [0, 1] {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.add_nullfont();
        engine.input.push_file(
            "gen.tex".into(),
            format!(
                "\\catcode`\\{{=1 \\catcode`\\}}=2\n\\pdfmapline{{=cmr10 CMR10 <cmr10.pfb}}\n\
                 \\pdfglyphtounicode{{A}}{{0041}}\\font\\f=cmr10 \\shipout\\hbox{{\\f A}}\\pdfgentounicode={gen}\n\\end"
            )
            .into_bytes(),
        );
        engine.run();
        assert_eq!(engine.error_count, 0, "{}", engine.term);
        let bytes = tex_core::driver::finish_pdf(&mut engine, false).expect("PDF finalization");
        let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");
        let cmap = pdf
            .objects
            .values()
            .filter_map(|object| object.as_dict().ok())
            .find(|dict| dict.get(b"Type").and_then(lopdf::Object::as_name).ok() == Some(b"Font"))
            .expect("font dictionary")
            .get(b"ToUnicode")
            .ok()
            .map(|reference| {
                let stream = pdf.get_object(reference.as_reference().unwrap()).unwrap();
                String::from_utf8(stream.as_stream().unwrap().decompressed_content().unwrap())
                    .unwrap()
            });
        match gen {
            0 => assert_eq!(cmap, None),
            _ => {
                let cmap = cmap.expect("ToUnicode with \\pdfgentounicode=1");
                assert!(cmap.contains("<41> <0041>"), "{cmap}");
            }
        }
    }
}

/// pdfTeX prints the MediaBox from the sp page size with `pdf_print_bp`
/// (3 decimals) and omits it when \pdfpageattr has its own /MediaBox.
/// Expected values are /usr/bin/pdftex output for the same input.
#[test]
fn mediabox_prints_page_size_like_pdftex() {
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.input.push_file(
        "mediabox.tex".into(),
        br#"\catcode`\{=1 \catcode`\}=2
\pdfpagewidth=210.3mm \pdfpageheight=280.13pt
\shipout\hbox{}
\pdfpageattr{/MediaBox [0 0 10 10]}
\shipout\hbox{}
\end"#
            .to_vec(),
    );
    engine.run();
    assert_eq!(engine.error_count, 0, "{}", engine.term);
    let bytes = tex_core::pdffile::write_pdf(&engine.pdf_doc).expect("PDF serialization");
    let text = String::from_utf8_lossy(&bytes);
    let boxes: Vec<_> = text.match_indices("/MediaBox [").map(|(at, _)| {
        let rest = &text[at..];
        &rest[..rest.find(']').unwrap() + 1]
    }).collect();
    assert_eq!(boxes, ["/MediaBox [0 0 596.126 279.083]", "/MediaBox [0 0 10 10]"]);
}

/// pdfTeX applies \mag to page output: the page stream starts with a
/// `m 0 0 m 0 0 cm` scaling, and the MediaBox, annotation rects and
/// destinations print through `pdf_print_mag_bp`. A later different \mag is
/// "Incompatible magnification" and keeps the first. Expected numbers are
/// /usr/bin/pdftex output for the same input.
#[test]
fn magnification_scales_page_geometry_like_pdftex() {
    for (mag, scale, media, rect, xyz) in [
        (2000, "2", "0 0 398.506 598.755", "150.575 450.77 170.501 456.946", "160.538 452.762"),
        (1095, "1.095", "0 0 218.182 327.818", "82.44 246.796 93.349 250.178", "87.895 247.887"),
    ] {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.add_nullfont();
        engine.input.push_file(
            "mag.tex".into(),
            format!(
                "\\catcode`\\{{=1 \\catcode`\\}}=2 \\mag={mag}\n\
                 \\pdfpagewidth=200pt \\pdfpageheight=300.5pt \\pdfhorigin=1in \\pdfvorigin=1in\n\
                 \\shipout\\vbox{{\\hbox{{\\kern3.3pt\\pdfannot width 10pt height 2.1pt depth 1pt \
                 {{/Subtype /Text}}\\kern5pt\\pdfdest name{{d}} xyz\\vrule width 2pt height 1pt}}}}\n\
                 \\mag=1000 \\shipout\\hbox{{}}\n\\end"
            )
            .into_bytes(),
        );
        engine.run();
        assert_eq!(engine.error_count, 1);
        assert_eq!(
            engine.diagnostics[0].message,
            format!("Incompatible magnification (1000); the previous value will be retained ({mag})")
        );
        let bytes = tex_core::pdffile::write_pdf(&engine.pdf_doc).expect("PDF serialization");
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(text.matches(&format!("/MediaBox [{media}]")).count(), 2, "{mag}");
        assert!(text.contains(&format!("/Rect [{rect}]")), "{mag}: {text}");
        assert!(text.contains(&format!("/XYZ {xyz} null")), "{mag}: {text}");
        let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");
        for page in pdf.get_pages().into_values() {
            let content = String::from_utf8(pdf.get_page_content(page)).unwrap();
            assert!(content.starts_with(&format!("{scale} 0 0 {scale} 0 0 cm\n")), "{content}");
        }
    }
}

/// pdfTeX link rectangles: a running link spans the enclosing line's height
/// and depth, continues as one annotation per line at its box nesting level
/// (`append_link`), ends at \pdfendlink, and is widened by \pdflinkmargin;
/// explicit height/depth override the box. No /Border is invented. Expected
/// rectangles are /usr/bin/pdftex output for the same input.
#[test]
fn link_rectangles_follow_pdftex_running_links() {
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.input.push_file(
        "links.tex".into(),
        br#"\catcode`\{=1 \catcode`\}=2 \pdfoutput=1
\pdfpagewidth=300pt \pdfpageheight=200pt \pdfhorigin=10pt \pdfvorigin=10pt
\font\f=cmr10 \f \hsize=100pt \parindent=0pt \baselineskip=12pt \pdflinkmargin=1pt
\setbox0\vbox{AA \pdfstartlink attr{/Border [0 0 1]} user{/S /URI /URI (http://a.b)}BBB\hfil\penalty-10000 CCC\hfil\penalty-10000 DDD\pdfendlink{} GG
\hbox{xx\pdfstartlink height 9pt depth 2pt user{/S /URI /URI (http://c.d)}yy\pdfendlink}\hfil\penalty-10000}
\shipout\box0
\end"#
            .to_vec(),
    );
    engine.run();
    assert_eq!(engine.error_count, 0, "{}", engine.term);
    let bytes = tex_core::pdffile::write_pdf(&engine.pdf_doc).expect("PDF serialization");
    let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");
    let page = pdf.get_dictionary(pdf.get_pages()[&1]).expect("page dictionary");
    let (_, annotations) = pdf
        .dereference(page.get(b"Annots").expect("page annotations"))
        .expect("resolve page annotations");
    let mut borders = 0;
    let rects: Vec<String> = annotations
        .as_array()
        .expect("annotation array")
        .iter()
        .map(|annotation| {
            let (_, annotation) = pdf.dereference(annotation).expect("resolve annotation");
            let annotation = annotation.as_dict().expect("annotation dictionary");
            borders += annotation.has(b"Border") as usize;
            let rect = annotation.get(b"Rect").and_then(lopdf::Object::as_array).unwrap();
            let rect: Vec<String> =
                rect.iter().map(|n| n.as_float().unwrap().to_string()).collect();
            rect.join(" ")
        })
        .collect();
    assert_eq!(
        rects,
        [
            "27.231 181.486 110.585 190.286",
            "8.966 169.531 110.585 178.331",
            "8.966 155.639 33.79 166.376",
            "64.591 155.583 77.1 168.535",
        ]
    );
    assert_eq!(borders, 3);
}

/// pdfTeX backend primitives against `pdftex -ini` (TeX Live 2026) on the
/// same source: `\pdffontname` follows pdf_init_font sharing (first
/// initialized font of a TFM owns the resource; `\pdfcopyfont` copies share
/// it), `\pdfpageref` names the page object of that page, `\pdftrailer`
/// extends the trailer, `\pdfomitinfodict`/`\pdfomitprocset` drop /Info and
/// /ProcSet, `\pdfincludechars` writes the font with those glyphs, a `+`
/// map line for an already mapped TFM warns once (then suppressed by
/// `\pdfsuppresswarningdupmap`), and IniTeX parameter defaults.
#[test]
fn pdftex_backend_primitives_match_pdftex() {
    let dir_buf = std::env::temp_dir().join(format!("pdf_backend_{}", std::process::id()));
    std::fs::create_dir_all(&dir_buf).unwrap();
    let dir = dir_buf.to_string_lossy().replace('\\', "/");
    let image = dir_buf.join("px.png");
    write_one_pixel_png(&image);
    let source = r#"\catcode`\{=1 \catcode`\}=2 \pdfoutput=1
\immediate\openout15=backend.out
\font\a=cmr10 \font\b=cmr10 at 12pt \font\c=cmbx10 \hyphenchar\c=7 \pdfmovechars=1
\immediate\write15{N:\pdffontname\b,\pdffontname\a,\pdffontname\c}
\pdfcopyfont\d=\c \hyphenchar\d=1
\immediate\write15{C:\fontname\d,\ifx\c\d same\else diff\fi,\the\hyphenchar\c,\the\hyphenchar\d,\pdffontname\d}
\pdfximage{IMAGE}\immediate\write15{D:\the\pdflastximagecolordepth}
\immediate\write15{V:\the\pdfimageresolution,\the\pdfgamma,\the\pdfimagegamma,\the\pdfimagehicolor,\the\pdfpagebox}
\pdftrailer{/TeXresProbe (yes)}\pdfomitinfodict=1 \pdfomitprocset=1
\pdfincludechars\c{AB}
\pdfmapline{+cmr10 CMR10 <cmr10.pfb}\pdfsuppresswarningdupmap=1 \pdfmapline{+cmr10 CMR10 <cmr10.pfb}
\immediate\write15{P:\pdfpageref2}
\shipout\hbox{\a A}\shipout\hbox{\b B}
\immediate\write15{M:\the\pdfmovechars}
\immediate\closeout15
\end"#
        .replace("IMAGE", &image.to_string_lossy().replace('\\', "/"));
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.out_dir = format!("{dir}/");
    e.input.push_file("backend.tex".to_string(), source.into_bytes());
    e.run();
    assert_eq!(e.error_count, 0, "{}", e.term);
    let out = std::fs::read_to_string(dir_buf.join("backend.out")).expect("backend.out");
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    // pdfTeX prints P:5; object numbering differs, so the number is checked
    // against the written page tree below instead.
    assert!(lines.len() == 6 && lines[4].starts_with("P:"), "{lines:?}");
    assert_eq!(
        [lines[0], lines[1], lines[2], lines[3], lines[5]],
        ["N:2,2,3", "C:cmbx10,diff,7,1,3", "D:8", "V:72,1000,2200,1,0", "M:0"]
    );
    assert_eq!(e.log.matches("already exists, duplicates ignored").count(), 1, "{}", e.log);
    assert_eq!(e.log.matches("Primitive \\pdfmovechars is obsolete.").count(), 1);

    let bytes = tex_core::driver::finish_pdf(&mut e, false).expect("PDF finalization");
    let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");
    // \pdfpageref 2 is the object number of the second page
    let page_ref: u32 = lines[4].strip_prefix("P:").unwrap().parse().unwrap();
    let pages: Vec<_> = pdf.get_pages().into_values().collect();
    assert_eq!(pages[1], (page_ref, 0));
    assert_eq!(
        pdf.trailer.get(b"TeXresProbe").and_then(lopdf::Object::as_str).ok(),
        Some(&b"yes"[..])
    );
    assert!(pdf.trailer.get(b"Info").is_err(), "\\pdfomitinfodict keeps /Info");
    assert!(!bytes.windows(8).any(|w| w == b"/ProcSet"));
    let base_fonts: std::collections::BTreeSet<String> = pdf
        .objects
        .values()
        .filter_map(|object| object.as_dict().ok())
        .filter_map(|dict| dict.get(b"BaseFont").ok()?.as_name().ok())
        .map(|name| String::from_utf8_lossy(name).split('+').last().unwrap().to_string())
        .collect();
    assert_eq!(base_fonts, ["CMBX10".to_string(), "CMR10".to_string()].into());
    std::fs::remove_dir_all(dir_buf).unwrap();
}
