//! Before/after microbench for Excel `XNPV`.
//!
//! Compares the per-term `powf` baseline (`excel_xnpv_naive`) with the
//! `exp`/`ln1p` production kernel (`excel_xnpv`).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench xnpv
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_xnpv, excel_xnpv_naive};

const ITERS_HEAVY: u32 = 8;
const ITERS_LIGHT: u32 = 40;

struct Case {
    name: &'static str,
    rate: f64,
    values: Vec<f64>,
    dates: Vec<i32>,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let ms_v = vec![-10000.0, 2750.0, 4250.0, 3250.0, 2750.0];
    let ms_d = vec![39448, 39508, 39751, 39859, 39904];
    let n10k = irregular(10_000, 39448, 14);
    let n100k = irregular(100_000, 39448, 7);
    let clustered = {
        let mut values = Vec::with_capacity(10_000);
        let mut dates = Vec::with_capacity(10_000);
        let mut d = 39448;
        for i in 0..10_000 {
            if i % 20 == 0 {
                d += 30;
            }
            values.push(if i == 0 { -50_000.0 } else { (i % 40) as f64 });
            dates.push(d);
        }
        (values, dates)
    };
    vec![
        Case {
            name: "Microsoft 5-flow example",
            rate: 0.09,
            values: ms_v,
            dates: ms_d,
            iters: 2_000,
        },
        Case {
            name: "10k irregular (biweekly)",
            rate: 0.08,
            values: n10k.0,
            dates: n10k.1,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "100k irregular (weekly)",
            rate: 0.06,
            values: n100k.0,
            dates: n100k.1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "10k clustered same-day",
            rate: 0.1,
            values: clustered.0,
            dates: clustered.1,
            iters: ITERS_LIGHT,
        },
    ]
}

fn irregular(n: usize, start: i32, step: i32) -> (Vec<f64>, Vec<i32>) {
    let mut values = Vec::with_capacity(n);
    let mut dates = Vec::with_capacity(n);
    for i in 0..n {
        values.push(if i == 0 {
            -(n as f64)
        } else {
            (i % 50) as f64 + 1.0
        });
        dates.push(start + (i as i32) * step);
    }
    (values, dates)
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
    println!("XNPV kernel bench (naive powf vs exp/ln1p)");
    println!(
        "{:<36} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(72));
    for c in cases() {
        let naive = time_it(c.iters, || {
            black_box(
                excel_xnpv_naive(black_box(c.rate), black_box(&c.values), black_box(&c.dates))
                    .unwrap(),
            );
        });
        let fast = time_it(c.iters, || {
            black_box(
                excel_xnpv(black_box(c.rate), black_box(&c.values), black_box(&c.dates)).unwrap(),
            );
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<36} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_xnpv_naive(c.rate, &c.values, &c.dates).unwrap();
        let b = excel_xnpv(c.rate, &c.values, &c.dates).unwrap();
        let scale = a.abs().max(b.abs()).max(1.0);
        assert!(
            (a - b).abs() / scale < 1e-9,
            "semantic mismatch on {}: {a} vs {b}",
            c.name
        );
    }
}
