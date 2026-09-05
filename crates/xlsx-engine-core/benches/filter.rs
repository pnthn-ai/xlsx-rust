//! Before/after microbench for Excel `FILTER`.
//!
//! Compares the materializing baseline (`excel_filter_naive`: clone-all +
//! retain / transpose) with the production kernel (`excel_filter`: mask +
//! clone-only-matches).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench filter
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_filter, excel_filter_naive};
use xlsx_types::ExcelValue;

const ITERS: u32 = 40;
const ROWS: usize = 8_192;
const COLS: usize = 8;
const PAYLOAD: usize = 64;

struct Case {
    name: &'static str,
    array: ExcelValue,
    include: ExcelValue,
    if_empty: Option<ExcelValue>,
}

fn text_row(tag: usize) -> Vec<ExcelValue> {
    (0..COLS)
        .map(|c| ExcelValue::Text(format!("{tag:04}-{c}-{}", "x".repeat(PAYLOAD))))
        .collect()
}

fn number_row(tag: usize) -> Vec<ExcelValue> {
    (0..COLS)
        .map(|c| ExcelValue::Number((tag * COLS + c) as f64))
        .collect()
}

fn bool_col(n: usize, pred: impl Fn(usize) -> bool) -> ExcelValue {
    ExcelValue::Array((0..n).map(|i| vec![ExcelValue::Bool(pred(i))]).collect())
}

fn bool_row(n: usize, pred: impl Fn(usize) -> bool) -> ExcelValue {
    ExcelValue::Array(vec![(0..n).map(|i| ExcelValue::Bool(pred(i))).collect()])
}

fn matrix(rows: usize, cell: impl Fn(usize) -> Vec<ExcelValue>) -> ExcelValue {
    ExcelValue::Array((0..rows).map(cell).collect())
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "8k×8 text, keep 1/8 rows",
            array: matrix(ROWS, text_row),
            include: bool_col(ROWS, |i| i % 8 == 0),
            if_empty: None,
        },
        Case {
            name: "8k×8 text, keep all rows",
            array: matrix(ROWS, text_row),
            include: bool_col(ROWS, |_| true),
            if_empty: None,
        },
        Case {
            name: "8k×8 text, keep none → #CALC!",
            array: matrix(ROWS, text_row),
            include: bool_col(ROWS, |_| false),
            if_empty: None,
        },
        Case {
            name: "8k×8 text, keep none → if_empty",
            array: matrix(ROWS, text_row),
            include: bool_col(ROWS, |_| false),
            if_empty: Some(ExcelValue::Text("none".into())),
        },
        Case {
            name: "8k×8 text, column filter keep 1/2",
            array: matrix(ROWS, text_row),
            include: bool_row(COLS, |i| i % 2 == 0),
            if_empty: None,
        },
        Case {
            name: "8k×8 numeric, keep 1/4 rows",
            array: matrix(ROWS, number_row),
            include: bool_col(ROWS, |i| i % 4 == 0),
            if_empty: None,
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
    println!("FILTER kernel bench (clone-all/transpose vs mask + clone-matches)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(80));
    for c in cases() {
        let if_empty = c.if_empty.as_ref();
        let naive = time_it(ITERS, || {
            let _ = black_box(excel_filter_naive(
                black_box(&c.array),
                black_box(&c.include),
                black_box(if_empty),
            ));
        });
        let fast = time_it(ITERS, || {
            let _ = black_box(excel_filter(
                black_box(&c.array),
                black_box(&c.include),
                black_box(if_empty),
            ));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_filter_naive(&c.array, &c.include, if_empty);
        let b = excel_filter(&c.array, &c.include, if_empty);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
