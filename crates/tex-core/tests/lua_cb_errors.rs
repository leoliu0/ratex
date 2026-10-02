//! LuaTeX's error reporting plumbing: the input stack context of an error
//! (`status.lasterrorcontext`, the `show_error_hook` callback), the text of
//! `status.lasterrorstring`, ignored errors (`show_ignored_error_message`)
//! and `normal_warning` (`show_warning_message`, `status.lastwarningtag`).
//!
//! Every expectation was produced by TeX Live 2026 LuaTeX 1.24.0:
//! `luatex -ini -interaction=nonstopmode -jobname=t t.tex`, where `t.tex` is
//! the catcode line, the `\directlua` line enabling the primitives, the
//! `HOOK` below (contexts only), the case body and `\end`. The hook writes
//! the message and context in pieces of 40 bytes (`CTX<n>.<k> `, newlines as
//! `~`, bytes above 127 as `<n>`), which the tests below reassemble.

use tex_core::engine::{Engine, EngineKind};

const HOOK: &str = r#"\directlua{ local n = 0 callback.register('show_error_hook', function()   n = n + 1   texio.write_nl('MSG'..n..' '..tostring(status.lasterrorstring))   local c = tostring(status.lasterrorcontext):gsub(string.char(10), string.char(126))   c = c:gsub('['..string.char(0)..'-'..string.char(31)..string.char(128)..'-'..string.char(255)..']', function(ch) return '<'..string.byte(ch)..'>' end)   local k = 0   for i = 1, string.len(c), 40 do     k = k + 1     texio.write_nl('CTX'..n..'.'..k..' '..c:sub(i, i + 39))   end end) } "#;

fn pin_environment() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var("TEX_MEM_LIMIT_MIB", "0");
        std::env::set_var("FORCE_SOURCE_DATE", "1");
        std::env::set_var("SOURCE_DATE_EPOCH", "1700000000");
    });
}

fn run(preamble: &str, body: &str) -> Engine {
    pin_environment();
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    for (c, v) in [(b'{', 1), (b'}', 2), (b'#', 6), (b'&', 4), (b' ', 10), (b'\n', 5), (b'\r', 5)] {
        e.eqtb.cat[c as usize] = v;
    }
    e.set_interaction_mode(tex_core::engine::InteractionMode::Nonstop);
    e.job_name = "t".to_string();
    e.input.push_file(
        "t.tex".into(),
        format!("\\directlua{{tex.enableprimitives('',tex.extraprimitives())}}\n{preamble}{body}\n\\end\n").into_bytes(),
    );
    e.run();
    e
}

/// `(status.lasterrorstring, status.lasterrorcontext)` of every error the
/// `show_error_hook` of `HOOK` saw.
fn contexts(body: &str) -> Vec<(String, String)> {
    let e = run(&format!("{HOOK}\n"), body);
    let mut messages: Vec<(usize, String)> = Vec::new();
    let mut pieces: Vec<(usize, String)> = Vec::new();
    for line in e.term.split('\n') {
        if let Some(rest) = line.strip_prefix("MSG") {
            let (n, text) = rest.split_once(' ').expect("MSG line");
            messages.push((n.parse().expect("number"), text.to_string()));
        } else if let Some(rest) = line.strip_prefix("CTX") {
            let (id, text) = rest.split_once(' ').expect("CTX line");
            let (n, _) = id.split_once('.').expect("CTX id");
            pieces.push((n.parse().expect("number"), text.to_string()));
        }
    }
    messages
        .into_iter()
        .map(|(n, message)| {
            let text: String = pieces.iter().filter(|(m, _)| *m == n).map(|(_, t)| t.as_str()).collect();
            (message, decode(&text))
        })
        .collect()
}

