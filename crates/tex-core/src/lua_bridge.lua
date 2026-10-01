-- LuaTeX library surface built on the engine bridge (crates/tex-core/src/lua_bridge.rs).
local B = __ratex_bridge
__ratex_bridge = nil

local type, select, rawget, setmetatable, getmetatable, tostring, load =
      type, select, rawget, setmetatable, getmetatable, tostring, load
local tex, lua = tex, lua

-- ---------------------------------------------------------------- token ---

token = token or {}
local token = token
local tok_mt = {}
local function wrap(packed) return setmetatable({ packed }, tok_mt) end
local function packed(t)
  if getmetatable(t) ~= tok_mt then
    error("lua <token> expected, not an object with type " .. type(t), 3)
  end
  return rawget(t, 1)
end
local fields = {
  command    = B.tok_cmd,
  mode       = B.tok_mode,
  index      = B.tok_index,
  cmdname    = B.tok_cmdname,
  csname     = B.tok_csname,
  active     = B.tok_active,
  expandable = B.tok_expandable,
  protected  = B.tok_protected,
  tok        = B.tok_tok,
  id         = function(p) return p end,
}
tok_mt.__index = function(t, key)
  local f = fields[key]
  if f then return f(rawget(t, 1)) end
  return nil
end
tok_mt.__eq = function(a, b) return rawget(a, 1) == rawget(b, 1) end
tok_mt.__tostring = function(t)
  return "<lua token " .. rawget(t, 1) .. ": " .. B.tok_tok(rawget(t, 1)) .. ">"
end
token.__metatable_of_tokens = tok_mt

function token.type(t) if getmetatable(t) == tok_mt then return "token" end return nil end
function token.is_token(t) return getmetatable(t) == tok_mt end
function token.create(v, cmd)
  if type(v) == "number" then return wrap(B.create_char(v, cmd)) end
  return wrap(B.create_cs(tostring(v)))
end
function token.new(chr, cmd) return wrap(B.new(chr, cmd)) end
function token.is_defined(name, exists)
  if type(name) ~= "string" then return false end
  return B.is_defined(name, exists and true or false)
end

local command_names = __ratex_command_names
__ratex_command_names = nil
local command_ids = {}
for i = 0, #command_names do command_ids[command_names[i]] = i end
function token.commands()
  local t = {}
  for i = 0, #command_names do t[i] = command_names[i] end
  return t
end
function token.command_id(name)
  if type(name) ~= "string" then return nil end
  return command_ids[name]
end
function token.biggest_char() return 0x10FFFF end

function token.get_next() return wrap(B.get_next()) end
function token.scan_token() return wrap(B.scan_token()) end
function token.scan_keyword(kw) return B.scan_keyword(kw, false) end
function token.scan_keyword_cs(kw) return B.scan_keyword(kw, true) end
token.scan_int = B.scan_int
function token.scan_dimen(inf, mu) return B.scan_dimen(mu and true or false) end
function token.scan_float() return B.scan_float(true) end
function token.scan_real() return B.scan_float(false) end
function token.scan_toks(macro_def, expand)
  if macro_def then error("token.scan_toks: macro parameter text is not supported") end
  local list = B.scan_toks(expand and true or false)
  for i = 1, #list do list[i] = wrap(list[i]) end
  return list
end
token.scan_string = B.scan_string
function token.scan_argument(expand)
  if type(expand) == "boolean" then return B.scan_argument(expand) end
  return B.scan_argument(true)
end
token.scan_word = B.scan_word
token.scan_csname = B.scan_csname
function token.scan_code(mask) return B.scan_code(mask or (2048 + 4096)) end

function token.put_next(...)
  local n = select("#", ...)
  if n == 0 then return end
  local first = ...
  local list = {}
  if type(first) == "table" and getmetatable(first) ~= tok_mt then
    if n > 1 then error("only one table permitted in put_next") end
    for i = 1, #first do list[i] = packed(first[i]) end
  else
    for i = 1, n do list[i] = packed((select(i, ...))) end
  end
  B.put_next(list)
end
token.unchecked_put_next = token.put_next
token.expand = B.expand

