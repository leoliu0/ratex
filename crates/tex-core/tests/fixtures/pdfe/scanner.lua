-- pdfscanner probe: run by lua_pdfe.rs (the engine) and luatex alike; writes OUT
local out = {}
local function P(...)
  local t = table.pack(...)
  for i = 1, t.n do t[i] = tostring(t[i]) end
  out[#out + 1] = table.concat(t, " | ")
end
local function show(v, depth)
  depth = depth or 0
  if type(v) ~= "table" then
    if math.type(v) then return string.format("%s:%s", math.type(v), v) end
    return string.format("%q", tostring(v))
  end
  local keys = {}
  for k in pairs(v) do keys[#keys + 1] = k end
  table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
  local o = {}
  for _, k in ipairs(keys) do o[#o + 1] = tostring(k) .. "=" .. show(v[k], depth + 1) end
  return "{" .. table.concat(o, ",") .. "}"
end

-- rawget on the table decides: use explicit operator names instead of __index
local names = { "q", "Q", "cm", "w", "J", "j", "M", "d", "ri", "i", "gs", "m", "l", "c", "v", "y", "h", "re", "S", "s", "f", "F", "f*", "B", "B*", "b", "b*", "n", "W", "W*", "BT", "ET", "Tc", "Tw", "Tz", "TL", "Tf", "Tr", "Ts", "Td", "TD", "Tm", "T*", "Tj", "TJ", "'", '"', "d0", "d1", "CS", "cs", "SC", "SCN", "sc", "scn", "G", "g", "RG", "rg", "K", "k", "sh", "BI", "ID", "EI", "Do", "MP", "DP", "BMC", "BDC", "EMC", "BX", "EX" }
local function table_for(tag, popper)
  local ops = {}
  for _, n in ipairs(names) do
    ops[n] = function(scanner, info)
      local o = {}
      while true do
        local v = popper(scanner)
        if v == nil then break end
        table.insert(o, 1, show(v))
      end
      P(tag, n, table.concat(o, " "))
    end
  end
  return ops
end

local pdfs = { "streams.pdf", "obj.pdf", "objstm.pdf", "incr.pdf" }
for _, name in ipairs(pdfs) do
  local doc = pdfe.open(FIXDIR .. name)
  if doc then
    local pages = pdfe.getnofpages(doc)
    for i = 1, pages do
      local page = pdfe.getpage(doc, i)
      local contents = page and page.Contents
      if contents then
        P("page", name, i, pdfe.type(contents))
        P("scan", pcall(pdfscanner.scan, contents, table_for(name .. "#" .. i, function(s) return s:pop() end), {}))
      end
    end
  end
end

-- strings: tokens and the pop family
local src = [[
q 1 0 0 1 100.5 -200 cm
/Name /Other (a(nested)\) str\n\101\0509) <48656C6C6F2> [1 2.5 (x) /N [3 4] true false] << /A 1 /B [ 1 2 ] /C << /D (e) >> >> 12 .5 -.5 -3 d
0.00 1.25 -0.75 17 BT /F1 12 Tf (Hello) Tj [(A) -120 (B)] TJ ET % comment text
(back\
slash \q) <> () <4> w -.0 -  m 1.2.3 re 007 y
Q
]]
P("string", pdfscanner.scan(src, table_for("str", function(s) return s:pop() end), {}))

local function popall(tag, extra)
  local ops = {}
  for _, n in ipairs { "cm", "Tj", "TJ", "Tf", "m", "w", "gs", "Do", "BDC", "d", "re", "q", "l", "Q" } do
    ops[n] = function(s, info)
      local r = {}
      r[#r + 1] = "n=" .. show(s:popnumber())
      r[#r + 1] = "nm=" .. show(s:popname())
      r[#r + 1] = "s=" .. show(s:popstring())
      r[#r + 1] = "a=" .. show(s:poparray())
      r[#r + 1] = "d=" .. show(s:popdictionary())
      r[#r + 1] = "b=" .. show(s:popboolean())
      r[#r + 1] = "any=" .. show(s:pop())
      r[#r + 1] = "info=" .. tostring(info.x)
      P(tag, n, table.concat(r, " "))
    end
  end
  return ops
end
pdfscanner.scan("1 2 m /N Do (s) Tj [1 2] w << /K 1 >> gs true q [1 [2 3] 4] d 1.5 re /A /B Tf", popall("pop"), { x = 7 })
for i, text in ipairs {
  "/A#20B /C(x) /D<41> /E[1] /F<</G 1>> 1.5.5 5. .. - 2 TJ",
  "true false TJ",
  "<< /K [ [ 1 ] 2 ] >> gs",
  "[ [ [ 1 ] ] ] d",
  "[1 2 3] [4 5] re",
  "<< /A 1 >> << /B 2 >> gs",
  "[(a) [(b)] (c)] TJ [] TJ << >> gs",
  "1 2 3 m 4 5 l",
  "q q Q Q",
  "(a\\101b\\7c\\18) Tj <4 1 4 2> Tj",
  "% only a comment\n1 w",
  "1 w % trailing",
  "\255 1 w",
  "1 w \200 2 w",
} do
  pdfscanner.scan(text, table_for("x" .. i, function(s) return s:pop() end), {})
  pdfscanner.scan(text, popall("y" .. i), { x = i })
end
-- typed pops in order
pdfscanner.scan("1 /N (s) [1] << /K 1 >> true 2.5 d", {
  d = function(s)
    P("order", show(s:popnumber()), show(s:popboolean()), show(s:popdictionary()), show(s:poparray()),
      show(s:popstring()), show(s:popname()), show(s:popNumber()), show(s:popnumber()))
  end }, {})
-- done skips the rest
pdfscanner.scan("1 a 2 b 3 c", { a = function(s) P("a", s:pop() and "got") s:done() end, b = function() P("b") end, c = function() P("c") end }, {})
-- inline image
pdfscanner.scan("q BI /W 2 /H 2 /BPC 8 /CS /G ID \1\2\3\4\nEI Q", {
  BI = function(s) P("BI") end, ID = function(s) P("ID") end,
  EI = function(s) P("EI", s:pop() and "str") end, Q = function() P("Q") end }, {})
-- argument checks
P("noargs", pdfscanner.scan())
P("two", pdfscanner.scan("q", {}))
P("bad2", pcall(pdfscanner.scan, "q", 1, {}))
P("bad3", pcall(pdfscanner.scan, "q", {}, 1))
P("bad1", pcall(pdfscanner.scan, 5, {}, {}))
P("empty", pdfscanner.scan("", {}, {}))
-- errors propagate
P("error", pcall(pdfscanner.scan, "q", { q = function() error("boom", 0) end }, {}))
P("scanner type", type(pdfscanner), pdfscanner.scan ~= nil)
pdfscanner.scan("q", { q = function(s) P("scanner", type(s), tostring(s):match("^[^:]*")) end }, {})
-- pdfe streams as arrays of streams
do
  local doc = pdfe.open(FIXDIR .. "streams.pdf")
  if doc then
    local page = pdfe.getpage(doc, 1)
    local c = page and page.Contents
    P("array", c and pdfe.type(c), c and #c)
    if c then
      local n = 0
      P("scan", pcall(pdfscanner.scan, c, setmetatable({}, { __index = function(t, k) return function() n = n + 1 end end }), {}))
      P("count", n)
    end
  end
end

local f = io.open(OUT, "wb")
f:write(table.concat(out, "\n"), "\n")
f:close()
