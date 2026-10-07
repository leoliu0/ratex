-- traversal iterators and the properties tables (lnodelib.c)
local D = node.direct
local td, tn = D.todirect, D.tonode
local function g(c, f)
  local n = D.new("glyph") D.setchar(n, c) D.setfont(n, f or 1) return n
end
local function k(w) local n = D.new("kern") D.setkern(n, w) return n end
local function chain(...)
  local t = {...}
  for i = 1, #t - 1 do D.setlink(t[i], t[i + 1]) end
  return t[1]
end
local function names(head)
  local o = {}
  for n, id in D.traverse(head) do
    o[#o + 1] = id == 29 and ("g" .. D.getchar(n)) or (id == 13 and ("k" .. D.getkern(n))) or node.type(id)
  end
  return table.concat(o, " ")
end

-- what each iterator yields
local box = D.new("hlist")
D.setlist(box, chain(g(70), k(1)))
local prot = g(68) D.setsubtype(prot, 256)
local head = chain(g(65), k(5), g(66, 2), box, prot, D.new("vlist"), g(67))
for n, id, sub in D.traverse(head) do P("traverse", n == nil, id, sub) end
for n, sub in D.traverse_id(13, head) do P("traverse_id", D.getkern(n), sub) end
for n, c, f in D.traverse_char(head) do P("traverse_char", c, f) end
for n, c, f in D.traverse_glyph(head) do P("traverse_glyph", c, f) end
for n, id, sub, l in D.traverse_list(head) do P("traverse_list", id, sub, l and D.getchar(l)) end

-- number of results of the iterators, at an element and past the end
local last = D.tail(head)
local function count(...) return select("#", ...) end
local f, s, c = D.traverse(head)
P("traverse returns", count(D.traverse(head)), s == head, c)
P("next counts", count(f(s, nil)), count(f(s, last)))
f, s = D.traverse_id(29, head)
P("traverse_id counts", count(f(s, nil)), count(f(s, last)))
f, s = D.traverse_char(head)
P("traverse_char counts", count(f(s, nil)), count(f(s, last)))
f, s = D.traverse_list(head)
P("traverse_list counts", count(f(s, nil)), count(f(s, last)))

-- nil and 0 heads
P("nil heads", count(D.traverse(nil)), count(D.traverse_id(29, nil)), count(D.traverse_char(nil)),
  count(D.traverse_glyph(nil)), count(D.traverse_list(nil)))
P("nil iterator", count(D.traverse(nil)(nil, nil)), D.traverse(nil)(nil, nil))
P("zero heads", count(D.traverse(0)), count(D.traverse_id(29, 0)), count(D.traverse_char(0)),
  count(D.traverse_glyph(0)), count(D.traverse_list(0)))
P("userdata nil heads", count(node.traverse(nil)), count(node.traverse_id(29, nil)), count(node.traverse_list(nil)))

-- the filter of traverse_id goes through lua_tointeger
local function ids(id)
  local o = {}
  for n in D.traverse_id(id, head) do o[#o + 1] = D.getid(n) end
  return table.concat(o, ",")
end
P("traverse_id filters", ids(13.0), ids("13"), ids(13.5), ids(nil), ids(1000), ids(-1))
local n2 = 0
for n in D.traverse_id(29, tostring(head)) do n2 = n2 + 1 end
P("string head", n2)

-- the list is read as the loop runs: a node relinked or removed at the
-- current position changes what comes next
local a, b, cc, dd = g(1), g(2), g(3), g(4)
local h = chain(a, b, cc, dd)
local seen = {}
for n in D.traverse(h) do
  seen[#seen + 1] = D.getchar(n)
  if n == a then D.setlink(a, dd) end
end
P("skip by relinking", table.concat(seen, ","))
local x, y, z = g(11), g(12), g(13)
h = chain(x, y)
seen = {}
for n in D.traverse_char(h) do
  seen[#seen + 1] = D.getchar(n)
  if n == y then D.setlink(y, z) end
end
P("extended while traversing", table.concat(seen, ","))
seen = {}
local p, q, r = g(21), k(2), g(23)
h = chain(p, q, r)
for n in D.traverse_glyph(h) do
  seen[#seen + 1] = D.getchar(n)
  if n == p then
    local new = g(22)
    D.setlink(p, new) D.setlink(new, r)
  end
end
P("replaced next", table.concat(seen, ","))

-- traverse_list hands out the list without touching its prev pointer
local lb = D.new("hlist")
local lh = g(90)
D.setlist(lb, lh)
D.setprev(lh, lb)
for n, id, sub, l in D.traverse_list(lb) do P("traverse_list prev", D.getprev(l) == lb) end

-- userdata flavour
local uh = tn(head)
for n, id, sub in node.traverse(uh) do P("ud traverse", type(n), id, sub) end
for n, sub in node.traverse_id(13, uh) do P("ud traverse_id", n.kern, sub) end
for n, c, f in node.traverse_char(uh) do P("ud traverse_char", c, f, n.char) end
for n, id, sub, l in node.traverse_list(uh) do P("ud traverse_list", id, sub, l and l.char) end
local uf, us = node.traverse(uh)
P("ud returns", count(node.traverse(uh)), us == uh, count(uf(us, nil)), count(uf(us, tn(last))))

-- properties: the direct table is a plain table, the userdata one a proxy
local props = D.get_properties_table()
local uprops = node.get_properties_table()
P("properties tables", type(props), getmetatable(props) == nil, props == uprops, type(getmetatable(uprops)))
D.set_properties_mode(true, true)
P("after mode", getmetatable(props) == nil)
local pn = g(80)
D.setproperty(pn, { colour = "red" })
P("getproperty", D.getproperty(pn).colour, props[pn].colour, node.getproperty(tn(pn)).colour,
  uprops[tn(pn)].colour, rawget(uprops, tn(pn)))
uprops[tn(pn)] = { colour = "blue" }
P("proxy set", props[pn].colour, D.getproperty(0), count(D.getproperty(0)))
local cp = D.copy(pn)
P("copy inherits", props[cp] ~= props[pn], props[cp].colour, rawget(props[cp], "colour"),
  getmetatable(props[cp]).__index == props[pn])
D.set_properties_mode(true, false)
local cp2 = D.copy(pn)
P("copy shares", props[cp2] == props[pn])
P("flush returns", D.flush_properties_table() == props, next(props))
D.set_properties_mode(false, false)
