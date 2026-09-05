//! Excel 1900 / 1904 date serials, including the 1900 leap-year bug.
//!
//! Serial 1 = 1900-01-01. Excel treats 1900 as a leap year, so serial 60 is
//! the fictitious 1900-02-29 and serial 61 is 1900-03-01.
//!
//! 1904 system: serial 0 = 1904-01-01 (= serial 1462 in the 1900 system).

use xlsx_types::{DateSystem, ExcelError};

/// Last Excel-representable civil date (9999-12-31) as a 1900-system serial.
pub const EXCEL_MAX_SERIAL_1900: i32 = 2_958_465;
/// 1904-01-01 in the 1900 date system.
pub const EXCEL1904_EPOCH_IN_1900: i32 = 1462;

pub fn is_excel_leap(year: i32) -> bool {
    if year == 1900 {
        return true;
    }
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i32, month: i32) -> i32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_excel_leap(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Excel `DATE` year rule: 0..=1899 are added to 1900; 1900..=9999 are literal.
pub fn normalize_date_year(year: i32) -> Result<i32, ExcelError> {
    if year < 0 || year > 9999 {
        return Err(ExcelError::Num);
    }
    if year <= 1899 {
        Ok(year + 1900)
    } else {
        Ok(year)
    }
}

/// Normalize month overflow the way Excel `DATE` does (`DATE(2000,13,1)` → 2001-01-01).
pub fn normalize_year_month(mut year: i32, month: i32) -> Result<(i32, i32), ExcelError> {
    // 0-based month that may be far outside 0..11.
    let mut month0 = month - 1;
    if month0 >= 0 {
        year = year.checked_add(month0 / 12).ok_or(ExcelError::Num)?;
        month0 %= 12;
    } else {
        // e.g. month = 0 → December of previous year.
        let borrow = ((-month0 - 1) / 12) + 1;
        year = year.checked_sub(borrow).ok_or(ExcelError::Num)?;
        month0 = month0 + borrow * 12;
    }
    if year < 0 || year > 9999 {
        return Err(ExcelError::Num);
    }
    Ok((year, month0 + 1))
}

/// Day-of-year (1-based) in the Excel calendar (1900 has a Feb 29).
fn day_of_year(year: i32, month: i32, day: i32) -> i32 {
    let mut n = day;
    for m in 1..month {
        n += days_in_month(year, m);
    }
    n
}

/// Convert a civil date already normalized into a 1900-system serial.
/// `day` may overflow / underflow relative to the month (Excel DATE semantics).
pub fn ymd_to_serial_1900(year: i32, month: i32, day: i32) -> Result<i32, ExcelError> {
    // Start at the 1st of the (normalized) month, then offset by day-1.
    let first = day_of_year(year, month, 1);
    let mut serial = serial_of_year_start(year)? + first - 1 + (day - 1);
    // serial_of_year_start(1900) + 0 = 1 for Jan 1; DATE(1900,1,0) = 0.
    if serial < 0 {
        return Err(ExcelError::Num);
    }
    if serial > EXCEL_MAX_SERIAL_1900 {
        return Err(ExcelError::Num);
    }
    // Re-borrow to silence unused mut if we don't adjust further.
    let _ = &mut serial;
    Ok(serial)
}

/// Serial of January 1 of `year` in the 1900 system (1900-01-01 = 1).
fn serial_of_year_start(year: i32) -> Result<i32, ExcelError> {
    if year < 1900 {
        // Dates before 1900-01-01: walk backwards from serial 1.
        let mut serial = 1;
        let mut y = 1900;
        while y > year {
            y -= 1;
            serial -= if is_excel_leap(y) { 366 } else { 365 };
        }
        return Ok(serial);
    }
    let mut serial = 1;
    for y in 1900..year {
        serial += if is_excel_leap(y) { 366 } else { 365 };
    }
    Ok(serial)
}

pub fn date_serial(year: i32, month: i32, day: i32, system: DateSystem) -> Result<f64, ExcelError> {
    let year = normalize_date_year(year)?;
    let (year, month) = normalize_year_month(year, month)?;
    let s1900 = ymd_to_serial_1900(year, month, day)?;
    match system {
        DateSystem::Excel1900 => Ok(s1900 as f64),
        DateSystem::Excel1904 => {
            let s = s1900 - EXCEL1904_EPOCH_IN_1900;
            if s < 0 {
                Err(ExcelError::Num)
            } else {
                Ok(s as f64)
            }
        }
    }
}

pub fn time_fraction(hour: f64, minute: f64, second: f64) -> Result<f64, ExcelError> {
    if !hour.is_finite() || !minute.is_finite() || !second.is_finite() {
        return Err(ExcelError::Num);
    }
    let total = hour * 3600.0 + minute * 60.0 + second;
    if total < 0.0 {
        return Err(ExcelError::Num);
    }
    // Excel TIME wraps at 24h; values that would overflow the TIME domain
    // (~32767 hours) become #NUM!.
    if hour.abs() >= 32767.0 {
        return Err(ExcelError::Num);
    }
    let secs_per_day = 86_400.0;
    Ok((total % secs_per_day) / secs_per_day)
}

pub fn serial_to_ymd(serial: f64, system: DateSystem) -> Result<(i32, u32, u32), ExcelError> {
    if !serial.is_finite() {
        return Err(ExcelError::Num);
    }
    let mut s = serial.trunc() as i32;
    match system {
        DateSystem::Excel1900 => {}
        DateSystem::Excel1904 => {
            s = s
                .checked_add(EXCEL1904_EPOCH_IN_1900)
                .ok_or(ExcelError::Num)?;
        }
    }
    if s < 0 || s > EXCEL_MAX_SERIAL_1900 {
        return Err(ExcelError::Num);
    }
    // Serial 0 in the 1900 system is the fictitious 1900-01-00.
    if s == 0 && matches!(system, DateSystem::Excel1900) {
        return Ok((1900, 1, 0));
    }
    if s == 60 && matches!(system, DateSystem::Excel1900) {
        return Ok((1900, 2, 29));
    }
    let mut rem = s;
    let mut year = 1900;
    loop {
        let len = if is_excel_leap(year) { 366 } else { 365 };
        if rem <= len {
            break;
        }
        rem -= len;
        year += 1;
        if year > 9999 {
            return Err(ExcelError::Num);
        }
    }
    let mut month = 1;
    while month <= 12 {
        let dim = days_in_month(year, month);
        if rem <= dim {
            return Ok((year, month as u32, rem as u32));
        }
        rem -= dim;
        month += 1;
    }
    Err(ExcelError::Num)
}

/// Convert a workbook-local date serial into the 1900 system.
pub fn to_1900_serial(serial: i32, system: DateSystem) -> Result<i32, ExcelError> {
    match system {
        DateSystem::Excel1900 => Ok(serial),
        DateSystem::Excel1904 => serial
            .checked_add(EXCEL1904_EPOCH_IN_1900)
            .ok_or(ExcelError::Num),
    }
}

/// Convert a 1900-system serial back into the workbook-local date system.
pub fn from_1900_serial(serial_1900: i32, system: DateSystem) -> Result<i32, ExcelError> {
    match system {
        DateSystem::Excel1900 => {
            if serial_1900 < 0 || serial_1900 > EXCEL_MAX_SERIAL_1900 {
                Err(ExcelError::Num)
            } else {
                Ok(serial_1900)
            }
        }
        DateSystem::Excel1904 => {
            let local = serial_1900
                .checked_sub(EXCEL1904_EPOCH_IN_1900)
                .ok_or(ExcelError::Num)?;
            if local < 0 || local > EXCEL_MAX_SERIAL_1900 - EXCEL1904_EPOCH_IN_1900 {
                Err(ExcelError::Num)
            } else {
                Ok(local)
            }
        }
    }
}

fn max_local_serial(system: DateSystem) -> i32 {
    match system {
        DateSystem::Excel1900 => EXCEL_MAX_SERIAL_1900,
        DateSystem::Excel1904 => EXCEL_MAX_SERIAL_1900 - EXCEL1904_EPOCH_IN_1900,
    }
}

/// Truncate a coerced date argument to a whole serial in `system`.
///
/// Negative / non-finite / past-9999-12-31 values are `#NUM!`. Serial 0 is
/// valid (1900-01-00, or 1904-01-01 in the 1904 system).
pub fn truncate_date_serial(n: f64, system: DateSystem) -> Result<i32, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Num);
    }
    let s = n.trunc();
    let max = max_local_serial(system) as f64;
    if s < 0.0 || s > max {
        return Err(ExcelError::Num);
    }
    Ok(s as i32)
}

