-- LPeg behaviour fixture, run by tests/lua_lpeg.rs under ratex and (to regenerate
-- lpeg_cases.expected) under TeX Live's texlua.
local P,S,R,C,V,B,Cc,Cg,Cb,Ct,Cs,Cf,Cmt,Cp,Carg=lpeg.P,lpeg.S,lpeg.R,lpeg.C,lpeg.V,lpeg.B,lpeg.Cc,lpeg.Cg,lpeg.Cb,lpeg.Ct,lpeg.Cs,lpeg.Cf,lpeg.Cmt,lpeg.Cp,lpeg.Carg
local state
local function rnd(n) state = (state * 1103515245 + 12345) % 2147483648; return (state // 65536) % n + 1 end
local function pick(t) return t[rnd(#t)] end

local function ser(v, depth)
  depth = depth or 0
  local t = type(v)
  if t == "string" then
    return '"' .. v:gsub("[^%w ,%-%(%)]", function(c) return string.format("\\%d", c:byte()) end) .. '"'
  elseif t == "number" then
    return (math.type(v) == "integer" and "i" or "f") .. tostring(v)
  elseif t == "table" then
    if depth > 6 then return "{...}" end
    local keys = {}
    for k in pairs(v) do keys[#keys+1] = k end
    table.sort(keys, function(a,b) return ser(a) < ser(b) end)
    local out = {}
    for _, k in ipairs(keys) do out[#out+1] = ser(k) .. "=" .. ser(v[k], depth+1) end
    return "{" .. table.concat(out, ",") .. "}"
  else
    return tostring(v):gsub("0x%x+","PTR")
  end
end
local function sers(...)
  local n = select('#', ...)
  local out = {}
  for i = 1, n do out[#out+1] = ser((select(i, ...))) end
  return n .. ":" .. table.concat(out, " ")
end

local capfuncs = {
  function(a, b) return a end,
  function(...) return ... end,
  function(...) return select('#', ...) end,
  function() end,
  function(a) return a and #tostring(a) end,
  function(a, b, c) return b, a, c end,
  function(a) return nil end,
  function(a) return {a} end,
  function(a) return false end,
}
local folds = {
  function(acc, ...) return ser(acc) .. "|" .. sers(...) end,
  function(acc, v) return (type(acc) == "number" and acc or 0) + (tonumber(v) or 1) end,
  function(acc, ...) return select('#', ...) end,
}
local cmts = {
  function(s, i, ...) return i end,
  function(s, i) return false end,
  function(s, i) return i + 1 <= #s + 1 and i + 1 end,
  function(s, i, ...) return true, ... end,
  function(s, i, c) return i, "dyn", i end,
  function(s, i, ...) return true, select('#', ...), s:sub(i, i) end,
  function(s, i, ...) return i, ... end,
  function() end,
  function(s, i) return nil end,
}
local tabs = { {a=1, b="x"}, {}, setmetatable({}, {__index = function(t, k) return k .. k end}), {["a"]="A", ["b"]=false} }
local strs = { "%1-%0", "x", "", "[%0]", "%%%1%2", "%1%1", "a%0b" }

local names = {"S", "A", "B"}
local function leaf(ingrammar)
  local r = rnd(24)
  if r == 1 then return P"a", 'P"a"'
  elseif r == 2 then return P"ab", 'P"ab"'
  elseif r == 3 then return P(1), 'P(1)'
  elseif r == 4 then return P(2), 'P(2)'
  elseif r == 5 then return P(0), 'P(0)'
  elseif r == 6 then return P(-1), 'P(-1)'
  elseif r == 7 then return P(true), 'P(true)'
  elseif r == 8 then return P(false), 'P(false)'
  elseif r == 9 then return S"ab", 'S"ab"'
  elseif r == 10 then return R"ad", 'R"ad"'
  elseif r == 11 then return R("az","12"), 'R("az","12")'
  elseif r == 12 then return Cc(1), 'Cc(1)'
  elseif r == 13 then return Cc("x", 2), 'Cc("x",2)'
  elseif r == 14 then return Cp(), 'Cp()'
  elseif r == 15 then return Cc(), 'Cc()'
  elseif r == 16 then return Cc(nil), 'Cc(nil)'
  elseif r == 17 then return P"c", 'P"c"'
  elseif r == 18 then return P"b", 'P"b"'
  elseif r == 19 then return P"1" + P"2", 'P"1"+P"2"'
  elseif r == 20 then return P" ", 'P" "'
  elseif r == 21 then return Cb"n", 'Cb"n"'
  elseif r == 22 then return Carg(1), 'Carg(1)'
  elseif r == 23 and ingrammar and inand == 0 then local n = pick(names); return V(n), 'V"'..n..'"'
  else return P"d", 'P"d"' end
end

local gen
local inand = 0
function gen(depth, ingrammar)
  if depth <= 0 or rnd(6) == 1 then return leaf(ingrammar) end
  local r = rnd(22)
  local p, d = gen(depth - 1, ingrammar)
  if r <= 4 then
    local q, e = gen(depth - 1, ingrammar)
    return p * q, "(" .. d .. " * " .. e .. ")"
  elseif r <= 7 then
    local q, e = gen(depth - 1, ingrammar)
    return p + q, "(" .. d .. " + " .. e .. ")"
  elseif r == 8 then
    local q, e = gen(depth - 1, ingrammar)
    return p - q, "(" .. d .. " - " .. e .. ")"
  elseif r == 9 then local n = pick{0,1,2,-1,-2,3}; return p ^ n, "(" .. d .. " ^ " .. n .. ")"
  elseif r == 10 then return -p, "(-" .. d .. ")"
  elseif r == 11 then inand = inand + 1; local ok, q, e = pcall(gen, depth - 1, ingrammar); inand = inand - 1; if not ok then error(q, 0) end; return #q, "(#" .. e .. ")"
  elseif r == 12 then return C(p), "C(" .. d .. ")"
  elseif r == 13 then return Ct(p), "Ct(" .. d .. ")"
  elseif r == 14 then return Cs(p), "Cs(" .. d .. ")"
  elseif r == 15 then local g = rnd(3) == 1 and "n" or nil; return Cg(p, g), "Cg(" .. d .. "," .. tostring(g) .. ")"
  elseif r == 16 then local i = rnd(#capfuncs); return p / capfuncs[i], "(" .. d .. " / cf" .. i .. ")"
  elseif r == 17 then local i = rnd(#strs); return p / strs[i], "(" .. d .. " / " .. string.format("%q", strs[i]) .. ")"
  elseif r == 18 then local i = rnd(#tabs); return p / tabs[i], "(" .. d .. " / tab" .. i .. ")"
  elseif r == 19 then local n = pick{0,1,2,3}; return p / n, "(" .. d .. " / " .. n .. ")"
  elseif r == 20 then local i = rnd(#folds); return Cf(p, folds[i]), "Cf(" .. d .. ", fold" .. i .. ")"
  elseif r == 21 and inand > 0 then return p, d
  elseif r == 21 then local i = rnd(#cmts); return Cmt(p, cmts[i]), "Cmt(" .. d .. ", cmt" .. i .. ")"
  else return B(p), "B(" .. d .. ")" end
end

local function genpat()
  if rnd(5) == 1 then
    -- grammar
    local rules, ds = {}, {}
    local n = rnd(3)
    local ok, err = pcall(function()
      for i = 1, n do
        local p, d = gen(rnd(3), true)
        rules[names[i]] = p
        ds[#ds+1] = names[i] .. " = " .. d
      end
    end)
    if not ok then return nil, "ERRGEN " .. tostring(err) end
    rules[1] = rnd(4) == 1 and names[n] or "S"
    return P(rules), "P{" .. table.concat(ds, "; ") .. "}"
  end
  return gen(rnd(4), false)
end

local alphabet = {"a","b","c","d","1","2"," ","a","b","a"}
local function subject()
  local n = rnd(9) - 1
  local t = {}
  for i = 1, n do t[i] = pick(alphabet) end
  return table.concat(t)
end


-- Cases whose expected output is TeX Live 2026 LuaTeX's lpeg 1.0.1:
--   texlua lpeg_cases.lua > lpeg_cases.expected
local function strip(msg) return (tostring(msg):gsub("^[^:\n]*:%d+: ", "")) end
local function report(name, ok, ...)
  if ok then print(name, "ok", sers(...)) else print(name, "error", strip((...))) end
end
local function case(name, f, ...) report(name, pcall(f, ...)) end
local function m(p, s, ...) return lpeg.match(p, s, ...) end

-- pattern construction and coercion
case("P string", m, P"abc", "abcd")
case("P number", m, P(2), "abc")
case("P negative", m, P(-2), "a")
case("P true/false", function() return m(P(true), "x"), m(P(false), "x") end)
case("P function", m, P(function(s, i) return i + 1 end), "ab")
case("P identity", function() local p = P"a"; return P(p) == p, lpeg.type(p), lpeg.type("a"), lpeg.type(P"a") end)
case("seq identity", function() local p = P"a"; return (p * P(true)) == p, (P(true) * p) == p, (p + P(true)) == p end)
case("string operand", function() return m("a" * P"b", "ab"), m(P"a" * "b", "ab"), m(1 * P"b", "ab"), m(P"a" + "x", "x") end)
case("operators keep string metatable clean", function() return debug.getmetatable("").__mul, debug.getmetatable(0) end)
case("userdata", function() return type(P"a"), tostring(P"a"):gsub("0x%x+", "PTR") end)
case("pattern methods", function() local p = P"a"; return p.match == lpeg.match, p:match("ab"), getmetatable(p).__index == lpeg end)
case("S R sets", function() return m(S"ab"^1, "abbac"), m(R("az", "09")^1, "ab12-"), m(S""^0, "x"), m(R()^0, "x") end)
case("B behind", function() return m(P"ab" * B(P"b") * C(P(1)), "abc"), m(lpeg.Cp() * P"a" * B(P"a"), "ab") end)
case("B errors", function() return pcall(B, P"a"^0) end)
case("B captures", B, C(P"a"))
case("B long", B, P(300))
case("diff", function() return m(P(3) - P"ab", "abc"), m((R"az"^1) - P"abc", "abd"), m(R"ad" - P"b", "b") end)
case("and/not", function() return m(#P"a" * P(1), "ab"), m(-P"a" * P(1), "ab"), m(-P"a" * P(1), "bb") end)
case("rep counts", function() return m(P"a"^2, "aaa"), m(P"a"^2, "a"), m(P"a"^-2, "aaa"), m(P"a"^-2, "b"), m(P"a"^0, "") end)
case("rep nullable", function() return P(true)^0 end)
case("rep nullable nested", function() return (P"a"^0)^1 end)
case("init", function() return m(C(P(1)), "abc", 2), m(C(P(1)), "abc", -1), m(C(P(1)), "abc", -10), m(C(P(1)), "abc", 10), m(P"c", "abc", 3), m(Cp(), "abc", -2) end)
case("init string", m, C(1), "abc", "2")
case("init float", m, C(1), "abc", 1.5)
case("subject number", m, C(P(1)^1), 12345, 2)
case("subject table", m, P"a", {})
case("pattern from string", lpeg.match, "ab", "abc")
case("pattern nil", lpeg.match, nil, "abc")
case("empty captures", function() return m(Cc(), "a"), m(Cc(nil), "a"), m(Cc(nil, nil), "a"), m(Cc(1, nil, 3), "a") end)

-- captures
case("simple", function() return m(C(P"a" * C(P"b")) * C(P"c"), "abc") end)
case("position", function() return m(Cp() * P"ab" * Cp(), "abc") end)
case("Ct", function() return m(Ct(C(1) * C(1)), "ab") end)
case("Ct named", function() return m(Ct(Cg(C(1), "x") * Cg(C(1), "y") * C(1)), "abc") end)
case("Cg anonymous", function() return m(Cg(C(1) * C(1)) * C(1), "abc") end)
case("Cb", function() return m(Cg(C(1), "k") * C(1) * Cb"k", "abc") end)
case("Cb missing", m, Cb"zz", "a")
case("Cb latest", function() return m(Cg(C(1), "k") * Cg(C(1), "k") * Cb"k", "ab") end)
case("Cs", function() return m(Cs((P"a" / "A" + 1)^0), "banana"), m(Cs((C(P"a") / {a = "X"} + 1)^0), "banana") end)
case("Cs nested", function() return m(Cs(P"a" * Cs(P"b" / "B") * P"c"), "abc") end)
case("Cs errors", m, Cs(Ct(P"a")), "a")
case("Cs numbers", function() return m(Cs((P"a" / 1 + P"b" / function() return 2.5 end + 1)^0), "abc") end)
case("Cf", function() return m(Cf(C(1) * C(1)^0, function(a, b) return a .. "+" .. b end), "abcd") end)
case("Cf none", m, Cf(P"a", function() end), "a")
case("Cf table", function() return ser(m(Cf(Ct"" * Cg(C(R"az") * P"=" * C(R"09")^1 * P";"^-1), rawset), "a=1;b=22;c=3")) end)
case("div function", function() return m(C(1) * C(1) / function(a, b) return b, a end, "ab") end)
case("div function none", function() return m(C(1) / function() end, "a") end)
case("div string", function() return m(C(1) * C(1) / "<%2%1%0%%>", "ab"), m(C(1) / "%1%1", "a") end)
case("div string errors", function() return pcall(m, C(1) / "%2", "a") end)
case("div string trailing percent", function() return #m(C(1) / "[%", "a") end)
case("div table", function() return m(C(1) / {a = "A"}, "a"), m(C(1) / {a = "A"}, "b"), m(C(1) / setmetatable({}, {__index = function(_, k) return k .. k end}), "q") end)
case("div number", function() return m(C(1) * C(1) / 2, "ab"), m(C(1) / 0, "a"), m(P"a" / 0, "a") end)
case("div number errors", function() return pcall(m, C(1) / 3, "a") end)
case("div bad value", function() return P"a" / true end)
case("Carg", function() return m(Carg(1) * Carg(2), "a", 1, "x", "y") end)
case("Carg absent", m, Carg(2), "a", 1, "x")
case("Carg bad", Carg, 0)
case("Cmt basic", function() return m(Cmt(C(R"az"^1), function(s, i, c) return i, c:upper() end), "abc!") end)
case("Cmt fail", function() return m(Cmt(P"a", function() return false end) + C(1), "a") end)
case("Cmt true", function() return m(Cmt(P"a", function() return true end), "ab") end)
case("Cmt captures", function() return m(Cmt(C(1) * C(1), function(s, i, a, b) return true, b, a end), "xy") end)
case("Cmt no capture gets match", function() return m(Cmt(P"ab", function(s, i, c) return i, c end), "abc") end)
case("Cmt invalid position", m, Cmt(P"a", function() return 0 end), "a")
case("Cmt backtrack", function()
  local calls = 0
  local p = (Cmt(P"a", function(s, i) calls = calls + 1; return i end) * P"x") + C(P"a")
  return m(p, "ab"), calls
end)
case("Cmt nested", function() return m(Ct(Cmt(Cmt(C(1), function(s, i, c) return i, c .. c end), function(s, i, c) return i, c, c end)), "ab") end)
case("Cmt in Cs", function() return m(Cs(Cmt(P"a", function(s, i) return i, "<dyn>" end) * 1), "ab") end)
case("Cmt position beyond", function() return m(Cmt(P"a", function(s, i) return i + 1 end) * Cp(), "abc") end)
case("Cmt subject", function() return m(Cmt(P"b", function(s, i) return i, s, i end), "abc", 2) end)

-- grammars
local expr = P{"E"; E = V"T" * (S"+-" * V"T")^0, T = R"09"^1 + "(" * V"E" * ")"}
case("grammar", m, expr, "1+(2-3)+4")
case("grammar initial rule by name", function() return m(P{"B"; A = P"a", B = P"b" * V"A"}, "ba") end)
case("grammar initial rule as pattern", function() return m(P{P"a" * V"x"^-1; x = P"b"}, "abb") end)
case("grammar balanced", function() return m(P{"B"; B = "(" * (1 - S"()" + V"B")^0 * ")"}, "(a(b)c)d"), m(P{"B"; B = "(" * (1 - S"()" + V"B")^0 * ")"}, "(a(b c") end)
case("grammar captures", function() return ser(m(P{"L"; L = Ct(V"I" * ("," * V"I")^0), I = C(R"az"^1) + Ct(P"[" * V"L" * "]")}, "a,[b,c],d")) end)
case("left recursion", function() return P{"a"; a = V"a" * P"x"} end)
case("left recursion mutual", function() return P{"a"; a = V"b" * P"x", b = V"a" + P"y"} end)
case("left recursion nullable prefix", function() return P{"a"; a = P"x"^-1 * V"a"} end)
case("empty loop", function() return P{"a"; a = (V"b")^0, b = P"x"^-1} end)
case("undefined rule", function() return P{"a"; a = V"zz"} end)
case("V outside", m, V"a", "x")
case("V outside nested", m, P"a" * V"a", "x")
case("V nil", V, nil)
case("grammar empty", P, {})
case("grammar initial not pattern", P, {"a"; a = "x"})
case("grammar rule not pattern", P, {"a"; a = P"x", b = 3})
case("grammar unknown initial", P, {"b"; a = P"x"})
case("grammar rule calling itself", function() return m(P{"a"; a = P"x", b = V"b"}, "x") end)
case("grammar nested", function() return m(P{"x"; x = P{"y"; y = P"a" * V"y"^-1}}, "aa") end)
case("grammar rules by number", function() return m(P{P"a" * V(2); P"b"}, "ab") end)
case("grammar deep", function() return m(P{"S"; S = P"a" * V"S" * P"b" + P""}, ("a"):rep(150) .. ("b"):rep(150)) end)

-- the backtrack stack
case("stack overflow", function() return m(P{"S"; S = P"a" * V"S" * P"b" + P""}, ("a"):rep(250) .. ("b"):rep(250)) end)
case("setmaxstack", function()
  local g = P{"S"; S = P"a" * V"S" * P"b" + P""}
  local s = ("a"):rep(250) .. ("b"):rep(250)
  lpeg.setmaxstack(1000)
  local ok = m(g, s)
  lpeg.setmaxstack(400)
  return ok, pcall(m, g, s)
end)
case("setmaxstack range", lpeg.setmaxstack, 0)
case("setmaxstack type", lpeg.setmaxstack, "x")
case("long subject", function() return m(Cp(), ("x"):rep(100000), -1), #m(Cs((P"a" / "bb" + 1)^0), ("a"):rep(100000)), select("#", m(C(1)^0, ("x"):rep(3000))) end)
case("long repetition", function() return m(P"ab"^0 * Cp(), ("ab"):rep(100000)) end)

-- bytes
case("binary", function() return m(C(P"\255\0" * R"\128\255"), "\255\0\200"), m(Cs((P"\0" / "<NUL>" + 1)^0), "a\0b\255") end)
case("locale", function()
  local l = lpeg.locale()
  return m(l.alpha^1, "abc1"), m(l.digit^1, "123a"), m(l.space^1, " \t\n\r\f\v!"), m(l.punct^1, "!?a"), m(l.xdigit^1, "fFg"), m(l.upper^1, "ABc"), m(l.lower^1, "abC"), m(l.alnum^1, "a1_"), m(l.cntrl^1, "\0\1a"), m(l.graph^1, "ab c"), m(l.print^1, "ab c\1"), m(l.alpha^1, "\195\169")
end)
case("locale into table", function() local t = {}; return lpeg.locale(t) == t, lpeg.type(t.alpha) end)
case("locale bad", lpeg.locale, 3)
case("version", lpeg.version)
case("lpeg.type", function() return lpeg.type(P"a"), lpeg.type(3), lpeg.type(io.stdout), lpeg.type() end)
case("global and loaded", function() return package.loaded.lpeg == lpeg, require"lpeg" == lpeg, package.loaded.re end)
case("argument errors", function()
  return select(2, pcall(P)), select(2, pcall(P, nil)), select(2, pcall(S)), select(2, pcall(R, "a")), select(2, pcall(lpeg.Cf, P"a", 1)),
         select(2, pcall(lpeg.match, P"a")), select(2, pcall(lpeg.match, P"a", {}))
end)
case("operator errors", function()
  return select(2, pcall(function() return P"a" * nil end)), select(2, pcall(function() return P"a" ^ "x" end)),
         select(2, pcall(function() return P"a" ^ 1.5 end)), select(2, pcall(function() return P"a" / nil end))
end)
case("unsupported operators", function()
  return select(2, pcall(function() return P"a" .. P"b" end)), select(2, pcall(function() return P"a" < P"b" end)),
         select(2, pcall(function() return P"a"() end)), select(2, pcall(function() local p = P"a"; p.x = 1 end))
end)

-- fuzz corpus (deterministic generator, see fz.lua in the oracle harness)

local function fuzz(seed, count)
  state = seed * 7919 + 13
  for k = 1, count do
    local ok, p, d = pcall(genpat)
    if not ok then print(k, "GENERR", strip(p))
    elseif p == nil then print(k, d)
    else
      print(k, d)
      for _, init in ipairs{1, 2, -2} do
        for _ = 1, 3 do
          local s = subject()
          report("  " .. ser(s) .. " " .. init, pcall(function() return lpeg.match(p, s, init, "ARG") end))
        end
      end
    end
  end
end
fuzz(1, 120)
fuzz(2, 120)
