-- fio (readers on io.open handles) and sio (the same on strings), as in
-- LuaTeX's liolibext.c, including its bounds checks.
local type, error, tostring, tointeger = type, error, tostring, math.tointeger
local format, unpack, byte = string.format, string.unpack, string.byte
local iotype = io.type
local concat = table.concat

local function argname(v)
  if v == nil then return "no value" end
  return type(v)
end

local function tofile(f, fname)
  local kind = iotype(f)
  if kind == "file" then return f end
  if kind == "closed file" then error("attempt to use a closed file", 3) end
  error(format("bad argument #1 to 'fio.%s' (FILE* expected, got %s)", fname, argname(f)), 3)
end

local function integer(v)
  if type(v) == "number" then return tointeger(v) or math.floor(v) end
  if type(v) == "string" then
    local n = tonumber(v)
    if n then return tointeger(n) or math.floor(n) end
  end
  return 0
end

local function checkstring(v, n, fname)
  local t = type(v)
  if t == "string" then return v end
  if t == "number" then return tostring(v) end
  error(format("bad argument #%d to '%s' (string expected, got %s)", n, fname, argname(v)), 3)
end

local function checkinteger(v, n, fname)
  local i = tointeger(v)
  if i == nil and type(v) == "string" then i = tointeger(tonumber(v)) end
  if i == nil then
    error(format("bad argument #%d to '%s' (number expected, got %s)", n, fname, argname(v)), 3)
  end
  return i
end

local fio, sio = {}, {}

-- every reader is described by width, signedness and byte order
local function define(name, width, signed, little)
  -- luatex computes the 4-byte cardinals in a C `int`: they wrap
  local fmt = (little and "<" or ">") .. ((signed or width == 4) and "i" or "I") .. width
  local fname = "read" .. (signed and "integer" or "cardinal") .. width .. (little and "le" or "")
  fio[fname] = function(f)
    local s = tofile(f, fname):read(width)
    if s == nil or #s < width then return nil end
    return (unpack(fmt, s))
  end
  -- the string versions need `position + width - 1 < #s`
  sio[fname] = function(s, p)
    s = checkstring(s, 1, fname)
    p = checkinteger(p, 2, fname) - 1
    if p < 0 or p + width > #s then return nil end
    return (unpack(fmt, s, p + 1))
  end
end
for width = 1, 4 do
  for _, signed in ipairs { false, true } do
    define(nil, width, signed, false)
    if width > 1 then
      define(nil, width, signed, true)
    else
      local base = "read" .. (signed and "integer" or "cardinal") .. "1"
      fio[base .. "le"] = fio[base]
      sio[base .. "le"] = sio[base]
    end
  end
end

