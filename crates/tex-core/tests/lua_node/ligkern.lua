local function ch(w, extra)
  local t = {width = w, height = 600000, depth = 100000}
  if extra then for k, v in pairs(extra) do t[k] = v end end
  return t
end
local chars = {
  [65] = ch(500000, {kerns = {[86] = -80000, [-2] = -1000}}),
  [86] = ch(500000),
  [102] = ch(300000, {ligatures = {[102] = {type = 0, char = 200}, [105] = {type = 0, char = 201}, [108] = {type = 0, char = 202}}}),
  [105] = ch(250000),
  [108] = ch(250000),
  [200] = ch(600000, {ligatures = {[105] = {type = 0, char = 203}}}),
  [201] = ch(550000),
  [202] = ch(550000),
  [203] = ch(800000),
  [97] = ch(450000, {ligatures = {[98] = {type = 2, char = 204}}}),
  [98] = ch(450000),
  [204] = ch(900000),
  [99] = ch(400000, {ligatures = {[100] = {type = 1, char = 205}}}),
  [100] = ch(400000),
  [205] = ch(700000),
  [101] = ch(400000, {ligatures = {[102] = {type = 7, char = 206}}}),
  [206] = ch(700000),
}
local id = font.define({
  name = "tst", size = 655360, designsize = 655360, type = "real", format = "type1",
  parameters = {slant = 0, space = 100000, space_stretch = 50000, space_shrink = 30000, x_height = 300000, quad = 655360, extra_space = 0},
  characters = chars,
})
local function mk(s)
  local head, tail
  for c in s:gmatch(".") do
    local n
    if c == " " then n = node.new("glue") n.width = 1000
    elseif c == "-" then n = node.new("disc")
    else n = node.new("glyph") n.char = c:byte() n.font = id n.lang = 0 end
    if tail then tail.next = n n.prev = tail else head = n end
    tail = n
  end
  return head, tail
end
local function d(head)
  local o = {}
  for n in node.traverse(head) do
    if n.id == 29 then
      local s = string.char(n.char < 128 and n.char or 63)
      if n.char >= 128 then s = "<" .. n.char .. ">" end
      o[#o+1] = s .. n.subtype
    elseif n.id == 13 then o[#o+1] = "k" .. n.kern .. "/" .. n.subtype
    elseif n.id == 7 then o[#o+1] = "{" .. d(n.pre) .. "|" .. d(n.post) .. "|" .. d(n.replace) .. "}"
    elseif n.id == 12 then o[#o+1] = "g"
    else o[#o+1] = node.type(n.id) end
  end
  return table.concat(o, " ")
end
P("id", id ~= nil)
for _, s in ipairs{"AV", "AVA", "ff", "ffi", "fi", "ffl", "fl", "ab", "cd", "ef", "AA V", "x"} do
  local h, t = mk(s)
  local nh, nt, ok = node.ligaturing(h, t)
  P("lig", s, d(nh), ok, nt and nt.id)
  h, t = mk(s)
  nh, nt, ok = node.kerning(h, t)
  P("kern", s, d(nh), ok)
  h, t = mk(s)
  nh = node.ligaturing(h, t)
  nh = node.kerning(nh)
  P("both", s, d(nh))
end
local h, t = mk("AVfi")
local nh, nt, ok = node.ligaturing(h)
P("lig no tail", d(nh), ok)
local h2 = mk("xAV")
local a, b = node.ligaturing(h2.next, node.tail(h2))
P("lig mid", d(h2), a.prev == h2)
P(node.ligaturing())
