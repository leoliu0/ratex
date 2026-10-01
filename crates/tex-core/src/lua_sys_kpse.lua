-- kpse: the Kpathsea library as exported by LuaTeX (lkpselib.c).
local S = __ratex_sys
local type, tostring, error, select, setmetatable, getmetatable, tointeger =
  type, tostring, error, select, setmetatable, getmetatable, math.tointeger
local format = string.format

local names = { S.kpse_format_names() }
local index_of = {}
for i, name in ipairs(names) do index_of[name] = i - 1 end

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

local function checkinteger(v, n, fname)
  local i = tointeger(v)
  if i == nil and type(v) == "string" then i = tointeger(tonumber(v)) end
  if i == nil then
    error(format("bad argument #%d to '%s' (number expected, got %s)", n, fname, argname(v)), 3)
  end
  return i
end

-- luaL_checkoption over the file type names
local function checkoption(v, n, fname)
  local i = index_of[v]
  if type(v) ~= "string" or i == nil then
    error(format("bad argument #%d to '%s' (invalid option '%s')", n, fname, tostring(v)), 3)
  end
  return i
end

local default_program = "luahbtex"
local function program_of(self)
  if self == nil then return S.kpse_program() end
  return self.program
end

-- functions shared by the module table and by `kpse.new` objects; `self` is
-- nil for the module table. `first` is the index of the first real argument.
local methods = {}

local function find_file(self, fname, ...)
  local first = self == nil and 1 or 2
  local name = checkstring((...), first, fname)
  local nargs = select("#", ...)
  local ftype, must_exist = index_of.tex, false
  for i = nargs, 2, -1 do
    local v = select(i, ...)
    local t = type(v)
    if t == "boolean" then
      must_exist = v
    elseif t == "number" then
      must_exist = tointeger(v) or math.floor(v)
    elseif t == "string" then
      ftype = checkoption(v, i + first - 1, fname)
    end
  end
  local path = S.kpse_find(name, ftype)
  if path == "" then return nil end
  return path
end

function methods.find_file(self, ...)
  return find_file(self, "find_file", ...)
end

