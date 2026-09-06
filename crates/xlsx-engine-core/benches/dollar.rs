//! Before/after microbench for Excel `DOLLAR`.
//!
//! Compares the `format!` + comma-insert baseline (`excel_dollar_naive` /
//! `excel_dollar_value_naive` / `excel_dollar_slice_naive`) with the
//! production scaled-integer kernel.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench dollar
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{
    excel_dollar, excel_dollar_naive, excel_dollar_slice, excel_dollar_slice_naive,
    excel_dollar_value, excel_dollar_value_naive,
};
use xlsx_types::ExcelValue;

const ITERS_HEAVY: u32 = 40;
const ITERS_LIGHT: u32 = 80;

struct F64Case {
    name: &'static str,
    numbers: Vec<f64>,
    digits: i32,
    iters: u32,
}

struct ValueCase {
    name: &'static str,
    values: Vec<ExcelValue>,
    digits: Option<ExcelValue>,
    iters: u32,
}

fn f64_cases() -> Vec<F64Case> {
    let mixed: Vec<f64> = (0..50_000)
        .map(|i| match i % 5 {
            0 => -(i as f64) - 0.567,
            1 => i as f64 + 0.135,
            2 => 1234.567,
            3 => -0.123,
            _ => 99.888,
        })
        .collect();
    vec![
        F64Case {
            name: "50k DOLLAR(n) default-2 hot path",
            numbers: vec![1234.567; 50_000],
            digits: 2,
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "50k DOLLAR(-n) parens",
            numbers: vec![-1234.567; 50_000],
            digits: 2,
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "50k DOLLAR(n, 0) integer",
            numbers: vec![1234.567; 50_000],
            digits: 0,
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "50k DOLLAR(n, -2) left of decimal",
            numbers: vec![1234.567; 50_000],
            digits: -2,
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "50k mixed sign / frac / Microsoft",
            numbers: mixed,
            digits: 2,
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "Microsoft DOLLAR(99.888) × 20k",
            numbers: vec![99.888; 20_000],
            digits: 2,
            iters: ITERS_LIGHT,
        },
    ]
}

fn value_cases() -> Vec<ValueCase> {
    vec![
        ValueCase {
            name: "20k Number(1234.567) omitted digits",
            values: vec![ExcelValue::Number(1234.567); 20_000],
            digits: None,
            iters: ITERS_LIGHT,
        },
        ValueCase {
            name: "20k text \"1234.5\" coerce",
            values: vec![ExcelValue::Text("1234.5".into()); 20_000],
            digits: None,
            iters: ITERS_LIGHT,
        },
        ValueCase {
            name: "20k reject \"not-a-number\"",
            values: vec![ExcelValue::Text("not-a-number".into()); 20_000],
            digits: None,
            iters: ITERS_LIGHT,
        },
        ValueCase {
            name: "20k mixed Number/bool/empty/text",
            values: (0..20_000)
                .map(|i| match i % 6 {
                    0 => ExcelValue::Number(-(i as f64) - 0.25),
                    1 => ExcelValue::Bool(true),
                    2 => ExcelValue::Empty,
                    3 => ExcelValue::Text("3.5".into()),
                    4 => ExcelValue::Text("x".into()),
                    _ => ExcelValue::Bool(false),
                })
                .collect(),
            digits: None,
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
    println!("DOLLAR kernel bench (format!+commas vs scaled-integer)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));

    for c in f64_cases() {
        let mut naive_out = vec![String::new(); c.numbers.len()];
        let mut fast_out = vec![String::new(); c.numbers.len()];
        let naive = time_it(c.iters, || {
            excel_dollar_slice_naive(&c.numbers, c.digits, &mut naive_out);
            black_box(&naive_out);
        });
        let fast = time_it(c.iters, || {
            excel_dollar_slice(&c.numbers, c.digits, &mut fast_out);
            black_box(&fast_out);
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        for &n in &c.numbers {
            let a = excel_dollar_naive(n, c.digits);
            let b = excel_dollar(n, c.digits);
            assert_eq!(a, b, "semantic mismatch DOLLAR({n}, {})", c.digits);
        }
    }

    for c in value_cases() {
        let naive = time_it(c.iters, || {
            for v in &c.values {
                let _ = black_box(excel_dollar_value_naive(black_box(v), c.digits.as_ref()));
            }
        });
        let fast = time_it(c.iters, || {
            for v in &c.values {
                let _ = black_box(excel_dollar_value(black_box(v), c.digits.as_ref()));
            }
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        for v in &c.values {
            let a = excel_dollar_value_naive(v, c.digits.as_ref());
            let b = excel_dollar_value(v, c.digits.as_ref());
            assert_eq!(a, b, "semantic mismatch DOLLAR({v:?})");
        }
    }
}
