//! Microbenches for Excel `DAYS360`.
//!
//! Compares the production helper (`days360`, closed-form `serial_to_ymd`)
//! against the year-walk reference (`days360_naive`).

use std::hint::black_box;
use xlsx_engine_core::dates::{date_serial, days360, days360_naive};
use xlsx_types::DateSystem;

const SYSTEM: DateSystem = DateSystem::Excel1900;

fn run_pair(
    f: fn(f64, f64, bool, DateSystem) -> Result<f64, xlsx_types::ExcelError>,
    start: f64,
    end: f64,
    european: bool,
) {
    black_box(
        f(
            black_box(start),
            black_box(end),
            black_box(european),
            SYSTEM,
        )
        .unwrap(),
    );
}

fn main() {
    let ms_start = date_serial(2011, 1, 1, SYSTEM).unwrap();
    let ms_end = date_serial(2011, 12, 31, SYSTEM).unwrap();
    let modern = date_serial(2024, 1, 1, SYSTEM).unwrap();
    let modern_end = date_serial(2025, 3, 15, SYSTEM).unwrap();
    let late = date_serial(1990, 6, 15, SYSTEM).unwrap();
    let late_end = date_serial(9999, 12, 31, SYSTEM).unwrap();

    for &(start, end, european) in &[
        (60.0, 61.0, false),
        (59.0, 61.0, true),
        (ms_start, ms_end, false),
        (ms_start, ms_end, true),
        (modern, modern_end, false),
    ] {
        let a = days360(start, end, european, SYSTEM).unwrap();
        let b = days360_naive(start, end, european, SYSTEM).unwrap();
        assert_eq!(
            a, b,
            "fast vs walk mismatch for ({start}, {end}, {european})"
        );
    }

    const SHORT_ITERS: u32 = 50_000;
    let short: &[(f64, f64, bool)] = &[
        (60.0, 61.0, false),
        (59.0, 61.0, true),
        (ms_start, ms_end, false),
        (ms_start, ms_end, true),
        (modern, modern_end, false),
        (modern, modern_end, true),
    ];

    let t0 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        for &(start, end, european) in short {
            run_pair(days360_naive, start, end, european);
        }
    }
    let walk_ns = t0.elapsed().as_nanos() / (SHORT_ITERS as u128 * short.len() as u128);

    let t1 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        for &(start, end, european) in short {
            run_pair(days360, start, end, european);
        }
    }
    let fast_ns = t1.elapsed().as_nanos() / (SHORT_ITERS as u128 * short.len() as u128);

    let t2 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        black_box(
            days360(
                black_box(ms_start),
                black_box(ms_end),
                black_box(false),
                SYSTEM,
            )
            .unwrap(),
        );
    }
    let ms_ns = t2.elapsed().as_nanos() / SHORT_ITERS as u128;

    let t3 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        black_box(days360(black_box(60.0), black_box(61.0), black_box(false), SYSTEM).unwrap());
    }
    let serial60_ns = t3.elapsed().as_nanos() / SHORT_ITERS as u128;

    const LONG_ITERS: u32 = 5_000;
    let t4 = std::time::Instant::now();
    for _ in 0..LONG_ITERS {
        black_box(
            days360_naive(
                black_box(late),
                black_box(late_end),
                black_box(false),
                SYSTEM,
            )
            .unwrap(),
        );
    }
    let walk_late_ns = t4.elapsed().as_nanos() / LONG_ITERS as u128;

    let t5 = std::time::Instant::now();
    for _ in 0..LONG_ITERS {
        black_box(
            days360(
                black_box(late),
                black_box(late_end),
                black_box(false),
                SYSTEM,
            )
            .unwrap(),
        );
    }
    let fast_late_ns = t5.elapsed().as_nanos() / LONG_ITERS as u128;

    println!(
        "days360 microbench  short_iters={SHORT_ITERS}  cases={}",
        short.len()
    );
    println!("  walk-ref mixed           {walk_ns} ns/call");
    println!("  days360 mixed            {fast_ns} ns/call");
    println!("  DAYS360(MS span, NASD)   {ms_ns} ns/call");
    println!("  DAYS360(60,61)           {serial60_ns} ns/call");
    println!("  walk 1990→9999 NASD      {walk_late_ns} ns/call");
    println!("  fast 1990→9999 NASD      {fast_late_ns} ns/call");
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
