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

/// pdfTeX (writefont.c, tounicode.c) font dictionaries, checked against
/// `pdftex` output for the same input: descriptor metrics preset from the
/// TFM and overridden by the program's keys, /CharSet, no /Encoding for a
/// builtin-encoded font and the `\pdfglyphtounicode` CMap.
#[test]
fn type1_font_dictionaries_follow_pdftex() {
    let mut e = engine(
        r"\pdfgentounicode=1 \pdfglyphtounicode{A}{0041}\pdfglyphtounicode{B}{0042 0301}
\font\x=cmr10 \shipout\hbox{\x AB}
\end",
    );
    e.embed_used_fonts().unwrap();
    let pdf = tex_core::pdffile::write_pdf(&e.pdf_doc).unwrap();
    let parsed = lopdf::Document::load_mem(&pdf).unwrap();
    let font = parsed
        .objects
        .values()
        .find_map(|o| o.as_dict().ok().filter(|d| d.has_type(b"Font")))
        .expect("font dictionary");
    assert!(font.get(b"Encoding").is_err(), "builtin encoding stays implicit");
    let descriptor = parsed.dereference(font.get(b"FontDescriptor").unwrap()).unwrap().1;
    let descriptor = descriptor.as_dict().unwrap();
    let int = |key: &[u8]| descriptor.get(key).unwrap().as_i64().unwrap();
    assert_eq!(
        [b"Ascent".as_slice(), b"CapHeight", b"Descent", b"ItalicAngle", b"StemV", b"XHeight"].map(int),
        [694, 683, -194, 0, 69, 431]
    );
    let bbox: Vec<i64> = descriptor
        .get(b"FontBBox")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert_eq!(bbox, [-40, -250, 1009, 750]);
    assert_eq!(descriptor.get(b"CharSet").unwrap().as_str().unwrap(), b"/A/B");
    let cmap = parsed.dereference(font.get(b"ToUnicode").unwrap()).unwrap().1;
    let cmap = cmap.as_stream().unwrap().decompressed_content().unwrap();
    let cmap = String::from_utf8(cmap).unwrap();
    assert!(cmap.contains("/CMapName /TeX-cmr10-builtin-0 def"), "{cmap}");
    assert!(cmap.contains("2 beginbfchar\n<41> <0041>\n<42> <00420301>\nendbfchar"), "{cmap}");
}

#[test]
fn duplicate_destinations_warn_like_pdftex() {
    let source = r"\pdfdest name{a} fit\pdfdest name{a} fit
\shipout\hbox{A\pdfdest name{a} fit}\pdfdest name{a} fit
\end";
    // pdftex: once at the \pdfdest after the shipout, twice when \end
    // ships the two early ones
    let e = engine(source);
    assert_eq!(e.log.matches("has been already used, duplicate ignored").count(), 3, "{}", e.log);
    assert!(e.log.contains("destination with the same identifier (name{a})"));
    let e = engine(&format!("\\pdfsuppresswarningdupdest=1 {source}"));
    assert!(!e.log.contains("duplicate ignored"), "{}", e.log);
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
    let name = format!("tex-reread-{}.tex", std::process::id());
    let path = std::env::temp_dir().join(&name);
    std::fs::write(&path, b"\\def\\value{old}\n").unwrap();
    let path_text = path.to_string_lossy().replace('\\', "/");
    // openout_any=p refuses absolute output names: write through the
    // output directory instead, then read the same file back absolutely.
    let mut e = Engine::new(true);
    e.out_dir = std::env::temp_dir().to_string_lossy().into_owned();
    e.init_primitives();
    e.add_nullfont();
    let source = format!(
        r"\catcode`\{{=1 \catcode`\}}=2 \catcode`\#=6
\input {path_text}\relax
\immediate\openout0={name}\relax
\immediate\write0{{\string\def\string\value{{new}}}}
\immediate\closeout0
\input {path_text}\relax
\message{{RESULT=\value}}
\end"
    );
    e.input.push_file("review.tex".into(), source.into_bytes());
    e.run();
    assert_eq!(e.error_count, 0, "{}", e.term);
    assert!(e.term.contains("RESULT=new"), "{}", e.term);
    std::fs::remove_file(path).unwrap();
}

fn run_lenient(source: &str) -> Engine {
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.set_interaction_mode(tex_core::engine::InteractionMode::Nonstop);
    let source = format!(
        "\\catcode`\\{{=1 \\catcode`\\}}=2 \\catcode`\\#=6 \\catcode`\\^=7\n{source}"
    );
    engine
        .input
        .push_file("review.tex".into(), source.into_bytes());
    engine.run();
    engine
}

