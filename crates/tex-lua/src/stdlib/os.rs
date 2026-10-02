// OS library: a port of loslib.c (Lua 5.3 and 5.5 behaviour).
// Implements: clock, date, difftime, execute, exit, getenv, remove, rename,
// setlocale, time, tmpname

use crate::LuaLanguageLevel;
use crate::lib_registry::LibraryModule;
use crate::lua_value::LuaValue;
use crate::lua_vm::{LuaResult, LuaState};
use crate::stdlib::lauxlib;

pub fn create_os_lib() -> LibraryModule {
    crate::lib_module!("os", {
        "clock" => os_clock,
        "date" => os_date,
        "difftime" => os_difftime,
        "execute" => os_execute,
        "exit" => os_exit,
        "getenv" => os_getenv,
        "remove" => os_remove,
        "rename" => os_rename,
        "setlocale" => os_setlocale,
        "time" => os_time,
        "tmpname" => os_tmpname,
    })
}

#[inline]
fn is_lua53(l: &LuaState) -> bool {
    l.global_state().language() == LuaLanguageLevel::Lua53
}

/// Broken-down time (`struct tm` with C's field conventions).
#[derive(Clone, Copy, Default)]
struct Tm {
    sec: i32,
    min: i32,
    hour: i32,
    mday: i32,
    /// Months since January (0-11).
    mon: i32,
    /// Years since 1900.
    year: i32,
    /// Days since Sunday (0-6).
    wday: i32,
    /// Days since January 1 (0-365).
    yday: i32,
    /// Positive, zero, or negative (unknown) like `tm_isdst`.
    isdst: i32,
}

#[cfg(unix)]
mod sys {
    use super::Tm;

    fn to_libc(tm: &Tm) -> libc::tm {
        // SAFETY: libc::tm is plain data; all-zero is a valid value.
        let mut out: libc::tm = unsafe { std::mem::zeroed() };
        out.tm_sec = tm.sec;
        out.tm_min = tm.min;
        out.tm_hour = tm.hour;
        out.tm_mday = tm.mday;
        out.tm_mon = tm.mon;
        out.tm_year = tm.year;
        out.tm_wday = tm.wday;
        out.tm_yday = tm.yday;
        out.tm_isdst = tm.isdst;
        out
    }

    fn from_libc(tm: &libc::tm) -> Tm {
        Tm {
            sec: tm.tm_sec,
            min: tm.tm_min,
            hour: tm.tm_hour,
            mday: tm.tm_mday,
            mon: tm.tm_mon,
            year: tm.tm_year,
            wday: tm.tm_wday,
            yday: tm.tm_yday,
            isdst: tm.tm_isdst,
        }
    }

    /// `localtime_r` / `gmtime_r`; the libc tm is kept for `strftime`.
    pub(super) fn broken_down(t: i64, utc: bool) -> Option<(Tm, libc::tm)> {
        let time = libc::time_t::try_from(t).ok()?;
        // SAFETY: plain data, filled in by the call below.
        let mut out: libc::tm = unsafe { std::mem::zeroed() };
        let result = unsafe {
            if utc {
                libc::gmtime_r(&time, &mut out)
            } else {
                libc::localtime_r(&time, &mut out)
            }
        };
        (!result.is_null()).then(|| (from_libc(&out), out))
    }

    /// `mktime`: normalizes `tm` in place.
    pub(super) fn mktime(tm: &mut Tm) -> Option<i64> {
        let mut raw = to_libc(tm);
        let t = unsafe { libc::mktime(&mut raw) };
        *tm = from_libc(&raw);
        (t != -1).then_some(t as i64)
    }

    /// `strftime` of one conversion (`spec` includes the '%').
    pub(super) fn strftime(spec: &[u8], tm: &libc::tm, out: &mut Vec<u8>) {
        let mut format = spec.to_vec();
        format.push(0);
        let mut buf = [0u8; 250];
        let n = unsafe {
            libc::strftime(
                buf.as_mut_ptr().cast(),
                buf.len(),
                format.as_ptr().cast(),
                tm,
            )
        };
        out.extend_from_slice(&buf[..n]);
    }

    /// `clock()`: processor time used by the process (glibc reads the
    /// same clock).
    pub(super) fn clock() -> f64 {
        // SAFETY: plain data, filled in by the call.
        let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
        if unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) } != 0 {
            return -1.0;
        }
        ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
    }
}

