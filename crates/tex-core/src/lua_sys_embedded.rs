//! Reading the embedded archive's virtual tree (`/<embedded>/…`, see
//! `tex_kpse::embedded_tree`) from Lua: the natives behind the `io.open`,
//! `io.lines`, `loadfile` and `dofile` wrappers of `lua_sys_embedded.lua`.

use tex_lua::{Lua, LuaApi, LuaBytes, LuaString};

use crate::lua_sys::{bytes_of, sys_reg};

pub(crate) const PRELUDE: &str = include_str!("lua_sys_embedded.lua");

pub(crate) fn register(lua: &mut Lua, s: &tex_lua::LuaTable) -> Result<(), String> {
    // Whether the name lies in the virtual tree (whether or not it exists).
    sys_reg!(lua, s, "embedded_is_path", |path: LuaString| -> bool {
        std::str::from_utf8(&bytes_of(&path)).is_ok_and(tex_kpse::embedded_tree::is_embedded_path)
    });
    // Contents of a virtual file; nothing for directories and missing names.
    sys_reg!(lua, s, "embedded_read", |path: LuaString| -> Option<LuaBytes> {
        let bytes = bytes_of(&path);
        let text = std::str::from_utf8(&bytes).ok()?;
        tex_kpse::embedded_tree::read(text).map(LuaBytes)
    });
    Ok(())
}
