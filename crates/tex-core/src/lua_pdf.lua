-- LuaTeX `pdf` library (lpdflib.c) over the pdfTeX backend of the engine.
local P = __ratex_pdflib
__ratex_pdflib = nil

local type, select, error, tostring, tonumber = type, select, error, tostring, tonumber
local tointeger = math.tointeger

local function lua_int(v) return tointeger(tonumber(v) or 0) or 0 end

pdf = {}
local pdf = pdf

-- ------------------------------------------------------------- variables ---

-- Lua name, \pdfvariable key, class
local variables = {
  { "compresslevel", "compresslevel", "int" },
  { "objcompresslevel", "objcompresslevel", "int" },
  { "decimaldigits", "decimaldigits", "int" },
  { "imageresolution", "imageresolution", "int" },
  { "pkresolution", "pkresolution", "int" },
  { "gentounicode", "gentounicode", "int" },
  { "inclusionerrorlevel", "inclusionerrorlevel", "int" },
  { "ignoreunknownimages", "ignoreunknownimages", "int" },
  { "omitcharset", "omitcharset", "int" },
  { "omitcidset", "omitcidset", "int" },
  { "omitinfo", "omitinfodict", "int" },
  { "omitmediabox", "omitmediabox", "int" },
  { "omitprocset", "omitprocset", "int" },
  { "ptexprefix", "ptexprefix", "int" },
  { "recompress", "recompress", "int" },
  { "suppressoptionalinfo", "suppressoptionalinfo", "int" },
  { "majorversion", "majorversion", "int" },
  { "minorversion", "minorversion", "int" },
  { "destmargin", "destmargin", "dim" },
  { "linkmargin", "linkmargin", "dim" },
  { "threadmargin", "threadmargin", "dim" },
  { "xformmargin", "xformmargin", "dim" },
  { "pageattributes", "pageattr", "text" },
  { "pagesattributes", "pagesattr", "text" },
  { "pageresources", "pageresources", "text" },
  { "xformattributes", "xformattr", "text" },
  { "xformresources", "xformresources", "text" },
  { "trailerid", "trailerid", "text" },
  { "catalog", "catalog", "text" },
  { "info", "info", "text" },
  { "names", "names", "text" },
  { "trailer", "trailer", "text" },
}
for _, v in ipairs(variables) do
  local name, key, class = v[1], v[2], v[3]
  if class == "text" then
    pdf["get" .. name] = function()
      local _, _, text = P.var_get(key)
      return text
    end
    pdf["set" .. name] = function(s)
      if type(s) ~= "string" and type(s) ~= "number" then error("string expected", 2) end
      P.var_set(key, 0, tostring(s))
    end
  else
    pdf["get" .. name] = function()
      local _, n = P.var_get(key)
      return n
    end
    pdf["set" .. name] = function(n)
      if type(n) ~= "number" then return end
      P.var_set(key, lua_int(n))
    end
  end
end

function pdf.getorigin()
  local _, h = P.var_get("horigin")
  local _, v = P.var_get("vorigin")
  return h, v
end
function pdf.setorigin(h, v)
  h = lua_int(h)
  if v == nil then v = h else v = lua_int(v) end
  P.var_set("horigin", h)
  P.var_set("vorigin", v)
end

-- ------------------------------------------------------------- queries ---

function pdf.getpos() return P.pos() end
function pdf.gethpos() return (P.pos()) end
function pdf.getvpos() local _, v = P.pos() return v end
function pdf.getlastobj() return P.last("obj") end
function pdf.getlastannot() return P.last("annot") end
function pdf.getlastlink() return P.last("link") end
function pdf.getretval() return P.last("retval") end
function pdf.getmaxobjnum() return P.max_obj() end
function pdf.getnofobjects() return P.max_obj() end
function pdf.getobjtype(n) return P.obj_type(lua_int(n)) end
function pdf.getfontname(f) return P.font_name(lua_int(f)) end
function pdf.getfontobjnum(f) return P.font_objnum(lua_int(f)) end
function pdf.getfontsize(f) return P.font_size(lua_int(f)) end
function pdf.getpageref(n) return P.page_ref(lua_int(n)) end
function pdf.getxformname(n) return P.xform_name(lua_int(n)) end
function pdf.getmatrix() return 1, 0, 0, 1, 0, 0 end
function pdf.hasmatrix() return false end

function pdf.getcreationdate() return P.creation_date() end