/// pdflatex: `[x\relax (EOL)y\relax (EOL)z]` — the end-of-line character is
/// processed after a control word (skip-blanks state).
#[test]
fn end_of_line_character_follows_control_words_like_tex() {
    let e = engine(
        "\\catcode`\\^^M=13 \\def^^M{(EOL)}\\message{[x\\relax\ny\\relax  \nz]}%\n\\catcode`\\^^M=5 \\end",
    );
    assert!(e.term.contains("[x\\relax (EOL)y\\relax (EOL)z]"), "{}", e.term);
}

/// pdflatex: the end-of-line character is appended when a line is read, so
/// changing \endlinechar affects only later lines (`a b`, then `cd`).
#[test]
fn endlinechar_changes_take_effect_on_the_next_line() {
    let e = engine(
        "\\endlinechar=-1 \\edef\\y{a\nb}\\message{[\\meaning\\y]}\n\\endlinechar=13 \\edef\\y{c\nd}\\message{[\\meaning\\y]}%\n\\end",
    );
    assert!(e.term.contains("[macro:->a b]"), "{}", e.term);
    assert!(e.term.contains("[macro:->cd]"), "{}", e.term);
}

/// pdflatex: `^^` notation inside a control word is reduced first.
#[test]
fn sup_notation_inside_control_words() {
    let e = engine("\\def\\foo{OK}\\message{[\\fo^^6f]}\\end");
    assert!(e.term.contains("[OK]"), "{}", e.term);
}

/// pdflatex: a forbidden \par aborts the macro call and is read again; a
/// prefix mismatch consumes the mismatching token.
#[test]
fn malformed_macro_calls_abort_like_tex() {
    let e = run_lenient(
        r"\def\a#1{[#1]}\def\b#1.{[#1]}\def\p.{X}
\message{1:\a\par Y}
\message{2:\b a\par b.Y}
\message{3:[\p,b]}
\end",
    );
    assert!(e.term.contains("1:\\par Y"), "{}", e.term);
    assert!(e.term.contains("2:\\par b.Y"), "{}", e.term);
    assert!(e.term.contains("3:[b]"), "{}", e.term);
    assert_eq!(e.error_count, 3, "{}", e.term);
}

/// pdflatex: an \outer macro may not appear in an argument or in skipped
/// conditional text; the call is aborted and the macro is read again, here
/// inside the \message text, which it ends too.
#[test]
fn outer_macros_end_arguments_and_skipped_text() {
    let e = run_lenient(
        r"\def\c#1{[#1]}\outer\def\o{O}
\message{1:\c\o}
\iffalse \o \fi
\end",
    );
    assert!(e.term.contains("1: "), "{}", e.term);
    let messages: Vec<&str> = e.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(
        messages.contains(&"Forbidden control sequence found while scanning use of \\c"),
        "{messages:?}"
    );
    assert!(
        messages.contains(&"Forbidden control sequence found while scanning text of \\message"),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.starts_with("Incomplete \\iffalse")),
        "{messages:?}"
    );
}

