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

pub fn days_in_month(year: i32, month: i32) -> i32 {
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

/// Gregorian leap years in `(-∞, year)` (year 1 and later).
fn gregorian_leaps_before(year: i32) -> i32 {
    if year <= 0 {
        return 0;
    }
    let y = year - 1;
    y / 4 - y / 100 + y / 400
}

/// Excel leap years in `(-∞, year)`. 1900 counts as a leap year.
fn excel_leaps_before(year: i32) -> i32 {
    gregorian_leaps_before(year) + i32::from(year > 1900)
}

/// Serial of January 1 of `year` in the 1900 system (1900-01-01 = 1).
///
/// Closed-form (O(1)) so `EOMONTH` / `DATE` / `YEAR` stay cheap on late-century
/// serials instead of walking every year from 1900.
fn serial_of_year_start(year: i32) -> Result<i32, ExcelError> {
    if year < 1 {
        // Rare pre-year-1 path kept for day-underflow walks.
        let mut serial = 1;
        let mut y = 1900;
        while y > year {
            y -= 1;
            serial -= if is_excel_leap(y) { 366 } else { 365 };
        }
        return Ok(serial);
    }
    let dy = year - 1900;
    let leaps = excel_leaps_before(year) - excel_leaps_before(1900);
    dy.checked_mul(365)
        .and_then(|d| d.checked_add(1))
        .and_then(|d| d.checked_add(leaps))
        .ok_or(ExcelError::Num)
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
    // 365.25-day estimate, then at most a couple of O(1) year-start adjustments.
    let mut year = 1900 + ((s as i64 - 1) * 4 / 1461) as i32;
    if year < 1900 {
        year = 1900;
    }
    if year > 9999 {
        year = 9999;
    }
    let rem = loop {
        let start = serial_of_year_start(year)?;
        let len = if is_excel_leap(year) { 366 } else { 365 };
        if s < start {
            year -= 1;
            if year < 0 {
                return Err(ExcelError::Num);
            }
            continue;
        }
        if s >= start + len {
            year += 1;
            if year > 9999 {
                return Err(ExcelError::Num);
            }
            continue;
        }
        break s - start + 1;
    };
    let mut rem = rem;
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

/// Excel `EOMONTH(start_date, months)`: last civil day of the month that is
/// `months` months before or after `start_date`.
///
/// `months` is truncated toward zero. Reuses [`serial_to_ymd`],
/// [`normalize_year_month`], [`days_in_month`], and [`ymd_to_serial_1900`]
/// so the 1900 leap-year bug (serial 60) is inherited, not re-implemented.
pub fn eomonth_serial(start: f64, months: f64, system: DateSystem) -> Result<f64, ExcelError> {
    if !start.is_finite() || !months.is_finite() || start < 0.0 {
        return Err(ExcelError::Num);
    }
    let (year, month, _day) = serial_to_ymd(start, system)?;
    // ±120_000 months is ~10_000 years — past every Excel-representable date.
    let months_i = months.trunc();
    if months_i.abs() > 120_000.0 {
        return Err(ExcelError::Num);
    }
    let month_sum = month as i32 + months_i as i32;
    let (year, month) = normalize_year_month(year, month_sum)?;
    if year < 1900 || year > 9999 {
        return Err(ExcelError::Num);
    }
    let last = days_in_month(year, month);
    let s1900 = ymd_to_serial_1900(year, month, last)?;
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

    #[test]
    fn eomonth_ms_examples() {
        // support.microsoft.com EOMONTH examples (1-Jan-11 ± months).
        assert_eq!(
            eomonth_serial(d(2011, 1, 1), 1.0, DateSystem::Excel1900).unwrap(),
            d(2011, 2, 28)
        );
        assert_eq!(
            eomonth_serial(d(2011, 1, 1), -3.0, DateSystem::Excel1900).unwrap(),
            d(2010, 10, 31)
        );
    }

    #[test]
    fn eomonth_month_end_edges() {
        assert_eq!(
            eomonth_serial(d(2011, 1, 31), 1.0, DateSystem::Excel1900).unwrap(),
            d(2011, 2, 28)
        );
        assert_eq!(
            eomonth_serial(d(2012, 1, 31), 1.0, DateSystem::Excel1900).unwrap(),
            d(2012, 2, 29)
        );
        assert_eq!(
            eomonth_serial(d(2011, 3, 31), -1.0, DateSystem::Excel1900).unwrap(),
            d(2011, 2, 28)
        );
        assert_eq!(
            eomonth_serial(d(2024, 3, 31), -1.0, DateSystem::Excel1900).unwrap(),
            d(2024, 2, 29)
        );
    }

    #[test]
    fn eomonth_serial_60_leap_bug() {
        let s = DateSystem::Excel1900;
        assert_eq!(eomonth_serial(60.0, 0.0, s).unwrap(), 60.0);
        assert_eq!(eomonth_serial(59.0, 0.0, s).unwrap(), 60.0);
        assert_eq!(eomonth_serial(60.0, 1.0, s).unwrap(), d(1900, 3, 31));
        assert_eq!(eomonth_serial(60.0, -1.0, s).unwrap(), d(1900, 1, 31));
        assert_eq!(eomonth_serial(d(1900, 1, 31), 1.0, s).unwrap(), 60.0);
        assert_eq!(eomonth_serial(d(1900, 3, 1), -1.0, s).unwrap(), 60.0);
    }

    #[test]
    fn eomonth_truncates_toward_zero() {
        let s = DateSystem::Excel1900;
        assert_eq!(eomonth_serial(d(2011, 1, 1), 1.9, s).unwrap(), d(2011, 2, 28));
        assert_eq!(
            eomonth_serial(d(2011, 1, 1), -3.9, s).unwrap(),
            d(2010, 10, 31)
        );
        assert_eq!(
            eomonth_serial(d(2011, 1, 15) + 0.9, 0.0, s).unwrap(),
            d(2011, 1, 31)
        );
    }

    #[test]
    fn eomonth_rejects_out_of_range() {
        let s = DateSystem::Excel1900;
        assert_eq!(eomonth_serial(d(1900, 1, 1), -1.0, s), Err(ExcelError::Num));
        assert_eq!(eomonth_serial(-1.0, 0.0, s), Err(ExcelError::Num));
        assert_eq!(
            eomonth_serial(d(9999, 12, 31), 1.0, s),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn eomonth_system_1904() {
        let s = DateSystem::Excel1904;
        assert_eq!(eomonth_serial(0.0, 0.0, s).unwrap(), 30.0);
        assert_eq!(
            eomonth_serial(date_serial(1904, 2, 1, s).unwrap(), 0.0, s).unwrap(),
            date_serial(1904, 2, 29, s).unwrap()
        );
        assert_eq!(eomonth_serial(0.0, -1.0, s), Err(ExcelError::Num));
    }

    #[test]
    fn serial_of_year_start_matches_known_epoch() {
        assert_eq!(serial_of_year_start(1900).unwrap(), 1);
        assert_eq!(serial_of_year_start(1901).unwrap(), 367);
        assert_eq!(serial_of_year_start(2000).unwrap(), 36526);
        assert_eq!(
            ymd_to_serial_1900(2001, 1, 1).unwrap(),
            serial_of_year_start(2001).unwrap()
        );
    }
}
