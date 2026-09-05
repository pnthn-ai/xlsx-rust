//! Before/after microbench for Excel `IRR`.
//!
//! Compares the per-term `pow` baseline (`excel_irr_naive`) with the
//! Horner production kernel (`excel_irr`). Same Newton / secant rules.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench irr
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_irr, excel_irr_naive};

const ITERS_HEAVY: u32 = 8;
const ITERS_LIGHT: u32 = 40;

struct Case {
    name: &'static str,
    values: Vec<f64>,
    guess: f64,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let ms = vec![-70000.0, 12000.0, 15000.0, 18000.0, 21000.0, 26000.0];
    let n1k = conventional(1_000, -10_000.0, 12.0);
    let n10k = conventional(10_000, -100_000.0, 12.0);
    let alternating = {
        let mut v = Vec::with_capacity(2_000);
        v.push(-5_000.0);
        for i in 0..1_999 {
            v.push(if i % 2 == 0 { 8.0 } else { -3.0 });
        }
        v
    };
    vec![
        Case {
            name: "Microsoft 5-period",
            values: ms,
            guess: 0.1,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "1k conventional (guess 0.1)",
            values: n1k,
            guess: 0.1,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "10k conventional (guess 0.1)",
            values: n10k.clone(),
            guess: 0.1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "10k conventional (guess 0.5)",
            values: n10k,
            guess: 0.5,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "2k alternating signs",
            values: alternating,
            guess: 0.1,
            iters: ITERS_HEAVY,
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
    println!("IRR kernel bench (naive pow vs Horner Newton/secant)");
    println!(
        "{:<36} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(72));
    for c in cases() {
        let naive = time_it(c.iters, || {
            black_box(excel_irr_naive(black_box(&c.values), black_box(c.guess)));
        });
        let fast = time_it(c.iters, || {
            black_box(excel_irr(black_box(&c.values), black_box(c.guess)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<36} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_irr_naive(&c.values, c.guess);
        let b = excel_irr(&c.values, c.guess);
        match (a, b) {
            (Some(x), Some(y)) => {
                assert!(
                    (x - y).abs() <= 1e-12,
                    "semantic mismatch on {}: {x} vs {y}",
                    c.name
                );
            }
            (None, None) => {}
            other => panic!("Option mismatch on {}: {other:?}", c.name),
        }
    }
}
