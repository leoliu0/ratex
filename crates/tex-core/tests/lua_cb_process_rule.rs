//! The `process_rule` callback and rule output of the LuaTeX backend
//! (pdfshipout.c `rule_callback_id`, pdflistout.c `hlist_out`/`vlist_out`,
//! pdfrule.c `pdf_place_rule`).
//!
//! Every `lua_cb_process_rule/<name>.tex` was run with TeX Live 2026
//! `luatex -ini -interaction=nonstopmode -jobname=job <name>.tex` and the
//! `<name>.expected` file written from the result: the page content streams
//! of the PDF (`## page` sections; no fonts, so no other stream) and the `PR ...` lines the callbacks
//! logged (`## log`). The engine must produce the same streams and lines.
//! `rules_math` sets a formula with `plain.tex` fonts, whose text operators
//! are not LuaTeX's yet, so only the `q ... Q` rule blocks are compared.

use tex_core::engine::{Engine, EngineKind};

/// The tests run in parallel inside one process: pin what would make two
/// PDFs differ (clock) or trip the memory cap, and keep the lookup hermetic
/// so that `\input plain` is the embedded one.
fn pin_environment() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var("TEX_MEM_LIMIT_MIB", "0");
        std::env::set_var("TEX_RS_HERMETIC", "1");
        std::env::set_var("FORCE_SOURCE_DATE", "1");
        std::env::set_var("SOURCE_DATE_EPOCH", "1700000000");
    });
}

struct Output {
    pages: Vec<String>,
    log: Vec<String>,
}

/// Run `source` and collect what the expectation files hold: the content
/// stream of every page and the lines the callbacks logged.
fn run(source: &str) -> Output {
    pin_environment();
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.add_nullfont();
    for (c, cat) in [(b'{', 1), (b'}', 2), (b'#', 6), (b' ', 10), (b'\n', 5)] {
        e.eqtb.cat[c as usize] = cat;
    }
    e.set_interaction_mode(tex_core::engine::InteractionMode::Nonstop);
    e.job_name = "job".to_string();
    e.input.push_file("t.tex".to_string(), source.as_bytes().to_vec());
    e.run();
    e.finish_job_diagnostics();
    assert_eq!(e.error_count, 0, "{:?}\n{}", e.diagnostics, e.term);
    let pages = e.pdf_doc.pages.iter().map(|p| p.content.iter().map(|&b| b as char).collect()).collect();
    // terminal lines wrap at 79 columns, a logged line ends with `@@`
    let flat = e.term.replace('\n', "");
    let log = flat
        .split("@@")
        .filter_map(|chunk| chunk.rfind("PR ").map(|i| chunk[i..].to_string()))
        .collect();
    Output { pages, log }
}

fn expected(text: &str) -> Output {
    let (pages, log) = text.split_once("## log\n").expect("log section");
    Output {
        pages: pages
            .split("## page\n")
            .skip(1)
            .map(|p| p.trim_end().to_string())
            .collect(),
        log: log.lines().map(str::to_string).collect(),
    }
}

/// The `q ... Q` blocks of a content stream, one line per block.
fn blocks(stream: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut open: Option<Vec<&str>> = None;
    for line in stream.lines() {
        match (line, &mut open) {
            ("q", None) => open = Some(Vec::new()),
            ("Q", Some(lines)) => {
                out.push(lines.join(" | "));
                open = None;
            }
            (l, Some(lines)) => lines.push(l),
            _ => {}
        }
    }
    out
}

fn check(source: &str, want: &str, rule_blocks_only: bool) {
    let (got, want) = (run(source), expected(want));
    assert_eq!(got.log, want.log, "callback log");
    assert_eq!(got.pages.len(), want.pages.len(), "pages");
    for (i, (g, w)) in got.pages.iter().zip(&want.pages).enumerate() {
        if rule_blocks_only {
            assert_eq!(blocks(g), blocks(w), "page {}", i + 1);
        } else {
            assert_eq!(g.trim_end(), w, "page {}", i + 1);
        }
    }
}

/// Normal rules (filled, hairlines stroked as `[] 0 d 0 J ... S`, `\nohrule`,
/// rule leaders, outline rules) with a `process_rule` callback registered:
/// it is not asked for them. A small user rule is.
#[test]
fn plain_rules_are_drawn_by_the_backend() {
    check(
        include_str!("lua_cb_process_rule/rules_plain.tex"),
        include_str!("lua_cb_process_rule/rules_plain.expected"),
        false,
    );
}

/// User rules and, with the callback, the math rules go to `process_rule`
/// as `(node, width, height)` in the orientation of the list, with running
/// dimensions resolved, in shifted boxes, as leaders; what the callback
/// `pdf.print`s lands between `q` and `Q` at the rule's lower left corner.
/// Without a callback (or one registered as `false`) they draw nothing / are
/// normal rules.
#[test]
fn user_and_math_rules_run_the_callback() {
    check(
        include_str!("lua_cb_process_rule/rules_user.tex"),
        include_str!("lua_cb_process_rule/rules_user.expected"),
        false,
    );
}

/// The literal modes of `pdf.print` inside the callback.
#[test]
fn pdf_print_modes_in_the_callback() {
    check(
        include_str!("lua_cb_process_rule/rules_modes.tex"),
        include_str!("lua_cb_process_rule/rules_modes.expected"),
        false,
    );
}

/// `\mathrulesmode=1` marks the fraction, radical, `\overline` and `\underline` rules.
#[test]
fn math_rules_run_the_callback() {
    check(
        include_str!("lua_cb_process_rule/rules_math.tex"),
        include_str!("lua_cb_process_rule/rules_math.expected"),
        true,
    );
}
