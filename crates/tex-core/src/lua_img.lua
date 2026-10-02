-- LuaTeX `img` library (limglib.c) over the pdfTeX image machinery.
-- An image is a userdata whose fields live in `data[image]`: the image-level
-- values (dimensions, transform) and a dictionary shared between copies.
local I = __ratex_imglib
__ratex_imglib = nil

local type, error, tostring, setmetatable, pairs, ipairs, select, tonumber, floor =
      type, error, tostring, setmetatable, pairs, ipairs, select, tonumber, math.floor

local keys = { "attribute_list", "bbox", "colordepth", "colorspace", "depth", "filename", "filepath", "height",
               "imagetype", "index", "keepopen", "objnum", "pagebox", "page", "pages", "ref_count", "rotation",
               "stream", "transform", "visiblefilename", "width", "xres", "xsize", "yres", "ysize" }
local boxes = { "media", "crop", "bleed", "trim", "art" }

local data = setmetatable({}, { __mode = "k" })
local serial = 700

local function round(v) return floor(v + 0.5) end

local function dimen(v, field)
  local t = type(v)
  if t == "number" then return round(v) end
  if t == "string" then return tex.sp(v) end
  error("img." .. field .. " needs integer or nil value or dimension string", 3)
end

local function new_dict()
  return { ref_count = 1, pagebox = "media", page = 1, pages = 0, rotation = 0, orientation = 0,
           xres = 0, yres = 0, xsize = 0, ysize = 0, keepopen = false, nolength = false, notype = false,
           nobbox = false, scanned = false, index = -1 }
end

local function new_image(dict)
  serial = serial + 1
  return { dict = dict or new_dict(), transform = 0, id = serial }
end

local function wrap(a)
  local obj = I.newud()
  data[obj] = a
  return obj
end

local function typename(...)
  if select("#", ...) == 0 then return "no value" end
  return type((...))
end

-- luaL_checkudata: the image behind argument 1, called from the user's function
local function check(v, fname)
  local a = data[v]
  if not a then
    error("bad argument #1 to '" .. fname .. "' (image.meta expected, got " .. typename(v) .. ")", 3)
  end
  return a
end

local readonly = { filename = true, visiblefilename = true, userpassword = true, ownerpassword = true,
                   attr = true, page = true, colorspace = true, pagebox = true, keepopen = true, bbox = true }

-- lua_to_image: the fields an image accepts
local function assign(a, k, v)
  local d = a.dict
  if k == "width" or k == "height" or k == "depth" then
    if v == nil then a[k] = nil else a[k] = dimen(v, k) end
    return
  elseif k == "transform" then
    if type(v) ~= "number" then error("img.transform needs integer value", 3) end
    a.transform = floor(v)
    return
  elseif readonly[k] and d.scanned then
    error("img." .. k .. " is now read-only", 3)
  end
  if k == "filename" or k == "visiblefilename" or k == "userpassword" or k == "ownerpassword" then
    if type(v) ~= "string" then error("img." .. k .. " needs string value", 3) end
    d[k] = v
  elseif k == "attr" then
    if v ~= nil and type(v) ~= "string" then error("img.attr needs string or nil value", 3) end
    d.attr = v
  elseif k == "page" then
    if type(v) == "string" then d.pagename, d.page = v, 0
    elseif type(v) == "number" then d.page, d.pagename = floor(v), nil
    else error("img.page needs integer or string value", 3) end
  elseif k == "colorspace" then
    if v ~= nil and type(v) ~= "number" then error("img.colorspace needs integer or nil value", 3) end
    d.colorspace = v and floor(v) or nil
  elseif k == "pagebox" then
    if v == nil then d.pagebox = "media"
    elseif type(v) == "number" then d.pagebox = boxes[floor(v) + 1] or "media"
    elseif type(v) == "string" then
      d.pagebox = "media"
      for _, b in ipairs(boxes) do if b == v then d.pagebox = b end end
    else error("img.pagebox needs string, number or nil value", 3) end
  elseif k == "keepopen" then
    if type(v) ~= "boolean" then error("img.bbox needs boolean value", 3) end
    d.keepopen = v
  elseif k == "nolength" or k == "notype" or k == "nobbox" then
    d[k] = v and true or false
  elseif k == "bbox" then
    if type(v) ~= "table" then error("img.bbox needs table value", 3) end
    if #v ~= 4 then error("img.bbox table must have exactly 4 elements", 3) end
    local b = {}
    for i = 1, 4 do
      local e = v[i]
      if type(e) == "number" or type(e) == "string" then b[i] = dimen(e, "bbox")
      else error("img.bbox table needs integer value or dimension string elements", 3) end
    end
    d.bbox = b
  elseif k == "stream" then
    if d.filename ~= nil then error("img.stream can't be used with image.filename", 3) end
    if d.scanned then error("img.stream is now read-only", 3) end
    d.stream, d.imagetype = v, "stream"
  end
  -- other keys are tolerated and ignored
