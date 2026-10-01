local function mk(spec)
  local head, tail
  for c in spec:gmatch("[^ ]+") do
    local n
    local t, a, b, c2 = c:match("^(%a+):?(-?[%d.]*):?(-?[%d.]*):?(-?[%d.]*)$")
    if t == "g" then n = node.new("glue") n.width = tonumber(a) or 0 n.stretch = tonumber(b) or 0 n.shrink = tonumber(c2) or 0
    elseif t == "k" then n = node.new("kern") n.kern = tonumber(a)
    elseif t == "r" then n = node.new("rule") n.width = tonumber(a) n.height = tonumber(b) n.depth = tonumber(c2)
    elseif t == "p" then n = node.new("penalty") n.penalty = tonumber(a)
    elseif t == "gl" then n = node.new("glue") n.width = tonumber(a) n.stretch = tonumber(b) n.stretch_order = 2
    elseif t == "h" then n = node.new("hlist") n.width = tonumber(a) n.height = tonumber(b) n.depth = tonumber(c2)
    elseif t == "v" then n = node.new("vlist") n.width = tonumber(a) n.height = tonumber(b) n.depth = tonumber(c2)
    elseif t == "m" then n = node.new("math") n.surround = tonumber(a)
    else error("bad spec " .. c) end
    if tail then tail.next = n n.prev = tail else head = n end
    tail = n
  end
  return head
end
local function box(b)
  if not b then return "nil" end
  return string.format("%s w=%d h=%d d=%d sh=%d set=%.5f ord=%d sign=%d sub=%d", node.type(b.id), b.width, b.height, b.depth, b.shift, b.glue_set, b.glue_order, b.glue_sign, b.subtype)
end
local specs = {
  "r:100:20:5",
  "r:100:20:5 g:10:5:3 r:50:30:2",
  "r:100:20:5 gl:10:5 r:50:30:2",
  "h:10:7:3 k:4 v:20:8:9",
  "r:10:5:5 g:10:5:3 r:10:5:5 p:100 r:10:5:5",
  "m:3 r:5:5:5 m:3",
  "r:30:1:0 g:0:0:0",
}
for i, s in ipairs(specs) do
  local h = mk(s)
  local b, bad = node.hpack(h)
  P("hpack natural", i, box(b), bad)
  h = mk(s)
  b, bad = node.hpack(h, 200)
  P("hpack 200", i, box(b), bad)
  h = mk(s)
  b, bad = node.hpack(h, 20, "exactly")
  P("hpack exactly 20", i, box(b), bad)
  h = mk(s)
  b, bad = node.hpack(h, 30, "additional")
  P("hpack additional 30", i, box(b), bad)
  h = mk(s)
  b, bad = node.vpack(h)
  P("vpack natural", i, box(b), bad)
  h = mk(s)
  b, bad = node.vpack(h, 100)
  P("vpack 100", i, box(b), bad)
  h = mk(s)
  b, bad = node.vpack(h, 50, "exactly")
  P("vpack exactly 50", i, box(b), bad)
  h = mk(s)
  b, bad = node.vpack(h, 50, "additional")
  P("vpack additional 50", i, box(b), bad)
  h = mk(s)
  P("dimensions", i, node.dimensions(h))
  P("dimensions2", i, node.dimensions(h, h.next))
  P("dimensions glue", i, node.dimensions(0.5, 1, 0, h))
  P("dimensions glue2", i, node.dimensions(-0.5, 2, 2, h, node.tail(h)))
  local par = node.hpack(mk(s), 200)
  P("rangedimensions", i, node.rangedimensions(par, par.list))
  P("rangedimensions2", i, node.rangedimensions(par, par.list, node.tail(par.list)))
  P("rangedimensions3", i, node.rangedimensions(par, par.list.next or par.list, node.tail(par.list)))
  local pv = node.vpack(mk(s), 200)
  P("rangedimensions v", i, node.rangedimensions(pv, pv.list))
end
local h = mk("r:10:5:5")
P((pcall(node.hpack, h, 10, "foo")))
P((pcall(node.hpack, h, 10, "exactly", "TRT")))
local b = node.hpack(h, 10, "exactly", "TRT")
P(b and b.dir, b and b.direction)
local h2 = mk("r:10:5:5")
local b2 = node.hpack(h2, 10, "exactly", "TLT")
P(b2.dir)
P(node.dimensions(mk("r:10:5:5 r:20:6:3"), nil))
local d = mk("r:10:5:5 r:20:6:3")
P(node.dimensions(d, d.next))
P(node.dimensions(d.next, d))
P(node.dimensions(1, 0, 0, d, nil, "TRT"))
