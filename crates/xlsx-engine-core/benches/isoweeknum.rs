//! Before/after microbench for Excel `ISOWEEKNUM`.
//!
//! Compares the year-walk baseline (`excel_isoweeknum_naive`: `serial_to_ymd_walk`)
//! with the production kernel (`excel_isoweeknum`: closed-form year / day-of-year
//! plus O(1) Excel weekday).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench isoweeknum
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_isoweeknum, excel_isoweeknum_naive};
use xlsx_types::DateSystem;

const ITERS_NEAR: u32 = 80;
const ITERS_FAR: u32 = 40;
const ITERS_SWEEP: u32 = 20;

struct Case {
    name: &'static str,
    serials: Vec<f64>,
    system: DateSystem,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let leap: Vec<f64> = (0..=70).map(|s| s as f64).collect();
    let y2k: Vec<f64> = (36_526..=36_526 + 366).map(|s| s as f64).collect();
    let far = vec![2_958_465.0];
    let mixed: Vec<f64> = (0..10_000)
        .map(|i| (i * 293) as f64 % 2_958_466.0)
        .collect();
    vec![
        Case {
            name: "leap window 0..=70",
            serials: leap,
            system: DateSystem::Excel1900,
            iters: ITERS_NEAR,
        },
        Case {
            name: "year 2000 serials",
            serials: y2k,
            system: DateSystem::Excel1900,
            iters: ITERS_SWEEP,
        },
        Case {
            name: "9999-12-31",
            serials: far,
            system: DateSystem::Excel1900,
            iters: ITERS_FAR,
        },
        Case {
            name: "10k strided serials",
            serials: mixed,
            system: DateSystem::Excel1900,
            iters: ITERS_SWEEP,
        },
        Case {
            name: "1904 epoch 0..=365",
            serials: (0..=365).map(|s| s as f64).collect(),
            system: DateSystem::Excel1904,
            iters: ITERS_NEAR,
        },
    ]
}

fn time_it(iters: u32, mut f: impl FnMut()) -> Duration {
    f();
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    start.elapsed() / iters
}

fn fmt_dur(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us >= 1000.0 {
        format!("{:.2} ms", us / 1000.0)
    } else {
        format!("{us:.1} µs")
    }
}

fn run_kernel(f: fn(f64, DateSystem) -> Result<f64, xlsx_types::ExcelError>, c: &Case) {
    for &s in &c.serials {
        let _ = black_box(f(black_box(s), black_box(c.system)));
    }
}

fn main() {
    println!("ISOWEEKNUM kernel bench (YMD year-walk vs closed-form + serial modulo)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || run_kernel(excel_isoweeknum_naive, &c));
        let fast = time_it(c.iters, || run_kernel(excel_isoweeknum, &c));
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        for &s in &c.serials {
            let a = excel_isoweeknum_naive(s, c.system);
            let b = excel_isoweeknum(s, c.system);
            assert_eq!(a, b, "semantic mismatch serial={s}");
        }
    }
}
