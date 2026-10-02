//! XeTeX hyphenation (xetex.web `\\patterns`, `\\hyphenation` and the
//! "Try to hyphenate the following word" pass). Every expectation was
//! produced by TeX Live 2026 `xetex -etex -ini` on the very same source with
//! the hyph-utf8 pattern files of the embedded archive.

use tex_core::engine::{Engine, EngineKind, InteractionMode};

fn boot() -> Engine {
    let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
    eng.init_primitives();
    eng.add_nullfont();
    eng.set_interaction_mode(InteractionMode::Nonstop);
    eng
}

fn run(eng: &mut Engine, src: &str) {
    eng.input.push_file("test.tex".into(), src.as_bytes().to_vec());
    eng.run();
}

/// The text of every "Underfull \\hbox" report: the words with their
/// discretionaries, joined by `|`.
fn underfull_reports(term: &str) -> Vec<String> {
    let term = unprivate(term);
    let lines: Vec<&str> = term.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].starts_with("Underfull \\hbox") {
            let mut parts = Vec::new();
            i += 1;
            while !lines[i].starts_with("\\hbox(") {
                if !lines[i].is_empty() {
                    parts.push(lines[i]);
                }
                i += 1;
            }
            out.push(parts.join("|"));
        }
        i += 1;
    }
    out
}

/// The text of the lines of each `\\showbox0` paragraph (native words are
/// printed as `\\x text`; a hyphen taken at the line end is part of the text).
fn showbox_lines(term: &str) -> Vec<Vec<String>> {
    let mut boxes: Vec<Vec<String>> = Vec::new();
    let mut current: Option<String> = None;
    for line in term.lines() {
        if line.starts_with("> \\box0=") {
            boxes.push(Vec::new());
            current = None;
        } else if line.starts_with(".\\hbox") {
            current = Some(String::new());
            boxes.last_mut().unwrap().push(String::new());
        } else if let Some(text) = current.as_mut() {
            if let Some(word) = line.strip_prefix("..\\x ") {
                text.push_str(word);
            } else if line.starts_with("..\\glue") && !line.contains("rightskip") && !line.contains("parfillskip") {
                text.push(' ');
            } else if line.starts_with("! OK") {
                current = None;
                continue;
            } else {
                continue;
            }
            *boxes.last_mut().unwrap().last_mut().unwrap() = text.trim().to_string();
        }
    }
    boxes
}

/// The transcript shows 8-bit characters of TFM fonts as private-use
/// U+F7xx; XeTeX writes the scalar itself.
fn unprivate(text: &str) -> String {
    text.chars()
        .map(|c| match c as u32 { 0xF780..=0xF7FF => char::from_u32(c as u32 - 0xF700).unwrap(), _ => c })
        .collect()
}

fn error_messages(term: &str) -> Vec<String> {
    term.lines()
        .filter_map(|l| l.strip_prefix("! "))
        .map(|l| l.trim_end_matches('.').to_string())
        .collect()
}

/// Words of the English, German, Spanish, French, Portuguese, Russian and
/// Greek pattern files set in a TFM font (`ec-lmr10`): `\\showhyphens` of each.
#[test]
fn tfm_words_are_hyphenated_as_xetex_does() {
    let mut eng = boot();
    run(&mut eng, TFM_SRC);
    let expected: Vec<&str> = TFM_EXPECTED.to_vec();
    assert_eq!(underfull_reports(&eng.log), expected);
}

/// The same words (and `\\hyphenation` exceptions of a language without
/// patterns) as native words of an OpenType font, forced into one fragment
/// per line by `\\hsize=1sp`.
#[test]
fn native_words_are_hyphenated_as_xetex_does() {
    let mut eng = boot();
    run(&mut eng, NATIVE_SRC);
    let got = showbox_lines(&eng.log);
    assert_eq!(got.len(), NATIVE_EXPECTED.len());
    for (language, (got, expected)) in got.iter().zip(NATIVE_EXPECTED.iter()).enumerate() {
        let got: Vec<&str> = got.iter().map(String::as_str).collect();
        assert_eq!(got.as_slice(), *expected, "language {language}");
    }
}

