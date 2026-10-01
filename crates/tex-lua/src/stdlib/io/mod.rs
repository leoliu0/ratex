// IO library: a port of liolib.c (Lua 5.3 and 5.5 behaviour).
// Implements: close, flush, input, lines, open, output, popen, read,
// tmpfile, type, write and the FILE* methods.
mod file;

use std::io::SeekFrom;

use crate::lib_registry::LibraryModule;
use crate::lua_value::{LuaUserdata, LuaValue};
use crate::lua_vm::lua_limits::MAX_STRING_SIZE;
use crate::lua_vm::{LuaResult, LuaState};
use crate::stdlib::lauxlib;
use crate::LuaLanguageLevel;
use file::{BufMode, CloseStatus};
pub use file::LuaFile;
pub(crate) use file::error_message;

/// Registry key of the FILE* metatable (luaL_newmetatable(L, LUA_FILEHANDLE)).
const FILEHANDLE: &str = "FILE*";
/// Registry key of a weak-keyed set of every file handle, so that
/// `os.exit` can flush them like C's `exit` flushes all stdio streams.
const OPEN_FILES: &str = "_IO_files";
const IO_INPUT: &str = "_IO_input";
const IO_OUTPUT: &str = "_IO_output";
/// Maximum number of formats of `lines` (MAXARGLINE).
const MAXARGLINE: usize = 250;
/// L_MAXLENNUM: maximum length of a numeral read by the "n" format.
const MAXLENNUM: usize = 200;
/// LUAL_BUFFERSIZE: default size for `setvbuf`.
const LUAL_BUFFERSIZE: i64 = 1024;

pub fn create_io_lib() -> LibraryModule {
    crate::lib_module!("io", {
        "close" => io_close,
        "flush" => io_flush,
        "input" => io_input,
        "lines" => io_lines,
        "open" => io_open,
        "output" => io_output,
        "popen" => io_popen,
        "read" => io_read,
        "tmpfile" => io_tmpfile,
        "type" => io_type,
        "write" => io_write,
    })
    .with_initializer(init_io_streams)
}

/// Create the FILE* metatable and the standard streams.
pub fn init_io_streams(l: &mut LuaState) -> LuaResult<()> {
    let io_table = l
        .get_global_value("io")?
        .filter(LuaValue::is_table)
        .ok_or_else(|| l.error("io table not found".to_string()))?;

    let lua53 = l.global_state().language() == LuaLanguageLevel::Lua53;
    let methods: [(&str, crate::lua_vm::CFunction); 7] = [
        ("close", f_close),
        ("flush", f_flush),
        ("lines", f_lines),
        ("read", f_read),
        ("seek", f_seek),
        ("setvbuf", f_setvbuf),
        ("write", f_write),
    ];
    let mt = l.create_table(0, 12)?;
    // 5.3 keeps the methods in the metatable itself (__index = metatable);
    // 5.4+ uses a separate method table.
    let method_table = if lua53 { mt } else { l.create_table(0, methods.len())? };
    for (name, function) in methods {
        let key = l.create_string(name)?;
        l.raw_set(&method_table, key, LuaValue::cfunction(function));
    }
    let mut fields = vec![
        ("__index", method_table),
        ("__gc", LuaValue::cfunction(f_gc)),
        ("__tostring", LuaValue::cfunction(f_tostring)),
    ];
    if !lua53 {
        fields.push(("__close", LuaValue::cfunction(f_gc)));
    }
    let name = l.create_string(FILEHANDLE)?;
    fields.push(("__name", name));
    for (key, value) in fields {
        let key = l.create_string(key)?;
        l.raw_set(&mt, key, value);
    }
    l.global_state_mut().registry_set(FILEHANDLE, mt)?;

    let open_files = l.create_table(0, 0)?;
    let weak_mt = l.create_table(0, 1)?;
    let mode_key = l.create_string("__mode")?;
    let mode = l.create_string("k")?;
    l.raw_set(&weak_mt, mode_key, mode);
    if let Some(table) = open_files.as_table_mut() {
        table.set_metatable(Some(weak_mt));
    }
    l.global_state_mut().registry_set(OPEN_FILES, open_files)?;

    let streams = [
        ("stdin", LuaFile::stdin(), Some(IO_INPUT)),
        ("stdout", LuaFile::stdout(), Some(IO_OUTPUT)),
        ("stderr", LuaFile::stderr(), None),
    ];
    for (name, stream, registry_key) in streams {
        let handle = new_file_handle(l, stream)?;
        let key = l.create_string(name)?;
        l.raw_set(&io_table, key, handle);
        if let Some(registry_key) = registry_key {
            set_default_file(l, registry_key, handle)?;
        }
    }
    Ok(())
}

