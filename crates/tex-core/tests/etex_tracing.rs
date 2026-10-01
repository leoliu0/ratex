//! e-TeX tracing and group behaviour; expected transcripts come from
//! `pdftex -ini -etex` (TeX Live 2026) on the same input.

use tex_core::engine::{Engine, InteractionMode};

fn run(src: &str) -> Engine {
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.set_interaction_mode(InteractionMode::Nonstop);
    let mut text = String::from("\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\catcode`\\&=4\n");
    text.push_str(src);
    engine.input.push_file("t.tex".into(), text.into_bytes());
    engine.run();
    engine
}

/// The `{...}` trace lines of a transcript.
fn trace_lines(log: &str) -> Vec<&str> {
    log.lines().filter(|line| line.starts_with('{')).collect()
}

/// TeX compares glue by spec pointer: copies of one spec are reassignments,
/// equal values scanned separately, arithmetic results and negations are
/// new specs, and every all-zero spec is the shared `zero_glue`.
#[test]
fn glue_assignments_reassign_only_the_same_spec() {
    let e = run(concat!(
        "\\tracingassigns=1 \\tracingonline=1\n",
        "\\skip4=1pt plus 1fil \\skip5=\\skip4 \\skip4=\\skip5 \\skip5=\\skip4 ",
        "\\skip4=1pt plus 1fil\n",
        "\\skip6=0pt plus 0fil \\skip6=0pt \\skip7=\\skip4 \\advance\\skip7 by 0pt ",
        "\\skip7=\\skip7 \\skip7=-\\skip7\n",
        "\\message{.}\\end\n"
    ));
    let lines = trace_lines(&e.log);
    let expected = [
        "{into \\tracingassigns=1}",
        "{changing \\tracingonline=0}",
        "{into \\tracingonline=1}",
        "{changing \\skip4=0.0pt}",
        "{into \\skip4=1.0pt plus 1.0fil}",
        "{changing \\skip5=0.0pt}",
        "{into \\skip5=1.0pt plus 1.0fil}",
        "{reassigning \\skip4=1.0pt plus 1.0fil}",
        "{reassigning \\skip5=1.0pt plus 1.0fil}",
        "{changing \\skip4=1.0pt plus 1.0fil}",
        "{into \\skip4=1.0pt plus 1.0fil}",
        "{reassigning \\skip6=0.0pt}",
        "{reassigning \\skip6=0.0pt}",
        "{changing \\skip7=0.0pt}",
        "{into \\skip7=1.0pt plus 1.0fil}",
        "{changing \\skip7=1.0pt plus 1.0fil}",
        "{into \\skip7=1.0pt plus 1.0fil}",
        "{reassigning \\skip7=1.0pt plus 1.0fil}",
        "{changing \\skip7=1.0pt plus 1.0fil}",
        "{into \\skip7=-1.0pt plus -1.0fil}",
    ];
    assert_eq!(&lines[..expected.len()], &expected, "{}", e.log);
}

/// A preamble `\tabskip` belongs to the alignment's group (tex.web
/// scan_spec opens it first): it is restored when the alignment ends, and
/// the alignment has the two align groups fin_align unsaves.
#[test]
fn preamble_tabskip_is_local_to_the_alignment() {
    let e = run(concat!(
        "\\tabskip=1pt \\tracingrestores=1 \\tracinggroups=1 \\tracingonline=1\n",
        "\\halign{#\\tabskip=7pt&#\\cr a&b\\cr}\n",
        "\\message{[\\the\\tabskip]}\\end\n"
    ));
    let lines = trace_lines(&e.log);
    let expected = [
        "{entering align group (level 1) at line 3}",
        "{entering align group (level 2) at line 3}",
        "{leaving align group (level 2) entered at line 3}",
        "{entering align group (level 2) at line 3}",
        "{leaving align group (level 2) entered at line 3}",
        "{entering align group (level 2) at line 3}",
        "{leaving align group (level 2) entered at line 3}",
        "{restoring \\tabskip=1.0pt}",
        "{leaving align group (level 1) entered at line 3}",
    ];
    assert!(lines.ends_with(&expected) || lines.windows(expected.len()).any(|w| w == expected), "{}", e.log);
    assert!(e.term.contains("[1.0pt]"), "{}", e.term);
}

