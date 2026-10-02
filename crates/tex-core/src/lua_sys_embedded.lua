-- Reading files of the embedded archive through the standard `io` and
-- `loadfile` functions. Paths below `/<embedded>/` (what `kpse.find_file`
-- reports for bundled files) denote a read-only virtual TDS tree; the
-- handles returned for them behave like read-only `io.open` handles of
-- an ordinary file (so `fio`, `lfs.lock`, ... accept them).
local S = __ratex_sys
local type, tostring, error, setmetatable, getmetatable, select, load =
  type, tostring, error, setmetatable, getmetatable, select, load
local sub, find, match, byte = string.sub, string.find, string.match, string.byte
local tointeger = math.tointeger

local is_path, read_file = S.embedded_is_path, S.embedded_read
local real_open, real_lines, real_type, real_close = io.open, io.lines, io.type, io.close
local real_loadfile, real_dofile = loadfile, dofile

local ENOENT, EBADF, EISDIR, EROFS = 2, 9, 21, 30
local function failure(message, errno) return nil, message, errno end

local vfile = { __name = "FILE*" }
local methods = {}
vfile.__index = methods

local function check(f, name)
  if getmetatable(f) ~= vfile then
    error("bad argument #1 to '" .. name .. "' (FILE* expected, got " .. type(f) .. ")", 3)
  end
  if f.closed then error("attempt to use a closed file", 3) end
  return f
end

local handles = 0
vfile.__tostring = function(f)
  if f.closed then return "file (closed)" end
  if not f.id then
    handles = handles + 1
    f.id = handles
  end
  return string.format("file (0x%08x)", f.id)
end

-- the Lua `read` formats over the in-memory contents
local function read_number(f)
  local data, pos = f.data, f.pos + 1
  local _, after = find(data, "^%s*", pos)
  pos = after + 1
  local text = match(data, "^[+-]?0[xX]%x*%.?%x*[pP][+-]?%d+", pos)
    or match(data, "^[+-]?0[xX]%x*%.?%x*", pos)
    or match(data, "^[+-]?%d*%.?%d*[eE][+-]?%d+", pos)
    or match(data, "^[+-]?%d*%.?%d*", pos)
  f.pos = pos - 1 + #(text or "")
  return text and tonumber(text) or nil
end

local function read_one(f, fmt)
  local data, pos = f.data, f.pos
  if type(fmt) == "number" then
    local n = tointeger(fmt) or error("bad argument to 'read' (number has no integer representation)", 3)
    if pos >= #data then return nil end
    f.pos = pos + n
    if f.pos > #data then f.pos = #data end
    return sub(data, pos + 1, pos + n)
  end
  if type(fmt) ~= "string" then error("bad argument to 'read' (invalid format)", 3) end
  local what = match(fmt, "^%*?(.)")
  if what == "n" then return read_number(f) end
  if what == "a" then
    f.pos = #data
    return sub(data, pos + 1)
  end
  if what == "l" or what == "L" then
    if pos >= #data then return nil end
    local stop = find(data, "\n", pos + 1, true)
    if stop == nil then
      f.pos = #data
      return sub(data, pos + 1)
    end
    f.pos = stop
    return sub(data, pos + 1, what == "L" and stop or stop - 1)
  end
  error("bad argument to 'read' (invalid format)", 3)
end

function methods.read(f, ...)
  check(f, "read")
  if f.directory then return failure("Is a directory", EISDIR) end
  local n = select("#", ...)
  if n == 0 then return read_one(f, "l") end
  local results = {}
  for i = 1, n do
    local value = read_one(f, (select(i, ...)))
    results[i] = value
    if value == nil then return table.unpack(results, 1, i) end
  end
  return table.unpack(results, 1, n)
end

function methods.lines(f, ...)
  check(f, "lines")
  local formats = table.pack(...)
  return function()
    if f.closed then error("file is already closed", 2) end
    return methods.read(f, table.unpack(formats, 1, formats.n))
  end
end

function methods.seek(f, whence, offset)
  check(f, "seek")
  whence = whence or "cur"
  offset = offset or 0
  local base
  if whence == "set" then base = 0
  elseif whence == "cur" then base = f.pos
  elseif whence == "end" then base = #f.data
  else error("bad argument #1 to 'seek' (invalid option '" .. tostring(whence) .. "')", 2) end
  local target = base + (tointeger(offset) or error("bad argument #2 to 'seek' (number has no integer representation)", 2))
  if target < 0 then return failure("Invalid argument", 22) end
  f.pos = target
  return target
end

function methods.close(f)
  check(f, "close")
  f.closed = true
  f.data = nil
  return true
end

function methods.write(f)
  check(f, "write")
  return failure("Bad file descriptor", EBADF)
end

function methods.flush(f)
  check(f, "flush")
  return f
end

function methods.setvbuf(f)
  check(f, "setvbuf")
  return true
end

vfile.__close = function(f) if not f.closed then f.closed = true f.data = nil end end

local function open_virtual(name, mode)
  mode = mode == nil and "r" or mode
  if type(mode) ~= "string" or not match(mode, "^[rwa]%+?b?$") and not match(mode, "^[rwa]b?%+?$") then
    error("bad argument #2 to 'open' (invalid mode)", 3)
  end
  if find(mode, "[wa+]") then return failure(name .. ": Read-only file system", EROFS) end
  local data = read_file(name)
  if data then return setmetatable({ data = data, pos = 0, name = name }, vfile) end
  if lfs.attributes(name, "mode") == "directory" then
    return setmetatable({ data = "", pos = 0, name = name, directory = true }, vfile)
  end
  return failure(name .. ": No such file or directory", ENOENT)
end

function io.open(name, mode)
  if type(name) == "string" and is_path(name) then return open_virtual(name, mode) end
  return real_open(name, mode)
end

function io.type(f)
  if getmetatable(f) == vfile then return f.closed and "closed file" or "file" end
  return real_type(f)
end

function io.close(f)
  if getmetatable(f) == vfile then return methods.close(f) end
  return real_close(f)
end

function io.lines(name, ...)
  if type(name) == "string" and is_path(name) then
    local f, message = open_virtual(name)
    if not f then error(message, 2) end
    local formats = table.pack(...)
    return function()
      if f.closed then error("file is already closed", 2) end
      local results = table.pack(methods.read(f, table.unpack(formats, 1, formats.n)))
      if results[1] == nil then methods.close(f) end
      return table.unpack(results, 1, results.n)
    end
  end
  return real_lines(name, ...)
end

-- loadfile/dofile skip a leading `#` line and a UTF-8 byte order mark
local function chunk_of(name)
  local data = read_file(name)
  if data == nil then return nil end
  if sub(data, 1, 3) == "\239\187\191" then data = sub(data, 4) end
  if byte(data, 1) == 35 then data = (data:gsub("^#[^\n]*", "", 1)) end
  return data
end

function loadfile(name, ...)
  if type(name) == "string" and is_path(name) then
    local data = chunk_of(name)
    if data == nil then return nil, "cannot open " .. name end
    return load(data, "@" .. name, ...)
  end
  return real_loadfile(name, ...)
end

function dofile(name)
  if type(name) == "string" and is_path(name) then
    local f, message = loadfile(name)
    if not f then error(message, 0) end
    return f()
  end
  return real_dofile(name)
end
