//! `lfs`: LuaFileSystem 1.7.0 as shipped in LuaTeX (`attributes`, `chdir`,
//! `currentdir`, `dir`, `link`, `lock`, `lock_dir`, `mkdir`, `rmdir`,
//! `setmode`, `symlinkattributes`, `touch`, `unlock`).

use std::cell::RefCell;
use std::fs;
use std::io;

use tex_lua::{Lua, LuaApi, LuaBytes, LuaFile, LuaString, Value};

use crate::lua_sys::{bytes_of, errno_of, os_bytes, path_bytes, path_of, strerror, strerror_no, sys_reg};

/// Errno values shared by Linux, macOS and the Windows CRT (`libc` lacks them on wasm32).
const ENOENT: i32 = 2;
const EROFS: i32 = 30;

pub(crate) const PRELUDE: &str = include_str!("lua_sys_lfs.lua");

thread_local! {
    /// Open `lfs.dir` iterators; `None` once closed.
    static DIRS: RefCell<Vec<Option<DirState>>> = const { RefCell::new(Vec::new()) };
}

struct DirState {
    /// `.` and `..` come first as `readdir` reports them; then the entries.
    dots: u8,
    entries: Box<dyn Iterator<Item = Vec<u8>>>,
}

/// The embedded archive's virtual tree (`/<embedded>/…`), if `bytes` names a
/// path in it.
fn embedded_path(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes).ok().filter(|text| tex_kpse::embedded_tree::is_embedded_path(text))
}

/// `lfs.attributes` of an embedded path: read-only, with the archive's
/// pinned modification time.
fn embedded_stat(path: &str) -> Option<(&'static str, &'static str, Vec<i64>)> {
    use tex_kpse::embedded_tree::{stat, EmbeddedKind, MTIME};
    let (mode, permissions, size) = match stat(path)? {
        EmbeddedKind::File { size } => ("file", "r--r--r--", size as i64),
        EmbeddedKind::Directory => ("directory", "r-xr-xr-x", 0),
    };
    let nlink = if mode == "directory" { 2 } else { 1 };
    Some((mode, permissions, vec![0, 0, nlink, 0, 0, 0, MTIME, MTIME, MTIME, size, (size + 511) / 512, 4096]))
}

type Tri<T> = (Option<T>, Option<LuaBytes>, Option<i64>);

fn failure<T>(error: &io::Error, info: Option<String>) -> Tri<T> {
    let message = match info {
        Some(info) => format!("{info}: {}", strerror(error)),
        None => strerror(error),
    };
    (None, Some(LuaBytes(message.into_bytes())), Some(errno_of(error)))
}

/// Failing with the C `errno` value `errno` (not a Win32 code).
fn failure_errno<T>(errno: i32) -> Tri<T> {
    (None, Some(LuaBytes(strerror_no(errno).into_bytes())), Some(i64::from(errno)))
}

fn done(result: io::Result<()>) -> Tri<bool> {
    match result {
        Ok(()) => (Some(true), None, None),
        Err(e) => failure(&e, None),
    }
}

fn mode_name(file_type: fs::FileType) -> &'static str {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if file_type.is_file() {
            "file"
        } else if file_type.is_dir() {
            "directory"
        } else if file_type.is_symlink() {
            "link"
        } else if file_type.is_socket() {
            "socket"
        } else if file_type.is_fifo() {
            "named pipe"
        } else if file_type.is_char_device() {
            "char device"
        } else if file_type.is_block_device() {
            "block device"
        } else {
            "other"
        }
    }
    #[cfg(not(unix))]
    {
        if file_type.is_file() {
            "file"
        } else if file_type.is_dir() {
            "directory"
        } else if file_type.is_symlink() {
            "link"
        } else {
            "other"
        }
    }
}

/// `rwxrwxrwx` for the permission bits of `mode`.
fn perm_string(mode: u32) -> Vec<u8> {
    const BITS: [(u32, u8); 9] = [
        (0o400, b'r'),
        (0o200, b'w'),
        (0o100, b'x'),
        (0o040, b'r'),
        (0o020, b'w'),
        (0o010, b'x'),
        (0o004, b'r'),
        (0o002, b'w'),
        (0o001, b'x'),
    ];
    BITS.iter().map(|&(bit, ch)| if mode & bit != 0 { ch } else { b'-' }).collect()
}

