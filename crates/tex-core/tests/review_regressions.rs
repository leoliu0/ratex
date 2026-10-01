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
