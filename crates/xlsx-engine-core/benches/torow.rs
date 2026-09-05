//! Before/after microbench for Excel `TOROW`.
//!
//! Compares the materializing baseline (`excel_torow_naive`: clone-all +
//! transpose/`retain`) with the production kernel (`excel_torow_apply`: one
//! walk, clone only kept cells, reserved buffer).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench torow
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_torow_apply, excel_torow_naive, TorowIgnore};
use xlsx_types::ExcelValue;

const ITERS: u32 = 40;
const ROWS: usize = 256;
const COLS: usize = 64;

struct Case {
    name: &'static str,
    array: ExcelValue,
    ignore: TorowIgnore,
    scan_by_col: bool,
}

fn n(x: f64) -> ExcelValue {
    ExcelValue::Number(x)
}

fn matrix(rows: usize, cols: usize, cell: impl Fn(usize, usize) -> ExcelValue) -> ExcelValue {
    ExcelValue::Array(
        (0..rows)
            .map(|r| (0..cols).map(|c| cell(r, c)).collect())
            .collect(),
    )
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "256×64 numbers, keep all, by row",
            array: matrix(ROWS, COLS, |r, c| n((r * COLS + c) as f64)),
            ignore: TorowIgnore::KeepAll,
            scan_by_col: false,
        },
        Case {
            name: "256×64 numbers, keep all, by col",
            array: matrix(ROWS, COLS, |r, c| n((r * COLS + c) as f64)),
            ignore: TorowIgnore::KeepAll,
            scan_by_col: true,
        },
        Case {
            name: "256×64 mixed, ignore blanks, by row",
            array: matrix(ROWS, COLS, |r, c| {
                let i = r * COLS + c;
                match i % 4 {
                    0 => ExcelValue::Empty,
                    1 => ExcelValue::Error(xlsx_types::ExcelError::Na),
                    2 => ExcelValue::Text("x".into()),
                    _ => n(i as f64),
                }
            }),
            ignore: TorowIgnore::Blanks,
            scan_by_col: false,
        },
        Case {
            name: "256×64 mixed, ignore blanks+errors, by col",
            array: matrix(ROWS, COLS, |r, c| {
                let i = r * COLS + c;
                match i % 4 {
                    0 => ExcelValue::Empty,
                    1 => ExcelValue::Error(xlsx_types::ExcelError::Na),
                    2 => ExcelValue::Text("x".into()),
                    _ => n(i as f64),
                }
            }),
            ignore: TorowIgnore::BlanksAndErrors,
            scan_by_col: true,
        },
        Case {
            name: "256×64 errors+blanks, ignore both → #CALC!",
            array: matrix(ROWS, COLS, |r, c| {
                if (r + c) % 2 == 0 {
                    ExcelValue::Empty
                } else {
                    ExcelValue::Error(xlsx_types::ExcelError::Div0)
                }
            }),
            ignore: TorowIgnore::BlanksAndErrors,
            scan_by_col: false,
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
    println!("TOROW kernel bench (clone-all/transpose+retain vs one-pass keep)");
    println!(
        "{:<50} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(86));
    for c in cases() {
        let naive = time_it(ITERS, || {
            let _ = black_box(excel_torow_naive(
                black_box(&c.array),
                black_box(c.ignore),
                black_box(c.scan_by_col),
            ));
        });
        let fast = time_it(ITERS, || {
            let _ = black_box(excel_torow_apply(
                black_box(&c.array),
                black_box(c.ignore),
                black_box(c.scan_by_col),
            ));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<50} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_torow_naive(&c.array, c.ignore, c.scan_by_col);
        let b = excel_torow_apply(&c.array, c.ignore, c.scan_by_col);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
