//! Before/after microbench for Excel `ABS`.
//!
//! Compares the comparison-branch baseline (`excel_abs_naive` /
//! `excel_abs_value_naive` / `excel_abs_slice_naive`) with the production
//! sign-bit-clear kernel.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench abs
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{
    excel_abs, excel_abs_naive, excel_abs_slice, excel_abs_slice_naive, excel_abs_value,
    excel_abs_value_naive,
};
use xlsx_types::ExcelValue;

const ITERS_HEAVY: u32 = 80;
const ITERS_LIGHT: u32 = 160;

struct F64Case {
    name: &'static str,
    numbers: Vec<f64>,
    iters: u32,
}

struct ValueCase {
    name: &'static str,
    values: Vec<ExcelValue>,
    iters: u32,
}

fn f64_cases() -> Vec<F64Case> {
    let mixed: Vec<f64> = (0..200_000)
        .map(|i| match i % 5 {
            0 => -(i as f64),
            1 => i as f64,
            2 => -0.0,
            3 => -1e-12 * (i as f64),
            _ => 1e12 - i as f64,
        })
        .collect();
    vec![
        F64Case {
            name: "200k ABS(-7) integer hot path",
            numbers: vec![-7.0; 200_000],
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "200k ABS(2) positive",
            numbers: vec![2.0; 200_000],
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "200k alternating sign",
            numbers: (0..200_000)
                .map(|i| if i % 2 == 0 { i as f64 } else { -(i as f64) })
                .collect(),
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "200k mixed sign / -0 / tiny",
            numbers: mixed,
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "Microsoft ABS(-2) × 50k",
            numbers: vec![-2.0; 50_000],
            iters: ITERS_LIGHT,
        },
    ]
}

fn value_cases() -> Vec<ValueCase> {
    vec![
        ValueCase {
            name: "50k Number(-7) value hot path",
            values: vec![ExcelValue::Number(-7.0); 50_000],
            iters: ITERS_LIGHT,
        },
        ValueCase {
            name: "50k text \"-7\" coerce",
            values: vec![ExcelValue::Text("-7".into()); 50_000],
            iters: ITERS_LIGHT,
        },
        ValueCase {
            name: "50k reject \"not-a-number\"",
            values: vec![ExcelValue::Text("not-a-number".into()); 50_000],
            iters: ITERS_LIGHT,
        },
        ValueCase {
            name: "50k mixed Number/bool/empty/text",
            values: (0..50_000)
                .map(|i| match i % 6 {
                    0 => ExcelValue::Number(-(i as f64)),
                    1 => ExcelValue::Bool(true),
                    2 => ExcelValue::Empty,
                    3 => ExcelValue::Text("-3.5".into()),
                    4 => ExcelValue::Text("x".into()),
                    _ => ExcelValue::Bool(false),
                })
                .collect(),
            iters: ITERS_LIGHT,
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
    println!("ABS kernel bench (branchy compare vs sign-bit clear)");
    println!(
        "{:<40} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(76));

    for c in f64_cases() {
        let mut naive_out = vec![0.0; c.numbers.len()];
        let mut fast_out = vec![0.0; c.numbers.len()];
        let naive = time_it(c.iters, || {
            excel_abs_slice_naive(&c.numbers, &mut naive_out);
            black_box(&naive_out);
        });
        let fast = time_it(c.iters, || {
            excel_abs_slice(&c.numbers, &mut fast_out);
            black_box(&fast_out);
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<40} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        for &n in &c.numbers {
            let a = excel_abs_naive(n);
            let b = excel_abs(n);
            if n == 0.0 {
                assert_eq!(b, 0.0, "semantic mismatch ABS({n})");
            } else {
                assert_eq!(a, b, "semantic mismatch ABS({n})");
            }
        }
    }

    for c in value_cases() {
        let naive = time_it(c.iters, || {
            for v in &c.values {
                let _ = black_box(excel_abs_value_naive(black_box(v)));
            }
        });
        let fast = time_it(c.iters, || {
            for v in &c.values {
                let _ = black_box(excel_abs_value(black_box(v)));
            }
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<40} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        for v in &c.values {
            let a = excel_abs_value_naive(v);
            let b = excel_abs_value(v);
            match (a, b) {
                (ExcelValue::Number(x), ExcelValue::Number(y)) if x == 0.0 && y == 0.0 => {}
                (left, right) => assert_eq!(left, right, "semantic mismatch ABS({v:?})"),
            }
        }
    }
}