/// Flush every file still open (what C's `exit` does to stdio streams).
pub(crate) fn flush_all(l: &mut LuaState) {
    let Ok(Some(files)) = l.global_state_mut().registry_get(OPEN_FILES) else {
        return;
    };
    let Some(table) = files.as_table() else {
        return;
    };
    for (handle, _) in table.iter_all() {
        if let Some(file) = file_of(&handle) {
            let _ = file.flush();
        }
    }
}

/// Wrap a stream in a userdata with the FILE* metatable.
fn new_file_handle(l: &mut LuaState, file: LuaFile) -> LuaResult<LuaValue> {
    let handle = l.create_userdata(LuaUserdata::new(file))?;
    if let Some(mt) = l.global_state_mut().registry_get(FILEHANDLE)?
        && let Some(ud) = handle.as_userdata_mut()
    {
        ud.set_metatable(mt);
    }
    l.global_state_mut().gc.check_finalizer(&handle);
    if let Some(files) = l.global_state_mut().registry_get(OPEN_FILES)? {
        l.raw_set(&files, handle, LuaValue::boolean(true));
    }
    Ok(handle)
}

#[inline]
fn file_of(value: &LuaValue) -> Option<&mut LuaFile> {
    value.as_userdata_mut()?.downcast_mut::<LuaFile>()
}

/// luaL_checkudata(L, 1, "FILE*") (`tolstream`).
fn check_stream(l: &mut LuaState) -> LuaResult<LuaValue> {
    match l.get_arg(1) {
        Some(value) if file_of(&value).is_some() => Ok(value),
        _ => Err(lauxlib::typeerror(l, 1, FILEHANDLE)),
    }
}

/// `tofile`: argument 1 must be an open file.
fn check_open_file(l: &mut LuaState) -> LuaResult<LuaValue> {
    let handle = l.get_arg(1).unwrap_or_default();
    match file_of(&handle) {
        Some(file) if !file.is_closed() => Ok(handle),
        Some(_) => Err(lauxlib::lual_error(l, "attempt to use a closed file")),
        None => Err(lauxlib::typeerror(l, 1, FILEHANDLE)),
    }
}

fn set_default_file(l: &mut LuaState, key: &str, handle: LuaValue) -> LuaResult<()> {
    l.global_state_mut().registry_set(key, handle)?;
    let vm = l.global_state_mut();
    if key == IO_INPUT {
        vm.io_default_input = Some(handle);
    } else {
        vm.io_default_output = Some(handle);
    }
    Ok(())
}

fn default_file(l: &mut LuaState, key: &str) -> LuaResult<LuaValue> {
    let cached = if key == IO_INPUT {
        l.global_state().io_default_input
    } else {
        l.global_state().io_default_output
    };
    match cached {
        Some(handle) => Ok(handle),
        None => Ok(l.global_state_mut().registry_get(key)?.unwrap_or_default()),
    }
}

/// `getiofile`: the default input/output file, which must be open.
fn open_default_file(l: &mut LuaState, key: &str) -> LuaResult<LuaValue> {
    let handle = default_file(l, key)?;
    if file_of(&handle).is_none_or(|file| file.is_closed()) {
        let kind = if l.global_state().language() == LuaLanguageLevel::Lua53 {
            "standard"
        } else {
            "default"
        };
        let which = &key["_IO_".len()..];
        return Err(lauxlib::lual_error(l, format!("{kind} {which} file is closed")));
    }
    Ok(handle)
}

/// luaL_fileresult(L, 0, fname): push fail, message, errno.
fn push_file_error(
    l: &mut LuaState,
    error: &std::io::Error,
    filename: Option<&str>,
) -> LuaResult<usize> {
    let message = file::error_message(error);
    let message = match filename {
        Some(name) => format!("{name}: {message}"),
        None => message,
    };
    l.push_value(LuaValue::nil())?;
    let message = l.create_string(&message)?;
    l.push_value(message)?;
    l.push_value(LuaValue::integer(error.raw_os_error().unwrap_or(0) as i64))?;
    Ok(3)
}

