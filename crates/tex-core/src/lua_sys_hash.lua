-- md5 and sha2 as exported by LuaTeX.
local S = __ratex_sys
local type, tostring, error, select = type, tostring, error, select
local format, pack = string.format, string.pack

local function checkstring(v, n, fname)
  local t = type(v)
  if t == "string" then return v end
  if t == "number" then return tostring(v) end
  error(format("bad argument #%d to '%s' (string expected, got %s)", n, fname, t == "nil" and "no value" or t), 3)
end

local md5 = {}
function md5.sum(message)
  return S.md5_sum(checkstring(message, 1, "md5.sum"))
end
function md5.exor(a, b)
  a, b = checkstring(a, 1, "md5.exor"), checkstring(b, 2, "md5.exor")
  if #a ~= #b then error("bad argument #2 to 'md5.exor' (lengths must be equal)", 2) end
  local out, n = {}, 0
  for i = 1, #a, 4096 do
    local x = a:sub(i, i + 4095)
    local y = b:sub(i, i + 4095)
    local bytes = { x:byte(1, -1) }
    local other = { y:byte(1, -1) }
    for j = 1, #bytes do bytes[j] = bytes[j] ~ other[j] end
    n = n + 1
    out[n] = string.char(table.unpack(bytes))
  end
  return table.concat(out)
end
function md5.crypt(message, key, ...)
  message, key = checkstring(message, 1, "md5.crypt"), checkstring(key, 2, "md5.crypt")
  local seed
  if select("#", ...) == 0 then
    seed = pack("=j", os.time())
  else
    seed = checkstring((...), 3, "md5.crypt")
  end
  return S.md5_crypt(message, key, seed)
end
function md5.decrypt(cipher, key)
  return S.md5_decrypt(checkstring(cipher, 1, "md5.decrypt"), checkstring(key, 2, "md5.decrypt"))
end

local sha2 = {}
function sha2.digest256(s) if type(s) == "string" then return S.sha2_256(s) end end
function sha2.digest384(s) if type(s) == "string" then return S.sha2_384(s) end end
function sha2.digest512(s) if type(s) == "string" then return S.sha2_512(s) end end

_G.md5 = md5
_G.sha2 = sha2
package.loaded.md5 = md5
package.loaded.sha2 = sha2
