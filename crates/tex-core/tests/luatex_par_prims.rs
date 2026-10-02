//! LuaTeX paragraph primitives: `\localleftbox`, `\localrightbox`, `\localinterlinepenalty`,
//! `\localbrokenpenalty` and the local_par node, `\leftghost`/`\rightghost`, `\nohrule`/`\novrule`,
//! `\gleaders`, `\shapemode`, `\fixupboxesmode`, `\breakafterdirmode`, `\glyphdimensionsmode`.
//! Expected values are `luatex --ini` (TeX Live 2026) output for the same input: the box and list
//! displays, the `\u` lines and what the Lua probes write.

use tex_core::engine::{Engine, EngineKind, InteractionMode};
use tex_core::prim::{DimParam, GlueParam, IntParam};

const PRE: &str = r#"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\$=3 \catcode`\^=7 \catcode`\_=8
\directlua{tex.enableprimitives("",tex.extraprimitives())}
\tracingonline1 \showboxbreadth100 \showboxdepth100 \scrollmode
\hbadness 10000 \vbadness 10000 \hfuzz 16000pt \vfuzz 16000pt
\font\f=cmr10 \f
\long\def\u#1{\immediate\write16{[#1]}}
\hsize 100pt \parindent 5pt \parfillskip 0pt plus 1fil
"#;

/// ini defaults luatex has and `Engine::new_with_kind` leaves to the format.
fn run(body: &str) -> Vec<String> {
    // TeX Live may or may not be installed: look fonts up in the embedded archive only
    std::env::set_var("TEX_RS_HERMETIC", "1");
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    for p in [GlueParam::ParFillSkip, GlueParam::BaselineSkip, GlueParam::LineSkip, GlueParam::ThinMuSkip, GlueParam::MedMuSkip, GlueParam::ThickMuSkip] {
        e.eqtb.set_initial_glue_param(p, tex_core::boxes::Glue::zero());
    }
    for p in [IntParam::Pretolerance, IntParam::LinePenalty, IntParam::HyphenPenalty, IntParam::ExHyphenPenalty, IntParam::ClubPenalty, IntParam::WidowPenalty, IntParam::HBadness, IntParam::VBadness, IntParam::LeftHyphenMin, IntParam::RightHyphenMin, IntParam::Defaulthyphenchar, IntParam::Defaultskewchar, IntParam::DelimiterFactor, IntParam::ErrorContextLines, IntParam::NewLineChar, IntParam::ShowBoxBreadth, IntParam::ShowBoxDepth] {
        e.eqtb.int_params[p.idx() as usize] = 0;
    }
    e.eqtb.int_params[IntParam::Tolerance.idx() as usize] = 10_000;
    for p in [DimParam::HSize, DimParam::VSize, DimParam::MaxDepth, DimParam::ParIndent, DimParam::Hfuzz, DimParam::Vfuzz, DimParam::OverfullRule, DimParam::BoxMaxDepth] {
        e.eqtb.dim_params[p.idx() as usize] = 0;
    }
    e.set_interaction_mode(InteractionMode::Nonstop);
    e.input.push_file("t.tex".to_string(), format!("{PRE}{body}\n\\end\n").into_bytes());
    e.run();
    extract(&e.term)
}

/// The displays and probe lines of a run (the same selection `luatex` was filtered with).
fn extract(term: &str) -> Vec<String> {
    let lines: Vec<&str> = term.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if l.starts_with("> \\box") || l.starts_with("### ") {
            while i < lines.len() && !lines[i].is_empty() {
                out.push(lines[i].to_string());
                i += 1;
            }
            continue;
        }
        let probe = (l.starts_with('[') && l.ends_with(']'))
            || l.starts_with("GD ")
            || l.starts_with("pre")
            || l.starts_with("post")
            || l.starts_with("ilp")
            || (l.starts_with("  ") && l[2..].starts_with(|c: char| c.is_ascii_lowercase()));
        if probe {
            out.push(l.to_string());
        }
        i += 1;
    }
    out
}

fn check(body: &str, expected: &str) {
    let got = run(body);
    let want: Vec<&str> = expected.lines().collect();
    assert_eq!(got, want, "\n--- got ---\n{}\n", got.join("\n"));
}

