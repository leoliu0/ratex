tex.enableprimitives("", tex.extraprimitives())
tex.set("outputmode", 1)
local function rec(tag, ok, ...)
  local t = table.pack(...)
  for i = 1, t.n do t[i] = tostring(t[i]) end
  -- the position prefix names the script, which differs between the two runs
  P(tag, ok, (table.concat(t, ' '):gsub("^.-:%d+: ", "")))
end
local function dump(tag, i)
  local o = {}
  for _, k in ipairs(img.keys()) do
    local v = i[k]
    if k == "objnum" and v then v = "obj" end
    if type(v) == "table" then v = table.concat(v, ",") end
    o[#o + 1] = k .. "=" .. tostring(v)
  end
  P(tag, table.concat(o, ";"))
end

local i1 = img.scan { filename = "a.png" }
P("type", type(i1), (tostring(i1):gsub("%d+", "N")))
dump("scan", i1)
dump("w3cm", img.scan { filename = "a.png", width = tex.sp("3cm") })
dump("h", img.scan { filename = "a.png", height = tex.sp("2.5in") })
dump("w151499", img.scan { filename = "a.png", width = 151499 })
dump("wh", img.scan { filename = "a.png", width = 151499, height = 12345 })
dump("d", img.scan { filename = "a.png", depth = 151499 })
dump("tiny", img.scan { filename = "a.png", width = 1, height = 1 })
dump("strdim", img.scan { filename = "a.png", width = "1in" })

local n = img.node(i1)
P("node", n.id, n.subtype, n.width, n.height, n.depth, n.transform, n.index)
P("indexed", i1.index, i1.keepopen, type(i1.objnum))
local n2 = img.node { filename = "a.png", width = 65536 * 10 }
P("node2", n2.id, n2.subtype, n2.width, n2.height, n2.index)
local i3 = img.scan { filename = "a.png", width = 65536 * 10, transform = 3 }
local x = img.immediatewrite(i3)
P("iw", type(x), x == i3, i3.index, i3.transform, i3.keepopen)
local w = img.write(i1)
P("write", type(w), w == i1)

-- fields of scanned and fresh images
local fresh = img.new()
P("fresh", (tostring(fresh):gsub("%d+", "N")))
P("fresh keys", fresh.filename, fresh.width, fresh.index, fresh.page, fresh.pagebox, fresh.ref_count)
fresh.filename = "a.png"
fresh.pagebox = "crop"
fresh.width = 5.5
fresh.bogus = 3
P("fresh set", fresh.filename, fresh.pagebox, fresh.width, fresh.bogus, (tostring(fresh):gsub("%d+ >$", "N >")))
for _, case in ipairs {
  { "width", "x" }, { "filename", 5 }, { "bbox", { 1, 2, 3, 4 } }, { "keepopen", 1 },
  { "pagebox", "crop" }, { "page", "2" }, { "colorspace", 4 }, { "visiblefilename", "abc" },
  { "stream", "abc" }, { "index", 4 }, { "ref_count", 4 }, { "rotation", 4 }, { "imagetype", "jpg" },
  { "transform", 4 }, { "bogus", 1 },
} do
  local ok, msg = pcall(function() i1[case[1]] = case[2] end)
  P("set", case[1], ok, (tostring(msg):gsub("^.-:%d+: ", "")), tostring(i1[case[1]]) == tostring(case[2]))
end

-- copies share the dictionary
local c = img.copy(i1)
P("copy", c == i1, c.filename, c.ref_count, i1.ref_count, c.width == i1.width)
local m = i1 * 2
P("mul", m.width == i1.width * 2, m.ref_count, 2 * i1 == nil)
P("keys", table.concat(img.keys(), ","), #img.types(), table.concat(img.boxes(), ","))

-- error messages
rec("e1", pcall(function() return img.node(5) end))
rec("e2", pcall(function() return img.node() end))
rec("e3", pcall(function() return img.write(5) end))
rec("e4", pcall(function() return img.scan(5) end))
rec("e5", pcall(function() return img.scan() end))
rec("e6", pcall(function() return img.copy(5) end))
rec("e7", pcall(function() return img.copy() end))
rec("e8", pcall(function() return img.new(5) end))
rec("e9", pcall(function() return img.immediatewriteobject(i1) end))
rec("e10", pcall(function() return img.immediatewriteobject(5, 6) end))
rec("e11", pcall(function() return img.immediatewrite() end))
rec("e12", pcall(function() return img.new { width = {} } end))
rec("e13", pcall(function() return img.new { transform = "x" } end))
rec("e14", pcall(function() return img.new { bbox = { 1, 2 } } end))
rec("e15", pcall(function() return img.new { pagebox = true } end))
rec("e16", pcall(function() return img.new { page = true } end))
rec("e17", pcall(function() return img.new { keepopen = 1 } end))
rec("e18", pcall(function() return img.new { filename = 5 } end))
rec("e19", pcall(function() return img.new { foo = 1 }.foo end))