#[cfg(not(unix))]
mod sys {
    use super::Tm;
    use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Timelike, Utc};

    pub(super) type Raw = DateTime<chrono::FixedOffset>;

    fn from_datetime(dt: &Raw) -> Tm {
        Tm {
            sec: dt.second() as i32,
            min: dt.minute() as i32,
            hour: dt.hour() as i32,
            mday: dt.day() as i32,
            mon: dt.month0() as i32,
            year: dt.year() - 1900,
            wday: dt.weekday().num_days_from_sunday() as i32,
            yday: dt.ordinal0() as i32,
            isdst: 0,
        }
    }

    pub(super) fn broken_down(t: i64, utc: bool) -> Option<(Tm, Raw)> {
        let utc_time = Utc.timestamp_opt(t, 0).single()?;
        let dt: Raw = if utc {
            utc_time.fixed_offset()
        } else {
            utc_time.with_timezone(&Local).fixed_offset()
        };
        Some((from_datetime(&dt), dt))
    }

    pub(super) fn mktime(tm: &mut Tm) -> Option<i64> {
        let year = tm.year as i64 + 1900 + (tm.mon as i64).div_euclid(12);
        let month = (tm.mon as i64).rem_euclid(12) as u32 + 1;
        let base = NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, 1)?.and_hms_opt(0, 0, 0)?;
        let naive = base
            + chrono::Duration::days(tm.mday as i64 - 1)
            + chrono::Duration::hours(tm.hour as i64)
            + chrono::Duration::minutes(tm.min as i64)
            + chrono::Duration::seconds(tm.sec as i64);
        let local = Local.from_local_datetime(&naive);
        let dt = local.single().or_else(|| local.latest())?;
        *tm = from_datetime(&dt.fixed_offset());
        Some(dt.timestamp())
    }

    /// `strftime` of one conversion (`spec` includes the '%'), validated by
    /// `conversion_length`. The `E` and `O` modifiers change nothing in the
    /// C locale. chrono's formatter reports unsupported items as a
    /// `fmt::Error` (a panic in `to_string`), so errors yield no text.
    pub(super) fn strftime(spec: &[u8], dt: &Raw, out: &mut Vec<u8>) {
        use std::fmt::Write;
        let conv = match spec {
            [b'%', b'E' | b'O', conv] => *conv,
            [b'%', conv] => *conv,
            _ => return,
        };
        if conv == b'Z' && dt.offset().local_minus_utc() == 0 {
            out.extend_from_slice(b"GMT");
            return;
        }
        let format = ['%', conv as char].into_iter().collect::<String>();
        let mut text = String::new();
        if write!(text, "{}", dt.format(&format)).is_ok() {
            out.extend_from_slice(text.as_bytes());
        }
    }
}

fn os_clock(l: &mut LuaState) -> LuaResult<usize> {
    #[cfg(unix)]
    let seconds = sys::clock();
    #[cfg(not(unix))]
    let seconds = l.global_state_mut().start_time.elapsed_secs_f64();
    l.push_value(LuaValue::float(seconds))?;
    Ok(1)
}

/// Conversions accepted by `os.date` (LUA_STRFTIMEOPTIONS for C99):
/// one-char options, then two-char `E`/`O` modified ones.
const STRFTIME_OPTIONS_1: &[u8] = b"aAbBcCdDeFgGhHIjmMnprRStTuUVwWxXyYzZ%";
const STRFTIME_OPTIONS_2: [&[u8]; 19] = [
    b"Ec", b"EC", b"Ex", b"EX", b"Ey", b"EY", b"Od", b"Oe", b"OH", b"OI", b"Om", b"OM", b"OS",
    b"Ou", b"OU", b"OV", b"Ow", b"OW", b"Oy",
];

/// `checkoption`: length of the valid conversion at the start of `conv`.
fn conversion_length(conv: &[u8]) -> Option<usize> {
    if conv.first().is_some_and(|c| STRFTIME_OPTIONS_1.contains(c)) {
        return Some(1);
    }
    STRFTIME_OPTIONS_2
        .iter()
        .any(|option| conv.starts_with(option))
        .then_some(2)
}

/// `setallfields`: store a broken-down time in the table `table`.
fn set_all_fields(l: &mut LuaState, table: &LuaValue, tm: &Tm) -> LuaResult<()> {
    let fields = [
        ("year", tm.year as i64 + 1900),
        ("month", tm.mon as i64 + 1),
        ("day", tm.mday as i64),
        ("hour", tm.hour as i64),
        ("min", tm.min as i64),
        ("sec", tm.sec as i64),
        ("yday", tm.yday as i64 + 1),
        ("wday", tm.wday as i64 + 1),
    ];
    for (name, value) in fields {
        let key = l.create_string(name)?;
        l.raw_set(table, key, LuaValue::integer(value));
    }
    // A negative isdst means "unknown": the field is left nil.
    if tm.isdst >= 0 {
        let key = l.create_string("isdst")?;
        l.raw_set(table, key, LuaValue::boolean(tm.isdst != 0));
    }
    Ok(())
}

