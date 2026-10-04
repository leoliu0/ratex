//! Reading the embedded archive's virtual tree (`/<embedded>/…`, see
//! `tex_kpse::embedded_tree`) from Lua: the natives behind the `io.open`,
//! `io.lines`, `loadfile` and `dofile` wrappers of `lua_sys_embedded.lua`.

use std::any::Any;

use tex_lua::{Lua, LuaApi, LuaBytes, LuaString, UserDataRef, UserDataTrait, Value};

use crate::lua_sys::{bytes_of, sys_reg};

pub(crate) const PRELUDE: &str = include_str!("lua_sys_embedded.lua");

/// An open virtual file owns decoded bytes, or an immutable font capability
/// whose metadata ranges can be read without decoding the complete program.
/// Explicit close releases any decoded buffer even if the handle stays live.
struct ReadBuffer(Option<ReadBytes>);

enum ReadBytes {
    Decoded(Vec<u8>),
    Font(tex_kpse::EmbeddedFontFile),
}

impl ReadBytes {
    fn len(&self) -> usize {
        match self {
            Self::Decoded(data) => data.len(),
            Self::Font(file) => file.len(),
        }
    }

    fn full(&mut self) -> Result<&[u8], String> {
        if let Self::Font(file) = self {
            let data = file.read_all().ok_or_else(|| "embedded file could not be decoded".to_string())?;
            *self = Self::Decoded(data);
        }
        match self {
            Self::Decoded(data) => Ok(data),
            Self::Font(_) => unreachable!(),
        }
    }

    fn read(&mut self, start: usize, end: usize) -> Result<Vec<u8>, String> {
        if let Self::Font(file) = self {
            if let Some(data) = file.metadata_slice(start, end) {
                return Ok(data.to_vec());
            }
        }
        Ok(self.full()?[start..end].to_vec())
    }
}

impl UserDataTrait for ReadBuffer {
    fn type_name(&self) -> &'static str {
        "ratex.embedded_file"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn buffer(value: &Value) -> Result<UserDataRef<ReadBuffer>, String> {
    value
        .as_userdata::<ReadBuffer>()
        .ok_or_else(|| "invalid embedded file buffer".to_string())
}

/// The indices used by Lua's string.sub are one-based, inclusive, and may be
/// negative relative to the end of the string.
fn slice_bounds(start: i64, end: i64, len: usize) -> (usize, usize) {
    let len = len as i128;
    let relative = |index: i64| {
        if index < 0 { len + i128::from(index) + 1 } else { i128::from(index) }
    };
    let start = relative(start).max(1);
    let end = relative(end).min(len);
    if start > end { (0, 0) } else { ((start - 1) as usize, end as usize) }
}

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
    let open = lua
        .create_callback(|cb| {
            let path: LuaString = cb.arg(1)?;
            let bytes = bytes_of(&path);
            let data = std::str::from_utf8(&bytes).ok().and_then(|path| {
                tex_kpse::EmbeddedFontFile::open(path).map(ReadBytes::Font)
                    .or_else(|| tex_kpse::embedded_tree::read(path).map(ReadBytes::Decoded))
            });
            let Some(data) = data else { return cb.push(Option::<i64>::None) };
            let len = data.len() as i64;
            let handle = cb.create_userdata(ReadBuffer(Some(data)))?;
            let n = cb.push(handle)?;
            Ok(n + cb.push(len)?)
        })
        .map_err(|e| format!("{e:?}"))?;
    s.set("embedded_open", open).map_err(|e| format!("{e:?}"))?;
    sys_reg!(lua, s, "embedded_close", |value: Value| -> Result<(), String> {
        buffer(&value)?.borrow_mut().map_err(|e| format!("{e:?}"))?.0 = None;
        Ok(())
    });
    sys_reg!(lua, s, "embedded_slice", |value: Value, start: i64, end: i64| -> Result<LuaBytes, String> {
        let handle = buffer(&value)?;
        let mut handle = handle.borrow_mut().map_err(|e| format!("{e:?}"))?;
        let data = handle.0.as_mut().ok_or_else(|| "attempt to use a closed file".to_string())?;
        let (start, end) = slice_bounds(start, end, data.len());
        Ok(LuaBytes(data.read(start, end)?))
    });
    sys_reg!(lua, s, "embedded_newline", |value: Value, start: i64| -> Result<Option<i64>, String> {
        let handle = buffer(&value)?;
        let mut handle = handle.borrow_mut().map_err(|e| format!("{e:?}"))?;
        let data = handle.0.as_mut().ok_or_else(|| "attempt to use a closed file".to_string())?.full()?;
        let (start, end) = slice_bounds(start, data.len() as i64, data.len());
        Ok(data[start..end].iter().position(|&byte| byte == b'\n').map(|offset| (start + offset + 1) as i64))
    });
    sys_reg!(lua, s, "embedded_number_prefix", |value: Value, start: i64| -> Result<LuaBytes, String> {
        let handle = buffer(&value)?;
        let mut handle = handle.borrow_mut().map_err(|e| format!("{e:?}"))?;
        let data = handle.0.as_mut().ok_or_else(|| "attempt to use a closed file".to_string())?.full()?;
        let (start, end) = slice_bounds(start, data.len() as i64, data.len());
        let mut stop = start;
        while stop < end && matches!(data[stop], b' ' | b'\t'..=b'\r') {
            stop += 1;
        }
        // Keep the existing Lua numeral patterns, but give them only the
        // possible numeral prefix rather than a copy of the remaining file.
        while stop < end && matches!(data[stop], b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' | b'p' | b'P' | b'x' | b'X' | b'.' | b'+' | b'-') {
            stop += 1;
        }
        Ok(LuaBytes(data[start..stop].to_vec()))
    });
    Ok(())
}