function token.get_command(t) return B.tok_cmd(packed(t)) end
function token.get_index(t) return B.tok_index(packed(t)) end
function token.get_mode(t) return B.tok_mode(packed(t)) end
function token.get_cmdname(t) return B.tok_cmdname(packed(t)) end
function token.get_csname(t) return B.tok_csname(packed(t)) end
function token.get_id(t) return packed(t) end
function token.get_tok(t) return B.tok_tok(packed(t)) end
function token.get_active(t) return B.tok_active(packed(t)) end
function token.get_expandable(t) return B.tok_expandable(packed(t)) end
function token.get_protected(t) return B.tok_protected(packed(t)) end
function token.get_macro(name)
  if type(name) ~= "string" then return end
  local s = B.get_macro(name, false)
  if s ~= nil then return s end
end
function token.get_meaning(name)
  if type(name) ~= "string" then return end
  local s = B.get_macro(name, true)
  if s ~= nil then return s end
end
function token.set_macro(...)
  local n = select("#", ...)
  local a1, a2, a3, a4 = ...
  local ct, name, str, scope
  if type(a1) == "number" then
    if n == 1 then return end
    ct, name, str, scope = a1, a2, a3, a4
  else
    name, str, scope = a1, a2, a3
  end
  if name == nil then return end
  B.set_macro(ct, tostring(name), str ~= nil and tostring(str) or nil, scope == "global")
end
function token.set_char(name, value, scope)
  if name == nil or value == nil then return end
  B.set_char(tostring(name), value, scope == "global")
end
function token.set_lua(name, id, ...)
  if name == nil or id == nil then return end
  local protected, global = false, false
  for i = 1, math.min(select("#", ...), 2) do
    local s = select(i, ...)
    if s == "global" then global = true elseif s == "protected" then protected = true end
  end
  B.set_lua(tostring(name), id, protected, global)
end

-- ----------------------------------------------------------- tex values ---

local function split_scope(...)
  local n = select("#", ...)
  local a1 = ...
  if n == 3 and a1 == "global" then return true, select(2, ...) end
  return false, select(n - 1, ...)
end
local function key_args(k)
  if type(k) == "string" then return nil, k end
  if type(k) == "number" then return k, nil end
  error("argument must be a string or a number", 3)
end
local function make_register(get, set, convert)
  local function setter(...)
    local global, k, v = split_scope(...)
    local i, name = key_args(k)
    if convert then v = convert(v) end
    set(i, name, v, global)
  end
  local function getter(k)
    local i, name = key_args(k)
    return get(i, name)
  end
  local proxy = setmetatable({}, {
    __index = function(_, k) return getter(k) end,
    __newindex = function(_, k, v) setter(k, v) end,
  })
  return setter, getter, proxy
end
local function to_sp(v)
  if type(v) == "string" then return tex.sp(v) end
  return v
end
tex.setcount, tex.getcount, tex.count = make_register(B.count_get, B.count_set)
tex.setdimen, tex.getdimen, tex.dimen = make_register(B.dimen_get, B.dimen_set, to_sp)
tex.settoks, tex.gettoks, tex.toks = make_register(B.toks_get, B.toks_set, tostring)
tex.setattribute, tex.getattribute, tex.attribute = make_register(B.attribute_get, B.attribute_set)
function tex.isattribute(k) return pcall(tex.getattribute, k) end
function tex.iscount(k) return pcall(tex.getcount, k) end

-- tex.print & co. (ltexlib.c do_luacprint / luac_store): strings and
-- numbers become pseudo-file lines, token objects are read back as such.
local DEFAULT_CAT, NO_CAT = -1, -2
local tointeger, tonumber = math.tointeger, tonumber
local function lua_int(v)
  -- lua_tointeger: 0 for anything without an exact integer value
  return tointeger(tonumber(v) or 0) or 0
end
local function store(v, partial, cattable)
  local t = type(v)
  if t == "string" or t == "number" then
    B.print_text(partial, cattable, tostring(v))
  elseif t == "table" and getmetatable(v) == tok_mt then
    B.print_token(partial, cattable, rawget(v, 1))
  else
    return false
  end
  return true
end
local function is_list(v)
  return type(v) == "table" and getmetatable(v) ~= tok_mt
