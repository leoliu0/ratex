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
