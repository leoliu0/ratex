//! LuaTeX-only primitives wired into the engine: `\nospaces`, the `\suppress*error`
//! family, `\outputbox`, `\alignmark`/`\aligntab`, the meaning of `\topmark` and
//! friends, and `\endlocalcontrol`. Expected values are `luatex --ini`
//! (TeX Live 2026) output for the same input.

use tex_core::engine::{Engine, EngineKind, InteractionMode};

fn run(body: &str) -> Vec<String> {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.set_interaction_mode(InteractionMode::Nonstop);
    let pre = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\catcode`\\$=3 \\catcode`\\^=7 \\catcode`\\_=8\n\
\\directlua{tex.enableprimitives(\"\",tex.extraprimitives())}\n\
\\long\\def\\u#1{\\immediate\\write16{[#1]}}\n";
    e.input.push_file("t.tex".to_string(), format!("{pre}{body}\n\\end\n").into_bytes());
    e.run();
    e.term
        .lines()
        .filter(|l| l.starts_with('[') || l.starts_with("local control"))
        .map(str::to_string)
        .collect()
}

#[test]
fn nospaces_drops_or_zeroes_interword_glue() {
    let out = run(
        "\\setbox0\\hbox{\\spaceskip=5pt a b\\ c}\\u{\\the\\wd0}\n\
\\nospaces=1 \\setbox0\\hbox{\\spaceskip=5pt a b\\ c}\\u{\\the\\wd0}\n\
\\nospaces=2 \\setbox0\\hbox{\\spaceskip=5pt a b\\ c}\\u{\\the\\wd0}",
    );
    assert_eq!(out, ["[10.0pt]", "[0.0pt]", "[0.0pt]"]);
}

#[test]
fn suppress_error_parameters_silence_the_real_sites() {
    let out = run(
        "\\suppressifcsnameerror=1 \\u{\\ifcsname a\\relax b\\endcsname T\\else F\\fi \\ifcsname relax\\endcsname T\\else F\\fi}\n\
\\suppressifcsnameerror=0\n\
\\def\\b#1.{<#1>}\\suppresslongerror=1 \\u{\\b x\\par y.} \\suppresslongerror=0\n\
\\outer\\def\\o{O}\\suppressoutererror=1 \\def\\c#1{<#1>}\\u{\\c\\o}\\u{\\meaning\\c}\n\
\\suppressoutererror=0\n\
\\suppressprimitiveerror=1 \\primitive\\foo \\u{ok}",
    );
    assert_eq!(out, ["[FT]", "[<x\\par y>]", "[<O>]", "[macro:#1-><#1>]", "[ok]"]);
}

#[test]
fn alignmark_and_aligntab_have_their_own_meaning() {
    let out = run(
        "\\u{\\meaning\\alignmark|\\meaning\\aligntab}\n\
\\edef\\c{\\alignmark\\alignmark\\aligntab}\\u{\\meaning\\c}\n\
\\setbox0\\vbox{\\halign{[\\alignmark]\\aligntab(\\alignmark)\\cr a\\aligntab b\\cr}}\\u{\\the\\wd0}",
    );
    assert_eq!(out[0], "[\\alignmark|\\aligntab]");
    assert_eq!(out[1], "[macro:->\\alignmark \\aligntab ]");
}

#[test]
fn outputbox_and_mark_meanings() {
    let out = run(
        "\\u{\\the\\outputbox}\\outputbox=7 \\u{\\the\\outputbox}\n\
\\u{\\meaning\\topmark|\\meaning\\firstmark|\\meaning\\splitbotmark}",
    );
    assert_eq!(out, ["[255]", "[7]", "[\\topmark:|\\firstmark:|\\splitbotmark:]"]);
}

#[test]
fn endlocalcontrol_ends_runtoks_and_complains_when_redundant() {
    let out = run(
        "\\toks0={\\u{in}\\endlocalcontrol\\u{after}}\n\
\\directlua{tex.runtoks(0)}\n\
\\u{\\directlua{tex.print(tex.getlocallevel())}}\n\
\\endlocalcontrol\n\
\\u{end}",
    );
    assert_eq!(
        out,
        [
            "[in]",
            "[after]",
            "local control level 0: redundant end local control",
            "[0]",
            "local control level 0: redundant end local control",
            "[end]"
        ]
    );
}

#[test]
fn moveleft_raise_and_their_aliases_keep_their_direction() {
    let out = run(
        "\\let\\x\\moveright \\let\\y\\raise \\let\\z\\moveleft \\let\\w\\lower\n\
\\setbox1\\vbox{\\x 5pt\\hbox{}\\z 5pt\\hbox{}}\\setbox2\\hbox{\\y 5pt\\hbox{}\\w 5pt\\hbox{}}\n\
\\u{\\the\\wd1,\\the\\ht2,\\the\\dp2}\n\
\\u{\\meaning\\x \\meaning\\z \\meaning\\y \\meaning\\w \\meaning\\moveright \\meaning\\raise}",
    );
    assert_eq!(out, ["[5.0pt,5.0pt,5.0pt]", "[\\moveright\\moveleft\\raise\\lower\\moveright\\raise]"]);
}
