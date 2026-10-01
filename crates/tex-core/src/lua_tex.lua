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
function tex.isbox(k)
  local t = type(k)
  if t == "number" then return k >= 0 and k <= 65535 end
  if t == "string" then return (T.register_kind(k) or ""):match("^box ") ~= nil end
  return false
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

-- xoshiro256** (the generator of Lua 5.4's math.random)
do
  local s0, s1, s2, s3 = 0, 0, 0, 0
  local function rotl(x, n) return (x << n) | (x >> (64 - n)) end
  local function nextrand()
    local r = rotl(s1 * 5, 7) * 9
    local t = s1 << 17
    s2 = s2 ~ s0; s3 = s3 ~ s1; s1 = s1 ~ s2; s0 = s0 ~ s3
    s2 = s2 ~ t; s3 = rotl(s3, 45)
    return r
  end
  local function seed(n1, n2)
    s0, s1, s2, s3 = n1, 0xff, n2, 0
    for _ = 1, 16 do nextrand() end
  end
  seed(0, 0)
  local function project(ran, lim)
    if lim & (lim + 1) == 0 then return ran & lim end
    local l = lim
    l = l | (l >> 1); l = l | (l >> 2); l = l | (l >> 4); l = l | (l >> 8); l = l | (l >> 16); l = l | (l >> 32)
    ran = ran & l
    while math.ult(lim, ran) do ran = nextrand() & l end
    return ran
  end
  function tex.lua_math_randomseed(a, b)
    if a == nil then a = os.time() end
    seed(lua_int(a), lua_int(b))
  end
  function tex.lua_math_random(m, n)
    local r = nextrand()
    local low, up
    if m == nil then
      return (r >> 11) * (0.5 / (1 << 52))
    end
    if n == nil then low, up = 1, lua_int(m) else low, up = lua_int(m), lua_int(n) end
    if low > up then error("bad argument #" .. (n == nil and 1 or 2) .. " to 'lua_math_random' (interval is empty)", 2) end
    return (low + project(r, up - low)) + 0.0
  end
end

-- ------------------------------------------------------ mode and page ---

function tex.forcehmode(indented)
  T.force_hmode(indented ~= false)
end
function tex.triggerbuildpage() T.trigger_build_page() end
function tex.getpagestate() return T.page_state() end
function tex.resetparagraph() T.reset_paragraph() end

local local_level = 0
do
  local runtoks = tex.runtoks
  function tex.runtoks(...)
    local_level = local_level + 1
    local ok, err = pcall(runtoks, ...)
    local_level = local_level - 1
    if not ok then error(err, 0) end
  end
end
function tex.getlocallevel() return local_level end
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
