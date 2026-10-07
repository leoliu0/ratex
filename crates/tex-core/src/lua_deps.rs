//! What a LuaTeX run depended on, for the result cache.
//!
//! The result cache replays a finished build when every input is unchanged.
//! TeX itself touches the outside world only through files and a few
//! environment variables the cache already tracks; a Lua script can read any
//! file, environment variable or directory and run any command. The standard
//! libraries report such interactions here (`tex_lua::HostAccess`) and the
//! `lfs`/`kpse` bridges call the methods below directly. Every interaction
//! is either recorded as a dependency of the build (files, missing files,
//! directory listings, existing directories, environment variables) or, when it cannot be
//! represented, clears `dependency_tracking_complete`, so the build is simply
//! not cached.
//!
//! Files below the font cache (`$TEXMFVAR`) are not dependencies: the font
//! database and font caches hold derived data that luaotfload validates for
//! itself, exactly as with TeX Live. The embedded runtime is part of the
//! executable, which the cache identity covers.

use std::path::Path;

use tex_lua::HostAccess;

use crate::engine::Engine;
use crate::lua_bridge::with_engine;

/// Install the observer of the standard libraries for this thread.
pub(crate) fn install_observer() {
    tex_lua::set_host_observer(Some(observe));
}

fn observe(access: &HostAccess<'_>) {
    let _ = with_engine(|engine| engine.lua_observe(access));
}

fn trace_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TEXRES_TRACE_LUA_DEPS").is_some())
}