local function lookup(self, fname, ...)
  local first = self == nil and 1 or 2
  local name, opts = ...
  name = checkstring(name, first, fname)
  local format_index, path, all, must_exist = -1, nil, false, false
  local subdirs = {}
  if type(opts) == "table" then
    local fmt = opts.format
    if type(fmt) == "string" then
      local i = index_of[fmt]
      if i == nil then
        error(format("bad argument #-1 to '%s' (invalid option '%s')", fname, fmt), 3)
      end
      format_index = i
    end
    if type(opts.path) == "string" then path = opts.path end
    if type(opts.all) == "boolean" then all = opts.all end
    if type(opts.mustexist) == "boolean" then must_exist = opts.mustexist end
    local subdir = opts.subdir
    if type(subdir) == "table" then
      for _, v in pairs(subdir) do
        if type(v) == "string" then subdirs[#subdirs + 1] = v end
      end
    elseif type(subdir) == "string" then
      subdirs[1] = subdir
    end
    if #subdirs > 0 then all = true end
  end
  local results = { S.kpse_lookup(program_of(self), name, format_index, path, all, must_exist, subdirs) }
  if #results == 0 then return nil end
  return table.unpack(results)
end

function methods.lookup(self, ...)
  return lookup(self, "lookup", ...)
end

local function simple(fname, call)
  return function(self, ...)
    local first = self == nil and 1 or 2
    local value = checkstring((...), first, fname)
    return call(self, value)
  end
end

methods.expand_path = simple("expand_path", function(self, s) return S.kpse_expand_path(program_of(self), s) end)
methods.expand_var = simple("expand_var", function(self, s) return S.kpse_expand_var(program_of(self), s) end)
methods.expand_braces = simple("expand_braces", function(_, s) return S.kpse_expand_braces(s) end)
methods.var_value = simple("var_value", function(self, s) return S.kpse_var_value(program_of(self), s) end)
methods.readable_file = simple("readable_file", function(_, s) return S.kpse_readable_file(s) end)
methods.in_name_ok = simple("in_name_ok", function(self, s) return S.kpse_name_ok(program_of(self), s, false, false) end)
methods.in_name_ok_silent_extended = simple("in_name_ok_silent_extended", function(self, s)
  return S.kpse_name_ok(program_of(self), s, false, true)
end)
methods.out_name_ok = simple("out_name_ok", function(self, s) return S.kpse_name_ok(program_of(self), s, true, false) end)
methods.out_name_ok_silent_extended = simple("out_name_ok_silent_extended", function(self, s)
  return S.kpse_name_ok(program_of(self), s, true, true)
end)

function methods.show_path(self, ...)
  local n = select("#", ...)
  local v = n > 0 and (select(n, ...)) or "tex"
  if n > 0 and type(v) ~= "string" then
    error(format("bad argument #-1 to 'show_path' (string expected, got %s)", argname(v)), 2)
  end
  return S.kpse_show_path(program_of(self), checkoption(v, -1, "show_path"))
end

function methods.init_prog(self, ...)
  local first = self == nil and 1 or 2
  local prefix, dpi, mode, fallback = ...
  checkstring(prefix, first, "init_prog")
  dpi = checkinteger(dpi, first + 1, "init_prog")
  mode = checkstring(mode, first + 2, "init_prog")
  if fallback ~= nil then checkstring(fallback, first + 3, "init_prog") end
  S.os_setenv("MAKETEX_BASE_DPI", tostring(dpi))
  S.os_setenv("MAKETEX_MODE", mode)
end

function methods.version()
  return "kpathsea version 6.4.2"
end

function methods.default_texmfcnf()
  return S.kpse_default_texmfcnf()
end

function methods.record_input_file(_, ...)
  local name = ...
  if name == nil then return end
  S.kpse_record(tostring(name), false)
end

function methods.record_output_file(_, ...)
  local name = ...
  if name == nil then return end
  S.kpse_record(tostring(name), true)
end

function methods.check_permission(_, ...)
  return S.kpse_check_permission(checkstring((...), 1, "check_permission"))
end

local kpse = {}
for name, f in pairs(methods) do
  kpse[name] = function(...) return f(nil, ...) end
end
-- the module functions do not take a `self`
kpse.record_input_file = function(name)
  if name == nil then return end
  S.kpse_record(tostring(name), false)
end
kpse.record_output_file = function(name)
  if name == nil then return end
  S.kpse_record(tostring(name), true)
end
kpse.check_permission = function(cmd) return S.kpse_check_permission(checkstring(cmd, 1, "check_permission")) end
kpse.version = methods.version
kpse.default_texmfcnf = methods.default_texmfcnf

function kpse.set_program_name(exe, program)
  exe = checkstring(exe, 1, "kpse.set_program_name")
  if program == nil then program = exe end
  S.kpse_set_program(checkstring(program, 2, "kpse.set_program_name"))
  if type(texconfig) == "table" then rawset(texconfig, "kpse_init", false) end
end

-- kpse.new(name [, program]) creates an independent instance
local instance = { __index = {} }
for name, f in pairs(methods) do
  if name ~= "check_permission" then
    instance.__index[name] = f
  end
end
instance.__index.version = methods.version
instance.__index.default_texmfcnf = methods.default_texmfcnf
instance.__index.record_input_file = methods.record_input_file
instance.__index.record_output_file = methods.record_output_file
instance.__name = "luatex.kpathsea"

function kpse.new(exe, program)
  exe = checkstring(exe, 1, "new")
  if program == nil then program = exe end
  return setmetatable({ program = checkstring(program, 2, "new") }, instance)
end

_G.kpse = kpse
package.loaded.kpse = kpse
