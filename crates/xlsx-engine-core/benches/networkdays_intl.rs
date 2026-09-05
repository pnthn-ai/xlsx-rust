//! Microbenches for Excel `NETWORKDAYS.INTL` (weekend codes / strings).
//!
//! Compares the production O(1) masked weekday count against a day-walk
//! reference that visits every serial in the span.

use std::hint::black_box;
use xlsx_engine_core::dates::{
    date_serial, networkdays_count_mask, networkdays_count_mask_walk, weekend_mask_from_code,
    weekend_mask_from_string, WEEKEND_SAT_SUN,
};
use xlsx_types::DateSystem;

const SYSTEM: DateSystem = DateSystem::Excel1900;

fn main() {
    let ms_start = date_serial(2006, 1, 1, SYSTEM).unwrap();
    let ms_end = date_serial(2006, 2, 1, SYSTEM).unwrap();
    let h1 = date_serial(2006, 1, 2, SYSTEM).unwrap();
    let h2 = date_serial(2006, 1, 16, SYSTEM).unwrap();
    let net_start = date_serial(2012, 10, 1, SYSTEM).unwrap();
    let net_end = date_serial(2013, 3, 1, SYSTEM).unwrap();
    let century_end = date_serial(2100, 1, 1, SYSTEM).unwrap();
    let mask7 = weekend_mask_from_code(7).unwrap();
    let mask11 = weekend_mask_from_code(11).unwrap();
    let mask_mwf = weekend_mask_from_string("0101011").unwrap();
    let hols = [h1, h2];

    for &(start, end, mask, hols) in &[
        (60.0, 60.0, WEEKEND_SAT_SUN, &[] as &[f64]),
        (ms_start, ms_end, mask7, hols.as_slice()),
        (net_start, net_end, WEEKEND_SAT_SUN, &[]),
        (net_start, net_end, mask11, &[]),
        (net_start, net_end, mask_mwf, &[]),
        (1.0, 367.0, 0, &[]),
    ] {
        let a = networkdays_count_mask(start, end, mask, hols, SYSTEM).unwrap();
        let b = networkdays_count_mask_walk(start, end, mask, hols, SYSTEM).unwrap();
        assert_eq!(a, b, "fast vs walk mismatch for ({start}, {end}, {mask})");
    }

    const SHORT_ITERS: u32 = 50_000;
    let short: &[((f64, f64, u8), &[f64])] = &[
        ((60.0, 60.0, WEEKEND_SAT_SUN), &[]),
        ((ms_start, ms_end, mask7), hols.as_slice()),
        ((net_start, net_end, WEEKEND_SAT_SUN), &[]),
        ((net_start, net_end, mask11), &[]),
        ((net_start, net_end, mask_mwf), &[]),
        ((1.0, 367.0, 0), &[]),
    ];

    let t0 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        for &((start, end, mask), hols) in short {
            black_box(
                networkdays_count_mask_walk(
                    black_box(start),
                    black_box(end),
                    black_box(mask),
                    hols,
                    SYSTEM,
                )
                .unwrap(),
            );
        }
    }
    let walk_ns = t0.elapsed().as_nanos() / (SHORT_ITERS as u128 * short.len() as u128);

    let t1 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        for &((start, end, mask), hols) in short {
            black_box(
                networkdays_count_mask(
                    black_box(start),
                    black_box(end),
                    black_box(mask),
                    hols,
                    SYSTEM,
                )
                .unwrap(),
            );
        }
    }
    let fast_ns = t1.elapsed().as_nanos() / (SHORT_ITERS as u128 * short.len() as u128);

    let t2 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        black_box(
            networkdays_count_mask(
                black_box(ms_start),
                black_box(ms_end),
                black_box(mask7),
                hols.as_slice(),
                SYSTEM,
            )
            .unwrap(),
        );
    }
    let ms_ns = t2.elapsed().as_nanos() / SHORT_ITERS as u128;

    let t3 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        black_box(
            networkdays_count_mask(
                black_box(60.0),
                black_box(60.0),
                black_box(mask11),
                &[],
                SYSTEM,
            )
            .unwrap(),
        );
    }
    let serial60_ns = t3.elapsed().as_nanos() / SHORT_ITERS as u128;

    const LONG_ITERS: u32 = 200;
    let t4 = std::time::Instant::now();
    for _ in 0..LONG_ITERS {
        black_box(
            networkdays_count_mask_walk(
                black_box(1.0),
                black_box(century_end),
                black_box(mask_mwf),
                &[],
                SYSTEM,
            )
            .unwrap(),
        );
    }
    let long_walk_ns = t4.elapsed().as_nanos() / LONG_ITERS as u128;

    let t5 = std::time::Instant::now();
    for _ in 0..SHORT_ITERS {
        black_box(
            networkdays_count_mask(
                black_box(1.0),
                black_box(century_end),
                black_box(mask_mwf),
                &[],
                SYSTEM,
            )
            .unwrap(),
        );
    }
    let long_fast_ns = t5.elapsed().as_nanos() / SHORT_ITERS as u128;

    println!(
        "networkdays_intl microbench  short_iters={SHORT_ITERS}  cases={}",
        short.len()
    );
    println!("  walk-ref mixed                {walk_ns} ns/call");
    println!("  networkdays_count_mask mixed  {fast_ns} ns/call");
    println!("  NETWORKDAYS.INTL(MS + 7)      {ms_ns} ns/call");
    println!("  NETWORKDAYS.INTL(60,60,11)    {serial60_ns} ns/call");
    println!("  walk 1900→2100 mask 0101011   {long_walk_ns} ns/call");
    println!("  fast 1900→2100 mask 0101011   {long_fast_ns} ns/call");
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
