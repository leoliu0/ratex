local function show(v)
  local tv = type(v)
  if tv == "userdata" then return "<node " .. node.type(v.id) .. ">" end
  if tv == "table" then return "<table>" end
  return tostring(v)
end
local t = node.types()
local ids = {} for k in pairs(t) do ids[#ids+1] = k end table.sort(ids)
local function dump(id, sub)
  local ok, n = pcall(node.new, id, sub)
  if not ok then P("new", id, sub, "ERR", n) return end
  local f = node.fields(id, sub)
  local ks = {} for k in pairs(f) do ks[#ks+1] = k end table.sort(ks)
  local o = {}
  for _, k in ipairs(ks) do
    local name = f[k]
    local ok2, v = pcall(function() return n[name] end)
    o[#o+1] = name .. "=" .. (ok2 and show(v) or "ERR:" .. tostring(v))
  end
  P("new", id, sub, node.type(id), table.concat(o, " "))
end
for _, id in ipairs(ids) do
  if id ~= 4 and id ~= 8 and id ~= 38 and id ~= 39 and id ~= 40 then
    dump(id)
  end
end
for sub = 0, 33 do if node.whatsits()[sub] then dump(8, sub) end end
for _, st in ipairs{0,1,2,3,100,101} do dump(12, st) dump(29, st) dump(13, st) dump(14, st) dump(0, st) end
P(pcall(node.new, "foo"))
P(pcall(node.new, 99))
P(pcall(node.new, 8))
P(select("#", pcall(node.new, 8, 99)))
P(pcall(node.new))
