//! Reports the interactions of a script with the outside world.
//!
//! A host that caches the result of a run needs to know what the run depended
//! on. The standard libraries tell an installed observer about every file
//! they open, chunk they load, environment variable they read and command
//! they start. The observer is per thread, like the Lua state itself.

use std::cell::Cell;

/// One interaction with the host system.
#[derive(Clone, Copy, Debug)]
pub enum HostAccess<'a> {
    /// `io.open`/`io.lines`/`io.input`/`io.output` (and `LuaFile::open`);
    /// `mode` is the C `fopen` mode, `ok` whether the open succeeded.
    Open { path: &'a [u8], mode: &'a [u8], ok: bool },
    /// `loadfile`/`dofile`/`luaL_loadfilex` reading a chunk file.
    Chunk { path: &'a str, ok: bool },
    /// `package.searchpath` probing a file.
    Probe { path: &'a str, ok: bool },
    /// `os.getenv`.
    Getenv { name: &'a [u8] },
    /// `os.execute`, `io.popen`.
    Spawn,
    /// `os.remove`.
    Remove { path: &'a [u8] },
    /// `os.rename`.
    Rename { from: &'a [u8], to: &'a [u8] },
}

/// The observer function type.
pub type HostObserver = fn(&HostAccess<'_>);

thread_local! {
    static OBSERVER: Cell<Option<HostObserver>> = const { Cell::new(None) };
}

/// Install (or with `None` remove) the observer of this thread.
pub fn set_host_observer(observer: Option<HostObserver>) {
    OBSERVER.with(|cell| cell.set(observer));
}

#[inline]
pub(crate) fn notify(access: &HostAccess<'_>) {
    if let Some(observer) = OBSERVER.with(Cell::get) {
        observer(access);
    }
}
