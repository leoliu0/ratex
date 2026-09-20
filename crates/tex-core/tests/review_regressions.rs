use tex_core::engine::Engine;

#[test]
fn balanced_arguments_preserve_guards_and_cross_source_boundaries() {
    use tex_core::token::Token;
    let mut e = Engine::new(true);
    e.init_primitives();
    let relax = e.cs.lookup(b"relax").unwrap();
    let noexpand = Token(tex_core::expand::NOEXP_FLAG | relax);
    let unexpanded = Token(0xe000_0000 | relax);
    let close = Token::char(2, b'}' as u32);
    e.input
        .push_toks(vec![Token::letter(b'A'), close, noexpand], "<test>");
    assert_eq!(&e.scan_balanced_raw(true)[..], &[Token::letter(b'A')]);
    assert_eq!(e.raw_token(), noexpand);

    e.input
        .push_toks(vec![noexpand, unexpanded, close], "<test>");
    assert_eq!(
        &e.scan_balanced_raw(true)[..],
        &[Token::from_cs(relax), unexpanded]
    );

    e.input
        .push_toks(vec![Token::letter(b'B'), close], "<outer>");
    e.input.push_toks(
        vec![Token::char(1, b'{' as u32), Token::letter(b'A'), close],
        "<inner>",
    );
    assert_eq!(
        &e.scan_balanced_raw(true)[..],
        &[
            Token::char(1, b'{' as u32),
            Token::letter(b'A'),
            close,
            Token::letter(b'B'),
        ]
    );
    assert_eq!(e.error_count, 0);
}

#[test]
fn selectors_forward_large_arguments_without_changing_expansion() {
    let argument = "abcdefghijklmnopqrstuvwxyz".repeat(8);
    let e = engine(&format!(
        r"\def\select#1#2{{#2}}
\def\discarded{{BAD}}
\edef\result{{\select{{\discarded}}{{{argument}}}}}
\message{{RESULT=\result}}
\end"
    ));
    assert!(e.term.contains(&format!("RESULT={argument}")), "{}", e.term);
}

#[test]
fn selector_fast_path_preserves_delimiters_guards_and_unselected_arguments() {
    let e = engine(
        r"\def\value{OK}
\def\pick A#1/#2/#3;{#2}
\long\def\drop#1{}
\def\one#1{#1}
\edef\result{\pick A{\undefined}/\value/{\undefined};\drop{\par\undefined}\one{!}}
\def\alias{\value}
\protected\def\protectedvalue{BAD}
\def\protectedalias{\protectedvalue}
\edef\saved{\protectedalias}
\def\expected{\protectedvalue}
\ifx\saved\expected\message{GUARD=OK}\else\message{GUARD=BAD}\fi
\message{RESULT=\result,ALIAS=\alias}
\end",
    );
    assert!(e.term.contains("RESULT=OK!,ALIAS=OK"), "{}", e.term);
    assert!(e.term.contains("GUARD=OK"), "{}", e.term);
}

