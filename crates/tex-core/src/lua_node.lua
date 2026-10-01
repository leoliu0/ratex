-- The public `node` and `node.direct` tables (lnodelib.c) over the natives of
-- lua_node_lib.rs. The natives work on integer handles (the node.direct
-- semantics); `node` is the same functions with userdata nodes.
local N = __ratex_nodelib
__ratex_nodelib = nil
local D = __ratex_nodedata
__ratex_nodedata = nil

local type, rawget, rawset, setmetatable, next, select, error, pairs, tostring =
  type, rawget, rawset, setmetatable, next, select, error, pairs, tostring
local tonode, todirect_ud = N.tonode, N.todirect_ud
local getfield, setfield = N.getfield, N.setfield

local direct = {}
local node = { direct = direct }

-- ---------------------------------------------------------------- data ----
local types = D.types

local function type_id(name)
  for i, v in pairs(types) do
    if v == name then return i end
  end
end

local function copy_table(src)
  local t = {}
  for i, v in pairs(src) do t[i] = v end
  return t
end

function node.types() return copy_table(types) end
function node.whatsits() return copy_table(D.whatsits) end

function node.id(name)
  if type(name) == "string" then return type_id(name) end
  return nil
end

function node.subtype(name)
  if type(name) == "string" then
    for i, v in pairs(D.whatsits) do
      if v == name then return i end
    end
  end
  return nil
end

function node.type(id)
  if type(id) == "number" then
    return types[id]
  elseif type(id) == "userdata" and N.is_node_ud(id) then
    return "node"
  end
  return nil
end

function node.values(name)
  local v = D.values[name]
  if v then return copy_table(v) end
  return nil
end

-- node ids whose subtype list is reachable by number (lnodelib.c)
local subtype_by_id = {
  [29] = "glyph", [12] = "glue", [10] = "dir", [6] = "boundary", [14] = "penalty", [13] = "kern",
  [2] = "rule", [0] = "list", [1] = "list", [5] = "adjust", [7] = "disc", [39] = "fill",
  [28] = "marginkern", [11] = "math", [18] = "noad", [19] = "radical", [21] = "accent", [22] = "fence",
}

function node.subtypes(id)
  local name
  if type(id) == "string" then
    name = id
  elseif type(id) == "number" then
    name = subtype_by_id[id]
  end
  local spec = name and D.subtypes[name]
  if spec then return copy_table(spec) end
  return nil
end

function node.fields(id, subtype)
  if type(id) == "string" then
    local i = type_id(id)
    if not i then error("invalid node type id: " .. id, 0) end
    id = i
  elseif type(id) ~= "number" then
    error("invalid node type id: " .. tostring(id), 0)
  end
  local f
  if id == 8 then
    f = D.wfields[subtype or 0]
  else
    f = D.fields[id]
  end
  if not f then error("invalid node type id: " .. tostring(id), 0) end
  return copy_table(f)
end

-- ------------------------------------------------------ plain natives ----
local passthrough = {
  "getid", "getsubtype", "setsubtype", "getnext", "getprev", "getboth", "setnext", "setprev",
  "setboth", "setlink", "setsplit", "new", "free", "flush_node", "flush_list", "copy", "copy_list",
  "remove", "insert_before", "insert_after", "slide", "tail", "end_of_math", "length", "count",
  "getfield", "setfield", "has_field", "has_attribute", "get_attribute", "find_attribute",
  "set_attribute", "unset_attribute", "current_attr", "getattributelist", "setattributelist",
  "getfont", "setfont", "getfam", "setfam", "getkern", "setkern", "getwidth", "setwidth",
  "getheight", "setheight", "getdepth", "setdepth", "getwhd", "setwhd", "getglue", "setglue",
  "getdisc", "setdisc", "getlist", "setlist", "getdata", "setdata", "getdir", "setdir",
  "getdirection", "setdirection", "getoffsets", "setoffsets", "is_char", "is_glyph", "is_node",
  "is_zero_glue", "first_glyph", "has_glyph", "flatten_discretionaries", "check_discretionaries",
  "check_discretionary", "protect_glyph", "protect_glyphs", "unprotect_glyph", "unprotect_glyphs",
  "protrusion_skippable", "effective_glue", "dimensions", "rangedimensions", "usedlist",
  "uses_font", "tostring", "fix_node_lists", "set_properties_mode",
  "getchar", "setchar", "getlang", "setlang", "getexpansion", "setexpansion", "getpenalty",
  "setpenalty", "getnucleus", "setnucleus", "getsub", "setsub", "getsup", "setsup", "getshift",
  "setshift", "getleader", "setleader", "getcomponents", "setcomponents",
}
for _, name in pairs(passthrough) do
  if N[name] then direct[name] = N[name] end
