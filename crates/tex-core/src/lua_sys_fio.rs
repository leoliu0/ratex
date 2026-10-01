//! `fio` (binary readers on `io.open` handles) and `sio` (the same on
//! strings). They are written against the handle's `read`/`seek` methods, so
//! they work on every kind of file handle `io.open` can return.

pub(crate) const PRELUDE: &str = include_str!("lua_sys_fio.lua");
