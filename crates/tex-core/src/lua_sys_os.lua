-- LuaTeX's additions to os and io, and the security overlay that
-- luatex-core.lua applies after initialization.
local S = __ratex_sys
local type, tostring, error, select, pairs, next, pcall, rawget, getmetatable =
  type, tostring, error, select, pairs, next, pcall, rawget, getmetatable
local format, gmatch, gsub, find = string.format, string.gmatch, string.gsub, string.find
local unpack = table.unpack

local function argname(v)
  if v == nil then return "no value" end
  return type(v)
end

local function optstring(v, n, fname)
  if v == nil then return nil end
  local t = type(v)
  if t == "string" then return v end
  if t == "number" then return tostring(v) end
  error(format("bad argument #%d to '%s' (string expected, got %s)", n, fname, argname(v)), 3)
end

local os = os
os.type, os.name = S.os_platform()
os.selfdir = S.os_selfdir()
os.selfname = nil
os.gettimeofday = S.os_gettimeofday

local env = {}
do
  local flat = { S.os_environ() }
  for i = 1, #flat, 2 do env[flat[i]] = flat[i + 1] end
end
os.env = env

function os.sleep(interval, units)
  if type(interval) ~= "number" then
    error(format("bad argument #1 to 'sleep' (number expected, got %s)", argname(interval)), 2)
  end
  S.os_sleep(interval, units)
end
function os.socketsleep(n)
  if type(n) ~= "number" then
    error(format("bad argument #1 to 'socketsleep' (number expected, got %s)", argname(n)), 2)
  end
  S.os_socketsleep(n)
end
os.socketgettime = S.os_gettimeofday

function os.uname()
  local sysname, machine, release, version, nodename = S.os_uname()
  if sysname == nil then return nil end
  return { sysname = sysname, machine = machine, release = release, version = version, nodename = nodename }
end

function os.times()
  local utime, stime, cutime, cstime = S.os_times()
  return { utime = utime, stime = stime, cutime = cutime, cstime = cstime }
end

function os.tmpdir(template)
  template = optstring(template, 1, "tmpdir") or "luatex.XXXXXX"
  if #template < 6 or template:sub(-6) ~= "XXXXXX" then
    return nil, "Invalid argument to os.tmpdir()"
  end
  local dir, message = S.os_tmpdir(template)
  if dir == nil then return nil, message end
  return dir
end

local setenv_raw = S.os_setenv
function os.setenv(key, value)
  key, value = optstring(key, 1, "setenv"), optstring(value, 2, "setenv")
  if key then
    local ok, message = pcall(setenv_raw, key, value)
    if not ok then error("unable to change environment", 2) end
  end
  return true
end

function os.execute(cmd)
  if cmd ~= nil then cmd = optstring(cmd, 1, "execute") end
  local status, message = S.os_execute(cmd)
  if status == nil then return nil, message end
  return status
end

