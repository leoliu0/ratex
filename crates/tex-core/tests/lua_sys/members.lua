lfs.mkdir("members") lfs.chdir("members")
for _, name in ipairs{"lfs","fio","sio","md5","sha2","zlib","gzip","zip","unicode","unicode.utf8","unicode.ascii","unicode.latin1","unicode.grapheme","ltn12","mime","texconfig","status","kpse","os","io","lua","string"} do
  local t = package.loaded[name] or _G[name]
  if name:find("%.") then local a,b = name:match("(.-)%.(.*)"); t = _G[a][b] end
  local k = {}
  for a, b in pairs(t or {}) do k[#k+1] = tostring(a)..":"..type(b) end
  table.sort(k)
  P(name .. ": " .. table.concat(k, " "))
end
local f=io.open("oracle.dump","wb") f:write(table.concat(out,"\n")) f:close()
