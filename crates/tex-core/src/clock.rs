/// Proleptic Gregorian date and minutes since midnight, without OS services.
pub(crate) fn utc(epoch: i64) -> (i32, i32, i32, i32) {
    // Clamp to the civil-date range usable by TeX integer registers.
    let epoch = epoch.clamp(-62_167_219_200, 253_402_300_799);
    let days = epoch.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (
        (y + i64::from(m <= 2)) as i32,
        m as i32,
        d as i32,
        (epoch.rem_euclid(86_400) / 60) as i32,
    )
}

/// `SOURCE_DATE_EPOCH` when `FORCE_SOURCE_DATE=1` asks TeX's date and time
/// parameters to follow it (web2c `get_date_and_time`).
pub(crate) fn forced_source_date_epoch() -> Option<i64> {
    forced_epoch(
        std::env::var_os("FORCE_SOURCE_DATE").as_deref(),
        std::env::var("SOURCE_DATE_EPOCH").ok().as_deref(),
    )
}

fn forced_epoch(force: Option<&std::ffi::OsStr>, epoch: Option<&str>) -> Option<i64> {
    if force.is_none_or(|v| v != "1") {
        return None;
    }
    epoch?.trim().parse().ok()
}

/// web2c `makepdftime`: `D:YYYYmmddHHMMSS` in local time followed by the
/// zone offset as `+HH'MM'` (or `Z` for UTC). `utc_zone` formats in UTC, as
/// pdfTeX does for SOURCE_DATE_EPOCH-derived dates. Like pdfTeX, the hour
/// part of the offset truncates toward zero and carries the sign, so
/// -00:30 prints as `+00'30'`.
pub(crate) fn pdf_date(epoch: i64, utc_zone: bool) -> String {
    use std::fmt::Write;

    let offset = if utc_zone { 0 } else { local_offset_minutes(epoch) };
    let local = epoch + offset * 60;
    let (year, month, day, minutes) = utc(local);
    let mut date = format!(
        "D:{year:04}{month:02}{day:02}{:02}{:02}{:02}",
        minutes / 60,
        minutes % 60,
        local.rem_euclid(60)
    );
    if offset == 0 {
        date.push('Z');
    } else {
        let hours = offset / 60;
        let _ = write!(date, "{hours:+03}'{:02}'", (offset - hours * 60).abs());
    }
    date
}

/// The local UTC offset in minutes at `epoch`.
#[cfg(unix)]
fn local_offset_minutes(epoch: i64) -> i64 {
    let time = epoch as libc::time_t;
    // SAFETY: `localtime_r` reads `time` and writes only to `tm`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i64 / 60
}

/// The Windows CRT's `localtime_s` knows the system zone (and a POSIX-style
/// `TZ` such as `CST-8`); the offset is the broken-down local time read as
/// UTC minus `epoch`.
#[cfg(windows)]
fn local_offset_minutes(epoch: i64) -> i64 {
    let time = epoch as libc::time_t;
    // SAFETY: `localtime_s` reads `time` and writes only to `tm`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_s(&mut tm, &time) } != 0 {
        return 0;
    }
    let (year, month, day) = (i64::from(tm.tm_year) + 1900, i64::from(tm.tm_mon) + 1, i64::from(tm.tm_mday));
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    let local = days * 86_400 + i64::from(tm.tm_hour) * 3600 + i64::from(tm.tm_min) * 60 + i64::from(tm.tm_sec);
    (local - epoch) / 60
}

/// Without a zone database (wasm) the offset is UTC.
#[cfg(not(any(unix, windows)))]
fn local_offset_minutes(_epoch: i64) -> i64 {
    0
}

/// Seconds since the Unix epoch now; the sandboxed web build has no clock
/// and, like `\time`, uses the epoch itself.
pub(crate) fn now() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        system_time_epoch(std::time::SystemTime::now())
    }
}

/// web2c `seconds_and_micros`: wall-clock seconds and microseconds since
/// the Unix epoch (pdfTeX's random seed and `\pdfelapsedtime` timer). The
/// sandboxed web build has no clock and reports the epoch itself.
pub(crate) fn now_micros() -> (i64, i32) {
    #[cfg(target_arch = "wasm32")]
    {
        (0, 0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(elapsed) => (elapsed.as_secs() as i64, elapsed.subsec_micros() as i32),
            Err(_) => (0, 0),
        }
    }
}

/// A file-system timestamp as `time_t` seconds (floored before 1970).
pub(crate) fn system_time_epoch(time: std::time::SystemTime) -> i64 {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_secs() as i64,
        Err(before) => {
            let before = before.duration();
            -(before.as_secs() as i64) - i64::from(before.subsec_nanos() > 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    #[test]
    fn source_date_epoch_drives_the_clock_only_when_forced() {
        assert_eq!(super::forced_epoch(None, Some("1700000000")), None);
        assert_eq!(super::forced_epoch(Some(OsStr::new("0")), Some("1700000000")), None);
        assert_eq!(
            super::forced_epoch(Some(OsStr::new("1")), Some(" 1700000000 ")),
            Some(1_700_000_000)
        );
        assert_eq!(super::forced_epoch(Some(OsStr::new("1")), None), None);
        // pdftex with FORCE_SOURCE_DATE=1 SOURCE_DATE_EPOCH=1700000000:
        // \year=2023 \month=11 \day=14 \time=1333
        assert_eq!(super::utc(1_700_000_000), (2023, 11, 14, 1333));
    }
}