end

function direct.todirect(n)
  if type(n) == "userdata" then return todirect_ud(n) end
  return n
end
function direct.tonode(n)
  if type(n) == "number" then return tonode(n) end
  return n
end
function direct.is_direct(n) return type(n) == "number" end

-- ------------------------------------------------ engine state natives ----
direct.last_node = N.last_node
direct.write = N.write
direct.set_synctex_fields = N.set_synctex_fields
direct.get_synctex_fields = N.get_synctex_fields
function direct.prepend_prevdepth(n, prevdepth) return N.prepend_prevdepth(n, prevdepth, false) end
function direct.is_node(n) if N.is_node_ud(n) then return n end return false end

-- --------------------------------------------------------- traversal ----
local getnext, getid, getsubtype = N.getnext, N.getid, N.getsubtype

local function nil_iter() return nil end

local function next_handle(state, c)
  local t
  if c == nil then t = state else t = getnext(c) end
  if t == nil then return nil end
  return t, getid(t), getsubtype(t)
end

function direct.traverse(n)
  if n == nil or n == 0 then return nil_iter end
  return next_handle, n, nil
end

function direct.traverse_id(id, n)
  if n == nil then return nil_iter end
  if n == 0 then return end
  local function iter(state, c)
    local t
    if c == nil then t = state else t = getnext(c) end
    while t ~= nil and getid(t) ~= id do t = getnext(t) end
    if t == nil then return nil end
    return t, getsubtype(t)
  end
  return iter, n, nil
end

local getchar, getfont = direct.getchar, N.getfont
function direct.traverse_char(n)
  if n == nil or n == 0 then return nil_iter end
  local function iter(state, c)
    local t
    if c == nil then t = state else t = getnext(c) end
    while t ~= nil and (getid(t) ~= 29 or N.is_protected(t)) do
      t = getnext(t)
    end
    if t == nil then return nil end
    return t, getchar(t), getfont(t)
  end
  return iter, n, nil
end

function direct.traverse_glyph(n)
  if n == nil or n == 0 then return nil_iter end
  local function iter(state, c)
    local t
    if c == nil then t = state else t = getnext(c) end
    while t ~= nil and getid(t) ~= 29 do t = getnext(t) end
    if t == nil then return nil end
    return t, getchar(t), getfont(t)
  end
  return iter, n, nil
end

local getlist = N.getlist
function direct.traverse_list(n)
  if n == nil or n == 0 then return nil_iter end
  local function iter(state, c)
    local t
    if c == nil then t = state else t = getnext(c) end
    while t ~= nil and getid(t) ~= 0 and getid(t) ~= 1 do t = getnext(t) end
    if t == nil then return nil end
    return t, getid(t), getsubtype(t), getlist(t)
  end
  return iter, n, nil
end

-- ------------------------------------------------- hpack, vpack, ... ----
direct.hpack = N.wrap_hpack
direct.vpack = N.wrap_vpack

-- ------------------------------------------------------- properties ----
local properties = {}
-- the engine learns the table when Lua first touches properties (the
-- engine is not reachable while the library is installed)
local registered = false
local function register()
  if not registered then
    registered = true
    N.set_properties_table(properties)
  end
end

function direct.get_properties_table() register() return properties end
function direct.flush_properties_table()
  for k in next, properties do properties[k] = nil end
end
function direct.getproperty(n) register() return rawget(properties, n) end
function direct.setproperty(n, v) register() rawset(properties, n, v) end

local props_meta = {
  __index = function(t, n) return rawget(t, todirect_ud(n) or n) end,
  __newindex = function(t, n, v)
    if type(n) == "userdata" then n = todirect_ud(n) end
    rawset(t, n, v)
  end,
}
local set_properties_mode = N.set_properties_mode
function direct.set_properties_mode(basic, use_metatable)
  register()
  set_properties_mode(basic, use_metatable)
  if use_metatable == true then
    setmetatable(properties, props_meta)
  elseif use_metatable == false then
    setmetatable(properties, nil)
  end
end

