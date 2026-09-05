//! Before/after microbench for Excel `ROUNDUP` / `ROUNDDOWN`.
//!
//! Compares the textbook two-`powi` baseline with the specialized production
//! kernel (digit-0 / ±1 / ±2 / ±3 fast paths + table scale) across signed
//! values and negative `num_digits`.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench roundup_rounddown
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{
    excel_rounddown, excel_rounddown_naive, excel_roundup, excel_roundup_naive,
};

const ITERS: u32 = 80;
const N: usize = 50_000;

struct Case {
    name: &'static str,
    values: Vec<f64>,
    digits: Vec<i32>,
}

fn cases() -> Vec<Case> {
    let mixed_n: Vec<f64> = (0..N)
        .map(|i| {
            let s = if i % 2 == 0 { 1.0 } else { -1.0 };
            s * (i as f64) * 0.137 + 0.15
        })
        .collect();
    let mixed_d: Vec<i32> = (0..N)
        .map(|i| match i % 7 {
            0 => 0,
            1 => 1,
            2 => 2,
            3 => 3,
            4 => -1,
            5 => -2,
            _ => -3,
        })
        .collect();
    let tens: Vec<f64> = (0..N).map(|i| 1000.0 + i as f64 * 0.37).collect();
    let ones: Vec<f64> = (0..N)
        .map(|i| {
            if i % 3 == 0 {
                -(i as f64) - 0.5
            } else {
                i as f64 + 0.25
            }
        })
        .collect();
    let decimals: Vec<f64> = (0..N).map(|i| 1.1 + (i as f64) * 1e-4).collect();
    vec![
        Case {
            name: "50k mixed signed × mixed digits",
            values: mixed_n,
            digits: mixed_d,
        },
        Case {
            name: "50k positive × num_digits=-1",
            values: tens,
            digits: vec![-1; N],
        },
        Case {
            name: "50k signed × num_digits=0",
            values: ones,
            digits: vec![0; N],
        },
        Case {
            name: "50k 1.1+eps × num_digits=2 (IEEE snap)",
            values: decimals,
            digits: vec![2; N],
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

fn fold_up_naive(values: &[f64], digits: &[i32]) -> f64 {
    let mut acc = 0.0;
    for (n, d) in values.iter().zip(digits.iter()) {
        acc += excel_roundup_naive(*n, *d);
    }
    acc
}

fn fold_up_fast(values: &[f64], digits: &[i32]) -> f64 {
    let mut acc = 0.0;
    for (n, d) in values.iter().zip(digits.iter()) {
        acc += excel_roundup(*n, *d);
    }
    acc
}

fn fold_down_naive(values: &[f64], digits: &[i32]) -> f64 {
    let mut acc = 0.0;
    for (n, d) in values.iter().zip(digits.iter()) {
        acc += excel_rounddown_naive(*n, *d);
    }
    acc
}

fn fold_down_fast(values: &[f64], digits: &[i32]) -> f64 {
    let mut acc = 0.0;
    for (n, d) in values.iter().zip(digits.iter()) {
        acc += excel_rounddown(*n, *d);
    }
    acc
}

fn main() {
    println!("ROUNDUP / ROUNDDOWN kernel bench (two-powi baseline vs specialized)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        for (label, naive_fold, fast_fold) in [
            (
                "ROUNDUP",
                fold_up_naive as fn(&[f64], &[i32]) -> f64,
                fold_up_fast as fn(&[f64], &[i32]) -> f64,
            ),
            (
                "ROUNDDOWN",
                fold_down_naive as fn(&[f64], &[i32]) -> f64,
                fold_down_fast as fn(&[f64], &[i32]) -> f64,
            ),
        ] {
            let naive = time_it(ITERS, || {
                black_box(naive_fold(black_box(&c.values), black_box(&c.digits)));
            });
            let fast = time_it(ITERS, || {
                black_box(fast_fold(black_box(&c.values), black_box(&c.digits)));
            });
            let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
            println!(
                "{:<42} {:>12} {:>12} {:>7.1}x",
                format!("{} / {}", label, c.name),
                fmt_dur(naive),
                fmt_dur(fast),
                speedup
            );
            let a = naive_fold(&c.values, &c.digits);
            let b = fast_fold(&c.values, &c.digits);
            assert_eq!(a, b, "semantic mismatch on {} / {}", label, c.name);
        }
    }
}
