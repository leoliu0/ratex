//! LuaTeX 1.24 primitive model. Expected values come from
//! `luatex --ini` (TeX Live 2026) running the same input.

use tex_core::engine::{Engine, EngineKind};

fn boot(kind: EngineKind) -> Engine {
    let mut e = Engine::new_with_kind(kind, true);
    e.init_primitives();
    for (c, cat) in [(b'{', 1), (b'}', 2), (b'#', 6), (b' ', 10), (b'\n', 5)] {
        e.eqtb.cat[c as usize] = cat;
    }
    e
}

fn run(e: &mut Engine, src: &str) {
    e.input.push_file("t.tex".to_string(), src.as_bytes().to_vec());
    e.run();
}

/// Run `body` after enabling every LuaTeX primitive and return the
/// terminal output.
fn run_enabled(body: &str) -> Engine {
    let mut e = boot(EngineKind::LuaTeX);
    run(&mut e, &format!("\\directlua{{tex.enableprimitives('',tex.extraprimitives())}}\n{body}\n\\end\n"));
    e
}

fn defined(e: &Engine, name: &str) -> bool {
    e.cs.lookup(name.as_bytes()).is_some_and(|id| e.eqtb.get(id).is_some())
}

#[test]
fn ini_defines_the_tex_group_and_directlua_only() {
    let mut e = boot(EngineKind::LuaTeX);
    // luatex --ini: tex group (340 names) plus \directlua.
    for name in ["glet", "hpack", "boundary", "Uleft", "exhyphenchar", "-", "relax", "directlua"] {
        assert!(defined(&e, name), "\\{name} must be defined at ini");
    }
    for name in ["outputmode", "pdfvariable", "toksapp", "luatexversion", "eTeXversion", "numexpr", "pdfoutput", "pdfstrcmp", "randomseed"] {
        assert!(!defined(&e, name), "\\{name} must be undefined at ini");
    }
    run(&mut e, "\\message{[\\meaning\\-]}\\end\n");
    assert!(e.term.contains("[\\explicitdiscretionary]"), "{}", e.term);
}

#[test]
fn enableprimitives_defines_the_extra_groups() {
    let e = run_enabled("");
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    for name in ["outputmode", "pdfvariable", "pdfextension", "toksapp", "luatexversion", "eTeXversion", "numexpr", "savepos", "adjustspacing", "uniformdeviate"] {
        assert!(defined(&e, name), "\\{name} must be enabled");
    }
    // Not LuaTeX primitives at all.
    for name in ["pdfoutput", "pdfstrcmp", "pdfsavepos", "elapsedtime"] {
        assert!(!defined(&e, name), "\\{name} is not a LuaTeX primitive");
    }
}

#[test]
fn enableprimitives_prefix_rules() {
    let mut e = boot(EngineKind::LuaTeX);
    run(
        &mut e,
        r"\def\luatexversion{mine}
\directlua{tex.enableprimitives('my', {'toksapp', 'mytrick', 'luatexversion'})
tex.enableprimitives('pdf', {'pdfvariable'})
tex.enableprimitives('', {'luatexversion', 'eTeXversion', 42, 'numexpr'})}
\message{[\meaning\luatexversion][\meaning\mytoksapp][\meaning\myluatexversion][\meaning\pdfvariable]}
\end
",
    );
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    // Defined names are left alone; a name already starting with the
    // prefix is used as is; non-primitives are skipped.
    assert!(e.term.contains("[macro:->mine][\\toksapp][\\luatexversion][\\pdfvariable]"), "{}", e.term);
    assert!(!defined(&e, "mytrick") && !defined(&e, "mymytrick"));
    assert!(defined(&e, "eTeXversion"));
    // The scan stops at the first non-string entry.
    assert!(!defined(&e, "numexpr"));
}

#[test]
fn extraprimitives_lists_groups_in_table_order() {
    let mut e = boot(EngineKind::LuaTeX);
    run(
        &mut e,
        r"\message{\directlua{local t = tex.extraprimitives('tex')
local n, m = 0, 0
for _ in ipairs(tex.extraprimitives()) do n = n + 1 end
for _ in ipairs(tex.primitives()) do m = m + 1 end
tex.print('[' .. t[1] .. ',' .. t[2] .. '][' .. tostring(m > n) .. ']')}}
\end
",
    );
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    // luatex: tex.extraprimitives('tex') starts vskip, write.
    assert!(e.term.contains("[vskip,write][true]"), "{}", e.term);
}

#[test]
fn pdfvariable_reads_and_assigns_backend_parameters() {
    let e = run_enabled(
        r"\message{[\the\pdfvariable compresslevel][\the\pdfvariable minorversion][\the\pdfvariable horigin][\the\pxdimen][\the\outputmode]}
\pdfvariable compresslevel=7 \pdfvariable omitmediabox=1 \pdfvariable xformmargin=2pt
\message{[\the\pdfvariable compresslevel][\the\pdfvariable omitmediabox][\the\pdfvariable xformmargin]}",
    );
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    assert!(e.term.contains("[0][0][0.0pt][1.00374pt][0]"), "{}", e.term);
    assert!(e.term.contains("[7][1][2.0pt]"), "{}", e.term);
}

#[test]
fn pdffeedback_needs_pdf_mode() {
    let e = run_enabled(r"\message{[\pdffeedback version]}");
    assert_eq!(e.error_count, 1, "{:?}", e.diagnostics);
    assert!(format!("{:?}", e.diagnostics).contains("unexpected use of \\\\pdffeedback"));
    let e = run_enabled(r"\outputmode=1 \message{[\pdffeedback version][\pdffeedback revision][\pdffeedback lastobj]}");
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    assert!(e.term.contains("[140][0][0]"), "{}", e.term);
}