/// `\localleftbox`/`\localrightbox` are packed boxes that every line carries: the
/// right box before `\rightskip` (before `\parfillskip` on the last line), the
/// left box after the empty `\parindent` box of the first line, and both eat
/// into the line width.
#[test]
fn local_boxes_shape_every_line() {
    check(
        r#"\hsize 50pt \tolerance 10000 \parindent 0pt \hbadness 10000
\setbox1\vbox{\localleftbox{\hbox to 3pt{\vrule width 3pt}}\localrightbox{\hbox to 4pt{\hss R}}
A A A A A A A A A A A A A A A A A A\par}
\showbox1
\setbox1\vbox{\interlinepenalty 100 \noindent A A A\localleftbox{\hbox to 5pt{L}}A A A A A A A A\localrightbox{\hbox to 4pt{R}} A A A A A A A\par}
\showbox1
"#,
        r#"> \box1=
\vbox(34.16656+0.0)x50.0, direction TLT
.\hbox(6.83331+0.0)x50.0, glue set 0.6006, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox
....\hbox(0.0+0.0)x3.0, direction TLT
.....\hbox(0.0+0.0)x3.0, direction TLT
......\rule(*+*)x3.0
...\localrightbox
....\hbox(6.83331+0.0)x4.0, direction TLT
.....\hbox(6.83331+0.0)x4.0, glue set - 3.36111fil, direction TLT
......\glue 0.0 plus 1.0fil minus 1.0fil
......\f R
..\hbox(0.0+0.0)x0.0, direction TLT
..\hbox(0.0+0.0)x3.0, direction TLT
...\hbox(0.0+0.0)x3.0, direction TLT
....\rule(*+*)x3.0
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\hbox(6.83331+0.0)x4.0, direction TLT
...\hbox(6.83331+0.0)x4.0, glue set - 3.36111fil, direction TLT
....\glue 0.0 plus 1.0fil minus 1.0fil
....\f R
..\glue(\rightskip) 0.0
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x50.0, glue set 0.6006, direction TLT
..\hbox(0.0+0.0)x3.0, direction TLT
...\hbox(0.0+0.0)x3.0, direction TLT
....\rule(*+*)x3.0
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\hbox(6.83331+0.0)x4.0, direction TLT
...\hbox(6.83331+0.0)x4.0, glue set - 3.36111fil, direction TLT
....\glue 0.0 plus 1.0fil minus 1.0fil
....\f R
..\glue(\rightskip) 0.0
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x50.0, glue set 0.6006, direction TLT
..\hbox(0.0+0.0)x3.0, direction TLT
...\hbox(0.0+0.0)x3.0, direction TLT
....\rule(*+*)x3.0
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\hbox(6.83331+0.0)x4.0, direction TLT
...\hbox(6.83331+0.0)x4.0, glue set - 3.36111fil, direction TLT
....\glue 0.0 plus 1.0fil minus 1.0fil
....\f R
..\glue(\rightskip) 0.0
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x50.0, glue set 0.6006, direction TLT
..\hbox(0.0+0.0)x3.0, direction TLT
...\hbox(0.0+0.0)x3.0, direction TLT
....\rule(*+*)x3.0
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\hbox(6.83331+0.0)x4.0, direction TLT
...\hbox(6.83331+0.0)x4.0, glue set - 3.36111fil, direction TLT
....\glue 0.0 plus 1.0fil minus 1.0fil
....\f R
..\glue(\rightskip) 0.0
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x50.0, glue set 24.66664fil, direction TLT
..\hbox(0.0+0.0)x3.0, direction TLT
...\hbox(0.0+0.0)x3.0, direction TLT
....\rule(*+*)x3.0
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\penalty 10000
..\hbox(6.83331+0.0)x4.0, direction TLT
...\hbox(6.83331+0.0)x4.0, glue set - 3.36111fil, direction TLT
....\glue 0.0 plus 1.0fil minus 1.0fil
....\f R
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box1=
\vbox(34.16656+0.0)x50.0, direction TLT
.\hbox(6.83331+0.0)x50.0, glue set 0.50049, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox
....\hbox(6.83331+0.0)x5.0, direction TLT
.....\hbox(6.83331+0.0)x5.0, direction TLT
......\f L
...\localrightbox=null
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\rightskip) 0.0
.\penalty 100
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x50.0, glue set 1.001, direction TLT
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\rightskip) 0.0
.\penalty 100
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x50.0, glue set 0.2002, direction TLT
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox
....\hbox(6.83331+0.0)x5.0, direction TLT
.....\hbox(6.83331+0.0)x5.0, direction TLT
......\f L
...\localrightbox
....\hbox(6.83331+0.0)x4.0, direction TLT
.....\hbox(6.83331+0.0)x4.0, direction TLT
......\f R
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\hbox(6.83331+0.0)x4.0, direction TLT
...\hbox(6.83331+0.0)x4.0, direction TLT
....\f R
..\glue(\rightskip) 0.0
.\penalty 100
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x50.0, glue set 0.2002, direction TLT
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\hbox(6.83331+0.0)x4.0, direction TLT
...\hbox(6.83331+0.0)x4.0, direction TLT
....\f R
..\glue(\rightskip) 0.0
.\penalty 100
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x50.0, glue set 33.49998fil, direction TLT
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
..\f A
..\penalty 10000
..\hbox(6.83331+0.0)x4.0, direction TLT
...\hbox(6.83331+0.0)x4.0, direction TLT
....\f R
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
"#,
    );
}

