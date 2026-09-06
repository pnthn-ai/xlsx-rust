//! Before/after microbench for Excel `ROUNDUP`.
//!
//! Compares the textbook two-`powi` baseline (`roundup_naive` / slice) with
//! the specialized production kernel (digit-0 / ±1 / ±2 / ±3 fast paths +
//! table scale) across signed values, omitted/`0` digits, and negative
//! `num_digits`.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench roundup
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{
    excel_roundup, excel_roundup_naive, excel_roundup_slice, excel_roundup_slice_naive,
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
            name: "50k signed × omitted/0 digits",
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

fn fold_naive(values: &[f64], digits: &[i32]) -> f64 {
    let mut acc = 0.0;
    for (n, d) in values.iter().zip(digits.iter()) {
        acc += excel_roundup_naive(*n, *d);
    }
    acc
}

fn fold_fast(values: &[f64], digits: &[i32]) -> f64 {
    let mut acc = 0.0;
    for (n, d) in values.iter().zip(digits.iter()) {
        acc += excel_roundup(*n, *d);
    }
    acc
}

fn row(name: &str, naive: Duration, fast: Duration) {
    let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
    println!(
        "{:<42} {:>12} {:>12} {:>7.1}x",
        name,
        fmt_dur(naive),
        fmt_dur(fast),
        speedup
    );
}

fn main() {
    println!("ROUNDUP kernel bench (two-powi baseline vs specialized)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(ITERS, || {
            black_box(fold_naive(black_box(&c.values), black_box(&c.digits)));
        });
        let fast = time_it(ITERS, || {
            black_box(fold_fast(black_box(&c.values), black_box(&c.digits)));
        });
        row(&format!("scalar / {}", c.name), naive, fast);
        let a = fold_naive(&c.values, &c.digits);
        let b = fold_fast(&c.values, &c.digits);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);

        let mut out_a = vec![0.0; c.values.len()];
        let mut out_b = vec![0.0; c.values.len()];
        let naive = time_it(ITERS, || {
            excel_roundup_slice_naive(
                black_box(&c.values),
                black_box(&c.digits),
                black_box(&mut out_a),
            );
        });
        let fast = time_it(ITERS, || {
            excel_roundup_slice(
                black_box(&c.values),
                black_box(&c.digits),
                black_box(&mut out_b),
            );
        });
        row(&format!("slice  / {}", c.name), naive, fast);
        excel_roundup_slice_naive(&c.values, &c.digits, &mut out_a);
        excel_roundup_slice(&c.values, &c.digits, &mut out_b);
        assert_eq!(out_a, out_b, "slice mismatch on {}", c.name);
    }
}
