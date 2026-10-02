-- Walks every pdfe function over the PDFs named in `arg` (FIXDIR..name) and
-- writes one line per call to OUT. Expected outputs: `luatex --luaonly` with
-- the same script (see README in this directory's expected/*.txt headers).
local OUTF=assert(io.open(OUT,"wb"))
local function P(...) local t=table.pack(...) for i=1,t.n do t[i]=tostring(t[i]) end OUTF:write(table.concat(t,"\t"),"\n") end
local function fmt(v)
  if type(v)=="userdata" then
    local s=tostring(v):gsub("0x%x+","PTR"):gsub("%d+(%.%d+)","%0")
    return "UD["..tostring(pdfe.type(v)).."]"
  elseif type(v)=="number" then
    return (math.type(v)=="integer" and "i" or "f")..string.format("%.14g",v)
  elseif type(v)=="table" then
    local ks={} for k in pairs(v) do ks[#ks+1]=k end
    table.sort(ks,function(a,b) return tostring(a)<tostring(b) end)
    local o={} for _,k in ipairs(ks) do o[#o+1]=tostring(k).."="..fmt(v[k]) end
    return "{"..table.concat(o,",").."}"
  elseif type(v)=="string" and #v>60 then local h=0 for i=1,#v do h=(h*31+v:byte(i))%1000000007 end return "string<len="..#v..",hash="..h..">"
  else return type(v)..":"..tostring(v) end
end
local function show(tag,...) local t=table.pack(...) local o={} for i=1,t.n do o[i]=fmt(t[i]) end P(tag, t.n, table.concat(o," | ")) end
local function try(tag,f,...) local r=table.pack(pcall(f,...)) if r[1] then show(tag,table.unpack(r,2,r.n)) else P(tag,"ERR",(tostring(r[2]):gsub("^.-:%d+: ",""))) end end
local function walk(tag,o,depth)
  local ty=pdfe.type(o)
  if depth>(DEPTH or 2) then return end
  if ty=="pdfe.dictionary" then
    try(tag.." len",function() return #o end)
    local n=#o
    for i=1,n do
      try(tag.." fd"..i,pdfe.getfromdictionary,o,i)
    end
    try(tag.." d2t",pdfe.dictionarytotable,o)
    try(tag.." d2t flat",pdfe.dictionarytotable,o,true)
    for i=1,n do
      local k=pdfe.getfromdictionary(o,i)
      try(tag.." idx["..k.."]",function() return o[k] end)
      try(tag.." gs",pdfe.getstring,o,k)
      try(tag.." gs1",pdfe.getstring,o,k,true)
      try(tag.." gs2",pdfe.getstring,o,k,false)
      try(tag.." gi",pdfe.getinteger,o,k)
      try(tag.." gn",pdfe.getnumber,o,k)
      try(tag.." gb",pdfe.getboolean,o,k)
      try(tag.." gname",pdfe.getname,o,k)
      try(tag.." gd",pdfe.getdictionary,o,k)
      try(tag.." ga",pdfe.getarray,o,k)
      try(tag.." gst",pdfe.getstream,o,k)
      local v=o[k]
      if type(v)=="userdata" then walk(tag.."/"..k,v,depth+1) end
    end
    try(tag.." idxnum",function() return o[1] end)
    try(tag.." idxbad",function() return o.Nope,o[true] end)
  elseif ty=="pdfe.array" then
    local n=#o
    try(tag.." len",function() return n end)
    try(tag.." a2t",pdfe.arraytotable,o)
    try(tag.." a2t flat",pdfe.arraytotable,o,true)
    for i=0,n+1 do
      try(tag.." fa"..i,pdfe.getfromarray,o,i)
      try(tag.." idx"..i,function() return o[i] end)
      try(tag.." gs",pdfe.getstring,o,i)
      try(tag.." gs1",pdfe.getstring,o,i,true)
      try(tag.." gs2",pdfe.getstring,o,i,false)
      try(tag.." gi",pdfe.getinteger,o,i)
      try(tag.." gn",pdfe.getnumber,o,i)
      try(tag.." gb",pdfe.getboolean,o,i)
      try(tag.." gname",pdfe.getname,o,i)
      try(tag.." gd",pdfe.getdictionary,o,i)
      try(tag.." ga",pdfe.getarray,o,i)
      try(tag.." gst",pdfe.getstream,o,i)
      local v=o[i]
      if type(v)=="userdata" then walk(tag.."/"..i,v,depth+1) end
    end
  elseif ty=="pdfe.stream" then
    try(tag.." len",function() return #o end)
    for i=1,#o do try(tag.." fs"..i,pdfe.getfromstream,o,i) end
    try(tag.." fs name",pdfe.getfromstream,o,"Length")
    try(tag.." rws",pdfe.readwholestream,o)
    try(tag.." rws1",pdfe.readwholestream,o,true)
    try(tag.." rws2",pdfe.readwholestream,o,false)
    try(tag.." call",function() return o() end)
    try(tag.." call1",function() return o(true) end)
    try(tag.." readfrom(unopened)",pdfe.readfromstream,o)
    try(tag.." open",pdfe.openstream,o)
    try(tag.." read1",pdfe.readfromstream,o)
    try(tag.." read2",pdfe.readfromstream,o)
    try(tag.." close",pdfe.closestream,o)
    try(tag.." open dec",pdfe.openstream,o,true)
    try(tag.." readdec",pdfe.readfromstream,o)
    try(tag.." close",pdfe.closestream,o)
    try(tag.." read after close",pdfe.readfromstream,o)
    for i=1,#o do try(tag.." idx"..i,function() return o[i] end) end
    try(tag.." idx Length",function() return o.Length,o.Filter end)
  end
end
for _,f in ipairs(arg) do
  P("=====",f)
  local d=pdfe.open(FIXDIR..f)
  try("open",function() return d end)
  if d then
  try("type",pdfe.type,d)
  try("ts",function() return (tostring(d):gsub("0x%x+","PTR")) end)
  try("version",pdfe.getversion,d)
  try("size",pdfe.getsize,d)
  try("status",pdfe.getstatus,d)
  try("nofobjects",pdfe.getnofobjects,d)
  try("nofpages",pdfe.getnofpages,d)
  try("memusage",function() local a,b=pdfe.getmemoryusage(d) return math.type(a),math.type(b) end)
  try("catalog",pdfe.getcatalog,d)
  try("trailer",pdfe.gettrailer,d)
  try("info",pdfe.getinfo,d)
  try("idx",function() return d.catalog,d.Catalog,d.info,d.Info,d.trailer,d.Trailer,d.pages,d.Pages,d.foo end)
  try("getpages",function() local p=pdfe.getpages(d) return #p,p[1] end)
  local np=pdfe.getnofpages(d) or 0
  for i=-1,np+1 do try("getpage"..i,pdfe.getpage,d,i) end
  for i=1,np do
   for _,b in ipairs{"MediaBox","CropBox","BleedBox","TrimBox","ArtBox","Foo"} do
    try("getbox"..i..b,pdfe.getbox,d,i,b)
   end
  end
  try("getbox page dict",function() return pdfe.getbox(pdfe.getpage(d,1),"MediaBox") end)
  try("getbox nostr",function() return pdfe.getbox(pdfe.getpage(d,1)) end)
  try("pagestotable",function() local t=pdfe.pagestotable(d) local o={} for i,v in ipairs(t) do o[#o+1]=fmt(v) end return table.concat(o,";") end)
  try("trailer walk",function() walk("trailer",pdfe.gettrailer(d),0) end)
  try("catalog walk",function() walk("catalog",pdfe.getcatalog(d),0) end)
  try("info walk",function() walk("info",pdfe.getinfo(d),0) end)
  for i=1,np do try("page walk",function() walk("page"..i,pdfe.getpage(d,i),0) end) end
  try("unencrypt",pdfe.unencrypt,d)
  try("unencrypt2",pdfe.unencrypt,d,"")
  try("unencrypt3",pdfe.unencrypt,d,"user","owner")
  try("unencrypt4",pdfe.unencrypt,d,nil,"owner")
  try("getstatus",pdfe.getstatus,d)
  try("close",pdfe.close,d)
  try("close2",pdfe.close,d)
  end
end

OUTF:close()