end

local function bbox_of(d)
  if d.bbox then return { d.bbox[1], d.bbox[2], d.bbox[3], d.bbox[4] } end
  return { 0, 0, d.xsize, d.ysize }
end

local nilable = { colorspace = true, colordepth = true, objnum = true, index = true, filename = true,
                  visiblefilename = true, filepath = true, attr = true, userpassword = true, ownerpassword = true }

local mt = I.meta
mt.__index = function(self, k)
  local a = data[self]
  local d = a.dict
  if k == "width" or k == "height" or k == "depth" or k == "transform" then return a[k]
  elseif k == "page" then return d.pagename or d.page
  elseif k == "bbox" then return bbox_of(d)
  elseif k == "imagetype" then return d.imagetype
  elseif k == "stream" then return d.imagetype == "stream" and d.stream or nil
  elseif k == "ref_count" or k == "pages" or k == "xsize" or k == "ysize" or k == "xres" or k == "yres"
      or k == "rotation" or k == "orientation" or k == "pagebox" or k == "keepopen" or k == "nolength"
      or k == "notype" or k == "nobbox" or nilable[k] then
    local v = d[k]
    if v == "" then return nil end
    return v
  end
  return nil
end
mt.__newindex = function(self, k, v) assign(data[self], k, v) end
mt.__tostring = function(self)
  local a = data[self]
  local d = a.dict
  if d.filename == nil then return "<img unset : " .. a.id .. " >" end
  return "<img " .. d.filename .. " : " .. tostring(d.pagename or d.page) .. " : " .. a.id .. " >"
end
mt.__mul = function(x, y)
  if type(x) == "number" then x, y = y, x end
  local a = check(x, "?")
  local scale = tonumber(y) or 0
  local d = a.dict
  d.ref_count = d.ref_count + 1
  local b = new_image(d)
  b.transform = a.transform
  for _, f in ipairs { "width", "height", "depth" } do
    if a[f] then b[f] = round(a[f] * scale) end
  end
  return wrap(b)
end

local img = {}
_G.img = img

function img.types() return { [0] = "none", "pdf", "png", "jpg", "jp2", "jbig2", "stream", "memstream" } end
function img.boxes() return { "none", "media", "crop", "bleed", "trim", "art" } end
function img.keys() local t = {} for i, k in ipairs(keys) do t[i] = k end return t end
img.fields = img.keys

-- l_new_image
function img.new(...)
  local spec = ...
  if select("#", ...) > 0 and type(spec) ~= "table" then
    error("img.new needs table as optional argument", 2)
  end
  local a = new_image()
  if spec then
    for k, v in pairs(spec) do assign(a, k, v) end
  end
  return wrap(a)
end

function img.copy(...)
  if select("#", ...) ~= 1 then error("img.copy needs an image as argument", 2) end
  local spec = ...
  if type(spec) == "table" then return img.new(spec) end
  local a = check(spec, "copy")
  local d = a.dict
  d.ref_count = d.ref_count + 1
  local b = new_image(d)
  b.width, b.height, b.depth, b.transform = a.width, a.height, a.depth, a.transform
  return wrap(b)
end

