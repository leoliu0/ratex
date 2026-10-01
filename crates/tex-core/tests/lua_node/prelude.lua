local __buf = {}
function P(...)
  local t = {}
  for i = 1, select('#', ...) do t[i] = tostring((select(i, ...))) end
  __buf[#__buf + 1] = table.concat(t, "\t")
end
function PEND()
  local f = io.open(OUTFILE, "w")
  f:write(table.concat(__buf, "\n"), "\n")
  f:close()
end
