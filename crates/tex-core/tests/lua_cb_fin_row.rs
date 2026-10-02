//! LuaTeX's `fin_row` and `fin_align` with Lua-shaped alignment rows: what
//! `hpack_filter`/`vpack_filter` (group `fin_row`, `align_set`, `preamble`)
//! and `append_to_vlist_filter` (location `alignment`) leave behind is what
//! the alignment's list holds, and the unset cells of the row are set from
//! the final column widths.
//!
//! Every expectation was produced by running the same source with TeX Live
//! 2026 `luatex -ini -interaction=nonstopmode -jobname=case t.tex` (`t.tex`:
//! the catcode line, the `\directlua` calls of `LIB`, the case's callbacks,
//! the body and `\end`). `SHOW` prints one line per node (type/subtype,
//! sizes in sp, glue set times 1000); `MARK` lines carry `prev_depth` and the
//! callback arguments.

use tex_core::engine::{Engine, EngineKind};

const LIB: &str = r##"function SHOW(n, d)
  d = d or 0
  for x in node.traverse(n) do
    local id = x.id
    local s = string.rep('.', d) .. node.type(id)
    if id == node.id('hlist') or id == node.id('vlist') or id == node.id('unset') then
      s = s .. '/' .. x.subtype .. ' ' .. x.width .. ' ' .. x.height .. ' ' .. x.depth
      if id ~= node.id('unset') then
        s = s .. ' sh=' .. x.shift .. ' gs=' .. x.glue_sign .. ' go=' .. x.glue_order .. ' set=' .. math.floor(x.glue_set * 1000 + 0.5)
      end
    elseif id == node.id('rule') then
      s = s .. ' ' .. x.width .. ' ' .. x.height .. ' ' .. x.depth
    elseif id == node.id('glue') then
      s = s .. '/' .. x.subtype .. ' ' .. x.width .. ' ' .. x.stretch .. ' ' .. x.shrink
    elseif id == node.id('kern') then
      s = s .. ' ' .. x.kern
    elseif id == node.id('penalty') then
      s = s .. ' ' .. x.penalty
    end
    texio.write_nl('R ' .. s .. '@@')
    if x.head then SHOW(x.head, d + 1) end
  end
