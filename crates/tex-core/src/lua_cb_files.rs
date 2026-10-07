//! LuaTeX's file callbacks (`texfileio.c`, `lcallbacklib.c`): `find_*_file`
//! return the name of a file, `read_*_file` its contents, `open_read_file`
//! an object whose `reader` and `close` functions feed `\input` and `\read`
//! line by line, and `start_file`/`stop_file` report the files TeX opens and
//! closes. A callback registered as `false` (state `-1`) counts as not
//! defined, as `callback_id > 0` does in luatex.

use tex_lua::{LuaApi, UdValue, Value, Variadic};

use crate::engine::Engine;
use crate::lua_callbacks::{Cb, CbArg, CbRet, CALLBACK_NAMES};

/// `filetype_*` codes of `start_file(category, name)` / `stop_file(category)`.
pub(crate) mod filetype {
    pub const TEX: i64 = 1;
    pub const MAP: i64 = 2;
    pub const IMAGE: i64 = 3;
    /// a subsetted font program
    pub const SUBSET: i64 = 4;
    /// a font program embedded in full
    pub const FONT: i64 = 5;
}

/// What a `read_*_file` callback delivered.
pub(crate) enum ReadFile {
    Data(Vec<u8>),
    NotOpened,
    Empty,
}

/// The reader object of an `open_read_file` callback, by number.
pub(crate) type ReaderId = u32;

impl crate::engine_lua::LuaEngine {
    /// Call the helper `name` of `lua_bridge.lua` with `args`.
    fn call_helper(&mut self, name: &str, args: Vec<UdValue>) -> Result<Vec<CbRet>, String> {
        let code = format!("return {name}(...)");
        let rets: Variadic<Value> = self
            .lua
            .load(&code)
            .call(Variadic(args))
            .map_err(|e| self.lua.get_error_message(e).message().to_string())?;
        Ok(rets.iter().map(CbRet::from_value).collect())
    }
}

impl Engine {
    /// An `R` result (`do_run_callback`): a string, `nil` or `false`; any
    /// other value is reported on stderr and counts as no result.
    fn cb_result_string(rets: &[CbRet]) -> Option<Vec<u8>> {
        match rets.first() {
            None | Some(CbRet::Nil) | Some(CbRet::Bool(false)) => None,
            Some(CbRet::Str(s)) => Some(s.clone()),
            Some(other) => {
                eprintln!("callback should return a string, false or nil, not: {}", other.type_name());
                None
            }
        }
    }

    /// `luatex_find_file` / `luatex_find_read_file`: the `find_*_file`
    /// callback `cb` (with the stream number `stream` for the read/write
    /// ones). `None`: no function is registered; `Some(None)`: it found no
    /// file.
    pub(crate) fn lua_find_file(&mut self, cb: Cb, stream: Option<i32>, name: &[u8]) -> Option<Option<Vec<u8>>> {
        if !self.cb_defined(cb) {
            return None;
        }
        let mut args = Vec::with_capacity(2);
        if let Some(n) = stream {
            args.push(CbArg::Int(i64::from(n)));
        }
        args.push(CbArg::Str(name.to_vec()));
        let rets = self.lua_cb_call(cb, CALLBACK_NAMES[cb as usize], args);
        Some(rets.and_then(|r| Self::cb_result_string(&r)))
    }