/// `\localinterlinepenalty`/`\localbrokenpenalty` replace `\interlinepenalty`/
/// `\brokenpenalty` from the position of their local_par node on, unless an
/// `\interlinepenalties` array applies.
#[test]
fn local_penalties_replace_line_penalties() {
    check(
        r#"\setbox1\vbox{\interlinepenalty 100 \brokenpenalty 200 \noindent A A A \localinterlinepenalty 7 A A A A A \localbrokenpenalty 9 A A A A A A A A A A A\par}
\showbox1
\hsize 20pt \parindent 0pt \tolerance 10000 \hbadness 10000 \hfuzz 100pt
\interlinepenalty 100 \brokenpenalty 200 \clubpenalty 0 \widowpenalty 0
\setbox1\vbox{\noindent AAA\discretionary{}{}{}AAA\discretionary{}{}{}AAA\discretionary{}{}{}AAA\par}
\showbox1
\setbox1\vbox{\noindent AAA\discretionary{}{}{}AAA\localbrokenpenalty 9 \discretionary{}{}{}AAA\localinterlinepenalty 4 \discretionary{}{}{}AAA\par}
\showbox1
\setbox1\vbox{\noindent\localbrokenpenalty 9 \localinterlinepenalty 4 AAA\discretionary{}{}{}AAA\discretionary{}{}{}AAA\discretionary{}{}{}AAA\par}
\showbox1
\interlinepenalties 2 11 12 \setbox1\vbox{\noindent\localinterlinepenalty 4 AAA\discretionary{}{}{}AAA\discretionary{}{}{}AAA\discretionary{}{}{}AAA\par}
\showbox1
\setbox1\vbox{\noindent\localbrokenpenalty 9 {\localbrokenpenalty 0 AAA\discretionary{}{}{}AAA}\discretionary{}{}{}AAA\discretionary{}{}{}AAA\par}
\showbox1
\setbox1\vbox{\noindent A\localinterlinepenalty 3\ B\par}
\showbox1
"#,
        r#"> \box1=
\vbox(20.49994+0.0)x100.0, direction TLT
.\hbox(6.83331+0.0)x100.0, glue set 0.43794, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\localpar
...\localinterlinepenalty=7
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\localpar
...\localinterlinepenalty=7
...\localbrokenpenalty=9
...\localleftbox=null
...\localrightbox=null
..\f A
..\glue(\rightskip) 0.0
.\penalty 7
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x100.0, glue set 0.43794, direction TLT
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\rightskip) 0.0
.\penalty 7
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x100.0, glue set 92.49998fil, direction TLT
..\f A
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box1=
\vbox(27.33325+0.0)x20.0, direction TLT
.\hbox(6.83331+0.0)x20.0, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f A
..\f A
..\f A
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 300
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 300
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 300
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box1=
\vbox(27.33325+0.0)x20.0, direction TLT
.\hbox(6.83331+0.0)x20.0, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f A
..\f A
..\f A
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 300
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=9
...\localleftbox=null
...\localrightbox=null
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 109
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\localpar
...\localinterlinepenalty=4
...\localbrokenpenalty=9
...\localleftbox=null
...\localrightbox=null
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 13
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box1=
\vbox(27.33325+0.0)x20.0, direction TLT
.\hbox(6.83331+0.0)x20.0, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=9
...\localleftbox=null
...\localrightbox=null
..\localpar
...\localinterlinepenalty=4
...\localbrokenpenalty=9
...\localleftbox=null
...\localrightbox=null
..\f A
..\f A
..\f A
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 13
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 13
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 13
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box1=
\vbox(27.33325+0.0)x20.0, direction TLT
.\hbox(6.83331+0.0)x20.0, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\localpar
...\localinterlinepenalty=4
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f A
..\f A
..\f A
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 204
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 204
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 204
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box1=
\vbox(27.33325+0.0)x20.0, direction TLT
.\hbox(6.83331+0.0)x20.0, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=9
...\localleftbox=null
...\localrightbox=null
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f A
..\f A
..\f A
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 300
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=9
...\localleftbox=null
...\localrightbox=null
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 109
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\discretionary (penalty 0)
..\glue(\rightskip) 0.0
.\penalty 109
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x20.0, direction TLT
..\f A
..\f A
..\f A
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box1=
\vbox(6.83331+0.0)x20.0, direction TLT
.\hbox(6.83331+0.0)x20.0, glue set 2.0833fil, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f A
..\localpar
...\localinterlinepenalty=3
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\glue(\spaceskip) 3.33333 plus 1.66666 minus 1.11111
..\f B
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
"#,
    );
}

