-- zlib (lzlib 0.4), gzip (lgzip) and zip (LuaZip 1.2.2) as exported by LuaTeX.
local S = __ratex_sys
local type, tostring, error, select, setmetatable, getmetatable, pcall =
  type, tostring, error, select, setmetatable, getmetatable, pcall
local format = string.format

local function argname(v)
  if v == nil then return "no value" end
  return type(v)
end

local function checkstring(v, n, fname)
  local t = type(v)
  if t == "string" then return v end
  if t == "number" then return tostring(v) end
  error(format("bad argument #%d to '%s' (string expected, got %s)", n, fname, argname(v)), 3)
end

local function optint(v, default, n, fname)
  if v == nil then return default end
  local i = math.tointeger(v)
  if i == nil and type(v) == "string" then i = math.tointeger(tonumber(v)) end
  if i == nil then
    error(format("bad argument #%d to '%s' (number expected, got %s)", n, fname, argname(v)), 3)
  end
  return i
end

local function pointer(t)
  local mt = getmetatable(t)
  setmetatable(t, nil)
  local text = tostring(t)
  setmetatable(t, mt)
  return (text:gsub("^table: ", ""))
end

------------------------------------------------------------------------ zlib

local raw_open = io.open
local zlib = {}

function zlib.version() return "1.3.2" end

function zlib.adler32(...)
  if select("#", ...) == 0 then return 1.0 end
  local adler, data = ...
  adler = optint(adler, 0, 1, "adler32")
  return S.zlib_adler32(adler, checkstring(data, 2, "adler32")) + 0.0
end

function zlib.crc32(...)
  if select("#", ...) == 0 then return 0.0 end
  local crc, data = ...
  crc = optint(crc, 0, 1, "crc32")
  return S.zlib_crc32(crc, checkstring(data, 2, "crc32")) + 0.0
end

function zlib.compress(data, level, method, window_bits, mem_level, strategy)
  data = checkstring(data, 1, "compress")
  level = optint(level, -1, 2, "compress")
  method = optint(method, 8, 3, "compress")
  window_bits = optint(window_bits, 15, 4, "compress")
  optint(mem_level, 8, 5, "compress")
  optint(strategy, 0, 6, "compress")
  if method ~= 8 then return nil, -2.0 end
  local out, code = S.zlib_compress(data, level, window_bits)
  return out, code + 0.0
end

function zlib.decompress(data, window_bits)
  data = checkstring(data, 1, "decompress")
  window_bits = optint(window_bits, 15, 2, "decompress")
  local out, code = S.zlib_decompress(data, window_bits)
  if out == nil then return nil, code + 0.0 end
  return out, code + 0.0
end

local zmeta = { __name = "zlib.zstream" }
zmeta.__index = zmeta

local function stream(z, kind, fname)
  if getmetatable(z) ~= zmeta then
    error(format("bad argument #1 to '%s' (zlib.zstream expected, got %s)", fname, argname(z)), 3)
  end
  local k = z.id and S.zlib_stream_kind(z.id)
  if k == nil or (kind and k ~= kind) then
    error(format("bad argument #1 to '%s' (attempt to use invalid zlib stream)", fname), 3)
  end
  return z.id
end

function zlib.compressobj(level, method, window_bits, mem_level, strategy)
  level = optint(level, -1, 1, "compressobj")
  method = optint(method, 8, 2, "compressobj")
  window_bits = optint(window_bits, 15, 3, "compressobj")
  optint(mem_level, 8, 4, "compressobj")
  optint(strategy, 0, 5, "compressobj")
  if method ~= 8 then error("failed to start decompressing", 2) end
  return S.ud_new(S.zlib_new_deflate(level, window_bits), zmeta)
end

function zlib.decompressobj(window_bits)
  window_bits = optint(window_bits, 15, 1, "decompressobj")
  return S.ud_new(S.zlib_new_inflate(window_bits), zmeta)
end

function zmeta.compress(z, data)
  local id = stream(z, "deflate", "compress")
  return S.zlib_stream_compress(id, checkstring(data, 2, "compress"), false)
end

function zmeta.decompress(z, data)
  local id = stream(z, "inflate", "decompress")
  return S.zlib_stream_decompress(id, checkstring(data, 2, "decompress"))
end

function zmeta.flush(z)
  local id = stream(z, nil, "flush")
  if S.zlib_stream_kind(id) == "inflate" then return "" end
  return S.zlib_stream_compress(id, "", true)
