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

/// tex.web §755: a character nucleus with scripts stays the bare character
/// node (plus its italic kern when no subscript follows); the scripts
/// follow it as shifted boxes. A standalone \delimiter nucleus is a
/// character too.
#[test]
fn scripted_character_nucleus_is_not_boxed() {
    let log = run(
        r#"\mathcode`\)="5029 \mathcode`\f="7166 \mathcode`\2="7032
\setbox1\hbox{$)^a f^2 f_2 f_2^2 \delimiter"4162362 ^2$}\showbox1"#,
    );
    assert_displays(
        &log,
        &[r#"> \box1=
\hbox(10.07336+4.41544)x50.30688
.\mathon
.\tenrm )
.\hbox(4.30554+0.0)x5.28589, shifted -3.62892
..\teni a
.\teni f
.\kern1.0764
.\hbox(6.44444+0.0)x5.00002, shifted -3.62892
..\tenrm 2
.\teni f
.\hbox(6.44444+0.0)x5.00002, shifted 3.00002
..\tenrm 2
.\teni f
.\vbox(14.4888+0.0)x6.07642, shifted 4.41544
..\hbox(6.44444+0.0)x5.00002, shifted 1.0764
...\tenrm 2
..\kern1.59991
..\hbox(6.44444+0.0)x5.00002
...\tenrm 2
.\teni b
.\hbox(6.44444+0.0)x5.00002, shifted -3.62892
..\tenrm 2
.\mathoff
"#],
    );
}

/// tex.web §1045: math-mode `\ ` is append_normal_space — \spaceskip when
/// nonzero, else the current text font's space glue with its CURRENT
/// \fontdimen2-4 (IEEEtran retunes them), exactly as in horizontal mode.
#[test]
fn control_space_in_math_uses_spaceskip_and_current_fontdimens() {
    let log = run(
        r#"\mathcode`\f="7166
\fontdimen2\tenrm=5pt \fontdimen3\tenrm=2pt \fontdimen4\tenrm=1pt
\setbox1\hbox{$f\ f$}\showbox1
\spaceskip=4pt plus 1pt
\setbox1\hbox{$f\ f$ f\ f}\showbox1"#,
    );
    assert_displays(
        &log,
        &[
            r#"> \box1=
\hbox(6.94444+1.94444)x16.94452
.\mathon
.\teni f
.\kern1.0764
.\glue 5.0 plus 2.0 minus 1.0
.\teni f
.\kern1.0764
.\mathoff
"#,
            r#"> \box1=
\hbox(6.94444+1.94444)x30.05566
.\mathon
.\teni f
.\kern1.0764
.\glue(\spaceskip) 4.0 plus 1.0
.\teni f
.\kern1.0764
.\mathoff
.\glue(\spaceskip) 4.0 plus 1.0
.\tenrm f
.\glue(\spaceskip) 4.0 plus 1.0
.\tenrm f
"#,
        ],
    );
}

/// tex.web §752 make_ord: a `\right` delimiter that took a script still
/// closes its `\left` group, so later adjacent characters of one family
/// keep their lig/kern program (cmmi10 kerns `Y` against the comma).
#[test]
fn scripted_right_delimiter_keeps_later_math_kerns() {
    let log = run(
        r#"\mathcode`\,="613B \thinmuskip=3mu
\setbox1\hbox{$\left(x\right)^a Y,b$}\showbox1"#,
    );
    assert_displays(
        &log,
        &[r#"> \box1=
\hbox(7.94446+2.5)x35.54277
.\mathon
.\hbox(7.5+2.5)x13.49307
..\hbox(7.5+2.5)x3.8889
...\tenrm (
..\teni x
..\hbox(7.5+2.5)x3.8889
...\tenrm )
.\hbox(4.30554+0.0)x5.28589, shifted -3.63892
..\teni a
.\glue(\thinmuskip) 1.66663
.\teni Y
.\kern2.22223
.\kern-1.66667
.\teni ;
.\glue(\thinmuskip) 1.66663
.\teni b
.\mathoff
"#],
    );
}

/// tex.web make_radical (§737): the radicand is a clean_box (no
/// \binoppenalty inside it even in a paragraph), the overbar rule has
/// running width, and the surd + overbar pair is packed into one hbox.
#[test]
fn radical_is_one_box_with_a_clean_radicand() {
    let log = run(
        r#"\mathcode`\+="202B \medmuskip=4mu plus 2mu minus 4mu \binoppenalty=700 \parfillskip=0pt plus 1fil
\setbox1\vbox{\hsize=100pt \noindent$\radical"270370 {a+b}+a$\par}\showbox1"#,
    );
    assert_displays(
        &log,
        &[r#"> \box1=
\vbox(10.39996+0.0)x100.0
.\hbox(8.9055+1.49446)x100.0, glue set 52.35893fil
..\mathon
..\hbox(8.9055+1.49446)x30.13304
...\hbox(0.39998+9.6)x8.33336, shifted -8.10555
....\tensy p
...\vbox(8.9055+0.83333)x21.79968
....\kern0.39998
....\rule(0.39998+0.0)x*
....\kern1.1611
....\hbox(6.94444+0.83333)x21.79968
.....\teni a
.....\glue(\medmuskip) 2.22217 plus 1.11108 minus 2.22217
.....\tenrm +
.....\glue(\medmuskip) 2.22217 plus 1.11108 minus 2.22217
.....\teni b
..\glue(\medmuskip) 2.22217 plus 1.11108 minus 2.22217
..\tenrm +
..\penalty 700
..\glue(\medmuskip) 2.22217 plus 1.11108 minus 2.22217
..\teni a
..\mathoff
"#],
    );
}
