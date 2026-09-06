//! Microbenches for Excel `WORKDAY.INTL`.
//!
//! Compares the production helper (`workday_serial_intl`, O(1) weekly inversion
//! plus O(H) holiday adjust) against a day-walk reference, including custom
//! weekend codes and 7-character masks.

use std::hint::black_box;
use xlsx_engine_core::dates::{
    date_serial, parse_weekend_mask, weekend_mask_from_code, weekend_mask_from_string,
    workday_serial_intl, workday_serial_intl_walk, WEEKEND_MASK_SAT_SUN,
};
use xlsx_types::{DateSystem, ExcelValue};

const SYSTEM: DateSystem = DateSystem::Excel1900;

fn run_pair(
    f: fn(f64, f64, u8, &[f64], DateSystem) -> Result<f64, xlsx_types::ExcelError>,
    start: f64,
    days: f64,
    weekend: u8,
    hols: &[f64],
) {
    black_box(
        f(
            black_box(start),
            black_box(days),
            black_box(weekend),
            black_box(hols),
            SYSTEM,
        )
        .unwrap(),
    );
}

fn main() {
    let ms_start = date_serial(2008, 10, 1, SYSTEM).unwrap();
    let modern = date_serial(2024, 1, 4, SYSTEM).unwrap();
    let jan2012 = date_serial(2012, 1, 1, SYSTEM).unwrap();
    let epoch_monday = 2.0;
    let h1 = date_serial(2008, 11, 26, SYSTEM).unwrap();
    let h2 = date_serial(2008, 12, 4, SYSTEM).unwrap();
    let h3 = date_serial(2009, 1, 21, SYSTEM).unwrap();
    let ms_hols = [h1, h2, h3];
    let sat_sun = WEEKEND_MASK_SAT_SUN;
    let sun_only = weekend_mask_from_code(11).unwrap();
    let fri_sat = weekend_mask_from_code(7).unwrap();
    let none = weekend_mask_from_string("0000000").unwrap();
    let custom = weekend_mask_from_string("1010100").unwrap();

    assert_eq!(
        parse_weekend_mask(Some(&ExcelValue::Text("0000011".into()))).unwrap(),
        sat_sun
    );

    for &(start, days, weekend, hols) in &[
        (60.0, 0.0, sat_sun, &[] as &[f64]),
        (60.0, 1.0, sat_sun, &[]),
        (59.0, 1.0, sun_only, &[60.0] as &[f64]),
        (ms_start, 151.0, sat_sun, &[]),
        (ms_start, 151.0, sat_sun, &ms_hols as &[f64]),
        (jan2012, 90.0, sun_only, &[]),
        (jan2012, 30.0, weekend_mask_from_code(17).unwrap(), &[]),
        (modern, 5.0, custom, &[]),
        (modern, -5.0, fri_sat, &[]),
        (modern, 1.0, none, &[]),
    ] {
        let a = workday_serial_intl(start, days, weekend, hols, SYSTEM).unwrap();
        let b = workday_serial_intl_walk(start, days, weekend, hols, SYSTEM).unwrap();
        assert_eq!(
            a, b,
            "fast vs walk mismatch for ({start}, {days}, {weekend:#08b})"
        );
    }

    const SHORT_ITERS: u32 = 50_000;
    let mixed: &[(f64, f64, u8, &[f64])] = &[
        (60.0, 0.0, sat_sun, &[]),
        (60.0, 1.0, sat_sun, &[]),
        (ms_start, 151.0, sat_sun, &[]),
        (ms_start, 151.0, sat_sun, &ms_hols),
        (jan2012, 90.0, sun_only, &[]),
        (modern, 20.0, custom, &[]),
        (modern, -20.0, fri_sat, &[]),
    ];

    let t0 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        for &(start, days, weekend, hols) in mixed {
            run_pair(workday_serial_intl_walk, start, days, weekend, hols);
        }
    }
    let walk_ns = t0.elapsed().as_nanos() / (SHORT_ITERS as u128 * mixed.len() as u128);

    let t1 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        for &(start, days, weekend, hols) in mixed {
            run_pair(workday_serial_intl, start, days, weekend, hols);
        }
    }
    let fast_ns = t1.elapsed().as_nanos() / (SHORT_ITERS as u128 * mixed.len() as u128);

    let t2 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        black_box(
            workday_serial_intl(
                black_box(jan2012),
                black_box(90.0),
                black_box(sun_only),
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
            workday_serial_intl(
                black_box(60.0),
                black_box(1.0),
                black_box(sat_sun),
                black_box(&[] as &[f64]),
                SYSTEM,
            )
            .unwrap(),
        );
    }
    let serial60_ns = t3.elapsed().as_nanos() / SHORT_ITERS as u128;

    const LONG_ITERS: u32 = 200;
    let long_days = 100_000.0;
    let t4 = std::time::Instant::now();
    for _ in 0..LONG_ITERS {
        black_box(
            workday_serial_intl_walk(
                black_box(epoch_monday),
                black_box(long_days),
                black_box(custom),
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
            workday_serial_intl(
                black_box(epoch_monday),
                black_box(long_days),
                black_box(custom),
                black_box(&[] as &[f64]),
                SYSTEM,
            )
            .unwrap(),
        );
    }
    let fast_century_ns = t5.elapsed().as_nanos() / LONG_ITERS as u128;

    println!(
        "workday.intl microbench  short_iters={SHORT_ITERS}  cases={}",
        mixed.len()
    );
    println!("  walk-ref mixed              {walk_ns} ns/call");
    println!("  workday_serial_intl mixed   {fast_ns} ns/call");
    println!("  WORKDAY.INTL(90, Sun-only)  {ms_ns} ns/call");
    println!("  WORKDAY.INTL(60,1)          {serial60_ns} ns/call");
    println!("  walk 200y custom weekend    {walk_century_ns} ns/call");
    println!("  fast 200y custom weekend    {fast_century_ns} ns/call");
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