/// pdflatex: definitions, general text (\message, \toks, \write) and
/// expanded text report an \outer macro reached directly, through an active
/// character or through expansion; TeX reads a space in its place, inserts
/// `}` and reads the macro again afterwards. A balanced macro argument taken
/// from a token list is checked as well.
#[test]
fn outer_macros_end_definitions_and_general_text() {
    let e = run_lenient(
        r"\outer\def\o{\message{[O]}}
\edef\b{\noexpand\o}
\edef\c{x\b y}\message{C=[\meaning\c]}
\message{1:\b z}
\toks0{a\o b}\message{T=[\the\toks0]}
\immediate\write16{2:\b w}
\message{3:\expanded{p\b q}}
\catcode`\~=13 \outer\def~{\message{[T]}}
\message{4:x~y}
\def\c#1{[#1]}\edef\x{\noexpand\c{\noexpand\o}}
\message{5:\x}
\end",
    );
    let forbidden: Vec<&str> = e
        .diagnostics
        .iter()
        .filter_map(|d| d.message.strip_prefix("Forbidden control sequence found while scanning "))
        .collect();
    assert_eq!(
        forbidden,
        [
            "definition of \\c",
            "text of \\message",
            "text of \\toks",
            "text of \\write",
            "text of \\expanded",
            "text of \\message",
            "text of \\message",
            "use of \\c",
            "text of \\message",
        ],
        "{}",
        e.term
    );
    assert!(e.term.contains("C=[macro:->x ]"), "{}", e.term);
    assert!(e.term.contains("T=[a ]"), "{}", e.term);
    assert!(e.term.contains("3:p  "), "{}", e.term);
    assert!(e.term.contains("4:x [T]"), "{}", e.term);
}

/// pdflatex: an \outer macro ends an alignment preamble (`\cr}` inserted)
/// and runs after the alignment.
#[test]
fn outer_macro_ends_alignment_preamble() {
    let e = run_lenient(
        r"\outer\def\o{\message{[O]}}
\halign{#\o\cr}
\end",
    );
    assert!(
        e.diagnostics.iter().any(|d| d.message
            == "Forbidden control sequence found while scanning preamble of \\halign"),
        "{}",
        e.term
    );
    assert!(e.term.contains("[O]"), "{}", e.term);
}

/// pdflatex: inside \csname, a \noexpand-marked token means \relax, which
/// ends the name with "Missing \endcsname inserted"; it is read again.
#[test]
fn noexpand_marked_token_ends_csname() {
    let e = run_lenient(
        r"\message{7:\csname a\noexpand\expanded{b}\endcsname}
\end",
    );
    assert!(
        e.diagnostics.iter().any(|d| d.message == "Missing \\endcsname inserted"),
        "{}",
        e.term
    );
    assert!(e.term.contains("7:\\a b\\endcsname"), "{}", e.term);
}

/// pdflatex: an undefined l3-style `\exp_args:N...` name is an undefined
/// control sequence like any other; nothing is synthesized for it.
#[test]
fn undefined_exp_args_names_are_undefined() {
    let e = run_lenient(
        r"\catcode`\:=11 \catcode`\_=11
\def\::N{}\def\:::{}
\exp_args:NN \message{[\meaning\exp_args:NN]}
\end",
    );
    assert!(
        e.diagnostics
            .iter()
            .any(|d| d.message == "Undefined control sequence \\exp_args:NN"),
        "{}",
        e.term
    );
    assert!(e.term.contains("[undefined]"), "{}", e.term);
}

/// pdflatex (tex.web print_cs/print with cp227.tcx): control bytes in
/// control-sequence names and printed text use `^^` notation.
#[test]
fn control_bytes_print_in_caret_notation() {
    let e = run_lenient(
        r"\def\y{\^^A ^^Bb}\show\y
\message{8:[\string\^^A][^^A]}
\^^A
\end",
    );
    let messages: Vec<&str> = e.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(
        messages.contains(&"Undefined control sequence \\^^A"),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("\\y = macro: -> \\^^A ^^Bb")),
        "{messages:?}"
    );
    assert!(e.term.contains("8:[\\^^A][^^A]"), "{}", e.term);
}

/// pdflatex: \write text is expanded as `{text}\endwrite`, so an argument
/// scan stops at the closing brace instead of running past the text.
#[test]
fn write_text_is_braced_for_argument_scanning() {
    let e = run_lenient(
        r"\def\b#1.{[#1]}
\immediate\write16{[W:X\b a]}
\message{AFTER}
\end",
    );
    let messages: Vec<&str> = e.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(messages.contains(&"Argument of \\b has an extra }"), "{messages:?}");
    assert!(messages.contains(&"Paragraph ended before \\b was complete"), "{messages:?}");
    assert!(e.term.contains("AFTER"), "{}", e.term);
}

/// pdflatex: only the most recently loaded font may gain \fontdimen
/// parameters (tex.web §579).
#[test]
fn only_the_last_font_gains_fontdimen_parameters() {
    let e = run_lenient(
        r"\font\fa=cmr10 \font\fb=cmr10 at 11pt
\fontdimen20\fa=1pt
\fontdimen20\fb=2pt \message{[\the\fontdimen20\fb]}
\end",
    );
    assert!(
        e.diagnostics
            .iter()
            .any(|d| d.message == "Font \\fa has only 7 fontdimen parameters"),
        "{}",
        e.term
    );
    assert!(e.term.contains("[2.0pt]"), "{}", e.term);
}

/// pdflatex: \noexpand marks an undefined control sequence too, so it is
/// stored unexpanded (LaTeX's `\@nil` delimiter in \@onefilewithoptions)
/// and means \relax in main control.
#[test]
fn noexpand_protects_undefined_control_sequences() {
    let e = engine(
        r"\def\ext{sty}
\edef\c{\def\noexpand\c##1\detokenize\expandafter{\expanded{.\ext}}\noexpand\nil{[##1]}}\c
\message{\expandafter\c\detokenize{xcolor.sty}\nil}
\noexpand\undefined
\end",
    );
    assert!(e.term.contains("[xcolor]"), "{}", e.term);
}

/// pdflatex: expanding an undefined control sequence is an error wherever
/// it happens (tex.web §370); TeX drops the token and reads on.
#[test]
fn undefined_control_sequences_are_errors_wherever_expanded() {
    let e = run_lenient(
        r"\expandafter\relax\uC
\count255=\uD 5 \message{D:\the\count255}
\message{E:\uE}
\edef\f{\expandafter\string\csname x\uF y\endcsname}\message{F:\f}
\edef\g{G:\uG}\message{\meaning\g}
\message{I:\number\uI 7}
\if\uJ\relax\relax\message{J:T}\else\message{J:F}\fi
\dimen0=1\uL pt \message{L:\the\dimen0}
\message{M:\romannumeral\uM 5 }
\noexpand\uB
\end",
    );
    let messages: Vec<&str> = e.diagnostics.iter().map(|d| d.message.as_str()).collect();
    let expected: Vec<String> = ["C", "D", "E", "F", "G", "I", "J", "L", "M"]
        .iter()
        .map(|name| format!("Undefined control sequence \\u{name}"))
        .collect();
    assert_eq!(messages, expected, "{}", e.term);
    for text in ["D:5", "E:", "F:\\xy", "macro:->G:", "I:7", "J:T", "L:1.0pt", "M:v"] {
        assert!(e.term.contains(text), "{text}: {}", e.term);
    }
}

/// pdflatex: \font defines its identifier as \nullfont before scanning the
/// file name and size, which may expand it; a font that cannot be loaded
/// leaves it \nullfont.
#[test]
fn font_identifier_is_nullfont_while_its_size_is_scanned() {
    let e = run_lenient(
        r"\font\x=cmr10 \x \message{A:\fontname\font}
\font\y=nonexistentfontzz \message{B:\fontname\y}
\end",
    );
    assert!(e.term.contains("A:cmr10"), "{}", e.term);
    assert!(e.term.contains("B:nullfont"), "{}", e.term);
    assert_eq!(e.error_count, 1, "{}", e.term);
}

/// pdflatex: active characters are checked for \outer meanings in macro
/// arguments and skipped conditional text, and an active character \let to
/// \fi ends skipped text.
#[test]
fn outer_active_characters_and_active_fi_in_skipped_text() {
    let e = run_lenient(
        r"\catcode`\~=13 \catcode`\!=13
\outer\def~{\message{T}}
\def\a#1{\message{[#1]}}
\message{1:}\a~
\def\b#1.{\message{[#1]}}
\message{2:}\b x~.
\let!=\fi
\iffalse x ! \message{3:after}
\iffalse ~ \fi
\end",
    );
    let messages: Vec<&str> = e.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "Forbidden control sequence found while scanning use of \\a",
            "Forbidden control sequence found while scanning use of \\b",
            "Incomplete \\iffalse; all text was ignored after line 10",
            "Extra \\fi",
        ],
        "{}",
        e.term
    );
    assert!(e.term.contains("3:after"), "{}", e.term);
}

/// pdftex -ini (tex.web §1370 write_out sets `mode:=0`): inside a `\write`
/// text \prevgraf reads 0 (`[PG 0/3]`: the same \vbox counted 3 lines),
/// \lastkern/\lastpenalty/\lastskip read 0, \lastnodetype reads -1, no mode
/// conditional holds, and \prevdepth/\spacefactor are "Improper" (§418)
/// with `\the` printing `0`. \message keeps the real mode (`[KM 3.0pt]`).
#[test]
fn write_texts_expand_in_mode_zero() {
    let e = run_lenient(
        r"\font\tenrm=cmr10 \tenrm
\hsize=1pt \parfillskip=0pt plus 1fil \tolerance=10000 \pretolerance=-1
\def\w{\immediate\write16}
\setbox1\vbox{a b c\par \count255=\prevgraf \w{[PG \the\prevgraf/\the\count255]}}
\setbox1\vbox{\kern3pt \w{[K \the\lastkern]}\message{[KM \the\lastkern]}%
\penalty7 \w{[P \the\lastpenalty]}\vskip2pt \w{[S \the\lastskip]}%
\w{[T \the\lastnodetype]}\w{[I \ifvmode V\fi\ifhmode H\fi\ifmmode M\fi\ifinner I\fi.]}%
\w{[D \the\prevdepth]}}
\setbox1\hbox{x\w{[F \the\spacefactor]}\w{[J \ifvmode V\fi\ifhmode H\fi\ifinner I\fi.]}}
\end",
    );
    for expected in [
        "[PG 0/3]", "[K 0.0pt]", "[KM 3.0pt]", "[P 0]", "[S 0.0pt]", "[T -1]", "[I .]", "[D 0]",
        "[F 0]", "[J .]",
    ] {
        assert!(e.term.contains(expected), "{expected}: {}", e.term);
    }
    let improper: Vec<&str> = e
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .filter(|message| message.starts_with("Improper"))
        .collect();
    assert_eq!(improper, ["Improper \\prevdepth", "Improper \\spacefactor"], "{}", e.term);
    assert_eq!(e.error_count, 2, "{}", e.term);
}

/// pdftex -ini (tex.web §418): \spacefactor is fetched only in horizontal
/// and \prevdepth only in vertical mode; elsewhere TeX reports "Improper"
/// and uses 0, which `\the` prints as `0` even for \prevdepth.
#[test]
fn space_factor_and_prev_depth_are_improper_outside_their_modes() {
    let e = run_lenient(
        r"\catcode`\$=3 \font\tenrm=cmr10 \tenrm
\textfont0=\tenrm \scriptfont0=\tenrm \scriptscriptfont0=\tenrm
\message{[VS \the\spacefactor][VD \the\prevdepth]}
\setbox1\hbox{\message{[HS \the\spacefactor][HD \the\prevdepth]}}
\setbox1\hbox{$\count1=\spacefactor \dimen1=\prevdepth \message{[M \the\count1/\the\dimen1]}$}
\end",
    );
    for expected in ["[VS 0][VD -1000.0pt]", "[HS 1000][HD 0]", "[M 0/0.0pt]"] {
        assert!(e.term.contains(expected), "{expected}: {}", e.term);
    }
    let improper: Vec<&str> = e
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .filter(|message| message.starts_with("Improper"))
        .collect();
    assert_eq!(
        improper,
        [
            "Improper \\spacefactor",
            "Improper \\prevdepth",
            "Improper \\spacefactor",
            "Improper \\prevdepth"
        ],
        "{}",
        e.term
    );
}