/// A paragraph starts with a local_par node, the local primitives append another
/// one when they take effect in horizontal mode and when their group ends, and a
/// display resumes the paragraph with a new one.
#[test]
fn local_par_nodes_in_lists() {
    check(
        r#"\setbox1\hbox{x}
\noindent A \localinterlinepenalty 7 A \showlists
\localleftbox{\hbox to 5pt{L}}
\showlists \par
\font\s=cmsy10 \font\x=cmex10 \font\m=cmmi10
\textfont0=\f \scriptfont0=\f \scriptscriptfont0=\f
\textfont1=\m \scriptfont1=\m \scriptscriptfont1=\m
\textfont2=\s \scriptfont2=\s \scriptscriptfont2=\s
\textfont3=\x \scriptfont3=\x \scriptscriptfont3=\x
\hsize 100pt \parindent 5pt \tolerance 10000
\setbox1\vbox{\noindent A {\localinterlinepenalty 5 B} C \showlists \par}
\setbox1\vbox{A$$\showlists\relax$$\showlists B\par}
\showbox1
\setbox1\vbox{\noindent$$\relax$$ B\par}
\showbox1
\setbox1\vbox{\noindent A\begingroup\localinterlinepenalty 5 \endgroup B \showlists\par}
\setbox1\vbox{\noindent \u{\the\lastnodetype}\showlists\par}
\setbox1\vbox{\noindent A \showlists\unskip\u{\the\lastnodetype}\par}
"#,
        r#"### horizontal mode entered at line 9
\localpar
.\localinterlinepenalty=0
.\localbrokenpenalty=0
.\localleftbox=null
.\localrightbox=null
\f A
\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
\localpar
.\localinterlinepenalty=7
.\localbrokenpenalty=0
.\localleftbox=null
.\localrightbox=null
\f A
\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
spacefactor 999
### vertical mode entered at line 0
prevdepth ignored
### horizontal mode entered at line 9
\localpar
.\localinterlinepenalty=0
.\localbrokenpenalty=0
.\localleftbox=null
.\localrightbox=null
\f A
\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
\localpar
.\localinterlinepenalty=7
.\localbrokenpenalty=0
.\localleftbox=null
.\localrightbox=null
\f A
\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
\localpar
.\localinterlinepenalty=7
.\localbrokenpenalty=0
.\localleftbox
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
.\localrightbox=null
\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
spacefactor 999
### vertical mode entered at line 0
prevdepth ignored
### horizontal mode entered at line 18
\localpar
.\localinterlinepenalty=7
.\localbrokenpenalty=0
.\localleftbox
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
.\localrightbox=null
\f A
\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
\localpar
.\localinterlinepenalty=5
.\localbrokenpenalty=0
.\localleftbox
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
.\localrightbox=null
\f B
\localpar
.\localinterlinepenalty=7
.\localbrokenpenalty=0
.\localleftbox
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
.\localrightbox=null
\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
\f C
\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
spacefactor 999
### internal vertical mode entered at line 18
prevdepth ignored
### vertical mode entered at line 0
### current page:
\glue(\topskip) 0.0
\hbox(6.83331+0.0)x100.0, glue set 78.33331fil, direction TLT
.\localpar
..\localinterlinepenalty=0
..\localbrokenpenalty=0
..\localleftbox=null
..\localrightbox=null
.\f A
.\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox=null
..\localrightbox=null
.\f A
.\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox
...\hbox(6.83331+0.0)x5.0, direction TLT
....\hbox(6.83331+0.0)x5.0, direction TLT
.....\f L
..\localrightbox=null
.\penalty 10000
.\glue(\parfillskip) 0.0 plus 1.0fil
.\glue(\rightskip) 0.0
total height 6.83331
 goal height 0.0
prevdepth 0.0, prevgraf 1 line
### display math mode entered at line 19
### internal vertical mode entered at line 19
\hbox(6.83331+0.0)x100.0, glue set 82.49998fil, direction TLT
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox
...\hbox(6.83331+0.0)x5.0, direction TLT
....\hbox(6.83331+0.0)x5.0, direction TLT
.....\f L
..\localrightbox=null
.\hbox(0.0+0.0)x5.0, direction TLT
.\hbox(6.83331+0.0)x5.0, direction TLT
..\hbox(6.83331+0.0)x5.0, direction TLT
...\f L
.\f A
.\penalty 10000
.\glue(\parfillskip) 0.0 plus 1.0fil
.\glue(\rightskip) 0.0
prevdepth 0.0, prevgraf 1 line
### vertical mode entered at line 0
### current page:
\glue(\topskip) 0.0
\hbox(6.83331+0.0)x100.0, glue set 78.33331fil, direction TLT
.\localpar
..\localinterlinepenalty=0
..\localbrokenpenalty=0
..\localleftbox=null
..\localrightbox=null
.\f A
.\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox=null
..\localrightbox=null
.\f A
.\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox
...\hbox(6.83331+0.0)x5.0, direction TLT
....\hbox(6.83331+0.0)x5.0, direction TLT
.....\f L
..\localrightbox=null
.\penalty 10000
.\glue(\parfillskip) 0.0 plus 1.0fil
.\glue(\rightskip) 0.0
total height 6.83331
 goal height 0.0
prevdepth 0.0, prevgraf 1 line
### horizontal mode entered at line 19
\localpar
.\localinterlinepenalty=7
.\localbrokenpenalty=0
.\localleftbox
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
.\localrightbox=null
spacefactor 1000
### internal vertical mode entered at line 19
\hbox(6.83331+0.0)x100.0, glue set 82.49998fil, direction TLT
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox
...\hbox(6.83331+0.0)x5.0, direction TLT
....\hbox(6.83331+0.0)x5.0, direction TLT
.....\f L
..\localrightbox=null
.\hbox(0.0+0.0)x5.0, direction TLT
.\hbox(6.83331+0.0)x5.0, direction TLT
..\hbox(6.83331+0.0)x5.0, direction TLT
...\f L
.\f A
.\penalty 10000
.\glue(\parfillskip) 0.0 plus 1.0fil
.\glue(\rightskip) 0.0
\penalty 0
\glue(\abovedisplayshortskip) 0.0
\glue(\baselineskip) 0.0
\hbox(0.0+0.0)x0.0, shifted 50.0, direction TLT
\penalty 0
\glue(\belowdisplayshortskip) 0.0
prevdepth 0.0, prevgraf 4 lines
### vertical mode entered at line 0
### current page:
\glue(\topskip) 0.0
\hbox(6.83331+0.0)x100.0, glue set 78.33331fil, direction TLT
.\localpar
..\localinterlinepenalty=0
..\localbrokenpenalty=0
..\localleftbox=null
..\localrightbox=null
.\f A
.\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox=null
..\localrightbox=null
.\f A
.\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox
...\hbox(6.83331+0.0)x5.0, direction TLT
....\hbox(6.83331+0.0)x5.0, direction TLT
.....\f L
..\localrightbox=null
.\penalty 10000
.\glue(\parfillskip) 0.0 plus 1.0fil
.\glue(\rightskip) 0.0
total height 6.83331
 goal height 0.0
prevdepth 0.0, prevgraf 1 line
> \box1=
\vbox(13.66663+0.0)x100.0, direction TLT
.\hbox(6.83331+0.0)x100.0, glue set 82.49998fil, direction TLT
..\localpar
...\localinterlinepenalty=7
...\localbrokenpenalty=0
...\localleftbox
....\hbox(6.83331+0.0)x5.0, direction TLT
.....\hbox(6.83331+0.0)x5.0, direction TLT
......\f L
...\localrightbox=null
..\hbox(0.0+0.0)x5.0, direction TLT
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
..\f A
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
.\penalty 0
.\glue(\abovedisplayshortskip) 0.0
.\glue(\baselineskip) 0.0
.\hbox(0.0+0.0)x0.0, shifted 50.0, direction TLT
.\penalty 0
.\glue(\belowdisplayshortskip) 0.0
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x100.0, glue set 87.91664fil, direction TLT
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
..\localpar
...\localinterlinepenalty=7
...\localbrokenpenalty=0
...\localleftbox
....\hbox(6.83331+0.0)x5.0, direction TLT
.....\hbox(6.83331+0.0)x5.0, direction TLT
......\f L
...\localrightbox=null
..\f B
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box1=
\vbox(6.83331+0.0)x100.0, direction TLT
.\penalty 0
.\glue(\abovedisplayshortskip) 0.0
.\hbox(0.0+0.0)x0.0, shifted 50.0, direction TLT
.\penalty 0
.\glue(\belowdisplayshortskip) 0.0
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x100.0, glue set 87.91664fil, direction TLT
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
..\localpar
...\localinterlinepenalty=7
...\localbrokenpenalty=0
...\localleftbox
....\hbox(6.83331+0.0)x5.0, direction TLT
.....\hbox(6.83331+0.0)x5.0, direction TLT
......\f L
...\localrightbox=null
..\f B
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
### horizontal mode entered at line 23
\localpar
.\localinterlinepenalty=7
.\localbrokenpenalty=0
.\localleftbox
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
.\localrightbox=null
\f A
\localpar
.\localinterlinepenalty=5
.\localbrokenpenalty=0
.\localleftbox
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
.\localrightbox=null
\localpar
.\localinterlinepenalty=7
.\localbrokenpenalty=0
.\localleftbox
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
.\localrightbox=null
\f B
\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
spacefactor 999
### internal vertical mode entered at line 23
prevdepth ignored
### vertical mode entered at line 0
### current page:
\glue(\topskip) 0.0
\hbox(6.83331+0.0)x100.0, glue set 78.33331fil, direction TLT
.\localpar
..\localinterlinepenalty=0
..\localbrokenpenalty=0
..\localleftbox=null
..\localrightbox=null
.\f A
.\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox=null
..\localrightbox=null
.\f A
.\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox
...\hbox(6.83331+0.0)x5.0, direction TLT
....\hbox(6.83331+0.0)x5.0, direction TLT
.....\f L
..\localrightbox=null
.\penalty 10000
.\glue(\parfillskip) 0.0 plus 1.0fil
.\glue(\rightskip) 0.0
total height 6.83331
 goal height 0.0
prevdepth 0.0, prevgraf 1 line
[-1]
### horizontal mode entered at line 24
\localpar
.\localinterlinepenalty=7
.\localbrokenpenalty=0
.\localleftbox
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
.\localrightbox=null
spacefactor 1000
### internal vertical mode entered at line 24
prevdepth ignored
### vertical mode entered at line 0
### current page:
\glue(\topskip) 0.0
\hbox(6.83331+0.0)x100.0, glue set 78.33331fil, direction TLT
.\localpar
..\localinterlinepenalty=0
..\localbrokenpenalty=0
..\localleftbox=null
..\localrightbox=null
.\f A
.\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox=null
..\localrightbox=null
.\f A
.\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox
...\hbox(6.83331+0.0)x5.0, direction TLT
....\hbox(6.83331+0.0)x5.0, direction TLT
.....\f L
..\localrightbox=null
.\penalty 10000
.\glue(\parfillskip) 0.0 plus 1.0fil
.\glue(\rightskip) 0.0
total height 6.83331
 goal height 0.0
prevdepth 0.0, prevgraf 1 line
### horizontal mode entered at line 25
\localpar
.\localinterlinepenalty=7
.\localbrokenpenalty=0
.\localleftbox
..\hbox(6.83331+0.0)x5.0, direction TLT
...\hbox(6.83331+0.0)x5.0, direction TLT
....\f L
.\localrightbox=null
\f A
\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
spacefactor 999
### internal vertical mode entered at line 25
prevdepth ignored
### vertical mode entered at line 0
### current page:
\glue(\topskip) 0.0
\hbox(6.83331+0.0)x100.0, glue set 78.33331fil, direction TLT
.\localpar
..\localinterlinepenalty=0
..\localbrokenpenalty=0
..\localleftbox=null
..\localrightbox=null
.\f A
.\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox=null
..\localrightbox=null
.\f A
.\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
.\localpar
..\localinterlinepenalty=7
..\localbrokenpenalty=0
..\localleftbox
...\hbox(6.83331+0.0)x5.0, direction TLT
....\hbox(6.83331+0.0)x5.0, direction TLT
.....\f L
..\localrightbox=null
.\penalty 10000
.\glue(\parfillskip) 0.0 plus 1.0fil
.\glue(\rightskip) 0.0
total height 6.83331
 goal height 0.0
prevdepth 0.0, prevgraf 1 line
[-1]
"#,
    );
}

