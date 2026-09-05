//! Before/after microbench for Excel `ROUNDUP` / `ROUNDDOWN`.
//!
//! Compares the `powi`-every-call baseline with the table + integer-scale
//! production kernel across signed values and negative `num_digits`.
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

struct Case {
    name: &'static str,
    values: Vec<f64>,
    digits: Vec<i32>,
}

fn cases() -> Vec<Case> {
    let mixed_n: Vec<f64> = (0..50_000)
        .map(|i| {
            let s = if i % 2 == 0 { 1.0 } else { -1.0 };
            s * (i as f64) * 0.137 + 0.15
        })
        .collect();
    let mixed_d: Vec<i32> = (0..50_000)
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
    let tens: Vec<f64> = (0..50_000).map(|i| 1000.0 + i as f64 * 0.37).collect();
    let tens_d = vec![-1i32; 50_000];
    let ones: Vec<f64> = (0..50_000)
        .map(|i| {
            if i % 3 == 0 {
                -(i as f64) - 0.5
            } else {
                i as f64 + 0.25
            }
        })
        .collect();
    let zero_d = vec![0i32; 50_000];
    let decimals: Vec<f64> = (0..50_000).map(|i| 1.1 + (i as f64) * 1e-4).collect();
    let two_d = vec![2i32; 50_000];
    vec![
        Case {
            name: "50k mixed signed × mixed digits",
            values: mixed_n,
            digits: mixed_d,
        },
        Case {
            name: "50k positive × num_digits=-1",
            values: tens,
            digits: tens_d,
        },
        Case {
            name: "50k signed × num_digits=0",
            values: ones,
            digits: zero_d,
        },
        Case {
            name: "50k 1.1+eps × num_digits=2 (IEEE snap)",
            values: decimals,
            digits: two_d,
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

fn fold_up(values: &[f64], digits: &[i32], naive: bool) -> f64 {
    let mut acc = 0.0;
    for (n, d) in values.iter().zip(digits.iter()) {
        acc += if naive {
            excel_roundup_naive(*n, *d)
        } else {
            excel_roundup(*n, *d)
        };
    }
    acc
}

fn fold_down(values: &[f64], digits: &[i32], naive: bool) -> f64 {
    let mut acc = 0.0;
    for (n, d) in values.iter().zip(digits.iter()) {
        acc += if naive {
            excel_rounddown_naive(*n, *d)
        } else {
            excel_rounddown(*n, *d)
        };
    }
    acc
}

fn main() {
    println!("ROUNDUP / ROUNDDOWN kernel bench (powi baseline vs table+scale)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        for (label, fold) in [
            ("ROUNDUP", fold_up as fn(&[f64], &[i32], bool) -> f64),
            ("ROUNDDOWN", fold_down as fn(&[f64], &[i32], bool) -> f64),
        ] {
            let naive = time_it(ITERS, || {
                black_box(fold(
                    black_box(&c.values),
                    black_box(&c.digits),
                    black_box(true),
                ));
            });
            let fast = time_it(ITERS, || {
                black_box(fold(
                    black_box(&c.values),
                    black_box(&c.digits),
                    black_box(false),
                ));
            });
            let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
            println!(
                "{:<42} {:>12} {:>12} {:>7.1}x",
                format!("{} / {}", label, c.name),
                fmt_dur(naive),
                fmt_dur(fast),
                speedup
            );
            let a = fold(&c.values, &c.digits, true);
            let b = fold(&c.values, &c.digits, false);
            assert_eq!(a, b, "semantic mismatch on {} / {}", label, c.name);
        }
    }
}
