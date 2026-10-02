//! XeTeX native text: expectations taken from `xetex -ini -etex` of TeX Live
//! 2026 on `fixtures/xetex_text_probe.tex` (font `lmroman10-*.otf` from the
//! TeX Live tree, which the embedded archive carries too).

use tex_core::engine::{Engine, EngineKind, InteractionMode};

fn run_probe() -> Engine {
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    let src = include_bytes!("fixtures/xetex_text_probe.tex");
    eng.input.push_file("probe.tex".into(), src.to_vec());
    eng.run();
    eng
}

fn has(term: &str, needle: &str) {
    assert!(term.contains(needle), "missing {needle:?} in:\n{term}");
}

#[test]
fn fonts_queries_and_boxes_match_texlive() {
    let eng = run_probe();
    let t = &eng.term;
    // \fontdimen1..8 of a native font (loadNativeFont)
    has(t, "[0.0pt|3.33pt|1.665pt|1.11pt|4.31pt|10.0pt|1.11pt|6.83pt]");
    // \fontname with and without features, \XeTeXfonttype, \XeTeXcountglyphs
    has(t, "\"[lmroman10-regular.otf]\" at 10.0pt|\"[lmroman10-regular.otf]:script=latn;+smcp");
    has(t, "|2|821]");
    has(t, "[32|64260]");
    // \XeTeXcharglyph, \XeTeXglyphname, \XeTeXglyphbounds (edges 1..4)
    has(t, "[27|backslash|0.56pt|7.5pt|0.57pt|2.5pt]");
    has(t, "[27|0|0]");
    // \fontcharwd/ht/dp/ic and \iffontchar
    has(t, "[7.5pt|6.83pt|2.06pt|0.50998pt|YN]");
    // OpenType script/language/feature queries
    has(t, "[3|1145457748|14|12]");
    // \showbox of native words, spaces and glyph nodes
    has(t, "\\hbox(11.27+2.89998)x86.68001\n.\\a Hello,\n.\\glue 3.33 plus 1.665 minus 1.11\n.\\a world");
    has(t, "\\hbox(13.524+3.47998)x63.37201\n.\\b Hello,\n.\\glue 3.996 plus 1.998 minus 1.332\n.\\b world");
    has(t, "\\hbox(11.27+2.89998)x19.58\n.\\a A\n.\\a glyph#36\n.\\a B");
    // \XeTeXuseglyphmetrics=1 measures the real glyph extents
    has(t, "\\hbox(6.94+0.10999)x22.5\n.\\a Hello");
}

#[test]
fn missing_character_and_mapping_match_texlive() {
    let eng = run_probe();
    let t = &eng.term;
    has(t, "Missing character: There is no \u{1F600} (U+1F600) in font [lmroman10-regular.otf]!");
    has(t, "Requested font \"[lmroman10-bold.otf]:mapping=tex-text\" at 11.0pt");
    // mapping=tex-text: `` '' --- -- become “ ” — –
    has(t, "\\hbox(7.634+2.13399)x46.63998\n.\\c \u{201c}q\u{201d}\n.\\glue 4.213 plus 2.10649 minus 1.40433\n.\\c \u{2014}\n.\\glue 4.213 plus 2.10649 minus 1.40433\n.\\c \u{2013}");
}

#[test]
fn interchartoks_fire_between_classes() {
    let eng = run_probe();
    let t = &eng.term;
    has(t, "[AB]");
    assert!(!t.contains("[bA]") && !t.contains("[Bb]"), "boundary tokens fired:\n{t}");
    has(t, "\\hbox(7.16+2.04999)x29.58\n.\\a xAByz");
}