#[test]
fn toks_combining_primitives() {
    let e = run_enabled(
        r"\toks0{a}\toksapp0{b}\tokspre0{c}\message{[\the\toks0]}
\toks2{x}\toksapp0\toks2 \message{[\the\toks0]}
\def\y{Y}\etoksapp0{\y}\message{[\the\toks0]}
{\gtoksapp0{G}}\message{[\the\toks0]}
{\toks4{l}\gtoksapp4{G}\message{[\the\toks4]}}\message{[\the\toks4]}",
    );
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    // A g-form updates a register set at the current level in place.
    assert!(e.term.contains("[cab] [cabx] [cabxY] [cabxYG] [lG] []"), "{}", e.term);
}

#[test]
fn let_variants_and_name_primitives() {
    let e = run_enabled(
        r#"{\glet\z\relax}\message{[\meaning\z]}
\begingroup\globaldefs=-1 \glet\w\relax\endgroup\message{[\meaning\w]}
\letcharcode`A=\relax \catcode`A=13 \message{[\meaning A]}\catcode`A=11
\edef\x{[\csstring\relax][\begincsname undefinedfoo\endcsname][\begincsname relax\endcsname]}\message{\meaning\x}
\message{[\ifdefined\undefinedfoo y\else n\fi]}
\edef\x{\luaescapestring{a"b'c\string\\}}\message{\meaning\x}
\message{[\eTeXVersion][\the\eTeXminorversion][\eTeXrevision][\formatname]}"#,
    );
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    assert!(e.term.contains("[\\relax] [undefined] [\\relax]"), "{}", e.term);
    assert!(e.term.contains("macro:->[relax][][\\relax ]"), "{}", e.term);
    assert!(e.term.contains("[n]"), "{}", e.term);
    assert!(e.term.contains(r#"macro:->a\"b\'c\\"#), "{}", e.term);
    assert!(e.term.contains("[2.2][2][.2][]"), "{}", e.term);
    let e = run_enabled(r"\letcharcode0=\relax");
    assert_eq!(e.error_count, 1);
    assert!(format!("{:?}", e.diagnostics).contains("invalid number for \\\\letcharcode"));
}

#[test]
fn glue_order_counts_the_fi_level() {
    let e = run_enabled(
        r"\skip0=0pt plus 1fil minus 1filll
\message{[\the\gluestretchorder\skip0][\the\eTeXgluestretchorder\skip0][\the\glueshrinkorder\skip0][\the\gluestretchorder\skip1]}",
    );
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    assert!(e.term.contains("[2][1][4][0]"), "{}", e.term);
}

#[test]
fn pack_primitives_build_boxes() {
    let e = run_enabled(
        r"\setbox0\hpack to 5pt{}\setbox1\vpack{\hrule height 2pt}\setbox2\tpack{\hrule height 3pt depth 0pt\hrule height 1pt}
\message{[\the\wd0][\the\ht1][\the\ht2][\the\dp2]}",
    );
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    assert!(e.term.contains("[5.0pt][2.0pt][3.0pt][1.0pt]"), "{}", e.term);
}

#[test]
fn pdftex_mode_has_no_luatex_names() {
    let e = boot(EngineKind::PdfTeX);
    for name in ["randomseed", "setrandomseed", "outputmode", "glet", "toksapp", "pdfvariable"] {
        assert!(!defined(&e, name), "pdfTeX must not define \\{name}");
    }
    assert!(defined(&e, "pdfrandomseed") && defined(&e, "pdfsetrandomseed"));
}

#[test]
fn format_keeps_the_enabled_set_and_the_primitive_table() {
    let mut e = boot(EngineKind::LuaTeX);
    run(&mut e, "\\directlua{tex.enableprimitives('', {'toksapp'})}\n");
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("luatex_model.fmt");
    tex_core::format::save_format(&e, &path).unwrap();
    let mut e = tex_core::format::load_format(&path).unwrap();
    assert!(defined(&e, "toksapp") && defined(&e, "glet"));
    // Not enabled, and no pdfTeX names restored from the scratch engine.
    for name in ["outputmode", "pdfvariable", "pdfstrcmp", "pdfoutput"] {
        assert!(!defined(&e, name), "\\{name} must stay undefined after load");
    }
    run(
        &mut e,
        "\\directlua{tex.enableprimitives('', {'outputmode', 'pdfvariable'})}\\outputmode=1 \\pdfvariable compresslevel=3 \\message{[\\the\\outputmode][\\the\\pdfvariable compresslevel][\\meaning\\-]}\\end\n",
    );
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    assert!(e.term.contains("[1][3][\\explicitdiscretionary]"), "{}", e.term);
}

#[test]
fn pdfextension_dispatches_in_pdf_mode_only() {
    let e = run_enabled(
        r"\setbox0\hbox{\pdfextension literal{0 g}}\message{[\the\wd0]}
\outputmode=1
\immediate\pdfextension obj {<</A 1>>}\edef\a{\pdffeedback lastobj}
\pdfextension obj reserveobjnum \edef\b{\pdffeedback lastobj}
\message{[\number\numexpr\b-\a]}
\setbox0\hbox{\pdfextension literal{0 g}\boundary5 \wordboundary\protrusionboundary2 \pdfextension save\pdfextension restore}
\message{[\the\wd0]}
\pdfextension foo",
    );
    // luatex: in DVI mode the literal is skipped and `0 g` is typeset text.
    assert!(e.term.contains("[0.0pt]"), "{}", e.term);
    assert!(e.term.contains("[1] [0.0pt]"), "{}", e.term);
    assert_eq!(e.error_count, 1, "{:?}", e.diagnostics);
    assert!(format!("{:?}", e.diagnostics).contains("unexpected use of \\\\pdfextension"));
}
