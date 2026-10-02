local function keys(t) local k = {} for n in pairs(t) do if n ~= "make_extensible" then k[#k+1] = n end end table.sort(k) return table.concat(k, " ") end
P("NODE", keys(node))
P("DIRECT", keys(node.direct))
local t = node.types()
local ids = {} for k in pairs(t) do ids[#ids+1] = k end table.sort(ids)
for _, k in ipairs(ids) do P("type", k, t[k]) end
local w = node.whatsits()
ids = {} for k in pairs(w) do ids[#ids+1] = k end table.sort(ids)
for _, k in ipairs(ids) do P("whatsit", k, w[k]) end
local function dump(id, sub)
  local f = node.fields(id, sub)
  local ks = {} for k in pairs(f) do ks[#ks+1] = k end table.sort(ks)
  local o = {} for _, k in ipairs(ks) do o[#o+1] = k .. "=" .. f[k] end
  return table.concat(o, " ")
end
for _, k in ipairs(ids) do end
local tids = {} for k in pairs(t) do tids[#tids+1] = k end table.sort(tids)
for _, k in ipairs(tids) do if k ~= 8 then P("fields", k, dump(k)) end end
local wids = {} for k in pairs(w) do wids[#wids+1] = k end table.sort(wids)
for _, k in ipairs(wids) do P("wfields", k, dump(8, k)) end
P(pcall(node.fields, 99))
P(pcall(node.fields, "foo"))
for _, n in ipairs{"dir","direction","glue","pdf_literal","pdf_action","pdf_window","color_stack","pagestate","foo"} do
  local v = node.values(n)
  if v then local ks={} for k in pairs(v) do ks[#ks+1]=k end table.sort(ks) local o={} for _,k in ipairs(ks) do o[#o+1]=k.."="..tostring(v[k]) end P("values",n,table.concat(o," ")) else P("values",n,"nil") end
end
for _, n in ipairs{"glyph","glue","dir","boundary","penalty","kern","rule","list","hlist","vlist","adjust","disc","fill","leader","marginkern","math","noad","radical","accent","fence","pdf_destination","pdf_literal","foo","whatsit","margin_kern",0,1,2,5,6,7,10,11,12,13,14,18,19,21,22,28,29,39,100} do
  local v = node.subtypes(n)
  if v then local ks={} for k in pairs(v) do ks[#ks+1]=k end table.sort(ks) local o={} for _,k in ipairs(ks) do o[#o+1]=k.."="..tostring(v[k]) end P("subtypes",n,table.concat(o," ")) else P("subtypes",n,"nil") end
end
P(node.subtype("pdf_literal"), node.subtype("foo"), node.id("glyph"), node.id("foo"), node.type(29), node.type(99), node.type(node.new("glue")), node.type({}), node.type("glyph"))
