local function d(head)
  local o = {}
  local prev = nil
  local n = head
  while n do
    local s = node.type(n.id) .. ":" .. tostring(n.subtype)
    if n.id == 29 then s = s .. "[" .. n.char .. "]" end
    if n.id == 13 then s = s .. "[" .. n.kern .. "]" end
    if n.id == 12 then s = s .. "[" .. n.width .. "]" end
    if n.id == 14 then s = s .. "[" .. n.penalty .. "]" end
    if n.prev ~= prev then s = s .. "!PREV" end
    o[#o+1] = s
    prev = n
    n = n.next
  end
  return table.concat(o, " ")
end
local function mk(chars)
  local head, tail
  for c in chars:gmatch(".") do
    local n
    if c == " " then n = node.new("glue") n.width = 10
    elseif c == "k" then n = node.new("kern") n.kern = 5
    elseif c == "p" then n = node.new("penalty") n.penalty = 100
    else n = node.new("glyph") n.char = c:byte() n.font = 0 end
    if tail then tail.next = n n.prev = tail else head = n end
    tail = n
  end
  return head, tail
end
local h, t = mk("ab kcp")
P("list", d(h))
P("length", node.length(h), node.length(h, t), node.length(h.next), node.length(nil))
P("count glyph", node.count(29, h), node.count(12, h), node.count(29, h, t), node.count(29, h, h.next.next))
P("tail", node.tail(h) == t, node.tail(nil), node.slide(h) == t)
local c = node.copy_list(h)
P("copy_list", d(c), c ~= h)
local c2 = node.copy_list(h, h.next.next)
P("copy_list to", d(c2))
local c3 = node.copy(h) P("copy", d(c3), c3.next)
-- insert_before / insert_after
local a = node.new("kern") a.kern = 1
local h2, cur = node.insert_before(h, h.next, a)
P("insert_before", d(h2), cur == a)
local b = node.new("kern") b.kern = 2
local h3, cur2 = node.insert_after(h2, t, b)
P("insert_after", d(h3), cur2 == b)
local h4, cur3 = node.insert_before(h3, h3, node.new("penalty"))
P("insert_before head", d(h4), node.type(cur3.id))
local x = node.new("penalty") x.penalty = 7
local h5, cur4 = node.insert_after(nil, nil, x)
P("insert_after nil", d(h5), cur4 == x)
-- remove
local h6, rem = node.remove(h4, h4.next)
P("remove", d(h6), node.type(rem.id), rem.next and rem.next.id, rem.prev and rem.prev.id)
local h7, rem2 = node.remove(h6, node.tail(h6))
P("remove tail", d(h7), rem2)
local h8, rem3 = node.remove(h7, h7)
P("remove head", d(h8), rem3 and rem3.id)
-- first_glyph, has_glyph
P("has_glyph", node.has_glyph(h8) and node.has_glyph(h8).char)
local k1 = node.new("kern")
P("has_glyph none", node.has_glyph(k1))
P("first_glyph", node.first_glyph(h8) and node.first_glyph(h8).char)
-- traverse
local o = {}
for n, id, st in node.traverse(h8) do o[#o+1] = id .. "/" .. st end
P("traverse", table.concat(o, " "))
o = {}
for n, st in node.traverse_id(29, h8) do o[#o+1] = n.char .. "/" .. st end
P("traverse_id", table.concat(o, " "))
o = {}
for n, c, f in node.traverse_char(h8) do o[#o+1] = c .. "/" .. f end
P("traverse_char", table.concat(o, " "))
o = {}
for n, c, f in node.traverse_glyph(h8) do o[#o+1] = c .. "/" .. f end
P("traverse_glyph", table.concat(o, " "))
local box = node.new("hlist") box.list = h8
o = {}
for n, id, st, l in node.traverse_list(box) do o[#o+1] = id end
P("traverse_list", table.concat(o, " "))
local hb = node.new("hlist") hb.list = box
o = {}
for n, id, st, l in node.traverse_list(hb.list) do o[#o+1] = id .. "/" .. tostring(l and l.id) end
P("traverse_list2", table.concat(o, " "))
-- getters
P("getnext/prev", node.getnext(h8) == h8.next, node.getprev(h8), node.getnext(nil))
local pn, nn = node.getboth(h8.next)
P("getboth", pn == h8, nn == h8.next.next)
-- flush
P("flush_list", node.flush_list(h8))
P("free", node.free(node.new("kern")))
local fl = node.new("kern") fl.next = node.new("kern")
P("free next", node.free(fl) ~= nil)
P("usedlist", type(node.usedlist()))
P("end_of_math", node.end_of_math(nil))
local m1 = node.new("math", 0) local m2 = node.new("math", 1) m1.next = m2 m2.prev = m1
P("end_of_math", node.end_of_math(m1) == m2)
P("is_node", node.is_node(m1) ~= false, node.is_node(5), node.is_node(nil), node.is_node({}))
P("is_zero_glue", node.is_zero_glue(node.new("glue")), node.is_zero_glue(node.new("glue")))
local gg = node.new("glue") gg.width = 5
P("is_zero_glue2", node.is_zero_glue(gg))
P(pcall(node.is_zero_glue, node.new("kern")))