end
local function cprint(partial, cattable, ...)
  local n, start = select("#", ...), 1
  if cattable ~= NO_CAT and type((...)) == "number" and n > 1 then
    cattable, start = lua_int((...)), 2
  end
  local first = select(start, ...)
  if is_list(first) then
    local i = 1
    while store(rawget(first, i), partial, cattable) do i = i + 1 end
  else
    for i = start, n do store((select(i, ...)), partial, cattable) end
  end
end
function tex.print(...) cprint(false, DEFAULT_CAT, ...) end
function tex.sprint(...) cprint(true, DEFAULT_CAT, ...) end
function tex.write(...) cprint(false, NO_CAT, ...) end
function tex.cprint(c, ...)
  c = lua_int(c)
  if c < 0 or c > 15 then c = 12 end
  local cattable = -c - 0xFF
  local first = ...
  if is_list(first) then
    local i = 1
    while store(rawget(first, i), true, cattable) do i = i + 1 end
  else
    for i = 1, select("#", ...) do store((select(i, ...)), true, cattable) end
  end
end
function tex.tprint(...)
  for i = 1, select("#", ...) do
    local t = select(i, ...)
    if not is_list(t) then error("no string to print", 2) end
    local cattable, j = DEFAULT_CAT, 1
    if type(t[1]) == "number" then cattable, j = lua_int(t[1]), 2 end
    while store(t[j], true, cattable) do j = j + 1 end
  end
end

-- texio (ltexiolib.c): an optional first selector argument, then strings.
local texio_targets = { ["term and log"] = 0, log = 1, term = 2 }
local function texio_print(nl, ...)
  local n = select("#", ...)
  local last = select(n, ...)
  if n == 0 or (type(last) ~= "string" and type(last) ~= "number") then
    error("no string to print", 3)
  end
  local target, start = 0, 1
  if n > 1 then
    local s = ...
    if type(s) == "string" then
      target, start = texio_targets[s] or 0, 2
    elseif type(s) == "number" then
      start = 2
    else
      error("first argument is not 'term and log', 'term', 'log' or a number", 3)
    end
  end
  for i = start, n do
    local s = select(i, ...)
    if type(s) ~= "string" and type(s) ~= "number" then error("argument is not a string", 3) end
    B.texio_print(target, nl, tostring(s))
  end
end
function texio.write(...) texio_print(false, ...) end
function texio.write_nl(...) texio_print(true, ...) end

function tex.setcatcode(...)
  local args = { ... }
  local i = 1
  local global = false
  if args[i] == "global" then global = true; i = i + 1 end
  local n = #args - i + 1
  if n == 3 then
    B.catcode_set(args[i], args[i + 1], args[i + 2], global)
  else
    B.catcode_set(nil, args[i], args[i + 1], global)
  end
end
function tex.getcatcode(a, b)
  if b ~= nil then return B.catcode_get(a, b) end
  return B.catcode_get(nil, a)
end

function tex.error(msg, help) B.tex_error(tostring(msg)) end
function tex.runtoks(f, ...)
  if type(f) == "function" then
    B.runtoks_begin()
    f()
    B.runtoks_pending()
  else
    local i, name = key_args(f)
    B.runtoks_toks(i, name)
  end
end
tex.chardef = token.set_char
function tex.hashtokens() return {} end

local tex_dynamic = {
  inputlineno = B.inputlineno,
}
setmetatable(tex, {
  __index = function(_, k)
    local f = tex_dynamic[k]
    if f then return f() end
    if type(k) ~= "string" then return nil end
    local ok, v = pcall(B.count_get, nil, k)
    if ok then return v end
    ok, v = pcall(B.dimen_get, nil, k)
    if ok then return v end
    return nil
  end,
})

-- ------------------------------------------------------------------ lua ---

local functions = {}
function lua.get_functions_table() return functions end
function __ratex_function(n)
  local f = functions[n]
  if type(f) == "function" then return f end
  return nil
end
local bytecode_shadow = {}
lua.bytecode = setmetatable({}, {
  __index = function(_, k)
    if type(k) ~= "number" or k < 0 then return nil end
    local f = bytecode_shadow[k]
    if f then return f end
    local code = B.bytecode_get(k)
    if code == nil then return nil end
    f = load(code, "bytecode", "b")
    if not f then error("bad bytecode register") end
    bytecode_shadow[k] = f
    return f
  end,
  __newindex = function(_, k, f, strip)
    if type(k) ~= "number" then error("bad argument") end
    if k < 0 then error("negative values not allowed") end
    if f ~= nil and type(f) ~= "function" then error("unsupported type") end
    bytecode_shadow[k] = nil
    B.bytecode_set(k, f and string.dump(f) or nil)
  end,
})
function lua.setbytecode(k, f, strip)
  if type(k) ~= "number" or k < 0 then error("negative values not allowed") end
  if f ~= nil and type(f) ~= "function" then error("unsupported type") end
  bytecode_shadow[k] = nil
  B.bytecode_set(k, f and string.dump(f, strip and true or false) or nil)
