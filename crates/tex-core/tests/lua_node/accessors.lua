local D = node.direct
local function show(v)
  local tv = type(v)
  if tv == "number" and math.type(v) == "float" then return string.format("%.4f", v) end
  if tv == "userdata" then return "<ud " .. node.type(v.id) .. ">" end
  if tv == "table" then return "<table>" end
  return tostring(v)
end
local function showd(v, direct)
  -- a direct number may be a handle; show node kinds only for fields that are nodes
  return show(v)
end
local function pack(...) return {n = select("#", ...), ...} end
local function res(r)
  if not r[1] and r.n == 0 then return "" end
  local o = {}
  for i = 1, r.n do o[#o+1] = show(r[i]) end
  return table.concat(o, ",")
end
local getters = {"getid","getsubtype","getnext","getprev","getboth","getlist","getleader","getdisc","getwhd","getglue","getchar","getfont","getfam","getkern","getwidth","getheight","getdepth","getshift","getpenalty","getnucleus","getsub","getsup","getexpansion","getlang","getcomponents","getdir","getdirection","getoffsets","getdata","getattributelist","is_zero_glue","getfield"}
local types = node.types()
local ids = {} for k in pairs(types) do ids[#ids+1] = k end table.sort(ids)
local function handle(id) return node.direct.new(id) end
local function isnode(v) return type(v) == "number" and v > 0 end
for _, id in ipairs(ids) do
  if id ~= 8 and id < 30 then
    local n = D.new(id)
    local o = {}
    for _, g in ipairs(getters) do
      local ok, r
      if g == "getfield" then ok, r = pcall(function() return pack(D.getfield(n, "width")) end)
      else ok, r = pcall(function() return pack(D[g](n)) end) end
      o[#o+1] = g .. "=" .. (ok and res(r) or "ERR")
    end
    P("get", types[id], table.concat(o, " "))
  end
end
-- setters
local function setter(name, id, ...)
  local n = D.new(id)
  local args = {...}
  local ok, err = pcall(function() return D[name](n, table.unpack(args)) end)
  return n, ok, err
end
local sets = {
  {"setsubtype", 3}, {"setfont", 1, 66}, {"setfont", 1}, {"setchar", 67}, {"setfam", 2}, {"setfam", 2, 70}, {"setkern", 11}, {"setkern", 11, 4},
  {"setwidth", 12}, {"setheight", 13}, {"setdepth", 14}, {"setshift", 15}, {"setpenalty", 16}, {"setexpansion", 17}, {"setlang", 3}, {"setdir", "TRT"}, {"setdirection", "TRT"},
  {"setoffsets", 5, 6}, {"setdata", 9}, {"setwhd", 1, 2, 3}, {"setglue", 1, 2, 3, 2, 3}, {"setglue", 1}, {"setglue"},
}
local getter_for = {setsubtype="getsubtype", setfont="getfont", setchar="getchar", setfam="getfam", setkern="getkern", setwidth="getwidth", setheight="getheight", setdepth="getdepth", setshift="getshift", setpenalty="getpenalty", setexpansion="getexpansion", setlang="getlang", setdir="getdir", setdirection="getdirection", setoffsets="getoffsets", setdata="getdata", setwhd="getwhd", setglue="getglue"}
for _, spec in ipairs(sets) do
  local name = spec[1]
  local args = {table.unpack(spec, 2)}
  local o = {}
  for _, id in ipairs(ids) do
    if id ~= 8 and id < 30 then
      local n, ok, err = setter(name, id, table.unpack(args))
      local r = "ERR"
      if ok then
        local ok2, rr = pcall(function() return pack(D[getter_for[name]](n)) end)
        r = ok2 and res(rr) or "GERR"
      end
      o[#o+1] = types[id] .. "=" .. r
    end
  end
  P("set", name, table.concat(args, ","), table.concat(o, " "))
end
-- disc / leader / list / components / nucleus
local d = D.new("disc")
local pre, post, rep = D.new("kern"), D.new("kern"), D.new("kern")
D.setdisc(d, pre, post, rep, 1, 55)
local a, b, c, st, pe = D.getdisc(d)
P("disc", a == pre, b == post, c == rep, st, pe)
local a2, b2, c2, st2, pe2 = D.getdisc(d, true)
P("disc true", a2 == pre, b2 == post, c2 == rep, st2 == pre, pe2 == post)
D.setdisc(d)
P("disc cleared", D.getdisc(d))
D.setdisc(d, pre) P("disc pre only", D.getdisc(d) == pre, select(2, D.getdisc(d)))
local g = D.new("glue") local l = D.new("glue")
D.setleader(g, l) P("leader", D.getleader(g) == l, D.getleader(D.new("kern")))
local h = D.new("hlist") local k = D.new("kern")
D.setlist(h, k) P("list", D.getlist(h) == k, D.getlist(g))
local gl = D.new("glyph") local comp = D.new("glyph")
D.setcomponents(gl, comp) P("components", D.getcomponents(gl) == comp)
local nd = D.new("noad") local nu = D.new("math_char")
D.setnucleus(nd, nu) D.setsub(nd, nu) D.setsup(nd, nu)
P("noad", D.getnucleus(nd) == nu, D.getsub(nd) == nu, D.getsup(nd) == nu)
-- setnext / setprev / setboth / setlink / setsplit
local x, y, z = D.new("kern"), D.new("kern"), D.new("kern")
D.setnext(x, y) D.setprev(y, x) P("link", D.getnext(x) == y, D.getprev(y) == x)
D.setboth(y, x, z) P("both", D.getprev(y) == x, D.getnext(y) == z)
local x2, y2, z2 = D.new("kern"), D.new("kern"), D.new("kern")
local ret = D.setlink(x2, y2, z2)
P("setlink", ret == x2, D.getnext(x2) == y2, D.getnext(y2) == z2, D.getprev(z2) == y2, D.getprev(y2) == x2)
local ret2 = D.setlink(nil, x2) P("setlink nil", ret2 == x2, D.getprev(x2))
local ret3 = D.setlink(x2, nil, z2) P("setlink nil mid", ret3 == x2, D.getnext(x2) == z2)
local l1, l2, l3 = D.new("kern"), D.new("kern"), D.new("kern")
D.setlink(l1, l2, l3)
local tail = D.setsplit(l2, nil) 
P("setsplit", tail)
