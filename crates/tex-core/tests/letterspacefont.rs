use tex_core::engine::Engine;
use tex_core::eqtb::Equiv;
use tex_core::prim::Prim;

fn boot() -> Engine {
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e
}

fn run_tex(e: &mut Engine, src: &str) {
    e.input
        .push_file("test.tex".to_string(), src.as_bytes().to_vec());
    e.run();
}

#[test]
fn letterspacefont_primitive_recognized_and_registered() {
    let e = boot();
    let cs_id = e.cs.lookup(b"letterspacefont").expect("primitive exists");
    match e.eqtb.resolve(cs_id) {
        Some(Equiv::Prim(p)) => {
            assert_eq!(*p, Prim::Letterspacefont);
            assert_eq!(*p, Prim::LetterspaceFont);
        }
        other => panic!("expected Prim::Letterspacefont, got {:?}", other),
    }
}

#[test]
fn letterspacefont_execution_registers_tracked_font_in_font_loader() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\font\base=cmr10 at 10pt
\letterspacefont \spaced = \base 50
\setbox0=\hbox{\base A}
\setbox1=\hbox{\spaced A}
\count0=\wd0
\count1=\wd1
\end
"#,
    );
    assert_eq!(e.error_count, 0, "errors: {}", e.term);

    // Resolve font IDs
    let base_cs = e.cs.lookup(b"base").expect("base cs found");
    let spaced_cs = e.cs.lookup(b"spaced").expect("spaced cs found");

    let base_font_id = match e.eqtb.resolve(base_cs) {
        Some(Equiv::FontRef(f)) => *f,
        other => panic!("expected FontRef for base, got {:?}", other),
    };
    let spaced_font_id = match e.eqtb.resolve(spaced_cs) {
        Some(Equiv::FontRef(f)) => *f,
        other => panic!("expected FontRef for spaced, got {:?}", other),
    };

    assert_ne!(base_font_id, spaced_font_id);

    // Verify font_loader registration
    assert!(!e.font_loader.is_tracked_font(base_font_id));
    assert!(e.font_loader.is_tracked_font(spaced_font_id));
    assert!(e.is_tracked_font(spaced_font_id));

    let tracked = e
        .font_loader
        .get_tracked_font(spaced_font_id)
        .expect("tracked font metadata registered in font_loader");
    assert_eq!(tracked.base_font, base_font_id);
    assert_eq!(tracked.tracking, 50);

    // Verify tracked font from engine helper
    let tracked_eng = e
        .tracked_font(spaced_font_id)
        .expect("tracked font via engine");
    assert_eq!(tracked_eng.base_font, base_font_id);
    assert_eq!(tracked_eng.tracking, 50);

    // Verify character advance widened by tracking:
    // quad of cmr10 at 10pt is 10pt = 655360 sp.
    // dw = 655360 * 50 / 1000 = 32768 sp.
    let dw = 32768;
    let base_w = e.eqtb.count[0];
    let spaced_w = e.eqtb.count[1];
    assert_eq!(
        spaced_w,
        base_w + dw,
        "spaced font advance must be base advance + quad*tracking/1000"
    );
}

#[test]
fn letterspacefont_syntax_variants_and_negative_tracking() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\font\base=cmr10 at 10pt
% Without equals sign
\letterspacefont \spacednoeq \base 100
% With double equals sign
\letterspacefont \spacedtwoeq = \base = 100
% Negative tracking
\letterspacefont \tight = \base -50
% With nolig keyword
\letterspacefont \noligfont = \base 50 nolig
\setbox0=\hbox{\base A}
\setbox1=\hbox{\tight A}
\count0=\wd0
\count1=\wd1
\end
"#,
    );
    assert_eq!(e.error_count, 0, "errors: {}", e.term);

    let base_cs = e.cs.lookup(b"base").unwrap();
    let base_fid = match e.eqtb.resolve(base_cs).unwrap() {
        Equiv::FontRef(f) => *f,
        _ => unreachable!(),
    };

    let tight_cs = e.cs.lookup(b"tight").unwrap();
    let tight_fid = match e.eqtb.resolve(tight_cs).unwrap() {
        Equiv::FontRef(f) => *f,
        _ => unreachable!(),
    };

    assert!(e.font_loader.is_tracked_font(tight_fid));
    let tracked_tight = e.font_loader.get_tracked_font(tight_fid).unwrap();
    assert_eq!(tracked_tight.base_font, base_fid);
    assert_eq!(tracked_tight.tracking, -50);

    // Negative advance: dw = -32768 sp
    let base_w = e.eqtb.count[0];
    let tight_w = e.eqtb.count[1];
    assert_eq!(tight_w, base_w - 32768);

    // Check nolig font
    let nolig_cs = e.cs.lookup(b"noligfont").unwrap();
    let nolig_fid = match e.eqtb.resolve(nolig_cs).unwrap() {
        Equiv::FontRef(f) => *f,
        _ => unreachable!(),
    };
    assert!(e.font_loader.is_tracked_font(nolig_fid));
}

#[test]
fn variable_font_expansion_in_linebreak() {
    let mut e = boot();
    run_tex(
        &mut e,
        r#"
\catcode`\{=1 \catcode`\}=2
\font\f=cmr10 at 10pt \f \hyphenchar\f=45
\pdfadjustspacing=2
\pdffontexpand\f 20 20 5 autoexpand
\hsize=100pt
\parindent=0pt
A quick brown fox jumps over the lazy dog near the riverbank.
\par
\end
"#,
    );
    assert_eq!(e.error_count, 0, "errors: {}", e.term);
}
