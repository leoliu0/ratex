-- mime.core, ltn12 and mime as LuaTeX ships them (LuaSocket's own Lua files
-- are run unchanged).
local S = __ratex_sys
local ltn12_source, mime_source = __ratex_ltn12_source, __ratex_mime_source
__ratex_ltn12_source, __ratex_mime_source = nil, nil

local core = {
  _VERSION = "MIME 1.0.3",
  b64 = S.mime_b64,
  unb64 = S.mime_unb64,
  qp = S.mime_qp,
  unqp = S.mime_unqp,
  wrp = S.mime_wrp,
  qpwrp = S.mime_qpwrp,
  eol = S.mime_eol,
  dot = S.mime_dot,
}
package.loaded["mime.core"] = core

-- ltn12.lua calls the legacy `module("ltn12")` after `ltn12 = _M`: it adds
-- _M, _NAME and _PACKAGE to the table that is already registered under that
-- name. The chunk runs with a private `module` doing exactly that.
local env = setmetatable({}, { __index = _G })
function env.module(name)
  local t = env.ltn12
  t._M = t
  t._NAME = name
  t._PACKAGE = ""
  package.loaded[name] = t
  _G[name] = t
end
local chunk = assert(load(ltn12_source, "=ltn12", "t", env))
local ltn12 = chunk()
package.loaded.ltn12 = ltn12
_G.ltn12 = ltn12

local mime_chunk = assert(load(mime_source, "=mime", "t"))
mime_chunk()
