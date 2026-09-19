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
