//! XeTeX picture primitives on an XeTeX `Engine`. Every expectation below was
//! produced by TeX Live 2026 `xetex -ini -etex` on the same files (the
//! fixtures in `fixtures/xetex_pic`: `multi.pdf` has three pages of
//! 100x60mm, 60x100mm and 80x80mm).

use tex_core::engine::{Engine, EngineKind, InteractionMode};

fn run(body: &str) -> Vec<String> {
    let dir = format!("{}/tests/fixtures/xetex_pic", env!("CARGO_MANIFEST_DIR"));
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    let src = format!(
        r#"\catcode`\{{=1 \catcode`\}}=2 \catcode`\#=6
\def\t#1#2{{\setbox0\hbox{{#1 {dir}/#2 }}\immediate\write16{{=\the\wd0,\the\ht0,\the\dp0}}}}
{body}
\end
"#
    );
    eng.input.push_file("test.tex".into(), src.into_bytes());
    eng.run();
    let mut lines: Vec<String> = eng.term.lines().map(str::to_owned).collect();
    lines.push(format!("!DIAG {:?}", eng.diagnostics));
    lines
}

fn dims(lines: &[String]) -> Vec<&str> {
    lines.iter().filter_map(|l| l.strip_prefix('=')).collect()
}

#[test]
fn picture_node_sizes_follow_load_picture() {
    let lines = run(r"
\t{\XeTeXpicfile}{tmp-1.png}
\t{\XeTeXpicfile}{tmp-1.png scaled 500}
\t{\XeTeXpicfile}{tmp-1.png width 33.3pt}
\t{\XeTeXpicfile}{tmp-1.png height 20pt rotated 37.3}
\t{\XeTeXpicfile}{tmp-1.png width 33.3pt rotated 37.3}
\t{\XeTeXpicfile}{tmp-1.png rotated 37.3 width 33.3pt}
\t{\XeTeXpicfile}{tmp-1.png width 50pt height 10pt rotated 90}
\t{\XeTeXpicfile}{tmp-1.png xscaled 700 yscaled 300 rotated 15.5 scaled 800}
\t{\XeTeXpicfile}{tmpj-1.jpg}
\t{\XeTeXpicfile}{tmpj-1.jpg width 100pt rotated 12.5}
");
    assert_eq!(
        dims(&lines),
        [
            "321.7656pt,242.22804pt,0.0pt",
            "160.8828pt,121.11403pt,0.0pt",
            "33.3pt,25.06854pt,0.0pt",
            "33.25323pt,32.00887pt,0.0pt",
            "41.68051pt,40.12077pt,0.0pt",
            "33.3pt,32.05388pt,0.0pt",
            "10.0pt,50.0pt,0.0pt",
            "189.17117pt,104.17375pt,0.0pt",
            "321.6015pt,242.1045pt,0.0pt",
            "113.92337pt,95.14041pt,0.0pt",
        ]
    );
}

#[test]
fn pdf_node_sizes_follow_page_boxes() {
    let lines = run(r"
\t{\XeTeXpdffile}{multi.pdf}
\t{\XeTeXpdffile}{multi.pdf page 2}
\t{\XeTeXpdffile}{multi.pdf page 3 media}
\t{\XeTeXpdffile}{multi.pdf page 9 rotated 45 scaled 700}
\t{\XeTeXpdffile}{multi.pdf page -1 width 77.7pt}
");
    assert_eq!(
        dims(&lines),
        [
            "284.52798pt,170.7168pt,0.0pt",
            "170.7168pt,284.52798pt,0.0pt",
            "227.62239pt,227.62239pt,0.0pt",
            "225.33467pt,225.33467pt,0.0pt",
            "77.7pt,77.7pt,0.0pt",
        ]
    );
}

#[test]
fn pdf_page_count_is_zero_for_missing_and_non_pdf_files() {
    let dir = format!("{}/tests/fixtures/xetex_pic", env!("CARGO_MANIFEST_DIR"));
    let lines = run(&format!(
        r"
\immediate\write16{{C=\the\XeTeXpdfpagecount {dir}/multi.pdf }}
\immediate\write16{{C=\the\XeTeXpdfpagecount {dir}/nothere.pdf }}
\immediate\write16{{C=\the\XeTeXpdfpagecount {dir}/tmp-1.png }}
"
    ));
    let counts: Vec<&str> = lines.iter().filter_map(|l| l.strip_prefix("C=")).collect();
    assert_eq!(counts, ["3", "0", "0"]);
}

#[test]
fn improper_size_is_reported_and_ignored() {
    let lines = run(r"\t{\XeTeXpicfile}{tmp-1.png width -3pt}");
    assert!(lines.iter().any(|l| l.starts_with("!DIAG") && l.contains("Improper image size (-3.0pt) will be ignored")), "{lines:?}");
    assert_eq!(dims(&lines), ["321.7656pt,242.22804pt,0.0pt"]);
}

#[test]
fn missing_picture_is_an_error_and_adds_no_node() {
    let lines = run(r"\t{\XeTeXpicfile}{nothere.png}");
    assert!(lines.iter().any(|l| l.starts_with("!DIAG") && l.contains("Unable to load picture or PDF file '") && l.contains("nothere.png'")), "{lines:?}");
    assert_eq!(dims(&lines), ["0.0pt,0.0pt,0.0pt"]);
}

/// `\showbox` of picture nodes: xetex.web prints `\XeTeXpicfile "path"` (the
/// directory part is fixture-specific here; TL showed `"./multi.pdf"`).
#[test]
fn show_box_prints_picture_nodes() {
    let dir = format!("{}/tests/fixtures/xetex_pic", env!("CARGO_MANIFEST_DIR"));
    let lines = run(&format!(
        r"\showboxbreadth100 \showboxdepth100 \tracingonline1
\setbox0\hbox{{\XeTeXpdffile {dir}/multi.pdf page -1 art \XeTeXpicfile {dir}/tmp-1.png }}\showbox0"
    ));
    let text = lines.join("\n").replace("\n", "");
    assert!(text.contains("\\hbox(242.22804+0.0)x"), "{text}");
    assert!(text.contains(&format!("\\XeTeXpdffile \"{dir}/multi.pdf\"")), "{text}");
    assert!(text.contains(&format!("\\XeTeXpicfile \"{dir}/tmp-1.png\"")), "{text}");
}

/// xetex.web `scan_decimal` reads `xetex_scan_dimen(.., requires_units=false)`:
/// internal integers and dimensions stand for their value as a number.
#[test]
fn rotated_takes_internal_quantities_without_units() {
    let lines = run(r"
\count1=30 \dimen2=45.5pt
\t{\XeTeXpicfile}{tmp-1.png rotated \count1 }
\t{\XeTeXpicfile}{tmp-1.png rotated \dimen2 }
\t{\XeTeXpicfile}{tmp-1.png rotated -\count1 }
\t{\XeTeXpicfile}{tmp-1.png rotated 12\relax}
");
    assert_eq!(
        dims(&lines),
        [
            "399.7712pt,370.65845pt,0.0pt",
            "398.29774pt,399.27933pt,0.0pt",
            "399.77121pt,370.65843pt,0.0pt",
            "365.0963pt,303.83362pt,0.0pt",
        ]
    );
}

/// BMP files of 24, 8, 4 and 1 bits (the resolution field is 2835 pixels/m).
#[test]
fn bmp_pictures_have_xetex_sizes() {
    let lines = run(r"
\t{\XeTeXpicfile}{rgb24.bmp}
\t{\XeTeXpicfile}{pal8.bmp width 40pt}
\t{\XeTeXpicfile}{pal4.bmp scaled 3000 rotated 30}
\t{\XeTeXpicfile}{bw1.bmp height 20pt}
");
    assert_eq!(
        dims(&lines),
        [
            "30.10873pt,20.0725pt,0.0pt",
            "40.0pt,30.58824pt,0.0pt",
            "68.30629pt,55.08162pt,0.0pt",
            "40.0pt,20.0pt,0.0pt",
        ]
    );
}

