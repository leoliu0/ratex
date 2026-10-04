-- status, texconfig and the lua table.
local S = __texres_sys
local type, error, select = type, error, select

local function value(name)
  local kind, int, str = S.status_field(name)
  if kind == 1 then return int end
  if kind == 2 then return str end
  if kind == 3 then return int ~= 0 end
  return nil
end

local status = {}

function status.list()
  local t = {}
  for _, name in ipairs { S.status_names() } do t[name] = value(name) end
  return t
end

function status.resetmessages() S.status_resetmessages() end

function status.setexitcode(code)
  local c = math.tointeger(code)
  if c == nil then
    error(string.format("bad argument #1 to 'setexitcode' (number expected, got %s)", code == nil and "no value" or type(code)), 2)
  end
  S.status_setexitcode(c)
end

setmetatable(status, {
  __index = function(_, key)
    if type(key) == "string" then return value(key) end
  end,
  __newindex = function() end,
})
_G.status = status
package.loaded.status = status

-- LuaTeX consults texconfig once at startup; the table starts empty.
_G.texconfig = {}

-- the lua table: what llualib.c exports
local lua = _G.lua
lua.id = nil
lua.version = "Lua 5.3"
function lua.newtable(narr, nrec)
  if math.tointeger(narr) == nil then
    error("bad argument #1 to 'newtable' (number expected, got " .. (narr == nil and "no value" or type(narr)) .. ")", 2)
  end
  if math.tointeger(nrec) == nil then
    error("bad argument #2 to 'newtable' (number expected, got " .. (nrec == nil and "no value" or type(nrec)) .. ")", 2)
  end
  return {}
end
function lua.getstacktop(...) return select("#", ...) end
function lua.getcalllevel() return S.lua_calllevel() end
function lua.getcodepage() return false, false end
lua.startupfile = nil
