-- LuaTeX `lang` library (llanglib.c) over the engine's hyphenation tables.
local L = __ratex_langlib
__ratex_langlib = nil

local type, error, tostring, setmetatable, getmetatable, tonumber =
      type, error, tostring, setmetatable, getmetatable, tonumber
local tointeger = math.tointeger

local lang = {}
_G.lang = lang

local next_id = 0

local function id_of(l, name)
  local id = L.lang_id(l)
  if not id then
    error("bad argument #1 to '" .. name .. "' (luatex.lang expected, got " .. type(l) .. ")", 3)
  end
  return id
end

function lang.new(id)
  if id == nil then
    id = next_id
  else
    id = tointeger(tonumber(id) or error("bad argument #1 to 'new' (number expected)", 2))
  end
  L.check(id)
  if id >= next_id then next_id = id + 1 end
  return L.lang_new(id)
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
