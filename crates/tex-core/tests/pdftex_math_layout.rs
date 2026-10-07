//! pdfTeX math and paragraph layout regressions found by the 100-document
//! benchmark. Expected boxes are those `pdftex -ini -etex` (TeX Live 2026)
//! shows for the same input.

use tex_core::engine::Engine;

fn run(source: &str) -> Engine {
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.set_interaction_mode(tex_core::engine::InteractionMode::Nonstop);
    let source = format!(
        "\\catcode`\\{{=1 \\catcode`\\}}=2 \\catcode`\\#=6 \\catcode`\\^=7\n\\catcode`\\$=3 \\catcode`\\_=8\n{source}"
    );
    engine.input.push_file("math.tex".into(), source.into_bytes());
    engine.run();
    engine
}

const MATH_FONTS: &str = r"\font\tenrm=cmr10 \font\sevenrm=cmr7 \font\fiverm=cmr5
\font\teni=cmmi10 \font\seveni=cmmi7 \font\fivei=cmmi5
\font\tensy=cmsy10 \font\sevensy=cmsy7 \font\fivesy=cmsy5 \font\tenex=cmex10
\textfont0=\tenrm \scriptfont0=\sevenrm \scriptscriptfont0=\fiverm
\textfont1=\teni \scriptfont1=\seveni \scriptscriptfont1=\fivei
\textfont2=\tensy \scriptfont2=\sevensy \scriptscriptfont2=\fivesy
\textfont3=\tenex \scriptfont3=\tenex \scriptscriptfont3=\tenex
\showboxbreadth=100 \showboxdepth=100 \scriptspace=0.5pt
";

/// tex.web §734-735: make_over/make_under box the nucleus in the style the
/// noad is converted in (a fraction denominator is script style even though
/// the field was scanned in text style), and the bar is a running-width
/// `fraction_rule`, so it grows with the \scriptspace a script box gets.
#[test]
fn over_and_underline_follow_the_conversion_style() {
    let e = run(&format!(
        r"{MATH_FONTS}\setbox0\hbox{{${{1\over\overline r}}$}}\showbox0
\setbox0\hbox{{$x_{{\underline r}}$}}\showbox0
\end"
    ));
    // pdftex: the overlined r of the denominator is \seveni
    assert!(e.log.contains("\\vbox(5.01378+0.0)x3.92825\n"), "{}", e.log);
    assert!(!e.log.contains("\\teni r"), "{}", e.log);
    // pdftex: the underlined subscript is the vbox itself, widened by
    // \scriptspace, its rule running
    assert!(
        e.log.contains(
            "\\vbox(3.01389+1.9999)x4.42825, shifted 1.49998
..\\hbox(3.01389+0.0)x3.92825
...\\seveni r
..\\kern1.19994
..\\rule(0.39998+0.0)x*
"
        ),
        "{}",
        e.log
    );
}

/// tex.web §1199 reads \displaywidth, \displayindent and \predisplaysize
/// when the display ends, so assignments inside the formula (LaTeX
/// classes reset \displaywidth/\displayindent in \everydisplay) place it.
#[test]
fn display_geometry_is_read_when_the_display_ends() {
    let e = run(&format!(
        r"{MATH_FONTS}\hsize=300pt \parindent=0pt \parshape 1 20pt 200pt
\setbox0\vbox{{\noindent$$\displayindent=0pt \displaywidth=\hsize \hbox to 100pt{{}}$$\par}}\showbox0
\parshape 0 \abovedisplayskip=10pt \abovedisplayshortskip=3pt
\setbox0\vbox{{\noindent\vrule width 250pt$$\hbox to 100pt{{}}$$\par}}\showbox0
\setbox0\vbox{{\noindent\vrule width 250pt$$\predisplaysize=-16383.99998pt \hbox to 100pt{{}}$$\par}}\showbox0
\end"
    ));
    // pdftex: centred in \hsize, not in the \parshape line
    assert_eq!(e.log.matches("\\hbox(0.0+0.0)x100.0, shifted 100.0, display").count(), 3, "{}", e.log);
    assert_eq!(e.log.matches(".\\glue(\\abovedisplayskip) 10.0\n").count(), 1, "{}", e.log);
    assert_eq!(e.log.matches(".\\glue(\\abovedisplayshortskip) 3.0\n").count(), 1, "{}", e.log);
}

/// pdftex.web find_protchar_right/left: the margin searches descend into
/// hboxes and stop at the first node that is not cp_skipable (a rule, an
/// explicit kern), which then protrudes nothing.
#[test]
fn protrusion_searches_follow_pdftex() {
    let e = run(r"\font\tenrm=cmr10 \tenrm \showboxbreadth=100 \showboxdepth=100
\pdfprotrudechars=2 \rpcode\tenrm`)=500 \lpcode\tenrm`(=500
\hsize=100pt \parindent=0pt \parfillskip=0pt plus 1fil \tolerance=10000 \hbadness=10000
\setbox1\vbox{(a) \hbox{\vrule width 1pt}\par}\showbox1
\setbox1\vbox{\indent\hbox{(b)}\par}\showbox1
\setbox1\vbox{(c)\kern1pt\par}\showbox1
\end");
    assert!(
        e.log.contains(
            ".\\hbox(7.5+2.5)x100.0, glue set 87.88887fil
..\\kern-5.00002 (left margin)
..\\hbox(0.0+0.0)x0.0
..\\tenrm (
..\\tenrm a
..\\tenrm )
..\\glue 3.33333 plus 1.66666 minus 1.11111
..\\hbox(0.0+0.0)x1.0
...\\rule(*+*)x1.0
..\\penalty 10000
..\\glue(\\parfillskip) 0.0 plus 1.0fil
"
        ),
        "{}",
        e.log
    );
    assert!(
        e.log.contains(
            ".\\hbox(7.5+2.5)x100.0, glue set 96.66666fil
..\\kern-5.00002 (left margin)
..\\hbox(0.0+0.0)x0.0
..\\hbox(7.5+2.5)x13.33337
...\\tenrm (
...\\tenrm b
...\\tenrm )
..\\penalty 10000
..\\kern-5.00002 (right margin)
..\\glue(\\parfillskip) 0.0 plus 1.0fil
"
        ),
        "{}",
        e.log
    );
    assert!(
        e.log.contains(
            ".\\hbox(7.5+2.5)x100.0, glue set 91.77777fil
..\\kern-5.00002 (left margin)
..\\hbox(0.0+0.0)x0.0
..\\tenrm (
..\\tenrm c
..\\tenrm )
..\\kern 1.0
..\\penalty 10000
..\\glue(\\parfillskip) 0.0 plus 1.0fil
"
        ),
        "{}",
        e.log
    );
}