/// pdftex -ini: web2c ends a file name at the end-of-line space even inside
/// an unterminated quote, so `\font\x="cmr10` loads cmr10.
#[test]
fn quoted_font_names_end_at_the_end_of_the_line() {
    let e = run_lenient("\\font\\x=\"cmr10\n\\message{[\\fontname\\x]}\\end");
    assert!(e.term.contains("[cmr10]"), "{}", e.term);
    assert_eq!(e.error_count, 0, "{}", e.term);
}

/// tex.web §107 `xn_over_d` truncates: space factor 1250 turns cmr10's
/// 109226sp interword stretch into 136532sp, not 136533sp. The 1sp shifts
/// the glue on this line enough to flip pdfTeX's TJ rounding; the expected
/// array is `pdftex -ini` output for the same input.
#[test]
fn space_factor_glue_truncates_like_pdftex() {
    let e = engine(
        r"\pdfoutput=1 \hoffset=-1in \sfcode`\,=1250 \font\tenrm=cmr10
\setbox0\hbox{\tenrm x, \global\skip1=\lastskip}\message{[\the\skip1]}
\shipout\hbox to 2031622sp{\tenrm x, y z}
\end",
    );
    assert!(e.term.contains("[3.33333pt plus 2.08331pt minus 0.88889pt]"), "{}", e.term);
    let page = String::from_utf8_lossy(&e.pdf_doc.pages[0].content);
    assert!(page.contains("[(x,)-697(y)-625(z)]TJ"), "{page}");
}

