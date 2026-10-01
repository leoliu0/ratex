-- unicode.{ascii,latin1,utf8,grapheme}: slnunicode string libraries.
local S = __ratex_sys
local type, tostring, error, select, pack, unpack =
  type, tostring, error, select, table.pack, table.unpack
local format = string.format

local function argname(v)
  if v == nil then return "no value" end
  return type(v)
end

local function checkstring(v, n, fname)
  local t = type(v)
  if t == "string" then return v end
  if t == "number" then return tostring(v) end
  error(format("bad argument #%d to '?' (string expected, got %s)", n, argname(v)), 3)
end

local function checkint(v, n, fname)
  local i = math.tointeger(v)
  if i == nil and type(v) == "string" then i = math.tointeger(tonumber(v)) end
  if i == nil then
    local t = type(v)
    if t == "number" or (t == "string" and tonumber(v)) then
      error(format("bad argument #%d to '%s' (number has no integer representation)", n, fname), 3)
    end
    error(format("bad argument #%d to '?' (number expected, got %s)", n, argname(v)), 3)
  end
  return i
end

local function optint(v, default, n, fname)
  if v == nil then return default end
  return checkint(v, n, fname)
end

local function posrelat(pos, len)
  if pos >= 0 then return pos end
  return len + pos + 1
end

-- capture values from flat (init, len) pairs
local function capture_values(s, caps, first, last)
  local out, n = {}, 0
  for i = first, last, 2 do
    local init, len = caps[i], caps[i + 1]
    n = n + 1
    if len == -1 then
      error("unfinished capture", 4)
    elseif len == -2 then
      out[n] = init + 1
    else
      out[n] = s:sub(init + 1, init + len)
    end
  end
  return out, n
end

local function one_capture(s, caps, index, start0, stop0)
  local count = #caps // 2
  if index >= count then
    if index == 0 then return s:sub(start0 + 1, stop0) end
    error("invalid capture index", 4)
  end
  local init, len = caps[2 * index + 1], caps[2 * index + 2]
  if len == -1 then error("unfinished capture", 4) end
  if len == -2 then return init + 1 end
  return s:sub(init + 1, init + len)
end

