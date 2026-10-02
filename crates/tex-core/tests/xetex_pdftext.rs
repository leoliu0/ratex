//! Native-font text in the PDF output, written like xdvipdfmx. The expected
//! content stream was produced by TeX Live 2026 `xetex -no-pdf` + `xdvipdfmx -E`
//! (20260113) on the same source (`lmroman10-regular.otf` from the TeX Live
//! tree): ActualText spans, glyph colour with its transparency ExtGState, and
//! `embolden`/`slant`/`extend` as `2 Tr`, `w` and the `Tm` matrix.

use tex_core::engine::{Engine, EngineKind, InteractionMode};

const SRC: &str = r#"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\pdfpagewidth=200pt \pdfpageheight=100pt
\font\a="[lmroman10-regular.otf]" at 10pt
\font\c="[lmroman10-regular.otf]:color=FF000080" at 10pt
\font\e="[lmroman10-regular.otf]:embolden=1.5;slant=0.2;extend=1.3" at 10pt
\XeTeXgenerateactualtext=1
\shipout\vbox{\hbox{\a Hello office\c{} ffi \e fat}}
\end
"#;

fn run() -> Engine {
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng.input.push_file("test.tex".into(), SRC.as_bytes().to_vec());
    eng.run();
    assert_eq!(eng.error_count, 0, "errors: {:?}, term: {}", eng.diagnostics, eng.term);
    eng
}

/// Content stream from the first `/Span`, whitespace collapsed, font resource
/// numbers and the Td/Tm y coordinate (it depends on how the page origin is
/// folded into the stream) replaced.
fn normalized(eng: &Engine) -> String {
    let content = String::from_utf8_lossy(&eng.pdf_doc.pages[0].content).into_owned();
    let from = content.find("/Span").expect("ActualText span");
    let mut toks: Vec<String> = content[from..].split_whitespace().map(str::to_string).collect();
    for i in 0..toks.len() {
        if let Some(n) = toks[i].strip_prefix("/F").filter(|n| n.chars().all(|c| c.is_ascii_digit())) {
            let _ = n;
            toks[i] = "/F#".into();
        }
        if toks[i].starts_with("/Xtx_Gs_") {
            toks[i] = "/Xtx_Gs_#".into();
        }
    }
    let mut out = toks.join(" ");
    // `-11.228 Td[` / `-11.228 Tm[`: keep the operator, drop the y number
    let mut res = String::new();
    let mut rest = out.as_str();
    while let Some(i) = rest.find(" Td[").or_else(|| rest.find(" Tm[")) {
        let head = &rest[..i];
        let cut = head.rfind(' ').unwrap_or(0);
        res.push_str(&head[..cut]);
        res.push_str(" Y");
        res.push_str(&rest[i..i + 4]);
        rest = &rest[i + 4..];
    }
    res.push_str(rest);
    out = res;
    out
}

#[test]
fn native_runs_follow_xdvipdfmx_text_operators() {
    let eng = run();
    let c = normalized(&eng);
    // TL: every native word is its own BDC .. EMC span; the coloured glyph
    // run sits in q /Xtx_Gs gs .. Q with the colour operators around it.
    let expect = [
        "/Span << /ActualText (Hello) >> BDC BT /F# 9.9626 Tf 0 Y Td[<003e0032004800480051>]TJ ET EMC",
        "/Span << /ActualText (office) >> BDC BT /F# 9.9626 Tf 25.734 Y Td[<0051007b002b0032>]TJ ET EMC",
        "/Span << /ActualText (ffi) >> BDC 1 0 0 RG 1 0 0 rg q /Xtx_Gs_# gs BT /F# 9.9626 Tf 51.178 Y Td[<007b>]TJ ET Q 0 G 0 g EMC",
        // embolden = 1.5% of 10pt -> `2 Tr 0.149994 w`; slant .2 and extend 1.3 in Tm
        "/Span << /ActualText (fat) >> BDC BT /F# 9.9626 Tf 2 Tr 0.149994 w 1.3 0 .2 1 62.795 Y Tm[<0037001c0069>]TJ 0 Tr ET EMC",
    ];
    assert_eq!(c.trim_end_matches(" Q"), expect.join(" "));
}
