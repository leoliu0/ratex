//! Node-dependent parts of the `tex` library (box registers as nodes, the
//! nest, lists, splitbox, \Umath parameters, the tex random stream). Every
//! expected line is what `luatex --ini` (LuaTeX 1.24.0, TeX Live 2026) logs for
//! the same script (`texio.write_nl` lines; tab separated).
use tex_core::engine::{Engine, EngineKind};

fn run_script(pre: &str, script: &str) -> Vec<String> {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    for (c, v) in [(b'{', 1), (b'}', 2), (b'#', 6), (b' ', 10), (b'\n', 5), (b'\r', 5)] {
        e.eqtb.cat[c as usize] = v;
    }
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!("lua_texnodes-{}-{:x}.lua", std::process::id(), script.len()));
    std::fs::write(&path, script).unwrap();
    e.input.push_file("t.tex".into(), format!("{pre}\\directlua{{dofile(\"{}\")}}\n\\end\n", path.display().to_string().replace('\\', "/")).into_bytes());
    e.run();
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    e.term.lines().filter(|l| l.contains('\t')).map(str::to_string).collect()
}

// oracle: luatex --ini -interaction=batchmode (script and preamble in this test)
#[test]
fn tex_boxes_nest_lists_and_math_match_luatex() {
    let pre = r####"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\^=7
\chardef\mybox=5
\setbox1\hbox{\kern3pt\vrule width 2pt}
\setbox2\vbox{\hrule height 10pt}
\setbox6\vbox{\hrule height 10pt\penalty-100\hrule height 20pt\penalty-100 \hrule height 5pt}
"####;
    let script = r####"local function p(...) local t={} for i=1,select("#",...) do t[i]=tostring((select(i,...))) end texio.write_nl(table.concat(t,"\t")) end
local function E(name, f, ...) local ok,e=pcall(f, ...) p(name, ok, (tostring(e):gsub("^[^:]*:%d+: ",""))) end
local top = tex.nest.top
p("nest", tex.nest.ptr, top.mode, top.modeline, top.prevdepth, top.spacefactor, top.prevgraf)
p("head", node.type(top.head.id), top.tail == top.head, top.head.next)
p("getnest", tex.getnest("ptr"), tex.getnest(0) == nil, tex.getnest(1), tex.nest[0] == tex.nest.top)
E("setnest", tex.setnest, 1)
E("nest newindex", function() tex.nest[0] = 1 end)
for _, k in ipairs{"page_head","contrib_head","temp_head","best_page_break","least_page_cost","best_size","bogus"} do p("list", k, tex.lists[k]) end
local b = tex.getbox(1)
p("box", b.id, b.width, b.head.id, tex.box[1] == b, tex.getbox(1) == tex.getbox(1))
p("void", tex.getbox(7), tex.box[7], tex.isbox(7), tex.isbox("foo"), tex.isbox("mybox"))
b.width = 12345
local bb = tex.getbox(2) bb.head.height = 9999
tex.box[3] = node.copy(b) p("copy", tex.box[3].width)
tex.setbox(3, nil) p("nil", tex.box[3])
tex.setbox("global", 3, b) p("global", tex.box[3] == b)
E("incompatible", tex.setbox, 4, node.new("glue"))
E("range", tex.setbox, 70000, b)
E("badarg", tex.getbox, {})
tex.setbox("mybox", node.new("hlist")) p("named", tex.getbox(5).id, tex.box.mybox ~= nil)
local t = tex.splitbox(6, 12*65536, "exactly")
p("split", t.id, t.height, tex.getbox(6).height)
local t2 = tex.splitbox(6, 12*65536, "additional")
p("split2", t2.height, tex.getbox(6).id)
E("splitmode", tex.splitbox, 6, 10, 3)
p("splitnil", tex.splitbox(6, "x"), tex.splitbox(9, 10))
tex.nest.top.prevdepth = 5.5 p("pd", tex.nest.top.prevdepth)
tex.nest.top.prevgraf = 7 p("pg", tex.nest.top.prevgraf, tex.get("prevgraf"))
tex.nest.top.spacefactor = 999.4 p("sf", tex.nest.top.spacefactor)
tex.nest.top.modeline = 12.2 p("ml", tex.nest.top.modeline)
p("math", tex.getmath("quad","text"), tex.getmath("ordordspacing","text"))
tex.setmath("quad","text", 12345.6) p("quad", tex.getmath("quad","text"))
tex.setmath("global","axis","display", 777) p("axis", tex.getmath("axis","display"))
E("mathopt", tex.getmath, "bogus", "text")
E("mathval", tex.setmath, "quad", "text", "x")
local g = node.new("glue_spec") g.width = 100 g.stretch = 7 g.stretch_order = 1
tex.setmath("ordordspacing","text", g) local gg = tex.getmath("ordordspacing","text")
p("mu", gg.id, gg.width, gg.stretch, gg.shrink, gg.stretch_order)
tex.init_rand(5) p("rand", tex.lua_math_random(100), tex.lua_math_random(), tex.lua_math_random(3, 9), tex.uniform_rand(100))
tex.lua_math_randomseed(5) p("rand2", tex.lua_math_random(100))
E("randempty", tex.lua_math_random, 5, 3) p("end")
"####;
    let expected = r####"nest	0	1	0	-65536000	1000	0
head	temp	true	nil
getnest	0	false	nil	false
setnest	false	You can't modify the semantic nest array directly
nest newindex	false	You can't modify the semantic nest array directly
list	page_head	nil
list	contrib_head	nil
list	temp_head	nil
list	best_page_break	nil
list	least_page_cost	0
list	best_size	0
list	bogus	nil
box	0	327680	13	true	true
void	nil	nil	true	false	true
copy	12345
nil	nil
global	true
incompatible	false	setbox: incompatible node type (glue)
range	false	incorrect index specification for tex.setbox()
badarg	false	argument must be a string or a number
named	0	true
split	1	786432	1638400
split2	1310720	1
splitmode	false	wrong mode in splitbox
splitnil	nil	nil
pd	6
pg	7	7
sf	999
ml	0
math	1073741823	nil
quad	12346
axis	777
mathopt	false	bad argument #1 to 'tex.getmath' (invalid option 'bogus')
mathval	false	argument must be a number
mu	39	100	7	0	1
rand	57.0	0.58496293033704	5.0	72
rand2	57.0
randempty	false	bad argument #2 to 'tex.lua_math_random' (interval is empty)"####;
    let got = run_script(pre, script);
    let want: Vec<&str> = expected.lines().filter(|l| l.contains('\t')).collect();
    assert_eq!(got, want);
}