end
function RULE(w, h, d) local r = node.new('rule') r.width = w r.height = h r.depth = d return r end
function PEN(p) local n = node.new('penalty') n.penalty = p return n end
function GLUE(w) local g = node.new('glue') g.width = w return g end
function MARK(s) texio.write_nl('R ' .. s .. '@@') end"##;
const BODY_H: &str = r##"\tabskip 1pt plus 2pt minus 1pt \baselineskip 12pt \lineskip 1pt \lineskiplimit 0pt \maxdepth 4pt \hbadness10000 \vbadness10000
\setbox3\vbox{\hbox to 4pt{\vrule height 2pt depth 3pt}\halign to 50pt{\hbox to 6pt{\hss}#\hfil\tabskip 3pt plus 1fil&\hfil#\tabskip 2pt\cr \vrule height 5pt depth 1pt width 3pt&\kern 2pt\vrule height 3pt depth 2pt\cr \omit\span\omit\kern 20pt\cr \noalign{\hrule}\hbox to 7pt{}\hskip 0pt plus 1fil\vrule height 8pt&\cr}\directlua{MARK('PD1 \the\prevdepth')}\hbox to 5pt{}\directlua{MARK('PD2 \the\prevdepth')}}
\directlua{SHOW(tex.box[3])}"##;
const BODY_V: &str = r##"\tabskip 1pt plus 2pt minus 1pt \baselineskip 12pt \maxdepth 4pt \hbadness10000 \vbadness10000
\setbox3\hbox{\valign to 40pt{\kern 3pt#\vfil\tabskip 3pt plus 1fil&\vfil#\tabskip 2pt\cr \hbox to 4pt{}&\hbox to 5pt{\vrule height 6pt}\cr \noalign{\vrule width 1pt}\hbox to 3pt{}\vskip 2pt\hbox to 7pt{}&\hbox to 2pt{}\cr}}
\directlua{SHOW(tex.box[3])}"##;
const BODY_N: &str = r##"\tabskip 1pt plus 2pt \baselineskip 12pt \lineskip 1pt \lineskiplimit 0pt \hbadness10000 \vbadness10000
\setbox3\vbox{\halign{#&#\cr \kern 2pt&\vbox{\halign{#\cr \kern 3pt\cr \kern 4pt\cr}}\cr \kern 5pt&\cr}}
\directlua{SHOW(tex.box[3])}"##;
const BODY_D: &str = r##"\tabskip 1pt plus 2pt \baselineskip 12pt \lineskip 1pt \lineskiplimit 0pt \hbadness10000 \vbadness10000 \hsize 100pt \parfillskip 0pt plus 1fil \displayindent 7pt \abovedisplayskip 3pt \belowdisplayskip 4pt \abovedisplayshortskip 1pt \belowdisplayshortskip 2pt
\setbox3\vbox{\noindent\hbox to 3pt{}$$\halign{#&#\cr \kern 2pt&\kern 3pt\cr \noalign{\kern 4pt}\kern 5pt&\cr}$$}
\directlua{SHOW(tex.box[3])}"##;
const BODY_T: &str = r##"\tabskip 1pt plus 2pt \baselineskip 12pt \hsize 100pt \hbadness10000 \vbadness10000
\setbox1\vbox{\halign to 60pt{\hfil#\tabskip 3pt&\kern 2pt#\hfil&\vrule#\cr \kern 1pt&\kern 2pt\kern 3pt&\cr \omit\span\omit\kern 10pt\cr \noalign{\hbox{\kern 5pt}}\kern 4pt\span\kern 3pt&\omit\cr \noalign{\hrule}\kern 7pt&\vbox{\halign{#\cr \kern 3pt\cr}}\cr}}
\setbox2\vbox{\noindent\hbox{\kern 3pt}$$\halign{#&#\cr \kern 2pt&\kern 3pt\cr \noalign{\kern 4pt}\kern 5pt&\cr}$$}
\setbox3\hbox{\valign{#\vfil&\vfil#\cr \hbox{\kern 3pt}&\kern 4pt\cr \noalign{\vrule width 1pt}\vskip 2pt\hbox{\kern 1pt}&\cr}}"##;
const CB_PASS: &str = r##"callback.register('hpack_filter', function(h) return true end)
callback.register('append_to_vlist_filter', function(b) return b end)"##;
const CB_AFTER: &str = r##"callback.register('append_to_vlist_filter', function(b, loc, pd, m)
if loc ~= 'alignment' then return b end
local r = RULE(-1073741824, 65536, 0) local p = PEN(77) local g = GLUE(131072)
b.next = r r.next = p p.next = g
return b, 123456
end)"##;
const CB_AFTERD: &str = r##"callback.register('append_to_vlist_filter', function(b, loc, pd, m)
if loc ~= 'alignment' then return b end
b.next = GLUE(131072)
return b
end)"##;
const CB_BEFORE: &str = r##"callback.register('append_to_vlist_filter', function(b, loc, pd, m)
if loc ~= 'alignment' then return b end
local g = GLUE(196608) local k = PEN(55)
g.next = k k.next = b
return g, pd
end)"##;
const CB_REPLACE: &str = r##"callback.register('append_to_vlist_filter', function(b, loc, pd, m)
if loc ~= 'alignment' then return b end
local nb = node.new('hlist') nb.width = 1310720 nb.height = 327680 nb.depth = 131072
return nb, 131072
end)"##;
const CB_DROP: &str = r##"ROWS = 0
callback.register('append_to_vlist_filter', function(b, loc, pd, m)
if loc ~= 'alignment' then return b end
ROWS = ROWS + 1
if math.fmod(ROWS, 2) == 0 then return nil end
return b
end)"##;
const CB_PD: &str = r##"callback.register('append_to_vlist_filter', function(b, loc, pd, m)
if loc ~= 'alignment' then return b end
return b, -65536000
end)"##;
const CB_WIDTH: &str = r##"callback.register('hpack_filter', function(h, g, s, t)
if g ~= 'fin_row' then return true end
for x in node.traverse(h) do
if x.id == node.id('unset') then x.width = x.width + 262144 x.height = x.height + 65536 end
end
return h
end)"##;
const CB_WIDTH2: &str = r##"callback.register('append_to_vlist_filter', function(b, loc, pd, m)
if loc ~= 'alignment' then return b end
b.height = 589824 b.depth = 196608
for x in node.traverse(b.head) do
if x.id == node.id('unset') then x.width = x.width + 262144 end
end
return b
end)"##;
const CB_ROWDIM: &str = r##"callback.register('append_to_vlist_filter', function(b, loc, pd, m)
if loc ~= 'alignment' then return b end
b.shift = 458752 b.width = 196608
return b
end)"##;
const CB_TOTALS: &str = r##"callback.register('hpack_filter', function(h, g, s, t)
if g ~= 'fin_row' then return true end
for x in node.traverse(h) do
if x.id == node.id('unset') then x.stretch = 131072 x.glue_order = 0 x.shrink = 65536 x.glue_sign = 0 end
end
return h
end)"##;
const CB_CELLCONTENT: &str = r##"callback.register('hpack_filter', function(h, g, s, t)
if g ~= 'fin_row' then return true end
for x in node.traverse(h) do
if x.id == node.id('unset') and x.head then
local k = node.new('kern') k.kern = 196608
x.head = node.insert_before(x.head, x.head, k)
end
end
return h
end)"##;
const CB_ROWGLUE: &str = r##"callback.register('hpack_filter', function(h, g, s, t)
if g ~= 'fin_row' then return true end
for x in node.traverse(h) do
if x.id == node.id('glue') then x.width = x.width + 131072 end
end
return h
end)"##;
const CB_VROW: &str = r##"callback.register('vpack_filter', function(h, g, s, t, md, d)
if g == 'fin_row' then
for x in node.traverse(h) do
if x.id == node.id('unset') then x.height = x.height + 131072 x.width = x.width + 65536 end
end
end
return h
end)"##;
const CB_VCELL: &str = r##"callback.register('vpack_filter', function(h, g, s, t, md, d)
if g == 'align_set' then
local last = node.tail(h)
local gl = GLUE(393216)
last.next = gl gl.prev = last
return h
end
return true
end)"##;
const CB_VPRE: &str = r##"callback.register('vpack_filter', function(h, g, s, t, md, d)
if g == 'preamble' then
local last = node.tail(h)
local gl = GLUE(393216)
last.next = gl gl.prev = last
return h
end
return true
end)"##;
const CB_TRACE: &str = r##"callback.register('hyphenate', function(h, t) MARK('hyphenate') end)
callback.register('ligaturing', function(h, t) MARK('ligaturing') end)
callback.register('kerning', function(h, t) MARK('kerning') end)
callback.register('hpack_filter', function(h, g, s, t, d, a) MARK('H ' .. g .. ' ' .. s .. ' ' .. t .. ' ' .. tostring(d) .. ' ' .. tostring(a)) return true end)
callback.register('vpack_filter', function(h, g, s, t, md, d, a) MARK('V ' .. g .. ' ' .. s .. ' ' .. t .. ' ' .. tostring(md) .. ' ' .. tostring(d) .. ' ' .. tostring(a)) return true end)
callback.register('append_to_vlist_filter', function(b, l, pd, m) MARK('A ' .. l .. ' ' .. pd .. ' ' .. tostring(m)) return b end)"##;
const CB_TEXTONLY: &str = r##"callback.register('ligaturing', function(h, t) MARK('ligaturing') end)"##;
const CB_BADBOX: &str = r##"callback.register('hpack_filter', function(h, g, s, t)
if g ~= 'fin_row' then return true end
local n = h.next
n.prev = nil
return n
end)"##;
const CB_HNIL: &str = r##"callback.register('hpack_filter', function(h, g, s, t)
if g == 'fin_row' then return nil end
return true
end)"##;
const CB_VNIL: &str = r##"callback.register('vpack_filter', function(h, g, s, t, md, d)
if g == 'fin_row' then return nil end
return true
end)"##;