/// `l_checktime` for an optional argument.
fn opt_time(l: &mut LuaState, narg: usize) -> LuaResult<i64> {
    match l.get_arg(narg) {
        Some(value) if !value.is_nil() => lauxlib::check_integer(l, narg),
        _ => Ok(crate::platform_time::unix_secs() as i64),
    }
}

fn os_date(l: &mut LuaState) -> LuaResult<usize> {
    let format = lauxlib::opt_lstring(l, 1)?;
    let format: &[u8] = format.as_deref().unwrap_or(b"%c");
    let t = opt_time(l, 2)?;
    let (utc, mut s) = match format.strip_prefix(b"!") {
        Some(rest) => (true, rest),
        None => (false, format),
    };
    let Some((tm, raw)) = sys::broken_down(t, utc) else {
        let what = if is_lua53(l) { "time" } else { "date" };
        return Err(lauxlib::lual_error(
            l,
            format!("{what} result cannot be represented in this installation"),
        ));
    };
    // strcmp(s, "*t"): the comparison stops at an embedded zero.
    if s.split(|&c| c == 0).next() == Some(b"*t".as_slice()) {
        let table = l.create_table(0, 9)?;
        set_all_fields(l, &table, &tm)?;
        l.push_value(table)?;
        return Ok(1);
    }
    let mut out = Vec::with_capacity(s.len() + 16);
    while let Some((&c, rest)) = s.split_first() {
        if c != b'%' {
            out.push(c);
            s = rest;
            continue;
        }
        let Some(len) = conversion_length(rest) else {
            // The message shows the rest of the format (C's '%s' of 'conv').
            let conv = rest.split(|&c| c == 0).next().unwrap_or(&[]);
            let message = format!(
                "invalid conversion specifier '%{}'",
                String::from_utf8_lossy(conv)
            );
            return Err(lauxlib::argerror(l, 1, &message));
        };
        sys::strftime(&s[..=len], &raw, &mut out);
        s = &rest[len..];
    }
    let result = l.create_bytes(&out)?;
    l.push_value(result)?;
    Ok(1)
}

/// `getfield`: an integer date field, `delta` subtracted (C field origin).
fn get_field(
    l: &mut LuaState,
    table: &LuaValue,
    key: &str,
    default: Option<i32>,
    delta: i64,
) -> LuaResult<i32> {
    let key_value = l.create_string(key)?;
    let value = l.table_get(table, &key_value)?.unwrap_or_default();
    let Some(n) = lauxlib::tointeger(&value) else {
        if !value.is_nil() {
            return Err(lauxlib::lual_error(l, format!("field '{key}' is not an integer")));
        }
        return match default {
            Some(default) => Ok(default),
            None => Err(lauxlib::lual_error(l, format!("field '{key}' missing in date table"))),
        };
    };
    let in_range = if is_lua53(l) {
        // L_MAXDATEFIELD = INT_MAX / 2
        let max = (i32::MAX / 2) as i64;
        (-max..=max).contains(&n)
    } else if n >= 0 {
        n - delta <= i32::MAX as i64
    } else {
        i32::MIN as i64 + delta <= n
    };
    if !in_range {
        return Err(lauxlib::lual_error(l, format!("field '{key}' is out-of-bound")));
    }
    Ok((n - delta) as i32)
}

fn os_time(l: &mut LuaState) -> LuaResult<usize> {
    let t = match l.get_arg(1) {
        None => crate::platform_time::unix_secs() as i64,
        Some(value) if value.is_nil() => crate::platform_time::unix_secs() as i64,
        Some(table) => {
            if !table.is_table() {
                return Err(lauxlib::typeerror(l, 1, "table"));
            }
            let mut tm = Tm::default();
            // The two versions read (and so report errors for) the fields
            // in opposite orders.
            let fields: [(&str, Option<i32>, i64); 6] = [
                ("year", None, 1900),
                ("month", None, 1),
                ("day", None, 0),
                ("hour", Some(12), 0),
                ("min", Some(0), 0),
                ("sec", Some(0), 0),
            ];
            let mut values = [0i32; 6];
            let order: [usize; 6] = if is_lua53(l) { [5, 4, 3, 2, 1, 0] } else { [0, 1, 2, 3, 4, 5] };
            for i in order {
                let (key, default, delta) = fields[i];
                values[i] = get_field(l, &table, key, default, delta)?;
            }
            [tm.year, tm.mon, tm.mday, tm.hour, tm.min, tm.sec] = values;
            let isdst_key = l.create_string("isdst")?;
            let isdst = l.table_get(&table, &isdst_key)?.unwrap_or_default();
            tm.isdst = if isdst.is_nil() { -1 } else { i32::from(isdst.is_truthy()) };
            let result = sys::mktime(&mut tm);
            set_all_fields(l, &table, &tm)?;
            match result {
                Some(t) => t,
                None => {
                    return Err(lauxlib::lual_error(
                        l,
                        "time result cannot be represented in this installation",
                    ));
                }
            }
        }
    };
    l.push_value(LuaValue::integer(t))?;
    Ok(1)
}

