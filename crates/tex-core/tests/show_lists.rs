//! Box and list displays that depend on the nest: `\showlists` in math mode,
//! inside alignments and in paragraphs with their own language, the unset
//! columns of an alignment packing report and `\openout` names. Every
//! expected text is the transcript of `pdftex -ini -etex` (TeX Live 2026)
//! for the same input.

use tex_core::engine::{Engine, InteractionMode};

/// Plain-like math fonts; families 0-3 in all three sizes.
const SETUP: &str = r#"\catcode`\{=1 \catcode`\}=2 \catcode`\$=3 \catcode`\&=4 \catcode`\#=6 \catcode`\^=7 \catcode`\_=8
\font\tenrm=cmr10 \font\teni=cmmi10 \font\tensy=cmsy10 \font\tenex=cmex10
\textfont0=\tenrm \scriptfont0=\tenrm \scriptscriptfont0=\tenrm
\textfont1=\teni \scriptfont1=\teni \scriptscriptfont1=\teni
\textfont2=\tensy \scriptfont2=\tensy \scriptscriptfont2=\tensy
\textfont3=\tenex \scriptfont3=\tenex \scriptscriptfont3=\tenex
\delcode`\(="028300 \delcode`\)="029301 \delcode`\.=0
\tracingonline1 \showboxbreadth100 \showboxdepth100 \tenrm
"#;

fn run(source: &str) -> String {
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.set_interaction_mode(InteractionMode::Nonstop);
    engine.input.push_file(
        "lists.tex".into(),
        format!("{SETUP}{source}\\end\n").into_bytes(),
    );
    engine.run();
    engine.log.clone()
}

fn assert_displays(log: &str, expected: &[&str]) {
    for block in expected {
        assert!(log.contains(block), "missing:\n{block}\nin:\n{log}");
    }
}

/// tex.web §690-§698: noad names, `\limits`/`\nolimits`, scripts, `{}`
/// for an empty sub-mlist, math kerns (`\/` is a plain kern), choice nodes.
#[test]
fn math_list_shows_noads_like_tex() {
    let log = run(
        r#"\setbox0\hbox{$a^b_c \mathop{x}\limits^1 \mathchardef\sum="1350 \sum\nolimits_2 \sum^{1}_2\mathbin{+}\mathrel{=}\mathinner{q}\mathord{}\fam1 A\kern1pt\/ \mathchoice{a}{b}{c}{d}\showlists$}"#,
    );
    assert_displays(
        &log,
        &[
            r#"### math mode entered at line 9
\mathord
.\fam1 a
^\fam1 b
_\fam1 c
\mathop\limits
.\fam1 x
^\fam0 1
\mathop\nolimits
.\fam3 P
_\fam0 2
\mathop
.\fam3 P
^\fam0 1
_\fam0 2
\mathbin
.\fam0 +
\mathrel
.\fam0 =
\mathinner
.\fam1 q
\mathord
.{}
\mathord
.\fam1 A
\kern 1.0
\kern0.0
\mathchoice
D\mathord
D.\fam1 a
T\mathord
T.\fam1 b
S\mathord
S.\fam1 c
s\mathord
s.\fam1 d
### restricted horizontal mode entered at line 9
spacefactor 1000
### vertical mode entered at line 0
prevdepth ignored
"#,
        ],
    );
}

/// tex.web §219: every `{` group, script argument or `\left` group is a
/// level of its own, the noad whose field is being scanned is already on
/// the enclosing list, and `\over` shows `this will begin denominator of:`.
#[test]
fn math_levels_show_pending_noads_and_fractions() {
    let log = run(
        r#"\setbox0\hbox{$\left( {a \over \showlists b} \right)$}
\setbox0\hbox{$x^{\showlists} \underline{y} \mathaccent"7012{\showlists} \radical"161{\showlists}$}
\setbox0\hbox{$a \atopwithdelims() b \showlists$}"#,
    );
    assert_displays(
        &log,
        &[
            r#"### math mode entered at line 9
this will begin denominator of:
\fraction, thickness = default
\\mathord
\.\fam1 a
### math mode entered at line 9
\left"28300
\mathord
### math mode entered at line 9
### restricted horizontal mode entered at line 9
spacefactor 1000
### vertical mode entered at line 0
prevdepth ignored
"#,
            r#"### math mode entered at line 10
### math mode entered at line 10
\mathord
.\fam1 x
### restricted horizontal mode entered at line 10
spacefactor 1000
### vertical mode entered at line 0
prevdepth ignored
"#,
            r#"### math mode entered at line 10
### math mode entered at line 10
\mathord
.\fam1 x
^{}
\underline
.\fam1 y
\accent\fam0 ^^R
### restricted horizontal mode entered at line 10
spacefactor 1000
### vertical mode entered at line 0
prevdepth ignored
"#,
            r#"### math mode entered at line 10
### math mode entered at line 10
\mathord
.\fam1 x
^{}
\underline
.\fam1 y
\accent\fam0 ^^R
.{}
\radical"161
### restricted horizontal mode entered at line 10
spacefactor 1000
### vertical mode entered at line 0
prevdepth ignored
"#,
            r#"### math mode entered at line 11
\mathord
.\fam1 b
this will begin denominator of:
\fraction, thickness 0.0, left-delimiter "28300, right-delimiter "29301
\\mathord
\.\fam1 a
### restricted horizontal mode entered at line 11
spacefactor 1000
### vertical mode entered at line 0
prevdepth ignored
"#,
        ],
    );
}