/// The tests run in parallel inside one process; the engine's resident-memory
/// cap is measured on the whole process, so disable it.
fn pin_environment() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::env::set_var("TEX_MEM_LIMIT_MIB", "0"));
}

fn run(callbacks: &str, body: &str) -> Engine {
    pin_environment();
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.add_nullfont();
    for (c, cat) in [(b'{', 1), (b'}', 2), (b'#', 6), (b'&', 4), (b'$', 3), (b' ', 10), (b'\n', 5)] {
        e.eqtb.cat[c as usize] = cat;
    }
    e.set_interaction_mode(tex_core::engine::InteractionMode::Nonstop);
    e.job_name = "job".to_string();
    e.input.push_file(
        "t.tex".to_string(),
        format!(
            "\\directlua{{tex.enableprimitives('',tex.extraprimitives('luatex'))}}\n\\directlua{{{LIB}}}\n\\directlua{{{callbacks}}}\n{body}\n\\end\n"
        )
        .into_bytes(),
    );
    e.run();
    e.finish_job_diagnostics();
    e
}

/// The `SHOW`/`MARK` lines of the terminal.
fn shown(e: &Engine) -> Vec<String> {
    e.term
        .lines()
        .filter_map(|l| l.strip_prefix("R "))
        .filter_map(|l| l.find("@@").map(|end| l[..end].to_string()))
        .collect()
}

fn check(callbacks: &str, body: &str, expected: &[&str]) {
    let e = run(callbacks, body);
    assert_eq!(e.error_count, 0, "{:?}\n{}", e.diagnostics, e.term);
    let got = shown(&e);
    assert_eq!(got, expected, "{}", got.join("\n"));
}

fn check_fatal(callbacks: &str, body: &str) {
    let e = run(callbacks, body);
    assert!(
        e.diagnostics.iter().any(|d| d.message.contains("error:  (alignment): bad box")),
        "{:?}\n{}",
        e.diagnostics,
        e.term
    );
    assert!(shown(&e).is_empty(), "{}", e.term);
}