/// `\nohrule`/`\novrule` take space but draw nothing, `\gleaders` is a leaders
/// kind of its own.
#[test]
fn rules_and_leaders() {
    check(
        r#"\setbox1\hbox{\nohrule width 3pt \novrule height 4pt \vrule width 5pt \hrule}
\showbox1
\setbox1\vbox{\nohrule height 3pt \hrule height 2pt\novrule}
\showbox1
\u{\the\wd1}
\setbox2\hbox{\leaders\hrule\hskip 10pt \gleaders\hrule\hskip 10pt \xleaders\hbox{A}\hskip 10pt \gleaders\hbox{A}\hskip 10pt}
\showbox2
\setbox2\hbox{\gleaders\hbox{A}\hskip 10pt}
\showbox2
\u{\meaning\gleaders}
"#,
        r#"> \box1=
\hbox(6.94444+1.94444)x51.5112, direction TLT
.\f w
.\f i
.\f d
.\f t
.\f h
.\glue(\spaceskip) 3.33333 plus 1.66666 minus 1.11111
.\f 3
.\f p
.\f t
.\glue(\spaceskip) 3.33333 plus 1.66666 minus 1.11111
.\norule(4.0+*)x0.4
.\rule(*+*)x5.0
> \box1=
\vbox(5.0+0.0)x100.0, direction TLT
.\norule(3.0+0.0)x*
.\rule(2.0+0.0)x*
.\glue(\parskip) 0.0
.\hbox(0.0+0.0)x100.0, glue set 94.6fil, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\hbox(0.0+0.0)x5.0, direction TLT
..\norule(*+*)x0.4
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
[100.0pt]
> \box2=
\hbox(6.83331+0.0)x40.0, direction TLT
.\leaders 10.0
..\rule(0.4+0.0)x*
.\gleaders 10.0
..\rule(0.4+0.0)x*
.\xleaders 10.0
..\hbox(6.83331+0.0)x7.50002, direction TLT
...\f A
.\gleaders 10.0
..\hbox(6.83331+0.0)x7.50002, direction TLT
...\f A
> \box2=
\hbox(6.83331+0.0)x10.0, direction TLT
.\gleaders 10.0
..\hbox(6.83331+0.0)x7.50002, direction TLT
...\f A
[\gleaders]
"#,
    );
}