fn push_io_result(l: &mut LuaState, result: std::io::Result<()>) -> LuaResult<usize> {
    match result {
        Ok(()) => {
            l.push_value(LuaValue::boolean(true))?;
            Ok(1)
        }
        Err(error) => push_file_error(l, &error, None),
    }
}

/// `aux_close`: close a stream, returning the values of its close function.
fn aux_close(l: &mut LuaState, handle: LuaValue) -> LuaResult<usize> {
    let file = file_of(&handle).expect("checked FILE*");
    if file.is_std_stream() {
        // io_noclose
        l.push_value(LuaValue::nil())?;
        let message = l.create_string("cannot close standard file")?;
        l.push_value(message)?;
        return Ok(2);
    }
    match file.close() {
        Ok(CloseStatus::File) => {
            l.push_value(LuaValue::boolean(true))?;
            Ok(1)
        }
        // luaL_execresult
        Ok(CloseStatus::Process(what, code)) => {
            let success = what == "exit" && code == 0;
            l.push_value(if success { LuaValue::boolean(true) } else { LuaValue::nil() })?;
            let what = l.create_string(what)?;
            l.push_value(what)?;
            l.push_value(LuaValue::integer(code as i64))?;
            Ok(3)
        }
        Err(error) => push_file_error(l, &error, None),
    }
}

fn io_type(l: &mut LuaState) -> LuaResult<usize> {
    let value = lauxlib::check_any(l, 1)?;
    let result = match file_of(&value) {
        None => LuaValue::nil(),
        Some(file) if file.is_closed() => l.create_string("closed file")?,
        Some(_) => l.create_string("file")?,
    };
    l.push_value(result)?;
    Ok(1)
}

fn f_tostring(l: &mut LuaState) -> LuaResult<usize> {
    let handle = check_stream(l)?;
    let file = file_of(&handle).expect("checked FILE*");
    let text = if file.is_closed() {
        "file (closed)".to_string()
    } else {
        format!("file ({:p})", file.stream_addr())
    };
    let text = l.create_string(&text)?;
    l.push_value(text)?;
    Ok(1)
}

fn f_close(l: &mut LuaState) -> LuaResult<usize> {
    let handle = check_open_file(l)?;
    aux_close(l, handle)
}

fn io_close(l: &mut LuaState) -> LuaResult<usize> {
    if l.arg_count() > 0 {
        return f_close(l);
    }
    let handle = default_file(l, IO_OUTPUT)?;
    if file_of(&handle).is_some_and(|file| file.is_closed()) {
        return Err(lauxlib::lual_error(l, "attempt to use a closed file"));
    }
    aux_close(l, handle)
}

/// `__gc` and `__close`: close the file, ignoring the result.
fn f_gc(l: &mut LuaState) -> LuaResult<usize> {
    let handle = check_stream(l)?;
    if file_of(&handle).is_some_and(|file| !file.is_closed() && !file.is_std_stream()) {
        let _ = file_of(&handle).expect("checked FILE*").close();
    }
    Ok(0)
}

fn io_open(l: &mut LuaState) -> LuaResult<usize> {
    let filename = lauxlib::check_lstring(l, 1)?;
    let mode = lauxlib::opt_lstring(l, 2)?;
    let mode: &[u8] = mode.as_deref().unwrap_or(b"r");
    if !file::valid_open_mode(mode) {
        return Err(lauxlib::argerror(l, 2, "invalid mode"));
    }
    match LuaFile::open(&filename, mode) {
        Ok(file) => {
            let handle = new_file_handle(l, file)?;
            l.push_value(handle)?;
            Ok(1)
        }
        Err(error) => push_file_error(l, &error, Some(&String::from_utf8_lossy(&filename))),
    }
}

fn io_popen(l: &mut LuaState) -> LuaResult<usize> {
    let command = lauxlib::check_lstring(l, 1)?;
    let command = String::from_utf8_lossy(&command).into_owned();
    let mode = lauxlib::opt_lstring(l, 2)?;
    let mode: &[u8] = mode.as_deref().unwrap_or(b"r");
    if mode != b"r" && mode != b"w" {
        return Err(lauxlib::argerror(l, 2, "invalid mode"));
    }
    match LuaFile::popen(&command, mode == b"w") {
        Ok(file) => {
            let handle = new_file_handle(l, file)?;
            l.push_value(handle)?;
            Ok(1)
        }
        Err(error) => push_file_error(l, &error, Some(&command)),
    }
}

