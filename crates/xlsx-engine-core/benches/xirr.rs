//! Before/after microbench for Excel `XIRR`.
//!
//! Compares the per-term `powf` baseline (`excel_xirr_naive`) with the
//! `exp` / hoisted `ln1p` production kernel (`excel_xirr`). Same Newton /
//! bisection rules.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench xirr
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_xirr, excel_xirr_naive};
use xlsx_types::excel_num_eq;

const ITERS_HEAVY: u32 = 8;
const ITERS_LIGHT: u32 = 40;

struct Case {
    name: &'static str,
    values: Vec<f64>,
    dates: Vec<i32>,
    guess: f64,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let ms_v = vec![-10000.0, 2750.0, 4250.0, 3250.0, 2750.0];
    let ms_d = vec![39448, 39508, 39751, 39859, 39904];
    let n1k = conventional(1_000, -10_000.0, 12.0, 14);
    let n10k = conventional(10_000, -100_000.0, 12.0, 7);
    let clustered = {
        let n = 10_000;
        let mut values = vec![1.0; n];
        values[0] = -5_000.0;
        values[n - 1] = 8_000.0;
        let mut dates = vec![40000; n];
        dates[n - 1] = 40000 + 365;
        (values, dates)
    };
    vec![
        Case {
            name: "Microsoft 5-flow example",
            values: ms_v,
            dates: ms_d,
            guess: 0.1,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "1k biweekly (guess 0.1)",
            values: n1k.0,
            dates: n1k.1,
            guess: 0.1,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "10k weekly (guess 0.1)",
            values: n10k.0.clone(),
            dates: n10k.1.clone(),
            guess: 0.1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "10k weekly (guess 0.5)",
            values: n10k.0,
            dates: n10k.1,
            guess: 0.5,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "10k clustered same-day",
            values: clustered.0,
            dates: clustered.1,
            guess: 0.1,
            iters: ITERS_HEAVY,
        },
    ]
}

fn conventional(n: usize, outlay: f64, inflow: f64, step_days: i32) -> (Vec<f64>, Vec<i32>) {
    let mut values = vec![inflow; n];
    values[0] = outlay;
    let dates: Vec<i32> = (0..n).map(|i| 40000 + (i as i32) * step_days).collect();
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
    println!("XIRR kernel bench (naive powf vs exp/ln1p Newton/bisection)");
    println!(
        "{:<36} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(72));
    for c in cases() {
        let naive = time_it(c.iters, || {
            black_box(excel_xirr_naive(
                black_box(&c.values),
                black_box(&c.dates),
                black_box(c.guess),
            ));
        });
        let fast = time_it(c.iters, || {
            black_box(excel_xirr(
                black_box(&c.values),
                black_box(&c.dates),
                black_box(c.guess),
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
        let a = excel_xirr_naive(&c.values, &c.dates, c.guess);
        let b = excel_xirr(&c.values, &c.dates, c.guess);
        match (a, b) {
            (Some(x), Some(y)) => {
                let scale = x.abs().max(y.abs()).max(1.0);
                assert!(
                    (x - y).abs() / scale <= 1e-12 || excel_num_eq(x, y),
                    "semantic mismatch on {}: {x} vs {y}",
                    c.name
                );
            }
            (None, None) => {}
            other => panic!("Option mismatch on {}: {other:?}", c.name),
        }
    }
}
