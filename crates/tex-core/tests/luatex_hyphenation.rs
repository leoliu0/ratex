//! LuaTeX hyphenation parameters (`texlang.c` `hnj_hyphenation`, `\hyphenation`
//! exceptions, `set_automatic_disc_penalty`, `append_discretionary`) on a
//! Lua-defined font. Every expected list was produced by `luatex --ini`
//! (TeX Live 2026) running the same input; the paragraph is hyphenated
//! before the first line-breaking pass (`\pretolerance=-1`) and the glyph,
//! kern and discretionary nodes of the line are compared as text.

use tex_core::engine::{Engine, EngineKind, InteractionMode};

const PREAMBLE: &str = r#"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\directlua{tex.enableprimitives("",tex.extraprimitives())}
\directlua{
local t = font.read_tfm("cmr10", 655360)
t.name = "mycmr"
t.hyphenchar = 45
font.current(font.define(t))
local function push(t, v) table.insert(t, v) end
local function walk(h, out)
  for n in node.traverse(h) do
    local ty = node.type(n.id)
    if ty == "glyph" then
      local s = utf8.char(n.char)
      if n.subtype ~= 1 and n.subtype ~= 0 then s = s .. "/" .. n.subtype end
      push(out, s)
    elseif ty == "disc" then
      local p, q, r = {}, {}, {}
      if n.pre then walk(n.pre, p) end
      if n.post then walk(n.post, q) end
      if n.replace then walk(n.replace, r) end
      push(out, "[" .. n.subtype .. ":" .. n.penalty .. "<" .. table.concat(p, " ") .. "|" .. table.concat(q, " ") .. "|" .. table.concat(r, " ") .. ">]")
    elseif ty == "kern" then
      push(out, "k" .. n.subtype .. "." .. n.kern)
    elseif ty == "glue" then
      push(out, "_")
    elseif ty == "penalty" then
      push(out, "P" .. n.penalty)
    elseif ty == "hlist" or ty == "vlist" then
      local o = {}
      walk(n.head, o)
      push(out, ty:sub(1,1) .. "{" .. table.concat(o, " ") .. "}")
    elseif ty == "rule" then
      push(out, "R")
    elseif ty == "local_par" or ty == "whatsit" then
    else
      push(out, "(" .. ty .. ")")
    end
  end
end
function check(name, expected)
  local line = tex.box[0].head
  while line and node.type(line.id) ~= "hlist" do line = line.next end
  local out = {}
  walk(line.head, out)
  local got = table.concat(out, " ")
  if got ~= expected then
    texio.write_nl("MISMATCH " .. name)
    texio.write_nl("  expected " .. expected)
    texio.write_nl("  got      " .. got)
  end
end
}
\lccode`a=`a \lccode`b=`b \lccode`c=`c \lccode`d=`d \lccode`e=`e \lccode`-=`-
\patterns{a1b b1c c1d d1e}
\lefthyphenmin1 \righthyphenmin1
\hsize=1000pt \parindent=0pt \pretolerance=-1 \hbadness10000 \hfuzz16000pt \vbadness10000 \tolerance10000
\def\T#1{\setbox0\vbox{#1\par}\directlua{check(NAME, EXPECTED)}}
"#;

/// Run one case in a fresh engine (language parameters, hjcodes and
/// exceptions are global in LuaTeX) and return the transcript.
fn run_case(name: &str, body: &str, expected: &str) -> (Engine, String) {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.add_nullfont();
    e.set_interaction_mode(InteractionMode::Nonstop);
    let src = format!(
        "{PREAMBLE}\\directlua{{NAME = \"{name}\" EXPECTED = [=====[{expected}]=====]}}\n{body}\n\\end\n"
    );
    e.input.push_file("t.tex".to_string(), src.into_bytes());
    e.run();
    let term = e.term.clone();
    (e, term)
}