-- ---------------------------------------------------- userdata flavour ----
-- Wrap a direct function: userdata arguments become handles; result
-- positions marked `n` in `rets` become userdata nodes.
local function wrap(f, rets)
  return function(...)
    local args = { ... }
    local n = select("#", ...)
    for i = 1, n do
      local a = args[i]
      if type(a) == "userdata" then args[i] = todirect_ud(a) end
    end
    local res = table.pack(f(table.unpack(args, 1, n)))
    for i = 1, res.n do
      if rets:sub(i, i) == "n" and type(res[i]) == "number" then res[i] = tonode(res[i]) end
    end
    return table.unpack(res, 1, res.n)
  end
end

local ud_specs = {
  usedlist = "n",
  copy = "n", copy_list = "n", remove = "nn", insert_before = "nn", insert_after = "nn",
  slide = "n", tail = "n", end_of_math = "n", first_glyph = "n", has_glyph = "n",
  flatten_discretionaries = "nx", getboth = "nn", getnext = "n", getprev = "n", getlist = "n",
  getleader = "n", getdisc = "nnn", new = "n", current_attr = "n", hpack = "n", vpack = "n",
  last_node = "n", free = "n", getnucleus = "n", getsub = "n", getsup = "n", getcomponents = "n",
  check_discretionaries = "", check_discretionary = "", mlist_to_hlist = "n",
  protect_glyphs = "", unprotect_glyphs = "", protect_glyph = "", unprotect_glyph = "",
  setnext = "", setprev = "", setboth = "", setlink = "", setsplit = "",
  setlist = "", setdisc = "", setleader = "", setsub = "", setsup = "", setnucleus = "",
  setcomponents = "", setfield = "", setglue = "", setwhd = "", setattributelist = "",
}
local ud_skip = { getfield = true, setfield = true, todirect = true, tonode = true, is_direct = true,
  traverse = true, traverse_id = true, traverse_char = true, traverse_glyph = true, traverse_list = true,
  getproperty = true, setproperty = true, get_properties_table = true, flush_properties_table = true,
  set_properties_mode = true, set_synctex_fields = true, get_synctex_fields = true, getbox = true,
  setbox = true, is_node = true, flush_node = true, tostring = true }

for name, f in pairs(direct) do
  if not ud_skip[name] then
    node[name] = wrap(f, ud_specs[name] or "")
  end
end

function node.is_node(n) return N.is_node_ud(n) or false end
node.tostring = N.tostring_node
function node.is_zero_glue(n)
  local r = N.is_zero_glue(todirect_ud(n))
  if r == nil then error("glue (spec) or list expected", 0) end
  return r
end
node.family_font = N.family_font
node.last_node = function() return tonode(N.last_node()) end
node.write = function(n) return N.write(todirect_ud(n)) end
function node.prepend_prevdepth(n, prevdepth) return N.prepend_prevdepth(todirect_ud(n), prevdepth, true) end
node.fix_node_lists = N.fix_node_lists
node.getfield = N.getfield_ud
node.setfield = N.setfield_ud
node.flush_node = function(n) return N.flush_node(todirect_ud(n)) end

-- userdata traversal (nodes in, nodes out)
local function node_next(state, c)
  local t
  if c == nil then t = todirect_ud(state) else t = getnext(todirect_ud(c)) end
  if t == nil then return nil end
  return tonode(t), getid(t), getsubtype(t)
end
function node.traverse(n)
  if n == nil then return nil_iter end
  return node_next, n, nil
end
function node.traverse_id(id, n)
  if n == nil then return nil_iter end
  local function iter(state, c)
    local t
    if c == nil then t = todirect_ud(state) else t = getnext(todirect_ud(c)) end
    while t ~= nil and getid(t) ~= id do t = getnext(t) end
    if t == nil then return nil end
    return tonode(t), getsubtype(t)
  end
  return iter, n, nil
end
function node.traverse_char(n)
  if n == nil then return nil_iter end
  local function iter(state, c)
    local t
    if c == nil then t = todirect_ud(state) else t = getnext(todirect_ud(c)) end
    while t ~= nil and (getid(t) ~= 29 or N.is_protected(t)) do t = getnext(t) end
    if t == nil then return nil end
    return tonode(t), getchar(t), getfont(t)
  end
  return iter, n, nil
end
function node.traverse_glyph(n)
  if n == nil then return nil_iter end
  local function iter(state, c)
    local t
    if c == nil then t = todirect_ud(state) else t = getnext(todirect_ud(c)) end
    while t ~= nil and getid(t) ~= 29 do t = getnext(t) end
    if t == nil then return nil end
    return tonode(t), getchar(t), getfont(t)
  end
  return iter, n, nil