/// `\\patterns` and `\\hyphenation` themselves: the characters TeX
/// accepts, `\\savinghyphcodes`, exceptions, `\\uchyph`, the hyphenation
/// minima, and the error messages.
#[test]
fn patterns_and_exceptions_follow_xetex_web() {
    let mut eng = boot();
    run(&mut eng, SEMANTICS_SRC);
    assert_eq!(underfull_reports(&eng.log), SEMANTICS_EXPECTED);
    assert_eq!(error_messages(&eng.log), SEMANTICS_ERRORS);
}

/// xetex.web §1252: `\\patterns` outside INITEX is an error that skips the
/// braces.
#[test]
fn patterns_outside_initex_are_rejected() {
    let mut eng = boot();
    eng.ini_mode = false;
    run(&mut eng, "\\catcode`\\{=1 \\catcode`\\}=2 \\patterns{a1b} \\hyphenation{a-b} \\end\n");
    assert_eq!(error_messages(&eng.log), ["Patterns can be loaded only by INITEX"]);
}

/// A format keeps the patterns, the exceptions and the `\savinghyphcodes`
/// tables of XeTeX; in the run that loads it `\patterns` is rejected and the
/// saved codes, not the live `\lccode`s, decide which letters words consist of.
#[test]
fn format_keeps_patterns_exceptions_and_saved_codes() {
    let mut eng = boot();
    run(&mut eng, FORMAT_BUILD_SRC);
    assert_eq!(eng.error_count, 0, "{:?}", eng.diagnostics);
    let path = std::env::temp_dir().join(format!("ratex-xetex-hyph-{}.fmt", std::process::id()));
    tex_core::format::save_format(&eng, &path).unwrap();
    let mut loaded = tex_core::format::load_format(&path).unwrap();
    std::fs::remove_file(&path).ok();
    loaded.set_interaction_mode(InteractionMode::Nonstop);
    run(&mut loaded, FORMAT_USE_SRC);
    assert_eq!(underfull_reports(&loaded.log), FORMAT_EXPECTED);
    assert_eq!(error_messages(&loaded.log), FORMAT_ERRORS);
}

const FORMAT_BUILD_SRC: &str = r####"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\lccode`\a=`\a \lccode`\b=`\b \lccode`\c=`\c \lccode`\d=`\d \lccode`\o=`\o \lccode`\x=`\x \lccode`\z=`\z
\lccode`\A=`\a \lccode`\B=`\b \lccode"E4="E4 \lccode"C4="E4 \lccode"F6="F6 \lccode"D6="F6 \lccode"3B1="3B1 \lccode"3B2="3B2
\savinghyphcodes=1
\language=5
\patterns{a1b 2c. .d3 ä1ö o1ä \string α1β}
\language=6
\patterns{x1z}
\hyphenation{foo-bar ba-zä-ö}
\language=5
\hyphenation{ab-cd Ab-Ba o-xo-x ä-ö-z}
"####;

