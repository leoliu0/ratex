//! Regression test: PDF driver primitives — pdfTeX dimension parameters
//! (assignment/\ifdim/\the/\divide), \pdfcolorstack action keywords, and
//! object-number tracking (\pdfobj/\pdfximage/\pdfxform + \pdflast*).
use tex_core::engine::Engine;

const SRC: &str = r#"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\$=3 \catcode`\^=7 \catcode`\_=11 \catcode`\@=11
\immediate\openout15=smoke.out
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
\immediate\closeout15
\end
"#;

#[test]
fn pdf_driver_primitives_smoke() {
    let dir = "/tmp/pdf_smoke";
    std::fs::create_dir_all(dir).unwrap();
    std::env::set_current_dir(dir).ok();
    let _ = std::fs::remove_file("smoke.out");
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.input
        .push_file("smoke.tex".to_string(), SRC.as_bytes().to_vec());
    e.run();
    let out = std::fs::read_to_string("smoke.out").expect("smoke.out produced");
    let norm: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    for expect in [
        "PW-OK",
        "DIV-OK",
        "IFDIM-OK",
        "CS-OK",
        "ORIGIN=72.26999pt",
        "LM=5.0pt",
        "LASTXFORM=7",
        "OCL=9",
        "OCLDEF=9",
    ] {
        assert!(norm.contains(expect), "missing {expect} in: {norm}\nTERM: {}", e.term);
    }
    assert!(!norm.contains("BAD"), "BAD marker in: {norm}");
}
