//! LuaTeX callbacks of the alignment, page output and error reporting paths.
//! Expected values are what `luatex --ini` (LuaTeX 1.24.0, TeX Live 2026)
//! prints for the same input (texio.write_nl lines of the callbacks).

use tex_core::engine::{Engine, EngineKind};

fn run(body: &str) -> Engine {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    for (c, v) in [(b'{', 1), (b'}', 2), (b'#', 6), (b'&', 4), (b' ', 10), (b'\n', 5), (b'\r', 5)] {
        e.eqtb.cat[c as usize] = v;
    }
    e.input.push_file(
        "t.tex".into(),
        format!("\\directlua{{tex.enableprimitives('',tex.extraprimitives('luatex'))}}\n{body}\n\\end\n").into_bytes(),
    );
    e.run();
    e
}

/// The callback lines (those starting with one of `tags`) of the terminal.
fn lines(e: &Engine, tags: &[&str]) -> Vec<String> {
    e.term.split_whitespace().filter(|l| tags.iter().any(|t| l.starts_with(t))).map(str::to_string).collect()
}

/// `fin_row`: every finished `\halign` row runs `hpack_filter` (group
/// `fin_row`) and `append_to_vlist_filter` (location `alignment`).
#[test]
fn halign_rows_run_fin_row_callbacks() {
    let e = run(r#"\directlua{
callback.register('hpack_filter', function(h, g) texio.write_nl('H:'..g) return true end)
callback.register('append_to_vlist_filter', function(b, loc, pd, m) texio.write_nl('A:'..loc..':'..pd) return b end)
}
\halign{#\hfil&\hfil#\cr a&b\cr c&d\cr}"#);
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    assert_eq!(
        lines(&e, &["H:", "A:"]),
        [
            "H:align_set", "H:align_set", "H:fin_row", "A:alignment:-65536000",
            "H:align_set", "H:align_set", "H:fin_row", "A:alignment:-65536000",
        ]
    );
}

/// An `append_to_vlist_filter` that returns nil appends nothing: the rows of
/// the alignment are gone and `prev_depth` stays what it was.
#[test]
fn halign_rows_dropped_by_append_to_vlist_filter() {
    let e = run(r#"\directlua{callback.register('append_to_vlist_filter', function(b,loc,pd) texio.write_nl('A:'..loc..':'..pd) return nil end)}
\setbox1\vbox{\halign{#\cr a\cr b\cr}\hrule height 1pt}
\message{HT=\the\ht1}"#);
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    assert_eq!(lines(&e, &["A:"]), ["A:alignment:-65536000", "A:alignment:-65536000"]);
    assert!(e.term.contains("HT=1.0pt"), "{}", e.term);
}

/// `pre_output_filter` sees the page before `\box255` is made; `page_order_index`
/// sorts the pages of the page tree.
#[test]
fn output_filters_and_page_order() {
    let e = run(r#"\outputmode=1 \pdfvariable compresslevel=0 \pdfvariable objcompresslevel=0
\directlua{callback.register('pre_output_filter', function(h,g,s,pt) texio.write_nl('P:'..g..':'..s..':'..pt) return true end)
 callback.register('page_order_index', function(n) texio.write_nl('O:'..n) return 3-n end)}
\vsize=100pt \output={\shipout\box255}
\hbox{\vrule width 11bp height 5bp}\penalty-10000
\hbox{\vrule width 22bp height 5bp}\penalty-10000"#);
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    // the callback lines share a terminal line with the page numbers
    let t = e.term.replace('\n', " ");
    let seen: Vec<&str> = t
        .split(|c: char| c == ' ' || c == '[' || c == ']')
        .filter(|w| w.starts_with("P:") || w.starts_with("O:"))
        .collect();
    assert_eq!(seen, ["P:output:6553600:exactly", "O:1", "P:output:6553600:exactly", "O:2"]);
    let bytes = tex_core::pdffile::write_pdf(&e.pdf_doc).expect("valid PDF");
    let doc = lopdf::Document::load_mem(&bytes).expect("valid PDF");
    let pages = doc.get_pages();
    let content = |n: u32| String::from_utf8_lossy(&doc.get_page_content(pages[&n])).into_owned();
    // page 2 (location 1) comes first
    assert!(content(1).contains("22"), "{}", content(1));
    assert!(content(2).contains("11"), "{}", content(2));
}

/// `show_error_message`, `show_error_hook` and `show_lua_error_hook` replace
/// the message and context of an error; `status` holds the strings.
#[test]
fn error_callbacks_replace_message_and_context() {
    let e = run(r#"\scrollmode\directlua{
callback.register('show_error_message', function() texio.write_nl('EM['..tostring(status.lasterrorstring):match('^! Undefined control sequence')..']') end)
callback.register('show_error_hook', function() texio.write_nl('EH['..tostring(status.lasterrorcontext)..']') end)
callback.register('show_lua_error_hook', function() texio.write_nl('LE['..(tostring(status.lastluaerrorstring):gsub(string.char(10)..'.*',''))..']') end)
}
\undefinedcs
\directlua{error('boom')}"#);
    let t = e.term.clone();
    assert!(t.contains("EM[! Undefined control sequence]"), "{t}");
    assert!(t.contains("EH[\nl.7 \\undefinedcs\n               ]"), "{t}");
    assert!(t.contains("LE[[\\directlua]:1: boom]"), "{t}");
}
