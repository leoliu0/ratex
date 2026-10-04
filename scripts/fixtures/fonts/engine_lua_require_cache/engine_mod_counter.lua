-- Module returning a table; records how often it ran and the name it was required under.
engine_counter_loads = (engine_counter_loads or 0) + 1
local M = {
  loads = engine_counter_loads,
  name = (...),
  n = 0,
}
function M.bump()
  M.n = M.n + 1
  return M.n
end
return M
