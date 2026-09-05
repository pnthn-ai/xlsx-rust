//! Before/after microbench for Excel `NPV`.
//!
//! Compares the per-period `powi` baseline (`excel_npv_naive`) with the
//! Horner production kernel (`excel_npv`).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench npv
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_npv, excel_npv_naive};

const ITERS_HEAVY: u32 = 8;
const ITERS_LIGHT: u32 = 40;

struct Case {
    name: &'static str,
    rate: f64,
    values: Vec<f64>,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let n10k: Vec<f64> = (1..=10_000).map(|i| i as f64).collect();
    let n100k: Vec<f64> = (1..=100_000).map(|i| i as f64).collect();
    let mixed: Vec<f64> = (1..=10_000)
        .map(|i| {
            if i % 7 == 0 {
                0.0
            } else {
                (i % 50) as f64 - 10.0
            }
        })
        .collect();
    vec![
        Case {
            name: "10k cash flows, rate 1%",
            rate: 0.01,
            values: n10k,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "100k cash flows, rate 0.1%",
            rate: 0.001,
            values: n100k,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "10k mixed signed, rate 8%",
            rate: 0.08,
            values: mixed,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "Microsoft 4-flow example",
            rate: 0.1,
            values: vec![-10000.0, 3000.0, 4200.0, 6800.0],
            iters: 2_000,
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

fn main() {
    println!("NPV kernel bench (naive powi vs Horner)");
    println!(
        "{:<40} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(76));
    for c in cases() {
        let naive = time_it(c.iters, || {
            black_box(excel_npv_naive(black_box(c.rate), black_box(&c.values)).unwrap());
        });
        let fast = time_it(c.iters, || {
            black_box(excel_npv(black_box(c.rate), black_box(&c.values)).unwrap());
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<40} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_npv_naive(c.rate, &c.values).unwrap();
        let b = excel_npv(c.rate, &c.values).unwrap();
        let scale = a.abs().max(b.abs()).max(1.0);
        assert!(
            (a - b).abs() / scale < 1e-9,
            "semantic mismatch on {}: {a} vs {b}",
            c.name
        );
    }
}
