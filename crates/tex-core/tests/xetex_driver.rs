//! xdvipdfmx special semantics on an XeTeX `Engine`. The expected values were
//! produced by TeX Live 2026 `xetex` + `xdvipdfmx` (20260113) on the same
//! source; only the parts independent of how the page origin is folded into
//! the content stream are compared (colour operators in order, the page size,
//! literal ordering).

use tex_core::engine::{Engine, EngineKind, InteractionMode};

const SRC: &str = r#"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\pdfpagewidth=200pt \pdfpageheight=100pt
\shipout\vbox{\hrule width 10pt height 5pt
\special{color push rgb 1 0 0}\hrule width 10pt height 5pt
\special{color push cmyk 0 1 1 0}\hrule width 10pt height 5pt\special{color pop}
\hrule width 10pt height 5pt\special{color pop}\hrule width 10pt height 5pt
\special{pdf:bcolor [0.5]}\hrule width 10pt height 5pt\special{pdf:ecolor}
\special{pdf:btrans rotate 90}\hrule width 10pt height 5pt\special{pdf:etrans}
\special{x:gsave}\special{x:scale 2 3}\hrule width 10pt height 5pt\special{x:grestore}
\special{pdf:literal 0 0 m 1 1 l S}\special{pdf:literal direct 2 0 m}
}
\end
"#;

fn page_content() -> (String, i64, i64) {
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng.input.push_file("test.tex".into(), SRC.as_bytes().to_vec());
    eng.run();
    assert_eq!(eng.error_count, 0, "errors: {:?}, term: {}", eng.diagnostics, eng.term);
    let page = &eng.pdf_doc.pages[0];
    (String::from_utf8_lossy(&page.content).into_owned(), page.width_sp, page.height_sp)
}

fn color_ops(content: &str) -> Vec<String> {
    let toks: Vec<&str> = content.split_whitespace().collect();
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let n = match *t {
            "g" | "G" => 1,
            "rg" | "RG" => 3,
            "k" | "K" => 4,
            _ => continue,
        };
        out.push(toks[i - n..=i].join(" "));
    }
    out
}

#[test]
fn color_stack_nesting_follows_xdvipdfmx() {
    let (content, w, h) = page_content();
    assert_eq!((w, h), (200 * 65536, 100 * 65536));
    let ops = color_ops(&content);
    // the colour operators after the initial reset, in TL's order
    let expected = [
        "1 0 0 RG", "1 0 0 rg", "0 1 1 0 K", "0 1 1 0 k", "1 0 0 RG", "1 0 0 rg", "0 G", "0 g", "0.5 G", "0.5 g", "0 G",
        "0 g",
    ];
    let mut at = 0;
    for want in expected {
        match ops[at..].iter().position(|o| o == want) {
            Some(i) => at += i + 1,
            None => panic!("missing {want} after position {at} in {ops:?}"),
        }
    }
}

#[test]
fn literals_stay_in_order_between_graphics_state_changes() {
    let (content, _, _) = page_content();
    let a = content.find("0 0 m 1 1 l S").expect("page literal");
    let b = content.find("2 0 m").expect("direct literal");
    assert!(a < b);
    assert_eq!(content.matches('q').count() >= content.matches('Q').count(), true);
}

#[test]
fn docinfo_overrides_creator_and_metadata_matches_xdvipdfmx() {
    let src = r#"\catcode`\{=1 \catcode`\}=2
\shipout\vbox{\special{pdf:docinfo<</Title(T)/Creator(LaTeX with hyperref)>>}\hrule width 10pt height 5pt}
\end
"#;
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng.input.push_file("test.tex".into(), src.as_bytes().to_vec());
    eng.run();
    let pdf = tex_core::driver::finish_pdf(&mut eng, false).expect("pdf");
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.starts_with("%PDF-1.7"), "{}", &text[..20]);
    assert!(text.contains("/Title (T)"), "{text}");
    assert!(text.contains("/Creator (LaTeX with hyperref)"));
    assert!(text.contains("/Producer (xdvipdfmx \\(20260113\\))"));
    assert!(!text.contains("/Trapped") && !text.contains("PTEX"));
}