/// True for files that are not dependencies (see the module documentation).
pub(crate) fn ignored(path: &Path) -> bool {
    let text = crate::lua_sys::kpse_path(path);
    if tex_kpse::embedded_tree::is_embedded_path(&text) {
        return true;
    }
    let cache = crate::lua_sys::kpse_path(&crate::lua_sys::font_cache_dir());
    text.strip_prefix(&cache).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

impl Engine {
    pub(crate) fn lua_observe(&mut self, access: &HostAccess<'_>) {
        if trace_enabled() {
            let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
            match *access {
                HostAccess::Open { path, mode, ok } => eprintln!("lua-dep: open {} {} ok={ok}", text(path), text(mode)),
                HostAccess::Chunk { path, ok } => eprintln!("lua-dep: chunk {path} ok={ok}"),
                HostAccess::Probe { path, ok } => eprintln!("lua-dep: probe {path} ok={ok}"),
                HostAccess::Getenv { name } => eprintln!("lua-dep: getenv {}", text(name)),
                HostAccess::Spawn => eprintln!("lua-dep: spawn"),
                HostAccess::Remove { path } => eprintln!("lua-dep: remove {}", text(path)),
                HostAccess::Rename { from, to } => eprintln!("lua-dep: rename {} {}", text(from), text(to)),
            }
        }
        match *access {
            HostAccess::Open { path, mode, ok } => {
                let path = crate::lua_sys::path_of(path);
                if mode.first() == Some(&b'r') && mode.get(1) != Some(&b'+') {
                    self.lua_dep_read(&path, ok);
                } else if ok && !ignored(&path) && !self.written_files.contains(&path) {
                    self.written_files.push(path);
                }
            }
            HostAccess::Chunk { path, ok } | HostAccess::Probe { path, ok } => {
                self.lua_dep_read(Path::new(path), ok);
            }
            HostAccess::Getenv { name } => self.lua_dep_environment(&String::from_utf8_lossy(name)),
            HostAccess::Spawn => self.lua_untracked("running a command"),
            HostAccess::Remove { path } => {
                if !ignored(&crate::lua_sys::path_of(path)) {
                    self.lua_untracked("os.remove");
                }
            }
            HostAccess::Rename { from, to } => {
                if !ignored(&crate::lua_sys::path_of(from)) || !ignored(&crate::lua_sys::path_of(to)) {
                    self.lua_untracked("os.rename");
                }
            }
        }
    }

    /// A script read the environment variable `name`.
    pub(crate) fn lua_dep_environment(&mut self, name: &str) {
        let known = &mut self.font_loader.dependency_environment;
        if !known.iter().any(|(known, _)| known == name) {
            known.push((name.to_string(), std::env::var_os(name)));
        }
    }

    /// A script read the file `path` (`present`), or found it missing.
    pub(crate) fn lua_dep_read(&mut self, path: &Path, present: bool) {
        if ignored(path) {
            return;
        }
        if present {
            if !self.loaded_files.iter().any(|known| known == path) {
                self.loaded_files.push(path.to_path_buf());
            }
        } else if !self.missing_files.iter().any(|known| known == path) {
            self.missing_files.push(path.to_path_buf());
        }
    }

    /// A script asked for the attributes of `path`: `kind` is the file type
    /// when it exists. Only the existence and contents matter to the cache;
    /// times and permissions are not dependencies.
    pub(crate) fn lua_dep_stat(&mut self, path: &Path, kind: Option<&str>) {
        if trace_enabled() {
            eprintln!("lua-dep: stat {} {kind:?}", path.display());
        }
        match kind {
            Some("file") => self.lua_dep_read(path, true),
            Some("directory") => {
                if !ignored(path) && !self.font_loader.dependency_present_directories.iter().any(|known| known == path) {
                    self.font_loader.dependency_present_directories.push(path.to_path_buf());
                }
            }
            Some(_) => {}
            None => {
                if !ignored(path) {
                    self.lua_dep_read(path, false);
                    self.lua_dep_missing_directory(path);
                }
            }
        }
    }

    /// A script listed the directory `path` (`present`) or found none.
    pub(crate) fn lua_dep_directory(&mut self, path: &Path, present: bool) {
        if trace_enabled() {
            eprintln!("lua-dep: dir {} {present}", path.display());
        }
        if ignored(path) {
            return;
        }
        if !present {
            self.lua_dep_missing_directory(path);
        } else if self.font_loader.dependency_directories.iter().any(|(known, _)| known == path) {
            // listed before in this run
        } else if let Some(fingerprint) = tex_kpse::directory_fingerprint(path) {
            self.font_loader.dependency_directories.push((path.to_path_buf(), fingerprint));
        } else {
            self.lua_untracked("directory listing");
        }
    }

    fn lua_dep_missing_directory(&mut self, path: &Path) {
        if !self.font_loader.dependency_missing_directories.iter().any(|known| known == path) {
            self.font_loader.dependency_missing_directories.push(path.to_path_buf());
        }
    }

    /// A script changed the file system at `path` (`what`).
    pub(crate) fn lua_dep_mutation(&mut self, path: &Path, what: &str) {
        if !ignored(path) {
            self.lua_untracked(what);
        }
    }

    /// A script searched for `name` with `kpse`. `format` is the file format
    /// searched, `found` the result.
    pub(crate) fn lua_dep_lookup(&mut self, name: &str, format: Option<tex_kpse::Format>, found: Option<&Path>) {
        if trace_enabled() {
            eprintln!("lua-dep: kpse {name} {format:?} -> {found:?}");
        }
        match (format, found) {
            (Some(format), Some(path)) if !ignored(path) => {
                self.font_loader.record_lookup_dependency(name, format, Some(path));
            }
            (Some(format), _) => self.font_loader.record_negative_dependency(name, format),
            (None, Some(path)) => self.lua_dep_read(path, true),
            (None, None) => self.lua_untracked("kpse lookup of an unknown file type"),
        }
    }

    /// The run did something the cache cannot represent: do not cache it.
    pub(crate) fn lua_untracked(&mut self, what: &str) {
        if trace_enabled() {
            eprintln!("lua-dep: not cacheable: {what}");
        }
        self.font_loader.dependency_tracking_complete = false;
    }
}