local function all_captures(s, caps, start0, stop0)
  if #caps == 0 then return s:sub(start0 + 1, stop0) end
  local values, n = capture_values(s, caps, 1, #caps - 1)
  return unpack(values, 1, n)
end

local specials = "[%^%$%*%+%?%.%(%[%%%-]"

local function new_library(mode)
  local lib = {}

  function lib.len(s)
    return S.uni_len(mode, checkstring(s, 1, "len"))
  end

  function lib.sub(s, i, j)
    s = checkstring(s, 1, "sub")
    return S.uni_sub(mode, s, checkint(i, 2, "sub"), optint(j, -1, 3, "sub"))
  end

  function lib.byte(s, i, j)
    s = checkstring(s, 1, "byte")
    return unpack({ S.uni_byte(mode, s, optint(i, 1, 2, "byte"), j ~= nil and checkint(j, 3, "byte") or nil) })
  end

  function lib.char(...)
    local codes = { ... }
    for i = 1, select("#", ...) do codes[i] = checkint(codes[i], i, "char") end
    return S.uni_char(mode, codes)
  end

  function lib.lower(s) return S.uni_lower(mode, checkstring(s, 1, "lower")) end
  function lib.upper(s) return S.uni_upper(mode, checkstring(s, 1, "upper")) end
  function lib.reverse(s) return S.uni_reverse(mode, checkstring(s, 1, "reverse")) end

  function lib.rep(s, n)
    s = checkstring(s, 1, "rep")
    return string.rep(s, checkint(n, 2, "rep"))
  end

  lib.dump = string.dump
  lib.format = string.format

  function lib.gfind()
    error("'string.gfind' was renamed to 'string.gmatch'", 2)
  end

  local function find_aux(find, s, p, init, plain)
    s = checkstring(s, 1, find and "find" or "match")
    p = checkstring(p, 2, find and "find" or "match")
    local l1 = #s
    init = posrelat(optint(init, 1, 3, find and "find" or "match"), l1) - 1
    if init < 0 then init = 0 elseif init > l1 then init = l1 end
    if find and (plain or not p:find(specials)) then
      local a, b = s:find(p, init + 1, true)
      if a then return a, b end
      return nil
    end
    local anchor = p:sub(1, 1) == "^"
    if anchor then p = p:sub(2) end
    local r = pack(S.uni_scan(mode, s, p, init, anchor))
    local start0 = r[1]
    if start0 == nil then return nil end
    local stop0 = r[2]
    local caps = { unpack(r, 3, r.n) }
    if find then
      if #caps == 0 then return start0 + 1, stop0 end
      local values, n = capture_values(s, caps, 1, #caps - 1)
      return start0 + 1, stop0, unpack(values, 1, n)
    end
    return all_captures(s, caps, start0, stop0)
  end

  function lib.find(s, p, init, plain) return find_aux(true, s, p, init, plain) end
  function lib.match(s, p, init) return find_aux(false, s, p, init) end

  function lib.gmatch(s, p)
    s = checkstring(s, 1, "gmatch")
    p = checkstring(p, 2, "gmatch")
    local position = 0
    return function()
      for src = position, #s do
        local r = pack(S.uni_try(mode, s, p, src))
        local stop0 = r[1]
        if stop0 ~= nil then
          position = stop0
          if stop0 == src then position = position + 1 end
          return all_captures(s, { unpack(r, 2, r.n) }, src, stop0)
        end
      end
    end
  end

  function lib.gsub(s, p, repl, max)
    s = checkstring(s, 1, "gsub")
    p = checkstring(p, 2, "gsub")
    local srcl = #s
    max = optint(max, srcl + 1, 4, "gsub")
    local anchor = p:sub(1, 1) == "^"
    if anchor then p = p:sub(2) end
    local kind = type(repl)
    if kind == "number" then repl = tostring(repl) kind = "string" end
    if kind ~= "string" and kind ~= "function" and kind ~= "table" then
      error(format("bad argument #3 to 'gsub' (string/function/table expected, got %s)", argname(repl)), 2)
    end
    local out, n, src = {}, 0, 0
    while n < max do
      local r = pack(S.uni_try(mode, s, p, src))
      local e = r[1]
      if e ~= nil then
        n = n + 1
        local caps = { unpack(r, 2, r.n) }
        local value
        if kind == "string" then
          local pieces = {}
          local i = 1
          while i <= #repl do
            local c = repl:sub(i, i)
            if c ~= "%" then
              pieces[#pieces + 1] = c
            else
              i = i + 1
              local d = repl:sub(i, i)
              if not d:match("%d") then
                pieces[#pieces + 1] = d
              elseif d == "0" then
                pieces[#pieces + 1] = s:sub(src + 1, e)
              else
                pieces[#pieces + 1] = tostring(one_capture(s, caps, tonumber(d) - 1, src, e))
              end
            end
            i = i + 1
          end
          value = table.concat(pieces)
        else
          if kind == "function" then
            value = repl(all_captures(s, caps, src, e))
          else
            value = repl[one_capture(s, caps, 0, src, e)]
          end
          if not value then
            value = s:sub(src + 1, e)
          elseif type(value) ~= "string" and type(value) ~= "number" then
            error(format("invalid replacement value (a %s)", type(value)), 2)
          else
            value = tostring(value)
          end
        end
        out[#out + 1] = value
      end
      if e ~= nil and e > src then
        src = e
      elseif src < srcl then
        out[#out + 1] = s:sub(src + 1, src + 1)
        src = src + 1
      else
        break
      end
      if anchor then break end
    end
    out[#out + 1] = s:sub(src + 1)
    return table.concat(out), n
  end

  return lib
end

unicode = {
  ascii = new_library(0),
  latin1 = new_library(1),
  utf8 = new_library(2),
  grapheme = new_library(3),
}
package.loaded.unicode = unicode

-- string extensions of lstrlibext.c
do
  local sbyte, ssub, schar = string.byte, string.sub, string.char
  local function check(s, n, fname)
    if type(s) == "number" then return tostring(s) end
    if type(s) ~= "string" then
      error(format("bad argument #%d to 'string.%s' (string expected, got %s)", n, fname, s == nil and "nil" or type(s)), 3)
    end
    return s
  end
  local function byteiter(name, step, conv)
    return function(s)
      s = check(s, 1, name)
      local ind, len = 0, #s
      return function()
        if ind >= len then return nil end
        local i = ind
        ind = ind + step
        return conv(s, i, len)
      end
    end
  end
  string.bytes = byteiter("bytes", 1, function(s, i) return sbyte(s, i + 1) end)
  string.bytepairs = byteiter("bytepairs", 2, function(s, i, len)
    return sbyte(s, i + 1), i + 1 < len and sbyte(s, i + 2) or nil
  end)
  string.characters = byteiter("characters", 1, function(s, i) return ssub(s, i + 1, i + 1) end)
  string.characterpairs = byteiter("characterpairs", 2, function(s, i, len)
    return ssub(s, i + 1, i + 1), i + 1 < len and ssub(s, i + 2, i + 2) or ""
  end)
  function string.bytetable(s)
    s = check(s, 1, "bytetable")
    local t = {}
    for i = 1, #s do t[i] = sbyte(s, i) end
    return t
  end
  function string.explode(s, joiner)
    s = check(s, 1, "explode")
    joiner = joiner == nil and " +" or check(joiner, 2, "explode")
    local t = {}
    local l = #s
    if l == 0 then t[1] = s return t end
    if joiner == "" then
      for i = 1, l do t[i] = ssub(s, i, i) end
      return t
    end
    local j = sbyte(joiner, 1)
    local mult = sbyte(joiner, 2) == 43
    local p = 1
    if mult then
      while p <= l and sbyte(s, p) == j do p = p + 1 end
    end
    local q, n = p, 1
    local i = p
    while i <= l do
      if sbyte(s, i) == j then
        t[n] = ssub(s, q, i - 1)
        n = n + 1
        if mult then
          while sbyte(s, i + 1) == j do i = i + 1 end
        end
        q = i + 1
      end
      i = i + 1
    end
    if mult and q == l + 1 then return t end
    if q <= l + 1 then t[n] = ssub(s, q) end
    return t
  end
  local function utfenc(c)
    if c > 0x10FFFF or c < 0 then return "" end
    if c < 0x80 then return schar(c) end
    if c < 0x800 then return schar(0xC0 | (c >> 6), 0x80 | (c & 0x3F)) end
    if c < 0x10000 then
      return schar(0xE0 | (c >> 12), 0x80 | ((c >> 6) & 0x3F), 0x80 | (c & 0x3F))
    end
    return schar(0xF0 | (c >> 18), 0x80 | ((c >> 12) & 0x3F), 0x80 | ((c >> 6) & 0x3F), 0x80 | (c & 0x3F))
  end
  function string.utfcharacter(...)
    local n = select("#", ...)
    local t = {}
    for i = 1, n do
      local c = math.tointeger((select(i, ...))) or 0
      t[i] = utfenc(c)
    end
    return table.concat(t)
  end
  -- one decoded value at byte index `ind` (0-based): value or nil, size
  local function decode(s, ind, len)
    local i = sbyte(s, ind + 1)
    if i < 0x80 then return i, 1 end
    if i >= 0xF0 then
      if ind + 3 < len then
        local j, k, l = sbyte(s, ind + 2, ind + 4)
        if j >= 0x80 and k >= 0x80 and l >= 0x80 then
          return (((((i - 0xF0) * 64) + (j - 128)) * 64) + (k - 128)) * 64 + (l - 128), 4
        end
      end
      return nil, 4
    elseif i >= 0xE0 then
      if ind + 2 < len then
        local j, k = sbyte(s, ind + 2, ind + 3)
        if j >= 0x80 and k >= 0x80 then
          return (((i - 0xE0) * 64) + (j - 128)) * 64 + (k - 128), 3
        end
      end
      return nil, 3
    elseif i >= 0xC0 then
      if ind + 1 < len then
        local j = sbyte(s, ind + 2)
        if j >= 0x80 then return ((i - 0xC0) * 64) + (j - 128), 2 end
      end
      return nil, 2
    end
    return nil, 1
  end
  function string.utfvalue(s)
    s = check(s, 1, "utfvalue")
    local t, n, ind, len = {}, 0, 0, #s
    while ind < len do
      local v, size = decode(s, ind, len)
      if v then n = n + 1 t[n] = v end
      ind = ind + size
    end
    return table.unpack(t, 1, n)
  end
  function string.utflength(s)
    s = check(s, 1, "utflength")
    local ind, num, len = 0, 0, #s
    while ind < len do
      local i = sbyte(s, ind + 1)
      if i < 0x80 then ind = ind + 1
      elseif i >= 0xF0 then ind = ind + 4
      elseif i >= 0xE0 then ind = ind + 3
      elseif i >= 0xC0 then ind = ind + 2
      else ind = ind + 1 end
      num = num + 1
    end
    return num
  end
  function string.utfvalues(s)
    s = check(s, 1, "utfvalues")
    local ind, len = 0, #s
    return function()
      if ind >= len then return nil end
      local v, size = decode(s, ind, len)
      ind = ind + size
      return v or 0xFFFD
    end
  end
  function string.utfcharacters(s)
    s = check(s, 1, "utfcharacters")
    local ind, len = 0, #s
    local mask = { 0x80, 0xE0, 0xF0, 0xF8 }
    local mequ = { 0x00, 0xC0, 0xE0, 0xF0 }
    return function()
      if ind >= len then return nil end
      local c = sbyte(s, ind + 1)
      for j = 0, 3 do
        if (c & mask[j + 1]) == mequ[j + 1] then
          if ind + 1 + j > len then ind = len return "\xEF\xBF\xBD" end
          for k = 1, j do
            if (sbyte(s, ind + k + 1) & 0xC0) ~= 0x80 then
              ind = ind + k
              return "\xEF\xBF\xBD"
            end
          end
          local r = ssub(s, ind + 1, ind + 1 + j)
          ind = ind + 1 + j
          return r
        end
      end
      ind = ind + 1
      return "\xEF\xBF\xBD"
    end
  end
end