/// `\font ... scaled` sizes the font with the same truncating `xn_over_d`
/// (tex.web §1258): cmr10 scaled 2074 is 1359216sp, identical to `at
/// 1359216sp`, as in pdftex.
#[test]
fn font_scaled_size_truncates_like_pdftex() {
    let e = engine(
        r"\font\big=cmr10 scaled 2074 \font\bigb=cmr10 at 1359216sp
\message{[\fontname\big][\ifx\big\bigb same\else diff\fi]}
\end",
    );
    assert!(e.term.contains("[cmr10 at 20.73999pt][same]"), "{}", e.term);
}

/// pdftex -ini output for this page: `\pdfsetmatrix` echoes its plain
/// numbers verbatim, and `pdf_print_char` writes `(`, `)`, space and `\` as
/// octal escapes while DEL stays raw.
#[test]
fn setmatrix_and_string_bytes_print_like_pdftex() {
    let e = engine(
        r"\pdfoutput=1 \font\tenrm=cmr10
\shipout\hbox{\pdfsave\pdfsetmatrix{.5 0 0 -.25}\tenrm(a)\char32\char127\char92\pdfrestore}
\end",
    );
    let page = String::from_utf8_lossy(&e.pdf_doc.pages[0].content);
    assert!(page.contains("\n.5 0 0 -.25 0 0 cm\n"), "{page}");
    assert!(page.contains("[(\\050a\\051\\040\x7f\\134)]TJ"), "{page}");
}

