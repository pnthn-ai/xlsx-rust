//! Microbenches for Excel `EOMONTH`.
//!
//! Compares the production helper (`eomonth_serial`, which reuses the O(1)
//! year-start converters) against a year-walk reference that mimics the
//! pre-hill-climb `serial_to_ymd` / `ymd_to_serial_1900` cost.

use std::hint::black_box;
use xlsx_engine_core::dates::{
    date_serial, days_in_month, eomonth_serial, is_excel_leap, normalize_year_month, serial_to_ymd,
};
use xlsx_types::DateSystem;

const SYSTEM: DateSystem = DateSystem::Excel1900;

/// Pre-hill-climb year walk: O(years since 1900) each way.
fn eomonth_serial_walk(start: f64, months: f64, system: DateSystem) -> Result<f64, xlsx_types::ExcelError> {
    if !start.is_finite() || !months.is_finite() || start < 0.0 {
        return Err(xlsx_types::ExcelError::Num);
    }
    let (year, month, _day) = serial_to_ymd_walk(start, system)?;
    let months_i = months.trunc();
    if months_i.abs() > 120_000.0 {
        return Err(xlsx_types::ExcelError::Num);
    }
    let (year, month) = normalize_year_month(year, month as i32 + months_i as i32)?;
    if year < 1900 || year > 9999 {
        return Err(xlsx_types::ExcelError::Num);
    }
    let last = days_in_month(year, month);
    let s1900 = ymd_to_serial_1900_walk(year, month, last)?;
    match system {
        DateSystem::Excel1900 => Ok(s1900 as f64),
        DateSystem::Excel1904 => {
            let s = s1900 - 1462;
            if s < 0 {
                Err(xlsx_types::ExcelError::Num)
            } else {
                Ok(s as f64)
            }
        }
    }
}

fn serial_of_year_start_walk(year: i32) -> i32 {
    if year < 1900 {
        let mut serial = 1;
        let mut y = 1900;
        while y > year {
            y -= 1;
            serial -= if is_excel_leap(y) { 366 } else { 365 };
        }
        return serial;
    }
    let mut serial = 1;
    for y in 1900..year {
        serial += if is_excel_leap(y) { 366 } else { 365 };
    }
    serial
}

fn serial_to_ymd_walk(
    serial: f64,
    system: DateSystem,
) -> Result<(i32, u32, u32), xlsx_types::ExcelError> {
    if !serial.is_finite() {
        return Err(xlsx_types::ExcelError::Num);
    }
    let mut s = serial.trunc() as i32;
    if matches!(system, DateSystem::Excel1904) {
        s = s.checked_add(1462).ok_or(xlsx_types::ExcelError::Num)?;
    }
    if s < 0 || s > 2_958_465 {
        return Err(xlsx_types::ExcelError::Num);
    }
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
            return Err(xlsx_types::ExcelError::Num);
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
    Err(xlsx_types::ExcelError::Num)
}

fn ymd_to_serial_1900_walk(year: i32, month: i32, day: i32) -> Result<i32, xlsx_types::ExcelError> {
    let mut first = 1;
    for m in 1..month {
        first += days_in_month(year, m);
    }
    let serial = serial_of_year_start_walk(year) + first - 1 + (day - 1);
    if serial < 0 || serial > 2_958_465 {
        return Err(xlsx_types::ExcelError::Num);
    }
    Ok(serial)
}

fn run_pair(f: fn(f64, f64, DateSystem) -> Result<f64, xlsx_types::ExcelError>, start: f64, months: f64) {
    black_box(f(black_box(start), black_box(months), SYSTEM).unwrap());
}

fn main() {
    let modern = date_serial(2024, 1, 31, SYSTEM).unwrap();
    let y2k = date_serial(2000, 1, 31, SYSTEM).unwrap();
    let late = date_serial(9999, 11, 15, SYSTEM).unwrap();

    // Warm + correctness cross-check against the walk reference.
    for &(start, months) in &[
        (60.0, 0.0),
        (60.0, 1.0),
        (60.0, -1.0),
        (modern, 1.0),
        (y2k, 13.0),
        (late, 0.0),
    ] {
        let a = eomonth_serial(start, months, SYSTEM).unwrap();
        let b = eomonth_serial_walk(start, months, SYSTEM).unwrap();
        assert_eq!(a, b, "fast vs walk mismatch for ({start}, {months})");
    }

    const ITERS: u32 = 50_000;
    let cases: &[(f64, f64)] = &[
        (60.0, 0.0),
        (60.0, -1.0),
        (modern, 1.0),
        (modern, -12.0),
        (y2k, 1.0),
        (late, 0.0),
    ];

    let t0 = std::time::Instant::now();
    for _ in 0..ITERS {
        for &(start, months) in cases {
            run_pair(eomonth_serial_walk, start, months);
        }
    }
    let walk_ns = t0.elapsed().as_nanos() / (ITERS as u128 * cases.len() as u128);

    let t1 = std::time::Instant::now();
    for _ in 0..ITERS {
        for &(start, months) in cases {
            run_pair(eomonth_serial, start, months);
        }
    }
    let fast_ns = t1.elapsed().as_nanos() / (ITERS as u128 * cases.len() as u128);

    let t2 = std::time::Instant::now();
    for _ in 0..ITERS {
        black_box(eomonth_serial(black_box(60.0), black_box(0.0), SYSTEM).unwrap());
    }
    let serial60_ns = t2.elapsed().as_nanos() / ITERS as u128;

    let t3 = std::time::Instant::now();
    for _ in 0..ITERS {
        black_box(eomonth_serial(black_box(modern), black_box(1.0), SYSTEM).unwrap());
    }
    let modern_ns = t3.elapsed().as_nanos() / ITERS as u128;

    let t4 = std::time::Instant::now();
    for _ in 0..ITERS {
        black_box(serial_to_ymd(black_box(modern), SYSTEM).unwrap());
    }
    let unpack_ns = t4.elapsed().as_nanos() / ITERS as u128;

    println!("eomonth microbench  iters={ITERS}  cases={}", cases.len());
    println!("  walk-ref mixed        {walk_ns} ns/call");
    println!("  eomonth_serial mixed  {fast_ns} ns/call");
    println!("  eomonth_serial(60,0)  {serial60_ns} ns/call");
    println!("  eomonth_serial(2024-01-31,1) {modern_ns} ns/call");
    println!("  serial_to_ymd(2024-01-31)    {unpack_ns} ns/call");
    if walk_ns > 0 {
        println!(
            "  hill-climb speedup    {:.2}x vs year-walk",
            walk_ns as f64 / fast_ns.max(1) as f64
        );
    }
}