end
function lua.getbytecode(k) return lua.bytecode[k] end
lua.name = setmetatable({}, {
  __index = function(_, k) if type(k) == "number" then return B.name_get(k) end end,
  __newindex = function(_, k, v)
    if type(k) == "number" and k >= 0 and k <= 65535 then
      B.name_set(k, v ~= nil and tostring(v) or nil)
    end
  end,
})
function lua.setluaname(name, k) lua.name[k] = name end
function lua.getluaname(k) return lua.name[k] end

-- ------------------------------------------------------------- callback ---

local callback_names = {
  "find_write_file", "find_output_file", "find_image_file", "find_format_file",
  "find_read_file", "open_read_file", "find_vf_file", "read_vf_file",
  "find_data_file", "read_data_file", "find_font_file", "read_font_file",
  "find_map_file", "read_map_file", "find_enc_file", "read_enc_file",
  "find_type1_file", "read_type1_file", "find_truetype_file", "read_truetype_file",
  "find_opentype_file", "read_opentype_file", "find_cidmap_file", "read_cidmap_file",
  "find_pk_file", "read_pk_file", "show_error_hook", "process_input_buffer",
  "process_output_buffer", "process_jobname", "start_page_number", "stop_page_number",
  "start_run", "stop_run", "define_font", "pre_output_filter", "buildpage_filter",
  "hpack_filter", "vpack_filter", "glyph_not_found", "glyph_info", "hyphenate",
  "ligaturing", "kerning", "pre_linebreak_filter", "linebreak_filter",
  "post_linebreak_filter", "append_to_vlist_filter", "mlist_to_hlist", "finish_pdffile",
  "finish_pdfpage", "pre_dump", "start_file", "stop_file", "show_error_message",
  "show_lua_error_hook", "show_ignored_error_message", "show_warning_message",
  "hpack_quality", "vpack_quality", "process_rule", "insert_local_par",
  "contribute_filter", "call_edit", "build_page_insert", "glyph_stream_provider",
  "font_descriptor_objnum_provider", "finish_synctex", "wrapup_run", "new_graf",
  "page_order_index", "make_extensible", "process_pdf_image_content",
  "provide_charproc_data", "input_level_string", "dvi_output",
}
local callback_ids = {}
for i, name in ipairs(callback_names) do callback_ids[name] = i end
local callbacks = {}
callback = {}
function callback.register(name, f)
  if type(name) ~= "string" then
    return nil, "Invalid arguments to callback.register, first argument must be string."
  end
  local tf = type(f)
  if tf ~= "function" and tf ~= "nil" and not (tf == "boolean" and f == false) then
    return nil, "Invalid arguments to callback.register."
  end
  local id = callback_ids[name]
  if not id then return nil, "No such callback exists." end
  callbacks[id] = f
  return id
end
function callback.find(name)
  if type(name) ~= "string" then return nil, "Invalid arguments to callback.find." end
  local id = callback_ids[name]
  if not id then return nil, "No such callback exists." end
  return callbacks[id]
end
function callback.list()
  local t = {}
  for i, name in ipairs(callback_names) do t[name] = type(callbacks[i]) == "function" end
  return t
end
function callback.listidx()
  local t = {}
  for i, name in ipairs(callback_names) do t[i] = name end
  return t
end
function __ratex_callback(name)
  local f = callbacks[callback_ids[name]]
  if type(f) == "function" then return f end
  return nil
end

-- ----------------------------------------------------------------- kpse ---

kpse = kpse or {}
function kpse.find_file(name, fmt, mustexist)
  if type(name) ~= "string" then return nil end
  if type(fmt) ~= "string" then fmt = "tex" end
  return B.kpse_find(name, fmt)