/// `\leftghost`/`\rightghost` keep the kern with the neighbour on the other side.
#[test]
fn ghost_kerning() {
    check(
        r#"\def\t#1{\setbox1\hbox{#1}\u{#1: \the\wd1}\showbox1}
\t{\rightghost AV}
\t{\rightghost AVA}
\t{A\rightghost VA}
\t{V\rightghost AV}

\def\t#1{\setbox1\hbox{#1}\u{#1: \the\wd1}}
\t{Te}\t{T\leftghost e}\t{T\rightghost e}\t{\leftghost Te}\t{\rightghost Te}
\t{e\leftghost T}\t{e\rightghost T}
\t{x\leftghost Te}\t{x\rightghost Te}
\t{T\leftghost e\leftghost e}
\t{A\leftghost C}\t{A\rightghost C}\t{F\leftghost A}\t{F\rightghost A}\t{\leftghost FA}\t{\rightghost FA}\t{F\leftghost AC}\t{F\rightghost AC}
\t{xe}\t{x\leftghost e}\t{x}
"#,
        r#"[\rightghost AV: 7.50002pt]
> \box1=
\hbox(6.83331+0.0)x7.50002, direction TLT
.\f V
[\rightghost AVA: 13.8889pt]
> \box1=
\hbox(6.83331+0.0)x13.8889, direction TLT
.\f V
.\kern-1.11113 (font)
.\f A
[A\rightghost VA: 15.00003pt]
> \box1=
\hbox(6.83331+0.0)x15.00003, direction TLT
.\f A
.\f A
[V\rightghost AV: 15.00003pt]
> \box1=
\hbox(6.83331+0.0)x15.00003, direction TLT
.\f V
.\f V
[Te: 10.83333pt]
[T\leftghost e: 6.38889pt]
[T\rightghost e: 7.22223pt]
[\leftghost Te: 4.44444pt]
[\rightghost Te: 4.44444pt]
[e\leftghost T: 4.44444pt]
[e\rightghost T: 4.44444pt]
[x\leftghost Te: 9.72224pt]
[x\rightghost Te: 9.72224pt]
[T\leftghost e\leftghost e: 6.38889pt]
[A\leftghost C: 7.22223pt]
[A\rightghost C: 7.50002pt]
[F\leftghost A: 5.41667pt]
[F\rightghost A: 6.5278pt]
[\leftghost FA: 7.50002pt]
[\rightghost FA: 7.50002pt]
[F\leftghost AC: 12.6389pt]
[F\rightghost AC: 13.75003pt]
[xe: 9.72224pt]
[x\leftghost e: 5.2778pt]
[x: 5.2778pt]
"#,
    );
}

