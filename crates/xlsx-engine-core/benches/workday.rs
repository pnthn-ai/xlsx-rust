//! Microbenches for Excel `WORKDAY`.
//!
//! Compares the production helper (`workday_serial`, O(1) weekday inversion
//! plus O(H) holiday adjust) against a day-walk reference.

use std::hint::black_box;
use xlsx_engine_core::dates::{date_serial, workday_serial, workday_serial_walk};
use xlsx_types::DateSystem;

const SYSTEM: DateSystem = DateSystem::Excel1900;

fn run_pair(
    f: fn(f64, f64, &[f64], DateSystem) -> Result<f64, xlsx_types::ExcelError>,
    start: f64,
    days: f64,
    hols: &[f64],
) {
    black_box(f(black_box(start), black_box(days), black_box(hols), SYSTEM).unwrap());
}

fn main() {
    let ms_start = date_serial(2008, 10, 1, SYSTEM).unwrap();
    let modern = date_serial(2024, 1, 4, SYSTEM).unwrap();
    let epoch_monday = 2.0;
    let h1 = date_serial(2008, 11, 26, SYSTEM).unwrap();
    let h2 = date_serial(2008, 12, 4, SYSTEM).unwrap();
    let h3 = date_serial(2009, 1, 21, SYSTEM).unwrap();
    let ms_hols = [h1, h2, h3];

    // Warm + correctness cross-check against the walk reference.
    for &(start, days, hols) in &[
        (60.0, 0.0, &[] as &[f64]),
        (60.0, 1.0, &[]),
        (59.0, 1.0, &[60.0] as &[f64]),
        (ms_start, 151.0, &[]),
        (ms_start, 151.0, &ms_hols as &[f64]),
        (modern, 5.0, &[]),
        (modern, -5.0, &[]),
    ] {
        let a = workday_serial(start, days, hols, SYSTEM).unwrap();
        let b = workday_serial_walk(start, days, hols, SYSTEM).unwrap();
        assert_eq!(a, b, "fast vs walk mismatch for ({start}, {days})");
    }

    const SHORT_ITERS: u32 = 50_000;
    let mixed: &[(f64, f64, &[f64])] = &[
        (60.0, 0.0, &[]),
        (60.0, 1.0, &[]),
        (ms_start, 151.0, &[]),
        (ms_start, 151.0, &ms_hols),
        (modern, 20.0, &[]),
        (modern, -20.0, &[]),
    ];

    let t0 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        for &(start, days, hols) in mixed {
            run_pair(workday_serial_walk, start, days, hols);
        }
    }
    let walk_ns = t0.elapsed().as_nanos() / (SHORT_ITERS as u128 * mixed.len() as u128);

    let t1 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        for &(start, days, hols) in mixed {
            run_pair(workday_serial, start, days, hols);
        }
    }
    let fast_ns = t1.elapsed().as_nanos() / (SHORT_ITERS as u128 * mixed.len() as u128);

    let t2 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        black_box(
            workday_serial(
                black_box(ms_start),
                black_box(151.0),
                black_box(&[] as &[f64]),
                SYSTEM,
            )
            .unwrap(),
        );
    }
    let ms_ns = t2.elapsed().as_nanos() / SHORT_ITERS as u128;

    let t3 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        black_box(
            workday_serial(
                black_box(60.0),
                black_box(1.0),
                black_box(&[] as &[f64]),
                SYSTEM,
            )
            .unwrap(),
        );
    }
    let serial60_ns = t3.elapsed().as_nanos() / SHORT_ITERS as u128;

    const LONG_ITERS: u32 = 200;
    let long_days = 100_000.0; // ~385 years of weekdays; stays inside 9999-12-31
    let t4 = std::time::Instant::now();
    for _ in 0..LONG_ITERS {
        black_box(
            workday_serial_walk(
                black_box(epoch_monday),
                black_box(long_days),
                black_box(&[] as &[f64]),
                SYSTEM,
            )
            .unwrap(),
        );
    }
    let walk_century_ns = t4.elapsed().as_nanos() / LONG_ITERS as u128;

    let t5 = std::time::Instant::now();
    for _ in 0..LONG_ITERS {
        black_box(
            workday_serial(
                black_box(epoch_monday),
                black_box(long_days),
                black_box(&[] as &[f64]),
                SYSTEM,
            )
            .unwrap(),
        );
    }
    let fast_century_ns = t5.elapsed().as_nanos() / LONG_ITERS as u128;

    println!(
        "workday microbench  short_iters={SHORT_ITERS}  cases={}",
        mixed.len()
    );
    println!("  walk-ref mixed           {walk_ns} ns/call");
    println!("  workday_serial mixed     {fast_ns} ns/call");
    println!("  WORKDAY(MS 151)          {ms_ns} ns/call");
    println!("  WORKDAY(60,1)            {serial60_ns} ns/call");
    println!("  walk 200y weekdays       {walk_century_ns} ns/call");
    println!("  fast 200y weekdays       {fast_century_ns} ns/call");
    if walk_ns > 0 {
        println!(
            "  hill-climb speedup (mixed) {:.2}x vs day-walk",
            walk_ns as f64 / fast_ns.max(1) as f64
        );
    }
    if walk_century_ns > 0 {
        println!(
            "  hill-climb speedup (century) {:.2}x vs day-walk",
            walk_century_ns as f64 / fast_century_ns.max(1) as f64
        );
    }
}
