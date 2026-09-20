//! Hermetic embedded-input and per-language hyphenation regressions.

use std::path::Path;
use tex_core::engine::Engine;

#[test]
fn raw_openin_reads_embedded_babel_locale_without_host_files() {
    let fs = tex_kpse::fs::MemoryFs::new(Path::new("/project"), 0).unwrap();
    let _scope = fs.enter();
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.input.push_file(
        "locale.tex".into(),
        br#"\catcode`\{=1 \catcode`\}=2
\openin0=babel-es.ini
\ifeof0
  \errmessage{Embedded locale missing}
\else
  \read0 to\firstline
  \message{LOCALE-LINE=\meaning\firstline}
  \closein0
\fi
\end
"#
        .to_vec(),
    );
    engine.run();
    assert_eq!(engine.error_count, 0, "{}", engine.term);
    assert!(
        engine.term.contains("This file is part of babel."),
        "The embedded stream must return the locale's actual first line: {}",
        engine.term
    );
}

#[test]
fn dumped_format_preserves_distinct_language_patterns_and_aliases() {
    let mut engine = Engine::new(false);
    tex_core::format::load_format_bytes_into(
        include_bytes!("../../tex-cli/assets/default.fmt.zst"),
        &mut engine,
    )
    .unwrap();
    let language = |name: &[u8]| -> u8 {
        let id = engine.cs.lookup(name).expect("Declared language missing");
        let value = match engine.eqtb.resolve(id).expect("Language undefined") {
            tex_core::eqtb::Equiv::CharDef(value) => *value,
            other => panic!("Expected a language register, got {other:?}"),
        };
        u8::try_from(value).unwrap()
    };
    let english = language(b"l@english");
    let spanish = language(b"l@spanish");
    let portuguese = language(b"l@portuguese");
    assert_ne!(english, spanish);
    assert_ne!(portuguese, spanish);
    assert_ne!(portuguese, english);
    assert_eq!(language(b"l@american"), english);
    assert_eq!(language(b"l@brazilian"), portuguese);

    let english_trie = engine
        .trie_for_language(english)
        .expect("English patterns missing");
    let spanish_trie = engine
        .trie_for_language(spanish)
        .expect("Spanish patterns missing");
    assert_eq!(english_trie.hyphenate(b"hyphenation", 2, 3), vec![2, 6]);
    assert_eq!(spanish_trie.hyphenate(b"computadora", 2, 2), vec![5, 9]);
    assert_ne!(
        english_trie.hyphenate(b"computadora", 2, 2),
        spanish_trie.hyphenate(b"computadora", 2, 2)
    );
}

#[test]
fn patterns_and_exceptions_expand_macros_in_normal_tex_context() {
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.input.push_file(
        "patterns.tex".into(),
        br#"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\protected\def\prefix{a}
\def\pattern#1{#1}
\patterns{\pattern{\prefix1bc}}
\hyphenation{\prefix b-cd}
\end"#
            .to_vec(),
    );
    engine.run();
    assert_eq!(engine.error_count, 0, "{}", engine.term);
    assert_eq!(engine.hyphen_trie.hyphenate(b"abc", 1, 1), vec![1]);
    assert_eq!(engine.hyphen_trie.hyphenate(b"abcd", 1, 1), vec![2]);
}
