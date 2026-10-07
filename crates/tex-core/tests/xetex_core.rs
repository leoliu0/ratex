//! XeTeX core semantics: parameters, character classes and inter-character
//! tokens, Unicode strings, input encodings, utilities and format
//! persistence.
//!
//! Every expectation was produced by TeX Live 2026's `xetex -ini -etex`
//! running the same source, except where a test says otherwise (that binary
//! crashes on `\XeTeXinputnormalization` and on ICU converters).

use std::path::PathBuf;
use tex_core::engine::{Engine, EngineKind, InteractionMode};

fn boot() -> Engine {
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng
}

fn run_bytes(src: &[u8], dir: Option<PathBuf>) -> Engine {
    let mut eng = boot();
    eng.main_dir = dir;
    eng.input.push_file("test.tex".into(), src.to_vec());
    eng.run();
    eng
}

fn run(src: &str) -> Engine {
    run_bytes(src.as_bytes(), None)
}

/// The `[..]` and `{..}` lines the source wrote with `\show`.
fn shown(eng: &Engine) -> Vec<String> {
    eng.term
        .lines()
        .filter(|l| l.starts_with('[') || l.starts_with('{'))
        .map(str::to_string)
        .collect()
}

const PRELUDE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\catcode`\\^=7\n\
                       \\def\\show#1{\\immediate\\write16{[#1]}}\n";

#[test]
fn parameters_have_texlive_defaults_scoping_and_tracing() {
    let eng = run(&format!(
        "{PRELUDE}{}",
        r#"\show{\the\XeTeXlinebreakpenalty,\the\XeTeXprotrudechars,\the\XeTeXupwardsmode,\the\XeTeXuseglyphmetrics,\the\XeTeXinterchartokenstate,\the\XeTeXdashbreakstate,\the\XeTeXinputnormalization,\the\XeTeXtracingfonts,\the\XeTeXinterwordspaceshaping,\the\XeTeXgenerateactualtext,\the\XeTeXhyphenatablelength,\the\showstream,\the\tracingstacklevels}
\show{\the\XeTeXlinebreakskip}
\XeTeXlinebreakpenalty=7 \XeTeXlinebreakskip=1pt plus 2fil
{\XeTeXlinebreakpenalty=9 \XeTeXlinebreakskip=3pt \XeTeXprotrudechars=2 \XeTeXtracingfonts=1
 \show{\the\XeTeXlinebreakpenalty,\the\XeTeXlinebreakskip,\the\XeTeXprotrudechars,\the\XeTeXtracingfonts}
 {\global\XeTeXhyphenatablelength=5 \XeTeXupwardsmode=1 }
 \show{\the\XeTeXupwardsmode}}
\show{\the\XeTeXlinebreakpenalty,\the\XeTeXlinebreakskip,\the\XeTeXprotrudechars,\the\XeTeXtracingfonts,\the\XeTeXhyphenatablelength,\the\XeTeXupwardsmode}
\show{\meaning\XeTeXlinebreakpenalty|\meaning\XeTeXlinebreakskip|\meaning\XeTeXtracingfonts}
\show{\meaning\showstream|\meaning\tracingstacklevels|\meaning\suppressfontnotfounderror}
\suppressfontnotfounderror=1 \show{\the\suppressfontnotfounderror}
\tracingassigns=1 \tracingonline=1
\XeTeXlinebreakpenalty=7
\XeTeXlinebreakpenalty=8
\XeTeXlinebreakskip=1pt plus 2fil
{\XeTeXinputnormalization=2 }
\tracingassigns=0
\countdef\c=10 \c=\XeTeXlinebreakpenalty \show{\the\c}
\dimen0=\XeTeXlinebreakskip \show{\the\dimen0}
\end
"#
    ));
    assert_eq!(eng.error_count, 0, "{}", eng.term);
    assert_eq!(
        shown(&eng),
        [
            "[0,0,0,0,0,0,0,0,0,0,63,-1,0]",
            "[0.0pt]",
            "[9,3.0pt,2,1]",
            "[0]",
            "[7,1.0pt plus 2.0fil,0,0,5,0]",
            "[\\XeTeXlinebreakpenalty|\\XeTeXlinebreakskip|\\XeTeXtracingfonts]",
            "[\\showstream|\\tracingstacklevels|\\suppressfontnotfounderror]",
            "[1]",
            "{into \\tracingonline=1}",
            "{reassigning \\XeTeXlinebreakpenalty=7}",
            "{changing \\XeTeXlinebreakpenalty=7}",
            "{into \\XeTeXlinebreakpenalty=8}",
            "{changing \\XeTeXlinebreakskip=1.0pt plus 2.0fil}",
            "{into \\XeTeXlinebreakskip=1.0pt plus 2.0fil}",
            "{changing \\XeTeXinputnormalization=0}",
            "{into \\XeTeXinputnormalization=2}",
            "{changing \\tracingassigns=1}",
            "[8]",
            "[1.0pt]",
        ],
        "{}",
        eng.term
    );
}

#[test]
fn char_classes_share_the_sfcode_entry_and_interchar_tokens_scope() {
    let eng = run(&format!(
        "{PRELUDE}{}",
        r#"\show{\the\XeTeXcharclass`a,\the\XeTeXcharclass"4E00,\the\XeTeXcharclass"10FFFF}