#[cfg(unix)]
fn stat_numbers(meta: &fs::Metadata) -> Vec<i64> {
    use std::os::unix::fs::MetadataExt;
    vec![
        meta.dev() as i64,
        meta.ino() as i64,
        meta.nlink() as i64,
        i64::from(meta.uid()),
        i64::from(meta.gid()),
        meta.rdev() as i64,
        meta.atime(),
        meta.mtime(),
        meta.ctime(),
        meta.size() as i64,
        meta.blocks() as i64,
        meta.blksize() as i64,
    ]
}

#[cfg(not(unix))]
fn stat_numbers(meta: &fs::Metadata) -> Vec<i64> {
    let secs = |t: io::Result<std::time::SystemTime>| {
        t.ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64)
    };
    vec![
        0,
        0,
        1,
        0,
        0,
        0,
        secs(meta.accessed()),
        secs(meta.modified()),
        secs(meta.modified()),
        meta.len() as i64,
        0,
        0,
    ]
}

fn permission_bits(meta: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode()
    }
    #[cfg(not(unix))]
    {
        if meta.permissions().readonly() {
            0o555
        } else {
            0o777
        }
    }
}

pub(crate) fn register(lua: &mut Lua, s: &tex_lua::LuaTable) -> Result<(), String> {
    // (mode, permissions-or-message, numbers...|errno)
    sys_reg!(lua, s, "lfs_stat", |path: LuaString, follow: bool| -> (Option<LuaBytes>, LuaBytes, Vec<i64>) {
        let bytes = bytes_of(&path);
        let p = path_of(&bytes);
        if let Some(text) = embedded_path(&bytes) {
            return match embedded_stat(text) {
                Some((mode, permissions, numbers)) => {
                    (Some(LuaBytes(mode.as_bytes().to_vec())), LuaBytes(permissions.as_bytes().to_vec()), numbers)
                }
                None => {
                    let mut message = b"cannot obtain information from file '".to_vec();
                    message.extend_from_slice(&bytes);
                    message.extend_from_slice(format!("': {}", strerror_no(ENOENT)).as_bytes());
                    (None, LuaBytes(message), vec![i64::from(ENOENT)])
                }
            };
        }
        let meta = if follow { fs::metadata(&p) } else { fs::symlink_metadata(&p) };
        let kind = meta.as_ref().ok().map(|meta| mode_name(meta.file_type()));
        let _ = crate::lua_bridge::with_engine(|e| e.lua_dep_stat(&p, kind));
        match meta {
            Ok(meta) => (
                Some(LuaBytes(mode_name(meta.file_type()).as_bytes().to_vec())),
                LuaBytes(perm_string(permission_bits(&meta))),
                stat_numbers(&meta),
            ),
            Err(e) => {
                let mut message = b"cannot obtain information from file '".to_vec();
                message.extend_from_slice(&bytes);
                message.extend_from_slice(format!("': {}", strerror(&e)).as_bytes());
                (None, LuaBytes(message), vec![errno_of(&e)])
            }
        }
    });
    sys_reg!(lua, s, "lfs_readlink", |path: LuaString| -> Tri<LuaBytes> {
        let p = path_of(&bytes_of(&path));
        let result = fs::read_link(&p);
        let _ = crate::lua_bridge::with_engine(|e| e.lua_dep_link(&p, result.as_deref().ok()));
        match result {
            Ok(target) => (Some(LuaBytes(path_bytes(&target))), None, None),
            Err(e) => failure(&e, None),
        }
    });
    sys_reg!(lua, s, "lfs_mkdir", |path: LuaString| -> Tri<bool> {
        let bytes = bytes_of(&path);
        if embedded_path(&bytes).is_some() {
            return failure_errno(EROFS);
        }
        let p = path_of(&bytes);
        let _ = crate::lua_bridge::with_engine(|e| e.lua_dep_mutation(&p, "lfs.mkdir"));
        #[cfg(unix)]
        let result = {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o775).create(&p)
        };
        #[cfg(not(unix))]
        let result = fs::create_dir(&p);
        done(result)
    });
    sys_reg!(lua, s, "lfs_rmdir", |path: LuaString| -> Tri<bool> {
        let p = path_of(&bytes_of(&path));
        let _ = crate::lua_bridge::with_engine(|e| e.lua_dep_mutation(&p, "lfs.rmdir"));
        done(fs::remove_dir(p))
    });
    sys_reg!(lua, s, "lfs_chdir", |path: LuaString| -> (bool, Option<LuaBytes>) {
        let bytes = bytes_of(&path);
        let _ = crate::lua_bridge::with_engine(|e| e.lua_untracked("lfs.chdir"));
        match std::env::set_current_dir(path_of(&bytes)) {
            Ok(()) => (true, None),
            Err(e) => {
                let mut message = b"Unable to change working directory to '".to_vec();
                message.extend_from_slice(&bytes);
                message.extend_from_slice(format!("'\n{}\n", strerror(&e)).as_bytes());
                (false, Some(LuaBytes(message)))
            }
        }
    });
    sys_reg!(lua, s, "lfs_currentdir", |_unused: Option<bool>| -> Tri<LuaBytes> {
        match std::env::current_dir() {
            Ok(dir) => (Some(LuaBytes(path_bytes(&dir))), None, None),
            Err(e) => failure(&e, None),
        }
    });
    sys_reg!(lua, s, "lfs_link", |old: LuaString, new: LuaString, symbolic: bool| -> Tri<i64> {
        let (old, new) = (path_of(&bytes_of(&old)), path_of(&bytes_of(&new)));
        let _ = crate::lua_bridge::with_engine(|e| e.lua_dep_mutation(&new, "lfs.link"));
        #[cfg(unix)]
        let result = if symbolic { std::os::unix::fs::symlink(&old, &new) } else { fs::hard_link(&old, &new) };
        #[cfg(not(unix))]
        let result: io::Result<()> = {
            let _ = (&old, &new, symbolic);
            Err(io::Error::other("Function not implemented"))
        };
        match result {
            Ok(()) => (Some(0), None, None),
            Err(e) => failure(&e, None),
        }
    });
    sys_reg!(lua, s, "lfs_touch", |path: LuaString, times: bool, atime: f64, mtime: i64| -> Tri<bool> {
        let p = path_of(&bytes_of(&path));
        let _ = crate::lua_bridge::with_engine(|e| e.lua_dep_mutation(&p, "lfs.touch"));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else {
                return failure(&io::Error::from_raw_os_error(libc::EINVAL), None);
            };
            let buf = libc::utimbuf { actime: atime as libc::time_t, modtime: mtime as libc::time_t };
            let rc = unsafe { libc::utime(c.as_ptr(), if times { &buf } else { std::ptr::null() }) };
            if rc == -1 {
                failure(&io::Error::last_os_error(), None)
            } else {
                (Some(true), None, None)
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (times, atime, mtime);
            let when = std::time::SystemTime::now();
            done(fs::OpenOptions::new().write(true).open(&p).and_then(|f| f.set_modified(when)))
        }
    });
    sys_reg!(lua, s, "lfs_dir_open", |path: LuaString| -> Result<i64, String> {
        let bytes = bytes_of(&path);
        if let Some(text) = embedded_path(&bytes) {
            return match tex_kpse::embedded_tree::read_dir(text) {
                Some(entries) => Ok(DIRS.with(|d| {
                    let mut d = d.borrow_mut();
                    let entries = entries.into_iter().map(|(name, _)| name.into_bytes());
                    d.push(Some(DirState { dots: 0, entries: Box::new(entries) }));
                    d.len() as i64 - 1
                })),
                None => Err(format!("cannot open {}: {}", text, strerror_no(ENOENT))),
            };
        }
        let listing = fs::read_dir(path_of(&bytes));
        let _ = crate::lua_bridge::with_engine(|e| e.lua_dep_directory(&path_of(&bytes), listing.is_ok()));
        match listing {
            Ok(entries) => Ok(DIRS.with(|d| {
                let mut d = d.borrow_mut();
                let entries = entries.flatten().map(|entry| os_bytes(&entry.file_name()));
                d.push(Some(DirState { dots: 0, entries: Box::new(entries) }));
                d.len() as i64 - 1
            })),
            Err(e) => Err(format!("cannot open {}: {}", String::from_utf8_lossy(&bytes), strerror(&e))),
        }
    });
    // `Ok(None)`: end of directory (closes it). `Err`: already closed.
    sys_reg!(lua, s, "lfs_dir_next", |id: i64| -> Result<Option<LuaBytes>, String> {
        DIRS.with(|d| {
            let mut d = d.borrow_mut();
            let slot = d.get_mut(id as usize).ok_or("bad argument #1 to 'it' (closed directory)")?;
            let Some(state) = slot.as_mut() else {
                return Err("bad argument #1 to 'it' (closed directory)".to_string());
            };
            if state.dots < 2 {
                state.dots += 1;
                return Ok(Some(LuaBytes(if state.dots == 1 { b".".to_vec() } else { b"..".to_vec() })));
            }
            if let Some(name) = state.entries.next() {
                return Ok(Some(LuaBytes(name)));
            }
            *slot = None;
            Ok(None)
        })
    });
    sys_reg!(lua, s, "lfs_dir_close", |id: i64| {
        DIRS.with(|d| {
            if let Some(slot) = d.borrow_mut().get_mut(id as usize) {
                *slot = None;
            }
        });
    });
    sys_reg!(lua, s, "lfs_lock_dir", |path: LuaString| -> Tri<LuaBytes> {
        let mut link = bytes_of(&path);
        link.extend_from_slice(b"/lockfile.lfs");
        let _ = crate::lua_bridge::with_engine(|e| e.lua_dep_mutation(&path_of(&link), "lfs.lock_dir"));
        #[cfg(unix)]
        let result = std::os::unix::fs::symlink("lock", path_of(&link));
        #[cfg(not(unix))]
        let result: io::Result<()> = match fs::OpenOptions::new().write(true).create_new(true).open(path_of(&link)) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        };
        match result {
            Ok(()) => (Some(LuaBytes(link)), None, None),
            Err(e) => (None, Some(LuaBytes(strerror(&e).into_bytes())), None),
        }
    });
    // fcntl(F_SETLK) on the descriptor of an io.open handle: (ok, message)
    sys_reg!(lua, s, "lfs_flock", |file: Value, mode: LuaString, start: i64, len: i64| -> (bool, Option<LuaBytes>) {
        let Some(handle) = file.as_userdata::<LuaFile>() else {
            return (false, Some(LuaBytes(b"Bad file descriptor".to_vec())));
        };
        let fd = handle.borrow().ok().and_then(|f| f.raw_fd());
        #[cfg(unix)]
        {
            let Some(fd) = fd else {
                return (false, Some(LuaBytes(b"Bad file descriptor".to_vec())));
            };
            let mut lock: libc::flock = unsafe { std::mem::zeroed() };
            lock.l_type = match bytes_of(&mode).first() {
                Some(b'w') => libc::F_WRLCK as _,
                Some(b'r') => libc::F_RDLCK as _,
                _ => libc::F_UNLCK as _,
            };
            lock.l_whence = libc::SEEK_SET as _;
            lock.l_start = start as _;
            lock.l_len = len as _;
            if unsafe { libc::fcntl(fd, libc::F_SETLK, &lock) } == -1 {
                (false, Some(LuaBytes(strerror(&io::Error::last_os_error()).into_bytes())))
            } else {
                (true, None)
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (fd, mode, start, len);
            (false, Some(LuaBytes(b"Function not implemented".to_vec())))
        }
    });
    sys_reg!(lua, s, "lfs_unlock_dir", |link: LuaString| {
        let _ = fs::remove_file(path_of(&bytes_of(&link)));
    });
    Ok(())
}
