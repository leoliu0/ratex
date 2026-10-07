-- Helper functions used by main.tex through \directlua.
function fib(n)
  local a, b = 0, 1
  for _ = 1, n do a, b = b, a + b end
  return a
end
function primes(n)
  local out = {}
  for k = 2, n do
    local ok = true
    for d = 2, math.floor(math.sqrt(k)) do
      if math.fmod(k, d) == 0 then ok = false break end
    end
    if ok then table.insert(out, k) end
  end
  return table.concat(out, ", ")
end
