function P(...)
  local t = {}
  for i = 1, select('#', ...) do t[i] = tostring((select(i, ...))) end
  local f = io.open(OUTFILE, "a")
  f:write(table.concat(t, "\t"), "\n")
  f:close()
end
function PEND() end