\XeTeXcharclass`a=3 \XeTeXcharclass"4E00=4095 \XeTeXcharclass"10FFFF=4096
\show{\the\XeTeXcharclass`a,\the\XeTeXcharclass"4E00,\the\XeTeXcharclass"10FFFF,\the\sfcode`a,\the\sfcode"4E00}
\sfcode`a=2000 \show{\the\XeTeXcharclass`a,\the\sfcode`a}
\sfcode"4E00=1234 \show{\the\XeTeXcharclass"4E00,\the\sfcode"4E00}
{\XeTeXcharclass`a=7 \sfcode`a=1500 \XeTeXcharclass"4E00=8
 \show{\the\XeTeXcharclass`a,\the\sfcode`a,\the\XeTeXcharclass"4E00,\the\sfcode"4E00}
 \global\XeTeXcharclass`b=9 }
\show{\the\XeTeXcharclass`a,\the\sfcode`a,\the\XeTeXcharclass"4E00,\the\sfcode"4E00,\the\XeTeXcharclass`b}
{\sfcode`a=3000 \global\XeTeXcharclass`a=11 }
\show{\the\XeTeXcharclass`a,\the\sfcode`a}
{\XeTeXcharclass`a=12 \global\sfcode`a=777 }
\show{\the\XeTeXcharclass`a,\the\sfcode`a}
\XeTeXinterchartoks 1 2={X}
\show{[\the\XeTeXinterchartoks 1 2][\the\XeTeXinterchartoks 2 1][\the\XeTeXinterchartoks 4095 0]}
{\XeTeXinterchartoks 1 2={Y} \XeTeXinterchartoks 4095 3={B}
 \show{[\the\XeTeXinterchartoks 1 2][\the\XeTeXinterchartoks 4095 3]}
 \global\XeTeXinterchartoks 5 6={G}}
\show{[\the\XeTeXinterchartoks 1 2][\the\XeTeXinterchartoks 4095 3][\the\XeTeXinterchartoks 5 6]}
\toks0=\XeTeXinterchartoks 1 2 \show{\the\toks0}
\XeTeXinterchartoks 7 7=\toks0 \show{\the\XeTeXinterchartoks 7 7}
\XeTeXinterchartoks 7 7={} \show{[\the\XeTeXinterchartoks 7 7]}
\tracingassigns=1 \tracingonline=1 \tracingrestores=1
{\XeTeXinterchartoks 1 2={Z}}
{\XeTeXcharclass`a=5 }
\XeTeXinterchartoks 1 2={X}
\tracingassigns=0 \tracingrestores=0
\show{\meaning\XeTeXcharclass|\meaning\XeTeXinterchartoks}
\let\foo\XeTeXcharclass \foo`c=13 \show{\the\XeTeXcharclass`c}
\afterassignment\show \XeTeXcharclass`d=2 {after}
\XeTeXinterchartoks 4096 0={Q}\show{\the\XeTeXinterchartoks 4096 0|\the\XeTeXinterchartoks 5 4096}
\end
"#
    ));
    assert_eq!(eng.error_count, 0, "{}", eng.term);
    let got = shown(&eng);
    // the `{..}` trace lines of the sfcode entry show class * 65536 + sfcode
    let expected = [
        "[0,0,0]",
        "[3,4095,4096,1000,1000]",
        "[3,2000]",
        "[4095,1234]",
        "[7,1500,8,1234]",
        "[3,2000,4095,1234,9]",
        "[11,3000]",
        "[12,777]",
        "[[X][][]]",
        "[[Y][B]]",
        "[[X][][G]]",
        "[X]",
        "[X]",
        "[[]]",
        "{into \\tracingonline=1}",
        "{changing \\tracingrestores=0}",
        "{into \\tracingrestores=1}",
        "{changing ?=?}",
        "{into ?=?}",
        "{restoring ?=?}",
        "{changing \\sfcode97=787209}",
        "{into \\sfcode97=328457}",
        "{restoring \\sfcode97=787209}",
        "{changing ?=?}",
        "{into ?=?}",
        "{changing \\tracingassigns=1}",
        "[\\XeTeXcharclass|\\XeTeXinterchartoks]",
        "[13]",
        "[after]",
        "[Q|]",
    ];
    assert_eq!(got, expected, "{}", eng.term);
}

