//! Before/after microbench for Excel `WEEKDAY`.
//!
//! Compares the calendar-walk baseline (`excel_weekday_naive`: `serial_to_ymd`
//! + `ymd_to_serial_1900`) with the production kernel (`excel_weekday`: O(1)
//! modulo on the 1900-system serial).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench weekday
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_weekday, excel_weekday_naive};
use xlsx_types::DateSystem;

const ITERS_NEAR: u32 = 80;
const ITERS_FAR: u32 = 40;
const ITERS_SWEEP: u32 = 20;

struct Case {
    name: &'static str,
    serials: Vec<f64>,
    return_types: Vec<i32>,
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
    let types_all = vec![1, 2, 3, 11, 12, 13, 14, 15, 16, 17];
    vec![
        Case {
            name: "leap window 0..=70 × type 1",
            serials: leap,
            return_types: vec![1],
            system: DateSystem::Excel1900,
            iters: ITERS_NEAR,
        },
        Case {
            name: "year 2000 serials × all return_types",
            serials: y2k,
            return_types: types_all.clone(),
            system: DateSystem::Excel1900,
            iters: ITERS_SWEEP,
        },
        Case {
            name: "9999-12-31 × all return_types",
            serials: far,
            return_types: types_all.clone(),
            system: DateSystem::Excel1900,
            iters: ITERS_FAR,
        },
        Case {
            name: "10k strided serials × type 1",
            serials: mixed,
            return_types: vec![1],
            system: DateSystem::Excel1900,
            iters: ITERS_SWEEP,
        },
        Case {
            name: "1904 epoch 0..=365 × type 1",
            serials: (0..=365).map(|s| s as f64).collect(),
            return_types: vec![1],
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

fn run_kernel(
    f: fn(f64, i32, DateSystem) -> Result<f64, xlsx_types::ExcelError>,
    c: &Case,
) {
    for &s in &c.serials {
        for &rt in &c.return_types {
            let _ = black_box(f(black_box(s), black_box(rt), black_box(c.system)));
        }
    }
}

fn main() {
    println!("WEEKDAY kernel bench (YMD walk vs O(1) serial modulo)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || run_kernel(excel_weekday_naive, &c));
        let fast = time_it(c.iters, || run_kernel(excel_weekday, &c));
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        for &s in &c.serials {
            for &rt in &c.return_types {
                let a = excel_weekday_naive(s, rt, c.system);
                let b = excel_weekday(s, rt, c.system);
                assert_eq!(a, b, "semantic mismatch serial={s} return_type={rt}");
            }
        }
    }
}
