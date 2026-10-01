//! LuaTeX's system-facing Lua libraries: `lfs`, `fio`/`sio`, `md5`, `sha2`,
//! `zlib`, `gzip`, `zip`, `unicode`, `ltn12`, `mime`, `texconfig`, `status`,
//! `kpse`, the LuaTeX extensions of `os`/`io`, and the `lua` table.
//!
//! Each library is a set of native primitives (registered in the private
//! table `__ratex_sys`) and a Lua prelude that assembles the public library
//! table from them, exactly as the member lists of TeX Live 2026's
//! `luatex` export them. Native primitives never see `nil`/`false` result
//! conventions of Lua: they return plain values and the prelude shapes the
//! `nil, message, errno` triple that LuaTeX's libraries use.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;

use tex_lua::{Lua, LuaApi, LuaString, LuaTable};

/// Register a native primitive in a table.
macro_rules! sys_reg {
    ($lua:expr, $tbl:expr, $name:literal, $f:expr) => {
        $tbl.set($name, $lua.create_function($f).map_err(|e| format!("{}: {e:?}", $name))?)
            .map_err(|e| format!("{}: {e:?}", $name))?
    };
}
pub(crate) use sys_reg;

/// The bytes of a Lua string as a file name.
pub(crate) fn os_str(bytes: &[u8]) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(bytes).to_os_string()
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

pub(crate) fn path_of(bytes: &[u8]) -> PathBuf {
    PathBuf::from(os_str(bytes))
}

/// File name bytes of a path (a Lua string).
pub(crate) fn path_bytes(path: &std::path::Path) -> Vec<u8> {
    os_bytes(path.as_os_str())
}

pub(crate) fn os_bytes(s: &OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        s.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        s.to_string_lossy().into_owned().into_bytes()
    }
}

pub(crate) fn bytes_of(s: &LuaString) -> Vec<u8> {
    s.as_bytes().map(|b| b.to_vec()).unwrap_or_default()
}

/// `errno` of an I/O error (0 when the error did not come from the OS).
pub(crate) fn errno_of(error: &io::Error) -> i64 {
    i64::from(error.raw_os_error().unwrap_or(0))
}

/// `strerror(errno)`: Rust appends " (os error N)" to OS errors; C does not.
pub(crate) fn strerror(error: &io::Error) -> String {
    let text = error.to_string();
    match text.find(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

pub(crate) fn strerror_no(errno: i32) -> String {
    strerror(&io::Error::from_raw_os_error(errno))
}

/// How much of the system the Lua libraries may reach, as set by
/// `--shell-escape`/`--no-shell-escape`/`shell_escape=p` in LuaTeX
/// (`shellenabledp`/`restrictedshell`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellEscape {
    /// `status.shell_escape == 0`: no command execution at all.
    Disabled,
    /// `status.shell_escape == 2`: only the `shell_escape_commands` list.
    Restricted,
    /// `status.shell_escape == 1`: any command.
    Enabled,
}

thread_local! {
    static SHELL: std::cell::Cell<ShellEscape> = const { std::cell::Cell::new(ShellEscape::Disabled) };
    static SAFER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Set the shell-escape policy of Lua's `os.execute`/`os.exec`/`os.spawn`/
/// `io.popen`, `status.shell_escape` and `kpse.check_permission`.
pub fn set_shell_escape(mode: ShellEscape) {
    SHELL.with(|s| s.set(mode));
}

pub(crate) fn shell_escape() -> ShellEscape {
    SHELL.with(|s| s.get())
}

/// LuaTeX's `--safer` option (`status.safer_option`).
pub fn set_safer_option(safer: bool) {
    SAFER.with(|s| s.set(safer));
}

pub(crate) fn safer_option() -> bool {
    SAFER.with(|s| s.get())
}

/// Install every system library into a Lua state whose standard libraries
/// and TeX bridge are open.
pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let sys: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;
    crate::lua_sys_lfs::register(lua, &sys)?;
    crate::lua_sys_hash::register(lua, &sys)?;
    crate::lua_sys_zlib::register(lua, &sys)?;
    crate::lua_sys_unicode::register(lua, &sys)?;
    crate::lua_sys_mime::register(lua, &sys)?;
    crate::lua_sys_status::register(lua, &sys)?;
    crate::lua_sys_kpse::register(lua, &sys)?;
    crate::lua_sys_os::register(lua, &sys)?;
    lua.set_global("__ratex_sys", sys).map_err(|e| format!("{e:?}"))?;
    for (name, code) in [
        ("lfs", crate::lua_sys_lfs::PRELUDE),
        ("fio", crate::lua_sys_fio::PRELUDE),
        ("hash", crate::lua_sys_hash::PRELUDE),
        ("zlib", crate::lua_sys_zlib::PRELUDE),
        ("unicode", crate::lua_sys_unicode::PRELUDE),
        ("mime", crate::lua_sys_mime::PRELUDE),
        ("status", crate::lua_sys_status::PRELUDE),
        ("kpse", crate::lua_sys_kpse::PRELUDE),
        ("os", crate::lua_sys_os::PRELUDE),
    ] {
        lua.load(code)
            .set_name(&format!("=[ratex {name}]"))
            .exec()
            .map_err(|e| format!("lua_sys {name} prelude: {}", lua.get_error_message(e).message()))?;
    }
    lua.execute("__ratex_sys = nil").map_err(|e| format!("{e:?}"))?;
    Ok(())
}
