-- LuaTeX `tex` library, beyond the registers of lua_bridge.lua (ltexlib.c).
local B, T = __ratex_bridge, __ratex_texlib
__ratex_bridge, __ratex_texlib = nil, nil

local type, select, error, tostring, tonumber, pairs, rawget, setmetatable, getmetatable =
      type, select, error, tostring, tonumber, pairs, rawget, setmetatable, getmetatable
local floor, tointeger = math.floor, math.tointeger
local tex, texio, token = tex, texio, token

-- Remove names that real LuaTeX does not export from the tex table.
tex.chardef, tex.luatexversion, tex.luatexrevision, tex.luatexbanner = nil, nil, nil, nil

-- ------------------------------------------------------------- helpers ---

local function lua_int(v)
  return tointeger(tonumber(v) or 0) or 0
end

-- "global" as an optional first argument of the setters.
local function scoped(...)
  local n = select("#", ...)
  local a1 = ...
  if a1 == "global" and n > 1 then return true, select(2, ...) end
  return false, ...
end

local function key_args(k, what)
  local t = type(k)
  if t == "string" then return nil, k end
  if t == "number" then return k, nil end
  error("argument of '" .. what .. "' must be a string or a number", 3)
end

-- glue_spec nodes (ltexlib.c uses node-list specs for glue parameters)
local function new_spec(w, st, sh, so, sho)
  local n = node.new("glue_spec")
  n.width, n.stretch, n.shrink = w, st, sh
  n.stretch_order, n.shrink_order = so, sho
  return n
end
local function spec_values(v)
  if type(v) == "table" or type(v) == "userdata" then
    return { v.width or 0, v.stretch or 0, v.shrink or 0, v.stretch_order or 0, v.shrink_order or 0 }
  end
  error("unsupported value type", 3)
end

-- ------------------------------------------------------ pure functions ---

function tex.sp(s)
  local t = type(s)
  if t == "number" then return s end
  if t ~= "string" then error("argument must be a string or a number", 2) end
  return T.sp(s)
end

function tex.round(x)
  return floor((tonumber(x) or 0) + 0.5)
end

function tex.scale(x, f)
  local function one(n)
    local v = floor(n * f + 0.5)
    return tointeger(v) or v
  end
  if type(x) == "table" then
    local out = {}
    for k, v in pairs(x) do out[k] = one(tonumber(v) or 0) end
    return out
  end
  return one(tonumber(x) or 0)
end

function tex.badness(t, s)
  t, s = lua_int(t), lua_int(s)
  if t <= 0 then return 0 end
  if s <= 0 then return 10000 end
  local r
  if t <= 7230584 then r = (t * 297) // s
  elseif s >= 1663497 then r = t // (s // 297)
  else r = t end
  if r > 1290 then return 10000 end
  return (r * r * r + 0x20000) // 0x40000
end

function tex.number(n)
  return lua_int(n)
end

