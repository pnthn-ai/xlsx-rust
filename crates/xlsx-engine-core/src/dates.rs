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
    // Serials 1..=59 stay in January/February 1900 (before the leap-bug day).
    // Serial >= 61 matches proleptic Gregorian (Excel 25569 = 1970-01-01).
    if s >= 61 {
        return Ok(civil_from_unix_days(s - 25569));
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

/// Civil date from days since 1970-01-01 (Howard Hinnant).
fn civil_from_unix_days(z: i32) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i32 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
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

/// Weekend mask bits follow `serial_1900 % 7` (serial 0 = Saturday):
/// bit 0 Sat, 1 Sun, 2 Mon, 3 Tue, 4 Wed, 5 Thu, 6 Fri.
pub const WEEKEND_SAT_SUN: u8 = 0b0000_0011;

/// Excel 1900-system weekend: serial 1 is Sunday, so `n % 7` is 0 (Sat) or 1 (Sun).
///
/// Serial 60 (the fictitious 1900-02-29) is a Wednesday — a workday.
#[inline]
pub fn is_weekend_sat_sun_1900(serial_1900: i32) -> bool {
    is_weekend_mask_1900(serial_1900, WEEKEND_SAT_SUN)
}

/// True when `serial_1900 % 7` is a weekend bit in `weekend_mask`.
#[inline]
pub fn is_weekend_mask_1900(serial_1900: i32, weekend_mask: u8) -> bool {
    let w = serial_1900.rem_euclid(7);
    weekend_mask & (1 << w) != 0
}

/// Excel `NETWORKDAYS.INTL` / `WORKDAY.INTL` weekend number (1–7, 11–17).
///
/// Invalid codes are `#NUM!` (Microsoft: weekend `0` is `#NUM!`).
pub fn weekend_mask_from_code(code: i32) -> Result<u8, ExcelError> {
    match code {
        1 => Ok(WEEKEND_SAT_SUN),
        2 => Ok(0b0000_0110),
        3 => Ok(0b0000_1100),
        4 => Ok(0b0001_1000),
        5 => Ok(0b0011_0000),
        6 => Ok(0b0110_0000),
        7 => Ok(0b0100_0001),
        11 => Ok(0b0000_0010),
        12 => Ok(0b0000_0100),
        13 => Ok(0b0000_1000),
        14 => Ok(0b0001_0000),
        15 => Ok(0b0010_0000),
        16 => Ok(0b0100_0000),
        17 => Ok(0b0000_0001),
        _ => Err(ExcelError::Num),
    }
}

/// Weekend string: seven characters Monday→Sunday; `1` = weekend, `0` = workday.
///
/// Wrong length or a character other than `0`/`1` is `#VALUE!`. `"1111111"` is
/// a valid mask (Microsoft: `NETWORKDAYS.INTL` then always returns 0).
pub fn weekend_mask_from_string(s: &str) -> Result<u8, ExcelError> {
    let b = s.as_bytes();
    if b.len() != 7 {
        return Err(ExcelError::Value);
    }
    // Mon, Tue, Wed, Thu, Fri, Sat, Sun → bits 2, 3, 4, 5, 6, 0, 1
    const BITS: [u32; 7] = [2, 3, 4, 5, 6, 0, 1];
    let mut mask = 0u8;
    for (i, &ch) in b.iter().enumerate() {
        match ch {
            b'1' => mask |= 1 << BITS[i],
            b'0' => {}
            _ => return Err(ExcelError::Value),
        }
    }
    Ok(mask)
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

pub fn weekday_count_sat_sun(lo_1900: i32, hi_1900: i32) -> i32 {
    weekday_count_mask(lo_1900, hi_1900, WEEKEND_SAT_SUN)
}

/// Workdays in `(-∞, n]` for an arbitrary weekend mask. Serial 0 is Saturday.
pub fn workdays_through_mask(n: i32, weekend_mask: u8) -> i32 {
    if n < 0 {
        return 0;
    }
    let work_per_week = 7 - weekend_mask.count_ones() as i32;
    if work_per_week == 0 {
        return 0;
    }
    let complete = (n + 1) / 7;
    let rem = (n + 1) % 7;
    let mut extra = 0;
    for r in 0..rem {
        if weekend_mask & (1 << r) == 0 {
            extra += 1;
        }
    }
    complete * work_per_week + extra
}

/// Workday count in `[lo, hi]` inclusive under `weekend_mask`. O(1).
pub fn weekday_count_mask(lo_1900: i32, hi_1900: i32, weekend_mask: u8) -> i32 {
    if hi_1900 < lo_1900 {
        return 0;
    }
    workdays_through_mask(hi_1900, weekend_mask) - workdays_through_mask(lo_1900 - 1, weekend_mask)
}

/// Excel `NETWORKDAYS(start, end, [holidays])` with weekend Sat/Sun.
///
/// Inclusive of both ends. `start > end` yields the negated forward count.
/// Holiday serials are truncated, de-duplicated, and ignored when they fall
/// on a weekend or outside `[min(start,end), max(start,end)]`.
pub fn networkdays_count(
    start: f64,
    end: f64,
    holidays: &[f64],
    system: DateSystem,
) -> Result<f64, ExcelError> {
    networkdays_count_mask(start, end, WEEKEND_SAT_SUN, holidays, system)
}

/// Excel `NETWORKDAYS.INTL` count for a pre-parsed weekend mask.
///
/// Same inclusive / reverse-sign / holiday rules as [`networkdays_count`].
/// `"1111111"` (mask `0x7f`) yields 0 for any span.
pub fn networkdays_count_mask(
    start: f64,
    end: f64,
    weekend_mask: u8,
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
    let mut work = weekday_count_mask(lo, hi, weekend_mask);
    let mut hols = Vec::with_capacity(holidays.len());
    for &h in holidays {
        hols.push(to_1900_serial(truncate_date_serial(h, system)?, system)?);
    }
    hols.sort_unstable();
    hols.dedup();
    for h in hols {
        if h >= lo && h <= hi && !is_weekend_mask_1900(h, weekend_mask) {
            work -= 1;
        }
    }
    Ok((sign * work) as f64)
}

/// Day-walk reference for benches and cross-checks. Not used on the hot path.
pub fn networkdays_count_mask_walk(
    start: f64,
    end: f64,
    weekend_mask: u8,
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
    let mut work = 0i32;
    for s in lo..=hi {
        if !is_weekend_mask_1900(s, weekend_mask) {
            work += 1;
        }
    }
    let mut hols = Vec::with_capacity(holidays.len());
    for &h in holidays {
        hols.push(to_1900_serial(truncate_date_serial(h, system)?, system)?);
    }
    hols.sort_unstable();
    hols.dedup();
    for h in hols {
        if h >= lo && h <= hi && !is_weekend_mask_1900(h, weekend_mask) {
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

/// Excel `WEEKDAY` type-1 day (1 = Sunday … 7 = Saturday) from a 1900-system serial.
///
/// Serial 1 (1900-01-01) is Sunday in Excel — historically Monday. The fictitious
/// 1900-02-29 (serial 60) keeps later dates on the civil weekday. This is a
/// single modulo; it does not walk the calendar.
#[inline]
pub fn type1_from_1900_serial(serial_1900: i32) -> i32 {
    let r = serial_1900 % 7;
    if r == 0 {
        7
    } else {
        r
    }
}

/// Map a type-1 weekday (Sun=1) onto Excel `return_type` 1/2/3/11–17.
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
///
/// Reuses the 1900 / 1904 epoch helpers; does **not** convert to YMD (that walk
/// is O(year − 1900) and is the naive path in [`weekday_naive`]).
pub fn weekday(serial: f64, return_type: i32, system: DateSystem) -> Result<f64, ExcelError> {
    let s1900 = serial_as_1900_int(serial, system)?;
    map_weekday_return_type(type1_from_1900_serial(s1900), return_type)
}

/// Calendar-walk `WEEKDAY`: `serial_to_ymd` then `ymd_to_serial_1900`, then the
/// same modulo map. Semantically identical; used as the bench baseline.
pub fn weekday_naive(serial: f64, return_type: i32, system: DateSystem) -> Result<f64, ExcelError> {
    if !serial.is_finite() || serial < 0.0 {
        return Err(ExcelError::Num);
    }
    let (y, m, d) = serial_to_ymd(serial, system)?;
    let s1900 = ymd_to_serial_1900(y, m as i32, d as i32)?;
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
        assert_eq!(
            serial_to_ymd(45366.0, DateSystem::Excel1900).unwrap(),
            (2024, 3, 15)
        );
        assert_eq!(
            serial_to_ymd(EXCEL_MAX_SERIAL_1900 as f64, DateSystem::Excel1900).unwrap(),
            (9999, 12, 31)
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

    fn nd(start: f64, end: f64, hols: &[f64]) -> f64 {
        networkdays_count(start, end, hols, DateSystem::Excel1900).unwrap()
    }

    fn wd(start: f64, days: f64, hols: &[f64]) -> f64 {
        workday_serial(start, days, hols, DateSystem::Excel1900).unwrap()
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
        assert_eq!(
            eomonth_serial(d(2011, 1, 1), 1.9, s).unwrap(),
            d(2011, 2, 28)
        );
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

    #[test]
    fn networkdays_ms_examples() {
        let start = d(2012, 10, 1);
        let end = d(2013, 3, 1);
        let h1 = d(2012, 11, 22);
        let h2 = d(2012, 12, 4);
        let h3 = d(2013, 1, 21);
        assert_eq!(start, 41183.0);
        assert_eq!(end, 41334.0);
        assert_eq!(nd(start, end, &[]), 110.0);
        assert_eq!(nd(start, end, &[h1]), 109.0);
        assert_eq!(nd(start, end, &[h1, h2, h3]), 107.0);
        assert_eq!(nd(end, start, &[]), -110.0);
        assert_eq!(nd(end, start, &[h1, h2, h3]), -107.0);
    }

    #[test]
    fn networkdays_week_edges() {
        assert_eq!(nd(d(2024, 1, 2), d(2024, 1, 2), &[]), 1.0);
        assert_eq!(nd(d(2024, 1, 6), d(2024, 1, 6), &[]), 0.0);
        assert_eq!(nd(d(2024, 1, 1), d(2024, 1, 5), &[]), 5.0);
        assert_eq!(nd(d(2024, 1, 1), d(2024, 1, 7), &[]), 5.0);
        assert_eq!(nd(d(2024, 1, 5), d(2024, 1, 1), &[]), -5.0);
        assert_eq!(nd(1.0, 1.0, &[]), 0.0);
        assert_eq!(nd(7.0, 7.0, &[]), 0.0);
        assert_eq!(nd(2.0, 2.0, &[]), 1.0);
        assert_eq!(nd(0.0, 0.0, &[]), 0.0);
        assert_eq!(nd(1.0, 7.0, &[]), 5.0);
        assert_eq!(nd(0.0, 2.0, &[]), 1.0);
    }

    #[test]
    fn networkdays_serial_60_leap_bug() {
        let s = DateSystem::Excel1900;
        assert_eq!(networkdays_count(60.0, 60.0, &[], s).unwrap(), 1.0);
        assert_eq!(networkdays_count(59.0, 61.0, &[], s).unwrap(), 3.0);
        assert_eq!(
            networkdays_count(d(1900, 2, 1), d(1900, 3, 5), &[], s).unwrap(),
            24.0
        );
        assert_eq!(
            networkdays_count(d(1900, 2, 1), d(1900, 3, 5), &[60.0], s).unwrap(),
            23.0
        );
        assert!(!is_weekend_sat_sun_1900(60));
        assert!(is_weekend_sat_sun_1900(1));
        assert!(is_weekend_sat_sun_1900(7));
    }

    #[test]
    fn networkdays_holidays() {
        let start = d(2024, 1, 1);
        let end = d(2024, 1, 7);
        assert_eq!(nd(start, end, &[d(2024, 1, 6)]), 5.0);
        assert_eq!(nd(start, end, &[d(2024, 1, 1)]), 4.0);
        assert_eq!(nd(start, end, &[d(2024, 1, 1), d(2024, 1, 1)]), 4.0);
        assert_eq!(nd(start, end, &[d(2023, 12, 25)]), 5.0);
        assert_eq!(nd(start, end, &[d(2024, 1, 1), d(2024, 1, 2)]), 3.0);
        assert_eq!(nd(start, end, &[1.5, 1.9]), 5.0);
    }

    #[test]
    fn networkdays_truncates_fractions() {
        assert_eq!(nd(d(2024, 1, 1) + 0.9, d(2024, 1, 5) + 0.1, &[]), 5.0);
    }

    #[test]
    fn networkdays_rejects_out_of_range() {
        let s = DateSystem::Excel1900;
        assert_eq!(networkdays_count(-1.0, 10.0, &[], s), Err(ExcelError::Num));
        assert_eq!(
            networkdays_count(1.0, (EXCEL_MAX_SERIAL_1900 as f64) + 1.0, &[], s),
            Err(ExcelError::Num)
        );
        assert_eq!(
            networkdays_count(1.0, 7.0, &[-1.0], s),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn networkdays_system_1904() {
        let s = DateSystem::Excel1904;
        assert_eq!(networkdays_count(0.0, 4.0, &[], s).unwrap(), 3.0);
        assert_eq!(networkdays_count(0.0, 0.0, &[], s).unwrap(), 1.0);
        assert_eq!(networkdays_count(0.0, 4.0, &[0.0], s).unwrap(), 2.0);
    }

    #[test]
    fn weekday_count_matches_walk_on_prefix() {
        for lo in 0..40 {
            for hi in lo..40 {
                let fast = weekday_count_sat_sun(lo, hi);
                let walk: i32 = (lo..=hi).filter(|&s| !is_weekend_sat_sun_1900(s)).count() as i32;
                assert_eq!(fast, walk, "[{lo}, {hi}]");
            }
        }
    }

    #[test]
    fn weekday_serial_one_is_sunday() {
        // Excel's 1900 leap-year bug: 1900-01-01 is Sunday (type 1 = 1),
        // not the historical Monday.
        assert_eq!(weekday(1.0, 1, DateSystem::Excel1900).unwrap(), 1.0);
        assert_eq!(
            weekday(
                date_serial(1900, 1, 1, DateSystem::Excel1900).unwrap(),
                1,
                DateSystem::Excel1900
            )
            .unwrap(),
            1.0
        );
    }

    #[test]
    fn weekday_leap_bug_window() {
        assert_eq!(weekday(0.0, 1, DateSystem::Excel1900).unwrap(), 7.0);
        assert_eq!(weekday(59.0, 1, DateSystem::Excel1900).unwrap(), 3.0);
        assert_eq!(weekday(60.0, 1, DateSystem::Excel1900).unwrap(), 4.0);
        assert_eq!(weekday(61.0, 1, DateSystem::Excel1900).unwrap(), 5.0);
    }

    #[test]
    fn weekday_ms_thursday_example() {
        // Microsoft docs: DATE(2008,2,14) is Thursday.
        let s = date_serial(2008, 2, 14, DateSystem::Excel1900).unwrap();
        assert_eq!(weekday(s, 1, DateSystem::Excel1900).unwrap(), 5.0);
        assert_eq!(weekday(s, 2, DateSystem::Excel1900).unwrap(), 4.0);
        assert_eq!(weekday(s, 3, DateSystem::Excel1900).unwrap(), 3.0);
        assert_eq!(weekday(s, 11, DateSystem::Excel1900).unwrap(), 4.0);
        assert_eq!(weekday(s, 12, DateSystem::Excel1900).unwrap(), 3.0);
        assert_eq!(weekday(s, 13, DateSystem::Excel1900).unwrap(), 2.0);
        assert_eq!(weekday(s, 14, DateSystem::Excel1900).unwrap(), 1.0);
        assert_eq!(weekday(s, 15, DateSystem::Excel1900).unwrap(), 7.0);
        assert_eq!(weekday(s, 16, DateSystem::Excel1900).unwrap(), 6.0);
        assert_eq!(weekday(s, 17, DateSystem::Excel1900).unwrap(), 5.0);
    }

    #[test]
    fn weekday_fraction_and_range() {
        assert_eq!(weekday(1.9, 1, DateSystem::Excel1900).unwrap(), 1.0);
        assert!(weekday(-1.0, 1, DateSystem::Excel1900).is_err());
        assert!(weekday(1.0, 4, DateSystem::Excel1900).is_err());
        assert!(weekday(1.0, 0, DateSystem::Excel1900).is_err());
        assert!(weekday(1.0, 18, DateSystem::Excel1900).is_err());
        assert_eq!(
            weekday(EXCEL_MAX_SERIAL_1900 as f64, 1, DateSystem::Excel1900).unwrap(),
            6.0
        );
        assert!(weekday((EXCEL_MAX_SERIAL_1900 + 1) as f64, 1, DateSystem::Excel1900).is_err());
    }

    #[test]
    fn weekday_1904_epoch_is_friday() {
        assert_eq!(weekday(0.0, 1, DateSystem::Excel1904).unwrap(), 6.0);
        let s = date_serial(1904, 1, 1, DateSystem::Excel1904).unwrap();
        assert_eq!(weekday(s, 1, DateSystem::Excel1904).unwrap(), 6.0);
    }

    #[test]
    fn weekday_matches_naive_across_range() {
        let types = [1, 2, 3, 11, 12, 13, 14, 15, 16, 17];
        for s in 0..=400 {
            for rt in types {
                let a = weekday(s as f64, rt, DateSystem::Excel1900).unwrap();
                let b = weekday_naive(s as f64, rt, DateSystem::Excel1900).unwrap();
                assert_eq!(a, b, "serial={s} return_type={rt}");
            }
        }
        for s in [36526, 39448, 39492, 42078, EXCEL_MAX_SERIAL_1900] {
            for rt in types {
                let a = weekday(s as f64, rt, DateSystem::Excel1900).unwrap();
                let b = weekday_naive(s as f64, rt, DateSystem::Excel1900).unwrap();
                assert_eq!(a, b, "serial={s} return_type={rt}");
            }
        }
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

    fn ndi(start: f64, end: f64, mask: u8, hols: &[f64]) -> f64 {
        networkdays_count_mask(start, end, mask, hols, DateSystem::Excel1900).unwrap()
    }

    #[test]
    fn weekend_mask_codes_match_documented_days() {
        // Bits: 0 Sat, 1 Sun, 2 Mon, 3 Tue, 4 Wed, 5 Thu, 6 Fri.
        assert_eq!(weekend_mask_from_code(1).unwrap(), WEEKEND_SAT_SUN);
        assert_eq!(weekend_mask_from_code(2).unwrap(), 0b0000_0110);
        assert_eq!(weekend_mask_from_code(7).unwrap(), 0b0100_0001);
        assert_eq!(weekend_mask_from_code(11).unwrap(), 0b0000_0010);
        assert_eq!(weekend_mask_from_code(17).unwrap(), 0b0000_0001);
        assert_eq!(weekend_mask_from_code(0), Err(ExcelError::Num));
        assert_eq!(weekend_mask_from_code(8), Err(ExcelError::Num));
        assert_eq!(weekend_mask_from_code(10), Err(ExcelError::Num));
        assert_eq!(weekend_mask_from_code(18), Err(ExcelError::Num));
    }

    #[test]
    fn weekend_mask_strings_mon_to_sun() {
        assert_eq!(
            weekend_mask_from_string("0000011").unwrap(),
            WEEKEND_SAT_SUN
        );
        assert_eq!(
            weekend_mask_from_string("0000001").unwrap(),
            weekend_mask_from_code(11).unwrap()
        );
        assert_eq!(weekend_mask_from_string("0000000").unwrap(), 0);
        assert_eq!(weekend_mask_from_string("1111111").unwrap(), 0x7f);
        assert_eq!(weekend_mask_from_string("1"), Err(ExcelError::Value));
        assert_eq!(weekend_mask_from_string("000001"), Err(ExcelError::Value));
        assert_eq!(weekend_mask_from_string("0000012"), Err(ExcelError::Value));
        assert_eq!(weekend_mask_from_string("00000111"), Err(ExcelError::Value));
    }

    #[test]
    fn networkdays_intl_ms_example() {
        // support.microsoft.com: 1-Jan-06 → 1-Feb-06, weekend 7 (Fri/Sat),
        // holidays 2-Jan and 16-Jan → 22.
        let start = d(2006, 1, 1);
        let end = d(2006, 2, 1);
        let mask7 = weekend_mask_from_code(7).unwrap();
        assert_eq!(
            ndi(start, end, mask7, &[d(2006, 1, 2), d(2006, 1, 16)]),
            22.0
        );
        assert_eq!(ndi(start, end, mask7, &[]), 24.0);
        assert_eq!(ndi(start, end, WEEKEND_SAT_SUN, &[]), 23.0);
        assert_eq!(
            ndi(
                start,
                end,
                weekend_mask_from_string("0000000").unwrap(),
                &[]
            ),
            32.0
        );
        assert_eq!(
            ndi(
                start,
                end,
                weekend_mask_from_string("1111111").unwrap(),
                &[]
            ),
            0.0
        );
    }

    #[test]
    fn networkdays_intl_matches_networkdays_for_sat_sun() {
        let start = d(2012, 10, 1);
        let end = d(2013, 3, 1);
        let hols = [d(2012, 11, 22), d(2012, 12, 4), d(2013, 1, 21)];
        assert_eq!(
            networkdays_count(start, end, &hols, DateSystem::Excel1900).unwrap(),
            ndi(start, end, WEEKEND_SAT_SUN, &hols)
        );
        assert_eq!(
            ndi(
                start,
                end,
                weekend_mask_from_string("0000011").unwrap(),
                &[]
            ),
            110.0
        );
    }

    #[test]
    fn networkdays_intl_week_2024() {
        let start = d(2024, 1, 1);
        let end = d(2024, 1, 7);
        assert_eq!(
            ndi(start, end, weekend_mask_from_code(1).unwrap(), &[]),
            5.0
        );
        assert_eq!(
            ndi(start, end, weekend_mask_from_code(11).unwrap(), &[]),
            6.0
        );
        assert_eq!(
            ndi(start, end, weekend_mask_from_code(17).unwrap(), &[]),
            6.0
        );
        assert_eq!(
            ndi(
                start,
                end,
                weekend_mask_from_string("0000000").unwrap(),
                &[]
            ),
            7.0
        );
        assert_eq!(
            ndi(
                start,
                end,
                weekend_mask_from_string("0101011").unwrap(),
                &[]
            ),
            3.0
        );
        assert_eq!(
            ndi(
                d(2024, 1, 6),
                d(2024, 1, 6),
                weekend_mask_from_code(11).unwrap(),
                &[]
            ),
            1.0
        );
        // Saturday holiday is ignored under Sat/Sun, subtracted under Sunday-only.
        assert_eq!(ndi(start, end, WEEKEND_SAT_SUN, &[d(2024, 1, 6)]), 5.0);
        assert_eq!(
            ndi(
                start,
                end,
                weekend_mask_from_code(11).unwrap(),
                &[d(2024, 1, 6)]
            ),
            5.0
        );
        assert_eq!(
            ndi(d(2024, 1, 5), d(2024, 1, 1), WEEKEND_SAT_SUN, &[]),
            -5.0
        );
    }

    #[test]
    fn networkdays_intl_serial_60() {
        let s = DateSystem::Excel1900;
        let wed = weekend_mask_from_code(14).unwrap();
        assert_eq!(
            networkdays_count_mask(60.0, 60.0, WEEKEND_SAT_SUN, &[], s).unwrap(),
            1.0
        );
        assert_eq!(
            networkdays_count_mask(60.0, 60.0, wed, &[], s).unwrap(),
            0.0
        );
        assert_eq!(
            networkdays_count_mask(59.0, 61.0, wed, &[], s).unwrap(),
            2.0
        );
    }

    #[test]
    fn networkdays_intl_system_1904() {
        let s = DateSystem::Excel1904;
        assert_eq!(
            networkdays_count_mask(0.0, 4.0, WEEKEND_SAT_SUN, &[], s).unwrap(),
            3.0
        );
        assert_eq!(
            networkdays_count_mask(0.0, 4.0, weekend_mask_from_code(11).unwrap(), &[], s).unwrap(),
            4.0
        );
        assert_eq!(networkdays_count_mask(0.0, 4.0, 0, &[], s).unwrap(), 5.0);
    }

    #[test]
    fn networkdays_intl_mask_matches_walk() {
        let s = DateSystem::Excel1900;
        let codes = [1, 2, 3, 4, 5, 6, 7, 11, 12, 13, 14, 15, 16, 17];
        let strings = ["0000000", "1111111", "0000011", "0101011", "1000001"];
        let hols = [60.0, 61.0, 64.0];
        for lo in 0..28 {
            for hi in lo..28 {
                for code in codes {
                    let mask = weekend_mask_from_code(code).unwrap();
                    let fast = networkdays_count_mask(lo as f64, hi as f64, mask, &[], s);
                    let walk = networkdays_count_mask_walk(lo as f64, hi as f64, mask, &[], s);
                    assert_eq!(fast, walk, "code={code} [{lo}, {hi}]");
                }
                for pat in strings {
                    let mask = weekend_mask_from_string(pat).unwrap();
                    let fast = networkdays_count_mask(lo as f64, hi as f64, mask, &hols, s);
                    let walk = networkdays_count_mask_walk(lo as f64, hi as f64, mask, &hols, s);
                    assert_eq!(fast, walk, "pat={pat} [{lo}, {hi}] hols");
                }
            }
        }
        let start = d(2006, 1, 1);
        let end = d(2006, 2, 1);
        let hols_ms = [d(2006, 1, 2), d(2006, 1, 16)];
        for code in codes {
            let mask = weekend_mask_from_code(code).unwrap();
            let fast = networkdays_count_mask(start, end, mask, &hols_ms, s).unwrap();
            let walk = networkdays_count_mask_walk(start, end, mask, &hols_ms, s).unwrap();
            assert_eq!(fast, walk, "MS span code={code}");
        }
    }

    #[test]
    fn sat_sun_mask_matches_legacy_workdays_through() {
        for n in -2..80 {
            assert_eq!(
                workdays_through(n),
                workdays_through_mask(n, WEEKEND_SAT_SUN),
                "n={n}"
            );
        }
    }
}