/// A `\fi` or `\else` that ends an unfinished condition's operand is traced
/// when it ends the operand and again when the condition skips to it.
#[test]
fn delimiter_ending_an_operand_is_traced_twice() {
    let e = run(concat!(
        "\\tracingifs=1 \\tracingonline=1\n",
        "\\ifnum 1=2\\else\\fi\n",
        "\\ifcase 1\\fi\n",
        "\\end\n"
    ));
    let lines = trace_lines(&e.log);
    let expected = [
        "{vertical mode: \\ifnum: (level 1) entered on line 3}",
        "{\\else: \\ifnum (level 1) entered on line 3}",
        "{\\else: \\ifnum (level 1) entered on line 3}",
        "{\\fi: \\ifnum (level 1) entered on line 3}",
        "{\\ifcase: (level 1) entered on line 4}",
        "{\\fi: \\ifcase (level 1) entered on line 4}",
        "{\\fi: \\ifcase (level 1) entered on line 4}",
    ];
    assert_eq!(lines, expected, "{}", e.log);
}

/// `\endgroup` in a simple group is off_save: `}` is inserted, the group
/// closes and the `\endgroup` is read again at the bottom level.
#[test]
fn endgroup_in_a_simple_group_inserts_a_brace() {
    let e = run(concat!(
        "\\tracinggroups=1 \\tracingonline=1\n",
        "{\\endgroup \\message{[\\the\\currentgrouplevel]}\\end\n"
    ));
    let lines = trace_lines(&e.log);
    assert_eq!(
        lines,
        [
            "{entering simple group (level 1) at line 3}",
            "{leaving simple group (level 1) entered at line 3}"
        ],
        "{}",
        e.log
    );
    assert!(e.log.contains("Missing } inserted"), "{}", e.log);
    assert!(e.log.contains("Extra \\endgroup"), "{}", e.log);
    assert!(e.term.contains("[0]"), "{}", e.term);
}

/// show_eqtb displays a box register with `depth_threshold=0` and
/// `breadth_max=1` using the escape character of the moment of the event.
#[test]
fn box_assignment_trace_uses_the_escape_character_of_the_event() {
    let e = run(concat!(
        "\\tracingassigns=1 \\tracingonline=1 \\setbox3\\hbox{} \\setbox300\\hbox{}\n",
        "\\escapechar=`! \\message{.}\\end\n"
    ));
    let lines: Vec<&str> = e.log.lines().collect();
    assert!(lines.contains(&"\\hbox(0.0+0.0)x0.0}"), "{}", e.log);
    assert!(!e.log.contains("!hbox(0.0+0.0)x0.0}\n{changing !box"), "{}", e.log);
}

/// `\unless` expands the conditional it prefixes in the same step, so an
/// `\expandafter` over `\unless\if...` sees the branch it selected (expl3's
/// `\str_tail:n` relies on this).
#[test]
fn expandafter_sees_through_unless() {
    let e = run(concat!(
        "\\def\\a#1{[#1]}\n",
        "\\message{\\expandafter\\a\\unless\\ifx ab X\\fi}\n",
        "\\let\\myif=\\ifnum \\message{\\expandafter\\a\\unless\\myif1=2 Y\\fi}\\end\n"
    ));
    assert!(e.term.contains("[X]"), "{}", e.term);
    assert!(e.term.contains("[Y]"), "{}", e.term);
    assert!(!e.log.contains("Extra \\fi"), "{}", e.log);
}

/// A trace line ends its line like `end_diagnostic(false)`, so a following
/// `\message` continues on a fresh line without a separating space, and
/// consecutive messages are separated by one.
#[test]
fn message_after_trace_line_starts_at_column_zero() {
    let e = run(concat!(
        "\\tracinggroups=1 \\tracingonline=1\n",
        "\\message{x}{\\message{a}\\message{b}}\\message{c}\\end\n"
    ));
    assert!(
        e.log
            .lines()
            .any(|l| l == "a b{leaving simple group (level 1) entered at line 3}"),
        "{}",
        e.log
    );
    assert!(e.log.lines().any(|l| l == "c"), "{}", e.log);
}

