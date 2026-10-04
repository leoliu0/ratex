#!/usr/bin/env texlua
local ok, msg = pcall(function() error("probe") end)
return "shebang", msg