/// `~` is a newline, `<n>` the byte n.
fn decode(text: &str) -> String {
    let mut bytes = Vec::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            if let Some(end) = rest.find('>') {
                if let Ok(b) = rest[1..end].parse::<u8>() {
                    bytes.push(b);
                    rest = &rest[end + 1..];
                    continue;
                }
            }
        }
        if c == '~' {
            bytes.push(b'\n');
        } else {
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
        rest = &rest[c.len_utf8()..];
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn check(body: &str, expected: &[(&str, &str)]) {
    let got = contexts(body);
    let want: Vec<(String, String)> = expected.iter().map(|(m, c)| (m.to_string(), c.to_string())).collect();
    assert_eq!(got, want);
}

/// The lines of the terminal (or of the log) that start with one of `tags`,
/// cut at the `@` that ends a piece.
fn lines(preamble: &str, body: &str, tags: &[&str], log: bool) -> Vec<String> {
    let e = run(preamble, body);
    let text = if log { e.log.clone() } else { e.term.clone() };
    text.split('\n')
        .filter(|l| tags.iter().any(|t| l.starts_with(t)))
        .map(|l| match l.split_once('@') {
            Some((head, _)) => format!("{head}@"),
            None => l.to_string(),
        })
        .collect()
}


/// Macro levels: arguments, nested calls, `\errorcontextlines`, a cut line.
#[test]
fn macro_frames() {
    check(
        "\\scrollmode\n\\def\\a#1#2{x#1\\errmessage{A}y#2}\n\\def\\b#1{\\a{#1}{z}tail}\n\\def\\c{\\errmessage{C}}\n\\undefinedcs\n\\a{foo}{bar}\n\\errorcontextlines=0 \\b{q}\n\\errorcontextlines=1 \\b{q}\n\\errorcontextlines=3 \\b{q}\n\\errorcontextlines=-1 \\b{q}\n\\errorcontextlines=100\n\\expandafter\\c\\undefinedcs\n\\a{\\undefinedcs}{b}\n\\def\\e#1{\\errmessage{E}}\\e{aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n\\def\\f#1#2#3#4#5#6#7#8#9{\\errmessage{F}#1#2#3#4#5#6#7#8#9}\n\\f aaaaaaaaaabbbbbbbbbbccccccccccddddddddddeeeeeeeeeeffffffffffgggggggggghhhhhhhhhhiiiiiiiiii\n\\edef\\d{\\undefinedcs\\c}",
        &[
            ("Undefined control sequence", "\nl.7 \\undefinedcs\n               "),
            ("", "\n\\a #1#2->x#1\\errmessage {A}\n                           y#2\nl.8 \\a{foo}{bar}\n               "),
            ("", "\n\\a #1#2->x#1\\errmessage {A}\n                           y#2\n...\nl.9 \\errorcontextlines=0 \\b{q}\n                             "),
            ("", "\n\\a #1#2->x#1\\errmessage {A}\n                           y#2\n\\b #1->\\a {#1}{z}\n                 tail\nl.10 \\errorcontextlines=1 \\b{q}\n                              "),
            ("", "\n\\a #1#2->x#1\\errmessage {A}\n                           y#2\n\\b #1->\\a {#1}{z}\n                 tail\nl.11 \\errorcontextlines=3 \\b{q}\n                              "),
            ("", "\n\\a #1#2->x#1\\errmessage {A}\n                           y#2\nl.12 \\errorcontextlines=-1 \\b{q}\n                               "),
            ("Undefined control sequence", "\nl.14 \\expandafter\\c\\undefinedcs\n                              "),
            ("", "\n\\c ->\\errmessage {C}\n                    \nl.14 \\expandafter\\c\\undefinedcs\n                              "),
            ("Undefined control sequence", "\n<argument> \\undefinedcs \n              \n\\a #1#2->x#1\n            \\errmessage {A}y#2\nl.15 \\a{\\undefinedcs}{b}\n                       "),
            ("", "\n\\a #1#2->x#1\\errmessage {A}\n                           y#2\nl.15 \\a{\\undefinedcs}{b}\n                       "),
            ("", "\n\\e #1->\\errmessage {E}\n                      \nl.16 ...aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}\n                                                  bbbbbbbbbbbbbbbbbbbbbbbbbb..."),
            ("", "\n\\f #1#2#3#4#5#6#7#8#9->\\errmessage {F}\n                                      #1#2#3#4#5#6#7#8#9\nl.18 \\f aaaaaaaaa\n                abbbbbbbbbbccccccccccddddddddddeeeeeeeeeeffffffffffggggggggg..."),
            ("Undefined control sequence", "\nl.19 \\edef\\d{\\undefinedcs\n                        \\c}"),
        ],
    );
}

/// `\everypar`, alignment templates, `\write`, `\scantokens`, `\afterassignment`, `\uppercase` levels.
#[test]
fn hook_levels() {
    check(
        "\\scrollmode \\errorcontextlines=100\n\\def\\a#1#2{x#1\\errmessage{A}y#2}\n\\def\\c{\\errmessage{C}}\n\\everypar{\\undefinedcs}\n\\noindent\\par\n\\indent x\\par\n\\everypar{}\n\\halign{#\\undefinedcs&#\\errmessage{T}\\cr a&b\\cr}\n\\halign{\\errmessage{U}#\\cr a\\cr}\n\\immediate\\write16{\\undefinedcs}\n\\scantokens{\\errmessage{S}}\n\\scantokens{\\a{1}{2}}\n\\afterassignment\\c \\count1=1\n\\uppercase{\\errmessage{U}}\n\\toks0={\\errmessage{T0}}\n\\the\\toks0\n\\hbox{\\vrule\\undefinedcs}",
        &[
            ("Undefined control sequence", "\n<everypar> \\undefinedcs \n              \nl.7 \\noindent\n            \\par"),
            ("Undefined control sequence", "\n<everypar> \\undefinedcs \n              \nl.8 \\indent\n           x\\par"),
            ("Undefined control sequence", "\n<template> \\undefinedcs \n              \\endtemplate \nl.10 \\halign{#\\undefinedcs&#\\errmessage{T}\\cr a&\n                                               b\\cr}"),
            ("", "\n<template> \\errmessage {T}\n                \\endtemplate \nl.10 ...ign{#\\undefinedcs&#\\errmessage{T}\\cr a&b\\cr\n                                                  }"),
            ("", "\n<template> \\errmessage {U}\n                \n<to be read again> \n a\nl.11 \\halign{\\errmessage{U}#\\cr a\n                                \\cr}"),
            ("Undefined control sequence", "\n<write> \\undefinedcs \n              \n<inserted text> \n }\\endwrite \nl.12 \\immediate\\write16{\\undefinedcs}\n                                    "),
            ("", "\nl.1 \\errmessage {S}\n                  \nl.13 \\scantokens{\\errmessage{S}}\n                               "),
            ("", "\n\\a #1#2->x#1\\errmessage {A}\n                           y#2\nl.1 \\a {1}{2}\n            \nl.14 \\scantokens{\\a{1}{2}}\n                         "),
            ("", "\n\\c ->\\errmessage {C}\n                    \nl.15 \\afterassignment\\c \\count1=1\n                                "),
            ("", "\n<recently read> \\errmessage {U}\n                \nl.16 \\uppercase{\\errmessage{U}}\n                              "),
            ("", "\n<inserted text> \\errmessage {T0}\n                 \nl.18 \\the\\toks0\n              "),
            ("Undefined control sequence", "\nl.19 \\hbox{\\vrule\\undefinedcs\n                            }"),
        ],
    );
}

/// Errors inside `\output`, with a macro with arguments.
#[test]
fn output_routine() {
    check(
        "\\scrollmode \\errorcontextlines=100\n\\def\\a#1#2{x#1\\errmessage{A}y#2}\n\\vsize=100pt \\hsize=100pt \\maxdepth=2pt\n\\output={\\errmessage{O1}\\setbox0\\box255 \\deadcycles=0 \\undefinedinout}\n\\hbox{}\\penalty-10000\n\\output={\\a{}{}\\setbox0\\box255 \\deadcycles=0 }\n\\hbox{}\\penalty-10000\n\\output={\\setbox0\\box255 \\deadcycles=0 }",
        &[
            ("", "\n<output> {\\errmessage {O1}\n                  \\setbox 0\\box 255 \\deadcycles =0 \\undefinedinout }\nl.7 \\hbox{}\\penalty-10000\n                        "),
            ("Undefined control sequence", "\n<output> ...tbox 0\\box 255 \\deadcycles =0 \\undefinedinout \n                                                  }\nl.7 \\hbox{}\\penalty-10000\n                        "),
            ("", "\n\\a #1#2->x#1\\errmessage {A}\n                           y#2\n<output> {\\a {}{}\n         \\setbox 0\\box 255 \\deadcycles =0 }\nl.9 \\hbox{}\\penalty-10000\n                        "),
        ],
    );
}

/// `<inserted text>`, `<to be read again>` and `<recently read>` levels.
#[test]
fn inserted_text() {
    check(
        "\\scrollmode \\errorcontextlines=100\n\\def\\p#1.{[#1]}\n\\outer\\def\\o{}\n\\def\\q#1{#1}\n\\hbox{\\vskip 1pt}\nx $a_b_c$ \\par\n\\p abc \\o\n\\q}\n\\count1=a\n\\dimen0=2zz\n\\ifnum\\undefinedcs=1 \\fi\n\\q{a}}\n",
        &[
            ("Missing ", "\n<inserted text> \n }\n<to be read again> \n \\vskip \nl.7 \\hbox{\\vskip\n                1pt}"),
            ("Too many }'s", "\n<recently read> }\n  \nl.7 \\hbox{\\vskip 1pt}\n                    "),
            ("Forbidden control sequence found while scanning use of \\p", "\n<inserted text> \n \\par \n<to be read again> \n \\o \nl.9 \\p abc \\o\n            "),
            ("Argument of ", "\n<inserted text> \n \\par \n<to be read again> \n }\nl.10 \\q}\n       "),
            ("Paragraph ended before ", "\n<to be read again> \n \\par \n<to be read again> \n }\nl.10 \\q}\n       "),
            ("Too many }'s", "\n<recently read> }\n  \nl.10 \\q}\n       "),
            ("Missing number, treated as zero", "\n<to be read again> \n a\nl.11 \\count1=a\n             "),
            ("Illegal unit of measure (pt inserted)", "\n<to be read again> \n z\nl.12 \\dimen0=2z\n              z"),
            ("Undefined control sequence", "\nl.13 \\ifnum\\undefinedcs\n                      =1 \\fi"),
            ("Missing number, treated as zero", "\n<to be read again> \n =\nl.13 \\ifnum\\undefinedcs=\n                       1 \\fi"),
            ("Too many }'s", "\nl.14 \\q{a}}\n          "),
        ],
    );
}

/// Nested `\scantokens` pseudo files: each shows `l.N`, the stack continues down to the file.
#[test]
fn pseudo_files() {
    check(
        "\\scrollmode \\errorcontextlines=100\n\\def\\c{\\errmessage{C}}\n\\scantokens{\\scantokens{\\c}}\n\\scantokens{a\n\\errmessage{second line}}",
        &[
            ("", "\n\\c ->\\errmessage {C}\n                    \nl.1 \\c\n     \nl.1 \\scantokens {\\c }\n                    \nl.5 \\scantokens{\\scantokens{\\c}}\n                               "),
            ("", "\nl.1 a \\errmessage {second line}\n                              \nl.7 \\errmessage{second line}}\n                            "),
        ],
    );
}

/// Raw bytes of the line, a changed `\endlinechar`.
#[test]
fn utf8_and_endlinechar() {
    check(
        "\\scrollmode\n\\errmessage{héllo wörld ünicode ☃ snowman}\n\\errmessage{ééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééé}\n{\\endlinechar=-1 \\errmessage{noelc}\n\\endlinechar=13 }",
        &[
            ("", "\nl.4 \\errmessage{héllo wörld ünicode ☃ snowman}\n                                                  "),
            ("", "\nl.5 ...ééééééééééééééééééééé}\n                                                  "),
            ("", "\nl.6 {\\endlinechar=-1 \\errmessage{noelc}\n                                      \r"),
        ],
    );
}

/// Without a callback `\ignoreprimitiveerror` notes go to the log and `status.lasterrorstring` keeps the message.
#[test]
fn ignored_error_plain() {
    let got = lines("", "\\directlua{function P(tag, s) s = tostring(s) for i = 1, math.max(1, string.len(s)), 30 do texio.write_nl(tag..' '..s:sub(i, i + 29)..'@') end end}\n\\scrollmode \\ignoreprimitiveerror=1\n\\setbox0=\\vbox{\\hrule height1pt\\vskip0pt minus1fil\\hrule height1pt}\n\\message{A}\n\\setbox1=\\vsplit0 to100pt\n\\directlua{P('LAST', status.lasterrorstring)}\n\\message{B}", &["LAST ", "ignored"], true);
    assert_eq!(
        got,
        [
            "ignored: Infinite glue shrinkage found in box being split",
            "LAST Infinite glue shrinkage found @",
            "LAST in box being split@",
        ]
    );
}

/// `show_ignored_error_message` runs when the next error is reported (`flush_err`), not when the error is ignored; `show_error_message` takes precedence.
#[test]
fn ignored_error_callback() {
    let got = lines("", "\\directlua{function P(tag, s) s = tostring(s) for i = 1, math.max(1, string.len(s)), 30 do texio.write_nl(tag..' '..s:sub(i, i + 29)..'@') end end}\n\\scrollmode \\ignoreprimitiveerror=1\n\\directlua{callback.register('show_ignored_error_message', function() P('IG', status.lasterrorstring) end)}\n\\setbox0=\\vbox{\\hrule height1pt\\vskip0pt minus1fil\\hrule height1pt}\n\\setbox1=\\vsplit0 to100pt\n\\directlua{P('LAST', status.lasterrorstring)}\n\\undefinedcs\n\\directlua{P('LAST', status.lasterrorstring)}\n\\setbox0=\\vbox{\\hrule height1pt\\vskip0pt minus1fil\\hrule height1pt}\n\\setbox1=\\vsplit0 to100pt\n\\directlua{callback.register('show_error_message', function() P('EM', status.lasterrorstring) end)}\n\\undefinedcs\n\\directlua{P('LAST', status.lasterrorstring)}", &["IG ", "LAST ", "EM "], false);
    assert_eq!(
        got,
        [
            "LAST nil@",
            "IG ignored: Infinite glue shrinka@",
            "IG ge found in box being split@",
            "LAST ignored: Infinite glue shrinka@",
            "LAST ge found in box being split@",
            "EM ignored: Infinite glue shrinka@",
            "EM ge found in box being split! U@",
            "EM ndefined control sequence@",
            "LAST ignored: Infinite glue shrinka@",
            "LAST ge found in box being split! U@",
            "LAST ndefined control sequence@",
        ]
    );
}

/// `normal_warning`: `warning  (tag): text` after an unconditional line break, and the `show_warning_message` callback with `status.lastwarningtag`.
#[test]
fn warning_format() {
    let got = lines("", "\\directlua{function P(tag, s) s = tostring(s) for i = 1, math.max(1, string.len(s)), 30 do texio.write_nl(tag..' '..s:sub(i, i + 29)..'@') end end}\n\\scrollmode\n\\directlua{tex.permitmathobsolete(true)}\n\\mathitalicsmode=1\n\\mathitalicsmode=1\n\\mathnolimitsmode=2\n\\directlua{callback.register('show_warning_message', function() P('WM', status.lastwarningtag..'|'..status.lastwarningstring) end)}\n\\mathscriptcharmode=0\n\\directlua{tex.permitmathobsolete(false)}\n\\mathitalicsmode=0\n\\directlua{P('MODE', tex.mathitalicsmode or 'x')}", &["warning", "WM ", "MODE "], true);
    assert_eq!(
        got,
        [
            "warning  (math): obsolete commands are permitted",
            "warning  (math): \\mathitalicsmode is obsolete",
            "warning  (math): \\mathnolimitssmode is obsolete",
            "WM math|\\mathscriptcharmode is ob@",
            "WM solete@",
            "WM math|obsolete commands are blo@",
            "WM cked@",
            "MODE 1@",
        ]
    );
}

/// `status.lasterrorstring` is the text `print_err` was given.
#[test]
fn last_error_string() {
    let got = lines("", "\\directlua{function P(tag, s) s = tostring(s) for i = 1, math.max(1, string.len(s)), 30 do texio.write_nl(tag..' '..s:sub(i, i + 29)..'@') end end}\n\\scrollmode\n\\def\\p#1.{[#1]}\n\\undefinedcs \\directlua{P('LAST', status.lasterrorstring)}\n\\errmessage{some text} \\directlua{P('LAST', status.lasterrorstring)}\n\\p abc\n\n\\directlua{P('LAST', status.lasterrorstring)}\n\\count1=300000000000 \\directlua{P('LAST', status.lasterrorstring)}\n\\hbox{\\vskip 1pt} \\directlua{P('LAST', status.lasterrorstring)}", &["LAST "], false);
    assert_eq!(
        got,
        [
            "LAST Undefined control sequence@",
            "LAST @",
            "LAST Paragraph ended before @",
            "LAST Number too big@",
            "LAST Too many }'s@",
        ]
    );
}
