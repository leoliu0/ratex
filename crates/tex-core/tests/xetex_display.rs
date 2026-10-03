//! XeTeX display math: `\predisplaysize` over native words and `rebox` of a
//! `\vtop` limit nucleus. Expectations come from TeX Live 2026
//! `xetex -ini -interaction=nonstopmode` on the same source (lmroman10 OTF and
//! cm TFM fonts from the TeX Live tree, which the embedded archive carries).

use tex_core::engine::{Engine, EngineKind, InteractionMode};

const SRC: &str = r#"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\$=3 \catcode`\^=7 \catcode`\_=8
\tracingonline1 \showboxbreadth100 \showboxdepth100
\font\a="[lmroman10-regular.otf]" \a
\font\tenrm=cmr10 \font\teni=cmmi10 \font\tensy=cmsy10 \font\tenex=cmex10
\textfont0=\tenrm \scriptfont0=\tenrm \scriptscriptfont0=\tenrm
\textfont1=\teni \scriptfont1=\teni \scriptscriptfont1=\teni
\textfont2=\tensy \scriptfont2=\tensy \scriptscriptfont2=\tensy
\textfont3=\tenex \scriptfont3=\tenex \scriptscriptfont3=\tenex
\lineskip=0pt \hsize=300pt \parindent=0pt \tolerance=10000 \parfillskip=0pt plus 1fil
Hello world
$$\global\dimen0=\predisplaysize\hbox{}$$
\par
\setbox0\hbox{$\mathop{\vtop{\hbox{x}\hbox{yy}}}\limits^{\hbox{abcdefghij}}$}
\showbox0
\end
"#;

#[test]
fn native_word_predisplaysize_and_vtop_rebox_match_texlive() {
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng.input.push_file("probe.tex".into(), SRC.as_bytes().to_vec());
    eng.run();
    assert_eq!(eng.eqtb.dimen[0], (70.29266_f64 * 65536.0).round() as i32);
    let tex_core::boxes::Node::Box { w, h, d, .. } = eng.eqtb.boxed[0].as_ref().unwrap() else {
        panic!("math operator did not produce a box");
    };
    let expected = [44.9078_f64, 27.64651, 17.13399].map(|pt| (pt * 65536.0).round() as i32);
    assert_eq!([*w, *h, *d], expected);
}
