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

/// Excel `EOMONTH(start_date, months)`. See `xlsx-engine-core::dates`.
pub fn eomonth_serial(start: f64, months: f64, system: DateSystem) -> Result<f64, ExcelError> {
    if !start.is_finite() || !months.is_finite() || start < 0.0 {
        return Err(ExcelError::Num);
    }
    let (year, month, _day) = serial_to_ymd(start, system)?;
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
pub fn is_weekend_sat_sun_1900(serial_1900: i32) -> bool {
    let w = serial_1900.rem_euclid(7);
    w == 0 || w == 1
}

fn workdays_through(n: i32) -> i32 {
    if n < 0 {
        return 0;
    }
    let complete = (n + 1) / 7;
    let rem = (n + 1) % 7;
    let extra = if rem <= 2 { 0 } else { rem - 2 };
    complete * 5 + extra
}

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

/// Mon–Fri count in `[lo, hi]` inclusive, 1900-system serials. O(1).
pub fn weekday_count_sat_sun(lo_1900: i32, hi_1900: i32) -> i32 {
    if hi_1900 < lo_1900 {
        return 0;
    }
    workdays_through(hi_1900) - workdays_through(lo_1900 - 1)
}

/// Excel `NETWORKDAYS(start, end, [holidays])` with weekend Sat/Sun.
pub fn networkdays_count(
    start: f64,
    end: f64,
    holidays: &[f64],
    system: DateSystem,
) -> Result<f64, ExcelError> {
    let start_s = truncate_date_serial(start, system)?;
    let end_s = truncate_date_serial(end, system)?;
    let start_1900 = to_1900_serial(start_s, system)?;
    let end_1900 = to_1900_serial(end_s, system)?;
    let sign = if start_s <= end_s { 1 } else { -1 };
    let (lo, hi) = if start_1900 <= end_1900 {
        (start_1900, end_1900)
    } else {
        (end_1900, start_1900)
    };
    let mut work = weekday_count_sat_sun(lo, hi);
    let mut hols = Vec::with_capacity(holidays.len());
    for &h in holidays {
        hols.push(to_1900_serial(truncate_date_serial(h, system)?, system)?);
    }
    hols.sort_unstable();
    hols.dedup();
    for h in hols {
        if h >= lo && h <= hi && !is_weekend_sat_sun_1900(h) {
            work -= 1;
        }
    }
    Ok((sign * work) as f64)
}

pub fn serial_as_1900_int(serial: f64, system: DateSystem) -> Result<i32, ExcelError> {
    if !serial.is_finite() || serial < 0.0 {
        return Err(ExcelError::Num);
    }
    let local = serial.trunc();
    if local > i32::MAX as f64 {
        return Err(ExcelError::Num);
    }
    let mut s = local as i32;
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
    Ok(s)
}

#[inline]
pub fn type1_from_1900_serial(serial_1900: i32) -> i32 {
    let r = serial_1900 % 7;
    if r == 0 {
        7
    } else {
        r
    }
}

pub fn map_weekday_return_type(type1: i32, return_type: i32) -> Result<f64, ExcelError> {
    let sun0 = type1 - 1;
    let n = match return_type {
        1 | 17 => type1,
        2 | 11 => (sun0 + 6) % 7 + 1,
        3 => (sun0 + 6) % 7,
        12 => (sun0 + 5) % 7 + 1,
        13 => (sun0 + 4) % 7 + 1,
        14 => (sun0 + 3) % 7 + 1,
        15 => (sun0 + 2) % 7 + 1,
        16 => (sun0 + 1) % 7 + 1,
        _ => return Err(ExcelError::Num),
    };
    Ok(n as f64)
}

/// Excel `WEEKDAY` from a date serial. O(1) on the integer serial.
pub fn weekday(serial: f64, return_type: i32, system: DateSystem) -> Result<f64, ExcelError> {
    let s1900 = serial_as_1900_int(serial, system)?;
    map_weekday_return_type(type1_from_1900_serial(s1900), return_type)
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

    #[test]
    fn weekday_serial_one_is_sunday() {
        assert_eq!(weekday(1.0, 1, DateSystem::Excel1900).unwrap(), 1.0);
        assert_eq!(weekday(60.0, 1, DateSystem::Excel1900).unwrap(), 4.0);
        assert_eq!(weekday(61.0, 1, DateSystem::Excel1900).unwrap(), 5.0);
    }
}
