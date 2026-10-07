//! LuaTeX's system-facing Lua libraries: `lfs`, `fio`/`sio`, `md5`, `sha2`,
//! `zlib`, `gzip`, `zip`, `unicode`, `ltn12`, `mime`, `texconfig`, `status`,
//! `kpse`, the LuaTeX extensions of `os`/`io`, and the `lua` table.
//!
//! Each library is a set of native primitives (registered in the private
//! table `__texres_sys`) and a Lua prelude that assembles the public library
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

/// A path as Kpathsea hands it to Lua: on Windows `kpathsea_normalize_path`
/// turns every `\\` into `/` (and the `\\?\` of a canonical path is not
/// shown). Lua source that embeds such a path, or splits it at `/`
/// (`lfs.mkdirp`), then works the same on every platform.
pub(crate) fn kpse_path(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        normalize_windows_path(&text)
    }
    #[cfg(not(windows))]
    {
        text
    }
}

#[cfg(any(windows, test))]
fn normalize_windows_path(text: &str) -> String {
    let text = match text.strip_prefix(r"\\?\UNC\") {
        Some(unc) => format!(r"\\{unc}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(text).to_string(),
    };
    text.replace('\\', "/")
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

/// `errno` of an I/O error (0 when the error did not come from the OS). On
/// Windows the value of the C runtime's `errno` for the failure (`_dosmaperr`
/// of the Win32 code), as LuaTeX's libraries report it.
pub(crate) fn errno_of(error: &io::Error) -> i64 {
    #[cfg(windows)]
    {
        i64::from(crt_errno(error))
    }
    #[cfg(not(windows))]
    {
        i64::from(error.raw_os_error().unwrap_or(0))
    }
}

#[cfg(windows)]
fn crt_errno(error: &io::Error) -> i32 {
    match error.raw_os_error() {
        Some(code) => crate::lua_sys_crt::errno_from_win32(code as u32),
        None => crate::lua_sys_crt::errno_from_kind(error.kind()),
    }
}

/// `strerror(errno)`: Rust appends " (os error N)" to OS errors; C does not.
/// On Windows the text is the C runtime's one for the mapped `errno`, not the
/// Win32 message.
pub(crate) fn strerror(error: &io::Error) -> String {
    #[cfg(windows)]
    {
        let errno = crt_errno(error);
        if errno != 0 {
            return crate::lua_sys_crt::strerror(errno).to_string();
        }
    }
    let text = error.to_string();
    match text.find(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

/// `strerror` of a C `errno` value (not a Win32 code).
pub(crate) fn strerror_no(errno: i32) -> String {
    #[cfg(windows)]
    {
        crate::lua_sys_crt::strerror(errno).to_string()
    }
    #[cfg(not(windows))]
    {
        strerror(&io::Error::from_raw_os_error(errno))
    }
}

/// How much of the system `\write18` and the Lua libraries may reach, as set
/// by `-shell-escape`/`-no-shell-escape`/`-shell-restricted` and
/// `shell_escape=p` (`shellenabledp`/`restrictedshell`). Like TeX Live's
/// `texmf.cnf`, the default is `Restricted`.
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
    static SHELL: std::cell::Cell<ShellEscape> = const { std::cell::Cell::new(ShellEscape::Restricted) };
    static SAFER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static CACHE_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Use `dir` as the per-user cache root (`-cache-directory`) instead of
/// `$TEX_RS_CACHE_DIR` or the platform's user cache directory. The Lua
/// font loader keeps its name database and font caches below it
/// (`$TEXMFVAR` is `<root>/texmf-var`).
pub fn set_cache_dir(dir: PathBuf) {
    CACHE_DIR.with(|d| *d.borrow_mut() = Some(dir));
}

pub(crate) fn cache_dir() -> PathBuf {
    CACHE_DIR
        .with(|d| d.borrow().clone())
        .unwrap_or_else(tex_kpse::platform_cache_dir)
}

/// Set the shell-escape policy of `\write18`, `\pdfshellescape`, Lua's `os.execute`/`os.exec`/`os.spawn`/
/// `io.popen`, `status.shell_escape` and `kpse.check_permission`.
pub fn set_shell_escape(mode: ShellEscape) {
    SHELL.with(|s| s.set(mode));
}

pub(crate) fn shell_escape() -> ShellEscape {
    SHELL.with(|s| s.get())
}

/// `\pdfshellescape`/`\shellescape` and `status.shell_escape`: 0 disabled,
/// 1 enabled, 2 restricted.
pub(crate) fn shell_escape_status() -> i32 {
    match shell_escape() {
        ShellEscape::Disabled => 0,
        ShellEscape::Enabled => 1,
        ShellEscape::Restricted => 2,
    }
}

/// LuaTeX's `--safer` option (`status.safer_option`).
pub fn set_safer_option(safer: bool) {
    SAFER.with(|s| s.set(safer));
}

pub(crate) fn safer_option() -> bool {
    SAFER.with(|s| s.get())
}

/// A userdata standing for a native resource slot (a directory iterator, a
/// zlib stream): `h.id` is the slot, `nil` once the resource is closed. The
/// Lua side attaches the type's metatable, as LuaTeX's C libraries do for
/// their `lua_newuserdata` objects.
struct HandleUd(Option<i64>);

impl tex_lua::UserDataTrait for HandleUd {
    fn type_name(&self) -> &'static str {
        "userdata"
    }

    fn get_field(&self, key: &str) -> Option<tex_lua::UdValue> {
        (key == "id").then(|| self.0.map_or(tex_lua::UdValue::Nil, tex_lua::UdValue::Integer))
    }

    fn set_field(&mut self, key: &str, value: tex_lua::UdValue) -> Option<Result<(), String>> {
        if key != "id" {
            return None;
        }
        Some(match value {
            tex_lua::UdValue::Nil => {
                self.0 = None;
                Ok(())
            }
            tex_lua::UdValue::Integer(i) => {
                self.0 = Some(i);
                Ok(())
            }
            _ => Err("invalid resource slot".to_string()),
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

fn register_handles(lua: &mut Lua, sys: &LuaTable) -> Result<(), String> {
    let ud_new = lua
        .create_callback(|cb| {
            let id: i64 = cb.arg(1)?;
            let meta: LuaTable = cb.arg(2)?;
            let ud = cb.create_userdata(HandleUd(Some(id)))?;
            let value = cb.pack(&ud)?;
            value.set_metatable(Some(&meta))?;
            cb.push(value)
        })
        .map_err(|e| format!("{e:?}"))?;
    sys.set("ud_new", ud_new).map_err(|e| format!("{e:?}"))?;
    // the address `tostring` shows for a userdata (`%p`)
    let address = lua
        .create_callback(|cb| {
            let value: tex_lua::Value = cb.arg(1)?;
            let text = value.to_pointer().map_or_else(String::new, |p| format!("{:#x}", p as usize));
            cb.push(text)
        })
        .map_err(|e| format!("{e:?}"))?;
    sys.set("address", address).map_err(|e| format!("{e:?}"))
}

/// Install every system library into a Lua state whose standard libraries
/// and TeX bridge are open.
pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let sys: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;
    register_handles(lua, &sys)?;
    crate::lua_sys_embedded::register(lua, &sys)?;
    crate::lua_sys_lfs::register(lua, &sys)?;
    crate::lua_sys_hash::register(lua, &sys)?;
    crate::lua_sys_zlib::register(lua, &sys)?;
    crate::lua_sys_unicode::register(lua, &sys)?;
    crate::lua_sys_mime::register(lua, &sys)?;
    crate::lua_sys_status::register(lua, &sys)?;
    crate::lua_sys_kpse::register(lua, &sys)?;
    crate::lua_sys_os::register(lua, &sys)?;
    lua.set_global("__texres_sys", sys).map_err(|e| format!("{e:?}"))?;
    for (name, code) in [
        ("embedded", crate::lua_sys_embedded::PRELUDE),
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
            .set_name(&format!("=[texres {name}]"))
            .exec()
            .map_err(|e| format!("lua_sys {name} prelude: {}", lua.get_error_message(e).message()))?;
    }
    lua.execute("__texres_sys = nil").map_err(|e| format!("{e:?}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalize_windows_path;

    #[test]
    fn windows_paths_reach_lua_with_forward_slashes() {
        assert_eq!(normalize_windows_path(r"C:\Users\RUNNER~1\cache\texmf-var"), "C:/Users/RUNNER~1/cache/texmf-var");
        assert_eq!(normalize_windows_path(r"\\?\C:\a\b/c"), "C:/a/b/c");
        assert_eq!(normalize_windows_path(r"\\?\UNC\host\share\x"), "//host/share/x");
    }
}