fn io_tmpfile(l: &mut LuaState) -> LuaResult<usize> {
    match LuaFile::tmpfile() {
        Ok(file) => {
            let handle = new_file_handle(l, file)?;
            l.push_value(handle)?;
            Ok(1)
        }
        Err(error) => push_file_error(l, &error, None),
    }
}

/// `opencheck`: open a file or raise "cannot open file".
fn open_or_error(l: &mut LuaState, narg: usize, mode: &[u8]) -> LuaResult<LuaValue> {
    let filename = lauxlib::check_lstring(l, narg)?;
    match LuaFile::open(&filename, mode) {
        Ok(file) => new_file_handle(l, file),
        Err(error) => Err(lauxlib::lual_error(
            l,
            format!(
                "cannot open file '{}' ({})",
                String::from_utf8_lossy(&filename),
                file::error_message(&error)
            ),
        )),
    }
}

/// `g_iofile`: io.input / io.output.
fn io_file(l: &mut LuaState, key: &str, mode: &[u8]) -> LuaResult<usize> {
    if let Some(arg) = l.get_arg(1).filter(|value| !value.is_nil()) {
        let handle = if arg.is_string() || arg.is_number() {
            open_or_error(l, 1, mode)?
        } else {
            check_open_file(l)?
        };
        set_default_file(l, key, handle)?;
    }
    let handle = default_file(l, key)?;
    l.push_value(handle)?;
    Ok(1)
}

fn io_input(l: &mut LuaState) -> LuaResult<usize> {
    io_file(l, IO_INPUT, b"r")
}

fn io_output(l: &mut LuaState) -> LuaResult<usize> {
    io_file(l, IO_OUTPUT, b"w")
}

/// `aux_lines`: build the `io_readline` closure over the file, the format
/// count, the close-at-EOF flag and the formats (arguments 2..).
fn aux_lines(l: &mut LuaState, handle: LuaValue, to_close: bool) -> LuaResult<LuaValue> {
    let nformats = l.arg_count().saturating_sub(1);
    if nformats > MAXARGLINE {
        return Err(lauxlib::argerror(l, MAXARGLINE + 2, "too many arguments"));
    }
    let mut upvalues = Vec::with_capacity(3 + nformats);
    upvalues.push(handle);
    upvalues.push(LuaValue::integer(nformats as i64));
    upvalues.push(LuaValue::boolean(to_close));
    upvalues.extend((2..2 + nformats).map(|i| l.get_arg(i).unwrap_or_default()));
    l.global_state_mut().create_c_closure(io_readline, upvalues)
}

fn f_lines(l: &mut LuaState) -> LuaResult<usize> {
    let handle = check_open_file(l)?;
    let iterator = aux_lines(l, handle, false)?;
    l.push_value(iterator)?;
    Ok(1)
}

fn io_lines(l: &mut LuaState) -> LuaResult<usize> {
    let (handle, to_close) = match l.get_arg(1).filter(|value| !value.is_nil()) {
        None => {
            let handle = default_file(l, IO_INPUT)?;
            if file_of(&handle).is_none_or(|file| file.is_closed()) {
                return Err(lauxlib::lual_error(l, "attempt to use a closed file"));
            }
            (handle, false)
        }
        Some(_) => (open_or_error(l, 1, b"r")?, true),
    };
    let iterator = aux_lines(l, handle, to_close)?;
    l.push_value(iterator)?;
    if to_close && l.global_state().language() != LuaLanguageLevel::Lua53 {
        // The file is the to-be-closed value of the generic for.
        l.push_value(LuaValue::nil())?;
        l.push_value(LuaValue::nil())?;
        l.push_value(handle)?;
        return Ok(4);
    }
    Ok(1)
}

/// Result of `g_read`, whose values are already on the stack.
enum ReadOutcome {
    /// `n` values were pushed; the last is nil if a format failed.
    Values { n: usize, first_ok: bool },
    /// An I/O error (luaL_fileresult) after some values were pushed.
    Error(std::io::Error),
}

