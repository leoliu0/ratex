//! Behaviour of the `tex`, `token`, `pdf` and `lang` libraries. Every expected
//! output below is what `luatex --ini` (LuaTeX 1.24.0, TeX Live 2026) prints
//! for the same Lua script (texio.write_nl lines in the log; wrapped log
//! lines are compared with newlines removed).
use tex_core::engine::{Engine, EngineKind};

fn run_script(pre: &str, script: &str) -> String {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    for (c, v) in [(b'{', 1), (b'}', 2), (b'#', 6), (b' ', 10), (b'\n', 5), (b'\r', 5)] {
        e.eqtb.cat[c as usize] = v;
    }
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!("lua_texlib-{}-{:x}.lua", std::process::id(), script.len()));
    std::fs::write(&path, script).unwrap();
    e.input.push_file("t.tex".into(), format!("{pre}\\directlua{{dofile(\"{}\")}}\n\\end\n", path.display()).into_bytes());
    e.run();
    assert_eq!(e.error_count, 0, "{:?}", e.diagnostics);
    e.term.replace('\n', "")
}

// oracle: luatex --ini -interaction=batchmode (script in this test)
#[test]
fn tex_registers_and_codes_match_luatex() {
    let script = r####"local function p(...) local t={} for i=1,select("#",...) do t[i]=tostring((select(i,...))) end texio.write_nl(table.concat(t,"\t")) end
local function T(name, f) local ok,e=pcall(f) if not ok then p(name,"ERR",(tostring(e):gsub("^[^:]*:%d+: ",""))) end end
tex.hsize = 100*65536
p("hsize", tex.hsize, tex.get("hsize"), tex.hsize == tex.get("hsize"))
tex.hsize = "3pt" p("hsize str", tex.hsize)
tex.set("global", "hsize", 7) p("set global", tex.hsize)
tex.tolerance = 3.7 p("tol float", tex.tolerance)
T("tol str", function() tex.tolerance = "5" end)
tex.everypar = "abc" p("toks", tex.everypar, tex.get("everypar"))
tex.setglue("baselineskip", 1,2,3,1,2) p("setglue", tex.getglue("baselineskip"))
tex.setglue("global", 5, 1) p("setglue5", tex.getglue(5))
tex.setglue(5,10,20,30,1,2) p("glue5 width", tex.glue[5])
tex.setmuglue("thinmuskip", 5,6,7,1,0) p("mu", tex.getmuglue("thinmuskip"))
p("count neg", pcall(tex.getcount, -1))
tex.count[0] = 2^31-1 p("count max", tex.count[0]) tex.count[0] = -2^31 p("count min", tex.count[0])
tex.count[0] = 1.5 p("count float", tex.count[0])
T("count str", function() tex.count[0] = "7" end)
tex.dimen[0] = "7pt" p("dimen str", tex.dimen[0])
tex.dimen[0] = 1.5 p("dimen float", tex.dimen[0])
tex.setdimen(2, "1in") p("dimen in", tex.getdimen(2))
tex.attribute[1] = 5 tex.setattribute("global", 2, 6) p("attr", tex.attribute[1], tex.attribute[2], tex.attribute[3])
tex.setmathcode(66, 7, 3, 0x42) p("mathcodes", tex.getmathcodes(66))
p("mathcode", tex.mathcode[66][1], tex.mathcode[66][2], tex.mathcode[66][3])
tex.setdelcode(70, 1,2,3,4) p("delcodes", tex.getdelcodes(70))
p("delcode none", tex.getdelcodes(300))
tex.setlccode(65, 66, 67) p("lc/uc", tex.getlccode(65), tex.getuccode(65))
tex.setsfcode(65, 1234) p("sf", tex.getsfcode(65), tex.sfcode[65])
tex.lccode[0x4e00] = 0x4e01 p("lc unicode", tex.lccode[0x4e00])
T("lc range", function() return tex.getlccode(-1) end)
T("cat range", function() return tex.getcatcode(1114112) end)
tex.catcode[65] = 12 p("catcode", tex.catcode[65])
"####;
    let expected = r####"hsize	6553600	6553600	truehsize str	196608set global	7tol float	0tol str	ERR	unsupported value typetoks	abc	abcsetglue	1	2	3	1	2setglue5	1	0	0	0	0glue5 width	10mu	5	6	7	1	0count neg	false	incorrect count indexcount max	2147483647count min	-2147483648count float	0count str	ERR	unsupported count value typedimen str	458752dimen float	2dimen in	4736286attr	5	6	-2147483647mathcodes	7	3	66mathcode	7	3	66delcodes	1	2	3	4delcode none	-1	0	0	0lc/uc	66	67sf	1234	1234lc unicode	19969lc range	ERR	incorrect character value -1 for tex.getlccode()cat range	ERR	incorrect character value 1114112 for tex.getcatcode()catcode	12"####;
    let got = run_script("", script);
    assert!(got.contains(expected), "got: {got}");
}