-- the non-"get" spellings LuaTeX keeps
pdf.fontname, pdf.fontobjnum, pdf.fontsize = pdf.getfontname, pdf.getfontobjnum, pdf.getfontsize
pdf.maxobjnum, pdf.objtype, pdf.pageref, pdf.xformname = pdf.getmaxobjnum, pdf.getobjtype, pdf.getpageref, pdf.getxformname

-- ------------------------------------------------------------- actions ---

local function cs(name) return { name } end
local function backend(name) return { name, "backend" } end

local function object_parts(immediate, ...)
  local n = select("#", ...)
  local args = { ... }
  local i = 1
  local parts = {}
  if immediate then parts[#parts + 1] = cs("immediate") end
  parts[#parts + 1] = backend("pdfobj")
  if type(args[i]) == "number" then
    parts[#parts + 1] = "useobjnum " .. lua_int(args[i]) .. " "
    i = i + 1
  end
  local kind
  if args[i] == "file" or args[i] == "stream" or args[i] == "streamfile" then
    kind = args[i]
    i = i + 1
  end
  local body, attr = args[i], args[i + 1]
  if type(body) ~= "string" then error("pdf object content must be a string", 3) end
  if kind == "stream" or kind == "streamfile" then parts[#parts + 1] = "stream " end
  if kind == "file" or kind == "streamfile" then parts[#parts + 1] = "file " end
  if attr ~= nil and kind ~= nil and kind ~= "file" then
    parts[#parts + 1] = "attr "
    parts[#parts + 1] = 1; parts[#parts + 1] = tostring(attr); parts[#parts + 1] = 2
  end
  parts[#parts + 1] = 1; parts[#parts + 1] = body; parts[#parts + 1] = 2
  return parts
end

function pdf.immediateobj(...)
  P.run(object_parts(true, ...))
  return P.last("obj")
end
function pdf.obj(...)
  P.run(object_parts(false, ...))
  return P.last("obj")
end
function pdf.reserveobj(kind)
  if kind == "annot" then
    P.run { backend("pdfannot"), "reserveobjnum " }
    return P.last("annot")
  end
  P.run { backend("pdfobj"), "reserveobjnum " }
  return P.last("obj")
end
function pdf.refobj(n)
  P.run { backend("pdfrefobj"), tostring(lua_int(n)) .. " " }
end

function pdf.mapfile(s) P.map(tostring(s), true) end
function pdf.mapline(s) P.map(tostring(s), false) end

function pdf.newcolorstack(init, mode, pagestart)
  local parts = {}
  if pagestart then parts[#parts + 1] = "page " end
  if mode ~= nil then parts[#parts + 1] = tostring(mode) .. " " end
  parts[#parts + 1] = 1; parts[#parts + 1] = tostring(init); parts[#parts + 1] = 2
  return P.colorstack_init(parts)
end

function pdf.setfontattributes(f, s)
  P.run { backend("pdffontattr"), { lua_int(f), "font" }, 1, tostring(s), 2 }
end

function pdf.includechar(f, c)
  f = lua_int(f)
  if type(c) == "table" then
    for i = 1, #c do P.include_char(f, lua_int(c[i])) end
  else
    P.include_char(f, lua_int(c))
  end
end

function pdf.includefont(f)
  P.include_font(lua_int(f))
end

-- lpdflib.c luapdfprint: the literal modes of pdf.print
local print_modes = { origin = 0, page = 1, text = 2, direct = 3, raw = 4 }
function pdf.print(...)
  local n = select("#", ...)
  local s = select(n, ...)
  if n == 0 or (type(s) ~= "string" and type(s) ~= "number") then error("no string to print", 0) end
  local mode = 0
  if n == 2 then
    local m = ...
    mode = type(m) == "string" and print_modes[m]
    if not mode then error("invalid first argument for print literal mode", 0) end
  elseif n ~= 1 then
    error("invalid number of arguments", 0)
  end
  P.print(mode, tostring(s))
end

function pdf.registerannot(...)
  if select("#", ...) ~= 1 then error("pdf.registerannot() needs exactly 1 argument", 0) end
  if not P.in_late_lua() then error("pdf.registerannot() can only be used in late lua", 0) end
  local n = tointeger(tonumber((...)))
  if n == nil then error("bad argument #1 to 'registerannot' (number expected, got " .. type((...)) .. ")", 2) end
  if n <= 0 then error("pdf.registerannot() can only register positive object numbers", 0) end
  P.register_annot(n)
end

-- lpdflib.c: only used while tracing, when no page was flushed
function pdf.setforcefile(v) P.set_force_file(v == true) end

-- lpdflib.c: the experimental wide Type 1 mode
function pdf.settypeonewidemode(v) P.set_type1_wide_mode(tointeger(tonumber(v)) or 0) end
