-- LuaTeX `img` library (limglib.c) over the pdfTeX image machinery.
local I = __ratex_imglib
__ratex_imglib = nil

local type, error, tostring, setmetatable, pairs, ipairs, rawget =
      type, error, tostring, setmetatable, pairs, ipairs, rawget

local keys = { "attribute_list", "bbox", "colordepth", "colorspace", "depth", "filename", "filepath", "height",
               "imagetype", "index", "keepopen", "objnum", "pagebox", "page", "pages", "ref_count", "rotation",
               "stream", "transform", "visiblefilename", "width", "xres", "xsize", "yres", "ysize" }
local valid = {}
for _, k in ipairs(keys) do valid[k] = true end

local data = setmetatable({}, { __mode = "k" })
local serial = 700

local mt = {}
mt.__index = function(self, k)
  local d = data[self]
  if valid[k] then return d[k] end
  return nil
end
mt.__newindex = function(self, k, v)
  if not valid[k] then error("img." .. tostring(k) .. " is not a valid field", 2) end
  data[self][k] = v
end
mt.__tostring = function(self)
  local d = data[self]
  if d.filename then return "<img " .. d.filename .. " : " .. d.ref_count .. " : " .. d.serial .. " >" end
  return "<img unset : " .. d.serial .. " >"
end

local function defaults()
  serial = serial + 1
  return {
    bbox = { 0, 0, 0, 0 }, index = -1, keepopen = false, pagebox = "media", page = 1, pages = 0,
    ref_count = 1, rotation = 0, transform = 0, xres = 0, yres = 0, xsize = 0, ysize = 0, serial = serial,
  }
end

local function wrap(d)
  local obj = {}
  data[obj] = d
  return setmetatable(obj, mt)
end

local function spec_of(v)
  if type(v) == "table" then
    local d = data[v]
    if d then return d end
    return v
  end
  error("image table expected", 3)
end

local img = {}
_G.img = img

function img.types() return { [0] = "none", "pdf", "png", "jpg", "jp2", "jbig2", "stream", "memstream" } end
function img.boxes() return { "none", "media", "crop", "bleed", "trim", "art" } end
function img.keys() local t = {} for i, k in ipairs(keys) do t[i] = k end return t end
img.fields = img.keys

function img.new(spec)
  local d = defaults()
  if spec ~= nil then
    if type(spec) ~= "table" then error("img.new: table expected", 2) end
    for k, v in pairs(spec) do
      if not valid[k] then error("img.new: invalid field " .. tostring(k), 2) end
      d[k] = v
    end
  end
  return wrap(d)
end

function img.copy(i)
  local d = data[i]
  if not d then error("img.copy: image expected", 2) end
  local c = defaults()
  for k, v in pairs(d) do c[k] = v end
  c.bbox = { d.bbox[1], d.bbox[2], d.bbox[3], d.bbox[4] }
  c.serial = nil
  serial = serial + 1
  c.serial = serial
  c.ref_count = d.ref_count + 1
  return wrap(c)
end

local function scan_into(d)
  local obj = I.scan(d)
  if not obj then
    error("cannot find image file '" .. tostring(d.filename) .. "'", 3)
  end
  d.objnum = nil
  d.filepath = I.info(obj, "path")
  d.imagetype = I.info(obj, "type")
  d.width, d.height, d.depth = I.info(obj, "width"), I.info(obj, "height"), I.info(obj, "depth")
  d.xsize, d.ysize = I.info(obj, "xsize"), I.info(obj, "ysize")
  d.rotation = I.info(obj, "rotation")
  d.pages = I.info(obj, "pages")
  d.page = d.page or 1
  if d.imagetype == "pdf" then
    local _, a, b, c, e = I.info(obj, "bbox")
    d.bbox = { a, b, c, e }
    d.colordepth = nil
  else
    d.bbox = { 0, 0, d.xsize, d.ysize }
    d.colordepth = I.info(obj, "colordepth")
  end
  d.visiblefilename = d.visiblefilename
  d.__obj = obj
  return d
end

function img.scan(spec)
  local d
  local given = spec_of(spec)
  if data[spec] then
    d = given
  else
    d = defaults()
    for k, v in pairs(given) do
      if not valid[k] then error("img.scan: invalid field " .. tostring(k), 2) end
      d[k] = v
    end
  end
  scan_into(d)
  return data[spec] and spec or wrap(d)
end

-- the pdfTeX object number of a scanned image
local function object_of(i)
  local d = data[i]
  if not d then error("img.write: image expected", 3) end
  if not d.__obj then scan_into(d) end
  return d
end

function img.write(i)
  local d = object_of(i)
  d.index = d.__obj
  I.ref(d.__obj)
  return i
end