// oracle: luatex --ini -interaction=batchmode (script in this test)
#[test]
fn tex_pure_functions_match_luatex() {
    let script = r####"local function p(...) local t={} for i=1,select("#",...) do t[i]=tostring((select(i,...))) end texio.write_nl(table.concat(t,"\t")) end
local function T(name, f) local ok,e=pcall(f) if not ok then p(name,"ERR",(tostring(e):gsub("^[^:]*:%d+: ",""))) end end
p("number", tex.number(12), tex.romannumeral(1994), tex.romannumeral(0), tex.romannumeral(5000))
p("round", tex.round(3.5), tex.round(-3.5), tex.round(2.4), tex.round(0.5), tex.round(-0.5))
p("scale", tex.scale(10, 3), tex.scale(5, 0.5), tex.scale(1.5, 1.5))
p("badness", tex.badness(100, 50), tex.badness(10, 100), tex.badness(65536, 65536), tex.badness(1, 0), tex.badness(7230585, 1))
p("sp1", tex.sp("1pt"), tex.sp("-.5truept"), tex.sp("1in"), tex.sp("1cm"), tex.sp("1bp"))
p("sp2", tex.sp("1dd"), tex.sp("1cc"), tex.sp("1mm"), tex.sp("1nd"), tex.sp("1nc"))
p("sp3", tex.sp("1pc"), tex.sp(" 1 pt "), tex.sp("1,5pt"))
p("sp max ok", tex.sp("16383.99999pt"))
T("sp max", function() return tex.sp("16384pt") end)
T("sp unit", function() return tex.sp("1fil") end)
T("sp none", function() return tex.sp("abc") end)
tex.init_rand(42) p("rand", tex.uniform_rand(10), tex.uniform_rand(10), tex.normal_rand())
tex.init_rand(42) p("uniformdeviate", tex.uniformdeviate(10), tex.uniformdeviate(10))
p("modes", tex.getmodevalues()[1], tex.getmodevalues()[134], tex.getmodevalues()[267], tex.getmodevalues()[0])
p("isX", tex.iscount(3), tex.iscount("foo"), tex.isdimen("hsize"), tex.isbox(0), tex.isbox("foo"))
tex.scantoks(5, 0, "ab c") p("scantoks", tex.toks[5])
p("localvl", tex.getlocallevel()) tex.runtoks(function() p("in runtoks", tex.getlocallevel()) end)
p("pagestate", tex.getpagestate())
p("lastpenalty", tex.lastpenalty, tex.get("lastkern"))
p("version", tex.luatexversion, tex.luatexrevision)

"####;
    let expected = r####"number	12	mcmxciv		mmmmmround	4	-3	2	1	0scale	30	3	2badness	800	0	100	10000	10000sp1	65536	-32768	4736286	1864679	65781sp2	70124	841489	186467	69925	839105sp3	786432	65536	98304sp max ok	1073741823sp max	ERR	Dimension too largesp unit	ERR	Illegal unit of measure (pt inserted)sp none	ERR	Missing number, treated as zerorand	8	4	-63802uniformdeviate	8	4modes	vertical	horizontal	math	unsetisX	3	false	false	true	falsescantoks	ab clocalvl	0in runtoks	1pagestate	0lastpenalty	0	0version	124	0"####;
    let got = run_script("", script);
    assert!(got.contains(expected), "got: {got}");
}