const FORMAT_USE_SRC: &str = r####"\lccode`\b=0 \lccode"E4=0 \lccode"F6=0
\font\x=ec-lmr10 \x \hyphenchar\x=`\- \uchyph=1 \showboxdepth=0 \showboxbreadth=1000
\def\sh#1#2{\setbox0\vbox{\parfillskip=0pt \hsize=16383pt \pretolerance=-1 \tolerance=-1 \hbadness=0 \language=#1 \lefthyphenmin=1 \righthyphenmin=1 \x\ #2}}
\scrollmode
\sh5{ababcd}\sh5{aböäxo}\sh5{Abba}\sh5{oxox}\sh5{äöz}\sh5{Äöz}\sh6{xzxz}\sh5{xzxz}\sh6{foobar}\sh5{foobar}\sh6{bazäö}\sh5{abcd}
\hyphenation{ab-ba}
\sh5{abba}
\patterns{b1c}
\hyphenation{ä-b}
\sh5{äb}
\end
"####;

const FORMAT_EXPECTED: [&str; 14] = [
        r####"[] \x a-ba-bcd"####,
        r####"[] \x a-böäxo"####,
        r####"[] \x Ab-ba"####,
        r####"[] \x o-xo-x"####,
        r####"[] \x ä-ö-z"####,
        r####"[] \x Ä-ö-z"####,
        r####"[] \x x-zx-z"####,
        r####"[] \x xzxz"####,
        r####"[] \x foo-bar"####,
        r####"[] \x foobar"####,
        r####"[] \x ba-zä-ö"####,
        r####"[] \x ab-cd"####,
        r####"[] \x ab-ba"####,
        r####"[] \x ä-b"####,
    ];

const FORMAT_ERRORS: [&str; 1] = [
        r####"Patterns can be loaded only by INITEX"####,
    ];

const TFM_SRC: &str = r####"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\^=7 \def\empty{}\let\bgroup={ \let\egroup=}
\def\lcrange#1#2#3{\ifnum#1>#2 \else \lccode#1=\numexpr#1+#3\relax \expandafter\lcrange\expandafter{\number\numexpr#1+1\relax}{#2}{#3}\fi}
\lcrange{"C0}{"D6}{32} \lcrange{"D8}{"DE}{32} \lcrange{"DF}{"DF}{0} \lcrange{"E0}{"F6}{0} \lcrange{"F8}{"FF}{0}
\lcrange{"152}{"152}{1} \lcrange{"153}{"153}{0} \lcrange{"2019}{"2019}{0}
\lcrange{"410}{"42F}{32} \lcrange{"430}{"44F}{0} \lcrange{"451}{"451}{0}
\lcrange{"386}{"386}{38} \lcrange{"388}{"38A}{37} \lcrange{"391}{"3A1}{32} \lcrange{"3A3}{"3AB}{32}
\lcrange{"390}{"390}{0} \lcrange{"3AC}{"3CE}{0} \lcrange{"3F2}{"3F2}{0} \lcrange{"2BC}{"2BC}{0}
\lcrange{"1F71}{"1F71}{0} \lcrange{"1F73}{"1F73}{0} \lcrange{"1F75}{"1F75}{0} \lcrange{"1F77}{"1F77}{0}
\lcrange{"1F79}{"1F79}{0} \lcrange{"1F7B}{"1F7B}{0} \lcrange{"1F7D}{"1F7D}{0} \lcrange{"1FBD}{"1FBD}{0} \lcrange{"1FBF}{"1FBF}{0}
\savinghyphcodes=1
\language=0\relax \input hyphen.tex\relax
\language=1\relax \input loadhyph-de-1996.tex\relax
\language=2\relax \input loadhyph-es.tex\relax
\language=3\relax \input loadhyph-fr.tex\relax
\language=4\relax \input loadhyph-pt.tex\relax
\language=5\relax \input loadhyph-ru.tex\relax
\language=6\relax \input loadhyph-el-monoton.tex\relax
\language=7\relax \hyphenation{abra-ca-da-bra grün-de der-Zeit}
\font\x=ec-lmr10\relax \x \hyphenchar\x=`\- \uchyph=1 \showboxdepth=100 \showboxbreadth=10000 \hbadness=10000
\def\sh#1#2{\setbox0\vbox{\parfillskip=0pt \hsize=16383pt \pretolerance=-1 \tolerance=-1 \hbadness=0 \showboxdepth=0 \showboxbreadth=1000 \language=#1 \lefthyphenmin=2 \righthyphenmin=2 \x\ #2}}
\scrollmode
\sh{0}{hyphenation}
\sh{0}{supercalifragilistic}
\sh{0}{associate}
\sh{0}{present}
\sh{0}{table}
\sh{0}{extraordinarily}
\sh{0}{International}
\sh{0}{algorithms}
\sh{0}{mathematics}
\sh{0}{typesetting}
\sh{0}{paragraph}
\sh{1}{Donaudampfschifffahrtsgesellschaft}
\sh{1}{Straße}
\sh{1}{Überschwemmung}
\sh{1}{Bücher}
\sh{1}{Äpfelbaum}
\sh{1}{größer}
\sh{1}{Fußball}
\sh{1}{Schloss}
\sh{1}{Schifffahrt}
\sh{1}{Hochschule}
\sh{1}{Zusammenarbeit}
\sh{1}{Rechtschreibung}
\sh{1}{Gemütlichkeit}
\sh{1}{Eisenbahnfahrplan}
\sh{1}{straßenbahnfahrer}
\sh{1}{übergroßer}
\sh{2}{murciélago}
\sh{2}{ñandú}
\sh{2}{pingüino}
\sh{2}{cigüeña}
\sh{2}{electroencefalograma}
\sh{2}{camión}
\sh{2}{ingeniería}
\sh{2}{diccionario}
\sh{2}{extraordinario}
\sh{2}{enseñanza}
\sh{2}{sevilla}
\sh{2}{ángel}
\sh{3}{développement}
\sh{3}{extraordinaire}
\sh{3}{anticonstitutionnellement}
\sh{3}{château}
\sh{3}{évidemment}
\sh{3}{hétérogène}
\sh{3}{typographie}
\sh{3}{aujourd'hui}
\sh{4}{desenvolvimento}
\sh{4}{extraordinário}
\sh{4}{coração}
\sh{4}{paralelepípedo}
\sh{4}{constituição}
\sh{7}{abracadabra}
\sh{7}{Abracadabra}
\sh{7}{gründe}
\sh{7}{derzeit}
\end
"####;

const TFM_EXPECTED: [&str; 56] = [
        r####"[] \x hy-phen-ation"####,
        r####"[] \x su-per-cal-ifrag-ilis-tic"####,
        r####"[] \x as-so-ciate"####,
        r####"[] \x present"####,
        r####"[] \x ta-ble"####,
        r####"[] \x ex-traor-di-nar-i-ly"####,
        r####"[] \x In-ter-na-tion-al"####,
        r####"[] \x al-go-rithms"####,
        r####"[] \x math-e-mat-ics"####,
        r####"[] \x type-set-ting"####,
        r####"[] \x para-graph"####,
        r####"[] \x Do-nau-dampf-schiff-fahrts-ge-sell-schaft"####,
        r####"[] \x Stra-ße"####,
        r####"[] \x Über-schwem-mung"####,
        r####"[] \x Bü-cher"####,
        r####"[] \x Äp-fel-baum"####,
        r####"[] \x grö-ßer"####,
        r####"[] \x Fuß-ball"####,
        r####"[] \x Schloss"####,
        r####"[] \x Schiff-fahrt"####,
        r####"[] \x Hoch-schu-le"####,
        r####"[] \x Zu-sam-men-ar-beit"####,
        r####"[] \x Recht-schrei-bung"####,
        r####"[] \x Ge-müt-lich-keit"####,
        r####"[] \x Ei-sen-bahn-fahr-plan"####,
        r####"[] \x stra-ßen-bahn-fah-rer"####,
        r####"[] \x über-gro-ßer"####,
        r####"[] \x mur-cié-la-go"####,
        r####"[] \x ñan-dú"####,
        r####"[] \x pin-güino"####,
        r####"[] \x ci-güe-ña"####,
        r####"[] \x elec-tro-en-ce-fa-lo-gra-ma"####,
        r####"[] \x ca-mión"####,
        r####"[] \x in-ge-nie-ría"####,
        r####"[] \x dic-cio-na-rio"####,
        r####"[] \x ex-tra-or-di-na-rio"####,
        r####"[] \x en-se-ñan-za"####,
        r####"[] \x se-vi-lla"####,
        r####"[] \x án-gel"####,
        r####"[] \x dé-ve-lop-pe-ment"####,
        r####"[] \x ex-tra-or-di-naire"####,
        r####"[] \x an-ti-cons-ti-tu-tion-nel-le-ment"####,
        r####"[] \x châ-teau"####,
        r####"[] \x évi-dem-ment"####,
        r####"[] \x hé-té-ro-gène"####,
        r####"[] \x ty-po-gra-phie"####,
        r####"[] \x au-jour-d'hui"####,
        r####"[] \x de-sen-vol-vi-men-to"####,
        r####"[] \x ex-tra-or-di-ná-rio"####,
        r####"[] \x co-ra-ção"####,
        r####"[] \x pa-ra-le-le-pí-pe-do"####,
        r####"[] \x cons-ti-tu-i-ção"####,
        r####"[] \x abra-ca-da-bra"####,
        r####"[] \x Abra-ca-da-bra"####,
        r####"[] \x grün-de"####,
        r####"[] \x der-zeit"####,
    ];

const NATIVE_SRC: &str = r####"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\^=7 \def\empty{}\let\bgroup={ \let\egroup=}
\def\lcrange#1#2#3{\ifnum#1>#2 \else \lccode#1=\numexpr#1+#3\relax \expandafter\lcrange\expandafter{\number\numexpr#1+1\relax}{#2}{#3}\fi}
\lcrange{"C0}{"D6}{32} \lcrange{"D8}{"DE}{32} \lcrange{"DF}{"DF}{0} \lcrange{"E0}{"F6}{0} \lcrange{"F8}{"FF}{0}
\lcrange{"152}{"152}{1} \lcrange{"153}{"153}{0} \lcrange{"2019}{"2019}{0}
\lcrange{"410}{"42F}{32} \lcrange{"430}{"44F}{0} \lcrange{"451}{"451}{0}
\lcrange{"386}{"386}{38} \lcrange{"388}{"38A}{37} \lcrange{"391}{"3A1}{32} \lcrange{"3A3}{"3AB}{32}
\lcrange{"390}{"390}{0} \lcrange{"3AC}{"3CE}{0} \lcrange{"3F2}{"3F2}{0} \lcrange{"2BC}{"2BC}{0}
\lcrange{"1F71}{"1F71}{0} \lcrange{"1F73}{"1F73}{0} \lcrange{"1F75}{"1F75}{0} \lcrange{"1F77}{"1F77}{0}
\lcrange{"1F79}{"1F79}{0} \lcrange{"1F7B}{"1F7B}{0} \lcrange{"1F7D}{"1F7D}{0} \lcrange{"1FBD}{"1FBD}{0} \lcrange{"1FBF}{"1FBF}{0}
\savinghyphcodes=1
\language=0\relax \input hyphen.tex\relax
\language=1\relax \input loadhyph-de-1996.tex\relax
\language=2\relax \input loadhyph-es.tex\relax
\language=3\relax \input loadhyph-fr.tex\relax
\language=4\relax \input loadhyph-pt.tex\relax
\language=5\relax \input loadhyph-ru.tex\relax
\language=6\relax \input loadhyph-el-monoton.tex\relax
\language=7\relax \hyphenation{abra-ca-da-bra grün-de der-Zeit}
\font\x="[EBGaramond-Regular.otf]"\relax \x \hyphenchar\x=`\- \uchyph=1 \showboxdepth=100 \showboxbreadth=10000 \hbadness=10000
\hfuzz=1000pt \parindent=0pt \parfillskip=0pt plus 1fil \pretolerance=-1 \tolerance=10000
\scrollmode
\setbox0\vbox{\hsize=1sp \language=0 \lefthyphenmin=2 \righthyphenmin=3 \x hyphenation supercalifragilistic associate present table extraordinarily International algorithms mathematics typesetting paragraph\par}\showbox0
\setbox0\vbox{\hsize=1sp \language=1 \lefthyphenmin=2 \righthyphenmin=3 \x Donaudampfschifffahrtsgesellschaft Straße Überschwemmung Bücher Äpfelbaum größer Fußball Schloss Schifffahrt Hochschule Zusammenarbeit Rechtschreibung Gemütlichkeit Eisenbahnfahrplan straßenbahnfahrer übergroßer\par}\showbox0
\setbox0\vbox{\hsize=1sp \language=2 \lefthyphenmin=2 \righthyphenmin=3 \x murciélago ñandú pingüino cigüeña electroencefalograma camión ingeniería diccionario extraordinario enseñanza sevilla ángel\par}\showbox0
\setbox0\vbox{\hsize=1sp \language=3 \lefthyphenmin=2 \righthyphenmin=3 \x développement extraordinaire anticonstitutionnellement château évidemment hétérogène typographie aujourd'hui œuvre cœur\par}\showbox0
\setbox0\vbox{\hsize=1sp \language=4 \lefthyphenmin=2 \righthyphenmin=3 \x desenvolvimento extraordinário coração paralelepípedo constituição\par}\showbox0
\setbox0\vbox{\hsize=1sp \language=5 \lefthyphenmin=2 \righthyphenmin=3 \x программирование Москва типографика переносов информационный программа\par}\showbox0
\setbox0\vbox{\hsize=1sp \language=6 \lefthyphenmin=2 \righthyphenmin=3 \x εκπαίδευση ηλεκτρονικός υπολογιστής αλφάβητο γραμματοσειρά\par}\showbox0
\setbox0\vbox{\hsize=1sp \language=7 \lefthyphenmin=2 \righthyphenmin=3 \x abracadabra Abracadabra gründe derzeit\par}\showbox0
\end
"####;

const NATIVE_EXPECTED: [&[&str]; 8] = [
        &[
            r####"hyphenation"####,
            r####"su-"####,
            r####"per-"####,
            r####"cal-"####,
            r####"ifrag-"####,
            r####"ilis-"####,
            r####"tic"####,
            r####"as-"####,
            r####"so-"####,
            r####"ciate"####,
            r####"present"####,
            r####"ta-"####,
            r####"ble"####,
            r####"ex-"####,
            r####"traor-"####,
            r####"di-"####,
            r####"nar-"####,
            r####"ily"####,
            r####"In-"####,
            r####"ter-"####,
            r####"na-"####,
            r####"tional"####,
            r####"al-"####,
            r####"go-"####,
            r####"rithms"####,
            r####"math-"####,
            r####"e-"####,
            r####"mat-"####,
            r####"ics"####,
            r####"type-"####,
            r####"set-"####,
            r####"ting"####,
            r####"para-"####,
            r####"graph"####,
        ],
        &[
            r####"Donaudampfschifffahrtsgesellschaft"####,
            r####"Straße"####,
            r####"Über-"####,
            r####"schwem-"####,
            r####"mung"####,
            r####"Bü-"####,
            r####"cher"####,
            r####"Äp-"####,
            r####"fel-"####,
            r####"baum"####,
            r####"grö-"####,
            r####"ßer"####,
            r####"Fuß-"####,
            r####"ball"####,
            r####"Schloss"####,
            r####"Schiff-"####,
            r####"fahrt"####,
            r####"Hoch-"####,
            r####"schule"####,
            r####"Zu-"####,
            r####"sam-"####,
            r####"men-"####,
            r####"ar-"####,
            r####"beit"####,
            r####"Recht-"####,
            r####"schrei-"####,
            r####"bung"####,
            r####"Ge-"####,
            r####"müt-"####,
            r####"lich-"####,
            r####"keit"####,
            r####"Ei-"####,
            r####"sen-"####,
            r####"bahn-"####,
            r####"fahr-"####,
            r####"plan"####,
            r####"stra-"####,
            r####"ßen-"####,
            r####"bahn-"####,
            r####"fah-"####,
            r####"rer"####,
            r####"über-"####,
            r####"gro-"####,
            r####"ßer"####,
        ],
        &[
            r####"murciélago"####,
            r####"ñandú"####,
            r####"pin-"####,
            r####"güino"####,
            r####"ci-"####,
            r####"güeña"####,
            r####"elec-"####,
            r####"tro-"####,
            r####"en-"####,
            r####"ce-"####,
            r####"fa-"####,
            r####"lo-"####,
            r####"grama"####,
            r####"ca-"####,
            r####"mión"####,
            r####"in-"####,
            r####"ge-"####,
            r####"nie-"####,
            r####"ría"####,
            r####"dic-"####,
            r####"cio-"####,
            r####"na-"####,
            r####"rio"####,
            r####"ex-"####,
            r####"tra-"####,
            r####"or-"####,
            r####"di-"####,
            r####"na-"####,
            r####"rio"####,
            r####"en-"####,
            r####"se-"####,
            r####"ñanza"####,
            r####"se-"####,
            r####"vi-"####,
            r####"lla"####,
            r####"án-"####,
            r####"gel"####,
        ],
        &[
            r####"développement"####,
            r####"ex-"####,
            r####"tra-"####,
            r####"or-"####,
            r####"di-"####,
            r####"naire"####,
            r####"an-"####,
            r####"ti-"####,
            r####"cons-"####,
            r####"ti-"####,
            r####"tu-"####,
            r####"tion-"####,
            r####"nel-"####,
            r####"le-"####,
            r####"ment"####,
            r####"châ-"####,
            r####"teau"####,
            r####"évi-"####,
            r####"dem-"####,
            r####"ment"####,
            r####"hé-"####,
            r####"té-"####,
            r####"ro-"####,
            r####"gène"####,
            r####"ty-"####,
            r####"po-"####,
            r####"gra-"####,
            r####"phie"####,
            r####"au-"####,
            r####"jour-"####,
            r####"d'hui"####,
            r####"œuvre"####,
            r####"cœur"####,
        ],
        &[
            r####"desenvolvimento"####,
            r####"ex-"####,
            r####"tra-"####,
            r####"or-"####,
            r####"di-"####,
            r####"ná-"####,
            r####"rio"####,
            r####"co-"####,
            r####"ra-"####,
            r####"ção"####,
            r####"pa-"####,
            r####"ra-"####,
            r####"le-"####,
            r####"le-"####,
            r####"pí-"####,
            r####"pedo"####,
            r####"cons-"####,
            r####"ti-"####,
            r####"tu-"####,
            r####"i-"####,
            r####"ção"####,
        ],
        &[
            r####"программирование"####,
            r####"Москва"####,
            r####"ти-"####,
            r####"по-"####,
            r####"гра-"####,
            r####"фика"####,
            r####"пе-"####,
            r####"ре-"####,
            r####"но-"####,
            r####"сов"####,
            r####"ин-"####,
            r####"фор-"####,
            r####"ма-"####,
            r####"ци-"####,
            r####"он-"####,
            r####"ный"####,
            r####"про-"####,
            r####"грамма"####,
        ],
        &[
            r####"εκπαίδευση"####,
            r####"ηλε-"####,
            r####"κτρο-"####,
            r####"νι-"####,
            r####"κός"####,
            r####"υπο-"####,
            r####"λο-"####,
            r####"γι-"####,
            r####"στής"####,
            r####"αλ-"####,
            r####"φά-"####,
            r####"βητο"####,
            r####"γραμ-"####,
            r####"μα-"####,
            r####"το-"####,
            r####"σειρά"####,
        ],
        &[
            r####"abracadabra"####,
            r####"Abra-"####,
            r####"ca-"####,
            r####"da-"####,
            r####"bra"####,
            r####"gründe"####,
            r####"der-"####,
            r####"zeit"####,
        ],
    ];

const SEMANTICS_SRC: &str = r####"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\lccode`\a=`\a \lccode`\b=`\b \lccode`\c=`\c \lccode`\d=`\d \lccode`\e=`\e \lccode`\f=`\f
\lccode`\o=`\o \lccode`\x=`\x \lccode`\z=`\z \lccode`\A=`\a \lccode`\B=`\b \lccode`\C=`\c \lccode"E4="E4 \lccode"C4="E4 \lccode"F6="F6 \lccode"D6="F6
\lccode"1E9E="DF \lccode"DF="DF \lccode"20AC="20AC \lccode"1D11E="1D11E
\savinghyphcodes=1
\language=5
\patterns{a1b 2c. .d3 e1f\relax 1x1 ä1ö o1ä}
\patterns{a1b}
\patterns{x$y}
\patterns{1ß1 ß2z}
\lccode`\b=0
\language=6 \patterns{a1b}
\lccode`\b=`\b
\language=5
\hyphenation{foo-bar ba-zä-ö ab-cd Ab-Ba o-xo-x ä-ö-z}
\hyphenation{\char"E4 -\char"F6 ab-}
\hyphenation{a}
\hyphenation{x\relax y}
\hyphenation{a-b-c-a-b-c-a-b-c-a-b-c-a-b-c-a-b-c-a-b-c-a-b-c-a-b-c}
\font\x=ec-lmr10 \x \hyphenchar\x=`\-
\def\sh#1#2{\setbox0\vbox{\parfillskip=0pt \hsize=16383pt \pretolerance=-1 \tolerance=-1 \hbadness=0 \showboxdepth=0 \showboxbreadth=1000 \language=#1 \lefthyphenmin=1 \righthyphenmin=1 \x\ #2}}
\scrollmode \uchyph=1 \showboxdepth=0 \showboxbreadth=1000
\sh5{abababab}
\sh5{ababcdefef}
\sh5{dxxxd}
\sh5{aböäxo}
\sh5{öbäö}
\sh5{aböoxäo}
\sh6{abab}
\sh5{foobar}
\sh5{bazäö}
\sh5{abcd}
\sh5{AbBa}
\sh5{oxox}
\sh5{äöz}
\sh5{ÄÖZ}
\sh5{abcabcabcabcabcabcabcabcabcabc}
\uchyph=0
\sh5{Abab}
\sh5{abAB}
\lefthyphenmin=3 \righthyphenmin=2
\sh5{abababab}
\message{[done]}
\patterns{b1c}
\end
"####;

const SEMANTICS_EXPECTED: [&str; 18] = [
        r####"[] \x a-ba-ba-ba-b"####,
        r####"[] \x a-ba-bcdefef"####,
        r####"[] \x d-xxxd"####,
        r####"[] \x a-böäxo"####,
        r####"[] \x öbä-ö"####,
        r####"[] \x a-böoxäo"####,
        r####"[] \x abab"####,
        r####"[] \x foo-bar"####,
        r####"[] \x ba-zä-ö"####,
        r####"[] \x ab-cd"####,
        r####"[] \x Ab-Ba"####,
        r####"[] \x o-xo-x"####,
        r####"[] \x ä-ö-z"####,
        r####"[] \x Ä-Ö-Z"####,
        r####"[] \x a-bca-bca-bca-bca-bca-bca-bca-bca-bca-bc"####,
        r####"[] \x Abab"####,
        r####"[] \x a-bA-B"####,
        r####"[] \x a-ba-ba-ba-b"####,
    ];

const SEMANTICS_ERRORS: [&str; 6] = [
        r####"Bad \patterns"####,
        r####"Duplicate pattern"####,
        r####"Nonletter"####,
        r####"Nonletter"####,
        r####"Improper \hyphenation will be flushed"####,
        r####"Too late for \patterns"####,
    ];