    /// The outcome of a `read_*_file` callback (`"S->bSd"`).
    ///
    /// `NotOpened`: the callback returned `false` (or a result of the wrong
    /// type before the flag); `Empty`: it opened the file but delivered no
    /// data. luatex tells the two apart (`file_opened`, `size > 0`).
    pub(crate) fn lua_read_file_ex(&mut self, cb: Cb, name: &[u8]) -> Option<ReadFile> {
        if !self.cb_defined(cb) {
            return None;
        }
        let rets = self.lua_cb_call(cb, CALLBACK_NAMES[cb as usize], vec![CbArg::Str(name.to_vec())]);
        let Some(rets) = rets else {
            return Some(ReadFile::NotOpened);
        };
        let opened = match rets.first() {
            Some(CbRet::Bool(b)) => *b,
            None | Some(CbRet::Nil) => false,
            Some(other) => {
                eprintln!("callback should return a boolean, not: {}", other.type_name());
                return Some(ReadFile::NotOpened);
            }
        };
        let data = match rets.get(1) {
            Some(CbRet::Str(s)) => s.clone(),
            other => {
                let t = other.map_or("nil", CbRet::type_name);
                eprintln!("callback should return a string, not: {t}");
                return Some(if opened { ReadFile::Empty } else { ReadFile::NotOpened });
            }
        };
        let size = match rets.get(2) {
            Some(CbRet::Int(n)) => *n,
            Some(CbRet::Num(n)) => *n as i64,
            other => {
                let t = other.map_or("nil", CbRet::type_name);
                eprintln!("callback should return a number, not: {t}");
                return Some(if opened { ReadFile::Empty } else { ReadFile::NotOpened });
            }
        };
        if !opened {
            return Some(ReadFile::NotOpened);
        }
        if size <= 0 {
            return Some(ReadFile::Empty);
        }
        let size = usize::try_from(size).unwrap_or(0).min(data.len());
        let mut data = data;
        data.truncate(size);
        Some(ReadFile::Data(data))
    }

    /// The `read_*_file` callbacks (`"S->bSd"`): the file contents when the
    /// callback opened the file and delivered data. `None`: no function is
    /// registered; `Some(None)`: unusable.
    pub(crate) fn lua_read_file(&mut self, cb: Cb, name: &[u8]) -> Option<Option<Vec<u8>>> {
        match self.lua_read_file_ex(cb, name)? {
            ReadFile::Data(data) => Some(Some(data)),
            ReadFile::NotOpened | ReadFile::Empty => Some(None),
        }
    }

    /// luatex `report_start_file(category, name)`: with a `start_file`
    /// callback the callback replaces the built-in message. Returns whether
    /// it did.
    pub(crate) fn lua_report_start_file(&mut self, category: i64, name: &[u8]) -> bool {
        if self.engine_kind != crate::engine::EngineKind::LuaTeX || !self.cb_defined(Cb::StartFile) {
            return false;
        }
        let _ = self.lua_cb_call(Cb::StartFile, "start_file", vec![CbArg::Int(category), CbArg::Str(name.to_vec())]);
        true
    }

    /// luatex `report_stop_file(category)`.
    pub(crate) fn lua_report_stop_file(&mut self, category: i64) -> bool {
        if self.engine_kind != crate::engine::EngineKind::LuaTeX || !self.cb_defined(Cb::StopFile) {
            return false;
        }
        let _ = self.lua_cb_call(Cb::StopFile, "stop_file", vec![CbArg::Int(category)]);
        true
    }

    /// luatex `lua_a_open_in` with an `open_read_file` callback: the object
    /// the callback returned for `name` (kept in Lua), or `None` when it
    /// returned nothing (the file cannot be read).
    pub(crate) fn lua_reader_open(&mut self, name: &[u8]) -> Option<ReaderId> {
        let name = name.to_vec();
        match self.lua_run(|lua| lua.call_helper("__texres_reader_open", vec![UdValue::Bytes(name)])) {
            Ok(rets) => match rets.first() {
                Some(CbRet::Int(id)) => u32::try_from(*id).ok(),
                _ => None,
            },
            Err(err) => {
                self.lua_callback_failed("open_read_file", &err);
                None
            }
        }
    }

