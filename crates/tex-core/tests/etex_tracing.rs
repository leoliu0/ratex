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