end
function node.traverse_list(n)
  if n == nil then return nil_iter end
  local function iter(state, c)
    local t
    if c == nil then t = todirect_ud(state) else t = getnext(todirect_ud(c)) end
    while t ~= nil and getid(t) ~= 0 and getid(t) ~= 1 do t = getnext(t) end
    if t == nil then return nil end
    return tonode(t), getid(t), getsubtype(t), tonode(getlist(t))
  end
  return iter, n, nil
end

function node.getproperty(n) register() return rawget(properties, todirect_ud(n)) end
function node.setproperty(n, v) register() rawset(properties, todirect_ud(n), v) end
node.get_properties_table = direct.get_properties_table
node.flush_properties_table = direct.flush_properties_table
node.set_properties_mode = direct.set_properties_mode

node.next = node.getnext
node.prev = node.getprev

-- only the members LuaTeX 1.24 has
local function restrict(t, names)
  local keep = {}
  for _, name in pairs(names) do keep[name] = true end
  for name in pairs(t) do
    if not keep[name] then t[name] = nil end
  end
end
restrict(node, {
  "check_discretionaries", "check_discretionary", "copy", "copy_list", "count", "current_attr",
  "dimensions", "direct", "effective_glue", "end_of_math", "family_font", "fields",
  "find_attribute", "first_glyph", "fix_node_lists", "flatten_discretionaries", "flush_list",
  "flush_node", "flush_properties_table", "free", "get_attribute", "get_properties_table",
  "getboth", "getchar", "getdisc", "getfield", "getfont", "getglue", "getid", "getleader",
  "getlist", "getnext", "getprev", "getproperty", "getsubtype", "getwhd", "has_attribute",
  "has_field", "has_glyph", "hpack", "hyphenating", "id", "insert_after", "insert_before",
  "is_char", "is_glyph", "is_node", "is_zero_glue", "kerning", "last_node", "length",
  "ligaturing", "make_extensible", "mlist_to_hlist", "new", "next", "prepend_prevdepth",
  "prev", "protect_glyph", "protect_glyphs", "protrusion_skippable", "rangedimensions",
  "remove", "set_attribute", "set_properties_mode", "setfield", "setglue", "setproperty",
  "slide", "subtype", "subtypes", "tail", "tostring", "traverse", "traverse_char",
  "traverse_glyph", "traverse_id", "traverse_list", "type", "types", "unprotect_glyph",
  "unprotect_glyphs", "unset_attribute", "usedlist", "uses_font", "values", "vpack",
  "whatsits", "write",
})
restrict(direct, {
  "check_discretionaries", "check_discretionary", "copy", "copy_list", "count", "current_attr",
  "dimensions", "effective_glue", "end_of_math", "find_attribute", "first_glyph",
  "flatten_discretionaries", "flush_list", "flush_node", "flush_properties_table", "free",
  "get_attribute", "get_properties_table", "get_synctex_fields", "getattributelist", "getboth",
  "getbox", "getchar", "getcomponents", "getdata", "getdepth", "getdir", "getdirection",
  "getdisc", "getexpansion", "getfam", "getfield", "getfont", "getglue", "getheight", "getid",
  "getkern", "getlang", "getleader", "getlist", "getnext", "getnucleus", "getoffsets",
  "getpenalty", "getprev", "getproperty", "getshift", "getsub", "getsubtype", "getsup",
  "getwhd", "getwidth", "has_attribute", "has_field", "has_glyph", "hpack", "hyphenating",
  "insert_after", "insert_before", "is_char", "is_direct", "is_glyph", "is_node",
  "is_zero_glue", "kerning", "last_node", "length", "ligaturing", "new", "prepend_prevdepth",
  "protect_glyph", "protect_glyphs", "protrusion_skippable", "rangedimensions", "remove",
  "set_attribute", "set_properties_mode", "set_synctex_fields", "setattributelist", "setboth",
  "setbox", "setchar", "setcomponents", "setdata", "setdepth", "setdir", "setdirection",
  "setdisc", "setexpansion", "setfam", "setfield", "setfont", "setglue", "setheight",
  "setkern", "setlang", "setleader", "setlink", "setlist", "setnext", "setnucleus",
  "setoffsets", "setpenalty", "setprev", "setproperty", "setshift", "setsplit", "setsub",
  "setsubtype", "setsup", "setwhd", "setwidth", "slide", "tail", "todirect", "tonode",
  "tostring", "traverse", "traverse_char", "traverse_glyph", "traverse_id", "traverse_list",
  "unprotect_glyph", "unprotect_glyphs", "unset_attribute", "usedlist", "uses_font", "vpack",
  "write",
})
node.direct = direct

return node
