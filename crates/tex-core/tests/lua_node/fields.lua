local function show(v)
  local tv = type(v)
  if tv == "userdata" then return "<node " .. node.type(v.id) .. ">" end
  return tostring(v)
end
local t = node.types()
local ids = {} for k in pairs(t) do ids[#ids+1] = k end table.sort(ids)
local function try(id, sub)
  local n = node.new(id, sub)
  local f = node.fields(id, sub)
  local ks = {} for k in pairs(f) do ks[#ks+1] = k end table.sort(ks)
  for _, k in ipairs(ks) do
    local name = f[k]
    if (id == 29 and (name == "width" or name == "height" or name == "depth")) then goto cont end
    if name ~= "id" and name ~= "next" and name ~= "prev" and name ~= "attr" and name ~= "subtype" then
      local d = n[name]
      local tv = type(d)
      local tries = {}
      if tv == "number" then tries = {7, 1.5, "8", -3}
      elseif tv == "string" then tries = {"abc", 5}
      else tries = {node.new("kern"), 7, "TRT", "abc"} end
      local o = {}
      for _, val in ipairs(tries) do
        local ok, err = pcall(function() n[name] = val end)
        local r
        if ok then
          local ok2, v = pcall(function() return n[name] end)
          r = ok2 and show(v) or ("RERR:" .. tostring(v))
        else r = "ERR:" .. tostring(err) end
        o[#o+1] = show(val) .. "=>" .. r
      end
      P("field", node.type(id), sub or "", name, show(d), table.concat(o, " | "))
    end
    ::cont::
  end
  -- invalid fields
  P("invalid", node.type(id), sub or "", show(n.foo), pcall(function() n.foo = 1 end))
  P("hasfield", node.type(id), sub or "", node.has_field(n, "foo"), node.has_field(n, "width"), node.has_field(n, "next"), node.has_field(n, "subtype"), node.has_field(n, "attr"))
end
for _, id in ipairs(ids) do
  if id ~= 4 and id ~= 8 and id < 30 then try(id) end
end
for _, sub in ipairs{3, 4, 7, 9, 18, 19, 20, 21, 26, 29, 30, 31, 32} do try(8, sub) end