/// `\shapemode` mirrors `\hangindent`, `\fixupboxesmode` closes the direction of a
/// box, and open directions are closed and reopened at every line.
#[test]
fn shapemode_fixupboxes_dirs() {
    check(
        r#"\hsize 40pt \parindent 0pt \tolerance 10000
\setbox1\vbox{A A A \textdir TRT A A A A A A A A\par}
\showbox1
\hsize 40pt \parindent 0pt \tolerance 10000 \shapemode 1 \hangindent 10pt \hangafter -1
\setbox1\vbox{A A A A A A A A A A A\par}
\showbox1
\shapemode 0
\setbox1\hbox{\fixupboxesmode1 \hbox{\textdir TRT A}}
\showbox1
\setbox1\hbox{\fixupboxesmode0 \hbox{\textdir TRT A}}
\showbox1
"#,
        r#"> \box1=
\vbox(20.49994+0.0)x40.0, direction TLT
.\hbox(6.83331+0.0)x40.0, glue set - 0.00002, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\hbox(0.0+0.0)x0.0, direction TLT
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\begindir TRT
..\f A
..\enddir TRT
..\glue(\rightskip) 0.0
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x40.0, glue set - 0.00002, direction TLT
..\begindir TRT
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\enddir TRT
..\glue(\rightskip) 0.0
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x40.0, glue set 10.8333fil, direction TLT
..\begindir TRT
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\penalty 10000
..\enddir TRT
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box1=
\vbox(20.49994+0.0)x40.0, direction TLT
.\hbox(6.83331+0.0)x40.0, glue set - 0.00002, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\hbox(0.0+0.0)x0.0, direction TLT
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\rightskip) 0.0
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x40.0, glue set - 0.00002, direction TLT
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\rightskip) 0.0
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x40.0, glue set 10.8333fil, direction TLT
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box1=
\hbox(6.83331+0.0)x7.50002, direction TLT
.\hbox(6.83331+0.0)x7.50002, direction TLT
..\begindir TRT
..\f A
..\enddir TRT
> \box1=
\hbox(6.83331+0.0)x7.50002, direction TLT
.\hbox(6.83331+0.0)x7.50002, direction TLT
..\begindir TRT
..\f A
"#,
    );
}

/// With `\breakafterdirmode=1` glue after a dir node is a legal break.
#[test]
fn break_after_dir_mode() {
    check(
        r#"\hsize 6pt \parindent 0pt \tolerance 10000 \hbadness 10000 \hfuzz 100pt
\breakafterdirmode=0
\setbox1\vbox{\noindent A\textdir TRT\ B\par}
\showbox1
\breakafterdirmode=1
\setbox1\vbox{\noindent A\textdir TRT\ B\par}
\showbox1
\breakafterdirmode=1
\setbox1\vbox{\noindent A\localleftbox{}\ B\par}
\showbox1
"#,
        r#"> \box1=
\vbox(6.83331+0.0)x6.0, direction TLT
.\hbox(6.83331+0.0)x6.0, glue set - 1.0, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f A
..\begindir TRT
..\glue(\spaceskip) 3.33333 plus 1.66666 minus 1.11111
..\f B
..\penalty 10000
..\enddir TRT
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box1=
\vbox(13.66663+0.0)x6.0, direction TLT
.\hbox(6.83331+0.0)x6.0, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f A
..\begindir TRT
..\enddir TRT
..\glue(\rightskip) 0.0
.\glue(\lineskip) 0.0
.\hbox(6.83331+0.0)x6.0, direction TLT
..\begindir TRT
..\f B
..\penalty 10000
..\enddir TRT
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box1=
\vbox(6.83331+0.0)x6.0, direction TLT
.\hbox(6.83331+0.0)x6.0, glue set - 1.0, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f A
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\glue(\spaceskip) 3.33333 plus 1.66666 minus 1.11111
..\f B
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
"#,
    );
}

/// `\glyphdimensionsmode` decides how the vertical offset of a glyph enters its height and depth.
#[test]
fn glyph_dimensions_mode() {
    check(
        r#"\directlua{
 local t = font.read_tfm("cmr10", 655360)
 t.name = "mycmr"
 local id = font.define(t)
 for _, mode in ipairs{0,1,2,3} do
  tex.glyphdimensionsmode = mode
  for _, y in ipairs{-100000, 0, 150000} do
   local g = node.new("glyph")
   g.font = id; g.char = 65; g.yoffset = y
   local h = node.hpack(g)
   texio.write_nl("GD mode="..mode.." y="..y.." h="..h.height.." d="..h.depth.." w="..h.width)
  end
 end
}
\setbox1\hbox{\directlua{
 local g = node.new("glyph") g.font = font.current() g.char = 65 g.yoffset = -70000 node.write(g)}}
\glyphdimensionsmode=1 \u{HT=\the\ht1 DP=\the\dp1}
\setbox1\hbox{\directlua{
 local g = node.new("glyph") g.font = font.current() g.char = 65 g.yoffset = 70000 node.write(g)}}
\glyphdimensionsmode=2 \setbox2\hbox{\unhbox1}
"#,
        r#"GD mode=0 y=-100000 h=347828 d=0 w=491521
GD mode=0 y=0 h=447828 d=0 w=491521
GD mode=0 y=150000 h=597828 d=0 w=491521
GD mode=1 y=-100000 h=347828 d=100000 w=491521
GD mode=1 y=0 h=447828 d=0 w=491521
GD mode=1 y=150000 h=597828 d=0 w=491521
GD mode=2 y=-100000 h=447828 d=100000 w=491521
GD mode=2 y=0 h=447828 d=0 w=491521
GD mode=2 y=150000 h=597828 d=0 w=491521
GD mode=3 y=-100000 h=447828 d=0 w=491521
GD mode=3 y=0 h=447828 d=0 w=491521
GD mode=3 y=150000 h=447828 d=0 w=491521
[HT=6.83331ptDP=0.0pt]
"#,
    );
}

