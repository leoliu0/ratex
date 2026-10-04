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
local function E(name, f, ...) local ok=pcall(f, ...) p(name, ok) end
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
setnest	false
nest newindex	false
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
incompatible	false
range	false
badarg	false
named	0	true
split	1	786432	1638400
split2	1310720	1
splitmode	false
splitnil	nil	nil
pd	6
pg	7	7
sf	999
ml	0
math	1073741823	nil
quad	12346
axis	777
mathopt	false
mathval	false
mu	39	100	7	0	1
rand	57.0	0.58496293033704	5.0	72
rand2	57.0
randempty	false"####;
    let got = run_script(pre, script);
    let want: Vec<&str> = expected.lines().filter(|l| l.contains('\t')).collect();
    assert_eq!(got, want);
}

#[test]
fn fi_glue_packs_below_fil_and_math_glue_retains_its_order() {
    let pre = r"\skip0=0pt plus 1fi \skip1=0pt plus 1fil \relax
\directlua{tex.setglue(2,0,10,0,1,0) tex.setglue(3,0,10,0,4,0)}
\setbox2=\hbox to100sp{\hskip\skip2}
\setbox3=\hbox to100sp{\hskip\skip3}\relax";
    let script = r####"local function p(...) local t={} for i=1,select("#",...) do t[i]=tostring((select(i,...))) end texio.write_nl(table.concat(t,"\t")) end
for order=0,4 do
  local g=node.new("glue") g.stretch=10 g.stretch_order=order
  local h=node.hpack(g,100,"exactly")
  p("pack",order,h.glue_sign,h.glue_order,math.floor(h.glue_set))
  node.flush_node(h)
end
for order=0,3 do
  local a,b=node.new("glue"),node.new("glue")
  a.stretch=10 a.stretch_order=order
  b.stretch=20 b.stretch_order=order+1
  a.next=b b.prev=a
  local h=node.hpack(a,100,"exactly")
  p("precedence",order,h.glue_order,math.floor(h.glue_set))
  node.flush_node(h)
end
local function skip_node(i)
  local g=node.new("glue")
  g.width,g.stretch,g.shrink,g.stretch_order,g.shrink_order=tex.getglue(i)
  return g
end
local a,b=skip_node(0),skip_node(1)
local h=node.hpack(a,100,"exactly")
p("scanned-fi",h.glue_order)
node.flush_node(h)
h=node.hpack(b,100,"exactly")
p("scanned-fil",h.glue_order)
node.flush_node(h)
p("scalar-set-fi",tex.box[2].glue_sign,tex.box[2].glue_order,math.floor(tex.box[2].glue_set))
p("scalar-set-filll",tex.box[3].glue_sign,tex.box[3].glue_order,math.floor(tex.box[3].glue_set))
local g=node.new("glue_spec") g.width=100 g.stretch=7 g.stretch_order=1
tex.setmath("ordordspacing","text",g)
local r=tex.getmath("ordordspacing","text")
p("math",r.width,r.stretch,r.stretch_order)
"####;
    assert_eq!(run_script(pre, script), [
        "pack\t0\t1\t0\t10",
        "pack\t1\t1\t1\t10",
        "pack\t2\t1\t2\t10",
        "pack\t3\t1\t3\t10",
        "pack\t4\t1\t4\t10",
        "precedence\t0\t1\t5",
        "precedence\t1\t2\t5",
        "precedence\t2\t3\t5",
        "precedence\t3\t4\t5",
        "scanned-fi\t1",
        "scanned-fil\t2",
        "scalar-set-fi\t1\t1\t10",
        "scalar-set-filll\t1\t4\t10",
        "math\t100\t7\t1",
    ]);
}