fn os_difftime(l: &mut LuaState) -> LuaResult<usize> {
    let t1 = lauxlib::check_integer(l, 1)?;
    let t2 = lauxlib::check_integer(l, 2)?;
    l.push_value(LuaValue::float(t1 as f64 - t2 as f64))?;
    Ok(1)
}

/// `luaL_execresult` for a finished child process.
#[cfg(not(target_arch = "wasm32"))]
fn push_exec_result(l: &mut LuaState, status: std::process::ExitStatus) -> LuaResult<usize> {
    #[cfg(unix)]
    let (what, code) = {
        use std::os::unix::process::ExitStatusExt;
        match status.signal() {
            Some(signal) => ("signal", signal),
            None => ("exit", status.code().unwrap_or(-1)),
        }
    };
    #[cfg(not(unix))]
    let (what, code) = ("exit", status.code().unwrap_or(-1));
    let success = what == "exit" && code == 0;
    l.push_value(if success { LuaValue::boolean(true) } else { LuaValue::nil() })?;
    let what = l.create_string(what)?;
    l.push_value(what)?;
    l.push_value(LuaValue::integer(code as i64))?;
    Ok(3)
}

/// luaL_fileresult(L, 0, fname).
fn push_os_error(l: &mut LuaState, error: &std::io::Error, filename: Option<&[u8]>) -> LuaResult<usize> {
    let message = crate::stdlib::io::error_message(error);
    let message = match filename {
        Some(name) => format!("{}: {message}", String::from_utf8_lossy(name)),
        None => message,
    };
    l.push_value(LuaValue::nil())?;
    let message = l.create_string(&message)?;
    l.push_value(message)?;
    l.push_value(LuaValue::integer(crate::stdlib::io::file::error_code(error)))?;
    Ok(3)
}

fn os_execute(l: &mut LuaState) -> LuaResult<usize> {
    let command = lauxlib::opt_lstring(l, 1)?;
    let Some(command) = command else {
        // system(NULL): is a shell available?
        l.push_value(LuaValue::boolean(cfg!(not(target_arch = "wasm32"))))?;
        return Ok(1);
    };
    #[cfg(target_arch = "wasm32")]
    {
        let _ = command;
        push_os_error(l, &std::io::Error::other("'execute' not supported"), None)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Write;
        let command = String::from_utf8_lossy(&command).into_owned();
        let _ = std::io::stdout().flush();
        #[cfg(windows)]
        let status = std::process::Command::new("cmd").args(["/C", &command]).status();
        #[cfg(not(windows))]
        let status = std::process::Command::new("/bin/sh").arg("-c").arg(&command).status();
        match status {
            Ok(status) => push_exec_result(l, status),
            Err(error) => push_os_error(l, &error, None),
        }
    }
}

fn os_exit(l: &mut LuaState) -> LuaResult<usize> {
    let status = match l.get_arg(1) {
        Some(value) if value.as_boolean().is_some() => {
            if value.is_truthy() { 0 } else { 1 }
        }
        _ => lauxlib::opt_integer(l, 1, 0)? as i32,
    };
    if l.get_arg(2).is_some_and(|value| value.is_truthy()) {
        l.global_state_mut().close();
    }
    // C's exit() flushes every stdio stream.
    crate::stdlib::io::flush_all(l);
    std::process::exit(status);
}

fn os_getenv(l: &mut LuaState) -> LuaResult<usize> {
    let name = lauxlib::check_lstring(l, 1)?;
    #[cfg(unix)]
    let value = {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        std::env::var_os(std::ffi::OsStr::from_bytes(&name)).map(OsStringExt::into_vec)
    };
    #[cfg(not(unix))]
    let value = std::env::var_os(String::from_utf8_lossy(&name).as_ref())
        .map(|v| v.to_string_lossy().into_owned().into_bytes());
    let result = match value {
        Some(bytes) => l.create_bytes(&bytes)?,
        None => LuaValue::nil(),
    };
    l.push_value(result)?;
    Ok(1)
}