/// pdftex.web `pdf_set_rule` centers a hairline at `y - (h + 1)/2` with
/// Pascal real division, truncated when passed on as scaled: an even 0.4pt
/// rule sits 13108sp above its bottom edge. `pdftex -ini` prints 25.907
/// here; integer halving gives 25.906.
#[test]
fn hairline_rule_center_truncates_like_pdftex() {
    let e = engine(
        r"\pdfoutput=1 \pdfpagewidth=100pt \pdfpageheight=100pt
\shipout\vbox{\kern100031sp\hrule height .4pt width 10pt}
\end",
    );
    let page = String::from_utf8_lossy(&e.pdf_doc.pages[0].content);
    assert!(page.contains("q\n1 0 0 1 72 25.907 cm\n[]0 d 0 J 0.398 w"), "{page}");
}

/// Fonts and the IniTeX parameter values the TeXXeT expectations below were
/// taken under (`pdftex -ini -etex`, TeX Live 2026).
const TEXXET_SETUP: &str = r"\catcode`\&=4
\font\tenrm=cmr10 \font\teni=cmmi10 \font\tensy=cmsy10 \font\tenex=cmex10
\textfont0=\tenrm \scriptfont0=\tenrm \scriptscriptfont0=\tenrm
\textfont1=\teni \scriptfont1=\teni \scriptscriptfont1=\teni
\textfont2=\tensy \scriptfont2=\tensy \scriptscriptfont2=\tensy
\textfont3=\tenex \scriptfont3=\tenex \scriptscriptfont3=\tenex
\tenrm \thinmuskip=0mu \medmuskip=0mu \thickmuskip=0mu
\hsize=200pt \parindent=0pt \parfillskip=0pt plus 1fil \tolerance=10000
\TeXXeTstate=1
";

/// Every `open ... close` span of `text`, in order.
fn spans<'a>(text: &'a str, open: &str, close: char) -> Vec<&'a str> {
    text.match_indices(open)
        .map(|(at, _)| {
            let rest = &text[at..];
            &rest[..=rest.find(close).unwrap()]
        })
        .collect()
}

#[test]
fn texxet_box_widths_and_lr_problems_match_etex() {
    let e = engine(&format!(
        r"{TEXXET_SETUP}
\setbox0\hbox{{ab\beginR cd\endR ef}}\message{{[w0=\the\wd0]}}
\setbox3\hbox spread 10pt{{AV\beginR AV fi ffl\hskip 3pt plus 1pt\endR VA}}\message{{[w3=\the\wd3]}}
\setbox2\hbox{{\beginR a$x+y$b\endR}}\message{{[w2=\the\wd2]}}
\setbox4\hbox{{a\endR b}}\setbox4\hbox{{\beginR ab}}\setbox4\hbox{{\beginL a\endR b\endL}}
\setbox4\hbox{{\beginR\beginL a}}\message{{[w4=\the\wd4]}}\setbox4\hbox{{a\endL\endR\endR b}}
\setbox5\hbox{{\beginR aaa bbb ccc\endR}}
\setbox6\vbox{{\TeXXeTstate=0 \hsize=40pt \unhcopy5\par}}
\shipout\copy6
\end"
    ));
    for expected in [
        "[w0=28.05562pt]",
        "[w3=75.22229pt]",
        "[w2=29.31026pt]",
        "[w4=5.00002pt]",
    ] {
        assert!(e.term.contains(expected), "{expected}: {}", e.term);
    }
    // The last report comes from ship_out: the lines were broken with
    // TeXXeT disabled, so they carry no LR boundary nodes.
    assert_eq!(
        spans(&e.log, "\\endL or \\endR problem (", ')'),
        [
            "\\endL or \\endR problem (0 missing, 1 extra)",
            "\\endL or \\endR problem (1 missing, 0 extra)",
            "\\endL or \\endR problem (0 missing, 1 extra)",
            "\\endL or \\endR problem (2 missing, 0 extra)",
            "\\endL or \\endR problem (0 missing, 3 extra)",
            "\\endL or \\endR problem (1 missing, 1 extra)",
        ],
        "{}",
        e.log
    );
}

#[test]
fn texxet_predisplay_direction_and_size_match_etex() {
    let e = engine(&format!(
        r"{TEXXET_SETUP}
\everydisplay{{\message{{[\the\predisplaydirection/\the\predisplaysize]}}}}
\setbox0\vbox{{\beginR aaa bbb ccc $$x+y\eqno(1)$$ ddd eee\endR\par}}
\setbox1\vbox{{\leftskip=10pt \rightskip=5pt \beginR aaa bbb $$x=y\leqno(2)$$ ddd eee\endR\par}}
\setbox2\vbox{{aaa \beginR bbb\endR\ ccc $$z$$ ddd\par}}
\setbox4\vbox{{aaa\beginL bbb $$w\eqno(3)$$ ccc\endL\par}}
\end"
    ));
    assert_eq!(
        spans(&e.term, "[", ']'),
        [
            "[-1/16383.99998pt]",
            "[-1/16383.99998pt]",
            "[0/71.66678pt]",
            "[1/51.6668pt]",
        ],
        "{}",
        e.term
    );
}

