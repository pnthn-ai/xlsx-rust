//! Before/after microbench for Excel `FIXED`.
//!
//! Compares the allocating first-draft kernel (`excel_fixed_naive` /
//! `excel_fixed_apply_naive` / `excel_fixed_slice_naive`) with the
//! production stack-buffer + cheap-ROUND path.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench fixed
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{
    excel_fixed, excel_fixed_apply, excel_fixed_apply_naive, excel_fixed_naive, excel_fixed_slice,
    excel_fixed_slice_naive,
};
use xlsx_types::ExcelValue;

const ITERS_HEAVY: u32 = 40;
const ITERS_LIGHT: u32 = 80;

struct F64Case {
    name: &'static str,
    numbers: Vec<f64>,
    decimals: i32,
    no_commas: bool,
    iters: u32,
}

struct ValueCase {
    name: &'static str,
    numbers: Vec<ExcelValue>,
    decimals: Option<ExcelValue>,
    no_commas: Option<ExcelValue>,
    iters: u32,
}

fn f64_cases() -> Vec<F64Case> {
    let mixed: Vec<f64> = (0..50_000)
        .map(|i| match i % 5 {
            0 => i as f64 * 1.234 + 0.567,
            1 => -(i as f64) * 0.25,
            2 => 1_000_000.0 + i as f64,
            3 => 44.332,
            _ => 1234.567,
        })
        .collect();
    vec![
        F64Case {
            name: "50k FIXED(1234.567, 2) grouped",
            numbers: vec![1234.567; 50_000],
            decimals: 2,
            no_commas: false,
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "50k FIXED(1234.567, 2, TRUE)",
            numbers: vec![1234.567; 50_000],
            decimals: 2,
            no_commas: true,
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "50k Microsoft FIXED(n, 1)",
            numbers: vec![1234.567; 50_000],
            decimals: 1,
            no_commas: false,
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "50k FIXED(n, -1) left of point",
            numbers: vec![1234.567; 50_000],
            decimals: -1,
            no_commas: false,
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "50k mixed magnitude / sign",
            numbers: mixed,
            decimals: 2,
            no_commas: false,
            iters: ITERS_HEAVY,
        },
        F64Case {
            name: "Microsoft FIXED(44.332) × 20k",
            numbers: vec![44.332; 20_000],
            decimals: 2,
            no_commas: false,
            iters: ITERS_LIGHT,
        },
    ]
}

fn value_cases() -> Vec<ValueCase> {
    vec![
        ValueCase {
            name: "20k Number(1234.567) value hot path",
            numbers: vec![ExcelValue::Number(1234.567); 20_000],
            decimals: None,
            no_commas: None,
            iters: ITERS_LIGHT,
        },
        ValueCase {
            name: "20k text \"1234.567\" coerce",
            numbers: vec![ExcelValue::Text("1234.567".into()); 20_000],
            decimals: None,
            no_commas: None,
            iters: ITERS_LIGHT,
        },
        ValueCase {
            name: "20k reject \"not-a-number\"",
            numbers: vec![ExcelValue::Text("not-a-number".into()); 20_000],
            decimals: None,
            no_commas: None,
            iters: ITERS_LIGHT,
        },
        ValueCase {
            name: "20k mixed Number/bool/empty/text",
            numbers: (0..20_000)
                .map(|i| match i % 6 {
                    0 => ExcelValue::Number(i as f64 + 0.567),
                    1 => ExcelValue::Bool(true),
                    2 => ExcelValue::Empty,
                    3 => ExcelValue::Text("44.332".into()),
                    4 => ExcelValue::Text("x".into()),
                    _ => ExcelValue::Bool(false),
                })
                .collect(),
            decimals: Some(ExcelValue::Number(2.0)),
            no_commas: None,
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
    println!("FIXED kernel bench (format!+commas vs stack buffer + cheap ROUND)");
    println!(
        "{:<40} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(76));

    for c in f64_cases() {
        let mut naive_out = vec![String::new(); c.numbers.len()];
        let mut fast_out = vec![String::new(); c.numbers.len()];
        let naive = time_it(c.iters, || {
            excel_fixed_slice_naive(&c.numbers, c.decimals, c.no_commas, &mut naive_out);
            black_box(&naive_out);
        });
        let fast = time_it(c.iters, || {
            excel_fixed_slice(&c.numbers, c.decimals, c.no_commas, &mut fast_out);
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
            let a = excel_fixed_naive(n, c.decimals, c.no_commas);
            let b = excel_fixed(n, c.decimals, c.no_commas);
            assert_eq!(
                a, b,
                "semantic mismatch FIXED({n}, {}, {})",
                c.decimals, c.no_commas
            );
        }
    }

    for c in value_cases() {
        let naive = time_it(c.iters, || {
            for v in &c.numbers {
                let _ = black_box(excel_fixed_apply_naive(
                    black_box(v),
                    c.decimals.as_ref(),
                    c.no_commas.as_ref(),
                ));
            }
        });
        let fast = time_it(c.iters, || {
            for v in &c.numbers {
                let _ = black_box(excel_fixed_apply(
                    black_box(v),
                    c.decimals.as_ref(),
                    c.no_commas.as_ref(),
                ));
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
        for v in &c.numbers {
            let a = excel_fixed_apply_naive(v, c.decimals.as_ref(), c.no_commas.as_ref());
            let b = excel_fixed_apply(v, c.decimals.as_ref(), c.no_commas.as_ref());
            assert_eq!(a, b, "semantic mismatch FIXED({v:?})");
        }
    }
}