/// tex.web `\vskip` in restricted horizontal mode is `off_save`: the box is
/// closed ("Missing } inserted") before the skip is read again, and a
/// math-only command in a box opens a formula ("Missing $ inserted") whose
/// groups nest inside the box group.
#[test]
fn vertical_and_math_commands_in_a_box_recover_like_tex() {
    let e = run(concat!(
        "\\tracinggroups=1 \\tracingonline=1\n",
        "\\setbox1\\hbox{a\\vskip 3pt b}\n",
        "\\setbox1\\hbox{\\mathchoice{}{}{}{}}\n",
        "\\end\n"
    ));
    let lines = trace_lines(&e.log);
    let expected = [
        "{entering hbox group (level 1) at line 3}",
        "{leaving hbox group (level 1) entered at line 3}",
        "{entering hbox group (level 1) at line 4}",
        "{entering math shift group (level 2) at line 4}",
        "{entering math choice group (level 3) at line 4}",
        "{leaving math choice group (level 3) entered at line 4}",
        "{entering math choice group (level 3) at line 4}",
        "{leaving math choice group (level 3) entered at line 4}",
        "{entering math choice group (level 3) at line 4}",
        "{leaving math choice group (level 3) entered at line 4}",
        "{entering math choice group (level 3) at line 4}",
        "{leaving math choice group (level 3) entered at line 4}",
    ];
    assert_eq!(&lines[..expected.len()], &expected, "{}", e.log);
    assert!(e.log.contains("Missing } inserted"), "{}", e.log);
    assert!(e.log.contains("Missing $ inserted"), "{}", e.log);
}

/// The read-only quantities (`\badness`, `\inputlineno`, `\lastnodetype`,
/// `\lastkern`, ...) are not assignments, `\prevdepth` belongs to vertical
/// modes: each is an illegal case that consumes only the command.
#[test]
fn read_only_quantities_and_aux_values_are_illegal_commands() {
    let e = run(concat!(
        "\\scrollmode\n",
        "\\setbox1\\hbox{a\\lastnodetype=0 \\badness=7 \\inputlineno=1 \\lastkern}\n",
        "\\setbox1\\vbox{a\\prevdepth=3pt}\n",
        "\\setbox1\\hbox{\\spacefactor=5 }\n",
        "\\end\n"
    ));
    let messages: Vec<&str> = e.diagnostics.iter().map(|d| d.message.as_str()).collect();
    let illegal: Vec<&&str> = messages
        .iter()
        .filter(|m| m.starts_with("You can't use"))
        .collect();
    let expected = [
        "You can't use `\\lastnodetype' in restricted horizontal mode",
        "You can't use `\\badness' in restricted horizontal mode",
        "You can't use `\\inputlineno' in restricted horizontal mode",
        "You can't use `\\lastkern' in restricted horizontal mode",
        "You can't use `\\prevdepth' in horizontal mode",
    ];
    assert_eq!(illegal.len(), expected.len(), "{messages:?}");
    for (got, want) in illegal.iter().zip(expected) {
        assert!(got.starts_with(want), "{got} vs {want}");
    }
}

/// tex.web §1121: the part of a discretionary list that is dropped is
/// displayed after the error.
#[test]
fn improper_discretionary_lists_show_the_deleted_sublist() {
    let e = run(concat!(
        "\\scrollmode\n",
        "\\setbox1\\hbox{\\discretionary{\\hskip3pt\\kern2pt}{\\penalty5}{}}\n",
        "\\end\n"
    ));
    assert!(
        e.log.contains(
            "The following discretionary sublist has been deleted:\n\\glue 3.0\n\\kern 2.0\n"
        ),
        "{}",
        e.log
    );
    assert!(
        e.log.contains(
            "The following discretionary sublist has been deleted:\n\\penalty 5\n"
        ),
        "{}",
        e.log
    );
}

/// Every `{entering ...}` / `{leaving ...}` event of a transcript, also when
/// it follows other text on its line.
fn group_events(log: &str) -> Vec<&str> {
    let mut events = Vec::new();
    let mut rest = log;
    while let Some(i) = rest.find("{entering ").into_iter().chain(rest.find("{leaving ")).min() {
        let tail = &rest[i..];
        let end = tail.find('}').map_or(tail.len(), |e| e + 1);
        events.push(&tail[..end]);
        rest = &tail[end..];
    }
    events
}

