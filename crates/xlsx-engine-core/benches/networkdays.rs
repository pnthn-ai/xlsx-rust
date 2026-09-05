//! Microbenches for Excel `NETWORKDAYS` (weekend Sat/Sun).
//!
//! Compares the production O(1) weekday count (`networkdays_count`) against a
//! day-walk reference that visits every serial in the span.

use std::hint::black_box;
use xlsx_engine_core::dates::{
    date_serial, is_weekend_sat_sun_1900, networkdays_count, to_1900_serial, truncate_date_serial,
};
use xlsx_types::DateSystem;

const SYSTEM: DateSystem = DateSystem::Excel1900;

fn networkdays_count_walk(
    start: f64,
    end: f64,
    holidays: &[f64],
    system: DateSystem,
) -> Result<f64, xlsx_types::ExcelError> {
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
        if !is_weekend_sat_sun_1900(s) {
            work += 1;
        }
    }
    let mut hols: Vec<i32> = Vec::with_capacity(holidays.len());
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

fn main() {
    let ms_start = date_serial(2012, 10, 1, SYSTEM).unwrap();
    let ms_end = date_serial(2013, 3, 1, SYSTEM).unwrap();
    let h1 = date_serial(2012, 11, 22, SYSTEM).unwrap();
    let h2 = date_serial(2012, 12, 4, SYSTEM).unwrap();
    let h3 = date_serial(2013, 1, 21, SYSTEM).unwrap();
    let modern = date_serial(2024, 1, 1, SYSTEM).unwrap();
    let modern_end = date_serial(2024, 1, 7, SYSTEM).unwrap();
    let century_end = date_serial(2100, 1, 1, SYSTEM).unwrap();

    let hols_three = [h1, h2, h3];
    let hols_one = [modern];

    for &(start, end, hols) in &[
        (60.0, 60.0, &[] as &[f64]),
        (59.0, 61.0, &[]),
        (ms_start, ms_end, &[]),
        (ms_start, ms_end, hols_three.as_slice()),
        (modern, modern_end, hols_one.as_slice()),
        (1.0, 367.0, &[]),
    ] {
        let a = networkdays_count(start, end, hols, SYSTEM).unwrap();
        let b = networkdays_count_walk(start, end, hols, SYSTEM).unwrap();
        assert_eq!(a, b, "fast vs walk mismatch for ({start}, {end})");
    }

    const SHORT_ITERS: u32 = 50_000;
    let short: &[((f64, f64), &[f64])] = &[
        ((60.0, 60.0), &[]),
        ((59.0, 61.0), &[]),
        ((ms_start, ms_end), &[]),
        ((ms_start, ms_end), hols_three.as_slice()),
        ((modern, modern_end), hols_one.as_slice()),
        ((1.0, 367.0), &[]),
    ];

    let t0 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        for &((start, end), hols) in short {
            black_box(
                networkdays_count_walk(black_box(start), black_box(end), hols, SYSTEM).unwrap(),
            );
        }
    }
    let walk_ns = t0.elapsed().as_nanos() / (SHORT_ITERS as u128 * short.len() as u128);

    let t1 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        for &((start, end), hols) in short {
            black_box(networkdays_count(black_box(start), black_box(end), hols, SYSTEM).unwrap());
        }
    }
    let fast_ns = t1.elapsed().as_nanos() / (SHORT_ITERS as u128 * short.len() as u128);

    let t2 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        black_box(networkdays_count(black_box(ms_start), black_box(ms_end), &[], SYSTEM).unwrap());
    }
    let ms_ns = t2.elapsed().as_nanos() / SHORT_ITERS as u128;

    let t3 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        black_box(networkdays_count(black_box(60.0), black_box(60.0), &[], SYSTEM).unwrap());
    }
    let serial60_ns = t3.elapsed().as_nanos() / SHORT_ITERS as u128;

    const LONG_ITERS: u32 = 200;
    let t4 = std::time::Instant::now();
    for _ in 0..LONG_ITERS {
        black_box(
            networkdays_count_walk(black_box(1.0), black_box(century_end), &[], SYSTEM).unwrap(),
        );
    }
    let long_walk_ns = t4.elapsed().as_nanos() / LONG_ITERS as u128;

    let t5 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        black_box(networkdays_count(black_box(1.0), black_box(century_end), &[], SYSTEM).unwrap());
    }
    let long_fast_ns = t5.elapsed().as_nanos() / SHORT_ITERS as u128;

    println!(
        "networkdays microbench  short_iters={SHORT_ITERS}  cases={}",
        short.len()
    );
    println!("  walk-ref mixed           {walk_ns} ns/call");
    println!("  networkdays_count mixed  {fast_ns} ns/call");
    println!("  NETWORKDAYS(MS span)     {ms_ns} ns/call");
    println!("  NETWORKDAYS(60,60)       {serial60_ns} ns/call");
    println!("  walk 1900→2100           {long_walk_ns} ns/call");
    println!("  fast 1900→2100           {long_fast_ns} ns/call");
    if walk_ns > 0 {
        println!(
            "  hill-climb speedup (mixed) {:.2}x vs day-walk",
            walk_ns as f64 / fast_ns.max(1) as f64
        );
    }
    if long_walk_ns > 0 {
        println!(
            "  hill-climb speedup (century) {:.2}x vs day-walk",
            long_walk_ns as f64 / long_fast_ns.max(1) as f64
        );
    }
}
