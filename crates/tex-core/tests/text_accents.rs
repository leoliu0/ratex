use tex_core::engine::Engine;

#[test]
fn accent_centers_over_chardef_base() {
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.input.push_file(
        "accent.tex".into(),
        br"\catcode`\{=1 \catcode`\}=2
\font\test=cmr10 \test
\chardef\dotless=16
\let\alias=\dotless
\def\expanded{\dotless}
\setbox0=\hbox{\accent19\char16}
\setbox1=\hbox{\accent19\dotless}
\setbox2=\hbox{\accent19\alias}
\setbox3=\hbox{\accent19\expanded}
\count0=\wd0 \count1=\wd1 \count2=\wd2 \count3=\wd3
\end
"
        .to_vec(),
    );
    e.run();
    assert_eq!(e.error_count, 0, "{}", e.term);
    assert!(e.eqtb.count[0] > 0, "test font must have a base glyph");
    for i in 1..=3 {
        assert_eq!(
            e.eqtb.count[i], e.eqtb.count[0],
            "accent must overlay the chardef base in box {i}"
        );
        assert_eq!(
            format!("{:?}", e.eqtb.boxed[i]),
            format!("{:?}", e.eqtb.boxed[0]),
            "chardef must produce the same accent positioning as explicit char"
        );
    }
}