/// Excel 1900-system weekend: serial 1 is Sunday, so `n % 7` is 0 (Sat) or 1 (Sun).
///
/// Serial 60 (the fictitious 1900-02-29) is a Wednesday — a workday.
pub fn is_weekend_sat_sun_1900(serial_1900: i32) -> bool {
    let w = serial_1900.rem_euclid(7);
    w == 0 || w == 1
}

/// Number of Mon–Fri serials in `(-∞, n]` (1900 system). Serial 0 is Saturday.
pub fn workdays_through(n: i32) -> i32 {
    if n < 0 {
        return 0;
    }
    let complete = (n + 1) / 7;
    let rem = (n + 1) % 7;
    // Partial week starting at serial 0: Sat, Sun, Mon, Tue, Wed, Thu, Fri.
    let extra = if rem <= 2 { 0 } else { rem - 2 };
    complete * 5 + extra
}

/// First 1900-system serial whose Mon–Fri count through that day equals `w`.
fn invert_workdays_through(w: i64) -> Result<i32, ExcelError> {
    if w <= 0 {
        return Err(ExcelError::Num);
    }
    let weeks = (w - 1) / 5;
    let rem = (w - 1) % 5;
    let t = weeks
        .checked_mul(7)
        .and_then(|x| x.checked_add(2 + rem))
        .ok_or(ExcelError::Num)?;
    if t < 0 || t > i64::from(EXCEL_MAX_SERIAL_1900) {
        return Err(ExcelError::Num);
    }
    Ok(t as i32)
}

