-- Calls every pdfe function with assorted arguments over obj.pdf and writes
-- one line per call to OUT; the expected output comes from `luatex --luaonly`.
local OUTF=assert(io.open(OUT,"wb"))
local function fmt(v)
  if type(v)=="userdata" then return "UD["..tostring(pdfe.type(v)).."]"
  elseif type(v)=="number" then return (math.type(v)=="integer" and "i" or "f")..string.format("%.14g",v)
  elseif type(v)=="table" then local ks={} for k in pairs(v) do ks[#ks+1]=k end table.sort(ks,function(a,b) return tostring(a)<tostring(b) end) local o={} for _,k in ipairs(ks) do o[#o+1]=tostring(k).."="..fmt(v[k]) end return "{"..table.concat(o,",").."}"
  else return type(v)..":"..tostring(v) end end
local function try(tag,f,...) local r=table.pack(pcall(f,...)) local o={} if r[1] then for i=2,r.n do o[#o+1]=fmt(r[i]) end OUTF:write(tag,"\t",r.n-1,"\t",table.concat(o," | "),"\n") else OUTF:write(tag,"\tERR\t",(tostring(r[2]):gsub("^.-:%d+: ",""):gsub("0x%x+","PTR")),"\n") end end
local d=pdfe.open(FIXDIR.."obj.pdf")
local page=pdfe.getpage(d,1)
local cat=pdfe.getcatalog(d)
local extra=pdfe.getarray(cat,"Extra")
local a={pdfe.getstream(pdfe.getarray(pdfe.getpage(d,1),"Contents"),0)}
local s=a[1]
local trailer=pdfe.gettrailer(d)
local _,_,ref=pdfe.getfromdictionary(trailer,"Root")
local refud=select(2,pdfe.getfromdictionary(trailer,"Root"))
local args={ {}, {1}, {"x"}, {d}, {page}, {extra}, {s}, {refud}, {nil,1}, {d,1}, {d,"x"}, {page,nil}, {page,true}, {page,1.5}, {page,"Type"}, {page,{}}, {extra,"x"}, {extra,0}, {extra,-1}, {extra,1.5}, {refud,"Pages"}, {refud,0}, {page,"Type",true}, {page,"Type",false}, {extra,8,true}, {extra,8,false}, {extra,8,1} }
local names={} for name in pairs(pdfe) do names[#names+1]=name end table.sort(names)
for _,name in ipairs(names) do local f=pdfe[name]
  if type(f)=="function" and name~="getfromstream" and name~="getmemoryusage" and name~="close" and name~="unencrypt" and name~="new" and name~="open" then
    for i,a in ipairs(args) do try(name.."#"..i, f, table.unpack(a, 1, 3)) end
  end
end
for i,a in ipairs{{},{1},{FIXDIR.."obj.pdf",1},{FIXDIR.."nonexist.pdf"},{""},{FIXDIR.."obj.pdf"}} do try("open#"..i, function(...) local x=pdfe.open(...) return x and pdfe.getsize(x) end, table.unpack(a)) end
local f=io.open(FIXDIR.."obj.pdf","rb") local str=f:read("a") f:close()
local function nsz(...) local x=pdfe.new(...) if type(x)=="userdata" then return pdfe.getsize(x) end return x end
for i,a in ipairs{{},{str},{str,#str},{str,#str-3},{str,#str+100},{str,0},{str,"x"},{str,2.5},{str,#str,"id1"},{str,#str,"id1"},{str,#str,""},{1,2},{nil,2}} do try("new#"..i, nsz, table.unpack(a,1,3)) end
for i,a in ipairs{{s,1},{s,"Length"},{s,99},{s,0},{s,"Nope"},{s},{1,1},{nil,1}} do try("getfromstream#"..i, pdfe.getfromstream, table.unpack(a,1,2)) end
try("mem", function() local a,b=pdfe.getmemoryusage(d) return math.type(a),math.type(b) end)
-- tostring / metatable / type
try("ts", function() return (tostring(d):gsub("0x%x+","PTR")), (tostring(page):gsub("0x%x+","PTR")), (tostring(extra):gsub("0x%x+","PTR")), (tostring(s):gsub("0x%x+","PTR")), tostring(refud) end)
try("type()", function() return type(d),type(page),type(extra),type(s),type(refud) end)
try("len", function() return #page,#extra,#s end)
try("index", function() return d.catalog~=nil, d.nothing, page.Type, page[1], extra[1], extra[9], extra[100], s.Length, s[1], s.Nope end)
try("call", function() return s() end)
try("newindex", function() page.Foo=1 end)
try("newindex2", function() extra[1]=1 end)
try("close", pdfe.close, d)
try("close again", pdfe.close, d)
try("close bad", pdfe.close, 1)
try("unencrypt bad", pdfe.unencrypt, 1)
try("unencrypt none", pdfe.unencrypt)
OUTF:close()