fn engine(source: &str) -> Engine {
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    let source = format!(
        r"\catcode`\{{=1 \catcode`\}}=2 \catcode`\#=6
\catcode`\$=3 \catcode`\^=7 \catcode`\_=8
{source}"
    );
    engine
        .input
        .push_file("review.tex".into(), source.into_bytes());
    engine.run();
    assert_eq!(engine.error_count, 0, "{}", engine.term);
    engine
}

#[test]
fn forms_embed_fonts_used_only_inside_forms() {
    let mut e = engine(
        r"\font\formfont=cmr10
\setbox0=\hbox{\formfont Form text}
\pdfxform0
\shipout\hbox{\pdfrefxform\pdflastxform}
\end",
    );
    assert!(e.pdf_doc.pages[0].fonts.is_empty());
    assert_eq!(e.pdf_doc.form_fonts.len(), 1);
    assert_eq!(e.pdf_doc.form_fonts[0].1.len(), 1);
    e.embed_used_fonts().unwrap();
    assert_eq!(e.pdf_doc.fonts.len(), 1);
    let pdf = tex_core::pdffile::write_pdf(&e.pdf_doc).expect("valid embedded fonts");
    let parsed = lopdf::Document::load_mem(&pdf).unwrap();
    let form = parsed
        .objects
        .values()
        .find_map(|o| {
            let stream = o.as_stream().ok()?;
            (stream.dict.get(b"Subtype").ok()?.as_name().ok()? == b"Form").then_some(stream)
        })
        .expect("form stream");
    let resources = form.dict.get(b"Resources").unwrap().as_dict().unwrap();
    let fonts = parsed
        .dereference(resources.get(b"Font").unwrap())
        .unwrap()
        .1
        .as_dict()
        .unwrap();
    let font = parsed
        .dereference(fonts.get(b"F1").unwrap())
        .unwrap()
        .1
        .as_dict()
        .unwrap();
    assert_eq!(font.get(b"Subtype").unwrap().as_name().unwrap(), b"Type1");
    let descriptor = parsed
        .dereference(font.get(b"FontDescriptor").unwrap())
        .unwrap()
        .1
        .as_dict()
        .unwrap();
    assert!(
        descriptor.get(b"FontFile").is_ok(),
        "form font must be embedded"
    );
}

#[test]
fn undefined_pdf_xobject_references_are_located_and_omitted() {
    use tex_core::engine::InteractionMode;

    let reference_line = r"\shipout\hbox{\ten A\pdfrefximage 91\pdfrefxform 92}";
    let source = format!("\\font\\ten=cmr10\n{reference_line}\n\\end");
    let mut e = Engine::new(false);
    e.init_primitives();
    e.add_nullfont();
    e.set_interaction_mode(InteractionMode::Nonstop);
    e.input
        .push_file("undefined-pdf-ref.tex".to_string(), source.into_bytes());

    e.run();

    assert_eq!(e.error_count, 2, "{}", e.diagnostic_output);
    assert_eq!(e.diagnostics.len(), 2, "{}", e.diagnostic_output);
    assert_eq!(
        e.diagnostics[0].message,
        "Undefined PDF image object 91 in \\pdfrefximage; reference omitted"
    );
    assert_eq!(
        e.diagnostics[1].message,
        "Undefined PDF form object 92 in \\pdfrefxform; reference omitted"
    );
    let image_source = e.diagnostics[0].primary.as_ref().unwrap();
    assert_eq!(
        (
            image_source.name.as_str(),
            image_source.line,
            image_source.column
        ),
        (
            "undefined-pdf-ref.tex",
            2,
            reference_line.find("91").unwrap() + 1
        )
    );
    let form_source = e.diagnostics[1].primary.as_ref().unwrap();
    assert_eq!(
        (
            form_source.name.as_str(),
            form_source.line,
            form_source.column
        ),
        (
            "undefined-pdf-ref.tex",
            2,
            reference_line.find("92").unwrap() + 1
        )
    );
    assert!(e.diagnostics[0]
        .help
        .as_deref()
        .is_some_and(|help| help.contains("\\pdfximage")));
    assert!(e.diagnostics[1]
        .help
        .as_deref()
        .is_some_and(|help| help.contains("\\pdfxform")));

    assert_eq!(e.pdf_doc.pages.len(), 1);
    let page = String::from_utf8_lossy(&e.pdf_doc.pages[0].content);
    assert!(!page.contains("/Im91 Do"), "{page}");
    assert!(!page.contains("/Fm92 Do"), "{page}");
    let pdf = tex_core::pdffile::write_pdf(&e.pdf_doc).expect("valid embedded fonts");
    lopdf::Document::load_mem(&pdf).expect("invalid references must not break the PDF");
}

#[test]
fn input_rereads_a_file_rewritten_by_tex() {
    let path = std::env::temp_dir().join(format!("tex-reread-{}.tex", std::process::id()));
    std::fs::write(&path, b"\\def\\value{old}\n").unwrap();
    let path_text = path.to_string_lossy().replace('\\', "/");
    let e = engine(&format!(
        r"\input {path_text}\relax
\immediate\openout0={path_text}\relax
\immediate\write0{{\string\def\string\value{{new}}}}
\immediate\closeout0
\input {path_text}\relax
\message{{RESULT=\value}}
\end"
    ));
    assert!(e.term.contains("RESULT=new"), "{}", e.term);
    std::fs::remove_file(path).unwrap();
}
