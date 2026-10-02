local function attrs(n)
  local a = n.attr
  local o = {}
  if a then
    local c = a.next
    while c do o[#o+1] = c.number .. "=" .. c.value c = c.next end
    return a.id .. ":" .. table.concat(o, ",")
  end
  return "nil"
end
local n = node.new("glyph")
P("empty", attrs(n), node.has_attribute(n, 5), node.get_attribute(n, 5))
node.set_attribute(n, 5, 10)
node.set_attribute(n, 3, 7)
node.set_attribute(n, 8, -1)
P("set", attrs(n), node.has_attribute(n, 5), node.has_attribute(n, 5, 10), node.has_attribute(n, 5, 11), node.get_attribute(n, 3))
P("index", n[5], n[3], n[4], n[0])
n[5] = 99
P("index set", attrs(n), n[5])
n[3] = -0x7FFFFFFF
P("index unset", attrs(n), n[3])
P("unset", node.unset_attribute(n, 5, 99), node.unset_attribute(n, 5), attrs(n))
P("unset 2", node.unset_attribute(n, 8), attrs(n), node.unset_attribute(n, 77))
node.set_attribute(n, 1, 1)
local m = node.copy(n)
P("copy", attrs(m), attrs(n))
node.set_attribute(m, 1, 2)
P("copy cow", attrs(m), attrs(n))
local k = node.new("kern")
P("kern attr", attrs(k))
node.set_attribute(k, 2, 3)
P("kern", attrs(k))
-- find_attribute
local l1 = node.new("kern") local l2 = node.new("kern") local l3 = node.new("kern")
l1.next = l2 l2.prev = l1 l2.next = l3 l3.prev = l2
node.set_attribute(l2, 4, 40)
node.set_attribute(l3, 4, 41)
local function fa(...) local v, n = node.find_attribute(...) return v, n and n.id, n and n.attr and n.attr.next.value end
P("find", fa(l1, 4))
P("find from 2", fa(l2, 4))
P("find none", fa(l1, 9))
P("find val", fa(l1, 4, 41))
-- setattributelist / getattributelist (direct)
local d1 = node.direct.new("kern") local d2 = node.direct.new("kern")
node.direct.set_attribute(d1, 6, 60)
node.direct.setattributelist(d2, d1)
P("attrlist", node.direct.get_attribute(d2, 6), node.direct.has_attribute(d2, 6))
node.direct.set_attribute(d2, 6, 61)
P("attrlist cow", node.direct.get_attribute(d2, 6), node.direct.get_attribute(d1, 6))
node.direct.setattributelist(d2, nil)
P("attrlist nil", node.direct.get_attribute(d2, 6))
P("direct attr unset", node.direct.unset_attribute(d1, 6), node.direct.get_attribute(d1, 6))
-- current_attr
P("current_attr", node.current_attr())
P("n.attr set nil", pcall(function() n.attr = nil end), attrs(n))
