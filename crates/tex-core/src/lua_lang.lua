-- LuaTeX `lang` library (llanglib.c) over the engine's hyphenation tables.
local L = __texres_langlib
__texres_langlib = nil

local type, error, tostring, setmetatable, getmetatable, tonumber =
      type, error, tostring, setmetatable, getmetatable, tonumber
local tointeger = math.tointeger

local lang = {}
_G.lang = lang

local function id_of(l, name)
  local id = L.lang_id(l)
  if not id then
    error("bad argument #1 to '" .. name .. "' (luatex.lang expected, got " .. type(l) .. ")", 3)
  end
  return id
end

-- llanglib.c `lang_new`: a number makes (or finds) that language, none the
-- one after the highest there is (texlang.c `new_language(-1)`)
function lang.new(id)
  if id ~= nil then
    id = tointeger(tonumber(id) or error("bad argument #1 to 'new' (number expected)", 2))
    if id == nil then error("lang.new(): undefined language", 2) end
  end
  return L.lang_new(L.new(id))
end

function lang.id(l) return id_of(l, "id") end

function lang.patterns(l, s)
  local id = id_of(l, "patterns")
  if s == nil then return L.patterns_get(id) end
  L.patterns_add(id, tostring(s))
end
function lang.clear_patterns(l) L.patterns_clear(id_of(l, "clear_patterns")) end

function lang.hyphenation(l, s)
  local id = id_of(l, "hyphenation")
  if s == nil then return L.exceptions_get(id) end
  L.exceptions_add(id, tostring(s))
end
function lang.clear_hyphenation(l) L.exceptions_clear(id_of(l, "clear_hyphenation")) end

function lang.clean(s)
  if type(s) ~= "string" then error("bad argument #1 to 'clean' (string expected, got " .. type(s) .. ")", 2) end
  return L.clean(s)
end

-- llanglib.c do_lang_hyphenate: head, tail, true
function lang.hyphenate(h, t) return node.hyphenating(h, t) end

local function param(name, which)
  lang[name] = function(l, v)
    local id = id_of(l, name)
    if v == nil then return L.param_get(id, which) end
    L.param_set(id, which, tointeger(tonumber(v)) or 0)
  end
end
param("prehyphenchar", "pre")
param("posthyphenchar", "post")
param("preexhyphenchar", "preex")
param("postexhyphenchar", "postex")
param("hyphenationmin", "min")

function lang.gethjcode(l, c) return L.hjcode_get(id_of(l, "gethjcode"), c) end
function lang.sethjcode(l, c, v) L.hjcode_set(id_of(l, "sethjcode"), c, v == nil and c or v) end

-- llanglib.c `luaopen_lang`: the "luatex.lang" metatable is its own __index
-- and holds the object methods.
do
  local mt = L.lang_mt
  mt.__index = mt
  mt.__name = "luatex.lang"
  for _, name in ipairs{ "clear_patterns", "clear_hyphenation", "patterns", "hyphenation", "prehyphenchar",
                         "posthyphenchar", "preexhyphenchar", "postexhyphenchar", "hyphenationmin",
                         "sethjcode", "gethjcode", "id" } do
    mt[name] = lang[name]
  end
end
