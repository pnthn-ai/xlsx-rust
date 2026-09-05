//! Before/after microbench for Excel `WRAPCOLS`.
//!
//! Compares the materializing baseline (`excel_wrapcols_naive`: clone-all +
//! flatten + column-chunk + transpose) with the production kernel
//! (`excel_wrapcols`: one walk, clone each source cell once).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench wrapcols
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_wrapcols, excel_wrapcols_naive};
use xlsx_types::ExcelValue;

const ITERS: u32 = 40;
const N: usize = 16_384;
const PAYLOAD: usize = 24;

struct Case {
    name: &'static str,
    vector: ExcelValue,
    wrap_count: ExcelValue,
    pad_with: Option<ExcelValue>,
}

fn number_col(n: usize) -> ExcelValue {
    ExcelValue::Array((0..n).map(|i| vec![ExcelValue::Number(i as f64)]).collect())
}

fn text_col(n: usize) -> ExcelValue {
    ExcelValue::Array(
        (0..n)
            .map(|i| vec![ExcelValue::Text(format!("{i:05}-{}", "x".repeat(PAYLOAD)))])
            .collect(),
    )
}

fn number_row(n: usize) -> ExcelValue {
    ExcelValue::Array(vec![(0..n).map(|i| ExcelValue::Number(i as f64)).collect()])
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "16k col, wrap 128 (even)",
            vector: number_col(N),
            wrap_count: ExcelValue::Number(128.0),
            pad_with: None,
        },
        Case {
            name: "16k col, wrap 100 (pad #N/A)",
            vector: number_col(N),
            wrap_count: ExcelValue::Number(100.0),
            pad_with: None,
        },
        Case {
            name: "16k col, wrap 7 + text pad",
            vector: number_col(N),
            wrap_count: ExcelValue::Number(7.0),
            pad_with: Some(ExcelValue::Text("pad".into())),
        },
        Case {
            name: "16k text col, wrap 64",
            vector: text_col(N),
            wrap_count: ExcelValue::Number(64.0),
            pad_with: None,
        },
        Case {
            name: "16k row, wrap 256",
            vector: number_row(N),
            wrap_count: ExcelValue::Number(256.0),
            pad_with: None,
        },
        Case {
            name: "16k col, wrap_count >= n (1 col)",
            vector: number_col(N),
            wrap_count: ExcelValue::Number((N + 1) as f64),
            pad_with: Some(ExcelValue::Text("unused".into())),
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
    println!("WRAPCOLS kernel bench (clone-all/transpose vs single-place)");
    println!(
        "{:<38} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(76));
    for c in cases() {
        let pad = c.pad_with.as_ref();
        let naive = time_it(ITERS, || {
            let _ = black_box(excel_wrapcols_naive(
                black_box(&c.vector),
                black_box(&c.wrap_count),
                black_box(pad),
            ));
        });
        let fast = time_it(ITERS, || {
            let _ = black_box(excel_wrapcols(
                black_box(&c.vector),
                black_box(&c.wrap_count),
                black_box(pad),
            ));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<38} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_wrapcols_naive(&c.vector, &c.wrap_count, pad);
        let b = excel_wrapcols(&c.vector, &c.wrap_count, pad);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