/// tex.web §1192 math_left_right: `\right` and `\middle` outside a
/// `\left...\right` group are "Extra" commands that leave the math shift
/// group open; inside a `{...}` group they close that group first
/// (off_save), and nothing closes the formula.
#[test]
fn stray_right_and_middle_are_extra_and_keep_the_formula_open() {
    let e = run(concat!(
        "\\scrollmode\\catcode`\\$=3 \\tracinggroups=1 \\tracingonline=1\n",
        "\\setbox1\\hbox{$x\\right)\\middle(y$}\n",
        "\\setbox1\\hbox{${\\right)}$}\n",
        "\\message{[\\the\\currentgrouplevel]}\n",
        "\\end\n"
    ));
    let expected = [
        "{entering hbox group (level 1) at line 3}",
        "{entering math shift group (level 2) at line 3}",
        "{leaving math shift group (level 2) entered at line 3}",
        "{leaving hbox group (level 1) entered at line 3}",
        "{entering hbox group (level 1) at line 4}",
        "{entering math shift group (level 2) at line 4}",
        "{entering math group (level 3) at line 4}",
        "{leaving math group (level 3) entered at line 4}",
        "{leaving math shift group (level 2) entered at line 4}",
        "{leaving hbox group (level 1) entered at line 4}",
    ];
    assert_eq!(group_events(&e.log), expected, "{}", e.log);
    assert_eq!(e.log.matches("Extra \\right").count(), 2, "{}", e.log);
    assert!(e.log.contains("Extra \\middle"), "{}", e.log);
    assert!(e.log.contains("Missing } inserted"), "{}", e.log);
    assert!(e.log.contains("[0]"), "{}", e.log);
}

/// tex.web build_choices: every `\mathchoice` part is a math choice group
/// pushed before `scan_left_brace` reads its `{`, so a part whose brace is
/// on the next line is entered on the line of the previous part. A part that
/// does not start with `{` gets "Missing { inserted" and still opens its
/// group, and an `\endgroup` inside a part closes that part first.
#[test]
fn mathchoice_parts_open_their_group_before_reading_the_brace() {
    let e = run(concat!(
        "\\scrollmode\\catcode`\\$=3 \\tracinggroups=1 \\tracingonline=1\n",
        "\\setbox1\\hbox{$\\mathchoice{a}{b}{c}\n",
        "{d}$}\n",
        "\\setbox1\\hbox{$\\mathchoice{a}{b}{c}\\relax\n",
        "d}$}\n",
        "\\message{[\\the\\currentgrouplevel]}\n",
        "\\end\n"
    ));
    let mut expected = vec![
        "{entering hbox group (level 1) at line 3}".to_string(),
        "{entering math shift group (level 2) at line 3}".to_string(),
    ];
    for _ in 0..4 {
        expected.push("{entering math choice group (level 3) at line 3}".into());
        expected.push("{leaving math choice group (level 3) entered at line 3}".into());
    }
    expected.push("{leaving math shift group (level 2) entered at line 3}".into());
    expected.push("{leaving hbox group (level 1) entered at line 3}".into());
    expected.push("{entering hbox group (level 1) at line 5}".into());
    expected.push("{entering math shift group (level 2) at line 5}".into());
    for _ in 0..4 {
        expected.push("{entering math choice group (level 3) at line 5}".into());
        expected.push("{leaving math choice group (level 3) entered at line 5}".into());
    }
    expected.push("{leaving math shift group (level 2) entered at line 5}".into());
    expected.push("{leaving hbox group (level 1) entered at line 5}".into());
    assert_eq!(group_events(&e.log), expected, "{}", e.log);
    assert_eq!(e.log.matches("Missing { inserted").count(), 1, "{}", e.log);

    let e = run(concat!(
        "\\scrollmode\\catcode`\\$=3 \\tracinggroups=1 \\tracingonline=1\n",
        "\\setbox1\\hbox{$\\mathchoice{a}{b\\endgroup}{c}{d}$}\n",
        "\\end\n"
    ));
    let events = group_events(&e.log);
    assert_eq!(
        &events[..12],
        [
            "{entering hbox group (level 1) at line 3}",
            "{entering math shift group (level 2) at line 3}",
            "{entering math choice group (level 3) at line 3}",
            "{leaving math choice group (level 3) entered at line 3}",
            "{entering math choice group (level 3) at line 3}",
            "{leaving math choice group (level 3) entered at line 3}",
            "{entering math choice group (level 3) at line 3}",
            "{leaving math choice group (level 3) entered at line 3}",
            "{entering math choice group (level 3) at line 3}",
            "{leaving math choice group (level 3) entered at line 3}",
            "{leaving math shift group (level 2) entered at line 3}",
            "{leaving hbox group (level 1) entered at line 3}",
        ],
        "{}",
        e.log
    );
    assert!(e.log.contains("Extra \\endgroup"), "{}", e.log);
}