end

function zmeta.reset(z)
  return S.zlib_stream_reset(stream(z, nil, "reset")) + 0.0
end

function zmeta.adler(z)
  return S.zlib_stream_adler(stream(z, nil, "adler")) + 0.0
end

function zmeta.close(z)
  S.zlib_stream_close(stream(z, nil, "close"))
  z.id = nil
end

function zmeta.__tostring(z)
  local kind = z.id and S.zlib_stream_kind(z.id)
  if kind == nil then return "zlib stream (closed)" end
  return format("zlib %s stream (%s)", kind, S.address(z))
end

function zmeta.__gc(z)
  if z.id then S.zlib_stream_close(z.id) z.id = nil end
end

------------------------------------------------------- in-memory file readers

-- Reading side shared by gzip files and files inside zip archives: the
-- whole content is held in memory, `pos` is the 0-based offset.
local function read_line(f)
  local data, pos = f.data, f.pos
  if pos >= #data then f.past = true return nil end
  local nl = data:find("\n", pos + 1, true)
  local line
  if nl then
    line = data:sub(pos + 1, nl - 1)
    f.pos = nl
  else
    line = data:sub(pos + 1)
    f.pos = #data
    f.past = true
  end
  return line
end

local function read_chars(f, n)
  local data, pos = f.data, f.pos
  local s = data:sub(pos + 1, pos + n)
  f.pos = pos + #s
  if #s < n then f.past = true end
  return s, (#s == n or #s > 0)
end

-- `test_eof` decides what read(0) reports
local function read_formats(f, test_eof, fname, ...)
  local n = select("#", ...)
  if n == 0 then
    return read_line(f)
  end
  local results = {}
  for i = 1, n do
    local fmt = select(i, ...)
    local ok
    if type(fmt) == "number" then
      local count = math.floor(fmt)
      if count == 0 then
        results[i] = ""
        ok = test_eof(f)
      else
        results[i], ok = read_chars(f, count)
      end
    else
      local p = type(fmt) == "string" and fmt or nil
      if not p or p:sub(1, 1) ~= "*" then
        error(format("bad argument #%d to '%s' (invalid option)", i + 1, fname), 3)
      end
      local c = p:sub(2, 2)
      if c == "l" then
        results[i] = read_line(f)
        ok = results[i] ~= nil
      elseif c == "a" then
        results[i] = read_chars(f, #f.data)
        ok = true
      else
        error(format("bad argument #%d to '%s' (invalid format)", i + 1, fname), 3)
      end
    end
    if not ok then
      results[i] = nil
      return table.unpack(results, 1, i)
    end
  end
  return table.unpack(results, 1, n)
end

local function reader_class(name, test_eof, whences)
  local class = {}
  class.__index = class
  local function live(f, fname)
    if getmetatable(f) ~= class then
      error(format("bad argument #1 to '%s' (%s expected, got %s)", fname, name, argname(f)), 3)
    end
    if f.closed then error("attempt to use a closed zip file", 3) end
  end
  function class.read(f, ...)
    live(f, "read")
    return read_formats(f, test_eof, "read", ...)
  end
  function class.lines(f)
    live(f, "lines")
    return function()
      if f.closed then error("file is already closed", 2) end
      return read_line(f)
    end
  end
  function class.seek(f, whence, offset)
    live(f, "seek")
    whence = whence == nil and "cur" or whence
    local base = whences[whence]
    if not base then
      error(format("bad argument #2 to 'seek' (invalid option '%s')", tostring(whence)), 2)
    end
    offset = optint(offset, 0, 3, "seek")
    local target = base(f) + offset
    if target < 0 or target > #f.data then return nil, "Invalid argument", 22 end
    f.pos = target
    return f.pos
  end
  function class.close(f)
    live(f, "close")
    f.closed = true
    f.data = nil
    return true
  end
  return class
end

----------------------------------------------------------------------- gzip

local gzfile = reader_class("zlib.gzFile", function(f) return not f.past end, {
  set = function() return 0 end,
  cur = function(f) return f.pos end,
})
gzfile.__index = gzfile

local gzip = {}

local function gz_failure(path, errno, message)
  return nil, format("%s: %s", path, message or "Success"), errno or 0
end

local function errno_message(message)
  -- io.open reports "name: strerror"
  return (message:gsub("^.*: ", ""))
end

function gzip.open(path, mode)
  path = checkstring(path, 1, "open")
  mode = mode == nil and "rb" or checkstring(mode, 2, "open")
  local kind, level, direct
  for c in mode:gmatch(".") do
    if c == "r" or c == "w" or c == "a" then
      kind = kind or c
    elseif c:match("%d") then
      level = tonumber(c)
    elseif c == "T" then
      direct = true
    elseif c == "+" then
      return gz_failure(path)
    end
  end
  if not kind then return gz_failure(path) end
  local f = setmetatable({ path = path, kind = kind, pos = 0 }, gzfile)
  if kind == "r" then
    local handle, message, errno = raw_open(path, "rb")
    if not handle then
      return nil, message, errno
    end
    local data = handle:read("a")
    handle:close()
    if data:sub(1, 2) == "\31\139" then
      -- one or more gzip members
      local out, rest = {}, data
      while #rest >= 2 and rest:sub(1, 2) == "\31\139" do
        local piece, code, consumed = S.gzip_member(rest)
        if code ~= 1 then break end
        out[#out + 1] = piece
        rest = rest:sub(consumed + 1)
      end
      data = table.concat(out)
    end
    f.data = data
  else
    local handle, message, errno = raw_open(path, kind == "a" and "ab" or "wb")
    if not handle then return nil, message, errno end
    f.handle, f.level, f.direct, f.buffer = handle, level or -1, direct, {}
  end
  return f
end

function gzip.close(f) return f:close() end

local function gz_live(f, fname)
  if getmetatable(f) ~= gzfile then
    error(format("bad argument #1 to '%s' (zlib.gzFile expected, got %s)", fname, argname(f)), 3)
  end
  if f.closed then error("attempt to use a closed file", 3) end
end

local function gz_flush(f)
  if f.kind == "r" then return true end
  local text = table.concat(f.buffer)
  f.buffer = {}
  if text == "" and not f.wrote then
    -- an empty gzip member is still written for an empty file
    f.wrote = true
  elseif text == "" then
    return true
  end
  f.wrote = true
  local out = text
  if not f.direct then out = zlib.compress(text, f.level, 8, 31) end
  local ok, message = f.handle:write(out)
  if not ok then return nil, message end
  return f.handle:flush() and true
end

function gzfile.close(f)
  gz_live(f, "close")
  local ok, message, errno = true, nil, nil
  if f.kind ~= "r" then
    ok, message = gz_flush(f)
    f.handle:close()
  end
  f.closed = true
  f.data, f.buffer = nil, nil
  if ok then return true end
  return nil, message, 0
end

function gzfile.flush(f)
  gz_live(f, "flush")
  local ok, message = gz_flush(f)
  if ok then return true end
  return nil, message, 0
end

function gzfile.write(f, ...)
  gz_live(f, "write")
  if f.kind == "r" then return nil, "Success", 0 end
  for i = 1, select("#", ...) do
    local v = select(i, ...)
    if type(v) == "number" then
      v = format(math.type(v) == "integer" and "%d" or "%.14g", v)
    else
      v = checkstring(v, i + 1, "write")
    end
    f.buffer[#f.buffer + 1] = v
  end
  return true
end

function gzfile.read(f, ...)
  gz_live(f, "read")
  if f.kind ~= "r" then
    if select("#", ...) > 0 then
      local first = ...
      if first == "*a" then return "" end
    end
    return nil
  end
  return read_formats(f, function(x) return not x.past end, "read", ...)
end

function gzfile.lines(f)
  gz_live(f, "lines")
  return function()
    if f.closed then error("file is already closed", 2) end
    if f.kind ~= "r" then return end
    return read_line(f)
  end
end

function gzfile.seek(f, whence, offset)
  gz_live(f, "seek")
  whence = whence == nil and "cur" or whence
  if whence ~= "set" and whence ~= "cur" then
    error(format("bad argument #2 to 'seek' (invalid option '%s')", tostring(whence)), 2)
  end
  if f.kind ~= "r" then return nil, "Success", 0 end
  offset = optint(offset, 0, 3, "seek")
  local target = (whence == "set" and 0 or f.pos) + offset
  if target < 0 or target > #f.data then return nil, "Success", 0 end
  f.pos = target
  f.past = false
  return f.pos
end

function gzfile.__tostring(f)
  if f.closed then return "gzip file (closed)" end
  return format("gzip file (%s)", pointer(f))
end

function gzfile.__gc(f)
  if not f.closed and f.kind and f.kind ~= "r" then pcall(gzfile.close, f) end
end

function gzip.lines(path)
  path = checkstring(path, 1, "lines")
  local f, message = gzip.open(path, "rb")
  if not f then
    error(format("bad argument #1 to 'lines' (%s)", errno_message(message)), 2)
  end
  return function()
    if f.closed then error("file is already closed", 2) end
    local line = read_line(f)
    if line == nil then
      f:close()
      return
    end
    return line
  end
end

------------------------------------------------------------------------- zip

local zipfile = {}
zipfile.__index = zipfile

local internal = reader_class("zip file", function() return true end, {
  set = function() return 0 end,
  cur = function(f) return f.pos end,
  ["end"] = function(f) return #f.data end,
})
internal.__tostring = function(f)
  if f.closed then return "file in zip file (closed)" end
  return format("file in zip file (%s)", pointer(f))
end
internal.__gc = function(f) if not f.closed then f.closed = true f.data = nil end end

local zip = {
  _COPYRIGHT = "Copyright (C) 2003-2006 Kepler Project",
  _DESCRIPTION = "Reading files inside zip files",
  _VERSION = "LuaZip 1.2.2",
}

local function zip_live(z, fname)
  if getmetatable(z) ~= zipfile then
    error(format("bad argument #1 to '%s' (zip file expected, got %s)", fname, argname(z)), 3)
  end
  if z.id == nil then error("attempt to use a closed zip file", 3) end
  return z.id
end

function zip.open(path)
  path = checkstring(path, 1, "open")
  local id = S.zip_open(path)
  if id == nil then return nil, format("could not open file `%s'", path) end
  return setmetatable({ id = id }, zipfile)
end

function zip.close(z)
  local id = zip_live(z, "close")
  S.zip_close(id)
  z.id = nil
  return true
end
zipfile.close = zip.close

function zip.type(z)
  if getmetatable(z) ~= zipfile then
    error(format("bad argument #1 to 'type' (zip file expected, got %s)", argname(z)), 2)
  end
  return z.id and "zip file" or "closed zip file"
end

local function internal_file(data)
  return setmetatable({ data = data, pos = 0 }, internal)
end

function zipfile.open(z, name)
  local id = zip_live(z, "open")
  name = checkstring(name, 2, "open")
  local data = S.zip_read(id, name)
  if data == nil then return nil, format("could not open file `%s'", name) end
  return internal_file(data)
end

function zipfile.files(z)
  local id = zip_live(z, "files")
  local names = { S.zip_names(id) }
  local info = { S.zip_info(id) }
  local i = 0
  return function()
    if z.id == nil then error("file is already closed", 2) end
    i = i + 1
    local name = names[i]
    if name == nil then return end
    return {
      compressed_size = info[3 * i - 2],
      compression_method = info[3 * i - 1],
      uncompressed_size = info[3 * i],
      filename = name,
    }
  end
end

function zipfile.__tostring(z)
  if z.id == nil then return "zip file (closed)" end
  return format("zip file (%s)", pointer(z))
end

function zipfile.__gc(z)
  if z.id then S.zip_close(z.id) z.id = nil end
end

-- zip.openfile("archive.zip/inner/name" or an ordinary file [, extensions])
function zip.openfile(path, extensions)
  path = checkstring(path, 1, "openfile")
  local exts = { "zip", "ZIP" }
  if type(extensions) == "string" then
    exts = { extensions }
  elseif type(extensions) == "table" then
    exts = {}
    for _, e in ipairs(extensions) do
      if type(e) == "string" or type(e) == "number" then exts[#exts + 1] = tostring(e) end
    end
  end
  local plain = raw_open(path, "rb")
  if plain then
    local data = plain:read("a")
    plain:close()
    return internal_file(data)
  end
  for cut = #path, 1, -1 do
    if path:sub(cut, cut) == "/" then
      local archive, inner = path:sub(1, cut - 1), path:sub(cut + 1)
      local candidates = { archive }
      for _, e in ipairs(exts) do candidates[#candidates + 1] = archive .. "." .. e end
      for _, candidate in ipairs(candidates) do
        local id = S.zip_open(candidate)
        if id then
          local data = S.zip_read(id, inner)
          S.zip_close(id)
          if data then return internal_file(data) end
        end
      end
    end
  end
  return nil, format("could not open file `%s'", path)
end

_G.zlib, _G.gzip, _G.zip = zlib, gzip, zip
package.loaded.zlib, package.loaded.gzip, package.loaded.zip = zlib, gzip, zip