end
kpse.lookup = kpse.find_file
function kpse.version() return "kpathsea version 6.4.2" end
function kpse.set_program_name() end

-- Files that kpse.find_file found in the embedded archive are read through
-- io.open/io.lines/lfs.attributes like disk files.
local embedded_prefix = "<embedded>/"
local function is_embedded(path)
  return type(path) == "string" and path:sub(1, #embedded_prefix) == embedded_prefix
end
local memfile = {}
memfile.__index = memfile
local function read_one(f, fmt)
  if type(fmt) == "number" then
    if f.pos > #f.data then return nil end
    local s = f.data:sub(f.pos, f.pos + fmt - 1)
    f.pos = f.pos + #s
    return s
  end
  fmt = tostring(fmt):gsub("^%*", "")
  local c = fmt:sub(1, 1)
  if c == "a" then
    local s = f.data:sub(f.pos)
    f.pos = #f.data + 1
    return s
  elseif c == "l" or c == "L" then
    if f.pos > #f.data then return nil end
    local e = f.data:find("\n", f.pos, true)
    local line
    if e then
      line = f.data:sub(f.pos, c == "L" and e or e - 1)
      f.pos = e + 1
    else
      line = f.data:sub(f.pos)
      f.pos = #f.data + 1
    end
    return line
  elseif c == "n" then
    local s, e = f.data:find("^%s*[-+]?%d*%.?%d+[eE]?[-+]?%d*", f.pos)
    if not s then return nil end
    f.pos = e + 1
    return tonumber(f.data:sub(s, e))
  end
  error("bad argument to 'read' (invalid format)")
end
function memfile:read(...)
  local n = select("#", ...)
  if n == 0 then return read_one(self, "l") end
  local out = {}
  for i = 1, n do
    out[i] = read_one(self, (select(i, ...)))
    if out[i] == nil then return table.unpack(out, 1, i) end
  end
  return table.unpack(out, 1, n)
end
function memfile:lines(fmt)
  return function() return read_one(self, fmt or "l") end
end
function memfile:seek(whence, offset)
  whence = whence or "cur"
  offset = offset or 0
  local base = whence == "set" and 0 or whence == "end" and #self.data or (self.pos - 1)
  self.pos = math.max(0, base + offset) + 1
  return self.pos - 1
end
function memfile:close() return true end
function memfile:write() return nil, "Bad file descriptor", 9 end
function memfile:setvbuf() return true end
function memfile:flush() return self end

local io_open, io_lines = io.open, io.lines
function io.open(path, mode)
  if is_embedded(path) then
    if mode and not tostring(mode):match("^r") then return nil, path .. ": Read-only file system", 30 end
    local data = B.read_found(path)
    if not data then return nil, path .. ": No such file or directory", 2 end
    return setmetatable({ data = data, pos = 1 }, memfile)
  end
  return io_open(path, mode)
end
function io.lines(path, ...)
  if is_embedded(path) then
    local f = assert(io.open(path))
    local fmt = ...
    return f:lines(fmt)
  end
  return io_lines(path, ...)
end
if lfs then
  local lfs_attributes, lfs_isfile = lfs.attributes, lfs.isfile
  function lfs.attributes(path, what)
    if is_embedded(path) then
      local data = B.read_found(path)
      if not data then return nil, path .. ": No such file or directory", 2 end
      local attr = { mode = "file", size = #data, modification = 0, access = 0, change = 0 }
      if what then return attr[what] end
      return attr
    end
    return lfs_attributes(path, what)
  end
  if lfs_isfile then
    function lfs.isfile(path)
      if is_embedded(path) then return B.read_found(path) ~= nil end
      return lfs_isfile(path)
    end
  end
end

-- luainit.c: package.searchers = { preload, kpse lua searcher }.
local function preload_searcher(name)
  local f = package.preload[name]
  if f == nil then return "\n\tno field package.preload['" .. name .. "']" end
  return f
end
local function kpse_lua_searcher(name)
  local code, path = B.lua_module(name)
  if code == nil then
    return "\n\t[kpse lua searcher] file not found: '" .. name .. "'"
  end
  local f, err = load(code, "@" .. path)
  if not f then
    error("error loading module " .. name .. " from file " .. path .. ":\n\t" .. err)
  end
  return f
end
package.searchers = { preload_searcher, kpse_lua_searcher }
