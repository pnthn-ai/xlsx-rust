//! Before/after microbench for Excel `EFFECT`.
//!
//! Compares the textbook `powf` baseline (`excel_effect_naive`) with the
//! production kernel (`excel_effect`: n=1 identity, n=2 closed form,
//! `powi` / `expm1(n·ln1p(r/n))`).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench effect
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_effect, excel_effect_naive};

const ITERS_LIGHT: u32 = 80;
const ITERS_HEAVY: u32 = 20;
const N: usize = 50_000;

struct Case {
    name: &'static str,
    rates: Vec<f64>,
    npery: f64,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let rates: Vec<f64> = (0..N).map(|i| 0.01 + (i as f64) * 1e-6).collect();
    let tiny: Vec<f64> = (0..N).map(|i| 1e-12 * (1 + i % 97) as f64).collect();
    vec![
        Case {
            name: "50k rates × npery=1 (identity)",
            rates: rates.clone(),
            npery: 1.0,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "50k rates × npery=2 (closed form)",
            rates: rates.clone(),
            npery: 2.0,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "50k rates × npery=4 (quarterly)",
            rates: rates.clone(),
            npery: 4.0,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "50k rates × npery=12 (monthly)",
            rates: rates.clone(),
            npery: 12.0,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "50k rates × npery=365 (daily)",
            rates,
            npery: 365.0,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "50k tiny rates × npery=12 (expm1)",
            rates: tiny,
            npery: 12.0,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "Microsoft EFFECT(0.0525, 4)",
            rates: vec![0.0525; 10_000],
            npery: 4.0,
            iters: 400,
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

fn fold(f: fn(f64, f64) -> Result<f64, xlsx_types::ExcelError>, rates: &[f64], npery: f64) -> f64 {
    let mut acc = 0.0;
    for &r in rates {
        if let Ok(v) = f(r, npery) {
            acc += v;
        }
    }
    acc
}

fn main() {
    println!("EFFECT kernel bench (powf baseline vs specialized)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            black_box(fold(
                excel_effect_naive,
                black_box(&c.rates),
                black_box(c.npery),
            ));
        });
        let fast = time_it(c.iters, || {
            black_box(fold(excel_effect, black_box(&c.rates), black_box(c.npery)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        for &r in c.rates.iter().step_by((c.rates.len() / 16).max(1)) {
            let a = excel_effect_naive(r, c.npery);
            let b = excel_effect(r, c.npery);
            match (a, b) {
                (Ok(a), Ok(b)) => {
                    let scale = a.abs().max(b.abs()).max(1e-18);
                    // Tiny rates: naive powf cancels; optimized keeps the term.
                    if r >= 1e-8 {
                        assert!(
                            (a - b).abs() / scale < 1e-9,
                            "semantic mismatch r={r} n={}: {a} vs {b}",
                            c.npery
                        );
                    }
                }
                (Err(ea), Err(eb)) => assert_eq!(ea, eb, "error mismatch r={r}"),
                other => panic!("domain mismatch r={r} n={}: {other:?}", c.npery),
            }
        }
    }
}
