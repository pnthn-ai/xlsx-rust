//! Before/after microbench for Excel `DROP`.
//!
//! Compares the materializing baseline (`excel_drop_naive`: clone-all +
//! drain) with the production kernel (`excel_drop`: clone only the kept
//! rectangle; keep-all can move the grid).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench drop
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_drop, excel_drop_naive};
use xlsx_types::ExcelValue;

const ITERS: u32 = 40;
const ROWS: usize = 8_192;
const COLS: usize = 8;
const PAYLOAD: usize = 64;

struct Case {
    name: &'static str,
    array: ExcelValue,
    rows: f64,
    cols: f64,
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

fn matrix(rows: usize, cell: impl Fn(usize) -> Vec<ExcelValue>) -> ExcelValue {
    ExcelValue::Array((0..rows).map(cell).collect())
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "8k×8 text, drop 1/8 top rows",
            array: matrix(ROWS, text_row),
            rows: (ROWS / 8) as f64,
            cols: 0.0,
        },
        Case {
            name: "8k×8 text, drop 1/8 bottom rows",
            array: matrix(ROWS, text_row),
            rows: -((ROWS / 8) as f64),
            cols: 0.0,
        },
        Case {
            name: "8k×8 text, keep-all (0,0)",
            array: matrix(ROWS, text_row),
            rows: 0.0,
            cols: 0.0,
        },
        Case {
            name: "8k×8 text, drop 2 left cols",
            array: matrix(ROWS, text_row),
            rows: 0.0,
            cols: 2.0,
        },
        Case {
            name: "8k×8 text, drop all rows → #CALC!",
            array: matrix(ROWS, text_row),
            rows: ROWS as f64,
            cols: 0.0,
        },
        Case {
            name: "8k×8 numeric, drop header+footer",
            array: matrix(ROWS, number_row),
            rows: 1.0,
            cols: 0.0,
        },
        Case {
            name: "8k×8 numeric, drop rows+cols mixed",
            array: matrix(ROWS, number_row),
            rows: -64.0,
            cols: 2.0,
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
    println!("DROP kernel bench (clone-all/drain vs clone-kept-rect)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(80));
    for c in cases() {
        let naive = time_it(ITERS, || {
            let _ = black_box(excel_drop_naive(
                black_box(&c.array),
                black_box(c.rows),
                black_box(c.cols),
            ));
        });
        let fast = time_it(ITERS, || {
            let _ = black_box(excel_drop(
                black_box(&c.array),
                black_box(c.rows),
                black_box(c.cols),
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
        let a = excel_drop_naive(&c.array, c.rows, c.cols);
        let b = excel_drop(&c.array, c.rows, c.cols);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
