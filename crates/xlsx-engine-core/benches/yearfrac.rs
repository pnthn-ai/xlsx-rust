//! Microbenches for Excel `YEARFRAC`.
//!
//! Compares the production helper (`yearfrac`, closed-form year-start +
//! `serial_to_ymd`) against the year-walk reference (`yearfrac_naive`).

use std::hint::black_box;
use xlsx_engine_core::dates::{date_serial, yearfrac, yearfrac_naive};
use xlsx_types::DateSystem;

const SYSTEM: DateSystem = DateSystem::Excel1900;

fn run_pair(
    f: fn(f64, f64, i32, DateSystem) -> Result<f64, xlsx_types::ExcelError>,
    start: f64,
    end: f64,
    basis: i32,
) {
    black_box(f(black_box(start), black_box(end), black_box(basis), SYSTEM).unwrap());
}

fn main() {
    let ms_start = date_serial(2012, 1, 1, SYSTEM).unwrap();
    let ms_end = date_serial(2012, 7, 30, SYSTEM).unwrap();
    let modern = date_serial(2024, 1, 1, SYSTEM).unwrap();
    let modern_end = date_serial(2025, 3, 15, SYSTEM).unwrap();
    let century = date_serial(1900, 1, 1, SYSTEM).unwrap();
    let century_end = date_serial(2000, 1, 1, SYSTEM).unwrap();
    let late = date_serial(1990, 6, 15, SYSTEM).unwrap();
    let late_end = date_serial(9999, 12, 31, SYSTEM).unwrap();

    for &(start, end, basis) in &[
        (60.0, 61.0, 0),
        (59.0, 61.0, 1),
        (ms_start, ms_end, 0),
        (ms_start, ms_end, 1),
        (modern, modern_end, 1),
        (century, century_end, 1),
    ] {
        let a = yearfrac(start, end, basis, SYSTEM).unwrap();
        let b = yearfrac_naive(start, end, basis, SYSTEM).unwrap();
        assert_eq!(a, b, "fast vs walk mismatch for ({start}, {end}, {basis})");
    }

    const SHORT_ITERS: u32 = 50_000;
    let short: &[(f64, f64, i32)] = &[
        (60.0, 61.0, 0),
        (59.0, 61.0, 1),
        (ms_start, ms_end, 0),
        (ms_start, ms_end, 1),
        (ms_start, ms_end, 3),
        (modern, modern_end, 1),
    ];

    let t0 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        for &(start, end, basis) in short {
            run_pair(yearfrac_naive, start, end, basis);
        }
    }
    let walk_ns = t0.elapsed().as_nanos() / (SHORT_ITERS as u128 * short.len() as u128);

    let t1 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        for &(start, end, basis) in short {
            run_pair(yearfrac, start, end, basis);
        }
    }
    let fast_ns = t1.elapsed().as_nanos() / (SHORT_ITERS as u128 * short.len() as u128);

    let t2 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        black_box(yearfrac(black_box(ms_start), black_box(ms_end), black_box(0), SYSTEM).unwrap());
    }
    let ms_ns = t2.elapsed().as_nanos() / SHORT_ITERS as u128;

    let t3 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        black_box(yearfrac(black_box(60.0), black_box(61.0), black_box(1), SYSTEM).unwrap());
    }
    let serial60_ns = t3.elapsed().as_nanos() / SHORT_ITERS as u128;

    const LONG_ITERS: u32 = 5_000;
    let t4 = std::time::Instant::now();
    for _ in 0..LONG_ITERS {
        black_box(
            yearfrac_naive(black_box(late), black_box(late_end), black_box(1), SYSTEM).unwrap(),
        );
    }
    let walk_late_ns = t4.elapsed().as_nanos() / LONG_ITERS as u128;

    let t5 = std::time::Instant::now();
    for _ in 0..LONG_ITERS {
        black_box(yearfrac(black_box(late), black_box(late_end), black_box(1), SYSTEM).unwrap());
    }
    let fast_late_ns = t5.elapsed().as_nanos() / LONG_ITERS as u128;

    println!(
        "yearfrac microbench  short_iters={SHORT_ITERS}  cases={}",
        short.len()
    );
    println!("  walk-ref mixed           {walk_ns} ns/call");
    println!("  yearfrac mixed           {fast_ns} ns/call");
    println!("  YEARFRAC(MS span,0)      {ms_ns} ns/call");
    println!("  YEARFRAC(60,61,1)        {serial60_ns} ns/call");
    println!("  walk 1990→9999 basis 1   {walk_late_ns} ns/call");
    println!("  fast 1990→9999 basis 1   {fast_late_ns} ns/call");
    if walk_ns > 0 {
        println!(
            "  hill-climb speedup (mixed) {:.2}x vs year-walk",
            walk_ns as f64 / fast_ns.max(1) as f64
        );
    }
    if walk_late_ns > 0 {
        println!(
            "  hill-climb speedup (late)  {:.2}x vs year-walk",
            walk_late_ns as f64 / fast_late_ns.max(1) as f64
        );
    }
}
