//! Tiny civil-calendar helpers so `usage_daily` and `logs` don't need a date/time crate.
//! `days_from_civil`/`civil_from_days` are Howard Hinnant's well-known constant-time formulas
//! (<https://howardhinnant.github.io/date_algorithms.html>), valid for any proleptic Gregorian
//! date `chrono`/`time` would also accept.

pub type Ymd = (i32, u32, u32);

pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m as i64 + 9) % 12; // [0, 11], Mar=0
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> Ymd {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

#[cfg(test)]
fn add_days((y, m, d): Ymd, delta: i64) -> Ymd {
    civil_from_days(days_from_civil(y, m, d) + delta)
}

pub fn fmt_date((y, m, d): Ymd) -> String {
    format!("{y:04}-{m:02}-{d:02}")
}

/// The "YYYY-MM-DD" day a unix timestamp falls on, at `utc_offset_secs` east of UTC (0 = UTC;
/// there's no timezone database in this binary, so "local midnight" is only as accurate as the
/// configured offset — see `Config.local_utc_offset_secs`).
pub fn day_key(ts: u64, utc_offset_secs: i64) -> String {
    let local = ts as i64 + utc_offset_secs;
    let days = local.div_euclid(86400);
    fmt_date(civil_from_days(days))
}

/// Earliest local calendar day included in a `days`-wide window ending on `ts`.
/// Use this for filtering rows whose day keys were assigned with the same UTC offset.
pub fn window_start_day(ts: u64, days: u32, utc_offset_secs: i64) -> String {
    let local_days = (ts as i64 + utc_offset_secs).div_euclid(86400);
    fmt_date(civil_from_days(local_days - (days.max(1) as i64 - 1)))
}

/// `logread`'s `ctime`-style month abbreviation ("Jan".."Dec") -> 1..12.
pub fn month_number(abbr: &str) -> Option<u32> {
    const NAMES: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    NAMES.iter().position(|n| *n == abbr).map(|i| i as u32 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_date_roundtrips() {
        for &ymd in &[
            (2026, 9, 24),
            (2026, 1, 1),
            (2025, 12, 31),
            (2000, 2, 29),
            (1970, 1, 1),
        ] {
            let days = days_from_civil(ymd.0, ymd.1, ymd.2);
            assert_eq!(civil_from_days(days), ymd, "roundtrip for {ymd:?}");
        }
    }

    #[test]
    fn add_days_crosses_month_and_year_boundaries() {
        assert_eq!(add_days((2026, 9, 24), -30), (2026, 8, 25));
        assert_eq!(add_days((2026, 1, 1), -1), (2025, 12, 31));
    }

    #[test]
    fn month_names_parse() {
        assert_eq!(month_number("Sep"), Some(9));
        assert_eq!(month_number("Jan"), Some(1));
        assert_eq!(month_number("Nope"), None);
    }

    #[test]
    fn day_key_at_utc() {
        let ts = days_from_civil(2026, 9, 24) as u64 * 86400 + 43200; // noon UTC
        assert_eq!(day_key(ts, 0), "2026-09-24");
    }

    #[test]
    fn day_key_with_offset_crosses_midnight() {
        // 23:30 UTC on the 23rd, IST (+5:30) is already 05:00 on the 24th.
        let ts = days_from_civil(2026, 9, 23) as u64 * 86400 + 23 * 3600 + 1800;
        assert_eq!(day_key(ts, 5 * 3600 + 1800), "2026-09-24");
    }

    #[test]
    fn usage_window_starts_from_the_configured_local_day() {
        let cases = [0, 5 * 3600 + 1800, -8 * 3600];
        for offset in cases {
            let local_ts = days_from_civil(2026, 9, 24) * 86400 + 1;
            let ts = (local_ts - offset) as u64;
            assert_eq!(window_start_day(ts, 1, offset), "2026-09-24");
        }
    }

    #[test]
    fn usage_window_boundary_tracks_local_midnight() {
        let local_day = (2026, 9, 24);
        for offset in [0, 5 * 3600 + 1800, -8 * 3600] {
            let local_midnight_utc =
                (days_from_civil(local_day.0, local_day.1, local_day.2) * 86400 - offset) as u64;
            assert_eq!(
                window_start_day(local_midnight_utc - 1, 1, offset),
                "2026-09-23"
            );
            assert_eq!(
                window_start_day(local_midnight_utc, 1, offset),
                "2026-09-24"
            );
            assert_eq!(
                window_start_day(local_midnight_utc + 1, 1, offset),
                "2026-09-24"
            );
        }
    }
}
