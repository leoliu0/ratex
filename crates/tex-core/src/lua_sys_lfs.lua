-- lfs: LuaFileSystem 1.7.0 as exported by LuaTeX 1.24.
local S = __texres_sys
local type, tostring, error, setmetatable, getmetatable, select =
  type, tostring, error, setmetatable, getmetatable, select
local format = string.format

local function checkstring(v, n, fname)
  local t = type(v)
  if t == "string" then return v end
  if t == "number" then return tostring(v) end
  error(format("bad argument #%d to '%s' (string expected, got %s)", n, fname, t == "nil" and "no value" or t), 3)
end

-- nil, message, errno on failure; the value otherwise
local function result(v, message, errno)
  if v == nil then return nil, message, errno end
  return v
end

local lfs = {
  _VERSION = "LuaFileSystem 1.7.0",
  _COPYRIGHT = "Copyright (C) 2003-2017 Kepler Project",
  _DESCRIPTION = "LuaFileSystem is a Lua library developed to complement the set of functions related to file systems offered by the standard Lua distribution",
}

local members = {
  "mode", "dev", "ino", "nlink", "uid", "gid", "rdev", "access", "modification",
  "change", "size", "permissions", "blocks", "blksize",
}
local position = {}
for i, name in ipairs(members) do position[name] = i end

local function build(follow, fname, path, what)
  path = checkstring(path, 1, fname)
  local mode, perms, dev, ino, nlink, uid, gid, rdev, atime, mtime, ctime, size, blocks, blksize =
    S.lfs_stat(path, follow)
  if mode == nil then return nil, perms, dev end
  local all = {
    mode = mode, dev = dev, ino = ino, nlink = nlink, uid = uid, gid = gid, rdev = rdev,
    access = atime, modification = mtime, change = ctime, size = size,
    permissions = perms, blocks = blocks, blksize = blksize,
  }
  local t = type(what)
  if t == "string" or t == "number" then
    what = tostring(what)
    if not position[what] then
      error(format("invalid attribute name '%s'", what), 3)
    end
    return all[what]
  end
  if t ~= "table" then what = {} end
  for _, name in ipairs(members) do what[name] = all[name] end
  return what
end

function lfs.attributes(path, what)
  return build(true, "attributes", path, what)
end

function lfs.symlinkattributes(path, what)
  if type(what) == "string" and what == "target" then
    local target, message, errno = S.lfs_readlink(checkstring(path, 1, "symlinkattributes"))
    if target == nil then return nil, "could not obtain link target: " .. message, errno end
    return target
  end
  local info, message, errno = build(false, "symlinkattributes", path, what)
  if type(info) == "table" and info.mode == "link" then
    local target = S.lfs_readlink(path)
    if target ~= nil then info.target = target end
  end
  return info, message, errno
end

function lfs.currentdir()
  return result(S.lfs_currentdir())
end

function lfs.chdir(path)
  local ok, message = S.lfs_chdir(checkstring(path, 1, "chdir"))
  if ok then return true end
  return nil, message
end

function lfs.mkdir(path)
  return result(S.lfs_mkdir(checkstring(path, 1, "mkdir")))
end

function lfs.rmdir(path)
  return result(S.lfs_rmdir(checkstring(path, 1, "rmdir")))
end

function lfs.link(old, new, symbolic)
  return result(S.lfs_link(checkstring(old, 1, "link"), checkstring(new, 2, "link"), symbolic and true or false))
end

function lfs.touch(path, atime, mtime)
  path = checkstring(path, 1, "touch")
  if select("#", path, atime, mtime) == 1 or (atime == nil and mtime == nil) then
    return result(S.lfs_touch(path, false, 0, 0))
  end
  if atime == nil then atime = 0 end
  if type(atime) ~= "number" then
    error(format("bad argument #2 to 'touch' (number expected, got %s)", type(atime)), 2)
  end
  if mtime == nil then mtime = atime end
  if math.type(mtime) ~= "integer" then
    local m = math.tointeger(mtime)
    if m == nil then
      error(format("bad argument #3 to 'touch' (number has no integer representation)"), 2)
    end
    mtime = m
  end
  return result(S.lfs_touch(path, true, atime, mtime))
end

-- directory iterators
local dirmeta = { __name = "directory metatable" }
local function dir_next(d)
  if type(d) ~= "userdata" or getmetatable(d) ~= dirmeta then
    error(format("bad argument #1 to 'it' (directory metatable expected, got %s)", type(d) == "nil" and "nil" or type(d)), 2)
  end
  local name = S.lfs_dir_next(d.id)
  if name == nil then return end
  return name
end
local function dir_close(d)
  if type(d) == "userdata" and getmetatable(d) == dirmeta then S.lfs_dir_close(d.id) end
end
dirmeta.__index = { next = dir_next, close = dir_close }
dirmeta.__gc = dir_close

function lfs.dir(path)
  path = checkstring(path, 1, "dir")
  local d = S.ud_new(S.lfs_dir_open(path), dirmeta)
  return dir_next, d
end

-- directory locks
local lockmeta = { __name = "lock metatable" }
local function unlock_dir(lock)
  local name = lock[1]
  if name then
    S.lfs_unlock_dir(name)
    lock[1] = nil
  end
end
lockmeta.__index = { free = unlock_dir }
lockmeta.__gc = unlock_dir

function lfs.lock_dir(path)
  local name, message = S.lfs_lock_dir(checkstring(path, 1, "lock_dir"))
  if name == nil then return nil, message end
  return setmetatable({ name }, lockmeta)
end

local function check_file(f, n, fname)
  local kind = io.type(f)
  if kind == nil then
    error(format("bad argument #%d to '%s' (FILE* expected, got %s)", n, fname, type(f) == "nil" and "no value" or type(f)), 3)
  elseif kind == "closed file" then
    error(format("bad argument #%d to '%s' (closed file)", n, fname), 3)
  end
  return f
end

-- the file mode is not distinguished on Unix: reports the previous mode
function lfs.setmode(f, mode)
  check_file(f, 1, "setmode")
  if mode ~= "binary" and mode ~= "text" then
    error(format("bad argument #2 to 'setmode' (invalid option '%s')", tostring(mode)), 2)
  end
  return true, "binary"
end

local function lock_mode(mode, fname)
  if type(mode) ~= "string" then
    error(format("bad argument #2 to '%s' (string expected, got %s)", fname, type(mode) == "nil" and "no value" or type(mode)), 3)
  end
  local c = mode:sub(1, 1)
  if c ~= "w" and c ~= "r" and c ~= "u" then error(fname .. ": invalid mode", 3) end
  return c
end

function lfs.lock(f, mode, start, length)
  check_file(f, 1, "lock")
  mode = lock_mode(mode, "lock")
  local ok, message = S.lfs_flock(f, mode, start or 0, length or 0)
  if ok then return true end
  return nil, message
end

function lfs.unlock(f, start, length)
  check_file(f, 1, "unlock")
  local ok, message = S.lfs_flock(f, "u", start or 0, length or 0)
  if ok then return true end
  return nil, message
end

package.loaded.lfs = lfs
_G.lfs = lfs