local function num(obj, field)
  local _, n = I.info(obj, field)
  return n
end

-- read_scale_img: read the file the first time and scale the running dimensions
local function scan_into(a)
  local d = a.dict
  if d.filename == nil or d.filename == "" then I.fatal("error:  (pdf backend): image file name missing") end
  local obj = I.scan({
    filename = d.filename, attr = d.attr, page = d.page ~= 0 and d.page or nil,
    colorspace = d.colorspace, pagebox = d.pagebox,
    width = a.width, height = a.height, depth = a.depth,
  })
  if not obj then I.fatal("error:  (pdf backend): cannot find image file '" .. d.filename .. "'") end
  d.objnum = nil
  d.filepath = I.info(obj, "path")
  -- kpathsea reports a file of the current directory as "./name"
  if d.filepath and not d.filepath:find("/", 1, true) then d.filepath = "./" .. d.filepath end
  d.imagetype = I.info(obj, "type")
  a.width, a.height, a.depth = num(obj, "width"), num(obj, "height"), num(obj, "depth")
  d.xsize, d.ysize = num(obj, "xsize"), num(obj, "ysize")
  d.rotation = num(obj, "rotation")
  d.pages = num(obj, "pages")
  if d.imagetype == "pdf" then
    local _, p, q, r, s = I.info(obj, "bbox")
    d.bbox = { p, q, r, s }
    d.colordepth = nil
  else
    d.bbox = nil
    d.colordepth = num(obj, "colordepth")
  end
  d.obj = obj
  d.scanned = true
end

function img.scan(...)
  if select("#", ...) ~= 1 then error("img.scan needs exactly 1 argument", 2) end
  local spec = ...
  if type(spec) == "table" then spec = img.new(spec) end
  scan_into(check(spec, "scan"))
  return spec
end

-- limglib.c setup_image: the first write or node gives the image its index
local function setup(a)
  local d = a.dict
  scan_into(a)
  if d.index == -1 then
    d.index = I.index_of(d.obj)
    d.objnum = d.obj
  end
end

-- write_image_or_node: the image of the single argument, a table being turned into one
local function image_arg(fname, ...)
  if select("#", ...) ~= 1 then error("img." .. fname .. "() expects an argument", 3) end
  local spec = ...
  if type(spec) == "table" then spec = img.new(spec) end
  return check(spec, fname), spec
end

function img.write(...)
  local a, spec = image_arg("write", ...)
  setup(a)
  I.ref(a.dict.obj)
  return spec
end

-- the rule node of subtype "image" that stands for the image
function img.node(...)
  local a = image_arg("node", ...)
  setup(a)
  local n = node.new("rule", 2)
  n.width, n.height, n.depth = a.width, a.height, a.depth
  n.index = a.dict.index
  n.transform = a.transform
  return n
end

function img.immediatewrite(...)
  local a, spec = image_arg("immediatewrite", ...)
  setup(a)
  I.write_now(a.dict.obj)
  a.dict.keepopen = true
  return spec
end

function img.immediatewriteobject(...)
  if select("#", ...) ~= 2 then error("img.immediatewrite() expects two argument", 2) end
  local a = check((...), "immediatewriteobject")
  scan_into(a)
  -- only PDF files are written as an object of a given number (write_epdf_object)
  if a.dict.imagetype ~= "pdf" then I.fatal("error:  (pdf inclusion): unknown document") end
  I.write_now(a.dict.obj)
  return a.dict.obj
end

-- pdf.includeimage(index): writes the image and reports its properties
function pdf.includeimage(index)
  local obj = I.obj_of(math.tointeger(tonumber(index)) or 0)
  if not obj then error("bad argument #1 to 'includeimage' (invalid image index)", 2) end
  I.write_now(obj)
  local kind = I.info(obj, "type")
  local kinds = { none = 0, pdf = 1, png = 2, jpg = 3, jp2 = 4, jbig2 = 5, stream = 6, memstream = 7 }
  return kinds[kind] or 0, 0, 0, num(obj, "xsize"), num(obj, "ysize"), num(obj, "rotation"), obj, nil
end