#[cfg(unix)]
fn c_path(bytes: &[u8]) -> std::io::Result<std::ffi::CString> {
    std::ffi::CString::new(bytes).map_err(|_| std::io::Error::from_raw_os_error(libc::ENOENT))
}

fn os_remove(l: &mut LuaState) -> LuaResult<usize> {
    let filename = lauxlib::check_lstring(l, 1)?;
    // remove(3) deletes files and empty directories.
    #[cfg(unix)]
    let result = c_path(&filename).and_then(|path| {
        if unsafe { libc::remove(path.as_ptr()) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    });
    #[cfg(not(unix))]
    let result = {
        let path = String::from_utf8_lossy(&filename).into_owned();
        std::fs::remove_file(&path).or_else(|error| std::fs::remove_dir(&path).map_err(|_| error))
    };
    match result {
        Ok(()) => {
            l.push_value(LuaValue::boolean(true))?;
            Ok(1)
        }
        Err(error) => push_os_error(l, &error, Some(&filename)),
    }
}

fn os_rename(l: &mut LuaState) -> LuaResult<usize> {
    let from = lauxlib::check_lstring(l, 1)?;
    let to = lauxlib::check_lstring(l, 2)?;
    #[cfg(unix)]
    let result = {
        use std::os::unix::ffi::OsStrExt;
        std::fs::rename(std::ffi::OsStr::from_bytes(&from), std::ffi::OsStr::from_bytes(&to))
    };
    #[cfg(not(unix))]
    let result = std::fs::rename(
        String::from_utf8_lossy(&from).as_ref(),
        String::from_utf8_lossy(&to).as_ref(),
    );
    match result {
        Ok(()) => {
            l.push_value(LuaValue::boolean(true))?;
            Ok(1)
        }
        Err(error) => push_os_error(l, &error, None),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn os_setlocale(l: &mut LuaState) -> LuaResult<usize> {
    let locale = lauxlib::opt_lstring(l, 1)?;
    let category = lauxlib::check_option(
        l,
        2,
        Some("all"),
        &["all", "collate", "ctype", "monetary", "numeric", "time"],
    )?;
    let category = [
        libc::LC_ALL,
        libc::LC_COLLATE,
        libc::LC_CTYPE,
        libc::LC_MONETARY,
        libc::LC_NUMERIC,
        libc::LC_TIME,
    ][category];
    // A name with an embedded zero is cut there, as C sees it.
    let locale = locale.map(|name| {
        let name = name.split(|&c| c == 0).next().unwrap_or(&[]);
        std::ffi::CString::new(name).expect("no interior zero")
    });
    let locale_ptr = locale.as_ref().map_or(std::ptr::null(), |name| name.as_ptr());
    let result = unsafe { libc::setlocale(category, locale_ptr) };
    let value = if result.is_null() {
        LuaValue::nil()
    } else {
        l.create_bytes(unsafe { std::ffi::CStr::from_ptr(result) }.to_bytes())?
    };
    l.push_value(value)?;
    Ok(1)
}

#[cfg(target_arch = "wasm32")]
fn os_setlocale(l: &mut LuaState) -> LuaResult<usize> {
    let locale = lauxlib::opt_lstring(l, 1)?;
    lauxlib::check_option(
        l,
        2,
        Some("all"),
        &["all", "collate", "ctype", "monetary", "numeric", "time"],
    )?;
    // Only the "C" locale exists.
    let accepted = locale.as_deref().is_none_or(|name| matches!(name, b"" | b"C" | b"POSIX"));
    let value = if accepted { l.create_string("C")? } else { LuaValue::nil() };
    l.push_value(value)?;
    Ok(1)
}

fn os_tmpname(l: &mut LuaState) -> LuaResult<usize> {
    // POSIX Lua: mkstemp("/tmp/lua_XXXXXX"), leaving the file created.
    #[cfg(unix)]
    let name = {
        let mut template = *b"/tmp/lua_XXXXXX\0";
        let fd = unsafe { libc::mkstemp(template.as_mut_ptr().cast()) };
        if fd == -1 {
            None
        } else {
            unsafe { libc::close(fd) };
            Some(template[..template.len() - 1].to_vec())
        }
    };
    #[cfg(not(unix))]
    let name = Some(
        std::env::temp_dir()
            .join(format!("lua_{}", crate::platform_time::unix_nanos()))
            .to_string_lossy()
            .into_owned()
            .into_bytes(),
    );
    let Some(name) = name else {
        return Err(lauxlib::lual_error(l, "unable to generate a unique filename"));
    };
    let name = l.create_bytes(&name)?;
    l.push_value(name)?;
    Ok(1)
}
