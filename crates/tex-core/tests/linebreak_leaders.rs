//! tex.web §866 and §879: a leaders node is a glue node, so the line breaker
//! counts its width, stretch and shrink, may break at it, and discards it at
//! the start of the next line. Each expected line width below was measured
//! with TeX Live 2026 `pdftex -ini` on the same source (cmr10, `\patterns{f1f
//! f1l e1l}`): the natural width of every line, last line first.

use tex_core::engine::{Engine, InteractionMode};

fn check_lines(text: &str, widths: &str) {
    let checks: String = widths.split(' ').map(|w| format!("\\check{w} ")).collect();
    let mut src = String::from(
        r"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\font\cmr=cmr10 \cmr \hyphenchar\cmr=45 \lefthyphenmin=1 \righthyphenmin=1
\parindent=0pt \overfullrule=0pt \hbadness=10000 \hfuzz=16000pt
\pretolerance=100 \tolerance=200 \linepenalty=10 \hyphenpenalty=50 \exhyphenpenalty=50
\parfillskip=0pt plus 1fil \patterns{f1f f1l e1l}
\def\check#1 {\setbox2\lastbox \setbox3\hbox{\unhcopy2}\ifdim\wd3=#1\else\errmessage{got \the\wd3}\fi\unskip\unpenalty}
",
    );
    src.push_str(&format!(
        "\\setbox1\\vbox{{{text}\\par {checks}\\setbox2\\lastbox \\ifvoid2 \\else\\errmessage{{extra line}}\\fi}}\n\\end\n"
    ));
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.set_interaction_mode(InteractionMode::Nonstop);
    engine.input.push_file("leaders.tex".to_string(), src.into_bytes());
    engine.run();
    assert_eq!(engine.error_count, 0, "{text}:\n{}", engine.diagnostic_output);
}

/// A LaTeX list-of-figures entry (`\@dottedtocline`) in a 469.75502pt text
/// block: `\parfillskip=-\rightskip` adds no stretch, so the dot leaders
/// before the page number are the last line's only stretch. With the leaders
/// counted, the first pass (no hyphenation) succeeds and `relates` stays
/// whole, as in TeX Live. With the leaders ignored, the first pass failed and
/// the second pass hyphenated `re-lates` (192.38571pt / 470.94183pt).
#[test]
fn dot_leaders_give_the_last_line_its_stretch() {
    check_lines(
        r"\font\big=cmr10 scaled 1095 \big \hyphenchar\big=45 \lefthyphenmin=2 \righthyphenmin=3
          \hsize=469.75502pt \leftskip=3.8em \rightskip=2.55em \parfillskip=-\rightskip
          \parindent=1.5em \indent\hbox{}\penalty10000 \hskip-\leftskip \hbox to 2.3em{1\hfil}\penalty10000
          We find that the low curve predicts the judgment, while the complex framework
          relates the average regression.\penalty10000
          \leaders\hbox to 3pt{\hss.\hss}\hfill\penalty10000 \hbox to 1.55em{\hfil 1}",
        "201.54114pt 454.48642pt",
    );
}

/// Leaders glue after a character is a legal breakpoint, and a leaders node
/// at the start of the next line is discarded.
#[test]
fn leaders_are_breakpoints_and_are_discarded_after_a_break() {
    for text in [
        r"\hsize=40pt \hskip0pt aa aa aa\leaders\hrule\hskip 3pt plus 1pt bb bb bb",
        r"\hsize=40pt \hskip0pt aa aa aa\leaders\hrule\hskip 3pt plus 1pt
          \leaders\hrule\hskip 4pt bb bb bb",
    ] {
        check_lines(text, "40.00009pt 36.66675pt");
    }
}