-- tables of n values of b bytes; stops at the first incomplete value
local function table_reader(signed)
  local function values(s, p, n, b)
    local t = {}
    if b >= 1 and b <= 4 then
      local fmt = ((signed or b == 4) and ">i" or ">I") .. b
      local available = (#s - p) // b
      if available < n then n = available end
      for i = 1, n do
        t[i] = (unpack(fmt, s, p + 1 + (i - 1) * b))
      end
    end
    return t
  end
  local fname = signed and "readintegertable" or "readcardinaltable"
  fio[fname] = function(f, n, b)
    f = tofile(f, fname)
    n, b = integer(n), integer(b)
    if n <= 0 or b < 1 or b > 4 then return {} end
    local s = f:read(n * b)
    return values(s or "", 0, n, b)
  end
  sio[fname] = function(s, p, n, b)
    s = checkstring(s, 1, fname)
    p = checkinteger(p, 2, fname) - 1
    n, b = integer(n), integer(b)
    if p < 0 then return {} end
    return values(s, p, n, b)
  end
end
table_reader(false)
table_reader(true)

-- 16.16 (readfixed4), 8.8 (readfixed2) and 2.14 (read2dot14) fixed point
fio.readfixed2 = function(f)
  local s = tofile(f, "readfixed2"):read(2)
  if s == nil or #s < 2 then return nil end
  local n = unpack(">I2", s)
  return (n >> 8) + (n & 0xff) / 256.0
end
sio.readfixed2 = function(s, p)
  s = checkstring(s, 1, "readfixed2")
  p = checkinteger(p, 2, "readfixed2") - 1
  -- LuaTeX tests `p+3 >= #s` here
  if p < 0 or p + 3 >= #s then return nil end
  local n = unpack(">I2", s, p + 1)
  return (n >> 8) + (n & 0xff) / 256.0
end
fio.readfixed4 = function(f)
  local s = tofile(f, "readfixed4"):read(4)
  if s == nil or #s < 4 then return nil end
  local n = unpack(">i4", s)
  return (n // 65536) + (n & 0xffff) / 65536.0
end
sio.readfixed4 = function(s, p)
  s = checkstring(s, 1, "readfixed4")
  p = checkinteger(p, 2, "readfixed4") - 1
  if p < 0 or p + 3 >= #s then return nil end
  local n = unpack(">i4", s, p + 1)
  return (n // 65536) + (n & 0xffff) / 65536.0
end
local function dot14(n)
  local top = n >> 14
  if top >= 2 then top = top - 4 end
  return top + (n & 0x3fff) / 16384.0
end
fio.read2dot14 = function(f)
  local s = tofile(f, "read2dot14"):read(2)
  if s == nil or #s < 2 then return nil end
  return dot14((unpack(">I2", s)))
end
sio.read2dot14 = function(s, p)
  s = checkstring(s, 1, "read2dot14")
  p = checkinteger(p, 2, "read2dot14") - 1
  if p < 0 or p + 1 >= #s then return nil end
  return dot14((unpack(">I2", s, p + 1)))
end

-- positions (fseek reports 0 on success)
fio.getposition = function(f)
  local position = tofile(f, "getposition"):seek("cur")
  return position
end
fio.setposition = function(f, p)
  f = tofile(f, "setposition")
  if f:seek("set", integer(p)) == nil then return nil end
  return 0
end
fio.skipposition = function(f, p)
  f = tofile(f, "skipposition")
  local here = f:seek("cur")
  if here == nil or f:seek("set", here + integer(p)) == nil then return nil end
  return 0
end

fio.readbytes = function(f, n)
  f = tofile(f, "readbytes")
  n = integer(n)
  if n <= 0 then return end
  local s = f:read(n)
  if s == nil then return end
  return byte(s, 1, #s)
end
sio.readbytes = function(s, p, n)
  s = checkstring(s, 1, "readbytes")
  p = checkinteger(p, 2, "readbytes") - 1
  n = integer(n)
  if p < 0 or p >= #s or n <= 0 then return end
  if p + n >= #s then n = #s - p end
  return byte(s, p + 1, p + n)
end
fio.readbytetable = function(f, n)
  f = tofile(f, "readbytetable")
  n = integer(n)
  local t = {}
  if n <= 0 then return t end
  local s = f:read(n)
  if s == nil then return t end
  for i = 1, #s do t[i] = byte(s, i) end
  return t
end
sio.readbytetable = function(s, p, n)
  s = checkstring(s, 1, "readbytetable")
  p = checkinteger(p, 2, "readbytetable") - 1
  n = integer(n)
  if p < 0 or p >= #s then return nil end
  if p + n >= #s then n = #s - p end
  local t = {}
  for i = 1, n do t[i] = byte(s, p + i) end
  return t
end

-- a line ends at "\n", "\r" or "\r\n"
fio.readline = function(f)
  f = tofile(f, "readline")
  local line = f:read("L")
  if line == nil then return nil end
  local cr = line:find("\r", 1, true)
  if cr then
    local rest = #line - cr
    if rest == 1 and byte(line, #line) == 10 then return line:sub(1, cr - 1) end
    if rest > 0 then f:seek("cur", -rest) end
    return line:sub(1, cr - 1)
  end
  if byte(line, #line) == 10 then return line:sub(1, #line - 1) end
  return line
end

_G.fio = fio
_G.sio = sio
package.loaded.fio = fio
package.loaded.sio = sio