#[test]
fn bad_class_and_character_values_are_errors() {
    let eng = run(&format!(
        "{PRELUDE}{}",
        r#"\XeTeXcharclass 5000000=1
\XeTeXcharclass`a=4097
\XeTeXinterchartoks 4097 1={}
\chardef\y="110000
\chardef\y=-1
\end
"#
    ));
    assert_eq!(eng.error_count, 5, "{}", eng.term);
    for message in [
        "Bad character code (5000000)",
        "Bad character class (4097)",
        "Bad character code (1114112)",
        "Bad character code (-1)",
    ] {
        assert!(
            eng.diagnostics.iter().any(|d| d.message.contains(message)),
            "{message}: {:?}",
            eng.diagnostics
        );
    }
}

#[test]
fn strings_are_one_token_per_scalar_and_case_codes_are_unicode() {
    let eng = run(&format!(
        "{PRELUDE}{}",
        r#"\def\len#1\end{\number\numexpr\lenB#1\end\relax}
\def\lenB#1{\ifx#1\end 0\else 1+\expandafter\lenB\fi}
\edef\s{\string é}\show{string:\expandafter\len\s\end}
\edef\s{\detokenize{é€𝄞x}}\show{detok:\expandafter\len\s\end}
\edef\s{\meaning\s}\show{meaning-len:\expandafter\len\s\end}
\catcode`é=11 \catcode`€=12 \def\mé{x}\edef\s{\string\mé}\show{csstring:\expandafter\len\s\end}
\edef\s{\meaning\mé}\show{meaningcs:\expandafter\len\s\end}
\edef\s{\fontname\nullfont}\show{fontname:\s:\expandafter\len\s\end}
\edef\s{\Uchar"1F600}\show{Uchar:\expandafter\len\s\end}
\edef\s{\Uchar32}\show{Uchar32:\expandafter\len\s\end}
\edef\s{\Ucharcat"E9 11}\show{Ucharcat:\expandafter\len\s\end,\ifcat\Ucharcat"E9 11 a Y\else N\fi}
\edef\s{\Ucharcat32 10 }\show{Ucharcat10:\expandafter\len\s\end}
\edef\s{\Ucharcat"41 12}\show{Ucharcat12:\meaning\s}
\def\q{\Ucharcat"E9 13 }
\show{ok}
\uccode"E9="C9 \lccode"C9="E9 \uccode"1D5D0="1D5D1 \uccode`a="E9
\uppercase{\edef\s{éa\char"1D5D0}}\show{upper:\expandafter\len\s\end}
\uppercase{\count0=`é }\show{upper:\the\count0}
\uppercase{\count0=`a }\show{upper:\the\count0}
\lowercase{\count0=`É }\show{lower:\the\count0}
\uccode`a=0 \uppercase{\count0=`a }\show{upper0:\the\count0}
\chardef\x="1F600 \show{\meaning\x}
\chardef\x=300 \show{\meaning\x\the\x}
\show{\number\x}
\expandafter\show\expandafter{\number`é}
\expandafter\show\expandafter{\number`\é}
\count0=`\^^^^^^10ffff \show{\the\count0}
\show{\romannumeral 12 \number"110000}
\catcode`\^^e9=11 \def\a^^e9{y}\show{\string\a^^e9}
\end
"#
    ));
    assert_eq!(eng.error_count, 0, "{}", eng.term);
    assert_eq!(
        shown(&eng),
        [
            "[string:1]",
            "[detok:4]",
            "[meaning-len:12]",
            "[csstring:3]",
            "[meaningcs:9]",
            "[fontname:nullfont:8]",
            "[Uchar:1]",
            "[Uchar32:0]",
            "[Ucharcat:1, Y]",
            "[Ucharcat10:0]",
            "[Ucharcat12:macro:->A]",
            "[ok]",
            "[upper:9]",
            "[upper:201]",
            "[upper:233]",
            "[lower:233]",
            "[upper0:97]",
            "[\\char\"1F600]",
            "[\\char\"12C300]",
            "[300]",
            "[233]",
            "[233]",
            "[1114111]",
            "[xii1114112]",
            "[\\aé]",
        ],
        "{}",
        eng.term
    );
}

/// tex.web `print_cs`: a single-character control sequence is followed by a
/// space only when its character is a letter, also beyond ASCII.
#[test]
fn single_unicode_character_control_sequences_print_like_texlive() {
    let eng = run(&format!(
        "{PRELUDE}{}",
        r#"\catcode`\é=11 \def\m{\é\–x\a}\show{\meaning\m}
\show{\detokenize{\é\–x}}
\end
"#
    ));
    assert_eq!(eng.error_count, 0, "{}", eng.term);
    assert_eq!(shown(&eng), ["[macro:->\\é \\–x\\a ]", "[\\é \\–x]"], "{}", eng.term);
}

fn utf16(text: &str, big_endian: bool) -> Vec<u8> {
    text.encode_utf16()
        .flat_map(|unit| if big_endian { unit.to_be_bytes() } else { unit.to_le_bytes() })
        .collect()
}

fn encoding_fixture(dir: &std::path::Path) -> Vec<u8> {
    let pre = PRELUDE.as_bytes().to_vec();
    let with_pre = |body: &[u8]| [pre.clone(), body.to_vec()].concat();
    std::fs::write(dir.join("encl1.tex"), with_pre(b"\\count0=`\xc3 \\show{input-bytes \\the\\count0}\n\\endinput\n")).unwrap();
    let text = |s: &str| [PRELUDE, s].concat();
    std::fs::write(
        dir.join("u16bom.tex"),
        [vec![0xff, 0xfe], utf16(&text("\\count0=`\u{e9} \\show{u16bom \\the\\count0}\n\\endinput\n"), false)].concat(),
    )
    .unwrap();
    std::fs::write(
        dir.join("u8bom.tex"),
        [vec![0xef, 0xbb, 0xbf], text("\\count0=`\u{e9} \\show{u8bom \\the\\count0}\n\\endinput\n").into_bytes()].concat(),
    )
    .unwrap();
    std::fs::write(dir.join("u16be.tex"), utf16(&text("\\count0=`\u{e9} \\show{u16be \\the\\count0}\n\\endinput\n"), true)).unwrap();
    std::fs::write(dir.join("u16nobom.tex"), utf16(&text("\\count0=`\u{e9} \\show{u16nobom \\the\\count0}\n\\endinput\n"), false)).unwrap();

    let mut main = pre;
    main.extend_from_slice(b"\\XeTeXinputencoding \"bytes\"\n\\count0=`\xc3 \\show{bytes \\the\\count0}\n");
    main.extend_from_slice(b"\\XeTeXinputencoding \"utf8\"\n");
    main.extend_from_slice(b"\\count0=`\xc3\xa9 \\show{utf8 \\the\\count0}\n");
    main.extend_from_slice(b"\\count0=`\xe9 \\show{badutf8 \\the\\count0}\n");
    main.extend_from_slice(b"\\count0=`\xe2\x82 \\show{truncated \\the\\count0}\n");
    main.extend_from_slice(b"\\count0=`\xf8\x88\x80\x80\x80 \\show{five \\the\\count0}\n");
    main.extend_from_slice(b"\\count0=`\xa9 \\show{stray \\the\\count0}\n");
    main.extend_from_slice(b"\\count0=`\xc0\x80 \\show{overlong \\the\\count0}\n");
    main.extend_from_slice(b"\\XeTeXinputencoding \"nonexistent-enc\"\n\\count0=`\xe9 \\show{unknown \\the\\count0}\n");
    main.extend_from_slice(b"\\XeTeXinputencoding \"utf8\"\n\\XeTeXinputencoding \"auto\"\n");
    main.extend_from_slice(b"\\XeTeXinputencoding \"utf16le\"\n");
    main.extend(utf16(
        "\\count0=`\u{e9} \\show{utf16le \\the\\count0}\n\\count0=`\u{1F600} \\show{astral \\the\\count0}\n\\XeTeXinputencoding \"utf8\"\n",
        false,
    ));
    main.extend_from_slice(b"\\XeTeXinputencoding \"utf16be\"\n");
    main.extend(utf16("\\count0=`\u{4e2d} \\show{utf16be \\the\\count0}\n\\XeTeXinputencoding \"utf8\"\n", true));
    main.extend_from_slice(b"\\XeTeXinputencoding \"UTF16\"\n");
    main.extend(utf16("\\count0=`\u{4e2d} \\show{utf16 \\the\\count0}\r\n\\XeTeXinputencoding \"utf8\"\r\n", false));
    main.extend_from_slice(b"\\XeTeXdefaultencoding \"bytes\"\n\\input encl1\n\\XeTeXdefaultencoding \"auto\"\n\\input u16bom\n\\input u8bom\n");
    main.extend_from_slice(b"\\XeTeXdefaultencoding \"utf16be\"\n\\input u16be\n\\XeTeXdefaultencoding \"auto\"\n\\input u16nobom\n");
    main.extend_from_slice(b"\\end\n");
    main
}

#[test]
fn input_encodings_decode_per_file_and_per_line() {
    let dir = std::env::temp_dir().join(format!("xetex_core_enc_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let main = encoding_fixture(&dir);
    let eng = run_bytes(&main, Some(dir.clone()));
    let _ = std::fs::remove_dir_all(&dir);
    // only the `auto` request is an error (TL: "Encoding mode `auto' is not
    // valid for \XeTeXinputencoding"); U+FEFF typeset in nullfont is not
    assert_eq!(eng.error_count, 1, "{}", eng.log);
    assert_eq!(
        shown(&eng),
        [
            "[bytes 195]",
            "[utf8 233]",
            "[badutf8 65533]",
            "[truncated 65533]",
            "[five 65533]",
            "[stray 169]",
            "[overlong 32]",
            "[unknown 233]",
            "[utf16le 233]",
            "[astral 128512]",
            "[utf16be 20013]",
            "[utf16 20013]",
            "[input-bytes 195]",
            "[u16bom 233]",
            "[u8bom 233]",
            "[u16be 233]",
            "[u16nobom 233]",
        ],
        "{}",
        eng.term
    );
    assert!(eng.log.contains("Invalid UTF-8 byte or sequence at line 7 replaced by U+FFFD."), "{}", eng.log);
    assert!(eng.log.contains("Invalid UTF-8 byte or sequence at line 8 replaced by U+FFFD."), "{}", eng.log);
    assert!(eng.log.contains("Invalid UTF-8 byte or sequence at line 9 replaced by U+FFFD."), "{}", eng.log);
    assert!(!eng.log.contains("at line 10 "), "{}", eng.log);
    assert!(eng.log.contains("Unknown encoding `nonexistent-enc'; reading as raw bytes"), "{}", eng.log);
}

/// ICU converters are not exercised by TeX Live's binary here (it crashes),
/// so the expectations follow the converters' tables: ISO-8859-1 maps every
/// byte to the character of that code, windows-1252 maps 0x80 to U+20AC.
#[test]
fn named_converters_and_normalization_follow_unicode() {
    let mut src = PRELUDE.as_bytes().to_vec();
    src.extend_from_slice(b"\\XeTeXinputencoding \"latin1\"\n\\count0=`\xe9 \\show{latin1 \\the\\count0}\n");
    src.extend_from_slice(b"\\XeTeXinputencoding \"cp1252\"\n\\count0=`\x80 \\show{cp1252 \\the\\count0}\n");
    src.extend_from_slice(b"\\XeTeXinputencoding \"utf8\"\n");
    src.extend_from_slice(
        "\\def\\len#1\\end{\\number\\numexpr\\lenB#1\\end\\relax}\n\\def\\lenB#1{\\ifx#1\\end 0\\else 1+\\expandafter\\lenB\\fi}\n\
         \\catcode`\u{301}=11 \\catcode`\u{e9}=11 \\catcode`e=11\n\
         \\XeTeXinputnormalization=1\n\\def\\a{e\u{301}}\\def\\b{\u{e9}}\\show{nfc \\expandafter\\len\\a\\end,\\expandafter\\len\\b\\end}\n\
         \\XeTeXinputnormalization=2\n\\def\\a{e\u{301}}\\def\\b{\u{e9}}\\show{nfd \\expandafter\\len\\a\\end,\\expandafter\\len\\b\\end}\n\
         \\XeTeXinputnormalization=0\n\\def\\a{e\u{301}}\\def\\b{\u{e9}}\\show{none \\expandafter\\len\\a\\end,\\expandafter\\len\\b\\end}\n\\end\n"
            .as_bytes(),
    );
    let eng = run_bytes(&src, None);
    assert_eq!(eng.error_count, 0, "{}", eng.term);
    assert_eq!(
        shown(&eng),
        [
            "[latin1 233]",
            "[cp1252 8364]",
            "[nfc 1,1]",
            "[nfd 2,2]",
            "[none 2,1]",
        ],
        "{}",
        eng.term
    );
}

#[test]
fn utility_primitives_use_their_xetex_names() {
    let dir = std::env::temp_dir().join(format!("xetex_core_util_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("fz.txt"), b"hello").unwrap();
    let eng = run_bytes(
        format!(
            "{PRELUDE}{}",
            r#"\show{\strcmp{abc}{abc},\strcmp{abc}{abd},\strcmp{b}{abc},\strcmp{}{}}
\show{\mdfivesum{hello}}
\show{\mdfivesum file {fz.txt}}
\show{\filesize{fz.txt}|\filesize{nonexistent.txt}|}
\show{\filedump{fz.txt}|\filedump offset 1 length 3 {fz.txt}}
\setrandomseed 42
\show{\the\randomseed:\uniformdeviate 100,\uniformdeviate 100,\uniformdeviate 1000,\normaldeviate}
\setrandomseed 42
\show{\uniformdeviate 100,\uniformdeviate 100}
\show{\ifdefined\pdfstrcmp Y\else N\fi\ifdefined\pdfoutput Y\else N\fi\ifdefined\TeXresUnicodeVersion Y\else N\fi\ifdefined\efcode Y\else N\fi}
\resettimer \show{\ifnum\elapsedtime<2000 Y\else N\fi}
\let\myrelax\relax \def\xx{}
\show{\ifprimitive\relax Y\else N\fi,\ifprimitive\myrelax Y\else N\fi,\ifprimitive\xx Y\else N\fi,\ifprimitive\strcmp Y\else N\fi,\ifprimitive\undefinedcs Y\else N\fi}
\def\ifnum{Z}\def\count{C}
\show{\meaning\ifnum|\primitive\meaning\ifnum|\meaning\strcmp|\meaning\mdfivesum}
\show{\meaning\ifprimitive|\meaning\primitive}
\show{\primitive\ifnum 1<2 Y\else N\fi}
\show{\ifprimitive\ifnum Y\else N\fi}
\show{\meaning\uniformdeviate|\meaning\normaldeviate|\meaning\randomseed}
\show{\meaning\setrandomseed|\meaning\shellescape|\meaning\creationdate}
\show{\meaning\elapsedtime|\meaning\resettimer|\meaning\filedump}
\show{\meaning\filemoddate|\meaning\filesize|\meaning\Uchar|\meaning\Ucharcat}
\end
"#
        )
        .as_bytes(),
        Some(dir.clone()),
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(eng.error_count, 0, "{}", eng.term);
    assert_eq!(
        shown(&eng),
        [
            "[0,-1,1,0]",
            "[5D41402ABC4B2A76B9719D911017C592]",
            "[5D41402ABC4B2A76B9719D911017C592]",
            "[5||]",
            "[|656C6C]",
            "[42:79,36,360,-18408]",
            "[79,36]",
            "[NNNN]",
            "[Y]",
            "[Y,N,N,Y,N]",
            "[macro:->Z|macro:->Z|\\strcmp|\\mdfivesum]",
            "[\\ifprimitive|\\primitive]",
            "[Y]",
            "[N]",
            "[\\uniformdeviate|\\normaldeviate|\\randomseed]",
            "[\\setrandomseed|\\shellescape|\\creationdate]",
            "[\\elapsedtime|\\resettimer|\\filedump]",
            "[\\filemoddate|\\filesize|\\Uchar|\\Ucharcat]",
        ],
        "{}",
        eng.term
    );
}

#[test]
fn xetex_state_survives_a_format_round_trip() {
    let mut eng = run(&format!(
        "{PRELUDE}{}",
        r#"\XeTeXcharclass`a=3 \sfcode`a=2000 \XeTeXcharclass"4E00=4095 \sfcode"4E00=1234
\XeTeXinterchartoks 1 2={X} \XeTeXinterchartoks 4095 3={B}
\XeTeXlinebreakpenalty=7 \XeTeXlinebreakskip=1pt plus 2fil \XeTeXhyphenatablelength=12
\XeTeXinterchartokenstate=1 \XeTeXprotrudechars=2
\def\keep{kept}
\let\strcmp\relax
\dump
"#
    ));
    assert_eq!(eng.error_count, 0, "{}", eng.term);
    assert!(eng.ini_mode);
    let path = std::env::temp_dir().join(format!("xetex_core_{}.fmt", std::process::id()));
    tex_core::format::save_format(&eng, &path).unwrap();
    eng = tex_core::format::load_format(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(eng.engine_kind, EngineKind::XeTeX);
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng.input.push_file(
        "after.tex".into(),
        format!(
            "{PRELUDE}{}",
            r#"\show{\the\XeTeXcharclass`a,\the\sfcode`a,\the\XeTeXcharclass"4E00,\the\sfcode"4E00}
\show{[\the\XeTeXinterchartoks 1 2][\the\XeTeXinterchartoks 4095 3][\the\XeTeXinterchartoks 2 1]}
\show{\the\XeTeXlinebreakpenalty,\the\XeTeXlinebreakskip,\the\XeTeXhyphenatablelength,\the\XeTeXinterchartokenstate,\the\XeTeXprotrudechars,\the\showstream}
\show{\meaning\keep|\meaning\strcmp|\meaning\mdfivesum|\meaning\XeTeXcharclass|\meaning\Uchar}
\show{\ifdefined\pdfoutput Y\else N\fi\ifdefined\pdfstrcmp Y\else N\fi\ifdefined\primitive Y\else N\fi}
\end
"#
        )
        .into_bytes(),
    );
    eng.run();
    assert_eq!(eng.error_count, 0, "{}", eng.term);
    assert_eq!(
        shown(&eng),
        [
            "[3,2000,4095,1234]",
            "[[X][B][]]",
            "[7,1.0pt plus 2.0fil,12,1,2,-1]",
            "[macro:->kept|\\relax|\\mdfivesum|\\XeTeXcharclass|\\Uchar]",
            "[NNY]",
        ],
        "{}",
        eng.term
    );
}


/// `\tracingstacklevels` prefixes of `\tracingmacros` lines (TeX Live
/// `tex.ch`): `~` and one `.` per input level below the level, `~~\name` alone
/// at or beyond it, plain lines at 0 and below, and no argument lines
/// below 0; token lists read back (`\csname`) and `\write` count as levels.
#[test]
fn tracingstacklevels_prefixes_macro_traces() {
    let eng = run(r####"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\~=13
\def\a#1{\b{#1}}\def\b#1{\c#1}\def\c{\d}\def\d{x}
\def\,{\relax}
\expandafter\def\csname\endcsname{k}
\tracingmacros=2 \tracingonline=1
\tracingstacklevels=2 \a{z}
\tracingstacklevels=4 \a{z}
\tracingstacklevels=-1 \a{z}
\tracingstacklevels=0 \a{z}
\tracingstacklevels=5 \csname\endcsname\,
\everypar{\a{y}}\noindent\par
\immediate\write16{\c}
\end
"####);
    assert_eq!(eng.error_count, 0, "{}", eng.term);
    let from = eng.term.find("~.\\a").expect("trace in the terminal output");
    let to = eng.term.find("[0]").unwrap_or(eng.term.len());
    assert_eq!(&eng.term[from..to], r####"~.\a #1->\b {#1}
#1<-z
~~\b 
~~\c 
~~\d 

~.\a #1->\b {#1}
#1<-z

~..\b #1->\c #1
#1<-z

~..\c ->\d 

~...\d ->x

\a #1->\b {#1}

\b #1->\c #1

\c ->\d 

\d ->x

\a #1->\b {#1}
#1<-z

\b #1->\c #1
#1<-z

\c ->\d 

\d ->x

~..\csname\endcsname ->k

~.\,->\relax 
\write->\c 

~...\c ->\d 

~...\d ->x
x
"####);
}

/// `\showstream` sends `\show`, `\showthe`, `\showtokens`, `\showbox`,
/// `\showlists`, `\showgroups` and `\showifs` to an open `\write` stream
/// instead of the terminal; with the selector on a `\write` file every
/// `print_nl` starts a new line. Streams 16 and up, negative values and
/// closed streams leave the commands alone.
#[test]
fn showstream_writes_the_show_commands_to_an_open_stream() {
    // \openout may only write below the working directory
    let path = std::path::PathBuf::from(format!("xetex_core_show_{}.txt", std::process::id()));
    let source = r####"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\immediate\openout3=OUTFILE \showboxdepth=0 \showboxbreadth=0
\def\m#1#2{x#1y}
\protected\long\def\p{p}
\showstream=3
\show\m \show\p \show\relax \show a \show\showstream
\showthe\count1 \showthe\skip0 \showtokens{ab#}
\setbox1\vbox{\hrule\hbox{ab}}\showbox1 \showbox99
\showlists
\begingroup \showgroups \endgroup
\ifnum1=1 \ifcase2 \or\or \showifs \fi\fi
\setbox2\hbox{\showlists}
\showstream=15 \showthe\count2
\showstream=16 \showthe\count3
\showstream=-1 \showthe\count4
\immediate\closeout3
\showstream=3 \showthe\count5
\end
"####.replace("OUTFILE", &path.display().to_string());
    let eng = run(&source);
    let written = std::fs::read(&path).expect("stream file");
    let _ = std::fs::remove_file(&path);
    assert_eq!(String::from_utf8_lossy(&written), r####"
> \m=macro:
#1#2->x#1y

> \p=\protected\long macro:
->p

> \relax=\relax

> the letter a

> \showstream=\showstream

> 0

> 0.0pt

> ab##

> \box1=
\vbox(0.4+0.0)x0.0 []



! OK

> \box99=void


! OK



### vertical mode entered at line 0

prevdepth ignored


! OK



### semi simple group (level 1) entered at line 10 (\begingroup)
### bottom level


! OK



### level 2: \ifcase entered on line 11
### level 1: \ifnum entered on line 11


! OK



### restricted horizontal mode entered at line 12

spacefactor 1000
### vertical mode entered at line 0

prevdepth ignored


! OK
"####);
    // the other commands went to the terminal as usual: streams 15 (not open),
    // 16, -1 and the closed stream 3
    assert_eq!(eng.error_count, 4, "{}", eng.term);
}


/// Characters print as xetex.web's `print_char` does (checked against
/// TeX Live's `xetex -ini`): `^^` notation below 32, for 127 and for
/// 128..=159, UTF-8 from 160 up. The text of TFM characters, `\string`,
/// `\meaning`, `\write`, the `Missing character` lines and macro traces
/// all go through it.
#[test]
fn characters_above_127_print_as_unicode_scalars() {
    let path = std::path::PathBuf::from(format!("xetex_core_print_{}.txt", std::process::id()));
    let eng = run(&r####"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\^=7
\immediate\write16{}
\tracingonline=1 \showboxbreadth=100 \showboxdepth=100
\font\x=ec-lmr10 \x
\setbox0\hbox{\char"E9 \char"F1 \char"80 \char"FF ab\char"1F \char"7F \char"A0 \char"9F}
\showbox0
\message{[\string^^e9][\string^^f1][\string^^80][\string^^a0][\string^^9f][\meaning^^e9][\meaning^^80][\detokenize{^^e9^^80^^a0}]}
\def\a{^^e9^^80^^a0\Uchar"1F600 \Uchar"4E2D}\message{\meaning\a}
\expandafter\def\csname ^^e9\endcsname{u}\expandafter\message\expandafter{\expandafter\meaning\csname ^^e9\endcsname}
\message{\expandafter\string\csname ^^e9\endcsname}
\tracinglostchars=2
\font\y=cmr10 \y
\setbox0\hbox{\char"E9 \char"80 \char"A0 \char"1F00 \char"FFFF \char"1F600}
\immediate\openout3=OUTFILE
\immediate\write3{^^e9^^80^^a0^^1f \string^^e9 \Uchar"1F600 \Uchar"4E2D}
\immediate\closeout3
\immediate\write16{^^e9^^80^^a0^^1f \string^^e9 \Uchar"1F600 \Uchar"4E2D}
\def\q#1{[#1]}\tracingmacros=2 \q{^^e9^^80}
\end
"####.replace("OUTFILE", &path.display().to_string()));
    let written = std::fs::read(&path).expect("write file");
    let _ = std::fs::remove_file(&path);
    assert_eq!(written, "é^^80\u{a0}^^_ é \u{1F600}\u{4E2D}\n".as_bytes());
    let lines: Vec<&str> = eng.term.lines().collect();
    let at = lines.iter().position(|l| *l == "> \\box0=").expect("box display");
    assert_eq!(
        &lines[at + 2..at + 12],
        [".\\x é",
        ".\\x ñ",
        ".\\x ^^80",
        ".\\x ÿ",
        ".\\x a",
        ".\\x b",
        ".\\x ^^_",
        ".\\x ^^?",
        ".\\x \u{a0}",
        ".\\x ^^9f"],
        "{}",
        eng.term
    );
    let has = |line: &str| assert!(lines.contains(&line), "missing {line:?} in\n{}", eng.term);
    has("[é][ñ][^^80][\u{a0}][^^9f][the character é][the character ^^80][é^^80\u{a0}]");
    has("macro:->é^^80\u{a0}\\Uchar \"1F600 \\Uchar \"4E2D macro:->u \\é");
    for line in ["Missing character: There is no é (\"E9) in font cmr10!",
        "Missing character: There is no ^^80 (\"80) in font cmr10!",
        "Missing character: There is no \u{a0} (\"A0) in font cmr10!",
        "Missing character: There is no ἀ (\"1F00) in font cmr10!",
        "Missing character: There is no  (\"FFFF) in font cmr10!",
        "Missing character: There is no 😀 (\"1F600) in font cmr10!",
        "Missing character: There is no é (\"E9) in font cmr10!",
        "Missing character: There is no ^^80 (\"80) in font cmr10!"] {
        has(line);
    }
    has("é^^80\u{a0}^^_ é 😀中");
    for line in ["\\q #1->[#1]", "#1<-é^^80"] {
        has(line);
    }
}