fn check(cases: &[(&str, &str, &str)]) {
    let mut bad = Vec::new();
    for (name, body, expected) in cases {
        let (e, term) = run_case(name, body, expected);
        if term.contains("MISMATCH") || e.error_count != 0 {
            bad.push(format!("{name}: errors {:?}\n{term}", e.diagnostics));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn automatic_hyphen_modes_follow_luatex() {
    check(&[
        ("ahm0w", r#"\automatichyphenmode=0 \T{-abcde ab-cde}"#, r#"h{} [2:0<-||->] a b k0.18205 c d e _ a b [2:0<-||->] c d e P10000 _ _"#),
        ("ahm1w", r#"\automatichyphenmode=1 \T{-abcde ab-cde}"#, r#"h{} - a b k0.18205 c d e _ a b [2:0<-||->] c d e P10000 _ _"#),
        ("ahm2w", r#"\automatichyphenmode=2 \T{-abcde ab-cde}"#, r#"h{} - a b k0.18205 c d e _ a b - c d e P10000 _ _"#),
        ("ahm3", r#"\automatichyphenmode=3 \T{-abcde ab-cde}"#, r#"h{} - a b k0.18205 c d e _ a b - c d e P10000 _ _"#),
        ("a1_chm1b", r#"\T{\compoundhyphenmode=1 \automatichyphenmode=1 ab-cdeab}"#, r#"h{} a b [2:0<-||->] c d e a b P10000 _ _"#),
        ("chm1", r#"\compoundhyphenmode=1 \T{ab-cdeab}"#, r#"h{} a b [2:0<-||->] c d e a b P10000 _ _"#),
        ("chm1h", r#"\compoundhyphenmode=1 \hyphenation{ab=cdeab} \T{ab-cdeab}"#, r#"h{} a b [2:0<-||->] c d e a b P10000 _ _"#),
        ("chm0h", r#"\hyphenation{ab=cdeab} \T{ab-cdeab}"#, r#"h{} a b [2:0<-||->] c d e a b P10000 _ _"#),
        ("chm1l", r#"\compoundhyphenmode=1 \T{abc-de-abcde}"#, r#"h{} a b k0.18205 c [2:0<-||->] d e [2:0<-||->] a b k0.18205 c d e P10000 _ _"#),
        ("dash3", r#"\automatichyphenmode=1 \T{ab---cd}"#, r#"h{} a b |/2 c d P10000 _ _"#),
        ("exend", r#"\T{abcde-}"#, r#"h{} a b k0.18205 c d e - P10000 _ _"#),
    ]);
}

#[test]
fn hyphen_penalty_modes_follow_luatex() {
    check(&[
        ("a1_exp", r#"\T{\exhyphenpenalty=77 ab-cd}"#, r#"h{} a b [2:77<-||->] c d P10000 _ _"#),
        ("a1_exp2", r#"\T{\explicithyphenpenalty=55 ab-cd}"#, r#"h{} a b [2:0<-||->] c d P10000 _ _"#),
        ("a1_exp3", r#"\T{\explicithyphenpenalty=55 \exhyphenpenalty=77 ab-cd}"#, r#"h{} a b [2:77<-||->] c d P10000 _ _"#),
        ("a1_aup", r#"\T{\automatichyphenpenalty=33 ab-cd}"#, r#"h{} a b [2:0<-||->] c d P10000 _ _"#),
        ("a1_hp", r#"\T{\hyphenpenalty=11 abcde}"#, r#"h{} a [3:11<-||>] b [3:11<-||k0.18205>] c [3:11<-||>] d [3:11<-||>] e P10000 _ _"#),
        ("exp0", r#"\hyphenpenaltymode=0 \exhyphenpenalty=3 \hyphenpenalty=4 \explicithyphenpenalty=5 \automatichyphenpenalty=6 \T{ab-cd ab\-cd ab\automaticdiscretionary cd abcde}"#, r#"h{} a b [2:3<-||->] c d _ a b [1:3<-||k0.18205>] c d _ a b [2:3<-||->] c d _ a [3:4<-||>] b [3:4<-||k0.18205>] c [3:4<-||>] d [3:4<-||>] e P10000 _ _"#),
        ("exp1", r#"\hyphenpenaltymode=1 \exhyphenpenalty=3 \hyphenpenalty=4 \explicithyphenpenalty=5 \automatichyphenpenalty=6 \T{ab-cd ab\-cd ab\automaticdiscretionary cd abcde}"#, r#"h{} a b [2:4<-||->] c d _ a b [1:4<-||k0.18205>] c d _ a b [2:4<-||->] c d _ a [3:4<-||>] b [3:4<-||k0.18205>] c [3:4<-||>] d [3:4<-||>] e P10000 _ _"#),
        ("exp2", r#"\hyphenpenaltymode=2 \exhyphenpenalty=3 \hyphenpenalty=4 \explicithyphenpenalty=5 \automatichyphenpenalty=6 \T{ab-cd ab\-cd ab\automaticdiscretionary cd abcde}"#, r#"h{} a b [2:3<-||->] c d _ a b [1:4<-||k0.18205>] c d _ a b [2:3<-||->] c d _ a [3:4<-||>] b [3:4<-||k0.18205>] c [3:4<-||>] d [3:4<-||>] e P10000 _ _"#),
        ("exp3", r#"\hyphenpenaltymode=3 \exhyphenpenalty=3 \hyphenpenalty=4 \explicithyphenpenalty=5 \automatichyphenpenalty=6 \T{ab-cd ab\-cd ab\automaticdiscretionary cd abcde}"#, r#"h{} a b [2:4<-||->] c d _ a b [1:3<-||k0.18205>] c d _ a b [2:4<-||->] c d _ a [3:4<-||>] b [3:4<-||k0.18205>] c [3:4<-||>] d [3:4<-||>] e P10000 _ _"#),
        ("exp4", r#"\hyphenpenaltymode=4 \exhyphenpenalty=3 \hyphenpenalty=4 \explicithyphenpenalty=5 \automatichyphenpenalty=6 \T{ab-cd ab\-cd ab\automaticdiscretionary cd abcde}"#, r#"h{} a b [2:6<-||->] c d _ a b [1:5<-||k0.18205>] c d _ a b [2:6<-||->] c d _ a [3:4<-||>] b [3:4<-||k0.18205>] c [3:4<-||>] d [3:4<-||>] e P10000 _ _"#),
        ("exp5", r#"\hyphenpenaltymode=5 \exhyphenpenalty=3 \hyphenpenalty=4 \explicithyphenpenalty=5 \automatichyphenpenalty=6 \T{ab-cd ab\-cd ab\automaticdiscretionary cd abcde}"#, r#"h{} a b [2:3<-||->] c d _ a b [1:5<-||k0.18205>] c d _ a b [2:3<-||->] c d _ a [3:4<-||>] b [3:4<-||k0.18205>] c [3:4<-||>] d [3:4<-||>] e P10000 _ _"#),
        ("exp6", r#"\hyphenpenaltymode=6 \exhyphenpenalty=3 \hyphenpenalty=4 \explicithyphenpenalty=5 \automatichyphenpenalty=6 \T{ab-cd ab\-cd ab\automaticdiscretionary cd abcde}"#, r#"h{} a b [2:4<-||->] c d _ a b [1:5<-||k0.18205>] c d _ a b [2:4<-||->] c d _ a [3:4<-||>] b [3:4<-||k0.18205>] c [3:4<-||>] d [3:4<-||>] e P10000 _ _"#),
        ("exp7", r#"\hyphenpenaltymode=7 \exhyphenpenalty=3 \hyphenpenalty=4 \explicithyphenpenalty=5 \automatichyphenpenalty=6 \T{ab-cd ab\-cd ab\automaticdiscretionary cd abcde}"#, r#"h{} a b [2:6<-||->] c d _ a b [1:3<-||k0.18205>] c d _ a b [2:6<-||->] c d _ a [3:4<-||>] b [3:4<-||k0.18205>] c [3:4<-||>] d [3:4<-||>] e P10000 _ _"#),
        ("exp8", r#"\hyphenpenaltymode=8 \exhyphenpenalty=3 \hyphenpenalty=4 \explicithyphenpenalty=5 \automatichyphenpenalty=6 \T{ab-cd ab\-cd ab\automaticdiscretionary cd abcde}"#, r#"h{} a b [2:6<-||->] c d _ a b [1:4<-||k0.18205>] c d _ a b [2:6<-||->] c d _ a [3:4<-||>] b [3:4<-||k0.18205>] c [3:4<-||>] d [3:4<-||>] e P10000 _ _"#),
        ("exp9", r#"\hyphenpenaltymode=9 \exhyphenpenalty=3 \hyphenpenalty=4 \explicithyphenpenalty=5 \automatichyphenpenalty=6 \T{ab-cd ab\-cd ab\automaticdiscretionary cd abcde}"#, r#"h{} a b [2:3<-||->] c d _ a b [1:3<-||k0.18205>] c d _ a b [2:3<-||->] c d _ a [3:4<-||>] b [3:4<-||k0.18205>] c [3:4<-||>] d [3:4<-||>] e P10000 _ _"#),
        ("a2_disc2", r#"\T{\hyphenpenalty=33 \exhyphenpenalty=44 \explicithyphenpenalty=55 ab\-cd \hyphenpenaltymode=1 ab\-cd \hyphenpenaltymode=2 ab\-cd}"#, r#"h{} a b [1:44<-||k0.18205>] c d _ a b [1:33<-||k0.18205>] c d _ a b [1:33<-||k0.18205>] c d P10000 _ _"#),
        ("a2_adisc", r#"\T{ab\automaticdiscretionary cd}"#, r#"h{} a b [2:0<-||->] c d P10000 _ _"#),
        ("a2_edisc2", r#"\T{\hyphenpenalty=33 \exhyphenpenalty=44 \explicithyphenpenalty=55 \automatichyphenpenalty=66 ab\automaticdiscretionary cd \explicitdiscretionary x}"#, r#"h{} a b [2:44<-||->] c d _ [1:44<-||>] x P10000 _ _"#),
    ]);
}

#[test]
fn hyphen_characters_follow_luatex() {
    check(&[
        ("a3_pre1", r#"\T{\prehyphenchar=`+ abcde}"#, r#"h{} a [3:0<+||>] b [3:0<+||k0.18205>] c [3:0<+||>] d [3:0<+||>] e P10000 _ _"#),
        ("a3_post1", r#"\T{\posthyphenchar=`+ abcde}"#, r#"h{} a [3:0<-|+|>] b [3:0<-|+|k0.18205>] c [3:0<-|+|>] d [3:0<-|+|>] e P10000 _ _"#),
        ("a3_pre2", r#"\T{\prehyphenchar=`+ \posthyphenchar=`* abcde}"#, r#"h{} a [3:0<+|*|>] b [3:0<+|*|k0.18205>] c [3:0<+|*|>] d [3:0<+|*|>] e P10000 _ _"#),
        ("a3_pre0", r#"\T{\prehyphenchar=0 abcde}"#, r#"h{} a [3:0<||>] b [3:0<||k0.18205>] c [3:0<||>] d [3:0<||>] e P10000 _ _"#),
        ("a3_preex", r#"\T{\preexhyphenchar=`+ ab-cd}"#, r#"h{} a b [2:0<+||->] c d P10000 _ _"#),
        ("a3_postex", r#"\T{\postexhyphenchar=`+ ab-cd}"#, r#"h{} a b [2:0<-|+|->] c d P10000 _ _"#),
        ("a3_preex2", r#"\T{\preexhyphenchar=`+ \postexhyphenchar=`* ab-cd}"#, r#"h{} a b [2:0<+|*|->] c d P10000 _ _"#),
        ("a3_exch", r#"\T{\exhyphenchar=`+ ab+cd ab-cd}"#, r#"h{} a b [2:0<+||+>] c d _ a [3:0<-||>] b - c [3:0<-||>] d P10000 _ _"#),
        ("a3_exch2", r#"\T{\exhyphenchar=`+ \preexhyphenchar=`* ab+cd}"#, r#"h{} a b [2:0<*||+>] c d P10000 _ _"#),
        ("exch0", r#"\exhyphenchar=0 \T{ab-cd}"#, r#"h{} a [3:0<-||>] b - c [3:0<-||>] d P10000 _ _"#),
        ("exchar", r#"\exhyphenchar=`~ \T{ab~cd ab-cd}"#, r#"h{} a b [2:0<~||~>] c d _ a [3:0<-||>] b - c [3:0<-||>] d P10000 _ _"#),
        ("vpre", r#"\prehyphenchar=`+ \posthyphenchar=`* \T{ab\-cd}"#, r#"h{} a b [1:0<+|*|k0.18205>] c d P10000 _ _"#),
        ("vpre2", r#"\preexhyphenchar=`+ \postexhyphenchar=`* \exhyphenchar=`= \T{ab\automaticdiscretionary cd}"#, r#"h{} a b [2:0<+|*|=>] c d P10000 _ _"#),
        ("vprelang_other", r#"\language=3 \prehyphenchar=`+ \language=0 \T{ab\-cd}"#, r#"h{} a b [1:0<-||k0.18205>] c d P10000 _ _"#),
        ("vprelang_set", r#"\language=3 \prehyphenchar=`+ \T{ab\-cd}"#, r#"h{} a b [1:0<+||k0.18205>] c d P10000 _ _"#),
    ]);
}

#[test]
fn hyphenation_limits_follow_luatex() {
    check(&[
        ("a3_hmin", r#"\T{\hyphenationmin=5 abcde abcd}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e _ a b k0.18205 c d P10000 _ _"#),
        ("a3_hmin2", r#"\T{\hyphenationmin=4 abcde abc}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e _ a b k0.18205 c P10000 _ _"#),
        ("a3_hmin3", r#"\T{\hyphenationmin=6 abcde}"#, r#"h{} a b k0.18205 c d e P10000 _ _"#),
        ("a3_lh", r#"\T{\lefthyphenmin2 \righthyphenmin2 abcde}"#, r#"h{} a b [3:0<-||k0.18205>] c [3:0<-||>] d e P10000 _ _"#),
        ("hmin5b", r#"\hyphenationmin=5 \hyphenation{ab-cd} \T{abcd}"#, r#"h{} a b k0.18205 c d P10000 _ _"#),
        ("hmininh", r#"\language=2 \hyphenationmin=9 \language=0 \T{abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("lhm3", r#"\lefthyphenmin=3 \righthyphenmin=3 \T{abcde}"#, r#"h{} a b k0.18205 c d e P10000 _ _"#),
        ("first", r#"\T{abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("mathw", r#"\T{abcde $a$ abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e _ $ a $ _ a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("firstlang", r#"\firstvalidlanguage=1 \T{abcde}"#, r#"h{} a b k0.18205 c d e P10000 _ _"#),
        ("lang2", r#"\T{abcde \language=2 abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e _ a b k0.18205 c d e P10000 _ _"#),
        ("hbr", r#"\hyphenationbounds=2 \T{abcde\hbox{}abcde abcde\vrule width1pt abcde\mark{1}abcde\vadjust{}}"#, r#"h{} a b k0.18205 c d e h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e _ a b k0.18205 c d e R a b k0.18205 c d e a b k0.18205 c d e P10000 _ _"#),
        ("hbboth", r#"\hyphenationbounds=3 \T{abcde\hbox{}abcde \hbox{}abcde\vrule width1pt abcde\penalty0 abcde}"#, r#"h{} a b k0.18205 c d e h{} a b k0.18205 c d e _ h{} a b k0.18205 c d e R a b k0.18205 c d e P0 a b k0.18205 c d e P10000 _ _"#),
        ("hb4", r#"\hyphenationbounds=3 \T{abc\kern1pt de abcde\kern1pt}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c k1.65536 d e _ a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e k1.65536 P10000 _ _"#),
    ]);
}

#[test]
fn hjcode_tables_follow_luatex() {
    check(&[
        ("sh1", r#"\directlua{local l=lang.new(0) for _,c in ipairs{97,98,99,100} do lang.sethjcode(l,c,c) end}\T{abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d e P10000 _ _"#),
        ("sh2", r#"\directlua{local l=lang.new(0) for _,c in ipairs{97,98,99,100,101,65} do lang.sethjcode(l,c,c) end lang.sethjcode(l,65,97)}\uchyph=1 \T{Abcde}"#, r#"h{} A [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("sh3", r#"\directlua{local l=lang.new(0) lang.sethjcode(l,0x3b1,0x3b1)}\T{abcde}"#, r#"h{} a b k0.18205 c d e P10000 _ _"#),
        ("a4_hj0", r#"\T{\hjcode`a=0 abcde}"#, r#"h{} a b k0.18205 c d e P10000 _ _"#),
        ("a4_hj3", r#"\T{\hjcode`a=0 \hjcode`b=0 abcde bcdea}"#, r#"h{} a b k0.18205 c d e _ b k0.18205 c d e a P10000 _ _"#),
        ("a4_hj7", r#"\T{\hjcode`A=`a ABCDE}"#, r#"h{} A B C D E P10000 _ _"#),
        ("hjall", r#"\hjcode`a=`a \hjcode`b=`b \hjcode`c=`c \hjcode`d=`d \hjcode`e=`e \T{abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("hjc0", r#"\hjcode`a=`a \hjcode`b=`b \hjcode`c=0 \hjcode`d=`d \hjcode`e=`e \T{abcde}"#, r#"h{} a [3:0<-||>] b k0.18205 c d [3:0<-||>] e P10000 _ _"#),
        ("hjuc0", r#"\hjcode`a=`a \hjcode`b=`b \hjcode`c=`c \hjcode`d=`d \hjcode`e=`e \hjcode`A=`a \hjcode`B=`b \uchyph=0 \T{ABcde aBcde}"#, r#"h{} A B c d e _ a [3:0<-||>] B [3:0<-||>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("hjuc1", r#"\hjcode`a=`a \hjcode`b=`b \hjcode`c=`c \hjcode`d=`d \hjcode`e=`e \hjcode`A=`a \hjcode`B=`b \uchyph=1 \T{ABcde aBcde}"#, r#"h{} A [3:0<-||>] B [3:0<-||>] c [3:0<-||>] d [3:0<-||>] e _ a [3:0<-||>] B [3:0<-||>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("hjsmall", r#"\hjcode`a=`a \hjcode`b=1 \hjcode`c=`c \hjcode`d=`d \hjcode`e=`e \T{abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("hjsmall0", r#"\hjcode`a=`a \hjcode`b=32 \hjcode`c=`c \hjcode`d=`d \hjcode`e=`e \T{abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d e P10000 _ _"#),
        ("hjlang", r#"\hjcode`a=`a \language=1 \T{abcde}"#, r#"h{} a b k0.18205 c d e P10000 _ _"#),
        ("lcA", r#"\lccode`A=`a \lccode`B=`b \T{ABcde abcde}"#, r#"h{} A B c d e _ a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("lc0", r#"\lccode`c=0 \T{abcde}"#, r#"h{} a [3:0<-||>] b k0.18205 c d [3:0<-||>] e P10000 _ _"#),
    ]);
}

#[test]
fn exceptions_follow_luatex() {
    check(&[
        ("a5_ex1", r#"\T{\hyphenation{ab-cde} abcde}"#, r#"h{} a b [3:0<-||k0.18205>] c d e P10000 _ _"#),
        ("a5_ex2", r#"\T{\hyphenation{a-b-c-d-e} abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("a5_ex3", r#"\T{\hyphenation{a{x}{y}{z}bcde} abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("a5_ex4", r#"\T{\hyphenation{a{x}{y}{}bcde} abcde}"#, r#"h{} a [3:0<x|y|>] b k0.18205 c d e P10000 _ _"#),
        ("a5_ex5", r#"\T{\hyphenation{a{x}{y}{z}[3]bcde} abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("a5_ex6", r#"\T{\exceptionpenalty=7 \hyphenation{a{x}{y}{z}[3]bcde} abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("a5_ex7", r#"\T{\exceptionpenalty=7 \hyphenation{a{x}{y}{z}bcde} abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("a5_ex8", r#"\T{\exceptionpenalty=20000 \hyphenation{a{x}{y}{z}[3]bcde} abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("a5_ex9", r#"\T{\exceptionpenalty=7 \hyphenation{a-bc-de} abcde}"#, r#"h{} a [3:0<-||>] b k0.18205 c [3:0<-||>] d e P10000 _ _"#),
        ("a5_ex10", r#"\T{\hyphenation{a=bc-de} abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("a5_ex11", r#"\T{\hyphenpenalty=9 \exceptionpenalty=7 \hyphenation{ab-cde} abcde}"#, r#"h{} a b [3:9<-||k0.18205>] c d e P10000 _ _"#),
        ("exr1", r#"\exceptionpenalty=7 \hyphenation{a{x}{y}{z}[3]bcde} \T{abcde}"#, r#"h{} a [3:0<-||>] b [3:0<-||k0.18205>] c [3:0<-||>] d [3:0<-||>] e P10000 _ _"#),
        ("exr2", r#"\exceptionpenalty=7 \hyphenation{ab{x}{y}{}[3]cde} \T{abcde}"#, r#"h{} a b [3:21<x|y|k0.18205>] c d e P10000 _ _"#),
        ("exr3", r#"\hyphenation{ab{x}{y}{cd}e} \T{abcde}"#, r#"h{} a b [3:0<x|y k0.-18205|c d>] e P10000 _ _"#),
        ("exr4", r#"\hyphenation{a{x}{y}{}b{x}{y}{}cde} \T{abcde}"#, r#"h{} a [3:0<x|y|>] b [3:0<x|y|k0.18205>] c d e P10000 _ _"#),
        ("exr5", r#"\hyphenation{a{-}{-}{}b-cde} \T{abcde}"#, r#"h{} a [3:0<-|-|>] b [3:0<-||k0.18205>] c d e P10000 _ _"#),
        ("exr6", r#"\hyphenation{a{x}{y}{b}cde} \T{abcde}"#, r#"h{} a [3:0<x|y|b k0.18205>] c d e P10000 _ _"#),
        ("exr7", r#"\hyphenation{a{x}{y}{}[3]{p}{q}{}bcde} \T{abcde}"#, r#"h{} a [3:0<x|y|>] [3:0<p|q|>] b k0.18205 c d e P10000 _ _"#),
        ("exr8", r#"\exceptionpenalty=20000 \hyphenation{ab{x}{y}{}[3]cde} \T{abcde}"#, r#"h{} a b [3:20000<x|y|k0.18205>] c d e P10000 _ _"#),
        ("exr9", r#"\exceptionpenalty=0 \hyphenpenalty=9 \hyphenation{ab{x}{y}{}[3]cde} \T{abcde}"#, r#"h{} a b [3:9<x|y|k0.18205>] c d e P10000 _ _"#),
        ("k1", r#"\hyphenation{ab{x}{y}{}cde} \T{abcde}"#, r#"h{} a b [3:0<x|y|k0.18205>] c d e P10000 _ _"#),
        ("k4", r#"\hyphenation{ab{x}{-}{}cde} \T{abcde}"#, r#"h{} a b [3:0<x|-|k0.18205>] c d e P10000 _ _"#),
        ("k7", r#"\hyphenation{ab{c}{c}{}cde} \T{abcde}"#, r#"h{} a b [3:0<c|c|k0.18205>] c d e P10000 _ _"#),
        ("k2", r#"\T{ab\discretionary{x}{y}{}cde}"#, r#"h{} a b [0:0<k0.-18205 x|y|k0.18205>] c d e P10000 _ _"#),
    ]);
}

#[test]
fn discretionary_ligatures_follow_luatex() {
    check(&[
        ("ligmode0", r#"\discretionaryligaturemode=0 \patterns{o1f f1f f1i i1c c1e} \lccode`o=`o \lccode`f=`f \lccode`i=`i \T{office}"#, r#"h{} o [3:0<-||>] [4:0<f -|/2|/2>] [5:0<f -|i|/2 ->] [3:0<-||>] c [3:0<-||>] e P10000 _ _"#),
        ("ligmode1", r#"\discretionaryligaturemode=1 \patterns{o1f f1f f1i i1c c1e} \lccode`o=`o \lccode`f=`f \lccode`i=`i \T{office}"#, r#"h{} o [3:0<-||>] [3:0<f -|/2|/2>] [3:0<-||>] c [3:0<-||>] e P10000 _ _"#),
        ("ligmode2", r#"\discretionaryligaturemode=2 \patterns{o1f f1f f1i i1c c1e} \lccode`o=`o \lccode`f=`f \lccode`i=`i \T{office}"#, r#"h{} o [3:0<-||>] [3:0</2 -|i|/2>] [3:0<-||>] c [3:0<-||>] e P10000 _ _"#),
        ("ligmode3", r#"\discretionaryligaturemode=3 \patterns{o1f f1f f1i i1c c1e} \lccode`o=`o \lccode`f=`f \lccode`i=`i \T{office}"#, r#"h{} o [3:0<-||>] [4:0<f -|/2|/2>] [5:0<f -|i|/2 ->] [3:0<-||>] c [3:0<-||>] e P10000 _ _"#),
    ]);
}

/// `hnj_serialize` walks the buckets of the pattern hash table of hyphen.c:
/// the order is that of `hnj_string_hash(word) % 31627` (expected from
/// `luatex --ini`), not alphabetical.
#[test]
fn lang_patterns_come_back_in_luatex_hash_order() {
    check_lua(
        r#"lang.patterns(lang.new(5), "a1b b1c .ab4c 1x2y 5ing. 3te")
           assert(lang.patterns(lang.new(5)) == "a1b b1c 3te 1x2y 5ing. .ab4c ", lang.patterns(lang.new(5)))
           lang.patterns(lang.new(5), "ca1b 2lm")
           assert(lang.patterns(lang.new(5)) == "a1b b1c 2lm 3te 1x2y 5ing. .ab4c ca1b ", lang.patterns(lang.new(5)))
           lang.patterns(lang.new(6), "bc1d xa1 .a2 1a.")
           assert(lang.patterns(lang.new(6)) == ".a2 1a. xa1 bc1d ", lang.patterns(lang.new(6)))"#,
    );
}

/// Exceptions keep what was written, `=` and `{pre}{post}{replace}[n]`
/// included (luatex returns the words in table order; they are compared
/// as a set).
#[test]
fn lang_hyphenation_keeps_exceptions_as_written() {
    check_lua(
        r#"local l = lang.new(7)
           lang.hyphenation(l, "a{x}{y}{z}bcd ab=cd ta-ble")
           local set = {}
           for w in lang.hyphenation(l):gmatch("[^ ]+") do set[w] = true end
           assert(set["ta-ble"] and set["a{x}{y}{z}bcd"] and set["ab=cd"])
           local n = 0 for _ in pairs(set) do n = n + 1 end
           assert(n == 3)
           lang.clear_hyphenation(l)
           assert(lang.hyphenation(l) == nil or lang.hyphenation(l) == "")"#,
    );
}

fn check_lua(code: &str) {
    let (e, term) = run_case("lua", &format!("\\directlua{{{code}}}"), "");
    assert!(e.error_count == 0 && !term.contains("MISMATCH") && !term.contains("error"), "{term}\n{:?}", e.diagnostics);
}
