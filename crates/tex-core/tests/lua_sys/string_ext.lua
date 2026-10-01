lfs.mkdir("string_ext") lfs.chdir("string_ext")
local function T(t) return "{"..table.concat(t,",").."}" end
P(T(string.explode("a,b,,c")), T(string.explode("a b  c"," ")), T(string.explode("a b  c"," +")), T(string.explode("abc","")), T(string.explode("")), T(string.explode(",a,",",")))
P(T(string.explode("a,b;c",",;")), T(string.explode("a--b","--")),T(string.explode("a.b",".")))
P(string.utflength("aä€😀"), string.utflength(""))
P(T({string.utfvalue("aä€😀")}), T({string.utfvalue("")}))
P(string.utfcharacter(97,228,0x20ac,0x1f600), string.utfcharacter())
for v in string.utfvalues("aä€") do P("v",v) end
for v in string.utfcharacters("aä€") do P("c",v) end
for a,b in string.characterpairs("abc") do P("cp",a,b) end
for a in string.characters("abc") do P("ch",a) end
for a,b in string.bytepairs("abc") do P("bp",a,b) end
for a in string.bytes("abc") do P("by",a) end
P(T(string.bytetable("abc")), T(string.bytetable("")))
P(pcall(string.utfcharacter,-1))
P(pcall(string.explode,nil))
P(pcall(string.utflength,1))
P(string.utflength("\xff\xfe"))
P(T({string.utfvalue("\xff")}))

