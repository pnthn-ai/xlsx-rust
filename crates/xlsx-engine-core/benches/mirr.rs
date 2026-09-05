//! Before/after microbench for Excel `MIRR`.
//!
//! Compares the sign-masked `powi` NPV baseline (`excel_mirr_naive`) with the
//! streaming production kernel (`excel_mirr`). Same Microsoft closed form.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench mirr
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_mirr, excel_mirr_naive};

const ITERS_HEAVY: u32 = 8;
const ITERS_LIGHT: u32 = 40;

struct Case {
    name: &'static str,
    values: Vec<f64>,
    finance: f64,
    reinvest: f64,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let ms = vec![-120000.0, 39000.0, 30000.0, 21000.0, 37000.0, 46000.0];
    let n1k = conventional(1_000, -10_000.0, 12.0);
    let n10k = conventional(10_000, -100_000.0, 12.0);
    let n100k = conventional(100_000, -1_000_000.0, 12.0);
    let mixed: Vec<f64> = (0..10_000)
        .map(|i| {
            if i == 0 {
                -5_000.0
            } else if i % 7 == 0 {
                -3.0
            } else if i % 11 == 0 {
                0.0
            } else {
                8.0
            }
        })
        .collect();
    vec![
        Case {
            name: "Microsoft 5-period",
            values: ms,
            finance: 0.1,
            reinvest: 0.12,
            iters: 2_000,
        },
        Case {
            name: "1k conventional 10%/12%",
            values: n1k,
            finance: 0.10,
            reinvest: 0.12,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "10k conventional 10%/12%",
            values: n10k,
            finance: 0.10,
            reinvest: 0.12,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "100k conventional 8%/10%",
            values: n100k,
            finance: 0.08,
            reinvest: 0.10,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "10k mixed signed 8%/10%",
            values: mixed,
            finance: 0.08,
            reinvest: 0.10,
            iters: ITERS_LIGHT,
        },
    ]
}

fn conventional(n: usize, outlay: f64, inflow: f64) -> Vec<f64> {
    let mut v = vec![inflow; n];
    v[0] = outlay;
    v
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
    println!("MIRR kernel bench (naive masked-NPV powi vs streaming)");
    println!(
        "{:<36} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(72));
    for c in cases() {
        let naive = time_it(c.iters, || {
            black_box(excel_mirr_naive(
                black_box(&c.values),
                black_box(c.finance),
                black_box(c.reinvest),
            ));
        });
        let fast = time_it(c.iters, || {
            black_box(excel_mirr(
                black_box(&c.values),
                black_box(c.finance),
                black_box(c.reinvest),
            ));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<36} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_mirr_naive(&c.values, c.finance, c.reinvest).unwrap();
        let b = excel_mirr(&c.values, c.finance, c.reinvest).unwrap();
        let scale = a.abs().max(b.abs()).max(1.0);
        assert!(
            (a - b).abs() / scale < 1e-9,
            "semantic mismatch on {}: {a} vs {b}",
            c.name
        );
    }
}