/// A box starts with a text direction list of its own, a group's `\textdir` ends with the group and
/// paragraphs reopen the directions in force.
#[test]
fn text_direction_is_per_box_and_group() {
    check(
        r#"\hsize 100pt \parindent 0pt
\setbox2\hbox{\textdir TRT A}
\setbox1\vbox{\noindent B\par}
\showbox1
\setbox2\hbox{\fixupboxesmode1 \textdir TRT A}
\showbox2
\begingroup\textdir TRT \setbox1\vbox{\noindent C\par}\endgroup
\showbox1
{\textdir TRT \setbox1\hbox{\textdir TLT x}\setbox3\vbox{\noindent D\par}\showbox3}
\setbox1\vbox{\noindent E\par}
\showbox1
"#,
        r#"> \box1=
\vbox(6.83331+0.0)x100.0, direction TLT
.\hbox(6.83331+0.0)x100.0, glue set 92.91664fil, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f B
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box2=
\hbox(6.83331+0.0)x7.50002, direction TLT
.\begindir TRT
.\f A
.\enddir TRT
> \box1=
\vbox(6.83331+0.0)x100.0, direction TLT
.\hbox(6.83331+0.0)x100.0, glue set 92.91664fil, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f B
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box3=
\vbox(6.83331+0.0)x100.0, direction TLT
.\hbox(6.83331+0.0)x100.0, glue set 92.3611fil, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f D
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
> \box1=
\vbox(6.83331+0.0)x100.0, direction TLT
.\hbox(6.83331+0.0)x100.0, glue set 93.19443fil, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f E
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
"#,
    );
}

/// node.local_par fields, `insert_local_par` and the lists the line breaker callbacks see.
#[test]
fn local_par_in_lua() {
    check(
        r#"\directlua{
local function dump(tag, head)
  local t = {}
  texio.write_nl(tag)
  for n in node.traverse(head) do
    local s = node.type(n.id) .. (n.subtype and ("/"..n.subtype) or "")
    if n.id == node.id("local_par") then
      s = s .. "[" .. n.pen_inter .. "," .. n.pen_broken .. "," .. tostring(n.dir) .. "," .. n.box_left_width .. "," .. n.box_right_width .. "," .. tostring(n.box_left ~= nil) .. "," .. tostring(n.box_right ~= nil) .. "]"
    end
    texio.write_nl("  "..s)
  end
end
luatexbase = nil
callback.register("pre_linebreak_filter", function(head, grp) dump("pre "..grp, head) return true end)
callback.register("insert_local_par", function(n, mode) texio.write_nl("ilp "..mode.." "..node.type(n.id)) end)
callback.register("post_linebreak_filter", function(head, grp) dump("post "..grp, head) return true end)
}
\hsize 100pt \parindent 5pt
\setbox1\vbox{\localinterlinepenalty 3 A\par}
\setbox1\vbox{A {\localrightbox{\hbox to 4pt{R}}B} C\par}
\setbox1\vbox{\noindent A \localleftbox{\hbox to 2pt{}}B\par}
\setbox1\vbox{\begingroup\localbrokenpenalty 4 \endgroup A\par}
\setbox1\vbox{\localinterlinepenalty 5 \localbrokenpenalty 6 \noindent\par}
\setbox1\vbox{A $$ x $$ B\par}
\setbox1\vbox{\noindent\directlua{
  local n = node.new("local_par"); n.pen_inter = 9; node.write(n)}A A\par}
\showbox1
"#,
        r#"ilp new_graf local_par
pre 
post 
ilp new_graf local_par
ilp local_box local_par
ilp hmode_par local_par
pre 
post 
ilp new_graf local_par
ilp local_box local_par
pre 
post 
ilp new_graf local_par
pre 
post 
ilp new_graf local_par
ilp new_graf local_par
pre math_shift
post math_shift
ilp penalty local_par
pre 
post 
ilp new_graf local_par
pre 
post 
> \box1=
\vbox(6.83331+0.0)x100.0, direction TLT
.\hbox(6.83331+0.0)x100.0, glue set 81.66664fil, direction TLT
..\localpar
...\localinterlinepenalty=0
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\localpar
...\localinterlinepenalty=9
...\localbrokenpenalty=0
...\localleftbox=null
...\localrightbox=null
..\f A
..\glue(\spaceskip) 3.33333 plus 1.66498 minus 1.11221
..\f A
..\penalty 10000
..\glue(\parfillskip) 0.0 plus 1.0fil
..\glue(\rightskip) 0.0
"#,
    );
}