/// tex.web start_eq_no and math_left_right: the tag of `\eqno` is typeset in
/// a math shift group of its own (nested in the display's, undoing its
/// assignments), `\middle` ends the `\left` group and begins another, and
/// neither `\eqno` nor `\halign` is legal inside the tag or a brace group.
#[test]
fn eqno_and_middle_have_their_own_groups() {
    let e = run(concat!(
        "\\scrollmode\\catcode`\\$=3 \\tracinggroups=1 \\tracingonline=1\n",
        "\\count1=1 \\hsize=100pt\n",
        "\\setbox1\\vbox{\\noindent$$ x\\eqno \\count1=2 \\showgroups y $$ \\par}\n",
        "\\message{[\\the\\count1]}\n",
        "\\setbox1\\hbox{$\\left. \\count1=5 x\\middle. \\showgroups y\\middle. \\message{[\\the\\count1]} \\right.$}\n",
        "\\message{[\\the\\count1]}\n",
        "\\setbox1\\vbox{\\noindent$$ x\\eqno a\\eqno b $$\\par}\n",
        "\\setbox1\\vbox{\\noindent$$ x\\leqno a\\halign{#\\cr}$$\\par}\n",
        "\\setbox1\\vbox{\\noindent$$ x{\\eqno a}$$\\par}\n",
        "\\end\n"
    ));
    let expected = [
        "{entering vbox group (level 1) at line 4}",
        "{entering math shift group (level 2) at line 4}",
        "{entering math shift group (level 3) at line 4}",
        "{leaving math shift group (level 3) entered at line 4}",
        "{leaving math shift group (level 2) entered at line 4}",
        "{leaving vbox group (level 1) entered at line 4}",
        "{entering hbox group (level 1) at line 6}",
        "{entering math shift group (level 2) at line 6}",
        "{entering math left group (level 3) at line 6}",
        "{leaving math left group (level 3) entered at line 6}",
        "{entering math left group (level 3) at line 6}",
        "{leaving math left group (level 3) entered at line 6}",
        "{entering math left group (level 3) at line 6}",
        "{leaving math left group (level 3) entered at line 6}",
        "{leaving math shift group (level 2) entered at line 6}",
        "{leaving hbox group (level 1) entered at line 6}",
        "{entering vbox group (level 1) at line 8}",
        "{entering math shift group (level 2) at line 8}",
        "{entering math shift group (level 3) at line 8}",
        "{leaving math shift group (level 3) entered at line 8}",
        "{leaving math shift group (level 2) entered at line 8}",
        "{leaving vbox group (level 1) entered at line 8}",
        "{entering vbox group (level 1) at line 9}",
        "{entering math shift group (level 2) at line 9}",
        "{entering math shift group (level 3) at line 9}",
        "{entering math group (level 4) at line 9}",
        "{leaving math group (level 4) entered at line 9}",
        "{leaving math shift group (level 3) entered at line 9}",
        "{leaving math shift group (level 2) entered at line 9}",
        "{leaving vbox group (level 1) entered at line 9}",
        "{entering vbox group (level 1) at line 10}",
        "{entering math shift group (level 2) at line 10}",
        "{entering math group (level 3) at line 10}",
        "{leaving math group (level 3) entered at line 10}",
        "{leaving math shift group (level 2) entered at line 10}",
        "{leaving vbox group (level 1) entered at line 10}",
    ];
    assert_eq!(group_events(&e.log), expected, "{}", e.log);
    // \showgroups: the tag is the \eqno group of the display, the second
    // segment of the \left group is the \middle one
    assert!(
        e.log.contains(concat!(
            "### math shift group (level 3) entered at line 4 (\\eqno)\n",
            "### math shift group (level 2) entered at line 4 ($$)\n",
            "### vbox group (level 1) entered at line 4 (\\setbox1=\\vbox{)\n"
        )),
        "{}",
        e.log
    );
    assert!(
        e.log.contains("### math left group (level 3) entered at line 6 (\\middle)\n"),
        "{}",
        e.log
    );
    // the tag's and the \left segments' assignments are local to them
    assert_eq!(e.log.matches("[1]").count(), 3, "{}", e.log);
    assert!(!e.log.contains("[2]") && !e.log.contains("[5]"), "{}", e.log);
    assert_eq!(e.log.matches("You can't use `\\eqno' in math mode").count(), 2, "{}", e.log);
    assert!(e.log.contains("You can't use `\\halign' in math mode"), "{}", e.log);
}