function tex.romannumeral(n)
  n = lua_int(n)
  if n <= 0 then return "" end
  local out = {}
  for _, e in ipairs { { 1000, "m" }, { 900, "cm" }, { 500, "d" }, { 400, "cd" }, { 100, "c" }, { 90, "xc" },
                       { 50, "l" }, { 40, "xl" }, { 10, "x" }, { 9, "ix" }, { 5, "v" }, { 4, "iv" }, { 1, "i" } } do
    while n >= e[1] do out[#out + 1] = e[2]; n = n - e[1] end
  end
  return table.concat(out)
end

-- ------------------------------------------------------- tex.get / set ---

local specials = {
  luatexversion = function() return 124 end,
  luatexrevision = function() return "0" end,
  luatexbanner = function() return "This is LuaTeX, Version 1.24.0" end,
  jobname = T.job_name,
  formatname = T.format_name,
  fontname = function() return T.font_name(T.cur_font()) end,
  fontid = function() return T.cur_font() end,
}

local function param_get(name, raw)
  local kind, a, b, c, d, e, text = T.pget(name)
  if kind == "int" or kind == "dim" then return a end
  if kind == "toks" then return text end
  if kind == "glue" or kind == "muglue" then
    if raw then return a, b, c, d, e end
    return new_spec(a, b, c, d, e)
  end
  local f = specials[name]
  if f then return f() end
  return nil
end

local function dim_value(v)
  if type(v) == "string" then return T.sp(v) end
  if type(v) == "number" then
    local i = tointeger(v)
    return i or floor(v + 0.5)
  end
  error("unsupported value type", 3)
end

local function int_value(v)
  if type(v) ~= "number" then error("unsupported value type", 3) end
  return lua_int(v)
end

local function param_set(global, name, value)
  local kind = T.pget(name)
  if kind == "int" then
    T.pset(name, global, { int_value(value) })
  elseif kind == "dim" then
    T.pset(name, global, { dim_value(value) })
  elseif kind == "glue" or kind == "muglue" then
    T.pset(name, global, spec_values(value))
  elseif kind == "toks" then
    if type(value) ~= "string" then error("unsupported value type", 3) end
    T.pset(name, global, {}, value)
  end
end

function tex.get(name, raw)
  if type(name) ~= "string" then return nil end
  return param_get(name, raw)
end

function tex.set(...)
  local global, name, value = scoped(...)
  if type(name) ~= "string" then error("argument must be a string", 2) end
  param_set(global, name, value)
end

-- --------------------------------------------------------------- glue ---

local function glue_funcs(mu)
  local function getglue(k)
    local i, name = key_args(k, mu and "getmuglue" or "getglue")
    return T.glue_get(mu, i, name)
  end
  local function setglue(...)
    local global, k, w, st, sh, so, sho = scoped(...)
    local i, name = key_args(k, mu and "setmuglue" or "setglue")
    T.glue_set(mu, i, name, global, { lua_int(w), lua_int(st), lua_int(sh), lua_int(so), lua_int(sho) })
  end
  local function getskip(k)
    local i, name = key_args(k, mu and "getmuskip" or "getskip")
    return new_spec(T.glue_get(mu, i, name))
  end
  local function setskip(...)
    local global, k, spec = scoped(...)
    local i, name = key_args(k, mu and "setmuskip" or "setskip")
    if type(spec) ~= "userdata" and type(spec) ~= "table" then
      error("argument of '" .. (mu and "setmuskip" or "setskip") .. "' must be a string or a number", 2)
    end
    T.glue_set(mu, i, name, global, spec_values(spec))
  end
  local width_view = setmetatable({}, {
    __index = function(_, k) return (getglue(k)) end,
    __newindex = function(_, k, v) setglue(k, v, 0, 0, 0, 0) end,
  })
  local skip_view = setmetatable({}, {
    __index = function(_, k) return getskip(k) end,
    __newindex = function(_, k, v) setskip(k, v) end,
  })
  return getglue, setglue, getskip, setskip, width_view, skip_view
end
tex.getglue, tex.setglue, tex.getskip, tex.setskip, tex.glue, tex.skip = glue_funcs(false)
tex.getmuglue, tex.setmuglue, tex.getmuskip, tex.setmuskip, tex.muglue, tex.muskip = glue_funcs(true)

-- ------------------------------------------------------ character codes ---

local function code_funcs(kind, getname, setname, pack, unpack)
  local function get(c)
    local v = T.code_get(kind, lua_int(c), "get" .. getname)
    return v
  end
  local function set(...)
    local global, c, v = scoped(...)
    T.code_set(kind, lua_int(c), lua_int(v), global, "set" .. setname)
  end
  return get, set
end

tex.getlccode, tex.setlccode = code_funcs("lc", "lccode", "lccode")
tex.getuccode, tex.setuccode = code_funcs("uc", "uccode", "uccode")
do
  -- tex.setlccode(c, lc [, uc]) and tex.setuccode(c, uc [, lc])
  local setlc, setuc = tex.setlccode, tex.setuccode
  function tex.setlccode(...)
    local global, c, lc, uc = scoped(...)
    setlc(global and "global" or c, global and c or lc, global and lc or nil)
    if uc ~= nil then setuc(global and "global" or c, global and c or uc, global and uc or nil) end
  end
  function tex.setuccode(...)
    local global, c, uc, lc = scoped(...)
    setuc(global and "global" or c, global and c or uc, global and uc or nil)
    if lc ~= nil then setlc(global and "global" or c, global and c or lc, global and lc or nil) end
  end
end
do
  local getcatcode = tex.getcatcode
  function tex.getcatcode(a, b)
    local c = b == nil and a or b
    if type(c) ~= "number" or c < 0 or c > 0x10FFFF then
      error("incorrect character value " .. tostring(c) .. " for tex.getcatcode()", 2)
    end
    return getcatcode(a, b)
  end
end
tex.getsfcode, tex.setsfcode = code_funcs("sf", "sfcode", "sfcode")

local function view(get, set)
  return setmetatable({}, {
    __index = function(_, c) return get(c) end,
    __newindex = function(_, c, v) set(c, v) end,
  })
end
tex.lccode = view(tex.getlccode, tex.setlccode)
tex.uccode = view(tex.getuccode, tex.setuccode)
tex.sfcode = view(tex.getsfcode, tex.setsfcode)
tex.catcode = view(tex.getcatcode, function(c, v) return tex.setcatcode(c, v) end)

-- math codes are (class, family, character) with a 21 bit character; the
-- natives pass them packed as class | family << 4 | char << 12. Delimiter
-- codes are (small fam, small char, large fam, large char) packed as
-- (smallfam + 1) | smallchar << 9 | largefam << 30 | largechar << 38.
function tex.getmathcodes(c)
  local v = T.code_get("math", lua_int(c), "getmathcodes")
  return v & 15, (v >> 4) & 255, v >> 12
end
function tex.getmathcode(c)
  local class, fam, char = tex.getmathcodes(c)
  return { class, fam, char }
end
function tex.setmathcode(...)
  local global, c, class, fam, char = scoped(...)
  if type(class) == "table" then
    class, fam, char = class[1], class[2], class[3]
  end
  class, fam, char = lua_int(class), lua_int(fam), lua_int(char)
  T.code_set("math", lua_int(c), (class & 15) | ((fam & 255) << 4) | (char << 12), global, "setmathcode")
end
function tex.getdelcodes(c)
  local v = T.code_get("del", lua_int(c), "getdelcodes")
  local sf = (v & 0x1FF) - 1
  if sf < 0 then return -1, 0, 0, 0 end
  return sf, (v >> 9) & 0x1FFFFF, (v >> 30) & 255, (v >> 38) & 0x1FFFFF
end
function tex.getdelcode(c)
  local a, b, cc, d = tex.getdelcodes(c)
  return { a, b, cc, d }
end
function tex.setdelcode(...)
  local global, c, sf, sc, lf, lc = scoped(...)
  if type(sf) == "table" then
    sf, sc, lf, lc = sf[1], sf[2], sf[3], sf[4]
  end
  sf, sc, lf, lc = lua_int(sf), lua_int(sc), lua_int(lf or 0), lua_int(lc or 0)
  T.code_set("del", lua_int(c), ((sf + 1) & 0x1FF) | (sc << 9) | ((lf & 255) << 30) | (lc << 38), global, "setdelcode")
end
tex.mathcode = setmetatable({}, {
  __index = function(_, c) return tex.getmathcode(c) end,
  __newindex = function(_, c, v) tex.setmathcode(c, v) end,
})
tex.delcode = setmetatable({}, {
  __index = function(_, c) return tex.getdelcode(c) end,
  __newindex = function(_, c, v) tex.setdelcode(c, v) end,
})

-- -------------------------------------------------------------- misc ---

local marks = { topmark = 0, firstmark = 1, botmark = 2, splitfirstmark = 3, splitbotmark = 4 }
function tex.getmark(name, class)
  local which = marks[name]
  if not which then return nil end
  return T.mark_get(which, lua_int(class))
end

function tex.fontname(f) return T.font_name(lua_int(f)) end
function tex.fontidentifier(f) return T.font_identifier(lua_int(f)) end

-- tex.isX(k): the register number for a number or a \xxxdef name, else false
-- (ltexlib.c `get_item_index`-based predicates); isdimen & co. accept the
-- same.
local function is_kind(kind)
  return function(k)
    local t = type(k)
    if t == "number" then return k end
    if t == "string" then
      local r = T.register_kind(k)
      if r then
        local what, n = r:match("^(%a+) (%d+)$")
        if what == kind then return tonumber(n) end
      end
    end
    return false
  end
end

-- -------------------------------------------- boxes, lists and the nest ---
-- ltexlib.c get_box_id: a number, or the name of a \chardef'd register
local ND = node.direct
local nd_tonode, nd_todirect = ND.tonode, ND.todirect

local function box_id(k, report)
  local t = type(k)
  if t == "string" then return T.box_index(k) or -1 end
  if t == "number" then return tointeger(k) or 0 end
  if report then error("argument must be a string or a number", 0) end
  return -1
end

local function last_arg(...)
  local n = select("#", ...)
  if n == 0 then return nil end
  return (select(n, ...))
end

local function direct_node(v)
  if type(v) ~= "userdata" or not node.is_node(v) then
    error("(node lib): lua <node> expected, not an object with type " .. type(v), 0)
  end
  return nd_todirect(v)
end

function tex.isbox(k)
  local id = box_id(k, false)
  return id >= 0 and id <= 65535
end

function tex.getbox(...)
  local h = T.box_get(box_id(last_arg(...), true))
  return h and nd_tonode(h)
end

function tex.setbox(...)
  local n = select("#", ...)
  local global = n == 3 and (...) == "global"
  local k = box_id(n >= 2 and (select(n - 1, ...)) or nil, true)
  if k < 0 or k > 65535 then error("incorrect index specification for tex.setbox()", 0) end
  local v = last_arg(...)
  if type(v) == "boolean" then
    if v then return end
    v = nil
  end
  if v == nil then
    T.box_set(k, nil, global)
    return
  end
  local h = direct_node(v)
  local id = ND.getid(h)
  if id ~= 0 and id ~= 1 then
    error("setbox: incompatible node type (" .. node.type(id) .. ")\n", 0)
  end
  T.box_set(k, h, global)
end

function tex.splitbox(k, h, m)
  local id = box_id(k, true)
  if tonumber(h) then
    local mode = 1
    if type(m) == "string" then
      if m == "exactly" then mode = 0 elseif m == "additional" then mode = 1 end
    elseif type(m) == "number" then
      mode = tointeger(m) or 0
    end
    if mode < 0 or mode > 1 then error("wrong mode in splitbox", 0) end
    local r = T.splitbox(id, floor(tonumber(h) + 0.5), mode == 0)
    return r and nd_tonode(r)
  end
  return nil
end

function tex.shipout(k)
  T.shipout(box_id(k, true))
end

tex.box = view(tex.getbox, tex.setbox)

-- lists of the page builder
function tex.getlist(...)
  local name = last_arg(...)
  if type(name) ~= "string" then return nil end
  local v = T.list_get(name)
  if v == nil then return nil end
  if name == "least_page_cost" or name == "best_size" then return v end
  return nd_tonode(v)
end

function tex.setlist(...)
  local first = ...
  local top = type(first) == "table" and 2 or 1
  local name, v = select(top, ...)
  if type(name) ~= "string" then return end
  if name == "best_size" or name == "least_page_cost" then
    T.list_set(name, lua_int(v))
  else
    T.list_set(name, v ~= nil and direct_node(v) or nil)
  end
end
tex.lists = setmetatable({}, {
  __index = function(_, k) return tex.getlist(k) end,
  __newindex = function(_, k, v) tex.setlist(k, v) end,
})

-- the semantic nest
function tex.getnest(...)
  local n = select("#", ...)
  local p = -1
  local ptr = T.nest_ptr()
  if n == 0 then
    p = ptr
  else
    local k = (select(n, ...))
    if type(k) == "number" then
      k = tointeger(k) or 0
      if k >= 0 and k <= ptr then p = k end
    elseif type(k) == "string" then
      if k == "top" then p = ptr elseif k == "ptr" then return ptr end
    end
  end
  if p > -1 then return T.nest_get(p) end
  return nil
end

function tex.setnest()
  error("You can't modify the semantic nest array directly", 0)
end
tex.nest = setmetatable({}, {
  __index = function(_, k) return tex.getnest(k) end,
  __newindex = function() tex.setnest() end,
})

-- tex.linebreak(head, params) -> lines, info
local lb_ints = {
  "pretolerance", "tracingparagraphs", "tolerance", "looseness", "adjustspacing", "adjdemerits",
  "protrudechars", "linepenalty", "lastlinefit", "doublehyphendemerits", "finalhyphendemerits",
  "hangafter", "interlinepenalty", "widowpenalty", "clubpenalty", "brokenpenalty",
}
local lb_dims = { "emergencystretch", "hangindent", "hsize" }
function tex.linebreak(head, params)
  local h = direct_node(head)
  if type(params) ~= "table" or select("#", head, params) ~= 2 then params = {} end
  local p = {}
  for _, k in ipairs(lb_ints) do
    local v = params[k]
    if type(v) == "number" then p[k] = tointeger(v) or 0 end
  end
  for _, k in ipairs(lb_dims) do
    local v = params[k]
    if type(v) == "number" then p[k] = floor(v + 0.5) end
  end
  for _, k in ipairs { "leftskip", "rightskip" } do
    local v = params[k]
    if v ~= nil then
      local d = nd_todirect(v)
      p[k] = { ND.getfield(d, "width"), ND.getfield(d, "stretch"), ND.getfield(d, "shrink"),
               ND.getfield(d, "stretch_order"), ND.getfield(d, "shrink_order") }
    end
  end
  if type(params.parshape) == "table" then
    local flat = {}
    for _, e in pairs(params.parshape) do
      local a, b = 0, 0
      if type(e) == "table" and type(e[1]) == "number" and type(e[2]) == "number" then
        a, b = floor(e[1] + 0.5), floor(e[2] + 0.5)
      end
      flat[#flat + 1], flat[#flat + 2] = a, b
    end
    p.parshape = flat
  end
  for _, k in ipairs { "interlinepenalties", "clubpenalties", "widowpenalties" } do
    if type(params[k]) == "table" then
      local arr = {}
      for _, v in pairs(params[k]) do
        arr[#arr + 1] = type(v) == "number" and (tointeger(v) or 0) or 0
      end
      p[k] = arr
    end
  end
  local first, demerits, looseness, prevdepth, prevgraf = T.linebreak(h, p)
  return first and first ~= 0 and nd_tonode(first) or nil,
    { demerits = demerits, looseness = looseness, prevdepth = prevdepth, prevgraf = prevgraf }
end

tex.run, tex.finish, tex.show_context = T.run, T.finish, T.show_context

-- tex.getmath / tex.setmath: the \Umath parameters (ltexlib.c)
local math_param_names = {
  "quad", "axis", "operatorsize", "overbarkern", "overbarrule", "overbarvgap", "underbarkern",
  "underbarrule", "underbarvgap", "radicalkern", "radicalrule", "radicalvgap", "radicaldegreebefore",
  "radicaldegreeafter", "radicaldegreeraise", "stackvgap", "stacknumup", "stackdenomdown",
  "fractionrule", "fractionnumvgap", "fractionnumup", "fractiondenomvgap", "fractiondenomdown",
  "fractiondelsize", "skewedfractionhgap", "skewedfractionvgap", "limitabovevgap", "limitabovebgap",
  "limitabovekern", "limitbelowvgap", "limitbelowbgap", "limitbelowkern", "nolimitsubfactor",
  "nolimitsupfactor", "underdelimitervgap", "underdelimiterbgap", "overdelimitervgap",
  "overdelimiterbgap", "subshiftdrop", "supshiftdrop", "subshiftdown", "subsupshiftdown", "subtopmax",
  "supshiftup", "supbottommin", "supsubbottommax", "subsupvgap", "spaceafterscript",
  "connectoroverlapmin",
}
local math_first_mu_glue = #math_param_names
do
  local classes = { "ord", "op", "bin", "rel", "open", "close", "punct", "inner" }
  for _, a in ipairs(classes) do
    for _, b in ipairs(classes) do math_param_names[#math_param_names + 1] = a .. b .. "spacing" end
  end
end
local math_style_names = {
  "display", "crampeddisplay", "text", "crampedtext", "script", "crampedscript", "scriptscript",
  "crampedscriptscript",
}
local math_param_index, math_style_index = {}, {}
for i, n in ipairs(math_param_names) do math_param_index[n] = i - 1 end
for i, n in ipairs(math_style_names) do math_style_index[n] = i - 1 end

-- luaL_argerror: luatex's `debug` has no getinfo, so the name is the global
-- one (`pushglobalfuncname`), as when the function is called through pcall
local function bad_arg(fname, i, msg)
  error("bad argument #" .. i .. " to 'tex." .. fname .. "' (" .. msg .. ")", 0)
end

-- luaL_checkoption: index of the option, or an argument error
local function check_option(fname, v, i, index)
  local t = type(v)
  if t == "number" then v, t = tostring(v), "string" end
  if t ~= "string" then
    bad_arg(fname, i, "string expected, got " .. (v == nil and "no value" or t))
  end
  local k = index[v]
  if k == nil then bad_arg(fname, i, "invalid option '" .. v .. "'") end
  return k
end

function tex.getmath(...)
  if select("#", ...) ~= 2 then return nil end
  local name, style = ...
  local i = check_option("getmath", name, 1, math_param_index)
  local j = check_option("getmath", style, 2, math_style_index)
  if i < math_first_mu_glue then return (T.math_get(i, j)) end
  local w, st, sh, so, sho = T.math_get(i, j)
  if w == nil then return nil end
  return new_spec(w, st, sh, so, sho)
end

function tex.setmath(...)
  local n = select("#", ...)
  if n ~= 3 and n ~= 4 then return end
  local global = n == 4 and (...) == "global"
  local name, style, value = select(n - 2, ...)
  local i = check_option("setmath", name, n - 2, math_param_index)
  local j = check_option("setmath", style, n - 1, math_style_index)
  if i >= math_first_mu_glue then
    local d = nd_todirect(value)
    T.math_set_glue(i, j, ND.getfield(d, "width"), ND.getfield(d, "stretch"), ND.getfield(d, "shrink"),
      ND.getfield(d, "stretch_order"), ND.getfield(d, "shrink_order"), global)
  elseif type(value) == "number" then
    T.math_set(i, j, floor(value + 0.5), global)
  else
    error("argument must be a number", 0)
  end
end

function tex.permitmathobsolete(on) T.permit_math_obsolete(on and true or false) end

-- box resources (XObject forms; ltexlib.c tex_save_box_resource & co.)
local null_flag = -0x40000000

-- ext_xn_over_d: x * n / d rounded half away from zero, in floating point
local function ext_xn_over_d(x, n, d)
  local r = (x * 1.0 * n) / d
  if r > 2.220446049250313e-16 then r = r + 0.5 else r = r - 0.5 end
  if r >= 2147483647.0 or r <= -2147483647.0 then r = 1073741823.0 end
  if r >= 0 then return floor(r) end
  return -floor(-r)
end

function tex.saveboxresource(box, attr, res, immediate, ty, margin)
  local is_node = type(box) ~= "number"
  local what = is_node and direct_node(box) or tointeger(box) or 0
  return T.box_resource_save(what, is_node, type(attr) == "string" and attr or nil,
    type(res) == "string" and res or nil, immediate == true,
    type(margin) == "number" and (tointeger(margin) or 0) or nil)
end

function tex.getboxresourcedimensions(index)
  if type(index) ~= "number" then return nil, nil, nil, nil end
  local w, h, d, m = T.box_resource_dimensions(tointeger(index) or 0)
  if w == nil then error("(pdf backend): xform object " .. index .. " does not exist", 0) end
  return w, h, d, m
end

function tex.getboxresourcebox(index)
  local h = T.box_resource_box(tointeger(index) or 0)
  return h and nd_tonode(h)
end

function tex.useboxresource(index, w, h, d)
  if type(index) ~= "number" then return nil, nil, nil, nil end
  local nw, nh, nd = T.box_resource_dimensions(tointeger(index) or 0)
  if nw == nil then error("(pdf backend): xform object " .. index .. " does not exist", 0) end
  local aw = type(w) == "number" and floor(w + 0.5) or null_flag
  local ah = type(h) == "number" and floor(h + 0.5) or null_flag
  local ad = type(d) == "number" and floor(d + 0.5) or null_flag
  local dw, dh, dd = nw, nh, nd
  if aw ~= null_flag or ah ~= null_flag or ad ~= null_flag then
    if aw ~= null_flag and ah ~= null_flag and ad ~= null_flag then
      dw, dh, dd = aw, ah, ad
    elseif aw ~= null_flag then
      dw = aw
      if ah ~= null_flag then
        dh = ah
        dd = ext_xn_over_d(ah, nd, nh)
      elseif ad ~= null_flag then
        dd = ad
        dh = ext_xn_over_d(aw, nh + nd, nw) - ad
      else
        dh = ext_xn_over_d(aw, nh, nw)
        dd = ext_xn_over_d(aw, nd, nw)
      end
    elseif ah ~= null_flag then
      dh = ah
      if ad ~= null_flag then
        dd = ad
        dw = ext_xn_over_d(ah + ad, nw, nh + nd)
      else
        dw = ext_xn_over_d(ah, nw, nh)
        dd = ext_xn_over_d(ah, nd, nh)
      end
    else
      dd = ad
      dh = nh - (ad - nd)
      dw = nw
    end
  end
  local rule = node.new("rule", 1)
  rule.index = tointeger(index) or 0
  rule.width, rule.height, rule.depth = dw, dh, dd
  return rule, dw, dh, dd
end

-- token.scan_glue([mu]) -> glue_spec; token.scan_list() -> box node
function token.scan_glue(mu)
  return new_spec(T.scan_glue(mu and true or false))
end
function token.scan_list()
  local h = T.scan_list()
  return h and nd_tonode(h)
end
tex.iscount, tex.isdimen, tex.isskip, tex.ismuskip = is_kind("count"), is_kind("dimen"), is_kind("skip"), is_kind("muskip")
tex.istoks, tex.isattribute, tex.isglue, tex.ismuglue = is_kind("toks"), is_kind("attribute"), tex.isskip, tex.ismuskip

-- ---------------------------------------------------- random numbers ---

function tex.init_rand(seed)
  if type(seed) ~= "number" then error("argument must be a number", 2) end
  T.rand_init(lua_int(seed))
end
function tex.uniform_rand(x)
  if type(x) ~= "number" then error("argument must be a number", 2) end
  return T.uniform_rand(lua_int(x))
end
function tex.normal_rand() return T.normal_rand() end

-- ltexlib.c: `lua_math_randomseed` is `init_rand`; `lua_math_random` is the
-- Lua interface of `math.random` over the TeX generator.
function tex.lua_math_randomseed(seed)
  if type(seed) ~= "number" then error("argument must be a number", 2) end
  T.rand_init(lua_int(seed))
end
do
  local function number_arg(i, v)
    local x = tonumber(v)
    if type(v) == "boolean" or x == nil or type(v) == "table" then
      error("bad argument #" .. i .. " to 'tex.lua_math_random' (number expected, got " .. (v == nil and "no value" or type(v)) .. ")", 3)
    end
    return x + 0.0
  end
  function tex.lua_math_random(...)
    local max = 0x7fffffff
    local r = T.uniform_rand(max)
    if r < 0 then r = -r end
    r = r / max
    local n = select("#", ...)
    if n == 0 then return r end
    if n == 1 then
      local u = number_arg(1, (...))
      if not (1.0 <= u) then error("bad argument #1 to 'tex.lua_math_random' (interval is empty)", 2) end
      return math.floor(r * u) + 1.0
    elseif n == 2 then
      local l, u = ...
      l, u = number_arg(1, l), number_arg(2, u)
      if not (l <= u) then error("bad argument #2 to 'tex.lua_math_random' (interval is empty)", 2) end
      return math.floor(r * (u - l + 1)) + l
    end
    error("wrong number of arguments", 2)
  end
end

-- ------------------------------------------------------ mode and page ---

function tex.forcehmode(indented)
  T.force_hmode(indented ~= false)
end
function tex.triggerbuildpage() T.trigger_build_page() end
function tex.getpagestate() return T.page_state() end
function tex.resetparagraph() T.reset_paragraph() end

function tex.getlocallevel() return T.local_level() end
function tex.quittoks() T.quit_local() end

function tex.hashtokens() return T.hash_tokens() end

function tex.scantoks(...)
  local global, reg, cat, text = scoped(...)
  local i, name = key_args(reg, "scantoks")
  T.scan_toks_into(i, name, lua_int(cat), tostring(text), global)
end

function tex.definefont(...)
  local global, name, id = scoped(...)
  if type(name) ~= "string" then error("argument must be a string", 2) end
  T.define_font(name, lua_int(id), global)
end

function tex.get_synctex_mode() return T.synctex_get("mode") end
function tex.get_synctex_tag() return T.synctex_get("tag") end
function tex.get_synctex_line() return T.synctex_get("line") end
function tex.set_synctex_mode(v) T.synctex_set("mode", lua_int(v)) end
function tex.set_synctex_tag(v) T.synctex_set("tag", lua_int(v)) end
function tex.set_synctex_line(v) T.synctex_set("line", lua_int(v)) end
function tex.force_synctex_tag(v) T.synctex_set("tag", lua_int(v)) end
function tex.force_synctex_line(v) T.synctex_set("line", lua_int(v)) end
function tex.set_synctex_no_files() T.synctex_set("nofiles", 0) end

tex.uniformdeviate = tex.uniform_rand
function tex.getmodevalues() return { [0] = "unset", [1] = "vertical", [134] = "horizontal", [267] = "math" } end

function texio.setescape(on)
  if type(on) ~= "boolean" then error("boolean expected", 2) end
  T.texio_setescape(on)
end
function texio.closeinput() T.texio_closeinput() end

-- Dispatch for assignments to and reads of parameters: tex.hsize = ...
setmetatable(tex, {
  __index = function(_, k)
    if type(k) ~= "string" then return nil end
    return param_get(k)
  end,
  __newindex = function(_, k, v)
    if type(k) ~= "string" then error("argument must be a string", 2) end
    param_set(false, k, v)
  end,
})