// oracle: luatex --ini -interaction=batchmode (script in this test)
#[test]
fn lang_library_matches_luatex() {
    let script = r####"local function p(...) local t={} for i=1,select("#",...) do t[i]=tostring((select(i,...))) end texio.write_nl(table.concat(t,"\t")) end
local function T(name, f) local ok,e=pcall(f) if not ok then p(name,"ERR",(tostring(e):gsub("^[^:]*:%d+: ",""))) end end
local function words(s) if not s then return "nil" end local t={} for w in s:gmatch("%S+") do t[#t+1]=w end table.sort(t) return table.concat(t,"|") end
local l = lang.new() p(type(l), lang.id(l), lang.id(lang.new()), lang.id(lang.new(7)), lang.id(lang.new()))
T("id number", function() return lang.id(7) end)
T("id string", function() return lang.id("x") end)
T("undefined", function() return lang.new(70000) end)
local l3 = lang.new(3)
lang.patterns(l3, "a1b .ab4c 1cd")
p("patterns", words(lang.patterns(l3)))
lang.patterns(l3, "x1y")
p("patterns2", words(lang.patterns(l3)))
lang.clear_patterns(l3)
p("cleared", words(lang.patterns(l3)))
lang.hyphenation(l3, "ta-ble foo-bar")
p("exceptions", words(lang.hyphenation(l3)))
lang.clear_hyphenation(l3)
p("exceptions cleared", words(lang.hyphenation(l3)))
p("clean", lang.clean("Hello-World"), lang.clean("ab c"))
p("hjcode", lang.gethjcode(l3, 65), lang.gethjcode(l3, 97))
lang.sethjcode(l3, 65, 97) p("hjcode set", lang.gethjcode(l3, 65))
p("prehyphenchar", lang.prehyphenchar(l3))
lang.prehyphenchar(l3, 5) lang.posthyphenchar(l3, 6) lang.preexhyphenchar(l3, 7) lang.postexhyphenchar(l3, 8)
p("chars", lang.prehyphenchar(l3), lang.posthyphenchar(l3), lang.preexhyphenchar(l3), lang.postexhyphenchar(l3))
p("hyphenationmin", lang.hyphenationmin(l3)) lang.hyphenationmin(l3, 4) p("hyphenationmin set", lang.hyphenationmin(l3))
"####;
    let expected = r####"userdata	0	1	7	8id number	ERR	bad argument #1 to 'id' (luatex.lang expected, got number)id string	ERR	bad argument #1 to 'id' (luatex.lang expected, got string)undefined	ERR	lang.new(70000): undefined languagepatterns	.ab4c|1cd|a1bpatterns2	.ab4c|1cd|a1b|x1ycleared	exceptions	foo-bar|ta-bleexceptions cleared	nilclean	helloworld	abhjcode	97	97hjcode set	97prehyphenchar	45chars	5	6	7	8hyphenationmin	-1hyphenationmin set	4"####;
    let got = run_script("", script);
    assert!(got.contains(expected), "got: {got}");
}

// oracle: luatex --ini -interaction=batchmode (script in this test)
#[test]
fn token_objects_match_luatex() {
    let script = r####"local function p(...) local t={} for i=1,select("#",...) do t[i]=tostring((select(i,...))) end texio.write_nl(table.concat(t,"\t")) end
local function T(name, f) local ok,e=pcall(f) if not ok then p(name,"ERR",(tostring(e):gsub("^[^:]*:%d+: ",""))) end end
local function show(t) local r={} for _,k in ipairs{"command","cmdname","csname","index","mode","active","expandable","protected"} do r[#r+1]=k.."="..tostring(t[k]) end return table.concat(r," ") end
p(type(token.create("relax")), show(token.create("relax")))
p(show(token.create("par")))
p(show(token.create(65)))
p(show(token.create(65, 12)))
p(show(token.create("undefinedcs")))
p(show(token.new(65, 11)))
p(show(token.new(1, 1)))
p(show(token.create("hsize")))
p(show(token.create("count")))
p(token.create("relax") == token.create("relax"), token.create(65) == token.create(65, 11), token.create(65) == token.create(65, 12))
p(token.is_token(token.create("relax")), token.is_token(1), token.type(token.create("a")), token.type(3))
p("assign", (pcall(function() local t = token.create("relax") t.foo = 1 end)))
p(token.create("relax").foo)
p(token.biggest_char(), token.command_id("relax"), token.command_id("foo"))
local c = token.commands() p(c[0], c[1], c[15], #c)
p(token.is_defined("relax"), token.is_defined("foo"), token.is_defined("par"))
token.set_macro("foo", "abc") p(token.get_macro("foo"), token.get_meaning("foo"))
token.set_macro("bar", "#1x", "global") p(token.get_macro("bar"))
token.set_char("ch", 65) p(show(token.create("ch")))
"####;
    let expected = r####"userdata	command=0 cmdname=relax csname=relax index=nil mode=1114112 active=false expandable=false protected=falsecommand=13 cmdname=par_end csname=par index=nil mode=1114112 active=false expandable=false protected=falsecommand=11 cmdname=letter csname=nil index=65 mode=65 active=false expandable=false protected=falsecommand=12 cmdname=other_char csname=nil index=65 mode=65 active=false expandable=false protected=falsecommand=133 cmdname=undefined_cs csname= index=0 mode=0 active=false expandable=true protected=falsecommand=11 cmdname=letter csname=nil index=65 mode=65 active=false expandable=false protected=falsecommand=1 cmdname=left_brace csname=nil index=1 mode=1 active=false expandable=false protected=falsecommand=90 cmdname=assign_dimen csname=hsize index=nil mode=464526 active=false expandable=false protected=falsecommand=111 cmdname=register csname=count index=0 mode=0 active=false expandable=false protected=falsetrue	true	falsetrue	false	token	nilassign	falsenil1114111	0	nilrelax	left_brace	delim_num	156true	false	trueabc	->abc##1xcommand=82 cmdname=char_given csname=ch index=65 mode=65 active=false expandable=false protected=false"####;
    let got = run_script("", script);
    assert!(got.contains(expected), "got: {got}");
}

// oracle: luatex --ini -interaction=batchmode (script in this test)
#[test]
fn pdf_library_matches_luatex() {
    let script = r####"local function p(...) local t={} for i=1,select("#",...) do t[i]=tostring((select(i,...))) end texio.write_nl(table.concat(t,"\t")) end
local function T(name, f) local ok,e=pcall(f) if not ok then p(name,"ERR",(tostring(e):gsub("^[^:]*:%d+: ",""))) end end
p("defaults", pdf.getcompresslevel(), pdf.getobjcompresslevel(), pdf.getdecimaldigits(), pdf.getmajorversion(), pdf.getminorversion())
p("text defaults", pdf.getcatalog(), pdf.getinfo(), pdf.getnames(), pdf.gettrailer(), pdf.getpageresources(), pdf.getpageattributes())
pdf.setcompresslevel(5) p("compresslevel", pdf.getcompresslevel())
pdf.setdecimaldigits("3") p("decimaldigits str", pdf.getdecimaldigits())
pdf.setdestmargin(5) p("destmargin", pdf.getdestmargin())
pdf.setdestmargin("1pt") p("destmargin str", pdf.getdestmargin())
pdf.setpageresources("/Foo 1 0 R") p("pageresources", pdf.getpageresources())
pdf.setpageattributes("/Rotate 90") p("pageattributes", pdf.getpageattributes())
pdf.setcatalog("/A 1") pdf.setcatalog("/B 2") p("catalog", pdf.getcatalog())
pdf.setnames("/N 1") p("names", pdf.getnames())
pdf.setinfo("/Title (x)") p("info", pdf.getinfo())
pdf.settrailer("/Z 1") p("trailer", pdf.gettrailer())
pdf.settrailerid("[<aa><bb>]") p("trailerid", pdf.gettrailerid())
pdf.setorigin(1, 2) p("origin", pdf.getorigin())
pdf.setorigin(7) p("origin one", pdf.getorigin())
pdf.setomitinfo(1) p("omitinfo", pdf.getomitinfo())
pdf.setinclusionerrorlevel(-1) p("inclusionerrorlevel", pdf.getinclusionerrorlevel())
p("pos", pdf.getpos())
p("hvpos", pdf.gethpos(), pdf.getvpos())
p("matrix", pdf.getmatrix())
p("hasmatrix", pdf.hasmatrix())
local a = pdf.reserveobj() local b = pdf.reserveobj()
p("reserve", b == a + 1, pdf.getlastobj() == b, pdf.getmaxobjnum() >= b)
local c = pdf.immediateobj("<< /A 1 >>") p("immediate", c == b + 1, pdf.getlastobj() == c, pdf.getobjtype(c))
local d = pdf.immediateobj("stream", "abc") p("stream", d == c + 1, pdf.getobjtype(d))
local e = pdf.immediateobj("stream", "abc", "/Type /X") p("stream attr", e == d + 1)
pdf.immediateobj(a, "<< /B 2 >>") p("useobjnum", pdf.getlastobj() == a, pdf.getobjtype(a))
p("objtype none", pdf.getobjtype(99999))
local s1 = pdf.newcolorstack("0 g") local s2 = pdf.newcolorstack("0 g", "direct") local s3 = pdf.newcolorstack("0 g", "page", true)
p("colorstack", s2 == s1 + 1, s3 == s2 + 1)
p("creationdate", #pdf.getcreationdate() >= 16, pdf.getcreationdate():sub(1,2))
p("retval", pdf.getretval())
p("lastannot", pdf.getlastannot(), pdf.getlastlink())
"####;
    let expected = r####"defaults	0	0	0	1	0text defaults	nil	nil	nil	nil	nil	nilcompresslevel	5decimaldigits str	0destmargin	5destmargin str	5pageresources	/Foo 1 0 Rpageattributes	/Rotate 90catalog	/B 2names	/N 1info	/Title (x)trailer	/Z 1trailerid	[<aa><bb>]origin	1	2origin one	7	7omitinfo	1inclusionerrorlevel	-1pos	0	0hvpos	0	0matrix	1	0	0	1	0	0hasmatrix	falsereserve	true	true	trueimmediate	true	true	objstream	true	objstream attr	trueuseobjnum	true	objobjtype none	nilcolorstack	true	truecreationdate	true	D:retval	0lastannot	0	0"####;
    let got = run_script("\\directlua{tex.enableprimitives('',{'outputmode'})}\\outputmode=1 ", script);
    assert!(got.contains(expected), "got: {got}");
}
