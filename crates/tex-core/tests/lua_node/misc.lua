local function g(c) local n = node.new("glyph") n.char = c n.font = 0 return n end
local function k(w) local n = node.new("kern") n.kern = w return n end
local function link(...)
  local t = {...}
  for i = 1, #t - 1 do t[i].next = t[i+1] t[i+1].prev = t[i] end
  return t[1], t[#t]
end
local function d(head)
  local o = {}
  for n in node.traverse(head) do
    local s = node.type(n.id)
    if n.id == 29 then s = s .. n.char .. ":" .. n.subtype elseif n.id == 13 then s = s .. n.kern
    elseif n.id == 7 then s = s .. "{" .. d(n.pre) .. "|" .. d(n.post) .. "|" .. d(n.replace) .. "}"
    elseif n.id == 0 or n.id == 1 then s = s .. "[" .. d(n.head) .. "]" end
    o[#o+1] = s
  end
  return table.concat(o, " ")
end
-- nested copy
local inner = link(g(65), k(3))
local box = node.new("hlist") box.head = inner
local copy = node.copy(box)
P("copy nested", d(box), d(copy), copy.head ~= box.head)
local l = link(box, g(66))
local lc = node.copy_list(l)
P("copy_list nested", d(lc), lc.head ~= box.head)
-- disc
local dd = node.new("disc")
dd.pre = g(45) dd.post = g(46) dd.replace = link(g(47), g(48))
local dc = node.copy(dd)
P("disc copy", d(dd), d(dc))
local pre, post, rep, st, pe = node.getdisc(dd)
P("getdisc", pre.char, post.char, rep.char)
dd.replace = nil
P("disc replace nil", d(dd))
node.direct.setdisc(node.direct.todirect(dd), node.direct.todirect(g(1)), node.direct.todirect(g(2)), node.direct.todirect(g(3)), 2, 30)
P("setdisc", d(dd), dd.subtype, dd.penalty)
-- flatten_discretionaries
local fl = link(g(65), dd, g(66))
local nh, count = node.flatten_discretionaries(fl)
P("flatten", d(nh), count)
-- check_discretionary(ies)
local cd = node.new("disc") cd.pre = g(45) cd.replace = g(46)
local cl = link(g(65), cd, g(66))
node.check_discretionaries(cl)
P("check_discretionaries", d(cl))
-- protect_glyph
local p = link(g(65), g(66), g(67))
node.protect_glyph(p)
P("protect", d(p), p.subtype, p.next.subtype)
node.protect_glyphs(p)
P("protect all", d(p))
node.unprotect_glyph(p)
P("unprotect", d(p))
node.unprotect_glyphs(p)
P("unprotect all", d(p))
local q = link(g(65), k(1), g(66))
P("is_char", node.is_char(q), node.is_char(q, 0), node.is_char(q, 1), node.is_char(q.next))
P("is_glyph", node.is_glyph(q), node.is_glyph(q.next))
local c1, f1 = node.is_char(q)
P("is_char vals", c1, f1)
node.protect_glyph(q)
P("is_char prot", node.is_char(q), node.is_glyph(q))
P("uses_font", node.uses_font(q, 0), node.uses_font(q, 1))
-- first_glyph / has_glyph
local function fgc(n) local r = node.first_glyph(n) return r and r.char end
P("first_glyph", fgc(q), fgc(q.next), fgc(q.next.next))
local bx = node.new("hlist") bx.head = link(k(1), g(70))
P("has_glyph box", node.has_glyph(bx))
local lk = link(k(1), bx)
P("first_glyph nested", node.first_glyph(lk))
-- effective_glue
local gl = node.new("glue") gl.width = 10 gl.stretch = 5 gl.shrink = 3
local hb = node.hpack(link(gl, k(2)), 100, "exactly")
P("effective_glue", node.effective_glue(gl, hb), node.effective_glue(gl))
-- protrusion_skippable
P("skippable", node.protrusion_skippable(k(0)), node.protrusion_skippable(k(1)), node.protrusion_skippable(g(65)), node.protrusion_skippable(node.new("penalty")), node.protrusion_skippable(node.new("glue")), node.protrusion_skippable(node.new("whatsit", "special")))
-- tostring / eq
local a1 = k(1)
local a2 = a1
P("eq", a1 == a2, a1 == k(1), a1 ~= nil, rawequal(a1, a2))
P("tostring fmt", (node.tostring(a1):gsub("%s+", " "):gsub("%d+", "N")), (tostring(a1):gsub("%s+", " "):gsub("%d+", "N")))
local lt = link(k(1), k(2))
P("tostring linked", (tostring(lt):gsub("%s+", " "):gsub("%d+", "N")), (tostring(lt.next):gsub("%s+", " "):gsub("%d+", "N")))
P("direct tostring", (node.direct.tostring(node.direct.new("kern")):gsub("%s+", " "):gsub("%d+", "N")))
P("direct tostring nil", node.direct.tostring(0), node.direct.tostring(nil))
-- properties
local pn = k(1)
P("prop none", node.getproperty(pn))
node.setproperty(pn, {a = 1})
P("prop", node.getproperty(pn).a, node.direct.getproperty(node.direct.todirect(pn)).a)
-- getfield on nil/invalid
P(pcall(node.getfield, k(1), "nonexistent"))
P(node.getfield(k(1), "kern"), node.getfield(k(7), "id"))
-- node.write
-- direct nodes
local dk = node.direct.new("kern")
P("todirect", node.direct.todirect(node.direct.tonode(dk)) == dk, node.direct.is_direct(dk) == dk, node.direct.is_direct(a1), node.direct.is_node(a1) == a1, node.direct.is_node(dk))
P("direct flush", node.direct.flush_node(dk), node.direct.flush_list(nil))
P(node.direct.getnext(nil), node.direct.getid(nil), node.direct.getfield(nil, "id"))