/// What one read format produced.
enum Item {
    /// The bytes collected in the read buffer.
    Bytes,
    Value(LuaValue),
    /// Nothing could be read (end of file or not a numeral).
    Fail,
}

/// `read_number`: read a numeral following Lua's lexical rules.
fn read_number(file: &mut LuaFile) -> std::io::Result<Option<LuaValue>> {
    struct Numeral<'a> {
        file: &'a mut LuaFile,
        current: Option<u8>,
        buffer: Vec<u8>,
        overflow: bool,
    }
    impl Numeral<'_> {
        /// `nextc`: save the current char and read the next one.
        fn next(&mut self) -> std::io::Result<bool> {
            if self.buffer.len() >= MAXLENNUM {
                self.overflow = true;
                return Ok(false);
            }
            self.buffer.push(self.current.unwrap_or(0));
            self.current = self.file.getc()?;
            Ok(true)
        }
        /// `test2`: accept the current char if it is one of `set`.
        fn test2(&mut self, set: &[u8; 2]) -> std::io::Result<bool> {
            match self.current {
                Some(c) if c == set[0] || c == set[1] => self.next(),
                _ => Ok(false),
            }
        }
        fn digits(&mut self, hex: bool) -> std::io::Result<usize> {
            let mut count = 0;
            while let Some(c) = self.current {
                let is_digit = if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() };
                if !is_digit || !self.next()? {
                    break;
                }
                count += 1;
            }
            Ok(count)
        }
    }

    let mut current = file.getc()?;
    // isspace in the C locale
    while matches!(current, Some(b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')) {
        current = file.getc()?;
    }
    let mut rn = Numeral {
        file,
        current,
        buffer: Vec::with_capacity(32),
        overflow: false,
    };
    let mut count = 0;
    let mut hex = false;
    rn.test2(b"-+")?;
    if rn.test2(b"00")? {
        if rn.test2(b"xX")? {
            hex = true;
        } else {
            count = 1;
        }
    }
    count += rn.digits(hex)?;
    if rn.test2(b"..")? {
        count += rn.digits(hex)?;
    }
    if count > 0 && rn.test2(if hex { b"pP" } else { b"eE" })? {
        rn.test2(b"-+")?;
        rn.digits(false)?;
    }
    if rn.current.is_some() {
        rn.file.ungetc();
    }
    if rn.overflow {
        return Ok(None);
    }
    let value = std::str::from_utf8(&rn.buffer)
        .map(crate::stdlib::basic::parse_number::parse_lua_number)
        .unwrap_or_default();
    Ok((!value.is_nil()).then_some(value))
}

/// Format value number `narg` of `g_read` as a string (luaL_checkstring).
/// Formats of a `lines` iterator live in upvalues, so the value is
/// passed in rather than read from the argument list.
fn format_spec(l: &mut LuaState, narg: usize, format: &LuaValue) -> LuaResult<u8> {
    let Some(spec) = format.as_bytes() else {
        let message = format!("string expected, got {}", format.type_name());
        return Err(lauxlib::argerror(l, narg, &message));
    };
    let spec = spec.strip_prefix(b"*").unwrap_or(spec);
    Ok(spec.first().copied().unwrap_or(0))
}

/// Read one format into `buffer`.
fn read_item(
    l: &mut LuaState,
    file: &mut LuaFile,
    narg: usize,
    format: Option<&LuaValue>,
    buffer: &mut Vec<u8>,
) -> LuaResult<std::io::Result<Item>> {
    let line = |file: &mut LuaFile, buffer: &mut Vec<u8>, keep| {
        file.read_line(buffer, keep)
            .map(|newline| if newline || !buffer.is_empty() { Item::Bytes } else { Item::Fail })
    };
    let Some(format) = format else {
        return Ok(line(file, buffer, false));
    };
    if format.is_number() {
        let Some(n) = lauxlib::tointeger(format) else {
            return Err(lauxlib::argerror(l, narg, "number has no integer representation"));
        };
        let n = n as u64;
        if n == 0 {
            // test_eof
            return Ok(file.getc().map(|c| match c {
                Some(_) => {
                    file.ungetc();
                    Item::Bytes
                }
                None => Item::Fail,
            }));
        }
        if n > MAX_STRING_SIZE as u64 {
            let message = if l.global_state().language() == LuaLanguageLevel::Lua53 {
                "not enough memory for buffer allocation"
            } else {
                "resulting string too large"
            };
            return Err(lauxlib::lual_error(l, message));
        }
        return Ok(file
            .read_chars(buffer, n as usize)
            .map(|()| if buffer.is_empty() { Item::Fail } else { Item::Bytes }));
    }
    Ok(match format_spec(l, narg, format)? {
        b'n' => read_number(file).map(|n| n.map_or(Item::Fail, Item::Value)),
        b'l' => line(file, buffer, false),
        b'L' => line(file, buffer, true),
        b'a' => file.read_all(buffer).map(|()| Item::Bytes),
        _ => return Err(lauxlib::argerror(l, narg, "invalid format")),
    })
}