-- command lines for exec/spawn: a string is split at spaces honouring
-- quotes, a table lists the arguments (field 0 names the program)
local function split(cmd)
  local args, piece, in_string, quoted = {}, {}, nil, false
  local i, n = 1, #cmd
  while i <= n and cmd:sub(i, i) == " " do i = i + 1 end
  while i <= n + 1 do
    local c = i <= n and cmd:sub(i, i) or ""
    local nxt = cmd:sub(i + 1, i + 1)
    if c == "\\" and (nxt == "\\" or nxt == "'" or nxt == '"') then
      quoted = true
    elseif in_string and c == in_string and not quoted then
      in_string = nil
    elseif (c == '"' or c == "'") and not quoted then
      in_string = c
    elseif (not in_string and c == " ") or c == "" then
      args[#args + 1] = table.concat(piece)
      piece = {}
      while i < n and cmd:sub(i + 1, i + 1) == " " do i = i + 1 end
    else
      piece[#piece + 1] = c
      quoted = false
    end
    i = i + 1
  end
  return args
end

local function command_line(cmd)
  if type(cmd) == "string" then
    if cmd == "" then return nil end
    local args = split(cmd)
    return args, args[1]
  elseif type(cmd) == "table" then
    local args = {}
    for j = 1, math.huge do
      local v = rawget(cmd, j)
      if v == nil then break end
      v = optstring(v, 1, "exec")
      if v == nil then break end
      args[j] = v
    end
    if #args == 0 then return nil end
    local run = rawget(cmd, 0)
    return args, run ~= nil and tostring(run) or args[1]
  end
end

function os.exec(...)
  if select("#", ...) ~= 1 then return nil, "invalid arguments passed" end
  local args, run = command_line((...))
  local message, errno = S.os_exec(args or {}, run)
  if errno then return nil, message, errno end
  return nil, message
end

function os.spawn(...)
  local top = select("#", ...)
  if top ~= 1 and top ~= 2 then return nil, "invalid arguments passed" end
  local cmd, environment = ...
  local args, run = command_line(cmd)
  local pairs_list
  if top == 2 and type(environment) == "table" then
    pairs_list = {}
    for k, v in pairs(environment) do
      if type(k) == "string" and type(v) == "string" then
        pairs_list[#pairs_list + 1] = k
        pairs_list[#pairs_list + 1] = v
      end
    end
  end
  local status, message, code = S.os_spawn(args or {}, run, pairs_list)
  if status == nil then return nil, message, code end
  return status
end

local io_popen = io.popen
function os.kpsepopen(command, mode)
  command = optstring(command, 1, "kpsepopen")
  if command == nil then
    error("bad argument #1 to 'kpsepopen' (string expected, got no value)", 2)
  end
  mode = optstring(mode, 2, "kpsepopen") or "r"
  local ok, found = kpse.check_permission(command)
  if not (ok and found) then return nil, found end
  if mode ~= "r" and mode ~= "w" then
    error("bad argument #2 to 'kpsepopen' (invalid mode)", 2)
  end
  return io_popen(found, mode)
end

-- ----------------------------------------------------------------------
-- the overlay of luatex-core.lua

local safer = status.safer_option
local shell = status.shell_escape -- 0 disabled, 1 anything, 2 restricted

function lfs.mkdirp(path)
  local full = ""
  local r1, r2, r3
  for sub in gmatch(path, "(/*[^\\/]+)") do
    full = full .. sub
    r1, r2, r3 = lfs.mkdir(full)
  end
  return r1, r2, r3
end

if shell ~= 1 then
  local mt = getmetatable(io.stderr)
  local kpse_in = kpse.in_name_ok_silent_extended
  local kpse_out = kpse.out_name_ok_silent_extended
  local record_in, record_out = kpse.record_input_file, kpse.record_output_file
  local io_open, io_lines = io.open, io.lines
  local os_rename, os_remove = os.rename, os.remove
  local lfs_attributes, lfs_chdir, lfs_lock_dir, lfs_dir = lfs.attributes, lfs.chdir, lfs.lock_dir, lfs.dir
  local lfs_link, lfs_mkdir, lfs_mkdirp, lfs_rmdir = lfs.link, lfs.mkdir, lfs.mkdirp, lfs.rmdir
  local lfs_symlinkattributes, lfs_touch = lfs.symlinkattributes, lfs.touch
  local EPERM, EPERM_MSG = -1, "LuaTeX: operation not permitted"
  io.saved_lines = io_lines
  mt.saved_lines = mt.lines

  function io.open(name, how)
    if not how then how = "r" end
    local check
    if how == "r" or how == "rb" or how == "" then
      check = kpse_in(name)
    else
      check = kpse_out(name)
    end
    local f
    if check then
      f = io_open(name, how)
      if f then
        if type(how) == "string" and find(how, "w") then
          record_out(name, "w")
        else
          record_in(name, "r")
        end
      end
    end
    return f
  end

  function io.lines(name, how)
    if type(name) == "string" then
      local f = kpse_in(name) and io_open(name, how or "r")
      if f then
        return function()
          local l = fio.readline(f)
          if not l then f:close() end
          return l
        end
      end
      error("patched 'io.lines' can't open '" .. name .. "'")
    end
    return io_lines()
  end

  io.popen = os.kpsepopen

  function os.rename(old, new)
    if kpse_in(old) and kpse_out(new) then return os_rename(old, new) end
    return nil, EPERM_MSG, EPERM
  end
  function os.remove(name)
    if kpse_in(name) and kpse_out(name) then return os_remove(name) end
    return nil, EPERM_MSG, EPERM
  end
  function lfs.attributes(path, opt)
    if kpse_in(path) then return lfs_attributes(path, opt) end
    return nil, EPERM_MSG, EPERM
  end
  function lfs.chdir(name)
    if kpse_in(name) and kpse_out(name) then return lfs_chdir(name) end
    return nil, EPERM_MSG
  end
  function lfs.lock_dir(name, stale)
    if kpse_in(name) and kpse_out(name) then return lfs_lock_dir(name, stale) end
    return nil, EPERM_MSG
  end
  function lfs.dir(name)
    if kpse_in(name) then return lfs_dir(name) end
    error(EPERM_MSG)
  end
  function lfs.link(old, new, symbolic)
    if kpse_in(new) and kpse_out(new) and kpse_in(old) and kpse_out(old) then
      return lfs_link(old, new, symbolic)
    end
    return nil, EPERM_MSG, EPERM
  end
  function lfs.mkdir(name)
    if kpse_in(name) and kpse_out(name) then return lfs_mkdir(name) end
    return nil, EPERM_MSG, EPERM
  end
  function lfs.mkdirp(name)
    if kpse_in(name) and kpse_out(name) then
      local full = ""
      local r1, r2, r3
      for sub in gmatch(name, "(/*[^\\/]+)") do
        full = full .. sub
        r1, r2, r3 = lfs_mkdir(full)
      end
      return r1, r2, r3
    end
    return nil, EPERM_MSG, EPERM
  end
  function lfs.rmdir(name)
    if kpse_in(name) and kpse_out(name) then return lfs_rmdir(name) end
    return nil, EPERM_MSG, EPERM
  end
  function lfs.symlinkattributes(path, name)
    if kpse_in(path) then return lfs_symlinkattributes(path, name) end
    return nil, EPERM_MSG, EPERM
  end
  function lfs.touch(name, atime, mtime)
    if kpse_in(name) and kpse_out(name) then return lfs_touch(name, atime, mtime) end
    return nil, EPERM_MSG
  end

  -- with a restricted shell the environment cannot be changed
  os.setenv = function() end
end

if safer then
  if safer ~= 0 then
    local function dummy(str, f)
      local reported = false
      return function(...)
        if not reported then
          texio.write_nl(format("safer option set, function %q is %s", str, f and "limited" or "disabled"))
          reported = true
        end
        if f then return f(...) end
      end
    end
    os.execute, os.spawn, os.exec, os.setenv = dummy("os.execute"), dummy("os.spawn"), dummy("os.exec"), dummy("os.setenv")
    os.tmpdir, os.kpsepopen = dummy("os.tmpdir"), dummy("os.kpsepopen")
    io.popen, io.open, io.tmpfile, io.output = dummy("io.popen"), dummy("io.open"), dummy("io.tmpfile"), dummy("io.output")
    os.rename, os.remove = dummy("os.rename"), dummy("os.remove")
    lfs.chdir, lfs.lock, lfs.touch = dummy("lfs.chdir"), dummy("lfs.lock"), dummy("lfs.touch")
    lfs.rmdir, lfs.mkdir, lfs.mkdirp = dummy("lfs.rmdir"), dummy("lfs.mkdir"), dummy("lfs.mkdirp")
  end
end

if safer ~= 0 or shell ~= 1 then
  package.loadlib = function() end
end

-- compatibility names luatex-core.lua provides
local sum = md5.sum
if not md5.sumhexa then
  function md5.sumhexa(k)
    return (gsub(sum(k), ".", function(c) return format("%02x", c:byte()) end))
  end
end
if not md5.sumHEXA then
  function md5.sumHEXA(k)
    return (gsub(sum(k), ".", function(c) return format("%02X", c:byte()) end))
  end
end
if not _G.unpack then _G.unpack = table.unpack end
if not package.loaders then package.loaders = package.searchers end
if not _G.loadstring then _G.loadstring = load end
if not package.loaded.mime then package.loaded.mime = package.loaded["mime.core"] end
package.loaded.lfs = lfs

do
  local attributes, symlinkattributes = lfs.attributes, lfs.symlinkattributes
  if not lfs.isfile then
    function lfs.isfile(name)
      local m = attributes(name, "mode")
      return m == "file" or m == "link"
    end
  end
  if not lfs.isdir then
    function lfs.isdir(name)
      return attributes(name, "mode") == "directory"
    end
  end
  if not lfs.shortname then
    function lfs.shortname(name) return name end
  end
  if not lfs.readlink then
    function lfs.readlink(name)
      return symlinkattributes(name, "target") or nil
    end
  end
end