fn workday_1900_no_holidays(start: i32, days: i32) -> Result<i32, ExcelError> {
    if days == 0 {
        return Ok(start);
    }
    if days > 0 {
        let target = i64::from(workdays_through(start)) + i64::from(days);
        invert_workdays_through(target)
    } else {
        let target = i64::from(workdays_through(start - 1)) + i64::from(days) + 1;
        invert_workdays_through(target)
    }
}

fn count_holidays_between(start: i32, end: i32, hols: &[i32]) -> i32 {
    if end > start {
        hols.iter().filter(|&&h| h > start && h <= end).count() as i32
    } else if end < start {
        hols.iter().filter(|&&h| h >= end && h < start).count() as i32
    } else {
        0
    }
}

fn workday_1900(start: i32, days: i32, hols: &[i32]) -> Result<i32, ExcelError> {
    if days == 0 {
        return Ok(start);
    }
    let mut extra = 0i32;
    loop {
        let signed_extra = if days > 0 { extra } else { -extra };
        let total = days.checked_add(signed_extra).ok_or(ExcelError::Num)?;
        let candidate = workday_1900_no_holidays(start, total)?;
        let counted = count_holidays_between(start, candidate, hols);
        if counted == extra {
            return Ok(candidate);
        }
        extra = counted;
    }
}

/// Excel `WORKDAY(start, days, [holidays])` with weekend Sat/Sun.
///
/// `days == 0` returns the truncated start date even when that day is a
/// weekend or a holiday. Non-zero `days` count workdays *after* (or before)
/// the start date. Holiday serials are truncated, de-duplicated, and ignored
/// on weekends. Uses an O(1) Mon–Fri inversion plus an O(H) holiday adjust.
pub fn workday_serial(
    start: f64,
    days: f64,
    holidays: &[f64],
    system: DateSystem,
) -> Result<f64, ExcelError> {
    let start_s = truncate_date_serial(start, system)?;
    if !days.is_finite() {
        return Err(ExcelError::Num);
    }
    let days_t = days.trunc();
    if days_t.abs() > f64::from(EXCEL_MAX_SERIAL_1900) {
        return Err(ExcelError::Num);
    }
    let days_i = days_t as i32;
    let start_1900 = to_1900_serial(start_s, system)?;

    let mut hols = Vec::with_capacity(holidays.len());
    for &h in holidays {
        let hs = to_1900_serial(truncate_date_serial(h, system)?, system)?;
        if !is_weekend_sat_sun_1900(hs) {
            hols.push(hs);
        }
    }
    hols.sort_unstable();
    hols.dedup();

    let result_1900 = workday_1900(start_1900, days_i, &hols)?;
    Ok(from_1900_serial(result_1900, system)? as f64)
}