    /// The next line from a reader object (`run_saved_callback(r, "reader",
    /// "->l")`): `None` at the end of the file. Trailing spaces are removed.
    pub(crate) fn lua_reader_line(&mut self, id: ReaderId) -> Option<Vec<u8>> {
        let args = vec![UdValue::Integer(i64::from(id)), UdValue::Bytes(b"reader".to_vec())];
        match self.lua_run(|lua| lua.call_helper("__texres_reader_call", args)) {
            Ok(rets) => match rets.into_iter().next() {
                Some(CbRet::Str(mut s)) => {
                    while s.last() == Some(&b' ') {
                        s.pop();
                    }
                    Some(s)
                }
                None | Some(CbRet::Nil) => None,
                Some(other) => {
                    eprintln!("callback should return a string, not: {}", other.type_name());
                    None
                }
            },
            Err(err) => {
                self.lua_callback_failed("reader", &err);
                None
            }
        }
    }

    /// `lua_a_close_in`: run the object's `close` function and drop it.
    pub(crate) fn lua_reader_close(&mut self, id: ReaderId) {
        let args = vec![UdValue::Integer(i64::from(id)), UdValue::Bytes(b"close".to_vec())];
        if let Err(err) = self.lua_run(|lua| lua.call_helper("__texres_reader_call", args)) {
            self.lua_callback_failed("close", &err);
        }
        let _ = self.lua_run(|lua| lua.call_helper("__texres_reader_free", vec![UdValue::Integer(i64::from(id))]));
    }

    /// tex.web §1335 `final_cleanup`: every file still open when `\end` is
    /// executed is shown as closed, ` )` each (luatex: `report_stop_file`,
    /// its `stop_file` callback or `)`, innermost first).
    pub(crate) fn close_open_files_at_end(&mut self) {
        let luatex = self.engine_kind == crate::engine::EngineKind::LuaTeX;
        let callback = luatex && self.cb_defined(Cb::StopFile);
        let open = self
            .input
            .stack
            .iter()
            .filter(|s| match s {
                crate::input::Source::File { name, lua_lines, lua_reader, announced, .. } => {
                    lua_lines.is_none()
                        && if callback {
                            !name.starts_with('<') || name.starts_with("<embedded:") || *lua_reader != 0
                        } else {
                            *announced
                        }
                }
                _ => false,
            })
            .count();
        for _ in 0..open {
            if callback {
                self.lua_report_stop_file(filetype::TEX);
            } else {
                self.tex_print_str(true, true, if luatex { ")" } else { " )" });
            }
        }
    }

    /// Close the `open_read_file` object of `\openin` stream `n`, if any.
    pub(crate) fn lua_close_read_reader(&mut self, n: usize) {
        let id = self.read_readers.get_mut(n).map_or(0, std::mem::take);
        if id != 0 {
            self.lua_reader_close(id);
        }
    }

    /// `\openin` with a `find_read_file` and/or `open_read_file` callback
    /// (stream numbers are passed as `n + 1`). Returns whether a callback
    /// handled the request; the stream is left closed when the file is not
    /// found.
    pub(crate) fn lua_openin(&mut self, n: usize, name: &str) -> bool {
        let find = self.cb_defined(Cb::FindReadFile);
        let open = self.cb_defined(Cb::OpenReadFile);
        if !find && !open {
            return false;
        }
        while self.read_readers.len() <= n {
            self.read_readers.push(0);
        }
        let fnam: Option<String> = if find {
            self.lua_find_file(Cb::FindReadFile, Some(n as i32 + 1), name.as_bytes())
                .flatten()
                .map(|b| String::from_utf8_lossy(&b).into_owned())
        } else {
            self.resolve_input_path(name).map(|p| p.to_string_lossy().into_owned())
        };
        self.read_files[n] = None;
        self.read_eof[n] = true;
        let Some(fnam) = fnam else {
            return true;
        };
        if open {
            if let Some(id) = self.lua_reader_open(fnam.as_bytes()) {
                self.read_readers[n] = id;
                self.read_files[n] = Some(Box::new(std::io::empty()));
                self.read_eof[n] = false;
            }
        } else if let Ok(bytes) = tex_kpse::fs::read(std::path::Path::new(&fnam)) {
            self.read_eof[n] = bytes.is_empty();
            self.read_files[n] = Some(Box::new(std::io::Cursor::new(bytes)));
        }
        true
    }
}