/// A pass-through `hpack_filter`/`append_to_vlist_filter` leaves the alignment as without callbacks.
#[test]
fn rows_roundtrip_unchanged_through_lua() {
    check(
        CB_PASS,
        BODY_H,
        &[
            "PD1 -1000.0pt",
            "PD2 -1000.0pt",
            "vlist/0 3276800 1336934 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".hlist/4 3276800 327680 131072 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 327680 131072 sh=0 gs=1 go=2 set=4400",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...rule 196608 327680 65536",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 327680 131072 sh=0 gs=1 go=2 set=1200",
            "...glue/0 0 65536 0",
            "...kern 131072",
            "...rule 26214 196608 131072",
            "..glue/12 131072 0 0",
            ".hlist/4 3276800 0 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 0 0 sh=0 gs=1 go=0 set=0",
            "...kern 1310720",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
            ".rule 3276800 26214 0",
            ".hlist/4 3276800 524288 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 524288 0 sh=0 gs=0 go=0 set=0",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...rule 26214 524288 -1073741824",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 524288 0 sh=0 gs=1 go=2 set=3600",
            "...glue/0 0 65536 0",
            "..glue/12 131072 0 0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// The nodes `append_to_vlist_filter` returns after the row stay in the alignment; the running rule is extended to the alignment width and `prev_depth` follows the callback.
#[test]
fn append_filter_adds_rule_penalty_glue_after_each_row() {
    check(
        CB_AFTER,
        BODY_H,
        &[
            "PD1 1.88379pt",
            "PD2 1.88379pt",
            "vlist/0 3276800 1926758 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".hlist/4 3276800 327680 131072 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 327680 131072 sh=0 gs=1 go=2 set=4400",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...rule 196608 327680 65536",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 327680 131072 sh=0 gs=1 go=2 set=1200",
            "...glue/0 0 65536 0",
            "...kern 131072",
            "...rule 26214 196608 131072",
            "..glue/12 131072 0 0",
            ".rule 3276800 65536 0",
            ".penalty 77",
            ".glue/0 131072 0 0",
            ".hlist/4 3276800 0 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 0 0 sh=0 gs=1 go=0 set=0",
            "...kern 1310720",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
            ".rule 3276800 65536 0",
            ".penalty 77",
            ".glue/0 131072 0 0",
            ".rule 3276800 26214 0",
            ".hlist/4 3276800 524288 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 524288 0 sh=0 gs=0 go=0 set=0",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...rule 26214 524288 -1073741824",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 524288 0 sh=0 gs=1 go=2 set=3600",
            "...glue/0 0 65536 0",
            "..glue/12 131072 0 0",
            ".rule 3276800 65536 0",
            ".penalty 77",
            ".glue/0 131072 0 0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// Glue appended after the row; no interline glue (the callback owns `append_to_vlist`).
#[test]
fn append_filter_extra_glue_after_row() {
    check(
        CB_AFTERD,
        BODY_H,
        &[
            "PD1 -1000.0pt",
            "PD2 -1000.0pt",
            "vlist/0 3276800 1730150 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".hlist/4 3276800 327680 131072 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 327680 131072 sh=0 gs=1 go=2 set=4400",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...rule 196608 327680 65536",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 327680 131072 sh=0 gs=1 go=2 set=1200",
            "...glue/0 0 65536 0",
            "...kern 131072",
            "...rule 26214 196608 131072",
            "..glue/12 131072 0 0",
            ".glue/0 131072 0 0",
            ".hlist/4 3276800 0 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 0 0 sh=0 gs=1 go=0 set=0",
            "...kern 1310720",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
            ".glue/0 131072 0 0",
            ".rule 3276800 26214 0",
            ".hlist/4 3276800 524288 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 524288 0 sh=0 gs=0 go=0 set=0",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...rule 26214 524288 -1073741824",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 524288 0 sh=0 gs=1 go=2 set=3600",
            "...glue/0 0 65536 0",
            "..glue/12 131072 0 0",
            ".glue/0 131072 0 0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// Glue and a penalty returned in front of the row, `prev_depth` passed through.
#[test]
fn append_filter_nodes_before_the_row() {
    check(
        CB_BEFORE,
        BODY_H,
        &[
            "PD1 -1000.0pt",
            "PD2 -1000.0pt",
            "vlist/0 3276800 1926758 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".glue/0 196608 0 0",
            ".penalty 55",
            ".hlist/4 3276800 327680 131072 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 327680 131072 sh=0 gs=1 go=2 set=4400",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...rule 196608 327680 65536",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 327680 131072 sh=0 gs=1 go=2 set=1200",
            "...glue/0 0 65536 0",
            "...kern 131072",
            "...rule 26214 196608 131072",
            "..glue/12 131072 0 0",
            ".glue/0 196608 0 0",
            ".penalty 55",
            ".hlist/4 3276800 0 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 0 0 sh=0 gs=1 go=0 set=0",
            "...kern 1310720",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
            ".rule 3276800 26214 0",
            ".glue/0 196608 0 0",
            ".penalty 55",
            ".hlist/4 3276800 524288 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 524288 0 sh=0 gs=0 go=0 set=0",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...rule 26214 524288 -1073741824",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 524288 0 sh=0 gs=1 go=2 set=3600",
            "...glue/0 0 65536 0",
            "..glue/12 131072 0 0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// The row is replaced by a box `node.new` made: nothing of the row is set.
#[test]
fn append_filter_replaces_row_by_another_box() {
    check(
        CB_REPLACE,
        BODY_H,
        &[
            "PD1 2.0pt",
            "PD2 2.0pt",
            "vlist/0 3276800 1730150 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".hlist/0 1310720 327680 131072 sh=0 gs=0 go=0 set=0",
            ".hlist/0 1310720 327680 131072 sh=0 gs=0 go=0 set=0",
            ".rule 3276800 26214 0",
            ".hlist/0 1310720 327680 131072 sh=0 gs=0 go=0 set=0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// Rows for which the callback returns nil disappear; the others are set.
#[test]
fn append_filter_drops_alternate_rows() {
    check(
        CB_DROP,
        BODY_H,
        &[
            "PD1 -1000.0pt",
            "PD2 -1000.0pt",
            "vlist/0 3276800 1336934 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".hlist/4 3276800 327680 131072 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 327680 131072 sh=0 gs=1 go=2 set=4400",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...rule 196608 327680 65536",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 327680 131072 sh=0 gs=1 go=2 set=1200",
            "...glue/0 0 65536 0",
            "...kern 131072",
            "...rule 26214 196608 131072",
            "..glue/12 131072 0 0",
            ".rule 3276800 26214 0",
            ".hlist/4 3276800 524288 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 524288 0 sh=0 gs=0 go=0 set=0",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...rule 26214 524288 -1073741824",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 524288 0 sh=0 gs=1 go=2 set=3600",
            "...glue/0 0 65536 0",
            "..glue/12 131072 0 0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// The second result sets `prev_depth` after the alignment.
#[test]
fn append_filter_chooses_prev_depth() {
    check(
        CB_PD,
        BODY_H,
        &[
            "PD1 -1000.0pt",
            "PD2 -1000.0pt",
            "vlist/0 3276800 1336934 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".hlist/4 3276800 327680 131072 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 327680 131072 sh=0 gs=1 go=2 set=4400",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...rule 196608 327680 65536",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 327680 131072 sh=0 gs=1 go=2 set=1200",
            "...glue/0 0 65536 0",
            "...kern 131072",
            "...rule 26214 196608 131072",
            "..glue/12 131072 0 0",
            ".hlist/4 3276800 0 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 0 0 sh=0 gs=1 go=0 set=0",
            "...kern 1310720",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
            ".rule 3276800 26214 0",
            ".hlist/4 3276800 524288 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 524288 0 sh=0 gs=0 go=0 set=0",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...rule 26214 524288 -1073741824",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 524288 0 sh=0 gs=1 go=2 set=3600",
            "...glue/0 0 65536 0",
            "..glue/12 131072 0 0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// Dimensions Lua gives the unset cells in the `fin_row` list reach the set cells.
#[test]
fn hpack_filter_edits_unset_cell_dimensions() {
    check(
        CB_WIDTH,
        BODY_H,
        &[
            "PD1 0.0pt",
            "PD2 0.0pt",
            "vlist/0 3276800 3106406 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".glue/2 196608 0 0",
            ".hlist/4 3276800 393216 131072 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 393216 131072 sh=0 gs=1 go=2 set=400",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...rule 196608 327680 65536",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 393216 131072 sh=0 gs=2 go=0 set=0",
            "...glue/0 0 65536 0",
            "...kern 131072",
            "...rule 26214 196608 131072",
            "..glue/12 131072 0 0",
            ".glue/2 589824 0 0",
            ".hlist/4 3276800 65536 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 65536 0 sh=0 gs=1 go=0 set=0",
            "...kern 1310720",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
            ".rule 3276800 26214 0",
            ".hlist/4 3276800 589824 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 589824 0 sh=0 gs=2 go=0 set=0",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...rule 26214 524288 -1073741824",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 589824 0 sh=0 gs=2 go=0 set=0",
            "...glue/0 0 65536 0",
            "..glue/12 131072 0 0",
            ".glue/2 786432 0 0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// Row height/depth and unset cell width edited in `append_to_vlist_filter`.
#[test]
fn append_filter_edits_row_and_cell_dimensions() {
    check(
        CB_WIDTH2,
        BODY_H,
        &[
            "PD1 -1000.0pt",
            "PD2 -1000.0pt",
            "vlist/0 3276800 2713190 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".hlist/4 3276800 589824 196608 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 589824 196608 sh=0 gs=1 go=2 set=400",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...rule 196608 327680 65536",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 589824 196608 sh=0 gs=2 go=0 set=0",
            "...glue/0 0 65536 0",
            "...kern 131072",
            "...rule 26214 196608 131072",
            "..glue/12 131072 0 0",
            ".hlist/4 3276800 589824 196608 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 589824 196608 sh=0 gs=1 go=0 set=0",
            "...kern 1310720",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
            ".rule 3276800 26214 0",
            ".hlist/4 3276800 589824 196608 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 589824 196608 sh=0 gs=2 go=0 set=0",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...rule 26214 524288 -1073741824",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 589824 196608 sh=0 gs=2 go=0 set=0",
            "...glue/0 0 65536 0",
            "..glue/12 131072 0 0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// `fin_align` sets width and shift of the row itself.
#[test]
fn append_filter_row_shift_and_width_are_overwritten() {
    check(
        CB_ROWDIM,
        BODY_H,
        &[
            "PD1 -1000.0pt",
            "PD2 -1000.0pt",
            "vlist/0 3276800 1336934 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".hlist/4 3276800 327680 131072 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 327680 131072 sh=0 gs=1 go=2 set=4400",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...rule 196608 327680 65536",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 327680 131072 sh=0 gs=1 go=2 set=1200",
            "...glue/0 0 65536 0",
            "...kern 131072",
            "...rule 26214 196608 131072",
            "..glue/12 131072 0 0",
            ".hlist/4 3276800 0 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 0 0 sh=0 gs=1 go=0 set=0",
            "...kern 1310720",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
            ".rule 3276800 26214 0",
            ".hlist/4 3276800 524288 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 524288 0 sh=0 gs=0 go=0 set=0",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...rule 26214 524288 -1073741824",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 524288 0 sh=0 gs=1 go=2 set=3600",
            "...glue/0 0 65536 0",
            "..glue/12 131072 0 0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// Stretch/shrink totals and orders Lua writes into the unset cells decide the glue setting of the set cells.
#[test]
fn hpack_filter_edits_glue_totals_of_unset_cells() {
    check(
        CB_TOTALS,
        BODY_H,
        &[
            "PD1 0.0pt",
            "PD2 0.0pt",
            "vlist/0 3276800 3040870 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".glue/2 262144 0 0",
            ".hlist/4 3276800 327680 131072 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 327680 131072 sh=0 gs=1 go=0 set=2200",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...rule 196608 327680 65536",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 327680 131072 sh=0 gs=1 go=0 set=600",
            "...glue/0 0 65536 0",
            "...kern 131072",
            "...rule 26214 196608 131072",
            "..glue/12 131072 0 0",
            ".glue/2 655360 0 0",
            ".hlist/4 3276800 0 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 0 0 sh=0 gs=1 go=0 set=13500",
            "...kern 1310720",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
            ".rule 3276800 26214 0",
            ".hlist/4 3276800 524288 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 524288 0 sh=0 gs=0 go=0 set=0",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...rule 26214 524288 -1073741824",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 524288 0 sh=0 gs=1 go=0 set=1800",
            "...glue/0 0 65536 0",
            "..glue/12 131072 0 0",
            ".glue/2 786432 0 0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// Nodes added to the list of an unset cell stay in the set cell.
#[test]
fn hpack_filter_edits_cell_contents() {
    check(
        CB_CELLCONTENT,
        BODY_H,
        &[
            "PD1 0.0pt",
            "PD2 0.0pt",
            "vlist/0 3276800 3040870 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".glue/2 262144 0 0",
            ".hlist/4 3276800 327680 131072 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 327680 131072 sh=0 gs=1 go=2 set=4400",
            "...kern 196608",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...rule 196608 327680 65536",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 327680 131072 sh=0 gs=1 go=2 set=1200",
            "...kern 196608",
            "...glue/0 0 65536 0",
            "...kern 131072",
            "...rule 26214 196608 131072",
            "..glue/12 131072 0 0",
            ".glue/2 655360 0 0",
            ".hlist/4 3276800 0 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 0 0 sh=0 gs=1 go=0 set=0",
            "...kern 196608",
            "...kern 1310720",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
            ".rule 3276800 26214 0",
            ".hlist/4 3276800 524288 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 524288 0 sh=0 gs=0 go=0 set=0",
            "...kern 196608",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...rule 26214 524288 -1073741824",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 524288 0 sh=0 gs=1 go=2 set=3600",
            "...kern 196608",
            "...glue/0 0 65536 0",
            "..glue/12 131072 0 0",
            ".glue/2 786432 0 0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// The tabskip glue nodes of the row list are the ones in the set row.
#[test]
fn hpack_filter_edits_tabskip_glue_of_the_row() {
    check(
        CB_ROWGLUE,
        BODY_H,
        &[
            "PD1 0.0pt",
            "PD2 0.0pt",
            "vlist/0 3276800 3040870 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".glue/2 262144 0 0",
            ".hlist/4 3276800 327680 131072 sh=0 gs=1 go=2 set=27000",
            "..glue/12 196608 131072 65536",
            "..hlist/5 878182 327680 131072 sh=0 gs=1 go=2 set=4400",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...rule 196608 327680 65536",
            "...glue/0 0 65536 0",
            "..glue/12 327680 65536 0",
            "..hlist/5 235930 327680 131072 sh=0 gs=1 go=2 set=1200",
            "...glue/0 0 65536 0",
            "...kern 131072",
            "...rule 26214 196608 131072",
            "..glue/12 262144 0 0",
            ".glue/2 655360 0 0",
            ".hlist/4 3276800 0 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 196608 131072 65536",
            "..hlist/5 878182 0 0 sh=0 gs=1 go=0 set=0",
            "...kern 1310720",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 262144 0 0",
            ".rule 3276800 26214 0",
            ".hlist/4 3276800 524288 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 196608 131072 65536",
            "..hlist/5 878182 524288 0 sh=0 gs=0 go=0 set=0",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...rule 26214 524288 -1073741824",
            "...glue/0 0 65536 0",
            "..glue/12 327680 65536 0",
            "..hlist/5 235930 524288 0 sh=0 gs=1 go=2 set=3600",
            "...glue/0 0 65536 0",
            "..glue/12 262144 0 0",
            ".glue/2 786432 0 0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// Rows of an alignment inside a cell run the callbacks, too.
#[test]
fn nested_alignment_rows_follow_callbacks() {
    check(
        CB_AFTER,
        BODY_N,
        &[
            "vlist/0 917504 786432 0 sh=0 gs=0 go=0 set=0",
            ".hlist/4 917504 393216 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 65536 131072 0",
            "..hlist/5 327680 393216 0 sh=0 gs=1 go=0 set=0",
            "...kern 131072",
            "..glue/12 65536 131072 0",
            "..hlist/5 393216 393216 0 sh=0 gs=0 go=0 set=0",
            "...vlist/0 393216 393216 0 sh=0 gs=0 go=0 set=0",
            "....hlist/4 393216 0 0 sh=0 gs=0 go=0 set=0",
            ".....glue/12 65536 131072 0",
            ".....hlist/5 262144 0 0 sh=0 gs=1 go=0 set=0",
            "......kern 196608",
            ".....glue/12 65536 131072 0",
            "....rule 393216 65536 0",
            "....penalty 77",
            "....glue/0 131072 0 0",
            "....hlist/4 393216 0 0 sh=0 gs=0 go=0 set=0",
            ".....glue/12 65536 131072 0",
            ".....hlist/5 262144 0 0 sh=0 gs=0 go=0 set=0",
            "......kern 262144",
            ".....glue/12 65536 131072 0",
            "....rule 393216 65536 0",
            "....penalty 77",
            "....glue/0 131072 0 0",
            "..glue/12 65536 131072 0",
            ".rule 917504 65536 0",
            ".penalty 77",
            ".glue/0 131072 0 0",
            ".hlist/4 917504 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 65536 131072 0",
            "..hlist/5 327680 0 0 sh=0 gs=0 go=0 set=0",
            "...kern 327680",
            "..glue/12 65536 131072 0",
            "..hlist/5 393216 0 0 sh=0 gs=1 go=0 set=0",
            "..glue/12 65536 131072 0",
            ".rule 917504 65536 0",
            ".penalty 77",
            ".glue/0 131072 0 0",
        ],
    );
}

/// Display alignments: rows and what the callback adds join the display.
#[test]
fn display_alignment_rows_follow_callbacks() {
    check(
        CB_AFTER,
        BODY_D,
        &[
            "vlist/0 6553600 1114112 0 sh=0 gs=0 go=0 set=0",
            ".hlist/1 6553600 0 0 sh=0 gs=1 go=2 set=97000",
            "..local_par",
            "..hlist/2 196608 0 0 sh=0 gs=0 go=0 set=0",
            "..penalty 10000",
            "..glue/15 0 65536 0",
            "..glue/9 0 0 0",
            ".penalty 0",
            ".glue/4 196608 0 0",
            ".hlist/4 720896 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 65536 131072 0",
            "..hlist/5 327680 0 0 sh=0 gs=1 go=0 set=0",
            "...kern 131072",
            "..glue/12 65536 131072 0",
            "..hlist/5 196608 0 0 sh=0 gs=0 go=0 set=0",
            "...kern 196608",
            "..glue/12 65536 131072 0",
            ".rule 720896 65536 0",
            ".penalty 77",
            ".glue/0 131072 0 0",
            ".kern 262144",
            ".hlist/4 720896 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 65536 131072 0",
            "..hlist/5 327680 0 0 sh=0 gs=0 go=0 set=0",
            "...kern 327680",
            "..glue/12 65536 131072 0",
            "..hlist/5 196608 0 0 sh=0 gs=1 go=0 set=0",
            "..glue/12 65536 131072 0",
            ".rule 720896 65536 0",
            ".penalty 77",
            ".glue/0 131072 0 0",
            ".penalty 0",
            ".glue/5 262144 0 0",
        ],
    );
}

/// `\valign` rows go through `vpack_filter` (`fin_row`); edits of the unset cells are set. luatex divides integers for the glue set of its cells.
#[test]
fn valign_rows_run_vpack_filter() {
    check(
        CB_VROW,
        BODY_V,
        &[
            "hlist/2 983040 2621440 0 sh=0 gs=0 go=0 set=0",
            ".vlist/4 393216 2621440 0 sh=0 gs=1 go=2 set=11000",
            "..glue/12 65536 131072 65536",
            "..vlist/5 393216 1114112 0 sh=0 gs=1 go=2 set=12000",
            "...kern 196608",
            "...hlist/2 262144 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..vlist/5 393216 393216 0 sh=0 gs=2 go=0 set=0",
            "...glue/0 0 65536 0",
            "...hlist/2 327680 393216 0 sh=0 gs=0 go=0 set=0",
            "....rule 26214 393216 -1073741824",
            "..glue/12 131072 0 0",
            ".rule 65536 2621440 0",
            ".vlist/4 524288 2621440 0 sh=0 gs=1 go=2 set=11000",
            "..glue/12 65536 131072 65536",
            "..vlist/5 524288 1114112 0 sh=0 gs=2 go=0 set=0",
            "...kern 196608",
            "...hlist/2 196608 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 131072 0 0",
            "...glue/2 786432 0 0",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..vlist/5 524288 393216 0 sh=0 gs=1 go=2 set=4000",
            "...glue/0 0 65536 0",
            "...hlist/2 131072 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
        ],
    );
}

/// The cells of a `\valign` go through `vpack_filter` (`align_set`).
#[test]
fn valign_cells_run_vpack_filter() {
    check(
        CB_VCELL,
        BODY_V,
        &[
            "hlist/2 851968 2621440 0 sh=0 gs=0 go=0 set=0",
            ".vlist/4 327680 2621440 0 sh=0 gs=2 go=0 set=1000",
            "..glue/12 65536 131072 65536",
            "..vlist/5 327680 1507328 0 sh=0 gs=1 go=2 set=14000",
            "...kern 196608",
            "...hlist/2 262144 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...glue/0 393216 0 0",
            "..glue/12 196608 65536 0",
            "..vlist/5 327680 786432 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...hlist/2 327680 393216 0 sh=0 gs=0 go=0 set=0",
            "....rule 26214 393216 -1073741824",
            "...glue/0 393216 0 0",
            "..glue/12 131072 0 0",
            ".rule 65536 2621440 0",
            ".vlist/4 458752 2621440 0 sh=0 gs=2 go=0 set=1000",
            "..glue/12 65536 131072 65536",
            "..vlist/5 458752 1507328 0 sh=0 gs=0 go=0 set=0",
            "...kern 196608",
            "...hlist/2 196608 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 131072 0 0",
            "...glue/2 786432 0 0",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...glue/0 393216 0 0",
            "..glue/12 196608 65536 0",
            "..vlist/5 458752 786432 0 sh=0 gs=1 go=2 set=6000",
            "...glue/0 0 65536 0",
            "...hlist/2 131072 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 393216 0 0",
            "..glue/12 131072 0 0",
        ],
    );
}

/// The packed preamble of a `\valign` is a `vpack_filter` list, too (`preamble`).
#[test]
fn valign_preamble_runs_vpack_filter() {
    check(
        CB_VPRE,
        BODY_V,
        &[
            "hlist/2 851968 2621440 0 sh=0 gs=0 go=0 set=0",
            ".vlist/4 327680 2621440 0 sh=0 gs=1 go=2 set=5000",
            "..glue/12 65536 131072 65536",
            "..vlist/5 327680 1114112 0 sh=0 gs=1 go=2 set=14000",
            "...kern 196608",
            "...hlist/2 262144 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..vlist/5 327680 393216 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...hlist/2 327680 393216 0 sh=0 gs=0 go=0 set=0",
            "....rule 26214 393216 -1073741824",
            "..glue/12 131072 0 0",
            ".rule 65536 2621440 0",
            ".vlist/4 458752 2621440 0 sh=0 gs=1 go=2 set=5000",
            "..glue/12 65536 131072 65536",
            "..vlist/5 458752 1114112 0 sh=0 gs=0 go=0 set=0",
            "...kern 196608",
            "...hlist/2 196608 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 131072 0 0",
            "...glue/2 786432 0 0",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..vlist/5 458752 393216 0 sh=0 gs=1 go=2 set=6000",
            "...glue/0 0 65536 0",
            "...hlist/2 131072 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
        ],
    );
}

/// No callbacks: a `\valign` is set as before.
#[test]
fn valign_pass_through() {
    check(
        "",
        BODY_V,
        &[
            "hlist/2 851968 2621440 0 sh=0 gs=0 go=0 set=0",
            ".vlist/4 327680 2621440 0 sh=0 gs=1 go=2 set=11000",
            "..glue/12 65536 131072 65536",
            "..vlist/5 327680 1114112 0 sh=0 gs=1 go=2 set=14000",
            "...kern 196608",
            "...hlist/2 262144 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..vlist/5 327680 393216 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...hlist/2 327680 393216 0 sh=0 gs=0 go=0 set=0",
            "....rule 26214 393216 -1073741824",
            "..glue/12 131072 0 0",
            ".rule 65536 2621440 0",
            ".vlist/4 458752 2621440 0 sh=0 gs=1 go=2 set=11000",
            "..glue/12 65536 131072 65536",
            "..vlist/5 458752 1114112 0 sh=0 gs=0 go=0 set=0",
            "...kern 196608",
            "...hlist/2 196608 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 131072 0 0",
            "...glue/2 786432 0 0",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..vlist/5 458752 393216 0 sh=0 gs=1 go=2 set=6000",
            "...glue/0 0 65536 0",
            "...hlist/2 131072 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
        ],
    );
}

/// Order and arguments of the callbacks for halign, spans, noalign, nested, display and valign alignments.
#[test]
fn callback_order_and_arguments() {
    check(
        CB_TRACE,
        BODY_T,
        &[
            "hyphenate",
            "ligaturing",
            "kerning",
            "H align_set 0 additional nil nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H align_set 0 additional nil nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H align_set 0 additional nil nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H fin_row 0 additional nil nil",
            "A alignment -65536000 false",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H align_set 0 additional nil nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H fin_row 0 additional nil nil",
            "A alignment -65536000 false",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H adjusted_hbox 0 additional TLT nil",
            "A box -65536000 false",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H align_set 0 additional nil nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H fin_row 0 additional nil nil",
            "A alignment -65536000 false",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H align_set 0 additional nil nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H align_set 0 additional nil nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H fin_row 0 additional nil nil",
            "A alignment -65536000 false",
            "V vbox 0 additional 0 TLT nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H align_set 0 additional nil nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H fin_row 0 additional nil nil",
            "A alignment -65536000 false",
            "V vbox 0 additional 0 TLT nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H hbox 0 additional TLT nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "A post_linebreak -65536000 false",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H align_set 0 additional nil nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H align_set 0 additional nil nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H fin_row 0 additional nil nil",
            "A alignment -65536000 false",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H align_set 0 additional nil nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H fin_row 0 additional nil nil",
            "A alignment -65536000 false",
            "V vbox 0 additional 0 TLT nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H adjusted_hbox 0 additional TLT nil",
            "A box -65536000 false",
            "V align_set 0 additional 0 nil nil",
            "V align_set 0 additional 0 nil nil",
            "V fin_row 0 additional 0 nil nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H adjusted_hbox 0 additional TLT nil",
            "A box -65536000 false",
            "V align_set 0 additional 0 nil nil",
            "V align_set 0 additional 0 nil nil",
            "V fin_row 0 additional 0 nil nil",
            "V preamble 0 additional 0 nil nil",
            "hyphenate",
            "ligaturing",
            "kerning",
            "H hbox 0 additional TLT nil",
        ],
    );
}

/// Only a text pass is registered: rows keep their shape.
#[test]
fn text_passes_alone_leave_the_rows() {
    check(
        CB_TEXTONLY,
        BODY_H,
        &[
            "ligaturing",
            "ligaturing",
            "ligaturing",
            "ligaturing",
            "ligaturing",
            "ligaturing",
            "ligaturing",
            "ligaturing",
            "ligaturing",
            "ligaturing",
            "ligaturing",
            "PD1 0.0pt",
            "PD2 0.0pt",
            "vlist/0 3276800 3040870 0 sh=0 gs=0 go=0 set=0",
            ".hlist/2 262144 131072 196608 sh=0 gs=0 go=0 set=0",
            "..rule 26214 131072 196608",
            ".glue/2 262144 0 0",
            ".hlist/4 3276800 327680 131072 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 327680 131072 sh=0 gs=1 go=2 set=4400",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...rule 196608 327680 65536",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 327680 131072 sh=0 gs=1 go=2 set=1200",
            "...glue/0 0 65536 0",
            "...kern 131072",
            "...rule 26214 196608 131072",
            "..glue/12 131072 0 0",
            ".glue/2 655360 0 0",
            ".hlist/4 3276800 0 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 0 0 sh=0 gs=1 go=0 set=0",
            "...kern 1310720",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 0 0 sh=0 gs=0 go=0 set=0",
            "..glue/12 131072 0 0",
            ".rule 3276800 26214 0",
            ".hlist/4 3276800 524288 0 sh=0 gs=1 go=2 set=27000",
            "..glue/12 65536 131072 65536",
            "..hlist/5 878182 524288 0 sh=0 gs=0 go=0 set=0",
            "...hlist/2 393216 0 0 sh=0 gs=1 go=2 set=6000",
            "....glue/0 0 65536 65536",
            "...hlist/2 458752 0 0 sh=0 gs=0 go=0 set=0",
            "...glue/0 0 65536 0",
            "...rule 26214 524288 -1073741824",
            "...glue/0 0 65536 0",
            "..glue/12 196608 65536 0",
            "..hlist/5 235930 524288 0 sh=0 gs=1 go=2 set=3600",
            "...glue/0 0 65536 0",
            "..glue/12 131072 0 0",
            ".glue/2 786432 0 0",
            ".hlist/2 327680 0 0 sh=0 gs=0 go=0 set=0",
        ],
    );
}

/// A row whose list no longer starts with glue and an unset cell is a fatal "bad box" in `fin_align`.
#[test]
fn row_without_leading_glue_is_a_bad_box() {
    check_fatal(CB_BADBOX, BODY_H);
}

/// An `hpack_filter` that returns nil leaves an empty row: fatal "bad box".
#[test]
fn row_list_emptied_by_hpack_filter_is_a_bad_box() {
    check_fatal(CB_HNIL, BODY_H);
}

/// The same for a `\valign` row (`vpack_filter`).
#[test]
fn valign_row_list_emptied_is_a_bad_box() {
    check_fatal(CB_VNIL, BODY_V);
}
