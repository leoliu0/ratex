//! pdfTeX/e-TeX engine primitives; expected values come from
//! `pdftex -ini -etex` (TeX Live 2026) on the same input.

use tex_core::engine::Engine;

fn run(src: &str) -> Engine {
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.input.push_file("t.tex".into(), src.as_bytes().to_vec());
    engine.run();
    engine
}

#[test]
fn parshape_queries_and_glue_conversions_match_pdftex() {
    let e = run(concat!(
        "\\catcode`\\{=1 \\catcode`\\}=2 \\parshape 3 1pt 2pt 3pt 4pt 5pt 6pt\n",
        "\\message{[\\the\\parshapelength1,\\the\\parshapeindent2,\\the\\parshapedimen1,",
        "\\the\\parshapedimen4,\\the\\parshapedimen5,\\the\\parshapelength9,",
        "\\the\\parshapeindent0,\\the\\parshapedimen-1]}\n",
        "\\skip0=1pt plus 2fil minus 3pt \\muskip0=4mu plus 5fill\n",
        "\\message{[\\the\\gluetomu\\skip0|\\the\\mutoglue\\muskip0|",
        "\\the\\numexpr\\mutoglue\\muskip0\\relax]}\n",
        "\\message{[\\the\\pdfretval|\\the\\pdfeachlineheight|\\the\\pdfignoreddimen|",
        "\\the\\lastlinefit|\\pdfinsertht0]}\\end\n"
    ));
    assert!(e.term.contains("[2.0pt,3.0pt,1.0pt,4.0pt,5.0pt,6.0pt,0.0pt,0.0pt]"), "{}", e.term);
    assert!(e.term.contains("[1.0mu plus 2.0fil minus 3.0mu|4.0pt plus 5.0fill|262144]"), "{}", e.term);
    assert!(e.term.contains("[0|-1000.0pt|-1000.0pt|0|0pt]"), "{}", e.term);
}

#[test]
fn pdfprimitive_reaches_the_initex_meaning_like_pdftex() {
    let e = run(concat!(
        "\\catcode`\\{=1 \\catcode`\\}=2 \\def\\relax{XX}\n",
        "\\edef\\x{\\pdfprimitive\\relax\\pdfprimitive\\number 5\\pdfprimitive\\foo A}\n",
        "\\message{[\\meaning\\x]}\\count3=7 \\count2=\\pdfprimitive\\count3 \n",
        "\\message{[\\the\\count2|\\the\\pdfprimitive\\count3]}\n",
        "\\let\\c\\count\\message{[\\ifpdfprimitive\\relax Y\\else N\\fi",
        "\\ifpdfprimitive\\count Y\\else N\\fi\\ifpdfprimitive\\c Y\\else N\\fi]}\\end\n"
    ));
    assert!(e.term.contains("[macro:->\\pdfprimitive XX5A]"), "{}", e.term);
    assert!(e.term.contains("[7|7]"), "{}", e.term);
    assert!(e.term.contains("[NYN]"), "{}", e.term);
}