/// tex.web resume_after_display ends with <Scan an optional space>, after
/// unsave has put the display group's \aftergroup tokens back.
#[test]
fn text_after_a_display_skips_one_optional_space() {
    let e = engine(&format!(
        r"{TEXXET_SETUP}
\def\sp{{ }}
\setbox0\vbox{{aaa $$ $$ \message{{[\the\lastnodetype]}}ddd\par}}
\setbox0\vbox{{aaa $$\aftergroup\sp $$\message{{[\the\lastnodetype]}}ddd\par}}
\setbox0\vbox{{aaa $$ $$x\message{{[\the\lastnodetype]}}\par}}
\setbox0\vbox{{aaa $$\halign{{#\cr a\cr}}$$ \message{{[\the\lastnodetype]}}ddd\par}}
\end"
    ));
    assert_eq!(spans(&e.term, "[", ']'), ["[-1]", "[-1]", "[0]", "[-1]"], "{}", e.term);
}

/// etex.ch §800: rows of an alignment in a display are dlist boxes, so
/// ship_out sets them left to right even inside right-to-left text.
#[test]
fn display_alignment_rows_keep_left_to_right_order_inside_r_text() {
    let e = engine(&format!(
        r"{TEXXET_SETUP}
\pdfcompresslevel=0
\shipout\hbox{{\beginR x\vbox{{\hsize=120pt aa $$\halign{{#\hfil&\hskip10pt#\cr ab&cd\cr}}$$ bb\par}}y\endR}}
\end"
    ));
    let content = String::from_utf8_lossy(&e.pdf_doc.pages[0].content);
    assert!(content.contains("[(ab)-1000(cd)]TJ"), "{content}");
}

/// tex.web show_box: a nonpositive \showboxbreadth shows five items per
/// level (IniTeX starts with \showboxbreadth=0).
#[test]
fn nonpositive_showboxbreadth_shows_five_items() {
    let e = run_lenient(
        r"\font\tenrm=cmr10 \tenrm \showboxbreadth=0 \showboxdepth=1
\setbox0\hbox{aaaaaaa}\showbox0
\end",
    );
    assert_eq!(e.log.matches("\n.\\tenrm a").count(), 5, "{}", e.log);
    assert!(e.log.contains("\n.\\tenrm a\n.etc.\n"), "{}", e.log);
}

/// pdftex -ini: \delimiterfactor=0 is used as given, so with a large
/// \delimitershortfall the smallest parenthesis (8.1778pt wide pair) is
/// chosen.
#[test]
fn zero_delimiterfactor_is_not_replaced() {
    let e = engine(&format!(
        r#"{TEXXET_SETUP}
\delcode`\(="028300 \delcode`\)="029301
\delimiterfactor=0 \delimitershortfall=100pt
\setbox1\hbox{{$\left(\vrule height 20pt depth 10pt\right)$}}\message{{[\the\wd1]}}
\end"#
    ));
    assert!(e.term.contains("[8.1778pt]"), "{}", e.term);
}

/// pdftex (TeX Live 2026) after `\pdfsetrandomseed 12345`:
/// [12345][709][377][-201][0][-113033] [7][50][-27960] [timer]
/// [macro:->\pdfelapsedtime ]; a negative seed is made positive,
/// and `\pdfelapsedtime` is an unexpandable internal integer.
#[test]
fn pdf_random_deviates_follow_the_seeded_generator() {
    let e = engine(
        r"\pdfsetrandomseed 12345
\message{[\the\pdfrandomseed][\pdfuniformdeviate 1000][\pdfuniformdeviate 1000][\pdfuniformdeviate -1000][\pdfuniformdeviate 0][\pdfnormaldeviate]}
\pdfsetrandomseed -7 \message{[\the\pdfrandomseed][\pdfuniformdeviate 100][\pdfnormaldeviate]}
\pdfresettimer \ifnum\pdfelapsedtime<65536 \message{[timer]}\fi
\edef\x{\noexpand\pdfelapsedtime}\message{[\meaning\x]}
\end",
    );
    let term: String = e.term.split_whitespace().collect();
    assert!(
        term.contains("[12345][709][377][-201][0][-113033][7][50][-27960][timer][macro:->\\pdfelapsedtime]"),
        "{}",
        e.term
    );
}

/// pdftex -ini (tex.web §1237): \advance, \multiply and \divide accept only
/// registers and the integer, dimen, glue and muglue parameters; every other
/// next token (set_aux, set_prev_graf, set_page_dimen, last_item, ...) is
/// consumed with "You can't use `x' after \advance" and nothing changes.
#[test]
fn arithmetic_rejects_quantities_that_are_not_register_like() {
    let e = run_lenient(
        r"\countdef\cc=5 \cc=3
\advance\cc by 2 \message{[\the\cc]}
\hbox{\spacefactor=1000 \advance\spacefactor by 5 \message{[\the\spacefactor]}%
\multiply\spacefactor 2 \divide\spacefactor 2 \message{[\the\spacefactor]}}
\advance\prevgraf by 1 \message{[\the\prevgraf]}
\advance\prevdepth by 1pt
\advance\deadcycles 1
\advance\pagegoal 1pt
\advance\wd0 1pt
\advance\catcode`a 1
\advance 5 \message{after5}
\advance\relax\count1 by 3 \message{[\the\count1]}
\def\m{\count2 }\advance\m by 7 \message{[\the\count2]}
\advance\lastpenalty 1
\multiply\interactionmode 2
\divide\hyphenchar\nullfont 2
\global\advance\dimen3 by 1pt \message{[\the\dimen3]}
\end",
    );
    let messages: Vec<&str> = e.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "You can't use `\\spacefactor' after \\advance",
            "You can't use `\\spacefactor' after \\multiply",
            "You can't use `\\spacefactor' after \\divide",
            "You can't use `\\prevgraf' after \\advance",
            "You can't use `\\prevdepth' after \\advance",
            "You can't use `\\deadcycles' after \\advance",
            "You can't use `\\pagegoal' after \\advance",
            "You can't use `\\wd' after \\advance",
            "You can't use `\\catcode' after \\advance",
            "You can't use `the character 5' after \\advance",
            "You can't use `\\relax' after \\advance",
            "Missing number, treated as zero",
            "You can't use `\\lastpenalty' after \\advance",
            "You can't use `\\interactionmode' after \\multiply",
            "You can't use `\\hyphenchar' after \\divide",
        ],
        "{}",
        e.term
    );
    let term: String = e.term.split_whitespace().collect();
    assert!(
        term.contains("[5][1000][1000][0]after5[0][7][1.0pt]"),
        "{}",
        e.term
    );
}

/// pdftex -ini (tex.web §1195): a formula is deleted, and mlist_to_hlist
/// skipped, unless families 2 and 3 have at least 22 and 13 \fontdimen
/// parameters in all three sizes; a display is deleted the same way.
#[test]
fn formulas_without_enough_math_font_parameters_are_deleted() {
    let e = run_lenient(
        r"\catcode`\$=3 \font\tenrm=cmr10 \tenrm
\message{a}
$x$
\textfont2=\tenrm \scriptfont2=\tenrm \scriptscriptfont2=\tenrm
$x^2$
\font\tensy=cmsy10 \textfont2=\tensy \scriptfont2=\tensy \scriptscriptfont2=\tensy
$x$
$$x$$
\end",
    );
    let messages: Vec<&str> = e.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "Math formula deleted: Insufficient symbol fonts",
            "Math formula deleted: Insufficient symbol fonts",
            "Math formula deleted: Insufficient extension fonts",
            "Math formula deleted: Insufficient extension fonts",
        ],
        "{}",
        e.term
    );
}

/// pdftex -ini: \pdffilesize and its siblings look names up like
/// kpse_find_tex, along TEXINPUTS only. TFM, Type 1, encoding and map files
/// of the same TeX tree are not found (the primitives expand to nothing),
/// while a TeX input is, with or without its default extension.
#[test]
fn pdf_file_queries_search_the_tex_input_path_only() {
    let e = run_lenient(
        r"\def\empty{}\def\q#1{\edef\r{\pdffilesize{#1}}\message{[#1=\ifx\r\empty-\else+\fi]}}
\q{cmr10.tfm}\q{cmr10.pfb}\q{lm-ec.enc}\q{pdftex.map}\q{8r.enc}\q{cmr10}
\q{plain.tex}\q{plain}\q{article.cls}\q{latex.ltx}
\edef\r{\pdfmdfivesum file{cmr10.tfm}}\message{[md5=\r]}
\edef\r{\pdffilemoddate{cmr10.tfm}}\message{[date=\r]}
\edef\r{\pdffiledump length 4{cmr10.tfm}}\message{[dump=\r]}
\end",
    );
    let term: String = e.term.split_whitespace().collect();
    assert!(
        term.contains(
            "[cmr10.tfm=-][cmr10.pfb=-][lm-ec.enc=-][pdftex.map=-][8r.enc=-][cmr10=-]\
             [plain.tex=+][plain=+][article.cls=+][latex.ltx=+][md5=][date=][dump=]"
        ),
        "{}",
        e.term
    );
}