/// tex.web §218 inside `\halign`: the cell, the row (tabskip glue, unset
/// cells, `spacefactor 0`) and the alignment level with its unset rows and
/// interline glue, `\noalign` material and a spanned cell.
#[test]
fn alignment_levels_show_rows_cells_and_unset_boxes() {
    let log = run(
        r#"\baselineskip12pt \tabskip3pt plus 1pt \parindent0pt
\setbox1\vbox{\halign to 50pt{#\tabskip4pt minus 1pt&\hfil#\cr
a&b\cr
\noalign{\kern2pt}
c\span d\cr
\omit\hbox{x}&\showlists\cr}}"#,
    );
    assert_displays(
        &log,
        &[
            r#"### restricted horizontal mode entered at line 14
\glue 0.0 plus 1.0fil
spacefactor 1000
### restricted horizontal mode entered at line 14
\glue(\tabskip) 3.0 plus 1.0
\unsetbox(4.30554+0.0)x5.2778
.\hbox(4.30554+0.0)x5.2778
..\tenrm x
\glue(\tabskip) 4.0 minus 1.0
spacefactor 0
### internal vertical mode entered at line 10
\unsetbox(6.94444+0.0)x21.55559
.\glue(\tabskip) 3.0 plus 1.0
.\unsetbox(4.30554+0.0)x5.00002
..\tenrm a
.\glue(\tabskip) 4.0 minus 1.0
.\unsetbox(6.94444+0.0)x5.55557, stretch 1.0fil
..\glue 0.0 plus 1.0fil
..\tenrm b
.\glue(\tabskip) 4.0 minus 1.0
\kern 2.0
\glue(\baselineskip) 5.05556
\unsetbox(6.94444+0.0)x17.00002
.\glue(\tabskip) 3.0 plus 1.0
.\unsetbox(6.94444+0.0)x10.00002 (2 columns), stretch 1.0fil
..\tenrm c
..\glue 0.0 plus 1.0fil
..\tenrm d
.\glue(\tabskip) 4.0 minus 1.0
prevdepth 0.0
### internal vertical mode entered at line 10
prevdepth ignored
### vertical mode entered at line 0
prevdepth ignored
"#,
        ],
    );
}

