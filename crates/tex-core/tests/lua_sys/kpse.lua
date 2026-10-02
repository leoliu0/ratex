lfs.mkdir("kpse") lfs.chdir("kpse")
-- Installation roots differ per machine (TeX Live prefix, Ratex's embedded
-- tree); report every path from its TDS directory on.
local P0 = P
local function N(s)
  if type(s) ~= "string" then return s end
  return s:match("/texmf%-dist/(.*)$") or s:match("^/<embedded>/(.*)$") or s:match("^/var/lib/texmf/(.*)$") or s
end
function P(...) local t = table.pack(...) for i = 1, t.n do t[i] = N(t[i]) end P0(table.unpack(t, 1, t.n)) end
local rf = lfs.currentdir() .. "/readable.txt" io.open("readable.txt", "wb"):close()
P(pcall(kpse.find_file,"article.cls"))
kpse.set_program_name("luatex")
P(kpse.find_file("article.cls"))
P(kpse.find_file("article","tex"), kpse.find_file("article.cls","tex",true))
P(kpse.find_file("lmroman10-regular.otf","opentype fonts"), kpse.find_file("lmroman10-regular","opentype fonts"))
P(kpse.find_file("lmroman10-regular.otf","truetype fonts"))
P(kpse.find_file("luaotfload-main.lua","lua"), kpse.find_file("luaotfload-main","lua"))
P(kpse.find_file("pdftex.map","map"), kpse.find_file("8r.enc","enc files"),kpse.find_file("cmr10.pfb","type1 fonts"))
P(kpse.find_file("nonexistent.xyz"))
P(kpse.lookup("article.cls"))
P(kpse.lookup("article.cls",{format="tex",mustexist=true}))
P(kpse.lookup("*.cls",{all=false}))
P(kpse.expand_braces("a{b,c}d"), kpse.expand_braces("{a,b}{1,2}"))
P(kpse.readable_file("article.cls"), kpse.readable_file(rf) == rf, kpse.readable_file("/nonexistent"))
P(kpse.in_name_ok("a.tex"), kpse.in_name_ok("/etc/passwd"), kpse.out_name_ok("a.tex"), kpse.out_name_ok("/etc/passwd"), kpse.out_name_ok(".hid"), kpse.out_name_ok("../x"))
P(kpse.version())
P(kpse.record_input_file("x.tex"), kpse.record_output_file("y.tex"))
P(kpse.find_file("article.cls", "tex", false), kpse.find_file("article.cls", false))

