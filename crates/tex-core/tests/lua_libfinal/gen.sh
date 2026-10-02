#!/bin/bash
# Regenerates NAME.expected from TeX Live's luatex:  gen.sh NAME [luatex options]
# The fixture runs under `luatex --ini` exactly as tests/lua_libfinal.rs runs it in
# the engine; P(...) lines are the expected output.
set -e
name=$1; shift
here=$(cd "$(dirname "$0")" && pwd)
d=$(mktemp -d)
cd "$d"
cp "$here/$name.lua" body.lua
cp "$here"/*.png . 2>/dev/null || true
cat > one.lua <<'LUA'
local out = {}
function P(...) local t = table.pack(...) for i = 1, t.n do t[i] = tostring(t[i]) end out[#out+1] = table.concat(t, ' | ') end
dofile("body.lua")
local f = io.open('probe.out', 'wb') f:write(table.concat(out, '\n')) f:close()
LUA
printf '\\catcode`\\{=1 \\catcode`\\}=2\n\\directlua{dofile("one.lua")}\n\\end\n' > one.tex
/usr/bin/luatex --ini "$@" -interaction=batchmode one.tex >/dev/null 2>&1 || true
cp probe.out "$here/$name.expected"
echo "wrote $name.expected"