/// `g_read`: push one value per format (`formats` are arguments
/// `first..`); a failed format pushes nil and stops.
fn g_read(l: &mut LuaState, file: &mut LuaFile, first: usize, formats: &[LuaValue]) -> LuaResult<ReadOutcome> {
    let mut buffer = file.take_scratch();
    let result = g_read_into(l, file, first, formats, &mut buffer);
    file.put_scratch(buffer);
    result
}

fn g_read_into(
    l: &mut LuaState,
    file: &mut LuaFile,
    first: usize,
    formats: &[LuaValue],
    buffer: &mut Vec<u8>,
) -> LuaResult<ReadOutcome> {
    let count = formats.len().max(1);
    for i in 0..count {
        buffer.clear();
        match read_item(l, file, first + i, formats.get(i), buffer)? {
            Err(error) => return Ok(ReadOutcome::Error(error)),
            Ok(Item::Fail) => {
                l.push_value(LuaValue::nil())?;
                return Ok(ReadOutcome::Values { n: i + 1, first_ok: i > 0 });
            }
            Ok(Item::Bytes) => {
                let value = l.create_bytes(buffer)?;
                l.push_value(value)?;
            }
            Ok(Item::Value(value)) => l.push_value(value)?,
        }
    }
    Ok(ReadOutcome::Values { n: count, first_ok: true })
}

fn read_results(l: &mut LuaState, outcome: ReadOutcome) -> LuaResult<usize> {
    match outcome {
        ReadOutcome::Values { n, .. } => Ok(n),
        ReadOutcome::Error(error) => push_file_error(l, &error, None),
    }
}

fn io_read(l: &mut LuaState) -> LuaResult<usize> {
    let handle = open_default_file(l, IO_INPUT)?;
    let file = file_of(&handle).expect("checked FILE*");
    let formats = l.arg_slice().to_vec();
    let outcome = g_read(l, file, 1, &formats)?;
    read_results(l, outcome)
}

fn f_read(l: &mut LuaState) -> LuaResult<usize> {
    let handle = l.get_arg(1).unwrap_or_default();
    let Some(file) = file_of(&handle).filter(|file| !file.is_closed()) else {
        return check_open_file(l).map(|_| 0);
    };
    let formats = l.arg_slice()[1..].to_vec();
    let outcome = g_read(l, file, 2, &formats)?;
    read_results(l, outcome)
}

/// `io_readline`: the iterator returned by `lines`. Upvalues: file,
/// format count, close-at-EOF flag, formats.
fn io_readline(l: &mut LuaState) -> LuaResult<usize> {
    let function = l
        .call_depth()
        .checked_sub(1)
        .and_then(|frame| l.get_frame_func(frame))
        .unwrap_or_default();
    let upvalues: &[LuaValue] = function.as_cclosure().map_or(&[], |c| c.upvalues());
    let handle = upvalues.first().copied().unwrap_or_default();
    let to_close = upvalues.get(2).is_some_and(LuaValue::is_truthy);
    let formats = upvalues[3.min(upvalues.len())..].to_vec();
    let Some(file) = file_of(&handle).filter(|file| !file.is_closed()) else {
        return Err(lauxlib::lual_error(l, "file is already closed"));
    };
    // The formats act as arguments 2.. of the iterator (as on the C stack).
    match g_read(l, file, 2, &formats)? {
        ReadOutcome::Error(error) => Err(lauxlib::lual_error(l, file::error_message(&error))),
        ReadOutcome::Values { n, first_ok: true } => Ok(n),
        ReadOutcome::Values { .. } => {
            if to_close {
                let _ = file.close();
            }
            Ok(0)
        }
    }
}