/// tex.web head_for_vmode (hmode+stop): `\end` and `\dump` inside an `\hbox`
/// close the box ("Missing } inserted") before the job ends.
#[test]
fn end_inside_a_box_closes_the_box_first() {
    let e = run(concat!(
        "\\scrollmode\\tracinggroups=1 \\tracingonline=1\n",
        "\\setbox1\\hbox{a\\end}\n",
        "\\message{[\\the\\currentgrouplevel]}\n",
        "\\end\n"
    ));
    assert_eq!(
        group_events(&e.log),
        [
            "{entering hbox group (level 1) at line 3}",
            "{leaving hbox group (level 1) entered at line 3}",
        ],
        "{}",
        e.log
    );
    assert!(e.log.contains("Missing } inserted"), "{}", e.log);
}

/// pdftex.web `\vadjust pre`: the material goes in front of the line (or
/// display, or alignment row) that holds it, `\vadjust` stays after it; the
/// group is printed as `\insert1`, the node as `\vadjust pre`, and `\vadjust`
/// is illegal in vertical modes.
#[test]
fn vadjust_pre_migrates_in_front_of_its_line() {
    let e = run(concat!(
        "\\scrollmode\\tracinggroups=1 \\tracingonline=1\n",
        "\\hsize=100pt \\parindent=0pt \\baselineskip=12pt \\parfillskip=0pt \\showboxbreadth=100 \\showboxdepth=100\n",
        "\\setbox1\\vbox{\\noindent\\vrule width 1pt height 1pt\\vadjust pre{\\kern1pt}",
        "\\vrule width 2pt height 2pt\\vadjust{\\kern2pt}",
        "\\vrule width 3pt height 3pt\\vadjust pre{\\kern3pt}\\par}\n",
        "\\showbox1\n",
        "\\setbox2\\hbox{\\vrule\\vadjust pre{\\showgroups}\\vrule\\vadjust{\\showgroups}\\showlists}\n",
        "\\showbox2\n",
        "\\setbox4\\vbox{\\halign{#\\vadjust pre{\\kern5pt}\\cr \\vrule width 1pt height 1pt\\cr",
        "\\vrule width 2pt height 2pt\\vadjust{\\kern6pt}\\cr}}\n",
        "\\showbox4\n",
        "\\setbox3\\vbox{\\vadjust pre{\\kern1pt}}\n",
        "\\end\n"
    ));
    let log = &e.log;
    // the paragraph: both pre kerns (in source order) precede the line
    assert!(
        log.contains(concat!(
            "> \\box1=\n\\vbox(9.0+0.0)x100.0\n.\\kern 1.0\n.\\kern 3.0\n.\\hbox(3.0+0.0)x100.0\n",
            "..\\rule(1.0+*)x1.0\n..\\rule(2.0+*)x2.0\n..\\rule(3.0+*)x3.0\n..\\penalty 10000\n",
            "..\\glue(\\parfillskip) 0.0\n..\\glue(\\rightskip) 0.0\n.\\kern 2.0\n"
        )),
        "{log}"
    );
    // inside an hbox the nodes stay, and the group shows the pre flag
    assert!(log.contains("### insert group (level 2) entered at line 6 (\\insert1{)"), "{log}");
    assert!(log.contains("### insert group (level 2) entered at line 6 (\\insert0{)"), "{log}");
    assert!(
        log.contains("\\rule(*+*)x0.4\n\\vadjust pre \n\\rule(*+*)x0.4\n\\vadjust\nspacefactor 1000"),
        "{log}"
    );
    // each alignment row gets its pre material in front of its interline glue
    assert!(
        log.contains(concat!(
            "> \\box4=\n\\vbox(29.0+0.0)x2.0\n.\\kern 5.0\n.\\hbox(1.0+0.0)x2.0\n",
            "..\\glue(\\tabskip) 0.0\n..\\hbox(1.0+0.0)x2.0\n...\\rule(1.0+*)x1.0\n",
            "..\\glue(\\tabskip) 0.0\n.\\kern 5.0\n.\\glue(\\baselineskip) 10.0\n",
            ".\\hbox(2.0+0.0)x2.0\n..\\glue(\\tabskip) 0.0\n..\\hbox(2.0+0.0)x2.0\n",
            "...\\rule(2.0+*)x2.0\n..\\glue(\\tabskip) 0.0\n.\\kern 6.0\n"
        )),
        "{log}"
    );
    assert!(log.contains("You can't use `\\vadjust' in internal vertical mode"), "{log}");
}