/// Day-walk reference for benches and cross-checks. Not used on the hot path.
pub fn workday_serial_walk(
    start: f64,
    days: f64,
    holidays: &[f64],
    system: DateSystem,
) -> Result<f64, ExcelError> {
    let start_s = truncate_date_serial(start, system)?;
    if !days.is_finite() {
        return Err(ExcelError::Num);
    }
    let days_t = days.trunc();
    if days_t.abs() > f64::from(EXCEL_MAX_SERIAL_1900) {
        return Err(ExcelError::Num);
    }
    let mut remaining = days_t as i32;
    let start_1900 = to_1900_serial(start_s, system)?;

    let mut hols = Vec::with_capacity(holidays.len());
    for &h in holidays {
        let hs = to_1900_serial(truncate_date_serial(h, system)?, system)?;
        if !is_weekend_sat_sun_1900(hs) {
            hols.push(hs);
        }
    }
    hols.sort_unstable();
    hols.dedup();

    if remaining == 0 {
        return Ok(start_s as f64);
    }
    let dir = if remaining > 0 { 1 } else { -1 };
    remaining = remaining.abs();
    let mut cur = start_1900;
    while remaining > 0 {
        cur = cur.checked_add(dir).ok_or(ExcelError::Num)?;
        if cur < 0 || cur > EXCEL_MAX_SERIAL_1900 {
            return Err(ExcelError::Num);
        }
        if !is_weekend_sat_sun_1900(cur) && hols.binary_search(&cur).is_err() {
            remaining -= 1;
        }
    }
    Ok(from_1900_serial(cur, system)? as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_1900_serials() {
        assert_eq!(ymd_to_serial_1900(1900, 1, 1).unwrap(), 1);
        assert_eq!(ymd_to_serial_1900(1900, 2, 28).unwrap(), 59);
        assert_eq!(ymd_to_serial_1900(1900, 2, 29).unwrap(), 60);
        assert_eq!(ymd_to_serial_1900(1900, 3, 1).unwrap(), 61);
        assert_eq!(ymd_to_serial_1900(1901, 1, 1).unwrap(), 367);
        assert_eq!(ymd_to_serial_1900(2000, 1, 1).unwrap(), 36526);
        assert_eq!(ymd_to_serial_1900(2000, 2, 29).unwrap(), 36585);
        assert_eq!(ymd_to_serial_1900(2001, 1, 1).unwrap(), 36892);
    }

    #[test]
    fn date_overflow_month() {
        let s = date_serial(2000, 13, 1, DateSystem::Excel1900).unwrap();
        assert_eq!(s, 36892.0);
    }

    #[test]
    fn serial_roundtrip_leap_bug() {
        assert_eq!(
            serial_to_ymd(60.0, DateSystem::Excel1900).unwrap(),
            (1900, 2, 29)
        );
        assert_eq!(
            serial_to_ymd(61.0, DateSystem::Excel1900).unwrap(),
            (1900, 3, 1)
        );
        assert_eq!(
            serial_to_ymd(36526.0, DateSystem::Excel1900).unwrap(),
            (2000, 1, 1)
        );
        assert_eq!(
            serial_to_ymd(0.0, DateSystem::Excel1900).unwrap(),
            (1900, 1, 0)
        );
    }

    #[test]
    fn system_1904_epoch() {
        let s = date_serial(1904, 1, 1, DateSystem::Excel1904).unwrap();
        assert_eq!(s, 0.0);
        assert_eq!(
            serial_to_ymd(0.0, DateSystem::Excel1904).unwrap(),
            (1904, 1, 1)
        );
    }

    #[test]
    fn time_halves() {
        assert_eq!(time_fraction(0.0, 0.0, 0.0).unwrap(), 0.0);
        assert_eq!(time_fraction(12.0, 0.0, 0.0).unwrap(), 0.5);
        assert_eq!(time_fraction(6.0, 0.0, 0.0).unwrap(), 0.25);
        assert_eq!(time_fraction(18.0, 0.0, 0.0).unwrap(), 0.75);
    }

    fn d(y: i32, m: i32, day: i32) -> f64 {
        date_serial(y, m, day, DateSystem::Excel1900).unwrap()
    }

    fn wd(start: f64, days: f64, hols: &[f64]) -> f64 {
        workday_serial(start, days, hols, DateSystem::Excel1900).unwrap()
    }

    #[test]
    fn workday_ms_examples() {
        let start = d(2008, 10, 1);
        let end_plain = d(2009, 4, 30);
        let end_hols = d(2009, 5, 5);
        let h1 = d(2008, 11, 26);
        let h2 = d(2008, 12, 4);
        let h3 = d(2009, 1, 21);
        assert_eq!(start, 39722.0);
        assert_eq!(end_plain, 39933.0);
        assert_eq!(end_hols, 39938.0);
        assert_eq!(wd(start, 151.0, &[]), end_plain);
        assert_eq!(wd(start, 151.0, &[h1, h2, h3]), end_hols);
        assert_eq!(wd(d(2012, 1, 1), 3.0, &[]), d(2012, 1, 4));
        assert_eq!(wd(d(2012, 1, 1), 3.0, &[d(2012, 1, 2)]), d(2012, 1, 5));
    }

    #[test]
    fn workday_zero_returns_start_even_on_weekend_or_holiday() {
        assert_eq!(wd(d(2024, 1, 6), 0.0, &[]), d(2024, 1, 6));
        assert_eq!(wd(d(2024, 1, 7), 0.0, &[]), d(2024, 1, 7));
        assert_eq!(wd(d(2024, 1, 1), 0.0, &[d(2024, 1, 1)]), d(2024, 1, 1));
        assert_eq!(wd(60.0, 0.0, &[60.0]), 60.0);
        assert_eq!(wd(0.0, 0.0, &[]), 0.0);
    }

    #[test]
    fn workday_weekend_start() {
        // Fri/Sat/Sun + 1 all land on the following Monday.
        assert_eq!(wd(d(2024, 1, 5), 1.0, &[]), d(2024, 1, 8));
        assert_eq!(wd(d(2024, 1, 6), 1.0, &[]), d(2024, 1, 8));
        assert_eq!(wd(d(2024, 1, 7), 1.0, &[]), d(2024, 1, 8));
        // Sat/Sun/Mon - 1 all land on Friday.
        assert_eq!(wd(d(2024, 1, 6), -1.0, &[]), d(2024, 1, 5));
        assert_eq!(wd(d(2024, 1, 7), -1.0, &[]), d(2024, 1, 5));
        assert_eq!(wd(d(2024, 1, 8), -1.0, &[]), d(2024, 1, 5));
        assert_eq!(wd(1.0, 1.0, &[]), 2.0);
        assert_eq!(wd(7.0, 1.0, &[]), 9.0);
        assert_eq!(wd(6.0, 1.0, &[]), 9.0);
    }

    #[test]
    fn workday_serial_60_leap_bug() {
        let s = DateSystem::Excel1900;
        assert!(!is_weekend_sat_sun_1900(60));
        assert_eq!(workday_serial(59.0, 1.0, &[], s).unwrap(), 60.0);
        assert_eq!(workday_serial(60.0, 1.0, &[], s).unwrap(), 61.0);
        assert_eq!(workday_serial(59.0, 2.0, &[], s).unwrap(), 61.0);
        assert_eq!(workday_serial(58.0, 1.0, &[], s).unwrap(), 59.0);
        assert_eq!(workday_serial(60.0, -1.0, &[], s).unwrap(), 59.0);
        assert_eq!(workday_serial(61.0, -1.0, &[], s).unwrap(), 60.0);
        assert_eq!(workday_serial(59.0, 1.0, &[60.0], s).unwrap(), 61.0);
        assert_eq!(workday_serial(d(1900, 2, 1), 20.0, &[], s).unwrap(), 60.0);
        assert_eq!(
            workday_serial(d(1900, 2, 1), 20.0, &[60.0], s).unwrap(),
            61.0
        );
    }

    #[test]
    fn workday_holidays() {
        let thu = d(2024, 1, 4);
        let fri = d(2024, 1, 5);
        let mon = d(2024, 1, 8);
        let tue = d(2024, 1, 9);
        assert_eq!(wd(thu, 1.0, &[fri]), mon);
        assert_eq!(wd(thu, 1.0, &[fri, mon]), tue);
        assert_eq!(wd(thu, 1.0, &[d(2024, 1, 6)]), fri);
        assert_eq!(wd(thu, 1.0, &[fri, fri]), mon);
        assert_eq!(wd(d(2024, 1, 1), 1.0, &[d(2024, 1, 1)]), d(2024, 1, 2));
        assert_eq!(wd(thu, 1.0, &[d(2023, 12, 25)]), fri);
    }

    #[test]
    fn workday_truncates_fractions() {
        assert_eq!(wd(d(2024, 1, 4) + 0.9, 1.9, &[]), d(2024, 1, 5));
        assert_eq!(wd(d(2024, 1, 8) + 0.9, -1.9, &[]), d(2024, 1, 5));
    }

    #[test]
    fn workday_rejects_out_of_range() {
        let s = DateSystem::Excel1900;
        assert_eq!(workday_serial(-1.0, 1.0, &[], s), Err(ExcelError::Num));
        assert_eq!(workday_serial(2.0, -1.0, &[], s), Err(ExcelError::Num));
        assert_eq!(workday_serial(1.0, -1.0, &[], s), Err(ExcelError::Num));
        assert_eq!(
            workday_serial(EXCEL_MAX_SERIAL_1900 as f64, 1.0, &[], s),
            Err(ExcelError::Num)
        );
        assert_eq!(workday_serial(1.0, 1.0, &[-1.0], s), Err(ExcelError::Num));
        assert_eq!(
            workday_serial(EXCEL_MAX_SERIAL_1900 as f64, 0.0, &[], s).unwrap(),
            EXCEL_MAX_SERIAL_1900 as f64
        );
    }

    #[test]
    fn workday_system_1904() {
        let s = DateSystem::Excel1904;
        assert_eq!(workday_serial(0.0, 0.0, &[], s).unwrap(), 0.0);
        assert_eq!(workday_serial(0.0, 1.0, &[], s).unwrap(), 3.0);
        assert_eq!(workday_serial(0.0, 1.0, &[0.0], s).unwrap(), 3.0);
        assert_eq!(workday_serial(3.0, -1.0, &[], s).unwrap(), 0.0);
    }

    #[test]
    fn workday_matches_walk_on_prefix() {
        let s = DateSystem::Excel1900;
        for start in 0..40 {
            for days in -15..=15 {
                let fast = workday_serial(start as f64, days as f64, &[], s);
                let walk = workday_serial_walk(start as f64, days as f64, &[], s);
                assert_eq!(fast, walk, "WORKDAY({start}, {days})");
            }
        }
        let hols = [60.0, 61.0, 64.0];
        for start in 50..80 {
            for days in -10..=10 {
                let fast = workday_serial(start as f64, days as f64, &hols, s);
                let walk = workday_serial_walk(start as f64, days as f64, &hols, s);
                assert_eq!(fast, walk, "WORKDAY({start}, {days}) with hols");
            }
        }
    }

    #[test]
    fn invert_workdays_matches_through() {
        for n in 0..=80 {
            if is_weekend_sat_sun_1900(n) {
                continue;
            }
            let w = workdays_through(n);
            assert_eq!(invert_workdays_through(w as i64).unwrap(), n, "n={n}");
        }
    }
}