/// `g_write`: write arguments `first..` to `file` (the userdata `handle`)
/// and return the file.
fn g_write(l: &mut LuaState, handle: LuaValue, file: &mut LuaFile, first: usize) -> LuaResult<usize> {
    let lua53 = l.global_state().language() == LuaLanguageLevel::Lua53;
    let nargs = l.arg_count();
    let mut status: std::io::Result<()> = Ok(());
    let mut written: usize = 0;
    let mut number = itoa::Buffer::new();
    for narg in first..=nargs {
        let value = l.get_arg(narg).unwrap_or_default();
        let float_text;
        let converted;
        let data: &[u8] = if let Some(bytes) = value.as_bytes() {
            bytes
        } else if let Some(n) = value.as_integer_strict() {
            number.format(n).as_bytes()
        } else if value.is_float() {
            float_text = value.as_number().unwrap_or_default().to_string();
            float_text.as_bytes()
        } else {
            converted = lauxlib::check_lstring(l, narg)?;
            &converted
        };
        match file.write(data) {
            Ok(()) => written += data.len(),
            // 5.3 keeps writing and reports the first failure at the end.
            Err(error) if lua53 => {
                if status.is_ok() {
                    status = Err(error);
                }
            }
            Err(error) => {
                let n = push_file_error(l, &error, None)?;
                l.push_value(LuaValue::integer(written as i64))?;
                return Ok(n + 1);
            }
        }
    }
    match status {
        Ok(()) => {
            l.push_value(handle)?;
            Ok(1)
        }
        Err(error) => push_file_error(l, &error, None),
    }
}

fn io_write(l: &mut LuaState) -> LuaResult<usize> {
    let handle = open_default_file(l, IO_OUTPUT)?;
    let file = file_of(&handle).expect("checked FILE*");
    g_write(l, handle, file, 1)
}

fn f_write(l: &mut LuaState) -> LuaResult<usize> {
    let handle = l.get_arg(1).unwrap_or_default();
    match file_of(&handle) {
        Some(file) if !file.is_closed() => g_write(l, handle, file, 2),
        _ => check_open_file(l).map(|_| 0),
    }
}

fn f_seek(l: &mut LuaState) -> LuaResult<usize> {
    let handle = check_open_file(l)?;
    let whence = lauxlib::check_option(l, 2, Some("cur"), &["set", "cur", "end"])?;
    let offset = lauxlib::opt_integer(l, 3, 0)?;
    let position = match whence {
        0 => {
            if offset < 0 {
                return push_file_error(l, &invalid_argument(), None);
            }
            SeekFrom::Start(offset as u64)
        }
        1 => SeekFrom::Current(offset),
        _ => SeekFrom::End(offset),
    };
    match file_of(&handle).expect("checked FILE*").seek(position) {
        Ok(position) => {
            l.push_value(LuaValue::integer(position as i64))?;
            Ok(1)
        }
        Err(error) => push_file_error(l, &error, None),
    }
}

fn invalid_argument() -> std::io::Error {
    #[cfg(unix)]
    {
        std::io::Error::from_raw_os_error(libc::EINVAL)
    }
    #[cfg(not(unix))]
    {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid argument")
    }
}

fn f_setvbuf(l: &mut LuaState) -> LuaResult<usize> {
    let handle = check_open_file(l)?;
    let mode = lauxlib::check_option(l, 2, None, &["no", "full", "line"])?;
    let size = lauxlib::opt_integer(l, 3, LUAL_BUFFERSIZE)?;
    let mode = [BufMode::No, BufMode::Full, BufMode::Line][mode];
    let result = file_of(&handle)
        .expect("checked FILE*")
        .setvbuf(mode, size.max(1) as usize);
    push_io_result(l, result)
}

fn flush_result(l: &mut LuaState, handle: LuaValue) -> LuaResult<usize> {
    let result = file_of(&handle).expect("checked FILE*").flush();
    push_io_result(l, result)
}

fn io_flush(l: &mut LuaState) -> LuaResult<usize> {
    let handle = open_default_file(l, IO_OUTPUT)?;
    flush_result(l, handle)
}

fn f_flush(l: &mut LuaState) -> LuaResult<usize> {
    let handle = check_open_file(l)?;
    flush_result(l, handle)
}