/// tex.web §804: the preamble list holds unset nodes; the report says
/// `in alignment at lines`.
#[test]
fn alignment_preamble_report_shows_unset_columns() {
    let log = run(
        r#"\hsize100pt \tabskip 1pt
\setbox1\vbox{\halign to 10pt{#\cr
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\cr
}}"#,
    );
    assert_displays(
        &log,
        &[r#"Overfull \hbox (182.00058pt too wide) in alignment at lines 10--12
 [] 

\hbox(0.0+0.0)x10.0
.\glue(\tabskip) 1.0
.\unsetbox(0.0+0.0)x190.00058
.\glue(\tabskip) 1.0
"#],
    );
}

/// tex.web §1091/§218: the `(language..:hyphenmin..)` note of every
/// horizontal level shows the values at the start of its paragraph and
/// `current language` is that level's `clang`.
#[test]
fn paragraph_language_is_the_one_in_force_at_its_start() {
    let log = run(
        r#"\hsize100pt \language=3 \lefthyphenmin=4 \righthyphenmin=5
\setbox1\vbox{a\language=7 \lefthyphenmin=1 b\showlists
 \vbox{\language9 c\showlists}}"#,
    );
    assert_displays(
        &log,
        &[
            r#"### horizontal mode entered at line 10 (language3:hyphenmin4,5)
\hbox(0.0+0.0)x0.0
\tenrm a
\setlanguage7 (hyphenmin 1,5)
\tenrm b
spacefactor 1000, current language 7
### internal vertical mode entered at line 10
prevdepth ignored
### vertical mode entered at line 0
prevdepth ignored
"#,
            r#"### horizontal mode entered at line 11 (language9:hyphenmin1,5)
\hbox(0.0+0.0)x0.0
\tenrm c
spacefactor 1000, current language 9
### internal vertical mode entered at line 11
prevdepth ignored
### horizontal mode entered at line 10 (language3:hyphenmin4,5)
\hbox(0.0+0.0)x0.0
\tenrm a
\setlanguage7 (hyphenmin 1,5)
\tenrm b
spacefactor 1000, current language 7
### internal vertical mode entered at line 10
prevdepth ignored
### vertical mode entered at line 0
prevdepth ignored
"#,
        ],
    );
}

/// tex.web §1356: `\openout` shows the name as scanned (`.tex` is not
/// appended, spaces are quoted).
#[test]
fn openout_names_print_as_scanned() {
    let log = run(
        r#"\setbox1\hbox{\openout3=foo \openout4=foo.tex \openout5=./sub/foo.txt \openout6="a b.tex" \write3{x}\closeout3}
\showbox1"#,
    );
    assert_displays(
        &log,
        &[r#"> \box1=
\hbox(0.0+0.0)x0.0
.\openout3=foo
.\openout4=foo.tex
.\openout5=./sub/foo.txt
.\openout6="a b.tex"
.\write3{x}
.\closeout3
"#],
    );
}

/// tex.web §1084/§1076: an `\hbox` appended to a vertical list is packed in
/// an adjusted_hbox_group, so its marks, insertions and `\vadjust` material
/// (`\vadjust pre` first, pdftex.web) follow it on the vertical list; a
/// `\setbox`, `\leaders` or `\copy` of such a box keeps them inside.
#[test]
fn appended_hbox_migrates_marks_inserts_and_vadjust() {
    let log = run(
        r#"\baselineskip=0pt \lineskip=0pt \lineskiplimit=0pt \topskip=0pt
\setbox2\vbox{\hbox{\tenrm y\vadjust pre{\kern 2pt}\mark{a}\vadjust{\kern 3pt}\insert100{\kern 1pt}}\moveleft 3pt\hbox{\mark{z}}\setbox1\hbox{\mark{q}}\copy1 \leaders\hbox{\mark{m}}\vskip 20pt}\showbox2"#,
    );
    assert_displays(
        &log,
        &[r#"> \box2=
\vbox(31.24998+0.0)x5.2778
.\kern 2.0
.\hbox(4.30554+1.94444)x5.2778
..\tenrm y
.\mark{a}
.\kern 3.0
.\insert100, natural size 1.0; split(0.0,0.0); float cost 0
..\kern 1.0
.\glue(\lineskip) 0.0
.\hbox(0.0+0.0)x0.0, shifted -3.0
.\mark{z}
.\glue(\baselineskip) 0.0
.\hbox(0.0+0.0)x0.0
..\mark{q}
.\leaders 20.0
..\hbox(0.0+0.0)x0.0
...\mark{m}
"#],
    );
}

/// tex.web §1151/§1186: `\mathopen{}` and friends are noads with an empty
/// sub-mlist nucleus (an empty hbox) that take part in inter-atom spacing,
/// and a group holding one box noad (`{\raise2pt\hbox{}}`, amsmath's
/// `\smash`) becomes that box nucleus, shift and all.
#[test]
fn empty_class_groups_are_spaced_noads_and_braced_boxes_stay_unpacked() {
    let log = run(
        r#"\thinmuskip3mu \medmuskip4mu \thickmuskip5mu
\setbox0\hbox{$\hbox{}\mathopen{}\mathop{x}y \hbox{}\mathbin{}y \hbox{}\mathrel{}y {\raise2pt\hbox{}}$}\showbox0
\setbox0\hbox{$\hbox{}\mathpunct{}y$}\showbox0
"#,
    );
    assert_displays(
        &log,
        &[
            r#"\hbox(4.65277+1.94444)x33.16644
.\mathon
.\hbox(0.0+0.0)x0.0
.\hbox(0.0+0.0)x0.0
.\hbox(4.30554+0.0)x5.71527, shifted -0.34723
..\teni x
.\glue(\thinmuskip) 1.66663
.\teni y
.\kern0.35878
.\hbox(0.0+0.0)x0.0
.\glue(\medmuskip) 2.22217
.\hbox(0.0+0.0)x0.0
.\glue(\medmuskip) 2.22217
.\teni y
.\kern0.35878
.\hbox(0.0+0.0)x0.0
.\glue(\thickmuskip) 2.77771
.\hbox(0.0+0.0)x0.0
.\glue(\thickmuskip) 2.77771
.\teni y
.\kern0.35878
.\hbox(0.0+0.0)x0.0, shifted -2.0
.\mathoff"#,
            r#"\hbox(4.30554+1.94444)x6.92822
.\mathon
.\hbox(0.0+0.0)x0.0
.\hbox(0.0+0.0)x0.0
.\glue(\thinmuskip) 1.66663
.\teni y
.\kern0.35878
.\mathoff"#,
        ],
    );
}
